//! A restart has to serve the world it saved, not a brand new one written over the top of it.
//!
//! `Config::save_target` has always fallen back from `save_file` to `world_file`, but the world
//! load in `main` only ever read `world_file`. Three flows set a save target and no world file,
//! and two of them are the first thing a new operator does:
//!
//! * the first-run wizard (`setup.rs:139`), which writes `save_file` and prints "the world will be
//!   generated at ... on first start",
//! * `main`'s bare-directory placement, whose stated purpose is that a world persists into
//!   `worlds/` "rather than serving something that vanishes on shutdown",
//! * a lone `--save` (and `tools/soak_ci.sh`, which is shaped exactly this way).
//!
//! Every one of them generated a *fresh* world on every boot and let the shutdown save write it
//! over the save already at that path, so restarting the server silently destroyed it.
//! `Config::resume_from_save_target`'s unit tests cover the decision; what is only provable out
//! here is the wiring, since the defect was precisely that `main` and `save_target` disagreed.
//!
//! The witness is the world's own **name**, and it has to be: the first draft of this test booted
//! twice and compared `world.id`, which passed with the fix deliberately neutralised. `world.id`
//! is `rand.next()` off `config.seed` (`worldgen/mod.rs:311`), and `seed` defaults to `0`, so a
//! regenerated world is byte-identical to the one it destroyed and no witness taken from
//! generation can tell the two apart.
//!
//! So the world is planted here instead of grown by the server, carrying a name the config does
//! not ask for. A load preserves the name in the file; generation takes `config.world_name`. The
//! two cannot agree by accident, whatever the seed does.

mod support;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn scratch_dir(label: &str) -> PathBuf {
    support::scratch_dir(&format!("terrustia-resume-{label}"))
}

/// The same poll-don't-sleep discipline as `shutdown_signal.rs` and `new_world_cli.rs`: a real
/// subprocess reaching a given line inside a fixed sleep is exactly the timing assumption this
/// project avoids elsewhere.
fn wait_for_line(rx: &mpsc::Receiver<String>, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
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

fn stream_stdout_lines(stdout: std::process::ChildStdout) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    rx
}

/// Boot the real binary against `config`, wait until it is actually serving, then stop it with a
/// real `SIGTERM` so the graceful shutdown save runs, and return the name of the world left on
/// disk. `autosave_secs = 300` in the config below means that save is the only write in this
/// window, which keeps "what reached disk" unambiguous.
fn boot_and_save(config: &Path, world: &Path) -> String {
    let child: Child = Command::new(env!("CARGO_BIN_EXE_terrustia"))
        .args(["-c", config.to_str().expect("a utf-8 config path")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("TERRUSTIA_LOG")
        // No test may depend on the network. Both of these are `tokio::spawn`ed at boot, so left
        // on, every server spawned here makes a real GitHub request and multicasts for a UPnP
        // gateway. Belt and braces with the config keys below, which say the same thing.
        .env("TERRUSTIA_UPDATE_CHECK_ENABLED", "false")
        .env("TERRUSTIA_UPNP_ENABLED", "false")
        .spawn()
        .expect("spawn terrustia");
    // Killed on drop, so a failing assertion below cannot leave it holding its port.
    let mut child = support::Reaper(child);
    let lines = stream_stdout_lines(child.stdout.take().expect("piped stdout"));
    assert!(
        wait_for_line(&lines, "accepting connections", Duration::from_secs(60)),
        "the server never reached \"accepting connections\""
    );

    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        assert!(
            wait_for_line(&lines, "game loop stopped", Duration::from_secs(30)),
            "the server did not shut down cleanly, so no shutdown save can be assumed"
        );
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    let status = child.wait().expect("reap the server");
    #[cfg(unix)]
    assert!(status.success(), "a graceful shutdown should exit 0");
    #[cfg(not(unix))]
    let _ = status;

    terrustia::world::wld::load(world)
        .unwrap_or_else(|e| panic!("reading back {}: {e}", world.display()))
        .name
}

/// The name only the file knows. Nothing in generation can produce it, because generation is
/// handed `config.world_name` below and that is deliberately something else.
const PLANTED: &str = "Planted Not Generated";

/// The wizard's config shape (`save_file`, no `world_file`) pointed at a world that already
/// exists. Before `Config::resume_from_save_target`, `main` reached its `None` arm, generated a
/// fresh world, and the shutdown save wrote it straight over this one.
#[test]
fn a_restart_serves_the_saved_world_instead_of_regenerating_over_it() {
    let dir = scratch_dir("restart");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let world_path = dir.join("Resume_Test.wld");
    let config = dir.join("terrustia.toml");

    // Plant the world the operator is supposed to keep. The smallest size `validate` accepts, so
    // this stays quick; what matters is only that a real `.wld` is sitting at the save target
    // before the server ever boots, exactly as it would be on anybody's second run.
    let (planted, _built) = terrustia::world::worldgen::build(400, 300, PLANTED, 0);
    terrustia::world::wld_save::save(&planted, &world_path).expect("plant a world to resume");

    // `world_name` here is what a generated world would be called, and it is not `PLANTED`. That
    // disagreement is the whole test: only a load can leave the planted name on disk.
    let listen = support::free_addr();
    std::fs::write(
        &config,
        format!(
            "world_name = \"Regenerated Over The Save\"\n\
             save_file = {world_path:?}\n\
             listen = \"{listen}\"\n\
             world_width = 400\n\
             world_height = 300\n\
             autosave_secs = 300\n\
             update_check_enabled = false\n\
             upnp_enabled = false\n"
        ),
    )
    .expect("write the config");

    assert_eq!(
        boot_and_save(&config, &world_path),
        PLANTED,
        "the server generated a fresh world and its shutdown save destroyed the one already at \
         the save target"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
