//! `omamail watch`: the user service behind new-mail notifications.
//!
//! Sits in IMAP IDLE on INBOX and posts one desktop notification per new
//! unread message (one summary when several land at once). Clicking it opens
//! the message in the Omamail window. Only mail that arrives while the
//! watcher runs is announced: the first start records the current UIDNEXT
//! instead of replaying the whole inbox.
//!
//! Notifications are sent straight to `org.freedesktop.Notifications` over
//! the user D-Bus socket, and verification codes are stored in the `0600`
//! watcher state file and piped into `wl-copy`, so senders, subjects and
//! codes never appear in `/proc/<pid>/cmdline`.

use crate::config::{self, state_dir};
use crate::liveness;
use crate::mailbox::{MailError, Mailbox, Summary};
use crate::message;
use crate::otp;
use crate::secret;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const PLUGIN_ID: &str = "reavann.omamail";
const INBOX: &str = "INBOX";
pub(crate) const GLYPH: &str = "\u{f01ee}"; // nf-md-email
const CODE_TTL_SECS: u64 = 30 * 60;
const MAX_STORED_CODES: usize = 10;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SavedCode {
    uid: u32,
    code: String,
    saved_at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// The account the UIDs belong to; a different account starts fresh.
    #[serde(default)]
    email: String,
    uidvalidity: u32,
    last_uid: u32,
    /// Recent (uid, code) pairs for click-to-copy notifications, kept in
    /// this 0600 file so the code never travels in process argv.
    #[serde(default)]
    codes: Vec<SavedCode>,
}

impl State {
    fn prune_codes(&mut self, now: u64) -> bool {
        let before = self.codes.len();
        self.codes.retain(|c| now.saturating_sub(c.saved_at) < CODE_TTL_SECS);
        self.codes.len() != before
    }

    fn remember_code(&mut self, uid: u32, code: String, now: u64) {
        self.prune_codes(now);
        self.codes.retain(|c| c.uid != uid);
        self.codes.push(SavedCode { uid, code, saved_at: now });
        if self.codes.len() > MAX_STORED_CODES {
            self.codes.drain(..self.codes.len() - MAX_STORED_CODES);
        }
    }

    fn code_for(&mut self, uid: u32, now: u64) -> Option<String> {
        self.prune_codes(now);
        self.codes.iter().find(|c| c.uid == uid).map(|c| c.code.clone())
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn state_file() -> PathBuf {
    state_dir().join("watch.json")
}

fn load_state() -> State {
    std::fs::read_to_string(state_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_state(state: &State) {
    if let Ok(text) = serde_json::to_string(state) {
        let _ = config::write_private(&state_file(), text.as_bytes());
    }
}

/// Notification bodies are rendered as markup by the Omarchy shell.
pub(crate) fn markup_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn sender(s: &Summary) -> String {
    s.from
        .first()
        .map(|p| if p.name.is_empty() { p.email.clone() } else { p.name.clone() })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Unknown sender".into())
}

fn omamail_bin() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_else(|| config::home().join(".local/lib/omamail/omamail").to_string_lossy().into_owned())
}

fn notify(summary: String, body: String, open_uid: Option<u32>) {
    let payload = match open_uid {
        Some(uid) => format!(r#"{{"folder":"{INBOX}","uid":{uid}}}"#),
        None => format!(r#"{{"folder":"{INBOX}"}}"#),
    };
    let argv = serde_json::json!(["omarchy-shell", "shell", "summon", PLUGIN_ID, payload]).to_string();
    let hints = vec![("omarchy-glyph".to_string(), GLYPH.to_string()), ("omarchy-exec-argv".to_string(), argv)];
    std::thread::spawn(move || {
        let _ = send_notification("Omamail", &summary, &body, &hints, -1);
    });
}

/// A notification whose click copies `code` instead of opening the mail.
/// `omarchy-exec-argv` only carries the message UID (`omamail copy <uid>`);
/// `copy` reads the code from the `0600` state file and pipes it to `wl-copy`.
fn notify_code(sender: String, domain: &str, subject: &str, code: &str, uid: u32) {
    let argv = serde_json::json!([omamail_bin(), "copy", uid.to_string()]).to_string();
    // The display name is the sender's to choose ("Your Bank"); the domain
    // shows where the mail really came from.
    let body = format!(
        "<b>{}</b>  ·  click to copy\n{}  ·  {}",
        markup_escape(code),
        markup_escape(domain),
        markup_escape(subject)
    );
    let hints = vec![("omarchy-glyph".to_string(), GLYPH.to_string()), ("omarchy-exec-argv".to_string(), argv)];
    std::thread::spawn(move || {
        let _ = send_notification("Omamail", &sender, &body, &hints, -1);
    });
}

/// `omamail copy <uid>`: invoked by clicking a verification-code notification.
/// Pipes the stored code to `wl-copy --sensitive` over stdin so the code never
/// appears in process arguments, and avoids repeating it in the confirmation.
pub fn copy(uid_arg: Option<&str>) -> i32 {
    let Some(uid) = uid_arg.and_then(|s| s.parse::<u32>().ok()) else {
        return 2;
    };
    let mut state = load_state();
    let Some(code) = state.code_for(uid, now_secs()) else {
        return 1;
    };
    if !pipe_to_wl_copy(&code) {
        return 1;
    }
    let _ = send_notification("omarchy-action", "Code copied", "", &[], 2000);
    0
}

fn pipe_to_wl_copy(code: &str) -> bool {
    let Ok(mut child) = Command::new("wl-copy")
        .arg("--sensitive")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let wrote = child.stdin.take().is_some_and(|mut s| s.write_all(code.as_bytes()).is_ok());
    wrote && child.wait().is_ok_and(|s| s.success())
}

// ---- D-Bus org.freedesktop.Notifications.Notify --------------------------

fn unescape_dbus_val(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn bus_socket_path(address: Option<&str>, runtime_dir: Option<&str>, uid: u32) -> PathBuf {
    if let Some(addr) = address {
        for entry in addr.split(';') {
            let Some(rest) = entry.trim().strip_prefix("unix:") else { continue };
            let mut path = None;
            let mut abstract_sock = false;
            for kv in rest.split(',') {
                if let Some(p) = kv.strip_prefix("path=") {
                    path = Some(unescape_dbus_val(p));
                } else if kv.starts_with("abstract=") {
                    abstract_sock = true;
                }
            }
            if !abstract_sock && let Some(p) = path.filter(|p| !p.is_empty()) {
                return PathBuf::from(p);
            }
        }
    }
    if let Some(dir) = runtime_dir.filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("bus");
    }
    PathBuf::from(format!("/run/user/{uid}/bus"))
}

fn session_bus_path() -> PathBuf {
    let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok();
    let uid = unsafe { libc::geteuid() };
    bus_socket_path(addr.as_deref(), runtime.as_deref(), uid)
}

fn pad(buf: &mut Vec<u8>, align: usize) {
    while !buf.len().is_multiple_of(align) {
        buf.push(0);
    }
}

fn clean_dbus_str(s: &str) -> Cow<'_, str> {
    if s.contains('\0') { Cow::Owned(s.replace('\0', "")) } else { Cow::Borrowed(s) }
}

fn put_u32(buf: &mut Vec<u8>, v: u32) {
    pad(buf, 4);
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_i32(buf: &mut Vec<u8>, v: i32) {
    pad(buf, 4);
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_str(buf: &mut Vec<u8>, s: &str) {
    let s = clean_dbus_str(s);
    put_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
}

fn put_header_str(buf: &mut Vec<u8>, code: u8, kind: u8, value: &str) {
    pad(buf, 8);
    buf.extend_from_slice(&[code, 1, kind, 0]);
    put_str(buf, value);
}

fn put_header_sig(buf: &mut Vec<u8>, sig: &str) {
    pad(buf, 8);
    buf.extend_from_slice(&[8, 1, b'g', 0, sig.len() as u8]);
    buf.extend_from_slice(sig.as_bytes());
    buf.push(0);
}

fn dbus_call(serial: u32, dest: &str, path: &str, iface: &str, member: &str, sig: &str, body: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(128 + body.len());
    msg.extend_from_slice(&[b'l', 1, 0, 1]);
    msg.extend_from_slice(&(body.len() as u32).to_le_bytes());
    msg.extend_from_slice(&serial.to_le_bytes());
    msg.extend_from_slice(&0u32.to_le_bytes());
    let fields_start = msg.len();
    put_header_str(&mut msg, 1, b'o', path);
    put_header_str(&mut msg, 2, b's', iface);
    put_header_str(&mut msg, 3, b's', member);
    put_header_str(&mut msg, 6, b's', dest);
    if !sig.is_empty() {
        put_header_sig(&mut msg, sig);
    }
    let fields_len = (msg.len() - fields_start) as u32;
    msg[12..16].copy_from_slice(&fields_len.to_le_bytes());
    pad(&mut msg, 8);
    msg.extend_from_slice(body);
    msg
}

fn notify_body(app: &str, summary: &str, body_text: &str, hints: &[(String, String)], timeout: i32) -> Vec<u8> {
    let mut b = Vec::with_capacity(256 + summary.len() + body_text.len());
    put_str(&mut b, app);
    put_u32(&mut b, 0); // replaces_id
    put_str(&mut b, ""); // app_icon
    put_str(&mut b, summary);
    put_str(&mut b, body_text);
    put_u32(&mut b, 0); // actions: empty as
    put_u32(&mut b, 0); // hints byte length placeholder
    let hints_len_pos = b.len() - 4;
    pad(&mut b, 8); // DICT_ENTRY alignment
    let hints_start = b.len();
    for (k, v) in hints {
        pad(&mut b, 8);
        put_str(&mut b, k);
        b.extend_from_slice(&[1, b's', 0]);
        put_str(&mut b, v);
    }
    let hints_len = (b.len() - hints_start) as u32;
    b[hints_len_pos..hints_len_pos + 4].copy_from_slice(&hints_len.to_le_bytes());
    put_i32(&mut b, timeout);
    b
}

fn sasl_auth(stream: &mut UnixStream) -> std::io::Result<()> {
    let uid = unsafe { libc::geteuid() }.to_string();
    let hex_uid: String = uid.bytes().map(|b| format!("{b:02x}")).collect();
    stream.write_all(format!("\0AUTH EXTERNAL {hex_uid}\r\n").as_bytes())?;
    let mut line = Vec::with_capacity(64);
    let mut byte = [0u8; 1];
    while line.len() < 512 {
        stream.read_exact(&mut byte)?;
        line.push(byte[0]);
        if line.ends_with(b"\r\n") {
            break;
        }
    }
    if !line.starts_with(b"OK ") {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "dbus auth rejected"));
    }
    stream.write_all(b"BEGIN\r\n")
}

fn read_reply(stream: &mut UnixStream) -> std::io::Result<()> {
    for _ in 0..8 {
        let mut head = [0u8; 16];
        stream.read_exact(&mut head)?;
        let msg_type = head[1];
        let (body_len, fields_len) = match head[0] {
            b'l' => (
                u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize,
                u32::from_le_bytes([head[12], head[13], head[14], head[15]]) as usize,
            ),
            b'B' => (
                u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize,
                u32::from_be_bytes([head[12], head[13], head[14], head[15]]) as usize,
            ),
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad dbus endian")),
        };
        let header_end = (16usize.saturating_add(fields_len).saturating_add(7)) & !7;
        let rest = (header_end - 16).saturating_add(body_len);
        if rest > 64 * 1024 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "dbus reply too large"));
        }
        let mut buf = vec![0u8; rest];
        stream.read_exact(&mut buf)?;
        match msg_type {
            2 => return Ok(()),
            3 => return Err(std::io::Error::other("dbus error reply")),
            _ => continue,
        }
    }
    Err(std::io::Error::other("no dbus method return"))
}

fn send_notification_to(
    socket: &Path,
    app: &str,
    summary: &str,
    body_text: &str,
    hints: &[(String, String)],
    timeout: i32,
) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(socket)?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    sasl_auth(&mut stream)?;

    let hello = dbus_call(1, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", "", &[]);
    stream.write_all(&hello)?;
    read_reply(&mut stream)?;

    let body = notify_body(app, summary, body_text, hints, timeout);
    let notify = dbus_call(
        2,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        "susssasa{sv}i",
        &body,
    );
    stream.write_all(&notify)?;
    read_reply(&mut stream)
}

pub(crate) fn send_notification(
    app: &str,
    summary: &str,
    body_text: &str,
    hints: &[(String, String)],
    timeout: i32,
) -> std::io::Result<()> {
    send_notification_to(&session_bus_path(), app, summary, body_text, hints, timeout)
}

// ---- watcher loop --------------------------------------------------------

const IDLE_TIMEOUT: Duration = Duration::from_secs(9 * 60);
/// At most this many body previews per batch: a burst after a resume must not
/// turn into dozens of fetches.
const MAX_PREVIEWS: usize = 3;
const MAX_CODE_NOTIFICATIONS: usize = 3;
/// Retries of a keyring that is not unlocked yet at login.
const KEYRING_TRIES: u32 = 24;
const KEYRING_RETRY: Duration = Duration::from_secs(5);
/// A login refused after earlier ones worked is retried, slowly, for about a
/// day (see `run`).
const AUTH_RETRY_FIRST: Duration = Duration::from_secs(10 * 60);
const AUTH_RETRY_MAX: Duration = Duration::from_secs(60 * 60);
const AUTH_RETRIES: u32 = 30;

fn domain_of(s: &Summary) -> String {
    s.from.first().and_then(|p| p.email.rsplit_once('@')).map(|(_, d)| d.to_string()).unwrap_or_default()
}

fn notify_plain(mail: &[&Summary]) {
    match mail {
        [] => {}
        [one] => notify(sender(one), markup_escape(&one.subject), Some(one.uid)),
        many => {
            let lines: Vec<String> =
                many.iter().take(3).map(|s| markup_escape(&format!("{}: {}", sender(s), s.subject))).collect();
            notify(format!("{} new messages", many.len()), lines.join("\n"), None);
        }
    }
}

/// Codes go to the 0600 state file before their notification: clicking it
/// reads the code from there.
fn notify_codes(state: &mut State, found: &[(&Summary, String)]) {
    if found.is_empty() {
        return;
    }
    let now = now_secs();
    for (s, code) in found {
        state.remember_code(s.uid, code.clone(), now);
    }
    save_state(state);
    for (s, code) in found {
        notify_code(sender(s), &domain_of(s), &s.subject, code, s.uid);
    }
}

/// Most mail states its code in the subject. Otherwise the start of the body
/// is read (without marking it read), but only when the subject reads like a
/// verification mail. Everything that needs no body is announced first, so
/// a slow preview never holds back ordinary mail.
fn announce(mailbox: &mut Mailbox, state: &mut State, new: &[Summary]) {
    let mut with_code: Vec<(&Summary, String)> = Vec::new();
    let mut candidates: Vec<&Summary> = Vec::new();
    let mut plain: Vec<&Summary> = Vec::new();
    for s in new.iter().filter(|s| !s.seen) {
        let room = with_code.len() + candidates.len() < MAX_CODE_NOTIFICATIONS;
        match otp::find_code(&s.subject, "") {
            Some(code) if room => with_code.push((s, code)),
            None if room && candidates.len() < MAX_PREVIEWS && otp::has_keyword(&s.subject) => candidates.push(s),
            _ => plain.push(s),
        }
    }
    notify_codes(state, &with_code);
    notify_plain(&plain);
    if candidates.is_empty() {
        return;
    }
    let uids: Vec<u32> = candidates.iter().map(|s| s.uid).collect();
    let previews = mailbox.fetch_previews(INBOX, &uids).unwrap_or_default();
    let mut found = Vec::new();
    let mut without = Vec::new();
    for s in candidates {
        let body = previews.iter().find(|(uid, _)| *uid == s.uid).map(|(_, raw)| message::plain_text(raw));
        match body.and_then(|text| otp::find_code(&s.subject, &text)) {
            Some(code) => found.push((s, code)),
            None => without.push(s),
        }
    }
    notify_codes(state, &found);
    notify_plain(&without);
}

fn check(mailbox: &mut Mailbox, state: &mut State, email: &str) -> Result<(), MailError> {
    if state.prune_codes(now_secs()) {
        save_state(state);
    }
    let baseline = (state.email == email && state.uidvalidity != 0).then_some((state.uidvalidity, state.last_uid));
    let new = mailbox.poll_new(INBOX, baseline)?;
    if baseline.is_none() || new.uidvalidity != state.uidvalidity {
        // First run, or the server renumbered the folder: start from now.
        state.email = email.to_string();
        state.uidvalidity = new.uidvalidity;
        state.last_uid = new.uidnext.saturating_sub(1);
        state.codes.clear();
        save_state(state);
        return Ok(());
    }
    // Every UID below the SELECT's UIDNEXT existed then and was just fetched
    // (or had already left the inbox): none of them needs looking at again.
    let newest = new.messages.iter().map(|s| s.uid).max().unwrap_or(0);
    let last_uid = state.last_uid.max(newest).max(new.uidnext.saturating_sub(1));
    if last_uid != state.last_uid {
        state.last_uid = last_uid;
        save_state(state);
    }
    if !new.messages.is_empty() {
        announce(mailbox, state, &new.messages);
    }
    Ok(())
}

/// The keyring may still be locked right after login: give it a while.
fn keyring_password(email: &str) -> Option<String> {
    for attempt in 1..=KEYRING_TRIES {
        if let Some(password) = secret::lookup(email) {
            return Some(password);
        }
        if attempt < KEYRING_TRIES {
            std::thread::sleep(KEYRING_RETRY);
        }
    }
    None
}

pub fn run() -> i32 {
    let Some(account) = config::load_account() else {
        eprintln!("omamail watch: no account configured; sign in from the Omamail window first");
        return 2;
    };
    let Some(password) = keyring_password(&account.email) else {
        eprintln!("omamail watch: no password in the keyring for {}", account.email);
        return 2;
    };
    liveness::start();
    let email = account.email.clone();
    let mut mailbox = Mailbox::new(account.clone(), password);
    let mut state = load_state();
    let mut backoff = liveness::Backoff::new(300);
    let mut logged_in = false;
    let mut auth_failures = 0;
    loop {
        let kicks = liveness::kicks();
        let started = Instant::now();
        let result = check(&mut mailbox, &mut state, &email).and_then(|_| {
            logged_in = true;
            auth_failures = 0;
            mailbox.idle(IDLE_TIMEOUT)
        });
        match result {
            Ok(_) => {
                if let Some(wait) = backoff.after_round(started.elapsed()) {
                    liveness::sleep(wait, kicks);
                }
            }
            Err(MailError::Auth) if !logged_in || auth_failures >= AUTH_RETRIES => {
                eprintln!("omamail watch: Gmail rejected the stored app password");
                return 3;
            }
            // It worked before: more likely a passing refusal (throttling, a
            // security check) than a revoked password. Retry slowly, with the
            // password as the keyring has it now.
            Err(MailError::Auth) => {
                auth_failures += 1;
                let wait = AUTH_RETRY_FIRST.saturating_mul(1 << (auth_failures - 1).min(3)).min(AUTH_RETRY_MAX);
                eprintln!("omamail watch: Gmail refused the login; retrying in {} min", wait.as_secs() / 60);
                liveness::sleep(wait, kicks);
                if let Some(password) = secret::lookup(&email) {
                    mailbox = Mailbox::new(account.clone(), password);
                }
            }
            Err(e) => {
                let wait = backoff.next(e.is_offline());
                eprintln!("omamail watch: {e}; retrying in {}s", wait.as_secs());
                if liveness::sleep(wait, kicks) {
                    backoff.reset();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn escapes_markup_in_subjects() {
        assert_eq!(markup_escape("<b>Sale</b> & more"), "&lt;b&gt;Sale&lt;/b&gt; &amp; more");
    }

    #[test]
    fn parses_session_bus_addresses() {
        assert_eq!(
            bus_socket_path(Some("unix:path=/run/user/1000/bus,guid=0123"), None, 1000),
            PathBuf::from("/run/user/1000/bus")
        );
        assert_eq!(
            bus_socket_path(Some("unix:abstract=/tmp/dbus-1;unix:path=/tmp/bus%201"), None, 1000),
            PathBuf::from("/tmp/bus 1")
        );
        assert_eq!(bus_socket_path(None, Some("/run/user/1000"), 1000), PathBuf::from("/run/user/1000/bus"));
        assert_eq!(bus_socket_path(None, None, 1000), PathBuf::from("/run/user/1000/bus"));
    }

    #[test]
    fn stored_codes_expire_and_cap() {
        let mut s = State::default();
        s.remember_code(1, "111111".into(), 100);
        assert_eq!(s.code_for(1, 100 + CODE_TTL_SECS - 1).as_deref(), Some("111111"));
        assert_eq!(s.code_for(1, 100 + CODE_TTL_SECS), None);
        for i in 0..15 {
            s.remember_code(i, format!("{i:06}"), 5000);
        }
        assert_eq!(s.codes.len(), MAX_STORED_CODES);
        assert_eq!(s.code_for(0, 5000), None);
        assert_eq!(s.code_for(14, 5000).as_deref(), Some("000014"));
    }

    #[test]
    fn sends_notify_over_dbus_wire_protocol() {
        let sock = std::env::temp_dir().join(format!("omamail-dbus-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();

        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut auth = Vec::new();
            let mut b = [0u8; 1];
            while !auth.ends_with(b"BEGIN\r\n") {
                conn.read_exact(&mut b).unwrap();
                auth.push(b[0]);
                if auth.ends_with(b"\r\n") && !auth.ends_with(b"BEGIN\r\n") {
                    conn.write_all(b"OK 0123456789abcdef0123456789abcdef\r\n").unwrap();
                }
            }
            assert!(auth.starts_with(b"\0AUTH EXTERNAL "));

            // Read Hello (serial 1) and reply with METHOD_RETURN (type 2).
            let mut head = [0u8; 16];
            conn.read_exact(&mut head).unwrap();
            let fields_len = u32::from_le_bytes([head[12], head[13], head[14], head[15]]) as usize;
            let body_len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
            let rest = ((16 + fields_len + 7) & !7) - 16 + body_len;
            let mut hello_payload = vec![0u8; rest];
            conn.read_exact(&mut hello_payload).unwrap();
            conn.write_all(&[b'l', 2, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]).unwrap();

            // Read Notify (serial 2) and reply with METHOD_RETURN.
            conn.read_exact(&mut head).unwrap();
            let fields_len = u32::from_le_bytes([head[12], head[13], head[14], head[15]]) as usize;
            let body_len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
            let header_rest = ((16 + fields_len + 7) & !7) - 16;
            let mut notify_header = vec![0u8; header_rest];
            let mut notify_body = vec![0u8; body_len];
            conn.read_exact(&mut notify_header).unwrap();
            conn.read_exact(&mut notify_body).unwrap();
            conn.write_all(&[b'l', 2, 1, 1, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]).unwrap();
            notify_body
        });

        let hints = vec![("omarchy-glyph".to_string(), GLYPH.to_string())];
        send_notification_to(&sock, "Omamail", "Sender Name", "Private Subject", &hints, -1).unwrap();
        let body = server.join().unwrap();
        let _ = std::fs::remove_file(&sock);

        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("Omamail"));
        assert!(text.contains("Sender Name"));
        assert!(text.contains("Private Subject"));
        assert!(text.contains("omarchy-glyph"));
    }
}
