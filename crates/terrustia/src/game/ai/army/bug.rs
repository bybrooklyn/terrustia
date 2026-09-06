//! Style 111: the Etherian Lightning Bug.
//!
//! It is a ranged enemy that has to stop to shoot. It closes to two hundred pixels, comes to a
//! halt, spends five ticks gathering, throws a bolt, and then sits on a half-second cooldown
//! before it will gather again — so a bug that is being pushed around never fires at all.
//!
//! The other thing it does is refuse to be below you. If its target is overhead, or if it finds
//! itself inside terrain, it climbs. That is what keeps a swarm of them at eye level in an arena
//! rather than piling into the floor.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    ARMY_FADE_IN, LIGHTNING_BUG_BOLT_DAMAGE, LIGHTNING_BUG_BOLT_SPEED, LIGHTNING_BUG_CHARGE,
    LIGHTNING_BUG_COOLDOWN, LIGHTNING_BUG_DECAY, LIGHTNING_BUG_FLOOR, LIGHTNING_BUG_RANGE,
    LIGHTNING_BUG_SEPARATION, LIGHTNING_BUG_SETTLE, LIGHTNING_BUG_SMOOTHING, LIGHTNING_BUG_SPEED,
};
use terrustia_proto::projectile::ids::LIGHTNING_BUG_BOLT;

use super::flyer::separate;
use crate::game::ai::{Shot, World};
use crate::game::npc::{Npc, TileView};

/// How far in front of itself it throws from, which is also where it is drawn holding the bolt.
const MUZZLE: (f32, f32) = (-20.0, 10.0);

pub fn lightning_bug(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    rng: &mut SmallRng,
) -> Vec<Shot> {
    let mut shots = Vec::new();
    npc.dirty = true;
    npc.no_gravity = true;

    if npc.local_ai[1] < ARMY_FADE_IN {
        npc.local_ai[1] += 1.0;
        npc.alpha = (255 - (npc.local_ai[1] as i32 * 5)).max(0);
    }
    separate(npc, world.avoid, LIGHTNING_BUG_SEPARATION);

    npc.rotation = npc.velocity.0.abs() * f32::from(npc.direction) * 0.1;
    npc.sprite_direction = npc.direction;

    let Some(target) = world.target.filter(|t| t.alive) else {
        return shots;
    };
    let (cx, cy) = npc.center();
    let muzzle = (
        cx + MUZZLE.0 * f32::from(npc.sprite_direction),
        cy + MUZZLE.1,
    );
    let toward = (target.center.0 - muzzle.0, target.center.1 - muzzle.1);
    let range = toward.0.hypot(toward.1);
    let unit = {
        let length = range.max(f32::MIN_POSITIVE);
        (toward.0 / length, toward.1 / length)
    };
    let wanted = (unit.0 * LIGHTNING_BUG_SPEED, unit.1 * LIGHTNING_BUG_SPEED);
    let clear = crate::game::ai::can_see(world.tiles, npc, target);

    // `local_ai[0]` is one counter doing two jobs: negative is the cooldown running out, positive
    // is the charge building up.
    if npc.local_ai[0] < 0.0 {
        npc.local_ai[0] += 1.0;
    }

    if range > LIGHTNING_BUG_RANGE || !clear {
        // Too far, or nothing to aim through: close.
        npc.velocity.0 =
            (npc.velocity.0 * (LIGHTNING_BUG_SMOOTHING - 1.0) + wanted.0) / LIGHTNING_BUG_SMOOTHING;
        npc.velocity.1 =
            (npc.velocity.1 * (LIGHTNING_BUG_SMOOTHING - 1.0) + wanted.1) / LIGHTNING_BUG_SMOOTHING;
    } else if toward.1 < LIGHTNING_BUG_FLOOR {
        // Close enough, but not high enough above: climb rather than shoot.
        npc.velocity.1 -= 0.03;
    } else if npc.local_ai[0] >= 0.0 {
        // In position: stop, and gather once it actually has.
        npc.velocity.0 *= LIGHTNING_BUG_DECAY;
        npc.velocity.1 *= LIGHTNING_BUG_DECAY;
        if npc.velocity.0.hypot(npc.velocity.1) < LIGHTNING_BUG_SETTLE {
            npc.local_ai[0] += 1.0;
            if npc.local_ai[0] >= LIGHTNING_BUG_CHARGE {
                npc.local_ai[0] = -LIGHTNING_BUG_COOLDOWN;
                npc.direction = if wanted.0 > 0.0 { 1 } else { -1 };
                npc.sprite_direction = npc.direction;
                shots.push(bolt(muzzle, toward, rng));
            }
        }
    }

    // It will not stay under its target, nor inside the world.
    let buried = {
        let (tx, ty) = ((cx / 16.0) as i32, (cy / 16.0) as i32);
        let tile = world.tiles.tile(tx, ty);
        tile.is_active() && terrustia_proto::tile_solid::solid(tile.block)
    };
    if target.center.1 < cy || buried {
        npc.velocity.1 = (npc.velocity.1 - 0.2).max(-10.0);
    }
    shots
}

/// The bolt, thrown with a wobble in both aim and speed so a line of bugs does not fire a wall.
fn bolt(from: (f32, f32), toward: (f32, f32), rng: &mut SmallRng) -> Shot {
    let scatter = |rng: &mut SmallRng| rng.random_range(-25.0..=25.0f32);
    let mut aim = (toward.0 + scatter(rng), toward.1 + scatter(rng));
    aim.0 *= 1.0 + rng.random_range(-20..=20) as f32 * 0.005;
    aim.1 *= 1.0 + rng.random_range(-20..=20) as f32 * 0.005;
    let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
    let mut velocity = (
        aim.0 / length * LIGHTNING_BUG_BOLT_SPEED,
        aim.1 / length * LIGHTNING_BUG_BOLT_SPEED,
    );
    velocity.0 *= 1.0 + rng.random_range(-20..=20) as f32 / 160.0;
    velocity.1 *= 1.0 + rng.random_range(-20..=20) as f32 / 160.0;
    Shot {
        projectile: LIGHTNING_BUG_BOLT,
        damage: LIGHTNING_BUG_BOLT_DAMAGE,
        position: from,
        velocity,
        time_left: 600,
        ai: [0.0; 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::DD2_LIGHTNING_BUG_T3;
    use terrustia_proto::tile::Tile;

    struct Sky(HashMap<(i32, i32), Tile>);

    impl TileView for Sky {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn world<'a>(tiles: &'a Sky, target: (f32, f32)) -> World<'a, Sky> {
        crate::game::ai::calm(
            tiles,
            Some(Target {
                slot: 0,
                center: target,
                velocity: (0.0, 0.0),
                alive: true,
            }),
        )
    }

    fn bug() -> Npc {
        Npc::new(DD2_LIGHTNING_BUG_T3, (1000.0, 1000.0), 1).expect("a lightning bug")
    }

    fn tick(npc: &mut Npc, w: &World<'_, Sky>, tiles: &Sky, rng: &mut SmallRng) -> Vec<Shot> {
        let shots = lightning_bug(npc, w, rng);
        crate::game::npc::step_physics(npc, tiles);
        shots
    }

    /// It closes on a distant target and then fires — repeatedly, on its cooldown.
    #[test]
    fn it_closes_and_then_fires() {
        let tiles = Sky(HashMap::new());
        // Target well below and to the right, so it is above and in range once it arrives.
        let w = world(&tiles, (1600.0, 1400.0));
        let mut rng = SmallRng::seed_from_u64(1);
        let mut n = bug();
        let mut shots = Vec::new();
        for _ in 0..1200 {
            shots.extend(tick(&mut n, &w, &tiles, &mut rng));
        }
        assert!(
            shots.len() > 4,
            "it should be firing steadily: {}",
            shots.len()
        );
        assert!(shots.iter().all(|s| s.projectile == LIGHTNING_BUG_BOLT));
    }

    /// The cooldown is real: it never fires faster than one every five-and-thirty ticks.
    #[test]
    fn the_cooldown_holds() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, (1600.0, 1400.0));
        let mut rng = SmallRng::seed_from_u64(2);
        let mut n = bug();
        let mut last: Option<i32> = None;
        let mut closest = i32::MAX;
        for tick_no in 0..2000 {
            if !tick(&mut n, &w, &tiles, &mut rng).is_empty() {
                if let Some(last) = last {
                    closest = closest.min(tick_no - last);
                }
                last = Some(tick_no);
            }
        }
        assert!(last.is_some(), "it fired at all");
        // The tick the cooldown reaches zero is also the first tick of the next charge, so the
        // two overlap by one and the tightest possible gap is thirty-four rather than thirty-five.
        assert_eq!(
            closest,
            (LIGHTNING_BUG_COOLDOWN + LIGHTNING_BUG_CHARGE) as i32 - 1,
            "it fired faster than it can gather"
        );
    }

    /// Its shots scatter rather than stacking, and all come out at the same speed.
    #[test]
    fn its_bolts_scatter() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, (1600.0, 1400.0));
        let mut rng = SmallRng::seed_from_u64(3);
        let mut n = bug();
        let mut shots = Vec::new();
        for _ in 0..3000 {
            shots.extend(tick(&mut n, &w, &tiles, &mut rng));
        }
        assert!(shots.len() > 8);
        let angles: Vec<f32> = shots
            .iter()
            .map(|s| s.velocity.1.atan2(s.velocity.0))
            .collect();
        let spread = angles.iter().cloned().fold(f32::MIN, f32::max)
            - angles.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            spread > 0.05,
            "they should not all go the same way: {spread}"
        );
        for shot in &shots {
            let speed = shot.velocity.0.hypot(shot.velocity.1);
            assert!(
                (speed - LIGHTNING_BUG_BOLT_SPEED).abs() < LIGHTNING_BUG_BOLT_SPEED * 0.2,
                "a bolt at {speed}"
            );
        }
    }

    /// It will not sit under its target: it climbs instead of shooting.
    #[test]
    fn it_refuses_to_be_underneath() {
        let tiles = Sky(HashMap::new());
        // Target directly overhead and close.
        let w = world(&tiles, (1000.0, 900.0));
        let mut rng = SmallRng::seed_from_u64(4);
        let mut n = bug();
        let start = n.position.1;
        // It fires happily once it has got above — the point is that it never fires from below.
        while n.center().1 > 900.0 {
            assert!(
                tick(&mut n, &w, &tiles, &mut rng).is_empty(),
                "it should be climbing, not shooting"
            );
            assert!(n.position.1 > -10_000.0, "and it should actually get there");
        }
        assert!(
            n.position.1 < start - 50.0,
            "and getting somewhere doing it"
        );
    }

    /// Buried in the world, it climbs out.
    #[test]
    fn it_climbs_out_of_the_ground() {
        let mut stone = HashMap::new();
        for x in 50..80 {
            for y in 50..80 {
                stone.insert((x, y), Tile::block(1));
            }
        }
        let tiles = Sky(stone);
        // A target far off to one side, so nothing but the burial makes it climb.
        let w = world(&tiles, (4000.0, 1000.0));
        let mut rng = SmallRng::seed_from_u64(5);
        let mut n = bug();
        n.position = (60.0 * 16.0, 60.0 * 16.0);
        for _ in 0..30 {
            lightning_bug(&mut n, &w, &mut rng);
        }
        assert!(
            n.velocity.1 < -0.5,
            "it should be pulling up: {:?}",
            n.velocity
        );
    }
}
