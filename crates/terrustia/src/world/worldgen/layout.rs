//! Where everything goes, decided before any tile is written.
//!
//! Every later pass reads this and nothing else decides placement, which is what stops the
//! dungeon being dug into the jungle or the evil swallowing spawn. Terraria's own generator keeps
//! the same handful of numbers on `GenVars` and refers back to them for a hundred passes.
//!
//! The constraints that actually matter, and why:
//!
//! * The **dungeon** and the **jungle** sit on opposite sides. A player must cross the world to
//!   go from Skeletron to the temple, and putting them together removes most of a playthrough's
//!   middle.
//! * The **evil biome** keeps well clear of spawn. Landing in corruption on the first morning is
//!   not a difficulty curve, it is a dead character.
//! * The **snow** and **desert** take the remaining surface, and neither may cover spawn either —
//!   a desert start has no wood.

use super::rand::UnifiedRandom;

/// Which evil a world has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evil {
    Corruption,
    Crimson,
}

/// A horizontal band of the surface, in tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub from: i32,
    pub to: i32,
}

impl Band {
    pub fn contains(&self, x: i32) -> bool {
        x >= self.from && x < self.to
    }

    pub fn width(&self) -> i32 {
        self.to - self.from
    }

    pub fn centre(&self) -> i32 {
        (self.from + self.to) / 2
    }

    fn overlaps(&self, other: &Band, gap: i32) -> bool {
        self.from - gap < other.to && other.from - gap < self.to
    }
}

/// Everything the passes need to know about where things are.
#[derive(Debug, Clone)]
pub struct Layout {
    pub width: i32,
    pub height: i32,
    /// The average surface height. Terrain varies around it.
    pub surface: i32,
    /// Where dirt gives way to stone.
    pub rock: i32,
    /// The top of the underworld.
    pub underworld: i32,
    pub spawn_x: i32,
    pub evil: Evil,
    pub dungeon_x: i32,
    pub ocean_left: Band,
    pub ocean_right: Band,
    pub jungle: Band,
    pub snow: Band,
    pub desert: Band,
    pub evil_band: Band,
    /// Where the jungle temple's entrance is, deep under the jungle.
    pub temple: (i32, i32),
    /// Drunk World only: which half of the world holds the Crimson, the other holding the
    /// Corruption (`WorldGen.cs:2052-2062`, `GenVars.crimsonLeft`). `None` in every other world,
    /// which has one evil across one band.
    pub drunk_crimson_left: Option<bool>,
    /// Drunk World only: the second evil's band, mirroring `evil_band` onto the other half. `None`
    /// in every other world.
    pub second_evil_band: Option<Band>,
}

impl Layout {
    /// Which evil is at this column. One answer for an ordinary world; two for Drunk World.
    pub fn evil_at(&self, x: i32) -> Evil {
        match self.drunk_crimson_left {
            None => self.evil,
            Some(crimson_left) => {
                let left = x < self.width / 2;
                if left == crimson_left {
                    Evil::Crimson
                } else {
                    Evil::Corruption
                }
            }
        }
    }
}

/// How wide an ocean is on a full-sized world, in tiles.
///
/// Scaled down on a small one — see [`Layout::plan`]. A fixed width makes the two oceans of a
/// four-hundred-tile world overlap in the middle, which leaves nowhere for anything else and
/// sends every later band's arithmetic backwards.
const OCEAN_WIDTH: i32 = 250;

/// The smallest fraction of a world either ocean may take.
const OCEAN_SHARE: i32 = 8;
/// How close to the world's edge anything else may come.
const EDGE_MARGIN: i32 = 60;
/// How far the evil biome must keep from spawn.
const EVIL_CLEARANCE: i32 = 200;
/// How far apart two surface biomes must sit.
const BIOME_GAP: i32 = 40;

impl Layout {
    /// Decide a world's shape.
    ///
    /// Everything is drawn from one generator in a fixed order, so the same seed lays out the
    /// same world. That is not vanilla parity — the numbers differ from Terraria's — but it is
    /// the property that makes a seed worth typing in at all.
    pub fn plan(width: i32, height: i32, rand: &mut UnifiedRandom) -> Self {
        let surface = (f64::from(height) * 0.28) as i32;
        let rock = (f64::from(height) * 0.42) as i32;
        // The underworld has to leave room below it for lava and hellstone. On a very short world
        // the obvious fraction puts its top *below* the space it needs, which sends every depth
        // range under it backwards, so it is clamped rather than merely scaled.
        let underworld = (height - (f64::from(height) * 0.14) as i32).min(height - 60);
        let spawn_x = width / 2;

        let evil = if rand.next_bool() {
            Evil::Corruption
        } else {
            Evil::Crimson
        };
        // -1 for the left of the world, 1 for the right. The jungle sits opposite. A local rather
        // than a field of the record: the two passes that read it are both right here, and nothing
        // downstream has ever asked which side the dungeon is on.
        let dungeon_side = if rand.next_bool() { -1 } else { 1 };

        // Never more than a fraction of the world each, so two oceans cannot meet in the middle.
        let ocean_width = OCEAN_WIDTH.min(width / OCEAN_SHARE).max(8);
        let ocean_left = Band {
            from: 0,
            to: ocean_width,
        };
        let ocean_right = Band {
            from: width - ocean_width,
            to: width,
        };

        // The dungeon sits just inside one ocean, which is where a player finds it by walking to
        // the edge of the world.
        let inset = (width / 20).clamp(10, 200);
        let dungeon_x = if dungeon_side < 0 {
            ocean_left.to + rand.next_range(inset / 2, inset + 1)
        } else {
            ocean_right.from - rand.next_range(inset / 2, inset + 1)
        }
        .clamp(ocean_width + 4, width - ocean_width - 4);

        // The jungle takes the side opposite the dungeon, so the two are a world apart.
        // A sixth of the world, but never wider than the room between the two oceans.
        let usable = (width - ocean_width * 2 - EDGE_MARGIN * 2).max(40);
        let jungle_width = (width / 6).clamp(24, 700).min(usable / 3);
        let jungle = if dungeon_side < 0 {
            let to = (ocean_right.from - rand.next_range(inset / 2, inset + 1))
                .clamp(ocean_width + jungle_width + 4, width - ocean_width - 4);
            Band {
                from: to - jungle_width,
                to,
            }
        } else {
            let from = (ocean_left.to + rand.next_range(inset / 2, inset + 1))
                .clamp(ocean_width + 4, width - ocean_width - jungle_width - 4);
            Band {
                from,
                to: from + jungle_width,
            }
        };

        // The snow, the desert and the evil take what is left, none of them over spawn and none
        // of them on top of each other.
        let mut taken = vec![jungle];
        let snow = Self::place(
            rand,
            width,
            ocean_width,
            (width / 8).clamp(20, 500).min(usable / 3),
            spawn_x,
            (width / 20).min(120),
            &taken,
        );
        taken.push(snow);
        let desert = Self::place(
            rand,
            width,
            ocean_width,
            (width / 10).clamp(16, 400).min(usable / 3),
            spawn_x,
            (width / 20).min(120),
            &taken,
        );
        taken.push(desert);
        let evil_band = Self::place(
            rand,
            width,
            ocean_width,
            (width / 9).clamp(18, 450).min(usable / 3),
            spawn_x,
            (width / 6).clamp(60, EVIL_CLEARANCE),
            &taken,
        );

        // The temple sits deep under the jungle, above the underworld.
        //
        // The band is clamped before it is drawn from rather than after. A small world can have
        // its rock layer and its underworld close enough together that the obvious
        // `rock + 120 .. underworld - 160` runs backwards, and the generator throws on that
        // rather than quietly returning nonsense — which is the right behaviour, and the reason
        // this has to be worked out here.
        let temple_top = rock + 100;
        let temple_bottom = (underworld - 120).max(temple_top + 20);
        let temple = (
            jungle.centre() + rand.next_range(-jungle_width / 4, jungle_width / 4),
            rand.next_range(temple_top, temple_bottom),
        );

        Self {
            width,
            height,
            surface,
            rock,
            underworld,
            spawn_x,
            evil,
            dungeon_x,
            ocean_left,
            ocean_right,
            jungle,
            snow,
            desert,
            evil_band,
            temple,
            drunk_crimson_left: None,
            second_evil_band: None,
        }
    }

    /// Find room for a band of a given width, clear of spawn and of everything already placed.
    ///
    /// Gives up after a bounded number of tries and takes the least bad spot rather than looping:
    /// a narrow world can genuinely have nowhere left, and a generator that spins there is worse
    /// than one that overlaps slightly.
    fn place(
        rand: &mut UnifiedRandom,
        width: i32,
        ocean_width: i32,
        band_width: i32,
        spawn_x: i32,
        clearance: i32,
        taken: &[Band],
    ) -> Band {
        let margin = EDGE_MARGIN.min(width / 20);
        let lowest = margin + ocean_width;
        let highest = width - margin - ocean_width - band_width;
        if highest <= lowest {
            return Band {
                from: lowest,
                to: lowest + band_width,
            };
        }
        // A small world can genuinely have nowhere left that satisfies everything. When that
        // happens the *best* candidate is kept rather than the first: taking the first put the
        // evil biome six tiles from spawn on an eight-hundred-wide world, which is a dead
        // character rather than a compromise.
        //
        // "Best" is: clear of everything already placed if possible, and failing that, as far
        // from spawn as it can manage.
        let mut best: Option<(i32, Band)> = None;
        for _ in 0..200 {
            let from = rand.next_range(lowest, highest);
            let band = Band {
                from,
                to: from + band_width,
            };
            let clear_of_spawn = !band.contains(spawn_x)
                && !(band.from - clearance..band.to + clearance).contains(&spawn_x);
            let clear_of_others = !taken
                .iter()
                .any(|other| band.overlaps(other, BIOME_GAP.min(width / 40)));

            if clear_of_spawn && clear_of_others {
                return band;
            }
            // Score what is left: keeping away from spawn matters most, since that is the one
            // that decides whether a new character survives the morning.
            let from_spawn = (band.centre() - spawn_x).abs();
            let score = from_spawn + if clear_of_others { 1000 } else { 0 };
            if best.is_none_or(|(had, _)| score > had) {
                best = Some((score, band));
            }
        }
        best.expect("the loop runs at least once").1
    }

    /// Which surface biome a column belongs to, if any.
    pub fn surface_biome(&self, x: i32) -> Option<Surface> {
        if self.ocean_left.contains(x) || self.ocean_right.contains(x) {
            Some(Surface::Ocean)
        } else if self.jungle.contains(x) {
            Some(Surface::Jungle)
        } else if self.snow.contains(x) {
            Some(Surface::Snow)
        } else if self.desert.contains(x) {
            Some(Surface::Desert)
        } else if self.evil_band.contains(x) || self.second_evil_band.is_some_and(|b| b.contains(x))
        {
            Some(Surface::Evil)
        } else {
            None
        }
    }
}

/// What a stretch of surface is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Ocean,
    Jungle,
    Snow,
    Desert,
    Evil,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(seed: i32) -> Layout {
        Layout::plan(4200, 1200, &mut UnifiedRandom::new(seed))
    }

    /// The same seed lays out the same world. Without this a seed is worth nothing.
    #[test]
    fn a_seed_decides_the_layout() {
        for seed in [1, 7, 12345, -99] {
            let a = layout(seed);
            let b = layout(seed);
            assert_eq!(a.evil, b.evil);
            assert_eq!(a.dungeon_x, b.dungeon_x);
            assert_eq!(a.jungle, b.jungle);
            assert_eq!(a.snow, b.snow);
            assert_eq!(a.temple, b.temple);
        }
    }

    /// ...and different seeds usually lay out different worlds.
    #[test]
    fn different_seeds_differ() {
        let worlds: Vec<Layout> = (1..8).map(layout).collect();
        assert!(
            worlds.windows(2).any(|w| w[0].jungle != w[1].jungle),
            "seven seeds should not all put the jungle in one place"
        );
    }

    /// The dungeon and the jungle sit on opposite sides, or half a playthrough's travel vanishes.
    #[test]
    fn the_dungeon_and_the_jungle_are_a_world_apart() {
        for seed in 1..40 {
            let l = layout(seed);
            let gap = (l.dungeon_x - l.jungle.centre()).abs();
            assert!(
                gap > l.width / 3,
                "seed {seed}: dungeon at {} and jungle at {} are only {gap} apart",
                l.dungeon_x,
                l.jungle.centre()
            );
        }
    }

    /// Nothing hostile covers spawn.
    #[test]
    fn spawn_is_left_alone() {
        for seed in 1..60 {
            let l = layout(seed);
            assert!(
                !l.evil_band.contains(l.spawn_x),
                "seed {seed} put the evil over spawn"
            );
            assert!(
                !l.jungle.contains(l.spawn_x) && !l.desert.contains(l.spawn_x),
                "seed {seed} put a biome over spawn"
            );
            assert!(
                (l.evil_band.from - l.spawn_x).abs() > 100
                    || (l.evil_band.to - l.spawn_x).abs() > 100,
                "seed {seed} put the evil right beside spawn"
            );
        }
    }

    /// Every band lies inside the world, and the oceans are at the edges.
    #[test]
    fn everything_is_inside_the_world() {
        for seed in 1..40 {
            let l = layout(seed);
            for (name, band) in [
                ("jungle", l.jungle),
                ("snow", l.snow),
                ("desert", l.desert),
                ("evil", l.evil_band),
            ] {
                assert!(
                    band.from >= 0 && band.to <= l.width,
                    "seed {seed}: {name} runs from {} to {} in a world {} wide",
                    band.from,
                    band.to,
                    l.width
                );
                assert!(band.width() > 0, "seed {seed}: {name} has no width");
            }
            assert_eq!(l.ocean_left.from, 0);
            assert_eq!(l.ocean_right.to, l.width);
            assert!(l.dungeon_x > 0 && l.dungeon_x < l.width);
        }
    }

    /// The layers are in the order the game reads them.
    #[test]
    fn the_layers_are_stacked_correctly() {
        let l = layout(1);
        assert!(l.surface < l.rock, "dirt is above stone");
        assert!(l.rock < l.underworld, "stone is above the underworld");
        assert!(
            l.underworld < l.height,
            "the underworld is inside the world"
        );
    }

    /// The temple sits under the jungle and above the underworld.
    #[test]
    fn the_temple_is_beneath_the_jungle() {
        for seed in 1..40 {
            let l = layout(seed);
            assert!(
                l.jungle.contains(l.temple.0)
                    || (l.temple.0 - l.jungle.centre()).abs() < l.jungle.width(),
                "seed {seed}: the temple at {} is not under the jungle {:?}",
                l.temple.0,
                l.jungle
            );
            assert!(
                l.temple.1 > l.rock && l.temple.1 < l.underworld,
                "seed {seed}: the temple at depth {} is not in the caverns",
                l.temple.1
            );
        }
    }
}
