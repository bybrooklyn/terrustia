//! Style 38 — the frost legion.
//!
//! Snowmen do not walk, they hop, and the rhythm is three to a phrase: two short hops and then a
//! long one. The counter that produces that rhythm is also what tells each of them when to stop
//! and do its own thing — Mister Stabby stands still for three seconds, the Snow Balla throws a
//! snowball, and the Snowman Gangsta never stops at all and fires flat down the line as it goes.

use terrustia_proto::npc_params::{
    FROST_HOP, FROST_HOPS_BEFORE_PAUSE, FROST_LEAP, FROST_STABBY_PAUSE, FROST_STUBBORN, frost_hop,
    frost_shot,
};

use super::{Shot, World, face, sight::within_firing_range};
use crate::game::npc::{Npc, TileView};

const SHOT_LIFETIME: u16 = 300;

/// Whether a type pauses after its run of hops rather than hopping forever.
fn pauses(npc_type: u16) -> bool {
    matches!(npc_type, 144 | 145)
}

/// Drive one member of the frost legion for a tick.
pub fn update<T: TileView>(npc: &mut Npc, world: &World<'_, T>) -> Option<Shot> {
    let hop = frost_hop(npc.npc_type);
    let mut shot = None;

    // The gangsta fires on its own clock and never breaks stride to do it.
    if let Some(spec) = frost_shot(npc.npc_type).filter(|s| s.flat) {
        npc.ai[2] += 1.0;
        if npc.ai[2] >= spec.cycle {
            npc.ai[2] = 0.0;
            let worth_it = world
                .target
                .is_some_and(|t| t.alive && within_firing_range(npc.center(), t.center));
            if worth_it {
                shot = Some(Shot {
                    projectile: spec.projectile,
                    damage: spec.damage,
                    position: (
                        npc.position.0 + npc.width() * 0.5 - (npc.direction as f32) * 12.0,
                        npc.position.1 + npc.height() * 0.5,
                    ),
                    velocity: (spec.speed * f32::from(npc.sprite_direction), 0.0),
                    time_left: SHOT_LIFETIME,
                    ai: [0.0; 3],
                });
                npc.dirty = true;
            }
        }
    }

    let resting = pauses(npc.npc_type) && npc.ai[1] >= FROST_HOPS_BEFORE_PAUSE;
    if resting {
        if let Some(t) = world.target {
            face(npc, t);
        }
        npc.sprite_direction = npc.direction;
        let spec = frost_shot(npc.npc_type);

        // A snow balla that has nobody worth throwing at simply sets off again.
        if let Some(spec) = spec.filter(|s| !s.flat) {
            let worth_it = world
                .target
                .is_some_and(|t| t.alive && within_firing_range(npc.center(), t.center));
            if npc.velocity.1 == 0.0 && npc.ai[2] == 0.0 && !worth_it {
                npc.ai[1] = 0.0;
                npc.dirty = true;
            } else if npc.velocity.1 == 0.0 {
                npc.ai[2] += 1.0;
                npc.velocity.0 *= 0.9;
                if npc.velocity.0 > -0.3 && npc.velocity.0 < 0.3 {
                    npc.velocity.0 = 0.0;
                }
                if npc.ai[2] >= spec.cycle {
                    npc.ai[2] = 0.0;
                    npc.ai[1] = 0.0;
                    npc.dirty = true;
                }
            }
            if npc.ai[2] == spec.release_at
                && let Some(t) = world.target
            {
                let muzzle = (
                    npc.position.0 + npc.width() * 0.5 - (npc.direction as f32) * 12.0,
                    npc.position.1 + npc.height() * 0.25,
                );
                let dx = t.center.0 - muzzle.0;
                let dy = t.center.1 - super::PLAYER_HEIGHT as f32 / 2.0 - muzzle.1;
                let k = spec.speed / (dx * dx + dy * dy).sqrt();
                shot = Some(Shot {
                    projectile: spec.projectile,
                    damage: spec.damage,
                    position: muzzle,
                    velocity: (dx * k, dy * k),
                    time_left: SHOT_LIFETIME,
                    ai: [0.0; 3],
                });
                npc.dirty = true;
            }
        } else if npc.velocity.1 == 0.0 {
            // Mister Stabby: just stands there, at length.
            npc.velocity.0 *= 0.9;
            npc.ai[2] += 1.0;
            if npc.velocity.0 > -0.3 && npc.velocity.0 < 0.3 {
                npc.velocity.0 = 0.0;
            }
            if npc.ai[2] >= FROST_STABBY_PAUSE {
                npc.ai[2] = 0.0;
                npc.ai[1] = 0.0;
                npc.dirty = true;
            }
        }
    } else {
        if npc.velocity.1 == 0.0 {
            // Went nowhere since the last hop: something is in the way, so turn and stop
            // re-targeting for a second, or it would just turn straight back.
            if npc.ai[3] == 0.0 && npc.position.0 == npc.old_position.0 {
                npc.direction = -npc.direction;
                npc.ai[3] = FROST_STUBBORN;
                npc.dirty = true;
            }
            if npc.ai[3] == 0.0
                && let Some(t) = world.target
            {
                face(npc, t);
            }
            npc.ai[0] += 1.0;
            if npc.ai[0] > 2.0 {
                // Every third hop is the long one.
                npc.ai[0] = 0.0;
                npc.ai[1] += 1.0;
                npc.velocity.1 = FROST_LEAP;
                npc.velocity.0 += f32::from(npc.direction) * hop.accel * 1.1;
            } else {
                npc.velocity.1 = FROST_HOP;
                npc.velocity.0 += f32::from(npc.direction) * hop.accel * 0.9;
            }
            npc.sprite_direction = npc.direction;
            npc.dirty = true;
        }
        // A trickle of thrust in the air, so a hop curves rather than arcing dead straight.
        npc.velocity.0 += f32::from(npc.direction) * hop.accel * 0.01;
    }

    if npc.ai[3] > 0.0 {
        npc.ai[3] -= 1.0;
    }
    if npc.velocity.0 > hop.max && npc.direction > 0 {
        npc.velocity.0 = hop.max;
    }
    if npc.velocity.0 < -hop.max && npc.direction < 0 {
        npc.velocity.0 = -hop.max;
    }
    npc.dirty = true;
    shot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use terrustia_proto::tile::Tile;

    struct Flat;

    impl TileView for Flat {
        fn tile(&self, _x: i32, y: i32) -> Tile {
            if y >= 100 { Tile::block(1) } else { Tile::AIR }
        }
    }

    fn world<'a>(tiles: &'a Flat, target: Option<Target>) -> World<'a, Flat> {
        crate::game::ai::calm(tiles, target)
    }

    fn snowman(npc_type: u16) -> Npc {
        let mut n = Npc::new(npc_type, (10_000.0, 10_000.0), 1).expect("a style 38 type");
        n.old_position = (0.0, 0.0);
        n
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
    fn every_third_hop_is_a_long_one() {
        let tiles = Flat;
        let mut s = snowman(144);
        let (cx, cy) = s.center();
        let t = Some(player_at(cx + 300.0, cy));
        let mut impulses = Vec::new();
        for _ in 0..6 {
            s.velocity.1 = 0.0;
            s.old_position = (s.position.0 - 1.0, s.position.1);
            update(&mut s, &world(&tiles, t));
            impulses.push(s.velocity.1);
        }
        assert_eq!(
            impulses,
            vec![
                FROST_HOP, FROST_HOP, FROST_LEAP, FROST_HOP, FROST_HOP, FROST_LEAP
            ]
        );
    }

    #[test]
    fn each_snowman_hops_at_its_own_pace() {
        assert_eq!(frost_hop(143).max, 3.0, "the gangsta is slowest");
        assert_eq!(frost_hop(145).max, 3.5);
        assert_eq!(frost_hop(144).max, 4.0, "and stabby is quickest");
    }

    #[test]
    fn a_snowman_boxed_in_turns_round_and_stays_turned() {
        let tiles = Flat;
        let mut s = snowman(144);
        s.direction = 1;
        // Went nowhere since the last tick.
        s.old_position = s.position;
        let (cx, cy) = s.center();
        let t = Some(player_at(cx + 300.0, cy));
        update(&mut s, &world(&tiles, t));
        assert_eq!(s.direction, -1, "should turn away from the wall");
        assert_eq!(s.ai[3], FROST_STUBBORN - 1.0, "and hold that for a while");

        // While stubborn, the player behind it does not pull it back round.
        s.velocity.1 = 0.0;
        s.old_position = (s.position.0 - 1.0, s.position.1);
        update(&mut s, &world(&tiles, t));
        assert_eq!(s.direction, -1);
    }

    #[test]
    fn a_gangsta_fires_flat_along_its_facing_without_stopping() {
        let tiles = Flat;
        let mut g = snowman(143);
        g.sprite_direction = 1;
        let (cx, cy) = g.center();
        let t = Some(player_at(cx + 300.0, cy + 200.0));
        let mut shot = None;
        for _ in 0..200 {
            g.velocity.1 = 0.0;
            g.old_position = (g.position.0 - 1.0, g.position.1);
            if let Some(s) = update(&mut g, &world(&tiles, t)) {
                shot = Some(s);
                break;
            }
        }
        let shot = shot.expect("should have fired");
        assert_eq!(shot.projectile, 110);
        assert_eq!(shot.velocity.1, 0.0, "flat, whatever the target's height");
        assert!(shot.velocity.0 > 0.0);
    }

    #[test]
    fn a_snow_balla_stops_to_aim_and_a_gangsta_does_not() {
        assert!(!frost_shot(145).unwrap().flat, "the balla aims");
        assert!(frost_shot(143).unwrap().flat, "the gangsta does not");
        assert!(frost_shot(144).is_none(), "stabby throws nothing");

        let tiles = Flat;
        let mut b = snowman(145);
        b.ai[1] = FROST_HOPS_BEFORE_PAUSE;
        let (cx, cy) = b.center();
        let t = Some(player_at(cx + 300.0, cy - 100.0));
        let mut shot = None;
        for _ in 0..60 {
            b.velocity.1 = 0.0;
            if let Some(s) = update(&mut b, &world(&tiles, t)) {
                shot = Some(s);
                break;
            }
        }
        let shot = shot.expect("should have thrown");
        assert_eq!(shot.projectile, 109);
        assert!(shot.velocity.1 < 0.0, "aimed upward at the player");
        let speed = (shot.velocity.0.powi(2) + shot.velocity.1.powi(2)).sqrt();
        assert!((speed - 10.0).abs() < 1e-3);
    }

    #[test]
    fn mister_stabby_stands_still_for_three_seconds_then_carries_on() {
        let tiles = Flat;
        let mut s = snowman(144);
        s.ai[1] = FROST_HOPS_BEFORE_PAUSE;
        let (cx, cy) = s.center();
        let t = Some(player_at(cx + 300.0, cy));
        for _ in 0..(FROST_STABBY_PAUSE as i32) {
            s.velocity.1 = 0.0;
            update(&mut s, &world(&tiles, t));
        }
        assert_eq!(s.ai[1], 0.0, "should have set off again");
    }
}
