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
/// `ZoneSandstorm`, `cloudAlpha`'s snow-in-a-storm bonus, the dual-dungeon seeds, `getGoodWorld`,
/// and the Wall of Flesh's underworld suppression (`NPC.cs:662-666`).
/// Journey mode's slider is real and is applied by the caller, where the power lives, rather than
/// threaded through here.
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
    /// `Player.ZoneGraveyard`: whether the player is standing among enough tombstones.
    ///
    /// `SetSpawnFlags` copies it across with the rest of the zones (`NPC.cs:389`), and it is the one
    /// player-carried zone the server *can* know, because the client computes it and sends it up
    /// (see [`crate::game::player::Player::in_graveyard`]). A graveyard is not a quiet place: it
    /// takes over the surface night roster and it loosens town suppression rather than tightening
    /// it (`NPC.cs:861`, `:884`, `:906`).
    pub graveyard: bool,
    /// Whether the player is standing inside a lunar pillar's zone.
    ///
    /// This is vanilla's `invaders`, not a flag of its own: `SetSpawnFlags` (`NPC.cs:404-409`)
    /// forces `invaders = true` and `ignoreSafeWalls = true` for any of the four `ZoneTower*`,
    /// which is what gives a pillar fight an invasion's rate and cap (`NPC.cs:782-786`) and lets
    /// its escort spawn through a walled-off arena. Named for the zone rather than for `invaders`
    /// because the server's own invasions never reach this function: `tick_spawning` returns into
    /// `spawn_invaders` before it.
    pub in_tower_zone: bool,
    /// `Player.ZoneMeteor`: whether enough meteorite is under the player's feet to count as a
    /// crater ([`Zones::meteor`]).
    ///
    /// It is not a [`Biome`], because a crater takes the biome it fell into with it, and it does two
    /// things here: the last arm of the zone chain that sets the rate (`NPC.cs:636-640`) and one of
    /// the exclusions on town suppression (`NPC.cs:800`). A meteor crater is meant to be dangerous
    /// however many people live beside it.
    pub meteor: bool,
    /// `Player.ZoneLihzhardTemple`: whether the player is standing in front of a Lihzahrd brick wall.
    ///
    /// Not a tile count and not a [`Biome`]: `SceneMetrics.cs:693` is the whole of it,
    /// `ZoneLihzhardTemple = tileSafely.wall == 87`, one wall read at the player's own tile, the
    /// same shape as `ZoneGranite`, `ZoneMarble` and `ZoneHive` beside it. The wall it reads is
    /// already read here for [`Self::behind_a_house_wall`], so the flag costs nothing.
    ///
    /// A temple also reads as [`Biome::Jungle`], because Lihzahrd brick is one of the game's own
    /// jungle zone tiles (`SceneMetrics.cs:613`), so this rate modifier stacks on the jungle's
    /// exactly as vanilla's does: `NPC.cs:641-650` is a separate `if` below the biome chain.
    pub lihzahrd_temple: bool,
    /// `NPC.Spawner.luck`, copied straight off the player this attempt is for
    /// (`SetSpawnFlags`, `NPC.cs:370`).
    ///
    /// The spawner rolls `RollLuck` in seventy-three places, and every one of them was written
    /// here as "a plain `Main.rand.Next(n)` at luck zero", which is what a vanilla player with no
    /// luck effects gets and what every player on this server got, because the server had no luck
    /// figure at all. It has one now (`Player::luck`, from packet 134), and this is how it reaches
    /// the spawner: one field, filled in per player inside [`try_spawn`], exactly as
    /// `SetSpawnFlags` fills in its own.
    pub luck: f32,
}

/// `Luck.RollLuck(luck, range) == 0` — the shape every luck-gated spawn roll in `NPC.Spawner`
/// takes, and the only shape any of them takes.
///
/// A free function beside the pools rather than a method, because the pick routines take a loose
/// `luck` and an `rng` rather than the whole of [`Conditions`]: what a rare-critter roll needs is
/// the number, not the biome it is standing in.
fn rolls_lucky(luck: f32, range: u32, rng: &mut SmallRng) -> bool {
    struct Bridge<'a>(&'a mut SmallRng);
    impl terrustia_proto::luck::LuckRng for Bridge<'_> {
        fn next_f32(&mut self) -> f32 {
            self.0.random::<f32>()
        }
        fn next_max(&mut self, max: i32) -> i32 {
            if max <= 0 {
                return 0;
            }
            self.0.random_range(0..max)
        }
        fn next_range(&mut self, min: i32, max: i32) -> i32 {
            if max <= min {
                return min;
            }
            self.0.random_range(min..max)
        }
    }
    let Ok(range) = i32::try_from(range) else {
        return false;
    };
    terrustia_proto::luck::roll_luck(luck, range, &mut Bridge(rng)) == 0
}

/// `Luck.RollOnlyBadLuckExtreme(luck, range) == 0` (`Terraria.GameContent/Luck.cs:53-60`):
///
/// ```csharp
/// if (luck < 0f && Main.rand.NextFloat() < 0f - luck) { return Main.rand.Next(range / 10); }
/// return -1;
/// ```
///
/// Unlike every other roll in `Luck`, this one has no fallthrough to the ordinary odds: at or
/// above zero luck it returns `-1`, which no caller's `== 0` can ever match. So the arms behind it
/// are genuinely unreachable for an ordinary player, in the game as much as here, and reachable
/// only by being unlucky. The Moss Zombie and the spawn-rate bonus in [`rates`] are the spawner's
/// two callers.
fn rolls_extremely_unlucky_only(luck: f32, range: u32, rng: &mut SmallRng) -> bool {
    if luck >= 0.0 || rng.random::<f32>() >= -luck {
        return false;
    }
    let Ok(range) = i32::try_from(range) else {
        return false;
    };
    // `Main.rand.Next(range / 10)`, and `UnifiedRandom.Next(0)` is 0, which the caller's `== 0`
    // then matches - so a `range` under ten is a certainty once the outer test passes.
    let narrowed = range / 10;
    narrowed <= 0 || rng.random_range(0..narrowed) == 0
}

/// `Luck.RollLuck(luck, range) < numerator` — the same roll, for the handful of callers that
/// compare against something other than zero. `NPC.cs:4378`'s `RollLuck(100) < 40` is the
/// spawner's one, and reading it as `== 0` would have turned a four-in-ten choice into a
/// one-in-a-hundred one.
fn rolls_lucky_under(luck: f32, range: u32, numerator: i32, rng: &mut SmallRng) -> bool {
    struct Bridge<'a>(&'a mut SmallRng);
    impl terrustia_proto::luck::LuckRng for Bridge<'_> {
        fn next_f32(&mut self) -> f32 {
            self.0.random::<f32>()
        }
        fn next_max(&mut self, max: i32) -> i32 {
            if max <= 0 {
                return 0;
            }
            self.0.random_range(0..max)
        }
        fn next_range(&mut self, min: i32, max: i32) -> i32 {
            if max <= min {
                return min;
            }
            self.0.random_range(min..max)
        }
    }
    let Ok(range) = i32::try_from(range) else {
        return false;
    };
    terrustia_proto::luck::roll_luck(luck, range, &mut Bridge(rng)) < numerator
}

/// `Luck.RollOnlyBadLuck(luck, range) == 0` (`Terraria.GameContent/Luck.cs:31-38`): bad luck
/// narrows the range, and good luck does nothing at all. The Groom's and the Bride's own arm is
/// the spawner's one caller (`NPC.cs:4623`, `:4628`).
fn rolls_only_unlucky(luck: f32, range: u32, rng: &mut SmallRng) -> bool {
    rolls_lucky(luck.min(0.0), range, rng)
}

/// `Luck.RollBadLuckExtreme(luck, range) == 0` (`Terraria.GameContent/Luck.cs:40-51`): good luck
/// multiplies the range by *ten*, which is the game's way of saying a lucky player almost never
/// gets this. Only the Statue Mimic's own branch uses it (`NPC.cs:1571`).
fn rolls_extremely_unlucky(luck: f32, range: u32, rng: &mut SmallRng) -> bool {
    let Ok(range) = i32::try_from(range) else {
        return false;
    };
    if luck > 0.0 && rng.random::<f32>() < luck {
        // `Main.rand.Next(range * 10)`.
        return rng.random_range(0..range.saturating_mul(10)) == 0;
    }
    if luck < 0.0 && rng.random::<f32>() < -luck {
        // `Main.rand.Next(Main.rand.Next(range / 2, range))`, the same narrowing `RollLuck`'s good
        // branch takes.
        let narrowed = if range > range / 2 {
            rng.random_range(range / 2..range)
        } else {
            range / 2
        };
        return narrowed <= 0 || rng.random_range(0..narrowed) == 0;
    }
    rng.random_range(0..range.max(1)) == 0
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
    // takes its own modifier and none of the others; the hallow's and the temple's are separate
    // `if`s below it. Three branches of that chain are absent here because this server has no
    // notion of the zone they test: `ZoneSandstorm`, `inDualDungeon` and
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
        // NPC.cs:636-640, the last arm of the same chain and the only one keyed on a zone that is
        // not one of [`Biome`]'s winners, so it answers exactly where none of the arms above did.
        _ if at.meteor => {
            rate *= 0.4;
            max *= 1.1;
        }
        _ => {}
    }
    // NPC.cs:641-650, a separate `if` sitting between the biome chain and the hallow's: the temple
    // is busier than the jungle it is buried in, and it stacks on the jungle's own modifier rather
    // than replacing it, because Lihzahrd brick makes the place read as jungle too. The
    // `Main.remixWorld` half of the branch (`:645-649`) is not modelled anywhere in this server.
    if at.lihzahrd_temple {
        rate *= 0.8;
        max *= 1.2;
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

    // `NPC.cs:782-786`, the invasion override, and the only rate a pillar fight ever runs at: a
    // tower zone sets `invaders` outright (`NPC.cs:404-409`), so the numbers are the moon
    // override's exactly. Without it a pillar's escort would arrive at the surrounding terrain's
    // ordinary rate, which is 30 times slower than the game's and would leave a hundred-kill
    // shield unbreakable in practice as well as in principle.
    if at.in_tower_zone {
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
    // friendly critter instead of a monster (`spawnFriendly`). One thing is deliberately not
    // modelled: the underworld's own separate, simpler fork (`NPC.cs:802-855`) — a base built in
    // the underworld is the rare exception, not the case this quiets.
    //
    // Each branch's `ZoneGraveyard` sub-case *is* modelled now that a graveyard is a thing the
    // server knows about, and it is not a variation on the ordinary case: it throttles the rate by
    // a smaller factor and then rolls *separately* for the friendly fork, where the ordinary case
    // picks one or the other. A base built in a graveyard stays a dangerous place to live, which is
    // the point of building one. Vanilla's own condition is
    // `ZoneGraveyard && (!ZonePeaceCandle || Main.rand.Next(3) == 0)`; `ZonePeaceCandle` is a
    // player-carried effect this server cannot see (this struct's own doc comment), so it is taken
    // as absent, which makes the left-hand disjunct true and leaves the bare graveyard test.
    let mut spawn_friendly = false;
    if town_suppression_applies(at) {
        match at.town_npcs {
            0 => {}
            // NPC.cs:861-882.
            1 => {
                if at.graveyard {
                    // NPC.cs:863-869.
                    rate *= 1.66;
                    if rng.random_ratio(1, 9) {
                        spawn_friendly = true;
                        max *= 0.6;
                    }
                } else if rng.random_ratio(1, 3) {
                    // NPC.cs:870-878, the ordinary case: a one-in-three chance forces a friendly
                    // spawn and shrinks the cap; the other two-in-three simply double the rate.
                    spawn_friendly = true;
                    max *= 0.6;
                } else {
                    rate *= 2.0;
                }
            }
            // NPC.cs:884-900: the odds flip to two-in-three for the friendly fork, and the rate
            // triples on the remaining one-in-three.
            2 => {
                if at.graveyard {
                    // NPC.cs:886-892.
                    rate *= 2.33;
                    if rng.random_ratio(1, 6) {
                        spawn_friendly = true;
                        max *= 0.6;
                    }
                } else if !rng.random_ratio(1, 3) {
                    spawn_friendly = true;
                    max *= 0.6;
                } else {
                    rate *= 3.0;
                }
            }
            // NPC.cs:902-921. `!Main.expertMode` is unconditionally true in classic mode, so the
            // ordinary branch sets `spawnFriendly` on *every* attempt rather than rolling for it —
            // `spawnRate` is never assigned there at all. Expert mode's own further
            // `Main.rand.Next(30) != 0` (a 29-in-30 chance) is folded into the same unconditional
            // case rather than threading a whole difficulty flag through `Conditions` for a
            // 1-in-30 edge: friendly wins the overwhelming majority of the time in expert mode
            // too, and this module already accepts small, disclosed over-approximations like it
            // elsewhere.
            _ => {
                if at.graveyard {
                    // NPC.cs:906-913: a full town in a graveyard still only triples the rate, and
                    // still spawns monsters two attempts in three.
                    rate *= 3.0;
                    if rng.random_ratio(1, 3) {
                        spawn_friendly = true;
                        max *= 0.6;
                    }
                } else {
                    spawn_friendly = true;
                    max *= 0.6;
                }
            }
        }
    }

    // `NPC.cs:925-929`: an unlucky player's world is busier, which is the one place bad luck does
    // something a player might actually want.
    //
    // ```csharp
    // if (!spawnFriendly && RollOnlyBadLuckExtreme(50) == 0) {
    //     spawnRate = (int)((float)spawnRate * 0.85f);
    //     maxSpawns = (int)((float)maxSpawns * 1.15f);
    // }
    // ```
    //
    // `Luck.RollOnlyBadLuckExtreme` (`Terraria.GameContent/Luck.cs:53-60`) returns `-1` outright
    // unless `luck < 0`, so this cannot fire for anybody at or above zero - which used to be
    // everybody on this server, and was the stated reason the arm was left out. A `rate` that is
    // *lower* is faster: it is the denominator of the per-tick attempt.
    if !spawn_friendly && at.luck < 0.0 && rng.random::<f32>() < -at.luck {
        // `Main.rand.Next(range / 10)` with `range` 50, so one in five once the outer test passes.
        if rng.random_range(0..5) == 0 {
            rate *= 0.85;
            max *= 1.15;
        }
    }

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
/// `invaders` is real here for one case only, and it is the pillar fight: a tower zone sets it
/// (`NPC.cs:404-409`), and this server's own invasions never reach this function at all, because
/// `tick_spawning` returns into `spawn_invaders` before it. One of the game's exclusions is still
/// dropped, because the thing it tests does not exist here: `ZoneOldOneArmy`. `Main.infectedSeed`,
/// which would clear `flag` again, is likewise unmodelled.
fn town_suppression_applies(at: Conditions) -> bool {
    !at.in_tower_zone
        && ((!at.blood_moon && !at.event_moon) || at.day_time)
        && (!at.eclipse || !at.day_time)
        && !matches!(at.biome, Biome::Corruption | Biome::Crimson)
        && !at.meteor
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
            in_tower_zone: false,
            graveyard: false,
            meteor: false,
            lihzahrd_temple: false,
            luck: 0.0,
        }
    }

    /// A fresh RNG for a call that never touches the town-suppression roll (`town_npcs: 0`, or an
    /// event overruling it) and so does not care which one it gets.
    fn any_rng() -> SmallRng {
        SmallRng::seed_from_u64(0)
    }

    /// The spawn *rate* itself has a luck arm, and it is the one place bad luck does something a
    /// player might want: `if (!spawnFriendly && RollOnlyBadLuckExtreme(50) == 0) { spawnRate =
    /// (int)(spawnRate * 0.85f); maxSpawns = (int)(maxSpawns * 1.15f); }` (`NPC.cs:925-929`).
    ///
    /// It cannot fire at or above zero luck - `RollOnlyBadLuckExtreme` returns -1 outright
    /// (`Luck.cs:53-60`) - which is why it was left out entirely while every player here was at
    /// zero. A *lower* rate is faster: it is the denominator of the per-tick attempt.
    #[test]
    fn an_unlucky_players_world_is_busier() {
        let mean = |luck: f32| {
            let mut rng = SmallRng::seed_from_u64(3);
            let mut rate_total = 0u64;
            let mut cap_total = 0.0f64;
            const RUNS: u32 = 20_000;
            for _ in 0..RUNS {
                let (rate, cap, _) = rates(Conditions { luck, ..plain() }, &mut rng);
                rate_total += u64::from(rate);
                cap_total += f64::from(cap);
            }
            (
                rate_total as f64 / f64::from(RUNS),
                cap_total / f64::from(RUNS),
            )
        };
        let (neutral_rate, neutral_cap) = mean(0.0);
        let (lucky_rate, lucky_cap) = mean(1.0);
        let (cursed_rate, cursed_cap) = mean(-0.7);

        assert_eq!(
            (neutral_rate, neutral_cap),
            (lucky_rate, lucky_cap),
            "good luck cannot reach this arm at all, which is what `RollOnlyBadLuckExtreme` means"
        );
        assert!(
            cursed_rate < neutral_rate,
            "a cursed world spawns faster: {cursed_rate:.1} against {neutral_rate:.1}"
        );
        assert!(
            cursed_cap > neutral_cap,
            "and holds more at once: {cursed_cap:.2} against {neutral_cap:.2}"
        );
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

    /// A meteor crater is busier than the ground it fell on, and a town cannot quiet it.
    ///
    /// Two vanilla lines, and they pull the same way: `NPC.cs:636-640` is the last arm of the zone
    /// rate chain (`spawnRate * 0.4`, `maxSpawns * 1.1`), and `NPC.cs:800` names `!ZoneMeteor` among
    /// the exclusions on the whole town-suppression block, alongside the evils. Building a house on
    /// a crater is not meant to make it safe.
    ///
    /// Neutralised twice, each failing its own assertion: deleting the `_ if at.meteor` arm from the
    /// biome chain gives "a crater should be busier than the forest it fell on: 216 vs 216", and
    /// dropping `&& !at.meteor` from `town_suppression_applies` gives "a full town should not quiet
    /// a crater".
    #[test]
    fn a_crater_is_busier_than_the_ground_it_fell_on_and_a_town_cannot_quiet_it() {
        let (forest, forest_cap, _) = rates(plain(), &mut any_rng());
        let crater = Conditions {
            meteor: true,
            ..plain()
        };
        let (rate, cap, _) = rates(crater, &mut any_rng());
        assert!(
            rate < forest,
            "a crater should be busier than the forest it fell on: {rate} vs {forest}"
        );
        assert!(
            cap > forest_cap,
            "and hold more of them: {cap} vs {forest_cap}"
        );

        // The evils take their own arm first, so a corrupted crater is corruption's rate, not the
        // crater's: vanilla's chain is `else if`, and this is the arm order rather than a tie.
        let corrupt = Conditions {
            biome: Biome::Corruption,
            ..plain()
        };
        assert_eq!(
            rates(corrupt, &mut any_rng()).0,
            rates(
                Conditions {
                    meteor: true,
                    ..corrupt
                },
                &mut any_rng()
            )
            .0,
            "the corruption's arm answers before the crater's",
        );

        // ...and a full town, which quiets a forest outright, does nothing at all here.
        let mut rng = SmallRng::seed_from_u64(1);
        for _ in 0..200 {
            let (town_rate, town_cap, friendly) = rates(
                Conditions {
                    town_npcs: 3,
                    ..crater
                },
                &mut rng,
            );
            assert!(!friendly, "a full town should not quiet a crater");
            assert_eq!((town_rate, town_cap), (rate, cap));
        }
        assert!(
            rates(
                Conditions {
                    town_npcs: 3,
                    ..plain()
                },
                &mut rng
            )
            .2,
            "a full town does quiet an ordinary forest, which is what makes the above a difference",
        );
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

    /// Standing among tombstones does not make the place safe, which is the whole point of building
    /// there: the town-suppression block's graveyard sub-cases throttle the rate by a smaller factor
    /// and then roll *separately* for the friendly fork (`NPC.cs:861-869`, `:886-892`, `:906-913`),
    /// where the ordinary case picks one or the other.
    ///
    /// The measure is monsters per attempt, not `spawnRate`: at two residents a graveyard is
    /// actually the *slower* of the two on raw rate (2.33x flat, against a mean of 1.67x), and is
    /// still three times as dangerous, because five attempts in six there draw a monster where two
    /// in three of the ordinary ones draw a harmless critter instead. Comparing rates alone would
    /// have read that backwards.
    ///
    /// Neutralised by deleting the three `if at.graveyard` arms in `rates` so both sides take the
    /// ordinary path: the two figures come out identical and the assertion fails at every one of
    /// the three headcounts.
    #[test]
    fn a_town_does_not_quieten_a_graveyard_the_way_it_quietens_a_forest() {
        for town_npcs in [1u32, 2, 3] {
            // Both forks are rolls, so this is an average over many draws: the chance that one
            // attempt puts a *monster* on the field, which is `1/rate` on the attempts that are not
            // diverted into a critter.
            let monsters_per_attempt = |graveyard: bool| {
                let mut rng = SmallRng::seed_from_u64(4);
                let mut total = 0.0;
                for _ in 0..20_000 {
                    let (rate, _, friendly) = rates(
                        Conditions {
                            town_npcs,
                            graveyard,
                            ..plain()
                        },
                        &mut rng,
                    );
                    if !friendly {
                        total += 1.0 / f64::from(rate);
                    }
                }
                total / 20_000.0
            };
            let ordinary = monsters_per_attempt(false);
            let haunted = monsters_per_attempt(true);
            assert!(
                haunted > ordinary * 1.5,
                "{town_npcs} residents: a graveyard base ({haunted:.6} monsters an attempt) should \
                 stay far more dangerous than an ordinary one ({ordinary:.6})"
            );
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

    /// The temple is busier than the jungle it is buried in, and it stacks (`NPC.cs:641-650` is a
    /// separate `if` below the biome chain, not another arm of it).
    ///
    /// Neutralised by turning the `if at.lihzahrd_temple` block in [`rates`] off: the first
    /// assertion fires, "spawnRate x0.8: left 300, right 240", the temple settling for the
    /// surrounding rock's own numbers.
    #[test]
    fn the_temple_quickens_the_jungle_it_is_buried_in() {
        // A plain underground band first, where neither clamp is anywhere near, so the two factors
        // can be read off exactly.
        let plain_band = Conditions {
            depth: Depth::Underground,
            ..plain()
        };
        let (base_rate, base_cap, _) = rates(plain_band, &mut any_rng());
        let (rate, cap, _) = rates(
            Conditions {
                lihzahrd_temple: true,
                ..plain_band
            },
            &mut any_rng(),
        );
        assert_eq!(rate, (base_rate as f32 * 0.8) as u32, "spawnRate x0.8");
        assert!(
            (cap - base_cap * 1.2).abs() < 1e-4,
            "maxSpawns x1.2: {base_cap} -> {cap}"
        );

        // ...and it stacks on the jungle rather than replacing it, which is what makes a temple
        // busier than the jungle it is buried in. The cap is already at its own ceiling down there
        // (`MAX_SPAWNS * 3`), so the rate is what shows it.
        let jungle = Conditions {
            depth: Depth::Cavern,
            biome: Biome::Jungle,
            ..plain()
        };
        let (jungle_rate, _, _) = rates(jungle, &mut any_rng());
        let (temple_rate, _, _) = rates(
            Conditions {
                lihzahrd_temple: true,
                ..jungle
            },
            &mut any_rng(),
        );
        assert!(
            temple_rate < jungle_rate,
            "the temple stacks on the jungle: {jungle_rate} -> {temple_rate}"
        );
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

/// Everything one scan of the tiles around a point says about the place.
///
/// The game's zones are independent flags rather than one winner, and `SceneMetrics.CalculateZones`
/// (`SceneMetrics.cs:668-686`) sets all of them off the same pass of tile counts. [`Biome`] has to
/// pick one, so the three flags that have their own spawn branches and are not a `Biome` ride along
/// here rather than being thrown away: each is a counter the scan is already walking every tile for,
/// so none of them costs a second pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zones {
    /// The one biome that won, which is what most of the game's own spawn checks read first.
    pub biome: Biome,
    /// `ZoneDesert = EnoughTilesForDesert` (`SceneMetrics.cs:683`), true alongside whatever else the
    /// place also is: a corrupted desert really is both at once, and that is exactly where the three
    /// converted sandsharks live.
    pub desert: bool,
    /// `ZoneGlowshroom = EnoughTilesForGlowingMushroom` (`SceneMetrics.cs:684`), which is
    /// `MushroomTileCount >= 100` (`:52`, `:260`).
    pub glowshroom: bool,
    /// `ZoneMeteor = EnoughTilesForMeteor` (`SceneMetrics.cs:685`), which is
    /// `MeteorTileCount >= 75` (`:56`, `:268`).
    pub meteor: bool,
    /// `SceneMetrics.EnoughTilesForShimmer` (`:252`), which is `ShimmerTileCount >= 300`
    /// (`:24`) and `ShimmerTileCount = _liquidCounts[3]` (`:601`, `LiquidID.Shimmer = 3`).
    ///
    /// The only one of these counted off *liquid* rather than off blocks, which is why it is the
    /// one flag the scan's `!is_active()` arm has to look at rather than skip.
    ///
    /// Deliberately the raw tile-count flag and not `SceneMetrics.ZoneShimmer`, which is
    /// `EnoughTilesForShimmer && UndergroundForShimmering && !ZoneDungeon` (`:711-712`). The one
    /// thing that reads this today is the shimmer pylon, and vanilla's pylon check reads the raw
    /// flag (`TeleportPylonsSystem.cs:307-308`), not the zone. Anything wanting the full
    /// `ZoneShimmer` has to add the other two terms itself.
    pub shimmer: bool,
}

/// Which biome the surrounding tiles say we are in.
///
/// Deliberately without a glowing-mushroom or shimmer variant, and it is not an oversight to fix
/// later. This is the ambient spawn pool's *single winner*, and neither of those is one in the
/// game either: a mushroom cave sits on mud, so vanilla's own roster for it hangs off the ground
/// tile (`tileType == 70`, `NPC.cs:3637`, `:3674`) rather than off a biome, and shimmer has no
/// ambient roster at all. Both are independent `SceneMetrics` flags, so they live on [`Zones`]
/// beside `desert` and `meteor`; giving either one a `Biome` seat would make it *displace* a real
/// pool (a mushroom cave in the snow would stop being snow), which is a spawn bug, not a fix.
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
        // The dungeon answers for itself at every depth and adds nothing generic, because vanilla's
        // `else if (ZoneDungeon)` (`NPC.cs:2629-2795`) returns down every path it has and never
        // reaches the underground chain the arm below transcribes. Without this a hardmode dungeon
        // grew Mimics, Rune Wizards and Black Recluses the game never puts in one. What hardmode
        // really adds there is [`hard_dungeon_pick`], which is a chain rather than a pool; the same
        // shape, and the same reason, as [`pool`]'s own `(_, Dungeon)` arm.
        (_, Dungeon) => &[],
        // The two evils, which are the same shape with different names.
        (Surface, Corruption) => &[
            81,  // CorruptSlime
            121, // Slimer
            94,  // Corruptor
        ],
        // The Clinger is a *corruption* enemy rather than a jungle one, whatever its place in a
        // "hardmode underground jungle" roster suggests: the arm it lives in is the two evils'
        // shared one, `tileType == 22 && ZoneCorrupt || 23 || 25 || 112 || 163 || 661`
        // (`NPC.cs:4125`), and 661 is Corrupt Jungle Grass, which is what puts one in a corrupted
        // jungle and nowhere else in a jungle at all. Its own branch is `(Main.hardMode & flag16)
        // && Main.rand.Next(3) == 0 && Main.tile[x, y].type == tileType` (`:4136-4139`), and it is
        // anchored to that tile the same way a Man Eater is (`SpawnNPC(..., 101, 0, spawnTileX,
        // spawnTileY)`); `game::ai::rooted` roots a plant that arrives without an anchor in the
        // solid tile under it, so the plumbing is already here.
        //
        // `flag16` is `spawnTileY >= Main.rockLayer` (`:4127`), which is this server's `Cavern`
        // alone. It is listed on the `Underground | Cavern` arm anyway, exactly as the Cursed
        // Hammer above it already is (`:4132`, the same `flag16`), so the two share one disclosed
        // narrowing rather than each getting its own depth split.
        (Underground | Cavern, Corruption) => &[
            81,  // CorruptSlime
            83,  // CursedHammer
            94,  // Corruptor
            98,  // SeekerHead — a world feeder
            101, // Clinger
            170, // PigronCorruption
            473, // BigMimicCorruption
        ],
        // Crimslime alone, and the Blood Feeder and Blood Jelly that used to sit beside it are gone
        // on purpose. Each of 241 and 242 has exactly one `SpawnNPC` site in the whole of
        // `NPC.Spawner`, and both are water: `NPC.cs:1772` and `:1776`, the two `Main.hardMode &&
        // waterTile && ZoneCrimson` arms, now in [`water_pick`]. The crimson's own land arm
        // (`:4066-4123`) never names either. Listing them here put a jellyfish on dry grass, where
        // its own AI (`game::ai::swimmer`) has nothing to do but tumble and fall.
        (Surface, Crimson) => &[
            183, // Crimslime
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
        // The jungle. The Angry Trapper is the one member of vanilla's hardmode jungle-grass arm
        // (`tileType == 60 && Main.hardMode && Main.rand.Next(3) != 0`, `NPC.cs:3864-3912`) that
        // was in no pool at all, so every other branch of that arm was here and its rooted one was
        // not: `else if (Main.rand.Next(3) == 0 && Main.tile[x, y].type == tileType)` at
        // `:3905-3908`.
        //
        // It is on both depth arms because vanilla reaches it at both. Its branch sits below the
        // arm's two `surfaceSpawn` ones (152 at `:3866`, 177 at `:3870`) and its three
        // `spawnTileY > Main.worldSurface` ones (205, 236 and the Moss Hornet at `:3874-3904`), and
        // `surfaceSpawn` is `spawnTileY <= Main.worldSurface` (`NPC.cs:1203`), so above the surface
        // line the three middle branches cannot fire and the trapper is what answers when the two
        // surface rolls decline. The arm's own fallthrough, the Giant Tortoise, is already on both
        // arms here for the same reason.
        (Surface, Jungle) => {
            if day {
                &[
                    177, // Derpling
                    153, // GiantTortoise
                    175, // AngryTrapper
                ]
            } else {
                &[
                    152, // GiantFlyingFox
                    153, // GiantTortoise
                    175, // AngryTrapper
                ]
            }
        }
        // No Arapaima here, for the same reason the crimson arm above has no Blood Jelly: 157 has
        // exactly one `SpawnNPC` site in `NPC.Spawner` and it is `NPC.cs:1768`, the `Main.hardMode
        // && waterTile && ZoneJungle` arm, now in [`water_pick`]. The jungle's own land arm
        // (`:3864-3912`) never names it. On land the fastest swimmer in the game just flops.
        (Underground | Cavern, Jungle) => &[
            175, // AngryTrapper
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
        // ...and everything under it. No Black Recluse (163): `NPC.Spawner` reaches it from exactly
        // one place, the spider nest's own arm (`NPC.cs:1673`, see [`spider_pick`]), so listing it
        // here put recluses in every hardmode cave instead of in the nests they belong to.
        (Underground | Cavern, _) => &[
            77,  // ArmoredSkeleton
            85,  // Mimic
            93,  // GiantBat
            110, // SkeletonArcher
            141, // ToxicSludge
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

/// What the surface night has going on beyond the plain weather and depth of it.
///
/// Its own struct rather than more fields on [`Conditions`], which is documented as the rate and
/// cap inputs: none of these changes how *fast* anything spawns, only what turns up.
#[derive(Clone, Copy, Debug, Default)]
pub struct Seasonal {
    /// `Main.halloween` (`Main.cs:13329-13347`), the real-world calendar rather than world state.
    pub halloween: bool,
    /// `Main.xMas` (`Main.cs:13290-13309`).
    pub xmas: bool,
    /// `Player.ZoneGraveyard` (`NPC.cs:389`), which is the client's own answer (see
    /// [`crate::game::player::Player::in_graveyard`]).
    pub graveyard: bool,
    pub hard_mode: bool,
    /// `Main.expertMode` (`Main.cs:2785`, `Difficulty >= GameDifficultyLevel.Expert`), which is the
    /// only gate the whole armed-zombie family has: 430 and 432-436, the Armed Zombie Eskimo (431)
    /// and the Armed Torch Zombie (591) are all behind it and behind nothing else about the world.
    /// It sits here rather than on [`Conditions`] for the same reason the calendar does: it changes
    /// what turns up, not how fast.
    pub expert: bool,
    pub blood_moon: bool,
    /// `Main.dayTime`. The chain below is the *night* chain, and a graveyard is the one thing that
    /// reaches it in daylight (`NPC.cs:4202`, `if (!ZoneGraveyard && Main.dayTime)`: standing among
    /// tombstones skips the whole daytime block and drops through to this).
    pub day_time: bool,
    /// `Main.moonPhase`. `Terraria.Enums.MoonPhase` names them: 0 is `Full` and 4 is `Empty`, the
    /// new moon, which is the darkest night and the one that doubles the demon eyes.
    pub moon_phase: u8,
    /// `Main.raining` (`NPC.cs:1287`, `raining = Main.raining`), for the surface night's own rain
    /// arm (`NPC.cs:4675`). It is here rather than on [`Conditions`] for the same reason the rest
    /// of this struct is: rain changes what turns up, not how fast.
    pub raining: bool,
    /// `BirthdayParty.PartyIsUp`, which `game/party.rs` already keeps. It sits beside the two
    /// calendar flags above because vanilla's bunny chain treats it as a third one: Halloween, then
    /// Christmas, then a party, then the plain bunny (`NPC.cs:1637-1656`, `:2588-2624`,
    /// `:4284-4303`).
    pub party: bool,
}

/// Which zombie this attempt would field, and how likely it is to be carrying a torch.
///
/// `NPC.GetZombieSettings` (`NPC.cs:5595-5619`), whose four out-parameters decide every one of the
/// surface night's closing arms. Vanilla calls it once at the top of `SpawnAnNPC` (`NPC.cs:1286`),
/// before any other roll in the chain, and every arm below reads the same answer: the armed zombie
/// and the plain zombie are the *same* style with a different sprite, which is why the two cannot
/// each roll their own.
///
/// The fourth out-parameter, `maggotZombieChance`, is not a field here: it is set to 20
/// (`NPC.cs:5600`) and nothing in the function ever moves it, so [`seasonal_night_pick`] carries it
/// inline with the same citation.
#[derive(Clone, Copy, Debug)]
pub struct ZombieSettings {
    /// `zombieStyle`, `Main.rand.Next(7)` (`NPC.cs:5601`): 0 to 6, indexing both the armed family
    /// and the plain one.
    pub style: u8,
    /// `spawnArmedZombies` (`NPC.cs:5598`), which starts true and is cleared in one place only:
    /// `WorldGen.Skyblock.lowTiles && !DownedAnyPreHardmodeBoss` (`NPC.cs:5615-5618`). This server
    /// generates no Skyblock world, so the field is here for the shape rather than because anything
    /// can currently clear it, and the narrowing is the same one the Skyblock arm at `NPC.cs:4691`
    /// already carries.
    pub armed: bool,
    /// `torchZombieChance` (`NPC.cs:5599`, `:5606-5614`), the one-in-N behind `NPC.cs:4722`.
    pub torch_chance: u32,
}

/// [`ZombieSettings`] for one spawn attempt.
///
/// `starting_health` is vanilla's `playerHasStartingHealth` (`NPC.cs:416`,
/// `player.statLifeMax <= 100`), read off the player the attempt is for. It is not a curiosity: a
/// fresh character has exactly 100, so on an ordinary server the torch zombie's odds start at one
/// in five rather than one in twelve, and a crowd sharpens them further.
///
/// The caller reads it off `Player::life_max`, which is the same number: packet 16's second field
/// is assigned straight to `statLifeMax` (`MessageBuffer.cs:1106`), the base maximum rather than
/// `statLifeMax2`, which is what carries a player's equipment bonuses and is what `NPC.cs:416`
/// deliberately does not read.
///
/// `active_players` is `numberOfActivePlayers` (`NPC.cs:266`), the same count [`Conditions`]
/// already carries.
pub fn zombie_settings(
    starting_health: bool,
    active_players: u32,
    rng: &mut SmallRng,
) -> ZombieSettings {
    // NPC.cs:5599-5600, the two defaults, and `:5601` the style.
    let mut torch_chance: i32 = 12;
    let style = rng.random_range(0..7u8);
    // NPC.cs:5606-5614. Integer division, then a floor of 2, exactly as the game writes it: five
    // players still leave the chance at 3, and six or more sit on the floor.
    if starting_health {
        torch_chance = (5 - (active_players / 2) as i32).max(2);
    }
    ZombieSettings {
        style,
        armed: true,
        torch_chance: torch_chance as u32,
    }
}

/// The surface night chain's own arms, ahead of the ordinary pool: `NPC.cs:4539-4740`.
///
/// This is not a pool. Vanilla's surface night is a *sequence* of independent rolls, each of which
/// returns the moment it hits, and the biome pool is only what is left when every one of them
/// misses. Folding these into a weighted list would give the Raven and the Ghost a share of every
/// night instead of one attempt in twelve and one in thirty, so the shape is kept: `None` means
/// nothing here answered and the caller should draw from [`pool`] as before.
///
/// Reached whenever the surface is dark, and *also* in daylight in a graveyard (`NPC.cs:4202`).
/// Not reached in the corruption, the crimson, the jungle or the dungeon, which vanilla answers
/// with their own arms of the outer chain before `else if (surfaceSpawn)` at `NPC.cs:4168` is
/// tested at all.
///
/// Deliberately left to the pools and to other lanes, each with its place in the order below kept
/// so the arms that *are* here land at vanilla's own odds:
///
/// * `NPC.cs:4618` the blood-moon Clown, `:4638` the Possessed Armor and `:4643` the Blood Zombie
///   and Drippler. All three already have a home in [`hardmode_pool`] and [`blood_moon_pool`];
///   taking them here as well would give each of them two sources. The last two now make their
///   rolls and hand the draw back, which is what the promise above is worth: without them a snowy
///   hardmode night would answer with a Wolf where the game had already answered with an armour.
///   (The line numbers here were `:4636` and `:4640` and named the wrong arm; both are corrected.)
/// * `NPC.cs:4691-4711` the Skyblock arm, for a world shape this server does not generate.
///
/// The chain's own last arm answers rather than handing back, which is the one place this differs
/// in shape from [`cavern_seasonal_pick`]: vanilla's surface night ends in a zombie
/// (`NPC.cs:4771-4816`) and not in a pool, so the fallthrough is transcribed and [`pool`]'s surface
/// night is reached only through the arms above that hand the draw back on purpose.
///
/// The negative ids vanilla spawns alongside six of these arms (`-38` to `-43`, `NPC.cs:4569`,
/// `:4581-4610`; `-54`/`-55` in the rain arm; and `-26` to `-37` and `-44`/`-45` in the closing
/// switch, `:4772-4815`) are not types: `NPCID.FromNetId` (`NPCID.cs:12478`) maps them back onto
/// the very same NPC with a size multiplier (`NPC.cs:8080-8137`) through `NetIdMap`
/// (`NPCID.cs:10451-10460`), so `SmallRainZombie` and `BigRainZombie` are both 223 and
/// `Small`/`Big Female Zombie` are both 200. `tools/check_spawn_reach.py` drops them from vanilla's
/// roster for that reason, and nothing here models NPC scale, so they are dropped here too. The
/// closing switch's own `Main.rand.Next(3) == 0` size swap (`:4812-4815`) is therefore the last
/// statement of the chain and changes nothing downstream, so unlike the rolls above it, it is not
/// reproduced.
///
/// `ground_block` is the tile the spawn stands on, the game's own `spawnTileY` (`NPC.cs:329`), which
/// is this server's `y + 1`. Only the snow arm reads it.
pub fn seasonal_night_pick(
    at: Seasonal,
    zombie: ZombieSettings,
    ground_block: u16,
    luck: f32,
    rng: &mut SmallRng,
) -> Option<u16> {
    let one_in = |rng: &mut SmallRng, n: u32| rng.random_ratio(1, n);

    // NPC.cs:4539. The Raven is the season's own bird, and a graveyard has one whether or not it is
    // October.
    if (at.halloween || at.graveyard) && one_in(rng, 12) {
        return Some(301); // Raven
    }
    // NPC.cs:4544. The Ghost is the graveyard's alone: no calendar produces one.
    if at.graveyard && one_in(rng, 30) {
        return Some(316); // Ghost
    }
    // NPC.cs:4549.
    if (at.halloween || at.graveyard) && at.hard_mode && one_in(rng, 10) {
        return Some(304); // HoppinJack
    }
    // NPC.cs:4554: one attempt in six takes the demon-eye branch, and on the new moon a further one
    // in two of the rest takes it as well, so the darkest night of the eight is roughly three times
    // as full of eyes as the others.
    if one_in(rng, 6) || (at.moon_phase == 4 && one_in(rng, 2)) {
        // NPC.cs:4556. Left to `hardmode_pool`, which already carries the Wandering Eye; the roll
        // itself is still made, because skipping it would hand its third of this branch to the arms
        // below and make them a third too common in hardmode.
        if at.hard_mode && one_in(rng, 3) {
            return None;
        }
        // NPC.cs:4561: `Main.rand.Next(317, 319)`, so 317 or 318.
        if at.halloween && one_in(rng, 2) {
            return Some(317 + rng.random_range(0..2)); // DemonEyeOwl, DemonEyeSpaceship
        }
        // NPC.cs:4566. The plain Demon Eye, which the surface night pool also carries; the same
        // reasoning as the Wandering Eye above applies, so the roll stands and the draw is left to
        // the pool.
        if one_in(rng, 2) {
            return None;
        }
        // NPC.cs:4577-4612: the five coloured eyes, one of which is drawn flat.
        const EYES: [u16; 5] = [
            190, // CataractEye
            191, // SleepyEye
            192, // DialatedEye
            193, // GreenEye
            194, // PurpleEye
        ];
        return Some(EYES[rng.random_range(0..EYES.len())]);
    }
    // NPC.cs:4623 and :4628. `RollOnlyBadLuck(300)` is a plain `Main.rand.Next(300)` at luck zero
    // (`Luck.cs:31-37`), unlike its `Extreme` sibling. The pair are the only wedding a blood moon
    // ever throws, and a graveyard has them on an ordinary night.
    if (at.blood_moon || at.graveyard) && rolls_only_unlucky(luck, 300, rng) {
        return Some(53); // TheGroom
    }
    if (at.blood_moon || at.graveyard) && rolls_only_unlucky(luck, 300, rng) {
        return Some(536); // TheBride
    }
    // NPC.cs:4633. The full moon, and note the third condition: this is *two* attempts in three, not
    // every one, and `!Main.dayTime` is explicit here, so a daylit graveyard gets no werewolves.
    if !at.day_time && at.moon_phase == 0 && at.hard_mode && !one_in(rng, 3) {
        return Some(104); // Werewolf
    }
    // NPC.cs:4638, the Possessed Armor, and `:4643`, the Blood Zombie and the Drippler. Both are
    // already carried by [`hardmode_pool`] and [`blood_moon_pool`], so both rolls are made and the
    // draw is handed straight back: they sit above the snow and rain arms below, and without them a
    // hardmode or blood-moon night would reach those arms every time instead of two thirds and
    // three fifths of the time.
    if !at.day_time && at.hard_mode && one_in(rng, 3) {
        return None;
    }
    // `Main.rand.Next(5) < 2`, which is not a `one_in`.
    if at.blood_moon && rng.random_range(0..5) < 2 {
        return None;
    }
    // NPC.cs:4655, the snow arm. It keys on the tile underfoot rather than on the player's zone, so
    // a patch of ice in a forest answers here too, and it `return`s: a snowy graveyard has no Maggot
    // Zombies and a snowy October no costumed ones, because vanilla never reaches those arms with
    // ice under the spawn.
    //
    // `TileID.Sets.IcesSnow` is `{161, 200, 163, 164, 147}` (`TileID.cs:297`) and the arm adds 162,
    // the thin ice, on its own line. These are tile ids: 163 and 164 here are Purple and Pink Ice,
    // not the two spiders.
    // A `matches!` rather than a `[u16; 6]` and `contains`: every draw that gets past the werewolf
    // reaches this, and on `measure_the_graveyard_and_the_seasonal_chain`'s ordinary night the
    // linear array scan cost +3.8 ns against this form's +0.6 ns.
    if matches!(ground_block, 147 | 161 | 162 | 163 | 164 | 200) {
        // NPC.cs:4657 and `:4661`. The Ice Elemental and the Wolf come from here and from one
        // other place each in the whole spawner (`:5232` is the caverns' own Ice Elemental, the
        // flying chain's snow arm), so without this arm both were unreachable. Note `!ZoneGraveyard`
        // on both: a graveyard in the snow gets neither.
        if !at.graveyard && at.hard_mode && one_in(rng, 4) {
            return Some(169); // IceElemental
        }
        if !at.graveyard && at.hard_mode && one_in(rng, 3) {
            return Some(155); // Wolf
        }
        // NPC.cs:4665, the expert Armed Zombie Eskimo, which is the arm's third branch and the only
        // place in the whole spawner 431 comes from.
        if zombie.armed && at.expert && one_in(rng, 2) {
            return Some(431); // ArmedZombieEskimo
        }
        // NPC.cs:4671's plain Zombie Eskimo is the arm's fallthrough and is already in [`pool`]'s
        // snow night, so it is handed back rather than answered.
        return None;
    }
    // NPC.cs:4675, the rain arm. All three of its outcomes are NPC 223 (`-54` and `-55` are the
    // small and big Rain Zombie, the same type at a different scale), so the inner `Next(3)` and
    // `Next(2)` collapse away.
    if at.raining && one_in(rng, 2) {
        return Some(223); // ZombieRaincoat
    }
    // NPC.cs:4712, and it answers ahead of the Maggot Zombie below it:
    //
    // ```csharp
    // if (ZoneGraveyard && RollOnlyBadLuckExtreme(30) == 0) {
    //     SpawnNPC(spawnTileX * 16 + 8, spawnTileY * 16, 691);
    //     return;
    // }
    // ```
    //
    // `RollOnlyBadLuckExtreme` returns `-1` outright for anybody whose luck is not negative
    // (`Terraria.GameContent/Luck.cs:53-60`), so this is a graveyard arm that only an *unlucky*
    // player ever sees - a broken Magic Mirror, a squashed ladybug, a Stinkbug. It was left out
    // while this server modelled no luck at all, on the reasoning that it could not fire; NPC 691
    // was the last of three entries in `docs/spawn-gaps.tsv` and the only one of them that was
    // ever going to move.
    if at.graveyard && rolls_extremely_unlucky_only(luck, 30, rng) {
        return Some(691); // MossZombie
    }
    // NPC.cs:4717: the graveyard's own zombie. `maggotZombieChance` is 20 and nothing in
    // `GetZombieSettings` (`NPC.cs:5595-5619`) ever moves it.
    if at.graveyard && one_in(rng, 20) {
        return Some(632); // MaggotZombie
    }
    // NPC.cs:4722, the Torch Zombie, and the only place either it or its armed twin comes from. Its
    // gate is neither a season nor a place: any ordinary night hands one out, and on a server whose
    // players still have their starting hundred health it is one attempt in five rather than the
    // one in twelve the bare default would give (see [`zombie_settings`]).
    if one_in(rng, zombie.torch_chance) {
        if zombie.armed && at.expert && one_in(rng, 2) {
            return Some(591); // ArmedTorchZombie
        }
        return Some(590); // TorchZombie
    }
    // NPC.cs:4734: `Main.rand.Next(319, 322)`, so 319, 320 or 321.
    if at.halloween && one_in(rng, 2) {
        return Some(319 + rng.random_range(0..3)); // ZombieDoctor, ZombieSuperman, ZombiePixie
    }
    // NPC.cs:4739: `Main.rand.Next(331, 333)`, so 331 or 332.
    if at.xmas && one_in(rng, 2) {
        return Some(331 + rng.random_range(0..2)); // ZombieXmas, ZombieSweater
    }
    // NPC.cs:4744-4769, the armed zombies. Note which style is missing from the game's own switch:
    // case 1 is excluded by the gate itself (`zombieStyle != 1`), because the Bald Zombie has no
    // armed twin, and cases 0 and 2 to 6 map onto 430 and 432 to 436 in order.
    if zombie.armed && zombie.style != 1 && at.expert && one_in(rng, 3) {
        return Some(match zombie.style {
            2 => 432, // ArmedZombiePincussion
            3 => 433, // ArmedZombieSlimed
            4 => 434, // ArmedZombieSwamp
            5 => 435, // ArmedZombieTwiggy
            6 => 436, // ArmedZombieCenx
            // `short type7 = 430` is the game's own initialiser as well as its case 0, so a style
            // the switch does not name would answer with it there too.
            _ => 430, // ArmedZombie
        });
    }
    // NPC.cs:4771-4816, the chain's fallthrough: the seven plain zombies, one per style. This is
    // what an ordinary surface night in the game actually ends in, so it answers rather than
    // handing the draw back to [`pool`].
    Some(match zombie.style {
        1 => 132, // BaldZombie
        2 => 186, // PincushionZombie
        3 => 187, // SlimedZombie
        4 => 188, // SwampZombie
        5 => 189, // TwiggyZombie
        6 => 200, // FemaleZombie
        // `short type8 = 3` is the initialiser and case 0 both, as above.
        _ => 3, // Zombie
    })
}

/// The cavern chain's own seasonal arms, ahead of the ordinary pool: `NPC.cs:5005-5199`.
///
/// The surface's sibling of [`seasonal_night_pick`], and the same shape for the same reason:
/// vanilla's caverns are a *sequence* of independent rolls, each returning the moment it hits, and
/// the pool is only what is left when every one of them misses. `None` means nothing here answered
/// and the caller should draw from [`pool`] as before.
///
/// Named for the cavern rather than "underground" on purpose. Vanilla's own `underGround`
/// (`NPC.cs:1144`) is `spawnTileY <= Main.rockLayer`, the *dirt* layer, whose arm is `NPC.cs:4818`
/// and is not this one. This chain is the fallthrough below the rock layer and above the
/// underworld, which is [`Depth::Cavern`] here.
///
/// `lower_caverns` is `(double)spawnTileY > (Main.rockLayer + (double)Main.maxTilesY) / 2.0`
/// (`NPC.cs:5021`), the bottom half of the stone, which is a different line from
/// [`Conditions::below_dirt_midline`]'s `(worldSurface + rockLayer) / 2`.
///
/// `ground_block` is the tile the spawn stands on, the game's own `spawnTileY` (`NPC.cs:329`:
/// `FindSpawnTile` walks down to the first solid tile, so its `spawnTileY` is this server's
/// `y + 1`). It is here for the two stone arms below and nothing else.
///
/// Deliberately left out, each with its place in the order kept so the arms that *are* here land at
/// vanilla's own odds:
///
/// * `NPC.cs:5007` the Skeleton Merchant, who already has his own branch in [`try_spawn`] with the
///   two conditions this signature cannot see (standing water, and one alive at a time).
/// * `NPC.cs:5017` the Rune Wizard, already carried by [`hardmode_pool`].
/// * `NPC.cs:5088`, `:5100` the ice and snow arms, which belong to the snow pool, and which is also
///   why the caller keeps [`Biome::Snow`] out of this chain: both of them `return` above `:5115`,
///   so a snow cavern never sees a Halloween skeleton.
/// * `NPC.cs:5127-5134`, the second and third branches of the Bone Throwing Skeleton arm. The arm
///   is `int num56 = Main.rand.Next(4)` followed by three `num56 == 0` tests in a row, so only the
///   first of them can ever hold: 450 and 451 are dead in the game itself, and the arm is one draw
///   in four for 449 with 452 taking the other three. That bug is transcribed as the bug it is, so
///   450 and 451 stay in `docs/spawn-gaps.tsv` while 449 and 452 leave it.
///
/// The closing `switch` (`NPC.cs:5141-5199`) answers rather than handing back, the same way
/// [`seasonal_night_pick`]'s does and for the same reason: it is vanilla's own fallthrough for the
/// walking half of the caverns, so [`pool`]'s cavern list is reached through the arms above that
/// decline on purpose (the flying half at `:5005`, hardmode's nine draws in ten at `:5051`, and the
/// Undead Miner and cavern-monster rolls at `:5083` and `:5105`) rather than by falling off the end.
/// Its four cases are the plain Skeleton and its three look-alikes, each with a one-in-three size
/// swap onto a negative net id (`-46` to `-53`) that `NPCID.NetIdMap` (`NPCID.cs:10451-10460`) maps
/// straight back onto the same four types, so the swap changes nothing and is not reproduced.
///
/// `glowshroom_ground` is vanilla's `ZoneGlowshroom && (tileType == 70 || tileType == 190)`, the
/// gate both halves of the chain put in front of their Spore pair (`NPC.cs:5110` and `:5209`). The
/// zone alone is not enough: the spawn also has to be standing on mushroom grass or a mushroom
/// block, so the caller decides it per candidate tile rather than once per player.
pub fn cavern_seasonal_pick(
    at: Seasonal,
    no_worms: bool,
    lower_caverns: bool,
    ground_block: u16,
    glowshroom_ground: bool,
    alive: &dyn Fn(u16) -> bool,
    rng: &mut SmallRng,
) -> Option<u16> {
    let one_in = |rng: &mut SmallRng, n: u32| rng.random_ratio(1, n);

    // NPC.cs:5005, `else if (Main.rand.Next(2) == 0)`: heads is this chain, the walkers, and tails
    // is the flying chain at `:5201` (Illuminant Bat, Jungle Bat, Chaos Elemental, Ice Bat, Giant
    // Bat). Both are [`pool`]'s business apart from the arms below, so the roll is still made and a
    // tails hands the draw straight back: skipping it would make every arm here twice as common.
    if !one_in(rng, 2) {
        // NPC.cs:5209, the flying half's own glowshroom arm and the Spore Bat's only source.
        //
        // Two arms sit above it and neither can answer where this is reached: `:5205`'s Jungle Bat
        // wants `ZoneJungle`, which the caller's own biome exclusion keeps out of this chain
        // entirely, and `:5201`'s Illuminant Bat wants a hardmode hallow. The hallow is *not*
        // excluded by the caller, so in a cavern that reads as hallowed and holds a hundred
        // mushroom tiles at once, vanilla would answer with an Illuminant Bat half the time and
        // this answers Spore Bat every time. That place needs pearlstone and mushroom grass in the
        // same scan box; the narrowing is disclosed rather than paid for with a biome parameter.
        if glowshroom_ground {
            return Some(634); // SporeBat
        }
        return None;
    }
    // NPC.cs:5012-5015, `if (Main.rand.Next(80) == 0)`, one arm below the Skeleton Merchant the
    // caller answers and one above the Rune Wizard [`hardmode_pool`] carries. No progression gate
    // and no depth gate beyond the cavern chain she lives in: she is found at any point in a
    // world's life.
    //
    // She is not a prop. `NPC.aiStyle = 42` (`NPC.cs:11524`) is the disguise, and its routine
    // (`NPC.cs:30360-30389`) drops it: within two hundred pixels of a player it can see, or on
    // being moved or hurt at all, it counts twenty-one ticks and calls `Transform(196)`. All of
    // that was already here and already tested (`game::ai::ambush::lost_girl`, dispatched at ai
    // style 42, whose `Some(NYMPH)` becomes [`crate::game::npc::Npc::become_type`] through
    // `Effects::transform`), so this one missing arm was the only reason either 195 or 196 had a
    // path into a world. An earlier reading of this file recorded the transformation as the
    // missing half and left the arm out on that basis; the transformation is present, and it was
    // the spawn that was missing.
    if one_in(rng, 80) {
        return Some(195); // LostGirl
    }
    // NPC.cs:5021. `offensiveToTim` (a second, likelier one in fifty for a player carrying a magic
    // weapon) is not read here, so Tim is very slightly rarer than the game's, never commoner.
    if lower_caverns && one_in(rng, 200) {
        return Some(45); // Tim
    }
    // NPC.cs:5027 and `:5039`, the marble and granite arms:
    //
    // ```csharp
    // if (nearMarble && Main.rand.Next(4) != 0)
    // {
    //     if (Main.rand.Next(6) != 0 && !AnyNPCs(480) && Main.hardMode) 480; else 481;
    //     return;
    // }
    // if (nearGranite && Main.rand.Next(5) != 0)
    // {
    //     if (Main.rand.Next(6) != 0 && !AnyNPCs(483)) 483; else 482;
    //     return;
    // }
    // ```
    //
    // All four types come from here and nowhere else in `NPC.Spawner`, which is why all four were
    // unreachable. Note what the gates actually say: only the Medusa is hardmode, so a granite
    // pocket has its golems and flyers on day one, and a marble one has its Greek Skeletons.
    //
    // `nearMarble`/`nearGranite` are narrowed to the ground tile itself (`NPC.cs:1059-1065`, the
    // first two of the flag's four branches). The other two are the player's own tile and a pair of
    // random box sweeps up to 61 tiles across (`NPC.cs:1067-1143`), left out for the same reason
    // `underground_desert_spot` and [`spider_pick`] leave out their equivalents: a real marble or
    // granite pocket is floored in its own stone, so the tile test finds the biome from inside it,
    // and the sweeps only widen the arm to spots outside one. The effect is that the four start a
    // little further in than the game's do, never that they turn up in ordinary rock.
    const MARBLE: u16 = 367;
    const GRANITE: u16 = 368;
    if ground_block == MARBLE && !one_in(rng, 4) {
        return Some(if !one_in(rng, 6) && !alive(480) && at.hard_mode {
            480 // Medusa
        } else {
            481 // GreekSkeleton
        });
    }
    if ground_block == GRANITE && !one_in(rng, 5) {
        return Some(if !one_in(rng, 6) && !alive(483) {
            483 // GraniteFlyer
        } else {
            482 // GraniteGolem
        });
    }
    // NPC.cs:5051, `if (Main.hardMode && Main.rand.Next(10) != 0)`: nine hardmode draws in ten
    // never reach the rest of this chain at all. Its own answers (Armored Viking, Armored Skeleton,
    // Icy Merman, Skeleton Archer) are all in [`hardmode_pool`] already, so the roll stands and the
    // draw is handed back. Without it a hardmode Halloween cavern would be ten times as full of
    // costumed skeletons as the game's.
    if at.hard_mode && !one_in(rng, 10) {
        return None;
    }
    // NPC.cs:5078. The graveyard's Ghost underground, and October's: unlike the surface arm at
    // `:4544`, which is the graveyard's alone, the calendar opens this one too. `!noWorms` is
    // vanilla's own condition on it, odd as it looks on something that does not burrow.
    if !no_worms && (at.halloween || at.graveyard) && one_in(rng, 30) {
        return Some(316); // Ghost
    }
    // NPC.cs:5083, the Undead Miner, and `:5105`, this world's own six cavern monsters. Both are
    // already in the caller's draw (44 in [`pool`], the six behind `CAVERN_SENTINEL`), so both
    // rolls are made and handed back. The one in three especially: dropping it would make the
    // Halloween arm below half again as common as the game's.
    if one_in(rng, 20) || one_in(rng, 3) {
        return None;
    }
    // NPC.cs:5110, the walking half's glowshroom arm, and the Spore Skeleton's only source. It sits
    // below the world's own six cavern monsters and above October's costumes, which is why it is
    // here rather than at the head: a mushroom cavern is still mostly an ordinary cavern.
    if glowshroom_ground {
        return Some(635); // SporeSkeleton
    }
    // NPC.cs:5115: `Main.rand.Next(322, 325)`, so 322, 323 or 324. The calendar alone, with no
    // graveyard half: standing among tombstones in June brings no costumes up out of the stone.
    if at.halloween && one_in(rng, 2) {
        // SkeletonTopHat, SkeletonAstonaut, SkeletonAlien
        return Some(322 + rng.random_range(0..3));
    }
    // NPC.cs:5120-5139, the Bone Throwing Skeletons. `num56` is drawn once and then tested against
    // zero three times, so the second and third branches are unreachable in the game: this is one
    // draw in four for 449 and three in four for 452, and 450 and 451 stay disclosed rather than
    // being handed odds vanilla never gives them.
    if at.expert && one_in(rng, 3) {
        return Some(if rng.random_range(0..4) == 0 {
            449 // BoneThrowingSkeleton
        } else {
            452 // BoneThrowingSkeleton4
        });
    }
    // NPC.cs:5141-5199, the chain's fallthrough: `Main.rand.Next(4)` over the plain Skeleton and its
    // three look-alikes.
    const SKELETONS: [u16; 4] = [
        21,  // Skeleton
        201, // HeadacheSkeleton
        202, // MisassembledSkeleton
        203, // PantlessSkeleton
    ];
    Some(SKELETONS[rng.random_range(0..SKELETONS.len())])
}

/// What hallowed ground offers ahead of the hallow pool (`NPC.cs:4039-4061`).
///
/// The whole arm, and its outer gate, is:
///
/// ```csharp
/// else if (((Main.hardMode && underGround) || (Main.remixWorld && Main.rand.Next(2) == 0))
///     && !waterTile && (tileType == 116 || tileType == 117 || tileType == 109 || tileType == 164))
/// {
///     if (downedPlantBoss && (Main.remixWorld || (!Main.dayTime && Main.time < 16200.0))
///         && surfaceSpawn && RollLuck(10) == 0 && !AnyNPCs(661))          661;
///     else if (raining && !AnyNPCs(244) && RollLuck(10) == 0)             244;
///     else if (!Main.dayTime && Main.rand.Next(2) == 0)                   122;
///     else if (Main.rand.Next(10) == 0 || (ZoneWaterCandle && Main.rand.Next(10) == 0)) 86;
///     else                                                                75;
/// }
/// ```
///
/// It is an ordered chain rather than a pool, which is the point: it is the only source in the whole
/// of `NPC.Spawner` for three of its five members. The Prismatic Lacewing (661) is the Empress of
/// Light's only summon, so with this arm missing an entire boss was unreachable; the Rainbow Slime
/// (244) and the Unicorn (86) come from here and nowhere else either, and all three sat in
/// `docs/spawn-gaps.tsv` together for exactly one reason.
///
/// `underGround` is `spawnTileY <= Main.rockLayer` (`NPC.cs:1144`), the *dirt* layer and above, so
/// the caller's gate is [`Depth::Surface`] or [`Depth::Underground`]; `surfaceSpawn` is
/// `spawnTileY <= Main.worldSurface` (`:1203`), which is [`Depth::Surface`] alone. The two are not
/// alternatives: hallowed grass in daylight is both at once, which is why the Lacewing's own arm can
/// carry `surfaceSpawn` inside a branch already gated on `underGround`.
///
/// Two leaves hand the draw back rather than answering, the same way [`cavern_seasonal_pick`] hands
/// back every arm [`pool`] already carries: the Gastropod (122) and the Pixie (75) are both in
/// [`hardmode_pool`]'s hallow entry at every depth this is reached from. Their rolls are still made,
/// in vanilla's own order, so the three arms above them land at the game's odds rather than at
/// inflated ones. The visible difference is that a hallow night here can also answer with an
/// Illuminant Bat or Slime where vanilla's `else` would have said Pixie.
///
/// Three narrowings, each disclosed rather than invented:
///
/// * `Main.remixWorld` is not modelled anywhere in this server, so the outer gate's second half
///   drops and the Lacewing's `(remixWorld || night)` collapses to the night half. Same treatment,
///   and same reason, as the Goblin Scout's arm in [`try_spawn`].
/// * `ZoneWaterCandle` is not modelled either (see [`sky_pick`], where its two arms are dead code in
///   the game itself), so the Unicorn's second chance drops and its gate is the plain one in ten.
/// * `RollLuck(10)` is `Main.rand.Next(10)` at luck zero (`Luck.cs:5-16`).
///
/// `alive` is vanilla's `AnyNPCs`. Both uses of it are real gates rather than politeness: one
/// Lacewing at a time is what stops a hallow night raining Empresses.
// Eight, one over the lint's line, and every one of them is a flag `SetSpawnFlags` sets: the same
// allow the other long pick routines in this file carry, for the same reason.
#[allow(clippy::too_many_arguments)]
pub fn hallow_ground_pick(
    downed_plant_boss: bool,
    day_time: bool,
    time: i32,
    surface_spawn: bool,
    raining: bool,
    alive: &dyn Fn(u16) -> bool,
    luck: f32,
    rng: &mut SmallRng,
) -> Option<u16> {
    // NPC.cs:4041.
    if downed_plant_boss
        && !day_time
        && time < LACEWING_LATEST
        && surface_spawn
        && rolls_lucky(luck, LACEWING_ODDS, rng)
        && !alive(PRISMATIC_LACEWING)
    {
        return Some(PRISMATIC_LACEWING);
    }
    // NPC.cs:4045.
    if raining && !alive(244) && rolls_lucky(luck, RAINBOW_SLIME_ODDS, rng) {
        return Some(244); // RainbowSlime
    }
    // NPC.cs:4049, the Gastropod, which [`hardmode_pool`] already carries: the roll is made and the
    // draw handed back.
    if !day_time && rng.random_ratio(1, 2) {
        return None;
    }
    // NPC.cs:4053.
    if rng.random_range(0..UNICORN_ODDS) == 0 {
        return Some(86); // Unicorn
    }
    // NPC.cs:4059's `else` is the Pixie, which [`hardmode_pool`] carries too.
    None
}

/// The holiday costume a critter or a plain slime wears, or the type unchanged.
///
/// Vanilla puts these swaps inside the draw rather than after it, as an `else if` above the
/// ordinary answer: `SpawnBunny`'s chain (`NPC.cs:1637-1644`, and again at `:2588-2595` and
/// `:4284-4291`) tries the Halloween bunny, then the Christmas one, then the party one, then the
/// plain one; `GetBasicSlimeToSpawn` (`NPC.cs:5654-5663`) does the same for the surface's blue
/// slime. Applying them to the type already drawn is the same distribution written the other way
/// round, and it does not need the season threaded through every pool table.
///
/// `GetBasicSlimeToSpawn_ChanceToBeHolidaySlime` (`NPC.cs:5678-5685`) is `Main.rand.Next(3) != 0`
/// outside Skyblock, the same two-in-three the bunny uses.
///
/// The Party Bunny is the third arm of the same chain (`NPC.cs:1645-1648`, `:2596-2599`,
/// `:4292-4295`) and sits below the two calendars in all three of vanilla's copies of it, so it
/// sits below them here. Without it 540 was in no pool and no branch, and a party could not put a
/// single bunny in a party hat.
///
/// One narrowing that predates the party arm and now covers all three: vanilla's copy at
/// `NPC.cs:2588-2599` carries `!flag11` (`spawnTileY >= Main.rockLayer && <= Main.UnderworldLayer`,
/// `:2557`), so a cavern's friendly draw dresses nothing up. The other two copies carry no such
/// gate, and neither does this, so a costumed bunny is very slightly commoner underground here than
/// in the game and never rarer.
///
/// The slime swap is surface-only because vanilla's is: `GetBasicSlimeToSpawn(surface: false, ...)`
/// has no holiday case at all, and the two tile types with their own answer (jungle grass, and snow
/// or ice) are answered before the default case this replaces.
fn holiday_costume(npc_type: u16, depth: Depth, at: Seasonal, rng: &mut SmallRng) -> u16 {
    const BLUE_SLIME: u16 = 1;
    let two_in_three = |rng: &mut SmallRng| !rng.random_ratio(1, 3);
    match npc_type {
        BUNNY if at.halloween && two_in_three(rng) => 303, // BunnySlimed
        BUNNY if at.xmas && two_in_three(rng) => 337,      // BunnyXmas
        BUNNY if at.party && two_in_three(rng) => 540,     // PartyBunny
        BLUE_SLIME if depth == Depth::Surface && at.halloween && two_in_three(rng) => 302, // SlimeMasked
        // `Main.rand.Next(333, 337)`, so 333 to 336: the four ribbon colours.
        BLUE_SLIME if depth == Depth::Surface && at.xmas && two_in_three(rng) => {
            333 + rng.random_range(0..4)
        }
        _ => npc_type,
    }
}

/// `NPC.goldCritterChance` (`NPC.cs:6054`), the one-in-N behind every `RollLuck(goldCritterChance)`
/// in the spawner. A plain `Main.rand.Next(400)` at luck zero (`Luck.cs:5-16`), the same narrowing
/// every other `Roll*Luck` call site in this file makes.
const GOLD_CRITTER_CHANCE: u32 = 400;

/// The gold twin a critter is drawn as instead of itself, one time in four hundred.
///
/// The sibling of [`holiday_costume`], and here for the same reason: vanilla writes these as an
/// `else if` *above* the ordinary answer rather than as a swap after it, and applying them to the
/// type already drawn is the same distribution written the other way round. Each pair below is one
/// such arm, and each is `RollLuck(goldCritterChance) == 0 ? gold : plain` with nothing else in it:
///
/// * the bird, `NPC.cs:2455-2459` and `:2538-2542` (and `:4331`, `:4352` on the monster path),
/// * the bunny, `NPC.cs:1629-1632`, `:2580-2583` and `:4276-4279`,
/// * the squirrel, `NPC.cs:1633-1636`, `:2584-2587` and `:4280-4283`,
/// * the butterfly, `NPC.cs:2489-2496` and `:4232-4239`,
/// * the frog, `NPC.SpawnFrog` (`NPC.cs:5627-5634`),
/// * the goldfish, `NPC.cs:1590-1597`, `:2303-2310` and `:2316-2323`,
/// * the worm, `NPC.cs:1616-1623`, `:2391-2398` and `:3782-3789`,
/// * the mouse, `NPC.cs:1603-1610` and `:3793-3800`.
///
/// `depth` is here for the squirrel and nothing else: its arm alone carries `& flag10`
/// (`surfaceSpawn`, `NPC.cs:1203`), so a squirrel met underground is never the gold one. The other
/// seven arms carry no depth gate at all.
///
/// Deliberately not in the table, each because its plain half is not something this server draws:
/// the Gold Dragonfly (601), the Gold Water Strider (613), the Gold Seahorse (627) and the Gold
/// Goldfish Walker (593), whose four base critters are all in the water and beach arms
/// [`water_pick`] already declines to model. The Gold Lady Bug (605) is left to the lane that
/// brings the plain Lady Bug (604) with it.
///
/// The roll is made only once a type with a twin has actually been drawn, so an ordinary attempt
/// pays one `match` and no `rng` draw at all.
fn gold_variant(npc_type: u16, depth: Depth, luck: f32, rng: &mut SmallRng) -> u16 {
    let gold = match npc_type {
        74 | 297 | 298 => 442,                 // Bird, BirdBlue, BirdRed -> GoldBird
        46 => 443,                             // Bunny -> GoldBunny
        356 => 444,                            // Butterfly -> GoldButterfly
        361 => 445,                            // Frog -> GoldFrog
        300 => 447,                            // Mouse -> GoldMouse
        357 => 448,                            // Worm -> GoldWorm
        55 => 592,                             // Goldfish -> GoldGoldfish
        299 if depth == Depth::Surface => 539, // Squirrel -> SquirrelGold
        _ => return npc_type,
    };
    if rolls_lucky(luck, GOLD_CRITTER_CHANCE, rng) {
        gold
    } else {
        npc_type
    }
}

/// The plain jungle Hornet, `NPCID.Hornet`, which is what [`pool`]'s jungle arms carry.
const HORNET: u16 = 42;

/// The first of the five hornet families, `NPCID.HornetFatty`. The other four follow it in id
/// order: Honey, Leafy, Spikey, Stingy.
const HORNET_FATTY: u16 = 231;

/// Which hornet a jungle draw is actually answered by (`NPC.SpawnHornet`, `NPC.cs:5289-5354`).
///
/// Vanilla never spawns a plain Hornet directly. All three of its hornet arms call `SpawnHornet`:
/// a Hive tile at `NPC.cs:3834-3861` (`tileType == 225`, half the time and only outside hardmode's
/// own Moss Hornet fork), a Hive wall at `:3925-3928` (`wallType == 86`, seven attempts in eight),
/// and the deep jungle-grass fallthrough at `:3929-3942`. Every one of them lands in the same
/// eight-way switch:
///
/// ```csharp
/// switch (Main.rand.Next(8))
/// {
/// case 0: ... return SpawnNPC(..., 231);
/// case 1: ... return SpawnNPC(..., 232);
/// case 2: ... return SpawnNPC(..., 233);
/// case 3: ... return SpawnNPC(..., 234);
/// case 4: ... return SpawnNPC(..., 235);
/// default: ... return SpawnNPC(..., 42);
/// }
/// ```
///
/// So five draws in eight are a *family* hornet and only three are the plain one. Every jungle
/// hornet this server has ever spawned was the plain 42, which is three eighths of what the game
/// puts in a hive.
///
/// This is [`holiday_costume`]'s and [`gold_variant`]'s shape of after-the-draw swap, and for the
/// same reason: 42 is what [`pool`]'s jungle arms stand for, so conditioning on "a hornet was
/// drawn" and then re-rolling which one is the same distribution written the other way round.
///
/// Each case also has two `Main.rand.Next(4) == 0` rolls above its answer, for the little and big
/// members of that family (`-56`/`-57` under 231, up to `-64`/`-65` under 235, and `-16`/`-17`
/// under 42). Those are not transcribed and nothing is lost by it: a negative id is a *net id*, not
/// a type, and `NPCID.FromNetId` resolves all ten of them straight back onto the same five types
/// this does answer with (`npc_data::NET_ID_MAP`, `NPCID.cs:12478-12484`). This server spawns by
/// type, so the only thing the size rolls decide is how big the sprite draws.
fn hornet_variant(npc_type: u16, rng: &mut SmallRng) -> u16 {
    if npc_type != HORNET {
        return npc_type;
    }
    match rng.random_range(0..8u16) {
        family @ 0..=4 => HORNET_FATTY + family,
        _ => HORNET,
    }
}

/// What the underworld's lava-bait critters answer with (`NPC.SpawnLavaBaitCritters`,
/// `NPC.cs:5850-5877`):
///
/// ```csharp
/// if (Main.rand.Next(3) != 0)
/// {
///     if (Main.dayTime) return SpawnNPC(..., 653);
///     ...four `Main.rand.Next(fireFlyMultiple) == 0` companions at 654...
///     return SpawnNPC(..., 654);
/// }
/// return SpawnNPC(..., 655);
/// ```
///
/// Two thirds of the draws are the day's Hell Butterfly or the night's Lavafly, and the remaining
/// third is a Magma Snail whatever the hour. Vanilla reaches it from two places, and so does this
/// server: the underworld's ordinary chain one draw in eight (`NPC.cs:4881-4884`) and the friendly
/// chain's own underworld case (`:2562-2578`).
///
/// The four extra Lavaflies are not transcribed: `fireFlyMultiple` is one of the three per-world
/// nightly critter-density rolls (`NPC.setFireFlyChance`, `NPC.cs:94495-94542`) that this server
/// does not model at all, and the same omission already stands for the firefly's own companions in
/// [`friendly_pool`]. Nothing is made unreachable by it; a Lavafly arrives alone rather than in a
/// cloud.
fn lava_bait_pick(day_time: bool, rng: &mut SmallRng) -> u16 {
    if rng.random_range(0..3) != 0 {
        if day_time {
            653 // HellButterfly
        } else {
            654 // Lavafly
        }
    } else {
        655 // MagmaSnail
    }
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
const MUSHROOM_THRESHOLD: i32 = 100; // MushroomTileThreshold (`SceneMetrics.cs:52`)
const METEOR_THRESHOLD: i32 = 75; // MeteorTileThreshold (`SceneMetrics.cs:56`)
const SHIMMER_THRESHOLD: i32 = 300; // ShimmerTileThreshold (`SceneMetrics.cs:24`)

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
    zones_at(world, x, y).biome
}

/// The same scan, also answering the zone flags that are not one of [`Biome`]'s winners.
///
/// The game's zones are independent flags, not one winner: `SceneMetrics.CalculateZones` sets
/// `ZoneDesert = EnoughTilesForDesert` (`SceneMetrics.cs:683`) alongside `ZoneCorrupt`,
/// `ZoneCrimson` and `ZoneHallow`, so a corrupted desert really is both at once. [`Biome`] has to
/// pick one, and it picks the evil, because the evils are what most of the game's own spawn checks
/// read first.
///
/// That collapse costs nothing anywhere else, and one thing here: `ZoneSandstorm` is
/// `ZoneDesert && ...`, so a corrupt, crimson or hallowed desert - which is exactly where the three
/// converted sandsharks live - would never have read as a sandstorm at all, and those three types
/// would have been unreachable in a real world while looking reachable to the roster. The count is
/// already made by the scan above; this only stops throwing it away, so the flag is free.
///
/// `ZoneGlowshroom` and `ZoneMeteor` are here for the same reason and at the same price: neither is
/// ever a `Biome` (a glowing mushroom cave sits on mud and reads as forest; a crater sits in
/// whatever it fell on), both are a plain tile count in the box this already walks, and each is the
/// only gate on a spawn branch of its own. [`Zones::shimmer`] joins them for the same reason,
/// counted off the liquid on the tiles the block counts skip.
pub fn zones_at(world: &World, x: i32, y: i32) -> Zones {
    // The ocean is defined by position rather than tiles.
    if x < 250 || x > world.width() - 250 {
        return Zones {
            biome: Biome::Ocean,
            desert: false,
            glowshroom: false,
            meteor: false,
            shimmer: false,
        };
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
    // MushroomTileCount (`SceneMetrics.cs:617`): mushroom grass, plants, trees and vines.
    const MUSHROOM_TILES: [u16; 4] = [70, 71, 72, 528];
    // MeteorTileCount (`SceneMetrics.cs:618`): meteorite ore, and nothing else.
    const METEORITE: u16 = 37;

    // Raw tile counts, before the game's evil/hallow reconciliation.
    let (mut evil, mut blood, mut holy, mut jungle, mut snow, mut sand, mut dungeon) =
        (0, 0, 0, 0, 0, 0, 0);
    let (mut mushroom, mut meteor, mut shimmer) = (0, 0, 0);
    for dy in -BIOME_SCAN_Y_UP..=BIOME_SCAN_Y_DOWN {
        for dx in -BIOME_SCAN_X..=BIOME_SCAN_X {
            let tile = world.tile(x + dx, y + dy);
            if !tile.is_active() {
                // A tile the block counts skip is exactly where the game counts its liquids:
                // `if (!tile.active()) { if (tile.liquid > 0) _liquidCounts[tile.liquidType()]++;
                // continue; }` (`SceneMetrics.cs:367-372`). Only bucket 3 is ever read as a zone
                // (`ShimmerTileCount = _liquidCounts[3]`, `:601`), so only bucket 3 is counted.
                // A tile the block counts skip is exactly where the game counts its liquids:
                // `if (!tile.active()) { if (tile.liquid > 0) _liquidCounts[tile.liquidType()]++;
                // continue; }` (`SceneMetrics.cs:367-372`). Only bucket 3 is ever read as a zone
                // (`ShimmerTileCount = _liquidCounts[3]`, `:601`), so only bucket 3 is counted.
                shimmer += i32::from(
                    tile.liquid > 0 && tile.liquid_kind == terrustia_proto::tile::Liquid::Shimmer,
                );
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
            mushroom += i32::from(MUSHROOM_TILES.contains(&b));
            meteor += i32::from(b == METEORITE);
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
    // `ZoneDesert = EnoughTilesForDesert` on its own, whatever else the place also is, and the same
    // for the other two flags that are nobody's winner.
    let flags = Zones {
        biome: Biome::Forest,
        desert: sand >= DESERT_THRESHOLD,
        glowshroom: mushroom >= MUSHROOM_THRESHOLD,
        meteor: meteor >= METEOR_THRESHOLD,
        shimmer: shimmer >= SHIMMER_THRESHOLD,
    };
    if dungeon >= DUNGEON_THRESHOLD {
        return Zones {
            biome: Biome::Dungeon,
            ..flags
        };
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
            return Zones { biome, ..flags };
        }
    }
    flags
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
        // The three big Angry Bones are not hardmode and not rare: the dungeon chain's own
        // fallthrough is `int num43 = Main.rand.Next(5)` with cases 0, 1 and 2 taking 294, 295 and
        // 296 (`NPC.cs:2767-2783`), and only cases 3 and 4 reach the plain Angry Bones below it
        // (`:2784-2795`, where `-14` and `-13` are size variants of that same 31 rather than types
        // of their own). Three fifths of what a pre-Skeletron-cleared dungeon actually throws at a
        // player was therefore missing, and the dungeon was correspondingly softer than the game's.
        //
        // This is the `num43` draw and nothing above it. The Dungeon Slime, the two traps, the
        // Cursed Skull and the Dark Caster used to be listed here too, flattened out of the chain
        // that really offers them; they are [`dungeon_pick`] now, which is where their odds and
        // their brick-style gates live.
        (_, Dungeon) => &[
            31,  // AngryBones
            294, // AngryBonesBig
            295, // AngryBonesBigMuscle
            296, // AngryBonesBigHelmet
        ],
        // The ocean's own roster is *aquatic*, and lives in [`water_pick`]: vanilla reaches it only
        // through `waterTile && isOcean` (`NPC.cs:1798`), so a shark cannot appear on dry sand.
        // A dry beach tile falls through to the ordinary surface pool the same way vanilla's does,
        // which is why standing on the shore at night still brings zombies. The beach's *harmless*
        // roster is neither of those two: it is [`friendly_chain`], which answers ahead of this.
        (depth, Ocean) => pool(depth, Forest, day),
        // No Corrupt Bunny (47) here, deliberately: vanilla's `NPC.Spawner` never spawns one at
        // any depth. The only way a Corrupt Bunny exists is `AttemptToConvertNPCToEvil`
        // (`NPC.cs:93050`) turning an ordinary Bunny under a blood moon, which
        // `convert_critters_under_a_blood_moon` now does. Listing it here made them appear out of
        // nothing in daylight.
        (Surface, Corruption) => &[
            6, // EaterofSouls
            7, // DevourerHead
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
        // The Spiked Jungle Slime is *not* a hardmode enemy, whatever a summary of the underground
        // jungle roster suggests: its arm is `tileType == 60 && spawnTileY > (worldSurface +
        // rockLayer) / 2.0` (`NPC.cs:3929`) with a one-in-four roll inside it (`:3931-3934`), and
        // `Main.hardMode` appears nowhere in either. It sits in the same arm as the Man Eater
        // (`:3935-3938`) and takes the same depth gate, so it is treated the same way this pool
        // already treats 43: the arm's own "bottom half of the underground layer and everything
        // below" is narrowed to "not the surface", which is the depth granularity [`Depth`] has.
        (_, Jungle) => &[
            42,  // Hornet
            43,  // ManEater
            56,  // Snatcher
            51,  // JungleBat
            204, // SpikedJungleSlime
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

/// The Tortured Soul (`NPCID.TorturedSoul`, the table's `DemonTaxCollector`), the one hostile in
/// the game a player turns into a townsperson rather than kills.
pub const TORTURED_SOUL: u16 = 534;

/// The Skeleton Merchant (`NPCID.SkeletonMerchant`), a wandering vendor rather than a resident: he
/// takes no house, joins no town, and leaves on the ordinary despawn timer when nobody is near.
pub const SKELETON_MERCHANT: u16 = 453;

/// The three bound town slimes (`NPCID.BoundTownSlimeOld`, `...Purple`, `...Yellow`), each of which
/// becomes a resident pet once freed.
///
/// They are three finds in three different places rather than one roster: the Old Slime is a
/// caverns find beside the Goblin Tinkerer and the Wizard (`NPC.cs:2095`), the Purple Slime is up
/// in the sky with the Harpies (`NPC.cs:1417`), and the Yellow Slime is a jungle-grass critter
/// draw (`NPC.SpawnFrog`, `NPC.cs:5621-5634`). What they share is the shape of their gate: every
/// one of them is offered only while its own unlock flag is still false, so freeing one is what
/// takes it out of the world's spawn table for good.
pub const BOUND_TOWN_SLIME_OLD: u16 = 685;
pub const BOUND_TOWN_SLIME_PURPLE: u16 = 686;
pub const BOUND_TOWN_SLIME_YELLOW: u16 = 687;

/// The Goblin Scout (`NPCID.GoblinScout`), the game's only source of Tattered Cloth (item 362,
/// `npc_drops.rs`), which is the only ingredient of the Goblin Battle Standard, which is the only
/// way anybody summons a goblin army. Nothing else in the game drops that cloth.
pub const GOBLIN_SCOUT: u16 = 73;

/// The Statue Mimic (`NPCID.StatueMimic`), which stands among the tombstones pretending to be a
/// statue on a plinth until somebody walks close enough (`game::ai::mimic`).
pub const STATUE_MIMIC: u16 = 690;

/// The Rock Golem (`NPCID.RockGolem`), the hardmode caverns' heavy: 1000 life and 85 damage out of
/// the plain stone, which is more than anything else a cave holds.
pub const ROCK_GOLEM: u16 = 631;

/// `Main.rand.Next(50)` in `CheckToSpawnRockGolem` (`NPC.cs:5809`).
const ROCK_GOLEM_ODDS: u32 = 50;

/// Plain Stone (`TileID.Stone`), one of the two grounds a Rock Golem is cut from; the other is
/// every colour of moss ([`terrustia_proto::tile_sets::is_moss`]).
const STONE: u16 = 1;

/// How far above the ground row the Rock Golem's headroom check reads (`NPC.cs:5813`,
/// `spawnTileY - 4`). A golem is 48 pixels tall and stands 1.1x scaled, so it wants a taller hole
/// than the three rows [`open_space`] already asked for.
const ROCK_GOLEM_HEADROOM: i32 = 4;

/// The Lihzahrd Temple's whole ambient roster (`NPC.cs:3914-3924`):
///
/// ```csharp
/// else if (((tileType == 226 || tileType == 232) && ZoneLihzhardTemple) || (Main.remixWorld && ZoneLihzhardTemple))
/// {
///     if (Main.rand.Next(3) == 0) SpawnNPC(..., 226); else SpawnNPC(..., 198);
/// }
/// ```
///
/// Both are unreachable anywhere else in `NPC.Spawner`, so before this the post-Plantera temple -
/// the one place in the game a player farms for Solar Tablet fragments and Lihzahrd Power Cells -
/// spawned nothing at all. The `Main.remixWorld` half of the gate is not modelled here, so the two
/// ground tiles are the whole of it.
const LIHZAHRD: u16 = 198;
const FLYING_SNAKE: u16 = 226;

/// `Main.rand.Next(3)` on the temple's own fork (`NPC.cs:3916`): a Flying Snake one draw in three,
/// a Lihzahrd the other two.
const FLYING_SNAKE_ODDS: u32 = 3;

/// The two ground tiles the temple arm accepts (`TileID.LihzahrdBrick`, `TileID.WoodenSpikes`).
const LIHZAHRD_BRICK: u16 = 226;
const WOODEN_SPIKES: u16 = 232;

/// `WallID.LihzahrdBrickUnsafe`, which is the entire definition of `ZoneLihzhardTemple`
/// (`SceneMetrics.cs:693`). It is an unsafe wall, so `Main.wallHouse[87]` is false
/// (`Main.cs:9880-10745` never sets it) and a temple does not suppress its own spawns.
const TEMPLE_WALL: u16 = 87;

/// `Main.rand.Next(20)` on the Tortured Soul's branch (`NPC.cs:4877`).
const TORTURED_SOUL_ODDS: u32 = 20;

/// The Goblin Scout's two rolls (`NPC.cs:4482`), which are OR'd rather than exclusive: one attempt
/// in fifteen wherever the distance gate is open, and a second, better chance on top of it while a
/// shadow orb has been smashed and the world is still waiting for its first goblin army.
const GOBLIN_SCOUT_ODDS: u32 = 15;
const GOBLIN_SCOUT_ORB_ODDS: u32 = 7;

/// The distressed Palworld pets (`NPCID.PalworldCattivaDistressed`, `NPCID.PalworldFoxsparksDistressed`).
///
/// A crossover encounter rather than a spawn: what arrives is a caged pet and the two Goblin
/// Archers guarding it. See `ai::hardmode::fixtures::pal` for the rest of it.
pub use terrustia_proto::npc_params::{PAL_CATTIVA, PAL_FOXSPARKS};

/// `Main.rand.Next(160) == 0` on the pal's arm (`NPC.cs:4374`).
const PAL_ODDS: u32 = 160;
/// `RollLuck(100) < 40` (`NPC.cs:4378`), which the pair are split on when the player is carrying
/// both pets already or neither. A plain `Main.rand.Next(100)` at luck zero (`Luck.cs:5-16`), the
/// same narrowing as every other `RollLuck` in this file.
const PAL_FOXSPARKS_CHANCE: u32 = 40;

/// Which of the two shows up: whichever one you are not already carrying (`NPC.cs:4375-4386`).
///
/// ```csharp
/// bool flag18 = Main.player[target].HasItem(5663);
/// bool flag19 = Main.player[target].HasItem(5664);
/// bool flag20 = RollLuck(100) < 40;
/// if (!flag19 & flag18) flag20 = true;
/// if (!flag18 & flag19) flag20 = false;
/// short type5 = (short)(flag20 ? 696 : 695);
/// ```
///
/// Carrying one of the pair and not the other pins the answer to the other one; carrying both or
/// neither leaves it to the roll. This is the one arm in this whole file that reads a player's
/// inventory, which the module's own doc comment says the spawner does not do: it does now, for
/// this one arm, because `HasItem` is answerable from the equipment sync the server already keeps
/// and leaving it out would have made the pair a coin toss forever. `Player.HasItem`
/// (`Player.cs:56554-56564`) scans slots 0 to 57, the main inventory with its coins and ammo, and
/// nothing worn, banked or in a piggy bank; the slot bound is transcribed rather than flattened to
/// "anywhere on the player".
fn pal_type(player: &Player, luck: f32, rng: &mut SmallRng) -> u16 {
    let carrying = |item: i32| {
        player
            .inventory
            .values()
            .any(|slot| slot.slot < 58 && slot.item.id == item && slot.item.stack > 0)
    };
    let has_cattiva = carrying(i32::from(terrustia_proto::npc_params::PAL_REWARD_CATTIVA));
    let has_foxsparks = carrying(i32::from(terrustia_proto::npc_params::PAL_REWARD_FOXSPARKS));
    let mut foxsparks = rolls_lucky_under(luck, 100, PAL_FOXSPARKS_CHANCE as i32, rng);
    if !has_foxsparks && has_cattiva {
        foxsparks = true;
    }
    if !has_cattiva && has_foxsparks {
        foxsparks = false;
    }
    if foxsparks {
        PAL_FOXSPARKS
    } else {
        PAL_CATTIVA
    }
}

/// Plain Sand (`TileID.Sand`), the one ground the Goblin Scout is never found on: vanilla's three
/// `tileType == 53` arms (`NPC.cs:4390`, `:4474`, `:4478`) answer every dry sand tile before the
/// chain reaches him.
const SAND: u16 = 53;

/// The two enemies the surface day only has while it is raining (`NPC.cs:4486-4493`), sitting in the
/// same chain as the Goblin Scout and immediately below him:
///
/// ```csharp
/// else if (raining && Main.rand.Next(4) == 0)
/// {
///     SpawnNPC(spawnTileX * 16 + 8, spawnTileY * 16, 224);
/// }
/// else if (!waterTile && raining && Main.rand.Next(2) == 0)
/// {
///     SpawnNPC(spawnTileX * 16 + 8, spawnTileY * 16, 225);
/// }
/// ```
///
/// The Flying Fish is the one arm in the whole daytime chain with no `!waterTile` on it: it is a
/// fish, and a flooded tile in the rain is exactly where one belongs. The Umbrella Slime keeps its
/// dry ground. Neither had an arm anywhere in this server, so a rainy afternoon on the surface was
/// the ordinary blue-slime day with wetter graphics.
pub const FLYING_FISH: u16 = 224;
pub const UMBRELLA_SLIME: u16 = 225;
const FLYING_FISH_ODDS: u32 = 4;
const UMBRELLA_SLIME_ODDS: u32 = 2;

/// ...and the two the surface day only has on a windy one (`NPC.cs:4494-4501`), the next two arms
/// down:
///
/// ```csharp
/// else if (!waterTile && wallType == 0 && Main.IsItAHappyWindyDay && isSpawningInWindDirection
///          && Main.rand.Next(3) != 0)
/// {
///     SpawnNPC(spawnTileX * 16 + 8, spawnTileY * 16, 594);
/// }
/// else if (!waterTile && wallType == 0 && (tileType == 2 || tileType == 477)
///          && Main.IsItAHappyWindyDay && isSpawningInWindDirection && Main.rand.Next(10) != 0
///          && Main.tile[spawnTileX, spawnTileY].type == tileType)
/// {
///     SpawnNPC(spawnTileX * 16 + 8, spawnTileY * 16, 628);
/// }
/// ```
///
/// Both rolls are `!= 0`, not `== 0`: on a windy day out in the open these are the *likely* outcome,
/// two attempts in three and nine in ten of what is left. A windy day is rare, and while one is
/// blowing it owns the surface.
///
/// The Dandelion is the reason this arm matters beyond a count. Its routine is already here in full
/// (`game::ai::critter::dandelion`, `AI_119_Dandelion`), it opens on `Main.IsItAHappyWindyDay`, and
/// with no arm to spawn one nothing this server could do put a Dandelion in a world.
pub const WINDY_BALLOON: u16 = 594;
pub const DANDELION: u16 = 628;
const WINDY_BALLOON_ODDS: u32 = 3;
const DANDELION_ODDS: u32 = 10;

/// The Dandelion's two grounds: `TileID.Grass` and `TileID.GolfGrass` (`NPC.cs:4498`). The Windy
/// Balloon above it takes any ground at all.
const GRASS: u16 = 2;
const GOLF_GRASS: u16 = 477;

/// `WallID.LivingWoodUnsafe` (`WallID.cs:557`), the one wall `GetSpawnWallType` reports in place of
/// whatever is actually behind the spawn tile (`NPC.cs:1016-1019`). It is why the inside of a living
/// tree never counts as open sky for the two windy-day arms, which want `wallType == 0`.
const LIVING_WOOD_WALL: u16 = 244;

/// `NPC.GetSpawnWallType` (`NPC.cs:1013-1021`):
///
/// ```csharp
/// int result = Main.tile[spawnTileX, spawnTileY - 1].wall;
/// if (Main.tile[spawnTileX, spawnTileY - 2].wall == 244 || Main.tile[spawnTileX, spawnTileY].wall == 244)
/// {
///     result = 244;
/// }
/// return result;
/// ```
///
/// Vanilla's `spawnTileY` is the ground tile, which is this server's `y + 1` (see [`try_spawn`]'s
/// own note on `tileType`), so the row it reports on is `y` and the two it checks for living wood
/// are `y - 1` and `y + 1`.
fn spawn_wall_type(world: &World, x: i32, y: i32) -> u16 {
    if world.tile(x, y - 1).wall == LIVING_WOOD_WALL
        || world.tile(x, y + 1).wall == LIVING_WOOD_WALL
    {
        return LIVING_WOOD_WALL;
    }
    world.tile(x, y).wall
}

/// `RollBadLuckExtreme(25)` on the Statue Mimic's branch (`NPC.cs:1571`), which at luck zero is a
/// plain `Main.rand.Next(25)` (`Luck.cs:40-51`) and so is what this server models, the same
/// narrowing (and the same reasoning) as every other `Roll*Luck` call site in this file.
const STATUE_MIMIC_ODDS: u32 = 25;

/// The dungeon library's own two (`NPCID.WaterBoltMimic`, `NPCID.LibrarianSkeleton`), the only NPCs
/// in the game that spawn on a *bookshelf* rather than on the tile the attempt found
/// (`NPC.cs:2748-2766`).
///
/// Both already had their routines here and neither could ever run one. The mimic is a cursed skull
/// that plays dead (`game::ai::skull`, `NPC.cs:21933-21998`) and the librarian a hardmode caster
/// (`conjuring(693)` in `npc_params`, `NPC.cs:21323-21337`), and with no arm to spawn either,
/// nothing this server could do put one in a world.
pub const WATER_BOLT_MIMIC: u16 = 694;
pub const LIBRARIAN_SKELETON: u16 = 693;

/// Their two rolls, `Main.rand.Next(8)` and `Main.rand.Next(10)` (`NPC.cs:2749`, `:2758`).
///
/// The second is an `else if`, not a second independent chance: the librarian is offered only on the
/// seven attempts in eight the mimic's roll turned down, so it is one in eight against roughly one
/// in eleven rather than one in eight against one in ten.
const WATER_BOLT_MIMIC_ODDS: u32 = 8;
const LIBRARIAN_ODDS: u32 = 10;

/// `TileID.Books` (`TileID.cs:537`), a dungeon shelf's single book tile.
///
/// Vanilla's test is `tile.type != 50` and nothing else (`NPC.cs:62972`), so neither `BooksEcho`
/// (707, `TileID.cs:1851`) nor the ordinary `Bookcases` furniture is a shelf for this purpose.
const BOOK_TILE: u16 = 50;

/// The box the spawner looks for a book in: `new Point(spawnTileX - 16, spawnTileY - 16), 32, 32`
/// (`NPC.cs:2752`, `:2761`). The offset is what makes the box surround the candidate rather than
/// hang off one corner of it.
const BOOK_SEARCH_BACK: i32 = 16;
const BOOK_SEARCH_SPAN: i32 = 32;

/// The Skeleton Merchant's two nested rolls collapsed (`NPC.cs:5004`'s `Next(2)` and `:5007`'s
/// `Next(35)`), because nothing between them can spawn anything.
const SKELETON_MERCHANT_ODDS: u32 = 70;

/// The five the ordinary dungeon chain offers ahead of its pool, and the rolls that offer them
/// (`NPC.cs:2723-2747`, transcribed in [`dungeon_pick`]).
///
/// The two traps are the reason this chain exists here at all. Both are dungeon furniture with a
/// routine of their own already written and tested (`game::ai::track::spike_ball` and
/// `::wheel`), and neither was in any pool or any branch, so no world this server ran could hold
/// one. The Spike Ball is slab-wall only and the Blazing Wheel tile-wall only, so both are also a
/// reason the brick style has to reach this chain rather than being spent on the hardmode one above
/// it.
const SPIKE_BALL: u16 = 70;
const BLAZING_WHEEL: u16 = 72;
const DUNGEON_SLIME: u16 = 71;
const CURSED_SKULL: u16 = 34;
const DARK_CASTER: u16 = 32;
const DUNGEON_SLIME_ODDS: u32 = 35;
const SPIKE_BALL_ODDS: u32 = 3;
const BLAZING_WHEEL_ODDS: u32 = 5;
const CURSED_SKULL_ODDS: u32 = 7;
const DARK_CASTER_ODDS: u32 = 7;

/// `aiStyle == 20`, which is the only thing `NearSpikeBall` matches on: it is a property of the
/// routine and not of the type, so it would catch anything else driven by the same one.
const SPIKE_BALL_AI_STYLE: i32 = 20;

/// The four ordinary critters the ground below the surface line has of its own, and the rolls that
/// offer them (`NPC.cs:3776-3805`, four consecutive arms of the outer chain).
///
/// None of them is a friendly-attempt draw: they sit on the monster path, above the Lihzahrd
/// temple at `:3914` and everything after it, which is where [`try_spawn`] puts them. So an
/// ordinary cave has worms and mice in it whether or not a nearby town has quieted the wild, which
/// is what makes these four the commonest critters in the game and is why all four sat unreachable
/// here: nothing in this server drew a critter outside a `spawnFriendly` attempt at all.
///
/// The Lac Beetle keys on jungle grass and nothing else, so an underground jungle is where it
/// lives; the Cochineal and Cyan Beetles are the caverns' own pair and have their own arm further
/// down (`NPC.cs:4925-4935`, [`CAVERN_BEETLE_ODDS`]).
const LAC_BEETLE: u16 = 219;
const LAC_BEETLE_ODDS: u32 = 60;

/// Doctor Bones (`NPCID.DoctorBones`), the one arm directly above the Lac Beetle's in the same
/// chain: `tileType == 60 && RollLuck(500) == 0 && !Main.dayTime` (`NPC.cs:3772-3775`).
///
/// Jungle grass at night and nothing else. It carries no depth gate, no biome gate and no
/// progression gate, so a jungle *surface* at night offers him as readily as an underground one,
/// which is why he goes in the outer chain here rather than into any pool: neither the surface's
/// seasonal chain nor the jungle's pool is reached from where vanilla answers with him.
///
/// `RollLuck(500)` is a plain `Main.rand.Next(500)` at luck zero (`Luck.cs:5-16`), the same
/// narrowing every other `Roll*Luck` call site in this file makes.
const DOCTOR_BONES: u16 = 52;
const DOCTOR_BONES_ODDS: u32 = 500;
const WORM: u16 = 357;
const WORM_ODDS: u32 = 8;
const MOUSE: u16 = 300;
const MOUSE_ODDS: u32 = 13;
const SNAIL: u16 = 359;
const SNAIL_ODDS: u32 = 13;

/// How far above the bottom of the world the worm and the mouse stop (`NPC.cs:3780`, `:3791`,
/// `spawnTileY < Main.maxTilesY - 210`), which is ten rows shallower than the underworld itself.
const CRITTER_FLOOR: i32 = 210;

/// The caverns' beetle pair (`NPC.cs:4925-4935`):
///
/// ```csharp
/// else if (Main.rand.Next(60) == 0)
/// {
///     if (ZoneSnow) { SpawnNPC(..., 218); }
///     else { SpawnNPC(..., 217); }
/// }
/// ```
///
/// One arm below the Rock Golem's and with no gate of its own beyond the roll, so one cavern draw
/// in sixty is a beetle wherever the stone is.
const COCHINEAL_BEETLE: u16 = 217;
const CYAN_BEETLE: u16 = 218;
const CAVERN_BEETLE_ODDS: u32 = 60;

/// The underworld's own draw of the lava-bait critters (`NPC.cs:4881-4884`), one attempt in eight,
/// immediately below the Tortured Soul's arm and above the Bone Serpent's. See [`lava_bait_pick`]
/// for what it answers with.
const LAVA_BAIT_ODDS: u32 = 8;

/// The Enchanted Nightcrawler's own arm, at the head of the friendly grass chain
/// (`NPC.cs:2409-2413`):
///
/// ```csharp
/// if (((!Main.dayTime && Main.numClouds <= 55 && Main.cloudBGActive == 0f
///     && Star.starfallBoost > 3f) & flag10) && RollLuck(2) == 0)
/// {
///     SpawnNPC(spawnTileX * 16 + 8, spawnTileY * 16, 484);
///     break;
/// }
/// ```
///
/// A clear, starry night with a meteor shower on it, and nothing about rain: the arm above it
/// (`:2381`) takes every rainy draw and `break`s, so `!raining` is already true wherever this is
/// reached. `flag10` is `surfaceSpawn`.
///
/// Nothing is narrowed here. The three sky clauses are all state this server already keeps: the
/// cloud count is the world's own (straight out of the `.wld` and walked every tick by
/// `Weather::tick_clouds`), `Main.cloudBGActive` is `Weather::cloud_bg_active`, and the shower is
/// the dusk roll behind [`EventSpawns::starfall_night`]. The caller resolves all three into that
/// one bool, so this arm costs one bool test on an ordinary night.
///
/// `RollLuck(2)` is a plain `Main.rand.Next(2)` at luck zero (`Luck.cs:5-16`).
///
/// It carries [`friendly_pool`]'s own standing narrowing and no new one: the arm lives inside
/// vanilla's `case 2`/`109`/`477`/`492`, the four grasses, and this server keys the friendly draw
/// on the player's biome rather than on the tile underfoot. So a stone-floored forest surface
/// answers here where the game would have fallen through the tile switch and drawn nothing.
const ENCHANTED_NIGHTCRAWLER: u16 = 484;
const ENCHANTED_NIGHTCRAWLER_ODDS: u32 = 2;

/// The Firefly (`NPCID.Firefly`) and what the same arm answers with when the grass underfoot is
/// hallowed instead (`NPC.cs:2414-2421`):
///
/// ```csharp
/// int type2 = 355;
/// if (tileType == 109) { type2 = 358; }
/// ```
///
/// Keyed on the tile and not on `ZoneHallow`, exactly as vanilla writes it, so a single hallowed
/// grass tile in an ordinary forest answers with a Lightning Bug and a pearlstone hallow floored in
/// plain grass does not. `109` is `TileID.HallowedGrass`.
const FIREFLY: u16 = 355;
const LIGHTNING_BUG: u16 = 358;
const HALLOWED_GRASS: u16 = 109;

/// The Prismatic Lacewing, which is not a critter you catch for a collection: killing one is the
/// *only* way the Empress of Light is ever summoned (`NPC.cs:80309-80319`). With no arm spawning
/// it, an entire boss, her fight and her whole drop table were unreachable. The id itself lives in
/// `npc_params` beside the rest of its routine's numbers, since its own AI reads it too.
use terrustia_proto::npc_params::PRISMATIC_LACEWING;

/// The hallow chain's four hallowed grounds, `tileType == 116 || 117 || 109 || 164`
/// (`NPC.cs:4039`): Pearlsand, Pearlstone, HallowedGrass and HallowedIce.
const HALLOW_GROUND: [u16; 4] = [116, 117, 109, 164];

/// The night is 32,400 ticks long, so `Main.time < 16200.0` (`NPC.cs:4041`) is its first half: a
/// Lacewing is found between dusk and midnight and never after it.
const LACEWING_LATEST: i32 = 16_200;

/// `RollLuck(10)` on the Lacewing's arm and again on the Rainbow Slime's (`NPC.cs:4041`, `:4045`),
/// which at luck zero is a plain `Main.rand.Next(10)` (`Luck.cs:5-16`), the same narrowing as every
/// other `Roll*Luck` call site in this file.
const LACEWING_ODDS: u32 = 10;
const RAINBOW_SLIME_ODDS: u32 = 10;

/// `Main.rand.Next(10)` on the Unicorn's arm (`NPC.cs:4053`).
const UNICORN_ODDS: u32 = 10;

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
/// the place and lets the caller pick evenly among it. The gem variants are left out on purpose;
/// they are a cosmetic roll on top of these, and the two arms that carry them
/// (`NPC.cs:2600-2620`) are the caverns' own, not this pool's. The gold variants are *not* left
/// out any more: see [`gold_variant`], which the caller applies to whatever this answers, and
/// [`friendly_chain`], which makes its own six rolls at the arms that carry them. The
/// underworld's lava-bait critters are a separate monster-path arm (see [`lava_bait_pick`]), not
/// part of `spawnFriendly` at all, so they were never this pool's to carry.
///
/// Everything below the surface line is empty here on purpose and is [`deep_friendly_pick`]'s: the
/// game's own tail there is a gated fork rather than a set, so a pool cannot say it.
///
/// It is not the whole of that branch, and never was. Four of vanilla's chains inside it answer and
/// `return` before this table is ever read: the graveyard's [`GRAVEYARD_VERMIN`], and the beach,
/// the water and the rain, which are [`friendly_chain`]. This is the fallthrough under all of them.
///
/// The whole of a graveyard's friendly draw (`NPC.cs:2101-2115`).
///
/// Not part of [`friendly_pool`], because vanilla's arm is not a variation on the ordinary one: it
/// sits ahead of every other friendly branch and `return`s, so among tombstones there are no birds,
/// no bunnies and no fireflies, only these two.
pub const GRAVEYARD_VERMIN: [u16; 2] = [
    606, // Maggot
    610, // Rat
];

/// The Frog (`NPCID.Frog`), which is not simply drawn: it is the last arm of a chain of its own.
const FROG: u16 = 361;

/// The plain Bunny (`NPCID.Bunny`), vanilla's own last resort in the friendly critter chain.
const BUNNY: u16 = 46;

/// The night Owl (`NPCID.Owl`), which like the frog is a chain rather than a draw.
const OWL: u16 = 611;

/// ...and what the chain's rare arm answers with instead (`NPCID.OwlMimic`).
pub const OWL_MIMIC: u16 = 689;

/// `RollBadLuckExtreme(100)` on the owl's own arm (`NPC.cs:2442`), a plain `Main.rand.Next(100)` at
/// luck zero (`Luck.cs:40-51`) and so what this server models, the same narrowing as
/// [`STATUE_MIMIC_ODDS`] and every other `Roll*Luck` call site in this file.
const OWL_MIMIC_ODDS: u32 = 100;

/// What an owl draw actually answers with (`NPC.cs:2439-2449`):
///
/// ```csharp
/// if ((!raining && !Main.dayTime && Main.rand.Next(5) == 0) & flag10)
/// {
///     if (RollBadLuckExtreme(100) == 0) { SpawnNPC(..., 689); }
///     else { SpawnNPC(..., 611); }
///     break;
/// }
/// ```
///
/// The mimic is not a standalone arm anywhere in vanilla's chain, and there is no biome or
/// progression gate anywhere near it. It is one branch of the ordinary grass critter switch
/// (`case 2`/`109`/`477`/`492`, the four grasses that [`friendly_pool`]'s surface forest arm already
/// stands for), sitting between the firefly arm above it and the daytime songbird arm below. So the
/// Owl Mimic is not something found in a place of its own: it is one owl in a hundred on an
/// ordinary clear night, and the other ninety-nine are harmless.
///
/// The `!raining` half is redundant where vanilla writes it. The whole case opens with
/// `if (raining && spawnTileY <= Main.UnderworldLayer)`, whose every arm ends in `break`
/// (`NPC.cs:2381-2407`), so control cannot reach here in the rain and the explicit test is belt and
/// braces. [`friendly_pool`] already declares that it does not model weather, so no gate of our own
/// is invented for it.
fn spawn_owl(luck: f32, rng: &mut SmallRng) -> u16 {
    if rolls_extremely_unlucky(luck, OWL_MIMIC_ODDS, rng) {
        OWL_MIMIC
    } else {
        OWL
    }
}

/// What a frog draw actually answers with (`NPC.SpawnFrog`, `NPC.cs:5621-5634`).
///
/// Vanilla never spawns a frog directly. Both jungle-grass friendly arms (`NPC.cs:2363` and
/// `:3831`) call `SpawnFrog`, which is a three-arm chain with the plain frog last, so the bound
/// Yellow Slime is only ever met by somebody wandering a jungle looking at frogs. It is the one
/// bound town slime with no progression gate and no depth gate whatsoever.
///
/// The middle arm, `RollLuck(goldCritterChance) == 0` -> Gold Frog (445), is left out. It is not
/// that the class is unmodelled: [`friendly_chain`] rolls [`GOLD_CRITTER_CHANCE`] on six of its own
/// arms, and the gem critters are a separate class again and are modelled in [`deep_friendly_pick`]
/// (the frog's arm is not one of their three sites). It is that the Gold Frog belongs with the Gold
/// Bunny, the Gold Butterfly and the rest of the ordinary-critter roster, which is still gapped in
/// `docs/spawn-gaps.tsv`, and taking one of them out of order would leave that list saying less
/// than it does now.
fn spawn_frog(world: &World, alive: &dyn Fn(u16) -> bool, luck: f32, rng: &mut SmallRng) -> u16 {
    // `RollLuck(30)` is `Main.rand.Next(30)` at luck zero (`Luck.cs:5-16`). The `alive` scan is
    // last on purpose: `&&` short-circuits, so an ordinary frog draw never walks the NPC table.
    if !world.progress.unlocked_slime_yellow
        && rolls_lucky(luck, 30, rng)
        && !alive(BOUND_TOWN_SLIME_YELLOW)
    {
        return BOUND_TOWN_SLIME_YELLOW;
    }
    FROG
}

/// The first id of the seven Gem Squirrels, `NPCID.GemSquirrelAmethyst`.
const GEM_SQUIRREL: u16 = 639;

/// ...and of the seven Gem Bunnies, `NPCID.GemBunnyAmethyst`.
const GEM_BUNNY: u16 = 646;

/// Which gem a gem critter carries (`GetGemSquirrelToSpawn`, `NPC.cs:5717-5745`, and
/// `GetGemBunnyToSpawn`, `NPC.cs:5687-5715`).
///
/// Vanilla writes the two as separate methods, but they are the same `Main.rand.Next(100)` ladder
/// with the same six cutoffs and the same gem order, because the two id runs are laid out
/// identically: `639..=645` is the squirrel run and `646..=652` the bunny run, each Amethyst, Topaz,
/// Sapphire, Emerald, Ruby, Diamond, Amber in id order. So this is one ladder keyed by the first id
/// of a run, and the cutoffs below are the game's own, written rarest first exactly as it writes
/// them:
///
/// ```csharp
/// int num = Main.rand.Next(100);
/// if (num < 5)  { return 644; }  // Diamond
/// if (num < 13) { return 645; }  // Amber
/// if (num < 23) { return 643; }  // Ruby
/// if (num < 35) { return 642; }  // Emerald
/// if (num < 51) { return 641; }  // Sapphire
/// if (num < 72) { return 640; }  // Topaz
/// return 639;                    // Amethyst
/// ```
///
/// So Amethyst is 28 draws in a hundred and Diamond is 5, which is the point of the ladder: the
/// critter tells you what the rock around it is worth before you have swung at it.
fn gem_critter(first: u16, rng: &mut SmallRng) -> u16 {
    let roll = rng.random_range(0..100);
    if roll < 5 {
        return first + 5; // Diamond
    }
    if roll < 13 {
        return first + 6; // Amber
    }
    if roll < 23 {
        return first + 4; // Ruby
    }
    if roll < 35 {
        return first + 3; // Emerald
    }
    if roll < 51 {
        return first + 2; // Sapphire
    }
    if roll < 72 {
        return first + 1; // Topaz
    }
    first // Amethyst
}

/// What a friendly attempt draws below the surface, which is a chain and not a pool
/// (`NPC.cs:2600-2624`):
///
/// ```csharp
/// else if (Main.rand.Next(3) == 0)
/// {
///     if (flag11)      { if (Main.rand.Next(5) == 0) SpawnNPC(..., GetGemSquirrelToSpawn()); }
///     else if (flag10) { SpawnNPC(..., Utils.SelectRandom(Main.rand, new short[2] { 299, 538 })); }
/// }
/// else if (flag11) { if (Main.rand.Next(5) == 0) SpawnNPC(..., GetGemBunnyToSpawn()); }
/// else             { SpawnNPC(..., 46); }
/// ```
///
/// `flag11` (`NPC.cs:2557`) is `spawnTileY >= Main.rockLayer && spawnTileY <= Main.UnderworldLayer`,
/// which is [`Depth::Cavern`] exactly, and `flag10` (`NPC.cs:2380`) is `surfaceSpawn`, which is
/// [`friendly_pool`]'s business. So the tail says three things this server did not:
///
/// * The caverns are the gem critters' one ordinary home. All fourteen were unreachable here, and
///   this arm is why: nothing in this server drew from either ladder at all.
/// * The caverns hold *nothing else*. `friendly_pool` used to answer the Bunny and the Squirrel at
///   every depth from one fallback arm, but neither is drawn under the rock layer in the game: a
///   cavern attempt is a gem critter one time in five on either side of the fork and nothing at all
///   the other four.
/// * The plain Bunny belongs to the dirt layer between the surface line and the rock, and the
///   Squirrel is the surface's alone.
///
/// `None` is a real answer, not a failure: vanilla's arm simply spawns nothing, and the caller drops
/// the attempt rather than falling through to a monster.
///
/// Two of the three sites that reach the ladders are left out, and neither of them is the only way
/// to meet a gem critter:
///
/// * `NPC.cs:2382-2389`, the rain arm, which is the same fourteen at better odds under the same
///   `deeperThanRockLayer` gate. Its other four arms are the rain critter roster (the Worm, the Gold
///   Worm and both Goldfish Walkers), none of which this server models, so taking the gem half alone
///   would put gem critters in the rain with none of their neighbours.
/// * `NPC.cs:2564-2574`, which wants `inRemixStartingArea`: the drunk-world seed's surface-in-hell
///   start, a world layout this server does not generate.
fn deep_friendly_pick(depth: Depth, rng: &mut SmallRng) -> Option<u16> {
    // `NPC.cs:2600`, the fork: heads is the squirrel side, tails the bunny side.
    if rng.random_range(0..3) == 0 {
        // `NPC.cs:2602-2607`. The `else if (flag10)` below it is the surface Squirrel pair, which
        // `friendly_pool` carries, so off the surface a miss here is simply nothing.
        if depth == Depth::Cavern && rng.random_range(0..5) == 0 {
            return Some(gem_critter(GEM_SQUIRREL, rng));
        }
        return None;
    }
    // `NPC.cs:2614-2620`.
    if depth == Depth::Cavern {
        if rng.random_range(0..5) == 0 {
            return Some(gem_critter(GEM_BUNNY, rng));
        }
        return None;
    }
    // `NPC.cs:2621-2624`, the plain Bunny, which is the dirt layer's and not the cavern's.
    (depth == Depth::Underground).then_some(BUNNY)
}

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
        // The ordinary surface: birds, a bunny, a squirrel and butterflies by day; fireflies and
        // owls by night (`NPC.cs:2414`,`2439-2449`,`2452-2552`). The evils and the hallow borrow
        // it, as their own critter rolls fall back to the same set in the game.
        //
        // The Red Squirrel is not a variant of the Squirrel: `NPC.cs:2611` draws the pair with
        // `Utils.SelectRandom(Main.rand, new short[2] { 299, 538 })`, an even coin flip
        // (`Utils.cs:2628-2631`), so both are ordinary entries here. That arm carries `& flag10`
        // (`surfaceSpawn`), which is why 538 is on this list and not on the underground one below,
        // where 299 already sits.
        //
        // The Stink Bug is an arm of its own rather than a coin flip (`NPC.cs:2474-2486`, and
        // `:4218-4229` on the monster path), gated on `Main.rand.Next(stinkBugChance) == 0`.
        // `stinkBugChance` is one of the three per-world nightly critter-density rolls
        // (`NPC.setFireFlyChance`, `NPC.cs:94495-94542`): on two days in three it is 999999 and the
        // bug never comes, and on the rest it is 1 to 13. This server models none of the three, and
        // the Butterfly beside it is already in this list ungated by its own `butterflyChance` for
        // exactly the same reason, so the bug joins as one more even entry. That is the pool's
        // standing narrowing, not a new one.
        (Surface, Forest | Corruption | Crimson | Hallow) => {
            if day {
                &[
                    74,    // Bird
                    297,   // BirdBlue
                    298,   // BirdRed
                    BUNNY, // Bunny
                    // The squirrel arm is `Utils.SelectRandom(Main.rand, new short[2] { 299, 538 })`
                    // (`NPC.cs:2611`), so the Red Squirrel is drawn exactly as often as the grey
                    // one. Only the grey one was here, which is why 538 had no producer at all.
                    299, // Squirrel
                    538, // SquirrelRed
                    356, // Butterfly
                    669, // Stinkbug
                ]
            } else {
                &[
                    355, // Firefly
                    OWL, // ...and the owl, which is a chain: see `spawn_owl`
                ]
            }
        }
        // The underworld's friendly draw is a chain, not a pool: `NPC.cs:2562-2578` hands it
        // straight to `SpawnLavaBaitCritters`, which [`lava_bait_pick`] transcribes and the caller
        // runs instead of drawing from here.
        (Underworld, _) => &[],
        // Underground and cavern are not a pool at all: vanilla's tail (`NPC.cs:2600-2624`) is a
        // fork with a one-in-five gate on each side, so it answers a gem critter, a plain Bunny, or
        // nothing, depending on the layer. [`deep_friendly_pick`] owns it, and this returning empty
        // is what hands the draw over to it. It used to answer a Bunny and a Squirrel at both
        // depths, which put the Squirrel three hundred tiles below the last tree and left every one
        // of the fourteen gem critters with no producer anywhere in this server.
        (Underground | Cavern, _) => &[],
        // Anything else is the surface, where the Bunny and the Squirrel really are the fallback
        // (`NPC.cs:2609-2611`, `:2621-2624`).
        (_, _) => &[
            BUNNY, // Bunny
            299,   // Squirrel
            538,   // SquirrelRed
        ],
    }
}

/// `WorldGen.beachDistance` (`WorldGen.cs:4071`), the *wider* of the game's two shore bands and not
/// the same number as [`OCEAN_DISTANCE`]: the ocean proper is the outer 250 columns, the beach the
/// outer 380. `isBeach` (`NPC.cs:1206`) is that band and the surface line and nothing else, with no
/// tile test at all, so a rock in the outer 380 columns above the surface still reads as beach.
const BEACH_DISTANCE: i32 = 380;

/// The beach and ocean surface's own critters (`NPC.cs:2116-2199`). None of them is a variation on
/// [`friendly_pool`]'s ocean entry: they are their own chain, which `return`s, so on a beach these
/// are what a friendly attempt draws and the goldfish and ducks are what it draws everywhere else.
const SEAGULL: u16 = 602;
const SEA_TURTLE: u16 = 625;
const DOLPHIN: u16 = 615;
const SEAHORSE: u16 = 626;
const GOLD_SEAHORSE: u16 = 627;

/// ...and the fourth arm of that chain, which the brief for this work did not name but which is the
/// direct sibling of the Seahorse in the same `switch (num31)` (`NPC.cs:2177-2186`, and again at
/// `:1899-1908`). It was unreachable for exactly the same reason as the other four.
const PUFFERFISH: u16 = 688;

/// The critters that stand on the *surface of the water* rather than on the floor under it
/// (`NPC.cs:2239-2297`), keyed by the tile the water sits on: jungle grass, sand, and everything
/// else.
const TURTLE: u16 = 616;
const TURTLE_JUNGLE: u16 = 617;
const GREBE: u16 = 608;
const WATER_STRIDER: u16 = 612;
const GOLD_WATER_STRIDER: u16 = 613;
const DUCK: u16 = 362;
const DUCK_WHITE: u16 = 364;

/// ...and what the same arm falls through to when there is no reachable water surface
/// (`NPC.cs:2299-2323`): a Pupfish on inland sand, and otherwise the ordinary goldfish.
///
/// The Pupfish's own gate is the surprise here, and it is the opposite of what its name suggests:
/// `spawnTileX > beachDistance && spawnTileX < maxTilesX - beachDistance` is *away* from the shore,
/// so a Pupfish is a desert-oasis fish rather than a seaside one.
const PUPFISH: u16 = 607;
const GOLDFISH: u16 = 55;
const GOLD_GOLDFISH: u16 = 592;

/// The rain's own grass critters (`NPC.cs:2381-2407`). The Goldfish Walker is a goldfish out of
/// water: the game only ever spawns one while it is raining, which is the whole of its condition.
/// The [`WORM`] that shares the arm with it is the same one the underground arms draw.
const GOLDFISH_WALKER: u16 = 230;
const GOLD_GOLDFISH_WALKER: u16 = 593;
const GOLD_WORM: u16 = 448;

/// The Butterfly (`NPCID.Butterfly`), which [`friendly_pool`] already draws, and what a windy day
/// turns it into.
const BUTTERFLY: u16 = 356;
const LADY_BUG: u16 = 604;
const GOLD_LADY_BUG: u16 = 605;

/// The Gold Dragonfly (`NPCID.GoldDragonfly`) and the six ordinary ones `RollDragonflyType`
/// (`NPC.cs:5526-5533`) answers with, split by the ground the cattail grows out of: the sand three
/// and the grass three.
const GOLD_DRAGONFLY: u16 = 601;
const SAND_DRAGONFLIES: [u16; 3] = [595, 598, 600];
const GRASS_DRAGONFLIES: [u16; 3] = [596, 597, 599];

/// `TileID.Cattail` (519), the one tile the dragonfly arm hangs off. This server's own worldgen does
/// not grow cattails yet, so this arm answers only in a world generated by the game itself and
/// loaded here, which is the ordinary case for this server.
const CATTAIL: u16 = 519;

/// The Gnome (`NPCID.Gnome`), which is not a water critter at all whatever a grouping by id
/// suggests: it is a living-tree and dirt-cave find, and it turns to stone in daylight
/// (`NPC.AI_003_Gnomes_ShouldTurnToStone`, `NPC.cs:56413`).
const GNOME: u16 = 624;

/// `NPC.GetGnomeChance()` (`NPC.cs:5381-5404`), which is a flat 10 in an ordinary world: the two
/// multipliers it can take are `WorldGen.Skyblock.lowTiles` and `Main.remixWorld`, neither of which
/// this server models.
const GNOME_CHANCE: u32 = 10;

/// The walls a Gnome will come out of underground (`NPC.cs:5783`, written there as
/// `wall != 2 && wall != 63 && (uint)(wall - 196) > 3u`): `WallID.DirtUnsafe`, `GrassUnsafe` and
/// `DirtUnsafe1` through `DirtUnsafe4` (`WallID.cs:73`, `:195`, `:461-467`). All six are natural
/// cave walls, so a dirt-walled cavern is a gnome's and a player-built room is not.
const GNOME_WALLS: [u16; 6] = [2, 63, 196, 197, 198, 199];

/// `TileID.GolfGrassHallowed`, the fourth of the grasses in the game's own grass `case`
/// (`NPC.cs:2375-2378`, alongside [`GRASS`], [`GOLF_GRASS`] and [`HALLOWED_GRASS`]).
const GOLF_GRASS_HALLOWED: u16 = 492;

/// `TileID.SnowBlock` and `TileID.IceBlock`, which have a `case` of their own above the grass one
/// (`NPC.cs:2328-2337`) and so never reach it.
const SNOW_BLOCK: u16 = 147;
const ICE_BLOCK: u16 = 161;

/// The scan every water arm makes for the top of the column it is standing in, and the row two
/// below that (`NPC.cs:2120-2143`; the same loop, minus the second row, at `:1940-1953`,
/// `:2014-2025` and `:2224-2236`):
///
/// ```csharp
/// for (int num30 = spawnTileY - 1; num30 > spawnTileY - 50; num30--)
/// {
///     if (Main.tile[spawnTileX, num30].liquid == 0 && !WorldGen.SolidTile(spawnTileX, num30)
///         && !WorldGen.SolidTile(spawnTileX, num30 + 1) && !WorldGen.SolidTile(spawnTileX, num30 + 2))
///     {
///         num28 = num30 + 2;
///         if (!WorldGen.SolidTile(spawnTileX, num28 + 1) && !WorldGen.SolidTile(spawnTileX, num28 + 2))
///             num29 = num28 + 2;
///         break;
///     }
/// }
/// if (num28 > spawnTileY) num28 = spawnTileY;
/// if (num29 > spawnTileY) num29 = spawnTileY;
/// ```
///
/// This is the whole reason the critter arms could not live in [`water_pick`]: they spawn at a row
/// the candidate did not name. `num28` is where a Seagull lands and a Sea Turtle floats, `num29` two
/// rows under it where a Dolphin and a Seahorse swim.
///
/// The `num29` test reads the *unclamped* `num28`, which is why the clamp is applied to each answer
/// at the end rather than to `num28` as it is found. Both rows are computed here even for the four
/// call sites that only want the first; that is two extra tile reads on a branch already gated
/// behind daylight, a water tile and a one-in-three roll.
fn water_surface(world: &World, x: i32, ground_y: i32) -> (Option<i32>, Option<i32>) {
    for row in (ground_y - 49..=ground_y - 1).rev() {
        if world.tile(x, row).liquid == 0
            && !solid_tile(world, x, row)
            && !solid_tile(world, x, row + 1)
            && !solid_tile(world, x, row + 2)
        {
            let top = row + 2;
            let below = (!solid_tile(world, x, top + 1) && !solid_tile(world, x, top + 2))
                .then_some((top + 2).min(ground_y));
            return (Some(top.min(ground_y)), below);
        }
    }
    (None, None)
}

/// `NPC.RollDragonflyType(tileType)` (`NPC.cs:5526-5533`): the sand three on sand, the grass three
/// on anything else.
fn dragonfly_type(ground_block: u16, rng: &mut SmallRng) -> u16 {
    let list = if ground_block == SAND {
        &SAND_DRAGONFLIES
    } else {
        &GRASS_DRAGONFLIES
    };
    list[rng.random_range(0..list.len())]
}

/// `NPC.FindCattailTop(landX, landY, out cattailX, out cattailY)` (`NPC.cs:81004-81043`).
///
/// A 61-by-41 sweep for a cattail *top* (tile 519 with `frameX >= 180`), picking one of however many
/// it finds by reservoir sampling: `Main.rand.Next(num) == 0` with `num` starting at 1 and rising by
/// one per hit, so the first found is always taken and each later one replaces it with probability
/// `1/n`. The game returns false when nothing moved off the point it was given.
///
/// This is the one expensive read in the friendly chain, 2501 tiles. It sits behind six cheaper
/// tests in vanilla's own order (the ground tile, the wind, the rain, daylight, a one-in-two roll
/// and the surface line), so it is reached only on a rainy afternoon, on grass or sand, at roughly
/// one spawn attempt in two of the small fraction that get that far.
fn find_cattail_top(world: &World, x: i32, y: i32, rng: &mut SmallRng) -> Option<(i32, i32)> {
    let mut found = None;
    let mut seen = 1u32;
    for i in x - 30..=x + 30 {
        for j in y - 20..=y + 20 {
            let tile = world.tile(i, j);
            if tile.is_active()
                && tile.block == CATTAIL
                && tile.frame_x >= 180
                && rng.random_range(0..seen) == 0
            {
                found = Some((i, j));
                seen += 1;
            }
        }
    }
    found
}

/// What a friendly attempt draws before [`friendly_pool`] is reached at all: vanilla's own chains
/// inside `else if (spawnFriendly)` (`NPC.cs:2099-2407`), each of which `return`s or `break`s and so
/// answers for the whole attempt.
///
/// `None` means no chain here answered and the caller should fall through to [`friendly_pool`].
/// `Some` of an empty list means a chain answered with nothing, which vanilla really does: the beach
/// arm's Sea Turtle and Dolphin draws each need a water row that may not exist, and when it does not
/// the `switch (num31)` has no `case` for them and the attempt is spent.
///
/// Positions are returned rather than types alone because that is the whole gap these arms sat in:
/// a Seagull is placed on the top of the water and a water strider a row above that, neither of
/// which is the row the candidate was chosen at. Vanilla's `spawnTileY` is the solid ground row,
/// which is this server's spawn row plus one, so a vanilla row `r` is pushed here as `r - 1`.
///
/// Four arms of the branch are deliberately not here, each for its own reason:
///
/// * **The graveyard's Maggot and Rat** (`:2101-2115`), which the caller already answers with
///   [`GRAVEYARD_VERMIN`] ahead of this.
/// * **The whole `switch (tileType)` below the rain arm** (`:2409-2624`): the fireflies, the owl,
///   the songbirds, the penguins, the scorpions, the macaws and the bunnies are [`friendly_pool`],
///   which this returns `None` into. The one thing taken out of it here is the rain arm at `:2381`,
///   because that arm `break`s before any of them and so is not a variation on the pool but a
///   replacement for it.
/// * **The two `deeperThanRockLayer` gem arms** at the head of the rain arm (`:2383-2390`,
///   `GetGemSquirrelToSpawn` and `GetGemBunnyToSpawn`). Those are a roster of their own and not this
///   lane's; the effect of leaving them out is that below the rock layer in the rain a Worm or a
///   Goldfish Walker is slightly commoner here than in the game, never rarer. Above the rock layer,
///   which is where a Goldfish Walker is actually met, both arms are closed anyway.
/// * **`butterflyChance` and `stinkBugChance`** (`NPC.setFireFlyChance`, `NPC.cs:94495-94541`), a
///   pair of per-world rolls that decide whether a world has butterflies or stink bugs at all.
///   [`friendly_pool`] already flattens the butterfly's own rate into a uniform draw, so the
///   ladybug swap in [`spawn_butterfly`] inherits that same disclosed narrowing rather than adding
///   a new one.
#[allow(clippy::too_many_arguments)]
pub fn friendly_chain(
    world: &World,
    x: i32,
    ground_y: i32,
    ground_block: u16,
    wet: bool,
    x_range: bool,
    wind: f32,
    luck: f32,
    rng: &mut SmallRng,
) -> Option<Vec<(u16, (f32, f32))>> {
    let surface = f64::from(world.surface);
    let spawn_tile_y = f64::from(ground_y);
    // Vanilla's `spawnTileY` is the solid ground row; this server's spawn row is the one above it.
    let at = |row: i32| (x as f32 * 16.0, (row - 1) as f32 * 16.0);

    // `NPC.TooWindyForButterflies` (`NPC.cs:6909`) and the unnamed `flag` at `:1294`, which is a
    // second, stronger wind threshold used only to keep water striders off choppy water. The game
    // writes the second as `windSpeedTarget < -0.45 || windSpeedTarget > 0.45`, which is the same
    // test as being outside the closed band and is written that way here for clippy's sake.
    let too_windy = wind.abs() >= 0.4;
    let choppy = !(-0.45..=0.45).contains(&wind);

    // `NPC.cs:2116-2199`, the beach. `isBeach` (`:1206`) is position only, so a dry beach tile is
    // answered here too and always with a Seagull.
    let is_beach =
        spawn_tile_y <= surface && (x < BEACH_DISTANCE || x > world.width() - BEACH_DISTANCE);
    if !x_range && is_beach {
        if !wet {
            return Some(vec![(SEAGULL, at(ground_y))]);
        }
        // `:2122`, which is `<` against the surface line where `isBeach` above was `<=`.
        let (top, below) = if spawn_tile_y < surface && ground_y > 50 {
            water_surface(world, x, ground_y)
        } else {
            (None, None)
        };
        if rng.random_range(0..2) == 0 {
            return Some(match rng.random_range(0..4) {
                // `:2148-2157`. Both of these need their row to exist; with no water surface in
                // reach the `switch` below them has no arm and the attempt spawns nothing.
                0 => top
                    .map(|row| vec![(SEA_TURTLE, at(row))])
                    .unwrap_or_default(),
                1 => below
                    .map(|row| vec![(DOLPHIN, at(row))])
                    .unwrap_or_default(),
                // `:2160-2186`. These two fall back to the candidate's own row instead.
                2 => {
                    let seahorse = if rolls_lucky(luck, GOLD_CRITTER_CHANCE, rng) {
                        GOLD_SEAHORSE
                    } else {
                        SEAHORSE
                    };
                    vec![(seahorse, at(below.unwrap_or(ground_y)))]
                }
                _ => vec![(PUFFERFISH, at(below.unwrap_or(ground_y)))],
            });
        }
        // `:2189-2192`. The `!xRange` vanilla repeats here is already the arm's own gate.
        return Some(top.map(|row| vec![(SEAGULL, at(row))]).unwrap_or_default());
    }

    // `NPC.cs:2200-2218`, the cattail dragonflies. A rainy, still afternoon over grass or sand, and
    // then a real cattail top somewhere in a 61-by-41 box around the candidate.
    if matches!(ground_block, GRASS | GOLF_GRASS | SAND)
        && !too_windy
        && world.raining
        && world.day_time
        && rng.random_range(0..2) == 0
        && spawn_tile_y <= surface
        && let Some((cattail_x, cattail_y)) = find_cattail_top(world, x, ground_y, rng)
    {
        let px = cattail_x as f32 * 16.0;
        let py = (cattail_y - 1) as f32 * 16.0;
        let lead = if rolls_lucky(luck, GOLD_CRITTER_CHANCE, rng) {
            GOLD_DRAGONFLY
        } else {
            dragonfly_type(ground_block, rng)
        };
        let mut swarm = vec![(lead, (px, py))];
        if rng.random_range(0..3) == 0 {
            swarm.push((dragonfly_type(ground_block, rng), (px - 16.0, py)));
        }
        if rng.random_range(0..3) == 0 {
            swarm.push((dragonfly_type(ground_block, rng), (px + 16.0, py)));
        }
        return Some(swarm);
    }

    // `NPC.cs:2220-2325`, standing in water. Terminal: every path through it spawns something.
    if wet {
        if spawn_tile_y <= surface
            && ground_y > 50
            && rng.random_range(0..3) != 0
            && world.day_time
            && let (Some(row), _) = water_surface(world, x, ground_y)
            && !x_range
        {
            // `:2239-2297`, keyed by the tile the water sits on. The water striders are a group of
            // one to three, each scattered up to a tile either side and placed a row above the
            // water's own surface (`num34 * 16 - 16`), which is what standing *on* the water looks
            // like from the tile grid.
            let striders = |rng: &mut SmallRng| {
                (0..rng.random_range(1..4))
                    .map(|_| {
                        let strider = if rolls_lucky(luck, GOLD_CRITTER_CHANCE, rng) {
                            GOLD_WATER_STRIDER
                        } else {
                            WATER_STRIDER
                        };
                        let jitter = rng.random_range(-16..=16) as f32;
                        (strider, (at(row).0 + jitter, at(row).1 - 16.0))
                    })
                    .collect()
            };
            return Some(match ground_block {
                JUNGLE_GRASS => {
                    if rng.random_range(0..3) != 0 && !choppy && !world.raining {
                        striders(rng)
                    } else {
                        vec![(TURTLE_JUNGLE, at(row))]
                    }
                }
                SAND => {
                    if rng.random_range(0..3) != 0 && !choppy && !world.raining {
                        striders(rng)
                    } else {
                        vec![(GREBE, at(row))]
                    }
                }
                _ => {
                    if rng.random_range(0..5) == 0 && matches!(ground_block, GRASS | GOLF_GRASS) {
                        vec![(TURTLE, at(row))]
                    } else if rng.random_range(0..2) == 0 {
                        vec![(DUCK, at(row))]
                    } else {
                        vec![(DUCK_WHITE, at(row))]
                    }
                }
            });
        }
        // `:2299-2310` and `:2312-2323`, which vanilla writes out twice with identical bodies: once
        // for "there was no water surface to stand on" and once for "it was night, or too deep, or
        // the one-in-three declined". They are the same three arms, so they are written once here.
        return Some(vec![(
            if ground_block == SAND && x > BEACH_DISTANCE && x < world.width() - BEACH_DISTANCE {
                PUPFISH
            } else if rolls_lucky(luck, GOLD_CRITTER_CHANCE, rng) {
                GOLD_GOLDFISH
            } else {
                GOLDFISH
            },
            at(ground_y),
        )]);
    }

    // `NPC.cs:2381-2407`, the rain. This is the head of the grass `case` and it `break`s, so while
    // it is raining nothing below it in that `switch` runs at all: no fireflies, no owl, no
    // songbirds, no butterflies. The `case` is reached by the four grasses outright and by anything
    // else below the surface line (`:2371-2378`, `default: if (spawnTileY > worldSurface) goto
    // case 2`); snow, ice, jungle grass and sand each have a `case` of their own above it.
    let grass_case = matches!(
        ground_block,
        GRASS | HALLOWED_GRASS | GOLF_GRASS | GOLF_GRASS_HALLOWED
    ) || (!matches!(ground_block, SNOW_BLOCK | ICE_BLOCK | JUNGLE_GRASS | SAND)
        && spawn_tile_y > surface);
    if grass_case && world.raining && ground_y <= world.height() - UNDERWORLD_DEPTH {
        return Some(vec![(
            if rolls_lucky(luck, GOLD_CRITTER_CHANCE, rng) {
                GOLD_WORM
            } else if rng.random_range(0..3) != 0 {
                WORM
            } else if rolls_lucky(luck, GOLD_CRITTER_CHANCE, rng) {
                GOLD_GOLDFISH_WALKER
            } else {
                GOLDFISH_WALKER
            },
            at(ground_y),
        )]);
    }

    None
}

/// What a butterfly draw actually answers with (`NPC.cs:2487-2534`):
///
/// ```csharp
/// if ((!tooWindyForButterflies && !raining && Main.dayTime && Main.rand.Next(butterflyChance) == 0) & flag10)
/// { ... 444 / 356 ... break; }
/// if ((tooWindyForButterflies && !raining && Main.dayTime && Main.rand.Next(butterflyChance / 2) == 0) & flag10)
/// {
///     if (RollLuck(goldCritterChance) == 0) { SpawnNPC(..., 605); } else { SpawnNPC(..., 604); }
///     ... four more 604 ...
///     break;
/// }
/// ```
///
/// The two arms are mutually exclusive on the same flag and sit next to each other in the same
/// grass `case`, so a windy day does not add ladybugs to a world, it *replaces* the butterflies with
/// them. `TooWindyForButterflies` is `Math.Abs(Main.windSpeedTarget) >= 0.4f` (`NPC.cs:6909`).
///
/// Two things are deliberately not modelled, and neither is new debt. The windy arm's rate is twice
/// the calm one's (`butterflyChance / 2`), which [`friendly_pool`]'s uniform draw already flattens
/// for the butterfly itself; and the four extra ladybugs the arm scatters around the first are left
/// out for the same reason [`friendly_pool`] drops the butterfly's own two, so a windy meadow is
/// thinner here than in the game rather than differently populated. The Gold Butterfly (444) is the
/// calm arm's own `RollLuck` and belongs to the ordinary-critter roster rather than this one.
fn spawn_butterfly(too_windy: bool, luck: f32, rng: &mut SmallRng) -> u16 {
    if !too_windy {
        return BUTTERFLY;
    }
    if rolls_lucky(luck, GOLD_CRITTER_CHANCE, rng) {
        GOLD_LADY_BUG
    } else {
        LADY_BUG
    }
}

/// The three fairy critters, `NPCID.FairyCritterPink` and the Green and Blue that follow it.
const FAIRY_CRITTER_PINK: u16 = 583;

/// One in five hundred, `CheckToSpawnUndergroundFairy`'s own base odds (`NPC.cs:5824`).
const FAIRY_CHANCE: u32 = 500;

/// `NPC.Spawner.CheckToSpawnUndergroundFairy` (`NPC.cs:5820-5848`):
///
/// ```csharp
/// if (!fairyLog) return false;
/// int num = 500;
/// if (Main.tenthAnniversaryWorld && !Main.getGoodWorld) num = 250;
/// if (Main.hardMode) num = (int)((float)num * 1.66f);
/// if (RollLuck(num) != 0) return false;
/// if ((double)spawnTileY < (Main.worldSurface + Main.rockLayer) / 2.0
///     || spawnTileY >= Main.maxTilesY - 300) return false;
/// if (AnyHelpfulFairies()) return false;
/// return true;
/// ```
///
/// This is the *underground* fairy, and the whole of what it asks is: the world has a fallen log
/// somewhere in it, the spot is below the halfway line between the surface and the rock layer but
/// clear of the bottom three hundred rows, one draw in five hundred came up, and there is not
/// already a fairy out doing this. It has no biome gate, no hour gate and no progression gate: a
/// player digging deep on their first day can meet one.
///
/// `fairy_log` is `NPC.Spawner.fairyLog` (`NPC.cs:150`), which the spawner only ever reads.
/// `MysticLogFairiesEvent.ScanWholeOverworldForLogs` owns every write to it, and the caller keeps
/// it: see `game/server/systems.rs`'s own `scan_for_fallen_logs`.
///
/// Hardmode makes a fairy *rarer*, not commoner: `(int)(500f * 1.66f)` is exactly 830 at `f32`
/// (the product rounds to 830.0 rather than to 829.99998, so the truncation costs nothing).
///
/// `Main.tenthAnniversaryWorld` is real state in this server (`world.secret_seeds.tenth_anniversary`,
/// already read by the Big Mimic AI) but the spawner has no such flag on `EventSpawns` to read it
/// through, so the 250 arm drops and the base is always 500 until one is plumbed here. `RollLuck(num)`
/// is `Main.rand.Next(num)` at luck zero (`Luck.cs:5-16`), the narrowing every other `Roll*Luck` site
/// in this file already makes.
///
/// `any_helpful_fairies` is `NPC.AnyHelpfulFairies()` (`NPC.cs:90954-90964`): a live 583, 584 or
/// 585 whose `ai[2] > 1f`, which is every state past the two drifting ones (`game::ai::fairy`'s own
/// `state` module). It is passed as a closure and asked last, so a declined roll never walks the
/// NPC table.
fn underground_fairy(
    world: &World,
    fairy_log: bool,
    hard_mode: bool,
    ground_y: i32,
    any_helpful_fairies: &dyn Fn() -> bool,
    luck: f32,
    rng: &mut SmallRng,
) -> bool {
    if !fairy_log {
        return false;
    }
    let chance = if hard_mode {
        (FAIRY_CHANCE as f32 * 1.66) as u32
    } else {
        FAIRY_CHANCE
    };
    if !rolls_lucky(luck, chance, rng) {
        return false;
    }
    let halfway = (f64::from(world.surface) + f64::from(world.rock_layer)) / 2.0;
    if f64::from(ground_y) < halfway || ground_y >= world.height() - 300 {
        return false;
    }
    !any_helpful_fairies()
}

/// `NPC.Spawner.CheckToSpawnUndergroundGnomes` (`NPC.cs:5747-5787`):
///
/// ```csharp
/// if (!isAValidZoneAndTile) return false;
/// if (Main.eclipse || Main.bloodMoon) return false;
/// if (RollLuck(gnomeChance) != 0) return false;
/// double num = Main.worldSurface * 0.800000011920929;
/// double num2 = Main.worldSurface * 1.100000023841858;
/// if ((double)spawnTileY < num || (double)spawnTileY > num2) return false;
/// if (CountNPCS(624) > Main.rand.Next(3)) return false;
/// Tile tile = Main.tile[spawnTileX, spawnTileY];
/// if (Main.dayTime && tile.wall <= 0) return false;
/// ushort wall = tile.wall;
/// if (wall != 2 && wall != 63 && (uint)(wall - 196) > 3u) return false;
/// return true;
/// ```
///
/// A narrow band around the surface line, a dirt cave wall, a one-in-ten roll, and at most two or
/// three gnomes in the world at once. The two `0.8`/`1.1` constants are written out at the width the
/// decompiler prints them, which is what a `float` literal widens to.
///
/// `isAValidZoneAndTile` is the caller's, and is exactly `!ZoneCorrupt && !ZoneCrimson && !waterTile`
/// (`NPC.cs:3629`). `Main.eclipse` and `Main.bloodMoon` are read off the world here rather than
/// passed, because that is where this server keeps them.
///
/// The `Main.dayTime && tile.wall <= 0` line is dead where the game writes it, and is transcribed
/// anyway rather than dropped: every wall the line below accepts is nonzero, so the only tile it
/// can turn down is one the wall set turns down a line later regardless of the hour. It is the same
/// belt-and-braces as the `!raining` on the owl's arm ([`spawn_owl`]), and it is called out here
/// because it looks from the outside like a daylight gate on gnomes and is not one.
fn underground_gnome(
    world: &World,
    x: i32,
    ground_y: i32,
    valid_zone_and_tile: bool,
    alive: &dyn Fn(u16) -> usize,
    luck: f32,
    rng: &mut SmallRng,
) -> bool {
    if !valid_zone_and_tile || world.eclipse || world.blood_moon {
        return false;
    }
    if !rolls_lucky(luck, GNOME_CHANCE, rng) {
        return false;
    }
    let surface = f64::from(world.surface);
    let spawn_tile_y = f64::from(ground_y);
    if spawn_tile_y < surface * 0.800_000_011_920_929
        || spawn_tile_y > surface * 1.100_000_023_841_858
    {
        return false;
    }
    if alive(GNOME) > rng.random_range(0..3) {
        return false;
    }
    let wall = world.tile(x, ground_y).wall;
    if world.day_time && wall == 0 {
        return false;
    }
    GNOME_WALLS.contains(&wall)
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
    let floor = world.tile(x, y + 1);
    open_space(world, x, y) && floor.is_active() && solid(floor.block)
}

/// The open-space half alone, which is all `HasTileSpawnSpace` (`NPC.cs:5406-5413`) ever asks for.
///
/// A flier chosen in the sky has nothing under it and never will, so the floor test in
/// [`has_room`] is the one thing that must not apply to it. The game's own check is over a
/// `(x - 1, y - 3, 2, 3)` rectangle and never mentions the ground: the ground requirement is
/// implicit in the *other* branch, the one that walks down to it.
fn open_space(world: &World, x: i32, y: i32) -> bool {
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
    true
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

/// `TileID.Sets.Platforms` (`TileID.cs:243`), which is exactly seven tiles and is *not* the same
/// set as `Main.tileSolidTop` ([`terrustia_proto::tile_solid::solid_top`], which has far more in
/// it). Only [`solid_tile2`] needs the distinction.
const PLATFORMS: [u16; 7] = [19, 427, 435, 436, 437, 438, 439];

/// `WorldGen.SolidTile2(i, j)` (`WorldGen.cs:71031-71048`), the looser of the game's two
/// "is this solid" tests:
///
/// ```csharp
/// if (Main.tile[i, j].active() && Main.tileSolid[Main.tile[i, j].type]
///     && ((TileID.Sets.Platforms[Main.tile[i, j].type]
///          && (Main.tile[i, j].halfBrick() || Main.tile[i, j].topSlope()))
///         || Main.tile[i, j].slope() == 0)
///     && !Main.tile[i, j].halfBrick() && !Main.tile[i, j].inActive())
/// ```
///
/// Where `SolidTile` throws every platform out (`!Main.tileSolidTop[...]`, and see
/// `worldgen::traps`'s copy of it), this one keeps a platform that has been hammered into a top
/// slope. The trailing `!halfBrick()` makes the `halfBrick()` inside the platform clause dead, so
/// that clause reduces to "a platform with a top slope"; `topSlope()` is `slope() == 1 || slope()
/// == 2` (`Tile.cs:292-298`), and `inActive()` is the actuated bit.
///
/// The game's own `Main.tile[i, j] == null` arm returns *true*, counting off the map as solid.
/// `World::tile` answers off the map with bare air instead, which is the opposite answer; nothing
/// reaches it, because [`try_spawn`] drops any candidate within ten tiles of an edge long before
/// this is asked.
fn solid_tile2(world: &World, x: i32, y: i32) -> bool {
    let tile = world.tile(x, y);
    tile.is_active()
        && solid(tile.block)
        && (tile.slope == 0 || (PLATFORMS.contains(&tile.block) && matches!(tile.slope, 1 | 2)))
        && !tile.flags.has(terrustia_proto::TileFlags::HALF_BRICK)
        && !tile.flags.has(terrustia_proto::TileFlags::ACTUATED)
}

/// `NPC.IsThisAGoodPlaceForAStatueMimic(x, y)` (`NPC.cs:43891-43898`): two solid tiles side by side
/// with three clear rows above both of them, which is the shape of the plinth a statue stands on.
///
/// ```csharp
/// if (WorldGen.SolidTile2(x, y) && WorldGen.SolidTile2(x + 1, y)
///     && !Main.tile[x, y - 1].active() && !Main.tile[x, y - 2].active()
///     && !Main.tile[x, y - 3].active() && !Main.tile[x + 1, y - 1].active()
///     && !Main.tile[x + 1, y - 2].active() && !Main.tile[x + 1, y - 3].active())
/// ```
///
/// It is the same function the mimic's own AI uses to pick where to reappear
/// (`AI_126_StatueMimic`, `NPC.cs:43994`), which is why it wants two columns rather than one: the
/// thing is 28 pixels wide and has to look like it was always there.
///
/// `ground_y` is the game's own `spawnTileY`, the solid row itself, which this server's spawn row
/// sits one above; the caller passes `y + 1`, the same convention as [`sandstone_check`]. The air
/// test is `active()` rather than `nactive()`, so an actuated tile still counts as being in the
/// way, which is transcribed as-is.
pub fn good_place_for_a_statue_mimic(world: &World, x: i32, ground_y: i32) -> bool {
    solid_tile2(world, x, ground_y)
        && solid_tile2(world, x + 1, ground_y)
        && (1..=3).all(|dy| {
            !world.tile(x, ground_y - dy).is_active()
                && !world.tile(x + 1, ground_y - dy).is_active()
        })
}

/// `WorldGen.SolidTile(i, j, noDoors = false)` (`WorldGen.cs:70650-70671`), the stricter of the
/// game's two "is this solid" tests:
///
/// ```csharp
/// if (Main.tile[i, j].active() && Main.tileSolid[Main.tile[i, j].type]
///     && !Main.tileSolidTop[Main.tile[i, j].type] && !Main.tile[i, j].halfBrick()
///     && Main.tile[i, j].slope() == 0 && !Main.tile[i, j].inActive())
/// ```
///
/// Where [`solid_tile2`] keeps a platform that has been hammered into a top slope, this one throws
/// every platform out outright, and it refuses any slope rather than only a non-top one. `noDoors`
/// is false at the one call site here ([`check_to_spawn_rock_golem`]), so the door carve-out is not
/// transcribed. `worldgen::smooth` has its own copy of the same function for its own pass; this is
/// a second one rather than a shared helper because that one is `pub(super)` inside `worldgen`.
///
/// The game's `Main.tile[i, j] == null` arm returns *true*, counting off the map as solid;
/// `World::tile` answers off the map with bare air, the opposite. Nothing reaches it, for the same
/// reason [`solid_tile2`] does not: [`try_spawn`] drops any candidate within ten tiles of an edge.
fn solid_tile(world: &World, x: i32, y: i32) -> bool {
    let tile = world.tile(x, y);
    tile.is_active()
        && solid(tile.block)
        && !terrustia_proto::tile_solid::solid_top(tile.block)
        && !tile.flags.has(terrustia_proto::TileFlags::HALF_BRICK)
        && tile.slope == 0
        && !tile.flags.has(terrustia_proto::TileFlags::ACTUATED)
}

/// `NPC.CheckToSpawnRockGolem(spawnTileX, spawnTileY, spawnTileType)` (`NPC.cs:5803-5818`):
///
/// ```csharp
/// if (!Main.hardMode || (spawnTileType != 1 && !TileID.Sets.Conversion.Moss[spawnTileType]) || ZoneSnow)
///     return false;
/// if (Main.rand.Next(50) != 0)
///     return false;
/// if (WorldGen.SolidTile(spawnTileX - 1, spawnTileY - 4) || WorldGen.SolidTile(spawnTileX, spawnTileY - 4) || WorldGen.SolidTile(spawnTileX + 1, spawnTileY - 4))
///     return false;
/// return true;
/// ```
///
/// Four clauses and no state: hardmode, plain stone or any moss underfoot, not in the snow, one
/// attempt in fifty, and three tiles of clear ceiling four rows up so there is somewhere for a
/// thing this tall to stand. `TileID.Sets.Conversion.Moss` (`TileID.cs:38`) is
/// [`terrustia_proto::tile_sets::is_moss`], which already holds all eleven of its entries.
///
/// The clause order is the game's own, which is also cheapest-first: three comparisons, then the
/// roll, and only then the three tile reads.
///
/// `ground_y` is the game's `spawnTileY`, the solid row itself, which this server's spawn row sits
/// one above; the caller passes `y + 1`, the same convention as [`good_place_for_a_statue_mimic`]
/// and [`sandstone_check`].
fn check_to_spawn_rock_golem(
    world: &World,
    x: i32,
    ground_y: i32,
    ground_block: u16,
    hard_mode: bool,
    snow: bool,
    rng: &mut SmallRng,
) -> bool {
    if !hard_mode
        || (ground_block != STONE && !terrustia_proto::tile_sets::is_moss(ground_block))
        || snow
    {
        return false;
    }
    if rng.random_range(0..ROCK_GOLEM_ODDS) != 0 {
        return false;
    }
    !(-1..=1).any(|dx| solid_tile(world, x + dx, ground_y - ROCK_GOLEM_HEADROOM))
}

/// `WorldGen.oceanDistance` (`WorldGen.cs:4069`), how far in from either edge of the map the
/// goldfish arm's own bounds test reaches (`NPC.cs:1999`).
const OCEAN_DISTANCE: i32 = 250;

/// `TileID.JungleGrass`, vanilla's `tileType == 60` on the Piranha arm (`NPC.cs:1932`).
const JUNGLE_GRASS: u16 = 60;

/// What a tile of water draws instead of the land pool.
///
/// Vanilla keeps the water rosters in their own `waterTile` branches ahead of every land branch
/// (`NPC.cs:1766-2085`), which is why a shark is never on the sand and a zombie is never in the sea.
/// This is that ladder, arm for arm and roll for roll.
///
/// `None` means every arm of it declined. Vanilla's chain is a plain `else if` ladder, so a decline
/// falls *through* to the land chain below rather than dropping the attempt, and the caller here
/// does the same. The old `&'static [u16]` shape could not express that and got it backwards twice
/// over: below the surface line it answered a Blue Jellyfish on every single attempt where the game
/// rolls one in three and lets the other two reach land, and it had nowhere to put the seven arms
/// that need hardmode, a ground tile or an evil biome to read. Those seven are the whole aquatic
/// combat roster below the ocean: Arapaima, Blood Jelly, Blood Feeder, Piranha, Angler Fish, Green
/// Jellyfish, and the two evils' goldfish.
///
/// `ground_y` is the game's `spawnTileY`, the solid row itself, which this server's spawn row sits
/// one above; the caller passes `y + 1`, the same convention as [`good_place_for_a_statue_mimic`],
/// [`sandstone_check`] and [`check_to_spawn_rock_golem`].
///
/// Deliberately left out, each for a reason and none of them "no AI for it":
///
/// * **Every critter arm inside the chain**, because each one spawns at a *different row* from the
///   candidate: vanilla scans up to the water's own surface (`num12`, `num14`, `num16`, `num17`,
///   `num22`, `num25`) and puts the critter there, and this producer answers a type for the row it
///   was asked about with nowhere to put a different one. That is the Seagull and the four-way
///   group in the ocean (`NPC.cs:1855-1910`), the Jungle Turtle and the water striders at
///   `:1935-1974`, and the turtle, grebe, pupfish and ducks at `:2009-2073`. Their rolls are not
///   made here, so the combat types they sit above are slightly commoner than in the game and never
///   rarer.
///
///   Every one of those critters is reachable now, through [`friendly_chain`] rather than through
///   here: vanilla writes the same roster twice, once in these combat arms and once in its own
///   `spawnFriendly` branch (`NPC.cs:2116-2325`), and the friendly copy is the one that returns a
///   position as well as a type. What stays out is this copy of it and only this copy.
/// * **The Angler (376)** at `:1778`, `:1801` and `:1928`, a rescue rather than a roster.
/// * **The gold critter variants** (592 Gold Goldfish) *in this function*, whose ordinary siblings
///   here are answered without their `RollLuck(goldCritterChance)`; [`spawn_frog`] carries the same
///   disclosure for the Gold Frog. [`friendly_chain`] does make those rolls, so the class is
///   modelled, just not on these arms.
/// * **`SharkSpawnChance`** (`NPC.cs:5558-5576`) returns 10 unless a Chum Bucket projectile (type
///   820) is floating in reach, and this server has no such projectile, so the constant is the
///   whole of it.
///
/// One narrowing that is the caller's rather than this function's: vanilla's `spawnFriendly` arm
/// sits at `:2099`, *below* every water arm, so in the game a friendly attempt standing in a
/// hardmode jungle lake still draws an Arapaima. This server answers `spawn_friendly` above the
/// water arm, so the four water arms that carry no `!spawnFriendly` of their own (`:1766`, `:1770`,
/// `:1774`, `:1999`) are reached only on the monster path and are that much rarer here. The other
/// arms are unaffected: `:1932` and `:1988` test `!spawnFriendly` themselves.
#[allow(clippy::too_many_arguments)]
pub fn water_pick(
    world: &World,
    x: i32,
    ground_y: i32,
    biome: Biome,
    hard_mode: bool,
    ground_block: Option<u16>,
    rng: &mut SmallRng,
) -> Option<u16> {
    // The game's own two comparisons, written the way it writes them: `spawnTileY` against
    // `Main.worldSurface` as a `double`, and `deeperThanRockLayer = spawnTileY >= Main.rockLayer`
    // (`NPC.cs:1204`).
    let surface = f64::from(world.surface);
    let spawn_tile_y = f64::from(ground_y);
    let deeper_than_rock_layer = ground_y >= i32::from(world.rock_layer);

    // `NPC.cs:1766`. Two attempts in three, and the arm carries no depth gate whatsoever: a
    // hardmode jungle's water is an arapaima's wherever in the column it sits.
    if hard_mode && biome == Biome::Jungle && rng.random_range(0..3) != 0 {
        return Some(157); // Arapaima
    }
    // `NPC.cs:1770-1777`. Two consecutive arms with the identical condition, so the jelly takes two
    // thirds and the feeder two thirds of the third that is left.
    if hard_mode && biome == Biome::Crimson {
        if rng.random_range(0..3) != 0 {
            return Some(242); // BloodJelly
        }
        if rng.random_range(0..3) != 0 {
            return Some(241); // BloodFeeder
        }
    }
    // `NPC.cs:1798-1926`, the ocean's own chain, and it is terminal: every path through it spawns
    // something, so an ocean never falls through to the arms below.
    if biome == Biome::Ocean {
        /// `SharkSpawnChance` with no chum in the water (`NPC.cs:5558-5576`).
        const SHARK_CHANCE: u32 = 10;
        if rng.random_range(0..SHARK_CHANCE) == 0 {
            return Some(65); // Shark
        }
        if hard_mode && rng.random_range(0..SHARK_CHANCE) == 0 {
            return Some(692); // Orca
        }
        if rng.random_range(0..40) == 0 {
            return Some(220); // SeaSnail
        }
        if rng.random_range(0..18) == 0 {
            return Some(221); // Squid
        }
        if rng.random_range(0..3) == 0 {
            return Some(67); // Crab
        }
        return Some(64); // PinkJellyfish
    }
    // `NPC.cs:1932-1986`. Half of every attempt below the rock layer, and *every* attempt standing
    // on jungle grass whatever the depth, which is what puts piranhas in a surface jungle pool.
    if (deeper_than_rock_layer && rng.random_range(0..2) == 0) || ground_block == Some(JUNGLE_GRASS)
    {
        return Some(if hard_mode && rng.random_range(0..3) > 0 {
            102 // AnglerFish
        } else {
            58 // Piranha
        });
    }
    // `NPC.cs:1988-1997`. One attempt in three anywhere below the surface line; the other two fall
    // through to the arm below it and then to land.
    if spawn_tile_y > surface && rng.random_range(0..3) == 0 {
        return Some(if hard_mode && rng.random_range(0..3) > 0 {
            103 // GreenJellyfish
        } else {
            63 // BlueJellyfish
        });
    }
    // `NPC.cs:1999-2085`, the goldfish arm, which is where both evils keep their own. The roll is
    // made before the bounds test, as vanilla writes it.
    if rng.random_range(0..4) == 0
        && ((x > OCEAN_DISTANCE && x < world.width() - OCEAN_DISTANCE)
            || spawn_tile_y > surface + 50.0)
    {
        return Some(match biome {
            Biome::Corruption => 57, // CorruptGoldfish
            Biome::Crimson => 465,   // CrimsonGoldfish
            _ => 55,                 // Goldfish
        });
    }
    None
}

/// The walls an underground desert is made of, `WallID.Sets.AllowsUndergroundDesertEnemiesToSpawn`
/// (`WallID.cs:42`): plain sandstone and hardened sand, each of their three converted forms, and
/// desert fossil.
///
/// This set, not a biome scan, is what the game's whole underground-desert roster hangs off
/// (`NPC.cs:1682`). That matters twice over. It is exact where a tile-count zone is a guess, and it
/// costs two tile reads where [`biome_at`] costs twenty thousand, so the branch it gates can be
/// tested on every candidate tile rather than once per player per tick.
const DESERT_SPAWN_WALLS: [u16; 9] = [
    187, // Sandstone
    216, // HardenedSand
    217, // CorruptHardenedSand
    218, // CrimsonHardenedSand
    219, // HallowHardenedSand
    220, // CorruptSandstone
    221, // CrimsonSandstone
    222, // HallowSandstone
    223, // DesertFossil
];

/// `WorldGen.checkUnderground` (`WorldGen.cs:10099-10144`), the other half of the underground
/// desert's gate.
///
/// Deep enough down it is simply true, high enough up simply false, and in the band between it asks
/// whether the roof is closed: a 120-by-3 strip 80 tiles above the point has to be at least 80%
/// solid tile, or the point itself has to be walled. Three of those four answers are a handful of
/// reads; only the fourth walks the strip, and see the hoist below for why the caller's own use
/// almost never reaches it.
///
/// The game wraps the whole thing in a bare `catch { return false; }` for its own out-of-bounds
/// tile access. `World::tile` answers for anything off the map already, so there is nothing here to
/// catch and no behaviour dropped by not catching it.
fn check_underground(world: &World, x: i32, y: i32) -> bool {
    /// `num`, `num2` and `num3` in the game's own order: the strip's width, how far above the point
    /// it sits, and how many rows of it are counted.
    const WIDTH: i32 = 120;
    const ABOVE: i32 = 80;
    const ROWS: i32 = 3;

    let surface = f64::from(world.surface);
    if f64::from(y) > surface + f64::from(ABOVE) {
        return true;
    }
    if f64::from(y) < surface / 2.0 {
        return false;
    }
    // The game's own test is `SolidTile(i, j) || Main.tile[x, y].wall > 0`, and its second half
    // reads the *point* rather than the strip: it does not depend on either loop variable, so when
    // it holds every one of the 360 cells counts and the answer is already true. Hoisting it is the
    // same answer, exactly, and it is what keeps this cheap where it is actually reached: the
    // caller only asks after finding a desert wall, and a desert wall is a wall.
    if world.tile(x, y).wall > 0 {
        return true;
    }
    let top = y - ABOVE;
    let left = (x - WIDTH / 2).clamp(0, (world.width() - WIDTH - 1).max(0));
    let mut closed = 0;
    for i in left..left + WIDTH {
        for j in top..top + ROWS {
            let tile = world.tile(i, j);
            if tile.is_active() && solid(tile.block) {
                closed += 1;
            }
        }
    }
    f64::from(closed) >= f64::from(WIDTH * ROWS) * 0.8
}

/// `WallID.SpiderUnsafe`, the wall a spider nest is lined with and the only thing that puts one on
/// the spawn path (`NPC.cs:1662`).
pub const SPIDER_WALL: u16 = 62;

/// The spider nest's roster, `NPC.cs:1662-1680`:
///
/// ```csharp
/// else if ((Main.tile[spawnTileX, spawnTileY].wall == 62 || spawnSpider) && CheckToSpawnSpider(...))
/// {
///     ...
///     else if (Main.hardMode && Main.rand.Next(10) != 0) 163;
///     else                                               164;
/// }
/// ```
///
/// The Wall Creeper and the Black Recluse have no other ambient spawn in the game at all: 163 and
/// 164 are the only two ids `NPC.Spawner` reaches from this arm, so with no arm the Wall Creeper was
/// unreachable outright and the Black Recluse was only reachable because [`hardmode_pool`] carried
/// it in the generic cavern list, which put recluses in every hardmode cave rather than in nests.
///
/// `CheckToSpawnSpider` (`NPC.cs:5790-5801`) is `true` outside the "not the bees" for-the-worthy
/// seed, so it is not transcribed.
///
/// Two things left to the caller, both disclosed:
///
/// * `|| spawnSpider` (`NPC.cs:1145-1176`), which widens the arm to spots merely *near* a spider
///   wall by sweeping a box of radius 5 to 15 one attempt in three. Left out for the same reason
///   `underground_desert_spot` leaves out its twin: it is up to 900 tile reads on the per-candidate
///   path to reach spots the wall test itself misses anyway, and a real nest is walled throughout.
/// * `NPC.cs:1669`, the Stylist at one dry nest tile in eight below the rock layer. She already has
///   a path here, [`bound_gate`], which reaches her through the same rescue table as every other
///   bound resident; giving her a second one would mean two mechanisms racing for the same
///   townsperson. The effect of skipping the branch is that 163 and 164 take that eighth as well,
///   in the window before she is rescued.
fn spider_pick(hard_mode: bool, rng: &mut SmallRng) -> u16 {
    if hard_mode && !rng.random_ratio(1, 10) {
        163 // BlackRecluse
    } else {
        164 // WallCreeper
    }
}

/// Whether a candidate spot is in the underground desert, `NPC.cs:1682`:
///
/// ```csharp
/// else if ((SpawnTileOrAboveHasAnyWallInSet(spawnTileX, spawnTileY,
///               WallID.Sets.AllowsUndergroundDesertEnemiesToSpawn) || spawnUndergroundDesert)
///          && WorldGen.checkUnderground(spawnTileX, spawnTileY))
/// ```
///
/// `y` is this server's spawn row, the one the NPC's feet occupy, so the game's `spawnTileY` (the
/// solid ground tile) is `y + 1` and its "or above" tile is `y` - the same one-row offset
/// [`water_tile`] documents. `SpawnTileOrAboveHasAnyWallInSet` (`NPC.cs:5535-5556`) is exactly those
/// two rows and nothing else; its `InWorld(x, y, 2)` guard needs no counterpart, because
/// `World::tile` already answers off the map with bare air, whose wall is 0 and so is in no set.
///
/// The `|| spawnUndergroundDesert` half is the disclosed narrowing. That flag (`NPC.cs:1178-1201`)
/// widens the branch to spots merely *near* desert walls above the rock layer: one attempt in three
/// sweeps a box of radius 5 to 15 around the chosen tile, and the other two read the wall at the
/// player's own tile. Left out on purpose, because the box is up to 900 tile reads on the
/// per-candidate path (20 candidates per player per tick) to reach spots the wall test misses
/// anyway, and a real underground desert is walled throughout. The effect is that the roster starts
/// a little further inside the biome than the game's does, never that it appears outside it.
pub fn underground_desert_spot(world: &World, x: i32, y: i32) -> bool {
    let walled = |row: i32| DESERT_SPAWN_WALLS.contains(&world.tile(x, row).wall);
    (walled(y + 1) || walled(y)) && check_underground(world, x, y + 1)
}

/// The underground desert's roster, `NPC.cs:1684-1764`.
///
/// The whole 1.4 desert lives here and nowhere else, which is why twelve types were unreachable:
/// the four ghouls, the two lamias, the scorpion, the beast, the djinn, the tomb crawler and both
/// giant antlions have no other ambient spawn in the game at all.
///
/// `spawn_y` is this server's spawn row; the game's `spawnTileY` is one below it, and every depth
/// test here is written against the game's own row. Two things the caller owns rather than this:
/// the Golfer at one attempt in twenty (`NPC.cs:1693-1697`), who arrives through [`bound_gate`]
/// like every other bound resident, and the branch's position in the chain.
///
/// `biome` stands in for the game's three independent `ZoneCorrupt`/`ZoneCrimson`/`ZoneHallow`
/// flags, which `SetSpawnFlags` copies straight off the player (`NPC.cs:381-383`). It is the
/// *player's* zone that picks the ghoul, not the tile under the spawn, so no per-spot scan is
/// needed or wanted. The narrowing is that [`Biome`] names one winner where the game can have two
/// set at once; with one set the two agree exactly, and the game's own "no evil, no hallow" fallback
/// to the plain ghoul is this function's `_` arm.
pub fn underground_desert_pick(
    world: &World,
    spawn_y: i32,
    hard_mode: bool,
    biome: Biome,
    no_worms: bool,
    alive: &dyn Fn(u16) -> usize,
    rng: &mut SmallRng,
) -> u16 {
    // `num10` (`NPC.cs:1684-1692`), which thins the two worms out with depth.
    let ground_y = f64::from(spawn_y + 1);
    let rock = f64::from(world.rock_layer);
    let mut scale = 1.3f32;
    if ground_y > (rock * 2.0 + f64::from(world.height())) / 3.0 {
        scale *= 0.5;
    } else if ground_y > rock {
        scale *= 0.85;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let worm_odds = (50.0 * scale) as u32;
    let worm_roll = |rng: &mut SmallRng| rng.random_range(0..worm_odds.max(1)) == 0;
    let deep = ground_y > f64::from(world.surface) + 100.0;

    // The Dune Splicer, hardmode only (`NPC.cs:1698-1702`).
    if hard_mode && worm_roll(rng) && !no_worms && deep {
        return 510;
    }
    // The Tomb Crawler, at any progression, but only ever one at a time (`NPC.cs:1703-1707`).
    if worm_roll(rng) && !no_worms && deep && alive(513) == 0 {
        return 513;
    }
    // Hardmode's own roster, four attempts in five (`NPC.cs:1708-1746`). The game builds a weighted
    // list and picks uniformly from it, so the duplicated ghoul really is twice as likely as the
    // rest; the lists below are that `List<int>` verbatim.
    if hard_mode && rng.random_range(0..5) != 0 {
        let list: &[u16] = match biome {
            // 525 x2, then the `ZoneCorrupt || ZoneCrimson` pair, then the beast.
            Biome::Corruption => &[525, 525, 533, 529, 532],
            Biome::Crimson => &[526, 526, 533, 529, 532],
            // The hallow takes the *other* pair, since it is neither evil.
            Biome::Hallow => &[527, 527, 530, 528, 532],
            _ => &[524, 524, 530, 528, 532],
        };
        return list[rng.random_range(0..list.len())];
    }
    // ...and the antlions, which are the desert whatever the progression (`NPC.cs:1747-1764`).
    let mut ty = [69, 580, 580, 580, 581][rng.random_range(0..5)];
    if rng.random_range(0..15) == 0 {
        ty = 537; // SandSlime, which replaces whatever was drawn rather than joining the draw.
    } else if rng.random_range(0..10) == 0 {
        // The giants are not a hardmode upgrade: this roll is on the plain path, so a fresh world's
        // underground desert already has them.
        ty = match ty {
            580 => 508, // GiantWalkingAntlion
            581 => 509, // GiantFlyingAntlion
            other => other,
        };
    }
    ty
}

/// `TileID.Sets.Conversion.Sand` (`TileID.cs:30`): sand and its three converted forms.
const SAND_CONVERSION: [u16; 4] = [
    53,  // Sand
    112, // Ebonsand
    116, // Pearlsand
    234, // Crimsand
];

/// `NPC.Spawner.Spawning_SandstoneCheck` (`NPC.cs:5464-5503`): enough sand under the spot to call it
/// a desert rather than a beach.
///
/// Walks up to eight rows down from the ground tile, and in each row up to four tiles right and four
/// left, stopping the moment a row (or a run within it) leaves sand. At least 40 of the possible 72
/// have to be sand. Every loop breaks early, so ordinary rock costs one read. The game's
/// `InWorld(x, y, 10)` guard needs no counterpart for the same reason [`underground_desert_spot`]'s
/// does not: off the map reads as bare air, which is not sand, which breaks the walk.
pub fn sandstone_check(world: &World, x: i32, ground_y: i32) -> bool {
    let sand = |x: i32, y: i32| {
        let tile = world.tile(x, y);
        tile.is_active() && SAND_CONVERSION.contains(&tile.block)
    };
    let mut count = 0;
    for i in 0..8 {
        if !sand(x, ground_y + i) {
            break;
        }
        count += 1;
        for j in 1..=4 {
            if !sand(x + j, ground_y + i) {
                break;
            }
            count += 1;
        }
        for k in 1..=4 {
            if !sand(x - k, ground_y + i) {
                break;
            }
            count += 1;
        }
    }
    count >= 40
}

/// The desert during a sandstorm, `NPC.cs:3952-4022`.
///
/// The gate is `Sandstorm.Happening && ZoneSandstorm && TileID.Sets.Conversion.Sand[tileType] &&
/// Spawning_SandstoneCheck(spawnTileX, spawnTileY)`, and the caller owns the first two: a sandstorm
/// is world weather (`game/weather.rs` already runs one) and `ZoneSandstorm` is
/// `ZoneDesert && SurfaceAtmospherics && Sandstorm.Happening` (`SceneMetrics.cs:706`), which is a
/// property of where the *player* stands. This owns the two per-spot halves and the roster.
///
/// Returns the type and how many tiles below the spawn row to put it: the Dune Splicer alone arrives
/// ten tiles down (`NPC.cs:3975`, `(spawnTileY + 10) * 16`), because it burrows in rather than
/// standing on the sand.
///
/// `ground_block` is the tile the spawn stands on, the game's `tileType`. It decides the sandshark's
/// flavour, and that one *is* per-spot rather than per-player: corrupt sand gives a corrupt
/// sandshark wherever it is, which is the opposite of how the ghouls above resolve their evil.
pub fn sandstorm_pick(
    hard_mode: bool,
    downed_boss1: bool,
    no_worms: bool,
    ground_block: u16,
    alive: &dyn Fn(u16) -> usize,
    rng: &mut SmallRng,
) -> (u16, i32) {
    // Before the Eye of Cthulhu, a sandstorm is tumbleweeds, vultures and antlions
    // (`NPC.cs:3954-3968`).
    if !downed_boss1 && !hard_mode {
        if rng.random_range(0..2) == 0 {
            return (546, 0); // Tumbleweed
        }
        if rng.random_range(0..2) == 0 {
            return (61, 0); // Vulture
        }
        return (69, 0); // Antlion
    }
    // The Sand Elemental, one at a time (`NPC.cs:3969-3972`).
    if hard_mode && rng.random_range(0..20) == 0 && alive(541) == 0 {
        return (541, 0);
    }
    // Dune Splicers, up to four (`NPC.cs:3973-3976`).
    if hard_mode && !no_worms && rng.random_range(0..3) == 0 && alive(510) < 4 {
        return (510, 10);
    }
    // Sandsharks, flavoured by the sand they swim through (`NPC.cs:3977-3992`). The game tests all
    // three sets in sequence rather than as an `else if` chain, so the last match wins; no tile is
    // in two of them, so the order is not observable.
    if hard_mode && !no_worms && rng.random_range(0..2) == 0 {
        // `TileID.Sets.Corrupt` / `Crimson` / `Hallow` (`TileID.cs:333`, `:351`, `:341`), narrowed
        // to the sand members, since `ground_block` has already passed [`SAND_CONVERSION`].
        return (
            match ground_block {
                112 => 543, // Ebonsand: SandsharkCorrupt
                234 => 544, // Crimsand: SandsharkCrimson
                116 => 545, // Pearlsand: SandsharkHallow
                _ => 542,   // SandShark
            },
            0,
        );
    }
    // A mummy for each sand (`NPC.cs:3994-4009`), which the game writes as four `else if` arms with
    // a roll each. Only one tile type can match, so folding them into a lookup keeps both the
    // one-in-three odds and the single roll.
    let mummy = match ground_block {
        53 => Some(78),   // Mummy
        112 => Some(79),  // DarkMummy
        234 => Some(630), // BloodMummy
        116 => Some(80),  // LightMummy
        _ => None,
    };
    if hard_mode
        && let Some(mummy) = mummy
        && rng.random_range(0..3) == 0
    {
        return (mummy, 0);
    }
    // The tail of the chain, which is what an ordinary hardmode sandstorm mostly draws
    // (`NPC.cs:4010-4021`).
    if rng.random_range(0..2) == 0 {
        return (546, 0); // Tumbleweed
    }
    if rng.random_range(0..2) == 0 {
        return (580, 0); // WalkingAntlion
    }
    (581, 0) // FlyingAntlion
}

/// The dungeon's slab walls, `WallID.BlueDungeonSlabUnsafe`, `PinkDungeonSlabUnsafe` and
/// `GreenDungeonSlabUnsafe` (`WallID.cs:257`, `:261`, `:265`).
const DUNGEON_SLAB_WALLS: [u16; 3] = [94, 96, 98];

/// ...and its tile walls, `BlueDungeonTileUnsafe`, `PinkDungeonTileUnsafe` and
/// `GreenDungeonTileUnsafe` (`WallID.cs:259`, `:263`, `:267`).
const DUNGEON_TILE_WALLS: [u16; 3] = [95, 97, 99];

/// `RollLuck(7) == 0` (`NPC.cs:2642-2645`): one attempt in seven ignores the wall it found and takes
/// a style at random, which is what keeps a single-style room from being a single-enemy room.
///
/// `RollLuck` is `Luck.RollLuck(luck, range)` (`NPC.cs:5356-5359`), and with no luck effects that is
/// `Main.rand.Next(range)`. This server models no luck at all, the same narrowing (and the same
/// reasoning) as the `rate * 0.85` bonus [`rates`] declines to transcribe.
const DUNGEON_STYLE_REROLL: u32 = 7;

/// Which of the dungeon's three brick styles a spot is built from: `num40` (`NPC.cs:2631-2645`).
///
/// This is *not* the wall's colour, whatever the ids look like at a glance. 94, 96 and 98 are the
/// blue, pink and green **slab** walls and 95, 97 and 99 the blue, pink and green **tile** walls
/// (`WallID.cs:257-267`), so the game sorts a spot by the shape of its masonry and not by its
/// colour: a blue slab corridor and a green slab corridor offer exactly the same enemies, and the
/// plain brick walls (7, 8 and 9) and everything else fall through to 0.
///
/// Tile is checked after slab and so wins when a spot has one of each within its two rows, which is
/// the game's own order rather than a preference of ours.
///
/// `y` is this server's spawn row, the one the NPC's feet occupy, so the game's `spawnTileY` is
/// `y + 1` and its "or above" tile is `y`: the same one-row offset [`underground_desert_spot`]
/// documents. Two tile reads, on a path that already reads several, and only where the caller has
/// already decided it is standing in a hardmode dungeon.
fn dungeon_brick_style(world: &World, x: i32, y: i32, rng: &mut SmallRng) -> u8 {
    let mut style = 0;
    let has = |walls: &[u16]| {
        walls.contains(&world.tile(x, y + 1).wall) || walls.contains(&world.tile(x, y).wall)
    };
    if has(&DUNGEON_SLAB_WALLS) {
        style = 1;
    }
    if has(&DUNGEON_TILE_WALLS) {
        style = 2;
    }
    if rng.random_range(0..DUNGEON_STYLE_REROLL) == 0 {
        style = rng.random_range(0..3u8);
    }
    style
}

/// The hardmode dungeon's own chain, `NPC.cs:2661-2722`.
///
/// `hardDungeon` is `downedPlantBoss && Main.hardMode` (`NPC.cs:381`), which the caller holds along
/// with the zone. Every arm here returns from the game's spawn attempt the moment it fires, so this
/// is a sequence of independent rolls and not a pool: folding it into [`pool`] would hand the
/// Paladin a share of every dungeon draw instead of one attempt in thirty-five, and would lose the
/// brick style entirely, since three quarters of the roster is chosen by it.
///
/// `None` means no arm answered and the caller should fall through to the ordinary dungeon pool, the
/// way vanilla falls through to `:2723`. `Some(None)` is the one arm that answers with nothing: the
/// caster at `:2691` returns whether or not its `!AnyNPCs` test lets it spawn (`:2703-2707`), so
/// while one is alive a twentieth of the dungeon's attempts produce no NPC at all rather than an
/// Angry Bones.
fn hard_dungeon_pick(
    style: u8,
    alive: &dyn Fn(u16) -> bool,
    rng: &mut SmallRng,
) -> Option<Option<u16>> {
    // `:2661`, the one arm that does not care which brick it is standing on.
    if rng.random_range(0..30) == 0 {
        return Some(Some(287)); // BoneLee
    }
    // `:2666-2680`. Three separate `if`s in the game, each testing its own `num40` before rolling,
    // and `num40` is exactly one of the three: one roll happens, and it is this one.
    let gunner = match style {
        0 => 293, // SkeletonCommando
        1 => 291, // SkeletonSniper
        _ => 292, // TacticalSkeleton
    };
    if rng.random_range(0..15) == 0 {
        return Some(Some(gunner));
    }
    // `:2681`, plain brick only, and never a second one. The game's own operand order: the census
    // test comes before the style and before the roll.
    if !alive(290) && style == 0 && rng.random_range(0..35) == 0 {
        return Some(Some(290)); // Paladin
    }
    // `:2686`, slab and tile only.
    if style != 0 && rng.random_range(0..30) == 0 {
        return Some(Some(289)); // GiantCursedSkull
    }
    // `:2691-2708`, `num41`: 281 for slab, +2 for brick, +4 for tile, then one of the pair. The
    // whole arm returns even when the census turns it away, which is what `Some(None)` carries.
    if rng.random_range(0..20) == 0 {
        let base = match style {
            0 => 283, // Necromancer, NecromancerArmored
            1 => 281, // RaggedCaster, RaggedCasterOpenCoat
            _ => 285, // DiabolistRed, DiabolistWhite
        };
        let caster = base + rng.random_range(0..2u16);
        return Some(if alive(caster) { None } else { Some(caster) });
    }
    // `:2709-2722`, `num42`: 269 for slab, +4 for brick, +8 for tile, then one of the four. Two
    // attempts in three, which makes the armoured bones most of what a post-Plantera dungeon is.
    if rng.random_range(0..3) != 0 {
        let base = match style {
            0 => 273, // BlueArmoredBones and its three variants
            1 => 269, // RustyArmoredBones and its three variants
            _ => 277, // HellArmoredBones and its three variants
        };
        let bones = base + rng.random_range(0..4u16);
        return Some(Some(bones));
    }
    None
}

/// The dungeon's ordinary chain, `NPC.cs:2723-2747`, which every dungeon runs and which
/// [`hard_dungeon_pick`] falls through into:
///
/// ```csharp
/// if (RollLuck(35) == 0)                        { 71; return; }
/// if (num40 == 1 && Main.rand.Next(3) == 0 && !NearSpikeBall(spawnTileX, spawnTileY))
///                                               { 70; return; }
/// if (num40 == 2 && Main.rand.Next(5) == 0)     { 72; return; }
/// if (num40 == 0 && Main.rand.Next(7) == 0)     { 34; return; }
/// if (Main.rand.Next(7) == 0)                   { 32; return; }
/// ```
///
/// `None` means no arm answered and the caller goes on to the library and then to the pool, the way
/// vanilla falls through to `:2748`.
///
/// These five used to be flattened into [`pool`]'s dungeon arm, which cost three things. The two
/// traps were dropped outright, so the Spike Ball and the Blazing Wheel were unreachable even though
/// both routines are here in full and tested (`game::ai::track`) and a dungeon corridor with nothing
/// running down it is not the dungeon. The brick style stopped mattering, when it decides two of the
/// five: no Cursed Skull is ever found on slab or tile, and no Spike Ball anywhere but slab. And the
/// odds were levelled, when they are not level at all: a Dungeon Slime is one attempt in
/// thirty-five, a Dark Caster one in seven of what is left, and each of the three preceding rolls
/// takes its share off the ones below it.
///
/// The three fifths of `num43` below this chain that fall to the plain Angry Bones remain the pool's
/// business (`:2767-2795`), because that is a draw rather than a chain.
fn dungeon_pick(
    style: u8,
    near_spike_ball: &dyn Fn() -> bool,
    luck: f32,
    rng: &mut SmallRng,
) -> Option<u16> {
    // `:2723`. `RollLuck(35)` is a plain `Main.rand.Next(35)` at luck zero (`Luck.cs:5-16`), the
    // same narrowing every other `Roll*Luck` call site in this file takes.
    if rolls_lucky(luck, DUNGEON_SLIME_ODDS, rng) {
        return Some(DUNGEON_SLIME);
    }
    // `:2728`, slab only. `NearSpikeBall` is asked last because `&&` short-circuits and it is the
    // one operand that walks the NPC table: the scan happens on one slab attempt in three and never
    // otherwise, which is vanilla's own operand order rather than a rearrangement for speed.
    if style == 1 && rng.random_range(0..SPIKE_BALL_ODDS) == 0 && !near_spike_ball() {
        return Some(SPIKE_BALL);
    }
    // `:2733`, tile only.
    if style == 2 && rng.random_range(0..BLAZING_WHEEL_ODDS) == 0 {
        return Some(BLAZING_WHEEL);
    }
    // `:2738`, plain brick only.
    if style == 0 && rng.random_range(0..CURSED_SKULL_ODDS) == 0 {
        return Some(CURSED_SKULL);
    }
    // `:2743`, whatever the brick.
    if rng.random_range(0..DARK_CASTER_ODDS) == 0 {
        return Some(DARK_CASTER);
    }
    None
}

/// `NPC.NearSpikeBall(x, y)` (`NPC.cs:91002-91017`):
///
/// ```csharp
/// Rectangle rectangle = new Rectangle(x * 16 - 300, y * 16 - 300, 600, 600);
/// for (int i = 0; i < Main.maxNPCs; i++)
///   if (Main.npc[i].active && Main.npc[i].aiStyle == 20) {
///     Rectangle rectangle2 = new Rectangle((int)Main.npc[i].ai[1], (int)Main.npc[i].ai[2], 20, 20);
///     if (rectangle.Intersects(rectangle2)) return true;
///   }
/// return false;
/// ```
///
/// It is what stops a corridor filling up with spike balls, and it is deliberately not a distance to
/// the ball itself: `ai[1]`/`ai[2]` are the ceiling point the ball dropped out of and stays anchored
/// to (`game::ai::track::spike_ball` seeds them on its first tick), so a ball that has swung a long
/// way from home still holds its spot. A ball that has not ticked yet carries `(0, 0)` and so anchors
/// at the top-left corner of the world, exactly as it does in the game.
///
/// `x`/`y` are vanilla's `spawnTileX`/`spawnTileY`, and its `spawnTileY` is this server's `y + 1`.
/// Both boxes are integer `Rectangle`s in the game and `Rectangle.Intersects` is a strict
/// four-way overlap, so the arithmetic is kept in `i32` with the same `(int)` truncation of the
/// anchor rather than being redone in floats at the edges.
fn near_spike_ball(npcs: &NpcStore, x: i32, y: i32) -> bool {
    /// `x * 16 - 300`, and a span of `600`.
    const REACH: i32 = 300;
    const SPAN: i32 = 600;
    /// The anchor's own box, `new Rectangle(ai[1], ai[2], 20, 20)`.
    const ANCHOR: i32 = 20;
    let (left, top) = (x * 16 - REACH, y * 16 - REACH);
    npcs.iter().any(|(_, n)| {
        if !n.is_alive() || n.stats.ai_style != SPIKE_BALL_AI_STYLE {
            return false;
        }
        let (ax, ay) = (n.ai[1] as i32, n.ai[2] as i32);
        ax < left + SPAN && left < ax + ANCHOR && ay < top + SPAN && top < ay + ANCHOR
    })
}

/// `NPC.AI_FindNearbyBook(searchPosition, 32, 32, out bookPosition, closestBook: true,
/// checkPlayerScreenRanges: true)` (`NPC.cs:62954-63010`), the one shape the spawner ever asks for.
///
/// ```csharp
/// for (int i = num5; i < num6; i++)
///   for (int j = num3; j < num4; j++) {
///     Tile tile = Main.tile[j, i];
///     if (!tile.active() || tile.type != 50) continue;
///     Vector2 vector3 = new Vector2(j, i);
///     if (checkPlayerScreenRanges && !Spawner.CheckNotSpawningOnScreen((int)vector3.X, (int)vector3.Y))
///         continue;
///     float num8 = vector3.Distance(vector2);
///     if (closestBook && num8 < num7) { num7 = num8; vector = vector3; continue; }
///     ...
///   }
/// ```
///
/// Two things about it read as bugs and are the game's, so both are kept:
///
/// * the distance is measured from `vector2`, which is `searchPosition` itself, the box's **top
///   left corner** and not the candidate tile in the middle of it. The shelf that wins is therefore
///   the one nearest that corner, which is up and to the left of where the attempt was made.
/// * "found nothing" and "found a book on the corner tile" are the same answer. `vector` starts as
///   the anchor and the tail (`:62995-63003`) returns false while the winner still equals it, so a
///   book sitting exactly on the corner is reported as no book at all.
///
/// `from` is that corner in tile coordinates and `player` the tile the attempt's own player stands
/// on. Two halves of the game's function are deliberately not transcribed, and neither can change
/// what this returns:
///
/// * the `closestBook: false` path, which fills a twenty-slot buffer and picks from it at random.
///   The spawner never asks for it. Its one caller is the Librarian's own spell placement
///   (`NPC.cs:21323-21337`, a 20-by-30 box), which is AI rather than spawning.
/// * that buffer's `if (num2 >= num) break` (`:62989-62992`), which cuts a row short once twenty
///   books that were *not* closer have gone into it. It cannot lose the winner: inside one row `j`
///   only increases and so does the distance from the corner, so the first book in a row that fails
///   to beat the running best is followed only by books that also fail. (The same monotonicity is
///   why the game gets away with it. On this path the buffer is written and never read, and a
///   twenty-first non-closest book would index one past the end of `_nearbyBooks`, which is
///   `new Point[20]` at `NPC.cs:6608`.)
///
/// `checkPlayerScreenRanges` is [`SAFE_RANGE_X`]/[`SAFE_RANGE_Y`] here, the same box [`try_spawn`]
/// already keeps its candidate tiles out of. Vanilla's `CheckNotSpawningOnScreen`
/// (`NPC.cs:5444-5462`) is a screen plus a safe range around *every* active player, which works out
/// at roughly 64 by 36 tiles against this server's 62 by 35 around the one player the attempt
/// belongs to. That is the narrowing the candidate test already carries, reused rather than widened
/// for one branch.
pub fn find_nearby_book(world: &World, from: (i32, i32), player: (i32, i32)) -> Option<(i32, i32)> {
    // `num3`..`num6`, the box clipped to the map.
    let left = from.0.max(0);
    let right = (from.0 + BOOK_SEARCH_SPAN).min(world.width());
    let top = from.1.max(0);
    let bottom = (from.1 + BOOK_SEARCH_SPAN).min(world.height());
    // `num7`, the running best, and `vector`, which starts as the anchor `vector2` itself.
    let mut best = i32::MAX;
    let mut found = from;
    for y in top..bottom {
        for x in left..right {
            let tile = world.tile(x, y);
            if !tile.is_active() || tile.block != BOOK_TILE {
                continue;
            }
            if (x - player.0).abs() < SAFE_RANGE_X && (y - player.1).abs() < SAFE_RANGE_Y {
                continue;
            }
            // Squared, which orders identically to the game's own `Vector2.Distance` and costs no
            // square root. The box is 32 tiles across, so nothing here can overflow.
            let (dx, dy) = (x - from.0, y - from.1);
            let distance = dx * dx + dy * dy;
            if distance < best {
                best = distance;
                found = (x, y);
            }
        }
    }
    (found != from).then_some(found)
}

/// Mushroom grass, `TileID.MushroomGrass` (`TileID.cs:577`).
///
/// This is the whole gate on the Glowing Mushroom roster, and it is worth being blunt about, because
/// it is the opposite of what the zone flag next to it suggests: `ZoneGlowshroom` has nothing to do
/// with these three branches. Vanilla asks only what the spawn is *standing on*
/// (`NPC.cs:3633`, `:3637`, `:3674`), so a single patch of mushroom grass grown on mud has the
/// Truffle Worm and the mushroom zombies whether or not there is a biome's worth of it around.
pub const MUSHROOM_GRASS: u16 = 70;
/// A glowing mushroom block, `TileID.MushroomBlock` (`TileID.cs:817`).
///
/// Only the Spore pair reads it, and only alongside the zone (`NPC.cs:5110`, `:5209`).
pub const MUSHROOM_BLOCK: u16 = 190;

/// The Glowing Mushroom roster, `NPC.cs:3637-3702`: the two arms that hang off mushroom grass.
///
/// Not a pool, for the same reason [`seasonal_night_pick`] is not: vanilla writes each arm as a
/// sequence of independent rolls that returns the moment one hits, and the arm as a whole is behind
/// a `Main.rand.Next(3) != 0` that lets one attempt in three fall through to the ordinary chain
/// below. `None` is that fallthrough.
///
/// `surface` is the game's `(double)spawnTileY <= Main.worldSurface`, which splits the two arms: the
/// surface one is open at any point in a world's life, the underground one is hardmode only and is
/// where the Truffle Worm lives. Taken from [`Depth`] here, so the boundary sits one tile out from
/// vanilla's (this server's spawn row is one above the ground tile the game measures); the same
/// one-row offset every other depth test in this module carries.
///
/// Three parts of the game's own conditions are deliberately not written out:
///
/// * `Main.tile[spawnTileX, spawnTileY].type == tileType`, on both `:3645` and `:3684`. It asks
///   whether `FindGroundTile` (`NPC.cs:5879-5897`) had to walk further down to find solid ground
///   than the spawn descent already had. Here the ground tile *is* the first solid tile below the
///   candidate by construction, so it is always true.
/// * `!Main.hardMode && Main.rand.Next(4) == 0` at `:3680`, inside an arm whose own gate already
///   requires hardmode. The left half is false, so its roll is never made and the one in eight
///   beside it is the whole of that line.
/// * `!Main.remixWorld || Main.getGoodWorld || spawnTileY < Main.maxTilesY - 360` at `:3674`. This
///   server generates neither seed, so the first disjunct is true and the line cannot fail.
///
/// `wet` is the game's `waterTile`, and it is what the first arm of the three reads: `Main.hardMode
/// && tileType == 70 && waterTile` -> Fungo Fish (`NPC.cs:3633-3636`), ahead of both arms below it
/// and with no roll of its own, so a hardmode mushroom cave's standing water is nothing but Fungo
/// Fish. It sat unreachable until now for a reason worth recording: the caller used to answer any
/// standing water below the surface line with a Blue Jellyfish on *every* attempt, missing vanilla's
/// own `Main.rand.Next(3) == 0` gate at `:1988`, so nothing wet ever reached this function at all.
/// [`water_pick`] declines properly now, and 256 is reachable.
pub fn mushroom_pick(
    surface: bool,
    hard_mode: bool,
    wet: bool,
    luck: f32,
    rng: &mut SmallRng,
) -> Option<u16> {
    let one_in = |rng: &mut SmallRng, n: u32| rng.random_ratio(1, n);

    // NPC.cs:3633. Above both arms below, and it takes the whole attempt when it holds.
    if hard_mode && wet {
        return Some(256); // FungoFish
    }

    if surface {
        // NPC.cs:3637. `Main.rand.Next(3) != 0`, so one attempt in three declines the whole arm.
        if one_in(rng, 3) {
            return None;
        }
        // NPC.cs:3639. Before hardmode the snail has two chances at it, after it only the one.
        if (!hard_mode && one_in(rng, 6)) || one_in(rng, 12) {
            return Some(360); // GlowingSnail
        }
        // NPC.cs:3643-3664, the critter third of the arm.
        if one_in(rng, 3) {
            if one_in(rng, 4) {
                // NPC.cs:3645-3655.
                return Some(if hard_mode && !one_in(rng, 3) {
                    260 // GiantFungiBulb
                } else {
                    259 // FungiBulb
                });
            }
            return Some(if one_in(rng, 2) {
                257 // AnomuraFungus
            } else {
                258 // MushiLadybug
            });
        }
        // NPC.cs:3665-3672, the rest: the mushroom zombies.
        return Some(if one_in(rng, 2) {
            254 // ZombieMushroom
        } else {
            255 // ZombieMushroomHat
        });
    }

    // NPC.cs:3674, the underground arm. Hardmode is part of its own gate, and is checked before the
    // roll, so a pre-hardmode mushroom cave falls straight through to the ordinary underground
    // chain rather than losing an attempt in three to a branch that cannot answer.
    if !hard_mode || one_in(rng, 3) {
        return None;
    }
    // NPC.cs:3676, the Truffle Worm, and the only thing in the game that summons Duke Fishron.
    // `RollLuck(5)` is a plain one in five at luck zero (`Luck.cs:5-16`), which is where every
    // player on this server sits.
    if rolls_lucky(luck, 5, rng) {
        return Some(374); // TruffleWorm
    }
    // NPC.cs:3680.
    if one_in(rng, 8) {
        return Some(360); // GlowingSnail
    }
    // NPC.cs:3684-3694.
    if one_in(rng, 4) {
        return Some(if !one_in(rng, 3) {
            260 // GiantFungiBulb
        } else {
            259 // FungiBulb
        });
    }
    // NPC.cs:3695-3702.
    Some(if one_in(rng, 2) {
        257 // AnomuraFungus
    } else {
        258 // MushiLadybug
    })
}

/// The Meteor Head (23), which is every single thing a meteor crater spawns.
///
/// `NPC.cs:2796-2799` is the whole branch: `else if (ZoneMeteor)` with one unconditional
/// `SpawnNPC(..., 23)` inside it, sitting between the dungeon's arm and the fallthrough that holds
/// every biome, every season and every tile type. Standing in a crater there is nothing else.
pub const METEOR_HEAD: u16 = 23;

/// The Harpy (48), which is the whole reason the sky is a place.
pub const HARPY: u16 = 48;
/// The Wyvern's head (87). Its fourteen trailing segments grow from its own first AI tick, the way
/// the Solar Crawltipede's do (`NPC.cs:51700-51730`).
pub const WYVERN_HEAD: u16 = 87;
/// The Martian Probe (399), which is the only thing in the game that starts Martian Madness.
pub const MARTIAN_PROBE: u16 = 399;

/// Whether a candidate tile is high enough to be *sky*, in which case nothing walks down to ground.
///
/// `FindSpawnTile` (`NPC.cs:979-986`), the two branches that set `skyMob`:
///
/// ```csharp
/// if (!invaders && (double)j < Main.worldSurface * 0.3499999940395355 && !spawnFriendly
///     && ((double)num < (double)Main.maxTilesX * 0.45 || (double)num > (double)Main.maxTilesX * 0.55
///         || Main.hardMode))
///     skyMob = true;
/// else if (!invaders && (double)j < Main.worldSurface * 0.44999998807907104 && !spawnFriendly
///     && Main.hardMode && Main.rand.Next(10) == 0)
///     skyMob = true;
/// else { for (; j < Main.maxTilesY && j < spawnArea.Bottom && !solid; j++) {} ... }
/// ```
///
/// So the sky is decided from the *spawn tile*, not from where the player stands: anyone whose
/// spawn box reaches above `worldSurface * 0.35` draws it, which is why harpies find you on a
/// mountain. Pre-hardmode the middle tenth of the map is excluded (`0.45 <= x/width <= 0.55`),
/// which is what keeps them off a fresh spawn point; hardmode drops that exclusion and adds a
/// second, lower band down to `worldSurface * 0.45` at one attempt in ten.
///
/// `!invaders` needs no test here: this server never reaches `try_spawn` while an invasion is
/// running (the invasion path returns first, `systems.rs`), which is the same answer.
/// `!spawnFriendly` is the caller's, since it owns that roll.
fn sky_tile(world: &World, x: i32, y: i32, hard_mode: bool, rng: &mut SmallRng) -> bool {
    let surface = f64::from(world.surface);
    let (fx, fy) = (f64::from(x), f64::from(y));
    let width = f64::from(world.width());
    if fy < surface * 0.3499999940395355 && (fx < width * 0.45 || fx > width * 0.55 || hard_mode) {
        return true;
    }
    fy < surface * 0.44999998807907104 && hard_mode && rng.random_range(0..10) == 0
}

/// `Main.wallLight` (`Main.cs:10717-10732`): the walls daylight comes through, no wall included.
///
/// Only the Martian Probe's own gate reads it, through `skyBehindPlayer`.
const WALL_LIGHT: [u16; 16] = [
    0, 21, 106, 107, 138, 139, 140, 141, 145, 150, 152, 168, 245, 315, 317, 318,
];

/// `skyBehindPlayer` (`NPC.cs:413`): `Main.wallLight[Main.tile[pX, pY].wall] || wall == 73`, read
/// at the player's own tile. A probe scouts people standing under open sky, not people in a house.
fn sky_behind_player(wall: u16) -> bool {
    WALL_LIGHT.contains(&wall) || wall == 73
}

/// What the sky sends, once a tile up there has been chosen (`NPC.cs:1383-1424`).
///
/// An ordered chain, not a weighted draw, and its last arm is unconditional: the Harpy is what the
/// sky is when nothing rarer wins. Transcribed with vanilla's own ordering and rolls.
///
/// Two of the game's arms are deliberately absent, each because the thing it needs does not exist
/// here:
///
/// * `invaders && Main.invasionType == 4` -> Martian Drone (388). This server's invasion spawning
///   is a separate path that never reaches the sky branch, so a drone is a Martian Madness member
///   rather than a sky mob.
/// * the two `ZoneWaterCandle` repeats at `:1409` and `:1418`, which are dead code in the game
///   itself: each repeats the condition of the arm immediately above it, so the earlier arm has
///   already answered whenever the later one could.
///
/// `probe_gate` is vanilla's `flag5`, resolved by the caller because it reads the player's tile.
fn sky_pick(
    hard_mode: bool,
    probe_gate: bool,
    world: &World,
    no_worms: bool,
    alive: &dyn Fn(u16) -> bool,
    luck: f32,
    rng: &mut SmallRng,
) -> u16 {
    // `NPC.cs:1400-1404`. `maxValue2`/`maxValue3` are 8 and 30 (`:1384-1385`); the water-candle
    // pair that narrows them to 3 and 10 is a player-carried item this server does not model.
    // One probe at a time, and only ever after the Golem: it is the invitation to Martian Madness,
    // so a second one while the first is still scouting would invite it twice.
    if probe_gate
        && hard_mode
        && world.progress.downed_golem
        && ((!world.progress.downed_martians && rng.random_range(0..8) == 0)
            || rng.random_range(0..30) == 0)
        && !alive(MARTIAN_PROBE)
    {
        return MARTIAN_PROBE;
    }
    // `NPC.cs:1412`: one in ten, one at a time, and not while a wall at the player's back keeps
    // burrowers out. A Wyvern is a worm, and `noWorms` is about worms wherever they fly.
    if hard_mode && !alive(WYVERN_HEAD) && !no_worms && rng.random_range(0..10) == 0 {
        return WYVERN_HEAD;
    }
    // `NPC.cs:1417`: one in twenty-five, one at a time, and only while nobody has freed it yet.
    // No progression gate at all, so a Purple Slime is findable on a fresh world by anyone who
    // gets high enough to be in the sky in the first place. `RollLuck(25)` is `Main.rand.Next(25)`
    // at luck zero (`Luck.cs:5-16`), which is what this server models.
    if !world.progress.unlocked_slime_purple
        && rolls_lucky(luck, 25, rng)
        && !alive(BOUND_TOWN_SLIME_PURPLE)
    {
        return BOUND_TOWN_SLIME_PURPLE;
    }
    HARPY
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
    /// `Sandstorm.Happening` (`Sandstorm.cs:14`), which `game/weather.rs` already simulates.
    ///
    /// Half of `ZoneSandstorm` (`SceneMetrics.cs:706`,
    /// `ZoneDesert && SurfaceAtmospherics && Sandstorm.Happening`); the other two halves are the
    /// player's biome and height, which [`try_spawn`] has to hand.
    pub sandstorm: bool,
    /// `Main.IsItAHappyWindyDay` (`Main.cs:2999`), which `game/weather.rs` latches alongside the
    /// storm from the same hysteresis band. Two of the surface day's arms are gated on it and on
    /// nothing else about the weather (`NPC.cs:4494`, `:4498`).
    pub happy_windy_day: bool,
    /// `Main.windSpeedTarget`, for `isSpawningInWindDirection` (`NPC.cs:1202`). The *target* and not
    /// the wind itself, which is what the game reads there: the two differ only while a gust is
    /// still turning around, and on that tick the game asks where the wind is going rather than
    /// where it has got to.
    pub wind_target: f32,
    pub downed_plantera: bool,
    pub downed_all_mechs: bool,
    /// Whether the field already holds as many event bosses as it will take.
    pub boss_cap: bool,
    /// `NPC.AnyDanger()` (`NPC.cs:81063-81106`): a moon, an invasion, the Old One's Army, a live
    /// boss, or the Moon Lord's countdown. Only the Martian Probe's gate reads it, and it reads it
    /// as "not while something is already happening".
    ///
    /// Resolved by the caller because half of it is server state `try_spawn` cannot see. The
    /// `DangerThatPreventsOtherDangers` set (the lunar pillars) is folded into the caller's boss
    /// test rather than named separately.
    pub any_danger: bool,
    /// `BirthdayParty.PartyIsUp` (`game/party.rs`), for the party bunny the bunny chain wears
    /// alongside its Halloween and Christmas costumes (see [`holiday_costume`]).
    pub party: bool,
    /// The whole sky half of the Enchanted Nightcrawler's gate (`NPC.cs:2409`): a meteor shower
    /// over a clear, un-overcast sky. Three clauses resolved by the caller into the one bit the
    /// spawner reads, because all three are state it already keeps and none of them varies per
    /// candidate tile.
    ///
    /// * `Star.starfallBoost > 3f`. `Star.NightSetup` (`Star.cs:41-60`) re-rolls the boost at every
    ///   dusk, from `Main.UpdateTime_StartNight` (`Main.cs:66208`): one night in ten it is
    ///   `Main.rand.Next(300, 501) * 0.01f`, otherwise one in three of the rest it is
    ///   `Main.rand.Next(100, 151) * 0.01f`, and otherwise it is 1. Only the first branch can clear
    ///   3, and only where it rolled 301 or above, so a shower night is a shade under one in ten.
    /// * `Main.numClouds <= 55`, the world's own published cloud count.
    /// * `Main.cloudBGActive == 0f`, the overcast-background counter
    ///   (`Main.updateCloudLayer`, `Main.cs:13346-13400`), which `game/weather.rs` already ticks.
    pub starfall_night: bool,
    /// `NPC.Spawner.fairyLog` (`NPC.cs:150`): whether this world still has a fallen log standing in
    /// it. The one gate on the underground fairy (see [`underground_fairy`]), and the only thing
    /// the spawner ever reads of the whole mystic-log business.
    ///
    /// Resolved by the caller for the same reason [`Self::starfall_night`] is: vanilla re-decides
    /// it at world load, at every dusk and whenever a log is broken (`MysticLogFairiesEvent`), not
    /// per candidate tile, so a per-tile scan of the whole overworld would be paying a million tile
    /// reads for an answer that changes at most once a night.
    pub fairy_log: bool,
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
    /// Where each lunar pillar that is still standing is, in [`crate::game::lunar::PILLARS`] order.
    ///
    /// Vanilla decides a tower zone on the client, in `SceneMetrics`: `ScanNPCPositions`
    /// (`SceneMetrics.cs:734-751`) keeps the nearest live NPC of every type, and each
    /// `CloseEnoughTo*Tower` (`SceneMetrics.cs:276-282`) is `WithinRangeOfNPC(<pillar>,
    /// NPCEventZoneRadius)` against it. The caller gathers the four positions once a tick, and only
    /// while the event is up, so the test in [`Self::tower_zone`] is four distance comparisons per
    /// player rather than another pass over the NPC store on the spawn path.
    pub towers: [Option<(f32, f32)>; 4],
}

impl EventSpawns<'_> {
    /// Whether anything is running that overrides the surface pool.
    fn running(&self) -> bool {
        self.moon.is_some() || self.eclipse
    }

    /// Which pillar's zone a point is inside, if any.
    ///
    /// The order is vanilla's own `else if` chain (Nebula, Vortex, Stardust, Solar:
    /// `NPC.cs:1297`, `:1321`, `:1347`, `:1356`), so two overlapping zones resolve the way the
    /// game resolves them rather than by whichever pillar happens to be nearer.
    fn tower_zone(&self, at: (f32, f32)) -> Option<u16> {
        use crate::game::lunar;

        /// `SceneMetrics.NPCEventZoneRadius` (`SceneMetrics.cs:130`), in pixels.
        const RADIUS: f32 = 4000.0;
        const ORDER: [u16; 4] = [lunar::NEBULA, lunar::VORTEX, lunar::STARDUST, lunar::SOLAR];

        ORDER.into_iter().find(|&pillar| {
            lunar::PILLARS
                .iter()
                .position(|p| *p == pillar)
                .and_then(|slot| self.towers[slot])
                // `WithinRangeOfNPC` (`SceneMetrics.cs:912-920`) compares squared distances.
                .is_some_and(|(x, y)| {
                    let (dx, dy) = (x - at.0, y - at.1);
                    dx * dx + dy * dy <= RADIUS * RADIUS
                })
        })
    }
}

/// What a lunar pillar's zone spawns, and nothing else spawns there: `NPC.cs:1297-1372`.
///
/// Each arm is a `Utils.SelectRandom` list, which is a uniform draw over the array
/// (`Utils.cs:2628-2631`), so a repeated id is that id's weight. Three of the four then sit inside
/// a `while` that re-rolls whenever the type drawn is already at its own live cap, `CountNPCS`
/// being `alive` here; the Stardust arm has no such loop at all.
///
/// The bound on the loop is ours. Vanilla's `while` is unbounded and cannot hang, because every
/// list has at least one entry with no cap on it (`427` for Vortex, `421` for Nebula, `417` and
/// friends for Solar); the bound is so a future edit to one of these tables cannot spin a tick
/// instead of failing. `None` means the roster was capped out, and the attempt is dropped.
pub fn tower_pool(pillar: u16, alive: &dyn Fn(u16) -> usize, rng: &mut SmallRng) -> Option<u16> {
    use crate::game::lunar;

    /// Nebula Soldier, Nebula Beast, Nebula Headcrab, Nebula Brain (`NPC.cs:1302`).
    const NEBULA: [u16; 11] = [424, 424, 424, 423, 423, 423, 421, 421, 421, 420, 420];
    /// Vortex Soldier, Vortex Hornet, Vortex Rifleman, Vortex Hornet Queen (`NPC.cs:1328`).
    const VORTEX: [u16; 9] = [429, 429, 429, 429, 427, 427, 425, 425, 426];
    /// Stardust Soldier, Spider, Jellyfish, Worm, Cell (`NPC.cs:1349`).
    const STARDUST: [u16; 8] = [411, 411, 411, 409, 409, 407, 402, 405];
    /// Solar Spearman, Solenian, Corite, Crawltipede, Sroller, Drakomire Rider, Drakomire
    /// (`NPC.cs:1364`).
    const SOLAR: [u16; 7] = [518, 419, 418, 412, 417, 416, 415];
    /// What half of the Corite draws become instead (`NPC.cs:1368`).
    const SOLAR_INSTEAD_OF_CORITE: [u16; 4] = [415, 416, 419, 417];
    /// How many re-rolls before the roster is called capped out. See this function's own doc.
    const ATTEMPTS: usize = 64;

    let pick = |list: &[u16], rng: &mut SmallRng| list[rng.random_range(0..list.len())];

    for _ in 0..ATTEMPTS {
        let drawn = match pillar {
            // `NPC.cs:1297-1319`.
            lunar::NEBULA => {
                let num = pick(&NEBULA, rng);
                if (num == 424 && alive(424) >= 3)
                    || (num == 423 && alive(423) >= 3)
                    || (num == 420 && alive(420) >= 3)
                {
                    continue;
                }
                num
            }
            // `NPC.cs:1321-1345`.
            lunar::VORTEX => {
                let num = pick(&VORTEX, rng);
                if (num == 425 && alive(425) >= 3)
                    || (num == 426 && alive(426) >= 3)
                    || (num == 429 && alive(429) >= 4)
                {
                    continue;
                }
                num
            }
            // `NPC.cs:1347-1354`: one draw, no cap, no loop.
            lunar::STARDUST => pick(&STARDUST, rng),
            // `NPC.cs:1356-1372`. The Corite is re-drawn on a coin flip *inside* the loop, so a
            // re-draw that lands on a capped type re-rolls the whole thing, as the game's does.
            lunar::SOLAR => {
                let mut num = pick(&SOLAR, rng);
                if num == 418 && rng.random_range(0..2) == 0 {
                    num = pick(&SOLAR_INSTEAD_OF_CORITE, rng);
                }
                if (num == 518 && alive(518) >= 2) || (num == 412 && alive(412) >= 1) {
                    continue;
                }
                num
            }
            _ => return None,
        };
        return Some(drawn);
    }
    None
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
    /// Indexed by player slot.
    entries: Vec<Option<Scan>>,
}

/// One cached scan: the tick it was taken, where, and what it said (see [`Zones`]).
type Scan = (u64, i32, i32, Zones);

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
    /// Hands back the whole [`Zones`], not just its winning [`Biome`]: the entry already holds it,
    /// and the caller's question (a shopping zone set) is the game's own independent-flag one.
    pub fn last(&self, slot: usize) -> Option<Zones> {
        self.entries.get(slot).copied().flatten().map(|e| e.3)
    }

    /// This player's zone, scanning only when the last answer has gone stale and this tick still
    /// has budget for it.
    ///
    /// `None` means "no answer available this tick", not "forest": a slot with no entry at all has
    /// no stale answer to fall back on, and handing back a default would put every joining player in
    /// the wrong spawn pool for the half second before its turn comes round. The caller skips that
    /// player for the tick instead, which costs nothing observable at roughly one attempt in 600
    /// placing anything.
    pub fn read(&mut self, world: &World, slot: usize, x: i32, y: i32) -> Option<Zones> {
        if self.entries.len() <= slot {
            self.entries.resize(slot + 1, None);
        }
        if let Some((at, sx, sy, zones)) = self.entries[slot]
            && self.now.saturating_sub(at) < Self::REFRESH
            && (x - sx).abs() <= Self::DRIFT
            && (y - sy).abs() <= Self::DRIFT
        {
            return Some(zones);
        }
        if self.left == 0 {
            // Out of scans this tick. A stale answer is still an answer; nothing is not.
            return self.entries[slot].map(|(_, _, _, zones)| zones);
        }
        self.left -= 1;
        let zones = zones_at(world, x, y);
        self.entries[slot] = Some((self.now, x, y, zones));
        Some(zones)
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
        // Bound Old Slime: Skeletron beaten, and the same caverns band as the two above
        // (`NPC.cs:2095`: `downedBoss3 && deeperThanRockLayer && spawnTileY < maxTilesY - 210`).
        // Its arm sits immediately after the Wizard's in the same `else if` chain, on the same
        // three geometry tests, which is why it belongs in this gate rather than in a chain of its
        // own. The `!unlockedSlimeOldSpawn` half is `rescues::still_bound`, as with every other
        // find here; the `!AnyNPCs(685)` half is `pick_bound`'s own "not already standing about".
        BOUND_TOWN_SLIME_OLD => p.downed_boss3 && depth == Depth::Cavern,
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
        // The bound Old Slime is found down here too but is not freed by talking, so it is not in
        // the rescue table (see `rescues::RESCUES`). Its own gate is in `bound_gate`, and the two
        // other bound slimes are absent from that gate, so chaining them in would find nothing.
        .chain(std::iter::once(BOUND_TOWN_SLIME_OLD))
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

        // Which pillar's zone this player is standing in, if any. Four distance comparisons
        // against positions the caller already gathered, so nothing here walks the NPC store.
        let tower = events.tower_zone(player_centre);

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
        let Some(zones) = biomes.read(world, usize::from(player.slot), px, py) else {
            continue;
        };
        let player_biome = zones.biome;
        // `ZoneSandstorm` (`SceneMetrics.cs:706`): `ZoneDesert && SurfaceAtmospherics &&
        // Sandstorm.Happening`, where `SurfaceAtmospherics` is `IsSurfaceForAtmospherics`
        // (`WorldGen.cs:11003-11014`) and outside a remix world is simply `y <= worldSurface`. It is
        // a *player* zone like every other, so it is answered once here rather than per candidate
        // tile; only the sand under the chosen spot is decided down there.
        //
        // `zones.desert` rather than `player_biome == Desert`: a corrupt, crimson or hallowed desert
        // is both zones at once in the game, and it is where three of the four sandsharks live.
        let sandstorm_zone = events.sandstorm && zones.desert && py <= i32::from(world.surface);
        // The wall at the player's own tile, read once: three player zones hang off it here, the
        // same three the game reads off its one `Framing.GetTileSafely(TileCenter)`
        // (`SceneMetrics.cs:670-693`). `noWorms` wants `Main.wallHouse[]` of it (`NPC.cs:411`),
        // `ZoneLihzhardTemple` wants it to be exactly 87, and the sky's own probe gate wants to
        // know whether there is open air behind the player.
        let player_wall = world.tile(px, py).wall;
        // `Player.ZoneLihzhardTemple` (`SceneMetrics.cs:693`), a player zone like the rest and so
        // answered here rather than per candidate tile: only the brick underfoot is decided down
        // in the loop.
        let temple_zone = player_wall == TEMPLE_WALL;
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
            behind_a_house_wall: terrustia_proto::housing::wall_encloses(player_wall),
            active_players,
            in_tower_zone: tower.is_some(),
            graveyard: player.in_graveyard(),
            meteor: zones.meteor,
            lihzahrd_temple: temple_zone,
            // `SetSpawnFlags` copies this off the player whose attempt this is (`NPC.cs:370`), so
            // two people standing in the same forest roll different odds for a gold critter.
            luck: player.luck,
        };
        // The season and the graveyard, read once per player per attempt: two bools off the world
        // and one bit off the zone packet this player last sent, so nothing here walks a tile.
        let seasonal = Seasonal {
            halloween: world.halloween,
            xmas: world.xmas,
            graveyard: conditions.graveyard,
            hard_mode: events.hard_mode,
            // `Main.expertMode` is `Main.Difficulty >= 2` (`Main.cs:2785`), and `Main.Difficulty`
            // in a Journey world is the slider rather than the world's own mode. The same two-line
            // shape `Server::effective_difficulty` uses, written here because `try_spawn` already
            // holds both halves of it and needs no accessor to reach them.
            expert: if journey_world {
                journey.difficulty_multiplier()
            } else {
                terrustia_proto::difficulty::of_game_mode(world.game_mode)
            } >= 2.0,
            blood_moon: world.blood_moon,
            day_time: world.day_time,
            moon_phase: world.moon_phase,
            // `NPC.cs:372`, `raining = Main.raining`.
            raining: world.raining,
            // `BirthdayParty.PartyIsUp`, resolved by the caller off `game/party.rs`.
            party: events.party,
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
        // `GetZombieSettings` (`NPC.cs:1286`), which vanilla draws at the top of `SpawnAnNPC` and so
        // reaches only once the cap and the rate roll have both let an attempt through. Here for the
        // same reason: it costs one `rng` draw, and put above the rate roll it would cost that draw
        // on every player on every tick instead of on the one attempt in six hundred that gets this
        // far. It is drawn once per attempt rather than once per candidate tile, which is one draw
        // fewer than the game makes and changes nothing: the style is independent of where in the
        // box the candidate lands.
        let zombie = zombie_settings(player.life_max <= 100, active_players, rng);
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
            // both, in the game and here. `ignoreSafeWalls` is exactly one thing: standing inside a
            // lunar pillar's zone (`NPC.cs:404-409`), where a walled arena does not keep the
            // escort out. It has to be honoured, or a player could simply wall the pillar in and
            // stand there while a shield that only falls to kills never moved.
            //
            // Without this test, a fully walled and fully lit base spawned zombies inside itself.
            let chosen = world.tile(x, from_y);
            if (chosen.is_active() && solid(chosen.block))
                || (tower.is_none() && terrustia_proto::housing::wall_encloses(chosen.wall))
            {
                continue;
            }

            // High enough up, the tile is taken where it is and nothing walks down to ground
            // (`NPC.cs:979-986`, and see [`sky_tile`]): the sky is a place, not a shortfall of
            // ground, and a Harpy has no floor. Without this branch the descent below threw every
            // sky candidate away, so nothing that lives up there could ever be chosen: the Harpy
            // and the Wyvern were unreachable in this server outright.
            //
            // A friendly attempt is excluded by the game at the same point it decides this, so it
            // is excluded here too: `!spawnFriendly` is part of both `skyMob` branches. So is
            // `!invaders` (`NPC.cs:981`, `:983`), which is why a tower zone never reaches the sky
            // and its escort always walks down to ground.
            let sky = tower.is_none()
                && !spawn_friendly
                && sky_tile(world, x, from_y, events.hard_mode, rng);

            // Drop to whatever ground is under the chosen point, then stand on top of it. The
            // descent stops at the bottom of the spawn box, as `NPC.cs:990-993` does, rather than a
            // fixed 30 tiles: from the top of the box that is up to 94 tiles, which is the reach an
            // ocean needs.
            let y = if sky {
                from_y
            } else {
                let Some(ground) = find_ground_within(world, x, from_y, py + SPAWN_RANGE_Y) else {
                    continue;
                };
                ground - 1
            };

            // Never spawn on top of somebody.
            if (x - px).abs() < SAFE_RANGE_X && (y - py).abs() < SAFE_RANGE_Y {
                continue;
            }
            // `xRange` (`NPC.cs:1003`), which is only meaningful once the safe box above has been
            // cleared: the candidate is still inside the safe *column* band, so it sits above or
            // below the player rather than off to one side. Three of the beach and water critter
            // arms refuse to place anything there ([`friendly_chain`]), because what they place is
            // several rows away from the candidate and would land on the player's head.
            let x_range = (x - px).abs() < SAFE_RANGE_X;
            // `HasTileSpawnSpace` asks only for open space; the floor half of [`has_room`] belongs
            // to the descent, which a sky tile skipped.
            if !(if sky {
                open_space(world, x, y)
            } else {
                has_room(world, x, y)
            }) {
                continue;
            }

            // A pillar's zone owns the spawn chain outright. Vanilla's four `ZoneTower*` arms sit
            // at the head of `SpawnAnNPC`'s `else if` chain (`NPC.cs:1297-1372`), ahead of the sky,
            // the invasions, the water, every biome and every moon, so inside a zone the only
            // things that appear are that pillar's own escort. This is the whole lunar event: the
            // shield is a count of escort killed (`game/lunar.rs`), so with nothing spawning the
            // four pillars could not be damaged at all and the Moon Lord was unreachable.
            if let Some(pillar) = tower {
                let alive_count = |ty: u16| {
                    npcs.iter()
                        .filter(|(_, n)| n.npc_type == ty && n.is_alive())
                        .count()
                };
                let Some(npc_type) = tower_pool(pillar, &alive_count, rng) else {
                    continue;
                };
                out.push((npc_type, (x as f32 * 16.0, y as f32 * 16.0)));
                break;
            }

            let depth = depth_at(world, y);
            // The two desert branches are decided at the *tile*, not from the player's zone, which
            // is what lets them be tested on every candidate: the underground desert is two wall
            // reads (`underground_desert_spot`) and the sandstorm is one tile read plus a sand
            // check that breaks out of its own first row on anything else. Neither goes near
            // `biome_at`.
            let desert_spot = !sky && underground_desert_spot(world, x, y);
            // The game's `tileType`: the ground tile the spawn stands on, which this server's row
            // `y` sits one above. Read once here rather than per branch, since three of them want
            // it. A sky spawn has no ground under it and so has no `tileType` at all.
            let ground_block = (!sky).then(|| world.tile(x, y + 1).block);
            // The game's `waterTile` (`NPC.cs:1058`), read once here because two arms want it: the
            // water chain itself and the Fungo Fish at the head of the Glowing Mushroom roster. A
            // sky spawn has no ground and never reaches either.
            let wet = !sky && water_tile(world, x, y);
            let sandstorm_spot = sandstorm_zone
                && ground_block.is_some_and(|g| {
                    SAND_CONVERSION.contains(&g) && sandstone_check(world, x, y + 1)
                });
            // `tileType == 70`, and nothing else: the whole Glowing Mushroom roster hangs off the
            // ground tile rather than off `ZoneGlowshroom` (`NPC.cs:3637`, `:3674`, and see
            // [`mushroom_pick`]).
            let mushroom_ground = ground_block == Some(MUSHROOM_GRASS);
            // The Spore pair's own gate, which wants the zone *and* the tile
            // (`NPC.cs:5110`, `:5209`).
            let glowshroom_ground =
                zones.glowshroom && matches!(ground_block, Some(MUSHROOM_GRASS | MUSHROOM_BLOCK));
            // An event owns the surface while it runs, and nothing below it. Except the sky:
            // vanilla's `skyMob` arm sits above every event arm in the same `else if` chain
            // (`NPC.cs:1383`), so a pumpkin moon does not reach up there.
            let event_type = if !sky && events.running() && depth == Depth::Surface {
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
                // The sky answers for itself, ahead of every other branch, because vanilla's own
                // `else if (skyMob)` sits ahead of them (`NPC.cs:1383`): above the sky line there
                // is no biome to read, no water to stand in and no event to hold the surface.
                None if sky => {
                    // `flag5` (`NPC.cs:1387-1391`): a probe scouts the outer two thirds of the
                    // map, at somebody standing under open sky, and not while anything else is
                    // already going on.
                    let half = world.width() / 2;
                    let probe_gate = (x - half).abs() as f32 / half as f32 > 0.33
                        && sky_behind_player(player_wall)
                        && !events.any_danger;
                    let alive =
                        |ty: u16| npcs.iter().any(|(_, n)| n.npc_type == ty && n.is_alive());
                    sky_pick(
                        events.hard_mode,
                        probe_gate,
                        world,
                        no_worms,
                        &alive,
                        conditions.luck,
                        rng,
                    )
                }
                // A graveyard's Statue Mimic (`NPC.cs:1571-1574`):
                //
                // ```csharp
                // else if (downedBoss3 && ZoneGraveyard && !noWorms && RollBadLuckExtreme(25) == 0
                //     && !AnyNPCs(690) && IsThisAGoodPlaceForAStatueMimic(spawnTileX, spawnTileY))
                // ```
                //
                // The arm sits this high in vanilla's chain too: one below the invasions, and above
                // the spider nest, both deserts, all the water, the dungeon, the meteor and every
                // biome pool. So it carries no depth and no biome gate of its own, and none is
                // invented here: a graveyard is a graveyard wherever the tombstones are.
                //
                // Its AI and its immortal-until-provoked pose were already built and tested
                // (`game::ai::mimic`, and `IMMORTAL_TYPE` in `game::server`); the spawn was the one
                // missing half, so nothing this server could do put one in a world.
                //
                // The conditions are in vanilla's own order, which is also cheapest-first: the two
                // free bools, then `noWorms`, then a one-in-twenty-five roll, and only then the
                // store scan and the eight tile reads of the plinth check.
                None if world.progress.downed_boss3
                    && seasonal.graveyard
                    && !no_worms
                    && rolls_extremely_unlucky(conditions.luck, STATUE_MIMIC_ODDS, rng)
                    && !npcs
                        .iter()
                        .any(|(_, n)| n.npc_type == STATUE_MIMIC && n.is_alive())
                    && good_place_for_a_statue_mimic(world, x, y + 1) =>
                {
                    STATUE_MIMIC
                }
                // Inside a Living Tree, at or above the surface line, on dry ground: one attempt in
                // two is a Gnome (`NPC.cs:1586-1628`).
                //
                // ```csharp
                // else if (wallType == 244 && !Main.remixWorld)
                // {
                //     if (waterTile) { ... 592 / 55 ... }
                //     else if ((double)spawnTileY > Main.worldSurface) { ... 447/300, 359, 448, 357 ... }
                //     else if (RollLuck(1 + gnomeChance / 10) == 0) { SpawnNPC(..., 624).timeLeft *= 10; }
                //     else if ... 443, 539, 303, 337, 540, 299/538, 46 ...
                // }
                // ```
                //
                // `gnomeChance` is 10 in an ordinary world ([`GNOME_CHANCE`]), so `RollLuck(2)`: a
                // living tree above ground is the one place in the game where a Gnome is *likely*
                // rather than rare, and this is what makes the type reachable at all. The arm sits
                // exactly where vanilla's does, one below the Statue Mimic and one above the spider
                // nest, so it is ahead of every water branch and of `spawnFriendly`.
                //
                // Two disclosed narrowings. The rest of the chain (the mouse, snail, worm, gold
                // bunny, gold squirrel and the holiday and party bunnies at `:1588-1656`) is the
                // ordinary-critter roster rather than this one, and is left to fall through to the
                // pools below, so a living tree that declines the gnome roll draws whatever the
                // place ordinarily has. And `timeLeft *= 10`, which lets a gnome linger ten times as
                // long as anything else, is not applied: [`try_spawn`] hands back a type and a
                // position, and the despawn timer is `NpcStore::spawn`'s.
                //
                // `spawn_wall_type` is three tile reads, so the two free tests go first.
                None if !sky
                    && !wet
                    && f64::from(y + 1) <= f64::from(world.surface)
                    && spawn_wall_type(world, x, y) == LIVING_WOOD_WALL
                    && rolls_lucky(conditions.luck, 1 + GNOME_CHANCE / 10, rng) =>
                {
                    GNOME
                }
                // A spider nest, which owns its own arm the way the desert below owns its
                // (`NPC.cs:1662`), one row above it in vanilla's chain and therefore one arm above
                // it here. The wall is read on the game's own `spawnTileY`, the solid ground tile,
                // which is this server's `y + 1`; it carries no depth or biome gate of its own,
                // because vanilla's does not either.
                None if world.tile(x, y + 1).wall == SPIDER_WALL => {
                    spider_pick(events.hard_mode, rng)
                }
                // The underground desert, which is its own chain and answers for everything in it
                // (`NPC.cs:1682-1765`). It sits here because that is where vanilla puts it: ahead of
                // every water branch, ahead of `spawnFriendly`, ahead of `ZoneDungeon` and ahead of
                // every biome pool. A friendly attempt reaching a sandstone wall therefore draws a
                // ghoul rather than a scorpion, which is the game's own ordering rather than an
                // oversight here.
                None if desert_spot => {
                    let alive_count = |ty: u16| {
                        npcs.iter()
                            .filter(|(_, n)| n.npc_type == ty && n.is_alive())
                            .count()
                    };
                    underground_desert_pick(
                        world,
                        y,
                        events.hard_mode,
                        player_biome,
                        no_worms,
                        &alive_count,
                        rng,
                    )
                }
                // A friendly attempt draws a harmless critter for this place; if there is no
                // critter for it, the attempt is dropped rather than turned into a monster the game
                // would not have spawned here.
                None if spawn_friendly => {
                    // A graveyard's friendly draw is its own and it is exactly two things
                    // (`NPC.cs:2101-2115`): a Maggot or a Rat, on dry land, and nothing at all
                    // standing in water. The arm sits ahead of every other friendly one and
                    // `return`s, so no bird, bunny, firefly, nightcrawler or lava-bait critter is
                    // drawn among the tombstones. That is why the two branches below it are below
                    // it: tombstones in the underworld really do answer with a Rat in the game.
                    let critters = if seasonal.graveyard {
                        if water_tile(world, x, y) {
                            continue;
                        }
                        &GRAVEYARD_VERMIN[..]
                    } else if depth == Depth::Underworld {
                        // The underworld's friendly draw is not a pool at all: vanilla's grass case
                        // hands every spawn below `Main.UnderworldLayer` straight to
                        // `SpawnLavaBaitCritters` (`NPC.cs:2562-2578`), which is the same chain the
                        // underworld's ordinary arm calls further down. So this is answered here
                        // rather than through [`friendly_pool`], whose underworld entry is empty
                        // for exactly that reason.
                        out.push((
                            lava_bait_pick(world.day_time, rng),
                            (x as f32 * 16.0, y as f32 * 16.0),
                        ));
                        break;
                    } else if events.starfall_night
                        && !world.day_time
                        && depth == Depth::Surface
                        && rng.random_range(0..ENCHANTED_NIGHTCRAWLER_ODDS) == 0
                    {
                        // The Enchanted Nightcrawler, at the head of the grass chain
                        // (`NPC.cs:2409-2413`, see [`ENCHANTED_NIGHTCRAWLER`]): half of every
                        // friendly surface attempt on a clear meteor-shower night, ahead of the
                        // firefly, the owl and every bird. Without it 484 was in no pool and no
                        // branch.
                        out.push((ENCHANTED_NIGHTCRAWLER, (x as f32 * 16.0, y as f32 * 16.0)));
                        break;
                    } else {
                        // ...and below the graveyard sit vanilla's own friendly chains: the beach,
                        // the cattail dragonflies, the water and the rain (`NPC.cs:2116-2407`).
                        // Each of them `return`s or `break`s in the game, so where one answers the
                        // pool below is not reached at all. They place their own NPCs because they
                        // place them at rows the candidate did not name: on the water's surface, a
                        // row above it, or at a cattail top thirty columns away.
                        if let Some(ground) = ground_block
                            && let Some(drawn) = friendly_chain(
                                world,
                                x,
                                y + 1,
                                ground,
                                wet,
                                x_range,
                                events.wind_target,
                                conditions.luck,
                                rng,
                            )
                        {
                            if drawn.is_empty() {
                                continue;
                            }
                            out.extend(drawn);
                            break;
                        }
                        friendly_pool(depth, player_biome, world.day_time)
                    };
                    if critters.is_empty() {
                        // Empty is not the same as nothing: below the surface line vanilla's own
                        // tail is a gated fork rather than a set, so the two layers down there hand
                        // the draw to [`deep_friendly_pick`] instead. It is where all fourteen gem
                        // critters come from, and where the plain Bunny of the dirt layer does. The
                        // underworld really is empty and falls through to the `continue`.
                        //
                        // No costume ever reaches a gem critter: vanilla's Halloween, Christmas and
                        // party bunnies (`NPC.cs:2584-2599`) all carry `!flag11`, so nothing under
                        // the rock layer is dressed up. `holiday_costume` only matches the plain
                        // Bunny, so passing this through it dresses the dirt layer's bunny exactly
                        // where vanilla dresses it and leaves the caverns untouched.
                        let Some(deep) = deep_friendly_pick(depth, rng) else {
                            continue;
                        };
                        holiday_costume(deep, depth, seasonal, rng)
                    } else {
                        let critter = critters[rng.random_range(0..critters.len())];
                        // A frog is a chain rather than a draw, and vanilla runs that chain exactly
                        // where this pool answers 361 (`NPC.cs:2363`, `:3831` -> `SpawnFrog`). No
                        // holiday costume applies: `holiday_costume` only ever dresses a bunny or a
                        // plain slime, and the jungle-grass arm is answered ahead of the case it
                        // reads.
                        let drawn = if critter == FROG {
                            let alive = |ty: u16| {
                                npcs.iter().any(|(_, n)| n.npc_type == ty && n.is_alive())
                            };
                            spawn_frog(world, &alive, conditions.luck, rng)
                        } else if critter == OWL {
                            // The owl is the other chain in this pool, for the same reason
                            // (`NPC.cs:2442-2448` -> `spawn_owl`): one draw in a hundred is the Owl
                            // Mimic instead. No holiday costume applies here either.
                            spawn_owl(conditions.luck, rng)
                        } else if critter == FIREFLY && ground_block == Some(HALLOWED_GRASS) {
                            // ...and the firefly is a two-way fork on the tile underfoot rather than
                            // a single type (`NPC.cs:2416-2420`, see [`LIGHTNING_BUG`]).
                            // `ground_block` is the read this attempt already made, not a second one.
                            LIGHTNING_BUG
                        } else if critter == BUTTERFLY {
                            // ...and the butterfly is a fork on the wind, because a windy day
                            // replaces it with a ladybug outright (`NPC.cs:2487-2534` ->
                            // [`spawn_butterfly`]). Its own gold twin is [`gold_variant`]'s below;
                            // the ladybug's is not, because a ladybug is not a butterfly by the
                            // time that swap is made.
                            spawn_butterfly(events.wind_target.abs() >= 0.4, conditions.luck, rng)
                        } else {
                            critter
                        };
                        // Gold first and the costume second, which is vanilla's own order: the gold
                        // bunny's arm (`NPC.cs:2580`) sits above the Halloween one (`:2588`), and a
                        // gold critter is never dressed up because it is no longer a bunny by then.
                        holiday_costume(
                            gold_variant(drawn, depth, conditions.luck, rng),
                            depth,
                            seasonal,
                            rng,
                        )
                    }
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
                // vanilla orders it: every `waterTile` branch (`NPC.cs:1766-2085`) sits ahead of
                // every land branch, so the sea gets sharks and the beach gets zombies. The chain
                // declining is not the same as there being no water roster here: vanilla's arms are
                // an `else if` ladder, so a decline reaches the land branches below, which is why
                // this is a guard that lets the match fall through rather than a `continue`.
                None if wet
                    && let Some(npc_type) = water_pick(
                        world,
                        x,
                        y + 1,
                        player_biome,
                        events.hard_mode,
                        ground_block,
                        rng,
                    ) =>
                {
                    npc_type
                }
                // A meteor crater, which answers with one thing and nothing else
                // (`NPC.cs:2796-2799`). The arm sits where vanilla's does: below the water and the
                // dungeon, above the whole fallthrough that holds every biome, season and tile
                // type. Standing in a crater there are Meteor Heads and there is nothing else, and
                // that is the entire point of the place.
                None if zones.meteor => METEOR_HEAD,
                // The underground fairy, sitting exactly where vanilla puts it: below the eclipse
                // chain that ends at `NPC.cs:3614` and directly above the two Gnome arms below
                // (`:3616-3624`, gate in [`underground_fairy`]).
                //
                // ```csharp
                // else if (CheckToSpawnUndergroundFairy(spawnTileX, spawnTileY))
                // {
                //     int type3 = Main.rand.Next(583, 586);
                //     if (Main.tenthAnniversaryWorld && !Main.getGoodWorld && Main.rand.Next(4) != 0) type3 = 583;
                //     SpawnNPC(spawnTileX * 16 + 8, spawnTileY * 16, type3, 0, 0f, 0f, 2f).TargetClosest();
                // }
                // ```
                //
                // The three colours are drawn flat and mean nothing: `Main.rand.Next(583, 586)` is
                // the whole of it, and a fairy's behaviour does not read its own type anywhere. The
                // tenth-anniversary arm drops for the same reason [`underground_fairy`]'s own doc
                // gives: real state (`world.secret_seeds.tenth_anniversary`), no flag plumbed
                // through `EventSpawns` to read it.
                //
                // `ai2 = 2f` is the fairy's `state::APPROACH` (`game::ai::fairy`), so one arrives
                // already coming for the player rather than drifting where it appeared, which is
                // the difference between meeting one in a cave and never knowing it was there. It
                // is also what `AnyHelpfulFairies` reads, so without it the gate below could never
                // suppress a second. `NpcStore::spawn` seeds it, beside the dungeon library mimic's
                // own dormancy, because this function answers with types and not with `ai` slots.
                None if !sky && {
                    let busy = || {
                        npcs.iter().any(|(_, n)| {
                            matches!(n.npc_type, FAIRY_CRITTER_PINK..=585)
                                && n.is_alive()
                                && n.ai[2] > 1.0
                        })
                    };
                    underground_fairy(
                        world,
                        events.fairy_log,
                        events.hard_mode,
                        y + 1,
                        &busy,
                        conditions.luck,
                        rng,
                    )
                } =>
                {
                    FAIRY_CRITTER_PINK + rng.random_range(0..3u16)
                }
                // The other two Gnomes, both sitting immediately above the Glowing Mushroom arm in
                // vanilla's own chain (`NPC.cs:3625` and `:3629`) and therefore immediately above it
                // here. Neither is reachable on a friendly attempt in the game: `spawnFriendly` is
                // answered at `:2099` and never falls this far, which is why they sit below the
                // friendly arm here too.
                //
                // The first is for a player standing inside a Living Tree, wherever in the column
                // the candidate happens to be:
                //
                // ```csharp
                // else if (!Main.remixWorld && !waterTile && (!Main.dayTime || Main.tile[spawnTileX, spawnTileY].wall > 0)
                //          && livingTree && !Main.eclipse && !Main.bloodMoon && RollLuck(gnomeChance * 3) == 0
                //          && CountNPCS(624) <= Main.rand.Next(3))
                // ```
                //
                // `livingTree` is `Main.tile[pX, pY].wall == 244` (`NPC.cs:414`), read at the
                // player's own tile and not at the candidate's, which is why this is `player_wall`
                // where the arm above is `spawn_wall_type`.
                None if !sky
                    && !wet
                    && (!world.day_time || world.tile(x, y + 1).wall > 0)
                    && player_wall == LIVING_WOOD_WALL
                    && !world.eclipse
                    && !world.blood_moon
                    && rolls_lucky(conditions.luck, GNOME_CHANCE * 3, rng)
                    && npcs
                        .iter()
                        .filter(|(_, n)| n.npc_type == GNOME && n.is_alive())
                        .count()
                        <= rng.random_range(0..3) =>
                {
                    GNOME
                }
                // ...and the second is the ordinary one: a dirt-walled cave in the narrow band
                // around the surface line (`NPC.cs:3629-3632` -> [`underground_gnome`]). Its
                // `isAValidZoneAndTile` argument is the caller's, written out at the call site in
                // the game as `!ZoneCorrupt && !ZoneCrimson && !waterTile`.
                None if !sky && {
                    let alive = |ty: u16| {
                        npcs.iter()
                            .filter(|(_, n)| n.npc_type == ty && n.is_alive())
                            .count()
                    };
                    underground_gnome(
                        world,
                        x,
                        y + 1,
                        !matches!(player_biome, Biome::Corruption | Biome::Crimson) && !wet,
                        &alive,
                        conditions.luck,
                        rng,
                    )
                } =>
                {
                    GNOME
                }
                // The Glowing Mushroom roster (`NPC.cs:3637-3702`), which asks only what the spawn
                // is standing on. It declines one attempt in three, and every attempt at all
                // underground before hardmode, and the ordinary chain below answers those.
                None if mushroom_ground
                    && let Some(npc_type) = mushroom_pick(
                        depth == Depth::Surface,
                        events.hard_mode,
                        wet,
                        conditions.luck,
                        rng,
                    ) =>
                {
                    npc_type
                }
                // The four critters the ground below the surface line has of its own, in vanilla's
                // own order and at vanilla's own odds (`NPC.cs:3776-3805`, see [`LAC_BEETLE`]).
                // Their place in the chain is where vanilla puts them: below the Glowing Mushroom
                // arm at `:3637` and above the Lihzahrd temple at `:3914`.
                //
                // These are not friendly-attempt draws. Vanilla spawns them on the monster path,
                // which is why a cave has worms and mice in it whether or not a town nearby has
                // quieted the wild, and why all four were unreachable here: until now nothing in
                // this server drew a critter outside a `spawnFriendly` attempt.
                //
                // `spawnTileY > Main.worldSurface` is exactly `depth != Surface`: the game's
                // `spawnTileY` is the solid row, this server's `y + 1`, and `depth_at` puts `y` in
                // `Surface` for `y < surface`, so the two comparisons agree row for row.
                //
                // The biome exclusions are vanilla's own `!ZoneSnow && !ZoneCrimson && !ZoneCorrupt
                // && !ZoneJungle && !ZoneHallow`, read here as the single winning [`Biome`] this
                // file picks everywhere. The dungeon is added because `else if (ZoneDungeon)`
                // (`:2629`) returns long before this arm, so a dungeon never reaches it in the game
                // either. The Lac Beetle carries none of them: its gate is jungle grass underfoot
                // and nothing else, and the Snail's arm is the one that leaves the jungle in.
                // Doctor Bones, the arm immediately above the Lac Beetle's in vanilla's own chain
                // (`NPC.cs:3772`, see [`DOCTOR_BONES`]). Jungle grass and a dark sky, at any depth
                // and in any biome, which is the whole of his gate.
                None if ground_block == Some(JUNGLE_GRASS)
                    && !world.day_time
                    && rng.random_range(0..DOCTOR_BONES_ODDS) == 0 =>
                {
                    DOCTOR_BONES
                }
                None if depth != Depth::Surface
                    && ground_block == Some(JUNGLE_GRASS)
                    && rng.random_range(0..LAC_BEETLE_ODDS) == 0 =>
                {
                    LAC_BEETLE
                }
                None if depth != Depth::Surface
                    && y + 1 < world.height() - CRITTER_FLOOR
                    && !matches!(
                        player_biome,
                        Biome::Snow
                            | Biome::Crimson
                            | Biome::Corruption
                            | Biome::Jungle
                            | Biome::Hallow
                            | Biome::Dungeon
                    )
                    && rng.random_range(0..WORM_ODDS) == 0 =>
                {
                    gold_variant(WORM, depth, conditions.luck, rng)
                }
                None if depth != Depth::Surface
                    && y + 1 < world.height() - CRITTER_FLOOR
                    && !matches!(
                        player_biome,
                        Biome::Snow
                            | Biome::Crimson
                            | Biome::Corruption
                            | Biome::Jungle
                            | Biome::Hallow
                            | Biome::Dungeon
                    )
                    && rng.random_range(0..MOUSE_ODDS) == 0 =>
                {
                    gold_variant(MOUSE, depth, conditions.luck, rng)
                }
                // The Snail's own gate is not the other two's: it keeps the jungle (`NPC.cs:3802`
                // omits `!ZoneJungle`) and it stops at the middle of the stone rather than ten rows
                // above the underworld.
                None if depth != Depth::Surface
                    && y + 1 < (i32::from(world.rock_layer) + world.height()) / 2
                    && !matches!(
                        player_biome,
                        Biome::Snow
                            | Biome::Crimson
                            | Biome::Corruption
                            | Biome::Hallow
                            | Biome::Dungeon
                    )
                    && rng.random_range(0..SNAIL_ODDS) == 0 =>
                {
                    SNAIL
                }
                // The Lihzahrd Temple, which answers with its own two and nothing else
                // (`NPC.cs:3914-3924`, see [`LIHZAHRD`]). The arm sits exactly where vanilla puts
                // it: below the Glowing Mushroom arm at `:3637` and above the sandstorm at `:3952`,
                // inside the same fallthrough both of those live in.
                //
                // No progression gate of any kind, and none is invented here: vanilla's arm never
                // reads `downedPlantBoss`. The Lihzahrd Power Cell door is what actually keeps a
                // player out of the temple before Plantera, and anybody who gets in early meets the
                // residents. The zone is the player's (a Lihzahrd brick wall at their own tile) and
                // the ground is the candidate's, which is exactly how the game splits it.
                //
                // One disclosed narrowing, the same one the Goblin Scout and Skeleton Merchant arms
                // carry: two arms above this one in vanilla's chain are not modelled here at all and
                // so never decline in this server's favour. `:3802`'s Grasshopper is the only critter
                // arm up there a temple can reach (it excludes snow, both evils and the hallow, but
                // not the jungle a temple reads as), and `:3737`'s one-in-seventy-five souls arm has
                // no biome gate at all in hardmode. The effect is that these two are very slightly
                // commoner here than in the game, never rarer.
                None if temple_zone
                    && matches!(ground_block, Some(LIHZAHRD_BRICK | WOODEN_SPIKES)) =>
                {
                    if rng.random_range(0..FLYING_SNAKE_ODDS) == 0 {
                        FLYING_SNAKE
                    } else {
                        LIHZAHRD
                    }
                }
                // A sandstorm over the desert, which owns the surface while it blows
                // (`NPC.cs:3952-4022`). It sits after the water and the dungeon, as vanilla's does,
                // and pushes its own result because one member of it does not stand where it was
                // chosen: the Dune Splicer burrows in ten tiles down.
                None if sandstorm_spot => {
                    let alive_count = |ty: u16| {
                        npcs.iter()
                            .filter(|(_, n)| n.npc_type == ty && n.is_alive())
                            .count()
                    };
                    let (npc_type, drop) = sandstorm_pick(
                        events.hard_mode,
                        world.progress.downed_boss1,
                        no_worms,
                        ground_block.unwrap_or_default(),
                        &alive_count,
                        rng,
                    );
                    out.push((npc_type, (x as f32 * 16.0, (y + drop) as f32 * 16.0)));
                    break;
                }
                // Hallowed ground, which has a chain of its own ahead of the hallow pool
                // (`NPC.cs:4039-4061`, and see [`hallow_ground_pick`] for the whole of it). It sits
                // here because that is where vanilla puts it: below the sandstorm and the four sand
                // conversions, above the two evils' own tile arms and above `else if (surfaceSpawn)`.
                //
                // Keyed on the tile underfoot rather than on [`Biome::Hallow`], because vanilla's is:
                // the gate is `tileType == 116 || 117 || 109 || 164` with no `ZoneHallow` anywhere in
                // it, so a single pearlstone ledge in an otherwise ordinary cavern roof is hallowed
                // ground for this purpose and a hallowed biome floored in plain stone is not.
                // `underGround` (`NPC.cs:1144`) is the dirt layer and above, so the caverns are out.
                //
                // One disclosed reorder: vanilla tests `!waterTile` before the four tile types, and
                // this tests the tiles first. Both are pure, so the branch taken is the same; the
                // order is chosen so the array scan shuts the gate before `water_tile`'s own tile
                // read is made. Measured at 0.89 ns a candidate on ordinary stone and 16.85 ns for
                // the whole chain on the one night it can answer with everything
                // ([`tests::measure_the_hallowed_ground_arm`]).
                None if events.hard_mode
                    && matches!(depth, Depth::Surface | Depth::Underground)
                    && ground_block.is_some_and(|g| HALLOW_GROUND.contains(&g))
                    && !water_tile(world, x, y)
                    && let Some(npc_type) = hallow_ground_pick(
                        world.progress.downed_plantera,
                        world.day_time,
                        world.time,
                        depth == Depth::Surface,
                        world.raining,
                        &|ty| npcs.iter().any(|(_, n)| n.npc_type == ty && n.is_alive()),
                        conditions.luck,
                        rng,
                    ) =>
                {
                    npc_type
                }
                // The underworld arm's own first branch (`NPC.cs:4877`):
                //
                // ```csharp
                // else if (Main.hardMode && !savedTaxCollector && Main.rand.Next(20) == 0 && !AnyNPCs(534))
                // ```
                //
                // It sits ahead of everything else the underworld spawns, which is why it is a
                // branch here rather than an entry in the pool: at one in twenty it is a fifth of
                // the whole underworld draw while it is open, and it closes for good once the world
                // has its Tax Collector. Without it the Tortured Soul never spawned, and since he
                // is the only way a Tax Collector ever exists, an entire townsperson (his shop,
                // his happiness, his arrival) was unreachable.
                None if depth == Depth::Underworld
                    && events.hard_mode
                    && !world.progress.saved_tax_collector
                    && rng.random_range(0..TORTURED_SOUL_ODDS) == 0
                    && !npcs
                        .iter()
                        .any(|(_, n)| n.npc_type == TORTURED_SOUL && n.is_alive()) =>
                {
                    TORTURED_SOUL
                }
                // ...and the arm immediately below his (`NPC.cs:4881-4884`), one underworld draw in
                // eight: the lava-bait critters, which are the only three creatures down there that
                // are not trying to kill anybody. See [`lava_bait_pick`] for the fork itself.
                //
                // It shares the Tortured Soul's `Depth::Underworld` gate, and with it the one
                // disclosed narrowing that gate already carried: vanilla's own arm opens at
                // `spawnTileY > Main.maxTilesY - 190` (`:4871`) and [`UNDERWORLD_DEPTH`] is 200, so
                // the ten rows between them draw from the cavern chain here where the game would
                // already be in the underworld.
                None if depth == Depth::Underworld && rng.random_range(0..LAVA_BAIT_ODDS) == 0 => {
                    lava_bait_pick(world.day_time, rng)
                }
                // The caverns' Rock Golem (`NPC.cs:4921-4924`):
                //
                // ```csharp
                // else if (CheckToSpawnRockGolem(spawnTileX, spawnTileY, tileType))
                // {
                //     SpawnNPC(spawnTileX * 16 + 8, spawnTileY * 16, 631);
                // }
                // ```
                //
                // A branch rather than a pool entry, because its gate is a *tile* gate with a
                // ceiling check on it ([`check_to_spawn_rock_golem`]) rather than anything a
                // depth-and-biome pool can express: a golem comes out of the plain stone or out of
                // moss, and only where there is a hole tall enough to hold one.
                //
                // It sits exactly where vanilla puts it. The arm is reached only once `underGround`
                // (`spawnTileY <= Main.rockLayer`, `NPC.cs:1144`) and the underworld arm at
                // `:4871` have both declined, which is this server's `Cavern` and nothing else, and
                // it is one row above the whole cavern chain the two arms below transcribe
                // (`:4925` onward, and `:5004`'s Skeleton Merchant). No biome exclusion beyond
                // vanilla's own `ZoneSnow`: the two evils and the jungle divert on their *tiles*
                // (`:4066`, `:4125`, `:3929-3948`) rather than their zones, and plain stone in a
                // corrupted cavern really does grow golems in the game. The dungeon is the one
                // exception, because `else if (ZoneDungeon)` (`:2629`) returns long before this.
                //
                // `ZoneSnow` is read here as `player_biome == Biome::Snow`, a narrowing this file
                // makes everywhere: the game's zones are independent flags and [`Biome`] picks one
                // winner, so a snowy place that reads as something else still grows golems here.
                //
                // 631's AI is already wired: it is `ai_style` 3, so `game::ai::fighter` walks it,
                // chases and hits. What is not transcribed is its own stand-and-throw block inside
                // `AI_003_Fighters` (`NPC.cs:56866-56931`, projectile 909), which is an AI-lane gap
                // rather than a spawn one and is left named rather than half-built.
                None if depth == Depth::Cavern
                    && player_biome != Biome::Dungeon
                    && let Some(ground) = ground_block
                    && check_to_spawn_rock_golem(
                        world,
                        x,
                        y + 1,
                        ground,
                        events.hard_mode,
                        player_biome == Biome::Snow,
                        rng,
                    ) =>
                {
                    ROCK_GOLEM
                }
                // ...and the arm immediately below the golem's (`NPC.cs:4925-4935`, see
                // [`CAVERN_BEETLE_ODDS`]): one cavern draw in sixty is a beetle, cyan in the snow
                // and cochineal everywhere else. It carries the golem's own two gates and nothing
                // more, because vanilla's carries none of its own at all.
                None if depth == Depth::Cavern
                    && player_biome != Biome::Dungeon
                    && rng.random_range(0..CAVERN_BEETLE_ODDS) == 0 =>
                {
                    if player_biome == Biome::Snow {
                        CYAN_BEETLE
                    } else {
                        COCHINEAL_BEETLE
                    }
                }
                // The cavern chain's rare wanderer (`NPC.cs:5004-5010`):
                //
                // ```csharp
                // else if (Main.rand.Next(2) == 0)
                // {
                //     if (Main.rand.Next(35) == 0 && !ZoneShadowCandle && !waterTile && CountNPCS(453) == 0)
                // ```
                //
                // One in seventy, at any point in a world's progression: he is not a hardmode or a
                // dungeon spawn, whatever the wikis say. Two deliberate narrowings, both of which
                // only make him very slightly more common than the game does. `ZoneShadowCandle`
                // is not modelled here at all, and vanilla reaches this arm only after the cavern
                // chain's earlier branches decline, whose per-tile conditions this server does not
                // read. The biome exclusion is that chain's own upstream tile diversions
                // (`NPC.cs:4066`, `:4125` for the two evils, `:3929-3948` for jungle mud): those
                // tiles are answered before the fallthrough he lives in, so he is not found there.
                None if depth == Depth::Cavern
                    && !matches!(
                        player_biome,
                        Biome::Corruption | Biome::Crimson | Biome::Jungle | Biome::Dungeon
                    )
                    && !water_tile(world, x, y)
                    && rng.random_range(0..SKELETON_MERCHANT_ODDS) == 0
                    && !npcs
                        .iter()
                        .any(|(_, n)| n.npc_type == SKELETON_MERCHANT && n.is_alive()) =>
                {
                    SKELETON_MERCHANT
                }
                None => {
                    let biome = player_biome;
                    // The surface *day* has a chain of its own too, and out at the edges of the map
                    // it holds the one thing in it that is not a critter (`NPC.cs:4482-4485`,
                    // inside `if (!ZoneGraveyard && Main.dayTime)` at `:4202`):
                    //
                    // ```csharp
                    // else if (!waterTile && (num45 > Main.maxTilesX / 3 || Main.remixWorld)
                    //     && (Main.rand.Next(15) == 0
                    //         || (!downedGoblins && WorldGen.shadowOrbSmashed
                    //             && Main.rand.Next(7) == 0)))
                    // ```
                    //
                    // `num45` is `Math.Abs(spawnTileX - Main.spawnTileX)` (`NPC.cs:4204`), the
                    // distance from the *world's* spawn point rather than from the player: a scout
                    // is something met out at the edges, never in the back garden. Both rolls sit
                    // under that one distance gate and are OR'd rather than exclusive, so a smashed
                    // shadow orb adds a second chance on top of the first rather than replacing it.
                    //
                    // Without this the Goblin Scout was in no pool and no branch, so Tattered Cloth
                    // never dropped, so the Goblin Battle Standard could not be crafted, so the
                    // goblin army could not be summoned: the recipe (`recipes.rs`), the drop
                    // (`npc_drops.rs`) and the invasion (`on_summon`'s `-1`) were all already here
                    // and all three were unreachable behind this one missing arm.
                    //
                    // Three narrowings, each disclosed rather than invented. `Main.remixWorld` is
                    // not modelled anywhere in this server, so the distance gate is the whole gate.
                    // The sand exclusion stands in for vanilla's three `tileType == 53` arms above
                    // this one, which answer every dry sand tile first, so a beach and a desert keep
                    // their own daytime residents. And the critter arms above it are not modelled at
                    // all, which only makes him slightly more common here than in the game, exactly
                    // as with the Skeleton Merchant's arm below.
                    let surface_day = depth == Depth::Surface
                        && world.day_time
                        && !seasonal.graveyard
                        && !matches!(
                            biome,
                            Biome::Corruption | Biome::Crimson | Biome::Jungle | Biome::Dungeon
                        );
                    // Above the scout in that same chain, and the one thing in it that is neither
                    // critter nor enemy: a distressed Palworld pet, cornered by two goblins
                    // (`NPC.cs:4374-4389`).
                    //
                    // ```csharp
                    // else if (!waterTile && num45 > Main.maxTilesX / 8
                    //     && (tileType == 2 || tileType == 147 || tileType == 60 || tileType == 161)
                    //     && Main.rand.Next(160) == 0 && !AnyNPCs(696) && !AnyNPCs(695))
                    // {
                    //     bool flag18 = Main.player[target].HasItem(5663);
                    //     bool flag19 = Main.player[target].HasItem(5664);
                    //     bool flag20 = RollLuck(100) < 40;
                    //     if (!flag19 & flag18) flag20 = true;
                    //     if (!flag18 & flag19) flag20 = false;
                    //     short type5 = (short)(flag20 ? 696 : 695);
                    //     SpawnNPC(spawnTileX * 16 + 8, spawnTileY * 16, type5);
                    // }
                    // ```
                    //
                    // The distance gate is `num45` again, the same read the scout below uses, and a
                    // looser one: an eighth of the world out from the spawn point rather than a
                    // third. `tileType == 60` is jungle grass, which the `surface_day` biome
                    // exclusion does not contradict: it excludes the jungle *zone*, and vanilla
                    // reaches this chain only outside it too (`NPC.cs:3929-3948` answers a jungle
                    // first), while a stray jungle-grass block on an ordinary surface reaches both.
                    //
                    // `!wet` is the `waterTile` this attempt already read rather than a second one;
                    // under `surface_day` the two are the same read, since `wet` differs from
                    // `water_tile` only by a sky test and the sky is not the surface.
                    if surface_day
                        && !wet
                        && (x - i32::from(world.spawn_x)).abs() > world.width() / 8
                        && matches!(
                            ground_block,
                            Some(GRASS | SNOW_BLOCK | JUNGLE_GRASS | ICE_BLOCK)
                        )
                        && rng.random_range(0..PAL_ODDS) == 0
                        && !npcs.iter().any(|(_, n)| {
                            matches!(n.npc_type, PAL_CATTIVA | PAL_FOXSPARKS) && n.is_alive()
                        })
                    {
                        out.push((
                            pal_type(player, conditions.luck, rng),
                            (x as f32 * 16.0, y as f32 * 16.0),
                        ));
                        break;
                    }
                    if surface_day
                        && ground_block != Some(SAND)
                        && !water_tile(world, x, y)
                        && (x - i32::from(world.spawn_x)).abs() > world.width() / 3
                        && (rng.random_range(0..GOBLIN_SCOUT_ODDS) == 0
                            || (!world.progress.downed_goblins
                                && world.progress.shadow_orb_smashed
                                && rng.random_range(0..GOBLIN_SCOUT_ORB_ODDS) == 0))
                    {
                        out.push((GOBLIN_SCOUT, (x as f32 * 16.0, y as f32 * 16.0)));
                        break;
                    }
                    // ...and immediately below him in that same chain, the four the surface day only
                    // has in weather: two in the rain (`NPC.cs:4486-4493`) and two on a windy day
                    // (`:4494-4501`). See [`FLYING_FISH`] and [`WINDY_BALLOON`] for the arms
                    // themselves. All four sit under `surface_day`, so they carry its gate and its
                    // three already-disclosed narrowings unchanged.
                    //
                    // `wet` is the `waterTile` read the attempt already made further up, not a
                    // second one: the whole of what these arms add to a spawn scan is at most two
                    // wall reads and four rolls, and only on a tile that got past every arm above.
                    if surface_day && world.raining && rng.random_range(0..FLYING_FISH_ODDS) == 0 {
                        out.push((FLYING_FISH, (x as f32 * 16.0, y as f32 * 16.0)));
                        break;
                    }
                    if surface_day
                        && !wet
                        && world.raining
                        && rng.random_range(0..UMBRELLA_SLIME_ODDS) == 0
                    {
                        out.push((UMBRELLA_SLIME, (x as f32 * 16.0, y as f32 * 16.0)));
                        break;
                    }
                    // `isSpawningInWindDirection` (`NPC.cs:1202`):
                    //
                    // ```csharp
                    // isSpawningInWindDirection = (float)(pX - spawnTileX) * Main.windSpeedTarget > 0f;
                    // ```
                    //
                    // `pX` is the player's own tile column, so this is "the wind blows from the
                    // spawn tile toward the player" and nothing else. A balloon and a dandelion's
                    // seeds only ever reach somebody downwind, which is why both arms ask.
                    //
                    // `&&` short-circuits, so the two wall reads behind `wallType` happen only on a
                    // windy day at a dry surface tile that got this far, which is a small fraction
                    // of a small fraction of attempts. Every other tick pays one bool test.
                    let open_sky = surface_day
                        && !wet
                        && events.happy_windy_day
                        && (px - x) as f32 * events.wind_target > 0.0
                        && spawn_wall_type(world, x, y) == 0;
                    if open_sky && rng.random_range(0..WINDY_BALLOON_ODDS) != 0 {
                        out.push((WINDY_BALLOON, (x as f32 * 16.0, y as f32 * 16.0)));
                        break;
                    }
                    // Vanilla's last clause here, `Main.tile[spawnTileX, spawnTileY].type ==
                    // tileType`, is a re-read of the tile `tileType` was taken from and so is a
                    // no-op except where `FindGroundTile` walked a row down first (`NPC.cs:5882`).
                    // It does that for exactly the two tiles in
                    // `TileID.Sets.InheritTypeOfTileBelowForNPCSpawning` (`TileID.cs:355`: 421 and
                    // 422, the two conveyor belts), which this server does not model, so the clause
                    // is not transcribed rather than being transcribed as something it is not.
                    if open_sky
                        && matches!(ground_block, Some(GRASS | GOLF_GRASS))
                        && rng.random_range(0..DANDELION_ODDS) != 0
                    {
                        out.push((DANDELION, (x as f32 * 16.0, y as f32 * 16.0)));
                        break;
                    }
                    // The surface night has a chain of its own ahead of the pool, and a graveyard
                    // reaches it in daylight too (`NPC.cs:4202`). It answers only where vanilla's
                    // `else if (surfaceSpawn)` (`NPC.cs:4168`) is reached: the corruption, the
                    // crimson, the jungle and the dungeon are all answered by earlier arms of the
                    // outer chain and never get here.
                    let seasonal_ground = depth == Depth::Surface
                        && (!world.day_time || seasonal.graveyard)
                        && !matches!(
                            biome,
                            Biome::Corruption | Biome::Crimson | Biome::Jungle | Biome::Dungeon
                        );
                    if seasonal_ground
                        && let Some(npc_type) = seasonal_night_pick(
                            seasonal,
                            zombie,
                            world.tile(x, y + 1).block,
                            conditions.luck,
                            rng,
                        )
                    {
                        out.push((npc_type, (x as f32 * 16.0, y as f32 * 16.0)));
                        break;
                    }
                    // The caverns have a chain of their own ahead of their pool, and it is where
                    // October and a graveyard reach underground (`NPC.cs:5005-5199`). The biome
                    // exclusions are vanilla's own diversions above it: the two evils and the
                    // jungle answer at `NPC.cs:4066`, `:4125` and `:3929-3948` on their tiles, the
                    // dungeon has its own arm, and the snow's `:5088`/`:5100` both return before
                    // the season is ever asked. One tile read (the stone underfoot, for the marble
                    // and granite arms) plus three comparisons and at most six `rng` draws, on a
                    // path that already made several.
                    let cavern_chain = depth == Depth::Cavern
                        && !matches!(
                            biome,
                            Biome::Corruption
                                | Biome::Crimson
                                | Biome::Jungle
                                | Biome::Dungeon
                                | Biome::Snow
                        );
                    // `NPC.cs:5021`, the bottom half of the stone.
                    let lower_caverns = y > (i32::from(world.rock_layer) + world.height()) / 2;
                    let alive =
                        |ty: u16| npcs.iter().any(|(_, n)| n.npc_type == ty && n.is_alive());
                    if cavern_chain
                        && let Some(npc_type) = cavern_seasonal_pick(
                            seasonal,
                            no_worms,
                            lower_caverns,
                            world.tile(x, y + 1).block,
                            glowshroom_ground,
                            &alive,
                            rng,
                        )
                    {
                        out.push((npc_type, (x as f32 * 16.0, y as f32 * 16.0)));
                        break;
                    }
                    // The dungeon's two chains, which sit ahead of its pool exactly as vanilla puts
                    // `NPC.cs:2661-2722` and then `:2723-2747` ahead of `:2748`. The brick style is
                    // read once for both, as vanilla reads `num40` once at `:2631-2645` for the
                    // whole `ZoneDungeon` block: it costs a roll of its own, so asking for it twice
                    // would be a second one.
                    //
                    // The hardmode half opens only once Plantera is down in a hardmode world, which
                    // is `hardDungeon` (`NPC.cs:381`, `downedPlantBoss && Main.hardMode`); the
                    // ordinary half runs in every dungeon there has ever been.
                    if biome == Biome::Dungeon {
                        let alive =
                            |ty: u16| npcs.iter().any(|(_, n)| n.npc_type == ty && n.is_alive());
                        let style = dungeon_brick_style(world, x, y, rng);
                        if events.hard_mode && events.downed_plantera {
                            match hard_dungeon_pick(style, &alive, rng) {
                                Some(Some(npc_type)) => {
                                    out.push((npc_type, (x as f32 * 16.0, y as f32 * 16.0)));
                                    break;
                                }
                                // The caster arm fired and its one-at-a-time gate turned it away,
                                // which vanilla answers by returning with nothing spawned.
                                Some(None) => continue,
                                None => {}
                            }
                        }
                        if let Some(npc_type) = dungeon_pick(
                            style,
                            &|| near_spike_ball(npcs, x, y + 1),
                            conditions.luck,
                            rng,
                        ) {
                            out.push((npc_type, (x as f32 * 16.0, y as f32 * 16.0)));
                            break;
                        }
                    }
                    // The dungeon library, `NPC.cs:2748-2771`:
                    //
                    // ```csharp
                    // bool flag13 = false;
                    // if (Main.rand.Next(8) == 0) {
                    //     Point bookPosition = Point.Zero;
                    //     if (AI_FindNearbyBook(new Point(spawnTileX - 16, spawnTileY - 16), 32, 32,
                    //             out bookPosition, closestBook: true, checkPlayerScreenRanges: true)) {
                    //         SpawnNPC(bookPosition.X * 16 + 8, bookPosition.Y * 16, 694, 0, 0f, 0f, 0f, 3f);
                    //         flag13 = true;
                    //     }
                    // } else if (Main.rand.Next(10) == 0) {
                    //     ...same, 693, and no ai...
                    // }
                    // int num43 = Main.rand.Next(5);
                    // if (flag13) return;
                    // ```
                    //
                    // Neither of the two is placed where the attempt found ground: both stand on the
                    // shelf, wherever in the box it turned out to be, which is why this pushes its
                    // own position rather than falling through to the draw below. With no shelf in
                    // the box `flag13` stays false and the attempt goes on to the ordinary
                    // fallthrough, so a corridor with no library in it is unaffected.
                    //
                    // The mimic's `3f` is its dormant state and is seeded by `NpcStore::spawn`,
                    // which is this server's `NewNPC` and the one door every NPC comes through.
                    //
                    // One thing about the placement is disclosed rather than matched: vanilla draws
                    // `num43` before it reads `flag13`, which costs a roll off its own stream and
                    // nothing else, and this server's `rng` is not that stream, so the dead draw is
                    // not reproduced. The five rolls vanilla makes ahead of this one (`:2723-2747`)
                    // are [`dungeon_pick`] directly above, so the library is now reached at the same
                    // point in the chain the game reaches it.
                    if biome == Biome::Dungeon {
                        let library = if rng.random_ratio(1, WATER_BOLT_MIMIC_ODDS) {
                            Some(WATER_BOLT_MIMIC)
                        } else if rng.random_ratio(1, LIBRARIAN_ODDS) {
                            Some(LIBRARIAN_SKELETON)
                        } else {
                            None
                        };
                        // `spawnTileY` is the ground tile, which is this server's `y + 1`: the same
                        // one-row offset every other tile test in this module carries.
                        if let Some(npc_type) = library
                            && let Some((bx, by)) = find_nearby_book(
                                world,
                                (x - BOOK_SEARCH_BACK, y + 1 - BOOK_SEARCH_BACK),
                                (px, py),
                            )
                        {
                            out.push((npc_type, (bx as f32 * 16.0, by as f32 * 16.0)));
                            break;
                        }
                    }
                    // A graveyard's daylight draws the *night* pool, for the same reason: with the
                    // daytime block skipped, the chain below `NPC.cs:4202` is the one that answers,
                    // and its fallthrough is the zombie (`NPC.cs:4770-4816`), not the day's slime.
                    let day = world.day_time && !seasonal.graveyard;
                    let ordinary = pool(depth, biome, day);
                    // Hardmode adds to what a place had rather than replacing it, so a hardmode
                    // forest still has zombies in it. The underworld's additions wait for a
                    // mechanical boss, which is progression rather than place and so is held here.
                    let extra = if events.hard_mode
                        && (depth != Depth::Underworld || events.downed_mech_any)
                    {
                        hardmode_pool(depth, biome, day)
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
                        // Two after-the-draw swaps, both of which decline for everything they are
                        // not about: a costume for the bunny and the surface slime, and a family
                        // for the hornet ([`hornet_variant`], which is what makes a hive throw
                        // Fatties and Stingies rather than five plain Hornets in eight).
                        hornet_variant(holiday_costume(ty, depth, seasonal, rng), rng)
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

    /// Every NPC type this server can put into a world by itself, from every ambient producer it
    /// has, with no console command and nobody summoning anything.
    ///
    /// Built by *calling* the producers rather than by reading them, so it cannot drift from what
    /// they actually answer. It is the half of the reachability check that has to run here;
    /// vanilla's half needs the decompiled tree and lives in `tools/check_spawn_reach.py`, which
    /// runs this test to get this set. A type in vanilla's list and not in this one is an NPC
    /// nobody playing here can ever meet, which is exactly how the Harpy and the Wyvern went
    /// missing.
    ///
    /// Deliberately not counted as reachable, because none of them is ambient spawning: statues,
    /// the admin `/spawn` command, boss summon items, the transformations one NPC undergoes on
    /// another's death, and the segments a worm head grows behind itself.
    fn ambient_roster() -> std::collections::BTreeSet<u16> {
        use crate::game::{army, cavern_monsters, event::Invasion, lunar, moons, rescues};

        let mut set = std::collections::BTreeSet::new();
        const DEPTHS: [Depth; 4] = [
            Depth::Surface,
            Depth::Underground,
            Depth::Cavern,
            Depth::Underworld,
        ];
        const BIOMES: [Biome; 9] = [
            Biome::Forest,
            Biome::Corruption,
            Biome::Crimson,
            Biome::Jungle,
            Biome::Snow,
            Biome::Desert,
            Biome::Ocean,
            Biome::Dungeon,
            Biome::Hallow,
        ];
        for depth in DEPTHS {
            for biome in BIOMES {
                for day in [true, false] {
                    set.extend(pool(depth, biome, day));
                    set.extend(hardmode_pool(depth, biome, day));
                    set.extend(friendly_pool(depth, biome, day));
                }
            }
            for hard_mode in [true, false] {
                set.extend(blood_moon_pool(depth, hard_mode));
            }
        }

        // The sky, asked through its own chain rather than listed, so deleting an arm of it shows
        // up here as a type that stopped being reachable.
        let mut sky = World::empty(800, 600, "roster");
        sky.progress.downed_golem = true;
        for seed in 0..200u64 {
            for hard_mode in [false, true] {
                for probe_gate in [false, true] {
                    let mut rng = SmallRng::seed_from_u64(seed);
                    set.insert(sky_pick(
                        hard_mode,
                        probe_gate,
                        &sky,
                        false,
                        &|_| false,
                        0.0,
                        &mut rng,
                    ));
                }
            }
        }
        // The water chain, asked through its own producer for the same reason: it is an `else if`
        // ladder, not a pool, so an arm that stops answering shows up here as a type that stopped
        // being reachable. Sampled across every input that changes what it can answer: the biome,
        // hardmode, the ground tile underfoot, and three rows (above the surface line, below it,
        // and below the rock layer).
        let mut sea = World::empty(800, 600, "roster");
        sea.surface = 200;
        sea.rock_layer = 300;
        for seed in 0..400u64 {
            for biome in BIOMES {
                for hard_mode in [false, true] {
                    for ground_block in [None, Some(JUNGLE_GRASS)] {
                        for ground_y in [150, 260, 350] {
                            let mut rng = SmallRng::seed_from_u64(seed);
                            set.extend(water_pick(
                                &sea,
                                400,
                                ground_y,
                                biome,
                                hard_mode,
                                ground_block,
                                &mut rng,
                            ));
                        }
                    }
                }
            }
        }

        // The two desert chains, asked through their own producers for the same reason the sky and
        // the pillars are: deleting an arm of one shows up here as a type that stopped being
        // reachable, rather than as nothing at all. Both are sampled across every input that
        // changes what they can answer - progression, the player's zone, whether burrowers are
        // allowed, and (for the sandstorm) which sand is underfoot.
        let mut desert = World::empty(800, 600, "roster");
        desert.surface = 200;
        desert.rock_layer = 300;
        for seed in 0..400u64 {
            for hard_mode in [false, true] {
                for biome in [
                    Biome::Forest,
                    Biome::Corruption,
                    Biome::Crimson,
                    Biome::Hallow,
                ] {
                    for no_worms in [false, true] {
                        // Two rows: one below `worldSurface + 100` where the worms are allowed, and
                        // one above it where they are not.
                        for spawn_y in [250, 350] {
                            let mut rng = SmallRng::seed_from_u64(seed);
                            set.insert(underground_desert_pick(
                                &desert,
                                spawn_y,
                                hard_mode,
                                biome,
                                no_worms,
                                &|_| 0,
                                &mut rng,
                            ));
                        }
                    }
                }
            }
            for hard_mode in [false, true] {
                for downed_boss1 in [false, true] {
                    for no_worms in [false, true] {
                        for ground in SAND_CONVERSION {
                            let mut rng = SmallRng::seed_from_u64(seed);
                            set.insert(
                                sandstorm_pick(
                                    hard_mode,
                                    downed_boss1,
                                    no_worms,
                                    ground,
                                    &|_| 0,
                                    &mut rng,
                                )
                                .0,
                            );
                        }
                    }
                }
            }
        }
        // The spider nest's two, asked through their own producer for the same reason: it is the
        // only place in the game either of them comes from.
        for hard_mode in [false, true] {
            for seed in 0..200u64 {
                let mut rng = SmallRng::seed_from_u64(seed);
                set.insert(spider_pick(hard_mode, &mut rng));
            }
        }
        // The friendly branch's own chains, asked through [`friendly_chain`]. One shore world with
        // forty rows of standing water over a stone floor, wide enough that the outer 380 columns
        // (the beach) and the middle band (everything else) are both reachable.
        //
        // A single rng runs the whole block rather than one reseeded per call: several of these arms
        // end in a one-in-four-hundred gold roll, and reseeding from a small counter would hand the
        // same few streams to every combination instead of sampling them independently.
        // The surface line sits below the floor rather than on it, because the beach arm's own scan
        // for the top of the water is gated on `spawnTileY < Main.worldSurface` (`NPC.cs:2122`),
        // strictly, where every other test in the arm is `<=`. With the two equal there is no water
        // surface to find and neither the Sea Turtle nor the Dolphin can be drawn at all.
        let mut shore = World::empty(1200, 600, "roster");
        shore.surface = 260;
        shore.rock_layer = 350;
        for x in 0..1200 {
            for y in 200..212 {
                shore.set_tile(x, y, terrustia_proto::Tile::block(1));
            }
            for y in 160..200 {
                shore.set_tile(
                    x,
                    y,
                    terrustia_proto::Tile::AIR
                        .with_liquid(terrustia_proto::tile::Liquid::Water, 255),
                );
            }
        }
        shore.day_time = true;
        let mut rng = SmallRng::seed_from_u64(11);
        let sample = |world: &World, x, ground, wet, wind, rng: &mut SmallRng| {
            friendly_chain(world, x, 200, ground, wet, false, wind, 0.0, rng)
                .unwrap_or_default()
                .into_iter()
                .map(|(ty, _)| ty)
                .collect::<Vec<_>>()
        };
        // The beach, wet and dry. The wet half is the Sea Turtle, Dolphin, Seahorse, Pufferfish and
        // Seagull chain; the dry half is a Seagull and nothing else.
        for wet in [true, false] {
            for _ in 0..80_000 {
                set.extend(sample(&shore, 100, GRASS, wet, 0.0, &mut rng));
            }
        }
        // Inland water, once per ground the arm keys on: jungle grass and sand for the water
        // striders, the Jungle Turtle and the Grebe; grass for the Turtle; a fourth ground for the
        // ducks; and the goldfish, gold goldfish and Pupfish all three arms fall through to.
        for ground in [JUNGLE_GRASS, SAND, GRASS, HALLOWED_GRASS] {
            for _ in 0..80_000 {
                set.extend(sample(&shore, 700, ground, true, 0.0, &mut rng));
            }
        }
        // The rain's grass arm, sampled on a windy day so the cattail scan above it is skipped.
        shore.raining = true;
        for _ in 0..80_000 {
            set.extend(sample(&shore, 700, GRASS, false, 0.5, &mut rng));
        }
        // ...and the cattail dragonflies, in still air on their own small world, because this is the
        // one arm that walks 2501 tiles per call and it needs far fewer of them.
        let mut marsh = World::empty(900, 400, "roster");
        marsh.surface = 200;
        marsh.raining = true;
        marsh.day_time = true;
        marsh.set_tile(450, 195, terrustia_proto::Tile::framed(CATTAIL, 180, 0));
        for ground in [GRASS, SAND] {
            for _ in 0..20_000 {
                set.extend(sample(&marsh, 450, ground, false, 0.0, &mut rng));
            }
        }
        // The butterfly's own chain, which is where the two ladybugs live, and the dragonfly roll,
        // which the cattail arm above calls into.
        for _ in 0..40_000 {
            set.insert(spawn_butterfly(false, 0.0, &mut rng));
            set.insert(spawn_butterfly(true, 0.0, &mut rng));
            set.insert(dragonfly_type(GRASS, &mut rng));
            set.insert(dragonfly_type(SAND, &mut rng));
        }
        // The Gnome, whose three arms are the only place in the game one comes from. Two of them are
        // bare conditions in [`try_spawn`]'s own chain (a Living Tree wall at the candidate, and a
        // player standing inside one); the third is a producer, asked here so that deleting it shows
        // up as a type that stopped being reachable rather than as nothing at all.
        let mut burrow = World::empty(800, 600, "roster");
        burrow.surface = 200;
        let mut dirt_walled = terrustia_proto::Tile::block(0);
        dirt_walled.wall = GNOME_WALLS[0];
        burrow.set_tile(400, 200, dirt_walled);
        for _ in 0..400 {
            if underground_gnome(&burrow, 400, 200, true, &|_| 0, 0.0, &mut rng) {
                set.insert(GNOME);
            }
        }
        // ...and the fairy directly above it, the same way: its gate is a producer, its answer is
        // three colours drawn flat in [`try_spawn`]'s own arm. `burrow` is 600 rows deep with its
        // surface at 200 and its rock layer at 300, so the gate's window is row 250 (their halfway
        // line) up to row 299 (`height - 300`, exclusive), and 275 sits inside it.
        for _ in 0..20_000 {
            if underground_fairy(&burrow, true, false, 275, &|| false, 0.0, &mut rng) {
                for colour in 0..3u16 {
                    set.insert(FAIRY_CRITTER_PINK + colour);
                }
            }
        }
        // The surface night's own chain, asked through itself for the same reason the sky is:
        // deleting an arm shows up here as a type that stopped being reachable. Every combination
        // of the flags it reads, since each one opens arms the others do not, both moon phases it
        // names, and two floors, because the snow arm keys on the tile underfoot.
        //
        // `expert` is one of those flags now: the whole armed-zombie family hangs off it and off
        // nothing else. The zombie settings are drawn from the same seeded stream, so the seven
        // styles and both torch chances all turn up across a run of seeds without another loop.
        for halloween in [false, true] {
            for xmas in [false, true] {
                for graveyard in [false, true] {
                    for hard_mode in [false, true] {
                        for expert in [false, true] {
                            for blood_moon in [false, true] {
                                for raining in [false, true] {
                                    for moon_phase in [0u8, 4] {
                                        let at = Seasonal {
                                            halloween,
                                            xmas,
                                            graveyard,
                                            hard_mode,
                                            expert,
                                            blood_moon,
                                            day_time: false,
                                            moon_phase,
                                            raining,
                                            party: false,
                                        };
                                        for ground_block in [2u16, 147] {
                                            // Both signs of luck, because one arm of this chain is
                                            // reachable only by being unlucky: the Moss Zombie's
                                            // `RollOnlyBadLuckExtreme(30)` (`NPC.cs:4712`) has no
                                            // fallthrough and returns `-1` at or above zero. A
                                            // roster probed at neutral luck alone would report it
                                            // unreachable, which is what `docs/spawn-gaps.tsv`
                                            // said for as long as the server had no luck at all.
                                            for luck in [0.0f32, -0.7] {
                                                for seed in 0..4_000u64 {
                                                    let mut rng = SmallRng::seed_from_u64(seed);
                                                    let zombie = zombie_settings(
                                                        seed % 2 == 0,
                                                        (seed % 8) as u32,
                                                        &mut rng,
                                                    );
                                                    set.extend(seasonal_night_pick(
                                                        at,
                                                        zombie,
                                                        ground_block,
                                                        luck,
                                                        &mut rng,
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // ...and the caverns' own chain, asked the same way. Both `no_worms` states, because the
        // Ghost's arm is gated on one of them, and both halves of the stone, because Tim's is.
        // Three floors, because two of the arms key on the stone underfoot: plain rock, marble
        // (367) and granite (368).
        //
        // `hard_mode` and `expert` are paired rather than crossed: hardmode turns nine draws in ten
        // away before the Bone Throwing Skeletons are ever asked (`NPC.cs:5051`), so the expert arm
        // needs a classic world to be sampled properly and the fourth combination adds nothing.
        for halloween in [false, true] {
            for graveyard in [false, true] {
                for (hard_mode, expert) in [(false, false), (false, true), (true, false)] {
                    let at = Seasonal {
                        halloween,
                        graveyard,
                        hard_mode,
                        expert,
                        ..Seasonal::default()
                    };
                    for no_worms in [false, true] {
                        for lower_caverns in [false, true] {
                            // Both halves of the chain have a glowshroom arm, and each is the only
                            // source of its own Spore, so the flag is sampled like the rest.
                            for glowshroom in [false, true] {
                                for ground_block in [1u16, 367, 368] {
                                    for seed in 0..4_000u64 {
                                        let mut rng = SmallRng::seed_from_u64(seed);
                                        set.extend(cavern_seasonal_pick(
                                            at,
                                            no_worms,
                                            lower_caverns,
                                            ground_block,
                                            glowshroom,
                                            &|_| false,
                                            &mut rng,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // ...and hallowed ground's own chain, asked the same way. Every flag it reads opens an arm
        // the others do not: Plantera down and the first half of the night for the Lacewing, rain
        // for the Rainbow Slime, and the surface for the Lacewing again. With nothing alive, since
        // two of its arms are gated on a census of themselves.
        for downed_plantera in [false, true] {
            for day_time in [false, true] {
                for raining in [false, true] {
                    for surface_spawn in [false, true] {
                        for seed in 0..4_000u64 {
                            let mut rng = SmallRng::seed_from_u64(seed);
                            set.extend(hallow_ground_pick(
                                downed_plantera,
                                day_time,
                                0,
                                surface_spawn,
                                raining,
                                &|_| false,
                                0.0,
                                &mut rng,
                            ));
                        }
                    }
                }
            }
        }

        // ...and the hardmode dungeon's, asked the same way. All three brick styles, because three
        // quarters of that roster is chosen by which one a spot is built from, and with nothing
        // alive, because two of its arms are gated on a census of themselves.
        for style in 0..3u8 {
            for seed in 0..4_000u64 {
                let mut rng = SmallRng::seed_from_u64(seed);
                set.extend(hard_dungeon_pick(style, &|_| false, &mut rng).flatten());
            }
        }

        // ...and the ordinary dungeon's, asked the same way and for the same reason: the brick style
        // decides three of its five, and with no spike ball anywhere near, since that is the one
        // arm gated on a scan of the world rather than on a roll.
        for style in 0..3u8 {
            for seed in 0..4_000u64 {
                let mut rng = SmallRng::seed_from_u64(seed);
                set.extend(dungeon_pick(style, &|| false, 0.0, &mut rng));
            }
        }

        // The Glowing Mushroom roster, asked through its own chain for the same reason the sky and
        // the deserts are. Both arms, and both progression states, since the underground one is
        // hardmode-only and the surface one gives its snail a second chance before hardmode.
        for surface in [false, true] {
            for hard_mode in [false, true] {
                for wet in [false, true] {
                    for seed in 0..4_000u64 {
                        let mut rng = SmallRng::seed_from_u64(seed);
                        set.extend(mushroom_pick(surface, hard_mode, wet, 0.0, &mut rng));
                    }
                }
            }
        }

        // The holiday costumes, which swap a critter or a plain slime for a dressed-up one after it
        // is drawn rather than sitting in a pool of their own. The party is the third of them and
        // is sampled like the two calendars: it is where the Party Bunny lives, and nowhere else.
        for at in [
            Seasonal {
                halloween: true,
                ..Seasonal::default()
            },
            Seasonal {
                xmas: true,
                ..Seasonal::default()
            },
            Seasonal {
                party: true,
                ..Seasonal::default()
            },
        ] {
            for base in [46, 1] {
                for seed in 0..200u64 {
                    let mut rng = SmallRng::seed_from_u64(seed);
                    set.insert(holiday_costume(base, Depth::Surface, at, &mut rng));
                }
            }
        }

        // ...and the gold variants, which are the same shape of after-the-draw swap and are asked
        // the same way: every base that has a twin, at both depths, since the squirrel's is
        // surface-only. One draw in four hundred, so the seed count is what the odds ask for.
        for base in [74, 297, 298, 46, 356, 361, 300, 357, 55, 299] {
            for depth in [Depth::Surface, Depth::Cavern] {
                for seed in 0..20_000u64 {
                    let mut rng = SmallRng::seed_from_u64(seed);
                    set.insert(gold_variant(base, depth, 0.0, &mut rng));
                }
            }
        }

        // ...and the third swap of that shape, the hornet's family. One base and no depth fork, so
        // a handful of seeds covers all six of its eighths.
        for seed in 0..200u64 {
            let mut rng = SmallRng::seed_from_u64(seed);
            set.insert(hornet_variant(HORNET, &mut rng));
        }

        // The underworld's lava-bait chain, asked through its own producer for the same reason the
        // sky is: both halves of its day fork, and enough seeds for the Magma Snail's third.
        for day_time in [false, true] {
            for seed in 0..200u64 {
                let mut rng = SmallRng::seed_from_u64(seed);
                set.insert(lava_bait_pick(day_time, &mut rng));
            }
        }

        // ...and what a friendly attempt draws in a graveyard, which is its own two-entry list
        // rather than an arm of `friendly_pool`.
        set.extend(GRAVEYARD_VERMIN);

        // The frog's own chain, asked through itself for the same reason the sky is: it is where
        // the bound Yellow Slime lives, and deleting its arm should show up here as a type that
        // stopped being reachable rather than as nothing at all.
        let jungle = World::empty(800, 600, "roster");
        for seed in 0..200u64 {
            let mut rng = SmallRng::seed_from_u64(seed);
            set.insert(spawn_frog(&jungle, &|_| false, 0.0, &mut rng));
        }

        // ...and the owl's, for the same reason again: it is where the Owl Mimic lives, and at one
        // draw in a hundred it needs more seeds than the frog before the rare arm comes up at all.
        for seed in 0..2_000u64 {
            let mut rng = SmallRng::seed_from_u64(seed);
            set.insert(spawn_owl(0.0, &mut rng));
        }

        // The friendly chain below the surface line, which `friendly_pool` deliberately leaves
        // empty: the dirt layer's plain Bunny, and the caverns' fourteen gem critters. The rarest
        // of the fourteen is a one-in-fifteen fork, then a one-in-five gate, then five draws in a
        // hundred on the gem ladder, so a Gem Bunny Diamond wants roughly one seed in a hundred and
        // fifty and this needs a long run before all fourteen have come up once.
        for depth in DEPTHS {
            for seed in 0..20_000u64 {
                let mut rng = SmallRng::seed_from_u64(seed);
                set.extend(deep_friendly_pick(depth, &mut rng));
            }
        }

        // The dungeon's doorman, and the residents found tied up underground.
        set.insert(DUNGEON_GUARDIAN);
        set.extend(rescues::RESCUES.iter().map(|r| r.bound));
        // The bound Old Slime is found by the same underground path but is not a talk-rescue, so
        // it is not in that table (`pick_bound` chains it in by hand, and `bound_gate` is what
        // limits it to the caverns). The bound Purple Slime is not listed here at all: it comes
        // out of `sky_pick` above, which is where it actually lives.
        set.insert(BOUND_TOWN_SLIME_OLD);
        // The two `try_spawn` answers with a branch of their own rather than a pool entry: the
        // Tortured Soul the underworld offers once per world, and the Skeleton Merchant the caverns
        // offer one attempt in seventy. Both are asserted to be reachable by `try_spawn` itself in
        // this module's own tests, so listing them here cannot drift into a claim their branches
        // have stopped making good.
        set.insert(TORTURED_SOUL);
        set.insert(SKELETON_MERCHANT);
        // ...and the two more with a branch of their own: the Goblin Scout the surface day offers
        // out past a third of the map from spawn, and the Statue Mimic a graveyard offers on a
        // plinth once Skeletron is down. Both are asserted reachable through `try_spawn` itself by
        // this module's own tests, so listing them here cannot drift into a claim that their
        // branches still make good.
        set.insert(GOBLIN_SCOUT);
        set.insert(STATUE_MIMIC);
        // ...and the pair the same surface day offers an eighth of the map out, one attempt in a
        // hundred and sixty, on grass, snow, jungle grass or ice (`NPC.cs:4374-4389`). Both are
        // asserted reachable through `try_spawn` itself by this module's own tests, so listing them
        // here cannot drift into a claim that their arm still makes good.
        set.insert(PAL_CATTIVA);
        set.insert(PAL_FOXSPARKS);
        // ...and the four the same surface day offers in weather rather than in a pool: two in the
        // rain and two on a windy day (`NPC.cs:4486-4501`). All four are asserted reachable through
        // `try_spawn` itself by this module's own tests, so listing them here cannot drift into a
        // claim that their arms still make good.
        set.insert(FLYING_FISH);
        set.insert(UMBRELLA_SLIME);
        set.insert(WINDY_BALLOON);
        set.insert(DANDELION);
        // ...and the Rock Golem the hardmode caverns cut out of stone or moss, whose gate is a tile
        // and a ceiling rather than a pool. Asserted reachable through `try_spawn` itself by this
        // module's own tests, so listing it here cannot drift into a claim its branch still makes
        // good.
        set.insert(ROCK_GOLEM);
        // ...and the temple's two, whose branch is a one-in-three fork with no roster behind it
        // (`NPC.cs:3914-3924`). `try_spawn` reaching both is asserted by this module's own tests,
        // so listing them here cannot drift into a claim the branch still makes good.
        set.insert(LIHZAHRD);
        set.insert(FLYING_SNAKE);
        // ...and the dungeon library's pair, which are a branch rather than a pool entry for a
        // reason no other arm has: neither stands where the attempt found ground, they stand on the
        // bookshelf `find_nearby_book` picked (`NPC.cs:2748-2766`). Both are asserted reachable
        // through `try_spawn` itself by this module's own tests.
        set.insert(WATER_BOLT_MIMIC);
        set.insert(LIBRARIAN_SKELETON);
        // ...and the Meteor Head, whose branch has no roll and no roster at all: standing in a
        // crater, it is the one thing that comes (`NPC.cs:2796-2799`). `try_spawn` reaching it is
        // asserted by this module's own tests, so listing it here cannot drift into a claim the
        // branch still makes good.
        set.insert(METEOR_HEAD);
        // ...and the six the ground itself offers on the monster path rather than through any
        // pool: the four below the surface line (`NPC.cs:3776-3805`) and the caverns' beetle pair
        // (`:4925-4935`). Each is a branch with a roll and a place gate and no roster behind it, so
        // there is nothing to sample; `try_spawn` reaching all six is asserted by this module's own
        // tests, which is what keeps listing them here from drifting into a claim their branches
        // still make good.
        set.insert(LAC_BEETLE);
        // ...and Doctor Bones, whose arm sits directly above the Lac Beetle's and is the same shape:
        // a roll, a tile underfoot, and no roster behind it (`NPC.cs:3772-3775`). `try_spawn`
        // reaching him is asserted by this module's own tests.
        set.insert(DOCTOR_BONES);
        set.insert(WORM);
        set.insert(MOUSE);
        set.insert(SNAIL);
        set.insert(COCHINEAL_BEETLE);
        set.insert(CYAN_BEETLE);
        // ...and the two the friendly chain answers with ahead of its own pool: the Enchanted
        // Nightcrawler on a meteor-shower night (`NPC.cs:2409`) and the Lightning Bug the firefly
        // arm becomes over hallowed grass (`:2416-2420`). Both are asserted reachable through
        // `try_spawn` itself by this module's own tests.
        set.insert(ENCHANTED_NIGHTCRAWLER);
        set.insert(LIGHTNING_BUG);
        // The six a world happens to have are drawn from thirteen, so every world's own set counts.
        for world_id in 0..200 {
            set.extend(cavern_monsters::CavernMonsters::for_world(world_id).flat());
        }
        // King Slime arrives on his own during a slime rain, which nothing else summons.
        set.insert(crate::game::slime_rain::KING_SLIME);

        // The four lunar pillar zones, asked through their own producer for the same reason the
        // sky is asked through its own chain: deleting an arm of one shows up here as a type that
        // stopped being reachable.
        for pillar in lunar::PILLARS {
            for seed in 0..200u64 {
                let mut rng = SmallRng::seed_from_u64(seed);
                set.extend(tower_pool(pillar, &|_| 0, &mut rng));
            }
        }

        // The rosters that already carry a membership test are read through it, which is exact
        // where sampling their spawn functions would only be likely.
        //
        // A membership test is only allowed to stand in for a roster when something actually
        // spawns from that roster. Each of the three below has a real path: the moons through
        // `moons::moon_spawn` in `spawn_at`, invasions through the invasion spawner, and the Old
        // One's Army through `apply_army`.
        //
        // `lunar::belongs_to` was here too and stays out, however reachable the pillar escort has
        // since become. It exists to classify a kill (`Lunar::note_kill`, which drops a pillar's
        // shield) and to decide what despawns when the event ends, and counting it as reachability
        // made the emptiest fight in the game invisible to the one tool built to find exactly that.
        // The escort is above, drawn from [`tower_pool`], which is what actually spawns it.
        for npc_type in 0..terrustia_proto::npc_data::NPC_COUNT {
            if moons::moon_points(npc_type) > 0
                || army::belongs(npc_type)
                || [
                    Invasion::Goblin,
                    Invasion::FrostLegion,
                    Invasion::Pirate,
                    Invasion::Martian,
                ]
                .into_iter()
                .any(|kind| belongs_to(kind, npc_type))
            {
                set.insert(npc_type);
            }
        }

        // The eclipse has no membership table, so its own function is asked directly, enough times
        // and under both progression states for every arm of it to have answered.
        for seed in 0..4000u64 {
            for (plantera, mechs) in [(false, false), (true, true)] {
                let mut rng = SmallRng::seed_from_u64(seed);
                set.insert(crate::game::moons::eclipse_spawn(
                    plantera,
                    mechs,
                    &|_| 0,
                    &mut rng,
                ));
            }
        }
        set
    }

    /// The reachable set, printed for `tools/check_spawn_reach.py` and checked for the one thing
    /// that needs no decompiled tree: that every producer names a type that exists and can be
    /// spawned at somebody.
    #[test]
    fn every_ambient_producer_names_a_real_type() {
        use terrustia_proto::npc_data::npc_stats;

        let roster = ambient_roster();
        assert!(roster.len() > 150, "only {} types reachable", roster.len());
        for npc_type in &roster {
            assert!(
                npc_stats(*npc_type).is_some(),
                "{npc_type} is reachable from a spawn producer but is not an NPC type",
            );
        }
        println!(
            "SPAWN-REACH {}",
            roster
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    /// Nothing running: what the ordinary world looks like to `try_spawn`.
    fn quiet() -> EventSpawns<'static> {
        EventSpawns {
            moon: None,
            eclipse: false,
            sandstorm: false,
            happy_windy_day: false,
            wind_target: 0.0,
            party: false,
            starfall_night: false,
            fairy_log: false,
            downed_plantera: false,
            downed_all_mechs: false,
            boss_cap: false,
            any_danger: false,
            hard_mode: false,
            downed_mech_any: false,
            census: &|_| 0,
            cavern_monsters: crate::game::cavern_monsters::CavernMonsters::for_world(7),
            towers: [None; 4],
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

        // ...and the same of the chain the two layers below the surface hand their draw to, whose
        // answers never pass through the table above and so would not otherwise be checked at all.
        for depth in [
            Depth::Surface,
            Depth::Underground,
            Depth::Cavern,
            Depth::Underworld,
        ] {
            for seed in 0..2_000u64 {
                let mut rng = SmallRng::seed_from_u64(seed);
                let Some(t) = deep_friendly_pick(depth, &mut rng) else {
                    continue;
                };
                let stats = npc_stats(t).expect("a real critter type");
                assert_eq!(
                    stats.damage, 0,
                    "{depth:?}'s friendly chain names the monster {}",
                    stats.name
                );
            }
        }
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

    /// Run a stretch of spawn ticks for one player standing at a tile, and report what was drawn.
    ///
    /// The NPC store is deliberately left empty between ticks, as the other `try_spawn` tests
    /// leave it: nothing accumulates, so the near-player cap never closes and a run of ticks is a
    /// run of independent attempts.
    fn spawns_at(world: &World, hard_mode: bool, px: i32, py: i32, ticks: u32) -> Vec<u16> {
        spawns_at_in(world, hard_mode, false, px, py, ticks)
    }

    /// The same, for a player who is (or is not) standing in a graveyard.
    ///
    /// The graveyard is a bit in the zone packet the client last sent, exactly as it reaches a real
    /// server: `[id][zone1][zone2][zone3][zone4][zone5][townNPCs]` with `ZoneGraveyard` at
    /// `zone4[6]` (`NetMessage.cs:936-946`, `Player.cs:3771`). Setting the bit here is what a client
    /// standing among twenty-eight tombstones would send.
    fn spawns_at_in(
        world: &World,
        hard_mode: bool,
        graveyard: bool,
        px: i32,
        py: i32,
        ticks: u32,
    ) -> Vec<u16> {
        spawns_with_zone(
            world,
            &EventSpawns {
                hard_mode,
                ..quiet()
            },
            graveyard,
            px,
            py,
            ticks,
        )
        .into_iter()
        .map(|(npc_type, _)| npc_type)
        .collect()
    }

    /// The same with the events under the test's control, and the position kept: a sandstorm has to
    /// be switched on from outside, and one of its members does not stand where it was chosen.
    fn spawns_with(
        world: &World,
        events: &EventSpawns<'_>,
        px: i32,
        py: i32,
        ticks: u32,
    ) -> Vec<(u16, (f32, f32))> {
        spawns_with_zone(world, events, false, px, py, ticks)
    }

    /// The one body the three helpers above are views onto.
    fn spawns_with_zone(
        world: &World,
        events: &EventSpawns<'_>,
        graveyard: bool,
        px: i32,
        py: i32,
        ticks: u32,
    ) -> Vec<(u16, (f32, f32))> {
        spawns_full(
            world,
            &NpcStore::new(),
            events,
            graveyard,
            None,
            px,
            py,
            ticks,
        )
    }

    /// The same with an NPC table that already has something in it, for the arms gated on a census
    /// of themselves.
    fn spawns_with_npcs(
        world: &World,
        npcs: &NpcStore,
        events: &EventSpawns<'_>,
        px: i32,
        py: i32,
        ticks: u32,
    ) -> Vec<(u16, (f32, f32))> {
        spawns_full(world, npcs, events, false, None, px, py, ticks)
    }

    /// ...and the same with an item in the player's inventory, for the one arm that reads it.
    fn spawns_carrying(world: &World, item: i16, px: i32, py: i32, ticks: u32) -> Vec<u16> {
        spawns_full(
            world,
            &NpcStore::new(),
            &quiet(),
            false,
            Some(item),
            px,
            py,
            ticks,
        )
        .into_iter()
        .map(|(npc_type, _)| npc_type)
        .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn spawns_full(
        world: &World,
        npcs: &NpcStore,
        events: &EventSpawns<'_>,
        graveyard: bool,
        carrying: Option<i16>,
        px: i32,
        py: i32,
        ticks: u32,
    ) -> Vec<(u16, (f32, f32))> {
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
        drop(out_rx);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = crate::game::ConnState::Playing;
        player.position = (px as f32 * 16.0, py as f32 * 16.0);
        if graveyard {
            player.zone = Some(bytes::Bytes::from_static(&[0, 0, 0, 0, 1 << 6, 0, 0]));
        }
        if let Some(item) = carrying {
            // Slot 3, an ordinary inventory slot: `Player.HasItem` scans 0 to 57.
            player.inventory.insert(
                3,
                terrustia_proto::inventory::SyncEquipment {
                    player: 0,
                    slot: 3,
                    item: terrustia_proto::item::ItemStack::new(i32::from(item), 1, 0),
                    favorited: false,
                    blocked: false,
                },
            );
        }
        let players = vec![Some(player)];

        let mut rng = SmallRng::seed_from_u64(20260830);
        let mut seen = Vec::new();
        let mut biomes = BiomeCache::default();
        for _ in 0..ticks {
            seen.extend(try_spawn(
                world,
                npcs,
                &players,
                events,
                &JourneyPowers::default(),
                &mut biomes,
                &mut rng,
            ));
        }
        seen
    }

    /// A plain forest surface and a place to stand on it.
    ///
    /// Deliberately not `test_world`: the generated world's own spawn point for seed 7 sits in the
    /// corruption, whose surface is answered by an earlier arm of vanilla's chain (`NPC.cs:4125`)
    /// and never reaches the seasonal one. A flat stone floor above the surface line is the
    /// simplest thing `biome_at` calls a forest, which is what these tests are about.
    ///
    /// And deliberately wider than the 800 columns the rest of this module's flat worlds use.
    /// `isBeach` is the outer [`BEACH_DISTANCE`] columns on each side with no tile test at all
    /// (`NPC.cs:1206`), so on an 800-column world the middle forty columns are the only ones that
    /// are not beach, a spawn box [`SPAWN_RANGE_X`] wide cannot fit between them, and every
    /// friendly draw anywhere in the world is the beach's Seagull. That is what the game does on a
    /// world that narrow too; its own smallest is 4200 columns across, so a fixture with an inland
    /// column in it is the honest stand-in for an ordinary surface.
    fn forest_surface() -> (World, (i32, i32)) {
        let world = flat_world_sized(1600, 90, 1);
        let at = (800, 88);
        assert_eq!(biome_at(&world, at.0, at.1), Biome::Forest);
        assert_eq!(depth_at(&world, at.1), Depth::Surface);
        (world, at)
    }

    /// The same, after dark.
    fn night_world() -> (World, (i32, i32)) {
        let (mut world, at) = forest_surface();
        world.day_time = false;
        (world, at)
    }

    /// A graveyard owns the surface night: the Ghost, the Groom, the Bride and the Maggot Zombie
    /// appear nowhere else on an ordinary night (`NPC.cs:4544`, `:4623`, `:4628`, `:4717`), and
    /// before this the server had no notion of a graveyard at all, so none of the four was in any
    /// producer and nobody could ever meet one.
    ///
    /// Neutralised by making `seasonal_night_pick` return `None` on its first line: every one of
    /// the four assertions below fails, and the same run with `graveyard: false` is unaffected,
    /// which is the other half of the claim.
    #[test]
    fn a_graveyard_brings_out_the_ghosts_and_the_wedding() {
        let (world, (px, py)) = night_world();
        // A million ticks rather than two hundred thousand, because the wedding is the rarest thing
        // asserted here and two hundred thousand was not enough to assert it with: 200k ticks place
        // roughly 870 NPCs, and the Groom and the Bride are a `RollOnlyBadLuck(300)` each, so the
        // expected count of each was under three and whether one turned up at all came down to the
        // seed. It did until anything above them in the chain moved the stream, and then it did not.
        let seen = spawns_at_in(&world, false, true, px, py, 1_000_000);
        let found: std::collections::BTreeSet<u16> = seen.iter().copied().collect();
        for (npc_type, name) in [
            (316u16, "Ghost"),
            (53, "TheGroom"),
            (536, "TheBride"),
            (632, "MaggotZombie"),
            (301, "Raven"),
        ] {
            assert!(
                found.contains(&npc_type),
                "no {name} ({npc_type}) in a graveyard: {found:?}"
            );
        }

        // And none of them anywhere else, which is what makes the graveyard the reason they came.
        let ordinary = spawns_at_in(&world, false, false, px, py, 200_000);
        let ordinary: std::collections::BTreeSet<u16> = ordinary.iter().copied().collect();
        for npc_type in [316u16, 53, 536, 632, 301] {
            assert!(
                !ordinary.contains(&npc_type),
                "{npc_type} turned up on an ordinary night: {ordinary:?}"
            );
        }
    }

    /// A graveyard runs the *night* roster in broad daylight: `NPC.cs:4202`'s daytime block is
    /// gated on `!ZoneGraveyard`, so standing among tombstones skips it entirely and drops through
    /// to the chain below, whose fallthrough is the zombie rather than the day's blue slime.
    ///
    /// Neutralised by dropping the `&& !seasonal.graveyard` from the `day` binding in `try_spawn`:
    /// the daylit graveyard then draws the day pool and no zombie appears, failing the assertion.
    #[test]
    fn a_graveyard_is_dark_at_noon() {
        let (world, (px, py)) = forest_surface();
        assert!(world.day_time, "an untouched world starts in daylight");
        let seen = spawns_at_in(&world, false, true, px, py, 20_000);
        assert!(
            seen.contains(&3),
            "no zombies in a daylit graveyard: {:?}",
            seen.iter().collect::<std::collections::BTreeSet<_>>()
        );
    }

    /// A town in a graveyard has vermin instead of songbirds: the friendly branch's own graveyard
    /// arm (`NPC.cs:2101-2115`) sits ahead of every other, so a Maggot or a Rat is the *only* thing
    /// a friendly attempt can draw there.
    ///
    /// Neutralised by dropping the `if seasonal.graveyard` fork in `try_spawn`'s friendly arm, so
    /// both cases read `friendly_pool`: no Maggot or Rat is drawn in an hour of ticks and the first
    /// assertion fails. The second assertion is what makes it a *replacement* rather than an
    /// addition, and it fails the same way in reverse if the arm is made additive.
    #[test]
    fn a_town_among_the_tombstones_draws_vermin_and_nothing_else() {
        const GUIDE: u16 = 22;
        let (world, (px, py)) = forest_surface();
        let mut npcs = NpcStore::new();
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
        drop(out_rx);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = crate::game::ConnState::Playing;
        player.position = (px as f32 * 16.0, py as f32 * 16.0);
        player.zone = Some(bytes::Bytes::from_static(&[0, 0, 0, 0, 1 << 6, 0, 0]));
        // Three townsfolk right where the player stands, which is what turns `spawnFriendly` on.
        for _ in 0..3 {
            npcs.spawn(GUIDE, player.position);
        }
        let players = vec![Some(player)];

        let mut rng = SmallRng::seed_from_u64(21);
        let mut biomes = BiomeCache::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..20_000 {
            for (npc_type, _) in try_spawn(
                &world,
                &npcs,
                &players,
                &quiet(),
                &JourneyPowers::default(),
                &mut biomes,
                &mut rng,
            ) {
                seen.insert(npc_type);
            }
        }
        for vermin in GRAVEYARD_VERMIN {
            assert!(
                seen.contains(&vermin),
                "no {vermin} in a graveyard: {seen:?}"
            );
        }
        for songbird in friendly_pool(Depth::Surface, Biome::Forest, true) {
            assert!(
                !seen.contains(songbird),
                "the ordinary critter {songbird} should not reach a graveyard: {seen:?}"
            );
        }
    }

    /// What a friendly attempt draws with a town around it, which is the only way to reach
    /// [`friendly_pool`] at all: three townsfolk standing on the player is what sets
    /// `spawnFriendly`.
    fn friendly_draws_at(
        world: &World,
        px: i32,
        py: i32,
        ticks: u32,
    ) -> std::collections::BTreeSet<u16> {
        friendly_draws_with(world, &quiet(), px, py, ticks)
    }

    /// The same, with the events under the test's control: the party and the meteor-shower night
    /// are both server state the friendly chain reads, and so is the wind.
    fn friendly_draws_with(
        world: &World,
        events: &EventSpawns<'_>,
        px: i32,
        py: i32,
        ticks: u32,
    ) -> std::collections::BTreeSet<u16> {
        friendly_draws_in(world, events, false, px, py, ticks)
    }

    /// ...and the same again for a player standing among tombstones, whose friendly arm is its own
    /// and answers ahead of every other one.
    fn friendly_draws_in(
        world: &World,
        events: &EventSpawns<'_>,
        graveyard: bool,
        px: i32,
        py: i32,
        ticks: u32,
    ) -> std::collections::BTreeSet<u16> {
        friendly_spawns_in(world, events, graveyard, px, py, ticks)
            .into_iter()
            .map(|(npc_type, _)| npc_type)
            .collect()
    }

    /// The same again with the *position* kept rather than thrown away: most of the beach and water
    /// arms place what they draw at a row the candidate did not name, which is the one thing a set
    /// of types cannot show.
    fn friendly_spawns_with(
        world: &World,
        events: &EventSpawns<'_>,
        px: i32,
        py: i32,
        ticks: u32,
    ) -> Vec<(u16, (f32, f32))> {
        friendly_spawns_in(world, events, false, px, py, ticks)
    }

    /// The one body the four helpers above are views onto.
    fn friendly_spawns_in(
        world: &World,
        events: &EventSpawns<'_>,
        graveyard: bool,
        px: i32,
        py: i32,
        ticks: u32,
    ) -> Vec<(u16, (f32, f32))> {
        const GUIDE: u16 = 22;
        let mut npcs = NpcStore::new();
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
        drop(out_rx);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = crate::game::ConnState::Playing;
        player.position = (px as f32 * 16.0, py as f32 * 16.0);
        // `1 << 6` on byte 4 is the graveyard flag, whose friendly arm answers ahead of every other
        // one; unset by default, so a caller that does not ask for tombstones reads the ordinary
        // pool rather than vermin.
        if graveyard {
            player.zone = Some(bytes::Bytes::from_static(&[0, 0, 0, 0, 1 << 6, 0, 0]));
        }
        for _ in 0..3 {
            npcs.spawn(GUIDE, player.position);
        }
        let players = vec![Some(player)];

        let mut rng = SmallRng::seed_from_u64(689);
        let mut biomes = BiomeCache::default();
        let mut seen = Vec::new();
        for _ in 0..ticks {
            seen.extend(try_spawn(
                world,
                &npcs,
                &players,
                events,
                &JourneyPowers::default(),
                &mut biomes,
                &mut rng,
            ));
        }
        seen
    }

    /// The surface night's owls, and the one in a hundred that is not an owl (`NPC.cs:2439-2449`).
    ///
    /// Both were unreachable: `friendly_pool`'s night arm held the Firefly and nothing else, so no
    /// Owl had a producer anywhere in this server and the Owl Mimic, which exists only as a branch
    /// of the Owl's own draw, could not be met by any means at all. The mimic's routine did not
    /// exist either; the base flight AI (`game::ai::bird`, style 24) did, and what it was missing
    /// was every branch vanilla writes for these two types specifically.
    ///
    /// Neutralised by replacing `spawn_owl`'s body with a bare `OWL`: the Owl Mimic is drawn zero
    /// times in two hundred thousand ticks and the second assertion fails, while the Owl keeps
    /// coming and the first still passes. Dropping `OWL` from `friendly_pool`'s night arm instead
    /// fails both.
    #[test]
    fn the_surface_night_draws_owls_and_one_of_them_is_a_mimic() {
        let (world, (px, py)) = night_world();
        let seen = friendly_draws_at(&world, px, py, 200_000);
        assert!(seen.contains(&OWL), "no Owl on a surface night: {seen:?}");
        assert!(
            seen.contains(&OWL_MIMIC),
            "no Owl Mimic in two hundred thousand ticks: {seen:?}"
        );
    }

    /// ...and it is a *night* draw. `NPC.cs`'s owl arm carries `!Main.dayTime`, and the daytime
    /// songbird arm below it is what answers instead.
    #[test]
    fn nothing_draws_an_owl_by_day() {
        let (world, (px, py)) = forest_surface();
        assert!(world.day_time);
        let seen = friendly_draws_at(&world, px, py, 200_000);
        // The songbirds first, so this cannot pass by drawing nothing friendly at all.
        assert!(
            seen.contains(&74),
            "no daytime songbird, so this proves nothing: {seen:?}"
        );
        assert!(!seen.contains(&OWL), "an owl in broad daylight: {seen:?}");
        assert!(!seen.contains(&OWL_MIMIC), "and a mimic too: {seen:?}");
    }

    /// The surface draws both squirrels. Vanilla's arm is
    /// `Utils.SelectRandom(Main.rand, new short[2] { 299, 538 })` (`NPC.cs:2611`), so the Red
    /// Squirrel comes up exactly as often as the grey one; only the grey one was in the pool, so
    /// 538 had no producer anywhere in this server.
    ///
    /// Neutralised by dropping 538 from `friendly_pool`'s surface arm: the second assertion fails
    /// and the first still passes, so this is not reading "nothing friendly spawned".
    #[test]
    fn the_surface_draws_both_squirrels() {
        let (world, (px, py)) = forest_surface();
        let seen = friendly_draws_at(&world, px, py, 20_000);
        assert!(seen.contains(&299), "no grey squirrel: {seen:?}");
        assert!(seen.contains(&538), "no red squirrel: {seen:?}");
    }

    /// A flat floor deep enough to be caverns, and a place to stand on it.
    ///
    /// `flat_world` puts the surface line at 100 and the rock layer at 200, so a floor at 300 is
    /// below the rock and above the underworld at `600 - 200`: vanilla's `flag11` exactly
    /// (`NPC.cs:2557`).
    fn cavern_floor() -> (World, (i32, i32)) {
        let world = flat_world(300);
        let at = (400, 298);
        assert_eq!(depth_at(&world, at.1), Depth::Cavern);
        (world, at)
    }

    /// The caverns hand out gem critters, and they hand out nothing else (`NPC.cs:2600-2624`).
    ///
    /// All fourteen were unreachable: `friendly_pool`'s one fallback arm answered a Bunny and a
    /// Squirrel at every depth below the surface, so neither gem ladder was called anywhere in this
    /// server and no player could meet a single one of them. The same arm was wrong in the other
    /// direction too, because vanilla's tail draws neither of those two under the rock layer: the
    /// `flag11` side of its fork is a gem critter one attempt in five and nothing the other four.
    ///
    /// Neutralised by making `deep_friendly_pick` return `Some(BUNNY)` for every depth, which is
    /// what the old fallback amounted to: the first assertion fails on all fourteen types at once
    /// and the second fails on the Bunny. Returning `None` for `Depth::Cavern` alone fails the first
    /// and passes the second, which is the half that says the caverns really are otherwise empty.
    #[test]
    fn the_caverns_are_gem_critters_and_nothing_else() {
        let (world, (px, py)) = cavern_floor();
        let seen = friendly_draws_at(&world, px, py, 200_000);
        for ty in GEM_SQUIRREL..=GEM_BUNNY + 6 {
            assert!(
                seen.contains(&ty),
                "no gem critter {ty} in the caverns: {seen:?}"
            );
        }
        for ty in [BUNNY, 299, 538] {
            assert!(
                !seen.contains(&ty),
                "the surface critter {ty} reached the caverns: {seen:?}"
            );
        }
    }

    /// ...and the dirt layer between the surface line and the rock keeps the plain Bunny it always
    /// had, with no gem critter anywhere in it (`NPC.cs:2614-2624`: the gem arms are `flag11`'s and
    /// the bunny is the `else`).
    ///
    /// Neutralised by widening both gem gates in `deep_friendly_pick` from `depth == Depth::Cavern`
    /// to `depth != Depth::Surface`: the dirt layer comes back holding twelve gem critters and no
    /// Bunny at all, which is both halves of this at once. The cavern test is unaffected by that
    /// widening and still passes, so the failure is this layer's and not a general one.
    #[test]
    fn the_dirt_layer_keeps_its_plain_bunny_and_no_gems() {
        let world = flat_world(150);
        let at = (400, 148);
        assert_eq!(depth_at(&world, at.1), Depth::Underground);
        let seen = friendly_draws_at(&world, at.0, at.1, 20_000);
        assert!(
            seen.contains(&BUNNY),
            "no bunny in the dirt layer: {seen:?}"
        );
        for ty in GEM_SQUIRREL..=GEM_BUNNY + 6 {
            assert!(
                !seen.contains(&ty),
                "the gem critter {ty} reached the dirt layer: {seen:?}"
            );
        }
    }

    /// The gem ladder is weighted, not uniform (`NPC.cs:5689-5714`, `:5719-5744`): Amethyst is 28
    /// draws in a hundred and Diamond is 5, so a Gem Bunny Diamond is a find and a Gem Bunny
    /// Amethyst is not.
    ///
    /// Neutralised by replacing `gem_critter`'s body with `first + rng.random_range(0..7)`: the
    /// uniform share is 14.3%, which fails the Amethyst bound at once.
    #[test]
    fn the_gem_ladder_is_weighted_rarest_last() {
        let mut rng = SmallRng::seed_from_u64(645);
        let mut counts = [0u32; 7];
        const RUNS: u32 = 200_000;
        for _ in 0..RUNS {
            counts[usize::from(gem_critter(GEM_BUNNY, &mut rng) - GEM_BUNNY)] += 1;
        }
        // Amethyst, Topaz, Sapphire, Emerald, Ruby, Diamond, Amber, in id order: the ladder's own
        // cutoffs read as widths are 28, 21, 16, 12, 10, 5, 8 per hundred.
        for (offset, want) in [28u32, 21, 16, 12, 10, 5, 8].into_iter().enumerate() {
            let got = counts[offset] * 100 / RUNS;
            assert!(
                got.abs_diff(want) <= 1,
                "gem offset {offset}: {got}% drawn, vanilla's ladder says {want}%"
            );
        }
    }

    /// A cavern floored in one material every eight rows, thick enough over the whole 169x124 scan
    /// box for [`biome_at`] to read whatever that material says.
    ///
    /// One solid row in eight rather than a slab, so every ledge has seven clear rows over it and
    /// [`has_room`] never turns a candidate away for want of headroom. `flat_world_of`'s single
    /// four-row floor is not enough tile for the snow threshold (1500) that one of these tests
    /// needs.
    fn cavern_of(block: u16) -> World {
        use terrustia_proto::tile::Tile;
        let mut world = World::empty(800, 600, "cavern");
        world.surface = 100;
        world.rock_layer = 200;
        for y in (200i32..340).step_by(8) {
            for x in 200..600 {
                world.set_tile(x, y, Tile::block(block));
            }
        }
        world
    }

    /// A shore: forty rows of standing water over a floor at row 190, in a world wide enough to hold
    /// both a beach (the outer [`BEACH_DISTANCE`] columns) and an inland column.
    ///
    /// The surface line sits well below the floor rather than on it. The beach arm's scan for the
    /// top of the water is `spawnTileY < Main.worldSurface` strictly (`NPC.cs:2122`) where every
    /// other test around it is `<=`, so a floor exactly on the line has no reachable water surface
    /// and neither the Sea Turtle nor the Dolphin can be drawn at all.
    ///
    /// The floor's depth is not free either. [`SHORE_STAND`] is where a player has to stand for a
    /// candidate to reach it: the descent to ground stops at `py + SPAWN_RANGE_Y` (`NPC.cs:991`), so
    /// a player floating more than 47 rows above the sea bed finds no ground under any candidate at
    /// all and nothing spawns anywhere. It also has to be more than [`SAFE_RANGE_Y`] above it, or
    /// every candidate within [`SAFE_RANGE_X`] is thrown out for being on top of the player and
    /// `xRange` is never false.
    fn shore_world(ground: u16) -> World {
        let mut world = World::empty(1200, 600, "shore");
        world.surface = 260;
        world.rock_layer = 350;
        for x in 0..world.width() {
            for y in 190..202 {
                world.set_tile(x, y, terrustia_proto::Tile::block(ground));
            }
            for y in 150..190 {
                world.set_tile(
                    x,
                    y,
                    terrustia_proto::Tile::AIR
                        .with_liquid(terrustia_proto::tile::Liquid::Water, 255),
                );
            }
        }
        world
    }

    /// The four critters the ground below the surface line has of its own (`NPC.cs:3776-3805`).
    ///
    /// All four sat unreachable for one root reason: this server drew critters only inside a
    /// `spawnFriendly` attempt, and vanilla spawns these on the *monster* path, in the same
    /// `else if` ladder as the Lihzahrd temple and the sandstorm. A cave with no town anywhere near
    /// it has worms and mice in it in the game, and had none at all here.
    ///
    /// Neutralised arm by arm, each rerun (the assertions are ordered, so each names the first to
    /// fire):
    /// * `&& false` on the Worm arm: "no Worm (357) underground: {10, 16, 21, 44, 49, 93, 195,
    ///   217, 300, 354, 359, 453, 496, 497, 498, 504, 506}".
    /// * the same on the Mouse arm: "no Mouse (300) underground: {10, 16, 21, 44, 49, 93, 195, 217,
    ///   354, 357, 359, 448, 453, 496, 497, 498, 504, 506}".
    /// * the same on the Snail arm: "no Snail (359) underground: {10, 16, 21, 44, 49, 93, 195, 217,
    ///   300, 354, 357, 448, 453, 496, 497, 498, 504, 506}".
    /// * the same on the Lac Beetle arm: "no Lac Beetle (219) on jungle grass: {42, 43, 51, 56,
    ///   204, 217, 354, 359}".
    /// * dropping `Biome::Jungle` from the Worm and Mouse arms' exclusion list: "the jungle keeps
    ///   the worm and the mouse out: {42, 43, 51, 56, 204, 217, 219, 300, 357, 359}".
    /// * relaxing `depth != Depth::Surface` to `true` on all four: "the surface has no worms of its
    ///   own, and got 357: {1, 300, 357, 359}".
    #[test]
    fn the_ground_under_the_surface_has_worms_mice_and_snails() {
        let stone = cavern_of(1);
        assert_eq!(depth_at(&stone, 255), Depth::Cavern);
        assert_eq!(biome_at(&stone, 400, 255), Biome::Forest);
        let seen: std::collections::BTreeSet<u16> = spawns_at(&stone, false, 400, 255, 20_000)
            .into_iter()
            .collect();
        for (npc_type, name) in [(WORM, "Worm"), (MOUSE, "Mouse"), (SNAIL, "Snail")] {
            assert!(
                seen.contains(&npc_type),
                "no {name} ({npc_type}) underground: {seen:?}"
            );
        }

        // Jungle grass is the Lac Beetle's whole gate, and the same floor shuts the worm and the
        // mouse out while leaving the snail in: `NPC.cs:3802` is the one of the four with no
        // `!ZoneJungle` on it.
        let jungle = cavern_of(JUNGLE_GRASS);
        assert_eq!(biome_at(&jungle, 400, 255), Biome::Jungle);
        let under_jungle: std::collections::BTreeSet<u16> =
            spawns_at(&jungle, false, 400, 255, 20_000)
                .into_iter()
                .collect();
        assert!(
            under_jungle.contains(&LAC_BEETLE),
            "no Lac Beetle ({LAC_BEETLE}) on jungle grass: {under_jungle:?}"
        );
        assert!(
            !under_jungle.contains(&WORM) && !under_jungle.contains(&MOUSE),
            "the jungle keeps the worm and the mouse out: {under_jungle:?}"
        );
        assert!(
            under_jungle.contains(&SNAIL),
            "the snail's arm has no jungle exclusion: {under_jungle:?}"
        );

        // ...and none of the four is a surface spawn: `spawnTileY > Main.worldSurface` on all of
        // them.
        let (surface, (px, py)) = forest_surface();
        let above: std::collections::BTreeSet<u16> = spawns_at(&surface, false, px, py, 20_000)
            .into_iter()
            .collect();
        for npc_type in [WORM, MOUSE, SNAIL, LAC_BEETLE] {
            assert!(
                !above.contains(&npc_type),
                "the surface has no worms of its own, and got {npc_type}: {above:?}"
            );
        }
        assert!(
            !above.is_empty(),
            "the surface answered with nothing at all"
        );
    }

    /// The caverns' beetle pair, one draw in sixty and nothing else gating it (`NPC.cs:4925-4935`).
    ///
    /// Two hundred thousand ticks and not the twenty thousand this used to run: a cavern at rest
    /// answers about eight times per thousand ticks and roughly one answer in ninety is a beetle, so
    /// the old count expected 1.7 of them and had close to a one-in-five chance of finding none. It
    /// found one on the stream it happened to be reading until an arm above it drew a number and
    /// reshuffled everything downstream, which is the shape of a test that was measuring luck rather
    /// than the arm. At this count the expected haul is eighteen.
    ///
    /// Neutralised with `&& false` on the arm: "no Cochineal Beetle (217) in the caverns: {10, 16,
    /// 21, 44, 49, 93, 195, 201, 202, 203, 300, 354, 357, 359, 453, 496, 497, 498, 504, 506}".
    /// Neutralised again by making the fork answer `COCHINEAL_BEETLE` in both branches: "no Cyan
    /// Beetle (218) in a snowy cavern: {147, 150, 167, 185, 217, 354, 453}".
    #[test]
    fn the_caverns_have_a_beetle_and_the_snow_has_the_other_one() {
        let stone = cavern_of(1);
        let seen: std::collections::BTreeSet<u16> = spawns_at(&stone, false, 400, 255, 200_000)
            .into_iter()
            .collect();
        assert!(
            seen.contains(&COCHINEAL_BEETLE),
            "no Cochineal Beetle ({COCHINEAL_BEETLE}) in the caverns: {seen:?}"
        );
        assert!(
            !seen.contains(&CYAN_BEETLE),
            "the cyan one is the snow's alone: {seen:?}"
        );

        // 147 is `TileID.SnowBlock`, and 1500 of them is `SnowTileNormalThreshold`.
        let snow = cavern_of(147);
        assert_eq!(biome_at(&snow, 400, 255), Biome::Snow);
        let snowy: std::collections::BTreeSet<u16> = spawns_at(&snow, false, 400, 255, 200_000)
            .into_iter()
            .collect();
        assert!(
            snowy.contains(&CYAN_BEETLE),
            "no Cyan Beetle ({CYAN_BEETLE}) in a snowy cavern: {snowy:?}"
        );
        assert!(
            !snowy.contains(&COCHINEAL_BEETLE),
            "the cochineal one is everywhere the snow is not: {snowy:?}"
        );
    }

    /// The underworld's lava-bait critters, one ordinary draw in eight (`NPC.cs:4881-4884` ->
    /// `SpawnLavaBaitCritters`, `:5850-5877`). The day fork is the whole difference between a Hell
    /// Butterfly and a Lavafly; the Magma Snail comes at either hour.
    ///
    /// Neutralised with `&& false` on the `LAVA_BAIT_ODDS` arm: "no Hell Butterfly (653) in the
    /// underworld by day: {24, 39, 59, 60, 62}", none of the three drawn in twenty thousand ticks.
    /// Neutralised again by dropping `lava_bait_pick`'s `day_time` fork so it always answers 654:
    /// "no Hell Butterfly (653) in the underworld by day: {24, 39, 59, 60, 62, 654, 655}", the
    /// other two now present. Neutralised a third time by putting the friendly arm's underworld
    /// branch back above its graveyard fork: "no graveyard vermin in an underworld graveyard:
    /// {59, 60, 62, 654, 655}".
    #[test]
    fn the_underworld_has_lava_bait_critters() {
        use terrustia_proto::tile::Tile;
        let mut world = World::empty(800, 600, "underworld");
        world.surface = 100;
        world.rock_layer = 200;
        for y in (400i32..560).step_by(8) {
            for x in 200..600 {
                world.set_tile(x, y, Tile::block(57)); // ash
            }
        }
        assert_eq!(depth_at(&world, 455), Depth::Underworld);

        let by_day: std::collections::BTreeSet<u16> = spawns_at(&world, false, 400, 455, 20_000)
            .into_iter()
            .collect();
        assert!(
            by_day.contains(&653),
            "no Hell Butterfly (653) in the underworld by day: {by_day:?}"
        );
        assert!(
            by_day.contains(&655),
            "no Magma Snail (655) by day: {by_day:?}"
        );
        assert!(
            !by_day.contains(&654),
            "the Lavafly is the night's: {by_day:?}"
        );

        world.day_time = false;
        let by_night: std::collections::BTreeSet<u16> = spawns_at(&world, false, 400, 455, 20_000)
            .into_iter()
            .collect();
        assert!(
            by_night.contains(&654),
            "no Lavafly (654) in the underworld at night: {by_night:?}"
        );
        assert!(
            by_night.contains(&655),
            "no Magma Snail (655) at night: {by_night:?}"
        );
        assert!(
            !by_night.contains(&653),
            "the Hell Butterfly is the day's: {by_night:?}"
        );

        // A graveyard's friendly arm sits ahead of the underworld's and `return`s
        // (`NPC.cs:2101-2115`), so tombstones in hell answer with a Maggot or a Rat rather than a
        // Lavafly. Only the friendly branch is governed by it: the ordinary chain's own lava-bait
        // arm at `:4881` has no graveyard test, and neither does this server's, which is why this
        // asserts what a graveyard *does* draw rather than what it does not.
        let among_tombstones = friendly_draws_in(&world, &quiet(), true, 400, 455, 20_000);
        assert!(
            GRAVEYARD_VERMIN
                .iter()
                .any(|v| among_tombstones.contains(v)),
            "no graveyard vermin in an underworld graveyard: {among_tombstones:?}"
        );
    }

    /// A meteor-shower night brings the Enchanted Nightcrawler out, and an ordinary one does not
    /// (`NPC.cs:2409-2413`). It is the head of the friendly grass chain, so it takes half of every
    /// friendly attempt while the sky is right, ahead of the firefly and the owl.
    ///
    /// Neutralised with `&& false` on the whole arm: "no Enchanted Nightcrawler (484) under a
    /// meteor shower: {355, 611, 689}". Neutralised again by dropping `events.starfall_night` from
    /// the arm's guard: the second assertion fails instead, "an ordinary night should have none",
    /// with 484 in the set.
    #[test]
    fn a_meteor_shower_night_brings_the_enchanted_nightcrawler() {
        let (world, (px, py)) = night_world();
        let shower = EventSpawns {
            starfall_night: true,
            ..quiet()
        };
        let seen = friendly_draws_with(&world, &shower, px, py, 20_000);
        assert!(
            seen.contains(&ENCHANTED_NIGHTCRAWLER),
            "no Enchanted Nightcrawler ({ENCHANTED_NIGHTCRAWLER}) under a meteor shower: {seen:?}"
        );

        let ordinary = friendly_draws_with(&world, &quiet(), px, py, 20_000);
        assert!(
            !ordinary.contains(&ENCHANTED_NIGHTCRAWLER),
            "an ordinary night should have none: {ordinary:?}"
        );
        assert!(
            ordinary.contains(&FIREFLY),
            "...and the firefly proves the ordinary night still drew something: {ordinary:?}"
        );

        // Daylight has none either, shower or no shower: `!Main.dayTime` is the arm's first clause.
        let (day, (dx, dy)) = forest_surface();
        let daylit = friendly_draws_with(&day, &shower, dx, dy, 20_000);
        assert!(
            !daylit.contains(&ENCHANTED_NIGHTCRAWLER),
            "a nightcrawler in broad daylight: {daylit:?}"
        );
    }

    /// The firefly arm is a two-way fork on the tile underfoot, not one type: hallowed grass makes
    /// it a Lightning Bug (`NPC.cs:2416-2420`, `type2 = 358` when `tileType == 109`).
    ///
    /// Neutralised by turning off the `critter == FIREFLY && ground_block == Some(HALLOWED_GRASS)`
    /// branch: "no Lightning Bug (358) over hallowed grass: {355, 611, 689}". Neutralised again by
    /// dropping the `ground_block` half of the guard so every firefly becomes one: the last
    /// assertion fails instead, "an ordinary forest has fireflies, not lightning bugs", with 358 in
    /// the set.
    #[test]
    fn hallowed_grass_turns_the_firefly_into_a_lightning_bug() {
        // 1600 columns wide and sampled in the middle, for the reason [`forest_surface`] gives: on
        // an 800-column world every candidate is inside `isBeach` and every friendly draw is a
        // Seagull.
        let mut world = flat_world_sized(1600, 90, HALLOWED_GRASS);
        world.day_time = false;
        assert_eq!(biome_at(&world, 800, 88), Biome::Hallow);
        let seen = friendly_draws_with(&world, &quiet(), 800, 88, 20_000);
        assert!(
            seen.contains(&LIGHTNING_BUG),
            "no Lightning Bug ({LIGHTNING_BUG}) over hallowed grass: {seen:?}"
        );
        assert!(
            !seen.contains(&FIREFLY),
            "the plain firefly should have been replaced: {seen:?}"
        );

        let (forest, (px, py)) = night_world();
        let plain = friendly_draws_with(&forest, &quiet(), px, py, 20_000);
        assert!(
            plain.contains(&FIREFLY) && !plain.contains(&LIGHTNING_BUG),
            "an ordinary forest has fireflies, not lightning bugs: {plain:?}"
        );
    }

    /// A party puts a bunny in a hat, which is the third arm of the same chain the two calendars
    /// use (`NPC.cs:1645`, `:2596`, `:4292`), and 540 came from nowhere else in the spawner.
    ///
    /// Neutralised by deleting the `BUNNY if at.party` arm from `holiday_costume`: no Party Bunny
    /// in twenty thousand ticks and the first assertion fails. Neutralised again by moving that arm
    /// to the head of the chain, above both calendars: the last assertion fails with
    /// "the chain is out of order: 303 x6030, 337 x1964, 540 x18020".
    ///
    /// The ordering is checked by counting rather than by membership, because at two draws in three
    /// each every arm still comes up whichever order they are in: a set of what appeared cannot
    /// tell the orders apart, and asserting on one would have looked like a test and proved
    /// nothing.
    #[test]
    fn a_party_puts_a_bunny_in_a_hat() {
        let (world, (px, py)) = forest_surface();
        let party = EventSpawns {
            party: true,
            ..quiet()
        };
        let seen = friendly_draws_with(&world, &party, px, py, 20_000);
        assert!(
            seen.contains(&540),
            "no Party Bunny (540) with a party on: {seen:?}"
        );
        let quiet_day = friendly_draws_with(&world, &quiet(), px, py, 20_000);
        assert!(
            !quiet_day.contains(&540) && quiet_day.contains(&46),
            "a party bunny with no party: {quiet_day:?}"
        );

        // The chain is Halloween, then Christmas, then the party (`NPC.cs:2588-2599`), and each arm
        // takes two draws in three of what reaches it. So with all three on, a bunny is Halloween's
        // two times in three, Christmas's two in nine, and the party's two in twenty-seven.
        let at = Seasonal {
            halloween: true,
            xmas: true,
            party: true,
            ..Seasonal::default()
        };
        let mut counts = std::collections::BTreeMap::new();
        for seed in 0..27_000u64 {
            let mut rng = SmallRng::seed_from_u64(seed);
            *counts
                .entry(holiday_costume(46, Depth::Surface, at, &mut rng))
                .or_insert(0usize) += 1;
        }
        let (spooky, festive, partying) = (counts[&303], counts[&337], counts[&540]);
        assert!(
            spooky > festive && festive > partying && partying > 0,
            "the chain is out of order: 303 x{spooky}, 337 x{festive}, 540 x{partying}"
        );
    }

    /// The Red Squirrel and the Stink Bug, two ordinary members of the surface day's friendly draw
    /// that this server's pool simply did not list (`NPC.cs:2611`'s `SelectRandom(299, 538)` and
    /// `:2474`'s own arm).
    ///
    /// Neutralised by removing each id from `friendly_pool`'s surface day arm in turn: "no
    /// SquirrelRed (538) on a friendly surface day: {46, 74, 297, 298, 299, 356, 669}" and "no
    /// Stinkbug (669) on a friendly surface day: {46, 74, 297, 298, 299, 356, 538}".
    #[test]
    fn the_surface_day_draws_a_red_squirrel_and_a_stink_bug() {
        let (world, (px, py)) = forest_surface();
        let seen = friendly_draws_with(&world, &quiet(), px, py, 20_000);
        for (npc_type, name) in [(538u16, "SquirrelRed"), (669, "Stinkbug")] {
            assert!(
                seen.contains(&npc_type),
                "no {name} ({npc_type}) on a friendly surface day: {seen:?}"
            );
        }
    }

    /// Where a player stands to fish this shore: 41 rows above the sea bed, which is inside the
    /// descent's reach and outside the safe box. See [`shore_world`].
    const SHORE_STAND: i32 = 148;

    /// The row the shore's water surface is at, and the row two below it, as [`water_surface`] finds
    /// them: the first dry row above the water is 149, so `num28` is 151 and `num29` is 153. This
    /// server's own spawn row is one above the game's, so the two are pushed at 150 and 152.
    const SHORE_TOP: f32 = 150.0 * 16.0;
    const SHORE_BELOW: f32 = 152.0 * 16.0;

    /// A beach is its own roster and its own placement (`NPC.cs:2116-2199`): a Seagull on the water,
    /// a Sea Turtle floating on it, and a Dolphin, a Seahorse and a Pufferfish two rows under.
    ///
    /// Fails before the fix, with every one of the five missing: `friendly_pool`'s ocean entry was
    /// three goldfish and ducks, and vanilla's beach chain was in no producer at all, so none of
    /// these five could be met anywhere in a world.
    ///
    /// Neutralised by making `friendly_chain`'s `if !x_range && is_beach` block unreachable
    /// (`if false && ...`): all five assertions fail and the draw falls back to the ordinary ocean
    /// pool. Neutralised again by returning `(None, None)` from `water_surface`, which leaves the
    /// Seagull and the Seahorse (they fall back to the candidate's own row) and loses the Sea
    /// Turtle and the Dolphin, and puts everything left at the floor row instead of on the water.
    #[test]
    fn a_beach_draws_the_seagull_and_the_sea_life() {
        let world = shore_world(SAND);
        // Column 150: inside `beachDistance`, and its whole spawn box with it.
        let drawn = friendly_spawns_with(&world, &quiet(), 150, SHORE_STAND, 400_000);
        let found: std::collections::BTreeSet<u16> = drawn.iter().map(|(ty, _)| *ty).collect();
        for (npc_type, name) in [
            (SEAGULL, "Seagull"),
            (SEA_TURTLE, "Sea Turtle"),
            (DOLPHIN, "Dolphin"),
            (SEAHORSE, "Seahorse"),
            (PUFFERFISH, "Pufferfish"),
        ] {
            assert!(
                found.contains(&npc_type),
                "no {name} ({npc_type}) on a beach: {found:?}"
            );
        }

        // ...and each of them is placed on the water rather than on the floor the candidate was
        // chosen at, which is the whole reason this arm could not be part of `water_pick`.
        for (npc_type, row) in [
            (SEAGULL, SHORE_TOP),
            (SEA_TURTLE, SHORE_TOP),
            (DOLPHIN, SHORE_BELOW),
            (SEAHORSE, SHORE_BELOW),
            (PUFFERFISH, SHORE_BELOW),
        ] {
            for (_, (_, y)) in drawn.iter().filter(|(ty, _)| *ty == npc_type) {
                assert_eq!(*y, row, "{npc_type} was not placed on the water");
            }
        }
    }

    /// Inland water is a different chain again, keyed by what the water is sitting on
    /// (`NPC.cs:2220-2325`): water striders and a Jungle Turtle over jungle grass, water striders
    /// and a Grebe over sand, a Turtle and ducks over grass, and a Pupfish, a Goldfish or a Gold
    /// Goldfish wherever the surface of the water is out of reach.
    ///
    /// Fails before the fix: none of the six had a producer anywhere in this server.
    ///
    /// Neutralised by making the `if wet` block in `friendly_chain` unreachable: every assertion
    /// below fails. Neutralised again by dropping the `!x_range` clause from the surface sub-chain:
    /// the placement assertions fail, water striders landing in the safe column band on top of the
    /// player. (The roster assertions do *not* fail for that one, which is why the placement ones
    /// are here: the arm's own one-in-three decline reaches the Pupfish and the goldfish anyway.)
    #[test]
    fn inland_water_draws_what_it_is_standing_on() {
        // Column 700 in a 1200-wide world: past `beachDistance` at both ends, so `isBeach` is false
        // for the whole spawn box and the arm above this one never answers.
        let ground_case = |ground: u16, wanted: &[(u16, &str)]| {
            let world = shore_world(ground);
            let drawn = friendly_spawns_with(&world, &quiet(), 700, SHORE_STAND, 400_000);
            let found: std::collections::BTreeSet<u16> = drawn.iter().map(|(ty, _)| *ty).collect();
            for (npc_type, name) in wanted {
                assert!(
                    found.contains(npc_type),
                    "no {name} ({npc_type}) over {ground}: {found:?}"
                );
            }
            drawn
        };

        let jungle = ground_case(
            JUNGLE_GRASS,
            &[
                (TURTLE_JUNGLE, "Jungle Turtle"),
                (WATER_STRIDER, "Water Strider"),
                (GOLDFISH, "Goldfish"),
            ],
        );
        // The strider stands a row *above* the water's own surface (`num34 * 16 - 16`), which is
        // what "walking on water" looks like from the tile grid, and the turtle floats on it.
        for (_, (_, y)) in jungle.iter().filter(|(ty, _)| *ty == WATER_STRIDER) {
            assert_eq!(*y, SHORE_TOP - 16.0, "a water strider was not on the water");
        }
        for (_, (_, y)) in jungle.iter().filter(|(ty, _)| *ty == TURTLE_JUNGLE) {
            assert_eq!(*y, SHORE_TOP, "a jungle turtle was not on the water");
        }
        // ...and `!xRange` (`NPC.cs:2237`): nothing that stands on the water is placed inside the
        // safe column band, because it goes several rows away from the candidate and would arrive
        // on the player's head. The striders scatter up to one column either side of theirs, so the
        // bound they have to clear is one short of [`SAFE_RANGE_X`].
        // The goldfish the arm falls through to is exempt: it stands where the candidate did, and
        // vanilla's `!xRange` sits on the surface sub-chain alone.
        for (npc_type, (x, _)) in jungle
            .iter()
            .filter(|(ty, _)| matches!(*ty, WATER_STRIDER | GOLD_WATER_STRIDER | TURTLE_JUNGLE))
        {
            assert!(
                (x / 16.0 - 700.0).abs() >= (SAFE_RANGE_X - 1) as f32,
                "{npc_type} was placed on top of the player, at column {}",
                x / 16.0
            );
        }

        ground_case(
            SAND,
            &[
                (GREBE, "Grebe"),
                (WATER_STRIDER, "Water Strider"),
                // Sand past `beachDistance` is the Pupfish's one home, and it is the *inland* side
                // of that band rather than the seaward one.
                (PUPFISH, "Pupfish"),
            ],
        );
        ground_case(
            GRASS,
            &[
                (TURTLE, "Turtle"),
                (DUCK, "Duck"),
                (DUCK_WHITE, "White Duck"),
                (GOLDFISH, "Goldfish"),
            ],
        );
    }

    /// Rain over grass replaces the whole daytime critter switch with one chain of its own
    /// (`NPC.cs:2381-2407`), and the Goldfish Walker is its last arm: a goldfish out of water,
    /// which the game spawns while it rains and at no other time.
    ///
    /// Fails before the fix: 230 and 357 were in no producer, and the rain had no friendly arm at
    /// all, so a rainy afternoon drew the ordinary songbirds and butterflies.
    ///
    /// Neutralised by dropping the `world.raining` clause from the arm's own condition: the dry
    /// assertions below fail, worms and goldfish walkers turning up in clear weather. Neutralised
    /// again by making the arm unreachable: the wet assertions fail instead.
    #[test]
    fn the_rain_turns_the_grass_over_to_worms_and_goldfish_walkers() {
        let mut world = flat_world_sized(1600, 90, GRASS);
        let at = (800, 88);
        let dry = friendly_draws_at(&world, at.0, at.1, 200_000);
        assert!(
            dry.contains(&74),
            "no daytime songbird, so this proves nothing: {dry:?}"
        );
        for npc_type in [GOLDFISH_WALKER, WORM] {
            assert!(
                !dry.contains(&npc_type),
                "{npc_type} turned up on a clear day: {dry:?}"
            );
        }

        world.raining = true;
        let wet = friendly_draws_at(&world, at.0, at.1, 200_000);
        for (npc_type, name) in [(GOLDFISH_WALKER, "Goldfish Walker"), (WORM, "Worm")] {
            assert!(
                wet.contains(&npc_type),
                "no {name} ({npc_type}) in the rain: {wet:?}"
            );
        }
        // ...and the arm `break`s, so nothing below it in the game's switch runs while it rains.
        assert!(
            !wet.contains(&BUTTERFLY),
            "a butterfly in the rain: {wet:?}"
        );
    }

    /// A windy day does not add ladybugs to a meadow, it replaces the butterflies with them
    /// (`NPC.cs:2487-2534`): the two arms sit next to each other and test the same flag in opposite
    /// directions.
    ///
    /// Fails before the fix: 604 and 605 were in no producer.
    ///
    /// Neutralised by replacing `spawn_butterfly`'s body with a bare `BUTTERFLY`: the windy
    /// assertions fail. Neutralised again by dropping the `!too_windy` early return, which puts
    /// ladybugs in dead calm and fails the still-air assertions.
    #[test]
    fn a_windy_day_turns_the_butterflies_into_ladybugs() {
        let world = flat_world_sized(1600, 90, GRASS);
        let at = (800, 88);
        let blowing = |wind: f32| {
            let events = EventSpawns {
                wind_target: wind,
                ..quiet()
            };
            friendly_spawns_with(&world, &events, at.0, at.1, 200_000)
                .into_iter()
                .map(|(npc_type, _)| npc_type)
                .collect::<std::collections::BTreeSet<u16>>()
        };

        // `TooWindyForButterflies` is `>= 0.4f`, so 0.39 is still a butterfly's day.
        let calm = blowing(0.39);
        assert!(
            calm.contains(&BUTTERFLY),
            "no butterfly in still air: {calm:?}"
        );
        assert!(
            !calm.contains(&LADY_BUG),
            "a ladybug in still air: {calm:?}"
        );

        let windy = blowing(0.4);
        assert!(
            windy.contains(&LADY_BUG),
            "no ladybug on a windy day: {windy:?}"
        );
        assert!(
            !windy.contains(&BUTTERFLY),
            "a butterfly on a windy day: {windy:?}"
        );
    }

    /// Cattails in the rain bring dragonflies, and they bring them to the cattail rather than to the
    /// spawn point (`NPC.cs:2200-2218`, `FindCattailTop`).
    ///
    /// Fails before the fix for the Gold Dragonfly (601), which had no producer at all. The six
    /// ordinary dragonflies had none either; they are absent from `docs/spawn-gaps.tsv` only
    /// because `check_spawn_reach.py` cannot read `Main.rand.NextFromList` and so never saw them on
    /// vanilla's side of the diff.
    ///
    /// Neutralised by making the cattail block in `friendly_chain` unreachable: the dragonfly
    /// assertion fails and the rain arm below answers instead. Neutralised again by dropping the
    /// `frame_x >= 180` clause from `find_cattail_top`, which is what tells a cattail's top from its
    /// stalk: the placement assertion fails, dragonflies appearing at the base of the plant.
    #[test]
    fn cattails_in_the_rain_bring_dragonflies() {
        let mut world = flat_world_sized(1600, 90, GRASS);
        world.raining = true;
        let at = (800, 88);
        // One cattail top, a hundred columns from the player and six rows up. A hundred and not ten:
        // no candidate ever lands inside [`SAFE_RANGE_X`] of the player, and `FindCattailTop` looks
        // only thirty columns either side of the candidate, so a cattail beside the player is a
        // cattail no spawn attempt can see. Only the top frame counts, and the stalk under it is the
        // same tile with a lower `frameX`.
        world.set_tile(700, 84, terrustia_proto::Tile::framed(CATTAIL, 180, 0));
        world.set_tile(700, 85, terrustia_proto::Tile::framed(CATTAIL, 0, 0));

        let drawn = friendly_spawns_with(&world, &quiet(), at.0, at.1, 200_000);
        let dragonflies: Vec<(f32, f32)> = drawn
            .iter()
            .filter(|(ty, _)| GRASS_DRAGONFLIES.contains(ty) || *ty == GOLD_DRAGONFLY)
            .map(|(_, pos)| *pos)
            .collect();
        assert!(
            !dragonflies.is_empty(),
            "no dragonflies over a cattail in the rain: {:?}",
            drawn
                .iter()
                .map(|(ty, _)| *ty)
                .collect::<std::collections::BTreeSet<u16>>()
        );
        // The swarm sits at the cattail's own top, one column either side of it, and never at the
        // spawn point the candidate was chosen at.
        for (x, y) in &dragonflies {
            assert_eq!(*y, 83.0 * 16.0, "a dragonfly was not at the cattail's top");
            assert!(
                (*x - 700.0 * 16.0).abs() <= 16.0,
                "a dragonfly was not at the cattail's column: {x}"
            );
        }
    }

    /// One critter draw in four hundred is the gold one (`goldCritterChance`, `NPC.cs:6054`), which
    /// is the whole of how the eight gold variants below are ever spawned in the game.
    ///
    /// Checked against the function rather than through `try_spawn`, because at one in four hundred
    /// on top of a pool draw the gold types need more attempts than a spawn-tick harness can
    /// afford; `the_gold_critters_reach_a_real_spawn` below is the end-to-end half of the claim.
    ///
    /// Neutralised by replacing `gold_variant`'s body with `npc_type` (keeping the `rng` draw, so
    /// the only change is which type comes back): "Bird -> 442 came up 0 times in 400,000, not
    /// about a thousand". Neutralised again by relaxing the squirrel arm's `depth ==
    /// Depth::Surface` guard to `|| true`: "a gold squirrel underground, left: 994".
    #[test]
    fn one_critter_in_four_hundred_is_the_gold_one() {
        let twins: [(u16, u16, &str); 9] = [
            (74, 442, "Bird"),
            (297, 442, "BirdBlue"),
            (298, 442, "BirdRed"),
            (46, 443, "Bunny"),
            (356, 444, "Butterfly"),
            (361, 445, "Frog"),
            (300, 447, "Mouse"),
            (357, 448, "Worm"),
            (55, 592, "Goldfish"),
        ];
        for (base, gold, name) in twins {
            let hits = (0..400_000u64)
                .filter(|seed| {
                    let mut rng = SmallRng::seed_from_u64(*seed);
                    gold_variant(base, Depth::Cavern, 0.0, &mut rng) == gold
                })
                .count();
            // 400,000 draws at one in four hundred is a thousand expected; the band is wide enough
            // that no seed sequence realistically misses it and narrow enough that a wrong constant
            // (one in forty, or one in four thousand) cannot land inside it.
            assert!(
                (700..1400).contains(&hits),
                "{name} -> {gold} came up {hits} times in 400,000, not about a thousand"
            );
        }

        // The squirrel is the one arm with a depth gate on it: `& flag10` (`NPC.cs:2584`).
        let surface = (0..400_000u64)
            .filter(|seed| {
                let mut rng = SmallRng::seed_from_u64(*seed);
                gold_variant(299, Depth::Surface, 0.0, &mut rng) == 539
            })
            .count();
        assert!(
            (700..1400).contains(&surface),
            "SquirrelGold came up {surface} times in 400,000 on the surface"
        );
        let underground = (0..400_000u64)
            .filter(|seed| {
                let mut rng = SmallRng::seed_from_u64(*seed);
                gold_variant(299, Depth::Cavern, 0.0, &mut rng) == 539
            })
            .count();
        assert_eq!(underground, 0, "a gold squirrel underground");

        // Anything with no twin is handed straight back, whatever the seed.
        for base in [355u16, OWL, 606, 148] {
            for seed in 0..1_000u64 {
                let mut rng = SmallRng::seed_from_u64(seed);
                assert_eq!(gold_variant(base, Depth::Surface, 0.0, &mut rng), base);
            }
        }
    }

    /// Luck reaches the spawner, and the gold critters are where a player would first notice.
    ///
    /// `NPC.Spawner` rolls `RollLuck` in seventy-three places and every one of them was a plain
    /// `Main.rand.Next` here, because this server had no luck figure at all. `SetSpawnFlags`
    /// copies the player's own luck onto the spawner (`NPC.cs:370`), and `Conditions::luck` is
    /// that; `gold_variant` is the arm people actually farm with a Luck Potion.
    ///
    /// Measured over many draws rather than asserted on one: `RollLuck` is a chance of a better
    /// chance, so a lucky player still draws the ordinary odds most of the time.
    #[test]
    fn luck_makes_gold_critters_commoner_and_bad_luck_makes_them_rarer() {
        let gold_rate = |luck: f32| {
            let mut rng = SmallRng::seed_from_u64(11);
            let mut gold = 0;
            const DRAWS: u32 = 400_000;
            for _ in 0..DRAWS {
                if gold_variant(46, Depth::Surface, luck, &mut rng) == 443 {
                    gold += 1;
                }
            }
            f64::from(gold) / f64::from(DRAWS)
        };
        let plain = gold_rate(0.0);
        let lucky = gold_rate(1.0);
        let cursed = gold_rate(-0.7);

        // `goldCritterChance` is 400, and at luck zero that is exactly what a draw is worth.
        assert!(
            (plain - 1.0 / 400.0).abs() < 0.0005,
            "no luck is the flat one in four hundred: {plain:.5}"
        );
        assert!(
            lucky > plain * 1.2,
            "a lucky player finds meaningfully more: {lucky:.5} against {plain:.5}"
        );
        assert!(
            cursed < plain * 0.9,
            "and an unlucky one finds fewer: {cursed:.5} against {plain:.5}"
        );
    }

    /// ...and the same swap really is reached from a spawn, rather than only from its own function.
    ///
    /// The assertion is "one of the twins turned up", not "the Gold Bunny turned up", and the tick
    /// count is what makes that a claim rather than a coin toss. Half a million ticks of a forest
    /// surface yield about thirteen hundred friendly draws, roughly a thousand of them types with a
    /// twin, so at one in four hundred the expected haul is two or three gold critters overall but
    /// well under one of any single *named* twin, and pinning the test to the Gold Bunny alone was
    /// closer to a two-in-three chance of failing on any given rng stream (measured: 5 of 12 fresh
    /// seeds passed at this tick count).
    /// It passed on this seed until an unrelated arm above it drew one more number and reshuffled
    /// everything downstream, which is exactly the shape of a test that was measuring luck.
    /// [`gold_variant`]'s own test above already pins each of the nine individually and exactly;
    /// what this one is for is the wiring, and any twin proves that.
    ///
    /// Six million ticks put the expected haul near thirty, so a run that finds none is not a run
    /// that got unlucky.
    ///
    /// Neutralised by replacing `gold_variant`'s body with `npc_type`: "no gold critter in six
    /// million ticks", the set holding the plain critters and nothing else.
    #[test]
    fn the_gold_critters_reach_a_real_spawn() {
        const TWINS: [u16; 8] = [442, 443, 444, 445, 447, 448, 592, 539];
        let (world, (px, py)) = forest_surface();
        let seen = friendly_draws_with(&world, &quiet(), px, py, 6_000_000);
        assert!(
            TWINS.iter().any(|twin| seen.contains(twin)),
            "no gold critter in six million ticks: {seen:?}"
        );
    }

    /// The Moss Zombie, the last of the three types `docs/spawn-gaps.tsv` carried as unreachable
    /// and the only one of them that was ever going to move.
    ///
    /// `if (ZoneGraveyard && RollOnlyBadLuckExtreme(30) == 0)` (`NPC.cs:4712`), and
    /// `RollOnlyBadLuckExtreme` has no fallthrough: at or above zero luck it returns `-1`
    /// (`Luck.cs:53-60`), which the `== 0` can never match. So this is a spawn only an unlucky
    /// player ever sees, and the arm was correctly absent while every player here was at zero.
    ///
    /// The other two entries stay where they are: 450 and 451 are dead in the game's own shipped
    /// source, and no amount of luck reaches them.
    #[test]
    fn an_unlucky_player_in_a_graveyard_meets_the_moss_zombie() {
        const MOSS_ZOMBIE: u16 = 691;
        let graveyard_night = |luck: f32| {
            let at = Seasonal {
                graveyard: true,
                ..Seasonal::default()
            };
            let mut rng = SmallRng::seed_from_u64(4);
            let mut seen = 0;
            for _ in 0..200_000 {
                let zombie = zombie_settings(true, 1, &mut rng);
                // Tile 2 is grass, which is what the other tests of this chain stand on.
                if seasonal_night_pick(at, zombie, 2, luck, &mut rng) == Some(MOSS_ZOMBIE) {
                    seen += 1;
                }
            }
            seen
        };
        assert_eq!(
            graveyard_night(0.0),
            0,
            "no ordinary player ever meets one, which is what `RollOnlyBadLuckExtreme` means"
        );
        assert_eq!(
            graveyard_night(1.0),
            0,
            "and being *lucky* does not help either: there is no good-luck branch at all"
        );
        assert!(
            graveyard_night(-0.7) > 0,
            "a cursed player in a graveyard does"
        );
    }

    /// The luck a player is *carrying* reaches the spawner, which is the one link the two tests
    /// above cannot show: they call the pick routines directly and hand them a number, so a
    /// `Conditions { luck: 0.0 }` in `try_spawn` would leave both of them passing while no real
    /// player's luck did anything at all.
    ///
    /// `SetSpawnFlags` is the vanilla line (`NPC.cs:370`, `luck = player.luck;`), and this drives
    /// the whole spawner with a cursed player: the spawn *rate* arm at `NPC.cs:925-929` only fires
    /// below zero luck, so a cursed world produces strictly more friendly draws over the same
    /// number of ticks than a neutral one. Counting draws rather than types, because the arm
    /// changes how often the spawner acts and not what it picks.
    #[test]
    fn a_cursed_player_carries_their_luck_into_the_spawner() {
        let (world, (px, py)) = forest_surface();
        let draws = |luck: f32| {
            // No town NPCs, unlike the friendly-draw harness above: the rate arm is gated on
            // `!spawnFriendly` (`NPC.cs:925`), and three Guides beside the player is exactly the
            // town suppression that sets it. With them in place this test cannot see the arm at
            // all, which is how it read 794 against 796 before they came out.
            let npcs = NpcStore::new();
            let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
            drop(out_rx);
            let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
            player.state = crate::game::ConnState::Playing;
            player.position = (px as f32 * 16.0, py as f32 * 16.0);
            player.luck = luck;
            let players = vec![Some(player)];
            let mut rng = SmallRng::seed_from_u64(689);
            let mut biomes = BiomeCache::default();
            let mut total = 0usize;
            for _ in 0..300_000 {
                total += try_spawn(
                    &world,
                    &npcs,
                    &players,
                    &quiet(),
                    &JourneyPowers::default(),
                    &mut biomes,
                    &mut rng,
                )
                .len();
            }
            total
        };
        let neutral = draws(0.0);
        let cursed = draws(-0.7);
        assert!(neutral > 0, "the harness must actually be spawning");
        assert!(
            cursed > neutral,
            "a cursed player's world is busier, and the only way that shows here is if their luck \
             reached `Conditions`: {cursed} draws against {neutral}"
        );
    }

    /// A Living Tree is where the gnomes are, and vanilla asks about it twice over: once at the
    /// spawn candidate's own wall, where above ground a gnome is one attempt in two
    /// (`NPC.cs:1586`, `:1625-1628`), and once at the *player's*, wherever in the column the
    /// candidate landed (`:3625-3628`, `livingTree` being `Main.tile[pX, pY].wall == 244` at
    /// `:414`).
    ///
    /// Fails before the fix: 624 was in no pool and no branch, and its own routine
    /// (`AI_003_Gnomes_ShouldTurnToStone`, and the petrified pose that goes with it) could never
    /// run in a world.
    ///
    /// The two arms are separated here rather than tested together, because a world with living
    /// wood everywhere opens both at once and neutralising either one on its own then proves
    /// nothing: the wall goes only on the columns the candidates can reach for the first, and only
    /// on the player's own tile for the second.
    ///
    /// Neutralised by making the `spawn_wall_type(world, x, y) == LIVING_WOOD_WALL` arm unreachable:
    /// the candidate assertion fails and the other two pass. Neutralised again by making the
    /// `player_wall == LIVING_WOOD_WALL` arm unreachable: the player assertion fails instead.
    #[test]
    fn a_living_tree_is_where_the_gnomes_are() {
        let at = (800, 88);
        // No candidate ever lands within `SAFE_RANGE_X` of the player, so columns 739 to 861 are
        // the player's own and nobody else's, and everything outside them is a candidate's.
        let wall_between = |world: &mut World, from: i32, to: i32| {
            for x in from..to {
                for y in 85..95 {
                    let mut tile = world.tile(x, y);
                    tile.wall = LIVING_WOOD_WALL;
                    world.set_tile(x, y, tile);
                }
            }
        };

        let plain = flat_world_sized(1600, 90, 1);
        let bare = spawns_at(&plain, false, at.0, at.1, 100_000);
        assert!(
            !bare.contains(&GNOME),
            "a gnome out of a plain stone wall: {bare:?}"
        );

        // The candidate's own wall, with the player standing outside the tree.
        let mut candidate_side = flat_world_sized(1600, 90, 1);
        wall_between(&mut candidate_side, 700, 739);
        wall_between(&mut candidate_side, 862, 900);
        assert_eq!(candidate_side.tile(at.0, at.1).wall, 0);
        let outside = spawns_at(&candidate_side, false, at.0, at.1, 100_000);
        assert!(
            outside.contains(&GNOME),
            "no gnome from a living tree the candidate stood in: {outside:?}"
        );

        // ...and the player's own, with every candidate outside it. At night, because that arm
        // takes `!Main.dayTime || Main.tile[spawnTileX, spawnTileY].wall > 0` and the candidates
        // here have no wall at all.
        let mut player_side = flat_world_sized(1600, 90, 1);
        player_side.day_time = false;
        wall_between(&mut player_side, at.0, at.0 + 1);
        let inside = spawns_at(&player_side, false, at.0, at.1, 100_000);
        assert!(
            inside.contains(&GNOME),
            "no gnome for a player standing in a living tree: {inside:?}"
        );
    }

    /// ...and a dirt-walled cave in the narrow band around the surface line is the other place, at
    /// one attempt in ten (`NPC.cs:3629` -> `CheckToSpawnUndergroundGnomes`).
    ///
    /// The unwalled control runs after dark on purpose. The game has two wall gates one above the
    /// other, `if (Main.dayTime && tile.wall <= 0) return false;` and then the wall set, and the
    /// first is dead: everything the set accepts is nonzero, so the only tile the daylight line can
    /// turn down is one the set turns down anyway. Testing the set in daylight would therefore prove
    /// nothing about it. (Dropping that daylight line changes no outcome at all, which is why there
    /// is no neutralisation listed for it; see [`underground_gnome`].)
    ///
    /// Neutralised by returning `false` from `underground_gnome`'s first line: the walled assertion
    /// fails. Neutralised again by dropping the `GNOME_WALLS.contains(&wall)` line: the after-dark
    /// assertion fails, gnomes coming out of bare stone.
    #[test]
    fn a_dirt_walled_cave_near_the_surface_is_the_other_gnome() {
        // `worldSurface` is 100 here, so the band is rows 80 to 110 and the floor at 90 is in it.
        let at = (800, 88);

        // Unwalled and after dark, so the daylight gate is open and the wall set is the only thing
        // left to turn it down.
        let mut night = flat_world_sized(1600, 90, 1);
        night.day_time = false;
        let dark = spawns_at(&night, false, at.0, at.1, 100_000);
        assert!(
            !dark.contains(&GNOME),
            "a gnome out of a bare stone cave: {dark:?}"
        );

        // Unwalled in daylight, which is the other gate.
        let day = flat_world_sized(1600, 90, 1);
        let lit = spawns_at(&day, false, at.0, at.1, 100_000);
        assert!(
            !lit.contains(&GNOME),
            "a gnome out of an unwalled cave in daylight: {lit:?}"
        );

        // ...and the same cave with dirt walls in it.
        let mut world = flat_world_sized(1600, 90, 1);
        for x in 0..world.width() {
            for y in 90..94 {
                let mut tile = world.tile(x, y);
                tile.wall = GNOME_WALLS[0];
                world.set_tile(x, y, tile);
            }
        }
        let walled = spawns_at(&world, false, at.0, at.1, 100_000);
        assert!(
            walled.contains(&GNOME),
            "no gnome in a dirt-walled cave: {walled:?}"
        );
    }

    /// The underground fairy, whose whole gate is whether the world still has a fallen log in it
    /// (`NPC.cs:3616-3624` -> [`underground_fairy`], `NPC.cs:5820-5848`).
    ///
    /// All three colours were in no pool and no branch, which made `game::ai::fairy` dead code: the
    /// one routine in the game whose entire purpose is to lead a player to ore, fully built here,
    /// and nothing could ever put one in a world. It has no biome gate, no hour gate and no
    /// progression gate, so a fresh character digging deep can meet one on their first day.
    ///
    /// The three depth and log assertions are separate claims and are asked separately: the log is
    /// asked end to end through `try_spawn` because that is the wiring worth proving, and the two
    /// depth edges through the gate itself, because at one draw in five hundred a spawn-tick
    /// harness would need millions of ticks to say anything about a row that is one off the line.
    ///
    /// Neutralised by dropping `events.fairy_log` from `underground_fairy`'s first line: the second
    /// assertion fails, "a world with no fallen log in it drew a fairy". Neutralised again by
    /// deleting the whole arm from `try_spawn`'s chain: the first fails, "no FairyCritterPink (583)
    /// in a logged world".
    #[test]
    fn a_fallen_log_is_what_puts_fairies_underground() {
        // 200,000 was a 1-in-3 coin toss for seeing all three colours (mean 2.5 draws at a 1/1000
        // gate); a merge-verifier's multi-seed probe found 1,000,000 gives 12/12 fresh streams.
        const TICKS: u32 = 1_000_000;
        // `flat_world_of` puts the surface at 100 and the rock layer at 200 in a world 600 rows
        // deep, so the gate's window is row 150 (their halfway line) up to row 299, exclusive.
        let floor = 255;
        let cave = flat_world_of(floor, 1);
        let at = (400, floor - 1);
        assert_eq!(depth_at(&cave, at.1), Depth::Cavern);

        let logged = EventSpawns {
            fairy_log: true,
            ..quiet()
        };
        let seen: std::collections::BTreeSet<u16> = spawns_with(&cave, &logged, at.0, at.1, TICKS)
            .into_iter()
            .map(|(npc_type, _)| npc_type)
            .collect();
        for colour in 0..3u16 {
            let npc_type = FAIRY_CRITTER_PINK + colour;
            assert!(
                seen.contains(&npc_type),
                "no fairy ({npc_type}) in a logged world: {seen:?}"
            );
        }

        // ...and a world whose logs have all been mined has none of the three.
        let bare: std::collections::BTreeSet<u16> = spawns_with(&cave, &quiet(), at.0, at.1, TICKS)
            .into_iter()
            .map(|(npc_type, _)| npc_type)
            .collect();
        assert!(
            !(FAIRY_CRITTER_PINK..=585).any(|npc_type| bare.contains(&npc_type)),
            "a world with no fallen log in it drew a fairy: {bare:?}"
        );

        // The two depth edges, asked of the gate directly. Both are exact: `< halfway` turns 149
        // down and keeps 150, and `>= height - 300` turns 300 down and keeps 299.
        let window = |ground_y: i32| {
            (0..40_000u64).any(|seed| {
                let mut rng = SmallRng::seed_from_u64(seed);
                underground_fairy(&cave, true, false, ground_y, &|| false, 0.0, &mut rng)
            })
        };
        assert!(
            !window(149),
            "a fairy above the surface-to-rock halfway line"
        );
        assert!(window(150), "no fairy on the halfway line itself");
        assert!(
            window(299),
            "no fairy one row above the bottom three hundred"
        );
        assert!(!window(300), "a fairy in the bottom three hundred rows");

        // ...and one already out keeps the next away (`NPC.AnyHelpfulFairies`, `NPC.cs:5843`).
        assert!(
            !(0..40_000u64).any(|seed| {
                let mut rng = SmallRng::seed_from_u64(seed);
                underground_fairy(&cave, true, false, 200, &|| true, 0.0, &mut rng)
            }),
            "a second fairy while one was already leading somebody"
        );

        // Hardmode makes them rarer rather than commoner: `(int)(500f * 1.66f)` is 830
        // (`NPC.cs:5829`), so the same seed count finds fewer.
        let hits = |hard_mode: bool| {
            (0..500_000u64)
                .filter(|seed| {
                    let mut rng = SmallRng::seed_from_u64(*seed);
                    underground_fairy(&cave, true, hard_mode, 200, &|| false, 0.0, &mut rng)
                })
                .count()
        };
        let (early, late) = (hits(false), hits(true));
        assert!(
            (800..1200).contains(&early),
            "one in five hundred came up {early} times in 500,000"
        );
        assert!(
            (450..750).contains(&late),
            "one in eight hundred and thirty came up {late} times in 500,000"
        );
    }

    /// Halloween is a real-world date, and it dresses the night up: the Raven, the two costumed
    /// demon eyes and the three costumed zombies (`NPC.cs:4539`, `:4561`, `:4734`).
    ///
    /// Neutralised by forcing `Seasonal::halloween` to `false` where `try_spawn` builds it: none of
    /// the six types below is drawn in two hundred thousand ticks.
    #[test]
    fn halloween_dresses_the_night_up() {
        let (mut world, (px, py)) = night_world();
        world.halloween = true;
        let seen = spawns_at_in(&world, false, false, px, py, 200_000);
        let found: std::collections::BTreeSet<u16> = seen.iter().copied().collect();
        for (npc_type, name) in [
            (301u16, "Raven"),
            (317, "DemonEyeOwl"),
            (318, "DemonEyeSpaceship"),
            (319, "ZombieDoctor"),
            (320, "ZombieSuperman"),
            (321, "ZombiePixie"),
        ] {
            assert!(
                found.contains(&npc_type),
                "no {name} ({npc_type}) at Halloween: {found:?}"
            );
        }
    }

    /// The caverns have a season too, and it is a different chain from the surface's: October
    /// dresses the skeletons up (`NPC.cs:5115`, `Main.rand.Next(322, 325)`) and October *or* a
    /// graveyard puts a Ghost in the stone (`NPC.cs:5078`, unlike the surface's `:4544`, which is
    /// the graveyard's alone).
    ///
    /// Neutralised arm by arm, each run and each failing its own assertion:
    ///
    /// * dropping the `NPC.cs:5115` branch: "no SkeletonTopHat (322) in an October cavern: {316}".
    /// * cutting `:5078`'s condition down to `at.graveyard` alone: "no Ghost (316) in an October
    ///   cavern: {322, 323, 324}".
    /// * cutting it down to `at.halloween` alone instead: "no Ghost in an underground graveyard".
    /// * dropping `!no_worms` from `:5078`: "a Ghost got past `noWorms`".
    /// * dropping `lower_caverns` from `:5021`: "an ordinary cavern answered the seasonal chain",
    ///   Tim having reached the upper stone.
    /// * dropping the `:5051` hardmode gate: "hardmode should reach this chain far less often:
    ///   6879 vs 6879".
    #[test]
    fn the_caverns_have_a_season_of_their_own() {
        // Plain stone underfoot: the two arms that key on the floor are the stone test below.
        let sample = |at: Seasonal, no_worms: bool, lower_caverns: bool| {
            (0..40_000u64)
                .filter_map(|seed| {
                    let mut rng = SmallRng::seed_from_u64(seed);
                    cavern_seasonal_pick(
                        at,
                        no_worms,
                        lower_caverns,
                        1,
                        false,
                        &|_| false,
                        &mut rng,
                    )
                })
                .collect::<Vec<u16>>()
        };
        let set = |v: Vec<u16>| v.into_iter().collect::<std::collections::BTreeSet<u16>>();
        let october = Seasonal {
            halloween: true,
            ..Seasonal::default()
        };
        let graveyard = Seasonal {
            graveyard: true,
            ..Seasonal::default()
        };
        let plain = Seasonal::default();

        let found = set(sample(october, false, false));
        for (npc_type, name) in [
            (316u16, "Ghost"),
            (322, "SkeletonTopHat"),
            (323, "SkeletonAstonaut"),
            (324, "SkeletonAlien"),
        ] {
            assert!(
                found.contains(&npc_type),
                "no {name} ({npc_type}) in an October cavern: {found:?}"
            );
        }

        // A graveyard underground is the Ghost and only the Ghost: the costumes are the calendar's.
        let found = set(sample(graveyard, false, false));
        assert!(
            found.contains(&316),
            "no Ghost in an underground graveyard: {found:?}"
        );
        for npc_type in [322u16, 323, 324] {
            assert!(
                !found.contains(&npc_type),
                "{npc_type} wore a costume in a June graveyard: {found:?}"
            );
        }

        // An ordinary cavern in the upper stone answers the Lost Girl (`NPC.cs:5012`) and the four
        // skeletons of the chain's own fallthrough (`:5141-5199`), and nothing else: neither
        // carries a season or a progression gate, so everything above is the season's doing rather
        // than the chain's.
        assert_eq!(
            set(sample(plain, false, false)),
            std::collections::BTreeSet::from([21u16, 195, 201, 202, 203]),
            "an ordinary cavern answered more than the Lost Girl and the skeletons"
        );
        // ...and the bottom half of the stone adds exactly one more: Tim (`NPC.cs:5021`).
        assert_eq!(
            set(sample(plain, false, true)),
            std::collections::BTreeSet::from([21u16, 45, 195, 201, 202, 203]),
            "the lower caverns should add Tim and nothing else"
        );

        // `!noWorms` is vanilla's own gate on the Ghost, odd as it looks on something that floats.
        assert!(
            !set(sample(graveyard, true, false)).contains(&316),
            "a Ghost got past `noWorms`"
        );

        // `NPC.cs:5051` swallows nine hardmode draws in ten before the season is ever asked.
        let classic = sample(october, false, false).len();
        let hard = sample(
            Seasonal {
                hard_mode: true,
                ..october
            },
            false,
            false,
        )
        .len();
        assert!(
            hard * 4 < classic,
            "hardmode should reach this chain far less often: {hard} vs {classic}"
        );
    }

    /// The same chain through `try_spawn`, which is what proves it is wired to the caverns rather
    /// than merely written: an October cavern brings up costumed skeletons and an ordinary one does
    /// not.
    ///
    /// Neutralised by pointing `try_spawn`'s `cavern_chain` gate at `Depth::Underworld` instead of
    /// `Depth::Cavern`, so the chain is still written and still called but no longer reaches the
    /// caverns: "no SkeletonTopHat (322) in an October cavern: {10, 16, 21, 44, 49, 93, 354, 453,
    /// 496, 497, 498, 504, 506}", the plain cavern pool and nothing else.
    #[test]
    fn halloween_reaches_down_into_the_caverns() {
        let mut world = flat_world(250);
        let (px, py) = (400, 248);
        assert_eq!(depth_at(&world, py), Depth::Cavern);
        assert_eq!(biome_at(&world, px, py), Biome::Forest);
        world.halloween = true;

        let seen = spawns_at_in(&world, false, false, px, py, 200_000);
        let found: std::collections::BTreeSet<u16> = seen.iter().copied().collect();
        for (npc_type, name) in [
            (322u16, "SkeletonTopHat"),
            (323, "SkeletonAstonaut"),
            (324, "SkeletonAlien"),
        ] {
            assert!(
                found.contains(&npc_type),
                "no {name} ({npc_type}) in an October cavern: {found:?}"
            );
        }

        world.halloween = false;
        let ordinary = spawns_at_in(&world, false, false, px, py, 200_000);
        let ordinary: std::collections::BTreeSet<u16> = ordinary.iter().copied().collect();
        for npc_type in [322u16, 323, 324] {
            assert!(
                !ordinary.contains(&npc_type),
                "{npc_type} turned up in an ordinary cavern: {ordinary:?}"
            );
        }
    }

    /// `GetZombieSettings` (`NPC.cs:5595-5619`) on its own: the torch zombie's odds are not the
    /// constant they look like, and the style is drawn once for the whole attempt.
    ///
    /// The `playerHasStartingHealth` half (`NPC.cs:416`, `statLifeMax <= 100`) is the one that
    /// matters in practice: a character who has not eaten a life crystal yet is what almost every
    /// player on a fresh server is, and it more than doubles how often the night hands out a torch.
    ///
    /// Neutralised by deleting the `if starting_health` block: every row of the second table comes
    /// back as 12 and the first assertion in it fails. Neutralised the other way, by dropping the
    /// `.max(2)`, the six- and twenty-player rows fail instead.
    #[test]
    fn a_fresh_character_doubles_the_torch_zombies_odds() {
        let mut rng = SmallRng::seed_from_u64(4);
        // NPC.cs:5599, the bare default: a player past their first life crystals, at any headcount.
        for players in [1u32, 4, 20] {
            let settings = zombie_settings(false, players, &mut rng);
            assert_eq!(settings.torch_chance, 12, "{players} players");
            // NPC.cs:5598. Nothing outside a Skyblock world ever clears it.
            assert!(settings.armed);
        }
        // NPC.cs:5606-5613: five, less half the headcount as an integer, floored at two.
        for (players, expected) in [
            (1u32, 5u32),
            (2, 4),
            (3, 4),
            (4, 3),
            (5, 3),
            (6, 2),
            (20, 2),
        ] {
            assert_eq!(
                zombie_settings(true, players, &mut rng).torch_chance,
                expected,
                "{players} players on a fresh character"
            );
        }
        // NPC.cs:5601, `Main.rand.Next(7)`: all seven, and nothing outside them.
        let styles: std::collections::BTreeSet<u8> = (0..500u64)
            .map(|seed| zombie_settings(false, 1, &mut SmallRng::seed_from_u64(seed)).style)
            .collect();
        assert_eq!(styles, (0..7u8).collect());
    }

    /// The surface night's own fallthrough, through `try_spawn`: the seven plain zombies
    /// (`NPC.cs:4771-4816`) and the Torch Zombie above them (`:4722`).
    ///
    /// Six of the seven had no producer anywhere in this server, because the chain used to hand the
    /// draw back to [`pool`] here and the pool's surface night is the plain Zombie and the Demon Eye.
    /// The Torch Zombie is the bigger miss of the two: its arm carries no season, no biome and no
    /// progression gate at all, so on a server of fresh characters it is one attempt in five, and it
    /// was simply absent.
    ///
    /// Neutralised by returning `None` instead of the closing `Some(match zombie.style ...)`: the
    /// six style assertions fail (the observed set is "{2, 3, 190, 191, 192, 193, 194, 590}", the
    /// pool's own night, the coloured eyes' own arm above the tail, and the torch arm above that).
    /// Neutralised the other way, by deleting the `NPC.cs:4722` arm, the torch assertion fails on
    /// its own and the styles keep coming.
    #[test]
    fn an_ordinary_night_hands_out_torches_and_all_seven_zombies() {
        let (world, (px, py)) = night_world();
        let seen: std::collections::BTreeSet<u16> = spawns_at(&world, false, px, py, 200_000)
            .into_iter()
            .collect();
        assert!(seen.contains(&590), "no Torch Zombie (590): {seen:?}");
        for (npc_type, name) in [
            (3u16, "Zombie"),
            (132, "BaldZombie"),
            (186, "PincushionZombie"),
            (187, "SlimedZombie"),
            (188, "SwampZombie"),
            (189, "TwiggyZombie"),
            (200, "FemaleZombie"),
        ] {
            assert!(
                seen.contains(&npc_type),
                "no {name} ({npc_type}) on an ordinary night: {seen:?}"
            );
        }
        // Every armed one is behind `Main.expertMode` and this world is classic.
        for npc_type in [430u16, 431, 432, 433, 434, 435, 436, 591] {
            assert!(
                !seen.contains(&npc_type),
                "{npc_type} turned up in a classic world: {seen:?}"
            );
        }
    }

    /// ...and what expert mode adds to that night: the armed zombies (`NPC.cs:4744-4769`) and the
    /// Armed Torch Zombie (`:4724`).
    ///
    /// `Main.expertMode` is `Main.Difficulty >= 2` (`Main.cs:2785`), which is `game_mode == 1` here.
    /// Note which style has no armed twin: case 1, the Bald Zombie, is excluded by the arm's own
    /// gate, so 132 keeps turning up in an expert world where the other six thin out.
    ///
    /// Neutralised by dropping `at.expert` from the `NPC.cs:4744` gate: 430 and 432-436 then turn up
    /// in the classic run of the test above and its exclusion assertions fail. Neutralised by
    /// deleting the arm outright, the six assertions here fail and the classic test still passes.
    #[test]
    fn expert_mode_arms_the_zombies() {
        let (mut world, (px, py)) = night_world();
        world.game_mode = 1;
        let seen: std::collections::BTreeSet<u16> = spawns_at(&world, false, px, py, 200_000)
            .into_iter()
            .collect();
        for (npc_type, name) in [
            (430u16, "ArmedZombie"),
            (432, "ArmedZombiePincussion"),
            (433, "ArmedZombieSlimed"),
            (434, "ArmedZombieSwamp"),
            (435, "ArmedZombieTwiggy"),
            (436, "ArmedZombieCenx"),
            (591, "ArmedTorchZombie"),
        ] {
            assert!(
                seen.contains(&npc_type),
                "no {name} ({npc_type}) in an expert world: {seen:?}"
            );
        }
        // The Bald Zombie has no armed twin, so style 1 still answers with the plain one.
        assert!(seen.contains(&132), "no BaldZombie (132): {seen:?}");
    }

    /// The snow arm's own third branch (`NPC.cs:4665`), which is the only place in the whole spawner
    /// the Armed Zombie Eskimo comes from.
    ///
    /// The arm keys on the tile underfoot rather than on the player's zone, and it `return`s, so the
    /// rest of the night's tail never runs with ice under the spawn: an expert snowfield hands out
    /// 431 and no armed zombie of any other kind.
    ///
    /// The floor is a single row of ice, which is nowhere near the three hundred tiles the zone scan
    /// wants, so this is a *forest* with ice underfoot: exactly the case that proves the arm keys on
    /// the tile. What answers when the arm hands back is the ordinary surface-night pool.
    ///
    /// Neutralised by deleting the `NPC.cs:4665` branch so the arm falls straight through to its
    /// `return None`: "no ArmedZombieEskimo (431) on expert ice: {2, 3, 190, 191, 192, 193, 194}",
    /// the surface night's pool and its coloured eyes and nothing else.
    #[test]
    fn expert_ice_underfoot_is_the_armed_eskimos_only_home() {
        let mut world = flat_world_sized(1600, 90, 161);
        world.day_time = false;
        world.game_mode = 1;
        let (px, py) = (800, 88);
        assert_eq!(depth_at(&world, py), Depth::Surface);

        let seen: std::collections::BTreeSet<u16> = spawns_at(&world, false, px, py, 200_000)
            .into_iter()
            .collect();
        assert!(
            seen.contains(&431),
            "no ArmedZombieEskimo (431) on expert ice: {seen:?}"
        );
        for npc_type in [430u16, 432, 433, 434, 435, 436, 590, 591] {
            assert!(
                !seen.contains(&npc_type),
                "{npc_type} got past the snow arm's own return: {seen:?}"
            );
        }
    }

    /// The caverns' own fallthrough (`NPC.cs:5141-5199`) and the expert arm above it (`:5120`),
    /// through `try_spawn`.
    ///
    /// Two of the four Bone Throwing Skeletons are dead in the game itself: `num56` is drawn once
    /// and then tested against zero three times over, so 450 and 451 cannot be reached by any
    /// vanilla player either and stay in `docs/spawn-gaps.tsv`. That is asserted here rather than
    /// left as a comment, because the tempting thing to do with that arm is to "fix" it.
    ///
    /// Neutralised by returning `None` instead of the closing `Some(SKELETONS[...])`: "no
    /// HeadacheSkeleton (201) in the caverns: {10, 16, 21, 44, 49, 93, 195, 217, 300, 354, 357, 359,
    /// 447, 453, 496, 497, 498, 504, 506}", the plain cavern pool and the arms above. Neutralised by
    /// deleting the `:5120` arm instead, "no BoneThrowingSkeleton (449) in an expert cavern" fails
    /// with 201, 202 and 203 present in that same set, so the two halves are independent.
    #[test]
    fn the_caverns_hand_out_the_skeleton_look_alikes_and_expert_adds_the_throwers() {
        let world = flat_world(250);
        let (px, py) = (400, 248);
        assert_eq!(depth_at(&world, py), Depth::Cavern);

        let classic: std::collections::BTreeSet<u16> = spawns_at(&world, false, px, py, 200_000)
            .into_iter()
            .collect();
        for (npc_type, name) in [
            (21u16, "Skeleton"),
            (201, "HeadacheSkeleton"),
            (202, "MisassembledSkeleton"),
            (203, "PantlessSkeleton"),
        ] {
            assert!(
                classic.contains(&npc_type),
                "no {name} ({npc_type}) in the caverns: {classic:?}"
            );
        }
        for npc_type in [449u16, 450, 451, 452] {
            assert!(
                !classic.contains(&npc_type),
                "{npc_type} turned up in a classic world: {classic:?}"
            );
        }

        let mut world = world;
        world.game_mode = 1;
        let expert: std::collections::BTreeSet<u16> = spawns_at(&world, false, px, py, 200_000)
            .into_iter()
            .collect();
        assert!(
            expert.contains(&449),
            "no BoneThrowingSkeleton (449) in an expert cavern: {expert:?}"
        );
        assert!(
            expert.contains(&452),
            "no BoneThrowingSkeleton4 (452) in an expert cavern: {expert:?}"
        );
        for npc_type in [450u16, 451] {
            assert!(
                !expert.contains(&npc_type),
                "{npc_type} is unreachable in the game itself and must stay unreachable here: \
                 {expert:?}"
            );
        }
    }

    /// Doctor Bones (`NPC.cs:3772-3775`), whose whole gate is jungle grass underfoot and a dark sky.
    ///
    /// No depth, no biome and no progression: he is as much a surface find as an underground one,
    /// which is why he sits in `try_spawn`'s own outer chain rather than in any pool. Nothing in
    /// this server drew him at all before.
    ///
    /// Neutralised by deleting the arm: "no Doctor Bones (52) on a jungle surface at night:
    /// {42, 51}", the jungle's own surface pair and nothing else. With the arm kept but
    /// `!world.day_time` dropped, "Doctor Bones in broad daylight: {42, 51, 52}" fails instead.
    #[test]
    fn doctor_bones_walks_the_jungle_grass_after_dark() {
        let mut world = flat_world_sized(1600, 90, JUNGLE_GRASS);
        world.day_time = false;
        let (px, py) = (800, 88);
        assert_eq!(depth_at(&world, py), Depth::Surface);

        let night: std::collections::BTreeSet<u16> = spawns_at(&world, false, px, py, 400_000)
            .into_iter()
            .collect();
        assert!(
            night.contains(&DOCTOR_BONES),
            "no Doctor Bones ({DOCTOR_BONES}) on a jungle surface at night: {night:?}"
        );

        world.day_time = true;
        let day: std::collections::BTreeSet<u16> = spawns_at(&world, false, px, py, 400_000)
            .into_iter()
            .collect();
        assert!(
            !day.contains(&DOCTOR_BONES),
            "Doctor Bones in broad daylight: {day:?}"
        );

        // ...and the same arm underground, since it carries no depth gate either.
        let mut deep = cavern_of(JUNGLE_GRASS);
        deep.day_time = false;
        let under: std::collections::BTreeSet<u16> = spawns_at(&deep, false, 400, 250, 400_000)
            .into_iter()
            .collect();
        assert!(
            under.contains(&DOCTOR_BONES),
            "no Doctor Bones underground: {under:?}"
        );
    }

    /// The caverns hand out a Lost Girl one attempt in eighty (`NPC.cs:5012-5015`), and she is the
    /// only path into the Nymph there has ever been.
    ///
    /// Both halves are asserted, because either one alone would be a half-wired lane: `try_spawn`
    /// really puts a 195 in a cavern, and the routine she arrives with really turns her into a 196
    /// (`game::ai::ambush::lost_girl`, `NPC.cs:30360-30389`). Vanilla's own ambient spawning never
    /// produces a 196 by any other route either, which is why 196 is not in `docs/spawn-gaps.tsv`
    /// while 195 was.
    ///
    /// The transformation half is `game::ai::ambush`'s own three tests, which already drive the
    /// routine through all three of vanilla's tells and assert it hands back a 196; this is the
    /// spawn half, which was the missing one.
    ///
    /// Neutralised by deleting the `one_in(rng, 80)` arm from [`cavern_seasonal_pick`]: "no Lost
    /// Girl in the caverns", 195 never appearing among the 200,000 attempts.
    #[test]
    fn the_caverns_hand_out_a_lost_girl() {
        let world = flat_world(250);
        let (px, py) = (400, 248);
        assert_eq!(depth_at(&world, py), Depth::Cavern);

        let found: std::collections::BTreeSet<u16> = spawns_at(&world, false, px, py, 200_000)
            .into_iter()
            .collect();
        assert!(
            found.contains(&195),
            "no Lost Girl in the caverns: {found:?}"
        );
    }

    /// Hallowed ground's own chain (`NPC.cs:4039-4061`), arm by arm.
    ///
    /// Three of its five are found nowhere else in `NPC.Spawner` at all, and the first of them is
    /// the Empress of Light's only summon, so the whole of that boss hung off this one branch.
    ///
    /// Neutralised arm by arm, each run and each failing its own assertion:
    ///
    /// * dropping `downed_plant_boss` from `:4041`: "a Lacewing before Plantera".
    /// * dropping `time < LACEWING_LATEST`: "a Lacewing after midnight".
    /// * dropping `surface_spawn`: "a Lacewing underground".
    /// * dropping `!day_time`: "a Lacewing in daylight".
    /// * dropping `!alive(661)`: "a second Lacewing while one is already up".
    /// * dropping `raining` from `:4045`: "a Rainbow Slime in the dry".
    /// * dropping `!alive(244)`: "a second Rainbow Slime while one is already up".
    /// * deleting the `:4053` arm: "no Unicorn on hallowed ground: {}".
    #[test]
    fn hallowed_ground_has_a_chain_of_its_own() {
        let sample = |downed: bool, day: bool, time: i32, surface: bool, raining: bool| {
            (0..40_000u64)
                .filter_map(|seed| {
                    let mut rng = SmallRng::seed_from_u64(seed);
                    hallow_ground_pick(
                        downed,
                        day,
                        time,
                        surface,
                        raining,
                        &|_| false,
                        0.0,
                        &mut rng,
                    )
                })
                .collect::<std::collections::BTreeSet<u16>>()
        };

        // Plantera down, the first half of a dry night, on the surface: the Lacewing's own window.
        let window = sample(true, false, 0, true, false);
        assert!(
            window.contains(&PRISMATIC_LACEWING),
            "no Lacewing in its own window: {window:?}"
        );
        // ...and the Unicorn, which waits on nothing but the ground it stands on.
        assert!(
            window.contains(&86),
            "no Unicorn on hallowed ground: {window:?}"
        );

        // Each of the Lacewing's four world conditions, taken away one at a time.
        assert!(
            !sample(false, false, 0, true, false).contains(&PRISMATIC_LACEWING),
            "a Lacewing before Plantera"
        );
        assert!(
            !sample(true, false, LACEWING_LATEST, true, false).contains(&PRISMATIC_LACEWING),
            "a Lacewing after midnight"
        );
        assert!(
            !sample(true, false, 0, false, false).contains(&PRISMATIC_LACEWING),
            "a Lacewing underground"
        );
        assert!(
            !sample(true, true, 0, true, false).contains(&PRISMATIC_LACEWING),
            "a Lacewing in daylight"
        );

        // `!AnyNPCs(661)`: one at a time, which is what stops a hallow night raining Empresses.
        let crowded = (0..40_000u64)
            .filter_map(|seed| {
                let mut rng = SmallRng::seed_from_u64(seed);
                hallow_ground_pick(
                    true,
                    false,
                    0,
                    true,
                    false,
                    &|ty| ty == PRISMATIC_LACEWING,
                    0.0,
                    &mut rng,
                )
            })
            .collect::<std::collections::BTreeSet<u16>>();
        assert!(
            !crowded.contains(&PRISMATIC_LACEWING),
            "a second Lacewing while one is already up: {crowded:?}"
        );

        // The Rainbow Slime wants rain and one of itself, and nothing else.
        assert!(
            sample(false, true, 0, false, true).contains(&244),
            "no Rainbow Slime in the rain"
        );
        assert!(
            !sample(false, true, 0, false, false).contains(&244),
            "a Rainbow Slime in the dry"
        );
        let wet = (0..40_000u64)
            .filter_map(|seed| {
                let mut rng = SmallRng::seed_from_u64(seed);
                hallow_ground_pick(false, true, 0, false, true, &|ty| ty == 244, 0.0, &mut rng)
            })
            .collect::<std::collections::BTreeSet<u16>>();
        assert!(
            !wet.contains(&244),
            "a second Rainbow Slime while one is already up: {wet:?}"
        );
    }

    /// The same chain through `try_spawn`, which is what proves it is wired to hallowed ground
    /// rather than merely written, and that it reads the tile underfoot rather than the biome.
    ///
    /// Neutralised by changing the arm's `HALLOW_GROUND.contains(&g)` to `g == SAND`, so the chain
    /// is still written and still called but no longer reaches pearlstone: "no Prismatic Lacewing
    /// (661) on hallowed ground", and no Unicorn either.
    #[test]
    fn hallowed_ground_reaches_the_lacewing_through_try_spawn() {
        const HALLOWED_GRASS: u16 = 109;
        let mut world = flat_world_of(80, HALLOWED_GRASS);
        let (px, py) = (400, 78);
        assert_eq!(depth_at(&world, py), Depth::Surface);
        world.progress.downed_plantera = true;
        world.day_time = false;
        world.time = 0;
        world.raining = true;

        let found: std::collections::BTreeSet<u16> = spawns_at(&world, true, px, py, 200_000)
            .into_iter()
            .collect();
        for (npc_type, name) in [
            (PRISMATIC_LACEWING, "Prismatic Lacewing"),
            (244u16, "Rainbow Slime"),
            (86, "Unicorn"),
        ] {
            assert!(
                found.contains(&npc_type),
                "no {name} ({npc_type}) on hallowed ground: {found:?}"
            );
        }

        // The same night on plain stone, which no amount of hallow in the air makes hallowed
        // ground: vanilla's gate is the tile, and there is no `ZoneHallow` anywhere in it.
        let mut stone = flat_world_of(80, 1);
        stone.progress.downed_plantera = true;
        stone.day_time = false;
        stone.time = 0;
        stone.raining = true;
        let ordinary: std::collections::BTreeSet<u16> = spawns_at(&stone, true, px, py, 200_000)
            .into_iter()
            .collect();
        assert!(
            !ordinary.contains(&PRISMATIC_LACEWING),
            "a Lacewing on plain stone: {ordinary:?}"
        );
    }

    /// A meteor crater answers with Meteor Heads and with nothing else at all (`NPC.cs:2796-2799`).
    ///
    /// The branch has no roll and no roster inside it: one `SpawnNPC(..., 23)` and a closing brace.
    /// Standing in a crater, that is the whole of it, which is what makes a meteorite field a place
    /// you clear rather than a place you walk through. The same ground without the meteorite draws
    /// the ordinary cavern pool, which is what proves the zone rather than the terrain is doing it.
    ///
    /// Neutralised by deleting the `None if zones.meteor` arm: "a crater spawned 21, not a Meteor
    /// Head", the plain cavern skeleton, and 23 never appears at all.
    #[test]
    fn a_crater_spawns_meteor_heads_and_nothing_else() {
        const METEORITE: u16 = 37;
        let world = flat_world_of(250, METEORITE);
        let (px, py) = (400, 248);
        assert_eq!(depth_at(&world, py), Depth::Cavern);
        assert!(
            zones_at(&world, px, py).meteor,
            "a floor of meteorite is a crater",
        );

        // A bound resident is not a monster and is not the crater's doing: vanilla's own bound arms
        // (`NPC.cs:1662`, `:2087-2098`) all sit *above* `else if (ZoneMeteor)`, so a crater is where
        // one may still be found. Rescues are not what this test is about.
        let monsters = |seen: Vec<u16>| {
            seen.into_iter()
                .filter(|ty| {
                    !terrustia_proto::npc_data::npc_stats(*ty)
                        .expect("a real type")
                        .friendly
                })
                .collect::<Vec<u16>>()
        };
        let seen = monsters(spawns_at(&world, false, px, py, 40_000));
        assert!(!seen.is_empty(), "the crater never spawned anything");
        for npc_type in &seen {
            assert_eq!(
                *npc_type, METEOR_HEAD,
                "a crater spawned {npc_type}, not a Meteor Head",
            );
        }

        // The same cavern floored with stone instead: an ordinary pool, and no Meteor Head in it.
        let stone = flat_world(250);
        assert!(!zones_at(&stone, px, py).meteor);
        let ordinary: std::collections::BTreeSet<u16> =
            monsters(spawns_at(&stone, false, px, py, 40_000))
                .into_iter()
                .collect();
        assert!(
            ordinary.len() > 1 && !ordinary.contains(&METEOR_HEAD),
            "an ordinary cavern drew {ordinary:?}",
        );
    }

    /// The Glowing Mushroom roster hangs off the ground tile, not off `ZoneGlowshroom`.
    ///
    /// This is the thing worth pinning, because it is the opposite of what the zone flag beside it
    /// suggests. Vanilla's two arms (`NPC.cs:3637`, `:3674`) test `tileType == 70` and nothing else
    /// about the biome, so the surface arm is open from the first day of a world and the
    /// underground one waits only on hardmode. Only the Spore pair (`:5110`, `:5209`) wants the
    /// zone as well, and that is the next test.
    ///
    /// Neutralised twice: dropping the `!hard_mode ||` from the underground arm's gate gives "no
    /// TruffleWorm (374) in a hardmode mushroom cave" turned inside out - the worm appears before
    /// hardmode, failing the classic-mode assertion; and returning `None` unconditionally from the
    /// surface arm gives "no ZombieMushroom (254) in a surface mushroom patch: {}".
    #[test]
    fn the_mushroom_roster_hangs_off_the_ground_tile() {
        let sample = |surface: bool, hard_mode: bool| {
            (0..40_000u64)
                .filter_map(|seed| {
                    let mut rng = SmallRng::seed_from_u64(seed);
                    mushroom_pick(surface, hard_mode, false, 0.0, &mut rng)
                })
                .collect::<std::collections::BTreeSet<u16>>()
        };

        // The surface arm, which needs no progression at all.
        let classic = sample(true, false);
        for (npc_type, name) in [
            (254u16, "ZombieMushroom"),
            (255, "ZombieMushroomHat"),
            (257, "AnomuraFungus"),
            (258, "MushiLadybug"),
            (259, "FungiBulb"),
            (360, "GlowingSnail"),
        ] {
            assert!(
                classic.contains(&npc_type),
                "no {name} ({npc_type}) in a surface mushroom patch: {classic:?}",
            );
        }
        // `Main.hardMode && Main.rand.Next(3) != 0` (`NPC.cs:3647`) is the only source of the big
        // bulb, and the Truffle Worm is the underground arm's alone.
        assert!(
            !classic.contains(&260) && !classic.contains(&374),
            "a classic surface patch drew {classic:?}",
        );
        assert!(sample(true, true).contains(&260), "no GiantFungiBulb");

        // The underground arm is hardmode's outright: `Main.hardMode` is part of its own gate.
        assert!(
            sample(false, false).is_empty(),
            "a pre-hardmode mushroom cave answers nothing here",
        );
        let hard = sample(false, true);
        for (npc_type, name) in [
            (374u16, "TruffleWorm"),
            (360, "GlowingSnail"),
            (259, "FungiBulb"),
            (260, "GiantFungiBulb"),
            (257, "AnomuraFungus"),
            (258, "MushiLadybug"),
        ] {
            assert!(
                hard.contains(&npc_type),
                "no {name} ({npc_type}) in a hardmode mushroom cave: {hard:?}",
            );
        }
        // The mushroom zombies are the surface arm's alone: nothing walks in the caves.
        assert!(
            !hard.contains(&254) && !hard.contains(&255),
            "a mushroom zombie underground: {hard:?}",
        );

        // One attempt in three declines outright (`Main.rand.Next(3) != 0` on both arms), which is
        // what lets the ordinary chain still answer inside a mushroom biome.
        let declined = (0..40_000u64)
            .filter(|seed| {
                let mut rng = SmallRng::seed_from_u64(*seed);
                mushroom_pick(true, false, false, 0.0, &mut rng).is_none()
            })
            .count();
        assert!(
            (12_000..15_000).contains(&declined),
            "{declined} of 40000 attempts declined, expected about a third",
        );
    }

    /// The Spore pair is the one thing here that really does want `ZoneGlowshroom`, and it wants the
    /// ground tile too: `ZoneGlowshroom && (tileType == 70 || tileType == 190)` (`NPC.cs:5110` for
    /// the Skeleton, `:5209` for the Bat).
    ///
    /// The two sit in opposite halves of the cavern chain's own coin flip, so a mushroom cavern gets
    /// both and everywhere else gets neither.
    ///
    /// Neutralised by dropping `glowshroom_ground` from both arms (making each unconditional): the
    /// last assertion fails with "an ordinary cavern grew spores: {634}", the bat having reached a
    /// stone cave.
    #[test]
    fn the_spore_pair_wants_the_zone_and_the_ground() {
        let sample = |glowshroom: bool| {
            (0..40_000u64)
                .filter_map(|seed| {
                    let mut rng = SmallRng::seed_from_u64(seed);
                    cavern_seasonal_pick(
                        Seasonal::default(),
                        false,
                        false,
                        1,
                        glowshroom,
                        &|_| false,
                        &mut rng,
                    )
                })
                .collect::<std::collections::BTreeSet<u16>>()
        };
        let mushroom = sample(true);
        assert!(
            mushroom.contains(&634) && mushroom.contains(&635),
            "a mushroom cavern should grow both spores: {mushroom:?}",
        );
        assert!(
            !sample(false).contains(&634) && !sample(false).contains(&635),
            "an ordinary cavern grew spores: {:?}",
            sample(false),
        );
    }

    /// The same three arms through `try_spawn`, which is what proves they are wired to the ground
    /// rather than merely written.
    ///
    /// Neutralised by pinning `mushroom_ground` and `glowshroom_ground` to `false` in `try_spawn`:
    /// "no TruffleWorm (374) standing on mushroom grass" and "no SporeSkeleton (635) in a mushroom
    /// cavern", with the plain cavern pool answering instead.
    #[test]
    fn a_mushroom_cave_is_wired_to_the_spawner() {
        // Deep enough to be caverns, so both the underground mushroom arm and the cavern chain that
        // holds the Spore pair are in play.
        let world = flat_world_of(250, MUSHROOM_GRASS);
        let (px, py) = (400, 248);
        assert_eq!(depth_at(&world, py), Depth::Cavern);
        let zones = zones_at(&world, px, py);
        assert!(zones.glowshroom, "a floor of mushroom grass is the zone");
        assert_eq!(zones.biome, Biome::Forest, "and it is nobody else's biome");

        let hard: std::collections::BTreeSet<u16> = spawns_at(&world, true, px, py, 200_000)
            .into_iter()
            .collect();
        for (npc_type, name) in [
            (374u16, "TruffleWorm"),
            (360, "GlowingSnail"),
            (259, "FungiBulb"),
            (635, "SporeSkeleton"),
            (634, "SporeBat"),
        ] {
            assert!(
                hard.contains(&npc_type),
                "no {name} ({npc_type}) in a mushroom cavern: {hard:?}",
            );
        }

        // Before hardmode the underground arm is shut, so the worm and the snail are gone and the
        // Spore pair - which has no progression gate at all - is not.
        let classic: std::collections::BTreeSet<u16> = spawns_at(&world, false, px, py, 200_000)
            .into_iter()
            .collect();
        assert!(
            !classic.contains(&374),
            "a Truffle Worm before hardmode: {classic:?}",
        );
        assert!(
            classic.contains(&634) && classic.contains(&635),
            "the Spore pair waits on no boss: {classic:?}",
        );

        // ...and the surface arm, which waits on nothing at all.
        let surface = flat_world_of(80, MUSHROOM_GRASS);
        assert_eq!(depth_at(&surface, 78), Depth::Surface);
        let day: std::collections::BTreeSet<u16> = spawns_at(&surface, false, px, 78, 200_000)
            .into_iter()
            .collect();
        for (npc_type, name) in [(254u16, "ZombieMushroom"), (255, "ZombieMushroomHat")] {
            assert!(
                day.contains(&npc_type),
                "no {name} ({npc_type}) on a surface mushroom patch: {day:?}",
            );
        }
    }

    /// Marble and granite are the only home the four of them have (`NPC.cs:5027-5050`), and only
    /// the Medusa waits for hardmode: a granite pocket has its golems and flyers on day one.
    ///
    /// Neutralised arm by arm, each failing its own assertion:
    ///
    /// * deleting the `NPC.cs:5027` marble arm: "no GreekSkeleton (481) on a marble floor: {}".
    /// * deleting `:5039`'s granite arm: "no GraniteFlyer (483) on a granite floor: {}".
    /// * dropping `&& at.hard_mode` from `:5029`: "a Medusa turned up before hardmode".
    /// * dropping `!alive(483)` from `:5041`: "a second Granite Flyer while one is already up".
    /// * dropping the `world.tile(x, y + 1).block` argument in `try_spawn` for a literal `1`: "no
    ///   GraniteGolem (482) in a granite cavern", the arm written but wired to nothing.
    #[test]
    fn marble_and_granite_are_where_those_four_live() {
        let stone = |ground_block: u16, hard_mode: bool, alive: &dyn Fn(u16) -> bool| {
            (0..40_000u64)
                .filter_map(|seed| {
                    let mut rng = SmallRng::seed_from_u64(seed);
                    cavern_seasonal_pick(
                        Seasonal {
                            hard_mode,
                            ..Seasonal::default()
                        },
                        false,
                        false,
                        ground_block,
                        false,
                        alive,
                        &mut rng,
                    )
                })
                .collect::<std::collections::BTreeSet<u16>>()
        };

        // Pre-hardmode marble is the Greek Skeleton alone: `NPC.cs:5029` ends `&& Main.hardMode`.
        let marble = stone(367, false, &|_| false);
        assert!(
            marble.contains(&481),
            "no GreekSkeleton (481) on a marble floor: {marble:?}"
        );
        assert!(!marble.contains(&480), "a Medusa turned up before hardmode");
        assert!(
            stone(367, true, &|_| false).contains(&480),
            "no Medusa on a hardmode marble floor"
        );

        // Granite has no such gate, and its two split on `!AnyNPCs(483)` (`NPC.cs:5041`).
        let granite = stone(368, false, &|_| false);
        for (npc_type, name) in [(483u16, "GraniteFlyer"), (482, "GraniteGolem")] {
            assert!(
                granite.contains(&npc_type),
                "no {name} ({npc_type}) on a granite floor: {granite:?}"
            );
        }
        assert!(
            !stone(368, false, &|ty| ty == 483).contains(&483),
            "a second Granite Flyer while one is already up"
        );

        // Ordinary stone answers none of the four, which is what makes the pocket the reason.
        let plain = stone(1, true, &|_| false);
        for npc_type in [480u16, 481, 482, 483] {
            assert!(
                !plain.contains(&npc_type),
                "{npc_type} turned up on plain rock: {plain:?}"
            );
        }

        // ...and it is wired to the caverns rather than merely written: a granite-floored hall.
        let floor = 300;
        let mut world = hall_world(floor);
        for x in 0..world.width() {
            world.set_tile(x, floor, terrustia_proto::Tile::block(368));
        }
        assert_eq!(depth_at(&world, floor - 1), Depth::Cavern);
        assert_eq!(
            biome_at(&world, world.width() / 2, floor - 1),
            Biome::Forest
        );
        assert!(
            spawns_of(&world, floor, &quiet(), 482, 4) > 0,
            "no GraniteGolem (482) in a granite cavern"
        );
    }

    /// Christmas puts two zombies in seasonal knitwear (`NPC.cs:4739`,
    /// `Main.rand.Next(331, 333)`), and dresses the surface slime in ribbons
    /// (`NPC.cs:5660-5662`).
    ///
    /// Neutralised by forcing `Seasonal::xmas` to `false` in `try_spawn`: none of the four appears.
    #[test]
    fn christmas_puts_the_zombies_in_sweaters() {
        let (mut world, (px, py)) = night_world();
        world.xmas = true;
        let seen = spawns_at_in(&world, false, false, px, py, 200_000);
        let found: std::collections::BTreeSet<u16> = seen.iter().copied().collect();
        for npc_type in [331u16, 332] {
            assert!(
                found.contains(&npc_type),
                "no {npc_type} at Christmas: {found:?}"
            );
        }

        // The daytime slime wears ribbons too, and that swap is a different site
        // (`GetBasicSlimeToSpawn`, not the night chain). Two draws in three take a ribbon
        // (`GetBasicSlimeToSpawn_ChanceToBeHolidaySlime`, `NPC.cs:5678-5685`), so the plain blue
        // slime is still there and should be outnumbered rather than gone.
        let (mut day, _) = forest_surface();
        day.xmas = true;
        let seen = spawns_at_in(&day, false, false, px, py, 50_000);
        let ribboned = seen.iter().filter(|&&ty| (333..=336).contains(&ty)).count();
        let plain_slime = seen.iter().filter(|&&ty| ty == 1).count();
        assert!(ribboned > 0, "no ribboned slime at Christmas");
        assert!(
            ribboned > plain_slime,
            "{ribboned} ribboned slimes against {plain_slime} plain ones: the two-in-three roll is \
             not being made"
        );
        // All four colours, since `Main.rand.Next(333, 337)` is a flat draw over them.
        for colour in 333u16..=336 {
            assert!(seen.contains(&colour), "no slime in ribbon colour {colour}");
        }
    }

    /// The full moon in hardmode is the only thing that brings a Werewolf, and only two attempts in
    /// three at that (`NPC.cs:4633`: `!Main.dayTime && Main.moonPhase == 0 && Main.hardMode &&
    /// Main.rand.Next(3) != 0`).
    ///
    /// Neutralised by deleting the Werewolf arm from `seasonal_night_pick`: the first assertion
    /// fails. The second half of the test is the gate itself, which is what stops a Werewolf on
    /// every other night of the eight.
    #[test]
    fn a_full_moon_in_hardmode_brings_out_the_werewolves() {
        let (mut world, (px, py)) = night_world();
        world.moon_phase = 0; // MoonPhase.Full
        let seen = spawns_at_in(&world, true, false, px, py, 50_000);
        assert!(
            seen.contains(&104),
            "no Werewolf under a hardmode full moon: {:?}",
            seen.iter().collect::<std::collections::BTreeSet<_>>()
        );

        // Not on any other phase, and not before the wall falls.
        world.moon_phase = 1;
        assert!(
            !spawns_at_in(&world, true, false, px, py, 50_000).contains(&104),
            "a Werewolf on a waning moon"
        );
        world.moon_phase = 0;
        assert!(
            !spawns_at_in(&world, false, false, px, py, 50_000).contains(&104),
            "a Werewolf before hardmode"
        );
    }

    /// The five coloured eyes are the tail of the demon-eye branch, and the new moon
    /// (`MoonPhase.Empty`, phase 4) doubles how often that branch is entered at all
    /// (`NPC.cs:4554`: `Main.rand.Next(6) == 0 || (Main.moonPhase == 4 && Main.rand.Next(2) == 0)`).
    ///
    /// Neutralised by dropping the `|| (at.moon_phase == 4 && one_in(rng, 2))` half of that
    /// condition: the two counts below come out equal and the assertion fails. Neutralised the
    /// other way, by deleting the whole eye arm, the first assertion fails instead.
    #[test]
    fn the_new_moon_is_the_night_of_the_eyes() {
        let count = |phase: u8| {
            let at = Seasonal {
                moon_phase: phase,
                ..Seasonal::default()
            };
            let mut rng = SmallRng::seed_from_u64(99);
            (0..200_000)
                .filter(|_| {
                    let zombie = zombie_settings(true, 1, &mut rng);
                    matches!(seasonal_night_pick(at, zombie, 2, 0.0, &mut rng), Some(ty) if (190..=194).contains(&ty))
                })
                .count()
        };
        let full = count(0);
        let new_moon = count(4);
        assert!(full > 0, "no coloured eyes at all on an ordinary night");
        // Entering the branch goes from one attempt in six to one in six plus one in two of the
        // remaining five sixths, which is 7/12: a factor of 3.5, so 3x is a wide floor.
        assert!(
            new_moon > full * 3,
            "the new moon should be thick with eyes: {new_moon} against {full}"
        );
    }

    /// Ice underfoot on a surface night is the Ice Elemental's and the Wolf's only home outside the
    /// caverns' flying chain (`NPC.cs:4655-4673`), and the arm keys on the *tile* rather than on the
    /// player's zone: 169 was in no producer at all and 155 in none either.
    ///
    /// The floor is snow in a world the zone scan still calls a forest (169 snow tiles in a
    /// 169-by-124 box, against a `SnowTileNormalThreshold` of 1500), which is what makes this a
    /// test of the tile and not of the biome.
    ///
    /// Neutralised arm by arm:
    ///
    /// * deleting the ice-and-snow `matches!` block: "no IceElemental (169) on ice".
    /// * dropping `!at.graveyard` from both hardmode arms: "a Wolf in a snowy graveyard".
    /// * dropping `at.hard_mode` from both: "an Ice Elemental before hardmode".
    /// * passing `2` instead of `world.tile(x, y + 1).block` in `try_spawn`: "no IceElemental (169)
    ///   on ice", the arm written but wired to nothing.
    #[test]
    fn ice_underfoot_brings_the_wolves_out() {
        let (mut world, (px, py)) = night_world();
        for x in 0..world.width() {
            world.set_tile(x, 90, terrustia_proto::Tile::block(147)); // SnowBlock
        }
        assert_eq!(
            biome_at(&world, px, py),
            Biome::Forest,
            "one row of snow is not a snow biome, which is the point of this test"
        );

        let found: std::collections::BTreeSet<u16> =
            spawns_at_in(&world, true, false, px, py, 60_000)
                .into_iter()
                .collect();
        for (npc_type, name) in [(169u16, "IceElemental"), (155, "Wolf")] {
            assert!(
                found.contains(&npc_type),
                "no {name} ({npc_type}) on ice: {found:?}"
            );
        }

        // `!ZoneGraveyard` on both arms (`NPC.cs:4657`, `:4661`): tombstones in the snow get neither.
        let haunted: std::collections::BTreeSet<u16> =
            spawns_at_in(&world, true, true, px, py, 60_000)
                .into_iter()
                .collect();
        assert!(!haunted.contains(&155), "a Wolf in a snowy graveyard");
        assert!(
            !haunted.contains(&169),
            "an Ice Elemental in a snowy graveyard"
        );

        // ...and `Main.hardMode` on both, so a fresh world's snow is just cold.
        let early: std::collections::BTreeSet<u16> =
            spawns_at_in(&world, false, false, px, py, 60_000)
                .into_iter()
                .collect();
        assert!(!early.contains(&169), "an Ice Elemental before hardmode");
        assert!(!early.contains(&155), "a Wolf before hardmode");
    }

    /// Rain on a surface night is the Zombie Raincoat's only home (`NPC.cs:4675-4689`), and all
    /// three of vanilla's outcomes there are the same 223 at different scales.
    ///
    /// Neutralised by deleting the `at.raining` arm: "no ZombieRaincoat (223) in the rain".
    /// Neutralised the other way, by dropping `at.raining` from the condition so it fires dry: the
    /// second assertion fails instead.
    #[test]
    fn rain_puts_a_coat_on_the_zombies() {
        let (mut world, (px, py)) = night_world();
        world.raining = true;
        let wet: std::collections::BTreeSet<u16> =
            spawns_at_in(&world, false, false, px, py, 60_000)
                .into_iter()
                .collect();
        assert!(
            wet.contains(&223),
            "no ZombieRaincoat (223) in the rain: {wet:?}"
        );

        world.raining = false;
        let dry: std::collections::BTreeSet<u16> =
            spawns_at_in(&world, false, false, px, py, 60_000)
                .into_iter()
                .collect();
        assert!(!dry.contains(&223), "a raincoat on a dry night: {dry:?}");
    }

    /// A world whose surface line is at 200, so the sky line (`worldSurface * 0.35`) is row 70 and
    /// everything above it is open air the generator never touched.
    fn sky_world() -> World {
        let mut world = test_world();
        world.surface = 200;
        world.rock_layer = 300;
        world
    }

    /// A cavern of open air behind sandstone walls, floored every eight rows, which is what an
    /// underground desert is to the spawner: not a biome scan but a wall
    /// (`WallID.Sets.AllowsUndergroundDesertEnemiesToSpawn`).
    ///
    /// Rows 300 to 400 so every candidate is deeper than `worldSurface + 100` and shallower than
    /// the underworld, and wide enough to cover the whole `SPAWN_RANGE_X` box. `floor` is the block
    /// the ledges are made of, which is also what the zone scan around the player reads: sand makes
    /// it a desert, ebonsand makes it a corrupt one, and the walls stay sandstone either way.
    /// A synthetic underground desert cut into a real generated world.
    ///
    /// The box has to cover the whole biome scan window around `(400, 350)` (`BIOME_SCAN_X` 84,
    /// `BIOME_SCAN_Y_UP` 62, `BIOME_SCAN_Y_DOWN` 61, so rows 288 to 411), or the rows just outside
    /// it are whatever the generator happened to put there and the fixture's biome is not its own
    /// to control. It used to stop at rows 295..400, leaving 19 rows of real terrain inside the
    /// window: enough ebonstone to cross `EVIL_THRESHOLD` and make a "clean" desert read as
    /// corruption, which is exactly what happened the first time the carver changed how much of
    /// that terrain is solid.
    fn desert_cavern(wall: u16, floor: u16) -> World {
        use terrustia_proto::tile::Tile;
        let mut world = test_world();
        world.surface = 200;
        world.rock_layer = 300;
        for y in 280i32..420 {
            for x in 250..550 {
                let mut tile = if y % 8 == 0 {
                    Tile::block(floor)
                } else {
                    Tile::AIR
                };
                tile.wall = wall;
                world.set_tile(x, y, tile);
            }
        }
        world
    }

    /// The underground desert has a roster of its own, and before this it had no arm at all: eleven
    /// types (both ghoul families, the lamias, the scorpion, the beast, the djinn and both giant
    /// antlions) have no other ambient spawn anywhere in the game, so a player who dug into one met
    /// sand slimes and antlions and nothing else.
    ///
    /// Neutralised by forcing `desert_spot` to `false` in `try_spawn`: nothing but the ordinary
    /// pool comes out over forty thousand ticks and every assertion below fails.
    #[test]
    fn the_underground_desert_draws_its_own_roster() {
        let world = desert_cavern(187, 53); // Sandstone wall, sand ledges.
        let seen = spawns_at(&world, false, 400, 350, 40_000);
        let set: std::collections::BTreeSet<u16> = seen.iter().copied().collect();
        for (npc_type, name) in [
            (69u16, "Antlion"),
            (580, "WalkingAntlion"),
            (581, "FlyingAntlion"),
            (508, "GiantWalkingAntlion"),
            (509, "GiantFlyingAntlion"),
            (513, "TombCrawlerHead"),
            (537, "SandSlime"),
        ] {
            assert!(
                set.contains(&npc_type),
                "no {name} in the underground desert: {set:?}",
            );
        }
        // Nothing from the ordinary cavern pool: vanilla's branch answers for the whole spot.
        assert!(
            !set.contains(&21),
            "a Skeleton reached a desert spot: {set:?}"
        );
    }

    /// `SpawnTileOrAboveHasAnyWallInSet` reads two rows, not one (`NPC.cs:5535-5556`), and it is
    /// the *ground* tile plus the one above it, which for this server's spawn row `y` is `y + 1`
    /// and `y`. Half a sandstone wall is still an underground desert; no sandstone wall is not.
    ///
    /// Neutralised by dropping either `walled(y + 1)` or `walled(y)` from
    /// `underground_desert_spot`: one of the two middle assertions fails each time. The world the
    /// spawn test above uses has the wall on both rows, so it cannot see this on its own.
    #[test]
    fn the_desert_wall_is_read_on_the_ground_tile_and_the_one_above_it() {
        use terrustia_proto::tile::Tile;
        let mut world = desert_cavern(0, 53); // sand ledges, no wall anywhere.
        // Row 351 is open, row 352 is a ledge, so 351 is a spawn row.
        assert!(
            !underground_desert_spot(&world, 400, 351),
            "no wall, no desert"
        );

        let mut floor = Tile::block(53);
        floor.wall = 187;
        world.set_tile(400, 352, floor);
        assert!(
            underground_desert_spot(&world, 400, 351),
            "the ground tile alone"
        );

        let mut world = desert_cavern(0, 53);
        let mut above = Tile::AIR;
        above.wall = 187;
        world.set_tile(400, 351, above);
        assert!(
            underground_desert_spot(&world, 400, 351),
            "the tile above alone"
        );

        // ...and a wall that is not one of the nine does not count, however sandy the floor is.
        let mut world = desert_cavern(0, 53);
        let mut floor = Tile::block(53);
        floor.wall = 1; // StoneWall
        world.set_tile(400, 352, floor);
        assert!(
            !underground_desert_spot(&world, 400, 351),
            "a stone wall is not a desert"
        );
    }

    /// The giant antlions are *not* a hardmode upgrade. The one-in-ten roll that turns a Walking or
    /// Flying Antlion into its giant (`NPC.cs:1752-1763`) sits on the plain fallthrough path, which
    /// is the only path a pre-hardmode world ever takes, so a fresh world's underground desert
    /// already has them.
    ///
    /// Neutralised by moving the giant swap inside the `hard_mode` branch of
    /// `underground_desert_pick`: this fails, and the hardmode half below still passes, which is
    /// what makes the assertion about progression rather than about reachability.
    #[test]
    fn the_giant_antlions_do_not_wait_for_hardmode() {
        let world = desert_cavern(187, 53);
        let early: std::collections::BTreeSet<u16> = spawns_at(&world, false, 400, 350, 10_000)
            .into_iter()
            .collect();
        assert!(
            early.contains(&508),
            "no giant walking antlion pre-hardmode"
        );
        assert!(early.contains(&509), "no giant flying antlion pre-hardmode");
        // ...and none of hardmode's own roster is out early.
        for locked in [524u16, 528, 530, 532, 510] {
            assert!(
                !early.contains(&locked),
                "{locked} appeared before hardmode: {early:?}",
            );
        }
    }

    /// Hardmode opens the ghouls, and which ghoul is a function of the *player's* zone rather than
    /// of the tile under the spawn: `SetSpawnFlags` copies `ZoneCorrupt`/`ZoneCrimson`/`ZoneHallow`
    /// straight off the player (`NPC.cs:381-383`) and the branch reads those copies
    /// (`NPC.cs:1710-1741`). A clean desert gives the plain ghoul with the scorpion and the light
    /// lamia; a corrupt one gives the corrupt ghoul with the djinn and the dark lamia.
    ///
    /// Neutralised by collapsing `underground_desert_pick`'s `match biome` to its `_` arm: the
    /// corrupt half fails outright, since 525, 533 and 529 stop being reachable at all.
    #[test]
    fn the_ghoul_follows_the_players_zone_not_the_tile() {
        let clean = desert_cavern(187, 53);
        let seen: std::collections::BTreeSet<u16> = spawns_at(&clean, true, 400, 350, 10_000)
            .into_iter()
            .collect();
        for (npc_type, name) in [
            (524u16, "DesertGhoul"),
            (530, "DesertScorpionWalk"),
            (528, "DesertLamiaLight"),
            (532, "DesertBeast"),
            (510, "DuneSplicerHead"),
        ] {
            assert!(
                seen.contains(&npc_type),
                "no {name} in a clean desert: {seen:?}"
            );
        }
        assert!(!seen.contains(&525), "a corrupt ghoul in a clean desert");
        assert!(!seen.contains(&533), "a djinn in a clean desert");

        // The same cavern with ebonsand ledges: the walls are still sandstone, so it is still the
        // underground desert, but the zone around the player now reads as corruption.
        let corrupt = desert_cavern(187, 112);
        assert_eq!(biome_at(&corrupt, 400, 350), Biome::Corruption);
        let seen: std::collections::BTreeSet<u16> = spawns_at(&corrupt, true, 400, 350, 10_000)
            .into_iter()
            .collect();
        for (npc_type, name) in [
            (525u16, "DesertGhoulCorruption"),
            (533, "DesertDjinn"),
            (529, "DesertLamiaDark"),
        ] {
            assert!(
                seen.contains(&npc_type),
                "no {name} in a corrupt desert: {seen:?}"
            );
        }
        assert!(!seen.contains(&524), "the plain ghoul in a corrupt desert");
        assert!(!seen.contains(&530), "the scorpion in a corrupt desert");
    }

    /// A room of the Lihzahrd Temple: open air behind Lihzahrd brick walls, floored in Lihzahrd
    /// brick every eight rows.
    ///
    /// Both halves of vanilla's gate are here on purpose and are separable: `wall` is what the
    /// *player* stands in front of (`ZoneLihzhardTemple`, `SceneMetrics.cs:693`) and `floor` is the
    /// `tileType` under the candidate (`NPC.cs:3914`). Rows 295 to 400 put every candidate in the
    /// caverns, below `rock_layer`, which is where a real temple sits.
    fn temple(wall: u16, floor: u16) -> World {
        use terrustia_proto::tile::Tile;
        let mut world = test_world();
        world.surface = 200;
        world.rock_layer = 300;
        for y in 295i32..400 {
            for x in 250..550 {
                let mut tile = if y % 8 == 0 {
                    Tile::block(floor)
                } else {
                    Tile::AIR
                };
                tile.wall = wall;
                world.set_tile(x, y, tile);
            }
        }
        world
    }

    /// The temple's whole ambient roster, which had no arm at all before this: the Lihzahrd and the
    /// Flying Snake come from `NPC.cs:3914-3924` and from nowhere else in `NPC.Spawner`, so the
    /// post-Plantera temple - the room players farm for Solar Tablet fragments and Power Cells -
    /// was silent on this server.
    ///
    /// Both halves of the gate get their own assertion, because either one alone would have looked
    /// like a pass: a temple wall over ordinary stone is not the temple, and Lihzahrd brick with no
    /// temple wall behind the player is a Lihzahrd brick floor somebody built.
    ///
    /// Neutralised three ways, each rerun (an assertion failure stops the test, so each line names
    /// the one that fires first):
    /// * turning the whole `None if temple_zone` arm off: "no Lihzahrd (198) in the temple:
    ///   {42, 43, 51, 56, 354}", the ordinary cavern pool answering in its place.
    /// * dropping `temple_zone` from the arm's guard: "the temple roster reached a brick floor
    ///   outside a temple: {198, 226, 354}".
    /// * dropping the `ground_block` half: "the temple roster reached a stone floor:
    ///   {198, 226, 354}".
    #[test]
    fn the_lihzahrd_temple_has_a_roster_of_its_own() {
        let world = temple(TEMPLE_WALL, LIHZAHRD_BRICK);
        assert_eq!(depth_at(&world, 350), Depth::Cavern);
        let seen: std::collections::BTreeSet<u16> = spawns_at(&world, false, 400, 350, 20_000)
            .into_iter()
            .collect();
        for (npc_type, name) in [(LIHZAHRD, "Lihzahrd"), (FLYING_SNAKE, "FlyingSnake")] {
            assert!(
                seen.contains(&npc_type),
                "no {name} ({npc_type}) in the temple: {seen:?}"
            );
        }
        // The arm answers for the whole spot, as vanilla's does: nothing from the cavern pool.
        // A bound resident is the one other thing that can turn up, because that branch sits above
        // every biome arm in the game too (`NPC.cs:1658-1697`, `:2087-2098`, all of them above the
        // fallthrough the temple arm lives in), so it is subtracted rather than asserted away.
        //
        // ...and so is the Snail, for the same reason and with more surprise value: its arm
        // (`NPC.cs:3802-3805`) sits a hundred lines *above* the temple's in the same `else if`
        // ladder, and its own gate excludes the snow, both evils, the hallow and the dungeon but
        // not the jungle a temple reads as. So one temple draw in thirteen really is a snail in the
        // game, and this is the temple's roster with that arm honoured rather than without it.
        let from_higher_arms: std::collections::BTreeSet<u16> = crate::game::rescues::RESCUES
            .iter()
            .map(|r| r.bound)
            .chain([BOUND_TOWN_SLIME_OLD, SNAIL])
            .collect();
        let wild: std::collections::BTreeSet<u16> =
            seen.difference(&from_higher_arms).copied().collect();
        assert_eq!(
            wild,
            [LIHZAHRD, FLYING_SNAKE].into_iter().collect(),
            "something other than the temple's own two got in: {seen:?}"
        );

        // Wooden Spikes are the second ground tile the arm accepts (`NPC.cs:3914`).
        let spiked: std::collections::BTreeSet<u16> =
            spawns_at(&temple(TEMPLE_WALL, WOODEN_SPIKES), false, 400, 350, 20_000)
                .into_iter()
                .collect();
        assert!(
            spiked.contains(&LIHZAHRD) && spiked.contains(&FLYING_SNAKE),
            "the spike floor is a temple floor too: {spiked:?}"
        );

        // The zone is the player's wall: Lihzahrd brick with no temple wall behind them is just a
        // floor, and the caverns answer for it.
        let unwalled: std::collections::BTreeSet<u16> =
            spawns_at(&temple(0, LIHZAHRD_BRICK), false, 400, 350, 20_000)
                .into_iter()
                .collect();
        assert!(
            !unwalled.contains(&LIHZAHRD) && !unwalled.contains(&FLYING_SNAKE),
            "the temple roster reached a brick floor outside a temple: {unwalled:?}"
        );
        assert!(!unwalled.is_empty(), "the caverns answered for it instead");

        // ...and the ground is the candidate's: a temple wall over ordinary stone is not a temple.
        let stone_floor: std::collections::BTreeSet<u16> =
            spawns_at(&temple(TEMPLE_WALL, 1), false, 400, 350, 20_000)
                .into_iter()
                .collect();
        assert!(
            !stone_floor.contains(&LIHZAHRD) && !stone_floor.contains(&FLYING_SNAKE),
            "the temple roster reached a stone floor: {stone_floor:?}"
        );
    }

    /// A surface desert with a sandstorm blowing over it, deep enough sand for
    /// `Spawning_SandstoneCheck` to pass.
    ///
    /// The scan box has to read as desert for `ZoneSandstorm`, which wants 1500 sand tiles, so the
    /// whole box is sand with a ledge every eight rows carved out of it. Rows 120 to 200 keep every
    /// candidate at or above `worldSurface`, which is `SurfaceAtmospherics`.
    fn sandstorm_desert(sand: u16) -> World {
        use terrustia_proto::tile::Tile;
        let mut world = test_world();
        world.surface = 200;
        world.rock_layer = 300;
        for y in 100i32..260 {
            for x in 250..550 {
                // Nine solid rows of sand under every three open ones: the sandstone check reads
                // eight rows down from the ground tile, and a spawn needs three clear rows above
                // it, so both are satisfied everywhere.
                let tile = if y % 12 == 0 || (y % 12) > 3 {
                    Tile::block(sand)
                } else {
                    Tile::AIR
                };
                world.set_tile(x, y, tile);
            }
        }
        world
    }

    /// A sandstorm over a desert has a roster of its own (`NPC.cs:3952-4022`), and this server ran
    /// sandstorms without ever spawning one of its members: the Tumbleweed, the Sand Elemental, all
    /// four Sandsharks and the Blood Mummy had no ambient spawn at all.
    ///
    /// Neutralised by forcing `sandstorm_spot` to `false`: the pre-hardmode half loses its
    /// Tumbleweed and the hardmode half loses everything asserted below, so both halves fail.
    /// Neutralised a second way by setting `events.sandstorm` to `false` in this test, which fails
    /// it the same way and proves the arm really is gated on the weather rather than on the sand.
    #[test]
    fn a_sandstorm_draws_the_sandstorm_roster() {
        let world = sandstorm_desert(53); // plain Sand
        let storm = EventSpawns {
            sandstorm: true,
            ..quiet()
        };
        let early: std::collections::BTreeSet<u16> = spawns_with(&world, &storm, 400, 160, 30_000)
            .into_iter()
            .map(|(ty, _)| ty)
            .collect();
        assert!(
            early.contains(&546),
            "no tumbleweed in an early sandstorm: {early:?}"
        );
        assert!(
            early.contains(&61),
            "no vulture in an early sandstorm: {early:?}"
        );
        assert!(
            early.contains(&69),
            "no antlion in an early sandstorm: {early:?}"
        );
        assert!(
            !early.contains(&542),
            "a sandshark before hardmode: {early:?}",
        );

        let storm = EventSpawns {
            sandstorm: true,
            hard_mode: true,
            ..quiet()
        };
        let late = spawns_with(&world, &storm, 400, 160, 30_000);
        let set: std::collections::BTreeSet<u16> = late.iter().map(|(ty, _)| *ty).collect();
        for (npc_type, name) in [
            (541u16, "SandElemental"),
            (542, "SandShark"),
            (546, "Tumbleweed"),
            (78, "Mummy"),
            (580, "WalkingAntlion"),
            (581, "FlyingAntlion"),
        ] {
            assert!(
                set.contains(&npc_type),
                "no {name} in a hardmode sandstorm: {set:?}"
            );
        }
        // The Dune Splicer burrows in ten tiles below the spot it was chosen at
        // (`NPC.cs:3975`, `SpawnNPC(x, (spawnTileY + 10) * 16, 510)`), which is the one member that
        // does not stand where it was drawn. Every ledge in this world has its open rows at
        // `y % 12` of 1, 2 and 3, and a spawn needs three clear rows, so every other type lands on
        // a row `== 3 (mod 12)` and every splicer ten below that, `== 1 (mod 12)`.
        let rows = |want: u16| -> Vec<i32> {
            late.iter()
                .filter(|(ty, _)| *ty == want)
                .map(|(_, (_, y))| (*y / 16.0) as i32 % 12)
                .collect()
        };
        let splicers = rows(510);
        assert!(
            !splicers.is_empty(),
            "no dune splicer in a hardmode sandstorm"
        );
        assert!(
            splicers.iter().all(|row| *row == 1),
            "a dune splicer did not burrow ten tiles in: {splicers:?}",
        );
        let tumbleweeds = rows(546);
        assert!(
            tumbleweeds.iter().all(|row| *row == 3),
            "a tumbleweed was moved off its ledge: {tumbleweeds:?}",
        );
    }

    /// The sandshark's flavour is decided by the sand under the spawn, not by the player's zone -
    /// the opposite of how the ghouls above resolve their evil (`NPC.cs:3979-3991`, three
    /// `TileID.Sets` tests against `tileType`).
    ///
    /// Neutralised by returning a bare `542` from `sandstorm_pick`'s sandshark arm: all three
    /// converted sandsharks stop being reachable and this fails on the first of them.
    #[test]
    fn the_sandshark_takes_the_flavour_of_the_sand_it_swims_in() {
        let storm = EventSpawns {
            sandstorm: true,
            hard_mode: true,
            ..quiet()
        };
        for (sand, shark, name) in [
            (53u16, 542u16, "Sand/SandShark"),
            (112, 543, "Ebonsand/SandsharkCorrupt"),
            (234, 544, "Crimsand/SandsharkCrimson"),
            (116, 545, "Pearlsand/SandsharkHallow"),
        ] {
            let world = sandstorm_desert(sand);
            let set: std::collections::BTreeSet<u16> =
                spawns_with(&world, &storm, 400, 160, 30_000)
                    .into_iter()
                    .map(|(ty, _)| ty)
                    .collect();
            assert!(set.contains(&shark), "{name}: no {shark} drawn: {set:?}");
            for other in [542u16, 543, 544, 545] {
                assert!(
                    other == shark || !set.contains(&other),
                    "{name}: {other} drawn over the wrong sand: {set:?}",
                );
            }
        }
    }

    /// A Harpy can be drawn at sky height, which is the whole bug: `Depth` had no sky band and
    /// nothing walked the sky, so every candidate tile up there was thrown away by the descent to
    /// ground and NPC 48 appeared in no pool at all.
    ///
    /// Neutralised by forcing `sky` to `false` in `try_spawn`'s candidate loop: no Harpy is drawn
    /// in ten thousand ticks, and the assertion below fails.
    #[test]
    fn a_harpy_can_be_drawn_in_the_sky() {
        let world = sky_world();
        // Row 40 is well above the sky line; column 300 is outside the middle tenth of the map
        // that `NPC.cs:981` excludes before hardmode (800 * 0.45 = 360).
        let seen = spawns_at(&world, false, 300, 40, 10_000);
        assert!(
            seen.contains(&HARPY),
            "nothing found a harpy in the sky: {} spawns, {:?}",
            seen.len(),
            seen.iter().collect::<std::collections::BTreeSet<_>>(),
        );
    }

    /// ...and a Wyvern once the wall is down, which is the hardmode half of the same branch
    /// (`NPC.cs:1412`, one attempt in ten).
    ///
    /// Neutralised two ways, each failing this test: forcing `sky` to `false` as above, and
    /// dropping the `hard_mode` term from `sky_pick`'s Wyvern arm so it can never be reached.
    #[test]
    fn a_wyvern_can_be_drawn_in_the_sky_in_hardmode() {
        let world = sky_world();
        let seen = spawns_at(&world, true, 300, 40, 10_000);
        assert!(
            seen.contains(&WYVERN_HEAD),
            "hardmode sky produced no wyvern: {} spawns, {:?}",
            seen.len(),
            seen.iter().collect::<std::collections::BTreeSet<_>>(),
        );
        // The Harpy is still the sky's default, not something the Wyvern replaced.
        assert!(seen.contains(&HARPY), "hardmode sky produced no harpy");
    }

    /// The two height bands and the middle-of-the-map exclusion, read off the tile the way
    /// `FindSpawnTile` reads it (`NPC.cs:979-986`).
    ///
    /// Before hardmode the middle tenth of the map is not sky at all, which is what keeps harpies
    /// off a fresh world's spawn point; hardmode drops that exclusion and adds the second band,
    /// down to `worldSurface * 0.45`, at one attempt in ten.
    #[test]
    fn the_sky_starts_where_the_game_says_it_does() {
        let world = sky_world(); // surface 200, width 800: sky line 70, deep line 90.
        let mut rng = SmallRng::seed_from_u64(4);
        let outside = 300; // < 800 * 0.45
        let middle = 400; // inside 360..=440

        assert!(sky_tile(&world, outside, 69, false, &mut rng));
        assert!(!sky_tile(&world, outside, 70, false, &mut rng));
        assert!(
            !sky_tile(&world, middle, 40, false, &mut rng),
            "the middle tenth is not sky before hardmode",
        );
        assert!(
            sky_tile(&world, middle, 40, true, &mut rng),
            "hardmode drops the middle-of-the-map exclusion",
        );
        // The second band only exists in hardmode, and only one attempt in ten.
        assert!(!sky_tile(&world, outside, 80, false, &mut rng));
        assert!(
            (0..200).any(|_| sky_tile(&world, outside, 80, true, &mut rng)),
            "hardmode's second band never fired",
        );
        assert!(
            (0..200).any(|_| !sky_tile(&world, outside, 80, true, &mut rng)),
            "the second band is a roll, not a certainty",
        );
        assert!(
            !sky_tile(&world, outside, 90, true, &mut rng),
            "and it stops at worldSurface * 0.45",
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
        //
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
        // The underground desert has to be cleared out of the spawn box for a second, independent
        // reason: vanilla's desert arm sits *ahead* of `spawnFriendly` in the chain
        // (`NPC.cs:1682` against `NPC.cs:2099`), so a populated base within reach of a
        // sandstone-walled spot really does draw ghouls and antlions in the game too - the branch
        // comment at this server's own `desert_spot` arm says so. There is no column of an 800-wide
        // world that both avoids the evil and keeps every desert wall out of an 84-by-47 box (the
        // ocean's own wall is Sandstone, and it is in the set), so the fixture removes the walls
        // rather than hunting for a column without them. Nothing else here reads a wall except the
        // house-wall check, which only ever rejects candidates.
        let mut world = world;
        for cx in px - SPAWN_RANGE_X..=px + SPAWN_RANGE_X {
            for cy in py - SPAWN_RANGE_Y..=py + SPAWN_RANGE_Y + 1 {
                let mut tile = world.tile(cx, cy);
                if DESERT_SPAWN_WALLS.contains(&tile.wall) {
                    tile.wall = 0;
                    world.set_tile(cx, cy, tile);
                }
            }
        }
        let world = world;
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

    /// One live player standing on a tile, which is all `try_spawn` needs of a player list.
    fn player_standing_at(cx: i32, cy: i32) -> Vec<Option<Player>> {
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
        let players = player_standing_at(cx, cy);

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
                // A dungeon regular is the `num43` pool *or* one of the five the chain ahead of it
                // offers ([`dungeon_pick`], `NPC.cs:2723-2747`), which are residents just as much
                // as the Angry Bones are.
                let chain = [
                    DUNGEON_SLIME,
                    SPIKE_BALL,
                    BLAZING_WHEEL,
                    CURSED_SKULL,
                    DARK_CASTER,
                ];
                assert!(
                    pool(depth_at(&world, cy), Biome::Dungeon, world.day_time).contains(&npc_type)
                        || chain.contains(&npc_type),
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

    /// The dungeon's fallthrough is mostly the *big* Angry Bones, not the plain one: `NPC.cs:2767`
    /// rolls `Main.rand.Next(5)` and three of its five cases (`:2774-2782`) are 294, 295 and 296,
    /// with only the other two reaching 31 at `:2794`. None of the three is gated on `hardDungeon`,
    /// so they are what a player meets the first time they walk into a cleared dungeon.
    ///
    /// Neutralised by removing the three from `pool`'s dungeon arm: nothing but the four originals
    /// is drawn in forty thousand ticks and the first assertion fails.
    #[test]
    fn the_dungeon_is_mostly_big_angry_bones() {
        let (mut world, (cx, cy)) = dungeon_world();
        world.progress.downed_boss3 = true;
        let npcs = NpcStore::new();
        let players = player_standing_at(cx, cy);

        let mut rng = SmallRng::seed_from_u64(3);
        let mut found = std::collections::BTreeSet::new();
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
                found.insert(npc_type);
            }
        }
        for (npc_type, name) in [
            (294u16, "AngryBonesBig"),
            (295, "AngryBonesBigMuscle"),
            (296, "AngryBonesBigHelmet"),
        ] {
            assert!(
                found.contains(&npc_type),
                "no {name} ({npc_type}) in a cleared dungeon: {found:?}"
            );
        }
    }

    /// Lay a shelf of `TileID.Books` (50) across one row of a dungeon pocket.
    ///
    /// Books are frame-important (`tile_sets::frame_important`) and not solid
    /// (`tile_solid::solid(50)` is false), so a row of them neither blocks [`open_space`] three
    /// rows above the floor nor gives anything a place to stand: the only thing it changes is what
    /// [`find_nearby_book`] can see.
    fn shelve_books(world: &mut World, cx: i32, row: i32) {
        for xx in (cx - 110)..=(cx + 110) {
            world.set_tile(xx, row, terrustia_proto::Tile::framed(BOOK_TILE, 0, 0));
        }
    }

    /// The dungeon library, `NPC.cs:2748-2766`: one attempt in eight is a Water Bolt Mimic and one
    /// of the rest in ten a Librarian Skeleton, and both of them only where the box around the
    /// candidate holds a bookshelf, and both of them standing *on the shelf* rather than on the
    /// floor the attempt found.
    ///
    /// Fails before the fix on every count: 693 and 694 were in no pool and no branch, so neither
    /// the mimic's play-dead routine (`game::ai::skull`) nor the librarian's caster entry
    /// (`conjuring(693)`) could ever run in a real world.
    ///
    /// Neutralised three ways. Deleting the `if biome == Biome::Dungeon` library block from
    /// `try_spawn` fails the first assertion, "a dungeon library produced no Water Bolt Mimic".
    /// Dropping the `tile.block != BOOK_TILE` half of `find_nearby_book`'s tile test makes every
    /// solid tile a shelf and fails the bookless-dungeon assertion instead. Dropping the
    /// `SAFE_RANGE_X` clause from its screen test fails the shelf loop, "694 spawned on a shelf
    /// the player is looking at"; note that
    /// `spawns_appear_outside_the_safe_zone_and_on_solid_ground` does *not* catch that one, since
    /// its world has no bookshelf in it, so the assertion here is the only guard on it.
    #[test]
    fn the_dungeon_library_stands_its_pair_on_a_bookshelf() {
        let (mut world, (cx, cy)) = dungeon_world();
        world.progress.downed_boss3 = true;
        // Three rows above the walk level, which is inside the 32-tall box (`spawnTileY - 16`) and
        // clear of `open_space`'s own three rows.
        let shelf = cy - 3;
        let bookless = world.clone();
        shelve_books(&mut world, cx, shelf);
        assert_eq!(
            biome_at(&world, cx, cy),
            Biome::Dungeon,
            "a row of shelves must not stop the pocket reading as a dungeon"
        );

        let library = |world: &World, ticks: u32| {
            spawns_with(world, &quiet(), cx, cy, ticks)
                .into_iter()
                .filter(|(ty, _)| *ty == WATER_BOLT_MIMIC || *ty == LIBRARIAN_SKELETON)
                .collect::<Vec<_>>()
        };

        // One in eight and one in eleven of a dungeon's attempts, so this wants a long run: the
        // pre-Skeletron flat rate of 10 (`NPC.cs:787-790`) is gone once `downedBoss3` is set and
        // the dungeon is back on the ordinary rate. This one measures 657 mimics and 474
        // librarians, which is the 8-to-11 split the two `else if` rolls predict.
        const TICKS: u32 = 400_000;
        let found = library(&world, TICKS);
        assert!(
            found.iter().any(|(ty, _)| *ty == WATER_BOLT_MIMIC),
            "a dungeon library produced no Water Bolt Mimic"
        );
        assert!(
            found.iter().any(|(ty, _)| *ty == LIBRARIAN_SKELETON),
            "a dungeon library produced no Librarian Skeleton"
        );

        // Every one of them stands on the shelf row, not on the floor the candidate was found on
        // (`SpawnNPC(bookPosition.X * 16 + 8, bookPosition.Y * 16, ...)`), and outside the box the
        // player can see (`checkPlayerScreenRanges: true`).
        for (ty, (bx, by)) in &found {
            assert_eq!(
                (by / 16.0) as i32,
                shelf,
                "{ty} spawned off the shelf at ({bx}, {by})"
            );
            assert!(
                ((bx / 16.0) as i32 - cx).abs() >= SAFE_RANGE_X,
                "{ty} spawned on a shelf the player is looking at"
            );
        }

        // The same dungeon with nothing to read has neither of them: the shelf is the whole gate,
        // and `AI_FindNearbyBook` returning false is what makes the attempt fall through.
        assert!(
            library(&bookless, TICKS).is_empty(),
            "a dungeon with no bookshelf produced the library pair anyway"
        );

        // ...and so does a cavern lined with shelves, because the arm is inside `else if
        // (ZoneDungeon)` (`NPC.cs:2629`) and a bookcase in a cave is not a library.
        let floor = 300;
        let mut cave = hall_world(floor);
        assert_eq!(biome_at(&cave, 400, floor - 1), Biome::Forest);
        shelve_books(&mut cave, 400, floor - 4);
        assert!(
            spawns_with(&cave, &quiet(), 400, floor - 1, TICKS)
                .into_iter()
                .all(|(ty, _)| ty != WATER_BOLT_MIMIC && ty != LIBRARIAN_SKELETON),
            "a bookshelf outside the dungeon produced the library pair"
        );
    }

    /// `AI_FindNearbyBook` on its own (`NPC.cs:62954-63010`), asked the way the spawner asks it.
    #[test]
    fn the_nearest_book_is_nearest_the_corner_of_the_box() {
        let mut world = flat_world(300);
        // Far enough from the "player" that nothing here is inside the screen box.
        let player = (10_000, 10_000);
        let corner = (400, 200);
        let book = |world: &mut World, x: i32, y: i32| {
            world.set_tile(x, y, terrustia_proto::Tile::framed(BOOK_TILE, 0, 0));
        };

        // Nothing to find, and `tile.type != 50` means exactly that: a box full of ordinary stone
        // is a box with no shelf in it.
        assert_eq!(find_nearby_book(&world, corner, player), None);
        let mut stone = flat_world(300);
        stone.set_tile(425, 225, terrustia_proto::Tile::block(1));
        assert_eq!(find_nearby_book(&stone, corner, player), None);

        // One book anywhere in the 32-by-32 box is found; one outside it is not.
        book(&mut world, 425, 225);
        assert_eq!(find_nearby_book(&world, corner, player), Some((425, 225)));
        let mut outside = flat_world(300);
        book(&mut outside, 432, 225);
        assert_eq!(find_nearby_book(&outside, corner, player), None);

        // `vector2` is `searchPosition`, the box's top-left corner, so the winner is the book
        // nearest *that* and not the one nearest the middle of the box where the candidate stood.
        // (416, 216) is the middle; (405, 205) is far from it and much closer to the corner.
        book(&mut world, 405, 205);
        assert_eq!(find_nearby_book(&world, corner, player), Some((405, 205)));

        // A book sitting exactly on the corner reads as no book at all: `vector` never moves off
        // `vector2`, and the tail returns false (`:62998-63002`).
        let mut on_corner = flat_world(300);
        book(&mut on_corner, corner.0, corner.1);
        assert_eq!(find_nearby_book(&on_corner, corner, player), None);

        // `checkPlayerScreenRanges: true`: a shelf the player can see is skipped and the next one
        // out wins instead. The test is `|dx| < 62 && |dy| < 35`, so a player at (363, 205) has
        // (405, 205) inside the box (42 across) and (425, 225) exactly on its edge (62), and only
        // the first is hidden.
        let near = (425 - SAFE_RANGE_X, 205);
        assert_eq!(
            find_nearby_book(&world, corner, near),
            Some((425, 225)),
            "the close book should have been hidden by the player's own screen"
        );
    }

    /// Paint one wall id across the whole dungeon pocket, so every candidate spot in it reads the
    /// same brick style. The unsafe dungeon walls are deliberately not in `Main.wallHouse`
    /// (`Main.cs`, which never sets 7, 8, 9 or 94 to 99), so painting them does not make the pocket
    /// a house and does not disqualify it as a spawn spot.
    fn paint_dungeon_wall(world: &mut World, cx: i32, cy: i32, wall: u16) {
        for yy in (cy - 55)..=(cy + 55) {
            for xx in (cx - 110)..=(cx + 110) {
                let mut tile = world.tile(xx, yy);
                tile.wall = wall;
                world.set_tile(xx, yy, tile);
            }
        }
    }

    /// The hardmode dungeon is sorted by the *shape* of its masonry, not by its colour.
    ///
    /// `num40` (`NPC.cs:2631-2645`) reads 94, 96 and 98 (the blue, pink and green *slab* walls) as
    /// one group and 95, 97 and 99 (their *tile* walls) as another, with plain brick and everything
    /// else as a third (`WallID.cs:257-267`). Each group owns three quarters of the post-Plantera
    /// roster (`NPC.cs:2661-2722`): slab gives the Rusty armoured bones, the Ragged Casters and the
    /// Skeleton Sniper, tile the Hell armoured bones, the Diabolists and the Tactical Skeleton, and
    /// brick the Blue armoured bones, the Necromancers, the Skeleton Commando and the Paladin. Bone
    /// Lee is the one arm that ignores the wall.
    ///
    /// Neutralised twice. Removing the `hard_dungeon_pick` block from `try_spawn` leaves every one
    /// of the twenty-five types missing and the first assertion fails on 269. Making
    /// `dungeon_brick_style` answer a constant 0 makes the wall unreadable: the slab and tile phases
    /// find none of their own roster and fail the same way, while the brick phase still passes,
    /// which is exactly the shape of the bug this catches.
    #[test]
    fn the_hardmode_dungeon_is_sorted_by_its_masonry() {
        let (mut world, (cx, cy)) = dungeon_world();
        world.progress.downed_boss3 = true;
        let npcs = NpcStore::new();
        let players = player_standing_at(cx, cy);
        let events = EventSpawns {
            hard_mode: true,
            downed_plantera: true,
            ..quiet()
        };

        // The wall to paint, that style's own roster (its four armoured bones first), and one
        // armoured bones id belonging to a different style.
        for (wall, own, foreign) in [
            (
                94u16,
                [269u16, 270, 271, 272, 281, 282, 289, 291].as_slice(),
                273u16,
            ),
            (95, [277, 278, 279, 280, 285, 286, 289, 292].as_slice(), 273),
            (7, [273, 274, 275, 276, 283, 284, 290, 293].as_slice(), 269),
        ] {
            paint_dungeon_wall(&mut world, cx, cy, wall);
            let mut rng = SmallRng::seed_from_u64(11);
            let mut counts = std::collections::BTreeMap::<u16, u32>::new();
            // One cache for the phase rather than a fresh one per tick, which is what the server
            // itself holds: the player never moves, so the scan it takes on the first tick stays
            // good for all of them, and fifty thousand full biome scans become one.
            let mut cache = BiomeCache::default();
            for _ in 0..50_000 {
                for (npc_type, _) in try_spawn(
                    &world,
                    &npcs,
                    &players,
                    &events,
                    &JourneyPowers::default(),
                    &mut cache,
                    &mut rng,
                ) {
                    *counts.entry(npc_type).or_default() += 1;
                }
            }
            for want in own {
                assert!(
                    counts.contains_key(want),
                    "wall {wall}: no {want} among {counts:?}"
                );
            }
            assert!(
                counts.contains_key(&287),
                "wall {wall}: no Bone Lee, who belongs to every style"
            );
            // The one-in-seven reroll (`NPC.cs:2642-2645`) still lets the other two styles through,
            // so this is a skew and not an absence: a spot's own armoured bones should outnumber a
            // foreign style's by roughly nineteen to one.
            let mine: u32 = own[..4].iter().filter_map(|t| counts.get(t)).sum();
            let theirs: u32 = (foreign..foreign + 4).filter_map(|t| counts.get(&t)).sum();
            assert!(
                mine > theirs * 3,
                "wall {wall}: {mine} of its own armoured bones against {theirs} foreign"
            );
        }
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

        let fresh = zones_at(&world, x, y);
        assert_eq!(
            cache.read(&world, 0, x, y),
            Some(fresh),
            "a first read scans"
        );

        // Walking beyond the drift bound takes a new scan: the outer columns read as ocean by
        // position, which is a different answer from the forest just cached.
        assert_ne!(fresh.biome, Biome::Ocean);
        cache.advance(30);
        assert_eq!(cache.read(&world, 0, x + 8, y), Some(fresh), "still fresh");
        assert_eq!(
            cache.read(&world, 0, 10, y).map(|z| z.biome),
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
            aged.read(&world, 0, x, y).map(|z| z.biome),
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
            .filter(|s| cache.last(*s).map(|z| z.biome) == Some(Biome::Ocean))
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

    /// A world with a surface line at 100 and a rock layer at 200, for the water-chain tests: row
    /// 80 is above the surface line, 150 below it, and 250 below the rock layer.
    fn water_world() -> World {
        let mut world = World::empty(800, 600, "water chain");
        world.surface = 100;
        world.rock_layer = 200;
        world
    }

    /// Everything [`water_pick`] can answer with over 2000 seeds at one point in one world.
    fn drawn_from_water(
        world: &World,
        x: i32,
        ground_y: i32,
        biome: Biome,
        hard_mode: bool,
        ground_block: Option<u16>,
    ) -> std::collections::BTreeSet<Option<u16>> {
        (0..2_000u64)
            .map(|seed| {
                let mut rng = SmallRng::seed_from_u64(seed);
                water_pick(world, x, ground_y, biome, hard_mode, ground_block, &mut rng)
            })
            .collect()
    }

    /// Water draws the aquatic roster and dry land does not (`NPC.cs:1798`, `:1988`).
    ///
    /// Fails before the fix twice over: the ocean roster was in the *land* pool, so sharks appeared
    /// on dry sand, and `has_room` refused the water they should actually have come from.
    ///
    /// The Sea Snail (220) and the hardmode Orca (692) are the two arms of the ocean chain that
    /// were missing entirely: the old pool was a flat four-type slice drawn uniformly, so neither
    /// existed and the other four were mis-weighted besides.
    #[test]
    fn the_ocean_roster_comes_out_of_water_and_not_off_the_sand() {
        let sea = water_world();
        let ocean = |hard_mode| {
            drawn_from_water(&sea, 100, 80, Biome::Ocean, hard_mode, None)
                .into_iter()
                .flatten()
                .collect::<std::collections::BTreeSet<_>>()
        };
        // `NPC.cs:1859-1926`. The ocean arm is terminal: every path through it spawns, so `None`
        // never comes back out of it.
        assert_eq!(
            ocean(false),
            std::collections::BTreeSet::from([
                64,  // PinkJellyfish
                65,  // Shark
                67,  // Crab
                220, // SeaSnail
                221, // Squid
            ]),
        );
        // `NPC.cs:1863`: the orca is the one arm of the chain hardmode adds.
        let mut with_orca = ocean(false);
        with_orca.insert(692);
        assert_eq!(ocean(true), with_orca);

        for wet in ocean(true) {
            assert!(
                !pool(Depth::Surface, Biome::Ocean, true).contains(&wet)
                    && !pool(Depth::Surface, Biome::Ocean, false).contains(&wet)
                    && !hardmode_pool(Depth::Surface, Biome::Ocean, true).contains(&wet),
                "{wet} is aquatic and must not be drawable from dry ocean sand",
            );
        }

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
                ocean(false).contains(npc_type),
                "the sea drew {npc_type} ({}), which is not in its water roster",
                stats.name,
            );
        }
    }

    /// The three arms vanilla writes above every other water branch (`NPC.cs:1766-1777`): a
    /// hardmode jungle's water is an Arapaima's, and a hardmode crimson's is a Blood Jelly's and
    /// then a Blood Feeder's.
    ///
    /// All three used to sit in the *land* pools instead, which is worse than being missing: each
    /// has exactly one `SpawnNPC` site in the whole of `NPC.Spawner` and all three are water, so
    /// this server put swimmers on dry grass where their own AI has nothing to do but flop.
    #[test]
    fn the_hardmode_jungle_and_crimson_keep_their_swimmers_in_the_water() {
        let sea = water_world();
        // Two attempts in three, so both the arapaima and the fallthrough show up.
        let jungle = drawn_from_water(&sea, 400, 150, Biome::Jungle, true, None);
        assert!(
            jungle.contains(&Some(157)),
            "the hardmode jungle's arapaima"
        );
        assert!(
            !drawn_from_water(&sea, 400, 150, Biome::Jungle, false, None).contains(&Some(157)),
            "and never before hardmode",
        );

        let crimson = drawn_from_water(&sea, 400, 150, Biome::Crimson, true, None);
        assert!(crimson.contains(&Some(242)), "the crimson's blood jelly");
        assert!(crimson.contains(&Some(241)), "and its blood feeder");
        let calm = drawn_from_water(&sea, 400, 150, Biome::Crimson, false, None);
        assert!(!calm.contains(&Some(242)) && !calm.contains(&Some(241)));

        // None of the three is drawable from dry ground anywhere, at any depth, day or night.
        for depth in [
            Depth::Surface,
            Depth::Underground,
            Depth::Cavern,
            Depth::Underworld,
        ] {
            for biome in [Biome::Jungle, Biome::Crimson, Biome::Forest, Biome::Ocean] {
                for day in [true, false] {
                    for &aquatic in &[157u16, 241, 242] {
                        assert!(
                            !pool(depth, biome, day).contains(&aquatic)
                                && !hardmode_pool(depth, biome, day).contains(&aquatic),
                            "{aquatic} is a water spawn and must not be in a land pool \
                             ({depth:?}/{biome:?}/day={day})",
                        );
                    }
                }
            }
        }
    }

    /// The Piranha arm (`NPC.cs:1932-1986`) and the jellyfish arm (`:1988-1997`), each of which
    /// swaps its type for a hardmode one and neither of which existed here.
    ///
    /// The Piranha arm's gate is the interesting half: half of every attempt below the rock layer,
    /// *or* any attempt at all standing on jungle grass whatever the depth, which is what puts
    /// piranhas in a surface jungle pool.
    #[test]
    fn the_piranha_and_jellyfish_arms_swap_their_type_for_a_hardmode_one() {
        let sea = water_world();
        // Below the rock layer: the piranha arm, half the time.
        let deep = drawn_from_water(&sea, 400, 250, Biome::Forest, false, None);
        assert!(deep.contains(&Some(58)), "the piranha");
        let deep_hard = drawn_from_water(&sea, 400, 250, Biome::Forest, true, None);
        assert!(deep_hard.contains(&Some(102)), "and the angler fish after");

        // Jungle grass at the *surface*, where the rock-layer half cannot fire at all.
        let pond = drawn_from_water(&sea, 400, 80, Biome::Jungle, false, Some(JUNGLE_GRASS));
        assert_eq!(
            pond,
            std::collections::BTreeSet::from([Some(58)]),
            "jungle grass takes the whole arm, with no roll of its own",
        );

        // Between the surface line and the rock layer: no piranha arm, so the jellyfish answers.
        let mid = drawn_from_water(&sea, 400, 150, Biome::Forest, false, None);
        assert!(mid.contains(&Some(63)), "the blue jellyfish");
        assert!(
            !mid.contains(&Some(58)),
            "and no piranha above the rock layer"
        );
        let mid_hard = drawn_from_water(&sea, 400, 150, Biome::Forest, true, None);
        assert!(mid_hard.contains(&Some(103)), "the green jellyfish");

        // Above the surface line the jellyfish arm cannot fire either.
        let shallow = drawn_from_water(&sea, 400, 80, Biome::Forest, true, None);
        assert!(!shallow.contains(&Some(63)) && !shallow.contains(&Some(103)));
    }

    /// Both evils keep their own goldfish in the goldfish arm (`NPC.cs:1999-2008`), and the arm is
    /// one attempt in four inside its bounds rather than a roster of the place.
    #[test]
    fn each_evil_keeps_its_own_goldfish_in_the_water() {
        let sea = water_world();
        // Inland, above the surface line: only the goldfish arm can answer here.
        let corrupt = drawn_from_water(&sea, 400, 80, Biome::Corruption, false, None);
        assert!(corrupt.contains(&Some(57)), "the corrupt goldfish");
        let crimson = drawn_from_water(&sea, 400, 80, Biome::Crimson, false, None);
        assert!(crimson.contains(&Some(465)), "the crimson goldfish");
        assert!(
            drawn_from_water(&sea, 400, 80, Biome::Forest, false, None).contains(&Some(55)),
            "and a plain one everywhere else",
        );

        // `NPC.cs:1999`: within `oceanDistance` of the edge the arm needs depth instead, so a
        // shallow pool out on the beach draws nothing from the water chain at all.
        assert_eq!(
            drawn_from_water(&sea, 100, 80, Biome::Corruption, false, None),
            std::collections::BTreeSet::from([None]),
        );
    }

    /// The whole point of `None`: vanilla's water branches are an `else if` ladder, so an attempt
    /// every arm declines carries on into the land chain instead of spawning a jellyfish anyway.
    ///
    /// The old pool answered a Blue Jellyfish on *every* attempt below the surface line, missing
    /// the `Main.rand.Next(3) == 0` at `NPC.cs:1988`, which is why nothing wet ever reached a land
    /// arm and the Fungo Fish below was unreachable.
    #[test]
    fn an_attempt_every_water_arm_declines_falls_through_to_the_land_chain() {
        let sea = water_world();
        let mid = drawn_from_water(&sea, 400, 150, Biome::Forest, false, None);
        assert!(
            mid.contains(&None),
            "two attempts in three past the jellyfish arm reach land: {mid:?}",
        );
        assert!(mid.contains(&Some(63)), "and the third is a jellyfish");
    }

    /// The Fungo Fish (`NPC.cs:3633-3636`), which needed the water chain to decline before it
    /// could ever be reached, and then takes the whole attempt when it holds.
    #[test]
    fn a_hardmode_mushroom_cave_under_water_is_nothing_but_fungo_fish() {
        let mut rng = SmallRng::seed_from_u64(1);
        for surface in [false, true] {
            for _ in 0..200 {
                assert_eq!(mushroom_pick(surface, true, true, 0.0, &mut rng), Some(256));
            }
        }
        // Before hardmode the arm cannot fire, and dry mushroom grass is the ordinary roster.
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..2_000u64 {
            let mut rng = SmallRng::seed_from_u64(seed);
            seen.extend(mushroom_pick(true, false, true, 0.0, &mut rng));
        }
        assert!(
            !seen.contains(&256),
            "no fungo fish before hardmode: {seen:?}"
        );
    }

    /// A flat world with a floor to stand on, for the tower-zone tests below.
    fn flat_world(floor: i32) -> World {
        flat_world_of(floor, 1)
    }

    /// The same, with the floor made of something in particular: the two zone flags this module
    /// classifies and every mushroom arm are decided by what the ground is made of.
    fn flat_world_of(floor: i32, block: u16) -> World {
        flat_world_sized(800, floor, block)
    }

    /// ...and the same again with the width under the test's control, which only [`forest_surface`]
    /// needs and only because of how wide the beach is.
    fn flat_world_sized(width: i32, floor: i32, block: u16) -> World {
        let mut world = World::empty(width, 600, "pillar arena");
        world.surface = 100;
        world.rock_layer = 200;
        for x in 0..world.width() {
            for y in floor..(floor + 4) {
                world.set_tile(x, y, terrustia_proto::Tile::block(block));
            }
        }
        world
    }

    /// Everything a pillar standing `offset` pixels from the player draws over `ticks` attempts.
    fn tower_spawns(
        world: &World,
        pillar: u16,
        at: (i32, i32),
        offset: f32,
        ticks: u32,
    ) -> Vec<u16> {
        let mut towers = [None; 4];
        let slot = crate::game::lunar::PILLARS
            .iter()
            .position(|p| *p == pillar)
            .expect("a real pillar");
        towers[slot] = Some((at.0 as f32 * 16.0 + offset, at.1 as f32 * 16.0));

        let events = EventSpawns {
            hard_mode: true,
            towers,
            ..quiet()
        };
        let npcs = NpcStore::new();
        let players = player_at((at.0 as f32 * 16.0, at.1 as f32 * 16.0));
        let mut rng = SmallRng::seed_from_u64(0x10_0000 + u64::from(pillar));
        let mut biomes = BiomeCache::default();
        let mut seen = Vec::new();
        for _ in 0..ticks {
            seen.extend(
                try_spawn(
                    world,
                    &npcs,
                    &players,
                    &events,
                    &JourneyPowers::default(),
                    &mut biomes,
                    &mut rng,
                )
                .into_iter()
                .map(|(npc_type, _)| npc_type),
            );
        }
        seen
    }

    /// Inside a pillar's zone, that pillar's escort is the only thing the world produces, and
    /// outside it none of the escort appears at all.
    ///
    /// This is the whole Lunar Apocalypse. A pillar's shield is a count of its own escort killed
    /// (`game/lunar.rs`) and a pillar takes no damage while the shield holds, so with nothing
    /// spawning the four towers were indestructible and the Moon Lord unreachable.
    ///
    /// Neutralised by deleting the `if let Some(pillar) = tower` arm from `try_spawn`'s candidate
    /// loop: every zone then draws the surface forest roster instead and the first assertion in
    /// each arm fails on a Zombie.
    #[test]
    fn a_pillar_zone_spawns_its_own_escort_and_nothing_else() {
        use crate::game::lunar;

        let floor = 150;
        let world = flat_world(floor);
        let at = (400, floor - 2);

        // The four rosters, `NPC.cs:1302`, `:1328`, `:1349`, `:1364`, as sets.
        let rosters: [(u16, &[u16]); 4] = [
            (lunar::NEBULA, &[420, 421, 423, 424]),
            (lunar::VORTEX, &[425, 426, 427, 429]),
            (lunar::STARDUST, &[402, 405, 407, 409, 411]),
            (lunar::SOLAR, &[412, 415, 416, 417, 418, 419, 518]),
        ];

        for (pillar, roster) in rosters {
            let seen = tower_spawns(&world, pillar, at, 0.0, 4_000);
            assert!(
                !seen.is_empty(),
                "pillar {pillar} spawned nothing at all in four thousand ticks",
            );
            for npc_type in &seen {
                assert!(
                    roster.contains(npc_type),
                    "pillar {pillar}'s zone drew {npc_type}, which is not on its list",
                );
            }
            // ...and every uncapped entry really is reachable, so a typo in one weight cannot
            // quietly drop a type out of the fight.
            let drawn: std::collections::BTreeSet<u16> = seen.into_iter().collect();
            for npc_type in roster {
                assert!(
                    drawn.contains(npc_type),
                    "pillar {pillar}'s zone never drew {npc_type} in four thousand ticks",
                );
            }
        }

        // `SceneMetrics.NPCEventZoneRadius` is 4000 px (`SceneMetrics.cs:130`), so a pillar half a
        // world away is not a zone: the ordinary surface roster answers instead.
        let far = tower_spawns(&world, lunar::SOLAR, at, 5_000.0, 4_000);
        assert!(!far.is_empty(), "the ordinary world stopped spawning too");
        for npc_type in &far {
            assert!(
                lunar::belongs_to(*npc_type).is_none(),
                "{npc_type} spawned five thousand pixels from its pillar",
            );
        }
    }

    /// What the tower-zone test costs on the spawn path, which runs once per player per tick.
    ///
    /// It is deliberately not a scan: a real client works its zone out in `SceneMetrics` by walking
    /// every NPC (`SceneMetrics.cs:734-751`), and doing that here would be another pass over the
    /// store per player per tick on top of the biome scan that already had to be cached to fit.
    /// Instead the caller gathers the four positions once, only while the event is up, and this is
    /// four distance comparisons against a stack array. On an M-series laptop, with the arguments
    /// behind a `black_box` so the loop cannot be hoisted (which costs more than the test itself
    /// does): 1.0 ns with no apocalypse running, which is almost always, 1.4 ns with all four
    /// standing and no zone matched, 1.0 ns standing inside one. At the 255-player bar the worst
    /// case is 0.4 us of a 16.67 ms tick, which is 0.002% of it.
    /// What the graveyard and the seasonal chain cost the tick, which is the thing to be careful
    /// about here: vanilla works `ZoneGraveyard` out by counting tombstones in a 169-by-124 tile box
    /// (`SceneMetrics.cs:16`, `:356`, `:622`), and doing *that* per player per tick would be about
    /// twenty-one thousand tile reads a head, which does not fit. It is not done. The client already
    /// counts them and sends the answer up in packet 36, exactly as it does for every other zone, so
    /// the server's whole graveyard test is one byte and one mask.
    ///
    /// Measured on an M-series laptop, arguments behind a `black_box` so nothing is hoisted:
    ///
    /// * `Player::in_graveyard`: 0.4 ns with no zone packet yet, 0.6 ns with one. That is one
    ///   `Option` test, one bounds-checked byte and a mask. It runs once per player per attempt, so
    ///   at the 255-player bar it is 0.15 us of a 16.67 ms tick: 0.001% of it. A tile scan for the
    ///   same answer would be twenty-one thousand tile reads a head and could not be afforded.
    /// * `seasonal_night_pick` on an ordinary night, the case that runs almost always: 4.2 ns. It
    ///   is reached at most once per tick server-wide, well after the rate roll has let an attempt
    ///   through and `try_spawn` has settled on a tile, so it is nanoseconds per *tick*, not per
    ///   player.
    /// * The same in a graveyard at Halloween with every arm live: 15.3 ns, which is the chain
    ///   actually rolling its dice rather than falling out of the first condition.
    ///
    /// The snow and rain arms (`NPC.cs:4655`, `:4675`) were added to that chain later and were
    /// measured against the same build on the same machine rather than against the numbers above:
    /// 4.89 ns before and 5.50 ns after on the ordinary night, and 17.95 against 16.68 in the
    /// graveyard, so the two arms and the two rolls above them cost about 0.6 ns of a run that
    /// happens at most once per *tick*. Written as an array of the six ice and snow tiles and
    /// `contains` instead, the same measurement was 8.68 ns, which is why it is a `matches!`.
    ///
    /// The night's own tail (`NPC.cs:4722` and `:4744-4816`) was added later again, together with
    /// the [`zombie_settings`] draw the loop below now makes before each call, and measured the same
    /// way against the same build on the same machine: 6.83 ns before and 14.90 ns after on the
    /// ordinary night, 22.71 against 31.43 in the graveyard. Most of that is not overhead but work
    /// that was not being done: an ordinary night used to fall off the end of the chain with a
    /// `None` and now runs the torch roll, the armed roll and the closing `match`, which is what
    /// actually answers a plain forest night. It remains a per-*tick* cost, not a per-player one.
    #[test]
    #[ignore]
    fn measure_the_graveyard_and_the_seasonal_chain() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let mut bare = Player::new(0, "127.0.0.1:1".parse().unwrap(), tx.clone());
        let mut haunted = Player::new(0, "127.0.0.1:1".parse().unwrap(), tx);
        haunted.zone = Some(bytes::Bytes::from_static(&[0, 0, 0, 0, 1 << 6, 0, 0]));

        for (name, player) in [
            ("no zone packet yet", &mut bare),
            ("graveyard", &mut haunted),
        ] {
            let n = 10_000_000;
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..n {
                sink += u32::from(std::hint::black_box(&*player).in_graveyard());
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("in_graveyard, {name}: {each:.2} ns/call (sink {sink})");
        }

        for (name, at) in [
            ("ordinary night", Seasonal::default()),
            (
                "graveyard at Halloween",
                Seasonal {
                    halloween: true,
                    graveyard: true,
                    hard_mode: true,
                    blood_moon: true,
                    moon_phase: 4,
                    ..Seasonal::default()
                },
            ),
        ] {
            let n = 10_000_000;
            let mut rng = SmallRng::seed_from_u64(1);
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..n {
                let zombie = zombie_settings(true, 1, &mut rng);
                sink += u32::from(
                    seasonal_night_pick(
                        std::hint::black_box(at),
                        std::hint::black_box(zombie),
                        std::hint::black_box(2),
                        0.0,
                        &mut rng,
                    )
                    .unwrap_or(0),
                );
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("seasonal_night_pick, {name}: {each:.2} ns/call (sink {sink})");
        }
    }

    /// What the bound town slimes cost the two chains they were added to.
    ///
    /// Neither adds a scan to the common path. `sky_pick` gains one bool read and, only when that
    /// bool is false, one `rand` call; `spawn_frog` is a whole new function but sits behind a `u16`
    /// compare on a draw that only ever happens in a jungle, and its `alive` closure (the one thing
    /// here that walks the NPC table) is the last term of a `&&` chain, so it runs on roughly one
    /// frog draw in thirty and never at all once the slime is freed.
    ///
    /// Measured on an M-series laptop, arguments behind a `black_box`:
    ///
    /// * `sky_pick`, fresh world: 2.49 ns. Freed (the arm short-circuits on the flag): 0.71 ns.
    /// * `spawn_frog`, fresh world: 2.44 ns. Freed: 0.72 ns, the flag read and nothing else.
    ///
    /// The freed numbers are the steady state of any world past its rescues, and they are the flag
    /// read alone: the arm costs nothing at all once the slime has moved in.
    ///
    /// Both are reached at most once per tick server-wide, after the rate roll has let an attempt
    /// through and a tile has been settled on, so this is nanoseconds per *tick*, not per player.
    #[test]
    #[ignore]
    fn measure_the_town_slime_arms() {
        let mut fresh = World::empty(800, 600, "bench");
        fresh.progress.downed_golem = true;
        let mut freed = fresh.clone();
        freed.progress.unlocked_slime_purple = true;
        freed.progress.unlocked_slime_yellow = true;
        let never = |_: u16| false;

        for (name, world) in [("fresh", &fresh), ("freed", &freed)] {
            let n = 10_000_000;
            let mut rng = SmallRng::seed_from_u64(1);
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..n {
                sink += u32::from(sky_pick(
                    false,
                    false,
                    std::hint::black_box(world),
                    false,
                    &never,
                    0.0,
                    &mut rng,
                ));
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("sky_pick, {name}: {each:.2} ns/call (sink {sink})");

            let mut rng = SmallRng::seed_from_u64(1);
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..n {
                sink += u32::from(spawn_frog(
                    std::hint::black_box(world),
                    &never,
                    0.0,
                    &mut rng,
                ));
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("spawn_frog, {name}: {each:.2} ns/call (sink {sink})");
        }
    }

    /// What the Statue Mimic's plinth check costs, since it is the one new per-candidate thing in
    /// [`try_spawn`] that reads tiles rather than bools.
    ///
    /// It is behind `downedBoss3 && ZoneGraveyard && !noWorms` and then a one-in-twenty-five roll,
    /// so an ordinary world never reaches it at all and a graveyard reaches it once in twenty-five
    /// candidates. Measured on this machine, release, with the column walked so no read is served
    /// from the last call's line: 23.14 ns on a plinth (all eight reads taken) and 4.52 ns on a
    /// floor of half bricks, where the first `solid_tile2` shuts it.
    ///
    /// The number that matters is not that one but how often anything reaches it. Both this lane's
    /// arms sit *inside* the candidate loop, which `try_spawn` enters only after the rate roll lets
    /// an attempt through: over 2,000,000 calls on an ordinary forest day the loop resolved 5,223
    /// times, one call in 383. So the whole addition is amortised over that, and it is below what
    /// wall-clock timing can resolve here: `try_spawn` measured 242-527 ns a call with these arms
    /// and 213-290 ns with both cut out, four runs each, the two ranges overlapping and neither
    /// reproducible to better than a third of itself while other lanes share the machine. The
    /// arithmetic is the answer where the clock is not.
    #[test]
    #[ignore]
    fn measure_the_statue_mimic_plinth_check() {
        let floor = 90;
        let plinth = flat_world(floor);
        let half_bricks = {
            let mut world = plinth.clone();
            for x in 0..world.width() {
                let mut tile = terrustia_proto::Tile::block(1);
                tile.flags.set(terrustia_proto::TileFlags::HALF_BRICK, true);
                world.set_tile(x, floor, tile);
            }
            world
        };

        for (name, world) in [("plinth", &plinth), ("half bricks", &half_bricks)] {
            let n = 10_000_000;
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for i in 0..n {
                sink += u32::from(good_place_for_a_statue_mimic(
                    std::hint::black_box(world),
                    200 + (i % 128),
                    floor,
                ));
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("good_place_for_a_statue_mimic, {name}: {each:.2} ns/call (sink {sink})");
        }
    }

    /// What the Rock Golem's gate costs, since it is this lane's one new per-candidate thing in
    /// [`try_spawn`] that does more than look at a pool.
    ///
    /// Its ground tile is [`try_spawn`]'s own `ground_block`, already read for the sandstorm, the
    /// mushroom arms and the Goblin Scout, so the branch adds no tile read of its own before the
    /// roll. The shape is cheapest-first, and each rung of it is measured here: a non-stone floor
    /// (dirt) leaves after three comparisons, plain stone spends a `SmallRng` draw and leaves
    /// forty-nine times in fifty, and only the fiftieth pays for three [`solid_tile`] reads.
    ///
    /// It is measured against [`good_place_for_a_statue_mimic`] in the same run, that being the
    /// nearest thing already in this loop and already accepted as cheap enough for it. Both
    /// numbers below are the `dev` profile (which this workspace has optimised, with debuginfo)
    /// rather than `--release`: the machine had no disk left for a release build with three other
    /// lanes sharing it, and the ratio is the point either way.
    ///
    /// The ground column is walked so no read is served from the last call's cache line. Measured:
    /// 5.32 ns on dirt, 6.56 ns on stone under open sky, 6.54 ns on stone under a ceiling, against
    /// 99.41 ns for the plinth check in the same run. The two stone figures agree because the
    /// ceiling reads only happen on the one attempt in fifty that gets past the roll (the sink
    /// counts 200,037 of 10,000,000, which is that fiftieth).
    ///
    /// So the branch costs about a fifteenth of a check this loop already carries, and only when
    /// `depth == Depth::Cavern`, on a `ground_block` [`try_spawn`] had already read.
    #[test]
    #[ignore]
    fn measure_the_rock_golem_gate() {
        let floor = 300;
        let stone = flat_world_of(floor, STONE);
        let dirt = flat_world_of(floor, 0);
        let roofed = {
            let mut world = stone.clone();
            for x in 0..world.width() {
                world.set_tile(
                    x,
                    floor - ROCK_GOLEM_HEADROOM,
                    terrustia_proto::Tile::block(1),
                );
            }
            world
        };

        for (name, world, block) in [
            ("dirt", &dirt, 0u16),
            ("stone, open", &stone, STONE),
            ("stone, roofed", &roofed, STONE),
        ] {
            let n = 10_000_000;
            let mut rng = SmallRng::seed_from_u64(631);
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for i in 0..n {
                sink += u32::from(check_to_spawn_rock_golem(
                    std::hint::black_box(world),
                    200 + (i % 128),
                    floor,
                    block,
                    true,
                    false,
                    &mut rng,
                ));
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("check_to_spawn_rock_golem, {name}: {each:.2} ns/call (sink {sink})");
        }

        // The reference, in the same run and the same build: an eight-tile-read check that is
        // already in this loop and already accepted as cheap enough for it.
        let plinth = flat_world(90);
        let n = 10_000_000;
        let start = std::time::Instant::now();
        let mut sink = 0u32;
        for i in 0..n {
            sink += u32::from(good_place_for_a_statue_mimic(
                std::hint::black_box(&plinth),
                200 + (i % 128),
                90,
            ));
        }
        let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
        println!("good_place_for_a_statue_mimic, plinth: {each:.2} ns/call (sink {sink})");
    }

    /// What the three new weather and dungeon arms cost the per-candidate loop.
    ///
    /// Two of the three add nothing measurable: the rain arms are a bool and a roll, and the windy
    /// pair's two wall reads sit behind `events.happy_windy_day`, which is false on most days and
    /// every night. The one that is paid on every attempt is the dungeon's, where
    /// `dungeon_brick_style` moved out from under `hard_mode && downed_plantera` so the ordinary
    /// chain can read it too, exactly as vanilla reads `num40` once for the whole `ZoneDungeon`
    /// block (`NPC.cs:2631-2645`). That is two wall reads and a roll a pre-Plantera dungeon did not
    /// used to make.
    ///
    /// Measured in release, ten million calls each: `spawn_wall_type` 9.3 ns,
    /// `dungeon_brick_style` 17.7 ns, `dungeon_pick` 5.3 ns on plain brick and 6.8 ns on slab,
    /// against 20.2 ns for the plinth check this loop already carries and accepts as cheap. The
    /// reference reproduces at 20.7 ns in `measure_the_rock_golem_gate`'s own run in the same
    /// build, which is what says the boxed closure this bench calls through is not what is being
    /// measured.
    ///
    /// So a dungeon attempt pays about 23 ns it did not, or a little over one plinth check, on a
    /// path that has already walked a ground column to get there. Some of that is not new at all:
    /// `dungeon_brick_style` was already run on every post-Plantera hardmode attempt, and
    /// `dungeon_pick` takes work off the weighted draw below it, whose dungeon pool went from seven
    /// entries to four.
    ///
    /// `near_spike_ball` is left out of the numbers because it is not a fixed cost: it walks the
    /// live NPC table, and it runs on one slab attempt in three after the Dungeon Slime's roll
    /// declines, so roughly a ninth of dungeon attempts on a third of dungeon walls.
    #[test]
    #[ignore]
    fn measure_the_weather_and_dungeon_arms() {
        const N: u32 = 10_000_000;
        let bench = |name: &str, mut body: Box<dyn FnMut(i32) -> u32 + '_>| {
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for i in 0..N {
                sink = sink.wrapping_add(body(200 + (i as i32 % 128)));
            }
            let each = start.elapsed().as_secs_f64() / f64::from(N) * 1e9;
            println!("{name}: {each:.2} ns/call (sink {sink})");
        };

        let surface = flat_world(90);
        bench(
            "spawn_wall_type",
            Box::new(|x| u32::from(spawn_wall_type(std::hint::black_box(&surface), x, 88))),
        );

        let (dungeon, (_, cy)) = dungeon_world();
        let mut rng = SmallRng::seed_from_u64(70);
        bench(
            "dungeon_brick_style",
            Box::new(|x| {
                u32::from(dungeon_brick_style(
                    std::hint::black_box(&dungeon),
                    x,
                    cy,
                    &mut rng,
                ))
            }),
        );
        for style in [0u8, 1] {
            let mut rng = SmallRng::seed_from_u64(72);
            bench(
                &format!("dungeon_pick, style {style}"),
                Box::new(|_| {
                    u32::from(
                        dungeon_pick(std::hint::black_box(style), &|| false, 0.0, &mut rng)
                            .unwrap_or_default(),
                    )
                }),
            );
        }

        // The reference, in the same run and the same build.
        let plinth = flat_world(90);
        bench(
            "good_place_for_a_statue_mimic",
            Box::new(|x| {
                u32::from(good_place_for_a_statue_mimic(
                    std::hint::black_box(&plinth),
                    x,
                    90,
                ))
            }),
        );
    }

    /// What the Palworld arm costs the per-candidate loop.
    ///
    /// The part paid on every surface-day attempt is four scalar tests: a bool already computed
    /// (`surface_day`), another already computed (`wet`), one subtraction and compare for the
    /// distance gate, and a four-way `matches!` on a tile id that was read once further up. There
    /// is nothing to measure there and the two calls below are what the arm actually adds.
    ///
    /// Both sit behind `Main.rand.Next(160) == 0`, which is vanilla's own ordering
    /// (`NPC.cs:4374-4375`: the roll comes before `AnyNPCs` and before `HasItem`), so an attempt
    /// pays them once in a hundred and sixty and only after every clause above them has passed.
    ///
    /// Measured in release, ten million calls each, against the plinth check the loop already
    /// carries and accepts as cheap:
    ///
    /// * `pal_type`, empty inventory: 6.99 ns
    /// * `pal_type`, a full 58-slot inventory holding neither pet: 89.06 ns
    /// * the `AnyNPCs` pair over a full 200-slot NPC table: 82.48 ns
    /// * `good_place_for_a_statue_mimic`, the reference, in the same run: 20.95 ns
    ///
    /// So the worst case is about 172 ns, or eight plinth checks, on one attempt in a hundred and
    /// sixty, on a path that has already walked a ground column to reach it: 1.1 ns amortised over
    /// every attempt that gets as far as this line. Both worst cases are pessimistic on purpose. A
    /// real player's inventory is not fifty-eight full slots of things that are not the pet, and
    /// the `AnyNPCs` pair returns at the first pet it finds, which is the case where the arm
    /// declines and the whole cost stops being paid.
    #[test]
    #[ignore]
    fn measure_the_palworld_arm() {
        const N: u32 = 10_000_000;
        let bench = |name: &str, mut body: Box<dyn FnMut() -> u32 + '_>| {
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..N {
                sink = sink.wrapping_add(body());
            }
            let each = start.elapsed().as_secs_f64() / f64::from(N) * 1e9;
            println!("{name}: {each:.2} ns/call (sink {sink})");
        };

        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1);
        drop(out_rx);
        let bare = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx.clone());
        // The worst case `Player.HasItem` can be given: every one of the fifty-eight slots it scans
        // full, and neither pet among them, so both scans run to the end.
        let mut loaded = Player::new(1, "127.0.0.1:2".parse().unwrap(), out_tx);
        for slot in 0..58u16 {
            loaded.inventory.insert(
                slot,
                terrustia_proto::inventory::SyncEquipment {
                    player: 1,
                    slot,
                    item: terrustia_proto::item::ItemStack::new(i32::from(slot) + 1, 1, 0),
                    favorited: false,
                    blocked: false,
                },
            );
        }

        for (name, player) in [("empty", &bare), ("full inventory", &loaded)] {
            let mut rng = SmallRng::seed_from_u64(1601);
            bench(
                &format!("pal_type, {name}"),
                Box::new(|| u32::from(pal_type(std::hint::black_box(player), 0.0, &mut rng))),
            );
        }

        // `!AnyNPCs(696) && !AnyNPCs(695)`, over a table with no pet in it and every slot taken,
        // which is the only shape where both scans run to the end.
        let mut npcs = NpcStore::new();
        while npcs.spawn(1, (1000.0, 1000.0)).is_some() {}
        bench(
            "AnyNPCs pair, full table",
            Box::new(|| {
                u32::from(!std::hint::black_box(&npcs).iter().any(|(_, n)| {
                    matches!(n.npc_type, PAL_CATTIVA | PAL_FOXSPARKS) && n.is_alive()
                }))
            }),
        );

        // The reference, in the same run and the same build.
        let plinth = flat_world(90);
        let mut x = 200;
        bench(
            "good_place_for_a_statue_mimic",
            Box::new(|| {
                x = 200 + (x + 1) % 128;
                u32::from(good_place_for_a_statue_mimic(
                    std::hint::black_box(&plinth),
                    x,
                    90,
                ))
            }),
        );
    }

    /// What the friendly tail costs the candidate loop, which is the only place it runs.
    ///
    /// It is deliberately not a scan and adds none: [`deep_friendly_pick`] reads no tile, no wall
    /// and no NPC, and it is reached only after `spawnFriendly` has already decided this attempt
    /// belongs to a critter, which wants a populated base standing on the player. What it replaced
    /// was a bounds-checked index into a two-entry slice plus a [`holiday_costume`] call, so both
    /// sides of the change are a couple of `random_range` calls and the point of measuring is to say
    /// that as a number.
    ///
    /// Measured with the depth behind a `black_box` so the branch cannot be folded away. This
    /// machine was carrying other lanes at the time and the wall clock says so: the same run put
    /// `seasonal_night_pick` on an ordinary night at 15.20 ns against the 4.2 ns
    /// [`measure_the_graveyard_and_the_seasonal_chain`] recorded on a quiet one, so read these as a
    /// ratio against the reference in their own run and not as absolute nanoseconds.
    ///
    /// * caverns 25.1 ns: the fork, the one-in-five gate, and the gem ladder on the fifth of runs
    ///   that get past it. Three `random_range` calls where one hits.
    /// * dirt layer 13.3 ns and surface 12.6 ns: the fork alone, and then a constant or nothing.
    /// * the two-entry pool draw this replaced, same run and same build: 6.6 ns. One
    ///   `random_range`, and a `holiday_costume` that makes no roll at all out of season.
    ///
    /// So the caverns cost about four `random_range` calls where the old draw cost one, and the
    /// whole tail is 1.65 times `seasonal_night_pick`'s ordinary night in the same run. It reads no
    /// tile, and the arm above it that used to do the work read none either, so nothing was added to
    /// the per-candidate tile budget at all. The loop can reach it twenty times in a tick, but only
    /// for a player whose rate roll has already come up and who is standing in a town big enough to
    /// have quieted the wild, so the worst case is a fully staffed cavern base at half a microsecond
    /// of a 16.67 ms tick.
    #[test]
    #[ignore]
    fn measure_the_deep_friendly_tail() {
        const N: u32 = 10_000_000;
        for depth in [Depth::Cavern, Depth::Underground, Depth::Surface] {
            let mut rng = SmallRng::seed_from_u64(646);
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..N {
                sink = sink.wrapping_add(u32::from(
                    deep_friendly_pick(std::hint::black_box(depth), &mut rng).unwrap_or_default(),
                ));
            }
            let each = start.elapsed().as_secs_f64() / f64::from(N) * 1e9;
            println!("deep_friendly_pick, {depth:?}: {each:.2} ns/call (sink {sink})");
        }

        // The reference: what this replaced, in the same run and the same build. A two-entry pool,
        // indexed, then dressed for the season the way every other friendly draw still is.
        let pool: &[u16] = &[BUNNY, 299];
        let mut rng = SmallRng::seed_from_u64(646);
        let start = std::time::Instant::now();
        let mut sink = 0u32;
        for _ in 0..N {
            let critter = pool[rng.random_range(0..pool.len())];
            sink = sink.wrapping_add(u32::from(holiday_costume(
                critter,
                std::hint::black_box(Depth::Cavern),
                Seasonal::default(),
                &mut rng,
            )));
        }
        let each = start.elapsed().as_secs_f64() / f64::from(N) * 1e9;
        println!("the two-entry pool draw it replaced: {each:.2} ns/call (sink {sink})");
    }

    /// What the dungeon library's bookshelf scan costs, since it is by far the widest per-candidate
    /// tile read this module has: 32 by 32 is 1024 tiles against the plinth check's eight.
    ///
    /// It is behind `ZoneDungeon` and then a one-in-eight roll (or one in eleven), so nowhere but a
    /// dungeon reaches it at all and a dungeon reaches it on about one candidate in five. The scan
    /// has no early exit, by design: vanilla's has none either, and a shelf found early cannot end
    /// the search because a later row can still hold one closer to the corner.
    ///
    /// Measured on this machine, release, walking the anchor so no read is served from the previous
    /// call's cache line. The honest reading of the four numbers below is a ratio and a count, not a
    /// wall clock: other lanes share this machine and the same build measured 2.97 us and 62 us for
    /// the same 1024 reads twenty minutes apart. What does hold in every run is that
    /// `find_nearby_book` costs what its 1024 `World::tile` reads cost and no more (the two lines
    /// track each other within a third across five runs, in both directions), and that the empty box
    /// and the shelved box cost the same, because the tile fetch dominates and it is taken on all
    /// 1024 either way. On the quietest run the scan was 2.97 us, which is 2.9 ns a tile read: the
    /// same per-read figure `measure_the_statue_mimic_plinth_check` arrived at independently.
    ///
    /// The number that decides whether that matters is the last one, and it is exact rather than
    /// timed: over 2,000,000 `try_spawn` calls on a dungeon with a shelf running through it, the
    /// scan ran 5,405 times, one call in 370. At 2.97 us that is 8 ns amortised onto a `try_spawn`
    /// that measured 499 ns on the same quiet run, so about 1.6% of it, and nothing at all outside a
    /// dungeon.
    #[test]
    #[ignore]
    fn measure_the_bookshelf_scan() {
        let empty = flat_world(300);
        let shelved = {
            let mut world = empty.clone();
            for x in 0..world.width() {
                world.set_tile(x, 210, terrustia_proto::Tile::framed(BOOK_TILE, 0, 0));
            }
            world
        };

        for (name, world) in [("empty", &empty), ("shelved", &shelved)] {
            let n = 200_000;
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for i in 0..n {
                sink += u32::from(
                    find_nearby_book(
                        std::hint::black_box(world),
                        (200 + (i % 128), 200),
                        (10_000, 10_000),
                    )
                    .is_some(),
                );
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("find_nearby_book, {name}: {each:.2} ns/call (sink {sink})");
        }

        // The same 1024 reads with nothing but the tile fetch, so the scan's own arithmetic can be
        // told apart from what `World::tile` costs.
        let n = 200_000;
        let start = std::time::Instant::now();
        let mut sink = 0u32;
        for i in 0..n {
            let left = 200 + (i % 128);
            for y in 200..232 {
                for x in left..left + 32 {
                    sink += u32::from(std::hint::black_box(&shelved).tile(x, y).is_active());
                }
            }
        }
        let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
        println!("bare 1024 World::tile reads: {each:.2} ns/call (sink {sink})");

        // ...and what a whole `try_spawn` costs on a dungeon that has a library in it, which is the
        // number that actually matters: the scan is behind a rate roll and then a one-in-five.
        let (mut dungeon, (cx, cy)) = dungeon_world();
        dungeon.progress.downed_boss3 = true;
        shelve_books(&mut dungeon, cx, cy - 3);
        let npcs = NpcStore::new();
        let players = player_standing_at(cx, cy);
        let mut rng = SmallRng::seed_from_u64(7);
        let mut biomes = BiomeCache::default();
        let n = 2_000_000;
        let start = std::time::Instant::now();
        let (mut sink, mut scans) = (0usize, 0usize);
        for _ in 0..n {
            for (npc_type, _) in try_spawn(
                std::hint::black_box(&dungeon),
                &npcs,
                &players,
                &quiet(),
                &JourneyPowers::default(),
                &mut biomes,
                &mut rng,
            ) {
                sink += 1;
                // In this world a scan never comes back empty: the shelf spans the whole pocket, so
                // every candidate's box holds one outside the player's screen box. So a library
                // spawn is a scan, and the count of them is the count of scans.
                scans +=
                    usize::from(npc_type == WATER_BOLT_MIMIC || npc_type == LIBRARIAN_SKELETON);
            }
        }
        let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
        println!(
            "try_spawn in a shelved dungeon: {each:.2} ns/call ({sink} spawned, {scans} scans, \
             one scan per {:.0} calls)",
            f64::from(n) / scans as f64
        );
    }

    /// What hallowed ground's own arm costs a candidate that is not standing on it, which is every
    /// candidate in most worlds.
    ///
    /// The gate reads no tiles of its own: `events.hard_mode` is a bool, the depth was resolved a
    /// dozen lines above, and `HALLOW_GROUND.contains` scans four `u16` against the ground tile the
    /// candidate loop already read for the sandstorm and mushroom arms. Only once all three pass
    /// does anything else happen, and the store scan behind `alive` is behind four more conditions
    /// and a one-in-ten roll on top of that.
    ///
    /// One deliberate reorder against vanilla, disclosed because it is a reorder: `NPC.cs:4039`
    /// tests `!waterTile` before the four tile types, and this tests the tile types first. Both are
    /// pure, so the branch it takes is identical; the order is chosen so the array scan shuts the
    /// gate before `water_tile`'s own tile read is ever made.
    ///
    /// Measured on this machine, release build, the numbers printed by this test.
    #[test]
    #[ignore]
    fn measure_the_hallowed_ground_arm() {
        let n = 10_000_000;

        // The gate, on ordinary stone: what a non-hallowed world pays per candidate.
        for (name, ground) in [("stone", 1u16), ("pearlstone", 117u16)] {
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..n {
                sink += u32::from(
                    HALLOW_GROUND.contains(std::hint::black_box(&ground))
                        && matches!(std::hint::black_box(Depth::Surface), Depth::Surface),
                );
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("hallow gate, {name}: {each:.2} ns/call (sink {sink})");
        }

        // ...and the chain itself, on the one night it can answer with everything it has.
        let mut rng = SmallRng::seed_from_u64(7);
        let start = std::time::Instant::now();
        let mut sink = 0u32;
        for _ in 0..n {
            sink += u32::from(
                hallow_ground_pick(
                    std::hint::black_box(true),
                    std::hint::black_box(false),
                    std::hint::black_box(0),
                    std::hint::black_box(true),
                    std::hint::black_box(true),
                    &|_| false,
                    0.0,
                    &mut rng,
                )
                .is_some(),
            );
        }
        let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
        println!("hallow_ground_pick, full night: {each:.2} ns/call (sink {sink})");
    }

    /// What the friendly chain and the two gnome arms cost the per-candidate loop.
    ///
    /// Three things are new on a path that already runs, and only two of them are paid by an
    /// ordinary attempt:
    ///
    /// * `spawn_wall_type`, three tile reads, now on every dry surface candidate rather than only
    ///   on a windy day. It is the Living Tree gnome arm's gate and it sits behind two free tests.
    /// * [`underground_gnome`], on every candidate that gets past the water and the meteor. Its
    ///   first act is a one-in-ten roll, so nine candidates in ten pay a roll and nothing else.
    /// * [`friendly_chain`], which only a friendly attempt reaches, and at most once per tick
    ///   server-wide.
    ///
    /// Measured on this machine, release build, arguments behind a `black_box`. The numbers this
    /// prints:
    ///
    /// * `spawn_wall_type` on unwalled grass: 10 to 11 ns, the three tile reads. `try_spawn` as a
    ///   whole measures 242-527 ns a call ([`tests::measure_the_statue_mimic_plinth_check`]), and
    ///   the match this sits in runs once or twice an attempt, so it is two to four percent of one.
    /// * `underground_gnome` declining on unwalled ground: 2 to 3 ns, which is the roll and nothing
    ///   else nine times in ten.
    /// * `friendly_chain` on a dry beach: 11 to 12 ns, the arm answering at once with a Seagull. In
    ///   inland water: 120 to 140 ns, which is almost all the water-surface scan (up to 49 rows,
    ///   four tile reads each). In the rain over grass, still air, no cattail within reach: 3.7 to
    ///   4.6 us, and that one is the cattail sweep.
    ///
    /// Only the last is worth a second look, and it is 2501 tile reads by construction
    /// (`FindCattailTop`, `NPC.cs:81004-81043`). It is reached on a rainy daytime friendly attempt
    /// over grass or sand, in still air, at one attempt in two, and a friendly attempt is at most
    /// one a tick server-wide: 4.6 us against a 16.67 ms frame is 0.03% of one, on the rainiest
    /// afternoon a world can have.
    #[test]
    #[ignore]
    fn measure_the_friendly_chain() {
        let n = 10_000_000;
        let mut world = World::empty(1200, 600, "bench");
        world.surface = 260;
        world.rock_layer = 350;
        world.day_time = true;
        for x in 0..1200 {
            for y in 190..202 {
                world.set_tile(x, y, terrustia_proto::Tile::block(GRASS));
            }
            for y in 150..190 {
                world.set_tile(
                    x,
                    y,
                    terrustia_proto::Tile::AIR
                        .with_liquid(terrustia_proto::tile::Liquid::Water, 255),
                );
            }
        }

        // The Living Tree gnome arm's gate, on ground that is not one.
        let start = std::time::Instant::now();
        let mut sink = 0u32;
        for i in 0..n {
            sink += u32::from(
                spawn_wall_type(
                    std::hint::black_box(&world),
                    std::hint::black_box(600 + i % 4),
                    90,
                ) == LIVING_WOOD_WALL,
            );
        }
        let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
        println!("spawn_wall_type, unwalled grass: {each:.2} ns/call (sink {sink})");

        // ...and the underground gnome, declining.
        let mut rng = SmallRng::seed_from_u64(7);
        let start = std::time::Instant::now();
        let mut sink = 0u32;
        for i in 0..n {
            sink += u32::from(underground_gnome(
                std::hint::black_box(&world),
                std::hint::black_box(600 + i % 4),
                std::hint::black_box(190),
                std::hint::black_box(true),
                &|_| 0,
                0.0,
                &mut rng,
            ));
        }
        let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
        println!("underground_gnome, unwalled: {each:.2} ns/call (sink {sink})");

        // The chain itself: a dry beach, inland water, and the rain over grass with no cattail
        // anywhere, which is the sweep's worst case.
        for (name, calls, x, wet, raining) in [
            ("dry beach", n, 100, false, false),
            ("inland water", n, 700, true, false),
            ("rain over grass", 200_000, 700, false, true),
        ] {
            world.raining = raining;
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for i in 0..calls {
                sink += u32::from(
                    friendly_chain(
                        std::hint::black_box(&world),
                        std::hint::black_box(x + i % 4),
                        std::hint::black_box(190),
                        std::hint::black_box(GRASS),
                        std::hint::black_box(wet),
                        std::hint::black_box(false),
                        std::hint::black_box(0.0),
                        0.0,
                        &mut rng,
                    )
                    .is_some(),
                );
            }
            let each = start.elapsed().as_secs_f64() / f64::from(calls) * 1e9;
            println!("friendly_chain, {name}: {each:.2} ns/call (sink {sink})");
        }
    }

    /// What the fairy gate and the hornet swap cost the per-candidate loop.
    ///
    /// Both sit on a path that already runs and neither adds a scan of any kind.
    ///
    /// * [`underground_fairy`] is one arm in [`try_spawn`]'s chain, reached by every candidate that
    ///   gets past the water and the meteor, the same place [`underground_gnome`] already sits. On
    ///   a world with no fallen log left in it the first line is a bool and it returns; on a world
    ///   with one it is a single roll five hundred times in five hundred and one, and the `alive`
    ///   scan behind it is asked last so a declined roll never walks the NPC table.
    /// * [`hornet_variant`] is an after-the-draw swap on the final pool draw, which happens at most
    ///   once per tick server-wide, and its own first line is a `u16` compare.
    ///
    /// Measured on this machine, release build, arguments behind a `black_box`. The numbers this
    /// prints:
    ///
    /// * `underground_fairy`, no log: 1.08 ns, which is the bool and the return.
    /// * `underground_fairy`, logged and declining: 2.04 ns, the roll and nothing else.
    /// * `hornet_variant` on something that is not a hornet: 2.03 ns. On a hornet: 1.97 ns. The two
    ///   are the same because at this size the `black_box` on the argument costs as much as the
    ///   body: the swap is a compare and, at most, one roll.
    ///
    /// `try_spawn` as a whole measures 242-527 ns a call
    /// ([`tests::measure_the_statue_mimic_plinth_check`]), so the fairy arm is under one percent of
    /// one attempt and the hornet swap is not measurable against a tick at all.
    ///
    /// The whole-world log sweep behind `fairy_log` is not here because it is not on this path at
    /// all: it runs at world load and at dusk, and is measured next to itself in
    /// `game/server/systems.rs`'s own `measure_the_fallen_log_sweep`.
    #[test]
    #[ignore]
    fn measure_the_fairy_gate_and_the_hornet_swap() {
        let n = 10_000_000;
        let mut world = World::empty(1200, 600, "bench");
        world.surface = 100;
        world.rock_layer = 200;

        let mut rng = SmallRng::seed_from_u64(7);
        for (name, fairy_log) in [("no log", false), ("logged and declining", true)] {
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for i in 0..n {
                sink += u32::from(underground_fairy(
                    std::hint::black_box(&world),
                    std::hint::black_box(fairy_log),
                    std::hint::black_box(false),
                    std::hint::black_box(200 + i % 4),
                    &|| false,
                    0.0,
                    &mut rng,
                ));
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("underground_fairy, {name}: {each:.2} ns/call (sink {sink})");
        }

        for (name, base) in [("not a hornet", 43u16), ("a hornet", HORNET)] {
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..n {
                sink = sink.wrapping_add(u32::from(hornet_variant(
                    std::hint::black_box(base),
                    &mut rng,
                )));
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("hornet_variant, {name}: {each:.2} ns/call (sink {sink})");
        }
    }

    /// What the water chain costs, both halves of it.
    ///
    /// Two things moved. [`water_tile`] is now hoisted out of its own match guard and read once per
    /// candidate, so a candidate that never reaches the water arm (a friendly attempt, say) pays
    /// for it where it used not to; and [`water_pick`] replaced a slice index with a ladder of
    /// rolls. Neither adds a scan: `water_tile` is the same three tile reads it always was, and the
    /// ladder's own gates are a `u16` compare, two bools and an `i32` compare before any roll.
    ///
    /// Measured on this machine, release build, arguments behind a `black_box`:
    ///
    /// * `water_tile` on dry ground: 3.35 ns, which is what the hoist actually costs a candidate
    ///   that used not to reach it. `try_spawn` as a whole measures 242-527 ns a call
    ///   ([`tests::measure_the_statue_mimic_plinth_check`]), so it is under one and a half percent
    ///   of one attempt, once per candidate.
    /// * `water_pick` in an inland pool above the rock layer, classic mode, which is the ladder
    ///   running all the way to `None`: 7.33 ns. In a cavern pool in hardmode: 9.74 ns. In an
    ///   ocean: 7.94 ns. On jungle grass in hardmode, which the third arm answers early: 6.26 ns.
    ///
    /// That is against roughly two nanoseconds for the old slice index, and it is reached at most
    /// once per *tick* server-wide, after the rate roll has let an attempt through, a tile has been
    /// settled on and that tile has turned out to be underwater.
    #[test]
    #[ignore]
    fn measure_the_water_chain() {
        let n = 10_000_000;
        let mut sea = World::empty(800, 600, "bench");
        sea.surface = 100;
        sea.rock_layer = 200;

        // The hoisted read, on dry ground: what every candidate now pays.
        let start = std::time::Instant::now();
        let mut sink = 0u32;
        for _ in 0..n {
            sink += u32::from(water_tile(
                std::hint::black_box(&sea),
                std::hint::black_box(400),
                std::hint::black_box(150),
            ));
        }
        let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
        println!("water_tile, dry ground: {each:.2} ns/call (sink {sink})");

        // The ladder, at the three points that exercise different amounts of it.
        for (name, x, ground_y, biome, hard_mode, ground) in [
            ("inland pool, classic", 400, 150, Biome::Forest, false, None),
            ("cavern pool, hardmode", 400, 250, Biome::Forest, true, None),
            ("ocean, hardmode", 100, 80, Biome::Ocean, true, None),
            (
                "jungle grass, hardmode",
                400,
                80,
                Biome::Jungle,
                true,
                Some(JUNGLE_GRASS),
            ),
        ] {
            let mut rng = SmallRng::seed_from_u64(7);
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..n {
                sink += u32::from(
                    water_pick(
                        std::hint::black_box(&sea),
                        std::hint::black_box(x),
                        std::hint::black_box(ground_y),
                        std::hint::black_box(biome),
                        std::hint::black_box(hard_mode),
                        std::hint::black_box(ground),
                        &mut rng,
                    )
                    .unwrap_or(0),
                );
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("water_pick, {name}: {each:.2} ns/call (sink {sink})");
        }
    }

    /// What [`Zones::shimmer`] costs the per-tick scan, since it put work on the one arm of the
    /// 169-by-124 loop that used to do nothing but `continue`.
    ///
    /// Short samples, many of them, reported as the minimum. On a machine with sibling lanes
    /// building on it (load average 6 on four performance cores) a single long sample measures the
    /// scheduler: whole-test reruns of this scan spread 30 to 40 us, several times the effect
    /// being looked for, and the spread swamped even the *sign* of the delta. A millisecond-ish
    /// sample is short enough that some repeat lands between preemptions, and the minimum is that
    /// one. Validated by running the identical loop against itself, which reported a 0.00 to
    /// 0.18 us delta: that is this measurement's resolution.
    ///
    /// Three worlds, because the branch is taken once per *inactive* tile: all air is its worst
    /// case (20,956 of 20,956 tiles), a shimmer lake is where the count also accumulates, and
    /// solid stone is the floor, where it is never reached at all.
    ///
    /// To read the delta, put `false &&` in front of the `tile.liquid > 0` in [`zones_at`] and run
    /// this again. Measured that way, as the minimum of three whole-test runs each side:
    ///
    /// | box                       | without | with  | delta |
    /// |---------------------------|---------|-------|-------|
    /// | all air, 20,956 inactive  | 47.66   | 53.23 | +5.57 |
    /// | a shimmer lake            | 47.85   | 55.59 | +7.74 |
    /// | solid stone, none inactive| 90.73   | 94.00 | +3.27 |
    ///
    /// Microseconds a scan. Solid stone never reaches the branch, so its +3.27 is the register
    /// pressure of one more accumulator in the *active*-tile path rather than work: read the real
    /// per-inactive-tile cost as the 2 to 4 us above it. The worst case is 7.74 us on a scan
    /// [`BiomeCache::BUDGET`] allows eight of a tick, so 62 us of a 16.67 ms frame, 0.37%.
    /// Nothing on this path reads [`Zones::shimmer`]; the pylon check does, off the same cached
    /// entry, which is why it is counted here rather than scanned for separately.
    #[test]
    #[ignore]
    fn measure_the_shimmer_count() {
        let air = World::empty(800, 600, "shimmer bench: all air");

        let mut stone = World::empty(800, 600, "shimmer bench: solid");
        for y in 0..600 {
            for x in 0..800 {
                stone.set_tile(x, y, terrustia_proto::Tile::block(1));
            }
        }

        let mut lake = World::empty(800, 600, "shimmer bench: a real lake");
        for y in 250..350 {
            for x in 350..450 {
                lake.set_tile(
                    x,
                    y,
                    terrustia_proto::Tile::AIR
                        .with_liquid(terrustia_proto::tile::Liquid::Shimmer, 255),
                );
            }
        }

        for (name, world) in [
            ("all air", &air),
            ("a lake", &lake),
            ("solid stone", &stone),
        ] {
            let (n, repeats) = (10, 400);
            let mut best = f64::MAX;
            for _ in 0..repeats {
                let start = std::time::Instant::now();
                let mut sink = 0u32;
                for i in 0..n {
                    let at = std::hint::black_box((400 + i % 4, 300));
                    let zones =
                        std::hint::black_box(zones_at(std::hint::black_box(world), at.0, at.1));
                    sink += u32::from(zones.shimmer) + u32::from(zones.glowshroom);
                }
                std::hint::black_box(sink);
                best = best.min(start.elapsed().as_secs_f64() / f64::from(n) * 1e6);
            }
            println!("zones_at, {name}: {best:.2} us/scan");
        }
    }

    #[test]
    #[ignore]
    fn measure_the_tower_zone_test() {
        use crate::game::lunar;

        let none = quiet();
        let mut far = quiet();
        far.towers = [Some((900_000.0, 900_000.0)); 4];
        let mut inside = quiet();
        let slot = lunar::PILLARS
            .iter()
            .position(|p| *p == lunar::SOLAR)
            .expect("a real pillar");
        inside.towers[slot] = Some((1_000.0, 1_000.0));

        for (name, events) in [
            ("no apocalypse", &none),
            ("four standing, out of range", &far),
            ("inside the solar zone", &inside),
        ] {
            let n = 10_000_000;
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for i in 0..n {
                let at = std::hint::black_box((1_000.0 + (i % 4) as f32, 1_000.0));
                sink += u32::from(std::hint::black_box(events).tower_zone(at).unwrap_or(0));
            }
            let each = start.elapsed().as_secs_f64() / f64::from(n) * 1e9;
            println!("{name}: {each:.2} ns/test (sink {sink})");
        }
    }

    /// What the six new critter arms cost the per-candidate loop.
    ///
    /// None of them reads a tile of its own. Every input they want was already resolved by the time
    /// the match is reached: `depth`, `player_biome` and `ground_block` are all computed once per
    /// candidate for the arms above them, and the rest is `i32` arithmetic on the world's own
    /// dimensions. So the whole of the addition is a handful of comparisons and, where those pass,
    /// a roll.
    ///
    /// The shape of the cost is worth naming, because it is not flat. A *surface* candidate pays
    /// one enum comparison for all four of the below-the-surface arms and then nothing, since
    /// `depth != Depth::Surface` shuts the first of them; a cavern candidate pays all four gates
    /// and their rolls, plus the beetle arm's two comparisons and roll. The underworld's lava-bait
    /// arm is one comparison anywhere else.
    ///
    /// Measured on this machine, release build, ten million iterations each, the numbers this test
    /// prints:
    ///
    /// * the four below-the-surface gates on a surface candidate: 0.91 ns, which is the single
    ///   `depth` comparison and nothing else.
    /// * the same four in a forest cavern, rolls included: 6.60 ns.
    /// * the beetle arm on top of that: 2.47 ns.
    /// * `gold_variant` on a critter with no twin (a firefly): 0.77 ns, no roll made at all; on one
    ///   with a twin (a bunny): 2.52 ns.
    ///
    /// Two more were added to this loop later and measured against the same build on the same
    /// machine as each other rather than against the numbers above (which read 0.68, 9.44, 3.01,
    /// 1.69 and 3.03 in that run):
    ///
    /// * Doctor Bones' arm (`NPC.cs:3772`), which leads with the tile underfoot: 0.66 ns on an
    ///   ordinary floor, where the `Option<u16>` comparison shuts it, and 3.02 ns on jungle grass
    ///   where it goes on to make its roll. It is one line above the Lac Beetle's, so every
    ///   candidate that used to reach that arm now pays the smaller of those two first.
    /// * `zombie_settings`: 2.98 ns, one `rng` draw and two comparisons. It is not in the candidate
    ///   loop at all: `try_spawn` calls it once per attempt that has already got past both the cap
    ///   and the rate roll, which is the same one call in a few hundred, so it is nanoseconds per
    ///   *tick*.
    ///
    /// So a cavern candidate pays about 9 ns it did not, and a surface one under 1 ns. The
    /// reference is the plinth check this same loop already carries and already accepts as cheap,
    /// measured at 20.67 ns in the same build and the same run: the whole addition is under half of
    /// it in the caverns and a twentieth of it on the surface. Both sit inside the candidate loop,
    /// which `try_spawn` enters only once the rate roll lets an attempt through, roughly one call
    /// in 383 on an ordinary forest day
    /// ([`tests::measure_the_statue_mimic_plinth_check`], which also records why wall-clock timing
    /// of `try_spawn` as a whole cannot resolve a change this size on this machine).
    #[test]
    #[ignore]
    fn measure_the_ordinary_critter_arms() {
        const N: u32 = 10_000_000;
        let world = flat_world(300);
        let height = world.height();
        let rock_layer = i32::from(world.rock_layer);

        // The four below-the-surface gates, written exactly as `try_spawn` writes them, so what is
        // timed is the gate and not a stand-in for it.
        let four_gates = |depth: Depth, biome: Biome, y: i32, rng: &mut SmallRng| -> u32 {
            let land = !matches!(
                biome,
                Biome::Snow
                    | Biome::Crimson
                    | Biome::Corruption
                    | Biome::Jungle
                    | Biome::Hallow
                    | Biome::Dungeon
            );
            let snail_land = !matches!(
                biome,
                Biome::Snow | Biome::Crimson | Biome::Corruption | Biome::Hallow | Biome::Dungeon
            );
            u32::from(
                (depth != Depth::Surface
                    && y + 1 < height - CRITTER_FLOOR
                    && land
                    && rng.random_range(0..WORM_ODDS) == 0)
                    || (depth != Depth::Surface
                        && y + 1 < height - CRITTER_FLOOR
                        && land
                        && rng.random_range(0..MOUSE_ODDS) == 0)
                    || (depth != Depth::Surface
                        && y + 1 < (rock_layer + height) / 2
                        && snail_land
                        && rng.random_range(0..SNAIL_ODDS) == 0),
            )
        };

        for (name, depth, y) in [
            ("surface candidate", Depth::Surface, 80),
            ("forest cavern candidate", Depth::Cavern, 300),
        ] {
            let mut rng = SmallRng::seed_from_u64(357);
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..N {
                sink += four_gates(
                    std::hint::black_box(depth),
                    std::hint::black_box(Biome::Forest),
                    std::hint::black_box(y),
                    &mut rng,
                );
            }
            let each = start.elapsed().as_secs_f64() / f64::from(N) * 1e9;
            println!("the four below-the-surface gates, {name}: {each:.2} ns/call (sink {sink})");
        }

        // The caverns' beetle arm, which is two comparisons and a roll and nothing else.
        let mut rng = SmallRng::seed_from_u64(217);
        let start = std::time::Instant::now();
        let mut sink = 0u32;
        for _ in 0..N {
            sink += u32::from(
                std::hint::black_box(Depth::Cavern) == Depth::Cavern
                    && std::hint::black_box(Biome::Forest) != Biome::Dungeon
                    && rng.random_range(0..CAVERN_BEETLE_ODDS) == 0,
            );
        }
        let each = start.elapsed().as_secs_f64() / f64::from(N) * 1e9;
        println!("the cavern beetle arm: {each:.2} ns/call (sink {sink})");

        // Doctor Bones' arm, which sits one line above the Lac Beetle's and leads with the tile
        // underfoot, so an ordinary floor pays one `Option<u16>` comparison and stops.
        for (name, ground) in [
            ("ordinary floor", Some(1u16)),
            ("jungle grass", Some(JUNGLE_GRASS)),
        ] {
            let mut rng = SmallRng::seed_from_u64(52);
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..N {
                sink += u32::from(
                    std::hint::black_box(ground) == Some(JUNGLE_GRASS)
                        && !std::hint::black_box(false)
                        && rng.random_range(0..DOCTOR_BONES_ODDS) == 0,
                );
            }
            let each = start.elapsed().as_secs_f64() / f64::from(N) * 1e9;
            println!("the Doctor Bones arm, {name}: {each:.2} ns/call (sink {sink})");
        }

        // ...and `GetZombieSettings`, which `try_spawn` calls once per attempt that has already got
        // past both the cap and the rate roll: one `rng` draw and two comparisons.
        {
            let mut rng = SmallRng::seed_from_u64(590);
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..N {
                let settings = zombie_settings(
                    std::hint::black_box(true),
                    std::hint::black_box(1),
                    &mut rng,
                );
                sink += u32::from(settings.style) + settings.torch_chance;
            }
            let each = start.elapsed().as_secs_f64() / f64::from(N) * 1e9;
            println!("zombie_settings: {each:.2} ns/call (sink {sink})");
        }

        // ...and the gold swap, which is a `match` first and a roll only for the eight types that
        // have a twin at all.
        for (name, base) in [("firefly, no twin", FIREFLY), ("bunny, has a twin", 46u16)] {
            let mut rng = SmallRng::seed_from_u64(442);
            let start = std::time::Instant::now();
            let mut sink = 0u32;
            for _ in 0..N {
                sink += u32::from(gold_variant(
                    std::hint::black_box(base),
                    std::hint::black_box(Depth::Surface),
                    0.0,
                    &mut rng,
                ));
            }
            let each = start.elapsed().as_secs_f64() / f64::from(N) * 1e9;
            println!("gold_variant, {name}: {each:.2} ns/call (sink {sink})");
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

    /// The bound Old Slime is a caverns find beside the Goblin Tinkerer and the Wizard, gated on
    /// Skeletron and closed for good once it is freed (`NPC.cs:2095`).
    ///
    /// Fails before the fix, when 685 was in no producer at all: `pick_bound` read only the talk
    /// rescue table and `bound_gate` had no arm for it, so the Old Slime was unreachable however
    /// far a world progressed.
    #[test]
    fn the_caverns_offer_a_bound_old_slime_once_skeletron_is_down() {
        let mut world = test_world();
        world.progress = Default::default();
        let y = i32::from(world.rock_layer) + 5;
        assert_eq!(depth_at(&world, y), Depth::Cavern);

        assert!(
            !bound_gate(
                BOUND_TOWN_SLIME_OLD,
                &world,
                Depth::Cavern,
                Biome::Forest,
                y
            ),
            "the Old Slime needs Skeletron down"
        );
        world.progress.downed_boss3 = true;
        assert!(bound_gate(
            BOUND_TOWN_SLIME_OLD,
            &world,
            Depth::Cavern,
            Biome::Forest,
            y
        ));
        assert!(
            !bound_gate(
                BOUND_TOWN_SLIME_OLD,
                &world,
                Depth::Underground,
                Biome::Forest,
                y
            ),
            "it is a caverns find, not a dirt-layer one"
        );

        // And it is actually offered: `pick_bound` has to reach it, which is the half a gate
        // alone cannot prove.
        let mut rng = SmallRng::seed_from_u64(11);
        let offered = (0..2_000).any(|_| {
            pick_bound(
                &world,
                &NpcStore::new(),
                Depth::Cavern,
                Biome::Forest,
                y,
                &mut rng,
            ) == Some(BOUND_TOWN_SLIME_OLD)
        });
        assert!(offered, "a post-Skeletron cavern must be able to offer it");

        // Freeing it shuts the arm: `!unlockedSlimeOldSpawn` is half of vanilla's condition, so a
        // world with the resident in a house must never offer a second bound one.
        world.progress.unlocked_slime_old = true;
        let mut rng = SmallRng::seed_from_u64(11);
        for _ in 0..2_000 {
            assert_ne!(
                pick_bound(
                    &world,
                    &NpcStore::new(),
                    Depth::Cavern,
                    Biome::Forest,
                    y,
                    &mut rng,
                ),
                Some(BOUND_TOWN_SLIME_OLD),
                "a freed Old Slime must not be found again"
            );
        }
    }

    /// The sky offers a bound Purple Slime one attempt in twenty-five until somebody frees it
    /// (`NPC.cs:1417`), and never once it is freed.
    ///
    /// Fails before the fix, when `sky_pick` had no arm for it at all and the sky was only ever a
    /// probe, a Wyvern or a Harpy.
    #[test]
    fn the_sky_offers_a_bound_purple_slime_until_somebody_frees_it() {
        let mut world = World::empty(800, 600, "sky slime");
        let never = |_: u16| false;

        let mut rng = SmallRng::seed_from_u64(3);
        let seen = (0..2_000)
            .filter(|_| {
                sky_pick(false, false, &world, false, &never, 0.0, &mut rng)
                    == BOUND_TOWN_SLIME_PURPLE
            })
            .count();
        assert!(
            seen > 0,
            "a fresh sky must be able to offer a bound Purple Slime"
        );

        // One at a time: `!AnyNPCs(686)` means a slime already up there stops a second.
        let already = |ty: u16| ty == BOUND_TOWN_SLIME_PURPLE;
        let mut rng = SmallRng::seed_from_u64(3);
        for _ in 0..2_000 {
            assert_ne!(
                sky_pick(false, false, &world, false, &already, 0.0, &mut rng),
                BOUND_TOWN_SLIME_PURPLE,
                "only one bound Purple Slime is ever in the sky at once"
            );
        }

        world.progress.unlocked_slime_purple = true;
        let mut rng = SmallRng::seed_from_u64(3);
        for _ in 0..2_000 {
            assert_ne!(
                sky_pick(false, false, &world, false, &never, 0.0, &mut rng),
                BOUND_TOWN_SLIME_PURPLE,
                "a freed Purple Slime must not be found again"
            );
        }
    }

    /// A frog draw is `NPC.SpawnFrog`'s chain, not a frog (`NPC.cs:5621-5634`): one in thirty is a
    /// bound Yellow Slime until somebody purifies it.
    ///
    /// Fails before the fix, when the jungle's friendly pool answered 361 flat and 687 was in no
    /// producer at all.
    #[test]
    fn a_frog_draw_can_be_a_bound_yellow_slime_until_it_is_freed() {
        let mut world = World::empty(800, 600, "frog");
        let never = |_: u16| false;

        let mut rng = SmallRng::seed_from_u64(7);
        let seen = (0..3_000)
            .filter(|_| spawn_frog(&world, &never, 0.0, &mut rng) == BOUND_TOWN_SLIME_YELLOW)
            .count();
        assert!(seen > 0, "a fresh jungle must be able to offer one");
        assert!(
            seen < 3_000 / 4,
            "the plain frog is still the common answer, not the slime: {seen} of 3000"
        );

        let already = |ty: u16| ty == BOUND_TOWN_SLIME_YELLOW;
        let mut rng = SmallRng::seed_from_u64(7);
        for _ in 0..3_000 {
            assert_eq!(
                spawn_frog(&world, &already, 0.0, &mut rng),
                FROG,
                "only one bound Yellow Slime is ever about at once"
            );
        }

        world.progress.unlocked_slime_yellow = true;
        let mut rng = SmallRng::seed_from_u64(7);
        for _ in 0..3_000 {
            assert_eq!(
                spawn_frog(&world, &never, 0.0, &mut rng),
                FROG,
                "a freed Yellow Slime must not be found again"
            );
        }
    }

    /// A flat, open, empty hall with a single floor row at `floor`, in a world whose surface and
    /// rock layer are fixed so `depth_at` is predictable. `World::empty` is all air, so the floor
    /// is the only tile in it and the biome scan reads a plain forest.
    fn hall_world(floor: i32) -> World {
        let mut world = World::empty(800, 600, "hall");
        world.surface = 100;
        world.rock_layer = 200;
        for x in 0..world.width() {
            world.set_tile(x, floor, terrustia_proto::Tile::block(1));
        }
        world
    }

    /// How many of `wanted` a long run of spawn attempts produces at `floor` in `world`.
    fn spawns_of(
        world: &World,
        floor: i32,
        events: &EventSpawns<'_>,
        wanted: u16,
        seed: u64,
    ) -> usize {
        let npcs = NpcStore::new();
        let players = player_at(((world.width() / 2) as f32 * 16.0, (floor - 1) as f32 * 16.0));
        let mut rng = SmallRng::seed_from_u64(seed);
        let mut seen = 0;
        for _ in 0..60_000 {
            seen += try_spawn(
                world,
                &npcs,
                &players,
                events,
                &JourneyPowers::default(),
                &mut BiomeCache::default(),
                &mut rng,
            )
            .iter()
            .filter(|(npc_type, _)| *npc_type == wanted)
            .count();
        }
        seen
    }

    /// The underworld offers a Tortured Soul (`NPC.cs:4877`), and only under vanilla's own three
    /// conditions: hardmode, and a world that has not got its Tax Collector yet.
    ///
    /// Fails before the fix, when 534 was in no pool and no branch: nothing this server could do
    /// would ever put one in a world, so the Tax Collector - his shop, his happiness, his arrival
    /// message, all of it already written - was unreachable for good.
    #[test]
    fn the_underworld_offers_a_tortured_soul_once_a_world_is_in_hardmode() {
        let floor = 600 - UNDERWORLD_DEPTH + 50;
        let mut world = hall_world(floor);
        assert_eq!(
            depth_at(&world, floor - 1),
            Depth::Underworld,
            "the hall has to be in the underworld for this to test anything"
        );

        // Pre-hardmode: `Main.hardMode` is the first clause of the branch, so never.
        assert_eq!(
            spawns_of(&world, floor, &quiet(), TORTURED_SOUL, 5),
            0,
            "a pre-hardmode underworld spawned a Tortured Soul"
        );

        let mut hard = quiet();
        hard.hard_mode = true;
        world.progress.hard_mode = true;
        assert!(
            spawns_of(&world, floor, &hard, TORTURED_SOUL, 5) > 0,
            "a hardmode underworld never offered a Tortured Soul"
        );

        // `!savedTaxCollector`: once the world has him, the branch is shut for good.
        world.progress.saved_tax_collector = true;
        assert_eq!(
            spawns_of(&world, floor, &hard, TORTURED_SOUL, 5),
            0,
            "a world that already has its Tax Collector kept spawning Tortured Souls"
        );
    }

    /// The caverns offer a Skeleton Merchant (`NPC.cs:5004-5010`), at any point in a world's
    /// progression, and never in the underworld above or the underground below him.
    ///
    /// Fails before the fix, when 453 was in no pool and no branch, so the one wandering vendor in
    /// the game could not be met however long a world was played.
    #[test]
    fn the_caverns_offer_a_skeleton_merchant_at_any_progression() {
        let cavern_floor = 300;
        let world = hall_world(cavern_floor);
        assert_eq!(depth_at(&world, cavern_floor - 1), Depth::Cavern);
        assert!(
            spawns_of(&world, cavern_floor, &quiet(), SKELETON_MERCHANT, 9) > 0,
            "a plain cavern never offered a Skeleton Merchant"
        );

        // He is a cavern spawn, not an underworld one: vanilla's underworld arm is a whole branch
        // earlier in the same chain and never reaches him.
        let hell_floor = 600 - UNDERWORLD_DEPTH + 50;
        let hell = hall_world(hell_floor);
        assert_eq!(depth_at(&hell, hell_floor - 1), Depth::Underworld);
        assert_eq!(
            spawns_of(&hell, hell_floor, &quiet(), SKELETON_MERCHANT, 9),
            0,
            "the underworld spawned a Skeleton Merchant"
        );
    }

    /// The surface offers a Goblin Scout by day, and only out past a third of the map from the
    /// world's spawn point (`NPC.cs:4482-4485`).
    ///
    /// Fails before the fix, when 73 was in no pool and no branch. That silence reached further
    /// than one missing enemy: he is the game's only source of Tattered Cloth, the cloth is the
    /// only ingredient of the Goblin Battle Standard, and the standard is the only way anybody
    /// summons a goblin army. All three of those were already written here and all three were
    /// unreachable.
    ///
    /// Neutralised by deleting the `out.push((GOBLIN_SCOUT, ...))` block from `try_spawn`: the
    /// first and last assertions fail ("a surface day far from spawn offered no Goblin Scout").
    /// Neutralised again by dropping the `(x - world.spawn_x).abs() > world.width() / 3` clause:
    /// the near-spawn assertion fails instead, scouts appearing in the back garden.
    #[test]
    fn the_surface_day_offers_a_goblin_scout_out_past_a_third_of_the_map() {
        // Long enough that the two rolls are told apart by their counts rather than by luck. The
        // run measures 69 scouts on the plain roll and 226 with a shadow orb smashed, against the
        // 3.0x the two rates predict (1/15 against 1/15 + 14/15 * 1/7); the assertions below leave
        // that a wide berth. The whole test is about a second.
        const TICKS: u32 = 400_000;
        let scouts = |world: &World, (px, py): (i32, i32)| {
            spawns_at(world, false, px, py, TICKS)
                .into_iter()
                .filter(|ty| *ty == GOBLIN_SCOUT)
                .count()
        };

        // `World::empty` puts the spawn point at the middle of the map, which is where these tests
        // stand their player, so every candidate tile is inside the spawn box and the gate is shut.
        let (near_spawn, at) = forest_surface();
        assert_eq!(
            scouts(&near_spawn, at),
            0,
            "a Goblin Scout turned up within a third of the map of the world spawn"
        );

        // The same world with the spawn point moved to the far edge: now every candidate is more
        // than `width / 3` away from it, which is the whole of vanilla's `num45` gate.
        let mut world = near_spawn.clone();
        world.spawn_x = 0;
        assert!(
            (at.0 - i32::from(world.spawn_x)).abs() - SPAWN_RANGE_X > world.width() / 3,
            "the whole spawn box has to be outside the gate for this to test the gate"
        );
        let baseline = scouts(&world, at);
        assert!(
            baseline > 0,
            "a surface day far from spawn offered no Goblin Scout"
        );

        // `!ZoneGraveyard && Main.dayTime` (`NPC.cs:4202`): the whole block he lives in is daytime
        // only, and a graveyard skips it even in daylight.
        let mut night = world.clone();
        night.day_time = false;
        assert_eq!(
            scouts(&night, at),
            0,
            "the surface night offered a Goblin Scout"
        );
        assert_eq!(
            spawns_at_in(&world, false, true, at.0, at.1, TICKS)
                .into_iter()
                .filter(|ty| *ty == GOBLIN_SCOUT)
                .count(),
            0,
            "a graveyard offered a Goblin Scout in daylight"
        );

        // `!waterTile`: standing in the sea is not standing on the ground. `water_tile` reads the
        // spawn row and the one above it, which for a floor at `at.1 + 2` are `at.1 + 1` and `at.1`.
        let mut flooded = world.clone();
        for x in 0..flooded.width() {
            for y in at.1..=(at.1 + 1) {
                flooded.set_tile(
                    x,
                    y,
                    terrustia_proto::Tile::AIR
                        .with_liquid(terrustia_proto::tile::Liquid::Water, 255),
                );
            }
        }
        assert_eq!(
            scouts(&flooded, at),
            0,
            "a flooded surface offered a Goblin Scout"
        );

        // The second roll (`!downedGoblins && WorldGen.shadowOrbSmashed && rand.Next(7) == 0`) is
        // OR'd onto the first rather than replacing it, so a smashed shadow orb takes him from one
        // attempt in fifteen to roughly one in five. Well over double either way, which is what
        // makes this an assertion rather than a coin toss.
        let mut hunted = world.clone();
        hunted.progress.shadow_orb_smashed = true;
        let raised = scouts(&hunted, at);
        assert!(
            raised > baseline * 2,
            "a smashed shadow orb barely changed the Goblin Scout rate: {baseline} -> {raised}"
        );

        // ...and it closes for good once the world has had its goblin army.
        hunted.progress.downed_goblins = true;
        let after = scouts(&hunted, at);
        assert!(
            after > 0 && after < raised / 2,
            "a downed goblin army left the shadow-orb rate up: {raised} -> {after}"
        );
    }

    /// The surface day offers a distressed Palworld pet an eighth of the map out from spawn, on
    /// grass, snow, jungle grass or ice, and only when neither of the pair is already about
    /// (`NPC.cs:4374-4389`).
    ///
    /// Fails before the fix, when 695 and 696 were in no pool and no branch: the whole crossover
    /// encounter (`ai::hardmode::fixtures::pal`, its two Goblin Archer guards and the two Palworld
    /// Minion items) was written and unreachable, and both types sat in `docs/spawn-gaps.tsv`.
    ///
    /// Neutralised by deleting the `out.push((pal_type(player, 0.0, rng), ...))` block from `try_spawn`:
    /// the "offered no pal" assertion fails. Neutralised again by dropping the
    /// `(x - world.spawn_x).abs() > world.width() / 8` clause: the near-spawn assertion fails
    /// instead. And a third time by dropping the `matches!(ground_block, ...)` clause: the
    /// stone-floor assertion fails, a pet turning up on bare rock.
    #[test]
    fn the_surface_day_offers_a_palworld_pet_on_the_right_ground_and_away_from_spawn() {
        // One attempt in 160 on top of everything else the chain declines first, so this wants a
        // long run: it measures 236 pets over 400k ticks.
        const TICKS: u32 = 400_000;
        let pets = |world: &World, (px, py): (i32, i32)| {
            spawns_at(world, false, px, py, TICKS)
                .into_iter()
                .filter(|ty| matches!(*ty, PAL_CATTIVA | PAL_FOXSPARKS))
                .count()
        };

        // Grass underfoot, since the arm keys on the ground tile and the shared flat world is
        // stone. `World::empty` puts the spawn point at the middle, which is where the player
        // stands, so the distance gate is shut here.
        let near_spawn = flat_world_sized(1600, 90, GRASS);
        let at = (800, 88);
        assert_eq!(depth_at(&near_spawn, at.1), Depth::Surface);
        assert_eq!(
            pets(&near_spawn, at),
            0,
            "a pal turned up within an eighth of the map of the world spawn"
        );

        let mut world = near_spawn.clone();
        world.spawn_x = 0;
        assert!(
            (at.0 - i32::from(world.spawn_x)).abs() - SPAWN_RANGE_X > world.width() / 8,
            "the whole spawn box has to be outside the gate for this to test the gate"
        );
        let baseline = pets(&world, at);
        assert!(baseline > 0, "a surface day far from spawn offered no pal");

        // Both of the pair are reachable, which is what keeps `ambient_roster` listing them honest.
        let drawn: std::collections::BTreeSet<u16> = spawns_at(&world, false, at.0, at.1, TICKS)
            .into_iter()
            .filter(|ty| matches!(*ty, PAL_CATTIVA | PAL_FOXSPARKS))
            .collect();
        assert!(
            drawn.contains(&PAL_CATTIVA) && drawn.contains(&PAL_FOXSPARKS),
            "only one of the pair is reachable: {drawn:?}"
        );

        // `tileType == 2 || 147 || 60 || 161`: stone is none of them, and the same world with a
        // stone floor offers nothing.
        let mut stone = flat_world_sized(1600, 90, 1);
        stone.spawn_x = 0;
        assert_eq!(pets(&stone, at), 0, "a pal turned up on a stone floor");

        // ...and the same for ice, which *is* one of them, so this is the other half of the claim.
        let mut ice = flat_world_sized(1600, 90, ICE_BLOCK);
        ice.spawn_x = 0;
        assert!(pets(&ice, at) > 0, "ice underfoot offered no pal");

        // `!ZoneGraveyard && Main.dayTime` (`NPC.cs:4202`), the block it lives in.
        let mut night = world.clone();
        night.day_time = false;
        assert_eq!(pets(&night, at), 0, "the surface night offered a pal");
        assert_eq!(
            spawns_at_in(&world, false, true, at.0, at.1, TICKS)
                .into_iter()
                .filter(|ty| matches!(*ty, PAL_CATTIVA | PAL_FOXSPARKS))
                .count(),
            0,
            "a graveyard offered a pal in daylight"
        );
    }

    /// `!AnyNPCs(696) && !AnyNPCs(695)` (`NPC.cs:4374`): one pet at a time in the whole world, and
    /// either of the pair blocks the other.
    ///
    /// Neutralised by dropping the `!npcs.iter().any(...)` clause: both assertions fail, a second
    /// pet appearing beside one that is already out.
    #[test]
    fn one_palworld_pet_at_a_time_blocks_the_other() {
        const TICKS: u32 = 400_000;
        let mut world = flat_world_sized(1600, 90, GRASS);
        world.spawn_x = 0;
        let at = (800, 88);

        for already in [PAL_CATTIVA, PAL_FOXSPARKS] {
            let mut npcs = NpcStore::new();
            npcs.spawn(already, (200.0 * 16.0, 88.0 * 16.0))
                .expect("a free slot");
            let seen = spawns_with_npcs(&world, &npcs, &quiet(), at.0, at.1, TICKS)
                .into_iter()
                .filter(|(ty, _)| matches!(*ty, PAL_CATTIVA | PAL_FOXSPARKS))
                .count();
            assert_eq!(seen, 0, "a pal was offered while {already} was already out");
        }
    }

    /// Which of the pair arrives is decided by what the player is already carrying
    /// (`NPC.cs:4375-4386`): with one of the two Palworld Minion items in the inventory and not the
    /// other, the one you are missing is the one you meet.
    ///
    /// Neutralised by dropping both `if !has_* && has_*` overrides from `pal_type`: both assertions
    /// fail, each run coming back with a mix instead of one type.
    #[test]
    fn a_pal_is_whichever_of_the_pair_you_are_not_carrying() {
        const TICKS: u32 = 200_000;
        let mut world = flat_world_sized(1600, 90, GRASS);
        world.spawn_x = 0;
        let at = (800, 88);

        for (carried, wanted, name) in [
            (
                terrustia_proto::npc_params::PAL_REWARD_CATTIVA,
                PAL_FOXSPARKS,
                "Foxsparks",
            ),
            (
                terrustia_proto::npc_params::PAL_REWARD_FOXSPARKS,
                PAL_CATTIVA,
                "Cattiva",
            ),
        ] {
            let drawn: std::collections::BTreeSet<u16> =
                spawns_carrying(&world, carried, at.0, at.1, TICKS)
                    .into_iter()
                    .filter(|ty| matches!(*ty, PAL_CATTIVA | PAL_FOXSPARKS))
                    .collect();
            assert_eq!(
                drawn,
                std::collections::BTreeSet::from([wanted]),
                "carrying {carried} should only ever offer the {name}"
            );
        }
    }

    /// The dungeon's own chain offers its two traps, and the brick decides which one
    /// (`NPC.cs:2723-2747`).
    ///
    /// Fails before the fix. `game::ai::track` holds both routines in full, with their own tests,
    /// and 70 and 72 were in no pool and no branch, so no dungeon this server ever built had a spike
    /// ball or a blazing wheel in it. The three that *were* in the pool were there flattened: a
    /// Cursed Skull turning up on slab and tile walls it is never found on, and all five sharing a
    /// draw instead of taking their own rolls in order.
    ///
    /// Neutralised by deleting the `SPIKE_BALL` and `BLAZING_WHEEL` arms from `dungeon_pick`: their
    /// two "offered no" assertions fail. Neutralised again by dropping the `style ==` test from each
    /// of the four gated arms: the three "on the wrong brick" assertions fail instead. Neutralised a
    /// third way, by making `near_spike_ball` return `false` unconditionally: the crowding assertion
    /// fails, a second ball dropping beside the first.
    #[test]
    fn a_dungeon_hangs_its_traps_on_the_brick_they_belong_to() {
        /// `WallID.BlueDungeonSlabUnsafe` and `WallID.BlueDungeonTileUnsafe` (`NPC.cs:2634`,
        /// `:2638`): one of each of the two sets `dungeon_brick_style` reads.
        const SLAB_WALL: u16 = 94;
        const TILE_WALL: u16 = 95;

        let (base, (cx, cy)) = dungeon_world();
        let walled = |wall: u16| {
            let mut world = base.clone();
            world.progress.downed_boss3 = true;
            if wall != 0 {
                for xx in (cx - 110)..=(cx + 110) {
                    for yy in (cy - 55)..=(cy + 55) {
                        let tile = world.tile(xx, yy).with_wall(wall);
                        world.set_tile(xx, yy, tile);
                    }
                }
            }
            world
        };
        let found = |world: &World, npcs: &NpcStore, wanted: u16| {
            let players = player_standing_at(cx, cy);
            let mut rng = SmallRng::seed_from_u64(20260902);
            let mut seen = 0;
            let mut biomes = BiomeCache::default();
            for _ in 0..60_000 {
                seen += try_spawn(
                    world,
                    npcs,
                    &players,
                    &quiet(),
                    &JourneyPowers::default(),
                    &mut biomes,
                    &mut rng,
                )
                .into_iter()
                .filter(|(ty, _)| *ty == wanted)
                .count();
            }
            seen
        };

        let empty = NpcStore::new();
        let (slab, tiled, plain) = (walled(SLAB_WALL), walled(TILE_WALL), walled(0));

        // A slab dungeon has spike balls and no wheels and no cursed skulls; a tile dungeon has
        // wheels and neither of the others; plain brick has cursed skulls and no trap at all.
        // `dungeon_brick_style` rerolls one attempt in seven (`NPC.cs:2642-2645`), so "the wrong
        // brick" is a rate rather than a bar, and each of these is comfortably the wrong side of it.
        assert!(
            found(&slab, &empty, SPIKE_BALL) > 0,
            "a slab-walled dungeon hung no Spike Ball"
        );
        assert!(
            found(&tiled, &empty, BLAZING_WHEEL) > 0,
            "a tile-walled dungeon ran no Blazing Wheel"
        );
        assert!(
            found(&plain, &empty, CURSED_SKULL) > 0,
            "a plain-brick dungeon offered no Cursed Skull"
        );
        let stray_wheel = found(&slab, &empty, BLAZING_WHEEL);
        let stray_ball = found(&tiled, &empty, SPIKE_BALL);
        let stray_skull = found(&slab, &empty, CURSED_SKULL);
        let ball = found(&slab, &empty, SPIKE_BALL);
        assert!(
            stray_wheel * 4 < ball && stray_ball * 4 < ball && stray_skull * 4 < ball,
            "the brick style barely moved what a dungeon hangs on it: \
             ball {ball}, stray wheel {stray_wheel}, stray ball {stray_ball}, \
             stray skull {stray_skull}"
        );

        // The Dark Caster's arm has no brick test at all, so it answers on every wall.
        for (world, name) in [(&slab, "slab"), (&tiled, "tile"), (&plain, "plain")] {
            assert!(
                found(world, &empty, DARK_CASTER) > 0,
                "a {name}-walled dungeon offered no Dark Caster, and its arm has no brick gate"
            );
        }

        // `NearSpikeBall` (`NPC.cs:91002-91017`): a ball already anchored within 300 pixels of a
        // candidate shuts that candidate's arm, which is what stops a corridor filling up with
        // them. It is a box around the *candidate*, not around the player, so most of a spawn ring
        // 84 tiles wide is well outside it and keeps producing balls: the claim is that none lands
        // inside the box, not that none lands at all.
        //
        // The anchor has to sit in the ring rather than on the player. `SAFE_RANGE_X` is 62 tiles,
        // so nothing ever spawns within 992 pixels of where the player stands and a box only 300
        // pixels wide around them could never have caught anything. Seventy tiles out is inside the
        // ring and clear of the safe box. And the anchor is `ai[1]`/`ai[2]`, not the ball's own
        // position, so it is seeded here the way its first tick seeds it.
        const ANCHOR_OUT: i32 = 70;
        const { assert!(ANCHOR_OUT > SAFE_RANGE_X && ANCHOR_OUT < SPAWN_RANGE_X) };
        let (anchor_x, anchor_y) = ((cx + ANCHOR_OUT) * 16, cy * 16);
        let mut crowded = NpcStore::new();
        let slot = crowded
            .spawn(SPIKE_BALL, (anchor_x as f32, anchor_y as f32))
            .expect("a spike ball");
        {
            let ball = crowded.get_mut(slot).expect("the ball");
            ball.ai[1] = anchor_x as f32;
            ball.ai[2] = anchor_y as f32;
        }
        // The box is spelled out here rather than asked of `near_spike_ball`, deliberately: a test
        // that checks production with the very function production used passes just as happily with
        // that function stubbed out to `false`, which is how this assertion was first written and
        // what neutralising it caught. This is `Rectangle.Intersects` on
        // `(x * 16 - 300, y * 16 - 300, 600, 600)` against `(ai[1], ai[2], 20, 20)`, straight off
        // `NPC.cs:91004-91010`, with vanilla's `spawnTileY` written out as this server's `y + 1`.
        let inside_the_box = |sx: f32, sy: f32| {
            let (left, top) = (sx as i32 - 300, sy as i32 + 16 - 300);
            anchor_x < left + 600
                && left < anchor_x + 20
                && anchor_y < top + 600
                && top < anchor_y + 20
        };
        let in_the_box = |npcs: &NpcStore| {
            let players = player_standing_at(cx, cy);
            let mut rng = SmallRng::seed_from_u64(20260902);
            let mut biomes = BiomeCache::default();
            let (mut balls, mut crowding) = (0, 0);
            for _ in 0..60_000 {
                for (ty, (sx, sy)) in try_spawn(
                    &slab,
                    npcs,
                    &players,
                    &quiet(),
                    &JourneyPowers::default(),
                    &mut biomes,
                    &mut rng,
                ) {
                    if ty != SPIKE_BALL {
                        continue;
                    }
                    balls += 1;
                    if inside_the_box(sx, sy) {
                        crowding += 1;
                    }
                }
            }
            (balls, crowding)
        };

        // With nothing there, the box is a place balls really do land, which is what stops the
        // assertion below from being vacuously true.
        let (loose_balls, loose_inside) = in_the_box(&empty);
        assert!(
            loose_inside > 0,
            "an empty dungeon put none of its {loose_balls} Spike Balls where the anchor will be, \
             so the crowding test below would prove nothing"
        );
        let (balls, crowding) = in_the_box(&crowded);
        assert!(balls > 0, "the crowded run produced no Spike Ball at all");
        assert_eq!(
            crowding, 0,
            "{crowding} of {balls} Spike Balls dropped inside another one's box"
        );
        // ...and the ball's arm is the only one the crowding shuts.
        assert!(
            found(&slab, &crowded, DARK_CASTER) > 0,
            "one spike ball emptied the whole dungeon chain"
        );
    }

    /// A rainy surface day has a Flying Fish and an Umbrella Slime in it and a dry one has neither
    /// (`NPC.cs:4486-4493`).
    ///
    /// Fails before the fix, when 224 and 225 were in no pool and no branch: rain changed the spawn
    /// rate here and nothing else, so the two enemies a shower is actually *for* could not be met.
    ///
    /// The other half of the claim is the water gate, which the two arms do not share: the Flying
    /// Fish's is the one arm in the whole daytime chain written without `!waterTile`, and a flooded
    /// surface is exactly where a fish belongs.
    ///
    /// Neutralised by dropping `world.raining` from both arms: the dry assertions fail, fish and
    /// slimes turning up in clear weather. Neutralised the other way, by deleting the two
    /// `out.push` blocks: the two rain assertions fail instead.
    #[test]
    fn a_rainy_surface_day_brings_flying_fish_and_umbrella_slimes() {
        const TICKS: u32 = 200_000;
        let seen = |world: &World, at: (i32, i32), wanted: u16| {
            spawns_at(world, false, at.0, at.1, TICKS)
                .into_iter()
                .filter(|ty| *ty == wanted)
                .count()
        };

        let (dry, at) = forest_surface();
        assert!(dry.day_time, "the arm is a daytime one");
        for (npc_type, name) in [
            (FLYING_FISH, "Flying Fish"),
            (UMBRELLA_SLIME, "Umbrella Slime"),
        ] {
            assert_eq!(
                seen(&dry, at, npc_type),
                0,
                "a dry surface day offered a {name}"
            );
        }

        let mut wet_day = dry.clone();
        wet_day.raining = true;
        assert!(
            seen(&wet_day, at, FLYING_FISH) > 0,
            "a rainy surface day offered no Flying Fish"
        );
        assert!(
            seen(&wet_day, at, UMBRELLA_SLIME) > 0,
            "a rainy surface day offered no Umbrella Slime"
        );

        // `!waterTile` on the Umbrella Slime's arm and nowhere on the Flying Fish's: flood the two
        // rows `water_tile` reads and only one of the two should still turn up.
        let mut flooded = wet_day.clone();
        for x in 0..flooded.width() {
            for y in at.1..=(at.1 + 1) {
                flooded.set_tile(
                    x,
                    y,
                    terrustia_proto::Tile::AIR
                        .with_liquid(terrustia_proto::tile::Liquid::Water, 255),
                );
            }
        }
        assert!(
            seen(&flooded, at, FLYING_FISH) > 0,
            "a flooded rainy surface offered no Flying Fish, and its arm has no water gate"
        );
        assert_eq!(
            seen(&flooded, at, UMBRELLA_SLIME),
            0,
            "a flooded rainy surface offered an Umbrella Slime"
        );

        // ...and the night is a different chain entirely (`NPC.cs:4202`).
        let mut night = wet_day.clone();
        night.day_time = false;
        assert_eq!(
            seen(&night, at, UMBRELLA_SLIME),
            0,
            "a rainy surface night offered an Umbrella Slime"
        );
    }

    /// A windy surface day blows a Windy Balloon and a Dandelion toward whoever is downwind of them
    /// (`NPC.cs:4494-4501`), and a still one blows neither.
    ///
    /// Fails before the fix. The Dandelion is the one that matters: `game::ai::critter::dandelion`
    /// is a full transcription of `AI_119_Dandelion` with its own tests, it opens on
    /// `Main.IsItAHappyWindyDay`, and 628 was in no pool and no branch, so none of that could ever
    /// run in a real world. The flag it reads was not `IsItAHappyWindyDay` either, which is the
    /// other half of this fix and lives in `game::weather`.
    ///
    /// Neutralised by deleting the `out.push((DANDELION, ...))` and `out.push((WINDY_BALLOON, ...))`
    /// blocks: the two windy assertions fail. Neutralised again by dropping
    /// `events.happy_windy_day` from `open_sky`: the still-air assertions fail instead, both turning
    /// up in dead calm. Neutralised a third way, by dropping the `(px - x) * wind_target > 0.0`
    /// clause: the upwind assertion fails, a balloon arriving from the wrong side of the player.
    #[test]
    fn a_windy_surface_day_blows_a_balloon_and_a_dandelion_downwind() {
        const TICKS: u32 = 100_000;
        // Grass, because the Dandelion's arm is `tileType == 2 || tileType == 477` and the balloon
        // above it takes any ground at all. The floor is four rows from `flat_world_of`, so the
        // player stands at `floor - 2` exactly as `forest_surface` does.
        let world = flat_world_of(90, GRASS);
        let at = (400, 88);
        assert_eq!(biome_at(&world, at.0, at.1), Biome::Forest);
        assert_eq!(depth_at(&world, at.1), Depth::Surface);

        let blowing = |world: &World, wind: f32, windy: bool, wanted: u16| {
            let events = EventSpawns {
                happy_windy_day: windy,
                wind_target: wind,
                ..quiet()
            };
            spawns_with(world, &events, at.0, at.1, TICKS)
                .into_iter()
                .filter(|(ty, _)| *ty == wanted)
                .count()
        };

        // Dead calm: the latch is off and the target is zero, and neither arm opens.
        for (npc_type, name) in [(WINDY_BALLOON, "Windy Balloon"), (DANDELION, "Dandelion")] {
            assert_eq!(
                blowing(&world, 0.0, false, npc_type),
                0,
                "a still surface day offered a {name}"
            );
        }

        // A windy day, blowing east. The spawn box straddles the player, so `(px - x)` is positive
        // for every candidate west of them and negative for every one east: with the wind positive
        // the western half of the box is what answers, and both arms fire from it.
        assert!(
            blowing(&world, 0.5, true, WINDY_BALLOON) > 0,
            "a windy surface day offered no Windy Balloon"
        );
        assert!(
            blowing(&world, 0.5, true, DANDELION) > 0,
            "a windy surface day offered no Dandelion"
        );

        // The same wind speed with the latch off is still air as far as the game is concerned:
        // `IsItAHappyWindyDay` is a latch and not a speed, which is the point of the weather half of
        // this fix.
        assert_eq!(
            blowing(&world, 0.5, false, DANDELION),
            0,
            "a Dandelion turned up on a day the wind latch never came on"
        );

        // Only somebody downwind gets one. Standing the player at the far *west* edge puts every
        // candidate east of them, so `(px - x)` is negative throughout and an east wind cannot
        // reach anybody.
        let upwind = |wind: f32, wanted: u16| {
            let events = EventSpawns {
                happy_windy_day: true,
                wind_target: wind,
                ..quiet()
            };
            spawns_with(&world, &events, 40, at.1, TICKS)
                .into_iter()
                .filter(|(ty, _)| *ty == wanted)
                .count()
        };
        assert!(
            upwind(-0.5, WINDY_BALLOON) > 0,
            "a west wind reached nobody at the west edge, so the direction test is inverted"
        );
        assert_eq!(
            upwind(0.5, WINDY_BALLOON),
            0,
            "an east wind reached a player with nothing but map east of them"
        );

        // `wallType == 0`: inside a living tree is not open sky. `GetSpawnWallType` reports 244 for
        // the whole column when either neighbour row carries it (`NPC.cs:1016-1019`), so painting
        // the spawn row alone is enough to shut both arms.
        let mut indoors = world.clone();
        for x in 0..indoors.width() {
            for y in (at.1 - 4)..=(at.1 + 1) {
                let tile = indoors.tile(x, y).with_wall(LIVING_WOOD_WALL);
                indoors.set_tile(x, y, tile);
            }
        }
        for (npc_type, name) in [(WINDY_BALLOON, "Windy Balloon"), (DANDELION, "Dandelion")] {
            assert_eq!(
                blowing(&indoors, 0.5, true, npc_type),
                0,
                "a {name} grew inside a living tree"
            );
        }

        // ...and the Dandelion wants grass under it where the balloon does not.
        let stone = flat_world_of(90, 1);
        assert_eq!(
            blowing(&stone, 0.5, true, DANDELION),
            0,
            "a Dandelion grew out of bare stone"
        );
        assert!(
            blowing(&stone, 0.5, true, WINDY_BALLOON) > 0,
            "a Windy Balloon needs no particular ground and found none over stone"
        );
    }

    /// A graveyard stands a Statue Mimic on a plinth once Skeletron is down
    /// (`NPC.cs:1571-1574`), and only where there is a plinth to stand it on
    /// (`IsThisAGoodPlaceForAStatueMimic`, `NPC.cs:43891-43898`).
    ///
    /// Fails before the fix: the AI (`game::ai::mimic`) and the immortal-until-provoked pose
    /// (`IMMORTAL_TYPE`) were both already built and tested, and 690 was in no pool and no branch,
    /// so none of that work could ever run in a real world.
    ///
    /// Neutralised by deleting the `None if world.progress.downed_boss3 && seasonal.graveyard ...`
    /// arm from `try_spawn`: the first assertion fails, "a graveyard after Skeletron offered no
    /// Statue Mimic". Neutralised again by making `good_place_for_a_statue_mimic` return `true` on
    /// its first line: the half-brick assertion fails instead, a mimic standing on a floor no
    /// statue could stand on.
    #[test]
    fn a_graveyard_stands_a_statue_mimic_on_a_plinth() {
        // He is a one-in-twenty-five draw off a surface spawn rate of 600, so a short run sees none
        // of him by luck alone rather than by any rule. This one measures 37.
        const TICKS: u32 = 400_000;
        let mimics = |world: &World, graveyard: bool, (px, py): (i32, i32)| {
            spawns_at_in(world, false, graveyard, px, py, TICKS)
                .into_iter()
                .filter(|ty| *ty == STATUE_MIMIC)
                .count()
        };

        let (mut world, at) = forest_surface();
        world.progress.downed_boss3 = true;
        assert!(
            mimics(&world, true, at) > 0,
            "a graveyard after Skeletron offered no Statue Mimic"
        );

        // `downedBoss3`: the arm's first clause, so a world that has not beaten Skeletron never
        // reaches it.
        let mut fresh = world.clone();
        fresh.progress.downed_boss3 = false;
        assert_eq!(
            mimics(&fresh, true, at),
            0,
            "a graveyard before Skeletron offered a Statue Mimic"
        );

        // `ZoneGraveyard`: he is a graveyard spawn and nothing else. Ordinary ground has no arm for
        // him at any depth, at any time of day, at any progression.
        assert_eq!(
            mimics(&world, false, at),
            0,
            "an ordinary forest offered a Statue Mimic"
        );

        // `IsThisAGoodPlaceForAStatueMimic`: the floor has to read as `SolidTile2`, which a half
        // brick does not. Everything else about this world is unchanged, and `has_room` still
        // accepts the floor (it asks only whether the block type is solid), so this isolates the
        // plinth check on its own.
        let mut half_bricks = world.clone();
        for x in 0..half_bricks.width() {
            let mut tile = terrustia_proto::Tile::block(1);
            tile.flags.set(terrustia_proto::TileFlags::HALF_BRICK, true);
            half_bricks.set_tile(x, at.1 + 1, tile);
        }
        assert_eq!(
            mimics(&half_bricks, true, at),
            0,
            "a Statue Mimic stood on a floor of half bricks"
        );
    }

    /// `IsThisAGoodPlaceForAStatueMimic` (`NPC.cs:43891-43898`) on its own: two solid columns and
    /// three clear rows above both of them.
    #[test]
    fn a_statue_mimic_wants_two_solid_tiles_and_three_clear_rows() {
        let floor = 90;
        let plinth = flat_world(floor);
        assert!(good_place_for_a_statue_mimic(&plinth, 400, floor));

        // `SolidTile2(x + 1, y)`: one column of plinth is not enough, and the check is asked at
        // `x` and `x + 1` rather than either side of `x`.
        let mut gapped = plinth.clone();
        gapped.set_tile(401, floor, terrustia_proto::Tile::AIR);
        assert!(!good_place_for_a_statue_mimic(&gapped, 400, floor));
        assert!(good_place_for_a_statue_mimic(&gapped, 398, floor));

        // The three clear rows, over both columns: six tiles, each of which alone shuts it.
        for dx in 0..2 {
            for dy in 1..=3 {
                let mut blocked = plinth.clone();
                blocked.set_tile(400 + dx, floor - dy, terrustia_proto::Tile::block(1));
                assert!(
                    !good_place_for_a_statue_mimic(&blocked, 400, floor),
                    "a block at ({dx}, -{dy}) left the spot good"
                );
            }
        }

        // `SolidTile2`'s own three exclusions: a half brick, an actuated tile, and a slope. Vanilla
        // keeps exactly one kind of sloped tile, a platform hammered to a *top* slope, and this is
        // the only spawn check in the game that cares.
        let sloped = |slope: u8, block: u16, half: bool, actuated: bool| {
            let mut world = plinth.clone();
            for x in 400..402 {
                // Platforms are frame-important, so they cannot go through `Tile::block`.
                let mut tile = terrustia_proto::Tile::framed(block, 0, 0);
                tile.slope = slope;
                tile.flags.set(terrustia_proto::TileFlags::HALF_BRICK, half);
                tile.flags
                    .set(terrustia_proto::TileFlags::ACTUATED, actuated);
                world.set_tile(x, floor, tile);
            }
            good_place_for_a_statue_mimic(&world, 400, floor)
        };
        assert!(sloped(0, 1, false, false), "plain stone is a plinth");
        assert!(!sloped(0, 1, true, false), "a half brick is not a plinth");
        assert!(
            !sloped(0, 1, false, true),
            "an actuated tile is not a plinth"
        );
        assert!(!sloped(1, 1, false, false), "sloped stone is not a plinth");
        // A platform (19) is in `Main.tileSolid`, so it passes on a top slope (1 and 2) and on no
        // slope at all, and fails on a bottom slope (3 and 4).
        assert!(sloped(0, PLATFORMS[0], false, false));
        assert!(sloped(1, PLATFORMS[0], false, false));
        assert!(sloped(2, PLATFORMS[0], false, false));
        assert!(!sloped(3, PLATFORMS[0], false, false));
        assert!(!sloped(4, PLATFORMS[0], false, false));
    }

    /// Everything drawn over `ticks` attempts standing at `at`, as a set.
    fn roster_at(
        world: &World,
        hard_mode: bool,
        at: (i32, i32),
        ticks: u32,
    ) -> std::collections::BTreeSet<u16> {
        spawns_at(world, hard_mode, at.0, at.1, ticks)
            .into_iter()
            .collect()
    }

    /// The underground jungle's own two, both of which were in no pool at all.
    ///
    /// The Angry Trapper (175) is the hardmode jungle-grass arm's rooted branch
    /// (`NPC.cs:3905-3908`, inside `tileType == 60 && Main.hardMode && Main.rand.Next(3) != 0` at
    /// `:3864`), and it is reached above the surface line too: the three branches between it and
    /// the two `surfaceSpawn` ones all want `spawnTileY > Main.worldSurface`, which is the exact
    /// negation of `surfaceSpawn` (`NPC.cs:1203`).
    ///
    /// The Spiked Jungle Slime (204) is *not* a hardmode enemy. Its arm is `tileType == 60 &&
    /// spawnTileY > (Main.worldSurface + Main.rockLayer) / 2.0` (`NPC.cs:3929`) with a one-in-four
    /// roll inside it (`:3931`), and `Main.hardMode` appears in neither, so a fresh world's
    /// underground jungle has them from the first day.
    ///
    /// `game::ai::rooted` already drives the trapper, down to its own reach, pull and speed cap
    /// (`npc_params::rooted`'s `175` arm), and `game::ai::slime` already drives the slime: the
    /// spawn was the only missing half of both.
    ///
    /// Neutralised by deleting `175` from both `hardmode_pool` jungle arms: the two trapper
    /// assertions fail ("a hardmode underground jungle offered no Angry Trapper", then the surface
    /// one). Neutralised again by deleting `204` from `pool`'s `(_, Jungle)` arm: the
    /// pre-hardmode assertion fails instead.
    #[test]
    fn the_underground_jungle_has_a_roster_of_its_own() {
        const TICKS: u32 = 200_000;

        // A cavern floor of jungle grass, which is what the biome scan counts (`JUNGLE_TILES`).
        let floor = 300;
        let deep = flat_world_of(floor, 60);
        let at = (400, floor - 1);
        assert_eq!(biome_at(&deep, at.0, at.1), Biome::Jungle);
        assert_eq!(depth_at(&deep, at.1), Depth::Cavern);

        let early = roster_at(&deep, false, at, TICKS);
        assert!(
            early.contains(&204),
            "a pre-hardmode underground jungle offered no Spiked Jungle Slime"
        );
        assert!(
            !early.contains(&175),
            "a pre-hardmode jungle offered an Angry Trapper"
        );

        let late = roster_at(&deep, true, at, TICKS);
        assert!(
            late.contains(&175),
            "a hardmode underground jungle offered no Angry Trapper"
        );

        // ...and the trapper again up in the canopy, where the arm's three
        // `spawnTileY > Main.worldSurface` branches cannot fire at all.
        let surface_floor = 90;
        let top = flat_world_of(surface_floor, 60);
        let up = (400, surface_floor - 1);
        assert_eq!(biome_at(&top, up.0, up.1), Biome::Jungle);
        assert_eq!(depth_at(&top, up.1), Depth::Surface);
        let sun = roster_at(&top, true, up, TICKS);
        assert!(
            sun.contains(&175),
            "a hardmode jungle surface offered no Angry Trapper"
        );
        // 204's own arm carries a depth gate the surface never satisfies.
        assert!(
            !sun.contains(&204),
            "the jungle surface offered a Spiked Jungle Slime"
        );
    }

    /// Five hornets in eight belong to a family, and this server only ever spawned the other three.
    ///
    /// Every hornet arm in the game routes through `NPC.SpawnHornet` (`NPC.cs:5289-5354`), whose
    /// `Main.rand.Next(8)` gives cases 0 to 4 to the five families (231 to 235) and only the
    /// default to the plain 42. All five were in no pool and no branch, so a jungle hive here threw
    /// nothing but plain Hornets while a real one throws them three times in eight.
    ///
    /// Checked through `try_spawn` rather than through [`hornet_variant`] alone, because the claim
    /// worth holding is that the swap is on the path a jungle draw actually takes, and the odds are
    /// cheap enough to reach end to end: the jungle pool is five entries, so a hornet is one draw in
    /// five and each family one in forty.
    ///
    /// The two halves are asked separately because they are separate claims: that every family is
    /// reachable, and that the plain hornet is still the commonest single answer rather than being
    /// replaced by them.
    ///
    /// Neutralised by replacing `hornet_variant`'s body with `npc_type` (keeping the `rng` draw, so
    /// only the answer changes): "the jungle offered no HornetFatty (231)". Neutralised again by
    /// deleting the `_ => HORNET` arm's reachability, folding the default eighth into case 0
    /// (`family @ 0..=7 => HORNET_FATTY + family.min(4)`): the second assertion fails with "the
    /// plain Hornet (42) is gone from the jungle".
    #[test]
    fn a_jungle_hornet_belongs_to_one_of_five_families() {
        const TICKS: u32 = 200_000;
        let floor = 300;
        let deep = flat_world_of(floor, JUNGLE_GRASS);
        let at = (400, floor - 1);
        assert_eq!(biome_at(&deep, at.0, at.1), Biome::Jungle);

        let seen = roster_at(&deep, false, at, TICKS);
        for (npc_type, name) in [
            (231u16, "HornetFatty"),
            (232, "HornetHoney"),
            (233, "HornetLeafy"),
            (234, "HornetSpikey"),
            (235, "HornetStingy"),
        ] {
            assert!(
                seen.contains(&npc_type),
                "the jungle offered no {name} ({npc_type}): {seen:?}"
            );
        }
        assert!(
            seen.contains(&HORNET),
            "the plain Hornet ({HORNET}) is gone from the jungle: {seen:?}"
        );

        // ...and the split really is three eighths plain to one eighth each, not a uniform sixth or
        // a plain hornet that only slipped through on a stray seed. Asked of the swap directly, so
        // the pool's own draw does not smear the counts.
        const DRAWS: u64 = 80_000;
        let mut counts = [0u32; 6];
        for seed in 0..DRAWS {
            let mut rng = SmallRng::seed_from_u64(seed);
            match hornet_variant(HORNET, &mut rng) {
                HORNET => counts[5] += 1,
                family => counts[(family - HORNET_FATTY) as usize] += 1,
            }
        }
        for (family, hits) in counts[..5].iter().enumerate() {
            let id = HORNET_FATTY + family as u16;
            assert!(
                (9_000..11_000).contains(hits),
                "{id} came up {hits} times in {DRAWS}, not about an eighth"
            );
        }
        assert!(
            (28_000..32_000).contains(&counts[5]),
            "the plain Hornet came up {} times in {DRAWS}, not about three eighths",
            counts[5]
        );

        // Anything that is not a hornet is handed straight back, whatever the seed.
        for base in [43u16, 51, 56, 176, 204] {
            for seed in 0..1_000u64 {
                let mut rng = SmallRng::seed_from_u64(seed);
                assert_eq!(hornet_variant(base, &mut rng), base);
            }
        }
    }

    /// A corrupt cavern grows a Clinger once the wall is down (`NPC.cs:4136-4139`), and never
    /// before it.
    ///
    /// The Clinger belongs to the two evils' shared arm (`NPC.cs:4125`) rather than to the jungle:
    /// what puts one in a *jungle* is tile 661, Corrupt Jungle Grass, in that same list. Its AI is
    /// already built and parameterised, ichor shot and all (`game::ai::rooted`, and
    /// `npc_params::rooted`'s `101` arm); 101 was simply in no pool.
    ///
    /// Neutralised by deleting `101` from `hardmode_pool`'s `(Underground | Cavern, Corruption)`
    /// arm: the second assertion fails, "a hardmode corrupt cavern offered no Clinger".
    #[test]
    fn a_corrupt_cavern_grows_a_clinger_in_hardmode() {
        const TICKS: u32 = 200_000;
        let floor = 300;
        // Ebonstone, which is in the scan's `EVIL_TILES`.
        let world = flat_world_of(floor, 25);
        let at = (400, floor - 1);
        assert_eq!(biome_at(&world, at.0, at.1), Biome::Corruption);
        assert_eq!(depth_at(&world, at.1), Depth::Cavern);

        assert!(
            !roster_at(&world, false, at, TICKS).contains(&101),
            "a pre-hardmode corruption offered a Clinger"
        );
        assert!(
            roster_at(&world, true, at, TICKS).contains(&101),
            "a hardmode corrupt cavern offered no Clinger"
        );
    }

    /// The hardmode caverns cut a Rock Golem out of the stone (`NPC.cs:4921-4924`, gated by
    /// `CheckToSpawnRockGolem` at `:5803-5818`).
    ///
    /// It is a *cavern* enemy, not a jungle one: its arm is reached only once `underGround`
    /// (`spawnTileY <= Main.rockLayer`, `NPC.cs:1144`) and the underworld arm at `:4871` have both
    /// declined, and its ground is plain stone or moss rather than mud.
    ///
    /// Fails before the fix, when 631 was in no pool and no branch, so the single hardest thing an
    /// ordinary cave holds (1000 life, 85 damage) could not be met however long a world was played.
    ///
    /// Neutralised by deleting the `None if depth == Depth::Cavern && ... check_to_spawn_rock_golem`
    /// arm from `try_spawn`: the stone and moss assertions both fail ("a hardmode stone cavern cut
    /// no Rock Golem"). Neutralised again by dropping the `|| snow` clause from
    /// `check_to_spawn_rock_golem`: the snow assertion fails instead, golems coming out of a
    /// snow-bound cavern.
    #[test]
    fn the_hardmode_caverns_cut_a_rock_golem_out_of_stone_and_moss() {
        const TICKS: u32 = 200_000;
        let floor = 300;
        let golems = |world: &World, hard_mode: bool| {
            spawns_at(world, hard_mode, 400, floor - 1, TICKS)
                .into_iter()
                .filter(|ty| *ty == ROCK_GOLEM)
                .count()
        };

        let stone = flat_world_of(floor, STONE);
        assert_eq!(depth_at(&stone, floor - 1), Depth::Cavern);
        assert_eq!(biome_at(&stone, 400, floor - 1), Biome::Forest);
        assert_eq!(
            golems(&stone, false),
            0,
            "a pre-hardmode cavern cut a Rock Golem"
        );
        assert!(
            golems(&stone, true) > 0,
            "a hardmode stone cavern cut no Rock Golem"
        );

        // `TileID.Sets.Conversion.Moss` counts as stone here, and dirt does not.
        let moss = flat_world_of(floor, 179);
        assert!(
            golems(&moss, true) > 0,
            "a hardmode moss cavern cut no Rock Golem"
        );
        let dirt = flat_world_of(floor, 0);
        assert_eq!(golems(&dirt, true), 0, "a dirt floor cut a Rock Golem");

        // `ZoneSnow`. The floor stays stone, so only the zone moves: the snow goes *under* it,
        // which is the one way to change the scan's answer without changing the tile the check
        // itself reads.
        let mut snowy = stone.clone();
        for x in 0..snowy.width() {
            for y in (floor + 1)..(floor + 13) {
                snowy.set_tile(x, y, terrustia_proto::Tile::block(147));
            }
        }
        assert_eq!(biome_at(&snowy, 400, floor - 1), Biome::Snow);
        assert_eq!(golems(&snowy, true), 0, "a snowy cavern cut a Rock Golem");

        // The ceiling, which is four rows above the ground row and so is a row `open_space`'s three
        // never reach. It is made of dirt so that a candidate landing on the ceiling itself is
        // turned away by the tile gate rather than passing this one.
        let mut low = stone.clone();
        for x in 0..low.width() {
            low.set_tile(
                x,
                floor - ROCK_GOLEM_HEADROOM,
                terrustia_proto::Tile::block(0),
            );
        }
        assert_eq!(
            golems(&low, true),
            0,
            "a Rock Golem stood up under a four-row ceiling"
        );
    }

    /// `CheckToSpawnRockGolem` (`NPC.cs:5803-5818`) on its own, clause by clause.
    #[test]
    fn check_to_spawn_rock_golem_wants_stone_or_moss_under_a_clear_ceiling() {
        let floor = 300;
        let world = flat_world_of(floor, STONE);
        // One attempt in fifty, so each case is asked enough times to tell "never" from "rarely":
        // an open gate passes about eight of these four hundred draws, a shut one none.
        let passes = |world: &World, block: u16, hard_mode: bool, snow: bool| {
            let mut rng = SmallRng::seed_from_u64(631);
            (0..400)
                .filter(|_| {
                    check_to_spawn_rock_golem(world, 400, floor, block, hard_mode, snow, &mut rng)
                })
                .count()
        };

        assert!(passes(&world, STONE, true, false) > 0, "plain stone");
        assert_eq!(passes(&world, STONE, false, false), 0, "!Main.hardMode");
        assert_eq!(passes(&world, STONE, true, true), 0, "ZoneSnow");
        assert_eq!(passes(&world, 0, true, false), 0, "dirt is not stone");
        // All eleven of `TileID.Sets.Conversion.Moss` (`TileID.cs:38`).
        for moss in [179u16, 180, 181, 182, 183, 381, 534, 536, 539, 625, 627] {
            assert!(
                passes(&world, moss, true, false) > 0,
                "moss {moss} was refused"
            );
        }

        // `SolidTile(x - 1, y - 4) || SolidTile(x, y - 4) || SolidTile(x + 1, y - 4)`: any one of
        // the three shuts it, and the row is four above the ground row rather than three.
        for dx in -1..=1 {
            let mut low = world.clone();
            low.set_tile(
                400 + dx,
                floor - ROCK_GOLEM_HEADROOM,
                terrustia_proto::Tile::block(1),
            );
            assert_eq!(
                passes(&low, STONE, true, false),
                0,
                "a block at dx={dx} left the spot good"
            );
            // One row lower is not the row it reads.
            let mut lower = world.clone();
            lower.set_tile(
                400 + dx,
                floor - ROCK_GOLEM_HEADROOM - 1,
                terrustia_proto::Tile::block(1),
            );
            assert!(
                passes(&lower, STONE, true, false) > 0,
                "a block at dx={dx} five rows up shut the spot"
            );
        }
    }

    /// A spider nest is the only place either spider comes from (`NPC.cs:1662-1680`), and the arm
    /// keys on the nest's own wall rather than on a depth or a biome.
    ///
    /// Neutralised by deleting the `None if world.tile(x, y + 1).wall == SPIDER_WALL` arm from
    /// `try_spawn`: the first assertion fails with "a spider nest produced no Wall Creeper", and
    /// the hardmode one with "a hardmode nest produced no Black Recluse". Neutralised again by
    /// putting `163` back in `hardmode_pool`'s generic `(Underground | Cavern, _)` list: the last
    /// assertion fails, an ordinary walled-off hardmode cavern producing recluses.
    #[test]
    fn a_spider_nest_is_where_the_spiders_are() {
        let floor = 300;
        let nest = {
            let mut world = hall_world(floor);
            assert!(
                !terrustia_proto::housing::wall_encloses(SPIDER_WALL),
                "a nest wall must not read as a house wall, or nothing would spawn in one"
            );
            for x in 0..world.width() {
                let mut tile = terrustia_proto::Tile::block(1);
                tile.wall = SPIDER_WALL;
                world.set_tile(x, floor, tile);
            }
            world
        };
        assert_eq!(depth_at(&nest, floor - 1), Depth::Cavern);

        // Pre-hardmode the arm answers with the Wall Creeper alone (`NPC.cs:1677-1680`).
        assert!(
            spawns_of(&nest, floor, &quiet(), 164, 3) > 0,
            "a spider nest produced no Wall Creeper"
        );
        let mut hard = quiet();
        hard.hard_mode = true;
        assert!(
            spawns_of(&nest, floor, &hard, 163, 3) > 0,
            "a hardmode nest produced no Black Recluse"
        );

        // ...and the same cavern without the wall has neither, which is what makes the nest the
        // reason they came rather than the depth (`NPC.cs:1673`, the only `163` in the spawner).
        let plain = hall_world(floor);
        assert_eq!(
            spawns_of(&plain, floor, &hard, 164, 3),
            0,
            "a Wall Creeper turned up in a cavern with no nest"
        );
        assert_eq!(
            spawns_of(&plain, floor, &hard, 163, 3),
            0,
            "a Black Recluse turned up in a cavern with no nest"
        );
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
