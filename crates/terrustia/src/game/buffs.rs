//! Buffs and debuffs on an NPC: the twenty slots, what they set, and what they cost.
//!
//! Almost every weapon in the game past the first hour inflicts something. On Fire!, Ichor,
//! Venom, Betsy's Curse, Daybreak — these are not decoration; a good fraction of a late-game
//! player's damage arrives this way, and the ones that lower armour change what every *other*
//! hit is worth. A server that drops them makes the whole second half of the game feel wrong
//! without ever looking broken.
//!
//! Three separate things happen here, and the game keeps them separate too:
//!
//! * **The slots.** Twenty of them, filled by [`Buffs::add`]. When they are full a good buff can
//!   be pushed out to make room for another; a debuff never can, which is why you cannot clear a
//!   boss's debuffs by stacking blessings on it.
//! * **The flags.** Each tick the slots are read into a set of booleans and every timer counts
//!   down. The flags are what everything else reads — the damage-over-time tally here, and the
//!   client, which is told the slots and derives the same flags to work out armour penetration.
//! * **The toll.** [`Buffs::dots`] ports `NPC.DOTTally`: each active debuff contributes to a
//!   life-regeneration figure, that figure accumulates, and every time the accumulator crosses a
//!   threshold the NPC loses a hit point. This is why poison ticks are irregular rather than
//!   once a second.
//!
//! The armour-lowering debuffs — ichor, broken armour, Betsy's curse — deliberately have no
//! effect on this server's own damage arithmetic, and that is correct rather than missing. The
//! whole of their effect is `NPC.checkArmorPenetration` (`NPC.cs:81972-81990`, +15/+20/+40), and
//! it has exactly three callers, all of which run on the hitting *client* and never on a server:
//!
//! * `Player.cs:44763`, item melee: the result is added to `num3` and then handed to
//!   `StrikeNPC`, which sends `num3` on the wire (`NPC.cs:82046`, `SendData(28, ...)`) *before*
//!   applying defence, so the penetration is already inside the damage the server receives.
//! * `Projectile.cs:13686`, guarded by `ownedBySomeone && !hostile` and by
//!   `Invariant.Assert(Main.netMode == 0 || owner == Main.myPlayer)` twelve lines below: a
//!   projectile only ever deals its damage on the machine that owns it.
//! * `Player.cs:20602`, `ApplyDamageToNPC`, reached only through a `whoAmI == Main.myPlayer`
//!   guard (`Player.cs:20582`).
//!
//! So this server's two `damage_taken` sites are right to apply plain defence: the one fed by
//! packet 28 (`dispatch.rs`) is handed a figure that already includes the client's penetration,
//! exactly as vanilla's own `fromNet: true` path is, and the other (an enemy hitting a
//! townsperson) has no player weapon in it at all. What the server owes the client is the *buff
//! list*, so its penetration is computed against the truth. Re-verified against the decompiled
//! tree on 2026-08-31.

use terrustia_proto::buffs::{is_debuff, npc_is_immune};

/// How many buffs one NPC can carry at once. `NPC.maxBuffs`.
pub const MAX_BUFFS: usize = 20;

/// The life-regeneration accumulator's threshold, in the game's units.
///
/// One point of damage is dealt each time the accumulator falls this far, so a debuff worth 12
/// regeneration deals a point every ten ticks.
const TOLL: i32 = 120;

// The buff ids the flags below are derived from. Named rather than inlined because several are
// referred to twice — once to set the flag, once in the tally — and a transposed digit between
// the two would be invisible.
const POISONED: u16 = 20;
const TIPSY: u16 = 25;
const ON_FIRE: u16 = 24;
const BLEEDING: u16 = 30;
const CONFUSED: u16 = 31;
const BROKEN_ARMOR: u16 = 36;
const ON_FIRE2: u16 = 39;
const ON_FROSTBURN: u16 = 44;
const ICHOR: u16 = 69;
const VENOM: u16 = 70;
const MIDAS: u16 = 72;
const DRIPPING: u16 = 103;
const LOVE_STRUCK: u16 = 119;
const STINKY: u16 = 120;
const DRIPPING_SLIME: u16 = 137;
const SOUL_DRAIN: u16 = 151;
const SHADOW_FLAME: u16 = 153;
const DRYAD_WARD: u16 = 165;
const JAVELINED: u16 = 169;
const CELLED: u16 = 183;
const DRYAD_BANE: u16 = 186;
const DAYBREAK: u16 = 189;
const BETSYS_CURSE: u16 = 203;
const OILED: u16 = 204;
const MARKED_BY_SCYTHE_WHIP: u16 = 310;
const DRIPPING_SPARKLE_SLIME: u16 = 320;
const ON_FIRE3: u16 = 323;
const ON_FROSTBURN2: u16 = 324;
const TENTACLE_SPIKED: u16 = 337;
const BLOOD_BUTCHERED: u16 = 344;
const SHIMMERING: u16 = 353;
const MARKED_BY_EEL_WHIP: u16 = 362;
const HEMORRHAGE: u16 = 375;
const POTENT_ACID: u16 = 395;
const CHLOROPHYTE_SPORE: u16 = 397;
const ACCELERATE_POISONS: u16 = 398;
const BLUE_LIGHTNING: u16 = 399;
const RED_LIGHTNING: u16 = 400;

/// The Blue Slime, whose `ai[1]` is not a state at all but the id of the item it has swallowed.
///
/// This is the 1.4 "slime with something inside" mechanic, and it reaches into the debuff
/// arithmetic in three places: one swallowed item makes the slime re-light itself every second,
/// another stops it burning in a Ravaged world, and four more make it regenerate. Reading
/// `ai[1]` as a phase number here would be wrong in a way that looks right.
const BLUE_SLIME: u16 = 1;

/// The Lava Slime, which likewise regenerates on one particular swallowed item, in lava.
const LAVA_SLIME: u16 = 59;

// The swallowed items that matter. A torch keeps a Ravaged slime from burning; the rest either
// keep it alight or heal it.
const SWALLOWED_TORCH: f32 = 8.0;
const SWALLOWED_RELIGHTER: f32 = 9.0;
const SWALLOWED_HEALER: f32 = 29.0;
const SWALLOWED_BIG_HEALERS: [f32; 6] = [364.0, 1104.0, 365.0, 1105.0, 366.0, 1106.0];
const SWALLOWED_LAVA_HEALER: f32 = 174.0;

/// One occupied buff slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Slot {
    pub kind: u16,
    pub time: i32,
}

impl Slot {
    fn is_empty(self) -> bool {
        self.kind == 0 || self.time <= 0
    }
}

/// What the buffs currently on an NPC mean, derived fresh every tick.
///
/// The game keeps these as three dozen loose booleans on `NPC` and resets them all at the top of
/// every update. Gathering them changes nothing about when they are read; it just means the
/// reset is one assignment instead of thirty-eight that can be forgotten one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    pub poisoned: bool,
    pub tipsy: bool,
    pub bleeding: bool,
    pub hemorrhage: bool,
    pub venom: bool,
    pub chlorophyte_spore: bool,
    pub potent_acid: bool,
    pub on_fire: bool,
    pub on_fire2: bool,
    pub on_fire3: bool,
    pub on_frostburn: bool,
    pub on_frostburn2: bool,
    pub shadow_flame: bool,
    pub accelerate_poisons: bool,
    /// Ichor, and below it broken armour and Betsy's curse. All three lower armour, and all three
    /// do it on the hitting client rather than here, see the module note for the three
    /// `checkArmorPenetration` call sites and why none of them can run on a server. Kept because
    /// the client can only apply them if it is told they are on.
    pub ichor: bool,
    pub broken_armor: bool,
    pub betsys_curse: bool,
    pub midas: bool,
    /// Confusion, which reverses the way an NPC faces at the tail of `SetTargetTrackingValues`
    /// (`NPC.cs:78576-78579`). Read by [`crate::game::ai::face`].
    pub confused: bool,
    pub dripping: bool,
    pub dripping_slime: bool,
    pub dripping_sparkle_slime: bool,
    pub love_struck: bool,
    /// Stinky. On the NPC side vanilla reads it for a colour tint (`NPC.cs:92208`), a gore burst
    /// (`:92465`), a town resident starting to walk (`:54181`) and the town-NPC threat search
    /// (`:54033-54084`), plus water washing the buff off in
    /// `TryRemovingWaterPerishableEffects` (`:94433`, `if (!stinky || Main.netMode == 1) return;`).
    /// This server models none of the last three: town residents do not treat a stinky NPC as a
    /// threat and nothing here washes a perishable buff off in water, not even On Fire.
    pub stinky: bool,
    pub soul_drain: bool,
    /// Dryad's Blessing. Raises armour by 20/15/10 on master/expert/classic
    /// (`NPC.cs:53550-53561`), which `tick_town_casualties` applies. Vanilla's two other reads,
    /// the thorns reflect in `BeHurtByOtherNPC` (`:93604`) and the +10 life regeneration in
    /// `CheckLifeRegen` (`:93627`), are both in paths this server does not model for NPCs.
    pub dryad_ward: bool,
    pub dryad_bane: bool,
    pub javelined: bool,
    pub tentacle_spiked: bool,
    pub blood_butchered: bool,
    pub celled: bool,
    pub daybreak: bool,
    pub oiled: bool,
    pub marked_by_scythe_whip: bool,
    pub marked_by_eel_whip: bool,
    pub blue_lightning: bool,
    pub red_lightning: bool,
    /// Shimmering. Its two server-side reads in vanilla both need machinery this project does not
    /// have: it gates `UpdateHomeTileState` for a town NPC standing still (`NPC.cs:53846`,
    /// `:53913`), and this server never derives a home tile from where somebody is standing (they
    /// come from housing), and it drives `shimmerTransparency` up to the `GetShimmered()`
    /// transformation (`NPC.cs:92634-92640`), which no NPC here undergoes - `shimmered_town_npcs`
    /// is read and written by the world file and by nothing else.
    pub shimmering: bool,
}

/// What the world outside the NPC contributes to its damage-over-time.
///
/// Five debuffs are not a flat rate at all: they are worth whatever is *stuck in* the NPC. A
/// Daybreak spear does a hundred a second per spear, so the count has to come from the
/// projectile table rather than from the buff.
#[derive(Debug, Clone, Copy, Default)]
pub struct Around {
    pub npc_type: u16,
    /// The NPC's `ai[1]`. For most types this means nothing here; for the two slimes it is the
    /// id of the item the slime swallowed, which changes what fire and regeneration do to it.
    pub ai1: f32,
    /// Whether this NPC is a link of something longer, in which case soul drain does not apply:
    /// the game checks `realLife == -1` so a worm is not drained once per segment.
    pub is_segment: bool,
    /// Ravaged, `Main.getGoodWorld`. Exempts one of King Slime's states from burning.
    pub get_good: bool,
    /// Whether it is standing in lava, which one Man Eater state heals from.
    pub lava_wet: bool,
    /// Daybreak spears (projectile 636) stuck in it.
    pub daybreaks: usize,
    /// Javelins (598).
    pub javelins: usize,
    /// Tentacle spikes (971).
    pub tentacles: usize,
    /// Blood butcherer knives (975).
    pub blood_knives: usize,
    /// Stardust cells (614).
    pub cells: usize,
    /// What the Dryad's Bane is worth right now, which grows with every boss downed.
    pub dryad_bane_dps: i32,
}

/// What one tick of debuffs did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Toll {
    /// Life regained from natural or supernatural regeneration.
    pub healed: i32,
    /// Life lost to debuffs. Dealt as separate hits, which is what the game reports.
    pub hurt: i32,
    /// How many separate hits `hurt` was dealt in, since the game sends one packet per hit.
    pub hits: i32,
}

/// The twenty slots and everything derived from them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Buffs {
    slots: [Slot; MAX_BUFFS],
    /// What the slots currently mean. Recomputed every tick.
    pub flags: Flags,
    /// This tick's regeneration figure, positive to heal and negative to hurt.
    life_regen: i32,
    /// The running total the threshold is checked against.
    life_regen_count: i32,
}

impl Default for Buffs {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffs {
    pub const fn new() -> Self {
        Self {
            slots: [Slot { kind: 0, time: 0 }; MAX_BUFFS],
            flags: Flags {
                poisoned: false,
                tipsy: false,
                bleeding: false,
                hemorrhage: false,
                venom: false,
                chlorophyte_spore: false,
                potent_acid: false,
                on_fire: false,
                on_fire2: false,
                on_fire3: false,
                on_frostburn: false,
                on_frostburn2: false,
                shadow_flame: false,
                accelerate_poisons: false,
                ichor: false,
                broken_armor: false,
                betsys_curse: false,
                midas: false,
                confused: false,
                dripping: false,
                dripping_slime: false,
                dripping_sparkle_slime: false,
                love_struck: false,
                stinky: false,
                soul_drain: false,
                dryad_ward: false,
                dryad_bane: false,
                javelined: false,
                tentacle_spiked: false,
                blood_butchered: false,
                celled: false,
                daybreak: false,
                oiled: false,
                marked_by_scythe_whip: false,
                marked_by_eel_whip: false,
                blue_lightning: false,
                red_lightning: false,
                shimmering: false,
            },
            life_regen: 0,
            life_regen_count: 0,
        }
    }

    /// Whether anything at all is on this NPC.
    ///
    /// Worth asking before doing any of the per-tick work, because for most NPCs most of the
    /// time the answer is no.
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.is_empty())
    }

    /// The occupied slots, in order, which is what the sync packet carries.
    ///
    /// Holes are skipped rather than stopped at, matching the game's own writer: removal
    /// compacts, so there should never be one, but a truncated buff list is an invisible bug and
    /// skipping costs nothing.
    pub fn active(&self) -> impl Iterator<Item = Slot> + '_ {
        self.slots.iter().copied().filter(|s| !s.is_empty())
    }

    /// Whether a given buff is on, ignoring the derived flags.
    pub fn has(&self, kind: u16) -> bool {
        self.slots.iter().any(|s| s.kind == kind && s.time >= 1)
    }

    /// Where a buff sits, if it is on and the type is not immune. `NPC.FindBuffIndex`.
    pub fn find(&self, npc_type: u16, kind: u16) -> Option<usize> {
        if npc_is_immune(npc_type, kind) {
            return None;
        }
        self.slots
            .iter()
            .position(|s| s.time >= 1 && s.kind == kind)
    }

    /// Whether a buff is absent or has `within` ticks or fewer left on it.
    ///
    /// Vanilla writes this inline as `FindBuffIndex(k) == -1 || buffTime[FindBuffIndex(k)] <= n`
    /// wherever something re-applies a buff on a repeating timer (`Projectile.cs:41963` and
    /// `:41967` are the Dryad's ward's pair). It reads like a redundant guard in front of
    /// [`Self::add`], which refuses a shorter time anyway, and it is not: without it a source that
    /// fires every ten ticks pins the remaining time at its full value forever and re-broadcasts
    /// the whole slot list each time. With it the timer sawtooths down to `within` and back, which
    /// is one packet per hundred ticks rather than ten.
    pub fn expiring(&self, npc_type: u16, kind: u16, within: i32) -> bool {
        match self.find(npc_type, kind) {
            Some(at) => self.slots[at].time <= within,
            None => true,
        }
    }

    /// Put a buff on, or extend one already there. `NPC.AddBuff`.
    ///
    /// Returns whether anything changed, which is what decides if clients need telling.
    ///
    /// The eviction rule is the game's and is worth stating: when all twenty slots are taken,
    /// the *first* slot holding something that is not a debuff is dropped to make room. If every
    /// slot holds a debuff, the new one is simply refused. So a boss cannot be talked out of its
    /// poison, and a player cannot displace one by piling on blessings.
    pub fn add(&mut self, npc_type: u16, kind: u16, time: i32) -> bool {
        if kind == 0 || npc_is_immune(npc_type, kind) {
            return false;
        }

        let mut at = None;
        for (i, slot) in self.slots.iter().enumerate() {
            if slot.kind == kind {
                // A shorter application never cuts an existing one short.
                if slot.time >= time {
                    return false;
                }
                at = Some(i);
                break;
            }
        }

        while at.is_none() {
            let Some(evictable) = self.slots.iter().position(|s| !is_debuff(s.kind)) else {
                return false; // twenty debuffs, and none of them may be pushed out
            };
            // Prefer a genuinely empty slot at or after the evictable one; the game looks for a
            // zero from there rather than from the start, so a run of debuffs at the front is
            // never disturbed.
            at = self.slots[evictable..]
                .iter()
                .position(|s| s.kind == 0)
                .map(|offset| evictable + offset);
            if at.is_none() {
                self.remove_at(evictable);
            }
        }

        let at = at.expect("the loop only exits with a slot or a return");
        self.slots[at] = Slot { kind, time };
        true
    }

    /// Take a buff off and close the gap behind it. `NPC.DelBuff`.
    ///
    /// The compaction matters beyond tidiness: the sync packet stops at the first empty slot, so
    /// a hole would hide everything after it from every client.
    pub fn remove_at(&mut self, at: usize) {
        if at >= MAX_BUFFS {
            return;
        }
        self.slots[at] = Slot::default();
        for i in 0..MAX_BUFFS - 1 {
            if self.slots[i].kind == 0 || self.slots[i].time == 0 {
                for j in i + 1..MAX_BUFFS {
                    self.slots[j - 1] = self.slots[j];
                    self.slots[j] = Slot::default();
                }
            }
        }
    }

    /// Take a named buff off, if a client is allowed to ask for that.
    ///
    /// `NPC.RequestBuffRemoval`. In this version the permitted set is empty, so every request is
    /// refused — the packet exists and the game validates against a table that happens to have
    /// nothing in it. Refusing is the correct behaviour, not a gap.
    pub fn remove_by_request(&mut self, npc_type: u16, kind: u16) -> bool {
        if !terrustia_proto::buffs::REMOVABLE_BY_REQUEST
            .get(kind as usize)
            .copied()
            .unwrap_or(false)
        {
            return false;
        }
        let Some(at) = self.find(npc_type, kind) else {
            return false;
        };
        self.remove_at(at);
        true
    }

    /// Drop everything whose time has run out. `NPC.UpdateNPC_BuffClearExpiredBuffs`.
    ///
    /// Returns whether anything went, which is what decides if clients need telling.
    pub fn clear_expired(&mut self) -> bool {
        let mut any = false;
        for i in 0..MAX_BUFFS {
            if self.slots[i].kind > 0 && self.slots[i].time <= 0 {
                self.remove_at(i);
                any = true;
            }
        }
        any
    }

    /// Read the slots into the flags and run every timer down one tick.
    ///
    /// `NPC.UpdateNPC_BuffFlagsReset` and `UpdateNPC_BuffSetFlags` together: the game resets and
    /// re-derives on every update rather than maintaining the flags incrementally, so a buff that
    /// runs out cannot leave its effect behind.
    pub fn set_flags(&mut self, npc_type: u16, ai1: f32) {
        self.flags = Flags::default();
        self.life_regen = 0;

        for i in 0..MAX_BUFFS {
            let kind = self.slots[i].kind;
            if kind == 0 || self.slots[i].time <= 0 {
                continue;
            }
            // Set before the decrement, exactly as the game does: acceleration applies on the
            // same tick the potion's own timer moves.
            if kind == ACCELERATE_POISONS {
                self.flags.accelerate_poisons = true;
            }
            self.slots[i].time -= 1;

            let f = &mut self.flags;
            match kind {
                POISONED => f.poisoned = true,
                TIPSY => f.tipsy = true,
                BLEEDING => f.bleeding = true,
                HEMORRHAGE => f.hemorrhage = true,
                VENOM => f.venom = true,
                CHLOROPHYTE_SPORE => f.chlorophyte_spore = true,
                POTENT_ACID => f.potent_acid = true,
                ON_FIRE => {
                    // A slime that swallowed a torch-lighter keeps re-lighting itself, so the
                    // burn is pinned at a second rather than running out.
                    if npc_type == BLUE_SLIME && ai1 == SWALLOWED_RELIGHTER {
                        self.slots[i].time = 60;
                    }
                    f.on_fire = true;
                }
                MIDAS => f.midas = true,
                ICHOR => f.ichor = true,
                BROKEN_ARMOR => f.broken_armor = true,
                CONFUSED => f.confused = true,
                ON_FIRE2 => f.on_fire2 = true,
                ON_FROSTBURN => {
                    if npc_type == BLUE_SLIME && ai1 == SWALLOWED_RELIGHTER {
                        self.slots[i].time = 60;
                    }
                    f.on_frostburn = true;
                }
                DRIPPING => f.dripping = true,
                DRIPPING_SLIME => f.dripping_slime = true,
                DRIPPING_SPARKLE_SLIME => f.dripping_sparkle_slime = true,
                LOVE_STRUCK => f.love_struck = true,
                STINKY => f.stinky = true,
                SOUL_DRAIN => f.soul_drain = true,
                SHADOW_FLAME => f.shadow_flame = true,
                DRYAD_WARD => f.dryad_ward = true,
                JAVELINED => f.javelined = true,
                TENTACLE_SPIKED => f.tentacle_spiked = true,
                BLOOD_BUTCHERED => f.blood_butchered = true,
                CELLED => f.celled = true,
                DRYAD_BANE => f.dryad_bane = true,
                DAYBREAK => f.daybreak = true,
                BETSYS_CURSE => f.betsys_curse = true,
                OILED => f.oiled = true,
                MARKED_BY_SCYTHE_WHIP => f.marked_by_scythe_whip = true,
                MARKED_BY_EEL_WHIP => f.marked_by_eel_whip = true,
                BLUE_LIGHTNING => f.blue_lightning = true,
                RED_LIGHTNING => f.red_lightning = true,
                ON_FIRE3 => f.on_fire3 = true,
                ON_FROSTBURN2 => f.on_frostburn2 = true,
                SHIMMERING => {
                    // Immunity is re-checked here, not only on application: an NPC that becomes
                    // immune mid-fight sheds the buff rather than keeping it.
                    if npc_is_immune(npc_type, SHIMMERING) {
                        self.remove_at(i);
                    } else {
                        f.shimmering = true;
                    }
                }
                _ => {}
            }
        }
    }

    /// Work out what the debuffs cost this tick. `NPC.UpdateNPC_BuffApplyDOTs`.
    ///
    /// The shape is worth keeping in mind, because it is not "N damage per second". Every debuff
    /// adds to a regeneration figure; that figure is added to a running total each tick; and a
    /// hit lands each time the total crosses [`TOLL`]. A debuff worth 12 therefore deals one
    /// point every ten ticks, and two of them deal one every five — which is why stacking them
    /// feels smooth rather than stepped.
    ///
    /// `expected_dps` is the game's own smoothing: once a debuff declares one, damage is dealt in
    /// larger, rarer lumps rather than one point at a time, so a Daybreak spear does not send a
    /// hundred packets a second.
    pub fn dots(&mut self, around: &Around, immortal: bool, dont_take_damage: bool) -> Toll {
        let mut toll = Toll::default();
        if dont_take_damage {
            return toll;
        }

        let f = self.flags;
        let mut tally = Tally {
            accelerating: f.accelerate_poisons,
            dripping_slime: f.dripping_slime,
            relighting_slime: around.npc_type == BLUE_SLIME && around.ai1 == SWALLOWED_RELIGHTER,
            ..Tally::default()
        };

        if f.poisoned {
            tally.poison(12, 0);
        }
        if f.bleeding {
            tally.blood_loss(24, 4);
        }
        if f.hemorrhage {
            tally.blood_loss(200, 40);
        }
        // A slime that swallowed a torch does not burn in a Ravaged world; everything else does.
        if f.on_fire
            && !(around.npc_type == BLUE_SLIME && around.ai1 == SWALLOWED_TORCH && around.get_good)
        {
            tally.flammable(8, 0);
        }
        if f.on_fire3 {
            tally.flammable(30, 5);
        }
        if f.on_frostburn {
            tally.flammable(16, 2);
        }
        if f.on_frostburn2 {
            tally.flammable(50, 10);
        }
        if f.on_fire2 {
            tally.flammable(48, 10);
        }
        if f.venom {
            tally.poison(60, 15);
        }
        if f.potent_acid {
            tally.poison(180, 60);
        }
        if f.chlorophyte_spore {
            tally.poison(180, 60);
        }
        if f.shadow_flame {
            tally.flammable(30, 5);
        }
        // Oil is worth nothing on its own and a great deal on something already alight.
        if f.oiled
            && (f.on_fire
                || f.on_fire2
                || f.on_fire3
                || f.on_frostburn
                || f.on_frostburn2
                || f.shadow_flame)
        {
            tally.add(50, 10);
        }
        if f.javelined {
            tally.stacked(around.javelins, 3, 1);
        }
        if f.tentacle_spiked {
            tally.stacked(around.tentacles, 3, 1);
        }
        if f.marked_by_eel_whip {
            tally.add(100, 50);
        }
        if f.blood_butchered {
            tally.stacked(around.blood_knives, 4, 1);
        }
        if f.daybreak {
            tally.stacked(around.daybreaks, 100, 4);
        }
        if f.celled {
            tally.stacked(around.cells, 20, 1);
        }
        if f.dryad_bane {
            let dps = around.dryad_bane_dps;
            tally.add(dps * 2, dps / 3);
        }
        if f.soul_drain && !around.is_segment {
            tally.add(50, 5);
        }
        // Two cases heal rather than hurt, and both are a slime digesting what it swallowed.
        if around.npc_type == LAVA_SLIME && around.ai1 == SWALLOWED_LAVA_HEALER && around.lava_wet {
            tally.supernatural += 32;
        }
        if around.npc_type == BLUE_SLIME {
            if around.ai1 == SWALLOWED_HEALER {
                tally.supernatural += 16;
            } else if SWALLOWED_BIG_HEALERS.contains(&around.ai1) {
                tally.supernatural += 24;
            }
        }

        if tally.blocks_regeneration && self.life_regen > 0 {
            self.life_regen = 0;
        }
        let mut loss = tally.regen_loss;
        if f.accelerate_poisons {
            let unboosted = tally.regen_loss - tally.boostable;
            loss = unboosted + tally.boostable * 2;
        }
        self.life_regen -= loss;
        self.life_regen += tally.supernatural;

        let mut per_hit = -1;
        if tally.expected_dps != 0 {
            per_hit = tally.expected_dps;
        }
        self.life_regen_count += self.life_regen;

        if per_hit == -1 {
            // With no declared rate, the game infers one from how fast the total is falling, so
            // heavy stacks still land in lumps.
            let inferred = self.life_regen_count / -TOLL;
            if inferred > 1 {
                per_hit = inferred;
            }
        }

        while self.life_regen_count >= TOLL {
            self.life_regen_count -= TOLL;
            if !immortal {
                toll.healed += 1;
            }
        }

        let step = if per_hit > 0 { per_hit } else { 1 };
        while self.life_regen_count <= -TOLL * step {
            self.life_regen_count += TOLL * step;
            toll.hurt += step;
            toll.hits += 1;
        }
        toll
    }
}

/// The running total a tick's debuffs build up. `NPC.DOTTally`.
#[derive(Debug, Clone, Copy, Default)]
struct Tally {
    regen_loss: i32,
    /// The part of `regen_loss` that Potion Sickness-style acceleration may double. Only poison
    /// and blood loss qualify; fire does not.
    boostable: i32,
    expected_dps: i32,
    blocks_regeneration: bool,
    supernatural: i32,
    accelerating: bool,
    dripping_slime: bool,
    relighting_slime: bool,
}

impl Tally {
    fn add(&mut self, regen_loss: i32, expected: i32) {
        self.blocks_regeneration = true;
        self.regen_loss += regen_loss;
        if self.expected_dps < expected {
            self.expected_dps = expected;
        }
    }

    fn support_acceleration(&mut self, regen_loss: i32) {
        if self.accelerating {
            self.boostable += regen_loss;
        }
    }

    fn poison(&mut self, regen_loss: i32, expected: i32) {
        self.support_acceleration(regen_loss);
        self.add(regen_loss, expected);
    }

    fn blood_loss(&mut self, regen_loss: i32, expected: i32) {
        self.support_acceleration(regen_loss);
        self.add(regen_loss, expected);
    }

    /// Fire, which a coat of slime doubles and a self-relighting slime adds a flat sixteen to.
    fn flammable(&mut self, regen_loss: i32, expected: i32) {
        let mut regen_loss = regen_loss;
        if self.dripping_slime {
            regen_loss *= 2;
        }
        if self.relighting_slime {
            regen_loss += 16;
        }
        self.support_acceleration(regen_loss);
        self.add(regen_loss, expected);
    }

    /// A debuff whose strength is however many of something are stuck in the target.
    ///
    /// The game counts none as one, so a mark whose projectile has already expired still ticks
    /// for its base rate until the buff itself runs out.
    fn stacked(&mut self, count: usize, damage_per_second: i32, divider: i32) {
        let count = i32::try_from(count).unwrap_or(i32::MAX).max(1);
        let expected = count * damage_per_second / divider.max(1);
        self.add(count * 2 * damage_per_second, expected);
    }
}

/// What the Dryad's Bane is worth, which grows with each boss put down.
/// `NPC.GetDryadsBaneDamagePerSecond`.
pub struct BossesDowned {
    pub eye: bool,
    pub evil: bool,
    pub skeletron: bool,
    pub queen_bee: bool,
    pub hard_mode: bool,
    pub queen_slime: bool,
    pub destroyer: bool,
    pub twins: bool,
    pub prime: bool,
    pub plantera: bool,
    pub golem: bool,
    pub cultist: bool,
    pub empress: bool,
    pub fishron: bool,
    /// Infected, the "not the bees" seed, which doubles the base.
    pub infected_seed: bool,
}

/// Work out the Dryad's Bane rate for a world at this point in its progression.
///
/// `difficulty` is the same continuous number `terrustia_proto::difficulty::of_game_mode`
/// produces (0.5 journey, 1 classic, 2 expert, 3 master) — continuous rather than a raw game mode
/// so a Journey world's `DifficultySlider` reaches this too (`server.rs`'s `effective_difficulty`
/// is the accessor every caller should use to get it).
pub fn dryad_bane_dps(downed: &BossesDowned, difficulty: f32, get_good: bool) -> i32 {
    let mut base = 4.0f32;
    let mut scale = 1.0f32;
    for (yes, by) in [
        (downed.eye, 0.1),
        (downed.evil, 0.1),
        (downed.skeletron, 0.1),
        (downed.queen_bee, 0.1),
        (downed.hard_mode, 0.4),
        (downed.queen_slime, 0.15),
        (downed.destroyer, 0.15),
        (downed.twins, 0.15),
        (downed.prime, 0.15),
        (downed.plantera, 0.15),
        (downed.golem, 0.15),
        (downed.cultist, 0.15),
        (downed.empress, 0.15),
        (downed.fishron, 0.15),
    ] {
        if yes {
            scale += by;
        }
    }
    if downed.infected_seed {
        base *= 2.0;
    }
    (base * scale * town_npc_damage_multiplier(difficulty, get_good)) as i32
}

/// How much harder a town NPC — and the Dryad's Bane, which borrows the same curve — hits on
/// each difficulty. `GameDifficultyData.TownNPCDamageMultiplier`.
///
/// A piecewise-linear curve over the difficulty number rather than a table, because Ravaged adds
/// one to that number and can push it past the named levels — and, now, because a Journey world's
/// `DifficultySlider` can land anywhere continuously between the four named levels too.
fn town_npc_damage_multiplier(mut difficulty: f32, get_good: bool) -> f32 {
    if get_good {
        difficulty += 1.0;
    }
    const KEYS: [(f32, f32); 4] = [(0.5, 2.0), (1.0, 1.0), (2.0, 1.5), (4.0, 2.0)];
    let mut lower = KEYS[0];
    for key in KEYS {
        if key.0 > difficulty {
            let span = key.0 - lower.0;
            if span <= 0.0 {
                return key.1;
            }
            let t = (difficulty - lower.0) / span;
            return lower.1 + (key.1 - lower.1) * t;
        }
        lower = key;
    }
    lower.1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zombie: immune to nothing at all, which makes it the right stand-in for "an ordinary
    /// enemy". The Blue Slime is *not* — it is immune to poison, which quietly turns a poison
    /// test into a test of nothing.
    const ZOMBIE: u16 = 3;
    /// The Blue Slime, used where immunity itself is the subject.
    const SLIME: u16 = 1;
    /// The Wall of Flesh, which is immune to everything that is not a whip.
    const WALL: u16 = 113;

    /// Ichor: a debuff with a high enough id that filling the slots in ascending order does not
    /// reach it, which the eviction tests need.
    const LATE_DEBUFF: u16 = ICHOR;

    #[test]
    fn a_buff_goes_on_and_runs_down() {
        let mut buffs = Buffs::new();
        assert!(buffs.add(ZOMBIE, ON_FIRE, 3));
        assert!(buffs.has(ON_FIRE));
        buffs.set_flags(ZOMBIE, 0.0);
        assert!(buffs.flags.on_fire);
        buffs.set_flags(ZOMBIE, 0.0);
        buffs.set_flags(ZOMBIE, 0.0);
        assert!(!buffs.has(ON_FIRE), "three ticks should use up three ticks");
        assert!(buffs.clear_expired());
        assert!(buffs.is_empty());
    }

    /// Re-applying never shortens what is already there.
    #[test]
    fn a_shorter_application_is_refused() {
        let mut buffs = Buffs::new();
        assert!(buffs.add(ZOMBIE, ON_FIRE, 600));
        assert!(!buffs.add(ZOMBIE, ON_FIRE, 60));
        assert_eq!(buffs.active().next().unwrap().time, 600);
        assert!(buffs.add(ZOMBIE, ON_FIRE, 900), "a longer one extends it");
        assert_eq!(buffs.active().next().unwrap().time, 900);
    }

    /// The gate a repeating source re-applies through: absent, or nearly out, and nothing else.
    ///
    /// The Dryad's ward is the one that needs it (`Projectile.cs:41963`, `:41967`). Without it a
    /// source firing every ten ticks pins the timer at its full value and reports a change every
    /// time, which is the whole slot list on the wire ten times as often as it needs to be.
    #[test]
    fn a_buff_nearly_out_is_the_only_one_worth_reapplying() {
        let mut buffs = Buffs::new();
        assert!(
            buffs.expiring(ZOMBIE, DRYAD_BANE, 20),
            "absent counts as expiring - there is nothing there to keep"
        );
        assert!(buffs.add(ZOMBIE, DRYAD_BANE, 120));
        assert!(
            !buffs.expiring(ZOMBIE, DRYAD_BANE, 20),
            "fresh, so left alone"
        );

        for _ in 0..99 {
            buffs.set_flags(ZOMBIE, 0.0);
        }
        assert!(
            !buffs.expiring(ZOMBIE, DRYAD_BANE, 20),
            "twenty-one left is still one too many"
        );
        buffs.set_flags(ZOMBIE, 0.0);
        assert!(
            buffs.expiring(ZOMBIE, DRYAD_BANE, 20),
            "and at twenty it is renewed"
        );
    }

    /// An immune type has no buff to be nearly out of, so it is always ready for one it can never
    /// take: `FindBuffIndex` returns -1 for an immunity exactly as it does for an empty slot.
    #[test]
    fn expiring_reads_an_immunity_as_absent() {
        let mut buffs = Buffs::new();
        assert!(!buffs.add(SLIME, POISONED, 600));
        assert!(buffs.expiring(SLIME, POISONED, 20));
    }

    /// The type's own immunities are honoured. A Blue Slime shrugs off poison.
    #[test]
    fn an_immune_type_refuses_the_buff() {
        let mut buffs = Buffs::new();
        assert!(
            !buffs.add(SLIME, POISONED, 600),
            "a Blue Slime is immune to poison"
        );
        assert!(buffs.is_empty());
        assert!(buffs.add(SLIME, ON_FIRE, 600), "but not to fire");
        assert!(
            buffs.add(ZOMBIE, POISONED, 600),
            "and a zombie is immune to nothing"
        );
    }

    /// Poison immunity implies bleeding immunity, which `SetDefaults` applies on top of the
    /// table rather than listing.
    #[test]
    fn poison_immunity_carries_bleeding_with_it() {
        let mut buffs = Buffs::new();
        assert!(!buffs.add(SLIME, BLEEDING, 600));
        assert!(!buffs.add(SLIME, HEMORRHAGE, 600));
        assert!(buffs.add(ZOMBIE, BLEEDING, 600), "a zombie still bleeds");
    }

    /// A boss immune to everything but whips can still be marked by one.
    #[test]
    fn a_whip_mark_lands_where_nothing_else_does() {
        let mut buffs = Buffs::new();
        assert!(!buffs.add(WALL, ON_FIRE, 600), "the Wall does not burn");
        assert!(
            buffs.add(WALL, MARKED_BY_SCYTHE_WHIP, 600),
            "but a whip still marks it"
        );
    }

    /// Twenty debuffs and no room: the twenty-first is refused rather than evicting one.
    #[test]
    fn debuffs_are_never_pushed_out() {
        let mut buffs = Buffs::new();
        // Twenty distinct debuffs the slime is not immune to.
        let mut placed = 0;
        for kind in 1..401u16 {
            if placed == MAX_BUFFS {
                break;
            }
            if is_debuff(kind) && kind != LATE_DEBUFF && buffs.add(ZOMBIE, kind, 600) {
                placed += 1;
            }
        }
        assert_eq!(placed, MAX_BUFFS, "the slots should be full of debuffs");
        assert!(
            !buffs.add(ZOMBIE, LATE_DEBUFF, 600),
            "there is nothing evictable left"
        );
    }

    /// A good buff is evicted to make room, which is the other half of the same rule.
    #[test]
    fn a_good_buff_makes_way() {
        let mut buffs = Buffs::new();
        // Buff 1 is Obsidian Skin — not a debuff.
        assert!(!is_debuff(1), "buff 1 should be a blessing, not a curse");
        assert!(buffs.add(ZOMBIE, 1, 600));
        for kind in 2..401u16 {
            if buffs.active().count() == MAX_BUFFS {
                break;
            }
            // Leave the one the eviction is then tested with out of the fill.
            if is_debuff(kind) && kind != LATE_DEBUFF {
                buffs.add(ZOMBIE, kind, 600);
            }
        }
        assert_eq!(buffs.active().count(), MAX_BUFFS);
        assert!(
            buffs.add(ZOMBIE, LATE_DEBUFF, 600),
            "the blessing should have made way"
        );
        assert!(buffs.has(LATE_DEBUFF));
        assert!(!buffs.has(1), "and it is the one that went");
    }

    /// Removing from the middle closes the gap, or the sync packet would truncate the list.
    #[test]
    fn removal_closes_the_gap() {
        let mut buffs = Buffs::new();
        buffs.add(ZOMBIE, ON_FIRE, 600);
        buffs.add(ZOMBIE, POISONED, 600);
        buffs.add(ZOMBIE, VENOM, 600);
        buffs.remove_at(1);
        let kinds: Vec<u16> = buffs.active().map(|s| s.kind).collect();
        assert_eq!(kinds, vec![ON_FIRE, VENOM]);
    }

    /// Poison at twelve deals a point every ten ticks, which is the rate the game shows.
    #[test]
    fn poison_deals_a_point_every_ten_ticks() {
        let mut buffs = Buffs::new();
        buffs.add(ZOMBIE, POISONED, 6000);
        let around = Around {
            npc_type: ZOMBIE,
            ..Default::default()
        };
        let mut hurt = 0;
        let mut ticks_to_first = 0;
        for tick in 1..=120 {
            buffs.set_flags(ZOMBIE, 0.0);
            let toll = buffs.dots(&around, false, false);
            if toll.hurt > 0 && ticks_to_first == 0 {
                ticks_to_first = tick;
            }
            hurt += toll.hurt;
        }
        assert_eq!(ticks_to_first, 10, "twelve a tick crosses 120 at ten");
        assert_eq!(hurt, 12, "a hundred and twenty ticks is twelve points");
    }

    /// Nothing on means nothing spent, which is the case for almost every NPC almost always.
    #[test]
    fn no_buffs_costs_nothing() {
        let mut buffs = Buffs::new();
        buffs.set_flags(ZOMBIE, 0.0);
        let toll = buffs.dots(&Around::default(), false, false);
        assert_eq!(toll, Toll::default());
    }

    /// Something that cannot be hurt at all is not hurt by debuffs either.
    #[test]
    fn an_untouchable_target_takes_nothing() {
        let mut buffs = Buffs::new();
        buffs.add(ZOMBIE, POISONED, 6000);
        for _ in 0..60 {
            buffs.set_flags(ZOMBIE, 0.0);
            assert_eq!(buffs.dots(&Around::default(), false, true).hurt, 0);
        }
    }

    /// Oil is worth nothing until something is alight.
    #[test]
    fn oil_only_costs_when_it_is_lit() {
        let mut dry = Buffs::new();
        dry.add(ZOMBIE, OILED, 600);
        let mut lit = Buffs::new();
        lit.add(ZOMBIE, OILED, 600);
        lit.add(ZOMBIE, ON_FIRE, 600);
        let mut only_fire = Buffs::new();
        only_fire.add(ZOMBIE, ON_FIRE, 600);

        let around = Around {
            npc_type: ZOMBIE,
            ..Default::default()
        };
        let mut totals = [0; 3];
        for (i, buffs) in [&mut dry, &mut lit, &mut only_fire].into_iter().enumerate() {
            for _ in 0..600 {
                buffs.set_flags(ZOMBIE, 0.0);
                totals[i] += buffs.dots(&around, false, false).hurt;
            }
        }
        assert_eq!(totals[0], 0, "oil alone does nothing");
        assert!(
            totals[1] > totals[2],
            "oil on a fire ({}) should beat fire alone ({})",
            totals[1],
            totals[2]
        );
    }

    /// A stack of Daybreak spears hurts in proportion to how many are stuck in.
    #[test]
    fn a_stack_scales_with_what_is_stuck_in_it() {
        let mut one = Buffs::new();
        one.add(ZOMBIE, DAYBREAK, 600);
        let mut five = Buffs::new();
        five.add(ZOMBIE, DAYBREAK, 600);

        let mut totals = [0; 2];
        for (i, (buffs, count)) in [(&mut one, 1usize), (&mut five, 5usize)]
            .into_iter()
            .enumerate()
        {
            let around = Around {
                npc_type: ZOMBIE,
                daybreaks: count,
                ..Default::default()
            };
            for _ in 0..600 {
                buffs.set_flags(ZOMBIE, 0.0);
                totals[i] += buffs.dots(&around, false, false).hurt;
            }
        }
        assert!(
            totals[1] >= totals[0] * 4,
            "five spears ({}) against one ({})",
            totals[1],
            totals[0]
        );
    }

    /// Slime doubles fire, which is what the Slime Staff's debuff is for.
    #[test]
    fn dripping_slime_doubles_fire() {
        let mut plain = Buffs::new();
        plain.add(ZOMBIE, ON_FIRE, 6000);
        let mut slimed = Buffs::new();
        slimed.add(ZOMBIE, ON_FIRE, 6000);
        slimed.add(ZOMBIE, DRIPPING_SLIME, 6000);

        let around = Around {
            npc_type: ZOMBIE,
            ..Default::default()
        };
        let mut totals = [0; 2];
        for (i, buffs) in [&mut plain, &mut slimed].into_iter().enumerate() {
            for _ in 0..1200 {
                buffs.set_flags(ZOMBIE, 0.0);
                totals[i] += buffs.dots(&around, false, false).hurt;
            }
        }
        assert_eq!(totals[1], totals[0] * 2, "slime should double the burn");
    }

    /// The difficulty curve matches the game's keys at every named level.
    #[test]
    fn the_difficulty_curve_hits_its_keys() {
        assert_eq!(town_npc_damage_multiplier(0.5, false), 2.0, "journey");
        assert_eq!(town_npc_damage_multiplier(1.0, false), 1.0, "classic");
        assert_eq!(town_npc_damage_multiplier(2.0, false), 1.5, "expert");
        assert_eq!(town_npc_damage_multiplier(3.0, false), 1.75, "master");
        assert_eq!(
            town_npc_damage_multiplier(3.0, true),
            2.0,
            "ravaged master reaches the top of the curve"
        );
    }

    /// A Journey world's `DifficultySlider` can land anywhere continuously between the named
    /// levels — proving that requires a value the four named `game_mode`s could never have
    /// produced (2.5 sits exactly between expert's 2.0 and master's 3.0), not just relabelled
    /// versions of the same four fixed points the test above already covers.
    #[test]
    fn a_slider_value_between_named_levels_interpolates_rather_than_snapping() {
        let expert = town_npc_damage_multiplier(2.0, false);
        let between = town_npc_damage_multiplier(2.5, false);
        let master = town_npc_damage_multiplier(3.0, false);
        assert!(
            between > expert && between < master,
            "2.5 must land strictly between expert ({expert}) and master ({master}), got {between}"
        );
        assert_eq!(
            between, 1.625,
            "the exact midpoint of the (2.0, 1.5)-(4.0, 2.0) segment"
        );
    }

    /// The Dryad's Bane grows as the world's bosses fall.
    #[test]
    fn dryad_bane_grows_with_progress() {
        let fresh = BossesDowned {
            eye: false,
            evil: false,
            skeletron: false,
            queen_bee: false,
            hard_mode: false,
            queen_slime: false,
            destroyer: false,
            twins: false,
            prime: false,
            plantera: false,
            golem: false,
            cultist: false,
            empress: false,
            fishron: false,
            infected_seed: false,
        };
        let late = BossesDowned {
            eye: true,
            evil: true,
            skeletron: true,
            queen_bee: true,
            hard_mode: true,
            queen_slime: true,
            destroyer: true,
            twins: true,
            prime: true,
            plantera: true,
            golem: true,
            cultist: true,
            empress: true,
            fishron: true,
            ..fresh
        };
        assert_eq!(dryad_bane_dps(&fresh, 1.0, false), 4);
        assert!(dryad_bane_dps(&late, 1.0, false) > dryad_bane_dps(&fresh, 1.0, false));
    }

    /// A request to lift a buff is refused, because this version permits none.
    #[test]
    fn a_removal_request_is_refused() {
        let mut buffs = Buffs::new();
        buffs.add(ZOMBIE, ON_FIRE, 600);
        assert!(!buffs.remove_by_request(ZOMBIE, ON_FIRE));
        assert!(buffs.has(ON_FIRE), "and the buff is still there");
    }
}
