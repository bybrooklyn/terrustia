//! Style 123 — Deerclops.
//!
//! Unusually for a boss, it walks. Everything it does is a ground attack chosen by where you are
//! standing and how long it has been since it last did something:
//!
//! * up close, a wall of **ice spikes** thrown forward — or, if it has already done that recently,
//!   thrown out to *both* sides, which is the answer to standing behind it;
//! * at four seconds, a **slam** that throws rubble up out of the ground in a fan, one chunk a tick,
//!   each arcing back down onto you;
//! * standing still for a second and a half, six **shadow hands** (and, in Expert, a passive rain of
//!   them running the whole fight besides);
//! * at a distance, a **roar** that leaves you Slowed for twelve seconds.
//!
//! It also has a den. Leave the snow, or get more than two and a half thousand pixels away, and it
//! stops fighting and walks home — and if the walk takes too long it simply teleports. Get far
//! enough away for half a second and it becomes untouchable, so there is no killing it from range.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    DEER_DEN, DEER_GIVE_UP, DEER_GOING_HOME, DEER_LEAVING, DEER_PASSIVE_SHADOW_FAST,
    DEER_PASSIVE_SHADOW_RANGE, DEER_PASSIVE_SHADOW_RING, DEER_PASSIVE_SHADOW_ROTATION,
    DEER_PASSIVE_SHADOW_SLOW, DEER_PASSIVE_SHADOW_WAVES, DEER_PATIENCE, DEER_PATIENCE_DEEP,
    DEER_ROAR, DEER_ROAR_RANGE, DEER_ROAR_SLOW, DEER_ROAR_TICKS, DEER_RUBBLE_DAMAGE,
    DEER_RUBBLE_SLAM, DEER_RUBBLE_TICKS, DEER_RUBBLE_WINDUP, DEER_SHADOW_AT, DEER_SHADOW_DAMAGE,
    DEER_SHADOW_DAMAGE_PASSIVE, DEER_SHADOW_HANDS, DEER_SHADOW_HANDS_COUNT, DEER_SHADOW_TICKS,
    DEER_SHIELD_AFTER, DEER_SHIELD_RANGE, DEER_SPIKE_COUNT, DEER_SPIKE_DAMAGE, DEER_SPIKE_RANGE,
    DEER_SPIKES_BOTH, DEER_SPIKES_BOTH_TICKS, DEER_SPIKES_BOTH_WINDUP, DEER_SPIKES_FORWARD,
    DEER_SPIKES_FORWARD_TICKS, DEER_SPIKES_FORWARD_WINDUP, DEER_STALKING, DEER_STOP_WITHIN,
    DEER_TELEPORT_AT, DEER_TELEPORTING, DEER_UNTIL_ROAR, DEER_UNTIL_RUBBLE, DEER_UNTIL_SHADOW,
    DEER_WALK, DEER_WALK_EASE, DEER_WALK_RAGE,
};
use terrustia_proto::projectile::ids::{DEER_RUBBLE, DEER_SHADOW_HAND, DEER_SPIKE};
use terrustia_proto::tile_solid::solid;

use crate::game::ai::{Shot, World};
use crate::game::npc::{Npc, TILE, TileView};
use crate::game::npc_ai::Target;

/// What a tick of the fight produced.
#[derive(Debug, Default)]
pub struct Rampage {
    pub shots: Vec<Shot>,
    /// Set when its roar should leave everyone nearby Slowed.
    pub roared: bool,
    /// Set when it has finished leaving.
    pub gone: bool,
}

/// The ground beneath a tile column, within a few tiles of a starting row.
fn ground_under(tiles: &impl TileView, x: i32, from: i32) -> Option<i32> {
    (from - 4..from + 16).find(|&y| {
        let t = tiles.tile(x, y);
        t.is_active() && solid(t.block) && !tiles.tile(x, y - 1).is_active()
    })
}

/// Throw a line of ice spikes out of the ground.
fn spikes<T: TileView>(npc: &Npc, world: &World<'_, T>, direction: i8, out: &mut Vec<Shot>) {
    let foot = (
        (npc.center().0 / TILE) as i32 + i32::from(direction) * 3,
        ((npc.position.1 + npc.height()) / TILE) as i32,
    );
    for step in 0..DEER_SPIKE_COUNT {
        let x = foot.0 + step * i32::from(direction);
        let Some(y) = ground_under(world.tiles, x, foot.1) else {
            continue;
        };
        // Each spike leans a little further out than the last, so the wall fans away from it.
        let lean = (step * i32::from(direction)) as f32
            * 0.7
            * (std::f32::consts::FRAC_PI_4 / DEER_SPIKE_COUNT as f32);
        out.push(Shot {
            projectile: DEER_SPIKE,
            damage: DEER_SPIKE_DAMAGE,
            position: ((x * 16 + 8) as f32, (y * 16 - 8) as f32),
            velocity: (lean.sin(), -lean.cos()),
            // Zero, meaning the type's own, because the spike's arm ends it at twenty ticks
            // (`aiStyle 157`) long before any table value matters. The 300 that used to sit here
            // was invented and was doing the ending: vanilla passes no lifetime at
            // `NPC.cs:45055`, and a wall of twenty spikes stood for five seconds rather than a
            // third of one.
            time_left: 0,
            ai: [0.0; 3],
        });
    }
}

/// Where a shadow hand comes out of the dark, how fast, and which of style 187's four routines it
/// runs. `Projectile.RandomizeInsanityShadowFor` (`Projectile.cs:43179-43272`), hostile arm.
///
/// The whole of the shadow-hand attack is decided here rather than in the projectile: `ai[0]` is
/// the band the arm reads to pick a routine (0 drift, 180 swing, 300 lunge, 390 arc) and `ai[1]`
/// is that routine's one parameter. A hand launched without them drifts, which is what all six of
/// a wave used to do.
///
/// Two narrowings, both because this server carries one target rather than 255 players. Vanilla
/// rolls the placement up to eight times, rejecting any that lands on top of *another* player and
/// stepping to the next routine each time it does (`:43250-43270`); with one target there is
/// nobody else to land on, so the first roll stands. `num` - the side a drifting hand comes from -
/// is flipped away from the direction the target is *moving*, and that is read here off the
/// target's own velocity exactly as vanilla reads it.
fn shadow_hand(target: &Target, rng: &mut SmallRng) -> ((f32, f32), (f32, f32), f32, f32) {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};

    /// `num5`: how far ahead of the target a drifting hand is aimed, and `num4 + 10 + 10` for the
    /// speed that covers the ring in that many ticks. The `+= 10f` fires on this arm alone.
    const LEAD: f32 = 30.0;
    const DRIFT_OVER: f32 = 50.0;
    /// The arc's backwards walk: sixty steps of eight pixels, turning by up to a quarter-turn
    /// spread over all of them.
    const ARC_STEPS: i32 = 60;
    const ARC_STEP: f32 = 8.0;
    /// What a swinging or lunging hand is already moving at when it appears.
    const ON_RING: f32 = 4.0;

    let mut side = if rng.random::<bool>() { 1.0f32 } else { -1.0 };
    if target.velocity.0 * side > 0.0 {
        side = -side;
    }
    // On the ring, pointed inward: the swing and the lunge share a placement and differ only in
    // what they do with it afterwards.
    let on_ring = |rng: &mut SmallRng| {
        let angle = TAU * rng.random::<f32>();
        let (sin, cos) = angle.sin_cos();
        (
            (
                target.center.0 - cos * DEER_PASSIVE_SHADOW_RING,
                target.center.1 - sin * DEER_PASSIVE_SHADOW_RING,
            ),
            (cos * ON_RING, sin * ON_RING),
            angle,
        )
    };
    match rng.random_range(0..4) {
        1 => {
            let (at, velocity, angle) = on_ring(rng);
            // Vanilla passes `num10 - PI/2` and `AI_187`'s swing never reads it: the swing works
            // off `rotation`, which a fresh projectile starts at zero. Carried anyway, because a
            // value the game sends is a value a client may read.
            (at, velocity, 180.0, angle - FRAC_PI_2)
        }
        2 => {
            let (at, velocity, angle) = on_ring(rng);
            (at, velocity, 300.0, angle)
        }
        // The arc. Vanilla walks the curve *backwards* from where the target will be in sixty
        // ticks, one eight-pixel step at a time and rotating as it goes, so the hand starts
        // wherever that walk ends up and the turn it is handed is the one that brings it back.
        3 => {
            let heading = TAU * rng.random::<f32>();
            let turn = FRAC_PI_2 / ARC_STEPS as f32 * (rng.random::<f32>() * 2.0 - 1.0);
            let mut at = (
                target.center.0 + target.velocity.0 * ARC_STEPS as f32,
                target.center.1 + target.velocity.1 * ARC_STEPS as f32,
            );
            let mut step = (heading.cos() * ARC_STEP, heading.sin() * ARC_STEP);
            let (sin, cos) = (-turn).sin_cos();
            for _ in 0..ARC_STEPS {
                at = (at.0 - step.0, at.1 - step.1);
                step = (step.0 * cos - step.1 * sin, step.0 * sin + step.1 * cos);
            }
            (at, step, 390.0, turn)
        }
        // The drift, which is vanilla's `default:` arm: straight in from one side, aimed a second
        // ahead of where the target is going, at the speed that covers the ring in fifty ticks.
        _ => {
            let jitter = (rng.random::<f32>() * 2.0 - 1.0) * PI * 0.125;
            let (sin, cos) = jitter.sin_cos();
            let out = -side * DEER_PASSIVE_SHADOW_RING;
            let speed = side * DEER_PASSIVE_SHADOW_RING / DRIFT_OVER;
            (
                (
                    target.center.0 + target.velocity.0 * LEAD + out * cos,
                    target.center.1 + target.velocity.1 * LEAD + out * sin,
                ),
                (speed * cos, speed * sin),
                0.0,
                0.0,
            )
        }
    }
}

/// DEER-2: throw one chunk of rubble up out of the ground, fanned by which one it is. Vanilla
/// `AI_123_Deerclops_ShootRubbleUp` (`NPC.cs:44913-44932`): the source is a point ten tiles above
/// Deerclops (and three ahead), scanned downward for the first solid ground; the chunk then launches
/// upward from there (proj 962, an arc that flies flat and falls back), fanned by an angle
/// proportional to `which`. This is the same ground-launched fan as the ice spikes, not the old
/// diagonal line dropped from above.
fn rubble_up<T: TileView>(
    npc: &Npc,
    world: &World<'_, T>,
    which: i32,
    rng: &mut SmallRng,
    out: &mut Vec<Shot>,
) {
    let dir = i32::from(npc.direction);
    let x = (npc.center().0 / TILE) as i32 + dir * 3 + which * dir;
    let from = (npc.position.1 / TILE) as i32 - 10;
    // The first solid tile below the source point, scanned over thirty-five tiles (`i in 0..35`).
    let Some(y) = (from..from + 35).find(|&y| {
        let t = world.tiles.tile(x, y);
        t.is_active() && solid(t.block)
    }) else {
        return;
    };
    let lean = (which * dir) as f32 * 0.7 * (std::f32::consts::FRAC_PI_4 / DEER_SPIKE_COUNT as f32);
    let speed = 8.0 + rng.random::<f32>() * 8.0;
    out.push(Shot {
        projectile: DEER_RUBBLE,
        damage: DEER_RUBBLE_DAMAGE,
        position: ((x * 16 + 8) as f32, (y * 16 - 8) as f32),
        velocity: (lean.sin() * speed, -lean.cos() * speed),
        time_left: 220,
        ai: [0.0; 3],
    });
}

/// Whether it should break off and go home.
fn should_go_home<T: TileView>(npc: &Npc, world: &World<'_, T>, chasing: bool) -> bool {
    let Some(target) = world.target else {
        return true;
    };
    let home = (npc.ai[2] * 16.0, npc.ai[3] * 16.0);
    let near_den = ((target.center.0 - home.0).powi(2) + (target.center.1 - home.1).powi(2)).sqrt()
        <= DEER_DEN;
    let at_home_in_the_cold = world.conditions.snow || near_den;
    let (cx, cy) = npc.center();
    let reach = ((target.center.0 - cx).powi(2) + (target.center.1 - cy).powi(2)).sqrt();
    !target.alive || (!chasing && !at_home_in_the_cold) || reach >= DEER_GIVE_UP
}

/// Walk toward something, stepping up what it can and stopping when close enough.
fn walk<T: TileView>(npc: &mut Npc, world: &World<'_, T>, toward: f32, halt: bool) {
    let speed = DEER_WALK + DEER_WALK_RAGE * (1.0 - npc.life as f32 / npc.life_max as f32);
    let across = toward - npc.center().0;
    let close = across.abs() < DEER_STOP_WITHIN;
    if close || halt {
        npc.velocity.0 *= 0.9;
        if npc.velocity.0.abs() < 0.1 {
            npc.velocity.0 = 0.0;
        }
    } else {
        let wanted = across.signum() * speed;
        npc.velocity.0 += (wanted - npc.velocity.0) / DEER_WALK_EASE;
        npc.direction = across.signum() as i8;
        npc.sprite_direction = npc.direction;
    }

    // A wall it can climb rather than be stopped by.
    if npc.velocity.1 == 0.0 && npc.velocity.0 != 0.0 {
        let ahead = ((npc.center().0 + npc.width() / 2.0 * npc.velocity.0.signum()) / TILE) as i32;
        let foot = ((npc.position.1 + npc.height() - 1.0) / TILE) as i32;
        let blocked = |y: i32| {
            let t = world.tiles.tile(ahead, y);
            t.is_active() && solid(t.block)
        };
        if blocked(foot) && !blocked(foot - 1) && !blocked(foot - 2) {
            npc.position.1 = (foot * 16) as f32 - npc.height();
        } else if blocked(foot) {
            npc.velocity.1 = -8.0;
        }
    }
}

/// Drive Deerclops for a tick.
pub fn update<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) -> Rampage {
    let mut out = Rampage::default();

    // First tick: this clearing is its den, and it is what it will walk back to.
    if npc.ai[2] == 0.0 && npc.ai[3] == 0.0 {
        npc.ai[2] = (npc.center().0 / TILE).floor();
        npc.ai[3] = ((npc.position.1 + npc.height()) / TILE).floor();
        npc.dirty = true;
    }

    // Out of reach for half a second and it stops being hittable at all.
    let far = world.target.is_none_or(|t| {
        let (cx, cy) = npc.center();
        ((t.center.0 - cx).powi(2) + (t.center.1 - cy).powi(2)).sqrt() >= DEER_SHIELD_RANGE
    });
    npc.local_ai[3] =
        (npc.local_ai[3] + if far { 1.0 } else { -1.0 }).clamp(0.0, DEER_SHIELD_AFTER);
    npc.invulnerable = npc.local_ai[3] >= DEER_SHIELD_AFTER;

    // DEER-1: in Expert Mode a passive rain of shadow hands runs throughout the fight, quite apart
    // from the dedicated shadow-hands attack below (`SpawnPassiveShadowHands`,
    // `NPC.cs:44522-44524,44890-44912`). They come faster as it is worn down: a hand every
    // `40 + 40 * lifePercent` ticks, three waves and then a pause, keyed off `local_ai[2]`.
    if world.conditions.expert
        && let Some(target) = world.target
    {
        let life_percent = npc.life as f32 / npc.life_max.max(1) as f32;
        let interval = (DEER_PASSIVE_SHADOW_FAST
            + (DEER_PASSIVE_SHADOW_SLOW - DEER_PASSIVE_SHADOW_FAST) * life_percent.clamp(0.0, 1.0))
        .round()
        .max(1.0);
        npc.local_ai[2] += 1.0;
        if npc.local_ai[2] % interval == 0.0 {
            let wave = (npc.local_ai[2] / interval) as u32;
            if npc.local_ai[2] / interval >= DEER_PASSIVE_SHADOW_WAVES {
                npc.local_ai[2] = 0.0;
            }
            // BS3-M4: a wave is not a hand for everybody. `Boss_CanShootExtraAt`
            // (`NPC.cs:47474-47494`) takes the wave's index modulo three and raises a hand only for
            // the players whose own slot matches, and refuses outright past 1200 pixels from the
            // boss. So three waves are one hand for any given player, not three, and running out of
            // the fight stops the rain rather than merely spreading it. Every wave fired here from
            // any distance, three times too often.
            //
            // Narrowing: vanilla walks all 255 slots and this server carries one target, so the
            // rotation is checked against that target's own slot. The rest of the predicate
            // (active, not dead, has interacted) is what `world.target` already means.
            let (bx, by) = npc.center();
            let in_rotation = u32::from(target.slot) % DEER_PASSIVE_SHADOW_ROTATION
                == wave % DEER_PASSIVE_SHADOW_ROTATION;
            let reach = (target.center.0 - bx).hypot(target.center.1 - by);
            if in_rotation && reach <= DEER_PASSIVE_SHADOW_RANGE {
                // One hand out of the dark around the target, placed and pointed by
                // [`shadow_hand`] - which is also what decides which of style 187's four routines
                // it runs. This used to roll its own angle and send the hand straight at the
                // target, which is one of the four and only by accident.
                let (position, velocity, ai0, ai1) = shadow_hand(&target, rng);
                out.shots.push(Shot {
                    projectile: DEER_SHADOW_HAND,
                    damage: DEER_SHADOW_DAMAGE_PASSIVE,
                    position,
                    velocity,
                    // Zero: the arm ends each hand one tick short of its own band, so a lifetime
                    // here could only ever cut a routine off partway.
                    time_left: 0,
                    ai: [ai0, ai1, 0.0],
                });
            }
        }
    }

    let mut halt = false;
    let mut going_home = false;
    let state = npc.ai[0];

    // DEER-3: the roar's Slow cooldown counts down every tick, whatever it is doing. It stands in
    // for vanilla's `flag13` gate (the target must not already carry the Slow buff) since the server
    // keeps no queryable player-buff state; see `DEER_ROAR_SLOW`.
    if npc.local_ai[0] > 0.0 {
        npc.local_ai[0] -= 1.0;
    }

    if state == DEER_STALKING {
        if should_go_home(npc, world, true) {
            npc.ai[0] = DEER_GOING_HOME;
            npc.ai[1] = 0.0;
            npc.local_ai[1] = 0.0;
            npc.dirty = true;
        } else if let Some(target) = world.target {
            npc.ai[1] += 1.0;
            let from = (npc.center().0, npc.position.1 + npc.height() - 32.0);
            let to = (target.center.0, target.center.1);
            let (dx, dy) = (to.0 - from.0, to.1 - from.1);
            let facing = dx.abs() >= dy.abs() * 0.6 || (dx * dx + dy * dy).sqrt() < 48.0;
            let level = dy <= 100.0 + crate::game::ai::PLAYER_HEIGHT as f32 && dy >= -200.0;
            let close = dx.abs() < DEER_SPIKE_RANGE && level && npc.velocity.1 == 0.0;

            // Which attack, in the order the game checks them.
            if close && npc.local_ai[1] >= 2.0 {
                npc.velocity.0 = 0.0;
                npc.ai[0] = DEER_SPIKES_BOTH;
                npc.ai[1] = 0.0;
                npc.local_ai[1] = 0.0;
            } else if close && facing {
                npc.velocity.0 = 0.0;
                npc.ai[0] = DEER_SPIKES_FORWARD;
                npc.ai[1] = 0.0;
                npc.local_ai[1] += 1.0;
            } else if npc.ai[1] >= DEER_UNTIL_RUBBLE
                && npc.velocity.1 == 0.0
                && npc.velocity.0 != 0.0
            {
                npc.velocity.0 = 0.0;
                npc.ai[0] = DEER_RUBBLE_SLAM;
                npc.ai[1] = 0.0;
                npc.local_ai[1] = 0.0;
            } else if npc.ai[1] >= DEER_UNTIL_SHADOW
                && npc.velocity.1 == 0.0
                && npc.velocity.0 == 0.0
            {
                npc.ai[0] = DEER_SHADOW_HANDS;
                npc.ai[1] = 0.0;
                npc.local_ai[1] = 0.0;
            } else if npc.ai[1] >= DEER_UNTIL_ROAR
                && npc.velocity.1 == 0.0
                && dx.abs() > DEER_ROAR_RANGE
                && npc.local_ai[0] <= 0.0
            {
                npc.velocity.0 = 0.0;
                npc.ai[0] = DEER_ROAR;
                npc.ai[1] = 0.0;
                npc.local_ai[1] = 0.0;
                // Hold off the next roar for the life of the Slow it applies (DEER-3).
                npc.local_ai[0] = DEER_ROAR_SLOW;
            }
            if npc.ai[0] != DEER_STALKING {
                npc.dirty = true;
            }
        }
    } else if state == DEER_SPIKES_FORWARD {
        npc.ai[1] += 1.0;
        halt = true;
        if npc.ai[1] == DEER_SPIKES_FORWARD_WINDUP {
            spikes(npc, world, npc.direction, &mut out.shots);
        }
        if npc.ai[1] >= DEER_SPIKES_FORWARD_TICKS {
            npc.ai[0] = DEER_STALKING;
            npc.ai[1] = 0.0;
            npc.dirty = true;
        }
    } else if state == DEER_SPIKES_BOTH {
        npc.ai[1] += 1.0;
        halt = true;
        if npc.ai[1] == DEER_SPIKES_BOTH_WINDUP {
            // The answer to standing behind it.
            spikes(npc, world, 1, &mut out.shots);
            spikes(npc, world, -1, &mut out.shots);
        }
        if npc.ai[1] >= DEER_SPIKES_BOTH_TICKS {
            npc.ai[0] = DEER_STALKING;
            npc.ai[1] = 0.0;
            npc.dirty = true;
        }
    } else if state == DEER_RUBBLE_SLAM {
        npc.ai[1] += 1.0;
        halt = true;
        // DEER-2: after the wind-up, one chunk of rubble a tick is thrown up out of the ground in a
        // fan, arcing back down (`NPC.cs:44718-44739`, one `ShootRubbleUp` per tick with `whichOne`
        // counting up), rather than the old diagonal line dropped from above.
        if npc.ai[1] >= DEER_RUBBLE_WINDUP {
            let which = (npc.ai[1] - DEER_RUBBLE_WINDUP) as i32;
            if which < DEER_SPIKE_COUNT {
                rubble_up(npc, world, which, rng, &mut out.shots);
            }
        }
        if npc.ai[1] >= DEER_RUBBLE_TICKS {
            npc.ai[0] = DEER_STALKING;
            npc.ai[1] = 0.0;
            npc.dirty = true;
        }
    } else if state == DEER_ROAR {
        npc.ai[1] += 1.0;
        halt = true;
        if npc.ai[1] == DEER_SHADOW_AT {
            out.roared = true;
        }
        if npc.ai[1] >= DEER_ROAR_TICKS {
            npc.ai[0] = DEER_STALKING;
            npc.ai[1] = 0.0;
            npc.dirty = true;
        }
    } else if state == DEER_SHADOW_HANDS {
        npc.ai[1] += 1.0;
        halt = true;
        if npc.ai[1] == DEER_SHADOW_AT
            && let Some(target) = world.target
        {
            for _ in 0..DEER_SHADOW_HANDS_COUNT {
                // They come out of the dark around whoever it is looking at, not out of Deerclops
                // - and each rolls its own routine, so a wave of six is a mix of drifts, swings,
                // lunges and arcs rather than six of the same thing thrown off a ring.
                let (position, velocity, ai0, ai1) = shadow_hand(&target, rng);
                out.shots.push(Shot {
                    projectile: DEER_SHADOW_HAND,
                    damage: DEER_SHADOW_DAMAGE,
                    position,
                    velocity,
                    time_left: 0,
                    ai: [ai0, ai1, 0.0],
                });
            }
        }
        if npc.ai[1] >= DEER_SHADOW_TICKS {
            npc.ai[0] = DEER_STALKING;
            npc.ai[1] = 0.0;
            npc.dirty = true;
        }
    } else if state == DEER_GOING_HOME {
        going_home = true;
        npc.ai[1] += 1.0;
        if !should_go_home(npc, world, false) {
            npc.ai[0] = DEER_STALKING;
            npc.ai[1] = 0.0;
            npc.local_ai[1] = 0.0;
            npc.dirty = true;
        } else {
            let home = (npc.ai[2] * 16.0, npc.ai[3] * 16.0);
            let (cx, cy) = npc.center();
            let from_home = ((home.0 - cx).powi(2) + (home.1 - cy).powi(2)).sqrt();
            let nearly_there = from_home < 1020.0;
            // Home, and pacing: it settles for most of every ten-second cycle.
            if nearly_there && npc.ai[1] % 600.0 < 420.0 {
                halt = true;
            }
            // Too deep, or too long about it, and it gives up walking.
            let stuck = (npc.position.1 > home.1 + 1600.0 && npc.ai[1] >= DEER_PATIENCE_DEEP)
                || (!nearly_there && npc.ai[1] >= DEER_PATIENCE);
            if stuck {
                npc.ai[0] = DEER_TELEPORTING;
                npc.ai[1] = 0.0;
                npc.dirty = true;
            }
        }
    } else if state == DEER_TELEPORTING {
        npc.ai[1] += 1.0;
        halt = true;
        npc.velocity.1 = -0.1;
        if npc.ai[1] == DEER_TELEPORT_AT {
            npc.position.0 = npc.ai[2] * 16.0 - npc.width() / 2.0;
            npc.position.1 = npc.ai[3] * 16.0 - npc.height();
            npc.dirty = true;
        }
        if npc.ai[1] >= 60.0 {
            npc.ai[0] = DEER_STALKING;
            npc.ai[1] = 0.0;
            npc.dirty = true;
        }
    } else if state == DEER_LEAVING {
        npc.ai[1] += 1.0;
        halt = true;
        if npc.ai[1] >= DEER_TELEPORT_AT {
            out.gone = true;
        }
    }

    // Movement, whatever it is doing: toward the target, or toward home, or nowhere.
    let toward = if going_home {
        let home = npc.ai[2] * 16.0;
        // Once home it paces rather than standing on the spot.
        if (home - npc.center().0).abs() < 240.0 {
            npc.center().0 + 160.0 * f32::from(npc.direction)
        } else {
            home
        }
    } else {
        world.target.map_or(npc.center().0, |t| t.center.0)
    };
    walk(npc, world, toward, halt);

    npc.dirty = true;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ai::Conditions;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    #[derive(Default)]
    struct Tundra(HashMap<(i32, i32), Tile>);

    impl TileView for Tundra {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn snowfield() -> Tundra {
        let mut t = Tundra::default();
        for x in 0..4000 {
            for y in 300..320 {
                t.0.insert((x, y), Tile::block(147));
            }
        }
        t
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(123)
    }

    fn deerclops(tile_x: i32) -> Npc {
        let mut n = Npc::new(668, (0.0, 0.0), 1).expect("deerclops");
        n.position = (tile_x as f32 * TILE, 300.0 * TILE - n.height());
        n
    }

    fn tundra<'a>(tiles: &'a Tundra, target: Option<Target>) -> World<'a, Tundra> {
        World {
            conditions: Conditions {
                snow: true,
                ..Conditions::default()
            },
            ..crate::game::ai::calm(tiles, target)
        }
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
    fn it_remembers_where_the_fight_started() {
        let tiles = snowfield();
        let mut d = deerclops(200);
        let t = Some(player_at(200.0 * TILE + 400.0, 299.0 * TILE));
        let (cx, bottom) = (d.center().0, d.position.1 + d.height());
        update(&mut d, &tundra(&tiles, t), &mut rng());
        assert_eq!(d.ai[2], (cx / TILE).floor(), "its den is where it woke up");
        assert_eq!(d.ai[3], (bottom / TILE).floor());
    }

    #[test]
    fn it_walks_at_you_and_faster_as_it_dies() {
        let tiles = snowfield();
        let speed_at = |life_fraction: f32| {
            let mut d = deerclops(200);
            d.life = (d.life_max as f32 * life_fraction) as i32;
            let t = Some(player_at(200.0 * TILE + 2000.0, 299.0 * TILE));
            for _ in 0..60 {
                d.ai[0] = DEER_STALKING;
                d.ai[1] = 0.0;
                update(&mut d, &tundra(&tiles, t), &mut rng());
            }
            d.velocity.0
        };
        assert!(speed_at(1.0) > 0.0, "it should be coming at you");
        assert!(
            speed_at(0.05) > speed_at(1.0),
            "and faster as it is worn down"
        );
    }

    #[test]
    fn up_close_it_throws_a_wall_of_spikes_forward() {
        let tiles = snowfield();
        let mut d = deerclops(200);
        d.ai[0] = DEER_SPIKES_FORWARD;
        d.direction = 1;
        let t = Some(player_at(200.0 * TILE + 60.0, 299.0 * TILE));
        let mut thrown = Vec::new();
        for _ in 0..(DEER_SPIKES_FORWARD_TICKS as i32) {
            thrown.extend(update(&mut d, &tundra(&tiles, t), &mut rng()).shots);
        }
        assert!(!thrown.is_empty(), "should have raised spikes");
        assert!(thrown.iter().all(|s| s.projectile == DEER_SPIKE));
        // A spike is launched with a *facing*, not a speed: `new Vector2(0f, -1f).RotatedBy(...)`
        // (`NPC.cs:45054`) is a unit vector, and `aiStyle 157` never touches it again, so the
        // spike creeps a pixel a tick for its twenty and stands where it grew. Anything larger
        // here would send a wall of ice across the arena.
        assert!(
            thrown
                .iter()
                .all(|s| (s.velocity.0.hypot(s.velocity.1) - 1.0).abs() < 1e-4),
            "each spike's launch vector should be unit length"
        );
        // Zero means the type's own; the arm ends it at twenty long before that matters, and the
        // 300 that used to sit here was invented.
        assert!(thrown.iter().all(|s| s.time_left == 0));
        assert!(
            thrown.iter().all(|s| s.position.0 >= d.center().0 - 64.0),
            "all out in front of it"
        );
        assert_eq!(d.ai[0], DEER_STALKING, "and then back to stalking");
    }

    /// Standing behind it is answered by the both-sides version, which is the point of it.
    #[test]
    fn the_second_spike_attack_covers_both_sides() {
        let tiles = snowfield();
        let mut d = deerclops(200);
        d.ai[0] = DEER_SPIKES_BOTH;
        d.direction = 1;
        let t = Some(player_at(200.0 * TILE - 60.0, 299.0 * TILE));
        let mut thrown = Vec::new();
        for _ in 0..(DEER_SPIKES_BOTH_TICKS as i32) {
            thrown.extend(update(&mut d, &tundra(&tiles, t), &mut rng()).shots);
        }
        let here = d.center().0;
        assert!(thrown.iter().any(|s| s.position.0 > here));
        assert!(thrown.iter().any(|s| s.position.0 < here));
    }

    /// DEER-2: the slam throws rubble UP out of the ground in a fan, one chunk a tick, and each
    /// arcs back down (proj 962 is a fly-flat-then-fall arc). The old code dropped a diagonal line
    /// of rubble downward from above; the `velocity.1 > 0` it asserted is exactly what this now
    /// forbids.
    #[test]
    fn the_slam_throws_rubble_up_out_of_the_ground() {
        let tiles = snowfield();
        let mut d = deerclops(200);
        d.ai[0] = DEER_RUBBLE_SLAM;
        d.direction = 1;
        let t = Some(player_at(200.0 * TILE + 400.0, 299.0 * TILE));
        let mut thrown = Vec::new();
        for _ in 0..(DEER_RUBBLE_TICKS as i32) {
            thrown.extend(update(&mut d, &tundra(&tiles, t), &mut rng()).shots);
        }
        assert!(!thrown.is_empty(), "the slam should throw rubble");
        assert!(thrown.iter().all(|s| s.projectile == DEER_RUBBLE));
        // Every chunk launches upward, and out of the ground surface (tile y=300 here), not from
        // ten tiles above Deerclops.
        assert!(
            thrown.iter().all(|s| s.velocity.1 < 0.0),
            "it is thrown up, not dropped down"
        );
        assert!(
            thrown.iter().all(|s| s.position.1 >= 299.0 * TILE),
            "and out of the ground, not the air above"
        );
        // The fan spreads: not every chunk shares the leftmost launch angle.
        let spread = thrown.iter().map(|s| s.velocity.0).fold(f32::MIN, f32::max)
            - thrown.iter().map(|s| s.velocity.0).fold(f32::MAX, f32::min);
        assert!(spread > 0.5, "the chunks fan out, got spread {spread}");
    }

    /// Where `RandomizeInsanityShadowFor` puts a hand and how fast it leaves, given the band it
    /// rolled.
    ///
    /// Three of the four routines place it exactly on the two-hundred-pixel ring and start it at
    /// four; the arc is the exception on both counts, because vanilla walks it backwards from
    /// where the target will be in a second, sixty eight-pixel steps at a time - so it starts
    /// anywhere up to that path length away, moving at the eight those steps are long.
    ///
    /// The speed is checked as well as the place because the drift's is *derived*
    /// (`num3 / (num4 + 10)`, and `num4` is the one local that gets a `+= 10f` on this arm alone),
    /// so it is the only one of the four that a wrong constant can move without moving anything
    /// this can otherwise see.
    fn placed_right(shot: &Shot, around: (f32, f32)) -> bool {
        let away = (shot.position.0 - around.0).hypot(shot.position.1 - around.1);
        let speed = shot.velocity.0.hypot(shot.velocity.1);
        match shot.ai[0] {
            b if b == 0.0 || b == 180.0 || b == 300.0 => {
                (away - DEER_PASSIVE_SHADOW_RING).abs() < 1.0 && (speed - 4.0).abs() < 0.01
            }
            390.0 => away <= 60.0 * 8.0 + 1.0 && (speed - 8.0).abs() < 0.01,
            _ => false,
        }
    }

    #[test]
    fn the_shadow_hands_come_out_of_the_dark_around_you() {
        let tiles = snowfield();
        let mut d = deerclops(200);
        d.ai[0] = DEER_SHADOW_HANDS;
        let player = player_at(200.0 * TILE + 400.0, 299.0 * TILE);
        let t = Some(player);
        let mut thrown = Vec::new();
        for _ in 0..(DEER_SHADOW_TICKS as i32) {
            thrown.extend(update(&mut d, &tundra(&tiles, t), &mut rng()).shots);
        }
        assert_eq!(thrown.len(), DEER_SHADOW_HANDS_COUNT);
        assert!(thrown.iter().all(|s| s.projectile == DEER_SHADOW_HAND));
        // Each hand is placed by the routine it rolled, not on one ring: this used to assert a
        // flat 250-to-600 band, which is what our own invented placement produced and none of
        // vanilla's four.
        assert!(
            thrown.iter().all(|s| placed_right(s, player.center)),
            "each hand should be placed the way its own band says: {:?}",
            thrown
                .iter()
                .map(|s| (s.ai[0], s.position))
                .collect::<Vec<_>>()
        );
    }

    /// Every one of style 187's four routines really is rolled, and the arm knows all four bands.
    ///
    /// `RandomizeInsanityShadowFor` picks `Main.rand.Next(4)` and writes the band into `ai[0]`
    /// (`Projectile.cs:43186`, `:43211`/`:43219`/`:43226`/`:43245`). A hand with no band drifts,
    /// which is what all six of a wave did before `Shot` could carry one.
    #[test]
    fn a_wave_of_hands_is_a_mix_of_all_four_routines() {
        let tiles = snowfield();
        let player = player_at(200.0 * TILE + 400.0, 299.0 * TILE);
        let t = Some(player);
        let mut bands = std::collections::HashSet::new();
        // Six hands a wave is not enough to see all four reliably; several waves is.
        for seed in 0..12u64 {
            let mut d = deerclops(200);
            d.ai[0] = DEER_SHADOW_HANDS;
            let mut r = SmallRng::seed_from_u64(seed);
            for _ in 0..(DEER_SHADOW_TICKS as i32) {
                for s in update(&mut d, &tundra(&tiles, t), &mut r).shots {
                    assert!(placed_right(&s, player.center), "band {}", s.ai[0]);
                    bands.insert(s.ai[0].to_bits());
                }
            }
        }
        let mut seen: Vec<f32> = bands.into_iter().map(f32::from_bits).collect();
        seen.sort_by(f32::total_cmp);
        assert_eq!(seen, vec![0.0, 180.0, 300.0, 390.0]);
    }

    /// DEER-1: in Expert Mode a passive rain of shadow hands runs throughout the fight, apart from
    /// the dedicated shadow-hands attack. Counted by their softer damage so a dedicated volley
    /// cannot be mistaken for them. A Classic world has none.
    #[test]
    fn expert_deerclops_rains_passive_shadow_hands() {
        let tiles = snowfield();
        let passive = |expert: bool| {
            let mut d = deerclops(200);
            let t = Some(player_at(200.0 * TILE + 400.0, 299.0 * TILE));
            let mut world = tundra(&tiles, t);
            world.conditions.expert = expert;
            let mut count = 0;
            for _ in 0..300 {
                count += update(&mut d, &world, &mut rng())
                    .shots
                    .iter()
                    .filter(|s| {
                        s.projectile == DEER_SHADOW_HAND && s.damage == DEER_SHADOW_DAMAGE_PASSIVE
                    })
                    .count();
            }
            count
        };
        assert!(passive(true) > 0, "Expert rains passive shadow hands");
        assert_eq!(passive(false), 0, "Classic does not");
    }

    /// BS3-M4: one hand per three-wave cycle for any given player, and none at all past 1200 pixels.
    ///
    /// `Boss_CanShootExtraAt` (`NPC.cs:47474-47494`) takes the wave's index modulo three and only
    /// raises a hand for the players whose slot matches, then refuses outright past its scan
    /// distance. Every wave used to fire, from any distance, so a lone player took three times the
    /// passive hands and could not walk out from under them. The ring is 200 pixels, the radius
    /// `RandomizeInsanityShadowFor` places a hostile hand at (`Projectile.cs:43187`), not the
    /// 300-to-500 spread this used.
    #[test]
    fn the_passive_rain_picks_one_wave_in_three_and_stops_at_range() {
        let tiles = snowfield();
        let hands = |gap: f32| {
            let mut d = deerclops(200);
            let (cx, _) = d.center();
            let at = (cx + gap, 299.0 * TILE);
            let t = Some(player_at(at.0, at.1));
            let mut world = tundra(&tiles, t);
            world.conditions.expert = true;
            let mut out = Vec::new();
            // A full cycle is three waves of eighty ticks at full health.
            for _ in 0..(DEER_PASSIVE_SHADOW_SLOW * DEER_PASSIVE_SHADOW_WAVES) as i32 {
                out.extend(update(&mut d, &world, &mut rng()).shots.into_iter().filter(
                    |s: &Shot| {
                        s.projectile == DEER_SHADOW_HAND && s.damage == DEER_SHADOW_DAMAGE_PASSIVE
                    },
                ));
            }
            (out, at)
        };

        let (near, at) = hands(400.0);
        assert_eq!(near.len(), 1, "one hand a cycle, not three");
        assert!(
            placed_right(&near[0], at),
            "it comes up where its own routine puts it: band {} at {:?}, player at {at:?}",
            near[0].ai[0],
            near[0].position
        );

        assert!(
            hands(DEER_PASSIVE_SHADOW_RANGE + 200.0).0.is_empty(),
            "and not at all once the player is out of scan range"
        );
    }

    #[test]
    fn the_roar_is_reported_so_the_caller_can_apply_it() {
        let tiles = snowfield();
        let mut d = deerclops(200);
        d.ai[0] = DEER_ROAR;
        let t = Some(player_at(200.0 * TILE + 400.0, 299.0 * TILE));
        let mut roared = false;
        for _ in 0..(DEER_ROAR_TICKS as i32) {
            roared |= update(&mut d, &tundra(&tiles, t), &mut rng()).roared;
        }
        assert!(roared);
    }

    /// DEER-3: it will not pick the roar again while the Slow the last one applied is still up. This
    /// stands in for vanilla's `flag13` gate (the target must not already carry the Slow buff,
    /// `NPC.cs:44653-44654`), which the server cannot read directly. The old gate re-picked the roar
    /// every roar-timer, keeping the player permanently Slowed.
    #[test]
    fn it_will_not_re_roar_while_its_slow_is_still_up() {
        let tiles = snowfield();
        let mut d = deerclops(200);
        let (cx, _) = d.center();
        let t = Some(player_at(cx + 400.0, 299.0 * TILE));

        // Put it exactly at the roar-selection point: stalking, roar timer met but short of the
        // rubble timer, moving (so the shadow attack is not the pick), grounded, well out of spike
        // range.
        let arm = |d: &mut Npc| {
            d.ai[0] = DEER_STALKING;
            d.ai[1] = DEER_UNTIL_ROAR + 5.0;
            d.velocity = (2.0, 0.0);
            d.local_ai[1] = 0.0;
        };

        arm(&mut d);
        d.local_ai[0] = 0.0;
        update(&mut d, &tundra(&tiles, t), &mut rng());
        assert_eq!(
            d.ai[0], DEER_ROAR,
            "it roars when the player is not yet Slowed"
        );

        // Re-armed while the cooldown from that roar is still up: it must pick something else.
        arm(&mut d);
        update(&mut d, &tundra(&tiles, t), &mut rng());
        assert_ne!(
            d.ai[0], DEER_ROAR,
            "it will not re-roar while the Slow is still up"
        );

        // Once the cooldown has lapsed it can roar again.
        d.local_ai[0] = 0.0;
        arm(&mut d);
        update(&mut d, &tundra(&tiles, t), &mut rng());
        assert_eq!(
            d.ai[0], DEER_ROAR,
            "and roars again once the Slow has lapsed"
        );
    }

    #[test]
    fn out_of_the_snow_it_goes_home() {
        let tiles = snowfield();
        let mut d = deerclops(200);
        let t = Some(player_at(200.0 * TILE + 800.0, 299.0 * TILE));
        update(&mut d, &tundra(&tiles, t), &mut rng());
        let mut warm = tundra(&tiles, t);
        warm.conditions.snow = false;
        // Chasing keeps it going, but the moment it checks as not chasing it turns back.
        d.ai[0] = DEER_GOING_HOME;
        update(&mut d, &warm, &mut rng());
        assert_eq!(d.ai[0], DEER_GOING_HOME, "it should stay on its way home");
    }

    #[test]
    fn far_enough_away_and_it_cannot_be_hurt() {
        let tiles = snowfield();
        let mut d = deerclops(200);
        let far = Some(player_at(
            200.0 * TILE + DEER_SHIELD_RANGE + 200.0,
            299.0 * TILE,
        ));
        for _ in 0..(DEER_SHIELD_AFTER as i32 + 2) {
            update(&mut d, &tundra(&tiles, far), &mut rng());
        }
        assert!(d.invulnerable, "no killing it from range");
        assert!(
            !d.take_damage(500, 0.0, 1) && d.life == d.life_max,
            "and a hit through `strike` really is refused"
        );

        let close = Some(player_at(200.0 * TILE + 50.0, 299.0 * TILE));
        for _ in 0..(DEER_SHIELD_AFTER as i32 + 2) {
            update(&mut d, &tundra(&tiles, close), &mut rng());
        }
        assert!(!d.invulnerable, "get close and it is fair game");
        d.take_damage(500, 0.0, 1);
        assert!(d.life < d.life_max, "and the hit lands");
    }

    #[test]
    fn a_long_walk_home_ends_in_a_teleport() {
        let tiles = snowfield();
        let mut d = deerclops(200);
        let t = Some(player_at(200.0 * TILE + 8000.0, 299.0 * TILE));
        update(&mut d, &tundra(&tiles, t), &mut rng());
        // Well away from its den, and out of patience.
        d.position.0 = 200.0 * TILE + 4000.0;
        d.ai[0] = DEER_GOING_HOME;
        d.ai[1] = DEER_PATIENCE;
        update(&mut d, &tundra(&tiles, t), &mut rng());
        assert_eq!(d.ai[0], DEER_TELEPORTING);

        for _ in 0..(DEER_TELEPORT_AT as i32 + 1) {
            update(&mut d, &tundra(&tiles, t), &mut rng());
        }
        assert!(
            (d.center().0 - 200.0 * TILE).abs() < 100.0,
            "it should be back at its den, at {}",
            d.center().0
        );
    }
}
