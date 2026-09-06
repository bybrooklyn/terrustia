//! The Lunatic Cultist: style 84.
//!
//! Its attacks come off a fixed script rather than a die roll, and every other entry in that script
//! is "move" — which is why it spends so much of the fight drifting into a new position above you.
//! The sequence never varies, so the fight is memorisable, and that is deliberate.
//!
//! What makes it *this* fight is the ritual, and the ritual is a guess. Partway through the script
//! it stands two more copies of itself on a circle around it, up to six in all, and they move and
//! cast in lockstep. Hit the real one inside the window and the ritual ends there and some of the
//! group is destroyed with it: ten in classic, which is more than can ever be out, so the group is
//! cleared; three in expert, so once the group has grown some always survive. That is the expert
//! fight. Hit a copy and it dies and the real one is stunned for two seconds, which is the only
//! thing in the whole fight that punishes you for guessing.
//!
//! A clone is the same routine reading its owner's state rather than deciding anything of its own,
//! which is exactly why they are indistinguishable while the ritual runs.

use rand::rngs::SmallRng;
use terrustia_proto::npc_params::{
    CULTIST_ANCIENT_LIGHT, CULTIST_ARRIVAL, CULTIST_CLONE, CULTIST_CLONE_RING, CULTIST_CLONES_MAX,
    CULTIST_CLONES_PER_RITUAL, CULTIST_FIRE_COUNT, CULTIST_FIRE_COUNT_EXPERT, CULTIST_FIRE_DAMAGE,
    CULTIST_FIRE_EVERY, CULTIST_FIRE_EVERY_EXPERT, CULTIST_HALF_DEFENSE, CULTIST_ICE_DAMAGE,
    CULTIST_ICE_EVERY, CULTIST_ICE_EVERY_EXPERT, CULTIST_LIGHTNING_DAMAGE, CULTIST_LIGHTNING_EVERY,
    CULTIST_LIGHTNING_EVERY_EXPERT, CULTIST_MOVE_STEP, CULTIST_ORBIT, CULTIST_ORBIT_SPREAD,
    CULTIST_PAUSE, CULTIST_RIGHT_GUESS_CULL, CULTIST_RIGHT_GUESS_CULL_EXPERT, CULTIST_RITUAL_TICKS,
    CULTIST_RITUAL_WINDOW, CULTIST_SCRIPT_HEALTHY, CULTIST_SCRIPT_WOUNDED,
    CULTIST_SHADOWFLAME_ANGLE_STEP, CULTIST_SHADOWFLAME_COUNT, CULTIST_SHADOWFLAME_EVERY,
    CULTIST_SHADOWFLAME_EVERY_EXPERT, CULTIST_SHADOWFLAME_SPAWNS, CULTIST_SHADOWFLAME_SPEED,
    CULTIST_STUN_TICKS,
};
use terrustia_proto::projectile::ids::{CULTIST_FIRE, CULTIST_ICE, CULTIST_LIGHTNING};

use super::skeletron::Parent;
use crate::game::ai::{Shot, World};
use crate::game::npc::{Npc, TileView};
use crate::game::npc_ai::Spawn;

/// The states, as `ai[0]` numbers them.
mod state {
    pub const ARRIVING: f32 = -1.0;
    pub const DECIDING: f32 = 0.0;
    pub const MOVING: f32 = 1.0;
    pub const FIREBALLS: f32 = 2.0;
    pub const ICE: f32 = 3.0;
    pub const LIGHTNING: f32 = 4.0;
    pub const RITUAL: f32 = 5.0;
    /// Stunned: what a wrong guess buys you, and what it costs. Two seconds of standing there
    /// (`NPC.cs:65936-65948`).
    ///
    /// This numbering is `ai[0]`, which goes out on the wire, so it has to be vanilla's: a client
    /// poses the Cultist from it. Shadowflame used to sit here at 6, which is the stun's number,
    /// so every client watching a shadowflame cast drew the wrong animation.
    pub const STUNNED: f32 = 6.0;
    pub const SHADOWFLAME: f32 = 7.0;
}

/// What it did this tick.
#[derive(Debug, Default)]
pub struct CultistOutcome {
    pub shots: Vec<Shot>,
    pub spawn: Vec<Spawn>,
    /// Set when this one has outlived whatever it was a copy of.
    pub spent: bool,
    /// Where it wants to move to, worked out once at the start of a reposition.
    pub move_to: Option<(f32, f32)>,
    /// How many of its own clones a correct guess has just destroyed (`NPC.cs:65229-65256`).
    pub cull_clones: Option<usize>,
    /// Set by a decoy that has just been destroyed: its owner guessed at, and is stunned for it
    /// (`NPC.cs:65194-65207`).
    pub punish_owner: bool,
}

/// Style 84.
///
/// `owner` is `None` for the real cultist and the state of the real one for a clone. `clones` is
/// how many copies are already out.
pub fn cultist(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    owner: Option<Parent>,
    clones: usize,
    _rng: &mut SmallRng,
) -> CultistOutcome {
    let mut out = CultistOutcome::default();
    npc.dirty = true;
    let real = npc.npc_type != CULTIST_CLONE;
    let expert = world.conditions.expert;
    let health = npc.life as f32 / npc.life_max.max(1) as f32;
    let wounded = health <= 0.5;
    if wounded {
        npc.defense = (npc.stats.defense as f32 * CULTIST_HALF_DEFENSE) as i32;
    }

    // A clone has no mind of its own: it copies the real one's state exactly, which is what makes
    // the two indistinguishable.
    if !real {
        let Some(owner) = owner else {
            out.spent = true;
            return out;
        };
        // Both halves of the owner's state, which is what keeps the group in lockstep
        // (`NPC.cs:65190-65191`, `ai[0] = owner.ai[0]; ai[1] = owner.ai[1];`). `Parent::state` is
        // the parent's `ai[1]`, so reading the state out of it put the owner's *timer* into the
        // clone's state slot and had the group flailing between attacks as that timer counted up.
        npc.ai[0] = owner.phase;
        npc.ai[1] = owner.state;
        // A clone cannot be hurt into advancing the fight; it simply dies.
        if npc.ai[0] != state::RITUAL {
            npc.invulnerable = true;
        } else if world.was_hurt {
            // The wrong guess. The decoy dies, and the real one is stunned for two seconds
            // (`NPC.cs:65194-65207`): it is the only thing in the fight that punishes you, and it
            // was not wired at all - a decoy took the hit, shrugged it off, and the fight carried
            // on as though nothing had happened.
            out.spent = true;
            out.punish_owner = true;
            return out;
        }
    }

    // The right guess: hitting the real one inside the ritual window ends the ritual early, counts
    // as an attack, and destroys some of the group (`NPC.cs:65215-65262`). Ten in classic, which is
    // more than can ever be out, so the whole group goes; three in expert, so against a grown group
    // some always survive, which is the expert fight. Nothing consumed `is_a_guess` before this, so
    // the ritual was a timer to wait out with neither a reward nor a penalty.
    if real && is_a_guess(npc) && world.was_hurt {
        npc.velocity = (0.0, 0.0);
        finish(npc, true);
        out.cull_clones = Some(if expert {
            CULTIST_RIGHT_GUESS_CULL_EXPERT
        } else {
            CULTIST_RIGHT_GUESS_CULL
        });
        return out;
    }

    // The arrival: seven seconds of fading in, during which nothing touches it.
    if npc.local_ai[0] == 0.0 {
        npc.local_ai[0] = 1.0;
        npc.alpha = 255;
        npc.ai[0] = state::ARRIVING;
    }

    let Some(target) = world.target.filter(|t| t.alive) else {
        return out;
    };
    let (cx, cy) = npc.center();

    match npc.ai[0] {
        state::ARRIVING => {
            npc.invulnerable = true;
            npc.alpha = (npc.alpha - 5).max(0);
            npc.ai[1] += 1.0;
            if npc.ai[1] >= CULTIST_ARRIVAL {
                npc.ai[0] = state::DECIDING;
                npc.ai[1] = 0.0;
                npc.invulnerable = false;
            } else if npc.ai[1] > 360.0 {
                npc.velocity.0 *= 0.95;
                npc.velocity.1 *= 0.95;
            } else if npc.ai[1] > 300.0 {
                npc.velocity = (0.0, -1.0);
            }
        }

        state::DECIDING => {
            npc.direction = (target.center.0 - cx).signum() as i8;
            npc.sprite_direction = npc.direction;
            npc.ai[1] += 1.0;
            if npc.ai[1] < CULTIST_PAUSE || !real {
                return out;
            }
            // The script, indexed by how many attacks it has made. Running off the end restarts
            // it. There are two scripts, chosen by health, not one shared by both halves of the
            // fight (`NPC.cs:65361-65461`).
            let script: &[u8] = if wounded {
                &CULTIST_SCRIPT_WOUNDED
            } else {
                &CULTIST_SCRIPT_HEALTHY
            };
            let step = npc.ai[3] as usize;
            let what = script.get(step).copied().unwrap_or(0);
            if step + 1 >= script.len() {
                npc.ai[3] = -1.0;
            }
            npc.ai[1] = 0.0;
            npc.ai[0] = match what {
                1 => state::ICE,
                2 => state::FIREBALLS,
                3 => state::LIGHTNING,
                4 => state::RITUAL,
                5 => state::SHADOWFLAME,
                _ => {
                    // Move. The destination is an ellipse above the player, and the whole group
                    // shares it out so they fan rather than stack.
                    let station = orbit_slot(target.center, 0, clones + 1);
                    let gap = (station.0 - cx, station.1 - cy);
                    let steps = (gap.0.hypot(gap.1) / CULTIST_MOVE_STEP).ceil().max(1.0);
                    npc.velocity = (gap.0 / steps, gap.1 / steps);
                    npc.ai[1] = steps * 2.0;
                    out.move_to = Some(station);
                    state::MOVING
                }
            };
        }

        state::MOVING => {
            // It travels in steps rather than smoothly, holding still every other tick.
            if (npc.ai[1] as i32) % 2 != 0 && npc.ai[1] != 1.0 {
                npc.position.0 -= npc.velocity.0;
                npc.position.1 -= npc.velocity.1;
            }
            npc.ai[1] -= 1.0;
            if npc.ai[1] <= 0.0 {
                npc.ai[0] = state::DECIDING;
                npc.ai[1] = 0.0;
                npc.velocity = (0.0, 0.0);
                if real {
                    npc.ai[3] += 1.0;
                }
            }
        }

        state::FIREBALLS => {
            let every = if expert {
                CULTIST_FIRE_EVERY_EXPERT
            } else {
                CULTIST_FIRE_EVERY
            };
            let count = if expert {
                CULTIST_FIRE_COUNT_EXPERT
            } else {
                CULTIST_FIRE_COUNT
            };
            npc.ai[1] += 1.0;
            if npc.ai[1] >= 4.0 && (npc.ai[1] as i32 - 4) % every as i32 == 0 {
                out.shots.push(aimed(
                    npc,
                    target.center,
                    CULTIST_FIRE,
                    CULTIST_FIRE_DAMAGE,
                    9.0,
                ));
            }
            if npc.ai[1] >= 4.0 + every * count as f32 {
                finish(npc, real);
            }
        }

        state::ICE => {
            let every = if expert {
                CULTIST_ICE_EVERY_EXPERT
            } else {
                CULTIST_ICE_EVERY
            };
            npc.ai[1] += 1.0;
            if npc.ai[1] as i32 == every as i32 {
                out.shots.push(aimed(
                    npc,
                    target.center,
                    CULTIST_ICE,
                    CULTIST_ICE_DAMAGE,
                    6.0,
                ));
            }
            if npc.ai[1] >= every + 20.0 {
                finish(npc, real);
            }
        }

        state::LIGHTNING => {
            let every = if expert {
                CULTIST_LIGHTNING_EVERY_EXPERT
            } else {
                CULTIST_LIGHTNING_EVERY
            };
            npc.ai[1] += 1.0;
            if npc.ai[1] as i32 == every as i32 / 2 {
                out.shots.push(aimed(
                    npc,
                    target.center,
                    CULTIST_LIGHTNING,
                    CULTIST_LIGHTNING_DAMAGE,
                    7.0,
                ));
            }
            if npc.ai[1] >= every {
                finish(npc, real);
            }
        }

        state::SHADOWFLAME => {
            // Five Ancient Lights fanned out toward you, twice over — the attack that used to be
            // an idle "move" (`NPC.cs:65949-66020`).
            let every = if expert {
                CULTIST_SHADOWFLAME_EVERY_EXPERT
            } else {
                CULTIST_SHADOWFLAME_EVERY
            };
            npc.ai[1] += 1.0;
            // `local_ai[1]` counts the volleys actually fired, which is what caps it at exactly
            // `CULTIST_SHADOWFLAME_COUNT` — deriving the cap from `ai[1]` arithmetic alone can
            // let a fire tick and the termination tick land together and fire one too many.
            if npc.ai[1] >= 4.0
                && (npc.ai[1] as i32 - 4) % every as i32 == 0
                && (npc.local_ai[1] as i32) < CULTIST_SHADOWFLAME_COUNT
            {
                npc.local_ai[1] += 1.0;
                let aim = (target.center.0 - cx, target.center.1 - cy);
                let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
                let aim = (aim.0 / length, aim.1 / length);
                let arc = std::f32::consts::TAU / CULTIST_SHADOWFLAME_SPAWNS as f32;
                let start = -(arc - CULTIST_SHADOWFLAME_ANGLE_STEP) / 2.0;
                for i in 0..CULTIST_SHADOWFLAME_SPAWNS {
                    let angle = start + CULTIST_SHADOWFLAME_ANGLE_STEP * i as f32;
                    let (s, c) = angle.sin_cos();
                    let v = (aim.0 * c - aim.1 * s, aim.0 * s + aim.1 * c);
                    out.spawn.push(Spawn {
                        handle: None,
                        npc_type: CULTIST_ANCIENT_LIGHT,
                        position: (cx, cy),
                        velocity: (
                            v.0 * CULTIST_SHADOWFLAME_SPEED,
                            v.1 * CULTIST_SHADOWFLAME_SPEED,
                        ),
                        parent: None,
                        ai: [None; 4],
                    });
                }
            }
            if npc.ai[1] >= 4.0 + every * CULTIST_SHADOWFLAME_COUNT as f32 {
                npc.local_ai[1] = 0.0;
                finish(npc, real);
            }
        }

        state::STUNNED => {
            // What a wrong guess buys you: two seconds of it standing there doing nothing, and
            // then it picks up the script again (`NPC.cs:65936-65948`).
            npc.velocity = (0.0, 0.0);
            npc.ai[1] += 1.0;
            if npc.ai[1] >= CULTIST_STUN_TICKS {
                finish(npc, real);
            }
        }

        _ => {
            // The ritual. It tops its group up once, and then they all cast together.
            //
            // Not a flat four every time: vanilla adds at most two and never grows past six
            // (`NPC.cs:65808-65812`), so the first ritual is a choice between three and a late one
            // a choice between seven. And they stand on a 180-pixel circle around the boss
            // (`NPC.cs:65798`, `spinningpoint = new Vector2(180f, 0f)` rotated per slot), rather
            // than all four stacked on its centre where the choice would be no choice at all.
            if real && npc.ai[1] == 0.0 {
                let new_clones = CULTIST_CLONES_PER_RITUAL
                    .min(CULTIST_CLONES_MAX - clones.min(CULTIST_CLONES_MAX));
                let slots = clones + new_clones + 1;
                let (cx, cy) = npc.center();
                for slot in 0..new_clones {
                    // The real one takes the slot furthest from the player, so the clones fill the
                    // rest starting from the one nearest it. Which index that is depends on where
                    // the player stands; the offset by one here keeps a new clone off the boss's
                    // own slot without needing to know the whole ring's occupancy.
                    let angle = (clones + slot + 1) as f32 * std::f32::consts::TAU / slots as f32
                        - std::f32::consts::FRAC_PI_2;
                    out.spawn.push(Spawn {
                        handle: None,
                        npc_type: CULTIST_CLONE,
                        position: (
                            cx + angle.cos() * CULTIST_CLONE_RING,
                            cy + angle.sin() * CULTIST_CLONE_RING,
                        ),
                        velocity: (0.0, 0.0),
                        parent: Some(Spawn::OWN_PARENT),
                        ai: [None; 4],
                    });
                }
            }
            npc.ai[1] += 1.0;
            if npc.ai[1] >= CULTIST_RITUAL_TICKS {
                finish(npc, real);
            }
        }
    }
    out
}

/// Whether hitting this one right now is the guess that advances the fight.
///
/// Only during the ritual, and only in the window after the clones have settled — a hit before
/// that is not a guess, it is a stray shot from the previous attack.
pub fn is_a_guess(npc: &Npc) -> bool {
    npc.ai[0] == state::RITUAL
        && npc.ai[1] >= CULTIST_RITUAL_WINDOW.0
        && npc.ai[1] < CULTIST_RITUAL_WINDOW.1
}

/// One of the positions on the ellipse above the player, shared out between the group.
fn orbit_slot(player: (f32, f32), index: usize, of: usize) -> (f32, f32) {
    let even = of.is_multiple_of(2);
    let step = (index + usize::from(even)).div_ceil(2) as f32;
    let mut angle = step * std::f32::consts::TAU * CULTIST_ORBIT_SPREAD / of as f32;
    if index % 2 == 1 {
        angle = -angle;
    }
    if of == 1 {
        angle = 0.0;
    }
    // Straight up, rotated, then squashed into the ellipse.
    let (sin, cos) = angle.sin_cos();
    (
        player.0 + sin * CULTIST_ORBIT.0,
        player.1 - cos * CULTIST_ORBIT.1,
    )
}

/// A shot aimed at the player, from the caster's middle.
fn aimed(npc: &Npc, player: (f32, f32), projectile: u16, damage: i32, speed: f32) -> Shot {
    let (cx, cy) = npc.center();
    let aim = (player.0 - cx, player.1 - cy);
    let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
    Shot {
        projectile,
        damage,
        position: (cx, cy),
        velocity: (aim.0 / length * speed, aim.1 / length * speed),
        time_left: 600,
        ai: [0.0; 3],
    }
}

/// End an attack and step the script on, but only for the real one — a clone follows.
fn finish(npc: &mut Npc, real: bool) {
    npc.ai[0] = state::DECIDING;
    npc.ai[1] = 0.0;
    if real {
        npc.ai[3] += 1.0;
    }
}

/// Stun the real Cultist because one of its decoys was destroyed (`NPC.cs:65203-65206`).
///
/// Applied by the caller rather than by the decoy, because a routine cannot reach another NPC. The
/// decoy reports [`CultistOutcome::punish_owner`] and the server walks its parent link to here.
pub fn punish(owner: &mut Npc) {
    owner.ai[0] = state::STUNNED;
    owner.ai[1] = 0.0;
    owner.dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::CULTIST;
    use terrustia_proto::tile::Tile;

    struct Sky(HashMap<(i32, i32), Tile>);

    impl TileView for Sky {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn world<'a>(tiles: &'a Sky, target: Option<(f32, f32)>) -> World<'a, Sky> {
        crate::game::ai::calm(
            tiles,
            target.map(|center| Target {
                slot: 0,
                center,
                velocity: (0.0, 0.0),
                alive: true,
            }),
        )
    }

    /// The real Cultist as a clone sees it. Its state is its `ai[0]`, which is `Parent::phase`;
    /// `Parent::state` is the parent's `ai[1]`, the timer running inside that state.
    fn owner_in(phase: f32) -> Parent {
        Parent {
            position: (0.0, 0.0),
            size: (40.0, 60.0),
            rotation: 0.0,
            scale: 1.0,
            velocity: (0.0, 0.0),
            direction: 1,
            sprite_direction: 1,
            time_left: 3600,
            state: 0.0,
            phase,
            health: 1.0,
        }
    }

    fn caster(npc_type: u16) -> Npc {
        let mut npc = Npc::new(npc_type, (0.0, 0.0), 1).expect("a cultist");
        // Past the arrival, so the tests are about the fight rather than the entrance.
        npc.local_ai[0] = 1.0;
        npc.ai[0] = state::DECIDING;
        npc
    }

    /// It cannot be touched while it is arriving.
    #[test]
    fn it_arrives_untouchable() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(84);
        let mut c = Npc::new(CULTIST, (0.0, 0.0), 1).unwrap();
        let w = world(&tiles, Some((300.0, 0.0)));

        cultist(&mut c, &w, None, 0, &mut rng);
        assert_eq!(c.ai[0], state::ARRIVING);
        assert!(c.invulnerable, "nothing lands on it yet");

        for _ in 0..(CULTIST_ARRIVAL as i32 + 2) {
            cultist(&mut c, &w, None, 0, &mut rng);
        }
        assert_eq!(c.ai[0], state::DECIDING, "then it starts");
        assert!(!c.invulnerable);
    }

    /// The script is fixed, so two fights run in the same order.
    #[test]
    fn its_attacks_come_in_a_fixed_order() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((300.0, 200.0)));
        let sequence = |seed: u64| {
            let mut rng = SmallRng::seed_from_u64(seed);
            let mut c = caster(CULTIST);
            let mut seen = Vec::new();
            let mut was = c.ai[0];
            for _ in 0..4000 {
                cultist(&mut c, &w, None, 0, &mut rng);
                if c.ai[0] != was {
                    if c.ai[0] != state::DECIDING {
                        seen.push(c.ai[0]);
                    }
                    was = c.ai[0];
                }
                if seen.len() >= 8 {
                    break;
                }
            }
            seen
        };
        assert_eq!(
            sequence(1),
            sequence(999),
            "the script should not depend on the dice"
        );
        assert!(
            sequence(1).contains(&state::ICE),
            "and should include its attacks: {:?}",
            sequence(1)
        );
    }

    /// B9: below half health it runs a different, longer script that includes the shadowflame —
    /// not the same fixed script the healthy half of the fight uses.
    #[test]
    fn a_wounded_cultist_runs_a_different_script() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((300.0, 200.0)));
        let sequence = |wounded: bool| {
            let mut rng = SmallRng::seed_from_u64(9);
            let mut c = caster(CULTIST);
            if wounded {
                c.life = c.life_max / 4;
            }
            let mut seen = Vec::new();
            let mut was = c.ai[0];
            for _ in 0..6000 {
                cultist(&mut c, &w, None, 0, &mut rng);
                if c.ai[0] != was {
                    if c.ai[0] != state::DECIDING {
                        seen.push(c.ai[0]);
                    }
                    was = c.ai[0];
                }
                if seen.len() >= 14 {
                    break;
                }
            }
            seen
        };
        let healthy = sequence(false);
        let wounded = sequence(true);
        assert_ne!(
            healthy, wounded,
            "a wounded cultist should not run the healthy script"
        );
        assert!(
            wounded.contains(&state::SHADOWFLAME),
            "and its script should include the shadowflame: {wounded:?}"
        );
        assert!(
            !healthy.contains(&state::SHADOWFLAME),
            "which the healthy script never reaches: {healthy:?}"
        );
    }

    /// B9 (found while fixing it): ice, fire and lightning had each other's damage numbers and
    /// were firing three completely unrelated placeholder projectiles instead of
    /// `CultistBossIceMist`/`FireBall`/`LightningOrb`.
    #[test]
    fn it_casts_the_real_vanilla_projectiles_with_the_right_damage() {
        let tiles = Sky(HashMap::new());
        let w = world(&tiles, Some((300.0, 0.0)));
        let shot_for = |state_val: f32| {
            let mut rng = SmallRng::seed_from_u64(11);
            let mut c = caster(CULTIST);
            c.ai[0] = state_val;
            (0..200).find_map(|_| {
                cultist(&mut c, &w, None, 0, &mut rng)
                    .shots
                    .into_iter()
                    .next()
            })
        };
        let ice = shot_for(state::ICE).expect("ice should have fired");
        assert_eq!(ice.projectile, 464, "CultistBossIceMist");
        assert_eq!(ice.damage, 35);

        let fire = shot_for(state::FIREBALLS).expect("fire should have fired");
        assert_eq!(fire.projectile, 467, "CultistBossFireBall");
        assert_eq!(fire.damage, 30);

        let lightning = shot_for(state::LIGHTNING).expect("lightning should have fired");
        assert_eq!(lightning.projectile, 465, "CultistBossLightningOrb");
        assert_eq!(lightning.damage, 45);
    }

    /// B9: the shadowflame fans out five Ancient Lights toward you, twice over.
    #[test]
    fn the_shadowflame_fires_five_ancient_lights_twice() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(10);
        let w = world(&tiles, Some((300.0, 0.0)));
        let mut c = caster(CULTIST);
        c.ai[0] = state::SHADOWFLAME;

        let mut volleys = Vec::new();
        for _ in 0..400 {
            let out = cultist(&mut c, &w, None, 0, &mut rng);
            let lights = out
                .spawn
                .iter()
                .filter(|s| s.npc_type == CULTIST_ANCIENT_LIGHT)
                .count();
            if lights > 0 {
                volleys.push(lights);
            }
            if c.ai[0] == state::DECIDING {
                break;
            }
        }
        assert_eq!(
            volleys,
            vec![5, 5],
            "it should fan five Ancient Lights out, twice"
        );
    }

    /// A clone copies the real one's state rather than deciding for itself.
    #[test]
    fn a_clone_mirrors_the_real_one() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(2);
        let w = world(&tiles, Some((300.0, 0.0)));
        let mut clone = caster(CULTIST_CLONE);

        cultist(&mut clone, &w, Some(owner_in(state::ICE)), 4, &mut rng);
        assert_eq!(clone.ai[0], state::ICE, "it does what the real one does");

        cultist(
            &mut clone,
            &w,
            Some(owner_in(state::FIREBALLS)),
            4,
            &mut rng,
        );
        assert_eq!(clone.ai[0], state::FIREBALLS);
    }

    /// A clone with no original does not survive.
    #[test]
    fn a_clone_without_an_original_is_gone() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(3);
        let w = world(&tiles, Some((300.0, 0.0)));
        let mut clone = caster(CULTIST_CLONE);
        assert!(cultist(&mut clone, &w, None, 0, &mut rng).spent);
    }

    /// Each ritual tops the group up by two, up to six, and stands them on a circle around it.
    ///
    /// Not a flat four every time and not all on the boss's own centre: `NPC.cs:65808-65812` adds
    /// `min(2, 6 - existing)` and `NPC.cs:65798`/`:65826` lay the whole group out on a 180-pixel
    /// ring. Spawning four stacked on one point made the choice no choice at all, and capped the
    /// fight's difficulty at its first ritual.
    #[test]
    fn each_ritual_adds_two_clones_on_a_ring_up_to_six() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(5);
        let w = world(&tiles, Some((300.0, 0.0)));
        let mut c = caster(CULTIST);
        c.ai[0] = state::RITUAL;
        let middle = c.center();

        let out = cultist(&mut c, &w, None, 0, &mut rng);
        assert_eq!(out.spawn.len(), CULTIST_CLONES_PER_RITUAL, "two at a time");
        assert!(out.spawn.iter().all(|s| s.npc_type == CULTIST_CLONE));
        for spawned in &out.spawn {
            let reach = (spawned.position.0 - middle.0).hypot(spawned.position.1 - middle.1);
            assert!(
                (reach - CULTIST_CLONE_RING).abs() < 1.0,
                "each stands on the ring, got {reach}"
            );
        }

        // A later ritual tops the group up to six and then stops.
        c.ai[1] = 0.0;
        assert_eq!(cultist(&mut c, &w, None, 4, &mut rng).spawn.len(), 2);
        c.ai[1] = 0.0;
        assert!(cultist(&mut c, &w, None, 6, &mut rng).spawn.is_empty());
    }

    /// The whole point of the ritual, and it did nothing at all before this.
    ///
    /// Hitting the real one inside the window ends the ritual and destroys some of the group
    /// (`NPC.cs:65215-65262`): ten in classic, more than can ever be out, so the group is cleared;
    /// three in expert, so against a grown group some always survive. Hitting a decoy kills it and
    /// stuns the real one for two seconds (`NPC.cs:65194-65207`). `is_a_guess` had no caller
    /// outside its own test, so the illusion phase was a timer to wait out: no reward for guessing
    /// right and no penalty for guessing wrong.
    #[test]
    fn the_guess_is_the_fight() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(6);
        let mut w = world(&tiles, Some((300.0, 0.0)));

        // Right, in classic: the ritual ends and the whole group goes.
        let mut c = caster(CULTIST);
        c.ai[0] = state::RITUAL;
        c.ai[1] = CULTIST_RITUAL_WINDOW.0 + 10.0;
        w.was_hurt = true;
        let out = cultist(&mut c, &w, None, 4, &mut rng);
        assert_eq!(out.cull_clones, Some(CULTIST_RIGHT_GUESS_CULL));
        assert_eq!(c.ai[0], state::DECIDING, "and the ritual is over");

        // Right, in expert: only three of them, so a grown group outlives the guess.
        let mut c = caster(CULTIST);
        c.ai[0] = state::RITUAL;
        c.ai[1] = CULTIST_RITUAL_WINDOW.0 + 10.0;
        w.conditions.expert = true;
        let out = cultist(&mut c, &w, None, 4, &mut rng);
        assert_eq!(out.cull_clones, Some(CULTIST_RIGHT_GUESS_CULL_EXPERT));
        w.conditions.expert = false;

        // Wrong: the decoy dies and asks for its owner to pay for it.
        let mut clone = caster(CULTIST_CLONE);
        let out = cultist(&mut clone, &w, Some(owner_in(state::RITUAL)), 4, &mut rng);
        assert!(out.spent, "the decoy dies");
        assert!(out.punish_owner, "and the real one pays for the guess");

        // Which the caller applies, and which really does stop it for two seconds.
        let mut real = caster(CULTIST);
        punish(&mut real);
        assert_eq!(real.ai[0], state::STUNNED);
        w.was_hurt = false;
        for _ in 0..(CULTIST_STUN_TICKS as i32) {
            assert_eq!(real.ai[0], state::STUNNED, "it stands there and takes it");
            cultist(&mut real, &w, None, 0, &mut rng);
        }
        assert_eq!(real.ai[0], state::DECIDING, "and then picks the script up");
    }

    /// Hitting one only counts as a guess inside the ritual's window.
    #[test]
    fn only_a_hit_during_the_ritual_is_a_guess() {
        let mut c = Npc::new(CULTIST, (0.0, 0.0), 1).unwrap();
        c.ai[0] = state::ICE;
        assert!(!is_a_guess(&c), "not while it is casting");

        c.ai[0] = state::RITUAL;
        c.ai[1] = 10.0;
        assert!(!is_a_guess(&c), "nor before the clones have settled");

        c.ai[1] = CULTIST_RITUAL_WINDOW.0 + 10.0;
        assert!(is_a_guess(&c), "but inside the window it is");

        c.ai[1] = CULTIST_RITUAL_WINDOW.1 + 10.0;
        assert!(!is_a_guess(&c), "and not after it has passed");
    }

    /// The group fans out around the player rather than stacking on one spot.
    #[test]
    fn the_group_shares_out_its_positions() {
        let player = (0.0, 0.0);
        let slots: Vec<(f32, f32)> = (0..5).map(|i| orbit_slot(player, i, 5)).collect();
        for (i, a) in slots.iter().enumerate() {
            for b in &slots[i + 1..] {
                assert!(
                    (a.0 - b.0).abs() > 1.0 || (a.1 - b.1).abs() > 1.0,
                    "two of them wanted the same place: {a:?} and {b:?}"
                );
            }
        }
        // All above the player, which is where the ellipse sits.
        assert!(slots.iter().all(|s| s.1 < player.1), "{slots:?}");
    }

    /// Below half health it sheds a third of its armour.
    #[test]
    fn a_hurt_cultist_is_softer() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(7);
        let w = world(&tiles, Some((300.0, 0.0)));
        let mut c = caster(CULTIST);
        c.life = c.life_max / 4;
        cultist(&mut c, &w, None, 0, &mut rng);
        assert!(
            c.defense < c.stats.defense,
            "{} should be under {}",
            c.defense,
            c.stats.defense
        );
    }
}
