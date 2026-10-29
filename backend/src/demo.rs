//! A made-up mailbox for screenshots (`cargo build --features demo`): `serve`
//! answers from the fixtures below and never touches the network or the
//! keyring. Messages go through the real `message::render`, so the reader
//! looks exactly as it does with real mail. Release builds leave this out.

use crate::mailbox::{Folder, Page, Person, Summary};
use crate::message;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

const ME: (&str, &str) = ("Alex Rivera", "alex@example.com");

struct Mail {
    uid: u32,
    from: (&'static str, &'static str),
    subject: &'static str,
    /// Minutes before now.
    ago: i64,
    seen: bool,
    flagged: bool,
    attachment: bool,
    html: &'static str,
}

const INBOX: &[Mail] = &[
    Mail {
        uid: 212,
        from: ("Lumen", "no-reply@lumen.example"),
        subject: "Your Lumen sign-in code",
        ago: 1,
        seen: false,
        flagged: false,
        attachment: false,
        html: "<p>Your verification code is <b>482913</b>.</p><p>It expires in 10 minutes. If you didn't try to sign in, you can ignore this mail.</p>",
    },
    Mail {
        uid: 211,
        from: ("Maya Chen", "maya@studio.example"),
        subject: "Design review moved to Thursday",
        ago: 14,
        seen: false,
        flagged: true,
        attachment: false,
        html: "<p>Hi Alex,</p>\
<p>Quick heads-up: the design review moves to <b>Thursday at 15:00</b>, same room. \
The new mockups are in the shared folder, and I'd love your eyes on the onboarding flow before then.</p>\
<p>Three things I'd like us to settle:</p>\
<ul><li>Sidebar density on small screens</li>\
<li>Whether search lives in the header or the list</li>\
<li>Dark theme contrast for unread rows</li></ul>\
<p>From last week's notes: people loved that the client picks up the desktop theme on its own, \
so let's keep every colour coming from the theme and avoid hard-coded accents.</p>\
<p>Agenda for Thursday:</p>\
<ol><li>Walk through the new onboarding (10 min)</li>\
<li>Keyboard-first navigation: j/k, folders, quick search (15 min)</li>\
<li>Notifications and one-click verification codes (10 min)</li>\
<li>Open questions and next steps</li></ol>\
<p>If Thursday doesn't work for you, just reply and we'll find another slot.</p>\
<p>Thanks!<br>Maya</p>\
<p>--<br>Maya Chen · Product Design<br>studio.example</p>",
    },
    Mail {
        uid: 210,
        from: ("Build Bot", "ci@forge.example"),
        subject: "✓ main passed in 2m 14s",
        ago: 38,
        seen: false,
        flagged: false,
        attachment: false,
        html: "<p>All 66 checks passed on <code>main</code>.</p>",
    },
    Mail {
        uid: 209,
        from: ("Leyla Aliyeva", "leyla@mail.example"),
        subject: "Dinner on Saturday?",
        ago: 95,
        seen: false,
        flagged: false,
        attachment: false,
        html: "<p>We found a new place by the boulevard. Saturday at eight?</p>",
    },
    Mail {
        uid: 208,
        from: ("Northwind Air", "boarding@northwind.example"),
        subject: "Your boarding pass: BAK → IST",
        ago: 180,
        seen: true,
        flagged: true,
        attachment: true,
        html: "<p>Your boarding pass for flight NW 214 is attached. Gate closes 20 minutes before departure.</p>",
    },
    Mail {
        uid: 207,
        from: ("Jonas Weber", "jonas@weber.example"),
        subject: "Photos from the weekend",
        ago: 60 * 20,
        seen: true,
        flagged: false,
        attachment: true,
        html: "<p>Here are the photos from the hike. The one at the lake is my favourite.</p>",
    },
    Mail {
        uid: 206,
        from: ("Priya Raman", "priya@raman.example"),
        subject: "Re: Plugin idea for the shell",
        ago: 60 * 26,
        seen: true,
        flagged: false,
        attachment: false,
        html: "<p>Love it. A mail client that just follows the theme is exactly what I wanted.</p>",
    },
    Mail {
        uid: 205,
        from: ("Hacker Weekly", "editor@weekly.example"),
        subject: "Issue #212: Wayland all the way down",
        ago: 60 * 30,
        seen: true,
        flagged: false,
        attachment: false,
        html: "<p>This week: compositors, tiling, and why your status bar is a protocol now.</p>",
    },
    Mail {
        uid: 204,
        from: ("Kestrel Bank", "statements@kestrel.example"),
        subject: "Your October statement is ready",
        ago: 60 * 50,
        seen: true,
        flagged: false,
        attachment: true,
        html: "<p>Your statement for October is ready to view.</p>",
    },
    Mail {
        uid: 203,
        from: ("Forge", "notifications@forge.example"),
        subject: "[omamail] New issue: keyboard shortcut for labels",
        ago: 60 * 75,
        seen: true,
        flagged: false,
        attachment: false,
        html: "<p>Would be great to jump between labels without the mouse.</p>",
    },
];

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn person((name, email): (&str, &str)) -> Person {
    Person {
        name: name.into(),
        email: email.into(),
    }
}

fn summary(m: &Mail) -> Summary {
    Summary {
        uid: m.uid,
        from: vec![person(m.from)],
        to: vec![person(ME)],
        subject: m.subject.into(),
        timestamp: now() - m.ago * 60,
        seen: m.seen,
        flagged: m.flagged,
        attachment: m.attachment,
    }
}

fn folders() -> Vec<Folder> {
    let unread = INBOX.iter().filter(|m| !m.seen).count() as u32;
    let folder = |path: &str, name: &str, role, unread| Folder {
        path: path.into(),
        name: name.into(),
        role,
        unread,
    };
    vec![
        folder("INBOX", "Inbox", Some("inbox"), unread),
        folder("[Gmail]/Starred", "Starred", Some("flagged"), 0),
        folder("[Gmail]/Sent Mail", "Sent", Some("sent"), 0),
        folder("[Gmail]/Drafts", "Drafts", Some("drafts"), 0),
        folder("[Gmail]/All Mail", "All Mail", Some("all"), 0),
        folder("[Gmail]/Spam", "Spam", Some("junk"), 0),
        folder("[Gmail]/Trash", "Trash", Some("trash"), 0),
        folder("Projects", "Projects", None, 2),
        folder("Receipts", "Receipts", None, 0),
        folder("Travel", "Travel", None, 1),
    ]
}

fn messages(folder: &str) -> Vec<Summary> {
    let keep = |m: &&Mail| match folder {
        "INBOX" | "[Gmail]/All Mail" => true,
        "[Gmail]/Starred" => m.flagged,
        _ => false,
    };
    INBOX.iter().filter(keep).map(summary).collect()
}

fn raw(uid: u32) -> Option<Vec<u8>> {
    let m = INBOX.iter().find(|m| m.uid == uid)?;
    let date = rfc2822(now() - m.ago * 60);
    Some(
        format!(
            "From: {} <{}>\r\nTo: {} <{}>\r\nSubject: {}\r\nDate: {date}\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/html; charset=utf-8\r\n\r\n<html><body>{}</body></html>\r\n",
            m.from.0, m.from.1, ME.0, ME.1, m.subject, m.html
        )
        .into_bytes(),
    )
}

/// RFC 2822 date in UTC, without pulling in a date crate.
fn rfc2822(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{weekday}, {day:02} {} {year} {:02}:{:02}:{:02} +0000",
        months[(month - 1) as usize],
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// The reply to a request. Changes to mail succeed and do nothing; anything
/// that would reach the real account (signing in or out, sending, saving
/// attachments) is refused here. None only for commands the real handler
/// does not know either, so it rejects them.
pub fn dispatch(req: &Value) -> Option<Result<Value, String>> {
    let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or_default();
    let folder = req.get("folder").and_then(Value::as_str).unwrap_or("INBOX");
    let uid = req.get("uid").and_then(Value::as_u64).unwrap_or(0) as u32;
    Some(match cmd {
        "status" => {
            Ok(json!({"configured": true, "email": ME.1, "version": env!("CARGO_PKG_VERSION")}))
        }
        "folders" => Ok(json!(folders())),
        "unread" => Ok(json!(
            folders()
                .iter()
                .find(|f| f.path == folder)
                .map_or(0, |f| f.unread)
        )),
        "list" => {
            let messages = messages(folder);
            Ok(json!(Page {
                total: messages.len() as u32,
                messages,
                uidvalidity: 1
            }))
        }
        "cached" => {
            let messages = messages(folder);
            Ok(json!(Page {
                total: messages.len() as u32,
                messages,
                uidvalidity: 1
            }))
        }
        "search" => {
            let query = req
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase();
            let messages: Vec<Summary> = messages(folder)
                .into_iter()
                .filter(|m| {
                    m.subject.to_lowercase().contains(&query)
                        || m.from[0].name.to_lowercase().contains(&query)
                })
                .collect();
            Ok(json!(Page {
                total: messages.len() as u32,
                messages,
                uidvalidity: 1
            }))
        }
        "open" => raw(uid)
            .map(|raw| json!(message::render(&raw, uid, false)))
            .ok_or_else(|| "No such message".into()),
        "draft" => {
            let mode = req.get("mode").and_then(Value::as_str).unwrap_or("reply");
            raw(uid)
                .and_then(|raw| message::draft(&raw, mode, ME.1))
                .map(|d| json!(d))
                .ok_or_else(|| "No such message".into())
        }
        "flag" | "move" | "undo" => Ok(Value::Null),
        "login" | "logout" | "send" | "attachment" => Err("Not available in the demo build".into()),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_rfc2822() {
        assert_eq!(rfc2822(0), "Thu, 01 Jan 1970 00:00:00 +0000");
        assert_eq!(rfc2822(1_792_682_400), "Thu, 22 Oct 2026 15:20:00 +0000");
    }

    #[test]
    fn every_message_renders() {
        for m in INBOX {
            let view = json!(message::render(&raw(m.uid).unwrap(), m.uid, false));
            assert_eq!(view["subject"], m.subject);
        }
        let page = dispatch(&json!({"cmd": "list", "folder": "INBOX"}))
            .unwrap()
            .unwrap();
        for cmd in ["login", "logout", "send", "attachment"] {
            assert!(
                matches!(dispatch(&json!({"cmd": cmd})), Some(Err(_))),
                "{cmd} must not reach the real handler"
            );
        }
        assert_eq!(page["messages"].as_array().unwrap().len(), INBOX.len());
    }
}
