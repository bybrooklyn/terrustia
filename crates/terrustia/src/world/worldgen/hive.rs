//! Wild bee hives: honey-filled chambers carved through the jungle mud.
//!
//! Transcribed from `HiveBiome`
//! (`.scratch/decompiled/Terraria.GameContent.Biomes/HiveBiome.cs`, all 425 lines) and its driving
//! `Beehives` pass (`WorldGen.cs:16017-16060`). The thirteenth of the fifteen `MicroBiome` classes,
//! and the largest of those that remained.
//!
//! # Not the same thing as `structures::hive`
//!
//! `micro_biomes.rs` flagged this distinction and it is worth keeping: `structures::hive` builds
//! *one* jungle hive holding the larva that summons the Queen Bee, transcribed separately. This
//! class is the other feature - five to eight wild hives scattered across the whole world at
//! mid-cavern depth, each a knot of two to four wandering tunnels, with honeyfalls dented into
//! their walls. Both exist in vanilla and they are different passes.
//!
//! # How one is carved
//!
//! Refuse the site if it is near a dungeon, temple or existing hive wall, or if the surrounding
//! circle is not at least three-quarters mud. Then wander two to four tunnels out from the origin,
//! each of which paints three nested radii per step: honey and hive wall at the core, hive block in
//! the shell, and hive wall over the middle band. Afterwards, walk out from each tunnel end to find
//! a wall worth cutting a honeyfall into, and put a larva stand at the last tunnel's end.
//!
//! # Disclosed narrowings
//!
//! * `drunkWorldGen` and `remixWorldGen` widen the tunnels and add a second larva; both seeds are
//!   deferred wholesale, so neither branch is modelled.
//! * `FrameOutAllHiveContents` is a pure re-framing pass over hive tiles and hive walls
//!   (`SquareTileFrame`/`SquareWallFrame`), which this generator does not compute - the same call
//!   every other pass here makes. Dropped.
//! * `WorldGen.PoundTile` (hammering a tile to a half-brick) has no counterpart: this generator
//!   writes no slopes or half-bricks, so the dent is cut as full tiles.
//! * The larva positions vanilla records in `GenVars.larvaX/Y` are returned instead, since nothing
//!   here reads a global.

use terrustia_proto::{Liquid, Tile, TileFlags, tile_solid};

use super::genpipe::{Action, Ctx, Link, Shape, chain, gen_shape};
use super::layout::Layout;
use super::rand::UnifiedRandom;
use super::structure_map::{Rect, StructureMap};
use crate::world::World;

/// Hive block.
const HIVE: u16 = 225;
/// Hive wall.
const HIVE_WALL: u16 = 86;
/// Crispy honey block, which a tunnel refuses to overwrite along with the hive wall.
const CRISPY_HONEY_WALL: u16 = 244;
/// Walls that mean a dungeon, a temple or another hive: a site near one is refused.
const IMPORTANT_WALLS: [u16; 3] = [83, 3, 87];
/// Lihzahrd brick, the other marker of somewhere this must not touch.
const LIHZAHRD: u16 = 226;

/// `TooCloseToImportantLocations` (`:253-281`). A coarse 10-tile stride over a 300x300 box.
fn too_close(world: &World, origin: (i32, i32)) -> bool {
    let reach = 150;
    let mut x = origin.0 - reach;
    while x < origin.0 + reach {
        if x > 0 && x < world.width() {
            let mut y = origin.1 - reach;
            while y < origin.1 + reach {
                if y > 0 && y < world.height() {
                    let t = world.tile(x, y);
                    if t.is_active() && t.block == LIHZAHRD {
                        return true;
                    }
                    if IMPORTANT_WALLS.contains(&t.wall) {
                        return true;
                    }
                }
                y += 10;
            }
        }
        x += 10;
    }
    false
}

/// `CreateHiveTunnel` (`:127-250`). Returns where the tunnel ended.
fn tunnel(world: &mut World, rand: &mut UnifiedRandom, from: (i32, i32)) -> (f64, f64) {
    let base = f64::from(rand.next_range(12, 21));
    let mut life = f64::from(rand.next_range(10, 21));
    let mut radius = base;
    let mut at = (f64::from(from.0), f64::from(from.1));
    let mut drift = (
        f64::from(rand.next_range(-10, 11)) * 0.2,
        f64::from(rand.next_range(-10, 11)) * 0.2,
    );

    while radius > 0.0 && life > 0.0 {
        if at.1 > f64::from(world.height() - 250) {
            life = 0.0;
        }
        radius = base * (1.0 + f64::from(rand.next_range(-20, 20)) * 0.01);
        life -= 1.0;

        let x0 = ((at.0 - radius) as i32).max(1);
        let x1 = ((at.0 + radius) as i32).min(world.width() - 1);
        let y0 = ((at.1 - radius) as i32).max(1);
        let y1 = ((at.1 + radius) as i32).min(world.height() - 1);

        for k in x0..x1 {
            for l in y0..y1 {
                // Refuse to grow into a dungeon or temple, or out through the surface.
                if k < 50 || l < 50 || k >= world.width() - 50 || l >= world.height() - 50 {
                    life = 0.0;
                } else {
                    for (dx, dy) in [(-10, 0), (10, 0), (0, -10), (0, 10)] {
                        if world.tile(k + dx, l + dy).wall == 87 {
                            life = 0.0;
                        }
                    }
                }

                let dx = (f64::from(k) - at.0).abs();
                let dy = (f64::from(l) - at.1).abs();
                let dist = (dx * dx + dy * dy).sqrt();

                // The core: honey, hive wall, no block.
                if dist < base * 0.4 * (1.0 + f64::from(rand.next_range(-10, 11)) * 0.005) {
                    let mut t = world.tile(k, l);
                    if rand.next_max(3) == 0 {
                        t.liquid = u8::MAX;
                    }
                    // Vanilla sets the honey flag whether or not it also filled the tile: the flag
                    // and the volume are separate bits there. Kept as written. A kind on a dry
                    // tile does not survive a save, but that is already true of several passes
                    // here and `tiles_equal_for_save` treats it as the non-difference it is.
                    t.liquid_kind = Liquid::Honey;
                    t.wall = HIVE_WALL;
                    t.flags = TileFlags(t.flags.0 & !TileFlags::ACTIVE);
                    // Clear the block id with the active bit. Vanilla leaves `type` set behind an inactive
                    // tile and its own save drops it the same way; keeping the stale id here would mean the
                    // running world and the saved one disagree, which is what `a_generated_world_survives_a_save`
                    // caught: 1113 tiles reading back as air with a hive block still recorded in memory.
                    t.block = 0;
                    t.slope = 0;
                    world.set_tile(k, l, t);
                } else if dist < base * 0.75 * (1.0 + f64::from(rand.next_range(-10, 11)) * 0.005) {
                    // The shell: hive block, unless the wall says this is already hive.
                    let mut t = world.tile(k, l);
                    t.liquid = 0;
                    if t.wall != HIVE_WALL && t.wall != CRISPY_HONEY_WALL {
                        t.flags = TileFlags(t.flags.0 | TileFlags::ACTIVE);
                        t.slope = 0;
                        t.block = HIVE;
                    }
                    world.set_tile(k, l, t);
                }

                // A wider band gets the hive wall whatever else happened to it.
                if dist < base * 0.6 * (1.0 + f64::from(rand.next_range(-10, 11)) * 0.005) {
                    let mut t = world.tile(k, l);
                    t.wall = HIVE_WALL;
                    world.set_tile(k, l, t);
                }
            }
        }

        at = (at.0 + drift.0, at.1 + drift.1);
        life -= 1.0;
        drift.1 += f64::from(rand.next_range(-10, 11)) * 0.05;
        drift.0 += f64::from(rand.next_range(-10, 11)) * 0.05;
    }
    at
}

/// `BadSpotForHoneyFall` (`:348-355`).
fn bad_spot(world: &World, x: i32, y: i32) -> bool {
    if world.tile(x, y).is_active()
        && world.tile(x, y + 1).is_active()
        && world.tile(x + 1, y).is_active()
    {
        return !world.tile(x + 1, y + 1).is_active();
    }
    true
}

/// `SpotActuallyNotInHive` (`:329-346`).
fn not_in_hive(world: &World, x: i32, y: i32) -> bool {
    for i in x - 1..=x + 2 {
        for j in y - 1..=y + 2 {
            if i < 10 || i > world.width() - 10 {
                return true;
            }
            let t = world.tile(i, j);
            if t.is_active() && t.block != HIVE {
                return true;
            }
        }
    }
    false
}

/// `CreateBlockedHoneyCube` (`:309-327`): a 2x2 pocket of honey walled in hive.
fn honey_cube(world: &mut World, x: i32, y: i32) {
    for i in x - 1..=x + 2 {
        for j in y - 1..=y + 2 {
            let mut t = world.tile(i, j);
            if i >= x && i <= x + 1 && j >= y && j <= y + 1 {
                t.flags = TileFlags(t.flags.0 & !TileFlags::ACTIVE);
                // Clear the block id with the active bit. Vanilla leaves `type` set behind an inactive
                // tile and its own save drops it the same way; keeping the stale id here would mean the
                // running world and the saved one disagree, which is what `a_generated_world_survives_a_save`
                // caught: 1113 tiles reading back as air with a hive block still recorded in memory.
                t.block = 0;
                t.liquid = u8::MAX;
                t.liquid_kind = Liquid::Honey;
            } else {
                t.flags = TileFlags(t.flags.0 | TileFlags::ACTIVE);
                t.block = HIVE;
            }
            world.set_tile(i, j, t);
        }
    }
}

/// `CreateDentForHoneyFall` (`:288-307`): cut sideways until the honey has somewhere to pour.
fn dent(world: &mut World, mut x: i32, y: i32, dir: i32) {
    let dir = -dir;
    let y = y + 1;
    let mut cut = 0;
    while (cut < 4 || solid(world, x, y)) && x > 10 && x < world.width() - 10 {
        cut += 1;
        x += dir;
        if solid(world, x, y) {
            // Vanilla hammers this to a half-brick; see the module doc.
            world.set_tile(x, y, Tile::AIR);
            if !world.tile(x, y + 1).is_active() {
                world.set_tile(x, y + 1, Tile::block(HIVE));
            }
        }
    }
}

fn solid(world: &World, x: i32, y: i32) -> bool {
    let t = world.tile(x, y);
    t.is_active() && tile_solid::solid(t.block)
}

/// `CreateStandForLarva` (`:357-390`): a 3x4 alcove with a hive floor, for a larva to sit on.
/// Returns the tile the larva belongs at.
fn larva_stand(world: &mut World, at: (f64, f64)) -> (i32, i32) {
    let x = (at.0 as i32).clamp(5, world.width() - 5);
    let y = (at.1 as i32).clamp(5, world.height() - 5);
    for i in x - 1..=x + 1 {
        if i <= 0 || i >= world.width() {
            continue;
        }
        for j in y - 2..=y + 1 {
            if j <= 0 || j >= world.height() {
                continue;
            }
            if j != y + 1 {
                let mut t = world.tile(i, j);
                t.flags = TileFlags(t.flags.0 & !TileFlags::ACTIVE);
                // Clear the block id with the active bit. Vanilla leaves `type` set behind an inactive
                // tile and its own save drops it the same way; keeping the stale id here would mean the
                // running world and the saved one disagree, which is what `a_generated_world_survives_a_save`
                // caught: 1113 tiles reading back as air with a hive block still recorded in memory.
                t.block = 0;
                world.set_tile(i, j, t);
            } else {
                let mut t = world.tile(i, j);
                t.flags = TileFlags(t.flags.0 | TileFlags::ACTIVE);
                t.block = HIVE;
                t.slope = 0;
                world.set_tile(i, j, t);
            }
        }
    }
    (x, y)
}

/// One hive at `origin`. On success returns where its larva stand went.
pub fn place(
    world: &mut World,
    structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
    origin: (i32, i32),
) -> Option<(i32, i32)> {
    if !structures.can_place(world, Rect::new(origin.0 - 50, origin.1 - 50, 100, 100), 0) {
        return None;
    }
    if too_close(world, origin) {
        return None;
    }

    // The surroundings must be mostly mud, with at least a little jungle grass.
    {
        let mut ctx = Ctx::new(world, rand);
        let all = ctx.counter();
        let muddy = ctx.counter();
        let grassy = ctx.counter();
        let mut c = chain([
            Link::new(Action::IsSolid),
            Link::new(Action::Scanner(all)),
            Link::new(Action::OnlyTiles(vec![60, 59])),
            Link::new(Action::Scanner(muddy)),
            Link::new(Action::OnlyTiles(vec![60])),
            Link::new(Action::Scanner(grassy)),
        ]);
        gen_shape(
            &mut ctx,
            origin,
            &Shape::Circle {
                h_radius: 15,
                v_radius: 15,
            },
            &mut c,
        );
        let total = ctx.counted(all);
        if total == 0 || f64::from(ctx.counted(muddy)) / f64::from(total) < 0.75 {
            return None;
        }
        if ctx.counted(grassy) < 2 {
            return None;
        }
    }

    let mut ends: Vec<(i32, i32)> = Vec::new();
    let mut at = (f64::from(origin.0), f64::from(origin.1));
    let knots = rand.next_range(2, 5);
    for _ in 0..knots {
        let mut end = at;
        let branches = rand.next_range(2, 5);
        for _ in 0..branches {
            // Vanilla re-tunnels from the *same* start each time and keeps only the last result,
            // so the first `branches - 1` tunnels are carved and then discarded as a position.
            end = tunnel(world, rand, (at.0 as i32, at.1 as i32));
        }
        at = end;
        ends.push((at.0 as i32, at.1 as i32));
    }

    // Honeyfalls: walk sideways from each tunnel end to a wall worth cutting.
    for &(ex, ey) in &ends {
        let dir = if rand.next_max(2) == 0 { -1 } else { 1 };
        let mut x = ex;
        let mut gave_up = false;
        while world.in_bounds(x, ey) && bad_spot(world, x, ey) {
            x += dir;
            if (x - ex).abs() > 50 {
                gave_up = true;
                break;
            }
        }
        if !gave_up {
            x += dir;
            if !not_in_hive(world, x, ey) {
                honey_cube(world, x, ey);
                dent(world, x, ey, dir);
            }
        }
    }

    let larva = larva_stand(world, at);
    structures.add_protected_structure(Rect::new(origin.0 - 50, origin.1 - 50, 100, 100), 5);
    Some(larva)
}

/// The `Beehives` pass (`WorldGen.cs:16017-16045`): five to eight hives scaled with world width,
/// sited between the surface and the rock layer's midpoint. Returns the larva positions.
pub fn scatter(
    world: &mut World,
    layout: &Layout,
    structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
) -> Vec<(i32, i32)> {
    let scale = f64::from(layout.width) / 4200.0;
    let lo = (5.0 * scale) as i32;
    let hi = (8.0 * scale) as i32;
    if hi <= lo || layout.width < 400 {
        return Vec::new();
    }
    let mut wanted = 1 + rand.next_range(lo, hi);
    let mut budget = 10_000;
    let mut larvae = Vec::new();
    let top = (layout.surface + layout.rock) / 2;
    if layout.height <= 300 || top >= layout.height - 300 {
        return larvae;
    }
    while wanted > 0 && budget > 0 {
        budget -= 1;
        let x = rand.next_range(20, layout.width - 20);
        let y = rand.next_range(top, layout.height - 300);
        if let Some(larva) = place(world, structures, rand, (x, y)) {
            larvae.push(larva);
            wanted -= 1;
        }
    }
    larvae
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mud with a skin of jungle grass, which is what the site census wants.
    fn mud(w: i32, h: i32) -> World {
        let mut world = World::empty(w, h, "hive");
        for x in 0..w {
            for y in 100..h {
                world.set_tile(x, y, Tile::block(59));
            }
        }
        for x in 0..w {
            for y in 100..104 {
                world.set_tile(x, y, Tile::block(60));
            }
        }
        // Jungle grass down at the hive depth too. The census wants at least two tiles of it
        // within radius 15, which surface grass alone does not satisfy - the first version of this
        // fixture was refused for exactly that.
        for x in 295..305 {
            world.set_tile(x, 300, Tile::block(60));
        }
        world
    }

    #[test]
    fn a_hive_carves_honey_and_hive_block_and_leaves_a_larva_stand() {
        let mut world = mud(600, 800);
        let mut structures = StructureMap::new();
        let mut rand = UnifiedRandom::new(90210);
        let larva = place(&mut world, &mut structures, &mut rand, (300, 300));
        let larva = larva.expect("the hive was refused on a clean mud site");

        let mut honey = 0;
        let mut hive = 0;
        let mut walls = 0;
        for x in 200..400 {
            for y in 200..420 {
                let t = world.tile(x, y);
                if t.liquid > 0 && t.liquid_kind == Liquid::Honey {
                    honey += 1;
                }
                if t.block == HIVE && t.is_active() {
                    hive += 1;
                }
                if t.wall == HIVE_WALL {
                    walls += 1;
                }
            }
        }
        assert!(honey > 0, "no honey");
        assert!(hive > 100, "hive block too sparse: {hive}");
        assert!(walls > 100, "hive wall too sparse: {walls}");
        assert!(
            world.tile(larva.0, larva.1 + 1).block == HIVE,
            "the larva stand has no floor"
        );
    }

    /// A site next to a dungeon wall is refused.
    #[test]
    fn a_site_near_a_dungeon_wall_is_refused() {
        let mut world = mud(600, 800);
        for x in 280..320 {
            for y in 280..320 {
                let mut t = world.tile(x, y);
                t.wall = 3;
                world.set_tile(x, y, t);
            }
        }
        let mut structures = StructureMap::new();
        let mut rand = UnifiedRandom::new(3);
        assert!(place(&mut world, &mut structures, &mut rand, (300, 300)).is_none());
    }

    /// A stone site is refused: not enough mud.
    #[test]
    fn a_stone_site_is_refused() {
        let mut world = World::empty(600, 800, "stone");
        for x in 0..600 {
            for y in 100..800 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        let mut structures = StructureMap::new();
        let mut rand = UnifiedRandom::new(4);
        assert!(place(&mut world, &mut structures, &mut rand, (300, 300)).is_none());
    }
}
