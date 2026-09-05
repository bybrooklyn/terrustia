//! A real `SIGTERM` against a real running server actually stops it — not just logs "shutting
//! down" and keeps ticking forever.
//!
//! Found by hand while verifying `packaging/terrustia.service`'s `ExecStart` path actually works
//! end to end (not just that the unit file parses): `main.rs` spawns `panel::supervise` with a
//! bare `tokio::spawn`, never storing or aborting its `JoinHandle` — but that task holds its own
//! clone of `events_tx` for as long as it runs, so `main`'s own `drop(events_tx)` during shutdown
//! was never actually dropping the *last* sender. `GameServer::run`'s only clean-exit path is
//! `events.recv() => None => break` (`game/server.rs`), which needs every sender gone — so a real
//! SIGTERM logged "shutting down" and then the game loop kept ticking and autosaving forever,
//! never actually stopping, until `packaging/terrustia.service`'s own `TimeoutStopSec=90` would
//! eventually force a hard kill — defeating the graceful shutdown save that whole unit's hardening
//! is built around. Fixed by aborting `panel::supervise`'s handle alongside `accept`/`console` in
//! `main`'s existing shutdown sequence — see that call site's own comment for the full story.
//!
//! `sigterm_stops_the_server_and_saves_within_a_bounded_window` below sends a real `SIGTERM` to a
//! real running subprocess and asserts it actually exits within a bounded window, with a real
//! shutdown save on disk — the unfixed code hung indefinitely at exactly this point (observed
//! directly: 27+ seconds and counting, autosaving on its ordinary 1-second interval throughout,
//! before being killed by hand).
//!
//! That fix was still incomplete for one real, undertested configuration: with the panel actually
//! *running* (not just wired up but never started), aborting `panel::supervise`'s own outer task
//! left its real inner axum-serving task — and the live `events_tx` clone captured inside it —
//! running forever, detached rather than stopped, for exactly the reason this module doc already
//! gives above (a dropped `JoinHandle` detaches, it does not stop the task). This test file's own
//! panel-off test could never have caught that: with the panel off, `supervise`'s local handle is
//! `None` for the test's whole life, so there is nothing for that half of the bug to leak.
//! `panel_enabled_sigterm_still_stops_the_server_and_saves_within_a_bounded_window` below is the
//! same pin, with the panel turned on — see its own doc comment, and `panel::PanelHandle`'s doc
//! comment in `crates/terrustia/src/panel/mod.rs` for the actual fix (an abort-on-drop guard around
//! `supervise`'s local handle, which makes the real inner task structurally unable to outlive
//! `supervise` itself regardless of how or why its own future stops).

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

fn scratch_home() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "terrustia-shutdown-signal-{}-{nanos}",
        std::process::id()
    ))
}

fn stream_stdout_lines(stdout: std::process::ChildStdout) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn wait_for_line(rx: &mpsc::Receiver<String>, needle: &str, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match rx.recv_timeout(remaining) {
            Ok(line) if line.contains(needle) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

/// `panel_listen`, when given, turns the web admin panel on for this run — the specific
/// configuration that triggers the second, separate SIGTERM bug `panel_enabled_sigterm_still_stops_
/// the_server_and_saves_within_a_bounded_window` below pins: the ordinary (panel-off) case above
/// never spawns the panel's own inner task at all, so it could not have caught that bug either way.
fn spawn_server(home: &std::path::Path, listen: &str, panel_listen: Option<&str>) -> Child {
    let save_file = home.join("ShutdownSignalTest.wld");
    let panel_config = match panel_listen {
        Some(addr) => format!("panel_enabled = true\npanel_listen = \"{addr}\"\n"),
        None => String::new(),
    };
    std::fs::write(
        home.join("terrustia.toml"),
        format!(
            "autosave_secs = 300\nworld_width = 400\nworld_height = 300\nlisten = \"{listen}\"\n\
             save_file = {save_file:?}\n{panel_config}"
        ),
    )
    .expect("write config");
    let mut command = Command::new(env!("CARGO_BIN_EXE_terrustia"));
    // Windows has no `kill -TERM`; a graceful stop has to arrive as a console control event, and
    // `GenerateConsoleCtrlEvent` can only address a process group, so the child needs one of its
    // own. `CREATE_NEW_PROCESS_GROUP` is 0x0000_0200. See the send site below for why `Child::kill`
    // cannot stand in: it is `TerminateProcess`, which is exactly the ungraceful stop this whole
    // file exists to prove the server does *not* need.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0000_0200);
    }
    command
        .current_dir(home)
        .env_remove("TERRUSTIA_LOG")
        // No test may depend on the network. Both of these are `tokio::spawn`ed at boot, so left
        // on, every server spawned here makes a real GitHub request and multicasts for a UPnP
        // gateway. This is hygiene, not a fix for anything: the CLI-test flake recorded in TODO.md
        // was measured with both already off and still failed 5 runs in 8, so the cause is elsewhere.
        .env("TERRUSTIA_UPDATE_CHECK_ENABLED", "false")
        .env("TERRUSTIA_UPNP_ENABLED", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terrustia")
}

/// A 300-second `autosave_secs` (the real default) means the world file can only ever land on
/// disk two ways: the periodic autosave, decades from now as far as this test is concerned, or a
/// clean shutdown. That makes this the tightest possible pin against the exact bug: on the
/// unfixed code, nothing would ever have written this file at all within this test's window.
#[test]
fn sigterm_stops_the_server_and_saves_within_a_bounded_window() {
    let home = scratch_home();
    std::fs::create_dir_all(&home).expect("scratch home");

    let mut child = spawn_server(&home, "127.0.0.1:17796", None);
    let stdout_lines = stream_stdout_lines(child.stdout.take().expect("piped stdout"));

    assert!(
        wait_for_line(
            &stdout_lines,
            "accepting connections",
            Duration::from_secs(30)
        ),
        "the server should have reached its main loop by now"
    );

    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
    }
    // Windows' own equivalent of the `SIGTERM` above: a real Ctrl+Break to the child's own process
    // group, which `stop_signal` listens for beside the close and shutdown events. `Child::kill`
    // is `TerminateProcess`, which runs no handler and skips the save, so using it here would make
    // this file assert that an ungraceful kill saves the world, which is the opposite of its point.
    #[cfg(windows)]
    {
        // SAFETY: `GenerateConsoleCtrlEvent` takes an event id and a process group id and touches
        // nothing else. The group id is the child's own pid, which is what
        // `CREATE_NEW_PROCESS_GROUP` at spawn made it. `CTRL_BREAK_EVENT` is 1.
        #[allow(unsafe_code)]
        unsafe {
            unsafe extern "system" {
                fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
            }
            GenerateConsoleCtrlEvent(1, child.id());
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = child.kill();
    }

    // The unfixed code hung here indefinitely — this timeout is generous next to the sub-second
    // shutdown actually measured by hand (~150ms from "shutting down" to "game loop stopped"),
    // not a guess at how long a fixed version might reasonably take.
    #[cfg(unix)]
    {
        assert!(
            wait_for_line(&stdout_lines, "game loop stopped", Duration::from_secs(15)),
            "the server must actually stop after SIGTERM, not just log \"shutting down\" and \
             keep running"
        );
    }

    let status = child
        .wait_timeout_or_kill(Duration::from_secs(10))
        .expect("the process must exit on its own after a graceful SIGTERM shutdown");
    assert!(
        status.success(),
        "a graceful SIGTERM shutdown should exit 0, got {status:?}"
    );

    // `autosave_secs = 300` above means the only way this file can exist at all, this soon, is
    // the shutdown save that just ran — on the unfixed code, nothing would ever have written it
    // within this test's whole window.
    let save_file = home.join("ShutdownSignalTest.wld");
    assert!(
        save_file.is_file(),
        "the shutdown save should have written {} by now",
        save_file.display()
    );
    assert!(
        std::fs::metadata(&save_file).is_ok_and(|m| m.len() > 0),
        "the saved world file should not be empty"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// The same bug the test above pins, but for the specific configuration that the fix above
/// (storing and aborting `panel_supervisor`'s own outer `JoinHandle`) did *not* actually close:
/// with the web panel turned on, `panel::supervise`'s own inner axum-serving task — which holds the
/// real `TcpListener` and a live clone of `events_tx` inside its `PanelState` — used to survive
/// `panel_supervisor.abort()` entirely, because cancelling `supervise`'s future just dropped its
/// local `handle` variable, which detaches a `JoinHandle` rather than stopping the task it names.
/// `GameServer::run`'s clean-exit path needs *every* clone of `events_tx` dropped, so that leaked
/// clone meant a real SIGTERM logged "shutting down" and then the server never actually stopped,
/// for as long as the panel had ever been running. Fixed by `panel::PanelHandle`, a small
/// abort-on-drop guard around `supervise`'s local handle — see its own doc comment in
/// `crates/terrustia/src/panel/mod.rs`.
///
/// This test is the reason the fix has to live where it does: with the panel off (the test above),
/// `supervise`'s local `handle` is `None` for the test's whole life, so there is nothing for the
/// old bug to leak and that test could never have caught this. Only a real panel-enabled run
/// exercises the code path this test pins.
#[test]
fn panel_enabled_sigterm_still_stops_the_server_and_saves_within_a_bounded_window() {
    let home = scratch_home();
    std::fs::create_dir_all(&home).expect("scratch home");

    let mut child = spawn_server(&home, "127.0.0.1:17798", Some("127.0.0.1:17799"));
    let stdout_lines = stream_stdout_lines(child.stdout.take().expect("piped stdout"));

    // Waited for in the order the server actually prints them, not the order that reads
    // naturally: `main` binds and starts the panel (`panel::run`, opt-in, `?`-propagated on
    // failure) *before* the accept loop's own "accepting connections" line — `wait_for_line`
    // discards whatever it scans past while searching, so asking for "accepting connections"
    // first would silently eat the earlier "web panel listening" line before the second wait ever
    // got a chance to see it. This file's own module doc points at `plan.md`'s "Tile action log"
    // Done row for another test in this codebase that hit exactly this ordering trap.
    assert!(
        wait_for_line(
            &stdout_lines,
            "web panel listening",
            Duration::from_secs(30)
        ),
        "the panel should have finished binding by now"
    );
    assert!(
        wait_for_line(
            &stdout_lines,
            "accepting connections",
            Duration::from_secs(30)
        ),
        "the server should have reached its main loop by now"
    );

    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
    }
    // Windows' own equivalent of the `SIGTERM` above: a real Ctrl+Break to the child's own process
    // group, which `stop_signal` listens for beside the close and shutdown events. `Child::kill`
    // is `TerminateProcess`, which runs no handler and skips the save, so using it here would make
    // this file assert that an ungraceful kill saves the world, which is the opposite of its point.
    #[cfg(windows)]
    {
        // SAFETY: `GenerateConsoleCtrlEvent` takes an event id and a process group id and touches
        // nothing else. The group id is the child's own pid, which is what
        // `CREATE_NEW_PROCESS_GROUP` at spawn made it. `CTRL_BREAK_EVENT` is 1.
        #[allow(unsafe_code)]
        unsafe {
            unsafe extern "system" {
                fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
            }
            GenerateConsoleCtrlEvent(1, child.id());
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = child.kill();
    }

    // On the unfixed code this hangs indefinitely — the panel's leaked inner task keeps the game
    // loop's own `events.recv()` from ever seeing its last sender go away, so "game loop stopped"
    // never prints at all. This timeout is generous next to the sub-second shutdown the fixed code
    // actually measures, not a guess at how long a correct version might reasonably take.
    #[cfg(unix)]
    {
        assert!(
            wait_for_line(&stdout_lines, "game loop stopped", Duration::from_secs(15)),
            "the server must actually stop after SIGTERM even with the web panel running, not \
             just log \"shutting down\" and keep the panel's leaked task running forever"
        );
    }

    let status = child.wait_timeout_or_kill(Duration::from_secs(10)).expect(
        "the process must exit on its own after a graceful SIGTERM shutdown, even with the \
             panel enabled",
    );
    assert!(
        status.success(),
        "a graceful SIGTERM shutdown should exit 0, got {status:?}"
    );

    // Same pin as the panel-off test above: a 300-second `autosave_secs` means the only way this
    // file can exist at all, this soon, is the shutdown save that just ran.
    let save_file = home.join("ShutdownSignalTest.wld");
    assert!(
        save_file.is_file(),
        "the shutdown save should have written {} by now",
        save_file.display()
    );
    assert!(
        std::fs::metadata(&save_file).is_ok_and(|m| m.len() > 0),
        "the saved world file should not be empty"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// `std::process::Child` has no built-in bounded wait — this is the small, hand-rolled
/// equivalent, matching this project's own stated preference for a narrow helper over a crate for
/// something this small (the `wait-timeout` crate exists solely for this one function).
trait WaitTimeoutOrKill {
    fn wait_timeout_or_kill(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<std::process::ExitStatus>;
}

impl WaitTimeoutOrKill for Child {
    fn wait_timeout_or_kill(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<std::process::ExitStatus> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            if std::time::Instant::now() >= deadline {
                let _ = self.kill();
                let _ = self.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "process did not exit within the timeout",
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
