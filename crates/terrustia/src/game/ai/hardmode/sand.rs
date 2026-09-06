//! The desert: styles 102 and 103.
//!
//! * A **sand shark** (103) treats sand the way a fish treats water. Inside sand — of any flavour,
//!   including hardened sand and sandstone — it swims freely, bobbing along a shallow sine and
//!   turning when it hits rock. Out of it, it is a helpless thing that falls and flops. The lunge
//!   is what makes one dangerous: when it is submerged, has a clear run, and you are more than a
//!   hundred and fifty pixels away and not already falling, it aims eighty pixels *above* you and
//!   erupts out of the ground at twelve pixels a tick.
//! * A **sand elemental** (102) drifts over terrain rather than through it, and its attack is not
//!   a projectile aimed at you but three sandnadoes raised out of the ground where you are *going*
//!   to be — it leads you by a second of your own movement. It holds perfectly still for the two
//!   and a quarter seconds that takes, which is the whole window to punish it, and it moves faster
//!   the more it is hurt.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::{
    npc_params::{
        ELEMENTAL_ACCEL, ELEMENTAL_CAST_AT, ELEMENTAL_CAST_RANGE, ELEMENTAL_CAST_REST,
        ELEMENTAL_CAST_TICKS, ELEMENTAL_CLIMB_ACCEL, ELEMENTAL_CLIMB_SPEED, ELEMENTAL_DRAG,
        ELEMENTAL_FALL_CAP, ELEMENTAL_GRAVITY, ELEMENTAL_MISS_REST, ELEMENTAL_RISE,
        ELEMENTAL_RISE_CAP, ELEMENTAL_SPEED, ELEMENTAL_STUBBORN_AT, ELEMENTAL_WOUNDED_ACCEL,
        ELEMENTAL_WOUNDED_SPEED, SANDNADO_APART, SANDNADO_LEAD, SANDNADO_SPREAD, SANDNADOES,
        SHARK_BEACHED_SPEED, SHARK_BOB, SHARK_BOB_ACCEL, SHARK_FALL_CAP, SHARK_GRAVITY,
        SHARK_HOME_ACCEL, SHARK_HOME_X, SHARK_HOME_Y, SHARK_LUNGE_ARC, SHARK_LUNGE_COOLDOWN,
        SHARK_LUNGE_RANGE, SHARK_LUNGE_READY, SHARK_LUNGE_SPEED, SHARK_MIN_RANGE, SHARK_SWIM_ACCEL,
        SHARK_SWIM_SPEED, STUCK_TOLERANCE, STUCK_TURN_REST, STUCK_TURN_TICKS,
    },
    projectile::ids::SANDNADO_MARK,
    tile_sets::sandy,
    tile_solid::solid,
};

use super::drifters::Outcome;
use crate::game::ai::{Shot, World, face};
use crate::game::npc::{Npc, TILE, TileView};

/// Whether the tile at a pixel position is sand of some kind.
fn in_sand(tiles: &impl TileView, at: (f32, f32)) -> bool {
    let tile = tiles.tile((at.0 / TILE) as i32, (at.1 / TILE) as i32);
    tile.is_active() && sandy(tile.block)
}

/// Style 103: the sand shark.
pub fn sand_shark(npc: &mut Npc, world: &World<'_, impl TileView>) {
    npc.dirty = true;
    let (cx, cy) = npc.center();
    // Water counts too: a shark that swims into the sea keeps swimming.
    let submerged = in_sand(world.tiles, (cx, cy)) || world.wet;

    let hunting = world.target.is_some_and(|t| {
        t.alive
            && world.target_velocity.1 > -0.1
            && (t.center.0 - cx).hypot(t.center.1 - cy) > SHARK_MIN_RANGE
    });

    if !submerged {
        // Beached. It flops along the ground and falls.
        if npc.velocity.1 == 0.0 {
            if hunting && let Some(target) = world.target {
                face(npc, target);
            }
            npc.velocity.0 += f32::from(npc.direction) * SHARK_SWIM_ACCEL;
            if npc.velocity.0.abs() > SHARK_BEACHED_SPEED {
                npc.velocity.0 *= 0.95;
            }
        }
        npc.velocity.1 = (npc.velocity.1 + SHARK_GRAVITY).min(SHARK_FALL_CAP);
        npc.ai[0] = 1.0;
        aim_nose(npc);
        return;
    }

    // Submerged. `ai[1]` remembers whether there is still sand under it, and `ai[2]` is the lunge
    // cooldown, which runs negative while it is recovering.
    let under = in_sand(world.tiles, (cx, cy + 24.0 - 2.0 * TILE));
    npc.ai[1] = if under { 1.0 } else { 0.0 };
    if npc.ai[2] < SHARK_LUNGE_READY {
        npc.ai[2] += 1.0;
    }

    if hunting {
        if let Some(target) = world.target {
            face(npc, target);
        }
        npc.velocity.0 = (npc.velocity.0 + f32::from(npc.direction) * SHARK_HOME_ACCEL)
            .clamp(-SHARK_HOME_X, SHARK_HOME_X);
        npc.velocity.1 = (npc.velocity.1 + f32::from(npc.direction_y) * SHARK_HOME_ACCEL)
            .clamp(-SHARK_HOME_Y, SHARK_HOME_Y);

        // What is directly in front of its nose, one body-length ahead.
        let speed = npc.velocity.0.hypot(npc.velocity.1);
        let reach = npc.width().hypot(npc.height()) / 2.0;
        let ahead = if speed > 0.0 {
            (
                cx + npc.velocity.0 / speed * reach + npc.velocity.0,
                cy + npc.velocity.1 / speed * reach + npc.velocity.1,
            )
        } else {
            (cx, cy)
        };
        let still_buried = in_sand(world.tiles, ahead)
            || (world.wet
                && world
                    .tiles
                    .tile((ahead.0 / TILE) as i32, (ahead.1 / TILE) as i32)
                    .liquid
                    > 0);

        // About to break the surface, going the way it is facing, with you in range and the
        // cooldown clear: that is the lunge.
        let in_range = world
            .target
            .is_some_and(|t| (t.center.0 - cx).hypot(t.center.1 - cy) < SHARK_LUNGE_RANGE);
        if !still_buried
            && npc.velocity.0.signum() as i8 == npc.direction
            && in_range
            && (npc.ai[2] >= SHARK_LUNGE_READY || npc.ai[2] < 0.0)
            && let Some(target) = world.target
        {
            npc.ai[2] = SHARK_LUNGE_COOLDOWN;
            // It aims above you, so it arcs over rather than into you.
            let (ax, ay) = (target.center.0 - cx, target.center.1 + SHARK_LUNGE_ARC - cy);
            let length = ax.hypot(ay).max(f32::MIN_POSITIVE);
            npc.velocity = (
                ax / length * SHARK_LUNGE_SPEED,
                ay / length * SHARK_LUNGE_SPEED,
            );
        }
    } else {
        // Patrolling: it bounces off rock and rides a slow sine so it weaves through the dune.
        if npc.collide_x {
            npc.velocity.0 *= -1.0;
            npc.direction = -npc.direction;
        }
        if npc.collide_y {
            npc.velocity.1 *= -1.0;
            npc.direction_y = npc.velocity.1.signum() as i8;
            npc.ai[0] = f32::from(npc.direction_y);
        }
        npc.velocity.0 += f32::from(npc.direction) * SHARK_SWIM_ACCEL;
        if npc.velocity.0.abs() > SHARK_SWIM_SPEED {
            npc.velocity.0 *= 0.95;
        }
        // Sand below means dive, no sand below means rise: it follows the dune's shape.
        npc.ai[0] = if under { -1.0 } else { 1.0 };
        if npc.ai[0] == -1.0 {
            npc.velocity.1 -= SHARK_BOB_ACCEL;
            if npc.velocity.1 < -SHARK_BOB {
                npc.ai[0] = 1.0;
            }
        } else {
            npc.velocity.1 += SHARK_BOB_ACCEL;
            if npc.velocity.1 > SHARK_BOB {
                npc.ai[0] = -1.0;
            }
        }
        if npc.velocity.1.abs() > 0.4 {
            npc.velocity.1 *= 0.95;
        }
    }
    aim_nose(npc);
}

/// The nose follows the climb, but only so far: a shark never points straight up.
fn aim_nose(npc: &mut Npc) {
    npc.rotation = (npc.velocity.1 * f32::from(npc.direction) * 0.1).clamp(-0.2, 0.2);
}

/// Style 102: the sand elemental.
pub fn sand_elemental(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    rng: &mut SmallRng,
) -> Outcome {
    let mut out = Outcome::default();
    npc.dirty = true;

    let health = npc.life as f32 / npc.life_max.max(1) as f32;
    let speed = ELEMENTAL_SPEED + (1.0 - health) * ELEMENTAL_WOUNDED_SPEED;
    let accel = ELEMENTAL_ACCEL + (1.0 - health) * ELEMENTAL_WOUNDED_ACCEL;
    // Past half health nothing shifts it.
    npc.knockback_immune = health < ELEMENTAL_STUBBORN_AT;
    npc.rotation = npc.velocity.0 * 0.04;
    npc.sprite_direction = if npc.direction > 0 { 1 } else { -1 };

    // The cast. `ai[0]` runs up through the cast and then deeply negative while it recovers.
    let mut casting = false;
    if npc.ai[0] < 0.0 {
        npc.ai[0] = (npc.ai[0] + 1.0).min(0.0);
    }
    if npc.ai[0] > 0.0 {
        casting = true;
        npc.ai[0] += 1.0;
        if npc.ai[0] >= ELEMENTAL_CAST_TICKS {
            npc.ai[0] = ELEMENTAL_CAST_REST;
        }
        if npc.ai[0] == ELEMENTAL_CAST_AT {
            match world.target.filter(|t| t.alive) {
                Some(target) => {
                    // Raised where you are *going*, not where you are.
                    let lead = (
                        target.center.0 + world.target_velocity.0 * SANDNADO_LEAD,
                        target.center.1,
                    );
                    let (cx, cy) = npc.center();
                    if (lead.0 - cx).hypot(lead.1 - cy) < ELEMENTAL_CAST_RANGE {
                        out.shots.extend(raise_sandnadoes(world, lead, rng));
                    } else {
                        // Too far to bother; it cuts the cast short.
                        npc.ai[0] = ELEMENTAL_MISS_REST;
                    }
                }
                None => npc.ai[0] = ELEMENTAL_MISS_REST,
            }
        }
    }
    if npc.ai[0] == 0.0 {
        npc.ai[0] = 1.0;
        casting = true;
    }

    // Stuck detection: with no ground under it there is nothing else to tell it it is blocked.
    if world.was_hurt {
        npc.local_ai[2] = 0.0;
    }
    if npc.local_ai[2] >= 0.0 {
        let mut tolerance = STUCK_TOLERANCE;
        let same_x = if (npc.local_ai[0] - tolerance..npc.local_ai[0] + tolerance)
            .contains(&npc.position.0)
        {
            true
        } else if (npc.velocity.0 < 0.0 && npc.direction > 0)
            || (npc.velocity.0 > 0.0 && npc.direction < 0)
        {
            // Being pushed the wrong way counts as stuck too, with a looser box.
            tolerance += 24.0;
            true
        } else {
            false
        };
        let same_y =
            (npc.local_ai[1] - tolerance..npc.local_ai[1] + tolerance).contains(&npc.position.1);
        if same_x && same_y {
            npc.local_ai[2] += 1.0;
            if npc.local_ai[2] >= STUCK_TURN_TICKS {
                npc.local_ai[2] = STUCK_TURN_REST;
                npc.direction = -npc.direction;
                npc.velocity.0 *= -1.0;
                npc.collide_x = false;
            }
        } else {
            npc.local_ai[0] = npc.position.0;
            npc.local_ai[1] = npc.position.1;
            npc.local_ai[2] = 0.0;
        }
        if !casting && let Some(target) = world.target {
            face(npc, target);
        }
    } else {
        // Recovering from a turn: it will not re-target, but it still faces the right way.
        npc.local_ai[2] += 1.0;
        if let Some(target) = world.target {
            npc.direction = if target.center.0 > npc.center().0 {
                1
            } else {
                -1
            };
        }
    }

    if casting {
        npc.velocity.0 *= ELEMENTAL_DRAG;
        npc.velocity.1 *= ELEMENTAL_DRAG;
        return out;
    }

    // Terrain sense: it looks ahead-and-down for something to clear, and below itself for a floor.
    let ahead_x =
        ((npc.position.0 + npc.width() / 2.0) / TILE) as i32 + i32::from(npc.direction) * 2;
    let feet_y = ((npc.position.1 + npc.height()) / TILE) as i32;
    let blocked = |x: i32, y: i32| {
        let tile = world.tiles.tile(x, y);
        (tile.is_active() && solid(tile.block)) || tile.liquid > 0
    };

    let mut clear_ahead = true;
    let mut needs_lift = false;
    for depth in 0..4 {
        if blocked(ahead_x, feet_y + depth) {
            if depth <= 1 {
                needs_lift = true;
            }
            clear_ahead = false;
            break;
        }
    }
    let bottom_x = ((npc.position.0 + npc.width() / 2.0) / TILE) as i32;
    for depth in 0..3 {
        if blocked(bottom_x, feet_y + depth) {
            needs_lift = true;
            clear_ahead = false;
            break;
        }
    }

    if clear_ahead {
        npc.velocity.1 = (npc.velocity.1 + ELEMENTAL_GRAVITY).min(ELEMENTAL_FALL_CAP);
    } else {
        if (npc.direction_y < 0 && npc.velocity.1 > 0.0) || needs_lift {
            npc.velocity.1 += ELEMENTAL_RISE;
        }
        npc.velocity.1 = npc.velocity.1.max(ELEMENTAL_RISE_CAP);
    }

    if npc.collide_x {
        npc.velocity.0 = npc.old_velocity.0 * -0.4;
        if npc.direction == -1 && (0.0..1.0).contains(&npc.velocity.0) {
            npc.velocity.0 = 1.0;
        }
        if npc.direction == 1 && (-1.0..0.0).contains(&npc.velocity.0) {
            npc.velocity.0 = -1.0;
        }
    }
    if npc.collide_y {
        npc.velocity.1 = npc.old_velocity.1 * -0.25;
        if (0.0..1.0).contains(&npc.velocity.1) {
            npc.velocity.1 = 1.0;
        }
        if (-1.0..0.0).contains(&npc.velocity.1) {
            npc.velocity.1 = -1.0;
        }
    }

    drift_axis(&mut npc.velocity.0, npc.direction, speed, accel);
    drift_axis(
        &mut npc.velocity.1,
        npc.direction_y,
        ELEMENTAL_CLIMB_SPEED,
        ELEMENTAL_CLIMB_ACCEL,
    );
    out
}

/// Ease one axis toward `direction`, pushing harder while still travelling the wrong way and
/// easing off once it is nearly there.
fn drift_axis(velocity: &mut f32, direction: i8, cap: f32, accel: f32) {
    if direction == -1 && *velocity > -cap {
        *velocity -= accel;
        if *velocity > cap {
            *velocity -= accel;
        } else if *velocity > 0.0 {
            *velocity += accel / 2.0;
        }
        *velocity = velocity.max(-cap);
    } else if direction == 1 && *velocity < cap {
        *velocity += accel;
        if *velocity < -cap {
            *velocity += accel;
        } else if *velocity < 0.0 {
            *velocity -= accel / 2.0;
        }
        *velocity = velocity.min(cap);
    }
}

/// Pick up to three well-separated columns near `at` and raise a sandnado in each.
///
/// Each one has to stand on ground with room above it, and no two may be within ten tiles, so a
/// cast in a cramped place produces fewer than three rather than a stack of them.
fn raise_sandnadoes(
    world: &World<'_, impl TileView>,
    at: (f32, f32),
    rng: &mut SmallRng,
) -> Vec<Shot> {
    let (tile_x, tile_y) = ((at.0 / TILE) as i32, (at.1 / TILE) as i32);
    let mut chosen: Vec<i32> = Vec::new();
    let mut shots = Vec::new();

    for _ in 0..1000 {
        if chosen.len() >= SANDNADOES {
            break;
        }
        let x = rng.random_range(tile_x - SANDNADO_SPREAD..=tile_x + SANDNADO_SPREAD);
        if chosen.iter().any(|c| (c - x).abs() < SANDNADO_APART) {
            continue;
        }
        // Fall from twenty tiles up until something solid stops us.
        let Some(floor) = (tile_y - 20..).take(51).find(|&y| {
            let tile = world.tiles.tile(x, y);
            tile.is_active() && solid(tile.block)
        }) else {
            continue;
        };
        chosen.push(x);
        shots.push(Shot {
            projectile: SANDNADO_MARK,
            damage: 0,
            position: (x as f32 * TILE, (floor - 15) as f32 * TILE),
            velocity: (0.0, 0.0),
            // Its own arm ends it at 120, having spawned the tornado at 60.
            time_left: 0,
            ai: [0.0; 3],
        });
    }
    shots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    struct Desert(HashMap<(i32, i32), Tile>);

    impl TileView for Desert {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    /// A dune: sand from `top` down, across the whole test area.
    fn dune(top: i32) -> Desert {
        let mut tiles = HashMap::new();
        for x in -200..200 {
            for y in top..top + 40 {
                tiles.insert((x, y), Tile::block(53));
            }
        }
        Desert(tiles)
    }

    fn world<'a>(tiles: &'a Desert, target: Option<(f32, f32)>) -> World<'a, Desert> {
        crate::game::ai::calm(
            tiles,
            target.map(|center| Target {
                slot: 0,
                center,
                velocity: (0.0, 0.0),
                alive: true,
            }),
        )
    }

    const SAND_SHARK: u16 = 542;
    const SAND_ELEMENTAL: u16 = 541;

    fn shark(tile_x: i32, tile_y: i32) -> Npc {
        Npc::new(SAND_SHARK, (tile_x as f32 * TILE, tile_y as f32 * TILE), 1).expect("sand shark")
    }

    /// In sand it swims; out of it, it falls. That difference is the entire enemy.
    #[test]
    fn a_shark_swims_in_sand_and_falls_out_of_it() {
        let tiles = dune(40);
        let w = world(&tiles, Some((10_000.0, 0.0)));

        let mut buried = shark(0, 50);
        sand_shark(&mut buried, &w);
        assert!(
            buried.velocity.1.abs() < SHARK_GRAVITY,
            "in sand it should not be falling, got {}",
            buried.velocity.1
        );

        let mut beached = shark(0, 10);
        sand_shark(&mut beached, &w);
        assert_eq!(beached.velocity.1, SHARK_GRAVITY, "in the air it falls");
    }

    /// The lunge is what makes one dangerous: it erupts out of the dune toward you.
    #[test]
    fn a_shark_lunges_out_of_the_dune() {
        // Sand only below the surface, so the shark reaches open air as it rises.
        let tiles = dune(40);
        let mut s = shark(0, 42);
        s.direction = 1;
        s.direction_y = -1;
        s.ai[2] = SHARK_LUNGE_READY;
        // A player standing on the dune, off to the right, not falling.
        let w = world(&tiles, Some((10.0 * TILE, 39.0 * TILE)));

        let mut launched = false;
        for _ in 0..400 {
            sand_shark(&mut s, &w);
            if s.velocity.0.hypot(s.velocity.1) > SHARK_LUNGE_SPEED - 0.5 {
                launched = true;
                break;
            }
            crate::game::npc::step_physics(&mut s, &tiles);
        }
        assert!(launched, "it should have broken the surface at speed");
        assert!(s.velocity.1 < 0.0, "and come up, not down");
    }

    /// A player falling onto a shark is not something it lunges at — it waits.
    #[test]
    fn a_shark_does_not_lunge_at_someone_dropping_on_it() {
        let tiles = dune(40);
        let mut s = shark(0, 42);
        s.ai[2] = SHARK_LUNGE_READY;
        let mut w = world(&tiles, Some((10.0 * TILE, 39.0 * TILE)));
        w.target_velocity = (0.0, -5.0);
        for _ in 0..200 {
            sand_shark(&mut s, &w);
            assert!(
                s.velocity.0.hypot(s.velocity.1) < SHARK_LUNGE_SPEED - 0.5,
                "it should not have lunged"
            );
        }
    }

    /// The elemental holds still while it casts — that is the window.
    #[test]
    fn an_elemental_stops_dead_while_it_casts() {
        let tiles = dune(60);
        let mut rng = SmallRng::seed_from_u64(102);
        let mut e = Npc::new(SAND_ELEMENTAL, (0.0, 40.0 * TILE), 1).expect("sand elemental");
        e.velocity = (3.0, 2.0);
        let w = world(&tiles, Some((20.0 * TILE, 40.0 * TILE)));

        sand_elemental(&mut e, &w, &mut rng);
        assert_eq!(e.ai[0], 1.0, "it should have started casting");
        let before = e.velocity.0.hypot(e.velocity.1);
        for _ in 0..20 {
            sand_elemental(&mut e, &w, &mut rng);
        }
        let after = e.velocity.0.hypot(e.velocity.1);
        assert!(after < before * 0.9, "it should be slowing to a stop");
    }

    /// The cast raises sandnadoes out of the ground ahead of the player, spaced apart.
    #[test]
    fn a_cast_raises_sandnadoes_where_you_are_heading() {
        let tiles = dune(60);
        let mut rng = SmallRng::seed_from_u64(3);
        let mut e = Npc::new(SAND_ELEMENTAL, (0.0, 40.0 * TILE), 1).expect("sand elemental");
        let mut w = world(&tiles, Some((20.0 * TILE, 55.0 * TILE)));
        // Running to the right, so the trap should be laid to the right.
        w.target_velocity = (6.0, 0.0);

        let mut raised = Vec::new();
        for _ in 0..(ELEMENTAL_CAST_TICKS as i32 + 5) {
            raised.extend(sand_elemental(&mut e, &w, &mut rng).shots);
        }
        assert!(!raised.is_empty(), "the cast should have raised something");
        assert!(raised.len() <= SANDNADOES, "and no more than three");
        assert!(
            raised.iter().all(|s| s.projectile == SANDNADO_MARK),
            "sandnadoes, not anything else"
        );
        // Spaced out rather than stacked.
        for (i, a) in raised.iter().enumerate() {
            for b in &raised[i + 1..] {
                assert!(
                    (a.position.0 - b.position.0).abs() >= SANDNADO_APART as f32 * TILE,
                    "two sandnadoes too close together"
                );
            }
        }
        // Laid ahead of the player, because it led them.
        let mean = raised.iter().map(|s| s.position.0).sum::<f32>() / raised.len() as f32;
        assert!(
            mean > 20.0 * TILE - SANDNADO_SPREAD as f32 * TILE,
            "the trap should be near where the player is heading, got {mean}"
        );
    }

    /// A wounded elemental is a faster one, and past half health nothing shoves it.
    #[test]
    fn a_wounded_elemental_is_quicker_and_immovable() {
        let tiles = dune(60);
        let mut rng = SmallRng::seed_from_u64(9);
        let mut hale = Npc::new(SAND_ELEMENTAL, (0.0, 40.0 * TILE), 1).unwrap();
        let mut hurt = Npc::new(SAND_ELEMENTAL, (0.0, 40.0 * TILE), 1).unwrap();
        hurt.life = hurt.life_max / 10;
        // Past the cast, so both are actually moving.
        hale.ai[0] = ELEMENTAL_CAST_REST;
        hurt.ai[0] = ELEMENTAL_CAST_REST;
        let w = world(&tiles, Some((60.0 * TILE, 40.0 * TILE)));

        // They have to actually travel, or the stuck detector turns them both round and the
        // comparison is between two arbitrary points in a reversal.
        for _ in 0..120 {
            sand_elemental(&mut hale, &w, &mut rng);
            sand_elemental(&mut hurt, &w, &mut rng);
            crate::game::npc::step_physics(&mut hale, &tiles);
            crate::game::npc::step_physics(&mut hurt, &tiles);
        }
        assert!(
            hurt.velocity.0.abs() > hale.velocity.0.abs(),
            "hurt should be faster: {} vs {}",
            hurt.velocity.0,
            hale.velocity.0
        );
        assert!(hurt.knockback_immune, "and past half health, immovable");
        assert!(!hale.knockback_immune);
    }
}
