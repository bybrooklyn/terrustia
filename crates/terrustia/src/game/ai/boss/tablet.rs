//! The cultist tablet and its devotes: style 83.
//!
//! The tablet is not a fight. It is a trigger: four cultists — two archers and two devotes — gather
//! at it and kneel, and while any of them lives nothing happens. Kill all four and the tablet
//! spends five seconds shattering, throwing shards, and the Lunatic Cultist rises where it stood.
//!
//! A devote does almost nothing: it paces, turns to face the tablet, and marks the tablet when it
//! is struck. Its only real job is to be in the way.

use terrustia_proto::npc_params::{
    CULTIST_ARCHER, CULTIST_DEVOTE, DEVOTE_DRAG, TABLET_CULTISTS, TABLET_SHARD_EVERY,
    TABLET_SHARD_FROM, TABLET_SHATTER_TICKS,
};
use terrustia_proto::projectile::ids::TABLET_SHARD;

use super::skeletron::Parent;
use crate::game::ai::{Shot, World};
use crate::game::npc::{Npc, TileView};
use crate::game::npc_ai::Spawn;

/// What the tablet or one of its attendants did this tick.
#[derive(Debug, Default)]
pub struct TabletOutcome {
    pub shots: Vec<Shot>,
    pub spawn: Vec<Spawn>,
    pub spent: bool,
    /// Set on the tick the tablet *starts* breaking, which is when the Cultist is raised.
    ///
    /// Vanilla creates him in the same statement that flips the tablet into its shatter
    /// (`NPC.cs:37179-37205`), so he stands there for the whole three seconds it takes to come
    /// apart and the shards converge on him. This used to fire at the *end* of the shatter, which
    /// is three seconds late and left `aiStyle 98` with nothing to aim at.
    pub ritual_complete: bool,
}

/// Style 83, for the tablet itself.
///
/// `attendants` is how many of its four cultists are still alive.
pub fn tablet(npc: &mut Npc, world: &World<'_, impl TileView>, attendants: usize) -> TabletOutcome {
    let mut out = TabletOutcome::default();
    npc.dirty = true;
    npc.invulnerable = true;

    // It calls its four the first time it runs: two archers and two devotes.
    if npc.local_ai[3] == 0.0 {
        npc.local_ai[3] = 1.0;
        let (cx, cy) = npc.center();
        for i in 0..TABLET_CULTISTS {
            // The middle two kneel; the outer two stand back and shoot.
            let devote = i == 1 || i == 2;
            let across = (i as f32 - 1.5) * 90.0;
            out.spawn.push(Spawn {
                handle: None,
                npc_type: if devote {
                    CULTIST_DEVOTE
                } else {
                    CULTIST_ARCHER
                },
                position: (cx + across, cy - 48.0),
                velocity: (0.0, 0.0),
                parent: Some(Spawn::OWN_PARENT),
                ai: [None; 4],
            });
        }
        return out;
    }

    // While any of them lives, nothing happens.
    if npc.ai[0] != -1.0 {
        if attendants > 0 {
            return out;
        }
        // All three, as `NPC.cs:37183-37185` writes them. `ai[1]` is read by nothing on this
        // server, but it is not a dead write: vanilla keeps its attendants' handles in `ai[0..2]`
        // and clearing them is part of the same statement, and every `ai` slot is synced, so a
        // client watching the tablet break sees the numbers the game would have sent.
        npc.ai[0] = -1.0;
        npc.ai[1] = 0.0;
        npc.ai[3] = 0.0;
        // **The Cultist is raised here, not at the end.** Vanilla creates him in the same
        // statement that flips `ai[0]` to -1 and records his handle in `ai[2]`
        // (`NPC.cs:37179-37205`), so he is standing there for the whole three seconds the tablet
        // takes to break - and the shards that come off it converge on *him*. This used to report
        // the ritual complete only once the shatter finished, which left the shards with nothing
        // to fly at for their entire flight and is why `aiStyle 98` could not be written.
        out.ritual_complete = true;
    }

    // Shattering. Shards come off it for the last three seconds, and then it is gone.
    npc.ai[3] += 1.0;
    if npc.ai[3] > TABLET_SHARD_FROM && npc.ai[3] % TABLET_SHARD_EVERY == 1.0 {
        let (cx, cy) = npc.center();
        // Thrown outward on the angle its own tick number picks, so the spray is even rather than
        // random — the tablet breaks the same way every time.
        let angle = npc.ai[3] * 0.7;
        let (sin, cos) = angle.sin_cos();
        out.shots.push(Shot {
            projectile: TABLET_SHARD,
            damage: 0,
            position: (cx + sin * 25.0, cy + cos * 25.0),
            velocity: (sin * 6.0, cos * 6.0),
            // Zero, meaning the type's own 120. `aiStyle 98` ends a shard when it arrives, which
            // is well inside that; the 300 here was invented and outlived the convergence.
            time_left: 0,
            // Vanilla passes the Cultist's *centre* here, read out of the tablet's own `ai[2]`
            // (`NPC.cs:37225`). This routine has no NPC table to read it from, so the shard
            // recovers the point on its first tick and writes it into these slots itself - the
            // same trade the Dryad's ward makes, and for the same reason: they are synced, and a
            // client running this arm against two zeroes would converge the spray on the world's
            // top-left corner.
            ai: [0.0; 3],
        });
    }
    if npc.ai[3] > TABLET_SHATTER_TICKS {
        out.spent = true;
    }
    let _ = world;
    out
}

/// Style 83, for a devote.
///
/// It paces and turns to face the tablet, and dies with it.
pub fn devote(npc: &mut Npc, tablet: Option<Parent>) -> TabletOutcome {
    let mut out = TabletOutcome::default();
    npc.dirty = true;

    npc.velocity.0 *= DEVOTE_DRAG;
    if npc.velocity.0.abs() < 0.1 {
        npc.velocity.0 = 0.0;
    }

    let Some(tablet) = tablet else {
        out.spent = true;
        return out;
    };
    // It faces the tablet, and stops dead whenever it has to turn round.
    let (cx, _) = npc.center();
    let toward = (tablet.center().0 - cx).signum() as i8;
    if toward != 0 && toward != npc.direction {
        npc.velocity.0 = 0.0;
        npc.direction = toward;
        npc.sprite_direction = toward;
    }
    npc.ai[0] += 1.0;
    if npc.ai[0] >= 300.0 {
        npc.ai[0] = 0.0;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::CULTIST_TABLET;
    use terrustia_proto::tile::Tile;

    struct Dungeon(HashMap<(i32, i32), Tile>);

    impl TileView for Dungeon {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn world<'a>(tiles: &'a Dungeon) -> World<'a, Dungeon> {
        crate::game::ai::calm(
            tiles,
            Some(Target {
                slot: 0,
                center: (300.0, 0.0),
                velocity: (0.0, 0.0),
                alive: true,
            }),
        )
    }

    fn tablet_at(position: (f32, f32)) -> Parent {
        Parent {
            position,
            size: (36.0, 48.0),
            rotation: 0.0,
            scale: 1.0,
            velocity: (0.0, 0.0),
            direction: 1,
            sprite_direction: 1,
            time_left: 3600,
            state: 0.0,
            phase: 0.0,
            health: 1.0,
        }
    }

    /// It gathers four: two who shoot and two who kneel.
    #[test]
    fn the_tablet_gathers_four_cultists() {
        let tiles = Dungeon(HashMap::new());
        let w = world(&tiles);
        let mut t = Npc::new(CULTIST_TABLET, (0.0, 0.0), 1).expect("tablet");

        let out = tablet(&mut t, &w, 0);
        assert_eq!(out.spawn.len(), TABLET_CULTISTS);
        let devotes = out
            .spawn
            .iter()
            .filter(|s| s.npc_type == CULTIST_DEVOTE)
            .count();
        assert_eq!(devotes, 2, "two kneel");
        assert_eq!(out.spawn.len() - devotes, 2, "and two shoot");
        assert!(tablet(&mut t, &w, 4).spawn.is_empty(), "only once");
    }

    /// Nothing happens while any of them is alive.
    #[test]
    fn the_ritual_waits_for_the_last_of_them() {
        let tiles = Dungeon(HashMap::new());
        let w = world(&tiles);
        let mut t = Npc::new(CULTIST_TABLET, (0.0, 0.0), 1).unwrap();
        tablet(&mut t, &w, 0);

        for _ in 0..600 {
            let out = tablet(&mut t, &w, 1);
            assert!(!out.ritual_complete, "one of them is still up");
            assert!(out.shots.is_empty());
        }
    }

    /// With the last of them gone it shatters, throws shards, and raises the Cultist.
    #[test]
    fn the_last_death_breaks_the_tablet() {
        let tiles = Dungeon(HashMap::new());
        let w = world(&tiles);
        let mut t = Npc::new(CULTIST_TABLET, (0.0, 0.0), 1).unwrap();
        tablet(&mut t, &w, 0);

        // The order is the point, and it used to be the other way round. Vanilla raises the
        // Cultist in the same statement that starts the shatter (`NPC.cs:37179-37205`), so he is
        // there for the whole three seconds of it and the shards fly *into* him. Reporting the
        // ritual complete at the end instead is three seconds late, and it is why `aiStyle 98`
        // could not be written: for a shard's entire flight there was nothing to converge on.
        let mut shards = 0;
        let mut raised_on = None;
        let mut gone_on = None;
        for tick in 0..(TABLET_SHATTER_TICKS as i32 + 10) {
            let out = tablet(&mut t, &w, 0);
            if out.ritual_complete {
                raised_on = Some(tick);
            }
            shards += out.shots.len();
            if out.spent {
                gone_on = Some(tick);
                break;
            }
        }
        let raised = raised_on.expect("the Cultist should have been raised");
        let gone = gone_on.expect("and the tablet should have finished breaking");
        assert_eq!(
            raised, 0,
            "raised on the first tick of the shatter, not the last"
        );
        assert!(
            gone > raised,
            "and the tablet outlives him by the shatter: {raised} then {gone}"
        );
        assert!(shards > 0, "with shards thrown in between");
    }

    /// The tablet itself cannot be attacked.
    #[test]
    fn the_tablet_cannot_be_broken_by_hand() {
        let tiles = Dungeon(HashMap::new());
        let w = world(&tiles);
        let mut t = Npc::new(CULTIST_TABLET, (0.0, 0.0), 1).unwrap();
        tablet(&mut t, &w, 4);
        assert!(t.invulnerable);
        assert!(!t.take_damage(9999, 0.0, 1));
    }

    /// A devote turns to face the tablet, and does not outlive it.
    #[test]
    fn a_devote_faces_the_tablet_and_dies_with_it() {
        let mut d = Npc::new(CULTIST_DEVOTE, (500.0, 0.0), 1).expect("devote");
        d.direction = 1;
        devote(&mut d, Some(tablet_at((0.0, 0.0))));
        assert_eq!(d.direction, -1, "the tablet is to its left");
        assert!(devote(&mut d, None).spent, "and it does not outlive it");
    }
}
