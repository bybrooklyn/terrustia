//! Style 109: the Dark Mage.
//!
//! It floats rather than walks, and it has three spells that it works through in order: a bolt at
//! you, a healing sigil planted on the ground beside it, and a summoning that raises the goblins
//! you have already killed as skeletons.
//!
//! The order is fixed but the choice is not. Before each cast it looks around: if there are not at
//! least two hurt things nearby the heal is pointless, so it goes straight to raising instead —
//! and if there are no corpses to raise it falls back to the bolt. It will not throw the bolt at
//! all past a thousand pixels or through a wall. So a mage left alone with a field of dead goblins
//! spends the whole fight resurrecting them, and one caught in the open does nothing but shoot.

use terrustia_proto::npc_params::{
    ARMY_FADE_IN, DARK_MAGE_BOLT_AT, DARK_MAGE_BOLT_DAMAGE, DARK_MAGE_BOLT_RANGE,
    DARK_MAGE_BOLT_SPEED, DARK_MAGE_CASTS, DARK_MAGE_COOLDOWN, DARK_MAGE_HEAL_AT,
    DARK_MAGE_HEAL_OUT, DARK_MAGE_RAISE_AT, DARK_MAGE_SHORT_COOLDOWN,
};
use terrustia_proto::projectile::ids::{DARK_MAGE_BOLT, DARK_MAGE_HEAL, DARK_MAGE_PORTAL};

use crate::game::ai::{Shot, World};
use crate::game::npc::{Npc, TileView};

/// Which spell it is on, as `ai[1]` numbers it.
mod spell {
    pub const BOLT: f32 = 0.0;
    pub const HEAL: f32 = 1.0;
    pub const RAISE: f32 = 2.0;
}

/// What it did this tick.
#[derive(Debug, Default)]
pub struct MageOutcome {
    pub shots: Vec<Shot>,
    /// Set on the tick it finishes a summoning, so the caller can raise what is lying about.
    pub raising: bool,
}

/// What the mage needs to know about its surroundings to choose a spell.
#[derive(Debug, Clone, Copy, Default)]
pub struct MageView {
    /// How many hurt things are within its healing reach.
    pub wounded: usize,
    /// Whether there are enough corpses nearby to be worth raising.
    pub can_raise: bool,
}

pub fn dark_mage(npc: &mut Npc, world: &World<'_, impl TileView>, around: MageView) -> MageOutcome {
    let mut out = MageOutcome::default();
    npc.dirty = true;
    npc.rotation = npc.velocity.0 * 0.04;
    npc.sprite_direction = npc.direction;

    // It comes out of its gate faded, and will not cast until it has finished arriving.
    if npc.local_ai[3] < ARMY_FADE_IN {
        npc.local_ai[3] += 1.0;
        npc.alpha = (255 - (npc.local_ai[3] as i32 * 5)).max(0);
    }

    // `ai[0]` is one counter running two ways: negative is the cooldown, positive is a cast in
    // progress. Zero is the only moment it can start something new.
    if npc.ai[0] < 0.0 {
        npc.ai[0] = (npc.ai[0] + 1.0).min(0.0);
    }

    let casting = npc.ai[0] > 0.0;
    if casting {
        npc.ai[0] -= 1.0;
        cast(npc, world, &mut out);
        if npc.ai[0] <= 0.0 {
            // Done: on to the next spell, with a cooldown that is shorter after the bolt.
            let finished = npc.ai[1];
            npc.ai[1] += 1.0;
            if npc.ai[1] >= 3.0 {
                npc.ai[1] = 0.0;
            }
            npc.ai[0] = if finished == spell::BOLT {
                -DARK_MAGE_SHORT_COOLDOWN
            } else {
                -DARK_MAGE_COOLDOWN
            };
        }
        // Casting, it holds still.
        npc.velocity.0 *= 0.9;
        npc.velocity.1 *= 0.9;
        return out;
    }

    if npc.ai[0] == 0.0 && npc.local_ai[3] >= ARMY_FADE_IN {
        choose(npc, world, around);
        if npc.ai[0] > 0.0 {
            npc.velocity.0 *= 0.9;
            npc.velocity.1 *= 0.9;
            return out;
        }
    }

    drift(npc, world);
    out
}

/// Pick a spell, or none. Every one of these checks can veto the cast entirely.
fn choose(npc: &mut Npc, world: &World<'_, impl TileView>, around: MageView) {
    // Fewer than two hurt things nearby means healing would be wasted: raise instead.
    if around.wounded < 2 {
        npc.ai[1] = spell::RAISE;
    }
    // ...and with nothing to raise, fall back to the bolt.
    if npc.ai[1] == spell::RAISE && !around.can_raise {
        npc.ai[1] = spell::BOLT;
    }
    // The bolt needs a target it can actually reach.
    if npc.ai[1] == spell::BOLT {
        let reachable = world.target.filter(|t| t.alive).is_some_and(|t| {
            let (cx, cy) = npc.center();
            (t.center.0 - cx).hypot(t.center.1 - cy) < DARK_MAGE_BOLT_RANGE
                && crate::game::ai::can_see(world.tiles, npc, t)
        });
        if !reachable {
            return;
        }
    }
    npc.ai[0] = DARK_MAGE_CASTS[(npc.ai[1] as usize).min(2)];
}

/// The moment in a cast when the spell actually goes off.
fn cast(npc: &mut Npc, world: &World<'_, impl TileView>, out: &mut MageOutcome) {
    let (cx, cy) = npc.center();
    match npc.ai[1] {
        spell::BOLT if npc.ai[0] == DARK_MAGE_BOLT_AT => {
            let Some(t) = world.target.filter(|t| t.alive) else {
                return;
            };
            let from = (cx + f32::from(npc.direction) * 10.0, cy - 16.0);
            let aim = (t.center.0 - from.0, t.center.1 - from.1);
            let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
            let velocity = (
                aim.0 / length * DARK_MAGE_BOLT_SPEED,
                aim.1 / length * DARK_MAGE_BOLT_SPEED,
            );
            // It turns to face the shot as it takes it, not before.
            npc.direction = if velocity.0 > 0.0 { 1 } else { -1 };
            out.shots.push(Shot {
                projectile: DARK_MAGE_BOLT,
                damage: DARK_MAGE_BOLT_DAMAGE,
                position: (cx + f32::from(npc.direction) * 10.0, cy - 16.0),
                velocity,
                time_left: 600,
                ai: [0.0; 3],
            });
        }
        spell::HEAL if DARK_MAGE_HEAL_AT.contains(&npc.ai[0]) => {
            // Three sigils, planted on whatever floor is under a point beside it.
            let beside = cx + f32::from(npc.direction) * DARK_MAGE_HEAL_OUT;
            if let Some(floor) = floor_under(world, beside, cy) {
                out.shots.push(Shot {
                    projectile: DARK_MAGE_HEAL,
                    damage: 0,
                    position: (beside, floor),
                    velocity: (0.0, 0.0),
                    time_left: 600,
                    ai: [0.0; 3],
                });
            }
        }
        spell::RAISE if npc.ai[0] == DARK_MAGE_RAISE_AT => {
            out.shots.push(Shot {
                projectile: DARK_MAGE_PORTAL,
                damage: 0,
                position: (cx + f32::from(npc.direction) * 24.0, cy - 40.0),
                velocity: (0.0, 0.0),
                time_left: 600,
                ai: [0.0; 3],
            });
            out.raising = true;
        }
        _ => {}
    }
}

/// The first solid tile below a point, within fifty tiles.
fn floor_under(world: &World<'_, impl TileView>, x: f32, y: f32) -> Option<f32> {
    let tx = (x / 16.0) as i32;
    let from = (y / 16.0) as i32;
    (from..from + 50).find_map(|ty| {
        let tile = world.tiles.tile(tx, ty);
        (tile.is_active() && terrustia_proto::tile_solid::solid(tile.block))
            .then_some(ty as f32 * 16.0)
    })
}

/// Between casts it drifts: it keeps a few tiles of air below, climbs over what is in front of it,
/// and bounces off anything it walks into.
fn drift(npc: &mut Npc, world: &World<'_, impl TileView>) {
    const MAX_X: f32 = 0.5;
    const ACCEL_X: f32 = 0.1;
    const MAX_Y: f32 = 0.5;
    const ACCEL_Y: f32 = 0.02;
    const SINK: f32 = 0.05;
    const SINK_CAP: f32 = 0.2;
    const CLIMB: f32 = -0.05;
    const CLIMB_CAP: f32 = -0.4;

    let ahead = ((npc.position.0 + npc.width() / 2.0) / 16.0) as i32 + i32::from(npc.direction) * 2;
    let below = ((npc.position.1 + npc.height()) / 16.0) as i32;
    let blocked = |x: i32, y: i32| {
        let tile = world.tiles.tile(x, y);
        (tile.is_active() && terrustia_proto::tile_solid::solid(tile.block)) || tile.liquid > 0
    };

    // Four tiles of air below is what it wants; anything within two makes it climb.
    let mut open = true;
    let mut low = false;
    for y in below..below + 4 {
        if blocked(ahead, y) {
            low = y <= below + 1;
            open = false;
            break;
        }
    }
    // Two more directly beneath its feet.
    let (fx, fy) = (
        (npc.position.0 + npc.width() / 2.0) as i32 / 16,
        (npc.position.1 + npc.height()) as i32 / 16,
    );
    for y in fy..fy + 2 {
        if blocked(fx, y) {
            low = true;
            open = false;
            break;
        }
    }

    if open {
        npc.velocity.1 = (npc.velocity.1 + SINK).min(SINK_CAP);
    } else {
        if (npc.direction_y < 0 && npc.velocity.1 > 0.0) || low {
            npc.velocity.1 += CLIMB;
        }
        npc.velocity.1 = npc.velocity.1.max(CLIMB_CAP);
    }

    // Walls turn it round rather than stopping it.
    if npc.collide_x {
        npc.velocity.0 = npc.old_velocity.0 * -0.4;
        if npc.direction == -1 && (0.0..1.0).contains(&npc.velocity.0) {
            npc.velocity.0 = 1.0;
        }
        if npc.direction == 1 && (-1.0..0.0).contains(&npc.velocity.0) {
            npc.velocity.0 = -1.0;
        }
    }
    if npc.collide_y {
        npc.velocity.1 = npc.old_velocity.1 * -0.25;
        if (0.0..1.0).contains(&npc.velocity.1) {
            npc.velocity.1 = 1.0;
        }
        if (-1.0..0.0).contains(&npc.velocity.1) {
            npc.velocity.1 = -1.0;
        }
    }

    approach(&mut npc.velocity.0, npc.direction, MAX_X, ACCEL_X);
    approach(&mut npc.velocity.1, npc.direction_y, MAX_Y, ACCEL_Y);
}

/// Ease one axis toward a cruising speed in the direction it wants to go, with the game's own
/// asymmetry: turning round is faster than speeding up.
fn approach(v: &mut f32, direction: i8, max: f32, accel: f32) {
    if direction == -1 && *v > -max {
        *v -= accel;
        if *v > max {
            *v -= accel;
        } else if *v > 0.0 {
            *v += accel / 2.0;
        }
        *v = v.max(-max);
    } else if direction == 1 && *v < max {
        *v += accel;
        if *v < -max {
            *v += accel;
        } else if *v < 0.0 {
            *v -= accel / 2.0;
        }
        *v = v.min(max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::DD2_DARK_MAGE_T1;
    use terrustia_proto::tile::Tile;

    struct Cave(HashMap<(i32, i32), Tile>);

    impl TileView for Cave {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn ground() -> Cave {
        let mut tiles = HashMap::new();
        for x in 0..400 {
            for y in 200..210 {
                tiles.insert((x, y), Tile::block(1));
            }
        }
        Cave(tiles)
    }

    fn world<'a>(tiles: &'a Cave, target: Option<(f32, f32)>) -> World<'a, Cave> {
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

    fn mage() -> Npc {
        let mut n = Npc::new(DD2_DARK_MAGE_T1, (2000.0, 3100.0), 1).expect("a dark mage");
        // Already arrived, so it will cast.
        n.local_ai[3] = ARMY_FADE_IN;
        n
    }

    fn run(
        n: &mut Npc,
        w: &World<'_, Cave>,
        tiles: &Cave,
        around: MageView,
        ticks: i32,
    ) -> Vec<MageOutcome> {
        (0..ticks)
            .map(|_| {
                let out = dark_mage(n, w, around);
                n.no_gravity = true;
                crate::game::npc::step_physics(n, tiles);
                out
            })
            .collect()
    }

    /// A mage with wounded allies and corpses to raise works through all three spells.
    #[test]
    fn it_works_through_all_three_spells() {
        let tiles = ground();
        let w = world(&tiles, Some((2100.0, 3100.0)));
        let mut n = mage();
        let around = MageView {
            wounded: 3,
            can_raise: true,
        };
        let mut thrown = std::collections::HashSet::new();
        let mut raised = 0;
        for out in run(&mut n, &w, &tiles, around, 2000) {
            for shot in out.shots {
                thrown.insert(shot.projectile);
            }
            raised += usize::from(out.raising);
        }
        assert!(thrown.contains(&DARK_MAGE_BOLT), "the bolt");
        assert!(thrown.contains(&DARK_MAGE_HEAL), "the heal");
        assert!(thrown.contains(&DARK_MAGE_PORTAL), "and the summoning");
        assert!(raised > 0, "which actually raises something");
    }

    /// With nobody hurt nearby, healing is pointless and it goes straight to raising.
    #[test]
    fn it_will_not_heal_a_healthy_field() {
        let tiles = ground();
        let w = world(&tiles, Some((2100.0, 3100.0)));
        let mut n = mage();
        let around = MageView {
            wounded: 0,
            can_raise: true,
        };
        let mut thrown = std::collections::HashSet::new();
        for out in run(&mut n, &w, &tiles, around, 3000) {
            for shot in out.shots {
                thrown.insert(shot.projectile);
            }
        }
        assert!(
            !thrown.contains(&DARK_MAGE_HEAL),
            "nothing hurt: no point healing"
        );
        assert!(thrown.contains(&DARK_MAGE_PORTAL), "it raises instead");
    }

    /// With nothing to raise either, it falls all the way back to the bolt.
    #[test]
    fn with_nothing_to_raise_it_only_shoots() {
        let tiles = ground();
        let w = world(&tiles, Some((2100.0, 3100.0)));
        let mut n = mage();
        let around = MageView {
            wounded: 0,
            can_raise: false,
        };
        let mut thrown = std::collections::HashSet::new();
        for out in run(&mut n, &w, &tiles, around, 3000) {
            for shot in out.shots {
                thrown.insert(shot.projectile);
            }
        }
        assert_eq!(
            thrown,
            std::collections::HashSet::from([DARK_MAGE_BOLT]),
            "the bolt and only the bolt"
        );
    }

    /// It will not throw its bolt across the world, nor through a wall.
    #[test]
    fn the_bolt_needs_a_clear_shot() {
        let tiles = ground();
        let far = world(&tiles, Some((2000.0 + 2000.0, 3100.0)));
        let mut n = mage();
        let around = MageView {
            wounded: 0,
            can_raise: false,
        };
        let shots: usize = run(&mut n, &far, &tiles, around, 2000)
            .iter()
            .map(|o| o.shots.len())
            .sum();
        assert_eq!(shots, 0, "two thousand pixels is out of range");

        // A wall between them, well inside range and too big to drift around.
        let mut walled = ground();
        for x in 128..132 {
            for y in 150..210 {
                walled.0.insert((x, y), Tile::block(1));
            }
        }
        let near = world(&walled, Some((2100.0, 3100.0)));
        let mut n = mage();
        let shots: usize = run(&mut n, &near, &walled, around, 2000)
            .iter()
            .map(|o| o.shots.len())
            .sum();
        assert_eq!(shots, 0, "and it will not shoot through stone");
    }

    /// It holds still while casting and drifts when it is not.
    #[test]
    fn it_stops_to_cast() {
        let tiles = ground();
        let w = world(&tiles, Some((2100.0, 3100.0)));
        let mut n = mage();
        let around = MageView {
            wounded: 3,
            can_raise: true,
        };
        let mut while_casting = f32::MIN;
        let mut while_free = f32::MIN;
        for _ in 0..2000 {
            dark_mage(&mut n, &w, around);
            n.no_gravity = true;
            crate::game::npc::step_physics(&mut n, &tiles);
            let speed = n.velocity.0.hypot(n.velocity.1);
            if n.ai[0] > 0.0 {
                while_casting = while_casting.max(speed);
            } else {
                while_free = while_free.max(speed);
            }
        }
        assert!(
            while_free > while_casting,
            "{while_free} vs {while_casting}"
        );
    }

    /// The cooldown after the bolt really is the short one.
    #[test]
    fn the_bolt_has_a_shorter_cooldown() {
        let tiles = ground();
        let w = world(&tiles, Some((2100.0, 3100.0)));
        let mut n = mage();
        n.ai[1] = spell::BOLT;
        n.ai[0] = 1.0;
        dark_mage(&mut n, &w, MageView::default());
        assert_eq!(n.ai[0], -DARK_MAGE_SHORT_COOLDOWN);

        let mut n = mage();
        n.ai[1] = spell::RAISE;
        n.ai[0] = 1.0;
        dark_mage(&mut n, &w, MageView::default());
        assert_eq!(n.ai[0], -DARK_MAGE_COOLDOWN);
    }
}
