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
| **Peak RSS under 1 GiB at 255 players** | **BOUNDED 2026-09-05, NOT RE-MEASURED** | Run 2 reached 1536 MiB because nothing bounded the sum of the outbound queues. A 256 MiB server-wide budget now does. The soak has not been re-run since. See "The memory ceiling" below |
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
   (`crates/terrustia/src/update.rs:58-60`). The operator has to finish the update by hand on Windows.
   Separately, and now fixed: Windows ARM64 could not self-update *at all*, because `target_triple`
   had no arm for a target `release.yml` has been publishing.
5. ~~One bad Journey-power id in a `.wld` silently defaults every power after it.~~ **Addressed
   2026-09-05**, though not by changing what it does: stopping the read is correct, since an id whose
   payload width is unknown cannot be stepped over without misreading everything after it. It now
   names the id and says what that costs, so the loss is diagnosable rather than silent.

## Player-visible gaps, disclosed in code

Not defects; deliberate narrowings that a player would nonetheless notice.

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
  **Moon Lord's true countdown timer** is still unmodelled
  (`crates/terrustia/src/game/ai/boss/moon_lord.rs:299`).
- **A hand holds its station** through its attacks rather than being pulled off it by each one
  (the sphere barrage's `SmoothStep` swing to `400 * side, -60`, for instance).
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
  transcribed, longest first: the Pirate's six shots at frames 1/16/24/32/40/48, the Arms Dealer's
  four and the Cyborg's three behind `if (Main.hardMode)`, the Steampunker's three unconditional.
  Each was read off the state block rather than inferred from the pattern the first one sets.
  The flat `cooldown` also goes, replaced by vanilla's own per-tick gate
  `Main.rand.Next(AttackAverageChance[type]) == 0` (`NPC.cs:56012`) - the module doc claimed this
  project had "no equivalent scheduling primitive" for it, which was never true, and the flat
  number was wrong for the Dye Trader, whose gate is `1` and was modelled at a nine-tick gap.
  Still narrowed, and each still disclosed at its own entry: the hardmode *damage* upgrades, the
  Cyborg's three-way projectile roll, the Pirate's close-range special, and the vertical
  aim-tolerance check.
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

- **Three proto tables have no generator**, and the largest of them is now held to source another
  way. `npc_data.rs` gained `just check-npc-data` on 2026-09-05: it re-reads `NPC.SetDefaults`' own
  691-arm chain and compares every entry on all 16 fields, from `just check-data` beside the drop
  and recipe checkers. It found the table **already correct**, with seven deliberate differences on
  its own record (the four Lunar Towers' `boss`, the Skeleton Merchant's `town_npc`, and the Torch
  God's size, which `SetDefaults` genuinely never sets). Rule 7's protection is that a table cannot
  drift from source unseen, and that is now true of `npc_data.rs` without a risky 13,000-line
  replacement.
  **`placed_items.rs` got the same treatment the same day**, and unlike the NPC table it was not
  already right: `just check-placed-items` re-does the inversion `Item.SetDefaults` supports and
  found **69 (tile, style) pairs missing outright and one wrong** - seventy framed objects that
  gave nothing back when mined, and a Boreal Wood sofa that gave the wrong bench. All seventy are
  fixed and all 1,027 pairs the game defines now match. `tile_object.rs` is the one left with
  neither a generator nor a checker.
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
