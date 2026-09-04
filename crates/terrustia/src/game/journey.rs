//! Journey (creative) mode powers — the state this server actually understands the effect of.
//!
//! Vanilla has 15 powers (`CreativePowerManager.cs:90-104`), in five different wire shapes. This
//! covers all five, all fifteen powers: the four one-shot buttons (day/noon/night/midnight,
//! handled entirely in `server.rs` — nothing to hold state for), the four shared on/off toggles
//! this struct holds (`FreezeTime`/`FreezeRain`/`FreezeWind`/`StopBiomeSpread`), the three shared
//! sliders (`time_rate`/`difficulty` need state here; `ModifyWind`/`ModifyRain` do not, see below),
//! and the three per-player powers (`godmode`/`far_placement_range`/`spawn_rate_slider`, one array
//! entry per player slot, 0..255).
//!
//! `ModifyWindDirectionAndStrength`/`ModifyRainPower` are `_syncToJoiningPlayers = false` in
//! source (unlike `ModifyTimeRate`'s own `true`) *and* neither implements
//! `IPersistentPerWorldContent` — real vanilla itself does not remember either past the moment
//! they're applied, so `server.rs`'s handler applies their effect straight to `Weather` and moves
//! on, nothing to hold onto here either.
//!
//! `Difficulty` (`DifficultySliderPower`) is the interesting one: real vanilla does not touch
//! `world.game_mode` (`Main.GameMode`) at all. Instead `Main.Difficulty` — the single float every
//! difficulty-scaled system in source actually reads, with `expertMode`/`masterMode` themselves
//! just `Difficulty >= 2`/`>= 3` — is overridden to this slider's own continuous value whenever
//! `IsJourneyMode` (`Main.cs`'s `UpdateCreativeGameModeOverride`), falling back to the discrete
//! `GameMode`-derived value otherwise. [`difficulty_multiplier`](Self::difficulty_multiplier) is
//! that continuous value; `server.rs`'s own `effective_difficulty()` is the fallback-aware
//! accessor every call site now goes through instead of reading `world.game_mode` directly.
//!
//! **The six shared fields persist; the three per-player ones deliberately do not.** Every
//! *shared* field this struct holds (`freeze_time`/`freeze_rain`/`freeze_wind`/
//! `stop_biome_spread`/`time_rate_slider`/`difficulty_slider`) is `IPersistentPerWorldContent` in
//! real vanilla, and now round-trips through the `.wld` file's own creative-powers section
//! (`wld::read_journey_powers`/`wld_save::write_journey_powers`), mirrored onto `world.journey_*`
//! by `GameServer::record_journey_powers` before every save and back by
//! `GameServer::restore_journey_powers` at startup — the same shape `town_npcs` and the Lunar
//! Pillars' own `saved_npcs` use to cross from live server state to world state and back.
//!
//! The three *per-player* fields are a different, genuinely correct kind of session-only: real
//! vanilla is `IPersistentPerPlayerContent` for these, saved into each player's own `.plr` file —
//! this project doesn't own player files at all (client-authoritative, matching every other
//! per-character setting), so "resets when the player reconnects" is not a gap relative to vanilla
//! at all, it is the honest shape of not being the thing that owns that file.

use terrustia_proto::net_module::power;

/// How many player slots a per-player power array holds — one entry per possible connection slot,
/// matching `_perPlayerIsEnabled`'s own fixed `bool[255]` in source.
const MAX_PLAYERS: usize = 255;

/// The shared on/off powers, `ModifyTimeRate`, and the three per-player powers whose gameplay
/// effect this server applies. This used to point at the module doc "for which one power this does
/// *not* cover yet"; there is no such power, and the module doc says so itself ("all five, all
/// fifteen"). The pointer went stale when `stop_biome_spread` grew its real gate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JourneyPowers {
    pub freeze_time: bool,
    pub freeze_rain: bool,
    pub freeze_wind: bool,
    /// Freezes the hardmode infections where they stand, and really does: `tick_world_update` reads
    /// this flag into its own `spreading` gate beside `hard_mode`
    /// (`game/server/systems.rs`, transcribing `AllowedToSpreadInfections`,
    /// `WorldGen.cs:72047-72052`), and `with_the_power_on_nothing_spreads` holds fifty ticks of a
    /// hardmode world to zero drift with it on.
    ///
    /// This comment used to say the opposite: that the power had "nothing to gate yet" because this
    /// project "does not model corruption/crimson/hallow tile spread at all". Both halves were false
    /// by the time anyone read them, and it is recorded here rather than quietly deleted because a
    /// doc comment that libels working code is the same failure as one that oversells missing code.
    pub stop_biome_spread: bool,
    /// `ModifyTimeRate`'s raw 0.0–1.0 slider position, exactly as the wire carries it and exactly
    /// as `_sliderCurrentValueCache` holds it in source — not the derived 1×–24× rate itself.
    /// Storing the raw value, not the computed rate, is what makes re-broadcasting it to a late
    /// joiner (`ASharedSliderPower::OnPlayerJoining` writes the cache, not the rate) exact rather
    /// than an inverse-remap guess. Default `0.0`, which [`time_rate`](Self::time_rate) resolves
    /// to `1` — matching `ModifyTimeRate::Reset`'s own explicit `TargetTimeRate = 1` alongside its
    /// `_sliderCurrentValueCache = 0f`, not a coincidence: `Remap(0, 0, 1, 1, 24)` is already `1`.
    pub time_rate_slider: f32,
    /// `DifficultySlider`'s raw 0.0–1.0 slider position, exactly as the wire carries it and
    /// exactly as `_sliderCurrentValueCache` holds it in source. Default `0.0`, which
    /// [`difficulty_multiplier`](Self::difficulty_multiplier) resolves to `0.5` — matching
    /// `DifficultySliderPower::Reset`'s own `_sliderCurrentValueCache = 0f`, which lands on the
    /// exact same value a Journey world already had before this power existed
    /// (`terrustia_proto::difficulty::of_game_mode(3)`), so an untouched slider changes nothing.
    pub difficulty_slider: f32,
    /// Godmode, one entry per player slot. Real vanilla immunity (`Player.cs`'s own
    /// `creativeGodMode` reads) is client-side and this project does not re-derive it — this array
    /// exists so the *server's own* authoritative damage path (`hurt_player`) can skip applying
    /// damage to a player who has it on, which a purely client-side flag could never do for
    /// server-dealt hits (traps, NPC contact damage resolved server-side, and so on).
    pub godmode: [bool; MAX_PLAYERS],
    /// `FarPlacementRange`, one entry per player slot.
    pub far_placement_range: [bool; MAX_PLAYERS],
    /// `SpawnRate`'s raw per-player slider value. Defaults to `0.5`, not `0.0` —
    /// `_sliderDefaultValue = 0.5f` in source, which [`spawn_rate_multiplier`](Self::
    /// spawn_rate_multiplier) resolves to the ordinary `1.0` multiplier, not "no spawns."
    pub spawn_rate_slider: [f32; MAX_PLAYERS],
}

impl Default for JourneyPowers {
    fn default() -> Self {
        Self {
            freeze_time: false,
            freeze_rain: false,
            freeze_wind: false,
            stop_biome_spread: false,
            time_rate_slider: 0.0,
            difficulty_slider: 0.0,
            godmode: [false; MAX_PLAYERS],
            far_placement_range: [false; MAX_PLAYERS],
            // `#[derive(Default)]` would fill this with `f32::default()` (0.0) per slot, which is
            // "no spawns at all" — wrong for a slot nobody has touched yet. A hand-written impl is
            // the only way to get vanilla's own real default (0.5, "ordinary spawn rate") instead.
            spawn_rate_slider: [0.5; MAX_PLAYERS],
        }
    }
}

impl JourneyPowers {
    /// The current state of one of the four modelled shared toggles, or `None` for any other
    /// power id (including every power this doesn't cover — see the module doc).
    pub fn get(&self, power_id: u16) -> Option<bool> {
        match power_id {
            power::FREEZE_TIME => Some(self.freeze_time),
            power::FREEZE_RAIN => Some(self.freeze_rain),
            power::FREEZE_WIND => Some(self.freeze_wind),
            power::STOP_BIOME_SPREAD => Some(self.stop_biome_spread),
            _ => None,
        }
    }

    /// Apply a toggle request. Returns whether `power_id` named one of the four this struct holds
    /// — `false` means nothing changed, the caller should not broadcast or persist anything.
    pub fn set(&mut self, power_id: u16, enabled: bool) -> bool {
        match power_id {
            power::FREEZE_TIME => self.freeze_time = enabled,
            power::FREEZE_RAIN => self.freeze_rain = enabled,
            power::FREEZE_WIND => self.freeze_wind = enabled,
            power::STOP_BIOME_SPREAD => self.stop_biome_spread = enabled,
            _ => return false,
        }
        true
    }

    /// `ModifyTimeRate::UpdateInfoFromSliderValueCache`'s own remap: `Utils.Remap(value, 0, 1, 1,
    /// 24)`, rounded to the nearest whole multiplier. The clock advances this many ticks per
    /// server tick instead of one — `World::tick_time`'s own `rate` parameter.
    pub fn time_rate(&self) -> i32 {
        (1.0 + self.time_rate_slider.clamp(0.0, 1.0) * 23.0).round() as i32
    }

    /// `DifficultySliderPower::UpdateInfoFromSliderValueCache`'s own two-segment remap: 0.0–0.33
    /// maps to 0.5×–1×, 0.33–1.0 maps to 1×–3×, rounded to the nearest 0.05 (`Math.Round(x * 20) /
    /// 20`, source's own rounding, not this project's invention). This is the same "difficulty"
    /// number `terrustia_proto::difficulty::of_game_mode` produces discretely (0.5 journey, 1
    /// classic, 2 expert, 3 master) — the slider just samples it continuously instead of at four
    /// fixed points. Only meaningful in a Journey world; `server.rs`'s `effective_difficulty()`
    /// decides when to call this instead of falling back to the discrete value.
    pub fn difficulty_multiplier(&self) -> f32 {
        let value = self.difficulty_slider.clamp(0.0, 1.0);
        let raw = if value <= 0.33 {
            0.5 + (value / 0.33) * 0.5
        } else {
            1.0 + ((value - 0.33) / 0.67) * 2.0
        };
        (raw * 20.0).round() / 20.0
    }

    /// Godmode's current state for one player slot. Out-of-range reads as `false` — nothing this
    /// server does can put a real player past slot 254, but a bad index from elsewhere should
    /// never panic over a vanity check like this one.
    pub fn is_godmode(&self, slot: u8) -> bool {
        self.godmode.get(slot as usize).copied().unwrap_or(false)
    }

    /// Set Godmode for one player slot, in place — real vanilla's own `SetEnabledState` mutates
    /// its array the same way, no separate "did this change" bookkeeping.
    pub fn set_godmode(&mut self, slot: u8, enabled: bool) {
        if let Some(entry) = self.godmode.get_mut(slot as usize) {
            *entry = enabled;
        }
    }

    pub fn has_far_placement_range(&self, slot: u8) -> bool {
        self.far_placement_range
            .get(slot as usize)
            .copied()
            .unwrap_or(false)
    }

    pub fn set_far_placement_range(&mut self, slot: u8, enabled: bool) {
        if let Some(entry) = self.far_placement_range.get_mut(slot as usize) {
            *entry = enabled;
        }
    }

    /// Set one player's raw `SpawnRate` slider value — clamped, the same guard
    /// `PushChangeAndSetSlider`'s own `MathHelper.Clamp(value, 0f, 1f)` applies before this value
    /// is ever cached, since nothing on the wire itself stops an out-of-range value arriving.
    pub fn set_spawn_rate_slider(&mut self, slot: u8, value: f32) {
        if let Some(entry) = self.spawn_rate_slider.get_mut(slot as usize) {
            *entry = value.clamp(0.0, 1.0);
        }
    }

    /// Whether spawns should be disabled outright for this player —
    /// `GetShouldDisableSpawnsFor`'s own exact condition, the slider sitting at its literal floor
    /// rather than merely low. `spawn_rate_multiplier` alone cannot express this: its own remap
    /// never reaches `0.0`, bottoming out at `0.1×` instead.
    pub fn spawns_disabled(&self, slot: u8) -> bool {
        self.spawn_rate_slider
            .get(slot as usize)
            .is_some_and(|&v| v == 0.0)
    }

    /// `SpawnRateSliderPerPlayerPower::RemapSliderValueToPowerValue`'s own two-segment remap:
    /// 0.0–0.5 maps to 0.1×–1×, 0.5–1.0 maps to 1×–10× — the default `0.5` lands exactly on the
    /// seam, `1×`, "ordinary." Callers that need the "hard off at 0.0" case should check
    /// [`spawns_disabled`](Self::spawns_disabled) first: this remap alone never reaches `0.0`.
    pub fn spawn_rate_multiplier(&self, slot: u8) -> f32 {
        let value = self
            .spawn_rate_slider
            .get(slot as usize)
            .copied()
            .unwrap_or(0.5)
            .clamp(0.0, 1.0);
        if value < 0.5 {
            0.1 + (value / 0.5) * 0.9
        } else {
            1.0 + ((value - 0.5) / 0.5) * 9.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_modelled_power_starts_off() {
        let powers = JourneyPowers::default();
        for id in [
            power::FREEZE_TIME,
            power::FREEZE_RAIN,
            power::FREEZE_WIND,
            power::STOP_BIOME_SPREAD,
        ] {
            assert_eq!(powers.get(id), Some(false));
        }
    }

    #[test]
    fn set_reports_whether_it_recognised_the_power_id() {
        let mut powers = JourneyPowers::default();
        assert!(powers.set(power::FREEZE_TIME, true));
        assert_eq!(powers.get(power::FREEZE_TIME), Some(true));

        assert!(!powers.set(power::GODMODE, true), "not one of the four");
        assert_eq!(
            powers.get(power::GODMODE),
            None,
            "and get() should agree it holds nothing for it"
        );
    }

    #[test]
    fn toggles_are_independent() {
        let mut powers = JourneyPowers::default();
        powers.set(power::FREEZE_WIND, true);
        assert_eq!(powers.get(power::FREEZE_TIME), Some(false));
        assert_eq!(powers.get(power::FREEZE_RAIN), Some(false));
        assert_eq!(powers.get(power::FREEZE_WIND), Some(true));
        assert_eq!(powers.get(power::STOP_BIOME_SPREAD), Some(false));
    }

    #[test]
    fn a_fresh_journey_state_ticks_time_at_one_times() {
        assert_eq!(JourneyPowers::default().time_rate(), 1);
    }

    #[test]
    fn the_time_rate_slider_remaps_across_its_whole_one_to_twenty_four_times_range() {
        let mut powers = JourneyPowers {
            time_rate_slider: 1.0,
            ..Default::default()
        };
        assert_eq!(powers.time_rate(), 24, "the top of the slider is 24x");

        powers.time_rate_slider = 0.5;
        assert_eq!(
            powers.time_rate(),
            13,
            "the midpoint rounds 12.5x to 13x, matching Math.Round's own away-from-zero rounding"
        );
    }

    /// A slider value outside 0.0–1.0 should never be trusted at face value — nothing on the wire
    /// stops a client (or a bug upstream of this call) from sending one.
    #[test]
    fn an_out_of_range_slider_value_is_clamped_rather_than_extrapolated() {
        let mut powers = JourneyPowers {
            time_rate_slider: 5.0,
            ..Default::default()
        };
        assert_eq!(powers.time_rate(), 24, "clamped to the slider's real top");

        powers.time_rate_slider = -3.0;
        assert_eq!(powers.time_rate(), 1, "clamped to the slider's real bottom");
    }

    #[test]
    fn a_fresh_journey_state_reproduces_journeys_own_old_fixed_difficulty() {
        // Default `difficulty_slider` is 0.0 — `DifficultySliderPower::Reset`'s own value — which
        // must land on exactly `of_game_mode(3)` (0.5), the value a Journey world had before this
        // power existed at all, so an untouched slider changes nothing.
        assert_eq!(JourneyPowers::default().difficulty_multiplier(), 0.5);
    }

    #[test]
    fn the_difficulty_slider_hits_its_four_named_presets() {
        let at = |value: f32| {
            JourneyPowers {
                difficulty_slider: value,
                ..Default::default()
            }
            .difficulty_multiplier()
        };
        assert_eq!(at(0.0), 0.5, "the Journey click position");
        assert_eq!(at(0.33), 1.0, "the Normal/classic click position");
        assert_eq!(at(1.0), 3.0, "the Master click position");
        // Source's own Expert click position is 0.66, not the exact 2/3 that would put it on the
        // segment boundary — it still lands exactly on 2.0 once rounded to the nearest 0.05.
        assert_eq!(at(0.66), 2.0, "the Expert click position");
    }

    #[test]
    fn the_difficulty_slider_interpolates_continuously_between_its_presets() {
        let at = |value: f32| {
            JourneyPowers {
                difficulty_slider: value,
                ..Default::default()
            }
            .difficulty_multiplier()
        };
        // A value strictly between two presets must land strictly between their multipliers —
        // proof this is a real continuous remap, not four buckets in a trench coat.
        assert!(at(0.1) > at(0.0) && at(0.1) < at(0.33));
        assert!(at(0.5) > at(0.33) && at(0.5) < at(1.0));
    }

    #[test]
    fn the_difficulty_slider_rounds_to_the_nearest_twentieth_like_source() {
        // `Math.Round(x * 20) / 20` in source. At 0.15 the unrounded remap is 0.727_27..., which
        // is not itself on the 0.05 grid — snapping it to 0.75 only happens if the rounding step
        // actually runs, not just carried-through floating-point division noise.
        let powers = JourneyPowers {
            difficulty_slider: 0.15,
            ..Default::default()
        };
        assert_eq!(powers.difficulty_multiplier(), 0.75);
    }

    #[test]
    fn a_difficulty_slider_value_outside_0_1_is_clamped_rather_than_extrapolated() {
        let at = |value: f32| {
            JourneyPowers {
                difficulty_slider: value,
                ..Default::default()
            }
            .difficulty_multiplier()
        };
        assert_eq!(at(5.0), 3.0, "clamped to the slider's real top");
        assert_eq!(at(-3.0), 0.5, "clamped to the slider's real bottom");
    }

    #[test]
    fn godmode_and_far_placement_range_are_off_and_independent_per_player() {
        let mut powers = JourneyPowers::default();
        assert!(!powers.is_godmode(3));
        assert!(!powers.has_far_placement_range(3));

        powers.set_godmode(3, true);
        assert!(powers.is_godmode(3), "slot 3 should now have it");
        assert!(!powers.is_godmode(4), "a different slot should not");
        assert!(
            !powers.has_far_placement_range(3),
            "the other per-player toggle should be untouched"
        );
    }

    #[test]
    fn an_out_of_range_slot_reads_as_off_rather_than_panicking() {
        let powers = JourneyPowers::default();
        assert!(!powers.is_godmode(254));
        // 255 does not exist in a 255-entry, 0-indexed array — the real edge this project's own
        // player slots never reach, but a bad caller elsewhere should never panic over it.
        assert!(!powers.is_godmode(255));
    }

    #[test]
    fn setting_an_out_of_range_slot_does_not_panic() {
        let mut powers = JourneyPowers::default();
        powers.set_godmode(255, true);
        powers.set_far_placement_range(255, true);
        powers.set_spawn_rate_slider(255, 1.0);
    }

    #[test]
    fn a_fresh_spawn_rate_slider_is_the_ordinary_one_times_multiplier() {
        let powers = JourneyPowers::default();
        assert_eq!(powers.spawn_rate_multiplier(9), 1.0);
        assert!(!powers.spawns_disabled(9));
    }

    #[test]
    fn the_spawn_rate_slider_remaps_across_its_two_segments() {
        let mut powers = JourneyPowers::default();
        powers.set_spawn_rate_slider(1, 1.0);
        assert_eq!(powers.spawn_rate_multiplier(1), 10.0, "the top: 10x");

        powers.set_spawn_rate_slider(1, 0.0);
        assert!(
            (powers.spawn_rate_multiplier(1) - 0.1).abs() < 1e-6,
            "the bottom of the remap is 0.1x, got {}",
            powers.spawn_rate_multiplier(1)
        );
        assert!(
            powers.spawns_disabled(1),
            "but 0.0 is also the hard 'no spawns at all' floor, on top of the 0.1x remap"
        );
    }

    #[test]
    fn the_spawn_rate_slider_clamps_out_of_range_values() {
        let mut powers = JourneyPowers::default();
        powers.set_spawn_rate_slider(1, 5.0);
        assert_eq!(
            powers.spawn_rate_multiplier(1),
            10.0,
            "clamped to the real top"
        );

        powers.set_spawn_rate_slider(1, -5.0);
        assert!(
            powers.spawns_disabled(1),
            "clamped to 0.0, which is also the hard floor"
        );
    }

    #[test]
    fn spawn_rate_sliders_are_independent_per_player() {
        let mut powers = JourneyPowers::default();
        powers.set_spawn_rate_slider(2, 0.0);
        assert!(powers.spawns_disabled(2));
        assert!(
            !powers.spawns_disabled(3),
            "a different, untouched slot should still be at the ordinary default"
        );
        assert_eq!(powers.spawn_rate_multiplier(3), 1.0);
    }
}
