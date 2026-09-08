//! "For the Worthy": a harder world, and one that looks it.
//!
//! Transcribed from `WorldGen.FinishGetGoodWorld` (`.scratch/decompiled/Terraria/WorldGen.cs:26074-26230`)
//! and the scattered `getGoodWorldGen` multipliers it ships with (`:9777`, `:18880`, `:21086`).
//!
//! The third of the nine secret seeds to get real generation content, after No Traps World and
//! [`super::not_the_bees`] and [`super::dont_starve`].
//!
//! # What the seed does to a world
//!
//! It is mostly a difficulty seed, and most of that lives outside world generation. What generation
//! itself does is:
//!
//! * **Paints the dungeon.** One colour is chosen from the dungeon's own brick type - blue, green or
//!   pink each pick from their own range - and every dungeon tile, cracked brick and dungeon wall in
//!   the world is painted with it. The temple gets its own wall colour, and Lihzahrd brick, the
//!   jungle grass sitting on it and the temple's armed dart traps are all painted red.
//! * **Corrupts the sky.** Every grass tile *above* the cloud line - which is to say the grass on
//!   the floating islands, not the surface - becomes Corrupt or Crimson grass, and the plants and
//!   flowers growing out of it are killed. Read quickly, vanilla's `n < num` looks like it corrupts
//!   the surface; `num` is the lowest row holding a cloud tile, so the rows it selects are the ones
//!   over the sky islands. A test written against the wrong reading is what caught this.
//! * **Turns some obsidian to lava**, one tile in fifteen.
//! * **Multiplies counts by 1.5**: ore veins, surface pots, and explosive traps.
//!
//! # Seed combinations
//!
//! All three of vanilla's are honoured: `drunkWorldGen` or `notTheBees` suppress the wall painting,
//! `tenthAnniversaryWorldGen` or `notTheBees` suppress the sky corruption, and `remixWorldGen`
//! suppresses the obsidian-to-lava conversion.
//!
//! # Disclosed narrowings
//!
//! * Tile *paint* is a real tile field this server round-trips, so the painting is applied for
//!   real; but this generator paints nothing else, so `for the worthy` is the only thing that ever
//!   sets a colour byte on a generated world.
//! * The dungeon-brick colour is read from whichever dungeon brick the scan finds first, exactly as
//!   vanilla does, including vanilla's own early `break` that stops at the first column holding one.

use terrustia_proto::{Liquid, TileFlags};

use super::layout::Layout;
use super::rand::UnifiedRandom;
use crate::world::World;

/// The three dungeon brick types, which decide the paint colour.
const BLUE_BRICK: u16 = 41;
const GREEN_BRICK: u16 = 43;
const PINK_BRICK: u16 = 44;
/// Cracked dungeon brick.
const CRACKED_BRICK: u16 = 481;
/// Lihzahrd brick and the temple wall.
const LIHZAHRD: u16 = 226;
const TEMPLE_WALL: u16 = 87;
/// Dart trap.
const DART_TRAP: u16 = 137;
/// Obsidian.
const OBSIDIAN: u16 = 57;
/// Ordinary grass, and the two evils it becomes.
const GRASS: u16 = 2;
const CORRUPT_GRASS: u16 = 23;
const CRIMSON_GRASS: u16 = 199;
/// The plants that die with the grass under them.
const PLANT: u16 = 3;
const JUNGLE_PLANT: u16 = 73;

fn is_dungeon_brick(block: u16) -> bool {
    matches!(block, BLUE_BRICK | GREEN_BRICK | PINK_BRICK)
}

fn is_dungeon_wall(wall: u16) -> bool {
    matches!(wall, 7 | 8 | 9 | 94 | 95 | 96)
}

/// `FinishGetGoodWorld`. Runs once, last, over the whole world.
pub fn finish(
    world: &mut World,
    layout: &Layout,
    rand: &mut UnifiedRandom,
    secret: super::secret_seed::SecretSeeds,
) {
    // The cloud line: the lowest row above the surface that still holds a cloud tile. Vanilla
    // scans top-down and keeps the last row it saw one on.
    let mut cloud_line = 0;
    for y in 20..layout.surface {
        for x in 20..world.width() - 20 {
            let t = world.tile(x, y);
            if t.is_active() && matches!(t.block, 189 | 196 | 460 | 717 | 718 | 719) {
                cloud_line = y;
                break;
            }
        }
    }

    // The dungeon's own colour, from whichever brick the scan meets first.
    let mut colour = rand.next_range(13, 25) as u8;
    'columns: for x in 0..world.width() {
        for y in 0..world.height() {
            let t = world.tile(x, y);
            if !t.is_active() || !is_dungeon_brick(t.block) {
                continue;
            }
            colour = match t.block {
                PINK_BRICK => {
                    if rand.next_max(2) == 0 {
                        rand.next_range(23, 25) as u8
                    } else {
                        rand.next_range(13, 15) as u8
                    }
                }
                GREEN_BRICK => rand.next_range(15, 19) as u8,
                _ => rand.next_range(19, 23) as u8,
            };
            break 'columns;
        }
    }

    let crimson = world.crimson;
    for x in 10..world.width() - 10 {
        for y in 10..world.height() - 10 {
            let mut t = world.tile(x, y);
            let mut changed = false;

            if t.is_active() && (is_dungeon_brick(t.block) || t.block == CRACKED_BRICK) {
                t.color = colour;
                changed = true;
            }
            // Drunk World and Not the Bees both suppress the wall painting.
            if is_dungeon_wall(t.wall) && !secret.drunk && !secret.not_the_bees {
                t.wall_color = colour;
                changed = true;
            }

            // The temple, its grass skin and its armed traps go red.
            if t.is_active() {
                let temple = t.block == LIHZAHRD
                    || (t.block == 60 && world.tile(x, y + 1).block == LIHZAHRD)
                    || (t.block == DART_TRAP && (1..=4).contains(&(t.frame_y / 18)));
                if temple {
                    t.color = 17;
                    changed = true;
                }
            }
            if t.wall == TEMPLE_WALL {
                t.wall_color = 25;
                changed = true;
            }

            // Remix leaves the obsidian alone.
            if !secret.remix && t.is_active() && t.block == OBSIDIAN && rand.next_max(15) == 0 {
                if world.tile(x, y - 1).block == OBSIDIAN {
                    t.flags = TileFlags(t.flags.0 & !TileFlags::ACTIVE);
                    t.block = 0;
                    t.frame_x = -1;
                    t.frame_y = -1;
                }
                t.liquid = u8::MAX;
                t.liquid_kind = Liquid::Lava;
                changed = true;
            }

            // The evil takes the sky islands: rows above the cloud line, not the surface.
            // Celebrationmk10 and Not the Bees both spare the sky islands.
            if !secret.tenth_anniversary
                && !secret.not_the_bees
                && t.is_active()
                && y < cloud_line
                && t.block == GRASS
            {
                t.block = if crimson {
                    CRIMSON_GRASS
                } else {
                    CORRUPT_GRASS
                };
                t.frame_x = -1;
                t.frame_y = -1;
                changed = true;
                let above = world.tile(x, y - 1);
                if above.block == PLANT || above.block == JUNGLE_PLANT {
                    let mut a = above;
                    a.flags = TileFlags(a.flags.0 & !TileFlags::ACTIVE);
                    a.block = 0;
                    a.frame_x = -1;
                    a.frame_y = -1;
                    world.set_tile(x, y - 1, a);
                }
            }

            if changed {
                world.set_tile(x, y, t);
            }
        }
    }
}

/// The seed's own count multiplier: ore veins, surface pots and explosive traps all get half again
/// as many (`:9777`, `:18880`, `:21086`).
pub fn count_scale(active: bool) -> f64 {
    if active { 1.5 } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::worldgen::secret_seed::SecretSeeds;
    use terrustia_proto::Tile;

    fn world_with(w: i32, h: i32) -> (World, Layout) {
        let mut world = World::empty(w, h, "worthy");
        let mut layout = Layout::plan(w, h, &mut UnifiedRandom::new(6));
        layout.surface = 200;
        layout.rock = 260;
        layout.underworld = h - 100;
        // Ground, with a grass skin.
        for x in 0..w {
            for y in 150..h {
                world.set_tile(x, y, Tile::block(1));
            }
            world.set_tile(x, 149, Tile::block(GRASS));
        }
        // A cloud, and an island of grass above it - which is what the seed corrupts.
        for x in 100..140 {
            world.set_tile(x, 120, Tile::block(189));
        }
        for x in 100..140 {
            world.set_tile(x, 90, Tile::block(GRASS));
        }
        (world, layout)
    }

    #[test]
    fn sky_island_grass_becomes_the_worlds_evil() {
        let (mut world, layout) = world_with(600, 500);
        world.crimson = false;
        let mut rand = UnifiedRandom::new(19);
        finish(&mut world, &layout, &mut rand, SecretSeeds::none());
        assert_eq!(
            world.tile(120, 90).block,
            CORRUPT_GRASS,
            "sky-island grass should have turned to corruption"
        );
        assert_eq!(
            world.tile(300, 149).block,
            GRASS,
            "the surface is below the cloud line and must be left alone"
        );
    }

    #[test]
    fn a_crimson_world_gets_crimson_grass_instead() {
        let (mut world, layout) = world_with(600, 500);
        world.crimson = true;
        let mut rand = UnifiedRandom::new(19);
        finish(&mut world, &layout, &mut rand, SecretSeeds::none());
        assert_eq!(world.tile(120, 90).block, CRIMSON_GRASS);
    }

    #[test]
    fn dungeon_bricks_are_painted_and_the_paint_is_uniform() {
        let (mut world, layout) = world_with(600, 500);
        for x in 200..240 {
            for y in 300..340 {
                world.set_tile(x, y, Tile::block(BLUE_BRICK));
            }
        }
        let mut rand = UnifiedRandom::new(21);
        finish(&mut world, &layout, &mut rand, SecretSeeds::none());
        let first = world.tile(210, 310).color;
        assert!(first > 0, "the dungeon was not painted");
        assert_eq!(
            world.tile(235, 335).color,
            first,
            "the whole dungeon must take one colour, not a colour each"
        );
    }

    #[test]
    fn the_temple_and_its_traps_go_red() {
        let (mut world, layout) = world_with(600, 500);
        world.set_tile(300, 300, Tile::block(LIHZAHRD));
        let mut t = world.tile(301, 300);
        t.wall = TEMPLE_WALL;
        world.set_tile(301, 300, t);
        let mut rand = UnifiedRandom::new(22);
        finish(&mut world, &layout, &mut rand, SecretSeeds::none());
        assert_eq!(
            world.tile(300, 300).color,
            17,
            "Lihzahrd brick should be red"
        );
        assert_eq!(world.tile(301, 300).wall_color, 25, "the temple wall too");
    }

    #[test]
    fn the_pass_is_reproducible_from_the_seed() {
        let run = || {
            let (mut world, layout) = world_with(400, 400);
            let mut rand = UnifiedRandom::new(333);
            finish(&mut world, &layout, &mut rand, SecretSeeds::none());
            let mut fingerprint = Vec::new();
            for x in (0..400).step_by(5) {
                for y in (100..400).step_by(5) {
                    let t = world.tile(x, y);
                    fingerprint.push((t.block, t.color, t.wall_color));
                }
            }
            fingerprint
        };
        assert_eq!(run(), run());
    }
}
