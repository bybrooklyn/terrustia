#![forbid(unsafe_code)]
//! A headless Terraria client.
//!
//! It speaks the same protocol a real client does — handshake, world streaming, movement, chat,
//! tile edits — without any rendering. That makes it useful for three things: driving integration
//! tests against a server, probing a real `TerrariaServer` to compare behaviour, and scripting a
//! bot.
//!
//! Copyright (C) 2026 Brooklyn Halmstad.
//! Licensed under the GNU Affero General Public License v3.0 or later; see LICENSE.

pub mod codec;
pub mod error;
pub mod tap;
pub mod world;

use std::{net::SocketAddr, time::Duration};

use bytes::BytesMut;
use terrustia_proto::{
    ItemStack, NetworkText, PacketWriter, id,
    items::{ItemOwner, NEW_ITEM_INDEX, SyncItem, decode_item_despawn},
    net_module::{self, MODULE_TEXT},
    npc::{DamageNpc, SyncNpc},
    objects::DoorToggle,
    packets::{self, PlayerSpawn, TileManipulation},
    reader::PacketReader,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use tracing::debug;

pub use codec::Frame;
pub use error::{ClientError, Result};
pub use tap::Tap;
pub use world::ClientWorld;

/// How long to wait for any single expected packet.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Something the server told us.
#[derive(Debug, Clone)]
pub enum Event {
    /// A chat line, from a player or from the server itself.
    Chat { author: u8, text: String },
    /// A tile section arrived and has been folded into the world view.
    SectionLoaded { section_x: i32, section_y: i32 },
    /// Another player appeared or vanished.
    PlayerActive { slot: u8, active: bool },
    /// Another player moved.
    PlayerMoved { slot: u8, x: f32, y: f32 },
    /// A single-tile edit.
    TileChanged(TileManipulation),
    /// An NPC appeared, moved, or died (life 0).
    NpcSynced(SyncNpc),
    /// An item entity appeared or moved.
    ItemSynced(SyncItem),
    /// An item was reserved for a player, who may now pick it up.
    ItemReserved(ItemOwner),
    /// A player is carrying something in a slot.
    EquipmentSynced(terrustia_proto::inventory::SyncEquipment),
    /// An item is gone.
    ItemDespawned(i16),
    /// Something is in flight.
    ProjectileSynced(terrustia_proto::projectile::SyncProjectile),
    /// ...and is not any more.
    ProjectileKilled(terrustia_proto::projectile::KillProjectile),
    /// A player took a hit.
    PlayerHurt(terrustia_proto::hurt::PlayerHurt),
    /// A player died.
    PlayerDied(terrustia_proto::hurt::PlayerDeath),
    /// Liquid moved. Already folded into the world view.
    LiquidChanged(Vec<terrustia_proto::net_module::LiquidChange>),
    /// The handshake finished.
    FinishedConnecting,
    /// Anything not otherwise interpreted.
    Other(Frame),
}

/// A connected client.
pub struct Client {
    stream: TcpStream,
    buf: BytesMut,
    slot: u8,
    name: String,
    world: ClientWorld,
    position: (f32, f32),
    timeout: Duration,
    /// Where to record the raw stream, when someone has asked for a capture.
    tap: Option<Tap>,
    /// Events interpreted during the closing stretch of the handshake, kept for the caller.
    ///
    /// Everything between `12 PlayerSpawn` going out and `129 FinishedConnectingToServer` coming
    /// back is join state a caller may well be waiting on: the angler quest, the cavern monster
    /// types, the travelling merchant's stock, the journey powers. Vanilla sends all of it *before*
    /// 129 (`MessageBuffer.cs:937`), so the handshake loop is what reads it, and dropping it on the
    /// floor made those packets unobservable to anyone using this crate.
    joined_with: std::collections::VecDeque<Event>,
}

impl Client {
    /// Connect and run the full handshake, returning once the world is ready to walk around in.
    pub async fn join(addr: SocketAddr, name: &str) -> Result<Self> {
        let mut client = Self::connect(addr, name).await?;
        client.handshake().await?;
        Ok(client)
    }

    /// Connect without starting the handshake.
    pub async fn connect(addr: SocketAddr, name: &str) -> Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            buf: BytesMut::with_capacity(64 * 1024),
            slot: 0,
            name: name.to_string(),
            world: ClientWorld::default(),
            position: (0.0, 0.0),
            timeout: DEFAULT_TIMEOUT,
            tap: None,
            joined_with: std::collections::VecDeque::new(),
        })
    }

    /// Record every byte of this connection to a file, in both directions.
    ///
    /// Set it before the handshake to capture the handshake, which is the part worth having.
    pub fn record_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.tap = Some(Tap::create(path)?);
        Ok(())
    }

    /// Flush the capture, if one is open.
    pub fn flush_recording(&mut self) {
        if let Some(tap) = self.tap.as_mut() {
            tap.flush();
        }
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    pub fn slot(&self) -> u8 {
        self.slot
    }

    pub fn world(&self) -> &ClientWorld {
        &self.world
    }

    pub fn position(&self) -> (f32, f32) {
        self.position
    }

    // ------------------------------------------------------------------ handshake

    /// Drive the connection sequence a real client performs.
    pub async fn handshake(&mut self) -> Result<()> {
        self.send(&{
            let mut w = PacketWriter::new(id::HELLO);
            w.string(id::VERSION_STRING);
            w.finish()?
        })
        .await?;

        // The server answers with a slot, or refuses.
        let frame = self
            .expect_one_of(&[id::PLAYER_INFO, id::KICK], "a player slot")
            .await?;
        if frame.id == id::KICK {
            return Err(ClientError::Kicked {
                reason: kick_reason(&frame.payload),
            });
        }
        self.slot = *frame.payload.first().unwrap_or(&0);

        self.send(&self.appearance_packet()?).await?;
        self.send(
            &packets::PlayerHealth {
                player: self.slot,
                life: 100,
                life_max: 100,
            }
            .encode()?,
        )
        .await?;
        self.send(
            &packets::PlayerMana {
                player: self.slot,
                mana: 20,
                mana_max: 20,
            }
            .encode()?,
        )
        .await?;
        self.send(&{
            let mut w = PacketWriter::new(id::CLIENT_UUID);
            w.string("terrustia-client");
            w.finish()?
        })
        .await?;
        self.send(&packets::empty(id::REQUEST_WORLD_DATA)?).await?;

        // World data, then ask for the tiles around spawn.
        let frame = self.expect(id::WORLD_DATA, "world data").await?;
        self.absorb_world_data(&frame.payload)?;

        self.send(&{
            let mut w = PacketWriter::new(id::SPAWN_TILE_DATA);
            w.i32(-1).i32(-1).u8(0);
            w.finish()?
        })
        .await?;

        // Sections stream in until the server says it is done.
        loop {
            let frame = self.read_frame().await?;
            let done = frame.id == id::INITIAL_SPAWN;
            self.interpret(frame)?;
            if done {
                break;
            }
        }

        // Spawn in, then wait to be told the connection is complete.
        let spawn = PlayerSpawn {
            player: self.slot,
            spawn_x: -1,
            spawn_y: -1,
            respawn_timer: 0,
            deaths_pve: 0,
            deaths_pvp: 0,
            team: 0,
            context: PlayerSpawn::CONTEXT_SPAWNING_INTO_WORLD,
        };
        self.send(&spawn.encode()?).await?;

        loop {
            let frame = self.read_frame().await?;
            let done = frame.id == id::FINISHED_CONNECTING_TO_SERVER;
            let event = self.interpret(frame)?;
            self.joined_with.push_back(event);
            if done {
                break;
            }
        }

        self.position = (
            f32::from(self.world.spawn.0) * 16.0,
            f32::from(self.world.spawn.1) * 16.0,
        );
        Ok(())
    }

    /// A complete packet 4, in the field order the 1.4.5.7 client uses.
    fn appearance_packet(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::SYNC_PLAYER);
        w.u8(self.slot)
            .u8(0) // skin variant
            .u8(1) // voice variant
            .f32(0.0) // voice pitch offset
            .u8(0) // hair
            .string(&self.name)
            .u8(0) // hair dye
            .u16(0) // accessory visibility
            .u8(0) // hidden misc slots
            .rgb([215, 90, 55])
            .rgb([255, 125, 90])
            .rgb([105, 90, 75])
            .rgb([175, 165, 140])
            .rgb([160, 180, 215])
            .rgb([255, 230, 175])
            .rgb([160, 105, 60])
            .u8(0) // difficulty and extra accessory
            .u8(0) // torch flags
            .u8(0); // consumable flags
        Ok(w.finish()?)
    }

    fn absorb_world_data(&mut self, payload: &[u8]) -> Result<()> {
        let mut r = PacketReader::new(payload);
        r.i32()?; // time
        r.u8()?; // day and moon flags
        r.u8()?; // moon phase
        self.world.width = i32::from(r.i16()?);
        self.world.height = i32::from(r.i16()?);
        self.world.spawn = (r.i16()?, r.i16()?);
        r.i16()?; // world surface
        r.i16()?; // rock layer
        r.i32()?; // world id
        self.world.name = r.string()?;
        Ok(())
    }

    // ------------------------------------------------------------------ actions

    /// Report a new position, as a real client does every tick.
    pub async fn move_to(&mut self, x: f32, y: f32) -> Result<()> {
        self.position = (x, y);
        let mut w = PacketWriter::new(id::PLAYER_CONTROLS);
        w.u8(self.slot)
            .u8(0x40) // facing right
            .u8(0) // no velocity block follows
            .u8(0)
            .u8(0)
            .u8(0) // selected item
            .vec2(x, y);
        self.send(&w.finish()?).await
    }

    /// Sit down at a position with a given hotbar slot selected — the same packet 13 as
    /// [`Client::move_to`], but with the sitting bit set (`bitsByte26[2]`, this project's own
    /// `PlayerControls::sitting`) and a real selected-item byte, both of which `move_to` always
    /// sends zeroed.
    pub async fn sit_at(&mut self, x: f32, y: f32, selected_item: u8) -> Result<()> {
        self.position = (x, y);
        let mut w = PacketWriter::new(id::PLAYER_CONTROLS);
        w.u8(self.slot)
            .u8(0x40) // facing right
            .u8(0) // no velocity block follows
            .u8(0x04) // sitting
            .u8(0)
            .u8(selected_item)
            .vec2(x, y);
        self.send(&w.finish()?).await
    }

    /// Walk to a tile position, pulling in any sections needed along the way.
    ///
    /// This is what a real client does as it moves: the server no longer pushes sections, so a
    /// client that never asks simply sees empty sky.
    pub async fn walk_to_tile(&mut self, tile_x: i32, tile_y: i32) -> Result<()> {
        self.move_to(tile_x as f32 * 16.0, tile_y as f32 * 16.0)
            .await?;
        let (sx, sy) = ClientWorld::section_of(tile_x, tile_y);
        for dx in -1..=1 {
            for dy in -1..=1 {
                let (nx, ny) = (sx + dx, sy + dy);
                if nx >= 0 && ny >= 0 && !self.world.has_section(nx, ny) {
                    self.request_section(nx as u16, ny as u16).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn request_section(&mut self, section_x: u16, section_y: u16) -> Result<()> {
        let mut w = PacketWriter::new(id::REQUEST_SECTION);
        w.u16(section_x).u16(section_y);
        self.send(&w.finish()?).await
    }

    /// Come back to life at the world spawn.
    ///
    /// The server only knows you are back when you tell it, and until you do, every routine that
    /// looks for a living player will pass you over.
    pub async fn respawn(&mut self) -> Result<()> {
        let (x, y) = self.world().spawn;
        let frame = terrustia_proto::packets::PlayerSpawn {
            player: self.slot(),
            spawn_x: x,
            spawn_y: y,
            respawn_timer: 0,
            deaths_pve: 0,
            deaths_pvp: 0,
            team: 0,
            context: 0,
        }
        .encode()?;
        self.send(&frame).await
    }

    pub async fn say(&mut self, text: &str) -> Result<()> {
        let mut w = PacketWriter::new(id::NET_MODULES);
        w.u16(MODULE_TEXT).string("Say").string(text);
        self.send(&w.finish()?).await
    }

    pub async fn break_tile(&mut self, x: i16, y: i16) -> Result<()> {
        self.tile_action(0, x, y, 0, 0).await
    }

    pub async fn place_tile(&mut self, x: i16, y: i16, block: u16) -> Result<()> {
        self.tile_action(1, x, y, block as i16, 0).await
    }

    pub async fn break_wall(&mut self, x: i16, y: i16) -> Result<()> {
        self.tile_action(2, x, y, 0, 0).await
    }

    pub async fn place_wall(&mut self, x: i16, y: i16, wall: u16) -> Result<()> {
        self.tile_action(3, x, y, wall as i16, 0).await
    }

    async fn tile_action(&mut self, action: u8, x: i16, y: i16, arg: i16, style: u8) -> Result<()> {
        let edit = TileManipulation {
            action,
            x,
            y,
            arg,
            style,
        };
        self.send(&edit.encode()?).await
    }

    /// Report hitting an NPC.
    pub async fn hit_npc(
        &mut self,
        index: u8,
        generation: u8,
        damage: i16,
        knockback: f32,
        direction: i8,
    ) -> Result<()> {
        let hit = DamageNpc {
            index,
            generation,
            damage,
            knockback,
            direction,
            crit: false,
        };
        self.send(&hit.encode()?).await
    }

    /// Offer some of our slots to the nearby chests, as the quick stack button does.
    pub async fn quick_stack(&mut self, slots: &[u16], smart: bool) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(id::QUICK_STACK_CHESTS);
        w.i32(slots.len() as i32);
        for slot in slots {
            w.i16(*slot as i16);
        }
        w.bool(smart);
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Drag a wiring tool from one tile to another.
    pub async fn mass_wire(&mut self, from: (i16, i16), to: (i16, i16), mode: u8) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(id::MASS_WIRE_OPERATION);
        w.i16(from.0).i16(from.1).i16(to.0).i16(to.1).u8(mode);
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Ask what a chest is called, the way the map does.
    pub async fn ask_chest_name(&mut self, x: i16, y: i16) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(id::CHEST_NAME);
        w.i16(-1).i16(x).i16(y);
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Ask the server to move us: 0 potion, 1 magic conch, 2 demon conch, 3 shellphone,
    /// 4 the rescue that fires when there is nowhere to stand.
    pub async fn ask_teleport(&mut self, kind: u8) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(id::REQUEST_TELEPORTATION_BY_SERVER);
        w.u8(kind);
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Place a tile entity: an item frame, a mannequin, a pylon.
    pub async fn place_tile_entity(&mut self, x: i16, y: i16, kind: u8) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(id::TILE_ENTITY_PLACEMENT);
        w.i16(x).i16(y).u8(kind);
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Put an item into a frame, onto a rack or platter, or into a display jar.
    pub async fn display_item(
        &mut self,
        message: u8,
        x: i16,
        y: i16,
        item: ItemStack,
    ) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(message);
        w.i16(x)
            .i16(y)
            .i16(item.id as i16)
            .u8(item.prefix)
            .i16(item.stack);
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Claim a tile entity, or release whatever was claimed by passing -1.
    pub async fn claim_tile_entity(&mut self, id: i32) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(id::REQUEST_TILE_ENTITY_INTERACTION);
        w.i32(id).u8(self.slot());
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Report inflicting a buff on an NPC, the way a weapon's on-hit effect does.
    pub async fn buff_npc(&mut self, index: u8, buff: u16, ticks: i16) -> Result<()> {
        let request = terrustia_proto::packets::AddNpcBuff { index, buff, ticks };
        self.send(&request.encode()?).await
    }

    /// Ask the server to take a buff off an NPC.
    pub async fn unbuff_npc(&mut self, index: u8, buff: u16) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(id::REQUEST_N_P_C_BUFF_REMOVAL);
        w.i16(i16::from(index)).u16(buff);
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Ask what a town NPC is called, as a client does the moment one comes into view.
    pub async fn ask_npc_name(&mut self, index: u8) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(id::UNIQUE_TOWN_N_P_C_INFO_SYNC_REQUEST);
        w.i16(i16::from(index));
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Start talking to a town NPC, or stop by passing `None`.
    ///
    /// Talking is not only chatter: it is how somebody tied up is freed, which is the only way six
    /// of the game's residents ever arrive.
    pub async fn talk_to_npc(&mut self, index: impl Into<Option<u8>>) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(id::SYNC_TALK_N_P_C);
        w.u8(self.slot);
        w.i16(index.into().map_or(-1, i16::from));
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Report picking an item up. Only works for an item the server reserved for us.
    pub async fn pick_up(&mut self, index: i16) -> Result<()> {
        self.send(&terrustia_proto::items::item_despawn(index)?)
            .await
    }

    /// Throw an item into the world, asking the server for a slot.
    /// Teleport this player, the way a magic mirror does.
    pub async fn teleport(&mut self, x: f32, y: f32) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(id::TELEPORT_ENTITY);
        // No flags: a player, to a given place, with no extra.
        w.u8(0);
        w.i16(i16::from(self.slot()));
        w.f32(x);
        w.f32(y);
        w.u8(0);
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Place a multi-tile object — a chest, a door, a workbench — with its cursor tile at
    /// `(x, y)`.
    pub async fn place_object(&mut self, x: i16, y: i16, block: u16, style: i16) -> Result<()> {
        let frame = terrustia_proto::packets::place_object(
            i32::from(x),
            i32::from(y),
            block,
            i32::from(style),
            -1,
        )?;
        self.send(&frame).await
    }

    /// Unlock a locked dungeon door with a Golden Key, the way a real client does.
    ///
    /// The server walks up to the door's top row (`frameY == 594`) and shifts all three rows by
    /// +54, which is `WorldGen.UnlockDoor` (`WorldGen.cs:37988-38017`).
    pub async fn unlock_door(&mut self, x: i16, y: i16) -> Result<()> {
        let frame = terrustia_proto::packets::lock_and_unlock(2, i32::from(x), i32::from(y))?;
        self.send(&frame).await
    }

    /// Use a summoning item: a boss by type, or an event by one of the negative codes.
    pub async fn summon(&mut self, what: i16) -> Result<()> {
        let mut w = terrustia_proto::PacketWriter::new(id::SPAWN_BOSS_USE_LICENSE_START_EVENT);
        w.i16(i16::from(self.slot()));
        w.i16(what);
        let frame = w.finish()?;
        self.send(&frame).await
    }

    /// Tell the server what is in one of this player's inventory slots.
    pub async fn set_equipment(&mut self, slot: u16, item: ItemStack) -> Result<()> {
        let frame = terrustia_proto::inventory::SyncEquipment {
            player: self.slot(),
            slot,
            item,
            favorited: false,
            blocked: false,
        }
        .encode()?;
        self.send(&frame).await
    }

    pub async fn drop_item(&mut self, item: ItemStack, position: (f32, f32)) -> Result<()> {
        let sync = SyncItem::dropped(NEW_ITEM_INDEX, position, item);
        self.send(&sync.encode()?).await
    }

    pub async fn open_chest(&mut self, x: i16, y: i16) -> Result<()> {
        let mut w = PacketWriter::new(id::REQUEST_CHEST_OPEN);
        w.i16(x).i16(y);
        self.send(&w.finish()?).await
    }

    pub async fn read_sign(&mut self, x: i16, y: i16) -> Result<()> {
        let mut w = PacketWriter::new(id::OPEN_SIGN_REQUEST);
        w.i16(x).i16(y);
        self.send(&w.finish()?).await
    }

    pub async fn toggle_door(&mut self, action: u8, x: i16, y: i16, direction: u8) -> Result<()> {
        let door = DoorToggle {
            action,
            x,
            y,
            direction,
        };
        self.send(&door.encode()?).await
    }

    /// Hit a switch, lever or pressure plate, which runs whatever it is wired to.
    pub async fn hit_switch(&mut self, x: i16, y: i16) -> Result<()> {
        let mut w = terrustia_proto::Writer::new();
        w.i16(x).i16(y);
        self.send(&terrustia_proto::packets::verbatim(
            id::HIT_SWITCH,
            w.as_slice(),
        )?)
        .await
    }

    /// Send an already-encoded frame, for anything this API does not cover.
    pub async fn send(&mut self, frame: &[u8]) -> Result<()> {
        if let Some(tap) = self.tap.as_mut() {
            tap.chunk(crate::tap::Direction::ToServer, frame);
        }
        self.stream.write_all(frame).await?;
        Ok(())
    }

    // ------------------------------------------------------------------ receiving

    /// Wait for the next event, interpreting the packet and updating the world view.
    pub async fn next_event(&mut self) -> Result<Event> {
        // Whatever arrived ahead of 129 comes out first, in the order the server sent it.
        if let Some(event) = self.joined_with.pop_front() {
            return Ok(event);
        }
        let frame = self.read_frame().await?;
        self.interpret(frame)
    }

    /// Read events until one matches, or the timeout expires.
    pub async fn wait_for<F>(&mut self, what: &str, mut matches: F) -> Result<Event>
    where
        F: FnMut(&Event) -> bool,
    {
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(ClientError::Timeout {
                    expected: what.to_string(),
                    seconds: self.timeout.as_secs(),
                });
            }
            let event = self.next_event().await?;
            if matches(&event) {
                return Ok(event);
            }
        }
    }

    /// Tell the server how much life this client has.
    ///
    /// A fresh character has a hundred, and several things in the game are gated on two hundred —
    /// an invasion will not begin for a party who have never found a life crystal, and the wind
    /// will not blow hard. A test or a probe that wants to reach those has to say so.
    pub async fn set_life(&mut self, life: i16, life_max: i16) -> Result<()> {
        let packet = packets::PlayerHealth {
            player: self.slot,
            life,
            life_max,
        };
        self.send(&packet.encode()?).await
    }

    /// Read events until one matches, or a shorter deadline than the client's own expires.
    ///
    /// For asserting that something does *not* arrive, where waiting the full timeout would only
    /// make the test slow.
    pub async fn try_wait_for<F>(
        &mut self,
        _what: &str,
        mut matches: F,
        within: std::time::Duration,
    ) -> Option<Event>
    where
        F: FnMut(&Event) -> bool,
    {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            match tokio::time::timeout(left, self.next_event()).await {
                Ok(Ok(event)) if matches(&event) => return Some(event),
                Ok(Ok(_)) => continue,
                _ => return None,
            }
        }
    }

    /// Turn a frame into an event, folding tile data into the world view on the way.
    fn interpret(&mut self, frame: Frame) -> Result<Event> {
        match frame.id {
            id::TILE_SECTION => {
                let bounds = self.world.apply_section(&frame.payload)?;
                let (sx, sy) = ClientWorld::section_of(bounds.x, bounds.y);
                Ok(Event::SectionLoaded {
                    section_x: sx,
                    section_y: sy,
                })
            }
            id::AREA_TILE_CHANGE => {
                // Merged onto this client's own record of the tile, the same as a real client
                // merges onto `Main.tile` (`MessageBuffer.cs:1358-1437`) — see `TileSquare::decode`'s
                // own doc. Matters here because this crate exists to compare against a real
                // server's wire behaviour; decoding into a fresh tile every time would silently
                // diverge from what a real client ends up holding whenever a square omits a field.
                let square =
                    terrustia_proto::square::TileSquare::decode(&frame.payload, |x, y| {
                        self.world.tile(x, y).unwrap_or(terrustia_proto::Tile::AIR)
                    })?;
                for dx in 0..usize::from(square.width) {
                    for dy in 0..usize::from(square.height) {
                        if let Some(tile) = square.tile(dx, dy) {
                            self.world.set_tile(
                                i32::from(square.x) + dx as i32,
                                i32::from(square.y) + dy as i32,
                                tile,
                            );
                        }
                    }
                }
                Ok(Event::Other(frame))
            }
            id::TILE_MANIPULATION => {
                let edit = TileManipulation::decode(&frame.payload)?;
                self.apply_edit(&edit);
                Ok(Event::TileChanged(edit))
            }
            id::PLAYER_ACTIVE => {
                let mut r = PacketReader::new(&frame.payload);
                Ok(Event::PlayerActive {
                    slot: r.u8()?,
                    active: r.bool()?,
                })
            }
            id::PLAYER_CONTROLS => {
                let controls = packets::PlayerControls::decode(&frame.payload)?;
                Ok(Event::PlayerMoved {
                    slot: controls.player,
                    x: controls.position.0,
                    y: controls.position.1,
                })
            }
            id::NET_MODULES => {
                // A server-to-client chat line is `[module][author][text][colour]`, which is a
                // different shape from the `[module][command][text]` a client sends.
                let mut r = PacketReader::new(&frame.payload);
                if r.u16()? == net_module::MODULE_TEXT {
                    let author = r.u8()?;
                    let text = NetworkText::read(&mut r)?;
                    return Ok(Event::Chat {
                        author,
                        text: text.text,
                    });
                }
                // Water moving is module 0, not a tile square, so a client that only reads
                // squares watches a flooding room stay dry.
                if let Some(changes) = net_module::decode_liquid_changes(&frame.payload)? {
                    for change in &changes {
                        self.world
                            .set_liquid(change.x, change.y, change.amount, change.kind);
                    }
                    return Ok(Event::LiquidChanged(changes));
                }
                Ok(Event::Other(frame))
            }
            id::SYNC_ITEM | id::SPAWN_INSTANCED_ITEM => {
                Ok(Event::ItemSynced(SyncItem::decode(&frame.payload)?))
            }
            id::SYNC_N_P_C => {
                let sync = SyncNpc::decode(&frame.payload)?;
                self.world.apply_npc(&sync);
                Ok(Event::NpcSynced(sync))
            }
            id::SYNC_PROJECTILE => Ok(Event::ProjectileSynced(
                terrustia_proto::projectile::SyncProjectile::decode(&frame.payload)?,
            )),
            id::KILL_PROJECTILE => Ok(Event::ProjectileKilled(
                terrustia_proto::projectile::KillProjectile::decode(&frame.payload)?,
            )),
            id::PLAYER_HURT_V2 => Ok(Event::PlayerHurt(
                terrustia_proto::hurt::PlayerHurt::decode(&frame.payload)?,
            )),
            id::PLAYER_DEATH_V2 => Ok(Event::PlayerDied(
                terrustia_proto::hurt::PlayerDeath::decode(&frame.payload)?,
            )),
            id::SYNC_EQUIPMENT => Ok(Event::EquipmentSynced(
                terrustia_proto::inventory::SyncEquipment::decode(&frame.payload)?,
            )),
            id::ITEM_OWNER => Ok(Event::ItemReserved(ItemOwner::decode(&frame.payload)?)),
            id::SYNC_ITEM_DESPAWN => Ok(Event::ItemDespawned(decode_item_despawn(&frame.payload)?)),
            id::FINISHED_CONNECTING_TO_SERVER => Ok(Event::FinishedConnecting),
            id::KICK => Err(ClientError::Kicked {
                reason: kick_reason(&frame.payload),
            }),
            _ => Ok(Event::Other(frame)),
        }
    }

    /// Mirror a single-tile edit into the local world view.
    fn apply_edit(&mut self, edit: &TileManipulation) {
        let (x, y) = (i32::from(edit.x), i32::from(edit.y));
        let Some(mut tile) = self.world.tile(x, y) else {
            return;
        };
        match edit.action {
            0 | 4 if edit.destroyed() => {
                tile.flags.set(terrustia_proto::TileFlags::ACTIVE, false);
                tile.block = 0;
            }
            1 => {
                tile.block = edit.arg.max(0) as u16;
                tile.flags.set(terrustia_proto::TileFlags::ACTIVE, true);
            }
            2 if edit.destroyed() => tile.wall = 0,
            3 => tile.wall = edit.arg.max(0) as u16,
            _ => return,
        }
        self.world.set_tile(x, y, tile);
    }

    async fn expect(&mut self, id: u8, what: &str) -> Result<Frame> {
        self.expect_one_of(&[id], what).await
    }

    /// Read until one of `ids` arrives, folding anything else in on the way.
    async fn expect_one_of(&mut self, ids: &[u8], what: &str) -> Result<Frame> {
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(ClientError::Timeout {
                    expected: what.to_string(),
                    seconds: self.timeout.as_secs(),
                });
            }
            let frame = self.read_frame().await?;
            if ids.contains(&frame.id) {
                return Ok(frame);
            }
            debug!(
                id = frame.id,
                name = id::name(frame.id),
                "skipping while waiting for {what}"
            );
        }
    }

    /// Read one whole frame, waiting for as much of the socket as it takes.
    async fn read_frame(&mut self) -> Result<Frame> {
        loop {
            if let Some(frame) = codec::decode(&mut self.buf)? {
                return Ok(frame);
            }
            let before = self.buf.len();
            let read = timeout(self.timeout, self.stream.read_buf(&mut self.buf))
                .await
                .map_err(|_| ClientError::Timeout {
                    expected: "a packet".into(),
                    seconds: self.timeout.as_secs(),
                })??;
            if read == 0 {
                return Err(ClientError::Closed);
            }
            if let Some(tap) = self.tap.as_mut() {
                tap.chunk(crate::tap::Direction::ToClient, &self.buf[before..]);
            }
        }
    }
}

fn kick_reason(payload: &[u8]) -> String {
    let mut r = PacketReader::new(payload);
    NetworkText::read(&mut r)
        .map(|t| t.text)
        .unwrap_or_else(|_| "no reason given".into())
}
