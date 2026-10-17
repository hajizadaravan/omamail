//! Downloads remote mail images on explicit request ("Load images").
//!
//! Only public addresses are fetched: a mail must not be able to make this
//! machine probe the LAN, a router admin page, or a Tailscale peer. The check
//! lives in the resolver ureq connects with, so the address that was checked
//! is the address that gets dialed (no DNS-rebinding window), and proxies are
//! off so a proxy cannot reach what the check refused.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

const MAX_IMAGES: usize = 40;
const MAX_BYTES: u64 = 5 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const WORKERS: usize = 6;

pub fn fetch_all(urls: &[String]) -> HashMap<String, PathBuf> {
    let urls: Vec<&String> = urls.iter().take(MAX_IMAGES).collect();
    let Ok(dir) = crate::config::private_cache("images") else { return HashMap::new() };
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let config = ureq::Agent::config_builder()
        .tls_config(tls)
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(10)))
        .max_redirects(0)
        .http_status_as_error(false)
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) Omamail")
        .build();
    let agent = ureq::Agent::with_parts(config, DefaultConnector::default(), PublicOnly);
    let found = Mutex::new(HashMap::new());
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..WORKERS.min(urls.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(url) = urls.get(i) else { break };
                    if let Some(path) = cached_or_fetch(&agent, url, &dir) {
                        if let Ok(mut map) = found.lock() {
                            map.insert((*url).clone(), path);
                        }
                    }
                }
            });
        }
    });
    found.into_inner().unwrap_or_default()
}

fn cached_or_fetch(agent: &ureq::Agent, url: &str, dir: &std::path::Path) -> Option<PathBuf> {
    let key = hex(&Sha256::digest(url.as_bytes()));
    for ext in ["png", "jpg", "gif", "webp", "bmp"] {
        let path = dir.join(format!("{key}.{ext}"));
        if path.exists() {
            return Some(path);
        }
    }
    let bytes = download(agent, url)?;
    let ext = match imagesize::image_type(&bytes).ok()? {
        imagesize::ImageType::Png => "png",
        imagesize::ImageType::Jpeg => "jpg",
        imagesize::ImageType::Gif => "gif",
        imagesize::ImageType::Webp => "webp",
        imagesize::ImageType::Bmp => "bmp",
        _ => return None,
    };
    let path = dir.join(format!("{key}.{ext}"));
    crate::config::write_private(&path, &bytes).ok()?;
    Some(path)
}

/// Resolves like ureq's default resolver, then drops every address that is
/// not public. An empty result fails the request before any connection.
#[derive(Debug)]
struct PublicOnly;

impl Resolver for PublicOnly {
    fn resolve(&self, uri: &Uri, config: &Config, timeout: NextTimeout) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let all = DefaultResolver::default().resolve(uri, config, timeout)?;
        let mut public = self.empty();
        for addr in all.iter().filter(|a| is_public_ip(a.ip())) {
            public.push(*addr);
        }
        if public.is_empty() { Err(ureq::Error::HostNotFound) } else { Ok(public) }
    }
}

fn download(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        if host_port(&current).is_none() {
            return None;
        }
        let mut response = agent.get(&current).call().ok()?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            let location = response.headers().get("location")?.to_str().ok()?;
            current = resolve_location(&current, location)?;
            continue;
        }
        if status != 200 {
            return None;
        }
        return response.body_mut().with_config().limit(MAX_BYTES).read_to_vec().ok();
    }
    None
}

fn resolve_location(base: &str, location: &str) -> Option<String> {
    if location.starts_with("https://") || location.starts_with("http://") {
        return Some(location.to_string());
    }
    let (scheme, rest) = base.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    if let Some(stripped) = location.strip_prefix("//") {
        return Some(format!("{scheme}://{stripped}"));
    }
    location.starts_with('/').then(|| format!("{scheme}://{host}{location}"))
}

/// Host and port of an http(s) URL, without userinfo.
fn host_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    if let Some(v6) = authority.strip_prefix('[') {
        let (host, after) = v6.split_once(']')?;
        let port = after.strip_prefix(':').map_or(Some(default_port), |p| p.parse().ok())?;
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok()?)),
        None => Some((authority.to_string(), default_port)),
    }
}

pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || a == 0
                || (a == 100 && (64..=127).contains(&b)) // CGNAT, incl. Tailscale
                || (a == 198 && (b == 18 || b == 19))
                || (a == 192 && b == 0 && c == 0)
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            // IPv4-compatible (::a.b.c.d) addresses carry an IPv4 target.
            if s[..6].iter().all(|x| *x == 0) {
                let v4 = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
                return !v4.is_unspecified() && is_public_ip(IpAddr::V4(v4));
            }
            !(v6.is_loopback()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link local
                || (s[0] & 0xffc0) == 0xfec0 // site local
                || s[0] == 0x2002 // 6to4, can wrap any IPv4
                || (s[0] == 0x0064 && s[1] == 0xff9b) // NAT64, can wrap any IPv4
                || (s[0] == 0x2001 && s[1] == 0x0db8))
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_addresses() {
        for ip in [
            "127.0.0.1", "192.168.1.1", "10.0.0.5", "100.100.100.100", "169.254.169.254", "0.0.0.0",
            "192.0.0.8", "::1", "::", "fe80::1", "fec0::1", "fd00::1", "::ffff:127.0.0.1", "::127.0.0.1",
            "::10.0.0.1", "2002:c0a8:0101::1", "64:ff9b::a00:1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "142.250.185.78", "2a00:1450:4001:80b::200e", "::ffff:8.8.8.8"] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn resolver_refuses_localhost() {
        let agent = ureq::Agent::with_parts(
            ureq::Agent::config_builder().proxy(None).build(),
            DefaultConnector::default(),
            PublicOnly,
        );
        assert!(agent.get("http://localhost:9/x.png").call().is_err());
        assert!(agent.get("http://127.0.0.1:9/x.png").call().is_err());
    }

    #[test]
    fn parses_authority() {
        assert_eq!(host_port("https://a.az/x"), Some(("a.az".into(), 443)));
        assert_eq!(host_port("http://u:p@a.az:8080/x?y"), Some(("a.az".into(), 8080)));
        assert_eq!(host_port("http://[2001:db8::1]:81/"), Some(("2001:db8::1".into(), 81)));
        assert_eq!(host_port("http://a.az:bad/"), None);
    }

    #[test]
    fn resolves_redirects() {
        assert_eq!(resolve_location("https://a.az/p/q", "/img.png").as_deref(), Some("https://a.az/img.png"));
        assert_eq!(resolve_location("https://a.az/p", "//cdn.az/i").as_deref(), Some("https://cdn.az/i"));
        assert_eq!(resolve_location("https://a.az/p", "http://b.az/").as_deref(), Some("http://b.az/"));
        assert_eq!(resolve_location("https://a.az/p", "relative.png"), None);
    }
}
