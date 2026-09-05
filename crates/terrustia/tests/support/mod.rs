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
