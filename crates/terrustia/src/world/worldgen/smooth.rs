//! Smoothing blocky terrain into slopes, pounded half-tiles and small kills.
//!
//! Every earlier pass in this generator lays tiles on a grid; nothing about that grid says a
//! staircase should be a ramp, or that a one-tile ledge should round off instead of standing as a
//! perfect right angle. Vanilla's `SmoothWorld` (`WorldGen.cs:16519`) is the pass that goes back
//! over the finished terrain and turns those grid artefacts into the slopes, half-bricks and small
//! clearings that make a world stop looking like graph paper.
//!
//! **Transcribed, with one deliberate deviation.** Every branch below maps directly to vanilla's
//! two loops. The deviation is in what a tile is allowed to protect: vanilla's `SmoothWorld` runs
//! as generation pass 53 of 105 — before trees, chests, statues, pots, piles, fallen logs and
//! traps are placed (passes 58-91), so the only fixtures it has to avoid stranding are the ones
//! already down by then (dungeon brick, the jungle temple, demon/crimson altars, living trees).
//! Its protection (`ForbidsSloping`, `CanKillTile`'s tree-trunk and container checks) is sized for
//! that world. This generator's own pipeline runs smoothing *last*, after every one of those
//! decorations is already placed (see `worldgen/mod.rs`) — the reverse of vanilla's order. A
//! literal port of vanilla's protection here would happily pound the floor out from under a
//! statue nobody told it to protect. So [`protects_tile_below`] generalises: it keeps vanilla's
//! `ForbidsSloping` set verbatim, and adds "anything frame-important sits here" as a stand-in for
//! vanilla's chest/tree-trunk/container checks — which, in this engine, is exactly the set of
//! things `place_object` and the tree passes put down. This is deliberately more conservative than
//! vanilla immediately around an ordinary tree trunk (it does not replicate `CanKillTile`'s
//! root/branch frame exception, which depends on frame numbers this pass has no reason to know),
//! and that is the right trade: an occasional un-smoothed tile under a tree's canopy is invisible
//! to a player, a floating statue is not.
//!
//! `SolidTile`, `SlopeTile`, `PoundTile`, `KillTile` and `PlaceTile` are all transcribed only for
//! their generation-time behaviour — vanilla's own `isGeneratingOrLoadingWorld` branch skips their
//! sound effects, dust, network sync and player-nudging entirely, so none of that is here either.
//! `PlaceTile`'s general-purpose moss/grass-conversion machinery is not transcribed for the same
//! reason: both call sites in this pass only ever copy a plain terrain neighbour's type into an air
//! gap, never a type that would take that machinery's other branches.

use terrustia_proto::tile_sets::frame_important;
use terrustia_proto::tile_solid::{solid, solid_top};
use terrustia_proto::{Liquid, Tile, TileFlags};

use super::layout::Layout;
use super::rand::UnifiedRandom;
use super::tiles;
use crate::world::World;

/// `TileID.Sets.CanBeClearedDuringGeneration`'s exceptions: sandstone and hardened sand, granite,
/// marble, the three dungeon brick colours, three more from the same factory call, lihzahrd brick,
/// and the Lihzahrd Altar itself. None of these are ever a slope/pound/kill candidate.
const CANNOT_BE_CLEARED: [u16; 17] = [
    396, 400, 401, 397, 398, 399, 404, 368, 367, 41, 43, 44, 481, 482, 483, 226, 237,
];

/// `TileID.Sets.PreventsSlopesDuringGeneration`: spikes, traps, a second spike variant, living
/// wood, sandstone brick, sand stone slab, pressure plates, and two more pressure-pad types.
const PREVENTS_SLOPES: [u16; 9] = [48, 137, 232, 191, 151, 274, 135, 442, 428];

/// `WorldGen.ForbidsSloping`, verbatim: chests, the demon altar, the Lihzahrd Altar, and a
/// handful of other fixtures a real client would notice losing their floor.
const FORBIDS_SLOPING: [u16; 13] = [21, 26, 77, 88, 235, 237, 441, 467, 468, 470, 475, 488, 597];

/// `TileID.Sets.Boulders`.
const BOULDERS: [u16; 10] = [138, 484, 664, 665, 711, 712, 713, 714, 715, 716];

/// `TileID.Sets.Conversion.Sand`: sand, ebonsand, pearlsand, crimsand.
const SAND_CONVERTIBLE: [u16; 4] = [53, 112, 116, 234];

/// `WallID.UnbreakableBlockWall` — `CanKillTile`'s `wall == 350` check.
const UNBREAKABLE_BLOCK_WALL: u16 = 350;

const MUSHROOM_BLOCK: u16 = 190;
const WOOD_BLOCK: u16 = 30;
const SANDSTONE_BRICK: u16 = 151;
const SAND_STONE_SLAB: u16 = 274;
const SWITCHES: u16 = 136;
const OBSIDIAN_BRICK: u16 = 75;
const HELLSTONE_BRICK: u16 = 76;
const SHELL_PILE: u16 = 495;

/// What one pass over the world did.
#[derive(Debug, Clone, Copy, Default)]
pub struct Report {
    pub sloped: usize,
    pub pounded: usize,
    pub killed: usize,
    pub filled: usize,
}

impl Report {
    pub fn total(&self) -> usize {
        self.sloped + self.pounded + self.killed + self.filled
    }
}

fn cannot_be_cleared(block: u16) -> bool {
    CANNOT_BE_CLEARED.contains(&block)
}

fn prevents_slopes(block: u16) -> bool {
    PREVENTS_SLOPES.contains(&block)
}

fn forbids_sloping(block: u16) -> bool {
    FORBIDS_SLOPING.contains(&block)
}

fn is_boulder(block: u16) -> bool {
    BOULDERS.contains(&block)
}

/// `Tile.blockType() == 0`: a plain full block, neither half-bricked nor sloped.
fn is_plain_block(t: Tile) -> bool {
    !t.flags.has(TileFlags::HALF_BRICK) && t.slope == 0
}

/// `WorldGen.SolidTile(int, int, bool noDoors = false)`, with `noDoors` always false — no call
/// site in this pass ever passes `true`.
pub(super) fn solid_tile(world: &World, x: i32, y: i32) -> bool {
    let t = world.tile(x, y);
    t.is_active()
        && solid(t.block)
        && !solid_top(t.block)
        && !t.flags.has(TileFlags::HALF_BRICK)
        && t.slope == 0
        && !t.flags.has(TileFlags::ACTUATED)
}

/// `WorldGen.SolidOrSlopedTile`, `includePlatforms: false`: like [`solid_tile`], but a half-brick
/// or sloped tile still counts.
pub(super) fn solid_or_sloped(world: &World, x: i32, y: i32) -> bool {
    let t = world.tile(x, y);
    t.is_active() && solid(t.block) && !solid_top(t.block) && !t.flags.has(TileFlags::ACTUATED)
}

/// `WorldGen.TileEmpty`: true when there is nothing really there — genuinely inactive, or active
/// but actuated off.
fn tile_empty(world: &World, x: i32, y: i32) -> bool {
    let t = world.tile(x, y);
    !t.is_active() || t.flags.has(TileFlags::ACTUATED)
}

/// True when the tile at `(x, y - 1)` is a fixture this pass must never risk stranding. See the
/// module doc comment for why this is a deliberate generalisation of vanilla's `ForbidsSloping` /
/// `CanKillTile` protection rather than a literal port of either.
fn protects_tile_below(world: &World, x: i32, y: i32) -> bool {
    let above = world.tile(x, y - 1);
    above.is_active() && (forbids_sloping(above.block) || frame_important(above.block))
}

/// `WorldGen.CanPoundTile`, folding in the target-tile checks from `CanKillTile` that this
/// project's `CanKillTile` stand-in ([`protects_tile_below`]) does not itself make — `CanKillTile`
/// starts from an already-`active` tile, so those checks are the caller's job in vanilla too.
fn can_pound_tile(world: &World, x: i32, y: i32) -> bool {
    let t = world.tile(x, y);
    if !t.is_active() || t.wall == UNBREAKABLE_BLOCK_WALL {
        return false;
    }
    if matches!(t.block, 10 | 48 | 137 | 232 | 380 | 387 | 388 | 476 | 484) {
        return false;
    }
    if is_boulder(t.block) {
        return false;
    }
    // `isGeneratingOrLoadingWorld` is always true here, so vanilla's branch for it always runs.
    if t.block == MUSHROOM_BLOCK || t.block == WOOD_BLOCK {
        return false;
    }
    !protects_tile_below(world, x, y)
}

/// `WorldGen.SlopeTile`, generation-time behaviour only.
fn slope_tile(world: &mut World, x: i32, y: i32, slope: u8) -> bool {
    if !can_pound_tile(world, x, y) {
        return false;
    }
    let mut t = world.tile(x, y);
    t.flags.set(TileFlags::HALF_BRICK, false);
    t.slope = slope;
    world.set_tile(x, y, t);
    true
}

/// `WorldGen.PoundTile`, generation-time behaviour only.
pub(super) fn pound_tile(world: &mut World, x: i32, y: i32) -> bool {
    if !can_pound_tile(world, x, y) {
        return false;
    }
    let mut t = world.tile(x, y);
    let half = t.flags.has(TileFlags::HALF_BRICK);
    t.flags.set(TileFlags::HALF_BRICK, !half);
    world.set_tile(x, y, t);
    true
}

/// `WorldGen.KillTile`, generation-time behaviour only: clears the tile, preserves its wall and
/// liquid untouched (vanilla's `KillTile` never touches either), and replicates the one
/// generation-relevant side effect vanilla's version has — breaking hellstone below the underworld
/// line fills the gap with lava.
pub(super) fn kill_tile(world: &mut World, layout: &Layout, x: i32, y: i32) -> bool {
    let t = world.tile(x, y);
    if !t.is_active() || t.wall == UNBREAKABLE_BLOCK_WALL {
        return false;
    }
    // A frame-important tile is one cell of a multi-tile object, and clearing one cell leaves the
    // rest of it standing as a corrupt object rather than removing it. `can_pound_tile` above
    // already refuses the same ids one by one (10 doors, 48 spikes, 137 traps, and the rest);
    // this is the same rule stated once.
    //
    // Found by the dungeon's locked door disappearing between generation and the end of `build`:
    // it was placed correctly, survived every structure pass, and was cleared here.
    if frame_important(t.block) {
        return false;
    }
    if protects_tile_below(world, x, y) {
        return false;
    }
    let mut cleared = Tile::AIR;
    cleared.wall = t.wall;
    cleared.wall_color = t.wall_color;
    cleared.liquid = t.liquid;
    cleared.liquid_kind = t.liquid_kind;
    if t.block == tiles::HELLSTONE && y > layout.underworld {
        cleared.liquid_kind = Liquid::Lava;
        cleared.liquid = 128;
    }
    world.set_tile(x, y, cleared);
    true
}

/// `WorldGen.PlaceTile`, restricted to what this pass's two call sites actually need: filling an
/// inactive tile with a plain copy of a neighbour's type. Refuses a frame-important type outright
/// rather than risk placing a multi-tile object with no frame — see the module doc comment.
fn place_plain(world: &mut World, x: i32, y: i32, block: u16) -> bool {
    let mut t = world.tile(x, y);
    if t.is_active() || frame_important(block) {
        return false;
    }
    t.block = block;
    t.flags.set(TileFlags::ACTIVE, true);
    t.frame_x = -1;
    t.frame_y = -1;
    world.set_tile(x, y, t);
    true
}

/// `Tile.SmoothSlope(x, y, applyToNeighbors: false)`, the one-tile-only call the sand-conversion
/// branch below makes.
fn smooth_sand_slope(world: &mut World, x: i32, y: i32) -> bool {
    if !can_pound_tile(world, x, y) || !solid_or_sloped(world, x, y) {
        return false;
    }
    let has_presence_above = !tile_empty(world, x, y - 1);
    let above_not_solid_but_present = !solid_or_sloped(world, x, y - 1) && has_presence_above;
    let below_solid = solid_or_sloped(world, x, y + 1);
    let left_solid = solid_or_sloped(world, x - 1, y);
    let right_solid = solid_or_sloped(world, x + 1, y);

    let code = (u8::from(has_presence_above) << 3)
        | (u8::from(below_solid) << 2)
        | (u8::from(left_solid) << 1)
        | u8::from(right_solid);

    let mut t = world.tile(x, y);
    match code {
        10 if !above_not_solid_but_present => {
            t.flags.set(TileFlags::HALF_BRICK, false);
            t.slope = 3;
        }
        9 if !above_not_solid_but_present => {
            t.flags.set(TileFlags::HALF_BRICK, false);
            t.slope = 4;
        }
        6 => {
            t.flags.set(TileFlags::HALF_BRICK, false);
            t.slope = 1;
        }
        5 => {
            t.flags.set(TileFlags::HALF_BRICK, false);
            t.slope = 2;
        }
        4 => {
            t.slope = 0;
            t.flags.set(TileFlags::HALF_BRICK, true);
        }
        _ => {
            t.flags.set(TileFlags::HALF_BRICK, false);
            t.slope = 0;
        }
    }
    world.set_tile(x, y, t);
    true
}

/// Smooth every column of `world`, once. Call last, after every other worldgen pass — including
/// decoration — has placed what it is going to place; see the module doc comment for why this
/// pass's own protection logic depends on that order being respected.
pub fn smooth(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) -> Report {
    let mut report = Report::default();
    let width = world.width();
    let height = world.height();

    // --- pass 1: slope, pound, kill and fill along vertical steps ------------------------------
    for x in 20..(width - 20) {
        for y in 20..(height - 20) {
            let cur = world.tile(x, y);
            let above = world.tile(x, y - 1);
            if (cur.is_active() && prevents_slopes(cur.block))
                || (above.is_active() && prevents_slopes(above.block))
            {
                continue;
            }

            let left = world.tile(x - 1, y);
            let right = world.tile(x + 1, y);

            if !above.is_active()
                && (!left.is_active() || left.block != SWITCHES)
                && (!right.is_active() || right.block != SWITCHES)
            {
                if solid_tile(world, x, y) && cannot_be_cleared(cur.block) {
                    // Vanilla's `else if`: the clearable-terrain branch did not run, so this tile
                    // is left exactly as it is — nothing to transcribe.
                } else if solid_tile(world, x, y) {
                    if (!left.is_active() || is_plain_block(left))
                        && (!right.is_active() || is_plain_block(right))
                    {
                        if solid_tile(world, x, y + 1) {
                            let left_below = world.tile(x - 1, y + 1);
                            let right_below = world.tile(x + 1, y + 1);
                            let above_left = world.tile(x - 1, y - 1);
                            let above_right = world.tile(x + 1, y - 1);

                            if !solid_tile(world, x - 1, y)
                                && !left_below.flags.has(TileFlags::HALF_BRICK)
                                && solid_tile(world, x - 1, y + 1)
                                && solid_tile(world, x + 1, y)
                                && !above_right.is_active()
                            {
                                if rand.next_max(2) == 0 {
                                    if slope_tile(world, x, y, 2) {
                                        report.sloped += 1;
                                    }
                                } else if pound_tile(world, x, y) {
                                    report.pounded += 1;
                                }
                            } else if !solid_tile(world, x + 1, y)
                                && !right_below.flags.has(TileFlags::HALF_BRICK)
                                && solid_tile(world, x + 1, y + 1)
                                && solid_tile(world, x - 1, y)
                                && !above_left.is_active()
                            {
                                if rand.next_max(2) == 0 {
                                    if slope_tile(world, x, y, 1) {
                                        report.sloped += 1;
                                    }
                                } else if pound_tile(world, x, y) {
                                    report.pounded += 1;
                                }
                            } else if solid_tile(world, x + 1, y + 1)
                                && solid_tile(world, x - 1, y + 1)
                                && !right.is_active()
                                && !left.is_active()
                                && pound_tile(world, x, y)
                            {
                                report.pounded += 1;
                            }

                            if solid_tile(world, x, y) {
                                // Vanilla checks these as two separate `if`/`else if` branches — a
                                // ledge overhanging to the right, or to the left — that both kill
                                // the same tile. Merged into one condition (short-circuiting `||`
                                // preserves the exact same evaluation order) because two branches
                                // with identical bodies is a clippy::if_same_then_else warning, not
                                // a behaviour difference.
                                if (solid_tile(world, x - 1, y)
                                    && solid_tile(world, x + 1, y + 2)
                                    && !right.is_active()
                                    && !right_below.is_active()
                                    && !above_left.is_active())
                                    || (solid_tile(world, x + 1, y)
                                        && solid_tile(world, x - 1, y + 2)
                                        && !left.is_active()
                                        && !left_below.is_active()
                                        && !above_right.is_active())
                                {
                                    if kill_tile(world, layout, x, y) {
                                        report.killed += 1;
                                    }
                                } else if !left_below.is_active()
                                    && !left.is_active()
                                    && solid_tile(world, x + 1, y)
                                    && solid_tile(world, x, y + 2)
                                {
                                    if rand.next_max(5) == 0 {
                                        if kill_tile(world, layout, x, y) {
                                            report.killed += 1;
                                        }
                                    } else if rand.next_max(5) == 0 {
                                        if pound_tile(world, x, y) {
                                            report.pounded += 1;
                                        }
                                    } else if slope_tile(world, x, y, 2) {
                                        report.sloped += 1;
                                    }
                                } else if !right_below.is_active()
                                    && !right.is_active()
                                    && solid_tile(world, x - 1, y)
                                    && solid_tile(world, x, y + 2)
                                {
                                    if rand.next_max(5) == 0 {
                                        if kill_tile(world, layout, x, y) {
                                            report.killed += 1;
                                        }
                                    } else if rand.next_max(5) == 0 {
                                        if pound_tile(world, x, y) {
                                            report.pounded += 1;
                                        }
                                    } else if slope_tile(world, x, y, 1) {
                                        report.sloped += 1;
                                    }
                                }
                            }
                        }

                        if solid_tile(world, x, y)
                            && !left.is_active()
                            && !right.is_active()
                            && kill_tile(world, layout, x, y)
                        {
                            report.killed += 1;
                        }
                    }
                } else if !cur.is_active()
                    && solid_tile(world, x, y + 1)
                    && world.tile(x, y + 1).block != SANDSTONE_BRICK
                    && world.tile(x, y + 1).block != SAND_STONE_SLAB
                {
                    let below = world.tile(x, y + 1);
                    let above_right = world.tile(x + 1, y - 1);
                    let above_left = world.tile(x - 1, y - 1);

                    if right.block != MUSHROOM_BLOCK
                        && right.block != 48
                        && right.block != 232
                        && solid_tile(world, x - 1, y + 1)
                        && solid_tile(world, x + 1, y)
                        && !left.is_active()
                        && !above_right.is_active()
                    {
                        let placed = if right.block == SHELL_PILE {
                            place_plain(world, x, y, right.block)
                        } else {
                            place_plain(world, x, y, below.block)
                        };
                        if placed {
                            report.filled += 1;
                        }
                        if rand.next_max(2) == 0 {
                            if slope_tile(world, x, y, 2) {
                                report.sloped += 1;
                            }
                        } else if pound_tile(world, x, y) {
                            report.pounded += 1;
                        }
                    }
                    if left.block != MUSHROOM_BLOCK
                        && left.block != 48
                        && left.block != 232
                        && solid_tile(world, x + 1, y + 1)
                        && solid_tile(world, x - 1, y)
                        && !right.is_active()
                        && !above_left.is_active()
                    {
                        let placed = if left.block == SHELL_PILE {
                            place_plain(world, x, y, left.block)
                        } else {
                            place_plain(world, x, y, below.block)
                        };
                        if placed {
                            report.filled += 1;
                        }
                        if rand.next_max(2) == 0 {
                            if slope_tile(world, x, y, 1) {
                                report.sloped += 1;
                            }
                        } else if pound_tile(world, x, y) {
                            report.pounded += 1;
                        }
                    }
                }
            } else if !world.tile(x, y + 1).is_active()
                && rand.next_max(2) == 0
                && solid_tile(world, x, y)
                && solid_tile(world, x, y - 1)
                && (!right.is_active() || is_plain_block(right))
                && (!left.is_active() || is_plain_block(left))
            {
                // As above: vanilla's two branches (an unsupported left corner vs. an unsupported
                // right corner) have identical bodies, so they are merged into one `||` — the
                // short-circuit evaluates the second `slope_tile` call only when the first
                // condition as a whole (including its own `slope_tile` call) was false, exactly
                // matching the original `if`/`else if`.
                if (solid_tile(world, x - 1, y)
                    && !solid_tile(world, x + 1, y)
                    && solid_tile(world, x - 1, y - 1)
                    && slope_tile(world, x, y, 3))
                    || (solid_tile(world, x + 1, y)
                        && !solid_tile(world, x - 1, y)
                        && solid_tile(world, x + 1, y - 1)
                        && slope_tile(world, x, y, 4))
                {
                    report.sloped += 1;
                }
            }
        }
    }

    // --- pass 2: fill in remaining single-tile slope corners, smooth sand, and clean up orphaned
    // slopes left over from anything the first pass (or an earlier one) removed a neighbour of ----
    for x in 20..(width - 20) {
        for y in 20..(height - 20) {
            let cur = world.tile(x, y);
            if rand.next_max(2) == 0
                && !world.tile(x, y - 1).is_active()
                && !matches!(
                    cur.block,
                    137 | MUSHROOM_BLOCK
                        | 232
                        | 191
                        | SANDSTONE_BRICK
                        | SAND_STONE_SLAB
                        | OBSIDIAN_BRICK
                        | HELLSTONE_BRICK
                )
                && solid_tile(world, x, y)
                && (!world.tile(x - 1, y).is_active() || world.tile(x - 1, y).block != 137)
                && (world.tile(x + 1, y).is_active() || world.tile(x + 1, y).block != 137)
            {
                if solid_tile(world, x, y + 1)
                    && solid_tile(world, x + 1, y)
                    && !world.tile(x - 1, y).is_active()
                    && slope_tile(world, x, y, 2)
                {
                    report.sloped += 1;
                }
                if solid_tile(world, x, y + 1)
                    && solid_tile(world, x - 1, y)
                    && !world.tile(x + 1, y).is_active()
                    && slope_tile(world, x, y, 1)
                {
                    report.sloped += 1;
                }
            }

            let cur = world.tile(x, y);
            if cur.is_active() && SAND_CONVERTIBLE.contains(&cur.block) {
                smooth_sand_slope(world, x, y);
            }

            let cur = world.tile(x, y);
            if cur.slope == 1 && !solid_tile(world, x - 1, y) {
                if slope_tile(world, x, y, 0) {
                    report.sloped += 1;
                }
                if pound_tile(world, x, y) {
                    report.pounded += 1;
                }
            }
            let cur = world.tile(x, y);
            if cur.slope == 2 && !solid_tile(world, x + 1, y) {
                if slope_tile(world, x, y, 0) {
                    report.sloped += 1;
                }
                if pound_tile(world, x, y) {
                    report.pounded += 1;
                }
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::worldgen::layout::Layout;

    fn stone(x: i32, y: i32, world: &mut World) {
        world.set_tile(x, y, Tile::block(1));
    }

    fn test_layout(width: i32, height: i32, seed: i32) -> Layout {
        let mut rand = UnifiedRandom::new(seed);
        Layout::plan(width, height, &mut rand)
    }

    /// A one-tile-high ledge — solid ground, then a single step up, with nothing else nearby —
    /// is exactly the shape `SmoothWorld` exists to round off. On the unfixed code (a stub that
    /// does nothing) this fails outright, since nothing here is sloped, pounded or killed.
    #[test]
    fn a_one_tile_ledge_gets_smoothed() {
        let mut world = World::empty(120, 120, "smooth-ledge");
        let layout = test_layout(120, 120, 7);
        let mut rand = UnifiedRandom::new(7);

        // A flat floor at y=60 for x in 40..80, stepping up by one tile at x=60.
        for x in 40..80 {
            let step_y = if x >= 60 { 59 } else { 60 };
            for y in step_y..70 {
                stone(x, y, &mut world);
            }
        }

        let report = smooth(&mut world, &layout, &mut rand);

        assert!(
            report.total() > 0,
            "a blocky ledge should give the pass something to do"
        );
        // The corner tiles right at the step must have been touched: either sloped, pounded, or
        // cleared, rather than left as a bare right-angle step.
        let touched_at_corner = (58..62).any(|x| {
            let t = world.tile(x, 59);
            !t.is_active() || t.slope != 0 || t.flags.has(TileFlags::HALF_BRICK)
        });
        assert!(
            touched_at_corner,
            "the step at x=60 should have been sloped, pounded or cleared"
        );
    }

    /// A tile with a statue-like frame-important object sitting on it must never be pounded,
    /// sloped or killed out from under it — the deliberate deviation from vanilla's own
    /// (differently ordered) protection, documented on [`protects_tile_below`].
    #[test]
    fn a_tile_under_a_frame_important_object_is_never_touched() {
        let mut world = World::empty(120, 120, "smooth-protect");
        let layout = test_layout(120, 120, 3);
        let mut rand = UnifiedRandom::new(3);

        for x in 40..80 {
            for y in 60..70 {
                stone(x, y, &mut world);
            }
        }
        // A frame-important placed object sitting on top of the floor at (60, 59).
        world.set_tile(60, 59, Tile::framed(28, 0, 0)); // 28: Pots, a real frame-important type.
        // Make the neighbourhood asymmetric enough that the un-protected pass would have acted.
        world.set_tile(61, 59, Tile::AIR);

        let before = world.tile(60, 60);
        smooth(&mut world, &layout, &mut rand);
        let after = world.tile(60, 60);

        assert_eq!(
            before, after,
            "the floor under a frame-important object must be left exactly as it was"
        );
    }

    /// A run over a real generated world must actually change something measurable, and must
    /// never touch a Lihzahrd Altar tile (`CanBeClearedDuringGeneration`'s own named exception).
    #[test]
    fn a_real_generated_world_gets_measurably_smoothed_without_disturbing_the_altar() {
        let (mut world, built) = super::super::build(4200, 1200, "smooth-real", 909_090);
        let layout = test_layout(4200, 1200, 909_090);
        let mut rand = UnifiedRandom::new(909_090);

        let altar_before: usize = (0..world.width())
            .flat_map(|x| (0..world.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| world.tile(x, y).block == 237 && world.tile(x, y).is_active())
            .count();
        assert!(
            altar_before > 0,
            "the temple's altar must exist to test against"
        );

        let report = smooth(&mut world, &layout, &mut rand);

        let altar_after: usize = (0..world.width())
            .flat_map(|x| (0..world.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| world.tile(x, y).block == 237 && world.tile(x, y).is_active())
            .count();
        assert_eq!(
            altar_before, altar_after,
            "CanBeClearedDuringGeneration must keep the altar untouched"
        );
        assert!(
            report.total() > 100,
            "a full-size world should give the pass thousands of real tiles to work; got {}",
            report.total()
        );
        let _ = built;
    }
}
