//! Vanilla's shape/modifier/action generation pipeline, in the subset this generator uses.
//!
//! Transcribed from `Terraria.WorldBuilding`: `GenShape`/`GenAction` (the two 33- and 44-line
//! abstract bases that define how the pieces compose), `Shapes.cs` (238), `Modifiers.cs` (665),
//! `Actions.cs` (637), `ModShapes.cs` (106) and `WorldUtils.Gen` (`WorldUtils.cs:36-44`), plus
//! `ActionVines` and `ActionGrass` from `Terraria.GameContent.Generation`.
//!
//! # Why this exists, when the project twice decided it did not need to
//!
//! `micro_biomes.rs`'s own module doc records the finding that reversed the earlier calls: reading
//! all 15 `MicroBiome` classes showed that *every one of them* leans on this pipeline, and that the
//! blotchy, dithered edge it produces "is not an optional decorative detail on top of these
//! biomes, it is most of what makes each one look like its own biome rather than a geometric
//! primitive". That module then shipped six biomes as plain circles and rectangles and said so.
//!
//! Building the pipeline is what unblocks the rest. Of the seven `MicroBiome` classes still not
//! ported, five (`EnchantedSwordBiome`, `MahoganyTreeBiome`, `MiningExplosivesBiome`, `HiveBiome`,
//! `DeadMansChestBiome`) are shaped almost entirely as calls into it, so the framework is the
//! shared dependency rather than a per-biome cost. This is deliberately *not* the whole ~2,260-line
//! framework: it is the operations those biomes actually call, and it grows when a biome needs an
//! operation it does not yet have.
//!
//! # What is faithful, and the two places it deliberately is not
//!
//! Faithful, including the parts that look like bugs:
//!
//! * `Blotches` draws a `NextDouble()` and **throws it away** before drawing the one it tests
//!   (`Modifiers.cs:127-128`). Keeping the discarded draw matters: every later draw in the world
//!   comes from the same stream, so dropping it would shift everything downstream.
//! * `ActionVines` draws its length *before* testing whether it can place anything
//!   (`ActionVines.cs:23`), so a vine that cannot grow still consumes a draw.
//! * `Expand` and `Blotches` return "did every unit succeed", not "did any", and a modifier that
//!   rejects a tile returns failure rather than silently continuing.
//!
//! Deliberately different, both following calls this generator already made:
//!
//! * **Framing is dropped.** `SetFrames`, `ClearTile(frameNeighbors)` and `PlaceWall(neighbors)`
//!   all call into `TileFrame`/`SquareWallFrame`, which are client-side auto-tile repaints; a
//!   dedicated server never computes them, since a client draws a block's connectivity from its
//!   neighbours alone. `pyramids.rs` and `tile_cleanup.rs` state the same reasoning for the same
//!   functions. `SetFrames` is still a real link in a chain, because its other job - recording
//!   into a `ShapeData` via `Output` - is load-bearing.
//! * **Out-of-bounds reads and writes are dropped, not faulted.** Vanilla indexes `Main.tile`
//!   directly and would throw; `World::tile` returns air outside the world and `World::set_tile`
//!   returns false, so a shape overhanging the edge trims instead of taking the process down
//!   (`AGENTS.md` rule 6).

use std::collections::HashMap;

use terrustia_proto::{Liquid, Tile, TileFlags, tile_solid};

use super::rand::UnifiedRandom;
use super::shape_data::ShapeData;
use crate::world::World;

/// Everything a chain needs to touch: the world, the world RNG, and the `ShapeData` slots that
/// `Output` writes into.
///
/// The slots exist because vanilla hands the same `ShapeData` to more than one action in a chain
/// (`EnchantedSwordBiome` gives one to both a `ClearTile` and a `SetTile`), which is a shared
/// mutable borrow that a `&mut ShapeData` per action cannot express. An index into a vector owned
/// by the context can.
pub struct Ctx<'a> {
    pub world: &'a mut World,
    pub rand: &'a mut UnifiedRandom,
    out: Vec<ShapeData>,
    /// Where `TileScanner` accumulates. Vanilla passes a dictionary per scanner; only one scanner
    /// is ever live at a time in the biomes ported here, so one map is enough.
    scan: HashMap<u16, i32>,
    /// Where `ShapeBranch` records its limb tips, which is where the caller puts leaves.
    pub branch_ends: Vec<(i32, i32)>,
    /// `Actions.Scanner`'s counters. Vanilla hands each one a `Ref<int>`.
    counters: Vec<i32>,
}

impl<'a> Ctx<'a> {
    pub fn new(world: &'a mut World, rand: &'a mut UnifiedRandom) -> Self {
        Self {
            world,
            rand,
            out: Vec::new(),
            scan: HashMap::new(),
            counters: Vec::new(),
            branch_ends: Vec::new(),
        }
    }

    /// Reserve an output slot for `Output(...)` to write into.
    pub fn slot(&mut self) -> usize {
        self.out.push(ShapeData::new());
        self.out.len() - 1
    }

    /// Reserve a slot pre-loaded with an existing shape, for chains that keep filling one.
    pub fn slot_from(&mut self, data: ShapeData) -> usize {
        self.out.push(data);
        self.out.len() - 1
    }

    pub fn shape(&self, slot: usize) -> &ShapeData {
        &self.out[slot]
    }

    pub fn take(&mut self, slot: usize) -> ShapeData {
        std::mem::take(&mut self.out[slot])
    }

    pub fn scan_reset(&mut self) {
        self.scan.clear();
    }

    /// The count `TileScanner` accumulated for one tile id, zero if it saw none.
    pub fn scan_count(&self, id: u16) -> i32 {
        self.scan.get(&id).copied().unwrap_or(0)
    }

    /// Reserve a counter for `Actions.Scanner`.
    pub fn counter(&mut self) -> usize {
        self.counters.push(0);
        self.counters.len() - 1
    }

    pub fn counted(&self, slot: usize) -> i32 {
        self.counters[slot]
    }
}

/// One link in an action chain: what to do, and optionally a `ShapeData` slot to record into.
///
/// Vanilla's `GenAction` carries `NextAction` and `OutputData` on every instance and `Chain` wires
/// the list together (`Actions.cs:624-631`). A slice of links plus `split_first_mut` says the same
/// thing without the pointer chasing, and makes the "continue down the chain" step explicit.
pub struct Link {
    action: Action,
    out: Option<usize>,
}

impl Link {
    pub fn new(action: Action) -> Self {
        Self { action, out: None }
    }

    /// Vanilla's `GenAction.Output(ShapeData)`.
    pub fn out(mut self, slot: usize) -> Self {
        self.out = Some(slot);
        self
    }
}

/// Sugar for building a chain, matching how the biome code reads in C#.
pub fn chain<const N: usize>(links: [Link; N]) -> Vec<Link> {
    links.into_iter().collect()
}

pub enum Action {
    // ---- Modifiers: forward to the rest of the chain, or fail ----
    /// `Modifiers.Blotches` (`Modifiers.cs:92-153`). The organic edge: with `chance`, splatter the
    /// unit over a random rectangle around itself instead of applying it in place.
    Blotches {
        min_x: i32,
        min_y: i32,
        max_x: i32,
        max_y: i32,
        chance: f64,
    },
    /// `Modifiers.Expand` (`:32-62`).
    Expand { x: i32, y: i32 },
    /// `Modifiers.OnlyTiles` (`:238-262`): pass only where an active tile is one of these.
    OnlyTiles(Vec<u16>),
    /// `Modifiers.SkipTiles` (`:392-416`): pass unless an active tile is one of these.
    SkipTiles(Vec<u16>),
    /// `Modifiers.IsEmpty` (`:539-549`).
    IsEmpty,
    /// `Modifiers.RectangleMask` (`:575-601`), bounds inclusive and relative to the origin.
    RectangleMask {
        x_min: i32,
        x_max: i32,
        y_min: i32,
        y_max: i32,
    },
    /// `Modifiers.Offset` (`:603-619`): shift the unit before the rest of the chain sees it.
    Offset { x: i32, y: i32 },

    // ---- Actions: do something, then continue ----
    /// `Actions.TileScanner` (`Actions.cs:60-112`). Counts active tiles of the given ids into the
    /// context; ids not listed are ignored, and every listed id reads back as at least 0.
    TileScanner(Vec<u16>),
    /// `Actions.ClearTile` (`:155-169`). Framing dropped, see the module doc.
    ClearTile,
    /// `Actions.SetTile` (`:203-236`). Framing dropped.
    SetTile(u16),
    /// `Actions.PlaceWall` (`:524-549`). Wall framing dropped.
    PlaceWall(u16),
    /// `Actions.SetLiquid` (`:551-569`).
    SetLiquid { kind: Liquid, value: u8 },
    /// `Actions.SetFrames` (`:592-606`). The framing itself is dropped; this stays because
    /// `Output` on it is load-bearing.
    SetFrames,
    /// `ActionVines` (`Terraria.GameContent.Generation/ActionVines.cs`). Grows a vine downward
    /// while tiles are empty. Always draws its length, even when it grows nothing.
    Vines { min: i32, max: i32, id: u16 },
    /// `ActionGrass` (`Terraria.GameContent.Generation/ActionGrass.cs`). Places grass or its
    /// jungle counterpart on an empty tile with empty space above.
    Grass,
    /// `Modifiers.IsSolid` (`Modifiers.cs:551-561`): pass only on an active, solid tile.
    IsSolid,
    /// `Modifiers.SkipWalls` (`Modifiers.cs:461-481`): pass unless the wall is one of these.
    SkipWalls(Vec<u16>),
    /// `Actions.RemoveWall` (`Actions.cs:515-522`).
    RemoveWall,
    /// `Actions.Scanner` (`Actions.cs:44-58`): count the units that reach it, into a counter slot.
    /// Vanilla passes a `Ref<int>`; a slot index says the same thing without the shared borrow.
    Scanner(usize),
}

/// Run a chain from its head. `true` when nothing in it failed.
fn run(chain: &mut [Link], ctx: &mut Ctx, origin: (i32, i32), x: i32, y: i32) -> bool {
    let Some((head, rest)) = chain.split_first_mut() else {
        return true;
    };
    apply(head, rest, ctx, origin, x, y)
}

/// Vanilla's `GenAction.UnitApply`: record into the output shape, then continue down the chain.
fn unit_apply(
    link: &Link,
    rest: &mut [Link],
    ctx: &mut Ctx,
    origin: (i32, i32),
    x: i32,
    y: i32,
) -> bool {
    if let Some(slot) = link.out {
        ctx.out[slot].add(x - origin.0, y - origin.1);
    }
    run(rest, ctx, origin, x, y)
}

fn apply(
    link: &mut Link,
    rest: &mut [Link],
    ctx: &mut Ctx,
    origin: (i32, i32),
    x: i32,
    y: i32,
) -> bool {
    match &mut link.action {
        Action::Blotches {
            min_x,
            min_y,
            max_x,
            max_y,
            chance,
        } => {
            let (min_x, min_y, max_x, max_y, chance) = (*min_x, *min_y, *max_x, *max_y, *chance);
            // The discarded draw is vanilla's, not a slip. See the module doc.
            ctx.rand.next_double();
            if ctx.rand.next_double() < chance {
                let x0 = ctx.rand.next_range(1 - min_x, 1);
                let x1 = ctx.rand.next_range(0, max_x);
                let y0 = ctx.rand.next_range(1 - min_y, 1);
                let y1 = ctx.rand.next_range(0, max_y);
                let mut failed = false;
                for i in x0..=x1 {
                    for j in y0..=y1 {
                        failed |= !unit_apply(link, rest, ctx, origin, x + i, y + j);
                    }
                }
                !failed
            } else {
                unit_apply(link, rest, ctx, origin, x, y)
            }
        }
        Action::Expand { x: ex, y: ey } => {
            let (ex, ey) = (*ex, *ey);
            let mut failed = false;
            for i in -ex..=ex {
                for j in -ey..=ey {
                    failed |= !unit_apply(link, rest, ctx, origin, x + i, y + j);
                }
            }
            !failed
        }
        Action::OnlyTiles(types) => {
            let t = ctx.world.tile(x, y);
            if t.is_active() && types.contains(&t.block) {
                unit_apply(link, rest, ctx, origin, x, y)
            } else {
                false
            }
        }
        Action::SkipTiles(types) => {
            let t = ctx.world.tile(x, y);
            if t.is_active() && types.contains(&t.block) {
                false
            } else {
                unit_apply(link, rest, ctx, origin, x, y)
            }
        }
        Action::IsEmpty => {
            if ctx.world.tile(x, y).is_active() {
                false
            } else {
                unit_apply(link, rest, ctx, origin, x, y)
            }
        }
        Action::RectangleMask {
            x_min,
            x_max,
            y_min,
            y_max,
        } => {
            let inside = x >= *x_min + origin.0
                && x <= *x_max + origin.0
                && y >= *y_min + origin.1
                && y <= *y_max + origin.1;
            if inside {
                unit_apply(link, rest, ctx, origin, x, y)
            } else {
                false
            }
        }
        Action::Offset { x: ox, y: oy } => {
            let (ox, oy) = (*ox, *oy);
            unit_apply(link, rest, ctx, origin, x + ox, y + oy)
        }
        Action::TileScanner(ids) => {
            let t = ctx.world.tile(x, y);
            // Every listed id reads back as at least 0, matching `Output`'s own priming.
            for &id in ids.iter() {
                ctx.scan.entry(id).or_insert(0);
            }
            if t.is_active() && ids.contains(&t.block) {
                *ctx.scan.entry(t.block).or_insert(0) += 1;
            }
            unit_apply(link, rest, ctx, origin, x, y)
        }
        Action::ClearTile => {
            if ctx.world.in_bounds(x, y) {
                ctx.world.set_tile(x, y, Tile::AIR);
            }
            unit_apply(link, rest, ctx, origin, x, y)
        }
        Action::SetTile(ty) => {
            let ty = *ty;
            if ctx.world.in_bounds(x, y) {
                let mut t = Tile::AIR;
                // Vanilla clears everything but wiring and actuators, then sets type and active.
                // Walls and liquids live on the same tile here, so carry them across rather than
                // dropping them: `Tile::Clear(~(Wiring|Actuator))` keeps neither, but every call
                // site in these biomes sets tiles into already-cleared space.
                let old = ctx.world.tile(x, y);
                t.wall = old.wall;
                t.liquid = old.liquid;
                t.liquid_kind = old.liquid_kind;
                t.block = ty;
                t.flags = TileFlags(TileFlags::ACTIVE);
                ctx.world.set_tile(x, y, t);
            }
            unit_apply(link, rest, ctx, origin, x, y)
        }
        Action::PlaceWall(ty) => {
            let ty = *ty;
            if ctx.world.in_bounds(x, y) {
                let mut t = ctx.world.tile(x, y);
                t.wall = ty;
                ctx.world.set_tile(x, y, t);
            }
            unit_apply(link, rest, ctx, origin, x, y)
        }
        Action::SetLiquid { kind, value } => {
            let (kind, value) = (*kind, *value);
            if ctx.world.in_bounds(x, y) {
                let mut t = ctx.world.tile(x, y);
                t.liquid_kind = kind;
                t.liquid = value;
                ctx.world.set_tile(x, y, t);
            }
            unit_apply(link, rest, ctx, origin, x, y)
        }
        Action::SetFrames => unit_apply(link, rest, ctx, origin, x, y),
        Action::Vines { min, max, id } => {
            let (min, max, id) = (*min, *max, *id);
            // Drawn before the placement test, as vanilla does.
            let length = ctx.rand.next_range(min, max + 1);
            let mut grown = 0;
            while grown < length && !ctx.world.tile(x, y + grown).is_active() {
                if !ctx.world.in_bounds(x, y + grown) {
                    break;
                }
                let mut t = ctx.world.tile(x, y + grown);
                t.block = id;
                t.flags = TileFlags(t.flags.0 | TileFlags::ACTIVE);
                ctx.world.set_tile(x, y + grown, t);
                grown += 1;
            }
            if grown > 0 {
                unit_apply(link, rest, ctx, origin, x, y)
            } else {
                false
            }
        }
        Action::IsSolid => {
            let t = ctx.world.tile(x, y);
            if t.is_active() && tile_solid::solid(t.block) {
                unit_apply(link, rest, ctx, origin, x, y)
            } else {
                false
            }
        }
        Action::Scanner(slot) => {
            let slot = *slot;
            ctx.counters[slot] += 1;
            unit_apply(link, rest, ctx, origin, x, y)
        }
        Action::SkipWalls(walls) => {
            if walls.contains(&ctx.world.tile(x, y).wall) {
                false
            } else {
                unit_apply(link, rest, ctx, origin, x, y)
            }
        }
        Action::RemoveWall => {
            if ctx.world.in_bounds(x, y) {
                let mut t = ctx.world.tile(x, y);
                t.wall = 0;
                ctx.world.set_tile(x, y, t);
            }
            unit_apply(link, rest, ctx, origin, x, y)
        }
        Action::Grass => {
            if ctx.world.tile(x, y).is_active() || ctx.world.tile(x, y - 1).is_active() {
                return false;
            }
            // `Utils.SelectRandom(random, [3, 73])`: plain grass or its jungle counterpart.
            let choices = [3u16, 73u16];
            let pick = choices[ctx.rand.next_max(choices.len() as i32) as usize];
            if ctx.world.in_bounds(x, y) {
                let mut t = Tile::AIR;
                let old = ctx.world.tile(x, y);
                t.wall = old.wall;
                t.block = pick;
                t.flags = TileFlags(TileFlags::ACTIVE);
                ctx.world.set_tile(x, y, t);
            }
            unit_apply(link, rest, ctx, origin, x, y)
        }
    }
}

/// The shapes a chain can be run over. `All` and `InnerOutline` are `ModShapes`, which walk a
/// `ShapeData` rather than computing geometry.
pub enum Shape {
    /// `Shapes.Rectangle` (`Shapes.cs:143-179`), `[left, right)` x `[top, bottom)` from the origin.
    Rectangle {
        left: i32,
        top: i32,
        width: i32,
        height: i32,
    },
    /// `Shapes.Circle` (`:7-48`).
    Circle { h_radius: i32, v_radius: i32 },
    /// `ShapeRunner` (`Terraria.GameContent.Generation/ShapeRunner.cs`, 98 lines): a blob that
    /// wanders under a drifting velocity while shrinking, which is how vanilla digs a tunnel that
    /// looks dug rather than drawn.
    Runner {
        strength: f64,
        steps: i32,
        velocity: (f64, f64),
    },
    /// `Shapes.Slime` (`:88-141`): a rounded dome over a shallower lower half.
    Slime {
        radius: i32,
        x_scale: f64,
        y_scale: f64,
    },
    /// `Shapes.Mound` (`:203-236`): a parabola of columns rising from the origin.
    Mound { half_width: i32, height: i32 },
    /// `ShapeBranch` (`Terraria.GameContent.Generation/ShapeBranch.cs`, 94 lines): a limb from the
    /// origin to an offset, with smaller limbs forking off it. Records where each limb ends, which
    /// is where the caller puts leaves.
    Branch { angle: f64, distance: f64 },
    /// `ShapeRoot` (`Terraria.GameContent.Generation/ShapeRoot.cs`, 55 lines): a tapering root that
    /// wanders as it goes, pulled back toward straight down.
    Root {
        angle: f64,
        distance: f64,
        starting_size: f64,
        ending_size: f64,
    },
    /// `ModShapes.All` (`ModShapes.cs:7-25`).
    All(ShapeData),
    /// `ModShapes.InnerOutline` (`:67-104`): the points of a shape that touch its edge.
    InnerOutline(ShapeData),
}

/// `ShapeBranch.PerformSegment`: a `size`-wide bundle of Bresenham lines from `start` to `end`.
fn segment(
    ctx: &mut Ctx,
    chain: &mut [Link],
    out: Option<usize>,
    origin: (i32, i32),
    start: (i32, i32),
    end: (i32, i32),
    size: i32,
) {
    let size = size.max(1);
    for i in -(size >> 1)..(size - (size >> 1)) {
        for j in -(size >> 1)..(size - (size >> 1)) {
            plot_line(ctx, chain, out, origin, (start.0 + i, start.1 + j), end);
        }
    }
}

/// `Utils.PlotLine`, the ordinary integer Bresenham walk.
fn plot_line(
    ctx: &mut Ctx,
    chain: &mut [Link],
    out: Option<usize>,
    origin: (i32, i32),
    from: (i32, i32),
    to: (i32, i32),
) {
    let (mut x, mut y) = from;
    let dx = (to.0 - x).abs();
    let dy = -(to.1 - y).abs();
    let sx = if x < to.0 { 1 } else { -1 };
    let sy = if y < to.1 { 1 } else { -1 };
    let mut err = dx + dy;
    // A line is at most the world's diagonal; the bound stops a degenerate call spinning.
    let mut guard = dx.max(-dy) + 2;
    loop {
        shape_unit(chain, ctx, out, origin, x, y);
        if (x == to.0 && y == to.1) || guard <= 0 {
            return;
        }
        guard -= 1;
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// `Utils.RandomVector2D(random, min, max)`. Two draws, x then y - the order matters.
fn random_vector(rand: &mut UnifiedRandom, min: f64, max: f64) -> (f64, f64) {
    let x = rand.next_double() * (max - min) + min;
    let y = rand.next_double() * (max - min) + min;
    (x, y)
}

/// `WorldUtils.WireLine` (`WorldUtils.cs:111-131`): an L of red wire from `start` to `end`,
/// horizontal along the start's row and vertical down the end's column.
pub fn wire_line(world: &mut World, start: (i32, i32), end: (i32, i32)) {
    let (x0, x1) = (start.0.min(end.0), start.0.max(end.0));
    let (y0, y1) = (start.1.min(end.1), start.1.max(end.1));
    for x in x0..=x1 {
        place_wire(world, x, start.1);
    }
    for y in y0..=y1 {
        place_wire(world, end.0, y);
    }
}

fn place_wire(world: &mut World, x: i32, y: i32) {
    if !world.in_bounds(x, y) {
        return;
    }
    let mut t = world.tile(x, y);
    t.flags = TileFlags(t.flags.0 | TileFlags::WIRE_RED);
    world.set_tile(x, y, t);
}

/// The eight neighbour offsets `ModShapes` uses, in vanilla's own order.
const POINT_OFFSETS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// `WorldUtils.Gen(origin, shape, action)` (`WorldUtils.cs:36-39`).
///
/// `quit_on_fail` is vanilla's `GenShape.QuitOnFail`, off by default: a chain that rejects a unit
/// normally just moves on to the next one.
pub fn gen_shape(ctx: &mut Ctx, origin: (i32, i32), shape: &Shape, chain: &mut [Link]) -> bool {
    gen_inner(ctx, origin, shape, chain, None, false)
}

/// `WorldUtils.Gen(origin, shape.Output(data), action)`: as [`gen_shape`], and the shape also
/// records every unit it visits into `slot`, whatever the chain then does with it.
///
/// This is vanilla's `GenShape.Output` (`GenShape.cs:22-26`), which records in `UnitApply` before
/// the action runs - so a unit the chain rejects is still in the shape.
pub fn gen_shape_out(
    ctx: &mut Ctx,
    origin: (i32, i32),
    shape: &Shape,
    chain: &mut [Link],
    slot: usize,
) -> bool {
    gen_inner(ctx, origin, shape, chain, Some(slot), false)
}

/// `GenShape.UnitApply` (`GenShape.cs:14-21`): record into the shape's own output, then run the
/// action chain.
fn shape_unit(
    chain: &mut [Link],
    ctx: &mut Ctx,
    out: Option<usize>,
    origin: (i32, i32),
    x: i32,
    y: i32,
) -> bool {
    if let Some(slot) = out {
        ctx.out[slot].add(x - origin.0, y - origin.1);
    }
    run(chain, ctx, origin, x, y)
}

fn gen_inner(
    ctx: &mut Ctx,
    origin: (i32, i32),
    shape: &Shape,
    chain: &mut [Link],
    out: Option<usize>,
    quit_on_fail: bool,
) -> bool {
    let (ox, oy) = origin;
    match shape {
        Shape::Rectangle {
            left,
            top,
            width,
            height,
        } => {
            for i in (ox + left)..(ox + left + width) {
                for j in (oy + top)..(oy + top + height) {
                    if !shape_unit(chain, ctx, out, origin, i, j) && quit_on_fail {
                        return false;
                    }
                }
            }
        }
        Shape::Circle { h_radius, v_radius } => {
            let num = (h_radius + 1) * (h_radius + 1);
            for i in (oy - v_radius)..=(oy + v_radius) {
                let scaled = f64::from(*h_radius) / f64::from(*v_radius) * f64::from(i - oy);
                let half = (*h_radius).min((f64::from(num) - scaled * scaled).sqrt() as i32);
                for j in (ox - half)..=(ox + half) {
                    if !shape_unit(chain, ctx, out, origin, j, i) && quit_on_fail {
                        return false;
                    }
                }
            }
        }
        Shape::Runner {
            strength,
            steps,
            velocity,
        } => {
            let mut remaining = f64::from(*steps);
            let total = f64::from(*steps);
            let mut power = *strength;
            let mut at = (f64::from(ox), f64::from(oy));
            // Vanilla rolls a random direction only when none was given.
            let mut vel = if *velocity == (0.0, 0.0) {
                random_vector(ctx.rand, -1.0, 1.0)
            } else {
                *velocity
            };
            while remaining > 0.0 && power > 0.0 {
                power = strength * (remaining / total);
                remaining -= 1.0;
                let x0 = 1.max((at.0 - power * 0.5) as i32);
                let y0 = 1.max((at.1 - power * 0.5) as i32);
                let x1 = ctx.world.width().min((at.0 + power * 0.5) as i32);
                let y1 = ctx.world.height().min((at.1 + power * 0.5) as i32);
                for i in x0..x1 {
                    for j in y0..y1 {
                        // The jitter term is drawn per tile, so the blob's edge is ragged. It is
                        // also why this shape consumes far more RNG than its size suggests.
                        let wobble = 1.0 + f64::from(ctx.rand.next_range(-10, 11)) * 0.015;
                        if (f64::from(i) - at.0).abs() + (f64::from(j) - at.1).abs()
                            < power * 0.5 * wobble
                        {
                            shape_unit(chain, ctx, out, origin, i, j);
                        }
                    }
                }
                let stride = (power / 50.0) as i32 + 1;
                remaining -= f64::from(stride);
                at = (at.0 + vel.0, at.1 + vel.1);
                for _ in 0..stride {
                    at = (at.0 + vel.0, at.1 + vel.1);
                    let d = random_vector(ctx.rand, -0.5, 0.5);
                    vel = (vel.0 + d.0, vel.1 + d.1);
                }
                let d = random_vector(ctx.rand, -0.5, 0.5);
                vel = (
                    (vel.0 + d.0).clamp(-1.0, 1.0),
                    (vel.1 + d.1).clamp(-1.0, 1.0),
                );
            }
        }
        Shape::Slime {
            radius,
            x_scale,
            y_scale,
        } => {
            let r = f64::from(*radius);
            let num2 = (radius + 1) * (radius + 1);
            for i in (oy - (r * y_scale) as i32)..=oy {
                let d = f64::from(i - oy) / y_scale;
                let half = (r * x_scale).min(x_scale * (f64::from(num2) - d * d).sqrt()) as i32;
                for j in (ox - half)..=(ox + half) {
                    if !shape_unit(chain, ctx, out, origin, j, i) && quit_on_fail {
                        return false;
                    }
                }
            }
            for k in (oy + 1)..=(oy + (r * y_scale * 0.5) as i32 - 1) {
                let d = f64::from(k - oy) * (2.0 / y_scale);
                let half = (r * x_scale).min(x_scale * (f64::from(num2) - d * d).sqrt()) as i32;
                for l in (ox - half)..=(ox + half) {
                    if !shape_unit(chain, ctx, out, origin, l, k) && quit_on_fail {
                        return false;
                    }
                }
            }
        }
        Shape::Mound { half_width, height } => {
            let w = f64::from(*half_width);
            for i in -*half_width..=*half_width {
                let columns = (*height).min(
                    ((0.0 - f64::from(height + 1) / (w * w))
                        * (f64::from(i) + w)
                        * (f64::from(i) - w)) as i32,
                );
                for j in 0..columns {
                    if !shape_unit(chain, ctx, out, origin, i + ox, oy - j) && quit_on_fail {
                        return false;
                    }
                }
            }
        }
        Shape::Branch { angle, distance } => {
            let off = (
                (angle.cos() * distance) as i32,
                (angle.sin() * distance) as i32,
            );
            let len = (f64::from(off.0).powi(2) + f64::from(off.1).powi(2)).sqrt();
            let size = (len / 6.0) as i32;
            let tip = (ox + off.0, oy + off.1);
            ctx.branch_ends.push(tip);
            segment(ctx, chain, out, origin, (ox, oy), tip, size);

            let forks = (len / 8.0) as i32;
            for i in 0..forks {
                let t = (f64::from(i) + 1.0) / (f64::from(forks) + 1.0);
                let base = ((t * f64::from(off.0)) as i32, (t * f64::from(off.1)) as i32);
                let arm = (f64::from(off.0 - base.0), f64::from(off.1 - base.1));
                let turn = (ctx.rand.next_double() * 0.5 + 1.0)
                    * if ctx.rand.next_max(2) != 0 { 1.0 } else { -1.0 };
                let (sin, cos) = turn.sin_cos();
                let rotated = (
                    (arm.0 * cos - arm.1 * sin) * 0.75,
                    (arm.0 * sin + arm.1 * cos) * 0.75,
                );
                let tip2 = (
                    rotated.0 as i32 + base.0 + ox,
                    rotated.1 as i32 + base.1 + oy,
                );
                ctx.branch_ends.push(tip2);
                segment(
                    ctx,
                    chain,
                    out,
                    origin,
                    (base.0 + ox, base.1 + oy),
                    tip2,
                    size - 1,
                );
            }
        }
        Shape::Root {
            angle,
            distance,
            starting_size,
            ending_size,
        } => {
            let target = *angle;
            let mut a = *angle;
            let (mut px, mut py) = (f64::from(ox), f64::from(oy));
            let mut travelled = 0.0f64;
            while travelled < distance * 0.85 {
                let t = travelled / distance;
                let size = starting_size + (ending_size - starting_size) * t;
                px += a.cos();
                py += a.sin();
                // The angle is nudged randomly, then pulled back toward a clamped band and toward
                // straight down as the root gets further from the trunk.
                a += f64::from(ctx.rand.next_float()) - 0.5
                    + f64::from(ctx.rand.next_float())
                        * (target - std::f64::consts::FRAC_PI_2)
                        * 0.1
                        * (1.0 - t);
                let band = 2.0 * (1.0 - 0.5 * t);
                a = a * 0.4
                    + 0.45 * a.clamp(target - band, target + band)
                    + (target + (std::f64::consts::FRAC_PI_2 - target) * t) * 0.15;
                for i in 0..size as i32 {
                    for j in 0..size as i32 {
                        if !shape_unit(chain, ctx, out, origin, px as i32 + i, py as i32 + j)
                            && quit_on_fail
                        {
                            return false;
                        }
                    }
                }
                travelled += 1.0;
            }
        }
        Shape::All(data) => {
            for (dx, dy) in data.iter() {
                if !shape_unit(chain, ctx, out, origin, dx + ox, dy + oy) && quit_on_fail {
                    return false;
                }
            }
        }
        Shape::InnerOutline(data) => {
            for (dx, dy) in data.iter() {
                let on_edge = POINT_OFFSETS
                    .iter()
                    .any(|(px, py)| !data.contains(dx + px, dy + py));
                if on_edge && !shape_unit(chain, ctx, out, origin, dx + ox, dy + oy) && quit_on_fail
                {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;

    fn world() -> World {
        World::empty(200, 200, "genpipe")
    }

    #[test]
    fn a_rectangle_sets_exactly_its_own_area() {
        let mut w = world();
        let mut r = UnifiedRandom::new(1);
        let mut ctx = Ctx::new(&mut w, &mut r);
        let mut c = chain([Link::new(Action::SetTile(1))]);
        gen_shape(
            &mut ctx,
            (100, 100),
            &Shape::Rectangle {
                left: 0,
                top: 0,
                width: 4,
                height: 3,
            },
            &mut c,
        );
        assert!(ctx.world.tile(100, 100).is_active());
        assert!(ctx.world.tile(103, 102).is_active());
        // One past each edge stays empty: the rectangle is half-open, as vanilla's is.
        assert!(!ctx.world.tile(104, 100).is_active());
        assert!(!ctx.world.tile(100, 103).is_active());
    }

    #[test]
    fn only_tiles_stops_the_chain_on_anything_else() {
        let mut w = world();
        w.set_tile(50, 50, Tile::block(1));
        w.set_tile(51, 50, Tile::block(2));
        let mut r = UnifiedRandom::new(1);
        let mut ctx = Ctx::new(&mut w, &mut r);
        let mut c = chain([
            Link::new(Action::OnlyTiles(vec![1])),
            Link::new(Action::SetTile(9)),
        ]);
        gen_shape(
            &mut ctx,
            (50, 50),
            &Shape::Rectangle {
                left: 0,
                top: 0,
                width: 2,
                height: 1,
            },
            &mut c,
        );
        assert_eq!(ctx.world.tile(50, 50).block, 9, "the id 1 tile passed");
        assert_eq!(
            ctx.world.tile(51, 50).block,
            2,
            "the id 2 tile was rejected"
        );
    }

    #[test]
    fn output_records_offsets_relative_to_the_origin() {
        let mut w = world();
        let mut r = UnifiedRandom::new(1);
        let mut ctx = Ctx::new(&mut w, &mut r);
        let slot = ctx.slot();
        let mut c = chain([Link::new(Action::SetTile(1)).out(slot)]);
        gen_shape(
            &mut ctx,
            (60, 70),
            &Shape::Rectangle {
                left: -1,
                top: -1,
                width: 2,
                height: 2,
            },
            &mut c,
        );
        let data = ctx.take(slot);
        assert_eq!(data.count(), 4);
        assert!(data.contains(-1, -1));
        assert!(data.contains(0, 0));
    }

    /// The discarded `NextDouble` is the whole point: a `Blotches` that drew once would leave the
    /// RNG one step ahead of vanilla for the rest of world generation.
    #[test]
    fn blotches_consumes_two_draws_per_unit_not_one() {
        let mut w = world();
        let mut r = UnifiedRandom::new(12345);
        // A chance of 0 means the second draw can never pass, so every unit costs exactly the two
        // draws and nothing else.
        let mut probe = UnifiedRandom::new(12345);
        for _ in 0..2 {
            probe.next_double();
        }
        let after_two = probe.next();

        let mut ctx = Ctx::new(&mut w, &mut r);
        let mut c = chain([
            Link::new(Action::Blotches {
                min_x: 2,
                min_y: 2,
                max_x: 2,
                max_y: 2,
                chance: 0.0,
            }),
            Link::new(Action::SetTile(1)),
        ]);
        gen_shape(
            &mut ctx,
            (30, 30),
            &Shape::Rectangle {
                left: 0,
                top: 0,
                width: 1,
                height: 1,
            },
            &mut c,
        );
        assert_eq!(
            ctx.rand.next(),
            after_two,
            "one unit of Blotches must consume exactly two draws"
        );
    }

    /// A vine that cannot grow still costs a draw.
    #[test]
    fn vines_draw_their_length_even_where_nothing_can_grow() {
        let mut w = world();
        w.set_tile(40, 40, Tile::block(1));
        let mut r = UnifiedRandom::new(999);
        let mut probe = UnifiedRandom::new(999);
        probe.next_range(3, 6);
        let after_one = probe.next();

        let mut ctx = Ctx::new(&mut w, &mut r);
        let mut c = chain([Link::new(Action::Vines {
            min: 3,
            max: 5,
            id: 382,
        })]);
        gen_shape(
            &mut ctx,
            (40, 40),
            &Shape::Rectangle {
                left: 0,
                top: 0,
                width: 1,
                height: 1,
            },
            &mut c,
        );
        assert_eq!(ctx.rand.next(), after_one);
        assert_eq!(
            ctx.world.tile(40, 40).block,
            1,
            "the solid tile is untouched"
        );
    }

    #[test]
    fn inner_outline_is_the_edge_and_not_the_interior() {
        let mut w = world();
        let mut r = UnifiedRandom::new(1);
        let mut solid = ShapeData::new();
        solid.add_bounds(-2, -2, 2, 2);
        let mut ctx = Ctx::new(&mut w, &mut r);
        let mut c = chain([Link::new(Action::SetTile(5))]);
        gen_shape(&mut ctx, (80, 80), &Shape::InnerOutline(solid), &mut c);
        assert_eq!(ctx.world.tile(78, 78).block, 5, "a corner is on the edge");
        assert_eq!(ctx.world.tile(80, 78).block, 5, "a side is on the edge");
        assert!(
            !ctx.world.tile(80, 80).is_active(),
            "the centre is interior and must be skipped"
        );
    }

    /// A shape hanging over the world edge trims rather than panicking.
    #[test]
    fn a_shape_over_the_edge_writes_what_fits_and_does_not_panic() {
        let mut w = world();
        let mut r = UnifiedRandom::new(1);
        let mut ctx = Ctx::new(&mut w, &mut r);
        let mut c = chain([Link::new(Action::SetTile(1))]);
        gen_shape(
            &mut ctx,
            (1, 1),
            &Shape::Circle {
                h_radius: 6,
                v_radius: 6,
            },
            &mut c,
        );
        assert!(ctx.world.tile(1, 1).is_active());
    }
}
