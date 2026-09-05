//! NPC behaviour.
//!
//! Terraria's `NPC.AI` is roughly 59,000 lines because every type's quirks live inline. These are
//! re-implementations of each *style's* core behaviour rather than line-by-line ports: an enemy
//! chases, jumps, flies or swims the way its style does, using the game's own stats and physics
//! constants, but without the per-type special cases.
//!
//! The styles here cover essentially every ordinary pre-hardmode enemy and critter. Which style a
//! type uses comes from `NpcStats::ai_style`, extracted from the game.

use rand::{Rng, rngs::SmallRng};

use super::npc::{Npc, TileView, step_physics};

/// An NPC a routine wants brought into the world.
///
/// Position and velocity both matter: King Slime throws its slimes, the Brain scatters its
/// creepers, and a caster places its orb exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spawn {
    pub npc_type: u16,
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    /// Set when the spawn is a part of the NPC that asked for it, as Skeletron's hands are.
    ///
    /// A routine cannot know its own slot, so it sets [`Spawn::OWN_PARENT`] and the caller — which
    /// does know — fills the real one in.
    pub parent: Option<u8>,
    /// Initial `ai[0..4]` slots to stamp onto the new NPC, the way vanilla's `NewNPC` seeds a
    /// part's identity before its style ever runs. Each `Some(v)` pins that slot; each `None`
    /// leaves it at the spawn default of zero.
    ///
    /// The one exception is `ai[0]` on a parented spawn: left `None`, it still falls back to the
    /// side carried in the velocity's sign (`velocity.0.signum()`), which is how a boss part with
    /// no richer identity than "left or right" is raised. A part that needs more names the slot
    /// outright here rather than smuggling one number through the velocity: a Moon Lord hand's side
    /// in `ai[2]` (`NPC.cs:41648`), a saucer part's in `ai[1]` (`NPC.cs:36433`), a Wall Hungry's
    /// fractional band in `ai[0]` (`NPC.cs:26197`), a Pumpking blade's phase in `ai[3]`
    /// (`NPC.cs:33383`).
    pub ai: [Option<f32>; 4],
    /// Where the spawner wants a handle to this spawn written back, as (its own slot, `ai` index).
    ///
    /// `NewNPC` returns the slot it filled, and a routine that keeps hold of what it raised needs
    /// that number. A pal writes `ai[1 + i] = num2 + 1` for each of the two guards it raises
    /// (`NPC.cs:43401`) and unpacks them later to ask whether either is still alive
    /// (`AI_127_Pal_TryUnpackNPC`, `NPC.cs:43496-43508`) - which is *not* the same question as
    /// "is either still guarding me", because a guard that has been woken has cleared its own
    /// back-link and still holds the pal.
    ///
    /// [`Spawn::parent`] reports the link the other way round and cannot stand in for this: it also
    /// makes the spawn a *part* of its parent, which for a Goblin Archer with a life of its own it
    /// is not. So the caller, which knows the slot, writes it here instead. The value stored is
    /// vanilla's own `slot + 1`, so an untouched zero still means "nobody".
    pub handle: Option<(u8, usize)>,
}

/// Everything about the world a tick of AI reads that is not the NPC or the tiles.
///
/// Bundled because the list only grows: each boss wants one more thing the routine cannot see for
/// itself, and threading them as positional arguments stopped being readable at about six.
#[derive(Debug, Clone, Copy, Default)]
pub struct Surroundings<'a> {
    pub conditions: super::ai::Conditions,
    /// Anything nearby that a timid critter would rather not be next to.
    pub hazards: &'a [Hazard],
    /// Whatever the crowded styles keep away from; see [`avoidance`]. Each entry is a centre and
    /// the reach at which a style with no separation distance of its own notices it. See
    /// [`super::ai::World::avoid`].
    pub avoid: &'a [(f32, f32, f32)],
    /// The nearest hostile NPC a town resident might fight — `slot` is an NPC table index, built
    /// by the caller from the NPC table rather than from `targets` (which is players only).
    pub hostile: Option<Target>,
    /// Whether a nebula headcrab is already latched onto the target.
    pub target_taken: bool,
    /// Where Plantera's hooks have bitten, averaged.
    pub hooks: Option<(f32, f32)>,
    /// ...and each of them on its own, in slot order, which is what an expert Plantera's
    /// hook-borne tentacles orbit rather than the body.
    pub hook_anchors: &'a [(f32, f32)],
    /// How many of Plantera's tentacles are the body's own rather than a hook's, which is the
    /// count vanilla's regrow roll is against.
    pub body_tentacles: usize,
    /// Whether another hook is still travelling.
    pub kin_moving: bool,
    /// How many of the Moon Lord's sockets are broken open.
    pub sockets_open: usize,
    /// What the Old One's Army looks like right now.
    pub army: super::ai::ArmyView,
    /// The best thing a fairy could lead someone to, when one is asking.
    pub treasure: Option<(i32, i32)>,
    /// What a Dark Mage can see around it.
    pub mage: super::ai::army::mage::MageView,
    /// How many of each NPC type are alive, for the routines that wait on their escort or their
    /// armour.
    pub census: &'a [(u16, usize)],
    /// How many of this NPC's own escorts are alive. Only a pal asks; see
    /// [`super::ai::World::own_escorts`].
    pub own_escorts: usize,
    /// For a boss part, where its parent is and how big it is.
    pub parent: Option<super::ai::boss::skeletron::Parent>,
    /// ...and which state that parent is in.
    pub parent_state: f32,
    /// ...and what fraction of its health it has left.
    pub parent_health: f32,
    /// This NPC's own table slot — `NPC.whoAmI`. A routine cannot see the table it lives in, so
    /// the caller, which is iterating it, fills this in. Only the Wall's Hungry reads it.
    pub slot: u8,
}

impl Spawn {
    /// Stand-in for "the NPC that asked for this", which the caller replaces with a real slot.
    pub const OWN_PARENT: u8 = u8::MAX;
}

/// A player an NPC might chase.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    pub slot: u8,
    pub center: (f32, f32),
    /// How fast they are moving, which the routines that lead their target need.
    pub velocity: (f32, f32),
    pub alive: bool,
}

/// How far an enemy will look for someone to chase, in pixels.
pub const AGGRO_RANGE: f32 = 1000.0;

/// Half the box, in pixels, within which a player keeps an enemy from expiring.
///
/// The game's `CheckActive` refreshes `timeLeft` for any player whose hitbox intersects a rectangle
/// of `sWidth + width * 2` by `sHeight + height * 2` centred on the NPC — a screen's worth either
/// side, plus the creature's own size. A screen is 1920 by 1200, so the half-extents are 960 and
/// 600.
///
/// A rectangle rather than the 2000-pixel radius this used to be. The radius was more than three
/// times too generous vertically, which kept creatures alive far above and below anyone who could
/// possibly see them — and every one of those holds a slot and its share of the sync budget.
pub const DESPAWN_HALF_WIDTH: f32 = 960.0;
pub const DESPAWN_HALF_HEIGHT: f32 = 600.0;

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    (dx * dx + dy * dy).sqrt()
}

/// Nearest living player within `range`.
pub fn pick_target(npc: &Npc, targets: &[Target], range: f32) -> Option<Target> {
    targets
        .iter()
        .filter(|t| t.alive)
        .map(|t| (*t, distance(npc.center(), t.center)))
        .filter(|(_, d)| *d <= range)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, _)| t)
}

/// What a tick of AI wants the world to do beyond moving the NPC itself.
#[derive(Debug, Default)]
pub struct AiOutput {
    /// Minions a boss wants summoned.
    pub spawn: Vec<Spawn>,
    /// Doors a fighter has finished working at.
    pub doors: Vec<super::ai::fighter::Action>,
    /// Where the NPC just updated wants its passenger to hang, if it is carrying one.
    pub carry: Option<(f32, f32)>,
    /// What the NPC just updated turned into, if it turned into anything.
    ///
    /// A lost girl dropping her disguise and a truffle worm bolting underground both do this: the
    /// entity survives, its type does not. Written per NPC, so the caller reads it straight after
    /// the call that set it.
    pub transform: Option<u16>,
    /// How long the thing an NPC turned into should sit still, if it asked for a rest.
    pub rest_for: i32,
    /// Set when the NPC just updated went off rather than merely dying.
    pub detonated: bool,
    /// Life the NPC just updated carried home, on the tick it arrived.
    pub healed: i32,
    /// Gates the Eternia Crystal wants raised, as (tile x, tile y, left gate).
    pub gates: Vec<super::ai::army::crystal::Gate>,
    /// Set when a lane portal wants an enemy let out, and from which side.
    pub release: Option<bool>,
    /// Set on the tick the crystal's drama finishes, carrying whether the event was won.
    pub army_ended: Option<bool>,
    /// Set when the crystal wants its gates told to shut.
    pub close_gates: bool,
    /// Set on the tick a Dark Mage finishes a summoning.
    pub raising: bool,
    /// Set on the tick Betsy screams.
    pub screamed: bool,
    /// Set when a roar should leave everyone nearby slowed.
    pub roared: bool,
    /// How far a draining aura reaches, while one is out.
    pub aura: Option<f32>,
    /// Where the NPC just updated wants to be put, once it has finished going.
    pub teleport_to: Option<(f32, f32)>,
    /// Set on the tick the Cultists' tablet finishes breaking.
    pub ritual_complete: bool,
    /// Set on the tick the Moon Lord's death drama clears the stage.
    pub cleared_stage: bool,
    /// Minions of the NPC just updated that it wants destroyed, as (type, how many at most).
    pub cull_kin: Option<(u16, usize)>,
    /// Set when the NPC just updated wants whatever it hangs off punished for its destruction.
    pub punish_owner: bool,
    /// Set when what it just did calls in an invasion.
    pub called_invasion: bool,
    /// Doors a town NPC wants opened or shut.
    pub town_doors: Vec<super::ai::town::DoorAction>,
    /// Projectiles a routine decided to throw. `server.rs` turns each into a real entity via
    /// `self.projectiles.launch(..)` and broadcasts it.
    pub shots: Vec<super::ai::Shot>,
    /// A town NPC's melee attack landing on a nearby hostile.
    pub melee_hits: Vec<super::ai::MeleeHit>,
    /// A buff the NPC just updated wants put straight onto a player, as (player slot, buff id,
    /// ticks) — see [`super::ai::Effects::player_buff`].
    pub player_buff: Option<(u8, u16, i32)>,
    /// The Moon Lord's leech step — see [`super::ai::Effects::brands_players`] and
    /// [`super::ai::Effects::harvests_brands`].
    pub brands_players: Option<(f32, f32)>,
    pub harvests_brands: Option<(f32, f32)>,
    /// An item to put into the world where this NPC stands, outside the kill path: see
    /// [`super::ai::Effects::reward`].
    pub reward: Option<i16>,
}

/// Move a worm segment to trail the one in front of it.
///
/// Kept as the name the server calls; the behaviour is [`super::ai::worm::follow`].
pub fn follow_leader(npc: &mut Npc, leader_center: (f32, f32)) {
    super::ai::worm::follow(npc, leader_center);
}

/// Drive one NPC for a tick: choose a target, run its style, then move it.
pub fn update(
    npc: &mut Npc,
    tiles: &impl TileView,
    targets: &[Target],
    rng: &mut SmallRng,
    out: &mut AiOutput,
) {
    update_with(npc, tiles, targets, rng, out, Surroundings::default())
}

/// As [`update`], but with the world conditions some routines read.
pub fn update_with(
    npc: &mut Npc,
    tiles: &impl TileView,
    targets: &[Target],
    rng: &mut SmallRng,
    out: &mut AiOutput,
    around: Surroundings<'_>,
) {
    out.transform = None;
    out.carry = None;
    let before = (npc.position, npc.velocity, npc.direction, npc.target);

    // `NPC.TargetClosest` has no range limit: enemies do not lose interest when you outrun them,
    // and the despawn timer is what removes them instead. Ported routines get those semantics.
    // The approximations keep the range they were tuned against, except for bosses, whose whole
    // design assumes an unbounded target — King Slime's catch-up teleport exists for that case.
    let target = if super::ai::is_ported(npc.stats.ai_style) {
        super::ai::target_closest(npc, targets)
    } else {
        let range = if npc.stats.boss {
            f32::INFINITY
        } else {
            AGGRO_RANGE
        };
        pick_target(npc, targets, range)
    };
    npc.target = target.map_or(255, |t| u16::from(t.slot));

    if super::ai::is_ported(npc.stats.ai_style) {
        // The same read the physics does for its own gravity pair, so a routine and the step under
        // it never disagree about standing in water.
        let liquid_at = |p: (f32, f32)| super::npc::liquid_at(tiles, p).is_some();
        let world = super::ai::World {
            tiles,
            target,
            wet: liquid_at(npc.center()),
            target_wet: target.is_some_and(|t| liquid_at(t.center)),
            conditions: around.conditions,
            was_hurt: npc.was_hurt,
            // Nothing here can see the rest of the NPC table, so the caller supplies this — but
            // only for the two styles that read it. Working it out for all two hundred NPCs would
            // be a quadratic scan every tick to feed a butterfly.
            crowding: if reads_crowding(npc.stats.ai_style) {
                // The two styles that read this each name their own reach; they agree on a hundred
                // pixels, and the constants are the source rather than a literal here.
                let reach = if npc.stats.ai_style == 26 {
                    terrustia_proto::npc_params::BUTTERFLY_FEAR_RANGE
                } else {
                    terrustia_proto::npc_params::DRAGONFLY_FEAR_NPC
                };
                crowding_at(npc, around.hazards, reach)
            } else {
                (0.0, 0.0)
            },
            // Same story as crowding: only the styles that jostle for space pay for it.
            avoid: if avoidance(npc.stats.ai_style).is_some() {
                around.avoid
            } else {
                &[]
            },
            target_taken: around.target_taken,
            hostile: around.hostile,
            hooks: around.hooks,
            hook_anchors: around.hook_anchors,
            body_tentacles: around.body_tentacles,
            kin_moving: around.kin_moving,
            sockets_open: around.sockets_open,
            army: around.army,
            treasure: around.treasure,
            mage: around.mage,
            target_velocity: target.map_or((0.0, 0.0), |t| t.velocity),
            census: around.census,
            own_escorts: around.own_escorts,
            parent: around.parent,
            parent_state: around.parent_state,
            parent_health: around.parent_health,
            slot: around.slot,
        };
        let effects = super::ai::run(npc, &world, rng);
        out.spawn.extend(effects.spawn);
        out.doors.extend(effects.doors);
        out.town_doors.extend(effects.town_doors);
        out.shots.extend(effects.shots);
        out.melee_hits.extend(effects.melee_hits);
        if effects.died {
            npc.life = 0;
        }
        out.transform = effects.transform;
        out.rest_for = effects.rest_for;
        out.detonated = effects.detonated;
        out.healed = effects.healed;
        out.gates = effects.gates;
        out.release = effects.release;
        out.army_ended = effects.army_ended;
        out.close_gates = effects.close_gates;
        out.raising = effects.raising;
        out.screamed = effects.screamed;
        out.aura = effects.aura;
        out.teleport_to = effects.teleport_to;
        out.ritual_complete = effects.ritual_complete;
        out.cleared_stage = effects.cleared_stage;
        out.cull_kin = effects.cull_kin;
        out.punish_owner = effects.punish_owner;
        out.called_invasion = effects.called_invasion;
        out.carry = effects.carry;
        out.roared = effects.roared;
        out.player_buff = effects.player_buff;
        out.brands_players = effects.brands_players;
        out.harvests_brands = effects.harvests_brands;
        out.reward = effects.reward;
        npc.was_hurt = false;

        step_physics(npc, tiles);
        tick_life(npc, targets);
        // Applied after `tick_life`'s own despawn-radius refresh, not before: a routine that
        // decided this is its last tick has to win over "but a player is standing right here",
        // or the refresh clobbers the same-tick zero before anything downstream ever sees it. A
        // one-shot payout whose own exit condition guarantees a player is in range (the Palworld
        // pet, `NPC.cs:43461-43467`) never got removed at all under the old order: `time_left`
        // was set to 0 and immediately refreshed back to `DEFAULT_TIME_LEFT` a few lines later in
        // the same tick, so the routine kept re-entering its payout arm and re-queuing the reward
        // item every tick until the collecting player fell out of packet-buffer range by chance.
        if effects.expired {
            npc.time_left = 0;
        }
        if worth_telling_clients(npc, before) {
            npc.dirty = true;
        }
        return;
    }

    match npc.stats.ai_style {
        54 => brain_of_cthulhu(npc, target, rng, out),
        // 13 is the rooted plants; 19 is the antlion, which sits buried and faces its target;
        // 20 rolls along a fixed track. All three hold position and turn to face.
        // 13 rooted plants, 19 the buried antlion, 20 spike balls on a track, 21 blazing wheels,
        // 42 the Lost Girl waiting to become a Nymph, 119 dandelions: all hold position.
        // 17 is the vulture, which perches and then flies; grouped with the other wanderers.
        // The critters: vultures, butterflies, snails, dragonflies, ladybugs, balloons and the
        // rest wander rather than hunt.
        // Nothing in the build reaches here any more: every NPC type's style is ported, and a
        // test walks the whole roster to prove it. It is kept as the floor rather than a panic,
        // because a style added later should hold still rather than take the server down.
        _ => idle(npc),
    }

    step_physics(npc, tiles);
    tick_life(npc, targets);

    if worth_telling_clients(npc, before) {
        npc.dirty = true;
    }
}

/// Whether this tick changed anything a client could not have worked out for itself.
///
/// The distinction is the whole of the game's network budget. A client runs the same routines and
/// extrapolates between updates, so an NPC walking in a straight line at a steady speed needs no
/// packets at all — its position a moment from now follows from the last one it was told. What a
/// client *cannot* guess is a decision: turning round, picking a new target, being shoved, landing.
///
/// Marking every tick that moved an NPC by any amount — which this used to do — makes every walking
/// creature a continuous stream, and measured at 1.4 syncs a second each against the real server's
/// 0.7. The rate limiter in `server.rs` bounded that but could not make it unnecessary.
fn worth_telling_clients(npc: &Npc, before: ((f32, f32), (f32, f32), i8, u16)) -> bool {
    let (_, old_velocity, old_direction, old_target) = before;

    // A decision, in every case.
    if npc.direction != old_direction || npc.target != old_target {
        return true;
    }

    // A change of speed the client cannot extrapolate through. Steady walking and the smooth part
    // of a fall both stay under this; a bounce, a landing, a knock and a turn do not.
    //
    // Gravity alone changes downward speed by 0.3 a tick, so the threshold sits above that: a body
    // in free fall follows a curve the client is already drawing, and telling it sixty times a
    // second says nothing it did not know.
    const MEANINGFUL: f32 = 0.5;
    (npc.velocity.0 - old_velocity.0).abs() > MEANINGFUL
        || (npc.velocity.1 - old_velocity.1).abs() > MEANINGFUL
}

/// Somewhere nearby that a timid critter would rather not be.
#[derive(Debug, Clone, Copy)]
pub struct Hazard {
    pub center: (f32, f32),
    pub half: (f32, f32),
}

/// Whether a style flinches from things near it, and so needs the hazard scan run for it.
///
/// Only the butterflies and the tumbleweeds do. Everything else gets a zero and never notices.
fn reads_crowding(style: i32) -> bool {
    matches!(style, 26 | 65)
}

/// What a style keeps its distance from, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avoids {
    /// Others of the same type — a pack of pirate ghosts fans out instead of stacking.
    OwnKind,
    /// Anything alive: a shimmerfly bolts from enemies and players alike.
    AnythingAlive,
}

/// Which styles jostle for space, and what they jostle with.
///
/// Returning `None` is what keeps this off the hot path: the list behind it is a scan of the whole
/// NPC table, so it is only built when something present actually reads it.
pub fn avoidance(style: i32) -> Option<Avoids> {
    match style {
        // 44 is here for the two lunar swarmers, the Stardust Spider Flying and the Flying Antlion
        // (`NPC.cs:31191-31206`); the other two style-44 types read the list and ignore it.
        44 | 85 | 86 | 90 | 108 | 111 | 122 => Some(Avoids::OwnKind),
        64 => Some(Avoids::AnythingAlive),
        _ => None,
    }
}

/// The average direction away from every hazard close enough to matter.
///
/// A butterfly bolts from anything dangerous within a hundred pixels; a dragonfly counts players
/// too. Returns a zero vector when there is nothing to avoid.
fn crowding_at(npc: &Npc, hazards: &[Hazard], reach: f32) -> (f32, f32) {
    let (cx, cy) = npc.center();
    let mut sum = (0.0, 0.0);
    let mut n = 0.0;
    for hazard in hazards {
        // Distance from the box, not from its middle, which is what the game measures.
        let dx = (cx - hazard.center.0).abs() - hazard.half.0;
        let dy = (cy - hazard.center.1).abs() - hazard.half.1;
        if dx.max(0.0).hypot(dy.max(0.0)) > reach {
            continue;
        }
        let away = (cx - hazard.center.0, cy - hazard.center.1);
        let length = (away.0 * away.0 + away.1 * away.1).sqrt();
        if length == 0.0 {
            continue;
        }
        sum.0 += away.0 / length;
        sum.1 += away.1 / length;
        n += 1.0;
    }
    if n == 0.0 {
        return (0.0, 0.0);
    }
    (sum.0 / n, sum.1 / n)
}

/// Count down toward despawn when nobody is close enough to care.
///
/// `NPC.CheckActive` (`NPC.cs:78697`) is the gate: `if (!active || ... || townNPC) return;`. The
/// Skeleton Merchant is the one creature where reading that as "`town_npc` in our table" is wrong.
/// `SetDefaults`' own arm for 453 (`NPC.cs:14427-14441`) sets `friendly` and never `townNPC`, which
/// is precisely why the game needs a separate `NPC.isLikeATownNPC` property (`NPC.cs:6880-6890`,
/// literally `if (type == 453) return true; return townNPC;`) for the places that *do* want to
/// count him as one. `npc_data.rs`'s 453 entry carries `town_npc: true` deliberately, and its own
/// comment says the faithful fix belongs at the call sites rather than in the table: this is the
/// one where the difference is observable. He is a passing encounter, not a resident, and vanilla
/// takes him away 750 ticks after the last player walks off. Without this he would wander the
/// caverns for the life of the world.
fn tick_life(npc: &mut Npc, targets: &[Target]) {
    if (npc.stats.town_npc && npc.npc_type != crate::game::spawn::SKELETON_MERCHANT)
        || npc.stats.boss
    {
        return;
    }
    // The game's box, not a radius: a screen either side of the creature, widened by its own size.
    let (half_w, half_h) = (
        DESPAWN_HALF_WIDTH + npc.width(),
        DESPAWN_HALF_HEIGHT + npc.height(),
    );
    let centre = npc.center();
    let near = targets.iter().any(|t| {
        t.alive
            && (t.center.0 - centre.0).abs() <= half_w
            && (t.center.1 - centre.1).abs() <= half_h
    });
    if near {
        npc.time_left = super::npc::DEFAULT_TIME_LEFT;
    } else {
        npc.time_left -= 1;
    }
}

fn idle(npc: &mut Npc) {
    npc.velocity.0 *= 0.9;
    if npc.velocity.0.abs() < 0.05 {
        npc.velocity.0 = 0.0;
    }
}

/// Steer toward a point, accelerating up to `speed`.
fn approach(npc: &mut Npc, aim: (f32, f32), speed: f32, accel: f32) {
    let (cx, cy) = npc.center();
    let (dx, dy) = (aim.0 - cx, aim.1 - cy);
    let length = (dx * dx + dy * dy).sqrt().max(0.001);
    let (wanted_x, wanted_y) = (dx / length * speed, dy / length * speed);

    npc.velocity.0 += (wanted_x - npc.velocity.0) * accel.min(1.0) * 4.0;
    npc.velocity.1 += (wanted_y - npc.velocity.1) * accel.min(1.0) * 4.0;
    npc.velocity.0 = npc.velocity.0.clamp(-speed * 1.5, speed * 1.5);
    npc.velocity.1 = npc.velocity.1.clamp(-speed * 1.5, speed * 1.5);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc::Npc;
    use rand::SeedableRng;
    use terrustia_proto::Tile;

    pub struct Terrain<F>(F);

    impl<F: Fn(i32, i32) -> Option<u16>> TileView for Terrain<F> {
        fn tile(&self, x: i32, y: i32) -> Tile {
            match (self.0)(x, y) {
                Some(block) if terrustia_proto::tile_sets::frame_important(block) => {
                    Tile::framed(block, 0, 0)
                }
                Some(block) => Tile::block(block),
                None => Tile::AIR,
            }
        }
    }

    /// Flat ground at row 10 and below.
    fn flat() -> Terrain<impl Fn(i32, i32) -> Option<u16>> {
        Terrain(|_x: i32, y: i32| if y >= 10 { Some(1) } else { None })
    }

    /// Ground far enough down for a boss fight to happen above it.
    pub fn flat_ground() -> Terrain<impl Fn(i32, i32) -> Option<u16>> {
        Terrain(|_x: i32, y: i32| if y >= 10 { Some(1) } else { None })
    }

    /// Open sky, for the flying bosses.
    pub fn empty_terrain() -> Terrain<impl Fn(i32, i32) -> Option<u16>> {
        Terrain(|_x: i32, _y: i32| None)
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(42)
    }

    fn player_at(x: f32, y: f32) -> Target {
        Target {
            slot: 0,
            center: (x, y),
            velocity: (0.0, 0.0),
            alive: true,
        }
    }

    /// Place an NPC standing on top of `ground_row`, so it starts clear of the terrain.
    ///
    /// Positioning by hand is easy to get wrong: an NPC's position is its top-left corner, so a
    /// 40-pixel-tall zombie placed at the ground line starts embedded in it and cannot move.
    fn stand_on(npc_type: u16, tile_x: f32, ground_row: f32) -> Npc {
        let mut npc = Npc::new(npc_type, (0.0, 0.0), 1).expect("known npc type");
        npc.position = (tile_x * 16.0, ground_row * 16.0 - npc.height());
        npc
    }

    /// Drop an NPC onto the ground so tests start from a settled state.
    fn settle(npc: &mut Npc, tiles: &impl TileView) {
        for _ in 0..120 {
            super::super::npc::step_physics(npc, tiles);
            if npc.on_ground {
                break;
            }
        }
    }

    #[test]
    fn a_zombie_walks_toward_the_player() {
        let terrain = flat();
        let mut zombie = stand_on(3, 100.0, 10.0);
        settle(&mut zombie, &terrain);
        let start = zombie.position.0;

        let targets = [player_at(120.0 * 16.0, 150.0)];
        let mut r = rng();
        for _ in 0..120 {
            update(
                &mut zombie,
                &terrain,
                &targets,
                &mut r,
                &mut AiOutput::default(),
            );
        }

        assert!(zombie.position.0 > start, "should have moved east");
        assert_eq!(zombie.direction, 1, "should face the player");
        assert_eq!(zombie.target, 0, "should have acquired the target");
    }

    #[test]
    fn a_zombie_turns_around_for_a_player_behind_it() {
        let terrain = flat();
        let mut zombie = stand_on(3, 100.0, 10.0);
        settle(&mut zombie, &terrain);

        let targets = [player_at(80.0 * 16.0, 150.0)];
        let mut r = rng();
        for _ in 0..60 {
            update(
                &mut zombie,
                &terrain,
                &targets,
                &mut r,
                &mut AiOutput::default(),
            );
        }
        assert_eq!(zombie.direction, -1, "should face west");
        assert!(zombie.velocity.0 < 0.0, "should be moving west");
    }

    #[test]
    fn a_zombie_hops_over_a_one_tile_step() {
        // Flat ground with a single block step at x = 105.
        let terrain = Terrain(|x: i32, y: i32| {
            if y >= 10 || (x == 105 && y == 9) {
                Some(1)
            } else {
                None
            }
        });
        let mut zombie = stand_on(3, 100.0, 10.0);
        settle(&mut zombie, &terrain);

        let targets = [player_at(120.0 * 16.0, 140.0)];
        let mut r = rng();
        let mut jumped = false;
        for _ in 0..400 {
            update(
                &mut zombie,
                &terrain,
                &targets,
                &mut r,
                &mut AiOutput::default(),
            );
            if zombie.velocity.1 < -1.0 {
                jumped = true;
            }
        }
        assert!(jumped, "never attempted to hop the step");
        assert!(
            zombie.position.0 > 105.0 * 16.0,
            "did not get past the step: x={}",
            zombie.position.0 / 16.0
        );
    }

    #[test]
    fn a_wandering_zombie_does_not_walk_off_a_cliff() {
        // Ground only up to x = 105; open air beyond.
        let terrain = Terrain(|x: i32, y: i32| if y >= 10 && x <= 105 { Some(1) } else { None });
        let mut zombie = stand_on(3, 100.0, 10.0);
        settle(&mut zombie, &terrain);
        zombie.direction = 1;

        let mut r = rng();
        for _ in 0..600 {
            update(&mut zombie, &terrain, &[], &mut r, &mut AiOutput::default());
        }
        assert!(
            zombie.position.1 < 200.0,
            "wandered off the edge and fell: y={}",
            zombie.position.1
        );
    }

    #[test]
    fn a_slime_hops_rather_than_walking() {
        let terrain = flat();
        let mut slime = stand_on(1, 100.0, 10.0);
        settle(&mut slime, &terrain);

        let targets = [player_at(120.0 * 16.0, 150.0)];
        let mut r = rng();
        let mut airborne_ticks = 0;
        let start = slime.position.0;
        for _ in 0..300 {
            update(
                &mut slime,
                &terrain,
                &targets,
                &mut r,
                &mut AiOutput::default(),
            );
            if !slime.on_ground {
                airborne_ticks += 1;
            }
        }
        assert!(airborne_ticks > 20, "a slime should spend time mid-hop");
        assert!(slime.position.0 > start, "should have closed on the player");
    }

    #[test]
    fn a_flyer_closes_on_the_player_through_the_air() {
        // Eater of Souls: noGravity.
        let terrain = flat();
        let mut flyer = Npc::new(6, (100.0 * 16.0, 50.0), 1).unwrap();
        let targets = [player_at(130.0 * 16.0, 60.0)];
        let start = distance(flyer.center(), targets[0].center);

        let mut r = rng();
        // An eater of souls takes a while to get going — for its first hundred ticks the jitter
        // outweighs the acceleration and it barely moves — and then, because it does not turn
        // hard, it sails past and comes round again. So the question is whether it ever reaches
        // the player, not where it happens to be after a fixed number of ticks.
        let mut closest = start;
        for _ in 0..500 {
            update(
                &mut flyer,
                &terrain,
                &targets,
                &mut r,
                &mut AiOutput::default(),
            );
            closest = closest.min(distance(flyer.center(), targets[0].center));
        }
        assert!(
            closest < start * 0.25,
            "flyer never reached the player: {start} -> {closest}"
        );
    }

    #[test]
    fn a_harpy_shot_reaches_the_ai_output() {
        let terrain = flat();
        let mut harpy = Npc::new(48, (1000.0, 60.0), 1).unwrap();
        let targets = [player_at(1200.0, 60.0)];
        let mut out = AiOutput::default();
        let mut r = rng();
        for _ in 0..100 {
            // Keep it convinced it can see someone, so it never drops into its drift.
            harpy.ai[1] = 0.0;
            update(&mut harpy, &terrain, &targets, &mut r, &mut out);
        }
        assert_eq!(out.shots.len(), 3, "three feathers in the first volley");
        assert!(out.shots.iter().all(|s| s.projectile == 38));
    }

    #[test]
    fn a_bat_weaves_toward_its_target_rather_than_flying_at_it() {
        let terrain = flat();
        let mut bat = Npc::new(49, (100.0 * 16.0, 60.0), 1).unwrap();
        let targets = [player_at(140.0 * 16.0, 60.0)];
        let start = distance(bat.center(), targets[0].center);

        let mut r = rng();
        let mut deepest: f32 = bat.position.1;
        let mut weaves = 0;
        let mut climbing = bat.velocity.1 < 0.0;
        for _ in 0..400 {
            update(
                &mut bat,
                &terrain,
                &targets,
                &mut r,
                &mut AiOutput::default(),
            );
            deepest = deepest.max(bat.position.1);
            if (bat.velocity.1 < 0.0) != climbing {
                climbing = !climbing;
                weaves += 1;
            }
        }
        assert!(
            distance(bat.center(), targets[0].center) < start,
            "should have closed the distance"
        );
        // The vertical brake term makes a bat overshoot by tens of pixels each way; that long
        // sine-wave weave, rather than a straight line, is what makes it read as a bat.
        assert!(
            weaves >= 4,
            "should weave up and down, got {weaves} reversals"
        );
        assert!(
            deepest < 10.0 * 16.0,
            "and never sink into the floor, got {deepest}"
        );
    }

    #[test]
    fn an_npc_with_nobody_around_counts_down_to_despawn() {
        let terrain = flat();
        let mut zombie = stand_on(3, 100.0, 10.0);
        settle(&mut zombie, &terrain);
        let start = zombie.time_left;

        let mut r = rng();
        for _ in 0..100 {
            update(&mut zombie, &terrain, &[], &mut r, &mut AiOutput::default());
        }
        assert!(zombie.time_left < start, "should be expiring");

        // A player arriving resets the clock.
        let targets = [player_at(100.0 * 16.0, 150.0)];
        update(
            &mut zombie,
            &terrain,
            &targets,
            &mut r,
            &mut AiOutput::default(),
        );
        assert_eq!(zombie.time_left, super::super::npc::DEFAULT_TIME_LEFT);
    }

    #[test]
    fn a_town_npc_never_expires() {
        let terrain = flat();
        let mut guide = stand_on(22, 100.0, 10.0);
        settle(&mut guide, &terrain);
        let mut r = rng();
        for _ in 0..300 {
            update(&mut guide, &terrain, &[], &mut r, &mut AiOutput::default());
        }
        assert_eq!(guide.time_left, super::super::npc::DEFAULT_TIME_LEFT);
    }

    /// ...but the Skeleton Merchant does, because he is not one. `SetDefaults` never sets
    /// `townNPC` on 453 (`NPC.cs:14427-14441`), so `NPC.CheckActive`'s `townNPC` early-return
    /// (`NPC.cs:78697`) does not cover him and he goes on the ordinary 750-tick inactivity clock.
    /// He is a passing encounter in the game, not a resident.
    ///
    /// Fails before the fix, when `npc_data.rs`'s deliberate `town_npc: true` for him was read
    /// here as vanilla's `townNPC`: a Skeleton Merchant would have wandered forever.
    #[test]
    fn a_skeleton_merchant_is_not_a_town_npc_and_does_expire() {
        let terrain = flat();
        let mut merchant = stand_on(crate::game::spawn::SKELETON_MERCHANT, 100.0, 10.0);
        settle(&mut merchant, &terrain);
        let start = merchant.time_left;
        let mut r = rng();
        for _ in 0..300 {
            update(
                &mut merchant,
                &terrain,
                &[],
                &mut r,
                &mut AiOutput::default(),
            );
        }
        assert!(
            merchant.time_left < start,
            "he wandered off nobody's clock at all"
        );

        // And a player standing next to him keeps him, exactly like any other wanderer.
        let targets = [player_at(100.0 * 16.0, 150.0)];
        update(
            &mut merchant,
            &terrain,
            &targets,
            &mut r,
            &mut AiOutput::default(),
        );
        assert_eq!(merchant.time_left, super::super::npc::DEFAULT_TIME_LEFT);
    }

    #[test]
    fn a_town_npc_stays_on_its_ledge() {
        let terrain = Terrain(|x: i32, y: i32| {
            if y >= 10 && (100..=110).contains(&x) {
                Some(1)
            } else {
                None
            }
        });
        let mut guide = stand_on(22, 105.0, 10.0);
        settle(&mut guide, &terrain);

        let mut r = rng();
        for _ in 0..2000 {
            update(&mut guide, &terrain, &[], &mut r, &mut AiOutput::default());
        }
        assert!(guide.position.1 < 200.0, "the guide fell off the ledge");
    }

    #[test]
    fn a_dead_player_is_not_chased() {
        let terrain = flat();
        let mut zombie = stand_on(3, 100.0, 10.0);
        settle(&mut zombie, &terrain);

        let dead = [Target {
            slot: 0,
            center: (110.0 * 16.0, 150.0),
            velocity: (0.0, 0.0),
            alive: false,
        }];
        let mut r = rng();
        update(
            &mut zombie,
            &terrain,
            &dead,
            &mut r,
            &mut AiOutput::default(),
        );
        assert_eq!(zombie.target, 255);
    }

    #[test]
    fn the_nearest_player_is_chosen() {
        let npc = stand_on(3, 100.0, 10.0);
        let targets = [
            player_at(140.0 * 16.0, 150.0),
            Target {
                slot: 3,
                center: (105.0 * 16.0, 150.0),
                velocity: (0.0, 0.0),
                alive: true,
            },
        ];
        assert_eq!(pick_target(&npc, &targets, AGGRO_RANGE).unwrap().slot, 3);
    }
}

// ---------------------------------------------------------------------------- bosses
//
// Each boss in the game has a bespoke multi-phase routine running to hundreds of lines. These
// reproduce the shape of each fight — the phases, the movement pattern, the minions — using the
// real stats and thresholds, without the per-frame detail.

/// Creeper, which orbits the Brain.
const CREEPER: u16 = 267;

/// Health fraction at which most pre-hardmode bosses change phase.
const PHASE_TWO: f32 = 0.5;

fn health_fraction(npc: &Npc) -> f32 {
    if npc.life_max <= 0 {
        return 0.0;
    }
    npc.life as f32 / npc.life_max as f32
}

/// Style 54 — Brain of Cthulhu.
///
/// Teleports around its target behind a screen of creepers; once they are gone it charges directly.
fn brain_of_cthulhu(npc: &mut Npc, target: Option<Target>, rng: &mut SmallRng, out: &mut AiOutput) {
    let Some(t) = target else {
        return;
    };
    let second_phase = health_fraction(npc) < PHASE_TWO;
    npc.ai[0] += 1.0;

    if !second_phase {
        // Keep a ring of creepers up early on.
        if npc.ai[0] % 240.0 == 0.0 {
            for _ in 0..3 {
                out.spawn.push(Spawn {
                    handle: None,
                    npc_type: CREEPER,
                    position: npc.center(),
                    velocity: (0.0, 0.0),
                    parent: None,
                    ai: [None; 4],
                });
            }
        }
        // Blink to a new spot around the target.
        if npc.ai[0] % 150.0 == 0.0 {
            let angle = rng.random_range(0.0..std::f32::consts::TAU);
            npc.position = (
                t.center.0 + angle.cos() * 260.0 - npc.width() / 2.0,
                t.center.1 + angle.sin() * 260.0 - npc.height() / 2.0,
            );
            npc.velocity = (0.0, 0.0);
            npc.dirty = true;
        } else {
            approach(npc, t.center, 1.5, 0.03);
        }
    } else {
        approach(npc, t.center, 8.0, 0.10);
    }

    npc.direction = if npc.velocity.0 > 0.0 { 1 } else { -1 };
    npc.sprite_direction = npc.direction;
}

#[cfg(test)]
mod boss_tests {
    use super::tests::*;
    use super::*;
    use crate::game::npc::{Npc, NpcStore};
    use rand::SeedableRng;

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(7)
    }

    fn player(x: f32, y: f32) -> Target {
        Target {
            slot: 0,
            center: (x, y),
            velocity: (0.0, 0.0),
            alive: true,
        }
    }

    #[test]
    fn the_eye_of_cthulhu_summons_servants_then_stops_in_phase_two() {
        let terrain = empty_terrain();
        let targets = [player(1600.0, 1600.0)];

        let mut eye = Npc::new(4, (1600.0, 1000.0), 1).unwrap();
        let mut out = AiOutput::default();
        let mut r = rng();
        for _ in 0..400 {
            update(&mut eye, &terrain, &targets, &mut r, &mut out);
        }
        assert!(
            out.spawn.iter().any(|s| s.npc_type == 5),
            "phase one should summon Servants of Cthulhu"
        );

        // Below half health it stops summoning and just charges.
        let mut wounded = Npc::new(4, (1600.0, 1000.0), 1).unwrap();
        wounded.life = wounded.life_max / 4;
        let mut out2 = AiOutput::default();
        for _ in 0..400 {
            update(&mut wounded, &terrain, &targets, &mut r, &mut out2);
        }
        assert!(
            out2.spawn.is_empty(),
            "phase two should not summon; it charges instead"
        );
    }

    #[test]
    fn the_eye_closes_on_its_target() {
        let terrain = empty_terrain();
        let targets = [player(2400.0, 1600.0)];
        let mut eye = Npc::new(4, (1000.0, 1000.0), 1).unwrap();
        let start = distance(eye.center(), targets[0].center);
        let mut out = AiOutput::default();
        let mut r = rng();
        for _ in 0..600 {
            update(&mut eye, &terrain, &targets, &mut r, &mut out);
        }
        assert!(
            distance(eye.center(), targets[0].center) < start * 0.5,
            "the eye should have closed the distance"
        );
    }

    #[test]
    fn king_slime_hops_and_sheds_slimes() {
        let terrain = flat_ground();
        let targets = [player(120.0 * 16.0, 150.0)];
        let mut king = Npc::new(50, (100.0 * 16.0, 0.0), 1).unwrap();
        // Drop it onto the ground first.
        for _ in 0..200 {
            crate::game::npc::step_physics(&mut king, &terrain);
            if king.on_ground {
                break;
            }
        }

        let mut out = AiOutput::default();
        let mut r = rng();
        let mut airborne = 0;
        for tick in 0..600 {
            update(&mut king, &terrain, &targets, &mut r, &mut out);
            if !king.on_ground {
                airborne += 1;
            }
            // It sheds on damage taken, not on a timer, so the fight has to actually happen.
            if tick % 60 == 0 {
                king.life -= king.life_max / 10;
            }
        }
        assert!(airborne > 50, "King Slime should be hopping");
        assert!(
            out.spawn.iter().any(|s| s.npc_type == 1),
            "it should shed Blue Slimes"
        );
    }

    #[test]
    /// Past three thousand pixels it does not chase, it leaves. The catch-up teleport is for
    /// someone hiding behind terrain, not someone who has run to the other end of the world.
    fn king_slime_gives_up_on_a_player_who_runs_right_away() {
        let terrain = flat_ground();
        let targets = [player(100_000.0, 150.0)];
        let mut king = Npc::new(50, (100.0 * 16.0, 100.0), 1).unwrap();

        let mut out = AiOutput::default();
        let mut r = rng();
        update(&mut king, &terrain, &targets, &mut r, &mut out);
        assert!(
            king.time_left <= 10,
            "it should be leaving, got {}",
            king.time_left
        );
    }

    #[test]
    /// Bees are one of three attacks she picks between, so the fight has to run long enough for
    /// her to choose it. She calls either kind.
    fn the_queen_bee_releases_bees_while_hovering() {
        let terrain = empty_terrain();
        let targets = [player(1600.0, 1600.0)];
        let mut queen = Npc::new(222, (1600.0, 1400.0), 1).unwrap();
        let mut out = AiOutput::default();
        let mut r = rng();
        for _ in 0..4000 {
            update(&mut queen, &terrain, &targets, &mut r, &mut out);
            if out
                .spawn
                .iter()
                .any(|s| s.npc_type == 210 || s.npc_type == 211)
            {
                return;
            }
        }
        panic!("she should have called bees at some point in a minute of fighting");
    }

    #[test]
    /// The Brain's phases turn on its creepers, not on its health: it puts twenty up on its first
    /// tick, and stays untouchable until the last of them is dead.
    fn the_brain_surrounds_itself_with_creepers_then_charges() {
        let terrain = empty_terrain();
        let targets = [player(1600.0, 1600.0)];

        let mut brain = Npc::new(266, (1600.0, 1200.0), 1).unwrap();
        let mut out = AiOutput::default();
        let mut r = rng();
        // Creepers reported alive, so it stays in its shielded phase.
        update_with(
            &mut brain,
            &terrain,
            &targets,
            &mut r,
            &mut out,
            Surroundings {
                conditions: crate::game::ai::Conditions {
                    crimson: true,
                    ..Default::default()
                },
                census: &[(terrustia_proto::npc_params::CREEPER, 20)],
                ..Default::default()
            },
        );
        assert_eq!(
            out.spawn.iter().filter(|s| s.npc_type == 267).count(),
            terrustia_proto::npc_params::BRAIN_CREEPERS,
            "it should put its whole guard up at once"
        );
        assert!(brain.invulnerable, "and hide behind them");

        // With them gone it exposes itself and charges.
        let start = distance(brain.center(), targets[0].center);
        for _ in 0..300 {
            update_with(
                &mut brain,
                &terrain,
                &targets,
                &mut r,
                &mut out,
                Surroundings {
                    conditions: crate::game::ai::Conditions {
                        crimson: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
        }
        assert!(!brain.invulnerable, "now it can be hurt");
        assert!(
            distance(brain.center(), targets[0].center) < start,
            "and it comes at you"
        );
    }

    /// A ported routine gets `TargetClosest` semantics, which have no range at all.
    ///
    /// This is not a detail: a zombie in the game keeps walking toward you from off-screen, and
    /// what eventually removes it is the despawn timer rather than a change of heart. Capping the
    /// range would make enemies visibly give up mid-chase.
    #[test]
    fn a_ported_enemy_never_loses_interest_however_far_away_you_get() {
        let terrain = empty_terrain();
        let far = [player(1000.0 + AGGRO_RANGE * 20.0, 1000.0)];
        let mut zombie = Npc::new(3, (1000.0, 1000.0), 1).unwrap();
        let mut r = rng();
        update(
            &mut zombie,
            &terrain,
            &far,
            &mut r,
            &mut AiOutput::default(),
        );
        assert_eq!(zombie.target, 0);
        assert_eq!(zombie.direction, 1, "and keeps heading that way");
    }

    #[test]
    fn a_worm_is_spawned_as_a_linked_chain() {
        let mut store = NpcStore::new();
        let head = store
            .spawn_worm(13, 14, 15, 5, (1000.0, 1000.0))
            .expect("worm should spawn");

        assert_eq!(store.len(), 6, "a head plus five segments");
        assert_eq!(store.get(head).unwrap().npc_type, 13);
        assert!(store.get(head).unwrap().follows.is_none(), "the head leads");

        // Every other link follows exactly one other, and the last is a tail.
        let followers: Vec<_> = store
            .iter()
            .filter(|(_, n)| n.follows.is_some())
            .map(|(i, n)| (i, n.npc_type, n.follows.unwrap()))
            .collect();
        assert_eq!(followers.len(), 5);
        assert_eq!(
            followers.last().unwrap().1,
            15,
            "the final segment should be the tail"
        );
    }

    #[test]
    fn a_segment_trails_its_leader_at_a_fixed_distance() {
        let mut segment = Npc::new(14, (1000.0, 1000.0), 1).unwrap();
        // Leader well away; the segment should close to exactly one gap.
        follow_leader(&mut segment, (1400.0, 1000.0));
        let gap = distance(segment.center(), (1400.0, 1000.0));
        let want = terrustia_proto::npc_params::worm_segment_gap(14, segment.stats.width);
        assert!(
            (gap - want).abs() < 1.0,
            "expected to sit {want} behind, got {gap}"
        );

        // Already close enough: it should not jitter.
        let before = segment.position;
        let here = segment.center();
        follow_leader(&mut segment, here);
        assert_eq!(segment.position, before);
    }
}
