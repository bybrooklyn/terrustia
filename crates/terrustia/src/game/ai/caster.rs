//! Style 8 - the casters.
//!
//! A caster never walks anywhere. It stands still (its horizontal velocity is bled away at 7% a
//! tick, and there is nothing anywhere in the routine that adds to it) and every so often it blinks
//! somewhere else near you. Between blinks it conjures.
//!
//! Thirteen types share the style, in two halves. The four **pre-hardmode** casters summon an
//! **NPC** rather than a projectile: the fire imp's burning sphere, the goblin sorcerer's chaos
//! ball, the dark caster's water sphere and Tim's are all one-hit-point, gravity-free NPCs running
//! style 9. The **hardmode** half throws ordinary projectiles: the Rune Wizard, the dungeon's
//! librarian, and the four two-variant dungeon casters (Ragged Caster, Necromancer, Diabolist),
//! plus the Desert Djinn, which drops ghost lanterns around you instead of aiming at you.
//!
//! The cycle is 650 ticks. By default it casts on ticks 100, 200 and 300 and at 650 it picks a tile
//! near its target and appears there, but each of the hardmode types has its own cadence and four
//! of them cut the cycle short so they blink more often than they cast.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    CASTER_BLINK, CASTER_CYCLE, CASTER_TELEFRAG_GUARD, CASTER_TELEPORT_LIMIT,
    CASTER_TELEPORT_RANGE, CASTER_WINDUP, Conjuring, DJINN_LANTERNS, DJINN_SPREAD, DJINN_WINDUP,
    Thrown, conjuring, dungeon_wall,
};
use terrustia_proto::tile_solid::solid;

use super::{Shot, World, face, sight::solid_collision, sight::within_firing_range};
use crate::game::npc::{Npc, TILE, TileView};
use crate::game::npc_ai::Target;

/// Desert Djinn (`NPCID.DesertDjinn`): the one caster whose spell is placed rather than thrown.
const DESERT_DJINN: u16 = 533;

/// How long anything a caster throws lives, matching the other ported routines.
const SHOT_LIFETIME: u16 = 300;

/// What a caster's tick produced.
#[derive(Debug, Default)]
pub struct Cast {
    /// An NPC it conjured, as (type, position).
    pub summon: Option<(u16, (f32, f32))>,
    /// A projectile it threw.
    pub shot: Option<Shot>,
}

/// Aim a thrown spell at a target, with whatever scatter and lead its type carries.
///
/// This is the `num101`/`num102` block at `NPC.cs:21269-21288`: the offsets are added to the raw
/// difference *before* it is normalised, so a scattered shot is aimed differently rather than
/// merely nudged sideways.
fn aim(from: (f32, f32), t: Target, spell: Thrown, rng: &mut SmallRng) -> (f32, f32) {
    let mut dx = t.center.0 - from.0;
    let mut dy = t.center.1 - from.1;
    if spell.scatter > 0 {
        dx += rng.random_range(-spell.scatter..=spell.scatter) as f32;
        dy += rng.random_range(-spell.scatter..=spell.scatter) as f32;
    }
    if spell.lead != 0.0 {
        dx -= t.velocity.0 * spell.lead;
        dy -= t.velocity.1 * spell.lead;
    }
    let d = (dx * dx + dy * dy).sqrt();
    if d <= 0.0 {
        return (0.0, spell.speed);
    }
    (dx * spell.speed / d, dy * spell.speed / d)
}

/// Somewhere near the target to drop a Desert Djinn's ghost lantern (`NPC.cs:21196-21231`).
///
/// It wants open air, not on top of the player, not on top of itself, and with a five-tile square
/// of clear space around it. Fifty tries, and nothing at all if the target is a long way off.
fn lantern_spot<T: TileView>(
    npc: &Npc,
    world: &World<'_, T>,
    t: Target,
    rng: &mut SmallRng,
) -> Option<(i32, i32)> {
    let here = (
        (npc.center().0 / TILE) as i32,
        (npc.center().1 / TILE) as i32,
    );
    let goal = ((t.center.0 / TILE) as i32, (t.center.1 / TILE) as i32);
    if ((t.center.0 - npc.center().0).powi(2) + (t.center.1 - npc.center().1).powi(2)).sqrt()
        > 2000.0
    {
        return None;
    }
    for _ in 0..50 {
        let x = rng.random_range(goal.0 - DJINN_SPREAD..=goal.0 + DJINN_SPREAD);
        let y = rng.random_range(goal.1 - DJINN_SPREAD..=goal.1 + DJINN_SPREAD);
        // `num95` is zero, so this is only "not the target's own tile".
        if x == goal.0 && y == goal.1 {
            continue;
        }
        // ...and never within six tiles of the djinn itself.
        if (x - here.0).abs() <= DJINN_SPREAD && (y - here.1).abs() <= DJINN_SPREAD {
            continue;
        }
        let tile = world.tiles.tile(x, y);
        if tile.is_active() || tile.liquid_kind == terrustia_proto::tile::Liquid::Lava {
            continue;
        }
        // `Collision.SolidTiles(x - 2, x + 2, y - 2, y + 2)`.
        if solid_collision(
            world.tiles,
            (((x - 2) * 16) as f32, ((y - 2) * 16) as f32),
            (5 * 16, 5 * 16),
        ) {
            continue;
        }
        return Some((x, y));
    }
    None
}

/// Whether a caster will fire regardless of how far away its target is. Only the goblin sorcerer.
fn always_in_range(npc_type: u16) -> bool {
    npc_type == 29
}

/// A caster with nothing to cast, which still blinks.
///
/// All thirteen style-8 types have a table entry, so nothing reaches this; it exists so that the
/// routine cannot ever be talked into returning before the teleport again.
const NO_SPELL: Conjuring = Conjuring {
    summons: None,
    throws: None,
    offset: (0.0, -8.0),
    offset_follows_facing: false,
    release_at: 25.0,
    blink: CASTER_BLINK,
    dungeon_bound: false,
    cadence: &[],
    cycle_ends_at: None,
};

/// Find somewhere near the target to appear, as `NPC.AI_AttemptToFindTeleportSpot` does.
///
/// It wants a solid tile with three clear tiles above it, not right where the caster already is,
/// and not close enough to a player to land on top of them. A dungeon caster additionally wants a
/// dungeon wall behind it, which is what keeps dark casters inside the dungeon.
fn find_landing<T: TileView>(
    npc: &Npc,
    world: &World<'_, T>,
    target: Target,
    range: i32,
    dungeon_bound: bool,
    rng: &mut SmallRng,
) -> Option<(i32, i32)> {
    let here = (
        (npc.center().0 / TILE) as i32,
        (npc.center().1 / TILE) as i32,
    );
    let goal = (
        (target.center.0 / TILE) as i32,
        (target.center.1 / TILE) as i32,
    );
    // Too far to reach at all.
    if ((here.0 - goal.0) * 16).abs() as f32 + ((here.1 - goal.1) * 16).abs() as f32
        > CASTER_TELEPORT_LIMIT
    {
        return None;
    }

    for _ in 0..100 {
        let x = rng.random_range(goal.0 - range..=goal.0 + range);
        let start = rng.random_range(goal.1 - range..=goal.1 + range);
        for y in start..goal.1 + range {
            // Not the tile it is already standing on.
            if (y - here.1).abs() <= 1 && (x - here.0).abs() <= 1 {
                continue;
            }
            let floor = world.tiles.tile(x, y);
            if !floor.is_active() || !solid(floor.block) {
                continue;
            }
            let above = world.tiles.tile(x, y - 1);
            if dungeon_bound && !dungeon_wall(above.wall) {
                continue;
            }
            if above.liquid > 0 && above.liquid_kind == terrustia_proto::tile::Liquid::Lava {
                continue;
            }
            // Three tiles of headroom either side.
            let clear = (x - 1..=x + 1)
                .all(|cx| (y - 4..=y - 1).all(|cy| !world.tiles.tile(cx, cy).is_active()));
            if !clear {
                continue;
            }
            // And nobody standing in it.
            let guard = (CASTER_TELEFRAG_GUARD * 16) as f32;
            let landing = ((x * 16) as f32, (y * 16) as f32);
            if (target.center.0 - landing.0).abs() < guard + 16.0
                && (target.center.1 - landing.1).abs() < guard + 16.0
            {
                continue;
            }
            return Some((x, y));
        }
    }
    None
}

/// Drive one caster for a tick.
pub fn update<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) -> Cast {
    let mut cast = Cast::default();
    if let Some(t) = world.target {
        face(npc, t);
    }

    // It never propels itself; whatever knocked it about is bled off.
    npc.velocity.0 *= 0.93;
    if npc.velocity.0 > -0.1 && npc.velocity.0 < 0.1 {
        npc.velocity.0 = 0.0;
    }
    if npc.ai[0] == 0.0 {
        npc.ai[0] = 500.0;
    }

    // A pending blink lands at the top of the tick after it was chosen.
    if npc.ai[2] != 0.0 && npc.ai[3] != 0.0 {
        npc.position.0 = npc.ai[2] * 16.0 - (npc.stats.width / 2) as f32 + 8.0;
        npc.position.1 = npc.ai[3] * 16.0 - npc.height();
        npc.velocity = (0.0, 0.0);
        npc.ai[2] = 0.0;
        npc.ai[3] = 0.0;
        npc.dirty = true;
    }

    npc.ai[0] += 1.0;
    // A style-8 type with no spell in the table still blinks. Vanilla runs the teleport for every
    // one of the thirteen (`NPC.cs:21155-21186`); bailing out above it left eight of them rooted to
    // the spot doing nothing at all but absorbing knockback.
    let spell = conjuring(npc.npc_type).unwrap_or(NO_SPELL);

    let in_range = always_in_range(npc.npc_type)
        || world
            .target
            .is_some_and(|t| within_firing_range(npc.center(), t.center));
    if in_range && spell.cadence.contains(&npc.ai[0]) {
        npc.ai[1] = if npc.npc_type == DESERT_DJINN {
            DJINN_WINDUP
        } else {
            CASTER_WINDUP
        };
        npc.dirty = true;
    }

    // Four of the hardmode casters shorten the cycle so they blink more often than they cast.
    if let Some((at, jump_to)) = spell.cycle_ends_at
        && npc.ai[0] >= at
    {
        npc.ai[0] = jump_to;
    }

    if npc.ai[0] >= CASTER_CYCLE {
        npc.ai[0] = 1.0;
        if let Some(t) = world.target
            && let Some((x, y)) = find_landing(
                npc,
                world,
                t,
                CASTER_TELEPORT_RANGE,
                spell.dungeon_bound,
                rng,
            )
        {
            npc.ai[1] = spell.blink;
            npc.ai[2] = x as f32;
            npc.ai[3] = y as f32;
        }
        npc.dirty = true;
    }

    if npc.ai[1] > 0.0 {
        npc.ai[1] -= 1.0;
        // The djinn's wind-up drops a lantern every thirtieth tick rather than releasing once.
        let releasing = if npc.npc_type == DESERT_DJINN {
            npc.ai[1] % 30.0 == 0.0 && npc.ai[1] / 30.0 < DJINN_LANTERNS
        } else {
            npc.ai[1] == spell.release_at
        };
        if releasing {
            let x = npc.position.0
                + npc.width() / 2.0
                + if spell.offset_follows_facing {
                    spell.offset.0 * f32::from(npc.direction)
                } else {
                    spell.offset.0
                };
            let y = if spell.throws.is_some_and(|t| t.from_center) {
                npc.position.1 + npc.height() / 2.0
            } else {
                npc.position.1 + spell.offset.1
            };
            if let Some(summon) = spell.summons {
                cast.summon = Some((summon, (x, y)));
            }
            if let Some(thrown) = spell.throws {
                cast.shot = match (npc.npc_type, world.target) {
                    (DESERT_DJINN, Some(t)) => {
                        lantern_spot(npc, world, t, rng).map(|(tx, ty)| Shot {
                            projectile: thrown.projectile,
                            damage: thrown.damage,
                            position: ((tx * 16 + 8) as f32, (ty * 16 + 8) as f32),
                            velocity: (0.0, 0.0),
                            time_left: SHOT_LIFETIME,
                            ai: [0.0; 3],
                        })
                    }
                    (DESERT_DJINN, None) => None,
                    _ => Some(Shot {
                        projectile: thrown.projectile,
                        // `GetAttackDamage_ForProjectiles(n, n * 0.8f)`.
                        damage: if world.conditions.expert {
                            (thrown.damage as f32 * 0.8) as i32
                        } else {
                            thrown.damage
                        },
                        position: (x, y),
                        velocity: world
                            .target
                            .map_or((0.0, 0.0), |t| aim((x, y), t, thrown, rng)),
                        time_left: SHOT_LIFETIME,
                        ai: [0.0; 3],
                    }),
                };
            }
            npc.dirty = true;
        }
    }

    npc.sprite_direction = npc.direction;
    cast
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    #[derive(Default)]
    struct Cave(HashMap<(i32, i32), Tile>);

    impl TileView for Cave {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    /// A floor at tile y = 500 across a wide span, optionally walled as a dungeon.
    fn cavern(dungeon: bool) -> Cave {
        let mut c = Cave::default();
        for x in 0..2000 {
            for y in 500..510 {
                c.0.insert((x, y), Tile::block(1));
            }
            if dungeon {
                for y in 480..500 {
                    c.0.insert((x, y), Tile::AIR.with_wall(7));
                }
            }
        }
        c
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(21)
    }

    fn caster(npc_type: u16, tile_x: i32) -> Npc {
        let mut n = Npc::new(npc_type, (0.0, 0.0), 1).expect("a style 8 type");
        n.position = (tile_x as f32 * TILE, 500.0 * TILE - n.height());
        n
    }

    fn player_at(x: f32, y: f32) -> Target {
        Target {
            slot: 0,
            center: (x, y),
            velocity: (0.0, 0.0),
            alive: true,
        }
    }

    fn world<'a>(tiles: &'a Cave, target: Option<Target>) -> World<'a, Cave> {
        crate::game::ai::calm(tiles, target)
    }

    #[test]
    fn a_caster_never_walks() {
        let tiles = cavern(false);
        let mut c = caster(29, 100);
        c.velocity.0 = 5.0;
        let (cx, cy) = c.center();
        let t = Some(player_at(cx + 200.0, cy));
        for _ in 0..200 {
            update(&mut c, &world(&tiles, t), &mut rng());
        }
        assert_eq!(c.velocity.0, 0.0, "it should have come to a complete stop");
    }

    #[test]
    fn every_pre_hardmode_caster_summons_an_npc_rather_than_firing() {
        for (npc_type, summon) in [(24u16, 25u16), (29, 30), (32, 33), (45, 665)] {
            let spell = conjuring(npc_type).expect("a caster");
            assert_eq!(spell.summons, Some(summon), "type {npc_type}");
            assert!(spell.throws.is_none(), "type {npc_type} throws nothing");
        }
        let librarian = conjuring(693).expect("librarian");
        assert!(librarian.summons.is_none());
        assert_eq!(
            librarian.throws.map(|t| (t.projectile, t.damage)),
            Some((1092, 13))
        );
    }

    /// Style 8 is thirteen types (`NPC.cs:21082-21155`), and eight of them had no entry at all.
    #[test]
    fn every_style_eight_type_has_a_spell() {
        for npc_type in [
            24u16, 29, 32, 45, 172, 281, 282, 283, 284, 285, 286, 533, 693,
        ] {
            let stats = terrustia_proto::npc_data::npc_stats(npc_type).expect("a real type");
            assert_eq!(stats.ai_style, 8, "type {npc_type} should be a caster");
            let spell = conjuring(npc_type).unwrap_or_else(|| panic!("type {npc_type}"));
            assert!(
                spell.summons.is_some() || spell.throws.is_some(),
                "type {npc_type} does nothing"
            );
            assert!(!spell.cadence.is_empty(), "type {npc_type} never casts");
        }
    }

    /// The regression: `conjuring` returning `None` short-circuited the routine, and the teleport
    /// block sat after it, so eight of the thirteen neither cast nor blinked. They stood still.
    #[test]
    fn every_style_eight_type_blinks_at_the_end_of_its_cycle() {
        let dungeon = cavern(true);
        let t = Some(player_at(140.0 * TILE, 499.0 * TILE));
        let mut r = rng();
        for npc_type in [
            24u16, 29, 32, 45, 172, 281, 282, 283, 284, 285, 286, 533, 693,
        ] {
            let mut c = caster(npc_type, 100);
            let start = c.position.0;
            for _ in 0..(CASTER_CYCLE as i32 + 5) {
                update(&mut c, &world(&dungeon, t), &mut r);
            }
            assert!(
                (c.position.0 - start).abs() > 100.0,
                "type {npc_type} should have blinked, still at {}",
                c.position.0
            );
        }
    }

    /// The Rune Wizard is in this server's live hardmode cavern spawn pool, so it is the one of the
    /// eight that was reachable in ordinary play. `NPC.cs:21095-21101`, `:21339-21352`.
    #[test]
    fn a_rune_wizard_throws_six_bolts_a_cycle() {
        let tiles = cavern(false);
        let mut c = caster(172, 100);
        c.ai[0] = 1.0;
        let (cx, cy) = c.center();
        let t = Some(player_at(cx + 300.0, cy));
        let mut r = rng();
        let mut shots = Vec::new();
        for _ in 0..500 {
            if let Some(shot) = update(&mut c, &world(&tiles, t), &mut r).shot {
                shots.push(shot);
            }
        }
        assert_eq!(shots.len(), 6, "six casts a cycle, not three");
        assert_eq!(shots[0].projectile, 129);
        assert_eq!(shots[0].damage, 40);
        let v = shots[0].velocity;
        assert!(
            ((v.0 * v.0 + v.1 * v.1).sqrt() - 10.0).abs() < 1e-3,
            "runes leave at 10, got {v:?}"
        );
        assert!(v.0 > 0.0, "and toward the player");
    }

    /// `GetAttackDamage_ForProjectiles(n, n * 0.8f)` (`NPC.cs:21290`).
    #[test]
    fn expert_shots_carry_four_fifths_of_the_listed_damage() {
        let tiles = cavern(true);
        let mut c = caster(285, 100);
        let (cx, cy) = c.center();
        let mut w = world(&tiles, Some(player_at(cx + 300.0, cy)));
        w.conditions.expert = true;
        c.ai[0] = 1.0;
        let mut r = rng();
        let mut damage = None;
        for _ in 0..400 {
            if let Some(shot) = update(&mut c, &w, &mut r).shot {
                damage = Some(shot.damage);
                break;
            }
        }
        assert_eq!(damage, Some(32), "40 becomes 32");
    }

    /// The four dungeon casters end their cycles early (`NPC.cs:21093`, `:21121`, `:21146`,
    /// `:21151`), so they blink far more often than a goblin sorcerer does.
    #[test]
    fn the_dungeon_casters_cut_their_cycle_short() {
        for (npc_type, ends_at) in [(281u16, 540.0), (283, 450.0), (285, 401.0), (533, 360.0)] {
            let spell = conjuring(npc_type).expect("a caster");
            assert_eq!(spell.cycle_ends_at.map(|(at, _)| at), Some(ends_at));
        }
        assert!(conjuring(29).unwrap().cycle_ends_at.is_none());
        assert!(conjuring(172).unwrap().cycle_ends_at.is_none());
    }

    /// `NPC.cs:21190-21237`: a djinn drops five ghost lanterns in open air near you rather than
    /// throwing anything at you.
    #[test]
    fn a_desert_djinn_drops_five_lanterns_around_its_target() {
        let tiles = cavern(false);
        let mut c = caster(533, 100);
        c.ai[0] = 1.0;
        // Far enough that the six-tile exclusion around the djinn itself is clear of the target.
        let t = player_at(140.0 * TILE, 495.0 * TILE);
        let mut r = rng();
        let mut lanterns = Vec::new();
        for _ in 0..400 {
            if let Some(shot) = update(&mut c, &world(&tiles, Some(t)), &mut r).shot {
                lanterns.push(shot);
            }
        }
        assert_eq!(lanterns.len(), 5, "five a cycle");
        for l in &lanterns {
            assert_eq!(l.projectile, 596);
            assert_eq!(l.velocity, (0.0, 0.0), "they are placed, not thrown");
            assert!(
                (l.position.0 - t.center.0).abs() <= (DJINN_SPREAD as f32 + 1.0) * TILE,
                "near the target, got {:?}",
                l.position
            );
        }
    }

    #[test]
    fn a_goblin_sorcerer_conjures_a_chaos_ball_on_its_cadence() {
        let tiles = cavern(false);
        let mut c = caster(29, 100);
        let (cx, cy) = c.center();
        let t = Some(player_at(cx + 200.0, cy));
        // A fresh caster starts its timer at 500, so it blinks first and only then settles into
        // the cadence. Wind it to the start of a cycle to watch a whole one.
        c.ai[0] = 1.0;
        let mut summons = Vec::new();
        for _ in 0..350 {
            if let Some(s) = update(&mut c, &world(&tiles, t), &mut rng()).summon {
                summons.push(s.0);
            }
        }
        assert_eq!(summons, vec![30, 30, 30], "three casts in one cycle");
    }

    #[test]
    fn a_fire_imp_conjures_to_the_side_and_sooner() {
        let tiles = cavern(false);
        let mut c = caster(24, 100);
        c.direction = 1;
        let (cx, cy) = c.center();
        let t = Some(player_at(cx + 200.0, cy));
        c.ai[0] = 1.0;
        let mut where_at = None;
        for _ in 0..350 {
            if let Some(s) = update(&mut c, &world(&tiles, t), &mut rng()).summon {
                where_at = Some(s.1);
                break;
            }
        }
        let at = where_at.expect("should have conjured");
        assert!(
            at.0 > c.position.0 + c.width() / 2.0,
            "should be out to its right, got {at:?}"
        );
        assert_eq!(conjuring(24).unwrap().release_at, 10.0, "and cast sooner");
    }

    /// `num91` (`NPC.cs:21160-21164`) is how long the blink is *held*, not how far it reaches:
    /// `AI_AttemptToFindTeleportSpot` is always called with its default twenty-tile range. Reading
    /// the Fire Imp's 5 as a search range put every candidate inside the five-tile telefrag guard,
    /// so an imp could never find anywhere to go and never blinked at all.
    #[test]
    fn a_fire_imps_five_is_its_blink_delay_and_not_its_reach() {
        assert_eq!(
            conjuring(24).unwrap().blink,
            terrustia_proto::npc_params::CASTER_BLINK_SHORT
        );
        assert_eq!(conjuring(29).unwrap().blink, CASTER_BLINK);

        let tiles = cavern(false);
        let mut c = caster(24, 100);
        let start = c.position.0;
        let t = Some(player_at(140.0 * TILE, 499.0 * TILE));
        let mut r = rng();
        for _ in 0..(CASTER_CYCLE as i32 + 5) {
            update(&mut c, &world(&tiles, t), &mut r);
        }
        assert!(
            (c.position.0 - start).abs() > 100.0,
            "an imp should blink too, still at {}",
            c.position.0
        );
    }

    #[test]
    fn a_caster_out_of_range_holds_its_fire_and_a_sorcerer_does_not() {
        let tiles = cavern(false);
        let (cx, cy) = (100.0 * TILE, 500.0 * TILE);
        let far = Some(player_at(cx + 40_000.0, cy));

        let mut imp = caster(24, 100);
        imp.ai[0] = 1.0;
        for _ in 0..350 {
            assert!(
                update(&mut imp, &world(&tiles, far), &mut rng())
                    .summon
                    .is_none()
            );
        }

        let mut sorcerer = caster(29, 100);
        sorcerer.ai[0] = 1.0;
        let mut cast = false;
        for _ in 0..350 {
            if update(&mut sorcerer, &world(&tiles, far), &mut rng())
                .summon
                .is_some()
            {
                cast = true;
                break;
            }
        }
        assert!(cast, "a goblin sorcerer fires whatever the range");
    }

    #[test]
    fn a_caster_blinks_at_the_end_of_its_cycle() {
        let tiles = cavern(false);
        let mut c = caster(29, 100);
        let start = c.position.0;
        let t = Some(player_at(140.0 * TILE, 499.0 * TILE));
        let mut r = rng();
        for _ in 0..(CASTER_CYCLE as i32 + 5) {
            update(&mut c, &world(&tiles, t), &mut r);
        }
        assert!(
            (c.position.0 - start).abs() > 100.0,
            "should have moved, from {start} to {}",
            c.position.0
        );
    }

    #[test]
    fn a_dark_caster_will_not_blink_out_of_the_dungeon() {
        // No dungeon walls anywhere, so there is nowhere it will accept.
        let plain = cavern(false);
        let mut c = caster(32, 100);
        let start = c.position.0;
        let t = Some(player_at(140.0 * TILE, 499.0 * TILE));
        let mut r = rng();
        for _ in 0..(CASTER_CYCLE as i32 + 5) {
            update(&mut c, &world(&plain, t), &mut r);
        }
        assert_eq!(c.position.0, start, "it should have stayed put");

        // With the walls in place it moves.
        let dungeon = cavern(true);
        let mut c = caster(32, 100);
        for _ in 0..(CASTER_CYCLE as i32 + 5) {
            update(&mut c, &world(&dungeon, t), &mut r);
        }
        assert!((c.position.0 - start).abs() > 100.0, "should have blinked");
    }

    #[test]
    fn a_caster_will_not_land_on_top_of_its_target() {
        let tiles = cavern(false);
        let mut c = caster(29, 100);
        let t = player_at(140.0 * TILE, 499.0 * TILE);
        let mut r = rng();
        for _ in 0..40 {
            if let Some((x, y)) = find_landing(&c, &world(&tiles, Some(t)), t, 20, false, &mut r) {
                let guard = (CASTER_TELEFRAG_GUARD * 16) as f32;
                let landing = ((x * 16) as f32, (y * 16) as f32);
                assert!(
                    (t.center.0 - landing.0).abs() >= guard
                        || (t.center.1 - landing.1).abs() >= guard,
                    "landed on the player at {landing:?}"
                );
            }
            c.ai[0] = 0.0;
        }
    }

    #[test]
    fn a_caster_a_world_away_does_not_try_to_blink() {
        let tiles = cavern(false);
        let c = caster(29, 100);
        let t = player_at(100.0 * TILE + CASTER_TELEPORT_LIMIT * 2.0, 499.0 * TILE);
        assert!(find_landing(&c, &world(&tiles, Some(t)), t, 20, false, &mut rng()).is_none());
    }
}
