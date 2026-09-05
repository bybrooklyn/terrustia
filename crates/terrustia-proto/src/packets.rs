//! Typed packets for the connection handshake and player sync.
//!
//! Field order is transcribed from the 1.4.5.7 client's `MessageBuffer.GetData` and
//! `NetMessage.SendData`; see `docs/protocol-notes.md`. Reordering anything here does not produce
//! an error at the client, it produces a silent hang, so the golden-byte tests below pin the
//! layouts rather than trusting the round-trips alone.

use crate::{
    error::{ProtoError, Result},
    id,
    net_text::NetworkText,
    reader::PacketReader,
    writer::{PacketWriter, Writer},
};

/// Packet `1`: the first thing a client sends, carrying its protocol version string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub version: String,
}

impl Hello {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        Ok(Self {
            version: r.string()?,
        })
    }

    /// Whether this client speaks a release this server understands.
    ///
    /// A range, not one string. 1.4.5.7 and 1.4.5.8 differ only in the number they announce, so
    /// pinning to one of them turns a cosmetic patch release into a locked door.
    pub fn is_supported(&self) -> bool {
        id::SUPPORTED_RELEASES
            .iter()
            .any(|release| self.version == format!("Terraria{release}"))
    }
}

/// Packet `2`: disconnect with a reason the client displays.
pub fn kick(reason: &NetworkText) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(id::KICK);
    reason.write(&mut w);
    w.finish()
}

/// Packet `3`: assign the client its player slot.
///
/// The trailing bool is new in 1.4.5; a 1.4.4 client expected only the slot byte.
pub fn player_info(slot: u8, check_bytes_in_client_loop: bool) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(id::PLAYER_INFO);
    w.u8(slot).bool(check_bytes_in_client_loop);
    w.finish()
}

/// Packet `8`: the client asking for the tiles around a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnTileData {
    pub x: i32,
    pub y: i32,
    pub team: u8,
}

impl SpawnTileData {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        Ok(Self {
            x: r.i32()?,
            y: r.i32()?,
            team: r.u8()?,
        })
    }
}

/// Packet `9`: extends the client's loading bar and sets its caption.
pub fn status_text(steps: i32, text: &NetworkText, flags: u8) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(id::STATUS_TEXT_SIZE);
    w.i32(steps);
    text.write(&mut w);
    w.u8(flags);
    w.finish()
}

/// Packet `12`: where and how a player enters the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerSpawn {
    pub player: u8,
    pub spawn_x: i16,
    pub spawn_y: i16,
    pub respawn_timer: i32,
    pub deaths_pve: i16,
    pub deaths_pvp: i16,
    pub team: u8,
    pub context: u8,
}

impl PlayerSpawn {
    /// `PlayerSpawnContext.SpawningIntoWorld`, the context used when first joining.
    pub const CONTEXT_SPAWNING_INTO_WORLD: u8 = 1;

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        Ok(Self {
            player: r.u8()?,
            spawn_x: r.i16()?,
            spawn_y: r.i16()?,
            respawn_timer: r.i32()?,
            deaths_pve: r.i16()?,
            deaths_pvp: r.i16()?,
            team: r.u8()?,
            context: r.u8()?,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::PLAYER_SPAWN);
        w.u8(self.player)
            .i16(self.spawn_x)
            .i16(self.spawn_y)
            .i32(self.respawn_timer)
            .i16(self.deaths_pve)
            .i16(self.deaths_pvp)
            .u8(self.team)
            .u8(self.context);
        w.finish()
    }
}

/// Packet `14`: announce that a player slot is occupied, or has been vacated.
pub fn player_active(player: u8, active: bool) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(id::PLAYER_ACTIVE);
    w.u8(player).bool(active);
    w.finish()
}

/// Packet `16`: current and maximum life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerHealth {
    pub player: u8,
    pub life: i16,
    pub life_max: i16,
}

impl PlayerHealth {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        Ok(Self {
            player: r.u8()?,
            life: r.i16()?,
            life_max: r.i16()?,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::PLAYER_LIFE_MANA);
        w.u8(self.player).i16(self.life).i16(self.life_max);
        w.finish()
    }
}

/// Packet `42`: current and maximum mana.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerMana {
    pub player: u8,
    pub mana: i16,
    pub mana_max: i16,
}

impl PlayerMana {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        Ok(Self {
            player: r.u8()?,
            mana: r.i16()?,
            mana_max: r.i16()?,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::PLAYER_MANA);
        w.u8(self.player).i16(self.mana).i16(self.mana_max);
        w.finish()
    }
}

/// Packet `66` (`id::UNKNOWN66` — genuinely unnamed in vanilla's own `MessageID.cs`): a
/// heal-on-touch projectile's own effect (`Projectile.cs:28951`, `aiStyle == 52`) landing on a
/// player, and its relay. The owning client computes the amount and applies it to its own local
/// copy before ever sending this; the server's job is only to apply the same amount to its own
/// authoritative copy and pass it on, exactly as `MessageBuffer.cs:3038-3056`'s own `case 66` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealPlayer {
    pub player: u8,
    pub amount: i16,
}

impl HealPlayer {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        Ok(Self {
            player: r.u8()?,
            amount: r.i16()?,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::UNKNOWN66);
        w.u8(self.player).i16(self.amount);
        w.finish()
    }
}

/// Packet `18`: world clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeSet {
    pub day_time: bool,
    pub time: i32,
    pub sun_mod_y: i16,
    pub moon_mod_y: i16,
}

impl TimeSet {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::SET_TIME);
        w.bool(self.day_time)
            .i32(self.time)
            .i16(self.sun_mod_y)
            .i16(self.moon_mod_y);
        w.finish()
    }
}

/// Packets `49` and `129`: both are bare signals with no payload.
/// Packet 79: place a multi-tile object.
///
/// Relayed to every other client so they place it themselves; the server has already written the
/// tiles into its own world by the time this goes out.
pub fn place_object(x: i32, y: i32, block: u16, style: i32, random: i32) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(crate::id::PLACE_OBJECT);
    w.i16(x as i16);
    w.i16(y as i16);
    w.i16(block as i16);
    w.i16(style as i16);
    // The alternate index; the server does not choose one.
    w.u8(0);
    w.i8(random as i8);
    // Direction, which the client applies to the sprite rather than to the tiles.
    w.bool(true);
    w.finish()
}

pub fn empty(message_id: u8) -> Result<Vec<u8>> {
    PacketWriter::new(message_id).finish()
}

/// Packet `57`: how much of the world is hallow, corruption and crimson, as whole percentages.
///
/// The Dryad reads all three out when you talk to her, and the client has no way to work them out
/// for itself — it only ever sees the sections it has asked for. Without this she reports a world
/// that is nought per cent of everything.
///
/// Bytes, not integers: the game counts tiles in ints and then rounds each to a percentage before
/// it sends them.
pub fn world_evil_tally(hallow: u8, corruption: u8, crimson: u8) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(id::UNKNOWN57);
    w.u8(hallow).u8(corruption).u8(crimson);
    w.finish()
}

/// What the game's `TownRoomManager.GetHouseholdStatus` reports about where a town NPC lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HouseholdStatus {
    /// Has a home tile, but no room on record for it.
    Settled = 0,
    /// No home at all: newly arrived, or evicted.
    Homeless = 1,
    /// Has a home and a room the game agrees is habitable.
    Housed = 2,
}

/// Packet `60`: where a town NPC lives.
///
/// Sent for every town NPC as a player joins, and again whenever one moves. It is what the housing
/// screen draws its banners and its "this room is home to" line from; a client never told is a
/// client whose housing menu is empty however many villagers are walking about.
pub fn npc_home(npc: u16, home_x: i16, home_y: i16, status: HouseholdStatus) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(id::UNKNOWN60);
    w.i16(npc as i16).i16(home_x).i16(home_y).u8(status as u8);
    w.finish()
}

/// Packet `139`: whether this player's slot counts as the host for gameplay purposes.
///
/// The game's rule is simply whether the connection came from the loopback address — somebody
/// playing on the same machine the server runs on. It is only sent when true.
pub fn counts_as_host(player: u8, is_host: bool) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(id::SET_COUNTS_AS_HOST_FOR_GAMEPLAY);
    w.u8(player).bool(is_host);
    w.finish()
}

/// Packet `13`: a client's control and position update.
///
/// Only the fields the server actually needs are parsed. The trailing optional blocks (velocity,
/// mount, potion-of-return positions, camera target) depend on flag bits, so the payload is
/// re-broadcast verbatim rather than re-serialised — see [`rewrite_owner`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerControls {
    pub player: u8,
    pub control_flags: [u8; 4],
    pub selected_item: u8,
    pub position: (f32, f32),
    pub velocity: Option<(f32, f32)>,
}

impl PlayerControls {
    /// Bit 2 of the second control byte says a velocity pair follows.
    const HAS_VELOCITY: u8 = 0x04;

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        let player = r.u8()?;
        let control_flags = [r.u8()?, r.u8()?, r.u8()?, r.u8()?];
        let selected_item = r.u8()?;
        let position = r.vec2()?;
        let velocity = if control_flags[1] & Self::HAS_VELOCITY != 0 {
            Some(r.vec2()?)
        } else {
            None
        };
        Ok(Self {
            player,
            control_flags,
            selected_item,
            position,
            velocity,
        })
    }

    pub fn facing_right(&self) -> bool {
        self.control_flags[0] & 0x40 != 0
    }

    /// Bit 2 of the third control byte (`MessageBuffer.cs`'s own `bitsByte26[2]`,
    /// `player13.sitting.isSitting = bitsByte26[2]`).
    pub fn sitting(&self) -> bool {
        self.control_flags[2] & 0x04 != 0
    }
}

/// Rewrite the leading player-slot byte of a payload so a relayed packet is attributed to the
/// sender rather than to whatever slot the client claimed.
///
/// Clients are not trusted to report their own slot; every relayed packet goes through this.
/// Rebuild a packet exactly as it arrived.
///
/// For the packets a server passes along untouched: it has already decided the message is
/// acceptable, and re-encoding rather than forwarding the raw bytes means the length prefix is
/// always right even if the payload came from somewhere odd.
pub fn verbatim(message_id: u8, payload: &[u8]) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(message_id);
    w.bytes(payload);
    w.finish()
}

pub fn rewrite_owner(message_id: u8, payload: &[u8], owner: u8) -> Result<Vec<u8>> {
    if payload.is_empty() {
        return Err(ProtoError::Eof {
            offset: 0,
            needed: 1,
            available: 0,
        });
    }
    let mut w = PacketWriter::new(message_id);
    w.u8(owner).bytes(&payload[1..]);
    w.finish()
}

/// Named positions inside the eleven world-flag bytes of packet `7`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorldFlags(pub [u8; 11]);

impl WorldFlags {
    fn set(&mut self, byte: usize, bit: u8, on: bool) {
        if on {
            self.0[byte] |= 1 << bit;
        } else {
            self.0[byte] &= !(1 << bit);
        }
    }

    /// Selects crimson over corruption as the world's evil biome.
    pub fn set_crimson(&mut self, on: bool) {
        self.set(1, 5, on);
    }

    pub fn set_hardmode(&mut self, on: bool) {
        self.set(0, 4, on);
    }

    /// Server-side characters change how the client treats its inventory; leave this off unless
    /// the server actually stores player data.
    pub fn set_server_side_character(&mut self, on: bool) {
        self.set(0, 6, on);
    }

    /// Set one flag by name.
    ///
    /// The client reads all eleven bytes and drives real behaviour off them — which shops open,
    /// which ores a Dryad talks about, whether the map shows an event — so a server that leaves
    /// them blank leaves the client believing a fresh world however far along the save is.
    pub fn set_flag(&mut self, flag: WorldFlag, on: bool) {
        let (byte, bit) = flag.position();
        self.set(byte, bit, on);
    }

    /// Read one flag back, which is what lets a test assert on what a client will actually be
    /// told rather than on the server-side field the bit was derived from.
    pub fn has_flag(&self, flag: WorldFlag) -> bool {
        let (byte, bit) = flag.position();
        self.0[byte] & (1 << bit) != 0
    }
}

/// The world flags of packet `7`, in the order the client unpacks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldFlag {
    ShadowOrbSmashed,
    DownedBoss1,
    DownedBoss2,
    DownedBoss3,
    HardMode,
    DownedClown,
    ServerSideCharacter,
    DownedPlantera,
    DownedMech1,
    DownedMech2,
    DownedMech3,
    DownedMechAny,
    CloudBackground,
    Crimson,
    PumpkinMoon,
    SnowMoon,
    FastForwardToDawn,
    SlimeRain,
    DownedKingSlime,
    DownedQueenBee,
    DownedFishron,
    DownedMartians,
    DownedAncientCultist,
    DownedMoonLord,
    DownedHalloweenKing,
    DownedHalloweenTree,
    DownedChristmasIceQueen,
    DownedChristmasSantank,
    DownedChristmasTree,
    DownedGolem,
    PartyIsUp,
    DownedPirates,
    DownedFrostLegion,
    DownedGoblins,
    Sandstorm,
    ArmyOngoing,
    DownedArmyTier1,
    DownedArmyTier2,
    DownedArmyTier3,
    CombatBookUsed,
    LanternNight,
    DownedTowerSolar,
    DownedTowerVortex,
    DownedTowerNebula,
    DownedTowerStardust,
    HalloweenToday,
    ChristmasToday,
    BoughtCat,
    BoughtDog,
    BoughtBunny,
    FreeCake,
    DrunkWorld,
    DownedEmpressOfLight,
    DownedQueenSlime,
    GetGoodWorld,
    TenthAnniversary,
    DontStarve,
    DownedDeerclops,
    NotTheBees,
    RemixWorld,
    UnlockedSlimeBlueSpawn,
    CombatBookTwoUsed,
    /// The three bound town slimes, once each has been freed (`NetMessage.cs:363-369`).
    ///
    /// Each one is an *anti*-gate on its own bound form's spawning rather than a permission to
    /// spawn: the game offers the bound slime only while its flag is still false. A client that is
    /// never told keeps offering the "you can free me" cursor for a slime that is already a
    /// resident, so these belong on the wire even though nothing the client draws depends on them.
    UnlockedSlimeOldSpawn,
    UnlockedSlimePurpleSpawn,
    UnlockedSlimeYellowSpawn,
}

impl WorldFlag {
    /// Which byte and bit of the flag block this one lives in.
    const fn position(self) -> (usize, u8) {
        use WorldFlag::*;
        match self {
            ShadowOrbSmashed => (0, 0),
            DownedBoss1 => (0, 1),
            DownedBoss2 => (0, 2),
            DownedBoss3 => (0, 3),
            HardMode => (0, 4),
            DownedClown => (0, 5),
            ServerSideCharacter => (0, 6),
            DownedPlantera => (0, 7),
            DownedMech1 => (1, 0),
            DownedMech2 => (1, 1),
            DownedMech3 => (1, 2),
            DownedMechAny => (1, 3),
            CloudBackground => (1, 4),
            Crimson => (1, 5),
            PumpkinMoon => (1, 6),
            SnowMoon => (1, 7),
            FastForwardToDawn => (2, 1),
            SlimeRain => (2, 2),
            DownedKingSlime => (2, 3),
            DownedQueenBee => (2, 4),
            DownedFishron => (2, 5),
            DownedMartians => (2, 6),
            DownedAncientCultist => (2, 7),
            DownedMoonLord => (3, 0),
            DownedHalloweenKing => (3, 1),
            DownedHalloweenTree => (3, 2),
            DownedChristmasIceQueen => (3, 3),
            DownedChristmasSantank => (3, 4),
            DownedChristmasTree => (3, 5),
            DownedGolem => (3, 6),
            PartyIsUp => (3, 7),
            DownedPirates => (4, 0),
            DownedFrostLegion => (4, 1),
            DownedGoblins => (4, 2),
            Sandstorm => (4, 3),
            ArmyOngoing => (4, 4),
            DownedArmyTier1 => (4, 5),
            DownedArmyTier2 => (4, 6),
            DownedArmyTier3 => (4, 7),
            CombatBookUsed => (5, 0),
            LanternNight => (5, 1),
            DownedTowerSolar => (5, 2),
            DownedTowerVortex => (5, 3),
            DownedTowerNebula => (5, 4),
            DownedTowerStardust => (5, 5),
            HalloweenToday => (5, 6),
            ChristmasToday => (5, 7),
            BoughtCat => (6, 0),
            BoughtDog => (6, 1),
            BoughtBunny => (6, 2),
            FreeCake => (6, 3),
            DrunkWorld => (6, 4),
            DownedEmpressOfLight => (6, 5),
            DownedQueenSlime => (6, 6),
            GetGoodWorld => (6, 7),
            TenthAnniversary => (7, 0),
            DontStarve => (7, 1),
            DownedDeerclops => (7, 2),
            NotTheBees => (7, 3),
            RemixWorld => (7, 4),
            UnlockedSlimeBlueSpawn => (7, 5),
            CombatBookTwoUsed => (7, 6),
            // Byte 8 is `bitsByte12` (`NetMessage.cs:362-374`), whose eight bits are the green,
            // old, purple, rainbow, red, yellow and copper slime unlocks and then
            // `fastForwardTimeToDusk`. Only the three this server models are named.
            UnlockedSlimeOldSpawn => (8, 1),
            UnlockedSlimePurpleSpawn => (8, 2),
            UnlockedSlimeYellowSpawn => (8, 5),
        }
    }
}

/// Packet `7`: everything the client needs to know about the world before tiles arrive.
///
/// The payload is exactly 159 bytes plus the encoded world name and any extra spawn points.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldData {
    pub time: i32,
    pub day_time: bool,
    pub blood_moon: bool,
    pub eclipse: bool,
    pub moon_phase: u8,
    pub max_tiles_x: i16,
    pub max_tiles_y: i16,
    pub spawn_tile_x: i16,
    pub spawn_tile_y: i16,
    pub world_surface: i16,
    pub rock_layer: i16,
    pub world_id: i32,
    pub world_name: String,
    pub game_mode: u8,
    pub unique_id: [u8; 16],
    pub world_gen_version: u64,
    pub moon_type: u8,
    /// Background styles, in the order the client applies them: 0, 10, 11, 12, 1..9.
    pub backgrounds: [u8; 13],
    pub ice_back_style: u8,
    pub jungle_back_style: u8,
    pub hell_back_style: u8,
    pub wind_speed_target: f32,
    pub num_clouds: u8,
    pub tree_x: [i32; 3],
    pub tree_style: [u8; 4],
    pub cave_back_x: [i32; 3],
    pub cave_back_style: [u8; 4],
    /// One byte per `TreeTopsInfo.AreaId`, of which there are 13.
    pub tree_tops: [u8; 13],
    pub max_raining: f32,
    pub flags: WorldFlags,
    pub sundial_cooldown: u8,
    pub moondial_cooldown: u8,
    /// copper, iron, silver, gold, cobalt, mythril, adamantite.
    pub ore_tiers: [i16; 7],
    pub invasion_type: i8,
    pub lobby_id: u64,
    pub sandstorm_severity: f32,
    pub extra_spawn_points: Vec<(i16, i16)>,
    /// Where the dungeon entrance is, in tiles.
    ///
    /// New in release 326. It is not in the 1.4.5.7 source this project is written against, and it
    /// was found the only way it could be: by connecting to a real 1.4.5.8 server and finding four
    /// bytes left over at the end of its packet 7. Two worlds' worth of captures matched their own
    /// `.wld` files' dungeon positions exactly, which is what turned a guess into a fact.
    ///
    /// Omitting it is not a cosmetic shortfall. A 326 client reads these four bytes whether or not
    /// they were sent, so a packet 7 without them leaves the client parsing four bytes of the next
    /// packet as a dungeon position and then desynchronised for the rest of the session.
    pub dungeon_x: i16,
    pub dungeon_y: i16,
}

/// Byte count of a `WorldData` payload before the world name and extra spawn points.
pub const WORLD_DATA_FIXED_LEN: usize = 163;

impl WorldData {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::WORLD_DATA);
        self.write_payload(&mut w);
        w.finish()
    }

    /// Read a packet 7 payload back into its fields.
    ///
    /// The server never needs this — it only ever writes packet 7 — which is exactly why it is
    /// worth having. Pointed at a capture from a real `TerrariaServer`, a decode that succeeds and
    /// re-encodes to the identical bytes is proof that this struct's field order, widths and
    /// signedness match Re-Logic's, rather than merely matching our own reading of them. Every
    /// other check available here is symmetric and cannot tell those two apart.
    ///
    /// Strict about trailing bytes for the same reason: a layout that is one field short still
    /// decodes happily if the leftovers are ignored.
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        let time = r.i32()?;
        let day_flags = r.u8()?;
        let moon_phase = r.u8()?;
        let max_tiles_x = r.i16()?;
        let max_tiles_y = r.i16()?;
        let spawn_tile_x = r.i16()?;
        let spawn_tile_y = r.i16()?;
        let world_surface = r.i16()?;
        let rock_layer = r.i16()?;
        let world_id = r.i32()?;
        let world_name = r.string()?;
        let game_mode = r.u8()?;
        let mut unique_id = [0u8; 16];
        unique_id.copy_from_slice(r.bytes(16)?);
        let world_gen_version = r.u64()?;
        let moon_type = r.u8()?;
        let mut backgrounds = [0u8; 13];
        backgrounds.copy_from_slice(r.bytes(13)?);
        let ice_back_style = r.u8()?;
        let jungle_back_style = r.u8()?;
        let hell_back_style = r.u8()?;
        let wind_speed_target = r.f32()?;
        let num_clouds = r.u8()?;

        let mut tree_x = [0i32; 3];
        for slot in &mut tree_x {
            *slot = r.i32()?;
        }
        let mut tree_style = [0u8; 4];
        tree_style.copy_from_slice(r.bytes(4)?);
        let mut cave_back_x = [0i32; 3];
        for slot in &mut cave_back_x {
            *slot = r.i32()?;
        }
        let mut cave_back_style = [0u8; 4];
        cave_back_style.copy_from_slice(r.bytes(4)?);
        let mut tree_tops = [0u8; 13];
        tree_tops.copy_from_slice(r.bytes(13)?);
        let max_raining = r.f32()?;
        let mut flag_bytes = [0u8; 11];
        flag_bytes.copy_from_slice(r.bytes(11)?);
        let sundial_cooldown = r.u8()?;
        let moondial_cooldown = r.u8()?;

        let mut ore_tiers = [0i16; 7];
        for slot in &mut ore_tiers {
            *slot = r.i16()?;
        }
        let invasion_type = r.i8()?;
        let lobby_id = r.u64()?;
        let sandstorm_severity = r.f32()?;

        let extra = r.u8()? as usize;
        let mut extra_spawn_points = Vec::with_capacity(extra);
        for _ in 0..extra {
            extra_spawn_points.push((r.i16()?, r.i16()?));
        }
        let dungeon_x = r.i16()?;
        let dungeon_y = r.i16()?;
        if !r.is_empty() {
            return Err(ProtoError::TrailingBytes {
                left: r.remaining(),
            });
        }

        Ok(Self {
            time,
            day_time: day_flags & 0b001 != 0,
            blood_moon: day_flags & 0b010 != 0,
            eclipse: day_flags & 0b100 != 0,
            moon_phase,
            max_tiles_x,
            max_tiles_y,
            spawn_tile_x,
            spawn_tile_y,
            world_surface,
            rock_layer,
            world_id,
            world_name,
            game_mode,
            unique_id,
            world_gen_version,
            moon_type,
            backgrounds,
            ice_back_style,
            jungle_back_style,
            hell_back_style,
            wind_speed_target,
            num_clouds,
            tree_x,
            tree_style,
            cave_back_x,
            cave_back_style,
            tree_tops,
            max_raining,
            flags: WorldFlags(flag_bytes),
            sundial_cooldown,
            moondial_cooldown,
            ore_tiers,
            invasion_type,
            lobby_id,
            sandstorm_severity,
            extra_spawn_points,
            dungeon_x,
            dungeon_y,
        })
    }

    fn write_payload(&self, w: &mut Writer) {
        let day_flags = u8::from(self.day_time)
            | (u8::from(self.blood_moon) << 1)
            | (u8::from(self.eclipse) << 2);

        w.i32(self.time)
            .u8(day_flags)
            .u8(self.moon_phase)
            .i16(self.max_tiles_x)
            .i16(self.max_tiles_y)
            .i16(self.spawn_tile_x)
            .i16(self.spawn_tile_y)
            .i16(self.world_surface)
            .i16(self.rock_layer)
            .i32(self.world_id)
            .string(&self.world_name)
            .u8(self.game_mode)
            .bytes(&self.unique_id)
            .u64(self.world_gen_version)
            .u8(self.moon_type)
            .bytes(&self.backgrounds)
            .u8(self.ice_back_style)
            .u8(self.jungle_back_style)
            .u8(self.hell_back_style)
            .f32(self.wind_speed_target)
            .u8(self.num_clouds);

        for x in self.tree_x {
            w.i32(x);
        }
        w.bytes(&self.tree_style);
        for x in self.cave_back_x {
            w.i32(x);
        }
        w.bytes(&self.cave_back_style)
            .bytes(&self.tree_tops)
            .f32(self.max_raining)
            .bytes(&self.flags.0)
            .u8(self.sundial_cooldown)
            .u8(self.moondial_cooldown);

        for tier in self.ore_tiers {
            w.i16(tier);
        }
        w.i8(self.invasion_type)
            .u64(self.lobby_id)
            .f32(self.sandstorm_severity)
            .u8(self.extra_spawn_points.len() as u8);
        for (x, y) in &self.extra_spawn_points {
            w.i16(*x).i16(*y);
        }
        // Release 326's addition, after the spawn-point list rather than before it. The order was
        // settled by capture: read the other way round, a world whose dungeon sits at 3413 comes
        // out at 21760.
        w.i16(self.dungeon_x).i16(self.dungeon_y);
    }
}

impl Default for WorldData {
    /// A plain, freshly generated small world at dawn.
    fn default() -> Self {
        Self {
            time: 13500,
            day_time: true,
            blood_moon: false,
            eclipse: false,
            moon_phase: 0,
            max_tiles_x: 4200,
            max_tiles_y: 1200,
            spawn_tile_x: 2100,
            spawn_tile_y: 300,
            world_surface: 350,
            rock_layer: 500,
            world_id: 1,
            world_name: "Terrustia".into(),
            game_mode: 0,
            unique_id: [0; 16],
            world_gen_version: 0,
            moon_type: 0,
            backgrounds: [0; 13],
            ice_back_style: 0,
            jungle_back_style: 0,
            hell_back_style: 0,
            wind_speed_target: 0.0,
            num_clouds: 0,
            tree_x: [4200, 4200, 4200],
            tree_style: [0; 4],
            cave_back_x: [4200, 4200, 4200],
            cave_back_style: [0; 4],
            tree_tops: [0; 13],
            max_raining: 0.0,
            flags: WorldFlags::default(),
            sundial_cooldown: 0,
            moondial_cooldown: 0,
            // Copper, iron, silver, gold, then the three hardmode tiers a fresh world has not
            // rolled yet. The hardmode ids are cobalt 107, mythril 108, adamantite 111 — this
            // list used to read 108, 111, 112, which is each one shifted into the next tier's
            // slot with a non-ore on the end.
            ore_tiers: [7, 6, 9, 8, -1, -1, -1],
            invasion_type: 0,
            lobby_id: 0,
            sandstorm_severity: 0.0,
            extra_spawn_points: Vec::new(),
            dungeon_x: 0,
            dungeon_y: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(frame: &[u8]) -> &[u8] {
        &frame[3..]
    }

    #[test]
    fn hello_recognises_only_this_release() {
        let mut w = Writer::new();
        w.string("Terraria325");
        let hello = Hello::decode(w.as_slice()).unwrap();
        assert!(hello.is_supported());

        let mut w = Writer::new();
        w.string("Terraria279"); // 1.4.4.9
        assert!(!Hello::decode(w.as_slice()).unwrap().is_supported());
    }

    #[test]
    fn player_info_carries_the_slot_and_the_new_flag() {
        // 1.4.5 added the trailing bool; a 1.4.4-shaped 4-byte frame would desync the client.
        let frame = player_info(3, false).unwrap();
        assert_eq!(frame, vec![5, 0, id::PLAYER_INFO, 3, 0]);
        assert_eq!(payload(&frame).len(), 2);
    }

    #[test]
    fn heal_player_round_trips() {
        let heal = HealPlayer {
            player: 3,
            amount: 40,
        };
        let frame = heal.encode().unwrap();
        assert_eq!(frame[2], id::UNKNOWN66);
        assert_eq!(HealPlayer::decode(payload(&frame)).unwrap(), heal);
    }

    #[test]
    fn heal_player_rejects_a_truncated_payload() {
        assert!(HealPlayer::decode(&[3]).is_err());
    }

    #[test]
    fn empty_signals_are_bare_headers() {
        for msg in [id::INITIAL_SPAWN, id::FINISHED_CONNECTING_TO_SERVER] {
            let frame = empty(msg).unwrap();
            assert_eq!(frame, vec![3, 0, msg]);
        }
    }

    #[test]
    fn spawn_tile_data_decodes_position_and_team() {
        let mut w = Writer::new();
        w.i32(2100).i32(300).u8(2);
        assert_eq!(
            SpawnTileData::decode(w.as_slice()).unwrap(),
            SpawnTileData {
                x: 2100,
                y: 300,
                team: 2
            }
        );
    }

    #[test]
    fn spawn_tile_data_rejects_a_1_4_4_shaped_payload() {
        // Without the team byte the packet is one byte short; better to error than to read past.
        let mut w = Writer::new();
        w.i32(2100).i32(300);
        assert!(SpawnTileData::decode(w.as_slice()).is_err());
    }

    #[test]
    fn player_spawn_round_trips() {
        let spawn = PlayerSpawn {
            player: 2,
            spawn_x: 100,
            spawn_y: 200,
            respawn_timer: 0,
            deaths_pve: 1,
            deaths_pvp: 2,
            team: 0,
            context: PlayerSpawn::CONTEXT_SPAWNING_INTO_WORLD,
        };
        let frame = spawn.encode().unwrap();
        assert_eq!(PlayerSpawn::decode(payload(&frame)).unwrap(), spawn);
        // 1 + 2 + 2 + 4 + 2 + 2 + 1 + 1
        assert_eq!(payload(&frame).len(), 15);
    }

    #[test]
    fn health_and_mana_round_trip() {
        let health = PlayerHealth {
            player: 1,
            life: 340,
            life_max: 400,
        };
        assert_eq!(
            PlayerHealth::decode(payload(&health.encode().unwrap())).unwrap(),
            health
        );

        let mana = PlayerMana {
            player: 1,
            mana: 20,
            mana_max: 40,
        };
        assert_eq!(
            PlayerMana::decode(payload(&mana.encode().unwrap())).unwrap(),
            mana
        );
    }

    #[test]
    fn time_set_is_nine_bytes() {
        let frame = TimeSet {
            day_time: true,
            time: 13500,
            sun_mod_y: 0,
            moon_mod_y: 0,
        }
        .encode()
        .unwrap();
        // 1 + 4 + 2 + 2
        assert_eq!(payload(&frame).len(), 9);
        assert_eq!(frame[3], 1);
    }

    #[test]
    fn player_controls_omits_velocity_when_the_flag_is_clear() {
        let mut w = Writer::new();
        w.u8(0).u8(0x40).u8(0).u8(0).u8(0).u8(0).vec2(100.0, 200.0);
        let controls = PlayerControls::decode(w.as_slice()).unwrap();
        assert_eq!(controls.position, (100.0, 200.0));
        assert_eq!(controls.velocity, None);
        assert!(controls.facing_right());
    }

    #[test]
    fn player_controls_reads_velocity_when_the_flag_is_set() {
        let mut w = Writer::new();
        w.u8(0)
            .u8(0)
            .u8(0x04)
            .u8(0)
            .u8(0)
            .u8(0)
            .vec2(100.0, 200.0)
            .vec2(-1.5, 3.0);
        let controls = PlayerControls::decode(w.as_slice()).unwrap();
        assert_eq!(controls.velocity, Some((-1.5, 3.0)));
        assert!(!controls.facing_right());
    }

    #[test]
    fn relayed_packets_are_attributed_to_the_sender() {
        // The client claims slot 7; the server must stamp its real slot instead.
        let claimed = [7u8, 0xAA, 0xBB];
        let frame = rewrite_owner(id::PLAYER_CONTROLS, &claimed, 2).unwrap();
        assert_eq!(payload(&frame), &[2, 0xAA, 0xBB]);
        assert_eq!(frame[2], id::PLAYER_CONTROLS);
    }

    #[test]
    fn relaying_an_empty_payload_is_an_error() {
        assert!(rewrite_owner(id::PLAYER_CONTROLS, &[], 0).is_err());
    }

    /// Packet `13` at its real, fully-flagged size: every optional trailing block present at once,
    /// built field-by-field from `MessageBuffer.cs`'s own read order for `case 13`
    /// (`MessageBuffer.cs:952-1051`), not the two-field minimal frame the tests above use.
    ///
    /// This is the single highest-traffic packet in the protocol — every connected client sends one
    /// on every network tick it moves or acts — and until now nothing pinned its byte layout at
    /// this size: the two tests above only ever exercise the velocity flag, and
    /// `relayed_packets_are_attributed_to_the_sender` relays a synthetic three-byte payload that
    /// happens to have no optional blocks to lose.
    ///
    /// Field order and sizes, cited to source:
    /// `player` (u8) — `MessageBuffer.cs:954`
    /// `bitsByte24..27` (four u8 flag bytes) — `MessageBuffer.cs:964-967`
    /// `selectedItemState` (u8) — `MessageBuffer.cs:991`
    /// `position` (Vector2, 8 bytes) — `MessageBuffer.cs:992`
    /// `velocity`, gated on `bitsByte25[2]` (Vector2, 8 bytes) — `MessageBuffer.cs:994-997`
    /// `mount`, gated on `bitsByte25[7]` (u16) — `MessageBuffer.cs:1021-1023`
    /// `PotionOfReturn` pair, gated on `bitsByte26[6]` (two Vector2, 16 bytes) —
    ///   `MessageBuffer.cs:1029-1033`
    /// `netCameraTarget`, gated on `bitsByte27[5]` (Vector2, 8 bytes) — `MessageBuffer.cs:1051`
    #[test]
    fn player_controls_round_trips_every_optional_block_at_once() {
        let mut w = Writer::new();
        w.u8(7) // player slot
            .u8(0x40) // bitsByte24: bit 6 -> facing right
            .u8(0x84) // bitsByte25: bit 2 (velocity) | bit 7 (mount)
            .u8(0x44) // bitsByte26: bit 2 (sitting) | bit 6 (potion of return)
            .u8(0x20) // bitsByte27: bit 5 (camera target)
            .u8(3) // selected item slot
            .vec2(123.5, 456.25) // position
            .vec2(-1.5, 2.0) // velocity
            .u16(5) // mount type
            .vec2(10.0, 20.0) // PotionOfReturnOriginalUsePosition
            .vec2(30.0, 40.0) // PotionOfReturnHomePosition
            .vec2(500.0, 600.0); // netCameraTarget
        let raw = w.as_slice();
        assert_eq!(raw.len(), 48, "1 + 4 + 1 + 8 + 8 + 2 + 8 + 8 + 8");

        // Our decoder only needs the fields the server actually uses, and must not choke on — or
        // silently misread — the trailing mount/potion/camera bytes it does not parse.
        let controls = PlayerControls::decode(raw).unwrap();
        assert_eq!(controls.player, 7);
        assert_eq!(controls.control_flags, [0x40, 0x84, 0x44, 0x20]);
        assert_eq!(controls.selected_item, 3);
        assert_eq!(controls.position, (123.5, 456.25));
        assert_eq!(controls.velocity, Some((-1.5, 2.0)));
        assert!(controls.facing_right());
        assert!(controls.sitting());

        // The relay path must reproduce every byte of the optional blocks it cannot itself parse,
        // not only the fields our own decoder happens to read.
        let verbatim_frame = verbatim(id::PLAYER_CONTROLS, raw).unwrap();
        assert_eq!(
            payload(&verbatim_frame),
            raw,
            "verbatim must not touch a byte"
        );
        assert_eq!(
            PlayerControls::decode(payload(&verbatim_frame)).unwrap(),
            controls
        );

        let owned_frame = rewrite_owner(id::PLAYER_CONTROLS, raw, 2).unwrap();
        let owned = payload(&owned_frame);
        assert_eq!(owned[0], 2, "the owner byte is rewritten");
        assert_eq!(
            &owned[1..],
            &raw[1..],
            "every byte after the owner, including the mount/potion/camera blocks, is untouched"
        );
    }

    #[test]
    fn world_data_payload_is_exactly_the_documented_size() {
        // This is the packet that silently hangs a client when a field drifts, so its size is
        // pinned against the byte count derived from the client's reader.
        let world = WorldData {
            world_name: String::new(),
            ..Default::default()
        };
        let frame = world.encode().unwrap();
        // An empty name still costs its one-byte length prefix.
        assert_eq!(payload(&frame).len(), WORLD_DATA_FIXED_LEN + 1);

        let named = WorldData::default();
        assert_eq!(
            payload(&named.encode().unwrap()).len(),
            WORLD_DATA_FIXED_LEN + 1 + named.world_name.len()
        );
    }

    #[test]
    fn world_data_leads_with_time_and_day_flags() {
        let world = WorldData {
            blood_moon: true,
            eclipse: true,
            ..Default::default()
        };
        let frame = world.encode().unwrap();
        let p = payload(&frame);

        assert_eq!(i32::from_le_bytes([p[0], p[1], p[2], p[3]]), 13500);
        // bit0 dayTime, bit1 bloodMoon, bit2 eclipse
        assert_eq!(p[4], 0b0000_0111);
        assert_eq!(p[5], 0); // moon phase
        assert_eq!(i16::from_le_bytes([p[6], p[7]]), 4200);
        assert_eq!(i16::from_le_bytes([p[8], p[9]]), 1200);
    }

    #[test]
    fn world_data_survives_a_round_trip_through_every_field() {
        // The decoder exists to be pointed at a real server's packet 7, so it has to be the exact
        // mirror of the writer — not merely close enough that a default world survives. Every
        // field is given a value distinguishable from its neighbours, so a swapped pair fails.
        let world = WorldData {
            time: 27_000,
            day_time: false,
            blood_moon: true,
            eclipse: false,
            moon_phase: 3,
            max_tiles_x: 8400,
            max_tiles_y: 2400,
            spawn_tile_x: 4201,
            spawn_tile_y: 341,
            world_surface: 800,
            rock_layer: 1200,
            world_id: 1_234_567,
            world_name: "Round Trip".into(),
            game_mode: 2,
            unique_id: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            world_gen_version: 0x0102_0304_0506_0708,
            moon_type: 5,
            backgrounds: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13],
            ice_back_style: 2,
            jungle_back_style: 1,
            hell_back_style: 3,
            wind_speed_target: -0.375,
            num_clouds: 42,
            tree_x: [100, 200, 300],
            tree_style: [1, 2, 3, 4],
            cave_back_x: [400, 500, 600],
            cave_back_style: [5, 6, 7, 8],
            tree_tops: [9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 1, 2, 3],
            max_raining: 0.625,
            flags: WorldFlags([0xAA, 0x55, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
            sundial_cooldown: 4,
            moondial_cooldown: 6,
            ore_tiers: [7, 6, 9, 8, 107, 108, 111],
            invasion_type: -1,
            lobby_id: 0x1122_3344_5566_7788,
            sandstorm_severity: 0.5,
            extra_spawn_points: vec![(11, 22), (33, 44)],
            dungeon_x: 3413,
            dungeon_y: 190,
        };

        let frame = world.encode().unwrap();
        let back = WorldData::decode(payload(&frame)).unwrap();
        assert_eq!(back, world);
        // And the other way round, which is the direction that matters for a capture: bytes in,
        // identical bytes out.
        assert_eq!(back.encode().unwrap(), frame);
    }

    /// Packet 7 as a real 1.4.5.8 `TerrariaServer` sent it, byte for byte.
    ///
    /// Captured from the game's own dedicated server serving a world it generated itself, so
    /// nothing about these bytes came from this project. It is the only test here that can catch a
    /// field this codebase has never heard of — which is exactly how the two dungeon shorts at the
    /// end were found, as four bytes left over that no field accounted for.
    ///
    /// The world was `ProbeTiny`, 4200x1200, seed 12345, and its `.wld` file independently reports
    /// the dungeon at (3413, 190) — which is what turned the leftovers from a guess into a fact.
    const REAL_SERVER_PACKET_7: &[u8] = &[
        0xbc, 0x34, 0x00, 0x00, 0x01, 0x00, 0x68, 0x10, 0xb0, 0x04, 0x2f, 0x08, 0xe8, 0x00, 0x4b,
        0x01, 0xab, 0x01, 0x32, 0x83, 0x8a, 0x71, 0x09, 0x50, 0x72, 0x6f, 0x62, 0x65, 0x54, 0x69,
        0x6e, 0x79, 0x00, 0x15, 0xca, 0x11, 0x82, 0x13, 0x41, 0x54, 0x44, 0xbf, 0xd3, 0x40, 0x72,
        0xe7, 0xb6, 0x66, 0x50, 0x01, 0x00, 0x00, 0x00, 0x46, 0x01, 0x00, 0x00, 0x02, 0x33, 0x09,
        0x02, 0x06, 0x01, 0x05, 0x00, 0x02, 0x03, 0x02, 0x05, 0x02, 0x00, 0x02, 0x01, 0x02, 0x3b,
        0x31, 0x37, 0x3e, 0x9d, 0x2a, 0x0b, 0x00, 0x00, 0x68, 0x10, 0x00, 0x00, 0x68, 0x10, 0x00,
        0x00, 0x02, 0x04, 0x00, 0x00, 0x0b, 0x07, 0x00, 0x00, 0x68, 0x10, 0x00, 0x00, 0x68, 0x10,
        0x00, 0x00, 0x05, 0x04, 0x00, 0x00, 0x02, 0x04, 0x00, 0x00, 0x01, 0x05, 0x00, 0x02, 0x03,
        0x02, 0x05, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa6, 0x00, 0xa7, 0x00, 0xa8, 0x00, 0x08, 0x00, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x55, 0x0d, 0xbe, 0x00,
    ];

    #[test]
    fn a_real_servers_packet_7_decodes_to_the_world_it_describes() {
        let world = WorldData::decode(REAL_SERVER_PACKET_7).expect("real bytes should decode");

        assert_eq!(world.world_name, "ProbeTiny");
        assert_eq!((world.max_tiles_x, world.max_tiles_y), (4200, 1200));
        assert_eq!((world.spawn_tile_x, world.spawn_tile_y), (2095, 232));
        assert_eq!(world.world_surface, 331);
        // The high half of this is the release the world was generated by: 0x146 is 326.
        assert_eq!(world.world_gen_version >> 32, 326);
        // Tin, lead, tungsten, gold — an alternate-ore world — and the three hardmode tiers still
        // unchosen. Terraria writes -1 for those, not nought, which is the sentinel `SmashAltar`
        // checks before it rolls one.
        assert_eq!(world.ore_tiers, [166, 167, 168, 8, -1, -1, -1]);
        // The four bytes that started all this. The world file agrees.
        assert_eq!((world.dungeon_x, world.dungeon_y), (3413, 190));
    }

    #[test]
    fn a_real_servers_packet_7_re_encodes_byte_for_byte() {
        // The check that actually proves the layout. Decoding leniently would survive a missing
        // trailing field or a pair of swapped shorts; producing the identical bytes back does not.
        let world = WorldData::decode(REAL_SERVER_PACKET_7).unwrap();
        let ours = world.encode().unwrap();
        assert_eq!(
            payload(&ours),
            REAL_SERVER_PACKET_7,
            "our packet 7 no longer matches the one a real server sends"
        );
    }

    /// Packets 57, 60 and 139 exactly as a real 1.4.5.8 server sent them.
    ///
    /// All three were found by connecting to the game's own dedicated server and noticing it sent
    /// things this server never did. The bytes below are that session's, so these are checks
    /// against Re-Logic's encoder rather than against our reading of it.
    #[test]
    fn the_small_join_packets_match_a_real_servers() {
        // Packet 57: nought per cent hallow, nought corrupt, four crimson — a fresh crimson world.
        // Three bytes, not three ints: the game counts tiles in ints and rounds before sending.
        assert_eq!(
            payload(&world_evil_tally(0, 0, 4).unwrap()),
            &[0x00, 0x00, 0x04]
        );

        // Packet 60, the Old Man: NPC 0, living at the dungeon door on (3413, 190), settled.
        assert_eq!(
            payload(&npc_home(0, 3413, 190, HouseholdStatus::Settled).unwrap()),
            &[0x00, 0x00, 0x55, 0x0d, 0xbe, 0x00, 0x00]
        );
        // And the Guide, freshly arrived at the spawn with nowhere to live.
        assert_eq!(
            payload(&npc_home(1, 2095, 232, HouseholdStatus::Homeless).unwrap()),
            &[0x01, 0x00, 0x2f, 0x08, 0xe8, 0x00, 0x01]
        );

        // Packet 139: player 0 is on the same machine as the server.
        assert_eq!(payload(&counts_as_host(0, true).unwrap()), &[0x00, 0x01]);
    }

    #[test]
    fn world_data_decode_rejects_a_payload_with_more_in_it() {
        // A layout one field short still parses if leftovers are ignored, and that is precisely
        // the bug this decoder is meant to be able to find.
        let mut frame = WorldData::default().encode().unwrap();
        frame.push(0);
        assert!(matches!(
            WorldData::decode(&frame[3..]),
            Err(ProtoError::TrailingBytes { left: 1 })
        ));
    }

    #[test]
    fn world_data_decode_rejects_a_truncated_payload() {
        let frame = WorldData::default().encode().unwrap();
        let p = payload(&frame);
        assert!(WorldData::decode(&p[..p.len() - 1]).is_err());
    }

    #[test]
    fn world_data_ends_with_the_spawn_point_list_and_then_the_dungeon() {
        // The spawn-point list was added in 1.4.5 and the dungeon pair in release 326; both are
        // easy to forget, and either one missing leaves the client reading past the payload.
        // The order is the part worth pinning: read the other way round, a dungeon at 3413 comes
        // back as 21760, which is how the two were told apart in the first place.
        let world = WorldData {
            extra_spawn_points: vec![(10, 20), (30, 40)],
            dungeon_x: 3413,
            dungeon_y: 190,
            ..Default::default()
        };
        let frame = world.encode().unwrap();
        let p = payload(&frame);
        let tail = &p[p.len() - 13..];
        assert_eq!(tail[0], 2, "two extra spawn points");
        assert_eq!(i16::from_le_bytes([tail[1], tail[2]]), 10);
        assert_eq!(i16::from_le_bytes([tail[7], tail[8]]), 40);
        assert_eq!(i16::from_le_bytes([tail[9], tail[10]]), 3413);
        assert_eq!(i16::from_le_bytes([tail[11], tail[12]]), 190);
    }

    #[test]
    fn world_flag_bits_land_where_the_client_reads_them() {
        let mut flags = WorldFlags::default();
        flags.set_crimson(true);
        assert_eq!(flags.0[1], 0b0010_0000);

        flags.set_hardmode(true);
        flags.set_server_side_character(true);
        assert_eq!(flags.0[0], 0b0101_0000);

        flags.set_server_side_character(false);
        assert_eq!(flags.0[0], 0b0001_0000);
    }

    #[test]
    fn kick_carries_a_readable_reason() {
        let frame = kick(&NetworkText::literal("nope")).unwrap();
        assert_eq!(frame[2], id::KICK);
        let mut r = PacketReader::new(payload(&frame));
        assert_eq!(NetworkText::read(&mut r).unwrap().text, "nope");
    }
}

/// Packet `17`: a single-tile edit.
///
/// The third field is overloaded: for the destroy actions it is a "fail" flag where 1 means the
/// tile was only damaged rather than removed, for placements it is the tile or wall type, and for
/// [`TileAction::Slope`] it is the slope index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileManipulation {
    pub action: u8,
    pub x: i16,
    pub y: i16,
    pub arg: i16,
    pub style: u8,
}

/// The tile actions this server understands, from `MessageBuffer`'s case 17 dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileAction {
    KillTile,
    PlaceTile,
    KillWall,
    PlaceWall,
    /// Destroy without dropping an item.
    KillTileNoItem,
    PlaceWire,
    KillWire,
    /// Hammer a block into a half brick.
    PoundTile,
    PlaceActuator,
    KillActuator,
    PlaceWire2,
    KillWire2,
    PlaceWire3,
    KillWire3,
    SlopeTile,
    PlaceWire4,
    KillWire4,
    /// Anything the vertical slice does not model.
    Other(u8),
}

impl TileManipulation {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        Ok(Self {
            action: r.u8()?,
            x: r.i16()?,
            y: r.i16()?,
            arg: r.i16()?,
            style: r.u8()?,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::TILE_MANIPULATION);
        w.u8(self.action)
            .i16(self.x)
            .i16(self.y)
            .i16(self.arg)
            .u8(self.style);
        w.finish()
    }

    pub fn kind(&self) -> TileAction {
        match self.action {
            0 => TileAction::KillTile,
            1 => TileAction::PlaceTile,
            2 => TileAction::KillWall,
            3 => TileAction::PlaceWall,
            4 => TileAction::KillTileNoItem,
            5 => TileAction::PlaceWire,
            6 => TileAction::KillWire,
            7 => TileAction::PoundTile,
            8 => TileAction::PlaceActuator,
            9 => TileAction::KillActuator,
            10 => TileAction::PlaceWire2,
            11 => TileAction::KillWire2,
            12 => TileAction::PlaceWire3,
            13 => TileAction::KillWire3,
            14 => TileAction::SlopeTile,
            16 => TileAction::PlaceWire4,
            17 => TileAction::KillWire4,
            other => TileAction::Other(other),
        }
    }

    /// For the destroy actions, whether the tile actually went away.
    ///
    /// A pickaxe swing that only damages a block sends `arg == 1`; treating that as a break would
    /// delete blocks on the first hit.
    pub fn destroyed(&self) -> bool {
        self.arg != 1
    }
}

#[cfg(test)]
mod tile_manipulation_tests {
    use super::*;

    #[test]
    fn round_trips_and_is_eight_bytes() {
        let edit = TileManipulation {
            action: 1,
            x: 2100,
            y: 300,
            arg: 3,
            style: 0,
        };
        let frame = edit.encode().unwrap();
        // 1 + 2 + 2 + 2 + 1
        assert_eq!(frame.len() - 3, 8);
        assert_eq!(TileManipulation::decode(&frame[3..]).unwrap(), edit);
    }

    #[test]
    fn actions_map_to_their_meanings() {
        let at = |action| {
            TileManipulation {
                action,
                x: 0,
                y: 0,
                arg: 0,
                style: 0,
            }
            .kind()
        };
        assert_eq!(at(0), TileAction::KillTile);
        assert_eq!(at(1), TileAction::PlaceTile);
        assert_eq!(at(2), TileAction::KillWall);
        assert_eq!(at(3), TileAction::PlaceWall);
        assert_eq!(at(4), TileAction::KillTileNoItem);
        assert_eq!(at(7), TileAction::PoundTile);
        assert_eq!(at(14), TileAction::SlopeTile);
        assert_eq!(at(15), TileAction::Other(15));
        assert_eq!(at(200), TileAction::Other(200));
    }

    #[test]
    fn a_damaged_tile_is_not_a_destroyed_one() {
        let mut edit = TileManipulation {
            action: 0,
            x: 1,
            y: 1,
            arg: 1,
            style: 0,
        };
        assert!(
            !edit.destroyed(),
            "arg == 1 means the block survived the hit"
        );
        edit.arg = 0;
        assert!(edit.destroyed());
    }

    #[test]
    fn negative_coordinates_survive_the_round_trip() {
        // The field is signed; a client near the edge can send an out-of-range negative.
        let edit = TileManipulation {
            action: 0,
            x: -5,
            y: -1,
            arg: 0,
            style: 0,
        };
        assert_eq!(
            TileManipulation::decode(&edit.encode().unwrap()[3..]).unwrap(),
            edit
        );
    }
}

/// Packet `55`: put a buff on a player.
///
/// The name in the protocol is `AddPlayerBuffPvP`, and clients only accept it from another player
/// in a PvP fight — but a server sending it is authoritative, which is how a wither beast's aura
/// or an enemy's touch lands a debuff on you at all.
pub fn add_player_buff(player: u8, buff: u16, ticks: i32) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(id::ADD_PLAYER_BUFF_PV_P);
    w.u8(player).u16(buff).i32(ticks);
    w.finish()
}

/// Packet `53`: a client asking that a buff be put on an NPC.
///
/// This is how nearly every weapon debuff reaches the server. The client that landed the hit
/// works out what it inflicts and says so; the server decides whether the target is immune.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddNpcBuff {
    pub index: u8,
    pub buff: u16,
    pub ticks: i16,
}

impl AddNpcBuff {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        // The index is a signed short on the wire but only ever names one of two hundred slots,
        // so anything outside that is a malformed packet rather than a slot to be found.
        let index = r.i16()?;
        Ok(Self {
            index: u8::try_from(index).map_err(|_| ProtoError::OutOfRange {
                field: "npc buff index",
                value: i64::from(index),
            })?,
            buff: r.u16()?,
            ticks: r.i16()?,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::ADD_N_P_C_BUFF);
        w.i16(i16::from(self.index)).u16(self.buff).i16(self.ticks);
        w.finish()
    }
}

/// Packet `54`: the whole buff list of one NPC.
///
/// Sent whenever the list changes, and it has to be: a client works out its own armour
/// penetration from what it believes is on the target, so an ichor-covered enemy the client has
/// not been told about takes fifteen points less damage per hit than it should.
///
/// The list is terminated by a zero rather than counted, and holes are skipped rather than sent,
/// which is why removal compacts the slots.
pub fn npc_buffs(index: u8, slots: impl IntoIterator<Item = (u16, i32)>) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(id::N_P_C_BUFFS);
    w.i16(i16::from(index));
    for (buff, ticks) in slots {
        if buff == 0 || ticks <= 0 {
            continue;
        }
        // The wire carries the remaining time as an unsigned short. A buff longer than about
        // eighteen minutes saturates rather than wrapping round to nearly-expired.
        w.u16(buff).u16(u16::try_from(ticks).unwrap_or(u16::MAX));
    }
    w.u16(0);
    w.finish()
}

/// Packet `137`: a client asking that a buff be taken *off* an NPC.
///
/// The server refuses every one of these in this version — the permitted set is empty — but the
/// packet still has to be read, or the bytes after it in the same batch are misparsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoveNpcBuff {
    pub index: u8,
    pub buff: u16,
}

impl RemoveNpcBuff {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        let index = r.i16()?;
        Ok(Self {
            index: u8::try_from(index).map_err(|_| ProtoError::OutOfRange {
                field: "npc buff index",
                value: i64::from(index),
            })?,
            buff: r.u16()?,
        })
    }
}

/// Packet `153`: damage an NPC took from a debuff rather than from a hit.
///
/// It is its own message because it is nobody's hit: no player is credited, no knockback is
/// applied, and the client shows it in the colour it uses for poison rather than for a strike.
pub fn npc_debuff_damage(index: u8, amount: i16) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(id::N_P_C_DEBUFF_DAMAGE);
    w.u8(index).i16(amount);
    w.finish()
}

/// Packet `56`: a town NPC's given name and which of its looks it wears.
///
/// A client asks for this by sending the same message with only the slot filled in, and until it
/// is answered the NPC has no name at all — every guide in the world is "Guide" and none of them
/// is Andrew.
pub fn town_npc_name(index: u8, name: &str, variation: i32) -> Result<Vec<u8>> {
    let mut w = PacketWriter::new(id::UNIQUE_TOWN_N_P_C_INFO_SYNC_REQUEST);
    w.i16(i16::from(index)).string(name).i32(variation);
    w.finish()
}
