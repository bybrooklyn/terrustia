//! The dead-write lint: struct fields this server *writes* in production and never *reads* there.
//!
//! ```text
//! cargo run -q -p terrustia-codegen --bin deadwrite
//! ```
//!
//! Why this exists, and why `rustc` cannot do it. `dead_code` already reports a field that is never
//! read, but a read inside a `#[cfg(test)]` module counts as a read, so a field the tests assert on
//! and production ignores is invisible to it. That is not a hypothetical shape: `Npc::damage_bonus`
//! is assigned at thirteen production sites (every boss enrage and every second-form damage
//! multiplier in the game) and read at none, and the unit tests around each of those sites passed,
//! because each one asserted that the flag had been *set*. A test that checks a producer and no
//! consumer proves the producer runs, and nothing else.
//!
//! What it does: parses every production `.rs` file under the server and proto crates with `syn`,
//! collects field writes and field reads, and reports fields with at least one production write and
//! no production read. Reads inside `#[cfg(test)]` modules, `#[test]`/`#[tokio::test]` functions,
//! `tests/` and `examples/` are deliberately *not* production reads; they are counted separately so
//! a report can say "read only by the tests" rather than "unused".
//!
//! Known limits, stated rather than hidden:
//!
//!  * Fields are keyed by **name**, not by declaring type: `syn` parses, it does not infer types, so
//!    `a.foo` cannot be resolved to a struct. Two structs with a same-named field share one verdict,
//!    which is conservative in the safe direction (any read anywhere clears both). The report lists
//!    every struct that declares the name so triage knows which ones are in play.
//!  * A field read only inside the module that writes it still counts as read. `FairyOutcome::
//!    wants_treasure` is that shape: `fairy.rs` reads its own flag, and the *dispatch* is what
//!    ignores it. Cross-module dataflow is a different check.
//!  * `npc.ai[3]` is a slot of a field, not a field. `ai` is read all over, so slot-level dead
//!    writes (the slime that never un-sticks from a wall) do not surface here.
//!  * Inside a macro body only a **dotted** mention counts as a read: `scan_tokens` looks for an
//!    ident directly after a `.`, because macro tokens carry no read/write distinction and
//!    counting every ident would excuse any field whose name appears in a `format!`. The cost is
//!    that a struct *pattern* inside a macro is invisible - `ConsoleLine { kind, level, text }`
//!    in `panel/mod.rs`'s `tokio::select!` is a real read this lint cannot see, and is on the
//!    `ALLOWED` list saying so. Outside a macro, `visit_field_pat` handles destructuring properly.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;
use proc_macro2::TokenTree;
use syn::visit::{self, Visit};

/// Fields that really are written and never read in production, on the record with the reason.
///
/// A key is `Struct::field`, `Struct::*` for a whole record type, or a bare `field` for a name
/// declared in one place. An entry only excuses a field when *every* struct declaring that name
/// matches it, so adding `Foo::bar` cannot quietly excuse an unrelated `Baz::bar`.
///
/// The bar is a *named* consumer that exists outside this workspace's own production code, or a
/// decision written down in the field's own doc comment. "It is probably fine" is not a reason;
/// a field with no reason belongs in the report, where somebody has to look at it.
const ALLOWED: &[(&str, &str)] = &[
    // `NPC.rarity`, generated from `SetDefaultsFromNetId` into all 65 net variants. Its vanilla
    // readers were traced rather than assumed, and there are two: `ContentSamples.cs:1228-1249`
    // sorts the bestiary with it, which is client-side content browsing a dedicated server has no
    // part in; and `CoinLossRevengeSystem.cs:351` uses `npc.rarity > 0` as one of the conditions
    // that *excludes* an NPC from getting a coin-loss revenge marker. This server models the
    // receiving half of that system (packet 92, `dispatch.rs::on_extra_value`) but not the marker
    // cache the gate belongs to, so there is nothing here for the field to gate yet.
    //
    // Excused rather than deleted because the value is right and comes free with the generated
    // table; the open work is the marker cache, tracked in `TODO.md`. This entry was added
    // 2026-09-06, when `check-dead-writes` was run for the first time in a while and turned out to
    // have been failing on `main` since `4d27097` introduced the field.
    (
        "NetVariant::rarity",
        "generated with the net-variant table; vanilla's only server-side reader is \
         CoinLossRevengeSystem's exclusion gate, and that marker cache is not modelled here yet \
         (TODO.md)",
    ),
    // The worldgen census. Its own doc says what it is for: "Returned so callers - and the tests
    // that guard this - can assert a world is playable rather than merely non-empty." The
    // consumer is the assertion, which is legitimately a test.
    (
        "Built::*",
        "the worldgen census, returned for the tests that assert a world is playable",
    ),
    // The reference-world comparison record: read by `roundtrip_wld` and the worldgen tests, which
    // is the whole point of generating it.
    (
        "Outcome::checked",
        "reference-world comparison diagnostics, read by the worldgen tests",
    ),
    (
        "Outcome::first_divergence",
        "reference-world comparison diagnostics, read by the worldgen tests",
    ),
    (
        "Divergence::expected",
        "reference-world comparison diagnostics, read by the worldgen tests",
    ),
    (
        "Divergence::got",
        "reference-world comparison diagnostics, read by the worldgen tests",
    ),
    (
        "PassResult::duration_ms",
        "parsed out of a real world's manifest for comparison; the game fills it, we only report it",
    ),
    (
        "Manifest::final_hash",
        "parsed out of a real world's manifest; the game only writes it with its own worldgen \
         debugger on, so it is almost always absent",
    ),
    (
        "Report::rounds",
        "a work measure for the liquid-settle tests, which assert it converges",
    ),
    (
        "Player::sitting",
        "packet 13's sitting bit, kept because it is the input to the red-hat Skeletron check; the \
         field's own doc already says nothing reads it yet",
    ),
    // Six debuffs whose only read on the NPC side in the real game is a dust or colour effect the
    // client draws: `NPC.cs:92278` (blueLightning), `92286` (redLightning), `92295`
    // (markedByScytheWhip), `92455` (loveStruck), `92476` (dripping) and the same block for
    // dripping_sparkle_slime, all inside the drawing/dust routine. The server has nothing to do
    // with them beyond telling the client the buff is on, which the buff sync already does.
    (
        "Flags::dripping",
        "client-side dust only in the real game (NPC.cs:92476)",
    ),
    (
        "Flags::dripping_sparkle_slime",
        "client-side dust only in the real game (NPC.cs:92476 block)",
    ),
    (
        "Flags::love_struck",
        "client-side dust only in the real game (NPC.cs:92455)",
    ),
    (
        "Flags::marked_by_scythe_whip",
        "client-side dust only in the real game (NPC.cs:92295); no read in Player.cs or \
         Projectile.cs either",
    ),
    (
        "Flags::blue_lightning",
        "client-side dust only in the real game (NPC.cs:92278)",
    ),
    (
        "Flags::red_lightning",
        "client-side dust only in the real game (NPC.cs:92286)",
    ),
    // The three armour-shredding debuffs. Their entire NPC-side effect is
    // `NPC.checkArmorPenetration` (`NPC.cs:81972-81990`), whose three callers -
    // `Player.cs:44763`, `Player.cs:20602` (behind `whoAmI == Main.myPlayer`) and
    // `Projectile.cs:13686` (behind `ownedBySomeone` and an
    // `Invariant.Assert(netMode == 0 || owner == Main.myPlayer)`) - all run on the hitting client,
    // which then sends the penetration already added into packet 28's damage. A server never runs
    // any of them. Traced against the decompiled tree 2026-08-31; the full walk is in the
    // `game/buffs.rs` module doc.
    (
        "Flags::ichor",
        "armour penetration is computed and sent by the hitting client (NPC.cs:81972, all three \
         callers client-owned)",
    ),
    (
        "Flags::broken_armor",
        "armour penetration is computed and sent by the hitting client (NPC.cs:81972, all three \
         callers client-owned)",
    ),
    (
        "Flags::betsys_curse",
        "armour penetration is computed and sent by the hitting client (NPC.cs:81972, all three \
         callers client-owned)",
    ),
    (
        "Flags::stinky",
        "colour and gore on the client (NPC.cs:92208, :92465); its three server-side reads are the \
         town-NPC threat search (:54033-54084), a resident starting to walk (:54181) and \
         TryRemovingWaterPerishableEffects (:94433), none of which this server models",
    ),
    (
        "Flags::shimmering",
        "gates UpdateHomeTileState (NPC.cs:53846, :53913) - this server takes home tiles from \
         housing, never from where an NPC stands - and drives shimmerTransparency to GetShimmered \
         (:92634), a transformation no NPC here undergoes",
    ),
    // Transcribed record fields whose only vanilla consumer is a branch this generator does not
    // run, or whose consumer is legitimately the tests, the shape `Built::*` above already
    // records.
    (
        "CaveCount::sand",
        "countTiles' sandCount column (WorldGen.cs:9506, :9576); its one vanilla consumer is \
         inside `if (remixWorldGen)` (:17929) and this generator has no remix worldgen",
    ),
    (
        "LogScatterResult::last_log",
        "vanilla's GenVars.logX/logY (WorldGen.cs:18775); its one consumer is inside \
         `if (remixWorldGen)` in the Flowers pass (:20631) and this generator has no remix worldgen",
    ),
    (
        "Outcome::ran_out",
        "a stop reason for the mass-wire tests, which assert a run halts for want of materials; \
         vanilla signals the same thing implicitly through MassWireOperationPay's amounts",
    ),
    (
        "ConsoleLine::level",
        "read by the panel's WebSocket feed (panel/mod.rs:819) in a `ConsoleLine { kind, level, \
         text }` pattern inside a `tokio::select!`; macro bodies are only scanned for `.field` \
         reads, so this is a limit of this lint rather than a dead write",
    ),
    // `terrustia-proto` is the MIT wire-format library, published to crates.io. A transcribed
    // table column is part of its published shape whether or not this server happens to consume
    // it, so long as the column is really in the game's table.
    (
        "Seat::mount",
        "a published `terrustia-proto` table column (Terraria's own rider->mount pairing)",
    ),
    (
        "TownToughness::reload",
        "a published `terrustia-proto` table column (the town-NPC attack-cooldown step)",
    ),
    (
        "Offer::floor",
        "a published `terrustia-proto` table column (Chest.cs's own `minimumRarity` floor)",
    ),
    (
        "NpcStats::lava_immune",
        "a published `terrustia-proto` table column, `NPC.lavaImmune` (NPC.cs:6526) as \
         SetDefaults sets it for 49 of the 691 types; its one consumer, \
         Collision_LavaCollision's 50 damage and On Fire (NPC.cs:94468), needs per-NPC lava \
         contact, which this server does not detect at all (`lava_wet` is hard-coded false at \
         systems.rs:92)",
    ),
];

fn main() {
    let repo = repo_root();
    let mut data = Data::default();

    // Production: the two crates whose fields are under audit, plus the client's own `src`, since a
    // client read of a `terrustia-proto` field is a real consumer of it.
    for crate_name in ["terrustia", "terrustia-proto", "terrustia-client"] {
        let src = repo.join("crates").join(crate_name).join("src");
        let declares = crate_name != "terrustia-client";
        for file in rust_files(&src) {
            scan(&mut data, &repo, &file, declares, false);
        }
    }
    // Not production: integration tests and the verification examples.
    for crate_name in ["terrustia", "terrustia-proto", "terrustia-client"] {
        for sub in ["tests", "examples"] {
            let dir = repo.join("crates").join(crate_name).join(sub);
            for file in rust_files(&dir) {
                scan(&mut data, &repo, &file, false, true);
            }
        }
    }

    let allowed: BTreeMap<&str, &str> = ALLOWED.iter().copied().collect();
    let mut findings = 0usize;
    let mut excused = 0usize;
    let mut foreign = 0usize;
    let mut serialized = 0usize;

    println!(
        "{} fields declared, {} written in production, {} read in production",
        data.decls.len(),
        data.writes.len(),
        data.reads.len()
    );
    println!();

    for (name, sites) in &data.writes {
        if data.reads.contains_key(name.as_str()) {
            continue;
        }
        let Some(decls) = data.decls.get(name.as_str()) else {
            // A write into a struct these crates do not declare: a `tokio`, `serde_json` or
            // `terrustia-client` type, whose consumer is that crate's own code. Out of scope.
            foreign += 1;
            continue;
        };
        // `#[derive(Serialize)]` *is* the consumer: the generated impl reads every field and the
        // value leaves as JSON. The panel's response structs are all this shape, and reporting them
        // would be reporting serde.
        if decls.iter().any(|d| d.derives.contains("Serialize")) {
            serialized += 1;
            continue;
        }
        if let Some(reason) = excuse(&allowed, name, decls) {
            excused += 1;
            println!("allowed  {name}: {reason}");
            continue;
        }
        findings += 1;
        let where_declared = decls
            .iter()
            .map(|d| format!("{} {}::{} at {}:{}", d.vis, d.owner, name, d.file, d.line))
            .collect::<Vec<_>>()
            .join("\n           ");
        let test_reads = data.test_reads.get(name.as_str()).copied().unwrap_or(0);
        println!("DEAD WRITE  {name}");
        println!("  declared  {where_declared}");
        if let Some(derives) = decls
            .iter()
            .map(|d| d.derives.as_str())
            .find(|d| !d.is_empty())
        {
            println!("  derives   {derives}");
        }
        // A generated table writes one field a few hundred times and the list stops being useful
        // after the first few; the count is what says "this is a table", the sites say where.
        let listed = sites.len().min(6);
        let tail = if sites.len() > listed {
            format!(", and {} more", sites.len() - listed)
        } else {
            String::new()
        };
        println!(
            "  written   {} production site(s): {}{tail}",
            sites.len(),
            sites[..listed].join(", ")
        );
        println!("  read      0 times in production, {test_reads} time(s) in tests/examples");
        println!();
    }

    println!(
        "{findings} dead write(s), {excused} allowed on the record, \
         {serialized} skipped as serde-serialized, {foreign} skipped as foreign fields"
    );
    if findings > 0 {
        println!();
        println!("Each of these is a producer with no consumer: the code that sets it runs, and");
        println!("nothing in production ever looks at the result. Either wire up the consumer or");
        println!("put the field in ALLOWED at the top of this file with the reason it stays.");
        std::process::exit(1);
    }
}

/// Whether an [`ALLOWED`] entry covers this field, and its reason.
///
/// Every struct declaring the name has to match: `Foo::bar` does not excuse an unrelated
/// `Baz::bar` that happens to share a name, which matters because this lint keys on names.
fn excuse<'a>(allowed: &BTreeMap<&str, &'a str>, name: &str, decls: &[Decl]) -> Option<&'a str> {
    if let Some(reason) = allowed.get(name) {
        return Some(reason);
    }
    let mut found: Option<&'a str> = None;
    for decl in decls {
        let reason = allowed
            .get(format!("{}::{name}", decl.owner).as_str())
            .or_else(|| allowed.get(format!("{}::*", decl.owner).as_str()))
            .copied()?;
        found.get_or_insert(reason);
    }
    found
}

/// The workspace root, found from this binary's own source path rather than the cwd, so the lint
/// gives the same answer from anywhere.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/terrustia-codegen has two ancestors")
        .to_path_buf()
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn scan(data: &mut Data, repo: &Path, file: &Path, declares: bool, is_test_file: bool) {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("read {}: {e}", file.display());
            return;
        }
    };
    let parsed = match syn::parse_file(&text) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("parse {}: {e}", file.display());
            std::process::exit(2);
        }
    };
    let shown = file
        .strip_prefix(repo)
        .unwrap_or(file)
        .display()
        .to_string();
    let mut scan = Scan {
        file: shown,
        declares,
        in_test: is_test_file,
        data,
    };
    scan.visit_file(&parsed);
}

/// One declaration of a field name: which struct, where, and how visible.
struct Decl {
    owner: String,
    file: String,
    line: usize,
    vis: &'static str,
    derives: String,
}

#[derive(Default)]
struct Data {
    decls: BTreeMap<String, Vec<Decl>>,
    /// Production write sites, `file:line`, per field name.
    writes: BTreeMap<String, Vec<String>>,
    reads: BTreeMap<String, usize>,
    test_reads: BTreeMap<String, usize>,
}

struct Scan<'a> {
    file: String,
    declares: bool,
    in_test: bool,
    data: &'a mut Data,
}

impl Scan<'_> {
    fn write(&mut self, ident: &proc_macro2::Ident) {
        if self.in_test {
            return;
        }
        let at = format!("{}:{}", self.file, ident.span().start().line);
        self.data
            .writes
            .entry(ident.to_string())
            .or_default()
            .push(at);
    }

    fn read(&mut self, name: String) {
        let bucket = if self.in_test {
            &mut self.data.test_reads
        } else {
            &mut self.data.reads
        };
        *bucket.entry(name).or_default() += 1;
    }

    /// The left-hand side of an assignment: the outermost named field is the write, everything the
    /// path walks through to reach it (`a.b.c = 1` reaches `c` through `a.b`) is an ordinary read.
    fn assign_lhs(&mut self, e: &syn::Expr) {
        match e {
            syn::Expr::Field(f) => {
                if let syn::Member::Named(id) = &f.member {
                    self.write(id);
                }
                self.visit_expr(&f.base);
            }
            syn::Expr::Index(ix) => {
                self.assign_lhs(&ix.expr);
                self.visit_expr(&ix.index);
            }
            syn::Expr::Paren(p) => self.assign_lhs(&p.expr),
            syn::Expr::Group(g) => self.assign_lhs(&g.expr),
            syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => self.assign_lhs(&u.expr),
            syn::Expr::Tuple(t) => {
                for elem in &t.elems {
                    self.assign_lhs(elem);
                }
            }
            other => self.visit_expr(other),
        }
    }

    /// Macro bodies are opaque token soup to `syn`, so `write!(f, "{}", npc.damage_bonus)` would
    /// otherwise be invisible. Every `. ident` pair inside one counts as a read: macro tokens carry
    /// no read/write distinction, and the safe direction is to assume a consumer.
    fn scan_tokens(&mut self, tokens: TokenStream) {
        let mut after_dot = false;
        for tree in tokens {
            match tree {
                TokenTree::Punct(p) if p.as_char() == '.' => {
                    after_dot = true;
                    continue;
                }
                TokenTree::Ident(id) if after_dot => self.read(id.to_string()),
                TokenTree::Group(g) => self.scan_tokens(g.stream()),
                _ => {}
            }
            after_dot = false;
        }
    }
}

/// A `#[test]`, a `#[tokio::test]`, or any `#[cfg(...)]` mentioning `test`.
fn is_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        let path = a.path();
        if path.segments.last().is_some_and(|s| s.ident == "test") {
            return true;
        }
        if path.is_ident("cfg")
            && let syn::Meta::List(list) = &a.meta
        {
            return format!("{}", list.tokens).contains("test");
        }
        false
    })
}

fn derives(attrs: &[syn::Attribute]) -> String {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for a in attrs {
        if a.path().is_ident("derive")
            && let syn::Meta::List(list) = &a.meta
        {
            for tree in list.tokens.clone() {
                if let TokenTree::Ident(id) = tree {
                    names.insert(id.to_string());
                }
            }
        }
    }
    names.into_iter().collect::<Vec<_>>().join(", ")
}

impl<'ast> Visit<'ast> for Scan<'_> {
    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        let was = self.in_test;
        self.in_test |= is_test(&i.attrs);
        visit::visit_item_mod(self, i);
        self.in_test = was;
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        let was = self.in_test;
        self.in_test |= is_test(&i.attrs);
        visit::visit_item_fn(self, i);
        self.in_test = was;
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        let was = self.in_test;
        self.in_test |= is_test(&i.attrs);
        visit::visit_impl_item_fn(self, i);
        self.in_test = was;
    }

    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        if self.declares && !self.in_test && !is_test(&i.attrs) {
            let derives = derives(&i.attrs);
            for field in &i.fields {
                let Some(ident) = &field.ident else { continue };
                self.data
                    .decls
                    .entry(ident.to_string())
                    .or_default()
                    .push(Decl {
                        owner: i.ident.to_string(),
                        file: self.file.clone(),
                        line: ident.span().start().line,
                        vis: if matches!(field.vis, syn::Visibility::Inherited) {
                            "private"
                        } else {
                            "pub"
                        },
                        derives: derives.clone(),
                    });
            }
        }
        visit::visit_item_struct(self, i);
    }

    fn visit_expr_assign(&mut self, i: &'ast syn::ExprAssign) {
        self.assign_lhs(&i.left);
        self.visit_expr(&i.right);
    }

    fn visit_expr_binary(&mut self, i: &'ast syn::ExprBinary) {
        // `x.a += 1` is a write, not a read: a field that only ever feeds its own update has no
        // consumer either. Reading it here would hide exactly the counters this lint is for.
        let compound = matches!(
            i.op,
            syn::BinOp::AddAssign(_)
                | syn::BinOp::SubAssign(_)
                | syn::BinOp::MulAssign(_)
                | syn::BinOp::DivAssign(_)
                | syn::BinOp::RemAssign(_)
                | syn::BinOp::BitXorAssign(_)
                | syn::BinOp::BitAndAssign(_)
                | syn::BinOp::BitOrAssign(_)
                | syn::BinOp::ShlAssign(_)
                | syn::BinOp::ShrAssign(_)
        );
        if compound {
            self.assign_lhs(&i.left);
        } else {
            self.visit_expr(&i.left);
        }
        self.visit_expr(&i.right);
    }

    fn visit_expr_field(&mut self, i: &'ast syn::ExprField) {
        if let syn::Member::Named(id) = &i.member {
            self.read(id.to_string());
        }
        self.visit_expr(&i.base);
    }

    /// `Foo { bar: value }` and its `Foo { bar }` shorthand are both writes of `bar`.
    fn visit_field_value(&mut self, i: &'ast syn::FieldValue) {
        if let syn::Member::Named(id) = &i.member {
            self.write(id);
        }
        self.visit_expr(&i.expr);
    }

    /// Destructuring, `let Foo { bar, .. } = x`, is a read.
    fn visit_field_pat(&mut self, i: &'ast syn::FieldPat) {
        if let syn::Member::Named(id) = &i.member {
            self.read(id.to_string());
        }
        self.visit_pat(&i.pat);
    }

    fn visit_macro(&mut self, i: &'ast syn::Macro) {
        self.scan_tokens(i.tokens.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyse(source: &str) -> Data {
        let mut data = Data::default();
        let parsed = syn::parse_file(source).expect("test source parses");
        let mut scan = Scan {
            file: "test.rs".to_owned(),
            declares: true,
            in_test: false,
            data: &mut data,
        };
        scan.visit_file(&parsed);
        data
    }

    /// The one thing this lint exists to catch, and the one `rustc` cannot: a field the tests read
    /// and production only writes. `kept` is the control, read by real code; `inert` is the bug.
    #[test]
    fn a_field_read_only_by_a_test_is_a_dead_write() {
        let data = analyse(
            r"
            struct Boss { kept: f32, inert: f32 }
            fn enrage(b: &mut Boss) { b.kept = 2.0; b.inert = 2.0; }
            fn hurt(b: &Boss) -> f32 { b.kept }

            #[cfg(test)]
            mod tests {
                #[test]
                fn it_enrages() {
                    let mut b = Boss { kept: 1.0, inert: 1.0 };
                    enrage(&mut b);
                    assert_eq!(b.inert, 2.0);
                }
            }
            ",
        );
        assert!(data.writes.contains_key("inert"), "the write is seen");
        assert!(
            !data.reads.contains_key("inert"),
            "a #[cfg(test)] read is not a production read"
        );
        assert_eq!(
            data.test_reads.get("inert"),
            Some(&1),
            "counted as a test read"
        );
        assert!(
            data.reads.contains_key("kept"),
            "an ordinary read is a production read"
        );
        assert_eq!(data.decls["inert"][0].owner, "Boss");
    }

    /// The three shapes that are easy to get backwards: a compound assignment is a write and not a
    /// read, a struct literal field is a write, and a field named inside a macro is a read.
    #[test]
    fn compound_assignment_struct_literals_and_macros() {
        let data = analyse(
            r#"
            struct S { counter: u32, built: u32, logged: u32 }
            fn go(s: &mut S) {
                s.counter += 1;
                let _ = S { built: 0, counter: 0, logged: 0 };
                println!("{}", s.logged);
            }
            "#,
        );
        assert!(data.writes.contains_key("counter"));
        assert!(
            !data.reads.contains_key("counter"),
            "`+=` feeding only itself is not a consumer"
        );
        assert!(
            data.writes.contains_key("built"),
            "struct literal is a write"
        );
        assert!(
            data.reads.contains_key("logged"),
            "a macro mention is a read"
        );
    }
}
