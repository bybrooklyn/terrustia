#!/usr/bin/env python3
"""Mutation-test the data checkers: corrupt the generated tables and prove the checkers notice.

    python3 tools/mutate_tables.py <decompiled-tree> [--limit N] [--seed S] [--rust] [--rust-limit N]

Why this exists. `tools/check_drops.py` sliced a rule's arguments with `[^)]*`, which stops at the
first `)`, so every `ItemDropRule.ByCondition(new Conditions.X(), 4611, 25)` in the game's own
source ended its first argument at `X()` and the item id was never seen at all. The checker still
printed a confident summary. Deleting one of those drops from the committed table would not have
made it fail, because it could not see either side of that comparison. A checker nobody has ever
seen fail is a checker nobody has any reason to trust.

So: take the committed tables, corrupt one entry at a time, run the checker, and require it to
fail. A mutant that *survives* is a hole in the checker, and the surviving mutants are the report.

Two runs, because the two kinds of checker cost very different amounts:

  * The Python checkers (`check_drops.py`, `check_recipes.py`) run in hundredths of a second, so
    they get hundreds of mutants. Nothing touches the working tree: the checker and the tables are
    copied into a temporary directory first, and `check_drops.py` finds its repo root relative to
    its own file, so a copy of it reads the copied tables.

  * The Rust suite (`crates/terrustia-proto/tests/generated_tables.rs`) needs a rebuild per mutant,
    so `--rust` opts into a much smaller run. That one has to mutate the real file in place, since
    `cargo` compiles the crate where it lives; the original bytes are restored in a `finally` and
    re-verified before exit. It also checks, before it trusts anything, that the compiler is really
    reading the file being mutated: see `build_sees_this_file`.

Exit code is 1 if a target's survival rate is worse than the blind spot written down for it in
`BUDGET`, or if the baseline or the build-provenance check fails.
"""

import argparse
import random
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PROTO = REPO / "crates" / "terrustia-proto" / "src"

# A mutation is a single-line text substitution: `(pattern, replacement)` applied to whichever
# capture group holds the value. Changing a value rather than deleting a row is deliberate: every
# one of these tables is a fixed-size array or an exhaustive `match`, so a deletion would fail to
# *compile*, and "the compiler caught it" says nothing about whether the checker can see the row.
# A wrong value compiles, which is exactly the shape every finding in the audit had: a table that
# was there, ran, and held the wrong number.
#
# 65000 is a real `u16` and not a real item id, so a mutated row is unambiguously wrong rather than
# accidentally still correct.
BOGUS = 65000

SITES = {
    # `Drop { item: N, one_in: .., min: .., max: .. }`: the unconditional half of the drop table.
    "npc_drops.rs": re.compile(r"^(\s*item: )(\d+)(,)$"),
    # `always(N)`, `sometimes(N, 10)`, `a_few(N, 1, 2, 3)`, `m_in_n(N, ..)`: the hand-written
    # conditional half. The item is always the first argument. The second pattern is the bare
    # `npc => item,` arm that `trophy` and `lunar_fragment` are written as; without it the trophy,
    # mask and fragment maps were never mutated at all, and Spazmatism's Soul of Sight looked like
    # a `check_drops.py` blind spot when it is simply carried by two tables at once.
    "conditional_drops.rs": (
        re.compile(r"^(.*(?:always|sometimes|a_few|m_in_n)\()(\d+)([,)].*)$"),
        re.compile(r"^(        \d+ => )(\d+)(,)$"),
    ),
    # `Recipe { result: N, makes: .., first: .., count: .. }`.
    "recipes.rs": re.compile(r"^(\s*result: )(\d+)(,)$"),
    # `npc_type => banner_index,`.
    "banners.rs": re.compile(r"^(\s*\d+ => )(\d+)(,)$"),
    # `Offer { item: N, .. }`.
    "travel_shop.rs": re.compile(r"^(\s*item: )(\d+)(,)$"),
    # `ProjectileData { .. damage: N, .. }` and friends: the width is a stable per-row field that
    # every projectile has and no two rows share by accident.
    "projectile_data.rs": re.compile(r"^(\s*width: )(\d+)(,)$"),
    # `NpcStats { .. life_max: N, .. }`.
    "npc_data.rs": re.compile(r"^(\s*life_max: )(\d+)(,)$"),
    # `(style, item)` pairs, which come in two shapes: a whole arm on one line for a block with a
    # single style, and one pair per line for everything else. Only the item moves.
    "placed_items.rs": (
        re.compile(r"^(\s*\(\d+, )(\d+)(\),)$"),
        re.compile(r"^(\s*\d+ => &\[\(\d+, )(\d+)(\)\],)$"),
    ),
    # `TileObject { .. full_height: N, .. }`: the field the hand-written table got wrong on 348 of
    # its 389 entries, and one every row has.
    "tile_object.rs": re.compile(r"^(\s*full_height: )(\d+)(,)$"),
}

# Which checker is supposed to catch a corruption of each table, and what it costs to run.
PYTHON_TARGETS = [
    ("npc_drops.rs", "check_drops.py"),
    ("conditional_drops.rs", "check_drops.py"),
    ("recipes.rs", "check_recipes.py"),
    ("npc_data.rs", "check_npc_data.py"),
    ("placed_items.rs", "check_placed_items.py"),
    ("tile_object.rs", "check_tile_object.py"),
]
# The checkers the sandbox needs a copy of, derived rather than listed, so adding a target above is
# the only edit a new checker needs.
PYTHON_CHECKERS = sorted({checker for _, checker in PYTHON_TARGETS})
# A checker's known, written-down blind spot, as the share of mutants it is allowed to miss.
#
# Zero unless there is a reason here. This is not a place to park a survival rate that has gone up:
# an entry says "this checker provably cannot see this class of row, and here is the class". Every
# surviving mutant is still printed either way.
BUDGET: dict[tuple[str, str], tuple[float, str]] = {
    ("recipes.rs", "check_recipes.py"): (
        0.25,
        "check_recipes.py reads recipes written as `currentRecipe.createItem.SetDefaults(N); ...; "
        "AddRecipe();` in Recipe.cs. Roughly 545 of the 3105 committed rows are not written that "
        "way: `AddStandardFurnitureSetRecipes` (22 call sites x 21 recipes) and "
        "`AddCritterStatueRecipe` (22) are parameterised helpers whose arguments come from the "
        "call site, and `CreateReverseWallRecipes`/`CreateReversePlatformRecipes` build theirs "
        "from arrays. Mutating one of those rows is invisible, so about a sixth of random mutants "
        "survive for that reason alone. Closing it means teaching the checker to substitute a "
        "helper's arguments into its body - worth doing, not done here.",
    ),
    ("placed_items.rs", "check_placed_items.py"): (
        0.12,
        "check_placed_items.py reads three sources - the `Item.SetDefaults` inversion, the two "
        "dozen `GetItemDrop_*` methods, and `WorldGen.cs`'s own tile-91 banner chain - and holds "
        "every pair any of them defines. 237 of the 2962 mutation sites (8.0%) are pairs none of "
        "them define and nothing therefore checks: paintings (tiles 240/242/245/246, 101 of them) "
        "have their drops written into their own worldgen arms, and the rest is a long tail of "
        "statues, campfires and one-off objects. Measured, not assumed: every survivor of a run "
        "has been confirmed to be in that class. It was 58.9% when only the inversion was read.",
    ),
    ("conditional_drops.rs", "check_drops.py"): (
        0.10,
        "check_drops.py compares game-minus-ours and reports ours-minus-game separately without "
        "gating on it (see that file's own reverse-direction section). A row that gives an NPC "
        "something `ItemDropDatabase` never registers for it therefore cannot be caught by "
        "corrupting it: there is nothing on the game side to stop matching. 14 items across 8 "
        "NPCs are currently in that state and are listed by every run of the checker.",
    ),
}

RUST_TARGETS = [
    "npc_drops.rs",
    "conditional_drops.rs",
    "banners.rs",
    "travel_shop.rs",
    "projectile_data.rs",
    "npc_data.rs",
]


class Site:
    """One mutable value in a table: which file, where in it, what it says, what it becomes."""

    def __init__(self, path: Path, index: int, line: str, mutated: str, value: int):
        self.path = path
        self.index = index
        self.line = line
        self.mutated = mutated
        self.value = value


def mutation_sites(path: Path) -> list[Site]:
    """Every line this file offers as a mutant."""
    patterns = SITES[path.name]
    if not isinstance(patterns, tuple):
        patterns = (patterns,)
    out: list[Site] = []
    for i, line in enumerate(path.read_text().splitlines()):
        # A doc comment quoting `a_few(1129, 3, 1, 1)` is prose and a `#[cfg(test)]` assertion
        # spelling out an expected chain is a test, not a row. Mutating either produces a mutant
        # nothing could ever be expected to catch, and the first run of this script reported four
        # of them as blind spots in `check_drops.py`, which they were not.
        stripped = line.lstrip()
        if stripped.startswith("//"):
            continue
        if stripped.startswith("#[cfg(test)]"):
            break
        m = next(filter(None, (p.match(line) for p in patterns)), None)
        if not m or int(m.group(2)) == BOGUS:
            continue
        out.append(Site(path, i, line, f"{m.group(1)}{BOGUS}{m.group(3)}", int(m.group(2))))
    return out


def apply_mutations(originals: dict[Path, str], sites: list[Site]) -> None:
    """Restore every file to its original, then apply these mutations. Passing no sites restores."""
    by_path: dict[Path, list[Site]] = {}
    for site in sites:
        by_path.setdefault(site.path, []).append(site)
    for path, original in originals.items():
        lines = original.splitlines(keepends=True)
        for site in by_path.get(path, []):
            ending = "\n" if lines[site.index].endswith("\n") else ""
            lines[site.index] = site.mutated + ending
        path.write_text("".join(lines))


def run_mutants(originals, all_sites, chosen, fails):
    """Apply each chosen mutant, ask `fails()` whether the checker rejected it, and sort the
    survivors into blind spots and equivalent mutants.

    An equivalent mutant is one where the same value is carried by another row, so corrupting a
    single row leaves the table still saying the same thing. `conditional_drops.rs` is full of
    them by design: the Ogre's Defender Medal is registered once per difficulty branch, so no
    single-row change can take it away. Those are not checker faults, and calling them faults
    would drown the ones that are. They are also not proof of anything, so they are proven rather
    than assumed: mutate *every* copy of the value and require the checker to catch that.

    `all_sites` spans every file the checker reads, not just the one being mutated. Item 1369 is
    Spazmatism's Soul of Sight, carried by both `npc_drops.rs` and `conditional_drops.rs`'
    lunar/trophy map, and `check_drops.py` unions the two before comparing: a same-file-only
    escalation called that a blind spot when it is an ordinary duplicate.
    """
    caught, equivalent, survivors = 0, [], []
    try:
        for site in chosen:
            apply_mutations(originals, [site])
            if fails():
                caught += 1
                continue
            twins = [s for s in all_sites if s.value == site.value]
            if len(twins) > 1:
                apply_mutations(originals, twins)
                if fails():
                    equivalent.append((site, len(twins)))
                    continue
            survivors.append(site)
    finally:
        apply_mutations(originals, [])
    return caught, equivalent, survivors


def run_python_checker(sandbox: Path, checker: str, decompiled: Path) -> bool:
    """True when the checker *failed*, which for a mutant is the outcome we want."""
    argv = [sys.executable, str(sandbox / "tools" / checker), str(decompiled)]
    if checker == "check_recipes.py":
        argv.append(str(sandbox / "crates" / "terrustia-proto" / "src" / "recipes.rs"))
    done = subprocess.run(argv, capture_output=True, text=True, check=False)
    return done.returncode != 0


def python_phase(decompiled: Path, limit: int, seed: int) -> int:
    """Mutate the tables the Python checkers read, in a throwaway copy of the tree."""
    survivors = 0
    with tempfile.TemporaryDirectory(prefix="terrustia-mutants-") as tmp:
        sandbox = Path(tmp)
        (sandbox / "tools").mkdir()
        (sandbox / "crates" / "terrustia-proto" / "src").mkdir(parents=True)
        for checker in PYTHON_CHECKERS:
            shutil.copy2(REPO / "tools" / checker, sandbox / "tools" / checker)
        for table in {t for t, _ in PYTHON_TARGETS}:
            shutil.copy2(PROTO / table, sandbox / "crates" / "terrustia-proto" / "src" / table)

        # A mutation run against a checker that is already failing proves nothing: every mutant
        # would be "caught" by the pre-existing failure. Establish the green baseline first.
        for checker in PYTHON_CHECKERS:
            if run_python_checker(sandbox, checker, decompiled):
                print(f"BASELINE FAILS: {checker} does not pass on the committed tables.")
                print("Fix that first; mutation testing on a red baseline measures nothing.")
                return -1

        for table, checker in PYTHON_TARGETS:
            # Every file this checker reads, so escalating a survivor can prove a value is carried
            # by a *sibling* table and not only by another row of the same one.
            siblings = [t for t, c in PYTHON_TARGETS if c == checker]
            originals = {
                sandbox / "crates" / "terrustia-proto" / "src" / t: (
                    sandbox / "crates" / "terrustia-proto" / "src" / t
                ).read_text()
                for t in siblings
            }
            all_sites = [s for p in originals for s in mutation_sites(p)]
            path = sandbox / "crates" / "terrustia-proto" / "src" / table
            sites = [s for s in all_sites if s.path == path]
            chosen = random.Random(seed).sample(sites, min(limit, len(sites)))
            caught, equivalent, missed = run_mutants(
                originals,
                all_sites,
                chosen,
                lambda: run_python_checker(sandbox, checker, decompiled),
            )
            survivors += report(
                table, checker, len(sites), len(chosen), caught, equivalent, missed
            )
    return survivors


def rust_phase(limit: int, seed: int) -> int:
    """Mutate in place and run the proto test suite. Restores the file whatever happens.

    Always returns 0: this phase measures rather than gates. `tests/generated_tables.rs` is twelve
    named spot checks ("every slime drops Gel", "Bone drops from the Angry Bones family") and was
    never a table verifier, so a random row corrupted anywhere else in a 3000-row table is expected
    to sail past it. The number is the point. `banners.rs`, `travel_shop.rs` and
    `projectile_data.rs` have no Python cross-checker at all, so this suite is the *only* thing
    standing between them and a silently wrong table, and this is what that is worth.
    """
    # Once, not per table: the failure this guards against is a property of the build, not of one
    # file, and it costs a full test run to ask.
    canary = PROTO / RUST_TARGETS[0]
    if not build_sees_this_file(canary, canary.read_text()):
        print(f"ABORTED: `cargo test` passed on a {RUST_TARGETS[0]} that cannot compile.")
        print("  The build is not reading the source being mutated: a stale or substituted")
        print("  artefact is under test, and every result from this phase would be a lie.")
        print("  Check CARGO_TARGET_DIR. A target directory shared between git worktrees does")
        print("  exactly this: cargo's unit hash for a workspace crate does not include the")
        print("  worktree path, so two trees write the same artefact filename and the last")
        print("  build wins.")
        return 1

    # The suite reads every one of these, so a value carried by a sibling table escalates the same
    # way it does on the Python side.
    originals = {PROTO / t: (PROTO / t).read_text() for t in RUST_TARGETS}
    all_sites = [s for p in originals for s in mutation_sites(p)]

    survivors = 0
    for table in RUST_TARGETS:
        path = PROTO / table
        sites = [s for s in all_sites if s.path == path]
        if not sites:
            print(f"{table}: no mutation sites matched; the table's shape changed under the regex")
            survivors += 1
            continue
        chosen = random.Random(seed).sample(sites, min(limit, len(sites)))
        try:
            caught, equivalent, missed = run_mutants(
                originals, all_sites, chosen, proto_tests_fail
            )
        finally:
            apply_mutations(originals, [])
            for p, original in originals.items():
                assert p.read_text() == original, f"failed to restore {p}"
        report(
            table, "tests/generated_tables.rs", len(sites), len(chosen), caught, equivalent, missed
        )
        survivors += len(missed)
    print(
        f"{survivors} of the Rust-phase mutants went unnoticed. Reported, not gated: this suite is\n"
        "twelve named facts, not a check of the whole table, and four of these tables have no\n"
        "second implementation to compare against at all. That is the finding, not a failure."
    )
    print()
    return 0


def build_sees_this_file(path: Path, original: str) -> bool:
    """Prove the compiler is reading *this* file before trusting a word the Rust phase says.

    A mutation run measures a checker only if the thing being checked is the thing being built. Put
    a line that cannot compile at the top of the file: if `cargo test` still passes, the artefact
    under test came from somewhere else and every "mutant survived" below would be an artefact of
    the build, not a blind spot in the suite.

    This is not hypothetical. A target directory shared between two git worktrees produces exactly
    this: cargo's unit hash for a workspace-local crate does not include the worktree path, so both
    trees write `libterrustia_proto-<same hash>.rlib` and whichever built last is what the other
    tree's crates link against. `cargo test -p terrustia-proto` passes while `cargo test
    --workspace` fails, which is the tell.
    """
    try:
        path.write_text("compile_error!(\"mutate_tables provenance canary\");\n" + original)
        return proto_tests_fail()
    finally:
        path.write_text(original)


def proto_tests_fail() -> bool:
    done = subprocess.run(
        ["cargo", "test", "-q", "-p", "terrustia-proto", "--test", "generated_tables"],
        cwd=REPO,
        capture_output=True,
        text=True,
        check=False,
    )
    return done.returncode != 0


def report(
    table: str,
    checker: str,
    total_sites: int,
    run: int,
    caught: int,
    equivalent: list[tuple[Site, int]],
    missed: list[Site],
) -> int:
    """Print one target's result, and return how many survivors count against the exit code."""
    rate = 100.0 * (caught + len(equivalent)) / run if run else 0.0
    print(f"{table} vs {checker}")
    print(
        f"  {run} mutants of {total_sites} possible; {caught} caught, "
        f"{len(equivalent)} equivalent, {len(missed)} survived ({rate:.1f}% killed)"
    )
    for site, copies in equivalent:
        print(
            f"    equivalent  {table}:{site.index + 1}  {site.line.strip()} "
            f"(value {site.value} appears in {copies} rows; caught once all of them changed)"
        )
    for site in missed:
        print(f"    SURVIVED  {table}:{site.index + 1}  {site.line.strip()}")
    allowed, reason = BUDGET.get((table, checker), (0.0, ""))
    over = run and len(missed) / run > allowed
    if allowed:
        print(f"  budget: up to {allowed:.0%} may survive. {reason}")
    print()
    return len(missed) if over else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("decompiled", type=Path, help="the decompiled Terraria tree")
    # 100 rather than a token handful: the survival rate is compared against a budget, and a
    # 40-mutant sample of a 215-row table is too noisy for a 10% threshold to mean anything. The
    # whole run is still well under a minute.
    ap.add_argument("--limit", type=int, default=100, help="mutants per Python target")
    ap.add_argument("--rust-limit", type=int, default=4, help="mutants per Rust target")
    ap.add_argument("--seed", type=int, default=20260830)
    ap.add_argument(
        "--rust",
        action="store_true",
        help="also mutate against the Rust suite (one full rebuild per mutant, minutes not seconds)",
    )
    args = ap.parse_args()

    survivors = python_phase(args.decompiled, args.limit, args.seed)
    if survivors < 0:
        return 2
    if args.rust:
        survivors += rust_phase(args.rust_limit, args.seed)

    if survivors:
        print(f"{survivors} mutant(s) survived beyond what BUDGET allows.")
        print("Each surviving mutant is a corruption the checker cannot see. Either close the")
        print("blind spot, or write it down in BUDGET with the class of row it covers. Do not")
        print("widen a budget to make a number go away.")
        return 1
    print("no target exceeded its budget; every survivor above is a recorded blind spot.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
