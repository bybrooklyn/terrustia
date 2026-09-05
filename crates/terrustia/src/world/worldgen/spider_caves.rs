//! Spider caves: a cobweb-lined pocket deep underground, with a scatter of pots, small piles and
//! ceiling stalactites inside it.
//!
//! Transcribed from the `SpiderCaves` generation pass (`WorldGen.cs:17470-17542`) and
//! `Spread.Spider` (`WorldGen.cs:3622-3729`), the wave flood-fill that decorates the pocket once a
//! candidate site passes [`super::cave_flood::count`]'s size check — the same siting mechanism
//! `gem_caves.rs` uses, with `lava_ok: true` here (vanilla's own `lavaOk: true` argument) since a
//! spider cave is allowed to run into lava rather than being blocked by it outright.
//!
//! Uses `SmallRng` throughout rather than the shared `UnifiedRandom` other structural passes in
//! this module take, because it calls straight into `pots::place_pot`/`piles::place_small_pile`,
//! which already commit to `SmallRng` — this project's own convention already splits generation
//! random state across the two (see `worldgen/mod.rs::build`'s own comment on `forest_rng`), since
//! parity with vanilla's single shared `genRand` was never the goal here.
//!
//! **One real gap, disclosed rather than silently dropped**: vanilla's `Spread.Spider` has a
//! 1-in-15 chance, when the floor-solid roll lands, of placing a buried chest (item 939, style 15)
//! instead of the ordinary pot (`WorldGen.cs:3677`, `AddBuriedChest`). This project's chest
//! placement (`structures::chests`) is a whole-world scatter pass, not a single-site placer, and
//! building one just for this rare sub-case is out of scope for this pass — noted here for
//! whoever next touches chest placement, not silently omitted. Everything else — the cobweb wall,
//! the pot, the large and small piles, the stalactites — is transcribed.
//!
//! **Site-acceptance is vanilla's whole rule again, and `spread_spider` needs no cap of its own**,
//! for the reason `gem_caves.rs` documents in full: the saturation that would have made vanilla's
//! 3500-tile upper bound reject everywhere was `terrain::fill`'s wall on the cavern layer's solid
//! rock, not `structures::caves()`' topology, and both have been fixed. So the upper bound is back
//! alongside the 500-tile lower bound and the mushroom-contamination check, and the wave runs to
//! the pocket's own edge the way vanilla's does.

use std::collections::HashSet;

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::{Tile, tile_solid};

use super::cave_flood;
use super::layout::Layout;
use super::piles::place_small_pile;
use super::pots::place_pot;
use crate::world::World;

/// `WallID.CaveUnsafe` — the cobweb-adjacent cave wall `Spread.Spider` paints (`byte wall = 62;`,
/// `WorldGen.cs:3628`). Not in `tiles::walls` yet; only this pass uses it so far.
const SPIDER_WALL: u16 = 62;

/// Tile 165, the stalactite `PlaceUncheckedStalactite` places — hangs from a solid ceiling, one
/// tile wide, two tall.
const STALACTITE: u16 = 165;

/// `TileID.LargePiles2` — a *tile* id, not to be confused with the wall id `WallID.Sandstone`,
/// which happens to share the same number (187) in a completely different id space. Vanilla's own
/// `PlaceTile(item.X, item.Y, 187, ...)` call is placing this tile. `piles.rs` has its own private
/// `LARGE_PILE_B` for the same id; not exported, so this is its own copy rather than a dependency
/// on piles.rs's internals.
const LARGE_PILE_TILE: u16 = 187;

/// The `SpiderCaves` pass: scatter cobweb-lined pockets through the rock and cavern layers.
///
/// Returns how many were placed.
pub fn scatter(world: &mut World, layout: &Layout, rng: &mut SmallRng) -> usize {
    // The search bands below are `200..width-200` and `layout.rock+30..height-230`. Real,
    // full-size worlds always clear both by a wide margin, but the small synthetic worlds several
    // unrelated tests build (to keep persistence/gameplay tests fast) do not — the same shape of
    // guard `oasis.rs::scatter` and `gem_caves.rs::scatter` need for their own search bands. Skip
    // rather than let `random_range` panic on an inverted or empty range.
    if layout.width <= 400 || world.height() <= layout.rock + 260 {
        return 0;
    }

    let attempts = ((layout.width as f64) * 0.005) as i32;
    let mut placed = 0usize;

    for _ in 0..attempts {
        let mut tries = 0;
        let max_tries = layout.width / 2;
        let mut x = rng.random_range(200..layout.width - 200);
        let mut y = rng.random_range(layout.rock + 30..world.height() - 230);
        let mut found = cave_flood::count(world, x, y, 3500, true, false);
        while (found.tiles >= 3500 || found.tiles < 500 || found.shroom > 1) && tries < max_tries {
            tries += 1;
            x = rng.random_range(200..layout.width - 200);
            y = rng.random_range(layout.rock + 30..world.height() - 230);
            found = cave_flood::count(world, x, y, 3500, true, false);
        }
        if tries < max_tries {
            spread_spider(world, x, y, rng);
            placed += 1;
        }
    }
    placed
}

/// `Spread.Spider`, transcribed. Nothing bounds it but the pocket, as in vanilla: the wave stops
/// at solid or walled rock and walls what it crosses, and the site check above already rejected any
/// pocket of 3500 tiles or more.
fn spread_spider(world: &mut World, x: i32, y: i32, rng: &mut SmallRng) {
    let mut seen: HashSet<(i32, i32)> = HashSet::new();
    let mut wave = vec![(x, y)];

    while !wave.is_empty() {
        let this_wave = std::mem::take(&mut wave);
        for (cx, cy) in this_wave {
            if cx < 1 || cx >= world.width() - 1 || cy < 1 || cy >= world.height() - 1 {
                continue;
            }
            if !seen.insert((cx, cy)) {
                continue;
            }
            let tile = world.tile(cx, cy);
            let solid_or_walled =
                (tile.is_active() && tile_solid::solid(tile.block)) || tile.wall != 0;
            if solid_or_walled {
                if tile.is_active() && tile.wall == 0 {
                    let mut t = tile;
                    t.wall = SPIDER_WALL;
                    world.set_tile(cx, cy, t);
                }
                continue;
            }

            let mut t = tile;
            t.wall = SPIDER_WALL;
            if !t.is_active() {
                // `liquid_kind` is only meaningful once `liquid` is non-zero (see its own doc
                // comment), so clearing `liquid` alone is enough to drain whatever was here.
                t.liquid = 0;
            }
            world.set_tile(cx, cy, t);

            if !world.tile(cx, cy).is_active() {
                let floor = world.tile(cx, cy + 1);
                let floor_solid = floor.is_active() && tile_solid::solid(floor.block);
                if floor_solid && rng.random_range(0..3) == 0 {
                    // Vanilla's 1-in-15 roll here is a buried chest instead of a pot
                    // (`AddBuriedChest`, item 939, style 15) — not built here, see the module doc.
                    if rng.random_range(0..15) != 0 {
                        place_pot(world, cx, cy, 19 + rng.random_range(0..2), rng);
                    }
                }
                if !world.tile(cx, cy).is_active() {
                    let ceiling = world.tile(cx, cy - 1);
                    let ceiling_solid = ceiling.is_active() && tile_solid::solid(ceiling.block);
                    if ceiling_solid && rng.random_range(0..3) == 0 {
                        place_stalactite(world, cx, cy, rng);
                    } else if floor_solid {
                        let style = 9 + rng.random_range(0..5);
                        // `place_large_pile` refuses (writes nothing) unless the whole 3x2
                        // footprint is clear and floored — matching vanilla's own `Place3x2`,
                        // which is genuinely all-or-nothing. When it refuses, `(cx, cy)` is still
                        // inactive afterward, which is exactly what un-deads the small-pile
                        // fallback below (`WorldGen.cs:3691-3699`'s own `if (!tile.active())`
                        // guards only ever pass when `Place3x2` itself didn't place anything).
                        place_large_pile(world, cx, cy, style);
                        if rng.random_range(0..3) == 0 {
                            if !world.tile(cx, cy).is_active() {
                                place_small_pile(world, cx, cy, 34 + rng.random_range(0..4), 1);
                            }
                            if !world.tile(cx, cy).is_active() {
                                place_small_pile(world, cx, cy, 48 + rng.random_range(0..6), 0);
                            }
                        }
                    }
                }
            }
            for n in [(cx - 1, cy), (cx + 1, cy), (cx, cy - 1), (cx, cy + 1)] {
                if !seen.contains(&n) {
                    wave.push(n);
                }
            }
        }
    }
}

/// `Place3x2` for a `TileID.LargePiles2` object (type 187): a 3-wide, 2-tall footprint anchored
/// with `(cx, cy)` as the bottom-middle cell — matching vanilla's own call
/// `PlaceTile(item.X, item.Y, 187, ..., 9 + genRand.Next(5))` (`WorldGen.cs:3691`), which
/// dispatches to `Place3x2` (`:52533`). Vanilla's own placer is genuinely all-or-nothing: all six
/// cells must already be inactive, and the row directly beneath each of the three columns must be
/// solid, or nothing is written at all — a partial pile is not a real pile. The old code wrote
/// only the single `(cx, cy)` cell with `frameX = style * 18`, which is neither the real 3x2
/// footprint nor `Place3x2`'s own `frameX = 54*style + 18*column` stride, and — because it always
/// unconditionally activated `(cx, cy)` — made the small-pile fallback below permanently dead code
/// (its own `!tile.active()` guards could never see anything but an already-active tile).
fn place_large_pile(world: &mut World, cx: i32, cy: i32, style: i32) -> bool {
    for dx in -1..=1 {
        for dy in -1..=0 {
            if world.tile(cx + dx, cy + dy).is_active() {
                return false;
            }
        }
        let floor = world.tile(cx + dx, cy + 1);
        if !(floor.is_active() && tile_solid::solid(floor.block)) {
            return false;
        }
    }
    let base_x = (54 * style) as i16;
    for (dx, column) in [(-1i32, 0i16), (0, 1), (1, 2)] {
        let frame_x = base_x + column * 18;
        world.set_tile(cx + dx, cy - 1, Tile::framed(LARGE_PILE_TILE, frame_x, 0));
        world.set_tile(cx + dx, cy, Tile::framed(LARGE_PILE_TILE, frame_x, 18));
    }
    true
}

/// `PlaceUncheckedStalactite`, the `spiders: true` branch only (`WorldGen.cs:38735-38748`) —
/// nothing in `Spread.Spider` calls the non-spider branch, so it is not transcribed here.
fn place_stalactite(world: &mut World, x: i32, y: i32, rng: &mut SmallRng) {
    if world.tile(x, y).is_active() || world.tile(x, y + 1).is_active() {
        return;
    }
    let variation = rng.random_range(0..3);
    let frame_x = (108 + variation * 18) as i16;
    let top = Tile::framed(STALACTITE, frame_x, 0);
    let bottom = Tile::framed(STALACTITE, frame_x, 18);
    world.set_tile(x, y, top);
    world.set_tile(x, y + 1, bottom);
}

#[cfg(test)]
mod tests {
    use super::super::rand::UnifiedRandom;
    use super::super::tiles;
    use super::*;
    use rand::SeedableRng;

    fn rock_block(width: i32, height: i32, rock: i32) -> (World, Layout) {
        let mut world = World::empty(width, height, "spider-caves");
        for x in 0..width {
            for y in 0..height {
                world.set_tile(x, y, Tile::block(tiles::STONE));
            }
        }
        let mut layout_rand = UnifiedRandom::new(1);
        let mut layout = Layout::plan(width, height, &mut layout_rand);
        layout.rock = rock;
        (world, layout)
    }

    #[test]
    fn a_large_hollow_pocket_in_rock_gets_cobweb_walls() {
        let (mut world, layout) = rock_block(1400, 1000, 300);
        for x in 690..730 {
            for y in 690..720 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        let mut rng = SmallRng::seed_from_u64(3);
        let placed = scatter(&mut world, &layout, &mut rng);
        assert!(
            placed > 0,
            "a well-formed 1200-tile pocket should take a spider cave"
        );

        let walled = (0..world.width())
            .flat_map(|x| (0..world.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| world.tile(x, y).wall == SPIDER_WALL)
            .count();
        assert!(
            walled > 0,
            "a placed spider cave should leave cobweb walls behind"
        );
    }

    #[test]
    fn no_spider_cave_forms_where_every_pocket_is_too_small() {
        let (mut world, layout) = rock_block(600, 500, 200);
        let mut rng = SmallRng::seed_from_u64(9);
        assert_eq!(scatter(&mut world, &layout, &mut rng), 0);
    }

    /// A real regression: the small synthetic worlds several unrelated tests build via the full
    /// `build()` pipeline are smaller than this pass's own search bands assume, and
    /// `random_range` panics on an inverted or empty range rather than returning something —
    /// `world.height() - 230` going below `layout.rock + 30` (or `layout.width - 200` below
    /// `200`) took down `world::wld_save`/`world::world::flag_tests` tests that never touch
    /// spider caves directly, just by calling `build()` on a small world. Fails on the pre-fix
    /// code (panics rather than returning `0`).
    #[test]
    fn a_world_too_small_for_the_search_bands_does_not_panic() {
        // Width 1000 keeps `attempts` (`width * 0.005`) at 5, not 0 — a width small enough to
        // zero out `attempts` would skip the loop entirely and never reach the panicking call,
        // proving nothing. `height=200, rock=50` makes `layout.rock + 30` (80) exceed
        // `world.height() - 230` (-30), the actual inverted-range shape that panicked.
        let (mut world, layout) = rock_block(1000, 200, 50);
        let mut rng = SmallRng::seed_from_u64(1);
        assert_eq!(scatter(&mut world, &layout, &mut rng), 0);
    }

    /// Vanilla's upper bound, restored: a 60,000-tile corridor is a vast open space, not a spider
    /// cave pocket. This is the same fixture that used to assert the opposite while the bound was
    /// switched off, and its doc comment then recorded that shape as what a real generated
    /// candidate looks like. The measured topology says otherwise (see
    /// `structures::cave_topology_measurement`), so the fixture now asserts what vanilla does.
    #[test]
    fn a_pocket_bigger_than_vanillas_own_window_is_not_a_site() {
        let (mut world, layout) = rock_block(4200, 1000, 300);
        for x in 100..4100 {
            for y in 690..705 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        let mut rng = SmallRng::seed_from_u64(3);
        assert_eq!(
            scatter(&mut world, &layout, &mut rng),
            0,
            "a 4000-tile-long open corridor is not a spider cave pocket"
        );

        let walled = (0..world.width())
            .flat_map(|x| (0..world.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| world.tile(x, y).wall == SPIDER_WALL)
            .count();
        assert_eq!(walled, 0, "nothing sited means nothing painted");
    }

    #[test]
    fn a_stalactite_needs_solid_rock_above_and_open_space_below() {
        let mut world = World::empty(50, 50, "stalactite");
        world.set_tile(10, 9, Tile::block(tiles::STONE));
        let mut rng = SmallRng::seed_from_u64(1);
        place_stalactite(&mut world, 10, 10, &mut rng);
        assert!(
            world.tile(10, 10).is_active(),
            "should place with solid rock above"
        );
        assert!(
            world.tile(10, 11).is_active(),
            "the stalactite is two tiles tall"
        );
        assert_eq!(world.tile(10, 10).block, STALACTITE);
    }

    /// `Place3x2`'s real footprint (`WorldGen.cs:52533-52660`) is 3 wide, 2 tall, anchored at
    /// `(cx, cy)` as the bottom-middle cell, with `frameX = 54*style + 18*column` — not the single
    /// tile the old code wrote. Fails on the pre-fix code (which only ever activated `(cx, cy)`
    /// itself).
    #[test]
    fn a_large_pile_places_the_full_3x2_footprint_with_the_right_stride() {
        let mut world = World::empty(30, 30, "large-pile-3x2");
        for dx in -1..=1 {
            world.set_tile(15 + dx, 21, Tile::block(tiles::STONE));
        }
        let placed = place_large_pile(&mut world, 15, 20, 9);
        assert!(placed, "a clear, floored 3x2 footprint should place");
        let base_x = 54 * 9;
        for (dx, column) in [(-1i32, 0i16), (0, 1), (1, 2)] {
            let top = world.tile(15 + dx, 19);
            let bottom = world.tile(15 + dx, 20);
            assert_eq!(top.block, LARGE_PILE_TILE, "top row, column {dx}");
            assert_eq!(bottom.block, LARGE_PILE_TILE, "bottom row, column {dx}");
            assert_eq!(
                top.frame_x,
                base_x + column * 18,
                "top row frame_x, column {dx}"
            );
            assert_eq!(top.frame_y, 0, "top row frame_y, column {dx}");
            assert_eq!(
                bottom.frame_x,
                base_x + column * 18,
                "bottom row frame_x, column {dx}"
            );
            assert_eq!(bottom.frame_y, 18, "bottom row frame_y, column {dx}");
        }
    }

    /// When any of the six footprint cells is already active, `Place3x2` refuses the whole
    /// object rather than filling in whichever cells happen to be free.
    #[test]
    fn a_large_pile_refuses_the_whole_object_if_any_cell_is_occupied() {
        let mut world = World::empty(30, 30, "large-pile-refuse");
        for dx in -1..=1 {
            world.set_tile(15 + dx, 21, Tile::block(tiles::STONE));
        }
        // One cell of the footprint (top-right) is already occupied.
        world.set_tile(16, 19, Tile::block(tiles::STONE));
        let placed = place_large_pile(&mut world, 15, 20, 9);
        assert!(
            !placed,
            "an occupied footprint cell must refuse the whole object"
        );
        assert!(
            !world.tile(15, 20).is_active(),
            "no cell should have been written on refusal"
        );
    }

    /// Vanilla's own small-pile fallback (`WorldGen.cs:3693-3699`) only ever fires when
    /// `Place3x2` itself refused to place — its `!tile.active()` guards can only pass then. The
    /// old code always unconditionally activated `(cx, cy)` before this check ran, so the
    /// fallback was permanently unreachable. Fails on the pre-fix code (in which `place_large_pile`
    /// activating `(cx, cy)` unconditionally would make this small pile branch dead).
    #[test]
    fn the_small_pile_fallback_can_fire_when_the_large_pile_is_refused() {
        let mut world = World::empty(30, 30, "small-pile-fallback");
        // A floor under only the small pile's own 2-wide footprint (columns 15-16 at row 21), not
        // under the large pile's full 3-wide one (columns 14-16) — enough for `place_small_pile`
        // to succeed on its own, while `place_large_pile` still refuses for lacking column 14's
        // floor.
        world.set_tile(15, 21, Tile::block(tiles::STONE));
        world.set_tile(16, 21, Tile::block(tiles::STONE));

        assert!(!place_large_pile(&mut world, 15, 20, 9));
        assert!(!world.tile(15, 20).is_active());
        // With the center cell still inactive, the small-pile call this session already trusts
        // elsewhere (`piles.rs`) can go ahead and actually write something there.
        assert!(
            place_small_pile(&mut world, 15, 20, 34, 1),
            "the small pile should have been free to place once the large pile refused"
        );
        assert!(world.tile(15, 20).is_active());
    }
}
