//! Styles 27, 28 and 29 — the Wall of Flesh and everything hanging off it.
//!
//! The Wall is the one boss that does not chase: it *advances*. It walks in one direction along the
//! underworld at a fixed pace, faster the more it is hurt, and never turns around. The fight is
//! therefore a race, and everything else in it exists to make the race harder. Expert Mode makes
//! it harder still: five more health thresholds push the pace up sooner, and a final multiplier
//! on top of all of them applies only there.
//!
//! Its **eyes** ride at a quarter and three-quarters of its height and fire lasers in volleys that
//! grow longer as the Wall weakens. Its **Hungry** hang off it on tethers and lunge at whoever comes
//! within reach, on leashes that lengthen at three-quarters and half health so the safe distance
//! keeps shrinking. And it spits **leeches** — worms that swim off after you — once it has been
//! alive long enough, sooner the more hurt it is.
//!
//! Dying is not how you escape it. If everyone in the underworld is dead it fades out over three
//! seconds and the fight is simply lost.

use rand::rngs::SmallRng;
use terrustia_proto::npc_params::{
    HUNGRY_ACCEL, HUNGRY_DEFENSE_DYING, HUNGRY_DEFENSE_WOUNDED, HUNGRY_EXPERT_ACCEL_DYING,
    HUNGRY_EXPERT_ACCEL_WOUNDED, HUNGRY_EXPERT_LEASH_SCALE, HUNGRY_EXPERT_SPEED_BASE,
    HUNGRY_EXPERT_SPEED_BONUS, HUNGRY_EXPERT_SPEED_CATCHUP, HUNGRY_EXPERT_SPEED_FACTOR,
    HUNGRY_EXPERT_SPEED_RAGE, HUNGRY_EXPERT_SPEED_SCALE, HUNGRY_LEASH, HUNGRY_LEASH_DYING,
    HUNGRY_LEASH_WOUNDED, HUNGRY_RECOIL, HUNGRY_SPEED, WALL_EXPERT_SPEED_BONUS,
    WALL_EXPERT_SPEED_SCALE, WALL_EYE, WALL_EYE_CADENCE, WALL_EYE_CHARGE, WALL_EYE_VOLLEY,
    WALL_FADE_TICKS, WALL_HUNGRY, WALL_HUNGRY_COUNT, WALL_LASER_DAMAGE, WALL_LASER_SPEED,
    WALL_LEECH, WALL_LEECH_AFTER, WALL_LEECH_CAP, WALL_LEECH_EVERY, WALL_MIN_HEIGHT, WALL_SPEED,
    WALL_SPEED_BONUS, WALL_SPEED_RAGE, WALL_SPEED_RAGE_EXPERT, WALL_SPEED_SCALE,
};
use terrustia_proto::projectile::ids::WALL_LASER;

use terrustia_proto::tile_solid::solid;

use crate::game::ai::{Shot, World};
use crate::game::npc::{Npc, TILE, TileView};
use crate::game::npc_ai::Spawn;

/// What a tick of the Wall produced.
#[derive(Debug, Default)]
pub struct Advance {
    pub spawn: Vec<Spawn>,
    pub shots: Vec<Shot>,
    /// Set when the fight is over, one way or the other.
    pub gone: bool,
}

/// How many of its health thresholds it has passed, and what they add up to.
///
/// Expert Mode crosses five more of its own, on top of these four (`aiStyle==27`'s
/// `Main.expertMode` block) — so only count them when `expert` is set.
fn rage(npc: &Npc, expert: bool) -> f32 {
    let health = npc.life as f32 / npc.life_max as f32;
    let crossed = |table: &[(f32, f32)]| -> f32 {
        table
            .iter()
            .filter(|(threshold, _)| health < *threshold)
            .map(|(_, extra)| extra)
            .sum()
    };
    let mut total = crossed(&WALL_SPEED_RAGE);
    if expert {
        total += crossed(&WALL_SPEED_RAGE_EXPERT);
    }
    total
}

/// Ease one smoothed shaft edge a pixel a tick toward its freshly-scanned value, snapping straight
/// to it from the game's -1 sentinel (`Main.wofDrawArea*` at `NPC.cs:25902-25976`).
fn ease_toward(edge: &mut f32, target: f32) {
    if *edge == -1.0 {
        *edge = target;
    } else if *edge > target {
        *edge = (*edge - 1.0).max(target);
    } else if *edge < target {
        *edge = (*edge + 1.0).min(target);
    }
}

/// Drive the Wall of Flesh for a tick.
///
/// `leeches` is how many of its worms are still swimming, which the caller counts.
pub fn wall<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    leeches: usize,
    rng: &mut SmallRng,
) -> Advance {
    let mut out = Advance::default();

    // The band it occupies. The game measures this against the terrain each tick; what matters for
    // the fight is that it is a tall slab centred on the Wall, at least ten tiles high.
    let top = npc.position.1 - WALL_MIN_HEIGHT / 2.0;
    let bottom = npc.position.1 + npc.height() + WALL_MIN_HEIGHT / 2.0;

    // First tick: two eyes and a row of Hungry come with it.
    if npc.local_ai[0] == 0.0 {
        npc.local_ai[0] = 1.0;
        // WOF-5: the smoothed floor and ceiling of the shaft it walks (`Main.wofDrawAreaBottom` /
        // `Top`), seeded to the game's -1 sentinel so the first scan snaps them to the real terrain
        // (`NPC.cs:25789-25790`). Kept per-Wall in `local_ai[2]` (bottom) and `local_ai[3]` (top).
        npc.local_ai[2] = -1.0;
        npc.local_ai[3] = -1.0;
        for (side, at) in [
            (1.0, (npc.center().1 + top) / 2.0),
            (-1.0, (npc.center().1 + bottom) / 2.0),
        ] {
            out.spawn.push(Spawn {
                handle: None,
                npc_type: WALL_EYE,
                position: (npc.position.0, at),
                // The side rides in the velocity, as it does for Skeletron's hands: the eyes want
                // only ai[0] = +/-1 (`NPC.cs:26192,26194`), which the sign of the velocity gives.
                velocity: (side, 0.0),
                parent: Some(Spawn::OWN_PARENT),
                ai: [None; 4],
            });
        }
        for n in 0..WALL_HUNGRY_COUNT {
            out.spawn.push(Spawn {
                handle: None,
                npc_type: WALL_HUNGRY,
                position: (npc.position.0, (npc.center().1 + bottom) / 2.0),
                velocity: (0.0, 0.0),
                parent: Some(Spawn::OWN_PARENT),
                // Its fractional height along the wall, spread evenly and read straight back as
                // ai[0] (`NPC.cs:26197`, `ai0 = num403 * 0.1 - 0.05`, n=0..10 so -0.05..0.95). Must be
                // set outright: the signum path would flatten every band to +/-1 and stack all
                // eleven Hungry at two heights instead of spreading them down the Wall.
                ai: [Some(n as f32 * 0.1 - 0.05), None, None, None],
            });
        }
        npc.dirty = true;
    }

    // Leeches, once it has been alive long enough — and sooner the more it is hurt.
    npc.ai[1] += 1.0;
    if npc.ai[2] == 0.0 {
        let health = npc.life as f32 / npc.life_max as f32;
        if health < 0.5 {
            npc.ai[1] += 1.0;
        }
        if health < 0.2 {
            npc.ai[1] += 1.0;
        }
        if npc.ai[1] > WALL_LEECH_AFTER {
            npc.ai[2] = 1.0;
        }
    }
    if npc.ai[2] > 0.0 && npc.ai[1] > WALL_LEECH_EVERY {
        let volley = 3.0
            + if (npc.life as f32) < npc.life_max as f32 * 0.3 {
                1.0
            } else {
                0.0
            };
        npc.ai[2] += 1.0;
        npc.ai[1] = 0.0;
        if npc.ai[2] > volley {
            npc.ai[2] = 0.0;
        }
        if leeches < WALL_LEECH_CAP {
            out.spawn.push(Spawn {
                handle: None,
                npc_type: WALL_LEECH,
                position: (
                    npc.position.0 + npc.width() / 2.0,
                    npc.position.1 + npc.height() / 2.0 + 20.0,
                ),
                velocity: (f32::from(npc.direction) * 8.0, 0.0),
                parent: None,
                ai: [None; 4],
            });
        }
        npc.dirty = true;
    }

    // WOF-5: re-centre vertically in the open shaft it is walking, the way vanilla measures the
    // terrain each tick rather than staying pinned to its spawn height. It scans the tile column the
    // Wall spans for the floor below and the ceiling above (counting solid or liquid tiles), eases
    // the found edges a pixel a tick, clamps them into the underworld and to a 160px minimum gap,
    // and drops its own Y to the middle (`aiStyle==27`, `NPC.cs:25866-25997`). Skipping it left the
    // Wall pinned wherever it spawned.
    let max_y = world.conditions.world_size.1 - 10; // maxTilesY - 10
    let underworld = world.conditions.world_size.1 - 200; // Main.UnderworldLayer
    let limit_top = underworld + 10; // num372
    let limit_bottom = limit_top + 70; // num373
    let col_left = (npc.position.0 / TILE) as i32; // num374
    let col_right = ((npc.position.0 + npc.width()) / TILE) as i32; // num375
    let center_row = ((npc.position.1 + npc.height() / 2.0) / TILE) as i32; // num376
    let blocking = |x: i32, y: i32| {
        let tile = world.tiles.tile(x, y);
        (tile.is_active() && solid(tile.block)) || tile.liquid > 0
    };

    // Floor: scan down from seven tiles below the centre until fifteen blocking tiles are seen.
    let mut count = 0;
    let mut floor_row = center_row + 7; // num378
    while count < 15 && floor_row > underworld {
        floor_row += 1;
        if floor_row > max_y {
            floor_row = max_y;
            break;
        }
        if floor_row < limit_top {
            continue;
        }
        for x in col_left..=col_right {
            if blocking(x, floor_row) {
                count += 1;
            }
        }
    }
    floor_row += 4;

    // Ceiling: scan up from seven tiles above the centre.
    count = 0;
    let mut ceil_row = center_row - 7; // num378
    while count < 15 && ceil_row < max_y {
        ceil_row -= 1;
        if ceil_row <= 10 {
            ceil_row = 10;
            break;
        }
        if ceil_row > limit_bottom {
            continue;
        }
        if ceil_row < limit_top {
            ceil_row = limit_top;
            break;
        }
        for x in col_left..=col_right {
            if blocking(x, ceil_row) {
                count += 1;
            }
        }
    }
    ceil_row -= 4;

    // Ease the smoothed edges toward the scan, clamp them into the underworld band, hold them at
    // least 160px apart, and centre the Wall between them.
    ease_toward(&mut npc.local_ai[2], (floor_row * 16) as f32);
    ease_toward(&mut npc.local_ai[3], (ceil_row * 16) as f32);
    let (lo, hi) = ((limit_top * 16) as f32, (limit_bottom * 16) as f32);
    npc.local_ai[2] = npc.local_ai[2].clamp(lo, hi);
    npc.local_ai[3] = npc.local_ai[3].clamp(lo, hi);
    if npc.local_ai[3] > npc.local_ai[2] - 160.0 {
        npc.local_ai[3] = npc.local_ai[2] - 160.0;
    } else if npc.local_ai[2] < npc.local_ai[3] + 160.0 {
        npc.local_ai[2] = npc.local_ai[3] + 160.0;
    }
    npc.position.1 = (npc.local_ai[2] + npc.local_ai[3]) / 2.0 - npc.height() / 2.0;

    // It walks, and only ever the way it is already going. Expert Mode applies its own multiplier
    // on top of every threshold crossed above (its own five, and Normal's four) — separate from,
    // and stacking with, get-fixed-boi's multiplier, which is applied only in a For-the-Worthy
    // world (`NPC.cs`'s `aiStyle==27` block gates its `velocity.X *= 1.1; += 0.2` on
    // `Main.getGoodWorld`). Applying it everywhere made every Wall walk at the secret seed's pace.
    let expert = world.conditions.expert;
    let mut speed = WALL_SPEED + rage(npc, expert);
    if expert {
        speed = speed * WALL_EXPERT_SPEED_SCALE + WALL_EXPERT_SPEED_BONUS;
    }
    if world.conditions.get_good_world {
        speed = speed * WALL_SPEED_SCALE + WALL_SPEED_BONUS;
    }
    if npc.velocity.0 == 0.0 {
        // Its opening direction is toward whoever woke it.
        if let Some(t) = world.target {
            npc.direction = if npc.center().0 < t.center.0 { 1 } else { -1 };
        }
        npc.velocity.0 = f32::from(npc.direction);
    }
    if npc.velocity.0 < 0.0 {
        npc.velocity.0 = -speed;
        npc.direction = -1;
    } else {
        npc.velocity.0 = speed;
        npc.direction = 1;
    }
    npc.velocity.1 = 0.0;
    npc.sprite_direction = npc.direction;

    // Nobody alive down here: it fades out over three seconds and the fight is lost.
    let everyone_dead = world.target.is_none_or(|t| !t.alive);
    if everyone_dead {
        npc.local_ai[1] += 1.0 / WALL_FADE_TICKS;
        if npc.local_ai[1] >= 1.0 {
            out.gone = true;
        }
    } else {
        npc.local_ai[1] = (npc.local_ai[1] - 1.0 / 30.0).max(0.0);
    }

    let _ = rng;
    npc.dirty = true;
    out
}

/// Drive one of the Wall's eyes for a tick.
///
/// `wall_at` is the Wall's position and size; `wall_health` is the fraction it has left, which is
/// what sets the eye's cadence.
pub fn eye<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    wall_at: Option<super::skeletron::Parent>,
    wall_health: f32,
) -> Option<Shot> {
    let super::skeletron::Parent {
        position: wall_position,
        size: wall_size,
        direction: wall_direction,
        ..
    } = wall_at?;
    // It rides the Wall's column and faces the way the Wall faces (`NPC.cs:26215-26217`:
    // `position.X = wall.position.X; direction = wall.direction; spriteDirection = direction`).
    // WOF-2: the old code compared the eye's x against the Wall's *after* snapping them equal, so
    // the test was always false and the eye kept its spawn-time direction of +1. On a Wall walking
    // left that left the eye facing away from the player for the whole fight, and the firing gate
    // below (`looking`) never opened, so half of all fights got no eye lasers at all.
    npc.position.0 = wall_position.0;
    npc.direction = wall_direction;
    npc.sprite_direction = npc.direction;

    let middle = wall_position.1 + wall_size.1 / 2.0;
    let want = if npc.ai[0] > 0.0 {
        middle - WALL_MIN_HEIGHT / 4.0
    } else {
        middle + WALL_MIN_HEIGHT / 4.0
    } - npc.height() / 2.0;
    if npc.position.1 > want + 1.0 {
        npc.velocity.1 = -1.0;
    } else if npc.position.1 < want - 1.0 {
        npc.velocity.1 = 1.0;
    } else {
        npc.velocity.1 = 0.0;
        npc.position.1 = want;
    }
    npc.velocity.1 = npc.velocity.1.clamp(-5.0, 5.0);

    // Its volley gets longer and its charge shorter as the Wall dies.
    let mut charge = 1.0;
    let mut volley = WALL_EYE_VOLLEY;
    for (threshold, extra_charge, extra_shots) in
        [(0.75, 1.0, 1), (0.5, 1.0, 1), (0.25, 1.0, 2), (0.1, 2.0, 3)]
    {
        if wall_health < threshold {
            charge += extra_charge;
            volley += extra_shots;
        }
    }
    npc.local_ai[1] += charge;

    let target = world.target?;
    // Only the eye facing the way the Wall is going can see anything to shoot at.
    let looking = (npc.direction > 0 && target.center.0 > npc.center().0)
        || (npc.direction < 0 && target.center.0 < npc.center().0);

    if npc.local_ai[2] == 0.0 {
        if npc.local_ai[1] > WALL_EYE_CHARGE {
            npc.local_ai[2] = 1.0;
            npc.local_ai[1] = 0.0;
            npc.dirty = true;
        }
        return None;
    }
    if npc.local_ai[1] <= WALL_EYE_CADENCE || !crate::game::ai::can_see(world.tiles, npc, target) {
        return None;
    }
    npc.local_ai[1] = 0.0;
    npc.local_ai[2] += 1.0;
    if npc.local_ai[2] >= volley as f32 {
        npc.local_ai[2] = 0.0;
    }
    if !looking {
        return None;
    }

    // The laser gets faster and harder as the Wall weakens, like everything else here.
    let mut speed = WALL_LASER_SPEED;
    let mut damage = WALL_LASER_DAMAGE;
    for (threshold, extra) in [(0.5, 1), (0.25, 1), (0.1, 2)] {
        if wall_health < threshold {
            damage += extra;
            speed += extra as f32;
        }
    }
    let muzzle = (
        npc.position.0 + npc.width() * 0.5,
        npc.position.1 + npc.height() * 0.5,
    );
    let (dx, dy) = (target.center.0 - muzzle.0, target.center.1 - muzzle.1);
    let k = speed / (dx * dx + dy * dy).sqrt().max(0.01);
    npc.dirty = true;
    Some(Shot {
        projectile: WALL_LASER,
        damage,
        position: (muzzle.0 + dx * k, muzzle.1 + dy * k),
        velocity: (dx * k, dy * k),
        time_left: 300,
        ai: [0.0; 3],
    })
}

/// Drive one Hungry for a tick.
///
/// Each hangs at a fixed fraction `ai[0]` down the Wall and strays no further than its leash.
///
/// `slot` is this Hungry's own NPC table slot (`NPC.whoAmI`) — Expert Mode's own leash formula is
/// keyed to it directly, not to the Wall's health at all (`NPC.cs:26406-26430`).
pub fn hungry<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    wall_at: Option<super::skeletron::Parent>,
    wall_health: f32,
    slot: u8,
) -> bool {
    let Some(super::skeletron::Parent {
        position: wall_position,
        size: wall_size,
        velocity: wall_velocity,
        ..
    }) = wall_at
    else {
        return false;
    };
    // Being hit knocks it out of its lunge for a moment.
    if world.was_hurt {
        npc.ai[1] = HUNGRY_RECOIL;
    }

    let expert = world.conditions.expert;

    // Its leash lengthens as the Wall dies, so the safe distance keeps shrinking — and it takes on
    // 30/20 defence doing it. Both are Normal Mode only (`NPC.cs:26372-26400`): Expert Mode leaves
    // the leash at its own formula below and speeds the pull up instead, and its own
    // `if (Main.expertMode) { defense = defDefense; ... }` (`:26406-26408`) runs unconditionally
    // just below and discards the bump regardless of health. Both write the live `defense` field
    // combat actually reads, not the type's own baseline stats — the same bug already fixed in
    // `eye.rs`/`queen_bee.rs`.
    let mut leash = HUNGRY_LEASH;
    let mut accel = HUNGRY_ACCEL;
    if wall_health < 0.5 {
        npc.defense = HUNGRY_DEFENSE_DYING;
        if expert {
            accel += HUNGRY_EXPERT_ACCEL_DYING;
        } else {
            leash = HUNGRY_LEASH_DYING;
        }
    } else if wall_health < 0.75 {
        npc.defense = HUNGRY_DEFENSE_WOUNDED;
        if expert {
            accel += HUNGRY_EXPERT_ACCEL_WOUNDED;
        } else {
            leash = HUNGRY_LEASH_WOUNDED;
        }
    }

    // Expert Mode's own leash: no health-tiered override above, but a multiplier keyed to which of
    // the world's live NPC slots this particular Hungry occupies (`:26406-26430`, `whoAmI % 4` then
    // `whoAmI % 3`, applied in turn, then a flat scale) — and the defence bump above never applies
    // here either, reverting to the type's own baseline instead.
    if expert {
        npc.defense = npc.stats.defense;
        leash *= match slot % 4 {
            0 => 1.75,
            1 => 1.5,
            2 => 1.25,
            _ => 1.0,
        };
        leash *= match slot % 3 {
            0 => 1.5,
            1 => 1.25,
            _ => 1.0,
        };
        leash *= HUNGRY_EXPERT_LEASH_SCALE;
    }

    // Where its tether is anchored: the Wall's column, at its own height along it.
    let anchor = (
        wall_position.0 + wall_size.0 / 2.0,
        wall_position.1 - WALL_MIN_HEIGHT / 2.0 + (wall_size.1 + WALL_MIN_HEIGHT) * npc.ai[0],
    );

    // Every hundred ticks it strains a little further than it should.
    npc.ai[2] += 1.0;
    if npc.ai[2] > 100.0 {
        leash *= 1.3;
        if npc.ai[2] > 200.0 {
            npc.ai[2] = 0.0;
        }
    }

    let Some(target) = world.target else {
        return true;
    };
    let (mut dx, mut dy) = (
        target.center.0 - npc.width() / 2.0 - anchor.0,
        target.center.1 - npc.height() / 2.0 - anchor.1,
    );
    let reach = (dx * dx + dy * dy).sqrt();

    if npc.ai[1] == 0.0 {
        // Within the leash it goes for the player; beyond it, only as far as the leash allows.
        if reach > leash {
            let k = leash / reach.max(0.01);
            dx *= k;
            dy *= k;
        }
        // Accelerating toward that point, and much harder while still moving the wrong way.
        if npc.position.0 < anchor.0 + dx {
            npc.velocity.0 += accel;
            if npc.velocity.0 < 0.0 && dx > 0.0 {
                npc.velocity.0 += accel * 2.5;
            }
        } else if npc.position.0 > anchor.0 + dx {
            npc.velocity.0 -= accel;
            if npc.velocity.0 > 0.0 && dx < 0.0 {
                npc.velocity.0 -= accel * 2.5;
            }
        }
        if npc.position.1 < anchor.1 + dy {
            npc.velocity.1 += accel;
            if npc.velocity.1 < 0.0 && dy > 0.0 {
                npc.velocity.1 += accel * 2.5;
            }
        } else if npc.position.1 > anchor.1 + dy {
            npc.velocity.1 -= accel;
            if npc.velocity.1 > 0.0 && dy < 0.0 {
                npc.velocity.1 -= accel * 2.5;
            }
        }

        // Its top speed, too, is higher in Expert Mode — unconditionally, and more again as the
        // Wall's own health crosses its own four thresholds, plus a further flat bonus while this
        // Hungry trails behind the Wall's own direction of travel, so it can catch back up
        // (`:26488-26520`).
        let mut cap = HUNGRY_SPEED;
        if expert {
            let mut bonus = HUNGRY_EXPERT_SPEED_BASE
                + HUNGRY_EXPERT_SPEED_RAGE
                    .iter()
                    .filter(|(threshold, _)| wall_health < *threshold)
                    .map(|(_, extra)| extra)
                    .sum::<f32>();
            bonus = bonus * HUNGRY_EXPERT_SPEED_SCALE + HUNGRY_EXPERT_SPEED_BONUS;
            cap += bonus * HUNGRY_EXPERT_SPEED_FACTOR;
            let wall_center_x = wall_position.0 + wall_size.0 / 2.0;
            let npc_center_x = npc.center().0;
            if npc_center_x < wall_center_x && wall_velocity.0 > 0.0 {
                cap += HUNGRY_EXPERT_SPEED_CATCHUP;
            }
            if npc_center_x > wall_center_x && wall_velocity.0 < 0.0 {
                cap += HUNGRY_EXPERT_SPEED_CATCHUP;
            }
        }
        npc.velocity.0 = npc.velocity.0.clamp(-cap, cap);
        npc.velocity.1 = npc.velocity.1.clamp(-cap, cap);
    } else {
        npc.ai[1] -= 1.0;
    }

    npc.sprite_direction = if dx > 0.0 { 1 } else { -1 };
    npc.rotation = dy.atan2(dx) + if dx > 0.0 { 0.0 } else { std::f32::consts::PI };
    npc.dirty = true;
    true
}

#[cfg(test)]
mod tests {

    /// A wall standing still at a given place, which is all these tests need of one.
    fn wall_at(position: (f32, f32), size: (f32, f32)) -> crate::game::ai::boss::skeletron::Parent {
        crate::game::ai::boss::skeletron::Parent {
            position,
            size,
            rotation: 0.0,
            scale: 1.0,
            velocity: (0.0, 0.0),
            direction: 1,
            sprite_direction: 1,
            time_left: 3600,
            state: 0.0,
            phase: 0.0,
            health: 1.0,
        }
    }
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use terrustia_proto::tile::Tile;

    struct Hell;

    impl TileView for Hell {
        fn tile(&self, _x: i32, _y: i32) -> Tile {
            Tile::AIR
        }
    }

    /// Solid everywhere from a given tile row on down: a flat floor with open air above it.
    struct Floored(i32);

    impl TileView for Floored {
        fn tile(&self, _x: i32, y: i32) -> Tile {
            if y >= self.0 {
                Tile::block(1)
            } else {
                Tile::AIR
            }
        }
    }

    fn floored_world<'a>(tiles: &'a Floored, target: Option<Target>) -> World<'a, Floored> {
        crate::game::ai::calm(tiles, target)
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(27)
    }

    fn hell<'a>(tiles: &'a Hell, target: Option<Target>) -> World<'a, Hell> {
        crate::game::ai::calm(tiles, target)
    }

    fn player_at(x: f32, y: f32) -> Target {
        Target {
            slot: 0,
            center: (x, y),
            velocity: (0.0, 0.0),
            alive: true,
        }
    }

    fn the_wall() -> Npc {
        Npc::new(113, (10_000.0, 20_000.0), 1).expect("wall of flesh")
    }

    #[test]
    fn it_arrives_with_its_eyes_and_its_hungry() {
        let tiles = Hell;
        let mut w = the_wall();
        let t = Some(player_at(11_000.0, 20_000.0));
        let out = wall(&mut w, &hell(&tiles, t), 0, &mut rng());
        assert_eq!(
            out.spawn.iter().filter(|s| s.npc_type == WALL_EYE).count(),
            2
        );
        assert_eq!(
            out.spawn
                .iter()
                .filter(|s| s.npc_type == WALL_HUNGRY)
                .count(),
            WALL_HUNGRY_COUNT
        );
        assert!(
            out.spawn
                .iter()
                .all(|s| s.parent == Some(Spawn::OWN_PARENT)),
            "and they all belong to it"
        );
    }

    /// WOF-1: each Hungry is raised with its own fractional band in `ai[0]`, spread evenly from
    /// -0.05 to 0.95 (n=0..10), the way vanilla seeds them (`NPC.cs:26197`, `ai0 = num403 * 0.1 -
    /// 0.05`). The
    /// Hungry reader multiplies this straight into its anchor height along the Wall, so the value
    /// has to survive as-is. Packed through the velocity, as it used to be, the consumer's
    /// `signum` would flatten all eleven to +/-1 and stack them at two heights.
    #[test]
    fn its_hungry_carry_their_band_in_ai0_not_a_flattened_sign() {
        let tiles = Hell;
        let mut w = the_wall();
        let t = Some(player_at(11_000.0, 20_000.0));
        let out = wall(&mut w, &hell(&tiles, t), 0, &mut rng());
        let bands: Vec<f32> = out
            .spawn
            .iter()
            .filter(|s| s.npc_type == WALL_HUNGRY)
            .map(|s| s.ai[0].expect("a Hungry's band is pinned outright, not left to signum"))
            .collect();
        let expected: Vec<f32> = (0..WALL_HUNGRY_COUNT)
            .map(|n| n as f32 * 0.1 - 0.05)
            .collect();
        assert_eq!(bands, expected, "eleven distinct bands, -0.05..0.95");
        assert!(
            bands
                .iter()
                .all(|b| bands.iter().filter(|x| *x == b).count() == 1),
            "and every Hungry sits at its own height: {bands:?}"
        );
    }

    /// WOF-5: the Wall measures the shaft each tick and drops to the middle of it, rather than
    /// staying at its spawn height (`NPC.cs:25866-25997`). Spawned below the world, it is pulled up
    /// into the underworld band; the old code left it wherever it was placed.
    #[test]
    fn it_recentres_into_the_underworld_shaft() {
        let tiles = Hell;
        let mut w = the_wall(); // spawns at y = 20_000, below a 1200-tile world
        let start_y = w.position.1;
        let t = Some(player_at(11_000.0, 20_000.0));
        let mut r = rng();
        for _ in 0..5 {
            wall(&mut w, &hell(&tiles, t), 0, &mut r);
        }
        assert!(
            w.position.1 < start_y - 1000.0,
            "should have been pulled up into the underworld, from {start_y} to {}",
            w.position.1
        );
        // The underworld band of a 1200-tile world is tiles 1010..1080 (px 16160..17280).
        let centre = w.position.1 + w.height() / 2.0;
        assert!(
            (16160.0..=17280.0).contains(&centre),
            "and its centre should sit in the underworld band, got {centre}"
        );
    }

    /// WOF-5: the band edges are real terrain, not just the clamp. A solid floor higher in the shaft
    /// leaves less open space below, so the Wall settles higher than it would over a deeper floor.
    #[test]
    fn a_higher_floor_settles_the_wall_higher() {
        let settle = |floor_row: i32| {
            let tiles = Floored(floor_row);
            let mut w = the_wall();
            w.position.1 = 1030.0 * TILE - w.height() / 2.0; // start inside the band
            let t = Some(player_at(11_000.0, w.center().1));
            let mut r = rng();
            for _ in 0..3 {
                wall(&mut w, &floored_world(&tiles, t), 0, &mut r);
            }
            w.position.1 + w.height() / 2.0
        };
        assert!(
            settle(1040) < settle(1078) - 50.0,
            "a floor at tile 1040 should settle the Wall higher than one at 1078: {} vs {}",
            settle(1040),
            settle(1078)
        );
    }

    /// The Wall never turns round. That is the whole fight.
    #[test]
    fn it_advances_in_one_direction_and_never_turns() {
        let tiles = Hell;
        let mut w = the_wall();
        let ahead = Some(player_at(11_000.0, 20_000.0));
        wall(&mut w, &hell(&tiles, ahead), 0, &mut rng());
        assert_eq!(w.direction, 1, "it sets off toward you");

        // Run behind it: it keeps going.
        let behind = Some(player_at(5_000.0, 20_000.0));
        for _ in 0..200 {
            wall(&mut w, &hell(&tiles, behind), 0, &mut rng());
        }
        assert_eq!(w.direction, 1, "and does not follow");
        assert!(w.velocity.0 > 0.0);
    }

    #[test]
    fn it_advances_faster_the_more_it_is_hurt() {
        let tiles = Hell;
        let pace = |life_fraction: f32| {
            let mut w = the_wall();
            w.life = (w.life_max as f32 * life_fraction) as i32;
            let t = Some(player_at(11_000.0, 20_000.0));
            wall(&mut w, &hell(&tiles, t), 0, &mut rng());
            w.velocity.0.abs()
        };
        assert!(pace(0.05) > pace(1.0) * 1.5, "the race gets harder");
    }

    /// Real vanilla (`NPC.cs`, `aiStyle==27`): at low health, `num382 += 0.3` twice more
    /// (66%/33%) and `+= 0.6` three times more (5%/3.5%/2.5%) than Normal mode ever adds, and then
    /// `num382 = num382 * 1.35f + 0.35f` on top of that. Get-fixed-boi's own `* 1.1f + 0.2f` is
    /// separate again and, WOF-3, applied only in a For-the-Worthy world. On the unfixed code
    /// neither `world.conditions.expert` nor these five thresholds were read at all, so Expert and
    /// Normal came out equal; and get-fixed-boi's pair was applied to every world, so an ordinary
    /// world walked at the secret seed's pace.
    #[test]
    fn expert_mode_crosses_five_more_thresholds_and_its_own_final_multiplier() {
        let tiles = Hell;
        let pace = |expert: bool, get_good: bool| {
            let mut w = the_wall();
            // Below every threshold either mode has, so the whole ladder is climbed.
            w.life = (w.life_max as f32 * 0.02) as i32;
            let t = Some(player_at(11_000.0, 20_000.0));
            let mut world = hell(&tiles, t);
            world.conditions.expert = expert;
            world.conditions.get_good_world = get_good;
            wall(&mut w, &world, 0, &mut rng());
            w.velocity.0.abs()
        };
        // An ordinary world: get-fixed-boi's pair is NOT applied. Normal is 1.5 + 1.75 (all four
        // Normal thresholds); Expert is (1.5 + 1.75 + 2.4) * 1.35 + 0.35 (its own five and final
        // multiplier). On the old, unconditional code these carried the extra * 1.1 + 0.2.
        let normal = pace(false, false);
        let expert = pace(true, false);
        assert!(
            (normal - 3.25).abs() < 1e-4,
            "an ordinary world does not get the secret-seed pace, got {normal}"
        );
        assert!(
            (expert - 7.977_5).abs() < 1e-3,
            "Expert Mode's own pace, without the secret seed, got {expert}"
        );
        // A For-the-Worthy world layers get-fixed-boi's `* 1.1 + 0.2` on top of each.
        let normal_good = pace(false, true);
        let expert_good = pace(true, true);
        assert!(
            (normal_good - 3.775).abs() < 1e-4,
            "the secret seed adds its pair, got {normal_good}"
        );
        assert!(
            (expert_good - 8.975_25).abs() < 1e-3,
            "and on top of Expert too, got {expert_good}"
        );
        assert!(
            expert > normal * 2.0,
            "Expert should be dramatically faster, not merely different: {expert} against {normal}"
        );
    }

    #[test]
    fn it_spits_leeches_once_it_has_been_alive_long_enough() {
        let tiles = Hell;
        let mut w = the_wall();
        let t = Some(player_at(11_000.0, 20_000.0));
        let mut r = rng();
        let mut spat = 0;
        for _ in 0..500 {
            spat += wall(&mut w, &hell(&tiles, t), 0, &mut r)
                .spawn
                .iter()
                .filter(|s| s.npc_type == WALL_LEECH)
                .count();
        }
        assert_eq!(spat, 0, "not straight away");

        w.ai[1] = WALL_LEECH_AFTER;
        for _ in 0..500 {
            spat += wall(&mut w, &hell(&tiles, t), 0, &mut r)
                .spawn
                .iter()
                .filter(|s| s.npc_type == WALL_LEECH)
                .count();
        }
        assert!(spat > 0, "but eventually, yes");
    }

    #[test]
    fn it_stops_spitting_once_its_leeches_fill_the_room() {
        let tiles = Hell;
        let mut w = the_wall();
        w.ai[1] = WALL_LEECH_AFTER;
        w.ai[2] = 1.0;
        let t = Some(player_at(11_000.0, 20_000.0));
        let mut r = rng();
        for _ in 0..500 {
            let out = wall(&mut w, &hell(&tiles, t), WALL_LEECH_CAP, &mut r);
            assert!(out.spawn.iter().all(|s| s.npc_type != WALL_LEECH));
        }
    }

    #[test]
    fn everyone_dying_ends_the_fight() {
        let tiles = Hell;
        let mut w = the_wall();
        let dead = Some(Target {
            slot: 0,
            center: (11_000.0, 20_000.0),
            velocity: (0.0, 0.0),
            alive: false,
        });
        let mut r = rng();
        let mut over = false;
        for _ in 0..(WALL_FADE_TICKS as i32 + 5) {
            over |= wall(&mut w, &hell(&tiles, dead), 0, &mut r).gone;
        }
        assert!(over, "and the fight is simply lost");
    }

    fn an_eye(side: f32) -> Npc {
        let mut n = Npc::new(114, (10_000.0, 20_000.0), 1).expect("wall eye");
        n.ai[0] = side;
        n.direction = 1;
        n
    }

    #[test]
    fn an_eye_rides_the_wall() {
        let tiles = Hell;
        let mut e = an_eye(1.0);
        e.position.0 = 9_000.0;
        let at = Some(wall_at((10_000.0, 20_000.0), (16.0, 200.0)));
        let t = Some(player_at(11_000.0, 20_000.0));
        eye(&mut e, &hell(&tiles, t), at, 1.0);
        assert_eq!(e.position.0, 10_000.0, "it is carried, not steered");
    }

    #[test]
    fn the_two_eyes_ride_at_different_heights() {
        let tiles = Hell;
        let at = Some(wall_at((10_000.0, 20_000.0), (16.0, 200.0)));
        let t = Some(player_at(11_000.0, 20_000.0));
        let mut top = an_eye(1.0);
        let mut bottom = an_eye(-1.0);
        for _ in 0..400 {
            eye(&mut top, &hell(&tiles, t), at, 1.0);
            top.position.1 += top.velocity.1;
            eye(&mut bottom, &hell(&tiles, t), at, 1.0);
            bottom.position.1 += bottom.velocity.1;
        }
        assert!(
            top.position.1 < bottom.position.1,
            "one high, one low: {} against {}",
            top.position.1,
            bottom.position.1
        );
    }

    #[test]
    fn an_eye_charges_and_then_fires_a_volley() {
        let tiles = Hell;
        let mut e = an_eye(1.0);
        let at = Some(wall_at((10_000.0, 20_000.0), (16.0, 200.0)));
        let t = Some(player_at(11_000.0, 20_000.0));
        let mut fired = Vec::new();
        for _ in 0..2000 {
            if let Some(shot) = eye(&mut e, &hell(&tiles, t), at, 1.0) {
                fired.push(shot);
            }
        }
        assert!(!fired.is_empty(), "it should have fired");
        assert_eq!(fired[0].projectile, WALL_LASER);
        assert!(fired[0].velocity.0 > 0.0, "and toward the player");
    }

    /// WOF-2: an eye faces the way the Wall faces (`NPC.cs:26216`), so a Wall walking left fires
    /// from its eyes at a player on its left. The old code kept the eye's spawn direction of +1
    /// (its direction test was a no-op) and its firing gate never opened leftward: on the unfixed
    /// code this fires nothing.
    #[test]
    fn an_eye_on_a_leftward_wall_fires_at_a_player_on_its_left() {
        let tiles = Hell;
        let mut left_wall = wall_at((10_000.0, 20_000.0), (16.0, 200.0));
        left_wall.direction = -1;
        let at = Some(left_wall);
        let mut e = an_eye(1.0);
        e.direction = 1; // as spawned, still facing right
        let t = Some(player_at(9_000.0, 20_000.0)); // and the player off to the left
        let mut fired = 0;
        for _ in 0..3000 {
            if eye(&mut e, &hell(&tiles, t), at, 1.0).is_some() {
                fired += 1;
            }
        }
        assert_eq!(e.direction, -1, "the eye takes the Wall's leftward heading");
        assert!(
            fired > 0,
            "a left-walking Wall's eye must fire at a player on its left, got {fired}"
        );
    }

    #[test]
    fn a_dying_wall_makes_its_eyes_fire_harder() {
        let tiles = Hell;
        let at = Some(wall_at((10_000.0, 20_000.0), (16.0, 200.0)));
        let t = Some(player_at(11_000.0, 20_000.0));
        let volley = |health: f32| {
            let mut e = an_eye(1.0);
            let mut n = 0;
            let mut damage = 0;
            for _ in 0..3000 {
                if let Some(shot) = eye(&mut e, &hell(&tiles, t), at, health) {
                    n += 1;
                    damage = shot.damage;
                }
            }
            (n, damage)
        };
        let (healthy, weak_damage) = volley(1.0);
        let (dying, hard_damage) = volley(0.05);
        assert!(dying > healthy, "more shots: {dying} against {healthy}");
        assert!(hard_damage > weak_damage, "and harder ones");
    }

    fn a_hungry() -> Npc {
        let mut n = Npc::new(115, (10_000.0, 20_000.0), 1).expect("the hungry");
        n.ai[0] = 0.5;
        n
    }

    #[test]
    fn a_hungry_without_a_wall_is_finished() {
        let tiles = Hell;
        let mut h = a_hungry();
        assert!(!hungry(&mut h, &hell(&tiles, None), None, 1.0, 0));
    }

    #[test]
    fn a_hungry_lunges_at_you_but_only_so_far() {
        let tiles = Hell;
        let mut h = a_hungry();
        let at = Some(wall_at((10_000.0, 20_000.0), (16.0, 200.0)));
        // Someone way beyond its leash.
        let t = Some(player_at(10_000.0 + HUNGRY_LEASH * 6.0, 20_000.0));
        for _ in 0..600 {
            hungry(&mut h, &hell(&tiles, t), at, 1.0, 0);
            h.position.0 += h.velocity.0;
            h.position.1 += h.velocity.1;
        }
        let strayed = h.position.0 - 10_000.0;
        assert!(strayed > 0.0, "it should come at you");
        assert!(
            strayed < HUNGRY_LEASH * 2.0,
            "but stay on its tether, got {strayed}"
        );
    }

    #[test]
    fn its_leash_lengthens_as_the_wall_dies() {
        let tiles = Hell;
        let strays = |wall_health: f32| {
            let mut h = a_hungry();
            let at = Some(wall_at((10_000.0, 20_000.0), (16.0, 200.0)));
            let t = Some(player_at(10_000.0 + HUNGRY_LEASH * 6.0, 20_000.0));
            let mut furthest: f32 = 0.0;
            for _ in 0..900 {
                hungry(&mut h, &hell(&tiles, t), at, wall_health, 0);
                h.position.0 += h.velocity.0;
                furthest = furthest.max(h.position.0 - 10_000.0);
            }
            furthest
        };
        assert!(
            strays(0.3) > strays(1.0),
            "the safe distance keeps shrinking"
        );
    }

    #[test]
    fn hitting_a_hungry_knocks_it_out_of_its_lunge() {
        let tiles = Hell;
        let mut h = a_hungry();
        let at = Some(wall_at((10_000.0, 20_000.0), (16.0, 200.0)));
        let t = Some(player_at(10_500.0, 20_000.0));
        let mut hit = hell(&tiles, t);
        hit.was_hurt = true;
        hungry(&mut h, &hit, at, 1.0, 0);
        // Set on the hit and immediately begins counting down, so it reads one short.
        assert_eq!(h.ai[1], HUNGRY_RECOIL - 1.0);
        let held = h.velocity;
        hungry(&mut h, &hell(&tiles, t), at, 1.0, 0);
        assert_eq!(h.velocity, held, "it coasts while it recovers");
    }

    /// Real vanilla (`NPC.cs:26372-26408`): `defense = 30`/`20` at the two health thresholds is
    /// written to the live field, but Normal Mode only — Expert Mode's own
    /// `if (Main.expertMode) { defense = defDefense; ... }` runs unconditionally afterward and
    /// discards it, reverting to the type's own baseline regardless of health. The unfixed code
    /// wrote `npc.stats.defense` (never read by combat) regardless of difficulty, so this fails on
    /// it twice over: Normal Mode's `npc.defense` never moved, and Expert Mode got the same 30
    /// Normal Mode does instead of reverting to base.
    #[test]
    fn expert_mode_does_not_apply_the_ordinary_defence_bump() {
        let tiles = Hell;
        let at = Some(wall_at((10_000.0, 20_000.0), (16.0, 200.0)));
        let t = Some(player_at(10_000.0, 20_100.0));
        let base_defense = a_hungry().stats.defense;

        let mut normal = a_hungry();
        let mut world = hell(&tiles, t);
        world.conditions.expert = false;
        hungry(&mut normal, &world, at, 0.3, 0);
        assert_eq!(
            normal.defense, HUNGRY_DEFENSE_DYING,
            "Normal Mode's own 30 bump, on the live field"
        );

        let mut expert = a_hungry();
        world.conditions.expert = true;
        hungry(&mut expert, &world, at, 0.3, 0);
        assert_eq!(
            expert.defense, base_defense,
            "Expert Mode reverts to defDefense, no bump at all"
        );
    }

    /// Real vanilla (`NPC.cs:26406-26430`): Expert Mode's leash is not the 300/500/700
    /// health-tiered range at all — it is `300 * (whoAmI % 4 factor) * (whoAmI % 3 factor) *
    /// 0.75`, keyed to which of the world's live NPC slots this particular Hungry occupies. Slot 0
    /// (`%4==0`, `%3==0`): `300 * 1.75 * 1.5 * 0.75 = 590.625`. Slot 11 (`%4==3`, `%3==2`, neither
    /// multiplier applies): `300 * 1.0 * 1.0 * 0.75 = 225`. The unfixed code had no slot parameter
    /// at all and could not have computed either.
    #[test]
    fn expert_mode_computes_its_leash_from_its_own_npc_slot() {
        let tiles = Hell;
        let at = Some(wall_at((10_000.0, 20_000.0), (16.0, 200.0)));
        let stray = |slot: u8| {
            let mut h = a_hungry();
            let t = Some(player_at(10_000.0 + 2_000.0, 20_100.0));
            let mut world = hell(&tiles, t);
            world.conditions.expert = true;
            let mut furthest: f32 = 0.0;
            for _ in 0..300 {
                hungry(&mut h, &world, at, 1.0, slot);
                h.position.0 += h.velocity.0;
                furthest = furthest.max(h.position.0 - 10_000.0);
            }
            furthest
        };
        let slot0 = stray(0);
        let slot11 = stray(11);
        // A loose comparison rather than a bound on either exact number, like
        // `its_leash_lengthens_as_the_wall_dies` above: the bang-bang pursuit overshoots its
        // target by an amount tied to how fast it was already moving when it got there, so a
        // tighter bound on the wandering distance itself is not reliable — but which of two
        // slots' Hungry gets further, when one's own leash is more than twice the other's, is.
        assert!(
            slot0 > slot11,
            "slot 0's leash (590.625) should let it stray further than slot 11's (225): \
             {slot0} against {slot11}"
        );
    }

    /// Real vanilla (`NPC.cs:26380-26400`): at the same two health thresholds where Normal Mode
    /// lengthens the leash, Expert Mode instead leaves the leash at its own formula and bumps the
    /// acceleration by 0.033/0.066 (`num414 += ...`). One tick, starting from rest, moving toward
    /// a target far to the right: velocity becomes exactly the tick's acceleration.
    #[test]
    fn expert_mode_speeds_up_the_pull_instead_of_lengthening_the_leash() {
        let tiles = Hell;
        let at = Some(wall_at((10_000.0, 20_000.0), (16.0, 200.0)));
        let after_one_tick = |expert: bool, wall_health: f32| {
            let mut h = a_hungry();
            let t = Some(player_at(20_000.0, 20_100.0));
            let mut world = hell(&tiles, t);
            world.conditions.expert = expert;
            hungry(&mut h, &world, at, wall_health, 0);
            h.velocity.0
        };
        assert!(
            (after_one_tick(false, 0.3) - HUNGRY_ACCEL).abs() < 1e-6,
            "Normal Mode's own base accel, unchanged below 50% wall health"
        );
        assert!(
            (after_one_tick(true, 0.3) - (HUNGRY_ACCEL + HUNGRY_EXPERT_ACCEL_DYING)).abs() < 1e-6,
            "Expert Mode's own bump below 50% wall health"
        );
        assert!(
            (after_one_tick(true, 0.6) - (HUNGRY_ACCEL + HUNGRY_EXPERT_ACCEL_WOUNDED)).abs() < 1e-6,
            "and its smaller bump below 75%"
        );
    }

    /// Real vanilla (`NPC.cs:26488-26511`): Expert Mode gives the Hungry's own top speed (4 in
    /// Normal Mode, always) an unconditional bonus — `((1.5*1.25+0.3)*0.35)`, 0.76125 — even at
    /// full wall health, and more again as the wall itself weakens, at its own four thresholds
    /// (75/50/25/10%): at 5% wall health, `(((1.5+0.7+0.7+0.9+0.9)*1.25+0.3)*0.35)`, 2.16125.
    /// Saturated by running far more ticks than it takes to reach top speed, since the clamp pins
    /// velocity to exactly the cap once reached.
    #[test]
    fn expert_mode_raises_the_hungrys_own_top_speed() {
        let tiles = Hell;
        let at = Some(wall_at((10_000.0, 20_000.0), (16.0, 200.0)));
        let saturated = |expert: bool, wall_health: f32| {
            let mut h = a_hungry();
            let t = Some(player_at(1_000_000.0, 20_100.0));
            let mut world = hell(&tiles, t);
            world.conditions.expert = expert;
            for _ in 0..300 {
                hungry(&mut h, &world, at, wall_health, 0);
            }
            h.velocity.0
        };
        assert!(
            (saturated(false, 1.0) - HUNGRY_SPEED).abs() < 1e-4,
            "Normal Mode's own flat cap, unaffected by wall health"
        );
        assert!(
            (saturated(true, 1.0) - 4.761_25).abs() < 1e-3,
            "Expert Mode's own unconditional bonus, even at full wall health"
        );
        assert!(
            (saturated(true, 0.05) - 6.161_25).abs() < 1e-3,
            "and more again as the wall weakens"
        );
    }

    /// Real vanilla (`NPC.cs:26512-26519`): a further flat 6 on top of everything else, only while
    /// this Hungry sits behind the Wall relative to the direction the Wall itself is moving — so
    /// it can catch back up rather than being left behind as the Wall advances.
    #[test]
    fn expert_mode_gives_it_extra_top_speed_while_trailing_the_wall() {
        let tiles = Hell;
        let leading = wall_at((10_000.0, 20_000.0), (16.0, 200.0));
        let mut trailing_wall = leading;
        // Moving right, away from a Hungry riding at its rear.
        trailing_wall.velocity = (5.0, 0.0);
        let cap = |wall: crate::game::ai::boss::skeletron::Parent| {
            let mut h = a_hungry();
            h.position.0 = 9_000.0; // to the Wall's left: behind it, since it is moving right.
            let t = Some(player_at(1_000_000.0, 20_100.0));
            let mut world = hell(&tiles, t);
            world.conditions.expert = true;
            for _ in 0..300 {
                hungry(&mut h, &world, Some(wall), 1.0, 0);
            }
            h.velocity.0
        };
        let without = cap(leading);
        let with = cap(trailing_wall);
        assert!(
            (with - without - HUNGRY_EXPERT_SPEED_CATCHUP).abs() < 1e-3,
            "exactly the flat 6 bonus, {with} against {without}"
        );
    }
}
