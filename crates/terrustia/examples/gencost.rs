//! Per-pass timing for world generation.
//!
//! `worldgen::build()` (`world/worldgen/mod.rs`) is one long sequential pipeline of 30+ passes.
//! This times each one individually, because "generation took 28 seconds" says nothing about
//! whether that is one slow pass or thirty ordinary ones — and that distinction is the difference
//! between "parallelize the one slow pass's own inner loop" and "there is nothing worth
//! parallelizing here."
//!
//! This is a diagnostic tool, not part of the generator itself: it calls the same `pub` pass
//! functions `build()` calls, **in the same order**, and has to be kept in sync with it by hand.
//! If `build()`'s own pass order or argument list changes, this drifts out of sync silently rather
//! than failing to compile (every pass function's signature is still valid to call, just possibly
//! in a now-stale order) — cross-check against `world/worldgen/mod.rs`'s own `build()` before
//! trusting a number here after that file changes. One deliberate omission: `build()`'s own
//! private `drop_orphaned_chests` helper isn't reachable from an example (it's not `pub`) and is a
//! cheap bookkeeping pass over a few hundred chest slots, not a generation pass, so it is left out
//! of both the call sequence and the timing table.
//!
//! Every pass is timed on **both** clocks, `game::clock::Cpu` and the wall clock, and the table
//! is ranked by wall time. They read the same for an ordinary single-threaded pass. This
//! session's own machine is shared and its load average routinely sits at 2x its core count,
//! which made an early wall-clock-only pass at this same measurement swing 8.4s to 41s for the
//! bit-identical work of the same seed and size run twice in a row — CPU time (counting only
//! time this thread actually spent on a core, ignoring time spent preempted) filters most of
//! that out, and is the more trustworthy number for comparing two runs on this machine. The two
//! clocks deliberately diverge for `tile_cleanup::gravitating_sand_cleanup`, the one pass this
//! generator parallelizes across worker threads (see that function's own doc comment): the
//! calling thread mostly blocks waiting on `.join()` rather than computing, so its own CPU time
//! undercounts the pass's real cost, while wall time still answers the question that actually
//! matters for a tick budget — how long was the single-writer thread unavailable.
//!
//! ```sh
//! cargo run --release -p terrustia --example gencost -- [--large] [--seed N]
//! ```

use terrustia::game::clock::Cpu;
use terrustia::world::{
    World, trees,
    worldgen::{
        dirt_wall_cleanup, fallen_logs, floating_islands, gem_caves, jungle_shrines, lakes,
        layout::{Evil, Layout},
        liquid_settle, living_trees, micro_biomes, moss, oasis, piles, pots, pyramids,
        rand::UnifiedRandom,
        scenery, smooth, speleothems, spider_caves, statue_gen, structure_map, structures,
        surface_plants, terrain, thin_ice, tile_cleanup,
        tiles::{COPPER, GOLD, IRON, SILVER},
        traps, underground_cabins, underworld_ruins, wall_variety, water_plants, waterfalls,
    },
};

fn main() {
    let mut large = false;
    let mut seed = 999u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--large" => large = true,
            "--seed" => seed = args.next().and_then(|s| s.parse().ok()).unwrap_or(999),
            _ => {}
        }
    }
    let (width, height) = if large { (8400, 2400) } else { (4200, 1200) };
    println!("generating {width}x{height}, seed {seed}");

    // Both clocks, not just one — `game/clock.rs`'s own reasoning applies here too: CPU time
    // says what this (calling) thread actually computed, wall time says how long it was
    // unavailable to do anything else. They read the same for every ordinary single-threaded
    // pass here. They diverge on purpose for `gravitating_sand_cleanup`, the one pass this file
    // parallelizes across worker threads: the calling thread mostly *blocks* waiting for workers
    // rather than computing, so its own CPU time undercounts the real cost — wall time is the
    // number that answers "how long would this stall the single-writer tick."
    let mut timings: Vec<(&'static str, std::time::Duration, std::time::Duration)> = Vec::new();
    macro_rules! timed {
        ($name:expr, $body:expr) => {{
            let cpu_began = Cpu::now();
            let wall_began = std::time::Instant::now();
            let result = $body;
            timings.push(($name, Cpu::now().since(cpu_began), wall_began.elapsed()));
            result
        }};
    }

    let overall_cpu = Cpu::now();
    let overall_wall = std::time::Instant::now();

    let mut world = World::empty(width, height, "gencost");
    let mut rand = UnifiedRandom::new(seed as i32);

    let plan = timed!("Layout::plan", Layout::plan(width, height, &mut rand));
    let mut structures = structure_map::StructureMap::new();
    world.id = rand.next();
    for byte in &mut world.unique_id {
        *byte = rand.next_max(256) as u8;
    }
    world.crimson = plan.evil == Evil::Crimson;
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
    timed!("scenery::choose", scenery::choose(&mut world, &mut rand));
    world.surface = plan.surface as i16;
    world.rock_layer = plan.rock as i16;
    world.dungeon_x = Some(plan.dungeon_x);

    let heights = timed!("terrain::heightmap", terrain::heightmap(&plan, &mut rand));
    timed!(
        "terrain::fill",
        terrain::fill(&mut world, &plan, &heights, &mut rand)
    );

    timed!(
        "structures::caves",
        structures::caves(&mut world, &plan, &mut rand)
    );
    timed!(
        "structures::ores",
        structures::ores(&mut world, &plan, &mut rand)
    );

    timed!(
        "tile_cleanup::gravitating_sand_cleanup",
        tile_cleanup::gravitating_sand_cleanup(&mut world, &plan)
    );
    timed!(
        "dirt_wall_cleanup::scrub",
        dirt_wall_cleanup::scrub(&mut world, &plan, &mut rand)
    );

    timed!(
        "structures::evil_chasms",
        structures::evil_chasms(&mut world, &plan, &heights, &mut rand, None)
    );
    timed!(
        "structures::dungeon",
        structures::dungeon(&mut world, &plan, &heights, &mut rand)
    );
    timed!(
        "structures::temple",
        structures::temple(&mut world, &plan, &mut rand)
    );
    timed!(
        "structures::hive",
        structures::hive(&mut world, &plan, &mut rand)
    );
    timed!(
        "structures::underworld",
        structures::underworld(&mut world, &plan, &mut rand)
    );

    timed!(
        "structures::altars",
        structures::altars(&mut world, &plan, &mut rand)
    );
    timed!(
        "structures::life_crystals",
        structures::life_crystals(&mut world, &plan, &mut rand)
    );
    timed!(
        "structures::chests",
        structures::chests(&mut world, &plan, &mut rand)
    );

    timed!(
        "structures::greenery",
        structures::greenery(&mut world, &plan, &heights, &mut rand)
    );
    timed!(
        "structures::cobwebs",
        structures::cobwebs(&mut world, &plan, &mut rand)
    );

    let mut forest_rng = {
        use rand::SeedableRng;
        rand::rngs::SmallRng::seed_from_u64(seed ^ 0x7265_6573)
    };
    timed!(
        "lakes::carve",
        lakes::carve(&mut world, &plan, &heights, &mut rand)
    );

    let liquids = timed!("liquid_settle::settle", liquid_settle::settle(&mut world));
    assert!(liquids.converged, "liquid settling did not converge");

    timed!(
        "oasis::scatter",
        oasis::scatter(&mut world, &plan, &mut rand)
    );
    timed!(
        "pyramids::scatter",
        pyramids::scatter(&mut world, &plan, &mut rand, &mut forest_rng)
    );
    timed!(
        "living_trees::scatter",
        living_trees::scatter(&mut world, &plan, &mut rand)
    );
    timed!(
        "living_trees::scatter_walls",
        living_trees::scatter_walls(&mut world, &plan)
    );
    timed!(
        "floating_islands::scatter",
        floating_islands::scatter(&mut world, &plan, &mut structures, &mut rand)
    );

    timed!(
        "trees::grass_the_jungle",
        trees::grass_the_jungle(&mut world)
    );
    timed!(
        "trees::plant_forest",
        trees::plant_forest(&mut world, &mut forest_rng)
    );
    timed!(
        "trees::plant_undergrowth",
        trees::plant_undergrowth(&mut world, &mut forest_rng)
    );

    timed!(
        "jungle_shrines::scatter",
        jungle_shrines::scatter(&mut world, &plan, &mut structures, &mut rand)
    );

    timed!(
        "micro_biomes::scatter",
        micro_biomes::scatter(&mut world, &plan, &mut structures, &mut rand)
    );

    timed!(
        "pots::scatter",
        pots::scatter(&mut world, &plan, &mut forest_rng)
    );
    timed!(
        "statue_gen::scatter",
        statue_gen::scatter(&mut world, &plan, &mut forest_rng)
    );
    timed!(
        "piles::scatter",
        piles::scatter(&mut world, &plan, &mut forest_rng)
    );
    timed!(
        "fallen_logs::scatter",
        fallen_logs::scatter(&mut world, &plan, &mut forest_rng)
    );

    timed!(
        "surface_plants::flowers",
        surface_plants::flowers(&mut world, &mut rand)
    );
    timed!(
        "surface_plants::mushrooms",
        surface_plants::mushrooms(&mut world, &mut rand)
    );
    timed!(
        "surface_plants::herbs",
        surface_plants::herbs(&mut world, &mut rand)
    );
    timed!(
        "surface_plants::sunflowers",
        surface_plants::sunflowers(&mut world, &mut rand)
    );

    timed!(
        "traps::scatter",
        traps::scatter(&mut world, &plan, &mut forest_rng, Default::default())
    );

    timed!(
        "gem_caves::scatter",
        gem_caves::scatter(&mut world, &plan, &mut rand)
    );
    timed!(
        "spider_caves::scatter",
        spider_caves::scatter(&mut world, &plan, &mut forest_rng)
    );

    timed!(
        "underground_cabins::scatter",
        underground_cabins::scatter(&mut world, &plan, &mut structures, &mut rand)
    );

    timed!(
        "underworld_ruins::scatter_ruins",
        underworld_ruins::scatter_ruins(&mut world, &plan, &mut rand)
    );
    timed!(
        "underworld_ruins::scatter_hellforges",
        underworld_ruins::scatter_hellforges(&mut world, &plan, &mut rand)
    );

    let spawn_y = heights[plan.spawn_x as usize];
    world.spawn_x = plan.spawn_x as i16;
    world.spawn_y = spawn_y as i16;
    timed!(
        "terrain::clear_spawn",
        terrain::clear_spawn(&mut world, plan.spawn_x, spawn_y)
    );
    world.dungeon_y = Some(heights[plan.dungeon_x.clamp(0, width - 1) as usize]);

    timed!(
        "smooth::smooth",
        smooth::smooth(&mut world, &plan, &mut rand)
    );

    timed!(
        "waterfalls::scatter",
        waterfalls::scatter(&mut world, &plan, &mut rand)
    );
    timed!("thin_ice::crust", thin_ice::crust(&mut world, &plan));
    timed!(
        "wall_variety::variety",
        wall_variety::variety(&mut world, &plan, &mut rand)
    );
    timed!(
        "wall_variety::enclosed_spaces",
        wall_variety::enclosed_spaces(&mut world, &plan, &mut rand)
    );
    timed!("moss::scatter", moss::scatter(&mut world, &plan, &mut rand));
    timed!("moss::hang_long_moss", moss::hang_long_moss(&mut world));
    timed!(
        "tile_cleanup::quick_cleanup",
        tile_cleanup::quick_cleanup(&mut world, &plan)
    );
    timed!(
        "tile_cleanup::surface_ore_and_stone",
        tile_cleanup::surface_ore_and_stone(&mut world, &plan, &mut rand)
    );
    timed!(
        "tile_cleanup::surface_dirt_walls_to_grass_walls",
        tile_cleanup::surface_dirt_walls_to_grass_walls(&mut world, &plan, &mut rand)
    );
    timed!(
        "speleothems::shared_web_and_honey",
        speleothems::shared_web_and_honey(&mut world, &plan, &mut rand)
    );
    timed!(
        "speleothems::exposed_gems_in_ice_biome",
        speleothems::exposed_gems_in_ice_biome(&mut world, &plan, &mut rand)
    );
    timed!(
        "speleothems::exposed_gems_underground",
        speleothems::exposed_gems_underground(&mut world, &plan, &mut rand)
    );
    timed!(
        "water_plants::cacti_and_beach_decorations",
        water_plants::cacti_and_beach_decorations(&mut world, &plan, &mut rand)
    );
    timed!(
        "tile_cleanup::tile_cleanup",
        tile_cleanup::tile_cleanup(&mut world)
    );
    timed!(
        "water_plants::lily_pads_and_cattails",
        water_plants::lily_pads_and_cattails(&mut world, &plan, &mut rand)
    );
    timed!(
        "speleothems::scatter",
        speleothems::scatter(&mut world, &plan, &mut rand)
    );
    timed!(
        "tile_cleanup::broken_trap_cleanup",
        tile_cleanup::broken_trap_cleanup(&mut world)
    );
    timed!(
        "tile_cleanup::final_cleanup",
        tile_cleanup::final_cleanup(&mut world, &plan)
    );

    let total_cpu = Cpu::now().since(overall_cpu);
    let total_wall = overall_wall.elapsed();

    // Ranked by wall time — the number that answers "how long was the single-writer thread
    // unavailable," which is what a tick budget actually cares about.
    timings.sort_by_key(|t| std::cmp::Reverse(t.2));
    println!();
    println!("{:<48} {:>10} {:>10}", "pass", "cpu ms", "wall ms");
    println!("{:-<70}", "");
    let mut accounted_cpu = std::time::Duration::ZERO;
    let mut accounted_wall = std::time::Duration::ZERO;
    for (name, cpu, wall) in &timings {
        accounted_cpu += *cpu;
        accounted_wall += *wall;
        println!(
            "{name:<48} {:>10.2} {:>10.2}",
            cpu.as_secs_f64() * 1e3,
            wall.as_secs_f64() * 1e3
        );
    }
    println!("{:-<70}", "");
    println!(
        "{:<48} {:>10.2} {:>10.2}",
        "sum of timed passes",
        accounted_cpu.as_secs_f64() * 1e3,
        accounted_wall.as_secs_f64() * 1e3
    );
    println!(
        "{:<48} {:>10.2} {:>10.2}",
        "total (incl. untimed glue)",
        total_cpu.as_secs_f64() * 1e3,
        total_wall.as_secs_f64() * 1e3
    );
    println!();
    println!(
        "top pass (by wall time) is {:.1}% of total wall time",
        timings[0].2.as_secs_f64() / total_wall.as_secs_f64() * 100.0
    );
}
