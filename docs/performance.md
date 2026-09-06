# Performance

A tick has **16,666 µs**. Everything here is measured against that number; anything approaching
it is a bug rather than a tuning problem.

## Measuring

```sh
cargo run --release -p terrustia --example savecost -- world.wld    # what a save costs
cargo run --release -p terrustia --example stress   -- 127.0.0.1:7777
cargo run --release -p terrustia --example crowd    -- 127.0.0.1:7777
cargo run --release -p terrustia --example memreport
cargo run --release -p terrustia --example profile_ai
```

The server reports its own worst tick every ten seconds at `debug`, and warns at `info` when a
tick uses more than half its budget:

```
WARN ticks are using a lot of their budget worst_us=70905 budget_us=16666 phase=world ...
```

The phase in that line is the point. "A tick took 71 ms" is a mystery; "the world phase took
71 ms with no NPCs in it" is a bug report.

## Where the tick goes

Phases, in `Phase`'s own declared order (`game/server/tick.rs`): `Snapshot`, `Liquids`, `Growth`,
`Spread`, `Weather`, `World`, `Sections`, `Items`, `Npcs`, `Projectiles`, `Damage`, `Spawning`,
`Housing`, `Sync`. Fourteen, not the nine this line used to list - and the five it was missing were
the five that run first, `Snapshot` among them, which is the phase this page's own headline finding
is about.

With 24 players and a busy world, the worst tick sits around **350–520 µs** — about 3% of budget.
The roster is not the problem and has not been.

## Large world, 16 and 255 players

The measurement above is on this project's usual 4200×1200 world — every prior number on this page
is. To find an actual ceiling rather than re-measure the same size again, this ran against a real
8400×2400 world instead (vanilla Terraria's own "Large" preset, 20.16M tiles — this codebase has no
named `LARGE_WIDTH`/`HEIGHT` constant, only `SMALL_WIDTH`/`HEIGHT`, so this is the first time it has
generated one), `--release`, seed 999. `examples/crowd` against it, real connections, `autosave_secs
= 0` so the number reflects player count rather than the separately-documented autosave cost below.

**Original pass** (world generation: 28.171 s):

| Players | Worst tick (cpu_us) | % of budget | Dropped connections |
|---:|---:|---:|---:|
| 16 | 363–658 µs | 3.9% | 0 |
| 255 (the real protocol max) | 410–2,219 µs | 13.3% | 88 of 255 (34.5%) |

Tick cost passed cleanly even at the real 255-player ceiling — but 88 of the 255 connections were
dropped with `outbound queue full; dropping a client that cannot keep up`, all within the first
several seconds after the join burst. That pass disclosed the ceiling rather than fixing it (see
`net/connection.rs`'s `outbound_queue` sizing, `8,192 + 256 × max_players`) and guessed the cause
was a join-time presence-and-inventory relay burst compounding across 255 simultaneous newcomers.

**That guess was wrong, or at least badly incomplete.** The drop log already carried a
`packet`/`name` field naming exactly which packet overflowed each queue — it was sitting right
there, unchecked. Reading it: the drops are almost entirely `PlayerControls` (id 13, ordinary
movement) and a handful of `SyncNPC`, not the presence/equipment frames a join sends, and the
dropped slots spread uniformly across the whole 0-254 slot range rather than clustering among the
earliest joiners, which is what an unread join-time backlog would produce. The real mechanism: once
everyone is moving, `on_player_controls`'s broadcast relays every control packet to every other
player, unconditionally, roughly once a tick — genuinely O(n²) in player count, and unrelated to
*how* the population arrived. `OUTBOUND_PER_PLAYER` (256) was sized against the join-time theory's
own numbers (~200 frames per peer, paid once) — comfortably inside the old 73,472-frame queue — not
against steady-state movement relay, which is what actually overflowed it. See
`net/connection.rs`'s own doc comment on `OUTBOUND_PER_PLAYER` and the pre-roadmap ledger's
corrections section (`plan.md`, in git history) for the full account.

**The fix**: `OUTBOUND_PER_PLAYER` is now 4,096. The depth formula's shape did not need to change,
only its per-player calibration — real, disclosed simplicity over cleverness. This is a mitigation,
not a cure: the O(n²) relay cost is real and is vanilla's own behaviour (this project transcribes
it rather than inventing a throttle vanilla does not have), so a deeper queue buys headroom without
removing the underlying cost. A genuine root-cause fix — not relaying movement to a peer who cannot
possibly render it, the way NPC sync already skips a client whose loaded sections do not cover it —
would touch `on_player_controls`'s own broadcast in `game/server.rs`, out of scope for a
`net/connection.rs` queue-sizing fix; left for whoever owns that file next.

**Re-measured after the fix**, same world (seed 999, `--release`, `autosave_secs = 0`; this
particular re-run generated the world in 8.9 s rather than 28.171 s — this machine was measurably
less loaded this time, the same kind of environmental variance this page discloses in the other
direction below):

| Players | Worst tick (cpu_us) | % of budget | Dropped connections |
|---:|---:|---:|---:|
| 16 | 339–937 µs | 5.6% | 0 |
| 255 (the real protocol max) | 295–3,451 µs | 20.7% | 0 |

Zero drops at the real 255-player ceiling. Tick cost is still a clean pass, with better than 4.8×
headroom — worse than the original pass's "7×", because that number was computed only from windows
*after* 88 connections had already been dropped and the surviving population had thinned to ~167;
this number is the honest one, measured across the full, undiminished 255-player run. Real stall
was as high as 107,809 µs on some windows; this machine was independently confirmed via `ps`,
mid-run, to be running a second concurrent agent session's own `cargo test` — so, as with every
other stall number on this page, the `cpu_us` figures above are the ones a quiet machine would also
show, and are what the pass/fail verdict rests on.

Calibration, not a blind guess: measured on a small 800×600 world across repeated real `--release`
two-process `examples/crowd` runs, `OUTBOUND_PER_PLAYER = 1,024` (4×) still left occasional drops
under real contention (5 of 255 over one 90-second run); `4,096` (16×) measured zero drops across
every trial, including a full 90-second run under real machine contention and the official
255-player large-world re-run above.

A regression test, `tests/queue_capacity.rs`, reproduces this against a real `terrustia`
subprocess — not an in-process mock, which was tried first and measured not to reproduce the drop
at all, because sharing one `tokio` runtime between the server and 255 simulated clients does not
recreate two independently-scheduled OS processes competing for the same cores the way a real
deployment does — using a deterministic packet-count burst rather than wall-clock-paced movement
(also measured to matter: real-time pacing was unreliable specifically under `cargo test`'s own,
differently-loaded profile). Verified failing on the unfixed code (31 of 255 dropped) and passing
on the fix (zero drops, repeatedly).

## World generation time, and what actually parallelizes safely

The section above measured a real 8400x2400 world at 28.171 s to generate, wall-clock, on this
session's own shared machine. That number is real — it happened, on a real clock — but re-running
the identical work (same seed, same size, so the same sequence of RNG draws and therefore the same
amount of algorithmic work every time) gave wall-clock totals from **8.4 s to 41.0 s** across five
back-to-back trials on the same machine, at the same commit. That is not seed-dependent variance;
it is contention. `ps aux` at the time showed a second, independent `cargo test` running in a
sibling worktree, plus a desktop's worth of ordinary GUI processes (Steam, WindowServer) — this
machine is shared, and its own load average sat at roughly 1.5-2x its core count for most of this
investigation.

Wall-clock time is the wrong tool for measuring this codebase's own cost on a machine like that —
`game/clock.rs`'s own doc comment already makes this exact point about tick timing, and it applies
just as well to a one-shot generation run. Re-measured on this thread's CPU clock instead
(`game::clock::Cpu`, the same mechanism the tick loop already uses), via `examples/gencost.rs` (new
this session, a per-pass profiler mirroring `build()`'s own call sequence), three trials each:

| World | wall-clock spread | CPU-time spread | CPU-time median |
|---|---:|---:|---:|
| 4200x1200 (seed 999) | 1.3-2.0 s | 1.8-2.1 s | ~1.9 s |
| 8400x2400 (seed 999) | 8.4-41.0 s | 7.9-12.6 s | ~8.6 s |

CPU time for the large world is about 4.5x the small world's — almost exactly the 4x tile-count
ratio between them. **World generation scales linearly with tile count.** The appearance of wild
superlinear scaling in an earlier wall-clock-only comparison of these same two sizes (a single
small-world sample against a single large-world sample showed individual passes apparently costing
36x to 546x more for a 4x tile increase) was a comparison of a low-contention moment against a
high-contention one, not a property of the code. Worth stating plainly, since it would have been
easy to chase as a bug: there is no algorithmic blowup here, once measured honestly.

### Where the time actually goes

Per-pass CPU time on the large world, seed 999 (`gencost.rs`, ranked by cost): `smooth::smooth`
(roughly a fifth of the total on its own), `waterfalls::scatter`, `dirt_wall_cleanup::scrub`,
`oasis::scatter`, `pots::scatter`, `wall_variety::variety`, and the `tile_cleanup` bundle together
account for most of the rest. Every one of these is what it looks like: a real full-world scan or a
real retry-based site search, run once, at the size the world actually is — not a hidden quadratic
term hiding behind a small line count.

### What's actually safe to parallelize, and what isn't

Every pass in `build()` was checked for genuine, safe parallelism — at the pass level (independent
passes running concurrently) and within a single pass (splitting its own scan across threads).

**Pass-level parallelism is not safe, and was not attempted.** `build()`'s own module doc already
says ordering is load-bearing for *world state* (caves before ore, structures before decorations);
what rules out running independent-looking passes concurrently is a second, separate dependency
this investigation turned up by reading every pass's own signature: nearly every one takes
`&mut rand` (`UnifiedRandom`) or `&mut forest_rng` — one shared, strictly-ordered RNG stream
threaded through the whole ~50-pass sequence. Two passes running on different threads would each
need their own RNG state to be sound at all, which means reseeding, which means every pass
downstream of the split draws different random numbers than it does today. That is not an
implementation detail — it would silently change the *already-measured, already-published*
per-seed counts this project's pre-roadmap ledger (`plan.md`, in git history) carries for roughly twenty Tier 2/3 passes
(jungle shrines, pyramids, cabins, ruins, speleothems and more, each pinned against seeds
999/4242/12345). Chasing pass-level concurrency here would mean re-verifying every one of those
rows for a codebase-wide behaviour change well outside this task's own brief.

**Intra-pass parallelism is safe for exactly one pass**, found by checking every big-cost candidate
for two properties: no shared RNG, and no cross-column read. `tile_cleanup::gravitating_sand_cleanup`
is the only one of the six biggest passes with neither — checked by reading every line, not
assumed: it takes no `rand` at all, and every read and write it makes is to its own column (`x`
never varies within the inner loop, and there is no `x-1`/`x+1` anywhere in it). Every other big
pass fails at least one of the two: `dirt_wall_cleanup::scrub`, `waterfalls::scatter`,
`oasis::scatter`, `pots::scatter` and `wall_variety::variety` all draw from the shared RNG inside
their own loop; `quick_cleanup`/`tile_cleanup`/`final_cleanup` are RNG-free but each reads a
horizontal neighbour (`x-1`/`x+1`) whose value, inside the current sequential pass, can itself
already have been modified earlier in that same run — genuinely ambiguous to parallelize safely
without either a fresh read of vanilla's own source (unavailable this session — the decompiled tree
was wiped by a tmp-reaper earlier in this project's history, see the pre-roadmap ledger in git
history) or an expensive
per-pass empirical proof that a snapshot-based rewrite changes nothing; `broken_trap_cleanup` is a
wire-circuit flood that can span the whole world, not local to any column range at all.

`gravitating_sand_cleanup` now splits `0..width` into one column band per available core
(`std::thread::available_parallelism`), computes each band's writes against a read-only `&World`
on its own thread — sound without `unsafe` or a lock, since `&World` has no interior mutability and
sharing it across threads is already permitted — and applies every write on the calling thread once
all workers finish. Application order cannot matter: `set_tile` during generation is a plain array
write (`track_dirty` is off until well after `build()` returns — see `World::set_tile`), and no two
columns can ever compute a write to the same tile.

Measured, this one pass alone, wall-clock (the number that matters for how long the single-writer
thread would be blocked if this ran mid-game — CPU time undercounts a threaded call, since the
calling thread mostly waits on `.join()` rather than computing):

| World | before (single-threaded) | after (parallel) | speedup |
|---|---:|---:|---:|
| 4200x1200 | 53 ms | 7.3 ms | ~7.3x |
| 8400x2400 | 251-314 ms | 34-44 ms | ~7-8x |

Real, with the honest caveat sitting right next to it: this pass was never among the pipeline's
biggest costs — roughly 2-6% of a large world's total build time before this change — so the effect
on the headline "how long does a Large world take to generate" number is real but small, not a
rewrite of the 28-second figure above. The passes that actually dominate (`smooth`, `waterfalls`,
`dirt_wall_cleanup`, `oasis`, `pots`, `wall_variety`) are exactly the ones this investigation found
genuinely unsafe to parallelize without either breaking RNG-order determinism or risking a silent
behaviour change nobody could fully verify this session. Left for whoever next has reason to chase
generation time further, with the reasoning already done rather than left to rediscover.

**Verified bit-identical, not just argued safe.** `tile_cleanup.rs` gained two new tests:
`many_independent_columns_parallelize_to_the_same_result_as_one_thread` (a synthetic wide world
with a floating-sand pocket in most columns, deliberately spanning several thread-band boundaries)
and `gravitating_sand_cleanup_is_bit_identical_to_a_single_threaded_reference_on_real_worlds` (the
real pipeline through `structures::ores`, three real seeds, full tile-for-tile comparison). Both
were checked to actually discriminate rather than pass by construction: a deliberate two-column gap
injected into the band-boundary math failed the synthetic test immediately (206,064 vs 207,008
tiles dropped), while the real-seed test — real "gravitating sand" sites are sparse — did *not*
reliably catch the same injected bug, which is why both tests are kept rather than either alone.

## Section encoding at join — measured, real, and fixed

`send_section` (`game/server.rs`) turns a section of tiles into the packet a client receives, and
runs inline on the single-writer game task — the same task every tick runs on. It caches what it
encodes (`section_cache`) and only re-encodes a section once a tile inside it has changed since it
was last sent, so ordinary gameplay pays this rarely. The place it cannot avoid paying it is a
join: `on_spawn_tile_data` sends every section in `sections_for`'s own starting block — up to a 5x3
block around spawn (15 sections) plus a 6x4 block around the joining player's own requested
position (24 more, if it names a real location) — synchronously, one `send_section` call after
another, inside one event. That is a guaranteed cache miss for whichever sections a server's very
first player ever spawns into, and a likely one for anyone spawning somewhere the cache hasn't been
warmed yet.

`examples/sectioncost.rs`, new this session, samples every section of a real generated world rather
than the one convenient section near spawn that `bench.rs` already measures (spawn is cleared, so
it is unusually cheap to encode):

| World | sections sampled | min | p50 | p99 | max | max as % of a 16,666 µs tick |
|---|---:|---:|---:|---:|---:|---:|
| 4200x1200 | 168 | 120 µs | 277 µs | 1,075 µs | 1,322 µs | 7.9% |
| 8400x2400 | 672 | 121 µs | 240 µs | 2,856 µs | 2,976 µs | 17.9% |

One section — even the worst one measured, on the larger world — is not a stall on its own; nowhere
close to the autosave's 71 ms. The real cost is the *burst*: multiplying a realistic 15-39 section
join by these real percentiles puts a cold join somewhere between roughly 15 ms (15 sections at the
small world's p50) and 115 ms (39 sections at the large world's p99) of synchronous work on the
single-writer task before that player sees a single tile — one to seven tick budgets, back to back.
The same *shape* of problem the autosave stall was: real synchronous cost, paid worst on a cold
cache.

This row once said "not fixed this session," with the code that would need to change
(`send_section`, `on_spawn_tile_data`, `sections_for`, all in `game/server.rs`) explicitly out of
scope while that file was single-owner with other in-flight work depending on it staying stable.
That constraint no longer applied once the session doing the work was the sole owner of the file
again, so it was picked up rather than left open. Unlike the autosave, this could not simply move to
a background task — encoding needs to read `World`, which the single-writer task owns for the whole
tick, so there is no snapshot-and-hand-off shortcut here the way there was for a save. Fixed instead
by *spreading* a join's own section burst across several ticks: `on_spawn_tile_data` now queues the
wanted sections onto a new `Player::pending_sections` and returns, rather than looping over
`send_section` inline; a new `drain_section_streams`, run from the tick loop in the same phase
`flush_dirty_sections` already occupies, advances every streaming player's own queue by a
time-boxed slice (`SECTION_STREAM_BUDGET`, 4,000µs) each tick, with the budget shared across every
player streaming at once rather than given to each one separately — a per-player budget would let a
burst of simultaneous joins reproduce the exact stall this fixes, just triggered by many joiners
instead of one. The items/NPCs/`TilesSent`/`INITIAL_SPAWN` steps that used to run right after the
loop now run once a player's own queue actually empties, in a new `finish_join_stream`. A joining
player was already sitting on a loading screen for the whole burst regardless, so a few extra ticks
of load time costs them nothing visible, in exchange for never freezing everyone else already
playing. See the pre-roadmap ledger's Done entry for this fix (`plan.md`, in git history) for the full writeup and its three pinned
tests. `section_cache` itself was never the problem — it does exactly what this page already
documented it doing; the problem was only ever the first, uncached pass through a join burst, now
paced rather than synchronous.

## The autosave stall, and how it was found

The crowd test reported this:

```
worst_us=70905 budget_us=16666 phase=world npcs=0 projectiles=0
```

Seventy-one milliseconds, in the world phase, with **nothing in the world**. And exactly every
five minutes. That is the autosave, and it was running on the game task.

Every autosave dropped three or four ticks. Players see that as a stutter, and it gets worse the
larger the world.

### What it actually cost

`examples/savecost.rs` exists to answer that, because the first guess was wrong:

```
world       4200x1200, 5040000 tiles

  reading every tile and nothing else       8254 µs
  ...plus finding the runs                 30924 µs
  ...plus encoding them (the whole job)    54667 µs

serialise      54667 µs
write            906 µs
```

Two things fall out of this, and both are worth keeping:

**The write is nothing.** 906 µs of 55,573. Moving only the disk write off the thread — the
obvious first move — would have bought 1.5%.

**Reading the tiles is nothing either, and that is surprising.** The encoder walks *column-major
through a row-major array*, striding 50 KB per step on a large world. That looks exactly like a
cache disaster, so the first fix was to transpose bands of columns into a scratch buffer before
encoding them.

It changed nothing measurable — 54.8 ms against 55.6. Reading all five million tiles costs 8 ms,
1.6 ns each; the hardware prefetcher handles a constant stride perfectly well. The transpose was
reverted and the reason recorded in `write_tiles`, so the next person does not spend the same
afternoon on it.

The cost is split roughly evenly between spotting runs (23 ms) and encoding them (24 ms).

### The fix, and how far it got

Even a threefold faster encoder would still bust the budget, and a bigger world would bust it
again. So the save moved off the game task entirely:

1. The tick takes a **snapshot** — `World::snapshot()`.
2. A blocking task serialises and writes it.
3. The result comes back down a channel, which the tick polls. It cannot be awaited, because the
   tick is not async and should not become so for this.

The snapshot is **atomic with respect to the tick**, so a save can never catch the world halfway
through an edit. A torn save is much worse than a slow one.

Measured on a 4200×1200 world, with players connected:

| | Before | After |
|---|---:|---:|
| On the tick | 71 ms | **32 ms** |
| Off the tick | — | 63 ms |
| Ticks dropped per save | ~4 | ~2 |

Better, and honestly still over budget. The remaining 32 ms is the copy: eighty megabytes at
about 2.5 GB/s.

### The buffer experiment, which failed

The obvious next step is to keep the snapshot buffer between saves so its pages stay warm, and
copy into it rather than allocating. That was built, measured, and **reverted**: successive saves
went 33 ms, then 41, then 45.

The reason is visible in `ps`: an idle server's RSS is 27 MB even though its tile array is 80 MB,
because the OS pages a quiet world out. A second eighty-megabyte buffer makes that pressure
worse, so more of the live world is evicted, so each save faults more of it back in. The buffer
cost more than it saved and doubled the footprint doing it.

The measurement is the only reason this is known. The hypothesis was reasonable and wrong, which
is the third one on this page.

### What would actually finish it

Two options, neither small:

- **Shrink `Tile`.** It is fifteen bytes of fields padded to sixteen. Eight would halve the copy
  to about 16 ms and halve the world's memory with it. `examples/memreport` prints what six and
  eight bytes would buy.
- **Snapshot incrementally.** `World` already tracks dirty sections for the network. A persistent
  shadow copy updated a few sections per tick would make the snapshot at save time nearly free.
  This is the proper fix and the subtle one: the shadow has to be consistent at the moment the
  save takes it, not merely up to date on average.

Two rules fall out of that arrangement:

- **One save at a time.** A request arriving while one is running is *dropped*, not queued. Two
  saves racing for the same path is worse than a missed autosave, and a server whose disk cannot
  keep up should not build a backlog of sixty-megabyte snapshots.
- **Shutdown waits.** The shutdown save is synchronous and runs after any background save has
  finished. Both write through a temporary file and rename, so neither can leave a half-written
  world — but the shutdown save has the newer state and must land last, and two renames racing
  would settle that by scheduling rather than by which is newer.

## Other things that have been measured and are fine

- **NPC sync.** Sending every NPC to every player is thousands of frames a second per client.
  The game's rule is followed instead: skip an NPC for a client whose loaded sections do not
  cover it, but never more than four times in a row, so something far away still updates
  occasionally rather than freezing where it was last seen.
- **Section caching.** Encoded sections are cached and invalidated by tile writes. Tracking is
  off during generation and loading, where every tile is written once — five million set inserts
  there would cost more than the cache saves.
- **Per-tick scans.** Several AI routines want a survey of the roster. Each such list is built
  once per tick and only when something present actually reads it.
- **Buffs.** The whole buff pass returns immediately when nothing anywhere is buffed, which is
  the ordinary case.
- **Progress packets.** Pillar shields, invasion progress and the Moon Lord countdown are
  recomputed every tick and almost never change, so each is compared against what went out last.

## The outbound queue, which was a tenth of the size it needed to be

Each connection has a bounded queue of frames waiting to be written, and a client whose queue
fills is removed — the assumption being that anything that far behind has stopped reading.

It was a flat 512, chosen against "the initial world burst is around forty section packets". That
counted the sections and nothing else. The burst a joining player is actually sent is:

- around 39 sections,
- one frame per item on the ground, up to 400,
- one per live NPC, up to 200,
- for every *other* player already in the world: their presence frames, **and one frame per
  relayed inventory slot**, because equipment sent before the newcomer arrived has to be replayed
  for them. That is roughly two hundred frames per player.

So the burst scales with how busy the server is, and on a populated world it passes 512 easily.
The failure is not a slow client: it is the *newcomer* being dropped, mid-load, with no message,
intermittently, and looking exactly like a network problem at their end.

The depth is now `1024 + 256 × max_players`, sized from the config rather than guessed. The real
bound is memory rather than count — the sections dominate at tens of kilobytes each while the rest
are tens of bytes — but a count is what a channel takes.

This section is the history of the *first* fix to this queue, kept as the record of what was
known and why at the time — both the base term (now `OUTBOUND_BASE = 8192`, raised again since to
cover chest-heavy sections; see that constant's own doc comment) and the per-player term (now
`OUTBOUND_PER_PLAYER = 4096`, not 256) have moved on since. The per-player term's own move is a
*different* mechanism than anything described above — steady-state movement broadcast, not a
join-time burst — see "Large world, 16 and 255 players" above for the full account.

## A section that failed to encode was lost for good

`send_section` recorded a section as delivered *before* encoding it, using the return of
`sent_sections.insert`. If the encode then failed, the section was marked sent anyway, and every
re-request the client made was dropped by that same dedupe.

The symptom is a 200×150 hole of untextured sky that never fills in however many times the player
walks back through it, and the only trace is one `debug!` line. It is reachable: a compressed
section over 65535 bytes cannot be framed, and because this server batches runs more strictly than
the game does, its sections are systematically larger than vanilla's for the same world.

Membership is now checked first and claimed only once the bytes exist, so a failure retries. The
log line was also raised to `warn!`, since the symptom is missing world rather than anything that
looks like an error.

## Memory

`examples/memreport`, on a 4200×1200 world:

```
baseline RSS          1.8 MB
size_of::<Tile>()      16 bytes
tile array           80.6 MB
process RSS          78.9 MB
```

The tile array is essentially all of it. A `Tile` is fifteen bytes of fields padded to sixteen;
memreport also reports what eight or six bytes would save, which is the obvious lever if memory
ever becomes the constraint.

A save briefly doubles it, since the snapshot is a second copy of the tiles. That copy is freed
as soon as the writer finishes, which is why the buffer-reuse experiment above — which would have
made the doubling permanent — was not worth its cost.

Note that RSS on an idle server reads far *below* the tile array's size, because the OS pages a
quiet world out. That is not a leak and not a saving; it is why a save on a long-idle server is
slower than one on a busy server.

## AI cost

`examples/profile_ai` runs every routine once and totals them:

```
685 routines, 108.3 µs if every one of them ran in the same tick
```

That is 0.65% of a tick for the entire roster acting at once, which cannot happen. The AI has
never been the constraint and this is the number that says so.

## Panic safety

A server that crashes is worse than one that stalls — and worse here than elsewhere, because the
game is a single actor task with no `catch_unwind` and the shutdown save runs *inside* it. A panic
does not degrade this server; it loses the world back to the last autosave.

Outside tests there are a small number of `unwrap`/`expect` calls, each on a proven invariant and
each carrying a message saying which: parsing the built-in default listen address, a buff-slot
search whose loop cannot exit without a slot, a boss routine that has just checked its target,
reading a fixed-width field out of a slice already length-checked, the worldgen layout's
best-candidate search, and — new this round — joining `gravitating_sand_cleanup`'s own worker
threads, safe because the closure they run only calls `World::tile` (which never panics) and
`Vec::push`.

**The count is pinned by `tests/panic_budget.rs` rather than stated here.** This paragraph used to
say "three", and named three, when there were seven — nobody had lied, the sentence was simply
written once and never revisited. A number in prose has nothing keeping it true; a failing test
does. Adding a panic site is now a deliberate act with a test to update.

Everything else that can fail returns a `Result` and is logged. The fuzzer (`examples/fuzz`) throws
twenty thousand malformed packets at a running server and checks it is still answering afterwards.
