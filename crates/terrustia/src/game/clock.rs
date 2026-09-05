//! How much processor a stretch of code actually used.
//!
//! Timing a tick with the wall clock answers the wrong question. A tick that takes 26 ms of wall
//! clock has usually not done 26 ms of work: it has done a fraction of a millisecond of work and
//! spent the rest descheduled, because the machine is also running a game, a compiler, or a
//! backup. Reporting that as "ticks are using a lot of their budget" sends somebody hunting for a
//! slow routine that does not exist.
//!
//! The thread clock counts only the time this thread was on a core, so it measures the server's
//! own cost. Both numbers are worth having — work that overruns is a bug in here, wall clock that
//! overruns without the work to match is the machine being busy elsewhere — so the game loop
//! records both and says which one it is.

use std::time::Duration;

/// A reading of the current thread's CPU clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cpu(Duration);

impl Cpu {
    /// Read the calling thread's consumed CPU time.
    ///
    /// The one place in the whole workspace that needs `unsafe`, and the allow is per-function so
    /// it stays an explicit exception rather than a blanket permission: there is no safe way to
    /// ask an operating system how much processor a thread has used.
    #[cfg(unix)]
    #[allow(unsafe_code)]
    pub fn now() -> Self {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `clock_gettime` writes a `timespec` through the pointer and reads nothing else.
        let ok = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) } == 0;
        if !ok {
            // A clock that cannot be read must not make every tick look free or infinitely slow;
            // zero means the CPU check simply never fires and the wall clock still reports.
            return Self(Duration::ZERO);
        }
        Self(Duration::new(
            ts.tv_sec.max(0) as u64,
            ts.tv_nsec.clamp(0, 999_999_999) as u32,
        ))
    }

    /// Read the calling thread's consumed CPU time.
    ///
    /// Windows has no `clock_gettime` and no `CLOCK_THREAD_CPUTIME_ID` — neither symbol exists in
    /// libc's Windows module — so the `unix` version above does not merely misbehave here, it
    /// fails to compile. This was the *only* thing stopping the whole project building on Windows.
    ///
    /// `GetThreadTimes` is the equivalent, reporting kernel and user time as 100-nanosecond ticks
    /// in a pair of `FILETIME`s. Both are wanted: a tick that spends its time in a write syscall
    /// costs the server just as much as one that spends it in a loop.
    ///
    /// **Disclosed narrowing, and it matters for what the tick metrics mean here.** The unit is
    /// 100 ns but the *resolution* is not: `GetThreadTimes` is updated on the scheduler's own
    /// clock tick, about 15.6 ms by default. A game tick's whole budget is 16.67 ms, so on Windows
    /// a per-tick CPU reading is effectively quantised to "zero" or "most of a tick", and the p99
    /// tick figure the soak harness reports there cannot be compared with the same figure from a
    /// unix host. This was found the first time the suite ran on Windows at all: four million
    /// multiplies measured as exactly zero, twice, with the clock working correctly underneath.
    ///
    /// The fix is `QueryThreadCycleTime`, which counts actual cycles and so has the resolution
    /// this wants, at the cost of needing a cycles-per-second figure to turn them back into a
    /// `Duration` (and that figure moves with turbo and thermal throttling, so it has to be
    /// calibrated rather than assumed). That is a real piece of work and is recorded in
    /// `docs/release-blockers.md` rather than guessed at here. Until then, wall-clock timings on
    /// Windows are unaffected and remain trustworthy; only the CPU half is coarse.
    ///
    /// Nothing silently degrades because of it: the phase accounting still adds up, it is just
    /// granular, and the stall branch that separates "the server was slow" from "the machine was
    /// busy" reads the wall clock, which is exact on every platform.
    #[cfg(windows)]
    #[allow(unsafe_code)]
    pub fn now() -> Self {
        use std::mem::MaybeUninit;

        // Minimal declarations rather than a dependency on the whole Windows API surface, matching
        // how the unix side uses `libc` directly. `FILETIME` is two 32-bit halves of a 64-bit
        // count of 100ns intervals.
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn GetThreadTimes(
                thread: isize,
                creation: *mut FileTime,
                exit: *mut FileTime,
                kernel: *mut FileTime,
                user: *mut FileTime,
            ) -> i32;
        }

        let mut creation = MaybeUninit::<FileTime>::uninit();
        let mut exit = MaybeUninit::<FileTime>::uninit();
        let mut kernel = MaybeUninit::<FileTime>::uninit();
        let mut user = MaybeUninit::<FileTime>::uninit();

        // SAFETY: `GetThreadTimes` writes four `FILETIME`s through the pointers and reads nothing
        // else; the handle from `GetCurrentThread` is a pseudo-handle that is always valid and
        // needs no closing. The values are only read when it reports success.
        let ok = unsafe {
            GetThreadTimes(
                GetCurrentThread(),
                creation.as_mut_ptr(),
                exit.as_mut_ptr(),
                kernel.as_mut_ptr(),
                user.as_mut_ptr(),
            ) != 0
        };
        if !ok {
            return Self(Duration::ZERO);
        }
        // SAFETY: written by the call above, which reported success.
        let (kernel, user) = unsafe { (kernel.assume_init(), user.assume_init()) };
        let ticks = |t: FileTime| (u64::from(t.high) << 32) | u64::from(t.low);
        Self(Duration::from_nanos(
            (ticks(kernel) + ticks(user)).saturating_mul(100),
        ))
    }

    /// CPU time consumed since an earlier reading.
    pub fn since(self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Cpu;
    use std::time::Duration;

    /// Sleeping burns wall clock and no CPU, which is the whole reason this module exists.
    #[test]
    fn sleeping_costs_no_processor_time() {
        let before = Cpu::now();
        std::thread::sleep(Duration::from_millis(30));
        let used = Cpu::now().since(before);
        assert!(
            used < Duration::from_millis(5),
            "sleeping should not look like work, got {used:?}"
        );
    }

    /// Work does show up, so the clock is not simply stuck at zero.
    ///
    /// The work is doubled until the clock registers it rather than fixed at four million
    /// multiplies, because a fixed batch asserts the clock's *resolution* and not the thing this
    /// test is named for. Windows reports thread CPU time in scheduler ticks of about 15.6 ms (see
    /// [`Cpu::now`]'s own note there), so four million multiplies really did read exactly zero on
    /// both Windows runners the first time this suite ran anywhere but Linux, while the clock
    /// underneath was working correctly.
    ///
    /// A clock that is genuinely stuck still fails: the loop gives up after a wall-clock second,
    /// which is two orders of magnitude past even Windows' quantum.
    #[test]
    fn work_costs_processor_time() {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut rounds = 4_000_000u64;
        loop {
            let before = Cpu::now();
            let mut total = 0u64;
            for i in 0..rounds {
                total = total.wrapping_add(i * i);
            }
            std::hint::black_box(total);
            if Cpu::now().since(before) > Duration::ZERO {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "a whole second of real work registered no processor time at all: the clock is stuck"
            );
            rounds *= 2;
        }
    }
}
