# AGENTS.md

Master instructions for AI agents (and humans) working in terrustia. This is the canonical file;
`CLAUDE.md` is a symlink to it, so Claude Code and any tool that reads `AGENTS.md` see the same
rules. Keep it current when a convention changes.

## What this project is

terrustia is a Terraria 1.4.5.8 dedicated server, written from scratch in async Rust. Real Terraria
clients connect to it, and the worlds they play are ordinary `.wld` files. The wire protocol is
release 326 (release 325 is accepted too). The version is deliberately `0.0.1`: world generation is
visibly unfinished and the first release should not imply otherwise by its number.

The scope is to transcribe vanilla Terraria faithfully and be honest about the gaps. It is not a
clone that reinvents mechanics; where behavior is defined, it is copied from the game's own shipped
code, and where a gap exists it is disclosed rather than papered over.

## Workspace layout

Four crates (`Cargo.toml` at the root). The default build set is three of them; the codegen tool is
excluded so a bare `cargo build`/`cargo test` never touches it or its `regex` dependency.

- `crates/terrustia-proto` - the wire format, with no I/O: primitives, packets, tile-section coding,
  and the tile/NPC/housing data tables extracted from the game. Licensed MIT on purpose, so any
  Terraria tool in Rust can build on it. Every packet round-trips in a unit test without a socket.
- `crates/terrustia-client` - a headless client that speaks the real protocol. It drives integration
  tests, probes a real `TerrariaServer` for comparison, and runs as a bot.
- `crates/terrustia` - the async server: world state, the game loop, connection handling, `.wld`
  read/write, the console, and the optional web admin panel. AGPL-3.0-or-later.
- `crates/terrustia-codegen` - a hand-run developer tool that regenerates the proto data tables from
  a decompiled Terraria tree. Not part of an ordinary build.

Toolchain: edition 2024, `rust-version = 1.97`, `resolver = "3"`. `bun` is only needed to build the
web panel frontend; the server builds and runs without it.

## Everyday commands

Recipes live in the `justfile`; each is a thin wrapper over `cargo`, so plain `cargo` works too.

- `just run -- --world W.wld` runs a release server (`cargo run --release -p terrustia -- ...`).
- `just dev` builds the web panel and runs a debug server with it embedded (the local iteration loop).
- `just build` builds the panel, then the whole release workspace.
- `just test` runs the workspace test suite; `just test-filter NAME` narrows it and shows output.
- `just check` runs exactly what CI runs: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo clippy -p terrustia --all-targets --no-default-features`
  (the only pass that compiles the panel's disk-serving branch and its `..`-traversal guard),
  `cargo deny check`, the web build, and `cargo test --workspace`. Run it before you call a change
  done. `just check-rust` is the lint pass alone, without the web build or the tests.
- `just soak [SECONDS]` runs a real server with three real clients (the CI soak).
- `just fuzz [TARGET] [SECONDS]` fuzzes a decoder target (needs nightly + `cargo-fuzz`).
- `just regen` regenerates every data table from a decompiled tree (dev-only; see below).
- `just check-data` is the qualification-time data pass, run locally and never in CI, because four of
  its five parts need a decompiled tree that can never ship to a hosted runner. It is `just fuzz`'s
  neighbour, not `just check`'s: run it before a release candidate.
  - `check-drops` and `check-recipes` compare the committed tables against the game's own source.
  - `check-parity` re-checks every vanilla citation in `crates/*/src` against the tree. Rule 2 below
    makes every transcription cite its source, so those ~2000 `NPC.cs:12345` references are data;
    `docs/parity-index.tsv` is derived from them and keys each one by a hash of the cited vanilla
    lines and a hash of our own item's body, so a "checked against the game" claim expires on its
    own and the failure says which side moved. It never judges whether a transcription is *correct*.
    `just parity-coverage` is the other half: what fraction of each vanilla file anything we wrote
    cites, and which large regions are cited by nothing. Rebuild the index with `just parity-update`
    and review the diff, exactly as with the generated tables.
  - `check-spawn-reach` diffs every NPC vanilla's own `NPC.Spawner` can spawn ambiently against
    every NPC this server can, and reports the ones nobody playing here could ever meet. An
    unreachable NPC is silent: nothing errors and no test fails, which is how the whole sky roster
    (the Harpy and the Wyvern) sat in no pool at all. Vanilla's side is read from the tree; ours is
    read from the server by running the one test that prints it. The known gaps live in
    `docs/spawn-gaps.tsv`, rebuilt with `just spawn-reach-update` and reviewed as a diff.
  - `check-dead-writes` reports struct fields written in production and read only by the tests.
    `rustc`'s `dead_code` cannot see these, because a read inside `#[cfg(test)]` counts as a read;
    that blind spot is how thirteen boss-enrage sites wrote `Npc::damage_bonus` that nothing read.
    Needs no decompiled tree.
  - `check-mutants` corrupts the committed tables one entry at a time and requires the checkers
    above to fail. A mutant that survives is a checker blind spot by definition, which is how a
    missing drop stays missing; `--rust` additionally measures the proto test suite, slowly.

## How to work here (non-negotiables)

These are the conventions that matter most in this codebase, drawn from how it has actually been
built.

1. **Investigate first; trace to root cause; verify, do not assume.** This is the project's spine.
   Before changing a system, read vanilla's real shipped source for it and understand the actual
   mechanism. "Audit" here means tracing behavior to its root cause in real code (both the game's and
   this repo's), never summarizing CI or test status. When a result is ambiguous, a test fails in a
   way that is easy to wave off as noise, or a finding would rest on "probably" instead of "verified",
   dig in and check. Investigate before committing. A fix is not done until you understand why the
   bug happened.

2. **Transcribe vanilla, and cite where from.** When you copy a table, a formula, or an AI routine
   from the decompiled game, note the source location in a comment (for example `WorldGen.cs`
   line ranges, or a method name). Disclose every deliberate narrowing or reordering. No decompiled
   game source, game assets, or game text is checked into this repository. The one documented
   exception is the town-NPC name pools in `town_names.rs`, whose provenance is recorded in
   `docs/generated-tables.md`.

3. **Hand-roll narrow, well-defined things; stability over build time.** This workspace hand-rolls
   the game's wire format, TOML-shaped needs, terminal line editing, UPnP IGD port-mapping
   (`upnp.rs`), and similar narrow protocols on purpose. A dependency is taken only where
   hand-rolling would be worse: password hashing (`argon2`), a secure temp directory (`tempfile`),
   config and admin-store TOML (`toml`), logging (`tracing`/`tracing-subscriber`), portable
   raw-mode/key decoding (`crossterm`), the panel's HTTP/WebSocket surface (`axum`), embedding the
   panel (`rust-embed`), the one update HTTP client (`ureq`), and deflate (`flate2` with the
   `zlib-rs` backend).
   Each of those choices is justified in a comment in the root `Cargo.toml`; read it before touching
   dependencies. Do not rewrite a working, already-verified subsystem to shave crates: a mature
   dependency is worth more than the crates it costs, and rewriting resets verification that has been
   earned. The full trade is in `TODO.md` under "Dependency pruning".

4. **Prefer explicit code over derive-heavy convenience.** Routes are built by hand rather than with
   `axum` macros; this is a deliberate house style, not an oversight.

5. **Warnings are errors.** The workspace sets `warnings = "deny"` and clippy `all = "deny"`
   (deliberately not `pedantic`, which would push transcribed code away from the game's shape).
   `unsafe_code` is warned, not forbidden, because one crate needs a single `unsafe` block for the
   CPU clock (`game/clock.rs`); the other two crates forbid unsafe at their roots. A dead-code or
   unused-import warning breaks the build, which is a useful forcing function when refactoring.
   Finish with `cargo fmt --all` and a clean `just check`.

6. **Do not panic on the packet path.** The server catches panics with `std::panic::catch_unwind`
   (`net/listener.rs`, `game/server.rs`) to save the world and exit non-zero so `Restart=on-failure`
   fires; `panic = "unwind"` in the release profile must stay for that to work. Production error
   handling should degrade gracefully (for example, an autosave that cannot write warns and retries;
   it does not take the process down). The ongoing effort to remove non-test `.unwrap()`/`.expect()`/
   `panic!` from production paths is tracked in `TODO.md`; `#[cfg(test)]` panics are out of scope.

7. **Generated data tables are codegen output. Never hand-edit them.** These files are produced by
   `just regen` from a decompiled tree and are checked in only so an ordinary build needs nothing but
   Rust: `recipes.rs`, `npc_drops.rs`, `projectile_data.rs`, `banners.rs`, `buffs.rs`, `angler.rs`,
   `shimmer.rs`, `hurt_tiles.rs`, `town_names.rs`, `travel_shop.rs`, `tile_death.rs` (all in
   `terrustia-proto/src`).
   To change one, change the generator and rerun `just regen`, then review the diff before committing.
   Their size is fine; they are excluded from the file-splitting refactor in `TODO.md`.

## File editing and commits

These apply to every agent, tool-agnostically.

- **Edit files with a real editor, not the shell.** No `sed -i`, no heredoc redirects, no `>`/`>>`
  into source, no inline `python -c`/scripts that rewrite files. A generator script that produces
  checked-in source is fine to run, but write the script itself with an editor.
- **No em dashes.** Not in prose, docs, comments, or commit messages. Use hyphens, colons,
  parentheses, or separate sentences. (Legacy em dashes still exist throughout the repo; removing
  them is a separate low-priority sweep noted in `TODO.md`, but do not add new ones.)
- **Never add `Co-Authored-By`, or any co-author trailer, to a commit.** Ever.
- **Commit or push only when asked.** If you are on the default branch, branch first. Keep commit
  messages factual and scoped.

## Verifying against the real game

Correctness here is proven against Terraria itself, not only against our own code. The
`terrustia-client` examples are the tools: `probe` dumps and compares the packet sequence against a
real `TerrariaServer`; `conform`/`roundtrip_wld` check decoding and `.wld` round-trips at the byte
level; `verify` joins, spawns things, and confirms enemies move, shoot, hurt, and drop loot;
`stress`/`crowd`/`load` hold a full world while the server reports per-phase tick costs; `bot` joins
and reports for comparison against both servers; `bestiary` walks all 691 NPC types over the wire;
`fuzz` throws malformed packets at a running server. The web panel is verified in a real browser with
Playwright. When you fix a bug, add a test that fails against the bug first, then passes.

## Where to look

- `README.md` - the feature-by-feature status, the audit findings worth telling, and the layout.
- `CONTRIBUTING.md` - the bar a change is held to and the contributor license terms.
- `AUDIT.md` / `SECURITY.md` - the audit trail and the security posture.
- `TODO.md` - the single backlog and the v0.0.1 roadmap: lanes, gates, deferred work, and the
  dependency-pruning record. The former `plan.md` and `GAPS.md` are folded into it; their full
  text lives in git history.
- `docs/release-blockers.md` - what is *not* done for v0.0.1: the release bar clause by clause with
  the evidence for each, the known bugs left in on purpose, and the backlog entries that were found
  claiming things the code contradicted. Read it before answering "is this ready", because that
  question has been answered too generously from memory before.
- `docs/` - protocol notes, world-file notes, performance method, and the generated-table provenance.
