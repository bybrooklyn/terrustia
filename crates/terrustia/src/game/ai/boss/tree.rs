//! Mourning Wood and the Everscream: style 57.
//!
//! Both walk — through terrain, since neither collides with it — and neither ever closes. They
//! hold a distance, wait five seconds, and throw something; the wait shortens as they are worn
//! down, so the second half of either fight is markedly busier than the first.
//!
//! What they throw is where they differ. Mourning Wood alternates flaming spears fired straight at
//! you with a wave of spheres lobbed over your head, and below a quarter health it gains harder
//! versions of both that the Everscream never gets. The Everscream throws ornaments fast and flat,
//! or pine needles slowly and high.
//!
//! Inside fifty pixels either stops walking entirely, which is what makes standing directly under
//! one a real tactic against the lobbed attacks and a terrible one against the flat ones.
//!
//! Daylight ends both fights: they walk away at eight pixels a tick.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    DESPAWN_ENCOURAGED_TICKS, EVERSCREAM, SCREAM_NEEDLES, SCREAM_ORNAMENTS, TREE_DESPERATE_AT,
    TREE_FLEE, TREE_STEER, TREE_TOO_CLOSE, TREE_WAIT, TREE_WALK, TREE_WALK_HALF, TREE_WALK_HURT,
    TreeAttack, WOOD_DESPERATE_SPEARS, WOOD_DESPERATE_SPHERES, WOOD_SPEARS, WOOD_SPHERES,
};

use crate::game::ai::{PLAYER_HEIGHT, PLAYER_WIDTH, Shot, World, face};
use crate::game::npc::{Npc, TILE, TileView};
use crate::game::npc_ai::Target;

/// What one of them did this tick.
#[derive(Debug, Default)]
pub struct TreeOutcome {
    pub shots: Vec<Shot>,
}

/// Which attack a state number means, for this type.
fn attack_for(npc_type: u16, state: f32) -> Option<TreeAttack> {
    let everscream = npc_type == EVERSCREAM;
    Some(match (state as i32, everscream) {
        (1, false) => WOOD_SPEARS,
        (2, false) => WOOD_SPHERES,
        (3, false) => WOOD_DESPERATE_SPEARS,
        (4, false) => WOOD_DESPERATE_SPHERES,
        (1, true) => SCREAM_ORNAMENTS,
        (2, true) => SCREAM_NEEDLES,
        _ => return None,
    })
}

/// Style 57.
pub fn tree(npc: &mut Npc, world: &World<'_, impl TileView>, rng: &mut SmallRng) -> TreeOutcome {
    let mut out = TreeOutcome::default();
    npc.dirty = true;
    npc.no_gravity = true;
    npc.no_tile_collide = true;

    let health = npc.life as f32 / npc.life_max.max(1) as f32;
    let mut walk = if health < 0.5 {
        TREE_WALK_HALF
    } else if health < 0.75 {
        TREE_WALK_HURT
    } else {
        TREE_WALK
    };
    // Whether it is standing still this tick, either because it is attacking or because you are
    // right on top of it.
    let mut planted = false;

    let Some(target) = world.target.filter(|t| t.alive) else {
        return out;
    };
    if world.conditions.day {
        // Daylight ends it. They walk away rather than fighting. C7-05: vanilla forces the despawn
        // hard here (`NPC.cs:33044`, `EncourageDespawn(10)`), so it is gone within ten ticks of
        // leaving the area, not the six hundred the old cap allowed.
        //
        // This used to also raise a `fleeing` flag, which no caller read. It is deleted rather than
        // wired, because wiring it would be *stronger* than vanilla: neither of these is a boss, so
        // both go through the ordinary despawn countdown, and vanilla's own `CheckActive`
        // (`NPC.cs:78735-78738`) resets `timeLeft` and clears `despawnEncouraged` outright for a
        // player standing nearby. Routing the flag to `expired` would zero the timer instead, and
        // one of these vanishing out of a lit arena is not what the game does. The clamp below is
        // the whole mechanism, and it is applied where vanilla applies it.
        walk = TREE_FLEE;
        npc.time_left = npc.time_left.min(DESPAWN_ENCOURAGED_TICKS);
    } else {
        face(npc, target);
        match npc.ai[0] {
            0.0 => {
                // Waiting. The wait shortens as it is worn down.
                npc.ai[1] += 1.0;
                if health < 0.5 {
                    npc.ai[1] += 1.0;
                }
                if health < 0.25 {
                    npc.ai[1] += 1.0;
                }
                if npc.ai[1] >= TREE_WAIT {
                    npc.ai[1] = 0.0;
                    // Below a quarter, Mourning Wood switches to its two heavier attacks.
                    npc.ai[0] = if health < TREE_DESPERATE_AT && npc.npc_type != EVERSCREAM {
                        rng.random_range(3..5) as f32
                    } else {
                        rng.random_range(1..3) as f32
                    };
                }
            }
            state => match attack_for(npc.npc_type, state) {
                None => {
                    npc.ai[0] = 0.0;
                    npc.ai[1] = 0.0;
                }
                Some(attack) => {
                    // The desperate attacks are thrown on the move; the others stop it dead.
                    planted = state < 3.0;
                    if state >= 3.0 {
                        walk = TREE_WALK_HALF;
                    }
                    npc.ai[1] += 1.0;
                    let due = npc.ai[1] > attack.warmup
                        && (attack.warmup == 0.0 || npc.ai[1] < attack.ticks - 60.0)
                        && npc.ai[1] % attack.every == 0.0;
                    if due {
                        // Aimed at the player's *top*, not their middle. Vanilla takes the X centre
                        // and the raw `position.Y` (`NPC.cs:33087-33088`,
                        // `player.position.X + width * 0.5f` against `player.position.Y`), which is
                        // half a player-height higher than the centre and tilts every one of these
                        // volleys upward. Both of these fight on the ground, so a shot aimed at your
                        // feet rather than your head is a different attack.
                        let aim = (
                            target.center.0,
                            target.center.1 - PLAYER_HEIGHT as f32 / 2.0,
                        );
                        out.shots.push(throw(npc, aim, &attack, rng));
                    }
                    if npc.ai[1] >= attack.ticks {
                        npc.ai[1] = 0.0;
                        npc.ai[0] = 0.0;
                    }
                }
            },
        }
    }

    // Standing right on top of it stops it walking.
    let (cx, _) = npc.center();
    if (cx - target.center.0).abs() < TREE_TOO_CLOSE {
        planted = true;
    }
    if planted {
        npc.velocity.0 *= 0.9;
        if npc.velocity.0.abs() < 0.1 {
            npc.velocity.0 = 0.0;
        }
    } else {
        // It drifts onto its walking speed rather than turning: a twentieth at a time.
        let wanted = walk * f32::from(npc.direction);
        npc.velocity.0 = (npc.velocity.0 * TREE_STEER + wanted) / (TREE_STEER + 1.0);
    }

    // C7-04: its vertical drift, run every tick (`NPC.cs:33274-33323`). The old code wrote only
    // `velocity.0`, so a tree floated at whatever height it spawned at for the whole fight; vanilla
    // hovers a little above the ground beneath it, sinks when there is none, and drops straight
    // down onto a player standing directly under it.
    hover(npc, world, target);
    out
}

/// The tree's vertical drift (`NPC.cs:33274-33323`): drop onto a player directly beneath it,
/// otherwise hover just above whatever ground is under its feet, and sink when there is nothing.
fn hover(npc: &mut Npc, world: &World<'_, impl TileView>, target: Target) {
    let player_left = target.center.0 - PLAYER_WIDTH as f32 / 2.0;
    let player_right = target.center.0 + PLAYER_WIDTH as f32 / 2.0;
    let player_bottom = target.center.1 + PLAYER_HEIGHT as f32 / 2.0;
    let feet = npc.position.1 + npc.height();
    // The player is within the trunk's own width and standing below its feet: come down on them.
    let on_player = npc.position.0 < player_left
        && npc.position.0 + npc.width() > player_right
        && feet < player_bottom - 16.0;
    if on_player {
        npc.velocity.1 += 0.5;
    } else if ground_beneath(world.tiles, npc) {
        if npc.velocity.1 > 0.0 {
            npc.velocity.1 = 0.0;
        }
        if npc.velocity.1 > -0.2 {
            npc.velocity.1 -= 0.025;
        } else {
            npc.velocity.1 -= 0.2;
        }
        npc.velocity.1 = npc.velocity.1.max(-4.0);
    } else {
        if npc.velocity.1 < 0.0 {
            npc.velocity.1 = 0.0;
        }
        if npc.velocity.1 < 0.1 {
            npc.velocity.1 += 0.025;
        } else {
            npc.velocity.1 += 0.5;
        }
    }
    npc.velocity.1 = npc.velocity.1.min(10.0);
}

/// Whether any solid block overlaps the 80x20-pixel box just below the tree's feet, the box vanilla
/// tests with `Collision.SolidCollision` (`NPC.cs:33276,33286`). Narrowed to solid blocks: vanilla
/// also catches platforms and slopes, which are not modelled here.
fn ground_beneath(tiles: &impl TileView, npc: &Npc) -> bool {
    let left = npc.center().0 - 40.0;
    let top = npc.position.1 + npc.height() - 20.0;
    let x0 = (left / TILE).floor() as i32;
    let x1 = ((left + 80.0) / TILE).floor() as i32;
    let y0 = (top / TILE).floor() as i32;
    let y1 = ((top + 20.0) / TILE).floor() as i32;
    (x0..=x1).any(|tx| {
        (y0..=y1).any(|ty| {
            let tile = tiles.tile(tx, ty);
            tile.is_active() && terrustia_proto::tile_solid::solid(tile.block)
        })
    })
}

/// One thrown projectile: aimed at the player, lifted into an arc, and scattered.
fn throw(npc: &Npc, player: (f32, f32), attack: &TreeAttack, rng: &mut SmallRng) -> Shot {
    let from = (
        npc.position.0 + npc.width() * 0.5,
        npc.position.1 + npc.height() * 0.5 + attack.from,
    );
    let mut across = player.0 - from.0;
    let mut rise = player.1 - from.1;
    // The lobbed attacks aim above you by a fraction of how far off you are, and gain speed with
    // distance — which is what makes them land on you rather than short.
    let mut speed = attack.speed + across.abs() * attack.reach_gain;
    if speed > attack.speed_cap {
        speed = attack.speed_cap;
    }
    if attack.arc > 0.0 {
        rise -= across.abs() * attack.arc;
        across += rng.random_range(-50..=50) as f32;
        rise -= rng.random_range(50..201) as f32;
    }
    // C7-06: the Everscream's ornaments jitter their aim point up to `scatter` pixels either way on
    // both axes and take a small random upward loft (`NPC.cs:33088-33090`), so a volley lands spread
    // out rather than flat and stacked on one line. The loft reads `across` after the scatter, the
    // way vanilla does (it is a fraction of the already-jittered horizontal gap).
    if attack.scatter > 0 {
        across += rng.random_range(-attack.scatter..=attack.scatter) as f32;
        rise += rng.random_range(-attack.scatter..=attack.scatter) as f32;
    }
    if attack.loft > 0 {
        rise -= across.abs() * (rng.random_range(0..=attack.loft) as f32 * 0.01);
    }
    let length = across.hypot(rise).max(f32::MIN_POSITIVE);
    let mut velocity = (across / length * speed, rise / length * speed);
    let jitter = |rng: &mut SmallRng| {
        1.0 + rng.random_range(-attack.spread..=attack.spread) as f32 * attack.spread_scale
    };
    velocity.0 *= jitter(rng);
    velocity.1 *= jitter(rng);

    // A few attacks pick from a run of projectile ids rather than using one.
    let projectile = if attack.projectile_span > 1 {
        attack.projectile + rng.random_range(0..attack.projectile_span)
    } else {
        attack.projectile
    };
    Shot {
        projectile,
        damage: attack.damage,
        position: from,
        velocity,
        time_left: 600,
        ai: [0.0; 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ai::Conditions;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::MOURNING_WOOD;
    use terrustia_proto::tile::Tile;

    struct Night(HashMap<(i32, i32), Tile>);

    impl TileView for Night {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn night(tiles: &Night, target: Option<(f32, f32)>) -> World<'_, Night> {
        let mut w = crate::game::ai::calm(
            tiles,
            target.map(|center| Target {
                slot: 0,
                center,
                velocity: (0.0, 0.0),
                alive: true,
            }),
        );
        w.conditions = Conditions {
            day: false,
            ..Conditions::default()
        };
        w
    }

    fn boss(npc_type: u16, x: f32, y: f32) -> Npc {
        Npc::new(npc_type, (x, y), 1).expect("a moon boss")
    }

    /// It throws at the player's head, not their middle.
    ///
    /// Vanilla aims at the X centre and the raw `position.Y` (`NPC.cs:33087-33088`), which is half
    /// a player-height above the centre this used. Both of these fight on the ground, so a volley
    /// aimed at your feet rather than your head is a different attack. The aim carries a `+/-50`
    /// jitter and a downward tilt with range, so this puts the boss level with the player and
    /// checks the whole volley averages above the centre rather than pinning one shot.
    #[test]
    fn it_aims_at_the_players_top_not_their_middle() {
        let tiles = Night(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(11);
        let mut t = boss(MOURNING_WOOD, 0.0, 0.0);
        let (cx, cy) = t.center();
        // Level with it and close, so the aim's own range tilt does not swamp the offset.
        let w = night(&tiles, Some((cx + 200.0, cy)));

        let mut rise = Vec::new();
        for _ in 0..2000 {
            let out = tree(&mut t, &w, &mut rng);
            // Hold it still: this is about where it throws, not where it walks.
            t.position = (0.0, 0.0);
            t.velocity = (0.0, 0.0);
            rise.extend(out.shots.into_iter().map(|s| s.velocity.1));
        }
        assert!(!rise.is_empty(), "it should have thrown something");
        let mean = rise.iter().sum::<f32>() / rise.len() as f32;
        assert!(
            mean < 0.0,
            "aimed at a player level with it, the volley should rise, got {mean}"
        );
    }

    /// It waits, then throws, and the wait shortens as it is worn down.
    #[test]
    fn a_worn_down_tree_attacks_more_often() {
        let tiles = Night(HashMap::new());
        let w = night(&tiles, Some((600.0, 0.0)));
        let attacks = |health: f32| {
            let mut rng = SmallRng::seed_from_u64(57);
            let mut t = boss(MOURNING_WOOD, 0.0, 0.0);
            t.life = (t.life_max as f32 * health) as i32;
            let mut count = 0;
            let mut was = t.ai[0];
            for _ in 0..3000 {
                tree(&mut t, &w, &mut rng);
                if was == 0.0 && t.ai[0] != 0.0 {
                    count += 1;
                }
                was = t.ai[0];
            }
            count
        };
        assert!(
            attacks(0.2) > attacks(1.0),
            "a hurt one should attack more: {} vs {}",
            attacks(0.2),
            attacks(1.0)
        );
    }

    /// Mourning Wood gains two attacks below a quarter health; the Everscream never does.
    #[test]
    fn only_mourning_wood_gets_desperate() {
        let tiles = Night(HashMap::new());
        let w = night(&tiles, Some((600.0, 0.0)));
        let states = |npc_type: u16| {
            let mut rng = SmallRng::seed_from_u64(3);
            let mut t = boss(npc_type, 0.0, 0.0);
            t.life = t.life_max / 10;
            let mut seen = std::collections::HashSet::new();
            for _ in 0..6000 {
                tree(&mut t, &w, &mut rng);
                if t.ai[0] != 0.0 {
                    seen.insert(t.ai[0] as i32);
                }
            }
            seen
        };
        let wood = states(MOURNING_WOOD);
        assert!(
            wood.contains(&3) || wood.contains(&4),
            "Mourning Wood should get its heavier attacks: {wood:?}"
        );
        let scream = states(EVERSCREAM);
        assert!(
            !scream.contains(&3) && !scream.contains(&4),
            "the Everscream should not: {scream:?}"
        );
    }

    /// The two of them throw different things.
    #[test]
    fn each_throws_its_own_ammunition() {
        let tiles = Night(HashMap::new());
        let w = night(&tiles, Some((600.0, 0.0)));
        let thrown = |npc_type: u16| {
            let mut rng = SmallRng::seed_from_u64(5);
            let mut t = boss(npc_type, 0.0, 0.0);
            let mut ids = std::collections::HashSet::new();
            for _ in 0..4000 {
                for shot in tree(&mut t, &w, &mut rng).shots {
                    ids.insert(shot.projectile);
                }
            }
            ids
        };
        let wood = thrown(MOURNING_WOOD);
        let scream = thrown(EVERSCREAM);
        assert!(!wood.is_empty() && !scream.is_empty(), "both should throw");
        assert!(
            wood.is_disjoint(&scream),
            "and never the same thing: {wood:?} vs {scream:?}"
        );
    }

    /// C7-06: the Everscream's ornaments scatter and loft rather than flying flat. Thrown at a
    /// player level with the throw point, the old flat volley sent every ornament dead horizontal
    /// (`velocity.1 == 0`); the +/-50 scatter and 0-20% loft (`NPC.cs:33088-33090`) give each one
    /// its own rise, so the volley fans out and, on average, arcs upward. Zeroing `scatter`/`loft`
    /// collapses the fan and fails this.
    #[test]
    fn everscream_ornaments_scatter_and_loft() {
        let npc = boss(EVERSCREAM, 0.0, 0.0);
        let attack = SCREAM_ORNAMENTS;
        // Level with the throw origin (`center.y + attack.from`) and well to the side, so a flat
        // throw would carry no vertical velocity at all.
        let player = (npc.center().0 + 500.0, npc.center().1 + attack.from);
        let mut rng = SmallRng::seed_from_u64(11);
        let rises: Vec<f32> = (0..300)
            .map(|_| throw(&npc, player, &attack, &mut rng).velocity.1)
            .collect();
        let max = rises.iter().copied().fold(f32::MIN, f32::max);
        let min = rises.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            max - min > 1.0,
            "ornaments fan out vertically rather than flying flat: {min}..{max}"
        );
        assert!(
            max > 0.0 && min < 0.0,
            "the scatter throws some up and some down: {min}..{max}"
        );
        let mean = rises.iter().sum::<f32>() / rises.len() as f32;
        assert!(
            mean < 0.0,
            "and the loft biases the volley upward on average: {mean}"
        );
    }

    /// Standing right under one stops it walking.
    #[test]
    fn standing_under_one_pins_it() {
        let tiles = Night(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(7);
        let mut t = boss(MOURNING_WOOD, 0.0, 0.0);
        t.velocity.0 = 3.0;
        let (cx, cy) = t.center();
        let w = night(&tiles, Some((cx + 10.0, cy)));

        for _ in 0..60 {
            tree(&mut t, &w, &mut rng);
        }
        assert_eq!(t.velocity.0, 0.0, "it should have stopped");
    }

    /// C7-04: it moves vertically. The old code wrote only `velocity.0`, so a tree hung at its
    /// spawn height for the whole fight. Vanilla (`NPC.cs:33274-33323`) sinks when there is nothing
    /// beneath it, rises rather than sinking through ground just under its feet, and drops straight
    /// down onto a player standing directly under it. On the pre-fix code `velocity.1` never moved.
    #[test]
    fn a_tree_moves_vertically() {
        // A ground band around tile y = 40 (pixels 640..704).
        let mut cells = HashMap::new();
        for x in -100..100 {
            for y in 40..44 {
                cells.insert((x, y), Tile::block(1));
            }
        }
        let tiles = Night(cells);
        // A player far off to the side, so the descend-on-player rule never fires.
        let far = night(&tiles, Some((9000.0, 0.0)));
        let mut r = SmallRng::seed_from_u64(1);

        // Hanging in open air well above the floor: nothing under its feet, so it sinks.
        let mut hanging = boss(MOURNING_WOOD, 0.0, 0.0);
        tree(&mut hanging, &far, &mut r);
        assert!(
            hanging.velocity.1 > 0.0,
            "with nothing beneath it, it should sink: {}",
            hanging.velocity.1
        );

        // Sitting with the floor just under its feet: it rises rather than sinking through.
        let mut resting = boss(MOURNING_WOOD, 0.0, 40.0 * 16.0 - 154.0);
        tree(&mut resting, &far, &mut r);
        assert!(
            resting.velocity.1 <= 0.0,
            "on the ground it should not keep sinking: {}",
            resting.velocity.1
        );

        // A player standing directly beneath it: it comes down on them.
        let mut over = boss(MOURNING_WOOD, 0.0, 0.0);
        let (ox, _) = over.center();
        let feet = over.position.1 + over.height();
        let below = night(&tiles, Some((ox, feet + 200.0)));
        tree(&mut over, &below, &mut r);
        assert!(
            over.velocity.1 >= 0.5,
            "it should drop onto a player standing under it: {}",
            over.velocity.1
        );
    }

    /// Daylight sends it away rather than letting the fight run on.
    #[test]
    fn daylight_sends_it_off() {
        let tiles = Night(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(9);
        let mut t = boss(MOURNING_WOOD, 0.0, 0.0);
        let mut w = night(&tiles, Some((6000.0, 0.0)));
        w.conditions.day = true;

        let out = tree(&mut t, &w, &mut rng);
        assert!(out.shots.is_empty(), "it stops fighting");
        assert_eq!(
            t.time_left, DESPAWN_ENCOURAGED_TICKS,
            "and is on its way out (`NPC.cs:33044`, `EncourageDespawn(10)`)"
        );
        for _ in 0..200 {
            tree(&mut t, &w, &mut rng);
        }
        assert!(
            t.velocity.0.abs() > TREE_WALK_HALF,
            "and leaves quickly: {}",
            t.velocity.0
        );
    }
}
