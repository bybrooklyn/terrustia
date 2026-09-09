//! Dead Man's Chests: a gold chest re-skinned as bait, wired to everything nearby that can kill.
//!
//! Transcribed from `DeadMansChestBiome`
//! (`.scratch/decompiled/Terraria.GameContent.Biomes/DeadMansChestBiome.cs`, all 626 lines) and its
//! driver at `WorldGen.cs:21082-21108`. The fourteenth of the fifteen `MicroBiome` classes and the
//! largest of them.
//!
//! # What it does
//!
//! It does not build a room. It takes a gold chest this generator already placed, turns it into a
//! Dead Man's Chest, and rigs the rock around it: dart traps tunnelled into the walls to either
//! side, boulder traps buried in the ceiling above, explosives set into the floor below, and red
//! wire joining the lot to the chest so opening it fires everything at once. One time in three it
//! also slips a Dead Man's Chest note into the loot.
//!
//! A site is refused unless it yields at least one dart trap *and* at least one boulder or
//! explosive: a chest with only half a trap is worse than an untrapped one, because it teaches the
//! player the wrong lesson.
//!
//! # `micro_biomes.rs` sized this as needing two things it does not
//!
//! That module deferred this class as needing "a pre-existing trappable-chest mechanism this
//! generator does not have and `DitherSnake`/`DitherSnakePass` (a further ~500 lines in the same
//! namespace) for its own tunnel dressing". Reading all 626 lines, neither is true: the class never
//! mentions `DitherSnake`, and the "trappable-chest mechanism" is just a scan over chests that
//! already exist, which this generator has. What it actually needs is wire, actuators and boulders,
//! all of which are already modelled here.
//!
//! # Disclosed narrowings
//!
//! * `WorldGen.countTiles` (a flood-fill measuring how enclosed a spot is) has no counterpart; the
//!   40-tile minimum it gates on is approximated by counting solid tiles in a 21x21 box, which
//!   answers the same question - is this chest buried in rock rather than sitting in open air -
//!   with a cheaper method.
//! * `oceanDepths` is not consulted: this generator does not place gold chests in the ocean.
//! * Tile framing is dropped throughout, as everywhere else here. The chest's own frame is written
//!   directly, because that is what carries its style.
//! * `Main.tileSolidTop` has no table here, so the explosive site test uses solidity alone; the
//!   only tiles that differ are platforms, which this generator does not place underground.

use terrustia_proto::{Tile, TileFlags, tile_sets::frame_important, tile_solid};

use super::layout::Layout;
use super::rand::UnifiedRandom;
use super::secret_seed::SecretSeeds;
use super::structure_map::StructureMap;
use crate::world::World;

/// Chest.
const CHEST: u16 = 21;
/// Dead Man's Chest, and the frame column that identifies it.
const DEAD_MANS_CHEST: u16 = 467;
const DEAD_MANS_FRAME_X: i16 = 144;
/// The gold chest's own style column: `frameX / 36 == 1`.
const GOLD_CHEST_STYLE: i16 = 1;
/// Dart trap.
const DART_TRAP: u16 = 137;
/// Boulder.
const BOULDER: u16 = 138;
/// Explosives.
const EXPLOSIVES: u16 = 141;
/// Stone, which the boulder's shaft is packed with.
const STONE: u16 = 1;

/// `NumberOfDartTraps`, inclusive.
const DART_TRAPS: (i32, i32) = (3, 6);
/// `NumberOfBoulderTraps`, inclusive.
const BOULDER_TRAPS: (i32, i32) = (2, 4);
/// `NumberOfStepsBetweenBoulderTraps`, inclusive.
const BOULDER_STEPS: (i32, i32) = (2, 4);

struct Dart {
    at: (i32, i32),
    dir_x: i32,
    trap: (i32, i32),
    push: i32,
}

struct Boulder {
    at: (i32, i32),
    y_push: i32,
    height: i32,
    fill: u16,
}

struct Wire {
    at: (i32, i32),
    dir: (i32, i32),
    steps: i32,
}

#[derive(Default)]
struct Plan {
    darts: Vec<Dart>,
    boulders: Vec<Boulder>,
    explosives: Vec<(i32, i32)>,
    wires: Vec<Wire>,
}

impl Plan {
    /// `AreThereEnoughTraps` (`:161-168`): a dart trap, plus something heavier.
    fn enough(&self) -> bool {
        (!self.boulders.is_empty() || !self.explosives.is_empty()) && !self.darts.is_empty()
    }
}

fn solid(world: &World, x: i32, y: i32) -> bool {
    let t = world.tile(x, y);
    t.is_active() && tile_solid::solid(t.block)
}

/// `TileID.Sets.IsAContainer`, narrowed to the ids this generator can actually produce: chest,
/// Dead Man's Chest, and dresser.
fn is_container(block: u16) -> bool {
    matches!(block, 21 | 467 | 88)
}

/// `IsAGoodSpot` (`:576-597`).
fn good_spot(world: &World, at: (i32, i32)) -> bool {
    if at.0 < 50 || at.1 < 50 || at.0 >= world.width() - 50 || at.1 >= world.height() - 50 {
        return false;
    }
    let t = world.tile(at.0, at.1);
    if t.block != CHEST || !t.is_active() {
        return false;
    }
    if t.frame_x / 36 != GOLD_CHEST_STYLE {
        return false;
    }
    // Something clearable two tiles down, so the explosives have floor to sit in.
    let below = world.tile(at.0, at.1 + 2);
    if below.is_active() && is_container(below.block) {
        return false;
    }
    // No existing wiring within 20 tiles: vanilla refuses to rig a chest that is already wired.
    for x in at.0 - 20..=at.0 + 20 {
        for y in at.1 - 20..=at.1 + 20 {
            if world.tile(x, y).flags.has(TileFlags::WIRE_RED) {
                return false;
            }
        }
    }
    // `countTiles >= 40`: is this chest buried rather than in open air? See the module doc.
    let mut packed = 0;
    for x in at.0 - 10..=at.0 + 10 {
        for y in at.1 - 10..=at.1 + 10 {
            if solid(world, x, y) {
                packed += 1;
            }
        }
    }
    packed >= 40
}

/// `FindDartTrapSpotSingle` (`:314-334`).
fn find_dart(world: &World, plan: &mut Plan, at: (i32, i32), dir_x: i32) -> bool {
    for i in 0..20 {
        let (x, y) = (at.0 + i * dir_x, at.1);
        let t = world.tile(x, y);
        if t.is_active() && is_container(t.block) {
            return false;
        }
        if t.is_active() && tile_solid::solid(t.block) {
            if i >= 5 && !frame_important(t.block) {
                plan.darts.push(Dart {
                    at,
                    dir_x,
                    trap: (x, y),
                    push: i,
                });
                return true;
            }
            return false;
        }
    }
    false
}

/// `FindDartTrapSpots` (`:296-312`).
fn find_darts(world: &World, plan: &mut Plan, rand: &mut UnifiedRandom, mut at: (i32, i32)) {
    let count = rand.next_range(DART_TRAPS.0, DART_TRAPS.1 + 1);
    let mut dir = if rand.next_max(2) != 0 { 1 } else { -1 };
    let mut steps = -1;
    for i in 0..count {
        if find_dart(world, plan, at, dir) {
            steps = i;
        }
        dir *= -1;
        at.1 -= 1;
    }
    plan.wires.push(Wire {
        at: (at.0, at.1 + count),
        dir: (0, -1),
        steps,
    });
}

/// `FindBoulderTrapSpot_CheckSpot` (`:236-292`).
fn check_boulder(world: &World, plan: &mut Plan, at: (i32, i32), y_push: i32) {
    let mut counts: std::collections::HashMap<u16, i32> = std::collections::HashMap::new();
    for i in at.0..at.0 + 2 {
        for j in at.1 - 4..=at.1 {
            let t = world.tile(i, j);
            if t.is_active() && !frame_important(t.block) && tile_solid::solid(t.block) {
                *counts.entry(t.block).or_insert(0) += 1;
            }
            if t.is_active() && is_container(t.block) {
                return;
            }
        }
    }
    // The shaft's roof must be solid all the way across, or the boulder falls out sideways.
    for k in at.0 - 1..at.0 + 3 {
        for l in at.1 - 5..=at.1 - 2 {
            let t = world.tile(k, l);
            if !t.is_active() || is_container(t.block) {
                return;
            }
        }
    }
    // And there must be somewhere for it to fall.
    if world.tile(at.0, at.1 + 1).is_active() && world.tile(at.0 + 1, at.1 + 1).is_active() {
        return;
    }
    for m in at.0 - 2..=at.0 + 3 {
        for n in at.1 - 6..=at.1 - 2 {
            let t = world.tile(m, n);
            if t.is_active() && (is_container(t.block) || matches!(t.block, 12 | 665 | 639)) {
                return;
            }
        }
    }
    // Pack the shaft with whatever the ceiling is mostly made of.
    let fill = counts
        .into_iter()
        .max_by_key(|&(block, n)| (n, std::cmp::Reverse(block)))
        .map(|(block, _)| block)
        .unwrap_or(STONE);
    plan.boulders.push(Boulder {
        at,
        y_push: y_push - 1,
        height: 4,
        fill,
    });
}

/// `FindBoulderTrapSpots` (`:180-221`).
fn find_boulders(world: &World, plan: &mut Plan, rand: &mut UnifiedRandom, at: (i32, i32)) {
    let count = rand.next_range(BOULDER_TRAPS.0, BOULDER_TRAPS.1 + 1);
    let stride = rand.next_range(BOULDER_STEPS.0, BOULDER_STEPS.1 + 1);
    let mut x = at.0 - count / 2 * stride;
    let top = at.1 - 6;
    for _ in 0..=count {
        for i in 0..50 {
            if world.tile(x, top - i).is_active() {
                check_boulder(world, plan, (x, top - i), i);
                break;
            }
        }
        x += stride;
    }
    if plan.boulders.is_empty() {
        return;
    }
    let mut lo = plan.boulders[0].at.0;
    let mut hi = lo;
    for b in &plan.boulders[1..] {
        lo = lo.min(b.at.0);
        hi = hi.max(b.at.0);
    }
    lo = lo.min(at.0);
    hi = hi.max(at.0);
    plan.wires.push(Wire {
        at: (lo, top - 1),
        dir: (1, 0),
        steps: hi - lo,
    });
    plan.wires.push(Wire {
        at,
        dir: (0, -1),
        steps: 7,
    });
}

/// `IsGoodSpotsForExplosive` (`:396-408`).
fn good_explosive(world: &World, x: i32, y: i32) -> bool {
    let t = world.tile(x, y);
    if t.is_active() && is_container(t.block) {
        return false;
    }
    t.is_active() && tile_solid::solid(t.block) && !frame_important(t.block)
}

/// `FindExplosiveTrapSpots` (`:337-394`).
fn find_explosives(world: &World, plan: &mut Plan, rand: &mut UnifiedRandom, at: (i32, i32)) {
    let y = at.1 + 3;
    let mut near = Vec::new();
    let mut x = at.0;
    if good_explosive(world, x, y) {
        near.push(x);
    }
    x += 1;
    if good_explosive(world, x, y) {
        near.push(x);
    }
    let under = if near.is_empty() {
        -1
    } else {
        near[rand.next_max(near.len() as i32) as usize]
    };

    let mut right = Vec::new();
    x += rand.next_range(2, 6);
    for i in x..x + 4 {
        if good_explosive(world, i, y) {
            right.push(i);
        }
    }
    let right_pick = if right.is_empty() {
        -1
    } else {
        right[rand.next_max(right.len() as i32) as usize]
    };

    // Vanilla reuses the same list here without clearing it, so the left-hand pick can draw a
    // right-hand candidate. Kept: it is what the game does, and it only widens the spread.
    let mut left = right.clone();
    let start = at.0 - 4 - rand.next_range(2, 6);
    for j in start..start + 4 {
        if good_explosive(world, j, y) {
            left.push(j);
        }
    }
    let left_pick = if left.is_empty() {
        -1
    } else {
        left[rand.next_max(left.len() as i32) as usize]
    };

    for pick in [left_pick, under, right_pick] {
        if pick != -1 {
            plan.explosives.push((pick, y));
        }
    }
}

fn wire_line(world: &mut World, at: (i32, i32), dir: (i32, i32), steps: i32) {
    for i in 0..=steps {
        let (x, y) = (at.0 + dir.0 * i, at.1 + dir.1 * i);
        if !world.in_bounds(x, y) {
            continue;
        }
        let mut t = world.tile(x, y);
        t.flags = TileFlags(t.flags.0 | TileFlags::WIRE_RED);
        world.set_tile(x, y, t);
    }
}

/// Rig the chest at `at`. Returns whether it was rigged.
pub fn place(world: &mut World, rand: &mut UnifiedRandom, at: (i32, i32)) -> bool {
    if !good_spot(world, at) {
        return false;
    }
    let mut plan = Plan::default();
    let below = (at.0, at.1 + 1);
    find_boulders(world, &mut plan, rand, below);
    find_darts(world, &mut plan, rand, below);
    find_explosives(world, &mut plan, rand, below);
    if !plan.enough() {
        return false;
    }

    // `TurnGoldChestIntoDeadMansChest` (`:485-517`). The style lives in the frame.
    for i in 0..2 {
        for j in 0..2 {
            let (x, y) = (at.0 + i, at.1 + j);
            let mut t = world.tile(x, y);
            t.block = DEAD_MANS_CHEST;
            t.frame_x = DEAD_MANS_FRAME_X + (i as i16) * 18;
            t.frame_y = (j as i16) * 18;
            world.set_tile(x, y, t);
        }
    }
    // One time in three, the note goes in. Vanilla shifts the loot down a slot to make room; this
    // generator's chest records are a plain list, so it is inserted at the front instead.
    let note_roll = rand.next_max(3);
    if note_roll == 0 {
        for chest in world.chests.iter_mut().flatten() {
            if i32::from(chest.x) == at.0 && i32::from(chest.y) == at.1 {
                chest.items.insert(
                    0,
                    terrustia_proto::ItemStack {
                        id: 5007,
                        stack: 1,
                        prefix: 0,
                    },
                );
                chest.items.truncate(40);
                break;
            }
        }
    }

    for dart in &plan.darts {
        let mut t = world.tile(dart.trap.0, dart.trap.1);
        t.block = DART_TRAP;
        t.frame_y = 0;
        t.frame_x = if dart.dir_x == -1 { 18 } else { 0 };
        t.slope = 0;
        world.set_tile(dart.trap.0, dart.trap.1, t);
        wire_line(world, dart.at, (dart.dir_x, 0), dart.push);
    }

    for w in &plan.wires {
        wire_line(world, w.at, w.dir, w.steps);
    }

    for b in &plan.boulders {
        // `ActuallyPlaceBoulderTrap` (`:546-614`): hollow the top, pack the shaft, actuate it.
        for i in b.at.0..b.at.0 + 2 {
            for j in b.at.1 - b.height..=b.at.1 + 2 {
                if j < b.at.1 - b.height + 2 || j > b.at.1 {
                    world.set_tile(i, j, Tile::AIR);
                    continue;
                }
                let mut t = world.tile(i, j);
                if !t.is_active() {
                    t.flags = TileFlags(t.flags.0 | TileFlags::ACTIVE);
                    t.block = b.fill;
                }
                t.slope = 0;
                t.flags = TileFlags(t.flags.0 | TileFlags::WIRE_RED);
                if tile_solid::solid(t.block) {
                    t.flags = TileFlags(t.flags.0 | TileFlags::ACTUATOR);
                }
                world.set_tile(i, j, t);
            }
        }
        let bx = b.at.0 + 1;
        let by = b.at.1 - b.height + 1;
        for k in bx - 3..=bx + 2 {
            for l in by - 3..=by + 2 {
                let mut t = world.tile(k, l);
                if t.block != BOULDER {
                    t.block = STONE;
                    if t.flags.has(TileFlags::WIRE_RED) {
                        t.flags = TileFlags(t.flags.0 | TileFlags::ACTUATOR);
                    }
                    world.set_tile(k, l, t);
                }
            }
        }
        super::place_object::place_object(world, bx, by, BOULDER, 0, -1);
        wire_line(world, b.at, (0, 1), b.y_push);
    }

    for &(x, y) in &plan.explosives {
        let mut t = world.tile(x, y);
        t.block = EXPLOSIVES;
        t.frame_x = 0;
        t.frame_y = 0;
        t.slope = 0;
        world.set_tile(x, y, t);
    }

    // `PlaceWiresForExplosives` (`:136-159`).
    if let Some(&(_, ey)) = plan.explosives.first() {
        wire_line(world, at, (0, 1), ey - at.1);
        let lo = plan.explosives.iter().map(|e| e.0).min().unwrap_or(at.0);
        let hi = plan.explosives.iter().map(|e| e.0).max().unwrap_or(at.0);
        wire_line(world, (lo, ey), (1, 0), hi - lo);
    }
    true
}

/// `WorldGenRange.ScaleValue` for `ScaleWith: WorldWidth`.
fn scaled_by_width(value: i32, width: i32) -> i32 {
    (f64::from(width) / 4200.0 * f64::from(value)) as i32
}

/// The driver (`WorldGen.cs:21082-21108`). `DeadManChests` is 10-20 scaled with world width. Skips
/// entirely under No Traps World, as vanilla does. Returns how many chests were rigged.
pub fn scatter(
    world: &mut World,
    layout: &Layout,
    _structures: &mut StructureMap,
    rand: &mut UnifiedRandom,
    secret: SecretSeeds,
) -> usize {
    if secret.no_traps {
        return 0;
    }
    // Every gold chest that could be rigged, found once up front the way vanilla does.
    let mut candidates: Vec<(i32, i32)> = world
        .chests
        .iter()
        .flatten()
        .map(|c| (i32::from(c.x), i32::from(c.y)))
        .filter(|&at| good_spot(world, at))
        .collect();
    if candidates.is_empty() {
        return 0;
    }

    let min = scaled_by_width(10, layout.width);
    let max = scaled_by_width(20, layout.width);
    if max < min || min < 1 {
        return 0;
    }
    let wanted = rand.next_range(min, max + 1);
    let mut rigged = 0;
    let mut budget = 3000;
    while rigged < wanted && !candidates.is_empty() {
        budget -= 1;
        if budget <= 0 {
            break;
        }
        let pick = rand.next_max(candidates.len() as i32) as usize;
        let at = candidates.remove(pick);
        if place(world, rand, at) {
            rigged += 1;
        }
    }
    rigged as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrustia_proto::ItemStack;

    /// Solid stone with a gold chest sitting in a small pocket, which is the shape this rigs.
    fn chest_in_rock(w: i32, h: i32, at: (i32, i32)) -> World {
        let mut world = World::empty(w, h, "deadman");
        for x in 0..w {
            for y in 100..h {
                world.set_tile(x, y, Tile::block(STONE));
            }
        }
        // A pocket around the chest so the darts have somewhere to fire through.
        for x in at.0 - 8..=at.0 + 8 {
            for y in at.1 - 6..=at.1 + 2 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        // Floor under it.
        for x in at.0 - 8..=at.0 + 8 {
            for y in at.1 + 2..=at.1 + 6 {
                world.set_tile(x, y, Tile::block(STONE));
            }
        }
        // The gold chest itself: style 1, so `frameX / 36 == 1`.
        for i in 0..2i32 {
            for j in 0..2i32 {
                // `Tile::framed`, not `Tile::block`: a chest is frame-important and the plain
                // constructor debug-asserts against exactly this mistake.
                let t = Tile::framed(CHEST, 36 + (i as i16) * 18, (j as i16) * 18);
                world.set_tile(at.0 + i, at.1 + j, t);
            }
        }
        world.chests.push(Some(crate::world::objects::Chest {
            x: at.0 as i16,
            y: at.1 as i16,
            name: String::new(),
            items: vec![ItemStack {
                id: 1,
                stack: 1,
                prefix: 0,
            }],
        }));
        world
    }

    #[test]
    fn a_rigged_chest_becomes_a_dead_mans_chest_with_wire_and_traps() {
        let at = (200, 200);
        let mut world = chest_in_rock(400, 400, at);
        let mut rand = UnifiedRandom::new(1234);
        assert!(place(&mut world, &mut rand, at), "the chest was not rigged");

        assert_eq!(
            world.tile(at.0, at.1).block,
            DEAD_MANS_CHEST,
            "the chest was not re-skinned"
        );
        assert_eq!(world.tile(at.0, at.1).frame_x, DEAD_MANS_FRAME_X);

        let mut wires = 0;
        let mut darts = 0;
        for x in at.0 - 30..at.0 + 30 {
            for y in at.1 - 30..at.1 + 30 {
                let t = world.tile(x, y);
                if t.flags.has(TileFlags::WIRE_RED) {
                    wires += 1;
                }
                if t.block == DART_TRAP {
                    darts += 1;
                }
            }
        }
        assert!(wires > 0, "nothing was wired");
        assert!(darts > 0, "no dart trap, but the plan claimed enough traps");
    }

    /// An unwired plain chest is not a gold chest and must be left alone.
    #[test]
    fn a_chest_of_the_wrong_style_is_refused() {
        let at = (200, 200);
        let mut world = chest_in_rock(400, 400, at);
        for i in 0..2i32 {
            for j in 0..2i32 {
                let mut t = world.tile(at.0 + i, at.1 + j);
                t.frame_x = (i as i16) * 18; // style 0, a plain chest
                world.set_tile(at.0 + i, at.1 + j, t);
            }
        }
        let mut rand = UnifiedRandom::new(1);
        assert!(!place(&mut world, &mut rand, at));
    }

    /// A chest already near wiring is refused, so two rigs cannot overlap.
    #[test]
    fn a_chest_near_existing_wire_is_refused() {
        let at = (200, 200);
        let mut world = chest_in_rock(400, 400, at);
        let mut t = world.tile(at.0 + 5, at.1);
        t.flags = TileFlags(t.flags.0 | TileFlags::WIRE_RED);
        world.set_tile(at.0 + 5, at.1, t);
        let mut rand = UnifiedRandom::new(1);
        assert!(!place(&mut world, &mut rand, at));
    }

    #[test]
    fn rigging_is_reproducible_from_the_seed() {
        let at = (200, 200);
        let run = || {
            let mut world = chest_in_rock(400, 400, at);
            let mut rand = UnifiedRandom::new(77);
            place(&mut world, &mut rand, at);
            let mut fingerprint = Vec::new();
            for x in at.0 - 30..at.0 + 30 {
                for y in at.1 - 30..at.1 + 30 {
                    let t = world.tile(x, y);
                    fingerprint.push((t.block, t.flags.0));
                }
            }
            fingerprint
        };
        assert_eq!(run(), run());
    }
}
