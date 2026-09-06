//! Duke Fishron and its Sharkrons: styles 69 and 71.
//!
//! Fishron runs one skeleton three times over with different numbers, which is why the per-phase
//! figures are a table rather than a stack of branches: hold station three hundred pixels to one
//! side and two hundred above, wait, then attack. What changes between phases is how long the wait
//! is, how fast it moves, how hard it hits, and — the part that matters — how much armour it has
//! left. By the third phase it has none at all.
//!
//! The attacks come off a counter rather than a die roll, ten charges to one sharkron burst to one
//! spray of bubbles, so the pattern is learnable. The counter also decides the pace: the hover is
//! short before each charge and runs its full length only on the two that wind up to the burst and
//! the bubble, which is the fight's own telegraph. Crossing half health does not interrupt
//! whatever it is doing: vanilla only re-checks the threshold once it is back in its hover,
//! choosing its next attack, so a charge/burst/bubble stream in progress finishes on its own
//! terms. Once it does notice, it stops to change for three seconds, untouchable throughout, and
//! in expert crossing fifteen per cent does it again.
//!
//! It has to be fought over the ocean. Anywhere else (too high, below the surface, or simply
//! inland) it hits and soaks double, hovers a tenth as long, charges faster, and stops throwing
//! sharkrons in favour of bubbles it barely winds up for.
//!
//! A **Sharkron** (71) is not a chaser. It hangs in the air for a second and a half, aims once, and
//! commits — and dies on whatever it hits, terrain included.

use terrustia_proto::npc_params::{
    DETONATING_BUBBLE, FISHRON_ABOVE, FISHRON_ARRIVAL_FADE, FISHRON_ARRIVAL_RISE,
    FISHRON_ARRIVAL_RISE_AT, FISHRON_ARRIVAL_TICKS, FISHRON_BESIDE, FISHRON_BUBBLE_AT,
    FISHRON_BUBBLE_ENRAGED_AT, FISHRON_BUBBLE_SPEED, FISHRON_BUBBLE_TICKS, FISHRON_BURST_ACCEL,
    FISHRON_BURST_EVERY, FISHRON_BURST_LATER_CURVE, FISHRON_BURST_LATER_DASH_SPEED,
    FISHRON_BURST_LATER_SPRAY_EVERY, FISHRON_BURST_LATER_SPRAY_SPEED, FISHRON_BURST_LATER_TICKS,
    FISHRON_BURST_SPEED, FISHRON_BURST_TICKS, FISHRON_CYCLE_BUBBLES, FISHRON_CYCLE_BUBBLES_LATER,
    FISHRON_CYCLE_SHARKRONS, FISHRON_CYCLE_SHARKRONS_LATER, FISHRON_DAMAGE, FISHRON_DEFENSE,
    FISHRON_ENRAGE_ABOVE, FISHRON_ENRAGE_FROM_EDGE, FISHRON_ENRAGED_CHARGE_BONUS,
    FISHRON_ENRAGED_DAMAGE, FISHRON_ENRAGED_DEFENSE, FISHRON_ENRAGED_HOVER_TICKS,
    FISHRON_EXPERT_PACE, FISHRON_FIRST, FISHRON_FIRST_EXPERT, FISHRON_FIRST_HOVER_CHARGING,
    FISHRON_HALF_CYCLE, FISHRON_HALF_CYCLE_LATER, FISHRON_SECOND, FISHRON_SECOND_AT,
    FISHRON_SECOND_EXPERT, FISHRON_SHIFT_TICKS, FISHRON_THIRD, FISHRON_THIRD_AT, FishronPhase,
};
use terrustia_proto::projectile::ids::FISHRON_BUBBLE;

use crate::game::ai::{Shot, World, target_box};
use crate::game::npc::{Npc, TILE, TileView};
use crate::game::npc_ai::Spawn;

/// The states, as `ai[0]` numbers them. The second and third phases repeat the first four at an
/// offset of five and ten, which is exactly how the game numbers them.
pub mod state {
    /// Before the fight: it hangs invisible for seventy-five ticks, then drops into `HOVERING`.
    pub const ARRIVING: f32 = -1.0;
    pub const HOVERING: f32 = 0.0;
    pub const CHARGING: f32 = 1.0;
    pub const BURSTING: f32 = 2.0;
    pub const BUBBLING: f32 = 3.0;
    pub const CHANGING: f32 = 4.0;
    /// The offset between one phase's states and the next.
    pub const PHASE: f32 = 5.0;
}

/// What Fishron did this tick.
#[derive(Debug, Default)]
pub struct FishronOutcome {
    pub shots: Vec<Shot>,
    pub spawn: Vec<Spawn>,
}

/// Which phase a state belongs to, and the movement numbers for the half of the cycle it is in.
///
/// Vanilla picks these off three things at once, not one: the phase (`flag3`/`flag4`), whether the
/// attack cycle is still in its charging half (`flag5 = ai[3] < num2 * 2`, `NPC.cs:49303-49304`),
/// and expert. Reading only the phase, as this used to, gave the first phase half its real attack
/// rate and the second phase three times its real one, and made an expert Fishron move exactly
/// like a classic one (`NPC.cs:49320-49353`).
fn phase_of(state: f32, step: f32, expert: bool) -> (i32, FishronPhase) {
    if state > 9.0 {
        // The one phase with no expert row and no half-cycle split: it only exists in expert.
        return (2, FISHRON_THIRD);
    }
    let base = if expert {
        FISHRON_FIRST_EXPERT
    } else {
        FISHRON_FIRST
    };
    if state > 4.0 {
        let charging = (step as i32) < FISHRON_HALF_CYCLE_LATER;
        let p = match (charging, expert) {
            (true, true) => FISHRON_SECOND_EXPERT,
            (true, false) => FISHRON_SECOND,
            // Winding up to a burst or a bubble it drops back to the base row entirely.
            (false, _) => base,
        };
        return (1, p);
    }
    let mut p = base;
    if (step as i32) < FISHRON_HALF_CYCLE {
        p.hover_ticks = FISHRON_FIRST_HOVER_CHARGING;
    }
    (0, p)
}

/// Style 69.
pub fn fishron(npc: &mut Npc, world: &World<'_, impl TileView>) -> FishronOutcome {
    let mut out = FishronOutcome::default();
    npc.dirty = true;

    let expert = world.conditions.expert;
    let (phase, mut p) = phase_of(npc.ai[0], npc.ai[3], expert);
    let base = phase as f32 * state::PHASE;

    // It arrives out of nothing rather than simply appearing mid-fight (`NPC.cs:49399-49409`,
    // `:49517-49566`).
    if npc.local_ai[0] == 0.0 {
        npc.local_ai[0] = 1.0;
        npc.alpha = 255;
        npc.rotation = 0.0;
        npc.ai = [state::ARRIVING, 0.0, 0.0, npc.ai[3]];
    }

    // Vanilla applies the expert 1.2 only in the two later branches; the first phase is a flat
    // `damage = defDamage` with no multiplier at all (`NPC.cs:49307-49318`).
    let pace = if expert && phase > 0 {
        FISHRON_EXPERT_PACE
    } else {
        1.0
    };
    npc.damage_bonus = FISHRON_DAMAGE[phase as usize] * pace;
    npc.defense = (npc.stats.defense as f32 * FISHRON_DEFENSE[phase as usize]) as i32;
    // Only the arrival and the phase changes are windows in vanilla's sense: it holds still, and
    // `dontTakeDamage = !flag7` makes it untouchable for every tick of them (`NPC.cs:50278`).
    npc.invulnerable = npc.ai[0] == state::ARRIVING || npc.ai[0] - base == state::CHANGING;

    let Some(target) = world.target.filter(|t| t.alive) else {
        // With nobody left to fight it rises out of the world and falls back to the current
        // phase's hover (`NPC.cs:49376-49389`). The fall-back matters now that the arrival exists:
        // it is a state that cannot be hurt, and without this it would sit in it forever.
        npc.velocity.1 -= 0.4;
        npc.ai[0] = if npc.ai[0] > 4.0 {
            state::PHASE
        } else {
            state::HOVERING
        };
        npc.ai[2] = 0.0;
        npc.invulnerable = false;
        return out;
    };
    let (cx, cy) = npc.center();
    let health = npc.life as f32 / npc.life_max.max(1) as f32;

    // Fought anywhere but over the ocean it enrages (`flag6`, `NPC.cs:49390-49398`), which is
    // measured off the player's top-left corner rather than their centre, as vanilla measures it.
    let (px, py) = target_box(target);
    let inland = px > FISHRON_ENRAGE_FROM_EDGE
        && px < world.conditions.world_size.0 as f32 * TILE - FISHRON_ENRAGE_FROM_EDGE;
    let enraged = py < FISHRON_ENRAGE_ABOVE || py > world.conditions.surface_y || inland;
    if enraged {
        p.hover_ticks = FISHRON_ENRAGED_HOVER_TICKS;
        p.charge_speed += FISHRON_ENRAGED_CHARGE_BONUS;
        npc.damage_bonus = FISHRON_ENRAGED_DAMAGE;
        npc.defense = (npc.stats.defense as f32 * FISHRON_ENRAGED_DEFENSE) as i32;
    }

    match npc.ai[0] - base {
        s if s == state::ARRIVING => {
            // It bleeds off whatever speed it was given, then rises and fades in.
            npc.velocity.0 *= 0.98;
            npc.velocity.1 *= 0.98;
            if npc.ai[2] > FISHRON_ARRIVAL_RISE_AT {
                npc.velocity.1 = FISHRON_ARRIVAL_RISE;
                npc.alpha = (npc.alpha - FISHRON_ARRIVAL_FADE).max(0);
            }
            // It turns to face you, but its rotation stays pinned at zero for the whole arrival,
            // so this is the bare flip rather than the body-spinning `face` the fight uses.
            let side = (target.center.0 - cx).signum() as i8;
            if side != 0 {
                npc.direction = side;
                npc.sprite_direction = -side;
            }
            npc.ai[2] += 1.0;
            if npc.ai[2] >= FISHRON_ARRIVAL_TICKS {
                npc.ai = [state::HOVERING, 0.0, 0.0, npc.ai[3]];
            }
        }

        s if s == state::HOVERING => {
            // The side it takes station on is chosen once and kept for the whole hover.
            if npc.ai[1] == 0.0 {
                npc.ai[1] = FISHRON_BESIDE * (cx - target.center.0).signum();
            }
            let station = (
                target.center.0 + npc.ai[1] - cx,
                target.center.1 - FISHRON_ABOVE - cy,
            );
            ease_toward(npc, station, p.hover_speed, p.hover_accel);
            face(npc, target.center.0 - cx);

            npc.ai[2] += 1.0;
            if npc.ai[2] < p.hover_ticks {
                return out;
            }

            // Real vanilla nests both this check and the Expert-only second one strictly inside
            // the hover-timer-just-expired branch (`AI_069_DukeFishron`: `flag`/`flag2` are only
            // ever read where `num28`/`num33` — the *next attack* it is about to choose — get
            // computed, right here, and nowhere else). Crossing a threshold does not interrupt a
            // charge, a burst or a bubble stream already under way; it only ever gets checked at
            // this one decision point, the same one vanilla checks it at.
            let wants_phase = if expert && health <= FISHRON_THIRD_AT {
                2
            } else if health <= FISHRON_SECOND_AT {
                1
            } else {
                0
            };
            if wants_phase > phase {
                npc.ai = [base + state::CHANGING, 0.0, 0.0, npc.ai[3]];
                return out;
            }

            // The cycle: five charges, then a burst, then bubbles — three charges instead of
            // five once past the first phase (`NPC.cs:49624-49646` vs `49889-49907`).
            let (cycle_sharkrons, cycle_bubbles) = if phase >= 1 {
                (FISHRON_CYCLE_SHARKRONS_LATER, FISHRON_CYCLE_BUBBLES_LATER)
            } else {
                (FISHRON_CYCLE_SHARKRONS, FISHRON_CYCLE_BUBBLES)
            };
            let attack = npc.ai[3] as i32;
            let mut next = if attack == cycle_sharkrons {
                npc.ai[3] = 1.0;
                state::BURSTING
            } else if attack == cycle_bubbles {
                npc.ai[3] = 0.0;
                state::BUBBLING
            } else {
                state::CHARGING
            };
            // Enraged it never throws sharkrons: the burst becomes another bubble, and the cycle
            // step it just consumed is spent all the same (`NPC.cs:49647`, `:49897`).
            if enraged && next == state::BURSTING {
                next = state::BUBBLING;
            }
            npc.ai[0] = base + next;
            npc.ai[1] = 0.0;
            npc.ai[2] = 0.0;
            // ...and the first phase's bubble comes out almost immediately rather than after a
            // second and a half of wind-up (`NPC.cs:49684`).
            if enraged && next == state::BUBBLING && phase == 0 {
                npc.ai[2] = FISHRON_BUBBLE_TICKS - FISHRON_BUBBLE_ENRAGED_AT;
            }
            if next == state::CHARGING {
                // Aimed once, at speed, and never corrected.
                let aim = (target.center.0 - cx, target.center.1 - cy);
                let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
                npc.velocity = (
                    aim.0 / length * p.charge_speed,
                    aim.1 / length * p.charge_speed,
                );
                npc.rotation = npc.velocity.1.atan2(npc.velocity.0);
            }
        }

        s if s == state::CHARGING => {
            npc.ai[2] += 1.0;
            if npc.ai[2] >= p.charge_ticks {
                npc.ai[0] = base + state::HOVERING;
                npc.ai[1] = 0.0;
                npc.ai[2] = 0.0;
                // Two steps along the cycle per charge, which is what makes the burst come round
                // every fifth charge rather than every tenth.
                npc.ai[3] += 2.0;
            }
        }

        s if s == state::BURSTING && phase == 0 => {
            // The first phase's burst: it keeps station and throws a detonating bubble every four
            // ticks. Vanilla spawns type 371 here (`NPC.cs:49768`), which on 1.4.5.8 is the
            // bubble, not the sharkron the older numbering put at that id.
            if npc.ai[1] == 0.0 {
                npc.ai[1] = FISHRON_BESIDE * (cx - target.center.0).signum();
            }
            let station = (
                target.center.0 + npc.ai[1] - cx,
                target.center.1 - FISHRON_ABOVE - cy,
            );
            ease_toward(npc, station, FISHRON_BURST_SPEED, FISHRON_BURST_ACCEL);
            face(npc, target.center.0 - cx);

            if npc.ai[2] % FISHRON_BURST_EVERY == 0.0 {
                // Out of its mouth, toward the player.
                let aim = (target.center.0 - cx, target.center.1 - cy);
                let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
                let reach = (npc.width() + 20.0) / 2.0;
                out.spawn.push(Spawn {
                    handle: None,
                    npc_type: DETONATING_BUBBLE,
                    position: (
                        cx + aim.0 / length * reach,
                        cy + aim.1 / length * reach + 45.0,
                    ),
                    velocity: (0.0, 0.0),
                    parent: None,
                    ai: [None; 4],
                });
            }
            npc.ai[2] += 1.0;
            if npc.ai[2] >= FISHRON_BURST_TICKS {
                npc.ai[0] = base + state::HOVERING;
                npc.ai[1] = 0.0;
                npc.ai[2] = 0.0;
            }
        }

        s if s == state::BURSTING && phase >= 1 => {
            // The second and third phases' burst: a dash that curves through the air for its
            // whole duration, spraying a detonating bubble out perpendicular to its own heading
            // every four ticks instead of holding station (`NPC.cs:49916-50015`, spawning 371 at
            // `:49999`).
            if npc.ai[2] == 0.0 {
                let aim = (target.center.0 - cx, target.center.1 - cy);
                let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
                npc.velocity = (
                    aim.0 / length * FISHRON_BURST_LATER_DASH_SPEED,
                    aim.1 / length * FISHRON_BURST_LATER_DASH_SPEED,
                );
                face(npc, target.center.0 - cx);
                npc.rotation = npc.velocity.1.atan2(npc.velocity.0);
            }

            if npc.ai[2] % FISHRON_BURST_LATER_SPRAY_EVERY == 0.0 {
                let vlen = npc.velocity.0.hypot(npc.velocity.1).max(f32::MIN_POSITIVE);
                let heading = (npc.velocity.0 / vlen, npc.velocity.1 / vlen);
                let side = f32::from(npc.direction);
                // Perpendicular to its own heading, not aimed at the player.
                let perp = (-heading.1 * side, heading.0 * side);
                let reach = (npc.width() + 20.0) / 2.0;
                out.spawn.push(Spawn {
                    handle: None,
                    npc_type: DETONATING_BUBBLE,
                    position: (cx + heading.0 * reach, cy + heading.1 * reach + 45.0),
                    velocity: (
                        perp.0 * FISHRON_BURST_LATER_SPRAY_SPEED,
                        perp.1 * FISHRON_BURST_LATER_SPRAY_SPEED,
                    ),
                    parent: None,
                    ai: [None; 4],
                });
            }

            // It curves through the air the whole time, rather than flying straight.
            let angle = -FISHRON_BURST_LATER_CURVE * f32::from(npc.direction);
            let (sin, cos) = angle.sin_cos();
            npc.velocity = (
                npc.velocity.0 * cos - npc.velocity.1 * sin,
                npc.velocity.0 * sin + npc.velocity.1 * cos,
            );
            npc.rotation -= FISHRON_BURST_LATER_CURVE * f32::from(npc.direction);

            npc.ai[2] += 1.0;
            if npc.ai[2] >= FISHRON_BURST_LATER_TICKS {
                npc.ai[0] = base + state::HOVERING;
                npc.ai[1] = 0.0;
                npc.ai[2] = 0.0;
            }
        }

        s if s == state::BUBBLING => {
            // It hangs almost still and spits. Which spit depends on the phase: the first throws
            // two bubbles out of its mouth that drift apart (`NPC.cs:49801`), the second one
            // single bubble from its own centre with no velocity at all, which then seeks
            // (`NPC.cs:50027`). Both carry `damage: 0`, as vanilla passes.
            npc.velocity.0 *= 0.98;
            npc.velocity.1 += (0.0 - npc.velocity.1) * 0.02;
            if npc.ai[2] == FISHRON_BUBBLE_TICKS - FISHRON_BUBBLE_AT {
                if phase == 0 {
                    let from = (
                        cx + f32::from(npc.direction) * (npc.width() + 20.0) / 2.0,
                        cy,
                    );
                    for side in [1.0, -1.0] {
                        out.shots.push(Shot {
                            projectile: FISHRON_BUBBLE,
                            damage: 0,
                            position: from,
                            velocity: (
                                side * f32::from(npc.direction) * FISHRON_BUBBLE_SPEED.0,
                                FISHRON_BUBBLE_SPEED.1,
                            ),
                            time_left: 900,
                            ai: [0.0; 3],
                        });
                    }
                } else {
                    // `NewProjectile(..., 1f, target + 1, flag6 ? 1 : 0)` (`NPC.cs:50027`), and
                    // those three numbers *are* the attack: `aiStyle 65` reads a non-zero `ai[1]`
                    // as "seek the player one below me", and `ai[2]` as the enrage that makes it
                    // sixteen pixels a tick rather than four. `flag6` is the same
                    // out-of-the-ocean test this routine already runs above, so it is that flag
                    // rather than a second reading of it.
                    //
                    // This site used to carry a standing narrowing - "`Shot` carries no `ai` ...
                    // so the bubble is launched without them and hangs where it was made" - and it
                    // is the site [`crate::game::ai::Shot::ai`] was added for.
                    out.shots.push(Shot {
                        projectile: FISHRON_BUBBLE,
                        damage: 0,
                        position: (cx, cy),
                        velocity: (0.0, 0.0),
                        time_left: 900,
                        ai: [
                            1.0,
                            f32::from(target.slot) + 1.0,
                            if enraged { 1.0 } else { 0.0 },
                        ],
                    });
                }
            }
            npc.ai[2] += 1.0;
            if npc.ai[2] >= FISHRON_BUBBLE_TICKS {
                npc.ai[0] = base + state::HOVERING;
                npc.ai[1] = 0.0;
                npc.ai[2] = 0.0;
            }
        }

        _ => {
            // Changing. It does nothing at all for three seconds and cannot be hurt for any of
            // them, so this is its window rather than yours.
            npc.velocity.0 *= 0.98;
            npc.velocity.1 += (0.0 - npc.velocity.1) * 0.02;
            npc.ai[2] += 1.0;
            if npc.ai[2] >= FISHRON_SHIFT_TICKS {
                // Into the next phase's hover.
                npc.ai = [base + state::PHASE, 0.0, 0.0, 0.0];
            }
        }
    }
    out
}

/// Ease toward an offset at a given speed, doubling the push while still going the wrong way.
fn ease_toward(npc: &mut Npc, offset: (f32, f32), speed: f32, accel: f32) {
    // The game subtracts the current velocity before normalising, which is what stops it
    // overshooting its own station at speed.
    let aim = (offset.0 - npc.velocity.0, offset.1 - npc.velocity.1);
    let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
    let wanted = (aim.0 / length * speed, aim.1 / length * speed);
    for (v, w) in [
        (&mut npc.velocity.0, wanted.0),
        (&mut npc.velocity.1, wanted.1),
    ] {
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

/// Turn to face the player, flipping the sprite without spinning the body.
fn face(npc: &mut Npc, across: f32) {
    let side = across.signum() as i8;
    if side == 0 {
        return;
    }
    npc.direction = side;
    if npc.sprite_direction != -side {
        npc.rotation += std::f32::consts::PI;
    }
    npc.sprite_direction = -side;
}

/// Style 71: a Sharkron.
///
/// It hangs where it was thrown, turning to face you, and after ninety ticks commits to a single
/// line at sixteen pixels a tick. It does not steer afterwards and it dies on whatever it meets,
/// so a Sharkron is dodged rather than outrun.
pub fn sharkron(npc: &mut Npc, world: &World<'_, impl TileView>) -> bool {
    npc.dirty = true;
    npc.no_gravity = true;
    npc.invulnerable = npc.ai[0] == 0.0 && npc.ai[1] < 60.0;

    let Some(target) = world.target.filter(|t| t.alive) else {
        return false;
    };
    let (cx, cy) = npc.center();

    if npc.ai[0] == 0.0 {
        // Winding up. It fades in, holds still, and turns to face you.
        npc.ai[1] += 1.0;
        npc.alpha = (npc.alpha - 6).max(0);
        let aim = (target.center.0 - cx, target.center.1 - cy);
        npc.rotation = aim.1.atan2(aim.0) + std::f32::consts::FRAC_PI_2;
        npc.velocity.1 = npc.ai[3];

        if npc.ai[1] >= 90.0 {
            npc.ai[0] = 1.0;
            npc.ai[1] = 0.0;
            let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
            npc.velocity = (aim.0 / length * 16.0, aim.1 / length * 16.0);
            npc.rotation = npc.velocity.1.atan2(npc.velocity.0);
            npc.direction = npc.velocity.0.signum() as i8;
        }
        return false;
    }

    // Committed. It travels its line, and hitting anything is the end of it.
    npc.no_tile_collide = false;
    npc.ai[1] += 1.0;
    if npc.ai[1] >= 60.0 {
        // Past a second it becomes solid again and falls out of the sky.
        npc.no_gravity = false;
    }
    npc.rotation = npc.velocity.1.atan2(npc.velocity.0);
    npc.collide_x || npc.collide_y
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::{FISHRON, SHARKRON};
    use terrustia_proto::tile::Tile;

    struct Sky(HashMap<(i32, i32), Tile>);

    impl TileView for Sky {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    /// Over the ocean, which is the only place Fishron is not enraged: the player has to be below
    /// the sky line, above the surface line, and within 6400 pixels of a world edge
    /// (`NPC.cs:49388`). Everything below is measured against an unenraged fight unless it says
    /// otherwise.
    fn world<'a>(tiles: &'a Sky, target: Option<(f32, f32)>) -> World<'a, Sky> {
        let mut w = crate::game::ai::calm(
            tiles,
            target.map(|center| Target {
                slot: 0,
                center,
                velocity: (0.0, 0.0),
                alive: true,
            }),
        );
        w.conditions.surface_y = 20_000.0;
        w
    }

    /// The player, out over the ocean: past the 800-pixel sky line and inside the shore.
    const AT_SEA: (f32, f32) = (600.0, 2000.0);

    /// One that has already arrived, which is where every test but the arrival's own starts.
    fn duke(x: f32, y: f32) -> Npc {
        let mut d = Npc::new(FISHRON, (x, y), 1).expect("duke fishron");
        d.local_ai[0] = 1.0;
        d
    }

    /// The cycle is a counter, not a die roll: ten charges, a sharkron burst, then bubbles.
    #[test]
    fn its_attacks_come_round_in_order() {
        let tiles = Sky(HashMap::new());
        let mut d = duke(0.0, 0.0);
        let w = world(&tiles, Some(AT_SEA));

        let mut order = Vec::new();
        let mut was = d.ai[0];
        for _ in 0..6000 {
            fishron(&mut d, &w);
            d.position.0 += d.velocity.0;
            d.position.1 += d.velocity.1;
            if d.ai[0] != was {
                if d.ai[0] != state::HOVERING {
                    order.push(d.ai[0]);
                }
                was = d.ai[0];
            }
        }
        assert!(order.contains(&state::CHARGING), "it charges: {order:?}");
        assert!(order.contains(&state::BURSTING), "it bursts: {order:?}");
        assert!(order.contains(&state::BUBBLING), "it bubbles: {order:?}");
    }

    /// This used to be `half_health_starts_the_second_phase`, and its own assertion — that
    /// starting Fishron mid-charge and crossing 50% health interrupts the charge on the very next
    /// tick — was itself the bug: real vanilla (`AI_069_DukeFishron`) only ever reads the
    /// threshold flags (`flag`/`flag2`) inside the `ai[0]==0f`/`ai[0]==5f` hover branches, at the
    /// exact point the hover timer has just expired and it is choosing its next attack. There is
    /// no code path in vanilla that checks health mid-charge, mid-burst, or mid-bubble-stream, so
    /// asserting an immediate interrupt there was asserting a behaviour vanilla doesn't have. This
    /// test now asserts the corrected shape: the current attack always finishes, and the phase
    /// only changes once it is back in the hover and its own timer has run out.
    #[test]
    fn half_health_finishes_the_current_attack_before_the_second_phase_starts() {
        let tiles = Sky(HashMap::new());
        let mut d = duke(0.0, 0.0);
        let w = world(&tiles, Some(AT_SEA));

        // Start it mid-charge, already below the 50% threshold.
        d.ai[0] = state::CHARGING;
        d.ai[2] = 0.0;
        d.life = d.life_max / 3;
        fishron(&mut d, &w);
        assert_eq!(
            d.ai[0],
            state::CHARGING,
            "a charge already in progress should not be interrupted by crossing a threshold"
        );

        // Let the charge run out on its own terms.
        for _ in 0..(FISHRON_FIRST.charge_ticks as i32 + 2) {
            if d.ai[0] != state::CHARGING {
                break;
            }
            fishron(&mut d, &w);
        }
        assert_eq!(
            d.ai[0],
            state::HOVERING,
            "the charge finishes and returns to hovering, not a phase change mid-attack"
        );

        // Only once it is back in the hover, past its own hover timer, does it notice.
        for _ in 0..(FISHRON_FIRST.hover_ticks as i32 + 2) {
            if d.ai[0] == state::CHANGING {
                break;
            }
            fishron(&mut d, &w);
        }
        assert_eq!(
            d.ai[0],
            state::CHANGING,
            "the hover's own decision point is where it finally notices"
        );

        for _ in 0..(FISHRON_SHIFT_TICKS as i32 + 2) {
            fishron(&mut d, &w);
        }
        assert_eq!(d.ai[0], state::PHASE, "and comes out in the second phase");
        assert!(
            d.defense < d.stats.defense,
            "with less armour: {} was {}",
            d.defense,
            d.stats.defense
        );
    }

    /// In expert it sheds its armour entirely for the last stretch.
    #[test]
    fn the_third_phase_has_no_armour_at_all() {
        let tiles = Sky(HashMap::new());
        let mut w = world(&tiles, Some(AT_SEA));
        w.conditions.expert = true;
        let mut d = duke(0.0, 0.0);
        d.ai[0] = state::PHASE + state::HOVERING;
        d.life = d.life_max / 20;

        // Same fix as `half_health_finishes_the_current_attack_before_the_second_phase_starts`:
        // even starting fresh in the hover, real vanilla only notices the threshold once that
        // hover's own timer has actually run out — not on the very first tick of it.
        for _ in 0..(FISHRON_SECOND_EXPERT.hover_ticks as i32 + 2) {
            if d.ai[0] == state::PHASE + state::CHANGING {
                break;
            }
            fishron(&mut d, &w);
        }
        assert_eq!(d.ai[0], state::PHASE + state::CHANGING);
        for _ in 0..(FISHRON_SHIFT_TICKS as i32 + 2) {
            fishron(&mut d, &w);
        }
        assert_eq!(d.ai[0], state::PHASE * 2.0, "into the third phase");
        fishron(&mut d, &w);
        assert_eq!(d.defense, 0, "and no armour left");
    }

    /// The burst throws sharkrons rather than projectiles.
    #[test]
    fn the_burst_throws_sharkrons() {
        let tiles = Sky(HashMap::new());
        let mut d = duke(0.0, 0.0);
        d.ai[0] = state::BURSTING;
        let w = world(&tiles, Some(AT_SEA));

        let mut thrown = Vec::new();
        for _ in 0..(FISHRON_BURST_TICKS as i32 + 2) {
            thrown.extend(fishron(&mut d, &w).spawn);
        }
        assert!(!thrown.is_empty(), "it should have thrown some");
        assert!(thrown.iter().all(|s| s.npc_type == DETONATING_BUBBLE));
    }

    /// B11: the first phase's burst throws roughly twenty bubbles over its eighty ticks: one
    /// every four — not the roughly six the old 120-tick/20-tick numbers gave.
    #[test]
    fn the_first_phase_burst_throws_about_twenty_sharkrons() {
        let tiles = Sky(HashMap::new());
        let mut d = duke(0.0, 0.0);
        d.ai[0] = state::BURSTING;
        let w = world(&tiles, Some(AT_SEA));

        let mut thrown = 0;
        for _ in 0..(FISHRON_BURST_TICKS as i32 + 2) {
            thrown += fishron(&mut d, &w).spawn.len();
        }
        assert_eq!(
            thrown, 20,
            "eighty ticks at one every four should be twenty"
        );
    }

    /// B11: past the first phase it bursts after three charges, not five.
    #[test]
    fn later_phases_burst_after_three_charges_not_five() {
        let tiles = Sky(HashMap::new());
        let mut d = duke(0.0, 0.0);
        d.ai[0] = state::PHASE + state::HOVERING;
        d.life = d.life_max / 3; // safely inside the second phase throughout
        let w = world(&tiles, Some(AT_SEA));

        let mut seen = Vec::new();
        let mut was = d.ai[0];
        for _ in 0..8000 {
            fishron(&mut d, &w);
            d.position.0 += d.velocity.0;
            d.position.1 += d.velocity.1;
            if d.ai[0] != was {
                if d.ai[0] != state::PHASE + state::HOVERING {
                    seen.push(d.ai[0]);
                }
                was = d.ai[0];
            }
            if d.ai[0] == state::PHASE + state::BURSTING {
                break;
            }
        }
        assert_eq!(
            d.ai[0],
            state::PHASE + state::BURSTING,
            "it should have burst: {seen:?}"
        );
        let charges = seen
            .iter()
            .filter(|&&s| s == state::PHASE + state::CHARGING)
            .count();
        assert_eq!(
            charges, 3,
            "three charges before the burst, not five: {seen:?}"
        );
    }

    /// B11: past the first phase the burst launches sharkrons already moving, perpendicular to
    /// its own heading — not the stationary, player-aimed drop the first phase uses.
    #[test]
    fn later_phase_burst_launches_sharkrons_moving_not_stationary() {
        let tiles = Sky(HashMap::new());
        let mut d = duke(0.0, 0.0);
        d.ai[0] = state::PHASE + state::BURSTING;
        d.direction = 1;
        let w = world(&tiles, Some(AT_SEA));

        let mut thrown = Vec::new();
        for _ in 0..(FISHRON_BURST_LATER_TICKS as i32 + 2) {
            thrown.extend(fishron(&mut d, &w).spawn);
        }
        assert!(!thrown.is_empty(), "it should have thrown some");
        assert!(thrown.iter().all(|s| s.npc_type == DETONATING_BUBBLE));
        for s in &thrown {
            let speed = s.velocity.0.hypot(s.velocity.1);
            assert!(
                (speed - FISHRON_BURST_LATER_SPRAY_SPEED).abs() < 0.01,
                "should launch at the spray speed, not stationary: {:?}",
                s.velocity
            );
        }
    }

    /// The bubbles come in pairs that drift apart.
    #[test]
    fn the_bubbles_come_in_pairs() {
        let tiles = Sky(HashMap::new());
        let mut d = duke(0.0, 0.0);
        d.ai[0] = state::BUBBLING;
        d.direction = 1;
        let w = world(&tiles, Some(AT_SEA));

        let mut bubbles = Vec::new();
        for _ in 0..(FISHRON_BUBBLE_TICKS as i32 + 2) {
            bubbles.extend(fishron(&mut d, &w).shots);
        }
        assert_eq!(bubbles.len(), 2, "two bubbles");
        assert!(
            bubbles[0].velocity.0 * bubbles[1].velocity.0 < 0.0,
            "and they should go opposite ways: {:?}",
            bubbles.iter().map(|b| b.velocity).collect::<Vec<_>>()
        );
    }

    /// F1: fought anywhere but over the ocean it enrages: twice the damage, twice the armour, a
    /// tenth of the hover, six pixels a tick more charge speed (`NPC.cs:49390-49397`), the
    /// sharkron burst swapped for a bubble (`:49647`) and that bubble pre-wound so it fires almost
    /// at once (`:49684`). None of it existed, so an inland Fishron fought exactly like an ocean
    /// one, which is the cheapest way there is to trivialise the fight.
    #[test]
    fn fought_inland_it_enrages() {
        let tiles = Sky(HashMap::new());
        // The same sky line, but far enough from either edge of the world to be inland.
        let inland = world(&tiles, Some((20_000.0, AT_SEA.1)));
        let ocean = world(&tiles, Some(AT_SEA));

        let mut d = duke(0.0, 0.0);
        fishron(&mut d, &inland);
        assert_eq!(d.damage_bonus, FISHRON_ENRAGED_DAMAGE, "twice the damage");
        assert_eq!(d.defense, d.stats.defense * 2, "twice the armour");

        // Ten ticks of hover, not the thirty a first-phase charge normally waits.
        let mut angry = duke(0.0, 0.0);
        let mut calm = duke(0.0, 0.0);
        for _ in 0..FISHRON_ENRAGED_HOVER_TICKS as i32 {
            fishron(&mut angry, &inland);
            fishron(&mut calm, &ocean);
        }
        assert_ne!(angry.ai[0], state::HOVERING, "it has already chosen");
        assert_eq!(
            calm.ai[0],
            state::HOVERING,
            "where an ocean one is still waiting"
        );
        assert!(
            angry.velocity.0.hypot(angry.velocity.1)
                > FISHRON_FIRST.charge_speed + FISHRON_ENRAGED_CHARGE_BONUS - 0.01,
            "and charges six pixels a tick faster: {:?}",
            angry.velocity
        );

        // The burst it would have thrown is a bubble instead, and it is already wound up.
        let mut d = duke(0.0, 0.0);
        d.ai[3] = FISHRON_CYCLE_SHARKRONS as f32;
        for _ in 0..FISHRON_ENRAGED_HOVER_TICKS as i32 {
            fishron(&mut d, &inland);
        }
        assert_eq!(d.ai[0], state::BUBBLING, "no sharkrons out of the ocean");
        assert_eq!(d.ai[3], 1.0, "but the cycle step is spent all the same");
        assert_eq!(
            d.ai[2],
            FISHRON_BUBBLE_TICKS - FISHRON_BUBBLE_ENRAGED_AT,
            "and the bubble is all but out already"
        );
    }

    /// F2: expert Fishron moved exactly like a classic one. Every movement number has an expert
    /// variant (`NPC.cs:49320-49353`); `FISHRON_EXPERT_PACE` was reaching `damage_bonus` alone,
    /// and vanilla does not even apply it there in the first phase, which is a flat
    /// `damage = defDamage` (`NPC.cs:49315-49318`).
    #[test]
    fn expert_moves_on_its_own_numbers() {
        let tiles = Sky(HashMap::new());
        let charge_speed = |expert: bool| {
            let mut w = world(&tiles, Some(AT_SEA));
            w.conditions.expert = expert;
            let mut d = duke(0.0, 0.0);
            for _ in 0..200 {
                fishron(&mut d, &w);
                if d.ai[0] == state::CHARGING {
                    break;
                }
            }
            assert_eq!(d.ai[0], state::CHARGING, "it should have charged");
            (d.velocity.0.hypot(d.velocity.1), d.damage_bonus)
        };
        let (classic, _) = charge_speed(false);
        let (expert, damage) = charge_speed(true);
        assert!(
            (classic - FISHRON_FIRST.charge_speed).abs() < 0.01,
            "classic charges at sixteen: {classic}"
        );
        assert!(
            (expert - FISHRON_FIRST_EXPERT.charge_speed).abs() < 0.01,
            "expert at seventeen: {expert}"
        );
        assert_eq!(
            damage, 1.0,
            "and the first phase takes no expert multiplier"
        );
    }

    /// F3: the hover is short before a charge and long before a burst or a bubble
    /// (`flag5 = ai[3] < num2 * 2`, `NPC.cs:49304`, spent at `:49335-49338`). One flat value per
    /// phase ran the first phase's charges at half their real rate.
    #[test]
    fn the_hover_is_short_before_a_charge_and_long_before_a_burst() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some(AT_SEA));
        let hover_for = |step: f32| {
            let mut d = duke(0.0, 0.0);
            d.ai[3] = step;
            let mut ticks = 0;
            while d.ai[0] == state::HOVERING && ticks < 500 {
                fishron(&mut d, &w);
                ticks += 1;
            }
            ticks
        };
        assert_eq!(
            hover_for(0.0),
            FISHRON_FIRST_HOVER_CHARGING as i32,
            "short before a charge"
        );
        assert_eq!(
            hover_for(FISHRON_CYCLE_SHARKRONS as f32),
            FISHRON_FIRST.hover_ticks as i32,
            "and the full wind-up before the burst"
        );
    }

    /// F4: past the first phase the bubble is one bubble from its own centre with no velocity at
    /// all, the one that seeks (`NPC.cs:50027`). It was reusing the first phase's two-bubble
    /// spread (`NPC.cs:49801`) in every phase.
    #[test]
    fn the_later_bubble_is_a_single_one_from_its_own_centre() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some(AT_SEA));
        let mut d = duke(0.0, 0.0);
        d.ai[0] = state::PHASE + state::BUBBLING;
        d.life = d.life_max / 3;
        d.direction = 1;

        let mut bubbles = Vec::new();
        for _ in 0..(FISHRON_BUBBLE_TICKS as i32 + 2) {
            bubbles.extend(fishron(&mut d, &w).shots);
        }
        assert_eq!(bubbles.len(), 1, "one bubble, not two");
        assert_eq!(bubbles[0].velocity, (0.0, 0.0), "and it does not drift");
        assert_eq!(bubbles[0].position, d.center(), "out of its own centre");
        assert_eq!(bubbles[0].damage, 0, "damage 0, as vanilla passes");
        // `NewProjectile(..., 1f, target + 1, flag6 ? 1 : 0)` (`NPC.cs:50027`). These three are
        // the attack: without them the bubble hangs where it was made for nine hundred ticks,
        // which is what this site's own comment described for as long as `Shot` had no `ai`.
        assert_eq!(bubbles[0].ai[0], 1.0, "the flag that turns seeking on");
        assert_eq!(
            bubbles[0].ai[1], 1.0,
            "target 0, plus one so zero can mean nobody"
        );
        assert_eq!(bubbles[0].ai[2], 0.0, "not enraged, out over the ocean");
    }

    /// Out of the ocean the same bubble carries the enrage, which is what doubles its speed twice
    /// over: `flag6` is `NPC.cs:49390`, and `aiStyle 65` reads it as `+12` on a base of four.
    #[test]
    fn an_enraged_dukes_bubble_is_told_so() {
        let tiles = Sky(HashMap::new());
        // High above the sky line is one of the three ways to be out of the ocean.
        let w = world(&tiles, Some((600.0, 100.0)));
        let mut d = duke(0.0, 0.0);
        d.ai[0] = state::PHASE + state::BUBBLING;
        d.life = d.life_max / 3;
        d.direction = 1;

        let mut bubbles = Vec::new();
        for _ in 0..(FISHRON_BUBBLE_TICKS as i32 + 2) {
            bubbles.extend(fishron(&mut d, &w).shots);
        }
        assert_eq!(bubbles.len(), 1);
        assert_eq!(bubbles[0].ai[2], 1.0);
    }

    /// It arrives out of nothing rather than simply appearing (`ai[0] = -1`,
    /// `NPC.cs:49399-49409`), and that stretch and the phase change are the two vanilla makes it
    /// untouchable for (`dontTakeDamage = !flag7`, `NPC.cs:50278`).
    #[test]
    fn it_arrives_faded_out_and_cannot_be_hurt_doing_it() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some(AT_SEA));
        // Not `duke`, which starts one that has already arrived.
        let mut d = Npc::new(FISHRON, (0.0, 0.0), 1).expect("duke fishron");

        fishron(&mut d, &w);
        assert_eq!(d.ai[0], state::ARRIVING);
        assert_eq!(d.alpha, 255, "invisible to start with");
        assert!(d.invulnerable, "and untouchable while it does it");

        for _ in 0..FISHRON_ARRIVAL_TICKS as i32 {
            fishron(&mut d, &w);
        }
        assert_eq!(d.ai[0], state::HOVERING, "then the fight starts");
        assert_eq!(d.alpha, 0, "solid");
        assert!(!d.invulnerable, "and hittable");

        d.ai = [state::CHANGING, 0.0, 0.0, 0.0];
        fishron(&mut d, &w);
        assert!(
            d.invulnerable,
            "the phase change is the other window, and it is its own"
        );
    }

    /// A Sharkron aims once and commits, and cannot be hurt while it winds up.
    #[test]
    fn a_sharkron_aims_once_and_commits() {
        let tiles = Sky(HashMap::new());
        // Type 372, the sharkron itself: the only type that really runs style 71. This test used
        // to drive the routine on 371, which is the detonating bubble and runs style 70.
        let mut s = Npc::new(SHARKRON, (0.0, 0.0), 1).expect("sharkron");
        let w = world(&tiles, Some((600.0, 0.0)));

        s.alpha = 255;
        sharkron(&mut s, &w);
        assert!(s.invulnerable, "it cannot be hit while it fades in");

        for _ in 0..95 {
            sharkron(&mut s, &w);
        }
        assert_eq!(s.ai[0], 1.0, "it should have committed");
        let speed = s.velocity.0.hypot(s.velocity.1);
        assert!((speed - 16.0).abs() < 0.1, "at its full speed, got {speed}");

        // Moving the player afterwards does not change its line.
        let aside = world(&tiles, Some((-600.0, 0.0)));
        let before = s.velocity;
        sharkron(&mut s, &aside);
        assert_eq!(s.velocity, before, "it does not steer once committed");
    }
}
