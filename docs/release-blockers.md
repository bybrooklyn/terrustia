# What is not done for v0.0.1

A point-in-time audit of the v0.0.1 release bar, taken 2026-09-04 after a large parity wave, because
"is it ready" was answered too generously twice in one day and both times the evidence said otherwise.

`TODO.md` stays the rolling backlog. This file is narrower and does one thing: for every clause of the
release bar and every known defect, it records what was actually checked, what the evidence was, and
who has to do something about it. Every claim here carries a file and a line, or a command whose
output can be reproduced. Nothing here is a plan; the plans live in `TODO.md`.

Three separate "done" claims turned out to be stale or wrong on the day this was written, so the rule
followed throughout is: a status word in a document is not evidence. Only code and command output are.

**Refreshed 2026-09-06, and this file had itself gone stale in twenty-four commits.** Every claim in
it was re-checked against the code. Six were wrong: the Lane B unwrap count (off by about fifty
times, and every file it named has zero), the Moon Lord countdown entry (wrong citation, wrong
conclusion, and the real gap is a different and smaller one), the `new_world_cli` flake (closed
fifteen hours after this file called it open, and by a diagnosis this file contradicts), the
town-NPC ladder (named the Cyborg, which has no ladder), the `check-data` row (six parts named of
nine), and the journey.rs entry in "Documents that were wrong", whose own two citations had rotted.
Roughly fourteen player-visible fixes had landed with no entry at all.

That is twice now for this file, which is the argument for `tools/check_doc_citations.py`: every
`file.rs:NNN` above is content-keyed in `docs/doc-citations.tsv` and `just check-data` fails when
one stops pointing at what it was written against. Prose still has to be read by a person; a
citation no longer does.

## How the bar is judged

`TODO.md`'s Phase 2 names the release bar. Each clause below is marked:

- **MET**: reproducible evidence in the repository or from a command anyone can rerun.
- **UNMET**: checked, and the evidence says no.
- **NOT RUN**: the check requires hardware, a real game client, or a quiet machine, and has not been done.

## The release bar, clause by clause

| Clause | State | Evidence |
|---|---|---|
| `just check` green (fmt, clippy x2, deny, packet audit, web build, workspace tests) | MET, re-run 2026-09-06 | `All checks passed ✓`, exit 0, 24 test binaries and zero failures. **CI was not green when this was re-run**, and `just check` could not have told anyone: the `packet-audit` job had been failing since `9f056a2` because `docs/packet-ids.tsv` row 51 disagreed with the code, and `just check` did not run that script. It does now (`justfile`'s `check-rust`), so the two cannot diverge again |
| `just check-data` green (nine parts: drops, npc-data, placed-items, tile-object, recipes, parity, spawn-reach, mutants, dead-writes) | **PARTIALLY RE-RUN 2026-09-06** | This row said "all six run 2026-09-04" and named six of the nine. Three of the nine (`check-npc-data`, `check-placed-items`, `check-tile-object`) did not exist on that date, and `check-citations` landed later still and immediately found real defects (`2753a1d`). Re-run 2026-09-06: `check-parity` green (3,848 citations still pointing at the code they were checked against), `check-drops` green (375 of 378 NPC types, 40 of 40 deferrals on the record), `check-spawn-reach` green (the two known ids in `docs/spawn-gaps.tsv`). The other six have not been re-run since 2026-09-04 |
| Zero unknown protocol IDs | MET | `docs/packet-ids.tsv` has no `unknown` row and no row that is `none`/`none` |
| Fuzzing green | MET | `fuzz/artifacts/` is empty; both targets run per-push in CI |
| p99 tick under the 16.67 ms budget at 255 players | MET | `TODO.md`'s soak table, four runs, with a neutralised control run proving the `BiomeCache` cap is what holds it |
| **Peak RSS under 1 GiB at 255 players** | **MET 2026-09-06** | A full 30-minute hold at 255 players, all 255 held to the end and none shed. Peak 158 MiB against the 1 GiB ceiling, oscillating between 64 and 158 and ending below its start rather than climbing. It took fixing the harness to measure: two earlier runs died at 14:43 and 12:19 because the old one-process-per-player rig needed ~3.5 GB before the server allocated anything. A ceiling result, not a leak-freedom result. See "The memory ceiling" below |
| Differential against a real `TerrariaServer` | RUN 2026-09-05, passed on what it covers | 66,542 bytes of Re-Logic's own output re-framed with nothing left over; every id with an encoder re-encoded byte-identically, including all 15 `TileSection` frames. Coverage is partial by construction; see "The differential" below |
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

The clause is "peak server RSS under 1 GiB at 255 players". The cause of the failure was structural
rather than a tuning miss, and the structure has since been changed.

`net::connection` gives every player an outbound queue of 4,096 frames, chosen to stop drops and right
for the retention clause. `queue_peak` reports only the deepest single connection, so it under-reported
the total: 255 slots times 1,052,672 frames is a ceiling in the tens of gigabytes, and nothing bounded
the sum. Which side of the ceiling a run landed on tracked how contended the machine was, not how the
server behaved: peak RSS ran 1536, 600, 206 and 169 MiB against external-stall counts of 35, 10, 1 and 3.
Run 2's 1536 MiB was backlog spread across many connections rather than piled on one, which is exactly
the shape the per-connection reading cannot see.

**The decision taken (2026-09-05): bound the sum, keep the depth.** `OUTBOUND_PER_PLAYER` stays at
4,096 because its own comment records what it buys - a transient backlog behind a descheduled game
loop drains again afterwards, and a shallower queue turns that recoverable case into dropped players.
`QueuedBytes` charges every queued frame to a per-connection counter and to one shared by the whole
listener; past `OUTBOUND_TOTAL_BUDGET` (256 MiB, a quarter of the ceiling) the server sheds whichever
connection is holding the most, which is the same answer `send_bytes` already gave a single connection
whose own queue filled. Depth covers the transient, the budget covers the aggregate.

**What this does and does not claim.** It makes the queues a bounded contributor to RSS, which is the
one term that could reach the tens of gigabytes. It does not re-measure the gate: the soak wants a
quiet machine and has not been re-run since. Until it is, the honest statement is that the unbounded
term is bounded, not that the clause is met.

### Re-measured 2026-09-06, twice, and stopped by the harness rather than the server

Two runs at the full 255 players since the budget landed. Neither finished the 30-minute hold: the
operating system's memory watchdog killed the whole process group at 14:43 and 12:19.

| run | reached | server RSS over the hold (MiB) | peak | shutdown save |
|---|---|---|---|---|
| A | 14:43 | 95, 159, 331, 562, 336, 339, 256, 279 | 562 | clean, 146 ms |
| B | 12:19 | 65, 117, 185, 296, 223, 252, 209 | 296 | clean, 170 ms |

Both stayed comfortably under the 1 GiB ceiling, both **fell** after about six minutes rather than
climbing, and both took a clean world save on the way down with 255 players attached - which is the
"clean world save under load" clause getting incidental evidence it did not have before.

**What stopped them is the test rig, not the server, and the arithmetic says so.** `soak_scale.sh`
runs each simulated player as its own process. A single client was measured at **13.7 MiB** (24 of
them, 328 MiB total), so 255 of them need about **3.5 GB before the server allocates anything** -
against a server that peaked at 0.3 to 0.6 GB. On this 16 GB machine, with a qemu VM, two browsers
and the editor holding roughly 2 GB between them, that is what ran out.

So the clause is still not met, and the reason is worth separating from the thing being measured:
nothing here suggests the server has a memory problem, and two runs at full player count say the
opposite. Closing it needs either about 4 GB of headroom on the box, or a soak client that holds
many connections in one process instead of one each - the latter being the change that would make
this clause measurable on an ordinary machine rather than only on an empty one.

### Met 2026-09-06: the rig was fixed, and the hold finished

The second option was taken. `examples/soak.rs` gained a player count and holds each player as a task
on one runtime; `soak_scale.sh` launches one process rather than 255. The per-process overhead that
dominated the old figure is paid once instead of 255 times: at 24 players the rig went from 328 MiB
to about 40, and at 255 it peaks at 465 MiB where the fan-out needed roughly 3.5 GB.

With the rig no longer competing with the thing it measures, the 30-minute hold ran to completion on
the first attempt:

| t (s) | 0 | 120 | 240 | 360 | 480 | 600 | 720 | 840 | 960 | 1080 | 1200 | 1320 | 1440 | 1560 | 1680 | 1800 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| server RSS (MiB) | 95 | 77 | 77 | 158 | 94 | 158 | 149 | 149 | 158 | 117 | 149 | 132 | 66 | 64 | 64 | 65 |

**Peak 158 MiB against the 1 GiB ceiling**, over a full 30 minutes at 255 players, with all 255
holding to the end and none shed by the server. The curve oscillates between roughly 64 and 158 MiB
and ends below where it started, which is the plateau shape the clause asks for and not a leak. The
tick bar was met in the same run (179 samples, median 2314 us, p99 4934 us against a 16667 us
budget), the world saved clean on shutdown, and the box logged 2 external stalls.

Two things this does not claim. Thirty minutes cannot separate a slow leak from burst working set, so
this is a ceiling result and leak freedom belongs to the extended pre-release soak, exactly as
`soak_scale.sh`'s own comment says. And the earlier runs are not retroactively passes: they measured
a server that behaved well while the harness died around it, which is a different statement from a
hold that finished.

## The differential

`docs/real-client.md` exists to explain why this one check cannot be replaced by any test we write:
`terrustia-client` and the server both encode through `terrustia-proto`, so a shared misreading is
invisible to every green test. The audits already found four defects of exactly that shape (a tile id
off by six, ore tiers shifted a slot, a dungeon coordinate that was silently the surface).

**Run for the first time on 2026-09-05, and it passed on everything it could check.** A real
`TerrariaServer` (the Steam build on this machine) generated a 4200x1200 world on port 7930;
`conform` joined it and recorded the session, and then our own server was pointed at *the same
world file* and recorded again, so the two runs differ only in which server produced the bytes.

| | real `TerrariaServer` | terrustia |
|---|---|---|
| bytes captured | 66,542 | 64,100 |
| frames re-framed, nothing left over | 647 | 501 |
| distinct ids seen | 20 | 18 |
| re-encoded byte-identically | 17 | 17 |

The 17 are `WorldData` (2) and `TileSection` (15). `TileSection` is the compressed tile format and by
some distance the highest-risk layout in the protocol, so having Re-Logic's own bytes for it come back
byte-identical through our encoder is the single most valuable thing this check has produced.

**What it does not yet say.** 17 of 681 frames were byte-verified because `conform` only re-encodes ids
that have an encoder to re-encode with; the rest decoded cleanly but proved only that the layout is
plausible. And a session covers what it covers: this one was a bare join, so `SyncItem` and
`SyncItemDespawn` appeared on the real server and not on ours simply because nothing dropped an item in
our shorter window. That is a census difference, not a defect, and it is exactly why `conform` prints
the census rather than scoring it. Driving a longer session through real gameplay is what would widen
this, and is the obvious next step.

**Re-read 2026-09-06: "17" is also a limit of `conform`'s own arms, not only of the session.**
Four ids sit in its `decode_only!` list because a *client* never writes them, yet the proto types
all have encoders the server uses every tick - `SyncNpc` (`terrustia-proto/src/npc.rs:79`), `SyncProjectile`
(`terrustia-proto/src/projectile.rs:68`), `SyncItem` (`terrustia-proto/src/items.rs:122`) and `TileManipulation` (`terrustia-proto/src/packets.rs:1412`).
Promoting those four arms re-checks bytes already captured, before any longer session is driven.
And this whole table is now older than the encoder it measured: `terrustia-proto` has changed
underneath it since (`projectile.rs` +52 lines, `golf_physics.rs` +3,511), so the run wants
repeating regardless of what the arms do.

The captures are kept out of the repository on purpose: a recording of Re-Logic's own wire output is
game-derived data, and rule 2 in `AGENTS.md` keeps that out of the tree. They live under `.scratch/`.

## Known bugs, found and deliberately not fixed

Each of these is disclosed in a comment at its own site, which is how they were found. They are listed
worst first by what a player or operator would actually notice.

1. ~~Cave topology is not vanilla's.~~ **Fixed 2026-09-05, and the recorded diagnosis was half
   wrong.** The entry read: the carver is this project's own wandering-tunnel algorithm producing one
   large interconnected network, and gem/spider cave siting is wrong because of it. Two separate
   defects were tangled together there, and the one that actually caused the symptom was not in the
   carver at all.

   The symptom was that `cave_flood::count` saturated for every candidate site in the world. That was
   `terrain::fill`: it painted a background wall behind every underground tile, solid rock included,
   and `nextCount` (`WorldGen.cs:9539`) reads a tile's wall *before* it asks whether the tile is
   solid, so one wall on a pocket's own stone boundary rejects the site. Vanilla's terrain never puts
   one there (`DirtWallBackgrounds`, `WorldGen.cs:11895`, stops at `worldSurface + 0..10`, and no
   biome pass creates a wall where none was). Measured with the new instrument
   (`structures::cave_topology_measurement`) on three real worlds: **400 of 400 sampled fills stopped
   on a walled tile and not one ever reached the 3500-tile cap** the entry blamed.

   The topology was separately wrong, just not in the shape recorded. It was never one network:
   **74 to 80 connected components** in the deep band, largest holding 9 to 16 per cent of open space.
   What it lacked was small pockets, only **4 to 11** anywhere in the 50-to-300-tile window `GemCaves`
   sites into, against vanilla's thousands. `caves()` is now vanilla's own four passes driven by
   `TileRunner` (`SmallHoles`, `DirtLayerCaves`, `RockLayerCaves`, and the `Caverer` tail of
   `SurfaceCaves`); the same measurement now reads **4717 to 4837 components, 632 to 698** in that
   window.

   Both halves were needed and both are proven so: `GemCaves`/`SpiderCaves` run vanilla's whole size
   rule again, and `a_real_world_still_sites_gem_and_spider_caves_under_vanillas_own_size_rule` pins
   the full quota. Neutralising the terrain fix alone drops gem caves from 12 to 0; neutralising the
   carver alone drops them from 12 to 2. Carving costs about 200 ms more per small world
   (`terrain::fill` plus `caves()` went from a 50 ms floor to a 227 ms one, minimum of nine runs each).
2. ~~An actuator toggle is lost inside one wire flood.~~ **Fixed 2026-09-05.** Both stone-block arms
   rewrite from a fresh read now, so a tile that is both an Active Stone Block and actuated keeps
   both changes; `an_actuated_active_stone_block_keeps_both_changes` pins it, and neutralising the
   fix fails it on the vanished toggle alone.
3. ~~`follows_boss` is broader than vanilla's segment gate.~~ **Fixed 2026-09-05.** The derivation
   has a name now (`shares_a_life_pool`) and answers vanilla's actual question, so Skeletron's hands
   and Golem's fists are drained like the separate NPCs they are.
4. **Self-update cannot replace the running binary off Unix**
   (`crates/terrustia/src/update.rs:485-498`, the `#[cfg(windows)] install` arm, which copies to
   `terrustia.exe.new` and returns `DownloadedForManualApply`). The operator has to finish the
   update by hand on Windows. This entry used to cite `:58-60`, the enum variant's doc comment,
   which itself points readers at a `cfg(not(unix))` branch that does not exist anywhere in the
   file - the branch is `#[cfg(windows)]` on `install`.
   Separately, and now fixed: Windows ARM64 could not self-update *at all*, because `target_triple`
   had no arm for a target `release.yml` has been publishing.
5. ~~One bad Journey-power id in a `.wld` silently defaults every power after it.~~ **Addressed
   2026-09-05**, though not by changing what it does: stopping the read is correct, since an id whose
   payload width is unknown cannot be stepped over without misreading everything after it. It now
   names the id and says what that costs, so the loss is diagnosable rather than silent.

## Player-visible gaps, disclosed in code

Not defects; deliberate narrowings that a player would nonetheless notice.

**This census is point-in-time, and the point was 2026-09-05.** A projectile and boss-AI wave landed
across 2026-09-05 and 2026-09-06 and closed roughly fourteen items of exactly this class, none of
which ever had an entry here, which is the same failure the section is meant to prevent. Listed
once, worst first by what a player saw, rather than folded into the bullets below:

| Fixed | What a player saw before it |
|---|---|
| `62e8c91` | Deerclops' shadow hands: `AI_187_ShadowHand` is four behaviours under one style, and a hand launched with nothing runs the first, so **every hand of a six-hand wave drifted** |
| `ecfb3c0` | The Sand Elemental's tornado: `SANDNADO` was 658 (`SandnadoHostileMark`) where the tornado is 657, so **three markers spun in place for 900 ticks and no tornado ever formed** |
| `9f056a2` | The Key of Light and Key of Night were craft-and-discard: no `NPC.BigMimicSummonCheck`, so no Corrupt, Crimson or Hallowed Mimic could ever be met |
| `2468025` | The boulder never moved - a wired Boulder Statue put a stationary hostile box under itself for 3,600 ticks |
| `00ff471` | 16 of 43 arm-less projectiles flew perfectly flat: bone, knife, syringe, daggerfish, Santa's bombs, snowball, cannonball, Ball of Fire, three grenades, a present, the ale |
| `dfb2b2a` | The Empress's lance never fired and her rainbow never curved: every shot passed `time_left: 900` where the table says 200/660/240/180, and for three of five that number *is* the mechanic |
| `ce837dc` | The Empress's streaks never homed and her sun dances drifted away |
| `9f65f54` | The Moon Lord's deathray flew away from him instead of sweeping across you |
| `ce83de5` | The Dark Mage's heal healed nothing; the Saucer's missile never turned |
| `ea22e13` | A `Shot` could not carry `ai` values, so Duke Fishron's bubble never chased |
| `74104f0` | Queen Slime's ground smash never grew its hitbox (5 to 30 tiles); the Deerclops spikes stood still |
| `1d40bff` | Nebula and Stardust escorts left in a straight line instead of escorting |
| `f0c0f8a` | Betsy's flame breath was launched at her dash velocity and trailed out behind her instead of riding her jaw |
| `8eea7ec` | The golf ball had no arm at all; projectile coverage is 81 of 81 now, from 43 of 79 arm-less when the lane opened |
| `6f78e4d` | Clients were told a world was Remix when nothing here mirrors one |

- **Player luck is modeled now** (2026-09-05), and it was never client-side state: packet 134
  (`UpdatePlayerLuckFactors`) exists to tell the server, and `MessageBuffer.cs:4190-4220` stores all
  eight factors and calls `RecalculateLuck`. This server relayed the packet verbatim and threw the
  contents away, which is why ~15 comments across the workspace each said luck was unmodelled.
  `terrustia-proto::luck` is the transcription (`Player.RecalculateLuck`, `GetLadyBugLuck`,
  `CalculateCoinLuck`, and both `Luck.RollLuck` branches); `Player::luck` is the figure, refreshed on
  packet 134, on packet 50 (`stinky` is one of the terms) and on a Lantern Night starting or ending
  (+0.3, and the server's own state).
  **The drop tables read it too**, as of the same day. `CommonDrop.TryDroppingItem`
  (`CommonDrop.cs:36`) rolls `info.player.RollLuck(chanceDenominator) < chanceNumerator`, so every
  ordinary drop scales unless its rule is one of the `NotScalingWithLuck` variants;
  `npc_drops::LuckScaling` is that distinction, emitted per rule by the generator from the
  constructor source used, and hand-marked in `conditional_drops.rs` for the ~10 entries the
  generated table does not own. `drop_coins`' luck double-roll (`NPC.cs:80440-80459`) is modelled
  as well. The luck used is the *closest* player's, which is what
  `NPCLoot_DropItems(closestPlayer)` passes (`NPC.cs:79649`, `:79741-79752`).
  Two over-drops fell out of that work and are fixed: the Twins' trophies and the Groom's and
  Bride's Bloody Tear were each registered in *both* the generated and the hand-written table, so
  `drop_loot` rolled them twice - 19 and 36 per cent against the 10 and 20 the game intends.
  `no_item_is_registered_in_both_tables` now guards that seam.
  **Ambient spawn rates are wired too**, as of the same day. `SetSpawnFlags` copies the player's
  luck onto the spawner (`NPC.cs:370`) and `spawn::Conditions::luck` is that. Every roll was
  checked against the vanilla line its own comment cites rather than converted on the strength of
  the constant: eleven turned out to be real `RollLuck`/`RollBadLuckExtreme`/`RollOnlyBadLuck`
  calls (the gold critters, the Gnome's two, the Lacewing, the Rainbow Slime, the Groom and Bride,
  the Statue Mimic, the Owl-turned-Mimic, the dungeon Slime, the Fungi Bulb, the sky's Purple
  Slime, the underground fairy, the Gold Frog and the Palworld pair), and five routines that had
  been given the parameter turned out to roll nothing but `Main.rand.Next` in source and had it
  taken back out. `rates`' own `RollOnlyBadLuckExtreme(50)` arm (`NPC.cs:925-929`) is transcribed:
  a cursed player's world genuinely spawns faster and holds more, which is the one thing bad luck
  buys.
  Two of the ten terms are absent on a server in vanilla too: `usedGalaxyPearl` is a player-file
  field no packet carries, and `stinky` is read off the server's own buff state, so a server's
  figure differs from the client's tooltip by up to those. Disclosed at the module.
- ~~**Fallen Stars** and the surface fairy mechanic are unmodeled.~~ **Fallen Stars fixed
  2026-09-05**: `systems::spawn_falling_objects` is `WorldGen.cs:72398-72434`, and the two-projectile
  handover and the item drop are `Projectile.cs:54028-54061` and `:79348`. This one was worse than
  its "not a defect" heading admitted: the whole pre-hardmode mana ladder had no first rung, because
  a Mana Crystal is made from Fallen Stars and no Fallen Star existed anywhere in any world this
  server served.
  **The "surface fairy" half of this entry was wrong.** Vanilla has exactly one fairy spawn arm,
  `CheckToSpawnUndergroundFairy` (`NPC.cs:3616`, `:5820-5842`), and this server models it; there is
  no surface one to be missing. What *was* missing is that arm's `tenthAnniversaryWorld` half, which
  halves the base chance to 250 and biases three draws in four to the pink fairy - dropped with the
  note "no flag plumbed through `EventSpawns`", and wired 2026-09-05 along with `Star.NightSetup`'s
  own two anniversary constants.
- ~~**Money rain** is unmodeled.~~ **Fixed 2026-09-05**, and it was never on this list: one shower
  in twenty-five is a money rain in vanilla (`Main.cs:65638-65652`) and nothing here set
  `Main.coinRain`, so `SpawnFallingObjects`' coin arm had no input. 75 to 150 gold scaled by world
  width is not a cosmetic omission.
- ~~**Moon Lord's** hand brand-then-blob mechanic~~ **modelled 2026-09-05.** It was not a missing
  flourish: the leech step is a three-link chain (`NPC.cs:42723-42754`) and this server had it as a
  timer. The head brands every living player within 3,000 px with projectile 456, each brand flies
  to its own player and leaves `BuffID.MoonLeech` on them (`aiStyle 85`, `Projectile.cs:32327-32393`),
  and at three fixed marks every brand still up whose player still carries the debuff becomes a
  leech *on the target*. Shedding it, or being far enough away that your brand has not landed, is
  the counter-play, and none of it existed. The timer also fired one leech every sixty ticks over a
  435-tick step - eight a cycle against the game's three, made at the boss rather than at anybody.
  ~~**Moon Lord's true countdown timer** is still unmodelled
  (`crates/terrustia/src/game/ai/boss/moon_lord.rs:299`).~~ **This entry was wrong twice, and
  rewritten 2026-09-06.** The citation never resolved: `moon_lord.rs` contains no occurrence of the
  word "countdown" at HEAD *or* at the commit that wrote the line, so it was pointing at the wrong
  file the day it was written. And the countdown *is* modelled - `NPC.MoonLordCountdown` /
  `MaxMoonLordCountdown = 3600` (`NPC.cs:6038`), started by `WorldGen.StartImpendingDoom`, is
  `lunar.rs:29` here, armed at `:127`, ticked at `:129-132`, announced with vanilla's own
  `Lang.misc[52]`, and broadcast as packet 103 with max-then-current exactly as
  `NetMessage.cs:1391-1392` sends it.

  **The real gap is one clause of `AnyDanger`, and it is smaller and more specific than what this
  entry claimed.** Vanilla's `NPC.AnyDanger` opens with `if (!ignorePillarsAndMoonlordCountdown &&
  MoonLordCountdown > 0) flag = true;` (`NPC.cs:81066`). Ours
  (`crates/terrustia/src/game/server/systems.rs:9246-9252`) is `moon.running() || army.ongoing() ||
  any live boss`, with a comment saying the countdown "is not modelled as a countdown" - which was
  true when it was written and is not now, since `lunar.countdown` is right there. So for the 3,600
  ticks between the last pillar falling and the Moon Lord arriving, vanilla considers the world
  dangerous and this server does not, and ambient spawning carries on as if nothing were coming.
  One clause plus a test; tracked in `TODO.md`.
- **A hand holds its station** through its attacks rather than being pulled off it by each one
  (the sphere barrage's `SmoothStep` swing to `400 * side, -60`, for instance). Still true at
  `moon_lord.rs:301-311`, but the cross-references beside it are not: `moon_lord.rs:452` and `:585`
  say this is "the same narrowing the deathray already carries" and the deathray stopped carrying
  it in `9f65f54`, which moved the sweep into `systems::tick_phantasmal_deathrays`. What remains
  narrowed is the hand station and the hover-then-relaunch gather, not the head and not the ray.
- ~~Old One's Army has no client-visible progress bar.~~ **Fixed 2026-09-05**: it rides packet 78
  with its own icon 3 and its wave number, as `DD2Event.cs:185`/`:191` do.
- ~~**Frost and Pumpkin Moon wave-gated drops** are flattened to guaranteed picks rather than gated on
  the live wave number.~~ **Fixed 2026-09-05.** The Pumpkin Moon half was already gated; the Frost
  Moon half was flattened for want of a `frost_moon_wave` field on `Conditions`, which is now there.
  Six places read it: both minibosses' fallback chains (through a new outer gate on
  `ConditionalChain`, since `LeadingConditionRule(cond).OnSuccess(chain)` cannot be folded into the
  links' own integer denominators), the Ice Queen's pool, Santa-NK1's Reindeer Bells and their wave-15
  floor, and all three trophies. It was not a small divergence: at wave 1 in a classic world every one
  of those fired about ten times too often, which left the event's whole wave progression with nothing
  to offer.
- ~~**Town-NPC attack windups** are skipped and the Pirate's escalating burst is unmodeled.~~
  **Fixed 2026-09-05.** A shot now leaves on its own `localAI[3]` mark rather than on the tick the
  decision is made (`NPC.cs:55049`), which is the telegraph; and the four burst ladders are
  transcribed, longest first: the Pirate's six shots at frames 1/16/24/32/40/48, the **Painter's**
  three and the Steampunker's three unconditional, and the Arms Dealer's four, which is the **only**
  ladder in the game behind `if (Main.hardMode)` (`NPC.cs:55129-55147`).

  **Both halves of that sentence were wrong here until 2026-09-06.** This entry named the Cyborg,
  which has `num54 = 1` and no ladder at all, and put its non-existent three behind hardmode
  alongside the Arms Dealer's - 227 is the Painter and 209 is the Cyborg, and the type ids are the
  trap (`town_combat.rs:131-148` records the same confusion in the code it was fixed in). The
  Painter's ladder really was behind the hardmode gate, which cost a classic-mode Painter two
  thirds of its defence. And the claim that "each was read off the state block rather than inferred
  from the pattern the first one sets" is refuted by the commit that corrected it (`c65a924`):
  they were inferred, and that is precisely how the wrong NPC got a ladder.
  The flat `cooldown` also goes, replaced by vanilla's own per-tick gate
  `Main.rand.Next(AttackAverageChance[type]) == 0` (`NPC.cs:56012`) - the module doc claimed this
  project had "no equivalent scheduling primitive" for it, which was never true, and the flat
  number was wrong for the Dye Trader, whose gate is `1` and was modelled at a nine-tick gap.
  Still narrowed, and each still disclosed at its own entry: the hardmode *damage* upgrades, the
  Cyborg's three-way projectile roll, the Pirate's close-range special, and the vertical
  aim-tolerance check. (Those four were re-checked 2026-09-06 and are all still accurate.)

  **And "Fixed 2026-09-05" was three defects short.** `2eda3bd` found that twenty-two of the
  twenty-eight combat-capable townsfolk are ranged and **nothing in this server had ever damaged an
  NPC with a friendly projectile**: their shots were decided, aimed, launched, synced, flown and
  expired without once being tested against a hitbox, because `Damage_PVE` sits behind
  `owner == Main.myPlayer` and `Main.myPlayer` is 255 on a dedicated server. `143c6ef` found the
  Dryad's ward launch speed was invented, and that every town shot carried an invented flat
  300-tick lifetime; `12286c2` found the zero that replaced it was wrong for the only two
  `timeLeft` overrides in `AI_007_TownEntities`, the Golfer's ball and the Goblin Tinkerer's spiky
  ball (both 480).
- ~~**Slime Rain** collapses its per-type flags to one case~~ **and had no spawns at all.** Fixed
  2026-09-05: `NPC.SlimeRainSpawns` (`NPC.cs:5905-5967`) is transcribed, so the event is slimes
  falling rather than a world flag and an announcement. Three of its four picks are negative net
  ids - the Pinky at one in two hundred and the two coloured slimes - which no world this server
  served had ever seen, because `Npc` carried no net id and packet 23 sent the type. The second
  half of the old entry was simply wrong: vanilla does *not* announce a start or stop instantly
  either (`slimeWarningDelay` is 420 ticks), and `slime_rain.rs`'s own module doc says so.
- **All 65 negative net ids** were absent from every world this server served, and the Slime Rain
  variants are only the first of them. `net_variants.rs` is the generated table
  (`NPC.SetDefaultsFromNetId`) and `NpcStore::spawn_net_id` applies it; what remains is to route
  the *other* callers through it, **and the surface night's are all done**: the closing switch's
  size swap for the seven zombie styles (`NPC.cs:4811-4814`, fourteen ids), the five coloured eyes'
  twins and the small Demon Eye (`:4569`, `:4581-4610`, six more), and the rain zombies' two sizes
  (`:4675-4690`). `try_spawn` carries a net id rather than a type so an arm that picks one can say
  so, and `Drawn` carries the *companion* vanilla sometimes spawns beside a draw, which is what the
  coloured eyes need: two `SpawnNPC` calls with no `return` between them.
- ~~**Lantern Night's** manual-forcing toggle is unmodeled.~~ **This entry was wrong**, and the
  code it pointed at said so at the time. `LanternNight.ToggleManualLanterns` is defined in
  `Terraria.GameContent.Events/LanternNight.cs:107` and called from **nowhere in the entire
  decompiled tree** - re-checked 2026-09-05, one definition and no caller - so there is nothing a
  player can do to reach it in the real game either. `lantern_night.rs` carries the `manual` field
  and the method anyway, so `is_up()` matches `LanternsUp`'s own `genuine ? true : manual`, and
  nothing calls it here for the same reason nothing calls it there. Listing it as a gap made this
  file's own count of player-visible gaps one too high.

## Structural

- ~~**Three proto tables have no generator.**~~ **Closed 2026-09-05.** All three are now held to
  source: two by a checker, and the third by a real generator. `npc_data.rs` gained `just check-npc-data` on 2026-09-05: it re-reads `NPC.SetDefaults`' own
  691-arm chain and compares every entry on all 16 fields, from `just check-data` beside the drop
  and recipe checkers. It found the table **already correct**, with seven deliberate differences on
  its own record (the four Lunar Towers' `boss`, the Skeleton Merchant's `town_npc`, and the Torch
  God's size, which `SetDefaults` genuinely never sets). Rule 7's protection is that a table cannot
  drift from source unseen, and that is now true of `npc_data.rs` without a risky 13,000-line
  replacement.
  **`placed_items.rs` got the same treatment the same day**, and unlike the NPC table it was not
  already right: `just check-placed-items` re-does the inversion `Item.SetDefaults` supports and
  found **69 (tile, style) pairs missing outright and one wrong** - seventy framed objects that
  gave nothing back when mined, and a Boreal Wood sofa that gave the wrong bench.
  **That was the first of four passes, and each of the next three found more**, because the checker
  was only reading part of its source. Following the placement helpers found 145 more (889 items
  place through `DefaultToPlaceableTile` rather than by assigning the fields, and all 101 music
  boxes were absent); reading the game's own `GetItemDrop_*` methods found 256 more and one wrong
  (a bench giving another bench's item); reading the six drop arms written inline in the shape
  validators - banners and the five painting sizes - found 204 more, including a plain 3x2 painting
  that gave the style-21 item; and reading the ~120 assignments that *compute* the field from
  `type` (`placeStyle = 1 + type - 3046;` is the five campfires) found 55 more.
  **736 of the table's 3,129 entries were wrong or absent**, every one an object that gave nothing,
  or the wrong thing, when a player mined it. Every one of those gaps was found by mutation-testing
  the checker rather than by reading it: `check_placed_items.py` killed 43% of mutants after the
  first pass and **kills 100% now**, with no entry in the table left unchecked.
  **`tile_object.rs` got a generator instead**, because its source is not a table to check against
  but a program to run: `TileObjectData.Initialize` mutates one shared object 389 times over 2,900
  lines. `terrustia-codegen tile_object` interprets it, `just regen` rebuilds the file, and
  `just check-tile-object` is a second independent interpreter that agrees with it entry for entry.
  It was the worst of the three: eight of thirteen fields were right and the other five had been
  filled in with a uniform guess, so **every object's style layout past style 0 framed wrong** -
  a torch's styles stepped 40 down the sheet instead of 22, a table's second style landed at
  (0, 72) instead of (54, 0), and a door's at 27 styles away from where it belongs. 2,266 styles
  across 72 tile types were reachable through ordinary placement. The one site that had noticed
  (`dispatch.rs`'s container placement) had written its own arithmetic around it and said in a
  comment that the table was wrong and not its to fix.
- ~~**Sixteen global drop rules have no source in this server.**~~ **Closed 2026-09-05, the same
  day it was found.** `ItemDropDatabase.RegisterToGlobal` hangs a rule off *every* NPC rather than a
  type, so nothing keys it by npc and `check_drops.py`'s per-type comparison was structurally blind
  to it. Adding that check found all sixteen missing:
  - **The five biome keys and the Desert Key** (1533-1537, 4714) never dropped, so none of the
    Dungeon's six biome chests could ever be opened and their six weapons were unobtainable.
  - **The Pirate Map** (1315) never dropped, so a Pirate Invasion could not be summoned by
    ordinary play.
  - **The four hardmode yoyos** (3282, 3286, 3289, 3290) never dropped.
  - The two Halloween weapons, the Goodie Bag, the Present and the Living Fire Block never dropped.

  All sixteen are implemented, each behind its own condition class read from `Conditions.cs`. That
  needed `conditional_drops::Conditions` to learn the credited player's zone (seven flags), the two
  seasons, `downedBoss3`, the difficulty slider and four facts about where the NPC died. It also
  corrected a related wrongness that only became reachable when the souls were added an hour
  earlier: `in_hallow`/`in_corruption`/`in_crimson` were built from the tile under the corpse, and
  every rule that reads them means the *player's* zone.
  `GLOBAL_DEFERRED` is empty and the check gates, so nothing can join them silently.

- ~~**Lane B (error handling and data safety) is the only lane in `TODO.md` with no "(done)"
  marker.**~~ **Closed 2026-09-06**, along with Lane G, which had earned its marker earlier and had
  not been given one. Every lane A-H is now marked, and the striking thing is that nothing had to
  be written to close either: all of Lane B's bullets were already built and none had been checked
  off, so the lane sat open on a stale count while the code underneath it was finished.

  **The count this entry used to carry was wrong by about fifty times, and how it was wrong is the
  point.** It read: "485 `.unwrap()` calls remain in production files (`net/listener.rs`,
  `net/codec.rs`, `world/wld.rs`, `world/wld_save.rs`, `admin/audit.rs` and others)". **All five of
  those files have zero production unwraps**; every occurrence in them is inside `#[cfg(test)]`.
  485 was a naive whole-file grep, and it was quoted here as evidence of an open sweep.

  Three counting methods were tried on 2026-09-06 and gave three answers: a naive grep says 505, a
  "count occurrences before each file's first `#[cfg(test)]`" heuristic says 1, and brace-matched
  stripping of `#[cfg(test)]` bodies says 9 - of which at least three are a comment, a method that
  happens to be named `expect`, and test bodies sitting after a nested `cfg(test)`. **That spread
  is the finding.** The genuine production sites are single-digit and each is an internal invariant
  carrying its own written reason ("the loop only exits with a slot or a return", "checked just
  above", `terrustia-proto/src/reader.rs:19`'s infallible `try_into` after a checked `take`).

  **And the instrument already exists, which this entry did not know either.**
  `crates/terrustia/tests/panic_budget.rs` pins the count, lists every site when it fails, and has
  been brace-aware since 2026-08-31 - it was fixed that day precisely because it used to truncate
  each file at its first `#[cfg(test)]` and so never scanned ~20,000 production lines. Being a test
  rather than a recipe, it already runs in CI and in `just check`. Its number is **12**.

  That number was checked against an independent implementation on 2026-09-06 (a `syn` parse rather
  than a brace scan, written before anyone noticed `panic_budget.rs` was there) and the two agree
  exactly: the parser finds **11**, and the twelfth is `terrustia-proto/src/reader.rs`'s
  `bytes.try_into().unwrap()`, which sits inside a `macro_rules!` body where a syntax tree cannot
  reach it and a text scan can. Each tool sees what the other cannot, and they land on the same
  total.

  **All twelve were then triaged by hand**, which is tractable at twelve and was the thing actually
  missing. Every one is an invariant local to its own function, with the reason already written at
  the site: three `unreachable!` arms in `liquid.rs` each guarded by an explicit `this_kind != X`
  two lines above; `traps.rs`'s arm over a `kind_type` rolled from a fixed range; `ai/mod.rs`'s
  style arm, defended by `every_ported_style_actually_runs_its_routine`, which walks all 697 types
  and asserts the unwired set is empty; `buffs.rs`'s `expect` after a `while at.is_none()` loop
  that can only exit with a slot or a `return`; `moon.rs`'s and `layout.rs`'s after their own
  guards; `record.rs`'s two fixed-width slices inside a `while at + 10 <= bytes.len()` loop; and
  `tile_cleanup.rs`'s thread join, whose worker only reads `World::tile` and pushes to a `Vec`.
  None is reachable from untrusted input as a *failure*; each aborts only if this server's own
  code contradicts itself.

  So the honest state of this bullet is: **the sweep is done, and now it is demonstrated.** What
  was never built is automated reachability analysis - "is this site on a path a client can drive"
  - and at twelve sites that is a question a person can answer and did.
- ~~**The flaky-test root cause is still undiagnosed**~~ for `tests/shutdown_signal.rs`.
  **Found and fixed 2026-09-05**, and it was not a race in the server at all. Two defects in the
  test compounded: the kill sits after the assertions and `std::process::Child` does not kill on
  drop, so a failing run left a real server alive; the port was a constant, so that leftover held
  it and made every later run on the machine fail. Caught in the act in a 20-run loop: run 9 timed
  out genuinely and left its server on 17796, and runs 10 through 20 then failed in 0.38 seconds
  each against it. That is the "one run in five": a poisoned machine, not a racy test.
  The third piece is why nobody saw it. The server printed `127.0.0.1:17796 is already in use` and
  exited, and the test pipes stdout and stderr and read neither on failure, so the reported symptom
  was always "the server should have reached its main loop by now". A kill-on-drop guard and an
  OS-assigned port are now in `tests/support/mod.rs`, shared with the three other files that spawn
  a server and had the same shape, and every assertion here now carries what the server said.
  Verified both ways: forcing a mid-test failure leaves two servers running on the old code and
  none on the new, and the suite is 8 for 8 with nothing left in `$TMPDIR`.
  ~~**`new_world_cli` is a separate fault and stays open.** It kills its child before asserting, so
  it cannot leak; its lead is still the `Command::spawn` `ENOENT` against a concurrently relinked
  `target/debug/terrustia` written up in `TODO.md`.~~ **Closed 2026-09-05 (`f9f8b09`), fifteen hours
  after this paragraph was written, and the ENOENT lead it repeats was wrong.** That commit's own
  message opens by saying so. Five faults, none of them a relink: a 120-second filesystem poll on
  the success path, a log line filtered out at the default level, constant ports 17779-17784, a
  racy `free_addr` that bound `:0` and dropped the listener before returning the port, and a
  30-second `try_wait` poll. Measured 1-in-6 before and 0-in-12 after.

  **Both flakes of this shape in the suite are now closed**, the second being
  `every_newly_covered_town_npc_actually_fights` (`61407f2`), whose wall-clock deadline now counts
  server events instead of seconds. Worth recording that the wrong lead survived in two documents
  at once: it was well argued, it explained every observation, and it was still not what was
  happening.

## Documents that were wrong

Recorded because the pattern matters more than the individual fixes: on 2026-09-04, three backlog
entries and one planning document were found stating things the code contradicted.

- `TODO.md`'s DESERT and TAXCOLLECTOR entries both described gaps that were fully implemented, the
  latter predicting infrastructure ("a general item-vs-live-NPC interaction, which nothing in this
  server currently has") that already existed at `dispatch.rs:4017-4023` (this file cited
  `:3869-3889`, which is now the pylon-travel source check - the citation rotted the same way the
  entries it is describing did).
- A planning document claimed eighteen boss AIs were unbuilt. Every one of them exists, several at over
  a thousand lines.
- ~~`crates/terrustia/src/game/journey.rs:55-58` claims Stop Biome Spread "has nothing to gate yet"
  and that this project "does not model corruption/crimson/hallow tile spread at all".~~ **The
  comment was corrected; this entry then went stale in its place.** `journey.rs:60-66` now reads
  "Freezes the hardmode infections where they stand, and really does", and says outright that it
  used to say the opposite. Both of this entry's own citations had also rotted: the gate is
  `systems.rs:7780` (`let spreading = hard_mode && !self.journey.stop_biome_spread;`) and the test
  is `with_the_power_on_nothing_spreads` at `systems.rs:10465`. A file about documents that were
  wrong is not exempt from being one.

The checkers caught none of these, because none of them check prose. `docs/spawn-gaps.tsv` would have
caught the first two the day they went stale, had anyone diffed it.
