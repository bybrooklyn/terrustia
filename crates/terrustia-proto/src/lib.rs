#![forbid(unsafe_code)]
//! Terraria network wire format.
//!
//! This crate is deliberately free of I/O so that every packet can be round-tripped in a unit test
//! without a socket. The async server lives in the `terrustia` crate.
//!
//! Copyright (c) 2026 Brooklyn Halmstad.
//! Licensed under the MIT licence; see the LICENSE file beside this crate's manifest. The server
//! that uses it is AGPL, but this crate is not: it is a description of a wire format, and holding
//! that hostage to a licence would help nobody.

pub mod angler;
pub mod banners;
pub mod buffs;
pub mod conditional_drops;
pub mod convert;
pub mod difficulty;
pub mod error;
pub mod happiness;
pub mod housing;
pub mod hurt;
pub mod hurt_tiles;
pub mod id;
pub mod inventory;
pub mod item;
pub mod items;
pub mod locks;
pub mod luck;
pub mod net_module;
pub mod net_text;
pub mod net_variants;
pub mod npc;
pub mod npc_data;
pub mod npc_drops;
pub mod npc_params;
pub mod objects;
pub mod orbs;
pub mod packets;
pub mod placed_items;
pub mod player_info;
pub mod prehardmode;
pub mod projectile;
pub mod projectile_data;
pub mod reader;
pub mod recipes;
pub mod section;
pub mod shimmer;
pub mod square;
pub mod statues;
pub mod tile;
pub mod tile_death;
pub mod tile_drops;
pub mod tile_entity;
pub mod tile_object;
pub mod tile_sets;
pub mod tile_solid;
pub mod touch_debuffs;
pub mod town_names;
pub mod travel_shop;
pub mod writer;

pub use error::{ProtoError, Result};
pub use item::ItemStack;
pub use net_text::{NetworkText, TextMode};
pub use reader::PacketReader;
pub use section::{SECTION_HEIGHT, SECTION_WIDTH, SectionBounds, SectionExtras};
pub use tile::{Liquid, Tile, TileFlags};
pub use writer::{MAX_FRAME_LEN, PacketWriter, Writer};
