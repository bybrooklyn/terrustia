# terrustia — Justfile
# https://github.com/casey/just
#
# Install: cargo install just  |  brew install just  |  pacman -S just

set shell := ["bash", "-euo", "pipefail", "-c"]

WEB := "crates/terrustia/web-panel"

# ─────────────────────────────────────────
# Default — list all recipes
# ─────────────────────────────────────────

[private]
default:
    @just --list

# ─────────────────────────────────────────
# DEVELOPMENT
# ─────────────────────────────────────────

# Install the dependencies needed for local development
install:
    @command -v cargo >/dev/null || { echo "error: cargo (Rust) is required"; exit 1; }
    @echo "── Rust dependencies ──"
    cargo fetch --locked
    @if command -v bun >/dev/null; then \
        echo "── Web panel dependencies ──"; \
        cd {{WEB}} && bun install --frozen-lockfile; \
    else \
        echo "warning: bun is not installed; the web panel (embed-web) will not build."; \
        echo "  install from https://bun.sh — the server itself runs fine without it."; \
    fi
    @echo "Ready. Next: just run   (or  just dev  to build the panel and embed it first)"

# Run a release server. Extra args pass through, e.g. `just run --new "My World"`
run *ARGS:
    cargo run --release -p terrustia -- {{ARGS}}

# Build the web panel and run a debug server with it embedded — the local iteration loop
dev: web-build
    cargo run -p terrustia --features embed-web

# Web panel dev server with hot reload (serve the panel from disk, not embedded)
web:
    cd {{WEB}} && bun install --frozen-lockfile && bun run dev

# Dev loop against your real Terraria save, read-only (saves go to a .terrustia.wld copy)
dev-live: web-build
    cargo run -p terrustia --features embed-web -- \
        --world "$HOME/Library/Application Support/Terraria/Worlds/The_Successful_Excrement.wld" \
        --save "$HOME/Library/Application Support/Terraria/Worlds/The_Successful_Excrement.terrustia.wld"

# ─────────────────────────────────────────
# BUILD
# ─────────────────────────────────────────

# Full release build — web panel first, then the whole Rust workspace
build: web-build
    cargo build --release --workspace
    @echo "Done → target/release/terrustia"

# Build the web panel assets only (→ crates/terrustia/web-panel/dist)
#
# The install is checked, not trusted. `bun install --frozen-lockfile` compares the lockfile against
# the tree and reports "no changes" whenever they agree, which it does even when the packages
# themselves are half there: bun's global cache has twice been found holding entries with their
# whole `dist/` directory missing, and `node_modules` is copied from it, so the breakage is
# faithfully reproduced and then declared fine in 50ms. What you get is an ERR_MODULE_NOT_FOUND
# stack trace out of node with no hint that the install is the problem.
#
# So: after installing, check that vite's entry point is actually there, and if it is not, clear the
# cache and install again for real. One `test -f` on the happy path.
web-build:
    cd {{WEB}} && bun install --frozen-lockfile
    @if [ ! -f "{{WEB}}/node_modules/vite/dist/node/cli.js" ]; then \
        echo "web panel: the install is incomplete (vite has no dist/). Repairing."; \
        cd {{WEB}} && bun pm cache rm && rm -rf node_modules && bun install; \
    fi
    cd {{WEB}} && bun run build

# Build the Rust workspace only (assumes web-panel/dist already exists)
rust-build:
    cargo build --release --workspace

# ─────────────────────────────────────────
# CHECKS — mirrors CI
# ─────────────────────────────────────────

# Everything CI runs: Rust format, clippy (both feature sets), supply-chain, the web build, tests
check: check-rust check-web test
    @echo "All checks passed ✓"

# Rust-only checks (faster). No tests: `just check` adds those, this is the lint pass on its own.
check-rust:
    @echo "── Rust format ──"
    cargo fmt --all --check
    @echo "── Rust clippy (0 warnings) ──"
    cargo clippy --workspace --all-targets -- -D warnings
    @# `embed-web` is default-on, so the pass above never compiles the panel's disk-serving branch
    @# (`panel/mod.rs::load_static_asset`'s `#[cfg(not(feature = "embed-web"))]` arm) or the `..`
    @# traversal guard inside it. CI lints it separately (ci.yml) and so does this.
    @echo "── Rust clippy, embed-web off (the disk-serving panel branch) ──"
    cargo clippy -p terrustia --all-targets --no-default-features -- -D warnings
    @echo "── Supply chain (cargo-deny) ──"
    cargo deny check
    @# CI has run this as its own job for a long time; `just check` did not, so a local run could
    @# come back "All checks passed" while the packet-id table and the code disagreed and CI was
    @# red. That is exactly what happened between 9f056a2 and 2026-09-06. It needs no decompiled
    @# tree and takes under a second, so there is no reason for it to live only in CI.
    @echo "── Packet id classification ──"
    python3 tools/packet_audit.py

# Web panel typecheck + build. Goes through `web-build` so it gets the same incomplete-install
# check; this is the recipe CI runs, and a half-installed cache there fails as a stack trace out of
# node with nothing pointing at the install.
check-web: web-build

# Format all Rust code
fmt:
    cargo fmt --all

# ─────────────────────────────────────────
# TESTS
# ─────────────────────────────────────────

# Run the whole workspace test suite
test:
    cargo test --workspace

# Run tests matching a filter, with output shown (e.g. `just test-filter fighter`)
test-filter FILTER:
    cargo test --workspace {{FILTER}} -- --nocapture

# The CI soak: a minute (default) of a real server with three real clients
soak SECONDS="60":
    ./tools/soak_ci.sh {{SECONDS}}

# Needs the real TerrariaServer and a QUIET machine: the script refuses to call its own output
# publishable when the load average says the box was contended.
# The README's comparison table, measured: startup, idle CPU, RAM and bandwidth vs the real server
compare SECONDS="300":
    COMPARE_WINDOW={{SECONDS}} ./tools/compare_vanilla.sh

# Play the game toward a player's goals and report the ones that could not be reached.
# Owns the server's lifecycle, because two of the goals are about surviving a save and reload.
playbot:
    cargo build --release -p terrustia -p terrustia-client --bins --examples
    ./tools/playbot.sh

# Fuzz a decoder target for a while (needs nightly + `cargo install cargo-fuzz`)
fuzz TARGET="packet_decoders" SECONDS="60":
    cargo +nightly fuzz run {{TARGET}} -- -max_total_time={{SECONDS}}

# ─────────────────────────────────────────
# VERIFY AGAINST REAL TERRARIA
# ─────────────────────────────────────────
# These point at a live server — ours, or a real `TerrariaServer` — to prove the
# protocol and world format against something this project did not write.

# Round-trip a .wld through our loader and saver and report any difference
roundtrip WLD OUT="/tmp/terrustia-roundtrip.wld":
    cargo run --release -p terrustia --example roundtrip_wld -- {{WLD}} {{OUT}}

# Check our decoding against a server's bytes (ours: 7777, real Terraria: its port)
conform ADDR="127.0.0.1:7777" CAPTURE="/tmp/terrustia-conform.trcap":
    cargo run --release -p terrustia-client --example conform -- {{ADDR}} {{CAPTURE}}

# ─────────────────────────────────────────
# DATA TABLES  (dev-only — need a decompiled Terraria tree, not in the repo)
# ─────────────────────────────────────────
# The big data files (recipes.rs, npc_drops.rs, projectile_data.rs, …) are
# *committed* precisely so an ordinary build needs nothing but Rust. They are only
# regenerated from a decompiled Terraria source tree when the game version changes.
# DECOMPILED defaults to where this repo keeps its (gitignored) decompile.

DECOMPILED := ".scratch/decompiled"

# Cross-check the checked-in drop table against the decompiled game
check-drops:
    python3 tools/check_drops.py {{DECOMPILED}}

# Cross-check `npc_data.rs` against `NPC.SetDefaults`, entry by entry and field by field.
#
# It is the largest table in the project and one of three with no generator, so rule 7's usual
# protection - regenerate and read the diff - does not apply to it. This is that protection by
# another road: 691 entries, 16 fields each, and seven differences on the record with reasons.
check-npc-data:
    python3 tools/check_npc_data.py {{DECOMPILED}}

# Cross-check `placed_items.rs` against `Item.SetDefaults`' own createTile/placeStyle pairs.
#
# The second table with no generator, and the same protection by the same road: every (tile, style)
# the game defines has to be here with the same item. Pairs that are ours alone are fine - they
# come from the `GetItemDrop_*` merge, a different source - so only one direction is checked.
check-placed-items:
    python3 tools/check_placed_items.py {{DECOMPILED}}

# Do the vanilla lines a citation names actually contain the numbers written next to it?
#
# `check-parity` proves a citation still points at the same text; it says so itself that it never
# judges whether the transcription is right. This asks the other half, and it found a class nothing
# had looked at: citations that are a few lines short of the value they document, so the hash
# guards lines that do not contain it. Report-only - a derived value (`num2 * 2`) legitimately
# appears nowhere in its own citation, and no rule can tell that from a mistake.
check-citations:
    python3 tools/check_citations.py {{DECOMPILED}}

# Do the *documents* still cite the code they were written against?
#
# `check-parity` does this for `crates/*/src`. Nothing did it for the markdown, and
# `docs/release-blockers.md` - the file that exists because a status went stale - went stale twice
# in three days, the second time caught only by a person reading it line by line. Its own account of
# the first time says why: "the checkers caught none of these, because none of them check prose."
#
# This checks the mechanically checkable half. It reads the `file.rs:1234` and `NPC.cs:81066`
# references inside the prose and keys each to the lines it points at, so the claim expires when the
# code moves and the failure names the document and line to re-read. It never judges the sentence.
check-doc-citations:
    python3 tools/check_doc_citations.py --self-test
    python3 tools/check_doc_citations.py {{DECOMPILED}}

# Rebuild docs/doc-citations.tsv and review the diff, as with the generated tables.
doc-citations-update:
    python3 tools/check_doc_citations.py {{DECOMPILED}} --update

# Re-read `TileObjectData.Initialize` and hold `tile_object.rs` to it.
#
# The table is generated now, so this is the same second opinion `check-drops` gives `npc_drops.rs`:
# an independent interpreter, written before the generator and agreeing with it on all 389 entries,
# that catches a hand-edit or a regen nobody ran. Its subject is a program rather than a
# declaration, which is exactly why one reading of it is not enough.
check-tile-object:
    python3 tools/check_tile_object.py {{DECOMPILED}}

# Cross-check the checked-in shimmer-decraft recipes against the decompiled game
check-recipes:
    python3 tools/check_recipes.py {{DECOMPILED}} crates/terrustia-proto/src/recipes.rs

# An NPC nobody can meet is silent: nothing errors, no test fails, the type simply never comes up.
# That is how the Harpy and the Wyvern sat in no spawn pool at all while every test passed. This
# reads vanilla's ambient roster out of `NPC.Spawner` and this server's out of the server itself
# (by running the test that prints it), and diffs them against `docs/spawn-gaps.tsv`.
#
# Rebuild the gap list with `spawn-reach-update` and review the diff, as with the other tables.
#
# Report NPCs vanilla's own ambient spawning can produce and this server cannot
check-spawn-reach:
    python3 tools/check_spawn_reach.py --self-test
    python3 tools/check_spawn_reach.py {{DECOMPILED}}

# Rebuild docs/spawn-gaps.tsv from the decompiled tree. Review the diff before committing
spawn-reach-update:
    python3 tools/check_spawn_reach.py {{DECOMPILED}} --update

# `dead_code` cannot see these, because a read inside `#[cfg(test)]` counts as a read: a field the
# tests assert on and production ignores is invisible to it. Needs no decompiled tree; it is here
# because it belongs to the same qualification pass, not because it needs the game.
#
# Report struct fields written in production and read only by the tests
check-dead-writes:
    cargo run -q -p terrustia-codegen --bin deadwrite

# AGENTS.md rule 2 makes every transcription cite its source, so the tree holds thousands of
# `NPC.cs:12345` references - the recipe prints the live count, because every figure written down
# here has gone stale (this line said ~1900 against an actual 3,848).
# `docs/parity-index.tsv` is derived from them and is never hand-edited:
# a hand-maintained parity ledger that rots does not say "unknown", it says "verified". Each entry
# carries a hash of the cited vanilla lines and a hash of our own item's body, so a claim expires on
# its own the moment either side moves, and this says which side that was. It answers "is this still
# the code it was checked against" and "what is cited by nothing", never "is this right".
#
# Rebuild the index with `parity-update` and review the diff, exactly as with the generated tables.
# A regenerated decompiled tree moves every line number at once; that is reported as one sentence
# rather than one drift per citation.
#
# Check every vanilla citation against the decompiled tree, and report what is cited by nothing
check-parity:
    python3 tools/parity_index.py --self-test
    python3 tools/parity_index.py {{DECOMPILED}}

# What fraction of each vanilla source file anything this project wrote has actually cited
parity-coverage:
    python3 tools/parity_index.py {{DECOMPILED}} --coverage

# Rebuild docs/parity-index.tsv from the citations in the source. Review the diff before committing
parity-update:
    python3 tools/parity_index.py {{DECOMPILED}} --update

# A surviving mutant is a blind spot in a checker, which is how a missing drop stays missing. Pass
# `--rust` to also measure the proto test suite, at a rebuild per mutant.
#
# Corrupt the generated tables and prove the checkers above actually fail
check-mutants *ARGS:
    python3 tools/mutate_tables.py {{DECOMPILED}} {{ARGS}}

# Part of release-candidate qualification, run locally against the decompiled tree: CI can never
# hold decompiled game source, so these deliberately never run there.
#
# `check-dead-writes` runs last on purpose: it was for a while the one that failed (21 fields
# written in production and read by nothing, since fixed to 0), and `just` stops a chain at the
# first failure. Putting it at the end means the others still report first if a future regression
# reintroduces one.
#
# Every data cross-check in one go: the tables, the citations, the dead writes, and the checkers
check-data: check-drops check-npc-data check-placed-items check-tile-object check-recipes check-parity check-doc-citations check-spawn-reach check-mutants check-dead-writes

# Regenerate every transcribed data table from a decompiled tree, then format
regen:
    cargo run -q -p terrustia-codegen -- recipes {{DECOMPILED}} crates/terrustia-proto/src/recipes.rs
    cargo run -q -p terrustia-codegen -- drops       {{DECOMPILED}} crates/terrustia-proto/src/npc_drops.rs
    cargo run -q -p terrustia-codegen -- projectiles {{DECOMPILED}} crates/terrustia-proto/src/projectile_data.rs
    cargo run -q -p terrustia-codegen -- net_variants {{DECOMPILED}} crates/terrustia-proto/src/net_variants.rs
    cargo run -q -p terrustia-codegen -- banners     {{DECOMPILED}} crates/terrustia-proto/src/banners.rs
    cargo run -q -p terrustia-codegen -- golf        {{DECOMPILED}} crates/terrustia-proto/src/golf_physics.rs
    cargo run -q -p terrustia-codegen -- buffs       {{DECOMPILED}} crates/terrustia-proto/src/buffs.rs
    cargo run -q -p terrustia-codegen -- angler      {{DECOMPILED}} crates/terrustia-proto/src/angler.rs
    cargo run -q -p terrustia-codegen -- shimmer     {{DECOMPILED}} crates/terrustia-proto/src/shimmer.rs
    cargo run -q -p terrustia-codegen -- hurt_tiles {{DECOMPILED}} crates/terrustia-proto/src/hurt_tiles.rs
    cargo run -q -p terrustia-codegen -- town_names {{DECOMPILED}} crates/terrustia-proto/src/town_names.rs
    cargo run -q -p terrustia-codegen -- travel_shop {{DECOMPILED}} crates/terrustia-proto/src/travel_shop.rs
    cargo run -q -p terrustia-codegen -- tile_death  {{DECOMPILED}} crates/terrustia-proto/src/tile_death.rs
    cargo run -q -p terrustia-codegen -- tile_object {{DECOMPILED}} crates/terrustia-proto/src/tile_object.rs
    cargo fmt --all
    @echo "Regenerated the data tables. Review the diff before committing."

# ─────────────────────────────────────────
# PACKAGING
# ─────────────────────────────────────────

# Build the Docker image locally (expects a prebuilt musl binary in dist/, see Dockerfile)
docker-build:
    docker build -t terrustia:dev .

# ─────────────────────────────────────────
# PUBLISH  (crates.io)
# ─────────────────────────────────────────
# Only terrustia-proto is published to crates.io: it is the MIT wire-format library, free for any
# Terraria tool to build on. The server crate is AGPL and application-shaped, not a library, so it
# is not published here.

# List exactly what would go into the terrustia-proto package — catches a stray or missing file
publish-proto-list:
    cargo package -p terrustia-proto --locked --list

# Dry run: build and pack terrustia-proto the way crates.io will, without uploading anything
publish-proto-dry:
    cargo publish -p terrustia-proto --locked --dry-run
    @echo "Dry run OK. Publish for real with: just publish-proto"

# Publish terrustia-proto to crates.io (needs a prior `cargo login`). Dry-runs first as a guard.
publish-proto: publish-proto-dry
    cargo publish -p terrustia-proto --locked
    @echo "Published terrustia-proto → https://crates.io/crates/terrustia-proto"

# ─────────────────────────────────────────
# UTILITIES
# ─────────────────────────────────────────

# Remove build and generated artifacts
clean:
    cargo clean
    rm -rf {{WEB}}/dist {{WEB}}/node_modules

# Count lines of source
loc:
    @echo "── Rust ──"
    @find crates -name '*.rs' -not -path '*/target/*' | xargs wc -l | tail -1
    @echo "── Web panel ──"
    @find {{WEB}}/src -type f \( -name '*.ts' -o -name '*.svelte' \) | xargs wc -l | tail -1
