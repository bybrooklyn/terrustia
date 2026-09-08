//! The Enchanted Sword shrine.
//!
//! Transcribed from `EnchantedSwordBiome` (`.scratch/decompiled/Terraria.GameContent.Biomes/
//! EnchantedSwordBiome.cs`, all 113 lines) and its driving loop in `WorldGen.cs:21147-21196`, with
//! the tuning values read out of vanilla's own shipped config rather than guessed
//! (`Terraria.GameContent.WorldBuilding.Configuration.json`: `ChanceOfEntrance` 0.3333333,
//! `ChanceOfRealSword` 1.0, `SwordShrineAttempts` 1-2 scaled with world width,
//! `SwordShrinePlacementChance` 0.5).
//!
//! This is the sixth of the fifteen `MicroBiome` classes to land and the first built on
//! [`super::genpipe`], the shape/modifier/action pipeline. `micro_biomes.rs` shipped its six as
//! plain circles because that pipeline did not exist; this one is the real shape, blotched edge
//! and all.
//!
//! # Two findings from reading it, neither of them a transcription choice
//!
//! **The fake sword is unreachable in 1.4.5.8.** `_chanceOfRealSword` is `1.0` in the shipped
//! config and the test is `NextDouble() <= _chanceOfRealSword`, which `UnifiedRandom` can never
//! fail: every shrine in a normal world holds the real Enchanted Sword (tile 187 style 17), and
//! the decorative 186/15 branch is dead. Both branches are transcribed anyway, because the draw
//! happens either way and dropping it would shift every later draw in the world.
//!
//! **The placement roll is inverted.** `WorldGen.cs:21168` places the shrine when
//! `!(genRand.NextDouble() < num13)` - that is, when the 0.5 roll *fails*. Read quickly it looks
//! like a 50% chance to place; it is, but only because 0.5 is symmetric. Kept as vanilla writes it
//! so a future config change behaves the way vanilla would.
//!
//! # Disclosed narrowings
//!
//! * The `errorWorld` ("get fixed boi") and `dualDungeons` branches are skipped: both are secret
//!   seeds deferred wholesale (`TODO.md`, v0.0.2), and `DungeonUtils.IntersectsAnyPotentialDungeonBounds`
//!   has no counterpart here.
//! * `tenthAnniversaryWorldGen` is skipped for the same reason.
//! * Vanilla clears tiles 21 and 467 out of `GeneralPlacementTiles` before its two `CanPlace`
//!   checks; `structure_map::general_placement_tile` already excludes both, so the default
//!   predicate is the modified one and `can_place` is called directly.

use super::genpipe::{Action, Ctx, Link, Shape, chain, gen_shape};
use super::layout::Layout;
use super::rand::UnifiedRandom;
use super::structure_map::{Rect, StructureMap};
use crate::world::World;
use terrustia_proto::{Liquid, tile_solid};

/// `ChanceOfEntrance` from vanilla's config.
const CHANCE_OF_ENTRANCE: f64 = 0.3333333;
/// `ChanceOfRealSword`. See the module doc: this is 1.0, so the real sword always wins.
const CHANCE_OF_REAL_SWORD: f64 = 1.0;
/// `SwordShrinePlacementChance`.
const PLACEMENT_CHANCE: f64 = 0.5;
/// The minimum depth a shrine may sit at, `num2` in vanilla.
const MIN_DEPTH: i32 = 55;

/// `WorldGenRange.ScaleValue` for `ScaleWith: WorldWidth` (`WorldGenRange.cs:43-57`).
fn scaled_by_width(value: i32, world_width: i32) -> i32 {
    (f64::from(world_width) / 4200.0 * f64::from(value)) as i32
}

/// One shrine at `origin`. Returns whether it placed.
pub fn place(
    world: &mut World,
    structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
    origin: (i32, i32),
) -> bool {
    // A shrine needs solid ground around it: at least 1250 of the 2500 tiles in a 50x50 box must
    // be dirt or stone.
    {
        let mut ctx = Ctx::new(world, rand);
        ctx.scan_reset();
        let mut c = chain([Link::new(Action::TileScanner(vec![0, 1]))]);
        gen_shape(
            &mut ctx,
            (origin.0 - 25, origin.1 - 25),
            &Shape::Rectangle {
                left: 0,
                top: 0,
                width: 50,
                height: 50,
            },
            &mut c,
        );
        if ctx.scan_count(0) + ctx.scan_count(1) < 1250 {
            return false;
        }
    }

    if origin.1 <= MIN_DEPTH {
        return false;
    }
    let reach = origin.1 - MIN_DEPTH;
    let column = reach.min(50);

    // Search upward for the first tile with no solid ground in the `column`-tall shaft below it:
    // that is where the entrance would break through. `Searches.Up` + `IsSolid().AreaOr(1, n).Not()`.
    let mut result_y = None;
    for i in 0..reach {
        let y = origin.1 - i;
        let any_solid = (y..y + column).any(|j| {
            let t = world.tile(origin.0, j);
            t.is_active() && tile_solid::solid(t.block)
        });
        if !any_solid {
            result_y = Some(y);
            break;
        }
    }
    let Some(mut result_y) = result_y else {
        return false;
    };
    if result_y <= MIN_DEPTH {
        return false;
    }

    // Sand anywhere in the shaft means the ceiling would collapse into it.
    let found_sand = (0..origin.1 - result_y).any(|i| {
        let t = world.tile(origin.0, origin.1 - i);
        t.is_active() && t.block == 53
    });
    if found_sand {
        return false;
    }

    result_y += 50;

    let cave = (origin.0, origin.1 + 20);
    let mound = (origin.0, origin.1 + 30);
    let scale = 0.8 + rand.next_double() * 0.5;

    let hall = Rect::new(
        cave.0 - (20.0 * scale) as i32,
        cave.1 - 20,
        (40.0 * scale) as i32,
        40,
    );
    if !structures.can_place(world, hall, 0) {
        return false;
    }
    let shaft = Rect::new(origin.0, result_y + 10, 1, origin.1 - result_y - 9);
    if !structures.can_place(world, shaft, 2) {
        return false;
    }

    let mut ctx = Ctx::new(world, rand);
    let hollow = ctx.slot();
    let hill = ctx.slot();

    // The cave itself: a blotched dome, cleared out.
    let mut c = chain([
        Link::new(Action::Blotches {
            min_x: 2,
            min_y: 2,
            max_x: 2,
            max_y: 2,
            chance: 0.4,
        }),
        Link::new(Action::ClearTile).out(hollow),
    ]);
    gen_shape(
        &mut ctx,
        cave,
        &Shape::Slime {
            radius: 20,
            x_scale: scale,
            y_scale: 1.0,
        },
        &mut c,
    );

    // A dirt mound on the floor for the sword to stand in.
    let mut c = chain([
        Link::new(Action::Blotches {
            min_x: 2,
            min_y: 1,
            max_x: 2,
            max_y: 1,
            chance: 0.8,
        }),
        Link::new(Action::SetTile(0)),
        Link::new(Action::SetFrames).out(hill),
    ]);
    gen_shape(
        &mut ctx,
        mound,
        &Shape::Mound {
            half_width: 14,
            height: 14,
        },
        &mut c,
    );

    // The mound is not part of the hollow.
    let hill_shape = ctx.take(hill);
    let mut hollow_shape = ctx.take(hollow);
    hollow_shape.subtract_from(&hill_shape, cave, mound);

    // Line the cave with grass, flood the lower half, then wall it and hang vines.
    let mut c = chain([Link::new(Action::SetTile(2)), Link::new(Action::SetFrames)]);
    gen_shape(
        &mut ctx,
        cave,
        &Shape::InnerOutline(hollow_shape.clone()),
        &mut c,
    );

    let mut c = chain([
        Link::new(Action::RectangleMask {
            x_min: -40,
            x_max: 40,
            y_min: 0,
            y_max: 40,
        }),
        Link::new(Action::IsEmpty),
        Link::new(Action::SetLiquid {
            kind: Liquid::Water,
            value: 255,
        }),
    ]);
    gen_shape(&mut ctx, cave, &Shape::All(hollow_shape.clone()), &mut c);

    let mut c = chain([
        Link::new(Action::PlaceWall(68)),
        Link::new(Action::OnlyTiles(vec![2])),
        Link::new(Action::Offset { x: 0, y: 1 }),
        Link::new(Action::Vines {
            min: 3,
            max: 5,
            id: 382,
        }),
    ]);
    gen_shape(&mut ctx, cave, &Shape::All(hollow_shape), &mut c);

    // A shaft up to the surface, sometimes, with stone bricks where it cuts through dungeon walls.
    if ctx.rand.next_double() <= CHANCE_OF_ENTRANCE {
        let dug = ctx.slot();
        let mut c = chain([
            Link::new(Action::Blotches {
                min_x: 2,
                min_y: 2,
                max_x: 2,
                max_y: 2,
                chance: 0.2,
            }),
            Link::new(Action::SkipTiles(vec![191, 192])),
            Link::new(Action::ClearTile).out(dug),
            Link::new(Action::Expand { x: 1, y: 1 }),
            Link::new(Action::OnlyTiles(vec![53])),
            Link::new(Action::SetTile(397)).out(dug),
        ]);
        gen_shape(
            &mut ctx,
            (origin.0, result_y + 10),
            &Shape::Rectangle {
                left: 0,
                top: 0,
                width: 1,
                height: origin.1 - result_y - 9,
            },
            &mut c,
        );
        let _ = ctx.take(dug);
    }

    // The sword. See the module doc: the second branch is unreachable in 1.4.5.8.
    if ctx.rand.next_double() <= CHANCE_OF_REAL_SWORD {
        super::place_object::place_object(ctx.world, mound.0, mound.1 - 15, 187, 17, -1);
    } else {
        super::place_object::place_object(ctx.world, mound.0, mound.1 - 15, 186, 15, -1);
    }

    // Grass on top of the mound.
    let mut c = chain([
        Link::new(Action::Offset { x: 0, y: -1 }),
        Link::new(Action::OnlyTiles(vec![2])),
        Link::new(Action::Offset { x: 0, y: -1 }),
        Link::new(Action::Grass),
    ]);
    gen_shape(&mut ctx, mound, &Shape::All(hill_shape), &mut c);

    structures.add_protected_structure(hall, 10);
    true
}

/// The driving loop, `WorldGen.cs:21147-21196`. Returns how many shrines were placed.
pub fn scatter(
    world: &mut World,
    layout: &Layout,
    structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
) -> usize {
    // `SwordShrineAttempts`: 1-2, scaled with world width.
    let min = scaled_by_width(1, layout.width);
    let max = scaled_by_width(2, layout.width);
    if max < min || layout.width < 200 {
        return 0;
    }
    let attempts = rand.next_range(min, max + 1);
    let mut placed = 0;
    for _ in 0..attempts {
        // Vanilla places when the roll *fails*; see the module doc.
        if rand.next_double() < PLACEMENT_CHANCE {
            continue;
        }
        let mut tries = 0;
        while tries <= layout.width {
            tries += 1;
            let y = layout.surface + rand.next_range(50, 100);
            let x = if rand.next_max(2) == 0 {
                rand.next_range(50, (f64::from(layout.width) * 0.3) as i32)
            } else {
                rand.next_range((f64::from(layout.width) * 0.7) as i32, layout.width - 50)
            };
            if place(world, structures, rand, (x, y)) {
                placed += 1;
                break;
            }
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrustia_proto::Tile;

    /// A solid stone block with a clear column above it, which is what the siting checks want.
    fn stone_world(w: i32, h: i32) -> World {
        let mut world = World::empty(w, h, "sword");
        for x in 0..w {
            for y in 200..h {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        world
    }

    #[test]
    fn a_shrine_carves_a_hollow_with_a_mound_and_a_sword_in_it() {
        let mut world = stone_world(600, 600);
        let mut structures = StructureMap::new();
        let mut rand = UnifiedRandom::new(4242);
        assert!(place(&mut world, &mut structures, &mut rand, (300, 300)));

        // The cave is hollow around its centre.
        let cave = (300, 320);
        let mut cleared = 0;
        for x in cave.0 - 10..=cave.0 + 10 {
            for y in cave.1 - 15..cave.1 {
                if !world.tile(x, y).is_active() {
                    cleared += 1;
                }
            }
        }
        assert!(cleared > 100, "expected a real cavity, cleared {cleared}");

        // The sword is a real multi-tile object standing in the mound.
        let mut found = false;
        for x in 295..=305 {
            for y in 300..=330 {
                if world.tile(x, y).block == 187 {
                    found = true;
                }
            }
        }
        assert!(found, "the Enchanted Sword was never placed");
    }

    #[test]
    fn a_shrine_too_close_to_the_surface_is_refused() {
        let mut world = stone_world(600, 600);
        let mut structures = StructureMap::new();
        let mut rand = UnifiedRandom::new(1);
        assert!(!place(&mut world, &mut structures, &mut rand, (300, 40)));
    }

    /// Sand above the shrine means no shrine: vanilla refuses rather than risk a collapse.
    #[test]
    fn sand_in_the_shaft_refuses_the_site() {
        let mut world = stone_world(600, 600);
        for y in 210..260 {
            world.set_tile(300, y, Tile::block(53));
        }
        let mut structures = StructureMap::new();
        let mut rand = UnifiedRandom::new(7);
        assert!(!place(&mut world, &mut structures, &mut rand, (300, 300)));
    }

    /// The same seed and world must produce the same shrine, twice.
    #[test]
    fn placement_is_reproducible_from_the_seed() {
        let run = || {
            let mut world = stone_world(600, 600);
            let mut structures = StructureMap::new();
            let mut rand = UnifiedRandom::new(99);
            place(&mut world, &mut structures, &mut rand, (300, 300));
            let mut fingerprint = Vec::new();
            for x in 260..340 {
                for y in 290..350 {
                    fingerprint.push(world.tile(x, y).block);
                }
            }
            fingerprint
        };
        assert_eq!(run(), run());
    }
}
