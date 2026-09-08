//! The ground itself: a surface line, the layers under it, and the biomes laid over both.
//!
//! This is the first thing that writes tiles and everything after it carves into what it leaves.
//! The order matters — a cave dug before the jungle is placed becomes a jungle cave, which is
//! what makes the world feel like one place rather than several pasted together.

use terrustia_proto::Tile;

use super::layout::{Evil, Layout, Surface};
use super::rand::UnifiedRandom;
use super::tiles::{self, walls};
use crate::world::World;

/// How deep the dirt goes before stone begins, at the surface.
const DIRT_DEPTH: i32 = 42;
/// How far the surface may wander from its average.
const ROLL: i32 = 24;
/// How deep an ocean's water reaches below the surface line.
const OCEAN_DEPTH: i32 = 45;

/// The height of the surface at every column.
///
/// Built as a walk rather than a formula so it can be steered: an ocean drops away, a desert sits
/// low and flat, a jungle undulates. A pure sine surface is recognisable as one at a glance and
/// makes every world feel the same.
pub fn heightmap(layout: &Layout, rand: &mut UnifiedRandom) -> Vec<i32> {
    let mut heights = Vec::with_capacity(layout.width as usize);
    let mut height = layout.surface;
    // How strongly the surface is currently trending up or down, so it makes hills rather than
    // noise. Terraria calls the same idea `worldSurfaceHigh`/`Low` and walks between them.
    let mut drift = 0i32;

    for x in 0..layout.width {
        let biome = layout.surface_biome(x);

        // Oceans are carved afterwards, in a second pass, once the land's own height at the
        // shore is known. Doing it inline cannot work: the left ocean is generated *before* the
        // land it meets, so its shore has nothing to match and the seam comes out as a cliff.
        if biome == Some(Surface::Ocean) {
            heights.push(height);
            continue;
        }

        // Each other biome pulls the surface towards its own level.
        let wanted = match biome {
            Some(Surface::Desert) => layout.surface + 6,
            Some(Surface::Jungle) => layout.surface + 10,
            Some(Surface::Snow) => layout.surface - 8,
            _ => layout.surface,
        };

        // Drift changes slowly and is nudged towards whatever this biome wants, so the join
        // between two biomes is a slope rather than a cliff.
        if rand.next_max(6) == 0 {
            drift += rand.next_range(-1, 2);
            drift = drift.clamp(-2, 2);
        }
        height += drift;
        height += (wanted - height).signum() * i32::from(rand.next_max(3) == 0);
        height = height.clamp(layout.surface - ROLL, layout.surface + 20);

        heights.push(height);
    }

    carve_oceans(layout, &mut heights);
    heights
}

/// Shape the two oceans, now that the height of the land they meet is known.
///
/// Each is a basin: the shore matches the land beside it exactly, so there is no seam, and the
/// floor falls away towards the world's edge. Squared rather than linear, so the beach shelves
/// gently and the far end is properly deep — a linear slope gives a wedge that fills with a
/// centimetre of water at the shore and looks wrong.
fn carve_oceans(layout: &Layout, heights: &mut [i32]) {
    for (band, shore_x, edge_x) in [
        (layout.ocean_left, layout.ocean_left.to, 0),
        (
            layout.ocean_right,
            layout.ocean_right.from,
            layout.width - 1,
        ),
    ] {
        let shore_height = heights
            .get(shore_x.clamp(0, layout.width - 1) as usize)
            .copied()
            .unwrap_or(layout.surface);
        let span = (shore_x - edge_x).abs().max(1);
        for x in band.from..band.to {
            let along = f64::from((x - shore_x).abs().min(span)) / f64::from(span);
            let depth = (along * along * f64::from(OCEAN_DEPTH)) as i32;
            if let Some(slot) = heights.get_mut(x as usize) {
                *slot = shore_height + depth;
            }
        }
    }
}

/// Fill the world: sky above the line, layers below it, biome materials over the top.
pub fn fill(world: &mut World, layout: &Layout, heights: &[i32], rand: &mut UnifiedRandom) {
    for x in 0..layout.width {
        let top = heights[x as usize];
        let biome = layout.surface_biome(x);

        for y in top..layout.height {
            let depth = y - top;
            let block = material(layout, biome, x, y, depth, rand);
            let mut tile = Tile::block(block);
            // The top two rows show sky behind them; below that a wall, or a cave's background
            // once something carves into it.
            tile.wall = if depth < 2 {
                0
            } else {
                wall_for(layout, biome, y)
            };
            world.set_tile(x, y, tile);
        }

        // An ocean is water over sand, and the water is what makes it an ocean rather than a dip.
        // Filled from the shore line down to the floor, so a shelving beach fills shallowly and
        // the far end fills deep.
        if biome == Some(Surface::Ocean) {
            let waterline = layout.surface + 5;
            for y in waterline..top {
                let mut tile = Tile::AIR;
                tile.liquid = 255;
                tile.liquid_kind = terrustia_proto::Liquid::Water;
                world.set_tile(x, y, tile);
            }
        }
    }
}

/// What a tile is made of at a given place.
fn material(
    layout: &Layout,
    biome: Option<Surface>,
    x: i32,
    y: i32,
    depth: i32,
    rand: &mut UnifiedRandom,
) -> u16 {
    // The underworld is ash whatever is above it.
    if y >= layout.underworld {
        return tiles::ASH;
    }

    match biome {
        Some(Surface::Ocean) => {
            if depth < 30 {
                tiles::SAND
            } else if y < layout.rock {
                tiles::SANDSTONE
            } else {
                tiles::STONE
            }
        }
        Some(Surface::Desert) => {
            if depth == 0 || depth < 6 {
                tiles::SAND
            } else if depth < 40 {
                tiles::HARDENED_SAND
            } else if y < layout.rock + 60 {
                tiles::SANDSTONE
            } else {
                tiles::STONE
            }
        }
        Some(Surface::Snow) => {
            if depth < 4 {
                tiles::SNOW
            } else if depth < 55 {
                if rand.next_max(3) == 0 {
                    tiles::SNOW
                } else {
                    tiles::ICE
                }
            } else if y < layout.rock {
                tiles::ICE
            } else {
                tiles::STONE
            }
        }
        // The jungle is mud all the way down to the caverns, which is what makes it one place
        // rather than a surface with stone under it.
        Some(Surface::Jungle) => {
            if depth == 0 {
                tiles::JUNGLE_GRASS
            } else if y < layout.rock + 120 {
                tiles::MUD
            } else {
                tiles::STONE
            }
        }
        Some(Surface::Evil) => {
            let (grass, stone, sand) = match layout.evil_at(x) {
                Evil::Corruption => (tiles::CORRUPT_GRASS, tiles::EBONSTONE, tiles::EBONSAND),
                Evil::Crimson => (tiles::CRIMSON_GRASS, tiles::CRIMSTONE, tiles::CRIMSAND),
            };
            if depth == 0 {
                grass
            } else if depth < DIRT_DEPTH {
                tiles::DIRT
            } else if y < layout.rock + 80 {
                stone
            } else if rand.next_max(9) == 0 {
                sand
            } else {
                tiles::STONE
            }
        }
        None => {
            if depth == 0 {
                tiles::GRASS
            } else if depth < DIRT_DEPTH {
                tiles::DIRT
            } else if y < layout.rock {
                // The band between dirt and rock is mixed, which is what gives a cave wall its
                // streaks rather than a hard line.
                if rand.next_max(4) == 0 {
                    tiles::DIRT
                } else {
                    tiles::STONE
                }
            } else if rand.next_max(60) == 0 {
                tiles::CLAY
            } else {
                tiles::STONE
            }
        }
    }
}

/// Which wall sits behind a tile.
///
/// **The cavern layer's plain stone carries no wall, because vanilla's does not.** The only
/// terrain-time wall vanilla paints is `DirtWallBackgrounds` (`WorldGen.cs:11895-11933`), and that
/// pass walks each column only from the first fully-buried row down to `worldSurface + num` with
/// `num` a 0..10 random walk. Everything below that boundary - the whole underground and cavern
/// layers - is left at wall 0 by terrain generation, and stays that way until `CaveWallVariety`
/// (`WorldGen.cs:16801`) and `CaveWallsInEnclosedSpaces` (`WorldGen.cs:17834`) paint wall back into
/// selected *open* pockets near the end of the pipeline. No biome pass adds one either: `IceBiome`
/// (`WorldGen.cs:12400-12441`) and `CorruptionAndCrimson` (`WorldGen.cs:14127-14547`) only
/// *convert* a wall that some other pass already placed (2 to 40, 216 to 218, 187 to 221), never
/// create one where there was none.
///
/// This is not cosmetic. `nextCount` (`WorldGen.cs:9539-9543`, transcribed as
/// [`super::cave_flood::count`]) saturates its whole search the instant the fill touches *any*
/// walled tile, and the fill reads the tile before it checks whether it is solid, so a walled tile
/// on a pocket's own stone boundary is enough. Painting the cavern layer's stone with a wall here
/// meant every `GemCaves`/`SpiderCaves`/`CaveWallsInEnclosedSpaces` measurement stopped on the
/// first boundary tile it reached and reported "too big" for every candidate in the world.
///
/// The jungle is the one real exception, and it is kept: `GenVars.mudWall` makes every `TileRunner`
/// step place a Mud or JungleUnsafe wall as it carves (`WorldGen.cs:77778-77792`), so jungle caves
/// genuinely do have a wall behind them in vanilla and genuinely are invisible to the same
/// measurement. The surface crust above `surface + DIRT_DEPTH` also keeps its wall: vanilla's own
/// crust is thinner (`worldSurface + 0..10` rather than 42 rows) and that difference is a separate,
/// purely cosmetic parity gap, deliberately not widened into this change.
fn wall_for(layout: &Layout, biome: Option<Surface>, y: i32) -> u16 {
    if y >= layout.underworld {
        return 0; // the underworld shows the sky behind it, which is what makes it look open
    }
    match biome {
        Some(Surface::Ocean) if y < layout.rock => walls::SANDSTONE,
        Some(Surface::Desert) if y < layout.rock => walls::HARDENED_SAND,
        Some(Surface::Snow) if y < layout.rock => walls::SNOW,
        Some(Surface::Jungle) if y < layout.rock + 120 => walls::JUNGLE,
        Some(Surface::Evil) | None if y < layout.surface + DIRT_DEPTH => walls::DIRT,
        _ => 0,
    }
}

/// Clear a pocket of sky so a player cannot arrive inside the ground.
///
/// The whole tile is replaced rather than having its active bit cleared, keeping only the wall.
/// A tile that is inactive but still carries a frame is inconsistent: the format writes no frame
/// for an inactive tile, so such a tile reads back different from what was written.
pub fn clear_spawn(world: &mut World, x: i32, y: i32) {
    for cx in x - 8..=x + 8 {
        for cy in y - 10..y {
            if !world.in_bounds(cx, cy) {
                continue;
            }
            let wall = world.tile(cx, cy).wall;
            let mut tile = Tile::AIR;
            tile.wall = wall;
            world.set_tile(cx, cy, tile);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned() -> (Layout, Vec<i32>, UnifiedRandom) {
        let mut rand = UnifiedRandom::new(4242);
        let layout = Layout::plan(2400, 900, &mut rand);
        let heights = heightmap(&layout, &mut rand);
        (layout, heights, rand)
    }

    /// The surface stays inside the world and near where it was asked to be.
    #[test]
    fn the_surface_stays_where_it_belongs() {
        let (layout, heights, _) = planned();
        assert_eq!(heights.len(), layout.width as usize);
        for (x, &h) in heights.iter().enumerate() {
            assert!(
                h > 0 && h < layout.rock,
                "column {x} has its surface at {h}, outside the dirt layer"
            );
        }
    }

    /// It rolls rather than stepping: no cliff between neighbouring columns.
    #[test]
    fn the_surface_is_continuous() {
        let (_, heights, _) = planned();
        for pair in heights.windows(2) {
            assert!(
                (pair[0] - pair[1]).abs() <= 3,
                "a step of {} between columns is a cliff, not a hill",
                (pair[0] - pair[1]).abs()
            );
        }
    }

    /// The oceans dip well below the rest of the land, or they hold no water.
    #[test]
    fn the_oceans_are_lower_than_the_land() {
        let (layout, heights, _) = planned();
        let ocean = heights[10];
        let inland = heights[layout.spawn_x as usize];
        assert!(
            ocean > inland + 20,
            "the ocean at {ocean} is not below the land at {inland}"
        );
    }

    /// Every biome actually produces its own material.
    #[test]
    fn each_biome_lays_its_own_ground() {
        let mut rand = UnifiedRandom::new(99);
        let layout = Layout::plan(4200, 1200, &mut rand);
        let heights = heightmap(&layout, &mut rand);
        let mut world = World::empty(4200, 1200, "biomes");
        fill(&mut world, &layout, &heights, &mut rand);

        let surface_at = |x: i32| world.tile(x, heights[x as usize]).block;
        assert_eq!(surface_at(layout.jungle.centre()), tiles::JUNGLE_GRASS);
        assert_eq!(surface_at(layout.snow.centre()), tiles::SNOW);
        assert_eq!(surface_at(layout.desert.centre()), tiles::SAND);
        assert_eq!(surface_at(10), tiles::SAND, "the ocean floor is sand");
        let evil = surface_at(layout.evil_band.centre());
        assert!(
            evil == tiles::CORRUPT_GRASS || evil == tiles::CRIMSON_GRASS,
            "the evil band should be evil grass, got {evil}"
        );
    }

    /// The underworld is ash, wall to wall.
    #[test]
    fn the_underworld_is_ash() {
        let mut rand = UnifiedRandom::new(5);
        let layout = Layout::plan(2400, 900, &mut rand);
        let heights = heightmap(&layout, &mut rand);
        let mut world = World::empty(2400, 900, "hell");
        fill(&mut world, &layout, &heights, &mut rand);
        for x in (0..2400).step_by(97) {
            assert_eq!(
                world.tile(x, layout.underworld + 20).block,
                tiles::ASH,
                "column {x} of the underworld is not ash"
            );
        }
    }

    /// An ocean holds water.
    #[test]
    fn the_oceans_hold_water() {
        let mut rand = UnifiedRandom::new(11);
        let layout = Layout::plan(2400, 900, &mut rand);
        let heights = heightmap(&layout, &mut rand);
        let mut world = World::empty(2400, 900, "sea");
        fill(&mut world, &layout, &heights, &mut rand);
        let wet = (0..40)
            .filter(|&x| {
                (layout.surface..layout.surface + OCEAN_DEPTH).any(|y| world.tile(x, y).liquid > 0)
            })
            .count();
        assert!(
            wet > 20,
            "only {wet} of the first forty ocean columns are wet"
        );
    }

    /// Spawn is cleared, so a player never arrives inside rock.
    #[test]
    fn spawn_is_hollowed_out() {
        let mut rand = UnifiedRandom::new(3);
        let layout = Layout::plan(2400, 900, &mut rand);
        let heights = heightmap(&layout, &mut rand);
        let mut world = World::empty(2400, 900, "spawn");
        fill(&mut world, &layout, &heights, &mut rand);
        let (sx, sy) = (layout.spawn_x, heights[layout.spawn_x as usize]);
        clear_spawn(&mut world, sx, sy);
        for y in sy - 8..sy {
            assert!(
                !world.tile(sx, y).is_active(),
                "spawn is still solid at {y}"
            );
        }
    }
}
