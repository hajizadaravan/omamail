use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

fn xdg(var: &str, fallback: &str) -> PathBuf {
    let base = std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(fallback));
    base.join("omamail")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state")
}

pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}

/// Creates `dir` under the cache, with the cache root readable only by the
/// user: it holds images from private mail.
pub fn private_cache(sub: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let root = cache_dir();
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let dir = root.join(sub);
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    Ok(dir)
}

pub fn download_dir() -> PathBuf {
    std::env::var_os("XDG_DOWNLOAD_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("Downloads"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub email: String,
    #[serde(default = "default_imap_host")]
    pub imap_host: String,
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    #[serde(default = "default_smtp_host")]
    pub smtp_host: String,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
}

fn default_imap_host() -> String {
    "imap.gmail.com".into()
}
fn default_imap_port() -> u16 {
    993
}
fn default_smtp_host() -> String {
    "smtp.gmail.com".into()
}
fn default_smtp_port() -> u16 {
    465
}

impl Account {
    pub fn gmail(email: &str) -> Self {
        Account {
            email: email.to_string(),
            imap_host: default_imap_host(),
            imap_port: default_imap_port(),
            smtp_host: default_smtp_host(),
            smtp_port: default_smtp_port(),
        }
    }
}

fn account_file() -> PathBuf {
    config_dir().join("config.json")
}

pub fn load_account() -> Option<Account> {
    let text = fs::read_to_string(account_file()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_account(account: &Account) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(account).map_err(std::io::Error::other)?;
    write_private(&account_file(), text.as_bytes())
}

pub fn remove_account() {
    let _ = fs::remove_file(account_file());
    let _ = fs::remove_file(state_dir().join("watch.json"));
}

/// Atomic write, readable only by the user.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    // Unique per writer: two threads may cache the same image at once.
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp.{}.{n}", std::process::id()));
    // A leftover temp file would keep its old mode; start from scratch.
    let _ = fs::remove_file(&tmp);
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    fs::rename(&tmp, path)
}
