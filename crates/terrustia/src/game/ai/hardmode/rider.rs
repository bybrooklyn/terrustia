//! Riders: style 75 — the parts that sit on something else.
//!
//! A rider has no movement of its own at all. Every tick it is placed at a fixed offset from its
//! mount's centre, *rotated by the mount's rotation and scaled by its scale*, so a saucer's guns
//! swing round with the hull rather than hanging off it in world coordinates. It borrows the
//! mount's facing, its velocity and its despawn timer, so an assembly moves and leaves as one
//! thing.
//!
//! What a rider does with its turn varies. A scutlix rider shoots on a one-second reload that
//! being hit sets back; a Dutchman's cannon lobs a ball every four seconds; the saucer's top plate
//! does nothing but sit there and, once all four guns are gone, tell the hull the fight has
//! changed. And a rider whose mount is gone is gone with it — that is what makes killing the mount
//! worthwhile.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    CANNON_RELOAD, CANNON_SHOT_DAMAGE, CANNON_SHOT_RISE, CANNON_SHOT_SPEED, DUTCHMAN_GUN,
    DUTCHMAN_GUN_SPACING, RIDER_FLINCH, RIDER_RANGE, RIDER_RELOAD, RIDER_SHOT_DAMAGE,
    RIDER_SHOT_SPEED, RIDER_SPREAD, SCUTLIX_RIDER, seat,
};
use terrustia_proto::projectile::ids::{CANNON_SHOT, RIDER_SHOT};

use super::drifters::Outcome;
use crate::game::ai::{Shot, World, boss::skeletron::Parent, can_see};
use crate::game::npc::{Npc, TileView};

/// Style 75.
///
/// `mount` is what it is riding on, or `None` when that is gone.
pub fn rider(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    mount: Option<Parent>,
    rng: &mut SmallRng,
) -> Outcome {
    let mut out = Outcome::default();
    let (Some(seat), Some(mount)) = (seat(npc.npc_type), mount) else {
        // No mount, or nothing to ride: it does not survive on its own.
        out.spent = true;
        return out;
    };
    npc.dirty = true;

    // Which of a mirrored pair this is. `ai[1]` is the index the mount gave it.
    let side = if npc.ai[1] >= 1.0 { 1.0 } else { -1.0 };
    let mut offset = (seat.offset.0 + seat.side_offset * side, seat.offset.1);
    if npc.npc_type == DUTCHMAN_GUN {
        // The Dutchman's guns are spaced along the hull rather than mirrored, and they hang off
        // whichever way the hull is facing.
        offset.0 = (seat.offset.0 + DUTCHMAN_GUN_SPACING * npc.ai[1])
            * if mount.sprite_direction == 1 {
                -1.0
            } else {
                1.0
            };
    }
    // Scaled by the mount, then turned with it.
    offset = (offset.0 * mount.scale, offset.1 * mount.scale);
    let (sin, cos) = mount.rotation.sin_cos();
    let turned = (
        offset.0 * cos - offset.1 * sin,
        offset.0 * sin + offset.1 * cos,
    );

    let (mx, my) = mount.center();
    npc.velocity = mount.velocity;
    npc.position = (
        mx - npc.width() / 2.0 + turned.0,
        my - npc.height() / 2.0 + turned.1,
    );
    npc.rotation = mount.rotation;
    npc.direction = mount.direction;
    npc.sprite_direction = if seat.faces_outward {
        side as i8
    } else {
        mount.sprite_direction
    };
    // It leaves when its mount does, rather than lingering where the mount used to be.
    npc.time_left = mount.time_left;

    let Some(target) = world.target.filter(|t| t.alive) else {
        return out;
    };
    let (cx, cy) = npc.center();
    let to_player = (target.center.0 - cx, target.center.1 - cy);

    if npc.npc_type == SCUTLIX_RIDER {
        // A one-second reload that being hit knocks back half a second.
        if npc.ai[1] < RIDER_RELOAD {
            npc.ai[1] += 1.0;
        }
        if world.was_hurt {
            npc.ai[1] = RIDER_FLINCH;
        }
        if !can_see(world.tiles, npc, target) {
            return out;
        }
        let reach = to_player.0.hypot(to_player.1);
        if reach >= RIDER_RANGE {
            return out;
        }
        // It only fires the way it is already facing, so a rider behind you has to wait.
        if npc.ai[1] == RIDER_RELOAD && to_player.0.signum() as i8 == npc.direction {
            npc.ai[1] = -RIDER_RELOAD;
            let from = (cx, cy - 4.0);
            let mut aim = (target.center.0 - from.0, target.center.1 - from.1);
            aim.0 += rng.random_range(-RIDER_SPREAD..=RIDER_SPREAD) as f32;
            aim.1 += rng.random_range(-RIDER_SPREAD..=RIDER_SPREAD) as f32;
            aim.0 *= rng.random_range(80..=120) as f32 * 0.01;
            aim.1 *= rng.random_range(80..=120) as f32 * 0.01;
            out.shots.push(Shot {
                projectile: RIDER_SHOT,
                damage: RIDER_SHOT_DAMAGE,
                position: from,
                velocity: aimed(aim, RIDER_SHOT_SPEED),
                time_left: 300,
                ai: [0.0; 3],
            });
        }
        return out;
    }

    if npc.npc_type == DUTCHMAN_GUN {
        if npc.ai[3] < CANNON_RELOAD {
            npc.ai[3] += 1.0;
        }
        if !can_see(world.tiles, npc, target) {
            npc.ai[2] = 0.0;
            return out;
        }
        if npc.ai[3] >= CANNON_RELOAD {
            npc.ai[3] = 0.0;
            // Aimed at you and then lifted, so the ball arcs.
            let mut shot = aimed(to_player, CANNON_SHOT_SPEED);
            shot.1 += CANNON_SHOT_RISE;
            out.shots.push(Shot {
                projectile: CANNON_SHOT,
                damage: CANNON_SHOT_DAMAGE,
                position: (cx, cy),
                velocity: shot,
                time_left: 600,
                ai: [0.0; 3],
            });
        } else {
            // Between shots it tracks you, in eight steps: `ai[2]` is which way it is pointing.
            npc.ai[2] = (facing_step(to_player, npc.sprite_direction)) as f32;
        }
    }
    out
}

/// A vector of length `speed` along `v`, falling back to straight down when `v` has no direction —
/// which is what the game does rather than producing a NaN.
fn aimed(v: (f32, f32), speed: f32) -> (f32, f32) {
    let length = v.0.hypot(v.1);
    if length <= 0.0 || !length.is_finite() {
        (0.0, speed)
    } else {
        (v.0 / length * speed, v.1 / length * speed)
    }
}

/// Which of eight directions points most nearly at `to_player`, numbered one to eight.
///
/// The game works this out by putting a point fifty pixels out in each of eight directions and
/// taking whichever lands closest to the player, which is the same answer as an angle but reached
/// the way the original reached it.
fn facing_step(to_player: (f32, f32), sprite_direction: i8) -> i32 {
    let mut best = 0;
    let mut best_gap = f32::MAX;
    for step in 0..8 {
        let angle = -(step as f32) * std::f32::consts::FRAC_PI_4;
        // Straight down, rotated.
        let (sin, cos) = angle.sin_cos();
        let probe = (-sin * 50.0, cos * 50.0);
        let gap = (probe.0 - to_player.0).hypot(probe.1 - to_player.1);
        if gap < best_gap {
            best_gap = gap;
            best = step;
        }
    }
    let facing = best + 1;
    if sprite_direction == 1 {
        9 - facing
    } else {
        facing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    struct Sky(HashMap<(i32, i32), Tile>);

    impl TileView for Sky {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn world<'a>(tiles: &'a Sky, target: Option<(f32, f32)>) -> World<'a, Sky> {
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

    fn mount_at(position: (f32, f32), rotation: f32) -> Parent {
        Parent {
            position,
            size: (100.0, 100.0),
            rotation,
            scale: 1.0,
            velocity: (2.0, -1.0),
            direction: 1,
            sprite_direction: 1,
            time_left: 1234,
            state: 0.0,
            phase: 0.0,
            health: 1.0,
        }
    }

    /// A rider with nothing to ride does not survive.
    #[test]
    fn a_rider_without_a_mount_is_gone() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(75);
        let mut r = Npc::new(SCUTLIX_RIDER, (0.0, 0.0), 1).expect("scutlix rider");
        assert!(rider(&mut r, &world(&tiles, None), None, &mut rng).spent);
    }

    /// It sits where the mount puts it, borrows its motion, and leaves when it leaves.
    #[test]
    fn a_rider_is_placed_by_its_mount() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(1);
        let mut r = Npc::new(SCUTLIX_RIDER, (0.0, 0.0), 1).expect("scutlix rider");
        let mount = mount_at((5000.0, 3000.0), 0.0);

        rider(&mut r, &world(&tiles, None), Some(mount), &mut rng);
        let (mx, my) = mount.center();
        let (cx, cy) = r.center();
        assert!(
            (cx - mx).abs() < 0.01,
            "level with the mount, got {cx} vs {mx}"
        );
        assert!(
            (cy - (my - 14.0)).abs() < 0.01,
            "and fourteen pixels above it, got {cy} vs {}",
            my - 14.0
        );
        assert_eq!(r.velocity, mount.velocity, "it moves with the mount");
        assert_eq!(r.time_left, mount.time_left, "and leaves with it");
    }

    /// Rotating the mount swings its parts round rather than sliding them.
    #[test]
    fn a_turning_mount_carries_its_parts_round_with_it() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(2);
        let mut r = Npc::new(SCUTLIX_RIDER, (0.0, 0.0), 1).expect("scutlix rider");
        let upright = mount_at((5000.0, 3000.0), 0.0);
        let turned = mount_at((5000.0, 3000.0), std::f32::consts::FRAC_PI_2);

        rider(&mut r, &world(&tiles, None), Some(upright), &mut rng);
        let above = r.center();
        rider(&mut r, &world(&tiles, None), Some(turned), &mut rng);
        let beside = r.center();

        let (mx, my) = upright.center();
        assert!(above.1 < my, "upright, it sits above");
        // A quarter turn puts the same offset out to one side instead.
        assert!(
            (beside.1 - my).abs() < 1.0,
            "turned, it should be level, got {}",
            beside.1
        );
        assert!(
            (beside.0 - mx).abs() > 10.0,
            "and off to the side, got {}",
            beside.0 - mx
        );
        assert_eq!(r.rotation, turned.rotation, "and turned with it");
    }

    /// The rider shoots on a reload, and being hit sets it back.
    #[test]
    fn hitting_a_rider_delays_its_shot() {
        let tiles = Sky(HashMap::new());
        let mount = mount_at((5000.0, 3000.0), 0.0);
        let (mx, my) = mount.center();

        let shots_over = |harried: bool| {
            let mut rng = SmallRng::seed_from_u64(9);
            let mut r = Npc::new(SCUTLIX_RIDER, (0.0, 0.0), 1).unwrap();
            let mut w = world(&tiles, Some((mx + 300.0, my)));
            w.was_hurt = harried;
            (0..600)
                .map(|_| rider(&mut r, &w, Some(mount), &mut rng).shots.len())
                .sum::<usize>()
        };
        let calm = shots_over(false);
        assert!(calm > 0, "it should get shots off");
        assert!(
            shots_over(true) < calm,
            "being hit should hold it off: {} vs {calm}",
            shots_over(true)
        );
    }

    /// A rider does not fire behind itself.
    #[test]
    fn a_rider_only_shoots_the_way_it_faces() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(4);
        let mount = mount_at((5000.0, 3000.0), 0.0);
        let (mx, my) = mount.center();
        // The mount faces right; put the player well off to the left.
        let mut r = Npc::new(SCUTLIX_RIDER, (0.0, 0.0), 1).unwrap();
        let w = world(&tiles, Some((mx - 300.0, my)));
        let fired: usize = (0..600)
            .map(|_| rider(&mut r, &w, Some(mount), &mut rng).shots.len())
            .sum();
        assert_eq!(fired, 0, "it should be waiting for a turn");
    }

    /// A Dutchman's gun lobs its ball on a four-second cycle.
    #[test]
    fn a_dutchman_gun_lobs_on_its_own_cycle() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(6);
        let mount = mount_at((5000.0, 3000.0), 0.0);
        let (mx, my) = mount.center();
        let mut g = Npc::new(DUTCHMAN_GUN, (0.0, 0.0), 1).expect("pirate ship cannon");
        let w = world(&tiles, Some((mx + 200.0, my + 200.0)));

        let mut fired = Vec::new();
        for tick in 0..1200 {
            if !rider(&mut g, &w, Some(mount), &mut rng).shots.is_empty() {
                fired.push(tick);
            }
        }
        assert!(fired.len() >= 4, "it should fire repeatedly: {fired:?}");
        // The first shot comes a tick later than the rest, because the counter has to reach the
        // reload before it can be spent; every gap after that is the reload exactly.
        for pair in fired.windows(2).skip(1) {
            assert_eq!(pair[1] - pair[0], CANNON_RELOAD as i32, "on a steady cycle");
        }
    }

    /// The two of a mirrored pair sit on opposite sides.
    #[test]
    fn a_mirrored_pair_sits_either_side() {
        use terrustia_proto::npc_params::SAUCER_TURRET;
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(8);
        let mount = mount_at((5000.0, 3000.0), 0.0);

        let mut left = Npc::new(SAUCER_TURRET, (0.0, 0.0), 1).expect("saucer turret");
        let mut right = Npc::new(SAUCER_TURRET, (0.0, 0.0), 1).expect("saucer turret");
        left.ai[1] = 0.0;
        right.ai[1] = 1.0;
        rider(&mut left, &world(&tiles, None), Some(mount), &mut rng);
        rider(&mut right, &world(&tiles, None), Some(mount), &mut rng);

        assert!(
            left.center().0 < mount.center().0 && right.center().0 > mount.center().0,
            "they should be either side, got {} and {}",
            left.center().0,
            right.center().0
        );
    }
}
