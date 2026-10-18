mod config;
mod images;
mod mailbox;
mod message;
mod otp;
mod sanitize;
mod secret;
mod send;
mod server;
mod utf7;
mod watch;

fn main() {
    // The app password lives in this process's memory; keep it out of core
    // dumps and away from ptrace by other processes of the same user.
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }
    match std::env::args().nth(1).as_deref() {
        Some("serve") => server::run(),
        Some("watch") => std::process::exit(watch::run()),
        Some("--version") => println!("omamail {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!("usage: omamail serve   # backend for the Omamail window (JSON lines on stdio)");
            eprintln!("       omamail watch   # new-mail notifications (run as a user service)");
            std::process::exit(2);
        }
    }
}
