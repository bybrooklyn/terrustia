//! Moss: recolours open cavern pockets and exposed stone with one of five moss colours, and hangs
//! a decorative overlay tile off every moss surface once placed.
//!
//! Transcribed from two real vanilla passes: `MossAndMossCaves` (`WorldGen.cs:17588-17823`, 236
//! lines) and, separately, `LongMoss` (`WorldGen.cs:20933-20980`, 48 lines) — together 284 lines,
//! matching `plan.md`'s own sizing table exactly. The two land in one module because they operate
//! on the same `Main.tileMoss` tile family, even though vanilla runs them at very different points
//! in its own pass order (`MossAndMossCaves` is generation pass 65 of 105; `LongMoss` is pass 94).
//!
//! **`LongMoss` is transcribed faithfully and completely.** For every active moss tile, each
//! inactive cardinal neighbour gets a decorative long-moss overlay (`TileID.HangingVines`-shaped
//! tile 184, one of vanilla's `PlaceTile` decorative-overlay types). No siting logic, no DSL, no
//! disclosed cut.
//!
//! **`MossAndMossCaves` lands narrower.** What's transcribed: `setMoss`'s own x-banded colour
//! selection (the world is split into left/middle/right thirds, each independently rolling one of
//! five moss colours — Green/Brown/Red/Blue/Purple — so a world can show up to three different moss
//! colours across its width, never all five); the pocket-scatter loop (find an open, plain-stone-
//! bordering pocket via [`super::cave_flood::count`], the same shared flood every Tier 2/3 pocket
//! search this session has used, then flood-paint it with [`spread_moss`], vanilla's `Spread.Moss`);
//! and both of vanilla's isolated-stone-to-moss-stone conversion loops (an unconditional one, and a
//! second that only converts a stone tile with at least one inactive neighbour — i.e. only ever
//! visibly exposed stone).
//!
//! **Disclosed and skipped**, each a genuinely separate subsystem rather than a missing detail:
//!
//! * `neonMossBiome` (`WorldGen.cs:9737-9853`, ~90 lines) — a late-game decorative variant (a
//!   shrinking, wandering-blob "neon moss" patch, gated behind a getGoodWorldGen/tenth-anniversary/
//!   remix check on an ordinary world) that recolours exposed stone via `SpreadGrass`'s general
//!   recursive grass-conversion machinery, not the plain moss-tile assignment this module uses.
//!   Porting it faithfully means porting `SpreadGrass` itself (`WorldGen.cs:75756-75868`, 113
//!   lines) — a separate general-purpose subsystem, the same class of cut this project made for
//!   `GrowLivingTree`'s root-passage system and the 15 `MicroBiome` classes' `Shapes`/`Modifiers`
//!   DSL dependency.
//! * `LavaMoss` (tile 381, a single rare decorative conversion near lava, gated by a 20-of-a-2500-
//!   tile-window liquid-lava density check) — a minor easter egg, not the moss mechanic itself.
//! * The closing `SpreadGrass`-based diffusion step (every moss tile tries to spread itself one
//!   step onto each of its 4 neighbours) — same `SpreadGrass` dependency as `neonMossBiome` above;
//!   without it, moss stays exactly where [`spread_moss`]/the two conversion loops put it rather
//!   than creeping outward afterward. The pocket flood-paint already gives every moss cave real
//!   area, so this is a secondary polish pass, not the mechanic's core visible effect.
//!
//! No `StructureMap` dependency — nothing here places a discrete, space-reserving structure.
//!
//! Both isolated-stone-conversion loops' own upper depth bound reads `layout.underworld` in place
//! of `GenVars.lavaLine`, the same stand-in `wall_variety.rs`'s own module doc explains in full.

use terrustia_proto::TileFlags;

use super::cave_flood;
use super::layout::Layout;
use super::rand::UnifiedRandom;
use super::tiles;
use crate::world::World;

/// `TileID.GreenMoss..PurpleMoss` (179-183), `setMoss`'s own `179 + mossType[band]`.
const MOSS_TILES: [u16; 5] = [179, 180, 181, 182, 183];
/// `WallID.CaveUnsafe..Cave5Unsafe` (54-58), `setMoss`'s own `54 + mossType[band]`.
const MOSS_WALLS: [u16; 5] = [54, 55, 56, 57, 58];
/// `TileID.LongMoss` — the decorative overlay `LongMoss` (the pass) hangs off every moss surface.
const LONG_MOSS: u16 = 184;

/// The three world-band moss colours `randMoss()` rolls, each distinct from the ones before it —
/// `mossType[1] != mossType[0]`, `mossType[2] != mossType[0] && != mossType[1]` in source.
fn roll_moss_types(rand: &mut UnifiedRandom) -> [usize; 3] {
    let a = rand.next_max(5) as usize;
    let mut b = rand.next_max(5) as usize;
    while b == a {
        b = rand.next_max(5) as usize;
    }
    let mut c = rand.next_max(5) as usize;
    while c == a || c == b {
        c = rand.next_max(5) as usize;
    }
    [a, b, c]
}

/// `setMoss(x, y)`: picks a band by `x`'s position across the world's width, not `y`.
fn moss_for_x(x: i32, width: i32, moss_type: [usize; 3]) -> (u16, u16) {
    let band = if (x as f64) < f64::from(width) * 0.334 {
        0
    } else if (x as f64) < f64::from(width) * 0.667 {
        1
    } else {
        2
    };
    let t = moss_type[band];
    (MOSS_TILES[t], MOSS_WALLS[t])
}

/// `Spread.Moss(x, y)`: floods outward through open (non-solid, unwalled) tiles, walling each one
/// with `moss_wall`; a solid or already-walled boundary tile gets `moss_wall` too (if it had no
/// wall) and, if it's plain Stone, is recoloured to `moss_tile` — but the flood does not continue
/// past it. No cap of its own in vanilla; capped here at `max_tiles` for the same reason every
/// other flood this session needed one (see `cave_flood.rs`'s own doc comment).
fn spread_moss(
    world: &mut World,
    x: i32,
    y: i32,
    moss_tile: u16,
    moss_wall: u16,
    max_tiles: usize,
) {
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
        let solid = terrustia_proto::tile_solid::solid(t.block) && t.is_active();
        if solid || t.wall != 0 {
            if t.is_active() {
                if t.wall == 0 {
                    t.wall = moss_wall;
                }
                if t.block == tiles::STONE {
                    t.block = moss_tile;
                }
                world.set_tile(cx, cy, t);
            }
            continue;
        }
        t.wall = moss_wall;
        world.set_tile(cx, cy, t);
        queue.push_back((cx - 1, cy));
        queue.push_back((cx + 1, cy));
        queue.push_back((cx, cy - 1));
        queue.push_back((cx, cy + 1));
    }
}

/// The `MossAndMossCaves` pass. Returns how many pockets were painted plus how many isolated stone
/// tiles were converted to moss stone, combined — a scatter/recolour pass, so a tile count is the
/// meaningful measurement.
pub fn scatter(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) -> usize {
    if layout.width < 500 || layout.height < 500 {
        // `RandomWorldPoint`-style world-derived `next_range` calls below need real headroom —
        // same guard shape as `wall_variety.rs`, not a claim any real world is this small.
        return 0;
    }
    let moss_type = roll_moss_types(rand);
    let mut changed = 0usize;

    // Pocket scatter: find an open, stone-bordering pocket and flood-paint it. Vanilla's own
    // initial pick uses a different y-range (`(worldSurface+rockLayer)/2 .. GenVars.waterLine`)
    // than its retry loop (`rockLayer+30 .. maxTilesY-230`) — a real, minor asymmetry. This project
    // has no `waterLine` equivalent to give the first range independent meaning, so both draws use
    // the retry loop's own range throughout, matching the same "collapse to one consistent bound"
    // simplification `oasis.rs`/`gem_caves.rs` already made for their own GenVars stand-ins.
    let attempts = (f64::from(layout.width) * 0.01) as i32;
    for _ in 0..attempts {
        let mut tries = 0;
        let mut x = rand.next_range(200, layout.width - 200);
        // `WorldGen.cs:17639`: Remix sites moss caves above the rock line instead of below it.
        let (cave_top, cave_bottom) = if layout.remix {
            (
                layout.surface + 50,
                (layout.rock - 50).max(layout.surface + 60),
            )
        } else {
            (layout.rock + 30, layout.height - 230)
        };
        let mut y = rand.next_range(cave_top, cave_bottom);
        let mut found = cave_flood::count(world, x, y, 2500, false, false);
        while (found.tiles >= 2500
            || found.tiles < 10
            || found.lava > 0
            || found.ice > 0
            || found.rock == 0
            || found.shroom > 0)
            && tries < 1000
        {
            tries += 1;
            x = rand.next_range(200, layout.width - 200);
            y = rand.next_range(cave_top, cave_bottom);
            found = cave_flood::count(world, x, y, 2500, false, false);
        }
        if tries < 1000 {
            let (moss_tile, moss_wall) = moss_for_x(x, layout.width, moss_type);
            spread_moss(world, x, y, moss_tile, moss_wall, 2500);
            changed += 1;
        }
    }

    // Unconditional isolated-stone conversion.
    let unconditional = layout.width;
    for _ in 0..unconditional {
        let x = rand.next_range(50, layout.width - 50);
        // `WorldGen.cs:17728`: Remix widens this to the whole world below the surface, where the
        // ordinary world takes the cavern half only.
        let y = if layout.remix {
            rand.next_range(layout.surface, layout.height - 300)
        } else {
            rand.next_range((layout.surface + layout.rock) / 2, layout.underworld)
        };
        let t = world.tile(x, y);
        if t.is_active() && t.block == tiles::STONE {
            let (moss_tile, _) = moss_for_x(x, layout.width, moss_type);
            let mut t = t;
            t.block = moss_tile;
            world.set_tile(x, y, t);
            changed += 1;
        }
    }

    // Exposed-only isolated-stone conversion: only a stone tile with at least one open neighbour.
    let exposed_budget = (f64::from(layout.width) * 0.05) as i32;
    let mut remaining = exposed_budget;
    let mut guard = exposed_budget * 200; // vanilla's own loop has no attempt cap; this keeps a
    // pathological "almost nothing is stone" world finite.
    while remaining > 0 && guard > 0 {
        guard -= 1;
        let x = rand.next_range(50, layout.width - 50);
        // `WorldGen.cs:17755`: Remix draws this band from the lava line up to just past the rock
        // layer, where the ordinary world draws it from the water line down to the underworld.
        let y = if layout.remix {
            rand.next_range(layout.lava_line(), layout.rock + 50)
        } else {
            rand.next_range((layout.surface + layout.rock) / 2, layout.underworld)
        };
        let t = world.tile(x, y);
        let exposed = !world.tile(x - 1, y).is_active()
            || !world.tile(x + 1, y).is_active()
            || !world.tile(x, y - 1).is_active()
            || !world.tile(x, y + 1).is_active();
        if t.is_active() && t.block == tiles::STONE && exposed {
            let (moss_tile, _) = moss_for_x(x, layout.width, moss_type);
            let mut t = t;
            t.block = moss_tile;
            world.set_tile(x, y, t);
            changed += 1;
            remaining -= 1;
        }
    }

    changed
}

fn is_moss(block: u16) -> bool {
    MOSS_TILES.contains(&block)
}

/// The `LongMoss` pass. Returns how many overlay tiles were placed.
///
/// `PlaceTile`'s dedicated `num == 184` branch (`WorldGen.cs:60202-60219`, reached before the
/// generic `Place1x1` path this file used to assume) writes `frameX = style * 18` and
/// `frameY = genRand.Next(3) * 18`. `LongMoss`'s own call (`WorldGen.cs:20951`,
/// `PlaceTile(num3, num4, 184, mute: true)`) always uses the default `style = 0`, so `frameX` is
/// always 0 here — real, not a narrowing. `frameY`'s `Next(3)*18` roll is a purely cosmetic
/// variant with no gameplay meaning, but reproducing it needs a `UnifiedRandom` this function has
/// no parameter for; threading one through means changing this function's public signature, which
/// needs a matching change at its call site in `mod.rs` — outside this lane (single-owner, see the
/// module doc). Fixed at a constant `frameY = 0` (a real, reachable vanilla value — the `Next(3)
/// == 0` case) rather than left at the old -1/-1 corruption sentinel; flagged for whoever next
/// touches `mod.rs`.
pub fn hang_long_moss(world: &mut World) -> usize {
    let (width, height) = (world.width(), world.height());
    let mut placed = 0usize;
    for x in 5..width - 5 {
        for y in 5..height - 5 {
            let t = world.tile(x, y);
            if !(t.is_active() && is_moss(t.block)) {
                continue;
            }
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if !world.tile(nx, ny).is_active() {
                    let mut overlay = world.tile(nx, ny);
                    overlay.block = LONG_MOSS;
                    overlay.frame_x = 0;
                    overlay.frame_y = 0;
                    overlay.flags.set(TileFlags::ACTIVE, true);
                    world.set_tile(nx, ny, overlay);
                    placed += 1;
                }
            }
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrustia_proto::Tile;

    fn stone_world(width: i32, height: i32, seed: i32) -> (World, Layout) {
        let mut world = World::empty(width, height, "moss");
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
    fn a_hollow_pocket_bordering_stone_gets_moss_walled() {
        let (mut world, layout) = stone_world(1200, 900, 5);
        // Within the pass's own real site-search range (`layout.rock + 30 .. height - 230`, both
        // for the initial pick and every retry — see the module doc's note on collapsing vanilla's
        // own two-range asymmetry into one), not merely somewhere plausible.
        let (cx, cy) = (600, layout.rock + 100);
        for x in cx - 8..cx + 8 {
            for y in cy - 8..cy + 8 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        let mut rand = UnifiedRandom::new(5);
        let changed = scatter(&mut world, &layout, &mut rand);
        assert!(changed > 0, "expected at least one moss change");
        let mossed = (cx - 9..cx + 9)
            .flat_map(|x| (cy - 9..cy + 9).map(move |y| (x, y)))
            .filter(|&(x, y)| MOSS_WALLS.contains(&world.tile(x, y).wall))
            .count();
        assert!(
            mossed > 0,
            "the pocket should carry real moss wall afterward"
        );
    }

    #[test]
    fn long_moss_hangs_off_every_exposed_moss_tile() {
        let mut world = World::empty(50, 50, "long-moss");
        world.set_tile(25, 25, Tile::block(MOSS_TILES[0]));
        // All four neighbours start empty.
        let placed = hang_long_moss(&mut world);
        assert_eq!(
            placed, 4,
            "all four cardinal neighbours should get the overlay"
        );
        assert_eq!(world.tile(24, 25).block, LONG_MOSS);
        assert_eq!(world.tile(26, 25).block, LONG_MOSS);
        assert_eq!(world.tile(25, 24).block, LONG_MOSS);
        assert_eq!(world.tile(25, 26).block, LONG_MOSS);
    }

    /// The format writes no frame for an inactive tile, but every *active* frame-important tile
    /// needs a real one — the old `-1/-1` here was the same corruption sentinel already found
    /// (and fixed) for doors, vines and cacti elsewhere in this session, not a valid frame. Fails
    /// on the pre-fix code (`frame_x == -1`).
    #[test]
    fn long_moss_overlay_tiles_are_actually_framed() {
        let mut world = World::empty(50, 50, "long-moss-framed");
        world.set_tile(25, 25, Tile::block(MOSS_TILES[0]));
        hang_long_moss(&mut world);
        for (x, y) in [(24, 25), (26, 25), (25, 24), (25, 26)] {
            let t = world.tile(x, y);
            assert_ne!(t.frame_x, -1, "overlay at ({x},{y}) has a corrupt frame_x");
            assert_ne!(t.frame_y, -1, "overlay at ({x},{y}) has a corrupt frame_y");
        }
    }

    #[test]
    fn non_moss_tiles_get_no_overlay() {
        let mut world = World::empty(50, 50, "long-moss-none");
        world.set_tile(25, 25, Tile::block(tiles::STONE));
        let placed = hang_long_moss(&mut world);
        assert_eq!(placed, 0);
    }

    #[test]
    fn a_small_world_does_not_panic() {
        let (mut world, layout) = stone_world(400, 300, 1);
        let mut rand = UnifiedRandom::new(1);
        assert_eq!(scatter(&mut world, &layout, &mut rand), 0);
        let _ = hang_long_moss(&mut world);
    }
}
