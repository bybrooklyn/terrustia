//! Wall crawlers: style 40.
//!
//! These are the forms a wall creeper, black recluse, blood crawler, jungle creeper or desert
//! scorpion takes while it is *on a wall*. Two things make them read differently from a normal
//! chaser:
//!
//! * They fly rather than walk, and they bounce. Hitting terrain rebounds them at half the speed
//!   they arrived with, with a floor under it, so a crawler in a tight shaft ricochets rather than
//!   grinding along the wall.
//! * They do not give up when they lose sight of you: they switch to a slow rolling wander that
//!   still leans toward where you were, so they *drift* out of a room rather than stopping in it.
//!
//! And whenever one finds room to be its ground form, it stops being a wall crawler entirely —
//! which is why walking into an open cave turns the swarm chasing you into something bigger.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    CRAWLER_ACCEL, CRAWLER_BOUNCE, CRAWLER_BOUNCE_FLOOR, CRAWLER_DRIFT_BRAKE, CRAWLER_DRIFT_CAP,
    CRAWLER_FORMS, CRAWLER_PULL, CRAWLER_SPEED, CRAWLER_SPIT_DAMAGE, CRAWLER_SPIT_SPEED,
    CRAWLER_WANDER, CRAWLER_WANDER_BAND, CRAWLER_WANDER_TURN, JUNGLE_CRAWLER_ACCEL,
    JUNGLE_CRAWLER_SPEED, SCORPION_CRAWLER_ACCEL, SCORPION_CRAWLER_SPEED,
};
use terrustia_proto::projectile::ids::CRAWLER_SPIT;

use super::drifters::{Outcome, simple_fly};
use crate::game::ai::{Shot, World, can_see};
use crate::game::npc::{Npc, TILE, TileView};

/// How quickly a given wall form moves, which is not the same for all of them.
fn gait(npc_type: u16) -> (f32, f32) {
    match npc_type {
        237 => (JUNGLE_CRAWLER_SPEED, JUNGLE_CRAWLER_ACCEL),
        531 => (SCORPION_CRAWLER_SPEED, SCORPION_CRAWLER_ACCEL),
        _ => (CRAWLER_SPEED, CRAWLER_ACCEL),
    }
}

/// The ground form a wall form becomes once it has room, if it has one.
pub fn ground_form(npc_type: u16) -> Option<u16> {
    CRAWLER_FORMS
        .iter()
        .find(|(wall, _)| *wall == npc_type)
        .map(|(_, ground)| *ground)
}

/// Whether a box of `width` by `height` centred on `centre` is clear of solid tiles.
///
/// This is what decides whether a crawler can unfold into its bigger ground form.
fn room_for(tiles: &impl TileView, centre: (f32, f32), width: f32, height: f32) -> bool {
    let left = ((centre.0 - width / 2.0) / TILE).floor() as i32;
    let right = ((centre.0 + width / 2.0) / TILE).floor() as i32;
    let top = ((centre.1 - height / 2.0) / TILE).floor() as i32;
    let bottom = ((centre.1 + height / 2.0) / TILE).floor() as i32;
    for x in left..=right {
        for y in top..=bottom {
            let tile = tiles.tile(x, y);
            if tile.is_active() && terrustia_proto::tile_solid::solid(tile.block) {
                return false;
            }
        }
    }
    true
}

/// Style 40.
pub fn crawler(npc: &mut Npc, world: &World<'_, impl TileView>, rng: &mut SmallRng) -> Outcome {
    let mut out = Outcome::default();
    npc.dirty = true;
    let (speed, accel) = gait(npc.npc_type);

    let Some(target) = world.target else {
        // With nobody at all it keeps its heading rather than stopping dead.
        npc.velocity.0 *= 0.99;
        return out;
    };

    // The game rounds both positions to eight-pixel steps before taking the difference, which
    // makes the approach jitter slightly rather than being perfectly smooth.
    let snap = |v: f32| (v / 8.0) as i32 as f32 * 8.0;
    let (cx, cy) = npc.center();
    let (mut dx, mut dy) = (
        snap(target.center.0) - snap(cx),
        snap(target.center.1) - snap(cy),
    );
    let reach = dx.hypot(dy);
    if reach == 0.0 {
        dx = npc.velocity.0;
        dy = npc.velocity.1;
    } else {
        dx = dx / reach * speed;
        dy = dy / reach * speed;
    }
    if !target.alive {
        // Nobody to chase: it drifts off in whatever direction it was facing.
        dx = f32::from(npc.direction) * speed / 2.0;
        dy = -speed / 2.0;
    }
    npc.sprite_direction = -1;

    if can_see(world.tiles, npc, target) {
        simple_fly(npc, (dx, dy), accel);
        npc.rotation = dy.atan2(dx);
    } else {
        // Blind. It rolls along a slow ramp — down for half the cycle, up for the other half, and
        // sideways on a longer beat — so it sweeps the room instead of sitting still.
        npc.ai[0] += 1.0;
        if npc.ai[0] > 0.0 {
            npc.velocity.1 += CRAWLER_WANDER;
        } else {
            npc.velocity.1 -= CRAWLER_WANDER;
        }
        if npc.ai[0] < -CRAWLER_WANDER_BAND || npc.ai[0] > CRAWLER_WANDER_BAND {
            npc.velocity.0 += CRAWLER_WANDER;
        } else {
            npc.velocity.0 -= CRAWLER_WANDER;
        }
        if npc.ai[0] > CRAWLER_WANDER_TURN {
            npc.ai[0] = -CRAWLER_WANDER_TURN;
        }
        // Even blind it still leans the way it last knew you were.
        npc.velocity.0 += dx * CRAWLER_PULL;
        npc.velocity.1 += dy * CRAWLER_PULL;
        npc.rotation = npc.velocity.1.atan2(npc.velocity.0);

        for v in [&mut npc.velocity.0, &mut npc.velocity.1] {
            if v.abs() > CRAWLER_DRIFT_BRAKE {
                *v *= 0.9;
            }
            *v = v.clamp(-CRAWLER_DRIFT_CAP, CRAWLER_DRIFT_CAP);
        }
    }
    if npc.npc_type == 531 {
        npc.rotation += std::f32::consts::FRAC_PI_2;
    }

    // Rebound off whatever it ran into, at half the speed it was doing, but never so slowly that
    // it settles against the wall.
    if npc.collide_x {
        npc.velocity.0 = npc.old_velocity.0 * -CRAWLER_BOUNCE;
        if npc.direction == -1 && (0.0..CRAWLER_BOUNCE_FLOOR).contains(&npc.velocity.0) {
            npc.velocity.0 = CRAWLER_BOUNCE_FLOOR;
        }
        if npc.direction == 1 && (-CRAWLER_BOUNCE_FLOOR..0.0).contains(&npc.velocity.0) {
            npc.velocity.0 = -CRAWLER_BOUNCE_FLOOR;
        }
    }
    if npc.collide_y {
        npc.velocity.1 = npc.old_velocity.1 * -CRAWLER_BOUNCE;
        if npc.velocity.1 > 0.0 && npc.velocity.1 < CRAWLER_DRIFT_BRAKE {
            npc.velocity.1 = CRAWLER_BOUNCE_FLOOR;
        }
        if npc.velocity.1 < 0.0 && npc.velocity.1 > -CRAWLER_DRIFT_BRAKE {
            npc.velocity.1 = -CRAWLER_BOUNCE_FLOOR;
        }
    }

    // In expert these spit web at you on a fuse that is *reset backwards* by being hit, so
    // fighting one is how you stop it shooting.
    let spits = matches!(npc.npc_type, 163 | 236 | 237 | 238);
    if world.conditions.expert && spits && target.alive && can_see(world.tiles, npc, target) {
        npc.local_ai[0] += 1.0;
        if world.was_hurt {
            npc.local_ai[0] = (npc.local_ai[0] - rng.random_range(20..60) as f32).max(0.0);
        }
        if npc.local_ai[0] > rng.random_range(180..900) as f32 {
            npc.local_ai[0] = 0.0;
            let (ax, ay) = (target.center.0 - cx, target.center.1 - cy);
            let length = ax.hypot(ay).max(f32::MIN_POSITIVE);
            out.shots.push(Shot {
                projectile: CRAWLER_SPIT,
                damage: CRAWLER_SPIT_DAMAGE,
                position: (cx, cy),
                velocity: (
                    ax / length * CRAWLER_SPIT_SPEED,
                    ay / length * CRAWLER_SPIT_SPEED,
                ),
                time_left: 300,
                ai: [0.0; 3],
            });
        }
    }

    // Room to unfold? Then it is not a wall crawler any more.
    if npc.local_ai[1] == 0.0
        && let Some(ground) = ground_form(npc.npc_type)
        && let Some(stats) = terrustia_proto::npc_data::npc_stats(ground)
        && room_for(
            world.tiles,
            npc.center(),
            stats.width as f32,
            stats.height as f32,
        )
    {
        out.became = Some(ground);
        npc.local_ai[1] = 12.0;
    }
    npc.local_ai[1] = (npc.local_ai[1] - 1.0).max(0.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ai::Conditions;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    struct Cave(HashMap<(i32, i32), Tile>);

    impl TileView for Cave {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    /// A shaft just wide enough for the wall form and not for the ground form.
    fn shaft() -> Cave {
        let mut tiles = HashMap::new();
        for y in -50..50 {
            for x in [-1, 2] {
                tiles.insert((x, y), Tile::block(1));
            }
        }
        Cave(tiles)
    }

    fn open() -> Cave {
        Cave(HashMap::new())
    }

    fn world<'a>(tiles: &'a Cave, target: Option<(f32, f32)>) -> World<'a, Cave> {
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

    /// The black recluse's wall form.
    const RECLUSE_WALL: u16 = 238;

    fn recluse(x: f32, y: f32) -> Npc {
        Npc::new(RECLUSE_WALL, (x, y), 1).expect("black recluse wall")
    }

    /// Given room, a wall crawler stops being one. This is the whole reason the wall forms exist
    /// as separate types.
    #[test]
    fn a_crawler_with_room_becomes_its_ground_form() {
        let tiles = open();
        let mut rng = SmallRng::seed_from_u64(40);
        let mut c = recluse(0.0, 0.0);
        let out = crawler(&mut c, &world(&tiles, Some((200.0, 0.0))), &mut rng);
        assert_eq!(out.became, Some(163), "a black recluse, not a wall one");
    }

    /// Squeezed into a shaft it stays a wall form, because there is nowhere to unfold.
    #[test]
    fn a_crawler_in_a_shaft_stays_on_the_wall() {
        let tiles = shaft();
        let mut rng = SmallRng::seed_from_u64(40);
        let mut c = recluse(TILE, 0.0);
        let out = crawler(&mut c, &world(&tiles, Some((200.0, 0.0))), &mut rng);
        assert_eq!(out.became, None, "no room to unfold");
    }

    /// Losing sight of you does not stop it: it keeps moving, and keeps leaning your way.
    #[test]
    fn a_blinded_crawler_keeps_drifting_toward_you() {
        // A wall between the crawler and the player.
        let mut tiles = HashMap::new();
        for y in -20..20 {
            tiles.insert((5, y), Tile::block(1));
        }
        let tiles = Cave(tiles);
        let mut rng = SmallRng::seed_from_u64(7);
        let mut c = recluse(0.0, 0.0);
        // Already unfolded once, so it will not transform out of the test.
        c.local_ai[1] = 1000.0;
        let w = world(&tiles, Some((20.0 * TILE, 0.0)));

        let mut moved = 0.0f32;
        for _ in 0..300 {
            crawler(&mut c, &w, &mut rng);
            moved += npc_speed(&c);
            c.position.0 += c.velocity.0;
            c.position.1 += c.velocity.1;
        }
        assert!(moved > 100.0, "a blind crawler should still be moving");
        assert!(
            c.position.0 > 0.0,
            "and drifting toward the player, got {}",
            c.position.0
        );
    }

    fn npc_speed(npc: &Npc) -> f32 {
        npc.velocity.0.hypot(npc.velocity.1)
    }

    /// It rebounds off terrain rather than sticking to it.
    #[test]
    fn a_crawler_bounces_off_what_it_hits() {
        let tiles = open();
        let mut rng = SmallRng::seed_from_u64(3);
        let mut c = recluse(0.0, 0.0);
        c.local_ai[1] = 1000.0;
        c.old_velocity = (6.0, 0.0);
        c.collide_x = true;
        crawler(&mut c, &world(&tiles, Some((500.0, 0.0))), &mut rng);
        assert!(
            c.velocity.0 <= -CRAWLER_BOUNCE_FLOOR,
            "it should have rebounded, got {}",
            c.velocity.0
        );
    }

    /// The web spit is an expert-mode attack, and hitting the spider delays it.
    #[test]
    fn only_an_expert_world_gets_spat_at() {
        let tiles = open();
        let mut rng = SmallRng::seed_from_u64(11);
        let shots_over = |expert: bool, harried: bool, rng: &mut SmallRng| {
            let mut c = recluse(0.0, 0.0);
            c.local_ai[1] = 100_000.0;
            let mut w = world(&tiles, Some((100.0, 0.0)));
            w.conditions = Conditions {
                expert,
                ..Conditions::default()
            };
            w.was_hurt = harried;
            (0..3000)
                .map(|_| crawler(&mut c, &w, rng).shots.len())
                .sum::<usize>()
        };
        assert_eq!(shots_over(false, false, &mut rng), 0, "classic: no spit");
        let calm = shots_over(true, false, &mut rng);
        assert!(calm > 0, "expert: it should spit");
        let harried = shots_over(true, true, &mut rng);
        assert!(
            harried < calm,
            "hitting it should hold the spit off: {harried} vs {calm}"
        );
    }
}
