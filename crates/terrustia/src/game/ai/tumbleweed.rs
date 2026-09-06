//! Style 26 - `AI_026_Unicorns` (`NPC.cs:63012-63573`).
//!
//! Seven types share it, and only one of them is a tumbleweed: Unicorn (86), Wolf (155), Headless
//! Horseman (315), Hellhound (329), Stardust Spider Small (410), Nebula Beast (423) and Tumbleweed
//! (546). It is a ground charger. There is no steering in it beyond picking a side: it accelerates
//! to a top speed, hops what it can and leaps gaps, and bounces along until something stops it.
//!
//! The anti-stall is the other half. `ai[3]` counts up whenever the creature makes no headway,
//! pinned against a wall or running against its own facing, and once it has spent half a second
//! stuck it stops re-targeting and simply reverses whenever it comes to a complete stop.
//!
//! What separates the seven is a small table of numbers and one attack each. A tumbleweed is slow
//! (4 pixels a tick), rides the wind in a sandstorm, and shoulders its own kind aside; a Nebula
//! Beast is the fastest thing here at 10 and stops to fire; a Headless Horseman throws a pumpkin
//! every eight seconds; a Stardust Spider is a walking bomb.

use terrustia_proto::npc_params::{
    TUMBLEWEED_ACCEL, TUMBLEWEED_JUMPS, TUMBLEWEED_LEAP, TUMBLEWEED_PATIENCE,
    TUMBLEWEED_PATIENCE_CAP, TUMBLEWEED_SPEED, TUMBLEWEED_STEP, TUMBLEWEED_WIND,
};
use terrustia_proto::tile_solid::{solid, solid_top};

use super::{Shot, World};
use crate::game::npc::{Npc, TILE, TileView};
use rand::Rng;
use rand::rngs::SmallRng;

/// How fast it has to be rolling before it will leap a gap (vanilla's `num25`).
const LEAP_SPEED: f32 = 3.0;

/// How long a projectile this style throws stays alive, matching the other ported routines.
const SHOT_LIFETIME: u16 = 300;

/// The types, by the `NPCID` names the game gives them. The Unicorn (86) appears in none of the
/// game's per-type branches, so it takes every default and is named only where the tests walk the
/// whole roster.
#[cfg(test)]
const UNICORN: u16 = 86;
const WOLF: u16 = 155;
const HEADLESS_HORSEMAN: u16 = 315;
const HELLHOUND: u16 = 329;
const STARDUST_SPIDER: u16 = 410;
const NEBULA_BEAST: u16 = 423;
const TUMBLEWEED: u16 = 546;

/// What a tick of this routine did beyond moving its NPC.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub shots: Vec<Shot>,
    /// Set when a Stardust Spider went off, which is the only way that type ever dies of its own
    /// accord (`NPC.cs:63133-63142`).
    pub died: bool,
}

/// Top speed and acceleration by type: vanilla's `num11` and `num12` (`NPC.cs:63258-63365`).
///
/// The tumbleweed's top speed is not constant, so it is handled at the call site; everything else
/// comes straight off the table. Note the Unicorn appears in none of the game's per-type branches,
/// so it takes the defaults and has no drag term at all.
fn gait(npc_type: u16) -> (f32, f32) {
    match npc_type {
        STARDUST_SPIDER => (6.0, 0.2),
        NEBULA_BEAST => (10.0, 0.2),
        TUMBLEWEED => (TUMBLEWEED_SPEED, TUMBLEWEED_ACCEL),
        _ => (6.0, 0.07),
    }
}

/// How hard it brakes when it is moving against its own facing (`NPC.cs:63269-63356`).
///
/// `None` for the Unicorn, which the game never gives a drag branch, so it coasts.
fn drag(npc_type: u16) -> Option<f32> {
    match npc_type {
        WOLF | HEADLESS_HORSEMAN => Some(0.95),
        HELLHOUND | STARDUST_SPIDER => Some(0.9),
        NEBULA_BEAST => Some(0.85),
        TUMBLEWEED => Some(0.92),
        _ => None,
    }
}

/// The stuck counter's ceiling, `num * num2` (`NPC.cs:63014-63016`, `:63096`).
///
/// `num2` is 10 for everything but the tumbleweed, which the game drops to 4, so a tumbleweed gives
/// up on being stuck two and a half times sooner than a wolf does.
fn patience_cap(npc_type: u16) -> f32 {
    if npc_type == TUMBLEWEED {
        TUMBLEWEED_PATIENCE_CAP
    } else {
        TUMBLEWEED_PATIENCE * 10.0
    }
}

fn blocking(tiles: &impl TileView, x: i32, y: i32) -> bool {
    let t = tiles.tile(x, y);
    t.is_active() && solid(t.block) && !solid_top(t.block)
}

/// Whether this one is closing on its target rather than running away from it.
fn closing(npc: &Npc, target_x: f32) -> bool {
    let cx = npc.center().0;
    (cx < target_x && npc.velocity.0 > 0.0) || (cx > target_x && npc.velocity.0 < 0.0)
}

/// Drive one style-26 creature for a tick.
///
/// `in_a_sandstorm` says whether its target is standing in one, which is the only thing that makes
/// the wind count; `crowding` keeps a drift of tumbleweeds from stacking into one another.
pub fn update<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    in_a_sandstorm: bool,
    rng: &mut SmallRng,
) -> Outcome {
    let mut out = Outcome::default();
    let kind = npc.npc_type;

    // `flag2`: running against its own facing on the ground. `NPC.cs:63019-63023`.
    let mut stalled = false;
    if npc.velocity.1 == 0.0
        && ((npc.velocity.0 > 0.0 && npc.direction < 0)
            || (npc.velocity.0 < 0.0 && npc.direction > 0))
    {
        stalled = true;
        npc.ai[3] += 1.0;
    }

    // `NPC.cs:63024-63052`. Only the tumbleweed shoulders its neighbours aside, and the shove is
    // never allowed to lift a grounded one off the floor (the game's own `flag4`).
    if kind == TUMBLEWEED {
        let grounded = npc.velocity.1 == 0.0;
        npc.velocity.0 += world.crowding.0 * 0.05;
        npc.velocity.1 += world.crowding.1 * 0.05;
        if grounded {
            npc.velocity.1 = 0.0;
        }
    }

    // `NPC.cs:63053-63067`: the Headless Horseman lobs a pumpkin every eight seconds, scattered
    // around itself and thrown along its own travel with a random rise or fall.
    if kind == HEADLESS_HORSEMAN {
        npc.local_ai[0] += 1.0;
        if npc.local_ai[0] > 480.0 {
            npc.local_ai[0] = 0.0;
            // The game's `num4 != 255`: it needs somebody to throw at.
            if world.target.is_some() {
                let (cx, cy) = npc.center();
                // `Main.rand.NextVector2Circular(40f, 40f)`.
                let angle = rng.random::<f32>() * std::f32::consts::TAU;
                let radius = rng.random::<f32>().sqrt() * 40.0;
                out.shots.push(Shot {
                    projectile: 1001,
                    damage: if world.conditions.expert { 30 } else { 40 },
                    position: (cx + angle.cos() * radius, cy + angle.sin() * radius),
                    // `Main.rand.NextFloatDirection()` is a float in [-1, 1).
                    velocity: (npc.velocity.0, (rng.random::<f32>() * 2.0 - 1.0) * 3.0),
                    time_left: SHOT_LIFETIME,
                    ai: [0.0; 3],
                });
            }
        }
    }

    // `NPC.cs:63088-63105`. `flag3`: it is properly stuck, not merely facing the wrong way.
    let mut stuck = false;
    if npc.position.0 == npc.old_position.0 || npc.ai[3] >= TUMBLEWEED_PATIENCE || stalled {
        npc.ai[3] += 1.0;
        stuck = true;
    } else if npc.ai[3] > 0.0 {
        npc.ai[3] -= 1.0;
    }
    if npc.ai[3] > patience_cap(kind) {
        npc.ai[3] = 0.0;
    }
    if npc.was_hurt {
        npc.ai[3] = 0.0;
    }

    // `NPC.cs:63107-63116`. The gap is measured to the player's *top*, not their centre.
    let gap = world.target.map_or(f32::INFINITY, |t| {
        let (cx, cy) = npc.center();
        ((t.center.0 - cx).powi(2) + (t.center.1 - super::PLAYER_HEIGHT as f32 / 2.0 - cy).powi(2))
            .sqrt()
    });
    if gap < 200.0 && !stuck {
        npc.ai[3] = 0.0;
    }

    // The per-type attack, `NPC.cs:63118-63216`. Exactly one arm runs. `charging` is the game's
    // `flag`: on the tick a Nebula Beast plants its feet it does not accelerate at all.
    let mut charging = false;
    match kind {
        STARDUST_SPIDER => {
            // `NPC.cs:63118-63143`. A walking bomb: it goes off after four seconds, or at once if
            // anyone is directly overhead within a screen.
            npc.ai[1] += 1.0;
            let overhead = world.target.is_some_and(|t| {
                let (cx, cy) = npc.center();
                t.alive
                    && ((t.center.0 - cx).powi(2) + (t.center.1 - cy).powi(2)).sqrt() < 800.0
                    && t.center.1 < cy
                    && (t.center.0 - cx).abs() < 20.0
            });
            if npc.ai[1] >= 240.0 || (npc.velocity.1 == 0.0 && overhead) {
                let (cx, cy) = npc.center();
                for _ in 0..3 {
                    out.shots.push(Shot {
                        projectile: 538,
                        damage: 50,
                        position: (cx, cy),
                        velocity: (
                            (rng.random::<f32>() - 0.5) * 2.0,
                            -4.0 - 10.0 * rng.random::<f32>(),
                        ),
                        time_left: SHOT_LIFETIME,
                        ai: [0.0; 3],
                    });
                }
                out.died = true;
                npc.dirty = true;
                // The game returns straight out of the routine here; nothing else runs.
                return out;
            }
        }
        NEBULA_BEAST => {
            // `NPC.cs:63145-63204`. It plants itself, spends half a second winding up, spits a
            // bolt, and then will not do it again for another five to ten seconds.
            if npc.ai[2] == 1.0 {
                npc.ai[1] += 1.0;
                npc.velocity.0 *= 0.7;
                if npc.velocity.0 > -0.5 && npc.velocity.0 < 0.5 {
                    npc.velocity.0 = 0.0;
                }
                if npc.ai[1] == 30.0 {
                    let (cx, cy) = npc.center();
                    let away = f32::from(-npc.sprite_direction);
                    out.shots.push(Shot {
                        projectile: 575,
                        damage: if world.conditions.expert { 35 } else { 50 },
                        position: (cx + away * 20.0, cy),
                        velocity: (away * 7.0, 0.0),
                        time_left: SHOT_LIFETIME,
                        ai: [0.0; 3],
                    });
                }
                if npc.ai[1] >= 60.0 {
                    npc.ai[1] = -(rng.random_range(320..601) as f32);
                    npc.ai[2] = 0.0;
                    npc.dirty = true;
                }
            } else {
                npc.ai[1] += 1.0;
                if npc.ai[1] >= 180.0 && gap < 500.0 && npc.velocity.1 == 0.0 {
                    charging = true;
                    npc.ai[1] = 0.0;
                    npc.ai[2] = 1.0;
                    npc.dirty = true;
                } else if npc.velocity.1 == 0.0
                    && gap < 100.0
                    && npc.velocity.0.abs() > LEAP_SPEED
                    && world.target.is_some_and(|t| closing(npc, t.center.0))
                {
                    npc.velocity.1 -= 4.0;
                }
            }
        }
        // `NPC.cs:63206-63218`: a pounce. The wolf and the hellhound need to be close for it; the
        // tumbleweed hops at any range.
        WOLF | HELLHOUND | TUMBLEWEED
            if npc.velocity.1 == 0.0
                && (kind == TUMBLEWEED || gap < 100.0)
                && npc.velocity.0.abs() > LEAP_SPEED
                && world.target.is_some_and(|t| closing(npc, t.center.0)) =>
        {
            npc.velocity.1 -= 4.0;
        }
        _ => {}
    }

    // `NPC.cs:63219-63228`. This is gated on the TYPE, not merely on the biome: only a tumbleweed
    // leaves for want of a desert. Dropping the type test made a Wolf, a Unicorn, a Hellhound, a
    // Headless Horseman, a Nebula Beast and a Stardust Spider set `time_left` to 10 on their first
    // tick anywhere but a desert, and be reaped ten ticks later.
    if kind == TUMBLEWEED
        && !in_a_sandstorm
        && world.target.is_some_and(|_| !world.conditions.desert)
    {
        npc.time_left = npc.time_left.min(10);
        npc.ai[3] = TUMBLEWEED_PATIENCE;
    }

    // `NPC.cs:63230-63245`.
    if npc.ai[3] < TUMBLEWEED_PATIENCE {
        if matches!(kind, HELLHOUND | HEADLESS_HORSEMAN) && !world.conditions.pumpkin_moon {
            // Both are Pumpkin Moon spawns; out of the event they go home.
            npc.time_left = npc.time_left.min(10);
        } else if let Some(t) = world.target {
            npc.direction = if t.center.0 > npc.center().0 { 1 } else { -1 };
        }
    } else {
        // Given up on steering: it reverses whenever it comes to a complete stop.
        if npc.velocity.0 == 0.0 {
            if npc.velocity.1 == 0.0 {
                npc.ai[0] += 1.0;
                if npc.ai[0] >= 2.0 {
                    npc.direction = -npc.direction;
                    npc.sprite_direction = npc.direction;
                    npc.ai[0] = 0.0;
                }
            }
        } else {
            npc.ai[0] = 0.0;
        }
        npc.direction_y = -1;
        if npc.direction == 0 {
            npc.direction = 1;
        }
    }

    // Rolling, `NPC.cs:63258-63385`. The tumbleweed's top speed rides the wind, which is what makes
    // a sandstorm sweep a whole drift of them one way.
    let (mut top, accel) = gait(kind);
    if kind == TUMBLEWEED && in_a_sandstorm {
        let strength = (0.6 + 0.4 * world.conditions.wind.abs()) * world.conditions.wind.signum();
        top += strength * f32::from(npc.direction) * TUMBLEWEED_WIND;
    }
    let footing = npc.velocity.1 == 0.0
        || world.wet
        || (npc.velocity.0 <= 0.0 && npc.direction < 0)
        || (npc.velocity.0 >= 0.0 && npc.direction > 0);
    if !charging && footing {
        if let Some(brake) = drag(kind)
            && npc.velocity.0.signum() as i32 != i32::from(npc.direction)
        {
            npc.velocity.0 *= brake;
        }
        // `NPC.cs:63285-63294`: a Hellhound gets a second, faster acceleration of its own up to 3
        // and then falls through to the shared one below, so it picks up 0.17 a tick from a
        // standing start. The Headless Horseman's own branch is the shared block written out
        // again, which likewise doubles its acceleration. Both are the game's own shape.
        if kind == HELLHOUND {
            if npc.direction > 0 && npc.velocity.0 < 3.0 {
                npc.velocity.0 += 0.1;
            }
            if npc.direction < 0 && npc.velocity.0 > -3.0 {
                npc.velocity.0 -= 0.1;
            }
        }
        if kind == HEADLESS_HORSEMAN {
            accelerate(npc, top, accel);
        }
        accelerate(npc, top, accel);
    }

    // Roll up a low step rather than jumping it. `NPC.cs:63386-63453`.
    if npc.velocity.1 >= 0.0 {
        let ahead = npc.velocity.0.signum() as i32;
        let next_x = npc.position.0 + npc.velocity.0;
        let probe_x =
            ((next_x + npc.width() / 2.0 + (npc.width() / 2.0 + 1.0) * ahead as f32) / TILE) as i32;
        let foot_y = ((npc.position.1 + npc.velocity.1 + npc.height() - 1.0) / TILE) as i32;
        if blocking(world.tiles, probe_x, foot_y)
            && !blocking(world.tiles, probe_x, foot_y - 1)
            && !blocking(world.tiles, probe_x, foot_y - 2)
            && !blocking(world.tiles, probe_x, foot_y - 3)
        {
            let step_top = (foot_y * 16) as f32;
            let rise = npc.position.1 + npc.height() - step_top;
            if rise > 0.0 && rise <= TUMBLEWEED_STEP {
                npc.position.1 = step_top - npc.height();
                npc.dirty = true;
            }
        }
    }

    // On the ground with headroom: jump whatever is in the way, sized to how tall it is.
    // `NPC.cs:63455-63548`. The game tests `spriteDirection` here (negated for 410, 423 and 546);
    // this server does not keep a sprite direction for the four types that never flip, so the
    // travel direction stands in for it, which is the same value for all seven in practice.
    if npc.velocity.1 == 0.0 {
        let head_y = ((npc.position.1 - 7.0) / TILE) as i32;
        let clear_above = ((npc.position.0 - 7.0) / TILE) as i32
            ..=((npc.position.0 + npc.width() + 7.0) / TILE) as i32;
        let headroom = !clear_above
            .clone()
            .any(|x| blocking(world.tiles, x, head_y));
        if headroom {
            let probe_x = ((npc.position.0
                + npc.width() / 2.0
                + (npc.width() / 2.0 + 2.0) * f32::from(npc.direction)
                + npc.velocity.0 * 5.0)
                / TILE) as i32;
            let probe_y = ((npc.position.1 + npc.height() - 15.0) / TILE) as i32;
            let heading = (npc.velocity.0 < 0.0 && npc.direction == -1)
                || (npc.velocity.0 > 0.0 && npc.direction == 1);
            // `flag7`: the two lunar walkers refuse to leap over a hole they can see a floor in.
            let picky = matches!(kind, STARDUST_SPIDER | NEBULA_BEAST);
            if heading {
                if blocking(world.tiles, probe_x, probe_y - 2) {
                    npc.velocity.1 = if blocking(world.tiles, probe_x, probe_y - 3) {
                        TUMBLEWEED_JUMPS[0]
                    } else {
                        TUMBLEWEED_JUMPS[1]
                    };
                    npc.dirty = true;
                } else if blocking(world.tiles, probe_x, probe_y - 1) {
                    npc.velocity.1 = TUMBLEWEED_JUMPS[2];
                    npc.dirty = true;
                } else if npc.position.1 + npc.height() - (probe_y * 16) as f32 > 20.0
                    && blocking(world.tiles, probe_x, probe_y)
                {
                    npc.velocity.1 = TUMBLEWEED_JUMPS[3];
                    npc.dirty = true;
                } else if (npc.direction_y < 0 || npc.velocity.0.abs() > LEAP_SPEED)
                    && !(picky && blocking(world.tiles, probe_x, probe_y + 1))
                    && !blocking(world.tiles, probe_x, probe_y + 2)
                    && !blocking(world.tiles, probe_x + i32::from(npc.direction), probe_y + 3)
                {
                    // A gap ahead, and enough speed to clear it.
                    npc.velocity.1 = TUMBLEWEED_LEAP;
                    npc.dirty = true;
                }
            }
        }
    }

    // `NPC.cs:63566-63570`: only the tumbleweed rolls, and only it draws back to front.
    if kind == TUMBLEWEED {
        npc.rotation += npc.velocity.0 * 0.05;
        npc.sprite_direction = -npc.direction;
    }
    npc.dirty = true;
    out
}

/// The shared acceleration block, `NPC.cs:63365-63384`.
fn accelerate(npc: &mut Npc, top: f32, accel: f32) {
    if npc.velocity.0 < -top || npc.velocity.0 > top {
        if npc.velocity.1 == 0.0 {
            npc.velocity.0 *= 0.8;
            npc.velocity.1 *= 0.8;
        }
    } else if npc.velocity.0 < top && npc.direction == 1 {
        npc.velocity.0 = (npc.velocity.0 + accel).min(top);
    } else if npc.velocity.0 > -top && npc.direction == -1 {
        npc.velocity.0 = (npc.velocity.0 - accel).max(-top);
    }
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
    struct Dunes(HashMap<(i32, i32), Tile>);

    impl TileView for Dunes {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(11)
    }

    fn sand() -> Dunes {
        let mut d = Dunes::default();
        for x in 0..4000 {
            for y in 300..320 {
                d.0.insert((x, y), Tile::block(53));
            }
        }
        d
    }

    fn walker(npc_type: u16, tile_x: i32) -> Npc {
        let mut n = Npc::new(npc_type, (0.0, 0.0), 1).expect("style 26 type");
        n.position = (tile_x as f32 * TILE, 300.0 * TILE - n.height());
        n.old_position = (n.position.0 - 1.0, n.position.1);
        n
    }

    fn weed(tile_x: i32) -> Npc {
        walker(TUMBLEWEED, tile_x)
    }

    fn desert<'a>(tiles: &'a Dunes, target: Option<Target>) -> World<'a, Dunes> {
        World {
            conditions: Conditions {
                desert: true,
                pumpkin_moon: true,
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
    fn a_tumbleweed_rolls_up_to_its_speed() {
        let tiles = sand();
        let mut w = weed(200);
        w.direction = 1;
        let (cx, cy) = w.center();
        let t = Some(player_at(cx + 600.0, cy));
        for _ in 0..400 {
            w.old_position = (w.position.0 - 1.0, w.position.1);
            update(&mut w, &desert(&tiles, t), true, &mut rng());
            w.velocity.1 = 0.0;
        }
        assert!(
            w.velocity.0 > 0.0,
            "should be rolling, got {}",
            w.velocity.0
        );
    }

    #[test]
    fn a_sandstorm_pushes_it_one_way_and_holds_it_the_other() {
        let tiles = sand();
        let mut downwind = weed(200);
        let mut upwind = weed(200);
        downwind.direction = 1;
        upwind.direction = -1;
        let mut w = desert(&tiles, None);
        w.conditions.wind = 1.0;
        for _ in 0..600 {
            downwind.old_position = (downwind.position.0 - 1.0, downwind.position.1);
            upwind.old_position = (upwind.position.0 - 1.0, upwind.position.1);
            downwind.ai[3] = TUMBLEWEED_PATIENCE;
            upwind.ai[3] = TUMBLEWEED_PATIENCE;
            update(&mut downwind, &w, true, &mut rng());
            update(&mut upwind, &w, true, &mut rng());
            downwind.velocity.1 = 0.0;
            upwind.velocity.1 = 0.0;
        }
        assert!(
            downwind.velocity.0.abs() > upwind.velocity.0.abs(),
            "downwind {} should outrun upwind {}",
            downwind.velocity.0,
            upwind.velocity.0
        );
    }

    #[test]
    fn a_tumbleweed_outside_the_desert_leaves() {
        let tiles = sand();
        let mut w = weed(200);
        let (cx, cy) = w.center();
        let mut out = desert(&tiles, Some(player_at(cx + 300.0, cy)));
        out.conditions.desert = false;
        update(&mut w, &out, false, &mut rng());
        assert!(w.time_left <= 10, "should be leaving, got {}", w.time_left);
    }

    /// The regression: `NPC.cs:63219` gates the leave on `type == 546`, and dropping that made
    /// every other style-26 type set `time_left` to 10 on its first tick anywhere but a desert.
    /// The whole style lived about ten ticks outside one biome.
    #[test]
    fn the_other_six_types_do_not_leave_for_want_of_a_desert() {
        let tiles = sand();
        for kind in [
            UNICORN,
            WOLF,
            HEADLESS_HORSEMAN,
            HELLHOUND,
            STARDUST_SPIDER,
            NEBULA_BEAST,
        ] {
            let mut n = walker(kind, 200);
            let (cx, cy) = n.center();
            let mut out = desert(&tiles, Some(player_at(cx + 300.0, cy)));
            out.conditions.desert = false;
            let before = n.time_left;
            update(&mut n, &out, false, &mut rng());
            assert_eq!(n.time_left, before, "type {kind} should be staying put");
        }
    }

    /// `NPC.cs:63232-63234`: both Pumpkin Moon walkers leave when the event is not running, and
    /// nothing else does.
    #[test]
    fn the_pumpkin_moon_pair_leave_when_the_moon_is_not_up() {
        let tiles = sand();
        for (kind, leaves) in [
            (HELLHOUND, true),
            (HEADLESS_HORSEMAN, true),
            (WOLF, false),
            (UNICORN, false),
        ] {
            let mut n = walker(kind, 200);
            let (cx, cy) = n.center();
            let mut out = desert(&tiles, Some(player_at(cx + 300.0, cy)));
            out.conditions.pumpkin_moon = false;
            update(&mut n, &out, false, &mut rng());
            assert_eq!(
                n.time_left <= 10,
                leaves,
                "type {kind} left={} expected {leaves}",
                n.time_left <= 10
            );
        }
    }

    /// The gaits, `NPC.cs:63258-63365`. A Nebula Beast outruns a Wolf outruns a tumbleweed.
    #[test]
    fn each_type_runs_at_its_own_top_speed() {
        let tiles = sand();
        let settle = |kind: u16| {
            let mut n = walker(kind, 200);
            n.direction = 1;
            n.ai[3] = TUMBLEWEED_PATIENCE;
            let mut w = desert(&tiles, None);
            w.conditions.pumpkin_moon = true;
            for _ in 0..600 {
                n.old_position = (n.position.0 - 1.0, n.position.1);
                // Keep the Nebula Beast out of its firing stance so this measures the run.
                n.ai[1] = 0.0;
                update(&mut n, &w, false, &mut rng());
                n.velocity.1 = 0.0;
            }
            n.velocity.0
        };
        let (weed, wolf, beast) = (settle(TUMBLEWEED), settle(WOLF), settle(NEBULA_BEAST));
        assert!(
            (weed - 4.0).abs() < 0.001,
            "tumbleweed tops out at 4: {weed}"
        );
        assert!((wolf - 6.0).abs() < 0.001, "a wolf at 6: {wolf}");
        assert!(
            (beast - 10.0).abs() < 0.001,
            "a nebula beast at 10: {beast}"
        );
    }

    /// `NPC.cs:63014-63016`, `:63096`: `num2` is 4 for a tumbleweed and 10 for the rest, so the
    /// stuck counter's ceiling is 120 against 300.
    #[test]
    fn a_wolf_stays_stuck_two_and_a_half_times_longer_than_a_tumbleweed() {
        assert_eq!(patience_cap(TUMBLEWEED), 120.0);
        assert_eq!(patience_cap(WOLF), 300.0);
        assert_eq!(patience_cap(UNICORN), 300.0);
    }

    #[test]
    fn a_tumbleweed_that_gets_nowhere_stops_chasing_and_reverses() {
        let tiles = sand();
        let mut w = weed(200);
        w.direction = 1;
        let (cx, cy) = w.center();
        // Someone to its right that it never makes progress toward.
        let t = Some(player_at(cx + 800.0, cy));
        for _ in 0..(TUMBLEWEED_PATIENCE as i32 + 2) {
            w.old_position = w.position;
            update(&mut w, &desert(&tiles, t), true, &mut rng());
            w.velocity = (0.0, 0.0);
        }
        assert!(
            w.ai[3] >= TUMBLEWEED_PATIENCE,
            "should have given up steering, got {}",
            w.ai[3]
        );
        // Standing still on the ground, it turns itself round.
        for _ in 0..3 {
            w.old_position = w.position;
            update(&mut w, &desert(&tiles, t), true, &mut rng());
            w.velocity = (0.0, 0.0);
        }
        assert_eq!(w.direction, -1);
    }

    /// It probes further ahead the faster it is rolling, and jumps higher the taller the thing it
    /// finds. A wall directly over its own head cancels the jump entirely, which is why the probe
    /// has to be out ahead of it for the tall jumps to be reachable at all.
    #[test]
    fn a_tumbleweed_jumps_a_wall_higher_the_taller_it_is() {
        let sample = weed(200);
        let speed = 4.0;
        let probe_x = ((sample.position.0
            + sample.width() / 2.0
            + (sample.width() / 2.0 + 2.0)
            + speed * 5.0)
            / TILE) as i32;
        let probe_y = ((sample.position.1 + sample.height() - 15.0) / TILE) as i32;

        let jump_over = |height: i32| {
            let mut w = weed(200);
            w.direction = 1;
            w.velocity = (speed, 0.0);
            let mut wall = sand();
            for up in 1..=height {
                wall.0.insert((probe_x, probe_y - up), Tile::block(1));
            }
            update(&mut w, &desert(&wall, None), true, &mut rng());
            w.velocity.1
        };

        let (short, tall, taller) = (jump_over(1), jump_over(2), jump_over(3));
        assert!(short < 0.0, "it should jump at all, got {short}");
        assert!(
            tall < short,
            "two tiles clears higher: {tall} against {short}"
        );
        assert!(
            taller < tall,
            "and three higher still: {taller} against {tall}"
        );
    }

    #[test]
    fn tumbleweeds_shoulder_each_other_apart() {
        let tiles = sand();
        let mut alone = weed(200);
        let mut crowded_weed = weed(200);
        alone.direction = 1;
        crowded_weed.direction = 1;
        let mut crowded = desert(&tiles, None);
        crowded.crowding = (-1.0, 0.0);
        update(&mut alone, &desert(&tiles, None), true, &mut rng());
        update(&mut crowded_weed, &crowded, true, &mut rng());
        assert!(
            crowded_weed.velocity.0 < alone.velocity.0,
            "a crowded one should be held back: {} against {}",
            crowded_weed.velocity.0,
            alone.velocity.0
        );
    }

    /// `NPC.cs:63024-63052`: nothing but a tumbleweed jostles.
    #[test]
    fn a_wolf_ignores_the_crowd() {
        let tiles = sand();
        let mut wolf = walker(WOLF, 200);
        wolf.direction = 1;
        let mut crowded = desert(&tiles, None);
        crowded.crowding = (-1.0, 0.0);
        let mut alone = walker(WOLF, 200);
        alone.direction = 1;
        update(&mut alone, &desert(&tiles, None), false, &mut rng());
        update(&mut wolf, &crowded, false, &mut rng());
        assert_eq!(wolf.velocity, alone.velocity);
    }

    /// `NPC.cs:63053-63067`: one pumpkin every 480 ticks, and none before.
    #[test]
    fn the_headless_horseman_throws_a_pumpkin_every_eight_seconds() {
        let tiles = sand();
        let mut h = walker(HEADLESS_HORSEMAN, 200);
        let (cx, cy) = h.center();
        let w = desert(&tiles, Some(player_at(cx + 300.0, cy)));
        let mut r = rng();
        let mut thrown = 0;
        for tick in 0..1000 {
            let out = update(&mut h, &w, false, &mut r);
            thrown += out.shots.len();
            assert!(
                out.shots.iter().all(|s| s.projectile == 1001),
                "only pumpkins"
            );
            if tick == 400 {
                assert_eq!(thrown, 0, "nothing thrown in the first four hundred ticks");
            }
            h.velocity.1 = 0.0;
        }
        assert_eq!(thrown, 2, "two in a thousand ticks");
    }

    /// `NPC.cs:63118-63143`: a Stardust Spider is a bomb on legs.
    #[test]
    fn a_stardust_spider_blows_itself_up() {
        let tiles = sand();
        let mut s = walker(STARDUST_SPIDER, 200);
        let (cx, cy) = s.center();
        let w = desert(&tiles, Some(player_at(cx + 3000.0, cy)));
        let mut r = rng();
        for _ in 0..239 {
            let out = update(&mut s, &w, false, &mut r);
            assert!(!out.died, "not yet");
            s.velocity.1 = 0.0;
        }
        let out = update(&mut s, &w, false, &mut r);
        assert!(out.died, "four seconds is its whole life");
        assert_eq!(out.shots.len(), 3);
        assert!(
            out.shots
                .iter()
                .all(|s| s.projectile == 538 && s.damage == 50)
        );
        assert!(
            out.shots.iter().all(|s| s.velocity.1 <= -4.0),
            "the spores go up"
        );
    }

    /// The same, but triggered early by somebody standing right on top of it.
    #[test]
    fn a_stardust_spider_goes_off_under_your_feet() {
        let tiles = sand();
        let mut s = walker(STARDUST_SPIDER, 200);
        let (cx, cy) = s.center();
        let out = update(
            &mut s,
            &desert(&tiles, Some(player_at(cx + 5.0, cy - 300.0))),
            false,
            &mut rng(),
        );
        assert!(out.died, "straight away");
    }

    /// `NPC.cs:63145-63204`: plant, wind up for half a second, fire, then a long cooldown.
    #[test]
    fn a_nebula_beast_stops_to_fire_and_then_waits() {
        let tiles = sand();
        let mut b = walker(NEBULA_BEAST, 200);
        b.direction = 1;
        b.sprite_direction = 1;
        let (cx, cy) = b.center();
        let w = desert(&tiles, Some(player_at(cx + 300.0, cy)));
        let mut r = rng();

        // 180 ticks of approach, then it plants itself.
        for _ in 0..180 {
            assert!(update(&mut b, &w, false, &mut r).shots.is_empty());
            b.velocity.1 = 0.0;
        }
        assert_eq!(b.ai[2], 1.0, "planted");

        let mut fired = None;
        for _ in 0..60 {
            if let Some(shot) = update(&mut b, &w, false, &mut r).shots.pop() {
                fired = Some(shot);
            }
            b.velocity.1 = 0.0;
        }
        let shot = fired.expect("it should have fired");
        assert_eq!(shot.projectile, 575);
        assert_eq!(shot.damage, 50, "classic damage");
        assert!(b.ai[1] < 0.0, "and it will not do it again for a while");
        assert_eq!(b.ai[2], 0.0, "back on its feet");
    }
}
