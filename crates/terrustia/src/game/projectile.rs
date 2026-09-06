//! Projectiles: the things NPCs throw, and what happens to them afterwards.
//!
//! A projectile is a much simpler entity than an NPC — no targeting, no state machine, usually no
//! decisions at all — but it is the half of combat the server was missing. Every routine that
//! decided to shoot has been emitting its aim and cadence for a while; this is what makes those
//! decisions land.
//!
//! Twenty-one behaviours are transcribed here, and twelve more in `server::systems` (see below).
//! This file used to say "a handful of behaviours cover everything the roster and the world's
//! traps fire"; the count, when it was finally taken, was **43 of the 79 types something here can
//! launch reaching no arm at all**. It is 8 of 80 now.
//!
//! The 8 left are not all straight lines, and saying so would be the same mistake again. Read
//! against `Projectile.cs`: none of them *falls* - they are hovering clouds, shockwaves and
//! convergences - so a straight line is a poorer approximation of them than it was of a thrown
//! bone, not a free one. One of the eight, the Rain Nimbus, is in fact **already right**: style
//! 45's branch for it sets a rotation and nothing else. `TODO.md`'s C6 has the list, by style,
//! with what each one does.
//!
//! **A style whose arm needs to see anything but tiles lives in `server::systems` instead**, and
//! is no less transcribed for it: a projectile cannot search the player list, walk the NPC table
//! or read the boss that launched it from inside its own tick. Styles 80 (the Saucer's missile),
//! 84 (the Moon Lord's deathray), 85 (his brand), 109 (the Mechanic's wrench), 111 (the Dryad's
//! ward), 127/128 (the Sand Elemental's mark and tornado), 133 (the Dark Mage's sigils) and
//! 171/180 (two of the Empress's) are all there, called from `tick_projectiles` before the
//! movement below, in vanilla's own order, and so is the seeking half of 65 (Duke Fishron's
//! second bubble). So is `tick_friendly_projectile_hits`, which is `Damage_PVE` and runs *after*
//! the movement, where `Projectile.Update` puts it. So is `tick_friendly_projectile_hits`, which is
//! `Damage_PVE` and runs after the movement, where `Projectile.Update` puts it.
//!
//! * **Style 1**, the arc: it flies straight for a quarter of a second and then starts falling, a
//!   tenth of a pixel a tick, capped at sixteen. Feathers, stingers, snowballs, skulls and darts.
//! * **Style 2**, the throw: twenty ticks flat, then it falls four tenths of a pixel a tick and
//!   drags, to a terminal of thirty-two. Bones, knives, syringes, cannonballs, Santa's bombs. The
//!   snowball falls gentler and drags lighter, and is the one type here with its own numbers.
//! * **Style 5**, the falling star: no acceleration at all. What it has instead is a latch - it
//!   passes through terrain until it has been clear of it once - so a star handed over inside a
//!   mountain falls out of it rather than dying on the tick it appears.
//! * **Style 8**, the fireball: twenty ticks, then a fifth of a pixel a tick. Three types skip the
//!   counter entirely and so never fall, which is why a Golem's fireball and a Cursed Flame cross
//!   a room flat while a Ball of Fire arcs into the floor.
//! * **Style 10**, the lob: it falls from the first tick and sticks where it lands.
//! * **Style 16**, the bomb: it *bounces*, at two fifths, off anything it meets, and off a floor
//!   only if it was still falling at speed. A mine settles dead where it lands; everything else
//!   keeps its own fuse and then rolls along whatever it landed on. A grenade thrown at a wall
//!   used to vanish into it.
//! * **Style 58**, the present: it holds its upward speed for half a second, tips over a tenth of
//!   a pixel a tick, and then falls at three and no faster. Catchable, which is the point of it.
//! * **Style 68**, the ale: fifteen ticks flat, then an ordinary fall.
//! * **Style 14**, the rolling ball: it falls, loses speed along the ground, and *bounces* off
//!   what it hits at nine tenths rather than dying on it. A spiky ball trap fills a corridor
//!   because of this one rule.
//! * **Style 18**, the scythe: it spins, and between its thirtieth and hundredth tick it
//!   *accelerates* by six per cent a tick — which is why a demon's scythe is harmless when it
//!   leaves and lethal by the time it reaches you.
//! * **Style 23**, the flame: straight, and never alight for more than a second.
//! * **Style 25**, the boulder: it falls, hops off a hard landing, and then rolls away from
//!   whichever side of it has a wall, working up to seven pixels a tick until it meets one head on
//!   and breaks. It is also harmless for its first seven ticks, which is the only reason standing
//!   next to a Boulder Statue you have just wired is survivable.
//! * **Style 37**, the spear: it grows out of its trap to three hundred pixels or until it hits
//!   something, then pulls back and dies when it is home.
//! * **Style 38**, the flamethrower: it does no damage itself. Every sixth tick it emits a flame,
//!   which is the thing that burns.
//! * **Style 126**, the geyser: it rises out of its vent until the way ahead is clear, then hangs
//!   there for a second. If it never gets clear it dies inside the wall.
//! * **Style 183**, the Zoologist's claw: it keeps a fifth of its sideways speed each tick and
//!   never falls, and the drag runs before the move - so a swipe thrown at twenty-four pixels a
//!   tick travels six pixels in total and then dies at eighteen ticks. It is a claw at arm's
//!   length, not a projectile; unarmed it crossed four hundred and thirty.
//! * **Style 186**, the Princess's weapon: sixty ticks and then gone. Everything else in its
//!   method is drawing, and its own table says 180, so nothing but the arm knows the real number.
//! * **Style 112**, the Truffle's spore: it does not travel. Its velocity is overwritten every
//!   tick with a pure vertical bob, a sine over three seconds, so it hangs where the Truffle put
//!   it. Style 112 is three unrelated bodies keyed on the type inside the arm, exactly as vanilla
//!   keys them, and the Dandelion seed's is not transcribed.
//! * **Style 135**, the Queen Slime's smash: nine ticks, stationary, and it **grows its own
//!   hitbox** from five tiles across to thirty around a fixed centre. The hitbox is the attack;
//!   without it a boss's shockwave was a thirty-pixel box you could stand beside.
//! * **Style 157**, the Deerclops ice spike: it never touches its velocity, because what it is
//!   launched with is a facing rather than a speed. Twenty ticks and then gone.
//! * **Style 65**, Duke Fishron's mouth bubbles: they bob, tracing a cosine around the speed they
//!   were thrown at over thirty ticks. That is the half of style 65 with no target; the half with
//!   one re-aims at a player every tick and is in `server::systems`, and a zero `ai[1]` is what
//!   tells the two apart.
//!
//! Style 25 was the one doing real harm by its absence. A Boulder Statue launches its boulder *at
//! rest*, so with no arm to give it gravity it never moved at all: a wired statue put a stationary
//! thirty-one-pixel hostile box under itself for a full minute and then took it away again. The
//! trap that is meant to chase you down a corridor was a damage aura around its own pedestal.
//!
//! **What is still narrowed, and it is no longer movement.** A style-16 bomb reaches the ground
//! and then simply expires: `Projectile.Kill`'s explosion, which widens the hitbox and breaks
//! tiles, is not modelled, so a grenade lands and does nothing where vanilla's takes a hole out of
//! the wall. Per-type flourishes inside the arms that are here are disclosed at each: a boulder's
//! six cousins keep their own branches, a bomb's two unreached families keep theirs. And the
//! per-axis collision probe every bouncing style shares is not vanilla's swept
//! `Collision.TileCollision`, so an inside corner comes off both axes here where the game would
//! clip one.

use terrustia_proto::projectile::{MAX_PROJECTILES, ProjectileKey, SERVER_OWNER};
use terrustia_proto::projectile_data::{ProjectileStats, projectile_stats};
use terrustia_proto::tile_solid::{solid, solid_top};

use super::npc::{TILE, TileView};

/// Terminal speed for anything that falls.
const TERMINAL: f32 = 16.0;
/// How long a style-1 projectile flies flat before gravity takes it.
const ARC_DELAY: f32 = 15.0;
/// ...and how hard it then falls.
const ARC_GRAVITY: f32 = 0.1;
/// The scythe's acceleration window and rate.
const SCYTHE_FROM: f32 = 30.0;
const SCYTHE_UNTIL: f32 = 100.0;
const SCYTHE_ACCEL: f32 = 1.06;
/// ...and how fast it spins.
const SCYTHE_SPIN: f32 = 0.8;

/// What a Zoologist's claw keeps of its sideways speed each tick (`velocity.X *= 0.2f`).
const ZOOLOGIST_DRAG: f32 = 0.2;
/// How long the Princess's weapon lasts, which is not what its table says.
const PRINCESS_WEAPON_LIFE: f32 = 60.0;
/// `ProjectileID.TruffleSpore`, the one style-112 type this server can launch.
const TRUFFLE_SPORE: u16 = 590;
/// Its bob: a full sine over three seconds, fifteen hundredths of a pixel at the extremes.
const SPORE_PERIOD: f32 = 180.0;
const SPORE_BOB: f32 = 0.15;
/// The Queen Slime's smash: nine ticks, and a hitbox easing from five tiles across to thirty.
/// (`num = 40f` is the Ogre's; `if (type == 922) num = 30f` is hers.)
const SMASH_TICKS: f32 = 9.0;
const SMASH_FROM: f32 = 5.0;
const SMASH_TO: f32 = 30.0;
/// Duke Fishron's mouth bubbles (`Projectile.cs:30012-30084`). The seeking half of style 65 needs
/// the player list and lives in `systems::tick_sharknado_bolts`; what is here is the bob, and the
/// zero `ai[1]` that tells the two apart.
const SHARKNADO_STYLE: i32 = 65;
/// `num523 = MathF.PI / 15f` and `num524 = 4f`: a cosine over thirty ticks, four pixels either way.
const BOLT_BOB_RATE: f32 = std::f32::consts::PI / 15.0;
const BOLT_BOB_AMPLITUDE: f32 = 4.0;
/// How long a Deerclops ice spike stands: `num10 = 20` for `type == 961`.
const SPIKE_LIFE: f32 = 20.0;

/// One projectile in flight.
#[derive(Debug, Clone, Copy)]
pub struct Projectile {
    pub key: ProjectileKey,
    pub projectile_type: u16,
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    pub damage: i32,
    pub knockback: f32,
    pub ai: [f32; 3],
    /// Working state a routine keeps to itself.
    ///
    /// The game splits these from `ai` because they are never synced: they are what a projectile
    /// needs to remember, not what a client needs to be told. A spear's anchor lives here, and so
    /// does a boulder's age. Four wide because vanilla's is (`Projectile.localAI`), and slot 2 is
    /// the one the boulder's own damage gate reads.
    pub local_ai: [f32; 4],
    pub rotation: f32,
    pub time_left: i32,
    /// How many more things it can hit. -1 means no limit.
    pub penetrate: i32,
    pub stats: ProjectileStats,
    /// Set whenever clients need telling about it.
    pub dirty: bool,
}

impl Projectile {
    pub fn width(&self) -> f32 {
        self.stats.width as f32
    }

    pub fn height(&self) -> f32 {
        self.stats.height as f32
    }

    pub fn center(&self) -> (f32, f32) {
        (
            self.position.0 + self.width() / 2.0,
            self.position.1 + self.height() / 2.0,
        )
    }

    /// Whether this box overlaps another.
    pub fn overlaps(&self, position: (f32, f32), size: (f32, f32)) -> bool {
        self.position.0 < position.0 + size.0
            && self.position.0 + self.width() > position.0
            && self.position.1 < position.1 + size.1
            && self.position.1 + self.height() > position.1
    }

    /// Whether it can hurt anything *this* tick, as against whether it is hostile at all.
    ///
    /// `Projectile.CanDamage` (`Projectile.cs:12491-12494`) is a chain of per-type exemptions and
    /// only one of them reaches a projectile this server launches: a boulder is harmless for its
    /// first seven ticks. That grace is the whole reason a Boulder Statue is survivable, because
    /// the boulder appears twenty-eight pixels below the statue's own base, which is inside the
    /// hitbox of anybody standing next to it triggering the thing.
    pub fn can_damage(&self) -> bool {
        let excused = self.stats.ai_style == BOULDER_STYLE
            && !matches!(self.projectile_type, 1005 | 1014 | 1021 | 1047)
            && self.local_ai[2] <= BOULDER_GRACE;
        !excused
    }
}

/// `ProjectileID.PurificationPowder`. The only *client*-owned projectile this server has to know
/// the flight of, because it is the only one whose whole effect is a server-side decision.
pub const PURIFICATION_POWDER: u16 = 10;

/// A Purification Powder cloud in flight, tracked apart from [`ProjectileStore`].
///
/// It is deliberately not a [`Projectile`] in the store. The store's contents are the server's own
/// shots: they are stepped, they go dirty, they are broadcast, and they send a kill packet when
/// they expire. A powder is somebody else's projectile, already on every client's screen from the
/// thrower's own packet 27, so putting one in the store would draw a second cloud beside it and
/// then kill a projectile the server does not own. All the server wants is where the cloud is.
///
/// Vanilla decides the powder's one real effect on the server (`Projectile.Damage_TryUsingPowders`
/// runs under `Main.netMode != 1`, `Projectile.cs:14787-14808`), unlike ordinary weapon damage,
/// which the owning client decides and reports. That is why this is tracked at all rather than
/// trusting a client to say "I purified that": the game does not trust it either.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Powder {
    /// Top-left, the same corner packet 27 carries and vanilla's `Damage_GetHitbox` reads
    /// (`Projectile.cs:15040`: `new Rectangle((int)position.X, (int)position.Y, width, height)`).
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    /// `ai[0]`, the tick counter `aiStyle == 6` kills the cloud on.
    pub age: f32,
}

/// How wide and tall a powder cloud is (`projectile_data.rs`'s own entry for type 10, four tiles
/// square). Read once here rather than through `projectile_stats` so the hit test cannot silently
/// become a point when a lookup fails.
pub const POWDER_SIZE: (f32, f32) = (64.0, 64.0);
/// `aiStyle == 6`: how much speed the cloud keeps each tick, and the tick it dies on.
const POWDER_DRAG: f32 = 0.95;
const POWDER_LIFE: f32 = 180.0;

impl Powder {
    /// One tick of `aiStyle == 6` (`Projectile.cs:24366-24372`):
    ///
    /// ```csharp
    /// velocity *= 0.95f;
    /// this.ai[0]++;
    /// if (this.ai[0] == 180f) { Kill(); }
    /// ```
    ///
    /// Returns `false` when the cloud is spent. Nothing here reads tiles: type 10 carries
    /// `tileCollide = false`, so a cloud drifts through a wall rather than dying on one.
    pub fn step(&mut self) -> bool {
        self.velocity.0 *= POWDER_DRAG;
        self.velocity.1 *= POWDER_DRAG;
        self.age += 1.0;
        self.position.0 += self.velocity.0;
        self.position.1 += self.velocity.1;
        self.age < POWDER_LIFE
    }

    /// Whether the cloud covers a box, the same overlap [`Projectile::overlaps`] uses.
    pub fn overlaps(&self, position: (f32, f32), size: (f32, f32)) -> bool {
        self.position.0 < position.0 + size.0
            && self.position.0 + POWDER_SIZE.0 > position.0
            && self.position.1 < position.1 + size.1
            && self.position.1 + POWDER_SIZE.1 > position.1
    }
}

/// Whether a tile stops a projectile.
fn blocking(tiles: &impl TileView, x: i32, y: i32) -> bool {
    let tile = tiles.tile(x, y);
    tile.is_active() && solid(tile.block) && !solid_top(tile.block)
}

/// Whether a box overlaps anything solid.
fn hits_terrain(tiles: &impl TileView, position: (f32, f32), size: (f32, f32)) -> bool {
    let left = (position.0 / TILE).floor() as i32;
    let right = ((position.0 + size.0 - 1.0) / TILE).floor() as i32;
    let top = (position.1 / TILE).floor() as i32;
    let bottom = ((position.1 + size.1 - 1.0) / TILE).floor() as i32;
    (left..=right).any(|x| (top..=bottom).any(|y| blocking(tiles, x, y)))
}

/// What a projectile's tick concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Flying,
    /// It hit something, ran out of time, or left the world.
    Spent,
}

/// A projectile a routine wants in the air, which it cannot put there from inside its own tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emission {
    pub projectile_type: u16,
    /// The centre it should appear at.
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    pub damage: i32,
}

/// How far a spear reaches before it starts pulling back.
const SPEAR_REACH: f32 = 300.0;
/// How often a flamethrower emits, and what it emits.
const FLAME_EVERY: f32 = 6.0;
const FLAME: u16 = 188;
/// How much of its speed a rolling ball keeps when it bounces.
const BOUNCE: f32 = -0.9;
/// A thrown thing (`Projectile.cs:23907-23930`): twenty ticks of flat flight, then it falls and
/// drags, to a terminal of thirty-two rather than sixteen.
const THROWN_DELAY: f32 = 20.0;
const THROWN_GRAVITY: f32 = 0.4;
const THROWN_DRAG: f32 = 0.97;
const THROWN_TERMINAL: f32 = 32.0;
/// It also tumbles the whole way (`Projectile.cs:23945-23947`).
const THROWN_SPIN: f32 = 0.03;
/// `ProjectileID.SnowBallFriendly`, the one style-2 type this server launches with its own numbers
/// (`Projectile.cs:23838-23843`): a gentler fall and a lighter drag.
const SNOWBALL: u16 = 166;
const SNOWBALL_GRAVITY: f32 = 0.3;
const SNOWBALL_DRAG: f32 = 0.98;
/// A fireball (`Projectile.cs:24618-24641`). Twenty ticks, then a soft fall, and a fixed spin.
const FIREBALL_DELAY: f32 = 20.0;
const FIREBALL_GRAVITY: f32 = 0.2;
const FIREBALL_SPIN: f32 = 0.3;
/// A falling star's own style (`Projectile.cs:24082-24133`), which is not a movement arm at all.
const STAR_STYLE: i32 = 5;
/// The sandnado (`Projectile.cs:36662-36705`): it stands still and fills its own column.
const SANDNADO_STYLE: i32 = 127;
/// `num995 = 300f` for type 657, the hostile one, against 900 for a player's.
const SANDNADO_LIFE: f32 = 300.0;
/// `num997`/`num998`, how far up and down it looks for the open span.
const SANDNADO_REACH: i32 = 15;
/// `vector154.X = vector154.Y * 0.2f` and then `width = vector154.X * 0.65f`: it is a fifth as wide
/// as it is tall, and its hitbox is two thirds of that again.
const SANDNADO_SLENDER: f32 = 0.2 * 0.65;
/// The Martian Saucer's missile (`Projectile.cs:31447-31513`). Its steering needs the player list
/// and lives in `systems::tick_saucer_missiles`; what is read here is the tile-collision flag that
/// phase turns on.
const SAUCER_MISSILE_STYLE: i32 = 80;
/// The Mechanic's wrench (`Projectile.cs:34652-34690`). Its return leg needs the NPC table and
/// lives in `systems::tick_mechanic_wrenches`; what is read here is the tile collision, which it
/// has on the way out and not on the way home, and the bounce that turns it round.
const WRENCH_STYLE: i32 = 109;
/// A dropped present (`Projectile.cs:29337-29366`): it drifts for half a second, tips over, and
/// then settles into a slow fall it never exceeds.
const PRESENT_DRIFT: f32 = 30.0;
const PRESENT_GRAVITY: f32 = 0.1;
const PRESENT_TERMINAL: f32 = 3.0;
const PRESENT_DRAG: f32 = 0.99;
/// The Empress's lance (`AI_179_FairyQueenLance`, `Projectile.cs:45853-45876`): it hangs where it
/// was drawn for a second, then leaves at forty.
const LANCE_HOLD: f32 = 60.0;
const LANCE_SPEED: f32 = 40.0;
/// Her lasting rainbow (`AI_173_HallowBossRainbowTrail`, `:46264-46275`): a curve that eases in
/// over thirty ticks to half a degree a tick and then holds it.
const RAINBOW_TURN: f32 = std::f32::consts::PI / 360.0;
const RAINBOW_EASE: f32 = 30.0;
/// A thrown ale (`Projectile.cs:30655-30677`): fifteen ticks flat, then an ordinary fall.
const ALE_DELAY: f32 = 15.0;
const ALE_GRAVITY: f32 = 0.2;
const ALE_DRAG: f32 = 0.99;
const ALE_SPIN: f32 = 0.25;
/// A bomb (`Projectile.cs:47764-48409`, `AI_016_Bombs`).
const BOMB_STYLE: i32 = 16;
const BOMB_GRAVITY: f32 = 0.2;
/// A mine settles rather than rolling: one drag on both axes, and anything under a tenth of a pixel
/// is snapped to a stop (`Projectile.cs:48354-48366`).
const MINE_DRAG: f32 = 0.97;
const MINE_STOP: f32 = 0.1;
/// Everything else rolls along whatever it landed on until it is under a hundredth of a pixel
/// (`Projectile.cs:48378-48399`), after a fuse of its own measured in `ai[0]`.
const BOMB_ROLL_DRAG: f32 = 0.97;
const BOMB_ROLL_STOP: f32 = 0.01;
const BOMB_LONG_FUSE: f32 = 10.0;
const BOMB_SHORT_FUSE: f32 = 5.0;
const BOMB_SPIN: f32 = 0.1;
/// A bomb bounces off what it hits at two fifths, and only off a floor it met at speed
/// (`Projectile.cs:19872-19886`).
const BOMB_BOUNCE: f32 = -0.4;
const BOMB_BOUNCE_FLOOR: f32 = 0.7;
/// A boulder: `Projectile.cs:26596-26746` for the flight, `:19056-19099` for what it does to the
/// wall it meets, and `:12491-12494` for the seven ticks before it can hurt anybody.
const BOULDER_STYLE: i32 = 25;
const BOULDER_GRACE: f32 = 7.0;
/// How fast it rolls off the mark, and the ceiling it works up to.
const BOULDER_NUDGE: f32 = 0.5;
const BOULDER_TOP_SPEED: f32 = 7.0;
const BOULDER_ACCEL: f32 = 0.05;
/// Rolling only picks up speed while it is not really falling.
const BOULDER_ROLLING_FALL: f32 = 6.0;
const BOULDER_GRAVITY: f32 = 0.3;
/// A landing harder than this hops rather than settling, at a fifth of the speed it arrived with.
const BOULDER_HOP_FROM: f32 = 5.0;
const BOULDER_HOP: f32 = -0.2;
const BOULDER_SPIN: f32 = 0.06;

/// Walk a box from `from` towards `to` in sub-tile increments, stopping at the first solid tile.
///
/// Returns the furthest position reached and whether terrain was struck. This is the swept form of
/// the game's `Collision.TileCollision`: a projectile crosses at most half a tile between checks,
/// so nothing tunnels however fast it flies, while still advancing the *full* velocity vanilla
/// gives it in a single pass. The old code confused the two, dividing velocity by the pass count,
/// which is what flew the fast types at a fraction of their speed.
fn advance(
    from: (f32, f32),
    to: (f32, f32),
    size: (f32, f32),
    tiles: &impl TileView,
) -> ((f32, f32), bool) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    // A projectile that is not moving cannot collide with anything, however deep in a wall it is
    // sitting. Vanilla is emphatic about this by construction: every one of `Collision.TileCollision`'s
    // four clauses (`Collision.cs:2299-2420`) tests the box at `Position + Velocity` against a
    // tile *and* the box at `Position` against being clear of it on that side, so a zero velocity
    // is handed straight back unchanged and no style ever reaches its kill. Ours probed `from`
    // itself, because `steps` floors at one and the single probe of a zero-length walk is the
    // start - so anything launched at rest and already overlapping terrain died on its first tick.
    // The Dryad's ward is cast exactly that way.
    if dx == 0.0 && dy == 0.0 {
        return (to, false);
    }
    let distance = (dx * dx + dy * dy).sqrt();
    let steps = (distance / (TILE * 0.5)).ceil().max(1.0) as i32;
    let mut last = from;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let probe = (from.0 + dx * t, from.1 + dy * t);
        if hits_terrain(tiles, probe, size) {
            return (last, true);
        }
        last = probe;
    }
    (to, false)
}

/// Drive one projectile for a tick.
///
/// Anything it decides to put in the air is pushed onto `emits` rather than spawned, because a
/// projectile cannot reach into the store that owns it.
///
/// The game runs the *entire* update body once per `extraUpdates + 1`: the AI, the movement and the
/// `timeLeft` decrement all repeat, at full velocity each pass. `Projectile.Update`
/// (`Projectile.cs:16781`) wraps `numUpdates = extraUpdates; while (numUpdates >= 0)` around the
/// whole thing, with `AI()` at 16881 and `timeLeft--` at 17308 inside the loop. Sub-stepping
/// velocity by the pass count and decrementing `timeLeft` once per frame flew the 33 hostile types
/// that carry extra updates at 1/(N+1) speed and kept them alive (N+1) times too long.
pub fn step(
    projectile: &mut Projectile,
    tiles: &impl TileView,
    emits: &mut Vec<Emission>,
) -> Outcome {
    let passes = projectile.stats.extra_updates + 1;
    for _ in 0..passes {
        match projectile.stats.ai_style {
            1 => {
                projectile.ai[0] += 1.0;
                if projectile.ai[0] >= ARC_DELAY {
                    projectile.velocity.1 = (projectile.velocity.1 + ARC_GRAVITY).min(TERMINAL);
                }
                projectile.rotation = projectile.velocity.1.atan2(projectile.velocity.0) + 1.57;
            }
            2 => {
                // A thrown thing: it flies flat for twenty ticks and then falls
                // (`Projectile.cs:23907-23930`). Bones, knives, syringes, cannonballs and Santa's
                // bombs all leave a hand or a barrel on an arc, and flew perfectly straight here.
                //
                // The spin (`:23945-23947`) is `* direction` in vanilla, which is the thrower's
                // facing. A projectile carries no direction on this server, so it always tumbles
                // one way; the rotation is drawn by the client and changes nothing else.
                projectile.rotation +=
                    (projectile.velocity.0.abs() + projectile.velocity.1.abs()) * THROWN_SPIN;
                let (gravity, drag) = if projectile.projectile_type == SNOWBALL {
                    (SNOWBALL_GRAVITY, SNOWBALL_DRAG)
                } else {
                    (THROWN_GRAVITY, THROWN_DRAG)
                };
                projectile.ai[0] += 1.0;
                if projectile.ai[0] >= THROWN_DELAY {
                    projectile.velocity.1 += gravity;
                    projectile.velocity.0 *= drag;
                }
                // Style 2's own terminal, which is twice everything else's.
                projectile.velocity.1 = projectile.velocity.1.min(THROWN_TERMINAL);
            }
            STAR_STYLE => {
                // A falling star does not accelerate at all: it keeps whatever speed it was given,
                // and the whole arm is bookkeeping. The one piece that matters to a server is the
                // latch below, handled at the collision step: `ai[1]` stays zero until the star
                // has been clear of terrain once (`Projectile.cs:24170-24176`), which is what stops
                // one handed over inside a mountain from dying on the tick it appears.
                //
                // Its death at dawn is real and lives in `systems::tick_falling_stars`, because it
                // needs the world clock this function is deliberately not given.
                if projectile.ai[1] == 0.0
                    && !hits_terrain(
                        tiles,
                        projectile.position,
                        (projectile.width(), projectile.height()),
                    )
                {
                    projectile.ai[1] = 1.0;
                }
            }
            8 => {
                // A fireball. The counter is skipped for three types, and that skip *is* the
                // behaviour: with `ai[1]` never reaching twenty, a Golem's fireball (258) and a
                // Cursed Flame (96) never take gravity at all and fly flat for their whole life,
                // where a Ball of Fire (15) arcs (`Projectile.cs:24618-24641`).
                if !matches!(projectile.projectile_type, 27 | 96 | 258) {
                    projectile.ai[1] += 1.0;
                }
                if projectile.ai[1] >= FIREBALL_DELAY {
                    projectile.velocity.1 += FIREBALL_GRAVITY;
                }
                projectile.rotation += FIREBALL_SPIN;
                projectile.velocity.1 = projectile.velocity.1.min(TERMINAL);
            }
            BOMB_STYLE => bomb(projectile),
            SANDNADO_STYLE => {
                // A sandnado does not travel and is not the size its table says. Every tick it
                // walks up and down the column it is standing in, up to fifteen tiles each way,
                // and then *becomes* that column: `width = height * 0.2 * 0.65`, `height` the
                // whole open span, centred in it (`Projectile.cs:36689-36703`). That is why a
                // sandnado is a wall you cannot walk under rather than a puff you step over, and
                // it is the only reason the table's 10x10 is not what a player meets. `stats` is a
                // per-projectile copy here, which is what makes writing to it the same move
                // vanilla makes on the projectile itself.
                projectile.ai[0] += 1.0;
                if projectile.ai[0] >= SANDNADO_LIFE {
                    return Outcome::Spent;
                }
                let centre = projectile.center();
                let column = (centre.0 / TILE) as i32;
                let row = (centre.1 / TILE) as i32;
                let mut top = row;
                while row - top < SANDNADO_REACH && !blocking(tiles, column, top - 1) {
                    top -= 1;
                }
                let mut bottom = row;
                while bottom - row < SANDNADO_REACH && !blocking(tiles, column, bottom + 1) {
                    bottom += 1;
                }
                // Vanilla's `ExpandVertically` hands back the *solid* tiles that stopped it and
                // then steps inside them with `topY++; bottomY--`. The walk above already stops on
                // the last open row, so those two are already applied and doing them again would
                // shrink the column by a tile at each end - which is exactly what the first version
                // of this did, and the test caught it at 96 pixels instead of 128.
                let (high, low) = (
                    top as f32 * TILE + TILE / 2.0,
                    bottom as f32 * TILE + TILE / 2.0,
                );
                let span = (low - high).max(TILE);
                projectile.stats.height = span as i32;
                projectile.stats.width = (span * SANDNADO_SLENDER) as i32;
                let middle = (high + low) / 2.0;
                projectile.position = (
                    column as f32 * TILE + TILE / 2.0 - projectile.width() / 2.0,
                    middle - projectile.height() / 2.0,
                );
                projectile.velocity = (0.0, 0.0);
            }
            179 => {
                // The Empress's lance: it hangs exactly where it was drawn for a full second and
                // *then* leaves, at forty pixels a tick (`Projectile.cs:45853-45866`). The hold is
                // the attack - a ring of lances appears around you, holds long enough to be read,
                // and then all of them fire at once. Vanilla launches it at `Vector2.Zero` and
                // keeps the angle in `ai[0]`; `Shot` carries no ai values, so the angle is taken
                // from the direction it was launched in and the hold is done by parking the
                // velocity, which comes to the same thing and needs no field on every shot.
                if projectile.local_ai[0] == 0.0 {
                    projectile.local_ai[0] = 1.0;
                    let (vx, vy) = projectile.velocity;
                    projectile.local_ai[1] = vy.atan2(vx);
                    projectile.velocity = (0.0, 0.0);
                }
                projectile.local_ai[2] += 1.0;
                if projectile.local_ai[2] >= LANCE_HOLD && projectile.velocity == (0.0, 0.0) {
                    let (sin, cos) = projectile.local_ai[1].sin_cos();
                    projectile.velocity = (cos * LANCE_SPEED, sin * LANCE_SPEED);
                    projectile.dirty = true;
                }
                projectile.rotation = projectile.local_ai[1];
            }
            173 => {
                // Her lasting rainbow: a constant curve that eases in over thirty ticks and then
                // holds half a degree a tick for the rest of its life (`:46264-46275`). It is what
                // makes the trail an arc rather than a line, and it flew dead straight here.
                let turn = projectile.ai[0];
                let (sin, cos) = turn.sin_cos();
                let v = projectile.velocity;
                projectile.velocity = (v.0 * cos - v.1 * sin, v.0 * sin + v.1 * cos);
                if projectile.ai[0] < RAINBOW_TURN {
                    projectile.ai[0] += RAINBOW_TURN / RAINBOW_EASE;
                }
                projectile.rotation = projectile.velocity.1.atan2(projectile.velocity.0)
                    + std::f32::consts::FRAC_PI_2;
            }
            58 => {
                // A present dropped by the Frost Moon's minibosses (`Projectile.cs:29337-29366`).
                // Two phases, and the first is what makes it read as *dropped* rather than thrown:
                // it holds whatever upward speed it was given for half a second, then takes a tenth
                // of a pixel a tick until it is falling at all, and only then commits to the second
                // phase and its slow terminal of three. A present that fell like a rock would be
                // impossible to catch.
                if projectile.ai[0] == 0.0 {
                    projectile.ai[1] += 1.0;
                    if projectile.ai[1] > PRESENT_DRIFT {
                        projectile.velocity.1 += PRESENT_GRAVITY;
                    }
                    if projectile.velocity.1 >= 0.0 {
                        projectile.ai[0] = 1.0;
                    }
                }
                if projectile.ai[0] == 1.0 {
                    projectile.velocity.1 =
                        (projectile.velocity.1 + PRESENT_GRAVITY).min(PRESENT_TERMINAL);
                    projectile.velocity.0 *= PRESENT_DRAG;
                }
                projectile.rotation = projectile.velocity.1.atan2(projectile.velocity.0)
                    + std::f32::consts::FRAC_PI_2;
            }
            68 => {
                // The Tavernkeep's ale (`Projectile.cs:30655-30677`): fifteen ticks flat, then an
                // ordinary fall with a light drag. The spin is `* direction` in vanilla, which a
                // projectile does not carry here; see the style-2 arm for the same note.
                projectile.rotation += ALE_SPIN;
                projectile.ai[0] += 1.0;
                if projectile.ai[0] >= ALE_DELAY {
                    projectile.velocity.1 = (projectile.velocity.1 + ALE_GRAVITY).min(TERMINAL);
                    projectile.velocity.0 *= ALE_DRAG;
                }
            }
            183 => {
                // The Zoologist's claw (`Projectile.cs:43893-43907`, `AI_183_ZoologistStrike`).
                // Four lines, three of them facing, and the fourth is the whole point: it keeps a
                // fifth of its sideways speed each tick and never falls. The drag runs before the
                // move, as vanilla's `AI()` does, so a swipe thrown at twenty-four pixels a tick
                // travels 24/5 + 24/25 + ... = **six pixels in total** and then dies at eighteen
                // ticks. It is a claw at arm's length, not a projectile. Without this it crossed
                // four hundred and thirty, which turned the shortest-ranged attack in the roster
                // (`DangerDetectRange[633]` is 100, the smallest there is) into one of the longest.
                projectile.velocity.0 *= ZOOLOGIST_DRAG;
                projectile.velocity.1 = 0.0;
            }
            SHARKNADO_STYLE if projectile.ai[1] == 0.0 => {
                // The Duke's mouth bubbles (`Projectile.cs:30058-30078`), the half of style 65
                // that is *not* seeking. A zero `ai[1]` is what says so, and vanilla writes the
                // bob as a difference rather than an absolute: it subtracts the offset its current
                // tick number implies, advances the tick, and adds the new one back, so the
                // vertical speed traces a cosine around whatever it was launched with rather than
                // being overwritten by one. The period is thirty ticks.
                //
                // The seeking half needs the player list and lives in
                // `systems::tick_sharknado_bolts`.
                let was = (BOLT_BOB_RATE * projectile.ai[0]).cos() - 0.5;
                projectile.velocity.1 -= was * BOLT_BOB_AMPLITUDE;
                projectile.ai[0] += 1.0;
                let now = (BOLT_BOB_RATE * projectile.ai[0]).cos() - 0.5;
                projectile.velocity.1 += now * BOLT_BOB_AMPLITUDE;
                // `if (wet) { position.Y -= 16f; Kill(); }` is not modelled: a projectile here
                // carries no wetness, and the Duke fights over the ocean, so this is the one
                // narrowing in the arm rather than an oversight.
            }
            135 => {
                // The Queen Slime's ground smash (`Projectile.cs:69740-69756`,
                // `AI_135_OgreStomp`). It does not travel and it does not last: nine ticks, and
                // **its whole point is that it grows its own hitbox** from five tiles across to
                // thirty, centred on the spot it was dropped. Vanilla stashes the centre, resizes,
                // and puts the centre back, so the box widens both ways rather than growing off
                // its top-left corner. Without it the smash was a thirty-pixel box that flew off
                // at whatever it was launched with and hung about for its table's 120: a boss's
                // shockwave you could stand next to.
                //
                // The Ogre shares this style at forty tiles; only 922 reaches it here, and the
                // per-type number is transcribed rather than folded away.
                projectile.ai[0] += 1.0;
                if projectile.ai[0] > SMASH_TICKS {
                    return Outcome::Spent;
                }
                projectile.velocity = (0.0, 0.0);
                let centre = projectile.center();
                let across = TILE
                    * (SMASH_FROM + (SMASH_TO - SMASH_FROM) * (projectile.ai[0] / SMASH_TICKS));
                projectile.stats.width = across as i32;
                projectile.stats.height = across as i32;
                projectile.position = (
                    centre.0 - projectile.width() / 2.0,
                    centre.1 - projectile.height() / 2.0,
                );
                projectile.dirty = true;
            }
            157 => {
                // The Deerclops ice spike (`Projectile.cs:52268-52400`, `AI_157_SharpTears`).
                // It never touches its velocity - it is launched with a *direction*, a unit
                // vector, so it creeps a pixel a tick - and everything else in the method is an
                // opacity envelope and a scale. What matters is the clock: it fades in over ten
                // ticks, fades out from ten, and is gone at twenty.
                //
                // The flags are read *before* the increment, which is why the last live tick is
                // the one that reads twenty rather than the one that reaches it. Ours was launched
                // with three hundred, so a wall of twenty spikes stood for five seconds and drifted
                // three hundred pixels upward while it did.
                let ending = projectile.ai[0] >= SPIKE_LIFE;
                projectile.ai[0] += 1.0;
                if ending {
                    return Outcome::Spent;
                }
            }
            186 => {
                // The Princess's weapon (`Projectile.cs:43454-43462`, `AI_186_PrincessWeapon`).
                // Everything in that method except its first four lines is drawing - opacity, an
                // eased scale envelope, four kinds of dust and a particle burst - and the four that
                // are not say it lives sixty ticks. Its own table says 180, so nothing but the arm
                // knows the real number.
                projectile.ai[0] += 1.0;
                if projectile.ai[0] >= PRINCESS_WEAPON_LIFE {
                    return Outcome::Spent;
                }
            }
            112 if projectile.projectile_type == TRUFFLE_SPORE => {
                // The Truffle's spore (`Projectile.cs:34822-34865`). Style 112 is three unrelated
                // bodies under one number, keyed on the type inside the arm exactly as vanilla
                // keys them, and this is the only one the server puts in the air today: a spore
                // that **does not travel at all**. Its velocity is overwritten every tick with a
                // pure vertical bob, a sine over three seconds at fifteen hundredths of a pixel,
                // so it hangs where the Truffle put it for its whole nine hundred ticks and
                // anything standing in it keeps taking the forty.
                projectile.velocity = (
                    0.0,
                    (std::f32::consts::TAU * projectile.ai[0] / SPORE_PERIOD).sin() * SPORE_BOB,
                );
                projectile.ai[0] += 1.0;
                if projectile.ai[0] >= SPORE_PERIOD {
                    projectile.ai[0] = 0.0;
                }
            }
            10 => {
                // A lob: it falls from the moment it leaves, and slows as it goes.
                projectile.velocity.1 = (projectile.velocity.1 + 0.41).min(TERMINAL);
                projectile.velocity.0 *= 0.98;
                projectile.rotation += 0.1;
            }
            14 => {
                // A rolling ball. It is given five ticks of free flight, then falls; once it is
                // running along the ground it scrubs off speed until it stops.
                projectile.ai[0] += 1.0;
                if projectile.ai[0] > 5.0 {
                    projectile.ai[0] = 5.0;
                    if projectile.velocity.1 == 0.0 && projectile.velocity.0 != 0.0 {
                        projectile.velocity.0 *= 0.97;
                        if projectile.velocity.0.abs() < 0.01 {
                            projectile.velocity.0 = 0.0;
                        }
                    }
                    projectile.velocity.1 += 0.2;
                }
                projectile.rotation += projectile.velocity.0 * 0.1;
            }
            18 => {
                projectile.rotation += SCYTHE_SPIN;
                projectile.ai[0] += 1.0;
                // The window that makes a demon scythe frightening.
                if (SCYTHE_FROM..SCYTHE_UNTIL).contains(&projectile.ai[0]) {
                    projectile.velocity.0 *= SCYTHE_ACCEL;
                    projectile.velocity.1 *= SCYTHE_ACCEL;
                }
            }
            23 => {
                // A flame is never alight for more than a second, however it was launched.
                projectile.time_left = projectile.time_left.min(60);
            }
            37 => {
                if spear(projectile, tiles) == Outcome::Spent {
                    return Outcome::Spent;
                }
            }
            38 => {
                // A flamethrower does no damage of its own: it is the thing holding the flame.
                projectile.ai[0] += 1.0;
                if projectile.ai[0] >= FLAME_EVERY {
                    projectile.ai[0] = 0.0;
                    emits.push(Emission {
                        projectile_type: FLAME,
                        position: projectile.center(),
                        velocity: projectile.velocity,
                        damage: projectile.damage,
                    });
                }
            }
            BOULDER_STYLE => boulder(projectile, tiles),
            126 => {
                // A geyser: it rises out of its vent until the way ahead is clear, then stops
                // and hangs there. If it never gets clear it dies inside the wall.
                if projectile.ai[0] == 0.0 {
                    let up = projectile.velocity.1 < 0.0;
                    let probe = (
                        projectile.position.0,
                        projectile.position.1 + if up { projectile.height() - 48.0 } else { 0.0 },
                    );
                    if !hits_terrain(tiles, probe, (projectile.width(), 48.0)) {
                        projectile.velocity = (0.0, if up { -0.001 } else { 0.001 });
                        projectile.ai[0] = 1.0;
                        projectile.ai[1] = 0.0;
                        projectile.time_left = 60;
                    } else {
                        projectile.ai[1] += 1.0;
                        if projectile.ai[1] >= 60.0 {
                            return Outcome::Spent;
                        }
                    }
                }
            }
            _ => {
                // Everything else flies straight and simply faces the way it is going.
                projectile.rotation = projectile.velocity.1.atan2(projectile.velocity.0) + 1.57;
            }
        }

        // Movement at full velocity, one pass at a time.
        let size = (projectile.width(), projectile.height());
        let next = (
            projectile.position.0 + projectile.velocity.0,
            projectile.position.1 + projectile.velocity.1,
        );
        // Three styles change their own `tileCollide` mid-flight. Vanilla flips the projectile's
        // own field; ours lives in the shared stats table, so the condition is read here instead.
        //
        // A falling star does not collide until it has been clear of terrain once, which its arm
        // latches into `ai[1]` above (`Projectile.cs:24170-24176`). A Saucer missile is the other
        // way round: it starts with collision *off* so it can leave the hull it was fired from, and
        // `tileCollide = true` is the first thing its homing phase does (`:31473`). That phase is
        // driven from `systems::tick_saucer_missiles`, which is why only the flag it sets is read
        // here. The Mechanic's wrench is the Saucer's shape again: it collides on the way out and
        // stops on the way home (`:34676`), so it cannot be stopped by the wall it is coming back
        // through.
        let collides = match projectile.stats.ai_style {
            STAR_STYLE => projectile.stats.tile_collide && projectile.ai[1] != 0.0,
            SAUCER_MISSILE_STYLE => projectile.ai[0] == 1.0,
            WRENCH_STYLE => projectile.stats.tile_collide && projectile.ai[0] == 0.0,
            _ => projectile.stats.tile_collide,
        };
        if collides {
            if projectile.stats.ai_style == WRENCH_STYLE {
                // A boomerang that meets a wall does not stop and does not die: it turns round
                // there and starts its return early (`Projectile.cs:19677-19684`, the shared
                // `aiStyle 3 || 13 || 69 || 109` collision block). `ai[0] = 1f` is the switch and
                // `velocity = -lastVelocity` is the turn, on both axes at once rather than per
                // axis - so a wrench that clips a floor comes back the way it went out rather than
                // skidding along it.
                if hits_terrain(tiles, next, size) {
                    projectile.ai[0] = 1.0;
                    projectile.velocity = (-projectile.velocity.0, -projectile.velocity.1);
                    projectile.dirty = true;
                } else {
                    projectile.position = next;
                }
            } else if projectile.stats.ai_style == 14 {
                // A rolling ball bounces off what it hits instead of dying on it, and each axis is
                // settled on its own: a ball that lands on a floor keeps travelling along it, which
                // is why a spiky ball trap fills a corridor rather than a doorway. Rolling balls
                // carry no extra updates and move well under a tile a step, so a plain destination
                // check settles them without any sweep.
                if hits_terrain(tiles, next, size) {
                    let sideways = (next.0, projectile.position.1);
                    let downward = (projectile.position.0, next.1);
                    let across = !hits_terrain(tiles, sideways, size);
                    let down = !hits_terrain(tiles, downward, size);
                    let (moved, bounce_x, bounce_y) = match (across, down) {
                        (true, false) => (sideways, false, true),
                        (false, true) => (downward, true, false),
                        // Neither axis is free, or both are free alone but not together, a corner.
                        // It comes off both and stays where it is for this step.
                        _ => (projectile.position, true, true),
                    };
                    projectile.position = moved;
                    if bounce_x {
                        projectile.velocity.0 *= BOUNCE;
                    }
                    if bounce_y {
                        projectile.velocity.1 *= BOUNCE;
                    }
                } else {
                    projectile.position = next;
                }
            } else if projectile.stats.ai_style == BOMB_STYLE {
                // A bomb bounces off what it hits rather than dying on it, at two fifths of the
                // speed it arrived with (`Projectile.cs:19872-19886`). The vertical bounce has a
                // floor: a bomb that has almost stopped falling settles instead of hopping for
                // ever, which is `lastVelocity.Y > 0.7`. Without any of this a grenade thrown at a
                // wall vanished on contact rather than dropping at its feet and going off.
                let last = projectile.velocity;
                if !hits_terrain(tiles, next, size) {
                    projectile.position = next;
                } else {
                    let across = !hits_terrain(tiles, (next.0, projectile.position.1), size);
                    let down = !hits_terrain(tiles, (projectile.position.0, next.1), size);
                    // As the rolling ball and the boulder above: both axes free alone but not
                    // together is a corner, and comes off both.
                    let (moved, hit_x, hit_y) = match (across, down) {
                        (true, false) => ((next.0, projectile.position.1), false, true),
                        (false, true) => ((projectile.position.0, next.1), true, false),
                        _ => (projectile.position, true, true),
                    };
                    projectile.position = moved;
                    if hit_x {
                        projectile.velocity.0 = last.0 * BOMB_BOUNCE;
                    }
                    if hit_y {
                        projectile.velocity.1 = if last.1 > BOMB_BOUNCE_FLOOR {
                            last.1 * BOMB_BOUNCE
                        } else {
                            0.0
                        };
                    }
                }
            } else if projectile.stats.ai_style == BOULDER_STYLE {
                // A boulder slides rather than dying: `Collision.TileCollision` zeroes whichever
                // axis is blocked, and only then does the reaction at `Projectile.cs:19056-19099`
                // read which one moved. A hard landing hops at a fifth of the speed it arrived
                // with, and a boulder that was going sideways and is not any more has hit a wall,
                // which kills it. Falling straight onto a floor leaves `velocity.X` unchanged at
                // zero, so it is not a wall and the boulder lives to roll.
                let last = projectile.velocity;
                let (moved, blocked_x, blocked_y) = if !hits_terrain(tiles, next, size) {
                    (next, false, false)
                } else {
                    let across = !hits_terrain(tiles, (next.0, projectile.position.1), size);
                    let down = !hits_terrain(tiles, (projectile.position.0, next.1), size);
                    match (across, down) {
                        (true, false) => ((next.0, projectile.position.1), false, true),
                        (false, true) => ((projectile.position.0, next.1), true, false),
                        // Neither axis is free, or both are free alone but not together: a corner.
                        // Disclosed narrowing, shared with the rolling ball above: vanilla's
                        // `Collision.TileCollision` sweeps the box and clips whichever axis really
                        // intersects, so a diagonal clip of an inside corner takes one axis there
                        // and both here. For a boulder that means it can break on a corner vanilla
                        // would have let it round. Closing it wants the real swept collision rather
                        // than a per-axis probe, which is a change to every style, not this one.
                        _ => (projectile.position, true, true),
                    }
                };
                projectile.position = moved;
                if blocked_y {
                    projectile.velocity.1 = if last.1 > BOULDER_HOP_FROM {
                        last.1 * BOULDER_HOP
                    } else {
                        0.0
                    };
                }
                if blocked_x {
                    projectile.velocity.0 = 0.0;
                    // `if (velocity.X != lastVelocity.X)` and nothing more: a boulder that was not
                    // travelling sideways cannot have had its sideways travel stopped, so vanilla
                    // never reaches the kill for one. That matters at the moment of release, when a
                    // statue drops its boulder overlapping the floor it is standing on and both
                    // axes read as blocked.
                    if last.0 != 0.0 {
                        return Outcome::Spent;
                    }
                }
            } else {
                // Everything else dies on what it hits. Swept so a fast pass cannot skip a wall.
                let (moved, hit) = advance(projectile.position, next, size, tiles);
                projectile.position = moved;
                if hit {
                    return Outcome::Spent;
                }
            }
        } else {
            projectile.position = next;
        }

        // The time budget is spent once per pass, exactly where the game spends it
        // (`Projectile.cs:17308`), so a projectile with extra updates lives the right length of
        // time rather than (N+1) times too long.
        projectile.time_left -= 1;
        if projectile.time_left <= 0 {
            return Outcome::Spent;
        }
    }

    projectile.dirty = true;
    Outcome::Flying
}

/// A bomb, a mine or a grenade: it arcs, lands, and settles or rolls depending on which it is.
///
/// `Projectile.AI_016_Bombs` (`Projectile.cs:47764-48409`). The routine is nine tenths dust, sound
/// and per-type fuses; what a server needs is the last eighty lines, and they split the family in
/// two. A **mine** takes gravity and one drag on *both* axes from the first tick and stops dead
/// where it lands, which is what makes it a mine. **Everything else** keeps its fuse: nothing at
/// all happens until `ai[0]` passes it, and then it falls, and rolls along whatever it landed on
/// until it is under a hundredth of a pixel a tick.
///
/// Two of vanilla's four families are not reached from here and so are not transcribed: the
/// `133`-family's fifteen-tick fuse and the `134`-family, which does not fall at all and freezes
/// on contact instead. Nothing this server launches is either.
fn bomb(p: &mut Projectile) {
    // `ai[0]++` (`Projectile.cs:48333`), before any of the branches read it.
    p.ai[0] += 1.0;
    match p.projectile_type {
        // `type == 135 || 138 || 141 || 144 || 778 || 782 || 795 || 798 || 801 || 786 || 789 || 792`
        // (`Projectile.cs:48354`): the proximity mines.
        135 | 138 | 141 | 144 | 778 | 782 | 786 | 789 | 792 | 795 | 798 | 801 => {
            p.velocity.1 += BOMB_GRAVITY;
            p.velocity.0 *= MINE_DRAG;
            p.velocity.1 *= MINE_DRAG;
            if p.velocity.0.abs() < MINE_STOP {
                p.velocity.0 = 0.0;
            }
            if p.velocity.1.abs() < MINE_STOP {
                p.velocity.1 = 0.0;
            }
        }
        other => {
            // The long fuse is `type == 30 || 397 || 517 || 681 || 588 || 779 || 783 || 862 || 863
            // || 1088`; everything else waits five ticks rather than ten
            // (`Projectile.cs:48378-48399`).
            let fuse = if matches!(
                other,
                30 | 397 | 517 | 588 | 681 | 779 | 783 | 862 | 863 | 1088
            ) {
                BOMB_LONG_FUSE
            } else {
                BOMB_SHORT_FUSE
            };
            if p.ai[0] > fuse {
                // Vanilla pins the counter here, so a bomb's `ai[0]` never climbs past ten however
                // long it lies there.
                p.ai[0] = BOMB_LONG_FUSE;
                if p.velocity.1 == 0.0 && p.velocity.0 != 0.0 {
                    p.velocity.0 *= BOMB_ROLL_DRAG;
                    if p.velocity.0.abs() < BOMB_ROLL_STOP {
                        p.velocity.0 = 0.0;
                    }
                }
                p.velocity.1 += BOMB_GRAVITY;
            }
        }
    }
    p.rotation += p.velocity.0 * BOMB_SPIN;
}

/// A boulder: it falls, and once it has landed it rolls away from whichever side has a wall.
///
/// `Projectile.cs:26596-26746`. The arm serves seven boulders in the game and this transcribes the
/// shared body: the per-type branches belong to the Shimmer boulder (1055), the Moon boulder (1021)
/// and four cosmetic ones, none of which anything here launches.
///
/// The ledge probe is the whole character of the thing. A boulder that has come to rest with no
/// sideways speed looks eight pixels left and eight right, at its own top row and the one below;
/// whichever side has a wall is the side it rolls *away* from, so a boulder dropped into a corridor
/// runs down it rather than sitting there. Failing that it looks again a tile further out, then two,
/// and if nothing is walled at any reach the boulder's own column decides, which stops two boulders
/// released side by side from always going the same way.
fn boulder(p: &mut Projectile, tiles: &impl TileView) {
    // `localAI[2]++` (`Projectile.cs:26361`), which is only bookkeeping in vanilla until
    // `CanDamage` reads it. See [`Projectile::can_damage`].
    p.local_ai[2] += 1.0;

    if p.ai[0] != 0.0 && p.velocity.1 <= 0.0 && p.velocity.0 == 0.0 {
        let row = (p.position.1 / TILE) as i32;
        let walled = |x: i32| blocking(tiles, x, row) || blocking(tiles, x, row + 1);
        let mut rolled = false;
        for reach in [0.0, TILE, TILE * 2.0] {
            let left = ((p.position.0 - 8.0 - reach) / TILE) as i32;
            let right = ((p.position.0 + p.width() + 8.0 + reach) / TILE) as i32;
            if walled(left) {
                p.velocity.0 = BOULDER_NUDGE;
            } else if walled(right) {
                p.velocity.0 = -BOULDER_NUDGE;
            } else {
                continue;
            }
            rolled = true;
            break;
        }
        if !rolled {
            // `if ((int)(base.Center.X / 16f) % 2 == 0)` at the widest reach only.
            p.velocity.0 = if (p.center().0 / TILE) as i32 % 2 == 0 {
                BOULDER_NUDGE
            } else {
                -BOULDER_NUDGE
            };
        }
    }

    p.rotation += p.velocity.0 * BOULDER_SPIN;
    // Set unconditionally at the end of every tick, so the probe above never runs on the first one:
    // a boulder is falling out of a statue then, not resting on anything.
    p.ai[0] = 1.0;
    p.velocity.1 = p.velocity.1.min(TERMINAL);
    if p.velocity.1 <= BOULDER_ROLLING_FALL {
        if p.velocity.0 > 0.0 && p.velocity.0 < BOULDER_TOP_SPEED {
            p.velocity.0 += BOULDER_ACCEL;
        }
        if p.velocity.0 < 0.0 && p.velocity.0 > -BOULDER_TOP_SPEED {
            p.velocity.0 -= BOULDER_ACCEL;
        }
    }
    p.velocity.1 += BOULDER_GRAVITY;
}

/// A spear growing out of its trap: it reaches, then pulls back, then dies at home.
///
/// The anchor is set a step and a half behind where it started, because the spear is drawn from
/// there and the retraction has to know where "home" is.
fn spear(projectile: &mut Projectile, tiles: &impl TileView) -> Outcome {
    if projectile.ai[1] == 0.0 {
        projectile.ai[1] = 1.0;
        let centre = projectile.center();
        projectile.local_ai[0] = centre.0 - projectile.velocity.0 * 1.5;
        projectile.local_ai[1] = centre.1 - projectile.velocity.1 * 1.5;
    }
    let anchor = (projectile.local_ai[0], projectile.local_ai[1]);
    let centre = projectile.center();
    let (dx, dy) = (centre.0 - anchor.0, centre.1 - anchor.1);
    let reach = (dx * dx + dy * dy).sqrt();
    projectile.rotation = dy.atan2(dx) - std::f32::consts::FRAC_PI_2;

    let size = (projectile.width(), projectile.height());
    let solid = hits_terrain(tiles, projectile.position, size);
    if projectile.ai[0] == 0.0 {
        if solid || reach > SPEAR_REACH {
            projectile.velocity.0 = -projectile.velocity.0;
            projectile.velocity.1 = -projectile.velocity.1;
            projectile.ai[0] += 1.0;
        }
        return Outcome::Flying;
    }
    let speed = (projectile.velocity.0 * projectile.velocity.0
        + projectile.velocity.1 * projectile.velocity.1)
        .sqrt();
    if solid || reach < speed {
        return Outcome::Spent;
    }
    Outcome::Flying
}

/// The fixed table of projectile slots.
#[derive(Debug)]
pub struct ProjectileStore {
    slots: Vec<Option<Projectile>>,
    /// The generation the projectile most recently in each slot carried.
    ///
    /// Per slot, as the game keeps it: `Projectile.NewProjectile` builds its key from
    /// `++slotGenerations[num]`, indexed by the slot being taken. One counter shared by every slot
    /// — which this used to be — repeats a value after 16384 launches *anywhere*, where per slot it
    /// takes 16384 reuses *of that slot*. On a busy server with a thousand slots in play that is
    /// three orders of magnitude sooner, and the whole point of the generation is that a kill
    /// packet arriving a moment late fails to match rather than destroying a bystander.
    ///
    /// No guard against zero here, unlike the NPC store: the game does not have one either, and
    /// nothing asserts on a projectile generation of nought.
    slot_generations: Vec<u16>,
}

impl Default for ProjectileStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectileStore {
    pub fn new() -> Self {
        Self {
            slots: (0..MAX_PROJECTILES).map(|_| None).collect(),
            slot_generations: vec![0; MAX_PROJECTILES],
        }
    }

    /// Launch one, returning its slot.
    pub fn launch(
        &mut self,
        projectile_type: u16,
        position: (f32, f32),
        velocity: (f32, f32),
        damage: i32,
        time_left: i32,
    ) -> Option<u16> {
        // Loud, because the symptom of a silent miss here is a boss that fights with no attacks
        // and reports nothing wrong. Thirty-two types were absent from the stats table and every
        // shot of theirs was dropped on this line without a word.
        let Some(stats) = projectile_stats(projectile_type) else {
            tracing::warn!(
                projectile_type,
                "no stats for this projectile; the shot was not fired"
            );
            return None;
        };
        let index = self.slots.iter().position(Option::is_none)?;
        // Fourteen bits on the wire, so the counter is kept inside them rather than truncated at
        // the point of packing.
        let generation = match self.slot_generations.get_mut(index) {
            Some(slot) => {
                *slot = slot.wrapping_add(1) & 0x3FFF;
                *slot
            }
            None => 1,
        };
        let projectile = Projectile {
            key: ProjectileKey {
                owner: SERVER_OWNER,
                index: index as u16,
                generation,
            },
            projectile_type,
            // The aim point is the centre; the entity is placed by its corner.
            position: (
                position.0 - stats.width as f32 / 2.0,
                position.1 - stats.height as f32 / 2.0,
            ),
            velocity,
            // The base damage, unscaled. The game keeps a hostile shot's wire damage at its base
            // and applies `hostileDamageScaling(difficulty) * 2` on the client at the moment it hits
            // a player (`Projectile.cs:14916-14919`), so the server must not bake the difficulty
            // multiplier in here: a client that received a pre-scaled number would scale it a second
            // time. The server applies the same scaling itself when it originates the hit (see
            // `tick_contact_damage`).
            damage,
            knockback: stats.knockback,
            ai: [0.0; 3],
            local_ai: [0.0; 4],
            rotation: 0.0,
            time_left: if time_left > 0 {
                time_left
            } else {
                stats.time_left
            },
            penetrate: stats.penetrate,
            stats,
            dirty: true,
        };
        self.slots[index] = Some(projectile);
        Some(index as u16)
    }

    pub fn get(&self, index: u16) -> Option<&Projectile> {
        self.slots.get(index as usize)?.as_ref()
    }

    pub fn get_mut(&mut self, index: u16) -> Option<&mut Projectile> {
        self.slots.get_mut(index as usize)?.as_mut()
    }

    pub fn remove(&mut self, index: u16) -> Option<Projectile> {
        self.slots.get_mut(index as usize)?.take()
    }

    pub fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, &Projectile)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_ref().map(|p| (i as u16, p)))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u16, &mut Projectile)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(i, p)| p.as_mut().map(|p| (i as u16, p)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    #[derive(Default)]
    struct Air(HashMap<(i32, i32), Tile>);

    impl TileView for Air {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn launched(projectile_type: u16, velocity: (f32, f32)) -> Projectile {
        let mut store = ProjectileStore::new();
        let index = store
            .launch(projectile_type, (1000.0, 1000.0), velocity, 15, 0)
            .expect("a known type");
        *store.get(index).unwrap()
    }

    #[test]
    fn a_launch_gets_a_slot_and_a_fresh_generation() {
        let mut store = ProjectileStore::new();
        let first = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
        let a = store.get(first).unwrap().key;
        store.remove(first);
        let second = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
        let b = store.get(second).unwrap().key;
        assert_eq!(a.index, b.index, "the slot is reused");
        assert_ne!(
            a.generation, b.generation,
            "but a stale kill packet must not match it"
        );
    }

    /// The generation counts reuses of a slot, not launches anywhere.
    ///
    /// The game builds its key from `++slotGenerations[num]`, indexed by the slot. One counter for
    /// the whole store repeats a value after 16384 launches *anywhere*, where per slot it takes
    /// 16384 reuses *of that slot* — and the generation exists precisely so that a late kill packet
    /// fails to match rather than destroying whatever took the slot since.
    #[test]
    fn projectile_generation_counts_reuses_of_a_slot() {
        let mut store = ProjectileStore::new();
        let first = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
        let second = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
        assert_ne!(first, second, "two slots");
        assert_eq!(store.get(first).unwrap().key.generation, 1);
        assert_eq!(
            store.get(second).unwrap().key.generation,
            1,
            "a second slot's first occupant is still its first"
        );

        store.remove(first);
        let again = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
        assert_eq!(again, first, "the freed slot is taken back");
        assert_eq!(store.get(again).unwrap().key.generation, 2);
        assert_eq!(
            store.get(second).unwrap().key.generation,
            1,
            "the untouched slot should not have moved"
        );
    }

    /// The generation stays inside the fourteen bits the wire gives it.
    ///
    /// Packing masks with `0x3FFF`, so a counter allowed past that silently starts repeating early
    /// — the value 16385 packs identically to 1, and a stale kill packet matches a live projectile.
    #[test]
    fn projectile_generation_stays_inside_its_fourteen_bits() {
        let mut store = ProjectileStore::new();
        for _ in 0..40_000 {
            let index = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
            let key = store.get(index).unwrap().key;
            assert!(
                key.generation <= 0x3FFF,
                "generation {} does not fit the wire",
                key.generation
            );
            // What is packed is what comes back.
            assert_eq!(ProjectileKey::unpack(key.pack()), key);
            store.remove(index);
        }
    }

    #[test]
    fn a_launch_is_centred_on_its_aim_point() {
        let p = launched(38, (6.0, 0.0));
        let centre = p.center();
        assert!((centre.0 - 1000.0).abs() < 0.01);
        assert!((centre.1 - 1000.0).abs() < 0.01);
    }

    /// The arc is the whole character of a thrown feather: flat, then falling.
    #[test]
    fn a_feather_flies_flat_and_then_falls() {
        let tiles = Air::default();
        let mut p = launched(38, (6.0, 0.0));
        for _ in 0..(ARC_DELAY as i32 - 1) {
            step(&mut p, &tiles, &mut Vec::new());
        }
        assert_eq!(p.velocity.1, 0.0, "still flat");
        for _ in 0..30 {
            step(&mut p, &tiles, &mut Vec::new());
        }
        assert!(p.velocity.1 > 0.0, "now falling, got {}", p.velocity.1);
    }

    #[test]
    fn nothing_falls_faster_than_terminal() {
        let tiles = Air::default();
        let mut p = launched(38, (0.0, 0.0));
        for _ in 0..2000 {
            if step(&mut p, &tiles, &mut Vec::new()) == Outcome::Spent {
                break;
            }
        }
        assert!(p.velocity.1 <= TERMINAL);
    }

    /// A demon's scythe is slow when it leaves and lethal when it arrives.
    #[test]
    fn a_demon_scythe_speeds_up_on_the_way_to_you() {
        let tiles = Air::default();
        let mut p = launched(44, (0.2, 0.0));
        let leaving = p.velocity.0.abs();
        for _ in 0..(SCYTHE_UNTIL as i32) {
            step(&mut p, &tiles, &mut Vec::new());
        }
        let arriving = p.velocity.0.abs();
        assert!(
            arriving > leaving * 10.0,
            "should have picked up speed: {leaving} to {arriving}"
        );
    }

    #[test]
    fn a_scythe_stops_accelerating_eventually() {
        let tiles = Air::default();
        let mut p = launched(44, (0.2, 0.0));
        for _ in 0..(SCYTHE_UNTIL as i32) {
            step(&mut p, &tiles, &mut Vec::new());
        }
        let settled = p.velocity.0;
        for _ in 0..50 {
            step(&mut p, &tiles, &mut Vec::new());
        }
        assert_eq!(p.velocity.0, settled, "it does not accelerate forever");
    }

    #[test]
    fn terrain_stops_what_it_should_and_not_what_it_should_not() {
        let mut tiles = Air::default();
        for y in 0..200 {
            tiles.0.insert((64, y), Tile::block(1));
        }
        // A feather collides.
        let mut feather = launched(38, (16.0, 0.0));
        let mut stopped = false;
        for _ in 0..40 {
            if step(&mut feather, &tiles, &mut Vec::new()) == Outcome::Spent {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "a feather should hit the wall");

        // A skull passes straight through.
        let mut skull = launched(299, (16.0, 0.0));
        for _ in 0..30 {
            assert_eq!(step(&mut skull, &tiles, &mut Vec::new()), Outcome::Flying);
        }
    }

    #[test]
    fn a_projectile_runs_out_of_time() {
        let tiles = Air::default();
        let mut p = launched(38, (0.0, 0.0));
        p.time_left = 3;
        assert_eq!(step(&mut p, &tiles, &mut Vec::new()), Outcome::Flying);
        assert_eq!(step(&mut p, &tiles, &mut Vec::new()), Outcome::Flying);
        assert_eq!(step(&mut p, &tiles, &mut Vec::new()), Outcome::Spent);
    }

    /// Extra updates run the whole body again, at full velocity, so a fast type covers (N+1) times
    /// its per-pass velocity a tick and its life ticks down (N+1) times. The old code moved
    /// velocity/N and spent one tick of life a frame, so these types crawled and overstayed.
    #[test]
    fn extra_updates_move_full_velocity_and_spend_life_each_pass() {
        let tiles = Air::default();
        // The eye laser carries two extra updates: three passes a tick.
        let mut laser = launched(83, (0.0, -3.0));
        assert_eq!(laser.stats.extra_updates, 2);
        let start_y = laser.position.1;
        let start_life = laser.time_left;
        step(&mut laser, &tiles, &mut Vec::new());
        assert!(
            (laser.position.1 - (start_y - 9.0)).abs() < 0.001,
            "three passes of -3 should move -9, not {}",
            laser.position.1 - start_y
        );
        assert_eq!(
            laser.time_left,
            start_life - 3,
            "three passes should each spend a tick of life"
        );
    }

    /// Fast projectiles move in substeps, which is what stops them tunnelling.
    #[test]
    fn a_fast_projectile_cannot_tunnel_through_a_single_tile_wall() {
        let mut tiles = Air::default();
        for y in 0..200 {
            tiles.0.insert((70, y), Tile::block(1));
        }
        // The eye laser has two extra updates and moves nine pixels a step.
        let mut laser = launched(83, (30.0, 0.0));
        assert!(laser.stats.extra_updates > 0);
        let mut stopped = false;
        for _ in 0..40 {
            if step(&mut laser, &tiles, &mut Vec::new()) == Outcome::Spent {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "it should not have passed through");
    }

    #[test]
    fn overlap_is_measured_from_the_corners() {
        let p = launched(38, (0.0, 0.0));
        assert!(p.overlaps((1000.0, 1000.0), (4.0, 4.0)));
        assert!(!p.overlaps((2000.0, 2000.0), (4.0, 4.0)));
    }

    #[test]
    fn an_unknown_type_will_not_launch() {
        let mut store = ProjectileStore::new();
        assert!(store.launch(60_000, (0.0, 0.0), (1.0, 0.0), 1, 0).is_none());
    }

    /// A spiky ball bounces off a wall instead of dying on it, which is the whole point of the
    /// trap that throws them.
    #[test]
    fn a_spiky_ball_bounces_off_a_wall() {
        let mut tiles = Air::default();
        for y in 0..200 {
            tiles.0.insert((64, y), Tile::block(1));
        }
        // Thrown east, at a wall four tiles away.
        let mut ball = launched(185, (6.0, 0.0));
        let mut bounced = false;
        for _ in 0..40 {
            assert_eq!(
                step(&mut ball, &tiles, &mut Vec::new()),
                Outcome::Flying,
                "a spiky ball should never die on a wall"
            );
            if ball.velocity.0 < 0.0 {
                bounced = true;
                break;
            }
        }
        assert!(bounced, "it never turned round: {:?}", ball.velocity);
        assert!(
            ball.velocity.0 < -5.0 && ball.velocity.0 > -6.0,
            "it should keep nine tenths of its speed, not {}",
            ball.velocity.0
        );
    }

    /// A ball that lands on a floor keeps travelling along it: the axis that was blocked bounces,
    /// the one that was free carries on.
    ///
    /// This is what makes a spiky ball trap fill a corridor rather than pile its balls up under
    /// itself, and it is the only reason the things are worth wiring.
    #[test]
    fn a_spiky_ball_rolls_along_the_floor_it_lands_on() {
        let mut tiles = Air::default();
        for x in 0..400 {
            tiles.0.insert((x, 64), Tile::block(1));
        }
        let mut ball = launched(185, (3.0, 0.0));
        let start = ball.position.0;
        for _ in 0..400 {
            if step(&mut ball, &tiles, &mut Vec::new()) == Outcome::Spent {
                break;
            }
        }
        assert!(
            ball.position.0 - start > 200.0,
            "it should have rolled a good way east, not {} pixels",
            ball.position.0 - start
        );
        assert!(
            ball.position.1 < 64.0 * 16.0,
            "and stayed on top of the floor, not fallen through to {}",
            ball.position.1
        );
    }

    /// A thrown thing flies flat for twenty ticks and then arcs into the ground.
    ///
    /// `Projectile.cs:23907-23930`. Seven types this server launches are style 2 - a skeleton's
    /// bone, a town NPC's throwing knife and frost daggerfish, the Nurse's syringe, Santa's bombs,
    /// a snowball and a wired cannon's cannonball - and every one of them used to fly dead straight
    /// until it hit something or ran out of time.
    #[test]
    fn a_thrown_bone_flies_flat_and_then_arcs() {
        let tiles = Air::default();
        let mut bone = launched(21, (6.0, 0.0));
        let start = bone.position.1;
        for _ in 0..19 {
            assert_eq!(step(&mut bone, &tiles, &mut Vec::new()), Outcome::Flying);
        }
        assert!(
            (bone.position.1 - start).abs() < 0.001,
            "flat for the first twenty ticks, not {}",
            bone.position.1 - start
        );
        assert_eq!(bone.velocity.0, 6.0, "and it has not slowed either");

        for _ in 0..20 {
            assert_eq!(step(&mut bone, &tiles, &mut Vec::new()), Outcome::Flying);
        }
        assert!(
            bone.position.1 > start + 50.0,
            "and then it falls: {} pixels",
            bone.position.1 - start
        );
        assert!(
            bone.velocity.0 < 6.0,
            "dragging as it goes, not {}",
            bone.velocity.0
        );
    }

    /// The snowball is the one style-2 type with its own numbers (`Projectile.cs:23838-23843`).
    ///
    /// A gentler fall and a lighter drag, so it carries further than a bone thrown the same way.
    /// Reading the shared tail for it would have been invisible: both fall, one just falls less.
    #[test]
    fn a_snowball_falls_gentler_than_a_bone() {
        let tiles = Air::default();
        let mut snowball = launched(SNOWBALL, (6.0, 0.0));
        let mut bone = launched(21, (6.0, 0.0));
        for _ in 0..60 {
            step(&mut snowball, &tiles, &mut Vec::new());
            step(&mut bone, &tiles, &mut Vec::new());
        }
        assert!(
            snowball.position.1 < bone.position.1,
            "the snowball should still be higher: {} against {}",
            snowball.position.1,
            bone.position.1
        );
        assert!(
            snowball.velocity.0 > bone.velocity.0,
            "and faster forward: {} against {}",
            snowball.velocity.0,
            bone.velocity.0
        );
    }

    /// Three style-8 types never take gravity, and that skip is the behaviour.
    ///
    /// `Projectile.cs:24618-24621` increments the counter for everything *except* 27, 96 and 258,
    /// so a Golem's fireball and a Cursed Flame cross a room flat where a Ball of Fire arcs. Both
    /// halves are asserted, because an arm that gave all three gravity would look right until
    /// somebody fought the Golem.
    #[test]
    fn only_the_ball_of_fire_falls_among_the_fireballs() {
        let tiles = Air::default();
        for flat in [96u16, 258] {
            let mut p = launched(flat, (6.0, 0.0));
            let start = p.position.1;
            for _ in 0..120 {
                step(&mut p, &tiles, &mut Vec::new());
            }
            assert!(
                (p.position.1 - start).abs() < 0.001,
                "projectile {flat} should fly flat, not fall {}",
                p.position.1 - start
            );
        }
        let mut ball = launched(15, (6.0, 0.0));
        let start = ball.position.1;
        for _ in 0..120 {
            step(&mut ball, &tiles, &mut Vec::new());
        }
        assert!(
            ball.position.1 > start + 100.0,
            "but a Ball of Fire arcs: {} pixels",
            ball.position.1 - start
        );
    }

    /// A grenade bounces off a wall instead of dying in it, and settles on the floor.
    ///
    /// `Projectile.cs:19872-19886` for the bounce and `:48378-48399` for the roll. Without either,
    /// a thrown grenade vanished on the first thing it touched, which is the opposite of the point
    /// of a grenade.
    #[test]
    fn a_grenade_bounces_off_a_wall_and_settles_on_the_floor() {
        let mut tiles = Air::default();
        // A floor with no edge to roll off, so what the test measures is the bounce and the roll
        // rather than how long the fixture happens to be.
        for x in 0..200 {
            tiles.0.insert((x, 64), Tile::block(1));
        }
        for y in 50..64 {
            tiles.0.insert((70, y), Tile::block(1));
        }
        let mut store = ProjectileStore::new();
        let index = store
            .launch(30, (60.0 * TILE, 60.0 * TILE), (8.0, 0.0), 60, 0)
            .expect("the grenade is a known type");
        let mut grenade = *store.get(index).unwrap();

        let mut turned = false;
        for _ in 0..400 {
            assert_eq!(
                step(&mut grenade, &tiles, &mut Vec::new()),
                Outcome::Flying,
                "a grenade must never die on what it hits"
            );
            if grenade.velocity.0 < 0.0 {
                turned = true;
            }
        }
        assert!(turned, "it should have come off the wall");
        assert!(
            grenade.position.1 < 64.0 * TILE,
            "and be resting on the floor, not through it at {}",
            grenade.position.1
        );
        assert_eq!(
            grenade.velocity.0, 0.0,
            "with its roll scrubbed off rather than skidding for ever"
        );
    }

    /// A mine drops where it is thrown; a grenade carries (`Projectile.cs:48354-48366`).
    ///
    /// The two are compared rather than measured against a number, because the difference is the
    /// whole point of the split and a number would only pin this fixture. A mine drags on *both*
    /// axes from its first tick, in the air as well as on the ground; a grenade keeps every bit of
    /// its speed until it lands and only then starts scrubbing it off. Throw them identically and
    /// the grenade ends up much further away.
    #[test]
    fn a_proximity_mine_drops_where_a_grenade_carries() {
        let mut tiles = Air::default();
        for x in 0..600 {
            tiles.0.insert((x, 64), Tile::block(1));
        }
        let thrown = |kind: u16| {
            let mut store = ProjectileStore::new();
            let index = store
                .launch(kind, (60.0 * TILE, 60.0 * TILE), (8.0, 0.0), 60, 0)
                .expect("a known type");
            let mut p = *store.get(index).unwrap();
            let start = p.position.0;
            for _ in 0..600 {
                step(&mut p, &tiles, &mut Vec::new());
            }
            (p.position.0 - start, p.velocity.0)
        };
        let (mine_travel, mine_speed) = thrown(135);
        let (grenade_travel, grenade_speed) = thrown(30);

        assert_eq!(mine_speed, 0.0, "a mine comes to a full stop");
        assert_eq!(grenade_speed, 0.0, "and so, eventually, does a grenade");
        assert!(
            grenade_travel > mine_travel * 2.0,
            "but the grenade should carry far further: {grenade_travel} against {mine_travel}"
        );
    }

    /// A sandnado fills the column it stands in, which is why you cannot walk under one.
    ///
    /// `Projectile.cs:36689-36703`. Its table size is 10x10 and vanilla overwrites it every tick
    /// from a scan of the open space above and below, up to fifteen tiles each way. Without that a
    /// sandnado is a ten-pixel dot on the floor, which is not the attack: the height *is* the
    /// threat. It also never moves and ends at 300 ticks rather than the 900 a player's does.
    #[test]
    fn a_sandnado_grows_to_fill_its_column() {
        let mut tiles = Air::default();
        // A floor at 64 and a ceiling at 54: a ten-tile shaft.
        for x in 50..70 {
            tiles.0.insert((x, 64), Tile::block(1));
            tiles.0.insert((x, 54), Tile::block(1));
        }
        let mut store = ProjectileStore::new();
        let index = store
            .launch(657, (60.0 * TILE, 60.0 * TILE), (0.0, 0.0), 30, 0)
            .expect("the sandnado is a known type");
        let mut nado = *store.get(index).unwrap();
        assert_eq!(nado.height(), 10.0, "it starts as its table's dot");

        step(&mut nado, &tiles, &mut Vec::new());
        // Rows 55..63 inclusive are open, so the span is nine tiles between their centres.
        assert!(
            nado.height() > 100.0,
            "and grows to fill the shaft, not stay at {}",
            nado.height()
        );
        assert!(
            nado.width() < nado.height() / 4.0,
            "staying far narrower than it is tall: {} by {}",
            nado.width(),
            nado.height()
        );
        assert!(
            nado.center().1 > 55.0 * TILE && nado.center().1 < 64.0 * TILE,
            "and centred inside the shaft, not clipping through it: {}",
            nado.center().1
        );

        let mut spent = false;
        for _ in 0..300 {
            if step(&mut nado, &tiles, &mut Vec::new()) == Outcome::Spent {
                spent = true;
                break;
            }
        }
        assert!(spent, "a hostile sandnado ends at 300 ticks, not 900");
    }

    /// The Empress's lance hangs where it was drawn for a second and then leaves at forty.
    ///
    /// `Projectile.cs:45853-45866`. The hold *is* the attack: a ring of lances appears around you,
    /// holds long enough to be read, and then every one of them fires at once. Ours crawled off at
    /// the one pixel a tick it was launched with and never accelerated, so the ring had no
    /// telegraph and no strike - it just drifted apart.
    #[test]
    fn an_empress_lance_hangs_and_then_fires() {
        let tiles = Air::default();
        let mut store = ProjectileStore::new();
        // Launched east at unit speed, which is how the angle reaches the arm.
        let index = store
            .launch(919, (1000.0, 1000.0), (1.0, 0.0), 100, 0)
            .expect("the lance is a known type");
        let mut lance = *store.get(index).unwrap();
        let start = lance.position;

        for _ in 0..59 {
            assert_eq!(step(&mut lance, &tiles, &mut Vec::new()), Outcome::Flying);
        }
        assert_eq!(
            lance.position, start,
            "it must hang exactly where it was drawn, not drift"
        );

        step(&mut lance, &tiles, &mut Vec::new());
        assert!(
            (lance.velocity.0 - 40.0).abs() < 0.01 && lance.velocity.1.abs() < 0.01,
            "and then leave east at forty: {:?}",
            lance.velocity
        );
    }

    /// Her lasting rainbow curves; it does not fly straight (`Projectile.cs:46264-46275`).
    ///
    /// The turn eases in over thirty ticks and then holds, so the trail is an arc. Both halves are
    /// checked, because a version that turned at full rate from tick one would bend far too early.
    #[test]
    fn an_empress_rainbow_eases_into_its_curve() {
        let tiles = Air::default();
        let mut store = ProjectileStore::new();
        let index = store
            .launch(872, (1000.0, 1000.0), (8.0, 0.0), 100, 0)
            .expect("the rainbow is a known type");
        let mut rainbow = *store.get(index).unwrap();

        step(&mut rainbow, &tiles, &mut Vec::new());
        assert!(
            rainbow.velocity.1.abs() < 0.001,
            "the first tick barely turns at all: {:?}",
            rainbow.velocity
        );
        for _ in 0..120 {
            step(&mut rainbow, &tiles, &mut Vec::new());
        }
        assert!(
            rainbow.velocity.1 > 1.0,
            "and after two seconds it is well off its launch heading: {:?}",
            rainbow.velocity
        );
        let speed = rainbow.velocity.0.hypot(rainbow.velocity.1);
        assert!(
            (speed - 8.0).abs() < 0.01,
            "a rotation must not change its speed: {speed}"
        );
    }

    /// A present is dropped, not thrown: it holds its rise, tips over, and floats down.
    ///
    /// `Projectile.cs:29337-29366`. The two phases matter separately - the first is a half-second
    /// of whatever upward speed it left with, and the second caps its fall at three - and a present
    /// that skipped either would be impossible to catch, which is what a Frost Moon present is for.
    #[test]
    fn a_present_holds_its_rise_and_then_floats_down() {
        let tiles = Air::default();
        let mut present = launched(351, (2.0, -6.0));
        let top = present.position.1;
        for _ in 0..30 {
            step(&mut present, &tiles, &mut Vec::new());
        }
        assert!(
            present.position.1 < top,
            "it should still be rising after half a second, not at {}",
            present.position.1 - top
        );
        assert!(present.velocity.1 < 0.0, "and still going up");

        for _ in 0..600 {
            step(&mut present, &tiles, &mut Vec::new());
        }
        assert!(
            present.velocity.1 <= PRESENT_TERMINAL + 0.001,
            "and never fall faster than three: {}",
            present.velocity.1
        );
        assert!(
            present.position.1 > top,
            "having come down again: {}",
            present.position.1 - top
        );
    }

    /// A thrown ale arcs (`Projectile.cs:30655-30677`).
    #[test]
    fn a_thrown_ale_falls_after_fifteen_ticks() {
        let tiles = Air::default();
        let mut ale = launched(669, (6.0, 0.0));
        let start = ale.position.1;
        for _ in 0..14 {
            step(&mut ale, &tiles, &mut Vec::new());
        }
        assert!(
            (ale.position.1 - start).abs() < 0.001,
            "flat for fifteen ticks, not {}",
            ale.position.1 - start
        );
        for _ in 0..40 {
            step(&mut ale, &tiles, &mut Vec::new());
        }
        assert!(
            ale.position.1 > start + 50.0,
            "and then it falls: {} pixels",
            ale.position.1 - start
        );
    }

    /// A falling star handed over inside a mountain falls out of it rather than dying in it.
    ///
    /// `Projectile.cs:24170-24176`: `ai[1]` latches the first tick the star is clear of terrain,
    /// and only then does it collide. `AI_148_StarSpawner` hands a star over at whatever position
    /// the spawner reached, which is not guaranteed to be open sky.
    #[test]
    fn a_falling_star_passes_through_terrain_until_it_is_clear_of_it() {
        let mut tiles = Air::default();
        for x in 50..70 {
            for y in 50..56 {
                tiles.0.insert((x, y), Tile::block(1));
            }
        }
        let mut store = ProjectileStore::new();
        let index = store
            .launch(12, (60.0 * TILE, 52.0 * TILE), (0.0, 8.0), 1000, 0)
            .expect("the star is a known type");
        let mut star = *store.get(index).unwrap();
        for tick in 0..30 {
            assert_eq!(
                step(&mut star, &tiles, &mut Vec::new()),
                Outcome::Flying,
                "the star died inside the rock it was handed over in, on tick {tick}"
            );
        }
        assert!(
            star.position.1 > 56.0 * TILE,
            "and it should have fallen out of the bottom, not stuck at {}",
            star.position.1
        );
        assert_eq!(star.ai[1], 1.0, "with the latch set once it was clear");
    }

    /// Put a boulder at rest at a tile position, the way a Boulder Statue and a broken Boulder
    /// tile both release one: no velocity at all, and gravity is the only thing that moves it.
    fn dropped_boulder(store: &mut ProjectileStore, tile_x: f32, tile_y: f32) -> u16 {
        let half = 31.0 / 2.0;
        store
            .launch(
                99,
                (tile_x * TILE + half, tile_y * TILE + half),
                (0.0, 0.0),
                70,
                0,
            )
            .expect("the boulder is a known type")
    }

    /// A Boulder Statue drops a boulder that *falls*, and once it lands it rolls.
    ///
    /// This is the whole trap and none of it happened: vanilla launches the boulder at rest and the
    /// arm gives it gravity, so with no arm at all a wired statue used to put a stationary hostile
    /// box under itself for a full minute. `Projectile.cs:26596-26746`.
    #[test]
    fn a_boulder_falls_and_then_rolls_away_from_the_wall_beside_it() {
        let mut tiles = Air::default();
        for x in 55..90 {
            tiles.0.insert((x, 64), Tile::block(1));
        }
        // A wall on its left, so it must roll east.
        for y in 50..64 {
            tiles.0.insert((61, y), Tile::block(1));
        }
        let mut store = ProjectileStore::new();
        let index = dropped_boulder(&mut store, 62.0, 56.0);
        let start = *store.get(index).unwrap();
        assert_eq!(start.velocity, (0.0, 0.0), "it is released at rest");

        let mut boulder = start;
        for _ in 0..200 {
            if step(&mut boulder, &tiles, &mut Vec::new()) == Outcome::Spent {
                break;
            }
        }
        assert!(
            boulder.position.1 > start.position.1 + 100.0,
            "it should have fallen to the floor, not hung at {}",
            boulder.position.1
        );
        assert!(
            boulder.velocity.0 > 1.0,
            "and rolled east away from the wall on its left, not sat at {}",
            boulder.velocity.0
        );
        assert!(
            boulder.position.0 > start.position.0 + 50.0,
            "which means actually travelling: {} pixels",
            boulder.position.0 - start.position.0
        );
    }

    /// It works up to seven pixels a tick and no further (`Projectile.cs:26715-26724`).
    ///
    /// The gate is `velocity.X < 7f` and the step is `+= 0.05f`, so the last step it is allowed to
    /// take can carry it a hair past seven and leave it there. That overshoot is the game's own and
    /// is transcribed rather than clamped; the tolerance below is what makes room for it.
    #[test]
    fn a_rolling_boulder_stops_gaining_speed_at_seven() {
        let mut tiles = Air::default();
        for x in 0..4000 {
            tiles.0.insert((x, 64), Tile::block(1));
        }
        for y in 50..64 {
            tiles.0.insert((61, y), Tile::block(1));
        }
        let mut store = ProjectileStore::new();
        let index = dropped_boulder(&mut store, 62.0, 62.0);
        let mut boulder = *store.get(index).unwrap();
        for _ in 0..1000 {
            if step(&mut boulder, &tiles, &mut Vec::new()) == Outcome::Spent {
                break;
            }
            assert!(
                boulder.velocity.0 <= BOULDER_TOP_SPEED + BOULDER_ACCEL,
                "a boulder should never pass seven by more than one step: {}",
                boulder.velocity.0
            );
        }
        assert!(
            boulder.velocity.0 > 6.0,
            "but it should get there: {}",
            boulder.velocity.0
        );
    }

    /// A boulder that meets a wall head on breaks; one that merely lands on a floor does not.
    ///
    /// `Projectile.cs:19084-19098`: the kill is on the *horizontal* axis changing, which is why
    /// falling straight down onto the ground leaves it alive to roll. Getting that the wrong way
    /// round would end the trap on the first tile it touched.
    #[test]
    fn a_boulder_breaks_on_a_wall_but_not_on_the_floor() {
        let mut tiles = Air::default();
        for x in 55..70 {
            tiles.0.insert((x, 64), Tile::block(1));
        }
        let mut store = ProjectileStore::new();

        let index = dropped_boulder(&mut store, 60.0, 56.0);
        let mut lands = *store.get(index).unwrap();
        for _ in 0..40 {
            assert_eq!(
                step(&mut lands, &tiles, &mut Vec::new()),
                Outcome::Flying,
                "landing on a floor must not break it"
            );
        }

        // And released *overlapping* the ground, which a statue flush against a floor will do. Both
        // axes read as blocked on that first tick, and the only thing telling it apart from a
        // boulder that has run into a wall is that this one was never moving sideways. Exactly one
        // tick is asserted because that is the whole contract: once it picks a direction, a boulder
        // embedded in rock has genuinely hit a wall and is supposed to break.
        let index = dropped_boulder(&mut store, 60.0, 63.0);
        let mut embedded = *store.get(index).unwrap();
        embedded.position.1 = 64.0 * TILE - 8.0;
        assert_eq!(
            step(&mut embedded, &tiles, &mut Vec::new()),
            Outcome::Flying,
            "a boulder released touching the ground broke before it had moved at all"
        );

        for y in 50..64 {
            tiles.0.insert((66, y), Tile::block(1));
        }
        let index = dropped_boulder(&mut store, 60.0, 63.0);
        let mut hits = *store.get(index).unwrap();
        hits.velocity = (6.0, 0.0);
        let mut broke = false;
        for _ in 0..40 {
            if step(&mut hits, &tiles, &mut Vec::new()) == Outcome::Spent {
                broke = true;
                break;
            }
        }
        assert!(
            broke,
            "it rolled into a wall and lived: {:?}",
            hits.position
        );
    }

    /// Seven ticks of grace, because the boulder appears inside whoever pulled the lever.
    ///
    /// `Projectile.CanDamage` (`Projectile.cs:12491-12494`). A Boulder Statue puts its boulder
    /// twenty-eight pixels below its own base, which is well inside the hitbox of anybody standing
    /// beside it, so without this the trap hits its own operator before it has moved at all.
    #[test]
    fn a_boulder_cannot_hurt_anybody_for_its_first_seven_ticks() {
        let tiles = Air::default();
        let mut store = ProjectileStore::new();
        let index = dropped_boulder(&mut store, 60.0, 20.0);
        let mut boulder = *store.get(index).unwrap();
        assert!(!boulder.can_damage(), "not on the tick it is released");
        for tick in 1..=7 {
            step(&mut boulder, &tiles, &mut Vec::new());
            assert!(
                !boulder.can_damage(),
                "still harmless on tick {tick}, local_ai[2] = {}",
                boulder.local_ai[2]
            );
        }
        step(&mut boulder, &tiles, &mut Vec::new());
        assert!(
            boulder.can_damage(),
            "and dangerous from the eighth: local_ai[2] = {}",
            boulder.local_ai[2]
        );
    }

    /// A flamethrower trap is not the thing that burns: it emits a flame every sixth tick, and
    /// that is what carries the damage.
    #[test]
    fn a_flamethrower_emits_flames_rather_than_burning() {
        let tiles = Air::default();
        let mut trap = launched(187, (5.0, 0.0));
        let mut emitted = Vec::new();
        for _ in 0..30 {
            step(&mut trap, &tiles, &mut emitted);
        }
        assert_eq!(emitted.len(), 5, "one every six ticks for thirty ticks");
        assert_eq!(emitted[0].projectile_type, 188);
        assert_eq!(
            emitted[0].velocity,
            (5.0, 0.0),
            "the flame goes where the trap points"
        );
        assert_eq!(emitted[0].damage, trap.damage);
    }

    /// A spear reaches out of its trap and then pulls back in, dying when it is home rather than
    /// hanging in the air.
    #[test]
    fn a_spear_reaches_out_and_comes_back() {
        let tiles = Air::default();
        let mut spear = launched(186, (8.0, 0.0));
        let start = spear.position.0;
        let mut furthest = start;
        let mut ticks = 0;
        loop {
            ticks += 1;
            let out = step(&mut spear, &tiles, &mut Vec::new());
            furthest = furthest.max(spear.position.0);
            if out == Outcome::Spent {
                break;
            }
            assert!(ticks < 300, "the spear never came home");
        }
        assert!(
            (furthest - start) > 290.0 && (furthest - start) < 320.0,
            "it should reach about three hundred pixels, not {}",
            furthest - start
        );
        assert!(
            (spear.position.0 - start).abs() < 20.0,
            "it should die where it started, not at {}",
            spear.position.0 - start
        );
    }

    /// A spear set into a wall stops at the wall rather than reaching its full length through it.
    #[test]
    fn a_spear_stops_at_what_it_hits() {
        let mut tiles = Air::default();
        for y in 0..200 {
            tiles.0.insert((66, y), Tile::block(1));
        }
        let mut spear = launched(186, (8.0, 0.0));
        let start = spear.position.0;
        let mut furthest = start;
        for _ in 0..300 {
            if step(&mut spear, &tiles, &mut Vec::new()) == Outcome::Spent {
                break;
            }
            furthest = furthest.max(spear.position.0);
        }
        assert!(
            (furthest - start) < 100.0,
            "the wall is 56 pixels away; it reached {}",
            furthest - start
        );
    }

    /// A flame burns out in a second however long its type says it lives.
    #[test]
    fn a_flame_burns_out_in_a_second() {
        let tiles = Air::default();
        let mut flame = launched(188, (4.0, 0.0));
        assert!(
            flame.time_left > 60,
            "it starts with the type's own lifetime"
        );
        step(&mut flame, &tiles, &mut Vec::new());
        assert!(
            flame.time_left <= 60,
            "and is cut to a second on its first tick"
        );
    }

    /// A projectile that is not moving cannot be killed by whatever it is sitting in.
    ///
    /// `Collision.TileCollision` (`Collision.cs:2299`) decides a hit by comparing the box at
    /// `Position + Velocity` against a tile *and* the box at `Position` against being outside it
    /// on that side, so a zero velocity comes back unchanged and no style reaches its kill. Ours
    /// walked from the start point inclusive, and a zero-length walk is only its own start, so
    /// anything launched at rest inside terrain died on its first tick.
    ///
    /// The Dryad's ward is the first shipped projectile to meet this: vanilla gives it no launch
    /// velocity at all and it is cast at chest height next to a townsperson standing on a floor.
    #[test]
    fn a_projectile_at_rest_is_not_killed_by_the_tile_it_is_standing_in() {
        let mut tiles = Air::default();
        for x in 60..70 {
            for y in 60..70 {
                tiles.0.insert((x, y), Tile::block(1));
            }
        }
        // Buried, at rest, and of a type that collides.
        let mut ward = launched(586, (0.0, 0.0));
        assert!(ward.stats.tile_collide, "the type does collide");
        ward.position = (64.0 * TILE, 64.0 * TILE);
        assert!(
            hits_terrain(&tiles, ward.position, (ward.width(), ward.height())),
            "and is inside a solid block"
        );

        assert_eq!(
            step(&mut ward, &tiles, &mut Vec::new()),
            Outcome::Flying,
            "vanilla hands a zero velocity straight back rather than killing on it"
        );
        assert_eq!(
            ward.position,
            (64.0 * TILE, 64.0 * TILE),
            "and it has not been pushed anywhere either"
        );

        // The guard is on the *move*, not on the type: give it a velocity and the wall still kills
        // it, so this has not quietly made a colliding projectile immortal.
        ward.velocity = (4.0, 0.0);
        assert_eq!(step(&mut ward, &tiles, &mut Vec::new()), Outcome::Spent);
    }

    /// The Queen Slime's smash widens where it landed, and is gone in nine ticks.
    ///
    /// `AI_135_OgreStomp` (`Projectile.cs:69740-69756`). The hitbox is the attack: it eases from
    /// five tiles across to thirty around a fixed centre, so what looked like a thirty-pixel box
    /// is really four hundred and eighty by the end. Ours flew off at its launch velocity and hung
    /// about for the table's 120.
    #[test]
    fn the_queen_slimes_smash_widens_where_it_landed_and_is_gone_in_nine_ticks() {
        let tiles = Air::default();
        let mut smash = launched(922, (0.0, 0.0));
        let centre = smash.center();
        assert_eq!((smash.width(), smash.height()), (30.0, 30.0), "its table's");

        // Tick one: five tiles across, plus the first ninth of the growth.
        assert_eq!(step(&mut smash, &tiles, &mut Vec::new()), Outcome::Flying);
        let first = smash.width();
        assert!(
            (80.0..140.0).contains(&first),
            "five tiles is eighty pixels, plus a ninth of the way to 480; got {first}"
        );
        assert_eq!(smash.center(), centre, "and it grew around its own centre");

        for _ in 0..8 {
            assert_eq!(step(&mut smash, &tiles, &mut Vec::new()), Outcome::Flying);
        }
        assert_eq!(
            smash.width(),
            480.0,
            "thirty tiles across on its ninth tick"
        );
        assert_eq!(smash.center(), centre, "still centred where it landed");
        assert_eq!(
            step(&mut smash, &tiles, &mut Vec::new()),
            Outcome::Spent,
            "and gone on the tenth"
        );
    }

    /// The Duke's mouth bubbles bob rather than fly straight, and only the ones with no target do.
    ///
    /// `aiStyle 65` (`Projectile.cs:30012-30084`) is two behaviours under one number, and a zero
    /// `ai[1]` is what says which. The bob traces a cosine around the launch velocity over thirty
    /// ticks; the seeking half is `systems::tick_sharknado_bolts` and must not be touched here,
    /// because that arm has already set this tick's velocity by the time `step` runs.
    #[test]
    fn the_dukes_mouth_bubbles_bob_and_the_seeking_one_is_left_alone() {
        let tiles = Air::default();

        // No target: it bobs around the eight it was thrown at, and comes back to it after a full
        // period rather than drifting off.
        let mut bubble = launched(385, (2.0, 8.0));
        let mut lowest = f32::MAX;
        let mut highest = f32::MIN;
        for _ in 0..30 {
            step(&mut bubble, &tiles, &mut Vec::new());
            lowest = lowest.min(bubble.velocity.1);
            highest = highest.max(bubble.velocity.1);
        }
        assert!(
            highest - lowest > 4.0,
            "a cosine of amplitude four either way should swing at least that far: \
             {lowest} to {highest}"
        );
        assert!(
            (bubble.velocity.1 - 8.0).abs() < 0.01,
            "and a full period returns it to its launch speed, not somewhere else: {}",
            bubble.velocity.1
        );
        assert_eq!(
            bubble.velocity.0, 2.0,
            "it never touches the sideways speed"
        );

        // Handed a target, `step` must leave the velocity exactly as the seeking arm set it.
        let mut seeker = launched(385, (0.0, 0.0));
        seeker.ai = [1.0, 1.0, 0.0];
        seeker.velocity = (3.0, 4.0);
        step(&mut seeker, &tiles, &mut Vec::new());
        assert_eq!(seeker.velocity, (3.0, 4.0));
        assert_eq!(seeker.ai[0], 1.0, "and does not run the bob's own counter");
    }

    /// A Deerclops ice spike stands for twenty ticks and then goes.
    ///
    /// `AI_157_SharpTears` (`Projectile.cs:52268-52400`), whose `num10` is 20 for type 961. It
    /// never touches its velocity - it is launched with a unit vector for its *facing*, so it
    /// creeps a pixel a tick - and its flags are read before the increment, which is why the last
    /// live tick is the one that reads twenty rather than the one that reaches it.
    #[test]
    fn a_deerclops_ice_spike_stands_for_twenty_ticks() {
        let tiles = Air::default();
        let mut spike = launched(961, (0.0, -1.0));
        assert_eq!(
            spike.time_left, 3600,
            "its table would keep it for a minute"
        );
        let start = spike.position;
        for tick in 1..=20 {
            assert_eq!(
                step(&mut spike, &tiles, &mut Vec::new()),
                Outcome::Flying,
                "still standing on tick {tick}"
            );
        }
        assert_eq!(
            step(&mut spike, &tiles, &mut Vec::new()),
            Outcome::Spent,
            "and gone on the twenty-first"
        );
        assert!(
            (spike.position.1 - start.1).abs() < 25.0,
            "a unit vector is a facing, not a speed: it moved {} pixels",
            start.1 - spike.position.1
        );
    }
}
