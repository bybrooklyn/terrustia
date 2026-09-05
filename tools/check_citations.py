#!/usr/bin/env python3
"""Do the lines a citation names actually contain the numbers next to it?

`just check-parity` proves a citation still points at the same *text* it pointed at before. It says
so itself: it never judges whether a transcription is correct. This asks the next question, and it
is a different one - a citation can be perfectly stable and still name the wrong lines.

For every item in `docs/parity-index.tsv`, this reads the vanilla lines it cites and the item's own
body, and reports numbers in the body that are not in those lines. Three buckets come out:

* **Mis-ranged**: the number is not in the cited lines but *is* within thirty lines of them. 70 of
  these repo-wide as of 2026-09-05. `ARAPAIMA_REVERSE_DAMPING = 0.95` cites `NPC.cs:23886-23891`,
  and the `velocity.X *= 0.95f` it documents is at `:23896`; `check-parity` hashes the six lines
  named, which do not contain it, so the real line could change and nothing would notice.

  **This is a candidate list, not a defect list.** The bucket also holds derived values whose
  citations are correct, and this project's own ids where they happen to sit near one - a number
  being absent from its citation has three possible causes and the tool can only see one of them.
  Five were worked through by hand and all five were real, which is the argument for reading it,
  not for trusting it.
* **Distant**: the number is nowhere near. Mostly legitimate - a derived value (`DESTROYER_SEGMENTS`
  is 81 because the game's inclusive loop runs `GetDestroyerSegmentsCount() + 1` times), a computed
  one (`CULTIST_ICE_DAMAGE` transcribes what `attackDamage_ForProjectiles` evaluates to), or an id
  this project chose. 669 of these, which is why this tool reports rather than gates.
* Everything else passes.

Match-arm keys and array lengths are stripped before comparing: `285 | 286 => ...` is this project's
own dispatch, not a number copied from the game, and reading them as transcribed values swamped the
signal in the first version of this.

Report-only by design. Run it when adding citations, or when a transcription is under suspicion.
"""

import importlib.util
import re
import sys
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
NEARBY = 30
NUMBER = re.compile(r"(?<![\w.])(\d+(?:\.\d+)?)")
ARM = re.compile(r"^\s*[-\d |.=]+ =>")
# `[f32; 3]`, `[(i32, i32); 5]`, `[u8; 12]`: the length is this project's own, never a number the
# game wrote. The first version of this excluded digits from the element type, so `f32` did not
# match and every fixed-size array reported its own length as an untraceable value.
ARRAY_LEN = re.compile(r"\[[^\[\];]*;\s*\d+\]")
# Everywhere, in both languages, and never evidence of anything.
STRUCTURAL = {0.0, 1.0, 2.0}


def parity_index():
    spec = importlib.util.spec_from_file_location("parity", REPO / "tools" / "parity_index.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def numbers(text):
    """Every numeric literal, normalised so `1f`, `1`, `1.0` and `1.00` are one number."""
    out = set()
    for m in NUMBER.finditer(text):
        try:
            out.add(float(m.group(1)))
        except ValueError:
            pass
    return out


def cited_text(tree, cs, spans, pad=0):
    for path in tree.rglob(cs):
        lines = path.read_text(errors="replace").split("\n")
        got = []
        for a, b in spans:
            got.extend(lines[max(0, a - 1 - pad) : b + pad])
        return "\n".join(got)
    return None


def strip_array_lengths(line):
    """Remove every `[T; N]`, innermost first.

    One pass is not enough: `[[(u8, i32); 5]; 3]` has its inner array removed and leaves `[; 3]`
    behind, and the outer length then reads as a number the game supposedly wrote. Repeat until
    the line stops changing.
    """
    while True:
        shorter = ARRAY_LEN.sub("", line)
        if shorter == line:
            return line
        line = shorter


def our_numbers(lines, span):
    kept = []
    for line in lines[span[0] : span[1] + 1]:
        if line.strip().startswith("//"):
            continue
        kept.append(strip_array_lengths(ARM.sub("", line)))
    return numbers("\n".join(kept))


def main():
    if len(sys.argv) < 2:
        sys.exit("usage: check_citations.py <decompiled tree> [file to limit to]")
    tree = Path(sys.argv[1])
    only = sys.argv[2] if len(sys.argv) > 2 else None
    parity = parity_index()

    cites = defaultdict(list)
    for row in parity.load_index():
        ours, item, cs, span = row[0], row[1], row[2], row[3]
        # A test's numbers are its own fixtures: `conditional(222, ...)` names the Queen Bee to ask
        # about her, and the 222 is this project's dispatch rather than anything copied out of the
        # game. Reading them as transcribed values put 40-odd tests on the list, all of them noise.
        if "mod tests ::" in item or "::" in item and item.split("::")[0].strip().startswith("mod "):
            continue
        if only is None or ours == only:
            cites[(ours, item)].append((cs, span))

    checked = 0
    misranged, distant = [], []
    for (ours, item), rows in sorted(cites.items()):
        path = REPO / ours
        if not path.exists():
            continue
        lines = path.read_text().split("\n")
        code, _ = parity.split_code_and_comments(lines)
        span = None
        for start, end, name in parity.find_items(lines, code):
            if name == item and (span is None or end - start < span[1] - span[0]):
                span = (start, end)
        if span is None:
            continue

        mine = our_numbers(lines, span)
        theirs, near = set(), set()
        for cs, spantext in rows:
            spans = parity.parse_spans(spantext)
            if text := cited_text(tree, cs, spans):
                theirs |= numbers(text)
            if wide := cited_text(tree, cs, spans, pad=NEARBY):
                near |= numbers(wide)
        if not theirs:
            continue
        checked += 1
        missing = {n for n in mine - theirs if n not in STRUCTURAL}
        if not missing:
            continue
        where = ", ".join(f"{cs}:{span}" for cs, span in rows[:2])
        outside = sorted(n for n in missing if n in near)
        far = sorted(n for n in missing if n not in near)
        # An item is only *mis-ranged* when everything it is missing is nearby. One that is also
        # missing something distant is carrying a derived value, and its citation may be fine; it
        # goes in the other bucket rather than being counted twice.
        if outside and not far:
            misranged.append((ours, item, where, outside))
        else:
            distant.append((ours, item, where, far or outside))

    print(f"{checked} items carry a citation this can resolve to real lines")
    print(f"  {len(misranged)} name lines that do not contain a number they document,")
    print(f"    but it is within {NEARBY} lines: the value is right and the citation is short")
    print(f"  {len(distant)} carry at least one number their citation does not reach at all,")
    print("    which is usually a derived or computed value rather than a transcribed one")
    print()
    print("MIS-RANGED")
    for ours, item, where, nums in misranged:
        print(f"  {Path(ours).name}  {item}  ({where})")
        print(f"    not in the cited lines: {nums}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
