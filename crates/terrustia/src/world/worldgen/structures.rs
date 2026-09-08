//! The things a world needs before it can be played through.
//!
//! Terrain alone is scenery. What makes a world *beatable* is a specific list, and every item on
//! it gates something:
//!
//! | Structure | Without it |
//! |---|---|
//! | Evil biome with orbs or hearts | no Eater of Worlds or Brain of Cthulhu, so no demonite, so no meteor |
//! | Dungeon | no Skeletron, and nothing behind him |
//! | Underworld with hellstone | no Wall of Flesh, so no hardmode |
//! | Demon altars | no hardmode ores, so nothing to fight the mechanical bosses with |
//! | Jungle temple | no Golem |
//! | Life crystals | a hundred hit points for the whole game |
//! | Chests | no starter weapons, no hooks, no boots |
//!
//! None of this is vanilla-identical and it does not try to be — see `docs/worldgen.md`. It is
//! built to be *complete and playable*, which is a different target and a reachable one.

use terrustia_proto::Tile;

use super::layout::{Band, Evil, Layout, Surface};
use super::place_object::place_object;
use super::rand::UnifiedRandom;
use super::tiles::{self, walls};
use crate::world::{Chest, World};

/// The Lihzahrd Altar. Using a Lihzahrd Power Cell on it is the only way to fight Golem.
///
/// Not a decoration: a real client will not let a player attempt the use-item interaction at all
/// without an active tile of this type nearby, so a temple with none anywhere in the world made
/// Golem permanently unreachable through ordinary play in every world this generator has ever
/// produced — found by a worldgen sizing pass that happened to read vanilla's separate
/// `LihzahrdAltar` generation pass (`WorldGen.cs:22131`) and noticed `structures::temple` never
/// calls anything like it. `terrustia-proto`'s own `tile_object` table confirms the shape
/// independently: entry 237 is a 3-wide, 2-tall object with origin `(1, 1)` — the bottom-middle
/// cell, which is what `place_object`'s anchor argument expects.
const LIHZAHRD_ALTAR: u16 = 237;

/// Hollow out a tile, leaving its wall and liquid behind so a cave looks like a cave.
///
/// The frame has to go with the block. An inactive tile that still carries a frame is
/// inconsistent state that nothing notices until it is saved — the format writes no frame for an
/// inactive tile, so it reads back different from what was written, and a round-trip check that
/// should be exact comes back five tiles short. That is exactly how this was found.
fn hollow(world: &mut World, x: i32, y: i32) {
    if !world.in_bounds(x, y) {
        return;
    }
    let was = world.tile(x, y);
    let mut tile = Tile::AIR;
    tile.wall = was.wall;
    tile.wall_color = was.wall_color;
    tile.liquid = was.liquid;
    tile.liquid_kind = was.liquid_kind;
    world.set_tile(x, y, tile);
}

/// Fill a tile with something, keeping whatever wall was there.
fn place(world: &mut World, x: i32, y: i32, block: u16) {
    if !world.in_bounds(x, y) {
        return;
    }
    let wall = world.tile(x, y).wall;
    let mut tile = Tile::block(block);
    tile.wall = wall;
    world.set_tile(x, y, tile);
}

/// Fill a tile *and* its wall.
fn place_with_wall(world: &mut World, x: i32, y: i32, block: u16, wall: u16) {
    if !world.in_bounds(x, y) {
        return;
    }
    let mut tile = Tile::block(block);
    tile.wall = wall;
    world.set_tile(x, y, tile);
}

/// Find a place to stand something of a given width, by falling until there is a floor.
///
/// Picking a random point and hoping it lands on a ledge almost never works — most of a world is
/// either solid or open air, and a ledge is the thin boundary between them. The first version of
/// the altar pass did exactly that and put down one altar where it wanted twelve. Falling from a
/// random point instead finds the floor beneath it, which is what a player would do.
///
/// Returns the row the thing's *feet* go on, with `height` rows of clear air above it.
fn find_ledge(
    world: &World,
    x: i32,
    from_y: i32,
    to_y: i32,
    width: i32,
    height: i32,
) -> Option<i32> {
    let mut y = from_y;
    while y < to_y {
        let floored = (0..width).all(|dx| world.tile(x + dx, y + 1).is_active());
        let clear =
            (0..width).all(|dx| (0..height).all(|dy| !world.tile(x + dx, y - dy).is_active()));
        if floored && clear {
            return Some(y);
        }
        y += 1;
    }
    None
}

/// A rough disc of hollow, which is the shape almost everything here is made of.
fn hollow_blob(world: &mut World, cx: i32, cy: i32, radius: i32, rand: &mut UnifiedRandom) {
    let wobble = rand.next_range(-1, 2);
    for x in cx - radius - 1..=cx + radius + 1 {
        for y in cy - radius - 1..=cy + radius + 1 {
            let (dx, dy) = (x - cx, y - cy);
            if dx * dx + dy * dy <= (radius + wobble) * (radius + wobble) {
                hollow(world, x, y);
            }
        }
    }
}

/// ...and the same in a material.
fn fill_blob(world: &mut World, cx: i32, cy: i32, radius: i32, block: u16) {
    for x in cx - radius..=cx + radius {
        for y in cy - radius..=cy + radius {
            let (dx, dy) = (x - cx, y - cy);
            if dx * dx + dy * dy <= radius * radius {
                place(world, x, y, block);
            }
        }
    }
}

/// `TileRunner` (`WorldGen.cs:77596-78046`), narrowed to the tile-*removing* form the cave passes
/// use (`type < 0`).
///
/// This is the whole mechanism behind vanilla's underground: a blob whose radius tapers linearly
/// from `strength` to nothing over `steps`, walked along a velocity that is itself a bounded random
/// walk. A runner therefore digs a self-terminating tube that narrows to a point, which is what
/// makes vanilla's caves a scatter of individually-bounded pockets rather than one network.
///
/// Deliberately narrowed, each an unreachable branch on the path this project's own `caves()`
/// takes rather than a behaviour dropped:
///
/// * The `drunkWorldGen`/`remixWorldGen`/`getGoodWorldGen`/`notTheBees` strength and step
///   perturbations (`WorldGen.cs:77678-77694`, `77720-77742`, `77906`, `77990`). Only
///   [`SecretSeeds::no_traps`][nt] is wired to a generation difference in this project, and none of
///   those seeds reaches here.
/// * `addTile`/`overRide`/`ignoreTileType`/the whole `type >= 0` half. Every caller below carves.
/// * The `GenVars.mudWall` wall-placing branch (`WorldGen.cs:77778-77792`): that flag is set only
///   for the duration of vanilla's `JunglePass`, which this project does not run through a runner.
/// * The `type == -2` liquid fill (`WorldGen.cs:77797-77816`). It sets `liquid`/`lava` on tiles it
///   is about to deactivate anyway, so it changes *what is in* a cave and never its shape; it needs
///   `GenVars.waterLine`/`lavaLine` (`TerrainPass.cs:213-214`), which this project's `Layout` has
///   no equivalent of. Named here rather than silently dropped: the callers below still roll their
///   `type = -2` draws so the random stream matches, they just carve dry.
/// * The `num > 50.0` extra-step ladder (`WorldGen.cs:77907-77988`). No caller here passes a
///   strength above 25, so `num` never reaches 50 and the whole ladder is dead.
/// * `Main.tileCut` in the frame-important skip. Nothing frame-important or cuttable exists yet
///   when `caves()` runs, since `terrain::fill` places only plain blocks; the check is transcribed
///   anyway because it is one term.
///
/// [nt]: super::secret_seed::SecretSeeds::no_traps
#[allow(clippy::too_many_arguments)]
fn tile_runner(
    world: &mut World,
    x: i32,
    y: i32,
    strength: f64,
    steps: i32,
    speed: Option<(f64, f64)>,
    no_y_change: bool,
    rand: &mut UnifiedRandom,
) {
    let mut num = strength;
    let mut num2 = f64::from(steps);
    let (mut px, mut py) = (f64::from(x), f64::from(y));
    // `val2.X`/`val2.Y` (`WorldGen.cs:77701-77709`): a random unit-ish drift unless the caller
    // names one.
    let (mut vx, mut vy) = speed.unwrap_or((
        f64::from(rand.next_range(-10, 11)) * 0.1,
        f64::from(rand.next_range(-10, 11)) * 0.1,
    ));

    while num > 0.0 && num2 > 0.0 {
        // `num = strength * (num2 / steps)` — the taper. Everything about the shape follows.
        num = strength * (num2 / f64::from(steps));
        num2 -= 1.0;
        let x0 = ((px - num * 0.5) as i32).max(1);
        let x1 = ((px + num * 0.5) as i32).min(world.width() - 1);
        let y0 = ((py - num * 0.5) as i32).max(1);
        let y1 = ((py + num * 0.5) as i32).min(world.height() - 1);

        for k in x0..x1 {
            for l in y0..y1 {
                let tile = world.tile(k, l);
                if tile.is_active() && terrustia_proto::tile_sets::frame_important(tile.block) {
                    continue;
                }
                // The diamond, jittered per tile so the edge is ragged rather than drawn. The
                // draw is inside the test in vanilla too, so it is consumed for every tile in the
                // box, not only the ones that pass.
                let jitter = 1.0 + f64::from(rand.next_range(-10, 11)) * 0.015;
                if (f64::from(k) - px).abs() + (f64::from(l) - py).abs() >= strength * 0.5 * jitter
                {
                    continue;
                }
                // `if (Main.tile[k, l].active() && Main.tile[k, l].type == 53) continue;`
                // (`WorldGen.cs:77794-77796`): a runner never digs through sand.
                if tile.is_active() && tile.block == tiles::SAND {
                    continue;
                }
                hollow(world, k, l);
            }
        }

        px += vx;
        py += vy;
        vx = (vx + f64::from(rand.next_range(-10, 11)) * 0.05).clamp(-1.0, 1.0);
        if !no_y_change {
            vy = (vy + f64::from(rand.next_range(-10, 11)) * 0.05).clamp(-1.0, 1.0);
        }
    }
}

/// `digTunnel` (`WorldGen.cs:80292-80355`): a fat, steered bore that [`caverer`] chains into a
/// cavern. Unlike [`tile_runner`] its radius does not taper to nothing, so it opens real rooms.
///
/// Returns where it ended, which is where the next link starts.
#[allow(clippy::too_many_arguments)]
fn dig_tunnel(
    world: &mut World,
    x: f64,
    y: f64,
    x_dir: f64,
    y_dir: f64,
    steps: i32,
    size: i32,
    wet: bool,
    rand: &mut UnifiedRandom,
) -> (f64, f64) {
    let mut num5 = f64::from(size);
    let mut num = x.clamp(num5 + 1.0, f64::from(world.width()) - num5 - 1.0);
    let mut num2 = y.clamp(num5 + 1.0, f64::from(world.height()) - num5 - 1.0);
    let (mut num3, mut num4) = (0.0f64, 0.0f64);

    for _ in 0..steps {
        let mut j = (num - num5) as i32;
        while f64::from(j) <= num + num5 {
            let mut k = (num2 - num5) as i32;
            while f64::from(k) <= num2 + num5 {
                let edge = num5 * (1.0 + f64::from(rand.next_range(-10, 11)) * 0.005);
                if (f64::from(j) - num).abs() + (f64::from(k) - num2).abs() < edge
                    && world.in_bounds(j, k)
                {
                    hollow(world, j, k);
                    if wet {
                        let mut tile = world.tile(j, k);
                        tile.liquid = 255;
                        tile.liquid_kind = terrustia_proto::Liquid::Water;
                        world.set_tile(j, k, tile);
                    }
                }
                k += 1;
            }
            j += 1;
        }
        num5 += f64::from(rand.next_range(-50, 51)) * 0.03;
        num5 = num5.clamp(f64::from(size) * 0.6, f64::from(size * 2));
        num3 = (num3 + f64::from(rand.next_range(-20, 21)) * 0.01).clamp(-1.0, 1.0);
        num4 = (num4 + f64::from(rand.next_range(-20, 21)) * 0.01).clamp(-1.0, 1.0);
        num += (x_dir + num3) * 0.6;
        num2 += (y_dir + num4) * 0.6;
    }
    (num, num2)
}

/// `Caverer` (`WorldGen.cs:80188-80290`): the large-cavern half of vanilla's underground, rolled
/// 50/50 between a dry branching cavern and a flooded one.
fn caverer(world: &mut World, x: i32, y: i32, rand: &mut UnifiedRandom) {
    let dir = |rand: &mut UnifiedRandom| {
        let mut a = f64::from(rand.next_max(100)) * 0.01;
        let mut b = 1.0 - a;
        if rand.next_max(2) == 0 {
            a = -a;
        }
        if rand.next_max(2) == 0 {
            b = -b;
        }
        (a, b)
    };

    if rand.next_max(2) == 0 {
        // Branch 0: a chain of wide bores, each with a side spur ended by a runner-dug chamber.
        let links = rand.next_range(7, 9);
        let (mut dx, mut dy) = dir(rand);
        let (mut px, mut py) = (f64::from(x), f64::from(y));
        for _ in 0..links {
            let steps = rand.next_range(6, 20);
            let size = rand.next_range(4, 9);
            (px, py) = dig_tunnel(world, px, py, dx, dy, steps, size, false, rand);
            dx = (dx + f64::from(rand.next_range(-20, 21)) * 0.1).clamp(-1.5, 1.5);
            dy = (dy + f64::from(rand.next_range(-20, 21)) * 0.1).clamp(-1.5, 1.5);
            let (sx, sy) = dir(rand);
            let steps = rand.next_range(30, 50);
            let size = rand.next_range(3, 6);
            let (ex, ey) = dig_tunnel(world, px, py, sx, sy, steps, size, false, rand);
            let strength = f64::from(rand.next_range(10, 20));
            let runner_steps = rand.next_range(5, 10);
            tile_runner(
                world,
                ex as i32,
                ey as i32,
                strength,
                runner_steps,
                None,
                false,
                rand,
            );
        }
    } else {
        // Branch 1: one long flooded bore, which is where an underground lake comes from.
        let links = rand.next_range(15, 30);
        let (mut dx, mut dy) = dir(rand);
        let (mut px, mut py) = (f64::from(x), f64::from(y));
        for _ in 0..links {
            let steps = rand.next_range(5, 15);
            let size = rand.next_range(2, 6);
            (px, py) = dig_tunnel(world, px, py, dx, dy, steps, size, true, rand);
            dx = (dx + f64::from(rand.next_range(-20, 21)) * 0.1).clamp(-1.5, 1.5);
            dy = (dy + f64::from(rand.next_range(-20, 21)) * 0.1).clamp(-1.5, 1.5);
        }
    }
}

/// The caves, as vanilla actually digs them.
///
/// Four of vanilla's own passes, in its own order, all driven by [`tile_runner`]:
/// `SmallHoles` (`WorldGen.cs:12046-12105`), `DirtLayerCaves` (`12106-12146`), `RockLayerCaves`
/// (`12147-12202`) and the [`caverer`] tail of `SurfaceCaves` (`12295-12312`). Between them they
/// seed roughly sixteen thousand independent runners into a small world, which is the entire reason
/// vanilla's underground reads as a mix of isolated pockets and occasional large caverns: nothing
/// steers them towards each other, so most of what they dig never meets anything else.
///
/// This replaced a single wandering-tunnel carver of this project's own (190 tunnels on a small
/// world, each up to 588 tiles long, walked with a turning angle). Measured on three real worlds,
/// that carver left the deep band in 74 to 80 connected components with the largest holding 9 to 16
/// per cent of all open space, and only 4 to 11 components anywhere in the 50-to-300-tile window
/// `GemCaves` sites into. See `structures::cave_topology_measurement` for the instrument and the
/// numbers on both sides.
///
/// **Depth range narrowed at the bottom.** Vanilla seeds `SmallHoles` and `RockLayerCaves` down to
/// `Main.maxTilesY`, because its own `Underworld` pass (`WorldGen.cs:13709-13763`) later rewrites
/// every column from the ash ceiling down. This project's `underworld` only hollows blobs into ash
/// it never rebuilds, so a runner seeded down there would leave a hole vanilla does not keep;
/// `layout.underworld` stands in for `maxTilesY` in the seed ranges for that reason.
///
/// **`GenVars`' layer pairs collapse.** Vanilla tracks `worldSurfaceLow`/`worldSurfaceHigh` and
/// `rockLayerLow`/`rockLayerHigh` (`TerrainPass.cs:230-236`) because its layer boundaries follow
/// the terrain; this project's `Layout` has one flat line for each. `layout.rock` stands in for
/// `rockLayerHigh`, and `layout.surface - 25` for `worldSurfaceLow`/`worldSurfaceHigh` - the 25 is
/// vanilla's own offset between them (`Main.worldSurface = worldSurfaceHigh + 25`,
/// `TerrainPass.cs:206`), kept because the mid-world reroll below is a no-op if the two are equal.
pub fn caves(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) {
    let area = i64::from(layout.width) * i64::from(layout.height);
    let surface_high = layout.surface - 25;
    // `GenVars.smallHolesBeachAvoidance = beachSandRandomCenter + 20` (`WorldGen.cs:11231`).
    let beach_avoid = layout.ocean_left.to + 20;
    // Every seed range below is `Next(a, b)` with `a < b`; a world too small for that is not one
    // this generator carves at all.
    if layout.rock >= layout.underworld || surface_high >= layout.underworld {
        return;
    }

    // A seed the spawn area and the beaches must not get: vanilla rerolls until the point is
    // outside them (`WorldGen.cs:12074-12078`).
    let reroll = |x: i32, y: i32| {
        ((x < beach_avoid || x > layout.width - beach_avoid) && y < surface_high)
            || (f64::from(x) > f64::from(layout.width) * 0.45
                && f64::from(x) < f64::from(layout.width) * 0.55
                && y < layout.surface)
    };

    // `SmallHoles`: two runners per iteration, a tiny one and a fat short one. This is the pass
    // that makes most of vanilla's isolated pockets.
    let small_holes = (area as f64 * 0.0015) as i32;
    for _ in 0..small_holes {
        // `type = -2` one time in five: vanilla would flood this hole. See `tile_runner`'s own
        // note; the draw is kept so the stream matches.
        let _wet = rand.next_max(5) == 0;
        for (min_strength, max_strength, min_steps, max_steps) in [(2, 5, 2, 20), (8, 15, 7, 30)] {
            let mut x = rand.next_range(0, layout.width);
            let mut y = rand.next_range(surface_high, layout.underworld);
            while reroll(x, y) {
                x = rand.next_range(0, layout.width);
                y = rand.next_range(surface_high, layout.underworld);
            }
            let strength = f64::from(rand.next_range(min_strength, max_strength));
            let steps = rand.next_range(min_steps, max_steps);
            tile_runner(world, x, y, strength, steps, None, false, rand);
        }
    }

    // `DirtLayerCaves`: longer runners through the dirt layer.
    let dirt_layer = (area as f64 * 3E-05) as i32;
    for _ in 0..dirt_layer {
        let _wet = rand.next_max(6) == 0;
        let mut x = rand.next_range(0, layout.width);
        let mut y = rand.next_range(surface_high, layout.rock + 1);
        while reroll(x, y) {
            x = rand.next_range(0, layout.width);
            y = rand.next_range(surface_high, layout.rock + 1);
        }
        let strength = f64::from(rand.next_range(5, 15));
        let steps = rand.next_range(30, 200);
        tile_runner(world, x, y, strength, steps, None, false, rand);
    }

    // `RockLayerCaves`: the cavern layer's own, fatter and much longer. No reroll in vanilla.
    let rock_layer = (area as f64 * 0.00013) as i32;
    for _ in 0..rock_layer {
        let _wet = rand.next_max(10) == 0;
        let strength = f64::from(rand.next_range(6, 20));
        let steps = rand.next_range(50, 300);
        let x = rand.next_range(0, layout.width);
        let y = rand.next_range(layout.rock, layout.underworld);
        tile_runner(world, x, y, strength, steps, None, false, rand);
    }

    // The `Caverer` tail of `SurfaceCaves`: five large caverns on a small world.
    let caverns = (5.0 * f64::from(layout.width) / 4200.0) as i32;
    let top = layout.rock;
    let bottom = layout.underworld - 400;
    if bottom > top && layout.width > beach_avoid * 2 {
        for _ in 0..caverns {
            let x = rand.next_range(beach_avoid, layout.width - beach_avoid);
            let y = rand.next_range(top, bottom);
            caverer(world, x, y, rand);
        }
    }
}

/// Ore, in the depth bands the game puts each metal in.
///
/// The bands are what make progression work: copper and iron near the surface so a new character
/// can find a pickaxe's worth, gold and silver deeper so they are worth going down for.
pub fn ores(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) {
    // (ore, from, to, how many per thousand columns, vein size)
    let bands: [(u16, i32, i32, i32, i32); 4] = [
        (tiles::COPPER, layout.surface, layout.underworld, 34, 5),
        (tiles::IRON, layout.surface + 20, layout.underworld, 28, 5),
        (tiles::SILVER, layout.rock, layout.underworld, 20, 4),
        (tiles::GOLD, layout.rock + 60, layout.underworld, 14, 4),
    ];
    for (ore, from, to, density, size) in bands {
        if to <= from + 4 {
            continue;
        }
        let veins = layout.width * density / 1000;
        for _ in 0..veins {
            let x = rand.next_range(10, layout.width - 10);
            let y = rand.next_range(from, to);
            // Only into stone: an ore vein hanging in a cave is not a vein.
            if world.tile(x, y).block != tiles::STONE {
                continue;
            }
            fill_blob(world, x, y, rand.next_range(1, size), ore);
        }
    }

    // Gems, which are rarer and only deep.
    let gems = [
        tiles::AMETHYST,
        tiles::TOPAZ,
        tiles::SAPPHIRE,
        tiles::EMERALD,
        tiles::RUBY,
        tiles::DIAMOND,
    ];
    for _ in 0..layout.width / 22 {
        let x = rand.next_range(10, layout.width - 10);
        let from = layout.rock + 100;
        let to = layout.underworld.max(from + 8);
        let y = rand.next_range(from, to);
        if world.tile(x, y).block != tiles::STONE {
            continue;
        }
        let gem = gems[rand.next_max(gems.len() as i32) as usize];
        fill_blob(world, x, y, rand.next_range(1, 3), gem);
    }
}

/// The evil biome's chasms, and the orbs or hearts at the bottom of them.
///
/// This is the one structure whose *contents* are progression rather than decoration: three orbs
/// smashed is the Eater of Worlds or the Brain of Cthulhu, and that is the whole of the first
/// act's gating.
/// `override_evil` replaces the layout's own evil and band for this call. It exists for Drunk
/// World, which puts Corruption on one half of the world and Crimson on the other
/// (`WorldGen.cs:2052-2062`, keyed on `GenVars.crimsonLeft`) - two calls, one per side, rather
/// than one biome with a mixed identity.
pub fn evil_chasms(
    world: &mut World,
    layout: &Layout,
    heights: &[i32],
    rand: &mut UnifiedRandom,
    override_evil: Option<(Evil, Band)>,
) -> usize {
    let (evil, evil_band) = override_evil.unwrap_or((layout.evil, layout.evil_band));
    let orb_tile = tiles::SHADOW_ORB;
    let chasms = 3 + rand.next_max(3);
    let mut orbs = 0;
    // `GenVars.ebonStoneWall`, 3 in a corruption world and 83 in a crimson one
    // (`WorldGen.cs:8294`, `:11330`). `ChasmRunner` writes it behind the chasm it digs
    // (`WorldGen.cs:76866`), so this pass has to place it rather than inherit it: `terrain::fill`
    // no longer walls the cavern layer at all (see `terrain::wall_for`), which is right for a cave
    // and wrong for a chasm.
    let evil_wall = if evil == Evil::Crimson {
        walls::CRIMSTONE
    } else {
        walls::EBONSTONE
    };

    for nth in 0..chasms {
        // Spread the chasms across the band rather than stacking them.
        let band = evil_band;
        let step = band.width() / (chasms + 1).max(1);
        let x = band.from + step * (nth + 1) + rand.next_range(-step / 3, step / 3 + 1);
        if x <= 2 || x >= layout.width - 2 {
            continue;
        }
        let top = heights[x.clamp(0, layout.width - 1) as usize];
        let bottom = (top + rand.next_range(90, 190)).min(layout.underworld - 40);

        // A chasm is a narrow shaft that widens as it goes down.
        let mut cx = x;
        for y in top..bottom {
            let along = f64::from(y - top) / f64::from((bottom - top).max(1));
            let half = (2.0 + along * 5.0) as i32;
            for dx in -half..=half {
                hollow(world, cx + dx, y);
                // `if (num13 > j + genRand.Next(3, 20)) tile.wall = ebonStoneWall;`
                // (`WorldGen.cs:76864-76867`): the top few rows stay open to the sky, and the draw
                // is rerolled per tile, so the line where the wall starts is ragged.
                if y > top + rand.next_range(3, 20) && world.in_bounds(cx + dx, y) {
                    let mut tile = world.tile(cx + dx, y);
                    tile.wall = evil_wall;
                    world.set_tile(cx + dx, y, tile);
                }
            }
            // The shaft wanders, so it is not a drilled hole.
            if rand.next_max(7) == 0 {
                cx += rand.next_range(-1, 2);
            }
        }

        // A pocket at the bottom, with an orb in it.
        hollow_blob(world, cx, bottom, 6, rand);
        // ...and the same wall behind it. Walling every open tile in the blob's own bounding box
        // is equivalent to walling the blob: the only other thing open in that box is the shaft
        // this pass just dug, which is walled already.
        for bx in cx - 8..=cx + 8 {
            for by in bottom - 8..=bottom + 8 {
                if world.in_bounds(bx, by) && !world.tile(bx, by).is_active() {
                    let mut tile = world.tile(bx, by);
                    tile.wall = evil_wall;
                    world.set_tile(bx, by, tile);
                }
            }
        }
        let orb_y = bottom + 2;
        // Frames say which half of the sheet the sprite comes from, and a crimson heart is the
        // right-hand half — `frameX >= 36`, which is what the break handler reads to decide
        // which boss to wake. Getting it wrong gives a corruption world crimson hearts.
        let frame_x: i16 = if evil == Evil::Crimson { 36 } else { 0 };
        for (dx, dy) in [(0i32, 0i32), (1, 0), (0, 1), (1, 1)] {
            let mut tile = Tile::framed(orb_tile, frame_x + (dx as i16) * 18, (dy as i16) * 18);
            // Was hardcoded to Ebonstone, so a crimson world's heart room carried a corruption
            // wall. `GenVars.ebonStoneWall` is the crimson one in a crimson world.
            tile.wall = evil_wall;
            world.set_tile(cx + dx, orb_y + dy, tile);
        }
        orbs += 1;
    }
    orbs
}

/// The demon altars: where hardmode ore comes from.
///
/// Scattered through the evil biome and the caverns. Without them a world stops dead at the Wall
/// of Flesh, because smashing them is the only source of cobalt, mythril and adamantite.
pub fn altars(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) -> usize {
    let wanted = (layout.width / 120).max(12) as usize;
    let mut placed = 0usize;
    let frame_base: i16 = if layout.evil == Evil::Crimson { 54 } else { 0 };

    for _ in 0..wanted * 40 {
        if placed >= wanted {
            break;
        }
        // Two thirds in the evil band, the rest anywhere underground — which is where a player
        // who has cleared their own biome goes looking for the last few.
        let x = if rand.next_max(3) > 0 && layout.evil_band.width() > 8 {
            rand.next_range(layout.evil_band.from + 3, layout.evil_band.to - 3)
        } else {
            rand.next_range(20, layout.width - 20)
        };
        let from = rand.next_range(layout.surface + 30, layout.underworld - 40);
        // An altar is three wide and two tall, and needs a floor under all three.
        let Some(y) = find_ledge(world, x, from, layout.underworld - 20, 3, 2) else {
            continue;
        };

        for dx in 0..3i32 {
            for dy in 0..2i32 {
                let wall = world.tile(x + dx, y - 1 + dy).wall;
                let mut tile = Tile::framed(
                    tiles::DEMON_ALTAR,
                    frame_base + (dx as i16) * 18,
                    (dy as i16) * 18,
                );
                tile.wall = wall;
                world.set_tile(x + dx, y - 1 + dy, tile);
            }
        }
        placed += 1;
    }
    placed
}

/// Life crystals, which are the only way past a hundred hit points.
pub fn life_crystals(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) -> usize {
    let wanted = (layout.width / 90).max(15) as usize;
    let mut placed = 0usize;
    for _ in 0..wanted * 60 {
        if placed >= wanted {
            break;
        }
        let x = rand.next_range(20, layout.width - 20);
        let from = rand.next_range(layout.rock, layout.underworld - 40);
        // Two wide and two tall, standing on something.
        let Some(feet) = find_ledge(world, x, from, layout.underworld - 20, 2, 2) else {
            continue;
        };
        for dx in 0..2i32 {
            for dy in 0..2i32 {
                let y = feet - 1 + dy;
                let wall = world.tile(x + dx, y).wall;
                let mut tile = Tile::framed(tiles::HEART, (dx as i16) * 18, (dy as i16) * 18);
                tile.wall = wall;
                world.set_tile(x + dx, y, tile);
            }
        }
        placed += 1;
    }
    placed
}

/// The dungeon: a warren of brick rooms behind the door Skeletron guards.
///
/// Deliberately simple compared with the game's — rooms on a grid joined by corridors — because
/// what the dungeon has to *be* for a playthrough is a large walled space full of dungeon brick,
/// with an entrance at the surface and chests inside. Its shape is atmosphere; its existence is
/// progression.
pub fn dungeon(world: &mut World, layout: &Layout, heights: &[i32], rand: &mut UnifiedRandom) {
    let brick = match rand.next_max(3) {
        0 => tiles::BLUE_DUNGEON_BRICK,
        1 => tiles::GREEN_DUNGEON_BRICK,
        _ => tiles::PINK_DUNGEON_BRICK,
    };
    let wall = match brick {
        tiles::BLUE_DUNGEON_BRICK => walls::BLUE_DUNGEON,
        tiles::GREEN_DUNGEON_BRICK => walls::GREEN_DUNGEON,
        _ => walls::PINK_DUNGEON,
    };

    let x = layout.dungeon_x.clamp(80, layout.width - 80);
    let entrance_y = heights[x as usize];
    let bottom = (layout.rock + 260).min(layout.underworld - 60);

    // A shaft from the surface down to the rooms, so the dungeon is reachable on foot.
    for y in entrance_y..bottom {
        for dx in -6..=6i32 {
            let edge = dx.abs() > 4;
            if edge {
                place_with_wall(world, x + dx, y, brick, wall);
            } else {
                place_with_wall(world, x + dx, y, 0, wall);
                hollow(world, x + dx, y);
            }
        }
    }

    // Rooms, spread either side of the shaft and down.
    //
    // `dungeon_loot_style` is vanilla's own `dungeonLootStyle`: one counter shared across every
    // chest this whole dungeon places, advanced only when a chest is actually placed
    // (`DungeonUtils.cs:379-383`), not reseeded per room.
    let rooms = 14 + rand.next_max(10);
    let mut dungeon_loot_style: u8 = 0;
    for _ in 0..rooms {
        let rw = rand.next_range(14, 30);
        let rh = rand.next_range(9, 16);
        let rx = x + rand.next_range(-90, 91);
        let ry = rand.next_range(entrance_y + 30, bottom);
        if rx - rw < 10 || rx + rw > layout.width - 10 {
            continue;
        }

        for cx in rx - rw..=rx + rw {
            for cy in ry - rh..=ry + rh {
                let edge = cx == rx - rw || cx == rx + rw || cy == ry - rh || cy == ry + rh;
                if edge {
                    place_with_wall(world, cx, cy, brick, wall);
                } else {
                    place_with_wall(world, cx, cy, 0, wall);
                    hollow(world, cx, cy);
                }
            }
        }

        // A corridor back to the shaft, so no room is sealed off.
        let corridor_y = ry;
        let (from, to) = if rx < x { (rx, x) } else { (x, rx) };
        for cx in from..=to {
            for cy in corridor_y - 2..=corridor_y + 2 {
                place_with_wall(world, cx, cy, 0, wall);
                hollow(world, cx, cy);
            }
        }

        // A chest in about half of them, with something worth the walk.
        if rand.next_max(2) == 0 {
            let cx = rx + rand.next_range(-rw + 2, rw - 2);
            let cy = ry + rh - 1;
            let (loot, style) = dungeon_loot(rand, layout, cy, dungeon_loot_style);
            if add_chest_styled(world, cx, cy, loot, rand, style) {
                // Only a chest that actually landed advances the round-robin — the same
                // `if (num3 && styleData.Style == 0) dungeonLootStyle++;` gate vanilla's own
                // caller applies (`DungeonUtils.cs:380-383`). `wrapping_add` rather than the
                // explicit ">=8 reset" vanilla stores: both pick the same slot via `% 8`, and a
                // dungeon never places enough chests to make the difference visible short of
                // overflowing a `u8` entirely.
                dungeon_loot_style = dungeon_loot_style.wrapping_add(1);
            }
        }
    }
}

/// The jungle temple: lihzahrd brick, and nothing gets in until Plantera falls.
pub fn temple(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) {
    let (tx, ty) = layout.temple;
    let half_w = rand.next_range(34, 55);
    let half_h = rand.next_range(20, 32);

    for x in tx - half_w..=tx + half_w {
        for y in ty - half_h..=ty + half_h {
            // A thick shell, since the point of the temple is that it cannot be dug into.
            let edge = x < tx - half_w + 3
                || x > tx + half_w - 3
                || y < ty - half_h + 3
                || y > ty + half_h - 3;
            if edge {
                place_with_wall(world, x, y, tiles::LIHZAHRD_BRICK, walls::LIHZAHRD_BRICK);
            } else {
                place_with_wall(world, x, y, 0, walls::LIHZAHRD_BRICK);
                hollow(world, x, y);
            }
        }
    }

    // Inner walls, so it is a temple rather than a box.
    let rooms = rand.next_range(3, 6);
    for nth in 1..=rooms {
        let at = tx - half_w + (half_w * 2 / (rooms + 1)) * nth;
        for y in ty - half_h + 3..=ty + half_h - 3 {
            // A gap in each, so every room is reachable.
            if (y - (ty + half_h - 6)).abs() > 3 {
                place_with_wall(world, at, y, tiles::LIHZAHRD_BRICK, walls::LIHZAHRD_BRICK);
            }
        }
    }

    // The altar. It stands on the temple's own floor — the last hollow row before the shell's
    // bottom edge — so `place_object`'s footprint-and-floor check passes against the brick the
    // edge loop above already laid down. That row sits inside every inner room's doorway gap
    // (`(y - (ty+half_h-6)).abs() <= 3`, the same test the wall loop above uses to *skip* a wall),
    // so no inner wall can be standing where the altar needs to go, in any room it might land in.
    //
    // Centred first, since the interior is symmetric and centre is clear of both the outer shell
    // and every inner wall column by construction; a few fallback offsets cover the rare case
    // where a small `half_w` roll puts the centre awkwardly close to a wall column.
    let altar_y = ty + half_h - 3;
    let mut altar_placed = false;
    for dx in [0, -4, 4, -8, 8, -12, 12] {
        if place_object(world, tx + dx, altar_y, LIHZAHRD_ALTAR, 0, -1) {
            altar_placed = true;
            break;
        }
    }
    debug_assert!(
        altar_placed,
        "the jungle temple must always get an altar, or Golem is unreachable in this world"
    );
}

/// Hellstone and lava, which are what the underworld is for.
pub fn underworld(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) {
    // The band has to be worked out rather than assumed: a short world can leave less room under
    // the underworld's top than the constants want, and the generator throws on a backwards range
    // rather than quietly returning nonsense.
    let top = layout.underworld + 4;
    let floor = (layout.height - 12).max(top + 8);
    // Open it out: the underworld is a cavern, not solid ash.
    for _ in 0..layout.width / 4 {
        let x = rand.next_range(10, layout.width - 10);
        let y = rand.next_range(top, floor);
        hollow_blob(world, x, y, rand.next_range(4, 12), rand);
    }
    // A floor of lava across most of the bottom, which is what makes crossing it a problem.
    let lava_line = ((layout.height - 32).max(top + 4)).min(layout.height - 4);
    for x in 0..layout.width {
        for y in lava_line..(layout.height - 2) {
            let mut tile = world.tile(x, y);
            if !tile.is_active() {
                tile.liquid = 255;
                tile.liquid_kind = terrustia_proto::Liquid::Lava;
                world.set_tile(x, y, tile);
            }
        }
    }
    // Hellstone, which is the only thing down here worth the trip.
    for _ in 0..layout.width / 6 {
        let x = rand.next_range(10, layout.width - 10);
        let y = rand.next_range(top, floor);
        if world.tile(x, y).block != tiles::ASH {
            continue;
        }
        fill_blob(world, x, y, rand.next_range(2, 5), tiles::HELLSTONE);
    }
}

/// A bee hive in the jungle, with the larva that wakes the Queen.
pub fn hive(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) -> bool {
    for _ in 0..80 {
        let x = rand.next_range(
            layout.jungle.from + 30,
            (layout.jungle.to - 30).max(layout.jungle.from + 31),
        );
        let from = layout.rock + 40;
        let to = (layout.rock + 200)
            .min(layout.underworld - 40)
            .max(from + 8);
        let y = rand.next_range(from, to);
        if world.tile(x, y).block != tiles::MUD {
            continue;
        }
        let radius = rand.next_range(11, 18);
        fill_blob(world, x, y, radius, tiles::HIVE);
        hollow_blob(world, x, y, radius - 3, rand);
        // The larva, which is the only way to call the Queen without a summon item.
        let floor = y + radius - 4;
        for dx in 0..2i32 {
            for dy in 0..2i32 {
                let mut tile = Tile::framed(tiles::LARVA, (dx as i16) * 18, (dy as i16) * 18);
                tile.wall = walls::JUNGLE;
                world.set_tile(x + dx, floor + dy, tile);
            }
        }
        return true;
    }
    false
}

/// Chests, scattered through the caverns with tiered loot.
///
/// The signature item is vanilla's own where the biome and depth match one it treats specially —
/// jungle and underground desert, both transcribed from `AddBuriedChest`'s own item selection
/// (`WorldGen.cs:36429-36447` for the jungle roll, `:36404-36420` for the desert one). Everywhere
/// else keeps the existing depth-tiered table: vanilla's own selection there is not self-contained
/// in this function — the underworld's rotates through a shuffled array set up during world
/// generation elsewhere, and this generator does not model underground-desert as a region distinct
/// from a plain deep desert column, so a biome-tagged column just below `rock` is treated as one.
pub fn chests(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) -> usize {
    let wanted = (layout.width / 14).max(60) as usize;
    let mut placed = 0usize;
    for _ in 0..wanted * 30 {
        if placed >= wanted {
            break;
        }
        let x = rand.next_range(20, layout.width - 20);
        let from = rand.next_range(layout.surface + 10, layout.underworld - 40);
        let Some(feet) = find_ledge(world, x, from, layout.underworld - 20, 2, 2) else {
            continue;
        };
        let loot = biome_chest_loot(layout, x, feet, rand)
            .unwrap_or_else(|| cavern_loot(layout, feet, rand));
        if add_chest(world, x, feet, loot, rand) {
            placed += 1;
        }
    }
    placed
}

/// Put a plain (style 0) chest down if there is room and a floor for it.
///
/// `pub(crate)`: `jungle_shrines.rs` reuses this directly for the chest a shrine's own floor gap
/// is built to hold, rather than re-deriving the same clearance/floor check a second time.
pub(crate) fn add_chest(
    world: &mut World,
    x: i32,
    y: i32,
    items: Vec<terrustia_proto::ItemStack>,
    rand: &mut UnifiedRandom,
) -> bool {
    add_chest_styled(world, x, y, items, rand, 0)
}

/// [`add_chest`], but able to frame the tile at a chosen chest *style* rather than always the
/// plain one — needed for a dungeon chest, which vanilla places locked (style 2) far more often
/// than not (see [`dungeon_loot`]'s own doc).
///
/// A chest tile's style lives entirely in its frame: each style is one 36-pixel-wide (two-tile)
/// column of the sprite sheet, so `frame_x = style * 36 + local_dx * 18` — confirmed against
/// `on_lock`'s own `frame_x / 36` read of an existing chest's style, and against
/// `terrustia_proto::locks`' style numbers, where 2 is exactly the locked dungeon/gold chest
/// `unlock_shift` already knows how to open with a key.
pub(crate) fn add_chest_styled(
    world: &mut World,
    x: i32,
    y: i32,
    items: Vec<terrustia_proto::ItemStack>,
    _rand: &mut UnifiedRandom,
    style: u16,
) -> bool {
    if !world.in_bounds(x, y) || !world.in_bounds(x + 1, y + 1) {
        return false;
    }
    // Two by two of air with solid ground beneath.
    let clear = (0..2).all(|dx| (0..2).all(|dy| !world.tile(x + dx, y - dy).is_active()));
    let floored = (0..2).all(|dx| world.tile(x + dx, y + 1).is_active());
    if !clear || !floored {
        return false;
    }
    if world.chest_at(x as i16, (y - 1) as i16).is_some() {
        return false;
    }

    let style_frame = style * 36;
    for dx in 0..2i32 {
        for dy in 0..2i32 {
            let wall = world.tile(x + dx, y - 1 + dy).wall;
            let frame_x = style_frame as i16 + (dx as i16) * 18;
            let mut tile = Tile::framed(tiles::CHEST, frame_x, (dy as i16) * 18);
            tile.wall = wall;
            world.set_tile(x + dx, y - 1 + dy, tile);
        }
    }
    let mut chest = Chest::empty_at(x as i16, (y - 1) as i16);
    for (slot, item) in items.into_iter().enumerate() {
        if let Some(cell) = chest.items.get_mut(slot) {
            *cell = item;
        }
    }
    world.add_chest(chest);
    true
}

/// Vanilla's real jungle-chest and underground-desert-chest signature items, if this site is
/// biome-tagged for one of them. `None` for everywhere else, so the caller falls back to the
/// existing depth-tiered table.
///
/// Transcribed from `AddBuriedChest`, which does not pick these by a clean per-style switch —
/// it derives them from a chain of boolean flags gated on the chest's site. The two item lists
/// below are exactly its `flag2` (jungle) and the desert-tool block near the top of the function,
/// item IDs and roll odds unchanged.
///
/// `pub(crate)`: `jungle_shrines.rs` reuses the jungle branch for its own chest, in place of
/// vanilla's separate `GetNextJungleChestItem` (a non-repeating cycle through the same signature
/// list) — a real, disclosed simplification: both draw from the same underlying jungle item set,
/// and duplicating a second selection mechanism for that difference alone was not worth it.
pub(crate) fn biome_chest_loot(
    layout: &Layout,
    x: i32,
    y: i32,
    rand: &mut UnifiedRandom,
) -> Option<Vec<terrustia_proto::ItemStack>> {
    use terrustia_proto::ItemStack;

    if layout.jungle.contains(x) {
        // The array `{ 670, 724, 950, 1319, 987, 1579, 6153 }` this branch used to draw from
        // (WorldGen.cs:36464) is the *ice/snow biome* chest table — that call is gated on tile
        // 147/161/162/197 (ice/snow variants) a few lines above, not on the jungle at all. The
        // real jungle signature item is `GetNextJungleChestItem` (`WorldGen.cs:10146-10173`): a
        // non-repeating cycle through Feral Claws(211)/Anklet of the Wind(212)/Staff of
        // Regrowth(213)/Boomstick(964), with a further reroll — 1/15 to Fiberglass Fishing
        // Pole(2292), else 1/20 to Flower Boots(3017). This function has no persistent
        // `JungleItemCount` to cycle (each call is a fresh site, not a shared per-world counter),
        // so the cycle is approximated with a uniform draw across the same four items — a real,
        // disclosed narrowing of `GetNextJungleChestItem`'s own non-repeating guarantee, not a
        // wrong item set.
        const JUNGLE: [i32; 4] = [211, 212, 213, 964];
        let mut signature = JUNGLE[rand.next_max(JUNGLE.len() as i32) as usize];
        if rand.next_max(15) == 0 {
            signature = 2292;
        } else if rand.next_max(20) == 0 {
            signature = 3017;
        }
        let mut items = vec![ItemStack::new(signature, 1, 0)];
        items.push(ItemStack::new(8, rand.next_range(10, 30) as i16, 0));
        items.push(ItemStack::new(71, rand.next_range(10, 99) as i16, 0));
        return Some(items);
    }

    // Vanilla's underground desert is a region distinct from a plain desert column at depth —
    // sized against `GenVars.UndergroundDesertLocation`, which this generator does not carve as
    // its own shape. A desert-biome column once it is below the rock layer is treated as close
    // enough: it is what the surface desert becomes once you dig, which is the case this table
    // exists for.
    if layout.desert.contains(x) && y > layout.rock {
        // WorldGen.cs:36404 `num10 = Utils.SelectRandom(genRand, new short[4] { 4056, 4055, 4262,
        // 4263 })`. Vanilla has a second, rarer four-item set for the shallow half of the desert
        // hive band specifically; that band is not modelled here, so only the common set is used.
        const DESERT: [i32; 4] = [4056, 4055, 4262, 4263];
        let signature = DESERT[rand.next_max(DESERT.len() as i32) as usize];
        let mut items = vec![ItemStack::new(signature, 1, 0)];
        items.push(ItemStack::new(8, rand.next_range(10, 30) as i16, 0));
        items.push(ItemStack::new(71, rand.next_range(10, 99) as i16, 0));
        return Some(items);
    }

    None
}

/// What a cavern chest holds. Deeper is better, which is the whole of the reward curve.
///
/// `pub(crate)`: `underground_cabins.rs` reuses this directly for the one chest each cabin holds,
/// rather than re-deriving the same depth-tiered table a second time.
pub(crate) fn cavern_loot(
    layout: &Layout,
    y: i32,
    rand: &mut UnifiedRandom,
) -> Vec<terrustia_proto::ItemStack> {
    use terrustia_proto::ItemStack;
    // The signature item, which is what a player opens a chest hoping for.
    //
    // Every id below used to disagree with its own comment (`ItemID.cs` says 965 is Rope, not
    // Shoe Spikes; 930 is Flare Gun, not Cloud in a Bottle; 158 is Lucky Horseshoe, not Hermes
    // Boots; 963 is Black Belt — a post-Plantera item, wrong for an ungated cavern chest — not
    // Bandage; 997 is Extractinator, not Magic Mirror; and in `deep`, 119 is Flamarang, 155 is
    // Muramasa, 1300 is Rifle Scope — hardmode — 281 is Blowpipe, and 3068 is Cordage Guide, a
    // guide book, not a chest reward). Replaced with each id the comment actually meant, all
    // verified against `ItemID.cs` directly, keeping the same shallow/deep split.
    let shallow = [
        49,  // Band of Regeneration
        965, // Rope
        930, // Flare Gun
        158, // Lucky Horseshoe
        975, // Shoe Spikes
        50,  // Magic Mirror
    ];
    let deep = [
        55,  // Enchanted Boomerang
        53,  // Cloud in a Bottle
        54,  // Hermes Boots
        296, // Spelunker Potion
        157, // Aqua Scepter
        997, // Extractinator
    ];
    let pool: &[i32] = if y > layout.rock + 120 {
        &deep
    } else {
        &shallow
    };
    let signature = pool[rand.next_max(pool.len() as i32) as usize];

    let mut items = vec![ItemStack::new(signature, 1, 0)];
    // Torches and a rope are what actually make a cave chest useful.
    items.push(ItemStack::new(8, rand.next_range(10, 30) as i16, 0));
    items.push(ItemStack::new(965, rand.next_range(20, 60) as i16, 0));
    // A little money.
    items.push(ItemStack::new(71, rand.next_range(10, 99) as i16, 0));
    items
}

/// ...and what a dungeon chest holds, which is a tier above — and, unlike every other chest table
/// in this file, not random at all.
///
/// A prior fix already replaced this table's original five mislabeled ids (327/328/329/330/676
/// wrongly commented as Muramasa/Cobalt Shield/Aqua Scepter/Blue Moon/Magnet Sphere) with a
/// classic weapon family, but the family itself was still fabricated: five items picked with
/// `next_max`, when real vanilla's own table has **eight** items chosen by a **deterministic
/// round-robin**, not a per-chest die roll — `WorldGen.GetDungeonLootAndChestStyle`
/// (`WorldGen.cs:36189-36237`), wired to a "style-0" regular dungeon chest by
/// `DungeonUtils.GenerateDungeonRegularChest` (`DungeonUtils.cs:334-385`).
///
/// `dungeonLootStyle` is a persistent counter, one shared value for the entire dungeon (not
/// reseeded per chest), advanced by the caller once per chest *actually placed* and wrapped at 8
/// (`DungeonUtils.cs:380-383`; the wrap in `GetDungeonLootAndChestStyle` itself resets the stored
/// counter to 0 at >=8 rather than taking a modulus, but the two are the same sequence of chosen
/// slots — `slot` here is the caller's counter already reduced mod 8). Slot 6 of the eight is
/// always a Golden Key, framed at chest style 0 (an ordinary *unlocked* chest, so exploring the
/// dungeon is never blocked on finding a key before finding the thing the key opens); every other
/// slot is framed at chest style 2 (the locked dungeon/gold chest `terrustia_proto::locks`
/// already knows how to open with one). Separately, **any** chest — whichever slot it landed on —
/// sitting within 50 tiles of the world surface line is overridden outright to the same
/// guaranteed unlocked Golden Key, so the entrance is never far from a way to open what is deeper
/// in (`WorldGen.cs:36228-36232`).
///
/// Returns the loot and the chest style [`add_chest_styled`] should frame the tile with.
fn dungeon_loot(
    rand: &mut UnifiedRandom,
    layout: &Layout,
    y: i32,
    slot: u8,
) -> (Vec<terrustia_proto::ItemStack>, u16) {
    use terrustia_proto::ItemStack;

    /// The real eight, in `dungeonLootStyle`'s own order (`WorldGen.cs:36197-36228`): Muramasa,
    /// Cobalt Shield, Aqua Scepter, Blue Moon, Magic Missile, Valor, Golden Key, Handgun.
    const ROUND_ROBIN: [i32; 8] = [155, 156, 157, 163, 113, 3317, 327, 164];
    /// The one slot that is a Golden Key rather than a weapon, and so the one slot framed
    /// unlocked (style 0) rather than locked (style 2).
    const GOLDEN_KEY_SLOT: usize = 6;
    const LOCKED_STYLE: u16 = 2;
    const UNLOCKED_STYLE: u16 = 0;

    let i = usize::from(slot) % ROUND_ROBIN.len();
    let (mut signature, mut style) = (
        ROUND_ROBIN[i],
        if i == GOLDEN_KEY_SLOT {
            UNLOCKED_STYLE
        } else {
            LOCKED_STYLE
        },
    );
    if y < layout.surface + 50 {
        signature = ROUND_ROBIN[GOLDEN_KEY_SLOT];
        style = UNLOCKED_STYLE;
    }

    let items = vec![
        ItemStack::new(signature, 1, 0),
        ItemStack::new(8, rand.next_range(15, 40) as i16, 0),
        ItemStack::new(72, rand.next_range(5, 40) as i16, 0),
    ];
    (items, style)
}

/// Grass, and the surface plants that grow on it.
pub fn greenery(world: &mut World, layout: &Layout, heights: &[i32], rand: &mut UnifiedRandom) {
    for x in 0..layout.width {
        let top = heights[x as usize];
        if layout.surface_biome(x) == Some(Surface::Ocean) {
            continue;
        }
        let ground = world.tile(x, top).block;
        let plant = match ground {
            tiles::GRASS => tiles::PLANTS,
            _ => continue,
        };
        if rand.next_max(3) != 0 {
            continue;
        }
        if world.tile(x, top - 1).is_active() {
            continue;
        }
        let wall = world.tile(x, top - 1).wall;
        let mut tile = Tile::framed(plant, (rand.next_max(6) * 18) as i16, 0);
        tile.wall = wall;
        world.set_tile(x, top - 1, tile);
    }
}

/// Cobwebs, which is what tells a player a cave has not been visited.
pub fn cobwebs(world: &mut World, layout: &Layout, rand: &mut UnifiedRandom) {
    for _ in 0..layout.width * 2 {
        let x = rand.next_range(10, layout.width - 10);
        let y = rand.next_range(layout.rock, layout.underworld - 20);
        if world.tile(x, y).is_active() {
            continue;
        }
        // Only where something is adjacent to hang from.
        let anchored = [(0, -1), (0, 1), (-1, 0), (1, 0)]
            .iter()
            .any(|(dx, dy)| world.tile(x + dx, y + dy).is_active());
        if !anchored {
            continue;
        }
        let mut spread = rand.next_range(3, 14);
        let (mut cx, mut cy) = (x, y);
        while spread > 0 {
            spread -= 1;
            if !world.tile(cx, cy).is_active() {
                place(world, cx, cy, tiles::COBWEB);
            }
            match rand.next_max(4) {
                0 => cx += 1,
                1 => cx -= 1,
                2 => cy += 1,
                _ => cy -= 1,
            }
            if !world.in_bounds(cx, cy) {
                break;
            }
        }
    }
}

#[cfg(test)]
mod chest_loot_tests {
    use super::*;
    use crate::world::worldgen::layout::Band;

    /// A biome-tagged chest carries vanilla's real jungle signature item — the
    /// `GetNextJungleChestItem` cycle (`WorldGen.cs:10146-10173`) — not the ice/snow-biome chest
    /// table (`670, 724, 950, 1319, 987, 1579, 6153`, gated on tile 147/161/162/197 at
    /// `WorldGen.cs:36464`, nothing to do with the jungle) this table used to draw from. Feral
    /// Claws(211), Anklet of the Wind(212), Staff of Regrowth(213) and Boomstick(964) must show
    /// up, and neither of the real reroll targets — Fiberglass Fishing Pole(2292) at 1/15,
    /// Flower Boots(3017) at 1/20 — nor any ice-table id may ever appear.
    #[test]
    fn a_jungle_column_gets_vanillas_jungle_chest_items() {
        let mut layout = test_layout();
        layout.jungle = Band { from: 100, to: 200 };

        let mut seen_signature = false;
        let mut seen_fishing_pole = false;
        let mut seen_flower_boots = false;
        for seed in 0..400i32 {
            let mut rand = UnifiedRandom::new(seed);
            let items = biome_chest_loot(&layout, 150, 500, &mut rand)
                .expect("a jungle-biome column must not fall through to the generic table");
            let signature = items[0].id;
            assert!(
                [211, 212, 213, 964, 2292, 3017].contains(&signature),
                "unexpected jungle chest item id {signature} — the frozen-chest set \
                 (670/724/950/1319/987/1579/6153/997) must never appear here"
            );
            match signature {
                211 | 212 | 213 | 964 => seen_signature = true,
                2292 => seen_fishing_pole = true,
                3017 => seen_flower_boots = true,
                _ => unreachable!(),
            }
        }
        assert!(
            seen_signature,
            "the jungle table should produce its own items"
        );
        assert!(
            seen_fishing_pole,
            "the one-in-fifteen Fiberglass Fishing Pole reroll should show up over 400 draws"
        );
        assert!(
            seen_flower_boots,
            "the one-in-twenty Flower Boots reroll should show up over 400 draws"
        );
    }

    #[test]
    fn a_deep_desert_column_gets_vanillas_desert_chest_items() {
        let mut layout = test_layout();
        layout.desert = Band { from: 300, to: 400 };
        layout.rock = 400;

        for seed in 0..50i32 {
            let mut rand = UnifiedRandom::new(seed);
            let items = biome_chest_loot(&layout, 350, 500, &mut rand)
                .expect("a deep desert column must not fall through to the generic table");
            assert!(
                [4056, 4055, 4262, 4263].contains(&items[0].id),
                "unexpected desert chest item id {}",
                items[0].id
            );
        }
    }

    /// A shallow desert column — above the rock layer — is not vanilla's underground desert, and
    /// a non-biome column gets the ordinary depth-tiered table, not a biome one.
    #[test]
    fn everywhere_else_falls_back_to_the_generic_table() {
        let mut layout = test_layout();
        layout.jungle = Band { from: 100, to: 200 };
        layout.desert = Band { from: 300, to: 400 };
        layout.rock = 400;
        let mut rand = UnifiedRandom::new(1);

        // Shallow desert: above the rock layer.
        assert!(biome_chest_loot(&layout, 350, 100, &mut rand).is_none());
        // Plain caverns, in neither band.
        assert!(biome_chest_loot(&layout, 250, 500, &mut rand).is_none());
    }

    /// `dungeon_loot`'s previous fix already replaced the original mislabeled ids
    /// (327/328/329/330/676/1266, disagreeing with their own comments — `ItemID.cs` says 327 is
    /// Golden Key, not Muramasa, and so on) with a five-item classic family, but that family was
    /// itself fabricated: real vanilla's table (`WorldGen.cs:36189-36237`) is eight items chosen
    /// by a **deterministic round-robin** over `dungeonLootStyle`, not a per-chest die roll, and
    /// one of the eight (slot 6) is a Golden Key rather than a weapon. This pins the real eight,
    /// in order, plus the chest style each slot frames — style 2 (locked) for the seven weapons,
    /// style 0 (unlocked) for the key, so a dungeon is never sealed behind its own loot.
    #[test]
    fn dungeon_loot_cycles_through_the_real_eight_item_table_in_order() {
        let layout = test_layout();
        let deep_y = layout.surface + 500; // comfortably past the near-surface override below
        let mut rand = UnifiedRandom::new(1);

        let real_table = [155, 156, 157, 163, 113, 3317, 327, 164];
        for (slot, &want_item) in real_table.iter().enumerate() {
            let (loot, style) = dungeon_loot(&mut rand, &layout, deep_y, slot as u8);
            assert_eq!(loot[0].id, want_item, "round-robin slot {slot}");
            let want_style = if slot == 6 { 0 } else { 2 };
            assert_eq!(style, want_style, "chest style for slot {slot}");
        }

        // `dungeonLootStyle` wraps at 8 (`WorldGen.cs:36192-36196`), back to slot 0's Muramasa.
        let (loot, style) = dungeon_loot(&mut rand, &layout, deep_y, 8);
        assert_eq!(loot[0].id, 155, "slot 8 wraps to slot 0");
        assert_eq!(style, 2);
    }

    /// Whichever slot the round-robin lands on, a chest within 50 tiles of the world surface line
    /// is always overridden to a guaranteed, *unlocked* Golden Key (`WorldGen.cs:36228-36232`) —
    /// so an explorer who has barely climbed down always finds a way to open the locked chests
    /// deeper in, rather than needing to already have a key to find the first key.
    #[test]
    fn a_near_surface_chest_always_overrides_to_an_unlocked_golden_key() {
        let layout = test_layout();
        let near_y = layout.surface + 49; // just inside the 50-tile window
        let mut rand = UnifiedRandom::new(1);

        for slot in 0..8u8 {
            let (loot, style) = dungeon_loot(&mut rand, &layout, near_y, slot);
            assert_eq!(
                loot[0].id, 327,
                "slot {slot} should still be overridden to the Golden Key"
            );
            assert_eq!(style, 0, "slot {slot} should still be unlocked");
        }
    }

    /// The boundary itself: `y < surface + 50` is false exactly at `surface + 50`, so the ordinary
    /// round-robin applies from there down.
    #[test]
    fn just_outside_the_near_surface_window_the_round_robin_applies_normally() {
        let layout = test_layout();
        let just_deep = layout.surface + 50;
        let mut rand = UnifiedRandom::new(1);

        let (loot, style) = dungeon_loot(&mut rand, &layout, just_deep, 0);
        assert_eq!(
            loot[0].id, 155,
            "slot 0 (Muramasa), not overridden to a key"
        );
        assert_eq!(style, 2, "locked, not the near-surface unlocked override");
    }

    /// `cavern_loot`'s old shallow/deep arrays disagreed with their own comments the same way —
    /// e.g. 963 really is Black Belt (a post-Plantera item, wrong for an ungated cavern chest,
    /// not "Bandage") and 1300 really is Rifle Scope (hardmode, not "Spelunker Potion"). Pins
    /// each pool to ids that now actually match what the chest is supposed to hold, and confirms
    /// neither of those two wrongly-tiered ids can appear at either depth.
    #[test]
    fn cavern_chests_hold_ids_that_match_their_own_comments() {
        let mut layout = test_layout();
        layout.rock = 500;
        let mut seen_shallow = std::collections::HashSet::new();
        let mut seen_deep = std::collections::HashSet::new();
        for seed in 0..400i32 {
            let mut rand = UnifiedRandom::new(seed);
            seen_shallow.insert(cavern_loot(&layout, layout.rock + 10, &mut rand)[0].id);
            let mut rand2 = UnifiedRandom::new(seed + 10_000);
            seen_deep.insert(cavern_loot(&layout, layout.rock + 200, &mut rand2)[0].id);
        }
        for wrong in [963, 1300] {
            assert!(
                !seen_shallow.contains(&wrong) && !seen_deep.contains(&wrong),
                "post-Plantera/hardmode item {wrong} must never come out of an ungated cavern \
                 chest"
            );
        }
        assert_eq!(
            seen_shallow,
            [49, 965, 930, 158, 975, 50].into_iter().collect(),
            "shallow pool"
        );
        assert_eq!(
            seen_deep,
            [55, 53, 54, 296, 157, 997].into_iter().collect(),
            "deep pool"
        );
    }

    fn test_layout() -> Layout {
        let mut rand = UnifiedRandom::new(1);
        Layout::plan(2000, 800, &mut rand)
    }
}

/// A chasm carries its own world's evil wall, and a Shadow Orb room used to carry the wrong one.
///
/// `evil_chasms` hardcoded `walls::EBONSTONE` behind every orb, so a crimson world's Crimson Heart
/// sat in a Corruption wall. Vanilla reads `GenVars.ebonStoneWall`, which is 3 in a corruption
/// world (`WorldGen.cs:8294`) and 83 in a crimson one (`WorldGen.cs:11330`), and `ChasmRunner`
/// writes that one value behind everything it digs (`WorldGen.cs:76867`).
///
/// The wall is also what makes a chasm a chasm rather than a cave: `terrain::fill` no longer walls
/// the cavern layer, because `DirtWallBackgrounds` (`WorldGen.cs:11895-11933`) walks each column
/// only to `worldSurface + 0..10` and no later pass adds one below that. So the chasm pass has to
/// place its own, and a chasm with no wall behind it is `nextCount`-transparent
/// (`WorldGen.cs:9539-9543`) in a way vanilla's never is.
#[cfg(test)]
mod evil_chasm_walls {
    use super::*;

    fn chasm_walls(crimson: bool, seed: i32) -> std::collections::BTreeSet<u16> {
        let mut rand = UnifiedRandom::new(seed);
        let mut layout = Layout::plan(1200, 600, &mut rand);
        layout.evil = if crimson {
            Evil::Crimson
        } else {
            Evil::Corruption
        };
        let mut world = World::empty(1200, 600, "chasm walls");
        world.crimson = crimson;
        let heights = crate::world::worldgen::terrain::heightmap(&layout, &mut rand);
        crate::world::worldgen::terrain::fill(&mut world, &layout, &heights, &mut rand);
        let orbs = evil_chasms(&mut world, &layout, &heights, &mut rand, None);
        assert!(orbs > 0, "the pass must have dug something to measure");

        // Every wall found behind a Shadow Orb tile, which is the one place the old code named
        // Ebonstone outright.
        let mut seen = std::collections::BTreeSet::new();
        for x in 0..1200 {
            for y in 0..600 {
                let tile = world.tile(x, y);
                if tile.is_active() && tile.block == tiles::SHADOW_ORB {
                    seen.insert(tile.wall);
                }
            }
        }
        seen
    }

    #[test]
    fn a_crimson_world_walls_its_heart_rooms_in_crimstone() {
        for seed in 0..12i32 {
            let seen = chasm_walls(true, seed);
            assert_eq!(
                seen,
                [walls::CRIMSTONE].into_iter().collect(),
                "seed {seed}: a Crimson Heart belongs in a Crimstone wall, not whatever the \
                 corruption branch happens to name"
            );
        }
    }

    #[test]
    fn a_corruption_world_still_walls_its_orb_rooms_in_ebonstone() {
        for seed in 0..12i32 {
            let seen = chasm_walls(false, seed);
            assert_eq!(
                seen,
                [walls::EBONSTONE].into_iter().collect(),
                "seed {seed}: and the branch that was already right stays right"
            );
        }
    }

    /// The shaft itself is walled below its first few rows, and open to the sky above them:
    /// `if (num13 > j + genRand.Next(3, 20))` (`WorldGen.cs:76864-76867`). A chasm walled all the
    /// way to the top would be a sealed pit rather than a mouth in the ground.
    #[test]
    fn the_top_of_a_chasm_stays_open_to_the_sky() {
        let mut rand = UnifiedRandom::new(7);
        let mut layout = Layout::plan(1200, 600, &mut rand);
        layout.evil = Evil::Corruption;
        let mut world = World::empty(1200, 600, "chasm mouth");
        let heights = crate::world::worldgen::terrain::heightmap(&layout, &mut rand);
        crate::world::worldgen::terrain::fill(&mut world, &layout, &heights, &mut rand);
        evil_chasms(&mut world, &layout, &heights, &mut rand, None);

        // Somewhere in the evil band there is an open, unwalled column of chasm within twenty
        // rows of the surface, and walled chasm below it.
        let mut open_near_the_top = 0;
        let mut walled_below = 0;
        for x in layout.evil_band.from.max(0)..layout.evil_band.to.min(1200) {
            let top = heights[x as usize];
            for y in top..(top + 20).min(600) {
                let tile = world.tile(x, y);
                if !tile.is_active() && tile.wall == 0 {
                    open_near_the_top += 1;
                }
            }
            for y in (top + 25).min(599)..(top + 60).min(600) {
                let tile = world.tile(x, y);
                if !tile.is_active() && tile.wall == walls::EBONSTONE {
                    walled_below += 1;
                }
            }
        }
        assert!(
            open_near_the_top > 0,
            "a chasm has to have a mouth, or nobody falls into it"
        );
        assert!(
            walled_below > 0,
            "and it has to be walled once it is inside, or it reads as an ordinary cave"
        );
    }
}

/// Golem's fight is gated on a single tile that `temple()` used to never place.
///
/// A real client will not let a player attempt to use a Lihzahrd Power Cell at all without an
/// active `LIHZAHRD_ALTAR` tile nearby — this is a client-side gate on the interaction itself, not
/// something a server can work around after the fact. So a temple with no altar anywhere in the
/// world does not make Golem merely hard to reach; it makes Golem unreachable through any
/// legitimate play, in every world this generator has ever produced, forever, silently — the
/// worldgen module's own doc comment already claimed "Jungle temple → no Golem" as the reason the
/// temple exists at all, so this is a case where the code did not do what its own comment said it
/// did.
///
/// Run across several seeds and several of the random temple sizes `temple()` itself rolls
/// (`half_w` 34-55, `half_h` 20-32), rather than one lucky draw, because the fix's fallback offsets
/// exist specifically to cover sizes where the centre placement is not immediately clear.
#[cfg(test)]
mod temple_altar_tests {
    use super::*;

    #[test]
    fn every_temple_gets_a_lihzahrd_altar() {
        for seed in 0..40i32 {
            let mut rand = UnifiedRandom::new(seed);
            let mut world = World::empty(400, 300, "temple");
            let layout_rand = &mut UnifiedRandom::new(seed);
            let mut layout = Layout::plan(400, 300, layout_rand);
            layout.temple = (200, 150);

            temple(&mut world, &layout, &mut rand);

            let mut found = 0usize;
            for x in 140..260 {
                for y in 100..200 {
                    if world.tile(x, y).is_active() && world.tile(x, y).block == LIHZAHRD_ALTAR {
                        found += 1;
                    }
                }
            }
            assert!(
                found > 0,
                "seed {seed}: no Lihzahrd Altar tile anywhere in this temple — Golem would be \
                 unreachable in a world generated from this seed"
            );
        }
    }
}

#[cfg(test)]
mod cave_wall_tests {
    use super::*;

    /// The cavern layer has to come out of `terrain::fill` with no wall on it at all, solid rock
    /// included.
    ///
    /// `nextCount` (`WorldGen.cs:9539-9543`, transcribed as `cave_flood::count`) reads a tile's
    /// wall *before* it asks whether the tile is solid, and saturates its whole search the moment
    /// it finds one. A pocket's own stone boundary is the first thing any fill touches, so one wall
    /// there is enough to make every `GemCaves`/`SpiderCaves`/`CaveWallsInEnclosedSpaces`
    /// measurement in the world answer "too big" and reject the site.
    ///
    /// **This is where the guard moved to, and why.** It used to assert that `caves()` itself
    /// stripped the wall off what it carved, because `terrain::fill` painted a stone wall over the
    /// whole underground and the carver was the only thing that could take it back off. That was
    /// the wrong half of the pipeline: vanilla's own `TileRunner` never touches a wall
    /// (`WorldGen.cs:77817`, a bare `active(false)`), and it does not have to, because vanilla's
    /// terrain never puts one there - `DirtWallBackgrounds` (`WorldGen.cs:11895-11933`) stops at
    /// `worldSurface + 0..10`. Stripping it in the carver also erased the dirt-crust wall that
    /// vanilla deliberately leaves behind a shallow cave, which is the whole reason
    /// `DirtWallCleanup` exists. So the rule is asserted here, on the source of the wall, and
    /// `caves()` is free to keep vanilla's own no-op.
    #[test]
    fn the_cavern_layer_comes_out_of_terrain_unwalled() {
        let (width, height) = (1200, 900);
        let mut rand = UnifiedRandom::new(7);
        let layout = Layout::plan(width, height, &mut rand);
        let mut world = World::empty(width, height, "cave-wall");
        world.crimson = layout.evil == Evil::Crimson;
        let heights = crate::world::worldgen::terrain::heightmap(&layout, &mut rand);
        crate::world::worldgen::terrain::fill(&mut world, &layout, &heights, &mut rand);

        let mut checked = 0usize;
        let mut walled = 0usize;
        for x in 0..width {
            // The jungle keeps its wall on purpose (vanilla's `GenVars.mudWall` really does wall
            // the jungle underground), so it is not part of this rule and is skipped.
            if layout.surface_biome(x) == Some(Surface::Jungle) {
                continue;
            }
            for y in layout.rock + 130..layout.underworld {
                checked += 1;
                walled += usize::from(world.tile(x, y).wall != 0);
            }
        }
        assert!(checked > 0, "nothing in the cavern layer to check");
        assert_eq!(
            walled, 0,
            "{walled} of {checked} cavern-layer tiles carry a wall straight out of terrain::fill - \
             every cave_flood measurement in the world saturates on the first one it touches"
        );
    }

    /// The same property, at real world size with a real seed rather than the small hand-built
    /// fixture above — this is the actual check that found the bug, not a stand-in for it.
    /// **Updated for Tier 3**: originally ran the full `generate()` pipeline and sampled its final
    /// output; now stops right after `terrain::fill` + `caves()`, the same two steps in the same
    /// order `build()` itself runs — see the test's own body comment for why the full pipeline is
    /// no longer the right thing to sample once `CaveWallVariety`/`CaveWallsInEnclosedSpaces`/
    /// `MossAndMossCaves` exist.
    ///
    /// **The second defect this used to flag is now fixed too, and the numbers are in
    /// `cave_topology_measurement`.** It read: pocket measurement from real sampled points
    /// saturates almost universally, because `caves()` was this project's own wandering-tunnel
    /// carver rather than vanilla's. Both halves of that turned out to be true but wrongly joined.
    /// The saturation was `terrain::fill`'s wall, not the topology: measured on three real worlds,
    /// 400 of 400 sampled fills stopped on a *walled tile* and not one of them ever reached the
    /// 3500-tile cap. The topology was separately wrong, just not in the way the note said - 74 to
    /// 80 connected components in the deep band with the largest holding 9 to 16 per cent of open
    /// space, so a mix of large networks rather than one, and only 4 to 11 pockets anywhere in the
    /// window `GemCaves` sites into. `caves()` is now vanilla's own four passes (see its doc
    /// comment); the same measurement reads 4717 to 4837 components with 632 to 698 in that window.
    #[test]
    fn a_real_generated_world_has_real_unwalled_open_cave_tiles() {
        // Sampled right after `terrain::fill` + `caves()` — the same two steps `build()` itself
        // runs in this order — rather than the full `generate()` pipeline. Tier 3 landed since this
        // test was first written (`CaveWallVariety`/`CaveWallsInEnclosedSpaces`/`MossAndMossCaves`,
        // all wired in after `smooth()`, near the end of `build()`) legitimately re-walls a real
        // fraction of the open cave network by design — that is the whole point of those three
        // passes, not a regression of the fix this test guards. Sampling the *final* world would
        // now measure Tier 3 working correctly and mistake it for `caves()` regressing. What this
        // test actually guards — `caves()` itself leaving no wall behind on what it carves — is
        // only ever true immediately after `caves()` runs, before anything downstream paints a wall
        // back on; on the real, full pipeline that is a fact about *this specific ordering*, not
        // about the finished world.
        let seed = 4242;
        let (width, height) = (4200, 1200);
        let mut rand = UnifiedRandom::new(seed);
        let plan = Layout::plan(width, height, &mut rand);
        let mut world = World::empty(width, height, "cave-wall-real");
        world.crimson = plan.evil == crate::world::worldgen::layout::Evil::Crimson;
        let heights = crate::world::worldgen::terrain::heightmap(&plan, &mut rand);
        crate::world::worldgen::terrain::fill(&mut world, &plan, &heights, &mut rand);
        caves(&mut world, &plan, &mut rand);

        let (from, to) = (plan.rock + 30, world.height() - 200);
        let mut rng = 12345u64;
        let mut open_samples = 0u32;
        let mut walled_samples = 0u32;
        for _ in 0..300 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let x = 20 + (((rng >> 16) as u32) % (world.width() as u32 - 40)) as i32;
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let y = from + (((rng >> 16) as u32) % (to - from).max(1) as u32) as i32;
            if world.tile(x, y).is_active() {
                continue;
            }
            open_samples += 1;
            if world.tile(x, y).wall != 0 {
                walled_samples += 1;
            }
        }
        assert!(
            open_samples > 20,
            "too few open samples ({open_samples}) to say anything about this world"
        );
        // Not zero: a blind sample across the whole deep-rock band also lands inside the
        // dungeon, the hive, the evil chasms and the jungle temple — every one of those still
        // uses `hollow`/`hollow_blob` and keeps its wall on purpose (see that function's own doc
        // comment), and this sampling has no way to tell "inside the dungeon" from "inside a
        // cave" without inspecting more than wall state. `caves()` carves far more of the deep
        // band than every other structure combined, so a large majority unwalled is the real
        // signal; a small minority walled is those other structures working as intended, not a
        // regression in this fix.
        let walled_fraction = f64::from(walled_samples) / f64::from(open_samples);
        assert!(
            walled_fraction < 0.2,
            "{walled_samples} of {open_samples} open points sampled from a real generated world \
             ({:.0}%) still carry a wall — too many to be only the dungeon/hive/chasms/temple; \
             caves() looks like it is leaving wall behind again",
            walled_fraction * 100.0
        );
    }
}

/// The instrument behind the "cave topology is not vanilla's" release blocker.
///
/// Nothing here asserts: it prints two independent measurements of the same world, so a claim
/// about pocket size can be a number rather than a paragraph.
///
/// * **Connectivity** is the topology question on its own terms: label every connected component
///   of open space in the deep band and report the size histogram. Vanilla's own `SmallHoles`/
///   `RockLayerCaves`/`DirtLayerCaves` passes (`WorldGen.cs:12046`/`12147`/`12106`) scatter many
///   thousands of independently-seeded `TileRunner` blobs, so its histogram is dominated by small
///   components; a single wandering-tunnel carver's is dominated by one giant one.
/// * **Reachability** is what the siting passes actually see: run the real
///   [`super::cave_flood::count`] predicate from random points in `GemCaves`'/`SpiderCaves`' own
///   search band and record *why* each fill ended. That distinguishes "the pocket is too big"
///   (a topology problem) from "the fill hit a walled tile" (a terrain problem), which the two
///   numbers together are the only way to tell apart.
///
/// Run with
/// `cargo test -p terrustia --lib structures::cave_topology_measurement -- --ignored --nocapture`.
#[cfg(test)]
mod cave_topology_measurement {
    use super::*;
    use crate::world::worldgen::layout::Layout;
    use terrustia_proto::tile_solid;

    /// Why one `cave_flood`-shaped fill stopped.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Stop {
        /// Ran into a tile carrying a wall. Vanilla's `nextCount` saturates on this
        /// (`WorldGen.cs:9539-9543`), and so does ours.
        Wall,
        /// Ran off the edge of the world (`WorldGen.cs:9518-9521`).
        Edge,
        /// Filled `max_tiles` without closing: the pocket is genuinely at least that big.
        Cap,
        /// Closed naturally against solid tiles. The only outcome a siting pass can use.
        Closed(usize),
    }

    /// `cave_flood::count`'s traversal with the stopping reason kept, which the real one throws
    /// away because no production caller needs it. Same order, same predicates.
    fn why(world: &World, x: i32, y: i32, max_tiles: usize) -> Stop {
        let mut seen: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
        let mut stack = vec![(x, y)];
        let mut tiles = 0usize;
        while let Some((cx, cy)) = stack.pop() {
            if tiles >= max_tiles {
                return Stop::Cap;
            }
            if cx <= 1 || cx >= world.width() - 1 || cy <= 1 || cy >= world.height() - 1 {
                return Stop::Edge;
            }
            if !seen.insert((cx, cy)) {
                continue;
            }
            let tile = world.tile(cx, cy);
            if tile.wall != 0 {
                return Stop::Wall;
            }
            if !tile_solid::solid(tile.block) || !tile.is_active() {
                tiles += 1;
                stack.push((cx - 1, cy));
                stack.push((cx + 1, cy));
                stack.push((cx, cy - 1));
                stack.push((cx, cy + 1));
            }
        }
        Stop::Closed(tiles)
    }

    /// Every connected component of open space between `from` and `to`, as a list of sizes.
    ///
    /// Ignores walls entirely: this is the shape of the carve, not what a siting pass can reach.
    fn component_sizes(world: &World, from: i32, to: i32) -> Vec<usize> {
        let (w, h) = (world.width(), world.height());
        let open = |x: i32, y: i32| {
            let t = world.tile(x, y);
            !tile_solid::solid(t.block) || !t.is_active()
        };
        let mut seen = vec![false; (w as usize) * ((to - from) as usize)];
        let idx = |x: i32, y: i32| (y - from) as usize * (w as usize) + x as usize;
        let mut sizes = Vec::new();
        let mut stack: Vec<(i32, i32)> = Vec::new();
        for y in from..to {
            for x in 0..w {
                if seen[idx(x, y)] || !open(x, y) {
                    continue;
                }
                seen[idx(x, y)] = true;
                stack.push((x, y));
                let mut size = 0usize;
                while let Some((cx, cy)) = stack.pop() {
                    size += 1;
                    for (nx, ny) in [(cx - 1, cy), (cx + 1, cy), (cx, cy - 1), (cx, cy + 1)] {
                        if nx < 0 || ny < from || nx >= w || ny >= to || ny >= h {
                            continue;
                        }
                        if seen[idx(nx, ny)] || !open(nx, ny) {
                            continue;
                        }
                        seen[idx(nx, ny)] = true;
                        stack.push((nx, ny));
                    }
                }
                sizes.push(size);
            }
        }
        sizes
    }

    fn report(label: &str, world: &World, plan: &Layout) {
        let from = plan.rock + 30;
        let to = world.height() - 230;

        let mut sizes = component_sizes(world, from, to);
        sizes.sort_unstable();
        let total: usize = sizes.iter().sum();
        let largest = sizes.last().copied().unwrap_or(0);
        let gem_window = sizes.iter().filter(|&&s| (50..300).contains(&s)).count();
        let spider_window = sizes.iter().filter(|&&s| (500..3500).contains(&s)).count();
        let tiny = sizes.iter().filter(|&&s| s < 50).count();
        eprintln!(
            "{label}: open={total} components={} largest={largest} ({:.1}% of open) \
             <50={tiny} 50..300={gem_window} 500..3500={spider_window}",
            sizes.len(),
            100.0 * largest as f64 / total.max(1) as f64,
        );

        // The reachability half: what a siting pass actually gets back.
        let mut rng = 987_654_321u64;
        let (mut wall, mut edge, mut cap, mut closed) = (0u32, 0u32, 0u32, 0u32);
        let mut closed_sizes: Vec<usize> = Vec::new();
        let mut sampled = 0u32;
        while sampled < 400 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let x = 200 + (((rng >> 16) as u32) % (world.width() as u32 - 400)) as i32;
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let y = from + (((rng >> 16) as u32) % (to - from).max(1) as u32) as i32;
            let tile = world.tile(x, y);
            if tile.is_active() && tile_solid::solid(tile.block) {
                continue;
            }
            sampled += 1;
            match why(world, x, y, 3500) {
                Stop::Wall => wall += 1,
                Stop::Edge => edge += 1,
                Stop::Cap => cap += 1,
                Stop::Closed(n) => {
                    closed += 1;
                    closed_sizes.push(n);
                }
            }
        }
        closed_sizes.sort_unstable();
        eprintln!(
            "{label}: of {sampled} open samples — wall={wall} edge={edge} cap={cap} \
             closed={closed} (median closed size {})",
            closed_sizes
                .get(closed_sizes.len() / 2)
                .map_or(0, |n| *n as i64),
        );
    }

    #[test]
    #[ignore]
    fn measure_cave_topology() {
        for seed in [4242i32, 999, 12345] {
            let (width, height) = (super::super::SMALL_WIDTH, super::super::SMALL_HEIGHT);
            let mut rand = UnifiedRandom::new(seed);
            let plan = Layout::plan(width, height, &mut rand);
            let mut world = World::empty(width, height, "cave-topology");
            world.crimson = plan.evil == Evil::Crimson;
            let heights = crate::world::worldgen::terrain::heightmap(&plan, &mut rand);
            // Processor time, not wall clock, for the reason `game::clock`'s own module doc gives:
            // this machine runs several worldgen lanes at once and a wall-clock reading of the
            // same code has come back anywhere from 2 to 17 seconds depending on who else was
            // building. The carve is single-threaded, so thread CPU time is its real cost.
            let started = crate::game::clock::Cpu::now();
            crate::world::worldgen::terrain::fill(&mut world, &plan, &heights, &mut rand);
            caves(&mut world, &plan, &mut rand);
            let carved = crate::game::clock::Cpu::now().since(started);
            report(
                &format!("seed {seed} (fill+caves, {carved:?})"),
                &world,
                &plan,
            );
        }
    }

    /// The same question one level up: how many gem and spider caves a *whole* generated world
    /// actually ends up with, which is the player-visible consequence of everything above.
    #[test]
    #[ignore]
    fn measure_sited_caves_on_real_worlds() {
        for seed in [4242u64, 999, 12345] {
            // Processor time again, see `measure_cave_topology`.
            let started = crate::game::clock::Cpu::now();
            let (_world, built) = crate::world::worldgen::build(
                super::super::SMALL_WIDTH,
                super::super::SMALL_HEIGHT,
                "measure-sited",
                seed,
            );
            eprintln!(
                "seed {seed}: gem_caves={} spider_caves={} cave_wall_variety={} \
                 cave_walls_enclosed={} underground_cabins={} ({:?})",
                built.gem_caves,
                built.spider_caves,
                built.cave_wall_variety,
                built.cave_walls_enclosed,
                built.underground_cabins,
                crate::game::clock::Cpu::now().since(started)
            );
        }
    }

    /// The regression guard for the whole thing, and the only one here that asserts.
    ///
    /// `GemCaves` and `SpiderCaves` both site by measuring the open pocket at a candidate point and
    /// rejecting anything outside a size window (50 to 299 tiles, and 500 to 3499). That is only
    /// answerable in a world whose caves come in pockets and whose cavern-layer rock carries no
    /// wall, so this one assertion covers both halves of this lane at once: with either half
    /// undone, every candidate in the world is rejected and both counts fall to zero. It runs the
    /// real pipeline at real size rather than a fixture, because the shape of a real generated
    /// world is exactly the thing in question.
    #[test]
    fn a_real_world_still_sites_gem_and_spider_caves_under_vanillas_own_size_rule() {
        for seed in [4242u64, 12345] {
            let (_world, built) = crate::world::worldgen::build(
                super::super::SMALL_WIDTH,
                super::super::SMALL_HEIGHT,
                "sited-caves",
                seed,
            );
            // The quotas are `width * 0.003` and `width * 0.005` (`WorldGen.cs:17546`, `:17476`).
            // Anything short of them means candidates were being rejected, which is the failure
            // this guards; the exact number is vanilla's own arithmetic, not a tuned figure.
            assert_eq!(
                built.gem_caves, 12,
                "seed {seed}: gem caves short of their quota - candidates are being rejected"
            );
            assert_eq!(
                built.spider_caves, 21,
                "seed {seed}: spider caves short of their quota - candidates are being rejected"
            );
        }
    }
}
