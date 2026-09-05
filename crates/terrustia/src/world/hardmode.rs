//! Hardmode: smashing altars, and what the world gives up for it.
//!
//! An altar is the only way the hardmode ores get into a world. The first three smashes each seed
//! one tier — cobalt or palladium, mythril or orichalcum, adamantite or titanium, and which of each
//! pair a world has is decided by the coin flip on that first smash and never changes. Every smash
//! after that adds more of the same, and each one adds less: the yield falls with the number of
//! altars already broken and again with how far round the cycle you are.
//!
//! It is not free. Every altar also puts a wraith on whoever broke it, which before Plantera is a
//! genuine fight — that is the price of the ore, and it is why smashing all of them at once is a
//! choice rather than an obvious move.

use rand::{Rng, rngs::SmallRng};

/// The three tiers, and the pair each one picks from.
pub const COBALT: u16 = 107;
pub const PALLADIUM: u16 = 221;
pub const MYTHRIL: u16 = 108;
pub const ORICHALCUM: u16 = 222;
pub const ADAMANTITE: u16 = 111;
pub const TITANIUM: u16 = 223;

/// The wraith an altar puts on whoever broke it.
pub const WRAITH: u16 = 82;

/// Which ores a world settled on. `None` means that tier has not been seeded yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OreTiers {
    pub cobalt: Option<u16>,
    pub mythril: Option<u16>,
    pub adamantite: Option<u16>,
}

impl OreTiers {
    /// Read the three out of a world's seven saved tiers, where they occupy the last slots and
    /// `-1` means unchosen.
    ///
    /// The world owns these, not the server: keeping a second copy alongside is what let the two
    /// disagree, so this converts on the way in and [`Self::store`] converts on the way back out.
    pub fn load(saved: &[i16; 7]) -> Self {
        let pick = |v: i16| (v >= 0).then_some(v as u16);
        Self {
            cobalt: pick(saved[4]),
            mythril: pick(saved[5]),
            adamantite: pick(saved[6]),
        }
    }

    /// Write the three back into a world's seven saved tiers.
    pub fn store(&self, saved: &mut [i16; 7]) {
        for (slot, tier) in [self.cobalt, self.mythril, self.adamantite]
            .into_iter()
            .enumerate()
        {
            saved[4 + slot] = tier.map_or(-1, |v| v as i16);
        }
    }
}

/// What smashing an altar does.
#[derive(Debug)]
pub struct Smashed {
    /// What the world should say about it.
    /// The game's own localization key for this ore's announcement, not the English.
    ///
    /// Vanilla sends a key and lets each client render it in its own language; sending the
    /// sentence meant a French player read English, our bytes differed from the real server's,
    /// and the game's text was compiled into this repository. Keys verified against
    /// `Terraria.Localization.Content.en-US.Legacy.json`.
    pub announcement: &'static str,
    /// The ore that was seeded, and where each vein wants to go.
    pub ore: u16,
    pub veins: Vec<(i32, i32, f64, i32)>,
    /// How many wraiths it put on the player.
    pub wraiths: u32,
    /// Whether this smash decided a tier, which is worth telling clients about.
    pub decided_a_tier: bool,
}

/// The shape of the world an altar's ore has to fit into.
#[derive(Debug, Clone, Copy)]
pub struct WorldShape {
    pub width: i32,
    pub height: i32,
    pub surface: i32,
    pub rock_layer: i32,
}

/// Smash an altar.
///
/// `altar_count` is how many have already gone. Returns `None` when it is not hardmode, because
/// before the wall falls an altar is only a crafting station.
pub fn smash(
    altar_count: i32,
    hard_mode: bool,
    tiers: &mut OreTiers,
    shape: WorldShape,
    rng: &mut SmallRng,
) -> Option<Smashed> {
    if !hard_mode {
        return None;
    }
    let WorldShape {
        width: world_width,
        height: world_height,
        surface,
        rock_layer,
    } = shape;
    let step = altar_count % 3;
    let round = altar_count / 3 + 1;

    // The yield: a full world's worth, less eighty-five for each step past the first, less again
    // for every full round of three already broken. So the fourth altar gives noticeably less than
    // the first and the fortieth gives almost nothing.
    let mut yield_ = f64::from(world_width) / 4200.0;
    yield_ = yield_ * 310.0 - f64::from(85 * step);
    yield_ *= 0.85;
    yield_ /= f64::from(round);

    let mut decided = false;
    let (ore, announcement) = match step {
        0 => {
            let ore = *tiers.cobalt.get_or_insert_with(|| {
                decided = true;
                if rng.random_range(0..2) == 0 {
                    PALLADIUM
                } else {
                    COBALT
                }
            });
            if ore == PALLADIUM {
                yield_ *= 0.9;
            }
            yield_ *= 1.05;
            (
                ore,
                if ore == PALLADIUM {
                    "LegacyMisc.21"
                } else {
                    "LegacyMisc.12"
                },
            )
        }
        1 => {
            let ore = *tiers.mythril.get_or_insert_with(|| {
                decided = true;
                if rng.random_range(0..2) == 0 {
                    ORICHALCUM
                } else {
                    MYTHRIL
                }
            });
            if ore == ORICHALCUM {
                yield_ *= 0.9;
            }
            (
                ore,
                if ore == ORICHALCUM {
                    "LegacyMisc.22"
                } else {
                    "LegacyMisc.13"
                },
            )
        }
        _ => {
            let ore = *tiers.adamantite.get_or_insert_with(|| {
                decided = true;
                if rng.random_range(0..2) == 0 {
                    TITANIUM
                } else {
                    ADAMANTITE
                }
            });
            if ore == TITANIUM {
                yield_ *= 0.9;
            }
            (
                ore,
                if ore == TITANIUM {
                    "LegacyMisc.23"
                } else {
                    "LegacyMisc.14"
                },
            )
        }
    };

    // Each tier goes deeper than the last, which is what keeps you digging further down for it.
    let top = match ore {
        MYTHRIL | ORICHALCUM => rock_layer,
        ADAMANTITE | TITANIUM => (rock_layer * 2 + world_height) / 3,
        _ => surface,
    };
    let bottom = world_height - 150;
    let mut veins = Vec::new();
    if top < bottom && world_width > 200 {
        let mut placed = 0.0;
        while placed < yield_ {
            placed += 1.0;
            let x = rng.random_range(100..world_width - 100);
            let y = rng.random_range(top..bottom);
            let strength = f64::from(rng.random_range(5..10));
            let steps = rng.random_range(5..10);
            veins.push((x, y, strength, steps));
        }
    }

    Some(Smashed {
        announcement,
        ore,
        veins,
        wraiths: rng.random_range(1..=2),
        decided_a_tier: decided,
    })
}

/// Which blocks an ore vein is allowed to replace.
///
/// Everything else it passes straight through, which is why a vein never eats a chest, a wall of a
/// dungeon, or somebody's house.
pub fn replaceable(block: u16) -> bool {
    matches!(
        block,
        0 | 1
            | 23
            | 25
            | 40
            | 53
            | 57
            | 59
            | 60
            | 70
            | 109
            | 112
            | 116
            | 117
            | 147
            | 161
            | 163
            | 164
            | 199
            | 200
            | 203
            | 234
            | 396..=403
    ) || terrustia_proto::tile_sets::is_moss(block)
}

/// What the world needs to let a vein be dug into it.
pub trait OreWorld {
    fn tile(&self, x: i32, y: i32) -> terrustia_proto::tile::Tile;
    fn set_tile(&mut self, x: i32, y: i32, tile: terrustia_proto::tile::Tile);
    fn width(&self) -> i32;
    fn height(&self) -> i32;
}

/// Dig one vein: a blob that wanders as it goes and narrows to nothing.
///
/// Returns the tiles it changed.
pub fn run_vein(
    world: &mut impl OreWorld,
    at: (i32, i32),
    strength: f64,
    steps: i32,
    ore: u16,
    rng: &mut SmallRng,
) -> Vec<(i32, i32)> {
    let mut changed = Vec::new();
    let mut here = (f64::from(at.0), f64::from(at.1));
    // It drifts, and the drift itself wanders, which is what gives a vein its crooked shape.
    let mut drift = (
        f64::from(rng.random_range(-10..=10)) * 0.1,
        f64::from(rng.random_range(-10..=10)) * 0.1,
    );
    let mut left = f64::from(steps);
    let mut width = strength;

    while width > 0.0 && left > 0.0 {
        width = strength * (left / f64::from(steps));
        left -= 1.0;
        let x0 = ((here.0 - width * 0.5) as i32).max(0);
        let x1 = ((here.0 + width * 0.5) as i32).min(world.width());
        let y0 = ((here.1 - width * 0.5) as i32).max(0);
        let y1 = ((here.1 + width * 0.5) as i32).min(world.height());

        for x in x0..x1 {
            for y in y0..y1 {
                // A rough circle with a ragged edge, rather than a square.
                let reach = strength * 0.5 * (1.0 + f64::from(rng.random_range(-10..=10)) * 0.015);
                if (f64::from(x) - here.0).abs() + (f64::from(y) - here.1).abs() >= reach {
                    continue;
                }
                let tile = world.tile(x, y);
                if !tile.is_active() || !replaceable(tile.block) {
                    continue;
                }
                let mut ore_tile = tile;
                ore_tile.block = ore;
                world.set_tile(x, y, ore_tile);
                changed.push((x, y));
            }
        }

        here = (here.0 + drift.0, here.1 + drift.1);
        drift.0 = (drift.0 + f64::from(rng.random_range(-10..=10)) * 0.05).clamp(-1.0, 1.0);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    struct Stone(HashMap<(i32, i32), Tile>);

    impl OreWorld for Stone {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
        fn set_tile(&mut self, x: i32, y: i32, tile: Tile) {
            self.0.insert((x, y), tile);
        }
        fn width(&self) -> i32 {
            500
        }
        fn height(&self) -> i32 {
            500
        }
    }

    fn solid_stone() -> Stone {
        let mut tiles = HashMap::new();
        for x in 0..500 {
            for y in 0..500 {
                tiles.insert((x, y), Tile::block(1));
            }
        }
        Stone(tiles)
    }

    fn smash_one(count: i32, tiers: &mut OreTiers, seed: u64) -> Option<Smashed> {
        let mut rng = SmallRng::seed_from_u64(seed);
        smash(
            count,
            true,
            tiers,
            WorldShape {
                width: 4200,
                height: 1200,
                surface: 300,
                rock_layer: 500,
            },
            &mut rng,
        )
    }

    /// Before hardmode an altar is only furniture.
    #[test]
    fn an_altar_does_nothing_before_hardmode() {
        let mut rng = SmallRng::seed_from_u64(1);
        let mut tiers = OreTiers::default();
        let shape = WorldShape {
            width: 4200,
            height: 1200,
            surface: 300,
            rock_layer: 500,
        };
        assert!(smash(0, false, &mut tiers, shape, &mut rng).is_none());
        assert_eq!(tiers, OreTiers::default(), "and settles nothing");
    }

    /// The first three smashes seed one tier each, in order.
    #[test]
    fn the_first_three_seed_the_three_tiers() {
        let mut tiers = OreTiers::default();
        let first = smash_one(0, &mut tiers, 1).unwrap();
        assert!(matches!(first.ore, COBALT | PALLADIUM));
        assert!(first.decided_a_tier);
        assert_eq!(tiers.cobalt, Some(first.ore));
        assert_eq!(tiers.mythril, None, "one at a time");

        let second = smash_one(1, &mut tiers, 2).unwrap();
        assert!(matches!(second.ore, MYTHRIL | ORICHALCUM));
        let third = smash_one(2, &mut tiers, 3).unwrap();
        assert!(matches!(third.ore, ADAMANTITE | TITANIUM));
        assert!(tiers.cobalt.is_some() && tiers.mythril.is_some() && tiers.adamantite.is_some());
    }

    /// Which ore of a pair a world gets is decided once and never revisited.
    #[test]
    fn a_world_keeps_the_ore_it_rolled() {
        let mut tiers = OreTiers::default();
        let first = smash_one(0, &mut tiers, 4).unwrap();
        for round in 1..8 {
            let again = smash_one(round * 3, &mut tiers, round as u64 + 10).unwrap();
            assert_eq!(again.ore, first.ore, "the tier changed on round {round}");
            assert!(!again.decided_a_tier, "and claimed to decide it again");
        }
    }

    /// Both ores of a pair really do come up across enough worlds.
    #[test]
    fn both_ores_of_a_pair_happen() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..40u64 {
            let mut tiers = OreTiers::default();
            seen.insert(smash_one(0, &mut tiers, seed).unwrap().ore);
        }
        assert_eq!(
            seen,
            std::collections::HashSet::from([COBALT, PALLADIUM]),
            "one of the pair never turned up"
        );
    }

    /// Each altar gives less than the last.
    #[test]
    fn the_yield_falls_with_every_altar() {
        let mut tiers = OreTiers::default();
        // Seed all three so later rounds are comparing like with like.
        for i in 0..3 {
            smash_one(i, &mut tiers, 1);
        }
        let veins = |count: i32| {
            let mut copy = tiers;
            smash_one(count, &mut copy, 7).unwrap().veins.len()
        };
        let first_round = veins(0);
        let fourth_round = veins(9);
        let tenth_round = veins(27);
        assert!(
            first_round > fourth_round,
            "{first_round} then {fourth_round}"
        );
        assert!(
            fourth_round > tenth_round,
            "{fourth_round} then {tenth_round}"
        );
        assert!(tenth_round > 0, "but never nothing at all");
    }

    /// The deeper tiers really are placed deeper.
    #[test]
    fn each_tier_goes_deeper_than_the_last() {
        let mut tiers = OreTiers::default();
        let shallowest = |smashed: &Smashed| smashed.veins.iter().map(|(_, y, _, _)| *y).min();
        let cobalt = smash_one(0, &mut tiers, 8).unwrap();
        let mythril = smash_one(1, &mut tiers, 8).unwrap();
        let adamantite = smash_one(2, &mut tiers, 8).unwrap();
        assert!(shallowest(&cobalt) < shallowest(&mythril));
        assert!(shallowest(&mythril) < shallowest(&adamantite));
    }

    /// It always costs a wraith or two.
    #[test]
    fn every_altar_costs_a_wraith() {
        let mut tiers = OreTiers::default();
        for count in 0..12 {
            let smashed = smash_one(count, &mut tiers, count as u64).unwrap();
            assert!(
                (1..=2).contains(&smashed.wraiths),
                "{} wraiths",
                smashed.wraiths
            );
        }
    }

    /// A vein replaces stone and dirt, and nothing else.
    #[test]
    fn a_vein_only_eats_what_it_should() {
        let mut world = solid_stone();
        // A chest and a dungeon brick right where the vein is going.
        world.set_tile(250, 250, Tile::framed(21, 0, 0));
        world.set_tile(251, 250, Tile::block(41));
        let mut rng = SmallRng::seed_from_u64(9);
        let changed = run_vein(&mut world, (250, 250), 12.0, 12, COBALT, &mut rng);

        assert!(!changed.is_empty(), "it should have dug something");
        assert_eq!(world.tile(250, 250).block, 21, "the chest survived");
        assert_eq!(world.tile(251, 250).block, 41, "so did the brick");
        assert!(
            changed
                .iter()
                .all(|(x, y)| world.tile(*x, *y).block == COBALT),
            "and everything it did dig is ore"
        );
    }

    /// A vein is a vein rather than a scattering: what it digs is connected and roughly round.
    #[test]
    fn a_vein_is_a_blob_not_a_scattering() {
        let mut world = solid_stone();
        let mut rng = SmallRng::seed_from_u64(10);
        let changed = run_vein(&mut world, (250, 250), 10.0, 10, MYTHRIL, &mut rng);
        assert!(
            changed.len() > 20,
            "too small to be a vein: {}",
            changed.len()
        );
        let xs: Vec<i32> = changed.iter().map(|(x, _)| *x).collect();
        let ys: Vec<i32> = changed.iter().map(|(_, y)| *y).collect();
        let span = (
            xs.iter().max().unwrap() - xs.iter().min().unwrap(),
            ys.iter().max().unwrap() - ys.iter().min().unwrap(),
        );
        assert!(span.0 < 40 && span.1 < 40, "it wandered too far: {span:?}");
    }

    /// A vein in open air digs nothing, rather than filling the air with ore.
    #[test]
    fn a_vein_in_the_air_digs_nothing() {
        let mut world = Stone(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(11);
        assert!(run_vein(&mut world, (250, 250), 10.0, 10, COBALT, &mut rng).is_empty());
    }
}

// --- The wall falling, and the biomes creeping afterwards ----------------------------------------

use terrustia_proto::convert::{self, Biome};

/// Which biomes spread on their own, and which block starts a spread.
///
/// The list is deliberately short: only the *core* blocks of an evil push outward. A corrupt
/// thorn spreads, a shadow orb does not, and that is why an infection creeps from its stone rather
/// than from anything that happens to be standing in it.
pub fn spreads(block: u16) -> Option<Biome> {
    match block {
        23 | 24 | 25 | 32 | 112 | 163 | 398 | 400 | 636 | 661 => Some(Biome::Corruption),
        109 | 110 | 113 | 115 | 116 | 117 | 164 | 402 | 403 | 492 => Some(Biome::Hallow),
        199 | 200 | 201 | 203 | 205 | 234 | 352 | 399 | 401 | 662 => Some(Biome::Crimson),
        _ => None,
    }
}

/// Whether a block is one a spread can take.
///
/// Grass, stone, ice, moss, sand and sandstone, and nothing else. Everything a player built is
/// safe. Ice (161) was missing here even though all three spreads target it identically
/// (`WorldGen.cs`'s own `SpreadsCorruption`/`SpreadsCrimson`/`SpreadsHallow` blocks each end with
/// a `type == 161` case) — without it, a frozen cave was the one biome an infection could never
/// actually enter.
fn takeable(block: u16) -> bool {
    matches!(block, 1 | 2 | 53 | 60 | 69 | 161 | 396 | 397 | 477)
        || terrustia_proto::tile_sets::is_moss(block)
}

/// Sunflowers hold an infection off, which is the whole point of planting them.
const SUNFLOWER: u16 = 27;
/// How far out a spread looks for one.
const SUNFLOWER_REACH: i32 = 2;
/// How far a spread reaches from the tile that started it.
const SPREAD_REACH: i32 = 3;

/// `TileID.CrystalShards` — the harvestable Underground Hallow shard that regrows (L3-14).
const CRYSTAL_SHARD: u16 = 129;
/// `TileID.Mud`, `TileID.JungleGrass`, `TileID.ChlorophyteOre` and the chlorophyte plant
/// (`TileID.PlantDetritus`, 346) — the tiles the chlorophyte cycle reads and writes.
const MUD: u16 = 59;
const JUNGLE_GRASS: u16 = 60;
const CHLOROPHYTE_ORE: u16 = 211;
const CHLOROPHYTE_PLANT: u16 = 346;

/// Whether `x, y` is at least `margin` tiles inside every edge — vanilla's own `InWorld(x, y, m)`.
fn in_world(world: &impl OreWorld, x: i32, y: i32, margin: i32) -> bool {
    x >= margin && y >= margin && x < world.width() - margin && y < world.height() - margin
}

/// The Underground Hallow stone crystal shards grow on — `TileID.Sets.CanGrowCrystalShards`
/// (`TileID.cs:345`): pearlstone, pearlsand, hardened pearlsand and the two pearl-sandstone forms.
fn can_grow_crystal_shards(block: u16) -> bool {
    matches!(block, 116 | 117 | 164 | 402 | 403)
}

/// `Convert(..., 8)` (`WorldGen.cs:56148-56153`): mud or jungle grass becomes chlorophyte ore.
fn convert_to_chlorophyte(block: u16) -> Option<u16> {
    matches!(block, MUD | JUNGLE_GRASS).then_some(CHLOROPHYTE_ORE)
}

/// `Convert(..., 9)` (`WorldGen.cs:56154-56203`): a tile is purified back toward the jungle around
/// spreading chlorophyte. `Some(Some(block))` sets a new block, `Some(None)` kills the tile (a
/// thorn or vine), `None` leaves it alone.
fn convert_toward_jungle(block: u16) -> Option<Option<u16>> {
    match block {
        2 | 23 | 109 | 199 | 477 | 492 | 661 | 662 => Some(Some(JUNGLE_GRASS)),
        0 => Some(Some(MUD)),
        25 | 203 => Some(Some(1)),
        112 | 234 => Some(Some(53)),
        398 | 399 => Some(Some(397)),
        400 | 401 => Some(Some(396)),
        24 | 32 | 201 | 205 | 352 | 636 => Some(None),
        _ => None,
    }
}

/// Whether chlorophyte may grow at a tile, given how much chlorophyte ore already crowds it —
/// `WorldGen.CanChlorophyteGrow` (`WorldGen.cs:70002-70053`). Too dense in either the inner or the
/// outer radius and it refuses, which is what keeps a chlorophyte farm from filling a whole cavern.
/// Above the rock layer the caps halve and the radii grow by half. The remix-world adjustments to
/// these caps are not modelled, and as of 2026-09-05 that is moot for a world this server made:
/// `SecretSeeds::honoured_by_this_generator` drops `remix` at generation, because nothing here
/// mirrors the world and a client told `F::RemixWorld` draws one that is. The flag survives a
/// `.wld` real Terraria generated, where the tiles really are mirrored, and it is still read from
/// and written to the world file (`wld.rs:842`, `wld_save.rs:700`) and still consumed where it is
/// consumed at all - it switches town-NPC happiness off entirely (`happiness.rs:362`, from
/// `server/mod.rs`, transcribed from `ShopHelper.cs:107`). So this cap remains a real gap for such
/// a world, and only for such a world.
fn can_chlorophyte_grow(world: &impl OreWorld, i: i32, j: i32, rock_layer: i32) -> bool {
    let (mut inner_cap, mut outer_cap, mut inner_r, mut outer_r) = (40, 130, 35, 85);
    if j < rock_layer {
        inner_cap /= 2;
        outer_cap /= 2;
        inner_r = (f64::from(inner_r) * 1.5) as i32;
        outer_r = (f64::from(outer_r) * 1.5) as i32;
    }
    let ore_within = |r: i32| -> i32 {
        let mut n = 0;
        for k in i - r..i + r {
            for l in j - r..j + r {
                let t = world.tile(k, l);
                if t.is_active() && t.block == CHLOROPHYTE_ORE {
                    n += 1;
                }
            }
        }
        n
    };
    ore_within(inner_r) <= inner_cap && ore_within(outer_r) <= outer_cap
}

/// Whether enough chlorophyte crowds a tile to hold an infection off it — `WorldGen.nearbyChlorophyte`
/// (`WorldGen.cs:70055-70096`). Three within five tiles always counts; fewer is a weighted die roll.
fn nearby_chlorophyte(world: &impl OreWorld, i: i32, j: i32, rng: &mut SmallRng) -> bool {
    let r = 5;
    if i <= r + 5 || i >= world.width() - r - 5 || j <= r + 5 || j >= world.height() - r - 5 {
        return false;
    }
    let mut count = 0;
    for k in i - r..=i + r {
        for l in j - r..=j + r {
            let t = world.tile(k, l);
            if t.is_active() && matches!(t.block, CHLOROPHYTE_ORE | CHLOROPHYTE_PLANT) {
                count += 1;
                if count >= 3 {
                    return true;
                }
            }
        }
    }
    count > 0 && rng.random_range(-1..4) < count
}

/// Whether the tile above blocks a chlorophyte seed — one of the plants vanilla refuses to grow
/// under (`WorldGen.cs:70194`: a tree, and three cactus/plant types).
fn blocks_chlorophyte_above(world: &impl OreWorld, x: i32, y: i32) -> bool {
    let above = world.tile(x, y - 1);
    above.is_active() && matches!(above.block, 5 | 236 | 702 | 238)
}

/// Crystal shards and chlorophyte regrowing, from the front of `WorldGen.hardUpdateWorld`
/// (`WorldGen.cs:70144-70239`). This runs from the same world-wide sampled tiles the biome spread
/// does, and (like vanilla) before the infection-spread gate, so it is not stopped by the Journey
/// "Stop Biome Spread" power the way [`spread`] is. Returns the tiles it changed.
///
/// One simplification, disclosed: a placed crystal shard is given a single frame from its rolled
/// style rather than reproducing the multi-stage growth frames vanilla's `PlaceTile` builds — a
/// cosmetic detail the client renders, not a gameplay one.
pub fn regrow(
    world: &mut impl OreWorld,
    x: i32,
    y: i32,
    surface: i32,
    rock_layer: i32,
    rng: &mut SmallRng,
) -> Vec<(i32, i32)> {
    let mut changed = Vec::new();
    let here = world.tile(x, y);
    if !here.is_active() {
        return changed;
    }
    let block = here.block;

    // Crystal shards regrow on Underground Hallow stone below the rock layer, one attempt in five.
    if can_grow_crystal_shards(block) && y > rock_layer && rng.random_range(0..5) == 0 {
        // Left, right or down (twice as likely) — there is no upward case in vanilla's own switch.
        let (dx, dy) = match rng.random_range(0..4) {
            0 => (-1, 0),
            1 => (1, 0),
            _ => (0, 1),
        };
        let (nx, ny) = (x + dx, y + dy);
        if !world.tile(nx, ny).is_active() {
            let mut nearby = 0;
            for k in x - 6..=x + 6 {
                for l in y - 6..=y + 6 {
                    let t = world.tile(k, l);
                    if t.is_active() && t.block == CRYSTAL_SHARD {
                        nearby += 1;
                    }
                }
            }
            if nearby < 2 {
                let style = if rng.random_range(0..50) == 0 {
                    18 + rng.random_range(0..6)
                } else {
                    rng.random_range(0..18)
                };
                let shard =
                    terrustia_proto::tile::Tile::framed(CRYSTAL_SHARD, (style * 18) as i16, 0);
                world.set_tile(nx, ny, shard);
                changed.push((nx, ny));
            }
        }
    }

    // Chlorophyte cycles below the midpoint of the surface and rock layers.
    if y > (surface + rock_layer) / 2 {
        // Jungle grass rarely seeds chlorophyte ore into a patch of mud nearby.
        if block == JUNGLE_GRASS && rng.random_range(0..300) == 0 {
            let (tx, ty) = (
                x + rng.random_range(-10..=10),
                y + rng.random_range(-10..=10),
            );
            if in_world(world, tx, ty, 2)
                && world.tile(tx, ty).is_active()
                && world.tile(tx, ty).block == MUD
                && !blocks_chlorophyte_above(world, tx, ty)
                && can_chlorophyte_grow(world, tx, ty, rock_layer)
                && let Some(ore) = convert_to_chlorophyte(world.tile(tx, ty).block)
            {
                let mut t = world.tile(tx, ty);
                t.block = ore;
                world.set_tile(tx, ty, t);
                changed.push((tx, ty));
            }
        }
        // Chlorophyte ore and the chlorophyte plant creep into adjacent mud or jungle grass, and
        // purify a nearby tile back toward the jungle.
        if block == CHLOROPHYTE_ORE || block == CHLOROPHYTE_PLANT {
            if rng.random_range(0..3) != 0 {
                let (dx, dy) = match rng.random_range(0..4) {
                    0 => (1, 0),
                    1 => (-1, 0),
                    2 => (0, 1),
                    _ => (0, -1),
                };
                let (tx, ty) = (x + dx, y + dy);
                if in_world(world, tx, ty, 2)
                    && world.tile(tx, ty).is_active()
                    && matches!(world.tile(tx, ty).block, MUD | JUNGLE_GRASS)
                    && can_chlorophyte_grow(world, tx, ty, rock_layer)
                    && let Some(ore) = convert_to_chlorophyte(world.tile(tx, ty).block)
                {
                    let mut t = world.tile(tx, ty);
                    t.block = ore;
                    world.set_tile(tx, ty, t);
                    changed.push((tx, ty));
                }
            }
            let (tx, ty) = (x + rng.random_range(-6..=6), y + rng.random_range(-6..=6));
            if in_world(world, tx, ty, 2)
                && world.tile(tx, ty).is_active()
                && let Some(conv) = convert_toward_jungle(world.tile(tx, ty).block)
            {
                let mut t = world.tile(tx, ty);
                match conv {
                    Some(new_block) => t.block = new_block,
                    None => t = terrustia_proto::tile::Tile::AIR,
                }
                world.set_tile(tx, ty, t);
                changed.push((tx, ty));
            }
        }
    }
    changed
}

/// One attempt at spreading from a tile.
///
/// Returns where it took hold and what it became, or `None` if nothing did. It keeps trying from
/// the same tile while it keeps succeeding — half the time on each success — so an infection
/// advances in bursts rather than one tile at a time.
pub fn spread(
    world: &mut impl OreWorld,
    x: i32,
    y: i32,
    downed_plantera: bool,
    rng: &mut SmallRng,
) -> Vec<(i32, i32)> {
    let mut taken = Vec::new();
    let here = world.tile(x, y);
    if !here.is_active() {
        return taken;
    }
    let Some(biome) = spreads(here.block) else {
        return taken;
    };
    // Plantera's death halves the rate for good, which is what makes killing her the thing that
    // stops a world being eaten.
    if downed_plantera && rng.random_range(0..2) != 0 {
        return taken;
    }

    let mut again = true;
    while again {
        again = false;
        let (tx, ty) = (
            x + rng.random_range(-SPREAD_REACH..=SPREAD_REACH),
            y + rng.random_range(-SPREAD_REACH..=SPREAD_REACH),
        );
        if tx < 10 || ty < 10 || tx >= world.width() - 10 || ty >= world.height() - 10 {
            continue;
        }
        // Chlorophyte nearby holds the infection off — `nearbyChlorophyte`/`ChlorophyteDefense`
        // (`WorldGen.cs:70256-70259`). The push-back itself (`Convert(..., 10)`) is a remix-world
        // seed only, so in an ordinary world this simply refuses the spread onto this tile (L3-14).
        if nearby_chlorophyte(world, tx, ty, rng) {
            continue;
        }
        let target = world.tile(tx, ty);
        if !target.is_active() || !takeable(target.block) {
            continue;
        }
        // A sunflower nearby holds it off entirely.
        if (tx - SUNFLOWER_REACH..=tx + SUNFLOWER_REACH).any(|sx| {
            (ty - SUNFLOWER_REACH..=ty + SUNFLOWER_REACH).any(|sy| {
                let tile = world.tile(sx, sy);
                tile.is_active() && tile.block == SUNFLOWER
            })
        }) {
            continue;
        }
        let Some(made) = convert::block(target.block, biome) else {
            continue;
        };
        let mut converted = target;
        converted.block = made;
        world.set_tile(tx, ty, converted);
        taken.push((tx, ty));
        // Half the time it tries again from the same tile, which is what makes a spread arrive in
        // bursts rather than trickling.
        again = rng.random_range(0..2) == 0;
    }
    taken
}

/// Where the two hardmode stripes go when the Wall of Flesh falls.
///
/// Two diagonal Vs are driven down from the surface at a third and two-thirds across, one hallowed
/// and one of whatever evil the world has. Which side gets which is a coin flip, and the pair is
/// pushed away from the dungeon so neither swallows it.
pub fn hardmode_stripes(world_width: i32, dungeon_x: i32, rng: &mut SmallRng) -> [(i32, i32); 2] {
    let far = f64::from(rng.random_range(300..400)) * 0.001;
    let near = f64::from(rng.random_range(200..300)) * 0.001;
    let mut good = (f64::from(world_width) * far) as i32;
    let mut evil = (f64::from(world_width) * (1.0 - far)) as i32;
    let mut lean = 1;
    if rng.random_range(0..2) == 0 {
        std::mem::swap(&mut good, &mut evil);
        lean = -1;
    }
    // Whichever stripe is on the dungeon's side is pulled in toward the middle, so the dungeon is
    // never inside one.
    if dungeon_x < world_width / 2 {
        if evil < good {
            evil = (f64::from(world_width) * near) as i32;
        } else {
            good = (f64::from(world_width) * near) as i32;
        }
    } else if evil > good {
        evil = (f64::from(world_width) * (1.0 - near)) as i32;
    } else {
        good = (f64::from(world_width) * (1.0 - near)) as i32;
    }
    [(good, 3 * lean), (evil, 3 * -lean)]
}

/// Drive one stripe down through the world, converting as it goes.
///
/// Returns the tiles it changed.
/// The band width of one hardmode stripe, scaled to the world's own width — `WorldGen.cs:76969-
/// 76972`. Vanilla truncates the scaled value to an `int` (`num2 = (int)(num2 * num3)`) and uses
/// that whole number for both the reach test and the bounding box; keeping it fractional (L3-31)
/// runs a stripe a sub-tile wider than it should, which is invisible on a 4,200-wide world and
/// shows on a 6,400-wide one where the fractional part is larger.
fn stripe_width(world_width: i32, roll: i32) -> f64 {
    (f64::from(roll) * f64::from(world_width) / 4200.0).trunc()
}

pub fn run_stripe(
    world: &mut impl OreWorld,
    x: i32,
    drift_x: i32,
    into: Biome,
    rng: &mut SmallRng,
) -> Vec<(i32, i32)> {
    let mut changed = Vec::new();
    let width = stripe_width(world.width(), rng.random_range(200..250));
    let mut here = (f64::from(x), 0.0);
    let mut drift = (f64::from(drift_x), 5.0);
    // The drift wanders around the stripe's *own* starting lean, not around zero — GERunner
    // clamps `val2.X` to the fixed `speedX` parameter plus or minus one, never to the evolving
    // drift itself, which is why this is captured once here rather than read back from `drift.0`.
    let speed_x = f64::from(drift_x);

    loop {
        let x0 = ((here.0 - width * 0.5) as i32).max(0);
        let x1 = ((here.0 + width * 0.5) as i32).min(world.width());
        let y0 = ((here.1 - width * 0.5) as i32).max(0);
        let y1 = ((here.1 + width * 0.5) as i32).min(world.height() - 5);

        for tx in x0..x1 {
            for ty in y0..y1 {
                // Ragged edges, so a stripe is a torn band rather than a drawn one.
                let reach = width * 0.5 * (1.0 + f64::from(rng.random_range(-10..=10)) * 0.015);
                if (f64::from(tx) - here.0).abs() + (f64::from(ty) - here.1).abs() >= reach {
                    continue;
                }
                let tile = world.tile(tx, ty);
                let mut made = tile;
                let mut moved = false;
                if tile.is_active()
                    && let Some(block) = convert::block(tile.block, into)
                {
                    made.block = block;
                    moved = true;
                }
                if let Some(wall) = convert::wall(tile.wall, into) {
                    made.wall = wall;
                    moved = true;
                }
                if moved {
                    world.set_tile(tx, ty, made);
                    changed.push((tx, ty));
                }
            }
        }
        here = (here.0 + drift.0, here.1 + drift.1);
        drift.0 = (drift.0 + f64::from(rng.random_range(-10..=10)) * 0.05)
            .clamp(speed_x - 1.0, speed_x + 1.0);
        // GERunner's own `while (flag2)` runs until the stripe leaves the world on *any* side —
        // not only the bottom, which is all the old `here.1 < height - 5` loop condition checked.
        // A stripe that drifted hard enough sideways (once possible: the old +-4 clamp let the
        // random walk saturate far past any real lean) would otherwise carve sideways forever.
        if here.0 < -width
            || here.1 < -width
            || here.0 > f64::from(world.width()) + width
            || here.1 > f64::from(world.height()) + width
        {
            break;
        }
    }
    changed
}

#[cfg(test)]
mod spread_tests {
    use super::*;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    struct Ground(HashMap<(i32, i32), Tile>);

    impl OreWorld for Ground {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
        fn set_tile(&mut self, x: i32, y: i32, tile: Tile) {
            self.0.insert((x, y), tile);
        }
        fn width(&self) -> i32 {
            500
        }
        fn height(&self) -> i32 {
            500
        }
    }

    fn field() -> Ground {
        let mut tiles = HashMap::new();
        for x in 0..500 {
            for y in 0..500 {
                tiles.insert((x, y), Tile::block(1));
            }
        }
        Ground(tiles)
    }

    /// An infection takes the stone around it and turns it to its own.
    #[test]
    fn an_infection_spreads() {
        let mut world = field();
        world.set_tile(250, 250, Tile::block(25));
        let mut rng = SmallRng::seed_from_u64(1);
        let mut taken = Vec::new();
        for _ in 0..200 {
            taken.extend(spread(&mut world, 250, 250, false, &mut rng));
        }
        assert!(!taken.is_empty(), "it should have spread");
        for (x, y) in taken {
            assert_eq!(world.tile(x, y).block, 25, "and made ebonstone of it");
        }
    }

    /// A sunflower holds it off.
    #[test]
    fn a_sunflower_holds_it_back() {
        let mut world = field();
        world.set_tile(250, 250, Tile::block(25));
        // Sunflowers over every tile the spread could reach.
        for x in 245..256 {
            for y in 245..256 {
                world.set_tile(x, y, Tile::framed(SUNFLOWER, 0, 0));
            }
        }
        world.set_tile(250, 250, Tile::block(25));
        let mut rng = SmallRng::seed_from_u64(2);
        let mut taken = Vec::new();
        for _ in 0..500 {
            taken.extend(spread(&mut world, 250, 250, false, &mut rng));
        }
        assert!(taken.is_empty(), "it got through: {taken:?}");
    }

    /// It only takes terrain. A chest in its path survives.
    #[test]
    fn an_infection_leaves_furniture_alone() {
        let mut world = field();
        world.set_tile(250, 250, Tile::block(25));
        for x in 247..254 {
            for y in 247..254 {
                if (x, y) != (250, 250) {
                    world.set_tile(x, y, Tile::framed(21, 0, 0));
                }
            }
        }
        let mut rng = SmallRng::seed_from_u64(3);
        for _ in 0..500 {
            spread(&mut world, 250, 250, false, &mut rng);
        }
        for x in 247..254 {
            for y in 247..254 {
                if (x, y) != (250, 250) {
                    assert_eq!(world.tile(x, y).block, 21, "the chest at {x},{y} was eaten");
                }
            }
        }
    }

    /// Killing Plantera halves it.
    ///
    /// The measure is how many attempts it takes to eat everything within reach, not how much it
    /// eats: a spread saturates its neighbourhood either way, and only the pace differs.
    #[test]
    fn plantera_slows_the_spread() {
        let attempts_to_saturate = |downed: bool| {
            let mut world = field();
            world.set_tile(250, 250, Tile::block(25));
            let mut rng = SmallRng::seed_from_u64(4);
            let mut idle = 0;
            let mut attempts = 0;
            // Saturated once it has failed to take anything a hundred times running.
            while idle < 100 && attempts < 100_000 {
                attempts += 1;
                if spread(&mut world, 250, 250, downed, &mut rng).is_empty() {
                    idle += 1;
                } else {
                    idle = 0;
                }
            }
            attempts
        };
        let quick = attempts_to_saturate(false);
        let slow = attempts_to_saturate(true);
        assert!(
            slow > quick,
            "with Plantera down it took {slow} attempts, not more than {quick}"
        );
    }

    /// Ordinary stone does not spread anything.
    #[test]
    fn clean_ground_spreads_nothing() {
        let mut world = field();
        let mut rng = SmallRng::seed_from_u64(5);
        for _ in 0..500 {
            assert!(spread(&mut world, 250, 250, false, &mut rng).is_empty());
        }
    }

    /// An infection can take ice — all three spreads target it identically in vanilla, and this
    /// was the one type `takeable` left out.
    ///
    /// Fails before the fix: `takeable` did not list 161, so ice next to an infection was passed
    /// over as if it were something a player had built, and a frozen cave stayed clean forever.
    #[test]
    fn an_infection_can_take_ice() {
        let mut world = field();
        world.set_tile(250, 250, Tile::block(25)); // ebonstone, already corrupt
        world.set_tile(251, 250, Tile::block(161)); // ice, right beside it
        let mut rng = SmallRng::seed_from_u64(6);
        let mut taken = Vec::new();
        for _ in 0..500 {
            taken.extend(spread(&mut world, 250, 250, false, &mut rng));
        }
        assert!(
            taken.contains(&(251, 250)),
            "the ice tile should have been reachable"
        );
        assert_eq!(
            world.tile(251, 250).block,
            163,
            "and turned into corrupt ice"
        );
    }

    /// The two hardmode stripes go on opposite sides, and neither lands on the dungeon.
    #[test]
    fn the_stripes_avoid_the_dungeon() {
        for seed in 0..30u64 {
            let mut rng = SmallRng::seed_from_u64(seed);
            for dungeon in [400, 4000] {
                let [(good, _), (evil, _)] = hardmode_stripes(4200, dungeon, &mut rng);
                assert!((good - evil).abs() > 400, "too close: {good} and {evil}");
                assert!(
                    (good - dungeon).abs() > 300 || (evil - dungeon).abs() > 300,
                    "both stripes landed on the dungeon at {dungeon}"
                );
                for x in [good, evil] {
                    assert!((0..4200).contains(&x), "stripe at {x} is off the world");
                }
            }
        }
    }

    /// A world wide enough that a stripe's drift alone decides how far sideways it wanders,
    /// without the world's own edge cutting the run short. `tile`/`set_tile` are backed by a
    /// sparse map over an effectively solid world rather than a real fill, since the width needed
    /// to give the drift room is far too large to actually allocate a tile for every cell of.
    struct WideGround {
        changed: HashMap<(i32, i32), u16>,
        width: i32,
        height: i32,
    }

    impl OreWorld for WideGround {
        fn tile(&self, x: i32, y: i32) -> Tile {
            match self.changed.get(&(x, y)) {
                Some(&block) => Tile::block(block),
                None => Tile::block(1),
            }
        }
        fn set_tile(&mut self, x: i32, y: i32, tile: Tile) {
            self.changed.insert((x, y), tile.block);
        }
        fn width(&self) -> i32 {
            self.width
        }
        fn height(&self) -> i32 {
            self.height
        }
    }

    /// The stripe's own drift stays locked to within one of its starting lean — `GERunner` clamps
    /// `val2.X` to `speedX +/- 1`, not to a fixed range unrelated to the lean it was given. A
    /// large lean (20 here) makes the difference unmistakable: the fixed clamp keeps drift near
    /// 20 the whole way down, while a `+-4`-regardless-of-lean clamp would slam it down to at most
    /// 4 after the very first step.
    ///
    /// Fails on the code before this fix: with a lean of 20, the deepest tiles landed only a few
    /// hundred tiles from the start (`.clamp(-4.0, 4.0)` capped every step after the first at 4),
    /// not the couple of thousand a lean of 20 actually carried down.
    #[test]
    fn the_stripes_drift_stays_near_its_own_large_lean() {
        let mut world = WideGround {
            changed: HashMap::new(),
            width: 10_000,
            height: 1000,
        };
        let mut rng = SmallRng::seed_from_u64(20);
        let speed_x = 20;
        let x0 = 5000;
        let changed = run_stripe(&mut world, x0, speed_x, Biome::Hallow, &mut rng);
        assert!(!changed.is_empty(), "it should have converted something");

        let deepest_y = changed.iter().map(|(_, y)| *y).max().unwrap();
        let deepest_x = changed
            .iter()
            .filter(|(_, y)| *y == deepest_y)
            .map(|(x, _)| *x)
            .max()
            .unwrap();
        // A clamp locked to 19..=21 covers roughly `deepest_y / 5 * 19` of drift by the time it
        // reaches this deep; a clamp capped at 4 regardless of the lean could only ever have
        // covered about a fifth of that. The threshold sits well above what the old code could
        // reach and well below what the fixed code reliably does, so it separates the two rather
        // than depending on exactly how far this particular run got.
        let minimum_expected = x0 + deepest_y * 3;
        assert!(
            deepest_x > minimum_expected,
            "the deepest tile ({deepest_x}) should be far right of {minimum_expected}, \
             given a lean of {speed_x} the clamp should have stayed close to"
        );
    }

    /// A stripe converts a band from the top of the world to the bottom.
    #[test]
    fn a_stripe_reaches_the_bottom() {
        let mut world = field();
        let mut rng = SmallRng::seed_from_u64(6);
        let changed = run_stripe(&mut world, 250, 3, Biome::Hallow, &mut rng);
        assert!(!changed.is_empty(), "it should have converted something");
        let deepest = changed.iter().map(|(_, y)| *y).max().unwrap();
        assert!(deepest > 400, "it should reach the depths: {deepest}");
        for (x, y) in &changed {
            assert_eq!(world.tile(*x, *y).block, 117, "pearlstone");
        }
    }

    /// L3-31: a stripe's band width is truncated to a whole number of tiles, the way vanilla's
    /// `(int)(num2 * num3)` is (`WorldGen.cs:76969-76972`).
    ///
    /// Fails before the fix: the width was kept fractional, running a stripe a sub-tile wider than
    /// the game does — invisible at 4,200 wide, real at 6,400.
    #[test]
    fn the_stripe_width_is_truncated_to_whole_tiles() {
        // 249 * 6400 / 4200 = 379.43..., truncated to 379.
        assert_eq!(stripe_width(6400, 249), 379.0);
        // At 4,200 wide the scale is exactly 1, so the roll passes through unchanged.
        assert_eq!(stripe_width(4200, 233), 233.0);
    }

    /// L3-14: crystal shards regrow on Underground Hallow stone below the rock layer, onto a free
    /// neighbour (`WorldGen.cs:70144-70187`).
    ///
    /// Fails before the fix: `regrow` did not exist, so a harvested shard patch never came back.
    #[test]
    fn crystal_shards_regrow_on_underground_hallow_stone() {
        let mut world = field();
        world.set_tile(250, 250, Tile::block(117)); // pearlstone
        for free in [(249, 250), (251, 250), (250, 251)] {
            world.set_tile(free.0, free.1, Tile::AIR);
        }
        let mut rng = SmallRng::seed_from_u64(1);
        let mut grew = false;
        for _ in 0..3000 {
            if !regrow(&mut world, 250, 250, 100, 200, &mut rng).is_empty() {
                grew = true;
                break;
            }
        }
        assert!(grew, "a crystal shard should have grown");
        assert!(
            [(249, 250), (251, 250), (250, 251)]
                .iter()
                .any(|&(x, y)| world.tile(x, y).block == 129),
            "and it should be a crystal shard on a free neighbour"
        );
    }

    /// L3-14: chlorophyte ore creeps into adjacent mud (`WorldGen.cs:70199-70226`, `Convert(...,8)`).
    ///
    /// Fails before the fix: chlorophyte never spread, so a harvested vein never regrew.
    #[test]
    fn chlorophyte_creeps_into_nearby_mud() {
        let mut world = field();
        world.set_tile(250, 250, Tile::block(211)); // chlorophyte ore
        world.set_tile(251, 250, Tile::block(59)); // mud beside it
        let mut rng = SmallRng::seed_from_u64(2);
        let mut spread_in = false;
        for _ in 0..2000 {
            regrow(&mut world, 250, 250, 100, 200, &mut rng);
            if world.tile(251, 250).block == 211 {
                spread_in = true;
                break;
            }
        }
        assert!(
            spread_in,
            "chlorophyte should have crept into the adjacent mud"
        );
    }

    /// L3-14: chlorophyte nearby holds an infection off — `nearbyChlorophyte`/`ChlorophyteDefense`
    /// (`WorldGen.cs:70256-70259`).
    ///
    /// Fails before the fix: `spread` had no chlorophyte check, so an infection ate straight
    /// through it.
    #[test]
    fn chlorophyte_holds_off_an_infection() {
        let corrupts_the_stone = |chlorophyte: bool| -> bool {
            let mut world = field();
            world.set_tile(250, 250, Tile::block(25)); // ebonstone source
            world.set_tile(248, 248, Tile::block(1)); // a stone target within reach
            if chlorophyte {
                for x in 246..251 {
                    for y in 246..251 {
                        if (x, y) != (250, 250) && (x, y) != (248, 248) {
                            world.set_tile(x, y, Tile::block(211));
                        }
                    }
                }
            }
            let mut rng = SmallRng::seed_from_u64(7);
            for _ in 0..2000 {
                spread(&mut world, 250, 250, false, &mut rng);
            }
            world.tile(248, 248).block == 25
        };
        assert!(
            corrupts_the_stone(false),
            "without chlorophyte the stone beside the source should have corrupted"
        );
        assert!(
            !corrupts_the_stone(true),
            "a dense chlorophyte cluster should have held the infection off that same tile"
        );
    }
}
