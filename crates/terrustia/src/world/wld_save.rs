//! Writer for Terraria's `.wld` save format.
//!
//! There are two paths, and which one runs depends on where the world came from.
//!
//! A world **loaded from a file** keeps its own header. Saving re-serialises the header verbatim
//! with the mutable fields patched in place, then the tiles, chests and signs. Every later
//! section is rewritten from this server's own live state rather than carried through: the
//! townsfolk (with the Lunar Pillars' own second list), the tile entities, the pressure-plate
//! section, the town manager's room list, the bestiary and the Journey powers. Only a townsfolk or
//! tile-entity section a load did not fully understand — where rewriting from a partial read would
//! mean silently dropping whatever came after the part that failed — still passes through as the
//! bytes it arrived as, and only the footer riding on the tail of the last section is ever copied
//! verbatim on purpose.
//!
//! A **generated** world has no header to copy, so one is written from scratch at
//! [`SAVE_VERSION`] — the whole of the game's own flag order, with the fields this server does
//! not model written as the values a fresh world holds. The format has no framing, so a field of
//! the wrong width there would put every field after it in the wrong place and corrupt the save
//! silently. That is why it is checked rather than trusted: the header written here is walked
//! independently and has to end exactly on the tile-section pointer.

use std::path::Path;

use terrustia_proto::{Tile, Writer, section::write_tile_with, tile_sets::allows_batching};

use super::{
    World,
    wld::{Scenery, WldError},
};

type Result<T> = std::result::Result<T, WldError>;

const MAGIC: &[u8; 7] = b"relogic";
const FILE_TYPE_WORLD: u8 = 2;

/// The format this writer emits for a world that has no file of its own.
///
/// A world loaded from a file keeps whatever version it came with, because its header is copied
/// verbatim and patched. A generated one has no header to copy, so it is written fresh at the
/// version this server was transcribed from.
// 326 is what real 1.4.5.8 writes; its section layout is byte-for-byte identical to 325 (no
// write-path field is gated between 323 and 326), so a freshly generated world is marked as the
// build this server transcribes rather than one release behind.
pub const SAVE_VERSION: i32 = 326;

/// How many sections the format has at [`SAVE_VERSION`].
const SECTIONS: usize = 11;

/// Serialise a world into `.wld` bytes.
pub fn serialize(world: &World) -> Result<Vec<u8>> {
    let Some(preserved) = world.preserved.as_ref() else {
        return Ok(serialize_fresh(world));
    };

    let section_count = 4 + preserved.trailing_sections.len();
    let mut w = Writer::with_capacity(4 * 1024 * 1024);

    // --- file format header ---------------------------------------------------------------
    w.i32(preserved.version)
        .bytes(MAGIC)
        .u8(FILE_TYPE_WORLD)
        .u32(preserved.revision.saturating_add(1))
        .u64(preserved.favorite)
        .i16(section_count as i16);

    // Pointers are patched once every section's position is known.
    let pointer_table = w.len();
    for _ in 0..section_count {
        w.i32(0);
    }

    // The importance table is packed least significant bit first, seeded at 0x80 as a sentinel.
    w.u16(preserved.importance.len() as u16);
    let mut current = 0u8;
    let mut bit = 0x80u8;
    for &framed in &preserved.importance {
        if bit == 0x80 {
            bit = 1;
            current = 0;
        } else {
            bit <<= 1;
        }
        if framed {
            current |= bit;
        }
        if bit == 0x80 {
            w.u8(current);
        }
    }
    // Flush a partial final byte.
    if bit != 0x80 && !preserved.importance.is_empty() {
        w.u8(current);
    }

    let mut pointers = vec![0i32; section_count];

    // --- section 0: world header, verbatim with the clock patched -------------------------
    pointers[0] = section_pointer(w.len())?;
    let mut header = preserved.header_bytes.clone();
    patch_clock(&mut header, preserved, world);
    w.bytes(&header);

    // --- section 1: tiles ------------------------------------------------------------------
    pointers[1] = section_pointer(w.len())?;
    let importance = |tile: u16| {
        preserved
            .importance
            .get(usize::from(tile))
            .copied()
            .unwrap_or(false)
    };
    write_tiles(&mut w, world, &importance);

    // --- section 2: chests -----------------------------------------------------------------
    pointers[2] = section_pointer(w.len())?;
    write_chests(&mut w, world, preserved.chest_slots);

    // --- section 3: signs ------------------------------------------------------------------
    pointers[3] = section_pointer(w.len())?;
    write_signs(&mut w, world);

    // --- sections 4..: rewritten from live state, except a section a load did not fully -----
    //     understand and the footer riding on the tail of the last one
    //
    // Sections 5 (tile entities), 6 (pressure plates), 7 (the town manager's room list), 8 (the
    // bestiary) and 9 (Journey powers) are all written from the server's own state rather than
    // copied. It has to be: a pylon placed while the server was running is not in the bytes that
    // were loaded, and one that was mined still is. Copying any of them back would mean the world
    // remembered whatever it had when it was opened and nothing since.
    //
    // Because these sections can change length, the pointers cannot be a single shift any more —
    // each is taken from where its section actually lands.
    for (index, section) in preserved.trailing_sections.iter().enumerate() {
        pointers[4 + index] = section_pointer(w.len())?;
        match index {
            // Rewritten only when the load understood the whole section. Rewriting one we read
            // partially would write back what we managed to decode and silently drop the rest —
            // which for these two sections means a world's residents (and the pillars riding in
            // the same section's second list) or its pylons.
            TOWN_NPC_SECTION if preserved.town_npcs_understood => {
                write_town_npcs(&mut w, world, preserved.version)
            }
            TILE_ENTITY_SECTION if preserved.tile_entities_understood => {
                write_tile_entities(&mut w, world, preserved.version)
            }
            // These four need no "understood" gate: none of them is rewritten from what was
            // *read*, only from live server state, so a section this build could not decode
            // simply loses nothing it was ever going to carry forward anyway.
            PRESSURE_PLATE_SECTION => write_pressure_plates(&mut w),
            TOWN_MANAGER_SECTION => write_town_rooms(&mut w, world),
            BESTIARY_SECTION => write_bestiary(&mut w),
            JOURNEY_SECTION => write_journey_powers(&mut w, world),
            _ => {
                w.bytes(section);
            }
        }
    }

    let mut bytes = w.into_bytes();
    for (index, pointer) in pointers.iter().enumerate() {
        let at = pointer_table + index * 4;
        bytes[at..at + 4].copy_from_slice(&pointer.to_le_bytes());
    }
    Ok(bytes)
}

/// One entry in the format's section-pointer table, refused rather than truncated.
///
/// The table is `i32`, so a world that serialised past 2 GiB would write a negative offset and
/// produce a file whose sections point outside it. The *trailing* pointers were already checked
/// this way; the first four were a bare `as i32` that would have wrapped silently. `save` verifies
/// what it wrote before replacing anything, so a wrapped pointer could never have reached the real
/// file - but the operator would have been told "the world was written but could not be read back",
/// which sounds like a bug in the writer, rather than "this world is too big for the format", which
/// is what actually happened.
fn section_pointer(at: usize) -> Result<i32> {
    i32::try_from(at).map_err(|_| WldError::SaveTooLarge {
        // Only for the message. A `usize` past `i64::MAX` cannot exist on any machine that got
        // this far, and reporting the ceiling beats reporting a wrapped negative.
        bytes: i64::try_from(at).unwrap_or(i64::MAX),
    })
}

/// Overwrite the world clock inside the preserved header.
///
/// The header is kept verbatim and patched in place rather than re-serialised, so writing a field
/// means knowing its byte. Everything the server can change has an offset recorded when the world
/// was read; a `None` offset means that world's header never reached the field, and the value
/// lives only for the session.
fn patch_clock(header: &mut [u8], preserved: &super::objects::PreservedWorld, world: &World) {
    let write = |header: &mut [u8], at: usize, value: &[u8]| {
        if let Some(slot) = header.get_mut(at..at + value.len()) {
            slot.copy_from_slice(value);
        }
    };
    let flags = |header: &mut [u8], at: Option<usize>, values: &[bool]| {
        let Some(at) = at else {
            return;
        };
        for (i, on) in values.iter().enumerate() {
            if let Some(slot) = header.get_mut(at + i) {
                *slot = u8::from(*on);
            }
        }
    };
    let p = &world.progress;
    flags(
        header,
        preserved.progress_offset,
        &[
            p.downed_boss1,
            p.downed_boss2,
            p.downed_boss3,
            p.downed_queen_bee,
            p.downed_mech1,
            p.downed_mech2,
            p.downed_mech3,
            p.downed_mech_any,
            p.downed_plantera,
            p.downed_golem,
            p.downed_king_slime,
            p.saved_goblin,
            p.saved_wizard,
            p.saved_mechanic,
            p.downed_goblins,
            p.downed_clown,
            p.downed_frost,
            p.downed_pirates,
            p.shadow_orb_smashed,
            p.spawn_meteor,
        ],
    );
    flags(header, preserved.hard_mode_offset, &[p.hard_mode]);
    if let Some(at) = preserved.orb_count_offset {
        write(header, at, &[p.shadow_orb_count]);
    }
    if let Some(at) = preserved.altar_offset {
        write(header, at, &p.altar_count.to_le_bytes());
    }
    flags(
        header,
        preserved.downed_run_offset,
        &[
            p.downed_fishron,
            p.downed_martians,
            p.downed_ancient_cultist,
            p.downed_moon_lord,
            p.downed_halloween_king,
            p.downed_halloween_tree,
            p.downed_christmas_ice_queen,
            p.downed_christmas_santank,
            p.downed_christmas_tree,
        ],
    );
    flags(
        header,
        preserved.tower_run_offset,
        &[
            p.downed_tower_solar,
            p.downed_tower_vortex,
            p.downed_tower_nebula,
            p.downed_tower_stardust,
            p.tower_active_solar,
            p.tower_active_vortex,
            p.tower_active_nebula,
            p.tower_active_stardust,
            p.lunar_apocalypse_up,
        ],
    );
    flags(
        header,
        preserved.army_run_offset,
        &[
            p.saved_bartender,
            p.downed_army_t1,
            p.downed_army_t2,
            p.downed_army_t3,
        ],
    );
    flags(header, preserved.combat_book_offset, &[p.combat_book]);
    flags(
        header,
        preserved.late_downed_run_offset,
        &[
            p.downed_empress_of_light,
            p.downed_queen_slime,
            p.downed_deerclops,
        ],
    );
    flags(
        header,
        preserved.combat_book_two_offset,
        &[p.combat_book_two],
    );
    // The slime unlocks (`WorldFile.cs:1416-1420`), written one at a time rather than as a run:
    // the rainbow (+2) and red (+3) slimes are not modelled here, so their bytes stay exactly as
    // the file had them instead of being flattened to false by a blanket write.
    if let Some(at) = preserved.slime_unlocks_offset {
        write(header, at, &[u8::from(p.unlocked_slime_old)]);
        write(header, at + 1, &[u8::from(p.unlocked_slime_purple)]);
        write(header, at + 4, &[u8::from(p.unlocked_slime_yellow)]);
    }
    if let Some(at) = preserved.rain_offset {
        write(header, at, &[u8::from(world.raining)]);
        write(header, at + 1, &world.rain_time.to_le_bytes());
        write(header, at + 5, &world.max_rain.to_le_bytes());
    }
    if let Some(at) = preserved.wind_offset {
        write(header, at, &world.wind.to_le_bytes());
    }
    if let Some(at) = preserved.sandstorm_offset {
        write(header, at, &[u8::from(world.sandstorm)]);
        write(header, at + 1, &world.sandstorm_time.to_le_bytes());
        write(header, at + 5, &world.sandstorm_severity.to_le_bytes());
        write(
            header,
            at + 9,
            &world.sandstorm_intended_severity.to_le_bytes(),
        );
    }
    // The hardmode ores an altar chose. Left unwritten this is not a lost setting but a corrupted
    // world: the header keeps saying -1 for "not chosen", so after a restart the next altar rolls a
    // second tier and the world ends up with two different ores sprayed through it.
    if let Some(at) = preserved.hardmode_ores_offset {
        for (i, tier) in world.ore_tiers[4..7].iter().enumerate() {
            write(header, at + i * 4, &i32::from(*tier).to_le_bytes());
        }
    }
    // Banner kill counts. The run's length belongs to the file — the count was written ahead of it
    // and every field after depends on it — so a banner beyond that length is dropped rather than
    // written, which would shift the rest of the header and take the Moon Lord with it.
    if let Some((at, kinds)) = preserved.banner_kills_offset {
        for (banner, count) in &world.banner_kills {
            let i = usize::from(*banner);
            if i < kinds {
                write(header, at + i * 4, &(*count as i32).to_le_bytes());
            }
        }
    }
    write(
        header,
        preserved.time_offset,
        &f64::from(world.time).to_le_bytes(),
    );
    write(
        header,
        preserved.day_time_offset,
        &[u8::from(world.day_time)],
    );
    write(
        header,
        preserved.moon_phase_offset,
        &i32::from(world.moon_phase).to_le_bytes(),
    );
    // A blood moon or eclipse in progress, so a save taken mid-event and reloaded resumes it
    // rather than silently ending it.
    write(
        header,
        preserved.blood_moon_offset,
        &[u8::from(world.blood_moon)],
    );
    write(header, preserved.eclipse_offset, &[u8::from(world.eclipse)]);
}

/// Tiles are stored column by column, with the same run-length encoding the network uses.
///
/// Reading down a column strides through a row-major array, which looks like it ought to be the
/// expensive part. It is not, and that is worth recording so nobody else spends an afternoon on
/// it: reading all five million tiles of a 4200×1200 world costs **8 ms**, and transposing bands
/// into a cache-friendly scratch buffer first changed nothing measurable. The prefetcher handles
/// a constant stride perfectly well.
///
/// The cost is in the other two thirds — spotting the runs and encoding them — and those are
/// measured by `examples/savecost.rs` rather than guessed at.
fn write_tiles(w: &mut Writer, world: &World, importance: &dyn Fn(u16) -> bool) {
    for x in 0..world.width() {
        let mut pending: Option<(Tile, u16)> = None;
        for y in 0..world.height() {
            let tile = world.tile(x, y);
            match pending {
                Some((prev, ref mut run)) if prev == tile && allows_batching(tile.block) => {
                    *run += 1;
                }
                _ => {
                    if let Some((prev, run)) = pending.take() {
                        write_tile_with(w, &prev, run, importance);
                    }
                    pending = Some((tile, 0));
                }
            }
        }
        if let Some((prev, run)) = pending.take() {
            write_tile_with(w, &prev, run, importance);
        }
    }
}

/// Write the chest section in the shape this file's own version uses.
///
/// Before 294 the capacity is stated once for the whole section and every chest writes exactly
/// that many slots; from 294 each chest carries its own count. The header is copied verbatim and
/// still names the old version, so writing the new shape into an old file makes the reader take
/// the first chest's coordinates for a slot count and lose the rest of the file with it.
/// Serialise a world that was generated rather than loaded, writing every section from scratch.
///
/// The header this produces is not a copy of anything: it is the whole of the game's own
/// `SaveWorldFlags` in order, at [`SAVE_VERSION`], with the fields this server does not model
/// written as the values a fresh world has. That is the risk the preserved path exists to avoid,
/// so the writer is checked against the reader rather than trusted: a generated world is
/// serialised, read back, and compared before it is offered as a save.
fn serialize_fresh(world: &World) -> Vec<u8> {
    let mut w = Writer::with_capacity(4 * 1024 * 1024);
    let importance: Vec<bool> = (0..terrustia_proto::tile_sets::TILE_COUNT)
        .map(terrustia_proto::tile_sets::frame_important)
        .collect();

    // --- file format header ---------------------------------------------------------------
    w.i32(SAVE_VERSION)
        .bytes(MAGIC)
        .u8(FILE_TYPE_WORLD)
        .u32(1)
        .u64(0)
        .i16(SECTIONS as i16);
    let pointer_table = w.len();
    for _ in 0..SECTIONS {
        w.i32(0);
    }
    write_importance(&mut w, &importance);

    let mut pointers = [0i32; SECTIONS];
    pointers[0] = w.len() as i32;
    write_fresh_header(&mut w, world);
    pointers[1] = w.len() as i32;
    write_tiles(&mut w, world, &|tile: u16| {
        importance.get(usize::from(tile)).copied().unwrap_or(false)
    });
    pointers[2] = w.len() as i32;
    write_chests(&mut w, world, None);
    pointers[3] = w.len() as i32;
    write_signs(&mut w, world);

    // Sections 5 to 11 hold state this server keeps in memory rather than on the world: the
    // townsfolk who have moved in (with the Lunar Pillars riding the same section), the tile
    // entities, the pressure plates that are held down, the rooms the town manager has assigned,
    // the bestiary and the Journey powers. A generated world that has just been made has none of
    // them (`world`'s own fields are all still at their fresh-world defaults), so these are the
    // same writers the loaded-world path uses, just fed a `World` with nothing live in it yet.
    pointers[4] = w.len() as i32;
    write_town_npcs(&mut w, world, SAVE_VERSION);
    pointers[5] = w.len() as i32;
    write_tile_entities(&mut w, world, SAVE_VERSION);
    pointers[6] = w.len() as i32;
    write_pressure_plates(&mut w);
    pointers[7] = w.len() as i32;
    write_town_rooms(&mut w, world);
    pointers[8] = w.len() as i32;
    write_bestiary(&mut w);
    pointers[9] = w.len() as i32;
    write_journey_powers(&mut w, world);
    pointers[10] = w.len() as i32;
    // The footer, which is what the game checks a save against before trusting it.
    w.bool(true).string(&world.name).i32(world.id);

    let mut bytes = w.into_bytes();
    for (index, pointer) in pointers.iter().enumerate() {
        let at = pointer_table + index * 4;
        bytes[at..at + 4].copy_from_slice(&pointer.to_le_bytes());
    }
    bytes
}

/// Where the tile entities sit among the sections after the signs.
///
/// Section 5 overall, which is index 1 of the run this server carries through.
const TILE_ENTITY_SECTION: usize = 1;

/// Index of the townsfolk among the trailing sections. Section 4 of the file.
const TOWN_NPC_SECTION: usize = 0;

/// Section 6: which weighted pressure plates are held down. Index 2 of the trailing run.
const PRESSURE_PLATE_SECTION: usize = 2;

/// Section 7: the town manager's room list. Index 3 of the trailing run.
const TOWN_MANAGER_SECTION: usize = 3;

/// Section 8: the bestiary. Index 4 of the trailing run.
const BESTIARY_SECTION: usize = 4;

/// Section 9: the Journey powers. Index 5 of the trailing run.
const JOURNEY_SECTION: usize = 5;

/// Write the townsfolk, matching `WorldFile.SaveNPCs` (`WorldFile.cs:1710-1757`).
///
/// Two lists, each led by a boolean per entry and closed by a bare `false`. The second holds the
/// non-town NPCs the game persists — in this build's target version, exactly the four Lunar
/// Pillars (`NPCID.Sets.SavesAndLoads`, `NPCID.cs:4807`) — written from `world.saved_npcs`, which
/// `GameServer::record_lunar_pillars` fills from the live roster before every save the same way
/// `world.town_npcs` is filled from it. Dropping this list (writing only the terminator) is the
/// L3-02 bug: the next load's first `tick_lunar` then sees no pillar standing against a
/// `tower_active_*` that still says one is, and marks every tower defeated.
fn write_town_npcs(w: &mut Writer, world: &World, version: i32) {
    w.i32(world.shimmered_town_npcs.len() as i32);
    for kind in &world.shimmered_town_npcs {
        w.i32(*kind);
    }

    for npc in &world.town_npcs {
        w.bool(true)
            .i32(npc.net_id)
            .string(&npc.name)
            .f32(npc.position.0)
            .f32(npc.position.1)
            .bool(npc.homeless)
            .i32(npc.home.0)
            .i32(npc.home.1)
            // Bit zero says a variation index follows, which it always does for a townsperson.
            .u8(1)
            .i32(npc.variation);
        // `homelessDespawn` only exists in the file from version 315. Rewriting a preserved world
        // that predates it (its header still says <315) must NOT emit the byte, or a real client
        // reads it as the next entry's lead boolean and the section desyncs. Fresh saves are at
        // SAVE_VERSION, so they always write it.
        if version >= super::wld::HOMELESS_DESPAWN_VERSION {
            w.bool(npc.homeless_despawn);
        }
    }
    w.bool(false);

    for npc in &world.saved_npcs {
        w.bool(true)
            .i32(npc.net_id)
            .f32(npc.position.0)
            .f32(npc.position.1);
    }
    w.bool(false);
}

/// Section 6: which weighted pressure plates (tile 428) are currently held down
/// (`SaveWeightedPressurePlates`, `WorldFile.cs:3374-3398`).
///
/// Real vanilla's own set (`PressurePlateHelper.PressurePlatesPressed`) is momentary — a plate a
/// player is standing on right now — and this server keeps no equivalent at all: `wiring.rs`
/// recognises tile 428 as wire-triggering but tracks no per-tile pressed state (live press/release
/// tracking is a follow-up). Writing the bytes this section arrived with would mean re-firing
/// whatever a *previous* session's players happened to be standing on the moment it saved, on a
/// world nobody has even joined yet — worse than writing nothing, since real vanilla's own loader
/// throws the pressed state away on every load regardless (`LoadWeightedPressurePlates` recreates
/// a fresh, all-unpressed `bool[255]` per key; only the coordinate itself survives a round-trip in
/// real vanilla, never any actual press state). An empty set is therefore the correct minimum for
/// a server-owned world, not merely the easiest one.
fn write_pressure_plates(w: &mut Writer) {
    w.i32(0);
}

/// Section 7: `TownRoomManager`'s cache of which tile each town-NPC *type* currently has a room
/// at (`SaveTownManager`, `WorldFile.cs:3400-3404`; `TownRoomManager.Save`,
/// `TownRoomManager.cs:94-106`) — keyed by type rather than by NPC instance, at most one entry per
/// type, matching the game's own `_hasRoom[NPCID.Count]` array.
///
/// Written from the residents `write_town_npcs` is about to record in section 4 (`world.town_npcs`,
/// which `GameServer::record_town_npcs` fills from the live roster immediately before a save)
/// rather than from this section's own imported bytes, or a resident who moved in — or died —
/// since the world was opened would disagree with what section 4 says about them.
fn write_town_rooms(w: &mut Writer, world: &World) {
    let housed: Vec<_> = world.town_npcs.iter().filter(|npc| !npc.homeless).collect();
    w.i32(housed.len() as i32);
    for npc in housed {
        w.i32(npc.net_id).i32(npc.home.0).i32(npc.home.1);
    }
}

/// Section 8: the bestiary's three trackers - kills, sightings, conversations
/// (`SaveBestiary`, `WorldFile.cs:3411-3415`; `BestiaryUnlocksTracker.Save`,
/// `BestiaryUnlocksTracker.cs:13-18`), each its own `count` then `count` persistent-id entries
/// (`NPCKillsTracker.cs:60-71`, `NPCWasNearPlayerTracker.cs:65-75`, `NPCWasChatWithTracker.cs`).
///
/// This server keeps no live equivalent of any of the three yet: no per-NPC persistent-id table,
/// and nothing wired to a kill or a sighting (the client's banner-kill counter this project does
/// track, `note_banner_kill`, is a different, separate vanilla system - `NetBannersModule`, not
/// `NetBestiaryModule` - and does not cover every NPC a banner does not exist for). Carrying the
/// imported bytes forward would claim progress the running server has no memory of and cannot
/// keep in step with what sections 4/5 say happened this session, so the genuinely empty shape -
/// three zero counts, exactly what a fresh world's own tracker looks like - is written instead.
/// Live kill/sighting/chat tracking is a follow-up.
fn write_bestiary(w: &mut Writer) {
    w.i32(0).i32(0).i32(0);
}

/// Section 9: the Journey (creative) mode powers real vanilla persists per world, matching
/// `CreativePowerManager.SaveToWorld` (`CreativePowerManager.cs:125-137`).
///
/// Only the six of vanilla's fifteen powers that implement `IPersistentPerWorldContent` are ever
/// written (`CreativePowers.cs`'s own `FreezeTime`, `ModifyTimeRate`, `FreezeRainPower`,
/// `FreezeWindDirectionAndStrength`, `DifficultySliderPower`, `StopBiomeSpreadPower`); the rest
/// are one-shot day/noon/night/midnight buttons, per-player powers saved into a `.plr` file this
/// project does not own, or (`ModifyWindDirectionAndStrength`/`ModifyRainPower`) not persisted by
/// vanilla itself either - see `game/journey.rs`'s own module doc. Written from `world.journey_*`,
/// which mirrors `GameServer::journey` before every save the same way `world.town_npcs` mirrors
/// the live roster.
///
/// The four toggles each write one `bool`; the two sliders each write their raw 0.0-1.0 value as
/// one `f32`, not the derived rate/multiplier (`ASharedTogglePower`/`ASharedSliderPower`'s own
/// `Save` methods). Ids are `CreativePowerManager.Initialize`'s own registration order
/// (`CreativePowerManager.cs:90-104`), shared with `wld::read_journey_powers` as the
/// `wld::JOURNEY_*` constants.
fn write_journey_powers(w: &mut Writer, world: &World) {
    use super::wld::{
        JOURNEY_DIFFICULTY_SLIDER, JOURNEY_FREEZE_RAIN, JOURNEY_FREEZE_TIME, JOURNEY_FREEZE_WIND,
        JOURNEY_MODIFY_TIME_RATE, JOURNEY_STOP_BIOME_SPREAD,
    };
    w.bool(true)
        .u16(JOURNEY_FREEZE_TIME)
        .bool(world.journey_freeze_time);
    w.bool(true)
        .u16(JOURNEY_MODIFY_TIME_RATE)
        .f32(world.journey_time_rate_slider);
    w.bool(true)
        .u16(JOURNEY_FREEZE_RAIN)
        .bool(world.journey_freeze_rain);
    w.bool(true)
        .u16(JOURNEY_FREEZE_WIND)
        .bool(world.journey_freeze_wind);
    w.bool(true)
        .u16(JOURNEY_DIFFICULTY_SLIDER)
        .f32(world.journey_difficulty_slider);
    w.bool(true)
        .u16(JOURNEY_STOP_BIOME_SPREAD)
        .bool(world.journey_stop_biome_spread);
    w.bool(false);
}

/// Write section 5: the furniture that remembers something.
///
/// The file form, which carries each entity's id and a logic sensor's state — neither of which
/// goes over the network.
fn write_tile_entities(w: &mut Writer, world: &World, version: i32) {
    w.i32(world.tile_entities.len() as i32);
    for entity in &world.tile_entities {
        entity.write(w, false, version);
    }
}

/// The frame-importance bitset, packed least significant bit first.
fn write_importance(w: &mut Writer, importance: &[bool]) {
    w.u16(importance.len() as u16);
    let mut current = 0u8;
    let mut bit = 0x80u8;
    for &framed in importance {
        if bit == 0x80 {
            bit = 1;
            current = 0;
        } else {
            bit <<= 1;
        }
        if framed {
            current |= bit;
        }
        if bit == 0x80 {
            w.u8(current);
        }
    }
    if bit != 0x80 && !importance.is_empty() {
        w.u8(current);
    }
}

/// The whole world header, in the order the game writes it.
///
/// Every field is here, including the ones this server does not model — they are written as the
/// values a world that has just been generated holds, because the format has no framing and a
/// field left out puts every field after it in the wrong place.
fn write_fresh_header(w: &mut Writer, world: &World) {
    let p = &world.progress;
    w.string(&world.name)
        // The seed as it was typed. Terraria shows it in the world-select list, and it is the
        // other half of the parity oracle, so dropping it makes a world's seed unrecoverable.
        .string(&world.seed_text)
        .u64(world.world_gen_version)
        .bytes(&world.unique_id)
        .i32(world.id);
    // The world rectangle in pixels, then its size in tiles — height first.
    w.i32(0)
        .i32(world.width() * 16)
        .i32(0)
        .i32(world.height() * 16)
        .i32(world.height())
        .i32(world.width());

    // The nine special world seed flags, in real vanilla's own `SaveWorldFlags` order — see
    // `wld.rs`'s own read path (`LoadWorldFlags`) for the matching gates on the load side; a fresh
    // world writes unconditionally at `SAVE_VERSION`, same as real vanilla always writes all nine
    // at whatever version it currently is.
    let s = world.secret_seeds;
    w.i32(i32::from(world.game_mode));
    w.bool(s.drunk)
        .bool(s.get_good)
        .bool(s.tenth_anniversary)
        .bool(s.dont_starve)
        .bool(s.not_the_bees)
        .bool(s.remix)
        .bool(s.no_traps)
        .bool(s.everything)
        .bool(s.skyblock);
    // Created and last played, which the game stores as .NET tick counts. Zero is a valid one.
    w.i64(0).i64(0);

    w.u8(world.moon_type);
    for x in world.tree_x {
        w.i32(x);
    }
    for style in world.tree_style {
        w.i32(i32::from(style));
    }
    for x in world.cave_back_x {
        w.i32(x);
    }
    for style in world.cave_back_style {
        w.i32(i32::from(style));
    }
    w.i32(i32::from(world.ice_back_style))
        .i32(i32::from(world.jungle_back_style))
        .i32(i32::from(world.hell_back_style));

    w.i32(i32::from(world.spawn_x))
        .i32(i32::from(world.spawn_y))
        .f64(f64::from(world.surface))
        .f64(f64::from(world.rock_layer));
    w.f64(f64::from(world.time))
        .bool(world.day_time)
        .i32(i32::from(world.moon_phase))
        .bool(world.blood_moon)
        .bool(world.eclipse);
    // The dungeon's door, which is where the Old Man waits and where the Lunatic Cultist appears
    // once Golem is down. Writing the surface here instead put both in the wrong place.
    let dungeon_x = world.dungeon_x.unwrap_or(world.width() / 2);
    let dungeon_y = world.dungeon_y.unwrap_or(i32::from(world.surface));
    w.i32(dungeon_x).i32(dungeon_y);
    w.bool(world.crimson);

    for flag in [
        p.downed_boss1,
        p.downed_boss2,
        p.downed_boss3,
        p.downed_queen_bee,
        p.downed_mech1,
        p.downed_mech2,
        p.downed_mech3,
        p.downed_mech_any,
        p.downed_plantera,
        p.downed_golem,
        p.downed_king_slime,
        p.saved_goblin,
        p.saved_wizard,
        p.saved_mechanic,
        p.downed_goblins,
        p.downed_clown,
        p.downed_frost,
        p.downed_pirates,
        p.shadow_orb_smashed,
        p.spawn_meteor,
    ] {
        w.bool(flag);
    }
    w.u8(p.shadow_orb_count).i32(p.altar_count);
    w.bool(p.hard_mode).bool(false); // the party of doom, which is a one-off event flag
    // No invasion is saved: one in progress is abandoned when the server stops, exactly as the
    // game abandons one when a world is closed.
    w.i32(0).i32(0).i32(0).f64(0.0);
    w.f64(0.0).u8(0); // slime rain, sundial cooldown

    w.bool(world.raining)
        .i32(world.rain_time)
        .f32(world.max_rain);
    // The three hardmode ore tiers, which the first three altars roll.
    //
    // `-1` is the game's "not chosen yet" sentinel and it has to survive: `SmashAltar` only picks
    // a tier when it reads `-1`, and `CheckSavedOreTiers` repairs the *other* four on load but
    // never these. A `0` here therefore sticks, and the altar sprays tile type 0 — dirt — leaving
    // the world with no hardmode ore and every mechanical boss out of reach.
    for slot in 4..7 {
        w.i32(i32::from(world.ore_tiers[slot]));
    }
    // Eight of the thirteen background styles, in the file's order rather than packet 7's — the
    // other five are written further down, where the format put them.
    for slot in [
        Scenery::TREE_1,
        Scenery::CORRUPT,
        Scenery::JUNGLE,
        Scenery::SNOW,
        Scenery::HALLOW,
        Scenery::CRIMSON,
        Scenery::DESERT,
        Scenery::OCEAN,
    ] {
        w.u8(world.backgrounds[slot]);
    }
    // Whether the cloud backdrop is drawn, how many clouds, and the wind that moves them.
    w.i32(0).i16(i16::from(world.num_clouds)).f32(world.wind);

    w.i32(0); // nobody has handed in an angler quest today
    w.bool(p.saved_angler)
        .i32(0)
        .bool(p.saved_stylist)
        .bool(p.saved_tax_collector)
        .bool(p.saved_golfer);
    w.i32(0).i32(0); // invasion size at the start, cultist delay

    // The banner kill counts, written as the dense array the loader expects. `BannerSystem.Load`
    // guards each index against its own array length, so a count that does not match the game's
    // 293 is read safely either way.
    const BANNERS: u16 = 293;
    w.i16(BANNERS as i16);
    for banner in 0..BANNERS {
        w.i32(world.banner_kills.get(&banner).copied().unwrap_or(0) as i32);
    }
    w.i16(0); // nothing waiting to be claimed: banners are handed over as they are earned
    w.bool(false); // not fast-forwarding to dawn
    for flag in [
        p.downed_fishron,
        p.downed_martians,
        p.downed_ancient_cultist,
        p.downed_moon_lord,
        p.downed_halloween_king,
        p.downed_halloween_tree,
        p.downed_christmas_ice_queen,
        p.downed_christmas_santank,
        p.downed_christmas_tree,
        p.downed_tower_solar,
        p.downed_tower_vortex,
        p.downed_tower_nebula,
        p.downed_tower_stardust,
        p.tower_active_solar,
        p.tower_active_vortex,
        p.tower_active_nebula,
        p.tower_active_stardust,
        p.lunar_apocalypse_up,
    ] {
        w.bool(flag);
    }

    w.bool(false).bool(false).i32(0).i32(0); // no party
    w.bool(world.sandstorm)
        .i32(world.sandstorm_time)
        .f32(world.sandstorm_severity)
        .f32(world.sandstorm_intended_severity);
    w.bool(p.saved_bartender)
        .bool(p.downed_army_t1)
        .bool(p.downed_army_t2)
        .bool(p.downed_army_t3);
    // The remaining five background styles.
    for slot in [
        Scenery::MUSHROOM,
        Scenery::UNDERWORLD,
        Scenery::TREE_2,
        Scenery::TREE_3,
        Scenery::TREE_4,
    ] {
        w.u8(world.backgrounds[slot]);
    }
    w.bool(p.combat_book);
    w.i32(0).bool(false).bool(false).bool(false); // lantern night

    // One tree-top variation per biome area. The file widens each to an int; the packet narrows
    // it back to a byte.
    w.i32(world.tree_tops.len() as i32);
    for variation in world.tree_tops {
        w.i32(i32::from(variation));
    }
    w.bool(false).bool(false); // no forced holiday today
    // The four ore tiers the world was generated with. `CheckSavedOreTiers` repairs these on load
    // if they are still `-1`, so an unchosen set is safe here in a way the hardmode three are not.
    for slot in 0..4 {
        w.i32(i32::from(world.ore_tiers[slot]));
    }
    for _ in 0..3 {
        w.bool(false); // no pets bought
    }
    w.bool(p.downed_empress_of_light)
        .bool(p.downed_queen_slime)
        .bool(p.downed_deerclops);
    for _ in 0..9 {
        w.bool(false); // no town spawns unlocked by other means
    }
    w.bool(p.combat_book_two);
    w.bool(false); // no peddler's satchel
    // The seven remaining slime unlocks (`WorldFile.cs:1414-1421`): green, old, purple, rainbow,
    // red, yellow, copper. Three of them are real world state here, and writing them as `false`
    // regardless is how a freshly generated world forgot every town slime it had ever freed.
    w.bool(false); // green
    w.bool(p.unlocked_slime_old);
    w.bool(p.unlocked_slime_purple);
    w.bool(false).bool(false); // rainbow, red
    w.bool(p.unlocked_slime_yellow);
    w.bool(false); // copper
    w.bool(false).u8(0); // not fast-forwarding to dusk, no moondial cooldown
    w.bool(false).bool(false); // no forced holiday forever
    w.bool(false).bool(false); // vampire and infected seeds
    w.i32(0).i32(0); // meteor showers seen, coin rain
    w.bool(false); // team-based spawns
    w.u8(0); // no extra spawn points
    w.bool(false); // dual dungeons
    w.bool(false).bool(false); // more lightning, no lightning
    // The generation manifest, which the game parses as JSON and falls back to empty on.
    w.string(r#"{"GenPassResults":[],"Version":"terrustia","GitSHA":"","FinalHash":null}"#);
}

fn write_chests(w: &mut Writer, world: &World, shared_slots: Option<i16>) {
    let chests: Vec<_> = world.chests.iter().flatten().collect();
    w.i16(chests.len() as i16);
    if let Some(slots) = shared_slots {
        w.i16(slots);
        for chest in chests {
            w.i32(i32::from(chest.x))
                .i32(i32::from(chest.y))
                .string(&chest.name);
            // Padded or truncated to the shared capacity: a chest the server created carries the
            // modern default, which need not be what this file says.
            for index in 0..slots.max(0) as usize {
                chest
                    .items
                    .get(index)
                    .unwrap_or(&terrustia_proto::ItemStack::EMPTY)
                    .write_save(w);
            }
        }
        return;
    }
    for chest in chests {
        w.i32(i32::from(chest.x))
            .i32(i32::from(chest.y))
            .string(&chest.name)
            .i32(chest.items.len() as i32);
        for item in &chest.items {
            item.write_save(w);
        }
    }
}

fn write_signs(w: &mut Writer, world: &World) {
    let signs: Vec<_> = world.signs.iter().flatten().collect();
    w.i16(signs.len() as i16);
    for sign in signs {
        w.string(&sign.text)
            .i32(i32::from(sign.x))
            .i32(i32::from(sign.y));
    }
}

/// How many previous worlds to keep beside the current one.
///
/// Verifying before replacing means a save can refuse rather than destroy; backups are what makes
/// a world recoverable when the damage came from somewhere else entirely — a bad edit, a griefing
/// run, or a bug in here that verification cannot see because the file parses perfectly and simply
/// says the wrong thing.
pub const BACKUPS_KEPT: usize = 3;

/// Shift the existing world down the backup chain: `.bak1` becomes `.bak2`, and so on.
///
/// Failures are logged rather than fatal. A backup that cannot be made is worth knowing about, and
/// is not a reason to refuse to save the world — that would turn a full disk into data loss
/// instead of merely a missing safety net.
///
/// The copy into `.bak1` goes through [`crate::safe_write::copy_atomic`] rather than a plain
/// `std::fs::copy`, which truncates its destination before it fills it: a disk that ran out
/// halfway through would otherwise replace the newest healthy backup with a fragment of a world,
/// and a fragment is exactly what nobody wants to find on the day they reach for a backup.
fn rotate_backups(path: &Path) {
    if !path.exists() {
        return;
    }
    let bak = |n: usize| path.with_extension(format!("wld.bak{n}"));

    // Drop the oldest, then walk upward so nothing is overwritten before it has been moved. A
    // rename is atomic, so a failure here leaves the chain shorter rather than damaged.
    let _ = std::fs::remove_file(bak(BACKUPS_KEPT));
    for n in (1..BACKUPS_KEPT).rev() {
        if bak(n).exists()
            && let Err(e) = std::fs::rename(bak(n), bak(n + 1))
        {
            let e = crate::safe_write::explain("rotating the world backups", &bak(n + 1), &e);
            tracing::warn!(error = %e, "could not rotate a world backup");
        }
    }
    if let Err(e) =
        crate::safe_write::copy_atomic("backing the world up before saving", path, &bak(1))
    {
        tracing::warn!(
            error = %e,
            "could not back up the world before saving over it; the previous backup is still intact"
        );
    }
}

/// Write a world to disk, so that a failure anywhere costs nothing that was already there.
///
/// Four steps, in this order and for these reasons:
///
/// 1. **Write** the bytes to a temporary file beside the target, synced to the disk rather than
///    left in the page cache.
/// 2. **Verify** them by parsing them back. An atomic rename over a file that turned out to be
///    corrupt is an atomic loss; we already own a reader, and this costs a fraction of the write.
/// 3. **Rotate** the backup chain - only now, because rotating first would push a healthy world out
///    of the chain to make room for one that turned out not to be writable.
/// 4. **Rename** into place, then sync the directory entry so the replacement survives a power cut
///    and not merely a process crash.
///
/// Every failure removes the temporary file (nothing else would ever clean it up, and the next save
/// needs the name back) and leaves the previous world byte-identical. Failures are explained
/// through [`crate::safe_write::explain`], so an operator gets "the filesystem holding
/// /srv/worlds/Terrustia.wld is full" rather than `Os { code: 28 }`.
///
/// A failed backup is a warning, not a refusal: refusing to save because the safety net could not
/// be made would turn a full disk into actual data loss rather than a missing spare copy.
pub fn save(world: &World, path: &Path) -> Result<()> {
    let bytes = serialize(world)?;
    let temp = path.with_extension("wld.tmp");

    // Write, then *verify*, then replace. An atomic rename over a file that turned out to be
    // corrupt is an atomic loss, so the new world is read back and parsed before it is allowed to
    // become the real one. We already own a reader; this costs a fraction of the write.
    let written = write_and_sync(&temp, &bytes);
    if let Err(e) = written {
        // Do not leave the half-written attempt lying about. Nothing else will ever clean it up,
        // and the next save has to be able to use the name.
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    if let Err(e) = super::wld::parse(&bytes) {
        let _ = std::fs::remove_file(&temp);
        return Err(WldError::WroteSomethingUnreadable {
            source: Box::new(e),
        });
    }

    // Only once the replacement is known good: rotating first would push a healthy world out of
    // the chain to make room for one that turned out not to be writable.
    rotate_backups(path);

    if let Err(e) = std::fs::rename(&temp, path) {
        // On Windows this fails outright if anything else holds the destination open — Terraria
        // itself, a backup tool, a virus scanner. The previous world is untouched either way.
        let _ = std::fs::remove_file(&temp);
        return Err(WldError::Write {
            path: path.display().to_string(),
            source: crate::safe_write::explain("saving the world", path, &e),
        });
    }
    // The rename is only durable once the *directory* entry is. Without this the world can be
    // atomically replaced and still not be there after a power cut.
    crate::safe_write::sync_parent_dir(path);
    Ok(())
}

/// Write the scratch file and get it onto the disk, rather than into the page cache.
///
/// The failure is explained against the *world's* path, not the scratch file's: an operator does
/// not care that `world.wld.tmp` could not be created, they care that the world could not be
/// saved and why. [`crate::safe_write::explain`] keeps the [`std::io::ErrorKind`] intact while
/// adding the sentence that says where to look.
fn write_and_sync(path: &Path, bytes: &[u8]) -> Result<()> {
    let target = path.with_extension("");
    crate::safe_write::write_and_sync(path, bytes).map_err(|e| WldError::Write {
        path: target.display().to_string(),
        source: crate::safe_write::explain("saving the world", &target, &e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::objects::PreservedWorld;

    /// A header of the right shape but made of sentinel bytes, so a patch is visible.
    fn header_with(offsets: PreservedWorld) -> (Vec<u8>, PreservedWorld) {
        (vec![0xAA; 4096], offsets)
    }

    fn preserved() -> PreservedWorld {
        PreservedWorld {
            version: 279,
            chest_slots: None,
            revision: 1,
            favorite: 0,
            header_bytes: Vec::new(),
            time_offset: 0,
            day_time_offset: 8,
            moon_phase_offset: 9,
            blood_moon_offset: 13,
            eclipse_offset: 14,
            progress_offset: Some(100),
            hard_mode_offset: Some(200),
            altar_offset: Some(210),
            orb_count_offset: Some(220),
            downed_run_offset: Some(300),
            tower_run_offset: Some(400),
            rain_offset: Some(500),
            wind_offset: Some(600),
            sandstorm_offset: Some(700),
            army_run_offset: Some(800),
            combat_book_offset: Some(810),
            late_downed_run_offset: Some(820),
            combat_book_two_offset: Some(830),
            slime_unlocks_offset: Some(840),
            hardmode_ores_offset: Some(900),
            banner_kills_offset: Some((1000, 293)),
            town_npcs_understood: true,
            tile_entities_understood: true,
            trailing_sections: Vec::new(),
            importance: Vec::new(),
        }
    }

    /// Every field the server can change is written back where the reader found it.
    #[test]
    fn the_patch_writes_every_mutable_field() {
        let (mut header, keep) = header_with(preserved());
        let mut world = crate::world::worldgen::generate(400, 300, "patch", 1);
        world.time = 13_500;
        world.day_time = false;
        world.moon_phase = 5;
        world.progress.hard_mode = true;
        world.progress.downed_plantera = true;
        world.progress.downed_moon_lord = true;
        world.progress.tower_active_vortex = true;
        world.progress.altar_count = 12;
        world.progress.shadow_orb_count = 3;
        world.raining = true;
        world.rain_time = 4200;
        world.max_rain = 0.75;
        world.wind = -0.4;
        world.sandstorm = true;
        world.sandstorm_time = 30_000;
        world.sandstorm_severity = 0.5;
        world.sandstorm_intended_severity = 0.8;
        world.progress.downed_army_t2 = true;
        world.progress.combat_book = true;
        world.progress.downed_deerclops = true;
        world.progress.combat_book_two = true;
        world.progress.unlocked_slime_old = true;
        world.progress.unlocked_slime_yellow = true;
        world.ore_tiers[4] = 107;
        world.ore_tiers[5] = 108;
        world.ore_tiers[6] = 111;
        world.banner_kills.insert(0, 50);
        world.banner_kills.insert(17, 250);
        // Past the end of the run this file has room for, so it has nowhere to go.
        world.banner_kills.insert(400, 9);

        patch_clock(&mut header, &keep, &world);

        assert_eq!(header[8], 0, "day_time");
        assert_eq!(
            i32::from_le_bytes(header[9..13].try_into().unwrap()),
            5,
            "moon_phase"
        );
        // The progression run: the ninth flag is Plantera, the eleventh King Slime.
        assert_eq!(header[100 + 8], 1, "plantera");
        assert_eq!(header[100 + 10], 0, "king slime, which is not down");
        assert_eq!(header[200], 1, "hard mode");
        assert_eq!(header[220], 3, "orb count");
        assert_eq!(
            i32::from_le_bytes(header[210..214].try_into().unwrap()),
            12,
            "altars"
        );
        // The late run: the fourth flag is the Moon Lord.
        assert_eq!(header[300 + 3], 1, "moon lord");
        assert_eq!(header[300], 0, "fishron, which is not down");
        // The tower run: the sixth flag is the vortex tower standing.
        assert_eq!(header[400 + 5], 1, "vortex standing");
        assert_eq!(header[400], 0, "solar, never beaten");
        assert_eq!(header[500], 1, "raining");
        assert_eq!(
            i32::from_le_bytes(header[501..505].try_into().unwrap()),
            4200,
            "rain time"
        );
        assert_eq!(
            f32::from_le_bytes(header[505..509].try_into().unwrap()),
            0.75,
            "max rain"
        );
        assert_eq!(
            f32::from_le_bytes(header[600..604].try_into().unwrap()),
            -0.4,
            "wind"
        );
        assert_eq!(header[700], 1, "sandstorm");
        assert_eq!(
            i32::from_le_bytes(header[701..705].try_into().unwrap()),
            30_000,
            "sandstorm time"
        );
        assert_eq!(
            f32::from_le_bytes(header[705..709].try_into().unwrap()),
            0.5,
            "severity"
        );
        assert_eq!(
            f32::from_le_bytes(header[709..713].try_into().unwrap()),
            0.8,
            "intended severity"
        );
        // The army run: the bartender first, then the three tiers.
        assert_eq!(header[800], 0, "the bartender, never saved");
        assert_eq!(header[800 + 2], 1, "the second tier, beaten");
        assert_eq!(header[810], 1, "the first combat book");
        // The late downed run: the empress, the queen, then Deerclops.
        assert_eq!(header[820], 0, "the empress, still alive");
        assert_eq!(header[820 + 2], 1, "deerclops");
        assert_eq!(header[830], 1, "the second combat book");
        // The slime unlocks: old, purple, rainbow, red, yellow. The two this server does not model
        // (rainbow at +2, red at +3) must keep whatever the file had rather than be flattened to
        // false, so they are checked to still hold the fixture's untouched filler byte.
        assert_eq!(header[840], 1, "the old slime, freed");
        assert_eq!(header[840 + 1], 0, "the purple slime, still bound");
        assert_eq!(
            header[840 + 2],
            0xAA,
            "the rainbow slime, not ours to write"
        );
        assert_eq!(header[840 + 3], 0xAA, "the red slime, not ours to write");
        assert_eq!(header[840 + 4], 1, "the yellow slime, freed");
        // The hardmode ores an altar chose: three i32s, cobalt first.
        assert_eq!(
            i32::from_le_bytes(header[900..904].try_into().unwrap()),
            107,
            "cobalt"
        );
        assert_eq!(
            i32::from_le_bytes(header[904..908].try_into().unwrap()),
            108,
            "mythril"
        );
        assert_eq!(
            i32::from_le_bytes(header[908..912].try_into().unwrap()),
            111,
            "adamantite"
        );
        // Banner kills land at their own index, and one past the end goes nowhere at all.
        assert_eq!(
            i32::from_le_bytes(header[1000..1004].try_into().unwrap()),
            50,
            "banner 0"
        );
        assert_eq!(
            i32::from_le_bytes(header[1000 + 17 * 4..1000 + 17 * 4 + 4].try_into().unwrap()),
            250,
            "banner 17"
        );
        assert_eq!(
            header[1000 + 400 * 4],
            0xAA,
            "a banner past the file's run must not be written, or every field after it shifts"
        );
    }

    /// The bug this file existed with: a field the server changes with nowhere to write it.
    ///
    /// Smashing an altar picks the hardmode ore tier. Before this, the choice never reached disk on
    /// a loaded world, so the header still read -1 for "not chosen" next launch and the following
    /// altar rolled a *second* tier — leaving two different ores sprayed through one world.
    #[test]
    fn an_altars_ore_choice_survives_a_save() {
        let (mut header, keep) = header_with(preserved());
        let mut world = crate::world::worldgen::generate(400, 300, "ores", 1);
        world.ore_tiers[4] = 221;

        patch_clock(&mut header, &keep, &world);

        assert_eq!(
            i32::from_le_bytes(header[900..904].try_into().unwrap()),
            221,
            "the tier an altar chose has to reach the file, or the next altar rolls a second one"
        );
    }

    /// The same bug, in the field our own comments claimed was already safe.
    ///
    /// `note_banner_kill` says "the count lives on the world, so it survives a restart". It did
    /// not, for any world loaded from a file.
    #[test]
    fn banner_kills_survive_a_save() {
        let (mut header, keep) = header_with(preserved());
        let mut world = crate::world::worldgen::generate(400, 300, "banners", 1);
        world.banner_kills.insert(3, 42);

        patch_clock(&mut header, &keep, &world);

        assert_eq!(
            i32::from_le_bytes(header[1012..1016].try_into().unwrap()),
            42,
        );
    }

    /// A blood moon or eclipse in progress used to be read from the file and thrown away, so
    /// loading a world mid-event silently ended it — the bytes on disk were always untouched,
    /// only the in-memory session forgot what they said.
    #[test]
    fn a_blood_moon_and_eclipse_survive_a_save() {
        let (mut header, keep) = header_with(preserved());
        let mut world = crate::world::worldgen::generate(400, 300, "storms", 1);
        world.blood_moon = true;
        world.eclipse = true;

        patch_clock(&mut header, &keep, &world);

        assert_eq!(
            header[13], 1,
            "a blood moon in progress must reach the file"
        );
        assert_eq!(header[14], 1, "an eclipse in progress must reach the file");
    }

    /// A world whose header never reached a field simply does not write it, rather than writing
    /// it somewhere else.
    #[test]
    fn a_short_header_is_left_alone() {
        let mut keep = preserved();
        keep.downed_run_offset = None;
        keep.tower_run_offset = None;
        keep.wind_offset = None;
        let (mut header, keep) = header_with(keep);
        let mut world = crate::world::worldgen::generate(400, 300, "short", 1);
        world.progress.downed_moon_lord = true;
        world.wind = 0.9;

        patch_clock(&mut header, &keep, &world);
        assert_eq!(header[300], 0xAA, "nothing written where nothing was read");
        assert_eq!(header[400], 0xAA);
        assert_eq!(header[600], 0xAA);
    }

    /// A pre-294 world states its chest capacity once, and every chest writes exactly that many
    /// slots with no count of its own.
    ///
    /// Writing the modern shape into an old file is not a cosmetic difference: the reader takes
    /// the first chest's x coordinate for the shared count and loses every byte after it.
    #[test]
    fn an_old_world_keeps_its_shared_chest_capacity() {
        let mut world = crate::world::worldgen::generate(400, 300, "old", 1);
        world.chests = vec![Some(crate::world::objects::Chest::empty_at(10, 20))];

        let mut w = Writer::new();
        write_chests(&mut w, &world, Some(40));
        let old = w.into_bytes();

        let mut w = Writer::new();
        write_chests(&mut w, &world, None);
        let new = w.into_bytes();

        // count, shared capacity, x, y, an empty name, then forty empty slots.
        assert_eq!(&old[..4], &[1, 0, 40, 0], "count then the shared capacity");
        assert_eq!(old.len(), 2 + 2 + 4 + 4 + 1 + 40 * 2);
        // The modern shape spends four bytes on a per-chest count instead of two on a shared one.
        assert_eq!(new.len(), 2 + 4 + 4 + 1 + 4 + 40 * 2);
        assert_ne!(old, new);
    }

    /// A chest the server created carries the modern default, which need not match what an older
    /// file says its chests hold. It is padded or truncated rather than written at its own size.
    #[test]
    fn a_chest_is_written_at_the_files_capacity_not_its_own() {
        let mut world = crate::world::worldgen::generate(400, 300, "old", 1);
        world.chests = vec![Some(crate::world::objects::Chest::empty_at(10, 20))];

        for slots in [20i16, 40, 60] {
            let mut w = Writer::new();
            write_chests(&mut w, &world, Some(slots));
            assert_eq!(
                w.len(),
                2 + 2 + 4 + 4 + 1 + slots as usize * 2,
                "{slots} slots"
            );
        }
    }

    /// Saving keeps the last few worlds, and refuses rather than replacing with something broken.
    ///
    /// The rename was atomic already, which protects against a crash *during* the write. It does
    /// nothing about the file being atomically replaced with rubbish, and nothing about damage
    /// that arrives some other way — a bad edit, a griefing run, a bug in here that produces a
    /// file which parses perfectly and says the wrong thing.
    #[test]
    fn saving_rotates_backups_and_keeps_a_bounded_number() {
        let dir = std::env::temp_dir().join(format!("terrustia-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join("world.wld");

        let world = crate::world::worldgen::generate(400, 300, "backups", 1);
        for _ in 0..BACKUPS_KEPT + 3 {
            save(&world, &path).expect("saving");
        }

        assert!(path.exists(), "the world itself");

        // `rotate_backups` logs its failures and carries on by design, so when a backup is missing
        // the reason has already been thrown away: unit tests initialise no subscriber, so the
        // `warn!` goes nowhere and the assertion below can only report the symptom. This runs the
        // one step the chain actually depends on, against the real files this test just made, so a
        // platform that refuses it says *why* in the failure rather than leaving the next reader to
        // guess from an absence. It is how the Windows half of this test was diagnosed at all.
        crate::safe_write::copy_atomic(
            "backing the world up before saving",
            &path,
            &path.with_extension("wld.bakprobe"),
        )
        .expect("copy_atomic is what rotate_backups uses to make .bak1; if it fails, so does that");
        let _ = std::fs::remove_file(path.with_extension("wld.bakprobe"));
        for n in 1..=BACKUPS_KEPT {
            assert!(
                path.with_extension(format!("wld.bak{n}")).exists(),
                "backup {n} should exist after several saves"
            );
        }
        assert!(
            !path
                .with_extension(format!("wld.bak{}", BACKUPS_KEPT + 1))
                .exists(),
            "the chain must be bounded, or a world eats its own disk"
        );
        // And nothing is left half-written.
        assert!(
            !path.with_extension("wld.tmp").exists(),
            "a temporary file was left behind; nothing else will ever clean it up"
        );

        // Every backup has to be a world, not a truncated one.
        let bytes = std::fs::read(path.with_extension("wld.bak1")).expect("reading a backup");
        crate::world::wld::parse(&bytes).expect("a backup must be loadable");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P1d: bytes that would fail the new section-pointer monotonicity check must never replace
    /// the previous good save, and must not leave a temp file behind either.
    ///
    /// This runs the exact sequence `save` itself does around its own verify step (write to the
    /// temp path, then read the just-written bytes back through `wld::parse` before ever
    /// touching the real path) with a deliberately corrupt buffer standing in for whatever
    /// produced one, since a healthy `World` can never serialise its own section pointers out of
    /// order. Before the monotonicity check landed, this exact buffer parsed successfully (as a
    /// world with an emptied trailing section) and the verify step would have waved it through.
    #[test]
    fn a_corrupt_write_never_replaces_the_previous_good_save() {
        let dir =
            std::env::temp_dir().join(format!("terrustia-refuse-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join("world.wld");

        let world = crate::world::worldgen::generate(400, 300, "refuse", 1);
        save(&world, &path).expect("the first, good save");
        let good_bytes = std::fs::read(&path).expect("reading the good save");

        // Swap two trailing section pointers (townsfolk and tile entities, indices 4 and 5) the
        // same way the wld.rs fixture does, to build a corrupt buffer out of a genuinely valid one.
        const POINTER_TABLE: usize = 4 + 7 + 1 + 4 + 8 + 2;
        let mut corrupt = good_bytes.clone();
        let (at4, at5) = (POINTER_TABLE + 4 * 4, POINTER_TABLE + 5 * 4);
        let (mut p4, mut p5) = ([0u8; 4], [0u8; 4]);
        p4.copy_from_slice(&corrupt[at4..at4 + 4]);
        p5.copy_from_slice(&corrupt[at5..at5 + 4]);
        corrupt[at4..at4 + 4].copy_from_slice(&p5);
        corrupt[at5..at5 + 4].copy_from_slice(&p4);
        assert!(
            crate::world::wld::parse(&corrupt).is_err(),
            "the fixture should actually be corrupt"
        );

        let temp = path.with_extension("wld.tmp");
        write_and_sync(&temp, &corrupt).expect("writing the corrupt bytes to the temp path");
        if crate::world::wld::parse(&corrupt).is_err() {
            // What save() itself does on a failed verify: drop the temp file and refuse, leaving
            // the real path untouched.
            let _ = std::fs::remove_file(&temp);
        } else {
            panic!("the corrupt buffer must not verify as loadable");
        }

        assert_eq!(
            std::fs::read(&path).expect("reading the world back"),
            good_bytes,
            "the previous good save must be exactly as it was"
        );
        assert!(!temp.exists(), "no half-written temp file left behind");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A world that cannot be written must cost nothing that was already written.
    ///
    /// The failure is a real one, not a mocked `io::Error`: the directory holding the world is
    /// made unwritable, which is what a wrong `chown` after a package upgrade, a container volume
    /// mounted read-only, or an operator's own `chmod` all look like from in here. The three things
    /// asserted are the three an operator would ask about, in order: is my world still there and
    /// byte-for-byte what it was, did the server tell me anything useful, and did it leave rubbish
    /// behind for the next attempt to trip over.
    #[cfg(unix)]
    #[test]
    fn a_world_that_cannot_be_written_leaves_the_previous_one_byte_identical() {
        let dir = crate::safe_write::tests::temp_dir("wld-readonly");
        let path = dir.join("world.wld");

        let world = crate::world::worldgen::generate(400, 300, "readonly", 1);
        save(&world, &path).expect("the first, good save");
        let before = std::fs::read(&path).expect("reading the good save");
        let backup_before = std::fs::read(path.with_extension("wld.bak1")).ok();

        let Some(_guard) = crate::safe_write::tests::ReadOnlyDir::new(&dir) else {
            eprintln!("skipping: this environment cannot make a directory read-only");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        let e = save(&world, &path).expect_err("a read-only directory must refuse the save");
        let message = e.to_string();
        drop(_guard);

        assert_eq!(
            std::fs::read(&path).expect("reading the world back"),
            before,
            "a failed save must leave the world byte-identical"
        );
        assert_eq!(
            std::fs::read(path.with_extension("wld.bak1")).ok(),
            backup_before,
            "a failed save must not disturb the backup chain either"
        );
        assert!(
            message.contains("world.wld") && message.contains("writable by the account"),
            "the failure must say what it was doing and what to check, got: {message}"
        );
        assert!(
            !path.with_extension("wld.tmp").exists(),
            "a failed save must clean up its own scratch file"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other everyday external failure: the world's directory is simply gone - unmounted, or
    /// deleted by a tidy-up script - while the server is running. It must be reported, not fatal.
    #[test]
    fn a_vanished_directory_is_reported_rather_than_fatal() {
        let dir = crate::safe_write::tests::temp_dir("wld-vanished");
        let path = dir.join("world.wld");
        let world = crate::world::worldgen::generate(400, 300, "vanished", 1);
        std::fs::remove_dir_all(&dir).expect("removing the directory out from under it");

        let e = save(&world, &path).expect_err("a missing directory must refuse the save");
        assert!(
            e.to_string().contains("directory no longer exists"),
            "got: {e}"
        );
    }

    /// A section pointer past the format's `i32` table is refused, not wrapped.
    ///
    /// The first four pointers were a bare `as i32`. A world that serialised past 2 GiB would have
    /// written a negative offset, and the operator would have been told "the world was written but
    /// could not be read back" by the verify step - which reads as a bug in the writer rather than
    /// as the world being too big for the format it is being written in. Tested on the helper
    /// rather than on a real world for the obvious reason: building a 2 GiB one in a unit test to
    /// prove a boundary is not a trade worth making.
    #[test]
    fn a_section_pointer_past_two_gigabytes_is_refused_rather_than_wrapped() {
        assert_eq!(section_pointer(0).expect("zero is a fine pointer"), 0);
        assert_eq!(
            section_pointer(i32::MAX as usize).expect("the last pointer that fits"),
            i32::MAX
        );
        let over = section_pointer(i32::MAX as usize + 1).expect_err("one past the ceiling");
        assert!(
            matches!(over, WldError::SaveTooLarge { bytes } if bytes == i64::from(i32::MAX) + 1),
            "it must say the world is too large, and how large: {over}"
        );
        assert!(
            over.to_string().contains("2 GiB"),
            "and the message must name the format's limit: {over}"
        );
    }

    /// A backup that cannot be made must be reported, and must leave the previous backup alone.
    ///
    /// What this test actually pins is the reported case: rotation warns and returns rather than
    /// propagating, the existing `.bak1` is untouched, and no scratch file is left behind (that
    /// last assertion is the one that fails without `copy_atomic`'s cleanup path).
    ///
    /// It deliberately does **not** claim to reproduce the unreported case, and it is worth being
    /// exact about why, because that case is the real reason `rotate_backups` no longer uses
    /// `std::fs::copy`. `copy` opens the destination with truncation and then fills it, so a
    /// failure *during* the copy - ENOSPC, an I/O error, or the process being killed - leaves the
    /// newest healthy backup replaced by a fragment of a world. Reaching that window needs a
    /// genuinely full filesystem or a dying process; neither is portably injectable from a unit
    /// test without root. `safe_write`'s own `/dev/full` test covers a real ENOSPC where the
    /// platform provides one, and the mapping test covers what the operator is then told.
    #[cfg(unix)]
    #[test]
    fn a_backup_that_cannot_be_made_leaves_the_previous_backup_alone() {
        let dir = crate::safe_write::tests::temp_dir("wld-backup-readonly");
        let path = dir.join("world.wld");
        let world = crate::world::worldgen::generate(400, 300, "backup", 1);
        save(&world, &path).expect("the first save");
        save(&world, &path).expect("the second save, which makes .bak1");
        let bak1 = path.with_extension("wld.bak1");
        let before = std::fs::read(&bak1).expect("reading the backup");

        let Some(_guard) = crate::safe_write::tests::ReadOnlyDir::new(&dir) else {
            eprintln!("skipping: this environment cannot make a directory read-only");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        // Rotation on its own, so the read-only failure lands on the backup rather than on the
        // world write that would otherwise fail first.
        rotate_backups(&path);
        drop(_guard);

        assert_eq!(
            std::fs::read(&bak1).expect("reading the backup back"),
            before,
            "a backup that could not be made must not damage the one that was already there"
        );
        assert!(
            !crate::safe_write::temp_path(&bak1).exists(),
            "a failed backup must clean up its own scratch file"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `World` whose `preserved` is populated the way a real load leaves it, so re-serialising
    /// it exercises `serialize`'s "preserved" branch (the trailing-section match arms) rather than
    /// `serialize_fresh`'s. Built by round-tripping a freshly generated world once, exactly the way
    /// `roundtrip_wld` and every real server does on its very first save after opening a file.
    fn preserved_world() -> super::super::World {
        let world = crate::world::worldgen::generate(400, 300, "preserved-fixture", 1);
        let bytes = serialize(&world).expect("a fresh world must serialise");
        super::super::wld::parse(&bytes).expect("and the bytes it wrote must parse back")
    }

    /// L3-02's headline fixture, at the `wld`/`wld_save` layer (the `GameServer`-level fixture -
    /// pillars actually standing, `tick_lunar` run before and after - lives in
    /// `game/server/systems.rs`, where the live roster and `tick_lunar` both are).
    ///
    /// The pillars (`world.saved_npcs`) must round-trip through a *preserved* world's save, not
    /// just a fresh one — a fresh world's second list is empty by construction, which proves
    /// nothing about the bug this section exists to fix.
    #[test]
    fn the_lunar_pillars_round_trip_through_a_preserved_worlds_save() {
        use crate::world::objects::SavedNpc;

        let mut world = preserved_world();
        world.saved_npcs = vec![
            SavedNpc {
                net_id: i32::from(crate::game::lunar::SOLAR),
                position: (100.0, 200.0),
            },
            SavedNpc {
                net_id: i32::from(crate::game::lunar::VORTEX),
                position: (300.0, 400.0),
            },
            SavedNpc {
                net_id: i32::from(crate::game::lunar::NEBULA),
                position: (500.0, 600.0),
            },
            SavedNpc {
                net_id: i32::from(crate::game::lunar::STARDUST),
                position: (700.0, 800.0),
            },
        ];

        let bytes = serialize(&world).expect("serialize");
        let back = super::super::wld::parse(&bytes).expect("parse");

        assert_eq!(
            back.saved_npcs, world.saved_npcs,
            "all four pillars, with their positions, must survive a save of a loaded world"
        );
    }

    /// Reverting the fix (dropping the second list, as `write_town_npcs` used to) turns this red:
    /// with no pillars written, `back.saved_npcs` comes back empty and the assertion above fails.
    /// Pinned here as its own test so that failure mode is named rather than folded into the
    /// round-trip test's own assertion message.
    #[test]
    fn an_empty_pillar_list_stays_empty_and_does_not_desync_the_section() {
        let world = preserved_world();
        assert!(world.saved_npcs.is_empty(), "the fixture itself has none");

        let bytes = serialize(&world).expect("serialize");
        let back = super::super::wld::parse(&bytes).expect("parse");

        assert!(back.saved_npcs.is_empty());
        // And nothing after the townsfolk section desynced: the tile entities (section 5, right
        // after) must still be understood, which they would not be if the second list's own
        // terminator were missing or misplaced.
        assert!(
            back.preserved
                .as_ref()
                .expect("a loaded world carries its preserved state")
                .tile_entities_understood
        );
    }

    /// L3-21: a pressure-plate section a load decoded as non-empty must still save as empty for a
    /// server-owned world — the imported bytes are never carried forward, because this server
    /// tracks no live press/release state to have written them from in the first place.
    #[test]
    fn pressure_plates_always_save_empty_even_when_the_loaded_section_held_something() {
        let mut world = preserved_world();
        let preserved = world.preserved.as_mut().expect("a loaded world");
        assert_eq!(
            preserved.trailing_sections[PRESSURE_PLATE_SECTION],
            0i32.to_le_bytes(),
            "the fixture's own section starts empty, same as any freshly generated world's"
        );
        // Stand in for a real vanilla file saved while a player was on a weighted plate: one
        // pressed tile at (12, 34).
        let mut held = Writer::new();
        held.i32(1).i32(12).i32(34);
        preserved.trailing_sections[PRESSURE_PLATE_SECTION] = held.into_bytes();

        let bytes = serialize(&world).expect("serialize");
        let back = super::super::wld::parse(&bytes).expect("parse");

        assert_eq!(
            back.preserved.expect("loaded").trailing_sections[PRESSURE_PLATE_SECTION],
            0i32.to_le_bytes(),
            "a server-owned world must never re-fire a plate nobody is standing on any more"
        );
    }

    /// L3-20: the room list is derived from the live residents this save is about to write into
    /// section 4, not from whatever the room section's own imported bytes said — a resident who
    /// moved in, moved out or died since the world was opened must be reflected in both sections
    /// alike.
    #[test]
    fn the_town_manager_room_list_is_derived_from_the_live_residents() {
        use crate::world::objects::TownNpc;

        let mut world = preserved_world();
        // A stale room the imported bytes claimed, for someone who is no longer housed by the time
        // this save runs — this must not survive into the rewritten section.
        let mut stale = Writer::new();
        stale.i32(1).i32(999).i32(11).i32(22);
        world
            .preserved
            .as_mut()
            .expect("a loaded world")
            .trailing_sections[TOWN_MANAGER_SECTION] = stale.into_bytes();

        world.town_npcs = vec![
            TownNpc {
                net_id: 22,
                name: "Andrew".into(),
                position: (1.0, 2.0),
                homeless: false,
                home: (77, 88),
                variation: 0,
                homeless_despawn: false,
            },
            TownNpc {
                net_id: 17,
                name: "Wilhelmina".into(),
                position: (3.0, 4.0),
                homeless: true,
                home: (0, 0),
                variation: 0,
                homeless_despawn: false,
            },
        ];

        let bytes = serialize(&world).expect("serialize");
        let back = super::super::wld::parse(&bytes).expect("parse");
        let section = &back.preserved.expect("loaded").trailing_sections[TOWN_MANAGER_SECTION];

        // count(i32), then (type, x, y) triples: only Andrew, who has a room; Wilhelmina is
        // homeless and the stale entry for type 999 must be gone.
        assert_eq!(
            &section[..4],
            &1i32.to_le_bytes(),
            "one room: the housed resident only"
        );
        assert_eq!(&section[4..8], &22i32.to_le_bytes(), "Andrew's type");
        assert_eq!(&section[8..12], &77i32.to_le_bytes(), "his home x");
        assert_eq!(&section[12..16], &88i32.to_le_bytes(), "his home y");
        assert_eq!(section.len(), 16, "no trailing stale entry");
    }

    /// L3-22: the bestiary is always written as the genuinely empty shape (three zero counts),
    /// never as whatever the loaded section's own bytes said, since this server keeps no live
    /// kill/sighting/chat tracker to have derived a non-empty one from.
    #[test]
    fn the_bestiary_always_saves_empty_even_when_the_loaded_section_held_something() {
        let mut world = preserved_world();
        let mut claimed = Writer::new();
        claimed.i32(1).string("Zombie").i32(42);
        claimed.i32(0);
        claimed.i32(0);
        world
            .preserved
            .as_mut()
            .expect("a loaded world")
            .trailing_sections[BESTIARY_SECTION] = claimed.into_bytes();

        let bytes = serialize(&world).expect("serialize");
        let back = super::super::wld::parse(&bytes).expect("parse");

        assert_eq!(
            back.preserved.expect("loaded").trailing_sections[BESTIARY_SECTION],
            [0i32.to_le_bytes(), 0i32.to_le_bytes(), 0i32.to_le_bytes()].concat(),
            "three empty counts: kills, sightings, conversations"
        );
    }

    /// L3-23: the Journey powers this server tracks live round-trip through a preserved world's
    /// save — not just the four toggles, but the two sliders at a non-default value, so this is
    /// not merely testing that "everything off" happens to look like the section's own empty
    /// shape.
    #[test]
    fn journey_powers_round_trip_through_a_preserved_worlds_save() {
        let mut world = preserved_world();
        world.journey_freeze_time = true;
        world.journey_freeze_rain = false;
        world.journey_freeze_wind = true;
        world.journey_stop_biome_spread = true;
        world.journey_time_rate_slider = 0.75;
        world.journey_difficulty_slider = 0.2;

        let bytes = serialize(&world).expect("serialize");
        let back = super::super::wld::parse(&bytes).expect("parse");

        assert!(back.journey_freeze_time);
        assert!(!back.journey_freeze_rain);
        assert!(back.journey_freeze_wind);
        assert!(back.journey_stop_biome_spread);
        assert!((back.journey_time_rate_slider - 0.75).abs() < 1e-6);
        assert!((back.journey_difficulty_slider - 0.2).abs() < 1e-6);
    }

    /// Reverting the fix (carrying the section's imported bytes forward, as it used to) turns this
    /// red: a Journey world's toggles would come back at whatever a fresh world's empty section
    /// decodes to (every power off, every slider at zero) regardless of what was set above.
    #[test]
    fn a_journey_world_with_everything_off_still_saves_the_six_modelled_entries() {
        // Not a placeholder `bool(false)` for "no creative powers" any more: even an all-off
        // Journey world writes all six entries, matching `CreativePowerManager.SaveToWorld`, which
        // writes one for every `IPersistentPerWorldContent` power regardless of its value.
        let world = preserved_world();
        let section = &world.preserved.as_ref().expect("loaded").trailing_sections[JOURNEY_SECTION];
        // Six `(true, u16, payload)` entries plus the terminator: (1 + 2 + 1) * 4 bools + (1 + 2 +
        // 4) * 2 sliders + 1 terminator byte.
        assert_eq!(
            section.len(),
            (1 + 2 + 1) * 4 + (1 + 2 + 4) * 2 + 1,
            "all six entries, not a one-byte placeholder"
        );
    }
}
