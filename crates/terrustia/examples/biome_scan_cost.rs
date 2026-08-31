//! What the biome scan costs the spawn tick, with and without `BiomeCache`. Dev tool, not part of
//! the server: it exists to check that reading the zone on every spawn attempt (which
//! `NPC.GetSpawnRate` requires) still fits in a tick at the 255-player bar.
//!
//! **This measures the worst tick, not the mean, and that distinction is the whole point.** An
//! earlier version drove a single slot for 60,000 ticks and multiplied its per-tick mean by 255.
//! That reported 345 us and was arithmetically correct: 255 players over a 60-tick refresh really is
//! 4.25 scans a tick. But multiplying a mean by the player count assumes the work spreads evenly,
//! and it does not. Clients arrive in a burst, so every slot fills on the same tick and every entry
//! then expires on the same tick. A real 255-player soak measured `phase=spawning phase_us=20763`,
//! which is 266 scans in one tick, over the entire frame budget, while this example was still
//! reporting 345 us. A per-tick mean over one player cannot see a per-tick maximum over 255.

use terrustia::game::spawn::{BiomeCache, biome_at};
use terrustia::world::worldgen;

/// Players in the burst, matching the server's own qualification bar.
const PLAYERS: usize = 255;

fn main() {
    let world = worldgen::generate(4200, 1200, "biome scan cost", 7);
    let x = world.width() / 2;
    let y = i32::from(world.surface) + 40;

    for _ in 0..20 {
        std::hint::black_box(biome_at(&world, x, y));
    }

    let runs = 500;
    let start = std::time::Instant::now();
    for i in 0..runs {
        std::hint::black_box(biome_at(&world, x + (i % 17), y));
    }
    let raw = start.elapsed().as_secs_f64() / f64::from(runs) * 1e6;

    // The join burst: every slot reads for the first time on tick 1, so every entry carries the
    // same age and they all come due together. Held long enough to cross several refresh windows.
    let mut cache = BiomeCache::default();
    let ticks = 600u64;
    let mut worst = 0.0f64;
    let mut worst_tick = 0u64;
    let mut total = 0.0f64;
    for tick in 1..=ticks {
        let start = std::time::Instant::now();
        cache.advance(tick);
        for slot in 0..PLAYERS {
            std::hint::black_box(cache.read(&world, slot, x, y));
        }
        let us = start.elapsed().as_secs_f64() * 1e6;
        total += us;
        if us > worst {
            worst = us;
            worst_tick = tick;
        }
    }
    let mean = total / ticks as f64;

    println!("biome_at             : {raw:.1} us per scan");
    println!(
        "  {PLAYERS} players, uncached: {:.0} us in one tick",
        raw * f64::from(u32::try_from(PLAYERS).unwrap_or(255))
    );
    println!();
    println!("cached, {PLAYERS} players in one burst, over {ticks} ticks:");
    println!("  mean per tick      : {mean:.1} us");
    println!("  WORST tick         : {worst:.1} us  (tick {worst_tick})");
    println!();
    println!("tick budget          : 16666.7 us");
    println!("worst tick is {:.2}% of budget", worst / 16666.7 * 100.0);
}
