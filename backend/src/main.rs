mod config;
#[cfg(feature = "demo")]
mod demo;
mod images;
mod liveness;
mod mailbox;
mod message;
mod otp;
mod sanitize;
mod secret;
mod send;
mod server;
mod update;
mod utf7;
mod watch;

fn main() {
    // The app password lives in this process's memory; keep it out of core
    // dumps and away from ptrace by other processes of the same user.
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }
    match std::env::args().nth(1).as_deref() {
        // The demo build (screenshots) must never act on the real account.
        Some("watch" | "update-check" | "copy") if cfg!(feature = "demo") => {
            eprintln!("omamail: the demo build only serves the window");
            std::process::exit(2);
        }
        Some("serve") => server::run(),
        Some("watch") => std::process::exit(watch::run()),
        Some("update-check") => std::process::exit(update::check()),
        Some("copy") => std::process::exit(watch::copy(std::env::args().nth(2).as_deref())),
        Some("--version") => println!("omamail {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!("usage: omamail serve   # backend for the Omamail window (JSON lines on stdio)");
            eprintln!("       omamail watch   # new-mail notifications (run as a user service)");
            eprintln!("       omamail update-check   # notify when the plugin has updates (run by a user timer)");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    /// The plugin listing shows manifest.json's version and `omamail
    /// --version` the crate's: every change bumps both (see CONTRIBUTING.md).
    /// A stale Cargo.lock is caught by `cargo test --locked`, not here: a
    /// plain `cargo test` rewrites it before this test is even compiled.
    #[test]
    fn manifest_and_crate_versions_match() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!("../../manifest.json")).unwrap();
        assert_eq!(manifest["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
    }
}
