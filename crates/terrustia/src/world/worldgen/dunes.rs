//! Surface sand dunes: the rolling hills a desert is made of, rather than a flat sand rectangle.
//!
//! Transcribed from `DunesBiome` (`.scratch/decompiled/Terraria.GameContent.Biomes/DunesBiome.cs`,
//! all 162 lines) and the `SurfaceMap` it depends on
//! (`.scratch/decompiled/Terraria.GameContent.Biomes.Desert/SurfaceMap.cs`, 75 lines), driven from
//! `WorldGen.cs:11573` inside the desert/pyramid pass. Tuning comes from vanilla's shipped config
//! (`Terraria.GameContent.WorldBuilding.Configuration.json`: `HeightScale` 1.0,
//! `SingleDunesWidth` 150-250, unscaled).
//!
//! The tenth of the fifteen `MicroBiome` classes. `pyramids.rs` already disclosed that it sources
//! pyramid sites from `layout.desert` directly rather than from this class; that stays true, and
//! this adds the dune shaping that was genuinely missing.
//!
//! # How a dune is drawn
//!
//! Each dune is a run of overlapping hills, and each hill is two quadratic Bezier curves sharing a
//! peak that is pushed downwind. Under every point of the curve, sand is filled from the curve
//! down to a depth that tapers toward the dune's edges (`sqrt(distance from edge) * 3`), and
//! anything non-sand in the ten tiles above is cleared out. Wind direction is one coin flip per
//! dune and decides which way every peak in it leans.
//!
//! # One deliberate guard vanilla does not have
//!
//! `PlaceCurvedLine` steps its parameter by `0.5 / (end.X - start.X)`. When the peak is pushed far
//! enough that `end.X <= start.X`, that step is zero or negative and vanilla's `for` loop never
//! terminates - a real hang, reachable only for a narrow hill whose peak offset exceeds its half
//! width. Here the loop is bounded by its own maximum step count and stops. `AGENTS.md` rule 6
//! says a generation pass may not take the process down, and an infinite loop is worse than a
//! panic: it hangs with no message.

use terrustia_proto::Tile;

use super::layout::Layout;
use super::rand::UnifiedRandom;
use super::structure_map::{Rect, StructureMap};
use crate::world::World;

/// `SingleDunesWidth`, unscaled.
const SINGLE_DUNES_WIDTH: (i32, i32) = (150, 250);
/// `HeightScale`.
const HEIGHT_SCALE: f64 = 1.0;
/// Sand.
const SAND: u16 = 53;
/// `TileID.Sets.Clouds` (`TileID.cs:197`). A cloud is not ground, so the surface scan looks past it.
const CLOUDS: [u16; 6] = [189, 196, 460, 717, 718, 719];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wind {
    Left,
    Right,
}

/// `SurfaceMap`: the first solid, non-cloud tile in each column of a span.
struct SurfaceMap {
    heights: Vec<i32>,
    x: i32,
}

impl SurfaceMap {
    /// `SurfaceMap.FromArea` (`SurfaceMap.cs:43-74`).
    fn from_area(world: &World, start_x: i32, width: i32) -> Self {
        let half = world.height() / 2;
        let mut heights = Vec::with_capacity(width.max(0) as usize);
        for i in start_x..start_x + width {
            let mut found = false;
            let mut height = 0;
            for j in 50..50 + half {
                let t = world.tile(i, j);
                if t.is_active() {
                    if CLOUDS.contains(&t.block) {
                        found = false;
                    } else if !found {
                        height = j;
                        found = true;
                    }
                }
                if !found {
                    height = half + 50;
                }
            }
            heights.push(height);
        }
        Self {
            heights,
            x: start_x,
        }
    }

    /// Vanilla indexes this without a bounds check; clamping keeps a hill whose curve wanders past
    /// the sampled span reading a real height instead of faulting.
    fn at(&self, absolute_x: i32) -> i32 {
        if self.heights.is_empty() {
            return 0;
        }
        let idx = (absolute_x - self.x).clamp(0, self.heights.len() as i32 - 1);
        self.heights[idx as usize]
    }
}

/// `DunesDescription`.
struct Dune {
    area: Rect,
    surface: SurfaceMap,
    wind: Wind,
}

impl Dune {
    /// `DunesDescription.CreateFromPlacement` (`DunesBiome.cs:26-35`).
    fn new(
        world: &World,
        rand: &mut UnifiedRandom,
        origin: (i32, i32),
        width: i32,
        height: i32,
    ) -> Self {
        let area = Rect::new(origin.0 - width / 2, origin.1 - height / 2, width, height);
        let surface = SurfaceMap::from_area(world, area.x - 20, area.width + 40);
        let wind = if rand.next_max(2) != 0 {
            Wind::Right
        } else {
            Wind::Left
        };
        Self {
            area,
            surface,
            wind,
        }
    }

    fn center_x(&self) -> i32 {
        self.area.x + self.area.width / 2
    }
}

/// `DunesBiome.Place` (`:52-63`). Two dunes straddling the origin. Always returns true, as vanilla
/// does: the hills are drawn unconditionally once a site is chosen.
pub fn place(
    world: &mut World,
    structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
    origin: (i32, i32),
) -> bool {
    let height_a = (f64::from(rand.next_range(60, 100)) * HEIGHT_SCALE) as i32;
    let height_b = (f64::from(rand.next_range(60, 100)) * HEIGHT_SCALE) as i32;
    let width_a = rand.next_range(SINGLE_DUNES_WIDTH.0, SINGLE_DUNES_WIDTH.1 + 1);
    let width_b = rand.next_range(SINGLE_DUNES_WIDTH.0, SINGLE_DUNES_WIDTH.1 + 1);

    let a = Dune::new(
        world,
        rand,
        (origin.0 - width_a / 2 + 30, origin.1),
        width_a,
        height_a,
    );
    let b = Dune::new(
        world,
        rand,
        (origin.0 + width_b / 2 - 30, origin.1),
        width_b,
        height_b,
    );
    place_single(world, structures, rand, &a);
    place_single(world, structures, rand, &b);
    true
}

/// `PlaceSingle` (`:65-86`): a run of small hills, then one or two large ones over the middle.
fn place_single(
    world: &mut World,
    structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
    dune: &Dune,
) {
    let count = rand.next_max(3) + 8;
    for i in 0..count - 1 {
        let span = (2.0 / f64::from(count) * f64::from(dune.area.width)) as i32;
        let mut x = (f64::from(i) / f64::from(count) * f64::from(dune.area.width)
            + f64::from(dune.area.x)) as i32
            + span * 2 / 5;
        x += rand.next_range(-5, 6);
        let t = f64::from(i) / f64::from(count - 2);
        let peak = 1.0 - (t - 0.5).abs() * 2.0;
        place_hill(
            world,
            rand,
            dune,
            x - span / 2,
            x + span / 2,
            (peak * 0.3 + 0.2) * HEIGHT_SCALE,
        );
    }
    let big = rand.next_max(2) + 1;
    for _ in 0..big {
        let half = dune.area.width / 2;
        let mut x = dune.center_x();
        x += rand.next_range(-10, 11);
        place_hill(
            world,
            rand,
            dune,
            x - half / 2,
            x + half / 2,
            0.8 * HEIGHT_SCALE,
        );
    }
    structures.add_structure(dune.area, 20);
}

/// `PlaceHill` (`:88-106`): a peak between two ends, leaned downwind, drawn as two curves.
fn place_hill(
    world: &mut World,
    rand: &mut UnifiedRandom,
    dune: &Dune,
    start_x: i32,
    end_x: i32,
    scale: f64,
) {
    let start = (start_x, dune.surface.at(start_x));
    let end = (end_x, dune.surface.at(end_x));
    let mut peak = (
        (start.0 + end.0) / 2,
        (start.1 + end.1) / 2 - (35.0 * scale) as i32,
    );
    let max_lean = (end.0 - peak.0) / 4;
    let min_lean = (end.0 - peak.0) / 16;
    // Vanilla's `Next(min, max + 1)` faults when min > max; both derive from the same difference,
    // so they can only invert when it is negative, which a zero-width hill produces.
    if min_lean <= max_lean {
        let lean = rand.next_range(min_lean, max_lean + 1);
        match dune.wind {
            Wind::Left => peak.0 -= lean,
            Wind::Right => peak.0 += lean,
        }
    }
    let down = (0, (scale * 12.0) as i32);
    let up = (down.0 / -2, down.1 / -2);
    let (first, second) = match dune.wind {
        Wind::Left => (down, up),
        Wind::Right => (up, down),
    };
    place_curved_line(world, dune, start, peak, first);
    place_curved_line(world, dune, peak, end, second);
}

fn lerp(a: (f64, f64), b: (f64, f64), t: f64) -> (f64, f64) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// `PlaceCurvedLine` (`:108-161`): a quadratic Bezier, filling sand under every point it visits.
fn place_curved_line(
    world: &mut World,
    dune: &Dune,
    start: (i32, i32),
    end: (i32, i32),
    anchor_offset: (i32, i32),
) {
    let anchor = (
        (start.0 + end.0) / 2 + anchor_offset.0,
        (start.1 + end.1) / 2 + anchor_offset.1,
    );
    let a = (f64::from(start.0), f64::from(start.1));
    let b = (f64::from(end.0), f64::from(end.1));
    let c = (f64::from(anchor.0), f64::from(anchor.1));
    let step = 0.5 / (b.0 - a.0);
    // See the module doc: vanilla hangs here for a backwards span. The bound is the number of
    // steps a forward span could ever need, so it never truncates a curve vanilla would finish.
    if !step.is_finite() || step <= 0.0 {
        return;
    }
    let max_steps = ((1.0 / step).ceil() as i64 + 2).max(0);

    let mut last: Option<(i32, i32)> = None;
    let mut t = 0.0;
    let mut taken = 0i64;
    while t <= 1.0 && taken <= max_steps {
        taken += 1;
        let p = lerp(lerp(a, c, t), lerp(c, b, t), t);
        let point = (p.0 as i32, p.1 as i32);
        t += step;
        if Some(point) == last {
            continue;
        }
        last = Some(point);

        // Depth tapers toward the dune's edges.
        let from_edge = dune.area.width / 2 - (point.0 - dune.center_x()).abs();
        let floor = dune.surface.at(point.0) + (f64::from(from_edge.max(0)).sqrt() * 3.0) as i32;

        for y in point.1 - 10..point.1 {
            let t = world.tile(point.0, y);
            if t.is_active() && t.block != SAND {
                world.set_tile(point.0, y, Tile::AIR);
            }
        }
        for y in point.1..floor {
            world.set_tile(point.0, y, Tile::block(SAND));
        }
    }
}

/// `MaximumWidth` (`:50`), for a caller sizing a site.
pub fn maximum_width() -> i32 {
    SINGLE_DUNES_WIDTH.1 * 2
}

/// Shape the surface desert this world laid out.
///
/// Vanilla drives `DunesBiome` from inside the desert/pyramid pass, choosing sites with
/// `RandomWorldPoint` and rejecting any too near the jungle, the world centre or the snow
/// (`WorldGen.cs:11574-11590`). This generator does not scatter deserts that way: `layout.desert`
/// is a single decided band, so the dune field is placed over it rather than searched for. The
/// rejection tests have nothing to reject here, which is why they have no counterpart.
pub fn scatter(
    world: &mut World,
    layout: &Layout,
    structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
) -> usize {
    let x = layout.desert.from + layout.desert.width() / 2;
    if layout.desert.width() < 100 || x - maximum_width() < 0 || x + maximum_width() >= layout.width
    {
        return 0;
    }
    if place(world, structures, rand, (x, layout.surface)) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat stone shelf with air above it: the surface map should find its top row.
    fn shelf(w: i32, h: i32, surface: i32) -> World {
        let mut world = World::empty(w, h, "dunes");
        for x in 0..w {
            for y in surface..h {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        world
    }

    #[test]
    fn the_surface_map_finds_the_first_solid_row() {
        let world = shelf(400, 600, 180);
        let map = SurfaceMap::from_area(&world, 0, 300);
        assert_eq!(map.at(0), 180);
        assert_eq!(map.at(299), 180);
    }

    /// A cloud is not ground: the scan keeps going past it.
    #[test]
    fn the_surface_map_looks_past_clouds() {
        let mut world = shelf(400, 600, 180);
        for y in 90..95 {
            world.set_tile(10, y, Tile::block(189));
        }
        let map = SurfaceMap::from_area(&world, 0, 300);
        assert_eq!(map.at(10), 180, "the cloud must not count as the surface");
    }

    #[test]
    fn a_dune_lays_down_real_sand_above_the_old_surface() {
        let mut world = shelf(1200, 600, 200);
        let mut structures = StructureMap::new();
        let mut rand = UnifiedRandom::new(31337);
        assert!(place(&mut world, &mut structures, &mut rand, (600, 200)));

        let mut sand = 0;
        for x in 300..900 {
            for y in 150..220 {
                if world.tile(x, y).block == SAND {
                    sand += 1;
                }
            }
        }
        assert!(sand > 500, "expected a real dune, found {sand} sand tiles");
    }

    /// The backwards-span case vanilla hangs on. A zero-width hill drives the peak offset past the
    /// end, which makes the step non-positive.
    #[test]
    fn a_backwards_span_returns_instead_of_hanging() {
        let mut world = shelf(400, 600, 200);
        let dune = Dune {
            area: Rect::new(100, 180, 100, 40),
            surface: SurfaceMap::from_area(&world, 80, 140),
            wind: Wind::Left,
        };
        // end.X < start.X: vanilla's loop would step by a negative amount and never finish.
        place_curved_line(&mut world, &dune, (200, 200), (150, 200), (0, 0));
    }

    #[test]
    fn dunes_are_reproducible_from_the_seed() {
        let run = || {
            let mut world = shelf(1200, 600, 200);
            let mut structures = StructureMap::new();
            let mut rand = UnifiedRandom::new(808);
            place(&mut world, &mut structures, &mut rand, (600, 200));
            let mut fingerprint = Vec::new();
            for x in (300..900).step_by(3) {
                for y in (150..250).step_by(3) {
                    fingerprint.push(world.tile(x, y).block);
                }
            }
            fingerprint
        };
        assert_eq!(run(), run());
    }
}
