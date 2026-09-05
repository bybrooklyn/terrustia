//! Reader for Terraria's `.wld` save format.
//!
//! Tiles are encoded exactly as they are in a network section — the same flag chain, the same
//! field order, the same run lengths — so [`terrustia_proto::section::read_tile_with`] is shared
//! between the two. The differences are that the file walks the world column by column rather than
//! row by row, and that it carries its own frame-importance table so an old save still loads after
//! the game's table changes.
//!
//! Layouts transcribed from `Terraria.IO.WorldFile` in the 1.4.5.7 build.

use std::path::Path;

use terrustia_proto::{ItemStack, PacketReader, section::read_tile_with};
use thiserror::Error;
use tracing::{debug, warn};

use super::progress::Progress;
use super::worldgen::secret_seed::SecretSeeds;
use super::{
    World,
    objects::{Chest, PreservedWorld, Sign},
};

/// Oldest save version this reader accepts.
///
/// The format grew a field at a time across dozens of releases; rather than guess at gates we
/// never exercise, only versions from the 1.4.4 era onward are accepted and anything older is
/// refused with a clear message.
pub const MIN_VERSION: i32 = 279;

/// Newest save version this reader has been transcribed against.
///
/// There is a ceiling as well as a floor, and it matters more than it looks. The header is copied
/// verbatim and patched at *byte offsets we walked to*, so a future format that inserts one field
/// ahead of those offsets does not fail to load — it loads, and then the next save writes the
/// clock over whatever now lives at those bytes. Vanilla refuses a world newer than it knows
/// (`StatusID.LaterVersion`) for exactly this reason, and so do we.
/// 326 is what real Terraria 1.4.5.8 writes and accepts (`WorldFile.cs:1180`; load guard
/// `num2 > 326 ? LaterVersion`); no load-path layout gate exists above 323, so a 326 file is
/// byte-for-byte a 325 file. This was 325, which refused every world a real 1.4.5.8 client saved,
/// the round-trip-through-real-Terraria check included.
pub const MAX_VERSION: i32 = 326;

/// `"relogic"`, followed by a file-type byte.
const MAGIC: &[u8; 7] = b"relogic";
const FILE_TYPE_WORLD: u8 = 2;

#[derive(Debug, Error)]
pub enum WldError {
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// A write that failed, already explained.
    ///
    /// Separate from [`WldError::Io`] because that one says "reading", and a save failure reported
    /// as a read is an operator sent looking in the wrong direction. `source` has been through
    /// [`crate::safe_write::explain`], so it already carries the path, what was being attempted and
    /// what to do about it; this variant deliberately adds nothing on top rather than printing a
    /// second copy of the path in front of it.
    #[error("{source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("not a Terraria world file (magic was {found:?})")]
    BadMagic { found: Vec<u8> },

    #[error("file type {found} is not a world (expected {FILE_TYPE_WORLD})")]
    NotAWorld { found: u8 },

    #[error(
        "world format version {found} is too old; this reader handles {MIN_VERSION} and newer \
         (open and re-save the world in Terraria to upgrade it)"
    )]
    TooOld { found: i32 },

    #[error(
        "world format version {found} is newer than this build knows ({MAX_VERSION}); refusing \
         rather than risk corrupting it, because saving patches the header at fixed offsets and a \
         format that moved them would write the clock over something else"
    )]
    TooNew { found: i32 },

    #[error("world claims implausible dimensions {width}x{height}")]
    BadDimensions { width: i32, height: i32 },

    #[error(
        "the late header stopped making sense at byte {at}: a count of {count} where a small \
         list was expected, so the reader is no longer where it thinks it is"
    )]
    LateHeaderOutOfStep { at: usize, count: i64 },

    #[error(
        "the progression flags did not decode as flags (invasion type {invasion_type}, size \
         {invasion_size}); the header layout has changed and this reader is reading the wrong bytes"
    )]
    ProgressionOutOfStep {
        invasion_type: i32,
        invasion_size: i32,
    },

    #[error("section pointer {index} is {pointer}, outside a {len}-byte file")]
    BadSectionPointer {
        index: usize,
        pointer: i64,
        len: usize,
    },

    #[error(
        "section pointer {index} is {pointer}, before section {prev_index}'s pointer {previous}; \
         section pointers must not go backwards, so the file's structure cannot be trusted"
    )]
    SectionPointersOutOfOrder {
        index: usize,
        pointer: i64,
        prev_index: usize,
        previous: i64,
    },

    #[error("tile data ended early: {decoded} of {expected} tiles")]
    TruncatedTiles { decoded: usize, expected: usize },

    #[error("world would serialise to {bytes} bytes, past the format's 2 GiB section offsets")]
    SaveTooLarge { bytes: i64 },

    #[error(
        "the world was written but could not be read back ({source}); the previous save has been \
         left in place rather than replaced with something unreadable"
    )]
    WroteSomethingUnreadable {
        #[source]
        source: Box<WldError>,
    },

    #[error("malformed data at byte {offset}: {source}")]
    Decode {
        offset: usize,
        #[source]
        source: terrustia_proto::ProtoError,
    },
}

type Result<T> = std::result::Result<T, WldError>;

/// Load a world from a `.wld` file.
pub fn load(path: &Path) -> Result<World> {
    let bytes = std::fs::read(path).map_err(|e| WldError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    parse(&bytes)
}

/// Parse an in-memory `.wld` image.
pub fn parse(bytes: &[u8]) -> Result<World> {
    let mut r = PacketReader::new(bytes);
    let file = read_file_header(&mut r, bytes.len())?;

    let header_start = file.sections[0] as usize;
    seek(&mut r, bytes, file.sections[0], 0)?;
    let (mut world, offsets) = read_world_header(&mut r, file.version, header_start)?;

    // The whole header is kept verbatim so a later save preserves the progression flags, event
    // state and sub-structures this server does not model.
    let tile_start = file.sections[1] as usize;
    let header_bytes = bytes
        .get(header_start..tile_start)
        .ok_or(WldError::BadSectionPointer {
            index: 1,
            pointer: i64::from(file.sections[1]),
            len: bytes.len(),
        })?
        .to_vec();

    seek(&mut r, bytes, file.sections[1], 1)?;
    read_tiles(&mut r, &mut world, &file.importance)?;

    let mut chest_slots = None;
    if file.sections.len() > 2 {
        seek(&mut r, bytes, file.sections[2], 2)?;
        world.chests = read_chests(&mut r, file.version, &mut chest_slots)?;
    }
    if file.sections.len() > 3 {
        seek(&mut r, bytes, file.sections[3], 3)?;
        world.signs = read_signs(&mut r)?;
    }

    // Sections 4 onwards hold townsfolk, tile entities, pressure plates, the town manager, the
    // bestiary and creative powers. Each is sliced out on its own so the one this server models —
    // the tile entities — can be rewritten from its own state while the rest pass through.
    let mut trailing_sections = Vec::new();
    if file.sections.len() > 4 {
        for (nth, &start) in file.sections[4..].iter().enumerate() {
            // Each section runs to the start of the next; the last runs to the end of the file,
            // taking the footer with it. `read_file_header` already refused a file whose pointers
            // go backwards, so `end >= start` here is an established invariant, not merely hoped
            // for: `.get` stays only as a check on a fact that should already be true rather than
            // as the thing that makes a corrupt file read as an empty section.
            let end = file
                .sections
                .get(5 + nth)
                .map_or(bytes.len(), |&next| next as usize);
            let (start, end) = (start as usize, end.min(bytes.len()));
            trailing_sections.push(bytes.get(start..end).unwrap_or_default().to_vec());
        }
    }

    // Section 4 is the townsfolk. Read rather than carried, because a world's residents are the
    // most visible thing in it: carrying the section through meant a real Terraria world's Guide
    // and Merchant were invisible to the server, and anyone who moved in during a session was
    // gone by the next restart along with their name and their house.
    let mut town_npcs_understood = true;
    if let Some(section) = trailing_sections.first() {
        let mut r = PacketReader::new(section);
        let read = read_town_npcs(&mut r, file.version);
        town_npcs_understood = read.complete;
        if !read.complete {
            warn!(
                residents = read.npcs.len(),
                "the townsfolk section did not fully decode; it will be carried through on save \
                 rather than rewritten, so nothing in it is lost"
            );
        }
        world.shimmered_town_npcs = read.shimmered;
        world.town_npcs = read.npcs;
        // The Lunar Pillars, from the same section's second list. Restored to live NPCs at
        // startup (`GameServer::restore_lunar_pillars`) rather than here — this module only reads
        // the file into `World` fields, and a pillar has to come back as something `tick_lunar`
        // can see standing, which needs the server's own NPC roster.
        world.saved_npcs = read.saved_npcs;
    }

    // Section 5 is the tile entities: pylons, item frames, mannequins, logic sensors. Read rather
    // than carried, because a pylon a client cannot be told about is a pylon nobody can use, and
    // because carrying them through means a pylon placed on this server is lost on the next save.
    let mut tile_entities_understood = true;
    if let Some(section) = trailing_sections.get(1) {
        let mut r = PacketReader::new(section);
        let (entities, complete) = read_tile_entities(&mut r, file.version);
        tile_entities_understood = complete;
        if !complete {
            warn!(
                decoded = entities.len(),
                "the tile-entity section did not fully decode; it will be carried through on save \
                 rather than rewritten, so no pylon or item frame is lost"
            );
        }
        world.tile_entities = entities;
        world.next_tile_entity = world
            .tile_entities
            .iter()
            .map(|e| e.id + 1)
            .max()
            .unwrap_or(0);
    }

    // Section 9 (index 5 of the trailing run): the Journey powers this server keeps live on
    // `GameServer::journey`, not on the world. Read here so a restart of a Journey world does not
    // silently reset its shared toggles and sliders; always rewritten from live state on save
    // (`wld_save::write_journey_powers`), the same as the townsfolk and pillars above, so nothing
    // here needs an "understood" flag of its own — a section this build cannot fully decode just
    // leaves whichever powers it did not reach at their defaults.
    if let Some(section) = trailing_sections.get(5) {
        let mut r = PacketReader::new(section);
        read_journey_powers(&mut r, &mut world);
    }

    world.preserved = Some(PreservedWorld {
        version: file.version,
        chest_slots,
        revision: file.revision,
        favorite: file.favorite,
        header_bytes,
        time_offset: offsets.time,
        day_time_offset: offsets.day_time,
        moon_phase_offset: offsets.moon_phase,
        blood_moon_offset: offsets.blood_moon,
        eclipse_offset: offsets.eclipse,
        progress_offset: offsets.progress,
        hard_mode_offset: offsets.hard_mode,
        altar_offset: offsets.altar,
        orb_count_offset: offsets.orb_count,
        downed_run_offset: offsets.late.downed_run,
        tower_run_offset: offsets.late.tower_run,
        rain_offset: offsets.late.rain,
        wind_offset: offsets.late.wind,
        sandstorm_offset: offsets.late.sandstorm,
        army_run_offset: offsets.late.army_run,
        combat_book_offset: offsets.late.combat_book,
        late_downed_run_offset: offsets.late.late_downed_run,
        combat_book_two_offset: offsets.late.combat_book_two,
        slime_unlocks_offset: offsets.late.slime_unlocks,
        hardmode_ores_offset: offsets.late.hardmode_ores,
        banner_kills_offset: offsets.late.banner_kills,
        town_npcs_understood,
        tile_entities_understood,
        trailing_sections,
        importance: file.importance,
    });

    if let Some(manifest) = world
        .preserved
        .as_ref()
        .and_then(|p| crate::world::worldgen::manifest::Manifest::from_header(&p.header_bytes))
    {
        debug!(
            passes = manifest.passes.len(),
            version = manifest.version.as_deref().unwrap_or("?"),
            "world carries a generation manifest"
        );
    }

    debug!(
        version = file.version,
        width = world.width(),
        height = world.height(),
        chests = world.chests.len(),
        signs = world.signs.len(),
        "loaded world file"
    );
    Ok(world)
}

struct FileHeader {
    version: i32,
    revision: u32,
    favorite: u64,
    sections: Vec<i32>,
    /// One flag per tile type: whether it stores frame coordinates.
    importance: Vec<bool>,
}

fn read_file_header(r: &mut PacketReader<'_>, len: usize) -> Result<FileHeader> {
    let version = num(r.i32(), r)?;
    if version < MIN_VERSION {
        return Err(WldError::TooOld { found: version });
    }
    if version > MAX_VERSION {
        return Err(WldError::TooNew { found: version });
    }

    let magic = num(r.bytes(7), r)?;
    if magic != MAGIC {
        return Err(WldError::BadMagic {
            found: magic.to_vec(),
        });
    }
    let file_type = num(r.u8(), r)?;
    if file_type != FILE_TYPE_WORLD {
        return Err(WldError::NotAWorld { found: file_type });
    }
    let revision = num(r.u32(), r)?;
    let favorite = num(r.u64(), r)?;

    let section_count = num(r.i16(), r)?;
    if section_count < 4 {
        return Err(WldError::BadSectionPointer {
            index: 0,
            pointer: i64::from(section_count),
            len,
        });
    }
    let mut sections = Vec::with_capacity(section_count as usize);
    for index in 0..section_count as usize {
        let pointer = num(r.i32(), r)?;
        if pointer < 0 || pointer as usize > len {
            return Err(WldError::BadSectionPointer {
                index,
                pointer: i64::from(pointer),
                len,
            });
        }
        // Every section runs from its own pointer to the next one's (or, for the last section, to
        // the end of the file); the trailing-section loop in `parse` relies on that ordering to
        // slice each one out. A pointer that goes backwards is not a smaller section, it is a file
        // whose structure cannot be trusted, and reading it anyway used to clamp the resulting
        // negative-length slice to empty rather than refuse: a corrupt file loaded as one with a
        // silently empty townsfolk or tile-entity section instead of being rejected outright.
        if let Some(&previous) = sections.last()
            && pointer < previous
        {
            return Err(WldError::SectionPointersOutOfOrder {
                index,
                pointer: i64::from(pointer),
                prev_index: index - 1,
                previous: i64::from(previous),
            });
        }
        sections.push(pointer);
    }

    // The importance table is a bitset packed least significant bit first: the writer starts its
    // mask at 0x80 as a sentinel so the first entry pulls a byte and uses bit 0, then walks
    // 1, 2, 4 ... 0x80 before pulling the next.
    let mask_count = num(r.u16(), r)? as usize;
    let mut importance = Vec::with_capacity(mask_count);
    let mut current = 0u8;
    let mut bit = 0x80u8;
    for _ in 0..mask_count {
        if bit == 0x80 {
            current = num(r.u8(), r)?;
            bit = 1;
        } else {
            bit <<= 1;
        }
        importance.push(current & bit != 0);
    }

    Ok(FileHeader {
        version,
        revision,
        favorite,
        sections,
        importance,
    })
}

/// Offsets of the mutable clock fields, relative to the start of the header section.
struct HeaderOffsets {
    time: usize,
    day_time: usize,
    moon_phase: usize,
    /// Whether a blood moon or eclipse is in progress. Always present at a fixed position right
    /// after the moon phase, unlike the flags further down — those move depending on which of
    /// several variable-length lists came before them, these do not.
    blood_moon: usize,
    eclipse: usize,
    /// The run of twenty booleans beginning at `downedBoss1`.
    progress: Option<usize>,
    hard_mode: Option<usize>,
    altar: Option<usize>,
    orb_count: Option<usize>,
    late: LateOffsets,
}

/// What the world looks like, as opposed to how it behaves.
///
/// Gathered on the way through the header because the file scatters it: eight backdrop styles in
/// one place, five more two hundred lines later, the tree tops later still, and the cloud count
/// between them. None of it changes a single rule of play — no routine reads any of it — but all
/// of it is sent in packet 7, and a server that reports nought for the lot serves every biome the
/// wrong sky.
///
/// The slot constants are packet 7's order rather than the file's, so the mapping is stated once
/// here instead of at each of the three places the file happens to write these out.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scenery {
    pub backgrounds: [u8; 13],
    pub tree_tops: [u8; 13],
    pub num_clouds: u8,
}

impl Scenery {
    pub const TREE_1: usize = 0;
    pub const TREE_2: usize = 1;
    pub const TREE_3: usize = 2;
    pub const TREE_4: usize = 3;
    pub const CORRUPT: usize = 4;
    pub const JUNGLE: usize = 5;
    pub const SNOW: usize = 6;
    pub const HALLOW: usize = 7;
    pub const CRIMSON: usize = 8;
    pub const DESERT: usize = 9;
    pub const OCEAN: usize = 10;
    pub const MUSHROOM: usize = 11;
    pub const UNDERWORLD: usize = 12;
}

/// Where in the header the flags past the invasion block live.
///
/// They are recorded rather than re-derived because saving preserves the header verbatim and
/// patches it in place: writing a flag means knowing the byte, and the byte is only knowable by
/// having walked there.
#[derive(Debug, Default, Clone, Copy)]
pub struct LateOffsets {
    /// The run of nine "downed" booleans beginning at `downedFishron`.
    pub downed_run: Option<usize>,
    /// The run of nine pillar flags: four beaten, four standing, and the apocalypse itself.
    pub tower_run: Option<usize>,
    /// The weather block: raining, how long for, and how hard.
    pub rain: Option<usize>,
    /// The wind the world is blowing toward.
    pub wind: Option<usize>,
    /// The sandstorm block: happening, how long for, and its two severities.
    pub sandstorm: Option<usize>,
    /// The bartender, then the three Old One's Army tiers.
    pub army_run: Option<usize>,
    /// The first combat book, which sits alone between two blocks that are not flags.
    pub combat_book: Option<usize>,
    /// The Empress of Light, Queen Slime and Deerclops, in that order.
    pub late_downed_run: Option<usize>,
    /// The second combat book, after the run of unlocked town-NPC spawns.
    pub combat_book_two: Option<usize>,
    /// `unlockedSlimeOldSpawn`, the head of the run old/purple/rainbow/red/yellow/copper.
    ///
    /// Recorded rather than derived because freeing a bound town slime is permanent world state:
    /// a world that forgot it would offer the same slime again on the next launch, and the
    /// resident already standing in a house would have a bound twin wandering the caves.
    pub slime_unlocks: Option<usize>,
    /// The three hardmode ore tiers — cobalt, mythril, adamantite — as `i32`s.
    ///
    /// Recorded because smashing an altar picks these, and a choice that never reaches the file is
    /// worse than one that was never made: the header still reads `-1` next launch, so the next
    /// altar rolls a *different* ore than the one already sprayed through the world.
    pub hardmode_ores: Option<usize>,
    /// The banner kill counts: where the run of `i32`s starts, and how many the file has room for.
    ///
    /// The length is fixed by the file — the count was written before them and everything after
    /// depends on it — so a count for a banner this world never allocated has nowhere to go and is
    /// dropped rather than shifting the header.
    pub banner_kills: Option<(usize, usize)>,
}

/// What the late header says about the weather.
#[derive(Debug, Default, Clone, Copy)]
struct Weather {
    raining: bool,
    rain_time: i32,
    max_rain: f32,
    wind: f32,
    sandstorm: bool,
    sandstorm_time: i32,
    severity: f32,
    intended_severity: f32,
}

/// Walk the header past the invasion block, picking up the flags the server actually uses.
///
/// Everything here is positional: there is no framing, so a field read at the wrong width puts
/// every flag after it in the wrong place. The two variable-length lists in the middle are why it
/// cannot simply be seeked into.
// Eight out-parameters rather than a struct, deliberately. This is a positional walk through a
// header with no framing, where a field read at the wrong width puts every field after it in the
// wrong place — the exact bug that misplaced the Moon Lord on pre-1.4.4.9 worlds. Restructuring it
// to satisfy an argument-count lint would mean editing two hundred lines of the most fragile code
// in the project to gain nothing a reader can see.
#[allow(clippy::too_many_arguments)]
fn read_late_header(
    r: &mut PacketReader<'_>,
    version: i32,
    progress: &mut Progress,
    weather: &mut Weather,
    ore_tiers: &mut [i16; 7],
    banner_kills: &mut std::collections::HashMap<u16, u32>,
    scenery: &mut Scenery,
    offsets: &mut LateOffsets,
    section_start: usize,
) -> Result<()> {
    let _slime_rain_time = num(r.f64(), r)?;
    let _sundial_cooldown = num(r.u8(), r)?;

    offsets.rain = Some(r.position() - section_start);
    weather.raining = num(r.bool(), r)?;
    weather.rain_time = num(r.i32(), r)?;
    weather.max_rain = num(r.f32(), r)?;

    // The hardmode ore tiers the world rolled when the wall fell: cobalt, mythril, adamantite.
    // These sit apart from the other four in the file but belong beside them everywhere else.
    offsets.hardmode_ores = Some(r.position() - section_start);
    for tier in &mut ore_tiers[4..7] {
        *tier = num(r.i32(), r)? as i16;
    }
    // Eight of the thirteen background styles. The file writes them in its own order, which is not
    // the packet's, so each is read straight into the slot packet 7 will send it from. The other
    // five are two hundred lines further down, past the Old One's Army flags.
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
        scenery.backgrounds[slot] = num(r.u8(), r)?;
    }
    let _cloud_bg_active = num(r.i32(), r)?;
    // The file keeps a short, the packet sends a byte; a sky with more than 255 clouds in it is
    // not a thing the game produces, but saturating beats wrapping to nearly none.
    scenery.num_clouds = num(r.i16(), r)?.clamp(0, i16::from(u8::MAX)) as u8;
    offsets.wind = Some(r.position() - section_start);
    weather.wind = num(r.f32(), r)?;

    // Who has already handed in an angler quest today: a list of names.
    let anglers = num(r.i32(), r)?;
    if !(0..=255).contains(&anglers) {
        return Err(WldError::LateHeaderOutOfStep {
            at: r.position(),
            count: i64::from(anglers),
        });
    }
    for _ in 0..anglers {
        num(r.string(), r)?;
    }
    progress.saved_angler = num(r.bool(), r)?;
    let _angler_quest = num(r.i32(), r)?;
    progress.saved_stylist = num(r.bool(), r)?;
    progress.saved_tax_collector = num(r.bool(), r)?;
    progress.saved_golfer = num(r.bool(), r)?;
    let _invasion_size_start = num(r.i32(), r)?;
    let _cultist_delay = num(r.i32(), r)?;

    // The banner kill counts, then — only from 289 — the claimable banners.
    //
    // The second list is the one version gate in this whole run that is easy to miss, because a
    // world that predates it usually has nothing after the kill counts that looks wrong: the two
    // bytes read for a count that is not there come back as zero, no items follow, and every flag
    // from here to the end of the header is silently two bytes out. That misplaces the Moon Lord,
    // the cultist and the four pillars on any world older than 1.4.4.9.
    let kinds = num(r.i16(), r)?;
    if !(0..=10_000).contains(&kinds) {
        return Err(WldError::LateHeaderOutOfStep {
            at: r.position(),
            count: i64::from(kinds),
        });
    }
    // Read rather than skipped: a hundred zombies killed before a restart should still count
    // towards the banner afterwards. Stored sparsely, because most of the two hundred and
    // ninety-three banners are never touched in one world.
    offsets.banner_kills = Some((r.position() - section_start, kinds as usize));
    for banner in 0..kinds {
        let count = num(r.i32(), r)?;
        if count > 0 {
            banner_kills.insert(banner as u16, count as u32);
        }
    }
    if version >= 289 {
        let claimable = num(r.i16(), r)?;
        if !(0..=10_000).contains(&claimable) {
            return Err(WldError::LateHeaderOutOfStep {
                at: r.position(),
                count: i64::from(claimable),
            });
        }
        for _ in 0..claimable {
            num(r.u16(), r)?;
        }
    }

    let _fast_forward_to_dawn = num(r.bool(), r)?;
    offsets.downed_run = Some(r.position() - section_start);
    for flag in [
        &mut progress.downed_fishron,
        &mut progress.downed_martians,
        &mut progress.downed_ancient_cultist,
        &mut progress.downed_moon_lord,
        &mut progress.downed_halloween_king,
        &mut progress.downed_halloween_tree,
        &mut progress.downed_christmas_ice_queen,
        &mut progress.downed_christmas_santank,
        &mut progress.downed_christmas_tree,
    ] {
        *flag = num(r.bool(), r)?;
    }
    offsets.tower_run = Some(r.position() - section_start);
    for flag in [
        &mut progress.downed_tower_solar,
        &mut progress.downed_tower_vortex,
        &mut progress.downed_tower_nebula,
        &mut progress.downed_tower_stardust,
        &mut progress.tower_active_solar,
        &mut progress.tower_active_vortex,
        &mut progress.tower_active_nebula,
        &mut progress.tower_active_stardust,
        &mut progress.lunar_apocalypse_up,
    ] {
        *flag = num(r.bool(), r)?;
    }

    // A party in progress, and who is celebrating.
    let _party_manual = num(r.bool(), r)?;
    let _party_genuine = num(r.bool(), r)?;
    let _party_cooldown = num(r.i32(), r)?;
    let partiers = num(r.i32(), r)?;
    if !(0..=1_000).contains(&partiers) {
        return Err(WldError::LateHeaderOutOfStep {
            at: r.position(),
            count: i64::from(partiers),
        });
    }
    for _ in 0..partiers {
        num(r.i32(), r)?;
    }

    offsets.sandstorm = Some(r.position() - section_start);
    weather.sandstorm = num(r.bool(), r)?;
    weather.sandstorm_time = num(r.i32(), r)?;
    weather.severity = num(r.f32(), r)?;
    weather.intended_severity = num(r.f32(), r)?;

    // The Old One's Army: the bartender who starts it, then the three tiers it has lost.
    offsets.army_run = Some(r.position() - section_start);
    for flag in [
        &mut progress.saved_bartender,
        &mut progress.downed_army_t1,
        &mut progress.downed_army_t2,
        &mut progress.downed_army_t3,
    ] {
        *flag = num(r.bool(), r)?;
    }

    // The other five background styles, stranded here rather than beside the first eight because
    // they were added to the format later.
    for slot in [
        Scenery::MUSHROOM,
        Scenery::UNDERWORLD,
        Scenery::TREE_2,
        Scenery::TREE_3,
        Scenery::TREE_4,
    ] {
        scenery.backgrounds[slot] = num(r.u8(), r)?;
    }
    offsets.combat_book = Some(r.position() - section_start);
    progress.combat_book = num(r.bool(), r)?;

    // Lantern night: its cooldown, then three flags about the night to come.
    num(r.i32(), r)?;
    for _ in 0..3 {
        num(r.bool(), r)?;
    }

    // One tree-top variation per biome, counted rather than fixed.
    let tree_tops = num(r.i32(), r)?;
    if !(0..=1_000).contains(&tree_tops) {
        return Err(WldError::LateHeaderOutOfStep {
            at: r.position(),
            count: i64::from(tree_tops),
        });
    }
    for area in 0..tree_tops {
        // The file stores an int per area and the packet sends a byte; the values are small style
        // indices, so the narrowing is the game's own (`TreeTopsInfo.SyncSend` casts to byte).
        // A file with more areas than the packet carries is read past rather than refused, which
        // is what the game does too.
        let variation = num(r.i32(), r)?;
        if let Some(slot) = scenery.tree_tops.get_mut(area as usize) {
            *slot = variation as u8;
        }
    }

    // Forced holidays for today, the four ore tiers the world was generated with, and the pets.
    for _ in 0..2 {
        num(r.bool(), r)?;
    }
    for tier in &mut ore_tiers[0..4] {
        *tier = num(r.i32(), r)? as i16;
    }
    for _ in 0..3 {
        num(r.bool(), r)?;
    }

    offsets.late_downed_run = Some(r.position() - section_start);
    for flag in [
        &mut progress.downed_empress_of_light,
        &mut progress.downed_queen_slime,
        &mut progress.downed_deerclops,
    ] {
        *flag = num(r.bool(), r)?;
    }

    // Nine town NPCs whose arrival has been unlocked by other means, then the second book.
    for _ in 0..9 {
        num(r.bool(), r)?;
    }
    offsets.combat_book_two = Some(r.position() - section_start);
    progress.combat_book_two = num(r.bool(), r)?;

    // The Peddler's Satchel and the green slime, then the run of seven remaining slime unlocks
    // (`WorldFile.cs:1414-1421`): green, old, purple, rainbow, red, yellow, copper. The offset is
    // recorded at `unlockedSlimeOldSpawn`, which is where the three this server models begin.
    for _ in 0..2 {
        num(r.bool(), r)?;
    }
    offsets.slime_unlocks = Some(r.position() - section_start);
    progress.unlocked_slime_old = num(r.bool(), r)?;
    progress.unlocked_slime_purple = num(r.bool(), r)?;
    // Rainbow and red, neither of which this server can produce or free: read past, and left
    // untouched on save rather than written back as false.
    for _ in 0..2 {
        num(r.bool(), r)?;
    }
    progress.unlocked_slime_yellow = num(r.bool(), r)?;

    Ok(())
}

fn read_world_header(
    r: &mut PacketReader<'_>,
    version: i32,
    section_start: usize,
) -> Result<(World, HeaderOffsets)> {
    let name = num(r.string(), r)?;
    let seed_text = num(r.string(), r)?;
    let world_gen_version = num(r.u64(), r)?;
    let mut unique_id = [0u8; 16];
    unique_id.copy_from_slice(num(r.bytes(16), r)?);
    let id = num(r.i32(), r)?;

    // The world rectangle in pixel coordinates; the tile dimensions follow it.
    for _ in 0..4 {
        num(r.i32(), r)?;
    }
    let height = num(r.i32(), r)?;
    let width = num(r.i32(), r)?;
    if !(10..=i32::from(i16::MAX)).contains(&width) || !(10..=i32::from(i16::MAX)).contains(&height)
    {
        return Err(WldError::BadDimensions { width, height });
    }

    // World flags, each gated on the version that introduced it — real vanilla's own
    // `WorldFile.LoadWorldFlags`, transcribed field for field (same order, same gates). Real
    // vanilla falls back to `zenithWorld = remixWorld && drunkWorld` for the one gap between
    // `noTrapsWorld` (266) and its own `zenithWorld` field (267); this project's own `SAVE_VERSION`
    // (325) is well past every one of these gates, so that fallback only matters for a save this
    // old, kept anyway since a genuinely old real vanilla file is exactly what the preserved-header
    // path exists to round-trip correctly.
    let game_mode = num(r.i32(), r)?;
    let mut secret_seeds = SecretSeeds::none();
    if version >= 222 {
        secret_seeds.drunk = num(r.bool(), r)?;
    }
    if version >= 227 {
        secret_seeds.get_good = num(r.bool(), r)?;
    }
    if version >= 238 {
        secret_seeds.tenth_anniversary = num(r.bool(), r)?;
    }
    if version >= 239 {
        secret_seeds.dont_starve = num(r.bool(), r)?;
    }
    if version >= 241 {
        secret_seeds.not_the_bees = num(r.bool(), r)?;
    }
    if version >= 249 {
        secret_seeds.remix = num(r.bool(), r)?;
    }
    if version >= 266 {
        secret_seeds.no_traps = num(r.bool(), r)?;
    }
    if version >= 267 {
        secret_seeds.everything = num(r.bool(), r)?;
    } else {
        secret_seeds.everything = secret_seeds.remix && secret_seeds.drunk;
    }
    if version >= 302 {
        secret_seeds.skyblock = num(r.bool(), r)?;
    }
    num(r.i64(), r)?; // creation time
    if version >= 284 {
        num(r.i64(), r)?; // last played
    }

    let moon_type = num(r.u8(), r)?;
    let mut tree_x = [0i32; 3];
    for slot in &mut tree_x {
        *slot = num(r.i32(), r)?;
    }
    let mut tree_style = [0u8; 4];
    for slot in &mut tree_style {
        *slot = num(r.i32(), r)? as u8;
    }
    let mut cave_back_x = [0i32; 3];
    for slot in &mut cave_back_x {
        *slot = num(r.i32(), r)?;
    }
    let mut cave_back_style = [0u8; 4];
    for slot in &mut cave_back_style {
        *slot = num(r.i32(), r)? as u8;
    }
    let ice_back_style = num(r.i32(), r)? as u8;
    let jungle_back_style = num(r.i32(), r)? as u8;
    let hell_back_style = num(r.i32(), r)? as u8;

    let spawn_x = num(r.i32(), r)?;
    let spawn_y = num(r.i32(), r)?;
    let surface = num(r.f64(), r)?;
    let rock_layer = num(r.f64(), r)?;

    let mut offsets = HeaderOffsets {
        time: r.position() - section_start,
        day_time: r.position() - section_start + 8,
        moon_phase: r.position() - section_start + 9,
        // Filled in immediately below, once the reader has actually walked to them — a moon
        // phase is a fixed 4 bytes so the offset can be computed in advance, but writing it this
        // way for the next two keeps every offset here computed the same way: from where the
        // reader actually is, not from an assumed width.
        blood_moon: 0,
        eclipse: 0,
        progress: None,
        hard_mode: None,
        altar: None,
        orb_count: None,
        late: LateOffsets::default(),
    };
    let time = num(r.f64(), r)?;
    let day_time = num(r.bool(), r)?;
    let moon_phase = num(r.i32(), r)?;
    offsets.blood_moon = r.position() - section_start;
    let blood_moon = num(r.bool(), r)?;
    offsets.eclipse = r.position() - section_start;
    let eclipse = num(r.bool(), r)?;
    let dungeon_x = num(r.i32(), r)?;
    let dungeon_y = num(r.i32(), r)?;
    let crimson = num(r.bool(), r)?;

    // What the world has already been through. These have to be read in file order, and they are
    // read rather than skipped because routines, spawn pools and shops all ask about them.
    let mut progress = Progress::default();
    let mut world_weather = Weather::default();
    offsets.progress = Some(r.position() - section_start);
    for flag in [
        &mut progress.downed_boss1,
        &mut progress.downed_boss2,
        &mut progress.downed_boss3,
        &mut progress.downed_queen_bee,
        &mut progress.downed_mech1,
        &mut progress.downed_mech2,
        &mut progress.downed_mech3,
        &mut progress.downed_mech_any,
        &mut progress.downed_plantera,
        &mut progress.downed_golem,
        &mut progress.downed_king_slime,
        &mut progress.saved_goblin,
        &mut progress.saved_wizard,
        &mut progress.saved_mechanic,
        &mut progress.downed_goblins,
        &mut progress.downed_clown,
        &mut progress.downed_frost,
        &mut progress.downed_pirates,
        &mut progress.shadow_orb_smashed,
        &mut progress.spawn_meteor,
    ] {
        *flag = num(r.bool(), r)?;
    }
    offsets.orb_count = Some(r.position() - section_start);
    progress.shadow_orb_count = num(r.u8(), r)?;
    offsets.altar = Some(r.position() - section_start);
    progress.altar_count = num(r.i32(), r)?;
    let hard_mode_at = r.position();
    offsets.hard_mode = Some(hard_mode_at - section_start);
    progress.hard_mode = num(r.bool(), r)?;
    // Reading a little further is how the offset above is checked: if `hardMode` were even one
    // byte out, these would not decode as an invasion.
    let after_party = num(r.bool(), r)?;
    let invasion_delay = num(r.i32(), r)?;
    let invasion_size = num(r.i32(), r)?;
    let invasion_type = num(r.i32(), r)?;
    let invasion_x = num(r.f64(), r)?;
    debug!(
        hard_mode = progress.hard_mode,
        altars = progress.altar_count,
        orbs = progress.shadow_orb_count,
        after_party,
        invasion_delay,
        invasion_size,
        invasion_type,
        invasion_x,
        hard_mode_at,
        "world progression"
    );
    if !(0..=4).contains(&invasion_type) || !(0..=200_000).contains(&invasion_size) {
        return Err(WldError::ProgressionOutOfStep {
            invasion_type,
            invasion_size,
        });
    }
    // The rest of the header is read for the flags that matter and skipped for the rest. It has
    // to be walked rather than seeked because two of the runs are variable-length lists.
    let mut late = LateOffsets::default();
    let mut ore_tiers = [-1i16; 7];
    let mut banner_kills = std::collections::HashMap::new();
    let mut scenery = Scenery::default();
    if let Err(error) = read_late_header(
        r,
        version,
        &mut progress,
        &mut world_weather,
        &mut ore_tiers,
        &mut banner_kills,
        &mut scenery,
        &mut late,
        section_start,
    ) {
        // A header that runs out is not fatal: the tile pointer is what actually finds the tiles,
        // and everything read up to here is already good. It only means the late flags stay at
        // their defaults, which is what an older world would have anyway.
        debug!(?error, "world header ended before the late flags");
    }

    // Everything past this point is skipped: the tile section pointer takes us straight to the
    // next section regardless of how long the rest of the header is.

    offsets.late = late;
    let mut world = World::empty(width, height, name);
    world.dungeon_x = Some(dungeon_x);
    world.dungeon_y = Some(dungeon_y);
    world.raining = world_weather.raining;
    world.rain_time = world_weather.rain_time;
    world.max_rain = world_weather.max_rain;
    world.wind = world_weather.wind;
    world.sandstorm = world_weather.sandstorm;
    world.sandstorm_time = world_weather.sandstorm_time;
    world.sandstorm_severity = world_weather.severity;
    world.sandstorm_intended_severity = world_weather.intended_severity;
    world.id = id;
    world.unique_id = unique_id;
    world.world_gen_version = world_gen_version;
    world.seed_text = seed_text;
    world.backgrounds = scenery.backgrounds;
    world.tree_tops = scenery.tree_tops;
    world.num_clouds = scenery.num_clouds;
    world.game_mode = game_mode.clamp(0, 3) as u8;
    world.secret_seeds = secret_seeds;
    world.spawn_x = spawn_x.clamp(0, width - 1) as i16;
    world.spawn_y = spawn_y.clamp(0, height - 1) as i16;
    world.surface = (surface as i32).clamp(0, height - 1) as i16;
    world.rock_layer = (rock_layer as i32).clamp(0, height - 1) as i16;
    world.time = time as i32;
    world.day_time = day_time;
    world.moon_phase = (moon_phase.rem_euclid(8)) as u8;
    // A blood moon or eclipse in progress used to be read and thrown away, so loading a world
    // mid-event silently ended it — the file said one was happening and the live session simply
    // never knew. The bytes themselves were always untouched on disk; only the in-memory session
    // forgot.
    world.blood_moon = blood_moon;
    world.eclipse = eclipse;
    world.crimson = crimson;
    world.ore_tiers = ore_tiers;
    world.banner_kills = banner_kills;
    world.progress = progress;
    world.moon_type = moon_type;
    world.tree_x = tree_x;
    world.tree_style = tree_style;
    world.cave_back_x = cave_back_x;
    world.cave_back_style = cave_back_style;
    world.ice_back_style = ice_back_style;
    world.jungle_back_style = jungle_back_style;
    world.hell_back_style = hell_back_style;
    Ok((world, offsets))
}

fn read_tiles(r: &mut PacketReader<'_>, world: &mut World, importance: &[bool]) -> Result<()> {
    // The file's own table wins over ours: a save written by another build may disagree, and the
    // table is what decides whether frame bytes are present.
    let framed = |tile: u16| importance.get(usize::from(tile)).copied().unwrap_or(false);

    let (width, height) = (world.width(), world.height());
    let expected = (width as usize) * (height as usize);
    let mut decoded = 0usize;

    // Column-major, unlike the row-major network sections.
    for x in 0..width {
        let mut y = 0i32;
        while y < height {
            let offset = r.position();
            let (tile, run) =
                read_tile_with(r, &framed).map_err(|source| WldError::Decode { offset, source })?;

            let count = i32::from(run) + 1;
            if y + count > height {
                return Err(WldError::TruncatedTiles {
                    decoded: decoded + count as usize,
                    expected,
                });
            }
            for _ in 0..count {
                world.set_tile(x, y, tile);
                y += 1;
                decoded += 1;
            }
        }
    }

    if decoded != expected {
        return Err(WldError::TruncatedTiles { decoded, expected });
    }
    Ok(())
}

fn read_chests(
    r: &mut PacketReader<'_>,
    version: i32,
    shared: &mut Option<i16>,
) -> Result<Vec<Option<Chest>>> {
    let count = num(r.i16(), r)?;
    // Before 294 every chest had the same capacity; since then each carries its own. Which it was
    // has to be remembered, because saving writes the section back in this file's own shape.
    let shared_slots = if version < 294 {
        let slots = num(r.i16(), r)?;
        *shared = Some(slots);
        i32::from(slots)
    } else {
        *shared = None;
        0
    };

    let mut chests = Vec::with_capacity(count.max(0) as usize);
    for _ in 0..count.max(0) {
        let x = num(r.i32(), r)?;
        let y = num(r.i32(), r)?;
        let name = num(r.string(), r)?;
        let slots = if version >= 294 {
            num(r.i32(), r)?
        } else {
            shared_slots
        };

        let mut items = Vec::with_capacity(slots.clamp(0, 1000) as usize);
        for _ in 0..slots.max(0) {
            items.push(num(ItemStack::read_save(r), r)?);
        }

        chests.push(Some(Chest {
            x: x as i16,
            y: y as i16,
            name,
            items,
        }));
    }
    Ok(chests)
}

fn read_signs(r: &mut PacketReader<'_>) -> Result<Vec<Option<Sign>>> {
    let count = num(r.i16(), r)?;
    let mut signs = Vec::with_capacity(count.max(0) as usize);
    for _ in 0..count.max(0) {
        let text = num(r.string(), r)?;
        let x = num(r.i32(), r)?;
        let y = num(r.i32(), r)?;
        signs.push(Some(Sign {
            x: x as i16,
            y: y as i16,
            text,
        }));
    }
    Ok(signs)
}

/// Read section 5: the furniture that remembers something.
///
/// A count, then each entity in its file form — with its id, and with a logic sensor's state,
/// neither of which the network form carries.
///
/// A truncated or unrecognised section gives up rather than failing the whole load. It is the
/// difference between "this world has an item frame this build does not know about" and "this
/// world will not open", and the first is much the better answer.
/// Read the townsfolk section: which types have been shimmered, then the residents themselves,
/// then the Lunar Pillars.
///
/// The entry loop is led by its own boolean rather than counted — `SaveNPCs` writes `active` before
/// each and a bare `false` to finish — which is why this cannot be seeked into.
///
/// A second list follows the town-list terminator, of the non-town NPCs the game persists
/// (`WorldFile.cs:1745-1755`, gated on `NPCID.Sets.SavesAndLoads` — `NPCID.cs:4807`, which in this
/// build's target version names only the four Lunar Pillars). Dropping this — reading only up to
/// the town list's own terminator and stopping there — used to mean a save mid-Lunar-Apocalypse
/// carried an *empty* second list forward, so the next load's `tick_lunar` found no pillar standing
/// against a `tower_active_*` that still said one was, and marked every tower defeated on the very
/// first tick: a free skip past the whole event.
/// The townsfolk section, and whether all of it was understood.
///
/// `complete` is the important field. This section is *rewritten* on save rather than carried
/// through, so a parse that gave up halfway used to mean the save wrote back only the residents it
/// managed to read — or, when the failure came before the commit, an **empty list**, permanently
/// deleting every resident, their names and their houses. Keeping what decoded is only half the
/// answer; the other half is refusing to rewrite a section we did not fully understand. The same
/// now goes for the second list: `complete` is only set once *both* terminators are read, so a
/// section understood up to the residents but not past them is carried through whole rather than
/// rewritten with the pillars silently dropped.
pub(crate) struct TownNpcSection {
    pub shimmered: Vec<i32>,
    pub npcs: Vec<super::objects::TownNpc>,
    pub saved_npcs: Vec<super::objects::SavedNpc>,
    pub complete: bool,
}

/// `WorldFile.LoadNPCs` reads each resident's `homelessDespawn` flag only for file version >= 315;
/// older worlds (down to `MIN_VERSION` 279) never wrote it, so reading it there consumes the next
/// entry's lead boolean and desyncs the whole section.
pub(crate) const HOMELESS_DESPAWN_VERSION: i32 = 315;

fn read_town_npcs(r: &mut PacketReader<'_>, version: i32) -> TownNpcSection {
    let mut section = TownNpcSection {
        shimmered: Vec::new(),
        npcs: Vec::new(),
        saved_npcs: Vec::new(),
        complete: false,
    };

    let Ok(shimmered_count) = r.i32() else {
        return section;
    };
    for _ in 0..shimmered_count.clamp(0, 1 << 12) {
        let Ok(id) = r.i32() else {
            return section;
        };
        section.shimmered.push(id);
    }

    // Each entry is led by its own boolean and the list ends with a bare `false`, so running out
    // of bytes mid-entry is a truncated section, not the end of one.
    loop {
        match r.bool() {
            Ok(true) => {}
            // The terminator: every resident was read. The second list, of the Lunar Pillars,
            // follows immediately — fall through to it rather than returning here.
            Ok(false) => break,
            Err(_) => return section,
        }
        let entry = (|| {
            let net_id = r.i32().ok()?;
            let name = r.string().ok()?;
            let x = r.f32().ok()?;
            let y = r.f32().ok()?;
            let homeless = r.bool().ok()?;
            let home_x = r.i32().ok()?;
            let home_y = r.i32().ok()?;
            // A flag byte whose first bit says a variation index follows.
            let flags = r.u8().ok()?;
            let variation = if flags & 1 != 0 { r.i32().ok()? } else { 0 };
            let homeless_despawn = if version >= HOMELESS_DESPAWN_VERSION {
                r.bool().ok()?
            } else {
                false
            };
            Some(super::objects::TownNpc {
                net_id,
                name,
                position: (x, y),
                homeless,
                home: (home_x, home_y),
                variation,
                homeless_despawn,
            })
        })();
        let Some(npc) = entry else {
            return section;
        };
        section.npcs.push(npc);
        if section.npcs.len() > 1_000 {
            // A malformed section rather than a world with a thousand residents. Deliberately not
            // `complete`: whatever follows was never read, so the section is carried through.
            return section;
        }
    }

    // The second list: `active` (a leading bool, doubling as this loop's own terminator),
    // `netID`, then a `Vector2` position — nothing else, unlike a `TownNpc` above.
    loop {
        match r.bool() {
            Ok(true) => {}
            Ok(false) => {
                section.complete = true;
                return section;
            }
            Err(_) => return section,
        }
        let entry = (|| {
            let net_id = r.i32().ok()?;
            let x = r.f32().ok()?;
            let y = r.f32().ok()?;
            Some(super::objects::SavedNpc {
                net_id,
                position: (x, y),
            })
        })();
        let Some(npc) = entry else {
            return section;
        };
        section.saved_npcs.push(npc);
        if section.saved_npcs.len() > 64 {
            // Real vanilla writes at most four entries here (the pillars). More than a handful
            // means this reader has drifted out of step with the format, not that the list is
            // legitimately large.
            return section;
        }
    }
}

/// Read the tile entities, reporting whether every one of them was understood.
///
/// The count is stated up front, so "complete" means exactly that many decoded. One that does not
/// — an entity kind from a newer build, say — used to end the loop and take **every entity after
/// it in the file** with it: pylons, item frames, weapon racks, mannequin contents, all silently
/// gone on the next save. Now the tail is kept as bytes instead, by not rewriting the section.
fn read_tile_entities(
    r: &mut PacketReader<'_>,
    version: i32,
) -> (Vec<terrustia_proto::tile_entity::TileEntity>, bool) {
    let Ok(count) = r.i32() else {
        return (Vec::new(), false);
    };
    let wanted = count.max(0) as usize;
    let mut entities = Vec::with_capacity(count.clamp(0, 1 << 16) as usize);
    for _ in 0..wanted {
        match terrustia_proto::tile_entity::TileEntity::read(r, false, version) {
            Ok(entity) => entities.push(entity),
            Err(_) => break,
        }
    }
    let complete = entities.len() == wanted;
    (entities, complete)
}

/// Ids `CreativePowerManager.Initialize` hands out, in its own fixed registration order
/// (`CreativePowerManager.cs:90-104`) — the id is just the 0-based position a power is
/// `Register`ed at, so it has to match that call sequence exactly. Only the six of the fifteen
/// that are `IPersistentPerWorldContent` are ever written into a world file at all; the rest
/// (one-shot day/noon/night/midnight buttons, the three per-player powers, and the two "modify"
/// sliders vanilla itself does not persist either) never appear in section 9, so this reader and
/// `wld_save::write_journey_powers` share these six and nothing else.
pub(crate) const JOURNEY_FREEZE_TIME: u16 = 0;
pub(crate) const JOURNEY_MODIFY_TIME_RATE: u16 = 8;
pub(crate) const JOURNEY_FREEZE_RAIN: u16 = 9;
pub(crate) const JOURNEY_FREEZE_WIND: u16 = 10;
pub(crate) const JOURNEY_DIFFICULTY_SLIDER: u16 = 12;
pub(crate) const JOURNEY_STOP_BIOME_SPREAD: u16 = 13;

/// Read section 9: the Journey powers, matching `CreativePowerManager.LoadFromWorld`
/// (`CreativePowerManager.cs:139-151`). A run of `(true, power id, power's own payload)` entries
/// ended by a bare `false`; the four toggles write one `bool` each and the two sliders write one
/// raw `f32` each (`ASharedTogglePower`/`ASharedSliderPower`'s own `Save` methods in
/// `CreativePowers.cs`).
///
/// An id this build does not model the payload width of stops the read outright: nothing after it
/// in the section can be trusted to be at the right offset, so whatever power was not reached
/// simply keeps its default rather than risk misreading the rest as some other power's value.
fn read_journey_powers(r: &mut PacketReader<'_>, world: &mut World) {
    loop {
        match r.bool() {
            Ok(true) => {}
            _ => return,
        }
        let Ok(id) = r.u16() else { return };
        let ok = match id {
            JOURNEY_FREEZE_TIME => r.bool().map(|v| world.journey_freeze_time = v).is_ok(),
            JOURNEY_MODIFY_TIME_RATE => r.f32().map(|v| world.journey_time_rate_slider = v).is_ok(),
            JOURNEY_FREEZE_RAIN => r.bool().map(|v| world.journey_freeze_rain = v).is_ok(),
            JOURNEY_FREEZE_WIND => r.bool().map(|v| world.journey_freeze_wind = v).is_ok(),
            JOURNEY_DIFFICULTY_SLIDER => {
                r.f32().map(|v| world.journey_difficulty_slider = v).is_ok()
            }
            JOURNEY_STOP_BIOME_SPREAD => r
                .bool()
                .map(|v| world.journey_stop_biome_spread = v)
                .is_ok(),
            // Stopping here is correct and is the whole reason this returns rather than skipping:
            // an id whose payload width this build does not know cannot be stepped over, so
            // everything after it would be read at the wrong offset. What was wrong is that it did
            // it in silence. A world saved by a newer Terraria, or carrying a power this build
            // predates, lost every setting after that point with nothing said, and the operator's
            // first clue was a toggle that would not stay put.
            unknown => {
                warn!(
                    power = unknown,
                    "this world names a Journey power this build does not know; its settings, and \
                     any stored after it, are left at their defaults because the rest of that \
                     section cannot be read at a trustworthy offset"
                );
                false
            }
        };
        if !ok {
            return;
        }
    }
}

/// Jump to a section pointer, checking it lies inside the file.
fn seek<'a>(r: &mut PacketReader<'a>, bytes: &'a [u8], pointer: i32, index: usize) -> Result<()> {
    if pointer < 0 || pointer as usize > bytes.len() {
        return Err(WldError::BadSectionPointer {
            index,
            pointer: i64::from(pointer),
            len: bytes.len(),
        });
    }
    *r = PacketReader::new(bytes);
    r.bytes(pointer as usize)
        .map_err(|source| WldError::Decode { offset: 0, source })?;
    Ok(())
}

/// Attach the current offset to a decode error.
fn num<T>(
    value: std::result::Result<T, terrustia_proto::ProtoError>,
    r: &PacketReader<'_>,
) -> Result<T> {
    value.map_err(|source| WldError::Decode {
        offset: r.position(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrustia_proto::Writer;

    /// A world at version 326 — what real Terraria 1.4.5.8 writes — must load, not be refused as
    /// `TooNew`. Serialise a generated world, stamp its leading version field to 326, and confirm
    /// it parses. With the old ceiling of 325 this exact world was rejected outright, which is what
    /// broke the round-trip-through-real-Terraria check the project depends on.
    #[test]
    fn a_world_at_version_326_loads_rather_than_being_refused() {
        let world = crate::world::worldgen::generate(400, 300, "v326", 1);
        let mut bytes = crate::world::wld_save::serialize(&world).expect("serialize");
        bytes[0..4].copy_from_slice(&326i32.to_le_bytes());
        let loaded = parse(&bytes).expect("a 1.4.5.8 (v326) world must load");
        assert_eq!(loaded.width(), 400);
    }

    /// The byte offset the section-pointer table starts at: version (4) + magic (7) + file type
    /// (1) + revision (4) + favorite (8) + section count (2). Each pointer is a little-endian i32
    /// from there on, one per section.
    const POINTER_TABLE: usize = 4 + 7 + 1 + 4 + 8 + 2;

    fn section_pointer(bytes: &[u8], index: usize) -> i32 {
        let at = POINTER_TABLE + index * 4;
        i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn set_section_pointer(bytes: &mut [u8], index: usize, value: i32) {
        let at = POINTER_TABLE + index * 4;
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// P1d: a file whose trailing-section pointers go backwards is refused with a real error.
    ///
    /// Before this check, the trailing-section loop in `parse` sliced each section out with
    /// `bytes.get(start..end.max(start))`, which silently clamped a backwards pointer to an empty
    /// range: a corrupt world loaded successfully with its townsfolk (or whichever section came
    /// after the swap) reading back as though nobody lived there, rather than being refused.
    #[test]
    fn a_file_with_out_of_order_trailing_section_pointers_is_refused() {
        let world = crate::world::worldgen::generate(400, 300, "corrupt", 1);
        let mut bytes = crate::world::wld_save::serialize(&world).expect("serialize");

        // Sections 4 and 5 are both trailing (townsfolk, then tile entities): swap their pointers
        // so section 5's now lands before section 4's.
        let (p4, p5) = (section_pointer(&bytes, 4), section_pointer(&bytes, 5));
        assert!(p5 > p4, "the fixture should not already be corrupt");
        set_section_pointer(&mut bytes, 4, p5);
        set_section_pointer(&mut bytes, 5, p4);

        match parse(&bytes) {
            Ok(_) => panic!("an out-of-order section pointer must be refused, not loaded"),
            Err(err) => assert!(
                matches!(err, WldError::SectionPointersOutOfOrder { index: 5, .. }),
                "expected SectionPointersOutOfOrder at index 5, got {err:?}"
            ),
        }
    }

    /// The whole late header, as a world of `version` writes it.
    ///
    /// Only the fields the reader distinguishes are given real values; the rest are the right
    /// widths filled with zero, which is what makes an off-by-two visible rather than plausible.
    fn late_tail(version: i32) -> Vec<u8> {
        let mut w = Writer::new();
        // The slime rain clock and the sundial, then the rain the reader keeps.
        w.f64(0.0).u8(0);
        w.bool(true).i32(600).f32(0.9);
        // Three hardmode ore tiers, eight background styles, the clouds, then the wind.
        for _ in 0..3 {
            w.i32(0);
        }
        for _ in 0..8 {
            w.u8(0);
        }
        w.i32(0).i16(0).f32(-0.3);
        // Nobody has handed in an angler quest, and the five saved townsfolk.
        w.i32(0);
        w.bool(false).i32(0).bool(false).bool(false).bool(false);
        w.i32(0).i32(0);
        // Banner kill counts, then the claimable list that only exists from 289.
        w.i16(2).i32(7).i32(9);
        if version >= 289 {
            w.i16(1).u16(1234);
        }
        w.bool(false); // fast forward to dawn
        // The nine "downed" flags: only the Moon Lord, the fourth, is set.
        for i in 0..9 {
            w.bool(i == 3);
        }
        // The nine pillar flags: only the vortex tower is standing, the sixth.
        for i in 0..9 {
            w.bool(i == 5);
        }
        // A party nobody is at.
        w.bool(false).bool(false).i32(0).i32(0);
        // The sandstorm, which is the first thing after the party with a recognisable value.
        w.bool(true).i32(4321).f32(0.25).f32(0.5);
        // The bartender and the three army tiers.
        w.bool(true).bool(true).bool(false).bool(false);
        // Five background styles, then the first combat book.
        for _ in 0..5 {
            w.u8(0);
        }
        w.bool(true);
        // Lantern night, then thirteen tree tops.
        w.i32(0).bool(false).bool(false).bool(false);
        w.i32(13);
        for _ in 0..13 {
            w.i32(1);
        }
        // Forced holidays, the four ore tiers, the three pets.
        w.bool(false).bool(false);
        for tier in [7, 167, 9, 169] {
            w.i32(tier);
        }
        for _ in 0..3 {
            w.bool(false);
        }
        // The empress, the queen and Deerclops: only the queen is down.
        w.bool(false).bool(true).bool(false);
        // Nine unlocked town spawns, then the second combat book.
        for _ in 0..9 {
            w.bool(false);
        }
        w.bool(false);
        // The Peddler's Satchel and the green slime, then the run this reader keeps: old, purple,
        // rainbow, red, yellow. Only the old and yellow slimes have been freed, which is what makes
        // a reader that skipped the wrong number of bytes read purple or red as true instead.
        w.bool(false).bool(false);
        w.bool(true).bool(false).bool(false).bool(false).bool(true);
        w.into_bytes()
    }

    fn walk(version: i32) -> (Progress, Weather) {
        let bytes = late_tail(version);
        let mut r = PacketReader::new(&bytes);
        let mut progress = Progress::default();
        let mut weather = Weather::default();
        let mut offsets = LateOffsets::default();
        let mut ore_tiers = [-1i16; 7];
        read_late_header(
            &mut r,
            version,
            &mut progress,
            &mut weather,
            &mut ore_tiers,
            &mut std::collections::HashMap::new(),
            &mut Scenery::default(),
            &mut offsets,
            0,
        )
        .unwrap_or_else(|e| panic!("version {version}: {e}"));
        (progress, weather)
    }

    /// A world older than 289 has no claimable-banner list, and reading one anyway puts every
    /// flag after it two bytes out.
    ///
    /// The two bytes come back as a count of zero, so nothing fails loudly: the world simply
    /// reports the wrong bosses. Both versions have to land on the same answers.
    #[test]
    fn the_claimable_banner_list_is_gated_on_its_version() {
        for version in [MIN_VERSION, 288, 289, 319] {
            let (p, weather) = walk(version);
            assert!(p.downed_moon_lord, "{version}: the moon lord");
            assert!(!p.downed_fishron, "{version}: fishron, which is not down");
            assert!(
                p.tower_active_vortex,
                "{version}: the vortex tower standing"
            );
            assert!(!p.downed_tower_solar, "{version}: solar, never beaten");
            assert!(
                weather.raining && weather.rain_time == 600,
                "{version}: rain"
            );
            assert_eq!(weather.wind, -0.3, "{version}: wind");
            assert!(weather.sandstorm, "{version}: the sandstorm");
            assert_eq!(weather.sandstorm_time, 4321, "{version}");
            assert_eq!(weather.severity, 0.25, "{version}");
            assert!(p.saved_bartender, "{version}: the bartender");
            assert!(p.downed_army_t1 && !p.downed_army_t2, "{version}: the army");
            assert!(p.combat_book && !p.combat_book_two, "{version}: the books");
            assert!(p.downed_queen_slime, "{version}: the queen");
            assert!(!p.downed_empress_of_light, "{version}: the empress");
            assert!(!p.downed_deerclops, "{version}: deerclops");
            // The slime unlocks are the last thing in the header this reader keeps, so they are
            // the field an off-by-one anywhere above lands on. Old and yellow freed, purple not,
            // and the two unmodelled ones (rainbow, red) skipped rather than mistaken for these.
            assert!(p.unlocked_slime_old, "{version}: the old slime");
            assert!(!p.unlocked_slime_purple, "{version}: the purple slime");
            assert!(p.unlocked_slime_yellow, "{version}: the yellow slime");
        }
    }

    /// A townsfolk section that stops mid-entry keeps what it read and says it is incomplete.
    ///
    /// This is the bug that could have deleted a whole town. The old reader committed the section
    /// with `if let Ok(..)`, so *any* error — including one after several residents had decoded —
    /// threw the lot away, and the save then wrote back an empty list. Every resident, their name
    /// and their house, gone on the first autosave.
    #[test]
    fn a_truncated_townsfolk_section_is_not_silently_emptied() {
        use terrustia_proto::Writer;

        let mut w = Writer::with_capacity(64);
        w.i32(0); // no shimmered types
        // One complete resident.
        w.bool(true)
            .i32(22)
            .string("Andrew")
            .f32(100.0)
            .f32(200.0)
            .bool(false)
            .i32(10)
            .i32(20)
            .u8(0)
            .bool(false);
        // A second that runs out of bytes halfway through its name.
        w.bool(true).i32(17);
        let bytes = w.into_bytes();

        let mut r = PacketReader::new(&bytes);
        let read = read_town_npcs(&mut r, 326);

        assert_eq!(read.npcs.len(), 1, "the resident that did decode is kept");
        assert_eq!(read.npcs[0].name, "Andrew");
        assert!(
            !read.complete,
            "a section that ran out of bytes must not claim to be complete, or the save \
             rewrites it from a partial read and loses the rest"
        );
    }

    /// A whole, well-formed section reports itself complete, so it is still rewritten normally.
    ///
    /// Also covers the second list: one pillar after the town-list terminator, then that list's
    /// own terminator, must both decode and still leave `complete` true.
    #[test]
    fn a_whole_townsfolk_section_is_complete() {
        use terrustia_proto::Writer;

        let mut w = Writer::with_capacity(64);
        w.i32(1);
        w.i32(5); // one shimmered type
        w.bool(true)
            .i32(22)
            .string("Andrew")
            .f32(1.0)
            .f32(2.0)
            .bool(true)
            .i32(0)
            .i32(0)
            .u8(0)
            .bool(false);
        w.bool(false); // the town-list terminator
        // The second list: one pillar, then its own terminator.
        w.bool(true)
            .i32(crate::game::lunar::VORTEX as i32)
            .f32(300.0)
            .f32(400.0);
        w.bool(false);
        let bytes = w.into_bytes();

        let mut r = PacketReader::new(&bytes);
        let read = read_town_npcs(&mut r, 326);

        assert_eq!(read.shimmered, vec![5]);
        assert_eq!(read.npcs.len(), 1);
        assert_eq!(read.saved_npcs.len(), 1, "the pillar in the second list");
        assert_eq!(read.saved_npcs[0].net_id, crate::game::lunar::VORTEX as i32);
        assert_eq!(read.saved_npcs[0].position, (300.0, 400.0));
        assert!(
            read.complete,
            "a section ending in both terminators is whole"
        );
    }

    /// The L3-02 bug directly: a section whose town list decodes fully but whose second (pillar)
    /// list is truncated must NOT be reported complete, or the save rewrites the section from a
    /// partial read and silently drops whichever pillars it did not reach.
    #[test]
    fn a_townsfolk_section_truncated_in_the_second_list_is_not_complete() {
        use terrustia_proto::Writer;

        let mut w = Writer::with_capacity(64);
        w.i32(0); // no shimmered types
        w.bool(false); // an empty, but well-formed, town list
        // The second list starts, then runs out of bytes before the position.
        w.bool(true).i32(crate::game::lunar::SOLAR as i32).f32(1.0);
        let bytes = w.into_bytes();

        let mut r = PacketReader::new(&bytes);
        let read = read_town_npcs(&mut r, 326);

        assert!(
            !read.complete,
            "a section that ran out of bytes in the pillar list must not be reported as \
             understood, or the save writes back the pillars it did manage to read and \
             silently drops the rest"
        );
    }

    /// The plain reading of `WorldFile.SaveNPCs`'s second loop: `active`/`netID`/position, and
    /// nothing else — unlike a `TownNpc`, no name, no home, no variation byte.
    #[test]
    fn the_second_list_decodes_type_and_position_only() {
        use terrustia_proto::Writer;

        let mut w = Writer::with_capacity(64);
        w.i32(0); // no shimmered types
        w.bool(false); // empty town list
        for (ty, x, y) in [
            (crate::game::lunar::SOLAR, 10.0f32, 20.0f32),
            (crate::game::lunar::VORTEX, 30.0, 40.0),
            (crate::game::lunar::NEBULA, 50.0, 60.0),
            (crate::game::lunar::STARDUST, 70.0, 80.0),
        ] {
            w.bool(true).i32(ty as i32).f32(x).f32(y);
        }
        w.bool(false); // the second list's own terminator
        let bytes = w.into_bytes();

        let mut r = PacketReader::new(&bytes);
        let read = read_town_npcs(&mut r, 326);

        assert!(read.complete);
        assert_eq!(read.saved_npcs.len(), 4, "all four pillars");
        let types: Vec<i32> = read.saved_npcs.iter().map(|n| n.net_id).collect();
        assert_eq!(
            types,
            vec![
                crate::game::lunar::SOLAR as i32,
                crate::game::lunar::VORTEX as i32,
                crate::game::lunar::NEBULA as i32,
                crate::game::lunar::STARDUST as i32,
            ]
        );
        assert_eq!(read.saved_npcs[2].position, (50.0, 60.0));
    }

    /// A pre-315 world (e.g. 1.4.4.x, still within `MIN_VERSION`) wrote no `homelessDespawn` byte.
    /// Reading one there would eat the section terminator and desync everything after it — the exact
    /// corruption that made such a world fail to load in real Terraria after a round-trip. Two
    /// residents with no despawn byte, ending in the terminator, must decode whole - the second
    /// list is unconditional on file version (`LoadNPCs` gates it on `>= 140`, well below this
    /// reader's own `MIN_VERSION` floor of 279), so an empty one still needs its own terminator.
    #[test]
    fn a_pre_315_townsfolk_section_has_no_despawn_byte() {
        use terrustia_proto::Writer;

        let mut w = Writer::with_capacity(64);
        w.i32(0); // no shimmered types
        for (id, name) in [(22, "Andrew"), (17, "Steve")] {
            w.bool(true)
                .i32(id)
                .string(name)
                .f32(1.0)
                .f32(2.0)
                .bool(false)
                .i32(0)
                .i32(0)
                .u8(0);
            // deliberately NO homeless_despawn byte — this is a <315 world
        }
        w.bool(false); // town-list terminator
        w.bool(false); // second-list terminator (empty)
        let bytes = w.into_bytes();

        let mut r = PacketReader::new(&bytes);
        let read = read_town_npcs(&mut r, 279);
        assert!(
            read.complete,
            "a pre-315 section without despawn bytes must decode whole, not desync"
        );
        assert_eq!(read.npcs.len(), 2);
        assert_eq!(read.npcs[1].name, "Steve");
        assert!(!read.npcs[0].homeless_despawn, "defaults to false pre-315");
    }

    /// Tile entities: fewer decoded than the count promised means the tail was not understood.
    #[test]
    fn a_short_tile_entity_section_reports_itself_incomplete() {
        use terrustia_proto::Writer;

        let mut w = Writer::with_capacity(32);
        w.i32(3); // claims three
        let bytes = w.into_bytes(); // supplies none

        let mut r = PacketReader::new(&bytes);
        let (entities, complete) = read_tile_entities(&mut r, 326);

        assert!(entities.is_empty());
        assert!(
            !complete,
            "three promised and none delivered must not be reported as understood"
        );
    }

    /// A world from a future Terraria is refused, not guessed at.
    ///
    /// This is the quiet one. A newer format does not fail to parse — it parses *positionally*,
    /// lands the clock offsets wherever the old layout put them, and then the first autosave
    /// writes the time over whatever field now occupies those bytes. Failing closed is the only
    /// safe answer, and it is what vanilla does too.
    #[test]
    fn a_world_newer_than_this_build_is_refused() {
        use terrustia_proto::Writer;

        let mut w = Writer::with_capacity(32);
        w.i32(MAX_VERSION + 1).bytes(MAGIC).u8(FILE_TYPE_WORLD);
        let bytes = w.into_bytes();

        match parse(&bytes) {
            Err(WldError::TooNew { found }) => assert_eq!(found, MAX_VERSION + 1),
            Err(other) => panic!("expected TooNew, got {other}"),
            Ok(_) => panic!("a future version must not be accepted"),
        }
    }

    /// The ceiling must not shut out the version we ourselves write.
    ///
    /// A `const` block rather than an assertion, so raising `SAVE_VERSION` past `MAX_VERSION`
    /// fails the build instead of a test run — the reader has to accept what the writer emits.
    const _: () = assert!(MAX_VERSION >= super::super::wld_save::SAVE_VERSION);
}
