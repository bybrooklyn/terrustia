//! Terraria's own random number generator.
//!
//! World generation is a chain: every pass draws from one generator that is never reseeded, so
//! the world a seed produces depends on the exact *number* of draws every earlier pass made, not
//! only on what they did with them. Reproducing a world therefore starts here — an approximation
//! of the generator is no use at all, and neither is a different one seeded the same way.
//!
//! This is the Knuth subtractive generator that .NET's `Random` has always used, which Terraria
//! copied into `UnifiedRandom` so its worlds would not change when the runtime did. It is
//! transcribed rather than approximated: the odd constants, the wrap at `int.MaxValue`, and the
//! `Sample()` multiplier are all load-bearing.
//!
//! Transcribed from `Terraria.Utilities.UnifiedRandom` in the 1.4.5.7 build.

/// The magic Knuth chose, and the reciprocal that turns a draw into a fraction.
const MSEED: i32 = 161_803_398;
const SAMPLE_SCALE: f64 = 4.656_612_875_245_797E-10;

/// The generator a world is built with.
#[derive(Debug, Clone)]
pub struct UnifiedRandom {
    seed_array: [i32; 56],
    inext: u32,
}

impl UnifiedRandom {
    pub fn new(seed: i32) -> Self {
        let mut me = Self {
            seed_array: [0; 56],
            inext: 0,
        };
        me.set_seed(seed);
        me
    }

    pub fn set_seed(&mut self, seed: i32) {
        self.seed_array = [0; 56];
        // `i32::MIN` has no positive counterpart, so it is mapped rather than negated.
        let magnitude = if seed == i32::MIN {
            i32::MAX
        } else {
            seed.wrapping_abs()
        };
        let mut prev = MSEED.wrapping_sub(magnitude);
        self.seed_array[55] = prev;
        let mut step = 1i32;
        for j in 1..55usize {
            let at = 21 * j % 55;
            self.seed_array[at] = step;
            step = prev.wrapping_sub(step);
            if step < 0 {
                step = step.wrapping_add(i32::MAX);
            }
            prev = self.seed_array[at];
        }
        // Four warm-up sweeps, which is what spreads the seed through the whole array.
        for _ in 1..5 {
            for l in 1..56usize {
                self.seed_array[l] =
                    self.seed_array[l].wrapping_sub(self.seed_array[1 + (l + 30) % 55]);
                if self.seed_array[l] < 0 {
                    self.seed_array[l] = self.seed_array[l].wrapping_add(i32::MAX);
                }
            }
        }
        self.inext = 0;
    }

    fn internal_sample(&mut self) -> i32 {
        let mut a = self.inext + 1;
        if a > 55 {
            a = 1;
        }
        let mut b = a + 21;
        if b > 55 {
            b -= 55;
        }
        let mut value = self.seed_array[a as usize].wrapping_sub(self.seed_array[b as usize]);
        if value == i32::MAX {
            value -= 1;
        }
        // Adding the sign-extended high bit is how the original folds a negative back into range.
        value = value.wrapping_add((value >> 31) & i32::MAX);
        self.seed_array[a as usize] = value;
        self.inext = a;
        value
    }

    fn sample(&mut self) -> f64 {
        f64::from(self.internal_sample()) * SAMPLE_SCALE
    }

    /// The next draw, and the one the world file records after every pass.
    ///
    /// Named for the game's own `Next`, which is what every transcribed pass calls.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> i32 {
        self.internal_sample()
    }

    /// What `next` *would* give, without taking it.
    ///
    /// The original does not fold the negative here, so this is not always the next value — it is
    /// transcribed as written because anything reading it depends on that.
    pub fn peek(&self) -> i32 {
        let mut a = self.inext + 1;
        if a > 55 {
            a = 1;
        }
        let mut b = a + 21;
        if b > 55 {
            b -= 55;
        }
        self.seed_array[a as usize].wrapping_sub(self.seed_array[b as usize])
    }

    /// A draw below `max`. A `max` of zero gives zero, as it does in the original.
    pub fn next_max(&mut self, max: i32) -> i32 {
        debug_assert!(max >= 0, "the game throws on a negative maximum");
        (self.sample() * f64::from(max)) as i32
    }

    /// A draw in `min..max`.
    pub fn next_range(&mut self, min: i32, max: i32) -> i32 {
        debug_assert!(min <= max, "the game throws when the range is backwards");
        let span = i64::from(max) - i64::from(min);
        if span <= i64::from(i32::MAX) {
            (self.sample() * span as f64) as i32 + min
        } else {
            ((self.sample_for_large_range() * span as f64) as i64 + i64::from(min)) as i32
        }
    }

    fn sample_for_large_range(&mut self) -> f64 {
        let mut value = self.internal_sample();
        if self.internal_sample() % 2 == 0 {
            value = -value;
        }
        (f64::from(value) + 2_147_483_646.0) / 4_294_967_293.0
    }

    pub fn next_double(&mut self) -> f64 {
        self.sample()
    }

    /// `UnifiedRandom.NextFloat()`: the same draw as [`Self::next_double`], narrowed to `f32`.
    ///
    /// The narrowing is not cosmetic. `ShapeRoot` accumulates its angle from these, and doing the
    /// arithmetic at `f64` would drift away from vanilla's branch shapes over a 40-to-60 step root.
    pub fn next_float(&mut self) -> f32 {
        self.sample() as f32
    }

    pub fn next_bool(&mut self) -> bool {
        self.next_max(2) == 0
    }

    pub fn next_bytes(&mut self, buffer: &mut [u8]) {
        for byte in buffer {
            *byte = (self.internal_sample() % 256) as u8;
        }
    }
}

/// Turn a world's seed *text* into the number the generator is started with.
///
/// A seed that reads as a number is used directly; anything else is hashed. That is why "goblin
/// army" and "1234" both work in the game's seed box.
///
/// Transcribed from `WorldFileData.TranslateSeed`.
pub fn translate_seed(text: &str) -> i32 {
    if let Ok(value) = text.parse::<i32>() {
        return if value == i32::MIN {
            i32::MAX
        } else {
            value.abs()
        };
    }
    crc32(text.as_bytes())
}

/// The CRC-32 the game hashes a text seed with.
fn crc32(bytes: &[u8]) -> i32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    (!crc) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same seed gives the same run, every time, in the same order.
    #[test]
    fn a_seed_is_a_sequence() {
        let a: Vec<i32> = {
            let mut r = UnifiedRandom::new(42);
            (0..20).map(|_| r.next()).collect()
        };
        let b: Vec<i32> = {
            let mut r = UnifiedRandom::new(42);
            (0..20).map(|_| r.next()).collect()
        };
        assert_eq!(a, b);
        let c: Vec<i32> = {
            let mut r = UnifiedRandom::new(43);
            (0..20).map(|_| r.next()).collect()
        };
        assert_ne!(a, c, "two seeds gave the same world");
    }

    /// Every draw is a non-negative int, which the whole of world generation assumes.
    #[test]
    fn draws_stay_in_range() {
        let mut r = UnifiedRandom::new(-7);
        for _ in 0..10_000 {
            let v = r.next();
            assert!((0..i32::MAX).contains(&v), "{v} is out of range");
        }
    }

    /// A bounded draw stays inside its bound, and a range draw inside its range.
    #[test]
    fn bounds_are_respected() {
        let mut r = UnifiedRandom::new(1234);
        for _ in 0..20_000 {
            let v = r.next_max(10);
            assert!((0..10).contains(&v), "{v} escaped 0..10");
            let v = r.next_range(-5, 5);
            assert!((-5..5).contains(&v), "{v} escaped -5..5");
        }
    }

    /// `next_max(0)` gives zero rather than dividing by nothing, as the original does.
    #[test]
    fn a_zero_bound_gives_zero() {
        let mut r = UnifiedRandom::new(9);
        assert_eq!(r.next_max(0), 0);
    }

    /// The whole range gets used, rather than a corner of it.
    #[test]
    fn the_distribution_covers_its_range() {
        let mut r = UnifiedRandom::new(5);
        let mut seen = [0usize; 6];
        for _ in 0..60_000 {
            seen[r.next_max(6) as usize] += 1;
        }
        for (face, &count) in seen.iter().enumerate() {
            assert!(
                count > 8_000 && count < 12_000,
                "face {face} came up {count} times in 60,000"
            );
        }
    }

    /// A numeric seed is itself; anything else is hashed. Both have to be stable.
    #[test]
    fn seed_text_translates() {
        assert_eq!(translate_seed("387441217"), 387_441_217);
        assert_eq!(translate_seed("-5"), 5);
        let hashed = translate_seed("rdererhrt456hjurty6jert5huy5er4");
        assert_eq!(hashed, translate_seed("rdererhrt456hjurty6jert5huy5er4"));
        assert_ne!(hashed, translate_seed("something else"));
    }

    /// The CRC is the standard one, checked against its published test vector.
    #[test]
    fn the_crc_is_the_usual_one() {
        assert_eq!(crc32(b"123456789") as u32, 0xCBF4_3926);
    }

    /// The sequence itself, pinned.
    ///
    /// These are not values this port chose — the first is .NET's own famous `Random(0).Next()`,
    /// and the rest were produced by a second implementation written from the same source. If a
    /// refactor ever moves one of them, every world this generator makes has changed.
    #[test]
    fn the_sequence_is_the_games_own() {
        let mut r = UnifiedRandom::new(0);
        assert_eq!(
            (0..6).map(|_| r.next()).collect::<Vec<_>>(),
            vec![
                1_559_595_546,
                1_755_192_844,
                1_649_316_166,
                1_198_642_031,
                442_452_829,
                1_200_195_957
            ],
            "seed 0 is .NET's own documented sequence"
        );

        let mut r = UnifiedRandom::new(42);
        assert_eq!(
            (0..6).map(|_| r.next()).collect::<Vec<_>>(),
            vec![
                1_434_747_710,
                302_596_119,
                269_548_474,
                1_122_627_734,
                361_709_742,
                563_913_476
            ]
        );

        // The seed of the reference world this generator is checked against.
        let mut r = UnifiedRandom::new(387_441_217);
        assert_eq!(
            (0..4).map(|_| r.next()).collect::<Vec<_>>(),
            vec![531_047_569, 229_513_906, 1_002_964_283, 739_233_999]
        );

        let mut r = UnifiedRandom::new(1234);
        assert_eq!(
            (0..8).map(|_| r.next_max(10)).collect::<Vec<_>>(),
            vec![3, 8, 3, 9, 3, 9, 8, 5]
        );
        let mut r = UnifiedRandom::new(1234);
        assert_eq!(
            (0..8).map(|_| r.next_range(-5, 5)).collect::<Vec<_>>(),
            vec![-2, 3, -2, 4, -2, 4, 3, 0]
        );
    }
}
