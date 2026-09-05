//! The single-writer game task.
//!
//! One task owns the world and the player table, so there are no locks on the hot path and packet
//! ordering is deterministic. Connections talk to it over an `mpsc` of [`ServerEvent`]; it talks
//! back through each player's outbound queue.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use rand::{Rng, SeedableRng, rngs::SmallRng};
use std::path::Path;
use terrustia_proto::{
    ItemStack, NetworkText, Tile, TileFlags, id,
    items::{ItemOwner, SyncItem, decode_item_despawn},
    net_module::{self, IncomingChat},
    npc::{DamageNpc, SyncNpc, damage_ack, damage_taken},
    objects::{
        self, DoorToggle, RequestChestOpen, RequestSign, SignText, SyncChestItem, SyncPlayerChest,
        SyncPlayerChestIndex,
    },
    packets::{
        self, HealPlayer, Hello, PlayerControls, PlayerHealth, PlayerMana, PlayerSpawn,
        SpawnTileData, TileAction, TileManipulation,
    },
    reader::PacketReader,
    section::encode_section_packet,
    square::TileSquare,
    tile_drops::tile_drop,
    tile_sets::frame_important,
};
use tokio::sync::{mpsc, oneshot};

use tracing::{debug, error, info, warn};

use crate::{
    config::Config,
    game::player::{ConnState, Player},
    game::{
        event::{Invasion, InvasionState},
        housing,
        npc::{NpcStore, TileView},
        npc_ai::{self, Target},
        spawn,
    },
    net::Frame,
    world::{
        Sign, World,
        items::{self, ItemStore},
        wld_save,
        world::{DAY_LENGTH, NIGHT_LENGTH},
    },
};

mod console;
// Test-only: the tab-completion list in `crate::console` is checked against it. Nothing in a
// production build needs it, and `warnings = "deny"` would reject an unused re-export.
#[cfg(test)]
pub(crate) use console::console_help_commands;
mod dispatch;
mod panel;
mod systems;
mod tick;

use tick::TickCost;

// The panel's snapshot types are defined beside the code that fills them in, and re-exported here
// so `crate::panel` keeps addressing them as `game::server::PanelStatus` and friends.
pub use panel::{
    PanelAccountInfo, PanelAccounts, PanelAuthLookup, PanelBackupEntry, PanelBackups,
    PanelConfigSnapshot, PanelGroupInfo, PanelMetrics, PanelPlayer, PanelStatus, PanelWhitelist,
    PanelWorldTiles, TileColor,
};

/// Signs hold a page of text at most; anything longer is a client that is not playing fair.
const MAX_SIGN_TEXT: usize = 1000;

/// How many world saves must fail in a row before the players are told, and not only the log.
///
/// Three. One failure is a blip - a backup tool holding the file open for a moment, a network mount
/// hiccuping - and interrupting everybody's game for it would teach them to ignore the message. At
/// the default five-minute autosave, three is fifteen minutes of play at risk: long enough that
/// telling people is warranted, short enough that they can still do something about it (log out and
/// let the operator sort it out, rather than build for another hour first). The console log warns
/// on the *first* failure regardless; this constant only governs the in-game broadcast.
pub const SAVE_FAILURES_BEFORE_ALARM: u32 = 3;

/// A save this slow is worth a line in the log even when it succeeded and nobody asked for it.
///
/// One second. A healthy autosave on a 4200x1200 world is around 120 ms, so a second is roughly
/// eight times that: comfortably past ordinary variance, and the point at which the cause is
/// something an operator would want to know about (a contended disk, a network mount, a filesystem
/// filling up) rather than noise. Below it a successful autosave logs at debug, because a server
/// left running produces one of these every five minutes forever and twenty identical lines saying
/// nothing went wrong is how a log stops being read at all.
pub const SLOW_SAVE_MS: u64 = 1_000;

/// Sections copied into the snapshot buffer per tick while a save waits to fire.
///
/// A section is a 200x150 rectangle of a row-major array, so copying one is 150 short strided
/// memcpys rather than one long one, and the pages it reads have not been touched since the last
/// save. In a benchmark loop, where every run finds the previous run's pages warm, that is about
/// 16 us a section (`examples/snapcost`). On a real server it is not: two 185-second runs on the
/// owner's own 4200x1200 world, autosaving every 20 s with nobody connected, logged anything from
/// 213 us for 5 sections to 3,548 us for 11. **Call it 250 to 700 us a section cold**, and treat
/// the warm figure as the one that misleads.
///
/// Eight was tried first, off the warm figure, and left a 2.6 ms drain tick. Three measured a
/// worst drain tick of 2,218 us against a 16,666 us budget, with the worst *save* tick falling
/// from 3,548 us to 757 us and `sections_copied` reaching zero on every one of nine saves. It
/// clears a whole 168-section world in 56 ticks, well inside [`SNAPSHOT_DRAIN_DEADLINE`].
///
// ponytail: a fixed section count against a per-section cost that varies 17x. If a drain tick ever
// needs to be tighter than this, the upgrade is a microsecond budget in the shape of
// SECTION_STREAM_BUDGET below rather than a smaller number here. Not done speculatively: 2.2 ms is
// 13% of a frame, and the tile-granular tracking in TODO.md would leave this almost nothing to do.
const SNAPSHOT_DRAIN_PER_TICK: usize = 3;

/// How long a pending save waits for the drain before copying the remainder in one go.
///
/// Ten seconds. If the world dirties faster than the drain clears it the save would otherwise be
/// postponed indefinitely, which risks more than a stutter does; this bounds it back to today's
/// spike under load that is already pathological. An idle world marks about 0.009 sections a tick.
const SNAPSHOT_DRAIN_DEADLINE: u64 = 600;

/// How far a player can be from an item and still have it reserved for them, in pixels.
///
/// Generous on purpose: the reservation only grants the right to pick the item up, and a client
/// needs to hold it before its own grab animation begins.
const ITEM_GRAB_RANGE: f32 = 400.0;

/// How much of a tick a single player's queued section stream may spend before
/// `drain_section_streams` picks it back up next tick, rather than draining it in one shot.
///
/// Set from the worst single-section encode this project has ever measured (`examples/
/// sectioncost.rs`: ~2,976µs on an 8400×2400 world) with headroom, so this phase's own share of a
/// tick stays bounded and predictable rather than reproducing the exact synchronous-burst problem
/// it exists to fix, just moved from one packet handler into the tick loop. At least one section
/// still goes out every tick a queue is non-empty, even past the budget, so a stream always makes
/// forward progress.
const SECTION_STREAM_BUDGET: Duration = Duration::from_micros(4_000);

/// Spawn slots a boss fight may fill with summoned minions.
const MAX_MINION_SLOTS: f32 = 40.0;

/// The Guide, who arrives as soon as there is a house.
const GUIDE: u16 = 22;

/// Ticks between housing scans. The room check is a flood fill, so it is not run every tick.
const HOUSING_SCAN_INTERVAL: u64 = 60 * 5;

/// Pixels of frame each pylon style occupies: three tiles wide, eighteen pixels each.
const PYLON_FRAME_WIDTH: i16 = 54;

/// How far from a pylon a player may stand and still travel, in tiles.
///
/// `TileReachCheckSettings.Pylons` overrides the usual reach with sixty in each direction — far
/// more than an arm's length, so that standing anywhere in the room counts.
const PYLON_REACH: f32 = 60.0;

/// How many townsfolk have to live near a pylon before it will carry anybody.
const PYLON_RESIDENTS_NEEDED: usize = 2;

/// Half the box a pylon looks in for those residents.
///
/// `SceneMetrics.ZoneScanSize` works out to 169 x 124 tiles — a screen at the game's assumed
/// 1920x1200, plus twenty-five tiles of padding on every side — and the box is centred on the
/// pylon. 169 is odd so it is symmetric at plus-or-minus 84; 124 is even, and
/// `Utils.CenteredRectangle` plus `Rectangle.Contains` make that -62..=61 rather than -62..=62.
const PYLON_SCAN_HALF_WIDTH: i32 = 84;
const PYLON_SCAN_HALF_HEIGHT_UP: i32 = 62;
const PYLON_SCAN_HALF_HEIGHT_DOWN: i32 = 61;

/// How far a resident may stray from its house and still count towards its pylon, in tiles.
/// `TeleportPylonsSystem.cs:237`.
const PYLON_RESIDENT_HOME_RANGE: f32 = 100.0;

/// The fraction of the surface line below which a Beach pylon stops being a Beach pylon.
///
/// `TeleportPylonsSystem.cs:284`: `Y <= worldSurface && Y > worldSurface * 0.35`. Without the
/// lower bound a pylon parked in the sky, hundreds of tiles above any water, still read as a
/// coastal one.
///
/// The decompiler prints that constant as `0.3499999940395355`, which is only the `double` the
/// game's `0.35f` widens to; as an `f32` this is the same bit pattern.
const BEACH_PYLON_SKY_LIMIT: f32 = 0.35;

/// How close to the bottom of a remix world a Glowing Mushroom pylon stops working, in rows.
///
/// `TeleportPylonsSystem.cs:295-297`: `if (Main.remixWorld && Y >= Main.maxTilesY - 200) return
/// false`. A remix world's surface is at the bottom, so its underworld-side mushroom fields are
/// not what that network is for.
const REMIX_MUSHROOM_PYLON_FLOOR: i32 = 200;

/// `NPC.cs:785`, `spawnRate = 20` during an invasion: a one-in-twenty roll per player per tick,
/// rather than a fixed cadence. An invasion arrives steadily rather than all at once, but it
/// arrives at the game's own pace.
const INVASION_SPAWN_RATE: u32 = 20;

/// The chest tile. Placing one needs a container behind it, not just tiles.
const CHEST_BLOCK: u16 = 21;

/// Ticks between NPC position broadcasts. Clients interpolate between them.
const NPC_SYNC_INTERVAL: u64 = 6;

/// Which section a world position falls in.
fn section_of(at: (f32, f32)) -> (i32, i32) {
    (
        (at.0 / crate::game::npc::TILE) as i32 / terrustia_proto::section::SECTION_WIDTH,
        (at.1 / crate::game::npc::TILE) as i32 / terrustia_proto::section::SECTION_HEIGHT,
    )
}

/// Whether somebody standing here has that section loaded.
fn near_section(standing: (f32, f32), section: (i32, i32)) -> bool {
    let theirs = section_of(standing);
    (theirs.0 - section.0).abs() <= SECTION_REACH && (theirs.1 - section.1).abs() <= SECTION_REACH
}

/// How far around a meteor's centre to push at clients, which is comfortably past the crater.
const METEOR_REACH: i32 = 40;

/// How often to check that the Old Man is still at his post.
const OLD_MAN_CHECK_INTERVAL: u64 = 60 * 5;
/// How near a player has to be, in tiles, before it is worth putting him back.
const OLD_MAN_NOTICE: f32 = 250.0;

/// How many sections either way still count as near enough to be told about.
///
/// One section is 200 by 150 tiles, so this reaches 600 by 450 — comfortably past what anybody
/// can see, which is the point: a client should be told about something before it comes on
/// screen, not as it arrives.
const SECTION_REACH: i32 = 1;

/// How many NPC syncs each of the two paths has sent, for the tick-window log.
///
/// Counters rather than a guess. The two paths look interchangeable from the code and are not: a
/// measurement here showed 251 full syncs against 5 streamed ones, which said immediately that the
/// rate limiter was doing all the work and the real problem was upstream in how often an NPC was
/// being marked as having changed. Relaxed atomics on a path that already does a hash lookup.
pub static SYNC_FULL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static SYNC_STREAM: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How much a round of proximity streaming has to accumulate before an update is sent.
///
/// The weights below are chosen against it: eight means a player standing on top of something gets
/// an update every streaming round, and one means they get one every eighth round.
const STREAM_THRESHOLD: u8 = 8;

/// How heavily one player's nearness counts towards streaming an NPC to them.
///
/// From `NPC.StreamUpdatesToNearbyPlayers`: full weight within about fifteen tiles, then halving
/// outward, and nothing at all past ninety-odd tiles — which is roughly a screen and a half.
fn stream_weight(distance: f32) -> u8 {
    match distance {
        d if d < 250.0 => 8,
        d if d < 500.0 => 4,
        d if d < 1000.0 => 2,
        d if d < 1500.0 => 1,
        _ => 0,
    }
}

/// How many times in a row an NPC's state may be withheld from a player who is not near it.
///
/// Withholding it entirely would leave a distant NPC frozen where a client last saw it; the game
/// lets four go by and then sends one anyway.
const MAX_NPC_SYNC_SKIPS: u8 = 4;

/// How many times in a row a distant player's own movement may be withheld from another player.
///
/// Deliberately far larger than [`MAX_NPC_SYNC_SKIPS`], because the two are not the same shape of
/// problem. An NPC syncs once every [`NPC_SYNC_INTERVAL`] ticks, so letting four go by already
/// spaces it out; player movement arrives *every tick*, so a budget of four would still relay one
/// in five and leave the fan-out quadratic with a constant factor. At sixty ticks a second this
/// budget is one update every half second for somebody nowhere near you.
///
/// What that costs is bounded and small: a player outside [`SECTION_REACH`] cannot be drawn, so the
/// only thing reading their position is the fullscreen map marker, which no one reads at frame
/// rate. The moment they come within reach the cull stops applying and they are back to every tick.
const MAX_PLAYER_SYNC_SKIPS: u8 = 30;

/// What a withheld update was about, so one ledger can serve every culled broadcast.
///
/// Keyed alongside the target slot in [`GameServer::skips`]. Three kinds share the mechanism
/// because they share the question: does this player have the part of the world it happened in?
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Withheld {
    /// An NPC's state, by its index.
    Npc(u8),
    /// A player's own movement, by the slot it came from.
    Player(u8),
    /// A projectile, by the packed identity the wire carries.
    Projectile(i32),
}

// The projectiles whose debuffs are worth however many of them are stuck in the target. Each is
// its own debuff — Daybreak, javelins, tentacle spikes, blood butcherer knives, stardust cells —
// and the count is what decides the rate, so a single spear is a scratch and eight are lethal.
const DAYBREAK_SPEAR: u16 = 636;
const JAVELIN: u16 = 598;
const TENTACLE_SPIKE: u16 = 971;
const BLOOD_BUTCHERER: u16 = 975;
const STARDUST_CELL: u16 = 614;

// The three commands a mannequin's slot-sync message can carry, besides the armour slots which
// are everything else. Named because the numbers appear nowhere else and mean nothing on sight.
const DOLL_DYE: u8 = 1;
const DOLL_POSE: u8 = 2;
const DOLL_MISC: u8 = 3;

/// What a client sends to say it has closed whatever tile entity it had open.
const NO_TILE_ENTITY: i32 = -1;

// The five things packet 73 can ask for, in the order `MessageBuffer` reads them.
const TELEPORT_POTION: u8 = 0;
const MAGIC_CONCH: u8 = 1;
const DEMON_CONCH: u8 = 2;
const SHELLPHONE_SPAWN: u8 = 3;
const NO_SPACE_RESCUE: u8 = 4;

/// How long a finished save took, or that it failed. Sent from the writing thread to the game
/// task, which is the only one that can say anything to anybody.
type SaveOutcome = std::result::Result<u64, ()>;
type SaveReport = std::sync::mpsc::Sender<SaveOutcome>;
type SaveReports = std::sync::mpsc::Receiver<SaveOutcome>;

/// The most password hashes that may be in flight at once, server-wide.
///
/// Per-slot exclusivity alone is not enough: at 255 players, 255 concurrent Argon2 hashes is
/// nineteen megabytes each and every core the machine has. This is the ceiling that makes the
/// cost bounded no matter how many people ask at once.
const MAX_AUTH_IN_FLIGHT: usize = 4;

/// Vanilla's tile-edit spam ceilings and decay rates, from `RemoteClient`.
///
/// Transcribed rather than chosen: these decide how long a burst is tolerated, and picking our own
/// numbers would mean a client vanilla is happy with gets booted here, or the reverse. Placing is
/// the tight one — 100 with 0.3 a tick back means about eighteen a second sustained. Breaking is
/// deliberately loose, because mining legitimately produces a lot of packets very quickly.
const SPAM_PLACE_MAX: f32 = 100.0;
const SPAM_PLACE_DECAY: f32 = 0.3;
const SPAM_BREAK_MAX: f32 = 500.0;
const SPAM_BREAK_DECAY: f32 = 5.0;
const SPAM_LIQUID_MAX: f32 = 50.0;
const SPAM_LIQUID_DECAY: f32 = 0.2;

/// A finished password hash, on its way back to the game task to be applied.
///
/// The work — hashing a new password, or verifying one against a stored hash — happens on a
/// worker thread. Only the game task may touch the admin store, so the answer comes back here and
/// is applied on the next tick.
enum AuthOutcome {
    /// A `/register` whose password has been hashed.
    Registered {
        slot: u8,
        account: Box<std::result::Result<crate::admin::Account, String>>,
        /// Whether this account claims the server, decided before the hash was started.
        first: bool,
    },
    /// A `/login` whose password has been checked.
    SignedIn {
        slot: u8,
        account: String,
        correct: bool,
        /// The connecting address's own IP, captured before the hash was started (`self.player`
        /// may be gone by the time this comes back, if they disconnected mid-check) so
        /// [`GameServer::note_finished_auth`] can record the outcome against the same per-IP
        /// throttle key `login_throttled` checked before spawning the hash at all.
        ip_key: Option<String>,
    },
}

type AuthReport = std::sync::mpsc::Sender<AuthOutcome>;
type AuthReports = std::sync::mpsc::Receiver<AuthOutcome>;

/// The most slots a quick stack may offer at once.
///
/// A player's main inventory is forty slots and the void bag adds another forty. Anything past
/// that is a crafted packet, and reading a claimed count of two billion before checking it is how
/// a server runs out of memory.
const MAX_QUICK_STACK_SLOTS: i32 = 200;

// The two town slimes that are made rather than found, and what packet 140 calls each.
const TRANSFORM_COPPER_SLIME: u8 = 1;
const TRANSFORM_ELDER_SLIME: u8 = 2;
const COPPER_SLIME: u16 = 684;
const OLD_SLIME: u16 = 679;
/// The only slime an Old Slime can be made from.
const OLD_SLIME_SOURCE: u16 = 685;

/// The other two town slimes, each freed by something that is not a packet at all.
///
/// The Purple Slime is what its bound form becomes when killed (`NPC.HitEffect`,
/// `NPC.cs:82596-82627`); the Yellow Slime is what its bound form becomes when a thrown cloud of
/// Purification Powder touches it (`Projectile.cs:14806-14824`).
const PURPLE_SLIME: u16 = 680;
const YELLOW_SLIME: u16 = 683;

/// Whether an NPC is one a fishing rod can bring up.
///
/// Six types, all blood-moon catches: the Red Slime that unlocks itself, two hardmode fish and
/// two pre-hardmode ones, and the Eyefish. Named rather than tabled — the set is tiny, has not
/// changed since 1.4.0, and `Projectile.FishingCheck_RollEnemySpawns` lists it in one place.
///
/// The check matters more than its size suggests. Without it, packet 130 is a free spawn of
/// anything in the game at any coordinates a client cares to name.
fn is_fishable(npc_type: u16) -> bool {
    matches!(npc_type, 586 | 587 | 618 | 620 | 621 | 682)
}

/// How often a resting item's true position is re-announced, and how many go out at a time.
///
/// Drift is slow, so this is deliberately lazy: a full sweep every tick would cost more than the
/// problem it fixes.
const ITEM_DRIFT_INTERVAL: u64 = 60 * 5;
const ITEMS_PER_SWEEP: usize = 16;

/// The Travelling Merchant, who is not a resident and has no house.
const TRAVELLING_MERCHANT: u16 = 368;
/// The Old Man at the dungeon door, who is not a resident either.
const OLD_MAN: u16 = 37;
/// ...nor is the Skeleton Merchant. Neither counts towards the two townsfolk the Travelling
/// Merchant wants to see before he will visit.
const SKELETON_MERCHANT: u16 = crate::game::spawn::SKELETON_MERCHANT;
/// He only turns up in the first half of the day, and leaves at this hour.
const MERCHANT_ARRIVES_BEFORE: i32 = 27_000;
const MERCHANT_LEAVES_AT: i32 = 48_600;
/// The odds of his arriving on any given tick of the morning. `Main.UpdateTime`'s own figure.
const MERCHANT_ODDS: i32 = 27_000 * 4;
/// How many passes over the offer chain are made to fill his stock.
const MERCHANT_ROLLS: usize = 50;
/// How many slots the stock packet carries, whatever he actually has. `Main.TravelShopMaxSlots`.
const TRAVEL_SHOP_SLOTS: usize = 40;

/// The Tax Collector, who exists only as a Tortured Soul somebody threw Purification Powder at
/// (`Projectile.cs:14798`). See [`GameServer::tick_powders`].
const TAX_COLLECTOR: u16 = 441;

/// How many Purification Powder clouds the server will follow at once.
///
/// A trust boundary, not a game rule: `powders` is filled straight from packet 27, so without a
/// ceiling a client that sent nothing but powder syncs would grow it without bound. Vanilla's own
/// ceiling is `Main.maxProjectiles`, 1000 shared by everything in the world; this is far below
/// that and still hundreds of clouds more than the one throw the mechanism needs.
const MAX_TRACKED_POWDERS: usize = 256;

/// One item that shimmer is about to break apart, and what into.
struct Decraft {
    index: i16,
    at: (f32, f32),
    recipe: &'static terrustia_proto::recipes::Recipe,
    /// How many whole batches came apart. A stack only decrafts in whole batches.
    batches: i32,
    /// What was left over and stays as it was.
    kept: i16,
}

/// The most of one thing a single dropped stack holds.
///
/// A stack is a signed short on the wire and nothing in the game stacks higher, so a decraft that
/// gives back more than this spreads across several piles rather than one impossible one.
const MAX_ITEM_STACK: i16 = 9_999;

// The two things packet 146 can announce: a transmutation's sparkle, and coins becoming luck.
const SHIMMER_EFFECT: u8 = 0;
const SHIMMER_COIN_LUCK: u8 = 1;

/// One world tile in pixels, for the arithmetic that turns a position into a tile.
const TILE_SIZE: f32 = crate::game::npc::TILE;

/// The Portal Gun's two ends, which are projectiles rather than tiles.
const PORTAL_PROJECTILE: u16 = 602;

/// Wire and actuators, as items, which is what a wiring tool spends.
const WIRE_ITEM: i16 = 530;
const ACTUATOR_ITEM: i16 = 849;

/// The longest drag a wiring tool will honour, in tiles.
///
/// Not the game's rule — the game has none, relying on the client's own tool range — which is
/// exactly why one is wanted here. A drag across a large world is a hundred thousand tile edits
/// broadcast to every client, which is a denial of service dressed up as a wiring tool.
const MAX_WIRE_DRAG: i32 = 512;

/// A gem lock, and how tall one frame of its sprite sheet is.
///
/// Whether it is locked lives in the frame rather than in a flag: the lower band of the sheet is
/// the locked form, so toggling one is a matter of moving all nine of its cells between bands.
///
/// `TileID.GemLocks = 440` (`TileID.cs:1317`). This constant read 442 - `TileID
/// .ProjectilePressurePad` (`TileID.cs:1321`) - which is a different tile of a different size, so
/// `on_gem_lock` rejected every real gem lock and the feature was dead over the wire. The rest of
/// this server already had it right: `world/wiring.rs`'s own trigger table and its 3x3 footprint
/// both name 440.
const GEM_LOCK: u16 = 440;
const GEM_LOCK_FRAME_HEIGHT: i16 = 54;

/// How much of one item a player is carrying, across every slot.
fn count_held(player: &Player, item: i16) -> i32 {
    player
        .inventory
        .values()
        .filter(|slot| slot.item.id == i32::from(item))
        .map(|slot| i32::from(slot.item.stack))
        .sum()
}

/// How far in from either edge the ocean reaches. `WorldGen.beachDistance`.
const BEACH_DISTANCE: i32 = 380;
/// ...and how much of that is water rather than sand worth standing on.
const BEACH_MARGIN: i32 = 50;

/// A player's box, which decides where their top-left corner goes for a given tile.
const PLAYER_HALF_WIDTH: f32 = 10.0;
const PLAYER_HEIGHT: f32 = 42.0;

/// Whether two `(left, top, right, bottom)` boxes overlap — the same open-interval test as
/// `Rectangle.Intersects` (touching edges do not count), used to keep a meteor off any player or
/// NPC.
fn boxes_overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.2 && a.2 > b.0 && a.1 < b.3 && a.3 > b.1
}

/// The world position of a tile's top-left corner.
fn tile_corner(x: i16, y: i16) -> (f32, f32) {
    (
        f32::from(x) * crate::game::npc::TILE,
        f32::from(y) * crate::game::npc::TILE,
    )
}

/// A target dummy, whose whole purpose is to be hit without ever dying.
const TARGET_DUMMY_AI: i32 = 92;
/// The one other type the game marks immortal outright.
const IMMORTAL_TYPE: u16 = 690;

/// Whether an NPC cannot lose life at all, however much is done to it. `NPC.immortal`.
///
/// Distinct from "takes no damage": a target dummy shows the numbers, which is the entire point
/// of it, and simply never goes below the life it started with.
fn is_immortal(npc: &crate::game::npc::Npc) -> bool {
    npc.stats.ai_style == TARGET_DUMMY_AI || npc.npc_type == IMMORTAL_TYPE
}

/// A hostile NPC candidate for a town resident to fight back against: (slot, center, velocity).
type Hostile = (u8, (f32, f32), (f32, f32));

/// The nearest hostile a town resident can actually see, from among the candidates it might fight
/// back against.
///
/// Real vanilla (`AI_007_TownEntities`, `NPC.cs:54029-54101`) filters its own equivalent
/// nearest-candidate scan on `Collision.CanHit` *before* comparing distance at all
/// (`NPC.cs:54033`) — a closer hostile behind a wall never becomes a candidate to begin with, so a
/// farther one actually in view is who ends up selected. Filtering here before `min_by` is what
/// reproduces that: without it, a wall-blocked hostile nearer than an open one would always win the
/// distance comparison and — now that `try_combat` refuses to fire at a target it cannot see —
/// leave the resident with nothing it is willing to fight, for as long as the blocked hostile stays
/// alive and nearest.
fn nearest_visible_hostile(
    tiles: &impl TileView,
    npc: &crate::game::npc::Npc,
    hostiles: &[Hostile],
) -> Option<Target> {
    let here = npc.center();
    hostiles
        .iter()
        .filter(|h| {
            crate::game::ai::can_see(
                tiles,
                npc,
                Target {
                    slot: h.0,
                    center: h.1,
                    velocity: h.2,
                    alive: true,
                },
            )
        })
        .min_by(|a, b| {
            let da = (a.1.0 - here.0).powi(2) + (a.1.1 - here.1).powi(2);
            let db = (b.1.0 - here.0).powi(2) + (b.1.1 - here.1).powi(2);
            da.total_cmp(&db)
        })
        .map(|&(slot, center, velocity)| Target {
            slot,
            center,
            velocity,
            alive: true,
        })
}

#[cfg(test)]
mod nearest_visible_hostile_tests {
    use super::nearest_visible_hostile;
    use crate::game::npc::{Npc, TILE, TileView};
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    #[derive(Default)]
    struct Ground(HashMap<(i32, i32), Tile>);

    impl TileView for Ground {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn wall_at(tiles: &mut Ground, x: i32) {
        for y in 85..=110 {
            tiles.0.insert((x, y), Tile::block(1));
        }
    }

    fn merchant_at(tile_x: i32) -> Npc {
        let mut n = Npc::new(17, (0.0, 0.0), 1).expect("a real town npc type");
        n.position = (tile_x as f32 * TILE, 100.0 * TILE - n.height());
        n
    }

    /// Before this fix, the scan compared every candidate purely by distance — a wall between the
    /// resident and its nearest hostile never disqualified that hostile from being picked, it just
    /// meant `try_combat` would then refuse to fire at it, leaving a visible, fightable hostile
    /// close by completely ignored.
    #[test]
    fn a_farther_visible_hostile_is_picked_over_a_nearer_hidden_one() {
        let mut tiles = Ground::default();
        wall_at(&mut tiles, 203);
        let merchant = merchant_at(200);
        let hostiles = [
            // Nearer, but behind the wall at tile 203.
            (
                1u8,
                (merchant.center().0 + 60.0, merchant.center().1),
                (0.0, 0.0),
            ),
            // Farther, but with a clear line to it.
            (
                2u8,
                (merchant.center().0 - 150.0, merchant.center().1),
                (0.0, 0.0),
            ),
        ];
        let picked = nearest_visible_hostile(&tiles, &merchant, &hostiles)
            .expect("the farther, visible hostile should still be a valid candidate");
        assert_eq!(
            picked.slot, 2,
            "should have skipped the blocked, nearer one"
        );
    }

    #[test]
    fn the_nearest_hostile_is_picked_when_nothing_blocks_the_view() {
        let tiles = Ground::default();
        let merchant = merchant_at(200);
        let hostiles = [
            (
                1u8,
                (merchant.center().0 + 200.0, merchant.center().1),
                (0.0, 0.0),
            ),
            (
                2u8,
                (merchant.center().0 + 50.0, merchant.center().1),
                (0.0, 0.0),
            ),
        ];
        let picked = nearest_visible_hostile(&tiles, &merchant, &hostiles)
            .expect("an unobstructed hostile should be picked");
        assert_eq!(
            picked.slot, 2,
            "the ordinary unobstructed case: nearest wins"
        );
    }

    #[test]
    fn nothing_is_picked_when_every_candidate_is_hidden() {
        let mut tiles = Ground::default();
        wall_at(&mut tiles, 203);
        let merchant = merchant_at(200);
        let hostiles = [(
            1u8,
            (merchant.center().0 + 60.0, merchant.center().1),
            (0.0, 0.0),
        )];
        assert!(nearest_visible_hostile(&tiles, &merchant, &hostiles).is_none());
    }
}

/// The count a running timer is set back to whenever it fires.
///
/// Five minutes of ticks, and a multiple of every timer period, which is what keeps two timers of
/// the same kind firing on the same tick however long they have been running.
const TIMER_WINDOW: i32 = 18_000;

/// How long a pressed Detonator stays down before it pops back up, in ticks — vanilla's own
/// `CheckMech(anchor, 60)` (`Wiring.cs:362`).
const DETONATOR_WINDOW: i32 = 60;

/// Every timer in the world that is switched on.
///
/// Walked once, when the world is loaded. It is the whole world, but only once and only for the
/// tile type, which costs a few milliseconds even on a large map.
fn running_timers_in(world: &World) -> HashMap<(i32, i32), i32> {
    let mut found = HashMap::new();
    for x in 0..world.width() {
        for y in 0..world.height() {
            if crate::world::wiring::timer_is_running(world.tile(x, y)) {
                found.insert((x, y), TIMER_WINDOW);
            }
        }
    }
    if !found.is_empty() {
        info!(
            timers = found.len(),
            "picked up timers that were left running"
        );
    }
    found
}

/// Chat colour for server notices.
const SERVER_CHAT_COLOUR: [u8; 3] = [255, 240, 20];

/// Things the connection tasks tell the game task about.
pub enum ServerEvent {
    Join {
        addr: SocketAddr,
        out: mpsc::Sender<Bytes>,
        /// Ends this connection without waiting for `out` to drain: see
        /// [`crate::net::connection::Closer`], and [`GameServer::disconnect`] for who fires it.
        close: crate::net::connection::Closer,
        /// This connection's share of the server-wide outbound byte budget: see
        /// [`crate::net::connection::OUTBOUND_TOTAL_BUDGET`].
        queued: crate::net::connection::QueuedBytes,
        /// Receives the assigned `(slot, epoch)`, or `None` when the server is full.
        ///
        /// `epoch` is [`GameServer::allocate_slot`]'s per-connection generation counter (see
        /// [`GameServer::remove_player`]'s doc comment for the whole design): the connection
        /// stamps it onto every [`ServerEvent::Packet`] and its own [`ServerEvent::Leave`] below,
        /// so a ghost connection's stale events can be told apart from whoever has since taken
        /// the same slot number.
        slot: oneshot::Sender<Option<(u8, u32)>>,
    },
    Packet {
        slot: u8,
        /// The epoch this connection was handed at `Join`. Dropped by `handle_event` as a ghost
        /// if it no longer matches the slot's current epoch.
        epoch: u32,
        frame: Frame,
    },
    Leave {
        slot: u8,
        /// Same epoch as `Packet` above, checked the same way.
        epoch: u32,
    },
    /// A line typed at the server's own console.
    ///
    /// Slot 255 is "the server" in chat, and the console is treated the same way: it owns the
    /// place unconditionally, because somebody with the terminal already has the world file.
    Console { line: String },
    /// The console asking who could be tab-completed, without typing a command.
    ///
    /// Names only — a snapshot for a completion popup, not something that should ever gate on the
    /// game task being free. The console times its own wait out and falls back to no suggestions
    /// rather than stall a keypress on a busy tick.
    ConsoleContext {
        reply: oneshot::Sender<ConsoleContext>,
    },
    /// The web panel asking what it needs to resolve a login, for one account name.
    ///
    /// Never carries a password: argon2 is deliberately expensive (see
    /// `Admin::account_hash_and_group`'s own doc comment) and must run on the panel's own task,
    /// off the game task's tick — this only hands back what that verification needs.
    PanelAuthLookup {
        name: String,
        reply: oneshot::Sender<PanelAuthLookup>,
    },
    /// The web panel inserting an account it already hashed off the game task (claiming an
    /// unclaimed server). Cheap — no hashing happens here, only the push into the store.
    PanelInsertAccount {
        account: crate::admin::Account,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// The web panel's own `/api/login` throttle logging one summarised refusal. The panel keeps
    /// its own `admin::Throttle`s (see `panel::PanelState`'s doc comment for why), but the
    /// append-only audit log lives on the game task, so this is how a refusal reaches it. No
    /// reply: `login` has already decided to refuse the request either way.
    PanelAuditThrottled { target: String, detail: String },
    /// The web panel asking for a snapshot to show on its status view.
    PanelStatus { reply: oneshot::Sender<PanelStatus> },
    /// The web panel asking who is connected, for the player list and the live world view.
    PanelPlayers {
        reply: oneshot::Sender<Vec<PanelPlayer>>,
    },
    /// The web panel's kick button. Reuses exactly what `/kick` and the console's `kick` already
    /// call ([`Self::kick`]/[`Self::announce`] by way of `run_admin_command`'s own logic) rather
    /// than a second copy of it. `actor` is the signed-in account making the request, for the audit
    /// log's own `issuer` field.
    PanelKick {
        actor: String,
        name: String,
        reason: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// The web panel's ban button. Same reasoning as `PanelKick`.
    PanelBan {
        actor: String,
        kind: crate::admin::BanKind,
        value: String,
        reason: String,
        reply: oneshot::Sender<()>,
    },
    PanelUnban {
        actor: String,
        value: String,
        reply: oneshot::Sender<usize>,
    },
    /// The web panel's mute button. `duration_secs` is `None` for permanent, matching `/mute`'s own
    /// optional-duration grammar (parsed in the panel handler, not here — the panel accepts a plain
    /// number of seconds rather than the console's `10m`/`2h` words, since it has a form field
    /// rather than a chat line to parse).
    PanelMute {
        actor: String,
        name: String,
        reason: String,
        duration_secs: Option<u64>,
        reply: oneshot::Sender<()>,
    },
    PanelUnmute {
        actor: String,
        name: String,
        reply: oneshot::Sender<bool>,
    },
    /// The web panel asking who is on the guest list, and whether it is currently in force.
    PanelWhitelist {
        reply: oneshot::Sender<PanelWhitelist>,
    },
    /// `actor` is the signed-in account making the request, recorded against
    /// [`crate::admin::AuditAction::Whitelist`] on a change, matching every other moderation route.
    PanelWhitelistAdd {
        actor: String,
        name: String,
        reply: oneshot::Sender<bool>,
    },
    /// Same reasoning as [`Self::PanelWhitelistAdd`].
    PanelWhitelistRemove {
        actor: String,
        name: String,
        reply: oneshot::Sender<bool>,
    },
    /// A coarse, sampled view of the world's tiles, for the panel's live world screen. Sampled
    /// rather than exhaustive — see [`Self::world_tile_sample`]'s own doc comment for why a full
    /// tile-for-tile dump is neither necessary nor safe to send over a websocket on every tick.
    PanelWorldTiles {
        reply: oneshot::Sender<PanelWorldTiles>,
    },
    /// A read-only snapshot of the running configuration, for the panel's settings view.
    PanelConfigSnapshot {
        reply: oneshot::Sender<PanelConfigSnapshot>,
    },
    /// The one setting the panel is allowed to change live: the MOTD is read fresh out of
    /// `Config` every time a player spawns (see `on_player_spawn`), so writing a new one here takes
    /// effect for the very next join with nothing else to coordinate.
    PanelSetMotd {
        motd: String,
        reply: oneshot::Sender<()>,
    },
    /// Ask to restart the process pointed at a different world file. `path` has already been
    /// resolved and validated against `crate::worlds::list()` by the panel — this only checks it
    /// still exists (it could have been removed between the listing and the click) and, if so, sets
    /// `stopping` so the ordinary shutdown save runs before `main` re-execs.
    PanelSwitchWorld {
        path: PathBuf,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// A live snapshot of the last tick's cost and the current entity counts, for the panel's
    /// metrics view. Reads [`Self::last_tick`] and a few `len()`s — nothing that costs the tick.
    PanelMetrics {
        reply: oneshot::Sender<PanelMetrics>,
    },
    /// The world backups on disk right now, for the panel's backup/rollback view. The listing
    /// itself is a filesystem read, but which file is *current* — and therefore what the backups
    /// belong to — is game-task state, so it is answered here.
    PanelBackups {
        reply: oneshot::Sender<PanelBackups>,
    },
    /// The panel's "save now" button. Reuses the same background save the console's `save` command
    /// runs, so it never blocks the tick.
    PanelForceSave {
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// The panel's rollback button. Reuses [`Self::roll_back`] exactly — the same destructive,
    /// stop-and-reload operation the console `rollback` command performs — and hands its message
    /// back so the panel can show it.
    PanelRollback {
        which: usize,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// The groups and accounts, for the panel's accounts admin view.
    PanelAccounts {
        reply: oneshot::Sender<PanelAccounts>,
    },
    /// Move an account into a different group. Same rule the console `group` command enforces (the
    /// group must exist), plus two guards: the lock-out guard the panel has always had (refuses to
    /// strip the last account that can still edit permissions, so nobody locks themselves out
    /// through the very screen they would use to fix it), and the anti-escalation guard every
    /// account/group change needs — `actor` (the signed-in account making the request) must already
    /// be able to reach every permission `group` holds, or the change is refused. See
    /// [`crate::admin::store::Admin::group_within_reach`].
    PanelSetAccountGroup {
        actor: String,
        name: String,
        group: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// The panel inserting an account it already hashed off the game task, into a chosen group.
    /// Distinct from [`Self::PanelInsertAccount`], the claim path, which always uses `owner`:
    /// this one validates the requested group exists first, and applies the same anti-escalation
    /// guard [`Self::PanelSetAccountGroup`] does — `actor` must already reach everything the new
    /// account's group holds.
    PanelCreateAccount {
        actor: String,
        account: crate::admin::Account,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Delete an account. Guarded the same way [`Self::PanelSetAccountGroup`] is, on both counts:
    /// the last account that can still administer the server cannot be removed, and `actor` must
    /// already reach everything the target account's own group holds (see
    /// [`crate::admin::store::Admin::group_within_reach`]) or the deletion is refused. Without that
    /// second guard an `admin.accounts` holder could delete an `owner` account outright, which is a
    /// strictly bigger escalation than anything `PanelSetAccountGroup`'s reach check stops.
    PanelDeleteAccount {
        actor: String,
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Whether the named account's group grants a permission — the per-route authorization check
    /// every panel handler makes, fresh on every request rather than cached at login, since a
    /// group's permissions (or an account's group) can change mid-session. Runs on the game task
    /// because only it holds the live `Admin` store. See `panel/mod.rs`'s module doc for the full
    /// route-to-permission mapping.
    PanelAuthorize {
        name: String,
        permission: String,
        reply: oneshot::Sender<bool>,
    },
    /// Add or remove a single permission string on a group — the group-editor's own mutation.
    /// `actor` must already hold `permission` themselves (checked the same way
    /// [`Self::PanelSetAccountGroup`]'s reach guard is, just for one permission instead of a whole
    /// group's worth), so nobody can use the editor to grant a group — including their own — a
    /// permission they do not already have. Refuses an unrecognised permission name outright,
    /// before it ever reaches the guard, so a typo is reported rather than silently doing nothing.
    PanelSetGroupPermission {
        actor: String,
        group: String,
        permission: String,
        grant: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Where the audit log writes, for the panel's audit view, or `None` for an in-memory log with
    /// nothing to read. The panel reads and parses the file itself, off the game task
    /// (`crate::admin::audit::tail_file`); the game task hands back only the path.
    PanelAuditPath {
        reply: oneshot::Sender<Option<std::path::PathBuf>>,
    },
}

/// What the console can offer to complete a command's second argument with.
#[derive(Debug, Default, Clone)]
pub struct ConsoleContext {
    pub players: Vec<String>,
    pub groups: Vec<String>,
}

/// How the game loop ended.
///
/// This exists so the process can exit with a code that means something. A panicked game task used
/// to look identical to a clean stop from outside — `main` returned `Ok(())` either way — so a
/// server that had crashed exited 0, and every supervisor configured with
/// `Restart=on-failure` (or a Docker restart policy keyed on exit status) left it dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Stopped {
    /// Asked to stop, and did.
    Cleanly,
    /// Something panicked. The world was still saved on the way out.
    Panicked,
}

pub struct GameServer {
    config: Config,
    world: World,
    players: Vec<Option<Player>>,
    /// One generation counter per slot, bumped by [`Self::allocate_slot`] every time it hands the
    /// slot to somebody new. Never touched by [`Self::remove_player`] - only a fresh allocation
    /// moves it - so it survives exactly as long as it needs to and no longer: see
    /// `remove_player`'s doc comment for the race this closes.
    slot_epochs: Vec<u32>,
    ticks: u64,
    items: ItemStore,
    npcs: NpcStore,
    /// Encoded sections, keyed by section coordinates.
    ///
    /// A section is several kilobytes of deflate that costs about 0.13 ms to build, and the same
    /// one goes to every player who joins or walks past. Caching turns that into a memcpy; the
    /// whole cache for a full-size world is well under a megabyte.
    section_cache: HashMap<(i32, i32), Bytes>,
    rng: SmallRng,
    save_path: Option<PathBuf>,
    /// Ticks between autosaves, or `None` when saving is unavailable or disabled.
    autosave_ticks: Option<u64>,
    /// Everything currently in flight.
    projectiles: crate::game::projectile::ProjectileStore,
    /// Purification Powder clouds a client has thrown, kept only until they settle.
    ///
    /// Not in `projectiles`, and not on the wire: see [`crate::game::projectile::Powder`] for why,
    /// and [`Self::tick_powders`] for what they are for. Bounded, because a client packet fills it.
    powders: Vec<crate::game::projectile::Powder>,
    /// How many shots have been fired since the server started, for `/npcs`.
    shots_thrown: u64,
    /// The invasion under way, if any.
    invasion: Option<InvasionState>,
    /// The Old One's Army, which is a siege rather than an invasion and so keeps its own state.
    army: crate::game::army::ArmyState,
    /// The arena's two ends, surveyed once when the crystal goes down and kept until it is gone.
    army_arena: Option<((i32, i32), (i32, i32))>,
    /// Which moon is up, how far through its waves it is, and how far into the current wave.
    ///
    /// A moon is not an invasion: there is nothing to see off, only twenty waves and one night to
    /// get through as many of them as you can. Dawn ends it wherever you got to.
    moon: crate::game::moons::MoonState,
    /// The Lunar Apocalypse: four pillars and what comes after them.
    lunar: crate::game::lunar::LunarState,
    /// The wind and the rain, which several ported routines read.
    weather: crate::game::weather::Weather,
    /// How much of the world is hallow, corruption and crimson. One column is counted per tick,
    /// as the game does, and the figures go out in packet 57 when a sweep completes.
    census: crate::world::census::Census,
    /// Which network each announced pylon belongs to, by position.
    ///
    /// Remembered rather than re-derived, because a pylon's network lives in its *tile's* frame
    /// and mining the tile is what removes the entity — so by the time there is a removal to
    /// announce, the frame is gone. The client matches a removal on position **and** type
    /// (`TeleportPylonInfo.Equals`), so getting the type wrong there does not remove anything: the
    /// pylon stays on every travel map for the rest of the session.
    pylon_kinds: HashMap<(i16, i16), u8>,
    /// How many updates in a row each culled thing has been withheld from each player.
    ///
    /// One ledger for NPCs, player movement and projectiles: see [`Withheld`].
    skips: HashMap<(Withheld, u8), u8>,
    /// The deepest any connection's outbound queue has been seen since the last tick report.
    ///
    /// Memory under load is queued frames, not the world: a 255-player hold peaks around 1.5 GiB
    /// against an idle 95 MiB, and `OUTBOUND_PER_PLAYER` was widened to 4096 precisely to let that
    /// backlog build rather than drop clients. Sampling the depth is what turns "memory is high"
    /// into a number attached to a cause, the same way `skips` did for the ledger.
    queue_high_water: usize,
    /// Scratch for [`GameServer::broadcast`]'s recipient list, reused across calls.
    ///
    /// The list has to be collected before sending because a send can remove a player and so
    /// invalidate an in-flight iterator, but collecting it into a fresh `Vec` every call is an
    /// allocation per broadcast, and under a full server that is hundreds a tick.
    broadcast_targets: Vec<u8>,
    /// How much nearness each player has accumulated towards the next streamed update of each NPC.
    npc_stream: HashMap<(u8, u8), u8>,
    /// Whose turn it is to have the ground around them searched for a house.
    housing_turn: usize,
    /// Timers that are switched on, and how long each has left in its window.
    running_timers: HashMap<(i32, i32), i32>,
    /// Trap tiles that have fired recently, and how long until each can fire again.
    ///
    /// The game keeps the same list, capped at 999 entries; this one is a map because looking a
    /// tile up is what it is for.
    mech_cooldown: HashMap<(i32, i32), i32>,
    /// Detonators pressed recently, by their top-left anchor, and how many ticks until each pops
    /// back up. The `UpdateMech` reset that makes the button momentary (`Wiring.cs:219-244`), timed
    /// here because it fires outside any circuit trip (L3-26).
    detonator_resets: HashMap<(i32, i32), i32>,
    /// The three cannon lockouts, which are *world*-level rather than per-tile: vanilla keeps them
    /// as three plain counters beside the `CheckMech` table (`Wiring.cs:69-73`) and winds each down
    /// once a frame in `UpdateMech` (`:147-158`). They stack with each cannon's own `CheckMech`
    /// window rather than replacing it, so a battery of cannons on one circuit fires in sequence
    /// instead of all at once: the plain Cannon takes 120 frames off every other Cannon in the
    /// world (`:1335`), the Bunny Cannon 480 (`:1338`), and the Snowball Launcher 15 (`:1393`).
    cannon_cooldown: i32,
    bunny_cannon_cooldown: i32,
    snowball_cannon_cooldown: i32,
    /// A save being written on another thread, if one is.
    ///
    /// Kept so that shutdown can wait for it and so that two never run at once.
    saving: Option<tokio::task::JoinHandle<()>>,
    /// A save that has been asked for and is waiting for the snapshot buffer to catch up with the
    /// world. See [`GameServer::try_fire_pending_save`]. Carries the reason the arming call gave.
    pending_save: Option<&'static str>,
    /// The tick a pending save was armed on: the drain's deadline, and what `drain_ticks` reports.
    pending_save_armed: u64,
    /// Why the save in flight was started, so the right thing is said when it finishes.
    save_reason: &'static str,
    /// How a finished background save reports back to the game task.
    ///
    /// A channel rather than the join handle because the tick is not async and polling a handle
    /// for its value needs an executor; a try-receive needs nothing.
    save_results: (SaveReport, SaveReports),
    /// How many world saves have failed in a row. `0` means saving is healthy.
    ///
    /// This is the whole of the saves-failing state: the panel reads it (as
    /// [`PanelStatus::save_failures`]), the in-game escalation is keyed off it crossing
    /// [`SAVE_FAILURES_BEFORE_ALARM`], and a single success clears it. Not persisted, deliberately:
    /// a restart is a fresh attempt, and a counter carried across one would keep alarming about a
    /// disk that has since been fixed.
    save_failures: u32,
    /// Password hashing that finished on a worker thread, waiting to be applied.
    ///
    /// Argon2 costs tens of milliseconds by design, against a tick budget of 16.67. Running it
    /// inline — which `/register` and `/login` used to do, with no permission check and no rate
    /// limit — meant any connected client could freeze the world for everybody simply by sending
    /// `/register` in a loop. The hash now happens off the game task and its answer arrives here,
    /// the same shape the background save already uses.
    auth_results: (AuthReport, AuthReports),
    /// Slots with a hash already running. One at a time, so nobody can queue up work.
    auth_in_flight: std::collections::HashSet<u8>,
    /// Per-IP login backoff, shared by the join password (`dispatch::on_password`) and `/login`
    /// (`console::run_admin_command`'s `"login"` arm): both are "does this address keep failing a
    /// password check" in the same sense. See `admin::throttle`'s own doc comment for the design.
    ip_throttle: crate::admin::Throttle,
    /// Per-account `/login` backoff, keyed by the (lowercased) account name typed rather than
    /// whether it actually exists: see `login_throttled`'s own doc comment for why that matters.
    account_throttle: crate::admin::Throttle,
    /// A one-time secret for claiming an unclaimed server, printed to the console at startup.
    ///
    /// Without this, the first account made owns the server — so on a fresh public server whoever
    /// connected first became the owner, and *everyone* had every permission until they did.
    /// That is fine among friends and a gift to a stranger.
    ///
    /// Requiring the token means claiming needs someone who can read the server's own terminal,
    /// which is the same trust boundary the console already sits behind: whoever has the terminal
    /// already has the world file. Cleared once the server is claimed.
    claim_token: Option<String>,
    /// A world-sized buffer to copy the next snapshot into, once a save has given one back.
    ///
    /// Held so the copy writes into pages that are already mapped. Empty while a save is running,
    /// and empty on the first save of a session, which is the one that pays for the mapping.
    spare_world: Option<crate::world::World>,
    /// Finished saves handing their buffer back.
    world_returns: (
        std::sync::mpsc::Sender<crate::world::World>,
        std::sync::mpsc::Receiver<crate::world::World>,
    ),
    /// The six cavern enemies this world happens to have.
    ///
    /// Drawn from the world's id rather than from the run's generator, so the same world always
    /// has the same six. Worked out once when the world is opened.
    cavern_monsters: crate::game::cavern_monsters::CavernMonsters,
    /// Each player's last biome scan, reused by the spawn rate rather than re-scanned every tick.
    /// See [`crate::game::spawn::BiomeCache`] for why it has to be cached at all.
    player_biomes: crate::game::spawn::BiomeCache,
    /// The last pillar shields, invasion progress and Moon Lord countdown that went out.
    ///
    /// All three are recomputed every tick and almost never move, so they are compared before
    /// they are sent. Broadcasting them unconditionally would be three packets a tick to every
    /// client for the whole of an event.
    last_sent_shields: [i32; 4],
    last_sent_countdown: i32,
    last_sent_invasion: (i32, i32),
    /// What the Travelling Merchant is carrying, if he is here.
    ///
    /// Rolled on arrival and thrown away when he leaves. Not world state: he takes his stock
    /// with him, so a world reloaded mid-visit simply has no merchant.
    travel_shop: Vec<u16>,
    /// Which fish the Angler is asking for today, as an index into the quest table.
    angler_quest: u8,
    /// Who has already handed one in since dawn.
    ///
    /// By name rather than by slot, because that is what the game keys it on and because a player
    /// who reconnects into a different slot should not get a second go at the reward.
    angler_finished_today: std::collections::HashSet<String>,
    /// Which tile entity each player currently has open, if any.
    ///
    /// One player per entity, which is the point: without it two people can open the same
    /// mannequin and each take everything off it.
    tile_entity_anchors: HashMap<u8, i32>,
    /// Liquid waiting to settle. Empty unless something has disturbed it.
    liquids: crate::world::liquid::Liquids,
    /// `Liquid.skipCount`: the liquid simulation runs only every second tick, not every tick
    /// (`WorldGen.cs:72072-72079`). This is half of what keeps liquid from running roughly four
    /// times too fast (the per-tile `skipLiquid` flag inside the sim is the other half, L3-09).
    liquid_skip_count: u8,
    /// Who may do what, and who is kept out. Read from disk beside the world.
    admin: crate::admin::Admin,
    /// The append-only record of every moderation and permission-affecting action. Beside the world
    /// too, like `admin`.
    audit: crate::admin::AuditLog,
    /// Set by the console's `stop`, so the loop ends and the world is saved on the way out.
    stopping: bool,
    worst_tick: TickCost,
    /// The cheapest and dearest tick since the status footer last drew, about a second's worth.
    ///
    /// The footer used to print the cost of whichever single tick happened to land on the refresh
    /// boundary, and that reads as wild instability: an idle server showed 70 us one second and
    /// 350 the next, which looks like the server misbehaving. It is not. Subsystems run on
    /// different cadences (liquid every other tick, the spawn pass periodically, section streaming
    /// only when somebody crosses a boundary, the save drain only while a save is armed), so tick
    /// cost is genuinely multi-modal and one sample in sixty is a random draw from it. Measured on
    /// an idle 4200x1200 world, the *worst* tick per ten-second window sat in a tight 255 to 342 us
    /// band across seven windows while the footer swung by 5x. Showing the range says both true
    /// things at once: the floor, and what it actually peaks at.
    status_span: (Duration, Duration),
    /// The longest a tick has been held off the processor this window.
    worst_stall: Duration,
    /// The palette the boot chose, so the live status footer can colour itself to match — or emit
    /// nothing when colour is off. Defaults to plain; `main` sets it via [`Self::with_palette`].
    palette: crate::term::Palette,
    /// The most recent tick's cost, surfaced live to the web panel's metrics view. Unlike
    /// [`Self::worst_tick`] — a windowed maximum that is taken and reset every report — this is
    /// simply the last tick that ran, so a graph drawn from it moves every frame. Never persisted.
    last_tick: TickCost,
    /// A trailing, time-windowed log of player tile edits, for `/world undo`. See
    /// [`crate::game::tile_log`]'s own module doc for retention and scope.
    tile_log: crate::game::tile_log::TileLog,
    /// The other end of [`crate::panel::supervise`]'s toggle channel, for the console's `panel`
    /// command. `None` in every test that never calls [`Self::with_panel_toggle`] — the console
    /// command still works in that case, it just has nothing to notify and says so.
    panel_toggle: Option<mpsc::UnboundedSender<()>>,
    /// Set by [`ServerEvent::PanelSwitchWorld`], and read by `main` after [`Self::run`] returns.
    ///
    /// Switching worlds is not something a single running process can do to its own in-memory
    /// [`crate::world::World`] safely — every system in this file, from `section_cache` to
    /// `cavern_monsters`, is sized and seeded for the world that was loaded at startup. The honest
    /// version is a full graceful restart pointed at a different save file: this field is how the
    /// request crosses from the game task (which decides *whether* to stop, by setting
    /// `stopping`) to `main` (which is the only thing that can actually replace the process), once
    /// `run` has already saved the current world and returned.
    ///
    /// An `Arc<Mutex<..>>` rather than a channel because `main` needs to read it *after* `self` has
    /// been moved into the spawned task and consumed by `run` — a handle cloned out before that
    /// move is the only way to still have an opinion once the task is gone.
    pending_world_switch: Arc<Mutex<Option<PathBuf>>>,
    /// Journey mode's shared toggles. See [`crate::game::journey`]'s own module doc for which
    /// powers this covers and, just as importantly, which fifteen-minus-eleven it does not yet.
    journey: crate::game::journey::JourneyPowers,
    /// Set by `main`'s background `update::boot_check` task once it finds a newer, signature-
    /// verified release; taken (delivered, then cleared) the first time a recognised admin signs
    /// in afterward, in [`Self::note_finished_auth`]. `None` in every test that never calls
    /// [`Self::with_update_notice`] — the sign-in path still works in that case, it just has
    /// nothing to hand over. An `Arc<Mutex<..>>` for the same reason `pending_world_switch` above
    /// is one: `main` needs to keep writing to it from outside after `self` is moved into the
    /// spawned game task, so a handle cloned out before that move is the only way in.
    update_notice: Option<Arc<Mutex<Option<String>>>>,
    /// The birthday party — see [`crate::game::party`]'s own module doc.
    party: crate::game::party::PartyState,
    /// Slime Rain — see [`crate::game::slime_rain`]'s own module doc.
    slime_rain: crate::game::slime_rain::SlimeRainState,
    /// Lantern Night — see [`crate::game::lantern_night`]'s own module doc.
    lantern_night: crate::game::lantern_night::LanternNightState,
    /// Whether tonight's `Star.starfallBoost` cleared 3, rolled at dusk by
    /// [`Self::roll_starfall_night`].
    ///
    /// The boost itself is a `float` in the game (`Star.cs:35`) and its other reader is the star
    /// fall that actually drops Fallen Stars (`WorldGen.cs:72406`), which this server does not
    /// model. The one thing the spawner asks of it is `> 3f` (`NPC.cs:2409`), so that is what is
    /// kept: a bool per night rather than a number nothing else here would read.
    starfall_night: bool,
    /// `NPC.Spawner.fairyLog` (`NPC.cs:150`): whether this world still has a fallen log in it,
    /// which is the one gate on the underground fairy (`spawn::underground_fairy`).
    ///
    /// `MysticLogFairiesEvent` owns the flag in the game and re-decides it by scanning the whole
    /// overworld at three moments: world load (`WorldGen.cs:3272`), every dusk
    /// (`Main.cs:66212`) and whenever a fallen log is broken (`WorldGen.cs:50299-50302`). This
    /// server keeps the first two, in [`Self::scan_for_fallen_logs`]; the third is the same scan
    /// with a narrower trigger, and skipping it only means a world whose last log was mined at dusk
    /// keeps its fairies until the next one.
    fairy_log: bool,
}

impl GameServer {
    pub fn new(config: Config, mut world: World) -> Self {
        // What day it is in the real world, which decides Halloween and Christmas. The game reads
        // it here too, on load and on generation (`WorldGen.cs:6906-6907`, `:11267-11268`), not only
        // at dawn: a server started on the thirty-first of October should be spawning Hoppin' Jacks
        // that first night rather than waiting for the following morning.
        world.refresh_calendar();
        // From here on, tile edits invalidate cached sections.
        world.start_tracking_changes();
        // A full snapshot now, while startup has no tick budget to blow, so the *first* real
        // autosave has a baseline to diff against instead of paying for a full copy inside a
        // counted tick. Measured on a real CI soak run before this existed: 14,833 µs — 89% of a
        // single tick's 16,666 µs budget, on the very first save after the server came up. This
        // moves that first full copy off the clock; what every save after it costs is a separate
        // and much larger number than an earlier note here claimed, measured and tabulated in
        // `save_world`'s own comment (2.0 to 12.8 ms, scaling with how much has changed).
        let spare_world = Some(world.snapshot());
        let slots = config.max_players;
        let save_path = config.save_target().map(Path::to_path_buf);
        let audit_log_max_bytes = config.audit_log_max_bytes;
        let audit_log_keep_segments = config.audit_log_keep_segments;
        let autosave_ticks = match (save_path.is_some(), config.autosave_secs) {
            (true, secs) if secs > 0 => Some(secs * 60),
            _ => None,
        };
        if save_path.is_none() {
            warn!(
                "no save target: set `save_file`, or pass --save, to keep this world when the \
                 server stops"
            );
        }

        // Timers that were left switched on are picked up here rather than waiting to be hit
        // again. That is a deliberate divergence: the game keeps its list of running timers only
        // in memory, so reopening a world leaves every one of them drawn as on and doing nothing
        // until somebody flips it twice. On a single-player world that is a curiosity; on a
        // server it means every contraption in the world dies silently on a restart.
        let running_timers = running_timers_in(&world);

        // The weather comes off the world it was loaded with, so a reloaded save picks up the
        // shower it was in the middle of rather than starting clear.
        let weather = crate::game::weather::Weather {
            wind: world.wind,
            target: world.wind,
            raining: world.raining,
            rain_time: world.rain_time,
            max_rain: world.max_rain,
            sandstorm: world.sandstorm,
            sandstorm_time: world.sandstorm_time,
            severity: world.sandstorm_severity,
            intended_severity: world.sandstorm_intended_severity,
            ..Default::default()
        };

        // Read before the world is moved into the server, since the monsters are drawn from it.
        let world_id = world.id;
        let mut server = Self {
            config,
            world,
            players: (0..slots).map(|_| None).collect(),
            slot_epochs: vec![0; slots],
            ticks: 0,
            items: ItemStore::new(),
            npcs: NpcStore::new(),
            section_cache: HashMap::new(),
            rng: SmallRng::seed_from_u64(0x7e77_a51a),
            projectiles: crate::game::projectile::ProjectileStore::new(),
            powders: Vec::new(),
            shots_thrown: 0,
            invasion: None,
            army: crate::game::army::ArmyState::default(),
            army_arena: None,
            moon: crate::game::moons::MoonState::default(),
            lunar: crate::game::lunar::LunarState::default(),
            weather,
            census: crate::world::census::Census::new(terrustia_proto::tile_sets::TILE_COUNT),
            pylon_kinds: HashMap::new(),
            skips: HashMap::new(),
            queue_high_water: 0,
            broadcast_targets: Vec::new(),
            npc_stream: HashMap::new(),
            housing_turn: 0,
            running_timers,
            mech_cooldown: HashMap::new(),
            detonator_resets: HashMap::new(),
            cannon_cooldown: 0,
            bunny_cannon_cooldown: 0,
            snowball_cannon_cooldown: 0,
            tile_entity_anchors: HashMap::new(),
            saving: None,
            pending_save: None,
            pending_save_armed: 0,
            save_reason: "",
            save_results: std::sync::mpsc::channel(),
            save_failures: 0,
            auth_results: std::sync::mpsc::channel(),
            auth_in_flight: std::collections::HashSet::new(),
            ip_throttle: crate::admin::Throttle::new(),
            account_throttle: crate::admin::Throttle::new(),
            claim_token: None,
            spare_world,
            world_returns: std::sync::mpsc::channel(),
            cavern_monsters: crate::game::cavern_monsters::CavernMonsters::for_world(world_id),
            player_biomes: crate::game::spawn::BiomeCache::default(),
            // Deliberately impossible starting values, so the first tick of each always sends.
            last_sent_shields: [-1; 4],
            last_sent_countdown: -1,
            last_sent_invasion: (-1, -1),
            travel_shop: Vec::new(),
            angler_quest: 0,
            angler_finished_today: std::collections::HashSet::new(),
            liquids: crate::world::liquid::Liquids::default(),
            liquid_skip_count: 0,
            // Beside the world it belongs to. A world with nowhere to save has nowhere to put
            // this either, and should not scatter one into whatever directory it was started in.
            admin: match &save_path {
                Some(path) => crate::admin::Admin::load(&path.with_extension("admin.toml")),
                None => crate::admin::Admin::in_memory(),
            },
            audit: match &save_path {
                Some(path) => crate::admin::AuditLog::new(
                    path.with_extension("audit.jsonl"),
                    audit_log_max_bytes,
                    audit_log_keep_segments,
                ),
                None => crate::admin::AuditLog::in_memory(),
            },
            stopping: false,
            worst_tick: TickCost::default(),
            status_span: (Duration::MAX, Duration::ZERO),
            worst_stall: Duration::ZERO,
            last_tick: TickCost::default(),
            save_path,
            autosave_ticks,
            tile_log: crate::game::tile_log::TileLog::default(),
            panel_toggle: None,
            pending_world_switch: Arc::new(Mutex::new(None)),
            journey: crate::game::journey::JourneyPowers::default(),
            update_notice: None,
            party: crate::game::party::PartyState::default(),
            slime_rain: crate::game::slime_rain::SlimeRainState::default(),
            lantern_night: crate::game::lantern_night::LanternNightState::default(),
            starfall_night: false,
            fairy_log: false,
            palette: crate::term::Palette::PLAIN,
        };
        // The Angler wants something from the moment the world opens, not from the first dawn.
        // A server that waited would give the first day's players nothing to do for him.
        server.roll_angler_quest();
        // Count the world once up front. The per-tick sweep would otherwise take a full minute of
        // play to produce its first figures, and the Dryad would tell everyone who joined in that
        // minute that their world was nought per cent of everything.
        server.census.sweep(&server.world);
        // ...and look for a fallen log once up front, which is `OnWorldLoad +=
        // mysticLogsEvent.StartWorld` (`WorldGen.cs:3272` -> `MysticLogFairiesEvent.cs:26-32`). A
        // world opened at night would otherwise have no underground fairies until its first dusk.
        server.scan_for_fallen_logs();
        // Remember every pylon's network up front, rather than waiting for the first join to fill
        // the map in. Otherwise a pylon mined before anybody had connected would be announced with
        // the wrong network and stay on every travel map afterwards.
        for pylon in server.pylons() {
            server.pylon_kinds.insert((pylon.x, pylon.y), pylon.kind);
        }
        server
    }

    /// Wires the console's `panel` command up to a running [`crate::panel::supervise`] task.
    /// Builder-style rather than a `new()` parameter: every existing call site (`main`, and
    /// seventeen call sites across this workspace's tests) would otherwise need a channel most of
    /// them have no use for, just to construct a server at all.
    pub fn with_panel_toggle(mut self, toggle: mpsc::UnboundedSender<()>) -> Self {
        self.panel_toggle = Some(toggle);
        self
    }

    /// Wires up the handle `main`'s background `update::boot_check` task writes a message into
    /// once it finds a newer, signature-verified release. Builder-style for the same reason as
    /// [`Self::with_panel_toggle`] just above — most call sites have no use for one.
    pub fn with_update_notice(mut self, notice: Arc<Mutex<Option<String>>>) -> Self {
        self.update_notice = Some(notice);
        self
    }

    /// Give the game task the boot's colour palette, so the live status footer it keeps at the
    /// bottom of the terminal can match it. Builder-style for the same reason as the two above —
    /// the many test call sites want a plain default, not a palette they have no terminal for.
    pub fn with_palette(mut self, palette: crate::term::Palette) -> Self {
        self.palette = palette;
        self
    }

    /// A handle to whatever [`ServerEvent::PanelSwitchWorld`] requests, for `main` to read once
    /// [`Self::run`] has returned. Call this before `run` consumes `self` — cloning an `Arc` out of
    /// a value about to be moved is the whole trick.
    pub fn world_switch_handle(&self) -> Arc<Mutex<Option<PathBuf>>> {
        self.pending_world_switch.clone()
    }

    /// Every pylon in the world, as module 8 describes them.
    ///
    /// A pylon keeps nothing of its own — which network it belongs to is the tile's frame — so the
    /// kind is read back off the tile rather than stored beside the entity, which is also what
    /// keeps the two from ever disagreeing.
    fn pylons(&self) -> Vec<net_module::Pylon> {
        use terrustia_proto::tile_entity::EntityKind;

        self.world
            .tile_entities
            .iter()
            .filter(|entity| entity.kind == EntityKind::TeleportationPylon)
            .filter_map(|entity| {
                let tile = self.world.tile(i32::from(entity.x), i32::from(entity.y));
                if !tile.is_active() || tile.block != EntityKind::TeleportationPylon.tile() {
                    // The entity outlived its tile. Not worth announcing: the client would draw a
                    // travel destination that is not there.
                    return None;
                }
                Some(net_module::Pylon {
                    x: entity.x,
                    y: entity.y,
                    // A pylon is three tiles wide, so each style occupies 54 pixels of frame.
                    kind: (tile.frame_x / PYLON_FRAME_WIDTH) as u8,
                })
            })
            .collect()
    }

    /// Tell one player about a pylon appearing or vanishing.
    fn broadcast_pylon(&mut self, message: net_module::PylonMessage, pylon: net_module::Pylon) {
        if let Ok(frame) = net_module::pylon_message(message, pylon) {
            self.broadcast(frame, None);
        }
    }

    /// How many housed town NPCs live within a pylon's scan box *and are actually home*.
    ///
    /// `TeleportPylonsSystem.DoesPositionHaveEnoughNPCs`, TeleportPylonsSystem.cs:224-247. Three
    /// conditions, not one: the resident has a home, that home is inside the scan box, and the
    /// resident is standing within a hundred tiles of it (`Vector2.Distance(home, Center / 16f) <
    /// 100f`, TeleportPylonsSystem.cs:235-237).
    ///
    /// The last of those was missing, so a pylon stayed usable while both its residents were off
    /// wandering the far side of the world. It is the same "is anybody actually there" idea the
    /// happiness check spells out at 120 tiles (`ShopHelper.IsFarFromHome`), and leaving it out
    /// made the pylon network answer a question about houses when the game asks one about people.
    fn town_npcs_near(&self, x: i16, y: i16) -> usize {
        self.npcs
            .iter()
            .filter(|(_, npc)| npc.stats.town_npc && npc.is_alive())
            .filter(|(_, npc)| {
                let Some((hx, hy)) = npc.home else {
                    // Homeless townsfolk do not count towards a pylon, in the game or here.
                    return false;
                };
                // `Utils.CenteredRectangle(centre, SceneMetrics.ZoneScanSize)` is 169 by 124 tiles
                // (`SceneMetrics.cs:16`), and `Rectangle.Contains` is inclusive at the top-left and
                // exclusive at the bottom-right: -84..=84 across, -62..=61 down. The lower bound
                // read `<= 62`, which is one row of houses too generous.
                if (hx - i32::from(x)).abs() > PYLON_SCAN_HALF_WIDTH
                    || hy - i32::from(y) < -PYLON_SCAN_HALF_HEIGHT_UP
                    || hy - i32::from(y) > PYLON_SCAN_HALF_HEIGHT_DOWN
                {
                    return false;
                }
                let (cx, cy) = npc.center();
                let (dx, dy) = (
                    cx / crate::game::npc::TILE - hx as f32,
                    cy / crate::game::npc::TILE - hy as f32,
                );
                (dx * dx + dy * dy).sqrt() < PYLON_RESIDENT_HOME_RANGE
            })
            .count()
    }

    /// The shopping zones a player is standing in, as `ShopHelper` wants to see them.
    ///
    /// The game reads these off the client's own `SceneMetrics` scan, which a dedicated server
    /// never runs. Here they come from the per-player [`BiomeCache`] the spawner already keeps,
    /// through `last` rather than `read`: this is reached from a packet handler, and a handler
    /// must never be able to make a client pay for a fresh 78 us scan on demand. The spawn pass
    /// refreshes that entry every tick for every active player, so the answer is the current one;
    /// a player with no entry yet (dead, or in their first tick) reads as plain forest.
    ///
    /// Two disclosed narrowings. The game's zone flags are independent and several can be true at
    /// once (`Player.ShoppingZone_AnyBiome`, `Player.cs:3807-3817`), and this sets every one the
    /// scan answers independently (`desert` and `mushroom` are [`Zones`] flags of their own); the
    /// remaining five are read off the single winner, so at most one of *those* is ever set. And
    /// the scan is centred on the player's top-left corner rather than their centre, which is what
    /// every other caller here does.
    ///
    /// `mushroom` used to be hardcoded `false` on the grounds that the server did not model the
    /// glowing mushroom biome, which cost the Truffle his one biome like. It does model it:
    /// `ZoneGlowshroom` is `EnoughTilesForGlowingMushroom` and nothing else (`SceneMetrics.cs:684`),
    /// which is the count [`spawn::Zones::glowshroom`] already makes in this very scan.
    ///
    /// [`BiomeCache`]: crate::game::spawn::BiomeCache
    /// [`Zones`]: crate::game::spawn::Zones
    /// [`spawn::Zones::glowshroom`]: crate::game::spawn::Zones::glowshroom
    fn shopping_zones(&self, slot: u8) -> terrustia_proto::happiness::Zones {
        use crate::game::spawn::Biome;
        use terrustia_proto::happiness::Zones;

        let Some(position) = self.player(slot).map(|p| p.position) else {
            return Zones::default();
        };
        let y = (position.1 / crate::game::npc::TILE) as i32;
        // A player with no entry yet reads as plain forest: no winner but `Forest`, no flag set.
        let zones = self.player_biomes.last(usize::from(slot));
        let biome = zones.map_or(Biome::Forest, |z| z.biome);
        Zones {
            ocean: biome == Biome::Ocean,
            snow: biome == Biome::Snow,
            desert: zones.is_some_and(|z| z.desert),
            jungle: biome == Biome::Jungle,
            hallow: biome == Biome::Hallow,
            mushroom: zones.is_some_and(|z| z.glowshroom),
            corruption: biome == Biome::Corruption,
            crimson: biome == Biome::Crimson,
            dungeon: biome == Biome::Dungeon,
            // `Player.ShoppingZone_BelowSurface`, `Player.cs:3819`.
            below_surface: y > i32::from(self.world.surface),
        }
    }

    /// What one town NPC's happiness does to the prices it quotes a player.
    ///
    /// The game's `Player.SetTalkNPC` (`Player.cs:4360-4375`) calls this once when the chat opens
    /// and caches the answer, and so does this: it is not a per-tick cost, and there is no cadence
    /// to invent because the game has none. Measured at 0.27 us for a thirty-five-resident town
    /// (`examples/happiness_cost.rs`), so even the absurd case of every player on a full server
    /// opening a chat in the same tick is 68 us of a 16.67 ms budget.
    fn shop_multiplier(&self, slot: u8, index: u8) -> f32 {
        use terrustia_proto::happiness::Resident;

        let resident = |npc: &crate::game::npc::Npc| {
            let (cx, cy) = npc.center();
            Resident {
                npc_type: npc.npc_type,
                home: npc.home,
                center: (cx / crate::game::npc::TILE, cy / crate::game::npc::TILE),
            }
        };
        let Some(shopkeeper) = self.npcs.get(index).map(&resident) else {
            return 1.0;
        };
        let others: Vec<Resident> = self
            .npcs
            .iter()
            .filter(|(other, npc)| *other != index && npc.stats.town_npc && npc.is_alive())
            .map(|(_, npc)| resident(npc))
            .collect();
        let zones = self.shopping_zones(slot);
        terrustia_proto::happiness::price_multiplier(
            &shopkeeper,
            &others,
            zones,
            self.world.secret_seeds.remix,
        )
    }

    /// Whether a pylon's surroundings still match its network, the game's biome gate on travelling
    /// to it (`TeleportPylonsSystem.DoesPylonAcceptTeleportation`, TeleportPylonsSystem.cs:254-312).
    ///
    /// The scan the game runs there is the same one the spawner uses (`spawn::zones_at`), so the
    /// biome networks read straight across. The pylon kinds are the game's `TeleportPylonType`
    /// values (TeleportPylonType.cs:5-15): 0 surface purity, 1 jungle, 2 hallow, 3 underground,
    /// 4 beach, 5 desert, 6 snow, 7 glowing mushroom, 8 victory, 9 underworld, 10 shimmer.
    ///
    /// The game reads each biome's tile count independently (`_sceneMetrics.EnoughTilesForJungle`
    /// and its siblings), so a spot can be over several thresholds at once. `spawn::Zones` answers
    /// four of those independently (`desert`, `glowshroom`, `shimmer`, `meteor`) and collapses the
    /// rest into one winner, so the four are read as flags here and the rest as the winner. That
    /// collapse is the one remaining narrowing on this function's biome side: a surface-purity
    /// pylon standing in, say, a snowfield that a corruption has already outvoted reads as
    /// corruption, and either answer refuses it, so the surviving cases are all ones where two
    /// clauses of the same `if` would have agreed anyway.
    ///
    /// Glowing mushroom (7) and shimmer (10) used to fall through to the permissive `_ => true`,
    /// on the grounds that this server modelled neither zone. It models both:
    /// `EnoughTilesForGlowingMushroom` is `MushroomTileCount >= 100` (`SceneMetrics.cs:52`, `:260`)
    /// and `EnoughTilesForShimmer` is `ShimmerTileCount >= 300` (`:24`, `:252`), both counted in
    /// the box this scan already walks. A mushroom or shimmer pylon dug out of its biome now stops
    /// working, as it does in the game.
    ///
    /// Not transcribed: the `Main.remixWorld` branches on the surface-purity (:260-262) and beach
    /// (:287-291) arms, which were already absent before this and are left so; the glowing
    /// mushroom arm's own remix branch (:295-297) *is* transcribed, since that arm is written here
    /// in full.
    fn pylon_accepts(&self, pylon: &net_module::Pylon) -> bool {
        use crate::game::spawn::{Biome, Depth, depth_at, zones_at};
        let (x, y) = (i32::from(pylon.x), i32::from(pylon.y));
        let depth = depth_at(&self.world, y);
        let zones = zones_at(&self.world, x, y);
        let biome = zones.biome;
        // The game's edge band (TeleportPylonsSystem.cs:265, :285): within 380 tiles of either side.
        let near_edge = x <= 380 || x >= self.world.width() - 380;
        match pylon.kind {
            // Jungle, Hallow, Desert, Snow simply demand their biome (TeleportPylonsSystem.cs:276,
            // :299, :280, :278).
            1 => biome == Biome::Jungle,
            2 => biome == Biome::Hallow,
            5 => biome == Biome::Desert,
            6 => biome == Biome::Snow,
            // SurfacePurity: the plain surface, clear of the edge bands and of every special biome
            // (TeleportPylonsSystem.cs:258-274). The game's exclusion list at :270 is Jungle, Snow,
            // Desert, *GlowingMushroom*, Hallow, Crimson, Corruption; the mushroom term was missing
            // here, so a surface purity pylon buried in a mushroom field kept working.
            0 => {
                depth == Depth::Surface
                    && !near_edge
                    && !zones.desert
                    && !zones.glowshroom
                    && !matches!(
                        biome,
                        Biome::Jungle
                            | Biome::Snow
                            | Biome::Hallow
                            | Biome::Corruption
                            | Biome::Crimson
                    )
            }
            // Beach: the surface band by an ocean edge (TeleportPylonsSystem.cs:282-292). The band
            // is bounded at both ends, `Y <= worldSurface && Y > worldSurface * 0.35`: `Surface`
            // alone reaches all the way to row zero, which let a pylon in the clouds pass as a
            // coastal one.
            4 => {
                depth == Depth::Surface
                    && near_edge
                    && y as f32 > f32::from(self.world.surface) * BEACH_PYLON_SKY_LIMIT
            }
            // Underground: anywhere at or below the surface line (TeleportPylonsSystem.cs:301-302).
            3 => depth != Depth::Surface,
            // Underworld: the underworld layer (TeleportPylonsSystem.cs:305-306).
            9 => depth == Depth::Underworld,
            // GlowingMushroom: enough mushroom tiles, and in a remix world never within 200 rows of
            // the bottom, because that is where remix puts its surface (TeleportPylonsSystem.cs:
            // 293-298).
            7 => {
                let remix_floor = self.world.secret_seeds.remix
                    && y >= self.world.height() - REMIX_MUSHROOM_PYLON_FLOOR;
                !remix_floor && zones.glowshroom
            }
            // Shimmer: enough shimmer, and nothing else. The game reads the raw
            // `EnoughTilesForShimmer` here rather than `ZoneShimmer`, so its depth and dungeon
            // terms do not apply (TeleportPylonsSystem.cs:307-308).
            10 => zones.shimmer,
            // Victory (8, TeleportPylonsSystem.cs:303-304) travels from anywhere, and so does
            // anything past the enum, matching the game's own `default` (:309-310).
            _ => true,
        }
    }

    /// Whether a pylon is a Lihzahrd temple pylon sealed until Plantera falls, the game's early
    /// access gate (`TeleportPylonsSystem.HandleTeleportRequest`, TeleportPylonsSystem.cs:124).
    ///
    /// The game refuses a destination that is below the surface, standing on the temple's own wall
    /// (`WallID.LihzahrdBrickUnsafe`, WallID.cs:243, value 87), while Plantera is still alive, so
    /// the temple's network cannot be reached before the temple is meant to open.
    fn temple_pylon_sealed(&self, pylon: &net_module::Pylon) -> bool {
        const LIHZAHRD_BRICK_WALL: u16 = 87;
        !self.world.progress.downed_plantera
            && i32::from(pylon.y) > i32::from(self.world.surface)
            && self.world.tile(i32::from(pylon.x), i32::from(pylon.y)).wall == LIHZAHRD_BRICK_WALL
    }

    /// The whole banner kill table, as module 11 message 0.
    fn banner_state_frame(&self) -> terrustia_proto::Result<Vec<u8>> {
        let mut kills = [0u32; net_module::BANNER_SLOTS];
        for (&banner, &count) in &self.world.banner_kills {
            if let Some(slot) = kills.get_mut(usize::from(banner)) {
                *slot = count;
            }
        }
        // Nothing is ever claimable here: a banner is dropped where the kill happened rather than
        // held for collection, so there is never a pending one to report.
        let claimable = [0u16; net_module::BANNER_SLOTS];
        net_module::banners_full_state(&kills, &claimable)
    }

    /// One packet 60 per town NPC, saying where it lives.
    ///
    /// A homeless one still gets a frame — the game sends its home tile anyway and flags it as
    /// homeless, which is how the client knows to draw the "no home" marker rather than nothing.
    fn npc_home_frames(&self) -> Vec<Vec<u8>> {
        self.npcs
            .iter()
            .filter(|(_, npc)| npc.stats.town_npc && npc.is_alive())
            .filter_map(|(index, _)| self.npc_home_frame(index))
            .collect()
    }

    /// One packet 60 for one town NPC.
    fn npc_home_frame(&self, index: u8) -> Option<Vec<u8>> {
        let npc = self.npcs.get(index)?;
        let (home, status) = match npc.home {
            // The game distinguishes "has a home tile" from "has a room the game agrees is
            // habitable"; this server only tracks the first, so a housed NPC reports `Settled`
            // rather than claiming a room record it does not keep.
            Some(home) => (home, packets::HouseholdStatus::Settled),
            // A homeless one still reports a position — wherever it happens to be standing — which
            // is what the game sends for an NPC that has been evicted.
            None => (
                (
                    (npc.position.0 / 16.0) as i32,
                    (npc.position.1 / 16.0) as i32,
                ),
                packets::HouseholdStatus::Homeless,
            ),
        };
        packets::npc_home(u16::from(index), home.0 as i16, home.1 as i16, status).ok()
    }

    /// Tell every client where one town NPC lives, after it moves in or is evicted.
    fn broadcast_npc_home(&mut self, index: u8) {
        if let Some(frame) = self.npc_home_frame(index) {
            self.broadcast(frame, None);
        }
    }

    /// Write the world to disk, blocking the game task until it is done.
    ///
    /// Only for shutdown, where there is no next tick to protect and the process must not exit
    /// before the bytes are on disk. Everything else wants [`Self::save_world_in_background`].
    fn save_world(&mut self, reason: &'static str) {
        let Some(path) = self.save_path.clone() else {
            return;
        };
        self.record_town_npcs();
        self.record_lunar_pillars();
        self.record_journey_powers();
        let started = Instant::now();
        match wld_save::save(&self.world, &path) {
            Ok(()) => {
                let ms = started.elapsed().as_millis();
                info!(path = %crate::worlds::display_path(&path), reason, elapsed_ms = ms as u64, "world saved");
                self.note_save_succeeded(reason);
                self.announce(&format!("World saved ({ms} ms)."));
            }
            Err(e) => {
                error!(path = %crate::worlds::display_path(&path), error = %e, "world save failed");
                self.note_save_failed(reason);
            }
        }
    }

    /// A save landed. Clear the failure state, and say so if anyone was told about it.
    ///
    /// The all-clear is owed to exactly the people who heard the alarm: if the count never reached
    /// [`SAVE_FAILURES_BEFORE_ALARM`] then nobody in the game was told anything, and announcing
    /// that saving is working again would be the first they had heard of it going wrong. The log
    /// line is unconditional, because an operator reading it back wants both edges.
    fn note_save_succeeded(&mut self, reason: &str) {
        let failures = std::mem::take(&mut self.save_failures);
        if failures == 0 {
            return;
        }
        info!(
            reason,
            after_failures = failures,
            "world saves are working again"
        );
        if failures >= SAVE_FAILURES_BEFORE_ALARM {
            self.announce(
                "Saving is working again. Everything since the last warning has now been kept.",
            );
        }
    }

    /// A save failed. Warn, count it, and escalate once it stops being a blip.
    ///
    /// What this deliberately does *not* do is stop the server. A disk that is full or briefly
    /// unwritable is a condition to survive, not to die of: the world in memory is still the good
    /// one, the previous save on disk is still intact (see [`crate::safe_write`]), and the next
    /// autosave is one cadence away. Exiting here would throw away the very state the operator is
    /// trying to keep.
    ///
    /// The escalation is repeated on every failure past the threshold rather than only on the
    /// crossing. Somebody who joined since the first warning has no way of knowing the server is in
    /// this state, and at the default five-minute cadence that is one line every five minutes,
    /// which is the right price for "your progress is not being kept".
    fn note_save_failed(&mut self, reason: &str) {
        self.save_failures = self.save_failures.saturating_add(1);
        let failures = self.save_failures;
        warn!(
            reason,
            failures,
            "a world save failed; the previous save is intact and the next autosave will retry"
        );
        // Whoever asked for this particular save is answered whatever the count says.
        if reason == "command" {
            self.announce("World save FAILED; see the server log.");
        }
        if failures >= SAVE_FAILURES_BEFORE_ALARM {
            self.announce(&format!(
                "Warning: the world has failed to save {failures} times in a row. Progress since \
                 the last good save is at risk - please tell whoever runs this server."
            ));
        }
    }

    /// Take a copy of the world and let another thread write it out.
    ///
    /// Serialising a large world costs about fifty-five milliseconds against a tick budget of
    /// sixteen and a half, so an autosave on the game task drops three or four ticks — a visible
    /// stutter every five minutes, and worse the bigger the world. Measured with
    /// `examples/savecost.rs`; the cost is in run-detection and encoding rather than in reading
    /// the tiles, which is why making the reads cache-friendlier did nothing.
    ///
    /// The copy costs the tick a few milliseconds of memcpy and nothing else, and it is atomic
    /// with respect to the tick, so the writer can never catch the world halfway through an edit.
    ///
    /// One save at a time. If a previous one is still going the new request is dropped rather
    /// than queued: two saves racing for the same path is worse than a missed autosave, and a
    /// server whose disk cannot keep up with its autosave interval should not build a backlog of
    /// sixty-megabyte snapshots waiting for it.
    ///
    /// This half only *arms* the save. The copy itself is spread over the next few ticks by
    /// [`Self::tick_snapshot_drain`] and fires from [`Self::try_fire_pending_save`] once the
    /// buffer has caught up - see those two for why.
    fn save_world_in_background(&mut self, reason: &'static str) {
        if self.save_path.is_none() {
            return;
        }
        if let Some(running) = &self.saving
            && !running.is_finished()
        {
            warn!(reason, "a save is still running; skipping this one");
            return;
        }
        // A second request arriving while one is still waiting for the buffer takes over the
        // reason but keeps the original deadline, so a stream of them cannot postpone the save
        // past [`SNAPSHOT_DRAIN_DEADLINE`] one reset at a time.
        if self.pending_save.is_none() {
            self.pending_save_armed = self.ticks;
        }
        self.pending_save = Some(reason);
        // Nothing to drain (an idle world, or one still being generated) means this fires here and
        // now, exactly as it always did.
        self.try_fire_pending_save();
    }

    /// Copy a few of the changed sections into the snapshot buffer, while a save waits to fire.
    ///
    /// Demand-driven, not a continuous trickle: this does nothing at all unless a save is armed,
    /// so an idle tick pays nothing and a section a player is digging through is not copied
    /// repeatedly only to be copied again at save time anyway. It copies exactly the sections the
    /// one-shot path copied, spread over as many ticks as it takes.
    fn tick_snapshot_drain(&mut self) {
        if self.pending_save.is_none() {
            return;
        }
        if let Some(spare) = self.spare_world.as_mut()
            && self.world.snapshot_is_incremental()
        {
            self.world
                .pre_copy_snapshot_tiles(spare, SNAPSHOT_DRAIN_PER_TICK);
        }
        self.try_fire_pending_save();
    }

    /// Take a copy of the world and let another thread write it out, once the copy is ready.
    ///
    /// **Why this waits.** Copying forty megabytes of tiles is the most expensive thing an idle
    /// server does, and on a world nobody is digging through almost none of those tiles have
    /// changed since the last save. The buffer a finished save hands back already holds that
    /// state, so only the sections changed since need copying into it - but even that is not
    /// cheap. Measured on a fresh 4200x1200 world with nobody connected:
    ///
    /// ```text
    ///     autosave every  15s   30 to 36 sections    2.0 to 3.1 ms
    ///     autosave every 300s   68 sections          6.3 ms
    ///     a loaded world with a town, every 300s    12.8 ms
    /// ```
    ///
    /// A real 1h49m run on a loaded world spiked to 24.8 ms, 149% of a tick's 16,666 us budget,
    /// with two NPCs and nobody connected, against a normal tick of 103 us. That is not a
    /// regression - an older build copies 36 sections in 3,059 us, slightly worse - it is simply
    /// what a whole-world diff has always cost.
    ///
    /// So the tile copying is drained a few sections a tick by [`Self::tick_snapshot_drain`] and
    /// the save fires only on a tick where nothing is left marked. **At that instant the buffer is
    /// bit-identical to the live world**: `World::set_tile` re-marks every section it touches, so
    /// a section copied early and then edited goes back on the list and is copied again. The
    /// buffer is assembled across ticks but delivered at one instant, which keeps the guarantee
    /// `World::snapshot` claims ("a torn save is much worse than a slow one") rather than trading
    /// it away. An autosave landing at 300.05 s instead of 300.00 s is not observable.
    ///
    /// The final refresh below still pays a fixed cost whatever the drain left: the side tables
    /// and the object tables are copied wholesale. Measured on the owner's real 4200x1200 world
    /// with 180 chests, that is **20 us**, so it is not worth being clever about.
    ///
    /// Serialising the copy costs about fifty-five milliseconds against the same budget, which is
    /// why it goes to another thread at all (`examples/savecost.rs`; the cost is in run-detection
    /// and encoding rather than in reading the tiles, which is why making the reads
    /// cache-friendlier did nothing).
    fn try_fire_pending_save(&mut self) {
        let Some(reason) = self.pending_save else {
            return;
        };
        let waited = self.ticks.saturating_sub(self.pending_save_armed);
        // A drain that never finishes would postpone the save for ever, which risks more than a
        // stutter does. Ten seconds of not converging and the whole remainder is copied here, back
        // to the old spike - but only under a dirty rate no real server produces. An idle world
        // marks about 0.009 sections a tick against a drain rate of eight.
        let overdue = waited >= SNAPSHOT_DRAIN_DEADLINE;
        let left = self.world.snapshot_pending();
        if left > 0
            && !overdue
            && self.spare_world.is_some()
            && self.world.snapshot_is_incremental()
        {
            return;
        }
        if left > 0 && overdue {
            warn!(
                reason,
                sections_left = left,
                waited_ticks = waited,
                "the snapshot buffer never caught up with the world; copying the rest in one tick"
            );
        }
        self.pending_save = None;

        let Some(path) = self.save_path.clone() else {
            return;
        };

        // The roster has to reach the world before the snapshot is taken, or the copy that goes to
        // disk holds whoever lived here when it was loaded rather than who lives here now - and
        // the same goes for the Lunar Pillars, or a save mid-Lunar-Apocalypse drops them outright.
        //
        // These run on the *firing* tick and never on the arming one. They write tables that
        // `copy_everything_but_tiles_from` copies wholesale, so running them early would leave the
        // object tables newer than the tiles, which is precisely the tear the drain exists to
        // avoid. None of them touches a tile, so they cannot dirty a section after the drain has
        // finished.
        self.record_town_npcs();
        self.record_lunar_pillars();
        self.record_journey_powers();

        // Copy into a buffer we already own where we have one. A fresh `snapshot()` asks the
        // allocator for a new forty-megabyte mapping and then faults in every page of it as it
        // writes: measured on a 4200x1200 world, 2.600 ms against 0.989 ms for copying into a
        // buffer whose pages are already mapped.
        //
        // Kept as the record of why the drain above exists, because a note here once claimed every
        // save after the first was "already 150-200 us" and was never re-measured. It was not, and
        // the cost scaled with how long the world had been left to change rather than with the
        // tick: 30 to 36 sections and 2.0 to 3.1 ms at a 15-second autosave, 68 sections and 6.3 ms
        // at the default 300, and on a real server run for two hours, seventeen of twenty-two
        // autosaves between 8,647 and 24,808 us with two NPCs and nobody connected, against a
        // normal tick of 103 us. It was never a regression, only invisible: the phase timer used to
        // bill the work elsewhere, and the note above said it was free.
        let began = Instant::now();
        let (mut snapshot, sections) = match self.spare_world.take() {
            Some(mut spare) if self.world.snapshot_is_incremental() => {
                let sections = self.world.refresh_snapshot(&mut spare);
                (spare, Some(sections))
            }
            // Either the first save of the session, or a world still being generated or loaded,
            // where nothing has been tracking changes.
            Some(mut spare) => {
                spare.copy_state_from(&self.world);
                (spare, None)
            }
            None => (self.world.snapshot(), None),
        };
        snapshot.shrink_caches();
        let copied = began.elapsed();

        let report = self.save_results.0.clone();
        // The writer hands the buffer back when it is finished with it, so the next save can
        // reuse it instead of asking for another mapping.
        let returned = self.world_returns.0.clone();
        self.saving = Some(tokio::task::spawn_blocking(move || {
            let started = Instant::now();
            let outcome = match wld_save::save(&snapshot, &path) {
                Ok(()) => {
                    let ms = started.elapsed().as_millis() as u64;
                    // A routine autosave that worked is not news, and on a default server it
                    // happens every five minutes for as long as the server runs: a couple of hours
                    // of ordinary operation was producing twenty-odd identical lines saying nothing
                    // had gone wrong. It goes to debug, and anything an operator would actually
                    // want to see stays at info: a save they asked for, a shutdown save, and any
                    // autosave slow enough to be worth knowing about. Failures are `error!` below
                    // and are never quieted.
                    if reason == "autosave" && ms < SLOW_SAVE_MS {
                        debug!(path = %crate::worlds::display_path(&path), reason, elapsed_ms = ms, "world saved");
                    } else {
                        info!(path = %crate::worlds::display_path(&path), reason, elapsed_ms = ms, "world saved");
                    }
                    Ok(ms)
                }
                Err(e) => {
                    error!(path = %crate::worlds::display_path(&path), error = %e, "world save failed");
                    Err(())
                }
            };
            // A closed channel means the server is already shutting down, which is not an error.
            let _ = report.send(outcome);
            // Hand the buffer back for the next save to write into. If nobody is listening the
            // server is stopping and it simply drops here.
            let _ = returned.send(snapshot);
        }));
        self.save_reason = reason;
        debug!(
            reason,
            snapshot_us = copied.as_micros() as u64,
            // `None` means the whole world was copied. A number suddenly equal to every section in
            // the world means the incremental path has quietly stopped working.
            sections_copied = sections,
            // How many ticks the drain took. Zero is an idle world with nothing to copy; a number
            // near SNAPSHOT_DRAIN_DEADLINE means it barely converged, and the warning above says
            // if it did not.
            drain_ticks = waited,
            "world snapshot taken; saving in the background"
        );
    }

    /// Start hashing a new account's password, once everything cheap has been checked.
    ///
    /// `owner` says this account claims the server. It is decided by the caller rather than
    /// re-derived here, because the two callers disagree about what earns it: from chat it takes
    /// the console's claim token, and from the console it takes nothing at all.
    ///
    /// `password` is never logged below, at any level: see `admin::mod`'s own "never logged"
    /// convention. It only ever travels into `Account::new`, which turns it into a PHC hash and
    /// nothing else.
    fn begin_registration(&mut self, slot: u8, account: &str, password: &str, owner: bool) {
        // Everything decidable without hashing is decided first, so a bad request costs nothing.
        if self.admin.name_taken(account) {
            self.tell(
                slot,
                &format!("there is already an account called {account}"),
            );
            return;
        }
        if password.len() < 6 {
            self.tell(
                slot,
                "that password is too short; use at least six characters",
            );
            return;
        }
        if !self.start_auth(slot) {
            return;
        }
        let group = if owner { "owner" } else { "default" }.to_string();
        let (account, password) = (account.to_string(), password.to_string());
        let report = self.auth_results.0.clone();
        tokio::task::spawn_blocking(move || {
            let made = crate::admin::Account::new(&account, &password, &group);
            let _ = report.send(AuthOutcome::Registered {
                slot,
                account: Box::new(made),
                first: owner,
            });
        });
    }

    /// List the world backups on disk, newest first.
    fn list_backups(&mut self) {
        let Some(path) = self.save_path.clone() else {
            info!("this world is not being saved, so there is nothing to roll back to");
            return;
        };
        let mut found = 0;
        for n in 1..=crate::world::wld_save::BACKUPS_KEPT {
            let bak = path.with_extension(format!("wld.bak{n}"));
            let Ok(meta) = std::fs::metadata(&bak) else {
                continue;
            };
            let age = meta
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map_or_else(
                    || "unknown".to_string(),
                    |d| format!("{}m ago", d.as_secs() / 60),
                );
            info!(
                backup = n,
                size_mb = meta.len() / 1_048_576,
                age,
                path = %bak.display(),
                "backup"
            );
            found += 1;
        }
        if found == 0 {
            info!("no backups yet; one is made each time the world is saved");
        } else {
            info!("roll one back with:  rollback <n>   (the server stops afterwards)");
        }
    }

    /// Put a backup back, and stop so it is loaded cleanly on the next start.
    ///
    /// Deliberately *not* a hot swap. Replacing the world under a running server would leave every
    /// connected client holding tiles that no longer exist, the NPC roster pointing at houses that
    /// moved, and the next autosave writing the in-memory world straight back over the backup that
    /// was just restored — undoing the rollback within five minutes. Stopping is honest and takes
    /// one restart.
    ///
    /// Returns the message describing what happened either way, so a caller that is not the console
    /// (the web panel) can report it to whoever asked rather than only to the log. The console arm
    /// logs whatever comes back; on success the same announce/stop side effects happen regardless of
    /// who called.
    fn roll_back(&mut self, which: usize) -> Result<String, String> {
        let Some(path) = self.save_path.clone() else {
            return Err(
                "this world is not being saved, so there is nothing to roll back to".into(),
            );
        };
        if which == 0 || which > crate::world::wld_save::BACKUPS_KEPT {
            return Err(format!(
                "there are only {} backups; use `rollback 1` for the most recent",
                crate::world::wld_save::BACKUPS_KEPT
            ));
        }
        let bak = path.with_extension(format!("wld.bak{which}"));
        if !bak.exists() {
            return Err(format!("there is no backup #{which} to roll back to"));
        }
        // Check it before trusting it. Restoring an unreadable file over a readable one would turn
        // a rollback into the very thing it exists to undo.
        std::fs::read(&bak)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                crate::world::wld::parse(&bytes)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
            .map_err(|e| format!("that backup will not load; refusing to roll back to it: {e}"))?;
        // Keep what is being replaced, so a rollback is itself reversible.
        let aside = path.with_extension("wld.before-rollback");
        if path.exists()
            && let Err(e) = std::fs::rename(&path, &aside)
        {
            return Err(format!(
                "could not move the current world aside; refusing to roll back: {e}"
            ));
        }
        // Through a temporary file, not a plain `std::fs::copy`: that truncates the destination
        // before it fills it, so a disk that ran out (or a process killed) halfway would leave the
        // world path holding a fragment of a world. The `aside` rename below undoes it for the
        // error we can see, but not for the process dying mid-copy, and a rollback is exactly the
        // moment somebody cannot afford a second accident.
        if let Err(e) = crate::safe_write::copy_atomic("restoring a world backup", &bak, &path) {
            let _ = std::fs::rename(&aside, &path);
            return Err(format!("could not restore the backup: {e}"));
        }
        self.announce("The world is being rolled back; the server is stopping.");
        info!(
            backup = which,
            replaced = %aside.display(),
            "world rolled back; stopping so it loads cleanly on the next start"
        );
        // The in-memory world must not be written over what was just restored.
        self.save_path = None;
        self.stopping = true;
        Ok(format!(
            "rolled back to backup #{which}; the server is stopping so it loads cleanly on the \
             next start"
        ))
    }

    /// Print the one-time claim token, if this server has not been claimed yet.
    ///
    /// Only ever to the log, which means the terminal or the service journal — never to a player.
    /// The whole point is that claiming requires someone who can see the server's own output.
    fn announce_claim_token(&mut self) {
        if !self.admin.unclaimed() {
            return;
        }
        // Not a password: a short one-time secret that lives for one process. It gates ownership on
        // two network-reachable paths (`/register <name> <pw> <token>` and an unclaimed panel
        // login), so it is drawn from a real CSPRNG — the same `rand_core::OsRng` the panel's own
        // session tokens use — rather than the clock+pid it used to be, whose entropy an attacker
        // who can estimate the boot time could search. It only has to be unguessable by someone who
        // cannot see this line.
        use argon2::password_hash::rand_core::{OsRng, RngCore};
        const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789"; // 30 symbols
        let mut token = String::with_capacity(12);
        while token.len() < 12 {
            let mut byte = [0u8; 1];
            OsRng.fill_bytes(&mut byte);
            // Reject the top of the byte range so the map onto 30 symbols is unbiased (240 = 8*30).
            if byte[0] < 240 {
                token.push(ALPHABET[(byte[0] % 30) as usize] as char);
            }
        }
        warn!(
            "this server has no accounts yet, so everyone connecting has every permission. \
             claim it with:  /register <name> <password> {token}   (or `claim <name> <password>` \
             here at the console)"
        );
        self.claim_token = Some(token);
    }

    /// Claim a slot's one allowed password hash, or explain why not.
    ///
    /// Two limits, and both are needed. Per-slot exclusivity stops one client queueing thousands
    /// of hashes; the server-wide ceiling stops two hundred and fifty-five clients each queueing
    /// one. Without them `/register` — which needs no permission at all — was a way for anybody
    /// who could join to stall the world for everybody else.
    fn start_auth(&mut self, slot: u8) -> bool {
        if self.auth_in_flight.contains(&slot) {
            self.tell(slot, "still checking your last one; wait a moment.");
            return false;
        }
        if self.auth_in_flight.len() >= MAX_AUTH_IN_FLIGHT {
            self.tell(
                slot,
                "the server is busy checking passwords; try again shortly.",
            );
            return false;
        }
        self.auth_in_flight.insert(slot);
        true
    }

    /// The gate `/login` checks before spending anything on the credential it was just handed:
    /// whether either the caller's own address or the account name they typed (case-folded, so a
    /// throttled attacker cannot dodge it by changing case: matches `Admin::account_hash`'s own
    /// case-insensitive lookup) currently has a backoff window open. `account_key` is the name as
    /// typed, not whether it resolves to a real account, deliberately: keying only on real names
    /// would let an attacker distinguish "this account exists" from "it does not" by whether
    /// their spam ever starts slowing down. See `admin::REFUSAL_MESSAGE`'s own doc comment.
    ///
    /// Tells `slot` the shared generic refusal and returns `true` if either window is open, in
    /// which case the caller must stop right there: no hash, no lookup, nothing that could itself
    /// leak anything a fast rejection would not. Any refusal worth a line in the audit log is
    /// written here, already folded down to a summary by `Throttle::check`. See its own doc
    /// comment for why this never becomes one log line per spam attempt.
    fn login_throttled(&mut self, slot: u8, ip_key: Option<&str>, account_key: &str) -> bool {
        let now = std::time::Instant::now();
        let mut refused = false;
        if let Some(ip_key) = ip_key
            && let crate::admin::Verdict::Refused { log_summary, .. } =
                self.ip_throttle.check(ip_key, now)
        {
            refused = true;
            if let Some(n) = log_summary {
                self.audit.record(
                    "system",
                    crate::admin::AuditAction::Throttled,
                    &format!("ip:{ip_key}"),
                    &format!("{n} refused login attempt(s) backed off"),
                );
            }
        }
        if let crate::admin::Verdict::Refused { log_summary, .. } =
            self.account_throttle.check(account_key, now)
        {
            refused = true;
            if let Some(n) = log_summary {
                self.audit.record(
                    "system",
                    crate::admin::AuditAction::Throttled,
                    &format!("account:{account_key}"),
                    &format!("{n} refused login attempt(s) backed off"),
                );
            }
        }
        if refused {
            self.tell(slot, crate::admin::REFUSAL_MESSAGE);
        }
        refused
    }

    /// Reclaim the snapshot buffer from a save that has finished with it.
    fn reclaim_snapshot_buffer(&mut self) {
        if let Ok(spare) = self.world_returns.1.try_recv() {
            self.spare_world = Some(spare);
        }
    }

    /// Apply any password hashing that finished since the last tick.
    ///
    /// Polled rather than awaited, for the same reason the save report is: the tick is not async
    /// and should not become so for this.
    fn note_finished_auth(&mut self) {
        while let Ok(outcome) = self.auth_results.1.try_recv() {
            match outcome {
                AuthOutcome::Registered {
                    slot,
                    account,
                    first,
                } => {
                    self.auth_in_flight.remove(&slot);
                    match *account {
                        Ok(made) => {
                            let (name, group) = (made.name.clone(), made.group.clone());
                            match self.admin.insert_account(made) {
                                Ok(()) => {
                                    let _ = self.admin.save();
                                    self.tell(slot, &format!("account '{name}' made ({group})."));
                                    if first {
                                        // Spent. A second claim must not be possible.
                                        self.claim_token = None;
                                        self.tell(
                                            slot,
                                            "you are the first account here, so you own it.",
                                        );
                                    }
                                    // Self-registration: whoever is registering is the only
                                    // identity there is to attribute it to.
                                    self.audit.record(
                                        &name,
                                        if first {
                                            crate::admin::AuditAction::Claim
                                        } else {
                                            crate::admin::AuditAction::Register
                                        },
                                        &name,
                                        &format!("group: {group}"),
                                    );
                                    info!(account = %name, group = %group, "account registered");
                                }
                                // Somebody else took the name while this was hashing.
                                Err(e) => self.tell(slot, &e),
                            }
                        }
                        Err(e) => self.tell(slot, &e),
                    }
                }
                AuthOutcome::SignedIn {
                    slot,
                    account,
                    correct,
                    ip_key,
                } => {
                    self.auth_in_flight.remove(&slot);
                    let account_key = account.to_ascii_lowercase();
                    if correct {
                        // No lockout: a right password always clears both windows immediately,
                        // whatever backoff either key had built up. See `admin::throttle`'s own
                        // top doc.
                        self.account_throttle.record_success(&account_key);
                        if let Some(ip_key) = &ip_key {
                            self.ip_throttle.record_success(ip_key);
                        }
                        self.admin.complete_sign_in(slot, &account);
                        let group = self.admin.group_of(slot).name.clone();
                        self.tell(slot, &format!("signed in as {account} ({group})."));
                        info!(slot, account, "signed in");
                        self.notify_update_if_pending(slot);
                    } else {
                        let now = std::time::Instant::now();
                        self.account_throttle.record_failure(&account_key, now);
                        if let Some(ip_key) = &ip_key {
                            self.ip_throttle.record_failure(ip_key, now);
                        }
                        // One message for both, so it does not say which accounts exist.
                        self.tell(slot, "that name and password do not go together.");
                    }
                }
            }
        }
    }

    /// Hands `slot` the pending update notice, if there is one and `slot` just signed in holding
    /// `panel.console` (or `*`) — "the first recognised admin who connects" from `update`'s own
    /// module doc, now spelled as "whoever holds the same standing power a raw console line does".
    /// `.take()` on the shared cell delivers it exactly once, to whoever this turns out to be;
    /// every sign-in after that finds the cell already empty and says nothing.
    fn notify_update_if_pending(&mut self, slot: u8) {
        if !self.admin.may(slot, crate::admin::perm::PANEL_CONSOLE) {
            return;
        }
        let Some(handle) = &self.update_notice else {
            return;
        };
        let message = handle.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(message) = message {
            self.tell(slot, &message);
        }
    }

    /// Announce a background save that has finished since the last tick.
    ///
    /// The writer runs on another thread and cannot reach the chat, so it posts its result down a
    /// channel and the game task collects it. Polled rather than awaited because the tick is not
    /// async and should not become so for this.
    ///
    /// Only worth saying out loud for a save somebody asked for: an autosave that works is not
    /// news, and one that fails is already in the log as an error.
    fn note_finished_save(&mut self) {
        let Ok(result) = self.save_results.1.try_recv() else {
            return;
        };
        let reason = self.save_reason;
        match result {
            Ok(ms) => {
                self.note_save_succeeded(reason);
                if reason == "command" {
                    self.announce(&format!("World saved ({ms} ms)."));
                }
            }
            Err(()) => self.note_save_failed(reason),
        }
    }

    fn handle_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::Join {
                addr,
                out,
                close,
                slot,
                queued,
            } => {
                let assigned = self.allocate_slot(addr, out, close, queued);
                let _ = slot.send(assigned);
            }
            ServerEvent::Packet { slot, epoch, frame } => {
                if self.slot_epoch_current(slot, epoch) {
                    self.handle_packet(slot, frame);
                } else {
                    debug!(
                        slot,
                        epoch, "dropping a ghost packet: its connection epoch is stale"
                    );
                }
            }
            ServerEvent::Leave { slot, epoch } => {
                if self.slot_epoch_current(slot, epoch) {
                    self.remove_player(slot);
                } else {
                    debug!(
                        slot,
                        epoch, "dropping a ghost leave: its connection epoch is stale"
                    );
                }
            }
            ServerEvent::Console { line } => self.run_console(&line),
            ServerEvent::ConsoleContext { reply } => {
                let players = self
                    .players
                    .iter()
                    .flatten()
                    .filter(|p| p.is_playing())
                    .map(|p| p.name.clone())
                    .collect();
                let groups = self.admin.groups.iter().map(|g| g.name.clone()).collect();
                let _ = reply.send(ConsoleContext { players, groups });
            }
            ServerEvent::PanelAuthLookup { name, reply } => {
                let hash_and_group = self.admin.account_hash_and_group(&name);
                let group_of = hash_and_group
                    .as_ref()
                    .and_then(|(_, group)| self.admin.groups.iter().find(|g| &g.name == group));
                let panel_view = group_of.is_some_and(|g| g.may(crate::admin::perm::PANEL_VIEW));
                let permissions = group_of
                    .map(|g| g.permissions.iter().cloned().collect())
                    .unwrap_or_default();
                let _ = reply.send(PanelAuthLookup {
                    unclaimed: self.admin.unclaimed(),
                    claim_token: self.claim_token.clone(),
                    hash_and_group,
                    panel_view,
                    permissions,
                });
            }
            ServerEvent::PanelInsertAccount { account, reply } => {
                // The claim path. Persist and retire the one-time token on success, exactly as the
                // console `claim` command does (`run_console`'s `"claim"` arm) — without the save, a
                // server claimed through the panel would forget its owner on the next restart (until
                // some later admin mutation happened to save), and the spent token would linger.
                let name = account.name.clone();
                let result = self.admin.insert_account(account);
                if result.is_ok() {
                    let _ = self.admin.save();
                    self.claim_token = None;
                    self.audit.record(
                        &name,
                        crate::admin::AuditAction::Claim,
                        &name,
                        "claimed from the web panel",
                    );
                }
                let _ = reply.send(result);
            }
            ServerEvent::PanelAuditThrottled { target, detail } => {
                self.audit.record(
                    "system",
                    crate::admin::AuditAction::Throttled,
                    &target,
                    &detail,
                );
            }
            ServerEvent::PanelStatus { reply } => {
                let player_count = self
                    .players
                    .iter()
                    .flatten()
                    .filter(|p| p.is_playing())
                    .count();
                let _ = reply.send(PanelStatus {
                    player_count,
                    max_players: self.config.max_players,
                    world_name: self.world.name.clone(),
                    world_file: self.current_world_file_stem(),
                    save_failures: self.save_failures,
                });
            }
            ServerEvent::PanelPlayers { reply } => {
                let _ = reply.send(self.panel_players());
            }
            ServerEvent::PanelKick {
                actor,
                name,
                reason,
                reply,
            } => {
                let Some(target) = self.slot_named(&name) else {
                    let _ = reply.send(Err(format!("nobody named {name} is connected")));
                    return;
                };
                let reason = if reason.trim().is_empty() {
                    "kicked from the web panel".to_string()
                } else {
                    reason
                };
                // The exact two calls `run_admin_command`'s own "kick" arm makes.
                self.announce(&format!("{name} was kicked: {reason}"));
                self.kick(target, &reason);
                self.audit
                    .record(&actor, crate::admin::AuditAction::Kick, &name, &reason);
                let _ = reply.send(Ok(()));
            }
            ServerEvent::PanelBan {
                actor,
                kind,
                value,
                reason,
                reply,
            } => {
                let reason = if reason.trim().is_empty() {
                    "banned from the web panel".to_string()
                } else {
                    reason
                };
                // The exact sequence `run_admin_command`'s own "ban" arm runs.
                self.admin.ban(kind.clone(), &value, &reason, &actor);
                self.announce(&format!("{value} is banned: {reason}"));
                if let Some(target) = self.slot_named(&value) {
                    self.kick(target, &reason);
                }
                self.audit.record(
                    &actor,
                    crate::admin::AuditAction::Ban,
                    &value,
                    &format!("{kind:?}: {reason}"),
                );
                info!(value, reason, "ban added from the web panel");
                let _ = reply.send(());
            }
            ServerEvent::PanelUnban {
                actor,
                value,
                reply,
            } => {
                let removed = self.admin.unban(&value);
                if removed > 0 {
                    self.audit
                        .record(&actor, crate::admin::AuditAction::Unban, &value, "");
                }
                let _ = reply.send(removed);
            }
            ServerEvent::PanelMute {
                actor,
                name,
                reason,
                duration_secs,
                reply,
            } => {
                let reason = if reason.trim().is_empty() {
                    "muted from the web panel".to_string()
                } else {
                    reason
                };
                self.admin.mute(&name, &reason, duration_secs, &actor);
                self.audit
                    .record(&actor, crate::admin::AuditAction::Mute, &name, &reason);
                info!(name, reason, "mute added from the web panel");
                let _ = reply.send(());
            }
            ServerEvent::PanelUnmute { actor, name, reply } => {
                let removed = self.admin.unmute(&name);
                if removed {
                    self.audit
                        .record(&actor, crate::admin::AuditAction::Unmute, &name, "");
                }
                let _ = reply.send(removed);
            }
            ServerEvent::PanelWhitelist { reply } => {
                let _ = reply.send(PanelWhitelist {
                    on: self.admin.whitelist_on(),
                    names: self.admin.whitelist.clone(),
                });
            }
            ServerEvent::PanelWhitelistAdd { actor, name, reply } => {
                let added = self.admin.add_to_whitelist(&name);
                if added {
                    let _ = self.admin.save();
                    self.audit
                        .record(&actor, crate::admin::AuditAction::Whitelist, &name, "added");
                }
                let _ = reply.send(added);
            }
            ServerEvent::PanelWhitelistRemove { actor, name, reply } => {
                let removed = self.admin.remove_from_whitelist(&name);
                if removed {
                    let _ = self.admin.save();
                    self.audit.record(
                        &actor,
                        crate::admin::AuditAction::Whitelist,
                        &name,
                        "removed",
                    );
                    // Take effect now rather than at their next join — mirrors the console's own
                    // `whitelist remove` arm.
                    if let Some(slot) = self.slot_named(&name) {
                        self.kick(slot, "You are no longer on this server's guest list.");
                    }
                }
                let _ = reply.send(removed);
            }
            ServerEvent::PanelWorldTiles { reply } => {
                let _ = reply.send(self.world_tile_sample());
            }
            ServerEvent::PanelConfigSnapshot { reply } => {
                let _ = reply.send(PanelConfigSnapshot {
                    listen: self.config.listen,
                    max_players: self.config.max_players,
                    world_width: self.world.width(),
                    world_height: self.world.height(),
                    motd: self.config.motd.clone(),
                    password_set: !self.config.password.is_empty(),
                    max_chat_len: self.config.max_chat_len,
                    idle_timeout_secs: self.config.idle_timeout_secs,
                    autosave_secs: self.config.autosave_secs,
                    save_target: self.save_path.as_ref().map(|p| p.display().to_string()),
                    whitelist_on: self.admin.whitelist_on(),
                    whitelist_count: self.admin.whitelist.len(),
                });
            }
            ServerEvent::PanelSetMotd { motd, reply } => {
                self.config.motd = motd;
                let _ = reply.send(());
            }
            ServerEvent::PanelSwitchWorld { path, reply } => {
                if !path.exists() {
                    let _ = reply.send(Err(format!("{} no longer exists", path.display())));
                    return;
                }
                self.announce("The server is restarting into a different world.");
                info!(path = %crate::worlds::display_path(&path), "world switch requested from the web panel");
                *self
                    .pending_world_switch
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Some(path);
                self.stopping = true;
                let _ = reply.send(Ok(()));
            }
            ServerEvent::PanelMetrics { reply } => {
                let _ = reply.send(self.panel_metrics());
            }
            ServerEvent::PanelBackups { reply } => {
                let _ = reply.send(self.panel_backups());
            }
            ServerEvent::PanelForceSave { reply } => {
                if self.save_path.is_none() {
                    let _ = reply.send(Err(
                        "this world is not being saved, so there is nothing to save".into(),
                    ));
                    return;
                }
                // The same background save the console `save` command runs — off the tick, one at a
                // time. The outcome reaches the operator on the live console feed, the same place
                // the console command's own "World saved" line goes.
                self.save_world_in_background("web panel");
                let _ = reply.send(Ok(()));
            }
            ServerEvent::PanelRollback { which, reply } => {
                let _ = reply.send(self.roll_back(which));
            }
            ServerEvent::PanelAccounts { reply } => {
                let _ = reply.send(self.panel_accounts());
            }
            ServerEvent::PanelSetAccountGroup {
                actor,
                name,
                group,
                reply,
            } => {
                let _ = reply.send(self.panel_set_account_group(&actor, &name, &group));
            }
            ServerEvent::PanelCreateAccount {
                actor,
                account,
                reply,
            } => {
                let _ = reply.send(self.panel_create_account(&actor, account));
            }
            ServerEvent::PanelDeleteAccount { actor, name, reply } => {
                let _ = reply.send(self.panel_delete_account(&actor, &name));
            }
            ServerEvent::PanelAuditPath { reply } => {
                let _ = reply.send(self.audit.path().map(std::path::Path::to_path_buf));
            }
            ServerEvent::PanelAuthorize {
                name,
                permission,
                reply,
            } => {
                let allowed = self
                    .admin
                    .account_hash_and_group(&name)
                    .is_some_and(|(_, group)| self.admin.group_grants_str(&group, &permission));
                let _ = reply.send(allowed);
            }
            ServerEvent::PanelSetGroupPermission {
                actor,
                group,
                permission,
                grant,
                reply,
            } => {
                let _ =
                    reply.send(self.panel_set_group_permission(&actor, &group, &permission, grant));
            }
        }
    }

    // ---------------------------------------------------------------- players

    fn allocate_slot(
        &mut self,
        addr: SocketAddr,
        out: mpsc::Sender<Bytes>,
        close: crate::net::connection::Closer,
        queued: crate::net::connection::QueuedBytes,
    ) -> Option<(u8, u32)> {
        let slot = self.players.iter().position(Option::is_none)?;
        let slot = u8::try_from(slot).ok()?;
        let mut player = Player::new(slot, addr, out);
        player.queued = queued;
        player.close = Some(close);
        self.players[slot as usize] = Some(player);
        // Wrapping, not saturating: a slot would need to cycle through u32::MAX connections
        // inside one process lifetime before this repeats, and if it somehow did, the result is
        // only ever this same race reopening for one ghost - never worse than the bug this
        // exists to fix - so an unbounded counter would buy nothing a real server could spend.
        let epoch = self.slot_epochs[slot as usize].wrapping_add(1);
        self.slot_epochs[slot as usize] = epoch;
        debug!(%addr, slot, epoch, "connection accepted into a slot");
        Some((slot, epoch))
    }

    /// Whether `epoch` is still what [`Self::allocate_slot`] most recently handed out for `slot`
    /// - see [`Self::remove_player`]'s doc comment for what this tells apart.
    fn slot_epoch_current(&self, slot: u8, epoch: u32) -> bool {
        self.slot_epochs.get(slot as usize) == Some(&epoch)
    }

    fn player(&self, slot: u8) -> Option<&Player> {
        self.players.get(slot as usize)?.as_ref()
    }

    fn player_mut(&mut self, slot: u8) -> Option<&mut Player> {
        self.players.get_mut(slot as usize)?.as_mut()
    }

    /// Take a player out of their slot.
    ///
    /// **A former known gap, closed by the epoch check below - recorded here rather than left to
    /// be rediscovered if it ever regresses.** Removing a player does not end their connection's
    /// read task. Dropping the [`Player`] drops the last sender for its outbound queue, which ends
    /// `write_loop` and shuts down the *write* half of the socket - but `read_loop` keeps going
    /// until the client closes or `idle_timeout_secs` expires, and keeps forwarding
    /// `ServerEvent::Packet { slot, .. }` and, finally, `ServerEvent::Leave { slot, .. }` for a
    /// slot the game no longer associates with it.
    ///
    /// While the slot stays empty that is harmless: every handler goes through
    /// [`Self::player_mut`], which returns `None`. The danger was that [`Self::allocate_slot`]
    /// hands out the first free slot, so a newcomer arriving inside that window would inherit the
    /// number - and then the ghost connection's packets would be attributed to them, and its
    /// eventual `Leave` would remove them.
    ///
    /// This is not new and is not specific to any one caller. Three paths reach here without the
    /// connection having ended: `/kick`, [`crate::game::server::GameServer::reap_stalled_handshakes`],
    /// and - reachable by any client with no privilege at all - [`Self::send_bytes`] dropping a
    /// player whose outbound queue filled up.
    ///
    /// **The fix: a per-connection epoch.** [`Self::allocate_slot`] hands back `(slot, epoch)` and
    /// bumps `slot_epochs[slot]` every time it hands the slot to somebody new; `remove_player`
    /// itself never touches `slot_epochs` - only a fresh allocation does. `net/connection.rs`'s
    /// `serve` learns its epoch from that reply and stamps it onto every `ServerEvent::Packet` it
    /// forwards and onto its own eventual `ServerEvent::Leave` (the only three places that build
    /// these events); `handle_event` drops anything whose epoch does not match
    /// [`Self::slot_epoch_current`] and logs it as a ghost at debug level rather than acting on
    /// it. So while the slot sits empty a ghost's events remain exactly as harmless as they always
    /// were - `slot_epochs[slot]` has not moved - and only start being discarded once somebody new
    /// actually occupies the slot, which is the one moment they were ever wrong.
    fn remove_player(&mut self, slot: u8) {
        self.disconnect(slot, None);
    }

    /// [`Self::remove_player`], plus one last frame to put on the wire on the way out.
    ///
    /// The farewell does not go through the outbound queue, and that is the point. Everything
    /// still queued for a connection the server has just decided to end is backlog from before
    /// that decision - up to about a million frames on a full server - and a kick notice appended
    /// to the end of it arrives at the far side of however long that takes to read, if the client
    /// has not given up by then. It rides [`crate::net::connection::Closer`] instead, which ends
    /// the connection *now* and writes this one frame first. See that type's doc comment.
    fn disconnect(&mut self, slot: u8, farewell: Option<Bytes>) {
        // Before the early return, not after it. A client that disconnects mid-check would
        // otherwise hold one of the server's few hashing slots until its worker happened to
        // finish — and one doing it deliberately, repeatedly, would hold all of them.
        self.auth_in_flight.remove(&slot);

        let Some(mut player) = self.players.get_mut(slot as usize).and_then(Option::take) else {
            return;
        };
        // Whatever is still queued for this connection will never be written now, so it must stop
        // counting against the server's budget. Without this, every shed leaks its own backlog into
        // the total for the life of the process and the budget ratchets shut: the second shed comes
        // sooner than the first, and eventually a healthy server sheds on its first frame.
        player.queued.abandon();
        // Fired here, with the player already out of the slot, so that nothing this function goes
        // on to broadcast can be queued for a connection that is no longer taking any.
        if let Some(close) = player.close.take() {
            let _ = close.send(farewell);
        }
        info!(slot, name = %player.name, "player disconnected");
        // A session is not state: whoever reuses this slot starts as nobody.
        self.admin.sign_out(slot);

        // Neither is a run of withheld updates. Slots are reused, so leaving these behind would
        // hand the next player to occupy this one a head start on the skip budget, and start them
        // off missing updates they should have had. Both directions go: what this player was owed,
        // and what was being withheld about them from everybody else.
        self.skips
            .retain(|&(what, target), _| target != slot && what != Withheld::Player(slot));

        // Whatever they had open is free again. Without this a mannequin somebody was looking at
        // when their connection dropped stays locked for the rest of the world's life.
        if self.tile_entity_anchors.remove(&slot).is_some()
            && let Ok(frame) = {
                let mut w = terrustia_proto::PacketWriter::new(id::REQUEST_TILE_ENTITY_INTERACTION);
                w.i32(NO_TILE_ENTITY).u8(slot);
                w.finish()
            }
        {
            self.broadcast(frame, Some(slot));
        }

        if player.state == ConnState::Playing {
            if let Ok(frame) = packets::player_active(slot, false) {
                self.broadcast(frame, Some(slot));
            }
            // `LegacyMultiplayer.20` is `"{0} has left."`, with the name as its one argument.
            let who = NetworkText::literal(&player.name);
            self.announce_key("LegacyMultiplayer.20", vec![who]);
        }
    }

    /// Queue a frame for one player, dropping the connection if its queue has backed up.
    fn send(&mut self, slot: u8, frame: Vec<u8>) {
        self.send_bytes(slot, Bytes::from(frame));
    }

    fn send_bytes(&mut self, slot: u8, frame: Bytes) {
        let Some((out, queued)) = self.player(slot).map(|p| (p.out.clone(), p.queued.clone()))
        else {
            return;
        };
        let size = frame.len();
        // A client that cannot keep up would otherwise grow the queue without bound. Dropping it is
        // the same call vanilla makes, and the read task notices the closed channel.
        //
        // A *closed* channel is a different thing and must not be reported as the same: it means
        // the connection has already gone, which happens every time anybody leaves — and every
        // player still on the server is then sent the news, one send per departed connection.
        // Calling that "dropping connection" at warning level sends an operator looking for a
        // network problem that is not there.
        let id = frame.get(2).copied().unwrap_or(255);
        queued.reserve(size);
        match out.try_send(frame) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => {
                queued.release(size);
                debug!(slot, "sending to a connection that has already gone");
                self.remove_player(slot);
                return;
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                queued.release(size);
                warn!(
                    slot,
                    packet = id,
                    name = terrustia_proto::id::name(id),
                    "outbound queue full; dropping a client that cannot keep up"
                );
                self.remove_player(slot);
                return;
            }
        }
        // One connection's own queue filling is the case above. This is the other one, which
        // nothing bounded until now: every queue individually inside its depth, and the *sum* of
        // them past what the server can afford to hold. One atomic load on the common path.
        if queued.total() > crate::net::connection::OUTBOUND_TOTAL_BUDGET {
            self.shed_the_deepest_queue();
        }
    }

    /// Drop whichever connection is holding the most unread outbound bytes.
    ///
    /// Reached only when the server-wide backlog is over
    /// [`crate::net::connection::OUTBOUND_TOTAL_BUDGET`], so walking the players is affordable
    /// here in a way it would not be on every send.
    ///
    /// The deepest queue rather than the connection that happened to trigger the check: the two are
    /// usually not the same, and shedding the trigger would punish whichever player the game task
    /// spoke to next rather than the one actually holding the memory. This is the aggregate
    /// counterpart of the per-connection shed above, and it answers the same way, because a
    /// connection sitting on a large share of a quarter-gigabyte backlog is by definition one that
    /// is not reading.
    fn shed_the_deepest_queue(&mut self) {
        let Some((slot, held)) = self
            .players
            .iter()
            .flatten()
            .map(|p| (p.slot, p.queued.mine()))
            .max_by_key(|(_, held)| *held)
        else {
            return;
        };
        warn!(
            slot,
            held_bytes = held,
            total_bytes = self
                .players
                .iter()
                .flatten()
                .map(|p| p.queued.mine())
                .sum::<usize>(),
            budget_bytes = crate::net::connection::OUTBOUND_TOTAL_BUDGET,
            "the server's outbound queues are over budget between them; dropping the connection \
             holding the most of it"
        );
        self.remove_player(slot);
    }

    /// Broadcast a tile square to the clients that actually hold the ground it covers.
    ///
    /// Vanilla gates packet 20 and nothing else this way (`NetMessage.cs:1721-1731`): its case 20
    /// loop adds `Netplay.Clients[i].SectionRange((int)Math.Max(number3, number4), number,
    /// (int)number2)` to the usual connected-and-broadcasting test, so a square only reaches a
    /// client one of whose sections it touches. `RemoteClient.SectionRange`
    /// (`RemoteClient.cs:192-215`) tests the square's four corners and returns true on the first
    /// one whose section that client has been sent.
    ///
    /// We were sending every square to every player. On a full server that is the same rectangle
    /// of tiles delivered to people who cannot see it and, worse, who have never been sent the
    /// section it patches, so the packet describes ground their client does not have. Grass
    /// spreading in one corner of the world was costing bandwidth in every other.
    ///
    /// `sent_sections` is our mirror of `TileSections`, already maintained by `send_section`.
    fn broadcast_tile_square(&mut self, square: &TileSquare, except: Option<u8>) {
        let Ok(frame) = square.encode() else {
            return;
        };
        let (x, y) = (i32::from(square.x), i32::from(square.y));
        // `Math.Max(number3, number4)` at the call site, where those are the square's width and
        // height. One size for both axes, which is what `SectionRange` expects.
        let size = i32::from(square.width.max(square.height));
        let bytes = Bytes::from(frame);
        let mut targets = std::mem::take(&mut self.broadcast_targets);
        targets.clear();
        targets.extend(
            self.players
                .iter()
                .flatten()
                .filter(|p| {
                    p.is_playing()
                        && Some(p.slot) != except
                        && self.square_in_section_range(p, x, y, size)
                })
                .map(|p| p.slot),
        );
        for slot in &targets {
            self.send_bytes(*slot, bytes.clone());
        }
        self.broadcast_targets = targets;
    }

    /// `RemoteClient.SectionRange`, `RemoteClient.cs:192-215`: the four corners of a `size`-square
    /// at `(x, y)`, true as soon as one of them lands in a section this client already holds.
    ///
    /// Vanilla passes one `size` for both axes (`Math.Max(width, height)` at the call site), so
    /// the corners it tests are `(x, y)`, `(x + size, y)`, `(x, y + size)` and `(x + size,
    /// y + size)`. Transcribed with that same single size rather than the square's real width and
    /// height, because the whole point of this check is to match who vanilla sends to.
    fn square_in_section_range(
        &self,
        player: &crate::game::player::Player,
        x: i32,
        y: i32,
        size: i32,
    ) -> bool {
        [(x, y), (x + size, y), (x, y + size), (x + size, y + size)]
            .into_iter()
            .any(|(cx, cy)| {
                player
                    .sent_sections
                    .contains(&self.world.section_of(cx, cy))
            })
    }

    /// Send to every player who is in the world, optionally skipping one.
    fn broadcast(&mut self, frame: Vec<u8>, except: Option<u8>) {
        let bytes = Bytes::from(frame);
        // Collect first: sending can remove a player, which would invalidate an in-flight iterator.
        // The buffer is taken from the server rather than allocated, so a broadcast under load does
        // not also cost an allocation. Taking it (rather than borrowing) keeps `send_bytes`'s own
        // `&mut self` free, and leaves re-entrant broadcasts (a send that removes a player, which
        // announces the departure) correct: the inner call finds an empty buffer, allocates its
        // own, and each level restores what it took.
        let mut targets = std::mem::take(&mut self.broadcast_targets);
        targets.clear();
        targets.extend(
            self.players
                .iter()
                .flatten()
                .filter(|p| p.is_playing() && Some(p.slot) != except)
                .map(|p| p.slot),
        );
        for slot in &targets {
            self.send_bytes(*slot, bytes.clone());
        }
        self.broadcast_targets = targets;
    }

    fn announce(&mut self, text: &str) {
        // This always reaches players as real in-game chat (`chat_broadcast`, just below) — a
        // kick notice, a save confirmation, `say`'s own text — so it is exactly as much "chat" for
        // the panel's live feed as a player's own line is, tagged the same way.
        info!(target: crate::term::CHAT_TARGET, "{text}");
        if let Ok(frame) = net_module::chat_broadcast(
            net_module::SERVER_AUTHOR,
            &NetworkText::literal(text),
            SERVER_CHAT_COLOUR,
        ) {
            self.broadcast(frame, None);
        }
    }

    /// Announce something the *game* says, by its localization key.
    ///
    /// Vanilla does not send these as text. It sends a key and its arguments — `NPC.cs:81383` is
    /// `NetworkText.FromKey("Announcement.HasAwoken", ...)` against
    /// `"HasAwoken": "{0} has awoken!"` — and the client renders it in whatever language that
    /// player is playing in. Sending the English instead was wrong three times over: different
    /// bytes on the wire than the real server, English for every non-English player, and verbatim
    /// game text compiled into this repository.
    ///
    /// Only for lines the game itself has a key for. Anything this server says on its own behalf —
    /// save reports, kick reasons, console replies — stays [`Self::announce`], because there is no
    /// key for it and inventing one would render as nothing at all.
    fn announce_key(&mut self, key: &str, args: Vec<NetworkText>) {
        // The log is for whoever is running the server, so it stays readable English.
        info!(key, "announcing");
        if let Ok(frame) = net_module::chat_broadcast(
            net_module::SERVER_AUTHOR,
            &NetworkText::key(key, args),
            SERVER_CHAT_COLOUR,
        ) {
            self.broadcast(frame, None);
        }
    }

    fn kick(&mut self, slot: u8, reason: &str) {
        debug!(slot, reason, "kicking");
        let notice = packets::kick(&NetworkText::literal(reason))
            .ok()
            .map(Bytes::from);
        self.disconnect(slot, notice);
    }
}

/// What a broken tile gives back.
///
/// Two tables, and the order matters. [`tile_drop`] is the game's own statement of what *mining*
/// a block gives, and it wins wherever it has an answer — mining grass gives dirt even though
/// grass seeds are what placed it. Only where it deliberately has none, which is every framed
/// object, does the style get worked out and the placing item looked up.
fn drop_of(tile: u16, frame_x: i16, frame_y: i16) -> Option<i32> {
    if let Some(item) = tile_drop(tile) {
        return Some(item);
    }
    let object = terrustia_proto::tile_object::tile_object(tile)?;
    // The frame given is of whichever cell the player hit; the style is read off the corner.
    let (dx, dy) = terrustia_proto::tile_object::origin_of(tile, frame_x, frame_y)?;
    let corner_x = frame_x - (dx * (object.coord_width + object.padding)) as i16;
    let corner_y = frame_y
        - object.coord_heights[..dy as usize]
            .iter()
            .map(|h| h + object.padding)
            .sum::<i32>() as i16;
    let style = object.style_of(corner_x, corner_y);
    terrustia_proto::placed_items::placed_item(tile, style)
}

/// Lets the AI read world tiles without borrowing the whole server.
struct WorldTiles<'a>(&'a World);

impl TileView for WorldTiles<'_> {
    fn tile(&self, x: i32, y: i32) -> Tile {
        self.0.tile(x, y)
    }
}

/// Tree tile types that share vanilla's own `KillTile_GetTreeDrops` branch (`WorldGen.cs`, `case
/// 5: case 596: case 616: case 634:`). Only 5 (ordinary trees) is ever worldgen-placed by this
/// project today (`world::trees`); the vanity-tree and ash-tree variants are included anyway
/// since it costs nothing and the item->tile placement table already knows their ids.
const TREE_TILES: [u16; 4] = [5, 596, 616, 634];

fn is_tree_tile(block: u16) -> bool {
    TREE_TILES.contains(&block)
}

/// `WorldGen.GetTreeType`'s own enum, narrowed to the species this project's ground types can
/// actually produce (see [`GameServer::tree_species_at`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum TreeSpecies {
    None,
    Forest,
    Corrupt,
    Crimson,
    Jungle,
    Hallowed,
    Snow,
    Mushroom,
}

/// `WorldGen.TreeTypeDropsAcorns`: every species drops an acorn from its canopy except a tree with
/// no resolved species, and the two whose canopy drops something else instead (a mushroom cap, a
/// Rich Mahogany sapling players plant deliberately rather than find as a bonus).
fn tree_drops_acorns(species: TreeSpecies) -> bool {
    !matches!(
        species,
        TreeSpecies::None | TreeSpecies::Mushroom | TreeSpecies::Jungle
    )
}

/// Item ids for tree drops (`ItemID.cs`): plain Wood, the six biome-specific woods, an Acorn, and
/// what a Mushroom-biome tree gives instead of wood.
const WOOD: i32 = 9;
const ACORN: i32 = 27;
const EBONWOOD: i32 = 619;
const RICH_MAHOGANY: i32 = 620;
const PEARLWOOD: i32 = 621;
const SHADEWOOD: i32 = 911;
const GLOWING_MUSHROOM: i32 = 183;
const BOREAL_WOOD: i32 = 2503;

/// The demon altar, which is a crafting station before hardmode and an ore mine after it.
const DEMON_ALTAR: u16 = 26;
/// A bee larva in a hive, which is the second way to reach the Queen.
const BEE_LARVA: u16 = 231;
/// The two bosses that have no summon item at all: breaking their tile is the only way in.
const PLANTERA: u16 = 262;
const QUEEN_BEE: u16 = 222;

/// How far a roar carries, and how long what it leaves behind lasts.
const ROAR_REACH: f32 = 800.0;
const ROAR_SLOW_TICKS: i32 = 720;
/// The Slow debuff, which is what a roar leaves.
const BUFF_SLOW: u16 = 32;

/// How far a Dark Mage looks for something worth healing, and for a corpse worth raising.
const HEAL_REACH: (f32, f32) = terrustia_proto::npc_params::DARK_MAGE_HEAL_RANGE;
const RAISE_CHECK_RANGE: f32 = terrustia_proto::npc_params::RAISE_CHECK_RANGE;
const RAISE_MINIMUM: usize = terrustia_proto::npc_params::RAISE_MINIMUM;

/// Does a panic on the untrusted-packet path actually get caught, and does the world still reach
/// disk on the way out?
///
/// `catch_unwind` used to wrap only `tick()`. `handle_event` — which is where every byte from
/// every client is decoded — was bare, so a panic under it unwound out of the loop, past the
/// shutdown save at the bottom of `run`, and the process still exited zero.
#[cfg(test)]
mod panic_path {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "panic probe")
    }

    #[tokio::test]
    async fn a_panic_handling_an_event_is_caught_and_reported() {
        let (tx, rx) = mpsc::channel::<ServerEvent>(4);
        let server = GameServer::new(Config::default(), tiny_world());

        tx.send(ServerEvent::Console {
            line: "__panic_probe".into(),
        })
        .await
        .expect("the game task should still be listening");

        // The panic is deliberate; let it not spew a backtrace over the test output.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = server.run(rx).await;
        std::panic::set_hook(hook);

        assert_eq!(
            outcome,
            Stopped::Panicked,
            "a panic on the packet path must be caught and reported, not unwind the task"
        );
    }

    /// Halloween and Christmas come off the wall clock, and the server has to actually go and look.
    ///
    /// Both tests hand the server a world that arrives *claiming* it is Halloween, which nothing
    /// legitimate ever does (neither flag is saved: see `World`'s own persistence audit), and check
    /// that the claim is overwritten by what the calendar really says. Written that way on purpose,
    /// so the test means something on the three hundred and thirty days of the year when the honest
    /// answer is `false` and a missing call would look identical to a working one.
    ///
    /// Neutralised by deleting `world.refresh_calendar()` from `GameServer::new`: the first fails.
    /// Neutralised by deleting it from the tick's dawn block: the second fails.
    #[test]
    fn booting_re_reads_the_calendar_rather_than_trusting_the_world() {
        use crate::world::calendar;

        let mut world = tiny_world();
        world.halloween = true;
        world.xmas = true;
        let server = GameServer::new(Config::default(), world);

        let (month, day) = calendar::today();
        assert_eq!(server.world.halloween, calendar::is_halloween(month, day));
        assert_eq!(server.world.xmas, calendar::is_xmas(month, day));
    }

    /// ...and again at every dawn, where `Main.cs:66375-66376` reads it, so a server left up across
    /// the ninth of October starts spawning the Halloween roster without a restart.
    #[test]
    fn dawn_re_reads_the_calendar() {
        use crate::world::calendar;
        use crate::world::world::NIGHT_LENGTH;

        let mut server = GameServer::new(Config::default(), tiny_world());
        server.world.day_time = false;
        server.world.time = NIGHT_LENGTH - 1;
        server.world.halloween = true;
        server.world.xmas = true;

        server.tick();
        assert!(server.world.day_time, "the tick should have reached dawn");

        let (month, day) = calendar::today();
        assert_eq!(server.world.halloween, calendar::is_halloween(month, day));
        assert_eq!(server.world.xmas, calendar::is_xmas(month, day));
    }

    /// The ordinary case still reports a clean stop, so the exit code stays meaningful.
    #[tokio::test]
    async fn a_normal_shutdown_reports_cleanly() {
        let (tx, rx) = mpsc::channel::<ServerEvent>(4);
        let server = GameServer::new(Config::default(), tiny_world());
        drop(tx);
        assert_eq!(server.run(rx).await, Stopped::Cleanly);
    }
}

/// Can a connected client stall the world by asking to register?
///
/// It could. `/register` needs no permission — it falls through the `needed` match to `None` — and
/// it used to run Argon2 inline on the game task. Argon2 is deliberately expensive: tens of
/// milliseconds against a 16.67 ms tick, so a client sending `/register` in a loop froze the world
/// for everybody. The hashing now happens on a worker thread, with a per-slot lock and a
/// server-wide ceiling so the queue cannot be grown either.
#[cfg(test)]
mod auth_cost {
    use super::*;
    use crate::config::Config;
    use std::time::{Duration, Instant};

    /// What one hash actually costs, measured rather than assumed, so the margin below is real.
    #[test]
    fn a_single_hash_is_far_more_than_a_tick() {
        let started = Instant::now();
        crate::admin::Account::new("probe", "a long enough password", "default").expect("hashing");
        let cost = started.elapsed();
        assert!(
            cost > Duration::from_millis(1),
            "argon2 finished in {cost:?}; if it is really this cheap the ceiling below is \
             meaningless and this whole guard needs rethinking"
        );
    }

    #[tokio::test]
    async fn a_burst_of_registrations_does_not_cost_the_tick() {
        let mut server = GameServer::new(Config::default(), World::empty(200, 150, "auth"));

        // Far more than the ceiling, from one slot and then from many.
        let started = Instant::now();
        for i in 0..64u8 {
            let _ = server.run_admin_command(
                i % 8,
                "register",
                &format!("account{i} a_long_enough_password"),
            );
        }
        let elapsed = started.elapsed();

        // Inline, sixty-four hashes would be well over a second. Deferred, this is bookkeeping.
        assert!(
            elapsed < Duration::from_millis(100),
            "sixty-four registrations took {elapsed:?} on the game task; they are being hashed \
             inline again, which is a denial of service any connected client can trigger"
        );
    }

    #[tokio::test]
    async fn one_slot_gets_one_hash_at_a_time_and_the_server_has_a_ceiling() {
        let mut server = GameServer::new(Config::default(), World::empty(200, 150, "auth"));

        assert!(server.start_auth(3), "the first is allowed");
        assert!(!server.start_auth(3), "a second from the same slot is not");

        // Fill the rest of the server-wide ceiling from other slots.
        for slot in 0..(MAX_AUTH_IN_FLIGHT as u8 - 1) {
            assert!(server.start_auth(slot), "slot {slot} within the ceiling");
        }
        assert!(
            !server.start_auth(200),
            "past the ceiling, nobody else gets one however many slots are asking"
        );
    }

    /// Leaving mid-check must not hold a hashing slot open.
    #[tokio::test]
    async fn disconnecting_releases_the_hashing_slot() {
        let mut server = GameServer::new(Config::default(), World::empty(200, 150, "auth"));
        assert!(server.start_auth(5));
        server.remove_player(5);
        assert!(
            server.start_auth(5),
            "a slot freed by a disconnect must be usable again, or repeated joins exhaust the pool"
        );
    }
}

/// The race `remove_player`'s doc comment records, and the epoch check that closes it: a
/// connection's read task outlives its player being removed, and can go on forwarding
/// `ServerEvent::Packet`/`ServerEvent::Leave` for a slot number a newcomer has since taken.
/// `allocate_slot` bumps `slot_epochs[slot]` on every fresh assignment; `handle_event` compares
/// the epoch stamped on an incoming `Packet`/`Leave` against it and drops anything stale.
#[cfg(test)]
mod ghost_connection_epoch {
    use super::*;
    use crate::config::Config;

    fn tiny_world(name: &str) -> World {
        World::empty(200, 150, name)
    }

    /// The exact scenario from `remove_player`'s doc comment: a player is removed (here standing
    /// in for `/kick`, the handshake reaper, or `send_bytes` dropping a backed-up client - all
    /// three reach `remove_player` without the connection itself having ended), a newcomer is
    /// handed the same slot number, and only then does the ghost's stale `Leave` arrive. Before
    /// the epoch check this evicted the newcomer; the newcomer's own connection never sent it.
    /// Two connections sharing one budget, one of them holding nearly all of it. A frame sent to
    /// the *other* one takes the total past the ceiling, and the server must shed the holder
    /// rather than the sender: they are usually not the same connection, and shedding the sender
    /// would drop whichever player the game task happened to speak to next.
    ///
    /// The bytes here are counted, not allocated - `reserve` moves two atomics - so a test can
    /// stand at the edge of a quarter-gigabyte ceiling for free.
    #[tokio::test]
    async fn the_aggregate_budget_sheds_the_deepest_queue_not_the_sender() {
        use crate::net::connection::{OUTBOUND_TOTAL_BUDGET, QueuedBytes};

        let mut server = GameServer::new(Config::default(), tiny_world("aggregate budget"));
        let total = std::sync::Arc::<std::sync::atomic::AtomicUsize>::default();

        let (tx_hog, _rx_hog) = mpsc::channel(16);
        let (hog, _) = server
            .allocate_slot(
                "127.0.0.1:6200".parse().expect("a literal"),
                tx_hog,
                oneshot::channel().0,
                QueuedBytes::new(total.clone()),
            )
            .expect("a free slot");
        let (tx_quiet, _rx_quiet) = mpsc::channel(16);
        let (quiet, _) = server
            .allocate_slot(
                "127.0.0.1:6201".parse().expect("a literal"),
                tx_quiet,
                oneshot::channel().0,
                QueuedBytes::new(total.clone()),
            )
            .expect("a second free slot");

        // The hog is sitting on the whole budget bar a byte: still inside it, so nothing has
        // fired yet, and its own queue is nowhere near its depth limit.
        server
            .player(hog)
            .expect("the hog is seated")
            .queued
            .reserve(OUTBOUND_TOTAL_BUDGET - 1);
        assert!(
            server.player(hog).is_some() && server.player(quiet).is_some(),
            "at the ceiling and not over it, nobody may be shed"
        );

        server.send_bytes(quiet, Bytes::from_static(&[0, 0, 0, 0]));

        assert!(
            server.player(hog).is_none(),
            "the connection holding the backlog is the one that must go"
        );
        assert!(
            server.player(quiet).is_some(),
            "the connection that merely happened to be sent to must survive"
        );
        assert_eq!(
            total.load(std::sync::atomic::Ordering::Relaxed),
            4,
            "shedding must hand the shed connection's bytes back, leaving only the frame that is \
             genuinely still queued; a shed that leaks its own backlog ratchets the budget shut"
        );
    }

    #[tokio::test]
    async fn a_ghost_leave_does_not_evict_the_slots_new_occupant() {
        let mut server = GameServer::new(Config::default(), tiny_world("ghost leave probe"));

        let (tx1, _rx1) = mpsc::channel(16);
        let (old_slot, old_epoch) = server
            .allocate_slot(
                "127.0.0.1:6000".parse().expect("a literal"),
                tx1,
                oneshot::channel().0,
                crate::net::connection::QueuedBytes::default(),
            )
            .expect("a free slot");
        // Stands in for `/kick`, the handshake reaper, or a full outbound queue: the player is
        // gone, but nothing told the ghost's `read_loop` that.
        server.remove_player(old_slot);

        let (tx2, _rx2) = mpsc::channel(16);
        let (new_slot, new_epoch) = server
            .allocate_slot(
                "127.0.0.1:6001".parse().expect("a literal"),
                tx2,
                oneshot::channel().0,
                crate::net::connection::QueuedBytes::default(),
            )
            .expect("the freed slot must be available again");
        assert_eq!(
            old_slot, new_slot,
            "the newcomer must land on the same recycled slot number for this to be the race"
        );
        assert_ne!(
            old_epoch, new_epoch,
            "a fresh allocation must bump the slot's epoch, or there is nothing to check"
        );
        server
            .player_mut(new_slot)
            .expect("the newcomer is seated")
            .name = "Newcomer".into();

        // The ghost's read task, having no idea any of this happened, finally reports the
        // disconnect it saw a while ago, stamped with the epoch it was handed at `Join`.
        server.handle_event(ServerEvent::Leave {
            slot: old_slot,
            epoch: old_epoch,
        });

        assert!(
            server.players[new_slot as usize].is_some(),
            "a ghost Leave carrying a stale epoch must not evict whoever actually holds the slot \
             now"
        );
        assert_eq!(
            server.player(new_slot).expect("still seated").name,
            "Newcomer",
            "the newcomer specifically must still be there, not merely somebody"
        );
    }

    /// The other half of the same race: not just the ghost's final `Leave`, but every `Packet` it
    /// forwards in between also has to be told apart from a real one, or the newcomer's own state
    /// gets mutated by a connection that is not theirs.
    #[tokio::test]
    async fn a_ghost_packet_with_a_stale_epoch_is_discarded_not_dispatched() {
        let mut server = GameServer::new(Config::default(), tiny_world("ghost packet probe"));

        let (tx1, _rx1) = mpsc::channel(16);
        let (old_slot, old_epoch) = server
            .allocate_slot(
                "127.0.0.1:6100".parse().expect("a literal"),
                tx1,
                oneshot::channel().0,
                crate::net::connection::QueuedBytes::default(),
            )
            .expect("a free slot");
        server.remove_player(old_slot);

        let (tx2, _rx2) = mpsc::channel(16);
        let (new_slot, new_epoch) = server
            .allocate_slot(
                "127.0.0.1:6101".parse().expect("a literal"),
                tx2,
                oneshot::channel().0,
                crate::net::connection::QueuedBytes::default(),
            )
            .expect("the freed slot must be available again");
        assert_eq!(old_slot, new_slot);
        assert_ne!(old_epoch, new_epoch);
        assert!(
            !server.player(new_slot).expect("the newcomer is seated").pvp,
            "a fresh player must start without pvp, or the toggle below proves nothing"
        );

        // TOGGLE_P_V_P's payload is `[slot][hostile bool]` (`dispatch::on_pvp`); it flips `pvp`
        // on whoever `slot` resolves to right now, which is exactly the attribution bug: a
        // handler has no way to tell a ghost's packet apart from the real occupant's own.
        let payload = Bytes::from(vec![new_slot, 1u8]);
        server.handle_event(ServerEvent::Packet {
            slot: old_slot,
            epoch: old_epoch,
            frame: Frame {
                id: id::TOGGLE_P_V_P,
                payload,
            },
        });

        assert!(
            !server.player(new_slot).expect("still seated").pvp,
            "a ghost packet carrying a stale epoch must not be dispatched onto whoever holds the \
             slot now"
        );
    }
}

/// Lane F: the claim-token compare (`console::run_admin_command`'s `"register"` arm) goes through
/// the shared `admin::constant_time_eq` rather than a plain `!=`, and `/login` is backed off by
/// `admin::throttle` per address and per account. This covers both from the in-game/chat side; the
/// panel's own `/api/login` versions of the same two things are covered end-to-end, over a real
/// socket, in `tests/panel.rs`.
#[cfg(test)]
mod claim_token_and_login_throttle {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> World {
        World::empty(200, 150, "claim token / login throttle probe")
    }

    /// A real connected slot, so `ip_key` (`self.player(slot).map(|p| p.addr.ip()...)`) has an
    /// address to key its half of the throttle on, exactly as a real join would.
    fn with_player(mut server: GameServer, slot: u8, addr: &str) -> GameServer {
        let (out_tx, _out_rx) = mpsc::channel(64);
        let player = Player::new(slot, addr.parse().expect("valid test address"), out_tx);
        server.players[usize::from(slot)] = Some(player);
        server
    }

    /// `begin_registration`/`/login`'s hash always runs on a real worker thread and reports back
    /// through `auth_results`, which `note_finished_auth` only ever drains on a tick: there is no
    /// synchronous point to await here, so this polls the way `tests/panel.rs`'s own `wait_until`
    /// does, on a deadline rather than a fixed sleep.
    async fn wait_until(
        server: &mut GameServer,
        deadline: std::time::Duration,
        mut done: impl FnMut(&GameServer) -> bool,
    ) -> bool {
        let start = std::time::Instant::now();
        loop {
            server.note_finished_auth();
            if done(server) {
                return true;
            }
            if start.elapsed() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// Fail-then-pass for the constant-time compare itself: a wrong token must never be able to
    /// claim the server (and must not even start a hash: the whole point of checking the token
    /// first), and the real one must.
    #[tokio::test]
    async fn the_claim_token_compare_rejects_wrong_and_accepts_right() {
        let mut server = with_player(
            GameServer::new(Config::default(), tiny_world()),
            0,
            "127.0.0.1:40001",
        );
        server.claim_token = Some("the-real-token".to_string());

        server
            .run_admin_command(0, "register", "owner ownerpassword the-wrong-token")
            .unwrap();
        assert!(
            server.admin.unclaimed(),
            "a wrong claim token must never claim the server"
        );
        assert!(
            server.auth_in_flight.is_empty(),
            "a refused token must be caught before any hash ever starts"
        );

        server
            .run_admin_command(0, "register", "owner ownerpassword the-real-token")
            .unwrap();
        assert!(
            wait_until(&mut server, std::time::Duration::from_secs(2), |s| {
                !s.admin.unclaimed()
            })
            .await,
            "the real claim token should claim the server"
        );
    }

    /// Fail-then-pass for the throttle: `admin::throttle::FREE_ATTEMPTS + 1` wrong `/login`s in a
    /// row open a backoff window (proven deterministically, with an injected clock, in
    /// `admin::throttle`'s own tests), and this proves the wiring: a real `/login` landing inside
    /// that window is refused before it even starts a hash, including one that would otherwise
    /// have been the right password, which is the whole point of checking the window first rather
    /// than racing the credential check against it.
    ///
    /// Kept to exactly the failures needed to open the window, checked immediately afterward with
    /// no extra work in between: every failure here is a real Argon2 hash on a worker thread this
    /// test has to wait out, so the less real time spent before the one assertion that depends on
    /// the window still being open, the less this depends on how fast Argon2 happens to run on
    /// whatever machine executes it.
    #[tokio::test]
    async fn a_throttled_login_is_refused_before_any_hash_starts() {
        let mut server = with_player(
            GameServer::new(Config::default(), tiny_world()),
            0,
            "127.0.0.1:40002",
        );
        server.claim_token = Some("tok".to_string());
        server
            .run_admin_command(0, "register", "victim rightpassword tok")
            .unwrap();
        assert!(
            wait_until(&mut server, std::time::Duration::from_secs(2), |s| {
                !s.admin.unclaimed()
            })
            .await,
            "setup: the account must exist before it can be logged into"
        );

        for _ in 0..=crate::admin::throttle::FREE_ATTEMPTS {
            server
                .run_admin_command(0, "login", "victim wrongpassword")
                .unwrap();
            wait_until(&mut server, std::time::Duration::from_secs(2), |s| {
                !s.auth_in_flight.contains(&0)
            })
            .await;
        }

        // Inside the window now: refused before a hash starts, even offering the real password.
        server
            .run_admin_command(0, "login", "victim rightpassword")
            .unwrap();
        assert!(
            server.auth_in_flight.is_empty(),
            "a throttled attempt must not start hashing, even with the right password"
        );
        assert!(
            server.admin.signed_in_as(0).is_none(),
            "and must not have signed in either"
        );
    }
}

/// The first autosave used to cost a whole extra world-copy inside a counted tick, because
/// `spare_world` started life empty and had nothing to diff the incremental path against.
///
/// Caught by a real CI soak run, not a unit test: `save_world_in_background`'s incremental path
/// (`refresh_snapshot`) requires a buffer that already holds the world's state as of the moment
/// change-tracking began, and there was no such buffer until the first save built one the
/// expensive way. Measured on that run, 14,833 us, 89% of a single tick's budget, against a
/// later save, which refreshes a buffer instead of rebuilding one. That later figure was written
/// here as "150-200 µs" and is not: `save_world`'s own comment carries the re-measured table
/// (2.0 to 12.8 ms, scaling with how much changed between saves). Refreshing is still much
/// cheaper than rebuilding; it was never that cheap.
#[cfg(test)]
mod snapshot_baseline {
    use super::*;
    use crate::config::Config;

    /// The property the fix turns on: a buffer exists before any save is ever requested, so the
    /// first one has something to diff against instead of paying for a full copy on the clock.
    #[test]
    fn a_spare_snapshot_buffer_exists_before_the_first_save_is_ever_requested() {
        let server = GameServer::new(Config::default(), World::empty(300, 200, "presnapshot"));
        assert!(
            server.spare_world.is_some(),
            "the first autosave has nothing to refresh against, and pays for a full world copy \
             inside a counted tick instead of the incremental refresh `save_world`'s own comment \
             measures"
        );
    }

    /// And it is a genuine, independent copy — not the live world under a second name — or the
    /// "incremental" path would be diffing a buffer against itself.
    #[test]
    fn the_spare_buffer_is_a_real_copy_not_an_alias_of_the_live_world() {
        use crate::world::worldgen::tiles;

        let mut server = GameServer::new(Config::default(), World::empty(300, 200, "alias probe"));
        server
            .world
            .set_tile(10, 10, Tile::framed(tiles::CHEST, 0, 0));
        let spare = server
            .spare_world
            .as_ref()
            .expect("pre-warmed at construction");
        assert!(
            !spare.tile(10, 10).is_active(),
            "editing the live world after construction must not be visible through the spare \
             buffer, or it is an alias rather than a snapshot"
        );
    }
}

/// Does the server announce the game's own lines the way the game does?
///
/// Vanilla sends a localization key and its arguments — `NPC.cs:81383` is
/// `NetworkText.FromKey("Announcement.HasAwoken", ...)` against `"HasAwoken": "{0} has awoken!"` —
/// and the client renders it in whatever language that player is using. Sending the English
/// sentence instead was wrong three ways at once: different bytes on the wire than the real
/// server, English for every non-English player, and the game's own text compiled into this
/// repository. So the thing worth pinning is the *mode*, not the words.
#[cfg(test)]
mod announcements {
    use terrustia_proto::NetworkText;
    use terrustia_proto::net_text::TextMode;

    /// Keys used for the game's own lines, each verified against the decompiled localization
    /// files rather than guessed. A wrong key renders as nothing on the client, which is worse
    /// than English — so anything unverified deliberately stays a literal.
    #[test]
    fn the_keys_we_send_are_the_ones_the_game_defines() {
        for key in [
            // Terraria.Localization.Content.en-US.Legacy.json, "LegacyMisc"
            "LegacyMisc.8",  // The Blood Moon is rising...
            "LegacyMisc.9",  // You feel an evil presence watching you...
            "LegacyMisc.15", // The ancient spirits of light and dark have been released.
            "LegacyMisc.19", // {0} was slain...
            "LegacyMisc.20", // A solar eclipse is happening!
            "LegacyMisc.31", // The Pumpkin Moon is rising...
            "LegacyMisc.34", // The Frost Moon is rising...
            "LegacyMisc.47", // The Moon Lord has awoken!
            // The six hardmode ores
            "LegacyMisc.12",
            "LegacyMisc.13",
            "LegacyMisc.14",
            "LegacyMisc.21",
            "LegacyMisc.22",
            "LegacyMisc.23",
            // "LegacyMultiplayer"
            "LegacyMultiplayer.19", // {0} has joined.
            "LegacyMultiplayer.20", // {0} has left.
            // Terraria.Localization.Content.en-US.Game.json, "Announcement"
            "Announcement.HasAwoken",
            "Announcement.HasBeenDefeated_Single",
        ] {
            let text = NetworkText::key(key, Vec::new());
            assert_eq!(text.mode, TextMode::LocalizationKey);
            assert!(
                key.contains('.') && !key.ends_with('.'),
                "{key} is not a section-qualified key"
            );
        }
    }

    /// The ore announcements carry keys, not sentences.
    ///
    /// `hardmode.rs` holds them in a `&'static str` that used to be English, so this is the field
    /// most likely to quietly revert.
    #[test]
    fn the_ore_announcements_are_keys() {
        use crate::world::hardmode::{OreTiers, WorldShape, smash};
        use rand::{SeedableRng, rngs::SmallRng};

        let mut tiers = OreTiers::default();
        let mut rng = SmallRng::seed_from_u64(7);
        let shape = WorldShape {
            width: 4200,
            height: 1200,
            surface: 400,
            rock_layer: 600,
        };

        let mut seen = 0;
        for altars in 1..=3 {
            if let Some(smashed) = smash(altars, true, &mut tiers, shape, &mut rng) {
                let key = smashed.announcement;
                assert!(
                    key.starts_with("LegacyMisc."),
                    "the ore announcement is {key:?}, a sentence rather than a key"
                );
                seen += 1;
            }
        }
        assert!(seen > 0, "no altar smash produced an announcement to check");
    }

    /// A key with an argument nests, which is how "{0} has awoken!" gets its boss.
    #[test]
    fn an_announcement_can_carry_a_name() {
        let who = NetworkText::key("NPCName.MoonLord", Vec::new());
        let line = NetworkText::key("Announcement.HasAwoken", vec![who]);
        assert_eq!(line.substitutions.len(), 1);
        assert_eq!(line.substitutions[0].mode, TextMode::LocalizationKey);
        assert_eq!(line.substitutions[0].text, "NPCName.MoonLord");
    }
}

/// The two pylon-travel gates that a biome scan made possible (L2-21): the destination must still
/// sit in its network's biome (`DoesPylonAcceptTeleportation`) and a temple pylon stays sealed
/// until Plantera falls (the `wall == 87` clause of `HandleTeleportRequest`). Also the other place
/// the same scan is read as the game's independent zone flags rather than as one winning biome,
/// `shopping_zones`, since a flag that is wrong there is wrong for the same reason.
#[cfg(test)]
mod pylon_gates {
    use super::*;
    use crate::config::Config;
    use terrustia_proto::tile::{Liquid, Tile};

    /// Tile ids used by more than one test below, each a member of the `SceneMetrics` count named.
    const MUSHROOM_GRASS: u16 = 70; // MushroomTileCount member (`SceneMetrics.cs:617`)
    const SAND: u16 = 53; // SandTileCount member (`SceneMetrics.cs:620`)
    const EBONSTONE: u16 = 23; // EvilTileCount member (`SceneMetrics.cs:614`)
    const BLUE_DUNGEON_BRICK: u16 = 41; // DungeonTileCount member (`SceneMetrics.cs:619`)

    fn server() -> GameServer {
        let mut world = World::empty(1200, 400, "pylon gate probe");
        world.surface = 100;
        world.rock_layer = 200;
        GameServer::new(Config::default(), world)
    }

    fn pylon(kind: u8, x: i16, y: i16) -> net_module::Pylon {
        net_module::Pylon { x, y, kind }
    }

    /// Paint a `side*2`-square patch of one block type centred on a point, enough to carry a biome.
    fn paint(server: &mut GameServer, x: i16, y: i16, block: u16, side: i32) {
        for dy in -side..side {
            for dx in -side..side {
                server
                    .world
                    .set_tile(i32::from(x) + dx, i32::from(y) + dy, Tile::block(block));
            }
        }
    }

    /// Flood exactly `count` tiles with a liquid, left to right and top to bottom from a corner.
    ///
    /// Liquid rather than blocks, because shimmer is the one zone the game counts off
    /// `_liquidCounts` on the tiles its block counts skip (`SceneMetrics.cs:367-372`, `:601`), and
    /// an exact count is what makes the 300-tile threshold testable at its edge.
    fn pour(server: &mut GameServer, x: i16, y: i16, kind: Liquid, count: i32) {
        for n in 0..count {
            let (dx, dy) = (n % 40 - 20, n / 40 - 20);
            server.world.set_tile(
                i32::from(x) + dx,
                i32::from(y) + dy,
                Tile::AIR.with_liquid(kind, 255),
            );
        }
    }

    /// L2-21 check (4): a pylon is a valid destination only while it still sits in its own biome
    /// (`TeleportPylonsSystem.DoesPylonAcceptTeleportation`, TeleportPylonsSystem.cs:254-312).
    /// Before the fix there was no biome gate at all, so a pylon planted in the wrong biome still
    /// carried players; every `!...accepts` line below passed unconditionally then.
    #[test]
    fn a_pylon_must_match_its_network_biome() {
        const JUNGLE_GRASS: u16 = 60; // JungleTileCount member (`SceneMetrics.cs:613`)
        const PEARLSTONE: u16 = 109; // HolyTileCount member (`SceneMetrics.cs:603`)

        // Jungle (kind 1): a jungle-network pylon standing in a real jungle is accepted; in plain
        // forest it is not.
        let (jx, jy) = (600i16, 150i16);
        let mut jungle = server();
        paint(&mut jungle, jx, jy, JUNGLE_GRASS, 15); // 900 jungle tiles, over the 140 threshold
        assert!(
            jungle.pylon_accepts(&pylon(1, jx, jy)),
            "a jungle pylon in the jungle should work",
        );
        assert!(
            !server().pylon_accepts(&pylon(1, jx, jy)),
            "a jungle pylon standing in plain forest should be refused",
        );

        // Hallow (kind 2): the cheap-to-declare holy biome, accepted once its tiles are down.
        let mut hallow = server();
        paint(&mut hallow, jx, jy, PEARLSTONE, 9); // 324 holy tiles, over the 125 threshold
        assert!(
            hallow.pylon_accepts(&pylon(2, jx, jy)),
            "a hallow pylon in the hallow should work",
        );
        assert!(
            !server().pylon_accepts(&pylon(2, jx, jy)),
            "a hallow pylon in plain forest should be refused",
        );

        // SurfacePurity (kind 0): the plain surface, accepted when clear, refused once the same spot
        // is jungle.
        let (sx, sy) = (600i16, 40i16); // above surface(100), away from the edge bands
        assert!(
            server().pylon_accepts(&pylon(0, sx, sy)),
            "a purity pylon on the plain surface should work",
        );
        let mut overgrown = server();
        paint(&mut overgrown, sx, sy, JUNGLE_GRASS, 15);
        assert!(
            !overgrown.pylon_accepts(&pylon(0, sx, sy)),
            "a purity pylon is refused once its ground is jungle",
        );

        // Beach (kind 4): the surface band by an edge. Accepted within 380 tiles of the side,
        // refused in the middle of the world.
        let s = server();
        assert!(
            s.pylon_accepts(&pylon(4, 200, 40)),
            "a beach pylon near the edge should work",
        );
        assert!(
            !s.pylon_accepts(&pylon(4, 600, 40)),
            "a beach pylon in the middle of the map should be refused",
        );

        // The depth-keyed networks. Underground (kind 3) wants anywhere below the surface;
        // Underworld (kind 9) wants the underworld layer.
        assert!(
            s.pylon_accepts(&pylon(3, 600, 150)),
            "an underground pylon below the surface should work",
        );
        assert!(
            !s.pylon_accepts(&pylon(3, 600, 40)),
            "an underground pylon on the surface should be refused",
        );
        assert!(
            s.pylon_accepts(&pylon(9, 600, 260)), // y past height - 200 => underworld
            "an underworld pylon in the underworld should work",
        );
        assert!(
            !s.pylon_accepts(&pylon(9, 600, 40)),
            "an underworld pylon on the surface should be refused",
        );

        // Victory (kind 8) travels from anywhere, biome or no biome.
        assert!(s.pylon_accepts(&pylon(net_module::Pylon::VICTORY, 600, 40)));
    }

    /// The Glowing Mushroom network (kind 7), which used to fall through to the permissive
    /// `_ => true` because `Biome` has no mushroom variant to test.
    ///
    /// It never needed one. Vanilla reads `_sceneMetrics.EnoughTilesForGlowingMushroom`
    /// (`TeleportPylonsSystem.cs:293-298`), an independent tile count
    /// (`MushroomTileCount >= 100`, `SceneMetrics.cs:52`, `:260`, `:617`), which is exactly the
    /// `Zones::glowshroom` flag the same scan already produced and this function threw away.
    #[test]
    fn a_mushroom_pylon_needs_its_mushrooms() {
        let (mx, my) = (600i16, 150i16);
        assert!(
            !server().pylon_accepts(&pylon(7, mx, my)),
            "a mushroom pylon in plain forest should be refused",
        );

        let mut grove = server();
        paint(&mut grove, mx, my, MUSHROOM_GRASS, 6); // 144 tiles, over the 100 threshold
        assert!(
            grove.pylon_accepts(&pylon(7, mx, my)),
            "a mushroom pylon in a real mushroom field should work",
        );

        // One tile short of the threshold is still not a biome: 9 rows of 11 is 99.
        let mut sparse = server();
        for dy in 0..9 {
            for dx in 0..11 {
                sparse.world.set_tile(
                    i32::from(mx) + dx,
                    i32::from(my) + dy,
                    Tile::block(MUSHROOM_GRASS),
                );
            }
        }
        assert!(
            !sparse.pylon_accepts(&pylon(7, mx, my)),
            "99 mushroom tiles is one short of the 100 the game wants",
        );

        // `if (Main.remixWorld && Y >= Main.maxTilesY - 200) return false`
        // (`TeleportPylonsSystem.cs:295-297`). The world is 400 tall, so that line is row 200.
        let (deep_x, deep_y) = (600i16, 250i16);
        let mut deep = server();
        paint(&mut deep, deep_x, deep_y, MUSHROOM_GRASS, 6);
        assert!(
            deep.pylon_accepts(&pylon(7, deep_x, deep_y)),
            "an ordinary world has no floor under its mushroom network",
        );
        deep.world.secret_seeds.remix = true;
        assert!(
            !deep.pylon_accepts(&pylon(7, deep_x, deep_y)),
            "a remix world refuses a mushroom pylon in its bottom 200 rows",
        );
        // ...and only in those rows: the same seed leaves row 150's grove alone.
        grove.world.secret_seeds.remix = true;
        assert!(
            grove.pylon_accepts(&pylon(7, mx, my)),
            "row 150 is above the remix floor and stays accepted",
        );
    }

    /// The Shimmer network (kind 10), the other arm that used to fall through to `_ => true`.
    ///
    /// Vanilla reads the raw `_sceneMetrics.EnoughTilesForShimmer`
    /// (`TeleportPylonsSystem.cs:307-308`), which is `ShimmerTileCount >= 300`
    /// (`SceneMetrics.cs:24`, `:252`) and `ShimmerTileCount = _liquidCounts[3]` (`:601`): shimmer
    /// *liquid* on inactive tiles, not a block type. Deliberately not `ZoneShimmer` (`:711-712`),
    /// whose extra depth and dungeon terms the pylon check does not apply.
    #[test]
    fn a_shimmer_pylon_needs_shimmer() {
        let (sx, sy) = (600i16, 150i16);
        assert!(
            !server().pylon_accepts(&pylon(10, sx, sy)),
            "a shimmer pylon in a dry cave should be refused",
        );

        let mut pool = server();
        pour(&mut pool, sx, sy, Liquid::Shimmer, 300);
        assert!(
            pool.pylon_accepts(&pylon(10, sx, sy)),
            "exactly 300 shimmer tiles reaches `ShimmerTileCount >= 300`",
        );

        let mut shallow = server();
        pour(&mut shallow, sx, sy, Liquid::Shimmer, 299);
        assert!(
            !shallow.pylon_accepts(&pylon(10, sx, sy)),
            "299 is one short of the threshold",
        );

        // The count is bucket 3 alone, not "any liquid": a lake of water is not a shimmer biome.
        let mut lake = server();
        pour(&mut lake, sx, sy, Liquid::Water, 600);
        assert!(
            !lake.pylon_accepts(&pylon(10, sx, sy)),
            "water is `_liquidCounts[0]` and never reaches `ShimmerTileCount`",
        );
    }

    /// Two more entries in the surface-purity pylon's exclusion list that were being missed,
    /// both because it tested the single winning `Biome` where the game tests independent flags.
    ///
    /// Vanilla's list is `EnoughTilesForJungle || Snow || Desert || GlowingMushroom || Hallow ||
    /// Crimson || Corruption` (`TeleportPylonsSystem.cs:270`). Glowing mushroom was absent
    /// outright, and desert was read off the winner, so a place where a *higher-priority* biome
    /// won the vote passed the desert clause however much sand it had.
    #[test]
    fn a_purity_pylon_is_refused_by_the_zones_no_biome_wins() {
        let (px, py) = (600i16, 40i16); // above surface(100), clear of both 380-tile edge bands
        assert!(
            server().pylon_accepts(&pylon(0, px, py)),
            "the plain surface still accepts a purity pylon",
        );

        // Glowing mushroom, which was not in the list at all.
        let mut grove = server();
        paint(&mut grove, px, py, MUSHROOM_GRASS, 6);
        assert!(
            !grove.pylon_accepts(&pylon(0, px, py)),
            "a purity pylon buried in a mushroom field should be refused",
        );

        // Sand under a dungeon: `Biome::Dungeon` takes the vote and is not on the exclusion list,
        // so reading the winner accepted this while `EnoughTilesForDesert` was plainly true. A
        // dungeon entrance beside an ocean's sand is where this really happens.
        let mut dungeon_beach = server();
        paint(&mut dungeon_beach, px, py, SAND, 22); // 1936 sand, over the 1500 threshold
        paint(&mut dungeon_beach, px, py, BLUE_DUNGEON_BRICK, 8); // 256 brick, over 250, on 256 sand
        assert_eq!(
            crate::game::spawn::biome_at(&dungeon_beach.world, i32::from(px), i32::from(py)),
            crate::game::spawn::Biome::Dungeon,
            "the dungeon wins the vote here, which is the whole point of the case",
        );
        assert!(
            crate::game::spawn::zones_at(&dungeon_beach.world, i32::from(px), i32::from(py)).desert,
            "...while the desert flag is independently true, as it is in the game",
        );
        assert!(
            !dungeon_beach.pylon_accepts(&pylon(0, px, py)),
            "a purity pylon standing on 1680 tiles of sand should be refused",
        );
    }

    /// `shopping_zones` is the other reader of the same scan, and it was dropping the same two
    /// flags: `mushroom` was hardcoded `false`, and `desert` was read off the winning `Biome`.
    ///
    /// The game's own set is independent (`Player.ShoppingZone_AnyBiome`, `Player.cs:3807-3817`),
    /// and `mushroom` is `Player.ZoneGlowshroom`, which is `EnoughTilesForGlowingMushroom` and
    /// nothing else (`SceneMetrics.cs:684`). Without it the Truffle never got his one biome like
    /// (`happiness::Biome::Mushroom`), standing in the middle of his own biome.
    #[test]
    fn shopping_zones_report_the_flags_no_biome_wins() {
        /// Seat a player, prime the biome cache for their slot the way a spawn pass would, and
        /// read the shopping zones back.
        fn zones_for(server: &mut GameServer, x: i16, y: i16) -> terrustia_proto::happiness::Zones {
            let (out_tx, _out_rx) = mpsc::channel(64);
            let mut player = Player::new(0, "127.0.0.1:1".parse().expect("test address"), out_tx);
            player.position = (f32::from(x) * 16.0, f32::from(y) * 16.0);
            server.players[0] = Some(player);
            server
                .player_biomes
                .read(&server.world, 0, i32::from(x), i32::from(y))
                .expect("a fresh cache has budget for the first scan");
            server.shopping_zones(0)
        }

        let (px, py) = (600i16, 40i16);

        let plain = zones_for(&mut server(), px, py);
        assert!(!plain.mushroom && !plain.desert, "plain forest: {plain:?}");
        assert!(plain.forest(), "...and it is `ShoppingZone_Forest`");

        // The Truffle at home.
        let mut grove = server();
        paint(&mut grove, px, py, MUSHROOM_GRASS, 6);
        let mushroom = zones_for(&mut grove, px, py);
        assert!(
            mushroom.mushroom,
            "a mushroom field is `ZoneGlowshroom`: {mushroom:?}",
        );
        assert!(
            !mushroom.forest(),
            "...so it is no longer `ShoppingZone_Forest`",
        );

        // A corrupted desert is both at once in the game, and the corruption wins our vote.
        let mut corrupt_desert = server();
        paint(&mut corrupt_desert, px, py, SAND, 22); // 1936 sand
        paint(&mut corrupt_desert, px, py, EBONSTONE, 9); // 324 evil, over 300, on 324 sand
        let both = zones_for(&mut corrupt_desert, px, py);
        assert!(
            both.corruption && both.desert,
            "a corrupted desert is both zones at once: {both:?}",
        );
    }

    /// L2-21 check (5): a pylon standing on the Lihzahrd temple's own wall, below the surface, will
    /// not carry anyone until Plantera is defeated (`HandleTeleportRequest`, TeleportPylonsSystem
    /// .cs:124; wall 87 is `WallID.LihzahrdBrickUnsafe`, WallID.cs:243). Before the fix this gate
    /// was absent, so `temple_pylon_sealed` would have been `false` in every case here.
    #[test]
    fn a_temple_pylon_stays_sealed_until_plantera() {
        const LIHZAHRD_BRICK_WALL: u16 = 87;
        let (tx, ty) = (600i16, 150i16); // below surface(100)

        let mut s = server();
        s.world.set_tile(
            i32::from(tx),
            i32::from(ty),
            Tile::AIR.with_wall(LIHZAHRD_BRICK_WALL),
        );
        assert!(
            s.temple_pylon_sealed(&pylon(1, tx, ty)),
            "a temple pylon before Plantera should be sealed",
        );

        // Once Plantera falls the same pylon opens up.
        s.world.progress.downed_plantera = true;
        assert!(
            !s.temple_pylon_sealed(&pylon(1, tx, ty)),
            "the temple pylon should answer once Plantera is down",
        );

        // A pylon on the surface, or one not on temple brick, is never a temple pylon.
        let mut surface = server();
        surface
            .world
            .set_tile(600, 40, Tile::AIR.with_wall(LIHZAHRD_BRICK_WALL));
        assert!(
            !surface.temple_pylon_sealed(&pylon(1, 600, 40)),
            "a pylon above the surface is not gated by the temple rule",
        );
        let mut plain = server();
        plain
            .world
            .set_tile(i32::from(tx), i32::from(ty), Tile::AIR.with_wall(4));
        assert!(
            !plain.temple_pylon_sealed(&pylon(1, tx, ty)),
            "a pylon that is not on temple brick is not a temple pylon",
        );
    }

    /// A Beach pylon's band has a floor as well as a ceiling: `Y <= worldSurface && Y >
    /// worldSurface * 0.35` (TeleportPylonsSystem.cs:284). The arm only tested `Depth::Surface`,
    /// which reaches all the way to row zero, so a pylon hung in the sky over an ocean passed.
    #[test]
    fn a_beach_pylon_in_the_sky_is_refused() {
        let s = server(); // surface = 100, so the floor is row 35
        assert!(
            s.pylon_accepts(&pylon(4, 200, 40)),
            "a beach pylon just above the shoreline should still work",
        );
        assert!(
            !s.pylon_accepts(&pylon(4, 200, 30)),
            "a beach pylon in the sky band should be refused",
        );
        assert!(
            !s.pylon_accepts(&pylon(4, 200, 35)),
            "the floor is exclusive: exactly 0.35 of the surface is already too high",
        );
    }

    /// `DoesPositionHaveEnoughNPCs` counts residents who are *home*, not houses: each one must be
    /// within a hundred tiles of its own front door (TeleportPylonsSystem.cs:235-237). Without
    /// that clause a pylon stayed live while its townsfolk were off across the world, which is the
    /// whole point of the check.
    #[test]
    fn a_pylon_only_counts_residents_who_are_near_home() {
        use crate::game::npc::TILE;
        const GUIDE: u16 = 22;
        let (px, py) = (600i16, 150i16);

        let place = |s: &mut GameServer, home: (i32, i32), standing: (i32, i32)| {
            let index = s
                .npcs
                .spawn(GUIDE, (standing.0 as f32 * TILE, standing.1 as f32 * TILE))
                .expect("a free NPC slot");
            s.npcs.get_mut(index).expect("just spawned").home = Some(home);
        };

        // Two residents at home beside the pylon: enough for it to carry anybody.
        let mut s = server();
        place(&mut s, (600, 150), (600, 150));
        place(&mut s, (601, 150), (601, 150));
        assert_eq!(s.town_npcs_near(px, py), 2);

        // Same two houses, but one of them has wandered a hundred and fifty tiles away. Its house
        // is still in the box; it is not.
        let mut s = server();
        place(&mut s, (600, 150), (600, 150));
        place(&mut s, (601, 150), (751, 150));
        assert_eq!(
            s.town_npcs_near(px, py),
            1,
            "a resident more than a hundred tiles from home should not count",
        );

        // The scan box is 169 by 124 and `Rectangle.Contains` is exclusive at the bottom, so it
        // reaches 62 rows up and only 61 down.
        let mut s = server();
        place(&mut s, (600, 150 - 62), (600, 150 - 62));
        place(&mut s, (600, 150 + 62), (600, 150 + 62));
        assert_eq!(
            s.town_npcs_near(px, py),
            1,
            "the box reaches 62 rows above the pylon and 61 below",
        );
    }
}

/// The happiness wiring: that the server hands `ShopHelper`'s transcription the shopper's zones
/// and the resident's neighbours, and gets the game's number back. The formula itself is checked
/// in `terrustia_proto::happiness`; what is checked here is the mapping into it.
#[cfg(test)]
mod happiness_wiring {
    use super::*;
    use crate::config::Config;

    const GUIDE: u16 = 22;

    /// A world with a surface at row 100, one player standing where asked, and one Guide living
    /// beside the pylon-free middle of it.
    fn town(player_tile: (i32, i32)) -> (GameServer, u8) {
        let mut world = World::empty(1200, 400, "happiness probe");
        world.surface = 100;
        world.rock_layer = 200;
        let mut server = GameServer::new(Config::default(), world);

        let (out_tx, _out_rx) = mpsc::channel(1024);
        let mut player = Player::new(0, "127.0.0.1:1".parse().expect("test address"), out_tx);
        // Playing, not WorldSent: talking to an NPC is a gameplay action, and the pre-dispatch
        // handshake gate refuses packet 40 from a connection still mid-handshake. This helper
        // predates that gate and set WorldSent, which is exactly the state the gate exists to
        // refuse, so the test was driving a path a real client cannot reach.
        player.state = ConnState::Playing;
        player.position = (
            player_tile.0 as f32 * crate::game::npc::TILE,
            player_tile.1 as f32 * crate::game::npc::TILE,
        );
        server.players[0] = Some(player);

        let guide = server
            .npcs
            .spawn(
                GUIDE,
                (
                    600.0 * crate::game::npc::TILE,
                    90.0 * crate::game::npc::TILE,
                ),
            )
            .expect("a free NPC slot");
        server.npcs.get_mut(guide).expect("just spawned").home = Some((600, 90));
        (server, guide)
    }

    /// A Guide alone in a forest, quoted to a shopper standing on the plain surface: the solitude
    /// bonus (`ShopHelper.cs:151-155`) and the Guide's Forest like
    /// (`PersonalityDatabasePopulator.cs:27-31`), 0.95 * 0.94 = 0.89.
    #[test]
    fn a_lone_guide_on_the_surface() {
        let (server, guide) = town((600, 50));
        assert_eq!(server.shop_multiplier(0, guide), 0.89);
    }

    /// The same Guide, quoted to a shopper who has gone underground. `ShoppingZone_Forest` is
    /// false below the surface line (`Player.cs:3819-3831`), so the Forest like drops out and only
    /// the solitude bonus is left.
    #[test]
    fn the_forest_like_is_a_property_of_where_the_shopper_stands() {
        let (server, guide) = town((600, 150));
        assert_eq!(server.shop_multiplier(0, guide), 0.95);
    }

    /// Opening and closing a chat is what takes and clears the number, exactly where
    /// `Player.SetTalkNPC` does it (`Player.cs:4360-4375`). Nothing else touches it, and in
    /// particular no tick does.
    #[test]
    fn talking_takes_the_number_and_closing_the_chat_clears_it() {
        let (mut server, guide) = town((600, 50));
        // Packet 40's body is the claimed player slot (rewritten to the sender's) and the NPC
        // index, or -1 for "stopped talking" (`MessageBuffer.cs:2246-2263`).
        let talk = |server: &mut GameServer, npc: i16| {
            let mut payload = vec![0u8];
            payload.extend_from_slice(&npc.to_le_bytes());
            server.handle_packet(
                0,
                crate::net::codec::Frame {
                    id: id::SYNC_TALK_N_P_C,
                    payload: Bytes::from(payload),
                },
            );
        };

        talk(&mut server, i16::from(guide));
        assert_eq!(
            server.players[0].as_ref().expect("player").talking_to,
            Some(guide)
        );
        assert_eq!(
            server.players[0].as_ref().expect("player").shop_multiplier,
            0.89
        );

        talk(&mut server, -1);
        assert_eq!(server.players[0].as_ref().expect("player").talking_to, None);
        assert_eq!(
            server.players[0].as_ref().expect("player").shop_multiplier,
            1.0,
            "closing the chat is ShoppingSettings.NotInShop",
        );
    }
}
