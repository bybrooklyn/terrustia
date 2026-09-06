#!/usr/bin/env python3
"""Do the documents still cite the code they were written against?

`just check-parity` does this for the ~3,850 vanilla citations in `crates/*/src`, and the reason it
exists applies word for word to the markdown: a citation that has quietly stopped pointing at what
it names does not say "unknown", it says "verified". `docs/release-blockers.md` went stale twice in
three days, and the second time it was found by hand. Its own summary of the first time says why no
checker caught it:

    The checkers caught none of these, because none of them check prose.

This checks the half of prose that is mechanically checkable. It does not read English and never
will; it reads the `file.rs:1234` and `NPC.cs:81066` references *inside* the English and keys each
one to the lines it points at, so a claim expires on its own when the code underneath it moves. The
sentence still has to be re-read by a person - but they are told which sentence.

What it caught the day it was written, from `docs/release-blockers.md` alone: `moon_lord.rs:299`
(cited for a countdown; the file contains no such word, and never did), `update.rs:58-60` (an enum
variant's doc comment, not the Windows install arm), `dispatch.rs:3869-3889` (now pylon-travel
code), and two `systems.rs` ranges that had moved by thousands of lines.

    python3 tools/check_doc_citations.py <decompiled>            # check
    python3 tools/check_doc_citations.py <decompiled> --update   # rebuild the index
    python3 tools/check_doc_citations.py --self-test             # no tree needed

`docs/doc-citations.tsv` is the index, and it is never hand-edited: rebuild it and review the diff,
exactly as with `docs/parity-index.tsv` and the generated data tables. The hash is `sha256` of the
cited lines, right-stripped and newline-joined, truncated to 12 - the same recipe
`tools/parity_index.py` uses, so the two agree about what "the cited lines changed" means.

## Two kinds of citation, and why the repo half is the noisier one

**Vanilla** (`NPC.cs:81066`) moves only when the decompiled tree is regenerated, which is rare and
moves everything at once.

**Repo** (`wld_save.rs:585-587`) moves whenever somebody edits near it, which is often. That noise
is the feature and not a defect in it: the whole failure being prevented is a documented line number
that silently comes to mean something else. Rebuilding after an ordinary edit is one command and a
diff to read; the ~110 rows here are a hundredth of what `check-parity` already asks anyone to
review, so the cost is bounded by construction.

## Ambiguity is an error, not a guess

`npc.rs` names two files in this workspace and `mod.rs` names eleven. Rather than pick one, this
refuses and asks the document to say which - `proto/src/npc.rs:79` rather than `npc.rs:79`. A
citation a tool cannot resolve is one a reader cannot either.
"""

import hashlib
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
INDEX = ROOT / "docs" / "doc-citations.tsv"
COLUMNS = ["doc", "kind", "target", "lines", "hash"]

# Markdown that carries citations. Everything at the top level plus `docs/`; `.tsv` files are data
# and `.scratch/` is not in the repository.
#
# Deduplicated by resolved path, because `CLAUDE.md` is a symlink to `AGENTS.md` and scanning both
# reports every finding in it twice under two names, which is how this was noticed.
def _docs():
    seen, out = set(), []
    for p in sorted(ROOT.glob("*.md")) + sorted((ROOT / "docs").glob("*.md")):
        real = p.resolve()
        if real not in seen:
            seen.add(real)
            out.append(p)
    return out


DOCS = _docs()

# `wld_save.rs:585-587`, `net/listener.rs:157`, `crates/terrustia/src/update.rs:485-498`. The path
# may be any suffix of the real one, which is what lets a document say `dispatch.rs:161` when that
# is unambiguous and `game/ai/mod.rs:1086` when it is not.
REPO_CITE = re.compile(r"(?P<path>[A-Za-z0-9_][A-Za-z0-9_./-]*\.rs):(?P<spans>\d+(?:-\d+)?)")
# `NPC.cs:81066`, `Terraria.GameContent/PortalHelper.cs:105-214`. Same shape parity_index.py uses.
VANILLA_CITE = re.compile(
    r"(?:(?P<dir>[A-Za-z0-9_.]+)/)?(?P<file>[A-Za-z_][A-Za-z0-9_.]*\.cs):(?P<spans>\d+(?:-\d+)?)"
)


def rust_files():
    """Every `.rs` file a document could be pointing at, by path relative to the repo root."""
    out = []
    for crate in sorted((ROOT / "crates").glob("*")):
        for sub in ("src", "tests", "examples", "benches"):
            out.extend(sorted((crate / sub).rglob("*.rs")))
    return [p.relative_to(ROOT) for p in out]


def resolve(cited, candidates):
    """Match a cited path against real ones by path suffix.

    Returns (path, None) on a unique match, or (None, reason) when there is nothing to point at or
    too much. Suffix matching is on whole path segments, so `npc.rs` cannot match `town_npc.rs`.
    """
    parts = tuple(pathlib.PurePosixPath(cited).parts)
    hits = [p for p in candidates if tuple(p.parts)[-len(parts) :] == parts]
    if not hits:
        return None, "no such file"
    if len(hits) > 1:
        shown = ", ".join(str(h) for h in sorted(hits)[:4])
        return None, f"ambiguous, matches {len(hits)} files ({shown}); qualify the path"
    return hits[0], None


def span_of(text):
    """`"585-587"` and `"299"` both become an inclusive 1-based (start, end)."""
    if "-" in text:
        a, b = text.split("-", 1)
        return int(a), int(b)
    n = int(text)
    return n, n


def hash_lines(lines, span):
    """The cited lines, keyed the way `parity_index.py` keys them."""
    a, b = span
    if a < 1 or b < a or b > len(lines):
        return None
    text = "\n".join(ln.rstrip() for ln in lines[a - 1 : b])
    return hashlib.sha256(text.encode()).hexdigest()[:12]


def harvest(tree):
    """Every citation in the documents, as {(doc, kind, target, lines): hash}, plus problems."""
    rows, problems = {}, []
    candidates = rust_files()
    cs_files = {}
    if tree is not None:
        for p in pathlib.Path(tree).rglob("*.cs"):
            cs_files.setdefault(p.name, []).append(p)
    cache = {}

    def lines_of(path):
        if path not in cache:
            try:
                cache[path] = path.read_text(errors="replace").splitlines()
            except OSError:
                cache[path] = None
        return cache[path]

    for doc in DOCS:
        rel_doc = doc.relative_to(ROOT).as_posix()
        for lineno, line in enumerate(doc.read_text().splitlines(), 1):
            for kind, pattern in (("repo", REPO_CITE), ("vanilla", VANILLA_CITE)):
                for m in pattern.finditer(line):
                    spans = m.group("spans")
                    if kind == "repo":
                        target, err = resolve(m.group("path"), candidates)
                        if err:
                            problems.append(f"{rel_doc}:{lineno}: {m.group('path')}:{spans} - {err}")
                            continue
                        path, shown = ROOT / target, target.as_posix()
                    else:
                        # The tree is optional so `--self-test` and a repo-only run still work.
                        if tree is None:
                            continue
                        name = m.group("file")
                        hits = cs_files.get(name, [])
                        if m.group("dir"):
                            want = f"{m.group('dir')}/{name}"
                            hits = [h for h in hits if h.as_posix().endswith(want)]
                        if len(hits) != 1:
                            problems.append(
                                f"{rel_doc}:{lineno}: {name}:{spans} - "
                                f"{'no such file in the tree' if not hits else 'ambiguous'}"
                            )
                            continue
                        path, shown = hits[0], name

                    body = lines_of(path)
                    if body is None:
                        problems.append(f"{rel_doc}:{lineno}: {shown}:{spans} - unreadable")
                        continue
                    digest = hash_lines(body, span_of(spans))
                    if digest is None:
                        problems.append(
                            f"{rel_doc}:{lineno}: {shown}:{spans} - "
                            f"outside the file, which is {len(body)} lines"
                        )
                        continue
                    rows[(rel_doc, kind, shown, spans)] = digest
    return rows, problems


def load_index():
    if not INDEX.exists():
        return None
    out = {}
    for line in INDEX.read_text().splitlines():
        if line.startswith("#") or not line.strip() or line.startswith(COLUMNS[0] + "\t"):
            continue
        doc, kind, target, spans, digest = line.split("\t")
        out[(doc, kind, target, spans)] = digest
    return out


def write_index(rows):
    header = [
        "# Generated by tools/check_doc_citations.py. Do not hand-edit: rebuild with",
        "# `just doc-citations-update` and review the diff, the same rule docs/parity-index.tsv and",
        "# the generated data tables live under.",
        "#",
        "# One row per file:line citation in the markdown. `hash` keys the cited lines as they exist",
        "# now, so `just check-doc-citations` fails when a document's pointer stops meaning what it",
        "# meant. It says a citation moved. It never says whether the sentence around it is true.",
        "#",
        "\t".join(COLUMNS),
    ]
    body = [
        "\t".join([doc, kind, target, spans, digest])
        for (doc, kind, target, spans), digest in sorted(rows.items())
    ]
    INDEX.write_text("\n".join(header + body) + "\n")


def self_test():
    """The parsing and resolution rules, checked without needing a tree or the real documents."""
    assert span_of("299") == (299, 299)
    assert span_of("585-587") == (585, 587)

    # Hashing matches parity_index.py's recipe: right-strip, newline-join, sha256, first 12.
    want = hashlib.sha256("a\nb".encode()).hexdigest()[:12]
    assert hash_lines(["a  ", "b\t", "c"], (1, 2)) == want
    # Out of range is a refusal, not a truncation.
    assert hash_lines(["a"], (1, 9)) is None
    assert hash_lines(["a"], (0, 1)) is None

    # Suffix matching is on whole segments, so a shorter name cannot match a longer one.
    cands = [
        pathlib.PurePosixPath("crates/terrustia/src/game/npc.rs"),
        pathlib.PurePosixPath("crates/terrustia-proto/src/npc.rs"),
        pathlib.PurePosixPath("crates/terrustia/src/game/town_npc.rs"),
    ]
    assert resolve("game/npc.rs", cands)[0] == cands[0]
    assert resolve("terrustia-proto/src/npc.rs", cands)[0] == cands[1]
    # Two files are named npc.rs, so the bare name is refused rather than guessed at.
    path, err = resolve("npc.rs", cands)
    assert path is None and "ambiguous" in err, err
    assert resolve("nothing.rs", cands)[1] == "no such file"

    # The citation patterns, including the forms that actually appear in these documents.
    assert REPO_CITE.search("see `wld_save.rs:585-587` for").group("path") == "wld_save.rs"
    assert REPO_CITE.search("(`net/listener.rs:157`)").group("spans") == "157"
    m = VANILLA_CITE.search("vanilla's `Terraria.GameContent/PortalHelper.cs:105-214` does")
    assert m.group("dir") == "Terraria.GameContent" and m.group("spans") == "105-214"
    # A method reference is not a line citation.
    assert REPO_CITE.search("`Minecart.rs::Initialize`") is None
    print("check_doc_citations: self-test passed")


def main():
    args = [a for a in sys.argv[1:]]
    if "--self-test" in args:
        self_test()
        return 0
    update = "--update" in args
    positional = [a for a in args if not a.startswith("--")]
    tree = positional[0] if positional else None
    if tree is None:
        print("usage: check_doc_citations.py <decompiled> [--update]", file=sys.stderr)
        return 2

    rows, problems = harvest(tree)
    if update:
        write_index(rows)
        print(f"check_doc_citations: wrote {len(rows)} rows to {INDEX.relative_to(ROOT)}")
        if problems:
            print("\nunresolved, and not in the index (fix the citation):")
            for p in problems:
                print(f"  - {p}")
            return 1
        return 0

    index = load_index()
    if index is None:
        print(f"check_doc_citations: {INDEX.relative_to(ROOT)} is missing; run --update", file=sys.stderr)
        return 1

    moved = [(k, index[k], rows[k]) for k in rows.keys() & index.keys() if index[k] != rows[k]]
    added = sorted(rows.keys() - index.keys())
    gone = sorted(index.keys() - rows.keys())

    if not (problems or moved or added or gone):
        print(f"check_doc_citations: all {len(rows)} citations still point at what they were written against")
        return 0

    if problems:
        print(f"{len(problems)} citation(s) that do not resolve at all:")
        for p in problems:
            print(f"  - {p}")
    if moved:
        print(f"\n{len(moved)} citation(s) whose lines have changed since the document was written:")
        for (doc, _kind, target, spans), was, now in sorted(moved):
            print(f"  - {doc} cites {target}:{spans} - was {was}, now {now}")
        print("  Re-read the sentence around each one, then `just doc-citations-update`.")
    if added or gone:
        print(f"\n{len(added)} citation(s) added and {len(gone)} removed since the index was built.")
        print("  If the documents are right, `just doc-citations-update` and review the diff.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
