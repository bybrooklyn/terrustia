//! Projectile networking: packets `27` (SyncProjectile) and `29` (KillProjectile).
//!
//! The identity of a projectile is not a slot number but a packed key: eight bits of owner, ten of
//! index and fourteen of generation, all in one `i32`. That generation counter is what stops a late
//! kill packet from destroying whatever has since taken the slot.
//!
//! Packet 27 is bit-packed in the same spirit as the NPC sync: two flag bytes say which of the AI
//! slots and which of the damage fields are non-zero, so an ordinary projectile with a single AI
//! value and no knockback costs a fraction of the worst case.

use crate::{error::Result, id, reader::PacketReader, writer::PacketWriter};

/// Terraria keeps a thousand projectile slots.
pub const MAX_PROJECTILES: usize = 1000;

/// AI slots a projectile carries. Only three are sent.
pub const MAX_AI: usize = 3;

/// The owner a server-spawned projectile carries: nobody.
pub const SERVER_OWNER: u8 = 255;

/// A projectile's identity on the wire.
///
/// The generation is the important part. Slots are reused constantly, so a kill packet that
/// arrives a moment late would otherwise destroy an innocent bystander; carrying the generation
/// means such a packet simply fails to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectileKey {
    pub owner: u8,
    pub index: u16,
    pub generation: u16,
}

impl ProjectileKey {
    /// Pack into the `i32` the wire carries.
    pub const fn pack(self) -> i32 {
        ((self.owner as u32)
            | ((self.index as u32 & 0x3FF) << 8)
            | ((self.generation as u32 & 0x3FFF) << 18)) as i32
    }

    /// Unpack from the wire.
    pub const fn unpack(bits: i32) -> Self {
        let bits = bits as u32;
        Self {
            owner: (bits & 0xFF) as u8,
            index: ((bits >> 8) & 0x3FF) as u16,
            generation: ((bits >> 18) & 0x3FFF) as u16,
        }
    }
}

/// Packet `27`: a projectile's full state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyncProjectile {
    pub key: ProjectileKey,
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    pub projectile_type: i16,
    pub ai: [f32; MAX_AI],
    pub banner: u16,
    pub damage: i16,
    pub knockback: f32,
    pub original_damage: i16,
}

impl SyncProjectile {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::SYNC_PROJECTILE);
        w.i32(self.key.pack());
        w.f32(self.position.0);
        w.f32(self.position.1);
        w.f32(self.velocity.0);
        w.f32(self.velocity.1);
        w.i16(self.projectile_type);

        // The second flag byte only exists when something in it is set, and the first byte's bit 2
        // is what says so.
        let mut extra = 0u8;
        if self.ai[2] != 0.0 {
            extra |= 1 << 0;
        }
        let mut flags = 0u8;
        if self.ai[0] != 0.0 {
            flags |= 1 << 0;
        }
        if self.ai[1] != 0.0 {
            flags |= 1 << 1;
        }
        if extra != 0 {
            flags |= 1 << 2;
        }
        if self.banner != 0 {
            flags |= 1 << 3;
        }
        if self.damage != 0 {
            flags |= 1 << 4;
        }
        if self.knockback != 0.0 {
            flags |= 1 << 5;
        }
        if self.original_damage != 0 {
            flags |= 1 << 6;
        }
        w.u8(flags);
        if extra != 0 {
            w.u8(extra);
        }
        if flags & (1 << 0) != 0 {
            w.f32(self.ai[0]);
        }
        if flags & (1 << 1) != 0 {
            w.f32(self.ai[1]);
        }
        if flags & (1 << 3) != 0 {
            w.u16(self.banner);
        }
        if flags & (1 << 4) != 0 {
            w.i16(self.damage);
        }
        if flags & (1 << 5) != 0 {
            w.f32(self.knockback);
        }
        if flags & (1 << 6) != 0 {
            w.i16(self.original_damage);
        }
        if extra & (1 << 0) != 0 {
            w.f32(self.ai[2]);
        }
        w.finish()
    }

    pub fn decode(body: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(body);
        let key = ProjectileKey::unpack(r.i32()?);
        let position = (r.f32()?, r.f32()?);
        let velocity = (r.f32()?, r.f32()?);
        let projectile_type = r.i16()?;
        let flags = r.u8()?;
        let extra = if flags & (1 << 2) != 0 { r.u8()? } else { 0 };
        let mut ai = [0.0; MAX_AI];
        if flags & (1 << 0) != 0 {
            ai[0] = r.f32()?;
        }
        if flags & (1 << 1) != 0 {
            ai[1] = r.f32()?;
        }
        let banner = if flags & (1 << 3) != 0 { r.u16()? } else { 0 };
        let damage = if flags & (1 << 4) != 0 { r.i16()? } else { 0 };
        let knockback = if flags & (1 << 5) != 0 { r.f32()? } else { 0.0 };
        let original_damage = if flags & (1 << 6) != 0 { r.i16()? } else { 0 };
        if extra & (1 << 0) != 0 {
            ai[2] = r.f32()?;
        }
        Ok(Self {
            key,
            position,
            velocity,
            projectile_type,
            ai,
            banner,
            damage,
            knockback,
            original_damage,
        })
    }
}

/// Packet `29`: a projectile is gone, and where it was when it went.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KillProjectile {
    pub key: ProjectileKey,
    pub position: (f32, f32),
}

impl KillProjectile {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut w = PacketWriter::new(id::KILL_PROJECTILE);
        w.i32(self.key.pack());
        w.f32(self.position.0);
        w.f32(self.position.1);
        w.finish()
    }

    pub fn decode(body: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(body);
        Ok(Self {
            key: ProjectileKey::unpack(r.i32()?),
            position: (r.f32()?, r.f32()?),
        })
    }
}

/// Projectile type ids, in vanilla's `ProjectileID` numbering.
///
/// These sit here rather than beside the NPC-type ids in [`crate::npc_params`] because both spaces
/// are `u16` and both are dense over the same range, so a bare name cannot say which space it
/// belongs to. Twelve numbers are claimed twice across the two: 574 is a nebula floater here and a
/// Kobold Flyer there, 438 a scutlix rider's shot here and a Cultist Devote there, 264 an angry
/// nimbus's rain here and a Plantera tentacle there. That class of mix-up has already cost this
/// project twice (see `npc_params`'s Mothron and Duke Fishron notes), and the import path is the
/// only thing that catches it: an id destined for a `Shot.projectile` belongs in this module, an id
/// destined for an NPC type belongs in `npc_params`, and neither ever moves the other way.
pub mod ids {
    /// The antlion's sand ball. Its speed, damage and reload are `npc_params::ANTLION_*`.
    pub const ANTLION_SHOT_TYPE: u16 = 31;

    /// `ProjectileID.MoonLeech`, the Moon Lord's brand.
    ///
    /// Not a weapon: it does no damage (`NewProjectile(..., 456, 0, 0f, ...)`,
    /// `ItemDropDatabase`'s neighbour at `NPC.cs:42737`), and its whole job is to fly to one
    /// player, put `BuffID.MoonLeech` on them, and fly home. `aiStyle 85` is that round trip, and
    /// the boss reads the surviving brands three times a step to decide how many leeches to make.
    pub const MOON_LEECH_BRAND: u16 = 456;

    /// A night's falling star, in its two halves.
    ///
    /// `FALLING_STAR_SPAWNER` is what `WorldGen.SpawnFallingObjects` puts in the sky: it does not
    /// collide with anything, streaks for 180 ticks, and then replaces itself with
    /// `FALLING_STAR`, which does collide and drops a Fallen Star where it lands
    /// (`Projectile.cs:54028-54061` and `:79348`).
    pub const FALLING_STAR: u16 = 12;
    pub const FALLING_STAR_SPAWNER: u16 = 720;

    /// The giant cursed skull's shot.
    pub const GIANT_SKULL_SHOT_TYPE: u16 = 299;

    /// Skeletron's skull barrage, thrown while it hovers (`NPC.cs:22059-22114`).
    pub const SKELETRON_BARRAGE: u16 = 270;

    /// The stinger Queen Bee spits.
    pub const STINGER: u16 = 719;

    /// Deerclops' three: the ice spike, the falling rubble, and the shadow hand.
    pub const DEER_SPIKE: u16 = 961;
    pub const DEER_RUBBLE: u16 = 962;
    pub const DEER_SHADOW_HAND: u16 = 965;

    /// The Wall of Flesh eye's laser.
    pub const WALL_LASER: u16 = 83;

    /// The big stardust jellyfish's shot.
    pub const JELLYFISH_SHOT: u16 = 539;

    /// The missile an elf copter drops.
    pub const COPTER_SHOT: u16 = 180;

    /// What an angry nimbus rains.
    pub const NIMBUS_SHOT: u16 = 264;

    /// The four shots an ancient doom lets go.
    pub const DOOM_SHOT: u16 = 593;

    /// What a style-73 caster turret throws.
    pub const CASTER_SHOT: u16 = 435;

    /// The web a wall crawler spits in expert.
    pub const CRAWLER_SPIT: u16 = 472;

    /// The sandnado a sand elemental raises.
    /// The Sand Elemental raises a **mark** (658, `SandnadoHostileMark`), and the mark is what
    /// spawns the tornado (657, `SandnadoHostile`) sixty ticks later (`Projectile.cs:36848-36855`).
    /// Only the mark used to have a name here, and it was called `SANDNADO`, so the elemental's
    /// whole attack was a marker that spun in place and never became anything.
    pub const SANDNADO_MARK: u16 = 658;
    pub const SANDNADO: u16 = 657;

    /// A scutlix rider's shot.
    pub const RIDER_SHOT: u16 = 438;

    /// A Dutchman cannon's shot: a slow lobbed ball.
    pub const CANNON_SHOT: u16 = 240;

    /// The floaters a nebula brain puts out (vanilla hurries these on a teleport,
    /// `NPC.cs:39982-40002`).
    pub const NEBULA_FLOATER: u16 = 574;

    /// The Dreadnautilus's spray bolt, and the portal its helpers come out of. The helper itself is
    /// an NPC, `npc_params::NAUTILUS_HELPER`.
    pub const NAUTILUS_SPRAY_SHOT: u16 = 814;
    pub const NAUTILUS_HELPER_PORTAL: u16 = 813;

    /// The laser every Destroyer body segment's probe fires.
    pub const DESTROYER_LASER: u16 = 100;

    /// The Golem head's fireball, and the eye-laser it only grows past half health
    /// (`NPC.cs:31504-31564` attached, `:31736-31801` free).
    pub const GOLEM_FIREBALL: u16 = 258;
    pub const GOLEM_LASER: u16 = 259;

    /// Plantera's first form: the seed, and the thorn ball and spiky seed it mixes in below eighty
    /// per cent.
    pub const PLANTERA_SEED: u16 = 275;
    pub const PLANTERA_THORN_BALL: u16 = 276;
    pub const PLANTERA_SPIKY: u16 = 277;

    /// Duke Fishron's bubble.
    pub const FISHRON_BUBBLE: u16 = 385;

    /// The spheres Pumpking throws while hovering: one of three, picked at random over
    /// `npc_params::PUMPKING_SPHERE_SPAN`.
    pub const PUMPKING_SPHERE: u16 = 326;

    /// The Ice Queen's mode-0 forward mist (`NPC.cs:33751-33796`) and mode-1 ice shard
    /// (`NPC.cs:33811-33919`).
    pub const ICE_QUEEN_MIST: u16 = 348;
    pub const ICE_QUEEN_SHARD: u16 = 349;

    /// The Santa-NK1's four weapons: the machine gun (`NPC.cs:34048-34065`), the present bomb
    /// (`:34096-34107`), the rocket volley (`:34112-34135`) and the missile volley
    /// (`:34141-34158`).
    pub const SANTA_BULLET: u16 = 180;
    pub const SANTA_ROCKET: u16 = 350;
    pub const SANTA_MISSILE: u16 = 351;
    pub const SANTA_PRESENT: u16 = 352;

    /// Queen Slime's dive burst (`NPC.cs:46024-46118`) and the ring her swoop fires
    /// (`NPC.cs:46159-46236`).
    pub const QUEEN_SLIME_DIVE_SHOT: u16 = 922;
    pub const QUEEN_SLIME_RING_SHOT: u16 = 926;

    /// The Lunatic Cultist's three spells: `ProjectileID.CultistBossIceMist`
    /// (`NPC.cs:65569-65639`), `CultistBossFireBall` (`NPC.cs:65640-65719`) and
    /// `CultistBossLightningOrb` (`NPC.cs:65720-65779`).
    pub const CULTIST_ICE: u16 = 464;
    pub const CULTIST_LIGHTNING: u16 = 465;
    pub const CULTIST_FIRE: u16 = 467;

    /// A shard off the shattering cultist tablet.
    pub const TABLET_SHARD: u16 = 526;

    /// What the Moon Lord's parts throw, from the `NewProjectile` calls in `AI_078`/`AI_079`: the
    /// eye stream (`NPC.cs:42155`), the sphere barrage (`NPC.cs:42199`), the head's deathray
    /// (`NPC.cs:42667`) and the bolt spread (`NPC.cs:42502`). The names match `ProjectileID.cs`:
    /// 452 is the eye, 454 the sphere, 462 the bolt. Damage for each is
    /// `npc_params::PHANTASMAL_*_DAMAGE`.
    pub const PHANTASMAL_EYE: u16 = 452;
    pub const PHANTASMAL_SPHERE: u16 = 454;
    pub const PHANTASMAL_DEATHRAY: u16 = 455;
    pub const PHANTASMAL_BOLT: u16 = 462;

    /// The Old One's Army lightning bug's bolt.
    pub const LIGHTNING_BUG_BOLT: u16 = 682;

    /// The Martian saucer's three: the deathray that opens a strafe, the missiles it sprays while
    /// hovering, and the lasers it aims through the low hold.
    pub const SAUCER_DEATHRAY: u16 = 447;
    pub const SAUCER_MISSILE: u16 = 448;
    pub const SAUCER_LASER: u16 = 449;

    /// The Dark Mage's aimed bolt, the healing sigil it plants on the ground, and the portal its
    /// skeletons come out of.
    pub const DARK_MAGE_PORTAL: u16 = 673;
    pub const DARK_MAGE_HEAL: u16 = 674;
    pub const DARK_MAGE_BOLT: u16 = 675;

    /// Betsy's two.
    pub const BETSY_FIREBALL: u16 = 686;
    pub const BETSY_FLAME_BREATH: u16 = 687;

    /// The projectiles the Old One's Army troops throw.
    pub const OGRE_SPIT: u16 = 676;
    pub const OGRE_POUND: u16 = 683;
    pub const DRAKIN_FIREBALL: u16 = 671;
    pub const JAVELIN: u16 = 662;
    pub const JAVELIN_T3: u16 = 685;
    pub const GOBLIN_BOMB: u16 = 681;
    pub const GOBLIN_SHARK_SHOT: u16 = 811;

    /// The Empress of Light's five, named as `ProjectileID.cs` names them.
    ///
    /// Two of these used to carry invented names that pointed at the wrong attack: 874 was
    /// `EMPRESS_SUN_DANCE` when it is the death aurora she plants on arrival and over her target
    /// (`ProjectileID.cs:2200`), and 923 was `EMPRESS_ETHEREAL_LANCE` when it is the sun dance
    /// itself (`:2298`). Every *use* of both was against the right id, checked site by site against
    /// `NPC.cs:46528`, `:46833` and `:47022`, so nothing behaved wrongly - but this is the shape
    /// that had Mothron laying Crimson Penguins when two ids sat one apart under the wrong names,
    /// and a name that lies about which attack it is only has to be believed once.
    pub const EMPRESS_RAINBOW: u16 = 872;
    pub const EMPRESS_BLAST: u16 = 873;
    pub const EMPRESS_DEATH_AURORA: u16 = 874;
    pub const EMPRESS_LANCE: u16 = 919;
    pub const EMPRESS_SUN_DANCE: u16 = 923;

    /// `ProjectileID.DryadsWardCircle`, the only thing the Dryad's own attack puts in the air.
    ///
    /// It is not a weapon and never touches what it overlaps: `AI_111_DryadsWard`
    /// (`Projectile.cs:41872-41978`) grows a radius around itself and, every ten ticks, blesses
    /// the town NPCs inside it and puts Dryad's Bane on the hostiles. Vanilla launches it with no
    /// velocity at all (`NPC.cs:55463-55469` sets no speed for `type == 20`), so it hangs where it
    /// was cast for its whole life.
    pub const DRYADS_WARD: u16 = 586;

    /// `ProjectileID.MechanicWrench`, which is a boomerang and not a thrown tool.
    ///
    /// `aiStyle 109` (`Projectile.cs:34652-34690`) flies it out for thirty ticks and then turns it
    /// round and brings it home to the Mechanic, whose index it carries in `ai[1]`
    /// (`NPC.cs:55068`). It dies when it reaches her, or on a wall, which bounces it straight back
    /// and starts the return early.
    pub const MECHANIC_WRENCH: u16 = 582;

    /// `ProjectileID.TruffleSpore`, which does not travel: `aiStyle 112`'s `type == 590` body
    /// (`Projectile.cs:34822-34865`) overwrites its velocity every tick with a pure vertical bob.
    pub const TRUFFLE_SPORE: u16 = 590;

    /// `ProjectileID.PrincessWeapon`, whose `aiStyle 186` (`Projectile.cs:43454-43462`) is sixty
    /// ticks of drawing and then `Kill`. Its table says 180.
    pub const PRINCESS_WEAPON: u16 = 950;

    /// `ProjectileID.DandelionSeed`, the third of `aiStyle 112`'s three unrelated bodies and the
    /// only one that steers (`Projectile.cs:34743-34820`). It reads the player in `ai[1]` and
    /// whether it chases them at all depends on which way the wind is blowing, which is why
    /// standing upwind of a dandelion works.
    pub const DANDELION_SEED: u16 = 836;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SyncProjectile {
        SyncProjectile {
            key: ProjectileKey {
                owner: 255,
                index: 37,
                generation: 9,
            },
            position: (1234.5, -678.25),
            velocity: (6.0, -2.5),
            projectile_type: 38,
            ai: [0.0, 0.0, 0.0],
            banner: 0,
            damage: 15,
            knockback: 0.0,
            original_damage: 0,
        }
    }

    #[test]
    fn a_key_survives_the_round_trip() {
        let key = ProjectileKey {
            owner: 255,
            index: 1000,
            generation: 16_383,
        };
        assert_eq!(ProjectileKey::unpack(key.pack()), key);
    }

    #[test]
    fn the_key_packs_into_the_fields_the_game_reads() {
        let key = ProjectileKey {
            owner: 7,
            index: 300,
            generation: 42,
        };
        let bits = key.pack() as u32;
        assert_eq!(bits & 0xFF, 7);
        assert_eq!((bits >> 8) & 0x3FF, 300);
        assert_eq!((bits >> 18) & 0x3FFF, 42);
    }

    #[test]
    fn a_plain_projectile_round_trips() {
        let p = sample();
        let bytes = p.encode().unwrap();
        assert_eq!(SyncProjectile::decode(&bytes[3..]).unwrap(), p);
    }

    #[test]
    fn every_optional_field_round_trips() {
        let p = SyncProjectile {
            ai: [1.5, -2.5, 3.5],
            banner: 12,
            knockback: 4.25,
            original_damage: 21,
            ..sample()
        };
        let bytes = p.encode().unwrap();
        assert_eq!(SyncProjectile::decode(&bytes[3..]).unwrap(), p);
    }

    /// The packing is the point: a bare projectile costs far less than a fully loaded one.
    #[test]
    fn an_empty_projectile_is_much_smaller_than_a_full_one() {
        let bare = SyncProjectile {
            damage: 0,
            ..sample()
        }
        .encode()
        .unwrap();
        let full = SyncProjectile {
            ai: [1.0, 2.0, 3.0],
            banner: 5,
            knockback: 1.0,
            original_damage: 9,
            ..sample()
        }
        .encode()
        .unwrap();
        assert!(
            full.len() >= bare.len() + 15,
            "bare {} against full {}",
            bare.len(),
            full.len()
        );
    }

    #[test]
    fn the_third_ai_slot_costs_a_whole_extra_flag_byte() {
        let without = SyncProjectile {
            ai: [1.0, 1.0, 0.0],
            ..sample()
        }
        .encode()
        .unwrap();
        let with = SyncProjectile {
            ai: [1.0, 1.0, 1.0],
            ..sample()
        }
        .encode()
        .unwrap();
        // One byte for the second flag byte, four for the value itself.
        assert_eq!(with.len(), without.len() + 5);
    }

    #[test]
    fn a_kill_round_trips() {
        let k = KillProjectile {
            key: ProjectileKey {
                owner: 255,
                index: 3,
                generation: 1,
            },
            position: (100.0, 200.0),
        };
        let bytes = k.encode().unwrap();
        assert_eq!(KillProjectile::decode(&bytes[3..]).unwrap(), k);
    }
}
