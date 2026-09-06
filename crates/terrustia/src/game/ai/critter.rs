//! Styles 115, 118 and 119 — the small drifting things.
//!
//! None of these chases anything. A **ladybug** (115) picks a heading at random, glides along it
//! with the wind, and turns whenever it is about to fly into rock or out over open sky; every so
//! often it lands and walks instead. A **seahorse** (118) does the same underwater, coasting
//! between kicks and bouncing off whatever it meets. A **dandelion** (119) does nothing at all
//! unless the wind is blowing it toward someone.
//!
//! The unifying trick is that the heading lives in `ai[0]` as an angle rather than as a velocity,
//! so bouncing is a reflection of that angle and the drift always resumes on the new course.

use rand::{Rng, rngs::SmallRng};

use super::{Shot, World};
use crate::game::npc::{Npc, TILE, TileView};
use terrustia_proto::npc_params::{
    BUTTERFLY_EASE, BUTTERFLY_FEAR_INTERVAL, BUTTERFLY_HOMING_RANGE, BUTTERFLY_PANIC_SPEED,
    BUTTERFLY_REPLAN, LACEWING_FADE_CAP, LACEWING_FADE_GONE, LACEWING_FEAR_INTERVAL,
    LACEWING_KEEP_RANGE, PRISMATIC_LACEWING,
};
use terrustia_proto::tile_solid::solid;

/// Speed a ladybug glides at, before the wind is added.
const GLIDE_SPEED: f32 = 1.0;
/// How much of the wind a ladybug picks up.
const WIND_PULL: f32 = 0.8;
/// How sharply it settles onto its chosen heading. Very gently: this is a glide, not a turn.
const GLIDE_EASE: f32 = 0.0125;

/// How far ahead a ladybug looks for something to fly into, and how far down it checks there is
/// still a world beneath it before climbing.
const FLOOR_LOOKAHEAD: i32 = 4;
const SKY_LOOKAHEAD: i32 = 30;

/// Beyond this a ladybug steers toward the nearest player rather than picking a random heading;
/// it is what stops them drifting off the edge of the world and never coming back.
const HOMING_RANGE: f32 = 700.0;

/// Ticks between a ladybug reconsidering, as an inclusive-exclusive range.
const REPLAN_TICKS: (u32, u32) = (60, 181);
/// One reconsideration in five also swaps between flying and walking.
const LANDING_CHANCE: u32 = 5;

/// Seahorse kick strength, top speed, and how long it coasts afterwards.
const KICK: f32 = 0.06;
const SWIM_SPEED: f32 = 3.0;
const COAST_TICKS: (u32, u32) = (450, 600);

/// How close to the surface a seahorse counts as being at the top of the water.
const SURFACE_MARGIN: f32 = 20.0;

/// A dandelion's seeds: the projectile, its damage, and the cadence of a puff.
const SEED_PROJECTILE: u16 = terrustia_proto::projectile::ids::DANDELION_SEED;
const SEED_DAMAGE: i32 = 7;
const PUFF_AT: f32 = 40.0;
const PUFF_OVER: f32 = 80.0;
/// How close a target has to be, and how level, for a dandelion to bother.
const PUFF_RANGE: f32 = 600.0;
const PUFF_HEIGHT: f32 = 100.0;
/// ...and the range at which it actually lets go.
const PUFF_LOOSE_RANGE: f32 = 500.0;

fn liquid_or_rock(tiles: &impl TileView, x: i32, y: i32) -> bool {
    let tile = tiles.tile(x, y);
    (tile.is_active() && solid(tile.block)) || tile.liquid > 0
}

/// Pick a heading: usually at random, but toward a distant player so they do not drift away.
fn choose_heading(npc: &mut Npc, world: &World<'_, impl TileView>, rng: &mut SmallRng) {
    npc.ai[0] = rng.random::<f32>() * std::f32::consts::TAU;
    if let Some(t) = world.target {
        let (cx, cy) = npc.center();
        let (dx, dy) = (t.center.0 - cx, t.center.1 - cy);
        if (dx * dx + dy * dy).sqrt() > HOMING_RANGE {
            npc.ai[0] = dy.atan2(dx) + (rng.random::<f32>() * 2.0 - 1.0) * 0.3;
        }
    }
    npc.dirty = true;
}

/// Drive one ladybug for a tick.
pub fn ladybug<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) {
    // `ai[1]` is its size, rolled once and then kept; `ai[2]` says whether it is walking; `ai[3]`
    // counts down to the next change of mind.
    if npc.ai[1] == 0.0 {
        npc.ai[1] = rng.random::<f32>() * 0.2 + 0.7;
        npc.dirty = true;
    }
    npc.ai[3] -= 1.0;
    if npc.ai[3] <= 0.0 {
        npc.ai[3] = rng.random_range(REPLAN_TICKS.0..REPLAN_TICKS.1) as f32;
        if rng.random_ratio(1, LANDING_CHANCE) {
            if npc.ai[2] == 0.0 {
                npc.ai[2] = 1.0;
                npc.ai[0] = 0.0;
            } else {
                npc.ai[2] = 0.0;
                choose_heading(npc, world, rng);
            }
        }
        choose_heading(npc, world, rng);
    }
    npc.scale = npc.ai[1];

    let (tile_x, tile_y) = (
        (npc.center().0 / TILE) as i32,
        (npc.center().1 / TILE) as i32,
    );

    if npc.ai[2] == 0.0 {
        // Gliding.
        let heading = (npc.ai[0].cos(), npc.ai[0].sin());
        let wanted = (
            heading.0 * GLIDE_SPEED + world.conditions.wind * WIND_PULL,
            heading.1 * GLIDE_SPEED,
        );
        npc.velocity.0 += (wanted.0 - npc.velocity.0) * GLIDE_EASE;
        npc.velocity.1 += (wanted.1 - npc.velocity.1) * GLIDE_EASE;

        // Something solid coming up: flip the heading and bleed the descent.
        if npc.velocity.1 > 0.0
            && (tile_y..tile_y + FLOOR_LOOKAHEAD).any(|y| liquid_or_rock(world.tiles, tile_x, y))
        {
            npc.ai[0] = -npc.ai[0];
            npc.velocity.1 *= 0.9;
        }
        // Nothing at all below for thirty tiles: it is over open sky and turns back down.
        if npc.velocity.1 < 0.0
            && !(tile_y..tile_y + SKY_LOOKAHEAD).any(|y| liquid_or_rock(world.tiles, tile_x, y))
        {
            npc.ai[0] = -npc.ai[0];
            npc.velocity.1 *= 0.9;
        }
        if npc.collide_x {
            npc.ai[0] = -npc.ai[0] + std::f32::consts::PI;
            npc.velocity.0 *= -0.2;
        }
    } else {
        // Walking. Water underfoot sends it back into the air.
        if npc.velocity.1 > 0.0 {
            let ahead = tile_x + i32::from(npc.direction);
            if (tile_y..tile_y + FLOOR_LOOKAHEAD).any(|y| world.tiles.tile(ahead, y).liquid > 0) {
                npc.velocity.1 = -1.0;
                npc.ai[2] = 0.0;
                npc.ai[0] =
                    rng.random::<f32>() * std::f32::consts::FRAC_PI_4 - std::f32::consts::FRAC_PI_2;
                if let Some(t) = world.target {
                    let (cx, cy) = npc.center();
                    let (dx, dy) = (t.center.0 - cx, t.center.1 - cy);
                    if (dx * dx + dy * dy).sqrt() > HOMING_RANGE {
                        npc.ai[0] = dy.atan2(dx) + (rng.random::<f32>() * 2.0 - 1.0) * 0.3;
                    }
                }
                npc.direction = if npc.velocity.0 > 0.0 { 1 } else { -1 };
                npc.dirty = true;
                return;
            }
        }
        if npc.velocity.1 != 0.0 {
            npc.velocity.0 *= 0.98;
            npc.velocity.1 += (2.0 - npc.velocity.1) * 0.005;
        } else {
            let wanted = f32::from(npc.direction);
            npc.velocity.0 += (wanted - npc.velocity.0) * 0.05;
            npc.velocity.1 += (0.0 - npc.velocity.1) * 0.05;
            npc.velocity.1 += 0.2;
            if npc.collide_x {
                npc.direction = -npc.direction;
                npc.velocity.0 *= -0.2;
                npc.dirty = true;
            }
        }
    }

    npc.direction = if npc.velocity.0 > 0.0 { 1 } else { -1 };
    npc.dirty = true;
}

/// Height of the water's surface in the column an NPC is in, if it is in water at all.
///
/// This is `Collision.GetWaterLineIterate`: walk up until the liquid stops, then measure how full
/// the last wet tile is.
pub fn water_line(tiles: &impl TileView, tile_x: i32, mut tile_y: i32) -> Option<f32> {
    let mut guard = 0;
    while tile_y > 0 && tiles.tile(tile_x, tile_y).liquid > 0 && guard < 10_000 {
        tile_y -= 1;
        guard += 1;
    }
    tile_y += 1;
    (tiles.tile(tile_x, tile_y).liquid > 0)
        .then(|| (tile_y * 16) as f32 - f32::from(tiles.tile(tile_x, tile_y - 1).liquid / 16))
}

/// Drive one seahorse for a tick.
pub fn seahorse<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) {
    npc.no_gravity = world.wet;
    let (tile_x, tile_y) = (
        (npc.center().0 / TILE) as i32,
        (npc.center().1 / TILE) as i32,
    );
    let surface = water_line(world.tiles, tile_x, tile_y);
    let at_surface = surface.is_some_and(|line| npc.position.1 - line < SURFACE_MARGIN);

    if !world.wet {
        // Out of water it flops: no propulsion, just friction and a slow tumble.
        if npc.velocity.1 == 0.0 {
            npc.velocity.0 *= 0.95;
        }
        npc.rotation += (npc.velocity.0 + npc.velocity.1) / 2.0 * 0.05;
    } else {
        npc.ai[1] -= 1.0;
        if npc.ai[1] <= 0.0 {
            // Kicking. Once it is up to speed it picks a fresh heading and coasts.
            npc.velocity.0 += npc.ai[0].cos() * KICK;
            npc.velocity.1 += npc.ai[0].sin() * KICK;
            let speed = (npc.velocity.0.powi(2) + npc.velocity.1.powi(2)).sqrt();
            if speed > SWIM_SPEED {
                npc.velocity.0 = npc.velocity.0.clamp(-SWIM_SPEED, SWIM_SPEED);
                npc.ai[1] = rng.random_range(COAST_TICKS.0..COAST_TICKS.1) as f32;
                npc.ai[0] = rng.random::<f32>() * std::f32::consts::TAU;
                // At the surface it will not choose a heading that takes it further up.
                if at_surface && npc.ai[0] > std::f32::consts::PI {
                    npc.ai[0] -= std::f32::consts::PI;
                }
                npc.dirty = true;
            }
        } else {
            npc.velocity.0 *= 0.95;
            npc.velocity.1 *= 0.95;
        }
        npc.rotation = npc.velocity.0 * 0.1;
    }

    // Bouncing reflects the heading, not just the velocity, so the coast resumes on the new course.
    let bounce_y = npc.collide_y && world.wet && (!at_surface || npc.velocity.1 < 0.0);
    if npc.collide_x || bounce_y {
        let mut heading = (npc.ai[0].cos(), npc.ai[0].sin());
        if npc.collide_x {
            heading.0 = -heading.0;
        }
        if bounce_y {
            heading.1 = -heading.1;
        }
        npc.ai[0] = heading.1.atan2(heading.0);
        let speed = (npc.velocity.0.powi(2) + npc.velocity.1.powi(2)).sqrt();
        npc.velocity = (npc.ai[0].cos() * speed, npc.ai[0].sin() * speed);
        npc.dirty = true;
    }
    npc.dirty = true;
}

/// Drive one dandelion for a tick, returning the seeds it let go of.
///
/// Without a wind blowing a player's way a dandelion does nothing but wither: its despawn timer is
/// cut to ten ticks every tick that the day is not windy.
pub fn dandelion<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    rng: &mut SmallRng,
) -> Vec<Shot> {
    if !world.conditions.windy && npc.time_left > 10 {
        npc.time_left = 10;
    }

    let mut downwind = false;
    let mut offset = 0.0;
    let mut reach = 0.0;
    // Whose slot the seeds are handed: `NewProjectile(..., 0f, target)` (`NPC.cs:47566`), and
    // `aiStyle 112` reads it every tick to decide whether to ride the wind at them or just sink.
    let toward = world.target.map_or(0.0, |t| f32::from(t.slot));
    if let Some(t) = world.target {
        let (cx, cy) = npc.center();
        offset = t.center.0 - cx;
        reach = offset.abs();
        downwind = (t.center.1 - cy).abs() < PUFF_HEIGHT
            && reach < PUFF_RANGE
            && ((offset > 0.0 && world.conditions.wind > 0.0)
                || (offset < 0.0 && world.conditions.wind < 0.0));
    }

    if npc.ai[0] != 1.0 {
        npc.ai[2] = 0.0;
        npc.ai[3] = 0.0;
        if downwind {
            npc.ai[0] = 1.0;
            npc.dirty = true;
        }
        return Vec::new();
    }

    npc.ai[2] = if reach < PUFF_LOOSE_RANGE { 1.0 } else { 0.0 };
    if !downwind {
        npc.ai[0] = 0.0;
        npc.dirty = true;
        return Vec::new();
    }
    if npc.ai[2] != 1.0 {
        return Vec::new();
    }

    npc.ai[3] += 1.0;
    if npc.ai[3] > PUFF_OVER {
        npc.ai[0] = 0.0;
        npc.dirty = true;
        return Vec::new();
    }
    if npc.ai[3] != PUFF_AT {
        return Vec::new();
    }

    // The puff: one to three seeds, scattered and blown downwind.
    let along = if offset > 0.0 { 1 } else { -1 };
    let (cx, cy) = npc.center();
    let mut seeds = Vec::new();
    for _ in 0..1 + rng.random_range(0..3) {
        let spread = (
            along as f32 * rng.random_range(-2..10) as f32,
            10.0 + rng.random_range(-6..6) as f32,
        );
        let mut velocity = (2.0 * along as f32 + spread.0 * 0.25, -2.0 + spread.1 * 0.25);
        if velocity.1 > -3.0 {
            velocity.1 = -3.0;
        }
        seeds.push(Shot {
            projectile: SEED_PROJECTILE,
            damage: SEED_DAMAGE,
            position: (cx + spread.0 + (along * 6) as f32, cy + spread.1),
            velocity,
            // Zero, meaning the type's own hour: a seed that never reaches anybody sinks and dies
            // on the ground, which `aiStyle 112` and tile collision settle between them rather
            // than a fuse.
            time_left: 0,
            ai: [0.0, toward, 0.0],
        });
    }
    npc.dirty = true;
    seeds
}

/// Drive one butterfly for a tick, returning whether it wants to be gone.
///
/// A butterfly's heading lives in `ai[0..1]` as a velocity it eases toward over a full second, so
/// its path is all long curves and no corners. Every couple of seconds it picks a new one: while it
/// is more than seven hundred pixels from anyone it heads back toward them, and the first time it
/// finds itself closer than that it stops homing for good and wanders freely from then on.
///
/// The Prismatic Lacewing shares the style and diverges inside it (`NPC.cs:45387-45445`), which is
/// what the return value is for: it is the one butterfly that can take itself out of the world.
pub fn butterfly<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) -> bool {
    // The Lacewing's own half of the routine, ahead of everything else exactly as vanilla puts it:
    //
    // ```csharp
    // int num3 = 60;
    // bool flag = false;
    // int num4 = 50;
    // NPCAimedTarget targetData = GetTargetData();
    // if (targetData.Invalid || targetData.Center.Distance(base.Center) >= 300f) flag = true;
    // if (!Main.remixWorld && !targetData.Invalid && targetData.Type == NPCTargetType.Player
    //     && !Main.player[target].ZoneHallow) { num4 = num3; flag = true; }
    // ai[2] = MathHelper.Clamp(ai[2] + (float)flag.ToDirectionInt(), 0f, num4);
    // if (ai[2] >= (float)num3) { active = false; ...; return; }
    // Opacity = Utils.GetLerpValue(num3, (float)num4 / 2f, ai[2], clamped: true);
    // ...
    // dontTakeDamage = ai[2] >= (float)(num4 / 2);
    // ```
    //
    // `flag.ToDirectionInt()` is `+1` when set and `-1` when not, so `ai[2]` is a fade counter that
    // climbs while nobody is near and unwinds while somebody is. Which is why the variety roll
    // below carries vanilla's own `type != 661`: for this one, `ai[2]` is not the variety at all,
    // and rolling a 1-to-8 into it would leave it permanently half faded and untouchable.
    //
    // Note what the two ceilings do. Out of range alone clamps at fifty, which never reaches sixty,
    // so a Lacewing you merely walk away from fades and turns untouchable but stays put; it is
    // *leaving the hallow* that raises the ceiling to sixty and lets it go. That is the whole rule
    // players know it by.
    //
    // Two disclosed narrowings. `ZoneHallow` is read off the nearest player rather than off this
    // one's own target, the same stand-in `Conditions`' `crimson`, `jungle`, `snow` and `desert`
    // already make; and `Main.remixWorld` is not modelled anywhere in this server, so that half of
    // the second gate drops. `Opacity` is left out as pure drawing: it is a pure function of `ai[2]`
    // and `num4`, both of which a client already has, and nothing server-side reads it back.
    if npc.npc_type == PRISMATIC_LACEWING {
        let (cx, cy) = npc.center();
        let far = world.target.is_none_or(|t| {
            let (dx, dy) = (t.center.0 - cx, t.center.1 - cy);
            (dx * dx + dy * dy).sqrt() >= LACEWING_KEEP_RANGE
        });
        let mut cap = LACEWING_FADE_CAP;
        let mut fading = far;
        if world.target.is_some() && !world.conditions.hallow {
            cap = LACEWING_FADE_GONE;
            fading = true;
        }
        npc.ai[2] = (npc.ai[2] + if fading { 1.0 } else { -1.0 }).clamp(0.0, cap);
        npc.dirty = true;
        if npc.ai[2] >= LACEWING_FADE_GONE {
            return true;
        }
        // Integer division, as vanilla writes it: twenty-five at the ordinary ceiling, thirty once
        // it has been raised.
        npc.invulnerable = npc.ai[2] >= ((cap as i32) / 2) as f32;
    }

    // `ai[2]` is which of the eight varieties it is — the game rolls it once to decide what lands
    // in your net — and `ai[3]` is its size. The Lacewing is excluded from the variety roll and
    // from it alone; everything below this line it does share.
    if npc.ai[2] == 0.0 && npc.npc_type != PRISMATIC_LACEWING {
        let roll = rng.random_range(0..100);
        npc.ai[2] = 1.0
            + match roll {
                0 => 5.0,
                1..=2 => 1.0,
                3..=8 => 2.0,
                9..=18 => 7.0,
                19..=33 => 3.0,
                34..=52 => 6.0,
                75.. => 0.0,
                _ => 4.0,
            };
        npc.dirty = true;
    }
    if npc.ai[3] == 0.0 {
        npc.ai[3] = rng.random_range(75..111) as f32 * 0.01;
    }
    npc.scale = npc.ai[3];

    npc.local_ai[0] -= 1.0;
    if npc.local_ai[0] <= 0.0 {
        npc.local_ai[0] = rng.random_range(BUTTERFLY_REPLAN.0..BUTTERFLY_REPLAN.1) as f32;
        let across = world
            .target
            .map_or(0.0, |t| (npc.center().0 - t.center.0).abs());
        if let Some(t) = world.target {
            npc.direction = if t.center.0 > npc.center().0 { 1 } else { -1 };
        }
        // Homing happens only until the first time it finds itself close enough. After that
        // `local_ai[3]` is set for good and it drifts wherever it likes.
        if across > BUTTERFLY_HOMING_RANGE && npc.local_ai[3] == 0.0 {
            let speed = if across > 1000.0 {
                rng.random_range(150..201) as f32 * 0.01
            } else if across > 850.0 {
                rng.random_range(100..151) as f32 * 0.01
            } else {
                rng.random_range(50..151) as f32 * 0.01
            };
            let along = i32::from(npc.direction) * rng.random_range(100..251);
            let mut rise = rng.random_range(-50..51);
            if let Some(t) = world.target
                && npc.position.1 > t.center.1 - super::PLAYER_HEIGHT as f32 / 2.0 - 100.0
            {
                rise -= rng.random_range(100..251);
            }
            let k = speed / ((along * along + rise * rise) as f32).sqrt();
            npc.ai[0] = along as f32 * k;
            npc.ai[1] = rise as f32 * k;
        } else {
            npc.local_ai[3] = 1.0;
            let speed = rng.random_range(26..301) as f32 * 0.01;
            let (dx, dy) = (rng.random_range(-100..101), rng.random_range(-100..101));
            let k = speed / (((dx * dx + dy * dy) as f32).sqrt()).max(f32::MIN_POSITIVE);
            npc.ai[0] = dx as f32 * k;
            npc.ai[1] = dy as f32 * k;
        }
        npc.dirty = true;
    }

    // A full second of easing onto the chosen heading, which is what makes the path a curve.
    npc.velocity.0 = (npc.velocity.0 * (BUTTERFLY_EASE - 1.0) + npc.ai[0]) / BUTTERFLY_EASE;
    npc.velocity.1 = (npc.velocity.1 * (BUTTERFLY_EASE - 1.0) + npc.ai[1]) / BUTTERFLY_EASE;

    let (tile_x, tile_y) = (
        (npc.center().0 / TILE) as i32,
        (npc.center().1 / TILE) as i32,
    );
    if npc.velocity.1 > 0.0 && (tile_y..tile_y + 3).any(|y| liquid_or_rock(world.tiles, tile_x, y))
    {
        npc.ai[1] = -npc.ai[1];
        npc.velocity.1 *= 0.9;
    }
    if npc.velocity.1 < 0.0
        && !(tile_y..tile_y + 30).any(|y| {
            let t = world.tiles.tile(tile_x, y);
            t.is_active() && solid(t.block)
        })
    {
        npc.ai[1] = -npc.ai[1];
        npc.velocity.1 *= 0.9;
    }

    // A butterfly is afraid of monsters, not of you. The caller works out what is nearby.
    if npc.local_ai[1] > 0.0 {
        npc.local_ai[1] -= 1.0;
    } else {
        // `NPC.cs:45549-45553`: the Lacewing looks around half again as often as the rest.
        npc.local_ai[1] = if npc.npc_type == PRISMATIC_LACEWING {
            LACEWING_FEAR_INTERVAL
        } else {
            BUTTERFLY_FEAR_INTERVAL
        };
        let (fx, fy) = world.crowding;
        if fx != 0.0 || fy != 0.0 {
            npc.velocity.0 += fx * 2.0;
            npc.velocity.1 += fy * 2.0;
            let speed = (npc.velocity.0.powi(2) + npc.velocity.1.powi(2)).sqrt();
            if speed > BUTTERFLY_PANIC_SPEED {
                npc.velocity.0 = npc.velocity.0 / speed * BUTTERFLY_PANIC_SPEED;
                npc.velocity.1 = npc.velocity.1 / speed * BUTTERFLY_PANIC_SPEED;
            }
        }
    }

    if npc.collide_x {
        npc.ai[0] = if npc.velocity.0 < 0.0 {
            npc.ai[0].abs()
        } else {
            -npc.ai[0].abs()
        };
        npc.velocity.0 *= -0.2;
    }
    if npc.velocity.0 < 0.0 {
        npc.direction = -1;
    }
    if npc.velocity.0 > 0.0 {
        npc.direction = 1;
    }
    npc.dirty = true;
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::{Liquid, Tile};

    #[derive(Default)]
    struct Air(HashMap<(i32, i32), Tile>);

    impl TileView for Air {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(9)
    }

    fn world<'a>(tiles: &'a Air, target: Option<Target>) -> World<'a, Air> {
        crate::game::ai::calm(tiles, target)
    }

    fn at(npc_type: u16, tile_x: i32, tile_y: i32) -> Npc {
        Npc::new(npc_type, (tile_x as f32 * TILE, tile_y as f32 * TILE), 1).expect("a critter type")
    }

    #[test]
    fn a_ladybug_rolls_its_own_size_once_and_keeps_it() {
        let tiles = Air::default();
        let mut bug = at(604, 500, 500);
        ladybug(&mut bug, &world(&tiles, None), &mut rng());
        let size = bug.ai[1];
        assert!((0.7..0.9).contains(&size), "got {size}");
        assert_eq!(bug.scale, size, "and wears it");
        for _ in 0..50 {
            ladybug(&mut bug, &world(&tiles, None), &mut rng());
        }
        assert_eq!(bug.ai[1], size, "it should not keep rerolling");
    }

    #[test]
    fn a_ladybug_over_open_sky_turns_back_down() {
        // Ground far below, so nothing within thirty tiles.
        let tiles = Air::default();
        let mut bug = at(604, 500, 500);
        bug.ai[3] = 1000.0;
        bug.ai[1] = 0.8;
        // Heading upward.
        bug.ai[0] = -std::f32::consts::FRAC_PI_2;
        bug.velocity = (0.0, -1.0);
        let before = bug.ai[0];
        ladybug(&mut bug, &world(&tiles, None), &mut rng());
        assert_eq!(bug.ai[0], -before, "should have flipped its heading");
    }

    #[test]
    fn a_ladybug_about_to_hit_rock_turns_away() {
        let mut tiles = Air::default();
        for y in 500..510 {
            tiles.0.insert((500, y), Tile::block(1));
        }
        let mut bug = at(604, 500, 498);
        bug.ai[3] = 1000.0;
        bug.ai[1] = 0.8;
        bug.ai[0] = std::f32::consts::FRAC_PI_2;
        bug.velocity = (0.0, 1.0);
        let before = bug.ai[0];
        ladybug(&mut bug, &world(&tiles, None), &mut rng());
        assert_eq!(bug.ai[0], -before);
    }

    #[test]
    fn a_ladybug_steers_toward_a_very_distant_player() {
        let tiles = Air::default();
        let mut bug = at(604, 500, 500);
        bug.ai[3] = 1.0;
        let (cx, cy) = bug.center();
        let far = Some(Target {
            slot: 0,
            center: (cx + 2000.0, cy),
            velocity: (0.0, 0.0),
            alive: true,
        });
        ladybug(&mut bug, &world(&tiles, far), &mut rng());
        // Straight along the positive x axis, give or take the game's own jitter.
        assert!(
            bug.ai[0].abs() < 0.4,
            "should head toward the player, got {}",
            bug.ai[0]
        );
    }

    #[test]
    fn the_water_line_is_the_top_of_the_column() {
        let mut tiles = Air::default();
        for y in 100..120 {
            tiles
                .0
                .insert((50, y), Tile::AIR.with_liquid(Liquid::Water, 255));
        }
        let line = water_line(&tiles, 50, 110).expect("in water");
        assert!(
            (line - 100.0 * 16.0).abs() < 16.0,
            "surface should be near tile 100, got {line}"
        );
        assert!(water_line(&tiles, 51, 110).is_none(), "and dry elsewhere");
    }

    #[test]
    fn a_seahorse_out_of_water_just_flops() {
        let tiles = Air::default();
        let mut horse = at(626, 50, 50);
        horse.velocity = (2.0, 0.0);
        seahorse(&mut horse, &world(&tiles, None), &mut rng());
        assert!(!horse.no_gravity, "it should fall");
        assert!(horse.velocity.0 < 2.0, "and lose its speed");
    }

    #[test]
    fn a_seahorse_in_water_kicks_up_to_speed_and_then_coasts() {
        let tiles = Air::default();
        let mut horse = at(626, 50, 50);
        let mut w = world(&tiles, None);
        w.wet = true;
        let mut r = rng();
        for _ in 0..400 {
            seahorse(&mut horse, &w, &mut r);
            if horse.ai[1] > 0.0 {
                break;
            }
        }
        assert!(horse.no_gravity, "weightless in water");
        assert!(
            horse.ai[1] >= COAST_TICKS.0 as f32,
            "should have settled into a coast, got {}",
            horse.ai[1]
        );
        let speed = (horse.velocity.0.powi(2) + horse.velocity.1.powi(2)).sqrt();
        assert!(
            speed <= SWIM_SPEED * 1.5,
            "and not exceed its speed: {speed}"
        );
    }

    #[test]
    fn a_seahorse_bouncing_reflects_its_heading() {
        let tiles = Air::default();
        let mut horse = at(626, 50, 50);
        horse.ai[0] = 0.5;
        horse.ai[1] = 100.0;
        horse.velocity = (2.0, 1.0);
        horse.collide_x = true;
        let mut w = world(&tiles, None);
        w.wet = true;
        seahorse(&mut horse, &w, &mut rng());
        assert!(
            horse.ai[0].cos() < 0.0,
            "should now be heading the other way, got angle {}",
            horse.ai[0]
        );
    }

    #[test]
    fn a_butterfly_rolls_a_variety_and_a_size_once() {
        let tiles = Air::default();
        let mut b = at(356, 500, 400);
        butterfly(&mut b, &world(&tiles, None), &mut rng());
        let (variety, size) = (b.ai[2], b.ai[3]);
        assert!((1.0..=8.0).contains(&variety), "got {variety}");
        assert!((0.75..1.11).contains(&size), "got {size}");
        assert_eq!(b.scale, size);
        for _ in 0..500 {
            butterfly(&mut b, &world(&tiles, None), &mut rng());
        }
        assert_eq!((b.ai[2], b.ai[3]), (variety, size), "and keeps them");
    }

    #[test]
    fn a_butterfly_far_from_anyone_heads_back_toward_them() {
        let mut tiles = Air::default();
        // Ground beneath, so it does not think it is over open sky and keep flipping.
        for x in 0..2000 {
            tiles.0.insert((x, 410), Tile::block(1));
        }
        let mut b = at(356, 500, 400);
        let (cx, cy) = b.center();
        let far = Some(Target {
            slot: 0,
            center: (cx + BUTTERFLY_HOMING_RANGE + 400.0, cy),
            velocity: (0.0, 0.0),
            alive: true,
        });
        b.local_ai[0] = 1.0;
        butterfly(&mut b, &world(&tiles, far), &mut rng());
        assert!(b.ai[0] > 0.0, "should aim toward them, got {}", b.ai[0]);
    }

    /// Once a butterfly has been near someone it stops homing, permanently.
    #[test]
    fn a_butterfly_that_has_been_close_wanders_freely_ever_after() {
        let tiles = Air::default();
        let mut b = at(356, 500, 400);
        let (cx, cy) = b.center();
        let close = Some(Target {
            slot: 0,
            center: (cx + 50.0, cy),
            velocity: (0.0, 0.0),
            alive: true,
        });
        b.local_ai[0] = 1.0;
        butterfly(&mut b, &world(&tiles, close), &mut rng());
        assert_eq!(b.local_ai[3], 1.0, "should have given up homing");

        let far = Some(Target {
            slot: 0,
            center: (cx + BUTTERFLY_HOMING_RANGE + 400.0, cy),
            velocity: (0.0, 0.0),
            alive: true,
        });
        for _ in 0..20 {
            b.local_ai[0] = 1.0;
            butterfly(&mut b, &world(&tiles, far), &mut rng());
            assert_eq!(b.local_ai[3], 1.0);
        }
    }

    #[test]
    fn a_butterfly_bolts_from_something_dangerous() {
        let tiles = Air::default();
        let mut b = at(356, 500, 400);
        butterfly(&mut b, &world(&tiles, None), &mut rng());
        b.local_ai[1] = 0.0;
        b.velocity = (0.0, 0.0);
        let mut w = world(&tiles, None);
        // A push to the left, as the caller would work out from a nearby monster.
        w.crowding = (-1.0, 0.0);
        butterfly(&mut b, &w, &mut rng());
        assert!(b.velocity.0 < 0.0, "should flee, got {}", b.velocity.0);
    }

    #[test]
    fn a_butterfly_eases_onto_its_heading_rather_than_snapping_to_it() {
        let tiles = Air::default();
        let mut b = at(356, 500, 400);
        butterfly(&mut b, &world(&tiles, None), &mut rng());
        b.local_ai[0] = 1000.0;
        b.velocity = (0.0, 0.0);
        b.ai[0] = 3.0;
        b.ai[1] = 0.0;
        butterfly(&mut b, &world(&tiles, None), &mut rng());
        assert!(
            b.velocity.0 > 0.0 && b.velocity.0 < 0.2,
            "one sixtieth of the way there, got {}",
            b.velocity.0
        );
    }

    /// The Lacewing keeps a fade counter in `ai[2]` where an ordinary butterfly keeps its variety
    /// (`NPC.cs:45387-45414`), which is why vanilla's variety roll carries `type != 661`.
    ///
    /// Neutralised by dropping the `&& npc.npc_type != PRISMATIC_LACEWING` from the variety roll:
    /// "a Lacewing rolled a variety into its fade counter: 6", and the Lacewing arrives already a
    /// tenth of the way faded and, one tick later than that, untouchable.
    #[test]
    fn a_lacewing_keeps_a_fade_counter_where_a_butterfly_keeps_its_variety() {
        let tiles = Air::default();
        let mut b = at(PRISMATIC_LACEWING, 500, 400);
        let (cx, cy) = b.center();
        let close = Some(Target {
            slot: 0,
            center: (cx + 50.0, cy),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let mut w = world(&tiles, close);
        w.conditions.hallow = true;
        butterfly(&mut b, &w, &mut rng());
        assert_eq!(
            b.ai[2], 0.0,
            "a Lacewing rolled a variety into its fade counter: {}",
            b.ai[2]
        );
        assert!(!b.invulnerable, "and it should still be killable");
    }

    /// Walking away from a Lacewing fades it and makes it untouchable, and leaves it there: the
    /// counter clamps at fifty and the exit is at sixty (`NPC.cs:45389-45409`).
    ///
    /// Neutralised by raising the clamp to `LACEWING_FADE_GONE` unconditionally: "it should still be
    /// here", the Lacewing vanishing from simple distance instead of from leaving the hallow.
    #[test]
    fn a_lacewing_left_alone_in_the_hallow_fades_but_stays() {
        let tiles = Air::default();
        let mut b = at(PRISMATIC_LACEWING, 500, 400);
        let mut w = world(&tiles, None);
        w.conditions.hallow = true;
        for _ in 0..200 {
            assert!(
                !butterfly(&mut b, &w, &mut rng()),
                "it should still be here"
            );
        }
        assert_eq!(b.ai[2], LACEWING_FADE_CAP, "and be as faded as it can get");
        assert!(b.invulnerable, "and untouchable while it is");

        // Come back and it unwinds, one a tick, and can be hurt again.
        let (cx, cy) = b.center();
        let close = Some(Target {
            slot: 0,
            center: (cx + 50.0, cy),
            velocity: (0.0, 0.0),
            alive: true,
        });
        let mut near = world(&tiles, close);
        near.conditions.hallow = true;
        for _ in 0..(LACEWING_FADE_CAP as i32) {
            butterfly(&mut b, &near, &mut rng());
        }
        assert_eq!(b.ai[2], 0.0, "should have come all the way back");
        assert!(!b.invulnerable, "and be killable again");
    }

    /// Leaving the hallow is what actually ends it: the second gate raises the ceiling to sixty,
    /// which is the only value that reaches the exit (`NPC.cs:45400-45409`).
    ///
    /// Neutralised by deleting the `!world.conditions.hallow` arm: "a Lacewing outside the hallow
    /// never left", the counter sitting at fifty for ever.
    #[test]
    fn a_lacewing_goes_when_the_player_leaves_the_hallow() {
        let tiles = Air::default();
        let mut b = at(PRISMATIC_LACEWING, 500, 400);
        let (cx, cy) = b.center();
        let close = Some(Target {
            slot: 0,
            center: (cx + 50.0, cy),
            velocity: (0.0, 0.0),
            alive: true,
        });
        // Standing right next to it, but no longer in the hallow.
        let w = world(&tiles, close);
        assert!(!w.conditions.hallow);
        let mut ticks = 0;
        loop {
            ticks += 1;
            if butterfly(&mut b, &w, &mut rng()) {
                break;
            }
            assert!(
                ticks <= LACEWING_FADE_GONE as i32 + 5,
                "a Lacewing outside the hallow never left"
            );
        }
        assert_eq!(ticks, LACEWING_FADE_GONE as i32, "one tick per count");
    }

    #[test]
    fn a_dandelion_on_a_still_day_withers() {
        let tiles = Air::default();
        let mut d = at(628, 50, 50);
        let seeds = dandelion(&mut d, &world(&tiles, None), &mut rng());
        assert!(seeds.is_empty());
        assert!(d.time_left <= 10, "should be about to vanish");
    }

    #[test]
    fn a_dandelion_puffs_at_someone_downwind() {
        let tiles = Air::default();
        let mut d = at(628, 50, 50);
        let (cx, cy) = d.center();
        let mut w = world(
            &tiles,
            Some(Target {
                slot: 0,
                center: (cx + 300.0, cy),
                velocity: (0.0, 0.0),
                alive: true,
            }),
        );
        w.conditions.windy = true;
        w.conditions.wind = 0.5;
        let mut r = rng();
        let mut seeds = Vec::new();
        for _ in 0..200 {
            seeds = dandelion(&mut d, &w, &mut r);
            if !seeds.is_empty() {
                break;
            }
        }
        assert!(!seeds.is_empty(), "should have let go");
        assert!(seeds.len() <= 3);
        assert!(seeds.iter().all(|s| s.projectile == SEED_PROJECTILE));
        assert!(
            seeds.iter().all(|s| s.velocity.0 > 0.0),
            "and blown downwind"
        );
    }

    #[test]
    fn a_dandelion_puff_tells_its_seeds_who_to_chase() {
        let tiles = Air::default();
        let mut d = at(628, 50, 50);
        let (cx, cy) = d.center();
        let mut w = world(
            &tiles,
            Some(Target {
                // Not slot zero: a seed that was told nothing reads as zero, so a target standing
                // there makes "told" and "not told" the same answer.
                slot: 3,
                center: (cx + 300.0, cy),
                velocity: (0.0, 0.0),
                alive: true,
            }),
        );
        w.conditions.windy = true;
        w.conditions.wind = 0.5;
        let mut r = rng();
        let mut seeds = Vec::new();
        for _ in 0..200 {
            seeds = dandelion(&mut d, &w, &mut r);
            if !seeds.is_empty() {
                break;
            }
        }
        assert!(!seeds.is_empty(), "it should have puffed");
        // `NewProjectile(..., 0f, target)` (`NPC.cs:47566`): `aiStyle 112` reads this every tick to
        // decide whether to ride the wind at somebody or just sink, so a seed without it sinks.
        assert!(
            seeds.iter().all(|s| s.ai[1] == 3.0),
            "every seed carries the slot it was aimed at, not zero"
        );
    }

    #[test]
    fn a_dandelion_ignores_someone_upwind() {
        let tiles = Air::default();
        let mut d = at(628, 50, 50);
        let (cx, cy) = d.center();
        let mut w = world(
            &tiles,
            Some(Target {
                slot: 0,
                center: (cx + 300.0, cy),
                velocity: (0.0, 0.0),
                alive: true,
            }),
        );
        w.conditions.windy = true;
        // Wind blowing the other way.
        w.conditions.wind = -0.5;
        let mut r = rng();
        for _ in 0..200 {
            assert!(dandelion(&mut d, &w, &mut r).is_empty());
        }
    }
}

/// Style 64 — fireflies, lightning bugs, lavaflies and the shimmerfly.
///
/// A firefly holds a heading for one to three seconds and then picks another, easing onto it over
/// eighty ticks so the drift never looks steered. Two rules keep it in the world without it ever
/// appearing to notice: it will not fly into ground four tiles below, and it will not climb away
/// from ground thirty tiles below — between those it wanders freely.
///
/// The one thing it does deliberately is come *to* you: from more than seven hundred pixels away
/// it heads your way, faster the further off it is, until it arrives once. After that it wanders
/// for the rest of its life and never seeks again, which is why fireflies gather where you are and
/// then mill about.
///
/// `world.avoid` carries whatever the shimmerfly should keep away from; for every other type it
/// is empty and the crowd check costs nothing.
pub fn firefly<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) {
    use terrustia_proto::npc_params::{
        FIREFLY_DARK_BELOW, FIREFLY_FLOOR_LOOK, FIREFLY_GLOW_FOR, FIREFLY_GLOW_GAP, FIREFLY_HOLD,
        FIREFLY_SCALE, FIREFLY_SEEK_AT, FIREFLY_SEEK_FAR, FIREFLY_SEEK_FAR_SPEED,
        FIREFLY_SEEK_FARTHER, FIREFLY_SEEK_FARTHER_SPEED, FIREFLY_SEEK_SPEED, FIREFLY_SKY_LOOK,
        FIREFLY_SMOOTH, FIREFLY_WANDER_SPEED, SHIMMERFLY, SHIMMERFLY_BOLT, SHIMMERFLY_BOLT_CAP,
        SHIMMERFLY_CHECK_EVERY, SHIMMERFLY_EDGE_CAP, SHIMMERFLY_EDGE_PUSH, SHIMMERFLY_MARGIN,
    };

    npc.dirty = true;
    // `ai[0]` and `ai[1]` are the heading it is easing toward, not its velocity.
    let (mut want_x, mut want_y) = (npc.ai[0], npc.ai[1]);

    npc.local_ai[0] -= 1.0;
    if npc.ai[3] == 0.0 {
        npc.ai[3] = rng.random_range(FIREFLY_SCALE.0..FIREFLY_SCALE.1) as f32 * 0.01;
    }
    if npc.local_ai[0] <= 0.0 {
        npc.local_ai[0] = rng.random_range(FIREFLY_HOLD.0..FIREFLY_HOLD.1) as f32;
        let (cx, cy) = npc.center();
        let across = world.target.map_or(0.0, |t| (cx - t.center.0).abs());
        // `local_ai[3]` remembers that it has arrived once. It never seeks again after that.
        if across > FIREFLY_SEEK_AT && npc.local_ai[3] == 0.0 {
            let speed = if across > FIREFLY_SEEK_FARTHER {
                rng.random_range(FIREFLY_SEEK_FARTHER_SPEED.0..FIREFLY_SEEK_FARTHER_SPEED.1)
            } else if across > FIREFLY_SEEK_FAR {
                rng.random_range(FIREFLY_SEEK_FAR_SPEED.0..FIREFLY_SEEK_FAR_SPEED.1)
            } else {
                rng.random_range(FIREFLY_SEEK_SPEED.0..FIREFLY_SEEK_SPEED.1)
            } as f32
                * 0.01;
            let step_x = i32::from(npc.direction) * rng.random_range(100..251);
            let mut step_y = rng.random_range(-50..51);
            // Below the player it aims well above, so it comes up to meet you rather than
            // crawling along the floor.
            if world.target.is_some_and(|t| cy > t.center.1 - 100.0) {
                step_y -= rng.random_range(100..251);
            }
            let length = ((step_x * step_x + step_y * step_y) as f32)
                .sqrt()
                .max(f32::MIN_POSITIVE);
            want_x = step_x as f32 * speed / length;
            want_y = step_y as f32 * speed / length;
        } else {
            npc.local_ai[3] = 1.0;
            let speed =
                rng.random_range(FIREFLY_WANDER_SPEED.0..FIREFLY_WANDER_SPEED.1) as f32 * 0.01;
            let step_x = rng.random_range(-100..101);
            let step_y = rng.random_range(-100..101);
            let length = ((step_x * step_x + step_y * step_y) as f32)
                .sqrt()
                .max(f32::MIN_POSITIVE);
            want_x = step_x as f32 * speed / length;
            want_y = step_y as f32 * speed / length;
        }
    }
    npc.scale = npc.ai[3];

    if npc.npc_type == SHIMMERFLY {
        // A shimmerfly turns back from the edges of the world rather than drifting out of it.
        let (cx, cy) = npc.center();
        let (tile_x, tile_y) = ((cx / TILE) as i32, (cy / TILE) as i32);
        let mut settled = true;
        if tile_x < SHIMMERFLY_MARGIN {
            want_x = (want_x + SHIMMERFLY_EDGE_PUSH).min(SHIMMERFLY_EDGE_CAP);
            settled = false;
        } else if tile_x > world.world_width() - SHIMMERFLY_MARGIN {
            want_x = (want_x - SHIMMERFLY_EDGE_PUSH).max(-SHIMMERFLY_EDGE_CAP);
            settled = false;
        }
        if tile_y < SHIMMERFLY_MARGIN {
            want_y = (want_y + SHIMMERFLY_EDGE_PUSH).min(SHIMMERFLY_EDGE_CAP);
            settled = false;
        } else if tile_y > world.world_height() - SHIMMERFLY_MARGIN {
            want_y = (want_y - SHIMMERFLY_EDGE_PUSH).max(-SHIMMERFLY_EDGE_CAP);
            settled = false;
        }

        if npc.local_ai[1] > 0.0 {
            npc.local_ai[1] -= 1.0;
        } else if settled {
            npc.local_ai[1] = SHIMMERFLY_CHECK_EVERY;
            // Anything alive *nearby* sends it off in the opposite direction, hard. The reach is
            // carried per entry because vanilla uses two of them: a hostile counts within a
            // hundred pixels (`NPC.cs:34446`) and a player within a hundred and fifty
            // (`NPC.cs:34451`). Without the test a shimmerfly bolts every fifteen ticks for as
            // long as anything hostile is loaded anywhere in the world, averaged over creatures
            // thousands of pixels away, and never settles.
            let mut away = (0.0f32, 0.0f32);
            let mut crowd = 0.0f32;
            for &(kx, ky, reach) in world.avoid {
                let (dx, dy) = (cx - kx, cy - ky);
                // Compared squared, which is the same test without the square root: most of the
                // list is out of reach, and this way those entries never pay for one.
                let gap2 = dx * dx + dy * dy;
                if gap2 > 0.0 && gap2 <= reach * reach {
                    let gap = gap2.sqrt();
                    crowd += 1.0;
                    away.0 += dx / gap;
                    away.1 += dy / gap;
                }
            }
            if crowd > 0.0 {
                away.0 = away.0 / crowd * SHIMMERFLY_BOLT;
                away.1 = away.1 / crowd * SHIMMERFLY_BOLT;
                npc.velocity.0 += away.0;
                npc.velocity.1 += away.1;
                let speed = npc.velocity.0.hypot(npc.velocity.1);
                if speed > SHIMMERFLY_BOLT_CAP {
                    npc.velocity.0 = npc.velocity.0 / speed * SHIMMERFLY_BOLT_CAP;
                    npc.velocity.1 = npc.velocity.1 / speed * SHIMMERFLY_BOLT_CAP;
                }
                // And it reconsiders where it was going almost at once.
                npc.local_ai[0] = 10.0;
            }
        }
    } else if npc.local_ai[2] > 0.0 {
        // Glowing.
        npc.local_ai[2] -= 1.0;
    } else if npc.local_ai[1] > 0.0 {
        npc.local_ai[1] -= 1.0;
    } else {
        npc.local_ai[1] = rng.random_range(FIREFLY_GLOW_GAP.0..FIREFLY_GLOW_GAP.1) as f32;
        // There is no point glowing in daylight on the surface.
        let underground =
            npc.position.1 / TILE > world.conditions.surface_y / TILE + FIREFLY_DARK_BELOW;
        if !world.conditions.day || underground {
            npc.local_ai[2] = rng.random_range(FIREFLY_GLOW_FOR.0..FIREFLY_GLOW_FOR.1) as f32;
        }
    }

    npc.velocity.0 = (npc.velocity.0 * (FIREFLY_SMOOTH - 1.0) + want_x) / FIREFLY_SMOOTH;
    npc.velocity.1 = (npc.velocity.1 * (FIREFLY_SMOOTH - 1.0) + want_y) / FIREFLY_SMOOTH;

    let (cx, cy) = npc.center();
    let (tile_x, tile_y) = ((cx / TILE) as i32, (cy / TILE) as i32);
    let blocked = |y: i32| {
        let tile = world.tiles.tile(tile_x, y);
        (tile.is_active() && solid(tile.block)) || tile.liquid > 0
    };
    if npc.velocity.1 > 0.0 {
        // Descending onto something: turn the heading around and slow the drop.
        for y in tile_y..tile_y + FIREFLY_FLOOR_LOOK {
            if blocked(y) {
                want_y *= -1.0;
                npc.velocity.1 *= 0.9;
            }
        }
    }
    if npc.velocity.1 < 0.0 {
        // Climbing with nothing at all beneath: it has drifted off the world and turns back.
        let ground_below = (tile_y..tile_y + FIREFLY_SKY_LOOK).any(|y| {
            let tile = world.tiles.tile(tile_x, y);
            tile.is_active() && solid(tile.block)
        });
        if !ground_below {
            want_y *= -1.0;
            npc.velocity.1 *= 0.9;
        }
    }
    if npc.collide_x {
        // Keep the heading's magnitude, take the sign from the way it is now going.
        want_x = if npc.velocity.0 < 0.0 {
            want_x.abs()
        } else {
            -want_x.abs()
        };
        npc.velocity.0 *= -0.2;
    }
    if npc.npc_type == SHIMMERFLY {
        npc.rotation = npc.velocity.0 * 0.3;
    }
    if npc.velocity.0 < 0.0 {
        npc.direction = -1;
    }
    if npc.velocity.0 > 0.0 {
        npc.direction = 1;
    }
    npc.ai[0] = want_x;
    npc.ai[1] = want_y;
}

#[cfg(test)]
mod firefly_tests {
    use super::*;
    use crate::game::npc::TileView;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    struct Meadow(HashMap<(i32, i32), Tile>);

    impl TileView for Meadow {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn meadow(floor: i32) -> Meadow {
        let mut tiles = HashMap::new();
        for x in -600..600 {
            for y in floor..floor + 4 {
                tiles.insert((x, y), Tile::block(1));
            }
        }
        Meadow(tiles)
    }

    fn world<'a>(tiles: &'a Meadow, target: Option<(f32, f32)>) -> World<'a, Meadow> {
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

    use terrustia_proto::npc_params::{SHIMMERFLY_SHY_OF_NPCS, SHIMMERFLY_SHY_OF_PLAYERS};

    const FIREFLY: u16 = 355;
    const SHIMMERFLY_TYPE: u16 = 677;

    fn bug(npc_type: u16, tile_x: i32, tile_y: i32) -> Npc {
        Npc::new(npc_type, (tile_x as f32 * TILE, tile_y as f32 * TILE), 1)
            .expect("a style 64 type")
    }

    /// A firefly a long way off comes to you once, and then stops seeking for good.
    #[test]
    fn a_distant_firefly_comes_to_you_once() {
        let tiles = meadow(60);
        let mut rng = SmallRng::seed_from_u64(64);
        let mut f = bug(FIREFLY, 0, 50);
        f.direction = 1;
        // Well past the seven-hundred-pixel threshold.
        let w = world(&tiles, Some((80.0 * TILE, 50.0 * TILE)));

        let start = f.position.0;
        for _ in 0..1200 {
            firefly(&mut f, &w, &mut rng);
            crate::game::npc::step_physics(&mut f, &tiles);
        }
        assert!(
            f.position.0 > start,
            "it should have drifted toward the player, moved {}",
            f.position.0 - start
        );
        assert_eq!(f.local_ai[3], 1.0, "and stopped seeking once it arrived");
    }

    /// It will not fly into the ground, however long it drifts.
    #[test]
    fn a_firefly_stays_off_the_floor() {
        let tiles = meadow(60);
        let mut rng = SmallRng::seed_from_u64(11);
        let mut f = bug(FIREFLY, 0, 55);
        // Already arrived, so it is wandering rather than steering anywhere.
        f.local_ai[3] = 1.0;
        let w = world(&tiles, Some((0.0, 55.0 * TILE)));

        let mut lowest = f.position.1;
        for _ in 0..3000 {
            firefly(&mut f, &w, &mut rng);
            crate::game::npc::step_physics(&mut f, &tiles);
            lowest = lowest.max(f.position.1);
        }
        assert!(
            lowest < 60.0 * TILE,
            "it should have stayed above the floor, got down to {lowest}"
        );
    }

    /// Each one is a slightly different size, chosen once and then kept.
    #[test]
    fn every_firefly_is_its_own_size() {
        let tiles = meadow(60);
        let mut rng = SmallRng::seed_from_u64(5);
        let w = world(&tiles, None);
        let mut sizes = Vec::new();
        for _ in 0..20 {
            let mut f = bug(FIREFLY, 0, 50);
            firefly(&mut f, &w, &mut rng);
            let first = f.scale;
            for _ in 0..50 {
                firefly(&mut f, &w, &mut rng);
            }
            assert_eq!(f.scale, first, "its size should not drift");
            sizes.push(first);
        }
        assert!(
            sizes.iter().any(|s| *s != sizes[0]),
            "they should not all be identical: {sizes:?}"
        );
        assert!(
            sizes.iter().all(|s| (0.75..=1.10).contains(s)),
            "and all in range: {sizes:?}"
        );
    }

    /// A shimmerfly bolts from anything alive rather than drifting past it.
    ///
    /// Well inside the world, because near an edge it is busy turning away from that instead and
    /// never looks around for company.
    #[test]
    fn a_shimmerfly_bolts_from_company() {
        let tiles = meadow(320);
        let w = world(&tiles, None);
        let run = |company: &[(f32, f32, f32)]| {
            // A fresh generator each time: two bugs sharing one stream diverge for reasons that
            // have nothing to do with the thing under test.
            let mut rng = SmallRng::seed_from_u64(677);
            let mut fly = bug(SHIMMERFLY_TYPE, 200, 300);
            let mut w = World { ..w };
            w.avoid = company;
            let mut jump: f32 = 0.0;
            for _ in 0..20 {
                let before = fly.velocity;
                firefly(&mut fly, &w, &mut rng);
                jump = jump.max(fly.velocity.0 - before.0);
            }
            jump
        };

        let (cx, cy) = bug(SHIMMERFLY_TYPE, 200, 300).center();
        let quiet = run(&[]);
        let crowded = run(&[(cx - 40.0, cy, SHIMMERFLY_SHY_OF_NPCS)]);
        assert!(
            crowded > quiet + 1.0,
            "company on the left should shove it right, hard: {crowded} vs {quiet}"
        );

        // B6: vanilla counts a hostile only within a hundred pixels and a player within a hundred
        // and fifty (`NPC.cs:34444-34453`). Before this the loop took every entry in the list at
        // any distance, so a shimmerfly bolted every fifteen ticks for as long as anything hostile
        // existed anywhere in the loaded world.
        let far = run(&[(cx - 4000.0, cy, SHIMMERFLY_SHY_OF_NPCS)]);
        assert!(
            (far - quiet).abs() < 1e-6,
            "something four thousand pixels away is not company: {far} vs {quiet}"
        );
        // The two reaches are different numbers, and the entry carries its own: a player at 120
        // pixels registers where a hostile at the same distance does not.
        let hostile_at_120 = run(&[(cx - 120.0, cy, SHIMMERFLY_SHY_OF_NPCS)]);
        let player_at_120 = run(&[(cx - 120.0, cy, SHIMMERFLY_SHY_OF_PLAYERS)]);
        assert!(
            (hostile_at_120 - quiet).abs() < 1e-6,
            "a hostile at 120 is outside its hundred-pixel reach"
        );
        assert!(
            player_at_120 > quiet + 1.0,
            "a player at 120 is inside its hundred-and-fifty: {player_at_120} vs {quiet}"
        );
    }
}
