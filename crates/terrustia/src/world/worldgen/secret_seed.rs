//! Vanilla's real secret world seeds: magic strings (or, in two cases, specific numbers) typed
//! into the world-creation seed field that flip extra world-generation (and, in real vanilla,
//! gameplay) flags for that one world.
//!
//! ## This module was rewritten once real source became available again
//!
//! An earlier version of this module built the whole detection mechanism — correctly — from
//! *reasoned inference*: the flag names and the general "typed text is checked against magic
//! strings" mechanism were hard evidence (already found and disclosed by other worldgen passes
//! reading real `WorldGen.cs`/`Main.cs` before the decompiled tree was reaped from `.scratch/`),
//! but the *exact seven magic strings* were not — they were taken from public, well-documented
//! community knowledge (the kind on the Terraria Wiki's "World creation" article), explicitly
//! flagged at the time as lower-confidence than everything else in this file.
//!
//! The decompiled tree is back. Checked directly against
//! `Terraria.WorldBuilding/WorldSeedOption_*.cs` (each option's own `SpecialSeedNames`/
//! `SpecialSeedValues` arrays — the literal comparison real vanilla's own `WorldGenerationOptions`
//! performs) rather than trusted secondhand a second time, **every single one of the seven
//! previously-guessed strings turned out to be wrong**, one of them (`"remix"` for the Remix seed,
//! really `"dontdigup"`) not even close. A player who typed the real vanilla phrase for six of the
//! seven seeds into this server would have gotten an ordinary world, silently — the mechanism was
//! sound, the strings it matched against were not. See `plan.md`'s own note on this correction for
//! the full comparison table.
//!
//! Two more real secret seeds were also found this pass that the original seven-seed list never
//! named at all: **For the Worthy** (`fortheworthy`) — the real name behind what this project's
//! own `getGoodWorld`/`getGoodWorldGen` flag actually is, previously misattributed as an internal
//! name for "get fixed boi" rather than its own separate seed — and **Skyblock** (`skyblock`),
//! which changes generation so fundamentally (denies almost all ordinary terrain generation in
//! favour of floating sky islands) that it is closer to a different generator entirely; detected
//! and its own flag persisted, same as every other seed here, but not attempted beyond that.
//!
//! ## Not a single choice — "get fixed boi" is a real combination of the other seven
//!
//! `WorldSeedOption_Everything`'s own `Dependencies` list (Remix, Drunk, NotTheBees, NoTraps,
//! DontStarve, Anniversary, ForTheWorthy) and its `OnEnabledStateChanged` hook (`dependency.Enabled
//! = base.Enabled` for every one of them) are what "get fixed boi" really is: typing it does not
//! select an eighth, independent identity — it turns on six of the other seven flags simultaneously
//! (Skyblock is not a dependency and is unaffected), on top of its own persisted `zenithWorld` bit.
//! An earlier version of this module modelled a single `SecretSeed` enum, one variant active at a
//! time — a real design defect once this combination is accounted for, not just an incomplete
//! detail: it could not represent "get fixed boi" activating `noTrapsWorldGen` too, so a
//! `getfixedboi` world would still have generated ordinary traps. [`SecretSeeds`] below is a set of
//! independent flags for exactly this reason.
//!
//! ## What actually branches on this, and what does not (yet)
//!
//! Persisting which flags are active — through generation, into the `.wld` file, and out again to
//! every connecting client's own `WorldFlag` bits — is real, valuable, honest work on its own even
//! before every worldgen pass consumes it, the same "narrower, disclosed" shape this project's own
//! Tier 2/3 rows already use. Only [`SecretSeeds::no_traps`] is wired to an actual *generation*
//! difference so far (`traps::scatter` short-circuits to placing nothing). The other real, per-seed
//! generation differences named across this project's own already-landed worldgen passes' doc
//! comments remain out of scope — see `plan.md`'s own sizing table, now backed by real call-site
//! counts from this pass's own source check rather than the earlier estimate's guesswork.

/// Which of vanilla's real secret-seed flags a world has active — every field independent, since
/// "get fixed boi" sets six of them at once (see [`SecretSeeds::detect`]'s own doc comment). Field
/// names match this project's own already-established internal names where one already existed
/// (`get_good` for `getGoodWorld`/"For the Worthy", `everything` for `zenithWorld`/"get fixed
/// boi"), and vanilla's own `Main`-field name otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SecretSeeds {
    /// `drunkWorld` — no text trigger in real vanilla at all, only the exact numeric seed
    /// `5162020`; see [`SecretSeeds::detect`].
    pub drunk: bool,
    /// `getGoodWorld` — real vanilla's own "For the Worthy" seed (`fortheworthy`). Previously
    /// misattributed in this module as the internal name for "get fixed boi"; it is its own,
    /// separate, well-known Terraria seed, and also one of "get fixed boi"'s seven dependencies.
    pub get_good: bool,
    /// `tenthAnniversaryWorld` — Celebrationmk10. Three triggers: the string itself, or either of
    /// two exact numeric seeds (`5162021`, `5162011`).
    pub tenth_anniversary: bool,
    /// `dontStarveWorld` — four real string triggers, none of them `"don't starve"` (the earlier
    /// guess): `constant`, `theconstant`, `eye4aneye`, `eyeforaneye`.
    pub dont_starve: bool,
    /// `notTheBeesWorld` — `notthebees`, no space and no exclamation mark.
    pub not_the_bees: bool,
    /// `remixWorld` — `dontdigup`, not `"remix"`. Mirrors the whole world (surface and underworld
    /// swap, among other pipeline-wide flips) — the single largest of these by real call-site
    /// count; see `plan.md`'s sizing note.
    pub remix: bool,
    /// `noTrapsWorld` — the one flag this project actually wires to a real generation difference.
    /// See `traps.rs`.
    pub no_traps: bool,
    /// `zenithWorld` — "get fixed boi" (`getfixedboi`) itself, real vanilla's own separate
    /// persisted bit *in addition to* the six dependency flags it also sets.
    pub everything: bool,
    /// `skyblockWorld` — its own real secret seed (`skyblock`), detected and persisted like every
    /// other flag here but not attempted further: real vanilla's own generation under this flag is
    /// close to a different generator (near-total denial of ordinary terrain in favour of floating
    /// islands), not a set of branches inside the ordinary one.
    pub skyblock: bool,
}

impl SecretSeeds {
    /// No secret seed active — an ordinary world. Same as [`Default::default`], named for
    /// readability at call sites that construct one explicitly rather than deriving it.
    pub fn none() -> Self {
        Self::default()
    }

    /// Whether any flag is set.
    pub fn any(self) -> bool {
        self.drunk
            || self.get_good
            || self.tenth_anniversary
            || self.dont_starve
            || self.not_the_bees
            || self.remix
            || self.no_traps
            || self.everything
            || self.skyblock
    }

    /// Case-insensitive, trimmed match against real vanilla's own magic strings and the two
    /// numeric-only triggers, read directly from every `WorldSeedOption_*.cs`'s own
    /// `SpecialSeedNames`/`SpecialSeedValues` arrays.
    ///
    /// "get fixed boi" (`getfixedboi`) is the one case that sets more than its own flag: real
    /// vanilla's `WorldSeedOption_Everything.OnEnabledStateChanged` turns on all seven of its own
    /// `Dependencies` (Remix, Drunk, NotTheBees, NoTraps, DontStarve, Anniversary, ForTheWorthy) —
    /// Skyblock is not among them and stays off. A world generated from the literal number
    /// `5162020` gets only [`SecretSeeds::drunk`]; typing `"getfixedboi"` gets `drunk` too, but as
    /// one of six dependency flags cascading from `everything`, not from the number.
    pub fn detect(seed_text: &str) -> Self {
        let trimmed = seed_text.trim();
        let numeric = trimmed.parse::<i64>().ok();

        // Vanilla lowercases the seed and strips every non-alphanumeric character before matching a
        // special-seed name (`WorldGenerationOptions.GetOptionFromSeedText`:
        // `Regex.Replace(processedSeed.ToLower(), "[^a-z0-9]+", "")`), so the spellings a player
        // actually types — "get fixed boi", "not the bees!", "no traps", "for the worthy",
        // "don't dig up", "celebration mk10" — all normalise onto the internal names below. Matching
        // only the pre-stripped forms (as this used to) left every one of those doing nothing.
        let norm: String = trimmed
            .to_lowercase()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();

        let mut flags = Self::none();
        flags.tenth_anniversary =
            norm == "celebrationmk10" || matches!(numeric, Some(5_162_021 | 5_162_011));
        // Drunk has no special-seed *name* in vanilla — only the value 5162020 (WorldSeedOption_Drunk).
        flags.drunk = numeric == Some(5_162_020);
        flags.not_the_bees = norm == "notthebees";
        flags.remix = norm == "dontdigup";
        flags.no_traps = norm == "notraps";
        flags.get_good = norm == "fortheworthy";
        flags.dont_starve = matches!(
            norm.as_str(),
            "constant" | "theconstant" | "eye4aneye" | "eyeforaneye"
        );
        flags.skyblock = norm == "skyblock";

        if norm == "getfixedboi" {
            flags.everything = true;
            flags.remix = true;
            flags.drunk = true;
            flags.not_the_bees = true;
            flags.no_traps = true;
            flags.dont_starve = true;
            flags.tenth_anniversary = true;
            flags.get_good = true;
        }
        flags
    }

    /// The flags left after dropping the ones whose whole meaning is a world *shape* this
    /// generator does not produce. Returns them and the names of whatever was dropped.
    ///
    /// A secret-seed flag is not a label. `remixWorld` is sent to every connecting client
    /// (`World::world_data`, `F::RemixWorld`) and a real client draws and measures depth by it:
    /// the underworld background goes at the surface, the surface background goes at the bottom,
    /// and the depth readout inverts. Setting it on a world whose tiles are ordinary does not give
    /// a player a Remix world. It gives them an ordinary world drawn upside down, which is worse
    /// than not offering the seed at all - and it also writes the claim into the `.wld`, so real
    /// Terraria would believe it too.
    ///
    /// `remixWorld` is 85 call sites in vanilla and zero of them are consumed here; nothing mirrors
    /// the world. `zenithWorld` goes with it because zenith *is* the combination, remix included,
    /// and a world file claiming zenith without remix is a state neither game can make. The other
    /// six flags "get fixed boi" turns on are unaffected: each of those is a difference in what
    /// generates or how something behaves, partly modelled and disclosed at its own site, not a
    /// claim about which way up the world is.
    ///
    /// **A `.wld` that real Terraria generated keeps its flag**, because there the tiles really are
    /// mirrored and the client is right to be told. This only governs what this generator claims
    /// about worlds it made itself.
    pub fn honoured_by_this_generator(mut self) -> (Self, Vec<&'static str>) {
        let mut dropped = Vec::new();
        if self.remix {
            self.remix = false;
            dropped.push("remix");
        }
        if self.everything {
            self.everything = false;
            dropped.push("get fixed boi");
        }
        (self, dropped)
    }

    /// Every active flag's own display name, for logging and the startup panel — real vanilla's
    /// own seed name where the seed has one commonly-known name, the internal flag name otherwise.
    /// Empty for an ordinary world.
    pub fn active_names(self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.everything {
            names.push("get fixed boi");
        }
        if self.remix {
            names.push("remix");
        }
        if self.drunk {
            names.push("drunk world");
        }
        if self.not_the_bees {
            names.push("not the bees");
        }
        if self.no_traps {
            names.push("no traps");
        }
        if self.dont_starve {
            names.push("don't starve");
        }
        if self.tenth_anniversary {
            names.push("celebrationmk10");
        }
        if self.get_good {
            names.push("for the worthy");
        }
        if self.skyblock {
            names.push("skyblock");
        }
        names
    }
}

/// Turn a typed seed string into the numeric seed [`super::rand::UnifiedRandom`] actually needs.
///
/// Real vanilla: parse the string as a number if it is one; otherwise hash it. This project's own
/// generator is already not seed-identical with vanilla's (see `worldgen/mod.rs`'s own module
/// doc), so the exact hash algorithm does not need to match vanilla's — only "the same text always
/// produces the same numeric seed" has to hold, which is what makes typing a word seed twice
/// reproduce the same world, the same property `worldgen::tests::a_seed_makes_the_same_world`
/// already checks for plain numeric seeds. A hand-rolled FNV-1a rather than `std`'s own
/// `DefaultHasher`, deliberately: `DefaultHasher`'s algorithm is not a stability guarantee across
/// std versions, and this project already leans on a pinned toolchain elsewhere for its own
/// reproducibility guarantees (see `rust-toolchain.toml`) — a dependency this ten-line function
/// does not need to add.
pub fn numeric_seed(seed_text: &str) -> u64 {
    let trimmed = seed_text.trim();
    if let Ok(n) = trimmed.parse::<u64>() {
        return n;
    }
    // FNV-1a, 64-bit.
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for byte in trimmed.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    type FlagCheck = fn(SecretSeeds) -> bool;

    #[test]
    fn every_real_magic_string_sets_exactly_its_own_flag() {
        let cases: &[(&str, FlagCheck)] = &[
            ("celebrationmk10", |f| f.tenth_anniversary),
            ("notthebees", |f| f.not_the_bees),
            ("dontdigup", |f| f.remix),
            ("notraps", |f| f.no_traps),
            ("fortheworthy", |f| f.get_good),
            ("constant", |f| f.dont_starve),
            ("theconstant", |f| f.dont_starve),
            ("eye4aneye", |f| f.dont_starve),
            ("eyeforaneye", |f| f.dont_starve),
            ("skyblock", |f| f.skyblock),
        ];
        for (text, own_flag) in cases {
            let flags = SecretSeeds::detect(text);
            assert!(own_flag(flags), "{text:?} should set its own flag");
            assert_eq!(
                flags.active_names().len(),
                1,
                "{text:?} should set exactly one flag, got {flags:?}"
            );
        }
    }

    #[test]
    fn strings_that_are_not_real_name_triggers_do_not_match() {
        // These read like seed names but are not among vanilla's `SpecialSeedNames`: Drunk has no
        // name at all (only the value 5162020), Remix's name is "dontdigup", and Don't Starve's are
        // "constant"/"theconstant"/"eye4aneye"/"eyeforaneye" — never the game's own title.
        for text in ["drunk world", "remix", "don't starve", "terraria"] {
            assert!(
                !SecretSeeds::detect(text).any(),
                "{text:?} is not a vanilla name trigger and must not match"
            );
        }
    }

    /// The spellings a player actually types must activate, exactly as vanilla's
    /// `Regex.Replace(ToLower, "[^a-z0-9]+", "")` normalisation makes them. This is the case the
    /// old test got backwards — it asserted "not the bees!", "no traps" and "get fixed boi" must
    /// *not* match, enshrining the bug where every punctuated/spaced spelling did nothing.
    #[test]
    fn real_spellings_with_spaces_and_punctuation_activate() {
        assert!(SecretSeeds::detect("not the bees!").not_the_bees);
        assert!(SecretSeeds::detect("No Traps").no_traps);
        assert!(SecretSeeds::detect("for the worthy").get_good);
        assert!(SecretSeeds::detect("don't dig up").remix);
        assert!(SecretSeeds::detect("celebration mk10").tenth_anniversary);
        assert!(SecretSeeds::detect("the constant").dont_starve);
        assert!(SecretSeeds::detect("eye for an eye").dont_starve);
        // "get fixed boi" cascades into the full Everything set.
        let fixed = SecretSeeds::detect("get fixed boi");
        assert!(fixed.everything && fixed.no_traps && fixed.not_the_bees);
    }

    #[test]
    fn drunk_world_has_no_text_trigger_only_a_number() {
        assert!(!SecretSeeds::detect("drunk").any());
        assert!(!SecretSeeds::detect("drunk world").any());
    }

    #[test]
    fn the_two_numeric_only_triggers_work() {
        assert!(SecretSeeds::detect("5162020").drunk);
        assert!(SecretSeeds::detect("5162021").tenth_anniversary);
        assert!(SecretSeeds::detect("5162011").tenth_anniversary);
        // Neighbouring numbers must not match.
        assert!(!SecretSeeds::detect("5162019").any());
        assert!(!SecretSeeds::detect("5162022").any());
    }

    #[test]
    fn get_fixed_boi_turns_on_six_dependencies_and_itself_but_not_skyblock() {
        let flags = SecretSeeds::detect("getfixedboi");
        assert!(flags.everything);
        assert!(flags.remix);
        assert!(flags.drunk);
        assert!(flags.not_the_bees);
        assert!(flags.no_traps);
        assert!(flags.dont_starve);
        assert!(flags.tenth_anniversary);
        assert!(flags.get_good);
        assert!(
            !flags.skyblock,
            "Skyblock is not one of Everything's own Dependencies"
        );
    }

    #[test]
    fn detection_is_case_insensitive_and_trims_whitespace() {
        assert!(SecretSeeds::detect("  GETFIXEDBOI  ").everything);
        assert!(SecretSeeds::detect("NoTraps").no_traps);
        assert!(SecretSeeds::detect("DontDigUp").remix);
    }

    #[test]
    fn ordinary_seeds_do_not_match() {
        for text in ["", "12345", "my world", "traps", "boi", "fixed"] {
            assert!(
                !SecretSeeds::detect(text).any(),
                "{text:?} should not match anything"
            );
        }
    }

    #[test]
    fn a_plain_number_parses_as_itself() {
        assert_eq!(numeric_seed("12345"), 12345);
        assert_eq!(numeric_seed("  9  "), 9);
        assert_eq!(numeric_seed("0"), 0);
    }

    #[test]
    fn word_seeds_hash_deterministically_and_differ() {
        let a = numeric_seed("getfixedboi");
        let b = numeric_seed("getfixedboi");
        let c = numeric_seed("dontdigup");
        assert_eq!(a, b, "the same text must hash to the same seed twice");
        assert_ne!(
            a, c,
            "different text should (almost always) hash differently"
        );
    }
}
