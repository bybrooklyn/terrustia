//! Pots.
//!
//! **Not routed through `place_object`.** `PotsGraveyardsAndBoulderPiles`
//! (`WorldGen.cs:18123`) calls `PlacePot` directly rather than the generic `PlaceTile`, and
//! `PlacePot` (`WorldGen.cs:54566-54584`) writes its own frame arithmetic: `frameX = k*18 +
//! Next(3)*36`, `frameY = (l+1)*18 + style*36`. It puts the random variant across the sheet and
//! the biome style down it, unconditionally. Block 28's `TileObjectData` entry is style-horizontal
//! with a wrap of 3, so `frame_of(style, random)` folds both into one index and lands the pair
//! somewhere else entirely. Checked this by computing both and comparing before writing a line of
//! this file: routing pots through `place_object` would have placed real pots with subtly wrong
//! frames, still 2x2 and still active, so nothing would have failed a test - it would just have
//! rendered wrong in a real client, which is exactly the kind of bug this project keeps finding by
//! measuring rather than assuming. So `PlacePot` is transcribed directly, the way `trees.rs`
//! transcribes `GrowTree`'s own hand-rolled frames.
//!
//! The original note here reached the same conclusion from a wrong fact: it said block 28's
//! `full_height` was 34 against `PlacePot`'s 36. It was 34, and it should not have been. The table
//! is generated now and reads 36, which is the row spacing `PlacePot` uses; the disagreement that
//! keeps pots on their own path is the axis the style rides on, not the row height.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::{Tile, tile_solid};

use super::layout::Layout;
use crate::world::World;

const POT: u16 = 28;

/// Which style range a pot rolls from, by what it is sitting near.
///
/// Transcribed from the cascading `if` chain in the pass body — later conditions override
/// earlier ones when more than one matches, exactly as vanilla's non-`else` chain does; the
/// underworld check runs last and so wins over everything above it.
fn style_range(ground_type: u16, wall: u16, below_underworld: bool) -> (i32, i32) {
    let mut range = (0, 4);
    if matches!(ground_type, 147 | 161 | 162) {
        range = (4, 7);
    }
    if ground_type == 60 {
        range = (7, 10);
    }
    if is_dungeon_wall(wall) || matches!(ground_type, 41 | 43 | 44 | 481 | 482 | 483) {
        range = (10, 13);
    }
    if matches!(ground_type, 23 | 25 | 22 | 163) {
        range = (16, 19);
    }
    if matches!(ground_type, 199 | 203 | 204 | 200) {
        range = (22, 25);
    }
    if ground_type == 367 {
        range = (31, 34);
    }
    if ground_type == 226 {
        range = (28, 31);
    }
    if matches!(wall, 187 | 216 | 223) {
        range = (34, 37);
    }
    if below_underworld {
        range = (13, 16);
    }
    range
}

/// `Main.wallDungeon[wall]` — every dungeon-brick wall variant. Vanilla sets exactly
/// {7, 8, 9, 94..=99} (`Main.cs:10737-10745`); the old `9..=11` both missed 7/8 and 94-99 and
/// wrongly matched GoldBrick(10)/SilverBrick(11) walls.
pub(crate) fn is_dungeon_wall(wall: u16) -> bool {
    matches!(wall, 7..=9 | 94..=99)
}

/// Try to place one pot at `(x, y)` — the bottom-left of its 2×2 footprint.
///
/// `PlacePot`'s own clearance rule: both columns clear from `y-1` to `y`, and both columns solid,
/// unsloped and not half-bricked at `y+1`.
/// `pub(crate)` rather than private: `spider_caves.rs` reuses this directly for the pot
/// `Spread.Spider` (`WorldGen.cs:3681`) scatters inside a spider cave, rather than re-deriving the
/// same frame arithmetic a second time.
pub(crate) fn place_pot(world: &mut World, x: i32, y: i32, style: i32, rng: &mut SmallRng) -> bool {
    for dx in 0..2 {
        for dy in -1..=0 {
            if world.tile(x + dx, y + dy).is_active() {
                return false;
            }
        }
        let floor = world.tile(x + dx, y + 1);
        if !floor.is_active()
            || floor.flags.has(terrustia_proto::TileFlags::HALF_BRICK)
            || floor.slope != 0
            || !tile_solid::solid(floor.block)
        {
            return false;
        }
    }
    let variant = rng.random_range(0..3) * 36;
    for dx in 0..2i32 {
        for dy in -1..=0i32 {
            // `PlacePot` itself only ever sets `active`/`type`/`frameX`/`frameY` — never `wall`
            // or `liquid`. Building from `Tile::framed` (which starts from `Tile::AIR`) instead
            // wiped whatever wall lined the room behind every pot this pass placed. Preserve it.
            let existing = world.tile(x + dx, y + dy);
            let mut tile = Tile::framed(
                POT,
                (dx * 18 + variant) as i16,
                ((dy + 1) * 18 + style * 36) as i16,
            );
            tile.wall = existing.wall;
            tile.wall_color = existing.wall_color;
            tile.liquid = existing.liquid;
            tile.liquid_kind = existing.liquid_kind;
            world.set_tile(x + dx, y + dy, tile);
        }
    }
    true
}

/// Scatter pots through the world.
///
/// Density matches vanilla: `width * height * 0.0008` attempts. Each attempt tries up to 10,000
/// random columns; for each column, it descends to the first surface it crosses and then treats
/// *every row below that* as a placement candidate, relying on `place_pot`'s own clearance check
/// to filter — most rows are solid rock and refuse instantly, but a column that passes through a
/// cave finds one. This is vanilla's actual algorithm (`WorldGen.cs:18123`'s `PotsGraveyard...`
/// pass), not a simplification of it: the outer retry counter increments once per *column*, and
/// the inner scan that does the real work is the full descent, not a single probe.
///
/// The siting search is reimplemented against `layout.surface` rather than
/// `GenVars.worldSurfaceHigh`/`worldSurfaceLow` (state a later, unbuilt pass would normally
/// refine), but the *style* table above is transcribed exactly, which is what actually makes a
/// pot look like it belongs where it landed.
pub fn scatter(world: &mut World, layout: &Layout, rng: &mut SmallRng) -> usize {
    let attempts = ((layout.width as i64 * world.height() as i64) as f64 * 0.0008) as usize;
    let mut placed = 0;
    let surface = layout.surface.max(1);
    let underworld_start = layout.underworld;
    let bottom = (world.height() - 20).max(surface + 1);

    for _ in 0..attempts {
        let mut done = false;
        let mut column_tries = 0;
        while !done && column_tries < 10_000 {
            column_tries += 1;
            let x = rng.random_range(20..(layout.width - 20).max(21));
            let mut y = rng.random_range(surface..bottom);

            // Descend to the first solid tile this column crosses.
            while y < bottom && !world.tile(x, y).is_active() {
                y += 1;
            }
            if y >= bottom || world.tile(x, y - 1).liquid > 0 {
                continue;
            }

            for candidate_y in (y + 1)..bottom {
                let below = world.tile(x, candidate_y + 1);
                if !below.is_active() {
                    continue;
                }
                let (lo, hi) = style_range(
                    below.block,
                    world.tile(x, candidate_y).wall,
                    candidate_y > underworld_start,
                );
                let style = rng.random_range(lo..hi);
                if place_pot(world, x, candidate_y, style, rng) {
                    placed += 1;
                    done = true;
                    break;
                }
            }
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    /// Pins the dungeon-wall set to vanilla's `Main.wallDungeon` = {7, 8, 9, 94..=99}. The old
    /// `9..=11` matched Gold/Silver brick walls (10/11 — not dungeon) and missed 7, 8 and 94-99.
    #[test]
    fn the_dungeon_wall_set_is_the_vanilla_walldungeon_set() {
        for w in [7, 8, 9, 94, 95, 96, 97, 98, 99] {
            assert!(is_dungeon_wall(w), "wall {w} is a dungeon wall in vanilla");
        }
        for w in [0, 1, 6, 10, 11, 12, 93, 100] {
            assert!(!is_dungeon_wall(w), "wall {w} is not a dungeon wall");
        }
    }

    /// A roof, an open room beneath it, and a floor beneath that — the shape vanilla's own
    /// algorithm actually looks for: it crosses the *first* solid surface it meets while
    /// descending (the roof), then keeps scanning every row below that as a candidate, which
    /// only succeeds once it reaches a row with an empty pair above solid ground (the bottom of
    /// the room, resting on the floor).
    fn cave() -> World {
        let mut world = World::empty(200, 200, "pots");
        for x in 0..200 {
            for y in 50..90 {
                world.set_tile(x, y, Tile::block(1)); // roof
            }
            for y in 110..200 {
                world.set_tile(x, y, Tile::block(1)); // floor and below
            }
            // y in 90..110 is left as air: the room.
        }
        world
    }

    #[test]
    fn a_pot_frames_match_placepots_own_arithmetic() {
        let mut world = cave();
        let mut rng = SmallRng::seed_from_u64(1);
        assert!(place_pot(&mut world, 60, 109, 2, &mut rng));
        // frameY for the bottom row must carry style*36 in its low bits regardless of the random
        // column offset, and the two columns must be 18px apart in X.
        let bl = world.tile(60, 109);
        let br = world.tile(61, 109);
        assert_eq!(bl.frame_y, 18 + 2 * 36);
        assert_eq!(br.frame_y, 18 + 2 * 36);
        assert_eq!(br.frame_x - bl.frame_x, 18);
        assert_ne!(bl.frame_x, -1);
    }

    /// `PlacePot` only ever sets `active`/`type`/`frameX`/`frameY` — never `wall` or `liquid`.
    /// Fails on the pre-fix code (`after.wall == 0`), which built the new tile from `Tile::framed`
    /// (starting from `Tile::AIR`) and so erased whatever wall was lining the room.
    #[test]
    fn a_placed_pot_keeps_the_wall_already_behind_it() {
        let mut world = cave();
        let mut seeded = world.tile(60, 109);
        seeded.wall = 9;
        seeded.wall_color = 4;
        world.set_tile(60, 109, seeded);
        let mut rng = SmallRng::seed_from_u64(1);
        assert!(place_pot(&mut world, 60, 109, 2, &mut rng));
        let after = world.tile(60, 109);
        assert_eq!(after.block, POT, "the pot should still have been placed");
        assert_eq!(
            after.wall, 9,
            "placing a pot must not erase the wall behind it"
        );
        assert_eq!(after.wall_color, 4, "wall_color must survive too");
    }

    #[test]
    fn style_ranges_match_the_transcribed_table() {
        assert_eq!(style_range(2, 0, false), (0, 4), "default ground");
        assert_eq!(style_range(60, 0, false), (7, 10), "jungle");
        assert_eq!(style_range(199, 0, false), (22, 25), "crimson");
        assert_eq!(
            style_range(367, 0, true),
            (13, 16),
            "underworld overrides marble, since it is checked last"
        );
    }

    #[test]
    fn pots_scatter_through_a_generated_cave() {
        let mut world = cave();
        let layout_rng_seed = 4;
        let mut rand = super::super::rand::UnifiedRandom::new(layout_rng_seed);
        let layout = Layout::plan(200, 200, &mut rand);
        let mut rng = SmallRng::seed_from_u64(9);
        let placed = scatter(&mut world, &layout, &mut rng);
        assert!(
            placed > 0,
            "a hollowed cave with floor should take some pots"
        );
    }
}
