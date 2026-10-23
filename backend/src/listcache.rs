//! Newest message headers, so the list can paint before IMAP answers.
//!
//! One file, `~/.local/state/omamail/list.json`, mode 0600. It holds
//! subjects and senders — a subject may be a verification code — and never
//! a body or the app password, so it lives with the other state file rather
//! than in the cache. Nothing in it is logged. Sign-out deletes the file.
//! A file that is too large, unreadable, from another account or from
//! another format version is ignored, and text read back is passed through
//! `display_text` because the file is not a trusted source the next time
//! it is opened.

use crate::mailbox::{self, Page, Person, Summary};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How many headers of one folder to keep. The list page is this same
/// window, so a refresh replaces the cached rows in place.
pub const LIMIT: usize = 200;
const MAX_FOLDERS: usize = 16;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct FolderCache {
    uidvalidity: u32,
    total: u32,
    used: u64,
    messages: Vec<Summary>,
}

#[derive(Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    v: u32,
    email: String,
    folders: HashMap<String, FolderCache>,
}

fn path() -> PathBuf {
    crate::config::state_dir().join("list.json")
}

fn empty() -> Page {
    Page {
        messages: vec![],
        total: 0,
        uidvalidity: 0,
    }
}

pub fn load(email: &str, folder: &str) -> Page {
    read_at(&path(), email, folder)
}

pub fn store(email: &str, folder: &str, page: &Page) {
    if email.is_empty() || folder.is_empty() || folder.len() > 1024 {
        return;
    }
    write_at(&path(), email, folder, page);
}

pub fn clear() {
    let _ = fs::remove_file(path());
}

/// Forgets the file when it belongs to another account. Signing in as
/// someone else must not leave the previous subjects on disk until the
/// next list happens to overwrite them.
pub fn retain(email: &str) {
    drop_other(&path(), email);
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Bounds and re-sanitizes headers. Callers may pass any order; the newest
/// UIDs are what the window opens on. `to` is dropped: the list never shows
/// it, and recipient lists do not need to sit on disk.
fn clean(messages: &[Summary]) -> Vec<Summary> {
    let mut ranked: Vec<&Summary> = messages.iter().filter(|m| m.uid != 0).collect();
    ranked.sort_by_key(|m| std::cmp::Reverse(m.uid));
    ranked.dedup_by_key(|m| m.uid);
    ranked.truncate(LIMIT);
    ranked
        .into_iter()
        .map(|m| Summary {
            uid: m.uid,
            from: m
                .from
                .iter()
                .take(4)
                .map(|p| Person {
                    name: clip(&mailbox::display_text(&p.name), 200),
                    email: clip(&mailbox::display_text(&p.email), 254),
                })
                .collect(),
            to: vec![],
            subject: clip(&mailbox::display_text(&m.subject), 500),
            timestamp: m.timestamp,
            seen: m.seen,
            flagged: m.flagged,
            attachment: m.attachment,
        })
        .collect()
}

fn read_store(path: &Path) -> Option<Store> {
    let file = File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut buf = String::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut buf)
        .ok()?;
    if buf.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    let store: Store = serde_json::from_str(&buf).ok()?;
    (store.v == VERSION).then_some(store)
}

fn read_at(path: &Path, email: &str, folder: &str) -> Page {
    let Some(store) = read_store(path).filter(|s| s.email == email) else {
        return empty();
    };
    let Some(folder) = store.folders.get(folder) else {
        return empty();
    };
    let messages = clean(&folder.messages);
    let total = folder.total.max(messages.len() as u32);
    Page {
        messages,
        total,
        uidvalidity: folder.uidvalidity,
    }
}

fn write_at(path: &Path, email: &str, folder: &str, page: &Page) {
    let mut store = read_store(path)
        .filter(|s| s.email == email)
        .unwrap_or(Store {
            v: VERSION,
            email: email.to_string(),
            folders: HashMap::new(),
        });
    store.folders.insert(
        folder.to_string(),
        FolderCache {
            uidvalidity: page.uidvalidity,
            total: page.total,
            used: now(),
            messages: clean(&page.messages),
        },
    );
    while store.folders.len() > MAX_FOLDERS {
        let oldest = store
            .folders
            .iter()
            .filter(|(name, _)| name.as_str() != folder)
            .min_by_key(|(_, f)| f.used)
            .map(|(name, _)| name.clone());
        let Some(oldest) = oldest else { break };
        store.folders.remove(&oldest);
    }
    let Ok(text) = serde_json::to_vec(&store) else {
        return;
    };
    let _ = crate::config::write_private(path, &text);
}

fn drop_other(path: &Path, email: &str) {
    if let Some(store) = read_store(path)
        && store.email != email
    {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn sample(uid: u32, subject: &str) -> Summary {
        Summary {
            uid,
            from: vec![Person {
                name: "Ada".into(),
                email: "ada@ex.com".into(),
            }],
            to: vec![Person {
                name: "hidden".into(),
                email: "hidden@ex.com".into(),
            }],
            subject: subject.into(),
            timestamp: 1_700_000_000,
            seen: true,
            flagged: false,
            attachment: false,
        }
    }

    fn page(messages: Vec<Summary>, total: u32, uidvalidity: u32) -> Page {
        Page {
            messages,
            total,
            uidvalidity,
        }
    }

    fn dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("omamail-list-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn roundtrip_keeps_one_folders_newest_headers() {
        let dir = dir();
        let path = dir.join("list.json");
        let mut messages: Vec<_> = (1..=250).map(|uid| sample(uid, "hello")).collect();
        messages.reverse();
        write_at(&path, "a@ex.com", "INBOX", &page(messages, 9000, 7));
        write_at(
            &path,
            "a@ex.com",
            "[Gmail]/Sent",
            &page(vec![sample(3, "sent")], 3, 2),
        );

        let inbox = read_at(&path, "a@ex.com", "INBOX");
        assert_eq!(inbox.messages.len(), LIMIT);
        assert_eq!(inbox.messages[0].uid, 250);
        assert_eq!(inbox.messages[LIMIT - 1].uid, 250 - LIMIT as u32 + 1);
        assert_eq!(inbox.total, 9000);
        assert_eq!(inbox.uidvalidity, 7);
        assert!(inbox.messages.iter().all(|m| m.to.is_empty()));
        assert_eq!(
            read_at(&path, "a@ex.com", "[Gmail]/Sent").messages[0].uid,
            3
        );
        assert!(read_at(&path, "b@ex.com", "INBOX").messages.is_empty());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // A rebuilt mailbox replaces that folder only.
        write_at(
            &path,
            "a@ex.com",
            "INBOX",
            &page(vec![sample(1, "fresh")], 1, 9),
        );
        let inbox = read_at(&path, "a@ex.com", "INBOX");
        assert_eq!(inbox.uidvalidity, 9);
        assert_eq!(inbox.messages.len(), 1);
        assert_eq!(read_at(&path, "a@ex.com", "[Gmail]/Sent").uidvalidity, 2);

        // The other account replaces the file rather than merging.
        write_at(
            &path,
            "b@ex.com",
            "INBOX",
            &page(vec![sample(4, "other")], 1, 1),
        );
        assert!(read_at(&path, "a@ex.com", "INBOX").messages.is_empty());
        assert_eq!(read_at(&path, "b@ex.com", "INBOX").messages[0].uid, 4);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn disk_text_is_treated_as_hostile() {
        let dir = dir();
        let path = dir.join("list.json");
        let raw = r#"{"v":1,"email":"a@ex.com","folders":{"INBOX":{"uidvalidity":3,"total":1,"used":1,"messages":[
            {"uid":4,"from":[{"name":"A\u0000da","email":"a@b.c"}],"to":[{"name":"secret","email":"s@b.c"}],
             "subject":"pay\u202Eme","timestamp":10,"seen":false,"flagged":true,"attachment":true},
            {"uid":0,"from":[],"subject":"drop","timestamp":0,"seen":false,"flagged":false,"attachment":false},
            {"uid":4,"from":[],"subject":"dup","timestamp":0,"seen":false,"flagged":false,"attachment":false}
        ]}}}"#;
        fs::write(&path, raw).unwrap();
        let loaded = read_at(&path, "a@ex.com", "INBOX");
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].subject, "payme");
        assert_eq!(loaded.messages[0].from[0].name, "Ada");
        assert!(loaded.messages[0].to.is_empty());
        assert!(loaded.messages[0].flagged);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn oversized_broken_and_other_accounts_are_ignored() {
        let dir = dir();
        let path = dir.join("list.json");
        fs::write(&path, vec![b'x'; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(read_at(&path, "a@ex.com", "INBOX").messages.is_empty());
        fs::write(&path, b"not json").unwrap();
        assert!(read_at(&path, "a@ex.com", "INBOX").messages.is_empty());
        fs::write(&path, br#"{"v":99,"email":"a@ex.com","folders":{}}"#).unwrap();
        assert!(read_at(&path, "a@ex.com", "INBOX").messages.is_empty());

        write_at(
            &path,
            "a@ex.com",
            "INBOX",
            &page(vec![sample(2, "keep")], 1, 1),
        );
        drop_other(&path, "a@ex.com");
        assert_eq!(read_at(&path, "a@ex.com", "INBOX").messages[0].uid, 2);
        drop_other(&path, "b@ex.com");
        assert!(read_at(&path, "a@ex.com", "INBOX").messages.is_empty());
        assert!(!path.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_folder_drops_the_least_recently_used() {
        let dir = dir();
        let path = dir.join("list.json");
        let mut folders = HashMap::new();
        for i in 1..=MAX_FOLDERS {
            folders.insert(
                format!("F{i}"),
                FolderCache {
                    uidvalidity: 1,
                    total: 1,
                    used: i as u64,
                    messages: vec![sample(i as u32, "s")],
                },
            );
        }
        let store = Store {
            v: VERSION,
            email: "a@ex.com".into(),
            folders,
        };
        crate::config::write_private(&path, &serde_json::to_vec(&store).unwrap()).unwrap();
        write_at(
            &path,
            "a@ex.com",
            "F-new",
            &page(vec![sample(99, "new")], 1, 1),
        );
        let store = read_store(&path).unwrap();
        assert_eq!(store.folders.len(), MAX_FOLDERS);
        assert!(store.folders.contains_key("F-new"));
        assert!(!store.folders.contains_key("F1"));
        assert!(store.folders.contains_key(&format!("F{MAX_FOLDERS}")));
        let _ = fs::remove_dir_all(dir);
    }
}
