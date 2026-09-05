use std::{
    collections::{HashSet, VecDeque},
    net::SocketAddr,
};

use bytes::Bytes;
use tokio::sync::mpsc;

/// How far through the handshake a connection has progressed.
///
/// Ordered, so a handler can require "at least this far" with a comparison. Packets that arrive out
/// of order are dropped rather than trusted: a client is free to send anything at any time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConnState {
    /// Socket accepted; the version string has not been checked yet.
    Greeting,
    /// Version accepted and a slot assigned.
    SlotAssigned,
    /// Appearance received, so the player has a name.
    Identified,
    /// Packet 7 sent.
    WorldSent,
    /// Sections streamed and `StartPlaying` sent.
    TilesSent,
    /// Spawned and visible to everyone else.
    Playing,
}

/// Server-side view of one connected client.
pub struct Player {
    pub slot: u8,
    pub addr: SocketAddr,
    pub out: mpsc::Sender<Bytes>,
    /// This connection's share of the server-wide outbound byte budget.
    ///
    /// Defaulted to a counter of its own by [`Player::new`] and replaced with the connection's real
    /// one by `allocate_slot`, the same shape `close` uses: a `Player` built in a test has no
    /// connection behind it and must not have to invent one to exist.
    pub queued: crate::net::connection::QueuedBytes,
    /// Ends this player's connection now, instead of at the far end of whatever `out` still holds.
    ///
    /// See [`crate::net::connection::Closer`] for why the queue running dry is the wrong signal.
    /// `None` for a player who has no real socket behind them, which is every test that seats one
    /// directly; firing it is `GameServer::disconnect`'s job and nobody else's.
    pub close: Option<crate::net::connection::Closer>,
    pub state: ConnState,
    pub name: String,
    /// The client's own self-reported identifier (packet 68). **Never proof of identity**: an
    /// unmodified client sends whatever value it likes, so nothing here may key an auth or
    /// permission decision off it: not sign-in, not account lookup, not a group or a permission
    /// check. The one legitimate use is as a ban *kind* alongside name and address
    /// (`admin::BanKind::Uuid`, `admin::store::Admin::ban_for`), three different ways to name
    /// somebody so a ban survives whichever the culprit can most easily change, never a claim
    /// about who they are. See `admin::ban`'s own module doc.
    pub uuid: Option<String>,
    pub position: (f32, f32),
    /// How fast they were moving at the last position update.
    ///
    /// Derived rather than received: the client sends a position and a velocity, but the velocity
    /// it sends is its own idea of the frame, so tracking the difference between updates is what
    /// the routines that lead their target actually want.
    pub velocity: (f32, f32),
    pub life: i16,
    pub life_max: i16,
    /// Ticks of invulnerability left after a hit.
    ///
    /// Without this a player standing in a zombie would take sixty hits a second. The game gives
    /// everyone a brief grace period after any hit, and it is what makes contact damage survivable.
    pub immune_ticks: i32,
    pub mana: i16,
    pub mana_max: i16,
    pub team: u8,
    /// The client's own packet 4 payload, replayed verbatim to describe this player to others.
    ///
    /// Appearance carries dozens of fields (dyes, accessory visibility, per-slot colours) that the
    /// server has no reason to model; relaying the original bytes keeps every one of them intact.
    pub appearance: Option<Bytes>,
    /// The most recent packet 13 payload, so a joining player sees everyone in their real pose.
    pub last_controls: Option<Bytes>,
    /// Whether this player is currently sitting (packet 13's own `bitsByte26[2]`).
    ///
    /// Real vanilla checks this every frame a player sits (`PlayerSittingHelper.UpdateSitting`) to
    /// see whether they are on the one specific chair that turns the nearby Clothier into a
    /// red-hatted Skeletron; nothing else in this project reads it yet.
    pub sitting: bool,
    /// Which hotbar slot (0-9) is currently selected, from the same packet.
    ///
    /// Paired with `inventory` to answer "what item is this player currently holding" — needed for
    /// the same red-hat Skeletron check above, which only fires while the selected item is the
    /// Clothier Voodoo Doll.
    pub selected_item: u8,
    /// Which town NPC this player has open, if any. A shop needs to know.
    pub talking_to: Option<u8>,
    /// What that resident's happiness does to his prices, as of the moment they started talking.
    ///
    /// The game's `Player.currentShoppingSettings.PriceAdjustment`: 1.0 when nobody is being
    /// talked to (`ShoppingSettings.NotInShop`, `ShoppingSettings.cs:9-13`), otherwise the number
    /// `ShopHelper` quotes. Taken once, at the moment `SetTalkNPC` runs (`Player.cs:4360-4375`),
    /// and deliberately not refreshed until the next one: the game snapshots it too, so a shopper
    /// who wanders into the corruption mid-purchase keeps the price they were quoted.
    ///
    /// Read only by `/happy`. See `terrustia_proto::happiness` for why nothing else can read it:
    /// prices are client-authoritative in this protocol.
    pub shop_multiplier: f32,
    /// Which way this player is looking.
    ///
    /// Only a wiring tool reads it, and only to decide which way its path turns the corner — but
    /// getting that wrong lays the wire along the wrong two sides of the rectangle, which is
    /// obvious the moment it happens.
    pub facing_right: bool,
    /// How many of the Angler's quests this character has finished.
    ///
    /// Character state rather than world state, so the server remembers what it is told and
    /// passes it on. The Angler's reward tiers are gated on it, which is why it has to reach
    /// every client rather than staying with the one that owns the character.
    pub angler_quests: i32,
    /// ...and their accumulated golf score, which travels in the same message.
    pub golf_score: i32,
    /// What this player is carrying, by slot.
    ///
    /// Sparse: a client sends a slot only when it holds something or when it has just been
    /// emptied, and the great majority of a player's three hundred and ninety-five slots are empty
    /// for their whole session. Storing the whole array per player would be most of a megabyte of
    /// nothing across a full server.
    pub inventory: std::collections::HashMap<u16, terrustia_proto::inventory::SyncEquipment>,
    /// Whether a valid `Hello` has been seen.
    ///
    /// The password exchange happens while the connection is still in `Greeting`, so without this
    /// a client could skip the version check by sending only a password.
    pub greeted: bool,
    /// Whether this connection has cleared the server password, if one is set.
    pub password_ok: bool,
    /// Whether this player has PvP enabled.
    pub pvp: bool,
    /// Vanilla's tile-edit spam counters, which this server was missing entirely.
    ///
    /// `RemoteClient` keeps a float per kind, bumps it on every edit packet, decays it each tick,
    /// and boots the connection past a ceiling — 100 for placing, 500 for breaking, 50 for
    /// liquid. Not having them was a *regression from vanilla*, not merely a place where we are
    /// as trusting as vanilla is, which is why it belongs inside "match vanilla's trust model"
    /// rather than outside it.
    ///
    /// Floats because the decay rates are fractional (0.3 a tick for placing) and rounding them
    /// to integers would change how long a burst is tolerated.
    pub spam_place: f32,
    pub spam_break: f32,
    pub spam_liquid: f32,
    /// The client's own buff packet, replayed so others see the same buff icons.
    pub buffs: Option<Bytes>,
    /// The eight luck factors this client last reported (packet 134), and what they add up to.
    ///
    /// Two fields rather than one because vanilla keeps two: the factors are what the packet
    /// carries and `RecalculateLuck` reads, and `luck` is what every roll reads. Recomputing the
    /// total on every roll would be wrong as well as wasteful - the total also depends on server
    /// state (a Lantern Night, a Stinkbug debuff) that can change without a packet arriving, and
    /// `GameServer::refresh_luck` is what folds that in.
    pub luck_factors: terrustia_proto::luck::LuckFactors,
    /// `Player.luck`: the number `Luck.RollLuck` scales a range by. Zero for a client that has
    /// never sent packet 134, which is what every roll in this server assumed of everybody before
    /// the packet was read at all.
    pub luck: f32,
    /// The client's last biome-zone packet.
    pub zone: Option<Bytes>,
    /// Which chest this player currently has open, or -1 for none.
    ///
    /// Vanilla refuses to open a chest another player is already in, so the server has to know.
    pub open_chest: i16,
    /// Sections already streamed to this client (`RemoteClient.TileSections`,
    /// `RemoteClient.cs:31`).
    ///
    /// The server pushes sections from player positions every tick (`Main.cs:65601` ->
    /// `RemoteClient.CheckSection`); packet 159 is only a client's own miss-repair request. Both
    /// paths run through `send_section`, which consults this, so a walk back and forth never
    /// resends megabytes of tiles.
    pub sent_sections: HashSet<(i32, i32)>,
    /// Which section this client was last checked against, so the per-tick stream
    /// (`check_player_sections`) does no work at all for a player who has not crossed a section
    /// boundary since the last tick.
    ///
    /// Vanilla has no equivalent: `CheckSection_ForClient` re-walks its 3x3 block every tick for
    /// every player. That is nine array reads there and nine hash lookups here, and at 255 players
    /// this server has a 16.67ms tick to protect, so the boundary is remembered instead. It also
    /// stops a section that is queued but not yet drained from being queued again on every tick in
    /// between.
    pub last_section: Option<(i32, i32)>,
    /// Sections still owed to this client from its initial world stream, drained a few at a time
    /// off the tick by `drain_section_streams` rather than sent in one synchronous burst inside the
    /// `SpawnTileData` packet handler — a first join can want up to ~39 of them, and sending them
    /// all in one call used to block every other player's tick for the whole burst.
    pub pending_sections: VecDeque<(i32, i32)>,
    /// When the game task first saw this connection, for the handshake reaper.
    ///
    /// The connection task has a handshake deadline of its own (`net::connection::read_loop`), but
    /// it stops applying once a connection has sent more than `HANDSHAKE_FRAMES` (64) frames - past
    /// that it is treated as an established session on the ordinary idle timeout. A client that
    /// sends sixty-five frames of anything and then never finishes joining therefore escapes that
    /// deadline entirely and holds a player slot for as long as it keeps the socket warm. Slots are
    /// the scarce resource (`max_players`, at most 255), so the game task needs its own clock on
    /// them, and this is it: set once when the slot is handed out and never touched again.
    pub connected_at: std::time::Instant,
    /// When this connection's last *accepted* chat line went out, for `config.chat_cooldown_ms` —
    /// a config-enabled mechanism, off by default (see `dispatch.rs`'s chat handling). `None` until
    /// the first line; reset on reconnect like every other per-connection field, deliberately: a
    /// cooldown is about this session's own pace, not a persistent record of the account.
    pub last_chat: Option<std::time::Instant>,
    /// How many server-issued teleports this client has not yet acknowledged
    /// (`Player.unacknowledgedTeleports`). Bumped whenever the server relocates this player
    /// without their own client having already moved itself locally first (a Teleportation
    /// Potion, a pylon), and dropped back down on packet 65's own ack sub-message (`what == 3`).
    /// While it is above zero, `on_player_controls` distrusts the position and velocity this
    /// client reports — they may still describe where it was *before* a teleport it has not
    /// caught up with yet (`MessageBuffer.cs:998-1002`).
    pub unacknowledged_teleports: u32,
}

impl Player {
    /// Whether this player is on the same machine as the server.
    ///
    /// The game's whole test for who counts as the host, in `NetMessage.
    /// DoesPlayerSlotCountAsAHost`, is whether the socket's far end is the loopback address. It
    /// gates packet 139, and through it a handful of things the client treats as the host's to
    /// decide.
    pub fn is_local(&self) -> bool {
        self.addr.ip().is_loopback()
    }

    pub fn new(slot: u8, addr: SocketAddr, out: mpsc::Sender<Bytes>) -> Self {
        Self {
            slot,
            addr,
            out,
            queued: crate::net::connection::QueuedBytes::new(std::sync::Arc::new(
                std::sync::atomic::AtomicUsize::new(0),
            )),
            close: None,
            state: ConnState::Greeting,
            name: format!("Player {slot}"),
            uuid: None,
            position: (0.0, 0.0),
            velocity: (0.0, 0.0),
            life: 100,
            immune_ticks: 0,
            life_max: 100,
            mana: 20,
            mana_max: 20,
            team: 0,
            appearance: None,
            last_controls: None,
            sitting: false,
            selected_item: 0,
            talking_to: None,
            shop_multiplier: 1.0,
            facing_right: true,
            angler_quests: 0,
            golf_score: 0,
            inventory: std::collections::HashMap::new(),
            greeted: false,
            password_ok: false,
            pvp: false,
            spam_place: 0.0,
            spam_break: 0.0,
            spam_liquid: 0.0,
            buffs: None,
            luck_factors: terrustia_proto::luck::LuckFactors::default(),
            luck: 0.0,
            zone: None,
            open_chest: -1,
            sent_sections: HashSet::new(),
            last_section: None,
            pending_sections: VecDeque::new(),
            connected_at: std::time::Instant::now(),
            last_chat: None,
            unacknowledged_teleports: 0,
        }
    }

    /// Whether this player is in the world and should receive and generate broadcasts.
    pub fn is_playing(&self) -> bool {
        self.state == ConnState::Playing
    }

    /// `Player.ZoneGraveyard`: whether this player is standing among enough tombstones.
    ///
    /// Read off the client's own zone packet rather than worked out here, because that is exactly
    /// what a vanilla *server* does. The zone flags are computed client-side by `SceneMetrics.Scan`
    /// (a 169-by-124-tile box around the player, `SceneMetrics.cs:16` and `:356`; a graveyard is
    /// `_tileCounts[85] - _tileCounts[27] / 2 >= 28`, `SceneMetrics.cs:622-623` and `:272`) and sent
    /// up as packet 36; `MessageBuffer.cs:2178-2200` stores them straight onto `Main.player[i]`, and
    /// `NPC.SetSpawnFlags` (`NPC.cs:389`) then copies `player.ZoneGraveyard` into the spawner. The
    /// server never scans tiles for this at all, which is why a graveyard costs the tick nothing.
    ///
    /// The layout is `MessageBuffer.cs:2179-2189` / `NetMessage.cs:936-946`:
    /// `[player id][zone1][zone2][zone3][zone4][zone5][townNPCs]`, and `ZoneGraveyard` is `zone4[6]`
    /// (`Player.cs:3771-3781`, the seventh property in that byte's run).
    ///
    /// A client that has not sent a zone packet yet reads as not in a graveyard, which is the same
    /// answer vanilla gives for a freshly zeroed `Player`.
    pub fn in_graveyard(&self) -> bool {
        /// Byte 0 is the player id, so `zone4` is the fifth byte of the payload.
        const ZONE4: usize = 4;
        /// `Player.ZoneGraveyard` is `zone4[6]` (`Player.cs:3771`).
        const GRAVEYARD_BIT: u8 = 1 << 6;
        self.zone
            .as_ref()
            .and_then(|zone| zone.get(ZONE4))
            .is_some_and(|zone4| zone4 & GRAVEYARD_BIT != 0)
    }

    /// Advance the handshake, never backwards.
    ///
    /// A client that re-sends an earlier packet must not be able to rewind its own state and
    /// replay a stage.
    pub fn advance_to(&mut self, state: ConnState) {
        if state > self.state {
            self.state = state;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_player() -> Player {
        let (tx, _rx) = mpsc::channel(4);
        Player::new(1, "127.0.0.1:1".parse().unwrap(), tx)
    }

    #[test]
    fn state_only_moves_forward() {
        let mut p = test_player();
        p.advance_to(ConnState::WorldSent);
        assert_eq!(p.state, ConnState::WorldSent);

        p.advance_to(ConnState::SlotAssigned);
        assert_eq!(p.state, ConnState::WorldSent, "state must not rewind");
    }

    #[test]
    fn a_fresh_player_is_not_playing() {
        assert!(!test_player().is_playing());
    }

    /// The graveyard is one bit of one byte of the zone packet, and picking the wrong one would
    /// read some other zone silently: `ZoneGraveyard` is `zone4[6]` (`Player.cs:3771`) and `zone4`
    /// is the fifth byte of the payload, after the player id (`NetMessage.cs:936-946`).
    ///
    /// Its neighbours are checked as well, since an off-by-one here would read the Lihzahrd Temple
    /// or the Shadow Candle instead and nothing would ever say so.
    #[test]
    fn a_graveyard_is_the_seventh_bit_of_the_fifth_zone_byte() {
        let mut p = test_player();
        assert!(!p.in_graveyard(), "a client that has sent no zone packet");

        p.zone = Some(Bytes::from_static(&[3, 0xff, 0xff, 0xff, 1 << 6, 0, 0]));
        assert!(p.in_graveyard());

        // `zone4[5]` is ZoneLihzhardTemple and `zone4[7]` is ZoneShadowCandle: neither is this.
        p.zone = Some(Bytes::from_static(&[3, 0, 0, 0, (1 << 5) | (1 << 7), 0, 0]));
        assert!(!p.in_graveyard());

        // The same bit in any *other* byte is a different zone entirely.
        p.zone = Some(Bytes::from_static(&[
            3,
            1 << 6,
            1 << 6,
            1 << 6,
            0,
            1 << 6,
            0,
        ]));
        assert!(!p.in_graveyard());

        // A truncated payload reads as no graveyard rather than panicking on the packet path.
        p.zone = Some(Bytes::from_static(&[3, 0, 0, 0]));
        assert!(!p.in_graveyard());
    }

    #[test]
    fn states_are_ordered_along_the_handshake() {
        assert!(ConnState::Greeting < ConnState::SlotAssigned);
        assert!(ConnState::SlotAssigned < ConnState::Identified);
        assert!(ConnState::Identified < ConnState::WorldSent);
        assert!(ConnState::WorldSent < ConnState::TilesSent);
        assert!(ConnState::TilesSent < ConnState::Playing);
    }
}
