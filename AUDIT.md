# Audit

What's actually been checked, what was wrong when it was, and how each finding was verified fixed
rather than assumed fixed. This is not a marketing document — a finding that's still open says so.

The pre-roadmap ledger this was drawn from (`plan.md`, folded into `TODO.md` and kept in git
history) was updated the moment a finding landed; this file is the readable summary of it.

## How audits happen here

Every finding below was caught by one of three things, never by review alone:

- **Measuring instead of assuming.** Several of the worst findings here were bugs *in a claim*, not
  just in code — a comment saying banner kills survived a restart, a metric mixing CPU time and
  wall time, a sizing estimate made without reading the source it was estimating. In each case the
  fix was to go measure the real thing and correct the claim, not to trust what was already written
  down.
- **A test that fails on the unfixed code, before it's made to pass.** Reverting a fix and watching
  the test go red is the actual bar, not "the fix looks right." Several rows below cite the exact
  assertion that failed.
- **Cross-checking against something outside this codebase's own account of itself** — the real
  decompiled game source, a real Terraria client, a real `TerrariaServer`, or (for the CI findings
  below) an actual GitHub Actions runner. A codebase cannot audit its own blind spots by reading
  itself more carefully; every serious finding here came from checking against something external.

## Data safety

The worst class of bug this project can ship is one that silently loses or corrupts a player's
world. Two were found and fixed:

| Finding | What it did | Verified fixed |
|---|---|---|
| Hardmode ore tiers didn't survive a save of a loaded world | Silent data loss — reloading a saved world could scramble which ore a Hardmode altar smash produces, corrupting an in-progress playthrough | `an_altars_ore_choice_survives_a_save` fails on the unfixed code (`left: -1431655766`, an uninitialized sentinel); real-world round trip afterward: `ores [221, 223, 227]` |
| Banner kill counts didn't survive a save | Silent data loss, and worse: a comment in the code claimed the opposite | `banner_kills_survive_a_save` fails on the unfixed code; real-world round trip: `banner 3 = Some(4242)` |
| A blood moon or eclipse in progress reset on save | The bytes on disk were always correct — the running session just forgot to write them back | Test verified failing on the unfixed code before the fix |

The structural fix behind all three: `World`'s persistence path is a destructure with no `..`, so
adding a field without deciding whether it survives a save is a compile error, not a silent gap.
Verified by adding a field and watching the build break — and it has caught a real omission since
(the blood-moon/eclipse row above).

Also fixed: `.wld` files newer than this server understands used to be misread and corrupted on
save rather than refused by name; and a truncated townsfolk or tile-entity section used to delete
every resident, or every pylon past the first bad entry, rather than keeping what decoded.

## Availability

| Finding | What it did | Verified fixed |
|---|---|---|
| A panic on the packet path — the untrusted-input path — wasn't caught | It unwound past the shutdown save and the process still exited `0`, so `Restart=on-failure` never fired. A malicious or malformed packet could kill the server *and* skip saving the world | Test verified failing without the guard |
| `/register` ran Argon2 inline on the game task, with no rate limit | Tens of milliseconds against a 16.67 ms tick budget — one client hammering `/register` could freeze the whole server for everyone connected | Now off-task, hashing capped per slot and server-wide, released on disconnect. Test measures 64 registrations against a hash it times first |
| The accept loop had no connection ceiling or per-address cap | Unbounded connections, and an idle-but-open socket held its slot forever (the idle timeout reset on *any* byte, including a slow trickle) | Both verified failing with the guard removed |

## Correctness

| Finding | What it did | Verified fixed |
|---|---|---|
| The Lihzahrd Altar was never placed in generated jungle temples | Made Golem **unreachable in every world this generator had ever produced** — a real client refuses the Power Cell interaction without an altar tile nearby, and there is no server-side workaround for that, since the gate is entirely client-side | Found by a worldgen sizing pass reading vanilla's source, not by anyone playing the game. Tested across 40 seeds and the full range of rolled temple sizes; verified failing on the unfixed code |
| `tick()`'s phase costs were measured on two different clocks | `worst_us` was CPU time, `phase_us` was wall time, so a phase could appear to cost more than the tick it ran in, and every phase figure this project had ever logged was inflated | Both now measured on the same clock; a dedicated test sleeps to force the two clocks apart and confirms sleeping time isn't charged as work |
| Announcements sent literal English text | Vanilla sends a localization key with substitutions, so every non-English client was reading text this project wrote rather than their own language's real string | Fixed for all but four keys this project couldn't independently verify — those are kept in English deliberately, named as such, rather than guessed at |
| The in-game housing screen did nothing | Packet 60 inbound fell through to an ignore arm — dragging an NPC into a room, or evicting one, had no effect | Driven over a real socket; verified failing without the dispatch arm |
| `tools/check_drops.py`, the tool meant to catch missing enemy drops, had seven real parsing bugs | It produced false positives severe enough to briefly "prove" Eye of Cthulhu owed the player an Iron Pickaxe — a checker that cries wolf gets ignored, which is worse than no checker | Every false positive traced by hand against the game's real drop-registration source before trusting any of them. With the checker actually correct, it found three genuinely missing boss trophies (Moon Lord, Empress of Light, Deerclops) and a missing weapon pool (the Martian Saucer) — all fixed. Boss loot now has zero remaining unjustified gaps |

## Estimates that were wrong, corrected rather than left standing

Two sizing estimates for remaining worldgen work were made without the ability to read the game's
decompiled source (it had been wiped mid-session by an OS tmp-reaper). Both were corrected once
source access was restored, rather than quietly superseded:

- An early pass estimated Tier 2 worldgen needed a **~5,253-line** port of the game's internal
  shape/structure framework before any of ten remaining biome set-pieces could even compile.
  Measured directly against restored source: **zero** of those passes actually call that framework.
  What they use is a ~200-line overlap-rectangle tracker, now built.
- The same restored-source pass found that a shipped fix (the Lihzahrd Altar, above) uses a
  different mechanism than vanilla's own dedicated, unconditional placement pass — functionally
  verified and passing, but flagged as a structural difference worth a future look rather than
  silently left unmentioned.

## CI and release infrastructure

"Written and validated, never run" is not the same claim as "works." This repository's GitHub
project, and its CI/release/container workflows, existed and passed local review for a long time
before a real push against a real runner ever happened. The first one found four independent bugs
that no amount of local `cargo check` could have caught:

| Finding | What it did | Verified fixed |
|---|---|---|
| CI and the container build triggered on a `main` branch | Every commit in this repository's history has been on `master`. Ordinary pushes would never have run CI at all | Both workflows repointed at `master` |
| Cross-compile targets were installed into the wrong Rust toolchain | `rust-toolchain.toml` pins a specific Rust version; the workflows installed Windows/macOS/musl targets into the unrelated `stable` channel. `cargo build --target` inside the repo always resolves to the pinned version, so it found the target missing | Targets declared directly in `rust-toolchain.toml`, the same self-declaring mechanism it already used for components |
| `crossterm`'s Windows backend couldn't compile at all | This workspace disabled `crossterm`'s default features and only re-enabled one, dropping the feature that gates the Windows-specific crates its own source needs to build. Untestable from a macOS/Linux development environment — only a real Windows runner could catch it | The feature re-enabled; every release target, Windows included, now builds clean in CI (five at the time, six since Windows ARM64 joined the matrix) |
| The first autosave after server startup cost 89% of a tick's budget | No prior snapshot buffer existed to diff against, so the first save did a full 40-megabyte copy inside a counted tick — 14,833 µs against a 16,666 µs budget. Every save after the first was already fast (150–200 µs); this gap was invisible to any test that didn't run a real soak against a freshly-started server | Fixed by building that buffer during startup, before any tick is being counted, rather than inside the first one. Two tests pin the property directly, both verified failing on the unfixed code first |
| Every CI run failed for 44+ hours and 30+ commits, undetected — nobody was checking `gh run list` after pushing | Three independent, pre-existing infrastructure bugs, none caused by the commits landing during that window. **(1)** The fuzz job: `rust-toolchain.toml`'s pin silently wins over the nightly toolchain `dtolnay/rust-toolchain@nightly` had just installed, for any cargo invocation run from inside the checkout — the same failure class this table already lists once, for cross-compile targets. `cargo-fuzz` had never actually fuzzed anything in CI; it failed on the sanitizer flags before running a single input. **(2)** Both musl targets, in `ci.yml` and `release.yml` alike: `ring` (pulled in transitively through `rustls`, via `ureq`'s HTTPS client for the update checker) runs real C compiler-family detection in its build script, which a linker override alone can't satisfy. Real release artifacts for `x86_64`/`aarch64-unknown-linux-musl` would have failed to build had a tag been cut during that window. **(3)** `cargo-deny`: `chacha20` 0.10.1 (via `igd-next`'s UPnP stack) was yanked from crates.io after this `Cargo.lock` was generated, so CI's fresh advisory-db pull failed where a locally-cached one didn't | Fuzz: explicit `+nightly` on both `cargo fuzz run` invocations. musl: `taiki-e/setup-cross-toolchain-action`, installing a real musl C cross-toolchain per target. `cargo-deny`: `chacha20` updated to 0.10.2. All three confirmed on a real run against a real GitHub Actions runner (`33121611587`), not just locally — every one of its 9 jobs green, the first fully green run since `2026-08-26T00:13` |
| The server-side clock periodically sent clients a message real vanilla never sends, and a real player watched the sky snap visibly to a different time of day because of it | `tick()` broadcast message id 18 (`TimeSet`) every 60 real seconds, and `/time`/Journey mode's four time-skip buttons sent the same on every use. Grepping the whole decompiled game source found zero calls to `SendData(18)`, anywhere, by anyone — real vanilla's own server never sends this message. The real client's own receive handler for it does a hard, unconditional assignment with no smoothing at all, so any correction that disagreed with the client's own local prediction (which this project's tick scheduler — `MissedTickBehavior::Skip` — can genuinely fall behind on, since a stall like the one this exact session logged is never made up afterward) showed up as an instant, jarring jump rather than a continued smooth flow. Existing test coverage only proved this project's own encoder was internally consistent with its own claim; nothing had ever driven a real, unmodified client against it before this session | `set_time` now resyncs with `broadcast_world_data()` (message id 7), matching real vanilla's own `Main.SkipToTime` exactly and matching this same file's own `skip_to` (the sundial/moondial), which already did it correctly. The unconditional periodic broadcast — which had no real-vanilla equivalent at all — is deleted outright. `tests/gameplay.rs`'s existing coverage had asserted the *wrong* thing (that the time-skip buttons produced a message-id-18 frame, codifying the bug); rewritten to assert a real `WorldData` frame instead, confirmed failing against the reverted fix (a 10-second timeout waiting for a frame that never arrived) before confirmed passing |

The container image itself has been built, pushed, signed with cosign, and smoke-tested serving
with no configuration — genuinely exercised, not just written. Signed releases remain untested:
that workflow only triggers on a version tag, and none has been cut yet — which means the musl
release-artifact fix above is verified through `ci.yml`'s identical `targets` job, not through an
actual `release.yml` run; the two jobs share the same fix but a real tag push has never exercised
`release.yml` itself.

## Open

- Packet coverage: portal-gunning an NPC (100), shop price overrides (104) and the dead-player fade
  cue (135) are the genuinely missing message ids, out of 163 total. `python3
  tools/packet_audit.py` prints this list rather than anyone maintaining it by hand; spectating
  (150) used to be named here and is host migration, which a dedicated server never does.
- Server-authoritative inventory and damage validation is not implemented. This is not an oversight
  — vanilla trusts the client for both, and diverging would change how the game plays. See
  `SECURITY.md` for the full reasoning.
- The remaining feature-completeness gaps are tracked in `TODO.md`, not repeated here — this
  document is about what's been checked for correctness and safety, not a feature checklist. This
  entry used to name four that have since closed (biome set-pieces, Journey mode, town NPC combat
  and "most enemy drop tables"); the live ones are 7 of 15 micro-biome classes and eight secret
  seeds' generation content, both deferred to v0.0.2.
