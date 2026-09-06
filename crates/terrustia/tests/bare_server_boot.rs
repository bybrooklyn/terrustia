//! The promise the download page makes: extract the binary into an empty directory on a machine
//! with nothing else on it, run it with no arguments and no terminal, and get a served world.
//!
//! That is not a hypothetical shape. It is what `setup::should_auto_trigger` recognises as "just
//! downloaded the raw binary and ran it where it landed": no arguments, the working directory is
//! the executable's own, no `terrustia.toml`, no `.wld`. It is also, character for character, what
//! a headless install looks like. Extract a release archive to `/opt/terrustia`, start it under
//! systemd or a container or `nohup`, and every one of those conditions holds.
//!
//! So the wizard used to launch on a server that had nobody to answer it, and went straight to
//! `io::stdin().read_line`. With stdin closed that returns `Ok(0)` for every prompt, so the run
//! took silent defaults and wrote a `terrustia.toml` nobody asked for. Backgrounded with a tty
//! still attached, reading stdin raises `SIGTTIN` and stops the process, printing nothing to
//! explain itself. Either way the operator's first experience of the server is that it does not
//! come up.
//!
//! This runs the real binary from a copy of itself in a scratch directory, which is the only way
//! to make the working directory genuinely equal the executable's own. Configuration is passed
//! through the environment rather than flags on purpose: a single argument would make
//! `should_auto_trigger` return `false` on its first line and the test would pass without ever
//! reaching the guard it exists to check.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod support;

fn scratch_dir(label: &str) -> PathBuf {
    support::scratch_dir(&format!("terrustia-bare-{label}"))
}

fn wait_for_line(rx: &mpsc::Receiver<String>, needle: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match rx.recv_timeout(remaining) {
            Ok(line) if line.contains(needle) => return Some(line),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

/// Extract-and-run, exactly as a first-time operator does it, with no terminal on either end.
#[test]
fn a_bare_binary_in_an_empty_directory_serves_a_world_with_no_arguments() {
    let dir = scratch_dir("boot");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    // The copy is what makes this real: `should_auto_trigger` compares the working directory
    // against the *executable's* directory, so running the one in `target/debug` would never
    // match and the guard under test would never be reached.
    let exe = dir.join("terrustia");
    std::fs::copy(env!("CARGO_BIN_EXE_terrustia"), &exe).expect("copy the binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))
            .expect("make the copy executable");
    }

    let mut child = Command::new(&exe)
        .current_dir(&dir)
        // No arguments at all. Anything here disables the auto-trigger before the guard is
        // reached, so the settings that keep this test hermetic go through the environment.
        .env("HOME", &dir)
        .env("XDG_DATA_HOME", dir.join("xdg"))
        .env("USERPROFILE", &dir)
        .env("TERRUSTIA_LISTEN", "127.0.0.1:17853")
        .env("TERRUSTIA_WORLD_WIDTH", "400")
        .env("TERRUSTIA_WORLD_HEIGHT", "300")
        .env("TERRUSTIA_UPDATE_CHECK_ENABLED", "false")
        .env("TERRUSTIA_UPNP_ENABLED", "false")
        .env_remove("TERRUSTIA_LOG")
        // Closed, not inherited: this is the headless case, and an inherited terminal would let
        // the wizard's prompts succeed and hide the very bug this pins.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the extracted binary");

    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });

    let ready = wait_for_line(&rx, "accepting connections", Duration::from_secs(120));
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        ready.is_some(),
        "an extracted binary run with no arguments and no terminal must serve a world; instead it \
         never reached \"accepting connections\""
    );

    // The decisive check, and the one the symptom actually shows up in. On a closed stdin
    // `prompt_yes_no` and its siblings do not hang: `read_line` returns `Ok(0)`, which every
    // prompt treats as "take the default". So the wizard runs to completion, answering its own
    // questions, and leaves a whole configured install behind in the dedicated directory it
    // picked without being asked. `HOME` is redirected at this test's scratch directory, so that
    // is where it would land.
    let wizard_dir = dir.join("terrustia-server");
    assert!(
        !wizard_dir.exists(),
        "the first-run wizard ran on a host with no terminal and answered its own prompts with \
         defaults, leaving an install at {}",
        wizard_dir.display()
    );

    let _ = std::fs::remove_dir_all(&dir);
}
