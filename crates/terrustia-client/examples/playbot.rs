//! Play toward the goals a person has, and report the ones that could not be reached.
//!
//! ```sh
//! ./tools/playbot.sh          # the whole thing, server lifecycle included
//! ```
//!
//! or, against a server somebody else is running:
//!
//! ```sh
//! cargo run --release -p terrustia-client --example playbot -- 127.0.0.1:7777 build /tmp/pb.state
//! # restart the server on the same world file, then
//! cargo run --release -p terrustia-client --example playbot -- 127.0.0.1:7777 verify /tmp/pb.state
//! ```
//!
//! ## Why this exists
//!
//! Two thousand unit tests were green while Moon Lord could not be killed, every pre-hardmode
//! caster's only attack hung motionless at the muzzle, placing a chest did nothing at all
//! server-side, doors reverted on reload, Gel dropped from nothing, and walking out of the block
//! sent at join showed sky forever. Each of those was found by a person playing for an hour.
//!
//! They are all *absences*: nothing is ever in an illegal state, the thing simply never happens.
//! An invariant checker walks straight past them. A goal does not, because the goal is the
//! assertion: "there is Gel in my hands" fails when Gel drops from nothing, and no amount of
//! self-consistency saves it.
//!
//! ## What is honest here
//!
//! Setup uses the console (`/spawn`, `/butcher`, `/time`) where a real player would use an item
//! this client has no way to hold. Setup only: no milestone is *asserted* by asking the console
//! whether it worked. Every row below says what it expected and what it got.
//!
//! [`Milestones`] lists what is deliberately absent and why, rather than shipping a check that
//! cannot fail.
//!
//! Copyright (C) 2026 Brooklyn Halmstad.
//! Licensed under the GNU Affero General Public License v3.0 or later; see LICENSE.

use std::{
    collections::HashMap,
    env, fmt, fs,
    process::ExitCode,
    time::{Duration, Instant},
};

use terrustia_client::{Client, ClientError, Event};
use terrustia_proto::{
    ItemStack, PacketWriter, id,
    npc_data::npc_stats,
    objects::{SyncChestItem, SyncPlayerChest},
    reader::PacketReader,
};

// --- what the goals are about -----------------------------------------------------------------

/// Gel: torches, and the Slime Crown that summons King Slime. It drops from slimes and nowhere
/// else, which is what made its absence end a playthrough at the first night.
const GEL: i32 = 23;
/// The slime it comes off.
const BLUE_SLIME: u16 = 1;
/// Bone. Every Bone recipe waits on it, and it comes only from the Angry Bones family.
const BONE: i32 = 154;
/// The only NPC types that ever give out [`BONE`] (`conditional_drops.rs`, the `31 | 32 | 34 |
/// 294 | 295 | 296 | 693` arm). A Skeleton, despite the name, does not.
const ANGRY_BONES: &str = "AngryBones";
const ANGRY_BONES_TYPE: u16 = 31;
/// One in three per kill, so one fight proves nothing. Enough of them that a real drop is not
/// missed by luck: `1 - (2/3)^12` is over 99%.
const BONE_KILLS: u32 = 12;
/// King Slime. Summonable, which is the path a Slime Crown takes.
const KING_SLIME: u16 = 50;
/// A pre-hardmode caster whose whole threat is one thrown orb.
const CASTER: &str = "GoblinSorcerer";
/// What it throws: the Chaos Ball. An *NPC*, because Terraria implements a caster's shot as an NPC
/// with one hit point and no gravity (`ai/orb.rs`, style 9). Watching only the projectile
/// subsystem would miss every pre-hardmode caster in the game.
const CASTER_ORB: u16 = 30;
/// Harpy and Wyvern: the sky's own roster.
const SKY_MOBS: [u16; 2] = [48, 87];
/// How high up "the sky" is, and how far down from it to accept a floating island as a perch.
const SKY_Y: i32 = 60;
const SKY_FLOOR: i32 = 180;
/// How far from spawn counts as "a long way": six sections, well outside the block sent at join.
const FAR: i32 = 1200;
/// Chest tile, door tile, and the door's open form.
const CHEST_TILE: u16 = 21;
const DOOR_SHUT: u16 = 10;
const DOOR_OPEN: u16 = 11;

/// Goals that a person has but this client cannot drive, left out rather than faked.
///
/// A milestone that cannot fail is worse than a missing one, and this repository has five known
/// examples of exactly that.
struct Milestones;
impl Milestones {
    const ABSENT: &'static [(&'static str, &'static str)] = &[(
        "craft a torch (Gel) and a bone recipe",
        "there is no crafting packet in the protocol at all: vanilla crafts entirely \
             client-side and only syncs the resulting inventory slot. The reachable half, \
             'is Gel/Bone obtainable', is asserted below; the crafting step itself has no \
             server behaviour to check.",
    )];
}

// --- the report -------------------------------------------------------------------------------

struct Row {
    goal: &'static str,
    budget: Duration,
    took: Duration,
    ok: bool,
    /// What was expected and what turned up, in a sentence somebody can act on.
    detail: String,
}

#[derive(Default)]
struct Report {
    rows: Vec<Row>,
}

impl Report {
    fn record(
        &mut self,
        goal: &'static str,
        budget: Duration,
        started: Instant,
        ok: bool,
        detail: String,
    ) {
        let took = started.elapsed();
        println!("  {}  {goal}", if ok { "reached" } else { "MISSED " });
        println!(
            "           budget {}s, took {}s: {detail}",
            budget.as_secs(),
            took.as_secs()
        );
        self.rows.push(Row {
            goal,
            budget,
            took,
            ok,
            detail,
        });
    }

    fn missed(&self) -> usize {
        self.rows.iter().filter(|r| !r.ok).count()
    }
}

// --- what the bot noticed while it played -------------------------------------------------------

/// Anything in the world followed across its updates, NPC or projectile alike.
///
/// The inert-orb bug put the attack in the air and left it there: it existed, it had the right
/// type, it did no harm, and every "does anything get thrown" check passed. Distance from where it
/// first appeared is the thing that tells them apart.
#[derive(Clone, Copy)]
struct Flight {
    kind: u16,
    life: i32,
    first: (f32, f32),
    last: (f32, f32),
    samples: u32,
}

impl Flight {
    fn travelled(&self) -> f32 {
        let (dx, dy) = (self.last.0 - self.first.0, self.last.1 - self.first.1);
        (dx * dx + dy * dy).sqrt()
    }
}

#[derive(Default)]
struct Seen {
    /// Live NPCs, keyed by slot *and* generation, so a reused slot is a new creature rather than
    /// an old one that teleported.
    npcs: HashMap<(u8, u8), Flight>,
    /// Every type that appeared, with the lowest life it was ever reported at, so a failure can
    /// separate "never spawned" from "would not die".
    types: HashMap<u16, i32>,
    /// How many of each type actually died, which is what a drop rate is per.
    deaths: HashMap<u16, u32>,
    items: Vec<ItemStack>,
    /// Keyed by the wire's own packed slot-plus-generation, for the same reason.
    flights: HashMap<i32, Flight>,
    /// What the server last said about a container placement: action, x, y, id.
    chest_update: Option<(u8, i16, i16, i16)>,
    /// Which chest the server says is open, and where it is.
    chest_open: Option<(i16, i16, i16)>,
    chest_slots: HashMap<u8, ItemStack>,
    chat: Vec<String>,
}

impl Seen {
    fn fold(&mut self, event: &Event) {
        match event {
            Event::NpcSynced(npc) => {
                // A death carries nothing but the slot. The server announces one as `SyncNpc {
                // index, generation: 0, net_id: 0, life: 0 }`, which is right: a real client
                // already knows what stood there and only needs telling that it is gone. So
                // resolve a death the way a real client does, by index against what we were told
                // before, rather than believing the type field, which reads as npc 0 and would
                // credit every kill in the game to a creature that does not exist.
                if npc.life <= 0 {
                    let dead = self
                        .npcs
                        .iter()
                        .find(|((index, _), _)| *index == npc.index)
                        .map(|(key, seat)| (*key, seat.kind));
                    if let Some((key, kind)) = dead {
                        self.types.insert(kind, 0);
                        *self.deaths.entry(kind).or_default() += 1;
                        self.npcs.remove(&key);
                    }
                    return;
                }
                let kind = npc.npc_type();
                let low = self.types.entry(kind).or_insert(i32::MAX);
                *low = (*low).min(npc.life);
                let seat = self
                    .npcs
                    .entry((npc.index, npc.generation))
                    .or_insert(Flight {
                        kind,
                        life: npc.life,
                        first: npc.position,
                        last: npc.position,
                        samples: 0,
                    });
                seat.life = npc.life;
                seat.last = npc.position;
                seat.samples += 1;
            }
            Event::ItemSynced(sync) => self.items.push(sync.item),
            Event::ProjectileSynced(sync) => {
                let flight = self.flights.entry(sync.key.pack()).or_insert(Flight {
                    kind: sync.projectile_type.max(0) as u16,
                    life: 1,
                    first: sync.position,
                    last: sync.position,
                    samples: 0,
                });
                flight.last = sync.position;
                flight.samples += 1;
            }
            Event::Chat { text, .. } => self.chat.push(text.clone()),
            Event::Other(frame) => self.fold_frame(frame),
            _ => {}
        }
    }

    /// The container packets, which the client has no event for because only a bot cares.
    fn fold_frame(&mut self, frame: &terrustia_client::Frame) {
        match frame.id {
            id::CHEST_UPDATES => {
                let mut r = PacketReader::new(&frame.payload);
                if let (Ok(action), Ok(x), Ok(y), Ok(_style), Ok(chest)) =
                    (r.u8(), r.i16(), r.i16(), r.i16(), r.i16())
                {
                    self.chest_update = Some((action, x, y, chest));
                }
            }
            id::SYNC_CHEST_ITEM => {
                if let Ok(sync) = SyncChestItem::decode(&frame.payload) {
                    self.chest_slots.insert(sync.slot, sync.item);
                }
            }
            id::SYNC_PLAYER_CHEST => {
                if let Ok(sync) = SyncPlayerChest::decode(&frame.payload) {
                    self.chest_open = Some((sync.chest, sync.x, sync.y));
                }
            }
            _ => {}
        }
    }

    /// The types that turned up, named, for a failure that has to say what it saw instead.
    fn roster(&self) -> String {
        if self.types.is_empty() {
            return "nothing at all".to_string();
        }
        let mut names: Vec<String> = self
            .types
            .keys()
            .map(|t| format!("{} ({t})", npc_stats(*t).map_or("?", |stats| stats.name)))
            .collect();
        names.sort();
        names.join(", ")
    }
}

/// A projectile that only moved this far never really left the muzzle.
const MUZZLE: f32 = 16.0;

// --- driving ------------------------------------------------------------------------------------

/// Stand somewhere and live for a while, folding everything that arrives.
///
/// A real client talks constantly, so this keeps reporting its position: a bot that waits quietly
/// looks exactly like a dead socket to the server, and gets dropped as one. Returns `false` if the
/// connection died, which makes every later milestone meaningless rather than merely failed.
async fn play(
    client: &mut Client,
    stand: (f32, f32),
    seen: &mut Seen,
    how_long: Duration,
    swinging: bool,
) -> bool {
    let deadline = Instant::now() + how_long;
    let mut last_act = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    while Instant::now() < deadline {
        if last_act.elapsed() >= Duration::from_millis(120) {
            last_act = Instant::now();
            if client.move_to(stand.0, stand.1).await.is_err() {
                return false;
            }
            if swinging {
                // Everything alive and hostile, a few times a second. Swinging on every event
                // instead floods the connection: a boss is many parts, each syncing.
                let targets: Vec<(u8, u8)> = seen
                    .npcs
                    .iter()
                    .filter(|((_, _), npc)| {
                        npc.life > 0 && npc_stats(npc.kind).is_some_and(|s| !s.friendly)
                    })
                    .map(|((index, generation), _)| (*index, *generation))
                    .collect();
                for (index, generation) in targets {
                    if client
                        .hit_npc(index, generation, 30_000, 0.0, 0)
                        .await
                        .is_err()
                    {
                        return false;
                    }
                }
            }
        }
        match client.next_event().await {
            Ok(Event::PlayerDied(_)) => {
                // Staying dead makes every later goal unreachable: nothing targets a corpse.
                if client.respawn().await.is_err() {
                    return false;
                }
            }
            Ok(event) => seen.fold(&event),
            // Quiet between ticks, which is ordinary.
            Err(ClientError::Timeout { .. }) => {}
            Err(_) => return false,
        }
    }
    true
}

/// Walk somewhere the way a player does, a stride at a time, so every section boundary is crossed
/// and the server gets the chance to stream what is on the other side of it.
///
/// A fixed pause per stride rather than a share of a budget: the point is to arrive and then spend
/// what is left waiting for the world, not to burn the whole allowance getting there.
async fn walk(client: &mut Client, seen: &mut Seen, to: (i32, i32), deadline: Instant) -> bool {
    let from = client.position();
    let (fx, fy) = (from.0 / 16.0, from.1 / 16.0);
    let strides = (((to.0 as f32 - fx).abs() + (to.1 as f32 - fy).abs()) / 40.0).ceil() as i32;
    let strides = strides.clamp(1, 200);
    for stride in 1..=strides {
        if Instant::now() >= deadline {
            return true;
        }
        let t = stride as f32 / strides as f32;
        let at = (
            (fx + (to.0 as f32 - fx) * t) * 16.0,
            (fy + (to.1 as f32 - fy) * t) * 16.0,
        );
        if !play(client, at, seen, Duration::from_millis(150), false).await {
            return false;
        }
    }
    true
}

/// Say something to the console and let the answer land.
async fn console(client: &mut Client, line: &str, seen: &mut Seen) -> bool {
    if client.say(line).await.is_err() {
        return false;
    }
    play(
        client,
        client.position(),
        seen,
        Duration::from_millis(400),
        false,
    )
    .await
}

/// The first solid tile in a column, as this client has been told about it.
///
/// `None` is two very different things, which is why the caller also counts what it holds: a
/// column it was never sent, and a column of nothing but sky.
fn ground(client: &Client, x: i32) -> Option<i32> {
    (0..client.world().height).find(|y| client.world().tile(x, *y).is_some_and(|t| t.is_active()))
}

/// How many tiles of a column this client actually holds.
fn known_in_column(client: &Client, x: i32) -> usize {
    (0..client.world().height)
        .filter(|y| client.world().tile(x, *y).is_some())
        .count()
}

/// The nearest floating island to a column, as somewhere to stand while waiting for the sky to
/// produce something. Only what this client actually holds counts.
fn island(client: &Client, near_x: i32) -> Option<(i32, i32)> {
    client
        .world()
        .known_tiles()
        .filter(|(_, y, tile)| (10..SKY_FLOOR).contains(y) && tile.is_active())
        .min_by_key(|(x, _, _)| (x - near_x).abs())
        .map(|(x, y, _)| (x, y))
}

/// Just the item ids, for a failure that has to say what dropped instead.
fn item_ids(seen: &Seen) -> Vec<i32> {
    let mut ids: Vec<i32> = seen.items.iter().map(|item| item.id).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn block_at(client: &Client, x: i32, y: i32) -> String {
    match client.world().tile(x, y) {
        None => "a tile this client was never sent".to_string(),
        Some(tile) if !tile.is_active() => "air".to_string(),
        Some(tile) => format!("block {}", tile.block),
    }
}

fn describe(item: ItemStack) -> String {
    if item.is_empty() {
        "nothing".to_string()
    } else {
        format!("item {} x{}", item.id, item.stack)
    }
}

/// Where the build site is, so both phases agree without either guessing.
#[derive(Clone, Copy)]
struct Site {
    chest: (i32, i32),
    door: (i32, i32),
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} {} {}",
            self.chest.0, self.chest.1, self.door.0, self.door.1
        )
    }
}

fn read_site(path: &str) -> Option<Site> {
    let text = fs::read_to_string(path).ok()?;
    let n: Vec<i32> = text
        .split_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect();
    match n[..] {
        [cx, cy, dx, dy] => Some(Site {
            chest: (cx, cy),
            door: (dx, dy),
        }),
        _ => None,
    }
}

// --- the phases -----------------------------------------------------------------------------------

#[tokio::main]
async fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let addr = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:7777".to_string())
        .parse()
        .expect("a socket address");
    let phase = args.next().unwrap_or_else(|| "build".to_string());
    let state = args.next().unwrap_or_else(|| "playbot.state".to_string());

    let mut client = match Client::join(addr, &format!("playbot-{phase}")).await {
        Ok(client) => client,
        Err(e) => {
            eprintln!("could not join {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Short, because every milestone loop polls: a long read timeout is a long stall.
    client.set_timeout(Duration::from_millis(100));
    // Casters are meant to hurt, and a bot that dies mid-fight teleports to spawn and takes every
    // later goal with it.
    client.set_life(400, 400).await.ok();

    println!(
        "joined \"{}\" {}x{}, spawn {:?}, phase {phase}\n",
        client.world().name,
        client.world().width,
        client.world().height,
        client.world().spawn,
    );

    let mut report = Report::default();
    let ok = match phase.as_str() {
        "build" => build(&mut client, &mut report, &state).await,
        "verify" => verify(&mut client, &mut report, &state).await,
        other => {
            eprintln!("unknown phase {other}: expected 'build' or 'verify'");
            return ExitCode::FAILURE;
        }
    };
    if !ok {
        println!("\n  the connection died part way through; the goals below it never ran");
    }

    println!();
    if phase == "verify" {
        println!("goals a person has that this client cannot drive at all:");
        for (goal, why) in Milestones::ABSENT {
            println!("  n/a   {goal}\n           {why}");
        }
        println!();
    }
    let missed = report.missed();
    if missed == 0 && ok {
        println!(
            "{} of {} goals reached",
            report.rows.len(),
            report.rows.len()
        );
        return ExitCode::SUCCESS;
    }
    println!("{missed} of {} goals missed:", report.rows.len());
    for row in report.rows.iter().filter(|r| !r.ok) {
        println!(
            "  {} (budget {}s, spent {}s)\n      {}",
            row.goal,
            row.budget.as_secs(),
            row.took.as_secs(),
            row.detail
        );
    }
    ExitCode::FAILURE
}

/// Everything that happens before the world is written to disk.
async fn build(client: &mut Client, report: &mut Report, state: &str) -> bool {
    let (spawn_x, spawn_y) = client.world().spawn;
    let (spawn_x, spawn_y) = (i32::from(spawn_x), i32::from(spawn_y));

    // --- get a long way from spawn, and keep receiving world ------------------------------------
    let budget = Duration::from_secs(60);
    let started = Instant::now();
    let far_x = (spawn_x + FAR).clamp(60, client.world().width - 60);
    let mut seen = Seen::default();
    if !walk(client, &mut seen, (far_x, spawn_y), started + budget).await {
        return false;
    }
    // Give whatever is still queued a chance to land before judging it.
    while started.elapsed() < budget && ground(client, far_x).is_none() {
        if !play(
            client,
            (far_x as f32 * 16.0, spawn_y as f32 * 16.0),
            &mut seen,
            Duration::from_secs(2),
            false,
        )
        .await
        {
            return false;
        }
    }
    let held = known_in_column(client, far_x);
    let floor = ground(client, far_x);
    report.record(
        "walk 1200 tiles from spawn and still have world under your feet",
        budget,
        started,
        floor.is_some(),
        match floor {
            Some(y) => format!(
                "walked from tile ({spawn_x}, {spawn_y}) to ({far_x}, {spawn_y}); the column \
                 arrived ({held} tiles held) and the ground is at y={y}; {} sections loaded",
                client.world().loaded_sections()
            ),
            None if held == 0 => format!(
                "expected the world around tile ({far_x}, {spawn_y}) to arrive while walking \
                 there; not one tile of that column was ever sent ({} sections loaded, none \
                 covering it), so a player walking there sees sky",
                client.world().loaded_sections()
            ),
            None => format!(
                "expected solid ground somewhere in the column at x={far_x}; all {held} tiles \
                 received for it are air, so a player walking there sees sky forever"
            ),
        },
    );
    let Some(floor) = floor else {
        // Nothing further can be built out here, and building at spawn would test a different
        // thing than the one that broke.
        return true;
    };

    // Clear a pocket to build in: the chest wants a clear 2x2 and the door a clear 1x3 with room
    // to swing into.
    let site = Site {
        chest: (far_x, floor - 2),
        door: (far_x + 2, floor - 3),
    };
    for x in far_x..far_x + 4 {
        for y in floor - 4..floor {
            if client.world().tile(x, y).is_some_and(|t| t.is_active()) {
                client.break_tile(x as i16, y as i16).await.ok();
            }
        }
    }
    // Stand on the *local* surface, not on the build column's. Reporting a position that is
    // actually inside the hillside costs line of sight, and a boss that cannot see you spends the
    // whole fight teleporting instead of being hit.
    let stand_x = far_x - 6;
    let stand_y = ground(client, stand_x).unwrap_or(floor) - 3;
    let stand = (stand_x as f32 * 16.0, stand_y as f32 * 16.0);
    let mut seen = Seen::default();
    if !play(client, stand, &mut seen, Duration::from_secs(2), false).await {
        return false;
    }

    // --- put a chest there --------------------------------------------------------------------
    //
    // Packet 34, because that is the packet a real client sends: `Main.tileContainer` keeps
    // containers out of the ordinary placement packet entirely. It is also the one placement the
    // server echoes back to the placer, with the chest's id on it, which is what makes building
    // this far from spawn checkable with one client.
    let budget = Duration::from_secs(20);
    let started = Instant::now();
    let mut w = PacketWriter::new(id::CHEST_UPDATES);
    // Action 0 places a chest; the cursor sits on the object's lower-left cell, one row below its
    // corner.
    w.u8(0)
        .i16(site.chest.0 as i16)
        .i16(site.chest.1 as i16 + 1)
        .i16(0)
        .i16(-1);
    match w.finish() {
        Ok(frame) => {
            if client.send(&frame).await.is_err() {
                return false;
            }
        }
        Err(e) => {
            report.record(
                "place a chest 1200 tiles from spawn",
                budget,
                started,
                false,
                format!("could not encode the placement: {e}"),
            );
            return true;
        }
    }
    let mut seen = Seen::default();
    while started.elapsed() < budget && seen.chest_update.is_none() {
        if !play(client, stand, &mut seen, Duration::from_secs(1), false).await {
            return false;
        }
    }
    let chest_id = seen
        .chest_update
        .filter(|(_, _, _, id)| *id >= 0)
        .map(|u| u.3);
    report.record(
        "place a chest 1200 tiles from spawn",
        budget,
        started,
        chest_id.is_some(),
        match seen.chest_update {
            Some((_, _, _, id)) if id >= 0 => {
                format!("chest {id} stands at tile {:?}", site.chest)
            }
            Some((_, x, y, _)) => format!(
                "expected a chest id back for a placement at tile {:?}; the server refused it \
                 (id -1 at ({x}, {y})), which means the 2x2 footprint was not clear",
                site.chest
            ),
            None => format!(
                "expected the server to answer a chest placement at tile {:?} within {}s; it \
                 sent nothing back at all, so nothing was placed",
                site.chest,
                budget.as_secs()
            ),
        },
    );

    // --- put something in it, and read it back off the server -----------------------------------
    if let Some(chest_id) = chest_id {
        let budget = Duration::from_secs(20);
        let started = Instant::now();
        let mut seen = Seen::default();
        if client
            .open_chest(site.chest.0 as i16, site.chest.1 as i16)
            .await
            .is_err()
        {
            return false;
        }
        if !play(client, stand, &mut seen, Duration::from_secs(2), false).await {
            return false;
        }
        let stored = ItemStack::new(GEL, 5, 0);
        let put = SyncChestItem {
            chest: chest_id,
            slot: 0,
            item: stored,
        };
        if let Ok(frame) = put.encode()
            && client.send(&frame).await.is_err()
        {
            return false;
        }
        // Close it and open it again, so what comes back is the server's copy rather than an echo
        // of what was just sent.
        if let Ok(frame) = (SyncPlayerChest {
            chest: -1,
            x: 0,
            y: 0,
            name: None,
        })
        .encode()
            && client.send(&frame).await.is_err()
        {
            return false;
        }
        let mut seen = Seen::default();
        if client
            .open_chest(site.chest.0 as i16, site.chest.1 as i16)
            .await
            .is_err()
        {
            return false;
        }
        while started.elapsed() < budget && seen.chest_slots.is_empty() {
            if !play(client, stand, &mut seen, Duration::from_secs(1), false).await {
                return false;
            }
        }
        let back = seen
            .chest_slots
            .get(&0)
            .copied()
            .unwrap_or(ItemStack::EMPTY);
        report.record(
            "put an item in the chest and have the server keep it",
            budget,
            started,
            back == stored,
            format!(
                "put {} in slot 0 of chest {chest_id}, closed it and opened it again; the server \
                 sent back {} ({} slots in all)",
                describe(stored),
                describe(back),
                seen.chest_slots.len()
            ),
        );
    }

    // --- a door, whose only honest test is on the far side of the save --------------------------
    client
        .place_object(site.door.0 as i16, site.door.1 as i16, DOOR_SHUT, 0)
        .await
        .ok();
    let mut seen = Seen::default();
    if !play(client, stand, &mut seen, Duration::from_secs(2), false).await {
        return false;
    }
    // Direction 1: the door swings into the column to its right, which the pocket left clear.
    client
        .toggle_door(0, site.door.0 as i16, site.door.1 as i16, 1)
        .await
        .ok();
    if !play(client, stand, &mut seen, Duration::from_secs(2), false).await {
        return false;
    }

    // --- kill something ordinary and get paid for it ---------------------------------------------
    let budget = Duration::from_secs(30);
    let started = Instant::now();
    let mut seen = Seen::default();
    if !console(client, "/butcher", &mut seen).await {
        return false;
    }
    let mut seen = Seen::default();
    for _ in 0..4 {
        if !console(client, "/spawn BlueSlime", &mut seen).await {
            return false;
        }
    }
    if !play(client, stand, &mut seen, budget, true).await {
        return false;
    }
    let died = seen.types.get(&BLUE_SLIME) == Some(&0);
    report.record(
        "kill an ordinary enemy and have it leave something behind",
        budget,
        started,
        died && !seen.items.is_empty(),
        format!(
            "spawned 4 Blue Slimes and swung until they stopped moving: {}, {} item(s) dropped \
             (present: {})",
            if died {
                "one died"
            } else {
                "not one of them ever reached zero life"
            },
            seen.items.len(),
            seen.roster()
        ),
    );
    let gel = seen.items.iter().filter(|i| i.id == GEL).count();
    report.record(
        "come away from a slime with Gel in hand",
        budget,
        started,
        gel > 0,
        if gel > 0 {
            format!("{gel} drop(s) of Gel (item {GEL}) from the slimes")
        } else {
            format!(
                "expected item {GEL} (Gel) among a slime's drops; no Gel means no torch and no \
                 Slime Crown. Got items {:?}",
                item_ids(&seen)
            )
        },
    );

    // --- and Bone, which comes from one family and nowhere else -----------------------------------
    let budget = Duration::from_secs(60);
    let started = Instant::now();
    let mut seen = Seen::default();
    if !console(client, "/butcher", &mut seen).await {
        return false;
    }
    let mut seen = Seen::default();
    let mut killed: u32 = 0;
    while started.elapsed() < budget
        && killed < BONE_KILLS
        && !seen.items.iter().any(|i| i.id == BONE)
    {
        let mut round = Seen::default();
        for _ in 0..4 {
            if !console(client, &format!("/spawn {ANGRY_BONES}"), &mut round).await {
                return false;
            }
        }
        if !play(client, stand, &mut round, Duration::from_secs(8), true).await {
            return false;
        }
        killed += round.deaths.get(&ANGRY_BONES_TYPE).copied().unwrap_or(0);
        seen.items.extend(round.items.iter().copied());
        for (kind, low) in round.types {
            let entry = seen.types.entry(kind).or_insert(i32::MAX);
            *entry = (*entry).min(low);
        }
    }
    let bone = seen.items.iter().filter(|i| i.id == BONE).count();
    report.record(
        "come away from the Angry Bones family with Bone in hand",
        budget,
        started,
        bone > 0,
        if bone > 0 {
            format!("{bone} drop(s) of Bone (item {BONE}) over {killed} {ANGRY_BONES} kills")
        } else {
            format!(
                "expected item {BONE} (Bone) from {ANGRY_BONES}, the only family that drops it \
                 (one kill in three), over {killed} kills; no Bone means every Bone recipe is \
                 uncraftable. Got items {:?}",
                item_ids(&seen)
            )
        },
    );

    // --- a caster's orb has to actually go somewhere ------------------------------------------------
    let budget = Duration::from_secs(40);
    let started = Instant::now();
    let mut seen = Seen::default();
    if !console(client, "/butcher", &mut seen).await {
        return false;
    }
    let mut seen = Seen::default();
    for _ in 0..3 {
        if !console(client, &format!("/spawn {CASTER}"), &mut seen).await {
            return false;
        }
    }
    // Not swinging: the caster has to live long enough to cast.
    if !play(client, stand, &mut seen, budget, false).await {
        return false;
    }
    // The orb is an NPC, not a projectile. Terraria implements a caster's shot as an NPC with one
    // hit point and no gravity (`ai/orb.rs`, style 9, `NPC.cs:21459-21600`), which is exactly why
    // "did a projectile appear" never noticed that these were harmless. Both are watched anyway,
    // in case one is ever moved onto the projectile subsystem.
    let mut shots: Vec<Flight> = seen
        .npcs
        .values()
        .filter(|f| f.kind == CASTER_ORB)
        .copied()
        .collect();
    shots.extend(seen.flights.values().copied());
    let flew = shots
        .iter()
        .find(|f| f.samples > 1 && f.travelled() > MUZZLE);
    let stuck: Vec<String> = shots
        .iter()
        .map(|f| {
            format!(
                "type {} seen {}x, {:.0}px from where it appeared",
                f.kind,
                f.samples,
                f.travelled()
            )
        })
        .collect();
    report.record(
        "provoke a pre-hardmode caster and watch its shot travel",
        budget,
        started,
        flew.is_some(),
        match flew {
            Some(f) => format!(
                "the {CASTER}'s orb (type {}) travelled {:.0}px over {} updates",
                f.kind,
                f.travelled(),
                f.samples
            ),
            None if stuck.is_empty() => format!(
                "expected {CASTER} to throw a Chaos Ball (npc {CASTER_ORB}) within {}s and for it \
                 to move more than {MUZZLE:.0}px; nothing was ever thrown at all (present: {})",
                budget.as_secs(),
                seen.roster()
            ),
            None => format!(
                "expected the {CASTER}'s shot to travel more than {MUZZLE:.0}px; every one hung \
                 at the muzzle: {}",
                stuck.join("; ")
            ),
        },
    );

    // --- summon a boss and finish it ------------------------------------------------------------
    let budget = Duration::from_secs(90);
    let started = Instant::now();
    let mut seen = Seen::default();
    if !console(client, "/butcher", &mut seen).await {
        return false;
    }
    let mut seen = Seen::default();
    if client.summon(KING_SLIME as i16).await.is_err() {
        return false;
    }
    while started.elapsed() < budget && seen.types.get(&KING_SLIME).is_none_or(|low| *low > 0) {
        if !play(client, stand, &mut seen, Duration::from_secs(2), true).await {
            return false;
        }
    }
    let low = seen.types.get(&KING_SLIME).copied();
    // How often it was heard from, and how far off it was the last time: "never took a scratch"
    // and "wandered out of earshot" are different bugs that both read as "did not die".
    let boss = seen.npcs.values().find(|f| f.kind == KING_SLIME).copied();
    report.record(
        "summon King Slime and kill it",
        budget,
        started,
        low == Some(0),
        match low {
            Some(0) => format!("King Slime died, leaving {} item(s)", seen.items.len()),
            Some(life) => format!(
                "expected King Slime ({KING_SLIME}) to reach zero life within {}s; its life never \
                 fell below {life}. It was synced {} time(s) and last seen {:.0} tiles away; it \
                 sheds a slime every 5% of its health and {} were seen",
                budget.as_secs(),
                boss.map_or(0, |f| f.samples),
                boss.map_or(f32::NAN, |f| {
                    ((f.last.0 - stand.0).powi(2) + (f.last.1 - stand.1).powi(2)).sqrt() / 16.0
                }),
                usize::from(seen.types.contains_key(&BLUE_SLIME)),
            ),
            None => format!(
                "expected the summon of King Slime ({KING_SLIME}) to put one in the world within \
                 {}s; it never appeared (saw {})",
                budget.as_secs(),
                seen.roster()
            ),
        },
    );

    // --- go up, and see whether the sky has anything living in it ---------------------------------
    let budget = Duration::from_secs(90);
    let started = Instant::now();
    let mut seen = Seen::default();
    if !console(client, "/butcher", &mut seen).await {
        return false;
    }
    if !console(client, "/time day", &mut seen).await {
        return false;
    }
    // Stand on a floating island if this client has been told about one. Nothing spawns in open
    // air (every spawn needs a tile to stand on), so waiting in empty sky would report "no sky
    // mob" for a world that simply had no perch, which is a different bug and not this one.
    let mut seen = Seen::default();
    if !walk(client, &mut seen, (far_x, SKY_Y), started + budget).await {
        return false;
    }
    let perch = island(client, far_x);
    let sky = perch.unwrap_or((far_x, SKY_Y));
    let at_sky = (sky.0 as f32 * 16.0, (sky.1 - 3) as f32 * 16.0);
    let mut seen = Seen::default();
    if !walk(client, &mut seen, (sky.0, sky.1 - 3), started + budget).await {
        return false;
    }
    while started.elapsed() < budget && !seen.types.keys().any(|t| SKY_MOBS.contains(t)) {
        if !play(client, at_sky, &mut seen, Duration::from_secs(3), false).await {
            return false;
        }
    }
    let met = seen.types.keys().any(|t| SKY_MOBS.contains(t));
    report.record(
        "meet a Harpy or a Wyvern in the sky",
        budget,
        started,
        met,
        if met {
            format!("met one at tile {sky:?}: {}", seen.roster())
        } else {
            format!(
                "expected a Harpy (48) or a Wyvern (87) to spawn while standing {} for {}s; what \
                 arrived instead was {}, which is the ordinary *surface* roster: the sky is not \
                 drawn from as a place of its own, so neither bird can be met anywhere",
                match perch {
                    Some(at) => format!("on the sky island at tile {at:?}"),
                    None => format!(
                        "at tile {sky:?} (no sky island in the loaded sections to perch on)"
                    ),
                },
                started.elapsed().as_secs(),
                seen.roster()
            )
        },
    );

    // --- write it all to disk, and leave the site behind for the second half ----------------------
    let mut seen = Seen::default();
    if !console(client, "/butcher", &mut seen).await {
        return false;
    }
    if client.say("/save").await.is_err() {
        return false;
    }
    let mut seen = Seen::default();
    let saved_by = Instant::now() + Duration::from_secs(60);
    while Instant::now() < saved_by && !seen.chat.iter().any(|t| t.contains("World saved")) {
        if !play(client, stand, &mut seen, Duration::from_secs(2), false).await {
            return false;
        }
    }
    if let Err(e) = fs::write(state, site.to_string()) {
        eprintln!("could not write the build site to {state}: {e}");
    }
    println!("\n  built at {site}; server says: {:?}", seen.chat);
    true
}

/// Everything that has to still be true after the world has been round through the disk.
async fn verify(client: &mut Client, report: &mut Report, state: &str) -> bool {
    let Some(site) = read_site(state) else {
        eprintln!("no build site in {state}: the build phase never got far enough to leave one");
        return false;
    };

    let budget = Duration::from_secs(40);
    let started = Instant::now();
    let mut seen = Seen::default();
    if !walk(
        client,
        &mut seen,
        (site.chest.0 - 2, site.chest.1),
        started + Duration::from_secs(30),
    )
    .await
    {
        return false;
    }
    let stand = (
        (site.chest.0 as f32 - 2.0) * 16.0,
        site.chest.1 as f32 * 16.0,
    );

    // --- is the chest still there, with what was put in it? --------------------------------------
    let mut seen = Seen::default();
    if client
        .open_chest(site.chest.0 as i16, site.chest.1 as i16)
        .await
        .is_err()
    {
        return false;
    }
    while started.elapsed() < budget && seen.chest_open.is_none() {
        if !play(client, stand, &mut seen, Duration::from_secs(1), false).await {
            return false;
        }
    }
    let stored = ItemStack::new(GEL, 5, 0);
    let back = seen
        .chest_slots
        .get(&0)
        .copied()
        .unwrap_or(ItemStack::EMPTY);
    report.record(
        "find the chest, and the item in it, after the world has been saved and reloaded",
        budget,
        started,
        seen.chest_open.is_some() && back == stored,
        match seen.chest_open {
            None => format!(
                "expected a chest at tile {:?} holding {}; the server would not open one there \
                 at all and the tile is {}",
                site.chest,
                describe(stored),
                block_at(client, site.chest.0, site.chest.1)
            ),
            Some((chest, x, y)) => format!(
                "chest {chest} is at ({x}, {y}) and the tile is {}; slot 0 holds {}, expected {}",
                block_at(client, site.chest.0, site.chest.1),
                describe(back),
                describe(stored)
            ),
        },
    );

    // --- is the door still open? -------------------------------------------------------------------
    let budget = Duration::from_secs(20);
    let started = Instant::now();
    let mut seen = Seen::default();
    if !play(client, stand, &mut seen, Duration::from_secs(2), false).await {
        return false;
    }
    // The open form is two tiles wide and can stand one column either side of where the shut door
    // was, so look across the whole swing.
    let mut shut = 0;
    let mut open = 0;
    let mut other = Vec::new();
    for x in site.door.0 - 1..=site.door.0 + 1 {
        for y in site.door.1..site.door.1 + 3 {
            match client.world().tile(x, y) {
                Some(tile) if tile.is_active() && tile.block == DOOR_OPEN => open += 1,
                Some(tile) if tile.is_active() && tile.block == DOOR_SHUT => shut += 1,
                Some(tile) if tile.is_active() => other.push(tile.block),
                _ => {}
            }
        }
    }
    report.record(
        "find the door still open after the world has been saved and reloaded",
        budget,
        started,
        open > 0,
        if open > 0 {
            format!(
                "{open} open-door tiles ({DOOR_OPEN}) stand at {:?}",
                site.door
            )
        } else if shut > 0 {
            format!(
                "expected open-door tiles ({DOOR_OPEN}) around {:?}; found {shut} shut-door \
                 tiles ({DOOR_SHUT}) instead: the door opened on the wire and the world was \
                 saved shut",
                site.door
            )
        } else {
            format!(
                "expected a door around {:?}; there is no door tile there at all ({} at the \
                 anchor, other blocks nearby: {other:?}): the placement did not survive",
                site.door,
                block_at(client, site.door.0, site.door.1)
            )
        },
    );

    // The chest's own tile, checked separately, because "no chest record" and "no chest tile" are
    // different bugs with the same symptom.
    let budget = Duration::from_secs(1);
    let started = Instant::now();
    let tile = client
        .world()
        .tile(site.chest.0, site.chest.1)
        .filter(|t| t.is_active());
    report.record(
        "find the chest's own tiles after the reload, not just its contents",
        budget,
        started,
        tile.is_some_and(|t| t.block == CHEST_TILE),
        format!(
            "expected block {CHEST_TILE} at tile {:?}; found {}",
            site.chest,
            block_at(client, site.chest.0, site.chest.1)
        ),
    );

    unlock_a_dungeon_door(client, report).await;
    true
}

/// Find a locked dungeon door, unlock it with a Golden Key, and check the frames really moved.
///
/// This goal used to sit in `ABSENT` with the note "worldgen builds no dungeon", which was wrong
/// twice: a generated world holds thousands of dungeon bricks and about a hundred doors, and every
/// one of those doors is locked. The note also had the frame axis wrong - vanilla identifies a
/// locked door by `frameY == 594`, not `frameX` - so anyone re-checking it by the stated rule would
/// have found nothing and agreed. The whole path (`LockAction::UnlockDoor` on the wire,
/// `WorldGen.UnlockDoor`'s +54 shift on the server) has been implemented the entire time and was
/// simply never driven.
async fn unlock_a_dungeon_door(client: &mut Client, report: &mut Report) {
    let budget = Duration::from_secs(60);
    let started = Instant::now();

    // The dungeon is nowhere near spawn, and a client only knows the sections it has been sent, so
    // this asks for them. The first version of this goal scanned only what had already arrived and
    // reported "no locked door" in zero seconds, which is a different way of not testing the thing.
    let (w, h) = (client.world().width, client.world().height);
    let (sections_x, sections_y) = (w / 200 + 1, h / 150 + 1);
    let mut found: Option<(i32, i32)> = None;
    'sweep: for sx in 0..sections_x {
        for sy in 0..sections_y {
            if started.elapsed() > budget {
                break 'sweep;
            }
            if !client.world().has_section(sx, sy)
                && client.request_section(sx as u16, sy as u16).await.is_err()
            {
                break 'sweep;
            }
            // Let the section land.
            for _ in 0..40 {
                match client.next_event().await {
                    Ok(_) => {}
                    Err(ClientError::Timeout { .. }) => break,
                    Err(_) => break 'sweep,
                }
            }
            if let Some((x, y, _)) = client
                .world()
                .known_tiles()
                .find(|(_, _, t)| t.is_active() && t.block == DOOR_SHUT && t.frame_y == 594)
            {
                found = Some((x, y));
                break 'sweep;
            }
        }
    }

    let Some((x, y)) = found else {
        report.record(
            "unlock a dungeon door with a Golden Key",
            budget,
            started,
            false,
            {
                // Report what was actually seen. A bare "not found" is what let this goal look
                // finished three times running.
                let mut doors = 0usize;
                let mut frames: Vec<i16> = Vec::new();
                let mut bricks = 0usize;
                for (_, _, t) in client.world().known_tiles() {
                    if !t.is_active() {
                        continue;
                    }
                    if t.block == DOOR_SHUT {
                        doors += 1;
                        if frames.len() < 8 {
                            frames.push(t.frame_y);
                        }
                    }
                    if matches!(t.block, 41 | 43 | 44) {
                        bricks += 1;
                    }
                }
                format!(
                    "no locked door: {} sections known, {bricks} dungeon brick(s), {doors} \
                     door tile(s), first frameY values {frames:?} (a locked door's top row is 594)",
                    client.world().loaded_sections()
                )
            },
        );
        return;
    };

    let before = client.world().tile(x, y).map(|t| t.frame_y);
    let _ = client.unlock_door(x as i16, y as i16).await;
    for _ in 0..200 {
        match client.next_event().await {
            Ok(_) => {}
            Err(ClientError::Timeout { .. }) => break,
            Err(_) => break,
        }
    }
    let after = client.world().tile(x, y).map(|t| t.frame_y);

    // Vanilla shifts the door's three rows by +54 when it opens the lock.
    let moved = matches!((before, after), (Some(b), Some(a)) if a == b + 54);
    report.record(
        "unlock a dungeon door with a Golden Key",
        budget,
        started,
        moved,
        if moved {
            format!("the door at ({x}, {y}) went from frameY {before:?} to {after:?}, a +54 shift")
        } else {
            format!(
                "the door at ({x}, {y}) was framed {before:?} and is now {after:?}; \
                 `WorldGen.UnlockDoor` shifts it by +54"
            )
        },
    );
}
