#!/usr/bin/env python3
"""Independently verify the generated recipe table against the decompiled source.

Written from the source rather than from the generator, so a bug shared by both would have to be
made twice. Re-parses every recipe's chunk by hand and compares.

**It reads all 3,090 of them now; it used to read 2,545.** The 545 it could not see were not one
gap but five, and every one was found by mutation-testing this checker (`just check-mutants`)
rather than by reading it:

* `SetupRecipes` builds 421 recipes through two parameterised helpers, whose bodies name their
  result as a parameter. [`inline`] substitutes each call's arguments and splices the body in.
* `AddCritterStatueRecipe`'s group branch takes its ingredient from
  `RecipeGroup.GetPlaceholderItemType()`, which is the group's first registered member
  ([`recipe_groups`]), and steps its slot with `requiredItem[++num]`, which the local resolver did
  not model - costing the second ingredient *and* overwriting the first one's stack.
* Three counted loops write a *different* recipe per iteration (`SetDefaults(2702 + m)` is
  thirty-six of them). Binding the counter to its last value, as this file used to, read one.
  [`unroll`] writes them all out, which is also right for the same-result loops.
* One `int[,]` table drives thirty-six more ([`unroll_table`]), and the fake-chest recipes take
  their ingredient from `ItemID.Sets.TextureCopyLoad` in another file ([`texture_copy_load`]).
* Fourteen recipes name their result through a *reassigned* local (`num3 = 2677;`), which only
  bound on its `int` declaration.

Nothing is skipped as unreadable any more. The two recipes that were are `CreateReverse*`'s, whose
output is `notDecraftable` and never reaches the table at all, so those two methods are dropped
outright rather than reported.
"""
import re
import sys
import pathlib

D = sys.argv[1]
GEN = pathlib.Path(sys.argv[2]).read_text()
src = pathlib.Path(D + "/Terraria/Recipe.cs").read_text(errors="replace")

# Pull the generated tables back out of the Rust. `rustfmt` puts a space after every tuple comma
# (`(1, 1419)`, not `(1,1419)`) and lays each `Recipe { .. }` out one field per line rather than on
# a single line — both defeated the old space-free, single-line-only regexes below, which then
# matched nothing at all (0 rows) rather than erroring, so every real recipe looked MISSING. `\s*`
# tolerates a run's actual whitespace either way, single space or newline-plus-indent alike.
ing = [
    (int(a), int(b))
    for a, b in re.findall(
        r"\((\d+),\s*(\d+)\),", GEN.split("static INGREDIENTS")[1].split("];")[0]
    )
]
recipes = [
    (int(m.group(1)), int(m.group(2)), int(m.group(3)), int(m.group(4)))
    for m in re.finditer(
        r"Recipe\s*\{\s*result:\s*(\d+),\s*makes:\s*(\d+),\s*first:\s*(\d+),\s*count:\s*(\d+),",
        GEN,
    )
]
crafted = {
    int(a): int(b)
    for a, b in re.findall(
        r"\((\d+),\s*(\d+)\),", GEN.split("static CRAFTED_BY:")[1].split("];")[0]
    )
}
if not crafted:
    # A parser that silently returns nothing is worse than one that errors: an empty `crafted`
    # makes every single sampled item look MISSING, which is indistinguishable from "everything is
    # actually broken" unless this is caught explicitly. This is exactly the failure mode that let
    # this checker rot unnoticed — see the regex comment above.
    raise SystemExit("parsed 0 CRAFTED_BY rows; recipes.rs's shape changed under this regex")

# Re-parse the source independently: last decraftable recipe per result wins.
body = src[src.index("public static void SetupRecipes()") :]


def resolve_locals(text: str) -> str:
    """Substitute the integer locals and ingredient arrays `SetupRecipes` hoists its materials
    into, so a recipe whose material is a variable is not read as no material at all.

    `Recipe.cs` declares exactly three that matter (`int num = 5; int stack = 2;` for the sofas,
    `int type = 3234;` for the crystal furniture, `int num = 3955;` for the Lesion furniture) and
    passes several of the Lesion ones through an `int[] objN = new int[K] { 0, ... }; objN[0] =
    num;` array. Reading only digits made a sofa give back 1+1 instead of 5+2 and turned the Lesion
    Bed's two ingredients into "one of item 7", the array variable's own trailing digit.

    A member access is never substituted: `requiredItem[0].stack = stack;` has one `stack` that is
    a field and one that is the local, and only the second is a value.
    """
    ints: dict[str, str] = {}
    arrays: dict[str, list[int]] = {}
    out: list[str] = []
    ident = re.compile(r"(?<![\w.])(\w+)")
    for raw in text.splitlines():
        line = raw.strip()
        # `int num3 = 2677;` and the bare `num3 = 2677;` that reassigns it later. Only the
        # declaration used to bind, so fourteen recipes written through a reassigned local
        # (`num2 = 1976; num3 = 2677;`) read their result as an identifier and were skipped.
        # A member access is not a local: `requiredItem[0].stack = 20;` has a dot in it.
        if m := re.fullmatch(r"(?:int )?(\w+) = (\d+);", line):
            ints[m.group(1)] = m.group(2)
            out.append(raw)
            continue
        # `for (int l = 3309; l <= 3314; l++) { ... SetDefaults(l) ... }` writes six recipes for
        # one result, and this file keeps the last decraftable recipe per result, so the surviving
        # one is the final iteration's. Binding the counter to its *last* value reproduces exactly
        # that recipe. Without this the loop counter was not a digit, the ingredient slot it fills
        # never matched, and the recipe came out one ingredient short: item 5547 read as "makes 1
        # from item 3306" when it takes 3306 *and* 3314. That was reported as the table being
        # wrong; the table was right and this checker was blind. `SetupRecipes` has three loops of
        # this counted shape (3665..=3704, 2114..=2118, 3309..=3314, 4327..=4332); the rest count
        # over an array length and write no recipe of their own.
        if m := re.match(r"for \(int (\w+) = ", line):
            counted = re.fullmatch(r"for \(int (\w+) = (\d+); \1 <= (\d+); \1\+\+\)", line)
            if counted:
                ints[counted.group(1)] = counted.group(3)
            else:
                # A loop over an array length rebinds the same short name (`i`, `j`) to something
                # this cannot resolve. Forgetting it is the point: leaving the previous counted
                # loop's last value bound would substitute it into an unrelated line later.
                ints.pop(m.group(1), None)
            out.append(raw)
            continue
        if m := re.fullmatch(r"int\[\] (\w+) = new int\[\d*\] \{([^}]*)\};", line):
            arrays[m.group(1)] = [int(n) for n in re.findall(r"-?\d+", m.group(2))]
            out.append(raw)
            continue
        # `requiredItem[++num]` steps the slot counter and uses the new value. Only
        # `AddCritterStatueRecipe` writes it, and only once, but reading it as anything else costs
        # the second ingredient *and* overwrites the first one's stack with the second's.
        if m := re.search(r"\[\+\+(\w+)\]", line):
            name = m.group(1)
            if name in ints:
                ints[name] = str(int(ints[name]) + 1)
                line = line.replace("[++%s]" % name, "[%s]" % ints[name])
        if re.match(r"(?:currentRecipe\.|\w+\.SetIngredients\(|\w+\[\d+\] = )", line):
            line = ident.sub(lambda m: ints.get(m.group(1), m.group(1)), line)
        # `objN[0] = 3955;` patches the placeholder its declaration left behind.
        if (m := re.fullmatch(r"(\w+)\[(\d+)\] = (-?\d+);", line)) and m.group(1) in arrays:
            values = arrays[m.group(1)]
            if int(m.group(2)) < len(values):
                values[int(m.group(2))] = int(m.group(3))
        # `recipeN.SetIngredients(objN);` becomes the numbers themselves.
        if (m := re.fullmatch(r"(\w+\.SetIngredients\()(\w+)(\);)", line)) and m.group(
            2
        ) in arrays:
            line = m.group(1) + ", ".join(map(str, arrays[m.group(2)])) + m.group(3)
        # The one arithmetic stack in the whole file (`Recipe.cs:6374`); a C# `(int)` cast of a
        # positive float truncates.
        line = re.sub(
            r"\(int\)\(\(float\)(\d+) \* ([\d.]+)f\)",
            lambda m: str(int(float(m.group(1)) * float(m.group(2)))),
            line,
        )
        out.append(line)
    return "\n".join(out)


HELPERS = ("AddStandardFurnitureSetRecipes", "AddCritterStatueRecipe")


def texture_copy_load(root):
    """`ItemID.Sets.TextureCopyLoad`, the one lookup `SetupRecipes` makes into another file.

    `Factory.CreateIntSet(default, key, value, key, value, ...)` (`ItemID.cs:1096`). The fake-chest
    recipes take their ingredient from it, and unrolling their loop turned one unreadable recipe
    into forty, all of them this. Reading it here closes them.
    """
    text = (root / "Terraria.ID" / "ItemID.cs").read_text(errors="replace")
    m = re.search(r"TextureCopyLoad = Factory\.CreateIntSet\(([^)]*)\);", text)
    if not m:
        raise SystemExit("no ItemID.Sets.TextureCopyLoad; ItemID.cs's shape changed")
    nums = [int(n) for n in re.findall(r"-?\d+", m.group(1))]
    return {nums[i]: nums[i + 1] for i in range(1, len(nums) - 1, 2)}


TABLE_2D = re.compile(r"^\t+int\[,\] (\w+) = new int\[(\d+), 2\]$")


def drop_reverse_helpers(text):
    """Remove `CreateReversePlatformRecipes` and `CreateReverseWallRecipes`.

    They build a recipe from another recipe (`SetDefaults(Main.recipe[i].requiredItem[0].type)`),
    which this checker cannot resolve, and then set `notDecraftable = true` on it *after*
    `AddRecipe()` - outside the chunk, so the ordinary decraft check cannot see it either. Their
    output is excluded from the table by definition, so removing them is exact rather than a
    concession: it is the difference between "two recipes this cannot read" and "two recipes that
    are not in the table".
    """
    for name in ("CreateReversePlatformRecipes", "CreateReverseWallRecipes"):
        head = re.search(r"^\tprivate static void %s\(\)$" % name, text, re.M)
        if not head:
            continue
        lines = text[head.end():].split("\n")
        depth, seen, span = 0, False, 0
        for line in lines:
            depth += line.count("{") - line.count("}")
            span += len(line) + 1
            if line.count("{"):
                seen = True
            if seen and depth == 0:
                break
        text = text[: head.start()] + text[head.end() + span :]
    return text


def unroll_table(text):
    """The one `int[,] array = new int[36, 2] { { a, b }, ... };` and its `GetLength(0)` loop.

    Thirty-six fake-chest recipes, each `SetDefaults(array[j, 0])` from `array[j, 1]`. Nothing else
    in `SetupRecipes` is shaped like this, so it is read here rather than given a general
    two-dimensional evaluator.
    """
    lines = text.split("\n")
    tables = {}
    for i, line in enumerate(lines):
        m = TABLE_2D.match(line)
        if not m:
            continue
        rows, j = [], i + 1
        while j < len(lines) and "};" not in lines[j]:
            pair = re.findall(r"-?\d+", lines[j])
            if len(pair) == 2:
                rows.append((int(pair[0]), int(pair[1])))
            j += 1
        if len(rows) == int(m.group(2)):
            tables[m.group(1)] = rows
    if not tables:
        return text

    head = re.compile(r"^(\t+)for \(int (\w+) = 0; \2 < (\w+)\.GetLength\(0\); \2\+\+\)$")
    out, i = [], 0
    while i < len(lines):
        m = head.match(lines[i])
        if not m or m.group(3) not in tables or lines[i + 1].strip() != "{":
            out.append(lines[i])
            i += 1
            continue
        var, rows = m.group(2), tables[m.group(3)]
        depth, j = 0, i + 1
        while j < len(lines):
            depth += lines[j].count("{") - lines[j].count("}")
            if depth == 0:
                break
            j += 1
        body = lines[i + 2 : j]
        for a, b in rows:
            for line in body:
                out.append(
                    line.replace("%s[%s, 0]" % (m.group(3), var), str(a))
                    .replace("%s[%s, 1]" % (m.group(3), var), str(b))
                )
        i = j + 1
    return "\n".join(out)


def unroll(text):
    """Write out every iteration of a counted loop that has numeric bounds.

    `resolve_locals` used to bind such a counter to its *last* value, which is right only when
    every iteration writes the same result. Three of these write a different one each time -
    `for (int m = 0; m < 36; m++) { SetDefaults(2702 + m); ... }` is 36 separate recipes - so
    binding the last value read one recipe and left 35 unchecked. Unrolling is also right for the
    same-result loops: the checker keeps the last recipe per result either way.

    Loops whose bound is not a number (`i < numRecipes`) are left alone. They belong to the
    post-processing passes that run after every recipe has been added, and create none.
    """
    head = re.compile(r"^(\t+)for \(int (\w+) = (\d+); \2 (<=|<) (\d+); \2\+\+\)$")
    lines = text.split("\n")
    out, i = [], 0
    while i < len(lines):
        m = head.match(lines[i])
        if not m:
            out.append(lines[i])
            i += 1
            continue
        indent, var, lo, op, hi = m.group(1), m.group(2), int(m.group(3)), m.group(4), int(m.group(5))
        if lines[i + 1].strip() != "{":
            out.append(lines[i])
            i += 1
            continue
        depth, j = 0, i + 1
        while j < len(lines):
            depth += lines[j].count("{") - lines[j].count("}")
            if depth == 0:
                break
            j += 1
        body = lines[i + 2 : j]
        for n in range(lo, hi + 1 if op == "<=" else hi):
            out.append("%sint %s = %d;" % (indent, var, n))
            out.extend(body)
        i = j + 1
    return "\n".join(out)


def recipe_groups(text):
    """`RecipeGroups.X` -> the item `GetPlaceholderItemType()` would return.

    That method is `return Items[0];` (`RecipeGroup.cs:101-103`) and the constructor adds
    `params int[] validItems` in order (`:35-52`), so the placeholder is the first id in the
    registration: `new RecipeGroup("Misc.Cockatiel", 5312, 5313)` is 5312. Derived here rather
    than taken from the generator, which is the point of this file existing.
    """
    out = {}
    for m in re.finditer(
        r"RecipeGroups\.(\w+) = new RecipeGroup\((?:\"[^\"]*\"|[^,]+), (\d+)", text
    ):
        out[m.group(1)] = int(m.group(2))
    if not out:
        raise SystemExit("parsed 0 RecipeGroup registrations; Recipe.cs's shape changed")
    return out


def method(text, name):
    """`(parameter names with defaults, body lines)` for one helper."""
    head = re.search(r"^\tprivate static void %s\((.*)\)$" % name, text, re.M)
    if not head:
        sys.exit("no %s in Recipe.cs" % name)
    params = []
    for part in head.group(1).split(","):
        part = part.strip()
        m = re.fullmatch(r"\w[\w<>\[\]]* (\w+)(?: = (.+))?", part)
        if not m:
            sys.exit("unreadable parameter %r of %s" % (part, name))
        params.append((m.group(1), m.group(2)))
    lines = text[head.end() :].split("\n")
    depth, seen, body = 0, False, []
    for line in lines:
        depth += line.count("{") - line.count("}")
        if line.count("{"):
            seen = True
        body.append(line)
        if seen and depth == 0:
            break
    return params, body[1:-1]


def split_args(text):
    out, depth, current = [], 0, ""
    for ch in text:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(current.strip())
            current = ""
            continue
        current += ch
    if current.strip():
        out.append(current.strip())
    return out


def inline(text):
    """Replace every call to a recipe-building helper with its body, arguments bound."""
    for name in HELPERS:
        params, body = method(text, name)
        call = re.compile(r"^\t\t%s\((.*)\);$" % name, re.M)

        def expand(m, params=params, body=body, name=name):
            args = split_args(m.group(1))
            if len(args) > len(params):
                sys.exit("%s called with %d arguments for %d parameters" % (name, len(args), len(params)))
            bindings = []
            for i, (param, default) in enumerate(params):
                value = args[i] if i < len(args) else default
                # A `RecipeGroup` argument is not a number and no recipe result depends on one:
                # `AddCritterStatueRecipe` uses it only to pick a placeholder item, which this
                # checker already cannot resolve and already reports as unreadable.
                if value is not None and (g := re.fullmatch(r"RecipeGroups\.(\w+)", value.strip())):
                    # `AddCritterStatueRecipe`'s group branch overwrites `critterItem` with the
                    # group's placeholder, so bind that rather than the literal 0 the call passes.
                    bindings.append("\t\tint critterItem = %d;" % GROUPS[g.group(1)])
                    continue
                if value is None or not re.fullmatch(r"-?\d+", value.strip()):
                    continue
                bindings.append("\t\tint %s = %s;" % (param, value.strip()))
            return "\n".join(bindings + body)

        text = call.sub(expand, text)
    return text



TEXTURE_COPY = texture_copy_load(pathlib.Path(D))
GROUPS = recipe_groups(src)
body = unroll(unroll_table(inline(drop_reverse_helpers(body))))
body = resolve_locals(body)
# The result is not always a plain number once the counted loops are unrolled: `SetDefaults(2702
# + 0)` is the first of thirty-six. Captured as text and read by `literal` like any ingredient.
chunks = re.findall(
    r"currentRecipe\.createItem\.SetDefaults\(([^()]*)\);(.*?)AddRecipe\(\);", body, re.S
)

def literal(arg: str) -> int | None:
    """A `SetDefaults(...)` argument as an integer, or `None` when it cannot be read here.

    Two shapes beyond a plain number occur in `SetupRecipes`: `num5 - 4327 + 4334`, which is
    arithmetic on a loop counter `resolve_locals` has already substituted, and
    `ItemID.Sets.TextureCopyLoad[i]`, a lookup into a table that lives in another file.

    Returning `None` for the second is the point. The old code's regex simply did not match it, so
    the ingredient vanished and the recipe was compared *one slot short*: item 3704 was reported
    as a table error when the table was right and this checker could not read the source. A
    checker that cannot read something has to say so, not quietly compare the remainder.
    """
    arg = arg.strip()
    if re.fullmatch(r"-?\d+", arg):
        return int(arg)
    if m := re.fullmatch(r"ItemID\.Sets\.TextureCopyLoad\[(\d+)\]", arg):
        return TEXTURE_COPY.get(int(m.group(1)))
    if re.fullmatch(r"-?\d+(?:\s*[-+]\s*\d+)+", arg):
        total, sign = 0, 1
        for token in re.findall(r"[-+]|\d+", arg):
            if token in "-+":
                sign = -1 if token == "-" else 1
            else:
                total += sign * int(token)
                sign = 1
        return total
    return None


truth = {}
unreadable = 0
for result, text in chunks:
    # The decraft check comes first: `CreateReverse*`'s recipes name their result as
    # `Main.recipe[i].requiredItem[0].type`, which is unreadable *and* not decraftable, and
    # counting them as the former would report two permanent skips that are not gaps at all.
    if "notDecraftable = true" in text or "DisableDecraft()" in text:
        continue
    result = literal(result)
    if result is None:
        unreadable += 1
        continue
    slots = {}
    skip = False
    for m in re.finditer(r"requiredItem\[(\d+)\]\.SetDefaults\(([^()]*)\)", text):
        value = literal(m.group(2))
        if value is None:
            skip = True
            break
        if value > 0:
            slots[int(m.group(1))] = [value, 1]
    if skip:
        unreadable += 1
        continue
    for m in re.finditer(r"requiredItem\[(\d+)\]\.stack = (\d+)", text):
        if int(m.group(1)) in slots:
            slots[int(m.group(1))][1] = int(m.group(2))
    m = re.search(r"SetIngredients\(([^)]*)\)", text)
    if m:
        nums = [int(x) for x in re.findall(r"-?\d+", m.group(1))]
        at = len(slots)
        for i in range(0, len(nums) - 1, 2):
            if nums[i] > 0:
                slots[at] = [nums[i], max(1, nums[i + 1])]
                at += 1
        if len(nums) % 2 == 1 and nums[-1] > 0:
            slots[at] = [nums[-1], 1]
    if not slots:
        continue
    makes = 1
    sm = re.search(r"createItem\.stack = (\d+)", text)
    if sm:
        makes = int(sm.group(1))
    truth[result] = (makes, [tuple(v) for _, v in sorted(slots.items())])

# Every craftable item, not a sample of them.
#
# This used to be `random.seed(20260823); random.sample(sorted(truth), 300)`: 300 of the 2543
# recipes the source defines, drawn against a *fixed* seed, so the same 88% of the table was never
# compared with anything and never would be. `tools/mutate_tables.py` measured exactly that:
# corrupting 40 random `result:` fields in `recipes.rs` was caught 3 times out of 40, and the three
# were the ones that happened to fall inside the sample. The whole run costs a fraction of a second
# either way, so the sampling bought nothing and hid nearly everything.
sample = sorted(truth)
bad = 0
for item in sample:
    makes, wants = truth[item]
    if item not in crafted:
        print(f"  MISSING: item {item} is craftable in the source but not in the table")
        bad += 1
        continue
    result, gen_makes, first, count = recipes[crafted[item]]
    got = ing[first : first + count]
    if result != item or gen_makes != makes or got != wants:
        print(f"  WRONG: item {item}")
        print(f"    source: makes {makes} from {wants}")
        print(f"    table:  makes {gen_makes} from {got} (result field {result})")
        bad += 1

print(f"checked {len(sample)} recipes against the source independently")
print(f"  {len(truth)} craftable items in the source, {len(crafted)} in the table")
print(f"  {unreadable} recipe(s) skipped: an ingredient this checker cannot resolve to a number")
print("  " + ("ALL MATCH" if bad == 0 else f"{bad} DISAGREEMENTS"))
sys.exit(1 if bad else 0)
