//! The Golem: styles 45–48.
//!
//! Four pieces that fight as one, and the order you take them apart in changes the fight rather
//! than merely shortening it:
//!
//! * The **body** (45) hops. Its charge fills faster for every part already destroyed and every
//!   health threshold crossed, so a Golem you have disarmed is a Golem that comes at you twice as
//!   often. It cannot be hurt at all while its head is attached.
//! * The **head** (46) rides on the body and spits fireballs on a fixed cycle.
//! * The **fists** (47) hold stations either side, wind up, and punch — but only at a player on
//!   their own side, so the left fist cannot reach you if you stay to the right.
//! * Kill the head and it comes back **free** (48), hovering three hundred pixels above you
//!   (`NPC.cs:85913-85918`). The body is still alive at that point, and stays the thing every
//!   threshold the free head has is measured against.
//!
//! And fighting it anywhere but the temple doubles every rate in it. That is not a difficulty
//! setting; it is the game refusing to be dragged out of the fight it designed.

use terrustia_proto::npc_params::{
    GOLEM_AIR_ACCEL, GOLEM_AIR_SPEED, GOLEM_AIR_SPEED_GET_GOOD, GOLEM_FIREBALL_DAMAGE,
    GOLEM_FIREBALL_DAMAGE_UPGRADED, GOLEM_FIREBALL_SPEED, GOLEM_FIST_LEFT, GOLEM_FIST_OFFSET,
    GOLEM_FIST_REACH, GOLEM_FIST_READY, GOLEM_FIST_RETURN, GOLEM_FIST_RETURN_BODY_HURT,
    GOLEM_FIST_RETURN_CAP, GOLEM_FIST_RETURN_HALF, GOLEM_FIST_RETURN_QUARTER, GOLEM_FIST_WINDUP,
    GOLEM_FREE_ABOVE, GOLEM_FREE_ACCEL, GOLEM_FREE_FIREBALL_DAMAGE, GOLEM_FREE_FIREBALL_STEPS,
    GOLEM_FREE_LASER_DAMAGE, GOLEM_FREE_LASER_DAMAGE_STEPS, GOLEM_FREE_LASER_INTERVAL,
    GOLEM_FREE_LASER_INTERVAL_STEPS, GOLEM_FREE_LASER_NO_LOS_BONUS,
    GOLEM_FREE_LASER_NO_LOS_DAMAGE_MULT, GOLEM_FREE_LASER_NO_LOS_SPEED_MULT,
    GOLEM_FREE_LASER_SPEED, GOLEM_FREE_SPEED, GOLEM_HEAD_CHARGE, GOLEM_HEAD_OFFSET,
    GOLEM_HEAD_TETHER_SPEED, GOLEM_HOP_ACROSS, GOLEM_HOP_BONUS_HALF, GOLEM_HOP_BONUS_HURT,
    GOLEM_HOP_BONUS_PART, GOLEM_HOP_BONUS_THIRD, GOLEM_HOP_PAUSE, GOLEM_HOP_READY, GOLEM_HOP_UP,
    GOLEM_HOP_UP_CAP, GOLEM_LASER_DAMAGE, GOLEM_LASER_INTERVAL, GOLEM_LASER_NO_LOS_BONUS,
    GOLEM_LASER_SPEED, GOLEM_LASER_SPEED_OFFSIDE, GOLEM_LEASH, GOLEM_OUTSIDE_PENALTY,
    GOLEM_PUNCH_BODY_HURT, GOLEM_PUNCH_CAP, GOLEM_PUNCH_HALF, GOLEM_PUNCH_QUARTER,
    GOLEM_PUNCH_REACH, GOLEM_PUNCH_SPEED, GOLEM_SLAM,
};
use terrustia_proto::projectile::ids::{GOLEM_FIREBALL, GOLEM_LASER};

use super::skeletron::Parent;
use crate::game::ai::{PLAYER_HEIGHT, PLAYER_WIDTH, Shot, World, face, sight, target_box, unit};
use crate::game::npc::{Npc, TileView};
use crate::game::npc_ai::Spawn;

/// Which of its parts are still standing, and where it is being fought.
#[derive(Debug, Clone, Copy)]
pub struct GolemState {
    pub head: bool,
    pub left_fist: bool,
    pub right_fist: bool,
    /// Whether the player is in the temple or jungle, and underground. Outside it, everything the
    /// Golem does happens twice as fast.
    pub at_home: bool,
    /// GOL-2: the game's own per-player balance factor (`GetMyBalance`, `NPC.cs:19547`), read off the
    /// part that spawned it: one for a lone player, higher on a crowded server. The old code hardcoded
    /// this to one, so a multiplayer Golem fought at single-player pace.
    pub balance: f32,
}

impl GolemState {
    /// The multiplier every rate in the fight is scaled by.
    fn pace(&self) -> f32 {
        // Its base pace is the game's per-player balance; dragged out of the temple, everything then
        // runs at twice that (`num *= 2f`, `NPC.cs:19554`). (Vanilla's separate `+2` in a
        // For-the-Worthy world is not modelled here.)
        let base = self.balance;
        if self.at_home {
            base
        } else {
            base * GOLEM_OUTSIDE_PENALTY
        }
    }
}

/// What a piece of the Golem did this tick.
#[derive(Debug, Default)]
pub struct GolemOutcome {
    pub shots: Vec<Shot>,
    pub spawn: Vec<Spawn>,
    /// Set when this piece has outlived whatever it hangs off.
    pub spent: bool,
}

/// Style 45: the body.
pub fn body(npc: &mut Npc, world: &World<'_, impl TileView>, state: GolemState) -> GolemOutcome {
    let mut out = GolemOutcome::default();
    npc.dirty = true;

    // On its first tick it assembles itself.
    if npc.local_ai[0] == 0.0 {
        npc.local_ai[0] = 1.0;
        let (cx, cy) = npc.center();
        for (npc_type, offset) in [
            (GOLEM_FIST_LEFT, (-GOLEM_FIST_OFFSET.0, GOLEM_FIST_OFFSET.1)),
            (
                terrustia_proto::npc_params::GOLEM_FIST_RIGHT,
                (GOLEM_FIST_OFFSET.0 - 6.0, GOLEM_FIST_OFFSET.1),
            ),
            (terrustia_proto::npc_params::GOLEM_HEAD, GOLEM_HEAD_OFFSET),
        ] {
            out.spawn.push(Spawn {
                handle: None,
                npc_type,
                position: (cx + offset.0, cy + offset.1),
                velocity: (0.0, 0.0),
                parent: Some(Spawn::OWN_PARENT),
                ai: [None; 4],
            });
        }
    }

    // While its head is on, the body itself cannot be hurt at all.
    npc.invulnerable = state.head;
    npc.alpha = (npc.alpha - 10).max(0);

    let Some(target) = world.target.filter(|t| t.alive) else {
        npc.no_tile_collide = true;
        return out;
    };

    // A jump turns tile collision off so the Golem can leave the platform it is standing on. This
    // is what turns it back on: either it has fallen far enough to be coming down on the player,
    // or it has a clear line to them and is not buried. Without this it would sink through the
    // floor and never land again.
    if npc.no_tile_collide {
        let below_the_player = npc.velocity.1 > 0.0
            && npc.position.1 + npc.height()
                > target.center.1 - crate::game::ai::PLAYER_HEIGHT as f32 / 2.0;
        let in_the_open =
            crate::game::ai::can_see(world.tiles, npc, target) && !boxed(world.tiles, npc);
        if below_the_player || in_the_open {
            npc.no_tile_collide = false;
        }
    }

    let (cx, cy) = npc.center();
    // Too far to keep fighting.
    if (cx - target.center.0).abs() + (cy - target.center.1).abs() > GOLEM_LEASH {
        out.spent = true;
        return out;
    }
    let pace = state.pace();

    if npc.ai[0] == 0.0 {
        // On the ground, winding up.
        if npc.velocity.1 != 0.0 {
            return out;
        }
        npc.velocity.0 *= 0.8;
        let mut rate = 1.0;
        if npc.ai[1] > 0.0 {
            // Every part already gone makes the next hop come sooner.
            for present in [state.head, state.left_fist, state.right_fist] {
                if !present {
                    rate += GOLEM_HOP_BONUS_PART;
                }
            }
            if npc.life < npc.life_max {
                rate += GOLEM_HOP_BONUS_HURT;
            }
            if npc.life < npc.life_max / 2 {
                rate += GOLEM_HOP_BONUS_HALF;
            }
            if npc.life < npc.life_max / 3 {
                rate += GOLEM_HOP_BONUS_THIRD;
            }
            rate *= pace;
        }
        npc.ai[1] += rate;
        if npc.ai[1] >= GOLEM_HOP_READY {
            // A short pause with its feet planted before it goes, which is the tell.
            npc.ai[1] = GOLEM_HOP_PAUSE;
        } else if npc.ai[1] >= -1.0 && npc.ai[1] < 0.0 {
            npc.no_tile_collide = true;
            face(npc, target);
            npc.velocity.0 = GOLEM_HOP_ACROSS * f32::from(npc.direction);
            npc.velocity.1 = if npc.life < npc.life_max {
                // A hurt Golem jumps higher, up to a limit.
                (GOLEM_HOP_UP * (pace + 9.0) / 10.0).max(GOLEM_HOP_UP_CAP)
            } else {
                GOLEM_HOP_UP
            };
            npc.ai[0] = 1.0;
            npc.ai[1] = 0.0;
        }
        return out;
    }

    // In the air.
    if npc.velocity.1 == 0.0 {
        npc.ai[0] = 0.0;
        return out;
    }
    face(npc, target);
    let over_the_player = npc.position.0 < target.center.0 - PLAYER_WIDTH as f32 / 2.0
        && npc.position.0 + npc.width() > target.center.0 + PLAYER_WIDTH as f32 / 2.0;
    if over_the_player {
        // Directly above: it stops steering and comes down on you.
        npc.velocity.0 *= 0.9;
        if npc.position.1 + npc.height() < target.center.1 {
            npc.velocity.1 += GOLEM_SLAM * (pace + 1.0) / 2.0;
        }
    } else {
        npc.velocity.0 += GOLEM_AIR_ACCEL * f32::from(npc.direction);
        // For the worthy more than doubles the airborne cap (`NPC.cs:46006-46010`).
        // `Conditions::get_good_world` was read by only two routines in the whole workspace, so
        // every other seed-specific behaviour, this one included, was silently the ordinary one.
        let mut cap = if world.conditions.get_good_world {
            GOLEM_AIR_SPEED_GET_GOOD
        } else {
            GOLEM_AIR_SPEED
        };
        if npc.life < npc.life_max {
            cap += 1.0;
        }
        if npc.life < npc.life_max / 2 {
            cap += 1.0;
        }
        if npc.life < npc.life_max / 4 {
            cap += 1.0;
        }
        cap *= (pace + 1.0) / 2.0;
        npc.velocity.0 = npc.velocity.0.clamp(-cap, cap);
    }
    out
}

/// Style 46: the head, while it is still attached.
pub fn head(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    body: Option<Parent>,
    state: GolemState,
) -> GolemOutcome {
    let mut out = GolemOutcome::default();
    npc.dirty = true;
    npc.no_tile_collide = true;
    npc.alpha = (npc.alpha - 10).max(0);

    let Some(body) = body else {
        out.spent = true;
        return out;
    };
    // It is towed by the body rather than following it: the velocity is the whole gap, capped.
    let (bx, by) = body.center();
    let (cx, cy) = npc.center();
    let station = (
        bx + GOLEM_HEAD_OFFSET.0 * npc.scale - cx,
        by + GOLEM_HEAD_OFFSET.1 * npc.scale - cy,
    );
    let gap = station.0.hypot(station.1);
    if gap < GOLEM_HEAD_TETHER_SPEED {
        npc.rotation = 0.0;
        npc.velocity = station;
    } else {
        let scale = GOLEM_HEAD_TETHER_SPEED / gap;
        npc.velocity = (station.0 * scale, station.1 * scale);
        npc.rotation = npc.velocity.0 * 0.1;
    }

    let Some(target) = world.target.filter(|t| t.alive) else {
        return out;
    };
    // Past half health it hits harder with the fireball, and starts growing eye-lasers too. That
    // is vanilla's `ai[0]`, set from health at the very end of the style each tick
    // (`NPC.cs:31566-31573`, `if (life < lifeMax / 2) ai[0] = 1f;`).
    let health = npc.life as f32 / npc.life_max.max(1) as f32;
    let hurt = health < 0.5;

    let pace = state.pace();
    // GOL-M12: the two phases charge on different clocks, and only the first one has the rhythm.
    //
    // Phase zero runs faster at the ends of its cycle, which is what gives its fireballs their
    // uneven beat rather than a metronome (`NPC.cs:31399-31411`). Phase one drops the rhythm
    // entirely for a flat `(pace + 3) / 4` per tick, and then adds that step again under forty
    // percent health and once more under twenty (`NPC.cs:31450-31459`) - so a nearly-dead head
    // spits three times as often. The old code ran phase zero's rhythm in both phases and had no
    // health steps at all, which left the second phase stuck at one 300-tick cycle throughout.
    let step = (pace + 3.0) / 4.0;
    if hurt {
        npc.ai[1] += step;
        for at in [0.4, 0.2] {
            if health < at {
                npc.ai[1] += step;
            }
        }
    } else {
        npc.ai[1] += if npc.ai[1] < 20.0 || npc.ai[1] > GOLEM_HEAD_CHARGE - 20.0 {
            1.0 + 2.0 * (pace - 1.0) / 3.0
        } else {
            1.0 * (pace - 1.0).max(0.0) / 2.0 + 1.0
        };
    }
    if npc.ai[1] >= GOLEM_HEAD_CHARGE {
        npc.ai[1] = 0.0;
        let from = (cx, cy + 10.0 * npc.scale);
        let aim = unit(
            (target.center.0 - from.0, target.center.1 - from.1),
            GOLEM_FIREBALL_SPEED,
        );
        out.shots.push(Shot {
            projectile: GOLEM_FIREBALL,
            damage: if hurt {
                GOLEM_FIREBALL_DAMAGE_UPGRADED
            } else {
                GOLEM_FIREBALL_DAMAGE
            },
            position: from,
            velocity: aim,
            time_left: 600,
            ai: [0.0; 3],
        });
    }

    if hurt {
        // The laser charge is on the same phase-one clock as the fireball, not on the raw pace
        // (`NPC.cs:31485-31501`, `ai[2] += num733`).
        npc.ai[2] += step;
        for at in [1.0 / 3.0, 1.0 / 4.0, 1.0 / 5.0] {
            if health < at {
                npc.ai[2] += step;
            }
        }
        if !crate::game::ai::can_see(world.tiles, npc, target) {
            npc.ai[2] += GOLEM_LASER_NO_LOS_BONUS;
        }
        if npc.ai[2] >= GOLEM_LASER_INTERVAL {
            npc.ai[2] = 0.0;
            // Centred on you it fires a pair; off to one side of the body, just one.
            let body_width = body.size.0;
            let centered = target.center.0 >= bx - body_width && target.center.0 <= bx + body_width;
            let speed = if centered {
                GOLEM_LASER_SPEED
            } else {
                GOLEM_LASER_SPEED_OFFSIDE
            };
            let aim = unit((target.center.0 - cx, target.center.1 - cy), speed);
            let volleys = if centered { 2 } else { 1 };
            for _ in 0..volleys {
                out.shots.push(Shot {
                    projectile: GOLEM_LASER,
                    damage: GOLEM_LASER_DAMAGE,
                    position: (cx, cy),
                    velocity: aim,
                    time_left: 300,
                    ai: [0.0; 3],
                });
            }
        }
    }
    out
}

/// Style 47: a fist.
pub fn fist(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    body: Option<Parent>,
    state: GolemState,
) -> GolemOutcome {
    let mut out = GolemOutcome::default();
    npc.dirty = true;
    npc.alpha = (npc.alpha - 10).max(0);

    let Some(body) = body else {
        out.spent = true;
        return out;
    };
    let pace = state.pace();
    let left = npc.npc_type == GOLEM_FIST_LEFT;
    let (bx, by) = body.center();
    // The station moves with the body's velocity as well as its position, so a fist keeps up with
    // a hopping Golem rather than trailing it.
    let station = (
        bx + body.velocity.0
            + if left {
                -GOLEM_FIST_OFFSET.0
            } else {
                GOLEM_FIST_OFFSET.0 - 6.0
            } * npc.scale,
        by + body.velocity.1 - 9.0 * npc.scale,
    );
    let (cx, cy) = npc.center();
    let to_station = (station.0 - cx, station.1 - cy);
    let gap = to_station.0.hypot(to_station.1);

    match npc.ai[0] {
        0.0 => {
            // Holding station, and winding up while it is there.
            npc.no_tile_collide = true;
            let mut speed = GOLEM_FIST_RETURN;
            if npc.life < npc.life_max / 2 {
                speed += GOLEM_FIST_RETURN_HALF;
            }
            if npc.life < npc.life_max / 4 {
                speed += GOLEM_FIST_RETURN_QUARTER;
            }
            if body.health < 1.0 {
                speed += GOLEM_FIST_RETURN_BODY_HURT;
            }
            speed = (speed * (pace + 3.0) / 4.0).min(GOLEM_FIST_RETURN_CAP);

            if gap < 12.0 + speed {
                // Home. Wind up.
                npc.rotation = 0.0;
                npc.velocity = to_station;
                let mut rate = pace;
                if npc.life < npc.life_max / 2 {
                    rate += pace;
                }
                if npc.life < npc.life_max / 4 {
                    rate += pace;
                }
                if body.health < 1.0 {
                    // A hurt body makes its fists punch ten times as often.
                    rate += 10.0 * pace;
                }
                npc.ai[1] += rate;
                if npc.ai[1] >= GOLEM_FIST_READY {
                    npc.ai[1] = 0.0;
                    // A fist only punches at somebody on its own side.
                    if let Some(target) = world.target.filter(|t| t.alive) {
                        let reachable = if left {
                            cx + GOLEM_FIST_REACH > target.center.0
                        } else {
                            cx - GOLEM_FIST_REACH < target.center.0
                        };
                        if reachable {
                            npc.ai[0] = 1.0;
                        }
                    }
                }
            } else {
                let scale = speed / gap;
                npc.velocity = (to_station.0 * scale, to_station.1 * scale);
                npc.rotation = if left {
                    npc.velocity.1.atan2(npc.velocity.0)
                } else {
                    (-npc.velocity.1).atan2(-npc.velocity.0)
                };
            }
        }

        1.0 => {
            // Cocked. It is pinned to the station for half a second and then launches.
            npc.ai[1] += 1.0;
            npc.position = (
                station.0 - npc.width() / 2.0,
                station.1 - npc.height() / 2.0,
            );
            npc.rotation = 0.0;
            npc.velocity = (0.0, 0.0);
            if npc.ai[1] >= GOLEM_FIST_WINDUP {
                npc.no_tile_collide = true;
                // Clear any stale collision flags so the punch cannot retract on its very first tick
                // (`NPC.cs:19451-19453`, `collideX = false; collideY = false;` at launch).
                npc.collide_x = false;
                npc.collide_y = false;
                npc.ai[0] = 2.0;
                npc.ai[1] = 0.0;
                let mut speed = GOLEM_PUNCH_SPEED;
                if npc.life < npc.life_max / 2 {
                    speed += GOLEM_PUNCH_HALF;
                }
                if npc.life < npc.life_max / 4 {
                    speed += GOLEM_PUNCH_QUARTER;
                }
                if body.health < 1.0 {
                    speed += GOLEM_PUNCH_BODY_HURT;
                }
                speed = (speed * (pace + 3.0) / 4.0).min(GOLEM_PUNCH_CAP);
                if let Some(target) = world.target {
                    npc.velocity = unit((target.center.0 - cx, target.center.1 - cy), speed);
                }
                npc.rotation = if left {
                    (-npc.velocity.1).atan2(-npc.velocity.0)
                } else {
                    npc.velocity.1.atan2(npc.velocity.0)
                };
            }
        }

        _ => {
            // GOL-1: punching. It travels the line it committed to. It phases through terrain on the
            // way out, but turns solid the moment it is past the player, so a wall behind them stops
            // it (`NPC.cs:19461-19482`). And it goes home by distance, not a fixed timer: once the
            // fist is more than its reach from the station, or it has struck a wall
            // (`NPC.cs:19483-19487`, `num2 > 700f || collideX || collideY`). The old code phased
            // through everything and always returned after a flat sixty ticks.
            npc.ai[1] += 1.0;
            if let Some(target) = world.target {
                if npc.velocity.0.abs() > npc.velocity.1.abs() {
                    if (npc.velocity.0 > 0.0 && cx > target.center.0)
                        || (npc.velocity.0 < 0.0 && cx < target.center.0)
                    {
                        npc.no_tile_collide = false;
                    }
                } else if (npc.velocity.1 > 0.0 && cy > target.center.1)
                    || (npc.velocity.1 < 0.0 && cy < target.center.1)
                {
                    npc.no_tile_collide = false;
                }
            }
            if gap > GOLEM_PUNCH_REACH || npc.collide_x || npc.collide_y {
                npc.no_tile_collide = true;
                npc.ai[0] = 0.0;
                npc.ai[1] = 0.0;
            }
        }
    }
    out
}

/// Style 48: the head once it has been knocked off the body.
///
/// GOL-M13: every threshold in this style keys on the **body**, not on the free head itself
/// (`Main.npc[golemBoss]` throughout `NPC.cs:31645-31778`). The head comes off when the attached
/// head dies (`NPC.cs:85913-85918`), which is well before the body does, so "how hurt is the
/// Golem" is a question only the body can answer. Reading its own health instead meant the free
/// head never accelerated at all: its fireball stayed on a flat 300-tick cycle where vanilla's
/// reaches sixty, and its laser never picked up either.
pub fn free_head(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    body: Option<Parent>,
    state: GolemState,
) -> GolemOutcome {
    let mut out = GolemOutcome::default();
    npc.dirty = true;

    let Some(body) = body else {
        // No body to key off: vanilla strikes the head for 9999 and returns (`NPC.cs:31599-31603`).
        out.spent = true;
        return out;
    };
    let Some(target) = world.target.filter(|t| t.alive) else {
        return out;
    };
    let health = body.health;
    // It comes through terrain when it cannot see you, and becomes solid again once it can.
    let seen = crate::game::ai::can_see(world.tiles, npc, target);
    if !seen {
        npc.no_tile_collide = true;
    } else if npc.no_tile_collide && !boxed(world.tiles, npc) {
        npc.no_tile_collide = false;
    }

    let (cx, cy) = npc.center();
    let wanted = unit(
        (
            target.center.0 - cx,
            target.center.1 - GOLEM_FREE_ABOVE - cy,
        ),
        GOLEM_FREE_SPEED,
    );
    for (v, w) in [
        (&mut npc.velocity.0, wanted.0),
        (&mut npc.velocity.1, wanted.1),
    ] {
        if *v < w {
            *v += GOLEM_FREE_ACCEL;
            if *v < 0.0 && w > 0.0 {
                *v += GOLEM_FREE_ACCEL;
            }
        } else if *v > w {
            *v -= GOLEM_FREE_ACCEL;
            if *v > 0.0 && w < 0.0 {
                *v -= GOLEM_FREE_ACCEL;
            }
        }
    }

    // It keeps spitting fireballs, harder than the attached head's (`NPC.cs:31674-31693`), on a
    // cycle that steps up four times as the *body* is worn down (`NPC.cs:31644-31661`). And it
    // will not fire through a wall: the charge is pinned at twenty for as long as it cannot see
    // you, so the shot lands on the tick it comes back into view (`NPC.cs:31669-31672`).
    let pace = state.pace();
    let fireball_step = (pace + 4.0) / 5.0;
    npc.ai[1] += fireball_step;
    for at in GOLEM_FREE_FIREBALL_STEPS {
        if health < at {
            npc.ai[1] += fireball_step;
        }
    }
    if !seen {
        npc.ai[1] = 20.0;
    }
    if npc.ai[1] >= GOLEM_HEAD_CHARGE {
        npc.ai[1] = 0.0;
        let from = (cx, cy - 10.0 * npc.scale);
        out.shots.push(Shot {
            projectile: GOLEM_FIREBALL,
            damage: GOLEM_FREE_FIREBALL_DAMAGE,
            position: from,
            velocity: unit(
                (target.center.0 - from.0, target.center.1 - from.1),
                GOLEM_FIREBALL_SPEED,
            ),
            time_left: 600,
            ai: [0.0; 3],
        });
    }

    // Eye-lasers, always present on the free head: a slower cadence than the fireball's, one
    // that quickens as the body is worn down and while the *body* cannot see you, and hits
    // harder and faster once the body is badly hurt (`NPC.cs:31694-31778`). The line of sight
    // that matters here is `flag55`, cast from the body's centre (`NPC.cs:31726-31734`), not the
    // head's own; they are different points, and only the head's decides tile collision above.
    let body_blind = !sight::can_hit(
        world.tiles,
        body.position,
        (body.size.0 as i32, body.size.1 as i32),
        target_box(target),
        (PLAYER_WIDTH, PLAYER_HEIGHT),
    );
    npc.ai[2] += pace;
    for at in GOLEM_FREE_LASER_INTERVAL_STEPS {
        if health < at {
            npc.ai[2] += pace;
        }
    }
    if body_blind {
        npc.ai[2] += pace * GOLEM_FREE_LASER_NO_LOS_BONUS;
    }
    if npc.ai[2] >= GOLEM_FREE_LASER_INTERVAL {
        npc.ai[2] = 0.0;
        let mut damage = GOLEM_FREE_LASER_DAMAGE;
        let mut speed = GOLEM_FREE_LASER_SPEED;
        for at in GOLEM_FREE_LASER_DAMAGE_STEPS {
            if health < at {
                damage += 1;
                speed += 0.25;
            }
        }
        if body_blind {
            damage = (damage as f32 * GOLEM_FREE_LASER_NO_LOS_DAMAGE_MULT) as i32;
            speed *= GOLEM_FREE_LASER_NO_LOS_SPEED_MULT;
        }
        let aim = unit((target.center.0 - cx, target.center.1 - cy), speed);
        for _ in 0..2 {
            out.shots.push(Shot {
                projectile: GOLEM_LASER,
                damage,
                position: (cx, cy),
                velocity: aim,
                time_left: 300,
                ai: [0.0; 3],
            });
        }
    }
    out
}

fn boxed(tiles: &impl TileView, npc: &Npc) -> bool {
    let tile = crate::game::npc::TILE;
    let x0 = (npc.position.0 / tile).floor() as i32;
    let x1 = ((npc.position.0 + npc.width() - 1.0) / tile).floor() as i32;
    let y0 = (npc.position.1 / tile).floor() as i32;
    let y1 = ((npc.position.1 + npc.height() - 1.0) / tile).floor() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let t = tiles.tile(x, y);
            if t.is_active() && terrustia_proto::tile_solid::solid(t.block) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc::TILE;
    use crate::game::npc_ai::Target;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::{GOLEM_BODY, GOLEM_FIST_RIGHT, GOLEM_HEAD, GOLEM_HEAD_FREE};
    use terrustia_proto::tile::Tile;

    struct Temple(HashMap<(i32, i32), Tile>);

    impl TileView for Temple {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn floor(at: i32) -> Temple {
        let mut tiles = HashMap::new();
        for x in -300..300 {
            for y in at..at + 4 {
                tiles.insert((x, y), Tile::block(1));
            }
        }
        Temple(tiles)
    }

    fn world<'a>(tiles: &'a Temple, target: Option<(f32, f32)>) -> World<'a, Temple> {
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

    fn whole() -> GolemState {
        GolemState {
            head: true,
            left_fist: true,
            right_fist: true,
            at_home: true,
            balance: 1.0,
        }
    }

    fn body_at(position: (f32, f32)) -> Parent {
        Parent {
            position,
            size: (100.0, 100.0),
            rotation: 0.0,
            scale: 1.0,
            velocity: (0.0, 0.0),
            direction: 1,
            sprite_direction: 1,
            time_left: 3600,
            state: 0.0,
            phase: 0.0,
            health: 1.0,
        }
    }

    fn piece(npc_type: u16, tile_x: i32, tile_y: i32) -> Npc {
        Npc::new(npc_type, (tile_x as f32 * TILE, tile_y as f32 * TILE), 1)
            .expect("a piece of the Golem")
    }

    /// It builds itself out of its parts on the first tick.
    #[test]
    fn the_golem_assembles_itself() {
        let tiles = floor(30);
        let w = world(&tiles, Some((300.0, 400.0)));
        let mut g = piece(GOLEM_BODY, 0, 25);
        let out = body(&mut g, &w, whole());
        let types: Vec<u16> = out.spawn.iter().map(|s| s.npc_type).collect();
        assert!(types.contains(&GOLEM_HEAD), "a head: {types:?}");
        assert!(types.contains(&GOLEM_FIST_LEFT), "a left fist: {types:?}");
        assert!(types.contains(&GOLEM_FIST_RIGHT), "a right fist: {types:?}");
        // And only once.
        assert!(body(&mut g, &w, whole()).spawn.is_empty());
    }

    /// While its head is on, the body cannot be hurt.
    #[test]
    fn the_body_is_untouchable_until_the_head_is_off() {
        let tiles = floor(30);
        let w = world(&tiles, Some((300.0, 400.0)));
        let mut g = piece(GOLEM_BODY, 0, 25);
        body(&mut g, &w, whole());
        assert!(g.invulnerable, "the head is protecting it");

        let headless = GolemState {
            head: false,
            ..whole()
        };
        body(&mut g, &w, headless);
        assert!(!g.invulnerable, "head off, body open");
    }

    /// For the worthy more than doubles its airborne speed cap (`NPC.cs:46006-46010`).
    ///
    /// `Conditions::get_good_world` was read by only two routines in the whole workspace, so this
    /// seed's Golem chased at the ordinary three pixels a tick.
    #[test]
    fn for_the_worthy_chases_far_faster_in_the_air() {
        let tiles = floor(30);
        // The same fixture the hop test uses: on the floor, charging, with the player far enough
        // off that it hops after them rather than standing still.
        let top_speed = |get_good: bool| {
            let mut g = piece(GOLEM_BODY, 0, 25);
            g.local_ai[0] = 1.0;
            g.ai[1] = 1.0;
            let mut w = world(&tiles, Some((2000.0, 29.0 * TILE)));
            w.conditions.get_good_world = get_good;
            let mut fastest: f32 = 0.0;
            for _ in 0..3000 {
                body(&mut g, &w, whole());
                fastest = fastest.max(g.velocity.0.abs());
                crate::game::npc::step_physics(&mut g, &tiles);
            }
            fastest
        };
        let ordinary = top_speed(false);
        let worthy = top_speed(true);
        assert!(
            worthy > ordinary * 1.5,
            "for the worthy should run it down far harder: {worthy} against {ordinary}"
        );
    }

    /// Every part destroyed makes it hop sooner. That is the whole shape of the fight.
    #[test]
    fn a_dismantled_golem_hops_more_often() {
        let tiles = floor(30);
        let w = world(&tiles, Some((200.0, 29.0 * TILE)));
        let hops = |state: GolemState| {
            let mut g = piece(GOLEM_BODY, 0, 25);
            g.local_ai[0] = 1.0;
            // Start it charging rather than at rest.
            g.ai[1] = 1.0;
            let mut count = 0;
            let mut grounded = true;
            for _ in 0..3000 {
                body(&mut g, &w, state);
                if grounded && g.velocity.1 < 0.0 {
                    count += 1;
                }
                grounded = g.velocity.1 == 0.0;
                crate::game::npc::step_physics(&mut g, &tiles);
            }
            count
        };
        let intact = hops(whole());
        let stripped = hops(GolemState {
            head: false,
            left_fist: false,
            right_fist: false,
            at_home: true,
            balance: 1.0,
        });
        assert!(
            stripped > intact,
            "a stripped Golem should hop more: {stripped} vs {intact}"
        );
    }

    /// GOL-2: on a crowded server the Golem fights faster, because its base pace is the per-player
    /// balance (`GetMyBalance`, `NPC.cs:19547`) rather than a hardcoded one. The old code pinned the
    /// factor to one, so a multiplayer Golem hopped at single-player pace.
    #[test]
    fn a_golem_scaled_for_a_crowd_hops_faster() {
        let tiles = floor(30);
        let w = world(&tiles, Some((200.0, 29.0 * TILE)));
        let hops = |balance: f32| {
            let mut g = piece(GOLEM_BODY, 0, 25);
            g.local_ai[0] = 1.0;
            g.ai[1] = 1.0;
            let state = GolemState { balance, ..whole() };
            let mut count = 0;
            let mut grounded = true;
            for _ in 0..3000 {
                body(&mut g, &w, state);
                if grounded && g.velocity.1 < 0.0 {
                    count += 1;
                }
                grounded = g.velocity.1 == 0.0;
                crate::game::npc::step_physics(&mut g, &tiles);
            }
            count
        };
        // One player is the vanilla flat 1.0; four players scale it up (GetStatScalingFactors).
        let lone = hops(1.0);
        let crowd = hops(terrustia_proto::difficulty::balance(4));
        assert!(
            crowd > lone,
            "a Golem scaled for a crowd should hop more often: {crowd} vs {lone}"
        );
    }

    /// Fighting it outside the temple doubles its pace.
    #[test]
    fn dragging_it_out_of_the_temple_makes_it_worse() {
        let tiles = floor(30);
        let w = world(&tiles, Some((200.0, 29.0 * TILE)));
        let hops = |at_home: bool| {
            let mut g = piece(GOLEM_BODY, 0, 25);
            g.local_ai[0] = 1.0;
            g.ai[1] = 1.0;
            let state = GolemState { at_home, ..whole() };
            let mut count = 0;
            let mut grounded = true;
            for _ in 0..3000 {
                body(&mut g, &w, state);
                if grounded && g.velocity.1 < 0.0 {
                    count += 1;
                }
                grounded = g.velocity.1 == 0.0;
                crate::game::npc::step_physics(&mut g, &tiles);
            }
            count
        };
        assert!(
            hops(false) > hops(true),
            "outside the temple it should be faster: {} vs {}",
            hops(false),
            hops(true)
        );
    }

    /// A part with no body does not survive.
    #[test]
    fn the_parts_die_with_the_body() {
        let tiles = floor(30);
        let w = world(&tiles, Some((300.0, 400.0)));
        let mut h = piece(GOLEM_HEAD, 0, 25);
        assert!(head(&mut h, &w, None, whole()).spent);
        let mut f = piece(GOLEM_FIST_LEFT, 0, 25);
        assert!(fist(&mut f, &w, None, whole()).spent);
    }

    /// The head spits fireballs on its cycle.
    #[test]
    fn the_head_spits_fireballs() {
        let tiles = floor(30);
        let w = world(&tiles, Some((300.0, 400.0)));
        let mut h = piece(GOLEM_HEAD, 0, 25);
        let mut shots = Vec::new();
        for _ in 0..1200 {
            shots.extend(head(&mut h, &w, Some(body_at((0.0, 400.0))), whole()).shots);
        }
        assert!(!shots.is_empty(), "it should have thrown something");
        assert!(shots.iter().all(|s| s.projectile == GOLEM_FIREBALL));
    }

    /// B8: a healthy attached head has no eye-lasers at all — those only grow in past half health.
    #[test]
    fn a_healthy_head_has_no_lasers() {
        let tiles = floor(30);
        let w = world(&tiles, Some((300.0, 400.0)));
        let mut h = piece(GOLEM_HEAD, 0, 25);
        let mut lasers = 0;
        for _ in 0..2000 {
            lasers += head(&mut h, &w, Some(body_at((0.0, 400.0))), whole())
                .shots
                .iter()
                .filter(|s| s.projectile == GOLEM_LASER)
                .count();
        }
        assert_eq!(lasers, 0, "a healthy head should not have lasers yet");
    }

    /// B8: past half health the attached head also fires eye-lasers, alongside a harder fireball.
    #[test]
    fn a_hurt_head_fires_eye_lasers_and_a_harder_fireball() {
        let tiles = floor(30);
        let w = world(&tiles, Some((300.0, 400.0)));
        let mut h = piece(GOLEM_HEAD, 0, 25);
        h.life = h.life_max / 4;
        let mut lasers = 0;
        let mut fireball_damage = None;
        for _ in 0..2000 {
            let out = head(&mut h, &w, Some(body_at((0.0, 400.0))), whole());
            for s in &out.shots {
                if s.projectile == GOLEM_LASER {
                    lasers += 1;
                } else if s.projectile == GOLEM_FIREBALL {
                    fireball_damage = Some(s.damage);
                }
            }
        }
        assert!(lasers > 0, "a hurt head should fire lasers too");
        assert_eq!(
            fireball_damage,
            Some(GOLEM_FIREBALL_DAMAGE_UPGRADED),
            "and its fireball should hit harder"
        );
    }

    /// GOL-M12: the head's second phase charges on its own clock, and steps up twice more as it
    /// dies (`num733 = (num720 + 3f) / 4f`, doubled under forty per cent and tripled under twenty,
    /// `NPC.cs:31450-31459`).
    ///
    /// The old code ran phase zero's edge-of-cycle rhythm in both phases and had neither health
    /// step, so a head at five per cent spat exactly as slowly as one at forty-five: a flat
    /// 300-tick cycle where vanilla is down to a hundred.
    #[test]
    fn the_hurt_head_spits_faster_the_closer_it_is_to_dying() {
        let tiles = floor(30);
        let w = world(&tiles, Some((300.0, 400.0)));
        let fireballs = |percent: i32| {
            let mut h = piece(GOLEM_HEAD, 0, 25);
            h.life = h.life_max * percent / 100;
            (0..1200)
                .flat_map(|_| head(&mut h, &w, Some(body_at((0.0, 400.0))), whole()).shots)
                .filter(|s| s.projectile == GOLEM_FIREBALL)
                .count()
        };
        // Both are in the second phase (under half health). At forty-five per cent the charge is
        // one a tick, so three hundred ticks a cycle; under twenty it is three a tick, so a
        // hundred.
        assert_eq!(fireballs(45), 4, "four cycles in twelve hundred ticks");
        assert_eq!(
            fireballs(15),
            12,
            "and three times as many once nearly dead"
        );
    }

    /// GOL-M13: every threshold the free head has is measured on the **body**
    /// (`Main.npc[golemBoss]`, `NPC.cs:31645-31661`), and its charge is paced by `(pace + 4) / 5`
    /// rather than advancing flat.
    ///
    /// Reading its own health with a flat `+= 1` left the free head on one 300-tick fireball cycle
    /// for the whole fight, where vanilla's reaches sixty ticks once the body is nearly dead.
    #[test]
    fn the_free_heads_fireball_keys_on_the_bodys_health() {
        let tiles = Temple(HashMap::new());
        let w = world(&tiles, Some((300.0, 400.0)));
        let fireballs = |body_health: f32| {
            // The head itself is untouched throughout: only the body's health may move this.
            let mut h = piece(GOLEM_HEAD_FREE, 0, 40);
            let mut body = body_at((0.0, 400.0));
            body.health = body_health;
            (0..1200)
                .flat_map(|_| free_head(&mut h, &w, Some(body), whole()).shots)
                .filter(|s| s.projectile == GOLEM_FIREBALL)
                .count()
        };
        assert_eq!(
            fireballs(1.0),
            4,
            "a whole body leaves it on a 300-tick cycle"
        );
        assert_eq!(fireballs(0.05), 20, "a body nearly dead brings it to sixty");
    }

    /// GOL-M13: and it does not fire through a wall. Vanilla pins the charge at twenty for every
    /// tick it cannot see you (`NPC.cs:31669-31672`), so the shot lands when you come back into
    /// view rather than arriving through the floor.
    #[test]
    fn the_free_head_will_not_fire_through_a_wall() {
        let tiles = floor(30);
        // Head below the floor, player above it.
        let w = world(&tiles, Some((0.0, 20.0 * TILE)));
        let mut h = piece(GOLEM_HEAD_FREE, 0, 40);
        let body = Some(body_at((0.0, 40.0 * TILE)));
        let fireballs: usize = (0..1200)
            .flat_map(|_| free_head(&mut h, &w, body, whole()).shots)
            .filter(|s| s.projectile == GOLEM_FIREBALL)
            .count();
        assert_eq!(fireballs, 0, "the wall should hold its charge");
        assert_eq!(h.ai[1], 20.0, "pinned at twenty, ready to fire on sight");
    }

    /// A free head with no body left to read is over (`NPC.cs:31599-31603`).
    #[test]
    fn the_free_head_dies_with_the_body() {
        let tiles = floor(30);
        let w = world(&tiles, Some((300.0, 400.0)));
        let mut h = piece(GOLEM_HEAD_FREE, 0, 40);
        assert!(free_head(&mut h, &w, None, whole()).spent);
    }

    /// B8: the free head fires eye-lasers of its own, on top of its fireball.
    #[test]
    fn the_free_head_fires_eye_lasers() {
        let tiles = floor(30);
        let player = (0.0, 29.0 * TILE);
        let w = world(&tiles, Some(player));
        let mut h = piece(GOLEM_HEAD_FREE, 0, 40);
        let body = Some(body_at((0.0, 25.0 * TILE)));
        let mut lasers = 0;
        for _ in 0..3000 {
            let out = free_head(&mut h, &w, body, whole());
            lasers += out
                .shots
                .iter()
                .filter(|s| s.projectile == GOLEM_LASER)
                .count();
            h.position.0 += h.velocity.0;
            h.position.1 += h.velocity.1;
        }
        assert!(lasers > 0, "the free head should have fired lasers");
    }

    /// A fist only punches at somebody on its own side.
    #[test]
    fn a_fist_will_not_reach_across_the_body() {
        let tiles = floor(30);
        let punched = |player_x: f32| {
            let w = world(&tiles, Some((player_x, 400.0)));
            let mut f = piece(GOLEM_FIST_LEFT, 0, 25);
            let parent = body_at((100.0, 400.0));
            for _ in 0..600 {
                fist(&mut f, &w, Some(parent), whole());
                // It has to actually travel to its station before it starts winding up.
                f.position.0 += f.velocity.0;
                f.position.1 += f.velocity.1;
                if f.ai[0] != 0.0 {
                    return true;
                }
            }
            false
        };
        // The left fist sits to the left of the body; a player further left is on its side.
        assert!(punched(-2000.0), "it should punch to its own side");
        assert!(!punched(6000.0), "and not across the body");
    }

    /// GOL-1: a punch ends by distance or by hitting a wall, not on a fixed timer. The old code
    /// always returned after sixty ticks. Here a fist still within reach, with no wall struck, keeps
    /// punching however long it has been out; and it retracts once it is past its reach or collides.
    #[test]
    fn a_fist_punch_ends_by_distance_or_wall_not_a_timer() {
        let tiles = floor(30);
        let parent = body_at((1000.0, 400.0));
        // The player far to the left, so a left fist never counts as having passed it here.
        let w = world(&tiles, Some((-3000.0, 400.0)));

        // Close to the station, no wall struck: it keeps punching long past the old sixty-tick timer.
        let mut f = piece(GOLEM_FIST_LEFT, 0, 0);
        f.position = (900.0, 400.0);
        f.ai[0] = 2.0;
        f.ai[1] = 1000.0; // far past the old GOLEM_PUNCH_TICKS of sixty
        f.velocity = (-12.0, 0.0);
        f.no_tile_collide = true;
        fist(&mut f, &w, Some(parent), whole());
        assert_eq!(
            f.ai[0], 2.0,
            "a fist within reach keeps punching, not on a timer"
        );

        // Beyond its reach (well over 700 from the station near x=966): home it goes.
        let mut far = piece(GOLEM_FIST_LEFT, 0, 0);
        far.position = (-2000.0, 400.0);
        far.ai[0] = 2.0;
        far.ai[1] = 5.0;
        far.velocity = (-12.0, 0.0);
        fist(&mut far, &w, Some(parent), whole());
        assert_eq!(far.ai[0], 0.0, "past its reach it returns");

        // A wall struck (the engine's collision flag) sends it home too, even close in.
        let mut hit = piece(GOLEM_FIST_LEFT, 0, 0);
        hit.position = (900.0, 400.0);
        hit.ai[0] = 2.0;
        hit.ai[1] = 5.0;
        hit.velocity = (-12.0, 0.0);
        hit.collide_x = true;
        fist(&mut hit, &w, Some(parent), whole());
        assert_eq!(hit.ai[0], 0.0, "a wall stops the punch short");
        assert!(hit.no_tile_collide, "and it phases home");
    }

    /// GOL-1: the fist phases through terrain on the way out but turns solid once it is past the
    /// player, so a wall behind them can stop it. The old code left it phasing the whole punch.
    #[test]
    fn a_fist_turns_solid_once_it_passes_the_player() {
        let tiles = floor(30);
        let parent = body_at((1000.0, 400.0));
        let w = world(&tiles, Some((800.0, 400.0)));
        let mut f = piece(GOLEM_FIST_LEFT, 0, 0);
        // Left of the player and moving further left: it has passed them.
        f.position = (700.0, 400.0);
        f.ai[0] = 2.0;
        f.ai[1] = 5.0;
        f.velocity = (-12.0, 0.0);
        f.no_tile_collide = true;
        fist(&mut f, &w, Some(parent), whole());
        assert!(!f.no_tile_collide, "past the player it becomes solid");
    }

    /// The free head hovers above you rather than resting on the ground.
    #[test]
    fn the_free_head_hovers_overhead() {
        let tiles = floor(30);
        let player = (0.0, 29.0 * TILE);
        let w = world(&tiles, Some(player));
        let mut h = piece(GOLEM_HEAD_FREE, 0, 40);
        let body = Some(body_at((0.0, 25.0 * TILE)));
        for _ in 0..2000 {
            free_head(&mut h, &w, body, whole());
            h.position.0 += h.velocity.0;
            h.position.1 += h.velocity.1;
        }
        let above = player.1 - h.center().1;
        assert!(
            (GOLEM_FREE_ABOVE - above).abs() < 120.0,
            "it should be settling three hundred pixels up, got {above}"
        );
    }
}
