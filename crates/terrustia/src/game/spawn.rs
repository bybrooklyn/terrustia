//! Enemy spawning.
//!
//! The rate, cap and spawn area come from the game (`defaultSpawnRate` 600, `defaultMaxSpawns` 5,
//! an area 0.7 screens across with a 0.52-screen safe zone around the player). The pools are
//! transcribed from what `Spawner.SpawnAnNPC` can choose pre-hardmode for each depth, time and
//! biome.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::tile_solid::solid;

use super::{journey::JourneyPowers, npc::NpcStore, player::Player};
use crate::world::World;

/// Everything about the world that changes how fast things spawn.
///
/// The player-carried modifiers — water and peace candles, battle and calming potions, invisibility,
/// the sunflower, the angler set — are deliberately absent: the server does not model a player's
/// inventory or buffs, so it cannot know them. Everything here is world state the server owns.
///
/// Also absent, and for the same reason (the server has no notion of the thing they test):
/// `ZoneSandstorm`, `ZoneMeteor`, `ZoneLihzhardTemple`, `cloudAlpha`'s snow-in-a-storm bonus, the
/// dual-dungeon seeds, `getGoodWorld`, and the Wall of Flesh's underworld suppression
/// (`NPC.cs:662-666`). Journey mode's slider is real and is applied by the caller, where the power
/// lives, rather than threaded through here.
#[derive(Debug, Clone, Copy)]
pub struct Conditions {
    /// Which rate band the *player* is in, from [`rate_depth_at`] rather than [`depth_at`]: the
    /// game's rate bands carry a screen-height offset its pool tests do not.
    pub depth: Depth,
    /// The biome the player is standing in.
    ///
    /// `SetSpawnFlags` (`NPC.cs:382-397`) copies the player's own `Zone*` flags across, and
    /// `GetSpawnRate` reads them for a whole block of rate and cap modifiers (`NPC.cs:591-660`).
    /// This was missing entirely, so the underground desert was five times too quiet, the jungle
    /// two and a half times, and the corruption one and a half.
    pub biome: Biome,
    pub hard_mode: bool,
    pub day_time: bool,
    pub blood_moon: bool,
    pub eclipse: bool,
    /// A pumpkin or frost moon: `Main.pumpkinMoon || Main.snowMoon`, with no height test.
    ///
    /// The height test belongs to the two *rate* branches rather than to the flag: both carry
    /// `&& player.position.Y < Main.worldSurface * 16.0` (`NPC.cs:543` and `:772`), and the town
    /// gate that also reads the moon (`NPC.cs:800`) carries no such test, so folding it in here
    /// would quietly turn town suppression back on for anyone underground during a moon. See
    /// [`Self::above_surface_line`].
    pub event_moon: bool,
    /// `player.position.Y < Main.worldSurface * 16.0`: the height half of the two moon branches.
    ///
    /// Not derivable from [`Depth`], whose surface band sits a screen height lower
    /// (see [`rate_depth_at`]).
    pub above_surface_line: bool,
    /// Townsfolk living near the player.
    ///
    /// This is what makes a base safe, and it is the single most player-visible spawn rule in the
    /// game: with nobody home the wilderness comes to your door, and with three residents it stops.
    /// The game suppresses only when nothing else is going on — an invasion, a blood moon, an
    /// eclipse or a moon all overrule it, because an event that a town could turn off would not be
    /// much of an event.
    pub town_npcs: u32,
    /// `player.nearbyActiveNPCs`: the spawn weight already close to this player.
    ///
    /// This was read only as a hard cap gate. The game *also* ramps the rate down as the area
    /// empties (`NPC.cs:668-698`, two stacked ladders), so a cleared cave refills faster than a
    /// crowded one: up to 2.38x faster than we were managing.
    pub nearby_active_npcs: f32,
    /// Whether the player is below `(worldSurface + rockLayer) / 2`, which is the second emptiness
    /// ladder's own gate (`NPC.cs:686`). Not derivable from [`Depth`]: it is a midline between two
    /// of its boundaries.
    pub below_dirt_midline: bool,
    /// `downedBoss3`, for the dungeon's pre-Skeletron flat rate (`NPC.cs:787-790`).
    pub downed_boss3: bool,
    /// Whether the player is standing in front of a house wall.
    ///
    /// `NPC.cs:411`, `noWorms = WorldGen.InWorld(pX, pY) && Main.wallHouse[Main.tile[pX, pY].wall]`:
    /// the other half of the "walls keep things out" rule, and the half that stops burrowers rather
    /// than walkers.
    pub behind_a_house_wall: bool,
    /// `numberOfActivePlayers` (`NPC.cs:266`), which the moon override's cap is a function of.
    pub active_players: u32,
}

/// Which rate band a *player* is in, which is not the same question [`depth_at`] answers.
///
/// `GetSpawnRate`'s boundaries carry a screen height on top of the layer they name
/// (`NPC.cs:487`: `position.Y > Main.rockLayer * 16.0 + sHeight`; `:508` the same for
/// `worldSurface`), where `sHeight => 1200` px (`NPC.cs:6793`), which is 75 tiles. The *pool*
/// tests do not: `underGround` and `deeperThanRockLayer` (`NPC.cs:1144`, `:1204`) compare the
/// chosen tile against the bare layer. So the two need different functions, and sharing one put
/// every rate band 75 tiles too shallow, roughly doubling the rate through the dirt-layer band.
///
/// The underworld boundary has no offset in the game either (`NPC.cs:485`,
/// `position.Y > Main.UnderworldLayer * 16`), so it is the same on both sides.
pub fn rate_depth_at(world: &World, y: i32) -> Depth {
    /// `NPC.sHeight` (1200 px) in tiles.
    const SCREEN_TILES: i32 = 75;

    if y >= world.height() - UNDERWORLD_DEPTH {
        Depth::Underworld
    } else if y > i32::from(world.rock_layer) + SCREEN_TILES {
        Depth::Cavern
    } else if y > i32::from(world.surface) + SCREEN_TILES {
        Depth::Underground
    } else {
        Depth::Surface
    }
}

/// The spawn rate and cap for a set of conditions, after `NPC.GetSpawnRate`.
///
/// A flat 600/5 — the game's *surface daytime default* — was being used everywhere, so caverns
/// were about two and a half times too quiet, the underworld half as busy as it should be, and
/// neither hardmode nor a blood moon made any difference at all.
///
/// Returns `(one_in_n_per_tick, cap, spawn_friendly)`. A *lower* rate means more spawning, which
/// is the game's own convention and reads backwards until you know it. `spawn_friendly` is real
/// vanilla's own `spawnFriendly` (`NPC.cs`): when true, this attempt should draw a friendly
/// critter instead of a monster rather than being throttled by the rate at all — see the town
/// suppression block below for where it comes from. `rng` is only ever consulted there; every
/// other modifier in this function is a deterministic fact about the world.
///
/// The same block's other output, `noWorms`, is [`no_worms`] instead of a fourth element here,
/// because for everything this server models it needs no roll.
pub fn rates(at: Conditions, rng: &mut SmallRng) -> (u32, f32, bool) {
    let mut rate = SPAWN_RATE as f32;
    let mut max = MAX_SPAWNS;

    if at.hard_mode {
        rate *= 0.9;
        max += 1.0;
    }

    match at.depth {
        Depth::Underworld => max *= 2.0,
        Depth::Cavern => {
            rate *= 0.4;
            max *= 1.9;
        }
        Depth::Underground => {
            let (r, m) = if at.hard_mode {
                (0.45, 1.8)
            } else {
                (0.5, 1.7)
            };
            rate *= r;
            max *= m;
        }
        Depth::Surface => {
            if !at.day_time {
                rate *= 0.6;
                max *= 1.3;
                if at.blood_moon {
                    rate *= 0.3;
                    max *= 1.8;
                }
                // NPC.cs:543, with its own `position.Y < worldSurface * 16` half.
                if at.event_moon && at.above_surface_line {
                    rate *= 0.2;
                    max *= 2.0;
                }
            } else if at.eclipse {
                rate *= 0.2;
                max *= 1.9;
            }
        }
    }

    // The biome block, `NPC.cs:591-660`. It is one `if`/`else if` chain in the game, so a dungeon
    // takes its own modifier and none of the others; only the hallow's is a separate `if`. Five
    // branches of that chain are absent here because this server has no notion of the zone they
    // test: `ZoneSandstorm`, `ZoneMeteor`, `ZoneLihzhardTemple`, `inDualDungeon` and
    // `tresspassingDualDungeon`. `ZoneUndergroundDesert` is real vanilla's
    // "desert + below the surface + a sandstone or hardened-sand wall that is not a house wall"
    // (`SceneMetrics.cs:699`), narrowed here to the first two, since the server does not track
    // which wall a player is standing in front of.
    match at.biome {
        // NPC.cs:591-595.
        Biome::Dungeon => {
            rate *= 0.3;
            max *= 1.8;
        }
        // NPC.cs:603-607.
        Biome::Desert if at.depth != Depth::Surface => {
            rate *= 0.2;
            max *= 3.0;
        }
        // NPC.cs:609-635: the jungle thins out as a town fills up, on its own ladder rather than
        // the general town suppression further down (which it does not replace: both apply).
        Biome::Jungle => {
            let (r, m) = match at.town_npcs {
                0 => (0.4, 1.5),
                1 => (0.55, 1.4),
                2 => (0.7, 1.3),
                _ => (0.85, 1.2),
            };
            rate *= r;
            max *= m;
        }
        // NPC.cs:637-641.
        Biome::Corruption | Biome::Crimson => {
            rate *= 0.65;
            max *= 1.3;
        }
        _ => {}
    }
    // NPC.cs:656-660, a separate `if`: the hallow is busier only below the rock layer.
    if at.biome == Biome::Hallow && matches!(at.depth, Depth::Cavern | Depth::Underworld) {
        rate *= 0.65;
        max *= 1.3;
    }

    // The emptiness ramp, `NPC.cs:668-698`. Two stacked ladders: everywhere, then again below the
    // dirt-layer midline or in either evil. An area that has been cleared refills faster than one
    // that is still full, which is what stops a farmed cave going quiet for minutes at a time.
    // Both read the *running* `maxSpawns`, before its own ceiling is applied, as the game does.
    let near = at.nearby_active_npcs;
    if near < max * 0.2 {
        rate *= 0.6;
    } else if near < max * 0.4 {
        rate *= 0.7;
    } else if near < max * 0.6 {
        rate *= 0.8;
    } else if near < max * 0.8 {
        rate *= 0.9;
    }
    if at.below_dirt_midline || matches!(at.biome, Biome::Corruption | Biome::Crimson) {
        if near < max * 0.2 {
            rate *= 0.7;
        } else if near < max * 0.4 {
            rate *= 0.9;
        }
    }

    // The game's own floor and ceiling, which stop a stack of modifiers running away
    // (`NPC.cs:738-745`). Everything below this point is an *override*: the game assigns rather
    // than multiplies, so a clamp placed after them would undo them. Putting the clamps last is
    // what made a pumpkin moon three and a half times too slow.
    rate = rate.max(SPAWN_RATE as f32 * 0.1);
    max = max.min(MAX_SPAWNS * 3.0);

    // `NPC.cs:772-776`, the moon override, absolute in both directions: it replaces the rate with a
    // flat 20 and the cap with a function of the party size, whatever the clamps just said. Reached
    // at 64 or 72 before this, against the game's 20.
    if at.event_moon && at.above_surface_line {
        max = MAX_SPAWNS * (2.0 + 0.3 * at.active_players as f32);
        rate = 20.0;
    }

    // `NPC.cs:787-790`: below the dungeon before Skeletron falls, the rate is a flat 10, which is
    // the pressure that makes early-dungeon farming impractical. The Dungeon Guardian this pairs
    // with landed in PR #32; the rate did not, so it arrived every 240 to 600 ticks instead.
    if at.biome == Biome::Dungeon && !at.downed_boss3 {
        rate = 10.0;
    }

    // Townsfolk quiet the place down, but only when nothing else is happening: an event overrules
    // them, so a blood moon still comes to a full town. Real vanilla (`NPC.cs:795-924`) is not a
    // flat multiplier here: past the event gate, every attempt is a coin flip between throttling
    // `spawnRate` and leaving it alone while shrinking `maxSpawns` and forcing the spawn to be a
    // friendly critter instead of a monster (`spawnFriendly`). Two things are deliberately not
    // modelled: the underworld's own separate, simpler fork (`NPC.cs:802-855`) — a base built in
    // the underworld is the rare exception, not the case this quiets — and the `ZoneGraveyard`
    // sub-case inside each branch below, since this project has no notion of a graveyard zone at
    // all yet (the same reason `ZonePeaceCandle` and the rest of a player's own buffs are already
    // out of `Conditions`' scope, per this struct's own doc comment).
    let mut spawn_friendly = false;
    if town_suppression_applies(at) {
        match at.town_npcs {
            0 => {}
            // NPC.cs:870-878, the ordinary (non-graveyard) case: a one-in-three chance forces a
            // friendly spawn and shrinks the cap; the other two-in-three simply double the rate.
            1 => {
                if rng.random_ratio(1, 3) {
                    spawn_friendly = true;
                    max *= 0.6;
                } else {
                    rate *= 2.0;
                }
            }
            // NPC.cs:893-901: the odds flip to two-in-three for the friendly fork, and the rate
            // triples on the remaining one-in-three.
            2 => {
                if !rng.random_ratio(1, 3) {
                    spawn_friendly = true;
                    max *= 0.6;
                } else {
                    rate *= 3.0;
                }
            }
            // NPC.cs:917-921: `!Main.expertMode` is unconditionally true in classic mode, so this
            // branch sets `spawnFriendly` on *every* attempt rather than rolling for it —
            // `spawnRate` is never assigned here at all. Expert mode's own further
            // `Main.rand.Next(30) != 0` (a 29-in-30 chance) is folded into the same unconditional
            // case rather than threading a whole difficulty flag through `Conditions` for a
            // 1-in-30 edge: friendly wins the overwhelming majority of the time in expert mode
            // too, and this module already accepts small, disclosed over-approximations like it
            // elsewhere.
            _ => {
                spawn_friendly = true;
                max *= 0.6;
            }
        }
    }

    // `NPC.cs:925-929` ends the function with a `RollOnlyBadLuckExtreme(50) == 0` bonus of
    // `rate * 0.85` and `cap * 1.15`. It is deliberately not transcribed, because it can never
    // fire here: `Luck.RollOnlyBadLuckExtreme` (`Terraria.GameContent/Luck.cs:53-60`) returns -1
    // unless `luck < 0`, and this server does not model player luck at all, so its players are at
    // luck 0 exactly as a vanilla player with no luck effects is. Vanilla skips it for them too.

    (rate as u32, max.max(1.0), spawn_friendly)
}

/// `noWorms`: whether burrowers are kept out of this attempt's draw.
///
/// Its own function rather than a fourth thing [`rates`] returns, because for everything this
/// server models it is decided without a roll. Two sources:
///
/// * the wall at the player's own back (`NPC.cs:411`,
///   `noWorms = WorldGen.InWorld(pX, pY) && Main.wallHouse[Main.tile[pX, pY].wall]`);
/// * the town, which sets it unconditionally in all three headcount branches of the ordinary
///   surface/underground fork (`NPC.cs:858`, `:883`, `:905`), behind the same event gate the rest of
///   town suppression sits behind (`NPC.cs:800`).
///
/// Only the underworld's own fork rolls for it (`NPC.cs:810` one in two, `:827` three in four,
/// `:843` nine in ten), and that fork is already disclosed in [`rates`] as not modelled.
/// `ZoneShadowCandle` clearing it again (`NPC.cs:420-424`) is a player-carried effect, out of
/// [`Conditions`]' scope like every other one.
pub fn no_worms(at: Conditions) -> bool {
    at.behind_a_house_wall || (town_suppression_applies(at) && at.town_npcs >= 1)
}

/// The gate the whole town-suppression block sits behind, `NPC.cs:800`:
///
/// ```csharp
/// if (!invaders && ((!Main.bloodMoon && !Main.pumpkinMoon && !Main.snowMoon) || Main.dayTime)
///     && (!Main.eclipse || !Main.dayTime) && !flag && !ZoneCrimson && !ZoneMeteor
///     && !ZoneOldOneArmy)
/// ```
///
/// where `flag` is `ZoneCorrupt || ZoneCrimson`. So an event overrules the town, and so does simply
/// standing in an evil: a corrupt base is never quiet, however many people live in it. That last
/// clause could not be modelled until `Conditions` carried a biome at all.
///
/// Note the moon here is tested with **no** height condition, unlike the two rate branches: a
/// player underground during a pumpkin moon still has town suppression switched off.
///
/// Three of the game's exclusions are dropped, each because the thing they test does not exist
/// here: `invaders` (invasions do not route through this function), `ZoneMeteor` and
/// `ZoneOldOneArmy`. `Main.infectedSeed`, which would clear `flag` again, is likewise unmodelled.
fn town_suppression_applies(at: Conditions) -> bool {
    ((!at.blood_moon && !at.event_moon) || at.day_time)
        && (!at.eclipse || !at.day_time)
        && !matches!(at.biome, Biome::Corruption | Biome::Crimson)
}

/// The burrowers whose spawn branch vanilla gates on `noWorms`, out of the types this server fields.
///
/// `NPC.cs:3704-3713` is the Devourer / World Feeder branch, and it is the only one of the game's
/// several `!noWorms` gates naming a type that appears in these pools. Deliberately *not* here: the
/// underworld's Bone Serpent, which the game spawns with no such gate at all (`NPC.cs:4885`,
/// `Main.rand.Next(40) == 0 && !AnyNPCs(39)`). The rest of the game's gates (`NPC.cs:1409` the
/// Wyvern, `:3973`, `:4062`, `:1698`) name hardmode worms with no pool here.
const NO_WORMS_GATES: [u16; 2] = [
    7,  // DevourerHead
    98, // SeekerHead, the World Feeder
];

#[cfg(test)]
mod rate_tests {
    use super::*;
    use rand::SeedableRng;

    /// A neutral world: plain forest surface, daytime, nothing running, nobody about.
    ///
    /// `nearby_active_npcs` is deliberately *not* zero. An empty area is the game's fastest case,
    /// not its neutral one (`NPC.cs:668`, rate x0.6), so pinning a modifier against an empty
    /// baseline would fold that ramp into every number here. This is far above the ramp's top rung
    /// (`maxSpawns * 0.8`) for any cap the modifiers can build, so it leaves the ramp off entirely
    /// and each pin measures the one modifier it names.
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
        }
    }

    /// A fresh RNG for a call that never touches the town-suppression roll (`town_npcs: 0`, or an
    /// event overruling it) and so does not care which one it gets.
    fn any_rng() -> SmallRng {
        SmallRng::seed_from_u64(0)
    }

    /// Going down makes the world busier, which is most of what depth is for.
    #[test]
    fn caverns_are_busier_than_the_surface() {
        let (surface, surface_cap, _) = rates(plain(), &mut any_rng());
        let (cavern, cavern_cap, _) = rates(
            Conditions {
                depth: Depth::Cavern,
                ..plain()
            },
            &mut any_rng(),
        );
        assert!(
            cavern < surface,
            "a lower rate means more spawning: {cavern} vs {surface}",
        );
        assert!(cavern_cap > surface_cap);
        // The game's figure is 0.4x the rate. A flat 600 everywhere made caves this much too quiet.
        assert_eq!(cavern, (surface as f32 * 0.4) as u32);
    }

    /// Night, a blood moon and an eclipse each raise the surface's rate.
    #[test]
    fn events_make_the_surface_dangerous() {
        let (day, _, _) = rates(plain(), &mut any_rng());
        let (night, _, _) = rates(
            Conditions {
                day_time: false,
                ..plain()
            },
            &mut any_rng(),
        );
        let (blood, blood_cap, _) = rates(
            Conditions {
                day_time: false,
                blood_moon: true,
                ..plain()
            },
            &mut any_rng(),
        );
        let (eclipse, _, _) = rates(
            Conditions {
                eclipse: true,
                ..plain()
            },
            &mut any_rng(),
        );

        assert!(night < day, "night is busier than day");
        assert!(
            blood < night,
            "a blood moon is busier than an ordinary night"
        );
        assert!(eclipse < day, "an eclipse is busier than a plain day");
        assert!(blood_cap > rates(plain(), &mut any_rng()).1);
    }

    /// A town of one or two residents forks every attempt between throttling the rate and
    /// shrinking the cap while forcing a friendly spawn — real vanilla is not a flat multiplier
    /// here (`NPC.cs:870-901`), so this samples many attempts rather than asserting one.
    #[test]
    fn one_or_two_residents_fork_between_a_slower_rate_and_a_smaller_friendly_cap() {
        let (wild, wild_cap, wild_friendly) = rates(plain(), &mut any_rng());
        assert!(
            !wild_friendly,
            "no town at all never forces a friendly spawn"
        );

        let mut rng = SmallRng::seed_from_u64(1);
        for town_npcs in [1, 2] {
            let mut saw_slower_rate = false;
            let mut saw_smaller_friendly_cap = false;
            for _ in 0..200 {
                let (rate, cap, friendly) = rates(
                    Conditions {
                        town_npcs,
                        ..plain()
                    },
                    &mut rng,
                );
                if friendly {
                    assert_eq!(cap, wild_cap * 0.6, "the friendly fork shrinks the cap");
                    assert_eq!(rate, wild, "and leaves the rate exactly where it was");
                    saw_smaller_friendly_cap = true;
                } else {
                    assert!(rate > wild, "the other fork should slow the rate");
                    assert_eq!(cap, wild_cap, "and leaves the cap exactly where it was");
                    saw_slower_rate = true;
                }
            }
            assert!(
                saw_slower_rate,
                "town_npcs {town_npcs}: 200 trials never rolled the rate fork"
            );
            assert!(
                saw_smaller_friendly_cap,
                "town_npcs {town_npcs}: 200 trials never rolled the friendly fork"
            );
        }
    }

    /// Three or more residents is the one headcount classic mode makes fully deterministic:
    /// every attempt forces a friendly spawn and shrinks the cap, and the rate is never touched
    /// at all (`NPC.cs:917-921`) — not throttled further, the way a flat multiplier would.
    #[test]
    fn three_or_more_residents_always_forces_a_friendly_spawn_at_the_unchanged_rate() {
        let (wild, wild_cap, _) = rates(plain(), &mut any_rng());
        let mut rng = SmallRng::seed_from_u64(2);
        for town_npcs in 3..8 {
            let (rate, cap, friendly) = rates(
                Conditions {
                    town_npcs,
                    ..plain()
                },
                &mut rng,
            );
            assert_eq!(rate, wild, "townNPCs >= 3 never assigns spawnRate");
            assert!(friendly, "and always forces a friendly spawn");
            assert_eq!(cap, wild_cap * 0.6);
        }
    }

    /// An event overrules the town: a blood moon still comes to a full street.
    #[test]
    fn an_event_ignores_the_town() {
        let quiet_night = rates(
            Conditions {
                day_time: false,
                town_npcs: 3,
                ..plain()
            },
            &mut any_rng(),
        );
        let blood_night = rates(
            Conditions {
                day_time: false,
                blood_moon: true,
                town_npcs: 3,
                ..plain()
            },
            &mut any_rng(),
        );
        assert!(
            blood_night.0 < quiet_night.0,
            "a town that could switch off a blood moon would not be much of an event",
        );
        assert!(
            !blood_night.2,
            "and an event never forces a friendly spawn either"
        );
    }

    /// The gate the whole town-suppression block sits behind (`NPC.cs:800`), which has two clauses
    /// worth their own pin.
    ///
    /// An evil is never quiet, however many people live in it: `!flag && !ZoneCrimson` where
    /// `flag = ZoneCorrupt || ZoneCrimson`. That clause could not be modelled at all until
    /// `Conditions` carried a biome.
    ///
    /// And the moon is tested there with no height condition, unlike the two rate branches, so a
    /// player underground during a pumpkin moon still has town suppression switched off.
    #[test]
    fn an_evil_is_never_quieted_by_a_town_and_a_moon_switches_it_off_at_any_depth() {
        let mut rng = SmallRng::seed_from_u64(3);
        for biome in [Biome::Corruption, Biome::Crimson] {
            for _ in 0..50 {
                let (_, cap, friendly) = rates(
                    Conditions {
                        biome,
                        town_npcs: 5,
                        ..plain()
                    },
                    &mut rng,
                );
                assert!(!friendly, "{biome:?} never draws a friendly for a town");
                assert_eq!(cap, 5.0 * 1.3, "and its cap is the evil's, not a town's");
            }
        }
        // A forest of the same headcount does get quieted, so the biome is what did it.
        assert!(
            rates(
                Conditions {
                    town_npcs: 5,
                    ..plain()
                },
                &mut rng,
            )
            .2
        );

        // Underground, during a moon, with a full town: no suppression, because the gate's moon
        // clause carries no height test.
        let deep_moon = Conditions {
            depth: Depth::Cavern,
            above_surface_line: false,
            event_moon: true,
            day_time: false,
            town_npcs: 5,
            ..plain()
        };
        assert!(!rates(deep_moon, &mut rng).2, "a moon overrules the town");
        assert!(!no_worms(deep_moon), "and its noWorms with it");
        // ...but the moon's own rate override does not reach down here.
        assert_ne!(rates(deep_moon, &mut rng).0, 20);
    }

    /// `noWorms` keeps burrowers out when there is a town or a wall at your back, and an event
    /// overrules the town half of that exactly as it overrules the rest of town suppression
    /// (`NPC.cs:411`, `:800`, `:858`, `:883`, `:905`).
    ///
    /// Fails before the fix, when `noWorms` was not modelled at all: Devourers came straight
    /// through a town and through a walled base's own walls.
    #[test]
    fn a_town_or_a_wall_keeps_the_burrowers_out() {
        assert!(!no_worms(plain()), "an empty wilderness has worms in it");
        for town in 1..5 {
            assert!(
                no_worms(Conditions {
                    town_npcs: town,
                    ..plain()
                }),
                "{town} residents should stop worms",
            );
        }
        assert!(
            no_worms(Conditions {
                behind_a_house_wall: true,
                ..plain()
            }),
            "so should a wall at your own back, with no town at all",
        );
        // An event overrules the town, but not the wall.
        for town in 0..5 {
            assert!(
                !no_worms(Conditions {
                    town_npcs: town,
                    blood_moon: true,
                    day_time: false,
                    ..plain()
                }),
                "a blood moon brings worms to a town of {town}",
            );
        }
        assert!(
            no_worms(Conditions {
                behind_a_house_wall: true,
                blood_moon: true,
                day_time: false,
                ..plain()
            }),
            "the wall is not part of the town's event gate",
        );

        // The gated set is the Devourer branch and nothing else this server fields.
        assert_eq!(NO_WORMS_GATES, [7, 98]);
        assert!(
            !NO_WORMS_GATES.contains(&39),
            "the underworld's Bone Serpent has no such gate in the game (NPC.cs:4885)",
        );
    }

    /// However the modifiers stack, they stay inside the game's own floor and ceiling.
    #[test]
    fn the_rate_is_bounded() {
        // No moon here: the moon override (`NPC.cs:772-776`) is *outside* the clamps by design and
        // sets a flat 20, so including it would be asking the clamps to bound something the game
        // deliberately puts beyond them. It has its own pin below.
        let worst = rates(
            Conditions {
                depth: Depth::Underworld,
                hard_mode: true,
                day_time: false,
                blood_moon: true,
                nearby_active_npcs: 0.0,
                below_dirt_midline: true,
                ..plain()
            },
            &mut any_rng(),
        );
        assert!(worst.0 >= (SPAWN_RATE as f32 * 0.1) as u32, "{worst:?}");
        assert!(worst.1 <= MAX_SPAWNS * 3.0, "{worst:?}");
    }
}

/// The baseline: `NPC.defaultSpawnRate`, before anything modifies it.
pub const SPAWN_RATE: u32 = 600;

/// Spawn slots a single player supports.
pub const MAX_SPAWNS: f32 = 5.0;

/// Spawn area around the player, in tiles: roughly 0.7 of a 1080p screen.
pub const SPAWN_RANGE_X: i32 = 84;
pub const SPAWN_RANGE_Y: i32 = 47;

/// Nothing spawns inside this box around the player.
pub const SAFE_RANGE_X: i32 = 62;
pub const SAFE_RANGE_Y: i32 = 35;

/// How deep below the world's bottom the underworld begins.
pub const UNDERWORLD_DEPTH: i32 = 200;

/// Where in the world column a spawn point sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Surface,
    Underground,
    Cavern,
    Underworld,
}

/// Which biome the surrounding tiles say we are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Biome {
    Forest,
    Corruption,
    Crimson,
    Jungle,
    Snow,
    Desert,
    Ocean,
    Dungeon,
    /// Only exists after the wall falls, which is why nothing recognised it before hardmode did.
    Hallow,
}

/// What hardmode adds where, on top of whatever the place had before.
///
/// Every entry was read out of `NPC.Spawner` with its real condition rather than from memory: the
/// zone it needs, the depth, and whether it wants day or night. A pool that named the wrong biome
/// would be worse than an empty one, because it would look right.
///
/// These *add* to the ordinary pool rather than replacing it — a hardmode forest still has
/// zombies — except in the hallow, which has no pre-hardmode life of its own.
pub fn hardmode_pool(depth: Depth, biome: Biome, day: bool) -> &'static [u16] {
    use Biome::*;
    use Depth::*;

    match (depth, biome) {
        // The two evils, which are the same shape with different names.
        (Surface, Corruption) => &[
            81,  // CorruptSlime
            121, // Slimer
            94,  // Corruptor
        ],
        (Underground | Cavern, Corruption) => &[
            81,  // CorruptSlime
            83,  // CursedHammer
            94,  // Corruptor
            98,  // SeekerHead — a world feeder
            170, // PigronCorruption
            473, // BigMimicCorruption
        ],
        (Surface, Crimson) => &[
            183, // Crimslime
            241, // BloodFeeder
            242, // BloodJelly
        ],
        (Underground | Cavern, Crimson) => &[
            174, // Herpling
            179, // CrimsonAxe
            180, // PigronCrimson
            182, // FloatyGross
            183, // Crimslime
            268, // IchorSticker
            474, // BigMimicCrimson
        ],
        // The hallow, which has nothing but hardmode life.
        (Surface, Hallow) => {
            if day {
                &[
                    75,  // Pixie
                    122, // Gastropod
                ]
            } else {
                &[
                    75,  // Pixie
                    122, // Gastropod
                    137, // IlluminantBat
                    138, // IlluminantSlime
                ]
            }
        }
        (Underground | Cavern, Hallow) => &[
            75,  // Pixie
            84,  // EnchantedSword
            120, // ChaosElemental
            137, // IlluminantBat
            138, // IlluminantSlime
            171, // PigronHallow
            475, // BigMimicHallow
        ],
        // The snow, which gets a great deal.
        (Surface, Snow) => &[
            197, // ArmoredViking
            243, // IceGolem
            250, // AngryNimbus
        ],
        (Underground | Cavern, Snow) => &[
            95,  // DiggerHead
            150, // IceBat
            154, // IceTortoise
            184, // SpikedIceSlime
            197, // ArmoredViking
            206, // IcyMerman
            629, // IceMimic
        ],
        // The jungle.
        (Surface, Jungle) => {
            if day {
                &[
                    177, // Derpling
                    153, // GiantTortoise
                ]
            } else {
                &[
                    152, // GiantFlyingFox
                    153, // GiantTortoise
                ]
            }
        }
        (Underground | Cavern, Jungle) => &[
            157, // Arapaima
            176, // MossHornet
            205, // Moth
            236, // JungleCreeper
            476, // BigMimicJungle
        ],
        // The desert, whose hardmode life is the underground half of it.
        (Underground | Cavern, Desert) => &[
            78,  // Mummy
            79,  // DarkMummy
            80,  // LightMummy
            510, // DuneSplicerHead
        ],
        // The underworld, which only opens up once a mechanical boss is down; the caller holds
        // that gate because it is progression rather than place.
        (Underworld, _) => &[
            151, // Lavabat
            156, // RedDevil
        ],
        // An ordinary forest surface at night.
        (Surface, _) => {
            if day {
                &[]
            } else {
                &[
                    82,  // Wraith
                    93,  // GiantBat
                    133, // WanderingEye
                    140, // PossessedArmor
                ]
            }
        }
        // ...and everything under it.
        (Underground | Cavern, _) => &[
            77,  // ArmoredSkeleton
            85,  // Mimic
            93,  // GiantBat
            110, // SkeletonArcher
            141, // ToxicSludge
            163, // BlackRecluse
            172, // RuneWizard
        ],
    }
}

/// What a blood moon adds to the surface at night.
///
/// It does not replace the night's pool — a blood moon night still has zombies — it widens it, and
/// the widening is what makes one worth fighting through rather than sleeping past. The Clown only
/// comes in hardmode, which is why he is not simply on the list.
pub fn blood_moon_pool(depth: Depth, hard_mode: bool) -> &'static [u16] {
    const EARLY: [u16; 2] = [
        489, // BloodZombie
        490, // Drippler
    ];
    const LATE: [u16; 3] = [
        489, // BloodZombie
        490, // Drippler
        109, // Clown
    ];
    if depth != Depth::Surface {
        return &[];
    }
    if hard_mode { &LATE } else { &EARLY }
}

/// Classify a spawn point by how far down it is.
pub fn depth_at(world: &World, y: i32) -> Depth {
    if y >= world.height() - UNDERWORLD_DEPTH {
        Depth::Underworld
    } else if y >= i32::from(world.rock_layer) {
        Depth::Cavern
    } else if y >= i32::from(world.surface) {
        Depth::Underground
    } else {
        Depth::Surface
    }
}

/// Half-extents of the biome scan box, in tiles.
///
/// The game scans `SceneMetrics.ZoneScanSize`, a 169-by-124 tile box centred on the tile it is
/// asked about (`SceneMetrics.cs:16`: `1920/16 + 25*2 - 1 = 169` across, `1200/16 + 25*2 - 1 = 124`
/// down). This used a 41-by-41 box (radius 20), which is small enough to miss a biome the player is
/// plainly standing in and large enough only to be fooled by a stray vein: both directions of
/// wrong. 169 is odd, so it is symmetric at plus-or-minus 84; 124 is even, taken as -62..=61.
const BIOME_SCAN_X: i32 = 84;
const BIOME_SCAN_Y_UP: i32 = 62;
const BIOME_SCAN_Y_DOWN: i32 = 61;

/// The per-biome tile counts a scan must reach for the place to read as that biome.
///
/// These are the game's own `SceneMetrics` thresholds, not one flat number: the evils and the
/// hallow are cheap to declare (a small pocket counts), snow and the desert are dear (a genuine
/// biome, not a stray patch). A single flat 60 was used for all of them, which made a handful of
/// sand read as a desert and a real snow field read as forest. (`SceneMetrics.cs:24-58`,`154-175`.)
const EVIL_THRESHOLD: i32 = 300; // CorruptionTileThreshold
const BLOOD_THRESHOLD: i32 = 300; // CrimsonTileThreshold
const HOLY_THRESHOLD: i32 = 125; // HallowTileThreshold
const JUNGLE_THRESHOLD: i32 = 140; // JungleTileThreshold
const SNOW_THRESHOLD: i32 = 1500; // SnowTileNormalThreshold
const DESERT_THRESHOLD: i32 = 1500; // DesertTileNormalThreshold
const DUNGEON_THRESHOLD: i32 = 250; // DungeonTileThreshold

/// Work out the biome from the tiles around a point, the way the game counts zone tiles.
///
/// Faithful to `SceneMetrics.ScanTiles`/`AggregateTileCounts`/`CalculateZones` in its scan box, its
/// per-biome tile lists and thresholds, and its evil-versus-hallow subtraction. Two disclosed
/// narrowings: the ocean is decided by position rather than by `oceanDepths`, as it was before; and
/// the sunflower (`tile 27`) and a couple of hardmode-only additions the game folds into the evil
/// and blood counts are omitted, because this server does not place them. The dungeon is taken on
/// its tile count alone rather than also requiring a dungeon wall at the centre, since a run of
/// dungeon brick is dungeon enough and the wall is not always modelled where this is called.
pub fn biome_at(world: &World, x: i32, y: i32) -> Biome {
    // The ocean is defined by position rather than tiles.
    if x < 250 || x > world.width() - 250 {
        return Biome::Ocean;
    }

    // The game's own per-biome tile lists (`SceneMetrics.AggregateTileCounts`). A tile can belong
    // to several at once (corrupt sandstone is both evil and sand, hallowed ice both holy and
    // snow), so each list is checked independently rather than in one match, exactly as the game
    // sums them into separate counts.
    // EvilTileCount (`SceneMetrics.cs:614`): ebonstone, corrupt grass/thorn/ice/sandstone/sand.
    const EVIL_TILES: [u16; 9] = [23, 661, 24, 25, 32, 112, 163, 400, 398];
    // BloodTileCount (`SceneMetrics.cs:615`): crimstone, crimson grass/thorn/ice/sandstone, crimsand, ichor.
    const BLOOD_TILES: [u16; 9] = [199, 662, 201, 203, 200, 401, 399, 234, 352];
    // HolyTileCount (`SceneMetrics.cs:603`): pearlstone, hallow-converted stones, pearlsand, hallowed grass/ice/sandstone.
    const HOLY_TILES: [u16; 9] = [109, 492, 110, 113, 117, 116, 164, 403, 402];
    // JungleTileCount (`SceneMetrics.cs:613`): jungle grass, plants, vines, mud, jungle thorn.
    const JUNGLE_TILES: [u16; 6] = [60, 61, 62, 74, 226, 225];
    // SnowTileCount (`SceneMetrics.cs:604`): snow, snow brick, ice, purple/red ice, slush.
    const SNOW_TILES: [u16; 7] = [147, 148, 161, 162, 164, 163, 200];
    // SandTileCount (`SceneMetrics.cs:620`): sand plus every converted sand/sandstone.
    const SAND_TILES: [u16; 12] = [53, 396, 397, 112, 116, 234, 398, 402, 399, 400, 403, 401];
    // DungeonTileCount (`SceneMetrics.cs:619`): the six dungeon bricks.
    const DUNGEON_TILES: [u16; 6] = [41, 43, 44, 481, 482, 483];

    // Raw tile counts, before the game's evil/hallow reconciliation.
    let (mut evil, mut blood, mut holy, mut jungle, mut snow, mut sand, mut dungeon) =
        (0, 0, 0, 0, 0, 0, 0);
    for dy in -BIOME_SCAN_Y_UP..=BIOME_SCAN_Y_DOWN {
        for dx in -BIOME_SCAN_X..=BIOME_SCAN_X {
            let tile = world.tile(x + dx, y + dy);
            if !tile.is_active() {
                continue;
            }
            let b = tile.block;
            evil += i32::from(EVIL_TILES.contains(&b));
            blood += i32::from(BLOOD_TILES.contains(&b));
            holy += i32::from(HOLY_TILES.contains(&b));
            jungle += i32::from(JUNGLE_TILES.contains(&b));
            snow += i32::from(SNOW_TILES.contains(&b));
            sand += i32::from(SAND_TILES.contains(&b));
            dungeon += i32::from(DUNGEON_TILES.contains(&b));
        }
    }

    // The game reconciles the two evils against the hallow before thresholding, so a tile that reads
    // as both does not count for both (`SceneMetrics.cs:648-664`).
    let holy_before = holy;
    holy -= evil;
    holy -= blood;
    evil -= holy_before;
    blood -= holy_before;
    let (holy, evil, blood) = (holy.max(0), evil.max(0), blood.max(0));

    // The dungeon takes precedence, then the first biome to reach its own threshold in a fixed
    // order (the evils first, as the game's own spawn checks read them first). Snow and desert sit
    // last because their thresholds are the dearest and a corrupted snow reads as corruption in the
    // game too.
    if dungeon >= DUNGEON_THRESHOLD {
        return Biome::Dungeon;
    }
    for (count, threshold, biome) in [
        (evil, EVIL_THRESHOLD, Biome::Corruption),
        (blood, BLOOD_THRESHOLD, Biome::Crimson),
        (holy, HOLY_THRESHOLD, Biome::Hallow),
        (jungle, JUNGLE_THRESHOLD, Biome::Jungle),
        (snow, SNOW_THRESHOLD, Biome::Snow),
        (sand, DESERT_THRESHOLD, Biome::Desert),
    ] {
        if count >= threshold {
            return biome;
        }
    }
    Biome::Forest
}

/// The enemies that can appear at a given place and time, pre-hardmode.
///
/// Every id here was resolved from `NPCID` by name and checked against the stats table, because
/// the numbers are not guessable: Undead Miner is 44 rather than 52, Sand Slime is 537, and Blood
/// Crawler is 239. The coloured slimes (Green, Purple, Jungle and the rest) are *negative* net ids
/// — variants of Blue Slime — so they are not in these pools; see `docs/protocol-notes.md`.
pub fn pool(depth: Depth, biome: Biome, day: bool) -> &'static [u16] {
    use Biome::*;
    use Depth::*;

    match (depth, biome) {
        // The hallow does not exist before the wall falls, so nothing pre-hardmode lives there of
        // its own. It borrows the forest's pool so a hallowed forest is not silently empty of the
        // ordinary things; what is *only* there in hardmode is in `hardmode_pool`.
        (depth, Hallow) => pool(depth, Forest, day),
        (Underworld, _) => &[
            24, // FireImp
            59, // LavaSlime
            60, // Hellbat
            62, // Demon
            66, // VoodooDemon
            39, // BoneSerpentHead
        ],
        (_, Dungeon) => &[
            31, // AngryBones
            32, // DarkCaster
            34, // CursedSkull
            71, // DungeonSlime
        ],
        // The ocean's own roster is *aquatic*, and lives in [`water_pool`]: vanilla reaches it only
        // through `waterTile && isOcean` (`NPC.cs:1798`), so a shark cannot appear on dry sand.
        // A dry beach tile falls through to the ordinary surface pool the same way vanilla's does,
        // which is why standing on the shore at night still brings zombies.
        (depth, Ocean) => pool(depth, Forest, day),
        (Surface, Corruption) => &[
            6,  // EaterofSouls
            7,  // DevourerHead
            47, // CorruptBunny
        ],
        (_, Corruption) => &[
            6,  // EaterofSouls
            7,  // DevourerHead
            81, // CorruptSlime
        ],
        (Surface, Crimson) => &[
            173, // Crimera
            181, // FaceMonster
        ],
        (_, Crimson) => &[
            173, // Crimera
            181, // FaceMonster
            239, // BloodCrawler
        ],
        (Surface, Jungle) => &[
            42, // Hornet
            51, // JungleBat
        ],
        (_, Jungle) => &[
            42, // Hornet
            43, // ManEater
            56, // Snatcher
            51, // JungleBat
        ],
        (Surface, Snow) => {
            if day {
                &[
                    147, // IceSlime
                    185, // SnowFlinx
                ]
            } else {
                &[
                    161, // ZombieEskimo
                    167, // UndeadViking
                    147, // IceSlime
                ]
            }
        }
        (_, Snow) => &[
            147, // IceSlime
            185, // SnowFlinx
            167, // UndeadViking
            150, // IceBat
        ],
        (Surface, Desert) => {
            if day {
                &[
                    61,  // Vulture
                    537, // SandSlime
                ]
            } else {
                &[
                    61,  // Vulture
                    537, // SandSlime
                    3,   // Zombie
                ]
            }
        }
        (_, Desert) => &[
            537, // SandSlime
            69,  // Antlion
            580, // WalkingAntlion
        ],
        (Surface, Forest) => {
            if day {
                // Only the slime is hostile here. The bunny, bird, squirrel and frog this used to
                // list are damage-0 critters the game spawns down its own `spawnFriendly` path
                // (`friendly_pool`), never at the player as monsters (`NPC.cs:2452-2624`).
                &[
                    1, // BlueSlime
                ]
            } else {
                &[
                    3, // Zombie
                    2, // DemonEye
                ]
            }
        }
        (Underground, _) => &[
            1,   // BlueSlime
            16,  // MotherSlime
            10,  // GiantWormHead
            44,  // UndeadMiner
            498, // Salamander
        ],
        (Cavern, _) => &[
            21,  // Skeleton
            49,  // CaveBat
            44,  // UndeadMiner
            16,  // MotherSlime
            10,  // GiantWormHead
            93,  // GiantBat
            498, // Salamander
        ],
    }
}

/// The ordinary draw weight, ten so a rarer type can be a single-digit fraction of it.
const ORDINARY_WEIGHT: u32 = 10;

/// Stands in for "this world's own cavern monsters" in a weighted pick, since that draw is a
/// function call rather than a fixed type. Never a real NPC id.
const CAVERN_SENTINEL: u16 = u16::MAX;

/// The Dungeon Guardian (`NPCID.DungeonGuardian`), the near-unkillable scythe the dungeon throws at
/// anyone who enters before Skeletron is down.
const DUNGEON_GUARDIAN: u16 = 68;

/// How often a hostile type is drawn relative to the others sharing its pool, the game's own
/// per-type spawn rate reduced to one number.
///
/// A flat uniform pick ignored this: the underworld draws a Voodoo Demon roughly one time in
/// seventy (`NPC.cs:4893-4897`: a one-in-seven branch, then a one-in-ten inside it), yet a
/// six-way uniform pick handed one out one time in six, about a dozen times too often. Only the
/// underworld's rates are transcribed here, because it is the one pre-hardmode pool whose cascade
/// is a plain sequence of `rand.Next` rolls rather than a thicket of tile and zone flags this
/// server does not model; every other pool keeps the ordinary weight, an even draw among its
/// members, which is what this did before minus the mis-placed critters. The numbers are the
/// cascade's effective shares (Hellbat is the fallthrough, the lava slime a one-in-three before it,
/// and so on down to the Voodoo Demon).
fn draw_weight(npc_type: u16) -> u32 {
    match npc_type {
        60 => 40, // Hellbat, the underworld fallthrough
        59 => 20, // LavaSlime
        62 => 9,  // Demon
        24 => 5,  // FireImp
        39 => 2,  // BoneSerpentHead (also capped, below)
        66 => 1,  // VoodooDemon, the rare one this fix exists for
        _ => ORDINARY_WEIGHT,
    }
}

/// The most of a type the field will hold at once, where the game caps it, else `None`.
///
/// The only verified pre-hardmode cap among the types these pools name is the Bone Serpent, which
/// the game gates on `!AnyNPCs(39)` so a second never begins while one is alive (`NPC.cs:4885`). A
/// world feeder is a screen-long chain of segments; two at once is a wall of them. Other heavies
/// are left uncapped rather than guessing a limit the game does not clearly set.
fn active_cap(npc_type: u16) -> Option<usize> {
    match npc_type {
        39 => Some(1), // BoneSerpentHead
        _ => None,
    }
}

/// Choose one entry from `candidates`, weighted by [`draw_weight`] and skipping any type already at
/// its [`active_cap`] per the live counts `alive` reports.
///
/// Returns the chosen id, which may be [`CAVERN_SENTINEL`] for a world's own cavern monsters, or
/// `None` when everything on offer is capped out. This is the weighted, cap-aware pick that
/// replaced a flat uniform index: the uniform one handed out a rare Voodoo Demon as often as a
/// common Hellbat and let a second Bone Serpent begin while the first was still on screen.
fn choose_weighted(
    candidates: &[u16],
    alive: &dyn Fn(u16) -> usize,
    rng: &mut SmallRng,
) -> Option<u16> {
    let eligible = |ty: u16| match active_cap(ty) {
        Some(cap) => alive(ty) < cap,
        None => true,
    };
    let total: u32 = candidates
        .iter()
        .copied()
        .filter(|&ty| eligible(ty))
        .map(draw_weight)
        .sum();
    if total == 0 {
        return None;
    }
    let mut roll = rng.random_range(0..total);
    for &ty in candidates {
        if !eligible(ty) {
            continue;
        }
        let w = draw_weight(ty);
        if roll < w {
            return Some(ty);
        }
        roll -= w;
    }
    None
}

/// The friendly critters the game draws instead of a monster when `spawnFriendly` is set.
///
/// This is the deferred friendly-critter table `rates` promised: with a populated base quieting the
/// wild, the game does not simply stop spawning, it spawns harmless critters (`NPC.cs:2099-2624`,
/// the whole `else if (spawnFriendly)` branch). Every id here is a real damage-0 critter, keyed by
/// the biome the player stands in rather than by the exact tile the game reads, which is the
/// disclosed narrowing: the game chooses a bird over a bunny by the grass under the spawn and by
/// weather, season and time this server does not all model, so this returns the ordinary set for
/// the place and lets the caller pick evenly among it. The underworld's lava-bait critters and the
/// gold and gem variants are left out on purpose; they are cosmetic rolls on top of these.
pub fn friendly_pool(depth: Depth, biome: Biome, day: bool) -> &'static [u16] {
    use Biome::*;
    use Depth::*;
    match (depth, biome) {
        // Penguins on snow and ice (`NPC.cs:2328-2337`).
        (_, Snow) => &[
            148, // Penguin
            149, // PenguinBlack
        ],
        // Scorpions on sand (`NPC.cs:2366-2368`).
        (_, Desert) => &[
            366, // ScorpionBlack
            367, // Scorpion
        ],
        // Goldfish and ducks on the water (`NPC.cs:2288-2322`).
        (_, Ocean) => &[
            55,  // Goldfish
            362, // Duck
            364, // DuckWhite
        ],
        // The jungle's tropical birds by day, a frog otherwise (`NPC.cs:2340-2364`).
        (Surface, Jungle) => {
            if day {
                &[
                    361, // Frog
                    671, // ScarletMacaw
                    672, // BlueMacaw
                    673, // Toucan
                    674, // YellowCockatiel
                    675, // GrayCockatiel
                ]
            } else {
                &[
                    361, // Frog
                ]
            }
        }
        (_, Jungle) => &[
            361, // Frog
        ],
        // The ordinary surface: birds, a bunny, a squirrel and butterflies by day; fireflies by
        // night (`NPC.cs:2414`,`2452-2552`). The evils and the hallow borrow it, as their own
        // critter rolls fall back to the same set in the game.
        (Surface, Forest | Corruption | Crimson | Hallow) => {
            if day {
                &[
                    74,  // Bird
                    297, // BirdBlue
                    298, // BirdRed
                    46,  // Bunny
                    299, // Squirrel
                    356, // Butterfly
                ]
            } else {
                &[
                    355, // Firefly
                ]
            }
        }
        // The underworld's friendly spawns are lava-bait critters this server does not model.
        (Underworld, _) => &[],
        // Underground and cavern: the game's own fallback is a bunny, with squirrels near the mouth
        // of a cave (`NPC.cs:2600-2623`).
        (_, _) => &[
            46,  // Bunny
            299, // Squirrel
        ],
    }
}

/// How far down the game looks for ground from a chosen point (`FindGroundTile`).
pub const GROUND_SCAN: i32 = 30;

/// Scan downward for the first solid tile, returning its row.
///
/// The game does this rather than requiring a random point to land exactly on the surface: at any
/// column there is usually one standable row in a 90-tile band, so picking blind would almost
/// never find it.
pub fn find_ground(world: &World, x: i32, from_y: i32) -> Option<i32> {
    find_ground_within(world, x, from_y, from_y + GROUND_SCAN)
}

/// The same descent, stopped at an explicit row rather than a fixed distance.
///
/// `FindSpawnTile` (`NPC.cs:990-993`) walks `for (; j < maxTilesY && j < spawnArea.Bottom && !solid;
/// j++)`, so its budget is "however far it is to the bottom of the spawn box", not a constant. From
/// the top of the box that is a full `2 * SPAWN_RANGE_Y` (94 tiles) rather than [`GROUND_SCAN`]'s
/// 30, which is the difference between reaching an ocean floor and giving up in the water above it.
fn find_ground_within(world: &World, x: i32, from_y: i32, bottom: i32) -> Option<i32> {
    (from_y..bottom.min(world.height())).find(|&y| {
        let tile = world.tile(x, y);
        tile.is_active() && solid(tile.block)
    })
}

/// Whether an NPC can stand at this tile: open space with something solid underneath.
///
/// The open-space half is `NPC.CanSpawnInTile` (`NPC.cs:5431-5442`), which rejects exactly two
/// things: an active solid tile, and lava at *any* depth. Water is explicitly allowed, which is
/// what lets a shark exist.
///
/// This used to be `tile.liquid > 200`, a test blind to which liquid it was looking at, so it had
/// both halves backwards at once: deep water was refused where the game permits it (the whole ocean
/// roster could only appear on a shoreline strip) and shallow lava was accepted where the game
/// refuses it at a single drop.
fn has_room(world: &World, x: i32, y: i32) -> bool {
    for dy in 0..3 {
        let tile = world.tile(x, y - dy);
        if tile.is_active() && solid(tile.block) {
            return false;
        }
        // `Tile.anyLava()`: the kind, not the depth. `liquid_kind` is only meaningful when there is
        // some liquid there, hence the amount test first.
        if tile.liquid > 0 && tile.liquid_kind == terrustia_proto::tile::Liquid::Lava {
            return false;
        }
    }
    let floor = world.tile(x, y + 1);
    floor.is_active() && solid(floor.block)
}

/// Whether a spawn point stands in water deep enough to draw the aquatic roster.
///
/// `NPC.SetSpawnFlagsForChosenTile` (`NPC.cs:1058`): `waterTile = tile[x, y-1].liquid > 0 &&
/// tile[x, y-2].liquid > 0 && tile[x, y-1].liquidType() == 0`, where its `y` is the solid ground
/// row. Ours is one above that (the row the NPC's feet occupy), so the two tiles to test are `y`
/// and `y - 1`. Both must be wet, so a single puddle underfoot is not the sea.
fn water_tile(world: &World, x: i32, y: i32) -> bool {
    let feet = world.tile(x, y);
    feet.liquid > 0
        && world.tile(x, y - 1).liquid > 0
        && feet.liquid_kind == terrustia_proto::tile::Liquid::Water
}

/// What a tile of water draws instead of the land pool.
///
/// Vanilla keeps the water rosters in their own `waterTile` branches ahead of every land branch
/// (`NPC.cs:1766-2000`), which is why a shark is never on the sand and a zombie is never in the sea.
/// Two of those branches are transcribed here, being the two whose types this server already
/// fields:
///
/// * the ocean, `waterTile && isOcean` (`NPC.cs:1798-1920`): Shark, Squid, Crab, Pink Jellyfish;
/// * water below the surface line, `waterTile && spawnTileY > worldSurface` (`NPC.cs:1988-1997`):
///   Blue Jellyfish.
///
/// Deliberately not transcribed, because each would need an NPC this server has no AI for: the
/// hardmode jungle's Arapaima (157), the hardmode crimson's water pair, the Piranha/Angler Fish
/// branch at `NPC.cs:1932`, the corrupt and crimson goldfish at `:1999`, and the ocean's own Sea
/// Snail (220) and Orca (692). An empty slice here means "no water roster for this place", and the
/// caller falls back to the land pool rather than inventing one.
pub fn water_pool(depth: Depth, biome: Biome) -> &'static [u16] {
    match (depth, biome) {
        (_, Biome::Ocean) => &[
            65,  // Shark
            221, // Squid
            67,  // Crab
            64,  // PinkJellyfish
        ],
        // `spawnTileY > Main.worldSurface`: anything below the surface line, which is every band
        // this enum has except the surface itself.
        (Depth::Surface, _) => &[],
        (_, _) => &[
            63, // BlueJellyfish
        ],
    }
}

/// Pick spawns for this tick.
///
/// Returns the types and pixel positions to create; the caller owns the NPC table, so this stays a
/// pure decision and is straightforward to test.
/// What the events running right now do to the spawn pool.
///
/// A moon or an eclipse does not add to the ordinary pool — it replaces it on the surface, which
/// is why standing outside during one is a different game and standing in a cave is not.
pub struct EventSpawns<'a> {
    /// Which moon is up, and which wave it is on.
    pub moon: Option<(crate::game::moons::Moon, i32)>,
    /// Whether a solar eclipse is happening.
    pub eclipse: bool,
    pub downed_plantera: bool,
    pub downed_all_mechs: bool,
    /// Whether the field already holds as many event bosses as it will take.
    pub boss_cap: bool,
    /// Whether the wall has fallen, which is what opens the hardmode half of every pool.
    pub hard_mode: bool,
    /// ...and whether a mechanical boss is down, which is what opens the underworld's.
    pub downed_mech_any: bool,
    /// How many of a type are alive, for the tables that cap their heavies.
    pub census: &'a dyn Fn(u16) -> usize,
    /// The six cavern enemies this particular world has.
    ///
    /// Not part of the pool tables because they are not the same in every world: each world draws
    /// six of the thirteen from its own id. Two worlds therefore feel different underground, and
    /// a player who knows theirs has Salamanders and no Crawdads is right about that permanently.
    pub cavern_monsters: crate::game::cavern_monsters::CavernMonsters,
}

impl EventSpawns<'_> {
    /// Whether anything is running that overrides the surface pool.
    fn running(&self) -> bool {
        self.moon.is_some() || self.eclipse
    }
}

/// How many townsfolk are close enough to a point to quiet it down.
///
/// The game counts them through `SceneMetrics`, which is roughly what is on screen. This uses a
/// radius in the same neighbourhood: far enough that a house nearby counts, close enough that a
/// town on the other side of the world does not make the whole map safe.
fn town_npcs_near(npcs: &NpcStore, at: (f32, f32)) -> u32 {
    /// Tiles, converted to pixels below.
    const REACH: f32 = 100.0 * 16.0;

    npcs.iter()
        .filter(|(_, npc)| npc.stats.town_npc && npc.is_alive())
        .filter(|(_, npc)| {
            (npc.position.0 - at.0).abs() < REACH && (npc.position.1 - at.1).abs() < REACH
        })
        .count() as u32
}

/// Half-extents, in pixels, of the box the cap looks in for the NPCs that already fill it.
///
/// The game counts against a player's `maxSpawns` only the NPCs whose active box overlaps that
/// player (`NPC.cs:78706`: `activeRangeX = sWidth*2.1`, `activeRangeY = sHeight*2.1`, so 1920*2.1
/// and 1200*2.1 pixels, about 252 tiles across and 157 down). The old cap counted every NPC in the
/// whole world, so a monster on the far side of the map held a lone player's own spawns down.
const ACTIVE_RANGE_X: f32 = 1920.0 * 2.1;
const ACTIVE_RANGE_Y: f32 = 1200.0 * 2.1;

/// The spawn weight already near a point: the sum of `npcSlots` over the live, non-town NPCs whose
/// active box overlaps it, which is exactly what the game checks a player's `maxSpawns` against
/// (`NPC.cs:313`, `player.nearbyActiveNPCs`, accumulated in `CheckActive` weighted by each NPC's
/// own `npcSlots`). A statue-spawned monster does not count, as it does not in the game (it carries
/// no spawn slots), which is what lets a statue farm keep working.
pub fn nearby_active_npcs(npcs: &NpcStore, at: (f32, f32)) -> f32 {
    npcs.iter()
        .filter(|(_, npc)| npc.is_alive() && !npc.stats.town_npc && !npc.from_statue)
        .filter(|(_, npc)| {
            (npc.position.0 - at.0).abs() < ACTIVE_RANGE_X
                && (npc.position.1 - at.1).abs() < ACTIVE_RANGE_Y
        })
        .map(|(_, npc)| npc.stats.npc_slots)
        .sum()
}

/// Each player's last biome scan, so the rate can read a zone without paying for one every tick.
///
/// `biome_at` walks a 169-by-124 tile box, measured at 78 us on a full-size world. The rate needs
/// it on every attempt, not only the roughly one in six hundred that places something
/// (`NPC.cs:591-660`), and 78 us per player per tick is 19.8 ms at this server's 255-player bar,
/// which is the whole 16.67 ms tick budget and then some. Vanilla has no equivalent cost: a real
/// dedicated server never scans at all, because the *client* runs `SceneMetrics` and sends its
/// zones up in packet 36.
///
/// So the scan is reused until it is either a second old or the player has walked far enough for it
/// to be stale. Both bounds are conservative against the box being scanned: 16 tiles is a fifth of
/// its half-width, so a cached answer is still one taken from well inside the same neighbourhood.
/// Caching alone was not enough, because it left nothing bounding how many scans land on one tick.
/// Every client in a join burst becomes `is_playing()` on the same tick with an empty entry, so the
/// first fill bought 255 scans at once, and those entries then carried the same `at` and expired
/// together sixty ticks later. A real 255-player soak measured `phase=spawning phase_us=20763`,
/// which is 266 scans: every player, in one tick, over the whole frame budget.
///
/// The average was never the problem, and reporting it is what hid this: 255 players over a
/// 60-tick refresh is 4.25 scans a tick, and the example that measured 345 us drove *one* slot for
/// 60,000 ticks. A per-tick mean over one player cannot see a per-tick maximum over 255.
///
/// So [`Self::BUDGET`] bounds the scans a single tick may buy, and stale answers are served past
/// it. **That budget is also the stagger.** Serving 8 of 255 expiries spreads their next `at` over
/// 32 ticks by construction, and re-spreads them after anything that re-synchronises the group (a
/// mass rejoin, a restart, everyone teleporting to a boss). A phase-based stagger would bound the
/// tick too, but it makes a drifted player wait up to 59 ticks for a fresh answer, which silently
/// guts the `DRIFT` guarantee; and an age-based one does not bound anything, because drift resets
/// the phase and the slots reconverge.
#[derive(Debug)]
pub struct BiomeCache {
    /// What tick it is, set by [`Self::advance`] before each spawn pass.
    now: u64,
    /// Scans left to spend this tick, reset by [`Self::advance`].
    left: u32,
    /// Indexed by player slot: the tick it was taken, where, and what it said.
    entries: Vec<Option<(u64, i32, i32, Biome)>>,
}

/// Hand-written rather than derived so a fresh cache starts with a full budget. A derived `Default`
/// gives `left: 0`, which would make a cache that has not been advanced yet refuse every scan and
/// answer `None` forever.
impl Default for BiomeCache {
    fn default() -> Self {
        Self {
            now: 0,
            left: Self::BUDGET,
            entries: Vec::new(),
        }
    }
}

impl BiomeCache {
    /// Ticks a scan stays good for when the player has not moved far.
    const REFRESH: u64 = 60;
    /// ...and how far they may move before it is taken again anyway, in tiles.
    const DRIFT: i32 = 16;
    /// Scans a single tick may buy, however many players want one.
    ///
    /// Demand at the 255-player bar is `255 / REFRESH` = 4.25 a tick, so 8 is about 1.9x demand and
    /// never builds a backlog, while `8 * 78 us` = 624 us is 3.7% of the frame. The bound does not
    /// grow with the player count, because this is a constant and the scanned box is a fixed
    /// 169x124 tiles.
    ///
    /// What it costs: when every slot expires at once, the oldest entry reaches
    /// `REFRESH + ceil(255 / BUDGET)` = 92 ticks, or 1.53 s, before it is refreshed. For a biome
    /// that moves at Clentaminator speed under a standing player, that is nothing.
    ///
    /// ponytail: the budget goes to the lowest slots that want it, since `try_spawn` walks players
    /// in slot order. Under ordinary play it rotates on its own, because a slot that just scanned is
    /// fresh next tick. Eight clients crossing `DRIFT` every single tick in slots 0-7 could hold it
    /// and leave the rest on stale answers, but that needs 960 tiles a second, so it is a
    /// hacked-client vector rather than a normal one, and the 624 us bound holds either way. If it
    /// is ever seen, the fix is a rotating start cursor over `entries`.
    const BUDGET: u32 = 8;

    /// Tell the cache what tick it is, and refill the scan budget. Called once, before the spawn
    /// pass, so the budget is per pass and resets even on a tick where nothing reads.
    pub fn advance(&mut self, ticks: u64) {
        self.now = ticks;
        self.left = Self::BUDGET;
    }

    /// The last answer taken for this player, however old, and never a fresh scan.
    ///
    /// For callers on a packet path rather than on the tick. A scan is 78 us and a client decides
    /// how often it sends, so a handler must not be able to ask for one: interleaving a move with
    /// whatever packet it is handling would invalidate the entry as fast as the handler refilled
    /// it, and a hundred such pairs in a tick is 7.8 ms of a 16.67 ms budget bought with two
    /// packets. The spawn pass already refreshes this every tick for every active player, so this
    /// is the same answer [`Self::read`] would give in all but the first tick of a session.
    pub fn last(&self, slot: usize) -> Option<Biome> {
        self.entries
            .get(slot)
            .copied()
            .flatten()
            .map(|(_, _, _, biome)| biome)
    }

    /// This player's zone, scanning only when the last answer has gone stale and this tick still
    /// has budget for it.
    ///
    /// `None` means "no answer available this tick", not "forest": a slot with no entry at all has
    /// no stale answer to fall back on, and handing back a default would put every joining player in
    /// the wrong spawn pool for the half second before its turn comes round. The caller skips that
    /// player for the tick instead, which costs nothing observable at roughly one attempt in 600
    /// placing anything.
    pub fn read(&mut self, world: &World, slot: usize, x: i32, y: i32) -> Option<Biome> {
        if self.entries.len() <= slot {
            self.entries.resize(slot + 1, None);
        }
        if let Some((at, sx, sy, biome)) = self.entries[slot]
            && self.now.saturating_sub(at) < Self::REFRESH
            && (x - sx).abs() <= Self::DRIFT
            && (y - sy).abs() <= Self::DRIFT
        {
            return Some(biome);
        }
        if self.left == 0 {
            // Out of scans this tick. A stale answer is still an answer; nothing is not.
            return self.entries[slot].map(|(_, _, _, biome)| biome);
        }
        self.left -= 1;
        let biome = biome_at(world, x, y);
        self.entries[slot] = Some((self.now, x, y, biome));
        Some(biome)
    }
}

/// One spawn attempt in this many considers a bound townsperson instead of an enemy.
///
/// Deliberately steep. A handful of them exist in a world's whole lifetime and each is a resident
/// you cannot otherwise have, so they want to be a find rather than a fixture.
const BOUND_RARITY: u32 = 120;

/// Whether a bound townsperson may be found at this depth, biome and spot.
///
/// Each gate is the real `NPC.Spawner.SpawnNPC` condition for that bound NPC rather than "anywhere
/// underground": without them the Wizard, Mechanic and Goblin Tinkerer were all findable on day
/// one, skipping the hardmode / Skeletron / goblin-army progression the game puts in front of
/// them. The gates key on world progress the server already tracks, plus the depth and biome of
/// the candidate spot. `spawn_y` is the tile row, used for the Mechanic's exact depth threshold.
///
/// Two deliberate narrowings, both because the server does not model the tile the real gate reads:
/// the Stylist's real gate is a spider-nest wall (wall 62, `NPC.cs:1662-1671`), approximated here
/// as "the caverns"; and the Angler's is the ocean surface/water (`NPC.cs:1778-1928`), so he is
/// gated to the ocean biome and is therefore never *mis*-found in a cave, even though this
/// underground-only bound path never reaches the ocean to place him (a disclosed gap, not a fake).
pub fn bound_gate(bound: u16, world: &World, depth: Depth, biome: Biome, spawn_y: i32) -> bool {
    let p = &world.progress;
    match bound {
        // Goblin Tinkerer: the goblin army beaten, deeper than the rock layer but above the
        // underworld (`NPC.cs:2087`: downedGoblins && deeperThanRockLayer && spawnTileY < maxTilesY-210).
        105 => p.downed_goblins && depth == Depth::Cavern,
        // Wizard: hardmode, same caverns band (`NPC.cs:2091`).
        106 => p.hard_mode && depth == Depth::Cavern,
        // Mechanic: Skeletron beaten, below (worldSurface*4 + rockLayer)/5 (`NPC.cs:2656`).
        123 => {
            let threshold = (f64::from(world.surface) * 4.0 + f64::from(world.rock_layer)) / 5.0;
            p.downed_boss3 && f64::from(spawn_y) > threshold
        }
        // Stylist: the spider nest, approximated as the caverns (`NPC.cs:1662-1671`; see above).
        354 => depth == Depth::Cavern,
        // Angler: the ocean (`NPC.cs:1778-1928`; see above).
        376 => biome == Biome::Ocean,
        // Bartender: the Old One's Army becomes available once the Eater of Worlds / Brain of
        // Cthulhu is down (`NPC.cs:1658`, `DD2Event.ReadyToFindBartender => NPC.downedBoss2`).
        579 => p.downed_boss2,
        // Golfer: the underground desert (`NPC.cs:1682-1697`).
        589 => biome == Biome::Desert && matches!(depth, Depth::Underground | Depth::Cavern),
        _ => false,
    }
}

/// Somebody still tied up somewhere in this world, if any are left to find here.
///
/// Refuses anyone already rescued, anyone already standing about waiting to be talked to (so a
/// world cannot end up with two Mechanics or a corridor full of bound wizards), and anyone whose
/// real progression / biome / depth gate this spot does not satisfy.
fn pick_bound(
    world: &World,
    npcs: &NpcStore,
    depth: Depth,
    biome: Biome,
    spawn_y: i32,
    rng: &mut SmallRng,
) -> Option<u16> {
    let waiting: Vec<u16> = crate::game::rescues::RESCUES
        .iter()
        .map(|r| r.bound)
        .filter(|bound| crate::game::rescues::still_bound(&world.progress, *bound))
        .filter(|bound| bound_gate(*bound, world, depth, biome, spawn_y))
        .filter(|bound| {
            !npcs
                .iter()
                .any(|(_, n)| n.npc_type == *bound && n.is_alive())
        })
        .collect();
    if waiting.is_empty() {
        return None;
    }
    Some(waiting[rng.random_range(0..waiting.len())])
}

pub fn try_spawn(
    world: &World,
    npcs: &NpcStore,
    players: &[Option<Player>],
    events: &EventSpawns<'_>,
    journey: &JourneyPowers,
    biomes: &mut BiomeCache,
    rng: &mut SmallRng,
) -> Vec<(u16, (f32, f32))> {
    let active: Vec<&Player> = players
        .iter()
        .flatten()
        .filter(|p| p.is_playing() && p.life > 0)
        .collect();
    if active.is_empty() {
        return Vec::new();
    }

    // The cap is per-player and near-player, as the game's is: each player is gated on their own
    // `maxSpawns` against the spawn weight already close to them (`NPC.cs:312-313`), inside the loop
    // below. There is no world-global slot total and no flat +30%-per-player multiplier here; a
    // second player raises the world's monster count only because they carry their own near-player
    // budget where they stand, which is what the game does and what a single global cap could not.
    let mut out = Vec::new();
    // `NPC.cs:266`, `numberOfActivePlayers`: read once, before the loop consumes the list.
    let active_players = active.len() as u32;
    for player in active {
        let (px, py) = (
            (player.position.0 / 16.0) as i32,
            (player.position.1 / 16.0) as i32,
        );

        // `CanSpawnEnemiesNear` (`NPC.cs:358-362`): nothing spawns anywhere near a live Moon Lord,
        // `player.isNearNPC(398, MoonLordFightingDistance)` with that distance being 4500 px
        // (`NPC.cs:6036`). The fight is meant to be the Moon Lord and its parts, not the Moon Lord
        // plus whatever the surface would ordinarily have sent.
        const MOON_LORD: u16 = 398;
        const MOON_LORD_FIGHTING_DISTANCE: f32 = 4500.0;
        let player_centre = (
            player.position.0 + crate::game::ai::PLAYER_WIDTH as f32 / 2.0,
            player.position.1 + crate::game::ai::PLAYER_HEIGHT as f32 / 2.0,
        );
        if npcs.iter().any(|(_, n)| {
            n.npc_type == MOON_LORD && n.is_alive() && {
                let (dx, dy) = (
                    n.center().0 - player_centre.0,
                    n.center().1 - player_centre.1,
                );
                dx.hypot(dy) < MOON_LORD_FIGHTING_DISTANCE
            }
        }) {
            continue;
        }

        // Journey mode's `SpawnRate`, gated on the world's own difficulty being literally
        // Journey (`Main.IsJourneyMode` — every one of its five real vanilla call sites checks
        // this before reading the power at all; the power itself has no effect outside a Journey
        // world, even for a player who somehow has it set). Both real effects — a hard "spawns
        // off" at the slider's exact floor, and the ordinary rate scaling otherwise — are checked
        // here; only the *rate*, not the shared cap above, is adjusted per player — this
        // function's own cap is already one number shared across every active player rather than
        // vanilla's fully independent per-player `maxSpawns`, an existing simplification predating
        // this power, not something worth restructuring just to extend one Journey slider into.
        let journey_world = world.game_mode == 3;
        if journey_world && journey.spawns_disabled(player.slot) {
            continue;
        }

        // The biome is the *player's* zone, worked out once from where they stand, not re-read at
        // each candidate tile. The game classifies the zone on the player (`SceneMetrics` scans
        // around the player's centre and `SetSpawnFlags` copies `player.Zone*` straight across,
        // `NPC.cs:382-397`); reading it at the far edge of the spawn box instead let a player in
        // the middle of a biome draw the wrong pool whenever a candidate happened to land just
        // outside it.
        //
        // It is read here, before the rate roll rather than after, because `GetSpawnRate` itself
        // reads it: a whole block of rate and cap modifiers keys on the zone (`NPC.cs:591-660`).
        // That is why it now goes through [`BiomeCache`] rather than scanning outright: paying for
        // the scan on every attempt rather than only a successful one is 78 us per player per tick,
        // which does not fit in a tick at this server's player bar.
        // `None` means this tick had no scan budget left and this player has no earlier answer to
        // fall back on, which happens only in the first ticks after a join burst. Skip them rather
        // than guess a zone: every rate and cap modifier below keys on it, so a wrong guess puts
        // them in the wrong spawn pool, and at roughly one attempt in 600 placing anything, a
        // player missing a few attempts is not observable.
        let Some(player_biome) = biomes.read(world, usize::from(player.slot), px, py) else {
            continue;
        };
        let near = nearby_active_npcs(npcs, player.position);

        // The rate and cap are the player's own, not one number for the world: two people in the
        // same world can be standing in a quiet forest and a busy cavern at the same moment.
        let conditions = Conditions {
            depth: rate_depth_at(world, py),
            biome: player_biome,
            hard_mode: world.progress.hard_mode,
            day_time: world.day_time,
            blood_moon: world.blood_moon,
            eclipse: world.eclipse,
            // `NPC.cs:543` and `:772` both carry the height half of this condition.
            event_moon: world.pumpkin_moon || world.snow_moon,
            // `NPC.cs:543` and `:772`, `player.position.Y < Main.worldSurface * 16.0`.
            above_surface_line: py < i32::from(world.surface),
            town_npcs: town_npcs_near(npcs, player.position),
            nearby_active_npcs: near,
            // `NPC.cs:686`, `player.position.Y / 16 > (worldSurface + rockLayer) / 2`.
            below_dirt_midline: py > (i32::from(world.surface) + i32::from(world.rock_layer)) / 2,
            downed_boss3: world.progress.downed_boss3,
            // `NPC.cs:411`, read at the player's own tile.
            behind_a_house_wall: terrustia_proto::housing::wall_encloses(world.tile(px, py).wall),
            active_players,
        };
        let (mut rate, band, spawn_friendly) = rates(conditions, rng);
        let no_worms = no_worms(conditions);
        // This player's own near-player cap, checked before the rate roll, exactly as the game does
        // (`NPC.cs:312-317`: `nearbyActiveNPCs >= maxSpawns` first, then `rand.Next(spawnRate)`).
        if near >= band {
            continue;
        }
        if journey_world {
            let multiplier = journey.spawn_rate_multiplier(player.slot);
            rate = ((rate as f32) / multiplier).max(1.0) as u32;
        }
        if rng.random_range(0..rate.max(1)) != 0 {
            continue;
        }
        // `spawnFriendly` (`NPC.cs:795-924`, see `rates`'s own doc): when a populated base has
        // quieted the wild, this attempt draws a harmless critter instead of a monster rather than
        // being thrown away. It is carried down into the candidate loop below, where the same
        // ground and safe-zone checks apply, and resolved against `friendly_pool`'s critter table.

        // Try a handful of candidate tiles rather than scanning the whole area.
        for _ in 0..20 {
            let x = px + rng.random_range(-SPAWN_RANGE_X..=SPAWN_RANGE_X);
            let from_y = py + rng.random_range(-SPAWN_RANGE_Y..=SPAWN_RANGE_Y);
            if x < 10 || from_y < 10 || x >= world.width() - 10 || from_y >= world.height() - 40 {
                continue;
            }

            // A house wall is the reason a walled base is safe, and it is tested on the *chosen*
            // tile before the descent to ground, not on where the descent lands (`NPC.cs:977`):
            //
            // ```csharp
            // if ((Main.tile[num, j].nactive() && Main.tileSolid[Main.tile[num, j].type])
            //     || (!ignoreSafeWalls && Main.wallHouse[Main.tile[num, j].wall])) continue;
            // ```
            //
            // `wall_encloses` is `Main.wallHouse` exactly (all 279 ids, `Main.cs:9880-10745`), which
            // is why housing and spawn suppression agree about what a wall is: the same set decides
            // both, in the game and here. `ignoreSafeWalls` is set only inside a lunar pillar's zone
            // (`NPC.cs:404-409`), an event this server does not field, so it is left out rather than
            // threaded through as a constant `false`.
            //
            // Without this test, a fully walled and fully lit base spawned zombies inside itself.
            let chosen = world.tile(x, from_y);
            if (chosen.is_active() && solid(chosen.block))
                || terrustia_proto::housing::wall_encloses(chosen.wall)
            {
                continue;
            }

            // Drop to whatever ground is under the chosen point, then stand on top of it. The
            // descent stops at the bottom of the spawn box, as `NPC.cs:990-993` does, rather than a
            // fixed 30 tiles: from the top of the box that is up to 94 tiles, which is the reach an
            // ocean needs.
            let Some(ground) = find_ground_within(world, x, from_y, py + SPAWN_RANGE_Y) else {
                continue;
            };
            let y = ground - 1;

            // Never spawn on top of somebody.
            if (x - px).abs() < SAFE_RANGE_X && (y - py).abs() < SAFE_RANGE_Y {
                continue;
            }
            if !has_room(world, x, y) {
                continue;
            }

            let depth = depth_at(world, y);
            // An event owns the surface while it runs, and nothing below it.
            let event_type = if events.running() && depth == Depth::Surface {
                match (events.moon, events.eclipse) {
                    (Some((moon, wave)), _) if !world.day_time => crate::game::moons::moon_spawn(
                        moon,
                        wave,
                        events.census,
                        events.boss_cap,
                        rng,
                    ),
                    (_, true) if world.day_time => Some(crate::game::moons::eclipse_spawn(
                        events.downed_plantera,
                        events.downed_all_mechs,
                        events.census,
                        rng,
                    )),
                    _ => None,
                }
            } else {
                None
            };
            // Somebody tied up, once in a long while, deep enough down to be worth finding.
            //
            // Rare and unique on purpose: these are the *only* way their residents ever arrive, so
            // one of them failing to appear is a whole townsperson missing — the Mechanic, and with
            // her every piece of wire in the game. Each one is gated on its real vanilla condition
            // (`bound_gate`), so the Wizard, Mechanic and Goblin Tinkerer are no longer findable
            // day one and the Golfer wants the underground desert.
            // A bound resident is a monster-path find, never a friendly attempt's.
            if !spawn_friendly
                && matches!(depth, Depth::Underground | Depth::Cavern)
                && rng.random_range(0..BOUND_RARITY) == 0
                && let Some(bound) = pick_bound(world, npcs, depth, player_biome, y, rng)
            {
                out.push((bound, (x as f32 * 16.0, y as f32 * 16.0)));
                break;
            }

            let npc_type = match event_type {
                Some(npc_type) => npc_type,
                // A friendly attempt draws a harmless critter for this place; if there is no
                // critter for it (the underworld), the attempt is dropped rather than turned into a
                // monster the game would not have spawned here.
                None if spawn_friendly => {
                    let critters = friendly_pool(depth, player_biome, world.day_time);
                    if critters.is_empty() {
                        continue;
                    }
                    critters[rng.random_range(0..critters.len())]
                }
                // Below the dungeon before Skeletron falls, the dungeon answers with the Dungeon
                // Guardian instead of its ordinary residents (`NPC.cs:2646-2654`: `!downedBoss3` in
                // the `ZoneDungeon` branch spawns 68 and returns). This is the wall the game puts up
                // so a fresh character cannot walk in and farm dungeon loot early; without it, Angry
                // Bones and Dark Casters spawned pre-Skeletron.
                None if player_biome == Biome::Dungeon && !world.progress.downed_boss3 => {
                    DUNGEON_GUARDIAN
                }
                // Standing in water draws the aquatic roster instead of the land one, which is how
                // vanilla orders it: every `waterTile` branch (`NPC.cs:1766-2000`) sits ahead of
                // every land branch, so the sea gets sharks and the beach gets zombies.
                None if water_tile(world, x, y) && !water_pool(depth, player_biome).is_empty() => {
                    let wet = water_pool(depth, player_biome);
                    wet[rng.random_range(0..wet.len())]
                }
                None => {
                    let biome = player_biome;
                    let ordinary = pool(depth, biome, world.day_time);
                    // Hardmode adds to what a place had rather than replacing it, so a hardmode
                    // forest still has zombies in it. The underworld's additions wait for a
                    // mechanical boss, which is progression rather than place and so is held here.
                    let extra = if events.hard_mode
                        && (depth != Depth::Underworld || events.downed_mech_any)
                    {
                        hardmode_pool(depth, biome, world.day_time)
                    } else {
                        &[]
                    };
                    // ...and a blood moon widens the night on top of both.
                    let bloody = if world.blood_moon && !world.day_time {
                        blood_moon_pool(depth, events.hard_mode)
                    } else {
                        &[]
                    };
                    // The caverns also draw from the six this world happens to have, which is a
                    // world-specific list rather than a table. It counts as one entry in the
                    // draw, as the game counts it — not six — so a world's own monsters are a
                    // seasoning on the cavern pool rather than most of it. A sentinel stands in for
                    // it in the weighted pick below.
                    let world_specific = depth == Depth::Cavern && biome == Biome::Forest;
                    let mut candidates: Vec<u16> = Vec::with_capacity(
                        ordinary.len() + extra.len() + bloody.len() + usize::from(world_specific),
                    );
                    candidates.extend_from_slice(ordinary);
                    candidates.extend_from_slice(extra);
                    candidates.extend_from_slice(bloody);
                    if world_specific {
                        candidates.push(CAVERN_SENTINEL);
                    }
                    // `noWorms` (`NPC.cs:3704`): a town, or a wall at the player's back, keeps
                    // burrowers out. Dropping them from the draw rather than throwing the whole
                    // attempt away is what the game's `else if` chain amounts to: the branch is
                    // skipped and a later one answers instead.
                    if no_worms {
                        candidates.retain(|ty| !NO_WORMS_GATES.contains(ty));
                    }
                    let alive_count = |ty: u16| {
                        npcs.iter()
                            .filter(|(_, n)| n.npc_type == ty && n.is_alive())
                            .count()
                    };
                    let Some(ty) = choose_weighted(&candidates, &alive_count, rng) else {
                        continue;
                    };
                    if ty == CAVERN_SENTINEL {
                        events.cavern_monsters.pick(rng)
                    } else {
                        ty
                    }
                }
            };

            // Position is the NPC's top-left, so it stands on the tile below.
            out.push((npc_type, (x as f32 * 16.0, y as f32 * 16.0)));
            break;
        }
        // `SpawnNPC` (`NPC.cs:291-306`) walks the player list and `break`s the moment
        // `TrySpawnAnNPC` returns true, so at most one NPC is spawned server-wide per tick however
        // many people are playing. Without this each player got their own draw, so a busy server
        // spawned monsters N times as fast as the game does.
        if !out.is_empty() {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing running: what the ordinary world looks like to `try_spawn`.
    fn quiet() -> EventSpawns<'static> {
        EventSpawns {
            moon: None,
            eclipse: false,
            downed_plantera: false,
            downed_all_mechs: false,
            boss_cap: false,
            hard_mode: false,
            downed_mech_any: false,
            census: &|_| 0,
            cavern_monsters: crate::game::cavern_monsters::CavernMonsters::for_world(7),
        }
    }
    use crate::world::worldgen;
    use rand::SeedableRng;

    fn test_world() -> World {
        worldgen::generate(800, 600, "spawn", 7)
    }

    #[test]
    fn depth_bands_follow_the_world_layers() {
        let mut world = test_world();
        world.surface = 200;
        world.rock_layer = 300;
        assert_eq!(depth_at(&world, 100), Depth::Surface);
        assert_eq!(depth_at(&world, 250), Depth::Underground);
        assert_eq!(depth_at(&world, 350), Depth::Cavern);
        assert_eq!(depth_at(&world, 599 - 1), Depth::Underworld);
    }

    /// The rate bands sit a screen height (75 tiles) below the layers the pool bands use
    /// (`NPC.cs:487`, `:508`, `sHeight => 1200` at `:6793`).
    ///
    /// Fails before the fix, when both questions were answered by `depth_at`: every rate band was
    /// 75 tiles too shallow, roughly doubling the spawn rate through the dirt-layer band.
    #[test]
    fn the_rate_bands_sit_a_screen_height_below_the_pool_bands() {
        let mut world = test_world();
        world.surface = 200;
        world.rock_layer = 300;

        // Just below the surface line is still the surface for rate purposes, and already
        // underground for pool purposes.
        assert_eq!(depth_at(&world, 210), Depth::Underground);
        assert_eq!(rate_depth_at(&world, 210), Depth::Surface);
        assert_eq!(rate_depth_at(&world, 275), Depth::Surface);
        assert_eq!(rate_depth_at(&world, 276), Depth::Underground);

        // The same 75 tiles again at the rock layer.
        assert_eq!(depth_at(&world, 310), Depth::Cavern);
        assert_eq!(rate_depth_at(&world, 310), Depth::Underground);
        assert_eq!(rate_depth_at(&world, 375), Depth::Underground);
        assert_eq!(rate_depth_at(&world, 376), Depth::Cavern);

        // The underworld boundary carries no offset in the game either, so the two agree there.
        let underworld = world.height() - UNDERWORLD_DEPTH;
        assert_eq!(rate_depth_at(&world, underworld), Depth::Underworld);
        assert_eq!(depth_at(&world, underworld), Depth::Underworld);
    }

    /// Every hardmode pool names real, hostile types, and each biome's are its own.
    #[test]
    fn the_hardmode_pools_are_real_and_placed_right() {
        use terrustia_proto::npc_data::npc_stats;
        let mut anywhere = std::collections::HashSet::new();
        for depth in [
            Depth::Surface,
            Depth::Underground,
            Depth::Cavern,
            Depth::Underworld,
        ] {
            for biome in [
                Biome::Forest,
                Biome::Corruption,
                Biome::Crimson,
                Biome::Jungle,
                Biome::Snow,
                Biome::Desert,
                Biome::Ocean,
                Biome::Dungeon,
                Biome::Hallow,
            ] {
                for day in [true, false] {
                    for npc_type in hardmode_pool(depth, biome, day) {
                        let stats = npc_stats(*npc_type)
                            .unwrap_or_else(|| panic!("{npc_type} in {biome:?} is not a type"));
                        assert!(
                            !stats.friendly && !stats.town_npc,
                            "{} is friendly and should not be spawned at anyone",
                            stats.name
                        );
                        anywhere.insert(*npc_type);
                    }
                }
            }
        }
        assert!(
            anywhere.len() > 40,
            "only {} hardmode types",
            anywhere.len()
        );
    }

    /// The hallow is empty before hardmode and full after it.
    #[test]
    fn the_hallow_only_lives_in_hardmode() {
        // Before: it borrows the forest's ordinary life rather than being barren.
        assert_eq!(
            pool(Depth::Surface, Biome::Hallow, true),
            pool(Depth::Surface, Biome::Forest, true),
        );
        // After: it has its own, and nothing it has is the forest's.
        let hallow = hardmode_pool(Depth::Surface, Biome::Hallow, true);
        assert!(!hallow.is_empty());
        let forest = hardmode_pool(Depth::Surface, Biome::Forest, true);
        assert!(
            hallow.iter().all(|t| !forest.contains(t)),
            "the hallow is sharing the forest's hardmode life"
        );
    }

    /// A blood moon widens the night rather than replacing it, and only on the surface.
    #[test]
    fn a_blood_moon_widens_the_night() {
        use terrustia_proto::npc_data::npc_stats;
        let early = blood_moon_pool(Depth::Surface, false);
        let late = blood_moon_pool(Depth::Surface, true);
        assert!(!early.is_empty());
        assert!(late.len() > early.len(), "hardmode adds the Clown");
        assert!(late.contains(&109), "the Clown");
        assert!(!early.contains(&109), "but not before hardmode");
        for npc_type in late {
            assert!(npc_stats(*npc_type).is_some(), "{npc_type} is not a type");
        }
        // Underground is untouched: a blood moon is a thing that happens to the sky.
        for depth in [Depth::Underground, Depth::Cavern, Depth::Underworld] {
            assert!(blood_moon_pool(depth, true).is_empty(), "{depth:?}");
        }
    }

    /// The two evils get different creatures, not the same list twice.
    #[test]
    fn the_evils_are_not_the_same_list() {
        for depth in [Depth::Surface, Depth::Cavern] {
            let corrupt = hardmode_pool(depth, Biome::Corruption, false);
            let crimson = hardmode_pool(depth, Biome::Crimson, false);
            assert!(!corrupt.is_empty() && !crimson.is_empty());
            assert!(
                corrupt.iter().all(|t| !crimson.contains(t)),
                "the two evils share {depth:?} spawns"
            );
        }
    }

    #[test]
    fn every_pool_is_non_empty_and_known() {
        // A pool that names an NPC this build does not define would spawn nothing at all.
        for depth in [
            Depth::Surface,
            Depth::Underground,
            Depth::Cavern,
            Depth::Underworld,
        ] {
            for biome in [
                Biome::Forest,
                Biome::Corruption,
                Biome::Crimson,
                Biome::Jungle,
                Biome::Snow,
                Biome::Desert,
                Biome::Ocean,
                Biome::Dungeon,
            ] {
                for day in [true, false] {
                    let types = pool(depth, biome, day);
                    assert!(!types.is_empty(), "{depth:?}/{biome:?} day={day} is empty");
                    for t in types {
                        assert!(
                            terrustia_proto::npc_data::npc_stats(*t).is_some(),
                            "{depth:?}/{biome:?} names unknown NPC {t}"
                        );
                    }
                }
            }
        }
    }

    /// Every id in a hostile pool is a monster, never a damage-0 critter (`NPC.cs`: critters spawn
    /// down the `spawnFriendly` path, never at the player). Fails before the fix, when the day
    /// forest listed the bunny, bird, squirrel and frog and the ocean listed the goldfish, all
    /// damage 0, so a base under attack could be "attacked" by a bunny.
    #[test]
    fn no_hostile_pool_names_a_critter() {
        use terrustia_proto::npc_data::npc_stats;
        for depth in [
            Depth::Surface,
            Depth::Underground,
            Depth::Cavern,
            Depth::Underworld,
        ] {
            for biome in [
                Biome::Forest,
                Biome::Corruption,
                Biome::Crimson,
                Biome::Jungle,
                Biome::Snow,
                Biome::Desert,
                Biome::Ocean,
                Biome::Dungeon,
                Biome::Hallow,
            ] {
                for day in [true, false] {
                    for t in pool(depth, biome, day) {
                        let stats = npc_stats(*t).expect("a real type");
                        assert!(
                            stats.damage > 0,
                            "{depth:?}/{biome:?} lists the critter {} in its hostile pool",
                            stats.name
                        );
                    }
                }
            }
        }
    }

    /// The friendly-critter table `rates` deferred names only real, harmless critters. Every entry
    /// is a defined type and every one has zero contact damage, so the friendly fork can never hand
    /// back a monster.
    #[test]
    fn spawn_friendly_lists_only_real_critters() {
        use terrustia_proto::npc_data::npc_stats;
        let mut saw_some = false;
        for depth in [
            Depth::Surface,
            Depth::Underground,
            Depth::Cavern,
            Depth::Underworld,
        ] {
            for biome in [
                Biome::Forest,
                Biome::Corruption,
                Biome::Crimson,
                Biome::Jungle,
                Biome::Snow,
                Biome::Desert,
                Biome::Ocean,
                Biome::Dungeon,
                Biome::Hallow,
            ] {
                for day in [true, false] {
                    for t in friendly_pool(depth, biome, day) {
                        saw_some = true;
                        let stats = npc_stats(*t).expect("a real critter type");
                        assert_eq!(
                            stats.damage, 0,
                            "{depth:?}/{biome:?} friendly table names the monster {}",
                            stats.name
                        );
                    }
                }
            }
        }
        assert!(saw_some, "the friendly table is empty everywhere");
    }

    /// The underworld draws its heavies at the game's rates, not a flat uniform share. The Voodoo
    /// Demon is a roughly-one-in-seventy roll in the game (`NPC.cs:4893-4897`); a six-way uniform
    /// pick handed it out one time in six. Sampling the weighted pick, the Voodoo Demon stays under
    /// a twentieth and the Hellbat (the cascade's fallthrough) is the plurality. Fails before the
    /// fix: a uniform pick puts every underworld type at one in six.
    #[test]
    fn the_underworld_draws_a_voodoo_demon_rarely() {
        // The underworld pool, minus the capped Bone Serpent (uncapped here, count always 0).
        let underworld = pool(Depth::Underworld, Biome::Forest, true);
        let none_alive = |_: u16| 0usize;
        let mut rng = SmallRng::seed_from_u64(99);
        let mut voodoo = 0u32;
        let mut hellbat = 0u32;
        const N: u32 = 60_000;
        for _ in 0..N {
            match choose_weighted(underworld, &none_alive, &mut rng) {
                Some(66) => voodoo += 1,
                Some(60) => hellbat += 1,
                _ => {}
            }
        }
        assert!(
            voodoo < N / 20,
            "voodoo demons drawn {voodoo}/{N}, far more than the game's ~1/70",
        );
        assert!(
            hellbat > voodoo * 10,
            "the hellbat should dominate the underworld: {hellbat} vs {voodoo} voodoo",
        );
    }

    /// A type already at its active cap is never drawn (`active_cap`; the game's `!AnyNPCs(39)` on
    /// the Bone Serpent, `NPC.cs:4885`). Fails before the fix, when a uniform pick had no notion of
    /// a cap at all and would start a second serpent while the first was alive.
    #[test]
    fn a_capped_type_is_never_drawn_while_at_its_cap() {
        let underworld = pool(Depth::Underworld, Biome::Forest, true);
        assert!(
            underworld.contains(&39),
            "the underworld has the bone serpent"
        );
        // One bone serpent already alive: it is at its cap of one.
        let serpent_alive = |t: u16| usize::from(t == 39);
        let mut rng = SmallRng::seed_from_u64(7);
        for _ in 0..20_000 {
            let drawn = choose_weighted(underworld, &serpent_alive, &mut rng);
            assert_ne!(drawn, Some(39), "drew a second bone serpent past its cap");
        }
    }

    #[test]
    fn surface_forest_swaps_between_day_and_night() {
        let day = pool(Depth::Surface, Biome::Forest, true);
        let night = pool(Depth::Surface, Biome::Forest, false);
        assert!(day.contains(&1), "slimes by day");
        assert!(night.contains(&3), "zombies by night");
        assert!(!night.contains(&46), "no bunnies at night");
    }

    #[test]
    fn the_ocean_is_decided_by_position_not_tiles() {
        let world = test_world();
        assert_eq!(biome_at(&world, 10, 100), Biome::Ocean);
        assert_eq!(biome_at(&world, world.width() - 10, 100), Biome::Ocean);
    }

    /// Fill a `w`-by-`h` block of one tile type with its top-left `dx`,`dy` from a centre.
    #[allow(clippy::too_many_arguments)]
    fn fill_block(
        world: &mut World,
        cx: i32,
        cy: i32,
        dx: i32,
        dy: i32,
        w: i32,
        h: i32,
        block: u16,
    ) {
        use terrustia_proto::tile::Tile;
        for yy in 0..h {
            for xx in 0..w {
                world.set_tile(cx + dx + xx, cy + dy + yy, Tile::block(block));
            }
        }
    }

    /// Blank the whole 169x124 scan box (with a margin) to plain dirt, so what the scan reads is
    /// only what a test then paints on. The generated test world is a cramped 800 wide, close
    /// enough that its evil band sometimes sits inside the box around the middle; that is a fact
    /// about a tiny world, not the scan, so a test that wants a clean forest makes one.
    fn plain_box(world: &mut World, cx: i32, cy: i32) {
        use terrustia_proto::tile::Tile;
        for dy in -70..=70 {
            for dx in -90..=90 {
                world.set_tile(cx + dx, cy + dy, Tile::block(0)); // dirt
            }
        }
    }

    #[test]
    fn plain_terrain_reads_as_forest() {
        let mut world = test_world();
        let (cx, cy) = (world.width() / 2, i32::from(world.surface) + 30);
        plain_box(&mut world, cx, cy);
        assert_eq!(biome_at(&world, cx, cy), Biome::Forest);
    }

    /// The biome scan reads the game's 169x124 box against the game's per-biome thresholds, not a
    /// 41x41 box against a flat 60 (`SceneMetrics.cs:16`,`24-58`). Fails before the fix on two
    /// counts, each its own assertion below: a 100-tile pocket of corruption used to *be*
    /// corruption (the flat 60 threshold, now 300), and corruption sitting thirty tiles out was
    /// invisible (past the old radius-20 box, inside the new one).
    #[test]
    fn the_biome_scan_uses_the_games_box_and_thresholds() {
        const EBONSTONE: u16 = 23;
        let base = test_world();
        let cx = base.width() / 2;
        let cy = i32::from(base.surface) + 40;

        // A clean forest, then a pocket of 100 corrupt tiles (10x10): over the old flat 60, under
        // the real 300, so this must now read as forest where it used to read as corruption.
        let mut world = test_world();
        plain_box(&mut world, cx, cy);
        assert_eq!(
            biome_at(&world, cx, cy),
            Biome::Forest,
            "baseline is forest"
        );
        fill_block(&mut world, cx, cy, -5, -5, 10, 10, EBONSTONE);
        assert_eq!(
            biome_at(&world, cx, cy),
            Biome::Forest,
            "a 100-tile pocket is under the 300 corruption threshold",
        );

        // A genuine 400-tile corruption (20x20) does read as corruption.
        let mut world = test_world();
        plain_box(&mut world, cx, cy);
        fill_block(&mut world, cx, cy, -10, -10, 20, 20, EBONSTONE);
        assert_eq!(biome_at(&world, cx, cy), Biome::Corruption);

        // 400 corrupt tiles placed entirely thirty-plus tiles to the right are inside the game's
        // box but were outside the old radius-20 one: they must be counted, so this reads as
        // corruption where the old scan saw an empty forest.
        let mut world = test_world();
        plain_box(&mut world, cx, cy);
        fill_block(&mut world, cx, cy, 25, -10, 20, 20, EBONSTONE);
        assert_eq!(
            biome_at(&world, cx, cy),
            Biome::Corruption,
            "the scan must reach past the old radius-20 box",
        );
    }

    #[test]
    fn nothing_spawns_without_players() {
        let world = test_world();
        let npcs = NpcStore::new();
        let mut rng = SmallRng::seed_from_u64(1);
        assert!(
            try_spawn(
                &world,
                &npcs,
                &[],
                &quiet(),
                &JourneyPowers::default(),
                &mut BiomeCache::default(),
                &mut rng,
            )
            .is_empty()
        );
    }

    /// A base with three or more residents stops producing monsters entirely (C1-b item 8):
    /// `NPC.cs:917-921`'s own classic-mode `spawnFriendly = true` on every attempt. What that
    /// attempt draws instead is now a real thing, not a dropped spawn: the game spawns harmless
    /// critters near a populated base, not nothing (`NPC.cs:2099-2624`). So a minute of ticks
    /// produces spawns, and every one of them is a damage-0 critter, never a monster. Fails before
    /// the critter table was wired: `try_spawn` used to skip the friendly attempt outright, so the
    /// count was flatly zero and the "some critters appear" half of this could never hold.
    #[test]
    fn a_populated_base_produces_critters_and_no_monsters() {
        use terrustia_proto::npc_data::npc_stats;
        const GUIDE: u16 = 22;
        let world = test_world();
        let mut npcs = NpcStore::new();
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
        drop(out_rx);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = crate::game::ConnState::Playing;
        // A town cannot quiet an evil (`NPC.cs:800`'s `!flag` clause), and this world's own spawn
        // point happens to sit in one, so find a plain forest column to build the town in instead.
        let py = i32::from(world.spawn_y);
        let px = (260..540)
            .step_by(4)
            .find(|&x| {
                !matches!(
                    biome_at(&world, x, py),
                    Biome::Corruption | Biome::Crimson | Biome::Ocean
                )
            })
            .expect("the test world has somewhere that is not an evil");
        player.position = (px as f32 * 16.0, py as f32 * 16.0);
        // Three townsfolk standing right where the player is, well inside town_npcs_near's reach.
        for _ in 0..3 {
            npcs.spawn(GUIDE, player.position);
        }
        let players = vec![Some(player)];

        let mut rng = SmallRng::seed_from_u64(13);
        let mut spawned = 0;
        for _ in 0..3600 {
            for (npc_type, _) in try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &JourneyPowers::default(),
                &mut BiomeCache::default(),
                &mut rng,
            ) {
                spawned += 1;
                let stats = npc_stats(npc_type).expect("a real type");
                assert_eq!(
                    stats.damage, 0,
                    "a populated base spawned a monster ({}), not a critter",
                    stats.name
                );
            }
        }
        assert!(
            spawned > 0,
            "a populated base should still produce friendly critters, not nothing"
        );
    }

    /// Build a world whose middle is a dungeon: an open pocket around `(cx, cy)` with a dungeon-
    /// brick floor and a deep dungeon-brick fill below it, enough brick in the scan box for
    /// `biome_at` to read Dungeon. Returns the world and the tile centre.
    fn dungeon_world() -> (World, (i32, i32)) {
        use terrustia_proto::tile::Tile;
        const DUNGEON_BRICK: u16 = 41;
        let mut world = test_world();
        let cx = world.width() / 2;
        let cy = i32::from(world.surface) + 70; // underground, clear of the surface
        for yy in (cy - 55)..=(cy + 55) {
            for xx in (cx - 110)..=(cx + 110) {
                // Air at and above the walk level, solid dungeon brick below it.
                let tile = if yy <= cy {
                    Tile::AIR
                } else {
                    Tile::block(DUNGEON_BRICK)
                };
                world.set_tile(xx, yy, tile);
            }
        }
        assert_eq!(
            biome_at(&world, cx, cy),
            Biome::Dungeon,
            "the middle is a dungeon"
        );
        (world, (cx, cy))
    }

    fn dungeon_player(cx: i32, cy: i32) -> Vec<Option<Player>> {
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
        drop(out_rx);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = crate::game::ConnState::Playing;
        player.position = (cx as f32 * 16.0, cy as f32 * 16.0);
        vec![Some(player)]
    }

    /// The dungeon before Skeletron is beaten spawns the Dungeon Guardian (68), not its ordinary
    /// residents (`NPC.cs:2646-2654`). Once Skeletron is down the same dungeon spawns Angry Bones
    /// and the rest and never the Guardian. Fails before the gate: a fresh dungeon spawned ordinary
    /// enemies a new character could farm.
    #[test]
    fn the_dungeon_gates_on_the_guardian_before_skeletron() {
        let (mut world, (cx, cy)) = dungeon_world();
        let npcs = NpcStore::new();
        let players = dungeon_player(cx, cy);

        // Skeletron not yet beaten: every spawn the dungeon offers is the Guardian.
        world.progress.downed_boss3 = false;
        let mut rng = SmallRng::seed_from_u64(3);
        let mut before = 0;
        for _ in 0..40_000 {
            for (npc_type, _) in try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &JourneyPowers::default(),
                &mut BiomeCache::default(),
                &mut rng,
            ) {
                assert_eq!(
                    npc_type, DUNGEON_GUARDIAN,
                    "pre-Skeletron dungeon spawned an ordinary enemy"
                );
                before += 1;
            }
        }
        assert!(
            before > 0,
            "the pre-Skeletron dungeon never spawned anything"
        );

        // Skeletron down: the ordinary dungeon pool returns and the Guardian never does.
        world.progress.downed_boss3 = true;
        let mut rng = SmallRng::seed_from_u64(3);
        let mut after = 0;
        for _ in 0..40_000 {
            for (npc_type, _) in try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &JourneyPowers::default(),
                &mut BiomeCache::default(),
                &mut rng,
            ) {
                assert_ne!(
                    npc_type, DUNGEON_GUARDIAN,
                    "the Guardian should be gone once Skeletron is down"
                );
                // The bound Mechanic's own gate is exactly "the dungeon, after Skeletron"
                // (`NPC.cs:2656`), so she is a correct find here rather than a stray draw from the
                // pool. Rescues are not what this test is about.
                let stats = terrustia_proto::npc_data::npc_stats(npc_type).expect("a real type");
                if stats.friendly {
                    continue;
                }
                assert!(
                    pool(depth_at(&world, cy), Biome::Dungeon, world.day_time).contains(&npc_type),
                    "post-Skeletron dungeon spawned {npc_type}, not a dungeon regular",
                );
                after += 1;
            }
        }
        assert!(
            after > 0,
            "the post-Skeletron dungeon never spawned anything"
        );
    }

    #[test]
    fn spawns_appear_outside_the_safe_zone_and_on_solid_ground() {
        let world = test_world();
        let npcs = NpcStore::new();
        let mut rng = SmallRng::seed_from_u64(9);

        let (tx, ty) = (world.spawn_x as i32, world.spawn_y as i32);
        let (tx_px, ty_px) = (tx as f32 * 16.0, ty as f32 * 16.0);
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
        drop(out_rx);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = crate::game::ConnState::Playing;
        player.position = (tx_px, ty_px);
        let players = vec![Some(player)];

        // Run many ticks so the one-in-600 roll fires repeatedly.
        let mut seen = 0;
        for _ in 0..20_000 {
            for (npc_type, (px, py)) in try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &JourneyPowers::default(),
                &mut BiomeCache::default(),
                &mut rng,
            ) {
                seen += 1;
                assert!(
                    terrustia_proto::npc_data::npc_stats(npc_type).is_some(),
                    "spawned an unknown type {npc_type}"
                );
                let (x, y) = ((px / 16.0) as i32, (py / 16.0) as i32);
                assert!(
                    (x - tx).abs() >= SAFE_RANGE_X || (y - ty).abs() >= SAFE_RANGE_Y,
                    "spawned inside the safe zone at ({x}, {y}) vs player ({tx}, {ty})"
                );
                assert!(has_room(&world, x, y), "spawned somewhere with no room");
            }
            if seen > 20 {
                break;
            }
        }
        assert!(seen > 0, "nothing ever spawned");
    }

    #[test]
    fn ground_is_found_by_scanning_down() {
        let world = test_world();
        let x = world.width() / 2;
        // Start well above the terrain; the scan should land on the first solid row.
        let surface = (0..world.height())
            .find(|y| world.tile(x, *y).is_active())
            .expect("the column has ground");
        assert_eq!(find_ground(&world, x, surface - 10), Some(surface));
        // And starting on the ground finds it immediately.
        assert_eq!(find_ground(&world, x, surface), Some(surface));
    }

    #[test]
    fn scanning_gives_up_rather_than_falling_through_the_world() {
        let world = test_world();
        // Deep sky above the terrain, more than the scan depth.
        assert_eq!(find_ground(&world, world.width() / 2, 0), None);
    }

    /// One playing player standing where the test world's spawn point is, for the whole-loop tests
    /// below.
    fn player_at(position: (f32, f32)) -> Vec<Option<Player>> {
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
        drop(out_rx);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = crate::game::ConnState::Playing;
        player.position = position;
        vec![Some(player)]
    }

    /// A walled base suppresses spawns inside itself (`NPC.cs:977`).
    ///
    /// Fails before the fix: the candidate loop had no wall test at all, so a fully walled, fully
    /// lit base spawned zombies in its own living room. Built as a flat hall with stone walls behind
    /// every open tile and a stone floor, wide enough that the whole spawn box is inside it.
    #[test]
    fn a_walled_base_suppresses_spawns_inside_itself() {
        let mut world = World::empty(800, 600, "walled");
        let floor = 300;
        // A wide, tall hall: solid floor, and every open tile above it backed by a stone wall.
        for x in 0..world.width() {
            world.set_tile(x, floor, terrustia_proto::Tile::block(1));
            for y in (floor - 120)..floor {
                let mut walled = terrustia_proto::Tile::AIR;
                walled.wall = 4; // stone wall, one of `Main.wallHouse`
                world.set_tile(x, y, walled);
            }
        }
        world.surface = 100;
        world.rock_layer = 200;

        let npcs = NpcStore::new();
        let players = player_at(((world.width() / 2) as f32 * 16.0, (floor - 1) as f32 * 16.0));
        let mut rng = SmallRng::seed_from_u64(11);
        let mut seen = 0;
        for _ in 0..60_000 {
            seen += try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &JourneyPowers::default(),
                &mut BiomeCache::default(),
                &mut rng,
            )
            .len();
        }
        assert_eq!(seen, 0, "{seen} spawns inside a fully walled base");

        // The same hall with the walls stripped out is not safe, so the test is measuring the wall
        // and not some other reason nothing could spawn there.
        for x in 0..world.width() {
            for y in (floor - 120)..floor {
                world.set_tile(x, y, terrustia_proto::Tile::AIR);
            }
        }
        let mut open = 0;
        for _ in 0..60_000 {
            open += try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &JourneyPowers::default(),
                &mut BiomeCache::default(),
                &mut rng,
            )
            .len();
        }
        assert!(open > 0, "an unwalled hall of the same shape should spawn");
    }

    /// `SpawnNPC` (`NPC.cs:291-306`) stops at the first player who spawns something, so a tick
    /// produces at most one NPC however many people are playing.
    ///
    /// Fails before the fix, which gave every player their own draw: a busy server spawned
    /// monsters N times as fast as the game does. Driven hard with journey mode's slider at the top
    /// so the rate is fast enough for two players to collide on the same tick often.
    #[test]
    fn a_tick_spawns_at_most_one_npc_however_many_players_there_are() {
        let mut world = test_world();
        world.game_mode = 3;
        let npcs = NpcStore::new();
        let (tx, ty) = (world.spawn_x as i32, world.spawn_y as i32);

        let mut players = Vec::new();
        for (slot, offset) in [(0u8, 0), (1, 400)] {
            let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
            drop(out_rx);
            let mut player = Player::new(slot, "127.0.0.1:1".parse().unwrap(), out_tx);
            player.state = crate::game::ConnState::Playing;
            player.position = ((tx + offset) as f32 * 16.0, ty as f32 * 16.0);
            players.push(Some(player));
        }

        let mut boosted = JourneyPowers::default();
        boosted.set_spawn_rate_slider(0, 1.0);
        boosted.set_spawn_rate_slider(1, 1.0);

        let mut rng = SmallRng::seed_from_u64(31);
        let mut cache = BiomeCache::default();
        let mut total = 0;
        for tick in 0..40_000u64 {
            cache.advance(tick);
            let batch = try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &boosted,
                &mut cache,
                &mut rng,
            );
            assert!(
                batch.len() <= 1,
                "a tick spawned {} NPCs: {batch:?}",
                batch.len()
            );
            total += batch.len();
        }
        assert!(total > 100, "the run has to actually spawn things: {total}");
    }

    /// The biome cache answers with the scan it would have run, and takes a fresh one once it is a
    /// second old or the player has walked out of its neighbourhood.
    ///
    /// It exists because `biome_at` is 78 us on a full-size world and the rate needs it on every
    /// attempt: uncached, that is 19.8 ms per tick at 255 players, over the whole tick budget.
    #[test]
    fn the_biome_cache_agrees_with_a_fresh_scan_and_refreshes_on_time() {
        let mut world = test_world();
        let (x, y) = (world.width() / 2, i32::from(world.surface) + 10);
        let mut cache = BiomeCache::default();

        let fresh = biome_at(&world, x, y);
        assert_eq!(
            cache.read(&world, 0, x, y),
            Some(fresh),
            "a first read scans"
        );

        // Walking beyond the drift bound takes a new scan: the outer columns read as ocean by
        // position, which is a different answer from the forest just cached.
        assert_ne!(fresh, Biome::Ocean);
        cache.advance(30);
        assert_eq!(cache.read(&world, 0, x + 8, y), Some(fresh), "still fresh");
        assert_eq!(
            cache.read(&world, 0, 10, y),
            Some(Biome::Ocean),
            "and drifted"
        );
        // A slot of its own is not the same slot.
        assert_eq!(cache.read(&world, 1, x, y), Some(fresh));

        // Age alone is only observable when the world underneath changes, so paint enough
        // ebonstone into the scan box to cross `EVIL_THRESHOLD` and watch the answer follow.
        let mut aged = BiomeCache::default();
        aged.advance(100);
        assert_eq!(aged.read(&world, 0, x, y), Some(fresh));
        for dx in -20..20 {
            for dy in -20..20 {
                world.set_tile(x + dx, y + dy, terrustia_proto::Tile::block(23));
            }
        }
        assert_eq!(biome_at(&world, x, y), Biome::Corruption, "a real evil now");
        aged.advance(100 + BiomeCache::REFRESH - 1);
        assert_eq!(
            aged.read(&world, 0, x, y),
            Some(fresh),
            "a scan under a second old is still used",
        );
        aged.advance(100 + BiomeCache::REFRESH);
        assert_eq!(
            aged.read(&world, 0, x, y),
            Some(Biome::Corruption),
            "and a second later it is taken again",
        );
    }

    /// A join burst cannot buy a scan for every player on one tick.
    ///
    /// No vanilla line to cite: a real dedicated server never scans at all, because the client runs
    /// `SceneMetrics` and sends its zones up in packet 36. The citation is the measurement. A scan
    /// is 78 us (`examples/biome_scan_cost.rs`), so 255 of them is 19,890 us against a 16,667 us
    /// tick, and a real 255-player soak measured `phase=spawning phase_us=20763` and failed the
    /// release gate on it.
    ///
    /// Fails before the budget: every one of the 255 reads scanned, because nothing bounded them.
    #[test]
    fn a_join_burst_cannot_buy_a_scan_for_every_player_in_one_tick() {
        let world = test_world();
        let (x, y) = (world.width() / 2, i32::from(world.surface) + 10);
        let mut cache = BiomeCache::default();

        // Every slot arrives with no entry at all, which is the first fill: 255 clients becoming
        // playable on the same tick.
        cache.advance(1);
        for slot in 0..255 {
            let _ = cache.read(&world, slot, x, y);
        }
        let scanned = (0..255).filter(|s| cache.last(*s).is_some()).count();
        assert!(
            scanned <= BiomeCache::BUDGET as usize,
            "{scanned} scans on one tick, budget is {}",
            BiomeCache::BUDGET,
        );

        // Everyone is served inside the refresh window rather than starved.
        for tick in 2..=64 {
            cache.advance(tick);
            for slot in 0..255 {
                let _ = cache.read(&world, slot, x, y);
            }
        }
        assert!(
            (0..255).all(|s| cache.last(s).is_some()),
            "every slot should have an answer within the refresh window",
        );

        // And the stale path is budgeted too, not only the empty one. Moving every player past
        // DRIFT on the same tick is synchronised expiry in its instant form, and the outer columns
        // read as ocean, so a rescan is visible as a changed answer.
        cache.advance(65);
        for slot in 0..255 {
            let _ = cache.read(&world, slot, 10, y);
        }
        let moved = (0..255)
            .filter(|s| cache.last(*s) == Some(Biome::Ocean))
            .count();
        assert!(
            moved <= BiomeCache::BUDGET as usize,
            "{moved} rescans on one tick, budget is {}",
            BiomeCache::BUDGET,
        );
    }

    /// Nothing spawns while a Moon Lord is on the field near you (`NPC.cs:358-362`,
    /// `MoonLordFightingDistance = 4500` at `NPC.cs:6036`).
    ///
    /// Fails before the fix, which had no suppression at all: the fight came with whatever the
    /// surface would ordinarily have sent, on top of the Moon Lord and its parts.
    #[test]
    fn a_moon_lord_on_the_field_stops_everything_else_spawning() {
        const MOON_LORD: u16 = 398;
        let world = test_world();
        let (tx, ty) = (world.spawn_x as i32, world.spawn_y as i32);
        let players = player_at((tx as f32 * 16.0, ty as f32 * 16.0));

        let count = |npcs: &NpcStore, seed: u64| {
            let mut rng = SmallRng::seed_from_u64(seed);
            let mut seen = 0;
            for _ in 0..20_000 {
                seen += try_spawn(
                    &world,
                    npcs,
                    &players,
                    &quiet(),
                    &JourneyPowers::default(),
                    &mut BiomeCache::default(),
                    &mut rng,
                )
                .len();
            }
            seen
        };

        assert!(count(&NpcStore::new(), 4) > 0, "the world spawns normally");

        // A Moon Lord standing on the player.
        let mut npcs = NpcStore::new();
        npcs.spawn(MOON_LORD, (tx as f32 * 16.0, ty as f32 * 16.0))
            .expect("a slot");
        assert_eq!(count(&npcs, 4), 0, "not while the Moon Lord is here");

        // ...and one 500 tiles away is well past the 4500 px reach, so it suppresses nothing.
        let mut far = NpcStore::new();
        far.spawn(MOON_LORD, ((tx + 500) as f32 * 16.0, ty as f32 * 16.0))
            .expect("a slot");
        assert!(count(&far, 4) > 0, "a distant Moon Lord is not this fight");
    }

    /// End to end: a wall at the player's back keeps Devourers out of the draw while the rest of
    /// the corruption still spawns (`NPC.cs:411`, `:3704`).
    ///
    /// Fails before the fix, when `noWorms` was not modelled: a walled base in the corruption still
    /// had Devourers coming through the floor.
    #[test]
    fn a_wall_at_your_back_keeps_devourers_out_of_a_corrupt_pool() {
        const DEVOURER: u16 = 7;
        let mut world = World::empty(800, 600, "corrupt");
        world.surface = 100;
        world.rock_layer = 200;
        let floor = 90;
        // A wide band of ebonstone, well past `EVIL_THRESHOLD`, with a floor to stand on.
        for x in 250..550 {
            for y in floor..floor + 30 {
                world.set_tile(x, y, terrustia_proto::Tile::block(23));
            }
        }
        let (px, py) = (400, floor - 1);
        let npcs = NpcStore::new();
        let players = player_at((px as f32 * 16.0, py as f32 * 16.0));

        let run = |world: &World, seed: u64| {
            let mut rng = SmallRng::seed_from_u64(seed);
            let mut seen = std::collections::HashSet::new();
            for _ in 0..40_000 {
                for (npc_type, _) in try_spawn(
                    world,
                    &npcs,
                    &players,
                    &quiet(),
                    &JourneyPowers::default(),
                    &mut BiomeCache::default(),
                    &mut rng,
                ) {
                    seen.insert(npc_type);
                }
            }
            seen
        };

        assert_eq!(biome_at(&world, px, py), Biome::Corruption);
        let wild = run(&world, 12);
        assert!(
            wild.contains(&DEVOURER),
            "an unwalled corruption should send Devourers: {wild:?}",
        );

        // Now put a house wall behind the player, and nothing else.
        let mut walled = world.tile(px, py);
        walled.wall = 4;
        world.set_tile(px, py, walled);

        let sheltered = run(&world, 12);
        assert!(
            !sheltered.contains(&DEVOURER),
            "a wall at your back stops burrowers: {sheltered:?}",
        );
        assert!(
            !sheltered.is_empty(),
            "but not everything else, or this proves nothing",
        );
    }

    /// Lava is refused at any depth and water is not refused at all (`NPC.cs:5431-5442`).
    ///
    /// Fails before the fix, which tested `liquid > 200` without looking at the kind: it rejected
    /// deep water the game permits and accepted shallow lava the game forbids.
    #[test]
    fn lava_is_refused_at_any_depth_and_water_is_not_refused_at_all() {
        use terrustia_proto::tile::Liquid;
        let mut world = World::empty(200, 200, "liquids");
        let floor = 100;
        for x in 90..110 {
            world.set_tile(x, floor, terrustia_proto::Tile::block(1));
        }

        // Dry: room to stand.
        assert!(has_room(&world, 100, floor - 1));

        // Filled to the brim with water: still room, because a shark lives there.
        for dy in 1..=3 {
            world.set_tile(
                100,
                floor - dy,
                terrustia_proto::Tile::AIR.with_liquid(Liquid::Water, 255),
            );
        }
        assert!(
            has_room(&world, 100, floor - 1),
            "deep water is where the ocean roster lives",
        );

        // A single drop of lava, far short of the old 200 threshold, is refused.
        world.set_tile(
            100,
            floor - 1,
            terrustia_proto::Tile::AIR.with_liquid(Liquid::Lava, 1),
        );
        assert!(
            !has_room(&world, 100, floor - 1),
            "`anyLava()` is about the kind, not the depth",
        );
    }

    /// Water draws the aquatic roster and dry land does not (`NPC.cs:1798`, `:1988`).
    ///
    /// Fails before the fix twice over: the ocean roster was in the *land* pool, so sharks appeared
    /// on dry sand, and `has_room` refused the water they should actually have come from.
    #[test]
    fn the_ocean_roster_comes_out_of_water_and_not_off_the_sand() {
        let ocean_water = water_pool(Depth::Surface, Biome::Ocean);
        assert!(
            ocean_water.contains(&65) && ocean_water.contains(&221),
            "the shark and the squid are the ocean's water roster: {ocean_water:?}",
        );
        for &wet in ocean_water {
            assert!(
                !pool(Depth::Surface, Biome::Ocean, true).contains(&wet)
                    && !pool(Depth::Surface, Biome::Ocean, false).contains(&wet),
                "{wet} is aquatic and must not be drawable from dry ocean sand",
            );
        }
        // Below the surface, still water, a different roster.
        assert_eq!(water_pool(Depth::Cavern, Biome::Forest), &[63]);
        assert!(water_pool(Depth::Surface, Biome::Forest).is_empty());

        // And end to end: a player floating in a walled-off sea gets the water roster.
        let mut world = World::empty(800, 600, "sea");
        world.surface = 100;
        world.rock_layer = 200;
        let floor = 150;
        for x in 0..world.width() {
            world.set_tile(x, floor, terrustia_proto::Tile::block(1));
            for y in (floor - 60)..floor {
                world.set_tile(
                    x,
                    y,
                    terrustia_proto::Tile::AIR
                        .with_liquid(terrustia_proto::tile::Liquid::Water, 255),
                );
            }
        }
        // `biome_at` calls the outer 250 columns ocean, so stand there.
        let npcs = NpcStore::new();
        let players = player_at((100.0 * 16.0, (floor - 30) as f32 * 16.0));
        let mut rng = SmallRng::seed_from_u64(5);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..60_000 {
            for (npc_type, _) in try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &JourneyPowers::default(),
                &mut BiomeCache::default(),
                &mut rng,
            ) {
                seen.insert(npc_type);
            }
        }
        assert!(!seen.is_empty(), "nothing spawned in the sea at all");
        for npc_type in &seen {
            // A bound resident is a rescue rather than a spawn from any roster, and vanilla's own
            // ocean branch is where the Angler is found (`NPC.cs:1800`), so they are not the
            // subject here.
            let stats = terrustia_proto::npc_data::npc_stats(*npc_type).expect("a real type");
            if stats.friendly {
                continue;
            }
            assert!(
                ocean_water.contains(npc_type),
                "the sea drew {npc_type} ({}), which is not in its water roster",
                stats.name,
            );
        }
    }

    #[test]
    fn spawning_is_frequent_enough_to_matter() {
        // Picking a blind point and demanding it be the surface almost never works; scanning down
        // is what makes the spawn rate real. A minute of ticks should produce several spawns.
        let world = test_world();
        let npcs = NpcStore::new();
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
        drop(out_rx);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = crate::game::ConnState::Playing;
        player.position = (
            f32::from(world.spawn_x) * 16.0,
            f32::from(world.spawn_y) * 16.0,
        );
        let players = vec![Some(player)];

        let mut rng = SmallRng::seed_from_u64(11);
        let mut spawned = 0;
        for _ in 0..3600 {
            spawned += try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &JourneyPowers::default(),
                &mut BiomeCache::default(),
                &mut rng,
            )
            .len();
        }
        assert!(
            spawned >= 3,
            "only {spawned} spawns in a minute of ticks; the spawn point search is too fussy"
        );
    }

    #[test]
    fn the_cap_stops_further_spawns() {
        let world = test_world();
        let mut npcs = NpcStore::new();
        // Fill well past the single-player cap.
        for _ in 0..40 {
            npcs.spawn(3, (0.0, 0.0));
        }

        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
        drop(out_rx);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = crate::game::ConnState::Playing;
        player.position = (100.0, 100.0);
        let players = vec![Some(player)];

        let mut rng = SmallRng::seed_from_u64(3);
        for _ in 0..5_000 {
            assert!(
                try_spawn(
                    &world,
                    &npcs,
                    &players,
                    &quiet(),
                    &JourneyPowers::default(),
                    &mut BiomeCache::default(),
                    &mut rng,
                )
                .is_empty(),
                "spawned past the cap"
            );
        }
    }

    /// The cap is near-player, not world-global: a crowd of monsters on the far side of the map
    /// does not hold a lone player's own spawns down (`NPC.cs:313`, `player.nearbyActiveNPCs`).
    /// Fails before the fix, when the cap counted every NPC in the world, so the same far-off crowd
    /// silenced spawns everywhere at once.
    #[test]
    fn far_off_monsters_do_not_cap_a_lone_player() {
        let world = test_world();
        let mut npcs = NpcStore::new();

        // A whole screen of players' worth of monsters, parked at the far edge of the world.
        let (sx, sy) = (world.spawn_x as i32, world.spawn_y as i32);
        let far_x = if sx > world.width() / 2 {
            10
        } else {
            world.width() - 10
        };
        assert!(
            ((far_x - sx).abs() as f32 * 16.0) > ACTIVE_RANGE_X,
            "the crowd must be outside the active range to make the point",
        );
        for _ in 0..60 {
            npcs.spawn(3, (far_x as f32 * 16.0, sy as f32 * 16.0));
        }

        let players = {
            let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
            drop(out_rx);
            let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
            player.state = crate::game::ConnState::Playing;
            player.position = (sx as f32 * 16.0, sy as f32 * 16.0);
            vec![Some(player)]
        };

        let mut rng = SmallRng::seed_from_u64(4);
        let mut seen = 0;
        for _ in 0..20_000 {
            seen += try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &JourneyPowers::default(),
                &mut BiomeCache::default(),
                &mut rng,
            )
            .len();
            if seen > 0 {
                break;
            }
        }
        assert!(
            seen > 0,
            "a lone player should still spawn with the only other monsters a map away",
        );
    }

    /// A single test player, `is_playing` and clear of the safe zone, with a real channel behind
    /// it — the same construction `spawns_appear_outside_the_safe_zone_and_on_solid_ground` above
    /// already uses.
    fn one_player(world: &World) -> Vec<Option<Player>> {
        let (tx, ty) = (world.spawn_x as i32, world.spawn_y as i32);
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
        drop(out_rx);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = crate::game::ConnState::Playing;
        player.position = (tx as f32 * 16.0, ty as f32 * 16.0);
        vec![Some(player)]
    }

    /// Journey mode's `SpawnRate` at its exact floor (`0.0`) disables spawns outright for that
    /// player — `GetShouldDisableSpawnsFor`'s own hard condition, not merely the 0.1× remap floor
    /// `spawn_rate_multiplier` alone would give.
    #[test]
    fn spawn_rate_at_the_floor_disables_spawns_in_a_journey_world() {
        let mut world = test_world();
        world.game_mode = 3; // Journey
        let npcs = NpcStore::new();
        let players = one_player(&world);
        let mut journey = JourneyPowers::default();
        journey.set_spawn_rate_slider(0, 0.0);

        let mut rng = SmallRng::seed_from_u64(11);
        let mut seen = 0;
        for _ in 0..20_000 {
            seen += try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &journey,
                &mut BiomeCache::default(),
                &mut rng,
            )
            .len();
        }
        assert_eq!(seen, 0, "spawns should be disabled outright at the floor");
    }

    /// The same slider at its floor has no effect at all outside a Journey world — every one of
    /// `SpawnRateSliderPerPlayerPower`'s five real vanilla call sites gates on `Main.IsJourneyMode`
    /// before reading the power, and this is the one behaviour among the three per-player powers
    /// where getting that gate wrong is easy to miss testing, since an ungated implementation would
    /// otherwise look identical to a correct one on an ordinary difficulty.
    #[test]
    fn spawn_rate_has_no_effect_outside_a_journey_world() {
        let world = test_world(); // game_mode 0: ordinary
        let npcs = NpcStore::new();
        let players = one_player(&world);
        let mut journey = JourneyPowers::default();
        journey.set_spawn_rate_slider(0, 0.0); // would disable spawns entirely, in a Journey world

        let mut rng = SmallRng::seed_from_u64(11);
        let mut seen = 0;
        for _ in 0..20_000 {
            seen += try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &journey,
                &mut BiomeCache::default(),
                &mut rng,
            )
            .len();
        }
        assert!(
            seen > 0,
            "an ordinary-difficulty world should spawn normally regardless of the slider"
        );
    }

    /// Above the floor, the slider scales how often spawns roll — pinned as a real measured ratio
    /// across many ticks, not just "some vs none": a 10× player should see roughly ten times as
    /// many spawn events as a 1× player over the same window.
    #[test]
    fn spawn_rate_at_its_top_spawns_far_more_often_than_the_default_in_a_journey_world() {
        let mut world = test_world();
        world.game_mode = 3;
        let npcs = NpcStore::new();
        let players = one_player(&world);

        let ordinary = JourneyPowers::default(); // 0.5 -> 1x, the default
        let mut boosted = JourneyPowers::default();
        boosted.set_spawn_rate_slider(0, 1.0); // the top of the slider -> 10x

        const TICKS: usize = 200_000;
        let mut ordinary_seen = 0;
        let mut rng = SmallRng::seed_from_u64(21);
        for _ in 0..TICKS {
            ordinary_seen += try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &ordinary,
                &mut BiomeCache::default(),
                &mut rng,
            )
            .len();
        }
        let mut boosted_seen = 0;
        let mut rng = SmallRng::seed_from_u64(21);
        for _ in 0..TICKS {
            boosted_seen += try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &boosted,
                &mut BiomeCache::default(),
                &mut rng,
            )
            .len();
        }
        assert!(
            boosted_seen > ordinary_seen * 5,
            "10x should spawn noticeably more often than 1x over {TICKS} ticks: \
             {boosted_seen} boosted vs {ordinary_seen} ordinary"
        );
    }

    /// The bound townsfolk are gated on their real progression, biome and depth, so the Wizard,
    /// Mechanic and Goblin Tinkerer are not findable on day one (`NPC.cs:2087-2091,2656`). Fails
    /// before the fix, when every still-bound resident was offered the moment a player reached a
    /// cavern, with no hardmode / Skeletron / goblin-army gate at all.
    #[test]
    fn bound_townsfolk_are_gated_on_real_progression() {
        let mut world = test_world();
        world.progress.hard_mode = false;
        world.progress.downed_boss3 = false;
        world.progress.downed_goblins = false;
        let y = i32::from(world.rock_layer) + 5; // squarely in the caverns
        assert_eq!(depth_at(&world, y), Depth::Cavern);

        // Day one, deep in a plain cavern: none of the three progression-gated finds are eligible.
        assert!(
            !bound_gate(106, &world, Depth::Cavern, Biome::Forest, y),
            "the Wizard needs hardmode"
        );
        assert!(
            !bound_gate(123, &world, Depth::Cavern, Biome::Forest, y),
            "the Mechanic needs Skeletron down"
        );
        assert!(
            !bound_gate(105, &world, Depth::Cavern, Biome::Forest, y),
            "the Goblin Tinkerer needs the army beaten"
        );

        // The Stylist has no progression gate (a spider nest is a day-one find), so she is eligible.
        assert!(bound_gate(354, &world, Depth::Cavern, Biome::Forest, y));

        // Hardmode opens the Wizard; Skeletron opens the Mechanic; the beaten army the Goblin.
        world.progress.hard_mode = true;
        world.progress.downed_boss3 = true;
        world.progress.downed_goblins = true;
        assert!(bound_gate(106, &world, Depth::Cavern, Biome::Forest, y));
        assert!(bound_gate(123, &world, Depth::Cavern, Biome::Forest, y));
        assert!(bound_gate(105, &world, Depth::Cavern, Biome::Forest, y));

        // The Golfer wants the underground desert, not a forest cavern.
        assert!(!bound_gate(589, &world, Depth::Cavern, Biome::Forest, y));
        assert!(bound_gate(
            589,
            &world,
            Depth::Underground,
            Biome::Desert,
            y
        ));

        // And a fresh cavern only ever offers the Stylist, never a progression-gated resident.
        let mut rng = SmallRng::seed_from_u64(5);
        world.progress.hard_mode = false;
        world.progress.downed_boss3 = false;
        world.progress.downed_goblins = false;
        for _ in 0..500 {
            if let Some(bound) = pick_bound(
                &world,
                &NpcStore::new(),
                Depth::Cavern,
                Biome::Forest,
                y,
                &mut rng,
            ) {
                assert_eq!(bound, 354, "only the Stylist is a day-one cavern find");
            }
        }
    }
}

/// Whether an NPC type counts against an invasion's remaining size.
///
/// Only the invasion's own members count. A goblin army is not shortened by killing the bats that
/// happened to be in the way, and the game keeps these rosters as flat lists for exactly that
/// reason.
pub fn belongs_to(kind: crate::game::event::Invasion, npc_type: u16) -> bool {
    use crate::game::event::Invasion;
    match kind {
        Invasion::Goblin => matches!(npc_type, 26 | 27 | 28 | 29 | 111 | 471),
        Invasion::FrostLegion => matches!(npc_type, 143..=145),
        Invasion::Pirate => matches!(npc_type, 212 | 213 | 214 | 215 | 216 | 252 | 491),
        Invasion::Martian => matches!(
            npc_type,
            381 | 382 | 383 | 385 | 386 | 388 | 389 | 390 | 395 | 520
        ),
    }
}

#[cfg(test)]
mod invasion_tests {
    use super::belongs_to;
    use crate::game::event::Invasion;

    /// Only an invasion's own members shorten it.
    #[test]
    fn bystanders_do_not_count_against_an_invasion() {
        assert!(belongs_to(Invasion::Goblin, 28), "a goblin peon does");
        assert!(!belongs_to(Invasion::Goblin, 1), "a blue slime does not");
        assert!(
            !belongs_to(Invasion::Goblin, 143),
            "nor does a member of a different invasion"
        );
        assert!(belongs_to(Invasion::Pirate, 491), "the Dutchman counts");
        assert!(belongs_to(Invasion::Martian, 395), "so does the saucer");
    }
}
