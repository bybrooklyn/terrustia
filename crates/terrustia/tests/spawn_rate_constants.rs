//! Golden pins for `game::spawn::rates`'s modifiers against `NPC.GetSpawnRate` in the decompiled
//! 1.4.5.8 source (`NPC.cs`), so a drift in any one multiplier fails here naming exactly which one
//! moved, rather than only showing up as a vaguely wrong spawn feel.
//!
//! `spawn.rs`'s own `#[cfg(test)] mod rate_tests` already checks *direction* ("caverns are busier
//! than the surface"). This file adds the *exact* number for each modifier, each isolated by holding
//! every other axis at its neutral value, with the citation that makes the number checkable against
//! source rather than merely plausible.
//!
//! Every citation is to the non-`remixWorld` branch of `GetSpawnRate`: this server does not model
//! the "Don't Starve" seed. The zone chain, the `nearbyActiveNPCs` self-correction and the moon and
//! dungeon overrides are all modelled and pinned here now. What remains genuinely out of scope is
//! the set of branches whose *input* this server has no notion of: journey mode's slider (handled
//! by the caller instead), `getGoodWorld`, `ZoneSandstorm`, `cloudAlpha`, the dual-dungeon seeds,
//! the Wall of Flesh's underworld suppression, and every player-carried buff (candles, potions,
//! the sunflower, the angler set). `ZoneMeteor` and `ZoneLihzhardTemple` used to be on that list
//! and are not any more: both are zones this server now knows. The temple's own branch is pinned
//! below; the meteor's (`NPC.cs:636-640`) is modelled in `rates` but has no pin here yet.
//!
//! `rates()` keeps the running rate and cap as `f32` throughout and casts once at the very end,
//! where the game's own `GetSpawnRate` reassigns an `int` at every step and so truncates after each
//! multiplication. Every *rate* this file exercises lands on the same integer either way: the
//! intermediates are whole numbers or float noise a hair above one (600, 540, 480, 420, 390, 360,
//! 330, 324, 300, 252, 243, 240, 156, 120, 108, 48), and the one stacked chain that does produce
//! fractions (`the_bounds_are_exactly_60_and_15`) is clamped to the floor from either side. The
//! *caps* do diverge by design and always have: 5 * 1.9 is 9.5 here and 9 in the game, because this
//! module keeps the cap fractional until the caller compares against it. That is a property of
//! these inputs, not a general proof the two orders always agree.

use rand::SeedableRng;
use rand::rngs::SmallRng;
use terrustia::game::spawn::{Biome, Conditions, Depth, MAX_SPAWNS, SPAWN_RATE, rates};

/// A neutral world: plain forest surface, daytime, nothing running, nobody about.
///
/// `nearby_active_npcs` is far above the emptiness ramp's top rung (`maxSpawns * 0.8`,
/// `NPC.cs:668-680`) for any cap the modifiers below can build, so the ramp stays off and each pin
/// measures the one modifier it names. Zero would be the *fastest* case in the game, not the
/// neutral one, and would fold a x0.42 into every number in this file.
///
/// `downed_boss3: true` for the same reason: `NPC.cs:787-790` overrides the rate to a flat 10 in
/// the dungeon before Skeletron, which would swallow every other modifier in a dungeon pin.
fn plain() -> Conditions {
    Conditions {
        depth: Depth::Surface,
        biome: Biome::Forest,
        hard_mode: false,
        day_time: true,
        blood_moon: false,
        eclipse: false,
        event_moon: false,
        above_surface_line: true,
        town_npcs: 0,
        nearby_active_npcs: 1_000.0,
        below_dirt_midline: false,
        downed_boss3: true,
        behind_a_house_wall: false,
        active_players: 1,
        in_tower_zone: false,
        graveyard: false,
        meteor: false,
        lihzahrd_temple: false,
        // These constants are the rates a player with no luck effects sees, which is what every
        // number in this file was measured against.
        luck: 0.0,
    }
}

/// A fresh RNG for a call with `town_npcs: 0` (or an event overruling the town), which never
/// touches `rates`'s own town-suppression roll and so cannot care which one it gets.
fn any_rng() -> SmallRng {
    SmallRng::seed_from_u64(0)
}

/// `NPC.cs:6190`: `private static int defaultSpawnRate = 600;`
/// `NPC.cs:6192`: `private static int defaultMaxSpawns = 5;`
#[test]
fn the_baseline_matches_defaultspawnrate_and_defaultmaxspawns() {
    assert_eq!(SPAWN_RATE, 600, "NPC.cs:6190, defaultSpawnRate");
    assert_eq!(MAX_SPAWNS, 5.0, "NPC.cs:6192, defaultMaxSpawns");
    assert_eq!(
        rates(plain(), &mut any_rng()),
        (600, 5.0, false),
        "a plain surface daytime world"
    );
}

/// `NPC.cs:478-482`:
/// ```csharp
/// if (Main.hardMode) {
///     spawnRate = (int)((double)defaultSpawnRate * 0.9);
///     maxSpawns = defaultMaxSpawns + 1;
/// }
/// ```
/// The cap bump is additive (`+1`), not a multiplier — worth pinning separately since every other
/// modifier in this file is multiplicative and it would be easy to fold this one in wrongly.
#[test]
fn hardmode_is_09_times_the_rate_and_one_more_slot() {
    let (rate, cap, _) = rates(
        Conditions {
            hard_mode: true,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(rate, (600.0 * 0.9) as u32, "NPC.cs:480");
    assert_eq!(cap, 6.0, "NPC.cs:481, defaultMaxSpawns + 1");
}

/// `NPC.cs:483-486`:
/// ```csharp
/// if (player.position.Y > (float)(Main.UnderworldLayer * 16)) {
///     maxSpawns = (int)((float)maxSpawns * 2f);
/// }
/// ```
/// This is the first depth check in the method and the only one that never touches `spawnRate` at
/// all — every other depth branch changes both numbers.
#[test]
fn the_underworld_only_doubles_the_cap() {
    let (rate, cap, _) = rates(
        Conditions {
            depth: Depth::Underworld,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(rate, 600, "NPC.cs:483-486 never assigns spawnRate");
    assert_eq!(cap, 10.0, "NPC.cs:485, maxSpawns * 2f");
}

/// `NPC.cs:502-506`, the non-`remixWorld` branch below `rockLayer`:
/// ```csharp
/// spawnRate = (int)((double)spawnRate * 0.4);
/// maxSpawns = (int)((float)maxSpawns * 1.9f);
/// ```
#[test]
fn caverns_are_04_times_the_rate_and_19_times_the_cap() {
    let (rate, cap, _) = rates(
        Conditions {
            depth: Depth::Cavern,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(rate, (600.0 * 0.4) as u32, "NPC.cs:504");
    assert_eq!(cap, 5.0 * 1.9, "NPC.cs:505");
}

/// `NPC.cs:515-524`, the non-`remixWorld` branch below `worldSurface`:
/// ```csharp
/// else if (Main.hardMode) {
///     spawnRate = (int)((double)spawnRate * 0.45);
///     maxSpawns = (int)((float)maxSpawns * 1.8f);
/// } else {
///     spawnRate = (int)((double)spawnRate * 0.5);
///     maxSpawns = (int)((float)maxSpawns * 1.7f);
/// }
/// ```
/// The hardmode branch multiplies the `spawnRate` the earlier hardmode check (478-482) already
/// discounted, so the two stack: `600 * 0.9 * 0.45 = 243`, not `600 * 0.45 = 270`.
#[test]
fn underground_is_05_17_pre_hardmode_and_045_18_stacked_with_hardmode() {
    let (rate, cap, _) = rates(
        Conditions {
            depth: Depth::Underground,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(rate, (600.0 * 0.5) as u32, "NPC.cs:522, pre-hardmode");
    assert_eq!(cap, 5.0 * 1.7, "NPC.cs:523, pre-hardmode");

    let (rate, cap, _) = rates(
        Conditions {
            depth: Depth::Underground,
            hard_mode: true,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(
        rate, 243,
        "NPC.cs:517 stacked on NPC.cs:480: 600 * 0.9 * 0.45"
    );
    assert_eq!(
        cap,
        6.0 * 1.8,
        "NPC.cs:518, on the hardmode-bumped cap of 6"
    );
}

/// `NPC.cs:534-537`, the non-`remixWorld`, non-daytime surface branch:
/// ```csharp
/// spawnRate = (int)((double)spawnRate * 0.6);
/// maxSpawns = (int)((float)maxSpawns * 1.3f);
/// ```
#[test]
fn surface_night_is_06_times_the_rate_and_13_times_the_cap() {
    let (rate, cap, _) = rates(
        Conditions {
            day_time: false,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(rate, (600.0 * 0.6) as u32, "NPC.cs:536");
    assert_eq!(cap, 5.0 * 1.3, "NPC.cs:537");
}

/// `NPC.cs:538-542`, nested inside the non-daytime branch:
/// ```csharp
/// if (Main.bloodMoon) {
///     spawnRate = (int)((double)spawnRate * 0.3);
///     maxSpawns = (int)((float)maxSpawns * 1.8f);
/// }
/// ```
/// Stacks on the night discount just above: `600 * 0.6 * 0.3 = 108`.
#[test]
fn a_blood_moon_is_03_times_the_already_nighttime_rate() {
    let (rate, cap, _) = rates(
        Conditions {
            day_time: false,
            blood_moon: true,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(
        rate, 108,
        "NPC.cs:540 stacked on NPC.cs:536: 600 * 0.6 * 0.3"
    );
    assert_eq!(cap, 5.0 * 1.3 * 1.8, "NPC.cs:541, stacked on the night cap");
}

/// `NPC.cs:543-547`, also nested inside the non-daytime branch:
/// ```csharp
/// if ((Main.pumpkinMoon || Main.snowMoon) && ...) {
///     spawnRate = (int)((double)spawnRate * 0.2);
///     maxSpawns *= 2;
/// }
/// ```
/// A pumpkin/frost moon only ever runs at night, so this is exercised together with `day_time:
/// false` here, the same as the game's own nesting.
///
/// This branch is real but *unobservable in the result*: `NPC.cs:772-776` reassigns both numbers
/// further down, and every path that reaches this branch reaches that one too (identical
/// conditions). The 72 this used to pin was the whole finding, so the pin now names both halves and
/// asserts the one that survives. See `a_moon_overrides_the_clamped_rate_with_a_flat_20`.
#[test]
fn an_event_moon_is_02_times_the_already_nighttime_rate_and_then_overridden() {
    let (rate, cap, _) = rates(
        Conditions {
            day_time: false,
            event_moon: true,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(rate, 20, "NPC.cs:775 replaces NPC.cs:545's 72 outright");
    assert_eq!(cap, 5.0 * 2.3, "NPC.cs:774 replaces NPC.cs:546's 13");
}

/// `NPC.cs:549-553`, the daytime counterpart:
/// ```csharp
/// else if (Main.dayTime && Main.eclipse) {
///     spawnRate = (int)((double)spawnRate * 0.2);
///     maxSpawns = (int)((float)maxSpawns * 1.9f);
/// }
/// ```
#[test]
fn a_daytime_eclipse_is_02_times_the_rate_and_19_times_the_cap() {
    let (rate, cap, _) = rates(
        Conditions {
            eclipse: true,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(rate, (600.0 * 0.2) as u32, "NPC.cs:551");
    assert_eq!(cap, 5.0 * 1.9, "NPC.cs:552");
}

/// `NPC.cs:750-757`, the floor and ceiling every other modifier in the method is clamped inside:
/// ```csharp
/// if ((double)spawnRate < (double)defaultSpawnRate * 0.1) {
///     spawnRate = (int)((double)defaultSpawnRate * 0.1);
/// }
/// if (maxSpawns > defaultMaxSpawns * 3) {
///     maxSpawns = defaultMaxSpawns * 3;
/// }
/// ```
/// Stacking every *surface* event at once is what actually pushes both bounds: the underworld
/// branch (`NPC.cs:483-486`) never touches `spawnRate` at all, so `spawn.rs`'s own
/// `the_rate_is_bounded` test — which stacks `Depth::Underworld` with every event flag — proves the
/// values never fall below the floor without ever actually driving them there (`worst.0` lands at
/// `540`, from the hardmode discount alone, comfortably above the `60` floor its own assertion only
/// checks `>=` against). This test instead stacks night, a blood moon *and* an event moon on the
/// surface, which the code applies as three independent multiplications
/// (`spawn.rs`'s `Depth::Surface` arm, three separate `if`s, not `else if`s) and so is the
/// combination that actually reaches both clamps.
#[test]
fn the_bounds_are_exactly_60_and_15() {
    assert_eq!(
        (SPAWN_RATE as f32 * 0.1) as u32,
        60,
        "NPC.cs:752, defaultSpawnRate * 0.1"
    );
    assert_eq!(MAX_SPAWNS * 3.0, 15.0, "NPC.cs:756, defaultMaxSpawns * 3");

    // A hardmode blood-moon night in a cleared corruption: 600 * 0.9 * 0.6 * 0.3 * 0.65 = 63, then
    // both emptiness ladders (`NPC.cs:668`, `:686`, the evil qualifies for the second) take it to
    // 26.5, well under the floor. The cap goes 5 + 1, * 1.3, * 1.8, * 1.3 = 18.25, over the
    // ceiling. No moon: `NPC.cs:772-776` is an *assignment* placed after these clamps, so a moon
    // does not stack toward them, it replaces what they produced. Pinned separately in
    // `a_moon_overrides_the_clamped_rate_with_a_flat_20`.
    let worst = rates(
        Conditions {
            depth: Depth::Surface,
            biome: Biome::Corruption,
            hard_mode: true,
            day_time: false,
            blood_moon: true,
            nearby_active_npcs: 0.0,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(worst.0, 60, "clamped at the floor");
    assert_eq!(worst.1, 15.0, "clamped at the ceiling");
}

/// `NPC.cs:772-776`, the moon override, which is applied *after* the clamps above:
/// ```csharp
/// if ((Main.pumpkinMoon || Main.snowMoon) && (Main.remixWorld || (double)player.position.Y < Main.worldSurface * 16.0)) {
///     maxSpawns = (int)((double)defaultMaxSpawns * (2.0 + 0.3 * (double)numberOfActivePlayers));
///     spawnRate = 20;
/// }
/// ```
/// It assigns rather than multiplies, so nothing before it survives. `spawn.rs` reached 64 (night's
/// `0.6 * 0.2 * 600 = 72`, then the hardmode 0.9) or 72, against the game's flat 20: 3.2 to 3.6
/// times too slow, which is why the late waves of a pumpkin or frost moon were unreachable.
/// The cap is `5 * (2 + 0.3n)`, so 11 for one player and 12 (11.5 truncated by the caller's own
/// integer use of it) for two.
#[test]
fn a_moon_overrides_the_clamped_rate_with_a_flat_20() {
    let one = rates(
        Conditions {
            day_time: false,
            event_moon: true,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(one.0, 20, "NPC.cs:775, spawnRate = 20 flat");
    assert_eq!(one.1, 5.0 * 2.3, "NPC.cs:774, defaultMaxSpawns * (2 + 0.3)");

    // Hardmode, night and a blood moon on top would all have been clamped to 60; the override
    // still wins.
    let stacked = rates(
        Conditions {
            day_time: false,
            blood_moon: true,
            hard_mode: true,
            event_moon: true,
            active_players: 4,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(
        stacked.0, 20,
        "the override is absolute, not another multiplier"
    );
    assert_eq!(stacked.1, 5.0 * 3.2, "four players: 5 * (2 + 1.2)");
}

/// `NPC.cs:782-786`, the invasion override, which is what a lunar pillar's zone runs at:
/// ```csharp
/// if (invaders) {
///     maxSpawns = (int)((double)defaultMaxSpawns * (2.0 + 0.3 * (double)numberOfActivePlayers));
///     spawnRate = 20;
/// }
/// ```
/// A tower zone sets `invaders` outright (`NPC.cs:404-409`), so this is the pillar fight's rate
/// and there is no other. At the surrounding surface's ordinary 600 an escort of a hundred would
/// take about an hour of real time per pillar, which is why the numbers here are the whole
/// difference between a fight and an unbreakable shield.
#[test]
fn a_lunar_pillar_zone_overrides_the_rate_with_a_flat_20() {
    let one = rates(
        Conditions {
            in_tower_zone: true,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(one.0, 20, "NPC.cs:785, spawnRate = 20 flat");
    assert_eq!(one.1, 5.0 * 2.3, "NPC.cs:784, defaultMaxSpawns * (2 + 0.3)");

    // ...and it wins over the daytime forest's own clamped 600 with four people standing in it.
    let crowded = rates(
        Conditions {
            in_tower_zone: true,
            active_players: 4,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(crowded.0, 20);
    assert_eq!(crowded.1, 5.0 * 3.2, "four players: 5 * (2 + 1.2)");

    // The town does not quiet a pillar fight: `NPC.cs:800`'s gate opens with `!invaders`, so three
    // residents beside a tower cannot turn its escort into bunnies.
    let (_, _, friendly) = rates(
        Conditions {
            in_tower_zone: true,
            town_npcs: 3,
            ..plain()
        },
        &mut any_rng(),
    );
    assert!(!friendly, "NPC.cs:800, an event overrules the town");
}

/// `NPC.cs:591-595`, `NPC.cs:787-790`: the dungeon is busy, and before Skeletron it is relentless.
/// ```csharp
/// if (inDualDungeon || ZoneDungeon) { spawnRate *= 0.3; maxSpawns *= 1.8; }
/// ...
/// if (ZoneDungeon && !downedBoss3) { spawnRate = 10; }
/// ```
/// PR #32 landed the Dungeon Guardian this pairs with but not either rate, so a fresh character
/// met one every 240 to 600 ticks instead of every 10, which made early-dungeon farming practical.
#[test]
fn the_dungeon_is_three_times_busier_and_relentless_before_skeletron() {
    let after = rates(
        Conditions {
            biome: Biome::Dungeon,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(after.0, 180, "NPC.cs:593, spawnRate * 0.3");
    assert_eq!(after.1, 9.0, "NPC.cs:594, maxSpawns * 1.8");

    let before = rates(
        Conditions {
            biome: Biome::Dungeon,
            downed_boss3: false,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(before.0, 10, "NPC.cs:789, spawnRate = 10 flat");
    assert_eq!(before.1, 9.0, "the cap is untouched by that override");
}

/// `NPC.cs:603-660`, the rest of the zone chain this server can model. Each is isolated against
/// `plain()`, and each was entirely absent: `Conditions` carried no biome at all.
#[test]
fn every_modelled_biome_carries_its_own_rate_and_cap() {
    let one = |at: Conditions| rates(at, &mut any_rng());

    // NPC.cs:603-607, ZoneUndergroundDesert. Five times too quiet before this: it ran at the
    // cavern's own 240 and 9.5.
    //
    // Both of its numbers land outside the game's own clamps and are pinned at them, in vanilla as
    // here: the cavern band gives 600 * 0.4 = 240 and a 9.5 cap, then 240 * 0.2 = 48 is below the
    // 60 floor and 9.5 * 3 = 28.5 is above the 15 ceiling. So the observable result is the pair of
    // clamps, and the multipliers are what drive it there.
    let desert = one(Conditions {
        biome: Biome::Desert,
        depth: Depth::Cavern,
        ..plain()
    });
    assert_eq!(desert.0, 60, "spawnRate * 0.2, driven into the floor");
    assert_eq!(desert.1, 15.0, "maxSpawns * 3, driven into the ceiling");
    // ...and a *surface* desert is not the underground desert and takes nothing.
    assert_eq!(
        one(Conditions {
            biome: Biome::Desert,
            ..plain()
        }),
        one(plain()),
        "the surface desert has no branch of its own",
    );

    // NPC.cs:609-635, ZoneJungle, on the town headcount rather than a flat number.
    for (town, r, m) in [
        (0u32, 0.4, 1.5),
        (1, 0.55, 1.4),
        (2, 0.7, 1.3),
        (3, 0.85, 1.2),
    ] {
        let jungle = one(Conditions {
            biome: Biome::Jungle,
            town_npcs: town,
            ..plain()
        });
        // Town suppression applies on top of this for a non-zero headcount, and its friendly fork
        // leaves the rate alone, so only the cap is comparable across every headcount. Compare the
        // rate only where the town has no say.
        if town == 0 {
            assert_eq!(
                jungle.0,
                (600.0f32 * r) as u32,
                "jungle at {town} residents"
            );
        }
        let expected_cap = 5.0 * m * if town == 0 { 1.0 } else { 0.6 };
        assert!(
            (jungle.1 - expected_cap).abs() < 0.001 || jungle.1 == 5.0 * m,
            "jungle cap at {town} residents: {} vs {expected_cap}",
            jungle.1,
        );
    }

    // NPC.cs:637-641, either evil.
    for biome in [Biome::Corruption, Biome::Crimson] {
        let evil = one(Conditions { biome, ..plain() });
        assert_eq!(
            evil.0,
            (600.0f32 * 0.65) as u32,
            "{biome:?} spawnRate * 0.65"
        );
        assert_eq!(evil.1, 5.0 * 1.3, "{biome:?} maxSpawns * 1.3");
    }

    // NPC.cs:656-660, the hallow, and only below the rock layer.
    let deep_hallow = one(Conditions {
        biome: Biome::Hallow,
        depth: Depth::Cavern,
        ..plain()
    });
    assert_eq!(deep_hallow.0, (240.0f32 * 0.65) as u32, "spawnRate * 0.65");
    assert_eq!(deep_hallow.1, 9.5 * 1.3, "maxSpawns * 1.3");
    assert_eq!(
        one(Conditions {
            biome: Biome::Hallow,
            ..plain()
        }),
        one(plain()),
        "a surface hallow takes nothing: the branch is gated on the rock layer",
    );
}

/// `NPC.cs:641-650`, the Lihzahrd Temple, a separate `if` sitting between the biome chain and the
/// hallow's:
/// ```csharp
/// if (ZoneLihzhardTemple) {
///     spawnRate = (int)((float)spawnRate * 0.8f);
///     maxSpawns = (int)((float)maxSpawns * 1.2f);
///     if (Main.remixWorld) { ... }
/// }
/// ```
/// The `remixWorld` half is not modelled here. Because it is a separate `if` rather than another
/// arm of the chain above it, it stacks on whatever the biome already took, which for a real temple
/// is the jungle's: Lihzahrd brick is one of the game's own jungle zone tiles
/// (`SceneMetrics.cs:613`), so a temple is a jungle as far as `GetSpawnRate` is concerned.
#[test]
fn the_lihzahrd_temple_is_08_times_the_rate_and_12_times_the_cap() {
    let (rate, cap, _) = rates(
        Conditions {
            lihzahrd_temple: true,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(rate, (600.0f32 * 0.8) as u32, "NPC.cs:643");
    assert_eq!(cap, 5.0 * 1.2, "NPC.cs:644");

    // Stacked on the cavern jungle a real temple sits in: 600 * 0.4 * 0.4 * 0.8.
    let (rate, _, _) = rates(
        Conditions {
            lihzahrd_temple: true,
            biome: Biome::Jungle,
            depth: Depth::Cavern,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(
        rate, 76,
        "NPC.cs:643 stacked on NPC.cs:612 and NPC.cs:504: 600 * 0.4 * 0.4 * 0.8"
    );
}

/// `NPC.cs:668-698`, the emptiness ramp: two stacked ladders keyed on `nearbyActiveNPCs` against
/// the *running* `maxSpawns`. `nearby_active_npcs` was read only as a hard cap gate, so a cleared
/// area refilled up to 2.38 times slower than the game refills it.
#[test]
fn an_empty_area_refills_faster_than_a_crowded_one() {
    let at = |near: f32| {
        rates(
            Conditions {
                nearby_active_npcs: near,
                ..plain()
            },
            &mut any_rng(),
        )
        .0
    };
    // The cap here is a plain 5, so the rungs sit at 1, 2, 3 and 4.
    assert_eq!(
        at(0.0),
        (600.0f32 * 0.6) as u32,
        "NPC.cs:670, < 20% -> x0.6"
    );
    assert_eq!(
        at(1.5),
        (600.0f32 * 0.7) as u32,
        "NPC.cs:674, < 40% -> x0.7"
    );
    assert_eq!(
        at(2.5),
        (600.0f32 * 0.8) as u32,
        "NPC.cs:678, < 60% -> x0.8"
    );
    assert_eq!(
        at(3.5),
        (600.0f32 * 0.9) as u32,
        "NPC.cs:682, < 80% -> x0.9"
    );
    assert_eq!(at(4.5), 600, "at 80% or more the ramp is off");

    // NPC.cs:686-698, the second ladder, which only applies below the dirt-layer midline or in an
    // evil. Both stack, so an empty corrupt cavern is 0.6 * 0.7 = 0.42 of the base rate.
    let deep = rates(
        Conditions {
            nearby_active_npcs: 0.0,
            below_dirt_midline: true,
            ..plain()
        },
        &mut any_rng(),
    );
    assert_eq!(deep.0, (600.0f32 * 0.6 * 0.7) as u32, "both ladders stack");
    // 600 against 252 is the 2.38x an empty deep area was too slow by when neither ladder existed.
    assert_eq!(deep.0, 252);
}

/// Town suppression's real shape (C1-b item 8, fixed): `spawn.rs` used to model it as one
/// deterministic `(slower, fewer)` pair per headcount, applied the same way regardless of what the
/// game itself actually rolls. Real vanilla (`NPC.cs:795-924`) is gated on the same event
/// exclusions `spawn.rs` already models (no invasion, no active blood/pumpkin/snow moon at night,
/// no daytime eclipse, not in a corrupt/crimson/meteor/Old One's Army zone — `NPC.cs:800`), which
/// `spawn.rs`'s own `event` guard correctly mirrors in spirit. Past that gate, in the ordinary
/// surface/underground branch that covers every base a player actually builds (`NPC.cs:856-923`,
/// the underworld's own separate, simpler fork at `NPC.cs:802-855` is not modelled — a base built
/// in the underworld is the rare exception, not the case this quiets), it is a per-attempt coin
/// flip between two outcomes (`Main.rand.Next(...)`) rather than a fixed multiplier: some attempts
/// scale `spawnRate`, others leave it untouched and instead shrink `maxSpawns` and force the spawn
/// to be a friendly critter (`spawnFriendly = true`) instead of a monster. `rates` now returns that
/// third element directly (`(rate, cap, spawn_friendly)`) and takes an `&mut SmallRng` to roll it,
/// matching the shape of a genuinely probabilistic function rather than a table lookup.
///
/// `townNPCs >= 3` is the one headcount where classic (non-expert) mode is fully deterministic:
/// ```csharp
/// // NPC.cs:903-923, the townNPCs >= 3 branch, ordinary (non-graveyard) case:
/// else if (townNPCs >= 3) {
///     noWorms = true;
///     if (ZoneGraveyard && ...) { /* the graveyard sub-case, which `rates` also models; it is
///                                    `plain()`'s `graveyard: false` that keeps this pin on the
///                                    ordinary branch */ }
///     else {
///         if (!Main.expertMode || Main.rand.Next(30) != 0) {
///             spawnFriendly = true;
///         }
///         maxSpawns = (int)((double)(float)maxSpawns * 0.6);
///     }
/// }
/// ```
/// `!Main.expertMode` is unconditionally `true` in classic mode, so `spawnFriendly` is set on
/// *every* attempt — `spawnRate` is never assigned in this branch at all. `spawn.rs::rates` mirrors
/// this exactly: `town_npcs >= 3` always returns `spawn_friendly: true` and the unchanged base rate.
#[test]
fn town_suppression_at_three_residents_leaves_the_rate_unchanged_in_classic_mode() {
    let (base, base_cap, base_friendly) = rates(plain(), &mut any_rng());
    assert!(
        !base_friendly,
        "no town at all never forces a friendly spawn"
    );

    let mut rng = SmallRng::seed_from_u64(7);
    let (three, three_cap, three_friendly) = rates(
        Conditions {
            town_npcs: 3,
            ..plain()
        },
        &mut rng,
    );
    assert_eq!(
        three, base,
        "NPC.cs:917-921: classic mode always sets spawnFriendly instead of touching spawnRate"
    );
    assert!(three_friendly, "and always forces a friendly spawn");
    assert_eq!(three_cap, base_cap * 0.6, "NPC.cs:921, maxSpawns * 0.6");
}
