//! `omamail update-check`: run once a day by `omamail-update-check.timer`.
//!
//! Fetches the upstream of the plugin's git checkout and, when it has new
//! commits, posts one notification per new upstream head. Nothing is
//! installed here, and nothing in the plugin folder is written: the shell
//! watches that tree, so the fetch goes into a bare repository in the cache
//! that borrows the checkout's objects. Clicking the notification opens a
//! terminal running `update.sh`, which goes through `omarchy plugin update`
//! (the diff to review and accept) before it rebuilds anything.

use crate::config::{self, state_dir};
use crate::watch::{GLYPH, PLUGIN_ID, markup_escape, send_notification};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_SUBJECTS: usize = 3;
const MAX_SUBJECT_CHARS: usize = 100;

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// The upstream commit the last notification was about.
    #[serde(default)]
    notified: String,
}

#[derive(Debug, PartialEq, Eq)]
struct Pending {
    upstream: String,
    behind: usize,
    subjects: Vec<String>,
}

fn plugin_dir() -> PathBuf {
    config::home().join(".config/omarchy/plugins").join(PLUGIN_ID)
}

fn state_file() -> PathBuf {
    state_dir().join("update.json")
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        // Set when run from a git hook; it would override -C.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        // Runs unattended: a remote that wants credentials must fail here,
        // not prompt on the terminal, through askpass or through ssh.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Where the checked-out branch comes from: (remote URL, upstream ref).
/// None for a detached HEAD or a branch without an upstream.
fn upstream_of(plugin: &Path) -> Option<(String, String)> {
    let branch = git(plugin, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let remote = git(plugin, &["config", "--get", &format!("branch.{branch}.remote")])?;
    let merge = git(plugin, &["config", "--get", &format!("branch.{branch}.merge")])?;
    let url = git(plugin, &["remote", "get-url", &remote])?;
    Some((url, merge))
}

/// Fetches the upstream branch into `cache` (a bare repository whose
/// alternates point at the checkout's objects) as `refs/upstream`.
fn fetch_upstream(plugin: &Path, cache: &Path, url: &str, merge: &str) -> Option<()> {
    if !cache.join("HEAD").exists() {
        git(cache, &["init", "--bare", "--quiet"])?;
    }
    let objects = plugin.join(".git/objects");
    std::fs::write(cache.join("objects/info/alternates"), format!("{}\n", objects.display())).ok()?;
    git(cache, &["fetch", "--quiet", "--no-tags", "--end-of-options", url, &format!("+{merge}:refs/upstream")]).map(|_| ())
}

/// What the fetched upstream has that `head` lacks. None when up to date.
fn pending(cache: &Path, head: &str) -> Option<Pending> {
    let range = format!("{head}..refs/upstream");
    let upstream = git(cache, &["rev-parse", "--verify", "--quiet", "refs/upstream"])?;
    let behind: usize = git(cache, &["rev-list", "--count", &range])?.parse().ok()?;
    if behind == 0 {
        return None;
    }
    let max = format!("--max-count={MAX_SUBJECTS}");
    let subjects = git(cache, &["log", &max, "--format=%s", &range])
        .map(|log| log.lines().map(str::to_string).collect())
        .unwrap_or_default();
    Some(Pending { upstream, behind, subjects })
}

/// Subjects come from the remote: keep them short and on one line, without
/// control, bidi or invisible characters that could make them read as
/// something else (a fake extra line, reversed text).
fn clean_subject(s: &str) -> String {
    let hidden = |c: char| {
        matches!(c, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
    };
    let kept: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|&c| !c.is_control() && !hidden(c))
        .collect();
    match kept.char_indices().nth(MAX_SUBJECT_CHARS) {
        Some((i, _)) => format!("{}…", &kept[..i]),
        None => kept,
    }
}

fn body(p: &Pending) -> String {
    let mut lines: Vec<String> =
        p.subjects.iter().map(|s| markup_escape(&format!("· {}", clean_subject(s)))).collect();
    if p.behind > p.subjects.len() {
        lines.push(format!("· and {} more", p.behind - p.subjects.len()));
    }
    lines.push("Click to review the changes and install.".into());
    lines.join("\n")
}

pub fn check() -> i32 {
    let dir = plugin_dir();
    // Installed from a checkout elsewhere (a copy without .git): the
    // developer updates that checkout themselves.
    if !dir.join(".git").exists() {
        return 0;
    }
    let (Some((url, merge)), Some(head)) = (upstream_of(&dir), git(&dir, &["rev-parse", "HEAD"])) else {
        return 0;
    };
    let fetched = config::private_cache("update.git").ok().filter(|cache| fetch_upstream(&dir, cache, &url, &merge).is_some());
    let Some(cache) = fetched else {
        eprintln!("omamail update-check: could not fetch updates for {}", dir.display());
        return 1;
    };
    let Some(p) = pending(&cache, &head) else {
        return 0;
    };
    let mut state: State =
        std::fs::read_to_string(state_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if state.notified == p.upstream {
        return 0;
    }
    let script = dir.join("update.sh").to_string_lossy().into_owned();
    let argv = serde_json::json!(["xdg-terminal-exec", script, "--pause"]).to_string();
    let hints = vec![("omarchy-glyph".to_string(), GLYPH.to_string()), ("omarchy-exec-argv".to_string(), argv)];
    let summary = if p.behind == 1 { "Omamail update available".to_string() } else { format!("Omamail: {} updates available", p.behind) };
    if let Err(e) = send_notification("Omamail", &summary, &body(&p), &hints, -1) {
        eprintln!("omamail update-check: could not post the notification: {e}");
        return 1;
    }
    state.notified = p.upstream;
    if let Ok(text) = serde_json::to_string(&state) {
        let _ = config::write_private(&state_file(), text.as_bytes());
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn finds_upstream_commits() {
        let root = std::env::temp_dir().join(format!("omamail-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let origin = root.join("origin");
        let clone = root.join("clone");
        let cache = root.join("cache");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        run(&origin, &["init", "--quiet"]);
        run(&origin, &["commit", "--quiet", "--allow-empty", "-m", "first"]);
        run(&root, &["clone", "--quiet", origin.to_str().unwrap(), clone.to_str().unwrap()]);
        let (url, merge) = upstream_of(&clone).unwrap();
        assert_eq!(merge, "refs/heads/main");
        let head = git(&clone, &["rev-parse", "HEAD"]).unwrap();
        fetch_upstream(&clone, &cache, &url, &merge).unwrap();
        assert_eq!(pending(&cache, &head), None);

        run(&origin, &["commit", "--quiet", "--allow-empty", "-m", "Fix <b>sync</b>"]);
        run(&origin, &["commit", "--quiet", "--allow-empty", "-m", "Add search"]);
        let before = std::fs::read_dir(clone.join(".git")).unwrap().count();
        fetch_upstream(&clone, &cache, &url, &merge).unwrap();
        let p = pending(&cache, &head).unwrap();
        assert_eq!(p.behind, 2);
        assert_eq!(p.subjects, ["Add search", "Fix <b>sync</b>"]);
        assert_eq!(Some(p.upstream.clone()), git(&origin, &["rev-parse", "HEAD"]));
        // The checkout was not touched: no FETCH_HEAD, its refs where they were.
        assert_eq!(std::fs::read_dir(clone.join(".git")).unwrap().count(), before);
        assert!(!clone.join(".git/FETCH_HEAD").exists());
        assert_eq!(git(&clone, &["rev-parse", "origin/main"]), Some(head.clone()));
        assert_eq!(git(&clone, &["log", "-1", "--format=%s"]).as_deref(), Some("first"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn subjects_are_cleaned_and_cut() {
        assert_eq!(clean_subject("Fix\u{202e}gpj.exe\tsync\u{2028}· and 0 more\u{200b}"), "Fixgpj.exe sync · and 0 more");
        let long = "ə".repeat(MAX_SUBJECT_CHARS + 5);
        assert_eq!(clean_subject(&long), format!("{}…", "ə".repeat(MAX_SUBJECT_CHARS)));
        assert_eq!(clean_subject(&"a".repeat(MAX_SUBJECT_CHARS)), "a".repeat(MAX_SUBJECT_CHARS));
    }

    #[test]
    fn body_escapes_subjects_and_counts_the_rest() {
        let p = Pending { upstream: "x".into(), behind: 5, subjects: vec!["a <i>b</i> & c".into(), "d".into()] };
        assert_eq!(
            body(&p),
            "· a &lt;i&gt;b&lt;/i&gt; &amp; c\n· d\n· and 3 more\nClick to review the changes and install."
        );
    }
}
