//! The Twins: styles 30 and 31.
//!
//! Both eyes run the same skeleton and differ in every number inside it, which is why the
//! per-eye figures live in a table rather than in branches here.
//!
//! The skeleton is: hold a station off to one side of you for ten seconds, shooting on a charge
//! that fills faster the more hurt the eye is, then run four dashes straight through you, then
//! settle and start again. Below forty per cent health the eye stops fighting entirely for a little
//! over three seconds — spinning up, then down, reflecting everything — and comes out of it as its
//! second form: half again the damage, ten more armour, and a heavier attack.
//!
//! What separates the two is where they want to be. Retinazer holds three hundred pixels *above*
//! and to the side and will only fire from up there, so it is fought by denying it height.
//! Spazmatism holds level with you at four hundred pixels and fires regardless, so it is fought by
//! moving.
//!
//! Daylight ends the fight: both climb away.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    RETINAZER, RETINAZER_TWIN, SPAZMATISM, SPAZMATISM_TWIN, TWIN_FLEE_CLIMB, TWIN_GET_GOOD_GAIN,
    TWIN_SECOND_DAMAGE, TWIN_SECOND_DEFENSE, TWIN_SHOT_LEAD, TWIN_SHOT_RANGE, TWIN_SHOT_SPREAD,
    TWIN_SPIN_CAP, TWIN_SPIN_RATE, TWIN_SPIN_TICKS, TWIN_TRANSFORM_AT, Twin,
};

use crate::game::ai::{Shot, World, can_see, unit};
use crate::game::npc::{Npc, TileView};

/// Which form an eye is in, from `ai[0]`.
mod form {
    pub const FIRST: f32 = 0.0;
    pub const SPINNING_UP: f32 = 1.0;
    pub const SPINNING_DOWN: f32 = 2.0;
    pub const SECOND: f32 = 3.0;
}

/// What an eye did this tick.
#[derive(Debug, Default)]
pub struct TwinOutcome {
    pub shots: Vec<Shot>,
    /// Set when daylight has sent it home.
    pub fleeing: bool,
}

fn table(npc_type: u16) -> Twin {
    if npc_type == RETINAZER {
        RETINAZER_TWIN
    } else {
        SPAZMATISM_TWIN
    }
}

/// Styles 30 and 31.
pub fn twin(npc: &mut Npc, world: &World<'_, impl TileView>, rng: &mut SmallRng) -> TwinOutcome {
    let mut out = TwinOutcome::default();
    npc.dirty = true;
    let t = table(npc.npc_type);
    let expert = world.conditions.expert;

    let Some(target) = world.target.filter(|p| p.alive) else {
        // Nobody to fight: it climbs out.
        npc.velocity.1 += TWIN_FLEE_CLIMB;
        out.fleeing = true;
        return out;
    };
    // Daylight is the timer on the whole fight.
    if world.conditions.day {
        npc.velocity.1 += TWIN_FLEE_CLIMB;
        out.fleeing = true;
        return out;
    }

    let (cx, cy) = npc.center();
    let to_player = (target.center.0 - cx, target.center.1 - cy);
    let health = npc.life as f32 / npc.life_max.max(1) as f32;

    match npc.ai[0] {
        f if f == form::FIRST => {
            first_form(npc, world, &t, target.center, expert, rng, &mut out);
            // Hurt enough, it stops fighting and changes.
            if health < TWIN_TRANSFORM_AT {
                npc.ai = [form::SPINNING_UP, 0.0, 0.0, 0.0];
            }
        }

        f if f == form::SPINNING_UP || f == form::SPINNING_DOWN => {
            // The transformation: it hangs still, spins up and then down, and comes out changed.
            //
            // BS3-M1: it stays perfectly hurtable throughout. This used to raise a `reflecting`
            // flag the dispatch turned into 200 ticks of invulnerability per eye, which is two
            // mistakes stacked. Vanilla raises `reflectsProjectiles` here only under
            // `IsMechQueenUp` (`NPC.cs:26872-26876`), the Mechdusa fight this server does not
            // model, and even then reflection is a projectile bounce (`Projectile.cs:12781-12790`,
            // `ReflectProjectile`), not immunity to everything.
            if npc.ai[0] == form::SPINNING_UP {
                npc.ai[2] = (npc.ai[2] + TWIN_SPIN_RATE).min(TWIN_SPIN_CAP);
            } else {
                npc.ai[2] = (npc.ai[2] - TWIN_SPIN_RATE).max(0.0);
            }
            npc.rotation += npc.ai[2];
            npc.ai[1] += 1.0;
            if npc.ai[1] >= TWIN_SPIN_TICKS {
                npc.ai[0] += 1.0;
                npc.ai[1] = 0.0;
                if npc.ai[0] == form::SECOND {
                    npc.ai[2] = 0.0;
                }
            }
            coast(npc);
        }

        _ => {
            npc.damage_bonus = TWIN_SECOND_DAMAGE;
            npc.defense = npc.stats.defense + TWIN_SECOND_DEFENSE;
            if npc.npc_type == SPAZMATISM {
                // Spazmatism's second form is a close-range flamethrower-and-dash loop, not
                // Retinazer's hover-and-throw — structurally different, not just re-tabled.
                spazmatism_second_form(npc, world, &t, target.center, expert, rng, &mut out);
            } else {
                second_form(npc, world, &t, target.center, expert, &mut out);
            }
        }
    }

    // It always looks where the player is, whatever it is doing.
    if npc.ai[0] != form::SPINNING_UP && npc.ai[0] != form::SPINNING_DOWN {
        npc.rotation = to_player.1.atan2(to_player.0) - std::f32::consts::FRAC_PI_2;
    }
    out
}

/// The first form: hold station and shoot, then four dashes.
fn first_form(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    t: &Twin,
    player: (f32, f32),
    expert: bool,
    rng: &mut SmallRng,
    out: &mut TwinOutcome,
) {
    let (cx, cy) = npc.center();
    match npc.ai[1] {
        0.0 => {
            // Holding station off to whichever side of you it is already on.
            let side = if cx < player.0 { -1.0 } else { 1.0 };
            let station = (
                player.0 + side * t.station.0 - cx,
                player.1 + t.station.1 - cy,
            );
            let (speed, accel) = if expert {
                (t.speed_expert, t.accel_expert)
            } else {
                (t.speed, t.accel)
            };
            steer(&mut npc.velocity, station, speed, accel);

            npc.ai[2] += 1.0;
            if npc.ai[2] >= t.hover_ticks {
                npc.ai = [form::FIRST, 1.0, 0.0, 0.0];
                return;
            }

            // The charge. Retinazer only fires from above and close; Spazmatism always fires.
            let reach = (player.0 - cx).hypot(player.1 - cy);
            let allowed = !t.shoots_only_from_above
                || (npc.position.1 + npc.height() < player.1 && reach < TWIN_SHOT_RANGE);
            if !allowed {
                return;
            }
            npc.ai[3] += 1.0;
            // In expert the charge fills faster the more hurt it is, which is what makes the
            // second half of the fight busier than the first.
            if expert {
                let health = npc.life as f32 / npc.life_max.max(1) as f32;
                for step in [0.9, 0.8, 0.7, 0.6] {
                    if health < step {
                        npc.ai[3] += 0.3;
                    }
                }
            }
            if npc.ai[3] >= t.shot_charge {
                npc.ai[3] = 0.0;
                let speed = if expert {
                    t.shot_speed_expert
                } else {
                    t.shot_speed
                };
                out.shots.push(aimed_shot(npc, player, t, speed, rng));
            }
        }

        1.0 => {
            // Committing to a dash: aimed once, at speed, and not corrected afterwards. In
            // expert, Spazmatism's dash also climbs as it takes damage (Retinazer's does not:
            // its ramp table is empty).
            let mut speed = if expert {
                t.dash_speed_expert
            } else {
                t.dash_speed
            };
            if expert {
                let health = npc.life as f32 / npc.life_max.max(1) as f32;
                for &(threshold, extra) in t.dash_speed_ramp {
                    if health < threshold {
                        speed += extra;
                    }
                }
            }
            let aim = (player.0 - cx, player.1 - cy);
            npc.velocity = unit(aim, speed);
            npc.ai[1] = 2.0;
        }

        _ => {
            // Running the dash out, then braking at each eye's own rate.
            npc.ai[2] += 1.0;
            if npc.ai[2] >= t.dash_brake_at {
                npc.velocity.0 *= t.dash_decay;
                npc.velocity.1 *= t.dash_decay;
                if npc.velocity.0.abs() < 0.1 {
                    npc.velocity.0 = 0.0;
                }
                if npc.velocity.1.abs() < 0.1 {
                    npc.velocity.1 = 0.0;
                }
            }
            if npc.ai[2] >= t.dash_ticks {
                npc.ai[3] += 1.0;
                npc.ai[2] = 0.0;
                if npc.ai[3] >= t.dashes {
                    npc.ai[1] = 0.0;
                    npc.ai[3] = 0.0;
                } else {
                    npc.ai[1] = 1.0;
                }
            }
        }
    }
    let _ = world;
}

/// The second form: hover and throw the heavy shot, then strafe.
fn second_form(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    t: &Twin,
    player: (f32, f32),
    expert: bool,
    out: &mut TwinOutcome,
) {
    let (cx, cy) = npc.center();
    if npc.ai[1] == 0.0 {
        let station = (
            player.0 + t.second_station.0 - cx,
            player.1 + t.second_station.1 - cy,
        );
        let (mut speed, mut accel) = if expert {
            (t.second_speed_expert, t.second_accel_expert)
        } else {
            (t.second_speed, t.second_accel)
        };
        // For the worthy takes both up by a seventh (`NPC.cs:26944-26948`, `num451 *= 1.15f;
        // num452 *= 1.15f;`). `Conditions::get_good_world` was read by only two routines in the
        // whole workspace, so every other seed-specific behaviour, this one included, was silently
        // the ordinary one.
        if world.conditions.get_good_world {
            speed *= TWIN_GET_GOOD_GAIN;
            accel *= TWIN_GET_GOOD_GAIN;
        }
        steer(&mut npc.velocity, station, speed, accel);

        npc.ai[2] += 1.0;
        if npc.ai[2] >= t.second_hover_ticks {
            npc.ai = [form::SECOND, 1.0, 0.0, 0.0];
            return;
        }
    } else {
        // Strafing past you at a fixed offset, on whichever side it is already on.
        let side = if cx < player.0 { -1.0 } else { 1.0 };
        let station = (player.0 + side * t.strafe_offset - cx, player.1 - cy);
        let (speed, accel) = if expert {
            (t.strafe_speed_expert, t.strafe_accel_expert)
        } else {
            (t.strafe_speed, t.strafe_accel)
        };
        steer(&mut npc.velocity, station, speed, accel);
    }

    // The heavy shot. Its charge fills faster the more hurt the eye is, in every difficulty.
    npc.local_ai[1] += 1.0;
    let health = npc.life as f32 / npc.life_max.max(1) as f32;
    for (step, extra) in [(0.75, 1.0), (0.5, 1.0), (0.25, 1.0), (0.1, 2.0)] {
        if health < step {
            npc.local_ai[1] += extra;
        }
    }
    let target = world.target.filter(|p| p.alive);
    if npc.local_ai[1] > t.second_shot_charge
        && let Some(target) = target
        && can_see(world.tiles, npc, target)
    {
        npc.local_ai[1] = 0.0;
        let speed = if expert {
            t.second_shot_speed_expert
        } else {
            t.second_shot_speed
        };
        // The heavy shot is not scattered, unlike the first form's.
        let aim = unit((player.0 - cx, player.1 - cy), speed);
        out.shots.push(Shot {
            projectile: t.second_shot,
            damage: t.second_shot_damage,
            position: (cx + aim.0 * TWIN_SHOT_LEAD, cy + aim.1 * TWIN_SHOT_LEAD),
            velocity: aim,
            time_left: 600,
            ai: [0.0; 3],
        });
    }
}

/// Spazmatism's second form: hold close off to one side, breathing cursed-inferno flame on a
/// charge that fills roughly twenty times faster than Retinazer's heavy shot, then run six short
/// dashes. Mechanically closer to the first form's dash than to Retinazer's hover-and-strafe,
/// which is why it does not share `second_form` (`NPC.cs:27555-27795`).
fn spazmatism_second_form(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    t: &Twin,
    player: (f32, f32),
    expert: bool,
    rng: &mut SmallRng,
    out: &mut TwinOutcome,
) {
    let (cx, cy) = npc.center();
    match npc.ai[1] {
        0.0 => {
            // Holding close off to whichever side of you it is already on.
            let side = if cx < player.0 { -1.0 } else { 1.0 };
            let station = (
                player.0 + side * t.second_station.0 - cx,
                player.1 + t.second_station.1 - cy,
            );
            let (speed, accel) = if expert {
                (t.second_speed_expert, t.second_accel_expert)
            } else {
                (t.second_speed, t.second_accel)
            };
            steer(&mut npc.velocity, station, speed, accel);

            npc.ai[2] += 1.0;
            if npc.ai[2] >= t.second_hover_ticks {
                npc.ai = [form::SECOND, 1.0, 0.0, 0.0];
                return;
            }

            // The flame only comes while it has line of sight, and its charge fills faster the
            // more hurt the eye is, same as Retinazer's heavy shot.
            let target = world.target.filter(|p| p.alive);
            if let Some(target) = target
                && can_see(world.tiles, npc, target)
            {
                npc.local_ai[1] += 1.0;
                let health = npc.life as f32 / npc.life_max.max(1) as f32;
                for (step, extra) in [(0.75, 1.0), (0.5, 1.0), (0.25, 1.0), (0.1, 2.0)] {
                    if health < step {
                        npc.local_ai[1] += extra;
                    }
                }
                if npc.local_ai[1] > t.second_shot_charge {
                    npc.local_ai[1] = 0.0;
                    let speed = if expert {
                        t.second_shot_speed_expert
                    } else {
                        t.second_shot_speed
                    };
                    let mut aim = unit((player.0 - cx, player.1 - cy), speed);
                    // A little scatter and a little of the eye's own motion, same as vanilla.
                    aim.0 += rng.random_range(-40..=40) as f32 * 0.01;
                    aim.1 += rng.random_range(-40..=40) as f32 * 0.01;
                    aim.0 += npc.velocity.0 * 0.5;
                    aim.1 += npc.velocity.1 * 0.5;
                    out.shots.push(Shot {
                        projectile: t.second_shot,
                        damage: t.second_shot_damage,
                        position: (cx, cy),
                        velocity: aim,
                        time_left: 600,
                        ai: [0.0; 3],
                    });
                }
            }
        }

        1.0 => {
            // Committing to a dash, exactly as the first form does.
            let speed = if expert {
                t.second_dash_speed_expert
            } else {
                t.second_dash_speed
            };
            let aim = (player.0 - cx, player.1 - cy);
            npc.velocity = unit(aim, speed);
            npc.ai[1] = 2.0;
        }

        _ => {
            // Running the dash out, then braking at Spazmatism's own faster rate.
            //
            // BS3-M7: expert winds this counter on half again as fast (`ai[2]++; if
            // (Main.expertMode) ai[2] += 0.5f;`, `NPC.cs:27757-27762`), so the brake lands around
            // tick 33 and the dash ends around 53, not 50 and 80. Six dashes are 320 ticks of
            // expert fight, not 480; without this the whole second form ran at classic pace.
            npc.ai[2] += 1.0;
            if expert {
                npc.ai[2] += 0.5;
            }
            if npc.ai[2] >= t.second_dash_brake_at {
                npc.velocity.0 *= t.second_dash_decay;
                npc.velocity.1 *= t.second_dash_decay;
                if npc.velocity.0.abs() < 0.1 {
                    npc.velocity.0 = 0.0;
                }
                if npc.velocity.1.abs() < 0.1 {
                    npc.velocity.1 = 0.0;
                }
            }
            if npc.ai[2] >= t.second_dash_ticks {
                npc.ai[3] += 1.0;
                npc.ai[2] = 0.0;
                if npc.ai[3] >= t.second_dashes {
                    npc.ai[1] = 0.0;
                    npc.ai[3] = 0.0;
                } else {
                    npc.ai[1] = 1.0;
                }
            }
        }
    }
}

/// A scattered shot, spawned ahead of the eye so it does not appear inside it. The lead and
/// scatter are per-eye: Retinazer's lands fifteen ticks out and Spazmatism's only four, and
/// Spazmatism's spread is tighter (`NPC.cs:26794-26798` vs `27398-27402`).
fn aimed_shot(npc: &Npc, player: (f32, f32), t: &Twin, speed: f32, rng: &mut SmallRng) -> Shot {
    let (cx, cy) = npc.center();
    let mut aim = unit((player.0 - cx, player.1 - cy), speed);
    aim.0 += rng.random_range(-TWIN_SHOT_SPREAD..=TWIN_SHOT_SPREAD) as f32 * t.shot_spread_scale;
    aim.1 += rng.random_range(-TWIN_SHOT_SPREAD..=TWIN_SHOT_SPREAD) as f32 * t.shot_spread_scale;
    Shot {
        projectile: t.shot,
        damage: t.shot_damage,
        position: (cx + aim.0 * t.shot_lead, cy + aim.1 * t.shot_lead),
        velocity: aim,
        time_left: 600,
        ai: [0.0; 3],
    }
}

/// Accelerate toward a wanted offset, doubling the push while still going the wrong way.
fn steer(velocity: &mut (f32, f32), offset: (f32, f32), speed: f32, accel: f32) {
    let wanted = unit(offset, speed);
    for (v, w) in [(&mut velocity.0, wanted.0), (&mut velocity.1, wanted.1)] {
        if *v < w {
            *v += accel;
            if *v < 0.0 && w > 0.0 {
                *v += accel;
            }
        } else if *v > w {
            *v -= accel;
            if *v > 0.0 && w < 0.0 {
                *v -= accel;
            }
        }
    }
}

fn coast(npc: &mut Npc) {
    npc.velocity.0 *= 0.98;
    npc.velocity.1 *= 0.98;
    if npc.velocity.0.abs() < 0.1 {
        npc.velocity.0 = 0.0;
    }
    if npc.velocity.1.abs() < 0.1 {
        npc.velocity.1 = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ai::Conditions;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::SPAZMATISM;
    use terrustia_proto::tile::Tile;

    struct Sky(HashMap<(i32, i32), Tile>);

    impl TileView for Sky {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn night(tiles: &Sky, target: Option<(f32, f32)>) -> World<'_, Sky> {
        let mut w = crate::game::ai::calm(
            tiles,
            target.map(|center| Target {
                slot: 0,
                center,
                velocity: (0.0, 0.0),
                alive: true,
            }),
        );
        w.conditions = Conditions {
            day: false,
            ..Conditions::default()
        };
        w
    }

    fn eye(npc_type: u16, x: f32, y: f32) -> Npc {
        Npc::new(npc_type, (x, y), 1).expect("a twin")
    }

    /// Daylight ends the fight for both of them.
    #[test]
    fn daylight_sends_them_home() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(30);
        for ty in [RETINAZER, SPAZMATISM] {
            let mut e = eye(ty, 0.0, 0.0);
            let mut w = night(&tiles, Some((300.0, 300.0)));
            w.conditions.day = true;
            let out = twin(&mut e, &w, &mut rng);
            assert!(out.fleeing, "it should be leaving");
            assert!(e.velocity.1 < 0.0, "and going up");
        }
    }

    /// MECH-1: the flee has to reach the server as a despawn. The dispatch routes `fleeing` to
    /// `expired`, which zeroes `time_left` (a boss never counts down through `tick_life`). On the
    /// pre-fix code the flag was dropped in the dispatch and both eyes hung in the sky for ever.
    #[test]
    fn daybreak_actually_despawns_them_through_the_dispatch() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(30);
        for ty in [RETINAZER, SPAZMATISM] {
            let mut e = eye(ty, 0.0, 0.0);
            let mut w = night(&tiles, Some((300.0, 300.0)));
            w.conditions.day = true;
            let effects = crate::game::ai::run(&mut e, &w, &mut rng);
            assert!(
                effects.expired,
                "daybreak must reach the server as a despawn"
            );
        }
    }

    /// Retinazer holds above you; Spazmatism holds level. That difference is the whole matchup.
    #[test]
    fn the_two_eyes_want_different_places() {
        let tiles = Sky(HashMap::new());
        let player = (0.0, 0.0);
        let settle = |ty: u16| {
            let mut rng = SmallRng::seed_from_u64(1);
            let mut e = eye(ty, 0.0, 0.0);
            let w = night(&tiles, Some(player));
            for _ in 0..400 {
                twin(&mut e, &w, &mut rng);
                e.position.0 += e.velocity.0;
                e.position.1 += e.velocity.1;
            }
            e.center()
        };
        let (_, retinazer_y) = settle(RETINAZER);
        let (spaz_x, spaz_y) = settle(SPAZMATISM);
        assert!(
            retinazer_y < player.1 - 100.0,
            "Retinazer should be well above you, at {retinazer_y}"
        );
        assert!(
            spaz_y.abs() < retinazer_y.abs(),
            "Spazmatism should be closer to level, at {spaz_y}"
        );
        assert!(spaz_x.abs() > 200.0, "and off to one side, at {spaz_x}");
    }

    /// The first form shoots on a charge, and then runs its dashes.
    #[test]
    fn the_first_form_shoots_and_then_dashes() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(2);
        let mut e = eye(SPAZMATISM, 0.0, 0.0);
        let w = night(&tiles, Some((300.0, 0.0)));

        let mut shots = 0;
        let mut dashed = false;
        for _ in 0..1200 {
            let out = twin(&mut e, &w, &mut rng);
            shots += out.shots.len();
            if e.ai[1] == 2.0 {
                dashed = true;
            }
        }
        assert!(shots > 5, "it should have been firing, got {shots}");
        assert!(dashed, "and then dashed");
    }

    /// Retinazer will not fire from below you, which is what makes height the answer to it.
    #[test]
    fn retinazer_only_fires_from_above() {
        let tiles = Sky(HashMap::new());
        let shots_from = |player_y: f32| {
            let mut rng = SmallRng::seed_from_u64(3);
            let mut e = eye(RETINAZER, 0.0, 0.0);
            // Hold it still so only the firing rule is under test.
            let w = night(&tiles, Some((200.0, player_y)));
            (0..600)
                .map(|_| {
                    let out = twin(&mut e, &w, &mut rng);
                    // Keep it where it started.
                    e.position = (0.0, 0.0);
                    out.shots.len()
                })
                .sum::<usize>()
        };
        assert!(shots_from(400.0) > 0, "from above it should fire");
        assert_eq!(shots_from(-400.0), 0, "from below it should not");
    }

    /// Below forty per cent it stops fighting, spins, and comes out changed.
    #[test]
    fn a_hurt_eye_transforms() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(4);
        let mut e = eye(RETINAZER, 0.0, 0.0);
        let w = night(&tiles, Some((300.0, 300.0)));

        e.life = (e.life_max as f32 * 0.3) as i32;
        twin(&mut e, &w, &mut rng);
        assert_eq!(e.ai[0], form::SPINNING_UP, "it should be changing");

        // BS3-M1: it is hurtable the whole way through. Vanilla only raises `reflectsProjectiles`
        // during the change under `IsMechQueenUp` (`NPC.cs:26872-26876`), and this server has no
        // Mechdusa. Reflection was routed to `invulnerable` in the dispatch, so the change handed
        // each eye a hundred spin-up plus a hundred spin-down ticks of free DPS.
        for _ in 0..(TWIN_SPIN_TICKS as i32 * 2 + 4) {
            crate::game::ai::run(&mut e, &w, &mut rng);
            assert!(!e.invulnerable, "the change must not make it untouchable");
        }
        assert_eq!(e.ai[0], form::SECOND, "and comes out as the second form");
        assert_eq!(e.damage_bonus, TWIN_SECOND_DAMAGE, "hitting harder");
        assert_eq!(
            e.defense,
            e.stats.defense + TWIN_SECOND_DEFENSE,
            "and tougher"
        );
    }

    /// For the worthy hovers a seventh faster (`NPC.cs:26944-26948`).
    ///
    /// `Conditions::get_good_world` was read by only two routines in the whole workspace, so this
    /// seed's Twins were the ordinary ones. The speed and the acceleration both take the 1.15.
    #[test]
    fn for_the_worthy_hovers_faster() {
        let tiles = Sky(HashMap::new());
        let speed_after = |get_good: bool| {
            let mut rng = SmallRng::seed_from_u64(31);
            let mut e = eye(RETINAZER, 0.0, 0.0);
            e.ai[0] = form::SECOND;
            let mut w = night(&tiles, Some((900.0, 900.0)));
            w.conditions.get_good_world = get_good;
            for _ in 0..30 {
                twin(&mut e, &w, &mut rng);
            }
            e.velocity.0.hypot(e.velocity.1)
        };
        let ordinary = speed_after(false);
        let worthy = speed_after(true);
        assert!(
            worthy > ordinary * 1.05,
            "for the worthy should close faster: {worthy} against {ordinary}"
        );
    }

    /// A hurt second form fires faster than a fresh one.
    #[test]
    fn the_second_form_speeds_up_as_it_dies() {
        let tiles = Sky(HashMap::new());
        let w = night(&tiles, Some((0.0, 400.0)));
        let shots_at = |health: f32| {
            let mut rng = SmallRng::seed_from_u64(5);
            let mut e = eye(RETINAZER, 0.0, 0.0);
            e.ai[0] = form::SECOND;
            e.life = (e.life_max as f32 * health) as i32;
            (0..1200)
                .map(|_| twin(&mut e, &w, &mut rng).shots.len())
                .sum::<usize>()
        };
        let fresh = shots_at(0.39);
        let dying = shots_at(0.05);
        assert!(
            dying > fresh,
            "a dying eye should fire more: {dying} vs {fresh}"
        );
    }

    /// B3: Retinazer's classic first-form dash is 12, not the getGoodWorld-inflated 14 it was
    /// wrongly given.
    #[test]
    fn retinazer_dash_speed_is_not_getgoodworld_inflated() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(11);
        let mut e = eye(RETINAZER, 0.0, 0.0);
        let w = night(&tiles, Some((1000.0, 1000.0)));
        e.ai = [form::FIRST, 1.0, 0.0, 0.0];
        twin(&mut e, &w, &mut rng);
        let speed = e.velocity.0.hypot(e.velocity.1);
        assert!(
            (speed - 12.0).abs() < 0.01,
            "classic dash speed should be 12, got {speed}"
        );
    }

    /// B2: Spazmatism's first-form dash is its own — ten short dashes, not Retinazer's four long
    /// ones — and starts at its own base speed of 13, not 14.
    #[test]
    fn spazmatism_first_form_dashes_ten_times_at_its_own_speed() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(12);
        let mut e = eye(SPAZMATISM, 0.0, 0.0);
        let w = night(&tiles, Some((1000.0, 1000.0)));
        e.ai = [form::FIRST, 1.0, 0.0, 0.0];

        let mut dash_starts = 0;
        let mut first_speed = None;
        for _ in 0..3000 {
            if e.ai[1] == 1.0 {
                dash_starts += 1;
            }
            let ai1_before = e.ai[1];
            twin(&mut e, &w, &mut rng);
            if first_speed.is_none() && ai1_before == 1.0 {
                first_speed = Some(e.velocity.0.hypot(e.velocity.1));
            }
            if ai1_before == 2.0 && e.ai[1] == 0.0 {
                break;
            }
        }
        assert_eq!(
            dash_starts, 10,
            "Spazmatism should run ten dashes, not Retinazer's four"
        );
        let speed = first_speed.expect("it should have dashed at least once");
        assert!(
            (speed - 13.0).abs() < 0.01,
            "classic dash speed should be 13, got {speed}"
        );
    }

    /// B1: Spazmatism's second form holds close beside you, not three hundred pixels above —
    /// that station belongs to Retinazer.
    #[test]
    fn spazmatism_second_form_holds_close_not_above() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(13);
        let player = (0.0, 0.0);
        let mut e = eye(SPAZMATISM, 0.0, 0.0);
        let w = night(&tiles, Some(player));
        e.ai = [form::SECOND, 0.0, 0.0, 0.0];
        // Stay well inside both the old and new hover windows (300 and 400 ticks) so a
        // pre-fix run never slips into a later state that would mask the station difference.
        for _ in 0..250 {
            twin(&mut e, &w, &mut rng);
            e.position.0 += e.velocity.0;
            e.position.1 += e.velocity.1;
        }
        let (_, y) = e.center();
        assert!(
            y.abs() < 100.0,
            "Spazmatism's second form should hold near the player's height, not 300px above, at {y}"
        );
    }

    /// B1: Spazmatism's second-form flame charges roughly twenty times faster than Retinazer's
    /// heavy shot (threshold 8 vs 180), not on the same clock.
    #[test]
    fn spazmatism_second_form_flame_charges_far_faster_than_retinazers_heavy_shot() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(14);
        let mut e = eye(SPAZMATISM, 0.0, 0.0);
        let w = night(&tiles, Some((50.0, 0.0)));
        e.ai = [form::SECOND, 0.0, 0.0, 0.0];

        let mut shots = 0;
        for _ in 0..600 {
            let out = twin(&mut e, &w, &mut rng);
            // Keep it in range and in sight so only the charge rate is under test.
            e.position = (0.0, 0.0);
            shots += out.shots.len();
        }
        assert!(
            shots > 20,
            "Spazmatism's flame should fire far more than once every 180 ticks, got {shots} in 600 ticks"
        );
    }

    /// B1: Spazmatism's second form runs six short dashes, the same kind of loop as its first
    /// form's, not Retinazer's indefinite strafe.
    #[test]
    fn spazmatism_second_form_runs_six_dashes() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(15);
        let mut e = eye(SPAZMATISM, 0.0, 0.0);
        let w = night(&tiles, Some((1000.0, 1000.0)));
        e.ai = [form::SECOND, 1.0, 0.0, 0.0];

        let mut dash_starts = 0;
        for _ in 0..3000 {
            if e.ai[1] == 1.0 {
                dash_starts += 1;
            }
            let ai1_before = e.ai[1];
            twin(&mut e, &w, &mut rng);
            if ai1_before == 2.0 && e.ai[1] == 0.0 {
                break;
            }
        }
        assert_eq!(
            dash_starts, 6,
            "Spazmatism's second form should run six dashes and loop back, got {dash_starts}"
        );
    }

    /// BS3-M7: expert runs Spazmatism's second-form dash counter half again as fast (`ai[2]++; if
    /// (Main.expertMode) ai[2] += 0.5f;`, `NPC.cs:27757-27762`), so the dash ends around tick 53
    /// rather than 80 and six of them take 320 ticks instead of 480. The counter used to advance
    /// flat, which left the whole second form running at classic pace in an expert world.
    #[test]
    fn spazmatism_dashes_half_again_as_fast_in_expert() {
        let tiles = Sky(HashMap::new());
        let dash_length = |expert: bool| {
            let mut rng = SmallRng::seed_from_u64(16);
            let mut e = eye(SPAZMATISM, 0.0, 0.0);
            let mut w = night(&tiles, Some((1000.0, 1000.0)));
            w.conditions.expert = expert;
            // Already committed to a dash, so this measures exactly one run of it.
            e.ai = [form::SECOND, 2.0, 0.0, 0.0];
            for tick in 1..1000 {
                twin(&mut e, &w, &mut rng);
                if e.ai[1] != 2.0 {
                    return tick;
                }
            }
            0
        };
        assert_eq!(dash_length(false), 80, "classic runs the full eighty ticks");
        // `ai[2]` reaches eighty on the fifty-fourth tick at 1.5 a tick (53 * 1.5 = 79.5).
        assert_eq!(
            dash_length(true),
            54,
            "expert gets there half again as fast"
        );
    }
}
