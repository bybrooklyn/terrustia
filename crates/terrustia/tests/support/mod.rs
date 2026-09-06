//! The two things every test that spawns a real server got wrong the same way.
//!
//! Four files here start a `terrustia` subprocess, assert against it, and kill it at the end:
//! `shutdown_signal.rs`, `world_switch.rs`, `resume_world_cli.rs` and `setup_wizard_cli.rs`. All
//! four had the same pair of defects, and together they are the "flaky test" `TODO.md` carried as
//! undiagnosed for weeks at roughly one run in five:
//!
//! * **A failing assertion skipped the kill.** The kill sits after the assertions, and
//!   `std::process::Child` does not kill on drop, so a panic left a real server running.
//! * **The port was a constant.** The leaked server then held it until somebody noticed, so one
//!   genuine failure made every later run on that machine fail too - and fail at the *first*
//!   assertion, which is why the reported symptom never matched the cause.
//!
//! Measured on `shutdown_signal.rs` before the fix: run 9 of a 20-run loop timed out for real and
//! left its server on 17796; runs 10 through 20 then failed in 0.38 seconds each. Forcing the same
//! failure by hand leaves two servers running on the old code and none under [`Reaper`].
//!
//! **Only `shutdown_signal.rs` was observed leaking.** Forcing a mid-test failure in
//! `world_switch.rs` left nothing behind, because that server keeps logging and dies of `SIGPIPE`
//! once the test process stops reading its stdout - which is luck, not design, and depends on
//! whether a given server happens to print again before it is orphaned. The other three get the
//! same guard because they have the same shape, not because each was caught.
//!
//! Included with `mod support;` rather than made a crate: `tests/` files are separate binaries, so
//! this is compiled into each of them, and anything unused in one of them would warn. Every item
//! here is therefore `pub` and marked `#[allow(dead_code)]`.

#![allow(dead_code)]

use std::process::Child;

/// A port the OS says is free right now.
///
/// There is a race between closing this listener and the server binding the same number, but it is
/// a race against the rest of the machine rather than against this suite's own leftovers, which is
/// the failure that actually happened.
pub fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    listener.local_addr().expect("the bound address").port()
}

/// A scratch directory nothing else in this process will be handed.
///
/// **The pid-plus-nanoseconds name every one of these tests grew independently is not unique**,
/// and that was a real flake rather than a theoretical one. `SystemTime::now().as_nanos()` is not
/// nanosecond-resolution: two threads reading it at the same moment get the *identical* value
/// **362 times in 2,000** on this machine, measured. Two tests in one binary start together, so
/// roughly one run in five handed both of them the same directory - and then they shared a
/// `terrustia.toml` and a world file, whichever wrote last decided both servers' configuration,
/// and the loser died on a port the winner had already taken.
///
/// That is what `shutdown_signal.rs` was failing on, 1 run in 4 under a concurrent `--test
/// gameplay`, reported as `127.0.0.1:51588 is already in use`. The port was a symptom: both
/// servers had read the same `listen` line out of the same file. It is also, in hindsight, what
/// `new_world_cli.rs`'s `ONE_AT_A_TIME` mutex was really fixing - serialising those four tests
/// stopped them calling this concurrently, so they stopped colliding.
///
/// The counter is what makes it safe; the clock and the pid are kept so a leftover directory can
/// still be traced to a run.
pub fn scratch_dir(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("the clock")
        .as_nanos();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}-{n}", std::process::id()))
}

/// A local address on an OS-assigned free port.
pub fn free_addr() -> String {
    format!("127.0.0.1:{}", free_port())
}

/// A child process that is killed and reaped when it goes out of scope.
///
/// `std::process::Child` deliberately does not kill on drop, which is right for a library and
/// wrong for a test: it makes a panicking assertion leak a server. Wrap the child once at the
/// spawn site and every exit path, including an unwind, goes through this drop.
pub struct Reaper(pub Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl std::ops::Deref for Reaper {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for Reaper {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}
