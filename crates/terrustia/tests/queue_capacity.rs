//! The outbound queue's real ceiling under a synchronized 255-player join burst — against the
//! real compiled binary, not an in-process mock.
//!
//! `examples/crowd` found this first (`docs/performance.md`'s "Large world, 16 and 255 players"),
//! and `plan.md`'s Benchmarks row disclosed it rather than fixing it: 255 players joining within
//! about a second dropped 88 of themselves (34.5%) with `outbound queue full; dropping a client
//! that cannot keep up`, even though tick cost stayed a clean 13.3% of budget throughout.
//!
//! That earlier pass's own root-cause guess — a presence-and-inventory relay burst landing on
//! every already-connected client's queue during the join itself — turned out to be incomplete.
//! Reproducing it here with the drop log's own `packet`/`name` fields turned on (which the
//! earlier pass had available but did not check) shows the drops are almost entirely
//! `PlayerControls` (id 13) and a handful of `SyncNPC` — ordinary steady-state movement
//! broadcast, not the join burst's presence/equipment frames — and the dropped slots spread
//! uniformly across the whole 0-254 range rather than clustering among the earliest joiners,
//! which is what the presence-relay theory would predict. See `net/connection.rs`'s own updated
//! doc comment on `OUTBOUND_PER_PLAYER` and `plan.md`'s corrections section for the full account.
//!
//! This has to run against a real subprocess rather than the in-process shape `gameplay.rs` and
//! `panel.rs` use. Measured directly: an in-process server sharing one `tokio` runtime with all
//! 255 simulated clients did not reproduce the drop at all, while the exact same scenario against
//! a real separate `terrustia` process did, repeatedly. That is real information, not a test
//! author's convenience — the bug is fundamentally about two independently-scheduled OS processes
//! (a real server, a real population of clients) each competing for the machine's own cores,
//! which is what a genuine deployment looks like and what `examples/crowd`'s own methodology
//! always used. `world_switch.rs` already set the precedent for reaching for a real subprocess
//! when a test's own shape needs one; this is that shape too.

use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use terrustia_client::Client;

mod support;

/// The real protocol maximum (`config::MAX_PLAYERS`) — the exact ceiling the benchmarking pass
/// measured against, not a scaled-down stand-in.
const PLAYERS: usize = 255;

/// How many movement packets each of the 255 connections sends back-to-back, without reading in
/// between, once everyone has joined.
///
/// A wall-clock-paced loop (`examples/crowd`'s own "one move every 16ms") was tried first and
/// measured to be unreliable specifically *inside `cargo test`*: the debug/test profile is not
/// `--release`, and with all 255 simulated clients and the server sharing this machine's cores,
/// how much real throughput 255 concurrent `tokio` tasks can push in wall-clock time varies with
/// however loaded the machine already is — which made the same test pass or fail depending on
/// what else this dev machine was doing, not on whether the fix was present. A fixed *count* of
/// unread sends removes that: how many `PlayerControls` frames land in one already-established
/// client's queue is then `(PLAYERS - 1) * SENDS_PER_PLAYER` by construction, independent of CPU
/// speed, and that is the same quantity `OUTBOUND_PER_PLAYER` is actually sized against — see
/// `net/connection.rs`'s own doc comment. 400 sends per player puts `254 * 400 = 101,600` unread
/// frames on every queue, comfortably past the old `73,472`-frame ceiling (measured: it did not
/// survive) and a small fraction of the fixed `1,052,672`-frame one (measured: zero drops).
const SENDS_PER_PLAYER: usize = 400;

/// How long to keep reading afterward, giving the server time to actually work through the burst
/// (broadcasting `(PLAYERS - 1) * SENDS_PER_PLAYER` frames to up to 254 peers each is real,
/// non-instant work) and giving any drop its chance to show up in the log this test reads.
const SETTLE_SECONDS: u64 = 20;

/// However many, if any, may legitimately fall out from ordinary environmental noise (a loaded CI
/// runner) without this being the bug. Measured repeatedly, the unfixed queue drops on the order
/// of a third to half the crowd — 121-122 of 255 in real `--release` two-process runs, 31 of 255
/// in this exact subprocess shape under `cargo test`'s own (lighter-loaded, opt-level 1) profile —
/// while the fixed queue measured zero drops in every repeated `cargo test` run of this file. A
/// handful of tolerance is a long way from either number, so it does not paper over a regression
/// while not chasing CI noise.
const DROP_TOLERANCE: usize = 8;

fn scratch_dir() -> PathBuf {
    support::scratch_dir("terrustia-queue-capacity")
}

/// Spawn a real `terrustia` server subprocess sized for a synchronized 255-player join.
///
/// Small and fast to generate — the bug is about player-count fan-out, not world size (the
/// presence/equipment burst that *does* scale with world size, chests especially, is not what
/// overflows here; see the module doc above). `max_connections`/`max_connections_per_address` are
/// not what this test is about, and are loosened well past 255 so real localhost connections are
/// never turned away before they ever reach the outbound queue this test exercises.
fn spawn_server(dir: &Path, port: u16) -> Child {
    std::fs::create_dir_all(dir).expect("scratch dir");
    let config_path = dir.join("terrustia.toml");
    std::fs::write(
        &config_path,
        format!(
            "listen = \"127.0.0.1:{port}\"\n\
             max_players = {PLAYERS}\n\
             max_connections = {cap}\n\
             max_connections_per_address = {cap}\n\
             world_width = 800\n\
             world_height = 600\n\
             autosave_secs = 0\n\
             motd = \"\"\n\
             panel_enabled = false\n\
             update_check_enabled = false\n\
             upnp_enabled = false\n",
            cap = PLAYERS + 16,
        ),
    )
    .expect("write config");

    Command::new(env!("CARGO_BIN_EXE_terrustia"))
        .args(["--config", config_path.to_str().expect("utf8 path")])
        .current_dir(dir)
        // `info` is enough to see both "accepting connections" and the drop warning this test
        // counts; `debug` would also print a tick-cost line a second, which is more than this
        // needs to read and adds noise the drop count does not want.
        .env("TERRUSTIA_LOG", "terrustia=info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn a real terrustia subprocess")
}

/// Every line a subprocess writes to stdout, delivered as it is written rather than only once the
/// process exits — this server never exits on its own, so waiting for it to finish before reading
/// its output would simply never return. The same helper `world_switch.rs` uses.
fn stream_stdout(child: &mut Child) -> mpsc::Receiver<String> {
    let stdout = child.stdout.take().expect("captured stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

async fn wait_for_port(addr: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn two_hundred_fifty_five_simultaneous_joins_do_not_drop_players() {
    let dir = scratch_dir();
    let port = 17_950u16;
    let addr = format!("127.0.0.1:{port}");

    let mut child = spawn_server(&dir, port);
    let lines = stream_stdout(&mut child);
    assert!(
        wait_for_port(&addr, Duration::from_secs(30)).await,
        "the real terrustia subprocess never started accepting connections"
    );

    let socket_addr: std::net::SocketAddr = addr.parse().expect("valid address");

    // The exact shape `examples/crowd` uses, and the same shape the earlier benchmarking pass
    // measured with: join everyone sequentially, with nobody's socket read in between. On
    // localhost this whole loop finishes in well under a second — the burst this test is about.
    let mut joined = Vec::with_capacity(PLAYERS);
    for i in 0..PLAYERS {
        match Client::join(socket_addr, &format!("crowd{i}")).await {
            Ok(c) => joined.push(c),
            Err(e) => {
                let _ = child.kill();
                panic!("player {i} of {PLAYERS} could not even join: {e}");
            }
        }
    }
    assert_eq!(
        joined.len(),
        PLAYERS,
        "every real connection should have joined cleanly"
    );

    // The deliberate burst: every connection sends `SENDS_PER_PLAYER` movement updates back to
    // back, reading nothing meanwhile — same principle as the join loop above (nobody's socket is
    // read while the crowd is busy), applied to steady movement instead of the join itself, and
    // with a fixed count rather than wall-clock pacing for the reason `SENDS_PER_PLAYER`'s own doc
    // comment explains.
    let spawn = joined[0].world().spawn;
    let width = joined[0].world().width;
    let mut tasks = Vec::with_capacity(PLAYERS);
    for (i, mut c) in joined.into_iter().enumerate() {
        let x = ((i as i32 * 137) % width.max(1)).clamp(200, width - 200) as f32 * 16.0;
        let y = f32::from(spawn.1) * 16.0 - 64.0;
        tasks.push(tokio::spawn(async move {
            let mut walk = 0.0f32;
            for _ in 0..SENDS_PER_PLAYER {
                walk = (walk + 8.0) % 320.0;
                if c.move_to(x + walk, y).await.is_err() {
                    return c; // the connection is gone; still hand it back to drain below
                }
            }
            c
        }));
    }

    // Hand every connection back and read from all of them for a while: this both lets the
    // server's write side actually flush the burst (a `Client` that is dropped closes its own
    // socket, which would otherwise race the server's own detection) and gives any drop's `warn!`
    // line time to land before this test reads the log.
    let mut clients = Vec::with_capacity(PLAYERS);
    for t in tasks {
        if let Ok(c) = t.await {
            clients.push(c);
        }
    }
    let mut drain_tasks = Vec::with_capacity(clients.len());
    for mut c in clients {
        drain_tasks.push(tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(SETTLE_SECONDS);
            while tokio::time::Instant::now() < deadline {
                if tokio::time::timeout(Duration::from_millis(50), c.next_event())
                    .await
                    .is_ok_and(|r| r.is_err())
                {
                    return; // closed under us
                }
            }
        }));
    }
    for t in drain_tasks {
        let _ = t.await;
    }

    // The drop count comes from the server's own real log line, the same signal an operator (and
    // the earlier benchmarking pass) would read — not inferred from which client tasks happened
    // to error, which conflates a queue-full drop with an ordinary end-of-test disconnect.
    let mut dropped = 0usize;
    while let Ok(line) = lines.try_recv() {
        if line.contains("outbound queue full") {
            dropped += 1;
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        dropped <= DROP_TOLERANCE,
        "{dropped} of {PLAYERS} players were dropped during a synchronized join burst — the \
         outbound queue is too shallow for steady-state broadcast fan-out at max_players={PLAYERS} \
         (see net/connection.rs's OUTBOUND_PER_PLAYER and this file's module doc). Allowed up to \
         {DROP_TOLERANCE}."
    );
}
