//! Cave wall variety: paints depth/biome-appropriate background wall onto open cavern pockets, and
//! separately fills large enclosed pockets with a wall so they don't read as bottomless black voids.
//!
//! Transcribed from two real vanilla passes:
//!
//! * `CaveWallVariety` (`WorldGen.cs:16801-16861`, 61 lines) — floods outward from a jungle-grass
//!   or plain-stone surface tile with open air above it, and if the flooded pocket is large enough
//!   and doesn't border ice/desert/mushroom/Living Wood, walls the pocket plus a one-tile fringe
//!   with a texture chosen by depth band (or, for a jungle-grass site, always the jungle texture).
//!   Uses the real `WorldUtils.Gen`/`ShapeFloodFill`/`Modifiers.IsTouching`/`ModShapes.OuterOutline`
//!   DSL in source; re-derived here as a purpose-built flood ([`flood_open`]) rather than that
//!   general framework, matching this project's standing preference (`cave_flood.rs`'s own doc
//!   comment makes the same choice for a near-identical flood).
//! * `CaveWallsInEnclosedSpaces` (`WorldGen.cs:17834-17966`, 133 lines) — finds a mid-sized enclosed
//!   pocket (10 to 1500 open tiles) and floods it with a wall chosen by what the pocket actually
//!   contains (mushroom-heavy, icy, lava-touched, or plain), reusing [`super::cave_flood::count`]'s
//!   `CaveCount` breakdown; a second loop looks specifically for a jungle-walled pocket (up to 2500
//!   tiles) and floods *that* with Mud wall instead.
//!
//! **`cave_flood::count` widened for a real second caller.** Its own doc comment said "no caller
//! passes `jungle: true` for anything built so far" — this pass's second loop is the first that
//! does. `jungle: true` in vanilla's own `nextCount` skips the wall-blocks-the-fill and
//! lava-blocks-the-fill checks entirely (both live inside `if (!jungle) { ... }`) — transcribed as
//! a `jungle: bool` parameter alongside the existing `lava_ok`, defaulting every existing call site
//! to `false` so nothing else changes.
//!
//! **A new flood-fill wall painter, `spread_wall`/`spread_wall_mud`, transcribed from
//! `Spread.Wall`/`Spread.Wall2`** (`WorldGen.cs:3297`/`3357`). `Spread.Wall2` is general-purpose in
//! source (a `stopsAtAir` flag gates a whole extra diagonal-expansion branch, per wall type via
//! `WallID.Sets.WallSpreadStopsAtAir`), but this pass's own only call site always passes wall type
//! 15 (Mud) — and `WallSpreadStopsAtAir[15]` is `false`. That makes the entire `stopsAtAir` branch
//! dead code on the one path this module actually needs, so `spread_wall_mud` below is transcribed
//! narrower: a plain capped flood with the `CannotBeReplacedByWallSpread` exclusion and the 5000-
//! tile `maxWallOut2` cap, not the general per-wall-type framework — the same "narrower, disclosed"
//! shape this project has used throughout Tier 2/3 rather than porting unreachable generality.
//!
//! **Capped rather than left to the real pocket topology, like every other flood in this session.**
//! `structures::caves()` produces one large interconnected network rather than vanilla's isolated
//! pockets, so both floods below are capped (1000 for `CaveWallVariety`'s own collection flood,
//! 1500/5000 for the two `CaveWallsInEnclosedSpaces` painters) exactly as vanilla's own local
//! `maxTileCount`/`maxWallOut2` already cap them — this project just leans on that existing cap
//! rather than needing a new one, unlike `gem_caves.rs`/`spider_caves.rs` which had to add one.
//!
//! **`GenVars.lavaLine` has no dedicated field in this project's `Layout`** — every depth check
//! against it below reads `layout.underworld` instead, the same stand-in `terrain.rs`/`structures.rs`
//! already established for "the lava/underworld threshold" (see e.g. `terrain.rs`'s own underworld
//! lava-fill using `layout.underworld` the same way), not a new substitution invented here.
//!
//! **Disclosed and skipped**: the `remixWorldGen` branch of `CaveWallsInEnclosedSpaces` (a second,
//! alternate site-search loop that never runs on an ordinary world), and the shimmer-position
//! reroll in `CaveWallVariety` (this project has no shimmer worldgen concept to avoid). Both are
//! dead code on the path an ordinary world's own generation actually reaches, the same class of cut
//! `floating_islands.rs` already made for `SnowCloudIsland`/`DesertCloudIsland`.

use terrustia_proto::tile_solid;

use super::cave_flood;
use super::layout::Layout;
use super::rand::UnifiedRandom;
use super::shape_data::ShapeData;
use super::tiles::{self, walls};
use crate::world::World;

/// `Modifiers.IsTouching`'s own invalid-neighbour list for `CaveWallVariety`, in both its jungle
/// and stone branches — see the module doc's note that vanilla's own stone-branch check (which adds
/// jungle-grass, 60, to this list) reduces to exactly this same set once `Chain`'s AND-semantics are
/// worked through, since 60 alone can never satisfy the second, narrower `IsTouching` check chained
/// after it. Snow, Ice, Sandstone, HardenedSand, MushroomGrass, Living Wood.
const INVALID_NEIGHBOURS: [u16; 6] = [
    tiles::SNOW,
    tiles::ICE,
    tiles::SANDSTONE,
    tiles::HARDENED_SAND,
    tiles::MUSHROOM_GRASS,
    191, // Living Wood — see `living_trees.rs`'s own local `LIVING_WOOD` constant.
];

/// Walls a painted pocket must never overwrite: the jungle temple's own brick, the (unsafe) hive
/// wall, and Shimmer.
const PROTECTED_WALLS: [u16; 3] = [walls::LIHZAHRD_BRICK, walls::HIVE, 244];

fn touches_invalid(world: &World, x: i32, y: i32) -> bool {
    const OFFSETS: [(i32, i32); 8] = [
        (0, -1),
        (1, 0),
        (-1, 0),
        (0, 1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ];
    OFFSETS.iter().any(|&(dx, dy)| {
        let t = world.tile(x + dx, y + dy);
        t.is_active() && INVALID_NEIGHBOURS.contains(&t.block)
    })
}

/// The collection flood `CaveWallVariety` runs before painting: `ShapeFloodFill(1000)` over
/// `Modifiers.IsNotSolid`, checking every visited tile's neighbourhood against
/// [`INVALID_NEIGHBOURS`] along the way. Returns the visited set, whether the fill completed inside
/// the 1000-tile budget (`false` once the cap is hit, matching vanilla's own `ShapeFloodFill.Perform`
/// return value), and whether any visited tile touched an invalid neighbour.
fn flood_open(world: &World, x: i32, y: i32, max_tiles: usize) -> (ShapeData, bool, bool) {
    let mut shape = ShapeData::new();
    let mut seen: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from([(x, y)]);
    let mut touched_invalid = false;
    let mut completed = true;

    while let Some((cx, cy)) = queue.pop_front() {
        if !seen.insert((cx, cy)) {
            continue;
        }
        if shape.count() >= max_tiles {
            completed = false;
            break;
        }
        if !world.in_bounds(cx, cy) {
            continue;
        }
        let here = world.tile(cx, cy);
        // `Modifiers.IsNotSolid`: not-solid means genuinely inactive, or active but not
        // `SolidOrSlopedTile`. Checking `is_active()` is not optional here — an inactive tile's
        // leftover `block` id (0, Dirt's own id) reads as solid if `tile_solid::solid` runs alone,
        // the same ordering bug already found and fixed once for `place_object.rs` and once more
        // for `gem_caves.rs`'s own flood (see that module's doc comment for the fuller account).
        if here.is_active() && tile_solid::solid(here.block) {
            continue;
        }
        shape.add(cx, cy);
        if touches_invalid(world, cx, cy) {
            touched_invalid = true;
        }
        queue.push_back((cx - 1, cy));
        queue.push_back((cx + 1, cy));
        queue.push_back((cx, cy - 1));
        queue.push_back((cx, cy + 1));
    }
    (shape, completed, touched_invalid)
}

/// `ModShapes.OuterOutline(shapeData, useDiagonals: true, useInterior: true)` +
/// `Actions.Chain(Modifiers.SkipWalls(...), Actions.PlaceWall(wall))`: walls every tile in `shape`
/// plus every 8-directional neighbour not already in `shape`, skipping [`PROTECTED_WALLS`].
fn paint_outer_outline(world: &mut World, shape: &ShapeData, wall: u16) {
    const OFFSETS: [(i32, i32); 8] = [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ];
    let mut targets: std::collections::HashSet<(i32, i32)> = shape.data().clone();
    for &(x, y) in shape.data() {
        for &(dx, dy) in &OFFSETS {
            targets.insert((x + dx, y + dy));
        }
    }
    for (x, y) in targets {
        let mut t = world.tile(x, y);
        if PROTECTED_WALLS.contains(&t.wall) {
            continue;
        }
        t.wall = wall;
        world.set_tile(x, y, t);
    }
}

/// The `CaveWallVariety` pass. Returns how many pockets were painted.
pub fn variety(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) -> usize {
    if layout.width < 40 || layout.height < 400 {
        // `RandomWorldPoint((int)worldSurface, 2, 190, 2)` needs `worldSurface..height-190` to be
        // non-empty and `2..width-2` likewise — a guard against the same `next_range` panic class
        // flagged across this session's Tier 2/3 work, not a claim any real world is this small.
        return 0;
    }

    let budget_scale = f64::from(layout.width) * f64::from(layout.height)
        / (f64::from(super::SMALL_WIDTH) * f64::from(super::SMALL_HEIGHT));
    let mut budget = (300.0 * budget_scale) as i32;
    let mut retries = 100_000i32;
    let mut painted = 0usize;
    // Vanilla's own `while (num2 > 0 && num4 > 0)` only decrements `num4` (`retries`) on a real
    // flood *attempt* that then fails acceptance — a candidate that never reaches the flood check
    // at all (not active, wrong material, no open air above) costs nothing and just rerolls. That
    // is fine under vanilla's own cave density; it is not safe in general, since a pathological
    // world with very few qualifying surface tiles could reroll indefinitely without ever touching
    // `retries`. A hard ceiling here is new, defensive, and does not change behaviour on any real
    // generated world (which reaches its `budget` long before this).
    let mut hard_ceiling = 5_000_000u32;

    while budget > 0 && retries > 0 && hard_ceiling > 0 {
        hard_ceiling -= 1;
        let x = rand.next_range(2, layout.width - 2);
        let y = rand.next_range(layout.surface, layout.height - 190);
        let tile = world.tile(x, y);
        if !tile.is_active() {
            continue;
        }
        let above = world.tile(x, y - 1);
        let is_jungle = tile.block == tiles::JUNGLE_GRASS;
        let wall = if is_jungle {
            walls::JUNGLE_UNSAFE[rand.next_max(4) as usize]
        } else if tile.block == tiles::STONE && above.wall == 0 {
            if layout.remix {
                // `WorldGen.cs:16833`, the remix half of the same ternary: the mapping inverts, so
                // dirt walls line the *deep* caves and rock walls the shallow ones. The lava branch
                // is reachable only below the lava line, and `||` short-circuits before the coin
                // flip above it, so a shallow cave costs one draw fewer than the ordinary path -
                // kept, because every later draw in the world comes from the same stream.
                if y > layout.rock {
                    walls::DIRT_UNSAFE[rand.next_max(4) as usize]
                } else if y <= layout.underworld || rand.next_max(2) != 0 {
                    walls::ROCKS_UNSAFE[rand.next_max(4) as usize]
                } else {
                    walls::LAVA_UNSAFE[rand.next_max(4) as usize]
                }
            } else if y < layout.rock {
                walls::DIRT_UNSAFE[rand.next_max(4) as usize]
            } else if y >= layout.underworld {
                walls::LAVA_UNSAFE[rand.next_max(4) as usize]
            } else {
                walls::ROCKS_UNSAFE[rand.next_max(4) as usize]
            }
        } else {
            0
        };
        if wall == 0 || above.is_active() {
            continue;
        }

        // Vanilla also requires the flood to *complete* inside its own 1000-tile budget
        // (`flag2`/`ShapeFloodFill.Perform`'s return value) before accepting a site — meaningful in
        // vanilla's own topology, where hitting that cap means "this pocket sprawls into something
        // implausibly large for an ordinary cave," a real rejection signal. `structures::caves()`
        // instead produces one large interconnected network (documented repeatedly across this
        // session — `gem_caves.rs`/`spider_caves.rs` hit the identical problem), so an ordinary,
        // perfectly normal-looking candidate here saturates the 1000-tile cap essentially every
        // time; requiring `completed` measured near-zero acceptances and made this pass take
        // several minutes to run on a real world, burning through all 100,000 retries almost every
        // call. Dropped for the same reason `gem_caves.rs` dropped its own `found.tiles >= 300`
        // rejection: the cap still bounds the *paint* footprint (`flood_open`'s own `max_tiles`),
        // it just no longer gates *acceptance* too.
        let (shape, _completed, touched_invalid) = flood_open(world, x, y - 1, 1000);
        if shape.count() > 50 && !touched_invalid {
            paint_outer_outline(world, &shape, wall);
            painted += 1;
            budget -= 1;
        } else {
            retries -= 1;
        }
    }
    painted
}

/// `Spread.Wall`: floods outward through open tiles, walling each one; a solid, active, wall-free
/// boundary tile gets the wall too but does not extend the flood past it. No cap of its own in
/// vanilla — bounded here at `max_tiles`, matching every other flood this session capped for the
/// same reason (see the module doc).
fn spread_wall(world: &mut World, x: i32, y: i32, wall: u16, max_tiles: usize) {
    let mut seen: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from([(x, y)]);
    while let Some((cx, cy)) = queue.pop_front() {
        if !seen.insert((cx, cy)) || seen.len() > max_tiles {
            continue;
        }
        if !world.in_bounds(cx, cy) {
            continue;
        }
        let mut t = world.tile(cx, cy);
        let solid = tile_solid::solid(t.block) && t.is_active();
        if solid || t.wall != 0 {
            if t.is_active() && t.wall == 0 {
                t.wall = wall;
                world.set_tile(cx, cy, t);
            }
            continue;
        }
        t.wall = wall;
        world.set_tile(cx, cy, t);
        queue.push_back((cx - 1, cy));
        queue.push_back((cx + 1, cy));
        queue.push_back((cx, cy - 1));
        queue.push_back((cx, cy + 1));
    }
}

/// `Spread.Wall2`, narrowed to wall type 15 (Mud) — see the module doc's note on why the general
/// `stopsAtAir` branch is dead code for this pass's one call site.
const CANNOT_BE_REPLACED: [u16; 7] = [4, 40, 3, 83, 87, 244, 34];
const MAX_WALL_OUT: usize = 5000;

fn spread_wall_mud(world: &mut World, x: i32, y: i32) {
    const MUD: u16 = walls::MUD;
    let mut seen: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from([(x, y)]);
    let mut painted = 0usize;
    while let Some((cx, cy)) = queue.pop_front() {
        if !seen.insert((cx, cy)) {
            continue;
        }
        if !world.in_bounds(cx, cy) {
            continue;
        }
        let mut t = world.tile(cx, cy);
        if t.wall == MUD || CANNOT_BE_REPLACED.contains(&t.wall) {
            continue;
        }
        let solid = tile_solid::solid(t.block) && t.is_active();
        if !solid {
            if painted >= MAX_WALL_OUT {
                continue;
            }
            painted += 1;
            t.wall = MUD;
            world.set_tile(cx, cy, t);
            queue.push_back((cx - 1, cy));
            queue.push_back((cx + 1, cy));
            queue.push_back((cx, cy - 1));
            queue.push_back((cx, cy + 1));
        } else if t.is_active() {
            t.wall = MUD;
            world.set_tile(cx, cy, t);
        }
    }
}

/// The wall choice `CaveWallsInEnclosedSpaces`' first loop makes from a pocket's own
/// [`cave_flood::CaveCount`] breakdown.
fn wall_for_pocket(found: &cave_flood::CaveCount, rand: &mut UnifiedRandom) -> u16 {
    if (found.shroom as f64) > (found.rock as f64) * 0.75 {
        return walls::MUSHROOM_UNSAFE;
    }
    if found.ice > 0 {
        return if rand.next_max(2) == 0 {
            walls::SNOW
        } else {
            walls::ICE
        };
    }
    if found.lava > 0 {
        return walls::OBSIDIAN_BACK;
    }
    match rand.next_max(4) {
        0 => walls::CAVE6,
        1 => walls::CAVE7,
        2 => walls::CAVE_WALL,
        _ => walls::CAVE_WALL2,
    }
}

/// The `CaveWallsInEnclosedSpaces` pass. Returns how many pockets were painted (both loops
/// combined).
pub fn enclosed_spaces(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) -> usize {
    if layout.width < 400 || layout.height < 450 {
        // Same guard shape as `variety` above — this pass's own site search needs
        // `(worldSurface+rock)/2 .. height-220` to be non-empty.
        return 0;
    }
    let mut painted = 0usize;

    // Both loops below drop vanilla's own `found.tiles >= 1500` upper-bound rejection, for the
    // same reason `variety` above does and `gem_caves.rs`/`spider_caves.rs` already did: this
    // generator's connected cave network saturates that cap on essentially every real candidate, so
    // treating it as a rejection signal (rather than just the paint-footprint cap it still is, via
    // `spread_wall`/`spread_wall_mud`'s own `max_tiles`) measured near-zero acceptances.

    // First loop: an ordinary mid-sized pocket, walled by what it contains.
    let attempts = (f64::from(layout.width) * 0.04) as i32;
    for _ in 0..attempts {
        let mut tries = 0;
        let mut x = rand.next_range(200, layout.width - 200);
        // `WorldGen.cs:12637`: Remix sites the glowing-mushroom patches above the rock line.
        let (patch_top, patch_bottom) = if layout.remix {
            layout.deep_band()
        } else {
            ((layout.surface + layout.rock) / 2, layout.height - 220)
        };
        let mut y = rand.next_range(patch_top, patch_bottom);
        let mut found = cave_flood::count(world, x, y, 1500, true, false);
        while found.tiles < 10 && tries < 500 {
            tries += 1;
            x = rand.next_range(200, layout.width - 200);
            y = rand.next_range(patch_top, patch_bottom);
            found = cave_flood::count(world, x, y, 1500, true, false);
        }
        if tries < 500 {
            let wall = wall_for_pocket(&found, rand);
            spread_wall(world, x, y, wall, 1500);
            painted += 1;
        }
    }

    // Second loop: a jungle-walled pocket specifically, flooded with Mud wall instead.
    let jungle_attempts = (f64::from(layout.width) * 0.02) as i32;
    for _ in 0..jungle_attempts {
        let mut tries = 0;
        let mut x = rand.next_range(200, layout.width - 200);
        let mut y = rand.next_range(layout.surface, layout.underworld);
        let mut found = if world.tile(x, y).wall == walls::JUNGLE {
            cave_flood::count(world, x, y, 1500, false, true)
        } else {
            cave_flood::CaveCount::default()
        };
        while found.tiles < 10 && tries < 1000 {
            tries += 1;
            x = rand.next_range(200, layout.width - 200);
            y = rand.next_range(layout.surface, layout.underworld);
            let wall_here = world.tile(x, y).wall;
            found = if wall_here == walls::JUNGLE {
                cave_flood::count(world, x, y, 1500, false, true)
            } else {
                cave_flood::CaveCount::default()
            };
        }
        if tries < 1000 {
            spread_wall_mud(world, x, y);
            painted += 1;
        }
    }

    painted
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrustia_proto::Tile;

    fn stone_world(width: i32, height: i32, seed: i32) -> (World, Layout) {
        let mut world = World::empty(width, height, "wall-variety");
        for x in 0..width {
            for y in 0..height {
                world.set_tile(x, y, Tile::block(tiles::STONE));
            }
        }
        let mut rand = UnifiedRandom::new(seed);
        let layout = Layout::plan(width, height, &mut rand);
        (world, layout)
    }

    #[test]
    fn a_large_open_pocket_below_a_stone_surface_tile_gets_painted() {
        let (mut world, layout) = stone_world(300, 400, 5);
        // A checkerboard cavern across the whole underground search range, rather than one small
        // hand-placed pocket: every even row is a thin stone "floor" with open air both directly
        // above (the odd row before it) and below (the giant connected open space every odd row
        // and the rest of the cavern forms). This maximises how often a random `(x, y)` pick lands
        // on a genuine candidate — real generated worlds have caves throughout their underground,
        // this is the same density in miniature so the random search converges fast in a test
        // rather than needing millions of picks against one sparse pocket.
        for x in 2..298 {
            for y in layout.surface..layout.height - 190 {
                let tile = if y % 2 == 0 {
                    Tile::block(tiles::STONE)
                } else {
                    Tile::AIR
                };
                world.set_tile(x, y, tile);
            }
        }

        let mut rand = UnifiedRandom::new(5);
        let painted = variety(&mut world, &layout, &mut rand);
        assert!(painted > 0, "expected at least one painted pocket");

        let has_variety_wall = (0..world.width())
            .flat_map(|x| (0..world.height()).map(move |y| (x, y)))
            .any(|(x, y)| {
                let w = world.tile(x, y).wall;
                walls::LAVA_UNSAFE.contains(&w)
                    || walls::ROCKS_UNSAFE.contains(&w)
                    || walls::DIRT_UNSAFE.contains(&w)
                    || walls::JUNGLE_UNSAFE.contains(&w)
            });
        assert!(
            has_variety_wall,
            "expected some wall-variety texture placed"
        );
    }

    #[test]
    fn a_mid_sized_enclosed_pocket_gets_filled_with_wall() {
        let (mut world, layout) = stone_world(1200, 900, 9);
        let (cx, cy) = (600, (layout.surface + layout.rock) / 2 + 20);
        for x in cx - 6..cx + 6 {
            for y in cy - 6..cy + 6 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        let mut rand = UnifiedRandom::new(9);
        let painted = enclosed_spaces(&mut world, &layout, &mut rand);
        assert!(painted > 0, "expected at least one enclosed pocket filled");
        let walled = (cx - 6..cx + 6)
            .flat_map(|x| (cy - 6..cy + 6).map(move |y| (x, y)))
            .filter(|&(x, y)| world.tile(x, y).wall != 0)
            .count();
        assert!(walled > 0, "the pocket should carry a real wall afterward");
    }

    #[test]
    fn a_small_world_does_not_panic() {
        let (mut world, layout) = stone_world(400, 300, 1);
        let mut rand = UnifiedRandom::new(1);
        assert_eq!(variety(&mut world, &layout, &mut rand), 0);
        assert_eq!(enclosed_spaces(&mut world, &layout, &mut rand), 0);
    }
}
