//! Styles 19 and 42 — the two that wait.
//!
//! Neither of these moves toward anything. The **antlion** (style 19) sits buried with only its
//! head showing and lobs sand at whatever comes into view above it; the **lost girl** (style 42)
//! stands perfectly still pretending to be bound until someone gets close, and then stops
//! pretending.

use terrustia_proto::npc_params::{
    ANTLION_RELOAD, ANTLION_SHOT_DAMAGE, ANTLION_SHOT_SPEED, LOST_GIRL_RANGE, LOST_GIRL_WINDUP,
    NYMPH,
};
use terrustia_proto::projectile::ids::ANTLION_SHOT_TYPE;
use terrustia_proto::tile_solid::solid;

use super::{Shot, World, can_see, face, sight::within_firing_range};
use crate::game::npc::{Npc, TILE, TileView};

/// A sand ball lives five seconds like the other NPC projectiles.
const SHOT_LIFETIME: u16 = 300;

/// The arc within which an antlion can actually loose a shot, in radians either side of straight
/// up. Outside it the shot would clip its own burrow.
const AIM_LIMIT: f32 = 1.2;
/// ...and the arc it will physically turn its head through, which is narrower still.
const HEAD_LIMIT: f32 = 0.8;

/// Drive one antlion for a tick, returning the sand ball it spat if it spat one.
pub fn antlion<T: TileView>(npc: &mut Npc, world: &World<'_, T>) -> Option<Shot> {
    let target = world.target;
    if let Some(t) = target {
        face(npc, t);
    }

    // Where a shot would go: at the player's feet, at a fixed speed.
    let aim = target.map(|t| {
        let (cx, cy) = npc.center();
        let dx = t.center.0 - cx;
        let dy = t.center.1 - super::PLAYER_HEIGHT as f32 / 2.0 - cy;
        let k = ANTLION_SHOT_SPEED / (dx * dx + dy * dy).sqrt();
        (dx * k, dy * k)
    });

    // It only shoots at something above it, and only within a narrow arc.
    let mut can_shoot = false;
    if npc.direction_y < 0
        && let Some(a) = aim
    {
        npc.rotation = a.1.atan2(a.0) + 1.57;
        can_shoot = npc.rotation >= -AIM_LIMIT && npc.rotation <= AIM_LIMIT;
        if !target.is_some_and(|t| t.alive) {
            can_shoot = false;
        }
        npc.rotation = npc.rotation.clamp(-HEAD_LIMIT, HEAD_LIMIT);
        if npc.velocity.0 != 0.0 {
            npc.velocity.0 *= 0.9;
            // The game's own test is always true, so an antlion never keeps a sideways drift for
            // more than a tick.
            npc.velocity.0 = 0.0;
            npc.dirty = true;
        }
    }

    if npc.ai[0] > 0.0 {
        npc.ai[0] -= 1.0;
    }

    let mut shot = None;
    if npc.ai[0] == 0.0
        && can_shoot
        && let (Some(t), Some(a)) = (target, aim)
        && within_firing_range(npc.center(), t.center)
        && can_see(world.tiles, npc, t)
    {
        npc.ai[0] = ANTLION_RELOAD;
        npc.dirty = true;
        shot = Some(Shot {
            projectile: ANTLION_SHOT_TYPE,
            damage: ANTLION_SHOT_DAMAGE,
            position: npc.center(),
            velocity: a,
            time_left: SHOT_LIFETIME,
            ai: [0.0; 3],
        });
    }

    // Buried in rock: rise slowly until the head is clear again.
    let feet_y = ((npc.position.1 + npc.height()) / TILE) as i32;
    let buried = [
        (npc.position.0 / TILE) as i32,
        ((npc.position.0 + npc.width() / 2.0) / TILE) as i32,
        ((npc.position.0 + npc.width()) / TILE) as i32,
    ]
    .into_iter()
    .any(|x| {
        let tile = world.tiles.tile(x, feet_y);
        tile.is_active() && solid(tile.block)
    });
    npc.no_gravity = buried;
    npc.no_tile_collide = buried;
    if buried {
        npc.velocity.1 = -0.2;
        npc.dirty = true;
    }
    shot
}

/// Drive one lost girl for a tick, returning what she turns into once the act is over.
pub fn lost_girl<T: TileView>(npc: &mut Npc, world: &World<'_, T>) -> Option<u16> {
    if npc.ai[0] == 0.0 {
        if let Some(t) = world.target {
            let (cx, cy) = npc.center();
            // The vertical reach is measured to the player's feet, so someone standing on her
            // ledge counts and someone directly above at a distance does not.
            let dx = t.center.0 - cx;
            let dy = t.center.1 - super::PLAYER_HEIGHT as f32 / 2.0 - cy;
            if (dx * dx + dy * dy).sqrt() < LOST_GIRL_RANGE && can_see(world.tiles, npc, t) {
                npc.ai[0] = 1.0;
            }
        }
        // Being moved or hurt gives it away just as well as being seen.
        if npc.velocity.0 != 0.0
            || npc.velocity.1 < 0.0
            || npc.velocity.1 > 2.0
            || npc.life != npc.life_max
        {
            npc.ai[0] = 1.0;
        }
        npc.dirty = true;
        return None;
    }

    npc.ai[0] += 1.0;
    npc.dirty = true;
    if npc.ai[0] >= LOST_GIRL_WINDUP {
        npc.ai[0] = LOST_GIRL_WINDUP;
        return Some(NYMPH);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    #[derive(Default)]
    struct Sand(HashMap<(i32, i32), Tile>);

    impl TileView for Sand {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn world<'a>(tiles: &'a Sand, target: Option<Target>) -> World<'a, Sand> {
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

    #[test]
    fn an_antlion_spits_at_someone_above_it() {
        let tiles = Sand::default();
        let mut a = Npc::new(69, (10_000.0, 10_000.0), 1).expect("antlion");
        let (cx, cy) = a.center();
        let shot = antlion(
            &mut a,
            &world(&tiles, Some(player_at(cx + 40.0, cy - 300.0))),
        );
        let shot = shot.expect("should have fired");
        assert_eq!(shot.projectile, ANTLION_SHOT_TYPE);
        assert!(shot.velocity.1 < 0.0, "and upward, got {:?}", shot.velocity);
        let speed = (shot.velocity.0.powi(2) + shot.velocity.1.powi(2)).sqrt();
        assert!((speed - ANTLION_SHOT_SPEED).abs() < 1e-3, "got {speed}");
    }

    #[test]
    fn an_antlion_ignores_someone_beneath_it() {
        let tiles = Sand::default();
        let mut a = Npc::new(69, (10_000.0, 10_000.0), 1).expect("antlion");
        let (cx, cy) = a.center();
        let shot = antlion(&mut a, &world(&tiles, Some(player_at(cx, cy + 300.0))));
        assert!(shot.is_none(), "it only fires upward");
    }

    #[test]
    fn an_antlion_reloads_between_shots() {
        let tiles = Sand::default();
        let mut a = Npc::new(69, (10_000.0, 10_000.0), 1).expect("antlion");
        let (cx, cy) = a.center();
        let t = Some(player_at(cx + 40.0, cy - 300.0));
        let mut shots = 0;
        for _ in 0..(ANTLION_RELOAD as i32 * 2 + 3) {
            if antlion(&mut a, &world(&tiles, t)).is_some() {
                shots += 1;
            }
        }
        assert_eq!(shots, 3, "one every two hundred ticks, got {shots}");
    }

    #[test]
    fn an_antlion_buried_in_rock_rises_out_of_it() {
        let mut tiles = Sand::default();
        let a = Npc::new(69, (10_000.0, 10_000.0), 1).expect("antlion");
        let feet = ((a.position.1 + a.height()) / TILE) as i32;
        for x in 620..630 {
            tiles.0.insert((x, feet), Tile::block(1));
        }
        let mut a = a;
        antlion(&mut a, &world(&tiles, None));
        assert!(a.no_tile_collide, "should be able to move through the sand");
        assert_eq!(a.velocity.1, -0.2, "and be climbing out of it");
    }

    #[test]
    fn a_lost_girl_waits_until_someone_comes_close() {
        let tiles = Sand::default();
        let mut g = Npc::new(195, (10_000.0, 10_000.0), 1).expect("lost girl");
        let (cx, cy) = g.center();
        assert!(
            lost_girl(&mut g, &world(&tiles, Some(player_at(cx + 900.0, cy)))).is_none(),
            "nobody near, so nothing happens"
        );
        assert_eq!(g.ai[0], 0.0);

        lost_girl(&mut g, &world(&tiles, Some(player_at(cx + 50.0, cy))));
        assert_eq!(g.ai[0], 1.0, "should have dropped the act");
    }

    #[test]
    fn a_lost_girl_becomes_a_nymph_after_the_windup() {
        let tiles = Sand::default();
        let mut g = Npc::new(195, (10_000.0, 10_000.0), 1).expect("lost girl");
        g.ai[0] = 1.0;
        let mut became = None;
        for _ in 0..(LOST_GIRL_WINDUP as i32 + 5) {
            if let Some(t) = lost_girl(&mut g, &world(&tiles, None)) {
                became = Some(t);
                break;
            }
        }
        assert_eq!(became, Some(NYMPH));
    }

    #[test]
    fn hurting_a_lost_girl_gives_her_away() {
        let tiles = Sand::default();
        let mut g = Npc::new(195, (10_000.0, 10_000.0), 1).expect("lost girl");
        g.life -= 1;
        lost_girl(&mut g, &world(&tiles, None));
        assert_eq!(g.ai[0], 1.0);
    }
}
