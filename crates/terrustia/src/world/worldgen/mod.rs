//! World generation.
//!
//! This builds a world that can be **played through**: every biome, a dungeon, an underworld, an
//! evil with orbs in it, altars, a temple, chests, life crystals. Not a vanilla-identical world —
//! the same seed here and in Terraria produce different maps — but a complete one.
//!
//! The distinction is the whole design and is worth being plain about. Seed-identical generation
//! means transcribing a hundred and six passes in exact order with exact random-number
//! consumption; it is measured in months and its progress is tracked in `docs/worldgen-parity.md`,
//! steered by the oracle in [`manifest`] and [`passes`]. What is here instead is the fallback
//! that plan names: every structure present, built with our own algorithms, beatable but not
//! identical. It exists because a server that generates unplayable worlds is not a working
//! server, and that is a much nearer target than parity.
//!
//! Passes run in a fixed order and each carves into what the last left:
//!
//! 1. [`layout`] decides where everything goes, before any tile is written.
//! 2. [`terrain`] lays the surface line and the layers, with biome materials over both.
//! 3. Caves are dug through the stone.
//! 4. Ore is seeded into what is left of it, in depth bands.
//! 5. The structures go in: evil chasms, the dungeon, the temple, the hive, the underworld.
//! 6. Altars, life crystals and chests fill the space that is left.
//! 7. Grass, plants and cobwebs finish it.
//!
//! Ordering is load-bearing rather than tidy. Ore seeded before the caves would be hollowed back
//! out; chests placed before the caves would have nowhere to stand.
//!
//! [`secret_seed`] covers vanilla's seven magic seed strings ("get fixed boi" among them) — the
//! real activation trigger, which of the seven this session actually wires to a behavioural
//! difference (one: [`traps`]'s own `noTrapsWorldGen` short-circuit), and a precise sizing note
//! for each of the other six, deferred. [`build_from_text`]/[`generate_from_text`] are the entry
//! points that actually check seed text against it; [`build`]/[`generate`] never do.

pub mod cave_flood;
pub mod dirt_wall_cleanup;
pub mod dunes;
pub mod enchanted_sword;
pub mod fallen_logs;
pub mod floating_islands;
pub mod gem_caves;
pub mod genpipe;
pub mod jungle_shrines;
pub mod lakes;
pub mod layout;
pub mod liquid_settle;
pub mod living_trees;
pub mod manifest;
pub mod micro_biomes;
pub mod mining_explosives;
pub mod moss;
pub mod oasis;
pub mod passes;
pub mod piles;
pub mod place_object;
pub mod pots;
pub mod pyramids;
pub mod rand;
pub mod scenery;
pub mod secret_seed;
pub mod shape_data;
pub mod smooth;
pub mod speleothems;
pub mod spider_caves;
pub mod statue_gen;
pub mod structure_map;
pub mod structures;
pub mod surface_plants;
pub mod terrain;
pub mod thin_ice;
pub mod tile_cleanup;
pub mod tiles;
pub mod traps;
pub mod underground_cabins;
pub mod underworld_ruins;
pub mod wall_variety;
pub mod water_plants;
pub mod waterfalls;

pub use passes::compare_against;

use layout::{Evil, Layout};
use rand::UnifiedRandom;
use secret_seed::SecretSeeds;
use tiles::{COPPER, GOLD, IRON, SILVER};

use super::World;

/// Standard "small" world dimensions.
pub const SMALL_WIDTH: i32 = 4200;
pub const SMALL_HEIGHT: i32 = 1200;

/// What a generated world came out holding.
///
/// Returned so callers — and the tests that guard this — can assert a world is playable rather
/// than merely non-empty. Every count here gates something; see [`structures`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Built {
    /// How many surface lakes were carved.
    pub lakes: usize,
    /// How many trees the forest pass grew.
    pub trees: usize,
    /// Vine tiles hung, and cacti grown.
    pub vines: usize,
    pub cacti: usize,
    pub orbs: usize,
    pub altars: usize,
    pub life_crystals: usize,
    pub chests: usize,
    pub hive: bool,
    pub pots: usize,
    pub statues: usize,
    pub piles: usize,
    pub small_piles: usize,
    pub fallen_logs: usize,
    pub flowers: usize,
    pub mushrooms: usize,
    pub herbs: usize,
    pub sunflowers: usize,
    /// Whether the lakes, oceans and underworld lava all reached a stable rest state. Always
    /// `true` outside a bug in the reused liquid simulator — see [`liquid_settle::Report`].
    pub liquids_converged: bool,
    pub dart_traps: usize,
    pub mines: usize,
    pub geysers: usize,
    pub boulder_traps: usize,
    pub sand_traps: usize,
    /// Tiles the closing smoothing pass turned into slopes, pounded half-tiles or cleared away.
    /// See [`smooth::Report`] for the breakdown; this is [`smooth::Report::total`].
    pub smoothed: usize,
    /// The first Tier 2 batch: gem-lined pockets, desert oases, cobweb-lined spider caves.
    pub gem_caves: usize,
    pub oases: usize,
    pub spider_caves: usize,
    /// Small hollow jungle-grass huts, each holding a chest and a torch.
    pub jungle_shrines: usize,
    /// Solid sandstone-brick masses buried in the desert, each with a tunnel to a treasure room.
    pub pyramids: usize,
    /// Small furnished houses found underground, in whichever material dominates the site.
    pub underground_cabins: usize,
    /// Ruined ash/hellstone-brick rooms scattered along the underworld surface.
    pub underworld_ruins: usize,
    /// Real Hellforges, each sited against a ruin's own background wall.
    pub hellforges: usize,
    /// Living-Wood trunk-and-branch trees, each with a taproot and one hollow chamber.
    pub living_trees: usize,
    /// Sky landmasses grown from a random-walk blob, most holding a house.
    pub floating_islands: usize,
    /// How many of those islands actually got a house built on them.
    pub floating_island_houses: usize,
    /// The lake-topped floating island variant — a water-filled basin instead of a house.
    pub cloud_lakes: usize,
    /// Frozen-pond patches of breakable ice, in the snow.
    pub thin_ice: usize,
    /// Surface dune fields: rolling sand hills over the desert instead of a flat shelf.
    pub dune_fields: usize,
    /// Rigged ore veins: explosives wired to a detonator. Zero under No Traps World.
    pub rigged_veins: usize,
    /// Enchanted Sword shrines: a flooded, vine-hung cavity with a sword in a dirt mound.
    pub sword_shrines: usize,
    /// Ebonstone sinkholes with a hollow core, Corruption-only.
    pub corruption_pits: usize,
    /// Stone hollows with a spiked floor.
    pub spike_pits: usize,
    /// Honeycomb-capped honey pools in the jungle.
    pub honey_patches: usize,
    /// Surface campfires with a tent and props.
    pub campsites: usize,
    /// Marble-painted cavern pockets.
    pub marble: usize,
    /// Granite-painted cavern pockets (a disclosed structural stand-in, not vanilla's real
    /// algorithm — see `micro_biomes`'s own module doc).
    pub granite: usize,
    /// Wall cells stripped from the surface crust above open cave space (`DirtWallCleanup`).
    pub dirt_wall_cleared: usize,
    /// Tiles pounded into a half-brick lip by `Waterfalls`.
    pub waterfalls: usize,
    /// Breakable-ice tiles crusted over ice-biome pond surfaces (`FragileIceOverIceBiomeWater` —
    /// distinct from [`Built::thin_ice`] above, which is the Tier 2 `ThinIceBiome` micro-biome
    /// count; see `thin_ice.rs`'s own module doc on why the two are not the same pass).
    pub fragile_ice: usize,
    /// Cavern pockets painted with depth/biome wall variety (`CaveWallVariety`).
    pub cave_wall_variety: usize,
    /// Enclosed pockets filled with wall so they don't read as bottomless voids
    /// (`CaveWallsInEnclosedSpaces`).
    pub cave_walls_enclosed: usize,
    /// Pockets flood-painted with moss plus isolated stone tiles converted to moss stone
    /// (`MossAndMossCaves`).
    pub moss_changed: usize,
    /// Decorative overlay tiles hung off exposed moss surfaces (`LongMoss`).
    pub long_moss: usize,
    /// Ceiling stalactites and floor stalagmites placed (`SpeleothemsAndGemTrees`, gem-tree branch
    /// disclosed-skipped — see `speleothems.rs`'s own module doc).
    pub speleothems: usize,
    /// Liquid tiles behind Hive wall converted to honey (`WebsInSpiderCavesAndHoneyPlus...`).
    pub honey_marked: usize,
    /// Stalactites placed specifically off Hive-walled ceilings, in the same shared pass.
    pub hive_stalactites: usize,
    /// Cobweb tiles scattered near spider-cave wall, in the same shared pass.
    pub cobwebs_placed: usize,
    /// Small gem tiles exposed in the ice biome (`ExposedGemsInIceBiome`).
    pub exposed_gems_ice: usize,
    /// Small gem tiles exposed underground, both the general pocket scatter and the desert-wall
    /// cluster scatter (`ExposedGemsUnderground`).
    pub exposed_gems_underground: usize,
    /// Cacti grown in the middle desert band (`CactusPalmTreesAndCoral`, palm-tree branch
    /// disclosed-skipped — see `water_plants.rs`'s own module doc).
    pub cacti_desert: usize,
    /// Coral and seashell tiles placed at the two ocean beaches, in the same pass.
    pub beach_decorations: usize,
    /// Lily pads placed on still surface water (`LilypadsCattailsBambooAndSeaweed`, bamboo/seaweed
    /// branches disclosed-skipped).
    pub lily_pads: usize,
    /// Cattails placed on still surface water, in the same pass.
    pub cattails: usize,
    /// Tiles dropped by `GravitatingSandCleanup`.
    pub gravitating_sand: usize,
    /// Tiles changed by `QuickCleanup` (desert-wall material normalisation, degenerate-slope
    /// straightening — see `tile_cleanup.rs`'s own module doc for what's narrowed).
    pub quick_cleanup: usize,
    /// Ore/stone blobs placed near the surface (`SurfaceOreAndStone`, a disclosed narrower
    /// stand-in for `OrePatch`/`StonePatch` — see `tile_cleanup.rs`'s own module doc).
    pub surface_ore_and_stone: usize,
    /// Grass-wall conversion sites flooded (`SurfaceDirtWallsToGrassWalls`, wall-conversion half
    /// only).
    pub surface_dirt_walls_to_grass_walls: usize,
    /// Tiles changed by `TileCleanup` (the three narrow fixups plus the thin-ice cleanup kept —
    /// see `tile_cleanup.rs`'s own module doc).
    pub tile_cleanup: usize,
    /// Tiles cleared by `BrokenTrapCleanup`'s real wire-circuit flood.
    pub broken_trap_cleanup: usize,
    /// Tiles changed by `FinalCleanup` (the five fixups kept — see `tile_cleanup.rs`'s own module
    /// doc).
    pub final_cleanup: usize,
    /// Which of vanilla's real secret-seed flags the seed text named, if any — every flag `false`
    /// for both an ordinary numeric seed and a text seed that matched nothing. Only set by
    /// [`build_from_text`]/[`generate_from_text`] and [`build_with_secret_seed`]; [`build`] and
    /// [`generate`] always leave this at [`SecretSeeds::none`], the same as an ordinary vanilla
    /// numeric seed would. See [`secret_seed`]'s own module doc for the real activation mechanism
    /// and exactly which of these actually change generation.
    pub secret_seeds: SecretSeeds,
}

/// Generate a world of the given size.
pub fn generate(width: i32, height: i32, name: impl Into<String>, seed: u64) -> World {
    build(width, height, name, seed).0
}

/// Generate a world, and say what went into it.
///
/// Always an ordinary world — no secret seed is ever active on this path, matching a plain
/// numeric seed typed into real vanilla's own seed field. See [`build_from_text`] for the path
/// that actually recognises the seven magic strings, and [`secret_seed`]'s own module doc for why
/// this project's ~40 existing callers of `build`/`generate` are left alone rather than all
/// growing a new required parameter for a feature only one of them (`main.rs`) can ever supply
/// real text for.
pub fn build(width: i32, height: i32, name: impl Into<String>, seed: u64) -> (World, Built) {
    build_with_secret_seed(width, height, name, seed, SecretSeeds::none())
}

/// Generate a world from the exact text typed into a seed field, the way a real player (or a
/// dedicated server's own `--seed` flag) would — a plain number reproduces that numeric seed,
/// free text is hashed into one instead, and either way the text itself is checked against
/// vanilla's real secret-seed magic strings (and, for two of them, specific numbers). See
/// [`secret_seed`]'s own module doc for the real mechanism, now checked directly against source.
///
/// Unlike [`generate`], the generated [`World::seed_text`] is the trimmed *original text*, not
/// the derived number — matching what real vanilla's own world-creation UI shows back afterward
/// (typing "get fixed boi" and later being told your seed was some large hashed integer would be
/// actively misleading).
pub fn generate_from_text(
    width: i32,
    height: i32,
    name: impl Into<String>,
    seed_text: &str,
) -> World {
    build_from_text(width, height, name, seed_text).0
}

/// [`generate_from_text`], and say what went into it. See its own doc comment.
pub fn build_from_text(
    width: i32,
    height: i32,
    name: impl Into<String>,
    seed_text: &str,
) -> (World, Built) {
    let secret = SecretSeeds::detect(seed_text);
    let seed = secret_seed::numeric_seed(seed_text);
    let (mut world, built) = build_with_secret_seed(width, height, name, seed, secret);
    // Overwrite the numeric-seed text `build_with_secret_seed` already recorded below with the
    // real typed text — see this function's own doc comment for why that's the honest thing to
    // show back, not the derived number.
    world.seed_text = seed_text.trim().to_string();
    (world, built)
}

/// [`build`], with a secret seed already decided rather than detected from text.
///
/// The real integration point every other function on this page delegates to — split out so
/// [`build`] (~40 existing callers across this workspace, none of which have real seed text to
/// give it) can keep its exact original signature while [`build_from_text`] (the one real caller
/// that does) gets a genuine hook to thread detected [`SecretSeeds`] through to whichever passes
/// need to branch on it. See [`secret_seed`]'s own module doc for exactly which passes that is —
/// currently only [`traps::scatter`], for [`SecretSeeds::no_traps`].
pub fn build_with_secret_seed(
    width: i32,
    height: i32,
    name: impl Into<String>,
    seed: u64,
    secret: SecretSeeds,
) -> (World, Built) {
    let mut world = World::empty(width, height, name);
    // What this generator actually made, not what was asked for. See
    // [`SecretSeeds::honoured_by_this_generator`]: a flag that describes the world's shape is a
    // claim the tiles have to back up, because a client is told it and draws by it.
    let (honoured, dropped) = secret.honoured_by_this_generator();
    if !dropped.is_empty() {
        tracing::warn!(
            seeds = dropped.join(", "),
            "this server cannot generate that world shape; \
             an ordinary world was made and the flag is not claimed"
        );
    }
    world.secret_seeds = honoured;
    // The same generator the parity work uses, so a seed means the same thing in both.
    let mut rand = UnifiedRandom::new(seed as i32);

    let plan = Layout::plan(width, height, &mut rand);
    // Shared across every Tier 2 pass that sites a set-piece structure — `GenVars.structures` in
    // vanilla, one session-global instance so a jungle shrine and an underground cabin (once that
    // lands) never overlap each other, the same way they cannot in a real generated world.
    let mut structures = structure_map::StructureMap::new();
    world.id = rand.next();
    for byte in &mut world.unique_id {
        *byte = rand.next_max(256) as u8;
    }
    world.crimson = plan.evil == Evil::Crimson;
    // What `structures::ores` actually lays down. The hardmode three stay unchosen at -1 until
    // the first three altars are broken.
    world.ore_tiers = [
        COPPER as i16,
        IRON as i16,
        SILVER as i16,
        GOLD as i16,
        -1,
        -1,
        -1,
    ];
    world.seed_text = seed.to_string();
    // The sky and the treetops. Rolled here, before the terrain passes, because the tree tops are
    // derived from the backdrops and both go out in packet 7 whatever the tiles turn out like.
    scenery::choose(&mut world, &mut rand);
    world.surface = plan.surface as i16;
    world.rock_layer = plan.rock as i16;
    world.dungeon_x = Some(plan.dungeon_x);

    let heights = terrain::heightmap(&plan, &mut rand);
    terrain::fill(&mut world, &plan, &heights, &mut rand);

    structures::caves(&mut world, &plan, &mut rand);
    structures::ores(&mut world, &plan, &mut rand);

    // `GravitatingSandCleanup` (pass 36) then `DirtWallCleanup` (39) — both run here in vanilla's
    // own pass order too, well before `SmoothWorld` (53) and every decorative pass after it. Both
    // only reshape/strip what the terrain and caves passes just above already created, so running
    // them this early matches vanilla and needs nothing placed later.
    let gravitating_sand = tile_cleanup::gravitating_sand_cleanup(&mut world, &plan);
    let dirt_wall_cleared = dirt_wall_cleanup::scrub(&mut world, &plan, &mut rand);

    let orbs = structures::evil_chasms(&mut world, &plan, &heights, &mut rand);
    structures::dungeon(&mut world, &plan, &heights, &mut rand);
    structures::temple(&mut world, &plan, &mut rand);
    let hive = structures::hive(&mut world, &plan, &mut rand);
    structures::underworld(&mut world, &plan, &mut rand);

    let altars = structures::altars(&mut world, &plan, &mut rand);
    let life_crystals = structures::life_crystals(&mut world, &plan, &mut rand);
    let chests = structures::chests(&mut world, &plan, &mut rand);

    structures::greenery(&mut world, &plan, &heights, &mut rand);
    structures::cobwebs(&mut world, &plan, &mut rand);

    // A forest, last of the surface passes so it grows on grass that has stopped moving.
    //
    // Generated worlds had no trees whatsoever until this — not fewer than vanilla, none — which
    // is the first thing anyone notices and the hardest to work around, since wood is the first
    // material in the game.
    //
    // Its own generator, seeded from the world's, because the tree pass draws a different number
    // of values than anything before it and threading it through `rand` would move every later
    // pass's numbers for no benefit while parity is not the goal.
    // `::rand` rather than `rand`: this module has its own `rand` submodule, holding the game's
    // generator, and it wins the name.
    let mut forest_rng = {
        use ::rand::SeedableRng;
        ::rand::rngs::SmallRng::seed_from_u64(seed ^ 0x7265_6573)
    };
    // Lakes before the greenery that would otherwise be drowned by them, and before the trees,
    // which refuse to grow where there is water.
    let lakes = lakes::carve(&mut world, &plan, &heights, &mut rand);

    // Settle every lake, ocean edge and underworld lava pool before anything checks "is this tile
    // wet" — trees refuse to grow into water, and until the water they're checking against has
    // actually come to rest, that check is answering a question about a world that no longer
    // exists a moment later. Reuses the runtime liquid simulator rather than vanilla's own
    // generation-time algorithm (see `liquid_settle`'s own doc comment for why); measured at
    // 19.5ms on a real 4200x1200 world, so it costs nothing worth noticing at generation time.
    let liquids = liquid_settle::settle(&mut world);
    debug_assert!(
        liquids.converged,
        "liquid settling did not converge; a generated world would ship with moving water"
    );

    // Pyramids, before essentially every decoration pass — vanilla's own `Pyramids` pass
    // (`WorldGen.cs:15438`) is the earliest of this trio, ahead of `LivingTrees` (15562) and
    // `Oasis` (16338) alike, though it rarely interacts with either in practice: a pyramid's own
    // site check only ever looks at the one point it starts digging from, not a wide window the
    // way oasis does.
    // Dunes before pyramids, because vanilla runs `DunesBiome` inside the same desert pass and
    // ahead of the pyramid siting loop (`WorldGen.cs:11573`), and a pyramid sited against a flat
    // shelf that then grows dunes on top of it would end up buried.
    let dune_fields = dunes::scatter(&mut world, &plan, &mut structures, &mut rand);

    let pyramids = pyramids::scatter(&mut world, &plan, &mut rand, &mut forest_rng);

    // Living trees, right after pyramids — vanilla's own `LivingTrees` pass (`WorldGen.cs:15562`)
    // runs between `Pyramids` (15438) and `Oasis` (16338), before anything below gets a chance to
    // leave a tile in the 100-wide clear footprint a trunk needs.
    let living_trees = living_trees::scatter(&mut world, &plan, &mut rand);
    living_trees::scatter_walls(&mut world, &plan);

    // Desert oases, last of this trio and still before anything further below gets a chance to
    // drop a decoration onto the desert surface. Its own siting check requires every active tile
    // in a wide scan window to be plain sand, faithful to vanilla — where the same check works
    // because vanilla's own `Oasis` pass (`WorldGen.cs:16338`) runs before essentially every
    // decorative pass, including `Statues` (16962), `PotsGraveyardsAndBoulderPiles` (18123) and,
    // notably, cacti themselves (`CactusPalmTreesAndCoral`, 21488 — the very end of vanilla's own
    // pass list). This module first ran after `plant_undergrowth` (which plants cacti) and,
    // later, after pots/statues too — both left desert columns carrying a decoration the scan
    // window's "must be plain sand" check would fail on, and oases placed zero on every real
    // world tried either way. Moving it before any of that runs matches vanilla's own order and
    // is the actual fix; nothing about `try_place` itself was wrong. It used to also run *before*
    // `Pyramids`/`LivingTrees` above, contradicting both vanilla and this very comment block's own
    // stated order (`WorldGen.cs:15438/15562/16338` puts Oasis strictly last of the three) — moved
    // here to match.
    let oases = oasis::scatter(&mut world, &plan, &mut rand);

    // Floating islands: entirely in the sky, well above `plan.surface`, so — unlike every other
    // Tier 2 pass above — its relative order against ground-level passes is genuinely inert rather
    // than merely convenient; placed here, alongside its fellow Tier 2 scatters, for readability
    // rather than because anything below would otherwise conflict with it. Vanilla itself runs the
    // driving `FloatingIslands` pass far earlier (`WorldGen.cs:12988`, before even `OresAndShinies`)
    // and `FloatingIslandHouses` much later (`:17986`, after the underground-cabins-equivalent
    // pass) — the two are merged into one call here; see `floating_islands`'s own module doc.
    let floating_islands = floating_islands::scatter(&mut world, &plan, &mut structures, &mut rand);

    // The jungle first: vines hang from grass, and until its mud was lined there was almost none.
    crate::world::trees::grass_the_jungle(&mut world);
    let trees = crate::world::trees::plant_forest(&mut world, &mut forest_rng);
    // Vines and cacti reuse the runtime growers, which already know the rules. A jungle without
    // vines reads as a green cave.
    let (vines, cacti) = crate::world::trees::plant_undergrowth(&mut world, &mut forest_rng);

    // Jungle shrines need real jungle grass under a candidate site, so this runs right after the
    // jungle itself is grassed — and before pots/statues/piles below get a chance to clutter a
    // floor tile a shrine's own clearance scan would then refuse.
    let jungle_shrines = jungle_shrines::scatter(&mut world, &plan, &mut structures, &mut rand);

    // Micro-biomes: real vanilla splits this across three passes at very different points in its
    // own pipeline (`Marble`/`Granite` run right after `Terrain`, long before `FloatingIslands`;
    // `MicroBiomes` itself runs much later, after every structure pass above). Merged into one call
    // here, placed after jungle grass exists (`honey_patch`'s own site check needs it, the same way
    // `jungle_shrines` above does) rather than split to match each vanilla pass's own timing — see
    // `micro_biomes`'s own module doc for exactly which of the 15 real classes this covers.
    let micro_biomes = micro_biomes::scatter(&mut world, &plan, &mut structures, &mut rand);

    // The Enchanted Sword shrine, the sixteenth `MicroBiome` class and the first built on the real
    // shape/modifier/action pipeline (`genpipe`). Vanilla runs it inside its own `MicroBiomes`
    // pass; it is a separate call here only because it is a separate module.
    let sword_shrines = enchanted_sword::scatter(&mut world, &plan, &mut structures, &mut rand);

    // Rigged ore veins: explosives wired to a detonator. Vanilla runs these in the same block, and
    // skips them entirely under No Traps World, which this honours.
    let rigged_veins =
        mining_explosives::scatter(&mut world, &plan, &mut structures, &mut rand, honoured);

    // Pots, statues, piles and fallen logs: the small object-placement passes built on
    // `place_object`. Ground-truth loot and decoration that makes a cave look excavated rather
    // than merely hollow.
    let pots = pots::scatter(&mut world, &plan, &mut forest_rng);
    let statues = statue_gen::scatter(&mut world, &plan, &mut forest_rng);
    let (piles, small_piles) = piles::scatter(&mut world, &plan, &mut forest_rng);
    let fallen_logs = fallen_logs::scatter(&mut world, &plan, &mut forest_rng);

    // The surface decoration passes: flowers upgrading existing plants, mushroom-cap frames near
    // that biome, alchemy herbs, and sunflowers. All four use the game's own shared generator
    // (`rand`), same as the terrain and structure passes above them, rather than `forest_rng` —
    // matching the convention `structures::greenery` already established for surface plant work.
    let flowers = surface_plants::flowers(&mut world, &mut rand);
    let mushrooms = surface_plants::mushrooms(&mut world, &mut rand);
    let herbs = surface_plants::herbs(&mut world, &mut rand);
    let sunflowers = surface_plants::sunflowers(&mut world, &mut rand);

    // Traps, after every other decoration so a trap never gets sited under a pot, statue or pile
    // that outranks it for the same floor tile — same relative order vanilla's own `Traps` pass
    // has against `Piles`.
    let traps = traps::scatter(&mut world, &plan, &mut forest_rng, secret);

    // The rest of Tier 2's first batch: gem-lined pockets and cobweb-lined spider caves both site
    // into the caves `structures::caves()` already carved (and, since that pass's own fix, left
    // genuinely unwalled) — after every other cave decoration above so they don't overwrite a
    // pot, statue or pile that got there first, matching the same reasoning traps' own ordering
    // comment gives. Oases are placed earlier, above, alongside their own ordering rationale.
    let gem_caves = gem_caves::scatter(&mut world, &plan, &mut rand);
    let spider_caves = spider_caves::scatter(&mut world, &plan, &mut forest_rng);

    // Underground cabins, same reasoning and same relative position as gem/spider caves above:
    // sites into ground the passes above have already decorated, and needs `structures` (already
    // carrying every protected structure placed so far) to avoid overlapping one of them.
    let underground_cabins =
        underground_cabins::scatter(&mut world, &plan, &mut structures, &mut rand);

    // Underworld ruins, same "after the terrain that carries them already exists" reasoning —
    // `structures::underworld` (the ash/lava fill) already ran, above. Hellforges site directly
    // onto a ruin's own background wall, so it has to run after `scatter_ruins`, not just after
    // the terrain.
    let underworld_ruins = underworld_ruins::scatter_ruins(&mut world, &plan, &mut rand);
    let hellforges = underworld_ruins::scatter_hellforges(&mut world, &plan, &mut rand);

    // Spawn goes on the surface in the middle, in a pocket cleared for it.
    let spawn_y = heights[plan.spawn_x as usize];
    world.spawn_x = plan.spawn_x as i16;
    world.spawn_y = spawn_y as i16;
    terrain::clear_spawn(&mut world, plan.spawn_x, spawn_y);
    world.dungeon_y = Some(heights[plan.dungeon_x.clamp(0, width - 1) as usize]);

    let chests = chests.saturating_sub(drop_orphaned_chests(&mut world));

    // Smoothing runs last, after every fixture above is already down — see `smooth`'s own doc
    // comment for why that is a deliberate reversal of vanilla's own pass order (vanilla smooths
    // before it decorates) and how `protects_tile_below` covers the difference.
    let smoothed = smooth::smooth(&mut world, &plan, &mut rand).total();

    // A second Tier 3 wave, run after smoothing rather than before every other decorative pass:
    // vanilla's own pass order puts every one of these *after* `SmoothWorld` (generation pass 53 of
    // 105) — `Waterfalls` is 54, `FragileIceOverIceBiomeWater` 55, `CaveWallVariety` 56,
    // `MossAndMossCaves` 65, `CaveWallsInEnclosedSpaces` 67, `LongMoss` 94 — unlike
    // `DirtWallCleanup`/`GravitatingSandCleanup` (39/36), which run *before* it and are wired in
    // above, near `terrain`/`structures::caves`. Since this project's own `smooth()` is already a
    // deliberate reversal of vanilla's order (see its own doc comment just above), the honest
    // choice is to keep it the true last *decorative-adjacent* step and run everything vanilla
    // itself places after `SmoothWorld` in this same trailing wave, rather than interleaving these
    // with the Tier 1/2 passes above and re-litigating where `smooth()` belongs.
    let waterfalls = waterfalls::scatter(&mut world, &plan, &mut rand);
    let fragile_ice = thin_ice::crust(&mut world, &plan);
    // `cave_wall_variety`/`cave_walls_enclosed` (`CaveWallVariety` 56 / `CaveWallsInEnclosedSpaces`
    // 67) are kept adjacent here rather than literally interleaved with `MossAndMossCaves` (65)
    // between them — the two are `plan.md`'s own single bundled "Wall variety" item, landed and
    // measured together as one unit; splitting their calls apart to match vanilla's exact pass
    // numbers would only reorder which random draws each independent pass consumes (no correctness
    // difference either way — this project already discloses seeds not being map-identical to real
    // Terraria), at the cost of invalidating this row's own already-measured, already-published
    // counts for no behavioural gain.
    let cave_wall_variety = wall_variety::variety(&mut world, &plan, &mut rand);
    let cave_walls_enclosed = wall_variety::enclosed_spaces(&mut world, &plan, &mut rand);
    let moss_changed = moss::scatter(&mut world, &plan, &mut rand);
    let long_moss = moss::hang_long_moss(&mut world);
    // `QuickCleanup` (70), then `SurfaceOreAndStone` (74), then `SurfaceDirtWallsToGrassWalls`
    // (79) — all between `CaveWallsInEnclosedSpaces` (67) and the web/honey pass (85) in vanilla's
    // own order. Vanilla itself interleaves `SurfaceOreAndStone`/`SurfaceDirtWallsToGrassWalls`
    // with `Hellforges`(72)/`FallenLogsAndWaterFeatures`(75)/`Traps`(76)/`Piles`(77) — all four
    // already landed earlier in this project's own pipeline, before `smooth()` — so the exact
    // vanilla interleaving with those four can't be reproduced without re-litigating where
    // `smooth()` belongs (see this wave's own opening comment); their relative order against each
    // other and against the rest of this trailing wave is preserved instead.
    let quick_cleanup = tile_cleanup::quick_cleanup(&mut world, &plan);
    let surface_ore_and_stone = tile_cleanup::surface_ore_and_stone(&mut world, &plan, &mut rand);
    let surface_dirt_walls_to_grass_walls =
        tile_cleanup::surface_dirt_walls_to_grass_walls(&mut world, &plan, &mut rand);
    // `WebsInSpiderCavesAndHoneyPlusSpeleothemsInBeehives` (85), then the two exposed-gem passes
    // (92, 93), then `LongMoss` (94) — all between `CaveWallsInEnclosedSpaces` (67) and
    // `MicroBiomes` (97, already landed as Tier 2) in vanilla's own real order.
    let (honey_marked, hive_stalactites, cobwebs_placed) =
        speleothems::shared_web_and_honey(&mut world, &plan, &mut rand);
    let exposed_gems_ice = speleothems::exposed_gems_in_ice_biome(&mut world, &plan, &mut rand);
    let exposed_gems_underground =
        speleothems::exposed_gems_underground(&mut world, &plan, &mut rand);
    // `CactusPalmTreesAndCoral` (99), then `TileCleanup` (100), then
    // `LilypadsCattailsBambooAndSeaweed` (102) — all after `MicroBiomes` (97, Tier 2) and
    // `LongMoss` (94), all before `SpeleothemsAndGemTrees` (103).
    let (cacti_desert, beach_decorations) =
        water_plants::cacti_and_beach_decorations(&mut world, &plan, &mut rand);
    let tile_cleanup_changed = tile_cleanup::tile_cleanup(&mut world);
    let (lily_pads, cattails) = water_plants::lily_pads_and_cattails(&mut world, &plan, &mut rand);
    // `SpeleothemsAndGemTrees` (103), then `BrokenTrapCleanup` (104), then `FinalCleanup` (105) —
    // vanilla's own true final three passes, in that order, closing out this trailing wave and
    // `build()` itself.
    let speleothems = speleothems::scatter(&mut world, &plan, &mut rand);
    let broken_trap_cleanup = tile_cleanup::broken_trap_cleanup(&mut world);
    let final_cleanup = tile_cleanup::final_cleanup(&mut world, &plan);

    let built = Built {
        lakes,
        trees,
        vines,
        cacti,
        orbs,
        altars,
        life_crystals,
        chests,
        hive,
        pots,
        statues,
        piles,
        small_piles,
        fallen_logs: fallen_logs.placed,
        flowers,
        mushrooms,
        herbs,
        sunflowers,
        liquids_converged: liquids.converged,
        dart_traps: traps.dart_traps,
        mines: traps.mines,
        geysers: traps.geysers,
        boulder_traps: traps.boulder_traps,
        sand_traps: traps.sand_traps,
        smoothed,
        gem_caves,
        oases,
        spider_caves,
        jungle_shrines,
        pyramids,
        underground_cabins,
        underworld_ruins,
        hellforges,
        living_trees,
        floating_islands: floating_islands.islands,
        floating_island_houses: floating_islands.houses,
        cloud_lakes: floating_islands.lakes,
        thin_ice: micro_biomes.thin_ice,
        dune_fields,
        rigged_veins,
        sword_shrines,
        corruption_pits: micro_biomes.corruption_pits,
        spike_pits: micro_biomes.spike_pits,
        honey_patches: micro_biomes.honey_patches,
        campsites: micro_biomes.campsites,
        marble: micro_biomes.marble,
        granite: micro_biomes.granite,
        dirt_wall_cleared,
        waterfalls,
        fragile_ice,
        cave_wall_variety,
        cave_walls_enclosed,
        moss_changed,
        long_moss,
        speleothems,
        honey_marked,
        hive_stalactites,
        cobwebs_placed,
        exposed_gems_ice,
        exposed_gems_underground,
        cacti_desert,
        beach_decorations,
        lily_pads,
        cattails,
        gravitating_sand,
        quick_cleanup,
        surface_ore_and_stone,
        surface_dirt_walls_to_grass_walls,
        tile_cleanup: tile_cleanup_changed,
        broken_trap_cleanup,
        final_cleanup,
        secret_seeds: secret,
    };
    (world, built)
}

/// Drop chest records whose tiles are no longer there, and say how many went.
///
/// Chests are placed part-way through generation and several later passes — greenery, cobwebs and
/// the spawn pocket — write tiles over the top of them. A record left pointing at cleared ground
/// is not harmless: Terraria loads it, then deletes it and its contents on its own first save, so
/// the loot disappears some time after the world was handed over rather than here.
///
/// This is the same footprint check the game runs when it saves (`WorldFile.cs:1620`): all four
/// tiles in bounds, active, and a container.
fn drop_orphaned_chests(world: &mut World) -> usize {
    let mut dropped = 0;
    for slot in 0..world.chests.len() {
        let Some(chest) = world.chests[slot].as_ref() else {
            continue;
        };
        let (x, y) = (i32::from(chest.x), i32::from(chest.y));
        let whole = (0..2).all(|dx| {
            (0..2).all(|dy| {
                world.in_bounds(x + dx, y + dy) && {
                    let tile = world.tile(x + dx, y + dy);
                    tile.is_active() && tile.block == tiles::CHEST
                }
            })
        });
        if !whole {
            world.chests[slot] = None;
            dropped += 1;
        }
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrustia_proto::Tile;

    fn small() -> (World, Built) {
        build(2400, 900, "test", 1234)
    }

    /// `build`/`generate` never detect a secret seed — a plain numeric seed is always ordinary
    /// generation, matching real vanilla typing a number into the seed field.
    #[test]
    fn build_never_sets_a_secret_seed() {
        let (_world, built) = small();
        assert!(!built.secret_seeds.any());
    }

    /// `build_from_text` with an ordinary numeric string is identical to `build` with that same
    /// number — the text path is a superset of the numeric path, not a different one.
    #[test]
    fn a_numeric_text_seed_matches_the_equivalent_numeric_seed() {
        let (from_number, built_number) = build(1600, 700, "numeric", 1234);
        let (from_text, built_text) = build_from_text(1600, 700, "numeric", "1234");
        assert!(!built_number.secret_seeds.any());
        assert!(!built_text.secret_seeds.any());
        let differing = (0..from_number.width())
            .step_by(7)
            .flat_map(|x| (0..from_number.height()).step_by(7).map(move |y| (x, y)))
            .filter(|&(x, y)| from_number.tile(x, y) != from_text.tile(x, y))
            .count();
        assert_eq!(
            differing, 0,
            "the numeric seed 1234 and the text seed \"1234\" produced different worlds"
        );
    }

    /// `build_from_text` records the real typed text on `World::seed_text`, not the derived
    /// number — what a player who typed "getfixedboi" should see reflected back.
    #[test]
    fn build_from_text_keeps_the_real_typed_text() {
        let (world, built) = build_from_text(1600, 700, "text", "  getfixedboi  ");
        assert_eq!(world.seed_text, "getfixedboi");
        assert!(built.secret_seeds.no_traps);
        assert!(built.secret_seeds.get_good);
        assert!(built.secret_seeds.not_the_bees);
    }

    /// A world this generator made never claims a shape it did not make.
    ///
    /// `remixWorld` mirrors the whole world in vanilla and nothing here mirrors anything, but the
    /// flag was still detected, persisted, and sent to every client as `F::RemixWorld` - which a
    /// real client honours by drawing the underworld at the surface. An ordinary world announced as
    /// a Remix one is not a partial feature, it is a wrong one, and it went into the `.wld` too.
    ///
    /// The six flags that describe *behaviour* rather than shape are untouched, so "get fixed boi"
    /// still gets everything this server can actually do.
    #[test]
    fn a_generated_world_does_not_claim_a_shape_it_does_not_have() {
        for text in ["dontdigup", "getfixedboi"] {
            let (world, _) = build_from_text(1600, 700, "text", text);
            assert!(
                !world.secret_seeds.remix,
                "{text}: an ordinary world must not be announced as a Remix one"
            );
            assert!(
                !world.secret_seeds.everything,
                "{text}: and zenith is the combination, remix included"
            );
            let flags = world.world_data().flags;
            assert!(
                !flags.has_flag(terrustia_proto::packets::WorldFlag::RemixWorld),
                "{text}: and the wire must not carry the claim either"
            );
        }
        // What "get fixed boi" can still honestly turn on.
        let (world, _) = build_from_text(1600, 700, "text", "getfixedboi");
        assert!(world.secret_seeds.no_traps);
        assert!(world.secret_seeds.drunk);
        assert!(world.secret_seeds.dont_starve);
    }

    /// `World::secret_seeds` — not just `Built`'s own copy — carries the detected flags too, since
    /// that is what a save/load round trip and `world_data()`'s own `WorldFlag` bits both read from
    /// (see `wld.rs`/`wld_save.rs`/`server.rs`'s own tests for those).
    #[test]
    fn world_itself_carries_the_detected_secret_seeds_not_just_built() {
        let (world, built) = build_from_text(1600, 700, "text", "notthebees");
        assert_eq!(world.secret_seeds, built.secret_seeds);
        assert!(world.secret_seeds.not_the_bees);
    }

    /// The one secret seed this session actually wires to a behavioural difference: "No Traps
    /// World" places zero traps of any kind on a real generated world, where an ordinary seed at
    /// the same size reliably places some. End-to-end through `build_from_text` — `traps.rs`'s
    /// own unit tests already pin the mechanism in isolation; this is the same claim proven
    /// through the real pipeline `main.rs` actually calls.
    #[test]
    fn no_traps_world_generates_a_playable_world_with_zero_traps() {
        let (_ordinary, ordinary_built) = build(SMALL_WIDTH, SMALL_HEIGHT, "ordinary", 4242);
        assert!(
            ordinary_built.dart_traps
                + ordinary_built.mines
                + ordinary_built.geysers
                + ordinary_built.boulder_traps
                + ordinary_built.sand_traps
                > 0,
            "the control seed should place at least one trap of some kind"
        );

        let (world, built) = build_from_text(SMALL_WIDTH, SMALL_HEIGHT, "no traps", "notraps");
        assert!(built.secret_seeds.no_traps);
        assert_eq!(built.dart_traps, 0);
        assert_eq!(built.mines, 0);
        assert_eq!(built.geysers, 0);
        assert_eq!(built.boulder_traps, 0);
        assert_eq!(built.sand_traps, 0);
        // Still a real, playable world — the short-circuit should not have broken anything else.
        assert!(built.altars >= 6);
        assert!(built.chests >= 30);
        let has_hellstone = (0..world.width()).step_by(3).any(|x| {
            (0..world.height())
                .step_by(3)
                .any(|y| world.tile(x, y).block == tiles::HELLSTONE)
        });
        assert!(
            has_hellstone,
            "no hellstone: No Traps World broke the underworld too"
        );
    }

    /// "get fixed boi" cascades `no_traps` true too (one of its own seven real dependency flags),
    /// so it must place zero traps the same as typing "notraps" directly — the exact behavioural
    /// difference the old single-variant `SecretSeed` enum could not represent (see
    /// `secret_seed.rs`'s own module doc).
    #[test]
    fn get_fixed_boi_also_places_zero_traps() {
        let (_world, built) =
            build_from_text(SMALL_WIDTH, SMALL_HEIGHT, "get fixed boi", "getfixedboi");
        assert_eq!(
            built.dart_traps + built.mines + built.geysers + built.boulder_traps + built.sand_traps,
            0,
            "getfixedboi's own no_traps dependency should have cleared traps too"
        );
    }

    /// Every real secret seed still generates a real, playable world — proof that threading
    /// `SecretSeeds` through `build_with_secret_seed` doesn't panic or corrupt anything for the
    /// flags this session leaves as ordinary generation, even though nothing downstream branches
    /// on them yet. See `secret_seed.rs`'s own module doc for exactly which one (`no_traps`) is
    /// the exception.
    #[test]
    fn every_secret_seed_still_generates_a_playable_world() {
        for text in [
            "celebrationmk10",
            "5162020", // Drunk World's only real trigger
            "notthebees",
            "dontdigup", // Remix
            "notraps",
            "getfixedboi",
            "constant", // Don't Starve
            "fortheworthy",
            "skyblock",
        ] {
            let (world, built) = build_from_text(1600, 700, "secret", text);
            assert!(
                built.secret_seeds.any(),
                "{text:?} should have been detected as a secret seed"
            );
            assert!(built.chests > 0, "{text:?}: no chests");
            assert!(built.altars > 0, "{text:?}: no altars");
            assert_eq!(
                world.seed_text, text,
                "{text:?}: seed text was not preserved"
            );
        }
    }

    /// A seed string that matches none of the real magic strings/numbers is just an ordinary
    /// (hashed) text seed.
    #[test]
    fn an_unrecognised_text_seed_is_ordinary_generation() {
        let (_world, built) = build_from_text(1600, 700, "ordinary text", "my cool world");
        assert!(!built.secret_seeds.any());
    }

    /// Real Tier 3 counts on real generated worlds — not asserted, just printed, matching
    /// `pyramids.rs`'s own `measure_on_real_worlds` precedent. Run with
    /// `cargo test -p terrustia --lib worldgen::tests::measure_tier3_on_real_worlds -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn measure_tier3_on_real_worlds() {
        for seed in [999u64, 4242, 12345] {
            let start = std::time::Instant::now();
            let (_world, built) = build(SMALL_WIDTH, SMALL_HEIGHT, "measure-tier3", seed);
            eprintln!(
                "seed {seed}: dirt_wall_cleared={} waterfalls={} fragile_ice={} \
                 cave_wall_variety={} cave_walls_enclosed={} moss_changed={} long_moss={} \
                 speleothems={} honey_marked={} hive_stalactites={} cobwebs_placed={} \
                 exposed_gems_ice={} exposed_gems_underground={} cacti_desert={} \
                 beach_decorations={} lily_pads={} cattails={} gravitating_sand={} \
                 quick_cleanup={} surface_ore_and_stone={} surface_dirt_walls_to_grass_walls={} \
                 tile_cleanup={} broken_trap_cleanup={} final_cleanup={} ({:?})",
                built.dirt_wall_cleared,
                built.waterfalls,
                built.fragile_ice,
                built.cave_wall_variety,
                built.cave_walls_enclosed,
                built.moss_changed,
                built.long_moss,
                built.speleothems,
                built.honey_marked,
                built.hive_stalactites,
                built.cobwebs_placed,
                built.exposed_gems_ice,
                built.exposed_gems_underground,
                built.cacti_desert,
                built.beach_decorations,
                built.lily_pads,
                built.cattails,
                built.gravitating_sand,
                built.quick_cleanup,
                built.surface_ore_and_stone,
                built.surface_dirt_walls_to_grass_walls,
                built.tile_cleanup,
                built.broken_trap_cleanup,
                built.final_cleanup,
                start.elapsed()
            );
        }
    }

    /// A world has to hold every structure a playthrough needs, or the game cannot be finished
    /// in it. This is the test that makes "playable" mean something.
    #[test]
    fn a_generated_world_can_be_played_through() {
        let (world, built) = small();

        assert!(
            built.orbs >= 3,
            "only {} orbs: three must be smashable to wake the evil's boss",
            built.orbs
        );
        assert!(
            built.altars >= 6,
            "only {} altars: hardmode ore comes from nowhere else",
            built.altars
        );
        assert!(
            built.life_crystals >= 10,
            "only {} life crystals: a hundred hit points is the whole game",
            built.life_crystals
        );
        assert!(
            built.chests >= 30,
            "only {} chests: no starter gear",
            built.chests
        );

        let has = |block: u16| {
            (0..world.width()).step_by(3).any(|x| {
                (0..world.height())
                    .step_by(3)
                    .any(|y| world.tile(x, y).block == block)
            })
        };
        assert!(has(tiles::HELLSTONE), "no hellstone: no Wall of Flesh");
        assert!(has(tiles::LIHZAHRD_BRICK), "no temple: no Golem");
        assert!(has(tiles::JUNGLE_GRASS), "no jungle");
        assert!(has(tiles::SNOW), "no snow biome");
        assert!(has(tiles::SAND), "no desert or ocean");
        assert!(
            has(tiles::BLUE_DUNGEON_BRICK)
                || has(tiles::GREEN_DUNGEON_BRICK)
                || has(tiles::PINK_DUNGEON_BRICK),
            "no dungeon: no Skeletron"
        );
        assert!(
            has(tiles::EBONSTONE) || has(tiles::CRIMSTONE),
            "no evil biome"
        );
        assert!(has(tiles::COPPER) && has(tiles::IRON), "no early ore");
        assert!(has(tiles::GOLD) && has(tiles::SILVER), "no deep ore");
    }

    /// The same seed makes the same world, twice running.
    #[test]
    fn a_seed_makes_the_same_world() {
        let (a, built_a) = build(1200, 600, "seeded", 77);
        let (b, built_b) = build(1200, 600, "seeded", 77);
        assert_eq!(built_a, built_b);
        assert_eq!(a.crimson, b.crimson);
        assert_eq!((a.spawn_x, a.spawn_y), (b.spawn_x, b.spawn_y));
        let differing = (0..a.width())
            .step_by(7)
            .flat_map(|x| (0..a.height()).step_by(7).map(move |y| (x, y)))
            .filter(|&(x, y)| a.tile(x, y) != b.tile(x, y))
            .count();
        assert_eq!(differing, 0, "the same seed produced two different worlds");
    }

    /// ...and different seeds make different worlds.
    #[test]
    fn different_seeds_make_different_worlds() {
        let (a, _) = build(1200, 600, "a", 1);
        let (b, _) = build(1200, 600, "b", 2);
        let differing = (0..a.width())
            .step_by(5)
            .flat_map(|x| (0..a.height()).step_by(5).map(move |y| (x, y)))
            .filter(|&(x, y)| a.tile(x, y).block != b.tile(x, y).block)
            .count();
        assert!(
            differing > 500,
            "two seeds differ in only {differing} sampled tiles"
        );
    }

    /// A full-size world gets Enchanted Sword shrines, and the sword is really in them.
    ///
    /// End-to-end rather than only in `enchanted_sword`'s own unit tests, because the siting
    /// checks are strict enough (1250 solid tiles in a 50x50 box, a clear shaft, no sand) that a
    /// biome can pass in a hand-built stone slab and never once place in a real world. That is the
    /// failure this asserts against, and it is the same shape as the unreachable-NPC problem
    /// `check-spawn-reach` exists for: nothing errors, no test fails, the feature is just absent.
    #[test]
    fn a_real_world_gets_sword_shrines_with_swords_in_them() {
        let mut total = 0;
        let mut swords = 0;
        for seed in 1..5u64 {
            let (world, built) = build(SMALL_WIDTH, SMALL_HEIGHT, "sword shrines", seed);
            total += built.sword_shrines;
            for x in (0..world.width()).step_by(2) {
                for y in (0..world.height()).step_by(2) {
                    if world.tile(x, y).block == 187 {
                        swords += 1;
                    }
                }
            }
        }
        assert!(total > 0, "no sword shrine placed across four full worlds");
        // Same claim for the rigged veins, which share the pipeline and the same silent-absence
        // failure mode.
        let (veins_world, veins_built) = build(SMALL_WIDTH, SMALL_HEIGHT, "rigged veins", 11);
        assert!(veins_built.rigged_veins > 0, "no rigged ore vein placed");
        let detonators = (0..veins_world.width()).step_by(2).any(|x| {
            (0..veins_world.height())
                .step_by(2)
                .any(|y| veins_world.tile(x, y).block == 411)
        });
        assert!(detonators, "veins placed but no detonator wired to any");
        assert!(
            swords > 0,
            "shrines placed but no Enchanted Sword tile in any"
        );
    }

    /// Spawn is somewhere a player can stand: air above, ground below, no water.
    #[test]
    fn spawn_is_somewhere_survivable() {
        for seed in [1u64, 2, 3, 99, 12345] {
            let (world, _) = build(1600, 700, "spawn", seed);
            let (x, y) = (i32::from(world.spawn_x), i32::from(world.spawn_y));
            for above in 1..8 {
                assert!(
                    !world.tile(x, y - above).is_active(),
                    "seed {seed}: spawn is buried at {above} above"
                );
                assert_eq!(
                    world.tile(x, y - above).liquid,
                    0,
                    "seed {seed}: spawn is underwater"
                );
            }
        }
    }

    /// A generated world saves and loads back unchanged, which is what makes it worth generating.
    #[test]
    fn a_generated_world_survives_a_save() {
        use crate::world::{wld, wld_save};
        let (world, built) = build(1200, 600, "roundtrip", 5);
        let bytes = wld_save::serialize(&world).expect("it should save");
        let back = wld::parse(&bytes).expect("it should load");

        assert_eq!(back.width(), world.width());
        assert_eq!(back.height(), world.height());
        assert_eq!(
            back.chests.iter().flatten().count(),
            world.chests.iter().flatten().count(),
            "chests were lost across a save"
        );
        assert!(
            world.chests.iter().flatten().count() >= built.chests,
            "the world should hold at least the cavern chests, plus the dungeon's"
        );
        let differing = (0..world.width())
            .flat_map(|x| (0..world.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| !tiles_equal_for_save(world.tile(x, y), back.tile(x, y)))
            .count();
        assert_eq!(differing, 0, "{differing} tiles changed across a save");
    }

    /// Whether two tiles are the same for the purpose of "did the save lose anything real".
    ///
    /// `liquid_kind` on a dry tile (`liquid == 0`) does not survive a save: the wire and file
    /// format both, deliberately, only encode `liquid_kind` when `liquid != 0`
    /// (`terrustia-proto/src/section.rs`'s `write_tile_with`, guarded by `if tile.liquid != 0`,
    /// and its matching reader), the same as real vanilla's own tile section, which never
    /// transmits a liquid type for a tile that currently holds none. A generated world can still
    /// carry a stale, pre-drain `liquid_kind` on a tile that no longer has any liquid (several
    /// worldgen passes drain liquid without resetting the type byte, matching vanilla's own
    /// `Tile.liquidType`, which is not reset on drain either); that byte alone differing, with
    /// both sides genuinely dry, is not a real difference a save lost.
    fn tiles_equal_for_save(before: Tile, after: Tile) -> bool {
        before == after
            || (before.liquid == 0
                && after.liquid == 0
                && Tile {
                    liquid_kind: after.liquid_kind,
                    ..before
                } == after)
    }

    /// Every header field survives a save.
    ///
    /// The tile comparison above cannot see a header field the writer drops, and several were
    /// being dropped in silence — the dungeon's y, the seed text, the hardmode ore tiers — because
    /// nothing ever compared them. Each piece was already in place: the generator recorded them,
    /// the parser read them back, and only the writer threw them away.
    ///
    /// This compares the whole header at once and reports every difference in one run, rather than
    /// stopping at the first, so the next field to go missing fails a test instead of a
    /// playthrough.
    #[test]
    fn every_header_field_survives_a_save() {
        for &(width, height) in &[(1200i32, 600i32), (4200, 1200)] {
            check_header_round_trip(width, height);
        }
    }

    /// One world's worth of the check above.
    ///
    /// Split out and run at more than one size deliberately: the header holds variable-length runs
    /// whose lengths depend on the world, so a writer and reader that disagree about one of them
    /// can still agree at one size and not another.
    fn check_header_round_trip(width: i32, height: i32) {
        use crate::world::{wld, wld_save};
        let (world, _) = build(width, height, "header roundtrip", 9);
        let bytes = wld_save::serialize(&world).expect("it should save");
        let back = wld::parse(&bytes).expect("it should load");

        let mut wrong: Vec<String> = Vec::new();
        macro_rules! same {
            ($($field:ident),+ $(,)?) => {$(
                if world.$field != back.$field {
                    wrong.push(format!(
                        "{}: wrote {:?}, read back {:?}",
                        stringify!($field),
                        world.$field,
                        back.$field,
                    ));
                }
            )+};
        }

        same!(
            spawn_x,
            spawn_y,
            surface,
            rock_layer,
            name,
            id,
            unique_id,
            time,
            day_time,
            blood_moon,
            eclipse,
            moon_phase,
            raining,
            rain_time,
            max_rain,
            sandstorm,
            sandstorm_time,
            sandstorm_severity,
            sandstorm_intended_severity,
            dungeon_x,
            dungeon_y,
            pumpkin_moon,
            snow_moon,
            wind,
            crimson,
            ore_tiers,
            progress,
            game_mode,
            world_gen_version,
            seed_text,
            secret_seeds,
            moon_type,
            tree_x,
            tree_style,
            cave_back_x,
            cave_back_style,
            ice_back_style,
            jungle_back_style,
            hell_back_style,
            backgrounds,
            tree_tops,
            num_clouds,
        );

        assert!(
            wrong.is_empty(),
            "{width}x{height}: {} header field(s) changed across a save:\n  {}",
            wrong.len(),
            wrong.join("\n  "),
        );
    }

    /// A generated secret-seed world's own active flags survive a real save and reload — the gap
    /// this project's own generated-world header writer used to have on purpose (`wld_save.rs`'s
    /// old comment: "the nine special world seeds, none of which a generated world has"), now that
    /// a generated world genuinely can have some. Uses "getfixedboi" specifically because it
    /// exercises the most flags at once (seven of nine), the case most likely to catch a
    /// transposed bit or a dropped flag in either the write or the read side.
    #[test]
    fn a_generated_secret_seed_worlds_flags_survive_a_save() {
        use crate::world::{wld, wld_save};
        let (world, built) = build_from_text(1600, 700, "secret roundtrip", "getfixedboi");
        assert!(built.secret_seeds.any(), "getfixedboi should have matched");

        let bytes = wld_save::serialize(&world).expect("it should save");
        let back = wld::parse(&bytes).expect("it should load");

        assert_eq!(
            back.secret_seeds, world.secret_seeds,
            "the secret-seed flags did not survive a save and reload"
        );

        // A second round through the *preserved*-header path (`back.preserved` is now `Some`,
        // since it was just loaded from a file) — the mutable-fields-patch path a real server
        // actually uses on every later autosave, not the fresh-header path this test's first
        // round exercised. The flag bytes are not among the offsets that path patches, so this
        // should carry through untouched, but "should" is exactly what a save/load round trip
        // is for checking.
        let bytes_again = wld_save::serialize(&back).expect("the preserved path should save");
        let back_again = wld::parse(&bytes_again).expect("it should load a second time");
        assert_eq!(
            back_again.secret_seeds, world.secret_seeds,
            "the secret-seed flags did not survive a second, preserved-header save"
        );
    }

    /// Every chest record has a chest under it.
    ///
    /// A record whose tiles were carved away by a later pass survives generation, a save and a
    /// load, and is only deleted when Terraria next saves the world — taking the loot with it,
    /// long after the world looked fine.
    ///
    /// This sweeps several sizes and seeds deliberately. A single world is not enough: of the
    /// twenty-one checked when this was written, seventeen had between one and four orphans and
    /// four had none, so one unlucky choice of seed makes the check pass while the bug is
    /// untouched.
    #[test]
    fn every_chest_record_has_tiles_under_it() {
        for &(width, height) in &[(1200i32, 600i32), (4200, 1200)] {
            for seed in 1..6u64 {
                let (world, _) = build(width, height, "chest footprints", seed);
                let orphans: Vec<_> = world
                    .chests
                    .iter()
                    .flatten()
                    .filter(|chest| {
                        let (x, y) = (i32::from(chest.x), i32::from(chest.y));
                        !(0..2).all(|dx| {
                            (0..2).all(|dy| {
                                world.in_bounds(x + dx, y + dy) && {
                                    let tile = world.tile(x + dx, y + dy);
                                    tile.is_active() && tile.block == tiles::CHEST
                                }
                            })
                        })
                    })
                    .map(|chest| (chest.x, chest.y))
                    .collect();
                assert!(
                    orphans.is_empty(),
                    "{width}x{height} seed {seed}: {} chest record(s) point at ground with no \
                     chest on it: {orphans:?}",
                    orphans.len(),
                );
                assert!(
                    world.chests.iter().flatten().count() > 0,
                    "{width}x{height} seed {seed}: the sweep took every chest",
                );
            }
        }
    }

    /// The townsfolk survive a save, with their names and their houses.
    ///
    /// The world file's NPC section was carried through as an opaque blob in both directions, so a
    /// resident was a session-long guest: nobody who moved in was ever written down, and a real
    /// Terraria world's existing residents were invisible to the server entirely.
    #[test]
    fn the_townsfolk_survive_a_save() {
        use crate::world::{objects::TownNpc, wld, wld_save};

        let (mut world, _) = build(1200, 600, "townsfolk", 3);
        world.town_npcs = vec![
            TownNpc {
                net_id: 22,
                name: "Andrew".into(),
                position: (1234.0, 567.0),
                homeless: false,
                home: (77, 88),
                variation: 1,
                homeless_despawn: false,
            },
            TownNpc {
                net_id: 17,
                name: "Wilhelmina".into(),
                position: (99.5, 12.25),
                homeless: true,
                home: (0, 0),
                variation: 0,
                homeless_despawn: true,
            },
        ];
        world.shimmered_town_npcs = vec![22];

        let bytes = wld_save::serialize(&world).expect("it should save");
        let back = wld::parse(&bytes).expect("it should load");

        assert_eq!(back.town_npcs, world.town_npcs, "the residents changed");
        assert_eq!(back.shimmered_town_npcs, world.shimmered_town_npcs);
    }

    /// Banner kill counts survive a save.
    ///
    /// Nothing counted kills at all before this, and the save wrote two zeroes with a comment
    /// saying so — meaning a hundred zombies killed before a restart counted for nothing after it.
    #[test]
    fn banner_kills_survive_a_save() {
        use crate::world::{wld, wld_save};

        let (mut world, _) = build(1200, 600, "banners", 4);
        world.banner_kills.insert(7, 49);
        world.banner_kills.insert(120, 1);
        world.banner_kills.insert(292, 1234);

        let bytes = wld_save::serialize(&world).expect("it should save");
        let back = wld::parse(&bytes).expect("it should load");

        assert_eq!(back.banner_kills.get(&7), Some(&49));
        assert_eq!(back.banner_kills.get(&120), Some(&1));
        assert_eq!(back.banner_kills.get(&292), Some(&1234));
        assert_eq!(
            back.banner_kills.get(&8),
            None,
            "untouched banners stay absent"
        );
    }

    /// Freeing a bound town slime survives a save, through both writer paths.
    ///
    /// These three are the only progress flags in the file that read backwards: a `true` is what
    /// *stops* their bound form spawning (`NPC.cs:2095`, `:1417`, `:5623` all test the negation),
    /// so a world that loses one does not merely forget a rescue, it puts a second bound slime back
    /// into the caverns beside the resident already living in a house.
    ///
    /// Fails before the fix in both halves: the fresh-header writer wrote seven hard-coded `false`
    /// bytes for the whole slime run, and the reader stopped at the second combat book and never
    /// looked at them at all.
    #[test]
    fn freed_town_slimes_survive_a_save() {
        use crate::world::{wld, wld_save};

        let (mut world, _) = build(1200, 600, "town slimes", 6);
        world.progress.unlocked_slime_old = true;
        world.progress.unlocked_slime_purple = true;
        world.progress.unlocked_slime_yellow = true;

        let bytes = wld_save::serialize(&world).expect("it should save");
        let back = wld::parse(&bytes).expect("it should load");
        assert!(back.progress.unlocked_slime_old, "the old slime");
        assert!(back.progress.unlocked_slime_purple, "the purple slime");
        assert!(back.progress.unlocked_slime_yellow, "the yellow slime");

        // ...and again through the preserved-header patch path, which is what every autosave after
        // the first actually takes. Flip one back off to prove the patch writes false as well as
        // true: a patch that only ever ORs bits in would pass the round trip above and still be
        // unable to represent a world that has not freed the slime yet.
        let mut second = back;
        second.progress.unlocked_slime_purple = false;
        let bytes = wld_save::serialize(&second).expect("the preserved path should save");
        let again = wld::parse(&bytes).expect("it should load a second time");
        assert!(again.progress.unlocked_slime_old);
        assert!(!again.progress.unlocked_slime_purple);
        assert!(again.progress.unlocked_slime_yellow);
    }

    /// Chests hold something. An empty chest is worse than no chest.
    #[test]
    fn chests_are_not_empty() {
        let (world, _) = small();
        let filled = world
            .chests
            .iter()
            .flatten()
            .filter(|c| c.items.iter().any(|i| !i.is_empty()))
            .count();
        let total = world.chests.iter().flatten().count();
        assert!(total > 0);
        assert_eq!(
            filled,
            total,
            "{} of {total} chests are empty",
            total - filled
        );
    }

    /// The world's own flags agree with what was built, since clients and saves read those
    /// rather than the tiles.
    #[test]
    fn the_world_flags_match_what_was_built() {
        let (world, _) = small();
        assert!(world.surface > 0 && world.surface < world.height() as i16);
        assert!(world.rock_layer > world.surface);
        assert!(world.dungeon_x.is_some(), "the dungeon has no recorded x");
        assert!(world.dungeon_y.is_some(), "the dungeon has no recorded y");
        assert!(!world.seed_text.is_empty(), "the seed is not recorded");
    }

    /// Nothing is written outside the world, and the top rows stay sky.
    #[test]
    fn the_sky_is_left_alone() {
        let (world, _) = small();
        for x in (0..world.width()).step_by(11) {
            assert!(
                !world.tile(x, 2).is_active(),
                "column {x} has ground at the very top of the world"
            );
        }
        assert_eq!(world.tile(-1, -1), Tile::AIR, "out of bounds reads as air");
    }
}
