//! `omamail serve`: the backend the QML window talks to.
//!
//! Protocol: one JSON object per line on stdin/stdout.
//!   request  {"id": 1, "cmd": "list", "folder": "INBOX", "page": 0}
//!   reply    {"id": 1, "ok": true, "data": ...} | {"id": 1, "ok": false, "code": "auth", "error": "..."}
//!   event    {"event": "changed", "folder": "INBOX"}
//! Requests run one at a time on a worker thread that owns the IMAP
//! connection; a second connection sits in IDLE on INBOX to push changes.

use crate::config::{self, Account};
use crate::liveness;
use crate::mailbox::{MailError, Mailbox};
use crate::message;
use crate::secret;
use crate::send::{self, Outgoing};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const PAGE_SIZE: u32 = 50;
const IDLE_TIMEOUT: Duration = Duration::from_secs(9 * 60);
const AUTH_RETRY: Duration = Duration::from_secs(10 * 60);
const RAW_CACHE: usize = 6;

#[derive(Clone)]
struct Out(Arc<Mutex<std::io::Stdout>>);

impl Out {
    fn emit(&self, value: &Value) {
        if let Ok(mut out) = self.0.lock() {
            let _ = writeln!(out, "{value}");
            let _ = out.flush();
        }
    }
}

struct Failure {
    code: &'static str,
    message: String,
}

impl From<MailError> for Failure {
    fn from(e: MailError) -> Self {
        let code = if matches!(e, MailError::Auth) { "auth" } else { "error" };
        Failure { code, message: e.to_string() }
    }
}

fn fail(message: impl Into<String>) -> Failure {
    Failure { code: "error", message: message.into() }
}

type Reply = Result<Value, Failure>;

fn str_arg<'a>(req: &'a Value, key: &str) -> Result<&'a str, Failure> {
    req.get(key).and_then(Value::as_str).ok_or_else(|| fail(format!("missing {key}")))
}

fn u32_arg(req: &Value, key: &str) -> Result<u32, Failure> {
    req.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| fail(format!("missing {key}")))
}

fn uids_arg(req: &Value) -> Result<Vec<u32>, Failure> {
    let list = req.get("uids").and_then(Value::as_array).ok_or_else(|| fail("missing uids"))?;
    list.iter()
        .map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| fail("bad uid")))
        .collect()
}

struct Session {
    account: Account,
    password: String,
    mailbox: Mailbox,
    idle_stop: Arc<AtomicBool>,
}

struct Server {
    out: Out,
    session: Option<Session>,
    raw_cache: VecDeque<((String, u32), Vec<u8>)>,
}

impl Server {
    fn start_session(&mut self, account: Account, password: String) {
        let idle_stop = Arc::new(AtomicBool::new(false));
        spawn_idle(account.clone(), password.clone(), self.out.clone(), Arc::clone(&idle_stop));
        self.session = Some(Session {
            mailbox: Mailbox::new(account.clone(), password.clone()),
            account,
            password,
            idle_stop,
        });
    }

    fn end_session(&mut self) {
        if let Some(mut s) = self.session.take() {
            s.idle_stop.store(true, Ordering::Relaxed);
            s.mailbox.close();
        }
        self.raw_cache.clear();
    }

    fn session(&mut self) -> Result<&mut Session, Failure> {
        self.session.as_mut().ok_or(Failure { code: "not_configured", message: "Sign in first".into() })
    }

    fn raw(&mut self, folder: &str, uid: u32, mark_seen: bool) -> Result<Vec<u8>, Failure> {
        let key = (folder.to_string(), uid);
        if let Some((_, raw)) = self.raw_cache.iter().find(|(k, _)| *k == key) {
            let raw = raw.clone();
            // The UI asks for this only when its row is still unread.
            if mark_seen {
                self.session()?.mailbox.set_flag(folder, &[uid], "seen", true)?;
            }
            return Ok(raw);
        }
        let raw = self.session()?.mailbox.fetch_raw(folder, uid, mark_seen)?;
        if self.raw_cache.len() >= RAW_CACHE {
            self.raw_cache.pop_front();
        }
        self.raw_cache.push_back((key, raw.clone()));
        Ok(raw)
    }

    /// None when the reply is sent later by another thread (see `send`).
    fn handle(&mut self, req: &Value) -> Option<Reply> {
        let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or_default();
        if cmd == "send" {
            return self.send(req);
        }
        if cmd == "open" && req.get("images").and_then(Value::as_bool) == Some(true) {
            return self.open_with_images(req);
        }
        Some(self.dispatch(cmd, req))
    }

    fn dispatch(&mut self, cmd: &str, req: &Value) -> Reply {
        match cmd {
            "status" => Ok(match &self.session {
                Some(s) => json!({"configured": true, "email": s.account.email}),
                None => json!({"configured": false, "email": config::load_account().map(|a| a.email)}),
            }),
            "login" => self.login(req),
            "logout" => {
                watcher("stop");
                if let Some(s) = &self.session {
                    secret::clear(&s.account.email);
                }
                self.end_session();
                config::remove_account();
                Ok(Value::Null)
            }
            "folders" => self.session().and_then(|s| Ok(json!(s.mailbox.folders(true)?))),
            "unread" => {
                let folder = str_arg(req, "folder")?.to_string();
                self.session().and_then(|s| Ok(json!(s.mailbox.unread(&folder)?)))
            }
            "list" => {
                let folder = str_arg(req, "folder")?.to_string();
                let before = u32_arg(req, "before_uid").unwrap_or(0);
                self.session().and_then(|s| Ok(json!(s.mailbox.page(&folder, before, PAGE_SIZE)?)))
            }
            "search" => {
                let folder = str_arg(req, "folder")?.to_string();
                let query = str_arg(req, "query")?.to_string();
                self.session().and_then(|s| Ok(json!(s.mailbox.search(&folder, &query, 100)?)))
            }
            "open" => {
                let folder = str_arg(req, "folder")?.to_string();
                let uid = u32_arg(req, "uid")?;
                let mark = req.get("mark").and_then(Value::as_bool).unwrap_or(true);
                self.raw(&folder, uid, mark).map(|raw| json!(message::render(&raw, uid, false)))
            }
            "flag" => {
                let folder = str_arg(req, "folder")?.to_string();
                let flag = str_arg(req, "flag")?.to_string();
                let on = req.get("on").and_then(Value::as_bool).unwrap_or(true);
                let uids = uids_arg(req)?;
                self.session().and_then(|s| {
                    s.mailbox.set_flag(&folder, &uids, &flag, on)?;
                    Ok(Value::Null)
                })
            }
            "move" => self.move_messages(req),
            "attachment" => self.save_attachment(req),
            "draft" => {
                let folder = str_arg(req, "folder")?.to_string();
                let uid = u32_arg(req, "uid")?;
                let mode = str_arg(req, "mode")?.to_string();
                let me = self.session()?.account.email.clone();
                let raw = self.raw(&folder, uid, false)?;
                message::draft(&raw, &mode, &me).map(|d| json!(d)).ok_or_else(|| fail("Could not read the message"))
            }
            other => Err(fail(format!("unknown command {other}"))),
        }
    }

    fn login(&mut self, req: &Value) -> Reply {
        let email = str_arg(req, "email")?.trim().to_string();
        let password: String = str_arg(req, "password")?.chars().filter(|c| !c.is_whitespace()).collect();
        if !email.contains('@') || email.contains(char::is_whitespace) {
            return Err(fail("Enter your full Gmail address"));
        }
        if password.is_empty() {
            return Err(fail("Enter the app password"));
        }
        let account = Account::gmail(&email);
        let mut probe = Mailbox::new(account.clone(), password.clone());
        probe.connect()?;
        probe.close();
        secret::store(&email, &password).map_err(fail)?;
        config::save_account(&account).map_err(|e| fail(format!("Could not save settings: {e}")))?;
        self.end_session();
        self.start_session(account, password);
        watcher("restart");
        Ok(json!({"email": email}))
    }

    fn move_messages(&mut self, req: &Value) -> Reply {
        let folder = str_arg(req, "folder")?.to_string();
        let uids = uids_arg(req)?;
        let role = str_arg(req, "to")?.to_string();
        let s = self.session()?;
        let dest = s
            .mailbox
            .folder_by_role(&role)?
            .ok_or_else(|| fail(format!("This account has no {role} folder")))?;
        if dest == folder {
            return Err(fail("The message is already there"));
        }
        s.mailbox.move_to(&folder, &uids, &dest)?;
        self.raw_cache.retain(|((f, u), _)| !(f == &folder && uids.contains(u)));
        Ok(Value::Null)
    }

    fn save_attachment(&mut self, req: &Value) -> Reply {
        let folder = str_arg(req, "folder")?.to_string();
        let uid = u32_arg(req, "uid")?;
        let index = u32_arg(req, "index")? as usize;
        let open = req.get("open").and_then(Value::as_bool).unwrap_or(false);
        let raw = self.raw(&folder, uid, false)?;
        let (name, bytes) = message::attachment(&raw, index).ok_or_else(|| fail("Attachment not found"))?;
        let path = write_unique(&config::download_dir(), &name, &bytes)
            .map_err(|e| fail(format!("Could not save {name}: {e}")))?;
        let opened = open && safe_to_open(&path) && content_is_viewable(&path);
        if opened {
            let _ = Command::new("xdg-open")
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
        Ok(json!({"path": path, "opened": opened}))
    }

    /// Downloading up to 40 remote images can take a while; it runs on its
    /// own thread so the worker keeps serving the list and other mail.
    fn open_with_images(&mut self, req: &Value) -> Option<Reply> {
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let folder = match str_arg(req, "folder") {
            Ok(f) => f.to_string(),
            Err(e) => return Some(Err(e)),
        };
        let uid = match u32_arg(req, "uid") {
            Ok(u) => u,
            Err(e) => return Some(Err(e)),
        };
        let raw = match self.raw(&folder, uid, false) {
            Ok(raw) => raw,
            Err(e) => return Some(Err(e)),
        };
        let out = self.out.clone();
        std::thread::spawn(move || {
            let view = message::render(&raw, uid, true);
            out.emit(&json!({"id": id, "ok": true, "data": view}));
        });
        None
    }

    /// Sending runs on its own thread so the window stays usable meanwhile;
    /// it answers the request itself and returns None here.
    fn send(&mut self, req: &Value) -> Option<Reply> {
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let outgoing: Outgoing = match serde_json::from_value(req.clone()) {
            Ok(o) => o,
            Err(e) => return Some(Err(fail(format!("bad message: {e}")))),
        };
        let forwarded = if outgoing.forward_uid > 0 {
            match self.raw(&outgoing.forward_folder, outgoing.forward_uid, false) {
                Ok(raw) => message::all_attachments(&raw),
                Err(e) => return Some(Err(e)),
            }
        } else {
            vec![]
        };
        let (account, password) = match self.session() {
            Ok(s) => (s.account.clone(), s.password.clone()),
            Err(e) => return Some(Err(e)),
        };
        let out = self.out.clone();
        std::thread::spawn(move || {
            let reply = match send::send(&account, &password, &outgoing, forwarded) {
                Ok(()) => json!({"id": id, "ok": true, "data": null}),
                Err(e) => json!({"id": id, "ok": false, "code": "error", "error": e}),
            };
            out.emit(&reply);
        });
        None
    }
}

/// Types handed to xdg-open straight from a mail. Anything that a handler
/// may execute or render with local file access (.desktop, scripts, .jar,
/// .html, .svg) is only saved, never opened.
fn safe_to_open(path: &std::path::Path) -> bool {
    const VIEWABLE: &[&str] = &[
        "pdf", "png", "jpg", "jpeg", "gif", "webp", "bmp", "heic", "txt", "csv", "md", "mp3", "m4a", "ogg", "opus",
        "wav", "flac", "mp4", "m4v", "mkv", "webm", "mov", "avi", "odt", "ods", "odp", "docx", "xlsx", "pptx", "doc",
        "xls", "ppt", "zip", "epub",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIEWABLE.contains(&e.to_ascii_lowercase().as_str()))
}

/// xdg-open picks the program from the file's *content* (a JAR named
/// "invoice.pdf" opens in java), so the content type is checked too, with
/// the same tool xdg-open itself asks.
fn content_is_viewable(path: &std::path::Path) -> bool {
    let Ok(out) = Command::new("xdg-mime").args(["query", "filetype"]).arg(path).stderr(Stdio::null()).output() else {
        return false;
    };
    out.status.success() && viewable_mime(String::from_utf8_lossy(&out.stdout).trim())
}

fn viewable_mime(mime: &str) -> bool {
    const EXACT: &[&str] = &[
        "application/pdf", "image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp", "image/heic",
        "text/plain", "text/csv", "text/markdown", "application/zip", "application/epub+zip", "application/msword",
        "application/vnd.ms-excel", "application/vnd.ms-powerpoint",
    ];
    const PREFIX: &[&str] = &[
        "audio/", "video/", "application/vnd.oasis.opendocument.", "application/vnd.openxmlformats-officedocument.",
    ];
    EXACT.contains(&mime) || PREFIX.iter().any(|p| mime.starts_with(p))
}

/// Best effort: the notification service is optional and may not be installed.
fn watcher(action: &str) {
    let mut cmd = Command::new("systemctl");
    cmd.arg("--user");
    if action != "stop" {
        cmd.arg("--no-block");
    }
    let _ = cmd
        .args([action, "omamail-watch.service"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Writes `bytes` as `name` in `dir`, picking "name (2).ext" etc. when taken.
/// `create_new` means an existing file is never overwritten.
fn write_unique(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1..1000 {
        let candidate = if n == 1 { dir.join(name) } else { dir.join(format!("{stem} ({n}){ext}")) };
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(mut f) => {
                f.write_all(bytes)?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many files with that name"))
}

/// Pushes "changed" for INBOX to the window. Each round looks at INBOX (one
/// SELECT) and then waits in IDLE. A refresh is sent after a wake (which may
/// be a flag change made elsewhere) or when the counts moved since the last
/// look: mail that arrived while not in IDLE, at startup or across a
/// reconnect, is caught that way. The first round always refreshes once.
fn spawn_idle(account: Account, password: String, out: Out, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut mailbox = Mailbox::new(account, password);
        let mut backoff = liveness::Backoff::new(120);
        let mut seen: Option<(u32, u32)> = None;
        let mut woke = true;
        while !stop.load(Ordering::Relaxed) {
            let kicks = liveness::kicks();
            let started = Instant::now();
            let result = mailbox.counts("INBOX").and_then(|now| {
                if (woke || seen != Some(now)) && !stop.load(Ordering::Relaxed) {
                    out.emit(&json!({"event": "changed", "folder": "INBOX"}));
                }
                seen = Some(now);
                mailbox.idle(IDLE_TIMEOUT)
            });
            match result {
                Ok(changed) => {
                    woke = changed;
                    if let Some(wait) = backoff.after_round(started.elapsed()) {
                        liveness::sleep(wait, kicks);
                    }
                }
                // The worker's requests report a rejected login to the window;
                // here, keep trying now and then in case it passes.
                Err(MailError::Auth) => {
                    liveness::sleep(AUTH_RETRY, kicks);
                }
                Err(e) => {
                    // Changes may have been missed while disconnected.
                    woke = true;
                    if liveness::sleep(backoff.next(e.is_offline()), kicks) {
                        backoff.reset();
                    }
                }
            }
        }
        mailbox.close();
    });
}

pub fn run() {
    liveness::start();
    let out = Out(Arc::new(Mutex::new(std::io::stdout())));
    let (tx, rx) = mpsc::channel::<Value>();
    let worker_out = out.clone();
    std::thread::spawn(move || {
        let mut server = Server { out: worker_out.clone(), session: None, raw_cache: VecDeque::new() };
        if let Some(account) = config::load_account() {
            if let Some(password) = secret::lookup(&account.email) {
                server.start_session(account, password);
            }
        }
        for req in rx {
            let id = req.get("id").cloned().unwrap_or(Value::Null);
            let Some(reply) = server.handle(&req) else { continue };
            let value = match reply {
                Ok(data) => json!({"id": id, "ok": true, "data": data}),
                Err(f) => json!({"id": id, "ok": false, "code": f.code, "error": f.message}),
            };
            worker_out.emit(&value);
        }
    });

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(req) => {
                if tx.send(req).is_err() {
                    break;
                }
            }
            Err(e) => out.emit(&json!({"id": null, "ok": false, "code": "error", "error": format!("bad request: {e}")})),
        }
    }
    // stdin closed: the window is gone, so the process ends with it.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_viewable_types_open() {
        use std::path::Path;
        assert!(safe_to_open(Path::new("/d/report.PDF")));
        assert!(safe_to_open(Path::new("/d/clip.mp4")));
        for bad in ["x.desktop", "x.sh", "x.jar", "x.html", "x.svg", "x.AppImage", "noext", "pdf"] {
            assert!(!safe_to_open(Path::new(bad)), "{bad}");
        }
    }

    #[test]
    fn content_types_that_run_code_are_refused() {
        for ok in ["application/pdf", "video/mp4", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"] {
            assert!(viewable_mime(ok), "{ok}");
        }
        for bad in [
            "application/java-archive", "application/x-desktop", "text/x-shellscript", "text/html",
            "image/svg+xml", "application/x-executable", "",
        ] {
            assert!(!viewable_mime(bad), "{bad}");
        }
    }

    #[test]
    fn unique_names_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("omamail-dl-{}", std::process::id()));
        let a = write_unique(&dir, "report.pdf", b"1").unwrap();
        let b = write_unique(&dir, "report.pdf", b"2").unwrap();
        let c = write_unique(&dir, ".hidden", b"3").unwrap();
        assert_eq!(a.file_name().unwrap(), "report.pdf");
        assert_eq!(b.file_name().unwrap(), "report (2).pdf");
        assert_eq!(c.file_name().unwrap(), ".hidden");
        assert_eq!(std::fs::read(&a).unwrap(), b"1");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
