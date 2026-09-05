use std::{net::SocketAddr, time::Duration};

use bytes::{Bytes, BytesMut};
use terrustia_proto::{NetworkText, packets};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, oneshot},
    time::timeout,
};
use tokio_util::codec::Decoder;
use tracing::{debug, warn};

use crate::{
    game::ServerEvent,
    net::codec::{CodecError, TerrariaCodec},
    net::record,
};

/// Frames queued for one client before it is considered too slow to keep, ignoring other players.
///
/// The burst a joining client is sent is much larger than it looks. Around forty section packets,
/// yes — but also one frame per item on the ground (up to 400), one per live NPC (up to 200), and
/// the world and progress packets. A flat 512 covered the sections and nothing else.
///
/// Chest contents are what makes it big. Every section carries the full inventory of every chest
/// inside it — forty frames per chest, as the game sends them — so one storage room in view of
/// spawn is worth more frames than the entire rest of the join put together. Fifty chests in a
/// section is two thousand frames from that section alone, and a player dropped for a full queue
/// is dropped silently, mid-load, with no message. The cost of being generous here is a queue of
/// pointers that is only ever that deep for the second or two a join takes.
const OUTBOUND_BASE: usize = 8192;

/// Extra room per player slot the server is configured for.
///
/// This constant used to be 256, sized against a theory that turned out to be only half right:
/// "every other player already in the world costs the newcomer their presence frames and one
/// relayed inventory slot each, ~200 frames" — a one-off *join-time* cost, paid once per newcomer.
/// A synchronized 255-player join (`examples/crowd`, the same shape `docs/performance.md`'s "Large
/// world, 16 and 255 players" section measured) dropped roughly half the crowd within 5-30 seconds
/// of joining even though that theory's own numbers fit comfortably inside the old queue. Reading
/// the drop log's own `packet`/`name` fields — which were sitting right there and unchecked —
/// shows why: the drops are almost entirely `PlayerControls` (movement) and a handful of
/// `SyncNPC`, and the dropped slots spread uniformly across the whole range rather than clustering
/// among the earliest joiners the way an unread join-time backlog would.
///
/// The real mechanism is `on_player_controls`'s unconditional broadcast in `game/server.rs`: once
/// everyone is moving, every one of up to `max_players - 1` peers relays a control packet to every
/// other peer roughly once a tick — genuinely O(n²) in player count, and nothing to do with *how*
/// the population got there, only how big it is. At `max_players = 255` that is up to 254 frames
/// landing in one already-established client's queue every ~16ms the moment the server's own
/// scheduling falls behind for even a few seconds — a loaded machine, a burst of `SyncNPC` while
/// wildlife spawns in — which 256 per player could not absorb at all. 4,096 survived every
/// `examples/crowd` trial at this player count (including a full 90-second run under real,
/// independently-confirmed machine contention, worst case 5 drops out of 255 at a quarter of this
/// number) with tick cost unaffected — this is a queue-sizing constant, not tick-cost work, and
/// `docs/performance.md`'s own re-measurement after this change confirms that directly rather than
/// assuming it. `OUTBOUND_BASE` above was not touched: no join-time packet (a section, a chest
/// slot, an item, a newcomer's own NPC burst) ever showed up in a drop log across any trial, so it
/// was never what overflowed here.
///
/// This is a mitigation, not a cure. The O(n²) relay cost is real and is vanilla's own behaviour —
/// this project transcribes it rather than inventing a throttle vanilla does not have — so a
/// deeper queue buys headroom without removing the underlying cost, and a big enough contention
/// spike could still in principle outrun any finite number. A genuine fix would look like NPC
/// sync's own answer to the same shape of problem (skip a client whose loaded sections cannot see
/// the thing being synced) applied to `on_player_controls`'s broadcast instead of widening this
/// queue further — but that is a change to `game/server.rs`'s broadcast logic, out of this file's
/// scope, and left for whoever owns that file next.
///
/// **That fix now exists** (`GameServer::broadcast_near`, applied to player movement and client
/// projectile syncs), and it was measured against this constant rather than assumed. Two 255-player
/// runs at the old 256, differing only in whether the cull was wired up, at matched NPC load:
///
/// | | no cull | cull |
/// |---|---|---|
/// | `outbound queue full` drops | 14 | 0 |
/// | clients held | 245/255 | 255/255 |
/// | peak queue depth | 73,465 of 73,472 | 38,713 |
///
/// So the cull does what this comment predicted: without it the queue runs literally full at the
/// pre-mitigation depth, and with it the peak halves and nothing is dropped.
///
/// **4,096 stays anyway.** A separate 255-player half-hour at this depth peaked at 578,447 queued
/// frames, eight times what 256 can hold, because queue depth is what absorbs a contention spike on
/// the test box: the game loop is descheduled, the backlog builds, and it drains again afterwards.
/// Shrinking the queue would convert that recoverable backlog into dropped players. The cull
/// removes the steady-state O(n²) term; the depth still covers the transient, and the two are not
/// substitutes for each other.
const OUTBOUND_PER_PLAYER: usize = 4096;

/// The queue depth to give one connection on a server configured for `max_players`.
pub fn outbound_queue(max_players: usize) -> usize {
    OUTBOUND_BASE + max_players * OUTBOUND_PER_PLAYER
}

/// The most memory every outbound queue on the server may hold between them, in bytes.
///
/// The depth above bounds one connection; nothing bounded the *sum*, and that is what the
/// 255-player soak kept running into. `queue_peak` reports only the deepest single connection, so
/// it under-reported the total: run 2 of the qualification soak reached 1536 MiB against a 1 GiB
/// ceiling with its backlog spread across many connections rather than piled on one, and
/// 255 slots times `outbound_queue(255)` frames is a theoretical ceiling in the tens of gigabytes.
///
/// Bounding the sum is the fix rather than shrinking the depth, because the depth is doing a real
/// job: `OUTBOUND_PER_PLAYER`'s own comment records that a transient backlog on a descheduled game
/// loop drains again afterwards, and that a shallower queue turns that recoverable case into
/// dropped players. So depth still covers the transient, and this covers the aggregate.
///
/// 256 MiB against the soak's own 1 GiB ceiling: a quarter of the budget for backlog, leaving the
/// world, the NPC and tile state and every other allocation the rest. A server that reaches this is
/// already in trouble - it means a quarter of a gigabyte of frames nobody has read - so the
/// response is to shed the connection holding the most of it, which is the same answer
/// `send_bytes` already gives a single connection whose own queue fills.
pub const OUTBOUND_TOTAL_BUDGET: usize = 256 * 1024 * 1024;

/// One connection's share of [`OUTBOUND_TOTAL_BUDGET`], and the whole server's, counted together.
///
/// Two counters rather than one because the two questions have different costs. "Are we over
/// budget?" is asked on every single send, so it has to be one atomic load; "who is holding it?" is
/// asked only when the answer to the first is yes, and can afford to walk the players.
///
/// [`Default`] gives a counter that shares its total with nobody, which is what a
/// [`crate::game::player::Player`] built outside a listener wants: it counts its own queue
/// correctly and cannot make an unrelated server look over budget.
#[derive(Clone, Debug, Default)]
pub struct QueuedBytes {
    mine: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    total: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl QueuedBytes {
    /// A fresh per-connection counter sharing `total` with every other connection.
    pub fn new(total: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
        Self {
            mine: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            total,
        }
    }

    /// Charge `bytes` to this connection and to the server.
    pub fn reserve(&self, bytes: usize) {
        use std::sync::atomic::Ordering::Relaxed;
        self.mine.fetch_add(bytes, Relaxed);
        self.total.fetch_add(bytes, Relaxed);
    }

    /// Give `bytes` back, once they have actually been written.
    ///
    /// Only what this connection was genuinely still holding comes off the *server's* total, which
    /// is why the two counters are not decremented by the same number. They differ in one case and
    /// it is a real one: [`Self::abandon`] zeroes `mine` for a connection being removed while its
    /// `write_loop` is mid-pop on a frame it took a moment earlier. Subtracting `bytes` from the
    /// total in both places would charge that frame back twice, leaving the shared counter below
    /// the true sum of every live connection's queue - permanently, and further below it after
    /// every disconnect, until a server genuinely over budget no longer sheds anything at all.
    pub fn release(&self, bytes: usize) {
        use std::sync::atomic::Ordering::Relaxed;
        let mut removed = 0;
        // The closure re-runs on contention, so `removed` ends up holding whatever the *winning*
        // attempt took off, which is the amount the total owes back.
        let _ = self.mine.fetch_update(Relaxed, Relaxed, |n| {
            removed = n.min(bytes);
            Some(n - removed)
        });
        if removed > 0 {
            let _ = self
                .total
                .fetch_update(Relaxed, Relaxed, |n| Some(n.saturating_sub(removed)));
        }
    }

    /// What this one connection is holding.
    pub fn mine(&self) -> usize {
        self.mine.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// What every connection is holding between them.
    pub fn total(&self) -> usize {
        self.total.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Hand back everything this connection was holding, for a player being removed: whatever is
    /// still queued for it will never be written now, so it must stop counting against the server.
    pub fn abandon(&self) {
        use std::sync::atomic::Ordering::Relaxed;
        let held = self.mine.swap(0, Relaxed);
        let _ = self
            .total
            .fetch_update(Relaxed, Relaxed, |n| Some(n.saturating_sub(held)));
    }
}

/// How the game task ends a connection without waiting for its outbound queue to drain.
///
/// Dropping a [`crate::game::player::Player`] drops the last `mpsc::Sender` for its queue, and that
/// is *not* enough on its own: `mpsc::Receiver::recv` keeps handing out everything already buffered
/// before it returns `None`, so `write_loop` goes on feeding a closed connection until the backlog
/// runs dry. At `outbound_queue(255)` that backlog is about a million frames, and a client shed for
/// being too slow is by definition one that takes a long time to read them - half an hour, in the
/// 255-player soak this was found in (`TODO.md`, "the retention clause"). It reads a stale world
/// the whole time and only learns it was dropped at the end of it.
///
/// So the decision travels on its own channel rather than being inferred from the queue running
/// out. Firing it says "this connection is over and whatever is still queued no longer matters";
/// `Some(frame)` carries the one frame that does still matter, a kick notice, which goes on the
/// wire *ahead* of the abandoned backlog instead of behind it.
///
/// Dropping the sender without firing it is deliberately not the same thing: that is what happens
/// when the game task itself goes away, and those clients are still owed what they have already
/// been queued (`/stop` and the rollback path both announce before they stop). `write_loop` treats
/// that case exactly as it always did, by draining.
///
/// **What this does not close.** `write_loop` notices the decision between writes, not during one,
/// so a client that has stopped reading its socket altogether still holds `write_all` for as long
/// as the kernel will wait, exactly as it did before. Cancelling a write mid-frame would fix that
/// and cost every kick notice its readable stream, so the bound here is one `WRITE_BATCH` (64 KiB)
/// rather than nothing at all: against the million-frame backlog this exists for, one batch is the
/// difference between seconds and half an hour.
pub type Closer = oneshot::Sender<Option<Bytes>>;

/// Starting read buffer. Section packets are large, so a bigger buffer avoids repeated growth.
const READ_BUFFER: usize = 16 * 1024;

/// How many frames a connection may send before it counts as having handshaked.
///
/// The real sequence is a version, a UUID, a player, an inventory and a spawn request — a couple
/// of dozen frames with the inventory slots. Past that it is an ordinary session and the idle
/// timeout is the right rule; before it, the connection is on a deadline.
const HANDSHAKE_FRAMES: u32 = 64;

/// Serve one accepted connection until it closes.
/// The limits a connection is served under.
///
/// Grouped because they travel together and are set once from the config; passing them
/// individually made `serve` a list of bare `Duration`s that were easy to swap by accident.
// Not `Copy`: `queued_total` is a shared handle, and a type that silently copies one reads as
// though each copy got its own counter.
#[derive(Debug, Clone)]
pub struct Limits {
    /// How long a single read may block before the connection is considered idle.
    pub idle: Duration,
    /// How long the whole handshake has, regardless of how often bytes trickle in.
    pub handshake: Duration,
    /// Capacity of this connection's outbound queue.
    pub outbound_queue: usize,
    /// The whole server's outbound backlog, in bytes, shared by every connection.
    ///
    /// Lives here rather than being made inside `serve` because it is precisely the thing that has
    /// to be *shared*: one counter per connection would bound each of them again, which
    /// `outbound_queue` already does, and would leave the sum as unbounded as it was.
    pub queued_total: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

pub async fn serve(
    stream: TcpStream,
    addr: SocketAddr,
    events: mpsc::Sender<ServerEvent>,
    limits: Limits,
    recorder: Option<record::Recorder>,
    // Held for the life of the connection, releasing its place on every exit path including a
    // panic. Never read — its whole job is to be dropped.
    _slot_held: crate::net::listener::ConnectionSlot,
) {
    let idle_timeout = limits.idle;
    let outbound_queue = limits.outbound_queue;
    let handshake_deadline = std::time::Instant::now() + limits.handshake;
    // Terraria is latency-sensitive and its packets are small; batching them hurts responsiveness.
    if let Err(e) = stream.set_nodelay(true) {
        debug!(%addr, error = %e, "could not disable Nagle");
    }

    let queued = QueuedBytes::new(limits.queued_total.clone());
    let (out_tx, out_rx) = mpsc::channel::<Bytes>(outbound_queue);
    let (slot_tx, slot_rx) = oneshot::channel();
    // See [`Closer`]: the game task's way of ending this connection now rather than at the far end
    // of whatever it has already queued.
    let (close_tx, close_rx) = oneshot::channel::<Option<Bytes>>();

    if events
        .send(ServerEvent::Join {
            addr,
            out: out_tx,
            close: close_tx,
            slot: slot_tx,
            queued: queued.clone(),
        })
        .await
        .is_err()
    {
        return; // the game task is gone; nothing to serve
    }

    let (mut read_half, mut write_half) = stream.into_split();

    // `epoch` is this connection's generation counter for `slot` (see
    // `game::server::GameServer::remove_player`'s doc comment): stamped onto every `Packet` below
    // and onto this connection's own eventual `Leave`, so the game task can tell this connection's
    // events apart from a ghost's once the slot has been recycled.
    let (slot, epoch) = match slot_rx.await {
        Ok(Some(assigned)) => assigned,
        _ => {
            // Full, or the game task dropped the request. Say so rather than closing silently:
            // a bare disconnect shows up in the client as an unexplained failure.
            if let Ok(frame) = packets::kick(&NetworkText::literal("This server is full.")) {
                let _ = write_half.write_all(&frame).await;
                let _ = write_half.flush().await;
            }
            return;
        }
    };

    let writer = tokio::spawn(write_loop(
        out_rx,
        close_rx,
        write_half,
        slot,
        recorder.clone(),
        queued,
    ));
    let reason = read_loop(
        &mut read_half,
        slot,
        epoch,
        &events,
        idle_timeout,
        handshake_deadline,
        recorder.as_ref(),
    )
    .await;
    debug!(%addr, slot, epoch, %reason, "connection closed");

    // Removing the player fires this connection's [`Closer`], which ends the write task without
    // it having to drain first. The abort is still here for the case that does not reach: a game
    // task that never gets to handle this `Leave` at all.
    let _ = events.send(ServerEvent::Leave { slot, epoch }).await;
    writer.abort();
}

/// Pump the outbound queue to the socket.
///
/// Frames arrive fully encoded, so this is a plain write rather than a `FramedWrite`.
/// How much of the queue one write may coalesce.
///
/// Frames are small and there are a lot of them: with players in sight of one another, every
/// movement one makes is a frame to each of the others. Writing them one at a time is one syscall
/// each, and the queue then drains slower than the game fills it — which shows up not as slowness
/// but as clients being *dropped* for falling behind. Gathering whatever is already waiting into
/// one write costs nothing and is what stops that.
const WRITE_BATCH: usize = 64 * 1024;

async fn write_loop(
    mut out: mpsc::Receiver<Bytes>,
    mut close: oneshot::Receiver<Option<Bytes>>,
    mut sink: tokio::net::tcp::OwnedWriteHalf,
    slot: u8,
    recorder: Option<record::Recorder>,
    queued: QueuedBytes,
) {
    let mut batch: Vec<u8> = Vec::with_capacity(WRITE_BATCH);
    // The last frame this connection is owed, if the game task named one when it closed us.
    let mut farewell: Option<Bytes> = None;
    // Cleared once `close` has produced something, so its branch is never polled again. Only the
    // `Err` arm below leaves the loop running, and it must not be re-polled after completing.
    let mut watch_close = true;
    loop {
        let frame = tokio::select! {
            // Biased, and the close first, so that the decision beats the queue whenever both are
            // ready. It always is both: this exists for a connection whose queue is *full*, and a
            // fair select against a never-empty queue is a coin toss per iteration rather than an
            // answer. Nothing legitimate can be queued after the close is fired either, because
            // the game task removes the player in the same `&mut self` call that fires it and
            // `send_bytes` finds no player after that - so everything still in the queue at this
            // point is, by construction, backlog from before the decision.
            biased;
            closed = &mut close, if watch_close => match closed {
                Ok(last) => {
                    farewell = last;
                    break;
                }
                // The game task went away without deciding anything (its own shutdown drops every
                // `Player`). Those clients are still owed what they were queued, so this drains
                // exactly as it did before the closer existed.
                Err(_) => {
                    watch_close = false;
                    continue;
                }
            },
            frame = out.recv() => match frame {
                Some(frame) => frame,
                None => break,
            },
        };
        batch.clear();
        batch.extend_from_slice(&frame);
        queued.release(frame.len());
        // Everything else already queued goes out in the same write. `try_recv` never waits, so
        // this only ever gathers what the game task has *already* produced.
        while batch.len() < WRITE_BATCH {
            match out.try_recv() {
                Ok(next) => {
                    batch.extend_from_slice(&next);
                    queued.release(next.len());
                }
                Err(_) => break,
            }
        }
        // Recorded before the write, so a batch that fails to go out is still visible as the last
        // thing this server tried to say.
        if let Some(recorder) = &recorder {
            recorder.chunk(record::Direction::Outbound, slot, &batch);
        }
        if sink.write_all(&batch).await.is_err() {
            break;
        }
    }
    // Ahead of the backlog rather than behind it, which is the whole point: a kicked client that
    // was a million frames behind used to be told why at the far end of those million frames.
    if let Some(frame) = farewell {
        if let Some(recorder) = &recorder {
            recorder.chunk(record::Direction::Outbound, slot, &frame);
        }
        let _ = sink.write_all(&frame).await;
    }
    let _ = sink.shutdown().await;
}

/// Decode frames off the socket and forward them to the game task.
async fn read_loop(
    read: &mut tokio::net::tcp::OwnedReadHalf,
    slot: u8,
    epoch: u32,
    events: &mpsc::Sender<ServerEvent>,
    idle_timeout: Duration,
    handshake_deadline: std::time::Instant,
    recorder: Option<&record::Recorder>,
) -> &'static str {
    let mut codec = TerrariaCodec;
    let mut buf = BytesMut::with_capacity(READ_BUFFER);
    // Cleared once the connection has said anything at all beyond the first frame. Until then it
    // is on a clock: `idle_timeout` wraps each individual *read*, so its timer resets on any byte
    // and a connection trickling one byte a minute would hold its place for ever.
    let mut still_handshaking = true;
    let mut frames_seen = 0u32;

    loop {
        // Drain everything already buffered before waiting on the socket again.
        loop {
            match codec.decode(&mut buf) {
                Ok(Some(frame)) => {
                    // A handful of frames is the whole handshake; past that it is a real session
                    // and the ordinary idle timeout is the right rule.
                    frames_seen = frames_seen.saturating_add(1);
                    if frames_seen > HANDSHAKE_FRAMES {
                        still_handshaking = false;
                    }
                    if events
                        .send(ServerEvent::Packet { slot, epoch, frame })
                        .await
                        .is_err()
                    {
                        return "server shutting down";
                    }
                }
                Ok(None) => break,
                Err(CodecError::FrameTooShort { len }) => {
                    warn!(slot, len, "desynchronised stream");
                    return "protocol error";
                }
                Err(e) => {
                    warn!(slot, error = %e, "decode failed");
                    return "protocol error";
                }
            }
        }

        // Whichever runs out first: the per-read idle timer, or the one-shot deadline for
        // getting through the handshake at all.
        let wait = if still_handshaking {
            idle_timeout.min(
                handshake_deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .max(Duration::from_millis(1)),
            )
        } else {
            idle_timeout
        };
        if still_handshaking && std::time::Instant::now() >= handshake_deadline {
            return "took too long to say who it was";
        }

        let before = buf.len();
        match timeout(wait, read.read_buf(&mut buf)).await {
            Ok(Ok(0)) => return "client closed",
            Ok(Ok(_)) => {
                // Exactly the bytes that arrived, before anything here has looked at them. This
                // is the whole value of a capture: ground truth that owes nothing to our own
                // idea of what the client should have sent.
                if let Some(recorder) = recorder {
                    recorder.chunk(record::Direction::Inbound, slot, &buf[before..]);
                }
            }
            Ok(Err(e)) => {
                debug!(slot, error = %e, "socket read failed");
                return "read error";
            }
            Err(_) if still_handshaking && std::time::Instant::now() >= handshake_deadline => {
                return "took too long to say who it was";
            }
            Err(_) if still_handshaking => continue,
            Err(_) => return "idle timeout",
        }
    }
}

/// What [`Closer`] is for, over a real socket: a connection the game task has finished with stops
/// writing where it stands, and one whose game task merely went away still gets what it was owed.
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A disconnect and its own `write_loop` racing over the same frame: the game task abandons
    /// the connection's whole queue while the writer, having popped a frame a moment earlier,
    /// releases it. The frame must come off the shared total once, not twice - the second
    /// subtraction would be taken out of a *different* connection's share, and the drift is one
    /// way, so a long-lived server would gradually stop believing anything was queued at all.
    #[test]
    fn releasing_after_abandoning_does_not_charge_the_frame_back_twice() {
        let total = std::sync::Arc::<std::sync::atomic::AtomicUsize>::default();
        let leaving = QueuedBytes::new(total.clone());
        let staying = QueuedBytes::new(total.clone());

        leaving.reserve(100);
        staying.reserve(100);
        assert_eq!(leaving.total(), 200);

        leaving.abandon();
        assert_eq!(
            leaving.total(),
            100,
            "abandoning must return exactly what the leaving connection held"
        );

        // Its writer, which had already taken that frame off the channel, now says so.
        leaving.release(100);
        assert_eq!(
            leaving.total(),
            100,
            "the connection that is still here is still holding its 100 bytes"
        );
        assert_eq!(staying.mine(), 100, "and it never gave them up");
    }

    /// One kilobyte of filler. Static so a test can queue thousands of them for nothing.
    static FILLER: [u8; 1024] = [7; 1024];
    /// Stands in for a kick notice: the one frame a closed connection is still owed.
    static NOTICE: [u8; 6] = *b"kicked";

    /// A connected pair: the client end, and the server end already split the way `serve` splits
    /// it. The read half is handed back only to be held: dropping it is harmless, but keeping it
    /// alive makes these tests a socket rather than a half of one.
    async fn pair() -> (
        TcpStream,
        tokio::net::tcp::OwnedReadHalf,
        tokio::net::tcp::OwnedWriteHalf,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let addr = listener.local_addr().expect("the bound address");
        let client = TcpStream::connect(addr).await.expect("connecting");
        let (server, _) = listener.accept().await.expect("accepting");
        let (read, write) = server.into_split();
        (client, read, write)
    }

    /// Everything the client can read before the server closes on it, with a ceiling on how long
    /// this may take so a regression fails the test instead of hanging the suite.
    async fn read_to_end(client: &mut TcpStream) -> Vec<u8> {
        let mut seen = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        timeout(Duration::from_secs(30), async {
            loop {
                match client.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => seen.extend_from_slice(&buf[..n]),
                }
            }
        })
        .await
        .expect("the server must close its end rather than write for ever");
        seen
    }

    /// The bug this whole mechanism exists for: a client shed under backpressure used to go on
    /// being fed its stale backlog, and only learned it had been dropped once the backlog ran out.
    /// Four megabytes of it here; at `outbound_queue(255)` the real thing is a million frames.
    ///
    /// The decision is taken before the writer is spawned, which is exactly the shape of the real
    /// one: `send_bytes` fires it from inside a tick, while the write task is between polls.
    #[tokio::test]
    async fn a_closed_connection_abandons_its_backlog_instead_of_draining_it() {
        let (mut client, _read, sink) = pair().await;
        let (out_tx, out_rx) = mpsc::channel::<Bytes>(4096);
        for _ in 0..4096 {
            out_tx
                .try_send(Bytes::from_static(&FILLER))
                .expect("the queue was sized for exactly this");
        }
        // So that a write loop which ignored the closer would still reach an end and fail the
        // assertion below, rather than blocking on `recv` for ever and hanging the suite.
        drop(out_tx);

        let (close_tx, close_rx) = oneshot::channel();
        close_tx.send(None).expect("the write task is still there");
        tokio::spawn(write_loop(
            out_rx,
            close_rx,
            sink,
            0,
            None,
            QueuedBytes::new(std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0))),
        ));

        assert!(
            read_to_end(&mut client).await.is_empty(),
            "a shed connection must not deliver a single frame of the backlog it was dropped for"
        );
    }

    /// The other half of the same decision: the notice rides the closer, so it goes out *ahead* of
    /// the abandoned backlog. Queued behind it, as `kick` used to, it would arrive at the far end
    /// of four megabytes the client no longer has any reason to read.
    #[tokio::test]
    async fn a_kick_notice_goes_out_ahead_of_the_abandoned_backlog() {
        let (mut client, _read, sink) = pair().await;
        let (out_tx, out_rx) = mpsc::channel::<Bytes>(4096);
        for _ in 0..4096 {
            out_tx.try_send(Bytes::from_static(&FILLER)).expect("room");
        }
        drop(out_tx);

        let (close_tx, close_rx) = oneshot::channel();
        close_tx
            .send(Some(Bytes::from_static(&NOTICE)))
            .expect("the write task is still there");
        tokio::spawn(write_loop(
            out_rx,
            close_rx,
            sink,
            0,
            None,
            QueuedBytes::new(std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0))),
        ));

        // Length first, so a regression reports a number rather than printing four megabytes of
        // filler at whoever ran the suite.
        let seen = read_to_end(&mut client).await;
        assert_eq!(
            seen.len(),
            NOTICE.len(),
            "the notice and nothing else: the backlog behind it is stale by definition"
        );
        assert_eq!(seen, NOTICE.to_vec());
    }

    /// The same decision taken against a writer that is already running and already draining,
    /// which is the ordering `biased` in the `select!` is there for. Sixteen megabytes is far more
    /// than any socket buffer holds, so a writer that ran to the end of the queue would deliver
    /// all of it; the fix bounds what escapes at whatever is already in flight plus one batch.
    #[tokio::test]
    async fn closing_a_connection_that_is_already_draining_stops_it_where_it_stands() {
        let (mut client, _read, sink) = pair().await;
        let queued = 16 * 1024;
        let (out_tx, out_rx) = mpsc::channel::<Bytes>(queued);
        for _ in 0..queued {
            out_tx.try_send(Bytes::from_static(&FILLER)).expect("room");
        }
        drop(out_tx);

        let (close_tx, close_rx) = oneshot::channel();
        tokio::spawn(write_loop(
            out_rx,
            close_rx,
            sink,
            0,
            None,
            QueuedBytes::new(std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0))),
        ));

        // Read once first: the writer cannot have got this far without being under way, so the
        // close below lands on a loop that is already in the middle of the backlog.
        let mut buf = [0u8; 16 * 1024];
        let first = timeout(Duration::from_secs(30), client.read(&mut buf))
            .await
            .expect("the writer must start writing")
            .expect("a readable socket");
        assert!(first > 0, "the writer should have begun draining");

        close_tx.send(None).expect("the write task is still there");
        let rest = read_to_end(&mut client).await.len();

        let delivered = first + rest;
        assert!(
            delivered < 4 * 1024 * 1024,
            "a closed connection must stop where it stands, not finish the queue: \
             {delivered} bytes of {} went out",
            queued * FILLER.len()
        );
    }

    /// And the case that must *not* change. The game task's own shutdown drops every `Player`,
    /// which drops both ends of this with nothing decided - and `/stop` and the rollback path both
    /// announce to everybody before they do it. Those frames are owed, so the queue still drains.
    #[tokio::test]
    async fn a_closer_dropped_without_a_decision_still_drains_what_was_queued() {
        let (mut client, _read, sink) = pair().await;
        let (out_tx, out_rx) = mpsc::channel::<Bytes>(64);
        for _ in 0..64 {
            out_tx.try_send(Bytes::from_static(&FILLER)).expect("room");
        }
        drop(out_tx);

        let (close_tx, close_rx) = oneshot::channel::<Option<Bytes>>();
        drop(close_tx);
        tokio::spawn(write_loop(
            out_rx,
            close_rx,
            sink,
            0,
            None,
            QueuedBytes::new(std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0))),
        ));

        assert_eq!(
            read_to_end(&mut client).await.len(),
            64 * FILLER.len(),
            "a shutting-down server still owes its clients what it has already queued for them"
        );
    }
}
