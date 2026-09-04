# TODO: the v0.0.1 roadmap and the single backlog

This file IS the plan. Work that is known and deferred, not hidden, organised as the release
roadmap. The former `plan.md` (the pre-roadmap working ledger) and `GAPS.md` (the seven-pass audit
trail) are folded into this file and removed; their full text lives in git history, and everything
in them that is still live appears below. There is no separate gaps file.

## What v0.0.1 means

A fully working, stable, production-usable, vanilla-identical replacement for the Terraria 1.4.5.8
dedicated server. There are two deliberate, documented exceptions. The first is worldgen: the
remaining secret-seed generation-content differences and remaining micro-biomes are deferred to
v0.0.2. The second is one place where vanilla is provably wrong: vanilla's liquid levelling rounds
with `Math.Round` and so slowly creates water (a faithful port was built as a test probe and
measured doing exactly that, +2 units on a thrash-prone pool), which on a long-running server would
flood worlds; terrustia keeps a conserving model that levels and settles correctly but does not
reproduce the duplication. The divergence is locked by the `faithful_port_converges_but_is_not_
conservative` test that records both measurements. Neither exception excuses unrelated
inaccuracies. Versioning collapses to v0.0.x from here: the next release after v0.0.1 is v0.0.2 (the
worldgen release), and the old v0.1.0 label is retired.

**The v0.0.1 gates:** parity completion plus a from-scratch re-audit; the error-handling and
data-safety sweep; the `server.rs` architectural split; a zero-unknown-ID protocol classification;
the admin overhaul (namespaced permissions, audit log, moderation toolkit); Windows ARM64 in the
release matrix; the codegen port finished in Rust; town NPC happiness and shop pricing; and a
255-player qualification run per release candidate. A human fresh-character Moon Lord playthrough is
a strongly-expected but waivable qualification step.

**Town NPC happiness and shop pricing, added to the gates 2026-08-31.** The C3 audit found no
happiness, no price multiplier, no `ShopHelper` equivalent and no pylon happiness threshold
anywhere in the repo: an absent subsystem rather than a bug, which is why no earlier pass reported
it and why it had never been named here at all. Under a vanilla-identical bar an absent vanilla
system is a gap like any other, so it went into scope and was built from
`Terraria.GameContent.Personalities/` rather than deferred.

**It has since landed, and this entry described the world before that.**
`crates/terrustia-proto/src/happiness.rs` is 734 lines transcribing `ShopHelper.ProcessMood`
(`ShopHelper.cs:99-178`) with its own test module, wired through `server/mod.rs:1424-1490` and
reported by `/happy` (`console.rs:810-830`), with `examples/happiness_cost.rs` measuring what it
costs. The multiplier is taken once per chat, on `SetTalkNPC` (`dispatch.rs:1685-1695`), the same
moment vanilla takes it. Anything still outstanding under this gate needs naming from a fresh
reading of the code, not from this paragraph. Left here rather than deleted because the gate's history is the point: a system missing
from both the code and the plan is invisible twice over, and so is one this file still calls
missing after it exists.

## Phase 0: preconditions (complete)

Recorded for the trail: the Container CI musl-toolchain fix landed and the workflow is green; the
~20 stranded agent worktrees were surveyed (nothing unique) and removed; every MASTER-FIXPLAN P0
item was verified against `main` with the gaps ticketed into the lanes below; the audit-wave branch
is retired in favour of one topic branch per lane off `main`; and the fork-collaboration review
(spawn parity, VegaKernel/Xekep) was posted to PR #1 with the CLA-affirmation ask.

## Phase 1: the v0.0.1 core campaign (complete)

Integrated, parity-first, per-subsystem. The from-scratch audit produced a findings ledger per
subsystem, and fixes folded into that subsystem's single visit (split the file, clear its panics,
apply the audit fixes, tidy) so heavy files were churned once. Single-owner hot files
(`game/server.rs`, `world/worldgen/mod.rs`) took one change at a time.

All eight lanes (A-H) below landed. The from-scratch re-audit (Lane C's C2) then ran as its own
wave and is recorded under Lane C. The lane detail is kept below as the built record; the only
open Phase 1 item is C3 (adopting the fork's spawn module), which is blocked on the fork, not on
us. What remains before the tag is Phase 2 qualification.

### Lane A: split `game/server.rs` by responsibility (done)

The 16,058-line, 108-panic-site elephant becomes a `game/server/` directory: `dispatch` (the
`handle_packet` receive match), `tick` (the loop and phase orchestration), `panel` (panel-request
handlers), `console` (`run_command`/`run_admin_command`/`run_console`), `systems` (per-system
update calls), and a thin `mod.rs` keeping the `GameServer` state and actor entry. Zero behaviour
change; production panics on caller- or environment-triggerable paths cleared in the moved code;
the single-writer actor preserved; suite, clippy and fmt green per extraction.

### Lane B: error handling and data safety

- Clear every non-test `.unwrap()`/`.expect()`/panicking index/truncating cast from paths the
  outside world can trigger; replace each with propagation and an operator-facing message. The
  `net::listener::bind` mapping for `os error 28` is the pattern: keep the error kind, add advice.
- Capped backoff in the accept loop on persistent `accept()` failure, so descriptor exhaustion does
  not become a hot loop.
- ENOSPC, read-only filesystems and vanished directories handled on every write path: world save
  and autosave, rotating backups, the admin store, the setup-wizard config. Write to a temp path
  and rename into place everywhere; never lose the last good save to a half write. A failed
  autosave warns and retries: console and panel on the first failure, and after a few consecutive
  failures an in-game broadcast that saves are failing and progress is at risk.
- From the P0 verification: a game-side reaper for stale non-Playing slots older than
  `handshake_timeout` (the connection-level 64-frame deadline is escapable by sending frames), and
  the persistence refusal in Lane C1 below.

### Lane C: parity completion and the from-scratch re-audit

**C1, the known tail (done)**, each landed with a fail-then-pass test. Kept as the built record:
- HC8: nebula headcrab applies buff 163 (needs a player-buff channel on the AI `Effects`/`Outcome`).
- HC9/HC10: Solar Sroller multi-bounce and Sand Shark sand-swim collision physics in `npc.rs`.
- The four AI-state drop gaps: Skeletron's RedHatSkeletron set (5624/5625/5626/5628/5737 when
  `ai[3] == 1`), Pumpking's weapon pool, Mourning Wood 327, Mothron 477 item 1570; needs a
  conditions field threaded into drop resolution.
- L2: liquid destroys furniture (`tileLavaDeath`/`tileWaterDeath` table via codegen; a partial
  table would kill the wrong tiles, so it stays a no-op until the table exists).
- Trapdoor and tall-gate wiring: real `ShiftTrapdoor`/`ShiftTallGate` domain logic.
- B13: Empress of Light damage re-derived from vanilla's seven case blocks.
- BI8: slime re-targets only during an active (flag3) hop.
- Server MINORs: NPC-buff broadcast scope, summon combat books (-11/-17), the teleport guard on
  player controls, the chest-open (packet 80) rigged-input check.
- Persistence: `wld.rs` refuses out-of-order section pointers with an error instead of an empty
  blob, with a corrupt-`.wld` fixture.

**C2, the from-scratch audit (done)** ran in six consolidated read-only lanes against the
decompiled source, tracing behaviour to root cause on both sides, and produced the consolidated
ledger (about 12 blockers, 50 majors, 30 minors; the recurring shape was systems that ran and
produced output but the wrong amount, which is why earlier passes read past them). Four cross-cutting
root causes were named and fixed once each: the single-integer spawn identity (R1), difficulty
scaling applied in the wrong layer (R2), the server originating damage vanilla computes client-side
(R3), and Outcome flags produced but never consumed (R4). The fixes then landed as a wave, each
finding with a fail-then-pass test citing the vanilla line:

- **R1** multi-slot spawn identity (`Spawn.ai`), the shared prep both AI lanes built on.
- **World runtime (FIX-1a/1b/1c/1d)**: growth and hardmode spread cadence (L3-01, the corruption/
  hallow-never-creep blocker); liquid evaporation, merge origination, pacing and border margin;
  BFS wire flood with per-colour pumps and teleporters; wind and weather; crystal-shard and
  chlorophyte regrowth; the CheckMech split (per-colour skip, momentary detonator) and wired-light
  toggling.
- **Persistence (FIX-2)**: the Lunar Pillar save/load blocker (a free endgame skip) and the trailing
  round-trips (town rooms, pressure plates, bestiary, journey powers, travelling merchant).
- **Boss AI (FIX-3/3b/3c)**: the Moon Lord finale and True Eyes (dead code, boss unkillable-as-
  designed), the mech-boss dawn despawn, Wall-of-Flesh lasers, the Martian Saucer phases, the Moon
  Lord fixed per-part attack timeline, and the full boss minor tail.
- **Combat and damage (FIX-4)**: the extraUpdates N+1 slow-motion blocker, the knockback curve, and
  the R2/R3 difficulty-scaling and damage-origination corrections.
- **Spawning and town (FIX-5/5b/5c)**: bound-NPC progression gates, arrival item sets, the eight
  blocked townsfolk, the real biome-classification box, weighted spawn pools with per-type rates and
  caps, the pre-Skeletron Dungeon Guardian gate, housing-through-doors, town regen, and pylon travel
  validation against vanilla's five checks.
- **Protocol and worldgen (FIX-6)**: the AreaTileChange field-merge blocker (ordinary building was
  deleting a world's liquid and paint), the netmodule gaps, and the dungeon-loot and worldgen pass
  ordering.
- **Security and infra (FIX-7)**: the panel account-delete reach-check blocker, terminal-escape
  sanitisation, and the CI and config hardening.

**Phase 3 re-audit (done)**: a read-only pass over everything the wave changed, on the project's own
lesson that a fix is a change and deserves the same suspicion. It found one major (an Old One's Army
finishing-kill clamp that over-applied to every wave) and four minors, all fixed.

**Deliberate seams, measured not skipped**: the liquid-levelling conservation divergence (see "What
v0.0.1 means"); and C7-01, the Nebula Brain floater-hurry, which needs the NEBULA_FLOATER charge-up
projectile AI (ai_style 102) that is not built yet, documented at its drop site. A handful of small
narrowings are disclosed in-code where they were made (for example the liquid cycles round-robin,
the CheckMech cross-frame refusal modelled per-trip, and a couple of cosmetic gaps).

Known minor divergences that remain by design: `SendSection` does not sync the section's NPCs the
way vanilla does at `NetMessage.cs:2732`, there is no `Main.SyncAnInvasion` on packet 6 (cosmetic),
and section batching is stricter than `Tile.isTheSameAs` (correct output, more bytes).

**The item slot table**, four disclosed seams around `ItemStore::pick_slot`'s transcription of
`Item.PickAnItemSlotToSpawnItemOn` (`Item.cs:49779-49845`). None of them changes whether a drop
lands; each changes which slot it lands in, or how a removal is worded on the wire.

- `EmergencyStacking` (`Terraria.GameContent/EmergencyStacking.cs`, 450 lines, a pending-transfer
  queue of its own that `Item.NewItem` also has to clear) is not built. Vanilla tries it between
  the picker's second and third tiers; this behaves exactly as vanilla does when it returns false.
- `Main.timeItemSlotCannotBeReusedFor` is not modelled. Its one writer is `WorldItem.MakeInstanced`
  (`WorldItem.cs:326-341`), where vanilla gives each player their own treasure bag over packet `90`
  and then holds the slot empty for 54000 ticks; `drop_instanced_bag` keeps the bag as a real
  occupied slot instead, so no slot here is ever free-looking-but-not, and every branch reading the
  timer degenerates (including vanilla's fourth loop, which becomes its third exactly).
- `ItemID.Sets.OverflowProtectionTimeOffset` (`ItemID.cs:96`) is not applied. Vanilla seeds a new
  item's age with 50 to 200 ticks for two dozen common junk types, so those are evicted first. Our
  `WorldItem::age` doubles as the despawn clock, which vanilla has no equivalent of, so seeding it
  would silently shorten those items' lifetime; the offset only reorders eviction among junk in the
  deepest fallback.
- Vanilla words every server-side item destruction as a `21` carrying a zero stack
  (`WorldItem.TurnToAirAndSync`, `WorldItem.cs:623`) rather than a `151`; the shimmer, decraft and
  voodoo-doll sites here use a `151`, which clears the item on a client the same way but is not the
  packet vanilla would send.

`DESPAWN_TICKS` was listed here too and has since been **deleted rather than disclosed**. A Terraria
world item never expires with age (`WorldItem.cs:646-714`'s self-destructs are all per-type, plus
lava and falling out of the world), and this server's own ten-minute cleanup was standing in for a
mechanism it did not have yet. It has it now: what bounds vanilla's 400-slot table is
`pick_slot`'s recycling, transcribed in the same lane that found the divergence. Keeping the timer
would have meant a player dropping something valuable, returning a quarter of an hour later, and
finding the real game still had it while this server had thrown it away. The worry it existed for,
an idle server hoarding dropped dirt, is bounded exactly as vanilla bounds it: the 401st item
recycles the oldest.

Found while transcribing the picker and left open, because it cannot be transcribed faithfully yet:
`WorldItem.TryCombiningIntoNearbyItems` (`WorldItem.cs:157-186`) merges two stacks of the same item
lying within 30 pixels of each other and syncs both, which is the ordinary relief valve the picker's
recycling is the emergency version of. It needs a per-type `maxStack`, and no table in
`terrustia-proto` carries one.

**Area-of-interest culling on player movement and projectile syncs**, a deliberate and measured
divergence. Vanilla relays a player's movement to every other player (`NetMessage.SendData(13)`
excludes only the sender) and a client's projectile syncs the same way. terrustia routes both through
`broadcast_near`, the loaded-section cull that NPC sync already used, so an update goes only to
players whose sections could contain it. What a distant client loses is the fullscreen map marker
moving smoothly rather than in steps; it cannot draw the player or the projectile at that range, and
a skip budget (four for projectiles, matching the game's own rule, thirty for movement because it
arrives every tick rather than every sixth) is what stops anything distant freezing outright.

Kept because it was measured, not because it sounded right. Two 255-player runs at the pre-mitigation
queue depth, differing only in whether the cull was wired up, at matched NPC load: without it, 14
`outbound queue full` drops, 245 of 255 clients held, and the outbound queue running literally full
at 73,465 of 73,472; with it, zero drops, 255 of 255 held, and a peak of 38,713. `connection.rs`'s
own comment had predicted exactly this fix and left it for whoever owned the broadcast next.

**C3, the spawn lane**: adopt the fork's spawn-parity module structure once Xekep affirms the CLA
and the posted punch-list is fixed (or take the punch-list over if the fork goes quiet). This is a
restructure of code we already own and have now audited and fixed, not a gap: `game/spawn.rs` is
2,340 lines transcribed from `Spawner.SpawnAnNPC` with 34 tests. Nothing about the release waits on
it.

**C5, the third from-scratch audit (2026-08-30) and its fix wave.** Eleven read-only lanes over the
whole tree, ten parity lanes against the decompiled source plus one over-engineering lane kept
separate so simplification proposals could not contaminate parity work. Generated tables were in
scope and re-derived with independent parsers rather than sampled. It found **9 blockers, 45 majors
and 44 minors**, recorded in `.scratch/audit-2026-08-30/LEDGER.md` with the fix assignments in
`FIXPLAN.md` beside it.

The recurring shape was the same one C2 named and is worth restating, because six passes had read
past it: **nothing crashed**. Every blocker was a system that ran, produced output, and produced the
wrong amount. Gel's registrations matched a generator regex that silently returned nothing. The
liquid wake queue terminated only because its cap discarded roughly 97% of the work it was given.
`damage_bonus` was written in 13 production sites and read in none outside `#[cfg(test)]`, so every
boss enrage multiplier was inert while the tests that read the field passed.

Three of the four data-table blockers and both liquid causes were generator or design faults that
`just regen` and the test suite faithfully reproduced. Two further defects were found during the
fixing rather than the audit, both by building a better instrument rather than reading harder:

- **Bone (item 154) was unobtainable**, so every bone recipe was uncraftable. Its only two
  registrations are `ByCondition` lines (`ItemDropDatabase.cs:1162-1164`), and `tools/check_drops.py`
  could not see them: its argument slices made every `ByCondition` rule in the game source invisible,
  and its epilogue explicitly excused treasure bags and master-mode drops, which is where a fourth
  blocker was hiding. Fixing the checker surfaced Bone within the hour.
- **Nothing here ever woke the tile above.** Vanilla does it twice (`Liquid.cs:947-966` and
  `:1518-1521`); without it a column draining from the bottom never learns its floor has gone. This
  was the larger of the two causes of water hanging in mid-air, and the audit did not have it.

Both are the argument for the instrument campaign below.

**The secret seeds, quantified for the first time, and it is a disclosure problem as much as a
parity one.** An audit lane counted the sites rather than estimating them:

- **`Main.getGoodWorld` (For the Worthy): 101 sites in `NPC.cs`, 79 of them in NPC AI.** Three are
  implemented (the Wall of Flesh pace, the lunar pillar surface clamp, `DESTROYER_SEGMENTS_GOOD`),
  five more are explicitly disclosed as absent at their sites, and **roughly 71 are silently
  absent**, including all eleven of the Eye of Cthulhu's and all nine of the Twins'.
- **`Main.remixWorld`: 85 sites, zero consumed by AI.** The seed is detected, persisted, and
  **advertised to clients** (`world.rs:788`, `F::RemixWorld`), and worldgen consumes it in at least
  one place (`hardmode.rs::can_chlorophyte_grow` switches town-NPC happiness off for a remix world),
  but no AI reads it. `world/hardmode.rs:602-608`'s own comment was fixed to say this correctly; the
  radii/caps `can_chlorophyte_grow` itself computes still do not adjust for a remix seed, which is
  the real gap left at that site.
- **`WorldGen.Skyblock.lowTiles`: about 20 sites, zero consumed.** Detected and persisted, unread.

The parity gap is ordinary deferred work. The **disclosure** gap is not: the server currently tells a
client it is running a remix world and then does not behave like one, and a stale comment tells a
reader the seed does not exist when it does. Under this project's own rules a narrowing is disclosed
at its site, so either these seeds are wired up or their absence is stated where a reader will meet
it, and the advertisement to clients is reconsidered. That is a v0.0.1 decision, not a v0.0.2 one,
because it concerns what the server claims about itself.

**C4 (done)**: expanded the golden/deterministic vanilla-derived tests that CAN run per-commit in
CI; the live differential against a real `TerrariaServer` remains a Phase 2 qualification step, since
decompiled or installed game material can never ship to hosted CI.

### Lane D: protocol classification, zero unknown IDs (done)

One authoritative, machine-readable per-ID table for the full 0..=162 surface (direction,
client/server send, live/dead/legacy, dedicated-server applicability, Steam/social/host-only,
terrustia recv and send implementation, source evidence, tests), validated against the actual code
by the evolved `tools/packet_audit.py` so drift is a red check, with `docs/packet-coverage.md`
generated from it. Classifications carried from the audit trail: `DevCommands` (94) deliberately
unhandled (a public server that honours it can be rewritten by anyone); host migration
(`SpectatePlayer` 150, `HostToken` 161) not applicable to a dedicated server; `ShopOverride` (104)
unimplemented and classified rather than faked.

### Lane E: the admin overhaul (done)

- **E1, namespaced permissions**: per-command leaves with dotted families and wildcards
  (`server.kick`, `server.ban`, `server.mute`, `world.time`, `panel.console`; `server.*`, `*`),
  extending the existing string-set store. Ships a four-tier ladder: `default` (self-service
  only), `moderator` (kick/mute/look, panel view), `admin` (bans, world and panel management, no
  group-editing and no raw console, so it cannot self-escalate), `owner` (`*`). Includes the
  registration path a future plugin uses to declare its own permissions; the plugin API itself is
  post-v0.0.1.
- **E2**: the coarse match table in `run_command` becomes specific namespaced checks.
- **E3, panel roles and management**: a moderator logs in and does only what its permissions allow
  (today it cannot log in at all); `/api/console` behind its own high permission; a
  permissions-management view extending the existing groups/accounts views; the raw stdin console
  stays fully privileged, unchanged.
- **E4, the audit log**: a dedicated append-only file beside the world (issuer, timestamp, target,
  reason for ban/unban/kick/mute/register/group-change/claim), independent of the admin TOML store,
  with issuer and timestamp added to `Ban` as current-state, a read surface (console command and
  panel view), and size-based rotation with generous configurable caps.
- **E5, moderation**: real `mute` (chat suppression, duration, persistence, permission-gated,
  audited) plus temp-mute escalation, shadow-mute (the muted player sees their own messages echoed,
  staff see them flagged) and per-connection chat cooldowns (`Player::last_chat`, deliberately reset
  on reconnect and independent of sign-in: this session's own pace, not a persistent account
  record). All off by default: a fresh server feels exactly like vanilla until the operator opts in.

Out of scope for v0.0.1, planned after: regions and spawn protection, warps, item/tile/projectile
restrictions, general policy machinery, server-side characters, stronger anti-cheat (today the
server trusts client health/mana and has no stack validation or ban lists; the audit trail's
detail is preserved under Phase 3).

### Lane F: plaintext-transport hardening (done)

Document the plaintext transport plainly; keep Argon2; guarantee passwords are never logged;
login-attempt throttling (per-IP and per-account exponential backoff with jitter, in-memory, reset
on success, no lockout, so brute force is impractical and account-name lockout-griefing is
impossible, `admin::Throttle`); never treat the Terraria UUID as proof of identity. From the P0
verification: constant-time comparison for the claim token (`admin::constant_time_eq`, used at both
the console and panel paths) and an fsync in the admin store's save.

### Lane G: platforms

Add `aarch64-pc-windows-msvc` to the CI and release matrices (six official targets), built and
smoke-tested on GitHub's native `windows-11-arm` runner (falling back to cross-compilation if
runner availability disappoints); keep `riscv64gc` compiling as a compile-only target; keep the
matrix affordable.

### Lane H: finish the codegen port (done)

The eight remaining Python generators (`gen_drops`, `gen_projectiles`, `gen_banners`, `gen_buffs`,
`gen_angler`, `gen_shimmer`, `gen_town_names`, `gen_travel_shop`) become `terrustia-codegen`
modules, each verified byte-identical against its committed table; `just regen` points at the
codegen crate and the last `tools/gen_*.py` are deleted. The three checker scripts stay in Python
by decision (`check_drops.py`, `check_recipes.py`, `packet_audit.py`); note they need the
decompiled tree, so they run locally at qualification time, never in hosted CI (`just check-data`).

Two of the eight, `gen_shimmer.py` and `gen_travel_shop.py`, initially failed byte-identical for a
reason unrelated to the port: past hand-edits (`78d07de`, `65f4be3`) had updated `shimmer.rs`'s
decraft doc paragraph and added `travel_shop.rs`'s BlackCounterweight/YellowCounterweight source
comment and regression test straight to the committed tables, without updating either generator's
`emit()` to match, in violation of "generated tables are never hand-edited". Reconciled by teaching
both generators to emit exactly what is committed rather than touching either table.

### Cross-cutting through Phase 1

- **Dense-file splits**, paired with panic-clearing and idiomatic cleanup in the same visit.
  Measured in **production lines**, with `#[cfg(test)]` bodies excluded (re-measured 2026-08-31,
  refreshed 2026-09-01; the earlier list counted total lines and so listed ten files that were
  never dense): `game/server/systems.rs` (6,573 -> **6,976**), `game/server/dispatch.rs` (4,943 ->
  **5,489**, now growing faster than systems.rs, +11% since the last measurement, and not on this
  watch list until now), `game/server/mod.rs` (2,788 -> **3,042**), `world/wiring.rs` (1,690 production against 1,627 test, not the 2,575 total
  this list used to quote), `game/spawn.rs` (1,644), `world/wld.rs` (1,364), `game/ai/mod.rs`
  (1,237), `game/npc.rs` (1,186), `world/wld_save.rs` (1,053). The generated proto tables are
  excluded: codegen output, never hand-edited, size is
  fine. So are `crates/terrustia/tests/*.rs`, which carry no `#[cfg(test)]` and are test files
  entire (`gameplay.rs` would otherwise rank first at 6,907 lines).

  `panel/mod.rs` (2,139 -> 2,250, off this list as of 2026-09-03) is split: the proposal three
  entries below sat unactioned past its own stated trigger for two days, so it is done now rather
  than staying an open item. `mod.rs` (481 lines) keeps `PanelState`, `run`/`supervise`, static
  asset serving and the shared `ask`/`err`/`send_ws`/`auth_lookup` plumbing every sibling reuses;
  `auth.rs` (377), `status.rs` (181), `players.rs` (369), `worlds.rs` (430), `settings.rs` (374)
  and `accounts.rs` (320) hold one resource apiece, each exposing a `router()` the coordinator
  merges — the same shape `game/server/`'s own split settled into.

  Off the list, all under 1,000 production lines: `world/worldgen/traps.rs` (989),
  `world/worldgen/structures.rs` (950), `term.rs` (929), `world/world.rs` (916), `game/army.rs`
  (847), `game/buffs.rs` (823), `world/worldgen/mod.rs` (670), `game/ai/town.rs` (654),
  `game/ai/critter.rs` (654), `game/npc_ai.rs` (555). Note the three at the top of the new list
  are the hot files the guardrail below sequences last, so the list is now ordered by size and
  worked in roughly the opposite order.
- **Feature-cohesive layout and a periodic hygiene scan** (requested 2026-08-31, explicitly lower
  priority than parity work and never allowed to derail it). The dense-file list above is organised
  by size; this is the layer above it, organised by subject. A reader who wants to know how Martian
  Madness works should find it in one place rather than tracing it through `event.rs`,
  `invasion.rs`, `moons.rs`, `spawn.rs`, `systems.rs`, `ai/hardmode/saucer.rs` and `npc_params.rs`.
  The `game/server/` split by responsibility is the precedent that worked.

  Two guardrails, because a reorganisation that churns a file without making anything clearer is
  pure cost. First, size alone is not a reason: a long file implementing one algorithm transcribed
  faithfully from vanilla is not a problem, and moving transcribed code away from the shape of its
  source makes every future parity check harder. Second, the hot files (`game/server/systems.rs`,
  `dispatch.rs`, `game/ai/`) are under near-constant edit, so any move touching them is sequenced
  after the wave that owns them, never during.

  The periodic scan is the durable half: files past a line threshold, modules with too many inbound
  dependencies, exact-body-hash duplicate helpers (name matching is not enough, an earlier pass
  found eleven copies of one helper that turned out to be **two** subtly different helpers whose
  merge would have been a behaviour change), doc comments naming a file or function that no longer
  exists, and `pub` items with no reader outside their own module. Most of that is a script; the
  last one is better served by `unreachable_pub` on `crates/terrustia` alone, never on
  `terrustia-proto`, whose public surface is the whole point of the crate.

  **The first scan already found four things worth acting on** (2026-08-31, full detail in
  `.scratch/audit-2026-08-30/HYGIENE.md`):

  1. **The dense-file list above was measured on the wrong number. Fixed 2026-08-31.** It counted
     total lines including `#[cfg(test)]` bodies, so ten of its seventeen entries were never dense
     (`world/wiring.rs` is 1,690 production against 1,627 test, not 2,575) while the three largest
     production files in the tree were absent from it entirely, `game/server/systems.rs`,
     `dispatch.rs` and `game/server/mod.rs`, the last being the file Lane A meant to leave thin.
     The list above now counts production lines only.
  2. **Stale prose.** About 190 references to `game/server.rs` survive the Lane A split, across code
     comments, `docs/*.md` (one of them a link that 404s) and AGENTS.md. Many sit in files under
     active edit, so this is a single sweep to run once the parity lanes land, not piecemeal.
  3. **`docs/generated-tables.md` documents a workflow that no longer exists**: a runnable command
     block invoking ten `tools/gen_*.py` scripts that Lane H deleted.
  4. **Table provenance, fixed in part.** `npc_data.rs`, `tile_object.rs` and `placed_items.rs`
     (21,768 lines together) described themselves as generated while having **no generator**, and
     none is on rule 7's list. So `just regen` never touched them, yet a reader seeing "GENERATED"
     would either refuse to correct a wrong number or expect a regeneration to preserve their fix.
     `npc_data.rs` was in fact hand-edited on 2026-08-31, correctly, in a file whose header forbade
     it. That is the same trap Lane H hit with `shimmer.rs` and `travel_shop.rs`. The three headers
     now say plainly that no generator exists and corrections are made in place with a citation.
     **Writing real generators for them remains open** and is the proper fix.

  **The owner's own example, Martian Madness in its own folder, is declined with reasons.**
  `game/ai/mod.rs::run` is a `match npc.stats.ai_style` mirroring vanilla's `NPC.AI()` switch arm for
  arm, and every module under `game/ai/` is named for the style it implements. A `martian/` folder
  would cut `invasion.rs` in half (the probe is Martian, the Flying Dutchman is Pirate) and
  `charger.rs` too (the drone is Martian, the Solar corite is the Lunar event), leaving the tree half
  indexed by style and half by event, and moving transcribed code away from the shape of its source.
  `game/ai/army/` exists only because the Old One's Army roster happens to share a style band, which
  does not generalise. What Martian Madness actually needs is the subsystem map (now written down)
  and the roughly 140 lines of orchestration currently scattered across five ranges of `systems.rs`,
  which the `systems.rs` split below delivers properly.

  Two splits remain proposed, in order (`panel/mod.rs` was the third; it split by resource on
  2026-09-03 during the panel consistency pass, see the dense-file entry above for the resulting
  layout): `game/{housing,arrivals,rescues}.rs` into `game/town/`, **before** the newly-gated happiness
  and pricing work needs a home; and `systems.rs` into `game/server/systems/` along its
  already-contiguous feature bands, **after** the parity lanes let go of it — checked 2026-09-01
  and **not yet met**: `systems.rs` grew 6,573 -> 6,976 production lines and took 18 commits since
  the last measurement, mostly the spawn-roster and boss-parity work still actively landing there,
  which is the opposite of the lanes having let go of it. Explicitly do not touch: `wiring.rs` (one
  algorithm from `Wiring.cs`), `wld.rs` and `wld_save.rs` (sequential readers in the file format's
  own order), `npc_params.rs` (banded by AI style on purpose, which is exactly why its Martian
  constants sit 2,300 lines apart), `game/spawn.rs`, and all of `game/ai/`.
  Scheduled in-campaign but not a release gate; its worst failure is a boot convenience that
  already falls back to a logged manual-port-forward message.
- **The autosave snapshot is the most expensive thing an idle server does, and it is on the tick
  thread** (found by the owner running a real server, 2026-08-31). A `phase=snapshot phase_us=12833`
  warning on a world with **two NPCs and no players**: 77% of a tick's budget spent copying the
  world for a save.

  Measured on a fresh 4200x1200 world before writing anything down. It scales with how much the
  world has changed since the last save, not with the tick: 30 to 36 sections and 2.0 to 3.1 ms at a
  15-second autosave, 68 sections and 6.3 ms at the default 300, and 12.8 ms on a loaded world with
  a town. **Not a regression**, which was the first hypothesis and the wrong one: the same probe on
  the pre-wave build copies 36 sections in 3,059 us, slightly worse than now. It has always cost
  this. It was invisible because the phase timer used to bill the work to the wrong bucket, and
  because a comment in `save_world_in_background` claimed every save after the first was "already
  150-200 us" and was never re-measured. That comment now carries the real table.

  **Half done.** The spike is off the tick: a save is now armed rather than taken, the tile copying
  is drained three sections a tick by `tick_snapshot_drain`, and the save fires from
  `try_fire_pending_save` only on a tick where `World::snapshot_pending()` is zero. The
  point-in-time question answered itself: `set_tile` re-marks every section it touches, so a
  section copied early and then edited goes back on the list and is copied again, and a buffer whose
  pending count reaches zero is bit-identical to the live world at that instant. Assembled across
  ticks, delivered at one. `record_town_npcs`/`record_lunar_pillars`/`record_journey_powers` run on
  the firing tick and never the arming one, or the object tables would be newer than the tiles,
  which is the tear this exists to avoid. A 600-tick deadline bounds the wait, `drain_ticks` on the
  `world snapshot taken` line makes a deferral visible, and the escape logs a `warn!`.

  Measured by running both builds against the owner's own 4200x1200 world for 185 s each, autosaving
  every 20 s with nobody connected, nine saves apiece:

  ```text
                        before                          after
    snapshot_us   454 2116 3548 2632 213 750 413    295 308 757 124 194 121 335 224 36
                  871 378                           (max 757, was 3,548)
    sections      6 6 11 9 5 10 9 5 7               0 0 0 0 0 0 0 0 0
    drain_ticks   -                                 1 2 1 2 2 2 2 3 2
    worst tick    3,826 us                          2,218 us
  ```

  `sections_copied` is zero on every save: the firing tick copies no tiles at all, only the 20 us of
  side and object tables. What is left on the worst tick is a *drain* tick, and the per-section cost
  it pays turns out to vary 17x with page residency (213 us for 5 sections, 3,548 for 11), which is
  why the cap is three and not the eight a warm benchmark suggested.

  **What that leaves on the table, and it is the bigger half.** The drain spreads the work; it does
  not remove it. The unit of "changed" is 30,000 times bigger than the change: instrumenting
  `set_tile` on an idle fresh 4200x1200 world over six consecutive 20-second autosaves found 150 to
  260 tiles actually changing per window, and that marked 24 to 37 sections, an amplification of
  about 5,000x. Roughly 200 real edits drag about 990,000 tiles into the copy.

  So the follow-up is to track changed **tiles** rather than the sections they sit in, with a cap
  and a fall back to the section bitset (still maintained in parallel) once it overflows, for the
  bulk cases: an explosion, a Clentaminator sweep, hardmode generation, a mass-wire operation.
  Worldgen is already exempt, because `track_dirty` is false during it. Measured on the owner's real
  4200x1200 world with `examples/snapcost`, scattered so no two picks share a cache line and through
  the dearer public `tile`/`set_tile` pair, so an upper bound:

  ```text
      200 loose tiles      1 us     what an idle window actually changes
      4,000 loose tiles   30 us
      one section         16 us     warm; about 90 to 100 us cold on a live server
      all 168 sections  2,710 us
      a refresh with nothing dirty  21 us  (side tables + object tables, the floor)
  ```

  About 7.5 ns a loose tile against 16 us a section, so one section is worth about 2,100 loose
  tiles and an idle window changes about 7 tiles per marked section: three to four orders of
  magnitude of headroom. An idle save's tile copying would fall from 480 us (30 warm sections) to
  1 us, and the floor would become the 21 us fixed cost.

  A `Vec<u32>` of tile indices beats the per-tile bitset that was the alternative. Capped at 65,536
  entries it is 256 KB of reserve and about 800 bytes in use, against 630 KB of bitset that is
  always resident and has to be scanned end to end on every save: at the 28 GB/s this machine
  copies tiles at, that scan alone is about 22 us, more than copying the 200 tiles it would find.
  Duplicates in the list (one tile written repeatedly) cost 7.5 ns each and are bounded by the cap.

  The two compose rather than compete: with a tile list the pending count reaches zero on the
  arming tick in the idle case, so the save fires at once and the drain costs nothing, and the drain
  stays as the safety net for exactly the bulk case that overflows the list back to sections.

  **Ruled out, so nobody re-derives it:** an equality check in `set_tile` to skip rewrites of a
  tile's existing value. The same instrumentation counted `noop_writes=0` on every one of the six
  windows. Every gameplay write is a real change, so the check would cost a `TileStore::get` per
  write and save nothing.

- **Performance discipline**: maintain the benchmarks, measure meaningful changes, reject confirmed
  regressions on CPU, memory, latency, startup, saves and joins, and keep the instrumentation. The
  deep optimisation campaign comes after the feature waves, not now.

  **Ruled 2026-08-31: no merge may be measurably slower than `main` on a hot path.** Parity work
  does add real work, because vanilla does things this server was skipping, so the rule is not "never
  cost anything": it is that a lane measures the cost, optimises until it is negligible against the
  16.67 ms per-tick budget at 255 players, and reports the number. Correctness is never traded away
  to hit it; the implementation is what gets optimised, not the behaviour.

  The standard was set by the case that forced the rule. Moving `biome_at` ahead of the spawn rate
  roll, which parity requires, cost 82 us per scan, or **20.8 ms per tick at 255 players, over the
  entire frame budget on its own**. Vanilla pays nothing for this because the client runs
  `SceneMetrics`. A `BiomeCache` brought it to 345 us, about 2% of budget, and
  `crates/terrustia/examples/biome_scan_cost.rs` reproduces both numbers so the claim stays checkable.
  Where a fix is locally slower and correct, as the walled liquid cases are now that water completes
  its fall instead of stranding partway, that is documented with its measurement rather than hidden.

  Shorter 255-player soaks run as frequent regression checks and are never treated as definitive
  while the machine is contended; the full quiet-machine 30-minute run is reserved for milestones and
  release candidates. The README's comparison table against the official server is refreshed on the
  same cadence, so its numbers never drift from the build they describe.
- **Docs**: de-slop `AUDIT.md` and `docs/*.md` (em dashes and the usual tells); this file replaces
  `plan.md` and `GAPS.md`.

## Instrumenting for the next audit

Runs after the C3 fix wave lands and **before** the coverage-gap pass, because most of what it
builds does that pass's job mechanically and deterministically, and because a checker that cannot
see a class of defect makes a clean run indistinguishable from a real one.

The C3 audit found nine blockers by hand. Not one of them crashed. Gel's registrations matched a
generator regex that silently returned nothing; Bone was registered only through two `ByCondition`
lines and dropped from no NPC at all; the liquid wake queue terminated only because its cap was
discarding roughly 97% of the work it was given. Every one was a system that ran, produced output,
and produced the wrong amount, which is precisely what a reader looking for something broken reads
past, six passes running.

Worse, **the tools built to catch this were part of why it was missed**. `check_drops.py`'s epilogue
excused treasure bags and master-mode drops, the exact two categories a blocker was hiding in, and
its `[^)]*` argument slices made every `ByCondition` rule in the game source invisible, which is
where Bone lived. Fixing that checker surfaced Bone within the hour. The same shape had already
appeared twice: three release bars that nothing evaluated, and a `cpu_us` double-count in the
instrument built to measure the fourth.

So the lesson is not "audit harder". It is that these defects have mechanical signatures, and the
leverage is in tools that find a *class* rather than a person finding an instance. Ranked by value
over effort; the first three are roughly a day each.

1. **Mutation-test the verifiers.** The highest-leverage item here and the only one that checks the
   checkers. If `check_drops.py` cannot see a `ByCondition` rule, then deleting a `ByCondition`
   drop from the committed table does not make it fail, and that is directly testable: corrupt or
   remove entries programmatically, run the checker and the suite, and assert every mutation is
   caught. A surviving mutant is a blind spot by definition. This would have found the Bone gap
   without anyone knowing Bone existed. `cargo-mutants` covers the Rust side; the tables need a
   small script. Run at qualification the way `just fuzz` is, not per commit.

2. **Reachability as a gate, from an independent implementation.** "Every item vanilla can produce,
   can we produce, and vice versa" is a set difference, and it collapses Gel, Bone, the lunar
   fragments, the four missing treasure bags, the 102 unreachable items, the 57 master-mode items
   and the 80 missing projectiles into one query. Most of it exists: the audit lane wrote parsers
   over `ItemDropDatabase.cs` and a full interpreter of `SetupRecipes`, and the C3 wave repaired
   the checker. What is missing is that it must exit non-zero and be wired into `just check-data`.
   **It must stay a second, independent implementation.** Re-running the generator and diffing
   against its own output proves nothing; that is the same tautology as asserting `BUFF_COUNT`
   against the array the same generator sized.

3. **A dead-write lint. Built; the open work is triage, not construction.** `damage_bonus` was
   assigned in 13 production sites and read in none outside `#[cfg(test)]`. So were `wet`, slime's
   `ai[3]`, `TreeOutcome::fleeing` and `FairyOutcome::wants_treasure`: one class, five findings,
   two of them blockers, and the same root cause (R4) as four blockers in the C2 wave. Rust's own
   `dead_code` misses it because a test read counts as a read.

   This entry used to estimate "about a hundred lines over `syn`". It exists:
   `crates/terrustia-codegen/src/bin/deadwrite.rs`, ~700 lines with its own tests, an `ALLOWED`
   list carrying a written reason per excused field, and a `just check-dead-writes` recipe wired
   into `just check-data`. It reached zero findings on 2026-08-31: of the 18 it was reporting,
   `confused`, `dryad_ward` and `tipsy` were wired to real consumers, `ArmyState::stand`,
   `ArmyState::champion_down` and `dungeon_side` were deleted as redundant state, `angler_quests`
   and `golf_score` now feed the rebroadcast the way `NetMessage.cs:1156-1160` does, and the rest
   went onto `ALLOWED` with a traced reason each. Keeping it at zero is the standing work.

4. **Invariants in the soak, not just thresholds.** Liquid conservation is a property: the total in
   a sealed world does not change however many passes run. FIX-B found its blocker by measuring
   exactly that across nine release sizes, where the existing tests used 40x30 worlds, pools of at
   most 180 tiles and zero fall distance. The same shape applies to tile-state legality, NPC
   position bounds, and drop distributions over many kills.

5. **Generate the golden pins.** The Frost Legion and Pirate invasion sizes were swapped *and a
   test asserted the swap*, so a correct implementation would have failed the suite. That is
   structurally impossible when a constant transcribed from vanilla is codegen output rather than
   hand-typed: a wrong pin cannot survive being derived from the source it exists to pin.

6. **A flaky CLI test is an unmeasured test, and A/B-ing one needs a matched rebuild.**
   `new_world_cli` fails whenever the world file never lands, timing out on the full 120s. It
   reproduces only when the whole *test binary set* has just been relinked, which any lib-file
   change causes; `touch`ing `main.rs` rebuilds the binary alone and hides it completely. Measured
   the wrong way, `main` looks clean 7 runs out of 7 and whatever branch is under test looks guilty
   3 out of 3. Measured with a matched relink, `main` fails 3 runs in 4, so it is pre-existing and
   any A/B that does not relink both trees identically will confidently blame the wrong change.
   Eliminated by measurement, each: the network (5 failures in 8 with the update check and UPnP
   both off), autosave (15 saves in 15s at `autosave_secs = 1`, first at 1.108s), leaked processes
   and held ports (none at the instant of failure; every test port is unique), first-execution
   code-signature validation (0.42s), and machine load. It is a heisenbug: a diagnostic that dumps
   the child's output on failure stops it reproducing.

   **The lead worth following**, seen by a second observer during the pillar lane: `Command::spawn`
   itself returning `ENOENT`. That is not a timeout, it is the binary in `CARGO_BIN_EXE_terrustia`
   not being there at the instant of the exec, which is exactly what a concurrent relink of
   `target/debug/terrustia` would produce. It explains every observation at once: only after a
   relink, never in isolation, worse under load, and heisenbug-shaped because any diagnostic moves
   the exec relative to the write. A world file that never lands inside 120 s is the same fault seen
   from further away, since a server that never started cannot write one. The same observer
   disproved the obvious alternative rather than assuming it: the scratch dirs are keyed off
   `SystemTime` nanoseconds, and four concurrent readers collided 0 times in 2000 trials at macOS's
   1 us clock resolution. Next step is to `stat` the binary immediately before spawning and record
   its inode and mtime on failure; if that confirms it, copy the binary once per test and exec the
   copy, which `tests/bare_server_boot.rs` already does for a different reason and has never flaked.
   Full write-up in `.scratch/audit-2026-08-30/FLAKE-new-world-cli.md`.

   `tests/shutdown_signal.rs::sigterm_stops_the_server_and_saves_within_a_bounded_window` belongs to
   the same class, found and measured 2026-09-04: it fails "the server should have reached its main
   loop by now" in well under a second rather than timing out at its real 30s budget, which is
   `wait_for_line`'s `Err(_) => return false` firing on a disconnected channel, not a slow boot. A
   git-worktree bisection wrongly pointed at that day's web-panel merge on single-shot evidence
   before five-run measurements at both that commit and the true pre-session base showed the same
   1-in-5 failure rate at each: pre-existing, not a regression, and any single-run A/B on this test
   will falsely implicate whatever commit happens to be under test. Same open next step as above.

**Explicitly not on this list: another audit pass by reading.** The C3 pass found 99 findings and
still missed Bone and the absent upward wake in `Liquid.Update`, both of which turned up during
fixing, and both of which were found by building an instrument rather than reading harder. Reading
passes are good at discovering a class and poor at exhausting one, and they are not repeatable, so
a clean one carries little information. Use them to find the class, then automate the class.

## Phase 2: release qualification

Per release candidate: the manual differential against a real `TerrariaServer` (`probe`/`verify`;
hosted CI can never hold the game); `just check-data` against the decompiled tree; and the
255-player qualification run, separate from per-commit CI, with an objective bar: 255 real headless
clients join a full-size world and hold 30 minutes, zero server panics, no disconnect storm, p99
tick under the 16.67 ms budget, peak server RSS under 1 GiB, and a clean world save under load. The
human
fresh-world Moon Lord playthrough is strongly expected, waivable only if the automated and
differential evidence is otherwise complete; anything found becomes a test. Final verification:
`just check` green across the six targets, fuzz and soak green, zero production panics on
hostile or environmental paths, zero unknown protocol IDs, the admin overhaul verified against a
real client and Playwright, no confirmed performance regressions. Then tag v0.0.1.

**The bar is enforced, not narrated.** `tools/soak_scale.sh` judges every clause of it and exits
non-zero on any failure. That is worth writing down because for a long time it did not, and three of
the five clauses were unmeasurable or unmeasured:

- **p99 tick had no data source.** `cpu_us` reached the log only through the stall branch, which
  fires when the *machine* is held off the processor, so the number quoted as tick cost was the cost
  of whichever tick happened to coincide with a hitch. A thirty-minute run produced five samples, all
  stall-coincident; a clean run produced none. The per-window line that carries the real figure was
  `debug` while the server defaults to `info`, and the warning for a genuinely over-budget tick named
  the same quantity `worst_us` rather than `cpu_us`, so the one line that mattered was the one the
  harness could not read.
- **Client retention counted the wrong thing.** The soak client discarded its send result and let a
  read error break only its inner drain loop, then returned success unconditionally, so a client
  whose connection the server had closed slept out its hold and exited zero. Runs where the server
  dropped 218 of 255 clients were recorded as `255/255 connected and held`.
- **Memory was printed, not judged.** The curve went to the output for a reader to interpret, which
  is how a run climbing to 689 MiB was recorded as a pass.

**Memory: peak RSS under 1 GiB at 255 players.** A number rather than "stable", because the adjective
is what let a multi-gigabyte figure stand unchallenged. What that figure was measuring turned out to
be the soak client failing to read: capped near 2,100 events a second, its receive window closed, the
server's outbound queues backed up behind it, and the kernel eventually killed connections with
`ETIMEDOUT`, which reads exactly like a server shedding clients under load. With the client draining
properly, a 255-player half-hour holds around 140 MiB and peaks near 200. The runs that exceed the
ceiling are the ones taken while the test box itself is contended, and the external-stall count in
the same output is what distinguishes the two. A thirty-minute run cannot separate a slow leak from
burst working set, so the bar tests the ceiling and leak detection stays with the extended soak.

**The p99 clause has one known cause and it is fixed but not yet re-measured.** The first 255-player
run failed the gate at `p99 tick 23430 us`, and the largest single contributor was traced:
`BiomeCache` bought one biome scan per player with no per-tick bound, so a join burst put 255 scans
of ~78 us on one tick and their entries, sharing an `at`, then expired together sixty ticks later. A
real run measured `phase=spawning phase_us=20763`, which is 266 scans in a single tick. `BUDGET = 8`
now caps the scans one tick may buy, which spreads the next expiry across 32 ticks by construction
and re-spreads after any resynchronising event, for a guaranteed 624 us/tick at any player count.

Two things about that are worth keeping: the instrument said the cost was 345 us and was right about
the wrong question. `examples/biome_scan_cost.rs` drove **one** slot for 60,000 ticks, and a per-tick
mean over one player cannot see a per-tick maximum over 255. And the fix sat unmerged on a branch
until a branch audit found it, so the gate went a long time without being re-run against it.

**Re-measured 2026-09-01, and the fix is load-bearing.** Four 255-player half-hours plus one
deliberately neutralised run, all on the same box, `tools/soak_scale.sh 255 1800`. The `shed` column
is the server's own `outbound queue full` count, which runs 1 to 3 were blind to (see below), and
`stalls` is the run's external-stall count, which is how contended the box was:

| run | p99 tick | median | max | peak RSS | shed | stalls | verdict |
|---|---|---|---|---|---|---|---|
| 1 | 8711 us | 4654 us | 8884 us | 169 MiB | 0 | 3 | pass |
| 2 | 8652 us | 4584 us | 9233 us | 1536 MiB | 3 | 35 | fail (memory) |
| 3 | 4803 us | 2726 us | 5177 us | 600 MiB | 5 | 10 | pass |
| 4 | 4861 us | 1935 us | 6227 us | 206 MiB | 0 | 1 | pass |
| `BUDGET = u32::MAX` | 41688 us | 18961 us | 41688 us | 96 MiB | 0 | 0 | fail (p99) |

Run 4 is the qualifying one: every clause met, nothing shed, one external stall in half an hour, and
it is the only run made with the harness counting sheds. Runs 2 and 3 are kept in the table because
the memory clause is what separates them from run 4, and the stall column is what separates those.

The neutralised run is the evidence that the cap is what holds the p99 down and not the weather: with
`BUDGET` removed every one of its 29 windows was over budget, every one of them with
`phase=spawning`, worst `phase_us=40322`, which is the original failure's own signature
(`phase=spawning phase_us=20763`). With the cap in place, across 702 windows of the four held runs,
not one window went over budget at all; four passed *half* budget, and the worst `spawning` cost in
any of them was `phase_us=6431` against the uncapped run's 40322. **The p99 clause is met**, between
a quarter and a half of the budget with the cap and at two and a half times the budget without it.

**Two things the same runs turned up. The first is now closed; the second is still open.**

**The retention clause could not fail for the reason a real server sheds players.** It counted client
exit codes, and a shed client cannot see that it was shed: `send_bytes` removes the player from the
world the moment its outbound queue fills, but the socket only closes once `write_loop` has drained
everything already queued behind that decision, and at `outbound_queue(255)` that is about a million
frames. Runs 2 and 3 shed 3 and 5 clients with the queue at 1,052,626 and 1,052,669 of its 1,052,672
capacity; all eight printed `done after 1800s`, exited zero, and were recorded as `255/255`. The
harness now subtracts server-side sheds, so those runs read 252/255 and 250/255, and run 4 reads
`255/255 clients connected and held (255 exited clean, 0 shed by the server)`. Confirmed by
controlled experiment rather than deduction: rebuilt with a 9,212-frame queue, the shed client
reports `the server closed the connection` within a second, because the backlog it has to drain
first is small enough to clear.

**The server half of that is now fixed.** A shed client is disconnected when the server decides to
shed it, not after replaying a half-hour-old backlog at whatever rate it can read. Dropping the
`Sender` could never do it, because `mpsc` still hands the buffered frames to `write_loop` before
`recv` returns `None`, so the decision travels on its own channel instead: `net::connection::Closer`,
a `oneshot` created in `serve`, carried on `Player`, selected on inside `write_loop` (`biased`, the
close first), and fired by `GameServer::disconnect` with the player already out of its slot. Every
path that removes a player gets it, not just the backpressure shed: `/kick`, the panel's kick and
ban, and `reap_stalled_handshakes`.

Two properties of that are load-bearing and pinned by tests in `net/connection.rs`. Its payload is
the one frame a closed connection is still owed, so `kick` no longer queues its notice *behind* the
backlog (where a client a million frames down would read it half an hour late, if at all) and puts
it on the wire ahead of it instead. And dropping the closer without firing it is deliberately not
the same thing: that is what the game task's own shutdown does, and `/stop` and the rollback path
both announce to everybody before they stop, so those frames are still owed and `write_loop` drains
for them exactly as it always did.

What it does not close: the decision is noticed between writes, not during one, so a client that has
stopped reading its socket altogether still holds `write_all` for as long as the kernel will wait.
Cancelling a write mid-frame would fix that and cost every kick notice its readable stream, so the
bound is one `WRITE_BATCH` (64 KiB) rather than nothing at all - against the million-frame backlog
this exists for, one batch is the difference between seconds and half an hour.

**The memory ceiling and the retention bar are in tension at this queue depth.** Run 2 breached the
1 GiB ceiling at 1536 MiB, and the mechanism is the same million-frame queue. `queue_peak` reports
only the *deepest single* connection in a window, so it under-reports the total: run 2 held it at or
just under the 1,052,672 ceiling for many consecutive windows, and one connection's backlog alone
does not account for 1536 MiB, because run 4 held 800,668 frames on its deepest connection at a peak
of 206 MiB. The backlog was spread across many connections at once. 255 slots times 1,052,672 frames
is a theoretical ceiling in the tens of gigabytes, and nothing bounds the sum.

`connection.rs` picked 4,096 per player to stop drops, which is the right trade for the retention
clause and the wrong one for the memory clause, and the two have never been measured against each
other. Which way a run falls tracks how contended the box is: peak RSS ran 1536, 600, 206 and 169 MiB
against external-stall counts of 35, 10, 1 and 3, and `vm.swapusage` sat at 1.7 to 1.8 GiB of 3 GiB
throughout. Qualifying the memory clause honestly needs a box that is not already paging; run 4 is
the closest this one got.

**The extended multi-hour boss soak is waived for v0.0.1** and carried to the next release. Its
distinct value over the thirty-minute run is leak detection over a long horizon, and the shorter run
now enforces a memory ceiling rather than printing a curve, which covers the failure mode that
matters most for a first release. Recorded as a decision rather than skipped quietly, because the
lesson of this qualification work is that an unenforced stated bar is indistinguishable from a met
one.

**Three more silent gaps found 2026-09-01. All three are closed as of 2026-09-04**, and each entry
below now records what actually turned out to be true rather than what the first read assumed. Found
by reading the over-reach list (41 entries then, 54 now that `check_spawn_reach.py` can read
`NextFromList` and the critter lanes made more types reachable; all of the original 41 checked and
cleared: King Slime via boss-summon, NutcrackerSpinning and the Crawdad/Shelly/Salamander family
already documented checker false-positives, the DD2 roster via its own real spawner) and then reading
the unreachable list for anything gating real content rather than cosmetic variety.

The unreachable list itself went 88 -> 3 over 2026-09-03/04 (seven merged lanes: gem critters,
ordinary critters, water/beach critters, the hornet families, the underground fairies, the
Halloween/graveyard/night roster, and the Palworld encounter). What is left is three types that are
*correctly* unreachable and will stay that way: 450 and 451 are dead in the game's own shipped source
(`num56` drawn once, tested against zero three times, `NPC.cs:5120-5138`), and 691 sits behind
`RollOnlyBadLuckExtreme`, which never fires at the luck this server does not model.

- **NIGHTZOMBIE**: ~~the whole Halloween/Graveyard/full-moon night roster is unwired~~ **done.** Two
  lanes, and the note above was wrong about the second half of it in two ways worth recording. The
  detection it said did not exist was the easy part: `world/calendar.rs` answers `Main.halloween` and
  `Main.xMas` off the real date, and `ZoneGraveyard` never needed a tile scan at all, because the
  client counts its own tombstones and sends the answer up in packet 36 like every other zone
  (`Player::in_graveyard`). The line range was wrong too: the chain is `NPC.cs:4539-4816`, and its
  last eighty lines are not a season's at all. They are `GetZombieSettings` (`NPC.cs:5595-5619`) and
  the ordinary night's own fallthrough, which is where the Torch Zombie (one attempt in five for a
  player still on their starting hundred health), the seven plain zombies, the expert armed family
  and the Armed Zombie Eskimo live. The caverns' sibling chain ends the same way, in the plain
  Skeleton and its three look-alikes with the Bone Throwing Skeletons above them. Doctor Bones (52)
  turned out to be neither: his arm is jungle grass underfoot and a dark sky, at any depth
  (`NPC.cs:3772`). `progress.rs`'s `downed_halloween_king`/`_tree` are Frost Moon boss flags, not
  calendar tracking, and must not be confused with this. Three types stay in `docs/spawn-gaps.tsv`
  on purpose: 450 and 451 are dead in the game's own source (`num56` is drawn once and tested
  against zero three times, `NPC.cs:5120-5138`), and 691 is behind `RollOnlyBadLuckExtreme`, which
  never fires for a player whose luck this server does not model.
- **DESERT**: ~~the entire hardmode Underground Desert roster is missing~~ **done.** DesertGhoul x4
  (including the corruption/crimson/hallow-tainted variants), DesertLamia x2, SandShark x4,
  SandElemental, DesertDjinn, the giant antlions and TombCrawlerHead are all off
  `docs/spawn-gaps.tsv`: the desert pool is no longer Vulture/SandSlime/Antlion alone. Verified
  2026-09-04 by their absence from the gap file rather than by reading the pool, which is the check
  that would have caught this note going stale in the first place.
- **TAXCOLLECTOR**: ~~two town NPCs can never be recruited because the mob each comes from never
  spawns~~ **done, and the premise was wrong by the time it was written.** Both mobs spawn: neither
  534 nor 453 is in `docs/spawn-gaps.tsv`. The Tortured Soul has its own underworld-hardmode arm
  (`TORTURED_SOUL_ODDS`, `spawn.rs`), and the item interaction this note predicted would need "a
  general item-vs-live-NPC interaction, which nothing in this server currently has" already exists:
  Purification Powder arrives as packet 140 and the *server* settles its effect
  (`dispatch.rs:3869-3889`), turning a Tortured Soul and setting `saved_tax_collector` through
  `Server::tick_powders`. `rescues.rs:26-33` explains why the Tax Collector is deliberately absent
  from the `Rescue` table rather than missing from the game. The Skeleton Merchant is likewise fully
  wired: its own cavern arm at `SKELETON_MERCHANT_ODDS`, an explicit exemption from the town-NPC
  despawn rule (`npc_ai.rs:501`), and shop handling beside the Old Man and the Travelling Merchant
  (`systems.rs:3072`).
- **PALWORLDPAL**: *done*. The two distressed Palworld pets (695, 696) are off `docs/spawn-gaps.tsv`
  with the whole encounter behind them rather than the spawn alone. The ambient arm is the surface
  day's (`NPC.cs:4374-4389`, inside the daytime block at `:4202`): more than `maxTilesX / 8` from
  world spawn, on tile 2/147/60/161, one attempt in 160, neither of the pair already out, and which
  one arrives decided by whether the player is carrying item 5663 or 5664 with a 40% default. That
  last clause makes this the one arm in `spawn.rs` that reads an inventory, and `Player.HasItem`'s
  own 0..58 slot bound is transcribed rather than flattened to "anywhere on the player".
  `AI_127_Pal` (`NPC.cs:43379-43478`) now runs `CultistRitual.CheckFloor2` (`CultistRitual.cs:133-159`)
  on its first tick and deletes itself where there is no floor, raises two Goblin Archers on the
  spots it finds with `ai[3] = -(whoAmI + 1)` on each, and hands over item 5663/5664 two seconds
  after a player reaches an unguarded pet (`AI_127_Pal_GiveRewerd`, `:43481-43489`). The guards' own
  half is `AI_003_Fighters`' type-111 branch (`NPC.cs:57553-57592`), which was also missing: a
  back-linked archer stands over its pet and faces it until it is hit or somebody comes within two
  hundred pixels. The three pieces of plumbing this wanted are `Spawn::handle` (the slot write-back
  `Spawn::parent` could not stand in for, because `parent` also makes the spawn a *part*),
  `Effects::reward` (an item handed over rather than looted, so nothing goes near the kill path) and
  `World::own_escorts` (which replaces a world-wide count of NPC 111 that let any Goblin Archer
  anywhere hold every pal in its waiting state). Two smaller fixes fell out of the read:
  `timeLeft = activeTime` is an assignment to 750 (`NPC.cs:6188`), not the `max(3600)` that was
  there, and vanilla's drag runs after the routine's early returns rather than before them. Not
  transcribed, all client-side: the pain and joy sounds, and the Foxsparks' own `Lighting.AddLight`
  glow (`:43432-43454`, `:43471-43474`).

## Phase 3: after v0.0.1, in order

1. **v0.0.2, the worldgen release**: the seven remaining secret seeds' generation content (Not the
   Bees, Drunk World, Remix, Celebrationmk10, "get fixed boi", Don't Starve, Skyblock; Don't
   Starve alone touches 53+ scattered branch points across nearly the whole of `WorldGen.cs`, and
   the others are comparable or larger) and the 7 of 15 remaining micro-biomes (each needs a
   genuinely separate subsystem: a trappable-chest mechanism, a second tree-growth engine, a
   wandering-tunnel shape, and so on). The six deferred drop-table gaps ride along: five need
   Remix's own generation content, the sixth is the documented npc-44 nested-fallback shape.
2. **Regions and spawn protection**: the first built-in addition.
3. **The plugin API**: Rust first (permissions land in v0.0.1, so the model exists; regions and the
   admin interfaces settle into real use cases first), then C# once the host API has proven
   itself. Event hooks, commands, permissions (reusing E1's registration path), opaque handles,
   validated operations, lifecycle, storage, unload/reload semantics; never expose
   `&mut GameServer`; the ordinary server stays self-contained with no .NET runtime.
4. **The optimisation campaign**, after the feature waves: eliminate unnecessary work, then
   algorithms and data structures, then memory/allocation/layout (the ~10 MB idle figure is an
   aspirational research target tracked per component, not a promise), then CPU hotspots, then
   safe parallelism, then explicit SIMD (AVX2/AVX-512, NEON, RVV, runtime dispatch, scalar
   fallback everywhere), then generated-assembly inspection, then hand-written assembly where it
   measurably wins. Profile first; every accelerated path proven bit-identical against the
   reference with fuzzing, Miri and sanitisers where applicable.
5. **Server-side characters**: much later; its own properly designed storage and auth system.

**Deferred with reasons written down** (carried from the audit trail and the tick-rate research):
- **Higher tick rates (120/240/480)**: demoted to an off-by-default experimental research note.
  Unmodified clients are hard-pinned to 60 Hz (fixed-timestep accumulator), run their own NPC AI
  prediction and advance world time themselves, so a faster server mostly manufactures desync; the
  mechanical "configurable multiple" alone is a large scattered-literal sweep where every miss is a
  silent pacing bug. Picked up only when a concrete need appears.
- **One-time join credentials**: a short-lived single-use token issued through the panel and typed
  into Terraria's normal password prompt, so a reusable password never crosses the plaintext
  protocol. Does not encrypt the connection and does not stop an active MITM; it makes an observed
  credential worthless.
- **TShock-style built-ins** (warps, restrictions, richer moderation) so normal administration
  never requires plugins.
- **Seed-identical world generation**: 219-372 engineer-days by the standing estimate; the oracle
  is built and green (`docs/worldgen-parity.md`). Generation is complete and playable; it is not
  Terraria's world for a given seed, and closing that is its own campaign.
- **Steam P2P (friend invites)**: needs the Steamworks SDK under AppID 105600 and a licence
  decision against the AGPL. Protocol-level Steam support is already complete; a Steam-launched
  client connecting by IP is byte-identical to any other.
- **Operational polish**: config reload without a restart; a general log-file sink with rotation
  (the moderation audit log in Lane E is separate and is in v0.0.1).
- **The extended multi-hour boss soak**, waived for v0.0.1 (see Phase 2) and due for the next
  release. It is the only run long enough to tell a slow leak from burst working set, which the
  thirty-minute qualification run explicitly does not attempt.

## TUI and hosting polish (opportunistic, never derails release work)

- **Smooth-gradient boot logo via a terminal image protocol.** The 5-row block-glyph banner cannot
  match `docs/assets/banner.svg`'s gradient as text. Plan: bake `docs/assets/boot-logo.svg` (the
  text-free transparent source) to a 2x transparent PNG offline via `rsvg-convert` and
  `include_bytes!` it; hand-roll the iTerm2 OSC 1337 and kitty graphics emitters (skip sixel);
  detect via the `supports-terminal-graphics` env heuristics gated behind `Palette::is_enabled()`,
  treating `TMUX` as unsupported; fall back image -> 256-colour `banner()` -> plain. No new
  dependencies: a ~15-line base64 encoder plus two emitters; explicitly not `viuer`. Gotcha:
  cursor advance after the image differs between iTerm2 and kitty.
Both of the other entries that were here are done and are recorded in git rather than left as
open items: the hanging indent for wrapped log lines (`term::wrap_ansi`, with word-boundary
breaking), and narrow-terminal awareness for the boot card (`term::info_block_at`, which wraps a
value under its own column instead of falling back to column 0 below about 70 columns). Fixing the
second turned up three arithmetic bugs underneath it, all now closed: `visible_len` counted every
character as one column when CJK and emoji are two, `wrap_ansi` hardcoded `\r\n` where a piped log
wants `\n`, and a word break carried its tail down without checking it fitted, which put a
47-column row in a 46-column terminal.

## Dependency pruning (decided; the record stays visible)

The default server build resolves **133 external crates**, down from 171. **Decision (2026-08-29):
stability over crate count.** The only cut made is hand-rolling UPnP away from `igd-next`, **done
2026-09-01** (-38 crates, no new dependencies). Everything else that could be cut is a working, in
several cases already-verified subsystem and is deliberately kept: a mature dependency is worth
more than the crates it costs, and rewriting one resets verification that has been earned.

Measured with `cargo tree -e no-dev --workspace --no-dedupe`, counting unique crate names and not
counting the four workspace members themselves (plain `cargo metadata` returns a feature-unified
maximal graph and undercounted `igd-next` by 20 crates; do not trust it for feature-gated
ownership). Exclusive ownership, as measured 2026-08-29: `igd-next` 31 (hand-rolled away; the real
figure turned out to be 38), `ureq` 13 plus `tempfile` 2 (keep: rustls/ring is the irreducible core
of a secure `terrustia update`), `rust-embed` 8 (keep), `crossterm` 7 (keep: Windows console path),
`toml` 6 (keep, deferred), `tracing-subscriber` 4 (keep, deferred), `argon2` 4 (keep: the one KDF
this workspace does not hand-roll), `axum` 23 with `rust-embed` (keep: the panel is
Playwright-verified and a transport rewrite resets that to zero). The combined `igd-next` +
`rust-embed` + `axum` lever (171 -> 96) is recorded in git history with the full crate list; only
its first third was taken.

**The UPnP hand-roll: done.** Only `search_gateway` and `add_port` were ever used, and UPnP-IGD
control traffic is plain HTTP/1.1 over the LAN with raw `IP:port` literals, so none of
`url`/`idna`/ICU was ever needed. `crates/terrustia/src/upnp.rs` now does the job itself: an SSDP
M-SEARCH datagram, a LOCATION parse, a tolerant tag scanner for `serviceType`/`controlURL` and SOAP
faults, `controlURL` resolution against `URLBase`, URL splitting, the `AddPortMapping` envelope, and
HTTP/1.1 framing (`Content-Length` and chunked). Every decision is a pure function over a `&str`;
the socket I/O is a thin shell of one multicast send/receive, one GET and one POST around them.
`pub async fn attempt(listen: SocketAddr)` and its behaviour are unchanged.

**Measured, not estimated: 171 -> 133 external crates, -38**, against the -31 this section had
estimated. Removed: `igd-next` and its `hyper`-side stack (`h2`, `attohttpc`, `want`, `try-lock`,
`fnv`, four `futures*`), its XML parser (`xmltree`, `xml-rs`), `url` and the whole `idna`/ICU tail
(`idna`, `idna_adapter`, six `icu_*`, `zerovec`(+derive), `zerotrie`, `zerofrom`(+derive), `yoke`
(+derive), `tinystr`, `writeable`, `litemap`, `utf8_iter`, `potential_utf`, `stable_deref_trait`,
`displaydoc`, `synstructure`), and `chacha20`, whose yank from crates.io broke CI once already
(see `AUDIT.md`). Nothing was added.

The parsing is unit-tested against captured output from real routers, cited in each fixture: two
SSDP search responses (a Wanadoo Livebox and a Linksys 802.11b, from miniupnpd's own `minissdp.c`),
two device descriptions (a LINKSYS WAG200G and a Sagemcom Livebox, from miniupnpc's parser test
corpus), and the success and fault bodies miniupnpd's `upnpsoap.c` actually builds. Fifteen
deliberate mutations of the pure functions were each confirmed to fail a named test, the same
"a checker that cannot fail is not a checker" discipline `just check-mutants` applies to the
generated tables. A sixteenth survived, and was right to: the condition it broke was redundant
with the all-digits port test beside it, and is now deleted rather than left untested.

It also took the workspace's only file-level-copyleft dependency with it. `attohttpc` (MPL-2.0)
came in through `igd-next`'s blocking search path, and the AGPL/MPL compatibility argument
`deny.toml` used to have to make no longer arises: that allowance is removed and `cargo deny
check` is clean without it.

Three real gaps in `igd-next` closed on the way past: it only searched for
`InternetGatewayDevice:1` and only accepted three service types, so an IGD:2 router advertising
`WANPPPConnection:2` (the Livebox fixture is one) was invisible to it; it ignored `URLBase`, which
is wrong for a router serving its description and its control endpoint on different ports; and its
tokio SOAP client had no timeout at all. Discovery timeouts are unchanged at its own 10s/5s.

**Still unverified, and the reason this is not "finished":** the live socket path has never run
against a real UPnP router, and CI cannot run it, having no IGD to answer an M-SEARCH. Closing that
means a human running `cargo run --example upnp_probe` on a real home network behind a real
UPnP-capable router and confirming the mapping appears in the router's own port-forwarding table.
`upnp.rs`'s own module doc says the same thing where someone changing it will read it.

## Release

Tag v0.0.1 when the Phase 1 gates are met and Phase 2 passes. Do not hold the release for
post-release nice-to-haves.
