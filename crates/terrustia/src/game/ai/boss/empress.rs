//! Style 120: the Empress of Light.
//!
//! She is the game's one boss with no reactive behaviour at all. Everything she does comes off a
//! fixed rotation — ten set pieces, in order, forever — and the only thing that changes it is her
//! own health: at half she stops, teleports, and comes back running a *different* rotation, faster,
//! with two attacks the first one never uses.
//!
//! Between every set piece she returns to a short idle, dashes once toward you, and picks the next
//! one. So the whole fight is a metronome, and learning her is learning the order.
//!
//! Fighting her in daylight enrages her, and an enraged Empress does nine thousand damage with
//! every attack but one: the death aurora is never scaled up (vanilla leaves it a flat forty, even
//! enraged). That is not a difficulty setting, it is the game refusing the fight, and the flag
//! that records it survives her leaving and coming back.
//!
//! **Every shot here passes `time_left: 0`, which means "use the projectile's own".** That is not a
//! detail: three of her five projectiles carry their whole shape in that number. A rainbow streak
//! lives 200 ticks and its arm reads `timeLeft` to decide when to stop drifting (over 140) and
//! start homing (over 30); a lasting rainbow's fade is keyed to 660; a sun dance ends at 180. Every
//! one of them used to be launched with a flat 900, which is longer than all three and left the
//! rainbow streak permanently in its drift phase - so the phases could not have worked even once
//! the arms existed. The one number was wrong in seven places for the same reason, which is why it
//! is stated here rather than seven times below.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    EMPRESS_ARRIVAL, EMPRESS_DAMAGE, EMPRESS_DAMAGE_PHASE_2, EMPRESS_DASH_OUT, EMPRESS_DASH_SPEED,
    EMPRESS_DEATH_AURORA_DAMAGE, EMPRESS_FLY_ACCEL, EMPRESS_FLY_SPEED, EMPRESS_GIVE_UP,
    EMPRESS_IDLE, EMPRESS_IDLE_PHASE_2, EMPRESS_RAINBOW_COUNT, EMPRESS_RAINBOW_SPEED,
    EMPRESS_SCRIPT, EMPRESS_SCRIPT_PHASE_2, EMPRESS_SCRIPT_PHASE_2_EXPERT, EMPRESS_SETTLED,
    EMPRESS_STATION_HIGH, EMPRESS_STATION_LEFT, EMPRESS_STATION_RIGHT, EMPRESS_STATION_RING,
    EMPRESS_WALL_LANCES, EMPRESS_WALL_SPACING,
};
use terrustia_proto::projectile::ids::{
    EMPRESS_BLAST, EMPRESS_DEATH_AURORA, EMPRESS_LANCE, EMPRESS_RAINBOW, EMPRESS_SUN_DANCE,
};

use super::super::hardmode::drifters::simple_fly;
use crate::game::ai::{Shot, World};
use crate::game::npc::{Npc, TileView};
use crate::game::npc_ai::Target;

/// Her attacks, as `ai[0]` numbers them. The gaps are the game's own.
mod attack {
    /// Fading in.
    pub const ARRIVING: f32 = 0.0;
    /// The idle: a dash, a pause, and the next one off the script.
    pub const IDLE: f32 = 1.0;
    /// A stream of blasts from her left hand.
    pub const BLASTS: f32 = 2.0;
    /// The death aurora, planted over your head (`ProjectileID.HallowBossDeathAurora`, 874). This
    /// state used to be called `SUN_DANCE`, after the name 874 was wrongly given; the sun dance is
    /// [`LANCE_RING`]'s ring of 923s.
    pub const DEATH_AURORA: f32 = 3.0;
    /// Prismatic bolts, laid across your path.
    pub const BOLTS: f32 = 4.0;
    /// The everlasting rainbow: a ring of thirteen.
    pub const RAINBOW: f32 = 5.0;
    /// Sun dances (`ProjectileID.FairyQueenSunDance`, 923), in a turning ring.
    pub const LANCE_RING: f32 = 6.0;
    /// Walls of lances, laid across the arena.
    pub const LANCE_WALLS: f32 = 7.0;
    /// The dash, from the left and from the right.
    pub const DASH_LEFT: f32 = 8.0;
    pub const DASH_RIGHT: f32 = 9.0;
    /// The turn: she stops, teleports, and comes back harder.
    pub const TRANSITION: f32 = 10.0;
    /// Bolts aimed behind you rather than across you.
    pub const CHASING_BOLTS: f32 = 11.0;
    /// Blasts in a full circle rather than a fan.
    pub const CIRCLING_BLASTS: f32 = 12.0;
    /// Leaving.
    pub const LEAVING: f32 = 13.0;
}

/// What she did this tick.
#[derive(Debug, Default)]
pub struct EmpressOutcome {
    pub shots: Vec<Shot>,
    pub spent: bool,
}

/// `enraged` is whether it is daylight: the one thing outside the fight that changes it.
pub fn empress(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    enraged: bool,
    expert: bool,
    rng: &mut SmallRng,
) -> EmpressOutcome {
    let mut out = EmpressOutcome::default();
    npc.dirty = true;

    // `ai[3]` carries two facts at once: whether she has turned, and whether this fight was
    // started in daylight. Beginning enraged marks the run, and the mark survives everything.
    if npc.life == npc.life_max && enraged && !genuinely_enraged(npc) {
        npc.ai[3] += 2.0;
    }
    let phase_2 = matches!(npc.ai[3] as i32, 1 | 3);
    let expert = expert || enraged;
    // Slots into `EMPRESS_DAMAGE`/`EMPRESS_DAMAGE_PHASE_2`: blast, rainbow, bolt, sun-dance ring,
    // lance wall. The *death aurora* is not here at all: unlike these five it is never scaled by
    // expert, phase or the enrage override (`EMPRESS_DEATH_AURORA_DAMAGE`'s own doc comment has the
    // citations), so folding it into this closure would make it look like it shared their scaling
    // when vanilla never lets it.
    let damage = |slot: usize| {
        let table = if phase_2 {
            EMPRESS_DAMAGE_PHASE_2
        } else {
            EMPRESS_DAMAGE
        };
        if enraged {
            9999
        } else if expert {
            table[slot].1
        } else {
            table[slot].0
        }
    };
    // Later and harder both make her attacks shorter, which is what tightens the fight.
    let quicker = if phase_2 { 15.0 } else { 0.0 } + if expert { 5.0 } else { 0.0 };

    let target = world.target.filter(|t| t.alive);
    let station = |npc: &mut Npc, at: (f32, f32), speed: f32, accel: f32| {
        let (cx, cy) = npc.center();
        let spot = match target {
            Some(t) => (t.center.0 + at.0, t.center.1 + at.1),
            None => (cx + at.0, cy + at.1),
        };
        let gap = (spot.0 - cx, spot.1 - cy);
        if gap.0.hypot(gap.1) > EMPRESS_SETTLED {
            let to = unit(gap);
            simple_fly(npc, (to.0 * speed, to.1 * speed), accel);
        }
    };

    // She can be hurt except where an attack says otherwise, and hits harder mid-dash.
    npc.invulnerable = false;
    npc.damage_bonus = 1.0;

    match npc.ai[0] {
        attack::ARRIVING => {
            if npc.ai[1] == 0.0 {
                npc.velocity = (0.0, 5.0);
                out.shots
                    .push(planted(npc, EMPRESS_DEATH_AURORA, 0, (0.0, -80.0)));
            }
            npc.velocity.0 *= 0.95;
            npc.velocity.1 *= 0.95;
            npc.ai[1] += 1.0;
            npc.invulnerable = true;
            npc.alpha = (255.0 * (1.0 - (npc.ai[1] / EMPRESS_ARRIVAL).clamp(0.0, 1.0))) as i32;
            if npc.ai[1] >= EMPRESS_ARRIVAL {
                if enraged && !genuinely_enraged(npc) {
                    npc.ai[3] += 2.0;
                }
                npc.ai[0] = attack::IDLE;
                npc.ai[1] = 0.0;
            }
        }
        attack::IDLE => idle(npc, target, phase_2, expert, enraged, npc.ai[2]),
        attack::BLASTS | attack::CIRCLING_BLASTS => {
            let circling = npc.ai[0] == attack::CIRCLING_BLASTS;
            if circling && npc.ai[1] == 0.0 {
                npc.velocity = (0.0, -12.0);
            }
            if circling {
                npc.velocity.0 *= 0.95;
                npc.velocity.1 *= 0.95;
            } else {
                station(
                    npc,
                    EMPRESS_STATION_LEFT,
                    EMPRESS_FLY_SPEED,
                    EMPRESS_FLY_ACCEL,
                );
            }
            blasts(
                npc,
                damage(0),
                circling,
                phase_2 && expert,
                expert,
                rng,
                &mut out,
            );
            npc.ai[1] += 1.0;
            if npc.ai[1] >= 60.0 + (90.0 - quicker) {
                back_to_idle(npc);
            }
        }
        attack::DEATH_AURORA => {
            npc.ai[1] += 1.0;
            station(
                npc,
                EMPRESS_STATION_RIGHT,
                EMPRESS_FLY_SPEED,
                EMPRESS_FLY_ACCEL,
            );
            if npc.ai[1] as i32 % 180 == 0
                && let Some(t) = target
            {
                out.shots.push(Shot {
                    projectile: EMPRESS_DEATH_AURORA,
                    damage: EMPRESS_DEATH_AURORA_DAMAGE,
                    position: (t.center.0, t.center.1 - 100.0),
                    velocity: (0.0, 0.0),
                    time_left: 0,
                });
            }
            if npc.ai[1] >= 120.0 {
                back_to_idle(npc);
            }
        }
        attack::BOLTS | attack::CHASING_BOLTS => {
            let chasing = npc.ai[0] == attack::CHASING_BOLTS;
            station(
                npc,
                EMPRESS_STATION_HIGH,
                EMPRESS_FLY_SPEED,
                EMPRESS_FLY_ACCEL,
            );
            bolts(npc, target, damage(2), chasing, expert, &mut out);
            npc.ai[1] += 1.0;
            if npc.ai[1] >= 100.0 + (20.0 - quicker) {
                back_to_idle(npc);
            }
        }
        attack::RAINBOW => {
            station(
                npc,
                EMPRESS_STATION_HIGH,
                EMPRESS_FLY_SPEED,
                EMPRESS_FLY_ACCEL,
            );
            if npc.ai[1] == 0.0 {
                // Thirteen of them, evenly round, from a random starting angle.
                let from = rng.random::<f32>() * std::f32::consts::TAU;
                let (cx, cy) = npc.center();
                let hand = (cx + 55.0, cy - 30.0);
                for i in 0..EMPRESS_RAINBOW_COUNT {
                    let along = i as f32 / EMPRESS_RAINBOW_COUNT as f32;
                    let angle = std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * along + from;
                    let (sin, cos) = angle.sin_cos();
                    let out_of = (-sin, cos);
                    out.shots.push(Shot {
                        projectile: EMPRESS_RAINBOW,
                        damage: damage(1),
                        position: (hand.0 + out_of.1 * 30.0, hand.1 - out_of.0 * 30.0),
                        velocity: (
                            out_of.0 * EMPRESS_RAINBOW_SPEED,
                            out_of.1 * EMPRESS_RAINBOW_SPEED,
                        ),
                        time_left: 0,
                    });
                }
            }
            npc.ai[1] += 1.0;
            if npc.ai[1] >= 42.0 + (30.0 - quicker) {
                back_to_idle(npc);
            }
        }
        attack::LANCE_RING => {
            station(
                npc,
                EMPRESS_STATION_RING,
                EMPRESS_FLY_SPEED * 0.3,
                EMPRESS_FLY_ACCEL * 0.7,
            );
            if npc.ai[1] as i32 % 60 == 0 && npc.ai[1] < 180.0 {
                let round = (npc.ai[1] / 60.0) as i32;
                let side = target.is_some_and(|t| t.center.0 > npc.center().0) as i32 as f32;
                let count = if expert { 8.0 } else { 6.0 };
                let step = 1.0 / count;
                let (cx, cy) = npc.center();
                let mut along = 0.0;
                while along < 1.0 {
                    // Each round is offset half a step from the last, so the rings interleave.
                    let at = (along + step * 0.5 + round as f32 * step * 0.5) % 1.0;
                    let angle = std::f32::consts::TAU * (at + side);
                    out.shots.push(Shot {
                        projectile: EMPRESS_SUN_DANCE,
                        damage: damage(3),
                        position: (cx, cy - 100.0),
                        velocity: (angle.cos(), angle.sin()),
                        time_left: 0,
                    });
                    along += step;
                }
            }
            npc.ai[1] += 1.0;
            if npc.ai[1] >= 180.0 + (120.0 - quicker) {
                back_to_idle(npc);
            }
        }
        attack::LANCE_WALLS => {
            station(
                npc,
                EMPRESS_STATION_HIGH,
                EMPRESS_FLY_SPEED * 0.4,
                EMPRESS_FLY_ACCEL,
            );
            let every = if expert { 40.0 } else { 60.0 };
            let rounds = if expert { 6.0 } else { 4.0 };
            let total = every * rounds;
            if npc.ai[1] as i32 % every as i32 == 0 && npc.ai[1] < total {
                lance_wall(
                    npc,
                    target,
                    (npc.ai[1] / every) as i32,
                    damage(4),
                    expert,
                    &mut out,
                );
            }
            npc.ai[1] += 1.0;
            if npc.ai[1] >= total + (if expert { 40.0 } else { 20.0 } - quicker) {
                back_to_idle(npc);
            }
        }
        attack::DASH_LEFT | attack::DASH_RIGHT => {
            let from = if npc.ai[0] == attack::DASH_LEFT {
                -1.0
            } else {
                1.0
            };
            // She cannot be touched while she is winding up, only while she is coming through.
            npc.invulnerable = (6.0..=40.0).contains(&npc.ai[1]);
            if npc.ai[1] <= 40.0 {
                station(
                    npc,
                    (from * -EMPRESS_DASH_OUT, 0.0),
                    EMPRESS_FLY_SPEED,
                    EMPRESS_FLY_ACCEL * 2.0,
                );
                if npc.ai[1] == 40.0 {
                    npc.velocity = (npc.velocity.0 * 0.3, npc.velocity.1 * 0.3);
                }
            } else if npc.ai[1] <= 90.0 {
                let wanted = from * EMPRESS_DASH_SPEED;
                npc.velocity.0 += (wanted - npc.velocity.0) * 0.05;
                npc.velocity.1 += (0.0 - npc.velocity.1) * 0.05;
                if npc.ai[1] == 90.0 {
                    npc.velocity = (npc.velocity.0 * 0.7, npc.velocity.1 * 0.7);
                }
                npc.damage_bonus = 1.5;
            } else {
                npc.velocity.0 *= 0.92;
                npc.velocity.1 *= 0.92;
            }
            npc.ai[1] += 1.0;
            if npc.ai[1] >= 90.0 + (20.0 - quicker) {
                back_to_idle(npc);
            }
        }
        attack::TRANSITION => {
            // The turn: three seconds of nothing, a teleport at the halfway point, and she comes
            // back running the other script. Untouchable through the middle of it.
            npc.invulnerable = (30.0..=170.0).contains(&npc.ai[1]);
            npc.velocity.0 *= 0.95;
            npc.velocity.1 *= 0.95;
            if npc.ai[1] == 90.0 {
                npc.ai[3] = if npc.ai[3] == 2.0 { 3.0 } else { 1.0 };
                if let Some(t) = target {
                    npc.position = (
                        t.center.0 - npc.width() / 2.0,
                        t.center.1 - 250.0 - npc.height() / 2.0,
                    );
                }
            }
            npc.ai[1] += 1.0;
            if npc.ai[1] >= 180.0 + (20.0 - quicker) {
                back_to_idle(npc);
                npc.ai[2] = 0.0;
            }
        }
        _ => {
            // Leaving — unless whoever she came for turns back up, in which case she stays.
            if npc.ai[1] == 0.0 {
                npc.velocity = (0.0, -7.0);
            }
            npc.velocity.0 *= 0.95;
            npc.velocity.1 *= 0.95;
            npc.invulnerable = true;
            let going = leaving(npc, target, enraged, world);
            npc.alpha = (npc.alpha + if going { 5 } else { -5 }).clamp(0, 255);
            npc.ai[1] += 1.0;
            if npc.ai[1] >= 20.0 && (npc.alpha == 0 || npc.alpha == 255) {
                if npc.alpha == 255 {
                    out.spent = true;
                } else {
                    back_to_idle(npc);
                }
            }
        }
    }

    if !matches!(npc.ai[0], attack::ARRIVING | attack::LEAVING) {
        npc.alpha = (npc.alpha - 5).clamp(0, 255);
    }
    out
}

/// Whether this fight was begun in daylight, which she does not forget.
fn genuinely_enraged(npc: &Npc) -> bool {
    matches!(npc.ai[3] as i32, 2 | 3)
}

fn back_to_idle(npc: &mut Npc) {
    npc.ai[0] = attack::IDLE;
    npc.ai[1] = 0.0;
}

/// The idle: one dash toward you, a pause, and then the next thing off the script.
fn idle(
    npc: &mut Npc,
    target: Option<Target>,
    phase_2: bool,
    expert: bool,
    enraged: bool,
    slot: f32,
) {
    let wait = if phase_2 {
        EMPRESS_IDLE_PHASE_2
    } else {
        EMPRESS_IDLE
    };
    if npc.ai[1] <= 10.0 {
        let Some(t) = target else {
            // Nobody to dash at: she goes.
            npc.ai[0] = attack::LEAVING;
            npc.ai[1] = 0.0;
            npc.ai[2] += 1.0;
            npc.velocity = (npc.velocity.0 / 4.0, npc.velocity.1 / 4.0);
            return;
        };
        dash_to(npc, t.center);
    }
    if npc.velocity.0.hypot(npc.velocity.1) > 16.0 && npc.ai[1] > 10.0 {
        npc.velocity = (npc.velocity.0 / 2.0, npc.velocity.1 / 2.0);
    }
    npc.velocity.0 *= 0.92;
    npc.velocity.1 *= 0.92;
    npc.ai[1] += 1.0;
    if npc.ai[1] < wait {
        return;
    }

    let script: &[u8] = if !phase_2 {
        &EMPRESS_SCRIPT
    } else if expert {
        &EMPRESS_SCRIPT_PHASE_2_EXPERT
    } else {
        &EMPRESS_SCRIPT_PHASE_2
    };
    let mut next = script[(slot as usize) % script.len()] as f32;

    // Half health in the first phase overrides everything: it is time to turn.
    if !phase_2 && npc.life * 2 <= npc.life_max {
        next = attack::TRANSITION;
    }

    // Nobody within range, or a daytime fight that has run into the night: she leaves.
    let gone = match target {
        None => true,
        Some(t) => {
            let (cx, cy) = npc.center();
            (t.center.0 - cx).hypot(t.center.1 - cy) > EMPRESS_GIVE_UP
        }
    };
    if gone || (genuinely_enraged(npc) && !enraged) {
        next = attack::LEAVING;
    }

    // The dash comes from whichever side she is not on.
    if next == attack::DASH_LEFT && target.is_some_and(|t| t.center.0 > npc.center().0) {
        next = attack::DASH_RIGHT;
    }

    // In expert she does not merely change attack, she repositions between them: a sidestep at
    // right angles to you, which is what keeps her moving around the arena rather than above it.
    if expert
        && next != attack::RAINBOW
        && next != attack::CIRCLING_BLASTS
        && let Some(t) = target
    {
        let (cx, cy) = npc.center();
        let away = unit((cx - t.center.0, cy - t.center.1));
        let turn = if t.center.0 > cx { 1.0 } else { -1.0 };
        npc.velocity = (-away.1 * turn * 20.0, away.0 * turn * 20.0);
    }

    npc.ai[0] = next;
    npc.ai[1] = 0.0;
    npc.ai[2] += 1.0;
}

/// The idle's dash: she aims three hundred pixels above you and eases off as she arrives.
fn dash_to(npc: &mut Npc, player: (f32, f32)) {
    let (cx, cy) = npc.center();
    let mut spot = (player.0, player.1 - 300.0);
    if (spot.0 - cx).hypot(spot.1 - cy) > 200.0 {
        let to = unit((spot.0 - cx, spot.1 - cy));
        spot = (spot.0 - to.0 * 100.0, spot.1 - to.1 * 100.0);
    }
    let gap = (spot.0 - cx, spot.1 - cy);
    let far = gap.0.hypot(gap.1);
    let along = ((far - 100.0) / 500.0).clamp(0.0, 1.0);
    let speed = far.min(18.0);
    let to = unit(gap);
    npc.velocity = (
        to.0 * speed + (gap.0 / 6.0 - to.0 * speed) * along,
        to.1 * speed + (gap.1 / 6.0 - to.1 * speed) * along,
    );
}

/// The blast stream, from her left hand: a fan while she hovers, a full circle when she rises.
fn blasts(
    npc: &Npc,
    damage: i32,
    circling: bool,
    wide: bool,
    expert: bool,
    rng: &mut SmallRng,
    out: &mut EmpressOutcome,
) {
    let every = if circling {
        if expert { 4 } else { 6 }
    } else if expert {
        2
    } else {
        3
    };
    let within = if circling {
        (10.0..60.0).contains(&npc.ai[1])
    } else {
        npc.ai[1] < 60.0
    };
    if npc.ai[1] as i32 % every != 0 || !within {
        return;
    }
    let (cx, cy) = npc.center();
    let hand = (cx - 55.0, cy - 30.0);
    let velocity = if circling {
        // A full turn over the volley, so the stream comes out as a spiral.
        let angle = std::f32::consts::TAU * ((npc.ai[1] - 10.0) / 50.0);
        let (sin, cos) = angle.sin_cos();
        (20.0 * sin, -20.0 * cos)
    } else if wide {
        let angle = rng.random::<f32>() * std::f32::consts::TAU;
        let (sin, cos) = angle.sin_cos();
        (10.0 * sin, -10.0 * cos)
    } else {
        // A quarter-turn either way of straight up.
        let angle = std::f32::consts::FRAC_PI_2 * (rng.random::<f32>() * 2.0 - 1.0);
        let (sin, cos) = angle.sin_cos();
        (6.0 * sin, -6.0 * cos)
    };
    out.shots.push(Shot {
        projectile: EMPRESS_BLAST,
        damage,
        position: hand,
        velocity,
        time_left: 0,
    });
}

/// The prismatic bolts: lances laid down across where you are about to be, not where you are.
fn bolts(
    npc: &Npc,
    target: Option<Target>,
    damage: i32,
    chasing: bool,
    expert: bool,
    out: &mut EmpressOutcome,
) {
    let every = if chasing { 3 } else { 4 };
    if npc.ai[1] as i32 % every != 0 || npc.ai[1] >= 100.0 {
        return;
    }
    let Some(t) = target else {
        return;
    };
    let (cx, cy) = npc.center();
    if (t.center.0 - cx).hypot(t.center.1 - cy) > 2400.0 {
        return;
    }

    // Where it starts, and which way it points.
    let (from, reach) = if chasing {
        // Directly behind your own motion, a hundred pixels back.
        (unit((-t.velocity.0, -t.velocity.1)), 100.0)
    } else {
        // Spread round a fan, on whichever side you are not moving toward.
        let spokes = if expert { 5.0 } else { 4.0 };
        let step = (npc.ai[1] / 4.0) as i32 as f32;
        let angle = std::f32::consts::PI / (spokes * 2.0) + step * (std::f32::consts::PI / spokes);
        let (sin, cos) = angle.sin_cos();
        let mut spoke = (cos, sin);
        if !expert {
            spoke.0 += if spoke.0 > 0.0 { 0.5 } else { -0.5 };
        }
        let mut spoke = unit(spoke);
        // It never lays one where you are already heading — always behind or beside.
        let heading = unit((t.velocity.0, t.velocity.1));
        if spoke.0 * heading.0 + spoke.1 * heading.1 > 0.0 {
            spoke = (-spoke.0, -spoke.1);
        }
        (spoke, if expert { 450.0 } else { 300.0 })
    };

    // It leads you by a second and a half, which is what makes standing still fatal.
    let lead = (
        t.center.0 + t.velocity.0 * 90.0,
        t.center.1 + t.velocity.1 * 90.0,
    );
    let mut origin = (
        t.center.0 + from.0 * reach - t.velocity.0 * if chasing { 0.0 } else { 30.0 },
        t.center.1 + from.1 * reach - t.velocity.1 * if chasing { 0.0 } else { 30.0 },
    );
    // Never closer than its own reach: a bolt that started on top of you would be unavoidable.
    if (origin.0 - t.center.0).hypot(origin.1 - t.center.1) < reach {
        let back = {
            let raw = (t.center.0 - origin.0, t.center.1 - origin.1);
            if raw == (0.0, 0.0) { from } else { unit(raw) }
        };
        origin = (t.center.0 - back.0 * reach, t.center.1 - back.1 * reach);
    }
    let along = (lead.0 - origin.0, lead.1 - origin.1);
    out.shots.push(Shot {
        projectile: EMPRESS_LANCE,
        damage,
        position: origin,
        velocity: unit(along),
        time_left: 0,
    });
}

/// One wall of lances. Six arrangements, cycled, each covering the arena a different way.
fn lance_wall(
    npc: &Npc,
    target: Option<Target>,
    round: i32,
    damage: i32,
    expert: bool,
    out: &mut EmpressOutcome,
) {
    let Some(t) = target else {
        return;
    };
    let (cx, cy) = npc.center();
    if (t.center.0 - cx).hypot(t.center.1 - cy) > 3200.0 {
        return;
    }
    let lances = EMPRESS_WALL_LANCES + if expert { 5.0 } else { 0.0 };
    let spacing = EMPRESS_WALL_SPACING + if expert { 50.0 } else { 0.0 };
    let span = lances * spacing * if expert { 0.5 } else { 1.0 };

    // The middle of the wall, how it is laid out, and which way it faces.
    let (middle, across, facing) = match round {
        0 => (
            (t.center.0 - span / 2.0, t.center.1),
            (0.0, span),
            (1.0, 0.0),
        ),
        1 => (
            (t.center.0 + span / 2.0, t.center.1 + spacing / 2.0),
            (0.0, span),
            (-1.0, 0.0),
        ),
        2 => (
            (t.center.0 - span * 0.4, t.center.1 - span * 0.4),
            (span * 1.4, 0.0),
            unit((1.0, 1.0)),
        ),
        3 => (
            (
                t.center.0 + span * 0.4 + spacing / 2.0,
                t.center.1 - span * 0.4,
            ),
            (-span * 1.4, 0.0),
            unit((-1.0, 1.0)),
        ),
        4 => {
            let middle = (t.center.0 - span * 0.4, t.center.1 + span * 0.4);
            (
                middle,
                (span * 1.4, 0.0),
                unit((t.center.0 - middle.0, t.center.1 - middle.1)),
            )
        }
        _ => {
            let middle = (
                t.center.0 + span * 0.4 + spacing / 2.0,
                t.center.1 + span * 0.4,
            );
            (
                middle,
                (-span * 1.4, 0.0),
                unit((t.center.0 - middle.0, t.center.1 - middle.1)),
            )
        }
    };

    let step = 1.0 / lances;
    let mut along = 0.0;
    while along <= 1.0 {
        let origin = (
            middle.0 + across.0 * (along - 0.5),
            middle.1 + across.1 * (along - 0.5),
        );
        // In expert each lance is turned three quarters of the way toward where you will be, so a
        // wall converges on you rather than merely sweeping past.
        let aim = if expert {
            let lead = (
                t.center.0 + t.velocity.0 * 20.0 * along,
                t.center.1 + t.velocity.1 * 20.0 * along,
            );
            let toward = unit((lead.0 - origin.0, lead.1 - origin.1));
            unit((
                facing.0 + (toward.0 - facing.0) * 0.75,
                facing.1 + (toward.1 - facing.1) * 0.75,
            ))
        } else {
            facing
        };
        out.shots.push(Shot {
            projectile: EMPRESS_LANCE,
            damage,
            position: origin,
            velocity: aim,
            time_left: 0,
        });
        along += step;
    }
}

/// Whether she should keep going once she has started to leave.
fn leaving(
    npc: &Npc,
    target: Option<Target>,
    enraged: bool,
    _world: &World<'_, impl TileView>,
) -> bool {
    if genuinely_enraged(npc) && !enraged {
        return true;
    }
    match target {
        None => true,
        Some(t) => {
            let (cx, cy) = npc.center();
            (t.center.0 - cx).hypot(t.center.1 - cy) > EMPRESS_GIVE_UP
        }
    }
}

fn planted(npc: &Npc, projectile: u16, damage: i32, at: (f32, f32)) -> Shot {
    let (cx, cy) = npc.center();
    Shot {
        projectile,
        damage,
        position: (cx + at.0, cy + at.1),
        velocity: (0.0, 0.0),
        time_left: 0,
    }
}

fn unit(v: (f32, f32)) -> (f32, f32) {
    let length = v.0.hypot(v.1).max(f32::MIN_POSITIVE);
    (v.0 / length, v.1 / length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::HALLOW_BOSS;
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
                velocity: (2.0, 0.0),
                alive: true,
            }),
        )
    }

    fn her() -> Npc {
        let mut n = Npc::new(HALLOW_BOSS, (5000.0, 3000.0), 1).expect("the Empress");
        // Past her arrival, so the tests are about the fight.
        n.ai[0] = attack::IDLE;
        n
    }

    fn tick(
        npc: &mut Npc,
        w: &World<'_, Sky>,
        tiles: &Sky,
        enraged: bool,
        expert: bool,
        rng: &mut SmallRng,
    ) -> EmpressOutcome {
        let out = empress(npc, w, enraged, expert, rng);
        npc.no_gravity = true;
        npc.no_tile_collide = true;
        crate::game::npc::step_physics(npc, tiles);
        out
    }

    /// She works through the whole first script, in order, and never chains two attacks.
    #[test]
    fn the_first_script_runs_in_order() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((5000.0, 3400.0)));
        let mut rng = SmallRng::seed_from_u64(1);
        let mut n = her();
        let mut order = Vec::new();
        let mut last = n.ai[0];
        for _ in 0..20000 {
            tick(&mut n, &w, &tiles, false, false, &mut rng);
            if n.ai[0] != last {
                if n.ai[0] != attack::IDLE {
                    order.push(n.ai[0] as u8);
                }
                last = n.ai[0];
            }
            if order.len() >= EMPRESS_SCRIPT.len() {
                break;
            }
        }
        // The dash is mirrored to whichever side she is on, so 8 and 9 are the same slot.
        let flatten = |a: u8| if a == 9 { 8 } else { a };
        let expected: Vec<u8> = EMPRESS_SCRIPT.iter().map(|a| flatten(*a)).collect();
        let got: Vec<u8> = order.iter().map(|a| flatten(*a)).collect();
        assert_eq!(got, expected);
    }

    /// At half health she turns, and comes back on the other script.
    #[test]
    fn half_health_turns_the_fight() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((5000.0, 3400.0)));
        let mut rng = SmallRng::seed_from_u64(2);
        let mut n = her();
        n.life = n.life_max / 2;
        let mut turned = false;
        let mut after = Vec::new();
        for _ in 0..8000 {
            tick(&mut n, &w, &tiles, false, false, &mut rng);
            if n.ai[0] == attack::TRANSITION {
                turned = true;
            }
            if turned && n.ai[3] == 1.0 && n.ai[0] != attack::IDLE && n.ai[0] != attack::TRANSITION
            {
                after.push(n.ai[0] as u8);
            }
        }
        assert!(turned, "she should have turned");
        assert_eq!(n.ai[3], 1.0, "and be in her second phase");
        assert!(
            after.contains(&(attack::LANCE_WALLS as u8)),
            "which has attacks the first does not: {after:?}"
        );
    }

    /// She cannot be touched while she is turning, nor while she winds up a dash.
    #[test]
    fn she_is_untouchable_at_the_right_moments() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((5000.0, 3400.0)));
        let mut rng = SmallRng::seed_from_u64(3);

        let mut n = her();
        n.ai[0] = attack::TRANSITION;
        let mut safe = 0;
        for _ in 0..200 {
            tick(&mut n, &w, &tiles, false, false, &mut rng);
            safe += usize::from(n.invulnerable);
        }
        assert!(safe > 100, "the middle of the turn is hers: {safe}");

        let mut n = her();
        n.ai[0] = attack::DASH_LEFT;
        let mut hurtable = 0;
        for _ in 0..110 {
            tick(&mut n, &w, &tiles, false, false, &mut rng);
            hurtable += usize::from(!n.invulnerable);
        }
        assert!(hurtable > 40, "but the dash itself is yours: {hurtable}");
    }

    /// And when she is touchable, a hit has to actually land.
    ///
    /// The damage gate used to ask the type's `dont_take_damage` seed as well as the live flag, and
    /// npc 636 carries that seed (`npc_data.rs`, as vanilla's `SetDefaults` does), so `strike`
    /// refused every hit the routine had opened the window for. Seventy thousand health that no
    /// weapon could touch, in a fight that deals nine thousand a hit in daylight. This has to go
    /// through `strike`, because reading `invulnerable` alone is what let it hide.
    #[test]
    fn a_hit_lands_when_she_is_open() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((5000.0, 3400.0)));
        let mut rng = SmallRng::seed_from_u64(9);
        let mut n = her();
        assert!(
            n.stats.dont_take_damage,
            "the type's seed says untouchable, and that is only where it starts"
        );

        tick(&mut n, &w, &tiles, false, false, &mut rng);
        assert!(!n.invulnerable, "the idle is not a safe phase");
        n.strike(1000, 0.0, 1, false);
        assert!(n.life < n.life_max, "so the hit has to land");
    }

    /// A fight begun in daylight is marked, and the mark makes everything she does lethal.
    #[test]
    fn daylight_enrages_her_for_good() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((5000.0, 3400.0)));
        let mut rng = SmallRng::seed_from_u64(4);
        let mut n = her();
        tick(&mut n, &w, &tiles, true, false, &mut rng);
        assert!(genuinely_enraged(&n), "starting by day marks the run");

        let mut damage = Vec::new();
        for _ in 0..3000 {
            damage.extend(
                tick(&mut n, &w, &tiles, true, false, &mut rng)
                    .shots
                    .iter()
                    .map(|s| s.damage),
            );
        }
        assert!(!damage.is_empty(), "she should have attacked");
        // The sun dance is the one attack vanilla's own enrage override never touches
        // (`EMPRESS_DEATH_AURORA_DAMAGE`'s doc comment has the citations): it stays a flat forty in
        // the fight proper, and the one planted while she arrives is a separate, always-zero shot.
        assert!(
            damage
                .iter()
                .all(|d| *d == 9999 || *d == EMPRESS_DEATH_AURORA_DAMAGE || *d == 0),
            "every attack but the sun dance should kill outright: {damage:?}"
        );
        assert!(
            damage.contains(&9999),
            "something in 3000 ticks should have hit for the enraged amount"
        );
    }

    /// B13: the ring of sun dances and the wall of lances are different attacks with
    /// different damage in vanilla (`num10` for the ring, `num7` for the wall, both finalised at
    /// `NPC.cs:46495-46499` from the locals declared at `NPC.cs:46463-46467`), which a prior pass
    /// had collapsed into one shared slot.
    #[test]
    fn the_lance_ring_and_the_lance_wall_do_not_share_a_damage_slot() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((5000.0, 3400.0)));
        let mut rng = SmallRng::seed_from_u64(11);

        let mut ring = her();
        ring.ai[0] = attack::LANCE_RING;
        let ring_damage = tick(&mut ring, &w, &tiles, false, false, &mut rng)
            .shots
            .iter()
            .find(|s| s.projectile == EMPRESS_SUN_DANCE)
            .expect("the ring should have fired")
            .damage;

        let mut wall = her();
        wall.ai[0] = attack::LANCE_WALLS;
        let wall_damage = tick(&mut wall, &w, &tiles, false, false, &mut rng)
            .shots
            .iter()
            .find(|s| s.projectile == EMPRESS_LANCE)
            .expect("the wall should have fired")
            .damage;

        assert_eq!(ring_damage, 50, "the ring's own num10, classic phase 1");
        assert_eq!(wall_damage, 70, "the wall's own num7, classic phase 1");
        assert_ne!(
            ring_damage, wall_damage,
            "they are different attacks with different damage, not one shared slot"
        );
    }

    /// B13: the sun dance is never scaled by phase or difficulty. `num5` (`NPC.cs:46462`) never
    /// passes through `GetAttackDamage_ForProjectiles` and the phase-2 block that raises the
    /// other five locals (`NPC.cs:46482-46494`) never touches it either.
    #[test]
    fn the_sun_dance_stays_flat_across_phase_and_difficulty() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((5000.0, 3400.0)));
        let mut rng = SmallRng::seed_from_u64(12);

        for (phase_2, expert) in [(false, false), (false, true), (true, false), (true, true)] {
            let mut n = her();
            n.ai[3] = if phase_2 { 1.0 } else { 0.0 };
            n.ai[0] = attack::DEATH_AURORA;
            // The shot fires when `ai[1]` (incremented before the check) lands exactly on a
            // multiple of 180; starting one short of that puts it on this call.
            n.ai[1] = 179.0;
            let shot = tick(&mut n, &w, &tiles, false, expert, &mut rng)
                .shots
                .into_iter()
                .find(|s| s.projectile == EMPRESS_DEATH_AURORA)
                .expect("the sun dance should have fired this tick");
            assert_eq!(
                shot.damage, EMPRESS_DEATH_AURORA_DAMAGE,
                "phase_2={phase_2} expert={expert}, got {}",
                shot.damage
            );
        }
    }

    /// ...and once marked, night makes her leave rather than fight fair.
    #[test]
    fn a_marked_fight_ends_at_nightfall() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((5000.0, 3400.0)));
        let mut rng = SmallRng::seed_from_u64(5);
        let mut n = her();
        n.ai[3] = 2.0;
        let mut left = false;
        for _ in 0..2000 {
            if tick(&mut n, &w, &tiles, false, false, &mut rng).spent {
                left = true;
                break;
            }
        }
        assert!(left, "she should have gone");
    }

    /// With nobody left she leaves rather than hanging about.
    #[test]
    fn she_leaves_when_there_is_nobody_to_fight() {
        let tiles = Sky(HashMap::new());
        let empty = world(&tiles, None);
        let mut rng = SmallRng::seed_from_u64(6);
        let mut n = her();
        let mut left = false;
        for _ in 0..2000 {
            if tick(&mut n, &empty, &tiles, false, false, &mut rng).spent {
                left = true;
                break;
            }
        }
        assert!(left);
    }

    /// The rainbow is thirteen, evenly round, at one speed.
    #[test]
    fn the_rainbow_is_a_ring_of_thirteen() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((5000.0, 3400.0)));
        let mut rng = SmallRng::seed_from_u64(7);
        let mut n = her();
        n.ai[0] = attack::RAINBOW;
        let mut fired = Vec::new();
        for _ in 0..200 {
            fired.extend(tick(&mut n, &w, &tiles, false, false, &mut rng).shots);
            if n.ai[0] != attack::RAINBOW {
                break;
            }
        }
        assert_eq!(fired.len(), EMPRESS_RAINBOW_COUNT as usize);
        for shot in &fired {
            let speed = shot.velocity.0.hypot(shot.velocity.1);
            assert!((speed - EMPRESS_RAINBOW_SPEED).abs() < 0.01, "at {speed}");
        }
        // Evenly round: no two go the same way, and they cover the circle.
        let mut angles: Vec<f32> = fired
            .iter()
            .map(|s| s.velocity.1.atan2(s.velocity.0))
            .collect();
        angles.sort_by(f32::total_cmp);
        let step = std::f32::consts::TAU / EMPRESS_RAINBOW_COUNT as f32;
        for pair in angles.windows(2) {
            assert!(
                (pair[1] - pair[0] - step).abs() < 0.01,
                "evenly spaced: {angles:?}"
            );
        }
    }

    /// The bolts are laid where you are going, not where you are.
    #[test]
    fn the_bolts_lead_you() {
        let tiles = Sky(HashMap::new());
        // Moving right, hard.
        let mut w = world(&tiles, Some((5000.0, 3400.0)));
        w.target = w.target.map(|mut t| {
            t.velocity = (8.0, 0.0);
            t
        });
        let mut rng = SmallRng::seed_from_u64(8);
        let mut n = her();
        n.ai[0] = attack::BOLTS;
        let mut fired = Vec::new();
        for _ in 0..200 {
            fired.extend(tick(&mut n, &w, &tiles, false, false, &mut rng).shots);
            if n.ai[0] != attack::BOLTS {
                break;
            }
        }
        assert!(!fired.is_empty(), "it should have laid some");
        // Each is aimed at where the player will be, well to the right of where they are.
        for shot in &fired {
            assert!(shot.velocity.0.hypot(shot.velocity.1) > 0.99, "unit aim");
        }
        let ahead = fired
            .iter()
            .filter(|s| {
                let along = (5000.0 - s.position.0, 3400.0 - s.position.1);
                s.velocity.0 * along.0 + s.velocity.1 * along.1 > 0.0
            })
            .count();
        assert!(ahead > fired.len() / 2, "most should point back at you");
    }

    /// Expert is faster: the same attack takes less time.
    #[test]
    fn expert_tightens_the_rotation() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((5000.0, 3400.0)));
        let run = |expert: bool| {
            let mut rng = SmallRng::seed_from_u64(9);
            let mut n = her();
            n.ai[0] = attack::LANCE_RING;
            let mut ticks = 0;
            while n.ai[0] == attack::LANCE_RING && ticks < 1000 {
                tick(&mut n, &w, &tiles, false, expert, &mut rng);
                ticks += 1;
            }
            ticks
        };
        assert!(run(true) < run(false), "expert should be quicker");
    }
}
