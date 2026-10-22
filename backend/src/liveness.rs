//! Keeps the IMAP connections from going deaf.
//!
//! A connection can die without either side saying so: the laptop slept, the
//! Wi-Fi changed, a VPN came up, a NAT forgot the mapping. A read blocked in
//! IDLE on such a socket would only notice when its 9-minute timeout ran out,
//! and new mail would wait that long. So:
//!
//! - every IMAP socket gets TCP keepalive (`keep_alive`), which turns a silent
//!   death into an error within about a minute and keeps NAT mappings warm;
//! - a resume from suspend (`CLOCK_BOOTTIME` racing ahead of
//!   `CLOCK_MONOTONIC`) or an address appearing or going away (rtnetlink)
//!   `kick`s every registered socket, cutting blocked reads short at once,
//!   and wakes reconnect backoffs (`sleep`), so the next round reconnects
//!   right away and fetches whatever arrived in the meantime.

use std::collections::HashSet;
use std::net::{Shutdown, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

/// Keepalive: first probe after 45 s of silence, then every 10 s; the
/// connection is given up 70 s after the last sign of life, which still
/// tolerates one lost probe.
const KEEPIDLE_SECS: i32 = 45;
const KEEPINTVL_SECS: i32 = 10;
const KEEPCNT: i32 = 3;
const USER_TIMEOUT_MS: i32 = 70_000;

/// How often the suspend detector looks at the clocks, and how much
/// suspended time counts as a resume.
const RESUME_POLL: Duration = Duration::from_secs(4);
const RESUME_MIN: Duration = Duration::from_secs(2);

/// Address changes come in bursts (DHCP, IPv6 autoconfiguration): wait for
/// the burst to end, then reconnect once.
const NETWORK_SETTLE: Duration = Duration::from_millis(1500);
/// At most one network kick this often: every kick means a fresh login, and
/// a flapping network (or forged router advertisements on a shared LAN) must
/// not turn into a login every few seconds. A change within the gap is not
/// dropped, only held until the gap is over.
const NETWORK_KICK_GAP: Duration = Duration::from_secs(30);

/// Reconnect delays once Gmail answered but the round failed, and while
/// there is no network at all. Offline attempts never reach Gmail, so they
/// can be frequent.
const OFFLINE_CAP_SECS: u64 = 30;

static SOCKETS: Mutex<Vec<(u64, TcpStream)>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static KICKS: Mutex<u64> = Mutex::new(0);
static KICKED: Condvar = Condvar::new();

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_int(fd: i32, level: i32, name: i32, value: i32) {
    // Best effort: a kernel without one of these options still works, only
    // notices dead connections later.
    unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            (&value as *const i32).cast(),
            std::mem::size_of::<i32>() as libc::socklen_t,
        );
    }
}

/// Enables TCP keepalive on an IMAP socket (before TLS; the options stay on
/// the socket).
pub fn keep_alive(tcp: &TcpStream) {
    let fd = tcp.as_raw_fd();
    set_int(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE, 1);
    set_int(fd, libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, KEEPIDLE_SECS);
    set_int(fd, libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, KEEPINTVL_SECS);
    set_int(fd, libc::IPPROTO_TCP, libc::TCP_KEEPCNT, KEEPCNT);
    set_int(fd, libc::IPPROTO_TCP, libc::TCP_USER_TIMEOUT, USER_TIMEOUT_MS);
}

/// A connection's socket, registered so `kick` can shut it down from another
/// thread. Dropping it unregisters it and closes the duplicate descriptor, so
/// a dropped connection really closes.
pub struct Tracked {
    id: u64,
}

impl Tracked {
    pub fn new(tcp: &TcpStream) -> Option<Tracked> {
        let clone = tcp.try_clone().ok()?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        lock(&SOCKETS).push((id, clone));
        Some(Tracked { id })
    }

    /// Sets the read timeout of the socket under the TLS stream: imap 2.4
    /// keeps its stream private, but the timeout belongs to the socket.
    pub fn set_read_timeout(&self, timeout: Duration) {
        if let Some((_, s)) = lock(&SOCKETS).iter().find(|(id, _)| *id == self.id) {
            let _ = s.set_read_timeout(Some(timeout));
        }
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        lock(&SOCKETS).retain(|(id, _)| *id != self.id);
    }
}

/// Cuts every registered connection short (blocked reads return at once and
/// their owners reconnect) and wakes everyone in `sleep`.
pub fn kick(reason: &str) {
    for (_, s) in lock(&SOCKETS).iter() {
        let _ = s.shutdown(Shutdown::Both);
    }
    *lock(&KICKS) += 1;
    KICKED.notify_all();
    eprintln!("omamail: {reason}; reconnecting");
}

/// The number of kicks so far. Taken before a round, it lets `sleep` return
/// at once for a kick that happened while the round was failing.
pub fn kicks() -> u64 {
    *lock(&KICKS)
}

/// Sleeps for `duration`, or until a kick newer than `since`. Returns true
/// when it was cut short: the network is probably back, retry now.
pub fn sleep(duration: Duration, since: u64) -> bool {
    let guard = lock(&KICKS);
    let (guard, _) = KICKED.wait_timeout_while(guard, duration, |k| *k == since).unwrap_or_else(|e| e.into_inner());
    *guard != since
}

/// A round (look, then IDLE) shorter than this counts as quick. Mail does
/// end IDLE quickly now and then; several quick rounds in a row mean the
/// server ends it at once (a BYE on every IDLE), and each round is a login.
const QUICK_ROUND: Duration = Duration::from_secs(2);
const QUICK_ROUNDS: u32 = 3;

/// Retry delays: 1 s after a first failure, doubling up to `cap`, but never
/// above 30 s while offline (those attempts cost nothing on Gmail's side).
pub struct Backoff {
    secs: u64,
    cap: u64,
    quick: u32,
}

impl Backoff {
    pub fn new(cap: u64) -> Self {
        Backoff { secs: 0, cap, quick: 0 }
    }

    pub fn reset(&mut self) {
        self.secs = 0;
        self.quick = 0;
    }

    /// After a successful round that took `elapsed`: a delay to wait before
    /// the next one when rounds keep ending at once, else None.
    pub fn after_round(&mut self, elapsed: Duration) -> Option<Duration> {
        if elapsed >= QUICK_ROUND {
            self.reset();
            return None;
        }
        self.quick += 1;
        (self.quick >= QUICK_ROUNDS).then(|| self.next(false))
    }

    pub fn next(&mut self, offline: bool) -> Duration {
        let cap = if offline { self.cap.min(OFFLINE_CAP_SECS) } else { self.cap };
        self.secs = (self.secs * 2).clamp(1, cap);
        Duration::from_secs(self.secs)
    }
}

/// Starts the resume and network-change detectors (once per process).
pub fn start() {
    static START: Once = Once::new();
    START.call_once(|| {
        let _ = std::thread::Builder::new().name("resume".into()).spawn(watch_resume);
        let _ = std::thread::Builder::new().name("netlink".into()).spawn(watch_addresses);
    });
}

// ---- suspend ---------------------------------------------------------------

fn clock(id: libc::clockid_t) -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe {
        libc::clock_gettime(id, &mut ts);
    }
    Duration::new(ts.tv_sec.max(0) as u64, ts.tv_nsec.clamp(0, 999_999_999) as u32)
}

/// Time spent suspended since boot: CLOCK_BOOTTIME counts it,
/// CLOCK_MONOTONIC does not.
fn suspended() -> Duration {
    clock(libc::CLOCK_BOOTTIME).saturating_sub(clock(libc::CLOCK_MONOTONIC))
}

fn resumed(before: Duration, after: Duration) -> bool {
    after.saturating_sub(before) >= RESUME_MIN
}

fn watch_resume() {
    let mut last = suspended();
    loop {
        std::thread::sleep(RESUME_POLL);
        let now = suspended();
        if resumed(last, now) {
            kick("resumed from suspend");
        }
        last = now;
    }
}

// ---- network addresses -----------------------------------------------------

const NLMSG_HDRLEN: usize = 16;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const IFADDRMSG_LEN: usize = 8;
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const RT_SCOPE_UNIVERSE: u8 = 0;

/// (interface index, address) of the global addresses we know about.
type Addresses = HashSet<(u32, Vec<u8>)>;

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

fn ne_u16(b: &[u8]) -> u16 {
    u16::from_ne_bytes([b[0], b[1]])
}

fn ne_u32(b: &[u8]) -> u32 {
    u32::from_ne_bytes([b[0], b[1], b[2], b[3]])
}

/// The address carried by an RTM_NEWADDR/RTM_DELADDR payload (ifaddrmsg +
/// attributes), with its interface, when it is a global one. IFA_LOCAL is the
/// interface's own address on point-to-point links, where IFA_ADDRESS is the
/// peer.
fn global_address(payload: &[u8]) -> Option<(u32, Vec<u8>)> {
    if payload.len() < IFADDRMSG_LEN || payload[3] != RT_SCOPE_UNIVERSE {
        return None;
    }
    let index = ne_u32(&payload[4..8]);
    let (mut local, mut address) = (None, None);
    let mut off = IFADDRMSG_LEN;
    while off + 4 <= payload.len() {
        let len = ne_u16(&payload[off..]) as usize;
        let kind = ne_u16(&payload[off + 2..]);
        if len < 4 || off + len > payload.len() {
            break;
        }
        let data = &payload[off + 4..off + len];
        match kind {
            IFA_LOCAL => local = Some(data),
            IFA_ADDRESS => address = Some(data),
            _ => {}
        }
        off += align4(len);
    }
    local.or(address).map(|a| (index, a.to_vec()))
}

/// Applies a batch of rtnetlink messages to `known`. True when the set of
/// usable addresses changed: an address we did not have appeared, or any
/// global address went away. Re-announcements of known addresses (lease and
/// router-advertisement lifetime refreshes) do not count. With `learn`, new
/// addresses are only recorded (the initial dump).
fn apply(buf: &[u8], known: &mut Addresses, learn: bool) -> (bool, bool) {
    let (mut changed, mut done) = (false, false);
    let mut off = 0;
    while off + NLMSG_HDRLEN <= buf.len() {
        let len = ne_u32(&buf[off..]) as usize;
        let kind = ne_u16(&buf[off + 4..]);
        if len < NLMSG_HDRLEN || off + len > buf.len() {
            break;
        }
        let payload = &buf[off + NLMSG_HDRLEN..off + len];
        match kind {
            NLMSG_DONE | NLMSG_ERROR => done = true,
            k if k == libc::RTM_NEWADDR => {
                if let Some(addr) = global_address(payload) {
                    changed |= known.insert(addr) && !learn;
                }
            }
            k if k == libc::RTM_DELADDR => {
                if let Some(addr) = global_address(payload) {
                    known.remove(&addr);
                    changed = true;
                }
            }
            _ => {}
        }
        off += align4(len);
    }
    (changed, done)
}

fn netlink_socket(groups: u32) -> Option<OwnedFd> {
    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_ROUTE) };
    if fd < 0 {
        return None;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    addr.nl_groups = groups;
    let bound = unsafe {
        libc::bind(
            fd.as_raw_fd(),
            (&addr as *const libc::sockaddr_nl).cast(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    (bound == 0).then_some(fd)
}

fn set_recv_timeout(fd: &OwnedFd, timeout: Option<Duration>) {
    let t = timeout.unwrap_or_default();
    let tv = libc::timeval { tv_sec: t.as_secs() as libc::time_t, tv_usec: t.subsec_micros() as libc::suseconds_t };
    unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&tv as *const libc::timeval).cast(),
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
}

/// Bytes received, or None on a timeout or error.
fn receive(fd: &OwnedFd, buf: &mut [u8]) -> Option<usize> {
    let n = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
    (n > 0).then_some(n as usize)
}

/// The addresses present now, from an RTM_GETADDR dump on its own socket.
fn current_addresses() -> Addresses {
    let mut known = Addresses::new();
    let Some(fd) = netlink_socket(0) else { return known };
    let mut req = [0u8; NLMSG_HDRLEN + IFADDRMSG_LEN];
    req[0..4].copy_from_slice(&((NLMSG_HDRLEN + IFADDRMSG_LEN) as u32).to_ne_bytes());
    req[4..6].copy_from_slice(&libc::RTM_GETADDR.to_ne_bytes());
    req[6..8].copy_from_slice(&((libc::NLM_F_REQUEST | libc::NLM_F_DUMP) as u16).to_ne_bytes());
    req[8..12].copy_from_slice(&1u32.to_ne_bytes());
    if unsafe { libc::send(fd.as_raw_fd(), req.as_ptr().cast(), req.len(), 0) } < 0 {
        return known;
    }
    set_recv_timeout(&fd, Some(Duration::from_secs(2)));
    let mut buf = vec![0u8; 32 * 1024];
    while let Some(n) = receive(&fd, &mut buf) {
        if apply(&buf[..n], &mut known, true).1 {
            break;
        }
    }
    known
}

fn watch_addresses() {
    let groups = (libc::RTMGRP_IPV4_IFADDR | libc::RTMGRP_IPV6_IFADDR) as u32;
    // Subscribe before the dump, so no change falls between the two.
    let Some(fd) = netlink_socket(groups) else { return };
    let mut known = current_addresses();
    let mut buf = vec![0u8; 32 * 1024];
    let mut last_kick: Option<Instant> = None;
    let mut pending = false;
    loop {
        if pending {
            let left = last_kick.map_or(Duration::ZERO, |t| NETWORK_KICK_GAP.saturating_sub(t.elapsed()));
            // A zero timeval would mean "no timeout": treat a sliver as done.
            if left < Duration::from_millis(1) {
                kick("network changed");
                last_kick = Some(Instant::now());
                pending = false;
                continue;
            }
            set_recv_timeout(&fd, Some(left));
        } else {
            set_recv_timeout(&fd, None);
        }
        let n = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n < 0 {
            match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::EINTR) => {}
                // The held-back kick is due.
                Some(e) if e == libc::EAGAIN || e == libc::EWOULDBLOCK => {}
                // The kernel dropped notifications: we may have missed a change.
                Some(libc::ENOBUFS) => {
                    known = current_addresses();
                    pending = true;
                }
                _ => return,
            }
            continue;
        }
        if n == 0 {
            return;
        }
        if !apply(&buf[..n as usize], &mut known, false).0 {
            continue;
        }
        set_recv_timeout(&fd, Some(NETWORK_SETTLE));
        while let Some(n) = receive(&fd, &mut buf) {
            apply(&buf[..n], &mut known, false);
        }
        pending = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    fn rtattr(kind: u16, data: &[u8]) -> Vec<u8> {
        let mut a = Vec::new();
        a.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
        a.extend_from_slice(&kind.to_ne_bytes());
        a.extend_from_slice(data);
        a.resize(align4(a.len()), 0);
        a
    }

    fn addr_msg(kind: u16, scope: u8, index: u32, attrs: &[Vec<u8>]) -> Vec<u8> {
        let mut payload = vec![2, 24, 0, scope];
        payload.extend_from_slice(&index.to_ne_bytes());
        for a in attrs {
            payload.extend_from_slice(a);
        }
        let mut m = Vec::new();
        m.extend_from_slice(&((NLMSG_HDRLEN + payload.len()) as u32).to_ne_bytes());
        m.extend_from_slice(&kind.to_ne_bytes());
        m.extend_from_slice(&[0; 10]);
        m.extend_from_slice(&payload);
        m
    }

    #[test]
    fn counts_only_real_address_changes() {
        let wifi = addr_msg(libc::RTM_NEWADDR, 0, 3, &[rtattr(IFA_ADDRESS, &[192, 168, 1, 7]), rtattr(IFA_LOCAL, &[192, 168, 1, 7])]);
        let link_local = addr_msg(libc::RTM_NEWADDR, 253, 3, &[rtattr(IFA_ADDRESS, &[169, 254, 0, 1])]);
        let mut known = Addresses::new();
        // The initial dump only learns.
        assert_eq!(apply(&wifi, &mut known, true), (false, false));
        // A lease renewal re-announces it: nothing changed.
        assert_eq!(apply(&wifi, &mut known, false).0, false);
        assert_eq!(apply(&link_local, &mut known, false).0, false);
        // A new network's address, in a batch with a refresh.
        let mut batch = wifi.clone();
        batch.extend(addr_msg(libc::RTM_NEWADDR, 0, 3, &[rtattr(IFA_LOCAL, &[10, 0, 0, 9])]));
        assert_eq!(apply(&batch, &mut known, false).0, true);
        // The old one goes away.
        let gone = addr_msg(libc::RTM_DELADDR, 0, 3, &[rtattr(IFA_LOCAL, &[192, 168, 1, 7])]);
        assert_eq!(apply(&gone, &mut known, false).0, true);
        assert_eq!(known.len(), 1);
        // The end of a dump.
        let mut done = vec![0u8; NLMSG_HDRLEN];
        done[0..4].copy_from_slice(&(NLMSG_HDRLEN as u32).to_ne_bytes());
        done[4..6].copy_from_slice(&NLMSG_DONE.to_ne_bytes());
        assert_eq!(apply(&done, &mut known, true), (false, true));
        // Truncated and garbage input is ignored.
        assert_eq!(apply(&batch[..20], &mut known, false), (false, false));
        assert_eq!(apply(&[0xff; 64], &mut known, false), (false, false));
    }

    #[test]
    fn reads_the_current_addresses() {
        // Whatever the machine has; this must just not hang or panic.
        let _ = current_addresses();
    }

    #[test]
    fn detects_suspended_time() {
        assert!(!resumed(Duration::from_secs(100), Duration::from_millis(100_500)));
        assert!(resumed(Duration::from_secs(100), Duration::from_secs(400)));
        assert!(suspended() < clock(libc::CLOCK_BOOTTIME));
    }

    #[test]
    fn backoff_is_quick_offline_and_capped() {
        let mut b = Backoff::new(300);
        let secs: Vec<u64> = (0..7).map(|_| b.next(true).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30]);
        let secs: Vec<u64> = (0..5).map(|_| b.next(false).as_secs()).collect();
        assert_eq!(secs, [60, 120, 240, 300, 300]);
        b.reset();
        assert_eq!(b.next(false).as_secs(), 1);
        b.reset();
        // Mail ends IDLE now and then; a server ending every IDLE at once
        // gets slowed down.
        let quick = Duration::from_millis(300);
        assert_eq!(b.after_round(quick), None);
        assert_eq!(b.after_round(Duration::from_secs(60)), None);
        assert_eq!(b.after_round(quick), None);
        assert_eq!(b.after_round(quick), None);
        assert_eq!(b.after_round(quick), Some(Duration::from_secs(1)));
        assert_eq!(b.after_round(quick), Some(Duration::from_secs(2)));
        assert_eq!(b.after_round(Duration::from_secs(30)), None);
        assert_eq!(b.after_round(quick), None);
    }

    #[test]
    fn kick_cuts_a_blocked_read_and_wakes_sleepers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let _server = listener.accept().unwrap();
        keep_alive(&client);
        let tracked = Tracked::new(&client).unwrap();
        tracked.set_read_timeout(Duration::from_secs(30));
        let since = kicks();
        let sleeper = std::thread::spawn(move || sleep(Duration::from_secs(30), since));
        let reader = std::thread::spawn(move || {
            let mut client = client;
            let start = std::time::Instant::now();
            let n = client.read(&mut [0u8; 16]).unwrap_or(0);
            (n, start.elapsed())
        });
        std::thread::sleep(Duration::from_millis(200));
        kick("test");
        let (n, waited) = reader.join().unwrap();
        assert_eq!(n, 0);
        assert!(waited < Duration::from_secs(5));
        assert!(sleeper.join().unwrap());
        // A kick from before the sleep started also cuts it short.
        assert!(sleep(Duration::from_secs(30), since));
        let id = tracked.id;
        drop(tracked);
        assert!(!lock(&SOCKETS).iter().any(|(i, _)| *i == id));
    }
}
