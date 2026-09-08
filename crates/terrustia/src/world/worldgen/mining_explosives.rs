//! A rigged ore vein: explosives wired to a detonator, sitting in a pocket of ore.
//!
//! Transcribed from `MiningExplosivesBiome`
//! (`.scratch/decompiled/Terraria.GameContent.Biomes/MiningExplosivesBiome.cs`, all 85 lines). The
//! eleventh of the fifteen `MicroBiome` classes, and the second thing it needed beyond
//! [`super::genpipe`] was `ShapeRunner` (98 lines, now in that module) and `WorldUtils.WireLine`.
//!
//! # What a player finds
//!
//! A short ore vein dug by a wandering blob, a small cavity blown out of one end, and a stack of
//! Explosives on the floor wired to a Detonator a few tiles away. Throwing the switch collapses the
//! vein. It is one of the few places vanilla ships wiring as *content* rather than as a trap.
//!
//! # Disclosed narrowings
//!
//! * **Ore choice.** Vanilla picks between the two ore tiers by asking which bar the world rolled
//!   (`GenVars.goldBar == 19 ? 8 : 169`, and so on for silver, iron and copper). This generator
//!   always lays down the four basic ores - `world.ore_tiers` is fixed to copper, iron, silver,
//!   gold, with the alternates unimplemented - so the choice reads that array instead. The array
//!   is indexed in vanilla's own order (gold, silver, iron, copper) so the single `Next(4)` draw
//!   maps to the same ore vanilla would pick, rather than only to the same distribution.
//! * `WorldGen.CanKillTile` has no counterpart here; the check that refuses to bury an
//!   unbreakable tile under the detonator is dropped, which can only matter where a dungeon or
//!   temple tile has already been placed, and the wall check above already refuses those sites.
//! * Slope and half-brick normalisation is dropped: this generator never writes slopes, so the
//!   floor under the detonator is already flat.

use terrustia_proto::{Tile, tile_solid};

use super::genpipe::{Action, Ctx, Link, Shape, chain, gen_shape, gen_shape_out, wire_line};
use super::layout::Layout;
use super::place_object::place_object;
use super::rand::UnifiedRandom;
use super::secret_seed::SecretSeeds;
use super::structure_map::{Rect, StructureMap};
use crate::world::World;

/// Explosives.
const EXPLOSIVES: u16 = 141;
/// Detonator.
const DETONATOR: u16 = 411;
/// Walls a site is refused on: vanilla names 216 and 187 directly.
const REFUSED_WALLS: [u16; 2] = [216, 187];

fn solid_at(world: &World, x: i32, y: i32) -> bool {
    let t = world.tile(x, y);
    t.is_active() && tile_solid::solid(t.block)
}

/// The first solid tile scanning in one direction, at most `max` away. `Searches.Left`/`Right`/
/// `Down` chained with `Conditions.IsSolid`.
fn find_solid(world: &World, from: (i32, i32), step: (i32, i32), max: i32) -> Option<(i32, i32)> {
    (0..max)
        .map(|i| (from.0 + step.0 * i, from.1 + step.1 * i))
        .find(|&(x, y)| solid_at(world, x, y))
}

/// One rigged vein at `origin`.
pub fn place(
    world: &mut World,
    structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
    origin: (i32, i32),
) -> bool {
    if solid_at(world, origin.0, origin.1) {
        return false;
    }
    if REFUSED_WALLS.contains(&world.tile(origin.0, origin.1).wall) {
        return false;
    }

    // Vanilla's order: gold, silver, iron, copper. See the module doc for why this reads the
    // world's own tier table rather than the bar ids.
    let ores = [
        world.ore_tiers[3],
        world.ore_tiers[2],
        world.ore_tiers[1],
        world.ore_tiers[0],
    ];
    let ore = ores[rand.next_max(4) as usize];
    if ore < 0 {
        return false;
    }
    let ore = ore as u16;

    let drift = rand.next_double() * 2.0 - 1.0;
    let step = if drift > 0.0 { (1, 0) } else { (-1, 0) };
    let Some(origin) = find_solid(world, origin, step, 40) else {
        return false;
    };
    let Some(origin) = find_solid(world, origin, (0, 1), 80) else {
        return false;
    };

    let mut ctx = Ctx::new(world, rand);
    let vein = ctx.slot();
    let visited = ctx.counter();
    let solid_seen = ctx.counter();
    let mut c = chain([
        Link::new(Action::Blotches {
            min_x: 2,
            min_y: 2,
            max_x: 2,
            max_y: 2,
            chance: 0.3,
        }),
        Link::new(Action::Scanner(visited)),
        Link::new(Action::IsSolid),
        Link::new(Action::Scanner(solid_seen)),
    ]);
    // The runner records where it went into `vein`; the chain counts how much of that was solid
    // ground, which is the test for whether the vein is buried or hanging in open air.
    gen_shape_out(
        &mut ctx,
        origin,
        &Shape::Runner {
            strength: 10.0,
            steps: 20,
            velocity: (drift, 1.0),
        },
        &mut c,
        vein,
    );

    // A vein mostly in open air is not a vein.
    if ctx.counted(solid_seen) < ctx.counted(visited) / 2 {
        return false;
    }

    let area = Rect::new(origin.0 - 15, origin.1 - 10, 30, 20);
    if !structures.can_place(ctx.world, area, 0) {
        return false;
    }

    let vein_shape = ctx.take(vein);
    let mut c = chain([Link::new(Action::SetTile(ore))]);
    gen_shape(&mut ctx, origin, &Shape::All(vein_shape), &mut c);

    // Blow a pocket out of the upwind end.
    let mut c = chain([
        Link::new(Action::Blotches {
            min_x: 2,
            min_y: 2,
            max_x: 2,
            max_y: 2,
            chance: 0.3,
        }),
        Link::new(Action::ClearTile),
    ]);
    gen_shape(
        &mut ctx,
        (origin.0 - (drift * -5.0) as i32, origin.1 - 5),
        &Shape::Circle {
            h_radius: 5,
            v_radius: 5,
        },
        &mut c,
    );

    // Two floors: one for the explosives, one for the detonator.
    let near_dx = if drift > 0.0 { 3 } else { -3 };
    let spread = if ctx.rand.next_max(4) == 0 { 3 } else { 7 };
    let far_dx = if drift > 0.0 { -spread } else { spread };
    let Some(mut charge) = find_solid(ctx.world, (origin.0 - near_dx, origin.1 - 3), (0, 1), 10)
    else {
        return false;
    };
    let Some(mut switch) = find_solid(ctx.world, (origin.0 - far_dx, origin.1 - 3), (0, 1), 10)
    else {
        return false;
    };
    charge.1 -= 1;
    switch.1 -= 1;

    // Clear a standing space for the detonator and make sure it has a floor.
    for i in -1..=1 {
        for dy in 0..=4 {
            let (x, y) = (switch.0 + i, switch.1 - dy);
            if ctx.world.in_bounds(x, y) {
                ctx.world.set_tile(x, y, Tile::AIR);
            }
        }
        let (fx, fy) = (switch.0 + i, switch.1 + 1);
        if !solid_at(ctx.world, fx, fy) {
            ctx.world.set_tile(fx, fy, Tile::block(1));
        }
    }

    place_object(ctx.world, charge.0, charge.1, EXPLOSIVES, 0, -1);
    place_object(ctx.world, switch.0, switch.1, DETONATOR, 0, -1);
    wire_line(ctx.world, charge, switch);
    structures.add_protected_structure(area, 5);
    true
}

/// `WorldGenRange.ScaleValue` for `ScaleWith: WorldArea` (`WorldGenRange.cs:43-57`).
fn scaled_by_area(value: i32, width: i32, height: i32) -> i32 {
    ((f64::from(width) * f64::from(height)) / 5_040_000.0 * f64::from(value)) as i32
}

/// The driving loop (`WorldGen.cs:21239-21272`). `ExplosiveTrapCount` is 14-29 scaled with world
/// area, with vanilla's own 3000-attempt budget. Returns how many were placed.
///
/// The No Traps World short-circuit is honoured: vanilla skips this whole block under
/// `actuallyNoTrapsForRealIMeanIt`, and these are traps in every sense a player cares about.
pub fn scatter(
    world: &mut World,
    layout: &Layout,
    structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
    secret: SecretSeeds,
) -> usize {
    if secret.no_traps {
        return 0;
    }
    let min = scaled_by_area(14, layout.width, layout.height);
    let max = scaled_by_area(29, layout.width, layout.height);
    if max < min || min < 1 {
        return 0;
    }
    let wanted = rand.next_range(min, max + 1);
    let mut budget = 3000;
    let mut placed = 0usize;
    let beach = 380;
    if layout.width <= beach * 2 || layout.height <= layout.rock + 200 {
        return 0;
    }
    while placed < wanted as usize {
        budget -= 1;
        if budget <= 0 {
            break;
        }
        let x = rand.next_range(beach, layout.width - beach);
        let y = rand.next_range(layout.rock, layout.height - 200);
        if place(world, structures, rand, (x, y)) {
            placed += 1;
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stone(w: i32, h: i32) -> World {
        let mut world = World::empty(w, h, "mining");
        for x in 0..w {
            for y in 100..h {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        world.ore_tiers = [7, 6, 9, 8, -1, -1, -1];
        world
    }

    /// Carve a cave pocket, because that is the only shape this biome accepts: the origin must be
    /// open air with rock within 40 tiles horizontally and 80 below. An origin in open sky fails
    /// the very first search, which is how the first version of this test failed.
    fn cave(world: &mut World, cx: i32, cy: i32, r: i32) {
        for x in cx - r..=cx + r {
            for y in cy - r..=cy + r {
                if (x - cx).pow(2) + (y - cy).pow(2) <= r * r {
                    world.set_tile(x, y, Tile::AIR);
                }
            }
        }
    }

    #[test]
    fn a_rigged_vein_places_explosives_a_detonator_and_wire_between_them() {
        let mut world = stone(400, 400);
        cave(&mut world, 200, 200, 6);
        let mut structures = StructureMap::new();
        let mut rand = UnifiedRandom::new(2024);
        assert!(place(&mut world, &mut structures, &mut rand, (200, 200)));

        let mut explosives = 0;
        let mut detonators = 0;
        let mut wires = 0;
        for x in 150..250 {
            for y in 150..300 {
                let t = world.tile(x, y);
                if t.block == EXPLOSIVES {
                    explosives += 1;
                }
                if t.block == DETONATOR {
                    detonators += 1;
                }
                if t.flags.has(terrustia_proto::TileFlags::WIRE_RED) {
                    wires += 1;
                }
            }
        }
        assert!(explosives > 0, "no explosives");
        assert!(detonators > 0, "no detonator");
        assert!(wires > 0, "nothing wired the two together");
    }

    #[test]
    fn a_site_inside_solid_rock_is_refused() {
        let mut world = stone(400, 400);
        let mut structures = StructureMap::new();
        let mut rand = UnifiedRandom::new(5);
        assert!(!place(&mut world, &mut structures, &mut rand, (200, 300)));
    }
}
