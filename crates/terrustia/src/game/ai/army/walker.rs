//! Style 107: the Old One's Army's ground troops.
//!
//! Twenty creatures on one routine, from a goblin that runs at you to an ogre with three separate
//! attacks. What they share is the walking: accelerate to a top speed, climb what is in front of
//! you, jump gaps, and shove past anything you have been stuck against for half a second. What
//! differs is entirely in [`Walker`], which is generated from the game's own per-type block.
//!
//! Two things about them are peculiar to this event. They come out of a lane portal faded, and
//! while they are arriving they can walk through the world — a goblin that cannot see you flies
//! at you through the terrain rather than pathing round it, which is what keeps a siege from
//! stalling on the arena walls. And a kobold does not die: it lights a fuse, runs at you, and
//! goes off.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    ARMY_FADE_IN, KOBOLD_BLAST, KOBOLD_BLAST_DAMAGE, OGRE_POUND_COOLDOWN, OgreAttack, WITHER_AURA,
    WITHER_FEEDS_EVERY, WITHER_HEALS, Walker, ogre_attack, walker,
};

use crate::game::ai::{Shot, World};
use crate::game::npc::{Npc, TileView};
use crate::game::npc_ai::Target;

/// What it did this tick.
#[derive(Debug, Default)]
pub struct WalkerOutcome {
    pub shots: Vec<Shot>,
    /// Set on the tick a kobold's blast goes off.
    pub burst: bool,
    pub spent: bool,
    /// How much its aura just gave it back.
    pub healed: i32,
    /// Set while its aura is out, so the caller can weaken anyone standing in it.
    pub aura: Option<f32>,
}

pub fn improved_walker(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    rng: &mut SmallRng,
) -> WalkerOutcome {
    let mut out = WalkerOutcome::default();
    npc.dirty = true;
    let mut it = walker(npc.npc_type);

    // Some types are not simply themselves: an ogre is whichever of its three attacks it has
    // chosen, and a kobold with a lit fuse is a different creature from one without.
    if is_ogre(npc.npc_type) {
        it = ogre(npc, world, it);
    }
    if it.explodes && fuse(npc, &mut it, &mut out) {
        return out;
    }
    if it.aura {
        aura(npc, &mut out);
    }

    // Coming out of the ground, or out of a portal: it cannot be touched and cannot move.
    if it.rises_for > 0.0 && npc.local_ai[3] < it.rises_for {
        npc.local_ai[3] += 1.0;
        npc.invulnerable = true;
        npc.velocity.0 = 0.0;
        npc.alpha = (255 - ((npc.local_ai[3] - 110.0).max(0.0) as i32 * 26)).clamp(0, 255);
        return out;
    }
    npc.invulnerable = false;
    let arrived = if it.from_portal {
        if npc.local_ai[3] < ARMY_FADE_IN {
            npc.local_ai[3] += 1.0;
            npc.alpha = (255 - (npc.local_ai[3] as i32 * 5)).max(0);
        }
        npc.local_ai[3] >= ARMY_FADE_IN
    } else {
        true
    };
    // Still arriving, it accelerates gently rather than at its own rate.
    if !arrived {
        it.accel = 0.01 + npc.local_ai[3] / ARMY_FADE_IN * 0.05;
        it.ranged_range = 1.0;
    }

    let target = world.target.filter(|t| t.alive);

    // Nothing between it and you that it can walk round: it comes through the world instead.
    let phasing = it.from_portal
        && npc.ai[0] <= 0.0
        && target.is_some_and(|t| {
            let (cx, cy) = npc.center();
            !crate::game::ai::can_see(world.tiles, npc, t)
                && (i32::from(npc.direction) == (t.center.0 - cx).signum() as i32
                    || (npc.no_gravity
                        && (t.center.0 - cx).hypot(t.center.1 - cy) > 50.0
                        && cy > t.center.1))
        });
    if phasing
        && let Some(t) = target
        && (npc.velocity.1 == 0.0 || (t.center.1 - npc.center().1).abs() > 800.0)
    {
        npc.no_gravity = true;
        npc.no_tile_collide = true;
    } else {
        npc.no_gravity = false;
        npc.no_tile_collide = false;
    }

    if it.swims && world.wet {
        swim(npc, world, target);
        return out;
    }

    let was_still = npc.velocity == (0.0, 0.0) && !world.was_hurt;
    let mut busy = false;
    // Whether the walking acceleration at the bottom should be skipped: a thrower does its own.
    let mut own_pace = it.ranged;

    if it.melee {
        busy |= melee(npc, world, &it, target, &mut own_pace, &mut out, rng);
    }
    if it.ranged && npc.ai[1] > 0.0 {
        busy = true;
    }

    // Stuck: shoved bodily past whatever it has been walking into.
    if !busy && stuck(npc, &it, target, world) {
        return out;
    }

    if !busy {
        if it.chases && npc.ai[3] < it.stuck_ticks as f32 {
            if let Some(t) = target {
                face(npc, t);
            }
        } else {
            wander(npc, &it);
        }
    }

    if !own_pace {
        walk(npc, &it, npc.direction);
    } else if it.ranged {
        ranged(npc, world, &it, target, rng, &mut out);
    }

    climb(npc, world, &it, was_still);
    if phasing && npc.no_tile_collide {
        phase(npc, target);
    }
    out
}

fn is_ogre(npc_type: u16) -> bool {
    matches!(
        npc_type,
        terrustia_proto::npc_params::DD2_OGRE_T2 | terrustia_proto::npc_params::DD2_OGRE_T3
    )
}

/// The ogre picks its attack by range, not in turn, and remembers not to pound twice running.
fn ogre(npc: &mut Npc, world: &World<'_, impl TileView>, it: Walker) -> Walker {
    if npc.local_ai[0] > 0.0 {
        npc.local_ai[0] -= 1.0;
    }
    if npc.ai[0] <= 0.0 {
        let was = npc.ai[1];
        if let Some(t) = world.target.filter(|t| t.alive)
            && npc.local_ai[3] >= ARMY_FADE_IN
        {
            let (cx, cy) = npc.center();
            let gap = (t.center.0 - cx).hypot(t.center.1 - cy);
            if gap <= it.melee_range + 300.0 && npc.local_ai[0] <= 0.0 {
                npc.ai[1] = OgreAttack::Pound as i32 as f32;
            } else if gap > it.melee_range + 30.0 {
                npc.ai[1] = OgreAttack::Spit as i32 as f32;
            } else if gap <= it.melee_range {
                npc.ai[1] = OgreAttack::Swipe as i32 as f32;
                // Coming out of the spit it drops straight into the swing.
                if was == OgreAttack::Spit as i32 as f32 {
                    npc.ai[0] = 0.0;
                }
            }
        }
    } else if npc.ai[1] == OgreAttack::Pound as i32 as f32 {
        // Pounding sets its own long cooldown, so it cannot simply pound forever.
        npc.local_ai[0] = OGRE_POUND_COOLDOWN;
    }
    let attack = match npc.ai[1] as i32 {
        1 => OgreAttack::Spit,
        2 => OgreAttack::Pound,
        _ => OgreAttack::Swipe,
    };
    let chosen = ogre_attack(it, attack);
    npc.ai[0] = npc.ai[0].max(-(chosen.melee_cooldown as f32));
    chosen
}

/// A kobold's fuse. Returns whether the routine should stop here.
fn fuse(npc: &mut Npc, it: &mut Walker, out: &mut WalkerOutcome) -> bool {
    // `ai[1]` is the fuse: nought unlit, one lit, two gone off.
    if npc.ai[1] == 2.0 {
        // Going off: it swells into its blast, waits three ticks, and is finished.
        let (cx, cy) = npc.center();
        npc.size = Some((KOBOLD_BLAST, KOBOLD_BLAST));
        npc.position = (cx - KOBOLD_BLAST / 2.0, cy - KOBOLD_BLAST / 2.0);
        npc.velocity = (0.0, 0.0);
        npc.set_contact_damage(KOBOLD_BLAST_DAMAGE);
        npc.ai[0] += 1.0;
        if npc.ai[0] >= 3.0 {
            out.burst = true;
            out.spent = true;
        }
        return true;
    }
    // Reaching a target with the fuse lit is what sets it off.
    if npc.ai[0] > 0.0 && npc.ai[1] == 1.0 {
        npc.ai[0] = 0.0;
        npc.ai[1] = 2.0;
        return true;
    }
    // The first swing lights it.
    if npc.ai[0] == 1.0 {
        npc.ai[1] = 1.0;
    }
    if npc.ai[1] > 0.0 && npc.ai[0] == 0.0 {
        // Lit: it stops being a walker and becomes a charge.
        it.melee_range = 64.0;
        it.accel = 0.3;
        it.max_speed = 4.0;
    }
    false
}

/// The wither beast's aura: it drains what stands in it and feeds itself on that.
fn aura(npc: &mut Npc, out: &mut WalkerOutcome) {
    if npc.ai[0] != 1.0 {
        return;
    }
    npc.ai[0] += 1.0;
    out.aura = Some(WITHER_AURA);
    if npc.ai[1] > 0.0 {
        npc.ai[1] -= 1.0;
    }
    if npc.ai[1] <= 0.0 {
        npc.ai[1] = WITHER_FEEDS_EVERY;
        let wanted = (npc.life_max / WITHER_HEALS).min(npc.life_max - npc.life);
        if wanted > 0 {
            npc.life += wanted;
            out.healed = wanted;
        }
    }
}

/// The close attack. Returns whether it is busy enough to skip walking.
#[allow(clippy::too_many_arguments)]
fn melee(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    it: &Walker,
    target: Option<Target>,
    own_pace: &mut bool,
    out: &mut WalkerOutcome,
    rng: &mut SmallRng,
) -> bool {
    // Negative is the cooldown counting back up to nought.
    if npc.ai[0] < 0.0 {
        npc.ai[0] += 1.0;
    }
    if npc.ai[0] == 0.0
        && npc.velocity.1 == 0.0
        && let Some(t) = target
        && crate::game::ai::can_see(world.tiles, npc, t)
    {
        let (cx, cy) = npc.center();
        if (t.center.0 - cx).hypot(t.center.1 - cy) < it.melee_range {
            npc.ai[0] = it.melee_ticks as f32;
        }
    }

    if npc.ai[0] <= 0.0 {
        return false;
    }

    npc.sprite_direction = npc.direction;
    npc.velocity.0 *= it.melee_brake;
    *own_pace = true;
    npc.ai[3] = 0.0;

    // The throw and the leap both happen at fixed points inside the swing, not at its start.
    if it.melee_throws && npc.ai[0] == it.ranged_at as f32 {
        throw(npc, world, it, target, rng, out);
    }
    if it.leaps {
        // Interrupted mid-air, the swing is cut short rather than abandoned.
        if npc.velocity.1 != 0.0 && npc.ai[0] < it.leap_floor as f32 {
            npc.ai[0] = it.leap_floor as f32;
        }
        if npc.ai[0] == it.leap_at as f32 {
            npc.velocity.1 = -it.leap_speed;
        }
    }

    npc.ai[0] -= 1.0;
    if npc.ai[0] == 0.0 {
        npc.ai[0] = -(it.melee_cooldown as f32);
    }
    true
}

/// The ranged attack: wind up, throw, and stand still until the wind-down is over.
fn ranged(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    it: &Walker,
    target: Option<Target>,
    rng: &mut SmallRng,
    out: &mut WalkerOutcome,
) {
    if npc.ai[1] > 0.0 {
        npc.ai[1] -= 1.0;
    }
    // Being hit interrupts the throw and puts it on a delay.
    if world.was_hurt {
        npc.ai[1] = it.hurt_delay;
        npc.ai[0] = 0.0;
    }

    if npc.ai[0] > 0.0 {
        if npc.ai[1] as i32 >= it.retarget_from
            && let Some(t) = target
        {
            face(npc, t);
        }
        if npc.ai[1] == it.ranged_at as f32 {
            let aim = throw(npc, world, it, target, rng, out);
            // The pose it holds afterwards depends on where it threw, which is why a javelinist
            // aiming up looks different from one aiming across.
            npc.ai[0] = pose(aim);
            if it.turn_to_shot {
                npc.direction = if aim.0 > 0.0 { 1 } else { -1 };
            }
        }
        // Landing, or running out of wind-down, ends it.
        if npc.velocity.1 != 0.0 || npc.ai[1] <= 0.0 {
            let resting = it.shot_cooldown != 0.0 && npc.ai[1] <= 0.0;
            npc.ai[0] = 0.0;
            npc.ai[1] = if resting { it.shot_cooldown } else { 0.0 };
        } else {
            npc.velocity.0 *= 0.9;
            npc.sprite_direction = npc.direction;
        }
    }

    // Starting one: it has to be on the ground, off cooldown, and close enough to be worth it.
    if npc.ai[0] <= 0.0
        && npc.velocity.1 == 0.0
        && npc.ai[1] <= 0.0
        && let Some(t) = target
        && crate::game::ai::can_see(world.tiles, npc, t)
    {
        let (cx, cy) = npc.center();
        let to = (t.center.0 - cx, t.center.1 - cy);
        if to.0.hypot(to.1) < it.ranged_range {
            npc.velocity.0 *= 0.5;
            npc.ai[1] = it.ranged_ticks as f32;
            npc.ai[0] = pose(to);
            if it.turn_to_shot {
                npc.direction = if to.0 > 0.0 { 1 } else { -1 };
            }
        }
    }

    if npc.ai[0] <= 0.0 {
        walk(npc, it, npc.direction);
    }
}

/// Which of the five aiming poses a direction falls in. It is purely cosmetic but it is synced.
fn pose(aim: (f32, f32)) -> f32 {
    if aim.1.abs() > aim.0.abs() * 2.0 {
        if aim.1 > 0.0 { 1.0 } else { 5.0 }
    } else if aim.0.abs() > aim.1.abs() * 2.0 {
        3.0
    } else if aim.1 > 0.0 {
        2.0
    } else {
        4.0
    }
}

/// Let something fly. Returns the direction it went, before the scatter.
fn throw(
    npc: &Npc,
    world: &World<'_, impl TileView>,
    it: &Walker,
    target: Option<Target>,
    rng: &mut SmallRng,
    out: &mut WalkerOutcome,
) -> (f32, f32) {
    let Some(t) = target else {
        return (f32::from(npc.direction), 0.0);
    };
    let (cx, cy) = npc.center();
    let from = (
        cx + it.muzzle.0 * f32::from(npc.direction),
        cy + it.muzzle.1,
    );
    let mut aim = (t.center.0 - from.0, t.center.1 - from.1);
    // The lob: it aims higher the further away you are, which is what arcs a bomb over a wall.
    aim.1 -= aim.0.abs() * it.shot_arc;
    let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
    let straight = (
        aim.0 / length * it.shot_speed,
        aim.1 / length * it.shot_speed,
    );
    // Every troop that throws something deals noticeably less in Expert Mode.
    let damage = if world.conditions.expert {
        it.shot_damage_expert
    } else {
        it.shot_damage
    };
    for _ in 0..it.shot_count {
        let scatter = (
            rng.random_range(-it.shot_spread..=it.shot_spread),
            rng.random_range(-it.shot_spread..=it.shot_spread),
        );
        let velocity = (straight.0 + scatter.0, straight.1 + scatter.1);
        out.shots.push(Shot {
            projectile: it.shot,
            damage,
            position: (
                from.0 + straight.0 * it.shot_lead,
                from.1 + straight.1 * it.shot_lead,
            ),
            velocity,
            time_left: 600,
            ai: [0.0; 3],
        });
    }
    straight
}

/// Half a second of getting nowhere and it shoves itself past whatever is in the way.
fn stuck(
    npc: &mut Npc,
    it: &Walker,
    target: Option<Target>,
    world: &World<'_, impl TileView>,
) -> bool {
    // Walking backwards counts as stuck too: it means it is being pushed.
    let backwards = npc.velocity.1 == 0.0 && npc.velocity.0 * f32::from(npc.direction) < 0.0;
    if npc.position.0 == npc.old_position.0 || npc.ai[3] >= it.stuck_ticks as f32 || backwards {
        npc.ai[3] += 1.0;
    } else if npc.velocity.0.abs() > 0.9 && npc.ai[3] > 0.0 {
        npc.ai[3] -= 1.0;
    }
    if npc.ai[3] > (it.stuck_ticks * 10) as f32 {
        npc.ai[3] = 0.0;
    }
    if world.was_hurt && !it.teleports_when_stuck {
        npc.ai[3] = 0.0;
    }
    // Touching its target is proof it is not stuck.
    if let Some(t) = target {
        let (cx, cy) = npc.center();
        if (t.center.0 - cx).abs() < npc.width() && (t.center.1 - cy).abs() < npc.height() {
            npc.ai[3] = 0.0;
        }
    }

    if npc.ai[3] == it.stuck_ticks as f32 && it.teleports_when_stuck {
        npc.no_gravity = true;
        npc.no_tile_collide = true;
        npc.position.0 += f32::from(npc.direction) * npc.width() * 2.0;
        return true;
    }
    false
}

/// Nothing to chase: it paces, and turns round whenever it comes to a stop.
fn wander(npc: &mut Npc, it: &Walker) {
    if npc.velocity.0 == 0.0 {
        if npc.velocity.1 == 0.0 {
            npc.ai[2] += 1.0;
            if npc.ai[2] >= 2.0 {
                npc.direction *= -1;
                npc.sprite_direction = npc.direction;
                npc.ai[2] = 0.0;
            }
        }
    } else if npc.ai[2] != 0.0 {
        npc.ai[2] = 0.0;
    }
    if npc.direction == 0 {
        npc.direction = 1;
    }
    if it.despawns {
        npc.time_left = npc.time_left.min(10);
    }
}

/// The walking itself: over the top speed it brakes, under it accelerates.
fn walk(npc: &mut Npc, it: &Walker, direction: i8) {
    if npc.velocity.0 < -it.max_speed || npc.velocity.0 > it.max_speed {
        if npc.velocity.1 == 0.0 {
            npc.velocity.0 *= it.brake;
            npc.velocity.1 *= it.brake;
        }
    } else if (npc.velocity.0 < it.max_speed && direction == 1)
        || (npc.velocity.0 > -it.max_speed && direction == -1)
    {
        npc.velocity.0 =
            (npc.velocity.0 + it.accel * f32::from(direction)).clamp(-it.max_speed, it.max_speed);
    }
}

/// Steps, ledges and gaps. The higher the wall, the harder it jumps.
fn climb(npc: &mut Npc, world: &World<'_, impl TileView>, it: &Walker, was_still: bool) {
    if npc.velocity.1 != 0.0 {
        return;
    }
    let solid = |x: i32, y: i32| {
        let tile = world.tiles.tile(x, y);
        tile.is_active() && terrustia_proto::tile_solid::solid(tile.block)
    };
    // On the ground at all?
    let foot = ((npc.position.1 + npc.height() + 7.0) / 16.0) as i32;
    let left = (npc.position.0 / 16.0) as i32;
    let right = ((npc.position.0 + npc.width()) / 16.0) as i32;
    if !(left..=right).any(|x| solid(x, foot)) {
        return;
    }

    let reach = npc.width() / 2.0 + it.step_reach;
    let ahead = ((npc.center().0 + reach * f32::from(npc.direction)) / 16.0) as i32;
    let level = ((npc.position.1 + npc.height() - 15.0) / 16.0) as i32;
    let deep = npc.position.1 + npc.height() - (level * 16) as f32 > 20.0;

    let facing = npc.sprite_direction;
    if npc.velocity.0 * f32::from(facing) <= 0.0 {
        return;
    }
    if npc.height() >= 32.0 && solid(ahead, level - 2) {
        // A wall two tiles up: a proper jump, higher still if it is three.
        npc.velocity.1 = if solid(ahead, level - 3) { -8.0 } else { -7.0 };
    } else if solid(ahead, level - 1) {
        npc.velocity.1 = -6.0;
    } else if deep && solid(ahead, level) {
        npc.velocity.1 = -5.0;
    } else if npc.direction_y < 0
        && !solid(ahead, level + 1)
        && !solid(ahead + i32::from(npc.direction), level + 1)
    {
        // A gap, and something worth crossing it for.
        npc.velocity.0 *= 1.5;
        npc.velocity.1 = -8.0;
    }
    // Stopped dead against something with the stuck counter just started: a hop.
    if npc.velocity.1 == 0.0 && was_still && npc.ai[3] == 1.0 {
        npc.velocity.1 = -5.0;
    }
}

/// Swimming: it goes straight at what it can reach, and otherwise angles upward.
fn swim(npc: &mut Npc, world: &World<'_, impl TileView>, target: Option<Target>) {
    const AT_TARGET: f32 = 5.0;
    const SINKING: f32 = 3.0;
    const RISING: f32 = 8.0;
    npc.no_gravity = true;
    if npc.collide_x {
        npc.velocity.0 = -npc.old_velocity.0;
    }
    if let Some(t) = target
        && crate::game::ai::can_see(world.tiles, npc, t)
    {
        let (cx, cy) = npc.center();
        let aim = (t.center.0 - cx, t.center.1 - cy);
        let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
        let wanted = (aim.0 / length * AT_TARGET, aim.1 / length * AT_TARGET);
        npc.velocity.0 += (wanted.0 - npc.velocity.0) * 0.05;
        npc.velocity.1 += (wanted.1 - npc.velocity.1) * 0.05;
        return;
    }
    // Blind: it swims up and along, faster when it is already climbing.
    let speed = if npc.velocity.1 > 0.0 {
        SINKING
    } else if npc.velocity.1 < 0.0 {
        RISING
    } else {
        AT_TARGET
    };
    let along = (f32::from(npc.direction), -1.0);
    let length = along.0.hypot(along.1);
    let wanted = (along.0 / length * speed, along.1 / length * speed);
    let ease = if speed < AT_TARGET { 0.04 } else { 0.1 };
    npc.velocity.0 += (wanted.0 - npc.velocity.0) * ease;
    npc.velocity.1 += (wanted.1 - npc.velocity.1) * ease;
}

/// Coming through the world rather than round it: it flies at you, keeping just above the floor.
fn phase(npc: &mut Npc, target: Option<Target>) {
    let Some(t) = target else {
        return;
    };
    let landed = npc.velocity.1 == 0.0;
    let (cx, cy) = npc.center();
    if (cx - t.center.0).abs() > 200.0 {
        npc.direction = if t.center.0 > cx { 1 } else { -1 };
        npc.sprite_direction = npc.direction;
        npc.velocity.0 += (f32::from(npc.direction) - npc.velocity.0) * 0.05;
    }
    let below = npc.position.1 + npc.height() < t.center.1 + 16.0;
    if below {
        npc.velocity.1 += 0.5;
    } else if t.center.1 - cy < -100.0 || (t.center.1 - cy < 10.0 && (t.center.0 - cx).abs() < 60.0)
    {
        if npc.velocity.1 > 0.0 {
            npc.velocity.1 = 0.0;
        }
        npc.velocity.1 -= if npc.velocity.1 > -0.2 { 0.025 } else { 0.2 };
        npc.velocity.1 = npc.velocity.1.max(-4.0);
    } else {
        if npc.velocity.1 < 0.0 {
            npc.velocity.1 = 0.0;
        }
        npc.velocity.1 += if npc.velocity.1 < 0.1 { 0.025 } else { 0.5 };
    }
    npc.velocity.1 = npc.velocity.1.min(10.0);
    if landed {
        npc.velocity.1 = 0.0;
    }
}

fn face(npc: &mut Npc, t: Target) {
    let (cx, _) = npc.center();
    npc.direction = if t.center.0 > cx { 1 } else { -1 };
    npc.sprite_direction = npc.direction;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::{
        DD2_DRAKIN_T3, DD2_GOBLIN_BOMBER_T1, DD2_GOBLIN_T1, DD2_JAVELINST_T3, DD2_KOBOLD_WALKER_T2,
        DD2_OGRE_T2, DD2_SKELETON_T1, DD2_WITHER_BEAST_T2,
    };
    use terrustia_proto::projectile::ids::{GOBLIN_BOMB, JAVELIN_T3, OGRE_POUND, OGRE_SPIT};
    use terrustia_proto::tile::Tile;

    struct Arena(HashMap<(i32, i32), Tile>);

    impl TileView for Arena {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    /// A flat floor at y = 200.
    fn floor() -> Arena {
        let mut tiles = HashMap::new();
        for x in 0..600 {
            for y in 200..210 {
                tiles.insert((x, y), Tile::block(1));
            }
        }
        Arena(tiles)
    }

    const GROUND: f32 = 200.0 * 16.0;

    fn world<'a>(tiles: &'a Arena, target: Option<(f32, f32)>) -> World<'a, Arena> {
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

    /// One standing on the floor, already arrived.
    fn troop(npc_type: u16, x: f32) -> Npc {
        let mut n = Npc::new(npc_type, (x, 0.0), 1).expect("a soldier");
        n.position.1 = GROUND - n.height();
        n.local_ai[3] = 200.0;
        n.direction = 1;
        n.sprite_direction = 1;
        n
    }

    fn run(
        n: &mut Npc,
        w: &World<'_, Arena>,
        tiles: &Arena,
        ticks: i32,
        rng: &mut SmallRng,
    ) -> Vec<WalkerOutcome> {
        (0..ticks)
            .map(|_| {
                let out = improved_walker(n, w, rng);
                crate::game::npc::step_physics(n, tiles);
                out
            })
            .collect()
    }

    /// A goblin walks toward you and keeps to its top speed.
    #[test]
    fn a_goblin_comes_at_you() {
        let tiles = floor();
        let w = world(&tiles, Some((5000.0, GROUND - 20.0)));
        let mut rng = SmallRng::seed_from_u64(1);
        let mut n = troop(DD2_GOBLIN_T1, 1000.0);
        let start = n.position.0;
        run(&mut n, &w, &tiles, 400, &mut rng);
        assert!(
            n.position.0 > start + 100.0,
            "it should have closed a long way"
        );
        assert!(
            n.velocity.0.abs() <= walker(DD2_GOBLIN_T1).max_speed + 0.01,
            "and not exceeded its pace: {}",
            n.velocity.0
        );
    }

    /// A bomber lobs bombs; it does not run at you.
    #[test]
    fn a_bomber_throws_rather_than_charges() {
        let tiles = floor();
        // Well inside its two hundred and eighty pixel range.
        let w = world(&tiles, Some((1200.0, GROUND - 20.0)));
        let mut rng = SmallRng::seed_from_u64(2);
        let mut n = troop(DD2_GOBLIN_BOMBER_T1, 1000.0);
        let thrown: Vec<Shot> = run(&mut n, &w, &tiles, 600, &mut rng)
            .into_iter()
            .flat_map(|o| o.shots)
            .collect();
        assert!(
            thrown.len() >= 5,
            "it should be lobbing steadily: {}",
            thrown.len()
        );
        assert!(thrown.iter().all(|s| s.projectile == GOBLIN_BOMB));

        // Expert Mode's version of the same throw hits for less, not more — the two values are
        // not related by a fixed ratio, so this only holds if both are genuinely stored.
        let mut expert_world = world(&tiles, Some((1200.0, GROUND - 20.0)));
        expert_world.conditions.expert = true;
        let mut expert_troop = troop(DD2_GOBLIN_BOMBER_T1, 1000.0);
        let mut expert_rng = SmallRng::seed_from_u64(2);
        let expert_thrown: Vec<Shot> = run(
            &mut expert_troop,
            &expert_world,
            &tiles,
            600,
            &mut expert_rng,
        )
        .into_iter()
        .flat_map(|o| o.shots)
        .collect();
        assert!(
            !expert_thrown.is_empty(),
            "expert should still be lobbing bombs"
        );
        assert!(
            expert_thrown[0].damage < thrown[0].damage,
            "expert should hit for less, not the same: {} vs classic {}",
            expert_thrown[0].damage,
            thrown[0].damage
        );

        // The lob arcs: a bomb thrown at something level goes up first.
        assert!(
            thrown.iter().any(|s| s.velocity.1 < 0.0),
            "and the throws arc upward"
        );
    }

    /// It will not throw across the world: a bomber's range is much shorter than a javelinist's.
    #[test]
    fn range_decides_who_can_reach_you() {
        let tiles = floor();
        let w = world(&tiles, Some((1450.0, GROUND - 20.0)));
        let mut rng = SmallRng::seed_from_u64(3);

        let mut bomber = troop(DD2_GOBLIN_BOMBER_T1, 1000.0);
        // Pin it in place so the test is about range, not about walking into range.
        let thrown: usize = run(&mut bomber, &w, &tiles, 60, &mut rng)
            .iter()
            .map(|o| o.shots.len())
            .sum();
        assert_eq!(
            thrown, 0,
            "four hundred and fifty pixels is out of a bomber's reach"
        );

        let mut javelinist = troop(DD2_JAVELINST_T3, 1000.0);
        let thrown: Vec<Shot> = run(&mut javelinist, &w, &tiles, 400, &mut rng)
            .into_iter()
            .flat_map(|o| o.shots)
            .collect();
        assert!(!thrown.is_empty(), "but not out of a javelinist's");
        assert!(thrown.iter().all(|s| s.projectile == JAVELIN_T3));
    }

    /// An ogre picks its attack by how far away you are, and all three come out over a fight.
    #[test]
    fn an_ogre_picks_its_attack_by_range() {
        let tiles = floor();
        let mut rng = SmallRng::seed_from_u64(4);
        let w = world(&tiles, Some((1200.0, GROUND - 20.0)));
        let mut n = troop(DD2_OGRE_T2, 1000.0);
        let mut thrown = std::collections::HashSet::new();
        let mut attacks = std::collections::HashSet::new();
        for _ in 0..4000 {
            for shot in improved_walker(&mut n, &w, &mut rng).shots {
                thrown.insert(shot.projectile);
            }
            crate::game::npc::step_physics(&mut n, &tiles);
            attacks.insert(n.ai[1] as i32);
        }
        assert!(
            attacks.len() > 1,
            "it should not be stuck on one: {attacks:?}"
        );
        assert!(
            thrown.contains(&OGRE_POUND) || thrown.contains(&OGRE_SPIT),
            "and it should be using them: {thrown:?}"
        );
    }

    /// An ogre far away spits rather than swinging at nothing.
    #[test]
    fn a_distant_ogre_spits() {
        let tiles = floor();
        let mut rng = SmallRng::seed_from_u64(5);
        // Beyond its pound range but inside its spit's.
        let w = world(&tiles, Some((1800.0, GROUND - 20.0)));
        let mut n = troop(DD2_OGRE_T2, 1000.0);
        let thrown: Vec<u16> = run(&mut n, &w, &tiles, 600, &mut rng)
            .into_iter()
            .flat_map(|o| o.shots)
            .map(|s| s.projectile)
            .collect();
        assert!(
            thrown.contains(&OGRE_SPIT),
            "it should be spitting: {thrown:?}"
        );
    }

    /// A kobold does not die: it lights its fuse, and reaching you sets it off.
    #[test]
    fn a_kobold_goes_off() {
        let tiles = floor();
        let mut rng = SmallRng::seed_from_u64(6);
        let w = world(&tiles, Some((1100.0, GROUND - 20.0)));
        let mut n = troop(DD2_KOBOLD_WALKER_T2, 1000.0);
        let mut lit = None;
        let mut burst = None;
        for at in 0..2000 {
            let out = improved_walker(&mut n, &w, &mut rng);
            crate::game::npc::step_physics(&mut n, &tiles);
            if lit.is_none() && n.ai[1] == 1.0 {
                lit = Some(at);
            }
            if out.burst {
                burst = Some(at);
                break;
            }
        }
        assert!(lit.is_some(), "the fuse should have caught");
        let burst = burst.expect("and it should have gone off");
        assert!(burst > lit.unwrap(), "in that order");
        assert_eq!((n.width(), n.height()), (KOBOLD_BLAST, KOBOLD_BLAST));
    }

    /// A wither beast stands in an aura, and it feeds itself on it.
    #[test]
    fn a_wither_beast_feeds_on_its_aura() {
        let tiles = floor();
        let mut rng = SmallRng::seed_from_u64(7);
        let w = world(&tiles, Some((1200.0, GROUND - 20.0)));
        let mut n = troop(DD2_WITHER_BEAST_T2, 1000.0);
        n.life = n.life_max / 2;
        n.ai[0] = 1.0;
        let healed: i32 = run(&mut n, &w, &tiles, 400, &mut rng)
            .iter()
            .map(|o| o.healed)
            .sum();
        assert!(healed > 0, "it should have fed");
        assert_eq!(healed, n.life - n.life_max / 2, "and kept what it took");
        assert!(
            run(&mut n, &w, &tiles, 1, &mut rng)[0].aura.is_some(),
            "and the aura should be out"
        );
    }

    /// A skeleton spends two seconds climbing out and cannot be touched while it does.
    #[test]
    fn a_skeleton_climbs_out_of_the_ground() {
        let tiles = floor();
        let mut rng = SmallRng::seed_from_u64(8);
        let w = world(&tiles, Some((1200.0, GROUND - 20.0)));
        let mut n = troop(DD2_SKELETON_T1, 1000.0);
        n.local_ai[3] = 0.0;
        let start = n.position.0;
        for _ in 0..119 {
            improved_walker(&mut n, &w, &mut rng);
            crate::game::npc::step_physics(&mut n, &tiles);
            assert!(n.invulnerable, "nothing can touch it on the way up");
        }
        assert_eq!(n.position.0, start, "and it does not move");
        run(&mut n, &w, &tiles, 60, &mut rng);
        assert!(!n.invulnerable, "and then it is a skeleton like any other");
    }

    /// Stuck against a wall, it shoves itself past rather than standing there forever.
    #[test]
    fn stuck_against_a_wall_it_shoves_past() {
        let mut tiles = floor();
        for y in 180..200 {
            for x in 70..73 {
                tiles.0.insert((x, y), Tile::block(1));
            }
        }
        let mut rng = SmallRng::seed_from_u64(9);
        // Target on the far side of the wall.
        let w = world(&tiles, Some((2000.0, GROUND - 20.0)));
        let mut n = troop(DD2_GOBLIN_T1, 66.0 * 16.0);
        let start = n.position.0;
        run(&mut n, &w, &tiles, 2000, &mut rng);
        assert!(
            n.position.0 > 73.0 * 16.0,
            "it should be past the wall by now: {} from {start}",
            n.position.0
        );
    }

    /// A drakin breathes fire at range rather than closing.
    #[test]
    fn a_drakin_breathes_fire() {
        let tiles = floor();
        let mut rng = SmallRng::seed_from_u64(10);
        let w = world(&tiles, Some((1400.0, GROUND - 20.0)));
        let mut n = troop(DD2_DRAKIN_T3, 1000.0);
        let thrown: Vec<Shot> = run(&mut n, &w, &tiles, 600, &mut rng)
            .into_iter()
            .flat_map(|o| o.shots)
            .collect();
        assert!(!thrown.is_empty(), "it should have breathed");
        assert!(
            thrown
                .iter()
                .all(|s| s.projectile == terrustia_proto::projectile::ids::DRAKIN_FIREBALL)
        );
    }

    /// With nobody to chase, they pace rather than standing still.
    #[test]
    fn with_nobody_to_chase_they_pace() {
        let tiles = floor();
        let mut rng = SmallRng::seed_from_u64(11);
        let w = world(&tiles, None);
        let mut n = troop(DD2_GOBLIN_T1, 1000.0);
        let mut furthest: f32 = 0.0;
        for _ in 0..600 {
            improved_walker(&mut n, &w, &mut rng);
            crate::game::npc::step_physics(&mut n, &tiles);
            furthest = furthest.max((n.position.0 - 1000.0).abs());
        }
        assert!(furthest > 20.0, "it should have wandered: {furthest}");
    }
}
