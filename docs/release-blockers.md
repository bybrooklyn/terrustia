# What is not done for v0.0.1

A point-in-time audit of the v0.0.1 release bar, taken 2026-09-04 after a large parity wave, because
"is it ready" was answered too generously twice in one day and both times the evidence said otherwise.

`TODO.md` stays the rolling backlog. This file is narrower and does one thing: for every clause of the
release bar and every known defect, it records what was actually checked, what the evidence was, and
who has to do something about it. Every claim here carries a file and a line, or a command whose
output can be reproduced. Nothing here is a plan; the plans live in `TODO.md`.

Three separate "done" claims turned out to be stale or wrong on the day this was written, so the rule
followed throughout is: a status word in a document is not evidence. Only code and command output are.

## How the bar is judged

`TODO.md`'s Phase 2 names the release bar. Each clause below is marked:

- **MET**: reproducible evidence in the repository or from a command anyone can rerun.
- **UNMET**: checked, and the evidence says no.
- **NOT RUN**: the check requires hardware, a real game client, or a quiet machine, and has not been done.

## The release bar, clause by clause

| Clause | State | Evidence |
|---|---|---|
| `just check` green (fmt, clippy x2, deny, web build, workspace tests) | MET | CI green on both jobs across the last six pushes; `just check` locally ends `All checks passed` |
| `just check-data` green (drops, recipes, parity, spawn-reach, dead-writes, mutants) | MET | All six run 2026-09-04; every mutant target inside its own survival budget |
| Zero unknown protocol IDs | MET | `docs/packet-ids.tsv` has no `unknown` row and no row that is `none`/`none` |
| Fuzzing green | MET | `fuzz/artifacts/` is empty; both targets run per-push in CI |
| p99 tick under the 16.67 ms budget at 255 players | MET | `TODO.md`'s soak table, four runs, with a neutralised control run proving the `BiomeCache` cap is what holds it |
| **Peak RSS under 1 GiB at 255 players** | **UNMET** | Same table: run 2 reached 1536 MiB. One run of four passed cleanly. See "The memory ceiling" below |
| **Differential against a real `TerrariaServer`** | **NOT RUN** | No `.trcap` capture exists anywhere in the tree. See "The differential" below |
| Test suite on every release platform | MET, 2026-09-05 | The three host-native matrix entries now run the suite for real. Closing it cost four bug fixes; see "What running the tests on Windows found" below |
| Human fresh-world Moon Lord playthrough | NOT RUN | Waivable by `TODO.md`'s own wording, but only "if the automated and differential evidence is otherwise complete", and the two rows above say it is not |
| README comparison table against the real server | NOT RUN | `tools/compare_vanilla.sh` had a real measurement bug fixed 2026-09-04 (it read the macOS launcher's pid, not the server's); it now needs a quiet machine, and its own contention gate refuses to publish otherwise |
| Extended multi-hour boss soak | WAIVED | Explicitly carried to the next release (`TODO.md` Phase 2), recorded rather than skipped quietly |

## What running the tests on Windows found

Recorded because it is the strongest argument in this file for keeping a gate rather than waiving
one. The suite had never run anywhere but Linux. Turning it on found four real defects on a platform
this project publishes binaries for, three of them user-facing, and none of them detectable from
Linux:

1. **`copy_atomic` had never once succeeded on Windows**, so no Windows server had ever made a world
   backup. `sync_all` is `FlushFileBuffers` there, which the API documents as needing
   `GENERIC_WRITE`, and the code reopened its temporary file read-only on purpose, with a comment
   explaining that `fsync` needs no write access. True on unix; false here. `rotate_backups` logs and
   carries on by design, and the world itself saved perfectly, so nothing ever looked wrong: the one
   safety net for a bad save simply was not there.
2. **Windows ARM64 could not self-update.** `release.yml` publishes that binary and `target_triple`
   had no arm for it, so those users were told there was no build for their platform.
3. **Ctrl+Break terminated the server without saving.** `stop_signal` caught the close and shutdown
   events on the stated reasoning that a managed stop is not Ctrl-C; Break is the same class and was
   missing.
4. **The CPU clock reads zero for sub-quantum work**, because `GetThreadTimes` is updated on the
   scheduler's ~15.6 ms tick. Not a bug in the clock, but it means a per-tick CPU figure on Windows
   is quantised against a 16.67 ms budget and cannot be compared with a unix host's. Disclosed at
   `Cpu::now`; `QueryThreadCycleTime` is the fix and needs a calibrated cycles-per-second, so it is
   named here rather than guessed at.

Two tests also could never have passed there: both killed the child with `TerminateProcess` and then
asserted on a save that only a graceful stop performs, which is the exact mistake one of them warns
about in its own comment for the unix side. Both now spawn into their own process group and send a
real `CTRL_BREAK_EVENT`.

## The memory ceiling

The clause is "peak server RSS under 1 GiB at 255 players". It is not met, and the reason is structural
rather than a tuning miss.

`net::connection` gives every player an outbound queue of 4,096 frames, chosen to stop drops and right
for the retention clause. `queue_peak` reports only the deepest single connection, so it under-reports
the total: 255 slots times 1,052,672 frames is a ceiling in the tens of gigabytes, and **nothing bounds
the sum**. Which side of the ceiling a run lands on tracks how contended the machine is, not how the
server behaves: peak RSS ran 1536, 600, 206 and 169 MiB against external-stall counts of 35, 10, 1 and 3.

That is a design decision nobody has taken yet, not a bug to fix. The options are a global byte budget
across all queues, a smaller per-connection queue traded against the retention clause, or an explicit
shedding policy. `TODO.md` states the tension honestly; what it does not have is a decision.

## The differential

`docs/real-client.md` exists to explain why this one check cannot be replaced by any test we write:
`terrustia-client` and the server both encode through `terrustia-proto`, so a shared misreading is
invisible to every green test. The audits already found four defects of exactly that shape (a tile id
off by six, ore tiers shifted a slot, a dungeon coordinate that was silently the surface).

No capture has ever been recorded. `find . -name '*.trcap'` returns nothing. The tooling is built and
documented; it has simply never been pointed at the real game.

## Known bugs, found and deliberately not fixed

Each of these is disclosed in a comment at its own site, which is how they were found. They are listed
worst first by what a player or operator would actually notice.

1. **Cave topology is not vanilla's** (`crates/terrustia/src/world/worldgen/structures.rs:1239-1247`).
   The carver is this project's own wandering-tunnel algorithm, and it produces one large interconnected
   network where vanilla produces a mix of isolated pockets and large caverns. Gem cave and spider cave
   siting depend on that distinction and are wrong because of it. The comment records that fixing it
   means reworking `caves()`'s topology, "a materially bigger and riskier change", and that it was
   flagged rather than attempted. This is the most serious entry in this file.
2. ~~An actuator toggle is lost inside one wire flood.~~ **Fixed 2026-09-05.** Both stone-block arms
   rewrite from a fresh read now, so a tile that is both an Active Stone Block and actuated keeps
   both changes; `an_actuated_active_stone_block_keeps_both_changes` pins it, and neutralising the
   fix fails it on the vanished toggle alone.
3. ~~`follows_boss` is broader than vanilla's segment gate.~~ **Fixed 2026-09-05.** The derivation
   has a name now (`shares_a_life_pool`) and answers vanilla's actual question, so Skeletron's hands
   and Golem's fists are drained like the separate NPCs they are.
4. **Self-update cannot replace the running binary off Unix**
   (`crates/terrustia/src/update.rs:58-60`). The operator has to finish the update by hand on Windows.
   Separately, and now fixed: Windows ARM64 could not self-update *at all*, because `target_triple`
   had no arm for a target `release.yml` has been publishing.
5. ~~One bad Journey-power id in a `.wld` silently defaults every power after it.~~ **Addressed
   2026-09-05**, though not by changing what it does: stopping the read is correct, since an id whose
   payload width is unknown cannot be stepped over without misreading everything after it. It now
   names the id and says what that costs, so the loss is diagnosable rather than silent.

## Player-visible gaps, disclosed in code

Not defects; deliberate narrowings that a player would nonetheless notice.

- Player **luck** is modeled nowhere (`crates/terrustia/src/game/spawn.rs:400`), so no luck item ever
  changes a spawn rate or a drop rarity.
- **Fallen Stars** and the surface fairy mechanic are unmodeled
  (`crates/terrustia/src/game/server/mod.rs:1258-1265`, `systems.rs:5150-5154`).
- **Moon Lord's** hand brand-then-blob mechanic and its true countdown timer are unmodeled
  (`crates/terrustia/src/game/ai/boss/moon_lord.rs:299,505`).
- ~~Old One's Army has no client-visible progress bar.~~ **Fixed 2026-09-05**: it rides packet 78
  with its own icon 3 and its wave number, as `DD2Event.cs:185`/`:191` do.
- **Frost and Pumpkin Moon wave-gated drops** are flattened to guaranteed picks rather than gated on
  the live wave number (`crates/terrustia-proto/src/conditional_drops.rs:706-758,1148`).
- **Town-NPC attack windups** are skipped and the Pirate's escalating burst is unmodeled
  (`crates/terrustia/src/game/ai/town_combat.rs:13-28`).
- **Slime Rain** collapses its per-type flags to one case and does not announce start and stop
  instantly (`crates/terrustia/src/game/slime_rain.rs:39,84`).
- **Lantern Night's** manual-forcing toggle is unmodeled (`crates/terrustia/src/game/lantern_night.rs:43`).

## Structural

- **Three proto tables have no generator.** `crates/terrustia-proto/src/npc_data.rs:6-11` says so in its
  own header: "There is no generator for this file". `tile_object.rs` and `placed_items.rs` are the
  same. Rule 7 in `AGENTS.md` says generated tables are codegen output that is never hand-edited; these
  three are hand-maintained, so the rule does not currently hold for them.
- **Lane B (error handling and data safety) is the only lane in `TODO.md` with no "(done)" marker**, and
  485 `.unwrap()` calls remain in production files (`net/listener.rs`, `net/codec.rs`, `world/wld.rs`,
  `world/wld_save.rs`, `admin/audit.rs` and others). The lane's claim is scoped to paths the outside
  world can trigger, so the count alone proves nothing either way. What is missing is any way to
  *demonstrate* the claim: no checker distinguishes an internal-invariant unwrap from a reachable one.
- **The flaky-test root cause is still undiagnosed.** `tests/shutdown_signal.rs`'s sigterm test and
  `new_world_cli` share a failure shape, measured at roughly one run in five on both the pre-session
  base and current `main`. The next diagnostic step named in `TODO.md` (stat the binary immediately
  before spawning, record inode and mtime on failure) has not been run.

## Documents that were wrong

Recorded because the pattern matters more than the individual fixes: on 2026-09-04, three backlog
entries and one planning document were found stating things the code contradicted.

- `TODO.md`'s DESERT and TAXCOLLECTOR entries both described gaps that were fully implemented, the
  latter predicting infrastructure ("a general item-vs-live-NPC interaction, which nothing in this
  server currently has") that already existed at `dispatch.rs:3869-3889`.
- A planning document claimed eighteen boss AIs were unbuilt. Every one of them exists, several at over
  a thousand lines.
- `crates/terrustia/src/game/journey.rs:55-58` claims Stop Biome Spread "has nothing to gate yet" and
  that this project "does not model corruption/crimson/hallow tile spread at all". Both halves are
  false: `systems.rs:5519-5524` implements it and `systems.rs:7692-7706` tests the gate.

The checkers caught none of these, because none of them check prose. `docs/spawn-gaps.tsv` would have
caught the first two the day they went stale, had anyone diffed it.
