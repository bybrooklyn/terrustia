//! The Underground Desert: a sand basin over a walled hive of chambers.
//!
//! Transcribed from `DesertBiome`
//! (`.scratch/decompiled/Terraria.GameContent.Biomes/DesertBiome.cs`, 72 lines) and the
//! `Terraria.GameContent.Biomes.Desert` sub-namespace it dispatches into: `DesertDescription`
//! (133), `SandMound` (76), `DesertHive` (507) and `PitEntrance` (73). The fifteenth and last of
//! the `MicroBiome` classes.
//!
//! `micro_biomes.rs` deferred this as "mostly a *dispatcher* to a whole separate sub-namespace
//! ... none of those classes are counted in `plan.md`'s own 4,240-line/15-class total, and porting
//! them is realistically its own Tier-2-sized item". That was right about the size, and this is
//! that item.
//!
//! # What a player finds
//!
//! A wide bowl of sand at the surface, and under it the Underground Desert proper: a cavity of
//! sandstone and hardened sand walled in sandstone brick, its interior carved into rounded chambers
//! by a field of metaballs, with lava at the bottom, cacti and pots on the ledges, and a pit at the
//! centre leading down into it.
//!
//! # How the hive is shaped
//!
//! Not by carving tunnels. A grid of `BlockColumnCount` x `BlockRowCount` cells is seeded at random
//! inside an ellipse, flood-grouped into clusters of at least four cells, and each cluster's cells
//! are jittered off their grid points. Every tile in the hive then sums `1 / distance^2` to the
//! blocks of every nearby cluster; the two strongest cluster sums decide, by four thresholds, if
//! that tile is open air, sandstone brick, hardened sand or plain sand. That is a metaball field,
//! and it is why the chambers look blown rather than dug.
//!
//! # Disclosed narrowings
//!
//! * **One entrance style of four.** Vanilla rolls between `ChambersEntrance`, `AnthillEntrance`,
//!   `LarvaHoleEntrance` and `PitEntrance`. Only the pit is ported: the other three need
//!   `Shapes.Tail` (a tapered line plot over `Utils.PlotTileTale`), `ModShapes.OuterOutline` and
//!   `Modifiers.NotInShape`, none of which any other biome here calls. The roll is still made and
//!   still consumes its draw, so the entrance appears at vanilla's own rate; three quarters of the
//!   time it is a pit where vanilla would have varied it.
//! * `Tile.SmoothSlope` is dropped, as everywhere else in this generator: no slopes are written,
//!   so there is nothing to smooth. This costs the chamber edges their bevel.
//! * The `remix`, `drunk`, `tenthAnniversary` and `surfaceIsDesert` branches are not modelled.
//! * `GenVars.UndergroundDesertLocation` is returned rather than written to a global.

use terrustia_proto::{Liquid, Tile, TileFlags, tile_solid};

use super::layout::Layout;
use super::rand::UnifiedRandom;
use super::structure_map::{Rect, StructureMap};
use crate::world::World;

/// Sand.
const SAND: u16 = 53;
/// Hardened Sand.
const HARDENED_SAND: u16 = 397;
/// Sandstone.
const SANDSTONE: u16 = 396;
/// Desert Fossil.
const FOSSIL: u16 = 404;
/// Sandstone wall, the hive's own.
const HIVE_WALL: u16 = 187;
/// Hardened sand wall, the outer shell.
const SHELL_WALL: u16 = 216;

/// `DesertDescription.DefaultBlockScale`.
const BLOCK_SCALE: (f64, f64) = (4.0, 2.0);

/// `Terraria.Utilities.FastRandom`, a 48-bit LCG. Needed because `DesertHive` seeds one from the
/// world seed and derives per-tile streams from it with `WithModifier`, which is what makes the
/// sand-versus-hardened-sand speckle stable for a given world rather than a function of draw order.
#[derive(Clone, Copy)]
pub struct FastRandom {
    seed: u64,
}

impl FastRandom {
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }

    fn next_seed(seed: u64) -> u64 {
        seed.wrapping_mul(25_214_903_917).wrapping_add(11) & 0xFFFF_FFFF_FFFF
    }

    pub fn with_modifier(&self, modifier: u64) -> Self {
        Self {
            seed: Self::next_seed(modifier) ^ self.seed,
        }
    }

    /// `WithModifier(int x, int y)`, arithmetic kept in the same widths vanilla uses.
    pub fn with_xy(&self, x: i32, y: i32) -> Self {
        let a = (i64::from(x) + 2_654_435_769i64 + (i64::from(y) << 6)) as u64;
        self.with_modifier(a.wrapping_add((y as u64) >> 2))
    }

    fn next_bits(&mut self, bits: u32) -> i32 {
        self.seed = Self::next_seed(self.seed);
        (self.seed >> (48 - bits)) as i32
    }

    pub fn next_double(&mut self) -> f64 {
        f64::from(self.next_bits(32) as f32 * 4.656_613e-10f32)
    }

    pub fn next(&mut self, max: i32) -> i32 {
        if max <= 0 {
            return 0;
        }
        if (max & max.wrapping_neg()) == max {
            return ((i64::from(max) * i64::from(self.next_bits(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next_bits(31);
            let val = bits % max;
            if bits - val + (max - 1) >= 0 {
                return val;
            }
        }
    }
}

/// `Tile.ResetToType`: become this block, active, with the frame data reset.
///
/// The frame reset is the part that matters. Overwriting a frame-important tile (a pot, a chest)
/// with plain sandstone while leaving its old `frame_x`/`frame_y` behind leaves the running world
/// holding frames that a save drops, because the format only stores them for frame-important
/// types. That is 17 tiles differing across a save, which is how this was found.
fn reset_to_type(t: &mut Tile, block: u16) {
    t.block = block;
    t.flags = TileFlags(t.flags.0 | TileFlags::ACTIVE);
    t.slope = 0;
    if terrustia_proto::tile_sets::frame_important(block) {
        t.frame_x = 0;
        t.frame_y = 0;
    } else {
        t.frame_x = -1;
        t.frame_y = -1;
    }
}

/// `DesertDescription`.
pub struct Description {
    pub combined: Rect,
    pub desert: Rect,
    pub hive: Rect,
    pub columns: i32,
    pub rows: i32,
    surface: Vec<i32>,
    surface_x: i32,
}

impl Description {
    fn surface_at(&self, x: i32) -> i32 {
        if self.surface.is_empty() {
            return 0;
        }
        let i = (x - self.surface_x).clamp(0, self.surface.len() as i32 - 1);
        self.surface[i as usize]
    }
}

/// `SurfaceMap.FromArea`, restricted to what this module needs. Shares its shape with
/// `dunes::SurfaceMap`; kept separate because that one is private to its own module and this one
/// also needs the average and the bottom.
fn surface_map(world: &World, start_x: i32, width: i32) -> (Vec<i32>, f64, i32) {
    let half = world.height() / 2;
    let mut heights = Vec::with_capacity(width.max(0) as usize);
    let mut sum = 0i64;
    let mut bottom = 0;
    for i in start_x..start_x + width {
        let mut found = false;
        let mut height = half + 50;
        for j in 50..50 + half {
            let t = world.tile(i, j);
            if t.is_active() && !matches!(t.block, 189 | 196 | 460 | 717 | 718 | 719) && !found {
                height = j;
                found = true;
            }
        }
        sum += i64::from(height);
        bottom = bottom.max(height);
        heights.push(height);
    }
    let average = if heights.is_empty() {
        0.0
    } else {
        sum as f64 / heights.len() as f64
    };
    (heights, average, bottom)
}

/// `DesertDescription.CreateFromPlacement` (`:54-91`). `None` where vanilla returns `Invalid`.
pub fn describe(
    world: &World,
    layout: &Layout,
    rand: &mut UnifiedRandom,
    origin: (i32, i32),
) -> Option<Description> {
    let scale = f64::from(layout.width) / 4200.0;
    let columns = (80.0 * scale) as i32;
    let rows = ((rand.next_double() * 0.5 + 1.5) * 170.0 * scale) as i32;
    let width = (BLOCK_SCALE.0 * f64::from(columns)) as i32;
    let height = (BLOCK_SCALE.1 * f64::from(rows)) as i32;
    if width < 10 || height < 10 {
        return None;
    }
    let x = origin.0 - width / 2;

    let (heights, average, bottom) = surface_map(world, x - 5, width + 10);
    // Refuse a site whose floor row runs through jungle, snow or ice.
    for i in x..x + width {
        let t = world.tile(i, bottom).block;
        if matches!(t, 59 | 60 | 161 | 147) {
            return None;
        }
    }

    let top = (average as i32 + bottom) / 2;
    let y = top + rand.next_range(40, 60);
    Some(Description {
        combined: Rect::new(x, top, width, y + height - top),
        hive: Rect::new(x, y, width, height),
        desert: Rect::new(x, top, width, y + height / 2 - top),
        columns,
        rows,
        surface: heights,
        surface_x: x - 5,
    })
}

/// `SandMound.Place` (`:7-50`): scoop the surface out and fill the bowl with sand.
fn sand_mound(world: &mut World, d: &Description, rand: &mut UnifiedRandom) {
    let mut bowl = d.desert;
    bowl.height = d.desert.height.min(d.hive.height / 2);
    let mut lower = d.desert;
    lower.y = bowl.bottom();
    lower.height = (d.desert.bottom() - bowl.bottom()).max(0);

    let mut wobble_a = 0;
    let mut wobble_b = 0;
    for i in -5..bowl.width + 5 {
        let mut across = (f64::from(i + 5) / f64::from(bowl.width + 10)).abs() * 2.0 - 1.0;
        across = across.clamp(-1.0, 1.0);
        if i % 3 == 0 {
            wobble_a += rand.next_range(-1, 2);
            wobble_a = wobble_a.clamp(-10, 10);
        }
        wobble_b += rand.next_range(-1, 2);
        wobble_b = wobble_b.clamp(-10, 10);

        let curve = (1.0 - across * across * across * across).max(0.0).sqrt();
        let floor = bowl.bottom() - (curve * f64::from(bowl.height)) as i32 + wobble_a;
        if across.abs() < 1.0 {
            // `Utils.UnclampedSmoothStep(0.5, 0.8, |across|)`.
            let t = (across.abs() - 0.5) / 0.3;
            let smooth = t * t * (3.0 - 2.0 * t);
            let cubed = smooth * smooth * smooth;
            let mut ceiling = 10 + (f64::from(bowl.y) - cubed * 20.0) as i32 + wobble_b;
            ceiling = ceiling.min(floor);
            for j in d.surface_at(i + d.desert.x) - 1..ceiling {
                let x = i + d.desert.x;
                if world.in_bounds(x, j) {
                    world.set_tile(x, j, Tile::AIR);
                }
            }
        }
        // `PlaceSandColumn`.
        let height = lower.bottom() - floor;
        for j in (floor..floor + height).rev() {
            if !world.in_bounds(i + d.desert.x, j) {
                continue;
            }
            let mut t = world.tile(i + d.desert.x, j);
            t.liquid = 0;
            reset_to_type(&mut t, SAND);
            world.set_tile(i + d.desert.x, j, t);
        }
    }
}

/// One jittered grid cell of a cluster.
type Block = (f64, f64);

/// `DesertHive.ClusterGroup.Generate` (`:85-195`): the metaball field's sources.
fn clusters(d: &Description, rand: &mut UnifiedRandom) -> Vec<Vec<Block>> {
    let (w, h) = (d.columns.max(2), d.rows.max(2));
    let mut seeded = vec![vec![false; h as usize]; w as usize];
    let (hw, hh) = (w / 2 - 1, h / 2 - 1);
    let r2 = (hw + 1) * (hw + 1);
    for i in -hh..=hh {
        let scaled = f64::from(hw) / f64::from(hh.max(1)) * f64::from(i);
        let span = hw.min(((f64::from(r2) - scaled * scaled).max(0.0)).sqrt() as i32);
        for j in -span..=span {
            let (x, y) = (hw + j, hh + i);
            if x >= 0 && x < w && y >= 0 && y < h {
                seeded[x as usize][y as usize] = rand.next_max(2) == 0;
            }
        }
    }

    // Flood-group into clusters, at most two steps out from each seed.
    fn search(map: &mut [Vec<bool>], out: &mut Vec<(i32, i32)>, x: i32, y: i32, level: i32) {
        out.push((x, y));
        map[x as usize][y as usize] = false;
        if level - 1 == -1 {
            return;
        }
        let (w, h) = (map.len() as i32, map[0].len() as i32);
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx >= 0 && nx < w && ny >= 0 && ny < h && map[nx as usize][ny as usize] {
                search(map, out, nx, ny, level - 1);
            }
        }
    }

    let mut groups: Vec<Vec<(i32, i32)>> = Vec::new();
    for k in 0..w {
        for l in 0..h {
            if seeded[k as usize][l as usize] && rand.next_max(2) == 0 {
                let mut points = Vec::new();
                search(&mut seeded, &mut points, k, l, 2);
                if points.len() > 2 {
                    groups.push(points);
                }
            }
        }
    }

    // Neighbouring groups either merge or are dropped, decided by a coin per contact.
    let mut owner = vec![vec![-1i32; h as usize]; w as usize];
    for (idx, group) in groups.iter().enumerate() {
        for &(x, y) in group {
            owner[x as usize][y as usize] = idx as i32;
        }
    }
    for idx in 0..groups.len() {
        for gi in 0..groups[idx].len() {
            let (x, y) = groups[idx][gi];
            if owner[x as usize][y as usize] == -1 {
                break;
            }
            let index = owner[x as usize][y as usize];
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w || ny >= h {
                    continue;
                }
                let other = owner[nx as usize][ny as usize];
                if other == -1 || other == index {
                    continue;
                }
                let claim = if rand.next_max(2) == 0 { -1 } else { index };
                for &(px, py) in &groups[other as usize] {
                    owner[px as usize][py as usize] = claim;
                }
            }
        }
    }

    let mut rebuilt: Vec<Vec<(i32, i32)>> = vec![Vec::new(); groups.len()];
    for x in 0..w {
        for y in 0..h {
            let o = owner[x as usize][y as usize];
            if o != -1 {
                rebuilt[o as usize].push((x, y));
            }
        }
    }

    let mut out = Vec::new();
    for group in rebuilt {
        if group.len() < 4 {
            continue;
        }
        let mut cluster = Vec::with_capacity(group.len());
        for (x, y) in group {
            cluster.push((
                f64::from(x) + (rand.next_double() - 0.5) * 0.5,
                f64::from(y) + (rand.next_double() - 0.5) * 0.5,
            ));
        }
        out.push(cluster);
    }
    out
}

/// `DesertHive.PlaceClustersArea` (`:226-388`): the metaball field, evaluated per tile.
fn place_clusters(
    world: &mut World,
    d: &Description,
    groups: &[Vec<Block>],
    rand: &mut UnifiedRandom,
    world_seed: u64,
) {
    let area = d.hive.inflated(20);
    let field = FastRandom::new(world_seed).with_modifier(57005);
    let scale = (f64::from(d.hive.width), f64::from(d.hive.height));
    let grid = (f64::from(d.columns), f64::from(d.rows));
    let half = (BLOCK_SCALE.0 / 2.0, BLOCK_SCALE.1 / 2.0);

    for i in area.x..area.right() {
        for j in area.y..area.bottom() {
            if !world.in_bounds(i, j) {
                continue;
            }
            let mut speckle = field;
            let block_type = if speckle.next(3) == 0 {
                HARDENED_SAND
            } else {
                SAND
            };

            let dx = i - d.hive.x;
            let dy = j - d.hive.y;
            let at = (
                (f64::from(dx) - half.0) / scale.0 * grid.0,
                (f64::from(dy) - half.1) / scale.1 * grid.1,
            );

            let mut best = 0.0;
            let mut second = 0.0;
            let mut best_index: i32 = -1;
            for (k, cluster) in groups.iter().enumerate() {
                if (cluster[0].0 - at.0).abs() > 10.0 || (cluster[0].1 - at.1).abs() > 10.0 {
                    continue;
                }
                let mut sum = 0.0;
                for b in cluster {
                    let d2 = (b.0 - at.0).powi(2) + (b.1 - at.1).powi(2);
                    if d2 > 0.0 {
                        sum += 1.0 / d2;
                    }
                }
                if sum > best {
                    if best > second {
                        second = best;
                    }
                    best = sum;
                    best_index = k as i32;
                } else if sum > second {
                    second = sum;
                }
            }
            let strength = best + second;

            let edge = (
                (f64::from(dx) - half.0) / scale.0 * 2.0 - 1.0,
                (f64::from(dy) - half.1) / scale.1 * 2.0 - 1.0,
            );
            let outside = (edge.0 * edge.0 + edge.1 * edge.1).sqrt() >= 0.8;

            let mut t = world.tile(i, j);
            if strength > 3.5 {
                // The open chamber.
                t = Tile::AIR;
                t.wall = HIVE_WALL;
                if best_index >= 0 && best_index % 15 == 2 {
                    reset_to_type(&mut t, FOSSIL);
                }
            } else if strength > 1.8 {
                t.wall = HIVE_WALL;
                if j < layout_surface(d) {
                    t.liquid = 0;
                } else {
                    t.liquid_kind = Liquid::Lava;
                }
                if !outside || t.is_active() {
                    reset_to_type(&mut t, SANDSTONE);
                }
            } else if strength > 0.7 || !outside {
                t.wall = SHELL_WALL;
                t.liquid = 0;
                if !outside || t.is_active() {
                    reset_to_type(&mut t, block_type);
                }
            } else if strength > 0.25 {
                let mut local = field.with_xy(dx, dy);
                let chance = (strength - 0.25) / 0.45;
                if local.next_double() < chance {
                    t.wall = HIVE_WALL;
                    if j < layout_surface(d) {
                        t.liquid = 0;
                    } else {
                        t.liquid_kind = Liquid::Lava;
                    }
                    if t.is_active() {
                        reset_to_type(&mut t, block_type);
                    }
                }
            } else {
                continue;
            }
            // A liquid kind is only meaningful with liquid behind it.
            if t.liquid == 0 {
                t.liquid_kind = Liquid::Water;
            }
            world.set_tile(i, j, t);
        }
    }
    let _ = rand;
}

/// The desert's own top, used where vanilla reads `Main.worldSurface`.
fn layout_surface(d: &Description) -> i32 {
    d.desert.y
}

/// `DesertHive.AddTileVariance` (`:390-478`), minus the decorative placements that need objects
/// this generator does not site underground.
fn tile_variance(world: &mut World, d: &Description) {
    for i in -20..d.hive.width + 20 {
        for j in -20..d.hive.height + 20 {
            let (x, y) = (i + d.hive.x, j + d.hive.y);
            if !world.in_bounds(x, y) {
                continue;
            }
            let t = world.tile(x, y);
            let below = world.tile(x, y + 1);
            let below2 = world.tile(x, y + 2);
            let solid_below = below.is_active() && tile_solid::solid(below.block);
            let solid_below2 = below2.is_active() && tile_solid::solid(below2.block);
            if t.block == SAND && t.is_active() && (!solid_below || !solid_below2) {
                let mut t = t;
                reset_to_type(&mut t, HARDENED_SAND);
                world.set_tile(x, y, t);
            }
        }
    }
}

/// `PitEntrance.PlaceAt` (`:17-58`): the shaft down into the hive.
fn pit_entrance(world: &mut World, d: &Description, rand: &mut UnifiedRandom) {
    let mut radius = rand.next_range(6, 9);
    let cx = d.combined.x + d.combined.width / 2;
    let cy = d.surface_at(cx);

    let span = d.hive.y - d.desert.y;
    for i in -radius - 3..radius + 3 {
        let column = d.surface_at(i + cx);
        for j in column..=d.hive.y + 10 {
            if span <= 0 {
                break;
            }
            let progress = (f64::from(j - column) / f64::from(span)).clamp(0.0, 1.0);
            // `GetHoleRadiusScaleAt`.
            let scale = if progress < 0.6 {
                1.0
            } else {
                let delta = ((progress - 0.6) / 0.4).clamp(0.0, 1.0);
                let smoother = 1.0 - (delta * std::f64::consts::PI).cos() * 0.5 - 0.5;
                (1.0 - smoother) * 0.5 + 0.5
            };
            let hole = (scale * f64::from(radius)) as i32;
            let (x, y) = (i + cx, j);
            if !world.in_bounds(x, y) {
                continue;
            }
            if i.abs() < hole {
                world.set_tile(x, y, Tile::AIR);
            } else if i.abs() < hole + 3 && progress > 0.35 {
                let mut t = world.tile(x, y);
                reset_to_type(&mut t, HARDENED_SAND);
                world.set_tile(x, y, t);
            }
            let taper = (f64::from(i) / f64::from(radius)).abs().powi(2);
            if i.abs() < hole + 3 && f64::from(j - cy) > 15.0 - 3.0 * taper {
                let mut t = world.tile(x, y);
                t.wall = HIVE_WALL;
                world.set_tile(x, y, t);
            }
        }
    }

    // A shallow crater at the lip.
    radius += 4;
    for k in -radius..radius {
        let mut depth = radius - k.abs();
        depth = 10.min(depth * depth);
        for l in 0..depth {
            let (x, y) = (k + cx, l + d.surface_at(k + cx));
            if world.in_bounds(x, y) {
                world.set_tile(x, y, Tile::AIR);
            }
        }
    }
}

/// `DesertBiome.Place` (`:13-44`). Returns the area it claimed.
pub fn place(
    world: &mut World,
    layout: &Layout,
    structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
    world_seed: u64,
    origin: (i32, i32),
) -> Option<Rect> {
    let mut d = describe(world, layout, rand, origin)?;
    sand_mound(world, &d, rand);
    let (heights, _, _) = surface_map(world, d.combined.x - 5, d.combined.width + 10);
    d.surface = heights;
    d.surface_x = d.combined.x - 5;

    // Vanilla rolls the entrance, then rolls which of four styles. Both draws are made, so the
    // entrance appears at vanilla's rate; only the style is narrowed. See the module doc.
    if rand.next_double() <= 0.3333 {
        let _style = rand.next_max(4);
        pit_entrance(world, &d, rand);
    }

    let groups = clusters(&d, rand);
    place_clusters(world, &d, &groups, rand, world_seed);
    tile_variance(world, &d);

    let area = Rect::new(
        d.combined.x,
        50,
        d.combined.width,
        (d.combined.bottom() - 20 - 50).max(1),
    );
    structures.add_structure(area, 10);
    Some(d.combined.inflated(10))
}

/// The desert is placed once, over the layout's own desert band.
pub fn scatter(
    world: &mut World,
    layout: &Layout,
    structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
    world_seed: u64,
) -> Option<Rect> {
    if layout.desert.width() < 100 || layout.width < 800 || layout.height < 400 {
        return None;
    }
    let x = layout.desert.from + layout.desert.width() / 2;
    place(
        world,
        layout,
        structures,
        rand,
        world_seed,
        (x, layout.surface),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandy(w: i32, h: i32) -> World {
        let mut world = World::empty(w, h, "desert");
        for x in 0..w {
            for y in 120..h {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        world
    }

    fn layout_for(world: &World) -> Layout {
        let mut l = Layout::plan(world.width(), world.height(), &mut UnifiedRandom::new(3));
        l.surface = 120;
        l.rock = 200;
        l.desert = super::super::layout::Band { from: 300, to: 900 };
        l
    }

    #[test]
    fn fast_random_matches_its_own_stream_twice() {
        let mut a = FastRandom::new(1234);
        let mut b = FastRandom::new(1234);
        for _ in 0..20 {
            assert_eq!(a.next(97), b.next(97));
        }
        // A modifier gives a different, still deterministic stream.
        let mut c = FastRandom::new(1234).with_xy(7, 9);
        let mut d = FastRandom::new(1234).with_xy(7, 9);
        assert_eq!(c.next(1000), d.next(1000));
    }

    #[test]
    fn a_desert_carves_a_hive_of_sandstone_with_walls_and_open_chambers() {
        let mut world = sandy(1600, 900);
        let layout = layout_for(&world);
        let mut structures = StructureMap::new();
        let mut rand = UnifiedRandom::new(31415);
        let area = scatter(&mut world, &layout, &mut structures, &mut rand, 999)
            .expect("the desert was refused");

        let mut sandstone = 0;
        let mut walls = 0;
        let mut open = 0;
        let mut sand = 0;
        for x in area.x.max(0)..area.right().min(world.width()) {
            for y in area.y.max(0)..area.bottom().min(world.height()) {
                let t = world.tile(x, y);
                if t.is_active() && t.block == SANDSTONE {
                    sandstone += 1;
                }
                if t.is_active() && (t.block == SAND || t.block == HARDENED_SAND) {
                    sand += 1;
                }
                if t.wall == HIVE_WALL || t.wall == SHELL_WALL {
                    walls += 1;
                }
                if !t.is_active() && t.wall == HIVE_WALL {
                    open += 1;
                }
            }
        }
        assert!(sand > 1000, "not enough sand: {sand}");
        assert!(sandstone > 100, "not enough sandstone: {sandstone}");
        assert!(walls > 1000, "the hive was never walled: {walls}");
        assert!(open > 100, "no open chambers were carved: {open}");
    }

    /// A site whose floor row runs through snow is refused, as vanilla refuses it.
    #[test]
    fn a_site_over_snow_is_refused() {
        let mut world = sandy(1600, 900);
        for x in 0..1600 {
            world.set_tile(x, 120, Tile::block(147));
        }
        let layout = layout_for(&world);
        let mut structures = StructureMap::new();
        let mut rand = UnifiedRandom::new(5);
        assert!(scatter(&mut world, &layout, &mut structures, &mut rand, 1).is_none());
    }

    #[test]
    fn the_desert_is_reproducible_from_the_seed() {
        let run = || {
            let mut world = sandy(1200, 700);
            let layout = layout_for(&world);
            let mut structures = StructureMap::new();
            let mut rand = UnifiedRandom::new(2718);
            scatter(&mut world, &layout, &mut structures, &mut rand, 42);
            let mut fingerprint = Vec::new();
            for x in (300..900).step_by(7) {
                for y in (100..600).step_by(7) {
                    let t = world.tile(x, y);
                    fingerprint.push((t.block, t.wall));
                }
            }
            fingerprint
        };
        assert_eq!(run(), run());
    }
}
