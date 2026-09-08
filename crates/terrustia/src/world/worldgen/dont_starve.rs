//! "The Constant": long wavy caves, marble piles with statues, and a lava layer carved open.
//!
//! Transcribed from the three generation passes vanilla adds under `dontStarveWorldGen`:
//! `WavyCaves` (`WorldGen.cs:12315-12352` driving `WavyCaverer`, `:77538-77590`),
//! `MarblePiles` (`:16370-16401` driving `MarblePileWithStatues`, 112 lines) and
//! `LavaLayerCaverer` (`:14938`, the function at `:77340` or thereabouts).
//!
//! The second of the eight remaining secret seeds to get real generation content, after
//! [`super::not_the_bees`].
//!
//! # What a player finds
//!
//! Long, sinusoidal tunnels sweeping across the cavern layer - the seed's signature, and nothing
//! like the blobby caves the ordinary generator makes. Marble outcrops on the surface either side
//! of the world centre, each with up to three statues standing on it. And the water line opened
//! into a continuous horizontal gallery with lava pooled along it.
//!
//! # Scoped to this seed alone
//!
//! As with `not_the_bees`, every branch that reads another secret seed is taken with that seed
//! false: `remixWorldGen` thirds the wavy-cave count, and `tenthAnniversaryWorldGen` or
//! `remixWorldGen` suppress the lava layer entirely. Those seeds have no generation content here,
//! so this is the plain `theconstant` path.
//!
//! # Disclosed narrowings
//!
//! * `LavaLayerCaverer`'s dungeon-wall repair - the block that reads and rewrites dungeon walls
//!   either side of the gallery so it does not cut a hole in the dungeon - is dropped. This
//!   generator's dungeon is placed after this pass would run, so there is no dungeon wall here to
//!   protect; the guard would have nothing to guard.
//! * `MarblePileWithStatues` places statue type 26 through `WorldGen.Statue`, a large helper with
//!   its own siting rules. This uses the generator's own `place_object` at the same tile with the
//!   same statue style, which is the placement without the siting search.

use terrustia_proto::{Liquid, Tile, TileFlags, tile_solid};

use super::layout::Layout;
use super::place_object::place_object;
use super::rand::UnifiedRandom;
use crate::world::World;

/// Marble.
const MARBLE: u16 = 367;
/// Statue, and the style `MarblePileWithStatues` asks for.
const STATUE: u16 = 105;
const STATUE_STYLE: i32 = 26;

fn solid(world: &World, x: i32, y: i32) -> bool {
    let t = world.tile(x, y);
    t.is_active() && tile_solid::solid(t.block)
}

/// `WorldGen.WavyCaverer` (`:77538-77590`): one long sinusoidal tunnel.
///
/// The wave is not decoration. Its amplitude and frequency both random-walk as the tunnel runs, and
/// the bore's height ramps up over the first stretch and back down over the last, so the tunnel
/// opens and closes rather than ending in a wall.
pub fn wavy_caverer(
    world: &mut World,
    rand: &mut UnifiedRandom,
    start_x: i32,
    start_y: i32,
    wave_strength: f64,
    wave_percent: f64,
    steps: i32,
) {
    let leftward = start_x > world.width() / 2;
    let min_height = 2 + rand.next_max(2);
    let max_height = 15 + rand.next_max(11);
    let ramp = 1 + rand.next_max(2);
    let ramp_steps = (f64::from(max_height) / f64::from(ramp)).ceil() as i32;
    let mut amplitude = 1.0f64;
    let mut frequency = 1.0f64;
    let slope = (-1.0 + rand.next_double() * 3.0) as i32;
    let mut height = min_height;
    let mut travelled = 0i32;
    let mut x = f64::from(start_x);

    for i in 0..steps {
        let opening = i < ramp_steps;
        let closing = i >= steps - ramp_steps;
        x += if leftward { -1.0 } else { 1.0 };
        if !opening && !closing {
            travelled += 1;
            amplitude = 2.0f64.min(0.5f64.max(amplitude + (-0.5 + rand.next_double()) * 0.25));
            frequency = 1.1f64.min(0.9f64.max(frequency + (-0.5 + rand.next_double()) * 0.02));
        }
        let wave = (f64::from(travelled) * 0.1 * frequency * wave_percent).sin()
            * amplitude
            * wave_strength;
        let mut y = f64::from(start_y) + wave + f64::from(travelled * slope);

        let was = height;
        if opening {
            height = max_height.min(height + ramp);
        } else if closing {
            height = min_height.max(height - ramp);
        }
        y -= f64::from((was + height) / 4);

        for j in 0..height {
            let (tx, ty) = (x as i32, y as i32 + j);
            // Vanilla's `InWorld(x, y, 20)`: stay 20 tiles clear of every edge.
            if tx < 20 || ty < 20 || tx >= world.width() - 20 || ty >= world.height() - 20 {
                continue;
            }
            world.set_tile(tx, ty, Tile::AIR);
        }
    }
}

/// The `WavyCaves` pass (`:12315-12352`). Returns how many tunnels were cut.
pub fn wavy_caves(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) -> usize {
    let scale = f64::from(layout.width) / 4200.0;
    let count = (35.0 * scale * scale) as i32;
    if count < 1 {
        return 0;
    }
    let (top, bottom) = (layout.surface + 100, layout.underworld - 100);
    if bottom <= top {
        return 0;
    }

    let mut last_y = 0;
    let mut cut = 0;
    for i in 0..count {
        let along = f64::from(i) / f64::from((count - 1).max(1));
        // Keep successive tunnels 80 tiles apart vertically, giving up after 100 tries.
        let mut y = rand.next_range(top, bottom);
        let mut tries = 0;
        while (y - last_y).abs() < 80 {
            tries += 1;
            if tries > 100 {
                break;
            }
            y = rand.next_range(top, bottom);
        }
        last_y = y;

        let margin = 80;
        let start_x = margin + (f64::from(layout.width - margin * 2) * along) as i32;
        let strength = f64::from(12 + rand.next_range(3, 6));
        let percent = 0.25 + rand.next_double();
        let steps = rand.next_range(300, 500);
        wavy_caverer(world, rand, start_x, y, strength, percent, steps);
        cut += 1;
    }
    cut
}

/// `WorldGen.MarblePileWithStatues` (112 lines): a marble outcrop with statues on it.
pub fn marble_pile(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom, x: i32) -> bool {
    // Fall to the ground.
    let mut y = layout.surface / 2;
    while !solid(world, x, y) {
        y += 1;
        if y > layout.surface {
            return false;
        }
    }
    let ground = world.tile(x, y);
    if ground.block != 0 && ground.block != 2 {
        return false;
    }
    if ground.wall > 0 {
        return false;
    }
    // Refuse a site already near marble or living wood.
    let x0 = (x - 130).clamp(30, world.width() - 30);
    let x1 = (x + 130).clamp(30, world.width() - 30);
    let y0 = (y - 60).clamp(30, world.height() - 30);
    let y1 = (y + 60).clamp(30, world.height() - 30);
    for i in x0..=x1 {
        for j in y0..=y1 {
            let t = world.tile(i, j);
            if t.is_active() && (t.block == MARBLE || t.block == 191) {
                return false;
            }
        }
    }

    let top = y - 1;
    let mut at = (f64::from(x), f64::from(top));
    let mut drift = (
        rand.next_double() * 0.6 - 0.3,
        rand.next_double() * 0.5 + 0.5,
    );
    let mut size = f64::from(rand.next_range(2, 4));
    if rand.next_max(10) == 0 {
        size += 1.0;
    }

    let mut passes = rand.next_range(3, 6);
    while passes > 0 {
        passes -= 1;
        let mut k = x - (size as i32) * 5;
        while f64::from(k) <= f64::from(x) + size * 5.0 {
            let mut j = top + (size as i32) * 3;
            while f64::from(j) > f64::from(top) - size * 3.0 {
                let reach = size * f64::from(rand.next_range(70, 91)) * 0.01 * 1.2;
                let mut delta = (at.0 - f64::from(k), at.1 - f64::from(j));
                if (delta.0 * delta.0 + delta.1 * delta.1).sqrt() > 30.0 {
                    // Wandered too far: snap back to the trunk and re-roll the drift.
                    at = (f64::from(x), f64::from(top));
                    drift = (
                        rand.next_double() * 0.6 - 0.3,
                        rand.next_double() * 0.5 + 0.5,
                    );
                } else {
                    delta.0 *= 0.25;
                    delta.1 *= 0.8;
                    let dist = (delta.0 * delta.0 + delta.1 * delta.1).sqrt();
                    if dist < reach && world.tile(k, j).is_active() {
                        let mut t = world.tile(k, j);
                        t.block = MARBLE;
                        t.slope = 0;
                        t.frame_x = -1;
                        t.frame_y = -1;
                        t.flags = TileFlags(t.flags.0 | TileFlags::ACTIVE);
                        world.set_tile(k, j, t);
                    }
                }
                j -= 1;
            }
            k += 1;
        }
        at = (at.0 + drift.0, at.1 + drift.1);
        drift.0 += rand.next_double() * 0.2 - 0.1;
        drift.1 += (0.1 + rand.next_double() * 0.1) * 0.8;
        // Vanilla calls `Utils.Clamp` here and throws the result away, so the drift is not
        // actually clamped. Kept: removing the calls would change nothing, and keeping the shape
        // records that the clamp is dead code in the game.
        drift.0 = drift.0.clamp(-0.3, 0.3);
        drift.1 = drift.1.clamp(0.5, 1.0);
    }

    // Up to three statues on top.
    let mut placed = 0;
    let mut l = x - (size as i32) * 5;
    while f64::from(l) <= f64::from(x) + size * 5.0 {
        if placed >= 3 {
            break;
        }
        if l % 2 != 1 && (placed <= 0 || rand.next_max(5) == 0) {
            let mut sy = at.1 as i32 - 20;
            while sy < world.height() - 1 && !world.tile(l, sy).is_active() {
                sy += 1;
            }
            if world.tile(l, sy).block == MARBLE
                && !world.tile(l, sy - 1).is_active()
                && place_object(world, l, sy - 1, STATUE, STATUE_STYLE, -1)
            {
                placed += 1;
            }
        }
        l += 1;
    }
    true
}

/// The `MarblePiles` pass (`:16370-16401`): up to `5 * width/4200` piles, none near the centre.
pub fn marble_piles(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) -> usize {
    let wanted = (5.0 * (f64::from(layout.width) / 4200.0)) as i32;
    if wanted < 1 || layout.width < 400 {
        return 0;
    }
    let centre = layout.width / 2;
    let (lo, hi) = (centre - 100, centre + 100);
    let mut placed = 0usize;
    for _ in 0..80 {
        let mut x = rand.next_range(100, layout.width - 100);
        if x >= lo && x <= hi {
            x = rand.next_range(100, layout.width - 100);
            if x >= lo && x <= hi {
                continue;
            }
        }
        if marble_pile(world, layout, rand, x) {
            placed += 1;
            if placed >= wanted as usize {
                break;
            }
        }
    }
    placed
}

/// `WorldGen.LavaLayerCaverer`: the water line opened into one long gallery with lava in it.
pub fn lava_layer(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) {
    // `GenVars.waterLine` is where the cavern layer's water sits; this generator's nearest
    // equivalent is the midpoint between the rock layer and the underworld.
    let water_line = (layout.rock + layout.underworld) / 2;
    let (min_gap, max_gap, drift_cap) = (2, 8, 30);
    let mut top = water_line - 1;
    let mut bottom = water_line + 1;
    let centre_start = water_line - drift_cap;
    let mut centre = centre_start;

    let mut x = 10;
    while x < world.width() - 10 {
        x += 1;
        if rand.next_max(4) == 0 {
            centre += rand.next_range(-4, 5);
            centre = centre.clamp(centre_start - drift_cap, centre_start + drift_cap);
        }
        if rand.next_max(3) == 0 {
            top += rand.next_range(-4, 5);
            top = top.clamp(centre - max_gap, centre - min_gap);
        }
        if rand.next_max(3) == 0 {
            bottom += rand.next_range(-4, 5);
            bottom = bottom.clamp(centre + min_gap, centre + max_gap);
        }
        for y in top..=bottom {
            if !world.in_bounds(x, y) {
                continue;
            }
            world.set_tile(x, y, Tile::AIR);
            if rand.next_max(15) == 0 {
                let mut t = world.tile(x, y);
                t.liquid = u8::MAX;
                t.liquid_kind = Liquid::Lava;
                world.set_tile(x, y, t);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stone(w: i32, h: i32) -> (World, Layout) {
        let mut world = World::empty(w, h, "constant");
        let mut layout = Layout::plan(w, h, &mut UnifiedRandom::new(4));
        layout.surface = 150;
        layout.rock = 250;
        layout.underworld = h - 120;
        for x in 0..w {
            for y in 150..h {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        for x in 0..w {
            world.set_tile(x, 149, Tile::block(0));
        }
        (world, layout)
    }

    #[test]
    fn a_wavy_cave_cuts_a_long_tunnel_that_actually_waves() {
        let (mut world, _layout) = stone(1200, 800);
        let mut rand = UnifiedRandom::new(4242);
        wavy_caverer(&mut world, &mut rand, 200, 400, 14.0, 0.8, 400);

        // The tunnel exists. Counted over the whole world, not a fixed window: `WavyCaverer`
        // applies a per-step vertical slope of -1, 0 or +1, so over 400 steps the tunnel can drift
        // 400 tiles away from where it started and a fixed window clips most of it.
        let mut air = 0;
        // And its floor is not a straight line: collect the first open row per column.
        let mut tops = Vec::new();
        for x in 20..1180 {
            let mut found = None;
            // Start below the surface, and only count a gap with solid rock directly above it:
            // otherwise every column's "first open tile" is the sky.
            for y in 160..780 {
                if !world.tile(x, y).is_active() {
                    air += 1;
                    if found.is_none() && world.tile(x, y - 1).is_active() {
                        found = Some(y);
                    }
                }
            }
            if let Some(y) = found {
                tops.push(y);
            }
        }
        assert!(air > 2000, "the tunnel is too small: {air} open tiles");
        assert!(
            tops.len() > 200,
            "the tunnel is too short: {} columns",
            tops.len()
        );
        let lo = *tops.iter().min().unwrap();
        let hi = *tops.iter().max().unwrap();
        assert!(
            hi - lo > 8,
            "the tunnel did not wave: its roof spans only {} tiles",
            hi - lo
        );
    }

    #[test]
    fn a_marble_pile_lays_marble_on_the_surface() {
        let (mut world, layout) = stone(1200, 800);
        let mut rand = UnifiedRandom::new(31);
        assert!(marble_pile(&mut world, &layout, &mut rand, 300));
        let mut marble = 0;
        for x in 250..350 {
            for y in 120..200 {
                if world.tile(x, y).block == MARBLE {
                    marble += 1;
                }
            }
        }
        assert!(marble > 20, "not enough marble: {marble}");
    }

    /// A second pile refuses to sit on top of the first.
    #[test]
    fn marble_piles_refuse_to_overlap() {
        let (mut world, layout) = stone(1200, 800);
        let mut rand = UnifiedRandom::new(31);
        assert!(marble_pile(&mut world, &layout, &mut rand, 300));
        assert!(
            !marble_pile(&mut world, &layout, &mut rand, 310),
            "a pile 10 tiles from another must be refused"
        );
    }

    #[test]
    fn the_lava_layer_opens_a_gallery_with_lava_in_it() {
        let (mut world, layout) = stone(1200, 800);
        let mut rand = UnifiedRandom::new(77);
        lava_layer(&mut world, &layout, &mut rand);
        let line = (layout.rock + layout.underworld) / 2;
        let mut air = 0;
        let mut lava = 0;
        for x in 20..1180 {
            for y in line - 60..line + 20 {
                let t = world.tile(x, y);
                if !t.is_active() {
                    air += 1;
                }
                if t.liquid > 0 && t.liquid_kind == Liquid::Lava {
                    lava += 1;
                }
            }
        }
        assert!(air > 3000, "no gallery was opened: {air}");
        assert!(lava > 100, "no lava pooled in it: {lava}");
    }

    #[test]
    fn the_passes_are_reproducible_from_the_seed() {
        let run = || {
            let (mut world, layout) = stone(900, 600);
            let mut rand = UnifiedRandom::new(1001);
            wavy_caves(&mut world, &layout, &mut rand);
            marble_piles(&mut world, &layout, &mut rand);
            lava_layer(&mut world, &layout, &mut rand);
            let mut fingerprint = Vec::new();
            for x in (0..900).step_by(7) {
                for y in (100..600).step_by(7) {
                    fingerprint.push(world.tile(x, y).block);
                }
            }
            fingerprint
        };
        assert_eq!(run(), run());
    }
}
