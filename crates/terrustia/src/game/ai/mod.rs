//! NPC behaviour, one module per AI style.
//!
//! Two rules hold this apart from the 59,000-line original it is ported from:
//!
//! 1. **Per-type variation lives in data, never in branches.** Where the game writes
//!    `if (type == 25) speed = 5;` inside its AI, that number belongs in a table. A hand-written
//!    module here contains algorithms only.
//! 2. **Parity is tracked, not claimed.** Every style declares a [`Parity`] level, so a style that
//!    is still an approximation is visible in a test rather than hidden behind one.

pub mod ambush;
pub mod army;
pub mod balloon;
pub mod bat;
pub mod bird;
pub mod boss;
pub mod caster;
pub mod creeper;
pub mod critter;
pub mod dragonfly;
pub mod eater;
pub mod eye;
pub mod fairy;
pub mod fighter;
pub mod fish;
pub mod frost;
pub mod granite;
pub mod grub;
pub mod hardmode;
pub mod haunt;
pub mod inert;
pub mod mimic;
pub mod orb;
pub mod rooted;
pub mod sight;
pub mod skull;
pub mod slime;
pub mod snail;
pub mod spore;
pub mod swimmer;
pub mod town;
pub mod town_combat;
pub mod track;
pub mod tumbleweed;
pub mod worm;

use rand::rngs::SmallRng;

use terrustia_proto::npc_params::{FlierSteering, Steering};

use crate::game::npc::{Npc, TileView};
use crate::game::npc_ai::Target;

/// The player hitbox every routine aims at, from `Player.SetDefaults`.
pub const PLAYER_WIDTH: i32 = 20;
pub const PLAYER_HEIGHT: i32 = 42;

/// World state the routines read.
///
/// The game reaches into `Main` for these from inside the AI; passing them in keeps the routines
/// testable and keeps `Main`-shaped global state out of the port.
#[derive(Debug, Clone, Copy)]
pub struct Conditions {
    pub blood_moon: bool,
    pub day: bool,
    pub eclipse: bool,
    /// Whether a Pumpkin Moon is running. Its two style-26 walkers and the Poltergeist leave when
    /// it is not (`NPC.cs:63232-63234`, `:24798-24802`).
    pub pumpkin_moon: bool,
    /// Rain, which sends residents indoors just as nightfall does.
    pub raining: bool,
    /// Whether the day is windy enough for the things that need wind to do anything.
    pub windy: bool,
    /// Whether the nearest player is standing in the crimson, which is where the Brain lives.
    pub crimson: bool,
    /// ...and whether they are in the jungle, which is where the Queen lives.
    pub jungle: bool,
    /// ...and whether they are in the snow, which is where Deerclops hunts.
    pub snow: bool,
    /// ...and whether they are in the hallow, which is the only place a Prismatic Lacewing will
    /// stay: `!Main.player[target].ZoneHallow` is what lets its own fade counter run all the way to
    /// the end and take it out of the world (`NPC.cs:45400-45412`).
    pub hallow: bool,
    /// Wind speed, signed: positive blows east.
    pub wind: f32,
    /// Whether the nearest player is standing in a desert, which is the only place a tumbleweed
    /// will stay.
    pub desert: bool,
    /// Whether a sandstorm is blowing. A tumbleweed in one is a different creature.
    pub sandstorm: bool,
    /// Whether a Slime Rain is falling — one of the four conditions that make a slime "active" and
    /// hop at twice the rate (`NPC.AI_001_Slimes`' own `flag3`, alongside night, being hurt, and
    /// being below the surface).
    pub slime_rain: bool,
    /// Pixel depth of the surface layer, below which "outdoors" stops applying.
    pub surface_y: f32,
    /// Whether the world is expert or better.
    ///
    /// A great many hardmode routines are not merely tuned differently in expert — they gain
    /// attacks they do not otherwise have, so this changes what an enemy *does*, not just how
    /// hard it hits.
    pub expert: bool,
    /// Whether hardmode has begun. Some routines behave differently before the wall falls.
    pub hardmode: bool,
    /// Whether this is a get-fixed-boi / For-the-Worthy world (`Main.getGoodWorld`). A handful of
    /// routines are harder there in ways that are not merely stat scaling: the Wall of Flesh walks
    /// faster and the Destroyer grows a longer body.
    pub get_good_world: bool,
    /// Whether this is a 10th-anniversary (`celebrationmk10`) world (`Main.tenthAnniversaryWorld`).
    /// One routine reads it: the Crimson big mimic gains a gag "stuff cannon" state there (C7-07).
    pub tenth_anniversary: bool,
    /// The world's size in tiles, for the handful of routines that steer away from its edges.
    pub world_size: (i32, i32),
}

impl Default for Conditions {
    /// A calm night in a large world: every flag false, as a derived `Default` would give, except
    /// the world size.
    ///
    /// The size is the one field that must not be zero. A routine that steers away from the edges
    /// of the world would decide it was against one, so "no world at all" is not a usable default
    /// the way "no wind" and "no blood moon" are.
    fn default() -> Self {
        Self {
            blood_moon: false,
            day: false,
            eclipse: false,
            pumpkin_moon: false,
            raining: false,
            windy: false,
            crimson: false,
            jungle: false,
            snow: false,
            hallow: false,
            wind: 0.0,
            desert: false,
            sandstorm: false,
            slime_rain: false,
            surface_y: 0.0,
            expert: false,
            hardmode: false,
            get_good_world: false,
            tenth_anniversary: false,
            world_size: (4200, 1200),
        }
    }
}

/// The top-left corner of a target's hitbox, which is what the collision routines want.
fn target_box(t: Target) -> (f32, f32) {
    (
        t.center.0 - PLAYER_WIDTH as f32 / 2.0,
        t.center.1 - PLAYER_HEIGHT as f32 / 2.0,
    )
}

/// Whether an NPC has a clear line to a target.
pub fn can_see(tiles: &impl TileView, npc: &Npc, t: Target) -> bool {
    sight::can_hit(
        tiles,
        npc.position,
        (npc.stats.width, npc.stats.height),
        target_box(t),
        (PLAYER_WIDTH, PLAYER_HEIGHT),
    )
}

/// The nearest living player, at any distance.
///
/// This is `NPC.TargetClosest`, and the two things it does differently from a plain search matter:
/// it measures with Manhattan distance rather than a real one, and it has no range limit at all.
/// Enemies do not lose interest when you outrun them — the despawn timer is what eventually
/// removes them.
pub fn target_closest(npc: &Npc, targets: &[Target]) -> Option<Target> {
    let (cx, cy) = npc.center();
    targets
        .iter()
        .filter(|t| t.alive)
        .min_by(|a, b| {
            let reach = |t: &&Target| (t.center.0 - cx).abs() + (t.center.1 - cy).abs();
            reach(a).total_cmp(&reach(b))
        })
        .copied()
}

/// Turn to face a target on both axes, the way `SetTargetTrackingValues` does.
///
/// The comparison is between hitbox midpoints computed with integer division, so an NPC of odd
/// width faces the way the game's arithmetic says rather than the way real arithmetic would.
///
/// Confusion flips the horizontal axis and only that one, unconditionally, at the tail of that
/// same method (`NPC.cs:78576-78579`), so it lands here rather than in each routine, which is
/// also where vanilla puts it: every `TargetClosest` call passes through it. The three routines
/// with their own local `face` (`slime.rs:51`, `boss/fishron.rs:325`, `army/walker.rs:631`) do
/// not go through this one and so are not confused; they turn on a bare X comparison rather than
/// on `SetTargetTrackingValues`.
pub fn face(npc: &mut Npc, t: Target) {
    let (bx, by) = target_box(t);
    let mid_x = bx as i32 + PLAYER_WIDTH / 2;
    let mid_y = by as i32 + PLAYER_HEIGHT / 2;
    npc.direction = if (mid_x as f32) < npc.position.0 + (npc.stats.width / 2) as f32 {
        -1
    } else {
        1
    };
    npc.direction_y = if (mid_y as f32) < npc.position.1 + (npc.stats.height / 2) as f32 {
        -1
    } else {
        1
    };
    if npc.buffs.flags.confused {
        npc.direction *= -1;
    }
}

/// Accelerate one axis toward the way it is facing.
///
/// Three terms, and the middle one carries the character: while the velocity still points the
/// wrong way the routine applies a nudge that either pushes back against the turn — giving a bat
/// its wide lazy arc — or hurries it along, which is what makes a wandering eye dart.
pub fn steer_axis(velocity: &mut f32, direction: i8, s: Steering) {
    steer_axis_gated(velocity, direction, s, true, true);
}

/// As [`steer_axis`], but with an extra condition on each arm.
///
/// The floating eye will only accelerate toward a target it has not already passed, so each arm
/// carries a position test alongside its speed test.
pub fn steer_axis_gated(
    velocity: &mut f32,
    direction: i8,
    s: Steering,
    toward_negative: bool,
    toward_positive: bool,
) {
    if direction == -1 && *velocity > -s.max && toward_negative {
        *velocity -= s.accel;
        if *velocity > s.overshoot_at {
            *velocity -= s.overshoot;
        } else if *velocity > 0.0 {
            *velocity += s.brake;
        }
        if *velocity < -s.max {
            *velocity = -s.max;
        }
    } else if direction == 1 && *velocity < s.engage_positive && toward_positive {
        *velocity += s.accel;
        if *velocity < -s.overshoot_at {
            *velocity += s.overshoot;
        } else if *velocity < 0.0 {
            *velocity -= s.brake;
        }
        if *velocity > s.max {
            *velocity = s.max;
        }
    }
}

/// Steer both axes at once.
pub fn steer(npc: &mut Npc, s: FlierSteering) {
    steer_axis(&mut npc.velocity.0, npc.direction, s.x);
    steer_axis(&mut npc.velocity.1, npc.direction_y, s.y);
}

/// A vector of length `speed` along `v`, or nothing when `v` has no direction.
///
/// Eight routines wrote this identically before it moved here: the Twins, Plantera, the Golem,
/// Mothron, the Big Mimic, the Hunter, the Charger and the Saucer. Vanilla spells the same thing
/// out at each of its own call sites, dividing by the length it just measured; the guard on a zero
/// or non-finite length is this server's, and was already in all eight copies, because a divide by
/// a zero length would put a NaN into a velocity that then never recovers.
pub fn unit(v: (f32, f32), speed: f32) -> (f32, f32) {
    let length = v.0.hypot(v.1);
    if length <= 0.0 || !length.is_finite() {
        (0.0, 0.0)
    } else {
        (v.0 / length * speed, v.1 / length * speed)
    }
}

/// Bounce off terrain, keeping enough speed to clear whatever was hit.
///
/// The reflection uses the velocity from *before* the move, because collision has already zeroed
/// the live one. Shared by the bat and eye styles, which write it identically.
pub fn bounce(npc: &mut Npc) {
    if npc.collide_x {
        npc.velocity.0 = npc.old_velocity.0 * -0.5;
        if npc.direction == -1 && npc.velocity.0 > 0.0 && npc.velocity.0 < 2.0 {
            npc.velocity.0 = 2.0;
        }
        if npc.direction == 1 && npc.velocity.0 < 0.0 && npc.velocity.0 > -2.0 {
            npc.velocity.0 = -2.0;
        }
    }
    if npc.collide_y {
        npc.velocity.1 = npc.old_velocity.1 * -0.5;
        if npc.velocity.1 > 0.0 && npc.velocity.1 < 1.0 {
            npc.velocity.1 = 1.0;
        }
        if npc.velocity.1 < 0.0 && npc.velocity.1 > -1.0 {
            npc.velocity.1 = -1.0;
        }
    }
}

/// Swim upward out of water rather than flying through it.
///
/// Half a pixel a tick against whatever the routine was doing, capped at four. The bat and eye
/// styles share it verbatim.
pub fn rise_out_of_water(npc: &mut Npc) {
    if npc.velocity.1 > 0.0 {
        npc.velocity.1 *= 0.95;
    }
    npc.velocity.1 -= 0.5;
    if npc.velocity.1 < -4.0 {
        npc.velocity.1 = -4.0;
    }
}

/// A projectile a routine wants launched.
///
/// The routine only decides — cadence, aim, scatter, reload; `server.rs` turns this into a real
/// entity via `self.projectiles.launch(..)` and broadcasts it, the same as a player's own shot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    pub projectile: u16,
    pub damage: i32,
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    pub time_left: u16,
    /// The `ai` values `NewProjectile`'s last two arguments carry, which for a large family of
    /// styles are not decoration but the whole behaviour.
    ///
    /// This was absent for a long time and three lanes worked around it by *recovering* what they
    /// needed on a projectile's first tick: the Moon Lord's deathray finds the eye it is standing
    /// on, and the Dryad's ward and the Mechanic's wrench each find their caster. That trade was
    /// right for those three, because each is an index a projectile can deduce from where it was
    /// launched. It does not generalise, and the count is why this field exists now: **five of the
    /// styles still missing choose their entire behaviour from what they are launched with.** The
    /// Duke's sharknado bolt bobs on the spot at `ai[1] == 0` and homes at a player above it;
    /// Deerclops's shadow hand picks one of four routines from an `ai[0]` of 0, 180, 300 or 390;
    /// the Stardust Jellyfish and the Nebula Eye hover by the parent named in `ai[1]`; the
    /// Cultist's shards converge on the point in `ai[0..1]`. None of those is deducible from a
    /// launch point.
    ///
    /// `boss/fishron.rs` had already written this down at its own launch site, as a narrowing it
    /// could not lift.
    pub ai: [f32; 3],
}

/// A melee hit a routine wants applied directly to another NPC — no projectile entity involved,
/// the way vanilla's own `StrikeNPCNoInteraction` isn't one either. `target` is an NPC table
/// index, from whatever gave the routine its [`World::hostile`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeleeHit {
    pub target: u8,
    pub damage: i32,
    pub knockback: f32,
    pub direction: i8,
    /// How long the target is then immune to *this server's* hits, in ticks.
    ///
    /// `nPC2.immune[myPlayer] = (int)ai[1] + 2` (`NPC.cs:55639`): the rest of the swinging NPC's
    /// attack state plus two, which is what makes state 15 one hit rather than one per tick. The
    /// state runs against whatever is in the box every tick it lasts and has no `localAI[3]` gate
    /// at all, so without this a Tax Collector's twelve-damage swing lands twelve damage a tick
    /// for the whole state.
    pub immune_for: i32,
}

/// What Plantera can see of its own fight.
fn plantera_state<T: TileView>(world: &World<'_, T>) -> boss::plantera::PlanteraState {
    boss::plantera::PlanteraState {
        hooks: world.hooks,
        // The jungle, underground: the only place it fights at its ordinary pace.
        at_home: world.conditions.jungle
            && world
                .target
                .is_some_and(|t| t.center.1 > world.conditions.surface_y),
    }
}

/// What the Golem can see of its own assembly.
///
/// Which of its parts are still standing changes how often the body hops, and where it is being
/// fought changes every rate in the fight, so both are worked out from the census rather than
/// guessed at.
fn golem_state<T: TileView>(npc: &Npc, world: &World<'_, T>) -> boss::golem::GolemState {
    boss::golem::GolemState {
        head: world.count(terrustia_proto::npc_params::GOLEM_HEAD) > 0,
        left_fist: world.count(terrustia_proto::npc_params::GOLEM_FIST_LEFT) > 0,
        right_fist: world.count(terrustia_proto::npc_params::GOLEM_FIST_RIGHT) > 0,
        // The temple is jungle, and the fight is meant to happen underground.
        at_home: world.conditions.jungle
            && world
                .target
                .is_some_and(|t| t.center.1 > world.conditions.surface_y),
        // GOL-2: the per-player balance this part was scaled for (its own `GetMyBalance`).
        balance: npc.balance(),
    }
}

/// The types worth counting each tick, because some routine's behaviour turns on how many are up.
pub const CENSUS_TYPES: [u16; 17] = [
    terrustia_proto::npc_params::CREEPER,
    terrustia_proto::npc_params::WALL_LEECH,
    terrustia_proto::npc_params::PAL_ESCORT,
    terrustia_proto::npc_params::DUTCHMAN_GUN,
    terrustia_proto::npc_params::NAUTILUS_HELPER,
    terrustia_proto::npc_params::MOTHRON_EGG,
    terrustia_proto::npc_params::MOTHRON_SPAWN_TYPE,
    terrustia_proto::npc_params::GOLEM_HEAD,
    terrustia_proto::npc_params::GOLEM_FIST_LEFT,
    terrustia_proto::npc_params::GOLEM_FIST_RIGHT,
    terrustia_proto::npc_params::CULTIST_DEVOTE,
    terrustia_proto::npc_params::CULTIST_ARCHER,
    terrustia_proto::npc_params::CULTIST_CLONE,
    terrustia_proto::npc_params::MOON_LORD_HAND,
    terrustia_proto::npc_params::MOON_LORD_HEAD,
    // The Martian Saucer's four guns: the core watches these to know when to end the fight (SAU-1).
    terrustia_proto::npc_params::MARTIAN_SAUCER_TURRET,
    terrustia_proto::npc_params::MARTIAN_SAUCER_CANNON,
];

impl<T: TileView> World<'_, T> {
    /// How many of `npc_type` are alive right now. Types nobody asked about read as none.
    /// The world's width in tiles.
    pub fn world_width(&self) -> i32 {
        self.conditions.world_size.0
    }

    /// The world's height in tiles.
    pub fn world_height(&self) -> i32 {
        self.conditions.world_size.1
    }

    pub fn count(&self, npc_type: u16) -> usize {
        self.census
            .iter()
            .find(|(ty, _)| *ty == npc_type)
            .map_or(0, |(_, n)| *n)
    }
}

/// A world with nothing going on in it, for the tests that only care about one thing at a time.
///
/// Every routine's tests need a `World`, and spelling the whole struct out in each module means a
/// new field is thirty edits. This is the one place that knows what "nothing going on" is.
#[cfg(test)]
pub fn calm<T: TileView>(tiles: &T, target: Option<crate::game::npc_ai::Target>) -> World<'_, T> {
    World {
        tiles,
        target,
        wet: false,
        target_wet: false,
        conditions: Conditions::default(),
        was_hurt: false,
        target_velocity: (0.0, 0.0),
        hostile: None,
        census: &[],
        own_escorts: 0,
        parent: None,
        parent_state: 0.0,
        parent_health: 1.0,
        crowding: (0.0, 0.0),
        avoid: &[],
        target_taken: false,
        hooks: None,
        hook_anchors: &[],
        body_tentacles: 0,
        kin_moving: false,
        sockets_open: 0,
        army: ArmyView::default(),
        treasure: None,
        mage: army::mage::MageView {
            wounded: 0,
            can_raise: false,
        },
        slot: 0,
    }
}

/// How close a style's implementation is to the game's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    /// Transcribed from the decompiled routine: same constants, same decisions.
    Ported,
    /// Recognisable but invented. Constants here are guesses and are not parity.
    Approximate,
}

/// The parity level of the routine driving an AI style.
///
/// Anything absent has no implementation at all.
pub fn parity(style: i32) -> Option<Parity> {
    let level = match style {
        // Ported from the decompiled source. Kept in order, so adding one is a one-line change and
        // a missing number is visible.
        0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19
        | 20 | 21 | 22 | 23 | 24 | 25 | 26 | 27 | 28 | 29 | 30 | 31 | 32 | 33 | 34 | 35 | 36
        | 37 | 38 | 39 | 40 | 41 | 42 | 43 | 44 | 45 | 46 | 47 | 48 | 49 | 50 | 51 | 52 | 53
        | 54 | 55 | 56 | 57 | 58 | 59 | 60 | 61 | 62 | 63 | 64 | 65 | 66 | 67 | 68 | 69 | 70
        | 71 | 72 | 73 | 74 | 75 | 76 | 77 | 78 | 79 | 80 | 81 | 82 | 83 | 84 | 85 | 86 | 87
        | 88 | 89 | 90 | 91 | 92 | 93 | 94 | 95 | 96 | 97 | 99 | 100 | 101 | 102 | 103 | 104
        | 105 | 106 | 107 | 108 | 109 | 110 | 111 | 112 | 113 | 114 | 115 | 116 | 117 | 118
        | 119 | 120 | 121 | 122 | 123 | 124 | 125 | 126 | 127 => Parity::Ported,
        _ => return None,
    };
    Some(level)
}

/// Whether this module owns the style, as opposed to the older approximations.
pub fn is_ported(style: i32) -> bool {
    parity(style) == Some(Parity::Ported)
}

/// Everything a ported routine may ask the world for.
pub struct World<'a, T: TileView> {
    pub tiles: &'a T,
    pub target: Option<Target>,
    /// Whether the NPC is standing in liquid.
    pub wet: bool,
    /// Whether its target is.
    pub target_wet: bool,
    pub conditions: Conditions,
    /// Whether the NPC was hit since its last tick.
    pub was_hurt: bool,
    /// How fast the target is moving, for the routines that lead it.
    pub target_velocity: (f32, f32),
    /// How many of each NPC type are alive, for the routines that wait on their escort, their
    /// armour or their swarm. Only the types something asks about are counted.
    pub census: &'a [(u16, usize)],
    /// How many of *this* NPC's own escorts are still in the world, which
    /// [`census`](Self::census) cannot answer because it counts a type across the whole world.
    ///
    /// Only a pal reads it, and only its own two guards are counted: the caller unpacks the
    /// handles the pal recorded through [`Spawn::handle`](crate::game::npc_ai::Spawn::handle), which
    /// is `AI_127_Pal_TryUnpackNPC` (`NPC.cs:43496-43508`). Every other style sees zero: the caller
    /// does not go looking on their behalf.
    pub own_escorts: usize,
    /// For a boss part, where its parent is and how big it is.
    pub parent: Option<boss::skeletron::Parent>,
    /// ...and which state that parent is in.
    pub parent_state: f32,
    /// ...and what fraction of its health it has left.
    pub parent_health: f32,
    /// Where Plantera's hooks have bitten, averaged. `None` when none have.
    pub hooks: Option<(f32, f32)>,
    /// ...and each of them on its own, in slot order. An expert Plantera grows a set of tentacles
    /// per hook that orbit *that hook* rather than the body (`NPC.cs:32235-32248`,
    /// `:32505-32508`), and a tentacle names its own by the index it carries in `ai[3]`.
    pub hook_anchors: &'a [(f32, f32)],
    /// How many of Plantera's tentacles are the body's own (`ai[3] == 0`) rather than a hook's.
    /// Only those regrow, and only against a count of themselves (`NPC.cs:32250-32264`).
    pub body_tentacles: usize,
    /// How many of the Moon Lord's sockets have been broken open without the part dying.
    pub sockets_open: usize,
    /// What the Old One's Army looks like right now, for the crystal and its gates.
    pub army: ArmyView,
    /// The best thing a fairy could lead someone to from here, when one is asking.
    pub treasure: Option<(i32, i32)>,
    /// What a Dark Mage can see around it that decides which spell it casts.
    pub mage: army::mage::MageView,
    /// Whether another of this NPC's own type is still travelling, which is how Plantera's hooks
    /// take turns rather than all letting go at once.
    pub kin_moving: bool,
    /// Whether something of this NPC's own type is already riding the target.
    ///
    /// Only the nebula headcrab asks, and only so that a swarm of them deals with you one at a
    /// time rather than all latching at once.
    pub target_taken: bool,
    /// What this one keeps its distance from: the rest of its own kind for a pirate ghost,
    /// anything alive at all for a shimmerfly. Empty unless the style asks for it, because
    /// building it is a scan of the whole table.
    ///
    /// Each entry is `(x, y, reach)`. The reach is how close the entry has to be before a style
    /// that has no separation distance of its own reacts to it, and zero means "this one is not
    /// something to bolt from at all". Only the shimmerfly reads it, because it is the only style
    /// whose reach is a property of what it is looking at rather than of itself: a hundred pixels
    /// for a hostile, a hundred and fifty for a player (`NPC.cs:34444-34453`). Every other reader
    /// brings its own constant and ignores the third element.
    pub avoid: &'a [(f32, f32, f32)],
    /// A unit push away from whatever nearby the NPC would rather not be next to.
    ///
    /// The routines that read this cannot see other NPCs, so the caller averages the directions
    /// away from anything dangerous close by and hands the result in. Zero means all clear.
    pub crowding: (f32, f32),
    /// The nearest hostile NPC a town resident might fight, if this style reads
    /// [`town_combat`](town_combat::town_combat). `slot` is an NPC table index here, not a player
    /// slot — the caller builds this from the NPC table, not from `targets`.
    pub hostile: Option<Target>,
    /// This NPC's own table slot — `NPC.whoAmI`.
    ///
    /// A routine cannot know its own slot any more than `Spawn::OWN_PARENT`'s own doc comment says
    /// it can; the caller, which owns the table, fills it in. Only the Wall's Hungry reads it, for
    /// a leash multiplier keyed to which of the world's live NPC slots it happens to occupy.
    pub slot: u8,
}

/// What the Old One's Army's fixtures need to know about the event around them.
///
/// The crystal and its gates are the only NPCs whose behaviour is driven by the event rather than
/// the other way round, so this is the one direction the world reaches into a routine.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArmyView {
    /// How often a gate lets one through, in ticks. Zero means the event is not running.
    pub rate: i32,
    /// Whether the gap between waves is holding everything back.
    pub on_hold: bool,
    /// Whether there is still a crystal to defend.
    pub crystal_alive: bool,
    /// The arena's two ends in tiles, once something has surveyed them.
    pub arena: Option<((i32, i32), (i32, i32))>,
}

/// What a ported routine did beyond moving its NPC.
#[derive(Debug, Default)]
pub struct Effects {
    /// NPCs a routine conjured.
    pub spawn: Vec<crate::game::npc_ai::Spawn>,
    pub doors: Vec<fighter::Action>,
    /// Doors a town NPC opened or pulled shut behind itself.
    pub town_doors: Vec<town::DoorAction>,
    pub shots: Vec<Shot>,
    /// A town NPC's melee attack landing on a nearby hostile — see [`MeleeHit`].
    pub melee_hits: Vec<MeleeHit>,
    /// Set when the routine wants its NPC gone.
    pub expired: bool,
    /// Set when the routine killed its NPC outright, as a spore does when it bursts.
    pub died: bool,
    /// Set when a routine's roar should leave everyone nearby slowed.
    pub roared: bool,
    /// A type this NPC should become: a lost girl dropping her disguise, a truffle worm fleeing.
    pub transform: Option<u16>,
    /// Set when what this NPC just did calls in an invasion.
    pub called_invasion: bool,
    /// Set when it went off rather than merely dying, which hurts whatever is next to it.
    pub detonated: bool,
    /// Life this one just carried back to whatever it belongs to.
    pub healed: i32,
    /// How long the thing it just turned into should sit still before doing anything.
    pub rest_for: i32,
    /// Where this NPC wants whatever it is carrying to hang.
    pub carry: Option<(f32, f32)>,
    /// Gates the crystal wants raised, as (tile x, tile y, left gate).
    pub gates: Vec<army::crystal::Gate>,
    /// Set when a gate wants an enemy let out, and from which side.
    pub release: Option<bool>,
    /// Set on the tick the crystal's drama finishes, carrying whether it was won.
    pub army_ended: Option<bool>,
    /// Set when the crystal wants its gates told to shut.
    pub close_gates: bool,
    /// Set on the tick a Dark Mage finishes a summoning.
    pub raising: bool,
    /// Set on the tick Betsy screams, which also brings wyverns out of the lane portals.
    pub screamed: bool,
    /// How far a draining aura reaches, while one is out.
    pub aura: Option<f32>,
    /// Where this NPC wants to be put, once it has finished going.
    pub teleport_to: Option<(f32, f32)>,
    /// Set on the tick the Cultists' tablet finishes breaking, which is what raises their master.
    pub ritual_complete: bool,
    /// Set on the tick the Moon Lord's leech step opens: every living player within 3,000 px of
    /// the head is branded (`NPC.cs:42723-42738`). The enumerating is the server's, because which
    /// players are alive and where they are is not something an AI routine is handed.
    pub brands_players: Option<(f32, f32)>,
    /// Set on each of the leech step's three marks: every brand still in the air whose player is
    /// still carrying `BuffID.MoonLeech` becomes a leech (`NPC.cs:42740-42754`).
    pub harvests_brands: Option<(f32, f32)>,
    /// Set on the one tick the Moon Lord's death drama clears the stage: every True Eye still
    /// hunting is killed and every shot the fight left in the air is dropped
    /// (`NPC.cs:41752-41764`).
    pub cleared_stage: bool,
    /// Minions of this NPC's own that it wants destroyed outright, as (type, how many at most).
    /// The Lunatic Cultist's right-guess cull is the only source.
    pub cull_kin: Option<(u16, usize)>,
    /// Set by a part that wants whatever it hangs off punished for its destruction. A destroyed
    /// Cultist decoy is the only source.
    pub punish_owner: bool,
    /// A buff this NPC wants put straight onto a player, as (player slot, buff id, ticks) — a
    /// latched nebula headcrab riding a head applies `Obstructed` to its rider every tick it sits
    /// there (`NPC.cs:37508-37526`, `player22.AddBuff(163, 59)`).
    pub player_buff: Option<(u8, u16, i32)>,
    /// An item to drop where this NPC stands, outside the kill path. A pal's pet is the only
    /// source: it is handed over rather than looted (`NPC.cs:43481-43489`).
    pub reward: Option<i16>,
}

/// Drive an NPC whose style is [`Parity::Ported`].
///
/// Dispatch lives here rather than beside the approximations so that claiming parity for a style
/// and actually running its routine are the same edit. The final arm is unreachable by
/// construction, and a test walks the whole roster to prove it.
pub fn run<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) -> Effects {
    let mut effects = Effects::default();
    // Read before the routine runs, because a pillar's routine borrows `npc` mutably.
    let npc_shield = npc.shield;
    let target = world.target;
    match npc.stats.ai_style {
        0 => inert::update(npc, target),
        1 => {
            // A slime is "active" — and hops at twice the rate — at night, when hurt, below the
            // surface, or during a Slime Rain (`NPC.AI_001_Slimes`' own `flag3`).
            let active = !world.conditions.day
                || npc.life != npc.life_max
                || npc.position.1 > world.conditions.surface_y
                || world.conditions.slime_rain;
            slime::update(npc, target, npc.on_ground, active, world.wet);
        }
        // Still `false`, but no longer for want of a graveyard: the server does know about one now
        // (`Player::in_graveyard`, off the client's own zone packet). What is missing is the wiring
        // between the two, because this argument is the *target's* graveyard rather than the
        // nearest player's, so it cannot come off [`Conditions`] the way `crimson` and `snow` do
        // and wants a per-NPC lookup that belongs with the AI rather than with the spawner.
        2 => eye::update(npc, world, false),
        6 => worm::update(npc, world, false),
        7 => {
            let home = npc.home.map(|(tile_x, tile_y)| town::Home {
                tile_x,
                floor_y: town::floor_under(world.tiles, tile_x, tile_y, i32::MAX / 2),
            });
            let result = town::update(npc, world, home, rng);
            if result.door != town::DoorAction::None {
                effects.town_doors.push(result.door);
            }
            if let Some(shot) = result.shot {
                effects.shots.push(shot);
            }
            if let Some(hit) = result.melee {
                effects.melee_hits.push(hit);
            }
        }
        5 => {
            if let Some(shot) = eater::update(npc, world, rng, world.conditions.expert) {
                effects.shots.push(shot);
            }
        }
        3 => {
            // A Goblin Archer with a negative `ai[3]` is one of a distressed pal's two guards, and
            // for as long as it is one it does not hunt at all (`NPC.cs:57553-57592`, inside
            // `AI_003_Fighters` and ahead of everything that walks):
            //
            // ```csharp
            // if (type == 111)
            // {
            //     if (ai[3] < 0f) TargetClosest(faceTarget: false);
            //     if (ai[3] < 0f && (justHit || Distance(Main.player[target].Center) < 200f))
            //     { ai[3] = 0f; ai[0] = 0f; netUpdate = true; }
            //     if (ai[3] < 0f)
            //     {
            //         directionY = -1; flag = false;
            //         velocity.X *= 0.93f;
            //         if ((double)velocity.X > -0.1 && (double)velocity.X < 0.1) velocity.X = 0f;
            //         int num56 = (int)(0f - ai[3] - 1f);
            //         int num57 = Math.Sign(Main.npc[num56].Center.X - base.Center.X);
            //         if (num57 != direction) { velocity.X = 0f; direction = num57; netUpdate = true; }
            //         if (ai[0] < 1000f) ai[0] = 1000f;
            //         if (++ai[0] >= 1300f) { ai[0] = 1000f; netUpdate = true; }
            //         return;
            //     }
            // }
            // ```
            //
            // Being hit, or anybody coming within two hundred pixels, clears the back-link and the
            // idle counter in one go and the archer falls straight through into the ordinary
            // fighter on the same tick, exactly as it does there. Vanilla's trailing
            // `if (ai[0] >= 1000f) ai[0] = 0f;` (`:57588-57591`) is unreachable: the only path out
            // of the guard state already zeroes `ai[0]`.
            //
            // `flag` is `AI_003_Fighters`' own "may jump" latch and `directionY = -1` its "look
            // up"; neither survives into this server's fighter, which is not a line-for-line port,
            // so neither is transcribed. The stop, the facing and the idle band are.
            if npc.npc_type == terrustia_proto::npc_params::PAL_ESCORT && npc.ai[3] < 0.0 {
                // Narrowing: vanilla reads `Main.player[target].Center` unconditionally, since
                // `target` is already a live index by the time `AI_003_Fighters` runs. This adds
                // `.filter(|t| t.alive)` on top, which only matters if `world.target` can ever
                // name a player this tick has already removed; strictly safer either way.
                let woken = world.was_hurt
                    || world.target.filter(|t| t.alive).is_some_and(|t| {
                        let (cx, cy) = npc.center();
                        (t.center.0 - cx).hypot(t.center.1 - cy)
                            < terrustia_proto::npc_params::PAL_ESCORT_WAKE
                    });
                if woken {
                    npc.ai[3] = 0.0;
                    npc.ai[0] = 0.0;
                    npc.dirty = true;
                } else {
                    npc.direction_y = -1;
                    npc.velocity.0 *= terrustia_proto::npc_params::PAL_DRAG;
                    if npc.velocity.0.abs() < 0.1 {
                        npc.velocity.0 = 0.0;
                    }
                    // The pal it is standing over, which the caller resolves from the same
                    // back-link. `Math.Sign` answers zero when the two centres line up exactly and
                    // `signum` would answer one, so it is spelled out rather than borrowed.
                    //
                    // Narrowing: vanilla reads `Main.npc[num56].Center` unconditionally, a stale
                    // slot and all, and always re-faces from it. This skips the whole facing step
                    // when `world.parent` is `None` rather than facing toward whatever the last
                    // occupant of a freed slot happens to be.
                    if let Some(pal) = world.parent {
                        let gap = pal.center().0 - npc.center().0;
                        let facing = match gap.partial_cmp(&0.0) {
                            Some(std::cmp::Ordering::Greater) => 1i8,
                            Some(std::cmp::Ordering::Less) => -1i8,
                            _ => 0i8,
                        };
                        if facing != npc.direction {
                            npc.velocity.0 = 0.0;
                            npc.direction = facing;
                            npc.dirty = true;
                        }
                    }
                    npc.ai[0] =
                        npc.ai[0].max(terrustia_proto::npc_params::PAL_ESCORT_IDLE_FROM) + 1.0;
                    if npc.ai[0] >= terrustia_proto::npc_params::PAL_ESCORT_IDLE_TO {
                        npc.ai[0] = terrustia_proto::npc_params::PAL_ESCORT_IDLE_FROM;
                        npc.dirty = true;
                    }
                    return effects;
                }
            }
            let action = fighter::update(npc, world.tiles, target, world.conditions);
            if action != fighter::Action::None {
                effects.doors.push(action);
            }
            // The two type splits `AI_003_Fighters` makes on its own life (`NPC.cs:57603-57610`),
            // both unconditional and both the server's (`Main.netMode != 1`):
            //
            // ```csharp
            // if (Main.netMode != 1 && type == 198 && (double)life <= (double)lifeMax * 0.55) Transform(199);
            // if (Main.netMode != 1 && type == 348 && (double)life <= (double)lifeMax * 0.55) Transform(349);
            // ```
            //
            // A Lihzahrd cracks open into a Lihzahrd Crawler, which is the whole reason 199 exists:
            // nothing in `NPC.Spawner` ever spawns one, so it arrives only this way. A wounded
            // Nutcracker starts spinning, the same 55% and the same `aiStyle` 3, so the pair stays
            // together here as it does there. The comparison is vanilla's own `double` one.
            const LIHZAHRD: u16 = 198;
            const LIHZAHRD_CRAWLER: u16 = 199;
            const NUTCRACKER: u16 = 348;
            const NUTCRACKER_SPINNING: u16 = 349;
            let splits_into = match npc.npc_type {
                LIHZAHRD => Some(LIHZAHRD_CRAWLER),
                NUTCRACKER => Some(NUTCRACKER_SPINNING),
                _ => None,
            };
            if let Some(into) = splits_into
                && f64::from(npc.life) <= f64::from(npc.life_max) * 0.55
            {
                effects.transform = Some(into);
            }
        }
        9 => effects.died = orb::update(npc, world, rng),
        19 => {
            if let Some(shot) = ambush::antlion(npc, world) {
                effects.shots.push(shot);
            }
        }
        20 => track::spike_ball(npc, target, rand::Rng::random_range(rng, 0..15)),
        13 => {
            let growth = rooted::plant(npc, world, rng);
            effects.died = growth.uprooted;
            effects.shots.extend(growth.shot);
            effects.spawn.extend(growth.spawn);
        }
        17 => rooted::vulture(npc, world),
        8 => {
            let cast = caster::update(npc, world, rng);
            if let Some((npc_type, position)) = cast.summon {
                effects.spawn.push(crate::game::npc_ai::Spawn {
                    handle: None,
                    npc_type,
                    position,
                    velocity: (0.0, 0.0),
                    parent: None,
                    ai: [None; 4],
                });
            }
            if let Some(shot) = cast.shot {
                effects.shots.push(shot);
            }
        }
        67 => snail::update(npc, world, rng),
        4 => effects.spawn.extend(boss::eye::update(npc, world, rng)),
        11 => {
            let head = boss::skeletron::head(npc, world, rng);
            effects.spawn.extend(head.spawn);
            effects.shots.extend(head.shots);
        }
        27 => {
            let advance = boss::wall::wall(
                npc,
                world,
                world.count(terrustia_proto::npc_params::WALL_LEECH),
                rng,
            );
            effects.spawn.extend(advance.spawn);
            effects.shots.extend(advance.shots);
            effects.expired = advance.gone;
        }
        28 => {
            if let Some(shot) = boss::wall::eye(npc, world, world.parent, world.parent_health) {
                effects.shots.push(shot);
            }
            effects.expired = world.parent.is_none();
        }
        29 => {
            effects.expired =
                !boss::wall::hungry(npc, world, world.parent, world.parent_health, world.slot);
        }
        123 => {
            let rampage = boss::deerclops::update(npc, world, rng);
            effects.shots.extend(rampage.shots);
            effects.roared = rampage.roared;
            effects.expired = rampage.gone;
        }
        43 => {
            let hive = boss::queen_bee::update(npc, world, rng);
            effects.spawn.extend(hive.bees);
            effects.shots.extend(hive.stingers);
        }
        12 => {
            // A hand is bound to its head, whose state the caller reads off the NPC table.
            let outcome = boss::skeletron::hand(
                npc,
                world.parent,
                world.parent_state == boss::skeletron::HOVERING,
                world.parent_state == boss::skeletron::LEAVING,
                world.target,
                world.conditions.expert,
            );
            effects.expired = outcome == boss::skeletron::HandOutcome::Orphaned;
        }
        54 => {
            // The creeper count and the biome are the caller's to know.
            let swarm = boss::brain::update(
                npc,
                world,
                world.count(terrustia_proto::npc_params::CREEPER),
                world.conditions.crimson,
                world.target_velocity,
                rng,
            );
            for (position, velocity) in swarm.creepers {
                effects.spawn.push(crate::game::npc_ai::Spawn {
                    handle: None,
                    npc_type: terrustia_proto::npc_params::CREEPER,
                    position,
                    velocity,
                    parent: None,
                    ai: [None; 4],
                });
            }
            effects.expired = swarm.gone;
        }
        15 => {
            let court = boss::king_slime::update(npc, world, rng);
            for (npc_type, position, velocity, ai) in court.shed {
                effects.spawn.push(crate::game::npc_ai::Spawn {
                    handle: None,
                    npc_type,
                    position,
                    velocity,
                    parent: None,
                    ai,
                });
            }
        }
        126 => mimic::update(npc, world, rng),
        23 => hardmode::hoverers::flying_weapon(npc, world),
        39 => hardmode::roller::roller(npc, world, rng),
        64 => critter::firefly(npc, world, rng),
        86 => hardmode::swooper::swooper(npc, world),
        57 => effects
            .shots
            .extend(boss::tree::tree(npc, world, rng).shots),
        69 => {
            let out = boss::fishron::fishron(npc, world);
            effects.shots.extend(out.shots);
            effects.spawn.extend(out.spawn);
        }
        71 => effects.died = boss::fishron::sharkron(npc, world),
        51 => {
            let out = boss::plantera::plantera(npc, world, plantera_state(world), rng);
            effects.shots.extend(out.shots);
            effects.spawn.extend(out.spawn);
        }
        52 => {
            // A hook waits its turn: it will not let go while another is still travelling.
            let out = boss::plantera::hook(npc, world, world.parent, world.kin_moving, rng);
            effects.expired = out.spent;
        }
        53 => {
            let out = boss::plantera::tentacle(npc, world, world.parent, rng);
            effects.expired = out.spent;
        }
        45 => {
            let state = golem_state(npc, world);
            let out = boss::golem::body(npc, world, state);
            effects.spawn.extend(out.spawn);
            effects.expired = out.spent;
        }
        46 => {
            let state = golem_state(npc, world);
            let out = boss::golem::head(npc, world, world.parent, state);
            effects.shots.extend(out.shots);
            effects.expired = out.spent;
        }
        47 => {
            let state = golem_state(npc, world);
            let out = boss::golem::fist(npc, world, world.parent, state);
            effects.expired = out.spent;
        }
        48 => {
            // The free head reads the body, not itself, for every threshold it has (GOL-M13), so
            // it needs the same parent link and the same pace the attached parts get.
            let state = golem_state(npc, world);
            let out = boss::golem::free_head(npc, world, world.parent, state);
            effects.shots.extend(out.shots);
            effects.expired = out.spent;
        }
        32 => {
            let out = boss::prime::prime_head(npc, world);
            effects.expired = out.leaving && npc.time_left <= 0;
            effects.spawn.extend(out.spawn);
        }
        33..=36 => {
            let out = boss::prime::prime_arm(npc, world, world.parent, rng);
            effects.shots.extend(out.shots);
            effects.expired = out.spent;
        }
        37 => {
            let out = boss::destroyer::destroyer(npc, world, rng);
            effects.shots.extend(out.shots);
            // MECH-1: daybreak (or the last player gone) sends it home. Vanilla caps its despawn
            // timer (`EncourageDespawn`) so it counts out; here bosses skip `tick_life`, so the
            // flee is routed straight to `expired` (`time_left = 0`), which the server reaps.
            effects.expired = out.fleeing;
        }
        30 | 31 => {
            let out = boss::twins::twin(npc, world, rng);
            effects.shots.extend(out.shots);
            // MECH-1: as the Destroyer above. At dawn or with nobody left, both eyes climb away
            // and go. Left unconsumed the Twins hung in the sky for ever after daybreak.
            effects.expired = out.fleeing;
        }
        88 => {
            // Its brood is its eggs and its hatchlings together.
            let brood = world.count(terrustia_proto::npc_params::MOTHRON_EGG)
                + world.count(terrustia_proto::npc_params::MOTHRON_SPAWN_TYPE);
            let out = hardmode::mothron::mothron(npc, world, brood, rng);
            effects.spawn.extend(out.spawn);
        }
        94 => {
            let out = hardmode::pillar::pillar(npc, world, npc_shield);
            effects.expired = out.spent;
            effects.died = out.died;
        }
        87 => {
            hardmode::big_mimic::big_mimic(npc, world, rng);
        }
        117 => {
            let helpers = world.count(terrustia_proto::npc_params::NAUTILUS_HELPER);
            let out = hardmode::nautilus::dreadnautilus(npc, world, helpers, rng);
            effects.shots.extend(out.base.shots);
            // Each helper arrives through a portal rather than simply appearing.
            for at in out.summons {
                effects.shots.push(Shot {
                    projectile: terrustia_proto::projectile::ids::NAUTILUS_HELPER_PORTAL,
                    damage: 0,
                    position: at,
                    velocity: (0.0, 0.0),
                    time_left: 300,
                    ai: [0.0; 3],
                });
            }
        }
        97 => {
            let out = hardmode::teleporter::nebula_brain(npc, world, rng);
            effects.shots.extend(out.base.shots);
            // C7-01 SEAM: `out.hurried_floaters` is produced on the teleport tick but not consumed
            // here, and this is deliberate, not an oversight. Vanilla hurries the brain's live
            // floaters by subtracting from their charge timer (`NPC.cs:39982-40002`, proj 574's
            // `ai[0] -= hurry` for every floater whose `ai[1] == whoAmI`, only while none has
            // launched). Our NEBULA_FLOATER (`ai_style 102`) has no charge-up AI: it is spawned with
            // a launch velocity and flies straight, so there is no `ai[0]` timer to hurry, and
            // `projectile::step` is not passed the player target a charge-then-home floater needs.
            // Honouring the hurry therefore depends on the floater charge-up (projectile-lane L2-14,
            // not landed) plus owner tracking and a server-side pass over the projectile store. Left
            // as a documented seam rather than faked: a hold-then-release floater would not be the
            // homing attack the hurry exists to bring forward. The `hurried_floaters` flag is kept so
            // the consumer is a one-line addition once the charge-up lands.
            let _ = out.hurried_floaters;
        }
        85 => {
            // Another of its kind already on the player's head is what stops this one trying.
            let taken = world.target_taken;
            hardmode::hunter::pathfinder(npc, world, taken);
            // Latched (`hunter::path::LATCHED`, ai[0] == 5.0): a nebula headcrab riding a head
            // keeps applying Obstructed to its rider, every tick it sits there, for as long as
            // that player is still there to receive it (`NPC.cs:37508-37526`).
            if npc.ai[0] == 5.0
                && let Some(t) = target.filter(|t| t.alive)
            {
                effects.player_buff = Some((t.slot, 163, 59));
            }
        }
        90 => {
            hardmode::hunter::mothron_spawn(npc, world);
        }
        75 => {
            let out = hardmode::rider::rider(npc, world, world.parent, rng);
            effects.shots.extend(out.shots);
            effects.expired = out.spent;
        }
        74 => {
            let out = hardmode::charger::charger(npc, world, rng);
            effects.expired = out.spent;
            effects.detonated = out.detonated;
        }
        68 => {
            let landing = bird::waterfowl(npc, world, rng);
            if let Some(walker) = landing.becomes {
                effects.transform = Some(walker);
                effects.rest_for = landing.rests_for;
            }
        }
        102 => effects
            .shots
            .extend(hardmode::sand::sand_elemental(npc, world, rng).shots),
        103 => hardmode::sand::sand_shark(npc, world),
        40 => {
            let out = hardmode::crawler::crawler(npc, world, rng);
            effects.shots.extend(out.shots);
            effects.transform = out.became;
        }
        41 => {
            let out = hardmode::leaper::leaper(npc, world);
            effects.expired = out.spent;
            effects.detonated = out.detonated;
        }
        25 => {
            // The Snowman Gangsta only takes an interest during the Frost Moon, which this server
            // does not run yet, so it hops without ever turning to face anybody.
            let indifferent = npc.npc_type == terrustia_proto::npc_params::SNOWMAN_GANGSTA;
            hardmode::hopper::hopper(npc, world, indifferent);
        }
        80 => {
            let out = hardmode::invasion::probe(npc, world);
            effects.expired = out.spent;
            effects.called_invasion = out.called_the_invasion;
        }
        93 => {
            let cannon = world.count(terrustia_proto::npc_params::DUTCHMAN_GUN);
            let out = hardmode::invasion::dutchman(npc, world, rng, cannon);
            effects.spawn.extend(out.spawn);
            effects.expired = out.spent;
            // A death, not a quiet despawn, exactly as the saucer above: losing the last cannon is
            // `StrikeNPCNoInteraction(9999, 0f, 0)` in source (`NPC.cs:39275`), so it has to drop
            // its loot and count toward the invasion.
            effects.died = out.died;
        }
        72 => {
            // A pinned part is drawn on its parent, so it needs the parent's centre rather than
            // its corner.
            let at = world.parent.map(|p| p.center());
            effects.expired = hardmode::fixtures::pinned(npc, at).spent;
        }
        73 => {
            // Only the Martian turret spends its first two seconds deploying, untouchable; the
            // other style-73 types are simply standing there already.
            let materialises = npc.npc_type == terrustia_proto::npc_params::MARTIAN_TURRET;
            let out = hardmode::fixtures::stationary_caster(npc, world, rng, materialises);
            effects.shots.extend(out.shots);
        }
        92 => effects.expired = hardmode::fixtures::training_dummy(npc, world).spent,
        122 => effects.expired = hardmode::fixtures::pirate_ghost(npc, world).spent,
        124 => hardmode::fixtures::slime_chest(npc),
        127 => {
            // Its *own* two, not every Goblin Archer in the world: this read
            // `world.count(PAL_ESCORT)`, so any archer anywhere (a goblin army has dozens) held
            // every pal in its waiting state, and killing an unrelated one released it. The count
            // is now vanilla's own unpack of the pal's two handles, done by the caller.
            let out = hardmode::fixtures::pal(npc, world, world.own_escorts);
            effects.spawn.extend(out.spawn);
            effects.reward = out.reward;
            effects.expired = out.spent;
        }
        49 => effects.shots.extend(hardmode::hoverers::nimbus(npc, world)),
        56 => hardmode::drifters::dungeon_spirit(npc, world),
        62 => effects
            .shots
            .extend(hardmode::hoverers::copter(npc, world, rng)),
        70 => {
            effects.expired = hardmode::hoverers::detonating_bubble(npc, world, rng).spent;
        }
        100 => effects.expired = hardmode::hoverers::ancient_light(npc).spent,
        101 => {
            let out =
                hardmode::hoverers::ancient_doom(npc, world.parent.map(|_| world.parent_health));
            effects.shots.extend(out.shots);
            effects.expired = out.spent;
        }
        116 => {
            let (cx, cy) = npc.center();
            let line = critter::water_line(
                world.tiles,
                (cx / crate::game::npc::TILE) as i32,
                (cy / crate::game::npc::TILE) as i32,
            );
            hardmode::hoverers::water_strider(npc, line, rng, world.wet);
        }
        63 => hardmode::drifters::flocko(npc, world),
        89 => {
            let out =
                hardmode::drifters::mothron_egg(npc, world.was_hurt, world.conditions.expert, rng);
            effects.transform = out.became;
        }
        95 => effects.transform = hardmode::drifters::stardust_cell(npc).became,
        96 => {
            let out = hardmode::drifters::stardust_jellyfish(npc, world, rng);
            effects.shots.extend(out.shot);
        }
        99 => effects.expired = hardmode::drifters::solar_goop(npc).spent,
        // Nothing but a marker: it removes itself the moment it is asked to do anything.
        104 => effects.expired = true,
        // A tumbleweed in a sandstorm is carried by the wind rather than merely rolling.
        26 => {
            let carried = world.conditions.sandstorm && world.conditions.desert;
            let out = tumbleweed::update(npc, world, carried, rng);
            effects.shots.extend(out.shots);
            effects.died = out.died;
        }
        113 | 125 => {
            if balloon::update(npc, world) == balloon::Outcome::Popped {
                effects.died = true;
            }
            effects.carry = Some(balloon::carry_point(npc));
        }
        114 => dragonfly::update(npc, world, rng),
        65 => effects.expired = critter::butterfly(npc, world, rng),
        // One hit in six knocks it down, and only in expert (`NPC.cs:39016`).
        91 => granite::update(
            npc,
            world,
            world.was_hurt && world.conditions.expert && rand::Rng::random_ratio(rng, 1, 6),
        ),
        22 => {
            // The drift it picks when it has nothing else to go on: -1.5, 0 or 1.5.
            let drift = rand::Rng::random_range(rng, -1..2) as f32 * 1.5;
            effects.shots.extend(haunt::update(npc, world, drift, rng));
        }
        38 => {
            if let Some(shot) = frost::update(npc, world) {
                effects.shots.push(shot);
            }
        }
        10 => {
            let bite = skull::update(npc, world, rng);
            effects.shots.extend(bite.shot);
            effects.spawn.extend(bite.spawn);
        }
        18 => swimmer::jellyfish(npc, world),
        44 => swimmer::flying_fish(npc, world),
        21 => track::wheel(npc, target),
        115 => critter::ladybug(npc, world, rng),
        118 => critter::seahorse(npc, world, rng),
        119 => effects.shots.extend(critter::dandelion(npc, world, rng)),
        42 => effects.transform = ambush::lost_girl(npc, world),
        66 => effects.transform = grub::update(npc, world, rng),
        14 => {
            let out = bat::update(npc, world, rng);
            effects.shots.extend(out.shot);
            effects.transform = out.became;
        }
        16 => fish::update(npc, target, world.wet, world.target_wet),
        24 => {
            let out = bird::update(npc, world);
            npc.no_gravity = !out.wants_gravity;
            effects.transform = out.became;
        }
        50 => {
            let hit_something = npc.collide_x || npc.collide_y;
            effects.died = spore::update(npc, target, hit_something, world.conditions.expert)
                == spore::Outcome::Burst;
        }
        55 => {
            // The Brain's position is threaded in through ai[2..3] by the server, which knows
            // where every NPC is; a creeper with no Brain removes itself.
            let brain = (npc.ai[2] != 0.0 || npc.ai[3] != 0.0).then_some((npc.ai[2], npc.ai[3]));
            // `(Main.expertMode && Main.rand.Next(100) == 0) || Main.rand.Next(200) == 0`
            // (`NPC.cs:32935`): expert doubles how often one breaks off to charge.
            let charging = (world.conditions.expert
                && rand::Rng::random_ratio(rng, 1, creeper::CHARGE_CHANCE_EXPERT))
                || rand::Rng::random_ratio(rng, 1, creeper::CHARGE_CHANCE);
            effects.expired =
                creeper::update(npc, world, brain, charging) == creeper::Outcome::BrainGone;
        }
        58 => {
            let out = boss::moon::pumpking(npc, world, rng);
            effects.shots.extend(out.shots);
            effects.spawn.extend(out.spawn);
            effects.expired = out.spent;
        }
        59 => {
            let out = boss::moon::pumpking_blade(npc, world.parent);
            effects.expired = out.spent;
        }
        60 => {
            let out = boss::moon::ice_queen(npc, world, rng);
            effects.shots.extend(out.shots);
            effects.spawn.extend(out.spawn);
            effects.expired = out.spent;
        }
        61 => {
            let out = boss::moon::santa(npc, world, rng);
            effects.shots.extend(out.shots);
            effects.spawn.extend(out.spawn);
            effects.expired = out.spent;
        }
        76 => {
            // SAU-1: the core watches its own four guns to know when the fight ends. Counted here
            // and handed in, the way the Moon Lord core is told how many of its eyes are open.
            let guns = world.count(terrustia_proto::npc_params::MARTIAN_SAUCER_TURRET)
                + world.count(terrustia_proto::npc_params::MARTIAN_SAUCER_CANNON);
            let out = hardmode::saucer::core(npc, world, guns);
            effects.spawn.extend(out.spawn);
            effects.shots.extend(out.shots);
            effects.expired = out.spent;
            // A death, not a quiet despawn: the Classic-mode finish drops the loot and records the
            // kill. Routed through `expired` (the old wiring) it vanished with nothing.
            effects.died = out.died;
        }
        77 => {
            // A socket that has been broken open is still on the field, so counting the parts is
            // not enough to know how far along the fight is.
            let parts = world.count(terrustia_proto::npc_params::MOON_LORD_HAND)
                + world.count(terrustia_proto::npc_params::MOON_LORD_HEAD);
            let open = (3usize.saturating_sub(parts) + world.sockets_open).min(3);
            let out = boss::moon_lord::core(npc, world, open);
            effects.spawn.extend(out.spawn);
            // A *death*, not an expiry. The core cannot be killed by damage outright: struck down
            // it enters a ten-second death drama (`checkdead` sets `ai[0] == 2`), and only the end
            // of that drama is the kill. Routing it through `expired` removed it quietly: no
            // luminite, and `downed_moon_lord` never set, so the world did not record the win.
            effects.died = out.spent;
            effects.cleared_stage = out.cleared_stage;
        }
        78 | 79 => {
            let out = boss::moon_lord::eye_socket(npc, world, world.parent);
            effects.shots.extend(out.shots);
            effects.spawn.extend(out.spawn);
            effects.expired = out.spent;
            effects.brands_players = out.brands_players;
            effects.harvests_brands = out.harvests_brands;
        }
        81 => {
            let out = boss::moon_lord::free_eye(npc, world, world.parent);
            effects.shots.extend(out.shots);
            effects.expired = out.spent;
        }
        82 => {
            let out = boss::moon_lord::leech(npc, world.parent);
            effects.expired = out.spent;
            effects.healed = out.healed;
        }
        83 => {
            let out = if npc.npc_type == terrustia_proto::npc_params::CULTIST_TABLET {
                boss::tablet::tablet(
                    npc,
                    world,
                    world.count(terrustia_proto::npc_params::CULTIST_DEVOTE)
                        + world.count(terrustia_proto::npc_params::CULTIST_ARCHER),
                )
            } else {
                boss::tablet::devote(npc, world.parent)
            };
            effects.shots.extend(out.shots);
            effects.spawn.extend(out.spawn);
            effects.expired = out.spent;
            effects.ritual_complete = out.ritual_complete;
        }
        84 => {
            let out = boss::cultist::cultist(
                npc,
                world,
                world.parent,
                world.count(terrustia_proto::npc_params::CULTIST_CLONE),
                rng,
            );
            effects.shots.extend(out.shots);
            effects.spawn.extend(out.spawn);
            effects.expired = out.spent;
            effects.teleport_to = out.move_to;
            // The ritual's two consequences, both of which reach past this NPC: a right guess
            // destroys some of the group, a wrong one stuns whatever the decoy was a copy of.
            effects.cull_kin = out
                .cull_clones
                .map(|n| (terrustia_proto::npc_params::CULTIST_CLONE, n));
            effects.punish_owner = out.punish_owner;
        }
        105 => {
            let out = army::crystal::crystal(npc, world.army.arena);
            effects.gates = out.gates;
            effects.army_ended = out.ended;
            effects.close_gates = out.close_gates;
            effects.expired = out.spent;
        }
        106 => {
            let out = army::crystal::portal(
                npc,
                world.army.rate.max(1),
                world.army.on_hold,
                world.army.crystal_alive,
            );
            effects.release = out.release;
            effects.expired = out.spent;
        }
        107 => {
            let out = army::walker::improved_walker(npc, world, rng);
            effects.shots.extend(out.shots);
            effects.detonated = out.burst;
            effects.expired = out.spent;
            effects.aura = out.aura;
        }
        108 => {
            let out = army::flyer::diving(npc, world, rng);
            effects.detonated = out.burst;
            effects.expired = out.spent;
        }
        109 => {
            let out = army::mage::dark_mage(npc, world, world.mage);
            effects.shots.extend(out.shots);
            effects.raising = out.raising;
        }
        110 => {
            let out = army::betsy::betsy(
                npc,
                world,
                world.count(terrustia_proto::npc_params::BETSY_WYVERN),
                rng,
            );
            effects.shots.extend(out.shots);
            effects.spawn.extend(out.spawn);
            effects.screamed = out.screamed;
        }
        111 => {
            effects
                .shots
                .extend(army::bug::lightning_bug(npc, world, rng));
        }
        112 => {
            let out = fairy::fairy(npc, world, world.treasure, rng);
            effects.expired = out.spent;
        }
        120 => {
            let out = boss::empress::empress(
                npc,
                world,
                world.conditions.day,
                world.conditions.expert,
                rng,
            );
            effects.shots.extend(out.shots);
            effects.expired = out.spent;
        }
        121 => {
            let out = boss::queen_slime::queen_slime(npc, world, rng);
            effects.shots.extend(out.shots);
            effects.teleport_to = out.teleport_to;
            // QS-1: the minions she sheds as she is worn down. Produced but never consumed, the
            // whole mechanic would be invisible.
            effects.spawn.extend(out.spawn);
        }
        style => unreachable!("style {style} claims parity but has no routine here"),
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;
    use terrustia_proto::{npc_data::npc_stats, prehardmode::PRE_HARDMODE};

    #[derive(Default)]
    struct Flat(HashMap<(i32, i32), Tile>);

    impl TileView for Flat {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    /// A claimed port has to be a wired one.
    ///
    /// Style 24 spent a while marked [`Parity::Ported`] while the dispatch still sent birds to the
    /// old approximation, which is exactly the failure the parity table exists to prevent. Running
    /// every ported type through [`run`] makes the claim and the wiring the same edit: the final
    /// arm of that match is `unreachable!`, so a style marked ported without a routine panics here.
    ///
    /// It walks the *whole* roster rather than the pre-hardmode list. It used to walk only the
    /// latter, and style 58 sat claimed-but-unwired behind that gap until a Pumpking crashed a
    /// profiling run: nothing before hardmode uses it, so nothing tested it.
    #[test]
    fn every_ported_style_actually_runs_its_routine() {
        let mut tiles = Flat::default();
        for x in 0..400 {
            for y in 700..710 {
                tiles.0.insert((x, y), Tile::block(1));
            }
        }
        let mut rng = rand::rngs::SmallRng::seed_from_u64(1);
        let mut seen = Vec::new();
        // Every unwired style at once rather than the first one: `run`'s last arm panics, so the
        // probe below catches that and collects the style instead of stopping the whole test.
        let mut unwired = std::collections::BTreeSet::new();
        let loud = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        for npc_type in 0..terrustia_proto::npc_data::NPC_COUNT {
            let Some(stats) = npc_stats(npc_type) else {
                continue;
            };
            if parity(stats.ai_style) != Some(Parity::Ported) {
                continue;
            }
            let mut npc = Npc::new(npc_type, (3000.0, 11_100.0), 1).expect("spawnable");
            let world = World {
                tiles: &tiles,
                target: Some(Target {
                    slot: 0,
                    center: (3200.0, 11_100.0),
                    velocity: (0.0, 0.0),
                    alive: true,
                }),
                wet: false,
                target_wet: false,
                conditions: Conditions::default(),
                was_hurt: false,
                target_velocity: (0.0, 0.0),
                hostile: None,
                census: &[],
                own_escorts: 0,
                parent: None,
                parent_state: 0.0,
                parent_health: 1.0,
                crowding: (0.0, 0.0),
                avoid: &[],
                target_taken: false,
                hooks: None,
                hook_anchors: &[],
                body_tentacles: 0,
                kin_moving: false,
                sockets_open: 0,
                army: ArmyView::default(),
                treasure: None,
                mage: Default::default(),
                slot: 0,
            };
            unwired.extend(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut probe = npc.clone();
                    let mut probe_rng = rand::rngs::SmallRng::seed_from_u64(1);
                    run(&mut probe, &world, &mut probe_rng);
                }))
                .err()
                .map(|_| stats.ai_style),
            );
            if unwired.contains(&stats.ai_style) {
                continue;
            }
            // Panics here rather than silently doing nothing, which is the point.
            let _ = run(&mut npc, &world, &mut rng);
            seen.push(stats.ai_style);
        }
        seen.sort_unstable();
        seen.dedup();
        assert!(!seen.is_empty());
        std::panic::set_hook(loud);
        assert!(
            unwired.is_empty(),
            "styles claiming parity with no routine wired up: {unwired:?}"
        );
    }

    /// A fighter standing on solid ground with somebody just out of reach.
    fn fighter_world(tiles: &Flat) -> World<'_, Flat> {
        World {
            tiles,
            target: Some(Target {
                slot: 0,
                center: (3200.0, 11_100.0),
                velocity: (0.0, 0.0),
                alive: true,
            }),
            wet: false,
            target_wet: false,
            conditions: Conditions::default(),
            was_hurt: false,
            target_velocity: (0.0, 0.0),
            hostile: None,
            census: &[],
            own_escorts: 0,
            parent: None,
            parent_state: 0.0,
            parent_health: 1.0,
            crowding: (0.0, 0.0),
            avoid: &[],
            target_taken: false,
            hooks: None,
            hook_anchors: &[],
            body_tentacles: 0,
            kin_moving: false,
            sockets_open: 0,
            army: ArmyView::default(),
            treasure: None,
            mage: Default::default(),
            slot: 0,
        }
    }

    /// `AI_003_Fighters` splits two of its own types open at 55% life (`NPC.cs:57603-57610`): a
    /// Lihzahrd becomes a Lihzahrd Crawler and a wounded Nutcracker starts spinning.
    ///
    /// The Lihzahrd's half matters most: nothing in `NPC.Spawner` spawns a Lihzahrd Crawler, so
    /// this is the only way one ever exists.
    ///
    /// The exact boundary is pinned at a life max of 20 rather than a round 100, and deliberately:
    /// `100.0 * 0.55` is `55.00000000000001` in `f64`, so a life of 55 is strictly under it and
    /// `<` and `<=` cannot be told apart there. `20.0 * 0.55` is exactly `11.0`, which is the one
    /// place the game's own `<=` is observable. That is vanilla's arithmetic too, not a divergence
    /// introduced here.
    ///
    /// Neutralised by forcing `splits_into` to `None` in the `3 =>` arm of [`run`]: "Lihzahrd
    /// splits at 20%: left None, right Some(199)". Neutralised again by changing `<=` to `<`, which
    /// leaves that one passing and fires the boundary instead: "Lihzahrd splits at exactly 55% of
    /// 20: left None, right Some(199)".
    #[test]
    fn a_wounded_lihzahrd_and_a_wounded_nutcracker_split_at_55_percent() {
        let mut tiles = Flat::default();
        for x in 0..400 {
            for y in 700..710 {
                tiles.0.insert((x, y), Tile::block(1));
            }
        }
        let world = fighter_world(&tiles);
        let split = |npc_type: u16, life_max: i32, life: i32| {
            let mut npc = Npc::new(npc_type, (3000.0, 11_100.0), 1).expect("spawnable");
            npc.life_max = life_max;
            npc.life = life;
            let mut rng = rand::rngs::SmallRng::seed_from_u64(1);
            run(&mut npc, &world, &mut rng).transform
        };

        for (npc_type, into, name) in [(198u16, 199u16, "Lihzahrd"), (348, 349, "Nutcracker")] {
            assert_eq!(split(npc_type, 100, 56), None, "{name} above 55% is unhurt");
            assert_eq!(split(npc_type, 100, 20), Some(into), "{name} splits at 20%");
            // The one life max where `lifeMax * 0.55` lands on a whole number, so the game's `<=`
            // is the difference between splitting and not.
            assert_eq!(
                split(npc_type, 20, 12),
                None,
                "{name} at 12 of 20 is unhurt"
            );
            assert_eq!(
                split(npc_type, 20, 11),
                Some(into),
                "{name} splits at exactly 55% of 20"
            );
        }

        // ...and nothing else in the style does, however badly hurt: a Zombie stays a Zombie.
        assert_eq!(split(3, 100, 1), None, "a Zombie turned into something");
    }

    /// A Goblin Archer raised as a pal's guard stands over it and does not hunt, until it is hit or
    /// somebody walks up to it (`NPC.cs:57553-57592`).
    ///
    /// Fails before the fix: nothing in this server read the pal's `ai[3]` back-link, so both
    /// guards charged the player from the moment they were raised and the pal was left standing on
    /// its own, which is not the encounter.
    ///
    /// Neutralised by deleting the whole `npc.npc_type == PAL_ESCORT && npc.ai[3] < 0.0` block from
    /// the `3 =>` arm of [`run`]: the "stands its ground" and "faces its pal" assertions fail, the
    /// archer walking off toward the player. Neutralised again by dropping only the `world.was_hurt`
    /// term from `woken`: the "being hit wakes it" assertion fails. And a third time by dropping the
    /// `PAL_ESCORT_WAKE` distance term: the "walking up to it wakes it" assertion fails.
    #[test]
    fn a_pals_goblin_archer_guards_it_until_hit_or_approached() {
        let mut tiles = Flat::default();
        for x in 0..400 {
            for y in 700..710 {
                tiles.0.insert((x, y), Tile::block(1));
            }
        }
        let escort = terrustia_proto::npc_params::PAL_ESCORT;
        // The pal it is guarding, three tiles to its left, as `parents` would report it.
        let pal_at = boss::skeletron::Parent {
            position: (2900.0, 11_100.0),
            size: (30.0, 30.0),
            rotation: 0.0,
            scale: 1.0,
            velocity: (0.0, 0.0),
            direction: 1,
            sprite_direction: 1,
            time_left: 750,
            state: 0.0,
            phase: 0.0,
            health: 1.0,
        };
        let guard = || {
            let mut npc = Npc::new(escort, (3000.0, 11_100.0), 1).expect("goblin archer");
            // `ai[3] = -(whoAmI + 1)` for a pal in slot 4.
            npc.ai[3] = -5.0;
            npc.direction = 1;
            npc.velocity = (2.0, 0.0);
            npc
        };
        let mut rng = rand::rngs::SmallRng::seed_from_u64(1);

        // The player is at 3200, two hundred pixels being what wakes it: `fighter_world` stands
        // them just inside that, so push them well out for the waiting half.
        let mut asleep = fighter_world(&tiles);
        asleep.target = Some(Target {
            slot: 0,
            center: (9000.0, 11_100.0),
            velocity: (0.0, 0.0),
            alive: true,
        });
        asleep.parent = Some(pal_at);

        let mut g = guard();
        run(&mut g, &asleep, &mut rng);
        assert_eq!(g.ai[3], -5.0, "nothing has woken it, it is still a guard");
        assert!(
            g.velocity.0 < 2.0,
            "it stands its ground rather than walking: {}",
            g.velocity.0
        );
        assert_eq!(
            g.direction, -1,
            "and it faces its pal, which is to its left"
        );
        assert!(
            (1000.0..1300.0).contains(&g.ai[0]),
            "its idle counter runs in its own band: {}",
            g.ai[0]
        );

        // Being hit wakes it, and it is an ordinary Goblin Archer from that tick on.
        let mut hurt = asleep;
        hurt.was_hurt = true;
        let mut h = guard();
        run(&mut h, &hurt, &mut rng);
        assert_eq!(h.ai[3], 0.0, "being hit wakes it");
        assert_eq!(h.ai[0], 0.0, "and clears the idle counter with it");

        // ...and so does simply walking up to it.
        let near = fighter_world(&tiles);
        let mut n = guard();
        assert!(
            (near.target.expect("a target").center.0 - n.center().0).abs()
                < terrustia_proto::npc_params::PAL_ESCORT_WAKE,
            "the player has to be inside the wake distance for this to test it"
        );
        run(&mut n, &near, &mut rng);
        assert_eq!(n.ai[3], 0.0, "walking up to it wakes it");

        // An archer that was never a guard is untouched by any of this.
        let mut plain = Npc::new(escort, (3000.0, 11_100.0), 1).expect("goblin archer");
        run(&mut plain, &near, &mut rng);
        assert_eq!(plain.ai[3], 0.0);
        assert!(
            plain.velocity.0 != 0.0,
            "an ordinary Goblin Archer walks at you"
        );
    }

    #[test]
    fn every_pre_hardmode_style_has_some_implementation() {
        let mut missing = Vec::new();
        for npc_type in PRE_HARDMODE {
            let stats = npc_stats(npc_type).expect("roster types all have stats");
            if parity(stats.ai_style).is_none() {
                missing.push((npc_type, stats.name, stats.ai_style));
            }
        }
        assert!(missing.is_empty(), "no behaviour at all for: {missing:?}");
    }

    /// Every type in the roster runs a routine transcribed from the game, not an approximation.
    ///
    /// This is the goal the whole `ai` module exists to reach, so it is an assertion rather than a
    /// report: anything that slips back to [`Parity::Approximate`] fails here by name.
    #[test]
    fn every_roster_type_is_a_port_rather_than_an_approximation() {
        let mut approximate = Vec::new();
        for npc_type in PRE_HARDMODE {
            let stats = npc_stats(npc_type).expect("stats");
            if parity(stats.ai_style) != Some(Parity::Ported) {
                approximate.push((npc_type, stats.name, stats.ai_style));
            }
        }
        assert!(
            approximate.is_empty(),
            "still approximations: {approximate:?}"
        );
    }

    /// Every NPC the game defines is driven by a ported routine.
    ///
    /// This is the whole point of the exercise, and it is easy to lose by accident: adding a
    /// dispatch arm without adding the number to the parity list leaves the routine unreachable
    /// and the style silently unported. So it is asserted rather than reported.
    #[test]
    fn every_npc_in_the_game_has_a_ported_routine() {
        use terrustia_proto::npc_data::{NPC_COUNT, npc_stats};
        let mut orphans = std::collections::BTreeMap::new();
        for npc_type in 0..NPC_COUNT {
            let Some(stats) = npc_stats(npc_type) else {
                continue;
            };
            if parity(stats.ai_style) != Some(Parity::Ported) {
                orphans
                    .entry(stats.ai_style)
                    .or_insert_with(Vec::new)
                    .push(stats.name);
            }
        }
        assert!(
            orphans.is_empty(),
            "styles with no ported routine: {orphans:?}"
        );
    }

    /// How much of the *whole* game is ported, not just what appears before hardmode.
    #[test]
    fn report_full_game_progress() {
        use terrustia_proto::npc_data::{NPC_COUNT, npc_stats};
        let (mut done, mut left) = (0, 0);
        let mut missing_styles = std::collections::BTreeSet::new();
        for npc_type in 0..NPC_COUNT {
            let Some(stats) = npc_stats(npc_type) else {
                continue;
            };
            if parity(stats.ai_style) == Some(Parity::Ported) {
                done += 1;
            } else {
                left += 1;
                missing_styles.insert(stats.ai_style);
            }
        }
        println!(
            "whole game: {done} of {} types ported, {left} left across {} styles",
            done + left,
            missing_styles.len()
        );
        println!("styles left: {missing_styles:?}");
    }

    /// Confusion turns an NPC away from what it is facing, and only on the horizontal axis.
    ///
    /// `SetTargetTrackingValues` ends with an unconditional `if (confused) direction *= -1;`
    /// (`NPC.cs:78576-78579`), with `directionY` untouched. Until 2026-08-31 the `confused` flag
    /// was derived every tick (`buffs.rs:246`) and read by nothing at all, so a Confused enemy
    /// walked straight at its target.
    #[test]
    fn confusion_reverses_the_way_an_npc_faces() {
        const ZOMBIE: u16 = 3;
        const CONFUSED: u16 = 31;
        let target = Target {
            slot: 0,
            center: (3200.0, 11_100.0),
            velocity: (0.0, 0.0),
            alive: true,
        };

        let mut clear = Npc::new(ZOMBIE, (3000.0, 11_100.0), 1).expect("spawnable");
        face(&mut clear, target);
        assert_eq!(clear.direction, 1, "the target is to its right");

        let mut dizzy = Npc::new(ZOMBIE, (3000.0, 11_100.0), 1).expect("spawnable");
        assert!(dizzy.buffs.add(ZOMBIE, CONFUSED, 600), "zombies confuse");
        dizzy.buffs.set_flags(ZOMBIE, 0.0);
        assert!(dizzy.buffs.flags.confused);
        face(&mut dizzy, target);
        assert_eq!(dizzy.direction, -1, "confused, so it turns away");
        assert_eq!(
            dizzy.direction_y, clear.direction_y,
            "vanilla flips `direction` only"
        );
    }

    /// Prints how far the port has got, for the record.
    #[test]
    fn report_parity_progress() {
        let (mut ported, mut approx) = (0, 0);
        for npc_type in PRE_HARDMODE {
            let stats = npc_stats(npc_type).expect("stats");
            match parity(stats.ai_style) {
                Some(Parity::Ported) => ported += 1,
                Some(Parity::Approximate) => approx += 1,
                None => {}
            }
        }
        println!(
            "parity: {ported} of {} roster types ported, {approx} still approximate",
            PRE_HARDMODE.len()
        );
        assert_eq!(ported + approx, PRE_HARDMODE.len());
    }
}
