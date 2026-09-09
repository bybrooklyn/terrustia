//! The jungle's Living Mahogany tree: a hollow trunk with a chest in its root.
//!
//! Transcribed from `MahoganyTreeBiome`
//! (`.scratch/decompiled/Terraria.GameContent.Biomes/MahoganyTreeBiome.cs`, all 94 lines), driven
//! from `WorldGen.cs:21279` alongside the living trees. The twelfth of the fifteen `MicroBiome`
//! classes.
//!
//! `micro_biomes.rs` recorded this as "a second, separate tree-growing subsystem distinct from
//! `living_trees.rs`'s own `GrowLivingTree` port, confirmed by reading it: different tile ids,
//! 383/384 vs Living Wood, and its own `ShapeBranch`/`ShapeRoot` growth shapes, sited specifically
//! in the jungle with a real jungle-chest reward". That is exactly what this is. The two growth
//! shapes now live in [`super::genpipe`], where they are 60 lines rather than a subsystem, because
//! the pipeline they plug into already exists.
//!
//! # How it grows
//!
//! Find a wide floor, check there is 30 to 60 tiles of headroom, then check the surroundings are
//! actually jungle rather than dirt, stone or snow by counting a 50x50 box. The trunk then rises in
//! five-tile segments that sway along a sine curve, each segment solid mahogany with its middle
//! hollowed out and walled. Two limbs fork off partway up and two more crown it, every limb tip
//! gets a blob of leaves, four roots splay out below, and a jungle chest goes in the base.
//!
//! # Disclosed narrowings
//!
//! * The `drunkWorldGen` branch that skips the biome check one time in fifty is not modelled;
//!   Drunk World is deferred wholesale.
//! * `WorldGen.AddBuriedChest` is a large placement helper with its own site search; this uses the
//!   generator's own `structures::add_chest` at the base tile vanilla names, with the jungle loot
//!   table `biome_chest_loot` already builds. That function's own doc discloses where its jungle
//!   cycle narrows `GetNextJungleChestItem`.
//! * Tile framing is dropped throughout, as everywhere else in this generator.

use terrustia_proto::Tile;

use super::genpipe::{Action, Ctx, Link, Shape, chain, gen_shape};
use super::layout::Layout;
use super::rand::UnifiedRandom;
use super::structure_map::{Rect, StructureMap};
use super::structures;
use crate::world::World;

/// Living Mahogany.
const MAHOGANY: u16 = 383;
/// Mahogany leaves.
const LEAVES: u16 = 384;
/// The wall the hollow trunk is lined with.
const TRUNK_WALL: u16 = 78;
/// Tiles a limb refuses to overwrite: chests, containers, dressers, and the like.
const KEEP_TILES: [u16; 4] = [21, 467, 226, 237];
/// The wall a limb refuses to grow through.
const KEEP_WALLS: [u16; 1] = [87];

fn solid_run(world: &World, x: i32, y: i32, width: i32) -> bool {
    (0..width).all(|i| {
        let t = world.tile(x + i, y);
        t.is_active() && terrustia_proto::tile_solid::solid(t.block)
    })
}

fn any_solid_run(world: &World, x: i32, y: i32, width: i32) -> bool {
    (0..width).any(|i| {
        let t = world.tile(x + i, y);
        t.is_active() && terrustia_proto::tile_solid::solid(t.block)
    })
}

/// One tree at `origin`.
pub fn place(
    world: &mut World,
    layout: &Layout,
    structures_map: &mut StructureMap,
    rand: &mut UnifiedRandom,
    origin: (i32, i32),
) -> bool {
    // A floor six tiles wide, within 200 below. `Searches.Down(200)` + `IsSolid().AreaAnd(6, 1)`.
    let floor = (0..200)
        .map(|i| (origin.0 - 3, origin.1 + i))
        .find(|&(x, y)| solid_run(world, x, y, 6));
    let Some(floor) = floor else {
        return false;
    };

    // A ceiling within 120 above, and between 30 and 60 tiles of headroom.
    // `Searches.Up(120)` + `IsSolid().AreaOr(6, 1)`.
    let ceiling = (0..120)
        .map(|i| (floor.0, floor.1 - 5 - i))
        .find(|&(x, y)| any_solid_run(world, x, y, 6));
    let Some(ceiling) = ceiling else {
        return false;
    };
    if floor.1 - 5 - ceiling.1 > 60 || floor.1 - ceiling.1 < 30 {
        return false;
    }

    if !structures_map.can_place(world, Rect::new(floor.0 - 30, floor.1 - 60, 60, 90), 0) {
        return false;
    }

    // The surroundings must actually be jungle. Vanilla counts a 50x50 box and refuses if there is
    // more dirt-and-stone than mud, or more ice and snow than mud, or simply not enough mud.
    {
        let mut ctx = Ctx::new(world, rand);
        ctx.scan_reset();
        let mut c = chain([Link::new(Action::TileScanner(vec![
            0, 59, 60, 147, 161, 163, 200, 164, 1, 25, 203, 117,
        ]))]);
        gen_shape(
            &mut ctx,
            (floor.0 - 25, floor.1 - 25),
            &Shape::Rectangle {
                left: 0,
                top: 0,
                width: 50,
                height: 50,
            },
            &mut c,
        );
        let stone =
            ctx.scan_count(1) + ctx.scan_count(25) + ctx.scan_count(203) + ctx.scan_count(117);
        let dirt = ctx.scan_count(0) + stone;
        let mud = ctx.scan_count(59) + ctx.scan_count(60);
        let ice =
            ctx.scan_count(161) + ctx.scan_count(163) + ctx.scan_count(200) + ctx.scan_count(164);
        if ctx.scan_count(147) + ice > mud || dirt > mud || mud < 50 {
            return false;
        }
    }

    let segments = (floor.1 - ceiling.1 - 9) / 5;
    if segments < 1 {
        return false;
    }
    let height = segments * 5;

    let mut ctx = Ctx::new(world, rand);
    let sway = ctx.rand.next_double() + 1.0;
    let mut lean = ctx.rand.next_double() + 2.0;
    if ctx.rand.next_max(2) == 0 {
        lean = -lean;
    }

    // The trunk: solid mahogany, hollowed and walled down the middle.
    let mut previous = 0;
    for i in 0..segments {
        let offset =
            (((f64::from(i + 1) / 12.0) * sway * std::f64::consts::PI).sin() * lean) as i32;
        let back = if offset < previous {
            offset - previous
        } else {
            0
        };
        let widened = (offset - previous).abs();
        let base_x = floor.0 + previous + back;
        let y = floor.1 - (i + 1) * 5;

        let mut c = chain([
            Link::new(Action::SkipTiles(KEEP_TILES.to_vec())),
            Link::new(Action::SkipWalls(KEEP_WALLS.to_vec())),
            Link::new(Action::RemoveWall),
            Link::new(Action::SetTile(MAHOGANY)),
            Link::new(Action::SetFrames),
        ]);
        gen_shape(
            &mut ctx,
            (base_x, y),
            &Shape::Rectangle {
                left: 0,
                top: 0,
                width: 6 + widened,
                height: 7,
            },
            &mut c,
        );

        let mut c = chain([
            Link::new(Action::SkipTiles(KEEP_TILES.to_vec())),
            Link::new(Action::SkipWalls(KEEP_WALLS.to_vec())),
            Link::new(Action::ClearTile),
            Link::new(Action::PlaceWall(TRUNK_WALL)),
        ]);
        gen_shape(
            &mut ctx,
            (base_x + 2, y),
            &Shape::Rectangle {
                left: 0,
                top: 0,
                width: 2 + widened,
                height: 5,
            },
            &mut c,
        );

        // The joint between this segment and the one below, so the hollow is continuous.
        let mut c = chain([
            Link::new(Action::SkipTiles(KEEP_TILES.to_vec())),
            Link::new(Action::SkipWalls(KEEP_WALLS.to_vec())),
            Link::new(Action::ClearTile),
            Link::new(Action::PlaceWall(TRUNK_WALL)),
        ]);
        gen_shape(
            &mut ctx,
            (floor.0 + previous + 2, floor.1 - i * 5),
            &Shape::Rectangle {
                left: 0,
                top: 0,
                width: 2,
                height: 2,
            },
            &mut c,
        );

        previous = offset;
    }

    // Limbs. Two partway up, alternating sides, then two crowning the top.
    let mut side = if lean < 0.0 { 0 } else { 6 };
    ctx.branch_ends.clear();
    for j in 0..2 {
        let up = (f64::from(j) + 1.0) / 3.0;
        let x = side
            + ((f64::from(segments) * up / 12.0 * sway * std::f64::consts::PI).sin() * lean) as i32;
        let mut angle = ctx.rand.next_double() * std::f64::consts::FRAC_PI_4
            - std::f64::consts::FRAC_PI_4
            - 0.2;
        if side == 0 {
            angle -= std::f64::consts::FRAC_PI_2;
        }
        let distance = f64::from(ctx.rand.next_range(12, 16));
        let y = floor.1 - (f64::from(segments * 5) * up) as i32;
        limb(&mut ctx, (floor.0 + x, y), angle, distance);
        side = 6 - side;
    }

    let crown = ((f64::from(segments) / 12.0) * sway * std::f64::consts::PI).sin() * lean;
    let crown = crown as i32;
    let d1 = f64::from(ctx.rand.next_range(16, 22));
    limb(
        &mut ctx,
        (floor.0 + 6 + crown, floor.1 - height),
        -0.6853981852531433,
        d1,
    );
    let d2 = f64::from(ctx.rand.next_range(16, 22));
    limb(
        &mut ctx,
        (floor.0 + crown, floor.1 - height),
        -2.45619455575943,
        d2,
    );

    // Leaves at every limb tip.
    let tips = std::mem::take(&mut ctx.branch_ends);
    for tip in tips {
        let mut c = chain([
            Link::new(Action::Blotches {
                min_x: 4,
                min_y: 4,
                max_x: 4,
                max_y: 4,
                chance: 2.0,
            }),
            Link::new(Action::SkipTiles(vec![MAHOGANY, 21, 467, 226, 237])),
            Link::new(Action::SkipWalls(vec![TRUNK_WALL, 87])),
            Link::new(Action::SetTile(LEAVES)),
            Link::new(Action::SetFrames),
        ]);
        gen_shape(
            &mut ctx,
            tip,
            &Shape::Circle {
                h_radius: 4,
                v_radius: 4,
            },
            &mut c,
        );
    }

    // Roots.
    for k in 0..4 {
        let angle = f64::from(k) / 3.0 * 2.0 + 0.57075;
        let distance = f64::from(ctx.rand.next_range(40, 60));
        let mut c = chain([
            Link::new(Action::SkipTiles(KEEP_TILES.to_vec())),
            Link::new(Action::SkipWalls(KEEP_WALLS.to_vec())),
            Link::new(Action::SetTile(MAHOGANY)),
        ]);
        gen_shape(
            &mut ctx,
            floor,
            &Shape::Root {
                angle,
                distance,
                starting_size: 4.0,
                ending_size: 1.0,
            },
            &mut c,
        );
    }

    // The reward.
    let loot = structures::biome_chest_loot(layout, floor.0 + 3, floor.1 - 1, ctx.rand)
        .unwrap_or_default();
    if !loot.is_empty() {
        // Clear the two cells the chest needs, since the trunk just filled them.
        for dx in 0..2 {
            for dy in 0..2 {
                ctx.world
                    .set_tile(floor.0 + 3 + dx, floor.1 - 1 - dy, Tile::AIR);
            }
        }
        structures::add_chest(ctx.world, floor.0 + 3, floor.1 - 1, loot, ctx.rand);
    }

    structures_map.add_protected_structure(Rect::new(floor.0 - 30, floor.1 - 30, 60, 60), 0);
    true
}

/// One `ShapeBranch` limb, with the chain every limb in this tree shares.
fn limb(ctx: &mut Ctx, at: (i32, i32), angle: f64, distance: f64) {
    let mut c = chain([
        Link::new(Action::SkipTiles(KEEP_TILES.to_vec())),
        Link::new(Action::SkipWalls(KEEP_WALLS.to_vec())),
        Link::new(Action::SetTile(MAHOGANY)),
        Link::new(Action::SetFrames),
    ]);
    gen_shape(ctx, at, &Shape::Branch { angle, distance }, &mut c);
}

/// The driving loop shares `LivingTreeCount` with the living trees (`WorldGen.cs:21279-21284`).
/// Returns how many grew.
pub fn scatter(
    world: &mut World,
    layout: &Layout,
    structures_map: &mut StructureMap,
    rand: &mut UnifiedRandom,
) -> usize {
    if layout.jungle.width() < 100 {
        return 0;
    }
    let wanted = 1 + rand.next_max(2);
    let mut grown = 0;
    let mut budget = 20_000;
    while grown < wanted && budget > 0 {
        budget -= 1;
        let x = rand.next_range(layout.jungle.from + 20, layout.jungle.to - 20);
        let y = rand.next_range(layout.surface, layout.rock.max(layout.surface + 1));
        if place(world, layout, structures_map, rand, (x, y)) {
            grown += 1;
        }
    }
    grown as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mud world with a hollow the tree can stand in: mud floor, open air above, mud ceiling.
    fn jungle(w: i32, h: i32) -> World {
        let mut world = World::empty(w, h, "mahogany");
        for x in 0..w {
            for y in 100..h {
                world.set_tile(x, y, Tile::block(59));
            }
        }
        // Carve a 40-tall chamber.
        for x in 100..300 {
            for y in 150..190 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        world
    }

    /// A planned layout, then overridden so the jungle band and the depths line up with the test
    /// world the fixture actually builds.
    fn layout_for(world: &World) -> Layout {
        let mut l = Layout::plan(world.width(), world.height(), &mut UnifiedRandom::new(7));
        l.surface = 100;
        l.rock = 200;
        l.jungle = super::super::layout::Band { from: 50, to: 350 };
        l
    }

    #[test]
    fn a_mahogany_tree_grows_a_trunk_leaves_and_roots() {
        let mut world = jungle(400, 400);
        let layout = layout_for(&world);
        let mut structures_map = StructureMap::new();
        let mut rand = UnifiedRandom::new(4321);
        assert!(place(
            &mut world,
            &layout,
            &mut structures_map,
            &mut rand,
            (200, 150)
        ));

        let mut trunk = 0;
        let mut leaves = 0;
        let mut walls = 0;
        for x in 130..270 {
            for y in 120..200 {
                let t = world.tile(x, y);
                if t.block == MAHOGANY {
                    trunk += 1;
                }
                if t.block == LEAVES {
                    leaves += 1;
                }
                if t.wall == TRUNK_WALL {
                    walls += 1;
                }
            }
        }
        assert!(trunk > 100, "trunk too small: {trunk}");
        assert!(leaves > 0, "no leaves at the limb tips");
        assert!(walls > 0, "the trunk was never hollowed");
    }

    #[test]
    fn a_site_with_no_headroom_is_refused() {
        let mut world = World::empty(400, 400, "flat");
        for x in 0..400 {
            for y in 100..400 {
                world.set_tile(x, y, Tile::block(59));
            }
        }
        let layout = layout_for(&world);
        let mut structures_map = StructureMap::new();
        let mut rand = UnifiedRandom::new(1);
        assert!(!place(
            &mut world,
            &layout,
            &mut structures_map,
            &mut rand,
            (200, 150)
        ));
    }

    #[test]
    fn a_stone_surround_is_refused_because_it_is_not_jungle() {
        let mut world = jungle(400, 400);
        // Replace the mud around the site with stone: the biome census should reject it.
        for x in 150..250 {
            for y in 190..240 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        let layout = layout_for(&world);
        let mut structures_map = StructureMap::new();
        let mut rand = UnifiedRandom::new(2);
        assert!(!place(
            &mut world,
            &layout,
            &mut structures_map,
            &mut rand,
            (200, 150)
        ));
    }
}
