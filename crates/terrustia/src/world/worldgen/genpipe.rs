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

use terrustia_proto::{Liquid, Tile, TileFlags};

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
}

impl<'a> Ctx<'a> {
    pub fn new(world: &'a mut World, rand: &'a mut UnifiedRandom) -> Self {
        Self {
            world,
            rand,
            out: Vec::new(),
            scan: HashMap::new(),
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
    /// `Shapes.Slime` (`:88-141`): a rounded dome over a shallower lower half.
    Slime {
        radius: i32,
        x_scale: f64,
        y_scale: f64,
    },
    /// `Shapes.Mound` (`:203-236`): a parabola of columns rising from the origin.
    Mound { half_width: i32, height: i32 },
    /// `ModShapes.All` (`ModShapes.cs:7-25`).
    All(ShapeData),
    /// `ModShapes.InnerOutline` (`:67-104`): the points of a shape that touch its edge.
    InnerOutline(ShapeData),
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
    gen_inner(ctx, origin, shape, chain, false)
}

fn gen_inner(
    ctx: &mut Ctx,
    origin: (i32, i32),
    shape: &Shape,
    chain: &mut [Link],
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
                    if !run(chain, ctx, origin, i, j) && quit_on_fail {
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
                    if !run(chain, ctx, origin, j, i) && quit_on_fail {
                        return false;
                    }
                }
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
                    if !run(chain, ctx, origin, j, i) && quit_on_fail {
                        return false;
                    }
                }
            }
            for k in (oy + 1)..=(oy + (r * y_scale * 0.5) as i32 - 1) {
                let d = f64::from(k - oy) * (2.0 / y_scale);
                let half = (r * x_scale).min(x_scale * (f64::from(num2) - d * d).sqrt()) as i32;
                for l in (ox - half)..=(ox + half) {
                    if !run(chain, ctx, origin, l, k) && quit_on_fail {
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
                    if !run(chain, ctx, origin, i + ox, oy - j) && quit_on_fail {
                        return false;
                    }
                }
            }
        }
        Shape::All(data) => {
            for (dx, dy) in data.iter() {
                if !run(chain, ctx, origin, dx + ox, dy + oy) && quit_on_fail {
                    return false;
                }
            }
        }
        Shape::InnerOutline(data) => {
            for (dx, dy) in data.iter() {
                let on_edge = POINT_OFFSETS
                    .iter()
                    .any(|(px, py)| !data.contains(dx + px, dy + py));
                if on_edge && !run(chain, ctx, origin, dx + ox, dy + oy) && quit_on_fail {
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
