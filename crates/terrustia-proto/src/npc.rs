//! NPC networking: packets `23` (SyncNPC), `28` (DamageNPC) and `162` (the damage acknowledgement).
//!
//! Packet 23 is heavily bit-packed: which of the four AI slots are non-zero, whether the NPC is at
//! full health, and how many bytes its remaining health needs are all encoded in two flag bytes, so
//! a full-health NPC with no AI state costs far less than the worst case.

use crate::{
    error::Result,
    id,
    npc_data::{catchable, from_net_id, npc_stats, sync_anchor},
    reader::PacketReader,
    writer::PacketWriter,
};

/// Terraria keeps 200 NPC slots.
pub const MAX_NPCS: usize = 200;

/// The target value meaning "not chasing anybody".
pub const NO_TARGET: u16 = 255;

/// Number of AI slots each NPC carries.
pub const MAX_AI: usize = 4;

/// Packet `23`: an NPC's full state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyncNpc {
    pub index: u8,
    /// Bumped when a slot is reused, so a stale damage packet cannot hit the new occupant.
    pub generation: u8,
    /// Top-left of the NPC, before the type's sync anchor is applied.
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    pub target: u16,
    pub direction: i8,
    pub direction_y: i8,
    pub sprite_direction: i8,
    pub ai: [f32; MAX_AI],
    /// Negative ids identify variants of a base type.
    pub net_id: i16,
    pub life: i32,
    pub life_max: i32,
    /// Only present for catchable critters.
    pub release_owner: u8,
    /// `NPC.spawnNeedsSyncing`: this is the NPC's first sync since it was created.
    ///
    /// The server sets it in `NPC.NewNPC` (`NPC.cs:81609`) and clears it the first time the NPC is
    /// broadcast (`NetMessage.cs:1738`), so exactly one frame per spawn carries it. The client
    /// reads it as `flag17 = bitsByte31[3] || generation differs` and, when either holds, builds a
    /// **fresh** instance in the slot instead of reusing whatever is there
    /// (`MessageBuffer.cs:1610-1620`).
    ///
    /// Found by `conform` against a real `TerrariaServer`: it sent `0x08` in the second flags byte
    /// where this encoder sent `0x00`. It had never been modelled, and the comment listing the
    /// bits deliberately left clear did not mention it.
    pub spawn_needs_syncing: bool,
}

impl SyncNpc {
    /// The positive type this NPC really is, resolving a variant's negative net id through
    /// `NPCID.FromNetId`. Vanilla's own packet-23 writer indexes by exactly this
    /// (`NetMessage.cs:694`, `NetMessage.cs:762`).
    pub fn npc_type(&self) -> u16 {
        from_net_id(self.net_id)
    }

    /// The position as it travels on the wire: the top-left shifted by the type's sync anchor.
    pub fn anchored_position(&self) -> (f32, f32) {
        let (ax, ay) = sync_anchor(self.npc_type());
        if ax == 0.0 && ay == 0.0 {
            return self.position;
        }
        let size = npc_stats(self.npc_type())
            .map(|s| (s.width as f32, s.height as f32))
            .unwrap_or((0.0, 0.0));
        (self.position.0 + ax * size.0, self.position.1 + ay * size.1)
    }

    /// Undo [`Self::anchored_position`] when reading a packet back.
    fn unanchor(position: (f32, f32), net_id: i16) -> (f32, f32) {
        let npc_type = from_net_id(net_id);
        let (ax, ay) = sync_anchor(npc_type);
        if ax == 0.0 && ay == 0.0 {
            return position;
        }
        let size = npc_stats(npc_type)
            .map(|s| (s.width as f32, s.height as f32))
            .unwrap_or((0.0, 0.0));
        (position.0 - ax * size.0, position.1 - ay * size.1)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::SYNC_N_P_C);

        // The wire position is offset by the type's anchor times its size. Only King Slime has a
        // non-zero anchor: its sprite grows as it loses health, so the game syncs a fixed point on
        // it rather than the corner of a box that keeps changing size.
        let position = self.anchored_position();

        // Equality, not `>=`, because that is what the game writes. An overhealed NPC is the only
        // case that differs, and it should send its real life rather than claim to be at full.
        let at_full_health = self.life == self.life_max;
        let mut flags1 = 0u8;
        if self.direction > 0 {
            flags1 |= 0x01;
        }
        if self.direction_y > 0 {
            flags1 |= 0x02;
        }
        for (slot, value) in self.ai.iter().enumerate() {
            if *value != 0.0 {
                flags1 |= 0x04 << slot;
            }
        }
        if self.sprite_direction > 0 {
            flags1 |= 0x40;
        }
        if at_full_health {
            flags1 |= 0x80;
        }

        // Bits for scaled stats (0x01), statue spawns (0x02) and difficulty (0x04) are left clear:
        // this server does not scale NPCs per player. Bit 0x10 is
        // `spawnNeedsSyncing && shimmerTransparency > 0f` and stays clear with it, since nothing
        // here runs an NPC through shimmer.
        //
        // 0x08 is `spawnNeedsSyncing` (`NetMessage.cs:718`) and is *not* one of the deliberate
        // omissions: it was missing until `conform` caught a real server sending it where this
        // encoder sent nothing. See the field's own documentation for what the client does with it.
        let mut flags2 = 0u8;
        if self.spawn_needs_syncing {
            flags2 |= 0x08;
        }

        w.u8(self.index)
            .u8(self.generation)
            .vec2(position.0, position.1)
            .vec2(self.velocity.0, self.velocity.1)
            .u16(self.target)
            .u8(flags1)
            .u8(flags2);

        for value in &self.ai {
            if *value != 0.0 {
                w.f32(*value);
            }
        }
        w.i16(self.net_id);

        if !at_full_health {
            // The health field is sized to the maximum, not the current value.
            let bytes: u8 = if self.life_max > 32767 {
                4
            } else if self.life_max > 127 {
                2
            } else {
                1
            };
            w.u8(bytes);
            match bytes {
                4 => {
                    w.i32(self.life);
                }
                2 => {
                    w.i16(self.life as i16);
                }
                _ => {
                    w.i8(self.life as i8);
                }
            }
        }

        if catchable(self.npc_type()) {
            w.u8(self.release_owner);
        }
        w.finish()
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        let index = r.u8()?;
        let generation = r.u8()?;
        let position = r.vec2()?;
        let velocity = r.vec2()?;
        let target = r.u16()?;
        let flags1 = r.u8()?;
        let flags2 = r.u8()?;

        let mut ai = [0.0f32; MAX_AI];
        for (slot, value) in ai.iter_mut().enumerate() {
            if flags1 & (0x04 << slot) != 0 {
                *value = r.f32()?;
            }
        }
        let net_id = r.i16()?;

        // These two are skipped rather than modelled, but their bytes still have to be consumed.
        if flags2 & 0x01 != 0 {
            r.u8()?;
        }
        if flags2 & 0x04 != 0 {
            r.f32()?;
        }

        // At full health the packet carries no health at all, because the receiver already knows
        // the type's maximum. Resolving it here rather than leaving a sentinel means callers never
        // have to special-case it.
        let npc_type = from_net_id(net_id);
        let life_max = npc_stats(npc_type).map_or(1, |s| s.life_max);
        let at_full_health = flags1 & 0x80 != 0;
        let life = if at_full_health {
            life_max
        } else {
            match r.u8()? {
                4 => r.i32()?,
                2 => i32::from(r.i16()?),
                _ => i32::from(r.i8()?),
            }
        };

        let release_owner = if catchable(npc_type) { r.u8()? } else { 0 };

        Ok(Self {
            index,
            generation,
            position: Self::unanchor(position, net_id),
            velocity,
            target,
            direction: if flags1 & 0x01 != 0 { 1 } else { -1 },
            direction_y: if flags1 & 0x02 != 0 { 1 } else { -1 },
            sprite_direction: if flags1 & 0x40 != 0 { 1 } else { -1 },
            ai,
            net_id,
            life,
            life_max,
            release_owner,
            spawn_needs_syncing: flags2 & 0x08 != 0,
        })
    }
}

/// Packet `28`: a client reporting that it hit an NPC.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageNpc {
    pub index: u8,
    /// Must match the NPC's current generation, or the hit is stale and is dropped.
    pub generation: u8,
    pub damage: i16,
    pub knockback: f32,
    /// -1 or 1; the wire form is offset by one so it fits in a byte.
    pub direction: i8,
    pub crit: bool,
}

impl DamageNpc {
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(payload);
        Ok(Self {
            index: r.u8()?,
            generation: r.u8()?,
            damage: r.i16()?,
            knockback: r.f32()?,
            direction: (i16::from(r.u8()?) - 1) as i8,
            crit: r.u8()? == 1,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::DAMAGE_N_P_C);
        w.u8(self.index)
            .u8(self.generation)
            .i16(self.damage)
            .f32(self.knockback)
            .u8((i16::from(self.direction) + 1) as u8)
            .u8(u8::from(self.crit));
        w.finish()
    }
}

/// Packet `162`: tells a client its hit was received, so it can stop re-sending it.
pub fn damage_ack() -> Result<Vec<u8>> {
    PacketWriter::new(id::DAMAGE_N_P_C_ACK).finish()
}

/// How much health an NPC actually loses from a hit.
///
/// `Main.CalculateDamageNPCsTake`: defence removes half its value, the result never drops below 1,
/// and a critical doubles what is left.
pub fn damage_taken(damage: i32, defense: i32, crit: bool) -> i32 {
    let base = (f64::from(damage) - f64::from(defense) * 0.5).max(1.0);
    (base * if crit { 2.0 } else { 1.0 }) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SyncNpc {
        SyncNpc {
            index: 3,
            generation: 1,
            position: (1600.0, 320.0),
            velocity: (1.5, -0.5),
            target: NO_TARGET,
            direction: 1,
            direction_y: -1,
            sprite_direction: 1,
            ai: [0.0; MAX_AI],
            net_id: 1,
            life: 25,
            life_max: 25,
            release_owner: 255,
            spawn_needs_syncing: false,
        }
    }

    fn payload(frame: &[u8]) -> &[u8] {
        &frame[3..]
    }

    /// `spawnNeedsSyncing` rides bit 3 of the second flags byte, and survives a round trip.
    ///
    /// Found by `conform` against a real `TerrariaServer`, which sent `0x08` at payload offset 21
    /// where this encoder sent `0x00`. Vanilla writes it at `NetMessage.cs:718`; the client uses it
    /// to decide whether to build a fresh NPC instance in the slot (`MessageBuffer.cs:1610-1620`).
    ///
    /// Offset 21 is the second flags byte: index (1), generation (1), position (8), velocity (8),
    /// target (2), first flags byte (1) - 21 bytes before it.
    #[test]
    fn a_freshly_spawned_npc_says_so_in_the_second_flags_byte() {
        let mut npc = sample();

        npc.spawn_needs_syncing = false;
        let quiet = npc.encode().expect("a sample NPC encodes");
        assert_eq!(
            payload(&quiet)[21] & 0x08,
            0,
            "an NPC that has already been synced must not ask for a fresh instance"
        );

        npc.spawn_needs_syncing = true;
        let fresh = npc.encode().expect("a sample NPC encodes");
        assert_eq!(
            payload(&fresh)[21] & 0x08,
            0x08,
            "a newly spawned NPC sets bit 3, as `NetMessage.cs:718` does"
        );

        // The bit is the only difference between the two frames, so nothing else moved with it.
        assert_eq!(quiet.len(), fresh.len());
        let differing: Vec<usize> = quiet
            .iter()
            .zip(fresh.iter())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(at, _)| at)
            .collect();
        assert_eq!(differing, vec![3 + 21], "only the flags byte should change");

        // And it survives being read back, which is what `conform` compares.
        assert!(
            SyncNpc::decode(payload(&fresh))
                .expect("the frame decodes")
                .spawn_needs_syncing
        );
        assert!(
            !SyncNpc::decode(payload(&quiet))
                .expect("the frame decodes")
                .spawn_needs_syncing
        );
    }

    #[test]
    fn a_full_health_npc_omits_its_health() {
        let npc = sample();
        let frame = npc.encode().unwrap();
        assert_eq!(frame[2], id::SYNC_N_P_C);
        // 1 + 1 + 8 + 8 + 2 + 1 + 1 + 2, with no AI and no health.
        assert_eq!(payload(&frame).len(), 24);

        let decoded = SyncNpc::decode(payload(&frame)).unwrap();
        assert_eq!(decoded.index, 3);
        assert_eq!(decoded.net_id, 1);
        assert_eq!(decoded.direction, 1);
        assert_eq!(decoded.sprite_direction, 1);
    }

    #[test]
    fn a_full_health_npc_resolves_its_maximum_from_the_type() {
        // A zombie is 45 health, and the packet says only "full".
        let mut npc = sample();
        npc.net_id = 3;
        npc.life_max = 45;
        npc.life = 45;
        let decoded = SyncNpc::decode(payload(&npc.encode().unwrap())).unwrap();
        assert_eq!(decoded.life, 45);
        assert_eq!(decoded.life_max, 45);
    }

    #[test]
    fn a_wounded_npc_sizes_its_health_field_to_its_maximum() {
        // A 25 HP slime fits in one byte; a 2800 HP boss needs two.
        let mut slime = sample();
        slime.life = 10;
        assert_eq!(payload(&slime.encode().unwrap()).len(), 24 + 1 + 1);

        let mut boss = sample();
        boss.net_id = 4;
        boss.life_max = 2800;
        boss.life = 1000;
        assert_eq!(payload(&boss.encode().unwrap()).len(), 24 + 1 + 2);

        let decoded = SyncNpc::decode(payload(&boss.encode().unwrap())).unwrap();
        assert_eq!(decoded.life, 1000);
        assert_eq!(
            decoded.life_max, 2800,
            "resolved from the type, not the packet"
        );
    }

    #[test]
    fn only_non_zero_ai_slots_are_sent() {
        let mut npc = sample();
        npc.ai = [0.0, 2.5, 0.0, -1.0];
        let frame = npc.encode().unwrap();
        // Two floats added.
        assert_eq!(payload(&frame).len(), 24 + 8);

        let decoded = SyncNpc::decode(payload(&frame)).unwrap();
        assert_eq!(decoded.ai, [0.0, 2.5, 0.0, -1.0]);
    }

    #[test]
    fn a_catchable_critter_carries_a_release_owner() {
        let mut bunny = sample();
        bunny.net_id = 46; // Bunny
        bunny.release_owner = 2;
        let frame = bunny.encode().unwrap();
        assert_eq!(payload(&frame).len(), 24 + 1);
        assert_eq!(SyncNpc::decode(payload(&frame)).unwrap().release_owner, 2);
    }

    #[test]
    fn a_variant_resolves_to_its_base_type_rather_than_clamping_to_none() {
        // The coloured slimes are not types of their own: they are variants of the Blue Slime
        // riding on a negative net id (`NPCID.cs:11055-11061`), which `NPCID.FromNetId` resolves
        // through `NetIdMap` (`NPCID.cs:10451-10459`, `NPCID.cs:12478-12484`). Clamping the net id
        // to zero instead answers `NPCID.None` for the entire family, and everything keyed off the
        // type - the stats, the name, the catchable byte, the sync anchor - goes with it.
        let mut green = sample();
        green.net_id = -3; // NPCID.GreenSlime
        let decoded = SyncNpc::decode(payload(&green.encode().unwrap())).unwrap();
        assert_eq!(decoded.net_id, -3, "the variant itself survives the wire");
        assert_eq!(decoded.npc_type(), 1, "and names the Blue Slime behind it");
        assert_eq!(
            npc_stats(decoded.npc_type()).map(|s| s.name),
            Some("BlueSlime")
        );
        assert_eq!(
            decoded.life_max, 25,
            "so a full-health packet resolves the base type's maximum, not the unknown-type 1"
        );

        // The hornets sit at the far end of the map, one past which there is no base type at all.
        let mut hornet = sample();
        hornet.net_id = -65; // NPCID.BigHornetStingy
        assert_eq!(
            SyncNpc::decode(payload(&hornet.encode().unwrap()))
                .unwrap()
                .npc_type(),
            235
        );
        let mut past_the_end = sample();
        past_the_end.net_id = -66; // NPCID.NegativeIDCount, which is not itself an id
        assert_eq!(
            SyncNpc::decode(payload(&past_the_end.encode().unwrap()))
                .unwrap()
                .npc_type(),
            0,
            "an id past the map answers None rather than panicking on a socket's bytes"
        );
    }

    #[test]
    fn directions_round_trip_as_signs() {
        let mut npc = sample();
        npc.direction = -1;
        npc.direction_y = 1;
        npc.sprite_direction = -1;
        let decoded = SyncNpc::decode(payload(&npc.encode().unwrap())).unwrap();
        assert_eq!(
            (
                decoded.direction,
                decoded.direction_y,
                decoded.sprite_direction
            ),
            (-1, 1, -1)
        );
    }

    #[test]
    fn king_slimes_position_survives_the_sync_anchor() {
        // King Slime is the one type with a non-zero anchor, so its position is shifted on the
        // wire and must come back unchanged.
        let mut king = sample();
        king.net_id = 50;
        king.life_max = 2000;
        king.life = 2000;

        let stats = crate::npc_data::npc_stats(50).unwrap();
        let anchored = king.anchored_position();
        assert_eq!(anchored.0, king.position.0 + 0.5 * stats.width as f32);
        assert_eq!(anchored.1, king.position.1 + stats.height as f32);

        let decoded = SyncNpc::decode(payload(&king.encode().unwrap())).unwrap();
        assert_eq!(decoded.position, king.position);
    }

    #[test]
    fn an_ordinary_npc_is_not_shifted() {
        let npc = sample();
        assert_eq!(npc.anchored_position(), npc.position);
        assert_eq!(
            SyncNpc::decode(payload(&npc.encode().unwrap()))
                .unwrap()
                .position,
            npc.position
        );
    }

    #[test]
    fn damage_packet_round_trips_including_direction() {
        for direction in [-1i8, 1] {
            let hit = DamageNpc {
                index: 5,
                generation: 2,
                damage: 37,
                knockback: 4.5,
                direction,
                crit: true,
            };
            let frame = hit.encode().unwrap();
            assert_eq!(payload(&frame).len(), 10);
            assert_eq!(DamageNpc::decode(payload(&frame)).unwrap(), hit);
        }
    }

    #[test]
    fn damage_is_reduced_by_half_of_defence() {
        // A 20-damage hit on 6 defence lands for 17.
        assert_eq!(damage_taken(20, 6, false), 17);
        // Criticals double what is left.
        assert_eq!(damage_taken(20, 6, true), 34);
        // And a hit never does nothing at all.
        assert_eq!(damage_taken(1, 100, false), 1);
    }

    #[test]
    fn truncated_npc_packets_error_rather_than_panic() {
        assert!(SyncNpc::decode(&[0, 0, 0]).is_err());
        assert!(DamageNpc::decode(&[1]).is_err());
    }
}
