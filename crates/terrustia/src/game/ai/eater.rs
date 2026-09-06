//! Style 5 — the eaters of souls, and everything else that simply flies at you.
//!
//! Ported from `AI_005_EaterOfSouls`. Where the bat and the eye steer axis by axis, this style
//! aims: it works out the velocity that would carry it straight at its target and then edges the
//! real velocity toward that, a fixed step per tick. Two details give the family its character.
//!
//! The first is the **jitter**. A sawtooth on `ai[0]` pushes the velocity around by a fiftieth of a
//! pixel each tick, in a pattern that repeats every four hundred. It is tiny, and it is why a
//! swarm of eaters spreads into a cloud instead of collapsing into one line.
//!
//! The second is the **lazy turn**. Most of this family accelerate at a flat rate whichever way
//! they are going, so an eater that overshoots you sails past and takes a wide arc back. The few
//! that do not — the meteor head, the servant of Cthulhu — double their acceleration while still
//! moving the wrong way, and snap around instead.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    DESPAWN_ENCOURAGED_TICKS, Jitter, eater_bounce, eater_flees_daylight, eater_homes_in_close,
    eater_hugs_the_surface, eater_jitter, eater_speed, eater_stinger, eater_turns_hard,
    eater_water_rise,
};

use super::{PLAYER_HEIGHT, PLAYER_WIDTH, Shot, World, can_see, face, sight::within_firing_range};
use crate::game::npc::{Npc, TileView};

/// The jitter's sawtooth runs between these, turning at zero and at the halfway marks.
const JITTER_PERIOD: f32 = 200.0;
const JITTER_TURN: f32 = 100.0;
/// How hard each jitter nudge pushes.
const JITTER_PUSH: f32 = 0.023;

/// Distance inside which the close-range types put on a burst of homing.
const HOMING_RANGE: f32 = 150.0;
/// ...and outside which the jitter starts for the types that only jitter at range.
const JITTER_RANGE: f32 = 100.0;
/// Strength of that homing burst, as a fraction of the desired velocity.
const HOMING_PULL: f32 = 0.007;

/// A stinger lives five seconds like every other NPC projectile.
const SHOT_LIFETIME: u16 = 300;

/// Snap a coordinate to the eight-pixel grid the game aims on.
///
/// Quantising both ends means a target that shuffles a pixel does not make the whole swarm
/// recompute its heading, which is visible as a faint stepping in how these enemies track you.
fn snap(v: f32) -> f32 {
    ((v / 8.0) as i32 * 8) as f32
}

/// Wander a little, so a swarm spreads out rather than stacking into a line.
fn jitter(npc: &mut Npc) {
    npc.ai[0] += 1.0;
    npc.velocity.1 += if npc.ai[0] > 0.0 {
        JITTER_PUSH
    } else {
        -JITTER_PUSH
    };
    npc.velocity.0 += if npc.ai[0] < -JITTER_TURN || npc.ai[0] > JITTER_TURN {
        JITTER_PUSH
    } else {
        -JITTER_PUSH
    };
    if npc.ai[0] > JITTER_PERIOD {
        npc.ai[0] = -JITTER_PERIOD;
    }
}

/// Edge one axis of the velocity toward where the routine wants to be going.
fn approach_axis(velocity: &mut f32, wanted: f32, accel: f32, turns_hard: bool) {
    if *velocity < wanted {
        *velocity += accel;
        if turns_hard && *velocity < 0.0 && wanted > 0.0 {
            *velocity += accel;
        }
    } else if *velocity > wanted {
        *velocity -= accel;
        if turns_hard && *velocity > 0.0 && wanted < 0.0 {
            *velocity -= accel;
        }
    }
}

/// Bounce off terrain, keeping `keep` of the speed and reversing it.
///
/// Sharper than the bat's rebound, and the minimum speeds are higher: these enemies are meant to
/// ricochet around a cave rather than settle against a wall.
fn bounce(npc: &mut Npc, keep: f32) {
    if npc.collide_x {
        npc.velocity.0 = npc.old_velocity.0 * -keep;
        if npc.direction == -1 && npc.velocity.0 > 0.0 && npc.velocity.0 < 2.0 {
            npc.velocity.0 = 2.0;
        }
        if npc.direction == 1 && npc.velocity.0 < 0.0 && npc.velocity.0 > -2.0 {
            npc.velocity.0 = -2.0;
        }
        npc.dirty = true;
    }
    if npc.collide_y {
        npc.velocity.1 = npc.old_velocity.1 * -keep;
        if npc.velocity.1 > 0.0 && npc.velocity.1 < 1.5 {
            npc.velocity.1 = 2.0;
        }
        if npc.velocity.1 < 0.0 && npc.velocity.1 > -1.5 {
            npc.velocity.1 = -2.0;
        }
        npc.dirty = true;
    }
}

/// Charge and loose a stinger.
///
/// A hornet will only spit in the direction it is already flying, so one that has just turned has
/// to come round before it can shoot. Failing that check throws the charge away rather than
/// holding it, which is why hornets circling a player fire so much less often than their charge
/// rate suggests.
fn sting<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    rng: &mut SmallRng,
    in_range: bool,
) -> Option<Shot> {
    let spec = eater_stinger(npc.npc_type)?;
    for _ in 0..spec.charge_rolls {
        npc.ai[1] += rng.random_range(5..20) as f32 * 0.1 * npc.scale;
    }
    if npc.ai[1] < spec.charge_needed {
        return None;
    }

    let Some(target) = world.target.filter(|_| in_range) else {
        npc.ai[1] = 0.0;
        return None;
    };
    if !can_see(world.tiles, npc, target) {
        npc.ai[1] = 0.0;
        return None;
    }

    let muzzle = (
        npc.position.0 + npc.width() * 0.5,
        npc.position.1 + (npc.stats.height / 2) as f32,
    );
    let mut aim = (
        target.center.0 - muzzle.0 + rng.random_range(-spec.scatter..=spec.scatter) as f32,
        target.center.1 - muzzle.1 + rng.random_range(-spec.scatter..=spec.scatter) as f32,
    );
    let heading_matches =
        (aim.0 < 0.0 && npc.velocity.0 < 0.0) || (aim.0 > 0.0 && npc.velocity.0 > 0.0);
    if !heading_matches {
        npc.ai[1] = 0.0;
        return None;
    }

    let length = (aim.0 * aim.0 + aim.1 * aim.1).sqrt();
    let scale = spec.speed / length;
    aim = (aim.0 * scale, aim.1 * scale);
    // Reset the charge fully on firing. Vanilla parks it at 101 for one tick and clears it to 0 on
    // the next (`if (ai[1] == 101) ai[1] = 0`), giving a full recharge between shots — but that
    // clear depended on the value being *exactly* 101, which the per-tick random increment here
    // never leaves it at, so the charge sat at ~101 and the hornet re-fired every ~25 ticks instead
    // of ~113. Clearing to 0 now is the same net cadence without the fragile equality.
    npc.ai[1] = 0.0;
    npc.dirty = true;
    Some(Shot {
        projectile: spec.projectile,
        damage: (spec.damage * npc.scale) as i32,
        position: muzzle,
        velocity: aim,
        time_left: SHOT_LIFETIME,
        ai: [0.0; 3],
    })
}

/// Drive one chaser for a tick, returning the stinger it spat if it spat one.
pub fn update<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    rng: &mut SmallRng,
    expert: bool,
) -> Option<Shot> {
    let speed = eater_speed(npc.npc_type, expert);
    // A hornet's size is a handicap: `2 - scale` turns a bigger body into a slower one.
    let handicap = if eater_hugs_the_surface(npc.npc_type) {
        2.0 - npc.scale
    } else {
        1.0
    };
    let (max, accel) = (speed.max * handicap, speed.accel * handicap);

    if let Some(t) = world.target {
        face(npc, t);
    }

    // Near the surface a hornet neither climbs far above its target nor dives right onto it, which
    // is what keeps them hovering in the canopy.
    if eater_hugs_the_surface(npc.npc_type)
        && npc.position.1 < world.conditions.surface_y
        && let Some(t) = world.target
    {
        let drop = t.center.1 - PLAYER_HEIGHT as f32 / 2.0 - npc.position.1;
        if (drop > 300.0 && npc.velocity.1 < 0.0) || (drop < 80.0 && npc.velocity.1 > 0.0) {
            npc.velocity.1 *= 0.97;
        }
    }

    // Where it wants to be going, aimed on the eight-pixel grid.
    let centre = (
        snap(npc.position.0 + npc.width() * 0.5),
        snap(npc.position.1 + npc.height() * 0.5),
    );
    let aim_at = world.target.map(|t| {
        (
            snap(t.center.0 - PLAYER_WIDTH as f32 / 2.0 + (PLAYER_WIDTH / 2) as f32),
            snap(t.center.1 - PLAYER_HEIGHT as f32 / 2.0 + (PLAYER_HEIGHT / 2) as f32),
        )
    });
    let offset = aim_at.map_or((0.0, 0.0), |a| (a.0 - centre.0, a.1 - centre.1));
    let reach = (offset.0 * offset.0 + offset.1 * offset.1).sqrt();
    let mut wanted = if reach == 0.0 {
        npc.velocity
    } else {
        let k = max / reach;
        (offset.0 * k, offset.1 * k)
    };

    if let Some(when) = eater_jitter(npc.npc_type) {
        if when == Jitter::Always || reach > JITTER_RANGE {
            jitter(npc);
        }
        if reach < HOMING_RANGE && eater_homes_in_close(npc.npc_type) {
            npc.velocity.0 += wanted.0 * HOMING_PULL;
            npc.velocity.1 += wanted.1 * HOMING_PULL;
        }
    }

    // With nobody left alive to chase, it climbs away and starts its despawn.
    let abandoned = world.target.is_none();
    if abandoned {
        wanted = (f32::from(npc.direction) * max / 2.0, -max / 2.0);
    }

    let turns_hard = eater_turns_hard(npc.npc_type);
    approach_axis(&mut npc.velocity.0, wanted.0, accel, turns_hard);
    approach_axis(&mut npc.velocity.1, wanted.1, accel, turns_hard);

    if let Some(keep) = eater_bounce(npc.npc_type) {
        bounce(npc, keep);
    }

    if world.wet
        && let Some(rise) = eater_water_rise(npc.npc_type)
    {
        if npc.velocity.1 > 0.0 {
            npc.velocity.1 *= 0.95;
        }
        npc.velocity.1 -= rise.accel;
        if npc.velocity.1 < -rise.cap {
            npc.velocity.1 = -rise.cap;
        }
        if rise.retarget
            && let Some(t) = world.target
        {
            face(npc, t);
        }
    }

    let in_range = world
        .target
        .is_some_and(|t| within_firing_range(npc.center(), t.center));
    let shot = sting(npc, world, rng, in_range);

    if (world.conditions.day && eater_flees_daylight(npc.npc_type)) || abandoned {
        npc.velocity.1 -= accel * 2.0;
        npc.time_left = npc.time_left.min(DESPAWN_ENCOURAGED_TICKS);
    }

    npc.sprite_direction = if npc.velocity.0 > 0.0 { 1 } else { -1 };
    npc.dirty = true;
    shot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ai::Conditions;
    use crate::game::npc::TILE;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    #[derive(Default)]
    struct Cave(HashMap<(i32, i32), Tile>);

    impl TileView for Cave {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(11)
    }

    fn chaser(npc_type: u16) -> Npc {
        Npc::new(npc_type, (10_000.0, 10_000.0), 1).expect("a style 5 type")
    }

    fn player_at(x: f32, y: f32) -> Target {
        Target {
            slot: 0,
            center: (x, y),
            velocity: (0.0, 0.0),
            alive: true,
        }
    }

    fn night<'a>(tiles: &'a Cave, target: Option<Target>) -> World<'a, Cave> {
        World {
            conditions: Conditions {
                day: false,
                surface_y: 100.0 * TILE,
                ..Conditions::default()
            },
            ..crate::game::ai::calm(tiles, target)
        }
    }

    #[test]
    fn an_eater_of_souls_flies_straight_at_you() {
        let tiles = Cave::default();
        let mut e = chaser(6);
        let (cx, cy) = e.center();
        let t = Some(player_at(cx + 3000.0, cy + 3000.0));
        for _ in 0..600 {
            update(&mut e, &night(&tiles, t), &mut rng(), false);
        }
        let speed = (e.velocity.0.powi(2) + e.velocity.1.powi(2)).sqrt();
        // Not exactly four: the jitter pushes harder (0.023) than the corrective step pulls
        // (0.02), so the speed pulses either side of nominal rather than settling on it.
        assert!(
            (3.2..5.0).contains(&speed),
            "should hover around its top speed of 4, got {speed}"
        );
        assert!(
            e.velocity.0 > 0.0 && e.velocity.1 > 0.0,
            "and head that way"
        );
    }

    #[test]
    fn each_type_flies_at_its_own_speed() {
        assert_eq!(eater_speed(23, false).max, 1.0, "a meteor head is slow");
        assert_eq!(eater_speed(5, false).max, 5.0, "a servant is quick");
        assert_eq!(
            eater_speed(6, true).accel,
            0.035,
            "expert sharpens the eater"
        );
        assert_eq!(eater_speed(6, false).accel, 0.02);
    }

    #[test]
    fn the_jitter_spreads_a_swarm_out() {
        let tiles = Cave::default();
        let mut swarm: Vec<Npc> = (0..4)
            .map(|i| {
                let mut e = chaser(6);
                e.ai[0] = f32::from(i as u8) * 50.0;
                e
            })
            .collect();
        let t = Some(player_at(10_000.0, 14_000.0));
        for e in &mut swarm {
            for _ in 0..200 {
                update(e, &night(&tiles, t), &mut rng(), false);
            }
        }
        // The routine sets velocity; the physics step that turns it into position is the
        // caller's, so the spread to look at here is in how they are heading.
        let spread = swarm
            .iter()
            .map(|e| e.velocity.0)
            .fold(f32::NEG_INFINITY, f32::max)
            - swarm
                .iter()
                .map(|e| e.velocity.0)
                .fold(f32::INFINITY, f32::min);
        assert!(
            spread > 0.5,
            "identical eaters should not fly in one line, spread was {spread}"
        );
    }

    #[test]
    fn a_meteor_head_snaps_round_and_an_eater_does_not() {
        assert!(eater_turns_hard(23), "a meteor head turns hard");
        assert!(!eater_turns_hard(6), "an eater of souls drifts round");
        assert!(!eater_turns_hard(42), "so does a hornet");
        assert!(eater_turns_hard(5), "a servant of Cthulhu does not");
    }

    #[test]
    fn a_servant_of_cthulhu_leaves_at_dawn_and_an_eater_stays() {
        assert!(eater_flees_daylight(5));
        assert!(!eater_flees_daylight(6));
        assert!(!eater_flees_daylight(23));
        assert!(!eater_flees_daylight(42));
        assert!(!eater_flees_daylight(173));

        let tiles = Cave::default();
        let mut s = chaser(5);
        let (cx, cy) = s.center();
        let mut w = night(&tiles, Some(player_at(cx, cy + 400.0)));
        w.conditions.day = true;
        update(&mut s, &w, &mut rng(), false);
        assert!(
            s.time_left <= DESPAWN_ENCOURAGED_TICKS,
            "should be leaving, got {}",
            s.time_left
        );
    }

    #[test]
    fn losing_everyone_makes_a_chaser_climb_away() {
        let tiles = Cave::default();
        let mut e = chaser(6);
        for _ in 0..30 {
            update(&mut e, &night(&tiles, None), &mut rng(), false);
        }
        assert!(e.velocity.1 < 0.0, "should climb, got {}", e.velocity.1);
        assert!(e.time_left <= DESPAWN_ENCOURAGED_TICKS);
    }

    #[test]
    fn an_eater_bounces_more_softly_than_a_meteor_head() {
        assert_eq!(eater_bounce(6), Some(0.4));
        assert_eq!(eater_bounce(23), Some(0.7));
        assert_eq!(eater_bounce(5), None, "a servant does not rebound at all");

        let tiles = Cave::default();
        let mut e = chaser(6);
        e.direction = 1;
        e.velocity = (4.0, 0.0);
        e.old_velocity = (4.0, 0.0);
        e.collide_x = true;
        update(&mut e, &night(&tiles, None), &mut rng(), false);
        assert!(e.velocity.0 < 0.0, "should rebound, got {}", e.velocity.0);
    }

    #[test]
    fn a_hornet_spits_a_stinger_in_the_direction_it_is_flying() {
        let tiles = Cave::default();
        let mut h = chaser(42);
        let (cx, cy) = h.center();
        let t = Some(player_at(cx + 300.0, cy));
        let mut rng = rng();
        let mut shot = None;
        for _ in 0..600 {
            // Keep it flying toward the player, which is what a stinger needs.
            h.velocity.0 = 3.0;
            if let Some(s) = update(&mut h, &night(&tiles, t), &mut rng, false) {
                shot = Some(s);
                break;
            }
        }
        let shot = shot.expect("a hornet should get a stinger away inside ten seconds");
        assert_eq!(shot.projectile, 55);
        assert_eq!(shot.damage, 10);
        let speed = (shot.velocity.0.powi(2) + shot.velocity.1.powi(2)).sqrt();
        assert!((speed - 8.0).abs() < 1e-3, "got {speed}");
        assert!(shot.velocity.0 > 0.0, "and fly toward the player");
    }

    #[test]
    fn a_hornet_recharges_fully_between_stingers() {
        let tiles = Cave::default();
        let mut h = chaser(42);
        let (cx, cy) = h.center();
        let t = Some(player_at(cx + 300.0, cy));
        let mut rng = rng();
        let mut shot_ticks = Vec::new();
        for tick in 0..1500 {
            h.velocity.0 = 3.0;
            if update(&mut h, &night(&tiles, t), &mut rng, false).is_some() {
                shot_ticks.push(tick);
                if shot_ticks.len() == 2 {
                    break;
                }
            }
        }
        assert_eq!(
            shot_ticks.len(),
            2,
            "a hornet in range should fire at least twice"
        );
        let gap = shot_ticks[1] - shot_ticks[0];
        // A full recharge is ~113 ticks. The old parked-at-101 bug refired in ~25.
        assert!(
            gap > 60,
            "a hornet fully recharges between stingers (~113 ticks), not ~25; got {gap}"
        );
    }

    #[test]
    fn a_hornet_flying_the_wrong_way_throws_its_charge_away() {
        let tiles = Cave::default();
        let mut h = chaser(42);
        let (cx, cy) = h.center();
        let t = Some(player_at(cx + 300.0, cy));
        let mut rng = rng();
        for _ in 0..600 {
            // Always retreating, so the aim never matches the heading.
            h.velocity.0 = -3.0;
            assert!(
                update(&mut h, &night(&tiles, t), &mut rng, false).is_none(),
                "should never get a shot away while flying away"
            );
        }
        assert!(h.ai[1] < 130.0, "and never hold a full charge");
    }

    #[test]
    fn an_eater_of_souls_carries_no_stinger() {
        let tiles = Cave::default();
        let mut e = chaser(6);
        let (cx, cy) = e.center();
        let t = Some(player_at(cx + 300.0, cy));
        let mut rng = rng();
        for _ in 0..600 {
            assert!(update(&mut e, &night(&tiles, t), &mut rng, false).is_none());
        }
    }

    #[test]
    fn a_hornet_in_water_swims_up_and_an_eater_barely_does() {
        assert_eq!(eater_water_rise(42).map(|w| w.cap), Some(4.0));
        assert_eq!(eater_water_rise(6).map(|w| w.cap), Some(2.0));
        assert_eq!(eater_water_rise(23), None, "a meteor head sinks");
    }
}
