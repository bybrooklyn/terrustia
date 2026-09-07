//! Sit in a world for a while, wandering, so the server's tick cost can be watched under the
//! conditions a real player creates: streamed sections, natural spawns, contact damage.
//!
//! ```text
//! cargo run --release --example soak -- 127.0.0.1:7777 180
//! cargo run --release --example soak -- 127.0.0.1:7777 1800 0 soak 255
//! ```
//!
//! The fifth argument is how many players to be. They are ordinary connections held as tasks on one
//! runtime rather than one process each, which is the difference between a 255-player run costing
//! about 3.5 GB of client and costing a fraction of that. That number is not a guess: a single soak
//! process was measured at 13.7 MiB resident (24 of them, 328 MiB), nearly all of it per-process
//! overhead rather than per-connection state, and 255 of those needed more memory for the test rig
//! than the server under test was allowed to use. Two 255-player runs were killed by the operating
//! system's memory watchdog before the hold finished, which measures the harness and not the server.

use std::{env, process::ExitCode, time::Duration};

use terrustia_client::{Client, ClientError};
use tokio::time::{Instant, sleep};

#[tokio::main]
async fn main() -> ExitCode {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:7777".to_string());
    let seconds: u64 = env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    let depth: f32 = env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    // The server refuses duplicate names at the door, so multiple soak clients against one server
    // must each be given their own — otherwise only the first joins and the rest exit failing,
    // silently reducing a "three real players" soak to one. Defaults to "soak" for a lone run.
    let name = env::args().nth(4).unwrap_or_else(|| "soak".to_string());
    let count: usize = env::args()
        .nth(5)
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1);
    let Ok(addr) = addr.parse() else {
        eprintln!("bad address");
        return ExitCode::FAILURE;
    };

    // One client keeps the name and depth it was given, so every existing caller (soak_ci.sh, a
    // hand-run single soak) behaves exactly as before. Only the multi-client form derives them.
    if count == 1 {
        return match hold(addr, seconds, depth, name.clone()).await {
            Ok(held) => {
                println!("done after {held:?}");
                ExitCode::SUCCESS
            }
            Err(why) => {
                eprintln!("{why}");
                ExitCode::FAILURE
            }
        };
    }

    println!("joining {count} clients for {seconds}s");
    let mut tasks = Vec::with_capacity(count);
    for i in 1..=count {
        // Spread the clients down the column the way the shell harness used to, so they stream
        // different sections instead of all sitting in one.
        let depth = ((i % 60) * 8 + 200) as f32;
        let name = format!("{name}{i}");
        tasks.push(tokio::spawn(async move {
            hold(addr, seconds, depth, name).await
        }));
    }

    let mut ok = 0usize;
    let mut failed = 0usize;
    for (i, task) in tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok(_)) => ok += 1,
            Ok(Err(why)) => {
                failed += 1;
                eprintln!("client {}: {why}", i + 1);
            }
            Err(e) => {
                failed += 1;
                eprintln!("client {}: task panicked: {e}", i + 1);
            }
        }
    }

    println!("held {ok} / {count} for {seconds}s ({failed} lost)");
    if failed > 0 {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// One player: join, wander, and stay on until the hold is up. `Err` carries why the connection
/// ended early, which is the thing worth reporting.
async fn hold(
    addr: std::net::SocketAddr,
    seconds: u64,
    depth: f32,
    name: String,
) -> Result<Duration, String> {
    let mut client = Client::join(addr, &name)
        .await
        .map_err(|e| format!("could not join as {name:?}: {e}"))?;
    client.set_timeout(Duration::from_millis(50));
    let (sx, sy) = client.position();
    // Per client, not per process: `soak_ci.sh` greps each client's log for "joined at" and counts a
    // missing line as a failure, precisely so a run that quietly became a one-player run cannot pass.
    println!("{name} joined at ({sx}, {sy}); soaking for {seconds}s");

    let started = Instant::now();
    let mut step = 0i32;
    // Why the connection ended early, if it did. A soak client that keeps wandering after the
    // server has hung up is worse than useless: it reports success, and a run where the server
    // dropped every one of its clients is indistinguishable from one where it held them all. That
    // is not hypothetical. A 255-player run was observed dropping all 255 inside ninety seconds
    // while every client still printed "done" and exited zero, because the send result was
    // discarded and a read error only broke the drain loop.
    let mut dropped: Option<String> = None;

    'hold: while started.elapsed() < Duration::from_secs(seconds) {
        // Wander a few hundred tiles back and forth so sections keep streaming.
        let sweep = ((step % 240) - 120) as f32 * 16.0;
        if let Err(e) = client.move_to(sx + sweep, sy + depth * 16.0).await {
            dropped = Some(format!("sending movement failed: {e}"));
            break 'hold;
        }
        step += 1;
        // Drain what has arrived, bounded by time rather than by a count of events.
        //
        // Both bounds have been wrong in an instructive way. A fixed 64 events per 30 ms caps the
        // client at about 2100 a second; a full server sends more than that to a player with
        // company, so the shortfall accumulated in the receive buffer, TCP's window closed, and the
        // kernel eventually gave up retransmitting and killed the connection. That reads as
        // `socket read failed error=Operation timed out (os error 60)` on the server and a broken
        // pipe here, and looks exactly like the server dropping clients under load when it is the
        // test client failing to read.
        //
        // Raising the count instead (8192) broke it the other way. The server stops treating a
        // connection as still handshaking only once it has *received* more than `HANDSHAKE_FRAMES`
        // from it, and this loop sends one frame per pass, so a drain long enough to swallow the
        // join burst starved the sends: all 255 clients were closed at the 30 s handshake deadline
        // with "took too long to say who it was".
        //
        // A time budget satisfies both. The socket gets drained as fast as events can be parsed,
        // and the client always gets back to sending promptly, whatever the server is sending it.
        let drain_until = Instant::now() + Duration::from_millis(10);
        loop {
            match client.next_event().await {
                Ok(_) => {}
                // The read timeout is deliberately short and this is a poll, so "nothing has
                // arrived yet" is the ordinary way to finish a drain, not a failure.
                Err(ClientError::Timeout { .. }) => break,
                // Anything else means this client is no longer on the server: the connection was
                // closed, it was kicked, or the socket itself failed.
                Err(e) => {
                    dropped = Some(e.to_string());
                    break 'hold;
                }
            }
            if Instant::now() >= drain_until {
                break;
            }
        }
        sleep(Duration::from_millis(30)).await;
    }

    match dropped {
        Some(why) => Err(format!(
            "dropped after {:?} of the {seconds}s hold: {why}",
            started.elapsed()
        )),
        None => Ok(started.elapsed()),
    }
}
