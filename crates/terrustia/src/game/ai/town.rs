//! Style 7 — town residents and the critters that share their routine.
//!
//! Ported from `AI_007_TownEntities`. Bunnies, squirrels, mice, penguins, ducks, turtles and frogs
//! run the same 2,614-line routine as the Guide and the Merchant; what separates them is a handful
//! of table lookups, not a different code path.
//!
//! The shape is a state machine on `ai[0]`. In **state 0** it stands still and counts down; in
//! **state 1** it walks and counts down faster; in the combat states — vanilla's own 10/12/14/15,
//! kept as-is so a real client's animation prediction recognises them — it is fighting back, see
//! [`super::town_combat`]. Which way it walks, and what it does when the ground runs out, is the
//! whole of the walking half of the routine.
//!
//! Three behaviours carry the character:
//!
//! * A resident is on a **leash**. Past twenty-five tiles from its home it will only turn further
//!   away by chance, past fifty it simply turns back, and past thirty-five its walk timer drains
//!   six times as fast when it is heading the wrong way. That is why townsfolk mill about their
//!   houses instead of wandering off.
//! * **Weather sends it indoors.** Rain, nightfall, an eclipse or a slime rain all set the same
//!   flag, and a resident then walks home and stops on its home tile. Critters ignore it.
//! * It **looks before it steps.** Every tick of walking, it probes the tile it is about to walk
//!   onto: a drop, deep water or lava turns it round, a one-, two- or three-tile step gets one of
//!   three jump impulses, and a closed door gets opened and then closed behind it.
//!
//! Not modelled here, and deliberately: shops, dialogue, sitting and pet idle animations — this
//! style was originally scoped to movement, housing and (now) combat only.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    TOWN_FAR_FROM_HOME, TOWN_JUMP, TOWN_JUMP_LOW, TOWN_JUMP_TALL, TOWN_LEASH, TOWN_LEASH_HARD,
    TOWN_STEP_HEIGHT, town_breathes_underwater, town_danger_range, town_hops_in_water,
    town_is_critter, town_is_slime, town_scurries, town_walk,
};
use terrustia_proto::tile::TileFlags;
use terrustia_proto::tile_solid::{solid, solid_top};

use super::town_combat::{self, AttackKind};
use super::{Conditions, MeleeHit, Shot, World, can_see};
use crate::game::npc::{Npc, TILE, TileView};

/// Door and tall-gate tile types.
const DOOR: u16 = 10;
const TALL_GATE: u16 = 388;

/// How far ahead of itself a walker probes, in pixels.
const PROBE_REACH: f32 = 15.0;

/// What the routine wants done to a door it has reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DoorAction {
    #[default]
    None,
    /// Swing it open and walk through.
    Open { x: i32, y: i32, direction: i8 },
    /// Pull it shut again on the way past.
    Close { x: i32, y: i32 },
}

/// Where a resident lives and where the floor of that home is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Home {
    pub tile_x: i32,
    pub floor_y: i32,
}

/// Whether the weather or the hour is telling residents to go inside.
///
/// One flag covers all of it: nightfall, rain, an eclipse or a slime rain. Critters do not read it.
pub fn wants_shelter(conditions: Conditions, npc: &Npc) -> bool {
    !conditions.day
        || conditions.eclipse
        || (conditions.raining && npc.position.1 < conditions.surface_y)
}

fn blocking(tiles: &impl TileView, x: i32, y: i32) -> bool {
    let t = tiles.tile(x, y);
    t.is_active() && !t.flags.has(TileFlags::ACTUATED) && solid(t.block) && !solid_top(t.block)
}

/// Whether a tile is something to stand on: solid, or a platform.
fn footing(tiles: &impl TileView, x: i32, y: i32) -> bool {
    let t = tiles.tile(x, y);
    t.is_active()
        && !t.flags.has(TileFlags::ACTUATED)
        // Platforms are in both sets, and are footing even though they are not walls.
        && solid(t.block)
}

fn door_at(tiles: &impl TileView, x: i32, y: i32) -> Option<u16> {
    let t = tiles.tile(x, y);
    (t.is_active() && (t.block == DOOR || t.block == TALL_GATE)).then_some(t.block)
}

/// Find the floor beneath a home tile, which is what the routine actually walks to.
pub fn floor_under(tiles: &impl TileView, home_x: i32, home_y: i32, limit: i32) -> i32 {
    let mut y = home_y;
    while y < limit && !footing(tiles, home_x, y) {
        y += 1;
    }
    y
}

/// The tile a town NPC is standing on.
fn standing_on(npc: &Npc) -> (i32, i32) {
    (
        ((npc.position.0 + (npc.stats.width / 2) as f32) / TILE) as i32,
        ((npc.position.1 + npc.height() + 1.0) / TILE) as i32,
    )
}

/// Whether the ground ahead is somewhere to avoid stepping.
///
/// Returns true for a drop, for lava, and for water deep enough to drown in. A critter, or a
/// resident heading home from outside its leash, ignores all of it and walks on.
fn avoid_falling<T: TileView>(
    npc: &Npc,
    tiles: &T,
    probe: (i32, i32),
    home: Option<Home>,
    drowning: bool,
) -> bool {
    let (tile_x, _) = standing_on(npc);
    let near_home = home.is_some_and(|h| (tile_x - h.tile_x).abs() <= TOWN_FAR_FROM_HOME);
    let heading_home =
        home.is_some_and(|h| i32::from(npc.direction) == (h.tile_x - tile_x).signum());
    if town_is_critter(npc.npc_type) || (!near_home && heading_home) {
        return false;
    }

    let mut liquid_depth = 0;
    let mut lava = false;
    let mut landed = false;
    for step in -1..=4 {
        let tile = tiles.tile(probe.0, probe.1 + step);
        if tile.liquid > 0 {
            liquid_depth += 1;
            if tile.liquid_kind == terrustia_proto::tile::Liquid::Lava {
                lava = true;
                break;
            }
        }
        if footing(tiles, probe.0, probe.1 + step) {
            landed = true;
            break;
        }
    }
    if lava {
        return true;
    }
    // Water as deep as the NPC is tall would put its head under.
    if liquid_depth >= (npc.height() / TILE).ceil() as i32
        && !town_breathes_underwater(npc.npc_type)
    {
        return true;
    }
    if drowning {
        return false;
    }
    !landed
}

/// Walk up a step rather than jumping it.
///
/// A town NPC steps a little higher than a fighter does — twenty pixels rather than sixteen —
/// which is what lets one climb its own doorstep without hopping.
fn step_up(npc: &mut Npc, tiles: &impl TileView) -> bool {
    let ahead = npc.direction;
    let probe_x = ((npc.position.0 + (npc.stats.width / 2) as f32 + PROBE_REACH * f32::from(ahead))
        / TILE) as i32;
    let foot_y = ((npc.position.1 + npc.height() - 1.0) / TILE) as i32;
    if !blocking(tiles, probe_x, foot_y) {
        return false;
    }
    for up in 1..=2 {
        if blocking(tiles, probe_x, foot_y - up) {
            return false;
        }
    }
    let step_top = foot_y as f32 * TILE;
    let rise = npc.position.1 + npc.height() - step_top;
    if rise <= 0.0 || rise > TOWN_STEP_HEIGHT {
        return false;
    }
    npc.position.1 = step_top - npc.height();
    npc.dirty = true;
    true
}

/// Face whoever is nearest, which is what a critter does instead of holding a course.
fn face_nearest(npc: &mut Npc, world: &World<'_, impl TileView>) {
    if let Some(t) = world.target {
        if npc.position.0 < t.center.0 {
            npc.direction = 1;
        }
        if npc.position.0 > t.center.0 {
            npc.direction = -1;
        }
        npc.sprite_direction = npc.direction;
    }
}

/// Stand still, and decide whether it is time to move.
fn stand<T: TileView>(npc: &mut Npc, world: &World<'_, T>, home: Option<Home>, rng: &mut SmallRng) {
    let shelter = wants_shelter(world.conditions, npc) && !town_is_critter(npc.npc_type);
    let (tile_x, tile_y) = standing_on(npc);

    if shelter && let Some(h) = home {
        if tile_x == h.tile_x && tile_y == h.floor_y {
            // Home: settle to a stop.
            slow_to_a_halt(npc);
        } else {
            npc.direction = if tile_x > h.tile_x { -1 } else { 1 };
            npc.ai[0] = 1.0;
            npc.ai[1] = 200.0 + rng.random_range(0..200) as f32;
            npc.ai[2] = 0.0;
            npc.local_ai[3] = 0.0;
            npc.dirty = true;
        }
    } else {
        if town_scurries(npc.npc_type) {
            npc.velocity.0 *= 0.5;
        }
        slow_to_a_halt(npc);
        if npc.ai[1] > 0.0 {
            npc.ai[1] -= 1.0;
        }

        let probe = probe_tile(npc);
        let drowning = world.wet && !town_breathes_underwater(npc.npc_type);
        let blocked = avoid_falling(npc, world.tiles, probe, home, drowning);
        if drowning {
            start_walking(npc, rng);
        } else if npc.ai[1] <= 0.0 {
            if blocked {
                // Nowhere to go this way; turn and wait a little longer.
                npc.direction = -npc.direction;
                npc.ai[1] = 60.0 + rng.random_range(0..120) as f32;
                npc.dirty = true;
            } else {
                start_walking(npc, rng);
            }
        }
    }

    // The leash. Only applies while it is not being driven indoors.
    if !shelter && let Some(h) = home {
        let drift = tile_x - h.tile_x;
        if !(-TOWN_LEASH..=TOWN_LEASH).contains(&drift) {
            if npc.local_ai[3] == 0.0 {
                if drift < -TOWN_LEASH_HARD && npc.direction == -1 {
                    npc.direction = 1;
                    npc.dirty = true;
                } else if drift > TOWN_LEASH_HARD && npc.direction == 1 {
                    npc.direction = -1;
                    npc.dirty = true;
                }
            }
        } else if npc.local_ai[3] == 0.0 && rng.random_ratio(1, 80) {
            npc.local_ai[3] = 200.0;
            npc.direction = -npc.direction;
            npc.dirty = true;
        }
    }
}

fn slow_to_a_halt(npc: &mut Npc) {
    if npc.velocity.0 > 0.1 {
        npc.velocity.0 -= 0.1;
    } else if npc.velocity.0 < -0.1 {
        npc.velocity.0 += 0.1;
    } else {
        npc.velocity.0 = 0.0;
    }
}

fn start_walking(npc: &mut Npc, rng: &mut SmallRng) {
    npc.ai[0] = 1.0;
    npc.ai[1] = 200.0 + rng.random_range(0..300) as f32;
    npc.ai[2] = 0.0;
    if town_is_critter(npc.npc_type) {
        npc.ai[1] += rng.random_range(200..400) as f32;
    }
    npc.local_ai[3] = 0.0;
    npc.dirty = true;
}

/// Whether this one's head is under, which is vanilla's `flag21` (`NPC.cs:54361`).
///
/// Not the same thing as `wet`. `wet` is the tile the NPC's *centre* is in; drowning is
/// `Collision.DrownCollision(position, width, height, 1f, ...)`, whose box starts two pixels above
/// the NPC's top edge (`Collision.cs:1387-1398`) and so needs water deep enough to cover it. A
/// resident wading across a stream is wet and is not drowning, and conflating the two had every
/// townsperson in ankle-deep water behaving as though it were going under: its walk timer froze and
/// (once the danger override below existed) it broke into a run.
///
/// Lava and shimmer are excluded the way `DrownCollision` excludes them (`Collision.cs:1418`).
fn drowning<T: TileView>(tiles: &T, npc: &Npc) -> bool {
    let x = ((npc.position.0 + npc.width() / 2.0) / TILE) as i32;
    let y = ((npc.position.1 - 2.0) / TILE) as i32;
    let tile = tiles.tile(x, y);
    tile.liquid > 0
        && !matches!(
            tile.liquid_kind,
            terrustia_proto::tile::Liquid::Lava | terrustia_proto::tile::Liquid::Shimmer
        )
}

/// The tile just ahead of the NPC's feet, which is what everything probes.
fn probe_tile(npc: &Npc) -> (i32, i32) {
    (
        ((npc.position.0 + (npc.stats.width / 2) as f32 + PROBE_REACH * f32::from(npc.direction))
            / TILE) as i32,
        ((npc.position.1 + npc.height() - 16.0) / TILE) as i32,
    )
}

/// Walk, and deal with whatever is in the way.
fn walk<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    home: Option<Home>,
    rng: &mut SmallRng,
) -> DoorAction {
    let (tile_x, tile_y) = standing_on(npc);
    let shelter = wants_shelter(world.conditions, npc) && !town_is_critter(npc.npc_type);

    // Arrived home in bad weather: stop.
    if shelter
        && let Some(h) = home
        && tile_x == h.tile_x
        && tile_y == h.floor_y
    {
        npc.ai[0] = 0.0;
        npc.ai[1] = 200.0 + rng.random_range(0..200) as f32;
        npc.local_ai[3] = 60.0;
        npc.dirty = true;
        return DoorAction::None;
    }

    let drowning = drowning(world.tiles, npc) && !town_breathes_underwater(npc.npc_type);
    if !drowning {
        // Walking away from home, far out: the timer drains six times as fast.
        if let Some(h) = home
            && (tile_x < h.tile_x - TOWN_FAR_FROM_HOME || tile_x > h.tile_x + TOWN_FAR_FROM_HOME)
        {
            let away = (npc.position.0 < (h.tile_x * 16) as f32 && npc.direction == -1)
                || (npc.position.0 > (h.tile_x * 16) as f32 && npc.direction == 1);
            if away {
                npc.ai[1] -= 5.0;
            }
        }
        npc.ai[1] -= 1.0;
    }
    if npc.ai[1] <= 0.0 {
        npc.ai[0] = 0.0;
        npc.ai[1] = 300.0 + rng.random_range(0..300) as f32;
        npc.ai[2] = 0.0;
        if town_is_critter(npc.npc_type) {
            npc.ai[1] -= rng.random_range(0..100) as f32;
        } else {
            npc.ai[1] += rng.random_range(0..900) as f32;
        }
        npc.local_ai[3] = 60.0;
        npc.dirty = true;
    }

    // Accelerate, or shed speed if something else pushed it past its limit. A resident with
    // something hostile inside its own detection range, or one that is drowning, drops the whole
    // per-type table and hurries (`NPC.cs:54467-54473`).
    let danger = world.hostile.is_some_and(|h| {
        let (cx, cy) = npc.center();
        h.alive && (h.center.0 - cx).hypot(h.center.1 - cy) < town_danger_range(npc.npc_type)
    });
    let speed = town_walk(
        npc.npc_type,
        world.wet,
        npc.stats.friendly && (danger || drowning),
        1.0 - npc.life as f32 / npc.life_max.max(1) as f32,
    );
    if town_hops_in_water(npc.npc_type) && world.wet {
        // A frog kicks once and then coasts.
        if npc.velocity.0.abs() < 0.05 && npc.velocity.1.abs() < 0.05 {
            npc.velocity.0 += speed.max * 10.0 * f32::from(npc.direction);
        } else {
            npc.velocity.0 *= 0.9;
        }
    } else if npc.velocity.0 < -speed.max || npc.velocity.0 > speed.max {
        if npc.velocity.1 == 0.0 {
            npc.velocity.0 *= 0.8;
            npc.velocity.1 *= 0.8;
        }
    } else if npc.velocity.0 < speed.max && npc.direction == 1 {
        npc.velocity.0 = (npc.velocity.0 + speed.accel).min(speed.max);
    } else if npc.velocity.0 > -speed.max && npc.direction == -1 {
        npc.velocity.0 -= speed.accel;
    }

    if npc.velocity.1 == 0.0 {
        step_up(npc, world.tiles);
    }

    npc.sprite_direction = npc.direction;
    npc.dirty = true;

    if npc.velocity.1 != 0.0 {
        // Airborne: nothing to negotiate until it lands.
        return DoorAction::None;
    }

    let probe = probe_tile(npc);
    let blocked = avoid_falling(npc, world.tiles, probe, home, drowning);

    // A door is opened rather than climbed, and a resident in bad weather never dithers about it.
    let head = (probe.0, probe.1 - 2);
    if !town_is_critter(npc.npc_type)
        && door_at(world.tiles, head.0, head.1).is_some()
        && (shelter || rng.random_ratio(1, 10))
    {
        npc.ai[1] += 80.0;
        npc.ai[2] = f32::from(npc.direction);
        // Vanilla remembers *which* door it opened in `doorX`/`doorY` alongside the `closeDoor`
        // flag (`NPC.cs:54612-54614`), and the close check later compares its own position against
        // those remembered tiles. Nothing here can be re-derived from the probe later: by the time
        // the resident is two tiles clear the probe has moved with it. `local_ai` is server-side
        // only, never sent, and the town routine uses nothing but slot 3.
        npc.local_ai[0] = head.0 as f32;
        npc.local_ai[1] = head.1 as f32;
        npc.dirty = true;
        return DoorAction::Open {
            x: head.0,
            y: head.1,
            direction: npc.direction,
        };
    }

    let heading = (npc.velocity.0 < 0.0 && npc.direction == -1)
        || (npc.velocity.0 > 0.0 && npc.direction == 1);
    if heading {
        // Three obstacle heights, three impulses. Anything taller is turned away from.
        if blocking(world.tiles, head.0, head.1) {
            if !blocking(world.tiles, head.0, head.1 - 1) {
                npc.velocity.1 = -TOWN_JUMP_TALL;
            } else {
                npc.direction = -npc.direction;
                npc.velocity.0 = 0.0;
            }
            npc.dirty = true;
        } else if blocking(world.tiles, probe.0, probe.1 - 1) {
            npc.velocity.1 = -TOWN_JUMP;
            npc.dirty = true;
        } else if npc.position.1 + npc.height() - (probe.1 * 16) as f32 > 20.0
            && blocking(world.tiles, probe.0, probe.1)
        {
            npc.velocity.1 = -TOWN_JUMP_LOW;
            npc.dirty = true;
        } else if blocked {
            npc.direction = -npc.direction;
            npc.velocity.0 = 0.0;
            npc.dirty = true;
        }
    }

    // Pull the door shut once well past it (`NPC.cs:54393`), measured against the door it actually
    // opened rather than against the tile in front of its feet: the probe is only fifteen pixels
    // ahead of its own centre, so a comparison against that can never be more than one tile and
    // the door would never be shut at all. Vanilla compares in fractional tiles, not whole ones.
    if npc.ai[2] != 0.0 {
        let here = (npc.position.0 + (npc.stats.width / 2) as f32) / TILE;
        let (door_x, door_y) = (npc.local_ai[0], npc.local_ai[1]);
        if here > door_x + 2.0 || here < door_x - 2.0 {
            npc.ai[2] = 0.0;
            return DoorAction::Close {
                x: door_x as i32,
                y: door_y as i32,
            };
        }
    }

    DoorAction::None
}

/// Drive one town NPC or critter for a tick.
/// What a tick of the town routine did, beyond moving the NPC itself.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TownUpdate {
    pub door: DoorAction,
    pub shot: Option<Shot>,
    pub melee: Option<MeleeHit>,
}

impl From<DoorAction> for TownUpdate {
    fn from(door: DoorAction) -> Self {
        Self {
            door,
            ..Self::default()
        }
    }
}

pub fn update<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    home: Option<Home>,
    rng: &mut SmallRng,
) -> TownUpdate {
    npc.direction_y = -1;
    if npc.direction == 0 {
        npc.direction = 1;
    }

    // A critter always faces whoever is nearest; a resident holds its course.
    if town_is_critter(npc.npc_type) {
        face_nearest(npc, world);
    }

    if npc.local_ai[3] > 0.0 {
        npc.local_ai[3] -= 1.0;
    }

    // A town slime bobs rather than sinks.
    if town_is_slime(npc.npc_type) && world.wet && npc.velocity.1 > 0.0 {
        npc.velocity.1 *= 0.5;
    }

    if let Some(fought) = try_combat(npc, world, rng) {
        return fought;
    }

    if npc.ai[0] == 1.0 {
        walk(npc, world, home, rng).into()
    } else {
        // Every other state is a rest or an animation; the ones this port does not model fall
        // back to standing, which is what the game does between them anyway.
        npc.ai[0] = 0.0;
        stand(npc, world, home, rng);
        TownUpdate::default()
    }
}

/// Fight back, if this NPC has a combat profile and a hostile is worth answering.
///
/// `Some` means combat owned this tick — either just opened fire, is mid-cooldown, or just
/// finished a swing — and `update` should not also try to walk or stand it this tick. `None` means
/// there is nothing to fight (no profile, no target in range, airborne — vanilla's own attack
/// branches all gate on `velocity.Y == 0f` too — or blocked, see below) and the ordinary dispatch
/// should run instead.
///
/// **Line of sight is checked once, at the moment combat is entered — not on every firing tick.**
/// That is deliberate, not an oversight: real vanilla (`AI_007_TownEntities`, `NPC.cs:56012-56109`)
/// re-verifies `Collision.CanHit`/`CanHitLine` on the chosen candidate right before the state
/// transition into its combat states (10/12/14/15), the same point this gate sits at — but once a
/// ranged attacker (state 10/12/14) is actually in that state, its firing loop
/// (`NPC.cs:54892-55087` and neighbours) never rechecks `CanHit` again for as long as the state
/// lasts. `hostile` itself (the caller's own nearest-candidate scan, `game/server.rs`) is filtered
/// on `can_see` too, matching vanilla's own initial scan (`NPC.cs:54033`), so a hostile behind a
/// wall never reaches here as a candidate in the first place; this is the second, commit-time gate
/// vanilla also has, not a substitute for the first. Two real, disclosed narrowings from full
/// vanilla fidelity: vanilla's melee state (15) uniquely re-checks `CanHit` per potential victim on
/// every tick of the swing, including at the end of each swing to decide whether to keep swinging
/// (`NPC.cs:55632-55676`) — not modelled here, since `town_combat`'s own melee shape is already a
/// single tracked target rather than vanilla's real swing-rectangle scan over every nearby hostile;
/// and vanilla splits the check itself by `AttackType` (`CanHit` for types 0/3, `CanHitLine` for
/// types 1/2, two genuinely different algorithms — `Collision.cs`), while this uses the one
/// `can_see`/`Collision.CanHit` port every boss AI file already shares, rather than porting a
/// second, otherwise-unused `CanHitLine`.
fn try_combat<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    rng: &mut SmallRng,
) -> Option<TownUpdate> {
    let combat = town_combat::town_combat(npc.npc_type)?;
    let hostile = world.hostile.filter(|h| h.alive)?;
    // The hardmode burst. One type has one (the Arms Dealer); everybody else's ladder is in
    // `combat.shots` because vanilla's is unconditional.
    let shots = if world.conditions.hardmode {
        town_combat::hardmode_shots(npc.npc_type).unwrap_or(combat.shots)
    } else {
        combat.shots
    };

    let already_fighting = npc.ai[0] == combat.state;
    if !already_fighting {
        if npc.velocity.1 != 0.0 {
            return None;
        }
        let (dx, dy) = (
            hostile.center.0 - npc.center().0,
            hostile.center.1 - npc.center().1,
        );
        if (dx * dx + dy * dy).sqrt() > combat.range {
            return None;
        }
        if !can_see(world.tiles, npc, hostile) {
            return None;
        }
        npc.ai[0] = combat.state;
        npc.local_ai[2] = -1.0;
    }

    // Between attacks: vanilla's own gate, `Main.rand.Next(AttackAverageChance[type]) == 0` per
    // tick (`NPC.cs:56012`) - a geometric wait rather than a fixed one, which is why two Merchants
    // side by side do not fire in lockstep without anything having to jitter them.
    // The frame counter is vanilla's `localAI[3]`, kept in `local_ai[2]` here because this
    // module's own pause timer already owns `local_ai[3]` and decrements it every tick - which is
    // exactly what a frame counter must not have happen to it.
    if npc.local_ai[2] < 0.0 {
        if rng.random_range(0..combat.average_chance.max(1)) != 0 {
            return Some(TownUpdate::default());
        }
        // The state opens. `ai[1] = AttackTime[type]` and `localAI[3] = 0`
        // (`NPC.cs:56030-56033`); the attack now runs for that long and cannot be re-rolled.
        npc.ai[1] = combat.attack_time as f32;
        npc.local_ai[2] = 0.0;
    }

    // `velocity.X *= 0.8f`, `ai[1]--`, `localAI[3]++`, and then the shot gate
    // (`NPC.cs:55045-55049`). A town NPC winding up visibly stops, which is the telegraph.
    npc.velocity.0 *= 0.8;
    npc.ai[1] -= 1.0;
    npc.local_ai[2] += 1.0;
    let frame = npc.local_ai[2] as i32;
    // The state ends when its own clock runs out, and only then can another be rolled for.
    if npc.ai[1] <= 0.0 {
        npc.local_ai[2] = -1.0;
    }
    // Melee swings every tick of the state; everything else fires on its own marks.
    if !shots.is_empty() && !shots.contains(&frame) {
        return Some(TownUpdate::default());
    }

    let (dx, dy) = (
        hostile.center.0 - npc.center().0,
        hostile.center.1 - npc.center().1,
    );
    let distance = (dx * dx + dy * dy).sqrt().max(1.0);
    npc.direction = if dx < 0.0 { -1 } else { 1 };

    Some(match combat.kind {
        AttackKind::Ranged {
            projectile,
            damage,
            speed,
            ..
        } => TownUpdate {
            shot: Some(Shot {
                projectile,
                damage: town_combat::town_npc_damage(damage, world.conditions.expert),
                // Vanilla's own launch point for every branch of `AI_007_TownEntities`
                // (`NPC.cs`, e.g. line 1553): `base.Center.X + spriteDirection * 16, base.Center.Y
                // - 2` — a real player watching this fire live saw the plain, un-offset
                // `npc.center()` this used to read as the shot visibly leaving from around the
                // NPC's head rather than an outstretched hand.
                position: (
                    npc.center().0 + f32::from(npc.direction) * 16.0,
                    npc.center().1 - 2.0,
                ),
                velocity: (dx / distance * speed, dy / distance * speed),
                // Zero means "whatever the projectile's own table says", which is what vanilla
                // gets: `NewProjectile` never passes a lifetime, so every one of these takes the
                // `timeLeft` its `SetDefaults` gave it. The flat 300 here was invented, and it was
                // wrong in both directions - it cut the Dryad's ward off at 300 ticks when its arm
                // runs to 570, and it kept the Princess's weapon alive for 300 when its own table
                // says 180. This is the same shape as the Empress's seven shots, where a made-up
                // 900 stopped a homing streak from ever homing. The Goblin Tinkerer and the Golfer
                // are the two exceptions, and vanilla's are explicit; see
                // [`town_combat::shot_lifetime`].
                time_left: town_combat::shot_lifetime(npc.npc_type),
            }),
            ..TownUpdate::default()
        },
        AttackKind::Melee {
            damage,
            knockback,
            reach,
        } => {
            if dx.abs() > reach.0 || dy.abs() > reach.1 {
                // In range to have opened the fight, out of swinging reach on this exact tick —
                // vanilla's own hitbox check misses the same way.
                TownUpdate::default()
            } else {
                TownUpdate {
                    melee: Some(MeleeHit {
                        target: hostile.slot,
                        damage: town_combat::town_npc_damage(damage, world.conditions.expert),
                        knockback,
                        direction: npc.direction,
                    }),
                    ..TownUpdate::default()
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    #[derive(Default)]
    struct Ground(HashMap<(i32, i32), Tile>);

    impl TileView for Ground {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    /// Flat ground from tile y = 100 down, across the given span.
    fn flat(from: i32, to: i32) -> Ground {
        let mut g = Ground::default();
        for x in from..to {
            for y in 100..110 {
                g.0.insert((x, y), Tile::block(1));
            }
        }
        g
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(3)
    }

    fn stand_on(npc_type: u16, tile_x: i32) -> Npc {
        let mut n = Npc::new(npc_type, (0.0, 0.0), 1).expect("a style 7 type");
        n.position = (tile_x as f32 * TILE, 100.0 * TILE - n.height());
        n
    }

    fn day<'a>(tiles: &'a Ground) -> World<'a, Ground> {
        World {
            tiles,
            target: None,
            wet: false,
            target_wet: false,
            conditions: Conditions {
                day: true,
                surface_y: 90.0 * TILE,
                ..Conditions::default()
            },
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
            army: crate::game::ai::ArmyView::default(),
            treasure: None,
            mage: Default::default(),
            slot: 0,
        }
    }

    /// Drive a town NPC until its attack actually leaves, and say how many ticks that took.
    ///
    /// A shot no longer leaves on the tick the decision is made: the NPC enters its attack state,
    /// slows to a stop, and the projectile leaves on its own `localAI[3]` mark
    /// (`NPC.cs:55049`) - ten frames later for most types, one for a few, thirty for the Dryad.
    /// That gap is the telegraph, and it is what every one of these tests used to assert away.
    fn attack_within(
        npc: &mut Npc,
        w: &World<'_, Ground>,
        rng: &mut SmallRng,
        ticks: u32,
    ) -> (TownUpdate, u32) {
        for _ in 1..=ticks {
            let out = update(npc, w, None, rng);
            if out.shot.is_some() || out.melee.is_some() {
                // `local_ai[2]` is the frame within the attack state, which is the mark the shot
                // actually left on - not the tick, which also carries the geometric wait for the
                // state to open in the first place.
                return (out, npc.local_ai[2] as u32);
            }
        }
        panic!("nothing came out in {ticks} ticks");
    }

    #[test]
    fn a_merchant_fights_back_against_a_nearby_hostile() {
        // Before this pass, `World` had no `hostile` field and `town_combat` did not exist — a
        // settled town NPC never fired regardless of what was nearby. README.md's own words for
        // the gap this closes: "the town stands still and dies."
        let tiles = flat(0, 400);
        let mut merchant = stand_on(17, 200);
        let mut w = day(&tiles);
        w.hostile = Some(crate::game::npc_ai::Target {
            slot: 9,
            center: (merchant.center().0 + 100.0, merchant.center().1),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let mut r = rng();
        let first = update(&mut merchant, &w, None, &mut r);
        assert!(
            first.shot.is_none(),
            "the shot does not leave on the tick the decision is made: `AttackTime` opens the \
             state and the projectile waits for its own `localAI[3]` mark"
        );
        let (result, mark) = attack_within(&mut merchant, &w, &mut r, 600);
        assert_eq!(
            mark, 10,
            "the Merchant's shot leaves on frame 10 of its attack state (`NPC.cs` state 10)"
        );
        let shot = result
            .shot
            .expect("a merchant with a hostile in range should open fire");
        assert_eq!(
            shot.projectile, 48,
            "the merchant's own pistol shot, NPC.cs:54969"
        );
        assert!(shot.damage > 0);
        assert!(
            shot.velocity.0 > 0.0,
            "the hostile is to the right; the shot should aim there"
        );
    }

    /// The Dryad's ward is cast at rest, and with the lifetime its own table gives it.
    ///
    /// Both numbers used to be invented here and both were load-bearing. State 14's speed local
    /// (`NPC.cs:55394`) is only ever assigned in the Clothier's and the Wizard's branches, so the
    /// Dryad's aim vector is multiplied by zero and the circle hangs where she cast it; this file
    /// gave it six pixels a tick, which is 3,420 pixels from the town by the time it expires. And
    /// `NewProjectile` passes no lifetime at all, so every town shot takes its type's own - which
    /// for the ward is 3,600 against a `Projectile.cs:41974` arm that runs to 570. The flat 300
    /// invented here cut it off at little over half.
    #[test]
    fn the_dryads_ward_is_cast_at_rest_and_takes_its_own_lifetime() {
        let tiles = flat(0, 400);
        let mut dryad = stand_on(20, 200);
        let mut w = day(&tiles);
        w.hostile = Some(Target {
            slot: 9,
            center: (dryad.center().0 + 300.0, dryad.center().1),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let mut r = rng();
        let (result, mark) = attack_within(&mut dryad, &w, &mut r, 20_000);
        assert_eq!(mark, 24, "the ward leaves on frame 24 of her attack state");
        let shot = result.shot.expect("the Dryad should raise her ward");
        assert_eq!(shot.projectile, 586, "ProjectileID.DryadsWardCircle");
        assert_eq!(
            shot.velocity,
            (0.0, 0.0),
            "vanilla never gives it a launch speed, so it hangs where it was cast"
        );
        assert_eq!(
            shot.time_left, 0,
            "zero means the projectile's own 3,600, which its arm ends at 570"
        );
        // It is still aimed - the position is the hand offset, not her centre - so this has not
        // been turned into a shot that spawns on top of her.
        assert!(
            (shot.position.0 - dryad.center().0).abs() > 8.0,
            "the launch point is still the outstretched-hand offset"
        );
    }

    /// The Golfer's ball and the Goblin Tinkerer's spiky ball keep vanilla's explicit 480.
    ///
    /// These two are the only shots in `AI_007_TownEntities` whose lifetime is set at all
    /// (`NPC.cs:55070-55077`), and they are also the two with the longest declared lifetimes in
    /// the table - 3,600 and 4,800. Handing them their own value, which is what "zero" does for
    /// everyone else, leaves a defending town strewn with balls for over a minute each instead of
    /// eight seconds.
    #[test]
    fn the_two_town_shots_vanilla_gives_a_lifetime_keep_it() {
        for (npc_type, projectile, table) in [(588u16, 721u16, 3600), (107, 24, 4800)] {
            let tiles = flat(0, 400);
            let mut npc = stand_on(npc_type, 200);
            let mut w = day(&tiles);
            w.hostile = Some(Target {
                slot: 9,
                center: (npc.center().0 + 60.0, npc.center().1),
                velocity: (0.0, 0.0),
                alive: true,
            });
            let mut r = rng();
            let (result, _) = attack_within(&mut npc, &w, &mut r, 20_000);
            let shot = result.shot.expect("both of these are ranged");
            assert_eq!(shot.projectile, projectile, "npc {npc_type}");
            assert_eq!(
                shot.time_left, 480,
                "npc {npc_type}'s shot is given 480 rather than its table's {table}"
            );
        }
        // And nobody else is: the Merchant's pistol shot takes its own.
        let tiles = flat(0, 400);
        let mut merchant = stand_on(17, 200);
        let mut w = day(&tiles);
        w.hostile = Some(Target {
            slot: 9,
            center: (merchant.center().0 + 100.0, merchant.center().1),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let mut r = rng();
        let (result, _) = attack_within(&mut merchant, &w, &mut r, 20_000);
        assert_eq!(result.shot.expect("a shot").time_left, 0);
    }

    /// A solid wall, floor to well above head height, at one tile column. Contiguous with no
    /// gaps, so `can_hit`'s own "two-tile hole" leniency (see `sight.rs`) cannot thread it.
    fn wall_at(tiles: &mut Ground, x: i32) {
        for y in 85..=110 {
            tiles.0.insert((x, y), Tile::block(1));
        }
    }

    #[test]
    fn a_hostile_behind_a_wall_does_not_draw_fire() {
        // Before this fix, `try_combat` never called `can_see` at all — a town NPC with a
        // hostile in range would open fire straight through a solid wall. Same scenario as
        // `a_merchant_fights_back_against_a_nearby_hostile` above, with a wall dropped between
        // them: everything else about the setup (type, distance, direction) is unchanged, so a
        // regression back to firing through it would be caught here, not just by omission.
        let mut tiles = flat(0, 400);
        wall_at(&mut tiles, 203);
        let mut merchant = stand_on(17, 200);
        let mut w = day(&tiles);
        w.hostile = Some(crate::game::npc_ai::Target {
            slot: 9,
            center: (merchant.center().0 + 100.0, merchant.center().1),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let result = update(&mut merchant, &w, None, &mut rng());
        assert!(
            result.shot.is_none(),
            "a wall stands between the merchant and the hostile; it should not fire through it"
        );
    }

    #[test]
    fn a_hostile_behind_a_wall_is_not_swung_at() {
        // The melee counterpart of the test above, covering `AttackKind::Melee` rather than
        // `AttackKind::Ranged` — a different match arm in `try_combat`, so the ranged case
        // passing does not prove this one does. The Dye Trader's own 32px reach (`town_combat.rs`)
        // means the hostile has to be genuinely close (24px, not 100) for the *reach* check inside
        // the melee arm to still pass once line of sight is fixed — otherwise a failure there would
        // prove nothing about the line-of-sight gate specifically. At that spacing there is exactly
        // one tile column between the trader (tile 200) and the hostile (tile 202): tile 201, which
        // is where the wall goes.
        let mut tiles = flat(0, 400);
        wall_at(&mut tiles, 201);
        let mut trader = stand_on(207, 200);
        let mut w = day(&tiles);
        w.hostile = Some(crate::game::npc_ai::Target {
            slot: 5,
            center: (trader.center().0 + 24.0, trader.center().1),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let result = update(&mut trader, &w, None, &mut rng());
        assert!(
            result.melee.is_none(),
            "a wall stands between the trader and the hostile; it should not swing through it"
        );
    }

    #[test]
    fn nothing_fires_with_no_hostile_nearby() {
        let tiles = flat(0, 400);
        let mut merchant = stand_on(17, 200);
        let result = update(&mut merchant, &day(&tiles), None, &mut rng());
        assert!(result.shot.is_none());
        assert!(result.melee.is_none());
    }

    #[test]
    fn a_hostile_out_of_range_is_not_engaged() {
        let tiles = flat(0, 400);
        let mut merchant = stand_on(17, 200);
        let mut w = day(&tiles);
        // Merchant's DangerDetectRange is 320; put the hostile well past it.
        w.hostile = Some(crate::game::npc_ai::Target {
            slot: 9,
            center: (merchant.center().0 + 2000.0, merchant.center().1),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let result = update(&mut merchant, &w, None, &mut rng());
        assert!(result.shot.is_none());
    }

    #[test]
    fn a_dye_trader_swings_at_a_hostile_within_reach() {
        // The one representative attack type that is melee rather than a projectile — vanilla's
        // own state 15 has no `Projectile.NewProjectile` call at all; it strikes directly via
        // `StrikeNPCNoInteraction` (NPC.cs:55637).
        let tiles = flat(0, 400);
        let mut trader = stand_on(207, 200);
        let mut w = day(&tiles);
        w.hostile = Some(crate::game::npc_ai::Target {
            slot: 5,
            center: (trader.center().0 + 10.0, trader.center().1),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let (result, _) = attack_within(&mut trader, &w, &mut rng(), 600);
        let hit = result
            .melee
            .expect("a hostile 10px away is well within the 32px reach");
        assert_eq!(hit.target, 5);
        assert!(hit.damage > 0);
        assert!(
            result.shot.is_none(),
            "the melee type never fires a projectile"
        );
    }

    /// The Pirate fires six times per attack state, not once.
    ///
    /// `NPC.cs:55245-55279`: `num54` starts at 1 and a cascade of
    /// `if (localAI[3] > num54) { num54 = <next>; }` walks it to 16, 24, 32, 40 and 48. Because
    /// `localAI[3]` is read before its own increment and the shot fires on `localAI[3] == num54`
    /// after it, each rung is one shot. The module doc used to name this burst as unmodelled by
    /// name; it is the longest of the four ladders.
    #[test]
    fn a_pirate_fires_a_six_shot_burst_within_one_attack_state() {
        let tiles = flat(0, 400);
        let mut pirate = stand_on(229, 200);
        let mut w = day(&tiles);
        w.hostile = Some(crate::game::npc_ai::Target {
            slot: 5,
            center: (pirate.center().0 + 250.0, pirate.center().1),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let mut r = rng();

        // One whole attack state: `AttackTime[229]` is 60, so run until it closes and record the
        // frame each shot left on.
        let mut marks = Vec::new();
        let mut opened = false;
        let mut closed = false;
        for _ in 0..2_000 {
            let out = update(&mut pirate, &w, None, &mut r);
            let in_state = pirate.local_ai[2] >= 0.0;
            if in_state {
                opened = true;
            }
            if out.shot.is_some() {
                marks.push(pirate.local_ai[2] as i32);
            }
            if opened && !in_state {
                closed = true;
                break;
            }
        }
        assert_eq!(
            marks,
            vec![1, 16, 24, 32, 40, 48],
            "vanilla's own ladder, in order, inside a single state"
        );
        assert!(
            closed,
            "and the state ends when `AttackTime` runs out, rather than running for ever - a \
             burst that never closes is a Pirate that never stops shooting"
        );
    }

    /// ...and the one ladder that *is* behind hardmode really is: the Arms Dealer fires once in
    /// classic and four times in hardmode (`NPC.cs:55129-55147`).
    #[test]
    fn the_arms_dealers_burst_is_hardmode_only() {
        let shots_in = |hardmode: bool| {
            let tiles = flat(0, 400);
            let mut dealer = stand_on(19, 200);
            let mut w = day(&tiles);
            w.conditions.hardmode = hardmode;
            w.hostile = Some(crate::game::npc_ai::Target {
                slot: 5,
                center: (dealer.center().0 + 250.0, dealer.center().1),
                velocity: (0.0, 0.0),
                alive: true,
            });
            let mut r = rng();
            let mut marks = Vec::new();
            let mut opened = false;
            for _ in 0..2_000 {
                let out = update(&mut dealer, &w, None, &mut r);
                let in_state = dealer.local_ai[2] >= 0.0;
                if in_state {
                    opened = true;
                }
                if out.shot.is_some() {
                    marks.push(dealer.local_ai[2] as i32);
                }
                if opened && !in_state {
                    break;
                }
            }
            marks
        };
        assert_eq!(
            shots_in(false),
            vec![1],
            "one shot before the mechanical bosses"
        );
        assert_eq!(shots_in(true), vec![1, 10, 20, 30], "and four after them");
    }

    /// ...and the Painter's is not, which is the mistake that hid behind that one.
    ///
    /// `NPC.cs:55159-55168` is the Painter's ladder and it stands on its own; the
    /// `if (Main.hardMode)` immediately under it (`:55169-55172`) adds two damage and nothing else.
    /// The ladder was in `hardmode_shots`, so a Painter defending a town before the mechanical
    /// bosses fired once where the game fires three times. Both halves are asserted, because only
    /// the classic one was ever wrong and a test that checked hardmode alone would have passed.
    #[test]
    fn the_painters_burst_is_not_hardmode_only() {
        let shots_in = |hardmode: bool| {
            let tiles = flat(0, 400);
            let mut painter = stand_on(227, 200);
            let mut w = day(&tiles);
            w.conditions.hardmode = hardmode;
            w.hostile = Some(crate::game::npc_ai::Target {
                slot: 5,
                center: (painter.center().0 + 250.0, painter.center().1),
                velocity: (0.0, 0.0),
                alive: true,
            });
            let mut r = rng();
            let mut marks = Vec::new();
            let mut opened = false;
            for _ in 0..2_000 {
                let out = update(&mut painter, &w, None, &mut r);
                let in_state = painter.local_ai[2] >= 0.0;
                if in_state {
                    opened = true;
                }
                if out.shot.is_some() {
                    marks.push(painter.local_ai[2] as i32);
                }
                if opened && !in_state {
                    break;
                }
            }
            marks
        };
        assert_eq!(
            shots_in(false),
            vec![1, 12, 24],
            "three shots before the mechanical bosses, not one"
        );
        assert_eq!(shots_in(true), vec![1, 12, 24], "and the same three after");
    }

    #[test]
    fn a_dryad_shoots_at_a_hostile_in_range() {
        // Her ranged attack is a real vanilla outlier — zero pre-scaling damage and a 600-tick
        // cooldown, both far off every other combat-capable town NPC's numbers (see
        // `town_combat`'s own module doc) — worth its own direct test rather than trusting the
        // exhaustive end-to-end sweep in `gameplay.rs` alone to exercise this specific shape.
        let tiles = flat(0, 400);
        let mut dryad = stand_on(20, 200);
        let mut w = day(&tiles);
        w.hostile = Some(crate::game::npc_ai::Target {
            slot: 5,
            center: (dryad.center().0 + 10.0, dryad.center().1),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let (result, mark) = attack_within(&mut dryad, &w, &mut rng(), 600);
        assert_eq!(
            mark, 24,
            "her own mark is frame 24, the longest windup on the roster"
        );
        let shot = result
            .shot
            .expect("a hostile 10px away is well within her 1200px range");
        assert_eq!(shot.projectile, 586);
        assert_eq!(shot.damage, 0, "her attack is faithfully harmless");
    }

    #[test]
    fn a_town_npc_with_no_combat_profile_still_does_not_fight() {
        // Town Cat (637): vanilla's own `AttackType` set explicitly re-asserts `-1` for town pets
        // rather than leaving it at the array's default (`NPCID.cs:4855`) — a real town resident
        // with no combat profile, and, unlike an ordinary town NPC this project simply hasn't
        // covered yet, guaranteed to *stay* that way rather than gain one the next time this
        // module's coverage grows (see `game::ai::town_combat`'s own module doc: all 28 real
        // `AttackType` NPCs are covered as of this session).
        let tiles = flat(0, 400);
        let mut cat = stand_on(637, 200);
        let mut w = day(&tiles);
        w.hostile = Some(crate::game::npc_ai::Target {
            slot: 9,
            center: (cat.center().0 + 10.0, cat.center().1),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let result = update(&mut cat, &w, None, &mut rng());
        assert!(result.shot.is_none());
        assert!(result.melee.is_none());
    }

    #[test]
    fn a_shot_just_fired_does_not_fire_again_until_its_cooldown_elapses() {
        let tiles = flat(0, 400);
        let mut merchant = stand_on(17, 200);
        let mut w = day(&tiles);
        w.hostile = Some(crate::game::npc_ai::Target {
            slot: 9,
            center: (merchant.center().0 + 100.0, merchant.center().1),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let mut r = rng();
        let (first, _) = attack_within(&mut merchant, &w, &mut r, 600);
        assert!(first.shot.is_some());
        let second = update(&mut merchant, &w, None, &mut r);
        assert!(
            second.shot.is_none(),
            "a fresh shot every single tick would be a machine gun"
        );
    }

    #[test]
    fn a_bunny_is_a_critter_and_the_guide_is_not() {
        assert!(town_is_critter(46), "bunny");
        assert!(town_is_critter(299), "squirrel");
        assert!(town_is_critter(616), "turtle");
        assert!(!town_is_critter(22), "the guide is a resident");
    }

    #[test]
    fn a_critter_faces_whoever_is_nearest() {
        let tiles = flat(0, 400);
        let mut bunny = stand_on(46, 200);
        bunny.direction = -1;
        let mut w = day(&tiles);
        w.target = Some(Target {
            slot: 0,
            center: (300.0 * TILE, 100.0 * TILE),
            velocity: (0.0, 0.0),
            alive: true,
        });
        update(&mut bunny, &w, None, &mut rng());
        assert_eq!(bunny.direction, 1, "should turn toward the player");
    }

    #[test]
    fn a_resident_walks_home_when_the_weather_turns() {
        let tiles = flat(0, 400);
        let mut guide = stand_on(22, 250);
        let home = Some(Home {
            tile_x: 200,
            floor_y: 100,
        });
        let mut w = day(&tiles);
        w.conditions.day = false;
        let mut r = rng();
        update(&mut guide, &w, home, &mut r);
        assert_eq!(guide.ai[0], 1.0, "should set off");
        assert_eq!(guide.direction, -1, "and head toward home");
    }

    #[test]
    fn a_resident_at_home_in_bad_weather_stops() {
        let tiles = flat(0, 400);
        let mut guide = stand_on(22, 200);
        guide.velocity = (0.05, 0.0);
        let home = Some(Home {
            tile_x: standing_on(&guide).0,
            floor_y: standing_on(&guide).1,
        });
        let mut w = day(&tiles);
        w.conditions.day = false;
        update(&mut guide, &w, home, &mut rng());
        assert_eq!(guide.velocity.0, 0.0, "should have settled");
        assert_eq!(guide.ai[0], 0.0, "and stayed put");
    }

    /// Given the same house and the same weather, a resident heads for it and a critter does not.
    #[test]
    fn a_critter_ignores_the_weather_a_resident_obeys_it() {
        let tiles = flat(0, 400);
        let home = Some(Home {
            tile_x: 200,
            floor_y: 100,
        });
        let mut w = day(&tiles);
        w.conditions.day = false;

        let mut guide = stand_on(22, 250);
        guide.direction = 1;
        update(&mut guide, &w, home, &mut rng());
        assert_eq!(guide.direction, -1, "a resident turns for home");

        let mut bunny = stand_on(46, 250);
        bunny.direction = 1;
        update(&mut bunny, &w, home, &mut rng());
        assert_eq!(bunny.direction, 1, "a bunny keeps hopping");
    }

    #[test]
    fn a_walker_accelerates_to_its_own_speed() {
        let tiles = flat(0, 400);
        for (npc_type, want) in [(22u16, 1.0f32), (299, 1.5), (300, 2.0)] {
            let mut n = stand_on(npc_type, 200);
            n.ai[0] = 1.0;
            n.ai[1] = 5000.0;
            n.direction = 1;
            for _ in 0..200 {
                update(&mut n, &day(&tiles), None, &mut rng());
                n.velocity.1 = 0.0;
            }
            assert!(
                (n.velocity.0 - want).abs() < 0.01,
                "type {npc_type} should walk at {want}, got {}",
                n.velocity.0
            );
        }
    }

    #[test]
    fn a_turtle_is_slow_on_land_and_quick_in_water() {
        assert_eq!(town_walk(616, false, false, 0.0).max, 0.5);
        assert_eq!(town_walk(616, true, false, 0.0).max, 2.0);
        assert_eq!(
            town_walk(625, true, false, 0.0).max,
            2.5,
            "a sea turtle more so"
        );
    }

    /// M5: the per-type table matched vanilla, but `NPC.cs:54467-54473` then overrides all of it
    /// for any friendly NPC with a hostile inside its detection range or one that is drowning:
    /// `num22 = 1.5 + (1 - life/lifeMax) * 0.9` and `num23 = 0.1`. Without it, residents ambled at
    /// 1.0 through a Blood Moon and a drowning townsperson never hurried out.
    #[test]
    fn a_resident_in_danger_drops_the_table_and_hurries() {
        let calm = town_walk(22, false, false, 0.0);
        assert_eq!((calm.max, calm.accel), (1.0, 0.07), "the Guide's own speed");

        let alarmed = town_walk(22, false, true, 0.0);
        assert_eq!((alarmed.max, alarmed.accel), (1.5, 0.1));

        // Wounded, it runs: at a sliver of health the term is worth the full 0.9.
        let bleeding = town_walk(22, false, true, 1.0);
        assert!((bleeding.max - 2.4).abs() < 1e-6, "got {}", bleeding.max);

        // It beats even the fastest per-type entry, which is the point of the override.
        assert!(alarmed.max > town_walk(22, false, false, 0.0).max);
        let mouse_calm = town_walk(300, false, false, 0.0);
        assert_eq!((mouse_calm.max, mouse_calm.accel), (2.0, 1.0));
        assert_eq!(
            town_walk(300, false, true, 0.0).max,
            1.5,
            "a frightened mouse takes the override, not its own sprint"
        );

        // A town slime in water is written after the override and wins outright
        // (`NPC.cs:54473-54477`).
        let slime = town_walk(670, true, true, 1.0);
        assert_eq!((slime.max, slime.accel), (2.0, 0.2));
    }

    /// The other half of the override is drowning, and drowning is not the same as wet.
    ///
    /// Vanilla's `flag21` is `Collision.DrownCollision(position, width, height, 1f, ...)`
    /// (`NPC.cs:54361`), whose box sits two pixels above the NPC's top edge
    /// (`Collision.cs:1387-1398`), while `wet` is only the tile its centre is in. This port read
    /// `world.wet` for both, so a resident standing in a puddle had its walk timer frozen and, once
    /// the danger override existed, ran as though it were going under. A real server test caught it:
    /// a merchant that fell through a water pocket kept the panic speed all the way down.
    #[test]
    fn wading_is_not_drowning() {
        let mut tiles = flat(0, 400);
        let merchant = stand_on(17, 208);
        let (feet_x, feet_y) = (
            ((merchant.position.0 + merchant.width() / 2.0) / TILE) as i32,
            ((merchant.position.1 + merchant.height() - 1.0) / TILE) as i32,
        );
        let head_y = ((merchant.position.1 - 2.0) / TILE) as i32;

        assert!(!drowning(&tiles, &merchant), "dry ground is not drowning");

        // Ankle deep: the tile at its feet is wet, its head is not.
        let mut wet = tiles.0.get(&(feet_x, feet_y)).copied().unwrap_or(Tile::AIR);
        wet.liquid = 255;
        tiles.0.insert((feet_x, feet_y), wet);
        assert!(!drowning(&tiles, &merchant), "wading is not drowning");

        // Under: the tile at its head is water too.
        let mut over = Tile::AIR;
        over.liquid = 255;
        tiles.0.insert((feet_x, head_y), over);
        assert!(drowning(&tiles, &merchant), "head under is");

        // Lava and shimmer are excluded the way `Collision.cs:1418` excludes them.
        let mut lava = over;
        lava.liquid_kind = terrustia_proto::tile::Liquid::Lava;
        tiles.0.insert((feet_x, head_y), lava);
        assert!(!drowning(&tiles, &merchant), "lava is not drowning in");
    }

    /// The detection range is per type, not a flat 200 (`NPCID.cs:4841`).
    #[test]
    fn residents_notice_trouble_at_their_own_ranges() {
        assert_eq!(town_danger_range(22), 700.0, "the Guide");
        assert_eq!(town_danger_range(20), 1200.0, "the Dryad, furthest of all");
        assert_eq!(town_danger_range(353), 60.0, "the Stylist barely looks up");
        assert_eq!(town_danger_range(637), 250.0, "a town pet");
        assert_eq!(town_danger_range(1), 200.0, "everything unnamed");
    }

    #[test]
    fn a_resident_turns_back_at_the_edge_of_its_leash() {
        let tiles = flat(0, 400);
        let mut guide = stand_on(22, 200 + TOWN_LEASH_HARD + 5);
        guide.direction = 1;
        guide.ai[1] = 1000.0;
        let home = Some(Home {
            tile_x: 200,
            floor_y: 100,
        });
        update(&mut guide, &day(&tiles), home, &mut rng());
        assert_eq!(guide.direction, -1, "should turn for home");
    }

    /// Set an NPC walking right, and put the ground's edge exactly where it is about to probe.
    fn walking_toward_the_edge(npc_type: u16) -> (Npc, Ground) {
        let mut n = stand_on(npc_type, 208);
        n.ai[0] = 1.0;
        n.ai[1] = 5000.0;
        n.direction = 1;
        n.velocity.0 = 1.0;
        let edge = probe_tile(&n).0;
        (n, flat(0, edge))
    }

    #[test]
    fn a_resident_stops_at_a_cliff_and_a_critter_does_not() {
        let home = Some(Home {
            tile_x: 200,
            floor_y: 100,
        });
        let (mut guide, tiles) = walking_toward_the_edge(22);
        update(&mut guide, &day(&tiles), home, &mut rng());
        assert_eq!(guide.direction, -1, "a resident looks before it steps");

        let (mut bunny, tiles) = walking_toward_the_edge(46);
        update(&mut bunny, &day(&tiles), None, &mut rng());
        assert_eq!(bunny.direction, 1, "a bunny does not");
    }

    #[test]
    fn a_resident_opens_a_door_rather_than_climbing_it() {
        let mut guide = stand_on(22, 208);
        guide.ai[0] = 1.0;
        guide.ai[1] = 5000.0;
        guide.direction = 1;
        guide.velocity.0 = 1.0;
        let probe = probe_tile(&guide);
        let mut tiles = flat(0, 400);
        // A door filling the three tiles above the floor just ahead.
        for y in (probe.1 - 2)..=probe.1 {
            tiles.0.insert((probe.0, y), Tile::framed(DOOR, 0, 0));
        }
        let mut w = day(&tiles);
        // Bad weather removes the one-in-ten dithering, so the door is tried every tick.
        w.conditions.day = false;
        let action = update(
            &mut guide,
            &w,
            Some(Home {
                tile_x: 300,
                floor_y: 100,
            }),
            &mut rng(),
        );
        assert!(
            matches!(action.door, DoorAction::Open { .. }),
            "expected a door to be opened, got {action:?}"
        );
    }

    /// The other half of the door: vanilla remembers the tile it opened in `doorX`/`doorY` and
    /// pulls it shut once its own centre is more than two tiles from that (`NPC.cs:54393-54406`,
    /// set at `NPC.cs:54612-54614`).
    ///
    /// This port had collapsed both into `ai[2]`, which holds only the direction, and then
    /// re-derived the door from the probe tile, which is fifteen pixels ahead of the NPC's own
    /// centre, so the difference could never exceed one tile and `DoorAction::Close` was
    /// unreachable. Doors stood open all night behind every resident, which is exactly what
    /// vanilla's close-behind-you logic exists to prevent.
    #[test]
    fn a_resident_pulls_the_door_shut_behind_it() {
        let mut guide = stand_on(22, 208);
        guide.ai[0] = 1.0;
        guide.ai[1] = 5000.0;
        guide.direction = 1;
        guide.velocity.0 = 1.0;
        let probe = probe_tile(&guide);
        let mut tiles = flat(0, 400);
        for y in (probe.1 - 2)..=probe.1 {
            tiles.0.insert((probe.0, y), Tile::framed(DOOR, 0, 0));
        }
        let mut w = day(&tiles);
        w.conditions.day = false;
        let home = Some(Home {
            tile_x: 300,
            floor_y: 100,
        });

        let opened = update(&mut guide, &w, home, &mut rng());
        let DoorAction::Open { x, y, .. } = opened.door else {
            panic!("expected the door to be opened first, got {opened:?}");
        };
        assert_ne!(guide.ai[2], 0.0, "it should remember it left one open");

        // Still beside it: nothing to do yet. Vanilla's test is on the NPC's own centre, so this
        // is the case the old probe-derived comparison could never tell apart from the next one.
        tiles.0.clear();
        let mut w = day(&tiles);
        w.conditions.day = false;
        let beside = update(&mut guide, &w, home, &mut rng());
        assert_eq!(
            beside.door,
            DoorAction::None,
            "two tiles is not yet past it"
        );

        // Three tiles on, and it shuts the door it actually opened rather than one under its feet.
        guide.position.0 += 3.0 * TILE;
        let past = update(&mut guide, &w, home, &mut rng());
        assert_eq!(
            past.door,
            DoorAction::Close { x, y },
            "it should shut the tile it opened"
        );
        assert_eq!(guide.ai[2], 0.0, "and stop remembering it");
    }

    #[test]
    fn a_critter_walks_into_a_door_rather_than_opening_it() {
        let mut bunny = stand_on(46, 208);
        bunny.ai[0] = 1.0;
        bunny.ai[1] = 5000.0;
        bunny.direction = 1;
        bunny.velocity.0 = 1.0;
        let probe = probe_tile(&bunny);
        let mut tiles = flat(0, 400);
        for y in (probe.1 - 2)..=probe.1 {
            tiles.0.insert((probe.0, y), Tile::framed(DOOR, 0, 0));
        }
        let action = update(&mut bunny, &day(&tiles), None, &mut rng());
        assert_eq!(action.door, DoorAction::None, "a bunny has no hands");
    }

    #[test]
    fn a_walker_jumps_a_low_wall_and_turns_from_a_tall_one() {
        let mut low = stand_on(22, 208);
        low.ai[0] = 1.0;
        low.ai[1] = 5000.0;
        low.direction = 1;
        low.velocity.0 = 1.0;
        let probe = probe_tile(&low);
        let mut tiles = flat(0, 400);
        // Two tiles of step, which is higher than it can walk up but well within a hop.
        tiles.0.insert((probe.0, probe.1), Tile::block(1));
        tiles.0.insert((probe.0, probe.1 - 1), Tile::block(1));
        update(&mut low, &day(&tiles), None, &mut rng());
        assert!(
            low.velocity.1 < 0.0,
            "should hop it, got {}",
            low.velocity.1
        );

        // A wall taller than anything it can clear.
        let mut tall = stand_on(22, 208);
        tall.ai[0] = 1.0;
        tall.ai[1] = 5000.0;
        tall.direction = 1;
        tall.velocity.0 = 1.0;
        let mut wall = flat(0, 400);
        for y in (probe.1 - 9)..=probe.1 {
            wall.0.insert((probe.0, y), Tile::block(1));
        }
        update(&mut tall, &day(&wall), None, &mut rng());
        assert_eq!(tall.direction, -1, "should give up and turn round");
    }

    #[test]
    fn a_frog_kicks_off_in_water_rather_than_swimming() {
        let tiles = flat(0, 400);
        let mut frog = stand_on(361, 200);
        frog.ai[0] = 1.0;
        frog.ai[1] = 5000.0;
        frog.direction = 1;
        let mut w = day(&tiles);
        w.wet = true;
        update(&mut frog, &w, None, &mut rng());
        assert!(
            frog.velocity.0 > 5.0,
            "a frog should shove off hard, got {}",
            frog.velocity.0
        );
    }

    #[test]
    fn the_walk_timer_drains_faster_when_heading_away_from_home() {
        let tiles = flat(0, 800);
        let home = Some(Home {
            tile_x: 200,
            floor_y: 100,
        });
        let mut away = stand_on(22, 200 + TOWN_FAR_FROM_HOME + 10);
        away.ai[0] = 1.0;
        away.ai[1] = 1000.0;
        away.direction = 1;

        let mut back = stand_on(22, 200 + TOWN_FAR_FROM_HOME + 10);
        back.ai[0] = 1.0;
        back.ai[1] = 1000.0;
        back.direction = -1;

        update(&mut away, &day(&tiles), home, &mut rng());
        update(&mut back, &day(&tiles), home, &mut rng());
        assert_eq!(away.ai[1], 994.0, "six ticks off for walking away");
        assert_eq!(back.ai[1], 999.0, "one for walking back");
    }
}
