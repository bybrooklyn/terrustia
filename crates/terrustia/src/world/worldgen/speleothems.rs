//! Speleothems (ceiling/floor stalactites), exposed surface gems, and the shared web/honey/spider
//! pass they turn out to share a placer with.
//!
//! Transcribed from four real vanilla passes, bundled the way `plan.md`'s own sizing table already
//! groups them (`SpeleothemsAndGemTrees`+2 exposed-gem passes+shared web pass, 229 lines):
//!
//! * `SpeleothemsAndGemTrees` (`WorldGen.cs:22228-22314`, 87 lines) — the driving speleothem scan,
//!   underground and along the sky above the surface.
//! * `ExposedGemsInIceBiome` (`:20860-20891`, 32) and `ExposedGemsUnderground` (`:20892-20932`, 41).
//! * `WebsInSpiderCavesAndHoneyPlusSpeleothemsInBeehives` (`:20158-20226`, 69) — named "shared" in
//!   the sizing table because its own Hive-wall branch calls the very same stalactite placer
//!   `SpeleothemsAndGemTrees` does.
//!
//! 87+32+41+69 = 229, matching the sizing table exactly.
//!
//! **The shared placer, `place_tight`/`place_unchecked_stalactite`, transcribed from `PlaceTight`/
//! `PlaceUncheckedStalactite`** (`WorldGen.cs:38704-39035`, ~330 lines) — **narrowed, disclosed**.
//! Vanilla's version carries three independent axes of detail this module does not: a `variation`
//! (0-2, picked at random, `frameX` offset by `variation*18`) and a `preferSmall`/`!preferSmall`
//! choice (one tile vs. a two-tile-tall icicle, `frameY` differing per tile). Both are dropped —
//! every stalactite/stalagmite placed here is a single tile, `variation` fixed at 0 — keeping only
//! the axis that actually changes *which* decoration a player sees: which of seven real material
//! families (Ice; Stone/Moss/Pearlstone/Ebonstone/Crimstone; Hive; Sandstone/HardenedSand; Granite;
//! Marble) the anchoring ceiling or floor tile belongs to, and whether it's a ceiling-hanging
//! stalactite or a floor-growing stalagmite. **One real, faithfully-preserved vanilla asymmetry**:
//! the floor (stalagmite) side has no Ice-family branch at all in source — icicles only ever hang
//! from an ice ceiling, never grow from an ice floor. Not corrected; that omission is real vanilla
//! behaviour, the same "keep a found asymmetry rather than silently fixing it" rule
//! `dirt_wall_cleanup.rs` already applied. `CheckStalactite`'s neighbour-run frame-merging (purely
//! cosmetic run-length styling, not placement) is also not transcribed.
//!
//! **`SpeleothemsAndGemTrees` itself lands without `TryGrowingTreeByType`'s "gem tree" branch** — a
//! genuinely separate, rare decorative-tree subsystem (its own tile-growing state machine over
//! seven gem-coloured tree types, `TileID.cs`'s 583-589), the same class of cut `MahoganyTreeBiome`
//! was disclosed-skipped for in `micro_biomes.rs`. What's kept: the full-underground stalactite scan
//! *and* the above-surface "sky" scan for icicles hanging under snow/desert/evil overhangs — both
//! real, both cheap once `place_tight` exists, and both reachable from the same loop vanilla runs
//! them in.
//!
//! No `StructureMap` dependency — nothing here places a discrete, space-reserving structure.

use terrustia_proto::{Tile, TileFlags};

use super::layout::Layout;
use super::rand::UnifiedRandom;
use super::smooth::solid_tile;
use super::tiles;
use crate::world::World;

const STALACTITE: u16 = 165;
/// `TileID.Sets.Ices` plus `TileID.Snow` — the ceiling-only material family.
fn is_ice_family(block: u16) -> bool {
    matches!(block, tiles::SNOW | tiles::ICE | 163 | 164 | 200)
}
/// Stone, every moss colour, Pearlstone, Ebonstone, Crimstone.
fn is_stone_family(block: u16) -> bool {
    block == tiles::STONE
        || (179..=183).contains(&block)
        || block == 117
        || block == tiles::EBONSTONE
        || block == tiles::CRIMSTONE
}

/// The `frameX` base for a stalactite/stalagmite anchored to `block`, `None` if `block` is not one
/// of the seven real material families `PlaceUncheckedStalactite` recognises.
fn material_base(block: u16) -> Option<i16> {
    if is_ice_family(block) {
        return Some(0);
    }
    if is_stone_family(block) {
        return Some(54);
    }
    match block {
        tiles::HIVE => Some(162),
        tiles::SANDSTONE | tiles::HARDENED_SAND => Some(378),
        tiles::GRANITE => Some(432),
        tiles::MARBLE => Some(486),
        _ => None,
    }
}

/// `PlaceTight(x, y)`, `spiders: false` always (no call site in this bundle ever passes `true` —
/// that branch belongs to `Spread.Spider`, already transcribed separately in `spider_caves.rs`).
/// The `anyShimmer()`/`Larva` early-return is transcribed as the Larva half only — this project has
/// no shimmer concept to check the other half against.
fn place_tight(world: &mut World, x: i32, y: i32) {
    let here = world.tile(x, y);
    if here.is_active() && here.block == tiles::LARVA {
        return;
    }
    place_unchecked_stalactite(world, x, y);
}

/// `PlaceUncheckedStalactite`, narrowed per the module doc — single-tile, `variation` fixed at 0.
fn place_unchecked_stalactite(world: &mut World, x: i32, y: i32) {
    if solid_tile(world, x, y - 1)
        && !world.tile(x, y).is_active()
        && !world.tile(x, y + 1).is_active()
    {
        if let Some(base) = material_base(world.tile(x, y - 1).block) {
            let mut t = world.tile(x, y);
            t.block = STALACTITE;
            t.frame_x = base;
            t.frame_y = 72;
            t.slope = 0;
            t.flags.set(TileFlags::ACTIVE, true);
            world.set_tile(x, y, t);
        }
        return;
    }
    if solid_tile(world, x, y + 1)
        && !world.tile(x, y).is_active()
        && !world.tile(x, y - 1).is_active()
    {
        let floor = world.tile(x, y + 1).block;
        // No Ice-family floor branch in vanilla — see the module doc's disclosed asymmetry.
        if is_ice_family(floor) {
            return;
        }
        if let Some(base) = material_base(floor) {
            let mut t = world.tile(x, y);
            t.block = STALACTITE;
            t.frame_x = base;
            t.frame_y = 90;
            t.slope = 0;
            t.flags.set(TileFlags::ACTIVE, true);
            world.set_tile(x, y, t);
        }
    }
}

/// The `SpeleothemsAndGemTrees` pass (gem-tree branch disclosed-skipped — see the module doc).
/// Returns how many stalactites/stalagmites were placed.
pub fn scatter(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) -> usize {
    if layout.width < 45 || layout.surface < 5 {
        return 0;
    }
    let mut placed = 0usize;
    for x in 20..layout.width - 20 {
        for y in layout.surface..world.height() - 20 {
            if rand.next_max(5) == 0
                && !world.tile(x, y).is_active()
                && world.tile(x, y).liquid == 0
                && place_tight_counted(world, x, y)
            {
                placed += 1;
            }
        }
        for y in 5..layout.surface {
            let above = world.tile(x, y - 1);
            let below = world.tile(x, y + 1);
            if above.is_active()
                && is_ice_family(above.block)
                && rand.next_max(5) == 0
                && place_tight_counted(world, x, y)
            {
                placed += 1;
            }
            if above.is_active()
                && (above.block == tiles::EBONSTONE || above.block == tiles::CRIMSTONE)
                && rand.next_max(5) == 0
                && place_tight_counted(world, x, y)
            {
                placed += 1;
            }
            if below.is_active()
                && (below.block == tiles::EBONSTONE || below.block == tiles::CRIMSTONE)
                && rand.next_max(5) == 0
                && place_tight_counted(world, x, y)
            {
                placed += 1;
            }
        }
    }
    placed
}

fn place_tight_counted(world: &mut World, x: i32, y: i32) -> bool {
    place_tight(world, x, y);
    world.tile(x, y).is_active() && world.tile(x, y).block == STALACTITE
}

/// `WebsInSpiderCavesAndHoneyPlusSpeleothemsInBeehives`. Returns
/// `(honey_marked, hive_stalactites, cobwebs_placed)`.
pub fn shared_web_and_honey(
    world: &mut World,
    layout: &Layout,
    rand: &mut UnifiedRandom,
) -> (usize, usize, usize) {
    if layout.width < 210 || layout.surface < 1 {
        return (0, 0, 0);
    }
    let (mut honey, mut hive_stalactites, mut cobwebs) = (0usize, 0usize, 0usize);
    for x in 100..layout.width - 100 {
        for y in layout.surface..world.height() - 100 {
            let t = world.tile(x, y);
            if t.wall == tiles::walls::HIVE {
                if t.liquid > 0 {
                    let mut wet = t;
                    wet.liquid_kind = terrustia_proto::Liquid::Honey;
                    world.set_tile(x, y, wet);
                    honey += 1;
                }
                if rand.next_max(3) == 0 && place_tight_counted(world, x, y) {
                    hive_stalactites += 1;
                }
            }
            if t.wall == 62 {
                // `WallID.SpiderUnsafe` — dry it (the `noSpiderCavesILiedMoreSpiderCaves` secret
                // seed branch, which turns liquid to honey instead, is out of scope).
                if t.liquid > 0 {
                    let mut dry = t;
                    dry.liquid = 0;
                    world.set_tile(x, y, dry);
                }
                if !world.tile(x, y).is_active() && rand.next_max(10) != 0 {
                    let reach = rand.next_range(2, 5);
                    let mut solid_nearby = false;
                    for k in x - reach..=x + reach {
                        for l in y - reach..=y + reach {
                            if solid_tile(world, k, l) {
                                solid_nearby = true;
                            }
                        }
                    }
                    if solid_nearby {
                        let mut web = Tile::block(tiles::COBWEB);
                        web.wall = world.tile(x, y).wall;
                        world.set_tile(x, y, web);
                        cobwebs += 1;
                    }
                }
            }
        }
    }
    (honey, hive_stalactites, cobwebs)
}

/// `randGemTile`'s own weighted roll, transcribed identically for both exposed-gem passes:
/// `genRand.Next(12)` in `{0,1,2}->amethyst`, `{3,4,5}->topaz`, `{6,7}->sapphire`, `{8,9}->emerald`,
/// `{10}->ruby`, `{11}->diamond`.
fn roll_gem_frame(rand: &mut UnifiedRandom) -> i16 {
    match rand.next_max(12) {
        0..=2 => 0,
        3..=5 => 1,
        6 | 7 => 2,
        8 | 9 => 3,
        10 => 4,
        _ => 5,
    }
}

const GEM_TILE: u16 = 178;

/// `PlaceTile`'s dedicated `num == 178` branch (`WorldGen.cs:60190-60200`) — not the generic
/// `Place1x1` default case: `frameX = style * 18` (the actual gem species — `KillTile`'s own drop
/// table reads this back as `frameX / 18`, `WorldGen.cs:66018`) and `frameY = genRand.Next(3) *
/// 18` (a purely cosmetic variant, no gameplay meaning). The old code had this backwards — species
/// on `frameY`, `frameX` left at the -1 "unframed" sentinel — which corrupted every placed gem and
/// made every one of them drop as Amethyst (`-1 / 18 == 0` truncates to the frame-0 case).
fn place_gem_at(world: &mut World, x: i32, y: i32, frame: i16, rand: &mut UnifiedRandom) -> bool {
    if world.tile(x, y).is_active() {
        return false;
    }
    let mut t = Tile::AIR;
    t.block = GEM_TILE;
    t.frame_x = frame * 18;
    t.frame_y = rand.next_max(3) as i16 * 18;
    t.flags.set(TileFlags::ACTIVE, true);
    world.set_tile(x, y, t);
    true
}

/// `ExposedGemsInIceBiome`. Returns how many gem tiles were placed.
pub fn exposed_gems_in_ice_biome(
    world: &mut World,
    layout: &Layout,
    rand: &mut UnifiedRandom,
) -> usize {
    if layout.snow.to <= layout.snow.from || layout.rock >= world.height() - 20 {
        return 0;
    }
    let mut placed = 0usize;
    let attempts = ((f64::from(layout.width)) * 0.25) as i32;
    for _ in 0..attempts {
        // `WorldGen.cs:20867`: Remix widens this band to the whole world below the surface.
        let y = if layout.remix {
            rand.next_range(layout.surface, world.height() - 300)
        } else {
            rand.next_range((layout.surface + layout.rock) / 2, layout.underworld)
        };
        let x = rand.next_range(
            layout.snow.from.max(2),
            layout.snow.to.min(layout.width - 2).max(3),
        );
        let t = world.tile(x, y);
        if !(t.is_active() && (is_ice_family(t.block) || t.block == super::tiles::BREAKABLE_ICE)) {
            continue;
        }
        let frame = roll_gem_frame(rand);
        let (a, b, c, d) = (
            rand.next_range(1, 4),
            rand.next_range(1, 4),
            rand.next_range(1, 4),
            rand.next_range(1, 4),
        );
        for j in x - a..x + b {
            for k in y - c..y + d {
                if place_gem_at(world, j, k, frame, rand) {
                    placed += 1;
                }
            }
        }
    }
    placed
}

/// `ExposedGemsUnderground`: a plain single-tile scatter through any open, dry underground pocket,
/// plus a second small-cluster scatter behind desert (Sandstone/HardenedSand) wall specifically.
pub fn exposed_gems_underground(
    world: &mut World,
    layout: &Layout,
    rand: &mut UnifiedRandom,
) -> usize {
    if layout.width < 45 || layout.rock >= world.height() - 300 {
        return 0;
    }
    let mut placed = 0usize;
    for _ in 0..layout.width {
        let x = rand.next_range(20, layout.width - 20);
        // The underground gem pass follows the same inversion: Remix lifts it above the rock line.
        let y = if layout.remix {
            let (top, bottom) = layout.deep_band();
            rand.next_range(top, bottom)
        } else {
            rand.next_range(layout.rock, world.height() - 300)
        };
        let t = world.tile(x, y);
        if !t.is_active() && t.liquid == 0 && t.wall != tiles::walls::LIHZAHRD_BRICK {
            let frame = roll_gem_frame(rand);
            if place_gem_at(world, x, y, frame, rand) {
                placed += 1;
            }
        }
    }
    for _ in 0..layout.width {
        let x = rand.next_range(20, layout.width - 20);
        let y = rand.next_range(layout.surface, world.height() - 300);
        let t = world.tile(x, y);
        if !t.is_active()
            && t.liquid == 0
            && (t.wall == tiles::walls::HARDENED_SAND || t.wall == tiles::walls::SANDSTONE)
        {
            let (a, b, c, d) = (
                rand.next_range(1, 4),
                rand.next_range(1, 4),
                rand.next_range(1, 4),
                rand.next_range(1, 4),
            );
            for j in x - a..x + b {
                for k in y - c..y + d {
                    // Vanilla's own frame here is a fixed 6 (a specific desert-gem-cluster frame),
                    // not a fresh `roll_gem_frame` draw.
                    if place_gem_at(world, j, k, 6, rand) {
                        placed += 1;
                    }
                }
            }
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrustia_proto::Liquid;

    fn stone_world(width: i32, height: i32, seed: i32) -> (World, Layout) {
        let mut world = World::empty(width, height, "speleothems");
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
    fn a_stalactite_needs_solid_rock_above_and_open_space_below() {
        let mut world = World::empty(50, 50, "stalactite");
        world.set_tile(25, 24, Tile::block(tiles::STONE));
        place_unchecked_stalactite(&mut world, 25, 25);
        assert_eq!(world.tile(25, 25).block, STALACTITE);
        assert_eq!(world.tile(25, 25).frame_y, 72);
    }

    #[test]
    fn a_stalagmite_needs_solid_rock_below_and_open_space_above() {
        let mut world = World::empty(50, 50, "stalagmite");
        world.set_tile(25, 26, Tile::block(tiles::STONE));
        place_unchecked_stalactite(&mut world, 25, 25);
        assert_eq!(world.tile(25, 25).block, STALACTITE);
        assert_eq!(world.tile(25, 25).frame_y, 90);
    }

    #[test]
    fn ice_never_grows_a_stalagmite_from_the_floor() {
        let mut world = World::empty(50, 50, "ice-floor");
        world.set_tile(25, 26, Tile::block(tiles::ICE));
        place_unchecked_stalactite(&mut world, 25, 25);
        assert!(
            !world.tile(25, 25).is_active(),
            "vanilla has no ice-floor stalagmite branch"
        );
    }

    #[test]
    fn a_real_generated_world_gets_real_speleothems() {
        let (mut world, layout) = stone_world(1200, 900, 3);
        // Carve a tall open shaft with solid stone ceilings/floors dotted along it so the scan
        // finds real candidates.
        for y in layout.surface..900 - 20 {
            for x in 595..605 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        let mut rand = UnifiedRandom::new(3);
        let placed = scatter(&mut world, &layout, &mut rand);
        assert!(
            placed > 0,
            "expected at least one speleothem in a tall open shaft"
        );
    }

    #[test]
    fn honey_liquid_behind_a_hive_wall_is_marked_honey() {
        let (mut world, layout) = stone_world(1200, 900, 4);
        let (x, y) = (600, layout.surface + 50);
        let mut wet = Tile::AIR;
        wet.wall = tiles::walls::HIVE;
        wet.liquid_kind = Liquid::Water;
        wet.liquid = 200;
        world.set_tile(x, y, wet);
        let mut rand = UnifiedRandom::new(4);
        let (honey, _hive, _cobwebs) = shared_web_and_honey(&mut world, &layout, &mut rand);
        assert!(honey > 0, "expected at least the one honey conversion");
        assert_eq!(world.tile(x, y).liquid_kind, Liquid::Honey);
    }

    #[test]
    fn a_spider_wall_pocket_near_solid_rock_gets_cobwebs() {
        let (mut world, layout) = stone_world(1200, 900, 6);
        let (cx, cy) = (600, layout.surface + 60);
        for x in cx - 5..cx + 5 {
            for y in cy - 5..cy + 5 {
                let mut air = Tile::AIR;
                air.wall = 62;
                world.set_tile(x, y, air);
            }
        }
        let mut rand = UnifiedRandom::new(6);
        let (_honey, _hive, cobwebs) = shared_web_and_honey(&mut world, &layout, &mut rand);
        assert!(
            cobwebs > 0,
            "expected cobwebs scattered near solid rock behind spider wall"
        );
    }

    #[test]
    fn exposed_gems_place_in_the_ice_biome() {
        let (mut world, layout) = stone_world(1200, 900, 8);
        // A checkerboard of ice (even rows) and open air (odd rows): `place_gem_at` only succeeds
        // on an *inactive* tile, and vanilla's own window scan places gems in the open space
        // *around* an ice seed point, not through solid ice — a solid block of ice with no open
        // neighbour anywhere (the first draft of this test) can never actually place anything.
        for x in layout.snow.from..layout.snow.to {
            for y in (layout.rock + 20)..(layout.rock + 60) {
                let tile = if y % 2 == 0 {
                    Tile::block(tiles::ICE)
                } else {
                    Tile::AIR
                };
                world.set_tile(x, y, tile);
            }
        }
        let mut rand = UnifiedRandom::new(8);
        let placed = exposed_gems_in_ice_biome(&mut world, &layout, &mut rand);
        assert!(
            placed > 0,
            "expected exposed gems inside a real ice-biome mass"
        );
    }

    /// `KillTile`'s own drop table for tile 178 reads `frameX / 18` to pick the item
    /// (`WorldGen.cs:66018`: 0→Amethyst, 1→Topaz, 2→Sapphire, 3→Emerald, 4→Ruby, 5→Diamond). The
    /// old code stored the species on `frameY` and left `frameX` at the -1 "unframed" sentinel —
    /// so every exposed gem was frame-corrupt and would have dropped Amethyst regardless of which
    /// species it was meant to be (`-1 / 18` truncates to 0). Fails on the pre-fix code
    /// (`frame_x == -1`).
    #[test]
    fn a_placed_gems_species_is_stored_in_frame_x_not_frame_y() {
        let mut world = World::empty(50, 50, "gem-frame");
        let mut rand = UnifiedRandom::new(1);
        // frame 3 = Emerald.
        assert!(place_gem_at(&mut world, 10, 10, 3, &mut rand));
        let t = world.tile(10, 10);
        assert_eq!(t.block, GEM_TILE);
        assert_eq!(
            t.frame_x,
            3 * 18,
            "the gem species must be stored in frame_x (real vanilla reads frameX/18 to pick \
             the drop), not corrupted to -1"
        );
        assert_ne!(
            t.frame_y, -1,
            "frame_y must not carry the -1 corruption sentinel either"
        );
    }

    #[test]
    fn exposed_gems_place_underground_in_the_open() {
        let (mut world, layout) = stone_world(1200, 900, 9);
        for x in 590..610 {
            for y in (layout.rock + 20)..(layout.rock + 60) {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        let mut rand = UnifiedRandom::new(9);
        let placed = exposed_gems_underground(&mut world, &layout, &mut rand);
        assert!(placed > 0, "expected exposed gems in an open dry pocket");
    }

    #[test]
    fn a_small_world_does_not_panic() {
        let (mut world, layout) = stone_world(400, 300, 1);
        let mut rand = UnifiedRandom::new(1);
        let _ = scatter(&mut world, &layout, &mut rand);
        let _ = shared_web_and_honey(&mut world, &layout, &mut rand);
        let _ = exposed_gems_in_ice_biome(&mut world, &layout, &mut rand);
        let _ = exposed_gems_underground(&mut world, &layout, &mut rand);
    }
}
