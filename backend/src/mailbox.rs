use crate::config::Account;
use crate::liveness;
use crate::utf7;
use imap::types::{Fetch, Flag, NameAttribute, UnsolicitedResponse};
use mail_parser::{Address, MessageParser};
use native_tls::{TlsConnector, TlsStream};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

type Session = imap::Session<Stream>;

const HEADER_FETCH: &str =
    "(UID FLAGS INTERNALDATE BODY.PEEK[HEADER.FIELDS (FROM TO SUBJECT DATE CONTENT-TYPE)])";
const NETWORK_TIMEOUT: Duration = Duration::from_secs(45);
/// Ending IDLE: the connection is dropped right after (see `idle`), so the
/// DONE reply only needs a short wait, and a dead link must not cost 45 s.
const DONE_TIMEOUT: Duration = Duration::from_secs(5);
/// Starting IDLE. "+ idling" comes within a round trip, but when an untagged
/// line ("* 5 EXISTS") arrives first, imap 2.4 reads on until the timeout.
const IDLE_START_TIMEOUT: Duration = Duration::from_secs(10);
/// Verification-code previews hold back their notification.
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(5);

/// Sidebar order. Gmail marks its system folders with RFC 6154 special-use
/// attributes; INBOX is matched by name.
const ROLE_ORDER: [&str; 7] = ["inbox", "flagged", "sent", "drafts", "all", "junk", "trash"];

/// TLS stream whose read timeout never drops to "block forever". imap 2.4
/// clears the timeout only when IDLE ends, right before reading the DONE
/// reply, and the connection is not reused after IDLE: a short timeout there
/// bounds that one read.
pub struct Stream(TlsStream<TcpStream>);

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl imap::extensions::idle::SetReadTimeout for Stream {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> imap::Result<()> {
        self.0
            .get_ref()
            .set_read_timeout(timeout.or(Some(DONE_TIMEOUT)))
            .map_err(imap::Error::Io)
    }
}

#[derive(Debug)]
pub enum MailError {
    Auth,
    /// No connection could be made at all (name lookup, TCP connect): the
    /// request never reached the server.
    Offline(String),
    Network(String),
    Server(String),
    Gone,
}

impl fmt::Display for MailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MailError::Auth => f.write_str(
                "Gmail rejected the login. Check the address and that you used a 16-character app password.",
            ),
            MailError::Offline(e) | MailError::Network(e) => write!(f, "Network error: {e}"),
            MailError::Server(e) => write!(f, "Server error: {e}"),
            MailError::Gone => f.write_str("The message no longer exists on the server."),
        }
    }
}

impl From<imap::Error> for MailError {
    fn from(e: imap::Error) -> Self {
        match e {
            imap::Error::No(msg) | imap::Error::Bad(msg) => MailError::Server(msg),
            other => MailError::Network(other.to_string()),
        }
    }
}

impl MailError {
    fn is_connection(&self) -> bool {
        matches!(self, MailError::Offline(_) | MailError::Network(_))
    }

    pub fn is_offline(&self) -> bool {
        matches!(self, MailError::Offline(_))
    }
}

pub type MailResult<T> = Result<T, MailError>;

#[derive(Clone, Debug, Serialize)]
pub struct Folder {
    pub path: String,
    pub name: String,
    pub role: Option<&'static str>,
    pub unread: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    pub email: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub uid: u32,
    pub from: Vec<Person>,
    #[serde(default)]
    pub to: Vec<Person>,
    pub subject: String,
    pub timestamp: i64,
    pub seen: bool,
    pub flagged: bool,
    pub attachment: bool,
}

#[derive(Debug, Serialize)]
pub struct Page {
    pub messages: Vec<Summary>,
    pub total: u32,
    /// UIDVALIDITY of the selected folder. Zero when this page did not come
    /// from a SELECT. Headers saved under another value are a different
    /// mailbox and must not be mixed into the list.
    pub uidvalidity: u32,
}

/// Sender-controlled text shown next to addresses: drops control and
/// invisible bidi/zero-width characters that could make "<evil@x>" render
/// as something else.
pub fn display_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !is_format_char(*c))
        .collect()
}

pub fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2069}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

pub fn people(address: Option<&Address>) -> Vec<Person> {
    address
        .map(|a| {
            a.iter()
                .map(|addr| Person {
                    name: display_text(addr.name().unwrap_or_default()),
                    email: addr.address().unwrap_or_default().to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn summary(fetch: &Fetch) -> Option<Summary> {
    let uid = fetch.uid?;
    let header = fetch.header().unwrap_or_default();
    let parsed = MessageParser::new().parse_headers(header);
    let flags = fetch.flags();
    let timestamp = fetch
        .internal_date()
        .map(|d| d.timestamp())
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|m| m.date())
                .map(|d| d.to_timestamp())
        })
        .unwrap_or(0);
    let (from, to, subject, mixed) = match &parsed {
        Some(m) => (
            people(m.from()),
            people(m.to()),
            display_text(m.subject().unwrap_or_default()),
            m.root_part().headers.iter().any(|h| {
                h.name.as_str().eq_ignore_ascii_case("content-type")
                    && h.value.as_content_type().is_some_and(|ct| {
                        ct.ctype().eq_ignore_ascii_case("multipart")
                            && ct
                                .subtype()
                                .is_some_and(|s| s.eq_ignore_ascii_case("mixed"))
                    })
            }),
        ),
        None => (vec![], vec![], String::new(), false),
    };
    Some(Summary {
        uid,
        from,
        to,
        subject,
        timestamp,
        seen: flags.contains(&Flag::Seen),
        flagged: flags.contains(&Flag::Flagged),
        attachment: mixed,
    })
}

pub fn uid_set(uids: &[u32]) -> MailResult<String> {
    let mut sorted: Vec<u32> = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.is_empty() || sorted.contains(&0) {
        return Err(MailError::Server("No messages selected".into()));
    }
    Ok(sorted
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(","))
}

/// Line breaks or NUL would end the IMAP command early and start another.
fn quote(value: &str) -> MailResult<String> {
    if value.contains(['\r', '\n', '\0']) {
        return Err(MailError::Server("Invalid characters in name".into()));
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// Parses `* STATUS "name" (UNSEEN 3 UIDNEXT 9)` lines. imap 2.4 routes STATUS
/// data to the unsolicited channel, so the raw response is read instead.
fn status_value(raw: &[u8], key: &str) -> Option<u32> {
    let text = String::from_utf8_lossy(raw);
    let needle = format!("{key} ");
    let line = text.lines().find(|l| l.starts_with("* STATUS"))?;
    let start = line.rfind('(')?;
    let items = &line[start..];
    let pos = items.find(&needle)?;
    items[pos + needle.len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// One IMAP connection. Not shared between threads: each thread owns its own.
pub struct Mailbox {
    account: Account,
    password: String,
    session: Option<Session>,
    selected: Option<String>,
    capabilities: HashSet<String>,
    folders: Option<Vec<Folder>>,
    uidvalidity: u32,
    /// Per-folder sender/subject index for substring search (see `search`).
    indexes: HashMap<String, HeaderIndex>,
    pub exists: u32,
    pub uidnext: u32,
    /// The server announced new messages (EXISTS) in a reply to some other
    /// command since the last SELECT: IDLE would not report them again.
    arrived: bool,
    /// IDLE starts that failed in a row (see `idle`).
    idle_start_failures: u32,
    /// The connection's socket, registered so a resume or network change can
    /// cut a blocked read short (see liveness.rs).
    tracked: Option<liveness::Tracked>,
    /// The address that worked last time, tried first: where IPv6 is
    /// configured but goes nowhere, every reconnect would wait for it.
    preferred: Option<SocketAddr>,
    tls: Option<TlsConnector>,
    capabilities_known: bool,
}

/// New mail in a folder, from one SELECT (see `poll_new`).
pub struct NewMail {
    pub uidvalidity: u32,
    pub uidnext: u32,
    /// Messages with a UID above the given one, oldest first.
    pub messages: Vec<Summary>,
}

impl Mailbox {
    pub fn new(account: Account, password: String) -> Self {
        Mailbox {
            account,
            password,
            session: None,
            selected: None,
            capabilities: HashSet::new(),
            folders: None,
            uidvalidity: 0,
            indexes: HashMap::new(),
            exists: 0,
            uidnext: 0,
            arrived: false,
            idle_start_failures: 0,
            tracked: None,
            preferred: None,
            tls: None,
            capabilities_known: false,
        }
    }

    pub fn connect(&mut self) -> MailResult<()> {
        self.close();
        let host = self.account.imap_host.clone();
        let mut addrs: Vec<SocketAddr> = (host.as_str(), self.account.imap_port)
            .to_socket_addrs()
            .map_err(|e| MailError::Offline(format!("{host}: {e}")))?
            .collect();
        // The address that worked last, then its family, then the rest.
        let preferred = self.preferred;
        addrs.sort_by_key(|a| {
            (
                Some(*a) != preferred,
                preferred.is_some_and(|p| p.is_ipv4() != a.is_ipv4()),
            )
        });
        // Try every address: a network with broken IPv6 must not cost a
        // full timeout before failing.
        let mut last = MailError::Offline(format!("{host}: no address"));
        let mut tcp = None;
        for addr in &addrs {
            match TcpStream::connect_timeout(addr, Duration::from_secs(10)) {
                Ok(t) => {
                    self.preferred = Some(*addr);
                    tcp = Some(t);
                    break;
                }
                Err(e) => last = MailError::Offline(format!("{host}: {e}")),
            }
        }
        let tcp = tcp.ok_or(last)?;
        tcp.set_read_timeout(Some(NETWORK_TIMEOUT)).ok();
        tcp.set_write_timeout(Some(NETWORK_TIMEOUT)).ok();
        liveness::keep_alive(&tcp);
        self.tracked = liveness::Tracked::new(&tcp);
        match self.open_session(&host, tcp) {
            Ok(session) => {
                self.session = Some(session);
                Ok(())
            }
            Err(e) => {
                // Close the registered duplicate too, or the half-open
                // socket lingers until the next attempt.
                self.tracked = None;
                Err(e)
            }
        }
    }

    /// TLS, greeting, LOGIN and (once) CAPABILITY over a fresh connection.
    fn open_session(&mut self, host: &str, tcp: TcpStream) -> MailResult<Session> {
        let tls = match &self.tls {
            Some(tls) => tls.clone(),
            None => {
                // Loads the system CA bundle: once per process is enough.
                let tls = TlsConnector::new().map_err(|e| MailError::Network(e.to_string()))?;
                self.tls = Some(tls.clone());
                tls
            }
        };
        let stream = tls
            .connect(host, tcp)
            .map_err(|e| MailError::Network(e.to_string()))?;
        let mut client = imap::Client::new(Stream(stream));
        client.read_greeting()?;
        let mut session = client
            .login(&self.account.email, &self.password)
            .map_err(|(e, _)| match e {
                imap::Error::No(msg) | imap::Error::Bad(msg) if is_auth_failure(&msg) => {
                    MailError::Auth
                }
                other => MailError::from(other),
            })?;
        // The server's capabilities do not change between connections; asking
        // once saves a round trip on every reconnect.
        if !self.capabilities_known {
            let caps = session.capabilities()?;
            self.capabilities = ["MOVE", "UIDPLUS", "X-GM-EXT-1", "LITERAL+", "LITERAL-"]
                .into_iter()
                .filter(|c| caps.has_str(c))
                .map(String::from)
                .collect();
            self.capabilities_known = true;
        }
        Ok(session)
    }

    pub fn close(&mut self) {
        if let Some(mut s) = self.session.take() {
            // A polite LOGOUT, but not a long wait on a dead connection.
            if let Some(t) = &self.tracked {
                t.set_read_timeout(DONE_TIMEOUT);
            }
            let _ = s.logout();
        }
        self.tracked = None;
        self.selected = None;
    }

    /// Drops a connection whose stream can no longer be trusted, without
    /// talking to it: a late reply to a timed-out command would otherwise be
    /// read as the reply to the next one (imap 2.4 asserts on the tag).
    fn poison(&mut self) {
        self.session = None;
        self.tracked = None;
        self.selected = None;
    }

    /// Clears the unsolicited responses collected during the last command,
    /// remembering whether they announced new messages.
    fn drain_unsolicited(&mut self) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let mut arrived = false;
        while let Ok(r) = s.unsolicited_responses.try_recv() {
            arrived |= matches!(
                r,
                UnsolicitedResponse::Exists(_) | UnsolicitedResponse::Recent(_)
            );
        }
        self.arrived |= arrived;
    }

    fn has_capability(&self, name: &str) -> bool {
        self.capabilities.contains(name)
    }

    /// Runs `op`, reconnecting once if the server dropped the connection.
    fn run<T>(&mut self, mut op: impl FnMut(&mut Self) -> MailResult<T>) -> MailResult<T> {
        if self.session.is_none() {
            self.connect()?;
        }
        let result = match op(self) {
            Err(e) if e.is_connection() => {
                self.poison();
                self.connect()?;
                op(self)
            }
            other => other,
        };
        if matches!(result, Err(ref e) if e.is_connection()) {
            self.poison();
        }
        self.drain_unsolicited();
        result
    }

    fn session(&mut self) -> MailResult<&mut Session> {
        self.session
            .as_mut()
            .ok_or_else(|| MailError::Network("not connected".into()))
    }

    // ------------------------------------------------------------- folders

    pub fn folders(&mut self, with_counts: bool) -> MailResult<Vec<Folder>> {
        self.run(|mb| {
            let names = mb.session()?.list(Some(""), Some("*"))?;
            let mut found: Vec<Folder> = Vec::new();
            for n in names.iter() {
                let attrs: Vec<String> = n
                    .attributes()
                    .iter()
                    .map(|a| match a {
                        NameAttribute::NoSelect => "\\noselect".to_string(),
                        NameAttribute::Custom(c) => c.to_ascii_lowercase(),
                        _ => String::new(),
                    })
                    .collect();
                if attrs
                    .iter()
                    .any(|a| a == "\\noselect" || a == "\\nonexistent")
                {
                    continue;
                }
                let path = n.name().to_string();
                let role = if path.eq_ignore_ascii_case("INBOX") {
                    Some("inbox")
                } else {
                    attrs.iter().find_map(|a| match a.as_str() {
                        "\\flagged" => Some("flagged"),
                        "\\sent" => Some("sent"),
                        "\\drafts" => Some("drafts"),
                        "\\all" => Some("all"),
                        "\\junk" => Some("junk"),
                        "\\trash" => Some("trash"),
                        "\\important" => Some("important"),
                        _ => None,
                    })
                };
                if role == Some("important") {
                    continue;
                }
                let delim = n.delimiter().unwrap_or("/");
                let mut name = if role == Some("inbox") {
                    "Inbox".to_string()
                } else {
                    utf7::decode(&path)
                };
                let gmail_prefix = format!("[Gmail]{delim}");
                if let Some(stripped) = name.strip_prefix(&gmail_prefix) {
                    name = stripped.to_string();
                }
                found.push(Folder {
                    path,
                    name,
                    role,
                    unread: 0,
                });
            }
            found.sort_by_key(|f| {
                (
                    f.role
                        .and_then(|r| ROLE_ORDER.iter().position(|o| *o == r))
                        .unwrap_or(ROLE_ORDER.len()),
                    f.name.to_lowercase(),
                )
            });
            if with_counts {
                for f in found.iter_mut() {
                    if matches!(f.role, Some("inbox") | Some("junk") | None) {
                        f.unread = match mb.status(&f.path, "UNSEEN") {
                            Ok(n) => n,
                            Err(MailError::Server(_)) => 0,
                            Err(e) => return Err(e),
                        };
                    }
                }
            }
            mb.folders = Some(found.clone());
            Ok(found)
        })
    }

    fn status(&mut self, path: &str, key: &str) -> MailResult<u32> {
        let raw = self
            .session()?
            .run_command_and_read_response(format!("STATUS {} ({key})", quote(path)?))?;
        Ok(status_value(&raw, key).unwrap_or(0))
    }

    pub fn unread(&mut self, path: &str) -> MailResult<u32> {
        let path = path.to_string();
        self.run(|mb| mb.status(&path, "UNSEEN"))
    }

    pub fn folder_by_role(&mut self, role: &str) -> MailResult<Option<String>> {
        if self.folders.is_none() {
            self.folders(false)?;
        }
        Ok(self.folders.as_ref().and_then(|fs| {
            fs.iter()
                .find(|f| f.role == Some(role))
                .map(|f| f.path.clone())
        }))
    }

    fn select(&mut self, path: &str, force: bool) -> MailResult<()> {
        if !force && self.selected.as_deref() == Some(path) {
            return Ok(());
        }
        self.selected = None;
        let mailbox = self.session()?.select(path)?;
        self.exists = mailbox.exists;
        self.uidnext = mailbox.uid_next.unwrap_or(0);
        self.uidvalidity = mailbox.uid_validity.unwrap_or(0);
        self.selected = Some(path.to_string());
        Ok(())
    }

    // ------------------------------------------------------------- listing

    /// The newest `limit` messages, or the `limit` messages just older than
    /// `before_uid`. Anchoring on a UID instead of an offset keeps "load
    /// more" right when mail is moved away or arrives between pages.
    pub fn page(&mut self, path: &str, before_uid: u32, limit: u32) -> MailResult<Page> {
        self.run(|mb| {
            // Re-select so EXISTS includes mail that arrived since last time.
            mb.select(path, true)?;
            let total = mb.exists;
            let hi = if before_uid == 0 {
                total
            } else {
                // Plain SEARCH answers with the sequence number of that UID.
                let seqs = mb.session()?.search(format!("UID {before_uid}"))?;
                match seqs.into_iter().next() {
                    Some(seq) => seq.saturating_sub(1),
                    None => {
                        // The anchor itself is gone: fall back to UIDs.
                        let mut older: Vec<u32> = mb
                            .session()?
                            .uid_search(format!("UID 1:{}", before_uid.saturating_sub(1).max(1)))?
                            .into_iter()
                            .filter(|u| *u < before_uid)
                            .collect();
                        older.sort_unstable_by(|a, b| b.cmp(a));
                        older.truncate(limit as usize);
                        return Ok(Page {
                            messages: mb.headers_by_uid(&older)?,
                            total,
                            uidvalidity: mb.uidvalidity,
                        });
                    }
                }
            };
            if hi == 0 {
                return Ok(Page {
                    messages: vec![],
                    total,
                    uidvalidity: mb.uidvalidity,
                });
            }
            let lo = hi.saturating_sub(limit - 1).max(1);
            let fetches = mb.session()?.fetch(format!("{lo}:{hi}"), HEADER_FETCH)?;
            let mut messages: Vec<Summary> = fetches.iter().filter_map(summary).collect();
            messages.sort_by_key(|m| std::cmp::Reverse(m.uid));
            Ok(Page {
                messages,
                total,
                uidvalidity: mb.uidvalidity,
            })
        })
    }

    /// Gmail's search (web syntax, bodies included) plus a local substring
    /// match on sender and subject. Gmail's IMAP search only matches whole
    /// words, so "neuro" alone would never find "NeuroBridge" there; the web
    /// UI does prefix matching that IMAP does not expose.
    pub fn search(&mut self, path: &str, query: &str, limit: usize) -> MailResult<Page> {
        let query: String = query
            .replace(['\r', '\n', '\0'], " ")
            .chars()
            .take(500)
            .collect();
        // Gmail answers a blank query with the whole folder.
        if query.trim().is_empty() {
            return Ok(Page {
                messages: vec![],
                total: 0,
                uidvalidity: 0,
            });
        }
        // Operators (from:, has:attachment, -word) only make sense to Gmail.
        let plain = !query.contains(':') && !query.split_whitespace().any(|w| w.starts_with('-'));
        self.run(|mb| {
            mb.select(path, true)?;
            let key = if mb.has_capability("X-GM-EXT-1") {
                "X-GM-RAW"
            } else {
                "TEXT"
            };
            let criteria = format!("CHARSET UTF-8 {key} {}", mb.astring(&query)?);
            let mut found: HashSet<u32> = mb.session()?.uid_search(criteria)?;
            let words: Vec<String> = fold(&query).split_whitespace().map(String::from).collect();
            if plain && !words.is_empty() {
                mb.update_index(path)?;
                if let Some(index) = mb.indexes.get(path) {
                    found.extend(
                        index
                            .entries
                            .iter()
                            .filter(|(_, hay)| words.iter().all(|w| hay.contains(w.as_str())))
                            .map(|(uid, _)| *uid),
                    );
                }
            }
            let mut uids: Vec<u32> = found.into_iter().collect();
            uids.sort_unstable_by(|a, b| b.cmp(a));
            let total = uids.len() as u32;
            uids.truncate(limit);
            Ok(Page {
                messages: mb.headers_by_uid(&uids)?,
                total,
                uidvalidity: mb.uidvalidity,
            })
        })
    }

    /// Brings the folder's index up to date: the newest INDEX_LIMIT messages
    /// on first use (or after the server renumbered UIDs), then only mail
    /// with a higher UID. Expects `path` to be selected.
    fn update_index(&mut self, path: &str) -> MailResult<()> {
        if self.exists == 0 {
            self.indexes.remove(path);
            return Ok(());
        }
        let validity = self.uidvalidity;
        // An index with no entries (the folder was empty) is rebuilt, never
        // grown from `1:*`, which would fetch the whole folder.
        let current = self
            .indexes
            .get(path)
            .filter(|i| i.uidvalidity == validity && i.max_uid > 0)
            .map(|i| i.max_uid);
        let fetches = match current {
            Some(max) => self
                .session()?
                .uid_fetch(format!("{}:*", max.saturating_add(1)), INDEX_FETCH)?,
            None => {
                let lo = self.exists.saturating_sub(INDEX_LIMIT as u32 - 1).max(1);
                self.session()?.fetch(format!("{lo}:*"), INDEX_FETCH)?
            }
        };
        let mut fresh: Vec<(u32, String)> = Vec::new();
        let floor = current.unwrap_or(0);
        for f in fetches.iter() {
            // `N:*` also returns the highest UID when it is below N.
            let Some(uid) = f.uid.filter(|u| *u > floor) else {
                continue;
            };
            fresh.push((uid, index_text(f.header().unwrap_or_default())));
        }
        drop(fetches);
        // Mail archived, deleted or moved away keeps its UID nowhere in this
        // folder; drop it so it neither counts nor takes a result slot.
        let index = self.indexes.entry(path.to_string()).or_default();
        if current.is_none() {
            *index = HeaderIndex {
                uidvalidity: validity,
                ..HeaderIndex::default()
            };
        }
        index.entries.extend(fresh);
        index.entries.sort_unstable_by_key(|(uid, _)| *uid);
        if index.entries.len() > INDEX_LIMIT {
            let excess = index.entries.len() - INDEX_LIMIT;
            index.entries.drain(..excess);
        }
        index.max_uid = index.entries.last().map_or(0, |(uid, _)| *uid);
        let oldest = index.entries.first().map(|(uid, _)| *uid);
        if let Some(oldest) = oldest {
            let alive = self.session()?.uid_search(format!("UID {oldest}:*"))?;
            if let Some(index) = self.indexes.get_mut(path) {
                index.entries.retain(|(uid, _)| alive.contains(uid));
            }
        }
        Ok(())
    }

    /// A quoted string for ASCII; otherwise a non-synchronizing literal
    /// (RFC 7888), because quoted strings must be 7-bit and Gmail rejects
    /// "Təbriklər" in one. Callers strip CR/LF first.
    fn astring(&self, value: &str) -> MailResult<String> {
        let literal_ok = self.has_capability("LITERAL+")
            || (self.has_capability("LITERAL-") && value.len() <= 4096);
        if value.is_ascii() || !literal_ok {
            quote(value)
        } else {
            Ok(format!("{{{}+}}\r\n{value}", value.len()))
        }
    }

    fn headers_by_uid(&mut self, uids: &[u32]) -> MailResult<Vec<Summary>> {
        if uids.is_empty() {
            return Ok(vec![]);
        }
        let set = uid_set(uids)?;
        let fetches = self.session()?.uid_fetch(set, HEADER_FETCH)?;
        let mut messages: Vec<Summary> = fetches.iter().filter_map(summary).collect();
        messages.sort_by_key(|m| std::cmp::Reverse(m.uid));
        Ok(messages)
    }

    /// Selects `path` and fetches the headers of messages with a UID above
    /// `since.1`, unless `since.0` is not the folder's UIDVALIDITY (the
    /// numbering changed, or there is no baseline yet). One SELECT answers
    /// "anything new?" and leaves the connection ready for `idle`.
    pub fn poll_new(&mut self, path: &str, since: Option<(u32, u32)>) -> MailResult<NewMail> {
        self.run(|mb| {
            mb.arrived = false;
            mb.select(path, true)?;
            let mut messages = Vec::new();
            // Fetched even when UIDNEXT says nothing is new: it costs one
            // round trip, and a UIDNEXT that lags behind would hide new mail.
            if let Some((_, last_uid)) = since.filter(|(validity, _)| *validity == mb.uidvalidity) {
                let fetches = mb
                    .session()?
                    .uid_fetch(format!("{}:*", last_uid.saturating_add(1)), HEADER_FETCH)?;
                // `N:*` always matches the highest UID, even when it is below
                // N, and imap 2.4 mixes unsolicited FETCH responses (flag
                // changes, without headers) into the result.
                messages = fetches
                    .iter()
                    .filter(|f| f.header().is_some())
                    .filter_map(summary)
                    .filter(|m| m.uid > last_uid)
                    .collect();
                messages.sort_by_key(|m| m.uid);
                messages.dedup_by_key(|m| m.uid);
            }
            Ok(NewMail {
                uidvalidity: mb.uidvalidity,
                uidnext: mb.uidnext,
                messages,
            })
        })
    }

    /// Selects `path` and returns (UIDNEXT, EXISTS): enough to tell whether
    /// mail arrived or left since the last look, before `idle`.
    pub fn counts(&mut self, path: &str) -> MailResult<(u32, u32)> {
        self.run(|mb| {
            mb.arrived = false;
            mb.select(path, true)?;
            Ok((mb.uidnext, mb.exists))
        })
    }

    // ------------------------------------------------------------- messages

    /// UIDVALIDITY from the last SELECT on this connection. Zero before that.
    pub fn uidvalidity(&self) -> u32 {
        self.uidvalidity
    }

    /// That value only if `path` is the folder currently selected. Another
    /// folder can use the same number, so a body cached for `path` must not
    /// be trusted just because the number matches.
    pub fn validity_if_selected(&self, path: &str) -> Option<u32> {
        (self.selected.as_deref() == Some(path) && self.uidvalidity != 0)
            .then_some(self.uidvalidity)
    }

    /// UIDVALIDITY changed means these UIDs now name different messages.
    /// `expected == 0` skips the check (callers that have not learned it yet).
    /// Compares the last SELECT; it does not force another round trip.
    fn reject_reused_uids(&self, expected: u32) -> MailResult<()> {
        if expected != 0 && self.uidvalidity != expected {
            Err(MailError::Server(
                "the mailbox changed, so this was not applied".into(),
            ))
        } else {
            Ok(())
        }
    }

    pub fn fetch_raw(
        &mut self,
        path: &str,
        uid: u32,
        mark_seen: bool,
        expected_validity: u32,
    ) -> MailResult<Vec<u8>> {
        self.run(|mb| {
            mb.select(path, false)?;
            mb.reject_reused_uids(expected_validity)?;
            let query = if mark_seen {
                "(UID FLAGS BODY[])"
            } else {
                "(UID FLAGS BODY.PEEK[])"
            };
            let fetches = mb.session()?.uid_fetch(uid.to_string(), query)?;
            let found = fetches
                .iter()
                .find(|f| f.uid == Some(uid))
                .ok_or(MailError::Gone)?;
            let raw = found.body().map(<[u8]>::to_vec).ok_or(MailError::Gone)?;
            Ok(raw)
        })
    }

    /// The first 64 KB of each message in the folder `poll_new` selected,
    /// without marking them read: enough to read a verification code out of a
    /// notification-sized mail. Notifications wait on this, so a slow reply
    /// gets PREVIEW_TIMEOUT and no retry.
    pub fn fetch_previews(&mut self, path: &str, uids: &[u32]) -> MailResult<Vec<(u32, Vec<u8>)>> {
        let set = uid_set(uids)?;
        if self.session.is_none() || self.selected.as_deref() != Some(path) {
            return Err(MailError::Network("folder not selected".into()));
        }
        if let Some(t) = &self.tracked {
            t.set_read_timeout(PREVIEW_TIMEOUT);
        }
        let result = self
            .session()?
            .uid_fetch(set, "(UID BODY.PEEK[]<0.65536>)")
            .map(|fetches| {
                fetches
                    .iter()
                    .filter_map(|f| Some((f.uid?, f.body()?.to_vec())))
                    .filter(|(uid, _)| uids.contains(uid))
                    .collect()
            });
        match result {
            Ok(previews) => {
                if let Some(t) = &self.tracked {
                    t.set_read_timeout(NETWORK_TIMEOUT);
                }
                self.drain_unsolicited();
                Ok(previews)
            }
            Err(e) => {
                // A late reply would be read as the answer to the next command.
                self.poison();
                Err(e.into())
            }
        }
    }

    pub fn set_flag(
        &mut self,
        path: &str,
        uids: &[u32],
        flag: &str,
        on: bool,
        expected_validity: u32,
    ) -> MailResult<()> {
        let flag = match flag {
            "seen" => "\\Seen",
            "flagged" => "\\Flagged",
            other => return Err(MailError::Server(format!("Unsupported flag {other}"))),
        };
        let set = uid_set(uids)?;
        self.run(|mb| {
            mb.select(path, false)?;
            mb.reject_reused_uids(expected_validity)?;
            let op = if on { "+FLAGS.SILENT" } else { "-FLAGS.SILENT" };
            mb.session()?.uid_store(&set, format!("{op} ({flag})"))?;
            Ok(())
        })
    }

    pub fn move_to(
        &mut self,
        path: &str,
        uids: &[u32],
        dest: &str,
        expected_validity: u32,
    ) -> MailResult<()> {
        let set = uid_set(uids)?;
        self.run(|mb| {
            mb.select(path, false)?;
            mb.reject_reused_uids(expected_validity)?;
            if mb.has_capability("MOVE") {
                mb.session()?.uid_mv(&set, dest)?;
                return Ok(());
            }
            mb.session()?.uid_copy(&set, dest)?;
            mb.session()?.uid_store(&set, "+FLAGS.SILENT (\\Deleted)")?;
            if mb.has_capability("UIDPLUS") {
                mb.session()?.uid_expunge(&set)?;
            } else {
                mb.session()?.expunge()?;
            }
            Ok(())
        })
    }

    /// Waits in IDLE on the folder `poll_new` or `counts` just selected, until
    /// the server reports a change (true) or `timeout` passes (false). Returns
    /// true at once when there is nothing to wait on: mail was announced since
    /// the SELECT, or the connection is gone. The caller then looks again.
    ///
    /// No second SELECT between looking and waiting: anything that arrives
    /// after the SELECT is pushed by the server, either in a reply (`arrived`)
    /// or during IDLE.
    pub fn idle(&mut self, timeout: Duration) -> MailResult<bool> {
        if self.session.is_none() || self.selected.is_none() || std::mem::take(&mut self.arrived) {
            return Ok(true);
        }
        if let Some(t) = &self.tracked {
            t.set_read_timeout(IDLE_START_TIMEOUT);
        }
        let started = self
            .session()?
            .idle()
            .map(|handle| handle.wait_with_timeout(timeout));
        // imap 2.4 sends DONE and reads its reply while dropping the IDLE
        // handle, ignoring errors. If that reply is late, the next command
        // would read it and trip imap's tag assertion, so the connection is
        // never reused after IDLE; the next round starts a fresh one.
        self.poison();
        let outcome = match started {
            Ok(outcome) => {
                self.idle_start_failures = 0;
                outcome
            }
            // Most likely mail arrived just as IDLE started ("* 5 EXISTS"
            // before "+ idling"): look again right away. Twice in a row is a
            // real problem and goes through the caller's backoff.
            Err(e) => {
                self.idle_start_failures += 1;
                return if self.idle_start_failures == 1 {
                    Ok(true)
                } else {
                    Err(e.into())
                };
            }
        };
        match outcome? {
            imap::extensions::idle::WaitOutcome::MailboxChanged => Ok(true),
            imap::extensions::idle::WaitOutcome::TimedOut => Ok(false),
        }
    }
}

const INDEX_LIMIT: usize = 20_000;
// At most 2 KB of headers per message: subjects and senders come from
// strangers and the index holds up to INDEX_LIMIT of them.
const INDEX_FETCH: &str = "(UID BODY.PEEK[HEADER.FIELDS (FROM SUBJECT)]<0.2048>)";

/// Folded "subject sender-name sender-email", each part capped.
fn index_text(header: &[u8]) -> String {
    let Some(m) = MessageParser::new().parse_headers(header) else {
        return String::new();
    };
    let mut hay: String = m.subject().unwrap_or_default().chars().take(300).collect();
    for p in people(m.from()).into_iter().take(3) {
        hay.push(' ');
        hay.extend(p.name.chars().take(100));
        hay.push(' ');
        hay.extend(p.email.chars().take(254));
    }
    fold(&hay)
}

#[derive(Default)]
struct HeaderIndex {
    uidvalidity: u32,
    max_uid: u32,
    /// (uid, folded "subject sender-name sender-email"), sorted by uid.
    entries: Vec<(u32, String)>,
}

/// Case-insensitive form for matching, Azerbaijani/Turkish aware: "İş" and
/// "iş" match, and the dotless ı matches i.
fn fold(text: &str) -> String {
    text.to_lowercase().replace('\u{307}', "").replace('ı', "i")
}

/// Gmail answers NO for transient trouble too ("Too many simultaneous
/// connections", "Temporary System Problem"); only credential problems
/// should send the user back to the sign-in screen.
fn is_auth_failure(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    [
        "invalid credentials",
        "authenticationfailed",
        "application-specific password",
        "web browser",
        "web login",
        "password",
    ]
    .iter()
    .any(|k| m.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_status_items() {
        let raw =
            b"* STATUS \"[Gmail]/Spam\" (UIDNEXT 120 UIDVALIDITY 7 UNSEEN 3)\r\nA1 OK done\r\n";
        assert_eq!(status_value(raw, "UNSEEN"), Some(3));
        assert_eq!(status_value(raw, "UIDNEXT"), Some(120));
        assert_eq!(status_value(raw, "UIDVALIDITY"), Some(7));
        assert_eq!(status_value(b"A1 OK\r\n", "UNSEEN"), None);
    }

    #[test]
    fn status_ignores_numbers_in_mailbox_name() {
        let raw = b"* STATUS \"UNSEEN 99 (x)\" (UNSEEN 2)\r\n";
        assert_eq!(status_value(raw, "UNSEEN"), Some(2));
    }

    #[test]
    fn uid_sets_are_sorted_and_validated() {
        assert_eq!(uid_set(&[5, 1, 5, 3]).unwrap(), "1,3,5");
        assert!(uid_set(&[]).is_err());
        assert!(uid_set(&[0]).is_err());
    }

    #[test]
    fn folding_matches_azerbaijani_case() {
        assert!(fold("İş Təklifi NeuroBridge").contains(&fold("iş")));
        assert!(fold("NeuroBridge.SI").contains("neuro"));
        assert!(fold("Qızıl").contains(&fold("qizil")));
    }

    #[test]
    fn only_credential_errors_are_auth() {
        assert!(is_auth_failure("Invalid credentials (Failure)"));
        assert!(is_auth_failure("Application-specific password required"));
        assert!(!is_auth_failure(
            "Too many simultaneous connections. (Failure)"
        ));
        assert!(!is_auth_failure(
            "Temporary System Problem. Try again later"
        ));
    }

    #[test]
    fn a_rebuilt_mailbox_does_not_reuse_uids() {
        let mut mb = Mailbox::new(super::Account::gmail("a@b.c"), String::new());
        assert!(mb.reject_reused_uids(0).is_ok());
        mb.uidvalidity = 4;
        assert!(mb.reject_reused_uids(4).is_ok());
        let err = mb.reject_reused_uids(9).unwrap_err();
        assert!(err.to_string().contains("mailbox changed"));
        assert_eq!(mb.validity_if_selected("INBOX"), None);
        mb.selected = Some("INBOX".into());
        assert_eq!(mb.validity_if_selected("INBOX"), Some(4));
        assert_eq!(mb.validity_if_selected("Sent"), None);
    }

    #[test]
    fn quotes_escape_specials() {
        assert_eq!(quote(r#"a"b\c"#).unwrap(), r#""a\"b\\c""#);
        assert!(quote("INBOX\r\nA1 LOGOUT").is_err());
    }
}
