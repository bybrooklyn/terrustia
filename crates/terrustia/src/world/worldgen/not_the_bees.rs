//! "Not the bees": the whole world turned to jungle and hive.
//!
//! Transcribed from `WorldGen.NotTheBees` (`.scratch/decompiled/Terraria/WorldGen.cs:25382-25600`),
//! the conversion pass the seed runs four times over the course of generation
//! (`:16247`, `:16775`, `:16874`, `:18008`) and once more at the end (`:22634`).
//!
//! This is the first of the eight remaining secret seeds to get real generation content. No Traps
//! World was already wired (`traps.rs`); the other seven are still detection-only.
//!
//! # What the seed does
//!
//! Everything solid that is not ore, dungeon, temple or a short exclusion list becomes hive above
//! the lava line and crispy honey below it; dirt becomes mud; every kind of grass becomes jungle
//! grass; vines become jungle vines; living wood becomes mahogany. Then a second pass walks the
//! result and turns *exposed* hive back into mud and exposed crispy honey into its own grass, so
//! the surface of the world reads as jungle rather than as a solid block of hive.
//!
//! # Seed combinations
//!
//! `NotTheBees` is 280 lines and roughly a third of them are conditions on *other* secret seeds.
//! Those seeds now have generation content, so the interactions are honoured rather than assumed
//! away:
//!
//! * **Remix** suppresses the coastal branch outright and moves the crispy-honey line to
//!   `maxTilesY - 180` rather than the midpoint (`:25394-25396`, `:25407`).
//! * **Don't Starve** and **Celebrationmk10** each force the coastal branch on for the whole world
//!   rather than only the dungeon's own half (`:25407`).
//! * **Don't Starve** additionally protects the stone-family tiles from conversion (`:25446`), so a
//!   Constant world keeps its stone where an ordinary bees world would turn it to hive.
//!
//! What is still not modelled is `skyblockWorldGen` (which cancels this pass along with everything
//! else, and is handled by `skyblock.rs`'s own branch before this runs) and `dualDungeons`, which
//! is a `SecretSeed.Variations` flag with no counterpart here.
//!
//! Also not modelled: the ocean/beach branch's dungeon recolouring (this generator does not paint
//! dungeon tiles), and the long wall-conversion tail, which in the shipped code is a chain of
//! `wall = <the same wall>` assignments and therefore a no-op for every wall it names.

use terrustia_proto::tile_solid;

use super::layout::Layout;
use super::rand::UnifiedRandom;
use crate::world::World;

/// Hive.
const HIVE: u16 = 225;
/// Crispy Honey Block, what hive becomes below the lava line.
const CRISPY_HONEY: u16 = 230;
/// Mud.
const MUD: u16 = 59;
/// Jungle Grass.
const JUNGLE_GRASS: u16 = 60;
/// Honey-side grass, the crispy honey's own surface.
const HONEY_GRASS: u16 = 70;
/// Jungle vine.
const JUNGLE_VINE: u16 = 62;
/// Living Mahogany and its leaves.
const MAHOGANY: u16 = 383;
const MAHOGANY_LEAVES: u16 = 384;
/// Sand becomes hardened honey at the beaches.
const HARDENED_HONEY: u16 = 229;

/// Tile types the conversion refuses to touch. Vanilla spells these out inline across several
/// nested conditions; gathered here with the same ids.
fn untouchable(block: u16) -> bool {
    matches!(
        block,
        // Ores are excluded wholesale by `TileID.Sets.Ore`; this generator only ever places these.
        6 | 7 | 8 | 9 | 166 | 167 | 168 | 169 | 22 | 204 | 58 | 211
        // Named exclusions from the same conditions.
        | 368 | 367 | 123 | 40 | 379 | 151 | 662 | 661 | 120 | 158 | 175 | 45 | 119
        | 57 | 76 | 75 | 229 | 230 | 407 | 404 | 10 | 203 | 25 | 137 | 141
        // Dungeon, temple, and the containers a conversion must not eat.
        | 226 | 202 | 70 | 48 | 232 | 21 | 467 | 88
        // Clouds.
        | 189 | 196 | 460 | 717 | 718 | 719
        // Wood platforms and the 63..=68 gem run.
        | 63 | 64 | 65 | 66 | 67 | 68
    )
}

/// Vanilla's grass-conversion set, narrowed to the grasses this generator places.
fn is_grass(block: u16) -> bool {
    matches!(block, 2 | 60 | 70 | 109 | 199 | 23 | 633)
}

/// Whether this tile is exposed: at least one of its eight neighbours is not active.
fn exposed(world: &World, x: i32, y: i32) -> bool {
    for (dx, dy) in [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ] {
        if !world.tile(x + dx, y + dy).is_active() {
            return true;
        }
    }
    false
}

/// Whether every one of the eight neighbours is active, which is what the pocketing pass wants.
fn enclosed(world: &World, x: i32, y: i32) -> bool {
    !exposed(world, x, y)
}

/// `WorldGen.NotTheBees`. Runs over the whole world; vanilla calls it repeatedly and so does this
/// generator, because each call converts whatever the passes since the last one laid down.
pub fn convert(
    world: &mut World,
    layout: &Layout,
    rand: &mut UnifiedRandom,
    secret: super::secret_seed::SecretSeeds,
) {
    let lava_line = layout.lava_line();
    // `num2`: below this, hive becomes crispy honey instead. Remix takes the world's own floor
    // rather than the midpoint (`:25394-25396`).
    let honey_line = if secret.remix {
        world.height() - 180
    } else {
        (lava_line + world.height() - 180) / 2
    };
    // The beach band. `layout` already decided where the oceans are, so read that rather than
    // re-deriving vanilla's `beachDistance`.
    let beach = layout.ocean_left.to.max(60);

    let bottom = world.height() - 180;
    if bottom <= 5 {
        return;
    }

    for i in 5..world.width() - 5 {
        for j in 5..bottom {
            // The ocean and beach band is left as ocean, not converted.
            // `:25407`: the coastal band is left as ocean. Remix suppresses the branch entirely;
            // Don't Starve and Celebrationmk10 force it on for the whole world rather than only
            // the dungeon's half, which this generator has no side for anyway.
            let near_coast = j < (layout.surface + layout.rock * 2) / 3 + rand.next_max(3)
                && (i < beach - 50 - rand.next_max(3)
                    || i > world.width() - beach + 50 + rand.next_max(3));
            let coastal = near_coast && !secret.remix;
            if coastal {
                continue;
            }

            let mut t = world.tile(i, j);

            // Vines first: they are not solid, so the solid test below would skip them.
            if t.block == 52 || t.block == 382 {
                t.block = JUNGLE_VINE;
                world.set_tile(i, j, t);
                continue;
            }

            if !t.is_active() || !tile_solid::solid(t.block) || untouchable(t.block) {
                continue;
            }
            // `:25446`: a Constant world keeps its stone family rather than turning it to hive.
            if secret.dont_starve
                && !secret.remix
                && matches!(t.block, 1 | 147 | 161 | 30 | 321 | 158 | 190 | 162)
            {
                continue;
            }

            if t.block == 191 || t.block == MAHOGANY {
                t.block = MAHOGANY;
            } else if t.block == 192 || t.block == MAHOGANY_LEAVES {
                t.block = MAHOGANY_LEAVES;
            } else if t.block == 224 {
                t.block = HARDENED_HONEY;
            } else if t.block == 53 {
                // Sand only converts at the beaches.
                if i < beach + rand.next_max(3) || i > world.width() - beach - rand.next_max(3) {
                    t.block = HARDENED_HONEY;
                } else {
                    continue;
                }
            } else if t.block == 397 || t.block == 396 {
                // Hardened sand and sandstone survive away from the beaches.
                if !(i <= beach - rand.next_max(3) || i >= world.width() - beach + rand.next_max(3))
                {
                    continue;
                }
                t.block = HIVE;
            } else if is_grass(t.block) {
                t.block = if j > lava_line + rand.next_range(-2, 3) + 2 {
                    HONEY_GRASS
                } else {
                    JUNGLE_GRASS
                };
            } else if t.block == 0 || t.block == MUD {
                t.block = MUD;
            } else if j > lava_line + rand.next_range(-2, 3) + 2 {
                if j <= honey_line + rand.next_max(3) {
                    t.block = CRISPY_HONEY;
                } else {
                    continue;
                }
            } else {
                t.block = HIVE;
            }
            // A converted block keeps no frames: none of the targets is frame-important, and a
            // stale frame is the defect the save round-trip test caught twice in the desert work.
            t.frame_x = -1;
            t.frame_y = -1;
            world.set_tile(i, j, t);
        }
    }

    // The crust pass: exposed hive becomes mud and exposed crispy honey becomes its grass, so the
    // world reads as jungle rather than as one solid block of hive. Each conversion also pokes
    // eight nearby enclosed tiles back to mud.
    for i in beach + 220..world.width() - beach - 220 {
        for j in 51..world.height() - 50 {
            let t = world.tile(i, j);
            if !t.is_active() {
                continue;
            }
            let surfaced = j < layout.surface || walls_clear(world, i, j);
            if t.block == HIVE && exposed(world, i, j) && surfaced {
                let mut t = t;
                t.block = JUNGLE_GRASS;
                world.set_tile(i, j, t);
                pocket(world, rand, i, j, HIVE);
            } else if t.block == CRISPY_HONEY && exposed(world, i, j) && surfaced {
                let mut t = t;
                t.block = HONEY_GRASS;
                world.set_tile(i, j, t);
                pocket(world, rand, i, j, CRISPY_HONEY);
            }
        }
    }
}

/// Vanilla's eight-neighbour "no walls around this tile" test.
fn walls_clear(world: &World, x: i32, y: i32) -> bool {
    for (dx, dy) in [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ] {
        if world.tile(x + dx, y + dy).wall != 0 {
            return false;
        }
    }
    true
}

/// Eight random pokes around a newly-surfaced tile, turning enclosed neighbours of `what` to mud.
fn pocket(world: &mut World, rand: &mut UnifiedRandom, x: i32, y: i32, what: u16) {
    for _ in 0..8 {
        let nx = x + rand.next_range(-2, 3);
        let ny = y + rand.next_range(-2, 3);
        let t = world.tile(nx, ny);
        if t.is_active() && t.block == what && enclosed(world, nx, ny) {
            let mut t = t;
            t.block = MUD;
            world.set_tile(nx, ny, t);
        }
    }
}

/// `FinishNotTheBees` (`WorldGen.cs:25663`): every pool of water in the world becomes honey.
pub fn finish(world: &mut World) {
    for x in 0..world.width() {
        for y in 0..world.height() {
            let t = world.tile(x, y);
            if t.liquid > 0 && t.liquid_kind == terrustia_proto::Liquid::Water {
                let mut t = t;
                t.liquid_kind = terrustia_proto::Liquid::Honey;
                world.set_tile(x, y, t);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::worldgen::secret_seed::SecretSeeds;
    use terrustia_proto::Tile;

    fn plain(w: i32, h: i32) -> (World, Layout) {
        let mut world = World::empty(w, h, "bees");
        let mut layout = Layout::plan(w, h, &mut UnifiedRandom::new(9));
        layout.surface = 120;
        layout.rock = 200;
        layout.underworld = h - 260;
        for x in 0..w {
            for y in 120..h {
                world.set_tile(x, y, Tile::block(if y < 200 { 0 } else { 1 }));
            }
        }
        for x in 0..w {
            world.set_tile(x, 119, Tile::block(2));
        }
        (world, layout)
    }

    #[test]
    fn stone_becomes_hive_and_dirt_becomes_mud() {
        let (mut world, layout) = plain(1200, 800);
        let mut rand = UnifiedRandom::new(11);
        convert(&mut world, &layout, &mut rand, SecretSeeds::none());

        let mut hive = 0;
        let mut mud = 0;
        for x in 500..700 {
            for y in 130..400 {
                match world.tile(x, y).block {
                    HIVE => hive += 1,
                    MUD => mud += 1,
                    _ => {}
                }
            }
        }
        assert!(hive > 1000, "stone did not become hive: {hive}");
        assert!(mud > 500, "dirt did not become mud: {mud}");
    }

    #[test]
    fn ore_survives_the_conversion() {
        let (mut world, layout) = plain(1200, 800);
        for x in 600..610 {
            world.set_tile(x, 300, Tile::block(7));
        }
        let mut rand = UnifiedRandom::new(12);
        convert(&mut world, &layout, &mut rand, SecretSeeds::none());
        assert_eq!(
            world.tile(605, 300).block,
            7,
            "copper ore must survive: the seed converts terrain, not ore"
        );
    }

    #[test]
    fn water_becomes_honey() {
        let (mut world, _layout) = plain(600, 400);
        let mut t = world.tile(300, 150);
        t.liquid = 255;
        t.liquid_kind = terrustia_proto::Liquid::Water;
        world.set_tile(300, 150, t);
        finish(&mut world);
        assert_eq!(
            world.tile(300, 150).liquid_kind,
            terrustia_proto::Liquid::Honey
        );
    }

    /// Don't Starve protects the stone family from the bees conversion (`WorldGen.cs:25446`),
    /// which is one of the interactions "get fixed boi" actually exercises.
    #[test]
    fn dont_starve_keeps_its_stone_when_the_bees_arrive() {
        let convert_with = |secret: SecretSeeds| {
            let (mut world, layout) = plain(1200, 800);
            let mut rand = UnifiedRandom::new(11);
            convert(&mut world, &layout, &mut rand, secret);
            let mut stone = 0usize;
            for x in 500..700 {
                for y in 250..400 {
                    if world.tile(x, y).block == 1 && world.tile(x, y).is_active() {
                        stone += 1;
                    }
                }
            }
            stone
        };

        let plain_bees = convert_with(SecretSeeds::none());
        let mut with_constant = SecretSeeds::none();
        with_constant.dont_starve = true;
        let bees_and_constant = convert_with(with_constant);

        assert_eq!(plain_bees, 0, "a plain bees world converts all its stone");
        assert!(
            bees_and_constant > 1000,
            "with Don't Starve the stone survives, saw {bees_and_constant}"
        );
    }

    #[test]
    fn the_conversion_is_reproducible_from_the_seed() {
        let run = || {
            let (mut world, layout) = plain(800, 500);
            let mut rand = UnifiedRandom::new(404);
            convert(&mut world, &layout, &mut rand, SecretSeeds::none());
            let mut fingerprint = Vec::new();
            for x in (100..700).step_by(5) {
                for y in (120..450).step_by(5) {
                    fingerprint.push(world.tile(x, y).block);
                }
            }
            fingerprint
        };
        assert_eq!(run(), run());
    }
}
