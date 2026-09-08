//! "Celebrationmk10": the tenth-anniversary world, painted top to bottom.
//!
//! Transcribed from `WorldGen.FinishTenthAnniversaryWorld`
//! (`.scratch/decompiled/Terraria/WorldGen.cs:24507-24551`) and the paint helpers it calls -
//! `PaintTheDungeon`, `PaintTheTemple`, `PaintTheSand`, `PaintTheClouds`, `PaintTheLivingTrees`,
//! `PaintThePyramids`, `PaintTheTrees` and `PaintTheMushrooms`.
//!
//! The fourth of the nine secret seeds to get real generation content.
//!
//! # What the seed does at generation time
//!
//! It is a party seed, and generation's share of that is paint. Every landmark in the world gets a
//! colour: the dungeon 24, the living trees 12, the temple 10 with wall 5, the clouds 12, the sand
//! 7, the pyramids 12. Boulders become party-coloured ones one time in four. The colours are fixed
//! constants in vanilla, not rolls, so a Celebrationmk10 world looks the same shade every time.
//!
//! # Scoped to this seed alone
//!
//! `FinishTenthAnniversaryWorld` opens with a condition on four other seeds and skips most of
//! itself under `remixWorldGen`, `getGoodWorldGen` or `drunkWorldGen`. All are false here, which is
//! the plain `celebrationmk10` path.
//!
//! # Disclosed narrowings
//!
//! * `ConvertSkyIslands(2, growTrees: true)` repaints and replants the floating islands; it needs
//!   the island-conversion helper, which has no counterpart here. Not attempted.
//! * `ImproveAllChestContents` upgrades every chest's loot table. That is a loot change rather than
//!   a terrain one, and it belongs with the drop tables rather than here. Not attempted.
//! * `PaintTheTrees`/`PaintTheMushrooms` walk vanilla's own tree and mushroom bookkeeping; the tree
//!   painting here is applied to the trunk tiles this generator actually places.

use super::layout::Layout;
use super::rand::UnifiedRandom;
use crate::world::World;

/// Vanilla's fixed paint colours, one per landmark.
const DUNGEON: u8 = 24;
const LIVING_TREE: u8 = 12;
const TEMPLE_TILE: u8 = 10;
const TEMPLE_WALL_COLOUR: u8 = 5;
const CLOUD: u8 = 12;
const SAND: u8 = 7;
const PYRAMID: u8 = 12;

/// Boulder, and the party boulder it becomes.
const BOULDER: u16 = 138;
const PARTY_BOULDER: u16 = 665;

fn is_dungeon_brick(block: u16) -> bool {
    matches!(block, 41 | 43 | 44 | 481 | 482 | 483)
}

fn is_dungeon_wall(wall: u16) -> bool {
    matches!(wall, 7 | 8 | 9 | 94 | 95 | 96)
}

/// `FinishTenthAnniversaryWorld`. One sweep, painting every landmark, then the boulders.
pub fn finish(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) {
    for x in 10..world.width() - 10 {
        for y in 10..world.height() - 10 {
            let mut t = world.tile(x, y);
            let mut changed = false;

            if t.is_active() {
                // `PaintTheDungeon(24, 24)`.
                if is_dungeon_brick(t.block) {
                    t.color = DUNGEON;
                    changed = true;
                }
                // `PaintTheTemple(10, 5)`: the brick, the moss on it, and its armed traps.
                let temple = t.block == 226
                    || (t.block == 61 && world.tile(x, y + 1).block == 226)
                    || (t.block == 137 && (1..=4).contains(&(t.frame_y / 18)));
                if temple {
                    t.color = TEMPLE_TILE;
                    changed = true;
                }
                // `PaintTheSand(7, 7)`, which also catches the cacti and pots standing on it.
                if matches!(t.block, 53 | 396 | 397) {
                    t.color = SAND;
                    changed = true;
                    if y > layout.surface {
                        for dy in [-1, -2] {
                            let above = world.tile(x, y + dy);
                            if matches!(above.block, 165 | 185 | 186 | 187) {
                                let mut a = above;
                                a.color = SAND;
                                world.set_tile(x, y + dy, a);
                            }
                        }
                    }
                }
                // `PaintTheClouds(12, 12)`.
                if matches!(t.block, 189 | 196 | 460 | 717 | 718 | 719) {
                    t.color = CLOUD;
                    changed = true;
                }
                // `PaintTheLivingTrees(12, 12)`: living wood and its leaves.
                if matches!(t.block, 191 | 192) {
                    t.color = LIVING_TREE;
                    changed = true;
                }
                // `PaintThePyramids(12, 12)`: sandstone brick and its wall.
                if t.block == 151 {
                    t.color = PYRAMID;
                    changed = true;
                }
            }

            if is_dungeon_wall(t.wall) {
                t.wall_color = DUNGEON;
                changed = true;
            }
            if t.wall == 87 {
                t.wall_color = TEMPLE_WALL_COLOUR;
                changed = true;
            }
            if t.wall == 34 {
                t.wall_color = PYRAMID;
                changed = true;
            }

            if changed {
                world.set_tile(x, y, t);
            }
        }
    }

    // One boulder in four becomes a party boulder. Vanilla keys on the top-left cell of the 2x2.
    for x in 50..world.width() - 50 {
        for y in 50..world.height() - 50 {
            let t = world.tile(x, y);
            if rand.next_max(4) == 0
                && t.is_active()
                && t.block == BOULDER
                && t.frame_x == 0
                && t.frame_y == 0
            {
                for (dx, dy) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    let mut c = world.tile(x + dx, y + dy);
                    c.block = PARTY_BOULDER;
                    world.set_tile(x + dx, y + dy, c);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrustia_proto::Tile;

    fn world_with(w: i32, h: i32) -> (World, Layout) {
        let mut world = World::empty(w, h, "party");
        let mut layout = Layout::plan(w, h, &mut UnifiedRandom::new(8));
        layout.surface = 200;
        layout.rock = 260;
        layout.underworld = h - 100;
        for x in 0..w {
            for y in 150..h {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        (world, layout)
    }

    #[test]
    fn every_landmark_gets_its_own_colour() {
        let (mut world, layout) = world_with(600, 500);
        world.set_tile(100, 300, Tile::block(41)); // dungeon brick
        world.set_tile(120, 300, Tile::block(226)); // Lihzahrd
        world.set_tile(140, 300, Tile::block(53)); // sand
        world.set_tile(160, 100, Tile::block(189)); // cloud
        world.set_tile(180, 300, Tile::block(191)); // living wood
        world.set_tile(200, 300, Tile::block(151)); // sandstone brick

        let mut rand = UnifiedRandom::new(10);
        finish(&mut world, &layout, &mut rand);

        assert_eq!(world.tile(100, 300).color, DUNGEON);
        assert_eq!(world.tile(120, 300).color, TEMPLE_TILE);
        assert_eq!(world.tile(140, 300).color, SAND);
        assert_eq!(world.tile(160, 100).color, CLOUD);
        assert_eq!(world.tile(180, 300).color, LIVING_TREE);
        assert_eq!(world.tile(200, 300).color, PYRAMID);
        // Plain stone is left alone.
        assert_eq!(world.tile(300, 300).color, 0);
    }

    #[test]
    fn walls_get_painted_too() {
        let (mut world, layout) = world_with(600, 500);
        for (x, wall) in [(100, 7u16), (120, 87), (140, 34)] {
            let mut t = world.tile(x, 300);
            t.wall = wall;
            world.set_tile(x, 300, t);
        }
        let mut rand = UnifiedRandom::new(11);
        finish(&mut world, &layout, &mut rand);
        assert_eq!(world.tile(100, 300).wall_color, DUNGEON);
        assert_eq!(world.tile(120, 300).wall_color, TEMPLE_WALL_COLOUR);
        assert_eq!(world.tile(140, 300).wall_color, PYRAMID);
    }

    /// Some boulders become party boulders, and the whole 2x2 converts together.
    #[test]
    fn boulders_sometimes_join_the_party() {
        let (mut world, layout) = world_with(600, 500);
        for i in 0..40 {
            let x = 100 + i * 4;
            for (dx, dy) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let t = Tile::framed(BOULDER, (dx as i16) * 18, (dy as i16) * 18);
                world.set_tile(x + dx, 300 + dy, t);
            }
        }
        let mut rand = UnifiedRandom::new(12);
        finish(&mut world, &layout, &mut rand);

        let mut converted = 0;
        for i in 0..40 {
            let x = 100 + i * 4;
            if world.tile(x, 300).block == PARTY_BOULDER {
                converted += 1;
                for (dx, dy) in [(0, 1), (1, 0), (1, 1)] {
                    assert_eq!(
                        world.tile(x + dx, 300 + dy).block,
                        PARTY_BOULDER,
                        "the whole 2x2 must convert together"
                    );
                }
            }
        }
        assert!(converted > 0, "no boulder joined the party");
        assert!(
            converted < 40,
            "every boulder converted; the 1-in-4 roll is not happening"
        );
    }

    #[test]
    fn the_pass_is_reproducible_from_the_seed() {
        let run = || {
            let (mut world, layout) = world_with(400, 400);
            for x in 100..140 {
                world.set_tile(x, 300, Tile::block(41));
            }
            let mut rand = UnifiedRandom::new(444);
            finish(&mut world, &layout, &mut rand);
            let mut fingerprint = Vec::new();
            for x in (0..400).step_by(5) {
                for y in (100..400).step_by(5) {
                    let t = world.tile(x, y);
                    fingerprint.push((t.block, t.color));
                }
            }
            fingerprint
        };
        assert_eq!(run(), run());
    }
}
