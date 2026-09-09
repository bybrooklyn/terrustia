//! "Skyblock": no world, only islands.
//!
//! Transcribed from the `Skyblock` gate class (`.scratch/decompiled/Terraria/WorldGen.cs:3097-3113`)
//! and the hundred `!Skyblock.denyAllGeneration` / `!Skyblock.denySomeGeneration` guards it feeds.
//!
//! The seventh of the nine secret seeds to get real generation content.
//!
//! # Why this one is a branch and not a pass
//!
//! Every other seed here *changes* generation. Skyblock cancels it. `denyAllGeneration` is a bare
//! `=> skyblockWorldGen`, and it gates the terrain, the caves, the biomes, the dungeon, the temple,
//! the underworld, the ores, the traps, the chests and the decoration - a hundred call sites,
//! almost all of them the same shape. `micro_biomes.rs` sized this correctly when it said real
//! vanilla's generation under this flag "is close to a different generator ... not a set of
//! branches inside the ordinary one".
//!
//! So this is that different generator: an empty world, the floating islands, and a place to stand.
//! Expressing it as one branch rather than a hundred guards is a deliberate divergence in shape, not
//! in result, and it is the shape that cannot rot - a new pass added to `build` is skipped here
//! automatically, where a hundredth guard would have to be remembered.
//!
//! # Disclosed narrowings
//!
//! * `spawnSolidifier` and `spawnShimmerPool` are the two things vanilla *adds* under this seed. The
//!   solidifier is here as the spawn platform; the shimmer pool is not, because this generator does
//!   not place shimmer.
//! * `Skyblock.Calculate` records what the finished world lacks (no altars, no dungeon, no temple,
//!   no hellstone, no fossils, no life crystals, no hellforge) so the game can adapt progression to
//!   it. Nothing here reads such a record, so it is not built; the world genuinely lacks all seven.

use terrustia_proto::{Tile, tile_solid};

use super::layout::Layout;
use crate::world::World;

/// Stone, for the spawn platform.
const STONE: u16 = 1;
/// Dirt and grass, for its surface.
const DIRT: u16 = 0;
const GRASS: u16 = 2;

/// `Skyblock.spawnSolidifier`: somewhere to stand at spawn, since there is no ground.
///
/// Vanilla's is a small solid pad under the spawn point. Without it the player falls out of the
/// world on the first tick, which makes this the one piece of ground a Skyblock world must have.
pub fn spawn_platform(world: &mut World, layout: &Layout) -> (i32, i32) {
    let x = layout.spawn_x;
    let y = layout.surface;
    for dx in -8..=8 {
        for dy in 0..4 {
            let block = if dy == 0 { GRASS } else { DIRT };
            world.set_tile(x + dx, y + dy, Tile::block(block));
        }
        // A stone lip under it, so it reads as an island rather than a floating slab of dirt.
        world.set_tile(x + dx, y + 4, Tile::block(STONE));
    }
    (x, y)
}

/// Take the ground away, leaving the islands.
///
/// Everything from a little below the island band downward is cleared: tiles, walls and liquid.
/// The band above is left alone, because that is where the islands and their clouds are.
pub fn strip_the_ground(world: &mut World, layout: &Layout) {
    let from = layout.surface - 20;
    for x in 0..world.width() {
        for y in from..world.height() {
            let t = world.tile(x, y);
            if t.is_active() || t.wall != 0 || t.liquid != 0 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
    }
}

/// Whether the finished world is one a Skyblock player would recognise: nothing but sky under the
/// islands. Used by the tests, and cheap enough to be worth having.
pub fn is_empty_below(world: &World, layout: &Layout) -> bool {
    let mut solid = 0;
    for x in (0..world.width()).step_by(7) {
        for y in (layout.surface + 60..world.height() - 20).step_by(7) {
            let t = world.tile(x, y);
            if t.is_active() && tile_solid::solid(t.block) {
                solid += 1;
            }
        }
    }
    solid == 0
}

#[cfg(test)]
mod tests {
    use super::super::rand::UnifiedRandom;
    use super::*;

    #[test]
    fn the_spawn_platform_is_solid_ground_at_spawn() {
        let mut world = World::empty(800, 600, "sky");
        let mut layout = Layout::plan(800, 600, &mut UnifiedRandom::new(2));
        layout.surface = 200;
        layout.spawn_x = 400;
        let (x, y) = spawn_platform(&mut world, &layout);
        assert_eq!((x, y), (400, 200));
        assert!(world.tile(400, 200).is_active(), "no ground at spawn");
        assert_eq!(world.tile(400, 200).block, GRASS);
        assert!(
            world.tile(408, 204).is_active(),
            "the platform is too narrow"
        );
        assert!(
            !world.tile(420, 200).is_active(),
            "the platform is too wide"
        );
    }

    #[test]
    fn an_empty_world_reads_as_empty_below() {
        let mut world = World::empty(800, 600, "sky");
        let mut layout = Layout::plan(800, 600, &mut UnifiedRandom::new(2));
        layout.surface = 200;
        assert!(is_empty_below(&world, &layout));
        // A patch rather than one tile: the check samples every seventh column and row, which is
        // enough to see ground and cheap enough to run over a whole world.
        for x in 400..420 {
            for y in 400..420 {
                world.set_tile(x, y, Tile::block(STONE));
            }
        }
        assert!(
            !is_empty_below(&world, &layout),
            "a patch of stone should be seen"
        );
    }
}
