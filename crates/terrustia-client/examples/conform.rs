//! Check `terrustia-proto` against a server it did not write.
//!
//! ```sh
//! # against the real game's dedicated server
//! cargo run --release -p terrustia-client --example conform -- 127.0.0.1:7930 real.trcap
//! # against ours, for the side-by-side
//! cargo run --release -p terrustia-client --example conform -- 127.0.0.1:7777 ours.trcap
//! ```
//!
//! ## Why this is different from every other test here
//!
//! `terrustia-client` is built on `terrustia-proto`, the crate the server encodes with. If a field
//! is read at the wrong width, both ends read it at the wrong width, they agree perfectly, and
//! every test passes. No test that pits our client against our server can find that, because the
//! bug is in the assumption they share rather than in either of them.
//!
//! Point this at a real `TerrariaServer` and the bytes on the wire were produced by Re-Logic's
//! code. They owe this project nothing. So:
//!
//! * a **decode** that succeeds says the layout is at least plausible;
//! * a **re-encode that is byte-identical** says the layout is *right* — every field, in order, at
//!   the correct width and signedness, with nothing missing off the end.
//!
//! The second is the one worth having, and it is the check this runs wherever the packet has an
//! encoder to run it with.
//!
//! ## What a run reports
//!
//! Three things, in descending order of what they are worth:
//!
//! 1. **Framing.** The whole inbound stream re-frames with nothing left over.
//! 2. **Byte-identical re-encodes**, per packet id, with the first mismatching offset named when
//!    one fails.
//! 3. **A census** of every id seen, so what the session did *not* cover is visible rather than
//!    being quietly mistaken for a pass.
//!
//! A session covers what it covers: an id that never arrived proves nothing either way, which is
//! why the census is printed rather than summarised into a score.

use std::{
    collections::BTreeMap, net::SocketAddr, path::PathBuf, process::ExitCode, time::Duration,
};

use terrustia_client::{Client, Event};
use terrustia_proto::{
    id,
    packets::{self, WorldData},
    section,
    writer::Writer,
};

/// How a single frame fared.
#[derive(Default)]
struct Tally {
    seen: usize,
    /// Frames that were decoded *and* re-encoded to the identical bytes.
    verified: usize,
    /// Frames decoded but with no encoder to check the bytes against.
    decoded_only: usize,
    /// The first failure of each kind, kept rather than counted: one worked example is more use
    /// than a number.
    problem: Option<String>,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(addr) = args.next() else {
        eprintln!("usage: conform <host:port> [capture.trcap]");
        return ExitCode::FAILURE;
    };
    let Ok(addr) = addr.parse::<SocketAddr>() else {
        eprintln!("not a socket address: {addr}");
        return ExitCode::FAILURE;
    };
    let capture = args.next().map(PathBuf::from);
    // How long to stay connected. A few seconds covers the join sequence and nothing else; the
    // packets that only appear once in a while — a sunrise, a shower starting, something spawning
    // and dying — need a session long enough to contain one.
    let soak = args
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(6));

    let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
    match runtime.block_on(run(addr, capture, soak)) {
        Ok(clean) => {
            if clean {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("conform: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(
    addr: SocketAddr,
    capture: Option<PathBuf>,
    soak: Duration,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut client = Client::connect(addr, "conform").await?;
    if let Some(path) = &capture {
        client.record_to(path)?;
        println!("recording to {}", path.display());
    }
    client.set_timeout(Duration::from_secs(20));

    let mut tallies: BTreeMap<u8, Tally> = BTreeMap::new();

    // The handshake is where the packets that gate entering a world live, so it is checked by
    // being *survived* — a real server that hangs up is a louder failure than any assertion — and
    // then the interesting packet is re-read from the capture below.
    client.handshake().await?;
    println!(
        "joined {:?}  {}x{}  spawn {},{}",
        client.world().name,
        client.world().width,
        client.world().height,
        client.world().spawn.0,
        client.world().spawn.1,
    );

    // Walk about so the server has reason to send more than the handshake: sections, NPCs, items,
    // projectiles, the clock. Wandering back and forth rather than in one direction, so a long run
    // keeps crossing the same ground and re-triggers whatever is spawning there instead of walking
    // off the edge of the world and standing still.
    let (sx, sy) = client.world().spawn;
    let deadline = tokio::time::Instant::now() + soak;
    let mut step = 0i32;
    while tokio::time::Instant::now() < deadline {
        let sway = if (step / 8) % 2 == 0 {
            step % 8
        } else {
            8 - step % 8
        };
        let x = i32::from(sx) + sway * 24;
        client.walk_to_tile(x, i32::from(sy)).await?;
        step += 1;
        let _ = client
            .try_wait_for("anything", |_| false, Duration::from_millis(400))
            .await;
    }

    // Drain whatever is left queued, folding each frame into the tally.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while tokio::time::Instant::now() < deadline {
        let left = deadline - tokio::time::Instant::now();
        match tokio::time::timeout(left, client.next_event()).await {
            Ok(Ok(Event::Other(frame))) => check(&mut tallies, frame.id, &frame.payload),
            // The typed events have already been decoded by the client on the way through; the
            // frame is consumed by that, so they are counted as decoded and not re-encoded.
            Ok(Ok(other)) => {
                let entry = tallies.entry(event_id(&other)).or_default();
                entry.seen += 1;
                entry.decoded_only += 1;
            }
            Ok(Err(e)) => {
                println!("stream ended: {e}");
                break;
            }
            Err(_) => break,
        }
    }
    client.flush_recording();

    // The handshake's own packets are only visible in the capture, since the client consumed them
    // on the way past. This is where packet 7 gets its byte-for-byte check.
    let mut verdict = true;
    if let Some(path) = &capture {
        drop(client);
        verdict = check_capture(path, &mut tallies)?;
    }

    println!(
        "\n{:<6} {:<28} {:>6} {:>9} {:>8}",
        "id", "name", "seen", "verified", "decoded"
    );
    for (packet_id, tally) in &tallies {
        println!(
            "{:<6} {:<28} {:>6} {:>9} {:>8}{}",
            packet_id,
            id::name(*packet_id),
            tally.seen,
            tally.verified,
            tally.decoded_only,
            tally
                .problem
                .as_ref()
                .map(|p| format!("   {p}"))
                .unwrap_or_default(),
        );
        if tally.problem.is_some() {
            verdict = false;
        }
    }

    let verified: usize = tallies.values().map(|t| t.verified).sum();
    let seen: usize = tallies.values().map(|t| t.seen).sum();
    println!(
        "\n{seen} frames over {} distinct ids; {verified} re-encoded byte-identically",
        tallies.len()
    );
    Ok(verdict)
}

/// Which tally an already-interpreted event belongs to.
fn event_id(event: &Event) -> u8 {
    match event {
        Event::Chat { .. } => id::NET_MODULES,
        Event::SectionLoaded { .. } => id::TILE_SECTION,
        Event::PlayerActive { .. } => id::PLAYER_ACTIVE,
        Event::PlayerMoved { .. } => id::PLAYER_CONTROLS,
        Event::TileChanged(_) => id::TILE_MANIPULATION,
        Event::NpcSynced(_) => id::SYNC_N_P_C,
        Event::ItemSynced(_) => id::SYNC_ITEM,
        Event::ItemReserved(_) => id::ITEM_OWNER,
        Event::EquipmentSynced(_) => id::SYNC_EQUIPMENT,
        Event::ItemDespawned(_) => id::SYNC_ITEM,
        Event::ProjectileSynced(_) => id::SYNC_PROJECTILE,
        Event::ProjectileKilled(_) => id::KILL_PROJECTILE,
        Event::PlayerHurt(_) => id::PLAYER_HURT_V2,
        Event::PlayerDied(_) => id::PLAYER_DEATH_V2,
        Event::LiquidChanged(_) => id::NET_MODULES,
        Event::FinishedConnecting => id::FINISHED_CONNECTING_TO_SERVER,
        Event::Other(frame) => frame.id,
    }
}

/// What checking one frame produced.
enum Checked {
    /// Decoded, and here is what re-encoding gave, alongside the bytes it should equal.
    ///
    /// The expected bytes are carried rather than assumed to be the payload, because a tile
    /// section is compared against its *inflated* stream: the compressed form differs between
    /// deflate implementations for reasons that have nothing to do with the protocol.
    Reencoded { ours: Vec<u8>, theirs: Vec<u8> },
    /// Decoded, but there is no encoder to check the bytes against.
    DecodedOnly,
}

/// Decode one frame, and where an encoder exists, re-encode it and compare the bytes.
fn check(tallies: &mut BTreeMap<u8, Tally>, packet_id: u8, payload: &[u8]) {
    let entry = tallies.entry(packet_id).or_default();
    entry.seen += 1;

    /// Decode with `decode`, re-encode, and hand back both sides of the comparison.
    macro_rules! round_trip {
        ($ty:path) => {
            <$ty>::decode(payload)
                .and_then(|value| value.encode())
                // The three framing bytes are ours, not theirs; only the payload is compared.
                .map(|frame| Checked::Reencoded {
                    ours: frame[3..].to_vec(),
                    theirs: payload.to_vec(),
                })
                .map_err(|e| e.to_string())
        };
    }

    // There used to be a `decode_only!` macro here, for "packets this crate never has to write".
    // Every id that had an encoder now uses `round_trip!`, so nothing was left for it to do;
    // `TileSquare` is the one remaining decode-only case and it needs its own arm regardless,
    // because decoding it takes a caller-supplied tile lookup.

    let outcome: Result<Checked, String> = match packet_id {
        id::WORLD_DATA => round_trip!(WorldData),
        id::PLAYER_SPAWN => round_trip!(packets::PlayerSpawn),
        id::PLAYER_LIFE_MANA => round_trip!(packets::PlayerHealth),
        id::PLAYER_MANA => round_trip!(packets::PlayerMana),
        // These four were `decode_only!` on the reasoning that this crate never has to *write*
        // them. True of the client, irrelevant to the check: the encoders exist because the server
        // sends all four every tick, and pointing them at Re-Logic's own bytes is exactly what this
        // example is for. Leaving them decode-only held the byte-verified count at 17 of 681 and
        // made it look like a limit of the protocol rather than of this match arm.
        id::SYNC_N_P_C => round_trip!(terrustia_proto::npc::SyncNpc),
        id::SYNC_PROJECTILE => round_trip!(terrustia_proto::projectile::SyncProjectile),
        // `SyncItem::encode` is the packet-21 form; `encode_instanced` is the same payload under
        // id 90, which is why the instanced arm below exists separately rather than sharing this.
        id::SYNC_ITEM => round_trip!(terrustia_proto::items::SyncItem),
        id::SPAWN_INSTANCED_ITEM => terrustia_proto::items::SyncItem::decode(payload)
            .and_then(|value| value.encode_instanced())
            .map(|frame| Checked::Reencoded {
                ours: frame[3..].to_vec(),
                theirs: payload.to_vec(),
            })
            .map_err(|e| e.to_string()),
        id::TILE_MANIPULATION => round_trip!(packets::TileManipulation),
        // Not the `decode_only!` macro: `TileSquare::decode` merges onto whatever tile is already
        // on the ground, so it needs a caller-supplied lookup. There is no real world state to
        // merge onto here (this only inspects a captured byte stream), so every position decodes
        // over bare air, same as `square.rs`'s own isolated round-trip tests do; that has no
        // bearing on this check, which only confirms the packet decodes without erroring.
        id::AREA_TILE_CHANGE => {
            terrustia_proto::square::TileSquare::decode(payload, |_, _| terrustia_proto::Tile::AIR)
                .map(|_| Checked::DecodedOnly)
                .map_err(|e| e.to_string())
        }
        // The richest check available. A section carries the tile bit-flags, the run-length
        // batching, the frame-importance table, and the chest, sign and tile-entity tails —
        // hundreds of decisions, every one of which has to match for the bytes to come back equal.
        id::TILE_SECTION => check_section(payload),
        // Nothing here claims to parse it, so nothing is asserted about it. Counted, not judged.
        _ => return,
    };

    match outcome {
        Ok(Checked::Reencoded { ours, theirs }) => {
            if ours == theirs {
                entry.verified += 1;
            } else if entry.problem.is_none() {
                entry.problem = Some(describe_mismatch(&theirs, &ours));
            }
        }
        Ok(Checked::DecodedOnly) => entry.decoded_only += 1,
        Err(e) => {
            if entry.problem.is_none() {
                entry.problem = Some(format!("DECODE FAILED: {e}"));
            }
        }
    }
}

/// Inflate a real section, decode it, and re-encode the uncompressed stream.
fn check_section(payload: &[u8]) -> Result<Checked, String> {
    let stream = section::inflate_section_payload(payload).map_err(|e| e.to_string())?;
    let (bounds, tiles, extras) =
        section::decode_section_stream(&stream).map_err(|e| e.to_string())?;

    let width = i32::from(bounds.width);
    let mut out = Writer::with_capacity(stream.len() + 64);
    section::write_section_stream(&mut out, bounds, &extras, |x, y| {
        let index = (y - bounds.y) * width + (x - bounds.x);
        tiles.get(index as usize).copied().unwrap_or_default()
    });

    Ok(Checked::Reencoded {
        ours: out.as_slice().to_vec(),
        theirs: stream,
    })
}

/// Say where two payloads first differ, which is the field that drifted.
fn describe_mismatch(theirs: &[u8], ours: &[u8]) -> String {
    if theirs.len() != ours.len() {
        return format!(
            "LENGTH: theirs {} bytes, ours {} bytes",
            theirs.len(),
            ours.len()
        );
    }
    match theirs.iter().zip(ours).position(|(a, b)| a != b) {
        Some(at) => format!(
            "BYTES DIFFER at offset {at}: theirs {:#04x}, ours {:#04x}",
            theirs[at], ours[at]
        ),
        None => "identical after all".into(),
    }
}

// ---------------------------------------------------------------- capture replay

/// Re-frame the recorded server-to-client stream and check every frame in it.
///
/// The live pass above misses the handshake, because the client consumes those frames on its way
/// into the world. The capture has them, and packet 7 is in there — the single most valuable
/// packet to check byte-for-byte, since it is the one that silently hangs a client when a field
/// drifts.
fn check_capture(
    path: &std::path::Path,
    tallies: &mut BTreeMap<u8, Tally>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let raw = std::fs::read(path)?;
    if !raw.starts_with(terrustia_client::tap::MAGIC) {
        return Err(format!("{} is not a TRCAP1 capture", path.display()).into());
    }

    // Reassemble the two directions, since a chunk is a socket read and may hold a partial frame.
    let mut inbound = Vec::new();
    let mut at = terrustia_client::tap::MAGIC.len();
    while at + 10 <= raw.len() {
        let direction = raw[at];
        let len = u32::from_le_bytes(raw[at + 6..at + 10].try_into().unwrap()) as usize;
        at += 10;
        if at + len > raw.len() {
            return Err(format!("capture truncated {} bytes into a chunk", raw.len() - at).into());
        }
        if direction == 1 {
            inbound.extend_from_slice(&raw[at..at + len]);
        }
        at += len;
    }

    println!("\ncapture: {} bytes from the server", inbound.len());

    let mut cursor = 0usize;
    let mut frames = 0usize;
    while cursor + 2 <= inbound.len() {
        let len = u16::from_le_bytes([inbound[cursor], inbound[cursor + 1]]) as usize;
        if len < 3 {
            println!("  FRAMING BROKE at byte {cursor}: length prefix says {len}");
            return Ok(false);
        }
        if cursor + len > inbound.len() {
            // A capture ends whenever the client stopped reading, so a partial trailing frame is
            // expected rather than a fault.
            println!(
                "  {} trailing byte(s): a partial frame the session ended in the middle of",
                inbound.len() - cursor
            );
            break;
        }
        let packet_id = inbound[cursor + 2];
        check(tallies, packet_id, &inbound[cursor + 3..cursor + len]);
        cursor += len;
        frames += 1;
    }
    println!("  re-framed into {frames} frames");
    Ok(true)
}
