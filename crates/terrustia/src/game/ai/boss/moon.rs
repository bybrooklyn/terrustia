//! The moon events' flying bosses: styles 58–61.
//!
//! * **Pumpking** (58) picks a mood every five seconds and commits to it: throwing spheres,
//!   charging, or setting its blades scything. It hovers two hundred pixels above you between
//!   moods, and in its charging mood it closes *faster the further off you are*, so distance is no
//!   defence against that one.
//! * A **blade** (59) is one of two that orbit Pumpking. It dies with it.
//! * The **Ice Queen** (60) does not hover at all. She rolls one of three moods every few seconds:
//!   sweeping back and forth across you at eight hundred pixels, chasing gently while dropping
//!   shards, or stopping dead to spin and spray them. All three accelerate at every quarter of her
//!   health, and the picker forces the sweep whenever you are a long way off.
//! * **Santa-NK1** (61) drives along the ground, waits five seconds, and then fires - and the gap
//!   between shots shortens at every quarter, from every sixteen ticks down to every eight. Three
//!   more weapons run on their own random fuses alongside that: a present bomb, a rocket volley and
//!   a missile volley.
//!
//! Daylight ends all of them, and each leaves differently: Pumpking sinks, the Ice Queen
//! accelerates away upward, Santa walks off.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    ICE_QUEEN_CHASE_PACE, ICE_QUEEN_MIST_DAMAGE, ICE_QUEEN_MIST_INTERVAL, ICE_QUEEN_MIST_RANGE,
    ICE_QUEEN_MIST_SPEED, ICE_QUEEN_MODE_PICK_RANGE, ICE_QUEEN_MODE_STEP, ICE_QUEEN_MODE0_AT,
    ICE_QUEEN_MODE0_EXIT_RANGE, ICE_QUEEN_MODE1_AT, ICE_QUEEN_MODE2_AT, ICE_QUEEN_MODES,
    ICE_QUEEN_SHARD_DAMAGE, ICE_QUEEN_SHARD_INTERVAL, ICE_QUEEN_SPIN_ABOVE, ICE_QUEEN_SPIN_DAMAGE,
    ICE_QUEEN_SPIN_DRAG, ICE_QUEEN_SPIN_INTERVAL, ICE_QUEEN_SPIN_MUZZLE, ICE_QUEEN_SPIN_ROTATION,
    ICE_QUEEN_SPIN_SPEED, PUMPKING_ABOVE, PUMPKING_BLADE, PUMPKING_BLADES, PUMPKING_CHARGE,
    PUMPKING_CHARGE_SMOOTH, PUMPKING_CHARGE_TICKS, PUMPKING_HOVER, PUMPKING_HOVER_SMOOTH,
    PUMPKING_LEASH, PUMPKING_MOOD_TICKS, PUMPKING_MOODS, PUMPKING_RUSH_STEPS,
    PUMPKING_SPHERE_DAMAGE, PUMPKING_SPHERE_EVERY, PUMPKING_SPHERE_SPAN, PUMPKING_SPHERE_SPEED,
    QUEEN_ABOVE_MAX, QUEEN_ABOVE_MIN, QUEEN_CLIMB, QUEEN_CLIMB_CAP, QUEEN_PACE, QUEEN_SWEEP,
    SANTA_BULLET_DAMAGE, SANTA_BULLET_SPEED, SANTA_CLIMB, SANTA_CLIMB_CAP, SANTA_CLIMB_CREEP,
    SANTA_CREEP_BAND, SANTA_DROP_CLEARANCE, SANTA_FALL, SANTA_FALL_CAP, SANTA_FIRE_RATE,
    SANTA_FIRE_TICKS, SANTA_HARDPOINT, SANTA_LEASH, SANTA_MISSILE_DAMAGE, SANTA_MISSILE_EVERY,
    SANTA_MISSILE_LEAN, SANTA_MISSILE_ODDS, SANTA_MISSILE_RISE, SANTA_MISSILE_SPEED,
    SANTA_MISSILE_START, SANTA_MUZZLE, SANTA_ODDS_STEPS, SANTA_PLANT_RANGE, SANTA_PRESENT_DAMAGE,
    SANTA_PRESENT_ODDS, SANTA_PRESENT_SPEED, SANTA_PROBE, SANTA_ROCKET_DAMAGE, SANTA_ROCKET_EVERY,
    SANTA_ROCKET_ODDS, SANTA_ROCKET_SPEED, SANTA_ROCKET_SPREAD, SANTA_VOLLEY_TICKS, SANTA_WAIT,
    SANTA_WALK,
};
use terrustia_proto::projectile::ids::{
    ICE_QUEEN_MIST, ICE_QUEEN_SHARD, PUMPKING_SPHERE, SANTA_BULLET, SANTA_MISSILE, SANTA_PRESENT,
    SANTA_ROCKET,
};

use super::skeletron::Parent;
use crate::game::ai::{Shot, World, face};
use crate::game::npc::{Npc, TILE, TileView};
use crate::game::npc_ai::Spawn;

/// What one of them did this tick.
#[derive(Debug, Default)]
pub struct MoonOutcome {
    pub shots: Vec<Shot>,
    pub spawn: Vec<Spawn>,
    pub spent: bool,
}

/// The Ice Queen's "my mode is over" marker: vanilla parks `-1` in `ai[0]` and the picker at the
/// bottom of the routine reads it in the same tick (`NPC.cs:33961-33978`).
const PICK_A_MODE: f32 = -1.0;

/// Pick the value for the first threshold this health is below.
fn by_health<const N: usize, T: Copy>(health: f32, table: [(f32, T); N]) -> T {
    let mut chosen = table[0].1;
    for (threshold, value) in table {
        if health < threshold {
            chosen = value;
        }
    }
    chosen
}

/// Style 58: Pumpking.
pub fn pumpking(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    rng: &mut SmallRng,
) -> MoonOutcome {
    let mut out = MoonOutcome::default();
    npc.dirty = true;
    npc.no_gravity = true;
    npc.no_tile_collide = true;

    // Its two blades go out on the first tick, on opposite sides and half a cycle out of phase so
    // they scythe opposite arcs rather than sitting on top of one another (`NPC.cs:33377-33387`):
    // the first is raised with ai[0] = -1, the second with ai[0] = 1 and its phase ai[3] started at
    // 150 of the 300-tick sweep. Left unset both would read ai[0] = signum(0) = 1 and ai[3] = 0.
    if npc.ai[0] == 0.0 {
        npc.ai[0] = 1.0;
        for side in 0..PUMPKING_BLADES {
            // The first goes left at phase 0, the second right and already 150 of its 300 ticks
            // along, so the two are half a cycle apart. Vanilla leaves the first's ai[3] at its
            // default and only sets the second's, which this mirrors.
            let (arc, phase) = if side == 0 {
                (-1.0, None)
            } else {
                (1.0, Some(150.0))
            };
            out.spawn.push(Spawn {
                handle: None,
                npc_type: PUMPKING_BLADE,
                position: npc.center(),
                velocity: (0.0, 0.0),
                parent: Some(Spawn::OWN_PARENT),
                ai: [Some(arc), None, None, phase],
            });
        }
    }

    // The mood, which changes every five seconds whatever it is doing.
    npc.local_ai[2] += 1.0;
    if npc.local_ai[2] > PUMPKING_MOOD_TICKS {
        npc.local_ai[2] = 0.0;
        npc.ai[3] = rng.random_range(0..PUMPKING_MOODS) as f32;
    } else if npc.ai[3] == 0.0
        && npc.local_ai[2] > PUMPKING_SPHERE_EVERY
        && npc.local_ai[2] % PUMPKING_SPHERE_EVERY == 0.0
        && let Some(target) = world.target.filter(|t| t.alive)
    {
        // In its throwing mood it lobs spheres from beneath itself.
        let (cx, cy) = npc.center();
        let from = (cx, cy + 30.0);
        let mut across = target.center.0 - from.0 + rng.random_range(-50..=50) as f32;
        // Aimed low and flat: a fifth of the vertical gap, plus a downward bias.
        let mut rise = (target.center.1 - from.1 + rng.random_range(50..201) as f32) * 0.2;
        let length = across.hypot(rise).max(f32::MIN_POSITIVE);
        across = across / length * PUMPKING_SPHERE_SPEED;
        rise = rise / length * PUMPKING_SPHERE_SPEED;
        let jitter = |rng: &mut SmallRng| 1.0 + rng.random_range(-30..=30) as f32 * 0.01;
        out.shots.push(Shot {
            projectile: PUMPKING_SPHERE + rng.random_range(0..PUMPKING_SPHERE_SPAN),
            damage: PUMPKING_SPHERE_DAMAGE,
            position: from,
            velocity: (across * jitter(rng), rise * jitter(rng)),
            time_left: 600,
            ai: [0.0; 3],
        });
    }

    // Lost, or daylight: it sinks away, but the two leave differently.
    let in_leash = world.target.filter(|t| {
        t.alive
            && (npc.position.0 - t.center.0).abs() <= PUMPKING_LEASH
            && (npc.position.1 - t.center.1).abs() <= PUMPKING_LEASH
    });
    // PUMP-2: daytime overrides everything with a hard sink and a horizontal brake
    // (`velocity.Y += 0.3; velocity.X *= 0.9`, `NPC.cs:33403-33407`). Losing the player - dead, or
    // more than the leash away - is instead the gentle "leaving" drift (`ai[1] = 2`):
    // `velocity.Y += 0.1`, a further brake on any upward drift, and a shorter `EncourageDespawn(500)`
    // grace. The old code lumped the leash exit into the daytime sink, so a leashed-out Pumpking
    // dropped three times as fast and lingered a hundred ticks longer than vanilla.
    if world.conditions.day {
        npc.velocity.1 += 0.3;
        npc.velocity.0 *= 0.9;
        npc.rotation = npc.velocity.0 * -0.02;
        npc.time_left = npc.time_left.min(600);
        return out;
    }
    if in_leash.is_none() {
        npc.velocity.1 += 0.1;
        if npc.velocity.1 < 0.0 {
            npc.velocity.1 *= 0.95;
        }
        // And the horizontal brake, `velocity.X *= 0.95f` (`NPC.cs:33479`), which was missed. It is
        // gentler than the daytime sink's 0.9 above, but it is there: without it a Pumpking that
        // lost you kept the whole of its last hover speed sideways for ever and sailed off across
        // the arena instead of settling and sinking.
        npc.velocity.0 *= 0.95;
        npc.rotation = npc.velocity.0 * -0.02;
        npc.time_left = npc.time_left.min(500);
        return out;
    }
    let target = in_leash.expect("checked just above");
    let (cx, cy) = npc.center();

    if npc.ai[1] == 0.0 {
        // Hovering above the player, and only its charging mood breaks that.
        npc.ai[2] += 1.0;
        if npc.ai[2] >= PUMPKING_MOOD_TICKS {
            npc.ai[2] = 0.0;
            if npc.ai[3] == 1.0 {
                npc.ai[1] = 1.0;
            }
        }
        let gap = (target.center.0 - cx, target.center.1 - PUMPKING_ABOVE - cy);
        let reach = gap.0.hypot(gap.1);
        // In its charging mood it closes faster the further off you are.
        let mut speed = PUMPKING_HOVER;
        if npc.ai[3] == 1.0 {
            for (at, faster) in PUMPKING_RUSH_STEPS {
                if reach > at {
                    speed = faster;
                    break;
                }
            }
        }
        if reach > 50.0 {
            let scale = speed / reach;
            npc.velocity.0 = (npc.velocity.0 * PUMPKING_HOVER_SMOOTH + gap.0 * scale)
                / (PUMPKING_HOVER_SMOOTH + 1.0);
            npc.velocity.1 = (npc.velocity.1 * PUMPKING_HOVER_SMOOTH + gap.1 * scale)
                / (PUMPKING_HOVER_SMOOTH + 1.0);
        }
    } else {
        // Charging: straight at you, very heavily smoothed, so it arcs rather than turns.
        npc.ai[2] += 1.0;
        if npc.ai[2] >= PUMPKING_CHARGE_TICKS || npc.ai[3] != 1.0 {
            npc.ai[1] = 0.0;
            npc.ai[2] = 0.0;
        }
        let gap = (target.center.0 - cx, target.center.1 - cy);
        let reach = gap.0.hypot(gap.1).max(f32::MIN_POSITIVE);
        let scale = PUMPKING_CHARGE / reach;
        npc.velocity.0 = (npc.velocity.0 * PUMPKING_CHARGE_SMOOTH + gap.0 * scale)
            / (PUMPKING_CHARGE_SMOOTH + 1.0);
        npc.velocity.1 = (npc.velocity.1 * PUMPKING_CHARGE_SMOOTH + gap.1 * scale)
            / (PUMPKING_CHARGE_SMOOTH + 1.0);
    }
    npc.rotation = npc.velocity.0 * -0.02;
    out
}

/// Style 59: one of Pumpking's blades.
///
/// It orbits its owner and dies with it. `ai[0]` is which side it is on and `ai[3]` its phase, so
/// the two sweep opposite arcs rather than sitting on top of one another.
pub fn pumpking_blade(npc: &mut Npc, owner: Option<Parent>) -> MoonOutcome {
    let mut out = MoonOutcome::default();
    npc.dirty = true;

    let Some(owner) = owner else {
        // No Pumpking, no blade.
        npc.velocity.0 *= 0.9;
        npc.velocity.1 *= 0.9;
        out.spent = true;
        return out;
    };
    npc.sprite_direction = -(npc.ai[0] as i8);

    // The arc: it swings out and back on a three-hundred-tick cycle.
    npc.ai[3] += 1.0;
    if npc.ai[3] >= 300.0 {
        npc.ai[3] = 0.0;
    }
    let along = npc.ai[3] / 300.0 * std::f32::consts::TAU;
    let radius = 150.0 + along.sin() * 100.0;
    let angle = along * npc.ai[0].signum();
    let (ox, oy) = owner.center();
    let station = (ox + angle.cos() * radius, oy + angle.sin() * radius);

    let (cx, cy) = npc.center();
    npc.velocity = ((station.0 - cx) * 0.2, (station.1 - cy) * 0.2);
    npc.rotation = npc.velocity.1.atan2(npc.velocity.0);
    out
}

/// Style 60: the Ice Queen.
///
/// Three modes, and the one it runs next is a coin toss between all three rather than the other
/// one (`NPC.cs:33961-33978`). Mode 0 sweeps back and forth, firing a mist forward while above
/// you. Mode 1 gives up the sweep for a gentler pursuit and drops ice shards straight down. Mode 2
/// stops dead, spins, and sprays shards on random bearings. Whichever it is in, its counter
/// advances by one to three a tick, so a mode lasts about half as long as its threshold reads.
///
/// The picker forces the sweep whenever you are more than a thousand pixels off, so it never
/// stands still spraying at empty air.
pub fn ice_queen(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    rng: &mut SmallRng,
) -> MoonOutcome {
    let mut out = MoonOutcome::default();
    npc.dirty = true;
    npc.no_gravity = true;
    npc.no_tile_collide = true;

    if world.conditions.day {
        // It accelerates away rather than slowing.
        npc.velocity.0 += 0.25 * if npc.velocity.0 > 0.0 { 1.0 } else { -1.0 };
        npc.velocity.1 -= 0.1;
        npc.rotation = npc.velocity.0 * 0.05;
        npc.time_left = npc.time_left.min(600);
        return out;
    }
    let Some(target) = world.target.filter(|t| t.alive) else {
        return out;
    };
    let (cx, cy) = npc.center();
    let health = npc.life as f32 / npc.life_max.max(1) as f32;
    // ICEQ-2: `ai[1] += Main.rand.Next(1, 4)` at all three exits (`NPC.cs:33801`, `:33913`,
    // `:33955`), mean two. Advancing by a flat one made every mode run twice as long as the game's.
    let step = |rng: &mut SmallRng| rng.random_range(ICE_QUEEN_MODE_STEP) as f32;

    if npc.ai[0] == 1.0 {
        // Mode 1: gentler pursuit, and ice shards dropped straight down.
        let (accel, cap) = {
            let mut chosen = (ICE_QUEEN_CHASE_PACE[0].1, ICE_QUEEN_CHASE_PACE[0].2);
            for (threshold, a, c) in ICE_QUEEN_CHASE_PACE {
                if health < threshold {
                    chosen = (a, c);
                }
            }
            chosen
        };
        if cx < target.center.0 {
            npc.velocity.0 += accel;
            if npc.velocity.0 < 0.0 {
                npc.velocity.0 *= 0.98;
            }
        }
        if cx > target.center.0 {
            npc.velocity.0 -= accel;
            if npc.velocity.0 > 0.0 {
                npc.velocity.0 *= 0.98;
            }
        }
        if npc.velocity.0 > cap || npc.velocity.0 < -cap {
            npc.velocity.0 *= 0.95;
        }
        let below = target.center.1 - (npc.position.1 + npc.height());
        if below < 180.0 {
            npc.velocity.1 -= 0.1;
        }
        if below > 200.0 {
            npc.velocity.1 += 0.1;
        }
        npc.velocity.1 = npc.velocity.1.clamp(-6.0, 6.0);
        npc.rotation = npc.velocity.0 * 0.01;

        npc.ai[3] += 1.0;
        let interval = by_health(health, ICE_QUEEN_SHARD_INTERVAL);
        if npc.ai[3] >= interval {
            npc.ai[3] = 0.0;
            let drop = (cx, npc.position.1 + npc.height() - 14.0);
            let tx = (drop.0 / TILE) as i32;
            let ty = (drop.1 / TILE) as i32;
            let tile = world.tiles.tile(tx, ty);
            let blocked = tile.is_active() && terrustia_proto::tile_solid::solid(tile.block);
            if !blocked {
                out.shots.push(Shot {
                    projectile: ICE_QUEEN_SHARD,
                    damage: ICE_QUEEN_SHARD_DAMAGE,
                    position: drop,
                    velocity: (npc.velocity.0 * 0.25, npc.velocity.1.max(0.0) + 3.0),
                    time_left: 600,
                    ai: [0.0; 3],
                });
            }
        }

        npc.ai[1] += step(rng);
        if npc.ai[1] > ICE_QUEEN_MODE1_AT {
            npc.ai[0] = PICK_A_MODE;
        }
    } else if npc.ai[0] == 2.0 {
        // ICEQ-1, mode 2: it brakes to a stop, spins, and sprays shards on random bearings
        // (`NPC.cs:33903-33959`). Nothing here aims at you; the fifteen-pixel bearing is picked
        // fresh every tick and the shot leaves four bearings out from twenty pixels above its
        // centre, so the spray fans out around it rather than starting inside its own body.
        npc.velocity.0 *= ICE_QUEEN_SPIN_DRAG;
        npc.velocity.1 *= ICE_QUEEN_SPIN_DRAG;
        npc.rotation += ICE_QUEEN_SPIN_ROTATION;

        let mut bearing = (
            rng.random_range(-1000..=1000) as f32,
            rng.random_range(-1000..=1000) as f32,
        );
        let length = bearing.0.hypot(bearing.1).max(f32::MIN_POSITIVE);
        bearing = (
            bearing.0 / length * ICE_QUEEN_SPIN_SPEED,
            bearing.1 / length * ICE_QUEEN_SPIN_SPEED,
        );

        npc.ai[3] += 1.0;
        if npc.ai[3] > by_health(health, ICE_QUEEN_SPIN_INTERVAL) {
            npc.ai[3] = 0.0;
            out.shots.push(Shot {
                projectile: ICE_QUEEN_SHARD,
                damage: ICE_QUEEN_SPIN_DAMAGE,
                position: (
                    cx + bearing.0 * ICE_QUEEN_SPIN_MUZZLE,
                    cy - ICE_QUEEN_SPIN_ABOVE + bearing.1 * ICE_QUEEN_SPIN_MUZZLE,
                ),
                velocity: bearing,
                time_left: 600,
                ai: [0.0; 3],
            });
        }

        npc.ai[1] += step(rng);
        if npc.ai[1] > ICE_QUEEN_MODE2_AT {
            npc.ai[0] = PICK_A_MODE;
        }
    } else if npc.ai[0] == 0.0 {
        // Mode 0: the sweep. `ai[2]` is which way it is going, and it only turns once it is well
        // past you.
        if npc.ai[2] == 0.0 {
            npc.ai[2] = if cx < target.center.0 { 1.0 } else { -1.0 };
        }
        let across = (cx - target.center.0).abs();
        if across > QUEEN_SWEEP
            && ((cx < target.center.0 && npc.ai[2] < 0.0)
                || (cx > target.center.0 && npc.ai[2] > 0.0))
        {
            npc.ai[2] = 0.0;
        }

        let (accel, cap) = {
            let mut chosen = (QUEEN_PACE[0].1, QUEEN_PACE[0].2);
            for (threshold, a, c) in QUEEN_PACE {
                if health < threshold {
                    chosen = (a, c);
                }
            }
            chosen
        };
        npc.velocity.0 = (npc.velocity.0 + npc.ai[2] * accel).clamp(-cap, cap);

        // It holds a band above you rather than a height.
        let below = target.center.1 - (npc.position.1 + npc.height());
        if below < QUEEN_ABOVE_MIN {
            npc.velocity.1 -= QUEEN_CLIMB;
        }
        if below > QUEEN_ABOVE_MAX {
            npc.velocity.1 += QUEEN_CLIMB;
        }
        npc.velocity.1 = npc.velocity.1.clamp(-QUEEN_CLIMB_CAP, QUEEN_CLIMB_CAP);
        npc.rotation = npc.velocity.0 * 0.05;

        // The forward mist: only while above you, and either close or already mid-volley.
        let above_player = npc.position.1 < target.center.1;
        if (across < ICE_QUEEN_MIST_RANGE || npc.ai[3] < 0.0) && above_player {
            npc.ai[3] += 1.0;
            let interval = by_health(health, ICE_QUEEN_MIST_INTERVAL);
            if npc.ai[3] > interval {
                npc.ai[3] = -interval;
            }
            if npc.ai[3] == 0.0 {
                let speed = by_health(health, ICE_QUEEN_MIST_SPEED);
                let from = (cx + npc.velocity.0 * 7.0, cy);
                let aim = (target.center.0 - from.0, target.center.1 - from.1);
                let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
                out.shots.push(Shot {
                    projectile: ICE_QUEEN_MIST,
                    damage: ICE_QUEEN_MIST_DAMAGE,
                    position: from,
                    velocity: (aim.0 / length * speed, aim.1 / length * speed),
                    time_left: 600,
                    ai: [0.0; 3],
                });
            }
        } else if npc.ai[3] < 0.0 {
            npc.ai[3] += 1.0;
        }

        npc.ai[1] += step(rng);
        // ICEQ-3: the sweep only ends while you are inside six hundred pixels (`NPC.cs:33803`).
        // Without that it handed off from wherever it happened to be.
        if npc.ai[1] > ICE_QUEEN_MODE0_AT && across < ICE_QUEEN_MODE0_EXIT_RANGE {
            npc.ai[0] = PICK_A_MODE;
        }
    }

    // The picker, run in the same tick a mode ends (`NPC.cs:33961-33978`): one of the three at
    // random, forced back to the sweep when you are a long way off.
    if npc.ai[0] == PICK_A_MODE {
        let mut mode = rng.random_range(0..ICE_QUEEN_MODES) as f32;
        if (cx - target.center.0).abs() > ICE_QUEEN_MODE_PICK_RANGE {
            mode = 0.0;
        }
        npc.ai = [mode, 0.0, 0.0, 0.0];
    }
    out
}

/// Style 61: Santa-NK1.
///
/// A ground vehicle, not a flier: it drives along whatever is under it, climbing solid ground and
/// falling off ledges, and drops hard on anyone standing directly beneath it. Four weapons run at
/// once. The machine gun belongs to the firing state; the present bomb, the rocket volley and the
/// missile volley each have their own random fuse and go off whatever it is doing.
pub fn santa(npc: &mut Npc, world: &World<'_, impl TileView>, rng: &mut SmallRng) -> MoonOutcome {
    let mut out = MoonOutcome::default();
    npc.dirty = true;
    npc.no_gravity = true;
    npc.no_tile_collide = true;

    let health = npc.life as f32 / npc.life_max.max(1) as f32;
    let mut walk = by_health(health, SANTA_WALK);
    let mut planted = false;
    let (cx, cy) = npc.center();

    // SNK-2: vanilla's `flag65` is `Distance(player.Center) <= 2000f` (`NPC.cs:33992`), a real 2D
    // distance. Measured across only, a player two thousand pixels straight up was "in reach".
    let in_reach = world
        .target
        .filter(|t| t.alive && (t.center.0 - cx).hypot(t.center.1 - cy) <= SANTA_LEASH);
    if world.conditions.day {
        // C7-05: vanilla forces the despawn at dawn (`NPC.cs:34011-34014`, `EncourageDespawn(10)`,
        // walk 8), so Santa-NK1 is gone within ten ticks of leaving the area, not six hundred.
        walk = 8.0;
        npc.time_left = npc
            .time_left
            .min(terrustia_proto::npc_params::DESPAWN_ENCOURAGED_TICKS);
        if npc.velocity.0 == 0.0 {
            npc.velocity.0 = 0.1;
        }
    } else {
        // SNK-2: the leash gates the gun, not the machine. Wrapped round the whole night branch it
        // also froze the wait/fire clock, so a Santa that lost you never came back to firing.
        if let Some(target) = world.target.filter(|t| t.alive) {
            face(npc, target);
        }
        if npc.ai[0] == 0.0 {
            npc.ai[1] += 1.0;
            if npc.ai[1] >= SANTA_WAIT {
                npc.ai[1] = 0.0;
                npc.ai[0] = 1.0;
            }
        } else {
            // Firing. It stands still and the gun speeds up as it is worn down.
            planted = true;
            npc.ai[1] += 1.0;
            let every = by_health(health, SANTA_FIRE_RATE);
            if npc.ai[1] % every == 0.0
                && let Some(target) = in_reach
            {
                let muzzle = (
                    cx + f32::from(npc.direction) * SANTA_MUZZLE,
                    cy + rng.random_range(15..36) as f32,
                );
                let mut across = target.center.0 - muzzle.0 + rng.random_range(-40..=40) as f32;
                let mut rise = target.center.1 - muzzle.1 + rng.random_range(-40..=40) as f32;
                let length = across.hypot(rise).max(f32::MIN_POSITIVE);
                across = across / length * SANTA_BULLET_SPEED;
                rise = rise / length * SANTA_BULLET_SPEED;
                let jitter = |rng: &mut SmallRng| 1.0 + rng.random_range(-20..=20) as f32 * 0.015;
                out.shots.push(Shot {
                    projectile: SANTA_BULLET,
                    damage: SANTA_BULLET_DAMAGE,
                    position: muzzle,
                    velocity: (across * jitter(rng), rise * jitter(rng)),
                    time_left: 300,
                    ai: [0.0; 3],
                });
            }
            // C7-03: the burst runs 240 ticks, not 300 (`NPC.cs:34067`, `ai[1] > 240`). The old
            // 300 was `SANTA_WAIT` reused; the two durations are unrelated in vanilla.
            if npc.ai[1] > SANTA_FIRE_TICKS {
                npc.ai[1] = 0.0;
                npc.ai[0] = 0.0;
            }
        }
    }

    santa_hardpoints(npc, in_reach.map(|t| t.center), rng, health, &mut out);

    // Close enough and it plants rather than driving past you (`NPC.cs:34165-34168`).
    if world
        .target
        .is_some_and(|t| (cx - t.center.0).abs() < SANTA_PLANT_RANGE)
    {
        planted = true;
    }

    if planted {
        npc.velocity.0 *= 0.9;
        if npc.velocity.0.abs() < 0.1 {
            npc.velocity.0 = 0.0;
        }
    } else {
        let wanted = walk * f32::from(npc.direction);
        npc.velocity.0 = (npc.velocity.0 * 20.0 + wanted) / 21.0;
    }

    santa_tracks(npc, world);
    out
}

/// SNK-1: the three fused weapons (`NPC.cs:34073-34163`).
///
/// All three fire from a hardpoint behind and above the cab. None of them is gated on the firing
/// state, so a driving Santa-NK1 is still throwing presents; only the rockets check the leash, which
/// is why the missiles keep coming when you break away. `aimed` is the target only while it is
/// within the leash.
fn santa_hardpoints(
    npc: &mut Npc,
    aimed: Option<(f32, f32)>,
    rng: &mut SmallRng,
    health: f32,
    out: &mut MoonOutcome,
) {
    let (cx, cy) = npc.center();
    let facing = f32::from(npc.direction);
    let hardpoint = (cx - facing * SANTA_HARDPOINT.0, cy - SANTA_HARDPOINT.1);
    // The odds shorten at every quarter, all three by the same factor.
    let odds = |base: u32| ((base as f32 * by_health(health, SANTA_ODDS_STEPS)) as u32).max(1);
    let jitter = |rng: &mut SmallRng, k: f32| 1.0 + rng.random_range(-20..=20) as f32 * k;

    // The present bomb: a nearly flat toss at one pixel a tick, so it lands short of you and waits.
    if rng.random_range(0..odds(SANTA_PRESENT_ODDS)) == 0 {
        let mut lob = (rng.random_range(1..100) as f32 * facing, 1.0);
        let length = lob.0.hypot(lob.1).max(f32::MIN_POSITIVE);
        lob = (
            lob.0 / length * SANTA_PRESENT_SPEED,
            lob.1 / length * SANTA_PRESENT_SPEED,
        );
        out.shots.push(Shot {
            projectile: SANTA_PRESENT,
            damage: SANTA_PRESENT_DAMAGE,
            position: hardpoint,
            velocity: lob,
            time_left: 600,
            ai: [0.0; 3],
        });
    }

    // The rocket volley, which only starts on its own fuse and then runs a hundred ticks.
    if rng.random_range(0..odds(SANTA_ROCKET_ODDS)) == 0 {
        npc.local_ai[1] = 1.0;
    }
    if npc.local_ai[1] >= 1.0 {
        npc.local_ai[1] += 1.0;
        if npc.local_ai[1] % SANTA_ROCKET_EVERY == 0.0
            && let Some(at) = aimed
        {
            let mut aim = (
                at.0 - hardpoint.0
                    + rng.random_range(-SANTA_ROCKET_SPREAD..=SANTA_ROCKET_SPREAD) as f32,
                at.1 - hardpoint.1
                    + rng.random_range(-SANTA_ROCKET_SPREAD..=SANTA_ROCKET_SPREAD) as f32,
            );
            let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
            aim = (
                aim.0 / length * SANTA_ROCKET_SPEED,
                aim.1 / length * SANTA_ROCKET_SPEED,
            );
            out.shots.push(Shot {
                projectile: SANTA_ROCKET,
                damage: SANTA_ROCKET_DAMAGE,
                position: hardpoint,
                velocity: (aim.0 * jitter(rng, 0.015), aim.1 * jitter(rng, 0.015)),
                time_left: 600,
                ai: [0.0; 3],
            });
        }
        if npc.local_ai[1] >= SANTA_VOLLEY_TICKS {
            npc.local_ai[1] = 0.0;
        }
    }

    // The missile volley, fired near-vertically and at nobody in particular: it comes down on the
    // arena rather than on you, and unlike the rockets it does not check the leash.
    if rng.random_range(0..odds(SANTA_MISSILE_ODDS)) == 0 {
        npc.local_ai[2] = SANTA_MISSILE_START;
    }
    if npc.local_ai[2] > 0.0 {
        npc.local_ai[2] += 1.0;
        if npc.local_ai[2] % SANTA_MISSILE_EVERY == 0.0 {
            let mut up = (
                rng.random_range(-SANTA_MISSILE_LEAN..=SANTA_MISSILE_LEAN) as f32,
                SANTA_MISSILE_RISE,
            );
            let length = up.0.hypot(up.1).max(f32::MIN_POSITIVE);
            up = (
                up.0 / length * SANTA_MISSILE_SPEED,
                up.1 / length * SANTA_MISSILE_SPEED,
            );
            out.shots.push(Shot {
                projectile: SANTA_MISSILE,
                damage: SANTA_MISSILE_DAMAGE,
                position: hardpoint,
                velocity: (up.0 * jitter(rng, 0.01), up.1 * jitter(rng, 0.01)),
                time_left: 600,
                ai: [0.0; 3],
            });
        }
        if npc.local_ai[2] >= SANTA_VOLLEY_TICKS {
            npc.local_ai[2] = 0.0;
        }
    }
}

/// SNK-3: the vertical routine (`NPC.cs:34188-34237`), which was absent entirely.
///
/// Nothing here ever wrote `velocity.1`, so a tracked vehicle with `no_gravity` and
/// `no_tile_collide` set simply hung at its spawn altitude and drove through the mountain. It probes
/// an 80x20 box under its treads: solid means climb (gently at first, then hard), empty means fall,
/// and a player entirely inside its own width and below its treads makes it drop on them.
fn santa_tracks(npc: &mut Npc, world: &World<'_, impl TileView>) {
    let (probe_w, probe_h) = SANTA_PROBE;
    let under = (
        npc.center().0 - probe_w as f32 / 2.0,
        npc.position.1 + npc.height() - probe_h as f32,
    );
    // Vanilla measures against the player's own box; the target here carries a centre, so the box
    // is rebuilt from the shared player size.
    let (player_w, player_h) = (
        crate::game::ai::PLAYER_WIDTH as f32,
        crate::game::ai::PLAYER_HEIGHT as f32,
    );
    let drop_on = world.target.is_some_and(|t| {
        let left = t.center.0 - player_w / 2.0;
        let bottom = t.center.1 + player_h / 2.0;
        npc.position.0 < left
            && npc.position.0 + npc.width() > left + player_w
            && npc.position.1 + npc.height() < bottom - SANTA_DROP_CLEARANCE
    });

    if drop_on {
        npc.velocity.1 += SANTA_FALL;
    } else if crate::game::ai::sight::solid_collision(world.tiles, under, (probe_w, probe_h)) {
        if npc.velocity.1 > 0.0 {
            npc.velocity.1 = 0.0;
        }
        // Two thresholds, and they are not symmetric: it creeps up while slower than 0.2 and falls
        // gently while slower than 0.1 (`NPC.cs:34206`, `:34225`).
        npc.velocity.1 -= if npc.velocity.1 > -SANTA_CLIMB {
            SANTA_CLIMB_CREEP
        } else {
            SANTA_CLIMB
        };
        npc.velocity.1 = npc.velocity.1.max(SANTA_CLIMB_CAP);
    } else {
        if npc.velocity.1 < 0.0 {
            npc.velocity.1 = 0.0;
        }
        npc.velocity.1 += if npc.velocity.1 < SANTA_CREEP_BAND {
            SANTA_CLIMB_CREEP
        } else {
            SANTA_FALL
        };
    }
    npc.velocity.1 = npc.velocity.1.min(SANTA_FALL_CAP);
    let _ = TILE;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ai::Conditions;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::PUMPKING;
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

    fn boss(npc_type: u16, x: f32, y: f32) -> Npc {
        Npc::new(npc_type, (x, y), 1).expect("a moon boss")
    }

    /// Solid rock from tile row `at` downward, wide enough for anything here to drive on.
    fn ground(at: i32) -> Sky {
        let mut tiles = HashMap::new();
        for x in -400..400 {
            for y in at..at + 8 {
                tiles.insert((x, y), Tile::block(1));
            }
        }
        Sky(tiles)
    }

    /// Pumpking sends out its two blades before anything else.
    #[test]
    fn pumpking_arrives_with_its_blades() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(58);
        let mut p = boss(PUMPKING, 0.0, 0.0);
        let w = night(&tiles, Some((600.0, 400.0)));
        let out = pumpking(&mut p, &w, &mut rng);
        assert_eq!(out.spawn.len(), PUMPKING_BLADES);
        assert!(out.spawn.iter().all(|s| s.npc_type == PUMPKING_BLADE));
        assert!(pumpking(&mut p, &w, &mut rng).spawn.is_empty(), "only once");
    }

    /// PUMP-1: the two blades are raised on opposite sides and half a cycle out of phase, so they
    /// scythe opposite arcs rather than orbiting as one (`NPC.cs:33377-33387`): the first with
    /// ai[0] = -1, the second ai[0] = 1 and its phase ai[3] = 150. Left to the consumer's signum
    /// both would read ai[0] = signum(0) = 1 and ai[3] = 0, and the pair would overlap exactly.
    #[test]
    fn its_blades_seat_on_opposite_sides_out_of_phase() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(58);
        let mut p = boss(PUMPKING, 0.0, 0.0);
        let w = night(&tiles, Some((600.0, 400.0)));
        let out = pumpking(&mut p, &w, &mut rng);
        let blades: Vec<&Spawn> = out
            .spawn
            .iter()
            .filter(|s| s.npc_type == PUMPKING_BLADE)
            .collect();
        assert_eq!(blades.len(), 2);
        assert_eq!(blades[0].ai[0], Some(-1.0), "the first blade goes left");
        assert_eq!(blades[0].ai[3], None, "and starts at phase 0 (its default)");
        assert_eq!(blades[1].ai[0], Some(1.0), "the second goes right");
        assert_eq!(blades[1].ai[3], Some(150.0), "half a 300-tick cycle ahead");
    }

    /// Its moods come round, and the throwing one actually throws.
    #[test]
    fn pumpking_changes_its_mood_and_throws() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(2);
        let mut p = boss(PUMPKING, 0.0, 0.0);
        let w = night(&tiles, Some((600.0, 400.0)));

        let mut moods = std::collections::HashSet::new();
        let mut thrown = 0;
        for _ in 0..4000 {
            let out = pumpking(&mut p, &w, &mut rng);
            thrown += out.shots.len();
            moods.insert(p.ai[3] as i32);
        }
        assert!(moods.len() >= 2, "its mood should change: {moods:?}");
        assert!(thrown > 0, "and it should throw something");
    }

    /// A blade without its Pumpking does not survive.
    #[test]
    fn a_blade_dies_with_its_owner() {
        let mut b = boss(PUMPKING_BLADE, 0.0, 0.0);
        assert!(pumpking_blade(&mut b, None).spent);
    }

    /// The Ice Queen sweeps past you and turns, rather than hovering.
    ///
    /// `ai[1]` is held down so she stays in mode 0 for the whole run: what is under test here is
    /// the sweep itself, not how long she keeps it up.
    #[test]
    fn the_ice_queen_sweeps_back_and_forth() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(60);
        let mut q = boss(terrustia_proto::npc_params::ICE_QUEEN, 0.0, 0.0);
        let w = night(&tiles, Some((0.0, 400.0)));

        let mut crossings = 0;
        let mut side = -1.0f32;
        for _ in 0..4000 {
            q.ai[1] = 0.0;
            ice_queen(&mut q, &w, &mut rng);
            q.position.0 += q.velocity.0;
            q.position.1 += q.velocity.1;
            let now = q.center().0.signum();
            if now != side && now != 0.0 {
                crossings += 1;
                side = now;
            }
        }
        assert!(
            crossings >= 2,
            "it should keep sweeping across: {crossings}"
        );
    }

    /// B12: the Ice Queen fires a mist forward while above you, in its first mode.
    #[test]
    fn the_ice_queen_fires_a_mist_while_above_you() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(60);
        let mut q = boss(terrustia_proto::npc_params::ICE_QUEEN, 0.0, 0.0);
        let w = night(&tiles, Some((0.0, 400.0)));

        let mut shots = Vec::new();
        for _ in 0..800 {
            q.ai[1] = 0.0; // held in mode 0
            let out = ice_queen(&mut q, &w, &mut rng);
            shots.extend(out.shots);
            q.position.0 += q.velocity.0;
            q.position.1 += q.velocity.1;
        }
        assert!(!shots.is_empty(), "it should have fired the mist");
        assert!(
            shots
                .iter()
                .all(|s| s.projectile == ICE_QUEEN_MIST && s.damage == ICE_QUEEN_MIST_DAMAGE)
        );
    }

    /// ICEQ-1: a mode that runs out goes back to the picker, which rolls all three
    /// (`NPC.cs:33961-33978`), rather than ping-ponging between two.
    #[test]
    fn the_ice_queen_rolls_a_fresh_mode_when_one_ends() {
        let tiles = Sky(HashMap::new());
        let w = night(&tiles, Some((0.0, 400.0)));
        let mut picked = std::collections::HashSet::new();
        for seed in 0..40u64 {
            let mut rng = SmallRng::seed_from_u64(seed);
            let mut q = boss(terrustia_proto::npc_params::ICE_QUEEN, 0.0, 0.0);
            // On the brink of mode 0's threshold, and standing right next to the player so the
            // exit's own range gate is satisfied.
            q.ai[1] = ICE_QUEEN_MODE0_AT;
            ice_queen(&mut q, &w, &mut rng);
            picked.insert(q.ai[0] as i32);
        }
        assert_eq!(
            picked,
            std::collections::HashSet::from([0, 1, 2]),
            "all three modes should be reachable, got {picked:?}"
        );
    }

    /// ICEQ-1: ...but not the standing-still ones when you are a long way off
    /// (`NPC.cs:33964-33967`), which is what stops it spraying at empty air.
    #[test]
    fn a_distant_player_forces_the_sweep() {
        let tiles = Sky(HashMap::new());
        let w = night(&tiles, Some((5000.0, 400.0)));
        for seed in 0..40u64 {
            let mut rng = SmallRng::seed_from_u64(seed);
            let mut q = boss(terrustia_proto::npc_params::ICE_QUEEN, 0.0, 0.0);
            // Ending mode 2, whose exit has no range gate of its own.
            q.ai[0] = 2.0;
            q.ai[1] = ICE_QUEEN_MODE2_AT;
            ice_queen(&mut q, &w, &mut rng);
            assert_eq!(q.ai[0], 0.0, "seed {seed} should have been forced to sweep");
        }
    }

    /// ICEQ-3: mode 0 only hands off while you are inside six hundred pixels (`NPC.cs:33803`).
    #[test]
    fn the_sweep_holds_while_you_are_out_of_range() {
        let tiles = Sky(HashMap::new());
        let w = night(&tiles, Some((5000.0, 400.0)));
        let mut rng = SmallRng::seed_from_u64(3);
        let mut q = boss(terrustia_proto::npc_params::ICE_QUEEN, 0.0, 0.0);
        q.ai[1] = ICE_QUEEN_MODE0_AT * 4.0;
        ice_queen(&mut q, &w, &mut rng);
        assert_eq!(q.ai[0], 0.0, "far off, the sweep keeps going");
        // ...and it is the *same* sweep, not a fresh one the picker handed back: the counter is
        // still climbing rather than reset. Without the range gate the mode would end here and the
        // picker would return mode 0 anyway, which is why `ai[0]` alone proves nothing.
        assert!(
            q.ai[1] > ICE_QUEEN_MODE0_AT,
            "the mode never ended, so its counter kept going: {}",
            q.ai[1]
        );
    }

    /// ICEQ-2: every mode's counter advances by one to three a tick (`NPC.cs:33801`), so a mode
    /// lasts about half as long as its threshold reads. A flat one ran them all twice as long.
    #[test]
    fn its_mode_counter_advances_by_one_to_three() {
        let tiles = Sky(HashMap::new());
        let w = night(&tiles, Some((0.0, 400.0)));
        let mut seen = std::collections::HashSet::new();
        for seed in 0..60u64 {
            let mut rng = SmallRng::seed_from_u64(seed);
            let mut q = boss(terrustia_proto::npc_params::ICE_QUEEN, 0.0, 0.0);
            ice_queen(&mut q, &w, &mut rng);
            seen.insert(q.ai[1] as i32);
        }
        assert_eq!(
            seen,
            std::collections::HashSet::from([1, 2, 3]),
            "expected a 1..3 step, got {seen:?}"
        );
    }

    /// B12: the second mode drops falling ice shards instead of firing the forward mist.
    #[test]
    fn the_ice_queen_drops_shards_in_its_second_mode() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(60);
        let mut q = boss(terrustia_proto::npc_params::ICE_QUEEN, 0.0, 0.0);
        q.ai[0] = 1.0;
        let w = night(&tiles, Some((0.0, 400.0)));

        let mut shots = Vec::new();
        for _ in 0..400 {
            q.ai[1] = 0.0; // held in mode 1
            let out = ice_queen(&mut q, &w, &mut rng);
            shots.extend(out.shots);
            q.position.0 += q.velocity.0;
            q.position.1 += q.velocity.1;
        }
        assert!(!shots.is_empty(), "it should drop shards");
        assert!(
            shots
                .iter()
                .all(|s| s.projectile == ICE_QUEEN_SHARD && s.damage == ICE_QUEEN_SHARD_DAMAGE)
        );
        assert!(
            shots.iter().all(|s| s.velocity.1 > 0.0),
            "falling, not rising: {:?}",
            shots.iter().map(|s| s.velocity).collect::<Vec<_>>()
        );
    }

    /// ICEQ-1: the third mode, which was absent entirely. She brakes to a stop, spins, and sprays
    /// shards on random bearings at fifteen pixels a tick (`NPC.cs:33903-33959`).
    #[test]
    fn the_ice_queen_spins_and_sprays_in_its_third_mode() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(60);
        let mut q = boss(terrustia_proto::npc_params::ICE_QUEEN, 0.0, 0.0);
        q.ai[0] = 2.0;
        q.velocity = (8.0, 0.0);
        let w = night(&tiles, Some((0.0, 400.0)));

        let mut shots = Vec::new();
        for _ in 0..200 {
            q.ai[1] = 0.0; // held in mode 2
            shots.extend(ice_queen(&mut q, &w, &mut rng).shots);
        }
        assert!(q.velocity.0.abs() < 0.1, "it should have braked to a stop");
        assert!(q.rotation.abs() > 1.0, "and be spinning");
        assert!(!shots.is_empty(), "it should have sprayed shards");
        assert!(
            shots
                .iter()
                .all(|s| s.projectile == ICE_QUEEN_SHARD && s.damage == ICE_QUEEN_SPIN_DAMAGE),
            "its own damage figure, not mode 1's"
        );
        assert!(
            shots
                .iter()
                .all(|s| (s.velocity.0.hypot(s.velocity.1) - ICE_QUEEN_SPIN_SPEED).abs() < 1e-3),
            "all at fifteen a tick"
        );
        // Random bearings: the spray goes every way, unlike either aimed mode.
        assert!(shots.iter().any(|s| s.velocity.0 > 0.0));
        assert!(shots.iter().any(|s| s.velocity.0 < 0.0));
        assert!(shots.iter().any(|s| s.velocity.1 < 0.0));
    }

    /// SNK-3: it is a tracked vehicle, and it never had a vertical routine at all
    /// (`NPC.cs:34188-34237`). With `no_gravity` and `no_tile_collide` set and nothing ever writing
    /// `velocity.1`, it hung at its spawn altitude and drove straight through the mountain.
    #[test]
    fn santa_climbs_ground_and_falls_off_ledges() {
        let mut rng = SmallRng::seed_from_u64(61);
        let mut s = boss(terrustia_proto::npc_params::SANTA_NK1, 0.0, 0.0);
        // Rock right under its treads.
        let floor = ground(((s.position.1 + s.height()) / TILE) as i32);
        let w = night(&floor, Some((600.0, 0.0)));
        santa(&mut s, &w, &mut rng);
        assert!(
            s.velocity.1 < 0.0,
            "on solid ground it should climb, got {}",
            s.velocity.1
        );

        let sky = Sky(HashMap::new());
        let mut over_a_pit = boss(terrustia_proto::npc_params::SANTA_NK1, 0.0, 0.0);
        let w = night(&sky, Some((600.0, 0.0)));
        santa(&mut over_a_pit, &w, &mut rng);
        assert!(
            over_a_pit.velocity.1 > 0.0,
            "over nothing it should fall, got {}",
            over_a_pit.velocity.1
        );
        // ...and the fall is capped rather than running away.
        for _ in 0..200 {
            santa(&mut over_a_pit, &w, &mut rng);
        }
        assert!((over_a_pit.velocity.1 - SANTA_FALL_CAP).abs() < 1e-4);
    }

    /// SNK-3: a player standing entirely inside its width and below its treads makes it drop, solid
    /// ground or not (`NPC.cs:34191-34199`).
    #[test]
    fn santa_drops_on_someone_underneath() {
        let mut rng = SmallRng::seed_from_u64(61);
        let mut s = boss(terrustia_proto::npc_params::SANTA_NK1, 0.0, 0.0);
        let floor = ground(((s.position.1 + s.height()) / TILE) as i32);
        // Directly beneath its middle, well below its treads.
        let under = (s.center().0, s.position.1 + s.height() + 200.0);
        let w = night(&floor, Some(under));
        santa(&mut s, &w, &mut rng);
        assert!(
            s.velocity.1 > 0.0,
            "it should drop on them rather than climb, got {}",
            s.velocity.1
        );
    }

    /// SNK-1: all four weapons, not just the gun (`NPC.cs:34048-34163`).
    #[test]
    fn santa_carries_four_weapons() {
        let sky = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(61);
        let mut s = boss(terrustia_proto::npc_params::SANTA_NK1, 0.0, 0.0);
        let w = night(&sky, Some((300.0, 0.0)));
        let mut kinds = std::collections::HashSet::new();
        for _ in 0..200_000 {
            for shot in santa(&mut s, &w, &mut rng).shots {
                kinds.insert(shot.projectile);
            }
            if kinds.len() == 4 {
                break;
            }
        }
        assert_eq!(
            kinds,
            std::collections::HashSet::from([
                SANTA_BULLET,
                SANTA_PRESENT,
                SANTA_ROCKET,
                SANTA_MISSILE
            ]),
            "the present bomb, the rockets and the missiles were all missing"
        );
    }

    /// SNK-1: the missiles go up, not at you, and carry their own damage figure
    /// (`NPC.cs:34141-34158`).
    #[test]
    fn santas_missiles_go_up_and_its_presents_go_slowly() {
        let sky = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(7);
        let mut s = boss(terrustia_proto::npc_params::SANTA_NK1, 0.0, 0.0);
        let w = night(&sky, Some((300.0, 0.0)));
        let mut missiles = Vec::new();
        let mut presents = Vec::new();
        for _ in 0..400_000 {
            for shot in santa(&mut s, &w, &mut rng).shots {
                if shot.projectile == SANTA_MISSILE {
                    missiles.push(shot);
                } else if shot.projectile == SANTA_PRESENT {
                    presents.push(shot);
                }
            }
            if missiles.len() > 20 && presents.len() > 5 {
                break;
            }
        }
        assert!(!missiles.is_empty() && !presents.is_empty());
        assert!(
            missiles
                .iter()
                .all(|m| m.velocity.1 < 0.0 && m.damage == SANTA_MISSILE_DAMAGE),
            "the missile volley is fired near-vertically"
        );
        assert!(
            presents.iter().all(|p| p.damage == SANTA_PRESENT_DAMAGE
                && (p.velocity.0.hypot(p.velocity.1) - SANTA_PRESENT_SPEED).abs() < 1e-3),
            "a present is lobbed at one pixel a tick and hits for eighty"
        );
    }

    /// SNK-2: the leash is a real 2D distance (`NPC.cs:33992`), and it gates the gun, not the
    /// machine. Measured across only, someone hovering straight overhead was always in range.
    #[test]
    fn santas_leash_is_measured_in_two_dimensions() {
        let sky = Sky(HashMap::new());
        let guns = |at: (f32, f32)| {
            let mut rng = SmallRng::seed_from_u64(61);
            let mut s = boss(terrustia_proto::npc_params::SANTA_NK1, 0.0, 0.0);
            s.ai[0] = 1.0; // firing
            let w = night(&sky, Some(at));
            (0..400)
                .flat_map(|_| santa(&mut s, &w, &mut rng).shots)
                .filter(|shot| shot.projectile == SANTA_BULLET)
                .count()
        };
        assert!(guns((1900.0, 0.0)) > 0, "inside two thousand: it shoots");
        assert_eq!(
            guns((1600.0, 1600.0)),
            0,
            "2263 px away on the diagonal is out of reach even though each axis is under 2000"
        );
    }

    /// SNK-2: and losing you does not stop its clock. Out of reach it still cycles between waiting
    /// and firing, so it is ready the moment you come back.
    #[test]
    fn santa_keeps_its_clock_when_you_are_out_of_reach() {
        let sky = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(61);
        let mut s = boss(terrustia_proto::npc_params::SANTA_NK1, 0.0, 0.0);
        let w = night(&sky, Some((9000.0, 0.0)));
        for _ in 0..(SANTA_WAIT as i32 + 1) {
            santa(&mut s, &w, &mut rng);
        }
        assert_eq!(s.ai[0], 1.0, "the wait should have run out anyway");
    }

    /// It plants rather than driving past you once you are within fifty pixels
    /// (`NPC.cs:34165-34168`), which is what stops it shunting you along the ground.
    #[test]
    fn santa_plants_when_you_are_right_next_to_it() {
        let sky = Sky(HashMap::new());
        let rng = SmallRng::seed_from_u64(61);
        let brake = |across: f32| {
            let mut s = boss(terrustia_proto::npc_params::SANTA_NK1, 0.0, 0.0);
            s.velocity.0 = 4.0;
            let w = night(&sky, Some((s.center().0 + across, 0.0)));
            santa(&mut s, &w, &mut rng.clone());
            s.velocity.0
        };
        // Planted, the brake is a flat 0.9 a tick; driving, the same speed is lerped toward the
        // walk over twenty-one ticks, which from four is a far gentler fall.
        assert!(
            (brake(20.0) - 4.0 * 0.9).abs() < 1e-4,
            "close in it plants, got {}",
            brake(20.0)
        );
        assert!(
            brake(400.0) > brake(20.0),
            "further off it drives on: {} vs {}",
            brake(400.0),
            brake(20.0)
        );
    }

    /// Santa's gun speeds up as it is worn down.
    #[test]
    fn santa_fires_faster_as_it_dies() {
        let tiles = Sky(HashMap::new());
        let w = night(&tiles, Some((600.0, 0.0)));
        let shots = |health: f32| {
            let mut rng = SmallRng::seed_from_u64(61);
            let mut s = boss(terrustia_proto::npc_params::SANTA_NK1, 0.0, 0.0);
            s.life = (s.life_max as f32 * health) as i32;
            s.ai[0] = 1.0;
            (0..300)
                .map(|_| santa(&mut s, &w, &mut rng).shots.len())
                .sum::<usize>()
        };
        assert!(
            shots(0.1) > shots(1.0),
            "a hurt Santa should fire more: {} vs {}",
            shots(0.1),
            shots(1.0)
        );
    }

    /// Daylight ends all of them, each in its own way.
    #[test]
    fn daylight_ends_the_moon() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(4);
        let mut w = night(&tiles, Some((600.0, 400.0)));
        w.conditions.day = true;

        let mut p = boss(PUMPKING, 0.0, 0.0);
        p.ai[0] = 1.0;
        pumpking(&mut p, &w, &mut rng);
        assert!(p.velocity.1 > 0.0, "Pumpking sinks");

        let mut q = boss(terrustia_proto::npc_params::ICE_QUEEN, 0.0, 0.0);
        q.velocity.0 = 1.0;
        ice_queen(&mut q, &w, &mut rng);
        assert!(q.velocity.1 < 0.0, "the Ice Queen climbs away");

        let mut s = boss(terrustia_proto::npc_params::SANTA_NK1, 0.0, 0.0);
        let out = santa(&mut s, &w, &mut rng);
        // The gun belongs to the night's firing state and stops. The three fused weapons do not:
        // vanilla runs them below the day/night branch, so a Santa-NK1 driving off at dawn is
        // still lobbing presents (`NPC.cs:34073`, outside the `Main.dayTime` chain).
        assert!(
            out.shots.iter().all(|s| s.projectile != SANTA_BULLET),
            "Santa's gun stops"
        );
    }

    /// PUMP-2: losing the player at night is the gentle leaving drift (`ai[1] = 2`):
    /// `velocity.Y += 0.1` with a shorter `EncourageDespawn(500)` grace, not the hard daytime sink
    /// (0.3 and a 600-tick grace) the old code reused for both.
    #[test]
    fn a_leashed_out_pumpking_leaves_gently() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(58);
        let mut p = boss(PUMPKING, 0.0, 0.0);
        p.velocity = (4.0, 0.0);
        p.time_left = 10_000;
        let w = night(&tiles, None); // night, but nobody to fight
        pumpking(&mut p, &w, &mut rng);
        assert!(
            (p.velocity.1 - 0.1).abs() < 1e-4,
            "the gentle 0.1 leaving drift, not the 0.3 daytime sink, got {}",
            p.velocity.1
        );
        // And the horizontal brake, `velocity.X *= 0.95f` (`NPC.cs:33479`), which was missed
        // entirely: without it a Pumpking that lost you kept its whole last hover speed sideways
        // and sailed off across the arena instead of settling.
        assert!(
            (p.velocity.0 - 4.0 * 0.95).abs() < 1e-4,
            "it brakes sideways too, got {}",
            p.velocity.0
        );
        assert!(
            p.time_left <= 500,
            "the shorter EncourageDespawn(500) grace, got {}",
            p.time_left
        );
    }
}
