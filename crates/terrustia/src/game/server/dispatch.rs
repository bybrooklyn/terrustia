//! What a connected client can ask for.
//!
//! [`GameServer::handle_packet`] is the one match every byte from an untrusted socket passes
//! through; everything below it is one packet id's handler, plus the few helpers that exist only to
//! serve them (the join handshake, the section stream, and the presence frames a new arrival needs).
//! Nothing here is trusted: a handler validates first and gives up quietly rather than panicking,
//! because the loop that calls it owns the world. Before any of them runs, [`handshake_allows`]
//! refuses whatever the connection's own state does not allow yet, which is where vanilla puts the
//! same check (`MessageBuffer.GetData`) and is not something an individual handler can be relied on
//! to do for itself.
//!
//! [`handshake_allows`]: GameServer::handshake_allows

// The parent module's prelude, wholesale, rather than a copy of it. Sixty-odd packet handlers
// between them name most of what `server/mod.rs` imports plus about twenty of its own private
// constants and helpers, and restating all of that here would be a second list to keep in step
// with the first. The smaller siblings (`console`, `panel`, `tick`) each name what they use.
use super::*;

/// A dresser is a chest by another name, three tiles wide instead of two.
const DRESSER_BLOCK: u16 = 88;

/// Whether a tile is one this server keeps a [`crate::world::Chest`] record behind.
///
/// `Main.tileContainer` (`Main.cs:10215-10219`) also names the display doll (470) and the hat rack
/// (475). Those are tile *entities* rather than chests, and unlike a chest their placement hook
/// does send the tiles: `TEDisplayDoll.Hook_AfterPlacement` sends a tile square and packet 87, so
/// they arrive through `on_tile_square` and `on_tile_entity_placed` rather than through packet 34.
fn is_container(block: u16) -> bool {
    matches!(
        block,
        CHEST_BLOCK | DRESSER_BLOCK | terrustia_proto::locks::CHEST_2
    )
}

impl GameServer {
    // ---------------------------------------------------------------- packets

    /// Vanilla's own pre-dispatch state gate, from `MessageBuffer.GetData`
    /// (`MessageBuffer.cs:158-172`). `false` means the packet is not dispatched at all.
    ///
    /// Three rules, in vanilla's order:
    ///
    /// * `State == -1` (asked for a password, has not answered) accepts only packet 38.
    /// * `State < 10` (mid-handshake) accepts only ids up to 12, plus 93/16/42/50/38/68/147/161.
    /// * `State == 0` (nothing said yet) accepts only packet 1.
    ///
    /// Our `ConnState` maps onto those directly: `greeted` false is State 0, `greeted` with
    /// `password_ok` false is State -1 (`on_hello` sends `REQUEST_PASSWORD` and leaves the
    /// connection exactly there), and anything short of `Playing` is State < 10, which is set on
    /// the same packet 12 vanilla sets State 10 on (`MessageBuffer.cs:925-928`).
    ///
    /// This is the guard the whole handshake rested on and did not have. Every handler was left to
    /// check `is_playing` for itself and twelve did not, so a socket that had only said hello could
    /// summon a boss (`on_summon`), make it rain for ever (`on_creative_power`, reached from
    /// `on_net_module` before its own check), free the Mechanic (`on_talk_npc`'s rescue), spawn
    /// projectiles, teleport, and open chests. With `password` set that is worse than untidy: the
    /// password prompt is sent and the connection stays in State -1, so none of it was ever behind
    /// the password at all.
    ///
    /// One deliberate difference: vanilla *boots* and then falls through into its own switch,
    /// handling the packet it just rejected. We refuse it instead, which is what booting is for.
    fn handshake_allows(&mut self, slot: u8, id: u8) -> bool {
        /// The ids vanilla lets through mid-handshake on top of everything up to 12: a Steam
        /// social handshake, life/mana, buffs, the password, the client UUID, a loadout, and the
        /// host token.
        const HANDSHAKE_EXTRAS: [u8; 8] = [
            id::SOCIAL_HANDSHAKE,
            id::PLAYER_LIFE_MANA,
            id::PLAYER_MANA,
            id::PLAYER_BUFFS,
            id::SEND_PASSWORD,
            id::CLIENT_UUID,
            id::SYNC_LOADOUT,
            id::HOST_TOKEN,
        ];

        let Some(player) = self.player(slot) else {
            return false;
        };
        if player.is_playing() {
            return true;
        }
        let (greeted, password_ok) = (player.greeted, player.password_ok);
        let allowed = if !greeted {
            id == id::HELLO
        } else if !password_ok {
            id == id::SEND_PASSWORD
        } else {
            id <= id::PLAYER_SPAWN || HANDSHAKE_EXTRAS.contains(&id)
        };
        if !allowed {
            debug!(
                slot,
                id,
                name = id::name(id),
                "refusing a packet from a connection that has not finished joining"
            );
            self.kick(slot, "Your client sent that too early.");
        }
        allowed
    }

    pub(super) fn handle_packet(&mut self, slot: u8, frame: Frame) {
        if !self.handshake_allows(slot, frame.id) {
            return;
        }
        let payload = frame.payload;
        let result = match frame.id {
            id::HELLO => self.on_hello(slot, &payload),
            id::SYNC_PLAYER => self.on_sync_player(slot, &payload),
            id::SYNC_EQUIPMENT => self.on_equipment(slot, &payload),
            id::SPAWN_BOSS_USE_LICENSE_START_EVENT => self.on_summon(slot, &payload),
            id::REQUEST_WORLD_DATA => self.on_request_world_data(slot),
            id::SPAWN_TILE_DATA => self.on_spawn_tile_data(slot, &payload),
            id::PLAYER_SPAWN => self.on_player_spawn(slot, &payload),
            id::PLAYER_CONTROLS => self.on_player_controls(slot, &payload),
            id::PLAYER_LIFE_MANA => self.on_health(slot, &payload),
            id::PLAYER_MANA => self.on_mana(slot, &payload),
            id::CLIENT_UUID => self.on_uuid(slot, &payload),
            id::SEND_PASSWORD => self.on_password(slot, &payload),
            id::TEAM_CHANGE | id::TEAM_CHANGE_FROM_U_I => self.on_team(slot, &payload),
            id::TOGGLE_P_V_P => self.on_pvp(slot, &payload),
            id::ADD_PLAYER_BUFF_PV_P => self.on_pvp_buff_spread(slot, &payload),
            id::PLAYER_BUFFS => self.on_buffs(slot, &payload),
            id::SYNC_PLAYER_ZONE => self.on_zone(slot, &payload),
            // Damage and death are not simulated; relaying keeps every client's view of another
            // player's health and death messages consistent with the client that took the hit.
            // Death names the sender and is rewritten (`MessageBuffer.cs:3910-3916`, `num13 =
            // whoAmI`); damage names the *victim* and is not, which is why it has its own handler.
            // Packet 135 is not in this list at all: it only ever travels server-to-client
            // (`NetMessage.SyncOnePlayer`, `NetMessage.cs:2933-2936`), and vanilla's own case 135
            // is `if (Main.netMode == 1)`, so a dedicated server reads it and drops it.
            id::PLAYER_HURT_V2 => self.on_player_hurt(slot, &payload),
            id::PLAYER_DEATH_V2 => self.relay_player_packet(slot, frame.id, &payload),
            // Everything a player does that only other clients need to be told about: a heal, a
            // mana burst, the angle of the item they are holding, a ninja dodge, their stealth, a
            // flute note, which NPC their minions are on, which tile they are mining, and which
            // loadout they just switched to. Each is the same shape — the owner byte is rewritten
            // to the connection it arrived on, so nobody can act as anybody else — and none of
            // them is something the server has an opinion about.
            id::PLAYER_HEAL
            | id::ITEM_ROTATION_AND_ANIMATION
            | id::MANA_EFFECT
            | id::INSTRUMENT_SOUND
            | id::SYNC_DODGE
            | id::PLAYER_STEALTH
            | id::MINION_ATTACK_TARGET_UPDATE
            | id::SYNC_TILE_PICKING
            | id::SYNC_LOADOUT => self.relay_player_packet(slot, frame.id, &payload),
            id::CRYSTAL_INVASION_START => self.on_crystal_placed(slot, &payload),
            id::SYNC_TILE_PAINT_OR_COATING | id::SYNC_WALL_PAINT_OR_COATING => {
                self.on_paint(slot, frame.id, &payload)
            }
            id::MISC_DATA_SYNC => self.on_misc_data(slot, &payload),
            id::LOCK_AND_UNLOCK => self.on_lock(slot, &payload),
            id::CHEST_UPDATES => self.on_chest_update(slot, &payload),
            id::TILE_ENTITY_PLACEMENT => self.on_tile_entity_placed(slot, &payload),
            id::HIT_SWITCH => self.on_hit_switch(slot, &payload),
            id::TOGGLE_PARTY => self.on_toggle_party(slot),
            id::EMOJI => self.on_emote(slot, &payload),
            id::NPC_HOME => self.on_npc_home(slot, &payload),
            id::BUG_CATCHING => self.on_bug_caught(slot, &payload),
            id::BUG_RELEASING => self.on_bug_released(slot, &payload),
            id::LIQUID_UPDATE => self.on_liquid(slot, &payload),
            // Social chatter and cosmetic effects: nothing to keep, but everyone else has to see
            // it or the world looks different from each side. Only the ids a real dedicated server
            // actually relays are here; see the read-and-drop arm below for the ones it does not.
            id::UPDATE_PLAYER_LUCK_FACTORS => self.on_luck_factors(slot, &payload),
            id::ITEM_USE_SOUND | id::SYNC_PROJECTILE_TRACKERS | id::LAND_GOLF_BALL_IN_CUP => {
                if self.player(slot).is_some_and(Player::is_playing)
                    && let Ok(relayed) = packets::verbatim(frame.id, &payload)
                {
                    self.broadcast(relayed, Some(slot));
                }
                Ok(())
            }
            // Where a player's minions idle. Vanilla stamps the sender over the owner byte before
            // relaying (`MessageBuffer.cs:3585-3596`, `num166 = whoAmI`), so this belongs with the
            // other owner-rewritten relays rather than in the verbatim group it used to sit in,
            // where a client could aim somebody else's minions.
            id::MINION_REST_TARGET_UPDATE => self.relay_player_packet(slot, frame.id, &payload),
            // The latency ping. Vanilla echoes it to the sender *alone*
            // (`MessageBuffer.cs:4445-4452`, `TrySendData(154, whoAmI)`), which is the whole
            // mechanism: `Ping.Update` sends one every 250 ms and will not send another until this
            // comes back (`Ping.cs`). Relaying it to everyone else instead, as this used to, left
            // the sender waiting for ever with a `CurrentPing` that only ever climbed, and gave
            // every other client four stray pings a second per peer.
            id::PING => {
                if self.player(slot).is_some_and(Player::is_playing)
                    && let Ok(frame) = packets::empty(id::PING)
                {
                    self.send(slot, frame);
                }
                Ok(())
            }
            id::SPECIAL_F_X => self.on_special_fx(slot, &payload),
            // Read and dropped, because that is exactly what a dedicated server does with them:
            // every one of these sits inside an `if (Main.netMode == 1)` in `MessageBuffer`, so a
            // real server never relays one and no honest client ever sends one. Relaying them
            // verbatim turned each into a client-driven effect on everybody else's screen:
            // combat text (`:3239,3247`), an emote bubble (`:3408`), the two achievement
            // announcements (`:3573,3579`), a puff of smoke (`:3762`), revenge markers
            // (`:4066,4072`), NPC immunity tampering (`:4145`), an arbitrary legacy sound
            // (`:4160`), and the two with real teeth. `SMART_TEXT_MESSAGE` (`:3772`) is
            // arbitrary coloured multiline text, indistinguishable from a server notice, and
            // `WIRED_CANNON_SHOT` (`:3781`) makes the *named* player's client fire a cannon with
            // attacker-chosen damage and knockback (`if (num77 == Main.myPlayer)
            // WorldGen.ShootFromCannon(..)`). `TEMPORARY_ANIMATION` (`:3189`) is not netMode-gated
            // but is not relayed either: it only ever travels server-to-client
            // (`NetMessage.SendTemporaryAnimation`).
            id::COMBAT_TEXT_INT
            | id::COMBAT_TEXT_STRING
            | id::SYNC_EMOTE_BUBBLE
            | id::ACHIEVEMENT_MESSAGE_N_P_C_KILLED
            | id::ACHIEVEMENT_MESSAGE_EVENT_HAPPENED
            | id::POOF_OF_SMOKE
            | id::SMART_TEXT_MESSAGE
            | id::WIRED_CANNON_SHOT
            | id::SYNC_REVENGE_MARKER
            | id::REMOVE_REVENGE_MARKER
            | id::TAMPER_WITH_N_P_C
            | id::PLAY_LEGACY_SOUND
            | id::TEMPORARY_ANIMATION => {
                debug!(
                    slot,
                    id = frame.id,
                    name = id::name(frame.id),
                    "dropping a client-only packet a server never relays"
                );
                Ok(())
            }
            id::PLACE_OBJECT => self.on_place_object(slot, &payload),
            id::TELEPORT_ENTITY => self.on_teleport(slot, &payload),
            id::UNKNOWN66 => self.on_heal_player(slot, &payload),
            // Which town NPC a player is talking to. The owner byte is first, so the ordinary
            // relay handles it, and remembering it is what a shop will need.
            id::SYNC_TALK_N_P_C => self.on_talk_npc(slot, &payload),
            id::REQUEST_SECTION => self.on_request_section(slot, &payload),
            id::AREA_TILE_CHANGE => self.on_tile_square(slot, &payload),
            id::SYNC_ITEM | id::SPAWN_INSTANCED_ITEM => self.on_sync_item(slot, &payload),
            id::SYNC_ITEM_DESPAWN => self.on_item_despawn(slot, &payload),
            id::DAMAGE_N_P_C => self.on_damage_npc(slot, &payload),
            id::TOGGLE_DOOR_STATE => self.on_door(slot, &payload),
            id::REQUEST_CHEST_OPEN => self.on_chest_open(slot, &payload),
            id::SYNC_CHEST_ITEM => self.on_chest_item(slot, &payload),
            id::SYNC_PLAYER_CHEST => self.on_player_chest(slot, &payload),
            id::OPEN_SIGN_REQUEST => self.on_sign_request(slot, &payload),
            id::OPEN_SIGN_RESPONSE => self.on_sign_write(slot, &payload),
            id::TILE_MANIPULATION => self.on_tile_manipulation(slot, &payload),
            id::NET_MODULES => self.on_net_module(slot, &payload),
            id::SYNC_PROJECTILE => self.on_client_projectile(slot, &payload),
            id::KILL_PROJECTILE => self.on_client_projectile_kill(slot, &payload),
            // Four different ids for one message: putting an item into a frame, onto a weapon
            // rack, onto a food platter, or into a display jar.
            id::ITEM_FRAME_TRY_PLACING
            | id::WEAPONS_RACK_TRY_PLACING
            | id::FOOD_PLATTER_TRY_PLACING
            | id::DEAD_CELLS_DISPLAY_JAR_TRY_PLACING => self.on_display_item(slot, &payload),
            id::T_E_LEASHED_ENTITY_ANCHOR_PLACE_ITEM => self.on_anchor_item(slot, &payload),
            id::REQUEST_TELEPORTATION_BY_SERVER => self.on_server_teleport(slot, &payload),
            id::QUICK_STACK_CHESTS => self.on_quick_stack(slot, &payload),
            id::FISH_OUT_N_P_C => self.on_fished_out_npc(slot, &payload),
            id::SET_MISC_EVENT_VALUES => self.on_misc_event_value(slot, &payload),
            id::REQUEST_LUCY_POPUP => self.on_lucy_popup(slot, &payload),
            id::RELEASE_ITEM_OWNERSHIP => self.on_release_item(slot, &payload),
            id::MURDER_SOMEONE_ELSES_PORTAL => self.on_close_portal(slot, &payload),
            id::TELEPORT_PLAYER_THROUGH_PORTAL => self.on_portal_teleport(slot, &payload),
            id::NEBULA_LEVELUP_REQUEST => self.on_nebula_booster(slot, &payload),
            id::SYNC_EXTRA_VALUE => self.on_extra_value(slot, &payload),
            id::CRYSTAL_INVASION_REQUESTED_TO_SKIP_WAIT_TIME => self.on_skip_army_wait(slot),
            id::REQUEST_QUEST_EFFECT => self.on_quest_effect(slot),
            id::MASS_WIRE_OPERATION => self.on_mass_wire(slot, &payload),
            id::CHEST_NAME => self.on_chest_name_request(slot, &payload),
            id::GEM_LOCK_TOGGLE => self.on_gem_lock(slot, &payload),
            id::ANGLER_QUEST_FINISHED => self.on_angler_finished(slot),
            id::QUESTS_COUNT_SYNC => self.on_quest_count(slot, &payload),
            id::T_E_DISPLAY_DOLL_DATA_SYNC => self.on_display_doll_slot(slot, &payload),
            id::T_E_HAT_RACK_ITEM_SYNC => self.on_hat_rack_slot(slot, &payload),
            id::REQUEST_TILE_ENTITY_INTERACTION => self.on_tile_entity_interaction(slot, &payload),
            id::ADD_N_P_C_BUFF => self.on_add_npc_buff(slot, &payload),
            id::REQUEST_N_P_C_BUFF_REMOVAL => self.on_remove_npc_buff(slot, &payload),
            id::UNIQUE_TOWN_N_P_C_INFO_SYNC_REQUEST => {
                self.on_town_npc_name_request(slot, &payload)
            }
            other => {
                debug!(slot, id = other, name = id::name(other), "ignoring packet");
                Ok(())
            }
        };

        if let Err(e) = result {
            debug!(slot, id = frame.id, name = id::name(frame.id), error = %e, "malformed packet");
        }
    }

    fn on_hello(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if self.player(slot).map(|p| p.state) != Some(ConnState::Greeting) {
            return Ok(()); // a second hello is not a way to restart the handshake
        }

        let hello = Hello::decode(payload)?;
        if !hello.is_supported() {
            // Name both sides. A refusal that says only what the server speaks leaves the person
            // on the other end guessing which of the two needs updating — and this exact check
            // once refused *every* current client, because it matched the string "Terraria325"
            // while the installed game announced 326.
            info!(slot, version = %hello.version, "rejecting unsupported client");
            self.kick(
                slot,
                &format!(
                    "Your client speaks {}; this server speaks {}. \
                     Whichever is older needs updating.",
                    hello.version,
                    id::SUPPORTED_RELEASES
                        .iter()
                        .map(|r| format!("Terraria{r}"))
                        .collect::<Vec<_>>()
                        .join(" and "),
                ),
            );
            return Ok(());
        }

        if let Some(player) = self.player_mut(slot) {
            player.greeted = true;
        }

        // With a password set, the slot is withheld until the client proves it knows it.
        if !self.config.password.is_empty() {
            self.send(slot, packets::empty(id::REQUEST_PASSWORD)?);
            return Ok(());
        }

        self.accept_player(slot)
    }

    /// Assign the slot and let the client proceed.
    fn accept_player(&mut self, slot: u8) -> terrustia_proto::Result<()> {
        if let Some(player) = self.player_mut(slot) {
            player.password_ok = true;
            player.advance_to(ConnState::SlotAssigned);
        }
        // The trailing bool is new in 1.4.5; see docs/protocol-notes.md.
        let frame = packets::player_info(slot, false)?;
        self.send(slot, frame);
        Ok(())
    }

    /// Packet 38: the client's answer to a password prompt.
    fn on_password(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        // Only a connection that has already passed the version check may offer a password.
        let ready = self
            .player(slot)
            .is_some_and(|p| p.greeted && p.state == ConnState::Greeting);
        if self.config.password.is_empty() || !ready {
            return Ok(());
        }
        // Per-IP backoff only: there is no account here, just the one shared server password, so
        // there is nothing for a per-account window to key on. See `admin::throttle`'s top doc.
        // Refused with the exact same "Incorrect password." a genuinely wrong guess gets, so a
        // throttled attempt is not distinguishable from an ordinary wrong one by its wording.
        let ip_key = self.player(slot).map(|p| p.addr.ip().to_string());
        let now = std::time::Instant::now();
        if let Some(ip_key) = &ip_key
            && let crate::admin::Verdict::Refused { log_summary, .. } =
                self.ip_throttle.check(ip_key, now)
        {
            if let Some(n) = log_summary {
                self.audit.record(
                    "system",
                    crate::admin::AuditAction::Throttled,
                    &format!("ip:{ip_key}"),
                    &format!("{n} refused join-password attempt(s) backed off"),
                );
            }
            self.kick(slot, "Incorrect password.");
            return Ok(());
        }
        // `offered` never appears in a log line below, or anywhere else: see `admin::mod`'s own
        // "never logged" convention.
        let offered = PacketReader::new(payload).string()?;
        if crate::admin::constant_time_eq(offered.as_bytes(), self.config.password.as_bytes()) {
            if let Some(ip_key) = &ip_key {
                self.ip_throttle.record_success(ip_key);
            }
            self.accept_player(slot)
        } else {
            if let Some(ip_key) = &ip_key {
                self.ip_throttle.record_failure(ip_key, now);
            }
            info!(slot, "wrong password");
            self.kick(slot, "Incorrect password.");
            Ok(())
        }
    }

    /// Packet 112: one of two unrelated effects, told apart by its first byte.
    ///
    /// `MessageBuffer.cs:3838-3862`. Sub-action 1 is a tree growing, and it is the only one a
    /// server relays: `TrySendData(b, -1, -1, ..)` with no excluded client, so the sender sees it
    /// too (its own tree has to pop as well). Sub-action 2 is a fairy's sparkle, handled locally
    /// and never relayed. We relayed both, and excluded the sender from each.
    fn on_special_fx(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        /// `WorldGen.TreeGrowFX`, the one sub-action a dedicated server passes on.
        const TREE_GROW: u8 = 1;

        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        if PacketReader::new(payload).u8()? != TREE_GROW {
            return Ok(());
        }
        let frame = packets::verbatim(id::SPECIAL_F_X, payload)?;
        self.broadcast(frame, None);
        Ok(())
    }

    /// Packet 117: a player took damage. The first byte is the *victim*, not the sender.
    ///
    /// `NetMessage.SendPlayerHurt`'s first argument is `playerTargetIndex` (`NetMessage.cs:2633`),
    /// and both callers pass the player being hurt (`Projectile.cs`/`Player.cs`, `SendPlayerHurt(i,
    /// ..)` where `i` is the victim), so in a PvP hit the attacker's client sends a packet naming
    /// somebody else. Vanilla relays that byte untouched (`MessageBuffer.cs:3890-3906`, which reads
    /// `num27` and hands the same `num27` back to `SendPlayerHurt`) precisely because it is not the
    /// sender. Ours went through `relay_player_packet`, which stamps the sender's slot over byte 0,
    /// so every third-party client applied the damage to the attacker instead of the victim. Self
    /// damage hid it: there the sender *is* the target, so the rewrite was a no-op.
    ///
    /// The condition around it comes with it, and has to: without the rewrite there is nothing else
    /// stopping a client naming anybody it likes. Vanilla's is `whoAmI == num27 ||
    /// (Main.player[num27].hostile && Main.player[whoAmI].hostile)`: hurt yourself, or hurt
    /// somebody when you are both in PvP.
    fn on_player_hurt(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let victim = PacketReader::new(payload).u8()?;
        let both_hostile = self.player(slot).is_some_and(|p| p.pvp)
            && self.player(victim).is_some_and(|p| p.pvp && p.is_playing());
        if victim != slot && !both_hostile {
            debug!(
                slot,
                victim, "refusing a hurt packet aimed at somebody else"
            );
            return Ok(());
        }
        let frame = packets::verbatim(id::PLAYER_HURT_V2, payload)?;
        self.broadcast(frame, Some(slot));
        Ok(())
    }

    /// Relay a packet that describes the sender, stamping our slot over whatever they claimed.
    fn relay_player_packet(
        &mut self,
        slot: u8,
        message: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let frame = packets::rewrite_owner(message, payload, slot)?;
        self.broadcast(frame, Some(slot));
        Ok(())
    }

    fn on_sync_player(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        // The name sits after slot, skin variant, voice variant, voice pitch and hair.
        let mut r = PacketReader::new(payload);
        r.bytes(1 + 1 + 1)?;
        r.f32()?;
        r.u8()?;
        let name = r.string()?;

        // Two players sharing a name is not merely confusing. `angler_finished_today` is keyed by
        // name on purpose, so a duplicate shares one daily reward with the original and either can
        // shed the cooldown by renaming. Refuse the collision at the door instead.
        let wanted = name.trim().to_string();
        if !wanted.is_empty()
            && self
                .players
                .iter()
                .flatten()
                .any(|p| p.slot != slot && p.name.eq_ignore_ascii_case(&wanted))
        {
            info!(slot, name = %wanted, "rejecting a duplicate name");
            self.kick(slot, "Someone is already playing under that name.");
            return Ok(());
        }

        if let Some(player) = self.player_mut(slot) {
            if !wanted.is_empty() {
                player.name = wanted;
            }
            player.appearance = Some(Bytes::copy_from_slice(payload));
            player.advance_to(ConnState::Identified);
        }

        // The name is known now, so a name or address ban can be enforced before the world is
        // sent. A UUID ban has to wait for packet 68, which arrives later.
        self.enforce_ban(slot);
        if self.player(slot).is_none() {
            return Ok(());
        }

        // Relay live appearance changes; a first-time sync reaches others at spawn instead.
        if self.player(slot).is_some_and(Player::is_playing) {
            let frame = packets::rewrite_owner(id::SYNC_PLAYER, payload, slot)?;
            self.broadcast(frame, Some(slot));
        }
        Ok(())
    }

    fn on_request_world_data(&mut self, slot: u8) -> terrustia_proto::Result<()> {
        let frame = self.world_data().encode()?;
        self.send(slot, frame);
        if let Some(player) = self.player_mut(slot) {
            player.advance_to(ConnState::WorldSent);
        }
        Ok(())
    }

    fn on_spawn_tile_data(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let request = SpawnTileData::decode(payload)?;

        // Vanilla re-sends world data here before the tiles; mirroring it keeps the client's
        // loading sequence identical to the one it was written against.
        let world_data = self.world_data().encode()?;
        self.send(slot, world_data);

        let sections = self.sections_for(request);
        // The key rather than the English, which is what vanilla sends here (`Lang.inter[44]`):
        // a literal would put "Receiving tile data" on the loading screen of a client playing in
        // any other language.
        let status = packets::status_text(
            sections.len() as i32,
            &NetworkText::key("LegacyInterface.44", Vec::new()),
            0,
        )?;
        self.send(slot, status);

        // Queued and drained a few at a time off the tick (`drain_section_streams`) rather than
        // sent in one synchronous loop here: a first join can want up to ~39 sections, and this
        // packet handler runs inline on the single-writer game task like everything else — sending
        // them all in one call blocked every other player's tick for the whole burst.
        let empty = sections.is_empty();
        if let Some(player) = self.player_mut(slot) {
            player.pending_sections = VecDeque::from(sections);
        }
        if empty {
            self.finish_join_stream(slot);
        }
        Ok(())
    }

    /// The rest of a client's initial world stream, run once every section it asked for has
    /// actually gone out: the entities that were alive when it joined, and the packet that tells
    /// the client the tile stream itself is done.
    ///
    /// Split out of `on_spawn_tile_data` so `drain_section_streams` can call it too, the moment a
    /// player's own queue empties on whichever tick that happens to land on.
    ///
    /// Once only, hence the guard on the state it sets. That queue now also carries the sections
    /// the per-tick stream pushes as a player walks ([`GameServer::check_player_sections`]), and
    /// re-running the join tail every time one of those drains would replay every dropped item and
    /// every NPC and then send `InitialSpawn` again, respawning a client that was only crossing a
    /// section boundary.
    fn finish_join_stream(&mut self, slot: u8) {
        if self
            .player(slot)
            .is_none_or(|p| p.state >= ConnState::TilesSent)
        {
            return;
        }
        // Vanilla sends the live entities after the tiles and before StartPlaying; without this a
        // joining player sees an empty world where everyone else sees dropped loot.
        //
        // A `22` follows each `21`, exactly as vanilla's own join loop pairs them
        // (`MessageBuffer.cs:843-850`, case 8), and with the same zero reservation timer: that
        // call passes no `number2`, so `NetMessage`'s case 22 writes a zero there
        // (`NetMessage.cs:678-687`) and only the owner itself carries. Without it a joining client
        // believed every item was free, walked up to one already reserved for somebody else, and
        // turned it to air locally on the way to a `151` this server then refused: the item stayed
        // on the server and stayed invisible to that one client, with nothing scheduled to tell it
        // otherwise, because `tick_items` only broadcasts a `22` for an item it is *newly*
        // reserving.
        let existing: Vec<(i16, ItemOwner, ItemStack)> = self
            .items
            .iter()
            .map(|(index, item)| {
                let owner = ItemOwner {
                    index,
                    owner: item.owner,
                    keep_reservation_ticks: 0,
                    grab_delay_player: 0,
                    grab_delay_ticks: 0,
                    position: item.position,
                };
                (index, owner, item.item)
            })
            .collect();
        for (index, owner, stack) in existing {
            match SyncItem::dropped(index, owner.position, stack).encode() {
                Ok(frame) => self.send(slot, frame),
                Err(e) => {
                    warn!(slot, error = %e, "could not encode a dropped item for a joining player");
                    return;
                }
            }
            match owner.encode() {
                Ok(frame) => self.send(slot, frame),
                Err(e) => {
                    warn!(slot, error = %e, "could not encode an item's owner for a joining player");
                    return;
                }
            }
        }

        if let Err(e) = self.send_npcs(slot) {
            warn!(slot, error = %e, "could not send npcs to a joining player");
            return;
        }

        // The lunar pillars' shields, in the same place vanilla sends them: case 8's tail, after
        // the entities and before `49 InitialSpawn` (`MessageBuffer.cs:869`).
        match self.tower_shield_frame() {
            Ok(frame) => self.send(slot, frame),
            Err(e) => warn!(slot, error = %e, "could not encode the tower shield strengths"),
        }

        if let Some(player) = self.player_mut(slot) {
            player.advance_to(ConnState::TilesSent);
        }
        match packets::empty(id::INITIAL_SPAWN) {
            Ok(frame) => self.send(slot, frame),
            Err(e) => warn!(slot, error = %e, "could not encode InitialSpawn"),
        }
    }

    /// Advance every player's own queued initial-join section stream by a bounded slice of a
    /// tick's budget.
    ///
    /// This used to be a single synchronous loop inside `on_spawn_tile_data`'s own packet handler
    /// — up to ~39 sections, each up to ~3ms on the largest world this project benchmarks, so one
    /// player joining could block every other player's tick for 15–115ms straight. Running it here
    /// instead spreads the same total work across many ticks, a few sections at a time, the same
    /// way every other per-tick system already shares the budget.
    ///
    /// The budget is shared across every player still streaming, not given to each one
    /// separately — a per-player budget would let a burst of simultaneous joins reproduce the
    /// exact stall this exists to fix, just triggered by many joiners at once instead of one.
    /// Slots are drained in ascending order each tick, so under a mass-simultaneous-join burst the
    /// earlier-numbered ones finish loading first rather than everyone progressing in lockstep — a
    /// disclosed ordering bias, not a fairness guarantee, since the problem this fixes (one join
    /// stalling everyone already playing) does not depend on joiners being served evenly.
    pub(super) fn drain_section_streams(&mut self) {
        self.drain_section_streams_bounded(None, SECTION_STREAM_BUDGET);
    }

    /// The shared drain itself, bounded by whichever limit trips first.
    ///
    /// `time_budget` is the wall-clock share of a tick a drain may spend (`SECTION_STREAM_BUDGET`
    /// in production). `max_sections` is a hard cap on how many sections the whole call may send,
    /// and is `None` in production, so a real drain is bounded only by the wall clock exactly as
    /// before. The cap exists purely so a test can drive the drain over a fixed section count
    /// rather than the wall clock: the number of sections that fit in four milliseconds swings with
    /// CI scheduling, which is what let the shared-budget test flake, so `Some(n)` makes the shared
    /// accounting deterministic without weakening what it proves. The counter is per call, spanning
    /// every slot, so it stays a shared budget: were it reset per player, two joiners would drain
    /// twice what one does, which is exactly the regression this whole mechanism guards against.
    fn drain_section_streams_bounded(
        &mut self,
        max_sections: Option<usize>,
        time_budget: Duration,
    ) {
        let slots: Vec<u8> = self
            .players
            .iter()
            .flatten()
            .filter(|p| !p.pending_sections.is_empty())
            .map(|p| p.slot)
            .collect();
        let began = Instant::now();
        let mut sent = 0usize;
        for slot in slots {
            while let Some((sx, sy)) = self
                .player_mut(slot)
                .and_then(|p| p.pending_sections.pop_front())
            {
                let _ = self.send_section(slot, sx, sy);
                sent += 1;
                let drained = self
                    .player(slot)
                    .is_some_and(|p| p.pending_sections.is_empty());
                if drained {
                    self.finish_join_stream(slot);
                    break;
                }
                if max_sections.is_some_and(|cap| sent >= cap) || began.elapsed() >= time_budget {
                    return;
                }
            }
        }
    }

    /// Stream one section, unless this client already has it.
    fn send_section(&mut self, slot: u8, sx: i32, sy: i32) -> terrustia_proto::Result<()> {
        if sx < 0 || sy < 0 || sx >= self.world.sections_x() || sy >= self.world.sections_y() {
            return Ok(());
        }
        // Membership is only *checked* here, and claimed further down once the bytes exist.
        // Claiming it up front meant a section that failed to encode was marked delivered anyway,
        // and every re-request for it was then dropped by this same dedupe — leaving a 200x150
        // hole of sky that no amount of walking back through would fill in.
        if self
            .player(slot)
            .is_none_or(|player| player.sent_sections.contains(&(sx, sy)))
        {
            return Ok(());
        }

        let bounds = self.world.section_bounds(sx, sy);
        if bounds.width == 0 || bounds.height == 0 {
            return Ok(());
        }
        self.flush_dirty_sections();
        let frame = match self.section_cache.get(&(sx, sy)) {
            Some(cached) => cached.clone(),
            None => {
                let extras = self.world.extras_for(bounds);
                let encoded =
                    match encode_section_packet(bounds, &extras, |x, y| self.world.tile(x, y)) {
                        Ok(bytes) => Bytes::from(bytes),
                        Err(e) => {
                            // Loud, because the symptom is a missing piece of world rather than
                            // anything that looks like an error to whoever is playing.
                            warn!(slot, sx, sy, error = %e, "could not encode a section");
                            return Err(e);
                        }
                    };
                self.section_cache.insert((sx, sy), encoded.clone());
                encoded
            }
        };
        if let Some(player) = self.player_mut(slot) {
            player.sent_sections.insert((sx, sy));
        }
        self.send_bytes(slot, frame);
        self.send_chest_contents_for_section(slot, bounds)?;
        Ok(())
    }

    /// Send the contents of every chest inside a section that has just gone out.
    ///
    /// The section itself only announces each chest's id, position and name — enough to draw it,
    /// and nothing more. The game follows every section with the contents as well
    /// (`NetMessage.SyncChestContentsForSection`), and the client needs them for the things it
    /// does without opening a chest: crafting from what is nearby, quick-stacking into it, and the
    /// item search. Without this a room full of stocked chests looks, to all three of those, like
    /// a room full of empty ones.
    fn send_chest_contents_for_section(
        &mut self,
        slot: u8,
        bounds: terrustia_proto::section::SectionBounds,
    ) -> terrustia_proto::Result<()> {
        let right = bounds.x + i32::from(bounds.width);
        let bottom = bounds.y + i32::from(bounds.height);
        let inside: Vec<(i16, Vec<terrustia_proto::ItemStack>)> = self
            .world
            .chests
            .iter()
            .enumerate()
            // A chest's id is its slot in the table, so the index has to survive the gaps that
            // deleted chests leave behind.
            .filter_map(|(id, slot)| slot.as_ref().map(|chest| (id, chest)))
            .filter(|(_, chest)| {
                let (x, y) = (i32::from(chest.x), i32::from(chest.y));
                x >= bounds.x && x < right && y >= bounds.y && y < bottom
            })
            .map(|(id, chest)| (id as i16, chest.items.clone()))
            .collect();

        for (id, items) in inside {
            self.send(slot, objects::sync_chest_size(id, items.len() as i16)?);
            for (index, item) in items.iter().enumerate() {
                let frame = SyncChestItem {
                    chest: id,
                    slot: index as u8,
                    item: *item,
                }
                .encode()?;
                self.send(slot, frame);
            }
        }
        Ok(())
    }

    /// Packet 159: the client asking for one section it finds it is missing.
    ///
    /// A repair, not the streaming path. The server still pushes sections from player positions
    /// every tick (`check_player_sections`, `Main.cs:65601`); 159's only two senders in the whole
    /// game are `WorldItem.cs:380` (a dropped item looking for an owner) and `Player.cs:41852` (a
    /// rope placed onto a tile `IsTileLoaded` says is not there yet).
    fn on_request_section(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if self
            .player(slot)
            .is_none_or(|p| p.state < ConnState::TilesSent)
        {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let sx = i32::from(r.u16()?);
        let sy = i32::from(r.u16()?);
        self.send_section(slot, sx, sy)
    }

    /// The sections vanilla streams on a tile request: a block around spawn, plus one around the
    /// requested position when it is a real location.
    fn sections_for(&self, request: SpawnTileData) -> Vec<(i32, i32)> {
        let mut wanted = HashSet::new();
        let (max_x, max_y) = (self.world.sections_x(), self.world.sections_y());

        // The block is *slid* inside the world rather than clipped against it. Clipping loses a
        // row or column whenever a player is near an edge — a player who spawns in the topmost
        // section used to get one fewer section beneath them than intended, which left the world
        // simply absent a hundred and fifty tiles below their feet. It only showed up when the
        // generator started putting the surface high enough to reach section zero.
        let mut add_block = |cx: i32, cy: i32, w: i32, h: i32| {
            let first_x = (cx - 2).clamp(0, (max_x - w).max(0));
            let first_y = (cy - 1).clamp(0, (max_y - h).max(0));
            for sx in first_x..(first_x + w).min(max_x) {
                for sy in first_y..(first_y + h).min(max_y) {
                    wanted.insert((sx, sy));
                }
            }
        };

        let (spawn_sx, spawn_sy) = self
            .world
            .section_of(i32::from(self.world.spawn_x), i32::from(self.world.spawn_y));
        add_block(spawn_sx, spawn_sy, 5, 3);

        let valid = request.x >= 10
            && request.y >= 10
            && request.x < self.world.width() - 10
            && request.y < self.world.height() - 10;
        if valid {
            let (sx, sy) = self.world.section_of(request.x, request.y);
            add_block(sx, sy, 6, 4);
        }

        let mut sections: Vec<(i32, i32)> = wanted.into_iter().collect();
        // Deterministic order keeps logs and tests reproducible.
        sections.sort_unstable();
        sections
    }

    fn on_player_spawn(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let spawn = PlayerSpawn::decode(payload)?;
        let was_playing = self.player(slot).is_some_and(Player::is_playing);

        if let Some(player) = self.player_mut(slot) {
            if player.state < ConnState::TilesSent {
                return Ok(()); // spawning before the world arrived is not a valid sequence
            }
            player.team = spawn.team;
            // A respawn puts you back on your feet. Without this the server keeps thinking you
            // are dead, and every routine that checks whether anyone is alive ignores you for the
            // rest of the session.
            if player.life <= 0 {
                player.life = player.life_max.max(1);
                player.immune_ticks = 0;
            }
            player.advance_to(ConnState::Playing);
        } else {
            return Ok(());
        }

        // Always relay the spawn so respawns after death are visible.
        let relay = packets::rewrite_owner(id::PLAYER_SPAWN, payload, slot)?;
        self.broadcast(relay, Some(slot));

        if !was_playing {
            self.introduce(slot)?;
            // What everyone already here is wearing. This waits until they are actually playing
            // rather than going out with the world: a client is still working through its
            // handshake when the tiles arrive, and anything sent then is read by the handshake
            // rather than by the game.
            self.send_existing_equipment(slot);
        }
        Ok(())
    }

    /// Exchange presence between a newly spawned player and everyone already in the world.
    fn introduce(&mut self, slot: u8) -> terrustia_proto::Result<()> {
        let others: Vec<u8> = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.is_playing() && p.slot != slot)
            .map(|p| p.slot)
            .collect();

        // Tell the newcomer about everyone else.
        for other in &other_slots(&others) {
            for frame in self.presence_frames(*other)? {
                self.send(slot, frame);
            }
        }

        // Tell everyone else about the newcomer.
        for frame in self.presence_frames(slot)? {
            self.broadcast(frame, Some(slot));
        }

        // A player joining on the same machine the server runs on counts as the host, which is
        // exactly the rule the game uses — `DoesPlayerSlotCountAsAHost` asks the socket whether the
        // far end is the loopback address and nothing else. Only sent when true, as the game only
        // sends it when true.
        if self.player(slot).is_some_and(Player::is_local) {
            self.send(slot, packets::counts_as_host(slot, true)?);
        }

        // How much of the world has gone over to each side. The client cannot work this out from
        // the sections it holds, so without it the Dryad reports a world that is nought per cent
        // of everything however far the corruption has spread.
        self.send(
            slot,
            packets::world_evil_tally(
                self.census.percent_hallow,
                self.census.percent_corrupt,
                self.census.percent_crimson,
            )?,
        );

        // Where every town NPC lives. This is what the housing screen draws its banners from; a
        // client never told has an empty housing menu no matter how many villagers it can see.
        for frame in self.npc_home_frames() {
            self.send(slot, frame);
        }

        // Every banner's kill count. The world has been recording these since §26; nothing was
        // ever telling a client about them, so the bestiary showed nought kills for everything.
        self.send(slot, self.banner_state_frame()?);

        // Every pylon. The client keeps its own list and draws the travel map from it, so one it
        // was never told about is scenery: standing beside it opens a map with nowhere to go.
        for pylon in self.pylons() {
            self.pylon_kinds.insert((pylon.x, pylon.y), pylon.kind);
            self.send(
                slot,
                net_module::pylon_message(net_module::PylonMessage::Added, pylon)?,
            );
        }

        // What the Travelling Merchant is carrying, if he is here. A client that joins mid-visit
        // and is not told finds him with nothing to sell.
        if !self.travel_shop.is_empty() {
            let mut w = terrustia_proto::PacketWriter::new(id::TRAVEL_MERCHANT_ITEMS);
            for at in 0..TRAVEL_SHOP_SLOTS {
                w.i16(self.travel_shop.get(at).copied().unwrap_or(0) as i16);
            }
            if let Ok(frame) = w.finish() {
                self.send(slot, frame);
            }
        }

        // Which six cavern enemies this world has. Fixed for the world's life, so it is sent once
        // on joining rather than kept up to date.
        {
            let mut w = terrustia_proto::PacketWriter::new(id::SYNC_CAVERN_MONSTER_TYPE);
            for kind in self.cavern_monsters.flat() {
                w.u16(kind);
            }
            if let Ok(frame) = w.finish() {
                self.send(slot, frame);
            }
        }

        // Journey mode's four shared toggles this server models — `ASharedTogglePower::
        // OnPlayerJoining`'s own effect. A client never told assumes every power starts off, which
        // is wrong the moment an operator has frozen time or the weather before this player joined.
        for id in [
            net_module::power::FREEZE_TIME,
            net_module::power::FREEZE_RAIN,
            net_module::power::FREEZE_WIND,
            net_module::power::STOP_BIOME_SPREAD,
        ] {
            if let Some(enabled) = self.journey.get(id)
                && let Ok(frame) = net_module::creative_power_toggle(id, enabled)
            {
                self.send(slot, frame);
            }
        }
        // `ModifyTimeRate` and `Difficulty` are the two shared sliders that sync to a joining
        // player (`_syncToJoiningPlayers = true`, the base `ASharedSliderPower` default that
        // neither constructor overrides — `ModifyWind`/`ModifyRain` are both `false` in source,
        // see `journey.rs`'s own module doc for why there is nothing to send for those two here).
        for (id, value) in [
            (
                net_module::power::MODIFY_TIME_RATE,
                self.journey.time_rate_slider,
            ),
            (
                net_module::power::DIFFICULTY,
                self.journey.difficulty_slider,
            ),
        ] {
            if let Ok(frame) = net_module::creative_power_slider(id, value) {
                self.send(slot, frame);
            }
        }
        // `Godmode`/`FarPlacementRange`'s full per-player state — `APerPlayerTogglePower::
        // OnPlayerJoining`'s own `SyncEveryone`, bit-packed. `SpawnRate` sends nothing here on
        // purpose: `APerPlayerSliderPower::OnPlayerJoining` only resets the *new* player's own
        // local cache to the default, no network message at all — another player's slider
        // position was never anyone else's business in the first place (see the slider handler's
        // own comment on why a change to it is never broadcast either).
        for (id, states) in [
            (net_module::power::GODMODE, self.journey.godmode),
            (
                net_module::power::FAR_PLACEMENT_RANGE,
                self.journey.far_placement_range,
            ),
        ] {
            if let Ok(frame) = net_module::creative_power_toggle_full_state(id, &states) {
                self.send(slot, frame);
            }
        }

        // What the Angler wants today. A client that is never told shows no quest at all, so a
        // player who joins after dawn would find the Angler had nothing to say until midnight.
        {
            let name = self
                .player(slot)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            let done = self.angler_finished_today.contains(&name);
            let mut w = terrustia_proto::PacketWriter::new(id::ANGLER_QUEST);
            w.u8(self.angler_quest).bool(done);
            if let Ok(frame) = w.finish() {
                self.send(slot, frame);
            }
        }

        // Last, and it has to be last. 129 is the "you are connected, start playing" signal, and
        // vanilla sends it as the closing act of `MessageBuffer` case 12 (`MessageBuffer.cs:937`):
        // after 139 (`:934`) and after `SyncConnectedPlayer` has already sent 60, 72 and 74
        // (`NetMessage.cs:2841-2856`), with only `greetPlayer`'s chat behind it. This used to sit
        // above the travelling-merchant stock, the cavern monster types and the angler quest, so a
        // client that treats 129 as the end of the handshake, which is what it is, never read any
        // of them. The same-world differential against a real `TerrariaServer`
        // (`tools/differential.sh`) is what caught it.
        self.send(slot, packets::empty(id::FINISHED_CONNECTING_TO_SERVER)?);

        let name = self
            .player(slot)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        // `LegacyMultiplayer.19` is `"{0} has joined."`.
        let who = NetworkText::literal(&name);
        self.announce_key("LegacyMultiplayer.19", vec![who]);

        let motd = self.config.motd.clone();
        if !motd.is_empty()
            && let Ok(frame) = net_module::chat_broadcast(
                net_module::SERVER_AUTHOR,
                &NetworkText::literal(&motd),
                SERVER_CHAT_COLOUR,
            )
        {
            self.send(slot, frame);
        }
        Ok(())
    }

    /// Everything another client needs in order to draw a player.
    fn presence_frames(&self, slot: u8) -> terrustia_proto::Result<Vec<Vec<u8>>> {
        let Some(player) = self.player(slot) else {
            return Ok(Vec::new());
        };

        let mut frames = vec![packets::player_active(slot, true)?];
        if let Some(appearance) = &player.appearance {
            frames.push(packets::rewrite_owner(id::SYNC_PLAYER, appearance, slot)?);
        }
        frames.push(
            PlayerHealth {
                player: slot,
                life: player.life,
                life_max: player.life_max,
            }
            .encode()?,
        );
        frames.push(
            PlayerMana {
                player: slot,
                mana: player.mana,
                mana_max: player.mana_max,
            }
            .encode()?,
        );
        if let Some(buffs) = &player.buffs {
            frames.push(packets::rewrite_owner(id::PLAYER_BUFFS, buffs, slot)?);
        }
        if let Some(zone) = &player.zone {
            frames.push(packets::rewrite_owner(id::SYNC_PLAYER_ZONE, zone, slot)?);
        }
        if player.team != 0 {
            let mut w = terrustia_proto::PacketWriter::new(id::TEAM_CHANGE);
            w.u8(slot).u8(player.team);
            frames.push(w.finish()?);
        }
        if player.pvp {
            let mut w = terrustia_proto::PacketWriter::new(id::TOGGLE_P_V_P);
            w.u8(slot).bool(true);
            frames.push(w.finish()?);
        }
        if let Some(controls) = &player.last_controls {
            frames.push(packets::rewrite_owner(id::PLAYER_CONTROLS, controls, slot)?);
        }
        Ok(frames)
    }

    fn on_player_controls(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let controls = PlayerControls::decode(payload)?;

        if let Some(player) = self.player_mut(slot) {
            // A server-issued teleport this client has not yet acknowledged means the position
            // and velocity it just reported may still describe where it was *before* that
            // teleport — trusting it would snap the player straight back
            // (`MessageBuffer.cs:998-1002`, `player13.unacknowledgedTeleports > 0`). Everything
            // else in the packet (facing, sitting, the selected slot) is unaffected in source,
            // so only position and velocity are held back here.
            if player.unacknowledged_teleports == 0 {
                // Velocity is what actually changed since the last update, not what the client
                // claims: the routines that lead a running player want the real thing.
                player.velocity = (
                    controls.position.0 - player.position.0,
                    controls.position.1 - player.position.1,
                );
                player.position = controls.position;
            }
            // Which way they are looking. Only one thing reads it — a wiring tool's L turns the
            // other way depending on it — but that one thing is visible the moment it is wrong.
            player.facing_right = controls.facing_right();
            player.sitting = controls.sitting();
            player.selected_item = controls.selected_item;
            player.last_controls = Some(Bytes::copy_from_slice(payload));
            if !player.is_playing() {
                return Ok(());
            }
        } else {
            return Ok(());
        }

        // Relayed verbatim: the payload has optional trailing blocks the server does not model.
        let frame = packets::rewrite_owner(id::PLAYER_CONTROLS, payload, slot)?;
        // Culled by loaded section rather than sent to everybody.
        //
        // This is a deliberate departure from vanilla, which relays a player's movement to every
        // other player (`NetMessage.SendData(13)` with only the sender excluded). It is the single
        // worst thing a full server does: every player sends one of these a tick, and relaying each
        // to all the others is `max_players - 1` channel sends per player per tick, which is what
        // fills outbound queues and gets slow clients dropped under load.
        //
        // What a distant client loses is the fullscreen map marker moving smoothly; it cannot draw
        // the player themselves at that range. [`MAX_PLAYER_SYNC_SKIPS`] is what keeps the marker
        // from freezing outright, and coming within [`SECTION_REACH`] restores every-tick updates
        // before they are on screen. The world still plays the same; the fan-out stops being
        // quadratic.
        let at = controls.position;
        self.broadcast_near(
            frame,
            at,
            Withheld::Player(slot),
            MAX_PLAYER_SYNC_SKIPS,
            Some(slot),
        );

        // Checked every real control update while sitting, matching real vanilla's own cadence
        // (`PlayerSittingHelper.UpdateSitting`, called every frame a player sits) rather than a
        // periodic tick — the whole point is that it responds the moment a player who is already
        // sitting selects the doll, not up to `OLD_MAN_CHECK_INTERVAL` seconds later.
        if controls.sitting() {
            self.check_red_hat_skeletron(slot);
        }
        Ok(())
    }

    /// The Clothier's own red-hatted Skeletron: a repeatable, vanity-only re-fight available once
    /// Skeletron has been defeated for real at least once (`NPC.cs:81216-81241`,
    /// `RegisterBoss_Skeletron`'s own five `RedHatSkeletronAdjustmentsEnabled`-gated items).
    ///
    /// Real vanilla's own condition, transcribed exactly rather than approximated now that both
    /// prerequisites this project was once missing — which hotbar slot is selected, and what the
    /// player is sitting on — are both tracked: the sitting player's own currently-selected item
    /// is the Clothier Voodoo Doll (`Player.killClothier`, reset every frame and set true only
    /// while that item is selected — modelled here as a direct check rather than a persisted flag,
    /// since nothing else needs the intermediate state), it is night, they are seated on the one
    /// specific chair frame the event uses (`tile.type == 89`, `frameX` in `2322..=2358`), and a
    /// real, active Clothier (54) — not the Old Man, who only stands in for him before Skeletron's
    /// first, real defeat — is close enough to see them.
    fn check_red_hat_skeletron(&mut self, slot: u8) {
        const CHAIR: u16 = 89;
        const CHAIR_FRAME_MIN: i16 = 2322;
        const CHAIR_FRAME_MAX: i16 = 2358;
        const CLOTHIER_VOODOO_DOLL: i32 = 1307;
        const CLOTHIER: u16 = 54;
        const SKELETRON: u16 = 35;
        /// How close the Clothier has to be — real vanilla's own `Collision.CanHit` is a
        /// line-of-sight check this project has no equivalent for on an NPC-to-player pair; a
        /// flat distance is the same narrowing this project's own `RedHatSkeletron` plan.md entry
        /// already flagged as the honest substitute, not a silent approximation.
        const CLOTHIER_REACH: f32 = 400.0;

        if self.world.day_time || !self.world.progress.downed_boss3 {
            return;
        }
        if self.npcs.iter().any(|(_, n)| n.npc_type == SKELETRON) {
            return;
        }
        let Some(player) = self.player(slot) else {
            return;
        };
        let holding_doll = player
            .inventory
            .get(&u16::from(player.selected_item))
            .is_some_and(|e| e.item.id == CLOTHIER_VOODOO_DOLL && e.item.stack > 0);
        if !holding_doll {
            return;
        }
        let (px, py) = player.position;
        // Real vanilla checks the tile under `player.Bottom + (0, -2)`
        // (`PlayerSittingHelper.UpdateSitting`), not the hitbox's own top-left corner: `position`
        // here is that top-left corner (matching every other tile lookup against it in this file),
        // so it needs the same width/2 and height-2 offsets vanilla's own `Bottom` applies before
        // either coordinate means anything as a tile index.
        let (tx, ty) = (
            ((px + crate::game::ai::PLAYER_WIDTH as f32 / 2.0) / crate::game::npc::TILE) as i32,
            ((py + crate::game::ai::PLAYER_HEIGHT as f32 - 2.0) / crate::game::npc::TILE) as i32,
        );
        let tile = self.world.tile(tx, ty);
        if tile.block != CHAIR || tile.frame_x < CHAIR_FRAME_MIN || tile.frame_x > CHAIR_FRAME_MAX {
            return;
        }
        let clothier = self.npcs.iter().find(|(_, n)| {
            n.npc_type == CLOTHIER
                && n.is_alive()
                && (n.center().0 - px).abs() < CLOTHIER_REACH
                && (n.center().1 - py).abs() < CLOTHIER_REACH
        });
        let Some((index, at)) = clothier.map(|(index, n)| (index, n.center())) else {
            return;
        };

        self.spawn_skeletron_from(index, at, true);
    }

    /// Shared by both real triggers: consume the cursed NPC at `index` and raise Skeletron in its
    /// place — the ordinary Old Man/Clothier curse (`red_hat = false`) and the Clothier's own
    /// repeatable vanity re-fight (`red_hat = true`, `SpawnSkeletron`'s own `redHatMode` argument,
    /// `NPC.cs:81232-81233`) differ only in that one flag.
    pub(super) fn spawn_skeletron_from(&mut self, index: u8, at: (f32, f32), red_hat: bool) {
        const SKELETRON: u16 = 35;

        self.npcs.remove(index);
        self.broadcast_npc_death(index);
        if let Some(spawned) = self.npcs.spawn(SKELETRON, at) {
            if red_hat && let Some(npc) = self.npcs.get_mut(spawned) {
                // What `RedHatSkeletronAdjustmentsEnabled` reads back at drop time.
                npc.ai[3] = 1.0;
            }
            self.announce("Skeletron has awoken!");
            self.broadcast_npc(spawned);
        }
    }

    fn on_health(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let health = PlayerHealth::decode(payload)?;
        if let Some(player) = self.player_mut(slot) {
            player.life = health.life;
            player.life_max = health.life_max;
        }
        if self.player(slot).is_some_and(Player::is_playing) {
            let frame = packets::rewrite_owner(id::PLAYER_LIFE_MANA, payload, slot)?;
            self.broadcast(frame, Some(slot));
        }
        Ok(())
    }

    fn on_mana(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let mana = PlayerMana::decode(payload)?;
        if let Some(player) = self.player_mut(slot) {
            player.mana = mana.mana;
            player.mana_max = mana.mana_max;
        }
        if self.player(slot).is_some_and(Player::is_playing) {
            let frame = packets::rewrite_owner(id::PLAYER_MANA, payload, slot)?;
            self.broadcast(frame, Some(slot));
        }
        Ok(())
    }

    fn on_team(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let mut r = PacketReader::new(payload);
        r.u8()?;
        let team = r.u8()?;
        let old_team = self.player(slot).map_or(0, |p| p.team);
        if let Some(player) = self.player_mut(slot) {
            player.team = team;
        }
        self.relay_player_packet(slot, id::TEAM_CHANGE, payload)?;

        // Vanilla follows the state relay with a chat line — but only to the changer, whoever was
        // on the old team, and whoever is now on the new one, never a full broadcast
        // (`MessageBuffer.cs:2325-2364`). `Lang.mp[13 + team]` names it for teams 0-4; team 5
        // (pink) is `Lang.mp[22]` specifically, not the `13 + team` formula's own `mp[18]` — a
        // real quirk in vanilla's own switch, not a typo to "fix" here.
        if let Some((name, playing)) = self.player(slot).map(|p| (p.name.clone(), p.is_playing()))
            && playing
        {
            let key = if team == 5 {
                "LegacyMultiplayer.22".to_string()
            } else {
                format!("LegacyMultiplayer.{}", 13 + u16::from(team))
            };
            let who = NetworkText::literal(name);
            if let Ok(frame) = net_module::chat_broadcast(
                net_module::SERVER_AUTHOR,
                &NetworkText::key(key, vec![who]),
                team_colour(team),
            ) {
                let targets: Vec<u8> = self
                    .players
                    .iter()
                    .flatten()
                    .filter(|p| {
                        p.slot == slot
                            || (old_team > 0 && p.team == old_team)
                            || (team > 0 && p.team == team)
                    })
                    .map(|p| p.slot)
                    .collect();
                for target in targets {
                    self.send(target, frame.clone());
                }
            }
        }
        Ok(())
    }

    /// Packet 60 inbound: a player using the housing screen.
    ///
    /// This id is sent both ways. The server announces where each town NPC lives, which it already
    /// did — but the *client* sends the same packet to ask for a change, and that half was falling
    /// through to the ignore arm. So dragging an NPC into a room, or evicting one, did nothing at
    /// all on this server while looking like it had worked locally.
    ///
    /// Vanilla's server half (`MessageBuffer.cs` case 60, the `netMode != 1` branches) is two
    /// cases: a status byte of 1 evicts, anything else assigns the room at the given tile. It also
    /// boots a client whose NPC index is out of range as a cheat attempt; we decline the packet
    /// instead, since the transport is not the place to decide somebody is cheating.
    fn on_npc_home(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let index = r.i16()?;
        let home_x = r.i16()?;
        let home_y = r.i16()?;
        let evicting = r.u8()? == 1;

        let Ok(index) = u8::try_from(index) else {
            debug!(
                slot,
                index, "housing request for an npc slot that cannot exist"
            );
            return Ok(());
        };
        // Only town NPCs have homes; anything else is a client asking for something meaningless.
        let Some(npc) = self.npcs.get(index) else {
            return Ok(());
        };
        if !npc.stats.town_npc || !npc.is_alive() {
            return Ok(());
        }

        if evicting {
            if let Some(npc) = self.npcs.get_mut(index) {
                npc.home = None;
            }
            info!(slot, index, "town npc evicted");
        } else {
            // The room has to be one the game would accept, or a client could house a merchant
            // inside solid rock and the server would agree.
            match crate::game::housing::check_room(
                &self.world,
                i32::from(home_x),
                i32::from(home_y),
            ) {
                Ok(_) => {
                    if let Some(npc) = self.npcs.get_mut(index) {
                        npc.home = Some((i32::from(home_x), i32::from(home_y)));
                    }
                    info!(slot, index, home_x, home_y, "town npc moved in");
                }
                Err(why) => {
                    debug!(slot, index, ?why, "housing request refused");
                    // Tell the asker what it actually is, so their screen stops showing the move.
                    if let Some(frame) = self.npc_home_frame(index) {
                        self.send(slot, frame);
                    }
                    return Ok(());
                }
            }
        }
        // Everyone's housing screen has to agree, including the one that asked.
        self.broadcast_npc_home(index);
        Ok(())
    }

    fn on_pvp(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let mut r = PacketReader::new(payload);
        r.u8()?;
        let hostile = r.bool()?;
        if let Some(player) = self.player_mut(slot) {
            player.pvp = hostile;
        }
        self.relay_player_packet(slot, id::TOGGLE_P_V_P, payload)?;

        // Vanilla always follows the state relay with a chat line to everyone, coloured to the
        // toggling player's own team (`MessageBuffer.cs:1860-1864`): `Lang.mp[11]` for turning PvP
        // on, `Lang.mp[12]` for turning it off.
        if let Some((name, team, playing)) = self
            .player(slot)
            .map(|p| (p.name.clone(), p.team, p.is_playing()))
            && playing
        {
            let key = if hostile {
                "LegacyMultiplayer.11"
            } else {
                "LegacyMultiplayer.12"
            };
            let who = NetworkText::literal(name);
            if let Ok(frame) = net_module::chat_broadcast(
                net_module::SERVER_AUTHOR,
                &NetworkText::key(key, vec![who]),
                team_colour(team),
            ) {
                self.broadcast(frame, None);
            }
        }
        Ok(())
    }

    fn on_buffs(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if let Some(player) = self.player_mut(slot) {
            player.buffs = Some(Bytes::copy_from_slice(payload));
        }
        // One of the buffs in there is a term of `Player.RecalculateLuck` (`stinky`), so a buff
        // list arriving can change this player's luck without any packet 134 following it.
        self.refresh_luck(slot);
        self.relay_player_packet(slot, id::PLAYER_BUFFS, payload)
    }

    /// Packet 55: a PvP-flagged player's own hit spreads a buff onto another PvP-flagged player.
    ///
    /// Unlike every other player packet on this page, this one is not a broadcast: real vanilla's
    /// own server relays it to exactly the named target, and only that player's own client
    /// actually calls `AddBuff` on receiving it — everyone else is never told. The payload itself
    /// carries no sender identity to rewrite (`target: u8, buff: u16, duration: i32`, the whole of
    /// it); the real sender is `slot`, read from the connection the way every other packet here
    /// already trusts it, not from anything in the payload.
    fn on_pvp_buff_spread(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let mut r = PacketReader::new(payload);
        let target = r.u8()?;
        let buff = r.u16()?;

        let both_hostile =
            self.player(slot).is_some_and(|p| p.pvp) && self.player(target).is_some_and(|p| p.pvp);
        if !both_hostile || !terrustia_proto::buffs::is_pvp_spreadable(buff) {
            return Ok(());
        }

        self.send(
            target,
            packets::verbatim(id::ADD_PLAYER_BUFF_PV_P, payload)?,
        );
        Ok(())
    }

    fn on_zone(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if let Some(player) = self.player_mut(slot) {
            player.zone = Some(Bytes::copy_from_slice(payload));
        }
        self.relay_player_packet(slot, id::SYNC_PLAYER_ZONE, payload)
    }

    /// A player teleported: a magic mirror, a teleporter, a recall potion.
    ///
    /// The server has to move its own idea of the player as well as relaying, because every
    /// routine that hunts a target reads that position. A teleport the server does not apply
    /// leaves every enemy in the world attacking where the player used to be.
    fn on_teleport(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let mut r = PacketReader::new(payload);
        let flags = r.u8()?;
        let _claimed = r.i16()?;
        let x = r.f32()?;
        let y = r.f32()?;
        let style = r.u8()?;

        // Bits 0 and 1 together say what is being teleported (`MessageBuffer.cs:2985-3034`'s own
        // `num84` switch): 0 a player, which is the only case relayed below; 3 is a client's own
        // acknowledgement of a teleport the server issued, handled on its own just underneath;
        // 1 (an NPC) and 2 (a player-to-player warp with its own chat announcement) are neither
        // modelled here.
        let what = (flags & 1) + ((flags & 2) >> 1) * 2;
        if what == 3 {
            // The client has caught up with a teleport the server told it about — the position it
            // reports in its next controls packet can be trusted again
            // (`Invariant.Assert(Main.player[num82].unacknowledgedTeleports-- >= 0, ...)`,
            // `MessageBuffer.cs:3033`; see `on_player_controls`'s own guard).
            if let Some(player) = self.player_mut(slot) {
                player.unacknowledged_teleports = player.unacknowledged_teleports.saturating_sub(1);
            }
            return Ok(());
        }
        if what != 0 {
            return Ok(());
        }
        // Bit 2 means "where they already are", which is how a client asks for the effect without
        // moving anything.
        let stay = flags & 4 != 0;
        let extra = if flags & 8 != 0 { r.i32()? } else { 0 };

        let at = if stay {
            match self.player(slot) {
                Some(player) => player.position,
                None => return Ok(()),
            }
        } else {
            (x, y)
        };
        if !at.0.is_finite() || !at.1.is_finite() {
            return Ok(());
        }
        if let Some(player) = self.player_mut(slot) {
            player.position = at;
            player.velocity = (0.0, 0.0);
        }

        let mut w = terrustia_proto::PacketWriter::new(id::TELEPORT_ENTITY);
        w.u8(flags);
        w.i16(i16::from(slot));
        w.f32(at.0);
        w.f32(at.1);
        w.u8(style);
        if flags & 8 != 0 {
            w.i32(extra);
        }
        let frame = w.finish()?;
        self.broadcast(frame, Some(slot));
        debug!(slot, x = at.0, y = at.1, "player teleported");
        Ok(())
    }

    /// Packet 66 (`id::UNKNOWN66`): a heal-on-touch projectile's effect landing on a player
    /// (`Projectile.cs:28951`, `aiStyle == 52`).
    ///
    /// The owning client already applied this to its own local copy before sending it — real
    /// vanilla's own receive side (`MessageBuffer.cs:3038-3056`) still applies it again to the
    /// server's authoritative copy rather than trusting the sender's math, and this does the same:
    /// only a positive amount is ever applied (`if (num72 > 0)`, matching real vanilla exactly —
    /// zero and negative heals are silently ignored, not clamped to zero and applied), a target
    /// outside the connected slots is a no-op rather than a panic, and life is clamped to the
    /// target's own max the same way `statLife`/`statLifeMax2` are. Relayed to everyone but the
    /// sender either way vanilla does (`NetMessage.TrySendData(66, -1, whoAmI, ...)`).
    fn on_heal_player(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let heal = HealPlayer::decode(payload)?;
        if heal.amount <= 0 {
            return Ok(());
        }
        let Some(target) = self.player_mut(heal.player) else {
            return Ok(());
        };
        // Saturating: `statLife` is a real `int` in source, wide enough that `+=` never overflows
        // it; this project's own `Player::life` is an `i16` to match the wire, which a maliciously
        // large claimed heal on an already-high life total genuinely can — panicking on the packet
        // path is worse than a heal simply capping out.
        target.life = target.life.saturating_add(heal.amount).min(target.life_max);

        self.broadcast(heal.encode()?, Some(slot));
        Ok(())
    }

    /// Packet 73: a client asking the server to move it somewhere.
    ///
    /// Five items work this way, and the reason they are the server's business rather than the
    /// client's is that all five have to *search the world* for somewhere safe to land — which
    /// means seeing tiles the client may not have loaded. None of them was handled, so a
    /// Teleportation Potion, a Magic Conch, a Demon Conch and a Shellphone were all inert.
    ///
    /// A search that finds nowhere leaves the player where they are. That is the game's own
    /// behaviour and the right one: a conch that fails is a wasted item, but a conch that drops
    /// you into a lava lake is a lost character.
    fn on_server_teleport(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        use crate::game::teleport::{self, Gates, Wants};
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let kind = r.u8()?;

        let (width, height) = (self.world.width(), self.world.height());
        let gates = Gates {
            downed_plantera: self.world.progress.downed_plantera,
            downed_skeletron: self.world.progress.downed_boss3,
            surface: i32::from(self.world.surface),
            width,
            height,
        };
        let Some(here) = self.player(slot).map(|p| p.position) else {
            return Ok(());
        };

        // The underworld begins here. The game keeps it as a fraction of the world's height.
        let underworld = height - 200;

        let spot = match kind {
            TELEPORT_POTION => {
                // Anywhere at all, above the underworld.
                let tiles = WorldTiles(&self.world);
                teleport::find_spot(
                    &tiles,
                    &mut self.rng,
                    (100, width - 200),
                    (100, underworld - 100),
                    &Wants::default(),
                    &gates,
                )
            }
            MAGIC_CONCH => {
                // The ocean on the far side of the world from wherever you are, which is what
                // makes the conch a way of crossing the map rather than a local shuffle.
                let far_side_is_left = here.0 / crate::game::npc::TILE >= (width / 2) as f32;
                let start = if far_side_is_left {
                    BEACH_MARGIN
                } else {
                    width - BEACH_DISTANCE
                };
                let tiles = WorldTiles(&self.world);
                teleport::find_spot(
                    &tiles,
                    &mut self.rng,
                    (start, BEACH_DISTANCE - BEACH_MARGIN),
                    (100, i32::from(self.world.surface) + 100),
                    &Wants {
                        avoid_any_liquid: true,
                        max_fall: 300,
                        ..Default::default()
                    },
                    &gates,
                )
            }
            DEMON_CONCH => {
                // The underworld, near the middle first and then further out if that fails.
                let middle = width / 2;
                let tiles = WorldTiles(&self.world);
                let wants = Wants {
                    avoid_any_liquid: true,
                    avoid_walls: true,
                    allow_platform_floor: true,
                    ..Default::default()
                };
                teleport::find_spot(
                    &tiles,
                    &mut self.rng,
                    (middle - 50, 100),
                    (underworld + 20, 80),
                    &wants,
                    &gates,
                )
                .or_else(|| {
                    // Failing the middle, anywhere in the underworld at all.
                    teleport::find_spot(
                        &tiles,
                        &mut self.rng,
                        (100, width - 200),
                        (underworld + 20, 80),
                        &wants,
                        &gates,
                    )
                })
            }
            // The Shellphone's spawn setting, and the rescue that fires when a player is crushed
            // with nowhere to stand. Both go to the world's spawn point, which is always valid.
            SHELLPHONE_SPAWN | NO_SPACE_RESCUE => Some((
                f32::from(self.world.spawn_x) * crate::game::npc::TILE - PLAYER_HALF_WIDTH,
                f32::from(self.world.spawn_y) * crate::game::npc::TILE - PLAYER_HEIGHT,
            )),
            _ => return Ok(()),
        };

        let Some(at) = spot else {
            debug!(slot, kind, "no safe landing spot; the player stays put");
            return Ok(());
        };
        if let Some(player) = self.player_mut(slot) {
            player.position = at;
            player.velocity = (0.0, 0.0);
            // The server is moving this player without their own client having done so first, so
            // it owes them an acknowledgement round trip before trusting their next reported
            // position again — see `unacknowledged_teleports`'s own doc, and
            // `NetMessage.cs:1108-1111`'s own `number == 0 && number2 != ignoreClient` (true here:
            // this broadcast excludes nobody, unlike a player's own client-initiated teleport).
            player.unacknowledged_teleports += 1;
        }

        // Style 2 is the potion's swirl, 11 the phone's. They are only an effect, but a client
        // that is not told which one plays the wrong animation.
        let style = if kind == TELEPORT_POTION { 2u8 } else { 11u8 };
        let mut w = terrustia_proto::PacketWriter::new(id::TELEPORT_ENTITY);
        w.u8(0).i16(i16::from(slot)).f32(at.0).f32(at.1).u8(style);
        let frame = w.finish()?;
        // To everyone including the mover: the client asked to be moved and does not move itself.
        self.broadcast(frame, None);
        debug!(slot, kind, x = at.0, y = at.1, "server-side teleport");
        Ok(())
    }

    /// A player started or stopped talking to a town NPC.
    fn on_talk_npc(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let mut r = PacketReader::new(payload);
        let _claimed = r.u8()?;
        let npc = r.i16()?;
        if let Some(player) = self.player_mut(slot) {
            player.talking_to = if npc >= 0 { Some(npc as u8) } else { None };
        }
        // `Player.SetTalkNPC` (`Player.cs:4360-4375`) takes the resident's happiness here and
        // nowhere else: once per chat, never on a tick. Closing the chat resets it to
        // `ShoppingSettings.NotInShop` (`ShoppingSettings.cs:9-13`), which quotes a flat price.
        let multiplier = if npc >= 0 {
            self.shop_multiplier(slot, npc as u8)
        } else {
            1.0
        };
        if let Some(player) = self.player_mut(slot) {
            player.shop_multiplier = multiplier;
        }
        if npc >= 0 {
            self.try_rescue(npc as u8);
        }
        self.relay_player_packet(slot, id::SYNC_TALK_N_P_C, payload)
    }

    /// Talking to somebody tied up frees them.
    ///
    /// Six residents are found rather than earned, and the flag their arrival waits on is only
    /// ever set here. Without this the Mechanic could never appear, and she sells the only wire in
    /// the game — so an entire implemented subsystem sat unreachable behind one missing
    /// interaction.
    fn try_rescue(&mut self, index: u8) {
        let Some(npc) = self.npcs.get(index) else {
            return;
        };
        let Some(rescue) = crate::game::rescues::rescue_for(npc.npc_type) else {
            return;
        };

        if let Some(npc) = self.npcs.get_mut(index) {
            npc.become_type(rescue.freed);
        }
        crate::game::rescues::remember(&mut self.world.progress, rescue.freed);
        self.announce(rescue.announcement);
        self.broadcast_npc(index);
        self.broadcast_world_data();
        info!(freed = rescue.freed, "a bound townsperson was rescued");
    }

    /// A player placed a multi-tile object: a chest, a door, a bed, a workbench.
    ///
    /// This has to happen on the server as well as on the clients. Every other client is told and
    /// places it locally, but if the server does not write the tiles too the object is gone the
    /// moment the world is saved, invisible to anyone who joins later, and does not count toward a
    /// house.
    fn on_place_object(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        // Same gates every other tile-writing handler has, and that `MessageBuffer.cs`'s own
        // `case 79` applies: a placement only counts from a client that has actually joined, it is
        // charged against that client's block-spam budget (`SpamAddBlock++`), and it is refused
        // outright unless the client owns the section it is placing into
        // (`Netplay.Clients[whoAmI].TileSections`, mirrored by `Player::sent_sections`). Without
        // these, any socket — handshake incomplete included — could scatter furniture, chests and
        // tile entities anywhere in the world at an unthrottled rate, growing `world.tile_entities`
        // without bound: exactly the illegitimate-edit class the packet-17 fix already closed.
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }

        let mut r = PacketReader::new(payload);
        let x = i32::from(r.i16()?);
        let y = i32::from(r.i16()?);
        let block = r.i16()?;
        let style = i32::from(r.i16()?);
        let _alternate = r.u8()?;
        let random = i32::from(r.i8()?);
        let _direction = r.bool()?;

        if self.note_tile_spam(slot, TileAction::PlaceTile) {
            return Ok(());
        }

        let Ok(block) = u16::try_from(block) else {
            return Ok(());
        };
        let Some(object) = terrustia_proto::tile_object::tile_object(block) else {
            debug!(
                slot,
                block, "ignoring a placement of something that is not an object"
            );
            return Ok(());
        };
        // Ten tiles clear of the world's edge, as the game requires.
        if x < 10 || y < 10 || x >= self.world.width() - 10 || y >= self.world.height() - 10 {
            return Ok(());
        }
        // The section the object is placed into must be one this client was actually sent. Vanilla
        // `break`s here — dropping the placement — rather than relaying a suppressed edit the way
        // case 17 does.
        let (sx, sy) = self.world.section_of(x, y);
        if !self
            .player(slot)
            .is_some_and(|p| p.sent_sections.contains(&(sx, sy)))
        {
            return Ok(());
        }

        // The packet gives the cursor tile; the object's own origin says where its corner goes.
        let (left, top) = (x - object.origin.0, y - object.origin.1);
        // Nothing is placed over anything already there — the game refuses the whole object
        // rather than filling in the gaps.
        for dx in 0..object.width {
            for dy in 0..object.height {
                if self.world.tile(left + dx, top + dy).is_active() {
                    return Ok(());
                }
            }
        }

        let (frame_x, frame_y) = object.frame_of(style, random);
        for dx in 0..object.width {
            let fx = frame_x + dx * (object.coord_width + object.padding);
            let mut fy = frame_y;
            for dy in 0..object.height {
                // A framed tile, which is what marks it active — setting the block alone leaves an
                // inactive tile that every client draws as empty air.
                let was = self.world.tile(left + dx, top + dy);
                let tile = terrustia_proto::tile::Tile::framed(block, fx as i16, fy as i16)
                    .with_wall(was.wall);
                self.world.set_tile(left + dx, top + dy, tile);
                self.liquids.disturb(left + dx, top + dy);
                fy += object.coord_heights.get(dy as usize).copied().unwrap_or(16) + object.padding;
            }
        }

        // A container is not only tiles: it needs somewhere to keep what is put in it. A real
        // client never reaches this handler for one - `Main.tileContainer` keeps chests, dressers
        // and Containers2 out of `SendObjectPlacement` entirely (`Player.cs:40461`), which is what
        // `on_chest_update` exists for - so this only covers a client that placed one the long way
        // round. Kept anyway, because a chest tile with no record behind it is a chest nobody can
        // open, and it is one call.
        if is_container(block) {
            self.register_chest(left, top);
        }

        // Nor is an item frame, a mannequin, a hat rack or a food platter. These are never asked
        // for by packet — the game's placement request does nothing for them — so placing the
        // tile is the *only* moment they can come into existence. Without this an item frame is
        // scenery: it can be built and never holds anything.
        if let Some(kind) = terrustia_proto::tile_entity::EntityKind::for_tile(block) {
            self.add_tile_entity(kind, left as i16, top as i16);
        }

        // Everyone else places it themselves from the same packet.
        self.broadcast(
            terrustia_proto::packets::place_object(x, y, block, style, random)?,
            Some(slot),
        );
        debug!(slot, block, x, y, "object placed");
        Ok(())
    }

    /// Give a container standing at `(left, top)` somewhere to keep what is put in it.
    ///
    /// `Chest.CreateChest` (`Chest.cs:583-600`) keys a chest on the object's own top-left tile,
    /// and a dresser is a chest by another name. Returns the id, which is what the wire calls the
    /// container from now on, or `None` when the world is already holding its 8000th chest.
    fn register_chest(&mut self, left: i32, top: i32) -> Option<i16> {
        let anchor = (left as i16, top as i16);
        match self.world.chest_at(anchor.0, anchor.1) {
            Some((id, _)) => Some(id),
            None => self
                .world
                .add_chest(crate::world::Chest::empty_at(anchor.0, anchor.1)),
        }
    }

    /// `World::world_data`, with the ambient events that live on `GameServer` rather than `World`
    /// patched in — `PartyIsUp` (`self.party`), the same shape `self.army`'s own tier flags would
    /// need if `ArmyOngoing` were wired up (it is not, a real pre-existing gap this project already
    /// disclosed — `World::world_data`'s own comment on `DownedArmyTier1..3`). Every caller that
    /// sends packet 7 should go through this rather than `self.world.world_data()` directly, or a
    /// joining client learns everything about the world except whether a party is happening in it
    /// right now.
    pub(super) fn world_data(&self) -> terrustia_proto::packets::WorldData {
        use terrustia_proto::packets::WorldFlag;
        let mut data = self.world.world_data();
        data.flags
            .set_flag(WorldFlag::PartyIsUp, self.party.is_up());
        data.flags
            .set_flag(WorldFlag::SlimeRain, self.slime_rain.is_active());
        data.flags
            .set_flag(WorldFlag::LanternNight, self.lantern_night.is_up());
        data
    }

    /// Tell everyone the world itself has changed — an eclipse begun, a blood moon risen.
    pub(super) fn broadcast_world_data(&mut self) {
        if let Ok(frame) = self.world_data().encode() {
            self.broadcast(frame, None);
        }
    }

    /// A player used a summoning item.
    ///
    /// This is the only way a boss enters the world, so it is also the only place a client gets to
    /// name an NPC type. What it may name is the game's own list and nothing else: without that
    /// check a crafted packet could ask for anything in the roster, in any number.
    ///
    /// Negative types are events rather than bosses — a pumpkin moon, an eclipse, an invasion.
    fn on_summon(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let mut r = PacketReader::new(payload);
        // The player the client claims; ignored in favour of the connection it arrived on.
        let _claimed = r.i16()?;
        let what = r.i16()?;

        if what >= 0 {
            let npc_type = what as u16;
            if !terrustia_proto::npc_params::summonable(npc_type) {
                debug!(
                    slot,
                    npc_type, "refusing to summon something that is not summonable"
                );
                return Ok(());
            }
            // One at a time. A second Eye of Cthulhu is not a thing the game allows.
            if self.npcs.iter().any(|(_, n)| n.npc_type == npc_type) {
                return Ok(());
            }
            self.summon_on_player(slot, npc_type);
            return Ok(());
        }

        match what {
            // A pumpkin or frost moon, which only rise at night.
            -4 | -5 => {
                let moon = if what == -4 {
                    crate::game::moons::Moon::Pumpkin
                } else {
                    crate::game::moons::Moon::Frost
                };
                self.start_moon(moon, slot);
            }
            // A solar eclipse, which only happens by day.
            -6 => {
                if self.world.day_time && !self.world.eclipse {
                    self.world.eclipse = true;
                    self.announce_key("LegacyMisc.20", Vec::new());
                    self.broadcast_world_data();
                }
            }
            -7 => self.start_invasion(Invasion::Martian),
            // A blood moon, which only rises at night and not twice in one night.
            -10 => {
                if !self.world.day_time && !self.world.blood_moon {
                    self.world.blood_moon = true;
                    self.announce_key("LegacyMisc.8", Vec::new());
                    self.broadcast_world_data();
                }
            }
            // Advanced Combat Techniques (item 4382) and its Volume Two (5336) — furniture-free
            // world unlocks, permanent once read. `Player.ItemCheck_UseCombatBook` sends this the
            // moment the client's own animation finishes; the receive side has no `!alreadyUsed`
            // guard of its own in source (`MessageBuffer.cs:2822-2827`, `2848-2853`), unlike the
            // blood moon/eclipse cases just above, so a repeat send re-announces rather than being
            // refused — transcribed as-is rather than "improved" with a guard vanilla itself
            // never had.
            -11 => {
                self.world.progress.combat_book = true;
                self.announce_key("Misc.CombatBookUsed", Vec::new());
                self.broadcast_world_data();
            }
            -17 => {
                self.world.progress.combat_book_two = true;
                self.announce_key("Misc.CombatBookVolumeTwoUsed", Vec::new());
                self.broadcast_world_data();
            }
            // The rest of the negative range is the invasions, numbered from -1.
            other => {
                if let Some(kind) = Invasion::from_id(i32::from(-other)) {
                    self.start_invasion(kind);
                } else {
                    debug!(slot, what = other, "ignoring an unrecognised summon");
                }
            }
        }
        Ok(())
    }

    /// Put a boss somewhere near a player: on the ground, out of arm's reach, or overhead when
    /// there is no ground to be found.
    pub(super) fn summon_on_player(&mut self, slot: u8, npc_type: u16) {
        use terrustia_proto::npc_params::{
            SUMMON_ABOVE, SUMMON_ATTEMPTS, SUMMON_RANGE_X, SUMMON_RANGE_Y, SUMMON_SAFE_X,
            SUMMON_SAFE_Y,
        };
        // Copied out, because the search below needs the generator and the player at once.
        let Some(at_player) = self.player(slot).map(|p| p.position) else {
            return;
        };
        let (px, py) = (
            (at_player.0 / crate::game::npc::TILE) as i32,
            (at_player.1 / crate::game::npc::TILE) as i32,
        );

        let mut at = None;
        for _ in 0..SUMMON_ATTEMPTS {
            let x = px + rand::Rng::random_range(&mut self.rng, -SUMMON_RANGE_X..=SUMMON_RANGE_X);
            let y = py + rand::Rng::random_range(&mut self.rng, -SUMMON_RANGE_Y..=SUMMON_RANGE_Y);
            // Not right on top of the player.
            if (x - px).abs() < SUMMON_SAFE_X && (y - py).abs() < SUMMON_SAFE_Y {
                continue;
            }
            if self.world.tile(x, y).is_active() {
                continue;
            }
            let Some(ground) = crate::game::spawn::find_ground(&self.world, x, y) else {
                continue;
            };
            at = Some((
                x as f32 * crate::game::npc::TILE,
                (ground - 1) as f32 * crate::game::npc::TILE,
            ));
            break;
        }
        // Nowhere to stand — a Moon Lord, or a player in mid-air. Overhead it is.
        let at = at.unwrap_or((at_player.0, at_player.1 - SUMMON_ABOVE));

        // A worm head spawned alone is a floating face: this is the same real trigger the evil
        // biome's own third-orb-break uses (`smash_orb`) and the one a real summon item's packet
        // reaches (`on_summon`), so a bodyless Eater of Worlds or Destroyer here is not a cosmetic
        // gap — it is the whole fight missing, since both bosses' own real damage/behaviour depend
        // on having a body at all. `/spawn`'s own admin command already knew to do this for the
        // four ordinary worm monsters; this was the one real path that never did.
        let spawned = match self.worm_parts(npc_type) {
            Some((body, tail, segments)) => {
                self.npcs.spawn_worm(npc_type, body, tail, segments, at)
            }
            None => self.npcs.spawn(npc_type, at),
        };
        if let Some(index) = spawned {
            let name = self
                .npcs
                .get(index)
                .map(|n| n.stats.name)
                .unwrap_or("Something");
            // `Announcement.HasAwoken` is `"{0} has awoken!"`, and its argument is itself a
            // keyed text — the NPC's name. Our internal names are exactly the game's `NPCName.*`
            // keys (`npc_data.rs` calls it "the NPCID constant name"), so the two line up without
            // a translation table.
            let who = NetworkText::key(format!("NPCName.{name}"), Vec::new());
            self.announce_key("Announcement.HasAwoken", vec![who]);
            self.broadcast_npc(index);
            info!(slot, npc_type, name, "boss summoned");
        }
    }

    /// One slot of a player's inventory.
    ///
    /// The slot is remembered whatever it is — the server is the authority on what a player is
    /// carrying — but only the public ones are passed on. A player's safe is their own business.
    ///
    /// The owner byte the client sends is not trusted: it is overwritten with the slot the packet
    /// actually arrived on, which is what stops one client rewriting another's inventory.
    fn on_equipment(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let mut equipment = terrustia_proto::inventory::SyncEquipment::decode(payload)?;
        equipment.player = slot;
        if equipment.slot >= terrustia_proto::inventory::SLOT_COUNT {
            debug!(
                slot,
                requested = equipment.slot,
                "ignoring an out-of-range inventory slot"
            );
            return Ok(());
        }
        if let Some(player) = self.player_mut(slot) {
            player.inventory.insert(equipment.slot, equipment);
        }
        if terrustia_proto::inventory::relayed(equipment.slot) {
            self.broadcast(equipment.encode()?, Some(slot));
        }
        Ok(())
    }

    /// Tell one player what everybody else is carrying.
    ///
    /// Without this a player who joins a running server sees everyone else naked: the equipment
    /// packets went out before they arrived and are never repeated.
    fn send_existing_equipment(&mut self, to: u8) {
        let frames: Vec<Bytes> = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.slot != to && p.is_playing())
            .flat_map(|p| p.inventory.values())
            .filter(|e| terrustia_proto::inventory::relayed(e.slot))
            .filter_map(|e| e.encode().ok())
            .map(Bytes::from)
            .collect();
        for frame in frames {
            self.send_bytes(to, frame);
        }
    }

    fn on_uuid(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let uuid = PacketReader::new(payload).string()?;
        if let Some(player) = self.player_mut(slot) {
            player.uuid = Some(uuid);
        }
        // The last of the three identities arrives here, so this is the first moment a UUID ban
        // can be enforced. Name and address are checked earlier, at the handshake; a UUID cannot
        // be, because packet 68 comes after the slot is already assigned.
        self.enforce_ban(slot);
        Ok(())
    }

    /// Turn somebody away if any of their three identities is banned.
    ///
    /// Name, address and client UUID. `Player::uuid` was stored by this server and read by nothing
    /// at all until now; this is what it was for.
    fn enforce_ban(&mut self, slot: u8) {
        let Some(player) = self.player(slot) else {
            return;
        };
        let (name, address) = (player.name.clone(), player.addr.ip().to_string());
        let uuid = player.uuid.clone();

        // The guest list first, when there is one. Checked here rather than earlier because it is
        // keyed by name, and the name only arrives with the player's appearance.
        if !self.admin.welcome(&name) {
            info!(slot, %name, %address, "refusing somebody not on the guest list");
            self.kick(slot, "You are not on this server's guest list.");
            return;
        }

        let Some(ban) = self.admin.ban_for(&name, &address, uuid.as_deref()) else {
            return;
        };
        let reason = ban.reason.clone();
        info!(slot, %name, %address, reason, "refusing a banned player");
        self.kick(slot, &format!("You are banned: {reason}"));
    }

    /// Count one tile edit against this player's spam budget, and say whether to stop.
    ///
    /// Vanilla's, transcribed from `RemoteClient`: a counter per kind, bumped once per edit
    /// packet, decayed every tick, and the connection booted past a ceiling. Placing is the
    /// tightest (100, decaying 0.3 a tick, so ~18 a second sustained); breaking is deliberately
    /// loose (500, decaying 5 a tick) because mining is fast and legitimate.
    ///
    /// Not having this at all was a regression *from* vanilla rather than a place where we simply
    /// match how trusting vanilla is — which is why it sits inside "match vanilla's trust model"
    /// rather than being the TShock-style validation that stays deferred.
    ///
    /// The ceilings only *apply* when `spam_check` is on, which is the other half of vanilla's own
    /// mechanism and was missing here: `RemoteClient.SpamUpdate` (`RemoteClient.cs:70-80`) opens
    /// `if (!Netplay.SpamCheck) { ...zero every counter...; return; }`, and `Netplay.SpamCheck`
    /// (`Netplay.cs:65`) is `false` unless the server was started with `secure=1`
    /// (`Main.cs:5200`) or `-secure` (`LaunchInitializer.cs:152`). A stock vanilla server never
    /// boots anybody for tile spam, so neither does a stock terrustia one.
    fn note_tile_spam(&mut self, slot: u8, kind: TileAction) -> bool {
        if !self.config.spam_check {
            return false;
        }
        let (counter, ceiling, why): (fn(&mut Player) -> &mut f32, f32, &str) = match kind {
            TileAction::KillTile | TileAction::KillTileNoItem | TileAction::KillWall => (
                |p| &mut p.spam_break,
                SPAM_BREAK_MAX,
                "breaking tiles too fast",
            ),
            _ => (
                |p| &mut p.spam_place,
                SPAM_PLACE_MAX,
                "placing tiles too fast",
            ),
        };
        let Some(player) = self.player_mut(slot) else {
            return true;
        };
        let count = counter(player);
        *count += 1.0;
        if *count <= ceiling {
            return false;
        }
        // Vanilla boots with `Net.CheatingTileSpam`; the reason travels as our own text because
        // it is the server talking, not the game.
        info!(slot, why, "disconnecting a client for edit spam");
        self.kick(slot, why);
        true
    }

    /// Let every player's spam budget recover, once a tick.
    ///
    /// Nothing counts up while `spam_check` is off, so there is nothing to decay: vanilla's own
    /// `SpamUpdate` returns before its decay for the same reason (`RemoteClient.cs:70-80`).
    pub(super) fn tick_tile_spam(&mut self) {
        if !self.config.spam_check {
            return;
        }
        for player in self.players.iter_mut().flatten() {
            player.spam_place = (player.spam_place - SPAM_PLACE_DECAY).max(0.0);
            player.spam_break = (player.spam_break - SPAM_BREAK_DECAY).max(0.0);
            player.spam_liquid = (player.spam_liquid - SPAM_LIQUID_DECAY).max(0.0);
        }
    }

    fn on_tile_manipulation(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }

        let edit = TileManipulation::decode(payload)?;
        // Counted before anything else, so an edit that is refused still costs its budget. A
        // client hammering out-of-bounds coordinates is spamming just as hard as one hammering
        // valid ones.
        if self.note_tile_spam(slot, edit.kind()) {
            return Ok(());
        }
        let (x, y) = (i32::from(edit.x), i32::from(edit.y));
        if !self.world.in_bounds(x, y) {
            return Ok(());
        }

        // Vanilla parity: `MessageBuffer.cs`'s packet-17 handler (`case 17`) starts its own local
        // `flag14` from the client's own claimed failure bit, then forces it to `true` — never back
        // to `false` — the moment the edit's section isn't in `Netplay.Clients[whoAmI].TileSections`
        // (`RemoteClient.cs:31`, the exact state `Player::sent_sections` already mirrors here). That
        // combined flag is what every `WorldGen.KillTile`/`KillWall` call in that packet passes as
        // its own `fail` argument: a client editing a tile it was never sent a section for still
        // gets the swing animation, but the edit itself never actually lands — the same shape the
        // check below reproduces by folding into `changed` rather than dropping the packet outright.
        // Relaying regardless (below, unconditional on `changed`) is also load-bearing and already
        // vanilla-shaped: it is exactly how vanilla's own "this edit failed" state reaches every
        // other client too, not something this check needs to special-case.
        let (sx, sy) = self.world.section_of(x, y);
        let section_owned = self
            .player(slot)
            .is_some_and(|p| p.sent_sections.contains(&(sx, sy)));

        let mut tile = self.world.tile(x, y);
        // Snapshotted before any match arm below touches it, so `/world undo` can put back the
        // tile's whole state — not just the field this particular edit happened to change.
        let before = tile;
        let mut changed = true;
        let mut broke = None;

        match edit.kind() {
            TileAction::KillTile | TileAction::KillTileNoItem => {
                // A pickaxe swing that only damages a block also arrives here; only a real break
                // clears the tile.
                if edit.destroyed() {
                    if tile.is_active() && matches!(edit.kind(), TileAction::KillTile) {
                        // The frames go with the block. Everything that decides what a broken
                        // object is worth — which evil an orb belongs to, which chair a chair is
                        // — lives in the frame, and the frame is about to be cleared.
                        broke = Some((tile.block, tile.frame_x, tile.frame_y));
                    }
                    tile.flags.set(TileFlags::ACTIVE, false);
                    tile.block = 0;
                    tile.frame_x = -1;
                    tile.frame_y = -1;
                    tile.slope = 0;
                    tile.flags.set(TileFlags::HALF_BRICK, false);
                } else {
                    changed = false;
                }
            }
            TileAction::PlaceTile => {
                let block = edit.arg.max(0) as u16;
                if frame_important(block) {
                    // Multi-tile objects need placement and framing rules the slice does not
                    // implement. The edit is still relayed so clients agree with each other.
                    debug!(slot, block, "not modelling framed tile placement");
                    changed = false;
                } else {
                    tile.block = block;
                    tile.frame_x = -1;
                    tile.frame_y = -1;
                    tile.flags.set(TileFlags::ACTIVE, true);
                    tile.flags.set(TileFlags::HALF_BRICK, false);
                    tile.slope = 0;
                }
            }
            TileAction::KillWall => {
                if edit.destroyed() {
                    tile.wall = 0;
                    tile.wall_color = 0;
                } else {
                    changed = false;
                }
            }
            TileAction::PlaceWall => tile.wall = edit.arg.max(0) as u16,
            TileAction::PoundTile => {
                // Hammering cycles a block through half-brick and the slopes; the client does the
                // same walk, so mirroring just the half-brick step keeps the common case right.
                tile.slope = 0;
                let half = tile.flags.has(TileFlags::HALF_BRICK);
                tile.flags.set(TileFlags::HALF_BRICK, !half);
            }
            TileAction::SlopeTile => {
                tile.slope = edit.arg.clamp(0, 4) as u8;
                tile.flags.set(TileFlags::HALF_BRICK, false);
            }
            TileAction::PlaceWire => tile.flags.set(TileFlags::WIRE_RED, true),
            TileAction::KillWire => tile.flags.set(TileFlags::WIRE_RED, false),
            TileAction::PlaceWire2 => tile.flags.set(TileFlags::WIRE_BLUE, true),
            TileAction::KillWire2 => tile.flags.set(TileFlags::WIRE_BLUE, false),
            TileAction::PlaceWire3 => tile.flags.set(TileFlags::WIRE_GREEN, true),
            TileAction::KillWire3 => tile.flags.set(TileFlags::WIRE_GREEN, false),
            TileAction::PlaceWire4 => tile.flags.set(TileFlags::WIRE_YELLOW, true),
            TileAction::KillWire4 => tile.flags.set(TileFlags::WIRE_YELLOW, false),
            TileAction::PlaceActuator => tile.flags.set(TileFlags::ACTUATOR, true),
            TileAction::KillActuator => tile.flags.set(TileFlags::ACTUATOR, false),
            TileAction::Other(action) => {
                debug!(slot, action, "unmodelled tile action; relaying only");
                changed = false;
            }
        }

        // Never turns a rejected edit back into an accepted one — only ever suppresses one the
        // match above already decided should apply, matching `flag14`'s own one-way OR in source.
        changed = changed && section_owned;
        if changed {
            self.world.set_tile(x, y, tile);
            // Mining a block is the commonest way liquid starts moving.
            self.liquids.disturb(x, y);
            if let Some(name) = self.player(slot).map(|p| p.name.clone()) {
                self.tile_log.record(x, y, before, &name);
            }
        }
        // Gated on `section_owned`, not `changed`: a rejected edit still leaves `broke` set to
        // whatever the match arm above decided a real kill would drop, and applying these side
        // effects (a real item drop, an altar smash, waking a boss) without the tile ever actually
        // having been removed is exactly the exploit this whole check exists to close.
        if section_owned && let Some((block, frame_x, frame_y)) = broke {
            self.spawn_tile_drop(block, frame_x, frame_y, x, y);
            // A demon altar is the only way hardmode ore gets into a world, and it always costs
            // something to break.
            if block == DEMON_ALTAR {
                self.smash_altar(x, y, slot);
            }
            // The handful of tiles that are worth more than the item they leave behind.
            if block == terrustia_proto::orbs::ORB_TILE {
                self.smash_orb(x, y, frame_x);
            }
            // Neither of these has a summon item: breaking the thing *is* the summon.
            if block == crate::world::bulbs::BULB {
                self.wake_from_tile(x, y, PLANTERA);
                // And another grows, so a world cannot be left with no way back to her.
                if !self.world.progress.downed_plantera {
                    self.grow_plantera_bulb();
                }
            }
            if block == BEE_LARVA {
                self.wake_from_tile(x, y, QUEEN_BEE);
            }
        }

        // Relay regardless: even an edit the server does not model must reach other clients, or
        // their view of the world silently diverges from the sender's.
        self.broadcast(edit.encode()?, Some(slot));
        Ok(())
    }

    /// Packet 20: a rectangle of tiles pushed as one unit.
    ///
    /// Clients send this for anything spanning more than a single tile — furniture, trees, a door
    /// swinging open. Applying it is what keeps the server's world in step with multi-tile
    /// operations without reimplementing the game's placement and framing rules.
    fn on_tile_square(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }

        // Merged onto the tile already on the ground at each position, not decoded into a fresh
        // one — `world.tile` is bounds-checked and returns air outside the world, which is fine
        // here: an out-of-bounds square is discarded whole by the bounds check just below,
        // whatever this merged against.
        let square = TileSquare::decode(payload, |x, y| self.world.tile(x, y))?;
        let (x0, y0) = (i32::from(square.x), i32::from(square.y));

        // A square is at most 255 on a side, so a hostile one cannot cost much; still refuse any
        // that reaches outside the world rather than clamping it into somewhere unintended.
        if !self.world.in_bounds(x0, y0)
            || !self.world.in_bounds(
                x0 + i32::from(square.width) - 1,
                y0 + i32::from(square.height) - 1,
            )
        {
            debug!(
                slot,
                x = square.x,
                y = square.y,
                "tile square out of bounds"
            );
            return Ok(());
        }

        for dx in 0..usize::from(square.width) {
            for dy in 0..usize::from(square.height) {
                if let Some(tile) = square.tile(dx, dy) {
                    let (x, y) = (x0 + dx as i32, y0 + dy as i32);
                    self.world.set_tile(x, y, tile);
                    // Anything a client rewrites might have been holding liquid up.
                    self.liquids.disturb(x, y);
                }
            }
        }

        // Section-gated like every other packet-20 send, and still excluding the client that sent
        // it: vanilla's case 20 loop tests `num23 != ignoreClient` and `SectionRange` together
        // (`NetMessage.cs:1725`), not one or the other.
        self.broadcast_tile_square(&square, Some(slot));
        Ok(())
    }

    /// Packet 19: a door, trapdoor or tall gate opening or closing.
    ///
    /// Applied server-side, not only relayed. The old comment here claimed the tiles would catch
    /// up "until a client pushes a tile square over them", and that never happens: a client that
    /// works a door sends packet 19 and nothing else (`Player.cs:33093-33098`). So every door a
    /// player ever touched stayed as the server had first loaded it - reverting on save, arriving
    /// wrong for a joining player, and read wrong by housing and by collision. Vanilla's own
    /// server does the work here before relaying (`MessageBuffer.cs:1299-1332`), which is the
    /// same thing this server already does for a *wired* door (`fire_wired_door` and its
    /// neighbours); this handler had simply never been joined up to it.
    fn on_door(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        use crate::world::trapdoors::{TRAPDOOR_CLOSED, TRAPDOOR_OPEN};

        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let door = DoorToggle::decode(payload)?;
        let (x, y) = (i32::from(door.x), i32::from(door.y));
        // `WorldGen.InWorld(num46, num47, 3)` (`MessageBuffer.cs:1304`): three tiles clear of the
        // edge, because a door is three tall and two wide and the whole of it has to fit.
        if x < 3 || y < 3 || x >= self.world.width() - 3 || y >= self.world.height() - 3 {
            return Ok(());
        }
        // `int num48 = ((reader.ReadByte() != 0) ? 1 : (-1))` (`MessageBuffer.cs:1306`).
        let direction: i8 = if door.direction != 0 { 1 } else { -1 };

        match door.action {
            0 => {
                crate::world::doors::open(&mut self.world, x, y, direction);
            }
            // `forced: true`: a player pulling a door shut is allowed to shut it on somebody, and
            // vanilla's packet path passes the flag that skips `Collision.EmptyTile`.
            1 => {
                crate::world::doors::close(&mut self.world, x, y);
            }
            // `onlyCloseOrOpen`, the fourth argument of `WorldGen.ShiftTrapdoor` (`WorldGen.cs:
            // 51905`): 1 for action 2 permits only the close, 0 for action 3 only the open. This
            // project's own `shift_trapdoor` dispatches on the live tile instead, so the
            // restriction becomes a type check on the way in - without which a stale or crafted
            // packet 19 flips a trapdoor the way it was not asked to.
            2 | 3 => {
                let wanted = if door.action == 2 {
                    TRAPDOOR_OPEN
                } else {
                    TRAPDOOR_CLOSED
                };
                if self.world.tile(x, y).block == wanted {
                    let occupants = self.entity_hitboxes();
                    crate::world::trapdoors::shift_trapdoor(
                        &mut self.world,
                        x,
                        y,
                        direction == 1,
                        |tx, ty| crate::game::server::systems::tile_occupied(tx, ty, &occupants),
                    );
                }
            }
            // `closing: false` for 4 and `true` for 5, both `forced: true`.
            4 | 5 => {
                crate::world::trapdoors::shift_tall_gate(
                    &mut self.world,
                    x,
                    y,
                    door.action == 5,
                    true,
                    |_, _| false,
                );
            }
            _ => {}
        }

        // Relayed whether or not anything moved: vanilla's own `TrySendData` sits outside the
        // switch, inside the `InWorld` check (`MessageBuffer.cs:1328-1331`).
        self.broadcast(door.encode()?, Some(slot));
        Ok(())
    }

    /// Packet 31: a client asking to open a chest.
    ///
    /// Two things real vanilla's own receive handler does beyond handing the opener their items
    /// (`MessageBuffer.cs:1868-1895`) were missing entirely: telling *other* clients which chest
    /// this player now has open (packet 80, `SyncPlayerChestIndex` — without it their own UI never
    /// shows the chest as already in use), and `WorldGen.IsChestRigged`'s own check, which treats
    /// opening a wired chest (tile 467, style 4) as hitting a switch.
    fn on_chest_open(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let request = RequestChestOpen::decode(payload)?;

        let Some((id, chest)) = self.world.chest_at(request.x, request.y) else {
            return Ok(());
        };
        // Vanilla refuses a chest someone else is already inside, so two players cannot both edit
        // the same slots and clobber each other.
        if self
            .players
            .iter()
            .flatten()
            .any(|p| p.slot != slot && p.open_chest == id)
        {
            debug!(slot, chest = id, "chest is already open elsewhere");
            return Ok(());
        }

        let name = chest.name.clone();
        let (x, y, slots) = (chest.x, chest.y, chest.items.len());
        let items: Vec<_> = chest.items.clone();

        self.send(slot, objects::sync_chest_size(id, slots as i16)?);
        for (index, item) in items.iter().enumerate() {
            let frame = SyncChestItem {
                chest: id,
                slot: index as u8,
                item: *item,
            }
            .encode()?;
            self.send(slot, frame);
        }
        self.send(
            slot,
            SyncPlayerChest {
                chest: id,
                x,
                y,
                name: Some(name).filter(|n| !n.is_empty()),
            }
            .encode()?,
        );

        if let Some(player) = self.player_mut(slot) {
            player.open_chest = id;
        }
        // Everyone else's own client wants to know too, so it can show the chest as taken
        // (`NetMessage.cs:1182-1185`, `MessageBuffer.cs:1886`) rather than letting them also try
        // to open it and only find out it is refused.
        if let Ok(frame) = (SyncPlayerChestIndex {
            player: slot,
            chest: id,
        })
        .encode()
        {
            self.broadcast(frame, Some(slot));
        }
        // `WorldGen.IsChestRigged` (`WorldGen.cs:36135-36142`): tile 467 (`Containers2`), frame
        // style 4 — a wired chest that fires its own circuit the instant it is opened, exactly as
        // a lever would (`MessageBuffer.cs:1887-1893`, `Wiring.SetCurrentUser`/`HitSwitch`).
        let (rx, ry) = (i32::from(request.x), i32::from(request.y));
        let clicked = self.world.tile(rx, ry);
        if clicked.block == 467 && clicked.frame_x / 36 == 4 {
            let fired = {
                let world = &mut self.world;
                crate::world::wiring::hit_switch(world, rx, ry)
            };
            self.apply_circuit(fired, (rx, ry));
            let mut w = terrustia_proto::PacketWriter::new(id::HIT_SWITCH);
            w.i16(request.x).i16(request.y);
            if let Ok(frame) = w.finish() {
                self.broadcast(frame, Some(slot));
            }
        }
        Ok(())
    }

    /// Packet 130: an NPC pulled out of the water on a fishing line.
    ///
    /// Fishing is how three town slimes and a handful of enemies arrive, and the Red Slime is a
    /// permanent unlock — catching one for the first time means the world can spawn them from
    /// then on. Placing the NPC is the server's, since only it owns the roster.
    fn on_fished_out_npc(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let (x, y) = (i32::from(r.u16()?), i32::from(r.u16()?));
        let npc_type = r.i16()?;
        let Ok(npc_type) = u16::try_from(npc_type) else {
            return Ok(());
        };
        if !self.world.in_bounds(x, y) {
            return Ok(());
        }
        // Only what a rod can actually bring up. Without this the packet is a free spawn of
        // anything in the game, Moon Lord included.
        if !is_fishable(npc_type) {
            debug!(slot, npc_type, "that is not something you can fish out");
            return Ok(());
        }

        let at = (
            x as f32 * crate::game::npc::TILE,
            y as f32 * crate::game::npc::TILE,
        );
        if let Some(index) = self.npcs.spawn(npc_type, at) {
            self.broadcast_npc(index);
            debug!(slot, npc_type, "fished out an npc");
        }
        Ok(())
    }

    /// Packet 140: the two town slimes that are made rather than found.
    ///
    /// A Copper Slime and an Old Slime are each transformed from another slime, once per world,
    /// and the transformation is a permanent unlock. Both have to be the server's: the unlock is
    /// world state and the transformation is a roster change.
    ///
    /// `NPC.TransformElderSlime` (`NPC.cs:19172-19193`) is the whole of the Old Slime's half:
    ///
    /// ```csharp
    /// else if (!unlockedSlimeOldSpawn && Main.npc.IndexInRange(npcIndex)) {
    ///     NPC nPC = Main.npc[npcIndex];
    ///     if (nPC.type == 685) {
    ///         unlockedSlimeOldSpawn = true;
    ///         NetMessage.SendData(7);
    ///         nPC.Transform(679);
    /// ```
    ///
    /// The `!unlockedSlimeOldSpawn` guard is the point: without it the packet is a free Old Slime
    /// for every client that sends it, and the caverns keep offering a bound one for ever because
    /// nothing ever shuts `NPC.cs:2095`'s arm. The powder the client spends (`Main.cs:43852-43869`,
    /// `TryFreeingElderSlime`) is the client's own bookkeeping in vanilla too; the server checks
    /// the type and the flag and nothing else, which is transcribed rather than tightened.
    fn on_misc_event_value(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let what = r.u8()?;
        let value = r.i32()?;
        let Ok(index) = u8::try_from(value) else {
            return Ok(());
        };

        let (wanted, into) = match what {
            TRANSFORM_COPPER_SLIME => (None, COPPER_SLIME),
            // `!unlockedSlimeOldSpawn` (`NPC.cs:19178`): once per world, and the flag is what keeps
            // the caverns from offering a second bound one.
            TRANSFORM_ELDER_SLIME if self.world.progress.unlocked_slime_old => return Ok(()),
            TRANSFORM_ELDER_SLIME => (Some(OLD_SLIME_SOURCE), OLD_SLIME),
            // Case 0 is the credits roll's clock, which only ever goes the other way.
            _ => return Ok(()),
        };
        let Some(npc) = self.npcs.get(index) else {
            return Ok(());
        };
        if let Some(wanted) = wanted
            && npc.npc_type != wanted
        {
            return Ok(());
        }
        if let Some(npc) = self.npcs.get_mut(index) {
            npc.become_type(into);
            npc.dirty = true;
        }
        // `unlockedSlimeOldSpawn = true; NetMessage.SendData(7);` (`NPC.cs:19183-19184`). Nothing
        // set this before, so a freed Old Slime was forgotten the moment the world was saved and
        // the caverns went on offering bound ones beside the resident already in a house.
        //
        // The Copper Slime falls through this without effect and is meant to: its own
        // `unlockedSlimeCopperSpawn` (`NPC.cs:19205`) gates nothing that spawns, since 684 appears
        // in no ambient arm of `NPC.Spawner` at all, so there is no world state to keep.
        crate::game::rescues::remember(&mut self.world.progress, into);
        self.broadcast_npc(index);
        self.broadcast_world_data();
        Ok(())
    }

    /// Packet 141: Lucy the Axe having something to say.
    ///
    /// Pure flavour, and relayed rather than modelled — but a talking axe only its owner can hear
    /// is a talking axe nobody believes in.
    fn on_lucy_popup(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let frame = packets::verbatim(id::REQUEST_LUCY_POPUP, payload)?;
        self.broadcast(frame, Some(slot));
        Ok(())
    }

    /// Packet 85: quick stack — emptying an armful of loot into the chests it belongs in.
    ///
    /// The client offers a list of its own slots and the server decides where each goes, because
    /// only the server knows what is in chests nobody has opened and only the server can stop two
    /// players both being told the same slot was free.
    ///
    /// The client's word is taken for *which of its slots are eligible* — favourited items and
    /// coins are excluded, and that is the client's own bookkeeping — but never for where
    /// anything lands.
    fn on_quick_stack(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        use crate::world::quick_stack::{self, Destination};
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let count = r.i32()?;
        if !(0..=MAX_QUICK_STACK_SLOTS).contains(&count) {
            debug!(slot, count, "refusing an implausible quick stack");
            return Ok(());
        }
        let mut offered = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let which = r.i16()?;
            let Ok(which) = u16::try_from(which) else {
                continue;
            };
            // What the slot holds is the server's own record, not the client's claim.
            if let Some(held) = self
                .player(slot)
                .and_then(|p| p.inventory.get(&which))
                .map(|e| e.item)
                .filter(|i| !i.is_empty())
            {
                offered.push((which, held));
            }
        }
        let smart = r.bool().unwrap_or(false);
        let _ = smart; // the sorting mode; the plain rule is the same either way here

        let Some(from) = self.player(slot).map(|p| p.position) else {
            return Ok(());
        };
        // A chest somebody has open is off limits, which is what stops a quick stack landing in
        // the middle of somebody else's rummaging.
        let open: Vec<i16> = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.slot != slot)
            .map(|p| p.open_chest)
            .filter(|id| *id >= 0)
            .collect();
        let mut destinations: Vec<Destination> = self
            .world
            .chests
            .iter()
            .enumerate()
            .filter_map(|(id, c)| c.as_ref().map(|c| (id as i16, c)))
            .map(|(id, c)| Destination {
                id,
                position: (
                    f32::from(c.x) * crate::game::npc::TILE + crate::game::npc::TILE,
                    f32::from(c.y) * crate::game::npc::TILE + crate::game::npc::TILE,
                ),
                items: c.items.clone(),
                blocked: open.contains(&id),
            })
            .collect();

        let outcome = quick_stack::run(from, &offered, &mut destinations);
        if outcome.moves.is_empty() && outcome.blocked.is_empty() {
            return Ok(());
        }

        // Write the results back and tell everybody: the chests to everyone, since anyone may
        // have one open, and the player's own slots to the player.
        for movement in &outcome.moves {
            if let Some(Some(chest)) = self
                .world
                .chests
                .get_mut(usize::try_from(movement.chest).unwrap_or(usize::MAX))
                && let Some(cell) = chest.items.get_mut(movement.chest_slot)
            {
                *cell = movement.chest_now;
            }
            let frame = SyncChestItem {
                chest: movement.chest,
                slot: movement.chest_slot as u8,
                item: movement.chest_now,
            }
            .encode()?;
            self.broadcast(frame, None);

            if let Some(player) = self.player_mut(slot)
                && let Some(held) = player.inventory.get_mut(&movement.from_slot)
            {
                held.item = movement.left_behind;
            }
        }
        // One equipment packet per slot that changed, rather than one per move: a stack split
        // across three chests changed once from the player's point of view.
        let mut told = std::collections::HashSet::new();
        for movement in &outcome.moves {
            if !told.insert(movement.from_slot) {
                continue;
            }
            if let Some(equip) = self
                .player(slot)
                .and_then(|p| p.inventory.get(&movement.from_slot))
                .copied()
                && let Ok(frame) = equip.encode()
            {
                self.broadcast(frame, None);
            }
        }

        // Which chests refused, so the client can mark them.
        if !outcome.blocked.is_empty() {
            let mut w = terrustia_proto::PacketWriter::new(id::QUICK_STACK_CHESTS);
            w.i32(outcome.blocked.len() as i32);
            for chest in &outcome.blocked {
                w.u16(*chest as u16);
            }
            if let Ok(frame) = w.finish() {
                self.send(slot, frame);
            }
        }
        debug!(
            slot,
            moved = outcome.moves.len(),
            blocked = outcome.blocked.len(),
            "quick stack"
        );
        Ok(())
    }

    /// Packet 39: a player giving up their claim on an item.
    ///
    /// A dropped item is reserved for whoever is nearest so two players cannot both grab it. This
    /// is the other half of that: a player whose inventory is full, or who simply walked past,
    /// releases the claim so somebody else can have it. Without it a full player standing over a
    /// pile locks all of it for as long as they stand there.
    fn on_release_item(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let index = r.i16()?;
        let _force_to_server = r.bool()?;

        let Some(item) = self.items.get_mut(index) else {
            return Ok(());
        };
        // Only the holder may release it, or one client could free another's claim from under it.
        if item.owner != slot {
            return Ok(());
        }
        item.owner = items::NO_OWNER;
        item.reservation = 0;
        let position = item.position;
        // Told to everyone: the next tick will offer it to whoever is nearest, and until then no
        // client should believe it is spoken for.
        if let Ok(frame) = ItemOwner::reserve(index, items::NO_OWNER, position).encode() {
            self.broadcast(frame, None);
        }
        Ok(())
    }

    /// Packet 95: closing somebody else's portal.
    ///
    /// The Portal Gun's two ends are projectiles. Firing a third replaces the oldest, and the
    /// client that owns them says which one to close — because it is the one that knows which of
    /// its own pair is which.
    fn on_close_portal(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let owner = r.u16()?;
        let which = f32::from(r.u8()?);
        // The owner named has to be the sender, or one player could close another's portals.
        if usize::from(owner) != usize::from(slot) {
            return Ok(());
        }

        let found = self
            .projectiles
            .iter()
            .find(|(_, p)| {
                p.projectile_type == PORTAL_PROJECTILE && p.key.owner == slot && p.ai[1] == which
            })
            .map(|(index, p)| (index, p.key, p.position));
        let Some((index, key, position)) = found else {
            return Ok(());
        };
        self.projectiles.remove(index);
        let kill = terrustia_proto::projectile::KillProjectile { key, position };
        if let Ok(frame) = kill.encode() {
            self.broadcast(frame, None);
        }
        Ok(())
    }

    /// Packet 96: a player stepping through a portal.
    ///
    /// The client works out where it comes out — it knows where both ends are and how it entered
    /// — and the server's job is to agree and tell everybody else. Refusing would desync the one
    /// client that has already moved.
    fn on_portal_teleport(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let _claimed = r.u8()?;
        let colour = r.i16()?;
        let (x, y) = r.vec2()?;
        let velocity = r.vec2()?;
        if !x.is_finite() || !y.is_finite() || !velocity.0.is_finite() || !velocity.1.is_finite() {
            return Ok(());
        }
        // A portal only reaches as far as its other end, which the server can bound even without
        // knowing where that is: nothing on the map is further than the map.
        if !self.world.in_bounds(
            (x / crate::game::npc::TILE) as i32,
            (y / crate::game::npc::TILE) as i32,
        ) {
            return Ok(());
        }
        if let Some(player) = self.player_mut(slot) {
            player.position = (x, y);
            player.velocity = velocity;
        }
        let mut w = terrustia_proto::PacketWriter::new(id::TELEPORT_PLAYER_THROUGH_PORTAL);
        w.u8(slot)
            .i16(colour)
            .f32(x)
            .f32(y)
            .f32(velocity.0)
            .f32(velocity.1);
        let frame = w.finish()?;
        self.broadcast(frame, Some(slot));
        Ok(())
    }

    /// Packet 102: a Nebula armour booster being picked up.
    ///
    /// Purely a relay — the effect is each client's own — but without it nobody else sees the
    /// burst, and a booster picked up in a group looks to everyone else like nothing happened.
    fn on_nebula_booster(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let _claimed = r.u8()?;
        let kind = r.u16()?;
        let at = r.vec2()?;
        let mut w = terrustia_proto::PacketWriter::new(id::NEBULA_LEVELUP_REQUEST);
        w.u8(slot).u16(kind).f32(at.0).f32(at.1);
        let frame = w.finish()?;
        self.broadcast(frame, None);
        Ok(())
    }

    /// Packet 92: coins an NPC is carrying beyond its own worth.
    ///
    /// This is the Coin Loss revenge system: money dropped on death is remembered against
    /// whatever killed you, and killing that back gives it up. The server accumulates rather than
    /// overwrites, because two players can both feed the same enemy.
    fn on_extra_value(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let index = r.i16()?;
        let extra = r.i32()?;
        let at = r.vec2()?;
        let Ok(index) = u8::try_from(index) else {
            return Ok(());
        };
        let Some(npc) = self.npcs.get_mut(index) else {
            return Ok(());
        };
        npc.extra_value = npc.extra_value.saturating_add(extra);
        let total = npc.extra_value;
        let mut w = terrustia_proto::PacketWriter::new(id::SYNC_EXTRA_VALUE);
        w.i16(i16::from(index)).i32(total).f32(at.0).f32(at.1);
        let frame = w.finish()?;
        self.broadcast(frame, None);
        Ok(())
    }

    /// Packet 143: a player asking the Old One's Army to send the next wave early.
    ///
    /// The gap between waves is generous on purpose, and skipping it is how a group that is
    /// ready gets on with it. Refused unless the event is actually waiting.
    fn on_skip_army_wait(&mut self, slot: u8) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        if let Some(left) = self.army.skip_wait() {
            self.broadcast_army_wait(left);
            debug!(slot, "the next army wave was called early");
        }
        Ok(())
    }

    /// Tell clients how long is left before the next wave comes through the gates.
    ///
    /// The countdown on screen is this and nothing else. Without it the gap between waves is a
    /// blank pause of unknown length, which is exactly the part of the event a group needs to
    /// plan around.
    pub(super) fn broadcast_army_wait(&mut self, ticks: i32) {
        let mut w = terrustia_proto::PacketWriter::new(id::CRYSTAL_INVASION_SEND_WAIT_TIME);
        w.i32(ticks);
        if let Ok(frame) = w.finish() {
            self.broadcast(frame, None);
        }
    }

    /// Packet 144: the Dryad's little animation when a quest is handed in.
    ///
    /// Nothing but a flourish, and relayed rather than modelled — but a flourish only one client
    /// can see is worse than none.
    fn on_quest_effect(&mut self, slot: u8) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let frame = packets::empty(id::REQUEST_QUEST_EFFECT)?;
        self.broadcast(frame, None);
        Ok(())
    }

    /// Packet 109: the Grand Design, run along a path.
    ///
    /// Every wiring tool past the first works this way, and it has to be the server's job: the
    /// client does not know how much wire the player has left, and a run that stops halfway has
    /// to stop at the same tile for everybody or two players see different circuits.
    ///
    /// The reply is packet 110 — how much was actually spent — which is what stops a client
    /// believing it still has wire the server has already used.
    fn on_mass_wire(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        use crate::world::mass_wire::{self, Supplies, ToolMode};
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let from = (i32::from(r.i16()?), i32::from(r.i16()?));
        let to = (i32::from(r.i16()?), i32::from(r.i16()?));
        let mode = ToolMode(r.u8()?);
        if !mode.does_anything() {
            return Ok(());
        }

        // A drag across the whole world would be a denial of service dressed as a wiring tool.
        let span = (to.0 - from.0).abs().max((to.1 - from.1).abs());
        if span > MAX_WIRE_DRAG {
            debug!(slot, span, "refusing an implausibly long wire drag");
            return Ok(());
        }

        let Some(player) = self.player(slot) else {
            return Ok(());
        };
        let supplies = Supplies {
            wire: count_held(player, WIRE_ITEM),
            actuators: count_held(player, ACTUATOR_ITEM),
        };
        let facing_right = player.facing_right;

        let outcome = mass_wire::run(&mut self.world, from, to, mode, supplies, facing_right);
        for change in &outcome.changes {
            let edit = TileManipulation {
                action: change.action,
                x: change.x as i16,
                y: change.y as i16,
                arg: 0,
                style: 0,
            };
            if let Ok(frame) = edit.encode() {
                self.broadcast(frame, None);
            }
        }

        self.spawn_wire_drops(&outcome.drops);

        // Tell the player what it cost. Both are sent even when zero, as the game sends both.
        for (item, spent) in [
            (WIRE_ITEM, outcome.wire_spent),
            (ACTUATOR_ITEM, outcome.actuators_spent),
        ] {
            let mut w = terrustia_proto::PacketWriter::new(id::MASS_WIRE_OPERATION_PAY);
            w.i16(item).i16(spent as i16).u8(slot);
            if let Ok(frame) = w.finish() {
                self.send(slot, frame);
            }
        }
        debug!(
            slot,
            wire = outcome.wire_spent,
            actuators = outcome.actuators_spent,
            tiles = outcome.changes.len(),
            "mass wire operation"
        );
        Ok(())
    }

    /// Packet 69: a client asking what a chest is called.
    ///
    /// Sent for the map, which shows a chest's name without opening it. Answered to the asker
    /// alone, since another client that has not looked at the chest has no use for it.
    fn on_chest_name_request(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let claimed = r.i16()?;
        let (x, y) = (r.i16()?, r.i16()?);

        // A client may name the chest by id or ask the server to find it by position.
        let found = if claimed == -1 {
            self.world.chest_at(x, y)
        } else {
            self.world
                .chests
                .get(usize::try_from(claimed).unwrap_or(usize::MAX))
                .and_then(|c| c.as_ref())
                .filter(|c| c.x == x && c.y == y)
                .map(|c| (claimed, c))
        };
        let Some((id, chest)) = found else {
            return Ok(());
        };
        let mut w = terrustia_proto::PacketWriter::new(id::CHEST_NAME);
        w.i16(id).i16(x).i16(y).string(&chest.name);
        let frame = w.finish()?;
        self.send(slot, frame);
        Ok(())
    }

    /// Packet 105: locking or unlocking a gem lock.
    ///
    /// A tile toggle rather than a wiring one, but the effect is a circuit's: a locked gem lock
    /// is what a Chlorophyte Extractinator run is wired to.
    fn on_gem_lock(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        /// The gem each gem-lock style holds, indexed by `frameX / 54`.
        ///
        /// `WorldGen.ToggleGemLock`'s own `switch (num2)` (`WorldGen.cs:46893-46915`): amethyst,
        /// topaz, sapphire, emerald, ruby, diamond, amber.
        const GEM_LOCK_GEMS: [i32; 7] = [1526, 1524, 1525, 1523, 1522, 1527, 3643];

        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let (x, y) = (i32::from(r.i16()?), i32::from(r.i16()?));
        let lock = r.bool()?;
        if !self.world.in_bounds(x, y) {
            return Ok(());
        }
        let tile = self.world.tile(x, y);
        if !tile.is_active() || tile.block != GEM_LOCK {
            return Ok(());
        }
        // A gem lock is a three-by-three object whose locked state lives in the frame's Y band:
        // the lower half of the sprite sheet is the locked form.
        let origin_y = tile.frame_y % GEM_LOCK_FRAME_HEIGHT;
        let wanted = if lock {
            origin_y + GEM_LOCK_FRAME_HEIGHT
        } else {
            origin_y
        };
        if tile.frame_y == wanted {
            return Ok(());
        }
        let (ox, oy) = (
            x - i32::from(tile.frame_x % 54) / 18,
            y - i32::from(origin_y) / 18,
        );
        for dx in 0..3 {
            for dy in 0..3 {
                let (tx, ty) = (ox + dx, oy + dy);
                let mut cell = self.world.tile(tx, ty);
                if !cell.is_active() || cell.block != GEM_LOCK {
                    continue;
                }
                cell.frame_y = if lock {
                    cell.frame_y % GEM_LOCK_FRAME_HEIGHT + GEM_LOCK_FRAME_HEIGHT
                } else {
                    cell.frame_y % GEM_LOCK_FRAME_HEIGHT
                };
                self.world.set_tile(tx, ty, cell);
            }
        }
        // Two tiles of reach covers the whole three-by-three from its centre. Vanilla's own
        // `NetMessage.SendTileSquare(-1, i - num3, j - num4, 3, 3)` (`WorldGen.cs:46929`); case 105
        // is never relayed, so this square is the only thing that tells the other clients.
        self.push_region(ox + 1, oy + 1, 2);

        // The gem comes back out when the lock opens. `WorldGen.ToggleGemLock`'s own
        // `if ((num != -1) & flag) Item.NewItem(..., i * 16, j * 16, 32, 32, num)`
        // (`WorldGen.cs:46925-46928`), with `flag` set when the lock was standing locked - which,
        // given the state-change guard above, is exactly the unlocking direction. Without it,
        // unlocking a gem lock destroyed the gem that was in it.
        if !lock
            && let Some(&gem) = GEM_LOCK_GEMS.get(usize::try_from(tile.frame_x / 54).unwrap_or(7))
        {
            let at = (x as f32 * 16.0 + 16.0, y as f32 * 16.0 + 16.0);
            self.spawn_item(terrustia_proto::ItemStack::new(gem, 1, 0), at);
        }

        // And the whole point of a gem lock: it is a switch. `WorldGen.cs:46930-46931` ends with
        // `Wiring.HitSwitch(i - num3, j - num4)` and `NetMessage.SendData(59, -1, -1, null, i -
        // num3, j - num4)`, both against the object's own corner rather than the clicked tile.
        self.fire_switch(ox, oy);
        let mut w = terrustia_proto::PacketWriter::new(id::HIT_SWITCH);
        w.i16(ox as i16).i16(oy as i16);
        let frame = w.finish()?;
        // To everybody: no client sent this one, the packet-105 click did.
        self.broadcast(frame, None);
        Ok(())
    }

    /// Packet 32: a client changing one chest slot.
    fn on_chest_item(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let sync = SyncChestItem::decode(payload)?;

        // Only the player who has the chest open may change it.
        if self.player(slot).map(|p| p.open_chest) != Some(sync.chest) {
            debug!(
                slot,
                chest = sync.chest,
                "rejecting edit to a chest that is not open"
            );
            return Ok(());
        }

        let Some(chest) = self.world.chest_mut(sync.chest) else {
            return Ok(());
        };
        let Some(cell) = chest.items.get_mut(usize::from(sync.slot)) else {
            return Ok(());
        };
        *cell = sync.item;

        self.broadcast(sync.encode()?, Some(slot));
        Ok(())
    }

    /// Packet 33: a client reporting which chest it has open, including closing one, and the one
    /// place a chest is ever renamed.
    ///
    /// The name field was decoded and thrown away. It is not decoration: `MessageBuffer.cs:
    /// 3162-3169`'s own `else` branch (the `netMode == 2` half) is the *only* write path for a
    /// chest's name in the whole game. Packet 69's server branch (`:3082-3095`) is a read request
    /// and nothing else - which `on_chest_name_request` already implements correctly - so with
    /// this half missing a chest could never be named at all and packet 69 was never broadcast.
    ///
    /// Vanilla names the chest the sender had open *before* this packet (`Main.player[whoAmI]
    /// .chest`, read before the assignment below it), not the one it names, which is what makes
    /// "type a name, then close the chest" work.
    fn on_player_chest(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let sync = SyncPlayerChest::decode(payload)?;
        let was_open = self.player(slot).map_or(-1, |p| p.open_chest);
        if let Some(name) = sync.name
            && let Some(chest) = self.world.chest_mut(was_open)
        {
            let (x, y) = (chest.x, chest.y);
            chest.name = name.clone();
            // `NetMessage.TrySendData(69, -1, whoAmI, null, chest3, chest4.x, chest4.y)`
            // (`MessageBuffer.cs:3166`): everyone else is told the new name.
            let mut w = terrustia_proto::PacketWriter::new(id::CHEST_NAME);
            w.i16(was_open).i16(x).i16(y).string(&name);
            let frame = w.finish()?;
            self.broadcast(frame, Some(slot));
        }
        if let Some(player) = self.player_mut(slot) {
            player.open_chest = sync.chest;
        }
        // `NetMessage.TrySendData(80, -1, whoAmI, null, whoAmI, num21)` (`MessageBuffer.cs:3168`):
        // the other clients need to be told when this player *stops* having a chest open, not only
        // when they start (which `on_chest_open` already sends). Without it a chest stays shown as
        // in use on every other screen for the rest of the session.
        let frame = terrustia_proto::objects::SyncPlayerChestIndex {
            player: slot,
            chest: sync.chest,
        }
        .encode()?;
        self.broadcast(frame, Some(slot));
        Ok(())
    }

    /// Packet 46: a client asking to read a sign.
    fn on_sign_request(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let request = RequestSign::decode(payload)?;
        let Some((id, sign)) = self.world.sign_at(request.x, request.y) else {
            return Ok(());
        };
        let frame = SignText {
            sign: id,
            x: sign.x,
            y: sign.y,
            text: sign.text.clone(),
            player: slot,
            editing: 0,
        }
        .encode()?;
        self.send(slot, frame);
        Ok(())
    }

    /// Packet 47: a client writing a sign.
    fn on_sign_write(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut update = SignText::decode(payload)?;
        if update.text.len() > MAX_SIGN_TEXT {
            debug!(slot, len = update.text.len(), "sign text too long");
            return Ok(());
        }

        let id = match self.world.sign_at(update.x, update.y) {
            Some((id, _)) => {
                if let Some(sign) = self.world.sign_mut(id) {
                    sign.text = update.text.clone();
                }
                id
            }
            None => {
                let sign = Sign {
                    x: update.x,
                    y: update.y,
                    text: update.text.clone(),
                };
                match self.world.add_sign(sign) {
                    Some(id) => id,
                    None => return Ok(()),
                }
            }
        };

        update.sign = id;
        update.player = slot;
        update.editing = 0;
        self.broadcast(update.encode()?, Some(slot));
        Ok(())
    }

    fn on_net_module(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        // Module 8: "take me to that pylon". Checked before chat because `IncomingChat::decode`
        // returns `None` for it and the request would otherwise be dropped on the floor.
        if let Some((message, pylon)) = net_module::decode_pylon_message(payload)?
            && message == net_module::PylonMessage::RequestTeleport
        {
            return self.on_pylon_teleport(slot, pylon);
        }

        // Module 4: Journey mode powers. Same reason as module 8 above — neither is chat, and
        // `IncomingChat::decode` would return `None` for it too, so it has to be checked first.
        if let Some(message) = net_module::decode_creative_power(payload)? {
            return self.on_creative_power(slot, message);
        }

        // Modules 9 (particles) and 2 (ping): vanilla's own dedicated-server branches for both
        // just re-broadcast the received frame to every other client
        // (`NetParticlesModule.cs:22-25`, `NetPingModule.cs:19-22`) — this server has no opinion
        // about the contents of either, only that they reach everyone else.
        let module_id = net_module::peek_module_id(payload)?;
        if module_id == net_module::MODULE_PARTICLES || module_id == net_module::MODULE_PING {
            if !self.player(slot).is_some_and(Player::is_playing) {
                return Ok(());
            }
            self.broadcast(net_module::relay_module(payload)?, Some(slot));
            return Ok(());
        }

        // Module 12: craft using whatever a nearby chest can cover.
        if let Some(request) = net_module::decode_craft_request(payload)? {
            return self.on_craft_request(slot, request);
        }

        let Some(chat) = IncomingChat::decode(payload)? else {
            return Ok(());
        };
        if !self.player(slot).is_some_and(Player::is_playing) || !chat.is_say() {
            return Ok(());
        }
        if net_module::validate_chat(&chat.text, self.config.max_chat_len).is_err() {
            debug!(
                slot,
                len = chat.text.len(),
                "dropping out-of-range chat line"
            );
            return Ok(());
        }

        if let Some(command) = chat.text.strip_prefix('/') {
            return self.run_command(slot, command);
        }

        let name = self
            .player(slot)
            .map(|p| p.name.clone())
            .unwrap_or_default();

        // The per-account chat cooldown — `Config::chat_cooldown_ms`, off by default (`0`). Checked
        // (and, on acceptance, its clock updated) before the mute check: it is about the *pace* a
        // connection is sending at, independent of whether the line ends up shadow-muted below.
        // Dropped silently rather than told to the sender — a cooldown that announced itself would
        // just teach a determined spammer exactly how fast they are allowed to go.
        if self.config.chat_cooldown_ms > 0 {
            let cooldown = Duration::from_millis(self.config.chat_cooldown_ms);
            let now = Instant::now();
            if let Some(player) = self.player(slot)
                && player
                    .last_chat
                    .is_some_and(|last| now.duration_since(last) < cooldown)
            {
                return Ok(());
            }
            if let Some(player) = self.player_mut(slot) {
                player.last_chat = Some(now);
            }
        }

        // The text goes out bare, with the author's slot beside it. The client adds the name
        // itself — `ChatHelper.DisplayMessage` prefixes `Main.player[author].name` whenever the
        // author is a real slot — so a server that helpfully prefixes it too has every line
        // rendered with the speaker's name twice, and puts the tag inside the speech bubble over
        // their head as well. Found by asking a real server to relay a line and comparing: it
        // sends `"provoke: hello"` where this sent `"<provoke-actor> provoke: hello"`.
        let frame = net_module::chat_broadcast(
            slot,
            &NetworkText::literal(chat.text.clone()),
            [255, 255, 255],
        )?;

        if self.admin.is_muted(&name) {
            // Shadow-mute: the muted player still sees their own line go out (nothing about the
            // client-visible behaviour changes, so muting is not obviously detectable and a muted
            // player cannot simply try a different client/workaround having learned they are
            // muted), staff see it flagged in the console/live feed, and nobody else receives
            // anything at all.
            info!(target: crate::term::CHAT_TARGET, "<{name}> {} [MUTED]", chat.text);
            self.send(slot, frame);

            // Escalation — `Config::mute_escalation_enabled`, off by default: a still-muted player
            // who keeps talking has their mute pushed further out each time, capped at
            // `mute_escalation_max_secs`. Attributed to `"system"`, since the extension itself was
            // nobody's direct instruction — the original mute's own issuer is unchanged.
            if self.config.mute_escalation_enabled
                && let Some(new_until) = self.admin.extend_mute(
                    &name,
                    self.config.mute_escalation_secs,
                    self.config.mute_escalation_max_secs,
                )
            {
                self.audit.record(
                    "system",
                    crate::admin::AuditAction::Mute,
                    &name,
                    &format!("escalated, now until unix {new_until}"),
                );
            }
            return Ok(());
        }

        // Tagged so the web panel's live feed can tell an in-game chat line apart from an
        // operational one — both are `info!`, and only the target says which is which. The console
        // line above (the muted branch) keeps its own `<name>` for the same reason.
        info!(target: crate::term::CHAT_TARGET, "<{name}> {}", chat.text);
        self.broadcast(frame, None);
        Ok(())
    }

    /// Module 12: craft a recipe using items in a nearby, currently-open-by-nobody-else chest.
    ///
    /// The client has already taken what it could reach on its own (its inventory, and any bank
    /// chest) and is asking this server to cover the rest from specific chests it can see —
    /// server-authoritative because two players quick-crafting from the same chest at once must
    /// not both be told the same stack was theirs (`CraftingRequests.HandleRequest`,
    /// `CraftingRequests.cs:308-321`). Approval is all-or-nothing: every requested entry must be
    /// fully covered by the usable chests or the whole request is denied, never partially filled.
    fn on_craft_request(
        &mut self,
        slot: u8,
        request: net_module::CraftRequest,
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }

        // `chests.RemoveAll(chest == null || !CanCraftFromChest(chest, whoAmI))`
        // (`CraftingRequests.cs:310`) — a stale `None` index drops the same way a null `Chest`
        // does, and the client's own chest order is preserved for the consume pass below.
        let usable: Vec<i16> = request
            .chests
            .into_iter()
            .flatten()
            .filter(|&id| {
                self.world
                    .chest(id)
                    .is_some_and(|chest| self.can_craft_from_chest(id, chest, slot))
            })
            .collect();

        // `items.All(req => CountMatches(req, chests) >= req.stack)` (`CraftingRequests.cs:311`).
        // A `RecipeGroup` entry can never be confirmed against real chest contents — this server
        // has no `RecipeGroup` table (see `CraftIngredient::is_recipe_group`'s own doc) — so it is
        // always treated as unavailable, which denies the whole request exactly the way a real
        // shortfall would rather than fabricating an approval this server cannot actually verify.
        let covered = request.items.iter().all(|entry| {
            !entry.is_recipe_group()
                && self.chest_stock(&usable, entry.item_id_or_group) >= i64::from(entry.stack)
        });

        if !covered {
            self.send(slot, net_module::craft_response(false)?);
            debug!(slot, chests = usable.len(), "craft request denied");
            return Ok(());
        }

        for entry in &request.items {
            self.consume_from_chests(&usable, entry.item_id_or_group, i64::from(entry.stack));
        }
        self.send(slot, net_module::craft_response(true)?);
        Ok(())
    }

    /// `CraftingRequests.CanCraftFromChest` (`CraftingRequests.cs:294-306`): not locked, and not
    /// open by anybody other than the requester.
    fn can_craft_from_chest(
        &self,
        id: i16,
        chest: &crate::world::objects::Chest,
        requester: u8,
    ) -> bool {
        // `Chest.IsLocked` treats a null tile as locked (`Chest.cs:297-300`); a chest recorded
        // outside the world (should never happen) is refused the same way.
        if !self.world.in_bounds(i32::from(chest.x), i32::from(chest.y)) {
            return false;
        }
        if is_chest_tile_locked(self.world.tile(i32::from(chest.x), i32::from(chest.y))) {
            return false;
        }
        // `Chest.UsingChest` (`Chest.cs:492-505`): in use by somebody who is not the requester.
        !self
            .players
            .iter()
            .flatten()
            .any(|p| p.slot != requester && p.open_chest == id)
    }

    /// `CraftingRequests.CountMatches` (`CraftingRequests.cs:199-207`), summed only over the
    /// already-filtered, already-usable chest list.
    fn chest_stock(&self, chests: &[i16], item_id: i32) -> i64 {
        chests
            .iter()
            .filter_map(|&id| self.world.chest(id))
            .flat_map(|chest| chest.items.iter())
            .filter(|item| item.id == item_id)
            .map(|item| i64::from(item.stack))
            .sum()
    }

    /// `CraftingRequests.Consume`/`ConsumeItemsFrom` (`CraftingRequests.cs:223-292`), the
    /// dedicated-server shape: no player inventory involved (that only happens on
    /// `Main.netMode != 2`), every chest in the list eligible (`fromChests: true`). Each slot that
    /// loses stock is told to every client (`NetMessage.SendData(32, ...)`,
    /// `CraftingRequests.cs:285`), whether it emptied outright or only shrank.
    fn consume_from_chests(&mut self, chests: &[i16], item_id: i32, mut to_consume: i64) {
        for &chest_id in chests {
            if to_consume <= 0 {
                return;
            }
            let Some(chest) = self.world.chest_mut(chest_id) else {
                continue;
            };
            let mut touched: Vec<(u8, ItemStack)> = Vec::new();
            for (index, item) in chest.items.iter_mut().enumerate() {
                if to_consume <= 0 {
                    break;
                }
                if item.id != item_id {
                    continue;
                }
                let held = i64::from(item.stack);
                if held > to_consume {
                    item.stack -= to_consume as i16;
                    to_consume = 0;
                } else {
                    to_consume -= held;
                    *item = ItemStack::EMPTY;
                }
                touched.push((index as u8, *item));
            }
            for (index, item) in touched {
                if let Ok(frame) = (SyncChestItem {
                    chest: chest_id,
                    slot: index,
                    item,
                })
                .encode()
                {
                    self.broadcast(frame, None);
                }
            }
        }
    }

    /// A client asking to be taken to a pylon.
    ///
    /// The game runs five checks in order before it will carry anyone
    /// (`TeleportPylonsSystem.HandleTeleportRequest`, TeleportPylonsSystem.cs:100-207): you are
    /// standing near a pylon (`IsPlayerNearAPylon`, :107); the one you are going to has the two
    /// townsfolk it needs, all but the Victory pylon (`HowManyNPCsDoesPylonNeed`, :314); the
    /// Lihzahrd temple is not being reached before Plantera (:124); the destination pylon still
    /// sits in its own biome (`DoesPylonAcceptTeleportation`, :139); and at least one pylon you are
    /// near is itself working, with its own townsfolk and its own biome, not merely present (the
    /// source loop, :145-188). This handler used to run only the first two, because the three that
    /// were missing all lean on a biome scan the server did not have until L2-11 landed one.
    fn on_pylon_teleport(
        &mut self,
        slot: u8,
        pylon: net_module::Pylon,
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let known = self.pylons();
        let Some(&destination) = known.iter().find(|p| p.x == pylon.x && p.y == pylon.y) else {
            debug!(slot, x = pylon.x, y = pylon.y, "no pylon there");
            return Ok(());
        };

        let Some(player) = self.player(slot) else {
            return Ok(());
        };
        let at = (player.position.0 / 16.0, player.position.1 / 16.0);

        // (1) You are standing near a pylon at all (`IsPlayerNearAPylon`, TeleportPylonsSystem.cs:107).
        // The pylons within reach are kept, because check (5) has to know which of them are working.
        let near: Vec<net_module::Pylon> = known
            .iter()
            .copied()
            .filter(|p| {
                (f32::from(p.x) - at.0).abs() <= PYLON_REACH
                    && (f32::from(p.y) - at.1).abs() <= PYLON_REACH
            })
            .collect();
        if near.is_empty() {
            self.tell(slot, "You need to be near a pylon to travel.");
            return Ok(());
        }

        // (2) The destination has the two townsfolk living near it that it needs; the Victory pylon
        //     needs none (`DoesPylonHaveEnoughNPCsAroundIt`/`HowManyNPCsDoesPylonNeed`,
        //     TeleportPylonsSystem.cs:116/314).
        if destination.kind != net_module::Pylon::VICTORY
            && self.town_npcs_near(destination.x, destination.y) < PYLON_RESIDENTS_NEEDED
        {
            self.tell(
                slot,
                "That pylon needs two townsfolk living near it before it will work.",
            );
            return Ok(());
        }

        // (3) The Lihzahrd temple stays sealed to its pylon until Plantera falls
        //     (TeleportPylonsSystem.cs:124).
        if self.temple_pylon_sealed(&destination) {
            self.tell(
                slot,
                "The temple's pylon will not answer until Plantera is defeated.",
            );
            return Ok(());
        }

        // (4) The destination pylon still sits in the biome its network belongs to
        //     (`DoesPylonAcceptTeleportation`, TeleportPylonsSystem.cs:139/254).
        if !self.pylon_accepts(&destination) {
            self.tell(
                slot,
                "That pylon is no longer in the right biome to travel to.",
            );
            return Ok(());
        }

        // (5) At least one of the pylons you are near is itself a working source: it has its own
        //     townsfolk and matches its own biome (the source loop, TeleportPylonsSystem.cs:145-188).
        //     Being near a broken pylon is not enough to leave from.
        let source_ready = near.iter().any(|p| {
            (p.kind == net_module::Pylon::VICTORY
                || self.town_npcs_near(p.x, p.y) >= PYLON_RESIDENTS_NEEDED)
                && self.pylon_accepts(p)
        });
        if !source_ready {
            self.tell(
                slot,
                "The pylon you are standing at is not working: it needs its townsfolk and its own biome.",
            );
            return Ok(());
        }

        // Land on the pylon's own tile, as the game does. `info.PositionInTiles
        // .ToWorldCoordinates()` (`TeleportPylonsSystem.cs:186`), and `ToWorldCoordinates`
        // (`Utils.cs:1857`) is `p.ToVector2() * 16f + new Vector2(autoAddX, autoAddY)` with both
        // defaulting to 8 - a half tile, on both axes, which this was dropping. Vanilla's own
        // `- new Vector2(0f, player.HeightOffsetBoost)` is not transcribed: this server has no
        // height-offset model, and the boost is zero for an ordinary player.
        let to = (
            f32::from(destination.x) * 16.0 + 8.0,
            f32::from(destination.y) * 16.0 + 8.0,
        );
        if let Some(player) = self.player_mut(slot) {
            player.position = to;
            player.velocity = (0.0, 0.0);
            // Server-decided, same as a Teleportation Potion — see `on_server_teleport`'s own
            // comment on why this owes the client an acknowledgement round trip.
            player.unacknowledged_teleports += 1;
        }
        // Style 9 is the pylon's own animation; the extra value picks the colour by network.
        let mut w = terrustia_proto::PacketWriter::new(id::TELEPORT_ENTITY);
        w.u8(0x08) // the fourth bit says an extra value follows
            .i16(i16::from(slot))
            .f32(to.0)
            .f32(to.1)
            .u8(9)
            .i32(i32::from(destination.kind));
        let frame = w.finish()?;
        self.broadcast(frame, None);
        debug!(slot, x = destination.x, y = destination.y, "pylon travel");
        Ok(())
    }

    /// Packet 21: a client dropping something, or updating an item it holds the reservation on.
    fn on_sync_item(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let sync = SyncItem::decode(payload)?;

        if sync.is_new() {
            // Through `take_item_slot` rather than the store directly: a client asking for a slot
            // gets one the same way a server-side drop does, recycled item and its `151` included.
            // Vanilla routes this branch back through `Item.NewItem` itself for that very reason
            // (`MessageBuffer.cs:1501-1505`, case 21).
            let Some(index) = self.take_item_slot(sync.item, sync.position) else {
                return Ok(());
            };
            if let Some(item) = self.items.get_mut(index) {
                item.velocity = sync.velocity;
                // A player throwing an item keeps first claim on it.
                item.owner = slot;
                item.reservation = items::RESERVATION_TICKS;
            }
            self.broadcast_item(index);
            return Ok(());
        }

        // Otherwise only the reserving player may move it.
        match self.items.get_mut(sync.index) {
            Some(item) if item.owner == slot => {
                item.item = sync.item;
                item.position = sync.position;
                item.velocity = sync.velocity;
                item.resting = false;
            }
            _ => {
                debug!(
                    slot,
                    index = sync.index,
                    "ignoring item update from a non-owner"
                );
                return Ok(());
            }
        }

        self.broadcast(sync.encode()?, Some(slot));
        Ok(())
    }

    /// Packet 151: a client reporting that it picked an item up.
    fn on_item_despawn(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        let index = decode_item_despawn(payload)?;
        match self.items.get(index) {
            Some(item) if item.owner == slot => {}
            _ => {
                debug!(
                    slot,
                    index, "ignoring pickup of an item reserved for someone else"
                );
                return Ok(());
            }
        }
        self.items.remove(index);
        self.broadcast(terrustia_proto::items::item_despawn(index)?, Some(slot));
        Ok(())
    }

    /// Pass a player's own projectile on to everyone else.
    ///
    /// The server does not simulate player weapons, so a client's arrows are relayed rather than
    /// re-created. Two checks come straight from the game: the projectile has to claim the sender
    /// as its owner, and it must not be a hostile type — otherwise a modified client could conjure
    /// a demon scythe and blame the server for it.
    pub(super) fn on_client_projectile(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        let sync = terrustia_proto::projectile::SyncProjectile::decode(payload)?;
        if sync.key.owner != slot {
            debug!(
                slot,
                owner = sync.key.owner,
                "dropping a mis-owned projectile"
            );
            return Ok(());
        }
        // A client may sync what it fired itself, but never something that would hurt other
        // players: that is the server's decision, not a claim. Vanilla refuses the same thing.
        let hostile =
            terrustia_proto::projectile_data::projectile_stats(sync.projectile_type as u16)
                .is_some_and(|stats| stats.hostile);
        if hostile {
            debug!(
                slot,
                projectile = sync.projectile_type,
                "dropping a hostile projectile from a client"
            );
            return Ok(());
        }
        // Purification Powder is the one throw whose effect the *server* settles rather than the
        // thrower: `Projectile.Damage_TryUsingPowders` (`Projectile.cs:14787-14826`) runs under
        // `Main.netMode != 1`, so in a real game it is the server that watches the cloud and turns
        // a Tortured Soul into the Tax Collector, or a bound Yellow Slime into a Yellow Slime, when
        // it touches one. Nothing about that arrives as a claim from the client, which is why the
        // cloud has to be followed here rather than waited for on some packet. See
        // [`Server::tick_powders`].
        //
        // Followed only while there is something for it to change, which in a world that has ever
        // had its Tax Collector and freed its Yellow Slime is never: that keeps the cost of the
        // whole mechanism at one NPC scan on a packet nobody ordinarily sends, and it is what
        // bounds the list against a client that sends nothing else. The cap is the second half of
        // that bound. Both convertible types have to be named here: gating on the Tortured Soul
        // alone would have made the Yellow Slime unpurifiable however well `tick_powders` handled
        // it, because no cloud would ever have been followed in the first place.
        if sync.projectile_type == crate::game::projectile::PURIFICATION_POWDER as i16
            && self.powders.len() < MAX_TRACKED_POWDERS
            && self.npcs.iter().any(|(_, n)| {
                matches!(
                    n.npc_type,
                    crate::game::spawn::TORTURED_SOUL | crate::game::spawn::BOUND_TOWN_SLIME_YELLOW
                ) && n.is_alive()
            })
        {
            self.powders.push(crate::game::projectile::Powder {
                position: sync.position,
                velocity: sync.velocity,
                age: 0.0,
            });
        }

        let frame = sync.encode()?;
        // Culled the same way an NPC's own state is, and for the same reason: a projectile outside
        // a client's loaded sections cannot be drawn by it. In combat this is the larger of the two
        // per-tick fan-outs, because one player firing a repeating weapon syncs several projectiles
        // a tick and each went to every other player.
        let at = sync.position;
        self.broadcast_near(
            frame,
            at,
            Withheld::Projectile(sync.key.pack()),
            MAX_NPC_SYNC_SKIPS,
            Some(slot),
        );
        Ok(())
    }

    /// ...and pass on the news that it is gone.
    pub(super) fn on_client_projectile_kill(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        let kill = terrustia_proto::projectile::KillProjectile::decode(payload)?;
        if kill.key.owner != slot {
            return Ok(());
        }
        let frame = kill.encode()?;
        // A kill is culled on the same footing as the syncs that preceded it, so a client that was
        // never told about a projectile is not told about its death either. The skip run is dropped
        // afterwards because the identity is finished with: leaving it would hold a stale entry per
        // player for every projectile that has ever died.
        let at = kill.position;
        let what = Withheld::Projectile(kill.key.pack());
        self.broadcast_near(frame, at, what, MAX_NPC_SYNC_SKIPS, Some(slot));
        self.forget_skips(what);
        Ok(())
    }

    /// Packet 53: a client reporting that it has inflicted something on an NPC.
    ///
    /// This is how virtually every weapon debuff in the game arrives. The client works out what
    /// its weapon inflicts — it knows its own accessories, its own flasks, its own imbues — and
    /// the server decides only whether the target is immune. Trusting the client for *what* it
    /// inflicts is the game's own arrangement, not a shortcut: the alternative would be
    /// reimplementing every weapon's on-hit rules on the server, and the client would still
    /// disagree.
    ///
    /// What the server does *not* trust is the outcome. Immunity is checked here, so no client
    /// can poison King Slime or set the Wall of Flesh alight by asserting it has.
    pub(super) fn on_add_npc_buff(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let request = terrustia_proto::packets::AddNpcBuff::decode(payload)?;
        // A negative duration would be an eternal buff once it is written into the slot, since
        // nothing counts it up. The game reads it as a short and lets `AddBuff` compare it, which
        // means a negative one is refused for already being shorter than what is there.
        if request.ticks <= 0 {
            return Ok(());
        }
        let Some(npc) = self.npcs.get_mut(request.index) else {
            return Ok(());
        };
        if npc
            .buffs
            .add(npc.npc_type, request.buff, i32::from(request.ticks))
        {
            npc.buffs_dirty = true;
            // Sent now rather than on the next tick: the client that landed the hit is about to
            // work out its next one's armour penetration, and a tick of lag there is a hit at
            // the wrong damage.
            self.broadcast_npc_buffs(request.index);
        }
        Ok(())
    }

    /// Packet 137: a client asking that a buff be taken off an NPC.
    ///
    /// Every one of these is refused, and that is the correct behaviour rather than a gap. The
    /// game validates the request against `BuffID.Sets.CanBeRemovedByNetMessage`, which in this
    /// version is empty — so the message exists, is read, and never does anything. Reading it is
    /// still necessary: several packets arrive in one batch, and skipping this one's bytes would
    /// misparse whatever follows it.
    pub(super) fn on_remove_npc_buff(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let request = terrustia_proto::packets::RemoveNpcBuff::decode(payload)?;
        let Some(npc) = self.npcs.get_mut(request.index) else {
            return Ok(());
        };
        if npc.buffs.remove_by_request(npc.npc_type, request.buff) {
            npc.buffs_dirty = true;
            self.broadcast_npc_buffs(request.index);
        }
        Ok(())
    }

    /// Packet 56: a client asking what a town NPC is called.
    ///
    /// The client sends this with only the slot filled in the moment the NPC comes into view, and
    /// shows the type's name until it is answered. Left unhandled — as it was — every guide in
    /// the world is "Guide", nobody has a name, and the Tax Collector never becomes Andrew.
    ///
    /// The name is rolled here rather than when the NPC spawns. Nothing can tell the difference:
    /// the roll is kept once made, so an NPC's name never changes, and until somebody asks there
    /// is nobody to notice it did not have one.
    pub(super) fn on_town_npc_name_request(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = terrustia_proto::PacketReader::new(payload);
        let index = r.i16()?;
        let Ok(index) = u8::try_from(index) else {
            return Ok(());
        };
        let Some(npc) = self.npcs.get(index) else {
            return Ok(());
        };
        let npc_type = npc.npc_type;
        if npc.given_name.is_empty() && terrustia_proto::town_names::has_given_name(npc_type) {
            let variation = self.roll_town_variation(npc_type);
            let name = self.roll_town_name(npc_type, variation);
            if let Some(npc) = self.npcs.get_mut(index) {
                npc.town_variation = variation;
                npc.given_name = name;
            }
        }
        let Some(npc) = self.npcs.get(index) else {
            return Ok(());
        };
        let frame = packets::town_npc_name(index, &npc.given_name, npc.town_variation)?;
        // Answered to the asker alone, as the game does: another client that has not seen this
        // NPC yet has no use for its name and will ask when it does.
        self.send(slot, frame);
        Ok(())
    }

    /// Packet 28: a client reporting a hit on an NPC.
    pub(super) fn on_damage_npc(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut hit = DamageNpc::decode(payload)?;
        // `MessageBuffer.cs:1785-1788`: the server floors the damage at zero before it does
        // anything else with the packet, and it matters far more for the *relay* than for the
        // arithmetic. A receiving client branches on the sign (`:1795-1803`): negative damage is
        // not a small hit, it is `life = 0; HitEffect(); active = false`, so one packet 28 carrying
        // -1 used to make an NPC vanish on every client except the sender's while this server kept
        // simulating it. Every enemy could be turned into a ghost nobody but the attacker could see.
        hit.damage = hit.damage.max(0);

        // Acknowledge first, as vanilla does, so the client stops resending the hit.
        self.send(slot, damage_ack()?);

        let Some(npc) = self.npcs.get(hit.index) else {
            return Ok(());
        };
        // A stale hit aimed at whoever used to hold this slot must not land on its new occupant.
        if npc.generation != hit.generation {
            debug!(
                slot,
                index = hit.index,
                "dropping a hit with a stale generation"
            );
            return Ok(());
        }
        // The Solar Crawltipede's tail is its only directly-damageable segment (`npc_data.rs`'s
        // own 412/413 entries are `dont_take_damage: true`, matching real vanilla exactly), but a
        // hit against it does not reduce its own life at all in source — `NPC.cs`'s own `realLife`
        // redirects every segment's health to the *head*'s shared pool (`statLife =
        // Main.npc[realLife].life`), and `checkDead` only ever processes death for the segment
        // whose own `realLife == whoAmI` — every other segment's own death is silently skipped
        // entirely. This project has no general `realLife` field; scoped narrowly here to the one
        // chain that needs it, walking the already-existing `follows` link instead of adding one.
        if npc.npc_type == terrustia_proto::npc_params::SOLAR_CRAWLTIPEDE_TAIL {
            return self.on_damage_crawltipede_tail(slot, hit);
        }

        let Some(npc) = self.npcs.get_mut(hit.index) else {
            return Ok(());
        };
        // Live armour, not the type's: a rolling tortoise really is twice as hard to hurt.
        let amount = damage_taken(i32::from(hit.damage), npc.defense, hit.crit);
        // A crit shoves 1.4x harder (`NPC.cs:82251-82254`), so the knockback needs to know.
        let mut killed = npc.strike(amount, hit.knockback, hit.direction, hit.crit);
        // `NPC.PlayerInteraction` (`NPC.cs:80756`), which on a server is reached from exactly this
        // packet (`MessageBuffer.cs:1789`) and from nowhere else. See [`Npc::hit_by_player`].
        npc.hit_by_player = true;
        // A statue's monster is worth nothing: the game zeroes its value on the way out of the
        // statue, which is what stops a wired statue being a coin printer.
        let value = if npc.from_statue {
            0.0
        } else {
            npc.stats.value
        };
        let (npc_type, center) = (npc.npc_type, npc.center());

        // The Eternia Crystal does not die when it runs out of life — it goes into its losing
        // drama, which is what actually ends the event ten seconds later.
        if killed && npc_type == terrustia_proto::npc_params::DD2_ETERNIA_CRYSTAL {
            killed = false;
            npc.ai[1] = 1.0;
            npc.ai[0] = 0.0;
            npc.life = npc.life_max;
            npc.dirty = true;
        }
        // ML-1/ML-2: nor do the Moon Lord's parts (`NPC.cs:78864-78883`, `checkDead`). A struck
        // hand or head becomes a broken, empty socket that frees its True Eye; the exposed core,
        // struck down, begins its death drama. None of the three is reaped by this hit.
        if killed && crate::game::ai::boss::moon_lord::checkdead(npc) {
            killed = false;
        }

        self.broadcast(hit.encode()?, Some(slot));

        if killed {
            self.npc_died(hit.index, npc_type, center, value);
            debug!(slot, npc_type, "npc killed");
        } else {
            self.broadcast_npc(hit.index);
        }
        Ok(())
    }

    /// A hit against a Solar Crawltipede's tail — the redirect `on_damage_npc` hands off to. See
    /// its own comment there for why this exists at all.
    fn on_damage_crawltipede_tail(
        &mut self,
        slot: u8,
        hit: DamageNpc,
    ) -> terrustia_proto::Result<()> {
        // Walk `follows` back to the root: the head, which is where the whole chain's shared life
        // actually lives.
        let mut head_index = hit.index;
        while let Some(ahead) = self.npcs.get(head_index).and_then(|n| n.follows) {
            head_index = ahead;
        }
        let Some(head) = self.npcs.get_mut(head_index) else {
            self.broadcast(hit.encode()?, Some(slot));
            return Ok(());
        };
        let amount = damage_taken(i32::from(hit.damage), head.defense, hit.crit);
        head.life = (head.life - amount.max(0)).max(0);
        head.was_hurt = true;
        head.dirty = true;
        let killed = head.life <= 0;
        let (npc_type, center, value) = (
            head.npc_type,
            head.center(),
            if head.from_statue {
                0.0
            } else {
                head.stats.value
            },
        );

        self.broadcast(hit.encode()?, Some(slot));

        if killed {
            // The whole chain goes together (`CheckActive_WormSegments`'s own real mechanism):
            // every segment still `follows`ing this head, transitively, is removed, but only the
            // head's own death is processed (loot, the shield credit, everything `npc_died`
            // does) — matching real vanilla's own `checkDead` gate, which skips death processing
            // entirely for any segment whose `realLife` points elsewhere.
            let follows: std::collections::HashMap<u8, Option<u8>> = self
                .npcs
                .iter()
                .map(|(index, n)| (index, n.follows))
                .collect();
            let chain: Vec<u8> = follows
                .iter()
                .filter(|&(_, &leader)| {
                    let mut at = leader;
                    while let Some(ahead) = at {
                        if ahead == head_index {
                            return true;
                        }
                        at = follows.get(&ahead).copied().flatten();
                    }
                    false
                })
                .map(|(&index, _)| index)
                .collect();
            for index in chain {
                self.npcs.remove(index);
                self.broadcast_npc_death(index);
            }
            self.npc_died(head_index, npc_type, center, value);
            debug!(slot, npc_type, "npc killed (crawltipede chain)");
        } else {
            self.broadcast_npc(head_index);
        }
        Ok(())
    }

    /// Packets 63 and 64: a player painted a tile or a wall.
    ///
    /// `WorldGen.paintTile`/`paintWall` (`WorldGen.cs:44514`, `:44634`): both refuse an absent
    /// target (`!tile.active()`, `tile.wall == 0`, matching `real` below) and both write straight
    /// into the tile's own color byte, which is what makes this correct to keep rather than only
    /// relay: it goes into the save and into every section a client asks for afterwards, so a tile
    /// that is not there cannot be painted, which is what stops a crafted packet colouring in the
    /// empty sky.
    ///
    /// Two narrowings, both cosmetic: vanilla's own `paintEffect` dust burst on a successful paint
    /// is not sent (no gameplay effect, purely a visual cue the vanilla client would already be
    /// showing itself from the relayed packet); and vanilla's early return when the new color
    /// equals the old one (`byte b = tile.color(); if (b == color) return false;`, saving a
    /// broadcast) is not replicated, so a same-color repaint here still writes and rebroadcasts
    /// where vanilla would have skipped both. Neither changes what a client ends up seeing.
    ///
    /// A coating (`paintCoatTile`/`paintCoatWall`, `WorldGen.cs:44538`, a switch on id 0
    /// [invisible/fullbright toggle] and id 1 [illuminant]) is relayed only, not modelled server
    /// side: both flags are purely visual and this server has no lighting or fullbright rendering
    /// of its own to keep consistent with them.
    pub(super) fn on_paint(
        &mut self,
        slot: u8,
        id: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let (x, y) = (i32::from(r.i16()?), i32::from(r.i16()?));
        let colour = r.u8()?;
        // The last byte separates paint from coating; a coating is not a colour and is only
        // relayed, because nothing on the server reads it.
        let coating = r.u8().unwrap_or(0) != 0;

        if !self.world.in_bounds(x, y) {
            return Ok(());
        }
        if !coating {
            let mut tile = self.world.tile(x, y);
            let painting_a_wall = id == id::SYNC_WALL_PAINT_OR_COATING;
            let real = if painting_a_wall {
                tile.wall != 0
            } else {
                tile.is_active()
            };
            if !real {
                debug!(slot, x, y, "painting nothing");
                return Ok(());
            }
            if painting_a_wall {
                tile.wall_color = colour;
            } else {
                tile.color = colour;
            }
            self.world.set_tile(x, y, tile);
        }
        self.broadcast(packets::verbatim(id, payload)?, Some(slot));
        Ok(())
    }

    /// Packets 89, 123, 133 and 149: putting an item into a frame, rack, platter or jar.
    ///
    /// All four are the same message with a different id, and all four are the whole point of the
    /// furniture they belong to: a weapon rack that cannot be given a weapon is a wall decoration.
    ///
    /// Whatever was in it already falls out, which is what the game does and what a player
    /// expects — swapping the sword on your rack should not eat the old one.
    pub(super) fn on_display_item(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        use terrustia_proto::tile_entity::EntityData;
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let (x, y) = (r.i16()?, r.i16()?);
        let item = ItemStack {
            id: i32::from(r.i16()?),
            prefix: r.u8()?,
            stack: r.i16()?,
        };

        let Some(at) = self
            .world
            .tile_entities
            .iter()
            .position(|e| e.x == x && e.y == y)
        else {
            // Nothing there to put it in, so it lands on the floor rather than vanishing.
            self.spawn_item(item, tile_corner(x, y));
            return Ok(());
        };
        let entity = &mut self.world.tile_entities[at];
        let EntityData::Held(existing) = entity.data else {
            // That kind of furniture does not hold a single item; the packet is for the wrong one.
            return Ok(());
        };
        entity.data = EntityData::Held(item);
        let id = entity.id;
        if !existing.is_empty() {
            self.spawn_item(existing, tile_corner(x, y));
        }
        self.share_tile_entity(id);
        Ok(())
    }

    /// Packet 156: a kite or a critter being clipped onto its anchor.
    pub(super) fn on_anchor_item(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        use terrustia_proto::tile_entity::EntityData;
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let (x, y) = (r.i16()?, r.i16()?);
        let item = r.i16()?;
        let Some(entity) = self
            .world
            .tile_entities
            .iter_mut()
            .find(|e| e.x == x && e.y == y)
        else {
            return Ok(());
        };
        let EntityData::Anchor { item: held } = &mut entity.data else {
            return Ok(());
        };
        *held = item;
        let id = entity.id;
        self.share_tile_entity(id);
        Ok(())
    }

    /// Packet 121: one slot of a mannequin.
    ///
    /// The message names a slot and a command rather than sending the whole thing, because a
    /// mannequin has nineteen slots and a player changes one at a time. Command 2 is the pose;
    /// 0, 1 and 3 are the armour, the dyes and the accessory.
    pub(super) fn on_display_doll_slot(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        use terrustia_proto::tile_entity::EntityData;
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let _claimed_player = r.u8()?; // rewritten to the sender, as vanilla does
        let id = r.i32()?;
        let index = usize::from(r.u8()?);
        let command = r.u8()?;

        let Some(entity) = self.world.tile_entities.iter_mut().find(|e| e.id == id) else {
            return Ok(());
        };
        let EntityData::DisplayDoll(doll) = &mut entity.data else {
            return Ok(());
        };
        if command == DOLL_POSE {
            doll.pose = r.u8()?;
        } else {
            let item = ItemStack {
                id: i32::from(r.u16()? as i16),
                stack: r.u16()? as i16,
                prefix: r.u8()?,
            };
            let into = match command {
                DOLL_DYE => doll.dyes.get_mut(index),
                DOLL_MISC => doll.misc.get_mut(index),
                _ => doll.equip.get_mut(index),
            };
            let Some(into) = into else {
                return Ok(()); // a slot number past the end is a crafted packet
            };
            *into = item;
        }
        // Relayed rather than re-serialised: the payload is exactly what every other client
        // needs, and the sender already has it.
        self.broadcast(
            packets::rewrite_owner(id::T_E_DISPLAY_DOLL_DATA_SYNC, payload, slot)?,
            Some(slot),
        );
        Ok(())
    }

    /// Packet 124: one slot of a hat rack.
    ///
    /// Two hats and two dyes, with the dye flag folded into the slot number by adding two — which
    /// is why the number has to be split apart again before it is used as an index.
    pub(super) fn on_hat_rack_slot(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        use terrustia_proto::tile_entity::EntityData;
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let _claimed_player = r.u8()?;
        let id = r.i32()?;
        let mut index = usize::from(r.u8()?);
        let dye = index >= 2;
        if dye {
            index -= 2;
        }

        let Some(entity) = self.world.tile_entities.iter_mut().find(|e| e.id == id) else {
            return Ok(());
        };
        let EntityData::HatRack(rack) = &mut entity.data else {
            return Ok(());
        };
        let item = ItemStack {
            id: i32::from(r.u16()? as i16),
            stack: r.u16()? as i16,
            prefix: r.u8()?,
        };
        let into = if dye {
            rack.dyes.get_mut(index)
        } else {
            rack.items.get_mut(index)
        };
        let Some(into) = into else {
            return Ok(());
        };
        *into = item;
        self.broadcast(
            packets::rewrite_owner(id::T_E_HAT_RACK_ITEM_SYNC, payload, slot)?,
            Some(slot),
        );
        Ok(())
    }

    /// Packet 122: which tile entity a player currently has open.
    ///
    /// Two things hang off it. A client cannot edit an entity it has not claimed, and only one
    /// player may hold a given entity at a time — which is what stops two people emptying the
    /// same mannequin into their own inventories at once.
    pub(super) fn on_tile_entity_interaction(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let id = r.i32()?;

        if id == NO_TILE_ENTITY {
            self.tile_entity_anchors.remove(&slot);
        } else {
            if !self.world.tile_entities.iter().any(|e| e.id == id) {
                return Ok(());
            }
            // Somebody else already has it open, so this claim is refused rather than shared.
            if self
                .tile_entity_anchors
                .iter()
                .any(|(&who, &held)| who != slot && held == id)
            {
                return Ok(());
            }
            self.tile_entity_anchors.insert(slot, id);
        }

        // Everyone is told who holds what, so each client can grey the thing out.
        let mut w = terrustia_proto::PacketWriter::new(id::REQUEST_TILE_ENTITY_INTERACTION);
        w.i32(id).u8(slot);
        let frame = w.finish()?;
        self.broadcast(frame, None);
        Ok(())
    }

    /// Packet 87: a client placed a tile entity.
    ///
    /// The tile has to be there and be the right one, and there has to be nothing there already.
    /// Both checks are the server's: without them a crafted packet hangs an item frame in the sky
    /// or stacks a hundred dummies on one tile.
    pub(super) fn on_tile_entity_placed(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        use terrustia_proto::tile_entity::{EntityKind, TileEntity};
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let (x, y) = (r.i16()?, r.i16()?);
        let Some(kind) = EntityKind::from_id(r.u8()?) else {
            return Ok(());
        };
        if !self.world.in_bounds(i32::from(x), i32::from(y)) {
            return Ok(());
        }
        // Most kinds cannot be asked for at all. The game's base `NetPlaceEntityAttempt` does
        // nothing, so a request naming an item frame or a mannequin is silently dropped — those
        // come into being when their *tile* goes down. Accepting all eleven, which this server
        // did, lets a crafted packet scatter tile entities through a world; a fuzzer duly found
        // three in a saved world that should have had none.
        if !kind.placeable_by_request() {
            debug!(slot, ?kind, "that kind is not placed by asking");
            return Ok(());
        }
        if self
            .world
            .tile_entities
            .iter()
            .any(|e| e.x == x && e.y == y)
        {
            return Ok(());
        }
        // The tile it claims to stand on has to actually be there.
        let tile = self.world.tile(i32::from(x), i32::from(y));
        if !tile.is_active() || tile.block != kind.tile() {
            debug!(slot, x, y, ?kind, "nothing there to place that on");
            return Ok(());
        }

        let id = self.world.next_tile_entity;
        self.world.next_tile_entity += 1;
        self.world
            .tile_entities
            .push(TileEntity::new(id, kind, x, y));
        debug!(slot, x, y, ?kind, id, "tile entity placed");
        // Everyone has to be told, the placer included: the client sends the placement but does
        // not create the entity itself, and the id it will refer to from now on is the server's
        // to hand out.
        self.share_tile_entity(id);
        Ok(())
    }

    /// Packet 34: a chest, dresser or Containers2 chest was placed or broken.
    ///
    /// It is the *only* wire notification either way, which is what makes this handler own the
    /// tiles and not just the record. A client never sends packet 79 or a tile square for a
    /// container: `Main.tileContainer[21|88|467|470|475] = true` (`Main.cs:10215-10219`) and
    /// `Player.cs:40461` skips `SendObjectPlacement` for anything in that table, so
    /// `Chest.AfterPlacement_Hook`'s own `NetMessage.SendData(34, ...)` (`Chest.cs:565-579`) is the
    /// whole of it. Breaking one sends packet 17 with the *fail* flag set (`Player.cs:54419-54432`,
    /// `SendData(17, ..., 0, x, y, 1f)`), which is a hit effect and not a break, and then this.
    /// Vanilla's server does the real work in both directions at `MessageBuffer.cs:1996-2116`:
    /// `WorldGen.PlaceChest` places the tiles *and* allocates the chest, and `WorldGen.KillTile`
    /// clears them again.
    ///
    /// The placing half here wrote nothing at all and relayed the payload verbatim, on the
    /// strength of a comment claiming placement arrived as a tile square. It does not, and the
    /// cost was a chest that existed on every client and on no server: invisible to the save, to a
    /// `.wld` round-trip, to housing and to any client that re-received the section, with anything
    /// put in it lost. The relay was worse: a client-sent packet 34 always writes `(short)0` in
    /// the id field (`NetMessage.cs:916-930`), so forwarding it verbatim made every *receiver* run
    /// `WorldGen.PlaceChestDirect(..., 0)` into `Chest.CreateWorldChest(0, ...)` (`Chest.cs:
    /// 583-600`) and overwrite their own chest 0. Vanilla replaces that field with the id the
    /// server allocated, which is what the broadcasts below now carry.
    ///
    /// One thing is deliberately not transcribed. `MessageBuffer.cs:2124-2157`'s trailing
    /// `switch (b4)` sits *outside* the `netMode == 2` block, so vanilla's own server also runs
    /// the receiver's `PlaceChestDirect`/`DestroyChestDirect` with the id it forced to zero at
    /// `:1993`, on top of the work it just did - which re-creates the chest it has already
    /// allocated at index 0. A server is not a receiver of its own broadcast, so only the
    /// `netMode == 2` branch is transcribed here.
    pub(super) fn on_chest_update(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let action = r.u8()?;
        let (x, y) = (r.i16()?, r.i16()?);
        let style = r.i16()?;

        // Odd actions break; even ones place.
        let breaking = action % 2 == 1;
        let block = match action {
            0 | 1 => CHEST_BLOCK,
            2 | 3 => DRESSER_BLOCK,
            4 | 5 => terrustia_proto::locks::CHEST_2,
            _ => return Ok(()),
        };
        let (ix, iy) = (i32::from(x), i32::from(y));
        if !self.world.in_bounds(ix, iy) {
            return Ok(());
        }
        let Some(object) = terrustia_proto::tile_object::tile_object(block) else {
            return Ok(());
        };

        if breaking {
            // `if (Main.tile[num32, num33].type == 21)` and its two siblings: a break naming a tile
            // that is not the container it claims does nothing.
            let tile = self.world.tile(ix, iy);
            if !tile.is_active() || tile.block != block {
                debug!(slot, x, y, block, "nothing of that kind there to break");
                return Ok(());
            }
            // The client reports whichever cell was clicked; the chest is anchored at the
            // top-left, so walk back to it before looking the chest up.
            let wide = if block == DRESSER_BLOCK { 54 } else { 36 };
            let anchor = (
                x - (tile.frame_x % wide) / 18,
                y - i16::from(tile.frame_y % 36 != 0),
            );
            let id = match self.world.chest_at(anchor.0, anchor.1) {
                Some((id, chest)) => {
                    // A chest with anything in it is not breakable (`Chest.CanDestroyChest`).
                    if chest.items.iter().any(|item| item.stack > 0) {
                        debug!(slot, x, y, "refusing to break a chest with things in it");
                        return Ok(());
                    }
                    self.world.remove_chest(id);
                    id
                }
                // `Chest.FindChest` returning -1: the tiles are there with no record behind them,
                // which is still a legal break. Receivers skip `DestroyChestDirect` on a negative
                // id and kill the tiles anyway.
                None => -1,
            };
            // `WorldGen.KillTile(num32, num33)` (`MessageBuffer.cs:2032`). Packet 17 arrived with
            // its fail flag set and so cleared nothing, which left this server keeping the tiles
            // of every chest anyone ever broke, and giving nothing back for them.
            let (left, top) = (i32::from(anchor.0), i32::from(anchor.1));
            for dx in 0..object.width {
                for dy in 0..object.height {
                    let mut cell = self.world.tile(left + dx, top + dy);
                    if !cell.is_active() || cell.block != block {
                        continue;
                    }
                    cell.flags.set(TileFlags::ACTIVE, false);
                    cell.block = 0;
                    cell.frame_x = -1;
                    cell.frame_y = -1;
                    cell.slope = 0;
                    cell.flags.set(TileFlags::HALF_BRICK, false);
                    self.world.set_tile(left + dx, top + dy, cell);
                    self.liquids.disturb(left + dx, top + dy);
                }
            }
            self.spawn_tile_drop(block, tile.frame_x, tile.frame_y, ix, iy);
            debug!(slot, x = anchor.0, y = anchor.1, id, "container broken");

            // `TrySendData(34, -1, -1, null, b4, num32, num33, 0f, number)`: the *anchor*, style
            // zeroed, the real id, and to everybody including the breaker (whose own client only
            // played a hit effect and is still drawing the chest).
            let mut w = terrustia_proto::PacketWriter::new(id::CHEST_UPDATES);
            w.u8(action).i16(anchor.0).i16(anchor.1).i16(0).i16(id);
            let frame = w.finish()?;
            self.broadcast(frame, None);
            return Ok(());
        }

        // Ten tiles clear of the world's edge, the same margin `on_place_object` transcribes for
        // the placement both go through (`TileObject.CanPlace`).
        if ix < 10 || iy < 10 || ix >= self.world.width() - 10 || iy >= self.world.height() - 10 {
            return Ok(());
        }
        // A deliberate narrowing: vanilla hands the wire's style straight to
        // `WorldGen.PlaceChest` with no range check at all, so a crafted packet 34 writes a
        // negative or wrapped `frameX` into a real tile and that corrupt frame goes into the save.
        // The bound is exactly what fits: the rightmost cell's frame has to survive the `i16` the
        // tile stores it in.
        let base = i32::from(style) * if block == DRESSER_BLOCK { 54 } else { 36 };
        if style < 0 || i16::try_from(base + (object.width - 1) * 18).is_err() {
            debug!(slot, style, block, "that container has no such style");
            return Ok(());
        }
        // The cursor sits on the object's lower-left cell, so its corner is one row up and, for a
        // dresser, one column left (`TileObjectData`'s `Origin` for each, which is what this
        // project's own `tile_object` table records).
        let (left, top) = (ix - object.origin.0, iy - object.origin.1);
        // `TileObject.CanPlace`: the whole footprint or nothing.
        let clear = (0..object.width).all(|dx| {
            (0..object.height).all(|dy| !self.world.tile(left + dx, top + dy).is_active())
        });
        let placed = clear.then(|| self.register_chest(left, top)).flatten();
        if let Some(id) = placed {
            // Frames straight from `WorldGen.PlaceChestDirect` and `PlaceDresserDirect`
            // (`WorldGen.cs:58337-58405`): `36 * style` for a chest or a Containers2 and `54 *
            // style` for a dresser, plus 18 per cell across, with `frameY` 0 then 18.
            //
            // Written out rather than routed through `tile_object::frame_of`, because these two
            // methods are what vanilla itself runs for a placed container and they write the
            // literals. The two now agree: `tile_object.rs` is generated from `TileObjectData
            // .Initialize`, which leaves 21/88/467 on `_baseObject`'s `StyleWrapLimit = 0,
            // StyleMultiplier = 1` (`TileObjectData.cs:1799-1801`), so `frame_of` gives exactly
            // `36 * style` and `54 * style` here.
            //
            // They did not always. The table used to give all three a multiplier of 2 and a wrap
            // of 2, which agreed with vanilla at style 0 and at no other, and this site was
            // written the long way around it. Kept the long way, now for the ordinary reason:
            // it is what `worldgen/structures.rs`'s `add_chest_styled` writes and what `on_lock`
            // reads back with `frame_x / 36`.
            debug!(slot, x, y, block, id, "container placed");
            for dx in 0..object.width {
                for dy in 0..object.height {
                    let was = self.world.tile(left + dx, top + dy);
                    let tile = terrustia_proto::tile::Tile::framed(
                        block,
                        (base + dx * 18) as i16,
                        (dy * 18) as i16,
                    )
                    .with_wall(was.wall);
                    self.world.set_tile(left + dx, top + dy, tile);
                    self.liquids.disturb(left + dx, top + dy);
                }
            }
        }

        let mut w = terrustia_proto::PacketWriter::new(id::CHEST_UPDATES);
        w.u8(action)
            .i16(x)
            .i16(y)
            .i16(style)
            .i16(placed.unwrap_or(-1));
        let frame = w.finish()?;
        match placed {
            // To everybody including the placer: their own `AfterPlacement_Hook` returned -1 in
            // netMode 1 without creating anything, so this broadcast is what builds the chest on
            // their screen too, with the server's id on it.
            Some(id) => {
                debug!(slot, x, y, block, id, "container placed");
                self.broadcast(frame, None);
            }
            // `TrySendData(34, whoAmI, -1, ...)` with -1: to the placer alone, whose client then
            // runs `WorldGen.KillTile` and takes the chest back off its own screen. Vanilla also
            // drops the container as an item here; this server does not, and the case only arises
            // with the world's 8000th chest or a footprint that was not clear.
            None => {
                debug!(slot, x, y, block, "no room for another container");
                self.send(slot, frame);
            }
        }
        Ok(())
    }

    /// Packet 52: a key turned in a chest or a door.
    ///
    /// Two of the chests are gated on Plantera, and that gate is the server's to hold: the biome
    /// chests and the temple are the whole reward for beating her.
    pub(super) fn on_lock(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        use terrustia_proto::locks::{self, LockAction};
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let action = LockAction::from_id(r.u8()?);
        let (x, y) = (i32::from(r.i16()?), i32::from(r.i16()?));
        let Some(action) = action else {
            return Ok(());
        };
        if !self.world.in_bounds(x, y) || !self.world.in_bounds(x + 1, y + 1) {
            return Ok(());
        }

        let anchor = self.world.tile(x, y);
        let moved = match action {
            LockAction::UnlockDoor => {
                // The client reframes its own copy and sends nothing but this packet
                // (`Player.cs:33064-33070`, which consumes the Golden Key and then only
                // `SendData(52, ..., 2f, ...)`), so a server that merely relays keeps the door
                // locked: it re-locks on the next section send and it saves locked, with the key
                // already spent. `WorldGen.UnlockDoor` (`WorldGen.cs:37988-38017`) is the work.
                //
                // The frame test is `WorldGen.IsLockedDoor` (`WorldGen.cs:69725-69732`), which is
                // what the client itself checks before sending. Testing only `block ==
                // DOOR_CLOSED`, as this used to, relayed a crafted action-2 packet aimed at any
                // ordinary door in the world.
                if !anchor.is_active()
                    || anchor.block != locks::DOOR_CLOSED
                    || !(594..=646).contains(&anchor.frame_y)
                    || anchor.frame_x >= 54
                {
                    debug!(slot, x, y, "that is not a locked door");
                    return Ok(());
                }
                // Walk up to the door's top row, which is the one framed at exactly 594. Vanilla
                // decrements first and tests the tile it lands on, bailing the moment the frame
                // drops below 594 or the walk reaches the top of the world.
                let mut top = y;
                while self.world.tile(x, top).frame_y != 594 {
                    top -= 1;
                    if top <= 0 || self.world.tile(x, top).frame_y < 594 {
                        return Ok(());
                    }
                }
                for row in top..=top + 2 {
                    let mut tile = self.world.tile(x, row);
                    tile.frame_y += 54;
                    self.world.set_tile(x, row, tile);
                }
                true
            }
            LockAction::UnlockChest | LockAction::LockChest => {
                let style = i32::from(anchor.frame_x) / 36;
                let shift = if action == LockAction::UnlockChest {
                    locks::unlock_shift(anchor.block, style)
                } else {
                    locks::lock_shift(anchor.block, style)
                };
                let Some((shift, needs_plantera)) = shift else {
                    debug!(slot, x, y, block = anchor.block, style, "not a lock");
                    return Ok(());
                };
                if needs_plantera && !self.world.progress.downed_plantera {
                    debug!(slot, x, y, "that lock waits for Plantera");
                    return Ok(());
                }
                // A chest is two tiles by two, and all four carry the frame.
                let toward = if action == LockAction::UnlockChest {
                    -shift
                } else {
                    shift
                };
                for dx in 0..2 {
                    for dy in 0..2 {
                        let mut tile = self.world.tile(x + dx, y + dy);
                        if tile.block != anchor.block {
                            continue;
                        }
                        tile.frame_x += toward;
                        self.world.set_tile(x + dx, y + dy, tile);
                    }
                }
                true
            }
        };
        if !moved {
            return Ok(());
        }
        self.broadcast(packets::verbatim(id::LOCK_AND_UNLOCK, payload)?, Some(slot));
        Ok(())
    }

    /// Packet 59: a switch, lever or pressure plate was hit.
    ///
    /// The circuit runs here as well as being relayed: an actuator changes what the world *is*,
    /// and a trap has to throw the same dart at everybody rather than one per client.
    pub(super) fn on_hit_switch(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let (x, y) = (i32::from(r.i16()?), i32::from(r.i16()?));
        if !self.world.in_bounds(x, y) {
            return Ok(());
        }

        self.fire_switch(x, y);
        self.broadcast(packets::verbatim(id::HIT_SWITCH, payload)?, Some(slot));
        Ok(())
    }

    /// Packet 111: somebody clicked a Party Center.
    ///
    /// Relaying this did nothing at all: no client acts on a received 111 - vanilla's own case is
    /// `if (Main.netMode == 2) BirthdayParty.ToggleManualParty()` and nothing else
    /// (`MessageBuffer.cs:3832-3836`), so the button was simply dead. The wire-triggered Party
    /// Monolith already went through `fire_switch`; only the direct click was missing.
    ///
    /// `BirthdayParty.ToggleManualParty` resyncs world data only when `PartyIsUp` actually
    /// changed, which is why `toggle_manual`'s return value is tested rather than ignored: a
    /// genuine Party Girl party already running is not interrupted by a click, and clicking during
    /// one changes nothing anybody can see.
    fn on_toggle_party(&mut self, slot: u8) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let was_up = self.party.is_up();
        if self.party.toggle_manual() != was_up {
            self.broadcast_world_data();
        }
        Ok(())
    }

    /// Packet 134: a player's luck factors changed.
    ///
    /// `MessageBuffer.GetData`'s case 134 (`MessageBuffer.cs:4190-4220`) does three things, and
    /// this server used to do only the last of them:
    ///
    /// ```csharp
    /// int num106 = reader.ReadByte();
    /// ... eight factors ...
    /// if (Main.netMode == 2) { num106 = whoAmI; }
    /// Player obj4 = Main.player[num106];
    /// obj4.ladyBugLuckTimeLeft = ladyBugLuckTimeLeft;  // ...and the other seven
    /// obj4.RecalculateLuck();
    /// if (Main.netMode == 2) { NetMessage.SendData(134, -1, num106, null, num106); }
    /// ```
    ///
    /// It was in the verbatim relay group, which got the relay right and dropped the state on the
    /// floor. That is why every luck-sensitive roll in this workspace was written against zero
    /// luck: not because luck is client-side (it is not - vanilla's server keeps a real figure per
    /// player, and this is where it gets it), but because nothing here ever read the packet that
    /// carries it.
    ///
    /// `num106 = whoAmI` is the security half and is kept: [`crate::game::luck::LuckFactors::
    /// decode`] discards the slot byte outright, so a client cannot report luck for anybody else.
    /// A malformed body is dropped rather than relayed, which is a change from the verbatim arm:
    /// there is no reason to hand every other client bytes this server could not read.
    fn on_luck_factors(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let factors = terrustia_proto::luck::LuckFactors::decode(payload)?;
        if let Some(player) = self.player_mut(slot) {
            player.luck_factors = factors;
        }
        self.refresh_luck(slot);
        if let Ok(relayed) = packets::verbatim(id::UPDATE_PLAYER_LUCK_FACTORS, payload) {
            self.broadcast(relayed, Some(slot));
        }
        Ok(())
    }

    /// Packet 120: a player used an emote.
    ///
    /// Never relayed by a real server. Vanilla runs `EmoteBubble.NewBubble` (`MessageBuffer.cs:
    /// 3855-3866`), which broadcasts packet **91** instead (`EmoteBubble.cs`'s own `NetMessage
    /// .SendData(91, -1, -1, null, ID, anchorType, anchorMeta, time, emoticon)`). Relaying 120, as
    /// this server used to, sends every receiver a packet whose own handler is `netMode == 2`-only:
    /// it is read and discarded, so emotes were invisible to everybody but their sender.
    fn on_emote(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        /// `EmoteID.Count` (`EmoteID.cs:9`). Vanilla bounds-checks the emote before making a
        /// bubble of it (`num260 >= 0 && num260 < EmoteID.Count`); this server checked nothing.
        const EMOTE_COUNT: u8 = 151;
        /// `EmoteBubble.NewBubble`'s own `time` for a player emote (`MessageBuffer.cs:3861`).
        const BUBBLE_TICKS: u16 = 360;
        /// `EmoteBubble.SerializeNetAnchor`: 0 is an NPC, 1 a player, 2 a projectile.
        const ANCHOR_PLAYER: u8 = 1;

        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let _claimed = r.u8()?;
        let emote = r.u8()?;
        if emote >= EMOTE_COUNT {
            debug!(slot, emote, "no such emote");
            return Ok(());
        }

        // `EmoteBubble.AssignNewID` is `NextID++` against a server-wide store this project does
        // not keep: a bubble is display-only, expires on its own after `BUBBLE_TICKS`, and is
        // never updated or removed by id. All the id has to do is separate the bubbles alive at
        // one time, and one player can raise at most one per tick, so the tick and the slot are
        // already that. Wrapping is harmless - a reused id lands on a bubble six seconds dead.
        let bubble = (self.ticks as i32)
            .wrapping_mul(256)
            .wrapping_add(i32::from(slot));
        let mut w = terrustia_proto::PacketWriter::new(id::SYNC_EMOTE_BUBBLE);
        w.i32(bubble)
            .u8(ANCHOR_PLAYER)
            .u16(u16::from(slot))
            .u16(BUBBLE_TICKS)
            .u8(emote);
        let frame = w.finish()?;
        // To everybody: vanilla's own `SendData(91, -1, -1, ...)` excludes nobody, and the sender's
        // own client does not raise the bubble for itself.
        self.broadcast(frame, None);
        Ok(())
    }

    /// Run the circuit a trigger at `(x, y)` starts: `Wiring.HitSwitch`.
    ///
    /// Shared by the direct click (packet 59, above) and by `WorldGen.ToggleGemLock`'s own tail
    /// (`WorldGen.cs:46930`), which fires the same `Wiring.HitSwitch` at the lock's corner. The
    /// caller owns the wire notification, because the two differ: a click is relayed to everyone
    /// but its sender, a gem lock's is sent to everyone.
    fn fire_switch(&mut self, x: i32, y: i32) {
        // The circuit is run here rather than only relayed. An actuator changes what the world
        // *is* — whether a block is solid — so a server that leaves it to the clients has a world
        // where players walk through walls the server thinks are there.
        let fired = {
            let world = &mut self.world;
            crate::world::wiring::hit_switch(world, x, y)
        };
        let party_monolith = fired.party_monolith;
        self.apply_circuit(fired, (x, y));

        // `BirthdayParty::ToggleManualParty` — a direct click or a wire signal reaching a Party
        // Monolith (`wiring.rs`'s own `PARTY_MONOLITH`). Real vanilla has no chat message for
        // this at all, unlike a natural party starting or any party ending at night — only the
        // world-data resync (`NetMessage.SendData(7)`) that lets clients react to it.
        if party_monolith {
            self.party.toggle_manual();
            self.broadcast_world_data();
        }
    }

    /// Packet 70: a critter was caught in a net, and is now an item.
    pub(super) fn on_bug_caught(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let index = r.i16()?;
        let Ok(index) = u8::try_from(index) else {
            return Ok(());
        };
        // Only a critter can be netted. Refusing anything else is what stops a crafted packet
        // deleting a boss.
        let catchable = self
            .npcs
            .get(index)
            .is_some_and(|n| n.stats.friendly && !n.stats.town_npc && !n.stats.boss);
        if !catchable {
            debug!(slot, index, "refusing to net that");
            return Ok(());
        }
        self.npcs.remove(index);
        self.broadcast_npc_death(index);
        Ok(())
    }

    /// Packet 71: a critter was let out of a jar.
    pub(super) fn on_bug_released(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let (x, y) = (r.i32()?, r.i32()?);
        let npc_type = r.i16()?;
        let Ok(npc_type) = u16::try_from(npc_type) else {
            return Ok(());
        };
        // The same rule in reverse: a jar holds critters, so only a critter comes out of one.
        let is_a_critter = terrustia_proto::npc_data::npc_stats(npc_type)
            .is_some_and(|s| s.friendly && !s.town_npc && !s.boss);
        if !is_a_critter {
            debug!(slot, npc_type, "refusing to release that");
            return Ok(());
        }
        if let Some(index) = self.npcs.spawn(npc_type, (x as f32, y as f32)) {
            self.broadcast_npc(index);
        }
        Ok(())
    }

    /// Packet 48: a bucket poured, or a client telling us liquid moved.
    ///
    /// The amount is taken and the tile woken; the settling itself is the server's, so a client
    /// cannot make water flow uphill by saying it did.
    pub(super) fn on_liquid(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        // Vanilla's third spam counter, and the tightest of the three: 50 with 0.2 a tick back.
        // Liquid is the cheapest thing to spam and the most expensive to simulate. Gated on
        // `spam_check` because vanilla gates the counter itself, not just the boot:
        // `MessageBuffer.cs:2415` reads `if (Main.netMode == 2 && Netplay.SpamCheck)` before it
        // ever touches `SpamWater`.
        if self.config.spam_check
            && let Some(player) = self.player_mut(slot)
        {
            player.spam_liquid += 1.0;
            if player.spam_liquid > SPAM_LIQUID_MAX {
                info!(slot, "disconnecting a client for liquid spam");
                self.kick(slot, "moving liquid too fast");
                return Ok(());
            }
        }
        let mut r = PacketReader::new(payload);
        let (x, y) = (i32::from(r.i16()?), i32::from(r.i16()?));
        let amount = r.u8()?;
        let kind = r.u8()?;
        if !self.world.in_bounds(x, y) {
            return Ok(());
        }
        let mut tile = self.world.tile(x, y);
        tile.liquid = amount;
        tile.liquid_kind = match kind {
            1 => terrustia_proto::tile::Liquid::Lava,
            2 => terrustia_proto::tile::Liquid::Honey,
            3 => terrustia_proto::tile::Liquid::Shimmer,
            _ => terrustia_proto::tile::Liquid::Water,
        };
        self.world.set_tile(x, y, tile);
        self.liquids.disturb(x, y);
        // An emptied tile is relayed at once, to everybody but the sender who already knows:
        // `MessageBuffer.cs:2438-2442`'s `if (b2 == 0) NetMessage.SendData(48, -1, whoAmI, ..)`.
        // Only the drain, because any other amount is left to the simulation, which tells clients
        // as it moves the liquid on (`tick_liquids`). A drain has nothing left to move: the tile is
        // already empty when `Liquids::settle` next reaches it, so it returns without reporting a
        // change, and for an isolated pool (a sealed container, a decorative pond, the last bucket
        // out of a farm) no neighbour ever refills it to correct the clients that missed it. They
        // would render the liquid until the section was sent again.
        if amount == 0 {
            let change = net_module::LiquidChange {
                x,
                y,
                amount,
                kind: tile.liquid_kind.as_type_byte(),
            };
            if let Ok(frame) = net_module::liquid_changes(&[change]) {
                self.broadcast(frame, Some(slot));
            }
        }
        Ok(())
    }

    /// Packet 113: a player put an Eternia Crystal on its stand.
    ///
    /// This is the only way the Old One's Army begins, and it is refused more often than not: the
    /// stand has to be real, there cannot already be a crystal, and the arena has to be sixty
    /// tiles clear on both sides. That last check is why building a proper arena is part of
    /// preparing for the event rather than a nicety.
    pub(super) fn on_crystal_placed(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        use terrustia_proto::npc_params::DD2_ETERNIA_CRYSTAL;
        /// The Eternia Crystal Stand.
        const STAND: u16 = 466;
        /// How much room the arena needs each side of the stand, in tiles.
        const ARENA_CLEARANCE: i32 = 60;

        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let (x, y) = (i32::from(r.i16()?), i32::from(r.i16()?));

        if self
            .npcs
            .iter()
            .any(|(_, n)| n.npc_type == DD2_ETERNIA_CRYSTAL)
        {
            return Ok(());
        }
        let tile = self.world.tile(x, y);
        if !tile.is_active() || tile.block != STAND {
            debug!(slot, x, y, "no crystal stand there");
            return Ok(());
        }
        // The crystal sits at the middle of the stand, not at the tile that was clicked.
        let origin = (
            x - i32::from(tile.frame_x) / 18,
            y - i32::from(tile.frame_y) / 18,
        );
        let (left, right) = crate::game::army::arena_ends(&WorldTiles(&self.world), origin);
        if right.0 - origin.0 < ARENA_CLEARANCE || origin.0 - left.0 < ARENA_CLEARANCE {
            debug!(slot, x, y, "the arena is too small for the army");
            return Ok(());
        }

        let Some(tier) = self.army_tier() else {
            return Ok(());
        };
        self.army.start(tier);
        self.army_arena = Some((left, right));
        // Three hundred ticks before the first wave, so the arena has a moment to settle.
        self.army.hold = 300;
        self.broadcast_army_wait(self.army.hold);

        let at = (
            origin.0 as f32 * crate::game::npc::TILE + 40.0,
            origin.1 as f32 * crate::game::npc::TILE + 64.0,
        );
        if let Some(index) = self.npcs.spawn(DD2_ETERNIA_CRYSTAL, at) {
            self.broadcast_npc(index);
        }
        self.announce("The Old One's Army is approaching!");
        info!(
            slot,
            ?tier,
            x = origin.0,
            y = origin.1,
            "old one's army started"
        );
        Ok(())
    }

    /// Packet 75: a player reporting that they have handed the Angler today's fish.
    ///
    /// One a day each, which the server has to be the judge of — a client that could tell itself
    /// it had not yet handed one in could farm the reward all day.
    pub(super) fn on_angler_finished(&mut self, slot: u8) -> terrustia_proto::Result<()> {
        let Some(name) = self
            .player(slot)
            .filter(|p| p.is_playing())
            .map(|p| p.name.clone())
        else {
            return Ok(());
        };
        if self.angler_finished_today.insert(name) {
            debug!(slot, "angler quest handed in");
        }
        Ok(())
    }

    /// Packet 76: how many quests a player has finished, and their golf score.
    ///
    /// Both live on the character rather than the world, so the server's job is to remember what
    /// it is told and pass it on — that is what makes the Angler's reward tiers work at all,
    /// since they are gated on the count.
    pub(super) fn on_quest_count(
        &mut self,
        slot: u8,
        payload: &[u8],
    ) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        let _claimed = r.u8()?;
        let quests = r.i32()?;
        let golf = r.i32()?;
        let Some(player) = self.player_mut(slot) else {
            return Ok(());
        };
        player.angler_quests = quests;
        player.golf_score = golf;
        // The rebroadcast is of the *stored* character state, not of the bytes just read:
        // `NetMessage.cs:1156-1160`'s `case 76` writes `Main.player[number].anglerQuestsFinished`
        // and `.golferScoreAccumulated`. Reading them back is what makes storing them mean
        // something rather than being a copy nothing ever consults.
        let (quests, golf) = (player.angler_quests, player.golf_score);
        let mut w = terrustia_proto::PacketWriter::new(id::QUESTS_COUNT_SYNC);
        w.u8(slot).i32(quests).i32(golf);
        let frame = w.finish()?;
        self.broadcast(frame, Some(slot));
        Ok(())
    }

    /// Packet 51: the odd-jobs packet, whose first action is the only way to fight Skeletron.
    ///
    /// A client sends it when the player takes the Old Man up on his offer. There is no summon
    /// item for Skeletron and never has been — the dialogue *is* the summon — so without this the
    /// dungeon stays shut and nothing behind it can be reached.
    pub(super) fn on_misc_data(&mut self, slot: u8, payload: &[u8]) -> terrustia_proto::Result<()> {
        if !self.player(slot).is_some_and(Player::is_playing) {
            return Ok(());
        }
        let mut r = PacketReader::new(payload);
        // The player the client names is ignored in favour of the connection it came in on.
        let _claimed = r.u8()?;
        let action = r.u8()?;
        match action {
            1 => self.summon_skeletron(),
            // A sundial or a moondial skipping to the next morning or evening.
            3 => self.skip_to(true),
            6 => self.skip_to(false),
            other => {
                debug!(slot, action = other, "ignoring a misc-data action");
                self.broadcast(packets::verbatim(id::MISC_DATA_SYNC, payload)?, Some(slot));
            }
        }
        Ok(())
    }

    /// Module 4: Journey mode powers. See [`crate::game::journey`]'s own module doc for exactly
    /// which of vanilla's fifteen this covers.
    ///
    /// `slot` is only used by the three per-player powers, and only ever to *override* whatever
    /// player index the wire carried — `APerPlayerTogglePower`/`APerPlayerSliderPower`'s own
    /// `DeserializeNetMessage` does the identical substitution on a dedicated server
    /// (`Main.netMode == 2`), which is why the proto layer discards that byte entirely rather than
    /// handing it up: a client cannot toggle Godmode for somebody else by lying about which slot
    /// it is. No permission-level check yet for any power (`PowerPermissionLevel` — real vanilla
    /// lets an operator configure who may flip each power, `journeypermission_<name>` in its own
    /// config) — every connected player may use every power this server models, disclosed rather
    /// than silently assumed.
    pub(super) fn on_creative_power(
        &mut self,
        slot: u8,
        message: net_module::CreativePowerMessage,
    ) -> terrustia_proto::Result<()> {
        use net_module::{CreativePowerMessage, power};

        match message {
            // The four buttons share `set_time` with `/time` — same effect, same values
            // (`DAY_LENGTH/2`/`NIGHT_LENGTH/2` for noon/midnight match `SkipToTime`'s own
            // `27000`/`16200` exactly; see that pair's own doc comment on the constants).
            CreativePowerMessage::Button(id) => {
                let set = match id {
                    power::START_DAY => Some((true, 0)),
                    power::START_NOON => Some((true, DAY_LENGTH / 2)),
                    power::START_NIGHT => Some((false, 0)),
                    power::START_MIDNIGHT => Some((false, NIGHT_LENGTH / 2)),
                    _ => None,
                };
                if let Some((day_time, time)) = set {
                    self.set_time(day_time, time);
                }
            }
            CreativePowerMessage::Toggle(id, enabled) => {
                if self.journey.set(id, enabled)
                    && let Ok(frame) = net_module::creative_power_toggle(id, enabled)
                {
                    // A dedicated server broadcasts the accepted state to everyone, the toggling
                    // player included — that player's own client does not apply its request
                    // locally, it waits to be told, the same request/confirm shape `RequestUse`
                    // uses in source.
                    self.broadcast(frame, None);
                }
            }
            // Four real, different effects behind the same wire shape:
            // - `ModifyTimeRate`/`Difficulty`: stored (`journey.time_rate_slider`/
            //   `difficulty_slider`), read every tick (`tick()`'s own `journey.time_rate()` call)
            //   or on demand (`effective_difficulty()`, called wherever this server used to read
            //   `world.game_mode` directly for anything difficulty-scaled).
            // - `ModifyWind`/`ModifyRain`: applied straight to `self.weather` and forgotten —
            //   neither is `_syncToJoiningPlayers` nor `IPersistentPerWorldContent` in source (see
            //   `journey.rs`'s own module doc), so there is nothing to hold onto here at all.
            CreativePowerMessage::Slider(id, value) => {
                let applied = match id {
                    power::MODIFY_TIME_RATE => {
                        self.journey.time_rate_slider = value;
                        true
                    }
                    power::DIFFICULTY => {
                        self.journey.difficulty_slider = value;
                        true
                    }
                    power::MODIFY_WIND => {
                        // `MathHelper.Lerp(-0.8f, 0.8f, value)`, set to both the current wind and
                        // its target at once — `ModifyWindDirectionAndStrength::
                        // UpdateInfoFromSliderValueCache`'s own two assignments.
                        let wind = -0.8 + value.clamp(0.0, 1.0) * 1.6;
                        self.weather.wind = wind;
                        self.weather.target = wind;
                        self.world.wind = wind;
                        true
                    }
                    power::MODIFY_RAIN => {
                        // `Main.StartRain(instant: true, value)`/`Main.StopRain(instant: true)`.
                        // Real vanilla rain set this way has no timer at all; this project's own
                        // rain model is timer-driven (`Weather::tick_rain`'s own countdown), so a
                        // long sentinel approximates "does not expire on its own" rather than
                        // removing the timer concept entirely — disclosed, not a silent gap.
                        if value <= 0.0 {
                            self.weather.stop_rain();
                        } else {
                            self.weather.raining = true;
                            self.weather.max_rain = value.clamp(0.0, 1.0);
                            self.weather.rain_time = i32::MAX;
                        }
                        self.world.raining = self.weather.raining;
                        self.world.rain_time = self.weather.rain_time;
                        self.world.max_rain = self.weather.max_rain;
                        true
                    }
                    _ => false,
                };
                if applied && let Ok(frame) = net_module::creative_power_slider(id, value) {
                    self.broadcast(frame, None);
                }
            }
            // `Godmode`/`FarPlacementRange`. `slot` — the real sender, never the wire's own
            // (discarded) player-index byte — is both what gets toggled and, once accepted, what
            // the confirmation names: exactly `SetEnabledState`'s own
            // `NetManager.Instance.Broadcast` of the same `SyncOnePlayer` shape to everyone,
            // toggling player included (its own client waits to be told, same as the shared
            // toggles above).
            CreativePowerMessage::PerPlayerToggle(id, enabled) => {
                let applied = match id {
                    power::GODMODE => {
                        self.journey.set_godmode(slot, enabled);
                        true
                    }
                    power::FAR_PLACEMENT_RANGE => {
                        self.journey.set_far_placement_range(slot, enabled);
                        true
                    }
                    _ => false,
                };
                if applied
                    && let Ok(frame) =
                        net_module::creative_power_toggle_for_player(id, slot, enabled)
                {
                    self.broadcast(frame, None);
                }
            }
            // `SpawnRate`. Stored for `slot` only — real vanilla's own `DeserializeNetMessage` has
            // no broadcast branch here at all (unlike the toggle shape above): another player's
            // personal spawn-rate preference is never anyone else's business, nothing to relay.
            CreativePowerMessage::PerPlayerSlider(id, value) => {
                if id == power::SPAWN_RATE {
                    self.journey.set_spawn_rate_slider(slot, value);
                }
            }
        }
        Ok(())
    }
}

/// Borrow helper: `introduce` needs the slot list detached from `self`.
fn other_slots(slots: &[u8]) -> Vec<u8> {
    slots.to_vec()
}

/// `Main.teamColor` (`Main.cs:6795-6800`): none/red/green/blue/yellow/pink, in that team-index
/// order. Used to colour the PvP-toggle and team-change chat lines the same way vanilla's own
/// dedicated server does (`MessageBuffer.cs:1863-1864`, `:2337`).
fn team_colour(team: u8) -> [u8; 3] {
    match team {
        1 => [218, 59, 59],
        2 => [59, 218, 85],
        3 => [59, 149, 218],
        4 => [242, 221, 100],
        5 => [224, 100, 242],
        _ => [255, 255, 255],
    }
}

/// `Chest.IsLocked(Tile)` (`Chest.cs:295-310`), used by [`GameServer::can_craft_from_chest`]. A
/// locked chest is not a different tile type — the ordinary chest (21) shifted along its frame
/// strip into one of six locked-style ranges, or the second chest tile (467) at style 13 exactly.
/// `terrustia_proto::locks` already knows the same style numbers from the other direction
/// (unlocking one), but not this "is it currently locked" read of an arbitrary frame.
fn is_chest_tile_locked(tile: Tile) -> bool {
    let frame_x = i32::from(tile.frame_x);
    if tile.block == terrustia_proto::locks::CHEST {
        return (72..=106).contains(&frame_x)
            || (144..=178).contains(&frame_x)
            || (828..=1006).contains(&frame_x)
            || (1296..=1330).contains(&frame_x)
            || (1368..=1402).contains(&frame_x)
            || (1440..=1474).contains(&frame_x);
    }
    if tile.block == terrustia_proto::locks::CHEST_2 {
        return frame_x / 36 == 13;
    }
    false
}

/// A join's own tile stream is spread across ticks (`drain_section_streams`) rather than sent in
/// one synchronous loop inside `on_spawn_tile_data`'s own packet handler — see
/// `SECTION_STREAM_BUDGET`'s own doc comment for the measured cost this bounds.
#[cfg(test)]
mod section_streaming {
    use super::*;
    use crate::config::Config;

    /// A real generated world, not an empty one: the measured section-encode costs
    /// `SECTION_STREAM_BUDGET` was set from (`examples/sectioncost.rs`, up to ~1,322µs on a
    /// comparable size) come from real terrain, and an empty world's sections would encode to
    /// almost nothing, proving nothing about pacing.
    fn real_world() -> crate::world::World {
        crate::world::worldgen::build(2400, 900, "section stream probe", 1234).0
    }

    /// A large outbound channel: this drains section frames without a live client reading them,
    /// and `send_bytes` drops (removes) a player whose channel fills up — sized well past
    /// anything one small world's own section count could produce.
    fn with_one_player(mut server: GameServer) -> (GameServer, mpsc::Receiver<Bytes>) {
        let (out_tx, out_rx) = mpsc::channel(100_000);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::WorldSent;
        server.players[0] = Some(player);
        (server, out_rx)
    }

    fn all_sections(server: &GameServer) -> VecDeque<(i32, i32)> {
        (0..server.world.sections_x())
            .flat_map(|sx| (0..server.world.sections_y()).map(move |sy| (sx, sy)))
            .collect()
    }

    /// The core architectural change: `SpawnTileData` used to stream every wanted section
    /// synchronously inside its own packet handler, reaching `TilesSent` in the same call. Now it
    /// only queues them — the state advance is deferred to whichever tick actually empties the
    /// queue.
    #[test]
    fn a_join_does_not_reach_tiles_sent_until_its_whole_section_queue_has_drained() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), real_world()));

        // What a real client's handshake sends: `x = -1, y = -1` (no extra requested position),
        // `team = 0`.
        let mut payload = Vec::new();
        payload.extend_from_slice(&(-1i32).to_le_bytes());
        payload.extend_from_slice(&(-1i32).to_le_bytes());
        payload.push(0);

        server.on_spawn_tile_data(0, &payload).unwrap();

        assert!(
            !server.player(0).unwrap().pending_sections.is_empty(),
            "a real world's own spawn block should always want at least one section"
        );
        assert_eq!(
            server.player(0).unwrap().state,
            ConnState::WorldSent,
            "must not reach TilesSent before every queued section has actually gone out"
        );

        for _ in 0..10_000 {
            if server.player(0).unwrap().pending_sections.is_empty() {
                break;
            }
            server.drain_section_streams();
        }

        assert!(
            server.player(0).unwrap().pending_sections.is_empty(),
            "the queue never actually finished draining"
        );
        assert_eq!(
            server.player(0).unwrap().state,
            ConnState::TilesSent,
            "should reach TilesSent once the queue actually empties"
        );
    }

    /// The other half of the same change: draining is bounded per call, not "as many as fit."
    /// Queuing every section a real world has and draining once must leave some behind —
    /// regardless of how cheap any individual section happens to be, since there is always a
    /// number of them large enough to blow a four-millisecond budget.
    #[test]
    fn one_drain_call_never_empties_a_large_enough_queue() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), real_world()));
        let all = all_sections(&server);
        let total = all.len();
        assert!(total > 8, "world too small to prove anything about pacing");
        server.player_mut(0).unwrap().pending_sections = all;

        server.drain_section_streams();

        let remaining = server.player(0).unwrap().pending_sections.len();
        assert!(
            remaining > 0,
            "all {total} sections drained in a single call — the budget bounded nothing"
        );
    }

    /// The budget is shared across every player streaming at once, not given to each one
    /// separately: two simultaneous joiners must not be able to drain more than one alone would in
    /// the same call — that would let a burst of simultaneous joins reproduce the exact stall this
    /// whole mechanism exists to prevent, just triggered by many joiners instead of one, exactly
    /// the scaling bug an earlier draft of this fix actually had (a `began` per player rather than
    /// per call).
    ///
    /// Driven over a fixed section cap (`drain_section_streams_bounded(Some(cap), ..)`) rather than
    /// the wall clock. The production drain stops on a four-millisecond budget, and how many
    /// sections fit in four milliseconds swings with CI scheduling, so comparing a solo drain's
    /// count against a paired drain's over the wall clock could flip either way under load even when
    /// the budget was correctly shared. The section cap makes the shared accounting exact without
    /// weakening the property: a shared budget drains exactly `cap` sections whether one player or
    /// two are queued, while a per-player budget would drain `2 * cap` for two.
    #[test]
    fn the_drain_budget_is_shared_across_players_not_given_to_each_one() {
        let (mut solo, _rx) = with_one_player(GameServer::new(Config::default(), real_world()));
        let queued = all_sections(&solo).len();
        // Half the world's sections: strictly fewer than one player has queued, so the drain always
        // stops on the cap with sections still pending rather than emptying a queue early.
        let cap = queued / 2;
        assert!(
            cap > 0,
            "world too small to prove anything about a shared cap"
        );

        solo.player_mut(0).unwrap().pending_sections = all_sections(&solo);
        solo.drain_section_streams_bounded(Some(cap), Duration::MAX);
        let solo_sent = queued - solo.player(0).unwrap().pending_sections.len();
        assert_eq!(
            solo_sent, cap,
            "one player alone should drain exactly the shared cap"
        );

        let (mut paired, _rx_a) = with_one_player(GameServer::new(Config::default(), real_world()));
        let (out_tx_b, _rx_b) = mpsc::channel(100_000);
        let mut player_b = Player::new(1, "127.0.0.1:2".parse().unwrap(), out_tx_b);
        player_b.state = ConnState::WorldSent;
        paired.players[1] = Some(player_b);
        paired.player_mut(0).unwrap().pending_sections = all_sections(&paired);
        paired.player_mut(1).unwrap().pending_sections = all_sections(&paired);

        paired.drain_section_streams_bounded(Some(cap), Duration::MAX);

        let paired_sent = (queued - paired.player(0).unwrap().pending_sections.len())
            + (queued - paired.player(1).unwrap().pending_sections.len());
        assert_eq!(
            paired_sent, solo_sent,
            "two simultaneous joiners together drained {paired_sent} sections in one call, \
             vs {solo_sent} for one alone — the budget is being given to each player \
             separately instead of shared across the whole call"
        );
    }
}

/// Lane F: the join password (`on_password`, above) is backed off per address by
/// `admin::throttle`, and its compare goes through the shared `admin::constant_time_eq` rather
/// than a plain `!=` (`constant_time_eq`'s own tests, in `admin::mod`, cover the primitive itself:
/// timing is not something a unit test can observe, so what matters here is that this call site
/// actually uses it and that the throttle actually gates it).
#[cfg(test)]
mod join_password_throttle {
    use super::*;
    use crate::config::Config;

    fn server_with_password(password: &str) -> GameServer {
        let config = Config {
            password: password.to_string(),
            ..Config::default()
        };
        GameServer::new(
            config,
            crate::world::World::empty(200, 150, "join password throttle probe"),
        )
    }

    /// A fresh connection at `slot`, past the version check and ready for a password: what a real
    /// client's handshake reaches by the time it may send one (`on_hello`'s own `player.greeted =
    /// true` before it prompts for a password at all).
    ///
    /// Returns the outbound channel's receiver, which the caller must hold onto for as long as the
    /// connection is meant to look alive: `send_bytes` treats a closed channel exactly like a dead
    /// connection and removes the player on the very next thing sent to it (accept's own
    /// `player_info` frame included, not only a kick), so dropping the receiver immediately would
    /// make every outcome below look identical.
    fn connect(server: &mut GameServer, slot: u8, addr: &str) -> mpsc::Receiver<Bytes> {
        let (out_tx, out_rx) = mpsc::channel(64);
        let mut player = Player::new(slot, addr.parse().expect("valid test address"), out_tx);
        player.greeted = true;
        server.players[usize::from(slot)] = Some(player);
        out_rx
    }

    /// `BinaryReader.ReadString`'s own wire shape for a short ASCII string: a one-byte 7-bit-encoded
    /// length (true for anything this test sends), then the bytes.
    fn password_payload(password: &str) -> Vec<u8> {
        let mut payload = vec![password.len() as u8];
        payload.extend_from_slice(password.as_bytes());
        payload
    }

    #[test]
    fn the_right_password_is_accepted_when_nothing_has_throttled_it() {
        let mut server = server_with_password("secret");
        let _rx = connect(&mut server, 0, "127.0.0.1:51000");

        server.on_password(0, &password_payload("secret")).unwrap();

        assert_eq!(
            server.player(0).map(|p| p.state),
            Some(ConnState::SlotAssigned),
            "the right password should be accepted and the connection kept"
        );
    }

    /// Fail-then-pass for the throttle itself: `FREE_ATTEMPTS + 1` wrong passwords from the same
    /// address opens a window (proven deterministically, with an injected clock, in
    /// `admin::throttle`'s own tests) and the next connection from that address is refused before
    /// its password is even compared. Proven here by offering the *right* password on that next
    /// connection and it still getting kicked: the only way that happens is the throttle refusing
    /// before `constant_time_eq` is ever reached.
    ///
    /// Each attempt is its own connection because a wrong join password kicks immediately
    /// (`on_password`'s own `else` arm): there was never a way to retry more than once on a
    /// single socket, throttled or not, so this reconnects with a fresh `Player` each time exactly
    /// as a real client retrying from the same address would.
    #[test]
    fn a_throttled_address_is_kicked_even_with_the_right_password() {
        let mut server = server_with_password("secret");
        let addr = "127.0.0.1:51001";

        for attempt in 0..=crate::admin::throttle::FREE_ATTEMPTS {
            let _rx = connect(&mut server, 0, addr);
            server.on_password(0, &password_payload("wrong")).unwrap();
            assert!(
                server.player(0).is_none(),
                "attempt {attempt}: a wrong password must still kick, exactly as before"
            );
        }

        let _rx = connect(&mut server, 0, addr);
        server.on_password(0, &password_payload("secret")).unwrap();
        assert!(
            server.player(0).is_none(),
            "the window should still be open, so even the right password gets kicked"
        );
    }

    /// A different address is never affected by another one's window: the reason a caller keys
    /// this per-IP at all rather than refusing every join once one address fails enough times.
    /// Same octets but for the port on purpose, matching this whole module's own point that the
    /// throttle keys on the address alone: two different *ports* at the same address (an ordinary
    /// reconnect) must still share one window, so proving "different" means a different address
    /// outright, not just a different socket.
    #[test]
    fn an_unrelated_address_is_never_throttled_by_someone_elses_failures() {
        let mut server = server_with_password("secret");
        for attempt in 0..=crate::admin::throttle::FREE_ATTEMPTS {
            let _rx = connect(&mut server, 0, "127.0.0.1:51002");
            server.on_password(0, &password_payload("wrong")).unwrap();
            assert!(server.player(0).is_none(), "attempt {attempt}");
        }

        let _rx = connect(&mut server, 1, "127.0.0.2:51002");
        server.on_password(1, &password_payload("secret")).unwrap();
        assert_eq!(
            server.player(1).map(|p| p.state),
            Some(ConnState::SlotAssigned),
            "a different address's own first attempt must not inherit someone else's backoff"
        );
    }
}

/// FIX-6 [30]/[45,157]: `TogglePVP` and `TeamChange` relayed the state correctly but never sent
/// the localized chat line vanilla always broadcasts alongside it (`MessageBuffer.cs:1851-1866`
/// for PvP, `:2325-2364` for team).
#[cfg(test)]
mod pvp_and_team_chat_lines {
    use super::*;
    use crate::config::Config;
    use terrustia_proto::net_text::TextMode;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "pvp/team chat line probe")
    }

    fn with_players(count: u8, mut server: GameServer) -> (GameServer, Vec<mpsc::Receiver<Bytes>>) {
        let mut rxs = Vec::new();
        for slot in 0..count {
            let (tx, rx) = mpsc::channel(16);
            let mut player = Player::new(slot, "127.0.0.1:1".parse().unwrap(), tx);
            player.state = ConnState::Playing;
            server.players[slot as usize] = Some(player);
            rxs.push(rx);
        }
        (server, rxs)
    }

    fn pvp_payload(hostile: bool) -> Vec<u8> {
        vec![0, hostile as u8]
    }

    fn team_payload(team: u8) -> Vec<u8> {
        vec![0, team]
    }

    /// Drain every module-1 (text) frame off a channel and decode it back to a `NetworkText`,
    /// ignoring anything else (the ordinary packet-30/45 state relay lands on the same channel).
    fn text_frames(rx: &mut mpsc::Receiver<Bytes>) -> Vec<NetworkText> {
        let mut out = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if frame.get(2) != Some(&terrustia_proto::id::NET_MODULES) {
                continue;
            }
            let mut r = PacketReader::new(&frame[3..]);
            if r.u16() != Ok(net_module::MODULE_TEXT) {
                continue;
            }
            r.u8().unwrap(); // author
            out.push(NetworkText::read(&mut r).unwrap());
        }
        out
    }

    #[test]
    fn enabling_pvp_broadcasts_lang_mp_11_to_everyone_including_the_toggler() {
        let (mut server, mut rxs) =
            with_players(2, GameServer::new(Config::default(), tiny_world()));
        server.on_pvp(0, &pvp_payload(true)).unwrap();

        for (slot, rx) in rxs.iter_mut().enumerate() {
            let texts = text_frames(rx);
            let line = texts
                .iter()
                .find(|t| t.mode == TextMode::LocalizationKey)
                .unwrap_or_else(|| panic!("slot {slot} should hear the PvP-on announcement"));
            assert_eq!(line.text, "LegacyMultiplayer.11");
            assert_eq!(line.substitutions.len(), 1);
        }
    }

    #[test]
    fn disabling_pvp_broadcasts_lang_mp_12() {
        let (mut server, mut rxs) =
            with_players(1, GameServer::new(Config::default(), tiny_world()));
        server.on_pvp(0, &pvp_payload(false)).unwrap();

        let line = text_frames(&mut rxs[0])
            .into_iter()
            .find(|t| t.mode == TextMode::LocalizationKey)
            .expect("the disabling line should still be sent");
        assert_eq!(line.text, "LegacyMultiplayer.12");
    }

    /// `MessageBuffer.cs:2348-2354`: the team-change line goes only to the changer, whoever was on
    /// the old team, and whoever is now on the new team — never a full broadcast. Slot 0 changes
    /// from team 1 to team 2; slot 1 stays on team 1 (old team — should hear it); slot 2 is
    /// already on team 2 (new team — should hear it); slot 3 is on team 3 (neither — must not).
    #[test]
    fn a_team_change_reaches_only_the_changer_and_old_and_new_teammates() {
        let (mut server, mut rxs) =
            with_players(4, GameServer::new(Config::default(), tiny_world()));
        server.player_mut(0).unwrap().team = 1;
        server.player_mut(1).unwrap().team = 1;
        server.player_mut(2).unwrap().team = 2;
        server.player_mut(3).unwrap().team = 3;

        server.on_team(0, &team_payload(2)).unwrap();

        for slot in [0usize, 1, 2] {
            assert!(
                text_frames(&mut rxs[slot])
                    .iter()
                    .any(|t| t.mode == TextMode::LocalizationKey),
                "slot {slot} should have heard the team-change line"
            );
        }
        assert!(
            text_frames(&mut rxs[3]).is_empty(),
            "a bystander on an unrelated team must not hear it"
        );
    }

    #[test]
    fn an_ordinary_team_uses_lang_mp_thirteen_plus_team() {
        let (mut server, mut rxs) =
            with_players(1, GameServer::new(Config::default(), tiny_world()));
        server.on_team(0, &team_payload(3)).unwrap();

        let line = text_frames(&mut rxs[0])
            .into_iter()
            .find(|t| t.mode == TextMode::LocalizationKey)
            .unwrap();
        assert_eq!(line.text, "LegacyMultiplayer.16", "13 + team 3");
    }

    /// The one quirk worth pinning by itself: team 5 (pink) does not follow the `13 + team`
    /// formula (which would be `mp[18]`) — vanilla special-cases it to `mp[22]`
    /// (`MessageBuffer.cs:2344-2347`).
    #[test]
    fn team_five_uses_lang_mp_22_not_the_formula() {
        let (mut server, mut rxs) =
            with_players(1, GameServer::new(Config::default(), tiny_world()));
        server.on_team(0, &team_payload(5)).unwrap();

        let line = text_frames(&mut rxs[0])
            .into_iter()
            .find(|t| t.mode == TextMode::LocalizationKey)
            .unwrap();
        assert_eq!(line.text, "LegacyMultiplayer.22");
    }
}

/// Packet 55 (`AddPlayerBuffPvP`): a PvP-flagged player's own hit spreads one of `Main.pvpBuff`'s
/// twenty — now generated as `terrustia_proto::buffs::PVP_BUFF` — real buffs onto another
/// PvP-flagged player. Real vanilla's own server relays this to exactly the named target, not to
/// everyone, and gates it on both players being hostile-flagged and the buff itself being one of
/// the whitelisted twenty.
#[cfg(test)]
mod pvp_buff_spread {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "pvp buff spread probe")
    }

    /// Three players: the attacker, the real target, and a bystander who must never see this —
    /// proof that this is a targeted relay, not `relay_player_packet`'s usual broadcast.
    fn with_three_players(
        mut server: GameServer,
    ) -> (
        GameServer,
        mpsc::Receiver<Bytes>,
        mpsc::Receiver<Bytes>,
        mpsc::Receiver<Bytes>,
    ) {
        let (attacker_tx, attacker_rx) = mpsc::channel(16);
        let (target_tx, target_rx) = mpsc::channel(16);
        let (bystander_tx, bystander_rx) = mpsc::channel(16);
        for (slot, tx) in [(0, attacker_tx), (1, target_tx), (2, bystander_tx)] {
            let mut player = Player::new(slot, "127.0.0.1:1".parse().unwrap(), tx);
            player.state = ConnState::Playing;
            server.players[slot as usize] = Some(player);
        }
        (server, attacker_rx, target_rx, bystander_rx)
    }

    /// Poisoned (20) — one of the real twenty, transcribed directly from `Main.pvpBuff`, not
    /// guessed.
    const REAL_PVP_BUFF: u16 = 20;
    /// An ordinary, non-PvP-spreadable debuff — real vanilla's own `Main.debuff[21]`
    /// (PotionSickness), never added to `pvpBuff`.
    const ORDINARY_DEBUFF: u16 = 21;

    /// Just the payload `on_pvp_buff_spread` reads — no length prefix or message id, which is
    /// what a real dispatch has already stripped by the time a handler ever sees `payload`
    /// (`PacketWriter::finish` builds the whole framed packet instead, one layer too many here).
    fn packet_55(target: u8, buff: u16, duration: i32) -> Vec<u8> {
        let mut payload = Vec::with_capacity(7);
        payload.push(target);
        payload.extend_from_slice(&buff.to_le_bytes());
        payload.extend_from_slice(&duration.to_le_bytes());
        payload
    }

    #[test]
    fn both_hostile_and_a_real_pvp_buff_reaches_only_the_named_target() {
        let (mut server, _attacker_rx, mut target_rx, mut bystander_rx) =
            with_three_players(GameServer::new(Config::default(), tiny_world()));
        server.player_mut(0).unwrap().pvp = true;
        server.player_mut(1).unwrap().pvp = true;

        let payload = packet_55(1, REAL_PVP_BUFF, 300);
        server.on_pvp_buff_spread(0, &payload).unwrap();

        assert!(
            target_rx.try_recv().is_ok(),
            "the named target should have received the relayed buff"
        );
        assert!(
            bystander_rx.try_recv().is_err(),
            "nobody else should ever see this — it is a targeted relay, not a broadcast"
        );
    }

    #[test]
    fn neither_player_needs_to_be_hostile_for_nothing_to_happen() {
        let (mut server, _attacker_rx, mut target_rx, _bystander_rx) =
            with_three_players(GameServer::new(Config::default(), tiny_world()));
        // Neither player.pvp is set — the ordinary, ungated case.
        let payload = packet_55(1, REAL_PVP_BUFF, 300);
        server.on_pvp_buff_spread(0, &payload).unwrap();

        assert!(
            target_rx.try_recv().is_err(),
            "a non-hostile attacker must not be able to spread a buff at all"
        );
    }

    #[test]
    fn a_buff_outside_the_real_whitelist_is_refused_even_with_both_hostile() {
        let (mut server, _attacker_rx, mut target_rx, _bystander_rx) =
            with_three_players(GameServer::new(Config::default(), tiny_world()));
        server.player_mut(0).unwrap().pvp = true;
        server.player_mut(1).unwrap().pvp = true;

        let payload = packet_55(1, ORDINARY_DEBUFF, 300);
        server.on_pvp_buff_spread(0, &payload).unwrap();

        assert!(
            target_rx.try_recv().is_err(),
            "only the real twenty `Main.pvpBuff` ids may be spread this way"
        );
    }
}

/// Does a client hammering tile edits get stopped, the way vanilla stops one?
///
/// Vanilla has this and we did not, which makes it a regression *from* the game rather than a
/// place where we are merely as trusting as it is. `RemoteClient` keeps a counter per kind, bumps
/// it per edit packet, decays it each tick and boots past a ceiling. The numbers are transcribed
/// rather than chosen, so a client vanilla tolerates is tolerated here and vice versa.
///
/// The numbers were the only half transcribed at first. Vanilla also refuses to *apply* them
/// unless the server was started with `secure=1`/`-secure` (`Netplay.SpamCheck`, `Netplay.cs:65`,
/// read by `RemoteClient.SpamUpdate` at `RemoteClient.cs:70-80` and by the liquid counter at
/// `MessageBuffer.cs:2415`), and the tests below asserted the ceilings without ever modelling that
/// axis, which is how a default-on kick passed review.
#[cfg(test)]
mod tile_spam {
    use super::*;
    use crate::config::Config;

    fn playing(server: &mut GameServer, slot: u8) -> mpsc::Receiver<Bytes> {
        let (tx, rx) = mpsc::channel(64);
        let mut player = Player::new(slot, "127.0.0.1:1".parse().expect("test address"), tx);
        player.state = ConnState::Playing;
        server.players[usize::from(slot)] = Some(player);
        rx
    }

    fn server(spam_check: bool) -> GameServer {
        GameServer::new(
            Config {
                spam_check,
                ..Config::default()
            },
            crate::world::World::empty(200, 150, "tile spam probe"),
        )
    }

    /// Fail-then-pass for the missing gate: a stock server must not boot anybody for edit spam,
    /// because a stock vanilla server does not either (`RemoteClient.cs:70-80`, which zeroes all
    /// four counters and returns when `Netplay.SpamCheck` is false).
    ///
    /// Six hundred breaks in one burst is not a hypothetical: a stick of dynamite clears a sphere
    /// of tiles at once, and `spam_break`'s ceiling is 500 against a 5-a-tick decay, so ordinary
    /// play used to disconnect the player who threw it.
    #[test]
    fn a_stock_server_never_kicks_for_edit_spam() {
        let mut server = server(false);
        let _rx = playing(&mut server, 0);

        for edit in 0..600 {
            assert!(
                !server.note_tile_spam(0, TileAction::KillTile),
                "edit {edit} was refused on a server with the spam check off"
            );
        }
        assert!(
            server.player(0).is_some(),
            "a player mining hard must still be connected on a stock server"
        );
    }

    /// ...and the mechanism is still there for an operator who asks for it, which is the other
    /// half of the gate being a gate rather than a deletion.
    #[test]
    fn a_secure_server_still_kicks_for_edit_spam() {
        let mut server = server(true);
        let _rx = playing(&mut server, 0);

        let tripped = (0..600).any(|_| server.note_tile_spam(0, TileAction::KillTile));
        assert!(tripped, "600 breaks is over the 500 ceiling and must trip");
        assert!(server.player(0).is_none(), "and the client is disconnected");
    }

    /// The liquid counter is gated at the increment rather than at the boot, because that is where
    /// vanilla gates it (`MessageBuffer.cs:2415`).
    #[test]
    fn the_liquid_counter_only_counts_on_a_secure_server() {
        for (spam_check, expected) in [(false, 0.0), (true, 3.0)] {
            let mut server = server(spam_check);
            let _rx = playing(&mut server, 0);
            for _ in 0..3 {
                server.on_liquid(0, &[10, 0, 10, 0, 255, 0]).unwrap();
            }
            assert_eq!(
                server.player(0).map(|p| p.spam_liquid),
                Some(expected),
                "spam_check = {spam_check}"
            );
        }
    }

    /// Placing is the tight one: 100, recovering 0.3 a tick.
    #[test]
    fn the_ceilings_and_decay_match_the_game() {
        assert_eq!(SPAM_PLACE_MAX, 100.0);
        assert_eq!(SPAM_PLACE_DECAY, 0.3);
        assert_eq!(SPAM_BREAK_MAX, 500.0);
        assert_eq!(SPAM_BREAK_DECAY, 5.0);
        assert_eq!(SPAM_LIQUID_MAX, 50.0);
        assert_eq!(SPAM_LIQUID_DECAY, 0.2);
    }

    /// Sustained placing above the decay rate eventually trips; ordinary building does not.
    ///
    /// At 60 ticks a second, 0.3 a tick is eighteen placements a second recovered. A player
    /// building fast is well under that; a script is not.
    #[test]
    fn a_realistic_building_rate_never_trips_the_limit() {
        // Ten placements a second for a solid minute, decaying each tick.
        let mut budget = 0.0f32;
        let mut worst = 0.0f32;
        for tick in 0..3600 {
            if tick % 6 == 0 {
                budget += 1.0;
            }
            budget = (budget - SPAM_PLACE_DECAY).max(0.0);
            worst = worst.max(budget);
        }
        assert!(
            worst < SPAM_PLACE_MAX,
            "ten placements a second reached {worst}, which would boot a player who is just \
             building quickly"
        );
    }

    /// And a client placing as fast as it can trips within a few seconds.
    #[test]
    fn a_flood_of_placements_trips_the_limit() {
        let mut budget = 0.0f32;
        let mut tripped = None;
        for tick in 0..600 {
            // Twenty a tick — a script, not a person.
            budget += 20.0;
            budget = (budget - SPAM_PLACE_DECAY).max(0.0);
            if budget > SPAM_PLACE_MAX && tripped.is_none() {
                tripped = Some(tick);
            }
        }
        let tripped = tripped.expect("a flood has to trip the limit");
        assert!(
            tripped < 60,
            "a flood took {tripped} ticks to be noticed; that is a second of free vandalism"
        );
    }

    /// Breaking is deliberately looser, because mining legitimately produces packets very fast.
    ///
    /// A `const` block, so swapping the two by accident fails the build rather than a test run.
    const _: () = assert!(SPAM_BREAK_MAX > SPAM_PLACE_MAX);
    const _: () = assert!(SPAM_BREAK_DECAY > SPAM_PLACE_DECAY);
}

/// What a connection is allowed to say, and what a relay is allowed to carry.
///
/// Five findings from one audit of the sixty-six handlers, and they share a shape: the server was
/// trusting a byte, or a connection, that vanilla does not.
#[cfg(test)]
mod untrusted_packets {
    use super::*;
    use crate::config::Config;

    fn world() -> crate::world::World {
        crate::world::World::empty(200, 150, "untrusted packet probe")
    }

    fn frame(id: u8, payload: &[u8]) -> Frame {
        Frame {
            id,
            payload: Bytes::copy_from_slice(payload),
        }
    }

    /// A connection at `slot` in whatever state the caller asks for. Holding the receiver keeps it
    /// looking alive: a closed channel is a dead connection to `send_bytes`.
    fn connect(server: &mut GameServer, slot: u8, state: ConnState) -> mpsc::Receiver<Bytes> {
        let (tx, rx) = mpsc::channel(64);
        let mut player = Player::new(slot, "127.0.0.1:1".parse().expect("test address"), tx);
        player.greeted = true;
        player.password_ok = state != ConnState::Greeting;
        player.state = state;
        server.players[usize::from(slot)] = Some(player);
        rx
    }

    fn frames(rx: &mut mpsc::Receiver<Bytes>) -> Vec<Bytes> {
        let mut out = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            out.push(frame);
        }
        out
    }

    /// Every frame of one id, with the two-byte length prefix and the id stripped.
    fn payloads(rx: &mut mpsc::Receiver<Bytes>, id: u8) -> Vec<Vec<u8>> {
        frames(rx)
            .into_iter()
            .filter(|f| f.get(2) == Some(&id))
            .map(|f| f[3..].to_vec())
            .collect()
    }

    /// Packet 61's payload: the player the client claims, then what it wants summoned. -11 is
    /// Advanced Combat Techniques, a permanent world unlock.
    const COMBAT_BOOK: [u8; 4] = [0, 0, 0xF5, 0xFF];

    /// B-01, fail-then-pass: a socket that has been asked for a password could still change the
    /// world.
    ///
    /// `MessageBuffer.cs:158-172` gates every packet on the connection's state before the switch
    /// ever runs; this server had no such gate and left each handler to check for itself, which
    /// twelve of them did not. `on_summon` is one: with `password` set, `on_hello` sends the
    /// prompt and returns, and this packet used to sail straight into a permanent world unlock
    /// without the password ever being offered.
    #[test]
    fn a_connection_that_has_not_authenticated_cannot_change_the_world() {
        let mut server = GameServer::new(
            Config {
                password: "hunter2".into(),
                ..Config::default()
            },
            world(),
        );
        let _rx = connect(&mut server, 0, ConnState::Greeting);

        server.handle_packet(
            0,
            frame(id::SPAWN_BOSS_USE_LICENSE_START_EVENT, &COMBAT_BOOK),
        );

        assert!(
            !server.world.progress.combat_book,
            "a connection still owing a password must not reach the world at all"
        );
        assert!(server.player(0).is_none(), "and is disconnected for trying");
    }

    /// The same gate mid-handshake: past the password, before the world, ids above 12 are refused.
    #[test]
    fn a_client_still_joining_cannot_edit_tiles() {
        let mut server = GameServer::new(Config::default(), world());
        let _rx = connect(&mut server, 0, ConnState::SlotAssigned);

        server.handle_packet(0, frame(id::TILE_MANIPULATION, &[0; 9]));

        assert!(server.player(0).is_none(), "vanilla boots for this");
    }

    /// ...and the handshake's own packets still get through, or nobody could ever join.
    #[test]
    fn the_handshake_packets_are_still_allowed_through() {
        let mut server = GameServer::new(Config::default(), world());
        let mut rx = connect(&mut server, 0, ConnState::SlotAssigned);

        server.handle_packet(0, frame(id::REQUEST_WORLD_DATA, &[]));

        assert!(server.player(0).is_some(), "packet 6 is part of joining");
        assert!(
            frames(&mut rx)
                .iter()
                .any(|f| f.get(2) == Some(&id::WORLD_DATA)),
            "and it is answered"
        );
    }

    /// A player who has finished joining is not gated at all.
    #[test]
    fn a_playing_client_can_still_use_the_combat_book() {
        let mut server = GameServer::new(Config::default(), world());
        let _rx = connect(&mut server, 0, ConnState::Playing);

        server.handle_packet(
            0,
            frame(id::SPAWN_BOSS_USE_LICENSE_START_EVENT, &COMBAT_BOOK),
        );

        assert!(server.world.progress.combat_book);
    }

    /// Freeing the bound Old Slime is once per world, and the world has to remember it.
    ///
    /// `NPC.TransformElderSlime` (`NPC.cs:19172-19193`) is guarded on `!unlockedSlimeOldSpawn` and
    /// sets it before transforming. Fails before the fix in both halves: nothing recorded the
    /// unlock, so a saved world forgot the rescue and the caverns went on offering bound slimes,
    /// and nothing refused a second packet, so any client could turn a fresh 685 into an Old Slime
    /// for as long as the caverns kept producing them.
    #[test]
    fn freeing_the_bound_old_slime_is_once_per_world_and_is_remembered() {
        use crate::game::server::{OLD_SLIME, OLD_SLIME_SOURCE, TRANSFORM_ELDER_SLIME};

        let mut server = GameServer::new(Config::default(), world());
        let _rx = connect(&mut server, 0, ConnState::Playing);
        let index = server
            .npcs
            .spawn(OLD_SLIME_SOURCE, (100.0, 100.0))
            .expect("a slot for the bound slime");

        // Packet 140's payload: a one-byte case, then the NPC index as an `i32`.
        let mut payload = vec![TRANSFORM_ELDER_SLIME];
        payload.extend_from_slice(&i32::from(index).to_le_bytes());
        server.handle_packet(0, frame(id::SET_MISC_EVENT_VALUES, &payload));

        assert_eq!(
            server.npcs.get(index).expect("still there").npc_type,
            OLD_SLIME
        );
        assert!(
            server.world.progress.unlocked_slime_old,
            "the unlock is world state; without it the rescue is forgotten on the next save"
        );

        // A second one, on a fresh bound slime, must be refused outright.
        let again = server
            .npcs
            .spawn(OLD_SLIME_SOURCE, (200.0, 100.0))
            .expect("a slot for a second bound slime");
        let mut payload = vec![TRANSFORM_ELDER_SLIME];
        payload.extend_from_slice(&i32::from(again).to_le_bytes());
        server.handle_packet(0, frame(id::SET_MISC_EVENT_VALUES, &payload));
        assert_eq!(
            server.npcs.get(again).expect("still there").npc_type,
            OLD_SLIME_SOURCE,
            "one Old Slime per world (`NPC.cs:19178`)"
        );
    }

    /// M-02, fail-then-pass: packet 117 names its victim, and the relay used to overwrite that
    /// byte with the sender's own slot, so every third-party client applied a PvP hit to the
    /// attacker. `NetMessage.SendPlayerHurt`'s first argument is `playerTargetIndex`
    /// (`NetMessage.cs:2633`), and vanilla hands the byte back untouched
    /// (`MessageBuffer.cs:3890-3906`).
    #[test]
    fn a_pvp_hit_still_names_its_victim_when_it_is_relayed() {
        let mut server = GameServer::new(Config::default(), world());
        let _attacker = connect(&mut server, 0, ConnState::Playing);
        let mut victim = connect(&mut server, 1, ConnState::Playing);
        let mut bystander = connect(&mut server, 2, ConnState::Playing);
        for slot in 0..3 {
            if let Some(p) = server.player_mut(slot) {
                p.pvp = true;
            }
        }

        // Victim 1, then whatever the rest of the packet holds; only the first byte matters here.
        server.handle_packet(0, frame(id::PLAYER_HURT_V2, &[1, 0, 0, 5, 0, 1, 0, 0]));

        for (who, rx) in [("the victim", &mut victim), ("a bystander", &mut bystander)] {
            let hurt = payloads(rx, id::PLAYER_HURT_V2);
            assert_eq!(hurt.len(), 1, "{who} should be told once");
            assert_eq!(hurt[0][0], 1, "{who} was told the wrong player was hit");
        }
    }

    /// ...and the gate that has to come with it: without the rewrite, nothing else stops a client
    /// naming anybody at all. Vanilla's own condition is `whoAmI == num27 || (both hostile)`.
    #[test]
    fn hurting_somebody_who_is_not_in_pvp_is_refused() {
        let mut server = GameServer::new(Config::default(), world());
        let _attacker = connect(&mut server, 0, ConnState::Playing);
        let mut victim = connect(&mut server, 1, ConnState::Playing);
        if let Some(p) = server.player_mut(0) {
            p.pvp = true;
        }

        server.handle_packet(0, frame(id::PLAYER_HURT_V2, &[1, 0, 0, 5, 0, 1, 0, 0]));

        assert!(
            payloads(&mut victim, id::PLAYER_HURT_V2).is_empty(),
            "a player who is not in PvP cannot be hurt by somebody else's packet"
        );
    }

    /// M-05, fail-then-pass: negative damage is not a small hit, it is a delete.
    ///
    /// The server clamps at `MessageBuffer.cs:1785-1788` before relaying, because a receiving
    /// client branches on the sign (`:1795-1803`): below zero means `life = 0; HitEffect();
    /// active = false`. Relaying the client's own -1 made every other client drop the NPC while
    /// this server kept simulating it.
    #[test]
    fn a_negative_hit_is_clamped_before_it_is_relayed() {
        let mut server = GameServer::new(Config::default(), world());
        let _attacker = connect(&mut server, 0, ConnState::Playing);
        let mut watcher = connect(&mut server, 1, ConnState::Playing);
        let index = server.npcs.spawn(1, (100.0, 100.0)).expect("a slime");
        let generation = server.npcs.get(index).expect("just spawned").generation;
        let _ = frames(&mut watcher); // the spawn's own packet

        let mut payload = vec![index, generation];
        payload.extend_from_slice(&(-1i16).to_le_bytes());
        payload.extend_from_slice(&0f32.to_le_bytes());
        payload.push(2); // direction +1
        payload.push(0); // not a crit
        server.handle_packet(0, frame(id::DAMAGE_N_P_C, &payload));

        let relayed = payloads(&mut watcher, id::DAMAGE_N_P_C);
        assert_eq!(relayed.len(), 1, "the hit is still relayed");
        assert_eq!(
            i16::from_le_bytes([relayed[0][2], relayed[0][3]]),
            0,
            "but with the damage floored, or the NPC vanishes on every other client"
        );
        assert!(
            server.npcs.get(index).is_some_and(|n| n.is_alive()),
            "and it is still alive here"
        );
    }

    /// M-07, fail-then-pass: the ping is an echo to the sender, not a broadcast to everybody else.
    ///
    /// `MessageBuffer.cs:4445-4452` is `TrySendData(154, whoAmI)`. The client sends one every
    /// 250 ms and blocks on the answer (`Ping.cs`), so relaying it to the wrong people left the
    /// sender's `CurrentPing` climbing for ever and gave everyone else four stray frames a second.
    #[test]
    fn a_ping_comes_back_to_the_sender_alone() {
        let mut server = GameServer::new(Config::default(), world());
        let mut sender = connect(&mut server, 0, ConnState::Playing);
        let mut other = connect(&mut server, 1, ConnState::Playing);

        server.handle_packet(0, frame(id::PING, &[]));

        assert_eq!(payloads(&mut sender, id::PING).len(), 1, "echoed back");
        assert!(
            payloads(&mut other, id::PING).is_empty(),
            "and nobody else hears it"
        );
    }

    /// M-08, fail-then-pass: the ids a dedicated server reads and drops are not relayed.
    ///
    /// Each of these sits inside an `if (Main.netMode == 1)` in `MessageBuffer`, so vanilla's
    /// server never passes one on. Relaying them gave any client a coloured server-looking notice
    /// (107), a cannon fired from another player's client with attacker-chosen damage (108), and
    /// eight more effects on everybody else's screen.
    #[test]
    fn client_only_packets_are_never_relayed() {
        let mut server = GameServer::new(Config::default(), world());
        let _sender = connect(&mut server, 0, ConnState::Playing);
        let mut other = connect(&mut server, 1, ConnState::Playing);

        for id in [
            id::SMART_TEXT_MESSAGE,
            id::WIRED_CANNON_SHOT,
            id::COMBAT_TEXT_INT,
            id::COMBAT_TEXT_STRING,
            id::SYNC_EMOTE_BUBBLE,
            id::ACHIEVEMENT_MESSAGE_N_P_C_KILLED,
            id::ACHIEVEMENT_MESSAGE_EVENT_HAPPENED,
            id::POOF_OF_SMOKE,
            id::SYNC_REVENGE_MARKER,
            id::REMOVE_REVENGE_MARKER,
            id::TAMPER_WITH_N_P_C,
            id::PLAY_LEGACY_SOUND,
            id::TEMPORARY_ANIMATION,
        ] {
            server.handle_packet(0, frame(id, &[0; 8]));
            assert!(
                payloads(&mut other, id).is_empty(),
                "packet {id} ({}) must not reach another client",
                id::name(id)
            );
        }
    }

    /// Packet 112 carries two unrelated effects. A server relays the tree one to *everybody*
    /// (`MessageBuffer.cs:3848-3857`, `TrySendData(b, -1, -1, ..)`) and the fairy one to nobody.
    #[test]
    fn only_the_tree_half_of_the_special_effects_packet_is_relayed() {
        let mut server = GameServer::new(Config::default(), world());
        let mut sender = connect(&mut server, 0, ConnState::Playing);
        let mut other = connect(&mut server, 1, ConnState::Playing);

        let mut tree = vec![1];
        tree.extend_from_slice(&[0; 11]);
        server.handle_packet(0, frame(id::SPECIAL_F_X, &tree));
        assert_eq!(payloads(&mut other, id::SPECIAL_F_X).len(), 1);
        assert_eq!(
            payloads(&mut sender, id::SPECIAL_F_X).len(),
            1,
            "the sender's own tree has to pop too, so it is not excluded"
        );

        let mut fairy = vec![2];
        fairy.extend_from_slice(&[0; 11]);
        server.handle_packet(0, frame(id::SPECIAL_F_X, &fairy));
        assert!(payloads(&mut other, id::SPECIAL_F_X).is_empty());
    }
}

/// Emptying a tile has to reach the other clients on its own.
///
/// `MessageBuffer.cs:2438-2442`: the server relays packet 48 to everyone but the sender the moment
/// a client reports a tile drained to zero, and leaves every other amount to the simulation, which
/// tells clients as it moves the liquid on. Ours took the amount and woke the tile, and nothing
/// else: for an isolated pool there is no later flow to carry the news, so every client that was
/// already in the section kept rendering liquid the server no longer had.
#[cfg(test)]
mod liquid_drain_relay {
    use super::*;
    use crate::config::Config;

    fn two_players() -> (GameServer, Vec<mpsc::Receiver<Bytes>>) {
        let mut server = GameServer::new(
            Config::default(),
            crate::world::World::empty(200, 150, "liquid relay probe"),
        );
        let mut rxs = Vec::new();
        for slot in 0..2 {
            let (tx, rx) = mpsc::channel(64);
            let mut player = Player::new(slot, "127.0.0.1:1".parse().expect("test address"), tx);
            player.state = ConnState::Playing;
            server.players[usize::from(slot)] = Some(player);
            rxs.push(rx);
        }
        (server, rxs)
    }

    /// Every module-0 liquid change waiting on a channel.
    fn liquid_frames(rx: &mut mpsc::Receiver<Bytes>) -> Vec<net_module::LiquidChange> {
        let mut out = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if frame.get(2) != Some(&terrustia_proto::id::NET_MODULES) {
                continue;
            }
            if let Ok(Some(changes)) = net_module::decode_liquid_changes(&frame[3..]) {
                out.extend(changes);
            }
        }
        out
    }

    /// `x`, `y`, amount, liquid type, as packet 48 carries them.
    fn payload(x: i16, y: i16, amount: u8) -> Vec<u8> {
        let mut p = x.to_le_bytes().to_vec();
        p.extend_from_slice(&y.to_le_bytes());
        p.push(amount);
        p.push(0);
        p
    }

    #[test]
    fn draining_a_tile_reaches_the_other_clients() {
        let (mut server, mut rxs) = two_players();
        let mut tile = server.world.tile(60, 40);
        tile.liquid = 255;
        server.world.set_tile(60, 40, tile);

        server.on_liquid(0, &payload(60, 40, 0)).unwrap();

        let told = liquid_frames(&mut rxs[1]);
        assert_eq!(
            told,
            vec![net_module::LiquidChange {
                x: 60,
                y: 40,
                amount: 0,
                kind: 0,
            }],
            "the other client has to be told the tile is empty"
        );
        assert!(
            liquid_frames(&mut rxs[0]).is_empty(),
            "the sender already emptied it locally; vanilla excludes it (`SendData(48, -1, whoAmI)`)"
        );
    }

    /// A pour is not relayed: vanilla only sends for `b2 == 0`, leaving anything else to the
    /// simulation, which broadcasts as the liquid actually moves.
    #[test]
    fn a_pour_is_left_to_the_simulation() {
        let (mut server, mut rxs) = two_players();
        server.on_liquid(0, &payload(60, 40, 255)).unwrap();
        assert!(liquid_frames(&mut rxs[1]).is_empty());
    }
}

/// Server MINOR (C1-b item 5): opening a chest now tells everyone else which chest this player
/// has open (packet 80), and opening a *rigged* one (`WorldGen.IsChestRigged`, tile 467 style 4)
/// fires its own wiring circuit — `MessageBuffer.cs:1868-1895`.
#[cfg(test)]
mod chest_open_minors {
    use super::*;
    use crate::config::Config;
    use crate::world::objects::Chest;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "chest open minors probe")
    }

    fn with_two_players(
        mut server: GameServer,
    ) -> (GameServer, mpsc::Receiver<Bytes>, mpsc::Receiver<Bytes>) {
        // Wide enough that opening a chest's own flood of per-slot item packets (`DEFAULT_CHEST_SLOTS`
        // = 40, plus the size and `SyncPlayerChest` frames) never fills the queue and gets the
        // opener dropped as "cannot keep up" (`send_bytes`'s own `TrySendError::Full` branch).
        let (opener_tx, opener_rx) = mpsc::channel(64);
        let (other_tx, other_rx) = mpsc::channel(64);
        for (slot, tx) in [(0u8, opener_tx), (1u8, other_tx)] {
            let mut player = Player::new(slot, "127.0.0.1:1".parse().unwrap(), tx);
            player.state = ConnState::Playing;
            server.players[slot as usize] = Some(player);
        }
        (server, opener_rx, other_rx)
    }

    fn open_chest_payload(x: i16, y: i16) -> Vec<u8> {
        let mut payload = Vec::with_capacity(4);
        payload.extend_from_slice(&x.to_le_bytes());
        payload.extend_from_slice(&y.to_le_bytes());
        payload
    }

    #[test]
    fn opening_a_chest_tells_the_other_player_via_packet_80() {
        let (mut server, mut opener_rx, mut other_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));
        let id = server
            .world
            .add_chest(Chest::empty_at(10, 10))
            .expect("room for a chest");

        server
            .on_chest_open(0, &open_chest_payload(10, 10))
            .unwrap();

        assert_eq!(server.player(0).unwrap().open_chest, id);
        // Drain the opener's own stream (chest size, its items, SyncPlayerChest) without caring
        // about their exact count, then confirm the *other* player got told which chest it was.
        let mut found = false;
        while let Ok(frame) = other_rx.try_recv() {
            if frame[2] == terrustia_proto::id::SYNC_PLAYER_CHEST_INDEX {
                let sync = SyncPlayerChestIndex::decode(&frame[3..]).unwrap();
                assert_eq!(
                    sync,
                    SyncPlayerChestIndex {
                        player: 0,
                        chest: id
                    }
                );
                found = true;
            }
        }
        assert!(found, "the other player should hear packet 80");
        // The opener already knows its own state from packet 33; it should not also get packet 80.
        assert!(
            std::iter::from_fn(|| opener_rx.try_recv().ok())
                .all(|frame| frame[2] != terrustia_proto::id::SYNC_PLAYER_CHEST_INDEX),
            "the opener itself should be excluded from the packet 80 broadcast"
        );
    }

    // `TileID.ActiveStoneBlock`/`InactiveStoneBlock` — a plain, already-proven wiring effect
    // (`wiring.rs`'s own `stone_blocks_hide_and_reappear`) reused here as the observable half of
    // an end-to-end trigger, rather than re-deriving a fresh mechanism just for this test.
    const ACTIVE_STONE: u16 = 130;

    fn wired_tile(block: u16, frame: (i16, i16)) -> terrustia_proto::Tile {
        let mut tile = if terrustia_proto::tile_sets::frame_important(block) {
            terrustia_proto::Tile::framed(block, frame.0, frame.1)
        } else {
            terrustia_proto::Tile::block(block)
        };
        tile.flags
            .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
        tile
    }

    #[test]
    fn opening_a_rigged_chest_fires_its_wiring_circuit() {
        let (mut server, _opener_rx, mut other_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));
        server
            .world
            .add_chest(Chest::empty_at(10, 10))
            .expect("room for a chest");
        // Tile 467 (Containers2), style 4 — `WorldGen.IsChestRigged`'s own exact test — wired
        // straight to an active stone block a few tiles over.
        for x in 10..12 {
            server.world.set_tile(x, 10, wired_tile(467, (4 * 36, 0)));
        }
        for x in 12..15 {
            let mut wire = terrustia_proto::Tile::AIR;
            wire.flags
                .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
            server.world.set_tile(x, 10, wire);
        }
        server
            .world
            .set_tile(15, 10, wired_tile(ACTIVE_STONE, (0, 0)));
        server
            .world
            .set_tile(15, 9, terrustia_proto::Tile::block(1)); // something to stand on

        server
            .on_chest_open(0, &open_chest_payload(10, 10))
            .unwrap();

        assert_eq!(
            server.world.tile(15, 10).block,
            131, // InactiveStoneBlock
            "the stone block should have been hidden by the circuit"
        );
        let told_switch_hit = std::iter::from_fn(|| other_rx.try_recv().ok())
            .any(|frame| frame[2] == terrustia_proto::id::HIT_SWITCH);
        assert!(told_switch_hit, "packet 59 should announce the switch hit");
    }

    /// The control case: an ordinary chest (not tile 467 at all) never touches wiring.
    #[test]
    fn opening_an_ordinary_chest_does_not_touch_wiring() {
        let (mut server, _opener_rx, mut other_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));
        server
            .world
            .add_chest(Chest::empty_at(10, 10))
            .expect("room for a chest");
        server
            .world
            .set_tile(10, 10, terrustia_proto::Tile::framed(21, 0, 0));

        server
            .on_chest_open(0, &open_chest_payload(10, 10))
            .unwrap();

        assert!(
            std::iter::from_fn(|| other_rx.try_recv().ok())
                .all(|frame| frame[2] != terrustia_proto::id::HIT_SWITCH),
            "an ordinary chest should never fire a switch"
        );
    }
}

/// FIX-6 [82/9]/[82/2]: the particles and ping modules used not to be relayed at all — a real
/// server re-broadcasts both to every other client (`NetParticlesModule.cs:22-25`,
/// `NetPingModule.cs:19-22`), unconditionally, without acting on the contents.
#[cfg(test)]
mod net_module_relays {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "net module relay probe")
    }

    fn with_two_players(
        mut server: GameServer,
    ) -> (GameServer, mpsc::Receiver<Bytes>, mpsc::Receiver<Bytes>) {
        let (a_tx, a_rx) = mpsc::channel(16);
        let (b_tx, b_rx) = mpsc::channel(16);
        for (slot, tx) in [(0u8, a_tx), (1u8, b_tx)] {
            let mut player = Player::new(slot, "127.0.0.1:1".parse().unwrap(), tx);
            player.state = ConnState::Playing;
            server.players[slot as usize] = Some(player);
        }
        (server, a_rx, b_rx)
    }

    #[test]
    fn a_particles_module_reaches_the_other_player_but_not_the_sender() {
        let (mut server, mut sender_rx, mut other_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));
        let mut w = terrustia_proto::writer::Writer::new();
        w.u16(net_module::MODULE_PARTICLES).u8(3).bytes(&[9, 9]);

        server.on_net_module(0, w.as_slice()).unwrap();

        let relayed = other_rx
            .try_recv()
            .expect("the other client should hear it");
        assert_eq!(relayed[2], terrustia_proto::id::NET_MODULES);
        assert_eq!(
            &relayed[3..],
            w.as_slice(),
            "relayed byte for byte, unchanged"
        );
        assert!(
            sender_rx.try_recv().is_err(),
            "the sender should not hear its own particles back"
        );
    }

    #[test]
    fn a_ping_module_reaches_the_other_player_but_not_the_sender() {
        let (mut server, mut sender_rx, mut other_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));
        let mut w = terrustia_proto::writer::Writer::new();
        w.u16(net_module::MODULE_PING).f32(100.0).f32(200.0);

        server.on_net_module(0, w.as_slice()).unwrap();

        let relayed = other_rx
            .try_recv()
            .expect("the other client should hear it");
        assert_eq!(&relayed[3..], w.as_slice());
        assert!(sender_rx.try_recv().is_err());
    }

    #[test]
    fn neither_relay_fires_for_a_connection_that_is_not_playing_yet() {
        let (mut server, _sender_rx, mut other_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));
        server.player_mut(0).unwrap().state = ConnState::Greeting;
        let mut w = terrustia_proto::writer::Writer::new();
        w.u16(net_module::MODULE_PING).f32(0.0).f32(0.0);

        server.on_net_module(0, w.as_slice()).unwrap();

        assert!(other_rx.try_recv().is_err());
    }
}

/// FIX-6 [82/12]: `NetCraftingRequestsModule` (craft using a nearby open chest's materials) used
/// to be entirely unimplemented — the module id was not even named, so a real client's request
/// silently vanished and the craft neither completed nor refunded (it would have hung until the
/// client gave up).
#[cfg(test)]
mod craft_requests {
    use super::*;
    use crate::config::Config;
    use crate::world::objects::Chest;
    use terrustia_proto::ItemStack;

    const WOOD: i32 = 9;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "craft request probe")
    }

    fn with_two_players(
        mut server: GameServer,
    ) -> (GameServer, mpsc::Receiver<Bytes>, mpsc::Receiver<Bytes>) {
        let (a_tx, a_rx) = mpsc::channel(16);
        let (b_tx, b_rx) = mpsc::channel(16);
        for (slot, tx) in [(0u8, a_tx), (1u8, b_tx)] {
            let mut player = Player::new(slot, "127.0.0.1:1".parse().unwrap(), tx);
            player.state = ConnState::Playing;
            server.players[slot as usize] = Some(player);
        }
        (server, a_rx, b_rx)
    }

    fn chest_with(items: &[ItemStack]) -> Chest {
        let mut chest = Chest::empty_at(10, 10);
        for (slot, item) in items.iter().enumerate() {
            chest.items[slot] = *item;
        }
        chest
    }

    fn request(items: &[(i32, i32)], chests: &[i16]) -> net_module::CraftRequest {
        net_module::CraftRequest {
            items: items
                .iter()
                .map(|&(item_id_or_group, stack)| net_module::CraftIngredient {
                    item_id_or_group,
                    stack,
                })
                .collect(),
            chests: chests.iter().map(|&id| Some(id)).collect(),
        }
    }

    /// The ordinary case: the chest holds enough, so the request is approved, the stock actually
    /// leaves the chest, and every client (not only the requester) is told the slot changed —
    /// `NetMessage.SendData(32, ...)` is a broadcast, not a targeted send
    /// (`CraftingRequests.cs:285`).
    #[test]
    fn a_covered_request_consumes_stock_and_approves() {
        let (mut server, mut requester_rx, mut other_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));
        let id = server
            .world
            .add_chest(chest_with(&[ItemStack::new(WOOD, 10, 0)]))
            .unwrap();

        server
            .on_craft_request(0, request(&[(WOOD, 6)], &[id]))
            .unwrap();

        assert_eq!(
            server.world.chest(id).unwrap().items[0].stack,
            4,
            "six of ten should have been taken"
        );
        let approved = std::iter::from_fn(|| requester_rx.try_recv().ok())
            .find(|f| f[2] == terrustia_proto::id::NET_MODULES)
            .expect("the requester should hear a response");
        assert_eq!(
            net_module::peek_module_id(&approved[3..]).unwrap(),
            net_module::MODULE_CRAFTING_REQUESTS
        );
        assert_eq!(approved[approved.len() - 1], 1, "approved is a true byte");
        let told_slot_change = std::iter::from_fn(|| other_rx.try_recv().ok())
            .any(|f| f[2] == terrustia_proto::id::SYNC_CHEST_ITEM);
        assert!(
            told_slot_change,
            "every client, not only the requester, should hear the chest slot changed"
        );
    }

    /// Not enough of the item anywhere in the offered chests: denied, and nothing moves. Approval
    /// is all-or-nothing (`CraftingRequests.cs:311`), never a partial take.
    #[test]
    fn an_uncovered_request_is_denied_and_untouched() {
        let (mut server, mut requester_rx, _other_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));
        let id = server
            .world
            .add_chest(chest_with(&[ItemStack::new(WOOD, 2, 0)]))
            .unwrap();

        server
            .on_craft_request(0, request(&[(WOOD, 6)], &[id]))
            .unwrap();

        assert_eq!(
            server.world.chest(id).unwrap().items[0].stack,
            2,
            "an uncovered request must not touch the chest at all"
        );
        let response = std::iter::from_fn(|| requester_rx.try_recv().ok())
            .find(|f| f[2] == terrustia_proto::id::NET_MODULES)
            .expect("the requester should still hear a response");
        assert_eq!(response[response.len() - 1], 0, "denied is a false byte");
    }

    /// `Chest.IsLocked` (`Chest.cs:295-310`): a locked chest is refused outright, however much it
    /// holds — style 2 (frame_x 72) is the ordinary locked dungeon/gold chest.
    #[test]
    fn a_locked_chest_is_never_usable_however_much_it_holds() {
        let (mut server, mut requester_rx, _other_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));
        let id = server
            .world
            .add_chest(chest_with(&[ItemStack::new(WOOD, 99, 0)]))
            .unwrap();
        // `can_craft_from_chest` reads the tile at exactly `(chest.x, chest.y)` — `Chest::empty_at`
        // above recorded that as `(10, 10)`.
        server.world.set_tile(10, 10, Tile::framed(21, 72, 0));

        server
            .on_craft_request(0, request(&[(WOOD, 6)], &[id]))
            .unwrap();

        assert_eq!(server.world.chest(id).unwrap().items[0].stack, 99);
        let response = std::iter::from_fn(|| requester_rx.try_recv().ok())
            .find(|f| f[2] == terrustia_proto::id::NET_MODULES)
            .unwrap();
        assert_eq!(response[response.len() - 1], 0, "a locked chest must deny");
    }

    /// `Chest.UsingChest` (`Chest.cs:492-505`): a chest somebody *else* has open is off limits,
    /// but the requester's own open chest is fine (`num != whoAmI` in
    /// `CraftingRequests.CanCraftFromChest`, `CraftingRequests.cs:300-304`).
    #[test]
    fn a_chest_open_by_someone_else_is_refused_but_the_requesters_own_is_fine() {
        let (mut server, mut requester_rx, _other_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));
        let id = server
            .world
            .add_chest(chest_with(&[ItemStack::new(WOOD, 10, 0)]))
            .unwrap();
        server.player_mut(1).unwrap().open_chest = id;

        server
            .on_craft_request(0, request(&[(WOOD, 6)], &[id]))
            .unwrap();

        assert_eq!(
            server.world.chest(id).unwrap().items[0].stack,
            10,
            "someone else has it open, so nothing should move"
        );
        let response = std::iter::from_fn(|| requester_rx.try_recv().ok())
            .find(|f| f[2] == terrustia_proto::id::NET_MODULES)
            .unwrap();
        assert_eq!(response[response.len() - 1], 0);

        // Now the requester has it open instead — the exact same request should now succeed.
        server.player_mut(1).unwrap().open_chest = -1;
        server.player_mut(0).unwrap().open_chest = id;
        server
            .on_craft_request(0, request(&[(WOOD, 6)], &[id]))
            .unwrap();
        assert_eq!(server.world.chest(id).unwrap().items[0].stack, 4);
    }

    /// This server has no `RecipeGroup` table to check a fake-item-id entry against real chest
    /// contents (`CraftIngredient::is_recipe_group`'s own doc), so it always denies rather than
    /// guessing — the disclosed seam, not a silent wrong answer.
    #[test]
    fn a_recipe_group_entry_is_always_denied() {
        let (mut server, mut requester_rx, _other_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));
        let id = server
            .world
            .add_chest(chest_with(&[ItemStack::new(WOOD, 99, 0)]))
            .unwrap();
        let group_id = net_module::CraftIngredient::RECIPE_GROUP_OFFSET + 1;

        server
            .on_craft_request(0, request(&[(group_id, 1)], &[id]))
            .unwrap();

        assert_eq!(server.world.chest(id).unwrap().items[0].stack, 99);
        let response = std::iter::from_fn(|| requester_rx.try_recv().ok())
            .find(|f| f[2] == terrustia_proto::id::NET_MODULES)
            .unwrap();
        assert_eq!(response[response.len() - 1], 0);
    }
}

/// Server MINOR (C1-b item 5): the summon-book sub-actions -11/-17
/// (`MessageBuffer.cs:2822-2827`, `2848-2853`) used to fall into `on_summon`'s own
/// unrecognised-negative-id fallback and do nothing.
#[cfg(test)]
mod combat_book_summons {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "combat book summon probe")
    }

    fn with_one_player(mut server: GameServer) -> (GameServer, mpsc::Receiver<Bytes>) {
        let (out_tx, out_rx) = mpsc::channel(16);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::Playing;
        server.players[0] = Some(player);
        (server, out_rx)
    }

    fn summon_payload(what: i16) -> Vec<u8> {
        let mut payload = Vec::with_capacity(4);
        payload.extend_from_slice(&0i16.to_le_bytes()); // claimed player slot, ignored
        payload.extend_from_slice(&what.to_le_bytes());
        payload
    }

    #[test]
    fn sub_action_negative_eleven_marks_the_first_combat_book_read() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        assert!(!server.world.progress.combat_book);

        server.on_summon(0, &summon_payload(-11)).unwrap();

        assert!(server.world.progress.combat_book);
    }

    #[test]
    fn sub_action_negative_seventeen_marks_the_second_combat_book_read() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        assert!(!server.world.progress.combat_book_two);

        server.on_summon(0, &summon_payload(-17)).unwrap();

        assert!(server.world.progress.combat_book_two);
    }
}

/// Server MINOR (C1-b item 5): the teleport guard on player controls
/// (`MessageBuffer.cs:998-1002`, `player13.unacknowledgedTeleports > 0`).
#[cfg(test)]
mod teleport_controls_guard {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "teleport controls guard probe")
    }

    fn with_one_player(mut server: GameServer) -> (GameServer, mpsc::Receiver<Bytes>) {
        let (out_tx, out_rx) = mpsc::channel(16);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::Playing;
        server.players[0] = Some(player);
        (server, out_rx)
    }

    /// Just the payload `on_player_controls` reads: player slot, four control-flag bytes, the
    /// selected item, and a position, with no trailing velocity block.
    fn controls_payload(position: (f32, f32)) -> Vec<u8> {
        let mut payload = Vec::with_capacity(11);
        payload.push(0); // player slot, ignored in favour of the connection's own
        payload.extend_from_slice(&[0, 0, 0, 0]); // control flags
        payload.push(0); // selected item
        payload.extend_from_slice(&position.0.to_le_bytes());
        payload.extend_from_slice(&position.1.to_le_bytes());
        payload
    }

    /// Just the payload `on_teleport` reads for an ack: `flags = 3` (both bit 0 and bit 1 set),
    /// the claimed player slot, a zeroed position, and a zeroed style — matching what real
    /// vanilla's own `NetMessage.TrySendData(65, -1, -1, null, 3, num82)` call actually puts on
    /// the wire (`NetMessage.cs:1092-1112`'s writer has no optional fields for this case).
    fn teleport_ack_payload() -> Vec<u8> {
        let mut payload = Vec::with_capacity(12);
        payload.push(3);
        payload.extend_from_slice(&0i16.to_le_bytes());
        payload.extend_from_slice(&0f32.to_le_bytes());
        payload.extend_from_slice(&0f32.to_le_bytes());
        payload.push(0);
        payload
    }

    #[test]
    fn an_unacknowledged_teleport_makes_controls_ignore_the_reported_position() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        server.player_mut(0).unwrap().position = (100.0, 100.0);
        server.player_mut(0).unwrap().unacknowledged_teleports = 1;

        server
            .on_player_controls(0, &controls_payload((9999.0, 9999.0)))
            .unwrap();

        assert_eq!(
            server.player(0).unwrap().position,
            (100.0, 100.0),
            "the stale pre-teleport position should have been ignored"
        );
    }

    #[test]
    fn an_ordinary_controls_update_is_trusted_once_acknowledged() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        server.player_mut(0).unwrap().position = (100.0, 100.0);
        server.player_mut(0).unwrap().unacknowledged_teleports = 0;

        server
            .on_player_controls(0, &controls_payload((9999.0, 9999.0)))
            .unwrap();

        assert_eq!(server.player(0).unwrap().position, (9999.0, 9999.0));
    }

    #[test]
    fn the_client_ack_clears_the_guard() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        server.player_mut(0).unwrap().unacknowledged_teleports = 1;

        server.on_teleport(0, &teleport_ack_payload()).unwrap();

        assert_eq!(server.player(0).unwrap().unacknowledged_teleports, 0);
    }

    /// A server-issued teleport (a Teleportation Potion) sets the guard, which the very next
    /// controls packet — the one already in flight when the teleport landed — must not undo.
    #[test]
    fn a_server_teleport_sets_the_guard_end_to_end() {
        let (mut server, mut rx) =
            with_one_player(GameServer::new(Config::default(), tiny_world()));
        server.player_mut(0).unwrap().position = (100.0, 100.0);

        // The Shellphone's spawn setting: unlike a potion's roaming search, it always finds
        // somewhere (the world's own spawn point), which keeps this test's world plain and empty.
        let payload = vec![SHELLPHONE_SPAWN];
        server.on_server_teleport(0, &payload).unwrap();
        // Drain whatever landing spot it found; only the guard state matters here.
        let _ = std::iter::from_fn(|| rx.try_recv().ok()).count();

        assert_eq!(server.player(0).unwrap().unacknowledged_teleports, 1);

        let after_teleport = server.player(0).unwrap().position;
        server
            .on_player_controls(0, &controls_payload((100.0, 100.0)))
            .unwrap();
        assert_eq!(
            server.player(0).unwrap().position,
            after_teleport,
            "the stale controls packet must not undo the teleport"
        );
    }
}

/// Packet 66 (C1-b item 6): a heal-on-touch projectile's own effect (`Projectile.cs:28951`,
/// `aiStyle == 52`), applied server-side and relayed — `MessageBuffer.cs:3038-3056`.
#[cfg(test)]
mod heal_on_touch {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "heal on touch probe")
    }

    fn with_two_players(
        mut server: GameServer,
    ) -> (GameServer, mpsc::Receiver<Bytes>, mpsc::Receiver<Bytes>) {
        let (sender_tx, sender_rx) = mpsc::channel(16);
        let (healed_tx, healed_rx) = mpsc::channel(16);
        for (slot, tx) in [(0u8, sender_tx), (1u8, healed_tx)] {
            let mut player = Player::new(slot, "127.0.0.1:1".parse().unwrap(), tx);
            player.state = ConnState::Playing;
            player.life = 50;
            player.life_max = 100;
            server.players[slot as usize] = Some(player);
        }
        (server, sender_rx, healed_rx)
    }

    fn heal_payload(player: u8, amount: i16) -> Vec<u8> {
        let mut payload = Vec::with_capacity(3);
        payload.push(player);
        payload.extend_from_slice(&amount.to_le_bytes());
        payload
    }

    #[test]
    fn a_positive_heal_is_applied_and_relayed() {
        let (mut server, mut sender_rx, _healed_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));

        server.on_heal_player(0, &heal_payload(1, 30)).unwrap();

        assert_eq!(server.player(1).unwrap().life, 80);
        // Relayed to everyone but the sender.
        assert!(sender_rx.try_recv().is_err());
    }

    #[test]
    fn a_heal_never_exceeds_the_targets_own_life_max() {
        let (mut server, _sender_rx, _healed_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));

        server.on_heal_player(0, &heal_payload(1, 9999)).unwrap();

        assert_eq!(server.player(1).unwrap().life, 100, "clamped to life_max");
    }

    /// Matches `if (num72 > 0)` exactly: zero and negative amounts are ignored outright, not
    /// clamped to zero and applied.
    #[test]
    fn a_non_positive_heal_amount_is_ignored() {
        let (mut server, _sender_rx, _healed_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));

        server.on_heal_player(0, &heal_payload(1, 0)).unwrap();
        assert_eq!(server.player(1).unwrap().life, 50);

        server.on_heal_player(0, &heal_payload(1, -10)).unwrap();
        assert_eq!(server.player(1).unwrap().life, 50);
    }

    /// A target slot with nobody connected there is a no-op, not a panic.
    #[test]
    fn an_out_of_range_target_does_not_panic() {
        let (mut server, _sender_rx, _healed_rx) =
            with_two_players(GameServer::new(Config::default(), tiny_world()));

        assert!(server.on_heal_player(0, &heal_payload(200, 30)).is_ok());
    }
}

/// The handlers that were relaying a client's word for something instead of doing it.
///
/// Every test here fails against the code as it stood: each names a packet whose whole effect was
/// a broadcast, with the server's own world left as it was. The common thread is a wrong belief
/// about what else the client sends - a tile square, a packet 79, a packet 17 that destroys - and
/// in each case the client sends nothing of the sort.
#[cfg(test)]
mod server_side_effects {
    use super::*;
    use crate::config::Config;
    use terrustia_proto::tile::Tile;

    fn world() -> crate::world::World {
        crate::world::World::empty(400, 300, "server-side effect probe")
    }

    fn frame(id: u8, payload: &[u8]) -> Frame {
        Frame {
            id,
            payload: Bytes::copy_from_slice(payload),
        }
    }

    fn connect(server: &mut GameServer, slot: u8) -> mpsc::Receiver<Bytes> {
        let (tx, rx) = mpsc::channel(64);
        let mut player = Player::new(slot, "127.0.0.1:1".parse().expect("test address"), tx);
        player.greeted = true;
        player.password_ok = true;
        player.state = ConnState::Playing;
        server.players[usize::from(slot)] = Some(player);
        rx
    }

    /// Everything waiting on a connection, as `(id, payload)`. Drained once per test, because
    /// several of these tests care about two different ids in the same batch.
    fn drain(rx: &mut mpsc::Receiver<Bytes>) -> Vec<(u8, Vec<u8>)> {
        let mut out = Vec::new();
        while let Ok(f) = rx.try_recv() {
            if let Some(&id) = f.get(2) {
                out.push((id, f[3..].to_vec()));
            }
        }
        out
    }

    fn of(frames: &[(u8, Vec<u8>)], id: u8) -> Vec<&[u8]> {
        frames
            .iter()
            .filter(|(i, _)| *i == id)
            .map(|(_, p)| p.as_slice())
            .collect()
    }

    /// Packet 34's payload, as a client writes it: the id field is always zero from a client
    /// (`NetMessage.cs:916-930`'s `else writer.Write((short)0)`).
    fn chest_packet(action: u8, x: i16, y: i16, style: i16) -> Vec<u8> {
        let mut payload = vec![action];
        payload.extend_from_slice(&x.to_le_bytes());
        payload.extend_from_slice(&y.to_le_bytes());
        payload.extend_from_slice(&style.to_le_bytes());
        payload.extend_from_slice(&0i16.to_le_bytes());
        payload
    }

    /// A shut wooden door, three tiles tall, with its top row at `top`.
    fn shut_door(server: &mut GameServer, x: i32, top: i32) {
        for dy in 0..3i32 {
            server.world.set_tile(
                x,
                top + dy,
                Tile::framed(crate::world::doors::DOOR_CLOSED, 0, (dy as i16) * 18),
            );
        }
    }

    /// B-03, fail-then-pass: placing a chest wrote nothing server-side.
    ///
    /// A client never sends packet 79 or a tile square for a container - `Main.tileContainer[21]`
    /// (`Main.cs:10215`) keeps it out of `SendObjectPlacement` (`Player.cs:40461`), so
    /// `Chest.AfterPlacement_Hook`'s packet 34 (`Chest.cs:565-579`) is the only notification.
    /// Vanilla's server answers it with `WorldGen.PlaceChest`, which places the tiles *and*
    /// allocates the chest (`MessageBuffer.cs:1999-2015`).
    #[test]
    fn a_placed_chest_becomes_real_tiles_and_a_real_chest() {
        let mut server = GameServer::new(Config::default(), world());
        let _placer = connect(&mut server, 0);

        server.handle_packet(0, frame(id::CHEST_UPDATES, &chest_packet(0, 50, 50, 0)));

        // `TileObjectData` for tile 21 puts the cursor on the object's lower-left cell, so the
        // corner is one row up (`Chest.AfterPlacement_Hook`'s `OriginToTopLeft`).
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let tile = server.world.tile(50 + dx, 49 + dy);
            assert!(tile.is_active(), "the chest's tiles have to be written");
            assert_eq!(tile.block, CHEST_BLOCK);
        }
        assert!(
            server.world.chest_at(50, 49).is_some(),
            "and the chest record has to exist, or anything put in it is lost"
        );
    }

    /// B-03's other half: the relay carried the client's zero, so every *receiver* rebuilt its own
    /// chest 0 on top of the new one (`WorldGen.PlaceChestDirect(..., 0)` into
    /// `Chest.CreateWorldChest(0, ...)`, `Chest.cs:583-600`). Vanilla broadcasts the id the server
    /// allocated instead (`MessageBuffer.cs:2014`).
    #[test]
    fn the_broadcast_carries_the_servers_own_chest_id() {
        let mut server = GameServer::new(Config::default(), world());
        // An existing chest, so the newly placed one cannot coincidentally be id 0.
        let existing = server
            .world
            .add_chest(crate::world::Chest::empty_at(10, 10))
            .expect("a chest slot");
        assert_eq!(existing, 0);
        let _placer = connect(&mut server, 0);
        let mut watcher = connect(&mut server, 1);

        server.handle_packet(0, frame(id::CHEST_UPDATES, &chest_packet(0, 50, 50, 0)));

        let seen = drain(&mut watcher);
        let sent = of(&seen, id::CHEST_UPDATES);
        assert_eq!(sent.len(), 1, "the placement is still announced");
        let id = i16::from_le_bytes([sent[0][7], sent[0][8]]);
        assert_eq!(id, 1, "the server's own id, not the client's zero");
        assert_eq!(
            server.world.chest(0).map(|c| (c.x, c.y)),
            Some((10, 10)),
            "and the world's first chest is untouched"
        );
    }

    /// B-03: a chest's style lives entirely in `frameX`, at `36 * style`
    /// (`WorldGen.PlaceChestDirect`, `WorldGen.cs:58339-58370`) - which is what `on_lock` reads
    /// back with `frame_x / 36` and what worldgen's own `add_chest_styled` writes. Placing a
    /// styled chest through `tile_object::frame_of` instead would have framed a Gold Chest at
    /// `frameX 0, frameY 72`, which is a plain wooden chest two rows down the sheet.
    #[test]
    fn a_styled_chest_is_framed_the_way_the_rest_of_this_server_reads_it() {
        let mut server = GameServer::new(Config::default(), world());
        let _placer = connect(&mut server, 0);

        server.handle_packet(0, frame(id::CHEST_UPDATES, &chest_packet(0, 50, 50, 1)));

        assert_eq!(server.world.tile(50, 49).frame_x, 36);
        assert_eq!(server.world.tile(51, 49).frame_x, 54);
        assert_eq!(server.world.tile(50, 50).frame_y, 18);
        assert_eq!(
            i32::from(server.world.tile(50, 49).frame_x) / 36,
            1,
            "and `on_lock` reads style 1 back out of it"
        );
    }

    /// A style the frame cannot hold writes a corrupt tile that goes into the save. Vanilla passes
    /// the wire's style straight to `WorldGen.PlaceChest` with no check; this is a disclosed
    /// narrowing at a trust boundary, not a transcription.
    #[test]
    fn a_container_style_that_does_not_fit_the_frame_is_refused() {
        let mut server = GameServer::new(Config::default(), world());
        let _placer = connect(&mut server, 0);

        for style in [-1i16, 30000] {
            server.handle_packet(0, frame(id::CHEST_UPDATES, &chest_packet(0, 50, 50, style)));
            assert!(
                !server.world.tile(50, 49).is_active(),
                "style {style} must not reach a real tile"
            );
            assert!(server.world.chest_at(50, 49).is_none());
        }
    }

    /// B-03: a dresser is a container too, and used to be storage-free. Vanilla's case 2 is
    /// `WorldGen.PlaceChest(x, y, 88, ...)` (`MessageBuffer.cs:2045`).
    #[test]
    fn a_dresser_gets_storage_as_well() {
        let mut server = GameServer::new(Config::default(), world());
        let _placer = connect(&mut server, 0);

        server.handle_packet(0, frame(id::CHEST_UPDATES, &chest_packet(2, 50, 50, 0)));

        // Three wide, cursor on the middle of the lower row: corner at (x - 1, y - 1).
        assert_eq!(server.world.tile(49, 49).block, DRESSER_BLOCK);
        assert_eq!(server.world.tile(51, 50).block, DRESSER_BLOCK);
        assert!(server.world.chest_at(49, 49).is_some(), "a dresser stores");
    }

    /// B-03's breaking half: packet 17 arrives with its fail flag set (`Player.cs:54419-54424`,
    /// `SendData(17, ..., 0, x, y, 1f)`), which is a hit effect and clears nothing, so the tiles
    /// were left standing on the server and the chest item was never given back. Vanilla clears
    /// them from case 34 itself (`MessageBuffer.cs:2032`, `WorldGen.KillTile`).
    #[test]
    fn breaking_a_chest_clears_its_tiles_and_gives_it_back() {
        let mut server = GameServer::new(Config::default(), world());
        let _breaker = connect(&mut server, 0);
        server.handle_packet(0, frame(id::CHEST_UPDATES, &chest_packet(0, 50, 50, 0)));
        assert!(server.world.chest_at(50, 49).is_some());

        // Break it by clicking its lower-right cell, which is not the anchor.
        server.handle_packet(0, frame(id::CHEST_UPDATES, &chest_packet(1, 51, 50, 0)));

        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert!(
                !server.world.tile(50 + dx, 49 + dy).is_active(),
                "every cell of a broken chest goes"
            );
        }
        assert!(server.world.chest_at(50, 49).is_none(), "and the record");
        assert_eq!(
            server.items.iter().count(),
            1,
            "a broken chest is an item again"
        );
    }

    /// M-01, fail-then-pass: `GEM_LOCK` named tile 442, which is
    /// `TileID.ProjectilePressurePad` (`TileID.cs:1321`). `TileID.GemLocks` is 440
    /// (`TileID.cs:1317`), so every real gem lock was rejected. The gem itself is
    /// `WorldGen.ToggleGemLock`'s own `Item.NewItem` on the way out (`WorldGen.cs:46925-46928`);
    /// without it unlocking destroyed the gem.
    #[test]
    fn a_gem_lock_is_tile_440_and_hands_the_gem_back() {
        let mut server = GameServer::new(Config::default(), world());
        let _player = connect(&mut server, 0);
        // A locked amethyst lock (style 0, so `frameX / 54 == 0`), three by three, lower band.
        for dx in 0..3i32 {
            for dy in 0..3i32 {
                server.world.set_tile(
                    100 + dx,
                    100 + dy,
                    Tile::framed(GEM_LOCK, (dx as i16) * 18, 54 + (dy as i16) * 18),
                );
            }
        }

        let mut payload = 100i16.to_le_bytes().to_vec();
        payload.extend_from_slice(&100i16.to_le_bytes());
        payload.push(0); // unlock
        server.handle_packet(0, frame(id::GEM_LOCK_TOGGLE, &payload));

        assert_eq!(
            server.world.tile(100, 100).frame_y,
            0,
            "an unlocked gem lock sits in the upper band"
        );
        assert_eq!(
            server.items.iter().next().map(|(_, i)| i.item.id),
            Some(1526),
            "and the amethyst comes back out"
        );
    }

    /// M-03, fail-then-pass: doors were cosmetic. The client sends packet 19 and nothing else
    /// (`Player.cs:33093-33098`), so the old comment's "until a client pushes a tile square over
    /// them" never happened and the server's door never moved. Vanilla applies it before relaying
    /// (`MessageBuffer.cs:1307-1327`).
    #[test]
    fn a_door_swings_in_the_servers_own_world() {
        let mut server = GameServer::new(Config::default(), world());
        let _player = connect(&mut server, 0);
        shut_door(&mut server, 100, 50);

        // Action 0, opening rightwards.
        server.handle_packet(0, frame(id::TOGGLE_DOOR_STATE, &[0, 100, 0, 50, 0, 1]));
        assert_eq!(
            server.world.tile(100, 50).block,
            crate::world::doors::DOOR_OPEN,
            "the door has to open here, not only on the wire"
        );

        server.handle_packet(0, frame(id::TOGGLE_DOOR_STATE, &[1, 100, 0, 50, 0, 1]));
        assert_eq!(
            server.world.tile(100, 50).block,
            crate::world::doors::DOOR_CLOSED,
            "and shut again"
        );
    }

    /// M-04, fail-then-pass: the Golden Key was consumed and the door stayed locked here, so it
    /// re-locked on the next section send and saved locked. `WorldGen.UnlockDoor`
    /// (`WorldGen.cs:37988-38017`) walks up to the row framed at 594 and adds 54 to three tiles;
    /// the client only ever sends packet 52 (`Player.cs:33064-33070`).
    #[test]
    fn a_dungeon_door_really_unlocks() {
        let mut server = GameServer::new(Config::default(), world());
        let _player = connect(&mut server, 0);
        for dy in 0..3i32 {
            server.world.set_tile(
                100,
                50 + dy,
                Tile::framed(crate::world::doors::DOOR_CLOSED, 0, 594 + (dy as i16) * 18),
            );
        }

        // Action 2 clicked on the middle row, which is not the row framed at 594.
        let mut payload = vec![2];
        payload.extend_from_slice(&100i16.to_le_bytes());
        payload.extend_from_slice(&51i16.to_le_bytes());
        server.handle_packet(0, frame(id::LOCK_AND_UNLOCK, &payload));

        for dy in 0..3i16 {
            assert_eq!(
                server.world.tile(100, 50 + i32::from(dy)).frame_y,
                648 + dy * 18,
                "every row of the door moves out of the locked band"
            );
        }
    }

    /// M-04's other half: the check was `block == DOOR_CLOSED` and nothing more, so a crafted
    /// action-2 packet aimed at an ordinary door was relayed as an unlock.
    ///
    /// `WorldGen.IsLockedDoor` (`WorldGen.cs:69725-69732`) is `frameY >= 594 && frameY <= 646 &&
    /// frameX < 54`.
    #[test]
    fn an_ordinary_door_cannot_be_unlocked() {
        let mut server = GameServer::new(Config::default(), world());
        let _player = connect(&mut server, 0);
        let mut watcher = connect(&mut server, 1);
        shut_door(&mut server, 100, 50);

        let mut payload = vec![2];
        payload.extend_from_slice(&100i16.to_le_bytes());
        payload.extend_from_slice(&50i16.to_le_bytes());
        server.handle_packet(0, frame(id::LOCK_AND_UNLOCK, &payload));

        assert_eq!(server.world.tile(100, 50).frame_y, 0, "nothing moved");
        assert!(
            of(&drain(&mut watcher), id::LOCK_AND_UNLOCK).is_empty(),
            "and nothing was relayed"
        );
    }

    /// M-06, fail-then-pass: packet 33's name field was decoded and dropped, and it is the only
    /// write path for a chest's name in the game (`MessageBuffer.cs:3162-3169`). Packet 69's own
    /// server branch (`:3082-3095`) is a read request, so nothing else could ever set one.
    #[test]
    fn a_chest_can_be_named() {
        let mut server = GameServer::new(Config::default(), world());
        let _namer = connect(&mut server, 0);
        let mut watcher = connect(&mut server, 1);
        server
            .world
            .add_chest(crate::world::Chest::empty_at(10, 10))
            .expect("a chest slot");
        if let Some(player) = server.player_mut(0) {
            player.open_chest = 0;
        }

        // Closing the chest and naming it in the same packet, which is how a client sends it.
        let mut payload = (-1i16).to_le_bytes().to_vec();
        payload.extend_from_slice(&0i16.to_le_bytes());
        payload.extend_from_slice(&0i16.to_le_bytes());
        payload.push(4); // the length marker vanilla tests against 20
        payload.push(4); // the string's own 7-bit length prefix
        payload.extend_from_slice(b"loot");
        server.handle_packet(0, frame(id::SYNC_PLAYER_CHEST, &payload));

        assert_eq!(
            server.world.chest(0).map(|c| c.name.as_str()),
            Some("loot"),
            "the name has to be kept, or it can never be set at all"
        );
        let seen = drain(&mut watcher);
        assert_eq!(
            of(&seen, id::CHEST_NAME).len(),
            1,
            "and broadcast, which packet 69 never was"
        );
        assert_eq!(
            of(&seen, id::SYNC_PLAYER_CHEST_INDEX).len(),
            1,
            "along with the close itself (`MessageBuffer.cs:3168`)"
        );
    }

    /// M-09, fail-then-pass: relaying packet 111 did nothing, because no client acts on a received
    /// one. Vanilla's whole case is `BirthdayParty.ToggleManualParty()` on the server
    /// (`MessageBuffer.cs:3832-3836`), so the Party Center button was dead.
    #[test]
    fn the_party_centre_starts_a_party() {
        let mut server = GameServer::new(Config::default(), world());
        let _player = connect(&mut server, 0);
        assert!(!server.party.is_up());

        server.handle_packet(0, frame(id::TOGGLE_PARTY, &[]));

        assert!(server.party.is_up(), "a click on a Party Center is a party");
    }

    /// M-09's other half: emotes were invisible. Vanilla never relays packet 120; it makes a
    /// bubble and broadcasts packet **91** (`MessageBuffer.cs:3855-3866` into
    /// `EmoteBubble.NewBubble`). A relayed 120 is read and dropped by every receiver, whose own
    /// case is `netMode == 2`-only.
    #[test]
    fn an_emote_goes_out_as_a_bubble_not_a_relay() {
        let mut server = GameServer::new(Config::default(), world());
        let _sender = connect(&mut server, 0);
        let mut watcher = connect(&mut server, 1);

        server.handle_packet(0, frame(id::EMOJI, &[0, 3]));

        let seen = drain(&mut watcher);
        assert!(of(&seen, id::EMOJI).is_empty(), "120 is never relayed");
        let bubbles = of(&seen, id::SYNC_EMOTE_BUBBLE);
        assert_eq!(bubbles.len(), 1, "91 is what a receiver understands");
        // i32 id, u8 anchor type, u16 anchor, u16 lifetime, u8 emote.
        assert_eq!(bubbles[0][4], 1, "anchored to a player");
        assert_eq!(u16::from_le_bytes([bubbles[0][5], bubbles[0][6]]), 0);
        assert_eq!(u16::from_le_bytes([bubbles[0][7], bubbles[0][8]]), 360);
        assert_eq!(bubbles[0][9], 3, "and it is the emote that was asked for");
    }

    /// M-09: `num260 < EmoteID.Count` (`MessageBuffer.cs:3859`), which this had no equivalent of.
    #[test]
    fn an_emote_out_of_range_makes_no_bubble() {
        let mut server = GameServer::new(Config::default(), world());
        let _sender = connect(&mut server, 0);
        let mut watcher = connect(&mut server, 1);

        server.handle_packet(0, frame(id::EMOJI, &[0, 200]));

        assert!(of(&drain(&mut watcher), id::SYNC_EMOTE_BUBBLE).is_empty());
    }
}

/// Vanilla's join loop sends a `21` and then a `22` for every live item
/// (`MessageBuffer.cs:843-850`, case 8). This sent only the `21`, so a joining client believed
/// every item on the ground was free to take: it would walk up to one already reserved for
/// somebody else, turn it to air locally on its way to a `151`, and have that `151` refused. The
/// item then stayed on the server and stayed invisible to that one client, with nothing scheduled
/// to correct it, because `tick_items` broadcasts a `22` only for an item it is *newly* reserving
/// and this one was already reserved.
///
/// Tested here rather than through `tests/gameplay.rs` because the headless client cannot see this:
/// `Client::handshake` interprets and discards everything between `SpawnTileData` and
/// `InitialSpawn`, which is the whole join stream. Its `joined_with` buffer only starts collecting
/// after that. `a_joining_player_is_told_about_npcs_already_alive` passes on the live NPC sync
/// broadcasts rather than on the join stream it names, for the same reason.
#[cfg(test)]
mod join_stream_item_owners {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "join stream probe")
    }

    #[test]
    fn every_item_in_the_join_stream_is_followed_by_who_it_belongs_to() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let (index, _) = server
            .items
            .spawn(ItemStack::new(9, 1, 0), (320.0, 480.0))
            .expect("a slot");
        server.items.get_mut(index).expect("the item").owner = 3;

        let (out_tx, mut out_rx) = mpsc::channel(64);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        // Anything below `TilesSent`: the join tail runs once and then advances past this.
        player.state = ConnState::WorldSent;
        server.players[0] = Some(player);

        server.finish_join_stream(0);

        let sent: Vec<Bytes> = std::iter::from_fn(|| out_rx.try_recv().ok()).collect();
        let ids: Vec<u8> = sent.iter().map(|f| f[2]).collect();
        assert_eq!(
            &ids[..2],
            &[id::SYNC_ITEM, id::ITEM_OWNER],
            "each item goes out as a 21 and then a 22, the way vanilla's own join loop pairs them"
        );

        let owner = ItemOwner::decode(&sent[1][3..]).expect("a decodable 22");
        assert_eq!((owner.index, owner.owner), (index, 3));
        assert_eq!(
            owner.keep_reservation_ticks, 0,
            "vanilla's join loop passes no `number2`, so this field goes out as zero"
        );
    }
}

/// Player luck, which this server had no model of at all until now. It is not client-side state:
/// packet 134 exists to tell the server, and `MessageBuffer.cs:4190-4220` stores every factor and
/// recomputes the total. This server relayed the packet and threw the contents away, which is why
/// every luck-sensitive roll in the workspace was written against zero.
#[cfg(test)]
mod player_luck {
    use super::*;
    use crate::config::Config;

    fn frame(id: u8, payload: &[u8]) -> Frame {
        Frame {
            id,
            payload: Bytes::copy_from_slice(payload),
        }
    }

    fn connect(server: &mut GameServer, slot: u8) -> mpsc::Receiver<Bytes> {
        let (tx, rx) = mpsc::channel(64);
        let mut player = Player::new(slot, "127.0.0.1:1".parse().expect("test address"), tx);
        player.greeted = true;
        player.password_ok = true;
        player.state = ConnState::Playing;
        server.players[usize::from(slot)] = Some(player);
        rx
    }

    /// Packet 134 as a client writes it (`NetMessage.cs:1565-1578`). Named rather than positional,
    /// because eight numbers in a row is exactly the shape a wrong-order bug hides in.
    #[derive(Default)]
    struct Factors {
        ladybug: i32,
        torch: f32,
        potion: u8,
        gnome: bool,
        mirror: bool,
        equipment: f32,
        coins: f32,
        kites: u8,
    }

    fn luck_packet(slot: u8, f: Factors) -> Vec<u8> {
        let mut body = vec![slot];
        body.extend_from_slice(&f.ladybug.to_le_bytes());
        body.extend_from_slice(&f.torch.to_le_bytes());
        body.push(f.potion);
        body.push(u8::from(f.gnome));
        body.push(u8::from(f.mirror));
        body.extend_from_slice(&f.equipment.to_le_bytes());
        body.extend_from_slice(&f.coins.to_le_bytes());
        body.push(f.kites);
        body
    }

    /// Packet 50: a slot byte and then `u16` buff ids, terminated by a zero
    /// (`MessageBuffer.cs:2470`).
    fn buff_packet(slot: u8, ids: &[u16]) -> Vec<u8> {
        let mut body = vec![slot];
        for id in ids {
            body.extend_from_slice(&id.to_le_bytes());
        }
        body.extend_from_slice(&0u16.to_le_bytes());
        body
    }

    #[test]
    fn packet_134_gives_the_server_a_real_luck_figure() {
        let mut server = GameServer::new(
            Config::default(),
            crate::world::World::empty(400, 300, "luck probe"),
        );
        let _rx = connect(&mut server, 0);
        assert_eq!(
            server.player(0).expect("seated").luck,
            0.0,
            "nobody has reported anything yet"
        );

        // Three Luck Potions and a Garden Gnome: 0.3 + 0.2.
        server.handle_packet(
            0,
            frame(
                id::UPDATE_PLAYER_LUCK_FACTORS,
                &luck_packet(
                    0,
                    Factors {
                        potion: 3,
                        gnome: true,
                        ..Factors::default()
                    },
                ),
            ),
        );
        let luck = server.player(0).expect("seated").luck;
        assert!(
            (luck - 0.5).abs() < 1e-5,
            "three potions and a gnome is 0.5: got {luck}"
        );
    }

    /// The slot byte is not trusted: `MessageBuffer.cs:4201-4204` overwrites it with `whoAmI` on a
    /// server specifically so a client cannot report luck for somebody else.
    #[test]
    fn a_client_cannot_report_luck_on_another_players_behalf() {
        let mut server = GameServer::new(
            Config::default(),
            crate::world::World::empty(400, 300, "luck spoof probe"),
        );
        let _rx0 = connect(&mut server, 0);
        let _rx1 = connect(&mut server, 1);

        // Slot 0 claims slot 1 is drinking three Luck Potions.
        server.handle_packet(
            0,
            frame(
                id::UPDATE_PLAYER_LUCK_FACTORS,
                &luck_packet(
                    1,
                    Factors {
                        potion: 3,
                        ..Factors::default()
                    },
                ),
            ),
        );
        assert_eq!(
            server.player(1).expect("seated").luck,
            0.0,
            "the victim's luck must be untouched"
        );
        assert!(
            server.player(0).expect("seated").luck > 0.0,
            "and the sender gets what they reported, which is the whole of the rewrite"
        );
    }

    /// A body the server cannot read is dropped rather than relayed. The old verbatim arm forwarded
    /// it: there is no reason to hand every other client bytes this server could not parse.
    #[test]
    fn a_malformed_body_is_not_relayed() {
        let mut server = GameServer::new(
            Config::default(),
            crate::world::World::empty(400, 300, "luck malformed probe"),
        );
        let _rx0 = connect(&mut server, 0);
        let mut rx1 = connect(&mut server, 1);

        server.handle_packet(0, frame(id::UPDATE_PLAYER_LUCK_FACTORS, &[0, 1, 2]));
        let mut seen = 0;
        while let Ok(f) = rx1.try_recv() {
            if f.get(2) == Some(&id::UPDATE_PLAYER_LUCK_FACTORS) {
                seen += 1;
            }
        }
        assert_eq!(seen, 0, "a truncated luck packet must not be forwarded");
        assert_eq!(server.player(0).expect("seated").luck, 0.0);
    }

    /// A well-formed one *is* relayed, exactly as `NetMessage.SendData(134, -1, num106, ...)` does:
    /// to everyone but the sender.
    #[test]
    fn a_well_formed_body_still_reaches_everybody_else() {
        let mut server = GameServer::new(
            Config::default(),
            crate::world::World::empty(400, 300, "luck relay probe"),
        );
        let mut rx0 = connect(&mut server, 0);
        let mut rx1 = connect(&mut server, 1);

        let body = luck_packet(
            0,
            Factors {
                potion: 1,
                ..Factors::default()
            },
        );
        server.handle_packet(0, frame(id::UPDATE_PLAYER_LUCK_FACTORS, &body));

        let count = |rx: &mut mpsc::Receiver<Bytes>| {
            let mut n = 0;
            while let Ok(f) = rx.try_recv() {
                if f.get(2) == Some(&id::UPDATE_PLAYER_LUCK_FACTORS) {
                    n += 1;
                }
            }
            n
        };
        assert_eq!(count(&mut rx1), 1, "everybody else hears about it");
        assert_eq!(count(&mut rx0), 0, "the sender already knows");
    }

    /// `stinky` is a term of `RecalculateLuck` and arrives on packet 50, not 134, so a buff list
    /// changing has to recompute the total on its own.
    #[test]
    fn a_stinky_player_is_less_lucky_and_the_buff_packet_says_so() {
        let mut server = GameServer::new(
            Config::default(),
            crate::world::World::empty(400, 300, "stinky probe"),
        );
        let _rx = connect(&mut server, 0);
        server.handle_packet(
            0,
            frame(
                id::UPDATE_PLAYER_LUCK_FACTORS,
                &luck_packet(
                    0,
                    Factors {
                        potion: 3,
                        ..Factors::default()
                    },
                ),
            ),
        );
        let clean = server.player(0).expect("seated").luck;
        assert!((clean - 0.3).abs() < 1e-5, "three potions: got {clean}");

        // Some other buff first, to prove the reader is not just finding any id at all.
        server.handle_packet(0, frame(id::PLAYER_BUFFS, &buff_packet(0, &[1, 2, 3])));
        assert!(
            (server.player(0).expect("seated").luck - clean).abs() < 1e-5,
            "an unrelated buff changes nothing"
        );

        server.handle_packet(
            0,
            frame(
                id::PLAYER_BUFFS,
                &buff_packet(0, &[1, super::super::systems::STINKY, 3]),
            ),
        );
        let stinky = server.player(0).expect("seated").luck;
        assert!(
            (stinky - (clean - 0.25)).abs() < 1e-5,
            "Stinky costs a quarter: got {stinky}"
        );

        // ...and it comes back when the buff runs out.
        server.handle_packet(0, frame(id::PLAYER_BUFFS, &buff_packet(0, &[])));
        assert!((server.player(0).expect("seated").luck - clean).abs() < 1e-5);
    }

    /// A lantern night is worth +0.3 to everybody, and it is entirely the server's own state - no
    /// packet reports it, so nothing but the server can fold it in.
    #[test]
    fn a_lantern_night_makes_everybody_luckier() {
        let mut server = GameServer::new(
            Config::default(),
            crate::world::World::empty(400, 300, "lantern luck probe"),
        );
        let _rx = connect(&mut server, 0);
        server.handle_packet(
            0,
            frame(
                id::UPDATE_PLAYER_LUCK_FACTORS,
                &luck_packet(0, Factors::default()),
            ),
        );
        assert_eq!(server.player(0).expect("seated").luck, 0.0);

        server.lantern_night.next_night_guaranteed = true;
        server.roll_natural_lantern_night();
        assert!(
            server.lantern_night.is_up(),
            "the guarantee is what makes this deterministic"
        );
        let up = server.player(0).expect("seated").luck;
        assert!((up - 0.3).abs() < 1e-5, "lanterns are worth 0.3: got {up}");

        // And the morning takes it away again.
        server.lantern_night.end_for_the_morning();
        server.refresh_all_luck();
        assert_eq!(server.player(0).expect("seated").luck, 0.0);
    }

    /// The two rolls this change wires: a lucky player has more stars aimed at them, and the
    /// luckiest player on the server makes money rain likelier for everybody.
    ///
    /// Measured over many rolls rather than asserted on one, because `RollLuck` is a chance of a
    /// better chance: a lucky player still sometimes draws the ordinary odds.
    #[test]
    fn luck_moves_both_of_the_rolls_that_read_it() {
        let money_rate = |luck: f32| {
            let mut server = GameServer::new(
                Config::default(),
                crate::world::World::empty(4200, 1200, "money luck probe"),
            );
            let _rx = connect(&mut server, 0);
            if let Some(p) = server.player_mut(0) {
                p.luck = luck;
            }
            let mut started = 0;
            for _ in 0..20_000 {
                server.weather.coin_rain = 0;
                server.roll_coin_rain();
                if server.weather.coin_rain > 0 {
                    started += 1;
                }
            }
            started
        };
        let unlucky = money_rate(0.0);
        let lucky = money_rate(1.0);
        assert!(
            lucky > unlucky,
            "a lucky server rains money more often: {lucky} against {unlucky}"
        );
        // At luck 1 the range is always narrowed to somewhere in [12, 25), so roughly a 1-in-18
        // average against 1-in-25: a real but not enormous shift, and the assertion is deliberately
        // loose because this is a distribution, not a formula.
        assert!(
            lucky < unlucky * 3,
            "and not absurdly more often: {lucky} against {unlucky}"
        );

        // Bad luck goes the other way, which is the branch that would be silently dead if
        // `roll_luck` had only been given its good half.
        let cursed = money_rate(-0.7);
        assert!(
            cursed < unlucky,
            "a cursed server rains money less often: {cursed} against {unlucky}"
        );
    }
}
