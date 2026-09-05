//! The data-table generator: reads a decompiled Terraria tree and writes the extracted tables in
//! `crates/terrustia-proto/src/`. One binary replacing the old `tools/gen_*.py` scripts.
//!
//! ```text
//! codegen <table|all> <decompiled-root> [out.rs]
//! ```
//!
//! `<table>` is one of the names in [`TABLES`], or `all` to regenerate every one into its own
//! default path. `<decompiled-root>` is the directory ilspycmd produced (the same `.scratch/
//! decompiled` the old scripts took). An explicit `out.rs` overrides the default path for a single
//! table. Run `just regen` to do them all and `cargo fmt` afterward, exactly as before.

mod angler;
mod banners;
mod buffs;
mod csharp;
mod drops;
mod hurt_tiles;
mod net_variants;
mod projectiles;
mod recipes;
mod shimmer;
mod tile_death;
mod tile_object;
mod town_names;
mod travel_shop;

use std::path::{Path, PathBuf};

/// One extractable table: its name on the command line, the file it writes by default, and the
/// function that turns a decompiled tree into that file's contents.
struct Table {
    name: &'static str,
    out: &'static str,
    generate: fn(&Path) -> String,
}

const TABLES: &[Table] = &[
    Table {
        name: "hurt_tiles",
        out: "crates/terrustia-proto/src/hurt_tiles.rs",
        generate: hurt_tiles::generate,
    },
    Table {
        name: "recipes",
        out: "crates/terrustia-proto/src/recipes.rs",
        generate: recipes::generate,
    },
    Table {
        name: "drops",
        out: "crates/terrustia-proto/src/npc_drops.rs",
        generate: drops::generate,
    },
    Table {
        name: "projectiles",
        out: "crates/terrustia-proto/src/projectile_data.rs",
        generate: projectiles::generate,
    },
    Table {
        name: "net_variants",
        out: "crates/terrustia-proto/src/net_variants.rs",
        generate: net_variants::generate,
    },
    Table {
        name: "banners",
        out: "crates/terrustia-proto/src/banners.rs",
        generate: banners::generate,
    },
    Table {
        name: "buffs",
        out: "crates/terrustia-proto/src/buffs.rs",
        generate: buffs::generate,
    },
    Table {
        name: "angler",
        out: "crates/terrustia-proto/src/angler.rs",
        generate: angler::generate,
    },
    Table {
        name: "town_names",
        out: "crates/terrustia-proto/src/town_names.rs",
        generate: town_names::generate,
    },
    Table {
        name: "shimmer",
        out: "crates/terrustia-proto/src/shimmer.rs",
        generate: shimmer::generate,
    },
    Table {
        name: "travel_shop",
        out: "crates/terrustia-proto/src/travel_shop.rs",
        generate: travel_shop::generate,
    },
    Table {
        name: "tile_death",
        out: "crates/terrustia-proto/src/tile_death.rs",
        generate: tile_death::generate,
    },
    Table {
        name: "tile_object",
        out: "crates/terrustia-proto/src/tile_object.rs",
        generate: tile_object::generate,
    },
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: codegen <table|all> <decompiled-root> [out.rs]");
        eprintln!(
            "tables: {}",
            TABLES.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        );
        std::process::exit(2);
    }
    let which = &args[1];
    let root = PathBuf::from(&args[2]);

    let run = |table: &Table, out: &Path| {
        let content = (table.generate)(&root);
        if let Err(e) = std::fs::write(out, content) {
            eprintln!("write {}: {e}", out.display());
            std::process::exit(1);
        }
        eprintln!("wrote {}", out.display());
    };

    if which == "all" {
        for table in TABLES {
            run(table, Path::new(table.out));
        }
    } else if let Some(table) = TABLES.iter().find(|t| t.name == *which) {
        let out = args
            .get(3)
            .map_or_else(|| PathBuf::from(table.out), PathBuf::from);
        run(table, &out);
    } else {
        eprintln!("unknown table {which:?}");
        std::process::exit(2);
    }
}
