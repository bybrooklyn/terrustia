//! Gem caves: pockets of stone recolored into gem ore and lined with a matching gem wall.
//!
//! Transcribed from the `GemCaves` generation pass (`WorldGen.cs:17543-17587`) and the two
//! functions it drives: `gemCave` (`WorldGen.cs:9673`, which rolls which 1-6 gem types this
//! particular pocket may contain) and `Spread.Gem` (`WorldGen.cs:3534`, the wave flood-fill that
//! actually paints the pocket). Site-searching reuses [`super::cave_flood::count`], the same
//! `countTiles`/`nextCount` mechanism `SpiderCaves` also drives off of.
//!
//! `Spread.Gem` is a *second*, different flood fill from `cave_flood`'s — it walks outward in
//! waves (a queue of "this wave's tiles", refilled from what the wave touched) rather than a
//! depth-first stack, because unlike `count` it does real per-tile work (a wall write, a possible
//! tile recolor) and a stack-based order would still visit every reachable tile, just in a
//! different sequence — vanilla's own wave order has no gameplay consequence here, but the
//! wave-queue shape is transcribed anyway rather than swapped for the stack `cave_flood` uses, to
//! keep this a faithful port rather than a reinterpretation of a pass with real random-number
//! consumption per tile.
//!
//! **Site-acceptance is vanilla's whole rule again**, after a spell running on only half of it.
//! Vanilla rejects a candidate whose pocket overshoots 300 tiles ([`super::cave_flood::count`]'s
//! own search cap), undershoots 50, holds any lava or ice, or never touches a stone tile at all
//! (`rockCount == 0`). The upper bound is what keeps a gem cave out of a vast open space, and
//! `rockCount` is what confirms the pocket really borders rock rather than floating inside some
//! other biome's material.
//!
//! Both were switched off here for a while, because every candidate in a real generated world came
//! back saturated with `rock == 0`, and keeping them would have rejected everywhere. That was true;
//! the reason recorded for it was not. It blamed `structures::caves()` for producing one large
//! interconnected network, and `structures::cave_topology_measurement` says the fills were not
//! saturating on size at all: 400 of 400 sampled fills stopped on the first *walled* tile they
//! touched, because `terrain::fill` painted a wall behind the cavern layer's solid rock and
//! `nextCount` reads a tile's wall before it asks whether the tile is solid (`WorldGen.cs:9539`).
//! `rock == 0` followed from the same thing: the fill broke out before it could count the stone it
//! had just touched. With that fixed and `caves()` on vanilla's own runners, both checks measure
//! what they are for, so both are back and [`spread_gem`] no longer needs a cap of its own.

use std::collections::HashSet;

use terrustia_proto::{TileFlags, tile_solid};

use super::cave_flood;
use super::layout::Layout;
use super::rand::UnifiedRandom;
use super::tiles::{self, walls};

/// `TileID.ExposedGems` — the same tile `speleothems.rs`'s own exposed-gem passes place.
const GEM_TILE: u16 = 178;

/// `Gemmable` (`WorldGen.cs:3731`): which active tile types `Spread.Gem` will recolor.
fn gemmable(block: u16) -> bool {
    matches!(
        block,
        0 | tiles::STONE | 40 | tiles::MUD | tiles::JUNGLE_GRASS | tiles::MUSHROOM_GRASS
    ) || matches!(block, tiles::SNOW | tiles::ICE)
}

/// `randGemTile`: 19 times out of 20, plain stone; the 1/20 goes to whichever gem this pocket
/// rolled. `gems` indexes [`tiles::GEM_WALLS`][gw]'s six slots by the same 0-5 order.
///
/// [gw]: super::tiles::walls::GEM_WALLS
fn rand_gem_tile(gems: [bool; 6], rand: &mut UnifiedRandom) -> u16 {
    const GEM_TILES: [u16; 6] = [
        tiles::AMETHYST,
        tiles::TOPAZ,
        tiles::SAPPHIRE,
        tiles::EMERALD,
        tiles::RUBY,
        tiles::DIAMOND,
    ];
    if rand.next_max(20) != 0 {
        return tiles::STONE;
    }
    GEM_TILES[rand_gem(gems, rand)]
}

/// `randGem`: rolls until it lands on one of the gems this pocket actually contains.
fn rand_gem(gems: [bool; 6], rand: &mut UnifiedRandom) -> usize {
    loop {
        let i = rand.next_max(6) as usize;
        if gems[i] {
            return i;
        }
    }
}

/// `Spread.Gem`, transcribed: wave flood-fill from `(x, y)`. A solid or walled tile gets its own
/// (and its four neighbours') gemmable type recolored; an open tile gets a gem wall instead and
/// joins the next wave.
///
/// Nothing bounds this but the pocket, exactly as in vanilla: the wave stops at solid or walled
/// rock, and it writes a wall on every open tile it crosses, so it cannot re-enter what it has
/// already painted. That is safe only because the site check above rejects any pocket of 300 tiles
/// or more, which is the guarantee this wave leans on. It carried a 300-tile cap of its own for a
/// while, when that site check was switched off; see the module doc for why it was, and why the
/// cap could go with it.
fn spread_gem(
    world: &mut super::super::World,
    x: i32,
    y: i32,
    gems: [bool; 6],
    rand: &mut UnifiedRandom,
) {
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
            // `is_active()` first: `tile_solid::solid` is a pure lookup by tile *type*, and an
            // inactive tile's leftover `block` id (0, dirt's own id) reads as solid if that check
            // runs alone — the same ordering bug `place_object.rs` was fixed for earlier.
            let solid_or_walled =
                (tile.is_active() && tile_solid::solid(tile.block)) || tile.wall != 0;
            if solid_or_walled {
                if tile.is_active() {
                    for (px, py) in [
                        (cx, cy),
                        (cx - 1, cy),
                        (cx + 1, cy),
                        (cx, cy - 1),
                        (cx, cy + 1),
                    ] {
                        let mut t = world.tile(px, py);
                        if t.is_active() && gemmable(t.block) {
                            t.block = rand_gem_tile(gems, rand);
                            world.set_tile(px, py, t);
                        }
                    }
                }
                continue;
            }
            let mut t = tile;
            t.wall = walls::GEM_WALLS[rand_gem(gems, rand)];
            world.set_tile(cx, cy, t);
            // `Spread.Gem`'s own open-tile branch (`WorldGen.cs:3589-3592`): once in a while, an
            // inactive tile in the pocket's own open interior gets a genuinely exposed gem tile
            // instead of staying bare wall — the pocket's real, findable loot, not just a colored
            // backdrop. `PlaceTile`'s dedicated `num == 178` dispatch (`WorldGen.cs:60190-60200`)
            // writes `frameX = style * 18` (the species — `KillTile`'s drop table reads this back
            // as `frameX / 18`) and `frameY = genRand.Next(3) * 18` (a cosmetic variant); a second,
            // independent `randGem()` roll picks the style here, separate from the wall's own.
            if !world.tile(cx, cy).is_active() && rand.next_max(2) == 0 {
                let style = rand_gem(gems, rand);
                let mut gem = world.tile(cx, cy);
                gem.block = GEM_TILE;
                gem.frame_x = style as i16 * 18;
                gem.frame_y = rand.next_max(3) as i16 * 18;
                gem.flags.set(TileFlags::ACTIVE, true);
                world.set_tile(cx, cy, gem);
            }
            for n in [(cx - 1, cy), (cx + 1, cy), (cx, cy - 1), (cx, cy + 1)] {
                if !seen.contains(&n) {
                    wave.push(n);
                }
            }
        }
    }
}

/// The `GemCaves` pass: scatter gem-lined pockets through the rock layer.
///
/// Returns how many were placed.
pub fn scatter(
    world: &mut super::super::World,
    layout: &Layout,
    rand: &mut UnifiedRandom,
) -> usize {
    // The search bands below are `200..width-200` and `layout.rock+30..height-230`. Real,
    // full-size worlds always clear both by a wide margin, but the small synthetic worlds several
    // unrelated tests build (to keep persistence/gameplay tests fast) do not — the same shape of
    // guard `oasis.rs::scatter` needed for its own search bands. Skip rather than let
    // `next_range` panic on an inverted or empty range.
    if layout.width <= 400 || world.height() <= layout.rock + 260 {
        return 0;
    }

    let attempts = ((layout.width as f64) * 0.003) as i32;
    let mut placed = 0usize;

    for _ in 0..attempts {
        let mut tries = 0;
        let mut x = rand.next_range(200, layout.width - 200);
        // Remix moves the cavern layer above the rock line; see `Layout::deep_band`.
        let (deep_top, deep_bottom) = layout.deep_band();
        let mut y = rand.next_range(deep_top, deep_bottom);
        let mut found = cave_flood::count(world, x, y, 300, false, false);
        while (found.tiles >= 300
            || found.tiles < 50
            || found.lava > 0
            || found.ice > 0
            || found.rock == 0)
            && tries < 1000
        {
            tries += 1;
            x = rand.next_range(200, layout.width - 200);
            y = rand.next_range(deep_top, deep_bottom);
            found = cave_flood::count(world, x, y, 300, false, false);
        }
        if tries < 1000 {
            // `gemCave`: always one random gem, then each of the other five independently has a
            // 1-in-6 chance of also being included in this pocket's palette.
            let mut gems = [false; 6];
            gems[rand.next_max(6) as usize] = true;
            for g in gems.iter_mut() {
                if rand.next_max(6) == 0 {
                    *g = true;
                }
            }
            spread_gem(world, x, y, gems, rand);
            placed += 1;
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;
    use terrustia_proto::Tile;

    fn stone_block(width: i32, height: i32, rock: i32) -> (World, Layout) {
        let mut world = World::empty(width, height, "gem-caves");
        for x in 0..width {
            for y in 0..height {
                world.set_tile(x, y, Tile::block(tiles::STONE));
            }
        }
        let mut rand = UnifiedRandom::new(1);
        let mut layout = Layout::plan(width, height, &mut rand);
        layout.rock = rock;
        (world, layout)
    }

    /// `Spread.Gem`'s own open-tile branch (`WorldGen.cs:3589-3592`) was never transcribed — a
    /// gem cave's open interior only ever got wall paint, never the exposed gem tile itself that
    /// is the pocket's own real, findable loot. Fails on the pre-fix code (no tile 178 anywhere).
    #[test]
    fn spread_gem_places_real_exposed_gem_tiles_in_the_open_interior() {
        let (mut world, _layout) = stone_block(200, 200, 100);
        for x in 90..110 {
            for y in 90..110 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        let mut rand = UnifiedRandom::new(7);
        // Every gem in this pocket's palette, maximizing how much of the 400-tile room the
        // exposed-gem roll actually gets to run against.
        let gems = [true; 6];
        spread_gem(&mut world, 100, 100, gems, &mut rand);

        let gem_tiles: Vec<(i32, i32)> = (90..110)
            .flat_map(|x| (90..110).map(move |y| (x, y)))
            .filter(|&(x, y)| world.tile(x, y).is_active() && world.tile(x, y).block == GEM_TILE)
            .collect();
        assert!(
            !gem_tiles.is_empty(),
            "expected at least one real exposed gem tile in a 400-tile open pocket"
        );
        for (x, y) in gem_tiles {
            let t = world.tile(x, y);
            assert!(
                (0..6).contains(&(t.frame_x / 18)) && t.frame_x % 18 == 0,
                "gem at ({x},{y}) has an invalid species frame_x {}",
                t.frame_x
            );
        }
    }

    #[test]
    fn a_hollow_pocket_in_the_rock_layer_gets_gem_walls() {
        let (mut world, layout) = stone_block(1200, 900, 300);
        // A pocket sized inside GemCaves' own 50-299 window, well below the rock layer.
        for x in 595..610 {
            for y in 595..605 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        let mut rand = UnifiedRandom::new(42);
        let placed = scatter(&mut world, &layout, &mut rand);
        assert!(
            placed > 0,
            "a well-formed pocket in the rock layer should take a gem cave"
        );

        let walled = (0..world.width())
            .flat_map(|x| (0..world.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| tiles::walls::GEM_WALLS.contains(&world.tile(x, y).wall))
            .count();
        assert!(
            walled > 0,
            "a placed gem cave should leave real gem-walled tiles behind"
        );
    }

    #[test]
    fn no_gem_cave_forms_where_every_pocket_is_too_small() {
        // Solid rock everywhere, no hollows at all — every candidate site fails countTiles' own
        // >=50 floor, so nothing should be placed.
        let (mut world, layout) = stone_block(600, 500, 200);
        let mut rand = UnifiedRandom::new(7);
        assert_eq!(scatter(&mut world, &layout, &mut rand), 0);
    }

    /// A real regression: the small synthetic worlds several unrelated tests build via the full
    /// `build()` pipeline are smaller than this pass's own search bands assume, and
    /// `UnifiedRandom::next_range` panics on an inverted or empty range rather than returning
    /// something — `world.height() - 230` going below `layout.rock + 30` (or `layout.width - 200`
    /// below `200`) took down `world::wld_save`/`world::world::flag_tests` tests that never
    /// touch gem caves directly, just by calling `build()` on a small world. Fails on the pre-fix
    /// code (panics rather than returning `0`).
    #[test]
    fn a_world_too_small_for_the_search_bands_does_not_panic() {
        // Width 1000 keeps `attempts` (`width * 0.003`) at 3, not 0 — a width small enough to
        // zero out `attempts` would skip the loop entirely and never reach the panicking call,
        // proving nothing. `height=200, rock=50` makes `layout.rock + 30` (80) exceed
        // `world.height() - 230` (-30), the actual inverted-range shape that panicked.
        let (mut world, layout) = stone_block(1000, 200, 50);
        let mut rand = UnifiedRandom::new(1);
        assert_eq!(scatter(&mut world, &layout, &mut rand), 0);
    }

    /// Vanilla's upper bound, restored: a pocket of 300 tiles or more is not a gem cave site.
    ///
    /// The two worlds here are the two shapes that used to be *accepted* while the bound was
    /// switched off, and the doc comments then recorded both as what a real candidate looks like.
    /// They are not: a 34,000-tile corridor and a wholly open lower half are exactly the vast open
    /// spaces the bound exists to keep gem caves out of. Nothing is placed in either now.
    #[test]
    fn a_pocket_bigger_than_vanillas_own_window_is_not_a_site() {
        let (mut world, layout) = stone_block(3600, 900, 300);
        for x in 100..3500 {
            for y in 595..605 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        let mut rand = UnifiedRandom::new(11);
        assert_eq!(
            scatter(&mut world, &layout, &mut rand),
            0,
            "a 3400-tile-long open corridor is not a gem cave pocket"
        );

        let (mut world, layout) = stone_block(1200, 900, 300);
        for x in 0..1200 {
            for y in 300..900 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        let mut rand = UnifiedRandom::new(3);
        assert_eq!(
            scatter(&mut world, &layout, &mut rand),
            0,
            "a wholly open cavern layer is not a gem cave pocket either"
        );
    }

    /// Kept from when the upper bound was switched off, inverted: what used to prove "a huge
    /// pocket is still accepted" now proves the wave paints only the pocket it was given, and
    /// stops at its edge without a cap of its own. A 15-by-10 room, the same shape
    /// `a_hollow_pocket_in_the_rock_layer_gets_gem_walls` uses, with a second identical room far
    /// away that must come back untouched.
    #[test]
    fn spread_gem_stops_at_the_pocket_it_was_given() {
        let (mut world, _layout) = stone_block(1200, 900, 300);
        for x in 595..610 {
            for y in 595..605 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        for x in 200..215 {
            for y in 595..605 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        let mut rand = UnifiedRandom::new(42);
        spread_gem(&mut world, 600, 600, [true; 6], &mut rand);

        let walled = |from: i32, to: i32| {
            (from..to)
                .flat_map(|x| (590..610).map(move |y| (x, y)))
                .filter(|&(x, y)| tiles::walls::GEM_WALLS.contains(&world.tile(x, y).wall))
                .count()
        };
        assert!(walled(590, 615) > 0, "the seeded room took no gem wall");
        assert_eq!(walled(195, 220), 0, "the far room should be untouched");
    }
}
