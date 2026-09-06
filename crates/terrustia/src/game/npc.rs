//! NPC entities: state, the fixed slot table, and tile-aware movement.
//!
//! Behaviour lives in [`super::npc_ai`]; this module is what every style shares — gravity,
//! collision against the world, and the bookkeeping the network layer needs.

use terrustia_proto::{
    Tile,
    npc::MAX_NPCS,
    npc_data::{NpcStats, npc_stats},
    npc_params::SOLAR_SROLLER,
    tile_solid::{solid, solid_top},
};

/// Downward acceleration, from `NPC.gravity`.
pub const GRAVITY: f32 = 0.3;

/// Terminal speed, from `UpdateNPC_UpdateGravity`.
pub const MAX_FALL_SPEED: f32 = 10.0;

/// The same two in liquid, from `UpdateNPC_UpdateGravity` (`NPC.cs:92054-92071`).
///
/// Being in a liquid replaces both outright rather than scaling them, and which liquid decides
/// which pair: shimmer 0.15/5.5, honey 0.1/4, anything else (water and lava alike, since vanilla's
/// `wet` covers both) 0.2/7. Without them every land NPC fell through water at 1.5 times vanilla's
/// gravity to 1.43 times its terminal speed, and sank through honey at three times the pull.
pub const WET_GRAVITY: f32 = 0.2;
pub const WET_MAX_FALL_SPEED: f32 = 7.0;
pub const HONEY_GRAVITY: f32 = 0.1;
pub const HONEY_MAX_FALL_SPEED: f32 = 4.0;
pub const SHIMMER_GRAVITY: f32 = 0.15;
pub const SHIMMER_MAX_FALL_SPEED: f32 = 5.5;

/// One world tile in pixels.
pub const TILE: f32 = 16.0;

/// Ticks an ordinary enemy survives with no player nearby before it despawns.
///
/// The game's `NPC.activeTime`, which is 750 — twelve and a half seconds. This was `60 * 60 * 12`,
/// a twelve-minute grace that is fifty-seven times too long, and it showed: a five-minute capture
/// caught a flying enemy that had left through the top of the world and was still being simulated
/// and broadcast eight thousand pixels above it, because nothing was going to reap it for another
/// seven minutes. The everyday cost is subtler and larger — every creature that wanders away from
/// a player holds its slot and its share of the sync budget for twelve minutes instead of twelve
/// seconds.
pub const DEFAULT_TIME_LEFT: i32 = 750;

/// What one full NPC sync costs the rate-limiting bucket, from `NPC.netSpamTicksPerPacket`.
///
/// The bucket drains by one a tick, so thirty is one packet every half second, sustained.
pub const NET_SPAM_PER_PACKET: i32 = 30;

/// The same for a boss, from `NPC.netSpamTicksPerPacketForBosses` — six times as often, because a
/// boss fight is the one place a client cannot afford to be guessing where something is.
pub const NET_SPAM_PER_PACKET_BOSS: i32 = 5;

/// How many packets may be sent back to back before the bucket runs dry, from
/// `NPC.netSpamPacketLimit`.
pub const NET_SPAM_PACKET_LIMIT: i32 = 3;

/// Ticks between rounds of proximity streaming, from `Main.npcStreamSpeed`.
pub const NPC_STREAM_SPEED: u8 = 30;

/// Anything the movement code needs to know about the world.
///
/// A trait rather than a direct `World` reference so the physics can be tested against hand-built
/// terrain without standing up a whole world.
pub trait TileView {
    fn tile(&self, x: i32, y: i32) -> Tile;
}

/// A live NPC.
#[derive(Debug, Clone, PartialEq)]
pub struct Npc {
    pub npc_type: u16,
    /// What goes on the wire in packet 23, which is not always the type.
    ///
    /// A negative net id names a *variant* of a positive type rather than a type of its own
    /// (`NPCID.FromNetId`, and [`terrustia_proto::net_variants`] for what each one changes): the
    /// coloured slimes, the small and big zombies and skeletons, and the hornet families all ride
    /// on one base type each. `NPC.type` is always the positive base, and `NPC.netID` carries the
    /// negative (`NPC.cs:7681-7684`).
    ///
    /// This is the type for everything ordinary, which is nearly everything. It was *always* the
    /// type before, so all sixty-five variants were absent from every world this server served:
    /// nothing errored and no test failed, because a Pinky sent as a Blue Slime is a perfectly
    /// valid Blue Slime.
    pub net_id: i16,
    /// Bumped each time a slot is reused, so a stale hit cannot land on the new occupant.
    pub generation: u8,
    /// Top-left corner, in pixels.
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    pub life: i32,
    pub life_max: i32,
    /// Player slot being chased, or 255 for nobody.
    pub target: u16,
    pub direction: i8,
    pub direction_y: i8,
    pub sprite_direction: i8,
    pub ai: [f32; 4],
    /// The game's `localAI`: four more slots that are never sent over the wire.
    ///
    /// Keeping them separate matters. Several routines use an `ai` slot and a `localAI` slot for
    /// different things at the same time — a snail's `ai[1]` says which way round it is crawling
    /// while its `localAI[3]` counts how long it has been touching nothing — and folding the two
    /// together silently breaks the routine.
    pub local_ai: [f32; 4],
    pub stats: NpcStats,
    /// A lunar pillar's shield: how many of its minions are still unaccounted for.
    ///
    /// Nothing else uses it. It lives on the NPC rather than in a global because a world can hold
    /// four pillars at once and each one's shield falls separately.
    pub shield: i32,
    /// A size that is not the type's, for the routines that grow or shrink mid-life.
    ///
    /// `None` means "whatever the type says", which is the case for all but a handful.
    pub size: Option<(f32, f32)>,
    /// Armour as it stands right now, which is not always what the type says: a training dummy
    /// drops its own to zero, and several hardmode enemies harden when they curl up.
    pub defense: i32,
    /// How faded out it is, 0 solid and 255 invisible.
    ///
    /// Mostly a drawing concern, but not only: a pirate ghost with nobody to chase fades and the
    /// end of the fade is what kills it, so the number has to be kept on the server.
    pub alpha: i32,
    /// A multiplier on this one's contact damage, for the routines that hit harder in some phase
    /// than others. One means "whatever the type says".
    pub damage_bonus: f32,
    /// Set while a routine is in a phase that shrugs off knockback regardless of the type's
    /// resistance — a rolling tortoise is not going to be shoved off course.
    pub knockback_immune: bool,
    /// Whether anything can hurt this one right now. Vanilla's `NPC.dontTakeDamage`, and the only
    /// thing [`Self::strike`] asks.
    ///
    /// There is exactly one of these, because vanilla has exactly one. `SetDefaults` seeds it from
    /// the type (that seed is `NpcStats::dont_take_damage`, copied in here at spawn and again on
    /// [`Self::become_type`]) and from then on it belongs to the routine: a phase that cannot be
    /// hurt sets it, a phase that can clears it, and a type vanilla never lets you hurt simply has
    /// no routine that clears it.
    ///
    /// It was briefly two: this field for the routine, and `stats.dont_take_damage` for the type,
    /// with the damage gate taking either. That made the type's seed permanent, so the three
    /// bosses whose routines open the window by clearing the routine flag - the Empress, the Moon
    /// Lord's core and the Martian Saucer's core - could not be hurt at all, by anything, ever. The
    /// Moon Lord one meant the game could not be finished: `checkdead` is reached through a lethal
    /// blow, and no blow was ever lethal. Anything writing `stats.dont_take_damage` after spawn is
    /// writing to a copy of the type table that nothing reads; write here instead.
    pub invulnerable: bool,
    /// Counts down while no player is near; the NPC despawns at zero.
    pub time_left: i32,
    pub on_ground: bool,
    /// Velocity as it was before this tick's movement.
    ///
    /// Several styles bounce off terrain by reflecting the velocity they *had*, not the zeroed one
    /// collision leaves behind, so it has to be kept.
    pub old_velocity: (f32, f32),
    /// Whether movement was stopped by terrain this tick.
    ///
    /// The game exposes these as `collideX` / `collideY` and several AI styles read them — a
    /// Fungi Spore bursts on either, and the fighter uses them to decide when to jump.
    pub collide_x: bool,
    pub collide_y: bool,
    /// How large this one is, from `SetDefaults`.
    ///
    /// A few routines read it as more than decoration: a hornet's speed is `2 - scale`, so a
    /// bigger one is slower, and its stinger's damage scales with it directly.
    pub scale: f32,
    /// Position as it was before this tick's movement.
    ///
    /// A routine that wants to know whether it actually got anywhere — a snowman checking whether
    /// it is stuck against a wall, a snail checking whether it is still on one — compares against
    /// this rather than trusting its own velocity.
    pub old_position: (f32, f32),
    /// Which way the sprite is turned, in radians.
    ///
    /// Purely visual for most types, but the wheels and worms keep their state in it, so it has to
    /// persist between ticks like anything else.
    pub rotation: f32,
    /// Whether the NPC was hit since its routine last ran.
    ///
    /// The game calls this `justHit`. A perched bird takes off on it, and several routines drop
    /// what they are doing when it is set.
    pub was_hurt: bool,
    /// Set whenever the state changed enough to be worth telling clients about.
    pub dirty: bool,
    /// The game's `netSpam`: a token bucket that decides how often this NPC may be sent in full.
    ///
    /// A sync costs [`NET_SPAM_PER_PACKET`] (or [`NET_SPAM_PER_PACKET_BOSS`]) and the bucket drains
    /// by one a tick, so the sustained rate is one packet per thirty ticks — twice a second for a
    /// boss, once every half second for anything else — with a burst of three allowed on top.
    ///
    /// Without it every NPC that moves is sent at whatever rate the sync loop runs, which for this
    /// server was ten a second: twenty times the game's sustained rate, and measured at seven times
    /// its bandwidth over a five-minute capture.
    pub net_spam: i32,
    /// Ticks counted towards the next round of proximity streaming.
    pub net_stream: u8,
    /// What this one was scaled by, kept so a transform can be scaled the same way.
    scaling: Scaling,
    /// Tile a town NPC calls home, if it has been given a house.
    pub home: Option<(i32, i32)>,
    /// Whether gravity is off this tick.
    ///
    /// The game keeps `noGravity` as mutable state on the NPC rather than as a property of the
    /// type: a bird has gravity while it is perched and none once it takes off, and its routine
    /// sets the flag both ways every tick. Starts from the type's default.
    pub no_gravity: bool,
    /// Whether terrain is ignored this tick, kept mutable for the same reason.
    pub no_tile_collide: bool,
    /// For a balloon, the slot of whatever is hanging from it.
    ///
    /// Unlike a worm segment, a passenger does not trail: it is held at a fixed point below its
    /// carrier and inherits its velocity outright.
    pub passenger: Option<u8>,
    /// For a boss part, the slot of the boss it belongs to.
    ///
    /// Unlike a worm segment or a balloon's passenger, a part steers itself; it only needs to know
    /// where its parent is and what that parent is doing.
    pub follows_boss: Option<u8>,
    /// For a worm segment, the slot of the segment ahead of it.
    ///
    /// A worm is a chain of separate NPCs; only the head steers, and every other link keeps a
    /// fixed distance behind the one in front.
    pub follows: Option<u8>,
    /// Whether a statue made this one.
    ///
    /// It is worth no coins and takes up no room in the spawn budget, which is the only reason a
    /// statue farm works: without it a wired statue would stop the world spawning anything else.
    pub from_statue: bool,
    /// Whether a player has ever hurt this one, vanilla's `playerInteraction`
    /// (`NPC.cs:80756-80880`, and `AnyInteractions` at `:79535`).
    ///
    /// Set only by a player's own blow: real vanilla writes it from `StrikeNPC`'s `owner`
    /// (`NPC.cs:82095-82101`) and, on a server, from the client's damage packet
    /// (`MessageBuffer.cs:1789`). A townsperson's swing does not set it and neither does anything
    /// environmental, which is exactly the distinction the one thing that reads it needs: killing a
    /// Prismatic Lacewing wakes the Empress of Light only `if (GetWereThereAnyInteractions())`
    /// (`NPC.cs:80310`), so a critter cut down by a passing Guide is not a boss summon.
    ///
    /// Narrowed to a single flag rather than vanilla's 255-entry per-player array, because the one
    /// reader asks "did anybody" and never "who".
    pub hit_by_player: bool,
    /// What is currently burning, poisoning or cursing it.
    ///
    /// Kept on the NPC rather than in a side table because it is read every tick by the routine
    /// that decides damage and written by any client that lands a hit; a lookup either way round
    /// would be a scan of the whole roster.
    pub buffs: super::buffs::Buffs,
    /// Set when the buff list changed and clients have not been told yet.
    ///
    /// Separate from `dirty` because the two go out as different packets: `dirty` sends the
    /// NPC's position and health, this sends its buff list, and a burning enemy standing still
    /// needs only the second.
    pub buffs_dirty: bool,
    /// Ticks before this NPC can be hit *by the server* again. `NPC.immune[255]`.
    ///
    /// Vanilla gives every NPC a 256-wide array of hit cooldowns, one per player, and index 255 is
    /// the server's own: `Main.myPlayer` is 255 on a dedicated server (`Netplay.cs:250`), so every
    /// hit the server originates - a town NPC's projectile and a town NPC's melee swing, which are
    /// the only two there are - checks and sets this slot. The other 255 are the clients' and
    /// arrive as packet 28, where the client has already applied its own.
    ///
    /// One number rather than an array because a server only ever needs its own: nothing here
    /// simulates a player's weapon. It counts down once per NPC update (`NPC.cs:91564-91567`).
    pub immune_ticks: i32,
    /// The personal name a town NPC, pet or slime carries on top of its type.
    ///
    /// Empty for everything else. A client asks for this the moment the NPC comes into view and
    /// shows the type's name until it is answered, so a server that never answers gives you a
    /// town full of people called "Guide".
    pub given_name: String,
    /// Coins this one is carrying beyond what its type is worth.
    ///
    /// The Coin Loss revenge system: money dropped on death is remembered against whatever killed
    /// you, and killing that back gives it up. It accumulates rather than being set, because two
    /// players can both feed the same enemy.
    pub extra_value: i32,
    /// Which of a type's two looks it wears, for the four types that have two.
    ///
    /// The Dryad, the Truffle, the Princess and the Guide each have an alternate; the game keeps
    /// the choice as a number rather than a flag because it is sent alongside the name.
    pub town_variation: i32,
    /// Whether a homeless townsperson is on the way out because their house was destroyed.
    ///
    /// This server has no despawn-timer routine that acts on it yet — it is carried purely so a
    /// world's own `homelessDespawn` flag round-trips rather than being clobbered to `false` on
    /// every save (`WorldFile.cs`'s own field, from file version 315). Set only by
    /// `restore_town_npcs` from what a load decoded; nothing here ever sets it `true`.
    pub homeless_despawn: bool,
    /// A friendly NPC's slow health recovery accumulator (`NPC.friendlyRegen`, `NPC.cs:6578`).
    ///
    /// Climbs by one a tick (more for a few named residents) and heals a single point each time it
    /// passes 180, so a hurt townsperson mends over a couple of minutes rather than staying at a
    /// sliver of health forever. Vanilla ticks it for every friendly NPC; this server only drives it
    /// for town NPCs (see `tick_town_regen`), which are the residents that realistically take a
    /// beating and persist, so nothing else ever touches this counter here.
    pub friendly_regen: i32,
}

/// Fold a type's scale into its hitbox, the way the tail of `SetDefaults` does.
///
/// `NPC.cs:17842-17843`: `width = (int)((float)width * scale); height = (int)((float)height *
/// scale);`. The game has one width field, so every later reader (contact damage, worm segment
/// spacing, line of sight) sees the scaled box. `npc_data.rs` stores the pre-multiply literal the
/// game assigns in the type's own `SetDefaults` branch, so without this a Wall of Flesh is
/// 100x100 on the server while the client, which runs its own `SetDefaults`, draws and collides
/// against 120x120.
///
/// Applied once at construction rather than inside `width()`/`height()` so it costs nothing per
/// tick, and so a routine that rebuilds its own hitbox from a base size (King Slime's `resize`)
/// cannot scale an already-scaled number a second time.
fn scaled(mut stats: NpcStats, scale: f32) -> NpcStats {
    stats.width = (stats.width as f32 * scale) as i32;
    stats.height = (stats.height as f32 * scale) as i32;
    stats
}

impl Npc {
    pub fn new(npc_type: u16, position: (f32, f32), generation: u8) -> Option<Self> {
        let scale = terrustia_proto::npc_params::npc_scale(npc_type);
        let stats = scaled(npc_stats(npc_type)?, scale);
        Some(Self {
            npc_type,
            net_id: npc_type as i16,
            generation,
            position,
            velocity: (0.0, 0.0),
            life: stats.life_max,
            life_max: stats.life_max,
            target: 255,
            direction: 1,
            direction_y: 1,
            sprite_direction: 1,
            ai: [0.0; 4],
            local_ai: [0.0; 4],
            stats,
            shield: 0,
            size: None,
            defense: stats.defense,
            damage_bonus: 1.0,
            knockback_immune: false,
            alpha: 0,
            invulnerable: stats.dont_take_damage,
            time_left: DEFAULT_TIME_LEFT,
            old_velocity: (0.0, 0.0),
            on_ground: false,
            collide_x: false,
            collide_y: false,
            scale,
            old_position: position,
            rotation: 0.0,
            was_hurt: false,
            dirty: true,
            scaling: Scaling::default(),
            no_gravity: stats.no_gravity,
            no_tile_collide: stats.no_tile_collide,
            home: None,
            passenger: None,
            follows_boss: None,
            follows: None,
            from_statue: false,
            hit_by_player: false,
            buffs: super::buffs::Buffs::new(),
            buffs_dirty: false,
            immune_ticks: 0,
            net_spam: 0,
            net_stream: 0,
            given_name: String::new(),
            extra_value: 0,
            town_variation: 0,
            homeless_despawn: false,
            friendly_regen: 0,
        })
    }

    pub fn width(&self) -> f32 {
        self.size.map_or(self.stats.width as f32, |(w, _)| w)
    }

    pub fn height(&self) -> f32 {
        self.size.map_or(self.stats.height as f32, |(_, h)| h)
    }

    /// Change how big this one is, keeping it centred where it already was.
    ///
    /// A few routines resize themselves mid-life and mean it: a chattering teeth bomb swelling to
    /// a hundred and sixty pixels across *is* its blast, because the hitbox is what does the
    /// damage.
    pub fn resize(&mut self, width: f32, height: f32) {
        let (cx, cy) = self.center();
        self.size = Some((width, height));
        self.position = (cx - width / 2.0, cy - height / 2.0);
        self.dirty = true;
    }

    /// Centre of the NPC, which is what the AI aims with.
    pub fn center(&self) -> (f32, f32) {
        (
            self.position.0 + self.width() / 2.0,
            self.position.1 + self.height() / 2.0,
        )
    }

    /// The evil form of a critter, if this type has one: `NPC.AttemptToConvertNPCToEvil`,
    /// `NPC.cs:93050-93083`.
    ///
    /// Three groups and nothing else. The bunny family (Bunny, and the four seasonal and gold
    /// variants), the goldfish family, and the two penguins, each becoming its crimson form or its
    /// corrupt one depending on which evil the world carries. Returns `None` for everything else,
    /// which is most of the roster: this is a critter mechanic, not a general one.
    pub fn evil_form(npc_type: u16, crimson: bool) -> Option<u16> {
        match npc_type {
            46 | 303 | 337 | 443 | 540 => Some(if crimson { 464 } else { 47 }),
            55 | 230 | 592 | 593 => Some(if crimson { 465 } else { 57 }),
            148 | 149 => Some(if crimson { 470 } else { 168 }),
            _ => None,
        }
    }

    /// Turn evil where it stands, if this is a critter with an evil form. Returns whether it did.
    ///
    /// `NPC.UpdateNPC_BloodMoonTransformations` (`NPC.cs:93033-93048`) is the caller that matters:
    /// under a blood moon the server runs this over every NPC, every tick, so the world's bunnies
    /// and goldfish turn as the moon comes up rather than being spawned already evil.
    ///
    /// The coin guard is vanilla's and is transcribed rather than reasoned about: it records
    /// whether `value` was zero *before* the transform and forces it back to zero afterwards.
    /// `Transform` goes through `SetDefaults`, which loads the new type's own coin value, so
    /// without this a statue-spawned critter (worth nothing, exactly so it cannot be farmed)
    /// becomes worth its evil form's coins the moment a blood moon rises.
    pub fn attempt_to_convert_to_evil(&mut self, crimson: bool) -> bool {
        let Some(evil) = Self::evil_form(self.npc_type, crimson) else {
            return false;
        };
        let was_worthless = self.stats.value == 0.0;
        self.become_type(evil);
        if was_worthless {
            self.stats.value = 0.0;
        }
        true
    }

    /// Turn into another type in place, the way the game's `NPC.Transform` does.
    ///
    /// The slot, the position and the generation all survive; everything the type decides — stats,
    /// size, routine — is replaced, and the AI state is cleared so the new routine starts fresh.
    pub fn become_type(&mut self, npc_type: u16) {
        let Some(stats) = npc_stats(npc_type) else {
            return;
        };
        let scale = terrustia_proto::npc_params::npc_scale(npc_type);
        let stats = scaled(stats, scale);
        self.npc_type = npc_type;
        // ...and the net id with it. `NPC.Transform` goes through `SetDefaults(newType)`, which
        // writes `netID = Type` for a positive id (`NPC.cs:8363`), so a transformed NPC stops
        // being whatever variant it was. Leaving this behind sends the *old* type to every client
        // - a freed bound townsperson arriving as the bound one, because `SyncNpc::npc_type()`
        // resolves the net id and not the type.
        self.net_id = npc_type as i16;
        self.stats = stats;
        self.life_max = stats.life_max;
        self.life = stats.life_max;
        self.no_gravity = stats.no_gravity;
        self.no_tile_collide = stats.no_tile_collide;
        self.defense = stats.defense;
        self.damage_bonus = 1.0;
        self.knockback_immune = false;
        self.size = None;
        self.invulnerable = stats.dont_take_damage;
        self.alpha = 0;
        self.scale = scale;
        self.ai = [0.0; 4];
        self.local_ai = [0.0; 4];
        self.was_hurt = false;
        self.dirty = true;
        // The table's raw values have just been written back over the scaled ones, so the world's
        // difficulty has to be applied again. `NPC.Transform` goes through `SetDefaults`, which
        // calls `ScaleStats` for exactly this reason — without it a slime that changes form on an
        // expert world quietly reverts to classic strength.
        self.scale_stats(self.scaling);
    }

    /// Apply the world's difficulty and player count, the way `NPC.ScaleStats` does.
    ///
    /// Called at spawn and again on any transform, because [`Self::become_type`] writes the raw
    /// table values back over the scaled ones. Without it an expert world fielded classic enemies
    /// and a crowded server fought the same boss a lone player would.
    pub fn scale_stats(&mut self, scaling: Scaling) {
        self.scaling = scaling;
        use terrustia_proto::difficulty;

        // `NPC.cs:18180`, the eligibility gate the whole method sits inside:
        //
        // ```csharp
        // if (NPCID.Sets.NeedsExpertScaling[type]
        //     || (lifeMax > 5 && damage != 0 && !friendly && !townNPC)) { ... }
        // ```
        //
        // There was no gate at all here, so town NPCs were scaled with everything else: the Guide
        // had 500 HP on expert and 750 on master against vanilla's flat 250. `tick_town_casualties`
        // reads those numbers, so townsfolk were surviving blood moons and invasions that should
        // have killed them.
        if !NEEDS_EXPERT_SCALING.contains(&self.npc_type)
            && !(self.life_max > 5
                && self.stats.damage != 0
                && !self.stats.friendly
                && !self.stats.town_npc)
        {
            return;
        }

        // `NPC.cs:18183-18184`: expert hardmode scaling runs *before* the difficulty curves, on the
        // raw table values.
        if scaling.difficulty >= EXPERT && scaling.hard_mode {
            self.scale_stats_for_expert_hardmode(scaling.downed_plant_boss);
        }

        let life = difficulty::life_multiplier(scaling.difficulty);
        let damage = difficulty::damage_multiplier(scaling.difficulty);
        let money = difficulty::money_multiplier(scaling.difficulty);

        self.life_max = (self.life_max as f32 * life) as i32;
        self.stats.damage = (self.stats.damage as f32 * damage) as i32;
        self.stats.value *= money;

        // `NPC.ScaleStats_ByDifficulty` (`NPC.cs:18211`) scales knockback resistance too, so a
        // harder world's enemies stagger less: classic leaves it, expert x0.9, master x0.8. It runs
        // on the raw table value the same as damage, and re-runs on transform, so it stays
        // idempotent. The per-player Brain-of-Cthulhu and Creeper overrides (`NPC.cs:29457-29461`)
        // are set inside that fight's own AI and belong with the boss lane, not here.
        self.stats.knockback_resist *=
            difficulty::knockback_to_enemies_multiplier(scaling.difficulty);

        // Bosses, and only bosses, also scale with how many people are fighting them. The game
        // lists them out rather than deriving it from a `boss` flag, because several boss *parts*
        // scale and a few flagged types do not.
        //
        // And only from expert upward: `NPC.cs:18186-18189` gates the whole
        // `ScaleStats_ByPlayerCount` call on `difficulty >= GameDifficultyLevel.Expert`, so classic
        // (1.0) and journey (0.5) never reach it. This was ungated, so a four-player classic world
        // was fighting a 2.63x-HP Eye of Cthulhu that vanilla would have kept at its solo strength.
        if scaling.difficulty >= EXPERT
            && BOSS_SCALES_WITH_PLAYERS
                .iter()
                .any(|range| range.contains(&self.npc_type))
        {
            let balance = difficulty::balance(scaling.players);
            self.life_max = (f64::from(self.life_max) * f64::from(balance)).round() as i32;
        }

        // The game's own floor, so nothing ends up unkillably cheap on journey. `NPC.cs:18191`
        // exempts the `ProjectileNPC` types from it, which are the ones with a deliberately tiny
        // life total because they are a shot rather than a creature.
        if !PROJECTILE_NPC.contains(&self.npc_type) {
            self.life_max = self.life_max.max(6);
        }
        self.life = self.life_max;
    }

    /// `NPC.ScaleStats_ForExpertHardmode` (`NPC.cs:18545-18593`), which was never transcribed.
    ///
    /// It props up anything that was designed for pre-hardmode once the wall has fallen on an
    /// expert or master world, so a Zombie on the hardmode surface is not free. Without it every
    /// pre-hardmode leftover kept classic stats through the whole back half of the game.
    ///
    /// Two details that are easy to get wrong and are load-bearing:
    ///
    /// * `num2 / num` is **integer** division in the game (`int / int`, assigned to a `float`), so
    ///   the factor is 80/57 = 1, not 1.4. It only starts biting where `num` is genuinely small.
    /// * the `ProjectileNPC` types take the damage scaling and nothing else, because their life,
    ///   defence and value are not what they are for.
    ///
    /// Not transcribed: the three `getGoodWorld` exemptions (`NPC.cs:18549-18563`, which turn the
    /// scaling off for the Eater of Souls, Fire Imp and Dark Caster while their "for the worthy"
    /// counterpart is alive), since this server does not field that seed's paired bosses.
    fn scale_stats_for_expert_hardmode(&mut self, downed_plant_boss: bool) {
        // `NPC.cs:18547`, `:18568`.
        if DONT_DO_HARDMODE_SCALING.contains(&self.npc_type)
            || self.stats.boss
            || self.life_max >= 1000
        {
            return;
        }
        let projectile = PROJECTILE_NPC.contains(&self.npc_type);

        // `NPC.cs:18575-18583`: a single "how tough is it already" number, against a bar that
        // rises once Plantera is down.
        let mut strength = self.stats.damage + self.defense + self.life_max / 4;
        if strength == 0 {
            strength = 1;
        }
        let bar = if downed_plant_boss { 100 } else { 80 };
        if strength >= bar {
            return;
        }

        // Integer division, exactly as the game does it before widening to a float.
        let factor = (bar / strength) as f32;
        self.stats.damage = (self.stats.damage as f32 * factor * 0.9) as i32;
        if !projectile {
            self.defense = (self.defense as f32 * factor) as i32;
            self.life_max = ((self.life_max as f32 * factor) as f64 * 1.1) as i32;
            self.stats.value = ((self.stats.value * factor) as f64 * 0.8) as f32;
        }
    }

    pub fn is_alive(&self) -> bool {
        self.life > 0
    }

    /// What touching this one costs right now, the live equivalent of vanilla's `NPC.damage`.
    ///
    /// Vanilla keeps two numbers: `defDamage`, fixed at spawn once `ScaleStats` has applied the
    /// world's difficulty (`NPC.cs:18197`), and `damage`, which an AI routine is free to move for
    /// the phase it is in (`defense *= 2; damage *= 2;` for the Prime's spin, `NPC.cs:27938-27939`).
    /// Contact reads the moved one (`Player.cs:31623` takes `Main.npc[i].damage` straight). Here
    /// `stats.damage` is the fixed one and [`Self::damage_bonus`] the routine's multiplier, so this
    /// is the product, truncated the way every one of vanilla's own `damage = (int)(defDamage * x)`
    /// writes truncate.
    pub fn contact_damage(&self) -> i32 {
        (self.stats.damage as f32 * self.damage_bonus) as i32
    }

    /// Set [`Self::damage_bonus`] from a plain vanilla-normal damage number.
    ///
    /// Some routines name an absolute figure rather than a multiple: Plantera's second form "hits
    /// for 70" (`NPC.cs:32209`). Vanilla writes those through `GetAttackDamage_ScaledByDifficulty`
    /// (`NPC.cs:7063`), which applies the difficulty multiplier itself, so the figure is a
    /// pre-scaling one. `stats.damage` has already been scaled once, so the bonus has to be measured
    /// against the table's raw damage: dividing by the scaled value would cancel the world's
    /// difficulty straight back out and leave an expert Plantera hitting for a classic 70.
    pub fn set_contact_damage(&mut self, normal_damage: i32) {
        let base = npc_stats(self.npc_type).map_or(1, |s| s.damage).max(1);
        self.damage_bonus = normal_damage as f32 / base as f32;
    }

    /// The per-player stat-scaling factor this NPC was scaled for, `NPC.GetMyBalance`
    /// (`NPC.cs:18518-18526`): one for a lone player, climbing with the crowd it spawned against
    /// (`GetStatScalingFactors`). Bosses read it to speed their fights up on a busy server; a
    /// count of one (or none) is a flat one, as vanilla's own early return gives.
    pub fn balance(&self) -> f32 {
        terrustia_proto::difficulty::balance(self.scaling.players)
    }

    /// Apply a non-critical hit, returning true if it killed the NPC.
    ///
    /// Most hits are not crits (a town blow, a contact tick, an invulnerability probe), so this is
    /// the ordinary door in. [`Self::strike`] is the same thing with the crit flag exposed.
    pub fn take_damage(&mut self, amount: i32, knockback: f32, direction: i8) -> bool {
        self.strike(amount, knockback, direction, false)
    }

    /// Apply a hit, returning true if it killed the NPC.
    ///
    /// The knockback is ported whole from `NPC.StrikeNPC_Inner` (`NPC.cs:82216-82311`). The old
    /// code applied resist once, had no diminishing curve, cap or crit, and *added* to velocity
    /// with no bound, so a rapid weapon accelerated an enemy without limit rather than shoving it a
    /// fixed distance.
    pub fn strike(&mut self, amount: i32, knockback: f32, direction: i8, crit: bool) -> bool {
        // One flag, as vanilla has one flag (`NPC.dontTakeDamage`). See [`Self::invulnerable`]:
        // asking the type's own seed here as well is what made three bosses unkillable.
        if self.invulnerable {
            return false;
        }
        let num = amount.max(0);
        self.life -= num;
        self.was_hurt = true;
        self.dirty = true;

        // knockback_resist is a multiplier: 0 means immovable, 1 fully affected. A routine can
        // override it outright while it is committed to a move. The game applies it twice: once
        // building the raw push, once again when it assigns the push to velocity.
        let resist = if self.knockback_immune {
            0.0
        } else {
            self.stats.knockback_resist
        };
        if knockback > 0.0 && resist > 0.0 {
            let mut num3 = knockback * resist;
            // The "On Fire!" 3.0 debuff makes a hit stagger a little harder.
            if self.buffs.flags.on_fire2 {
                num3 *= 1.1;
            }
            // The diminishing ladder (`NPC.cs:82223-82250`): every band past a threshold counts for
            // progressively less, and the whole thing is capped at sixteen.
            if num3 > 8.0 {
                num3 = 8.0 + (num3 - 8.0) * 0.9;
            }
            if num3 > 10.0 {
                num3 = 10.0 + (num3 - 10.0) * 0.8;
            }
            if num3 > 12.0 {
                num3 = 12.0 + (num3 - 12.0) * 0.7;
            }
            if num3 > 14.0 {
                num3 = 14.0 + (num3 - 14.0) * 0.6;
            }
            if num3 > 16.0 {
                num3 = 16.0;
            }
            if crit {
                num3 *= 1.4;
            }

            // A hit worth more than a tenth of max life (a fifteenth in expert and up) shoves hard,
            // but *additively* and clamped so a run of big hits builds towards the push without ever
            // overshooting it. Any smaller hit assigns the push outright.
            let expert = self.scaling.difficulty >= 2.0;
            let big_hit = if expert { num * 15 } else { num * 10 } > self.life_max;
            let dir = f32::from(direction);
            if big_hit {
                if direction < 0 && self.velocity.0 > -num3 {
                    if self.velocity.0 > 0.0 {
                        self.velocity.0 -= num3;
                    }
                    self.velocity.0 -= num3;
                    if self.velocity.0 < -num3 {
                        self.velocity.0 = -num3;
                    }
                } else if direction > 0 && self.velocity.0 < num3 {
                    if self.velocity.0 < 0.0 {
                        self.velocity.0 += num3;
                    }
                    self.velocity.0 += num3;
                    if self.velocity.0 > num3 {
                        self.velocity.0 = num3;
                    }
                }
                // The game's own special case for type 185 (`NPC.cs:82286`).
                if self.npc_type == 185 {
                    num3 *= 1.5;
                }
                num3 = if self.no_gravity {
                    num3 * -0.5
                } else {
                    num3 * -0.75
                };
                if self.velocity.1 > num3 {
                    self.velocity.1 += num3;
                    if self.velocity.1 < num3 {
                        self.velocity.1 = num3;
                    }
                }
            } else {
                self.velocity.1 = if self.no_gravity {
                    -num3 * 0.5 * resist
                } else {
                    -num3 * 0.75 * resist
                };
                self.velocity.0 = num3 * dir * resist;
            }
        }
        self.life <= 0
    }
}

/// Whether the box at `(left, top)` overlaps anything that blocks movement.
///
/// Platforms are skipped here: they are in the solid set but only stop something landing on them
/// from above, which [`move_vertical`] handles separately.
/// Whether a box overlaps anything solid, for an NPC that may pass through some tiles.
///
/// `npc_type` decides what counts as solid: almost every NPC is stopped by everything, but a sand
/// shark swims through sand, so the type has to be part of the question.
fn blocked_for(
    tiles: &impl TileView,
    npc_type: u16,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
) -> bool {
    let x0 = (left / TILE).floor() as i32;
    let x1 = ((left + width - 1.0) / TILE).floor() as i32;
    let y0 = (top / TILE).floor() as i32;
    let y1 = ((top + height - 1.0) / TILE).floor() as i32;

    for y in y0..=y1 {
        for x in x0..=x1 {
            let tile = tiles.tile(x, y);
            if tile.is_active()
                && solid(tile.block)
                && !solid_top(tile.block)
                && !terrustia_proto::npc_params::phases_through(npc_type, tile.block)
            {
                return true;
            }
        }
    }
    false
}

/// Whether a platform's top edge sits within the span an NPC would fall through this step.
fn platform_underfoot(
    tiles: &impl TileView,
    left: f32,
    feet_from: f32,
    feet_to: f32,
    width: f32,
) -> Option<f32> {
    let x0 = (left / TILE).floor() as i32;
    let x1 = ((left + width - 1.0) / TILE).floor() as i32;
    let y0 = (feet_from / TILE).floor() as i32;
    let y1 = (feet_to / TILE).floor() as i32;

    for y in y0..=y1 {
        let top = y as f32 * TILE;
        if top < feet_from - 0.01 || top > feet_to {
            continue;
        }
        for x in x0..=x1 {
            let tile = tiles.tile(x, y);
            if tile.is_active() && solid_top(tile.block) {
                return Some(top);
            }
        }
    }
    None
}

/// Move horizontally, stopping at the first wall.
fn move_horizontal(npc: &mut Npc, tiles: &impl TileView) {
    let next = npc.position.0 + npc.velocity.0;
    if !blocked_for(
        tiles,
        npc.npc_type,
        next,
        npc.position.1,
        npc.width(),
        npc.height(),
    ) {
        npc.position.0 = next;
        return;
    }
    npc.collide_x = true;

    // Step up to the wall a pixel at a time so the NPC ends flush against it rather than short of
    // it, which is what lets the fighter AI decide it is time to jump.
    let step = npc.velocity.0.signum();
    while !blocked_for(
        tiles,
        npc.npc_type,
        npc.position.0 + step,
        npc.position.1,
        npc.width(),
        npc.height(),
    ) {
        npc.position.0 += step;
        if (npc.position.0 - next).abs() < 1.0 {
            break;
        }
    }
    npc.velocity.0 = 0.0;
}

/// Move vertically, landing on solid ground or on a platform fallen onto from above.
fn move_vertical(npc: &mut Npc, tiles: &impl TileView) {
    npc.on_ground = false;
    let next = npc.position.1 + npc.velocity.1;

    if npc.velocity.1 > 0.0 {
        let feet_from = npc.position.1 + npc.height();
        let feet_to = next + npc.height();
        if let Some(top) =
            platform_underfoot(tiles, npc.position.0, feet_from, feet_to, npc.width())
        {
            npc.position.1 = top - npc.height();
            npc.velocity.1 = 0.0;
            npc.on_ground = true;
            return;
        }
    }

    if !blocked_for(
        tiles,
        npc.npc_type,
        npc.position.0,
        next,
        npc.width(),
        npc.height(),
    ) {
        npc.position.1 = next;
        return;
    }
    npc.collide_y = true;

    // Whether the NPC is already inside solid ground *before* this tick's velocity is even
    // applied — a tile placed under or through it by a player building nearby, not a collision its
    // own motion is approaching. A real player watching a real Guide's house get built around him
    // found exactly this: an embedded, `on_ground: false` NPC that could never move again, in any
    // direction, for the rest of its existence.
    //
    // The loop below (`while !blocked_for(position + step) { position += step; ... }`) exists to
    // creep an NPC from open air toward a *new* collision one pixel at a time, stopping the moment
    // the very next pixel would be blocked — which assumes the *current* position is already
    // clear. For an embedded NPC that assumption is false everywhere nearby: shifting by a single
    // pixel essentially never crosses a whole `TILE`-sized tile boundary, so `blocked_for` at
    // `position + step` reports exactly as blocked as `position` itself did, the loop's own guard
    // never lets it take even one step, and gravity making `velocity.1` strictly positive on every
    // tick an NPC is not `on_ground` means this is not a one-tick problem either — the very same
    // stuck check repeats forever on every following tick too.
    //
    // Climbing out needs an unconditional, un-gated push instead — not "is the next pixel already
    // clear," just "move up, tick after tick, until it is." `ESCAPE_SPEED` is deliberately modest:
    // fast enough to clear a person-sized NPC in well under a second, slow enough that escaping a
    // deep burial reads as digging free rather than teleporting.
    if blocked_for(
        tiles,
        npc.npc_type,
        npc.position.0,
        npc.position.1,
        npc.width(),
        npc.height(),
    ) {
        const ESCAPE_SPEED: f32 = 2.0;
        npc.position.1 -= ESCAPE_SPEED;
        npc.velocity.1 = 0.0;
        return;
    }

    let step = npc.velocity.1.signum();
    while !blocked_for(
        tiles,
        npc.npc_type,
        npc.position.0,
        npc.position.1 + step,
        npc.width(),
        npc.height(),
    ) {
        npc.position.1 += step;
        if (npc.position.1 - next).abs() < 1.0 {
            break;
        }
    }
    if npc.velocity.1 > 0.0 {
        npc.on_ground = true;
    }
    npc.velocity.1 = 0.0;
}

/// Which liquid an NPC is standing in, if any.
///
/// The centre tile, which is the same approximation of vanilla's `Collision.WetCollision` over the
/// whole hitbox that [`super::npc_ai`] already hands the AI routines as `World::wet`. It is read
/// here too, so a routine and the physics under it never disagree about being in water.
pub fn liquid_at(
    tiles: &impl TileView,
    point: (f32, f32),
) -> Option<terrustia_proto::tile::Liquid> {
    let tile = tiles.tile(
        (point.0 / TILE).floor() as i32,
        (point.1 / TILE).floor() as i32,
    );
    (tile.liquid > 0).then_some(tile.liquid_kind)
}

/// Advance an NPC's position by one tick, applying gravity and collision.
pub fn step_physics(npc: &mut Npc, tiles: &impl TileView) {
    npc.old_position = npc.position;
    npc.old_velocity = npc.velocity;
    npc.collide_x = false;
    npc.collide_y = false;
    if !npc.no_gravity {
        // `UpdateNPC_UpdateGravity` runs before the routine every tick and decides both numbers
        // from what the NPC is standing in (`NPC.cs:92054-92071`); the pair is then applied as
        // `velocity.Y += gravity; if (velocity.Y > maxFallSpeed) velocity.Y = maxFallSpeed;`
        // (`NPC.cs:91581-91586`), which is this line.
        let (gravity, max_fall) = match liquid_at(tiles, npc.center()) {
            None => (GRAVITY, MAX_FALL_SPEED),
            Some(terrustia_proto::tile::Liquid::Shimmer) => {
                (SHIMMER_GRAVITY, SHIMMER_MAX_FALL_SPEED)
            }
            Some(terrustia_proto::tile::Liquid::Honey) => (HONEY_GRAVITY, HONEY_MAX_FALL_SPEED),
            Some(_) => (WET_GRAVITY, WET_MAX_FALL_SPEED),
        };
        npc.velocity.1 = (npc.velocity.1 + gravity).min(max_fall);
    }

    if npc.no_tile_collide {
        npc.position.0 += npc.velocity.0;
        npc.position.1 += npc.velocity.1;
        npc.on_ground = false;
        return;
    }

    move_horizontal(npc, tiles);
    move_vertical(npc, tiles);

    if npc.npc_type == SOLAR_SROLLER {
        collision_move_solar_sroller(npc);
    }
}

/// `Collision_MoveSolarSroller`, `NPC.cs:93879-93900`.
///
/// Curled and mid-bounce (`ai[0] == 6`, the roller's `phase::BOUNCING`), a wall or floor hit costs
/// the sroller one of the two-to-four bounces its wind-up rolled for it (`ai[2]`, `NPC.cs:29608-
/// 29609`) and sends that axis back the other way at nine tenths the speed it hit with, flipping
/// direction on an X bounce so it does not just wedge itself into the wall again next tick. Once
/// `ai[2]` reaches zero the bouncing loop in `roller()` (its own `ai[2] == 0.0` check, matching
/// `NPC.cs:29818`) stands the sroller back up instead of waiting out the 1200-tick safety timer.
///
/// Vanilla decides "did this axis just hit something" off the sub-pixel residual
/// `Collision.TileCollision` leaves behind (`velocity.X != 0f && velocity.X != oldVelocity.X`): a
/// swept collision resolves to the exact remaining distance to the wall face, not to zero. This
/// engine's own tile step above (`move_horizontal`/`move_vertical`) resolves a blocked axis to
/// precisely zero instead, so `collide_x`/`collide_y` (the same "this axis's velocity changed
/// because of terrain" fact vanilla itself records a few lines later as `collideX`/`collideY`,
/// `Collision_MoveWhileDry`, `NPC.cs:93734-93751`) stand in for the nonzero-residual check without
/// changing what triggers a bounce.
fn collision_move_solar_sroller(npc: &mut Npc) {
    if npc.ai[0] != 6.0 || !(npc.collide_x || npc.collide_y) {
        return;
    }
    npc.ai[2] -= 1.0;
    // `NPC.cs`'s `ai[3] = 1f` here only starts a client-side dust and gore burst; this server does
    // not render either, but the flag is still part of the synced `ai` array a real client watches
    // for that cue, so it is set for parity even though nothing here reads it back.
    npc.ai[3] = 1.0;
    if npc.ai[2] > 0.0 {
        if npc.collide_x {
            npc.velocity.0 = -npc.old_velocity.0 * 0.9;
            npc.direction = -npc.direction;
        }
        if npc.collide_y {
            npc.velocity.1 = -npc.old_velocity.1 * 0.9;
        }
    }
}

/// `Main.Difficulty` at expert, the threshold every `difficulty >= GameDifficultyLevel.Expert`
/// test in `ScaleStats` compares against (`NPC.cs:18183`, `:18186`).
const EXPERT: f32 = 2.0;

/// `NPCID.Sets.NeedsExpertScaling` (`NPCID.cs:4803`): types that go through `ScaleStats` even
/// though they fail the ordinary "has life, has damage, is not friendly" test. Mostly the
/// projectile-shaped NPCs and the Moon Lord's parts.
const NEEDS_EXPERT_SCALING: [u16; 15] = [
    25, 30, 665, 33, 112, 666, 261, 265, 371, 516, 519, 397, 396, 398, 491,
];

/// `NPCID.Sets.ProjectileNPC` (`NPCID.cs:4805`): NPCs that are really a shot. Their life, defence
/// and value are skipped by both the hardmode prop-up and the minimum-life floor, because those
/// numbers are not what they are for.
const PROJECTILE_NPC: [u16; 11] = [25, 30, 665, 33, 112, 666, 261, 265, 371, 516, 519];

/// `NPCID.Sets.DontDoHardmodeScaling` (`NPCID.cs:4442`): types the expert-hardmode prop-up skips
/// outright. The worm segments and the Wall of Flesh's parts are here because their head already
/// carries the fight's difficulty.
const DONT_DO_HARDMODE_SCALING: [u16; 17] = [
    5, 13, 14, 15, 267, 113, 114, 115, 116, 117, 118, 119, 658, 659, 660, 400, 522,
];

/// The types whose life grows with the number of players, from `NPC.ScaleStats_ByPlayerCount`.
///
/// Written as ranges because that is how the game writes it, and because the boss *parts* — the
/// Destroyer's body, Golem's fists, the lunar pillars — have to scale with their head or a fight
/// ends up with a healthy boss and paper limbs.
const BOSS_SCALES_WITH_PLAYERS: &[std::ops::RangeInclusive<u16>] = &[
    4..=4,     // Eye of Cthulhu
    13..=15,   // Eater of Worlds, all segments
    35..=36,   // Skeletron and its hands
    50..=50,   // King Slime
    113..=116, // Wall of Flesh and its eyes
    125..=126, // The Twins
    127..=131, // Skeletron Prime and its limbs
    134..=136, // The Destroyer
    139..=139,
    222..=222, // Queen Bee
    245..=249, // Golem
    262..=264, // Plantera
    266..=267, // Brain of Cthulhu and its creepers
    370..=370, // Duke Fishron
    396..=398, // Moon Lord
    439..=440, // Lunatic Cultist
    454..=459, // the lunar pillars
    471..=472, // Martian Saucer
    523..=523,
    551..=551, // Betsy
    636..=636, // Empress of Light
    657..=660, // Queen Slime
    668..=668, // Deerclops
];

/// The fixed table of NPC slots.
#[derive(Debug)]
pub struct NpcStore {
    slots: Vec<Option<Npc>>,
    /// The generation the NPC most recently in each slot carried.
    ///
    /// Per slot, not one counter for the whole store, because that is what the game does:
    /// `NewNPCInstanceInSlot` reads `Main.npc[slot].generation`, adds one, and skips zero. The
    /// number exists so a client can tell "slot 5, the one I was told about" from "slot 5, someone
    /// else now" — `NPC.Equals` compares slot *and* generation — so what matters is how long it
    /// takes to come back around to a value a client still remembers.
    ///
    /// A single global counter, which this used to be, wraps after 256 spawns *anywhere*: minutes
    /// on a busy server, and every stale reference in play becomes ambiguous at once. Per slot it
    /// takes 256 reuses *of that slot*.
    last_generation: Vec<u8>,
    /// The world's difficulty and how many people are in it.
    ///
    /// Kept here rather than passed to every `spawn` call because there are eighteen of those and
    /// one of them forgetting is exactly how enemies came to be unscaled in the first place. One
    /// choke point cannot be half-applied.
    scaling: Scaling,
}

/// What every newly spawned NPC is scaled by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scaling {
    /// `Main.Difficulty`: 1 classic, 2 expert, 3 master, 0.5 journey.
    pub difficulty: f32,
    /// Players currently in the world, for the boss life curve.
    pub players: u32,
    /// `Main.hardMode`, which with expert turns on `ScaleStats_ForExpertHardmode`
    /// (`NPC.cs:18183`).
    pub hard_mode: bool,
    /// `NPC.downedPlantBoss`, which raises that prop-up's bar from 80 to 100 (`NPC.cs:18581`).
    pub downed_plant_boss: bool,
}

impl Default for Scaling {
    fn default() -> Self {
        Self {
            difficulty: 1.0,
            players: 1,
            hard_mode: false,
            downed_plant_boss: false,
        }
    }
}

impl Default for NpcStore {
    fn default() -> Self {
        Self::new()
    }
}

/// The Water Bolt Mimic (`NPCID.WaterBoltMimic`), which enters the world asleep.
///
/// Its one spawn in the whole game passes `ai3: 3f` (`NPC.cs:2754`, the dungeon library's own arm,
/// and the only `SpawnNPC`/`NewNPC` of 694 anywhere in the source). That value is the dormant state
/// its routine reads (`NPC.cs:21666-21690`, `game::ai::skull`): it sits perfectly still, takes no
/// notice of anybody, and only wakes and hunts once something has hit it.
///
/// Seeded here rather than at the spawner because this is the one door every NPC in this server
/// comes through, which is also where vanilla's own `ai3` argument lands (`NewNPC`, called from
/// `SpawnNPC` at `NPC.cs:5263`).
const WATER_BOLT_MIMIC: u16 = 694;
const MIMIC_DORMANT: f32 = 3.0;

/// The three fairy critters, `NPCID.FairyCritterPink` through `FairyCritterBlue`, which enter the
/// world already coming for whoever dug them up.
///
/// The underground fairy's one spawn passes `ai2: 2f` (`NPC.cs:3623`, the `CheckToSpawnUndergroundFairy`
/// arm), which is the fairy routine's `state::APPROACH` (`game::ai::fairy`). Left at zero it would
/// be `state::IDLE`, drifting about the cave it appeared in until somebody wandered within two
/// hundred and fifty pixels of it, and `NPC.AnyHelpfulFairies` (`NPC.cs:90954-90964`, `ai[2] > 1f`)
/// would never see it either, so the spawner's own "not while one is already out" gate could not
/// hold.
///
/// Seeded here for the same reason the mimic above is. The one caveat, recorded rather than
/// guarded: vanilla's *other* producer, `MysticLogFairiesEvent.TrySpawningFairies`
/// (`MysticLogFairiesEvent.cs:118`), spawns its surface fairies with a plain `NewNPC` and so leaves
/// them idle. This server has no analog of that event, so nothing here wants the idle form; a lane
/// that adds it should clear `ai[2]` on its own spawns rather than widen this.
const FAIRY_CRITTERS: std::ops::RangeInclusive<u16> = 583..=585;
const FAIRY_APPROACHING: f32 = 2.0;

impl NpcStore {
    pub fn new() -> Self {
        Self {
            slots: (0..MAX_NPCS).map(|_| None).collect(),
            last_generation: vec![0; MAX_NPCS],
            scaling: Scaling::default(),
        }
    }

    /// Tell the store what the world is like. Applied to everything spawned afterwards.
    pub fn set_scaling(&mut self, scaling: Scaling) {
        self.scaling = scaling;
    }

    /// Spawn a worm: a head, a run of body segments and a tail, each linked to the one ahead.
    ///
    /// Returns the head's slot. Worms are the reason NPC slots are addressed by index on the wire:
    /// the segments have to refer to each other.
    pub fn spawn_worm(
        &mut self,
        head: u16,
        body: u16,
        tail: u16,
        segments: usize,
        position: (f32, f32),
    ) -> Option<u8> {
        let head_index = self.spawn(head, position)?;
        self.grow_worm_body(head_index, body, tail, segments, position);
        Some(head_index)
    }

    /// Attach a run of body segments and a tail to a head that already exists, each linked to the
    /// one ahead — [`Self::spawn_worm`]'s own segment loop, reusable for a head that grows its own
    /// body on its own first AI tick rather than all at once at creation (`NPC.cs:51913-51936`'s
    /// own `type == 412 && ai[0] == 0f` gate, the Solar Crawltipede's real mechanism: `dontTakeDamage`
    /// is set on the head by design — it is genuinely not the target — so a head with no body
    /// attached is not merely incomplete, it is unkillable).
    pub fn grow_worm_body(
        &mut self,
        head_index: u8,
        body: u16,
        tail: u16,
        segments: usize,
        position: (f32, f32),
    ) {
        self.grow_worm_chain(
            head_index,
            (0..segments).map(|i| if i + 1 == segments { tail } else { body }),
            position,
        );
    }

    /// The same, for a worm whose parts are not one body type and a tail.
    ///
    /// The Wyvern's fourteen are legs, bodies and three distinct tail pieces in a fixed order
    /// (`npc_params::WYVERN_SEGMENTS`), so it needs the list rather than a count.
    pub fn grow_worm_chain(
        &mut self,
        head_index: u8,
        parts: impl IntoIterator<Item = u16>,
        position: (f32, f32),
    ) {
        let mut previous = head_index;
        for part in parts {
            let Some(index) = self.spawn(part, position) else {
                break;
            };
            if let Some(npc) = self.get_mut(index) {
                npc.follows = Some(previous);
            }
            previous = index;
        }
    }

    /// Spawn a *variant* by its negative net id — `NPC.SetDefaultsFromNetId`
    /// (`NPC.cs:7681-7684`), which resolves the base type through `NetIdMap`, builds it, and then
    /// applies the variant's own overrides.
    ///
    /// A positive id is just [`Self::spawn`] of that type, which is what a caller holding a raw
    /// `netID` wants and what vanilla's own `SetDefaults` does with one.
    ///
    /// The order is vanilla's and it matters: the base type is built and *then* scaled for world
    /// difficulty, and only then does the variant override. A Pinky's 150 life is 150 in classic
    /// and in master alike, because `SetDefaultsFromNetId` writes it after `SetDefaults` has
    /// already done its own scaling - which is why this cannot simply be a second scaling pass.
    pub fn spawn_net_id(&mut self, net_id: i16, position: (f32, f32)) -> Option<u8> {
        let base = terrustia_proto::npc_data::from_net_id(net_id);
        let index = self.spawn(base, position)?;
        let Some(variant) = terrustia_proto::net_variants::net_variant(net_id) else {
            return Some(index);
        };
        let npc = self.get_mut(index)?;
        npc.net_id = net_id;
        if let Some(scale) = variant.scale {
            npc.scale *= scale;
        }
        if let Some(damage) = variant.damage {
            npc.stats.damage = damage;
        }
        if let Some(defense) = variant.defense {
            npc.defense = defense;
        }
        if let Some(life) = variant.life {
            npc.life = life;
            npc.life_max = life;
        }
        if let Some(mul) = variant.knockback_mul {
            npc.stats.knockback_resist *= mul;
        }
        Some(index)
    }

    pub fn spawn(&mut self, npc_type: u16, position: (f32, f32)) -> Option<u8> {
        let index = self.slots.iter().position(Option::is_none)?;
        let generation = self.next_generation_for(index);
        let mut npc = Npc::new(npc_type, position, generation)?;
        npc.scale_stats(self.scaling);
        if npc_type == WATER_BOLT_MIMIC {
            npc.ai[3] = MIMIC_DORMANT;
        }
        if FAIRY_CRITTERS.contains(&npc_type) {
            npc.ai[2] = FAIRY_APPROACHING;
        }
        self.slots[index] = Some(npc);
        u8::try_from(index).ok()
    }

    /// The generation for the next NPC to occupy a slot.
    ///
    /// One more than whatever was there before, skipping zero on the way round. Zero is the game's
    /// "no generation": a real client asserts on receiving it
    /// (`Invariant.Assert(generation != 0)`), so a server that ever hands one out is a server that
    /// eventually breaks a client outright rather than merely confusing it.
    fn next_generation_for(&mut self, index: usize) -> u8 {
        let Some(slot) = self.last_generation.get_mut(index) else {
            return 1;
        };
        *slot = slot.wrapping_add(1);
        if *slot == 0 {
            *slot = 1;
        }
        *slot
    }

    pub fn get(&self, index: u8) -> Option<&Npc> {
        self.slots.get(usize::from(index))?.as_ref()
    }

    pub fn get_mut(&mut self, index: u8) -> Option<&mut Npc> {
        self.slots.get_mut(usize::from(index))?.as_mut()
    }

    pub fn remove(&mut self, index: u8) -> Option<Npc> {
        self.slots.get_mut(usize::from(index))?.take()
    }

    pub fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = (u8, &Npc)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|npc| (i as u8, npc)))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u8, &mut Npc)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_mut().map(|npc| (i as u8, npc)))
    }

    /// Total spawn slots in use, which is what the spawn cap is measured against.
    pub fn used_slots(&self) -> f32 {
        self.slots
            .iter()
            .flatten()
            // A statue's monster costs nothing against the cap, so a farm does not starve the
            // rest of the world of spawns.
            .filter(|npc| !npc.stats.town_npc && !npc.from_statue)
            .map(|npc| npc.stats.npc_slots)
            .sum()
    }
}

#[cfg(test)]
mod scaling_tests {
    use super::*;

    /// A zombie is tougher in expert than in classic, and tougher again in master.
    ///
    /// Nothing scaled anything before this: `life_max` came straight off the type table, so an
    /// expert world fielded classic enemies. The store applies it at spawn so no call site can
    /// forget.
    #[test]
    fn difficulty_reaches_a_spawned_enemy() {
        const ZOMBIE: u16 = 3;

        let life_at = |game_mode: u8| {
            let mut store = NpcStore::new();
            store.set_scaling(Scaling {
                difficulty: terrustia_proto::difficulty::of_game_mode(game_mode),
                ..Scaling::default()
            });
            let index = store.spawn(ZOMBIE, (0.0, 0.0)).expect("a slot");
            store.get(index).expect("the zombie").life_max
        };

        let classic = life_at(0);
        assert!(classic > 0);
        assert_eq!(life_at(1), classic * 2, "expert doubles it");
        assert_eq!(life_at(2), classic * 3, "master triples it");
        assert!(life_at(3) < classic, "journey is gentler");
    }

    /// An NPC that changes form keeps the world's difficulty.
    ///
    /// Found by auditing the scaling change rather than by a failure: `become_type` writes the raw
    /// table stats back over the scaled ones, so a transform silently reverted an enemy to classic
    /// strength on an expert world. The game avoids it by routing `Transform` through
    /// `SetDefaults`, which scales.
    #[test]
    fn a_transformed_npc_keeps_its_scaling() {
        const ZOMBIE: u16 = 3;
        const SKELETON: u16 = 21;

        let mut store = NpcStore::new();
        store.set_scaling(Scaling {
            difficulty: 3.0, // master
            ..Scaling::default()
        });
        let index = store.spawn(ZOMBIE, (0.0, 0.0)).expect("a slot");

        let plain_skeleton = terrustia_proto::npc_data::npc_stats(SKELETON)
            .expect("skeleton stats")
            .life_max;
        let npc = store.get_mut(index).expect("the zombie");
        npc.become_type(SKELETON);

        assert_eq!(
            npc.life_max,
            plain_skeleton * 3,
            "a transform must not drop back to classic strength",
        );
    }

    /// A boss grows with the crowd, from expert upward; an ordinary enemy never does.
    ///
    /// `NPC.cs:18186-18189` gates `ScaleStats_ByPlayerCount` on
    /// `difficulty >= GameDifficultyLevel.Expert`, so a four-player classic world fights the same
    /// Eye of Cthulhu a lone player does. This was ungated, giving that world a 2.63x-HP boss.
    #[test]
    fn only_bosses_scale_with_player_count_and_only_from_expert_up() {
        const ZOMBIE: u16 = 3;
        const EYE_OF_CTHULHU: u16 = 4;

        let life_at = |npc_type: u16, players: u32, difficulty: f32| {
            let mut store = NpcStore::new();
            store.set_scaling(Scaling {
                difficulty,
                players,
                ..Scaling::default()
            });
            let index = store.spawn(npc_type, (0.0, 0.0)).expect("a slot");
            store.get(index).expect("the npc").life_max
        };

        assert_eq!(
            life_at(ZOMBIE, 1, 2.0),
            life_at(ZOMBIE, 8, 2.0),
            "a zombie is a zombie however many people are watching",
        );
        assert!(
            life_at(EYE_OF_CTHULHU, 8, 2.0) > life_at(EYE_OF_CTHULHU, 1, 2.0),
            "a boss has to grow on expert, or a full server trivialises every fight",
        );
        assert_eq!(
            life_at(EYE_OF_CTHULHU, 1, 1.0),
            life_at(EYE_OF_CTHULHU, 4, 1.0),
            "NPC.cs:18186: classic never calls ScaleStats_ByPlayerCount",
        );
        assert_eq!(
            life_at(EYE_OF_CTHULHU, 1, 0.5),
            life_at(EYE_OF_CTHULHU, 4, 0.5),
            "and neither does journey",
        );
    }

    /// Townsfolk and friendlies never go through `ScaleStats` at all (`NPC.cs:18180`).
    ///
    /// Fails before the fix, which had no eligibility gate: the Guide came out of an expert world
    /// with 500 HP and a master world with 750 against vanilla's flat 250, so `tick_town_casualties`
    /// let townsfolk walk out of blood moons and invasions that should have killed them.
    #[test]
    fn town_npcs_and_friendlies_are_not_scaled_by_difficulty() {
        const GUIDE: u16 = 22;
        const OLD_MAN: u16 = 37;

        for npc_type in [GUIDE, OLD_MAN] {
            let raw = terrustia_proto::npc_data::npc_stats(npc_type).expect("real type");
            for difficulty in [1.0, 2.0, 3.0, 0.5] {
                let mut store = NpcStore::new();
                store.set_scaling(Scaling {
                    difficulty,
                    players: 4,
                    ..Scaling::default()
                });
                let index = store.spawn(npc_type, (0.0, 0.0)).expect("a slot");
                let npc = store.get(index).expect("the npc");
                assert_eq!(
                    npc.life_max, raw.life_max,
                    "{} at difficulty {difficulty} should keep its table life",
                    raw.name,
                );
                assert_eq!(npc.stats.damage, raw.damage, "and its table damage");
            }
        }
    }

    /// `ScaleStats_ForExpertHardmode` (`NPC.cs:18545-18593`) props up pre-hardmode leftovers on an
    /// expert hardmode world, and does nothing at all before hardmode or below expert.
    ///
    /// Fails before the fix: the routine was never transcribed, so a Zombie kept classic-derived
    /// stats through the whole back half of an expert game.
    #[test]
    fn expert_hardmode_props_up_a_pre_hardmode_leftover() {
        const ZOMBIE: u16 = 3;

        let life_at = |difficulty: f32, hard_mode: bool, downed_plant_boss: bool| {
            let mut store = NpcStore::new();
            store.set_scaling(Scaling {
                difficulty,
                players: 1,
                hard_mode,
                downed_plant_boss,
            });
            let index = store.spawn(ZOMBIE, (0.0, 0.0)).expect("a slot");
            let npc = store.get(index).expect("the zombie");
            (npc.life_max, npc.stats.damage, npc.defense)
        };

        // A Zombie is damage 14, defence 6, life 45: `14 + 6 + 45/4 = 31`, under the bar of 80, so
        // the integer factor is `80 / 31 = 2`.
        let raw = terrustia_proto::npc_data::npc_stats(ZOMBIE).expect("zombie stats");
        assert_eq!(raw.damage + raw.defense + raw.life_max / 4, 31);

        let plain_expert = life_at(2.0, false, false);
        let hardmode_expert = life_at(2.0, true, false);
        assert!(
            hardmode_expert.0 > plain_expert.0 && hardmode_expert.1 > plain_expert.1,
            "hardmode expert should be tougher than pre-hardmode expert: {hardmode_expert:?} vs \
             {plain_expert:?}",
        );
        // life: (45 * 2) * 1.1 = 99, then the expert x2 life curve.
        assert_eq!(hardmode_expert.0, 99 * 2);
        // damage: 14 * 2 * 0.9 = 25, then the expert x2 damage curve.
        assert_eq!(hardmode_expert.1, 25 * 2);
        assert_eq!(hardmode_expert.2, raw.defense * 2);

        // Plantera raises the bar to 100, so the factor becomes `100 / 31 = 3`.
        let after_plantera = life_at(2.0, true, true);
        assert_eq!(after_plantera.2, raw.defense * 3, "NPC.cs:18581, +20 bar");

        // Classic hardmode never reaches the routine at all.
        assert_eq!(
            life_at(1.0, true, false),
            life_at(1.0, false, false),
            "NPC.cs:18183 gates the whole routine on expert",
        );
    }

    /// A boss and anything already tough are skipped by the hardmode prop-up
    /// (`NPC.cs:18568`), and so is everything in `DontDoHardmodeScaling` (`NPCID.cs:4442`).
    #[test]
    fn the_hardmode_prop_up_skips_bosses_and_the_named_set() {
        let life_at = |npc_type: u16, hard_mode: bool| {
            let mut store = NpcStore::new();
            store.set_scaling(Scaling {
                difficulty: 2.0,
                players: 1,
                hard_mode,
                downed_plant_boss: false,
            });
            let index = store.spawn(npc_type, (0.0, 0.0)).expect("a slot");
            store.get(index).expect("the npc").life_max
        };

        // 4 is the Eye of Cthulhu (a boss), 13 the Eater of Worlds' head (in the named set).
        for npc_type in [4u16, 13] {
            assert_eq!(
                life_at(npc_type, true),
                life_at(npc_type, false),
                "{npc_type} should be exempt from the hardmode prop-up",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Becoming another type takes the net id with it.
    ///
    /// `NPC.Transform` goes through `SetDefaults(newType)`, which writes `netID = Type`
    /// (`NPC.cs:8363`). Leaving the old net id behind is invisible on the server and wrong on
    /// every client, because packet 23 carries the net id and `SyncNpc::npc_type()` resolves *it*
    /// rather than the type: a freed bound townsperson arrived as the bound one.
    #[test]
    fn a_transformed_npc_stops_being_whatever_variant_it_was() {
        // A Pinky: a Blue Slime by type, -4 by net id.
        let mut npc = Npc::new(1, (0.0, 0.0), 1).expect("a blue slime");
        npc.net_id = -4;
        npc.become_type(2);
        assert_eq!(npc.npc_type, 2);
        assert_eq!(
            npc.net_id, 2,
            "the variant does not survive the transform, and the wire must say so"
        );
    }

    /// Terrain built from a closure, for testing movement without a world.
    struct Terrain<F>(F);

    impl<F: Fn(i32, i32) -> Option<u16>> TileView for Terrain<F> {
        fn tile(&self, x: i32, y: i32) -> Tile {
            match (self.0)(x, y) {
                // Platforms and other multi-tile types are frame-important, so they have to be
                // built with frames rather than as plain blocks.
                Some(block) if terrustia_proto::tile_sets::frame_important(block) => {
                    Tile::framed(block, 0, 0)
                }
                Some(block) => Tile::block(block),
                None => Tile::AIR,
            }
        }
    }

    /// Solid ground at and below tile row 10.
    fn ground() -> Terrain<impl Fn(i32, i32) -> Option<u16>> {
        Terrain(|_x: i32, y: i32| if y >= 10 { Some(1) } else { None })
    }

    fn zombie_at(x: f32, y: f32) -> Npc {
        Npc::new(3, (x, y), 1).expect("zombie stats")
    }

    /// A Water Bolt Mimic enters the world asleep, and nothing else does.
    ///
    /// `NPC.cs:2754` is the only spawn of 694 in the game and it passes `ai3: 3f`, which is the
    /// dormant state `game::ai::skull` reads (`NPC.cs:21666-21690`, and
    /// `a_water_bolt_mimic_plays_dead_until_it_is_hit` next door, which is what proves 3 means
    /// asleep). Without the seed a mimic that the dungeon library placed on a shelf would run the
    /// ordinary cursed-skull routine and come straight at the player, which is the opposite of the
    /// whole trap.
    ///
    /// Neutralised by deleting the `if npc_type == WATER_BOLT_MIMIC` guard from `NpcStore::spawn`:
    /// the first assertion fails with `0.0` against `3.0`.
    #[test]
    fn a_water_bolt_mimic_is_born_asleep() {
        let mut store = NpcStore::new();
        let mimic = store.spawn(WATER_BOLT_MIMIC, (0.0, 0.0)).expect("a slot");
        assert_eq!(store.get(mimic).expect("the mimic").ai[3], MIMIC_DORMANT);

        // The seed is that one type's and not the style's: an ordinary Cursed Skull runs the same
        // routine (ai_style 10) and hunts from the moment it appears.
        let skull = store.spawn(34, (0.0, 0.0)).expect("a slot");
        assert_eq!(store.get(skull).expect("the skull").ai[3], 0.0);
    }

    /// ...and an underground fairy enters it already coming for whoever dug it up.
    ///
    /// `NPC.cs:3623` is the only spawn of 583 to 585 this server has, and it passes `ai2: 2f`,
    /// which is `game::ai::fairy`'s `state::APPROACH`. Left at zero the fairy would drift about the
    /// cave it appeared in (`state::IDLE`) until somebody happened within two hundred and fifty
    /// pixels of it, and `NPC.AnyHelpfulFairies` (`NPC.cs:90954-90964`) reads the same slot, so the
    /// spawner's "not while one is already out" gate would never fire either.
    ///
    /// Neutralised by deleting the `FAIRY_CRITTERS.contains` guard from `NpcStore::spawn`: all
    /// three assertions fail with `0.0` against `2.0`.
    #[test]
    fn an_underground_fairy_is_born_already_coming_for_you() {
        let mut store = NpcStore::new();
        for npc_type in FAIRY_CRITTERS {
            let index = store.spawn(npc_type, (0.0, 0.0)).expect("a slot");
            assert_eq!(
                store.get(index).expect("the fairy").ai[2],
                FAIRY_APPROACHING,
                "fairy {npc_type}"
            );
        }

        // The range is exactly the three, not the ids either side of them: 582 is the Antlion Larva
        // and 586 the Zombie Merman, and neither reads `ai[2]` as a fairy state.
        for npc_type in [582u16, 586] {
            let index = store.spawn(npc_type, (0.0, 0.0)).expect("a slot");
            assert_eq!(store.get(index).expect("a neighbour").ai[2], 0.0);
        }
    }

    /// One flag decides whether a hit lands, because vanilla has one flag.
    ///
    /// `NpcStats::dont_take_damage` is the type's *seed* for [`Npc::invulnerable`], the way
    /// `SetDefaults` seeds `NPC.dontTakeDamage`, and nothing more. Asking it again inside
    /// [`Npc::strike`] made the seed permanent, which left the Empress, the Moon Lord's core and
    /// the Martian Saucer's core unkillable by anything at all: all three carry the seed and all
    /// three clear the live flag when their routine opens the window. Restoring the `||` in
    /// `strike` turns the second half of this red.
    #[test]
    fn the_types_untouchable_flag_is_a_seed_and_not_a_second_gate() {
        // The Moon Lord's core, and the two others that shipped broken behind the same seed.
        for npc_type in [398u16, 636, 395] {
            let mut npc = Npc::new(npc_type, (0.0, 0.0), 1).expect("a real type");
            assert!(
                npc.stats.dont_take_damage,
                "{npc_type} is seeded untouchable by its table"
            );
            assert!(npc.invulnerable, "so it starts untouchable");
            assert!(
                !npc.take_damage(9999, 0.0, 1) && npc.life == npc.life_max,
                "{npc_type} refuses a hit while the live flag stands"
            );

            // A routine opening its window is the last word on the matter.
            npc.invulnerable = false;
            npc.take_damage(1, 0.0, 1);
            assert!(
                npc.life < npc.life_max,
                "{npc_type} has to be hurtable once its routine says so"
            );
        }
    }

    /// Every type starts where its table says, including on a transform.
    #[test]
    fn a_transform_reseeds_the_untouchable_flag_from_the_new_type() {
        let mut npc = zombie_at(0.0, 0.0);
        assert!(!npc.invulnerable, "an ordinary zombie is fair game");
        npc.become_type(398);
        assert!(npc.invulnerable, "and the Moon Lord's core is not");
    }

    /// An NPC's generation counts reuses of *its slot*, not spawns anywhere.
    ///
    /// The number is how a client tells "slot 5, the one I was told about" from "slot 5, somebody
    /// else now" — `NPC.Equals` compares slot and generation together. A single counter shared by
    /// every slot, which this store used to keep, comes back around after 256 spawns anywhere on
    /// the server; per slot it takes 256 reuses of that one slot. Found by watching a real server
    /// send generation 0 for two fresh NPCs where this one sent 1 and 2.
    #[test]
    fn generation_counts_reuses_of_a_slot_rather_than_spawns_anywhere() {
        let mut store = NpcStore::new();

        // Two different slots, both used for the first time, both generation 1.
        let a = store.spawn(3, (100.0, 100.0)).unwrap();
        let b = store.spawn(3, (200.0, 100.0)).unwrap();
        assert_ne!(a, b, "they should be in different slots");
        assert_eq!(store.get(a).unwrap().generation, 1);
        assert_eq!(
            store.get(b).unwrap().generation,
            1,
            "a second slot's first occupant is still its first"
        );

        // Reuse the first slot: that one advances, and only that one.
        store.remove(a);
        let again = store.spawn(3, (100.0, 100.0)).unwrap();
        assert_eq!(again, a, "the freed slot should be taken first");
        assert_eq!(store.get(again).unwrap().generation, 2);
        assert_eq!(
            store.get(b).unwrap().generation,
            1,
            "the untouched slot should not have moved"
        );
    }

    /// Generation never comes out as zero, however many times a slot turns over.
    ///
    /// Zero is the game's "no generation", and a real client asserts outright on receiving one
    /// (`Invariant.Assert(generation != 0, ...)`). Wrapping straight through it — which
    /// `wrapping_add` alone does — is a server that works perfectly for 255 reuses and then breaks
    /// a client.
    #[test]
    fn generation_skips_zero_on_the_way_round() {
        let mut store = NpcStore::new();
        let mut seen_wrap = false;
        for turn in 0..600 {
            let index = store.spawn(3, (100.0, 100.0)).expect("a slot");
            let generation = store.get(index).unwrap().generation;
            assert_ne!(generation, 0, "generation went to zero on turn {turn}");
            if turn > 0 && generation == 1 {
                seen_wrap = true;
            }
            store.remove(index);
        }
        assert!(
            seen_wrap,
            "600 reuses of one slot should have wrapped at least once"
        );
    }

    #[test]
    fn transforming_keeps_the_slot_and_replaces_everything_the_type_decides() {
        // A lost girl becoming a nymph: same entity, entirely different creature.
        let mut girl = Npc::new(195, (1000.0, 1000.0), 7).expect("lost girl");
        girl.ai = [1.0, 2.0, 3.0, 4.0];
        girl.life = 1;
        let position = girl.position;

        girl.become_type(196);
        assert_eq!(girl.npc_type, 196);
        assert_eq!(girl.stats.name, "Nymph");
        assert_eq!(girl.life, girl.life_max, "and comes back at full health");
        assert_eq!(girl.ai, [0.0; 4], "with a fresh routine");
        assert_eq!(girl.position, position, "but does not move");
        assert_eq!(girl.generation, 7, "and keeps its identity");
    }

    #[test]
    fn an_npc_falls_and_lands_on_the_ground() {
        let mut npc = zombie_at(32.0, 0.0);
        let terrain = ground();
        for _ in 0..200 {
            step_physics(&mut npc, &terrain);
            if npc.on_ground {
                break;
            }
        }
        assert!(npc.on_ground, "never landed");
        // Its feet should rest exactly on the top of row 10.
        assert!(
            (npc.position.1 + npc.height() - 160.0).abs() <= 1.0,
            "rested at {}",
            npc.position.1
        );
        assert_eq!(npc.velocity.1, 0.0);
    }

    #[test]
    fn falling_speed_is_capped() {
        let mut npc = zombie_at(32.0, 0.0);
        let empty = Terrain(|_: i32, _: i32| None);
        for _ in 0..500 {
            step_physics(&mut npc, &empty);
        }
        assert_eq!(npc.velocity.1, MAX_FALL_SPEED);
    }

    /// BA3-01, fail-then-pass: an NPC in a liquid falls by that liquid's rules.
    ///
    /// `UpdateNPC_UpdateGravity` (`NPC.cs:92054-92071`) overwrites both numbers outright when
    /// `wet` is set, and `wet` covers water and lava alike. Until this, `step_physics` took no
    /// liquid at all and used 0.3/10 everywhere: every enemy that walked into a lake fell at one
    /// and a half times vanilla's gravity to 1.43 times its terminal speed, and honey barely
    /// slowed anything down at all.
    #[test]
    fn a_liquid_replaces_gravity_and_terminal_speed() {
        use terrustia_proto::tile::Liquid;

        /// A world that is nothing but one liquid, with no solid tile anywhere to land on.
        struct Pool(Liquid);
        impl TileView for Pool {
            fn tile(&self, _x: i32, _y: i32) -> Tile {
                let mut tile = Tile::AIR;
                tile.liquid = 255;
                tile.liquid_kind = self.0;
                tile
            }
        }

        for (kind, gravity, terminal) in [
            (Liquid::Water, WET_GRAVITY, WET_MAX_FALL_SPEED),
            (Liquid::Lava, WET_GRAVITY, WET_MAX_FALL_SPEED),
            (Liquid::Honey, HONEY_GRAVITY, HONEY_MAX_FALL_SPEED),
            (Liquid::Shimmer, SHIMMER_GRAVITY, SHIMMER_MAX_FALL_SPEED),
        ] {
            let pool = Pool(kind);
            let mut npc = zombie_at(32.0, 0.0);
            step_physics(&mut npc, &pool);
            assert_eq!(npc.velocity.1, gravity, "{kind:?}: one tick of gravity");
            for _ in 0..500 {
                step_physics(&mut npc, &pool);
            }
            assert_eq!(npc.velocity.1, terminal, "{kind:?}: terminal speed");
        }
    }

    #[test]
    fn a_wall_stops_horizontal_movement() {
        // A wall at tile x = 5, with ground below.
        let terrain = Terrain(|x: i32, y: i32| {
            if y >= 10 || (x == 5 && y >= 7) {
                Some(1)
            } else {
                None
            }
        });
        let mut npc = zombie_at(32.0, 100.0);
        npc.velocity.0 = 2.0;
        for _ in 0..200 {
            step_physics(&mut npc, &terrain);
            npc.velocity.0 = 2.0;
        }
        assert!(
            npc.position.0 + npc.width() <= 5.0 * TILE + 1.0,
            "walked into the wall: x={}",
            npc.position.0
        );
    }

    /// The solar sroller's curled bounce is not the game's ordinary tile collision: a wall hit
    /// during it costs one of the two-to-four bounces the wind-up rolled and sends it straight back
    /// the way it came, rather than just stopping it dead the way an ordinary NPC's wall hit would.
    /// `Collision_MoveSolarSroller`, `NPC.cs:93879-93900`.
    #[test]
    fn a_bouncing_solar_sroller_loses_a_bounce_off_a_wall() {
        let mut s = Npc::new(SOLAR_SROLLER, (0.0, 100.0), 1).expect("solar sroller");
        // A wall five pixels past its right edge, and nothing else solid, so a sixteen-pixel move
        // collides on X only.
        let wall_x = s.position.0 + s.width() + 5.0;
        let wall_tile = (wall_x / TILE).floor() as i32;
        let terrain = Terrain(move |x: i32, _y: i32| if x >= wall_tile { Some(1) } else { None });
        s.ai[0] = 6.0; // phase::BOUNCING
        s.ai[2] = 3.0; // three bounces still owed from the wind-up's rand(2..5)
        s.direction = 1;
        s.velocity = (16.0, 0.0);

        step_physics(&mut s, &terrain);

        assert!(s.collide_x, "should have hit the wall this tick");
        assert_eq!(
            s.ai[2], 2.0,
            "a wall hit during a bounce should cost it one"
        );
        assert!(
            s.velocity.0 < 0.0,
            "and send it back the other way, got {}",
            s.velocity.0
        );
        assert_eq!(s.direction, -1, "direction flips with an X bounce");
    }

    #[test]
    fn a_flying_npc_ignores_gravity() {
        // Eater of Souls has noGravity.
        let mut npc = Npc::new(6, (32.0, 0.0), 1).unwrap();
        assert!(npc.stats.no_gravity);
        let terrain = ground();
        step_physics(&mut npc, &terrain);
        assert_eq!(npc.velocity.1, 0.0, "gravity should not apply");
    }

    #[test]
    fn a_no_tile_collide_npc_passes_through_ground() {
        // Eye of Cthulhu ignores terrain entirely.
        let mut npc = Npc::new(4, (32.0, 100.0), 1).unwrap();
        assert!(npc.stats.no_tile_collide);
        npc.velocity.1 = 5.0;
        let terrain = ground();
        for _ in 0..20 {
            step_physics(&mut npc, &terrain);
        }
        assert!(
            npc.position.1 > 160.0,
            "should have passed through the floor"
        );
    }

    /// A resting NPC (`velocity.1 == 0.0`) fully inside solid ground — a real player watching a
    /// real Guide build a house live saw this exact symptom, and it reproduces the same way here:
    /// a tile placed under or through an NPC that was not already falling. `move_vertical`'s own
    /// recovery loop steps by `velocity.1.signum()`, which is `0.0` for a resting NPC, so an
    /// already-embedded-but-resting NPC could never move at all — not further in, not back out —
    /// on any tick, forever.
    #[test]
    fn a_resting_npc_pushed_underground_eventually_escapes() {
        let terrain = ground(); // solid at and below row 10 (y = 160px)
        let mut npc = zombie_at(32.0, 200.0); // well inside the solid ground
        npc.velocity.1 = 0.0;
        for _ in 0..60 {
            step_physics(&mut npc, &terrain);
        }
        // A couple of pixels of tolerance for ordinary float resting contact — the same harmless
        // sub-pixel overlap every other landing in this engine has — not for the 80-pixel burial
        // this test actually reproduces.
        assert!(
            npc.position.1 + npc.height() <= 162.0,
            "should have been pushed back out of solid ground, feet ended at {}",
            npc.position.1 + npc.height()
        );
    }

    #[test]
    fn a_platform_is_landed_on_but_not_a_wall() {
        // Wood platform (19) across row 10, nothing else.
        let terrain = Terrain(|_x: i32, y: i32| if y == 10 { Some(19) } else { None });

        let mut walker = zombie_at(32.0, 100.0);
        walker.velocity.0 = 2.0;
        for _ in 0..60 {
            step_physics(&mut walker, &terrain);
            walker.velocity.0 = 2.0;
        }
        assert!(walker.on_ground, "should stand on the platform");
        assert!(
            walker.position.0 > 60.0,
            "a platform must not block sideways movement"
        );
    }

    #[test]
    fn knockback_scales_with_resistance() {
        let mut slime = Npc::new(1, (0.0, 0.0), 1).unwrap();
        assert_eq!(slime.stats.knockback_resist, 1.0);
        slime.take_damage(5, 4.0, 1);
        assert!(slime.velocity.0 > 0.0, "a full-resist NPC should be pushed");

        // Eye of Cthulhu has knockback_resist 0 and should not move.
        let mut boss = Npc::new(4, (0.0, 0.0), 1).unwrap();
        assert_eq!(boss.stats.knockback_resist, 0.0);
        boss.take_damage(5, 4.0, 1);
        assert_eq!(boss.velocity.0, 0.0);
    }

    /// The push is bounded, not accumulating. `NPC.StrikeNPC_Inner` (`NPC.cs:82216-82311`) runs a
    /// diminishing ladder capped at sixteen and *assigns* the small-hit push rather than adding it,
    /// so hammering a small enemy cannot fling it off at ever-growing speed the way the old
    /// unbounded `velocity += knockback` did.
    #[test]
    fn knockback_is_capped_and_assigned_not_accumulated() {
        // A small hit against a resilient, high-life enemy takes the assign branch. A blue slime is
        // low-life, so give it a wall of health to keep num*10 below lifeMax.
        let mut slime = Npc::new(1, (0.0, 0.0), 1).unwrap();
        slime.life_max = 100_000;
        slime.life = 100_000;
        // A colossal raw knockback still lands capped at sixteen, once, in the hit direction.
        slime.take_damage(1, 1000.0, 1);
        assert!(
            (slime.velocity.0 - 16.0).abs() < 1e-3,
            "the push is capped at sixteen, got {}",
            slime.velocity.0
        );
        // A second identical hit does not stack past the cap: it is assigned, not added.
        slime.take_damage(1, 1000.0, 1);
        assert!(
            (slime.velocity.0 - 16.0).abs() < 1e-3,
            "a second hit must not accumulate past the cap, got {}",
            slime.velocity.0
        );

        // A crit shoves 1.4x harder before the cap is reached.
        let mut a = Npc::new(1, (0.0, 0.0), 1).unwrap();
        a.life_max = 100_000;
        a.life = 100_000;
        a.strike(1, 5.0, 1, false);
        let mut b = Npc::new(1, (0.0, 0.0), 1).unwrap();
        b.life_max = 100_000;
        b.life = 100_000;
        b.strike(1, 5.0, 1, true);
        assert!(
            (b.velocity.0 - a.velocity.0 * 1.4).abs() < 1e-3,
            "a crit should push 1.4x: {} vs {}",
            b.velocity.0,
            a.velocity.0
        );
    }

    #[test]
    fn damage_kills_when_health_runs_out() {
        let mut slime = Npc::new(1, (0.0, 0.0), 1).unwrap();
        assert!(!slime.take_damage(10, 0.0, 1));
        assert_eq!(slime.life, 15);
        assert!(slime.take_damage(15, 0.0, 1), "should report the kill");
        assert!(!slime.is_alive());
    }

    #[test]
    fn slots_are_reused_with_a_fresh_generation() {
        let mut store = NpcStore::new();
        let first = store.spawn(3, (0.0, 0.0)).unwrap();
        let gen_a = store.get(first).unwrap().generation;
        store.remove(first);

        let second = store.spawn(3, (0.0, 0.0)).unwrap();
        assert_eq!(second, first, "the freed slot is reused");
        assert_ne!(
            store.get(second).unwrap().generation,
            gen_a,
            "a reused slot must not keep its old generation"
        );
    }

    #[test]
    fn spawning_an_unknown_type_fails_rather_than_inventing_one() {
        let mut store = NpcStore::new();
        assert_eq!(store.spawn(u16::MAX, (0.0, 0.0)), None);
        assert!(store.is_empty());
    }

    #[test]
    fn town_npcs_do_not_count_against_the_spawn_cap() {
        let mut store = NpcStore::new();
        store.spawn(22, (0.0, 0.0)); // Guide
        assert_eq!(store.used_slots(), 0.0);
        store.spawn(3, (0.0, 0.0)); // Zombie
        assert_eq!(store.used_slots(), 1.0);
    }

    /// B1: `SetDefaults` ends by folding the type's scale into its hitbox (`NPC.cs:17842-17843`),
    /// and this server was storing the pre-multiply literal from `npc_data.rs` and never doing the
    /// multiply. The client runs its own `SetDefaults`, so the box it draws and collides against
    /// disagreed with the one the server charges contact damage from.
    ///
    /// The expected numbers are vanilla's own `width =`/`height =` assignments times the scale in
    /// the same block: the Wall of Flesh 100x100 at 1.2 (`NPC.cs:10370-10387`), the Destroyer head
    /// 38x38 at 1.25, the Dungeon Slime 36x24 at 1.25, the Spike Ball 34x34 at 1.5
    /// (`NPC.cs:9708-9723`), and the Goblin Peon 18x38 at 0.90 (`NPC.cs:15672-15685`) the other
    /// way. The multiply is a truncating `(int)` cast over `f32`, which is why 18 at 0.90 is 16
    /// and not 17.
    #[test]
    fn a_types_scale_is_folded_into_its_hitbox() {
        let box_of = |ty: u16| {
            let n = Npc::new(ty, (0.0, 0.0), 1).expect("a known type");
            (n.width(), n.height())
        };
        assert_eq!(box_of(113), (120.0, 120.0), "Wall of Flesh, scale 1.2");
        assert_eq!(box_of(114), (120.0, 120.0), "its eye, scale 1.2");
        assert_eq!(box_of(134), (47.0, 47.0), "Destroyer head, scale 1.25");
        assert_eq!(box_of(71), (45.0, 30.0), "Dungeon Slime, scale 1.25");
        assert_eq!(box_of(70), (51.0, 51.0), "Spike Ball, scale 1.5");
        // Under one it goes the other way: these were hurting from further off than vanilla.
        assert_eq!(box_of(26), (16.0, 34.0), "Goblin Peon, scale 0.90");
        assert_eq!(box_of(112), (14.0, 14.0), "Vile Spit, scale 0.90");
        // A type at scale one is untouched.
        assert_eq!(
            box_of(1),
            (24.0, 18.0),
            "a blue slime is what the table says"
        );
    }

    /// King Slime is the one routine that rebuilds its own hitbox from a base size, so it is the
    /// one that could end up scaled twice. Its own `resize` compares against `stats.width`, which
    /// now already carries the 1.25, so it finds nothing to do and leaves the box alone.
    #[test]
    fn king_slime_is_not_scaled_twice() {
        let ks = Npc::new(50, (0.0, 0.0), 1).expect("king slime");
        assert_eq!(
            (ks.width(), ks.height()),
            (122.0, 115.0),
            "98x92 at 1.25, which is what its AI rebuilds from"
        );
        assert_eq!(ks.scale, 1.25, "and it still knows its own scale");
    }

    /// `Transform` goes through `SetDefaults` in vanilla, so the new type's scale has to be folded
    /// in again rather than the raw table value being left behind.
    #[test]
    fn becoming_another_type_rescales_the_hitbox() {
        let mut n = Npc::new(1, (0.0, 0.0), 1).expect("blue slime");
        n.become_type(71); // Dungeon Slime, scale 1.25
        assert_eq!((n.width(), n.height()), (45.0, 30.0));
        n.become_type(1);
        assert_eq!((n.width(), n.height()), (24.0, 18.0), "and back down again");
    }

    /// A Corrupt Bunny is a converted Bunny, never a spawned one: vanilla's `NPC.Spawner` cannot
    /// produce type 47 at any depth, and `AttemptToConvertNPCToEvil` is the only source.
    #[test]
    fn a_bunny_turns_corrupt_under_a_corruption_worlds_blood_moon() {
        let mut bunny = Npc::new(46, (100.0, 100.0), 1).expect("a bunny");
        assert!(bunny.attempt_to_convert_to_evil(false), "it converts");
        assert_eq!(bunny.npc_type, 47, "into a Corrupt Bunny");
    }

    /// `NPC.cs:93055`: the same bunny in a crimson world becomes the crimson form instead.
    #[test]
    fn the_same_bunny_turns_crimson_in_a_crimson_world() {
        let mut bunny = Npc::new(46, (100.0, 100.0), 1).expect("a bunny");
        assert!(bunny.attempt_to_convert_to_evil(true));
        assert_eq!(bunny.npc_type, 464);
    }

    /// All three groups, both evils, from `NPC.cs:93052-93082`. Written out rather than looped so
    /// a wrong pairing is visible as a wrong pairing.
    #[test]
    fn every_group_the_game_converts_and_nothing_else() {
        for t in [46, 303, 337, 443, 540] {
            assert_eq!(
                Npc::evil_form(t, false),
                Some(47),
                "bunny family, corruption"
            );
            assert_eq!(Npc::evil_form(t, true), Some(464), "bunny family, crimson");
        }
        for t in [55, 230, 592, 593] {
            assert_eq!(Npc::evil_form(t, false), Some(57));
            assert_eq!(Npc::evil_form(t, true), Some(465));
        }
        for t in [148, 149] {
            assert_eq!(Npc::evil_form(t, false), Some(168));
            assert_eq!(Npc::evil_form(t, true), Some(470));
        }
        // A zombie is not a critter and a blood moon does not change it into anything.
        assert_eq!(Npc::evil_form(3, false), None);
        assert_eq!(Npc::evil_form(1, true), None);
    }

    /// Vanilla's own anti-farm guard (`NPC.cs:93037-93046`): a critter worth no coins, which is
    /// what a statue spawns, must still be worth no coins after it turns. Without this a blood
    /// moon over a bunny statue becomes a coin farm.
    #[test]
    fn a_worthless_critter_is_still_worthless_once_it_turns() {
        let mut bunny = Npc::new(46, (100.0, 100.0), 1).expect("a bunny");
        bunny.stats.value = 0.0;
        bunny.attempt_to_convert_to_evil(false);
        assert_eq!(
            bunny.stats.value, 0.0,
            "a statue-spawned critter must not become worth its evil form's coins"
        );
    }

    /// And one that was worth something keeps whatever its new form is worth, so the guard is a
    /// guard and not a blanket zeroing.
    #[test]
    fn a_wild_critter_takes_its_new_forms_value() {
        let mut bunny = Npc::new(46, (100.0, 100.0), 1).expect("a bunny");
        bunny.stats.value = 5.0;
        bunny.attempt_to_convert_to_evil(false);
        assert_eq!(
            bunny.stats.value,
            npc_stats(47).expect("corrupt bunny").value,
            "a wild one is worth exactly what a Corrupt Bunny is worth"
        );
    }
}
