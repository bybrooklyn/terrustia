# Contributing

Contributions are welcome: bug reports, fixes, worldgen and AI parity work, docs, packaging, tests.
Open an issue if you want to discuss something first, or send a pull request directly for a
self-contained change.

## Working in the codebase

### Prerequisites

A recent stable Rust toolchain is all you need to build and test the server. The workspace is on the
2024 edition with a minimum of Rust 1.97 (`rust-version` in `Cargo.toml`). [`just`](https://github.com/casey/just)
runs the project's recipes, but every recipe is a thin wrapper over `cargo`, so plain `cargo` works
just as well. You only need [`bun`](https://bun.sh) if you are touching the web panel's frontend.

### Layout

Three crates, described in the README's [Layout](README.md#layout) section: `terrustia-proto` (the
wire format, no I/O), `terrustia-client` (a headless client that speaks the real protocol), and
`terrustia` (the async server). A fourth, `terrustia-codegen`, is a hand-run developer tool that
regenerates the data tables and is excluded from the default build.

### Everyday commands

Each recipe and the raw `cargo` it wraps:

| Recipe | Runs | For |
|---|---|---|
| `just run -- --world W.wld` | `cargo run --release -p terrustia -- ...` | Run the server |
| `just build` | build the web panel, then `cargo build --release` | A release binary with the panel embedded |
| `just check` | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo clippy -p terrustia --all-targets --no-default-features`, `cargo deny check`, the web checks, `cargo test --workspace` | What CI runs |
| `just check-rust` | the same, minus the web build and the tests | The lint pass on its own |
| `just test` | `cargo test --workspace` | The whole test suite |
| `just test-filter NAME` | `cargo test --workspace NAME` | One test or module |
| `just fmt` | `cargo fmt --all` | Format everything |

Clippy is denied workspace-wide, so a warning fails the build. Run `just check` before opening a
pull request and it will catch what CI would.

### The web panel

The admin panel is a Svelte and Vite frontend, built with `bun` (`just web-build`) and embedded into
the binary behind the `embed-web` cargo feature. Its build output (`dist/`) is gitignored, so rebuild
it before an embedded or release build. `just dev` builds the panel and runs a debug server with it
embedded.

### Data tables

`terrustia-proto` ships tables extracted from the game (which tiles hurt, what each NPC drops, and so
on). They are generated from a local decompiled tree that never ships, by `terrustia-codegen`
(`just regen`). The tree is `ilspycmd` output of `TerrariaServer.exe`; see `docs/generated-tables.md`.
The Python `check_*` scripts (`just check-drops`, `just check-recipes` and the rest of
`just check-data`) validate the tables **locally, before a release candidate**. They cannot run in
CI: they need the decompiled tree, which must never reach a hosted runner. `tools/packet_audit.py`
is the one Python check CI does run, because it reads only this repository.

### Checking against the real game

Because this is a reimplementation, the strongest checks compare it to the real game rather than to
itself. The examples do that, and they are split across both crates, so each needs its own `-p`:
`probe` (in `terrustia`) dumps and compares the packet sequence, `verify` (in `terrustia-client`)
joins and confirms things move, shoot, hurt and drop loot, `stress` (`terrustia`) and `soak`
(`terrustia-client`) hold the world full under load, `bot` walks a client east to compare against
vanilla, and `fuzz` (`terrustia`) throws malformed packets at a running server. There is no
`just conform` or `just roundtrip` recipe; they are examples too:

```sh
cargo run --release -p terrustia-client --example conform -- 127.0.0.1:7930 real.trcap
cargo run --release -p terrustia --example roundtrip_wld -- input.wld /tmp/out.wld
```

For a protocol or gameplay change, run the relevant one against real Terraria.

### Conventions

- Transcribe vanilla where you transcribe vanilla, and cite the decompiled source in a comment
  (`Wiring.cs:2042`, and so on) so the next reader can check it.
- Add a test that fails on the unfixed code and passes on the fix, wherever a test can express it.
- Keep clippy silent. Warnings are denied, so there is no such thing as a warning that is fine to
  leave.
- Disclose partial work: say what an implementation does not do, in the code and in the README row,
  rather than implying completeness.
- Never commit decompiled game source, game assets, game text, or `.wld` save files.
- Plain prose in docs. No em-dashes.

### Where to look

`docs/` holds the protocol notes and subsystem write-ups. `AUDIT.md` records what the audits found
and fixed. `TODO.md` is the roadmap and the single backlog of known, deferred work.

## The bar for a change

Every change is held to the same standard the rest of the project follows:

1. It matches vanilla where it transcribes vanilla. This is a reimplementation of the 1.4.5.8
   dedicated server, so behaviour is checked against the decompiled game, not against a guess.
2. It has a test that fails on the unfixed code and passes on the fixed code, where a test can
   express it.
3. It builds clean: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets` with no
   warnings, and `cargo test --workspace` green.
4. It is honest about its own limits. A partial implementation says what it does not do, in the code
   and in the README row, rather than implying completeness.

`docs/` and `TODO.md` describe how the project is put together and how it verifies itself against
the real game. Reading the relevant part before a large change saves a round trip.

## AI-assisted contributions

AI-assisted contributions are welcome. Much of this project was written that way. The condition is
that a human is accountable for every line:

- You have read and understood the change you are submitting. You can explain why it is correct and
  answer questions about it in review.
- It meets the bar above, including a real test and a clean build. AI output that was not run,
  tested, or checked against the decompiled source is not acceptable, whether a person or a model
  produced it.
- You do not paste decompiled game source, game assets, or game text into the repository. The data
  tables are generated from a local decompiled tree that never ships; see `docs/generated-tables.md`.

A change that clears the bar is judged on the change, not on how it was written. A change that does
not clear it is declined, for the same reason.

## Contributor license terms

By submitting a contribution (a pull request, a patch, or any other work) to this project, you agree
to the following. Read them, because they are broader than an ordinary inbound license.

1. **You have the right to submit it.** The contribution is your own work, or you have the right to
   submit it under these terms, and submitting it does not knowingly violate anyone else's rights.

2. **You keep your copyright.** You are not assigning ownership of your contribution to anyone.

3. **You grant a broad, relicensable license.** You grant the project maintainer (the owner of the
   `github.com/bybrooklyn/terrustia` repository) a perpetual, irrevocable, worldwide, royalty-free,
   sublicensable, and transferable license to use, reproduce, modify, prepare derivative works of,
   publicly display, distribute, and **relicense** your contribution, in whole or in part, under any
   license terms, including licenses different from the project's current one and including
   proprietary terms.

This grant is what lets the project change its license later (for example, to dual-license or to
move to a different open-source license) without tracking down every past contributor. Your
contribution stays available to everyone under the project's public license at the time it was
made; the grant is in addition to that, not instead of it.

If you cannot agree to these terms for a particular contribution, say so in the pull request rather
than submitting it, and we can work out another way to get the change in.
