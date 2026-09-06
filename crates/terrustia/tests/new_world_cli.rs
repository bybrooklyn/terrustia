//! `--new <name>` end to end: a real invocation of the compiled binary, not a unit test of the
//! path arithmetic — `worlds::new_world_path` already has those. What isn't proven anywhere else
//! is that `--new` actually reaches the filesystem: that the world it generates is written into
//! wherever this platform's own Terraria keeps its worlds, under the name given, and that asking
//! twice for the same name refuses the second time rather than clobbering the first.
//!
//! Runs the real `terrustia` binary as a subprocess with `HOME`/`XDG_DATA_HOME`/`USERPROFILE` all
//! redirected at a scratch directory — never the machine's real Terraria world directory — so
//! this both proves the CLI wiring and stays entirely inside a directory this test owns and
//! deletes when it is done.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

fn scratch_home() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "terrustia-new-world-cli-{}-{nanos}",
        std::process::id()
    ))
}

/// Every file under `dir` named exactly `name`, found by walking recursively — sidesteps needing
/// to know, in the test itself, which of `worlds::directory`'s three platform branches applies.
fn find_named(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(find_named(&path, name));
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            found.push(path);
        }
    }
    found
}

/// Wait until the server is up, then for its world file to land.
///
/// This used to be a bare 120-second filesystem poll, and that was the last real flake in the file:
/// on a saturated machine generating a world takes as long as it takes, the poll returned whatever
/// it had — usually nothing — and the assertion *after* it then failed on a missing file. Measured
/// by running this binary against a concurrent `--test gameplay`: the run took 121.5 seconds, which
/// is exactly the deadline, and it was `new_ignores_a_stale_world_file_left_in_the_config` that
/// went red, on a world that simply had not been written yet.
///
/// A bigger number only moves the line. The load-sensitive part is generation, and the server says
/// when that is over: `accepting connections` is logged once the world exists and the listener is
/// bound. So this waits on that line rather than on a clock, and only then polls for the file,
/// which by that point is a short and deterministic wait.
///
/// **The first attempt at this waited for `world saved` and was wrong**, in a way worth recording
/// because it looked like a server bug: no such line ever appears. A fast autosave logs at `debug`
/// on purpose (`game/server/mod.rs:2062-2069` — "a routine autosave that worked is not news"), and
/// these tests run at the default level, so the only `info` save is the one on shutdown. The world
/// is on disk from generation, long before any autosave, which is why the original poll saw it at
/// all.
///
/// Draining stdout is a second fix in the same move: nothing read it before, so a chatty server
/// could fill the pipe buffer and block on its own logging.
fn wait_for_generated_world(
    child: &mut std::process::Child,
    dir: &Path,
    name: &str,
    timeout: Duration,
) -> Vec<PathBuf> {
    use std::io::{BufRead, BufReader};

    let stdout = child.stdout.take().expect("run_new pipes stdout");
    let (lines, from_server) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = std::time::Instant::now() + timeout;
    let mut transcript = Vec::new();
    let mut up = false;
    while std::time::Instant::now() < deadline {
        if !up {
            match from_server.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => {
                    up = line.contains("accepting connections");
                    transcript.push(line);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            continue;
        }
        let found = find_named(dir, name);
        if found
            .iter()
            .any(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 0))
        {
            return found;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!(
        "{name} never landed within {}s (server {}up). It said:\n{}",
        timeout.as_secs(),
        if up { "" } else { "never came " },
        transcript.join("\n")
    );
}

/// Run `terrustia --new <name>` against a scratch home, with autosave fast enough that a short
/// fixed wait is enough to see the file land, then kill it — a clean shutdown's own save path is
/// already covered by the game-level save/reload tests; this only needs the file to exist once.
fn run_new(home: &Path, name: &str, listen: &str) -> std::process::Child {
    // The smallest size `Config::validate` accepts, generated much faster than the default —
    // this test needs the file to land, not a world worth playing in.
    std::fs::write(
        home.join("terrustia.toml"),
        "autosave_secs = 1\nworld_width = 400\nworld_height = 300\n",
    )
    .expect("write config");
    Command::new(env!("CARGO_BIN_EXE_terrustia"))
        .args(["--new", name, "--listen", listen])
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join("xdg"))
        .env("USERPROFILE", home)
        .env_remove("TERRUSTIA_LOG")
        // No test may depend on the network. Both of these are `tokio::spawn`ed at boot, so left
        // on, every server spawned here makes a real GitHub request and multicasts for a UPnP
        // gateway. This is hygiene, not a fix for anything: the CLI-test flake recorded in TODO.md
        // was measured with both already off and still failed 5 runs in 8, so the cause is elsewhere.
        .env("TERRUSTIA_UPDATE_CHECK_ENABLED", "false")
        .env("TERRUSTIA_UPNP_ENABLED", "false")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terrustia")
}

#[test]
fn new_generates_a_world_into_the_platforms_terraria_world_directory() {
    let home = scratch_home();
    std::fs::create_dir_all(&home).expect("scratch home");

    let mut child = run_new(&home, "Fork Test World", "127.0.0.1:17779");
    let found = wait_for_generated_world(
        &mut child,
        &home,
        "Fork_Test_World.wld",
        Duration::from_secs(120),
    );
    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(
        found.len(),
        1,
        "expected exactly one Fork_Test_World.wld under {}, found {:?}",
        home.display(),
        found
    );
    assert!(
        std::fs::metadata(&found[0]).is_ok_and(|m| m.len() > 0),
        "the world file should not be empty"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// `--new` must generate a fresh world even when `terrustia.toml` already sets `world_file` to a
/// different, existing world — that config-file value is layered in (`Config::load`, then
/// `apply_env`) before any CLI flag is read, and `--new` used to only redirect where the result is
/// *saved* without ever clearing `world_file`, so the server would silently load and re-save the
/// stale world under the new name instead of generating one. Proven here by size, not just file
/// existence: the stale world and the freshly-requested one are given different dimensions, and
/// the resulting file is loaded back and checked against the *new* config's width, not the stale
/// file's.
#[test]
fn new_ignores_a_stale_world_file_left_in_the_config() {
    let home = scratch_home();
    std::fs::create_dir_all(&home).expect("scratch home");

    // First, generate the "stale" world `terrustia.toml` will point at. A generous timeout: this
    // is the only test in this file that generates two worlds in sequence, each waiting on top of
    // whatever the other tests' own concurrently-running server subprocesses are costing it.
    let mut stale = run_new(&home, "Stale World", "127.0.0.1:17782");
    let stale_found = wait_for_generated_world(
        &mut stale,
        &home,
        "Stale_World.wld",
        Duration::from_secs(120),
    );
    let _ = stale.kill();
    let _ = stale.wait();
    assert_eq!(
        stale_found.len(),
        1,
        "the stale world should have been written first"
    );
    let stale_path = stale_found[0]
        .to_str()
        .expect("utf8 path")
        .replace('\\', "\\\\");

    // Now point the config at it directly, with a different width, and ask for a new world.
    std::fs::write(
        home.join("terrustia.toml"),
        format!(
            "autosave_secs = 1\nworld_width = 600\nworld_height = 300\nworld_file = \"{stale_path}\"\n"
        ),
    )
    .expect("write config");
    let mut fresh = Command::new(env!("CARGO_BIN_EXE_terrustia"))
        .args(["--new", "Fresh World", "--listen", "127.0.0.1:17783"])
        .current_dir(&home)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join("xdg"))
        .env("USERPROFILE", &home)
        .env_remove("TERRUSTIA_LOG")
        .env("TERRUSTIA_UPDATE_CHECK_ENABLED", "false")
        .env("TERRUSTIA_UPNP_ENABLED", "false")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terrustia");
    let fresh_found = wait_for_generated_world(
        &mut fresh,
        &home,
        "Fresh_World.wld",
        Duration::from_secs(120),
    );
    let _ = fresh.kill();
    let _ = fresh.wait();
    assert_eq!(
        fresh_found.len(),
        1,
        "the fresh world should have been written"
    );

    let loaded = terrustia::world::wld::load(&fresh_found[0]).expect("load the generated world");
    assert_eq!(
        loaded.width(),
        600,
        "--new should generate at the new config's width, not silently load+re-save the stale \
         world_file (which was generated at the old, smaller default width)"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// `Config::validate` skips its own width/height/section-alignment checks entirely whenever
/// `world_file.is_some()` — correct for `--world`, where a loaded world brings its own dimensions,
/// but `Config::load` runs `validate` once *before* `--new` can clear `world_file`, so an
/// out-of-range `world_width`/`world_height` sitting in a config file that also sets `world_file`
/// used to reach real generation completely unvalidated. Proven by giving `--new` a config whose
/// `world_file` would have suppressed the check and whose dimensions are genuinely invalid (below
/// the documented 400x300 floor): this must fail fast with a clear message, not attempt
/// generation at an unvalidated size.
#[test]
fn new_still_validates_dimensions_even_with_a_world_file_set() {
    let home = scratch_home();
    std::fs::create_dir_all(&home).expect("scratch home");
    std::fs::write(
        home.join("terrustia.toml"),
        "world_width = 50\nworld_height = 20\nworld_file = \"anything.wld\"\n",
    )
    .expect("write config");

    let mut child = Command::new(env!("CARGO_BIN_EXE_terrustia"))
        .args(["--new", "Too Small World", "--listen", "127.0.0.1:17784"])
        .current_dir(&home)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join("xdg"))
        .env("USERPROFILE", &home)
        .env_remove("TERRUSTIA_LOG")
        .env("TERRUSTIA_UPDATE_CHECK_ENABLED", "false")
        .env("TERRUSTIA_UPNP_ENABLED", "false")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terrustia");
    // A bounded poll, not a blocking `child.wait()` — every other subprocess test in this file
    // already uses one (`wait_for_file`'s own deadline loop), and this one didn't, which is
    // exactly what let a single contention-slow run on a shared machine hang the entire suite
    // indefinitely instead of failing loudly with a clear message.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll terrustia") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "terrustia did not exit within 30s — an out-of-range world_width/world_height \
                 should be refused immediately at startup, not left running"
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(
        !status.success(),
        "an out-of-range world_width/world_height must be refused, not silently generated"
    );
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("captured stdout")
        .read_to_string(&mut stdout)
        .expect("read stdout");
    assert!(
        stdout.contains("must be at least 400x300"),
        "expected a clear size-refusal message on stdout, got: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn new_refuses_a_name_that_already_exists() {
    let home = scratch_home();
    std::fs::create_dir_all(&home).expect("scratch home");

    let mut first = run_new(&home, "Collision World", "127.0.0.1:17780");
    let found = wait_for_generated_world(
        &mut first,
        &home,
        "Collision_World.wld",
        Duration::from_secs(120),
    );
    let _ = first.kill();
    let _ = first.wait();
    assert_eq!(
        found.len(),
        1,
        "the first run should have written the world before the second one is tried"
    );

    // A second `--new` under the same name must refuse rather than silently overwrite the first
    // server's world out from under it — the whole reason `--new` checks first.
    let mut second = Command::new(env!("CARGO_BIN_EXE_terrustia"))
        .args(["--new", "Collision World", "--listen", "127.0.0.1:17781"])
        .current_dir(&home)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join("xdg"))
        .env("USERPROFILE", &home)
        .env_remove("TERRUSTIA_LOG")
        .env("TERRUSTIA_UPDATE_CHECK_ENABLED", "false")
        .env("TERRUSTIA_UPNP_ENABLED", "false")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terrustia");
    let status = second.wait().expect("wait for the second run");
    assert!(
        !status.success(),
        "a second --new with the same name should fail rather than clobber the first world"
    );
    // `error!()` goes through the same `TermLayer` as every other log line, onto stdout — not
    // stderr, which this process never writes to at all.
    let mut stdout = String::new();
    second
        .stdout
        .take()
        .expect("captured stdout")
        .read_to_string(&mut stdout)
        .expect("read stdout");
    assert!(
        stdout.contains("already exists"),
        "expected a clear refusal on stdout, got: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&home);
}
