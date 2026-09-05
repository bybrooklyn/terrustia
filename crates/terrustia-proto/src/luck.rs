//! Player luck: packet 134 (`UpdatePlayerLuckFactors`), the number it adds up to, and the two
//! rolls that read it.
//!
//! Luck is the one player statistic this server had no model of at all, and it is not cosmetic:
//! vanilla routes ambient spawn rolls, several drop rules, the money-rain roll, the falling-star
//! aim and both moon events' loot gates through `Player.RollLuck`. Every one of those was written
//! here as "reduces to `rand(n)` at zero luck, and player luck is unmodelled", which is the right
//! answer for a player with no luck buffs and the wrong one for anybody drinking a Luck Potion.
//!
//! **The server is meant to know.** Luck is computed on the client and then sent: packet 134
//! carries eight factors, and `MessageBuffer.GetData`'s case 134 (`MessageBuffer.cs:4190-4220`)
//! stores all eight on the server's copy of that player, calls `RecalculateLuck`, and rebroadcasts
//! the packet to everyone else. So the server holds a real luck value for every connected player,
//! and this module is that.
//!
//! **Two of the ten terms are not in the packet**, and vanilla's server is missing them too:
//! `usedGalaxyPearl` (+0.03) is a player-file field no packet carries, and `stinky` (-0.25) is a
//! buff. Vanilla's `RecalculateLuck` reads both off its own `Player`, where on a server they are
//! whatever the server happens to know: the pearl is never set at all, and `stinky` is real buff
//! state. So a server's luck figure differs from the client's own by up to those two terms in
//! vanilla exactly as it does here. Disclosed rather than papered over, because it means a
//! Stinkbug-cursed player is luckier in a server's rolls than in their own tooltip.
//!
//! `LanternNight.LanternsUp` (+0.3) is the opposite case: the server owns that flag outright, so
//! the caller passes it in and it is exact.

use crate::{Result, reader::PacketReader};

/// `NPC.ladyBugGoodLuckTime` and `ladyBugBadLuckTime` (`NPC.cs:6643,6645`). The bad one really is
/// stored negative in the game, which is what makes `GetLadyBugLuck`'s two branches produce a
/// signed fraction from one field: a positive timer divided by a positive constant, a negative
/// timer negated and divided by a *negative* constant.
const LADYBUG_GOOD_TIME: f32 = 43_200.0;
const LADYBUG_BAD_TIME: f32 = -10_800.0;

/// The eight luck factors a client reports, exactly as packet 134 carries them.
///
/// Kept as the factors rather than as one collapsed number because that is what the packet says
/// and what vanilla's server stores: the total is recomputed from them (`RecalculateLuck`), and
/// collapsing on arrival would make a later Lantern Night unable to change the answer.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LuckFactors {
    /// Ticks of Ladybug luck left, negative for the bad kind (a squashed one).
    pub ladybug_time_left: i32,
    /// `Player.torchLuck`, the Torch God's own contribution: how well-lit and how biome-correct
    /// the torches near the player are.
    pub torch_luck: f32,
    /// Luck Potion tier, 0 to 3.
    pub luck_potion: u8,
    pub garden_gnome_nearby: bool,
    /// A Magic Mirror broken by a Shimmer dunk: bad luck for a while.
    pub broken_mirror_bad_luck: bool,
    /// What the player's equipment adds, already summed by the client.
    pub equipment_bonus: f32,
    /// Coins thrown into Shimmer, in copper. Read through a step table, not linearly.
    pub coin_luck: f32,
    /// How many kites the player is flying, 0 to 3.
    pub kite_luck_level: u8,
}

impl LuckFactors {
    /// `payload` is a packet-134 body with the leading message id already stripped.
    ///
    /// The leading slot byte is read and discarded: `MessageBuffer` overwrites it with `whoAmI` on
    /// a server (`MessageBuffer.cs:4201-4204`) precisely so a client cannot report luck on
    /// somebody else's behalf, and the caller here already knows whose connection this arrived on.
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        r.u8()?; // slot - not trusted, see above
        Ok(Self {
            ladybug_time_left: r.i32()?,
            torch_luck: r.f32()?,
            luck_potion: r.u8()?,
            garden_gnome_nearby: r.bool()?,
            broken_mirror_bad_luck: r.bool()?,
            equipment_bonus: r.f32()?,
            coin_luck: r.f32()?,
            kite_luck_level: r.u8()?,
        })
    }

    /// `Player.RecalculateLuck` (`Player.cs:29367-29394`):
    ///
    /// ```csharp
    /// luck = GetLadyBugLuck() * 0.2f + torchLuck * 0.2f;
    /// luck += (float)(int)luckPotion * 0.1f;
    /// luck += (float)(int)kiteLuckLevel * 0.1f / 3f;
    /// if (usedGalaxyPearl) { luck += 0.03f; }
    /// if (LanternNight.LanternsUp) { luck += 0.3f; }
    /// if (HasGardenGnomeNearby) { luck += 0.2f; }
    /// if (stinky) { luck -= 0.25f; }
    /// luck += equipmentBasedLuckBonus;
    /// luck += CalculateCoinLuck();
    /// if (brokenMirrorBadLuck) { luck -= 0.25f; }
    /// ```
    ///
    /// `lanterns_up` and `stinky` are passed in because they are the server's own state rather
    /// than the client's report; `usedGalaxyPearl` is never set on a server, in the game either.
    /// See the module doc.
    ///
    /// Not clamped. `luckMinimumCap`/`luckMaximumCap` (`Player.cs:3071,3073`) exist, and the only
    /// thing that reads them is the luck *display* (`Player.cs:4098-4102`); the number the rolls
    /// use is uncapped, so this is too.
    pub fn luck(&self, lanterns_up: bool, stinky: bool) -> f32 {
        let mut luck = self.ladybug_luck() * 0.2 + self.torch_luck * 0.2;
        luck += f32::from(self.luck_potion) * 0.1;
        luck += f32::from(self.kite_luck_level) * 0.1 / 3.0;
        if lanterns_up {
            luck += 0.3;
        }
        if self.garden_gnome_nearby {
            luck += 0.2;
        }
        if stinky {
            luck -= 0.25;
        }
        luck += self.equipment_bonus;
        luck += self.coin_luck();
        if self.broken_mirror_bad_luck {
            luck -= 0.25;
        }
        luck
    }

    /// `Player.GetLadyBugLuck` (`Player.cs:18270-18281`).
    fn ladybug_luck(&self) -> f32 {
        let left = self.ladybug_time_left as f32;
        if self.ladybug_time_left > 0 {
            left / LADYBUG_GOOD_TIME
        } else if self.ladybug_time_left < 0 {
            -left / LADYBUG_BAD_TIME
        } else {
            0.0
        }
    }

    /// `Player.CalculateCoinLuck` (`Player.cs:18229-18268`): a step table, not a curve, and the
    /// steps are in copper. The `> 24900f` rung really is written twice in the game, the second
    /// unreachable; transcribed as the one rung it behaves as.
    ///
    /// Note the bottom rung: *any* coin luck at all is worth 0.025, so a single copper thrown into
    /// Shimmer is not nothing.
    fn coin_luck(&self) -> f32 {
        let coins = f64::from(self.coin_luck);
        if self.coin_luck == 0.0 {
            0.0
        } else if coins > 249_000.0 {
            0.2
        } else if coins > 24_900.0 {
            0.175
        } else if coins > 2_490.0 {
            0.15
        } else if coins > 249.0 {
            0.125
        } else if coins > 24.9 {
            0.1
        } else if coins > 2.49 {
            0.075
        } else if coins > 0.249 {
            0.05
        } else {
            0.025
        }
    }
}

/// `Luck.RollLuck(luck, range)` (`Terraria.GameContent/Luck.cs:5-16`):
///
/// ```csharp
/// if (luck > 0f && Main.rand.NextFloat() < luck) { return Main.rand.Next(Main.rand.Next(range / 2, range)); }
/// if (luck < 0f && Main.rand.NextFloat() < 0f - luck) { return Main.rand.Next(Main.rand.Next(range, range * 2)); }
/// return Main.rand.Next(range);
/// ```
///
/// Every caller tests `== 0`, so what this actually decides is a chance of `1 / range`, made
/// better or worse by narrowing or widening the range first. The *nested* `Next` is the point and
/// is easy to misread: good luck picks a new range uniformly from `[range/2, range)` and rolls in
/// that, so it is not simply "half the range" - it is a random range, which is why a big luck
/// bonus still sometimes rolls the ordinary odds.
///
/// `range / 2` is integer division, and `Next(a, b)` is exclusive of `b`, so a `range` of 1 makes
/// the good-luck branch call `Next(0, 1)` (always 0) and then `Next(0)`, which the game's own
/// `UnifiedRandom` returns 0 for. That is the same answer as the ordinary branch, so a gate already
/// at 1-in-1 cannot be improved, and this returns 0 for it rather than dividing by zero.
pub fn roll_luck(luck: f32, range: i32, rng: &mut impl LuckRng) -> i32 {
    let range = range.max(1);
    if luck > 0.0 && rng.next_f32() < luck {
        let narrowed = rng.next_range(range / 2, range);
        return rng.next_max(narrowed);
    }
    if luck < 0.0 && rng.next_f32() < -luck {
        let widened = rng.next_range(range, range * 2);
        return rng.next_max(widened);
    }
    rng.next_max(range)
}

/// `Luck.RollBadLuck(luck, range)` (`Luck.cs:18-29`), which is [`roll_luck`] with its two branches
/// swapped: good luck *widens* the range here. Used where a roll landing on zero is a bad outcome
/// for the player - the remix seed's falling stars are the one caller.
pub fn roll_bad_luck(luck: f32, range: i32, rng: &mut impl LuckRng) -> i32 {
    roll_luck(-luck, range, rng)
}

/// The three draws [`roll_luck`] needs, so it can be transcribed once and used from either crate's
/// own generator. `terrustia-proto` takes no `rand` dependency (see the root `Cargo.toml`), and
/// this is the whole of what the routine asks of one.
pub trait LuckRng {
    /// `Main.rand.NextFloat()`: nought inclusive to one exclusive.
    fn next_f32(&mut self) -> f32;
    /// `Main.rand.Next(max)`: nought to `max` exclusive, and 0 when `max <= 0`, which is what the
    /// game's own `UnifiedRandom.Next` does with a non-positive bound.
    fn next_max(&mut self, max: i32) -> i32;
    /// `Main.rand.Next(min, max)`: `min` inclusive to `max` exclusive, and `min` when the range is
    /// empty.
    fn next_range(&mut self, min: i32, max: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic stand-in: `next_f32` walks a fixed script so each branch of `roll_luck` can
    /// be entered on purpose, and the integer draws return their lower bound so the result is
    /// readable rather than random.
    struct Scripted {
        floats: Vec<f32>,
        at: usize,
        /// Every `(min, max)` `next_range` was asked for, which is what the test is really about.
        ranges: Vec<(i32, i32)>,
        maxes: Vec<i32>,
    }

    impl Scripted {
        fn new(floats: &[f32]) -> Self {
            Self {
                floats: floats.to_vec(),
                at: 0,
                ranges: Vec::new(),
                maxes: Vec::new(),
            }
        }
    }

    impl LuckRng for Scripted {
        fn next_f32(&mut self) -> f32 {
            let value = self.floats.get(self.at).copied().unwrap_or(0.5);
            self.at += 1;
            value
        }
        fn next_max(&mut self, max: i32) -> i32 {
            self.maxes.push(max);
            0
        }
        fn next_range(&mut self, min: i32, max: i32) -> i32 {
            self.ranges.push((min, max));
            min
        }
    }

    #[test]
    fn the_factors_decode_in_the_order_the_packet_writes_them() {
        // `NetMessage.cs:1565-1578`: slot, ladybug i32, torch f32, potion u8, gnome bool,
        // mirror bool, equipment f32, coins f32, kites u8.
        let mut body = vec![7u8];
        body.extend_from_slice(&43_200i32.to_le_bytes());
        body.extend_from_slice(&0.5f32.to_le_bytes());
        body.push(3);
        body.push(1);
        body.push(0);
        body.extend_from_slice(&0.125f32.to_le_bytes());
        body.extend_from_slice(&250_000.0f32.to_le_bytes());
        body.push(3);

        let factors = LuckFactors::decode(&body).expect("a well-formed body");
        assert_eq!(factors.ladybug_time_left, 43_200);
        assert_eq!(factors.torch_luck, 0.5);
        assert_eq!(factors.luck_potion, 3);
        assert!(factors.garden_gnome_nearby);
        assert!(!factors.broken_mirror_bad_luck);
        assert_eq!(factors.equipment_bonus, 0.125);
        assert_eq!(factors.coin_luck, 250_000.0);
        assert_eq!(factors.kite_luck_level, 3);
    }

    #[test]
    fn a_truncated_body_is_an_error_rather_than_a_default() {
        assert!(LuckFactors::decode(&[7, 0, 0]).is_err());
        assert!(LuckFactors::decode(&[]).is_err());
    }

    /// Every term of `RecalculateLuck`, added up by hand.
    #[test]
    fn the_total_is_every_term_the_game_adds() {
        let everything = LuckFactors {
            ladybug_time_left: 43_200, // a full timer: 1.0 * 0.2
            torch_luck: 1.0,           // 1.0 * 0.2
            luck_potion: 3,            // 3 * 0.1
            garden_gnome_nearby: true, // 0.2
            broken_mirror_bad_luck: false,
            equipment_bonus: 0.05,
            coin_luck: 250_000.0, // top rung: 0.2
            kite_luck_level: 3,   // 3 * 0.1 / 3
        };
        let expected = 0.2 + 0.2 + 0.3 + 0.1 + 0.2 + 0.05 + 0.2;
        assert!(
            (everything.luck(false, false) - expected).abs() < 1e-5,
            "got {}",
            everything.luck(false, false)
        );
        // A lantern night is worth as much as three Luck Potions, and it is the server's to add.
        assert!((everything.luck(true, false) - (expected + 0.3)).abs() < 1e-5);
        // Stinky costs the same as a broken mirror.
        assert!((everything.luck(false, true) - (expected - 0.25)).abs() < 1e-5);

        let cursed = LuckFactors {
            ladybug_time_left: -10_800, // a squashed ladybug, the whole bad timer: -1.0 * 0.2
            broken_mirror_bad_luck: true,
            ..LuckFactors::default()
        };
        assert!(
            (cursed.luck(false, false) - (-0.2 - 0.25)).abs() < 1e-5,
            "got {}",
            cursed.luck(false, false)
        );
        // Nothing at all is exactly nothing, which is the state every roll in this workspace
        // assumed before this module existed.
        assert_eq!(LuckFactors::default().luck(false, false), 0.0);
    }

    /// `CalculateCoinLuck`'s step table, rung by rung, including the bottom one: any coin luck at
    /// all is worth something.
    #[test]
    fn coin_luck_is_a_step_table_not_a_curve() {
        let at = |coins: f32| {
            LuckFactors {
                coin_luck: coins,
                ..LuckFactors::default()
            }
            .coin_luck()
        };
        assert_eq!(at(0.0), 0.0);
        assert_eq!(at(0.1), 0.025);
        assert_eq!(at(1.0), 0.05);
        assert_eq!(at(10.0), 0.075);
        assert_eq!(at(100.0), 0.1);
        assert_eq!(at(1_000.0), 0.125);
        assert_eq!(at(10_000.0), 0.15);
        assert_eq!(at(100_000.0), 0.175);
        assert_eq!(at(1_000_000.0), 0.2);
    }

    /// The nested `Next` is what luck actually does, and it is the easy thing to get wrong: good
    /// luck rolls in a range drawn from `[range/2, range)`, not in `range/2`.
    #[test]
    fn good_luck_narrows_the_range_by_drawing_a_new_one() {
        // The first float is under the luck, so the good branch is taken.
        let mut rng = Scripted::new(&[0.1]);
        assert_eq!(roll_luck(0.5, 20, &mut rng), 0);
        assert_eq!(
            rng.ranges,
            vec![(10, 20)],
            "a new range drawn from [range/2, range), not the flat range/2"
        );
        assert_eq!(rng.maxes, vec![10], "and then rolled inside that");

        // Over the luck: the ordinary branch, and the range is untouched.
        let mut rng = Scripted::new(&[0.9]);
        roll_luck(0.5, 20, &mut rng);
        assert!(rng.ranges.is_empty(), "no new range on the ordinary branch");
        assert_eq!(rng.maxes, vec![20]);

        // Bad luck widens instead, to `[range, range * 2)`.
        let mut rng = Scripted::new(&[0.1]);
        roll_luck(-0.5, 20, &mut rng);
        assert_eq!(rng.ranges, vec![(20, 40)]);

        // Zero luck never draws a float at all beyond the two guards, and never re-ranges.
        let mut rng = Scripted::new(&[0.0]);
        roll_luck(0.0, 20, &mut rng);
        assert!(rng.ranges.is_empty());
        assert_eq!(rng.maxes, vec![20]);
    }

    /// `RollBadLuck` is the same routine with its branches swapped, which is exactly negating the
    /// luck: good luck widens, bad luck narrows.
    #[test]
    fn bad_luck_is_the_mirror_of_the_ordinary_roll() {
        let mut rng = Scripted::new(&[0.1]);
        roll_bad_luck(0.5, 20, &mut rng);
        assert_eq!(rng.ranges, vec![(20, 40)], "good luck is bad here");

        let mut rng = Scripted::new(&[0.1]);
        roll_bad_luck(-0.5, 20, &mut rng);
        assert_eq!(rng.ranges, vec![(10, 20)]);
    }

    /// A gate already at 1-in-1 cannot be improved, and must not divide by zero trying.
    #[test]
    fn a_certain_roll_stays_certain_however_lucky() {
        let mut rng = Scripted::new(&[0.0]);
        assert_eq!(roll_luck(1.0, 1, &mut rng), 0);
        let mut rng = Scripted::new(&[0.0]);
        assert_eq!(roll_luck(-1.0, 1, &mut rng), 0);
        // And a nonsense range is clamped rather than panicking on `range * 2` or `range / 2`.
        let mut rng = Scripted::new(&[0.0]);
        assert_eq!(roll_luck(0.0, 0, &mut rng), 0);
        let mut rng = Scripted::new(&[0.0]);
        assert_eq!(roll_luck(0.0, -5, &mut rng), 0);
    }
}
