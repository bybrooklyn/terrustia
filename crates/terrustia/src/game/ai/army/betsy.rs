//! Style 110: Betsy.
//!
//! The Old One's Army's last champion, and the only boss in the game whose whole fight is written
//! down in advance. Eight slots, cycled: dash, dash, flame breath, dash, fireball run, spin,
//! flame breath, scream. One slot is uncertain — the spin has a one-in-three chance of being
//! skipped for the scream instead — and that is the only randomness in her.
//!
//! Between every attack she returns to the same place, three hundred pixels to one side of you and
//! two hundred up, and waits half a second. So the fight has a pulse: attack, reset, attack. What
//! makes it hard is that the attacks themselves are long, and two of them cross the whole arena.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    BETSY_ARRIVE, BETSY_ATTACK_DAMAGE, BETSY_BREATH_APPROACH, BETSY_BREATH_DASH,
    BETSY_BREATH_LINE_UP, BETSY_BREATH_OUT, BETSY_BREATH_RUN, BETSY_BREATH_UP, BETSY_DASH_SPEED,
    BETSY_DASH_TICKS, BETSY_FIREBALL_EVERY, BETSY_FIREBALLS, BETSY_HOVER_ACCEL, BETSY_HOVER_OUT,
    BETSY_HOVER_SPEED, BETSY_HOVER_TICKS, BETSY_HOVER_UP, BETSY_LEAP_AT, BETSY_RUN_APPROACH,
    BETSY_RUN_CLIMB, BETSY_RUN_LINE_UP, BETSY_RUN_OUT, BETSY_RUN_SPEED, BETSY_RUN_UP,
    BETSY_SCREAM_AT, BETSY_SCREAM_CHASE, BETSY_SCREAM_CLOSE, BETSY_SCREAM_TICKS, BETSY_SCRIPT,
    BETSY_SKIPPABLE, BETSY_SPIN_TICKS, BETSY_WYVERN, BETSY_WYVERN_CAP,
};
use terrustia_proto::projectile::ids::{BETSY_FIREBALL, BETSY_FLAME_BREATH};

use crate::game::ai::{Shot, World};
use crate::game::npc::{Npc, TileView};
use crate::game::npc_ai::Spawn;

/// What she is doing, as `ai[0]` numbers it.
mod attack {
    /// Just arrived.
    pub const ARRIVING: f32 = 0.0;
    /// Between attacks.
    pub const HOVER: f32 = 1.0;
    /// A plain dash straight through you.
    pub const DASH: f32 = 2.0;
    /// The flame breath: a long line-up and then a run across the arena.
    pub const BREATH: f32 = 3.0;
    /// The fireball run: further out, faster, six fireballs on the way past.
    pub const RUN: f32 = 4.0;
    /// The spin.
    pub const SPIN: f32 = 5.0;
    /// The scream, which brings wyverns.
    pub const SCREAM: f32 = 6.0;
}

/// What she did this tick.
#[derive(Debug, Default)]
pub struct BetsyOutcome {
    pub shots: Vec<Shot>,
    pub spawn: Vec<Spawn>,
    /// Set on the tick she screams, so the caller can also raise wyverns at the lane portals.
    pub screamed: bool,
}

/// How many wyverns are already out, so she does not fill the arena with them.
pub fn betsy(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    wyverns: usize,
    rng: &mut SmallRng,
) -> BetsyOutcome {
    let mut out = BetsyOutcome::default();
    npc.dirty = true;
    let Some(target) = world.target.filter(|t| t.alive) else {
        return out;
    };
    let player = target.center;
    let (cx, cy) = npc.center();
    let toward = unit((player.0 - cx, player.1 - cy));
    let side = |from: f32, to: f32| if from < to { 1.0f32 } else { -1.0 };

    match npc.ai[0] {
        attack::ARRIVING => {
            npc.ai[1] += 1.0;
            if npc.ai[1] >= BETSY_ARRIVE {
                npc.ai[1] = 0.0;
                npc.ai[0] = attack::HOVER;
                npc.ai[2] = 0.0;
            }
        }
        attack::HOVER => {
            // She holds station to one side of you — whichever side she was already on.
            if npc.ai[2] == 0.0 {
                npc.ai[2] = side(cx, player.0);
            }
            let spot = (
                player.0 - npc.ai[2] * BETSY_HOVER_OUT,
                player.1 - BETSY_HOVER_UP,
            );
            let to = unit((spot.0 - cx, spot.1 - cy));
            super::super::hardmode::drifters::simple_fly(
                npc,
                (to.0 * BETSY_HOVER_SPEED, to.1 * BETSY_HOVER_SPEED),
                BETSY_HOVER_ACCEL,
            );
            npc.direction = side(cx, player.0) as i8;
            npc.sprite_direction = npc.direction;

            npc.ai[1] += 1.0;
            if npc.ai[1] >= BETSY_HOVER_TICKS {
                begin_next(npc, player, toward, rng);
            }
        }
        attack::DASH => {
            // Nothing to steer: the direction was fixed as the dash began.
            npc.ai[1] += 1.0;
            if npc.ai[1] >= BETSY_DASH_TICKS {
                back_to_hover(npc);
            }
        }
        attack::BREATH => breath(npc, player, &mut out),
        attack::RUN => run(npc, player, &mut out),
        attack::SPIN => spin(npc, player),
        _ => scream(npc, player, wyverns, rng, &mut out),
    }

    aim_sprite(npc, player);
    out
}

/// Pick the next attack off the script and set it up.
fn begin_next(npc: &mut Npc, player: (f32, f32), toward: (f32, f32), rng: &mut SmallRng) {
    // The one uncertain slot: the spin is sometimes skipped for the scream.
    if npc.ai[3] as usize == BETSY_SKIPPABLE && rng.random_range(0..3) == 0 {
        npc.ai[3] += 1.0;
    }
    let slot = (npc.ai[3] as usize).min(BETSY_SCRIPT.len() - 1);
    let next = BETSY_SCRIPT[slot] as f32;
    npc.ai[0] = next;
    npc.ai[1] = 0.0;
    npc.ai[2] = 0.0;
    npc.ai[3] += 1.0;
    if npc.ai[3] >= BETSY_SCRIPT.len() as f32 {
        npc.ai[3] = 0.0;
    }

    let (cx, _) = npc.center();
    match next {
        attack::DASH => {
            npc.sprite_direction = if toward.0 > 0.0 { 1 } else { -1 };
            npc.rotation = facing_rotation(toward, npc.sprite_direction);
            npc.velocity = (toward.0 * BETSY_DASH_SPEED, toward.1 * BETSY_DASH_SPEED);
        }
        attack::BREATH => {
            // She backs off before the breath rather than closing.
            let away = if player.0 > cx { 1.0 } else { -1.0 };
            npc.sprite_direction = away as i8;
            npc.velocity = (away * -2.0, 0.0);
        }
        attack::SPIN => {
            npc.sprite_direction = if toward.0 > 0.0 { 1 } else { -1 };
            npc.rotation = facing_rotation(toward, npc.sprite_direction);
            npc.velocity = (toward.0 * 32.0, toward.1 * 32.0);
        }
        _ => {}
    }
}

fn back_to_hover(npc: &mut Npc) {
    npc.ai[0] = attack::HOVER;
    npc.ai[1] = 0.0;
    npc.ai[2] = 0.0;
}

/// The flame breath: line up six hundred pixels out, then breathe and run through.
fn breath(npc: &mut Npc, player: (f32, f32), out: &mut BetsyOutcome) {
    npc.ai[1] += 1.0;
    let (cx, cy) = npc.center();
    npc.ai[2] = if cx < player.0 { 1.0 } else { -1.0 };

    if npc.ai[1] < BETSY_BREATH_LINE_UP {
        // The line-up is not flight: she is moved straight there, which is why it always works.
        let spot = (
            player.0 - npc.ai[2] * BETSY_BREATH_OUT,
            player.1 - BETSY_BREATH_UP,
        );
        let gap = (spot.0 - cx, spot.1 - cy);
        let far = gap.0.hypot(gap.1);
        if far < BETSY_BREATH_APPROACH {
            npc.position = (spot.0 - npc.width() / 2.0, spot.1 - npc.height() / 2.0);
        } else {
            let to = unit(gap);
            npc.position.0 += to.0 * BETSY_BREATH_APPROACH;
            npc.position.1 += to.1 * BETSY_BREATH_APPROACH;
        }
        if far < 16.0 {
            npc.ai[1] = BETSY_BREATH_LINE_UP - 1.0;
        }
        return;
    }

    if npc.ai[1] == BETSY_BREATH_LINE_UP {
        let facing = if player.0 > cx { 1.0 } else { -1.0 };
        npc.velocity = (facing * BETSY_BREATH_DASH, 0.0);
        npc.direction = facing as i8;
        npc.sprite_direction = npc.direction;
        out.shots.push(Shot {
            projectile: BETSY_FLAME_BREATH,
            damage: BETSY_ATTACK_DAMAGE,
            position: (cx, cy),
            velocity: npc.velocity,
            time_left: BETSY_BREATH_RUN as u16,
            ai: [0.0; 3],
        });
    }

    // Past five hundred and fifty pixels she opens up, so the run always finishes.
    if (player.0 - cx).abs() > 550.0 && npc.velocity.0.abs() < 20.0 {
        npc.velocity.0 += npc.velocity.0.signum() * 0.5;
    }
    if npc.ai[1] >= BETSY_BREATH_LINE_UP + BETSY_BREATH_RUN {
        back_to_hover(npc);
    }
}

/// The fireball run: fifteen hundred pixels out, then a flat charge with six fireballs on the way.
fn run(npc: &mut Npc, player: (f32, f32), out: &mut BetsyOutcome) {
    let (cx, cy) = npc.center();
    npc.ai[2] = if cx < player.0 { 1.0 } else { -1.0 };

    if npc.ai[1] < BETSY_RUN_LINE_UP {
        let spot = (
            player.0 - npc.ai[2] * BETSY_RUN_OUT,
            player.1 - BETSY_RUN_UP,
        );
        let to = unit((spot.0 - cx, spot.1 - cy));
        let wanted = (to.0 * BETSY_RUN_APPROACH, to.1 * BETSY_RUN_APPROACH);
        npc.velocity.0 += (wanted.0 - npc.velocity.0) / 30.0;
        npc.velocity.1 += (wanted.1 - npc.velocity.1) / 30.0;
        npc.direction = if cx < player.0 { 1 } else { -1 };
        npc.sprite_direction = npc.direction;
        if (spot.0 - cx).hypot(spot.1 - cy) < 16.0 {
            npc.ai[1] = BETSY_RUN_LINE_UP - 1.0;
        }
    } else if npc.ai[1] == BETSY_RUN_LINE_UP {
        // The charge is flattened: she comes in almost level however far below you she was.
        let mut aim = (player.0 - cx, player.1 - cy);
        aim.1 *= 0.25;
        let aim = unit(aim);
        npc.sprite_direction = if aim.0 > 0.0 { 1 } else { -1 };
        npc.rotation = facing_rotation(aim, npc.sprite_direction);
        npc.velocity = (aim.0 * BETSY_RUN_SPEED, aim.1 * BETSY_RUN_SPEED);
    } else {
        // Mid-charge she is dragged along rather than flying: steady, and unturnable.
        let across = unit((player.0 - cx, player.1 - cy)).0;
        let up = unit((player.0 - cx, player.1 - 400.0 - cy)).1;
        npc.position.0 += across * 7.0;
        npc.position.1 += up * 6.0;

        let along = (npc.ai[1] - BETSY_RUN_LINE_UP + 1.0) as i32;
        if along <= BETSY_FIREBALLS * BETSY_FIREBALL_EVERY && along % BETSY_FIREBALL_EVERY == 0 {
            // They leave from her mouth, which is a hundred and forty pixels out along her nose.
            let (sin, cos) = npc.rotation.sin_cos();
            let nose = (140.0 * f32::from(npc.direction), 20.0);
            out.shots.push(Shot {
                projectile: BETSY_FIREBALL,
                damage: BETSY_ATTACK_DAMAGE,
                position: (
                    cx + nose.0 * cos - nose.1 * sin,
                    cy + nose.0 * sin + nose.1 * cos,
                ),
                velocity: npc.velocity,
                time_left: 600,
                ai: [0.0; 3],
            });
        }
    }

    let total =
        BETSY_RUN_LINE_UP + (BETSY_FIREBALLS * BETSY_FIREBALL_EVERY) as f32 + BETSY_RUN_CLIMB;
    if npc.ai[1] > total - BETSY_RUN_CLIMB {
        npc.velocity.1 -= 0.1;
    }
    npc.ai[1] += 1.0;
    if npc.ai[1] >= total {
        back_to_hover(npc);
    }
}

/// The spin: one full turn in a second, drifting onto you the whole time.
fn spin(npc: &mut Npc, player: (f32, f32)) {
    let turn = std::f32::consts::TAU / BETSY_SPIN_TICKS * f32::from(npc.direction);
    let (sin, cos) = (-turn).sin_cos();
    npc.velocity = (
        npc.velocity.0 * cos - npc.velocity.1 * sin,
        npc.velocity.0 * sin + npc.velocity.1 * cos,
    );
    npc.position.1 -= 0.1;
    let (cx, cy) = npc.center();
    let onto = unit((player.0 - cx, player.1 - cy));
    npc.position.0 += onto.0 * 10.0;
    npc.position.1 += onto.1 * 10.0;
    npc.rotation -= turn;

    npc.ai[1] += 1.0;
    if npc.ai[1] >= BETSY_SPIN_TICKS {
        back_to_hover(npc);
        npc.velocity = (npc.velocity.0 / 2.0, npc.velocity.1 / 2.0);
    }
}

/// The scream: close first, then hold still and call wyverns down three times.
fn scream(
    npc: &mut Npc,
    player: (f32, f32),
    wyverns: usize,
    rng: &mut SmallRng,
    out: &mut BetsyOutcome,
) {
    let (cx, cy) = npc.center();
    if npc.ai[1] == 0.0 {
        // She closes twice as hard as she hovers, and gives up chasing after three seconds.
        let spot = (player.0, player.1 - BETSY_HOVER_UP);
        let to = unit((spot.0 - cx, spot.1 - cy));
        super::super::hardmode::drifters::simple_fly(
            npc,
            (
                to.0 * BETSY_HOVER_SPEED * 2.0,
                to.1 * BETSY_HOVER_SPEED * 2.0,
            ),
            BETSY_HOVER_ACCEL * 2.0,
        );
        npc.direction = if cx < player.0 { 1 } else { -1 };
        npc.sprite_direction = npc.direction;
        npc.ai[2] += 1.0;
        if (player.0 - cx).hypot(player.1 - cy) < BETSY_SCREAM_CLOSE
            || npc.ai[2] >= BETSY_SCREAM_CHASE
        {
            npc.ai[1] = 1.0;
        }
        return;
    }

    npc.velocity.0 *= if npc.ai[1] < BETSY_LEAP_AT {
        0.95
    } else {
        0.98
    };
    npc.velocity.1 *= if npc.ai[1] < BETSY_LEAP_AT {
        0.95
    } else {
        0.98
    };
    if npc.ai[1] == BETSY_LEAP_AT {
        // The scream throws her upward as it lands.
        if npc.velocity.1 > 0.0 {
            npc.velocity.1 /= 3.0;
        }
        npc.velocity.1 -= 3.0;
    }

    if BETSY_SCREAM_AT.contains(&npc.ai[1]) && wyverns <= BETSY_WYVERN_CAP {
        // One comes down out of the sky around her, on an ellipse, and never on top of you.
        let angle = rng.random::<f32>() * std::f32::consts::TAU;
        let reach = 300.0 * (0.6 + rng.random::<f32>() * 0.4);
        let at = (cx + angle.cos() * 2.0 * reach, cy + angle.sin() * reach);
        if (at.0 - player.0).hypot(at.1 - player.1) > 100.0 {
            out.spawn.push(Spawn {
                handle: None,
                npc_type: BETSY_WYVERN,
                position: at,
                velocity: (0.0, 0.0),
                parent: Some(Spawn::OWN_PARENT),
                ai: [None; 4],
            });
        }
        out.screamed = true;
    }

    npc.ai[1] += 1.0;
    if npc.ai[1] >= BETSY_SCREAM_TICKS {
        back_to_hover(npc);
    }
}

/// Turn toward where the attack wants her pointed, at the rate that attack turns.
fn aim_sprite(npc: &mut Npc, player: (f32, f32)) {
    let (cx, cy) = npc.center();
    let mut wanted = (player.1 - cy).atan2(player.0 - cx);
    let mut rate = 0.04;
    match npc.ai[0] {
        attack::DASH | attack::SPIN => rate = 0.0,
        attack::BREATH => {
            rate = 0.01;
            wanted = 0.0;
            if npc.sprite_direction == -1 {
                wanted -= std::f32::consts::PI;
            }
            if npc.ai[1] >= BETSY_BREATH_LINE_UP {
                // Mid-breath she cants, which is what makes the flame sweep rather than sit.
                wanted += f32::from(npc.sprite_direction) * std::f32::consts::PI / 12.0;
                rate = 0.05;
            }
        }
        attack::RUN => {
            rate = 0.01;
            wanted = std::f32::consts::PI;
            if npc.sprite_direction == 1 {
                wanted += std::f32::consts::PI;
            }
        }
        attack::SCREAM => {
            rate = 0.02;
            wanted = 0.0;
            if npc.sprite_direction == -1 {
                wanted -= std::f32::consts::PI;
            }
        }
        _ => {}
    }
    if npc.sprite_direction == -1 {
        wanted += std::f32::consts::PI;
    }
    if rate != 0.0 {
        npc.rotation = angle_towards(npc.rotation, wanted, rate);
    }
}

/// Turn one angle toward another by at most a step, the short way round.
fn angle_towards(from: f32, to: f32, step: f32) -> f32 {
    let mut delta = (to - from) % std::f32::consts::TAU;
    if delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    }
    if delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    from + delta.clamp(-step, step)
}

fn facing_rotation(aim: (f32, f32), sprite_direction: i8) -> f32 {
    let rotation = aim.1.atan2(aim.0);
    if sprite_direction == -1 {
        rotation + std::f32::consts::PI
    } else {
        rotation
    }
}

fn unit(v: (f32, f32)) -> (f32, f32) {
    let length = v.0.hypot(v.1).max(f32::MIN_POSITIVE);
    (v.0 / length, v.1 / length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::DD2_BETSY;
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

    fn her() -> Npc {
        Npc::new(DD2_BETSY, (5000.0, 3000.0), 1).expect("Betsy")
    }

    fn tick(npc: &mut Npc, w: &World<'_, Sky>, tiles: &Sky, rng: &mut SmallRng) -> BetsyOutcome {
        let out = betsy(npc, w, 0, rng);
        npc.no_gravity = true;
        npc.no_tile_collide = true;
        crate::game::npc::step_physics(npc, tiles);
        out
    }

    /// She works through every attack she has, and always comes back to the hover in between.
    #[test]
    fn she_runs_the_whole_script() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, (5000.0, 3200.0));
        let mut rng = SmallRng::seed_from_u64(1);
        let mut n = her();
        let mut order = Vec::new();
        for _ in 0..8000 {
            tick(&mut n, &w, &tiles, &mut rng);
            if order.last() != Some(&n.ai[0]) {
                order.push(n.ai[0]);
            }
        }
        let attacks: Vec<f32> = order
            .iter()
            .cloned()
            .filter(|a| *a != attack::HOVER && *a != attack::ARRIVING)
            .collect();
        for wanted in [
            attack::DASH,
            attack::BREATH,
            attack::RUN,
            attack::SPIN,
            attack::SCREAM,
        ] {
            assert!(attacks.contains(&wanted), "missing {wanted}: {attacks:?}");
        }
        // Every attack is separated by a hover: she never chains two together.
        for pair in order.windows(2) {
            assert!(
                pair[0] == attack::HOVER || pair[1] == attack::HOVER,
                "{pair:?} with no reset between"
            );
        }
    }

    /// The script's order is the game's, save for the one slot that is sometimes skipped.
    #[test]
    fn the_order_is_the_script() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, (5000.0, 3200.0));
        let mut rng = SmallRng::seed_from_u64(2);
        let mut n = her();
        let mut attacks = Vec::new();
        let mut last = n.ai[0];
        for _ in 0..12000 {
            tick(&mut n, &w, &tiles, &mut rng);
            if n.ai[0] != last && n.ai[0] != attack::HOVER && n.ai[0] != attack::ARRIVING {
                attacks.push(n.ai[0] as u8);
            }
            last = n.ai[0];
        }
        assert!(
            attacks.len() >= 8,
            "long enough to see the loop: {attacks:?}"
        );
        for (at, got) in attacks.iter().enumerate() {
            let expected = BETSY_SCRIPT[at % BETSY_SCRIPT.len()];
            // The spin's slot may have been skipped, which shifts what follows for that pass.
            if expected == BETSY_SCRIPT[BETSY_SKIPPABLE] {
                continue;
            }
            assert_eq!(*got, expected, "at {at} of {attacks:?}");
        }
    }

    /// The breath goes off once, mid-attack, not at the start.
    #[test]
    fn the_breath_comes_after_the_line_up() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, (5000.0, 3200.0));
        let mut rng = SmallRng::seed_from_u64(3);
        let mut n = her();
        n.ai[0] = attack::BREATH;
        let mut fired = Vec::new();
        for at in 0..200 {
            for shot in tick(&mut n, &w, &tiles, &mut rng).shots {
                fired.push((at, shot.projectile));
            }
            if n.ai[0] != attack::BREATH {
                break;
            }
        }
        assert_eq!(fired.len(), 1, "one breath: {fired:?}");
        assert_eq!(fired[0].1, BETSY_FLAME_BREATH);
        assert!(fired[0].0 > 0, "and not on the first tick");
    }

    /// The fireball run really throws six, ten ticks apart.
    #[test]
    fn the_run_throws_six_fireballs() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, (5000.0, 3200.0));
        let mut rng = SmallRng::seed_from_u64(4);
        let mut n = her();
        n.ai[0] = attack::RUN;
        let mut fired = Vec::new();
        for at in 0..400 {
            if !tick(&mut n, &w, &tiles, &mut rng).shots.is_empty() {
                fired.push(at);
            }
            if n.ai[0] != attack::RUN {
                break;
            }
        }
        assert_eq!(fired.len(), BETSY_FIREBALLS as usize, "six: {fired:?}");
        for pair in fired.windows(2) {
            assert_eq!(pair[1] - pair[0], BETSY_FIREBALL_EVERY);
        }
    }

    /// The scream brings wyverns, three times, and not once the arena is full of them.
    #[test]
    fn the_scream_brings_wyverns() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, (5000.0, 3200.0));
        let mut rng = SmallRng::seed_from_u64(5);

        let mut n = her();
        n.ai[0] = attack::SCREAM;
        let mut screams = 0;
        let mut called = 0;
        for _ in 0..400 {
            let out = tick(&mut n, &w, &tiles, &mut rng);
            screams += usize::from(out.screamed);
            called += out.spawn.len();
            if n.ai[0] != attack::SCREAM {
                break;
            }
        }
        assert_eq!(screams, BETSY_SCREAM_AT.len(), "she screams three times");
        // Each scream rolls a spot on an ellipse around her and drops the one that would land on
        // top of you, so three screams sometimes bring two.
        assert!(
            (2..=3).contains(&called),
            "and two or three come down: {called}"
        );

        let mut full = her();
        full.ai[0] = attack::SCREAM;
        let mut called = 0;
        for _ in 0..400 {
            let out = betsy(&mut full, &w, 20, &mut rng);
            called += out.spawn.len();
            full.no_gravity = true;
            crate::game::npc::step_physics(&mut full, &tiles);
            if full.ai[0] != attack::SCREAM {
                break;
            }
        }
        assert_eq!(called, 0, "not with twenty already out");
    }

    /// The spin turns her a full circle, and it takes a second.
    #[test]
    fn the_spin_is_one_turn_a_second() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, (5000.0, 3200.0));
        let mut rng = SmallRng::seed_from_u64(6);
        let mut n = her();
        n.ai[0] = attack::SPIN;
        n.rotation = 0.0;
        n.direction = 1;
        let mut turned = 0.0;
        let mut ticks = 0;
        let mut last = n.rotation;
        while n.ai[0] == attack::SPIN && ticks < 300 {
            tick(&mut n, &w, &tiles, &mut rng);
            turned += (n.rotation - last).abs();
            last = n.rotation;
            ticks += 1;
        }
        assert_eq!(ticks, BETSY_SPIN_TICKS as i32, "a second");
        assert!(
            (turned - std::f32::consts::TAU).abs() < 0.2,
            "and one full turn, not {turned}"
        );
    }

    /// With nobody left she stops rather than flying off after nothing.
    #[test]
    fn she_does_nothing_without_a_target() {
        let tiles = Sky(HashMap::new());
        let w = crate::game::ai::calm(&tiles, None);
        let mut rng = SmallRng::seed_from_u64(7);
        let mut n = her();
        let before = (n.ai, n.position);
        for _ in 0..200 {
            betsy(&mut n, &w, 0, &mut rng);
        }
        assert_eq!((n.ai, n.position), before);
    }
}
