//! The Dreadnautilus: style 117.
//!
//! It rises out of the ground when the blood moon fishes it up, takes a station three hundred
//! pixels to one side and two hundred above you, and then cycles attacks off a counter rather than
//! a die roll: every seventh is the summon, every other one is the spray, and the rest are charges.
//! The cycle is fixed, so the fight is learnable — what varies is where you are standing.
//!
//! The charge is the interesting one. It winds up for a second and a half, *reflecting projectiles
//! the whole time*, and then rams for three seconds along the line its mouth is pointing —
//! backwards, because the mouth trails. It steers only faintly, so a charge is dodged by moving
//! rather than by outrunning it.
//!
//! Daylight or the end of the blood moon takes its target away entirely, and it goes back to
//! drifting.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    NAUTILUS_ABOVE, NAUTILUS_ACCEL, NAUTILUS_ARRIVED, NAUTILUS_CHARGE_HOMING, NAUTILUS_CHARGE_MIN,
    NAUTILUS_CHARGE_SPEED, NAUTILUS_CHARGE_TICKS, NAUTILUS_CHARGE_WINDUP, NAUTILUS_EMERGE_AT,
    NAUTILUS_EMERGE_RISE, NAUTILUS_EMERGE_TICKS, NAUTILUS_FADE_IN, NAUTILUS_HOLD,
    NAUTILUS_MOUTH_ANGLE, NAUTILUS_MOUTH_REACH, NAUTILUS_SPEED, NAUTILUS_SPRAY_BURSTS,
    NAUTILUS_SPRAY_COUNT, NAUTILUS_SPRAY_DAMAGE, NAUTILUS_SPRAY_RECOIL, NAUTILUS_SPRAY_SPEED,
    NAUTILUS_SPRAY_SPREAD, NAUTILUS_SPRAY_TICKS, NAUTILUS_SPRAY_WINDUP, NAUTILUS_STANDOFF,
    NAUTILUS_SUMMON_AT, NAUTILUS_SUMMON_TICKS,
};
use terrustia_proto::projectile::ids::NAUTILUS_SPRAY_SHOT;

use super::drifters::{Outcome, simple_fly};
use crate::game::ai::{Shot, World};
use crate::game::npc::{Npc, TileView};

/// The states, as `ai[0]` numbers them. `-1` is the emergence.
mod state {
    pub const EMERGING: f32 = -1.0;
    pub const STATION: f32 = 0.0;
    pub const CHARGING: f32 = 1.0;
    pub const SPRAYING: f32 = 2.0;
    pub const SUMMONING: f32 = 3.0;
}

/// What it decided this tick.
#[derive(Debug, Default)]
pub struct NautilusOutcome {
    pub base: Outcome,
    /// Places it wants a helper portal opened, if any.
    pub summons: Vec<(f32, f32)>,
}

/// Where its mouth is and which way that points.
///
/// The mouth trails the body by twenty-seven degrees, which is why a charge travels *backwards*
/// along this direction rather than forwards.
fn mouth(npc: &Npc) -> ((f32, f32), (f32, f32)) {
    let mut angle = npc.rotation + NAUTILUS_MOUTH_ANGLE * f32::from(npc.sprite_direction);
    if npc.sprite_direction == -1 {
        angle += std::f32::consts::PI;
    }
    let direction = (angle.cos(), angle.sin());
    let (cx, cy) = npc.center();
    (
        (
            cx + direction.0 * NAUTILUS_MOUTH_REACH,
            cy + direction.1 * NAUTILUS_MOUTH_REACH,
        ),
        direction,
    )
}

/// Turn `rotation` toward `wanted` by at most `step`, the short way round.
fn turn_toward(rotation: f32, wanted: f32, step: f32) -> f32 {
    let mut delta = (wanted - rotation) % std::f32::consts::TAU;
    if delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    }
    if delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    rotation + delta.clamp(-step, step)
}

/// Style 117.
///
/// `helpers` is how many of its called-in helpers are already about, since it will not stack more
/// than three.
pub fn dreadnautilus(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    helpers: usize,
    rng: &mut SmallRng,
) -> NautilusOutcome {
    let mut out = NautilusOutcome::default();
    npc.dirty = true;

    // It arrives invisible and rises into view.
    if npc.local_ai[0] == 0.0 {
        npc.local_ai[0] = 1.0;
        npc.alpha = 255;
        npc.ai[0] = state::EMERGING;
    }

    // Daylight or a finished blood moon leaves it with nothing to hunt.
    let target = world
        .target
        .filter(|t| t.alive)
        .filter(|_| !world.conditions.day && world.conditions.blood_moon);
    let Some(target) = target else {
        npc.velocity.0 *= 0.98;
        npc.velocity.1 *= 0.98;
        return out;
    };
    let (cx, cy) = npc.center();
    let to_player = (target.center.0 - cx, target.center.1 - cy);
    let reach = to_player.0.hypot(to_player.1);

    // What to change to at the end of this tick, if anything.
    let mut next: Option<f32> = None;

    match npc.ai[0] {
        s if s == state::EMERGING => {
            npc.velocity.0 *= 0.98;
            npc.velocity.1 *= 0.98;
            if to_player.0 != 0.0 {
                npc.direction = to_player.0.signum() as i8;
                npc.sprite_direction = -npc.direction;
            }
            // It only starts climbing once it has been down there a moment.
            if npc.ai[2] > NAUTILUS_EMERGE_AT {
                npc.velocity.1 = NAUTILUS_EMERGE_RISE;
                npc.alpha = (npc.alpha - NAUTILUS_FADE_IN).max(0);
            }
            npc.ai[2] += 1.0;
            if npc.ai[2] >= NAUTILUS_EMERGE_TICKS {
                next = Some(state::STATION);
            }
        }

        s if s == state::STATION => {
            // `ai[2]` holds which side it took station on, so it does not swap every tick.
            let station = (
                target.center.0 - npc.ai[2] * NAUTILUS_STANDOFF,
                target.center.1 - NAUTILUS_ABOVE,
            );
            let gap = (station.0 - cx, station.1 - cy);
            if gap.0.hypot(gap.1) > NAUTILUS_ARRIVED {
                let length = gap.0.hypot(gap.1).max(f32::MIN_POSITIVE);
                simple_fly(
                    npc,
                    (
                        gap.0 / length * NAUTILUS_SPEED,
                        gap.1 / length * NAUTILUS_SPEED,
                    ),
                    NAUTILUS_ACCEL,
                );
            }
            npc.direction = if cx < target.center.0 { 1 } else { -1 };
            aim_at(npc, to_player, 0.02);

            npc.ai[1] += 1.0;
            if npc.ai[1] > NAUTILUS_HOLD {
                // The cycle: every seventh is the summon, every other one the spray.
                let attack = npc.ai[3] as i32;
                next = Some(if attack % 7 == 3 {
                    state::SUMMONING
                } else if attack % 2 == 0 {
                    state::SPRAYING
                } else {
                    state::CHARGING
                });
            }
        }

        s if s == state::CHARGING => {
            npc.direction = if cx < target.center.0 { -1 } else { 1 };
            if npc.ai[1] < NAUTILUS_CHARGE_WINDUP {
                // Winding up. Vanilla makes shots bounce off it for the whole wind-up
                // (`NPC.cs:47987`, `reflectsProjectiles = flag`), which is the tell. Reflection
                // itself is not modelled here and cannot be: it lives entirely in the
                // projectile-versus-NPC path (`Projectile.cs:12781-12790`), and a player's shots
                // are the client's to simulate on this server. What matters is that
                // `AI_117_BloodNautilus` never touches `dontTakeDamage`, so the wind-up is a
                // window to hit it in, not a window where it is untouchable. This used to raise a
                // flag the dispatch read as invulnerability, costing melee builds ninety ticks per
                // charge at exactly the moment it sits still and closest.
                npc.velocity.0 *= 0.95;
                npc.velocity.1 *= 0.95;
                aim_at(npc, (-to_player.0, -to_player.1), 0.02);
            } else {
                aim_at(npc, (-to_player.0, -to_player.1), 0.05);
                let (_, direction) = mouth(npc);
                if reach > NAUTILUS_CHARGE_MIN {
                    // Backwards along the mouth line, with the faintest pull toward the player.
                    let homing = (
                        to_player.0 / reach * NAUTILUS_CHARGE_HOMING,
                        to_player.1 / reach * NAUTILUS_CHARGE_HOMING,
                    );
                    npc.velocity = (
                        direction.0 * NAUTILUS_CHARGE_SPEED + homing.0,
                        direction.1 * NAUTILUS_CHARGE_SPEED + homing.1,
                    );
                }
            }
            npc.ai[1] += 1.0;
            if npc.ai[1] >= NAUTILUS_CHARGE_WINDUP + NAUTILUS_CHARGE_TICKS {
                next = Some(state::STATION);
            }
        }

        s if s == state::SPRAYING => {
            npc.direction = if cx < target.center.0 { 1 } else { -1 };
            aim_at(npc, to_player, 0.2);
            if npc.ai[1] < NAUTILUS_SPRAY_WINDUP {
                npc.velocity.0 *= 0.95;
                npc.velocity.1 *= 0.95;
            } else {
                npc.velocity.0 *= 0.9;
                npc.velocity.1 *= 0.9;
                // Three evenly spaced bursts across the spray.
                let burst_length = NAUTILUS_SPRAY_TICKS / NAUTILUS_SPRAY_BURSTS as f32;
                let into_burst = (npc.ai[1] - NAUTILUS_SPRAY_WINDUP) % burst_length;
                if into_burst as i32 == 0 {
                    let (at, direction) = mouth(npc);
                    // Each burst shoves it backwards, so a spray walks it away from you.
                    npc.velocity.0 += direction.0 * NAUTILUS_SPRAY_RECOIL;
                    npc.velocity.1 += direction.1 * NAUTILUS_SPRAY_RECOIL;
                    let count = rng.random_range(NAUTILUS_SPRAY_COUNT.0..NAUTILUS_SPRAY_COUNT.1);
                    for _ in 0..count {
                        // Each bolt is thrown a few pixels off the line, so a burst is a cone.
                        let jitter = (
                            rng.random_range(-NAUTILUS_SPRAY_SPREAD..NAUTILUS_SPRAY_SPREAD),
                            rng.random_range(-NAUTILUS_SPRAY_SPREAD..NAUTILUS_SPRAY_SPREAD),
                        );
                        out.base.shots.push(Shot {
                            projectile: NAUTILUS_SPRAY_SHOT,
                            damage: NAUTILUS_SPRAY_DAMAGE,
                            position: (at.0 - direction.0 * 5.0, at.1 - direction.1 * 5.0),
                            velocity: (
                                direction.0 * NAUTILUS_SPRAY_SPEED + jitter.0,
                                direction.1 * NAUTILUS_SPRAY_SPEED + jitter.1,
                            ),
                            time_left: 300,
                            ai: [0.0; 3],
                        });
                    }
                }
            }
            npc.ai[1] += 1.0;
            if npc.ai[1] >= NAUTILUS_SPRAY_WINDUP + NAUTILUS_SPRAY_TICKS {
                next = Some(state::STATION);
            }
        }

        _ => {
            // Summoning. It hangs perfectly still and calls three helpers in the first half second.
            npc.direction = if cx < target.center.0 { 1 } else { -1 };
            npc.sprite_direction = npc.direction;
            npc.velocity = (0.0, 0.0);
            npc.rotation = turn_toward(npc.rotation, 0.0, 0.02);
            if NAUTILUS_SUMMON_AT.contains(&npc.ai[1])
                && helpers + out.summons.len() < terrustia_proto::npc_params::NAUTILUS_HELPERS_MAX
                && reach < 2000.0
                && let Some(spot) = helper_spot(world, npc, rng)
            {
                out.summons.push(spot);
            }
            npc.ai[1] += 1.0;
            if npc.ai[1] >= NAUTILUS_SUMMON_TICKS {
                next = Some(state::STATION);
            }
        }
    }

    if let Some(state) = next {
        npc.ai[0] = state;
        npc.ai[1] = 0.0;
        npc.ai[2] = 0.0;
        if state == state::STATION {
            // Take station on whichever side it is now facing.
            npc.ai[2] = f32::from(npc.direction);
        } else {
            npc.ai[3] += 1.0;
        }
    }
    out
}

/// Point the body at `toward`, easing by `step` and flipping cleanly when it changes side.
fn aim_at(npc: &mut Npc, toward: (f32, f32), step: f32) {
    let mut wanted =
        toward.1.atan2(toward.0) - NAUTILUS_MOUTH_ANGLE * f32::from(npc.sprite_direction);
    if npc.sprite_direction == -1 {
        wanted += std::f32::consts::PI;
    }
    if npc.sprite_direction != npc.direction {
        // Changing side mirrors the whole pose rather than spinning it round.
        npc.sprite_direction = npc.direction;
        npc.rotation = -npc.rotation;
        wanted = -wanted;
    }
    npc.rotation = turn_toward(npc.rotation, wanted, step);
}

/// Somewhere near it, out of the ground and not right on top of it, to open a helper portal.
fn helper_spot(
    world: &World<'_, impl TileView>,
    npc: &Npc,
    rng: &mut SmallRng,
) -> Option<(f32, f32)> {
    let tile = crate::game::npc::TILE;
    let (cx, cy) = npc.center();
    let here = ((cx / tile) as i32, (cy / tile) as i32);
    for _ in 0..100 {
        let x = rng.random_range(here.0 - 20..=here.0 + 20);
        let y = rng.random_range(here.1 - 20..=here.1 + 20);
        // Not in the ring right around it, and not right on top of it.
        if (y - here.1).abs() <= 8 && (x - here.0).abs() <= 8 {
            continue;
        }
        let occupied = world.tiles.tile(x, y).is_active();
        if occupied {
            continue;
        }
        // Two tiles of clearance all round, so a helper has somewhere to be.
        let mut clear = true;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let t = world.tiles.tile(x + dx, y + dy);
                if t.is_active() && terrustia_proto::tile_solid::solid(t.block) {
                    clear = false;
                }
            }
        }
        if clear {
            return Some((x as f32 * tile + 8.0, y as f32 * tile + 8.0));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ai::Conditions;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    struct Sea(HashMap<(i32, i32), Tile>);

    impl TileView for Sea {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn night(tiles: &Sea, target: Option<(f32, f32)>) -> World<'_, Sea> {
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
            blood_moon: true,
            ..Conditions::default()
        };
        w
    }

    const DREADNAUTILUS: u16 = 618;

    fn nautilus(x: f32, y: f32) -> Npc {
        Npc::new(DREADNAUTILUS, (x, y), 1).expect("dreadnautilus")
    }

    /// It rises out of the ground and fades in before it does anything else.
    #[test]
    fn it_emerges_before_it_fights() {
        let tiles = Sea(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(117);
        let mut n = nautilus(0.0, 0.0);
        let w = night(&tiles, Some((400.0, 0.0)));

        dreadnautilus(&mut n, &w, 0, &mut rng);
        assert_eq!(n.ai[0], state::EMERGING);
        assert_eq!(n.alpha, 255, "it starts invisible");

        for _ in 0..NAUTILUS_EMERGE_TICKS as i32 + 2 {
            dreadnautilus(&mut n, &w, 0, &mut rng);
        }
        assert_eq!(n.ai[0], state::STATION, "then it takes station");
        assert!(n.alpha < 255, "and it should have faded in");
    }

    /// Daylight, or the blood moon ending, leaves it with nothing to do.
    #[test]
    fn it_does_nothing_in_daylight() {
        let tiles = Sea(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(2);
        let mut n = nautilus(0.0, 0.0);
        n.velocity = (5.0, 5.0);
        let mut w = night(&tiles, Some((400.0, 0.0)));
        w.conditions.day = true;

        for _ in 0..120 {
            let out = dreadnautilus(&mut n, &w, 0, &mut rng);
            assert!(out.base.shots.is_empty(), "it should not attack by day");
        }
        assert!(n.velocity.0.abs() < 1.0, "it should have coasted to a stop");
    }

    /// The attacks run off a counter rather than a die roll, so the pattern repeats.
    #[test]
    fn the_attack_cycle_is_fixed() {
        let tiles = Sea(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(5);
        let mut n = nautilus(0.0, 0.0);
        n.local_ai[0] = 1.0;
        n.ai[0] = state::STATION;
        let w = night(&tiles, Some((400.0, 0.0)));

        let mut order = Vec::new();
        for _ in 0..6000 {
            let before = n.ai[0];
            dreadnautilus(&mut n, &w, 0, &mut rng);
            if n.ai[0] != before && n.ai[0] != state::STATION {
                order.push(n.ai[0]);
            }
            if order.len() >= 8 {
                break;
            }
        }
        assert!(
            order.contains(&state::CHARGING)
                && order.contains(&state::SPRAYING)
                && order.contains(&state::SUMMONING),
            "all three attacks should come round: {order:?}"
        );
    }

    /// It winds a charge up on the spot, and stays hurtable while it does.
    ///
    /// `AI_117_BloodNautilus` never touches `dontTakeDamage` (`NPC.cs:47640-48033`): the wind-up
    /// raises `reflectsProjectiles` only (`NPC.cs:47987`), which bounces shots in the
    /// projectile-versus-NPC path and leaves everything else landing normally. This used to raise a
    /// flag the dispatch turned into full invulnerability for all ninety wind-up ticks, which is
    /// most of the time it spends sitting still and close enough to reach.
    #[test]
    fn a_charge_stays_hurtable_while_it_winds_up() {
        let tiles = Sea(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(3);
        let mut n = nautilus(0.0, 0.0);
        n.local_ai[0] = 1.0;
        n.ai[0] = state::CHARGING;
        let w = night(&tiles, Some((600.0, 0.0)));

        for _ in 0..(NAUTILUS_CHARGE_WINDUP + NAUTILUS_CHARGE_TICKS) as i32 {
            crate::game::ai::run(&mut n, &w, &mut rng);
            assert!(!n.invulnerable, "nothing here makes it untouchable");
        }
        let speed = n.velocity.0.hypot(n.velocity.1);
        assert!(speed > 10.0, "the charge should be fast, got {speed}");
    }

    /// The spray comes in three bursts, and each one shoves it backwards.
    #[test]
    fn the_spray_comes_in_three_bursts() {
        let tiles = Sea(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(4);
        let mut n = nautilus(0.0, 0.0);
        n.local_ai[0] = 1.0;
        n.ai[0] = state::SPRAYING;
        let w = night(&tiles, Some((400.0, 0.0)));

        let mut bursts = 0;
        let mut bolts = 0;
        for _ in 0..(NAUTILUS_SPRAY_WINDUP + NAUTILUS_SPRAY_TICKS) as i32 {
            let out = dreadnautilus(&mut n, &w, 0, &mut rng);
            if !out.base.shots.is_empty() {
                bursts += 1;
                bolts += out.base.shots.len();
            }
        }
        assert_eq!(bursts, NAUTILUS_SPRAY_BURSTS, "three bursts");
        assert!(
            bolts >= bursts as usize * NAUTILUS_SPRAY_COUNT.0 as usize,
            "each burst is a handful of bolts, got {bolts}"
        );
    }

    /// It calls in helpers, and stops once three are about.
    #[test]
    fn it_will_not_call_a_fourth_helper() {
        let tiles = Sea(HashMap::new());
        let w = night(&tiles, Some((400.0, 0.0)));

        let called = |already: usize| {
            let mut rng = SmallRng::seed_from_u64(7);
            let mut n = nautilus(0.0, 0.0);
            n.local_ai[0] = 1.0;
            n.ai[0] = state::SUMMONING;
            (0..NAUTILUS_SUMMON_TICKS as i32)
                .map(|_| dreadnautilus(&mut n, &w, already, &mut rng).summons.len())
                .sum::<usize>()
        };
        assert!(called(0) > 0, "it should call for help");
        assert_eq!(called(3), 0, "but not a fourth");
    }
}
