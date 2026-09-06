//! Style 10 — the cursed skulls.
//!
//! A skull does not fly at you steadily; it stalks. Its speed is tiered by distance and every tier
//! is *slower* than the one outside it, so it rushes in from across the room and then dawdles once
//! it is close. Inside 250 pixels it stops steering entirely and jitters, and all the while a timer
//! is running. At six hundred ticks that timer flips it into a charge — eight times the
//! acceleration, four times the speed — for fifty ticks, and then resets.
//!
//! The giant one interrupts all of that to spit a skull of its own; the water bolt mimic plays dead
//! until something hits it.

use terrustia_proto::npc_params::{
    GIANT_SKULL_RANGE, GIANT_SKULL_RECOVER, GIANT_SKULL_RELEASE, GIANT_SKULL_SHOT_DAMAGE,
    GIANT_SKULL_SHOT_SPEED, GIANT_SKULL_WINDUP, SKULL_CHARGE_ACCEL, SKULL_CHARGE_AT,
    SKULL_CHARGE_OVER, SKULL_CHARGE_SPEED, SKULL_JITTER_PERIOD, SKULL_JITTER_PUSH,
    SKULL_JITTER_RANGE, SKULL_JITTER_RATE, SKULL_JITTER_TURN, skull_approach,
};
use terrustia_proto::projectile::ids::GIANT_SKULL_SHOT_TYPE;

use super::{Shot, World};
use crate::game::npc::{Npc, TileView};
use crate::game::npc_ai::Spawn;
use rand::{Rng, rngs::SmallRng};

/// A spat skull lives five seconds like every other NPC projectile.
const SHOT_LIFETIME: u16 = 300;

/// Big Mimic's cursed cousin (`NPCID.BigMimicCorruption`'s dungeon sibling, type 694), which plays
/// dead and then hunts with a second attack of its own.
const WATER_BOLT_MIMIC: u16 = 694;
/// The Water Sphere it conjures, which is an NPC (style 9) rather than a projectile.
const WATER_SPHERE: u16 = 33;

/// Type 694's own numbers, `NPC.cs:21933-21939` (`num164`..`num171`).
const MIMIC_CAST_RANGE: f32 = 500.0;
const MIMIC_BAND: (f32, f32) = (100.0, 300.0);
const MIMIC_WINDUP: f32 = 120.0;
const MIMIC_CAST_OVER: f32 = 30.0;
const MIMIC_HOLD_OVER: f32 = 60.0;
const MIMIC_RELEASE: f32 = 17.0;
const MIMIC_COOLDOWN: f32 = 300.0;
/// How close it has to be before it stops closing and holds its charge back (`NPC.cs:21736-21742`).
const MIMIC_HOLD_RANGE: f32 = 100.0;
const MIMIC_HOLD_FOR: f32 = -60.0;

/// What a skull's tick produced.
#[derive(Debug, Default)]
pub struct Bite {
    /// The giant one's spat skull.
    pub shot: Option<Shot>,
    /// The mimic's Water Sphere.
    pub spawn: Option<Spawn>,
}

/// Whether a type lies dormant until struck, rather than hunting from the start.
fn plays_dead(npc_type: u16) -> bool {
    npc_type == WATER_BOLT_MIMIC
}

/// Whether a type spits skulls of its own.
fn spits(npc_type: u16) -> bool {
    npc_type == 289
}

/// Drive one cursed skull for a tick, returning what it spat if it spat anything.
pub fn update<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) -> Bite {
    let mut out = Bite::default();
    let Some(target) = world.target else {
        // Nobody to stalk: drift away and let the despawn timer have it.
        npc.velocity.0 *= 0.98;
        npc.velocity.1 *= 0.98;
        npc.dirty = true;
        return out;
    };

    // `NPC.cs:21682-21684`: the timer runs in every state but the dormant one. It used to be
    // bumped after the two mimic states returned, which meant a woken mimic never reached the
    // eighty ticks that end its stun and stayed asleep for good.
    if npc.ai[3] != 3.0 {
        npc.ai[1] += 1.0;
    }

    // States 3 and 4 belong to the mimic: dormant, then stunned after being woken.
    if plays_dead(npc.npc_type) {
        if npc.ai[3] == 3.0 {
            npc.velocity = (0.0, 0.0);
            npc.rotation = 0.0;
            if npc.was_hurt {
                npc.ai[3] = 4.0;
                npc.dirty = true;
            }
            return out;
        }
        if npc.ai[3] == 4.0 {
            npc.velocity = (0.0, 0.0);
            npc.rotation = 0.0;
            // Eighty ticks of shaking itself awake.
            if npc.ai[1] > 80.0 {
                npc.ai[1] = 0.0;
                npc.ai[3] = 0.0;
                npc.dirty = true;
            }
            return out;
        }
    }

    let (cx, cy) = npc.center();
    let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
    let reach = (dx * dx + dy * dy).sqrt();

    // `flag14` (`NPC.cs:21686`): the mimic's second state is a dead stop, and the whole movement
    // block is skipped while it holds.
    let holding = plays_dead(npc.npc_type) && npc.ai[2] >= 0.0 && npc.ai[3] == 2.0;
    if holding {
        npc.dirty = true;
        mimic_attack(npc, reach, rng, &mut out);
        return out;
    }

    let charging = npc.ai[1] > SKULL_CHARGE_AT;
    let (mut speed, mut accel) = skull_approach(reach);
    if charging {
        speed = SKULL_CHARGE_SPEED;
        accel = SKULL_CHARGE_ACCEL;
        if npc.ai[1] > SKULL_CHARGE_OVER {
            npc.ai[1] = 0.0;
        }
        npc.dirty = true;
    } else if plays_dead(npc.npc_type) && reach < MIMIC_HOLD_RANGE && npc.ai[1] >= 0.0 {
        // `NPC.cs:21736-21742`: right on top of you it stops winding up to charge and simply keeps
        // station instead, which is what gives it time to cast.
        npc.ai[1] = MIMIC_HOLD_FOR;
        npc.dirty = true;
    } else if reach < SKULL_JITTER_RANGE {
        // Circling: a sawtooth on ai[0] pushes it around rather than at anything.
        npc.ai[0] += SKULL_JITTER_RATE;
        npc.velocity.1 += if npc.ai[0] > 0.0 {
            SKULL_JITTER_PUSH
        } else {
            -SKULL_JITTER_PUSH
        };
        npc.velocity.0 += if npc.ai[0] < -SKULL_JITTER_TURN || npc.ai[0] > SKULL_JITTER_TURN {
            SKULL_JITTER_PUSH
        } else {
            -SKULL_JITTER_PUSH
        };
        if npc.ai[0] > SKULL_JITTER_PERIOD {
            npc.ai[0] = -SKULL_JITTER_PERIOD;
            npc.dirty = true;
        }
    }

    // Where it wants to be going.
    let k = speed / reach.max(f32::MIN_POSITIVE);
    let wanted = if target.alive {
        (dx * k, dy * k)
    } else {
        (f32::from(npc.direction) * speed / 2.0, -speed / 2.0)
    };
    if npc.velocity.0 < wanted.0 {
        npc.velocity.0 += accel;
    } else if npc.velocity.0 > wanted.0 {
        npc.velocity.0 -= accel;
    }
    if npc.velocity.1 < wanted.1 {
        npc.velocity.1 += accel;
    } else if npc.velocity.1 > wanted.1 {
        npc.velocity.1 -= accel;
    }

    npc.sprite_direction = if dx > 0.0 { -1 } else { 1 };
    npc.rotation = (dy * k).atan2(dx * k);
    npc.dirty = true;

    if plays_dead(npc.npc_type) {
        mimic_attack(npc, reach, rng, &mut out);
        return out;
    }
    if !spits(npc.npc_type) {
        return out;
    }

    // The giant one's spit: a long wind-up, then a short window with the shot in the middle.
    if npc.was_hurt {
        npc.ai[2] = 0.0;
        npc.ai[3] = 0.0;
    }
    if reach > GIANT_SKULL_RANGE {
        npc.ai[2] = 0.0;
        npc.ai[3] = 0.0;
        return out;
    }
    npc.ai[2] += 1.0;
    if npc.ai[3] == 0.0 {
        if npc.ai[2] > GIANT_SKULL_WINDUP {
            npc.ai[2] = 0.0;
            npc.ai[3] = 1.0;
            npc.dirty = true;
        }
        return out;
    }
    if npc.ai[2] > GIANT_SKULL_RECOVER {
        npc.ai[3] = 0.0;
        npc.dirty = true;
    }
    if npc.ai[2] != GIANT_SKULL_RELEASE {
        return out;
    }
    let k = GIANT_SKULL_SHOT_SPEED / reach.max(f32::MIN_POSITIVE);
    out.shot = Some(Shot {
        projectile: GIANT_SKULL_SHOT_TYPE,
        damage: GIANT_SKULL_SHOT_DAMAGE,
        position: (cx, cy),
        velocity: (dx * k, dy * k),
        time_left: SHOT_LIFETIME,
        ai: [0.0; 3],
    });
    out
}

/// Type 694's second attack, `NPC.cs:21940-21998`.
///
/// Two states off one wind-up. Anywhere inside five hundred pixels it conjures a Water Sphere; in
/// the hundred-to-three-hundred band it instead plants itself for a second and then takes a long
/// cooldown, and when both are available it picks the hold one time in three.
fn mimic_attack(npc: &mut Npc, reach: f32, rng: &mut SmallRng, out: &mut Bite) {
    let can_hold = reach >= MIMIC_BAND.0
        && reach <= MIMIC_BAND.1
        && npc.ai[2] >= 0.0
        && matches!(npc.ai[3], 0.0 | 2.0);
    let can_cast = reach <= MIMIC_CAST_RANGE && npc.ai[2] >= 0.0 && matches!(npc.ai[3], 0.0 | 1.0);

    if can_hold && (!can_cast || rng.random_ratio(1, 3)) {
        npc.ai[2] += 1.0;
        if npc.ai[3] == 0.0 {
            if npc.ai[2] > MIMIC_WINDUP {
                npc.ai[2] = 0.0;
                npc.ai[3] = 2.0;
                npc.dirty = true;
            }
        } else if npc.ai[3] == 2.0 && npc.ai[2] > MIMIC_HOLD_OVER {
            npc.ai[2] = -MIMIC_COOLDOWN;
            npc.ai[3] = 0.0;
            npc.dirty = true;
        }
    } else if can_cast {
        npc.ai[2] += 1.0;
        if npc.ai[3] == 0.0 {
            if npc.ai[2] > MIMIC_WINDUP {
                npc.ai[2] = 0.0;
                npc.ai[3] = 1.0;
                npc.dirty = true;
            }
        } else if npc.ai[3] == 1.0 {
            if npc.ai[2] > MIMIC_CAST_OVER {
                npc.ai[2] = 0.0;
                npc.ai[3] = 0.0;
                npc.dirty = true;
            }
            if npc.ai[2] == MIMIC_RELEASE {
                out.spawn = Some(Spawn {
                    handle: None,
                    npc_type: WATER_SPHERE,
                    position: npc.center(),
                    velocity: (0.0, 0.0),
                    parent: None,
                    ai: [None; 4],
                });
            }
        }
    } else {
        // Out of reach, or riding out the cooldown: the counter climbs back toward zero and stops.
        npc.ai[2] += 1.0;
        if npc.ai[2] > 0.0 {
            npc.ai[2] = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use terrustia_proto::tile::Tile;

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(3)
    }

    struct Void;

    impl TileView for Void {
        fn tile(&self, _x: i32, _y: i32) -> Tile {
            Tile::AIR
        }
    }

    fn world<'a>(tiles: &'a Void, target: Option<Target>) -> World<'a, Void> {
        crate::game::ai::calm(tiles, target)
    }

    fn skull(npc_type: u16) -> Npc {
        Npc::new(npc_type, (10_000.0, 10_000.0), 1).expect("a style 10 type")
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
    fn a_skull_closes_fast_from_far_and_dawdles_up_close() {
        assert_eq!(skull_approach(400.0).0, 5.0);
        assert_eq!(skull_approach(320.0).0, 3.0);
        assert_eq!(skull_approach(280.0).0, 1.5);
        assert_eq!(skull_approach(100.0).0, 1.0, "and barely moves once there");
    }

    #[test]
    fn a_skull_charges_after_ten_seconds_of_circling() {
        let tiles = Void;
        let mut s = skull(34);
        let (cx, cy) = s.center();
        let t = Some(player_at(cx + 100.0, cy));
        s.ai[1] = SKULL_CHARGE_AT - 1.0;
        let before = s.velocity;
        update(&mut s, &world(&tiles, t), &mut rng());
        let jitter = (s.velocity.0 - before.0).abs();
        s.velocity = before;
        s.ai[1] = SKULL_CHARGE_AT + 1.0;
        update(&mut s, &world(&tiles, t), &mut rng());
        let charge = (s.velocity.0 - before.0).abs();
        assert!(
            charge > jitter * 2.0,
            "the charge should be much harder: {charge} against {jitter}"
        );
    }

    #[test]
    fn the_charge_ends_and_the_cycle_starts_again() {
        let tiles = Void;
        let mut s = skull(34);
        let (cx, cy) = s.center();
        let t = Some(player_at(cx + 100.0, cy));
        s.ai[1] = SKULL_CHARGE_OVER;
        update(&mut s, &world(&tiles, t), &mut rng());
        assert_eq!(s.ai[1], 0.0);
    }

    #[test]
    fn a_skull_up_close_jitters_rather_than_steering() {
        let tiles = Void;
        let mut s = skull(34);
        let (cx, cy) = s.center();
        // Directly on top of the player, so any movement can only be the jitter.
        let t = Some(player_at(cx + 20.0, cy));
        let mut turns = 0;
        let mut last = s.ai[0];
        for _ in 0..1000 {
            update(&mut s, &world(&tiles, t), &mut rng());
            if s.ai[0] < last {
                turns += 1;
            }
            last = s.ai[0];
        }
        assert!(turns > 0, "the sawtooth should have wrapped at least once");
    }

    #[test]
    fn a_giant_skull_spits_on_a_cycle_and_only_within_range() {
        let tiles = Void;
        let mut s = skull(289);
        let (cx, cy) = s.center();
        let close = Some(player_at(cx + 300.0, cy));
        let mut shot = None;
        for _ in 0..400 {
            if let Some(sh) = update(&mut s, &world(&tiles, close), &mut rng()).shot {
                shot = Some(sh);
                break;
            }
        }
        let shot = shot.expect("should have spat");
        assert_eq!(shot.projectile, GIANT_SKULL_SHOT_TYPE);
        assert_eq!(shot.damage, GIANT_SKULL_SHOT_DAMAGE);
        let speed = (shot.velocity.0.powi(2) + shot.velocity.1.powi(2)).sqrt();
        assert!((speed - GIANT_SKULL_SHOT_SPEED).abs() < 1e-3);

        let mut far_off = skull(289);
        let far = Some(player_at(cx + GIANT_SKULL_RANGE + 200.0, cy));
        for _ in 0..600 {
            assert!(
                update(&mut far_off, &world(&tiles, far), &mut rng())
                    .shot
                    .is_none()
            );
        }
    }

    #[test]
    fn an_ordinary_cursed_skull_spits_nothing() {
        let tiles = Void;
        let mut s = skull(34);
        let (cx, cy) = s.center();
        let t = Some(player_at(cx + 200.0, cy));
        for _ in 0..600 {
            let out = update(&mut s, &world(&tiles, t), &mut rng());
            assert!(out.shot.is_none() && out.spawn.is_none());
        }
    }

    /// `NPC.cs:21965-21998`: type 694 has a second attack cycle that ends in a Water Sphere.
    /// Before this it did the dormancy and then behaved like a plain cursed skull with no attack.
    #[test]
    fn a_water_bolt_mimic_conjures_a_water_sphere() {
        let tiles = Void;
        let mut m = skull(WATER_BOLT_MIMIC);
        let (cx, cy) = m.center();
        // Inside the cast range but outside the hold band, so only the cast is available.
        let t = Some(player_at(cx + 420.0, cy));
        let mut r = rng();
        let mut sphere = None;
        for _ in 0..400 {
            let out = update(&mut m, &world(&tiles, t), &mut r);
            if out.spawn.is_some() {
                sphere = out.spawn;
                break;
            }
            m.position = (10_000.0, 10_000.0);
            m.velocity = (0.0, 0.0);
        }
        let s = sphere.expect("it should have conjured");
        assert_eq!(s.npc_type, WATER_SPHERE);
        assert_eq!(s.velocity, (0.0, 0.0), "the sphere aims itself");
    }

    /// `NPC.cs:21686`, `:21721-21730`: while it is holding, nothing moves it at all.
    #[test]
    fn a_holding_mimic_does_not_move() {
        let tiles = Void;
        let mut m = skull(WATER_BOLT_MIMIC);
        m.ai[3] = 2.0;
        m.velocity = (3.0, 2.0);
        let (cx, cy) = m.center();
        let t = Some(player_at(cx + 200.0, cy));
        let held = m.velocity;
        update(&mut m, &world(&tiles, t), &mut rng());
        assert_eq!(m.velocity, held, "it plants itself");
    }

    /// `NPC.cs:21736-21742`: right on top of you it stops winding up to charge.
    #[test]
    fn a_mimic_in_your_face_holds_its_charge_back() {
        let tiles = Void;
        let mut m = skull(WATER_BOLT_MIMIC);
        let (cx, cy) = m.center();
        let t = Some(player_at(cx + 40.0, cy));
        m.ai[1] = 10.0;
        update(&mut m, &world(&tiles, t), &mut rng());
        assert_eq!(m.ai[1], MIMIC_HOLD_FOR, "the charge timer is pushed back");
    }

    #[test]
    fn a_water_bolt_mimic_plays_dead_until_it_is_hit() {
        let tiles = Void;
        let mut m = skull(694);
        m.ai[3] = 3.0;
        let (cx, cy) = m.center();
        let t = Some(player_at(cx + 100.0, cy));
        for _ in 0..100 {
            update(&mut m, &world(&tiles, t), &mut rng());
        }
        assert_eq!(m.velocity, (0.0, 0.0), "it should not have stirred");
        assert_eq!(m.ai[3], 3.0);

        m.was_hurt = true;
        update(&mut m, &world(&tiles, t), &mut rng());
        assert_eq!(m.ai[3], 4.0, "should have woken");
    }

    #[test]
    fn a_woken_mimic_shakes_itself_off_and_then_hunts() {
        let tiles = Void;
        let mut m = skull(694);
        m.ai[3] = 4.0;
        let (cx, cy) = m.center();
        let t = Some(player_at(cx + 100.0, cy));
        m.ai[1] = 81.0;
        update(&mut m, &world(&tiles, t), &mut rng());
        assert_eq!(m.ai[3], 0.0, "and is now a mimic like any other");
    }
}
