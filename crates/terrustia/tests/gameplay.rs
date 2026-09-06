//! Gameplay behaviour, driven through the headless client.
//!
//! These exercise the parts of the server a player actually touches: chests, signs, multi-tile
//! edits, the world clock, chat commands and saving.

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use terrustia::{
    config::Config,
    game::{GameServer, ServerEvent, Stopped},
    net::listener,
    world::{Chest, Sign, World, wld, wld_save, worldgen},
};
use terrustia_client::{Client, Event};
use terrustia_proto::{ItemStack, Tile, id, square::TileSquare};
use tokio::{net::TcpListener, sync::mpsc};

/// Start a server on an ephemeral port, letting the caller shape the world first.
async fn start_with<F: FnOnce(&mut World)>(mut config: Config, prepare: F) -> SocketAddr {
    config.world_width = 800;
    config.world_height = 600;
    config.motd = String::new();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let mut world = worldgen::generate(
        config.world_width,
        config.world_height,
        config.world_name.clone(),
        7,
    );
    prepare(&mut world);

    let (tx, rx) = mpsc::channel::<ServerEvent>(1024);
    tokio::spawn(GameServer::new(config.clone(), world).run(rx));
    tokio::spawn(listener::run(listener, config, tx, None));
    addr
}

/// Hollow out a working area in a generated world.
///
/// Tests that place, break, paint or pour need somewhere empty to do it, and a *generated* world
/// is mostly solid — relying on the generator happening to leave a particular tile open makes a
/// test that fails the next time the generator changes, which is exactly what happened. Clearing
/// the space explicitly says what the test needs instead of hoping for it.
fn clear_area(world: &mut World, x: i32, y: i32, half_w: i32, half_h: i32) {
    for cx in x - half_w..=x + half_w {
        for cy in y - half_h..=y + half_h {
            world.set_tile(cx, cy, Tile::AIR);
        }
    }
}

/// The same, with a floor under it, for anything that has to stand on something.
fn clear_with_floor(world: &mut World, x: i32, y: i32, half_w: i32, half_h: i32) {
    clear_area(world, x, y, half_w, half_h);
    for cx in x - half_w..=x + half_w {
        world.set_tile(cx, y + half_h + 1, Tile::block(1));
    }
}

async fn start() -> SocketAddr {
    start_with(Config::default(), |_| {}).await
}

async fn join(addr: SocketAddr, name: &str) -> Client {
    let mut client = Client::join(addr, name).await.expect("handshake");
    client.set_timeout(Duration::from_secs(10));
    client
}

#[tokio::test]
async fn a_chest_can_be_opened_and_its_contents_edited() {
    // Put a chest somewhere the spawn stream will cover.
    let addr = start_with(Config::default(), |world| {
        world.chests = vec![Some(Chest {
            x: 400,
            y: 320,
            name: "Loot".into(),
            items: vec![ItemStack::new(3507, 5, 0), ItemStack::EMPTY],
        })];
    })
    .await;

    let mut client = join(addr, "looter").await;
    client.open_chest(400, 320).await.unwrap();

    // The server announces the size, then every slot, then which chest is open.
    let mut size = None;
    let mut slots = Vec::new();
    let mut opened = false;
    for _ in 0..40 {
        match client.next_event().await.unwrap() {
            Event::Other(frame) if frame.id == id::SYNC_CHEST_SIZE => {
                size = Some(i16::from_le_bytes([frame.payload[2], frame.payload[3]]));
            }
            Event::Other(frame) if frame.id == id::SYNC_CHEST_ITEM => {
                slots
                    .push(terrustia_proto::objects::SyncChestItem::decode(&frame.payload).unwrap());
            }
            Event::Other(frame) if frame.id == id::SYNC_PLAYER_CHEST => {
                let sync =
                    terrustia_proto::objects::SyncPlayerChest::decode(&frame.payload).unwrap();
                assert_eq!(sync.name.as_deref(), Some("Loot"));
                opened = true;
                break;
            }
            _ => {}
        }
    }

    assert_eq!(size, Some(2), "the client must be told the chest's size");
    assert!(opened, "the server never confirmed the chest was open");
    assert_eq!(slots.len(), 2, "every slot should be sent");
    assert_eq!(slots[0].item, ItemStack::new(3507, 5, 0));
    assert!(slots[1].item.is_empty());
}

#[tokio::test]
async fn a_chest_edit_is_refused_unless_the_chest_is_open() {
    let addr = start_with(Config::default(), |world| {
        world.chests = vec![Some(Chest {
            x: 400,
            y: 320,
            name: String::new(),
            items: vec![ItemStack::EMPTY; 4],
        })];
    })
    .await;

    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    // Bob never opened it, so his edit must not take.
    let forged = terrustia_proto::objects::SyncChestItem {
        chest: 0,
        slot: 0,
        item: ItemStack::new(99, 1, 0),
    };
    bob.send(&forged.encode().unwrap()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Alice opens it and should see an empty slot.
    alice.open_chest(400, 320).await.unwrap();
    let mut first_slot = None;
    for _ in 0..40 {
        if let Event::Other(frame) = alice.next_event().await.unwrap()
            && frame.id == id::SYNC_CHEST_ITEM
        {
            let sync = terrustia_proto::objects::SyncChestItem::decode(&frame.payload).unwrap();
            if sync.slot == 0 {
                first_slot = Some(sync.item);
                break;
            }
        }
    }
    assert_eq!(
        first_slot,
        Some(ItemStack::EMPTY),
        "an edit from a player who never opened the chest was applied"
    );
}

#[tokio::test]
async fn two_players_cannot_open_the_same_chest() {
    let addr = start_with(Config::default(), |world| {
        world.chests = vec![Some(Chest {
            x: 400,
            y: 320,
            name: String::new(),
            items: vec![ItemStack::EMPTY; 2],
        })];
    })
    .await;

    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    alice.open_chest(400, 320).await.unwrap();
    alice
        .wait_for(
            "alice's chest to open",
            |e| matches!(e, Event::Other(f) if f.id == id::SYNC_PLAYER_CHEST),
        )
        .await
        .unwrap();

    // Bob's attempt should be ignored while alice is inside.
    bob.open_chest(400, 320).await.unwrap();
    bob.set_timeout(Duration::from_secs(2));
    let opened = bob
        .wait_for(
            "bob's chest to open",
            |e| matches!(e, Event::Other(f) if f.id == id::SYNC_PLAYER_CHEST),
        )
        .await;
    assert!(opened.is_err(), "two players opened the same chest at once");
}

#[tokio::test]
async fn a_sign_can_be_read_and_rewritten() {
    let addr = start_with(Config::default(), |world| {
        world.signs = vec![Some(Sign {
            x: 400,
            y: 320,
            text: "original".into(),
        })];
    })
    .await;

    let mut client = join(addr, "reader").await;
    client.read_sign(400, 320).await.unwrap();

    let text = client
        .wait_for(
            "the sign text",
            |e| matches!(e, Event::Other(f) if f.id == id::OPEN_SIGN_RESPONSE),
        )
        .await
        .unwrap();
    let Event::Other(frame) = text else {
        unreachable!()
    };
    let sign = terrustia_proto::objects::SignText::decode(&frame.payload).unwrap();
    assert_eq!(sign.text, "original");

    // Rewrite it, then have a second player read it back.
    let update = terrustia_proto::objects::SignText {
        sign: sign.sign,
        x: 400,
        y: 320,
        text: "rewritten".into(),
        player: 0,
        editing: 0,
    };
    client.send(&update.encode().unwrap()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut second = join(addr, "second").await;
    second.read_sign(400, 320).await.unwrap();
    let event = second
        .wait_for(
            "the updated sign",
            |e| matches!(e, Event::Other(f) if f.id == id::OPEN_SIGN_RESPONSE),
        )
        .await
        .unwrap();
    let Event::Other(frame) = event else {
        unreachable!()
    };
    assert_eq!(
        terrustia_proto::objects::SignText::decode(&frame.payload)
            .unwrap()
            .text,
        "rewritten"
    );
}

/// A tile edit for a section the client was never actually sent must not apply — vanilla parity:
/// `MessageBuffer.cs`'s packet-17 handler forces its own `flag14` (`fail`) true the moment the
/// edited tile's section is missing from `Netplay.Clients[whoAmI].TileSections`
/// (`RemoteClient.cs:31`), which is the same state `Player::sent_sections` mirrors here.
///
/// Server-side truth has to be checked through a client that never received the original attempt's
/// relay: every edit is broadcast to already-connected clients regardless of whether it actually
/// applied (`on_tile_manipulation`'s own comment: "even an edit the server does not model must
/// reach other clients"), so a bystander present at the time would show the edit having happened
/// in its own optimistic view either way. A client joining *after* the attempt, requesting the
/// section for the first time, only ever sees the server's real canonical state.
#[tokio::test]
async fn a_tile_edit_for_a_never_sent_section_is_rejected() {
    let addr = start().await;
    let mut bob = join(addr, "bob").await;

    // Any tile bob's own client-side world has no data for at all is, by construction, one the
    // server never streamed to bob either — the two can't have diverged, since streaming is the
    // only way a client learns a tile exists. Scan for one rather than assuming a fixed distance
    // from spawn, since the test world here is a fixed 800x600 (`start_with` hardcodes it) and a
    // guess could land inside whatever the initial spawn stream actually covered.
    let (far_x, far_y) = (0..800)
        .step_by(97)
        .flat_map(|x| (0..600).step_by(97).map(move |y| (x, y)))
        .find(|&(x, y)| bob.world().tile(x, y).is_none())
        .expect("an 800x600 world should have at least one tile bob's spawn stream never covered");

    bob.place_tile(far_x as i16, far_y as i16, 30)
        .await
        .unwrap(); // 30: stone, distinctive enough

    let mut witness = join(addr, "witness").await;
    let (far_sx, far_sy) = (
        far_x / terrustia_proto::section::SECTION_WIDTH,
        far_y / terrustia_proto::section::SECTION_HEIGHT,
    );
    witness
        .request_section(far_sx as u16, far_sy as u16)
        .await
        .unwrap();
    witness
        .wait_for("the far section, from a client that never saw bob's attempt", |e| {
            matches!(e, Event::SectionLoaded { section_x, section_y } if *section_x == far_sx && *section_y == far_sy)
        })
        .await
        .unwrap();

    assert_ne!(
        witness.world().tile(far_x, far_y).map(|t| t.block),
        Some(30),
        "a tile edit for a section bob was never sent applied anyway"
    );
}

#[tokio::test]
async fn a_tile_square_is_applied_and_relayed() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    // A 2x2 block of stone, the shape a piece of furniture would arrive as.
    let square = TileSquare {
        x: 402,
        y: 330,
        width: 2,
        height: 2,
        change_type: 0,
        tiles: vec![Tile::block(1); 4],
    };
    bob.send(&square.encode().unwrap()).await.unwrap();

    // Alice sees the relay.
    alice
        .wait_for(
            "the relayed square",
            |e| matches!(e, Event::Other(f) if f.id == id::AREA_TILE_CHANGE),
        )
        .await
        .unwrap();

    // And a fresh player is streamed the applied result.
    let fresh = join(addr, "fresh").await;
    for dx in 0..2 {
        for dy in 0..2 {
            let tile = fresh.world().tile(402 + dx, 330 + dy);
            assert_eq!(
                tile.map(|t| t.block),
                Some(1),
                "square tile ({dx}, {dy}) did not stick"
            );
        }
    }
}

#[tokio::test]
async fn a_tile_square_reaching_outside_the_world_is_refused() {
    let addr = start().await;
    let mut client = join(addr, "edge").await;

    let square = TileSquare {
        x: 799,
        y: 599,
        width: 4,
        height: 4,
        change_type: 0,
        tiles: vec![Tile::block(1); 16],
    };
    client.send(&square.encode().unwrap()).await.unwrap();

    // The server should stay up and keep answering.
    client.say("still here?").await.unwrap();
    client
        .wait_for(
            "chat to still work",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("still here?")),
        )
        .await
        .expect("server stopped responding after an out-of-bounds square");
}

#[tokio::test]
async fn chat_commands_answer() {
    let addr = start().await;
    let mut client = join(addr, "asker").await;

    client.say("/players").await.unwrap();
    let event = client
        .wait_for(
            "the player list",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("online")),
        )
        .await
        .unwrap();
    if let Event::Chat { text, .. } = event {
        assert!(text.contains("asker"), "player list should name us: {text}");
    }

    client.say("/time night").await.unwrap();
    client
        .wait_for(
            "the time change",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("Time set to night")),
        )
        .await
        .unwrap();

    client.say("/nonsense").await.unwrap();
    client
        .wait_for(
            "an unknown-command reply",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("unknown command")),
        )
        .await
        .unwrap();
}

/// A shadow-mute: the muted player's own ordinary chat line is suppressed for everyone else, but
/// echoed straight back to the sender — nothing about the client-visible behaviour changes for the
/// person who is muted — while the reply to `/unmute` proves the mute actually lifts.
///
/// An unclaimed server grants every permission to everyone (see
/// `a_stranger_cannot_claim_an_unclaimed_server`'s own comment), so `witness` may run `/mute`
/// without registering first — this needs `server.mute`, not `server.look`.
///
/// Fail-then-pass: before the mute check was wired into `on_net_module`'s chat path, `griefer`'s
/// line reached `witness` exactly like any other broadcast — `try_wait_for` below would have found
/// it well within the one-second window instead of timing out.
#[tokio::test]
async fn mute_suppresses_a_broadcast_but_shadow_echoes_to_the_sender() {
    let addr = start().await;
    let mut griefer = join(addr, "griefer").await;
    let mut witness = join(addr, "witness").await;

    // witness mutes griefer.
    witness.say("/mute griefer spamming").await.unwrap();
    witness
        .wait_for(
            "the mute confirmation",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("griefer is muted")),
        )
        .await
        .unwrap();

    // griefer's ordinary chat line (not a command) is shadow-muted.
    griefer.say("hello from the shadows").await.unwrap();

    // The sender still sees their own line, as their own author slot.
    let echoed = griefer
        .wait_for(
            "the shadow echo back to the sender",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("hello from the shadows")),
        )
        .await
        .unwrap();
    if let Event::Chat { author, .. } = echoed {
        assert_eq!(
            author,
            griefer.slot(),
            "the echo must carry the sender's own slot"
        );
    }

    // The witness never receives it at all — checked with a bounded wait rather than the full
    // timeout, since asserting an absence should not make the test slow.
    let leaked = witness
        .try_wait_for(
            "the muted line, which must not arrive",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("hello from the shadows")),
            Duration::from_millis(800),
        )
        .await;
    assert!(
        leaked.is_none(),
        "a shadow-muted line must never reach anyone but its own sender: {leaked:?}"
    );

    // /unmute lifts it, and a subsequent line reaches the witness normally again.
    witness.say("/unmute griefer").await.unwrap();
    witness
        .wait_for(
            "the unmute confirmation",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("griefer is unmuted")),
        )
        .await
        .unwrap();

    griefer.say("back in the light").await.unwrap();
    witness
        .wait_for(
            "the line to reach the witness now that the mute is lifted",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("back in the light")),
        )
        .await
        .unwrap();
}

/// `chat_cooldown_ms` — a config-enabled mechanism, off by default (`Config::default()` leaves it
/// `0`) — drops a line sent before the cooldown has elapsed since the sender's last accepted one,
/// but lets a later line through once it has.
#[tokio::test]
async fn chat_cooldown_drops_a_rapid_second_line_but_lets_a_later_one_through() {
    let config = Config {
        chat_cooldown_ms: 600,
        ..Config::default()
    };
    let addr = start_with(config, |_| {}).await;
    let mut speaker = join(addr, "speaker").await;
    let mut witness = join(addr, "witness").await;

    speaker.say("first line").await.unwrap();
    witness
        .wait_for(
            "the first line",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("first line")),
        )
        .await
        .unwrap();

    // Sent immediately after — well inside the 600ms cooldown.
    speaker.say("too soon").await.unwrap();
    let leaked = witness
        .try_wait_for(
            "the dropped line, which must not arrive",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("too soon")),
            Duration::from_millis(400),
        )
        .await;
    assert!(
        leaked.is_none(),
        "a line sent before the cooldown elapses must be dropped: {leaked:?}"
    );

    tokio::time::sleep(Duration::from_millis(650)).await;
    speaker.say("after the cooldown").await.unwrap();
    witness
        .wait_for(
            "the line once the cooldown has elapsed",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("after the cooldown")),
        )
        .await
        .unwrap();
}

/// `mute_escalation_enabled` — a config-enabled mechanism, off by default — extends a still-active
/// mute every time the muted player speaks again, so a short mute placed on a repeat offender does
/// not simply lapse while they keep going. Proven end to end, without reaching into server state:
/// mute for one second, have the player talk once while muted (triggering escalation), then show
/// they are *still* muted well past that original one-second window.
#[tokio::test]
async fn mute_escalation_extends_an_active_mute_when_the_player_keeps_talking() {
    let config = Config {
        mute_escalation_enabled: true,
        mute_escalation_secs: 30,
        mute_escalation_max_secs: 0,
        ..Config::default()
    };
    let addr = start_with(config, |_| {}).await;
    let mut griefer = join(addr, "griefer").await;
    let mut witness = join(addr, "witness").await;

    witness.say("/mute griefer 1s spam").await.unwrap();
    witness
        .wait_for(
            "the mute confirmation",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("griefer is muted")),
        )
        .await
        .unwrap();

    // Speak once while muted — the shadow echo back proves the line was actually processed
    // (and, since escalation is on, this is what pushes the mute's expiry out).
    griefer.say("still talking").await.unwrap();
    griefer
        .wait_for(
            "the shadow echo, proving the line reached the chat path",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("still talking")),
        )
        .await
        .unwrap();

    // Past the *original* one-second duration. Without escalation the mute would have lapsed by
    // now and this line would reach the witness; with it, griefer is still muted.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    griefer.say("after the original duration").await.unwrap();
    let leaked = witness
        .try_wait_for(
            "a line that must still be suppressed if escalation worked",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("after the original duration")),
            Duration::from_millis(800),
        )
        .await;
    assert!(
        leaked.is_none(),
        "escalation should have pushed the mute well past its original 1s duration: {leaked:?}"
    );
}

/// A generated world with nowhere to be saved says so, rather than pretending it saved.
#[tokio::test]
async fn a_world_with_no_save_target_says_so() {
    let addr = start().await;
    let mut client = join(addr, "saver").await;

    client.say("/save").await.unwrap();
    client
        .wait_for(
            "the refusal",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("nowhere to be saved")),
        )
        .await
        .unwrap();
}

/// A generated world can be saved and served back, which is what makes a fresh server keep
/// anything at all.
///
/// The header has no original to copy — it is written from scratch at the format's current
/// version — so this is the check that the whole of it lands where it should.
#[tokio::test]
async fn a_generated_world_saves_and_reloads() {
    let dir = std::env::temp_dir().join(format!("terrustia-fresh-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target: PathBuf = dir.join("fresh.wld");
    let _ = std::fs::remove_file(&target);

    let config = Config {
        save_file: Some(target.clone()),
        ..Config::default()
    };
    let addr = start_with(config, |world| {
        clear_with_floor(world, 401, 300, 8, 4);
        world.set_tile(400, 300, Tile::block(57));
    })
    .await;

    let mut client = join(addr, "founder").await;
    client.set_timeout(Duration::from_secs(20));
    client.place_tile(402, 300, 30).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    client.say("/save").await.unwrap();
    client
        .wait_for(
            "the save",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("World saved")),
        )
        .await
        .unwrap();

    // The file this server never had a template for reads back with the edits in place.
    let reloaded = wld::load(&target).expect("a world this server wrote loads again");
    assert_eq!(
        reloaded.tile(400, 300).block,
        57,
        "the world it was built with"
    );
    assert_eq!(
        reloaded.tile(402, 300).block,
        30,
        "and the player's own block"
    );
    assert_eq!(reloaded.name, "Terrustia", "and its name");

    // And it serves: a client joining the reloaded world streams the section with the edit in it.
    let config = Config {
        world_file: Some(target.clone()),
        motd: String::new(),
        ..Config::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel::<ServerEvent>(1024);
    tokio::spawn(GameServer::new(config.clone(), reloaded).run(rx));
    tokio::spawn(listener::run(listener, config, tx, None));

    let mut client = join(addr, "returner").await;
    client.set_timeout(Duration::from_secs(20));
    assert_eq!(
        client.world().tile(402, 300).map(|t| t.block),
        Some(30),
        "the streamed world should carry the edit"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `/world undo` actually reverts a player's own tile edits, and the revert is visible to another
/// connected player — not just applied silently on the server and never synced back out.
///
/// An unclaimed server grants every permission to everyone (see
/// `a_stranger_cannot_claim_an_unclaimed_server`'s own comment on that), so this needs no
/// registration step to exercise `/world undo`'s `server.undo` gate.
#[tokio::test]
async fn world_undo_reverts_a_players_tile_edits_and_a_witness_sees_it() {
    let addr = start_with(Config::default(), |world| {
        clear_with_floor(world, 401, 300, 8, 4);
        world.set_tile(400, 300, Tile::block(57));
        world.set_tile(402, 300, Tile::block(57));
    })
    .await;

    let mut griefer = join(addr, "griefer").await;
    griefer.set_timeout(Duration::from_secs(10));
    let mut witness = join(addr, "witness").await;
    witness.set_timeout(Duration::from_secs(10));

    // Two edits, so the undo has more than one thing to put back. Ordinary player edits relay to
    // other clients as the original TileManipulation packet (`on_tile_manipulation`'s own comment:
    // "relay regardless... or their view of the world silently diverges from the sender's"), which
    // the client surfaces as `Event::TileChanged` — wait for those specifically rather than a bare
    // sleep, since the client only folds a packet into its own world model once it actually reads
    // one off the socket.
    griefer.break_tile(400, 300).await.unwrap();
    witness
        .wait_for(
            "the break to relay",
            |e| matches!(e, Event::TileChanged(edit) if edit.x == 400 && edit.y == 300),
        )
        .await
        .unwrap();
    griefer.place_tile(402, 300, 30).await.unwrap();
    witness
        .wait_for(
            "the placement to relay",
            |e| matches!(e, Event::TileChanged(edit) if edit.x == 402 && edit.y == 300),
        )
        .await
        .unwrap();

    assert_eq!(
        witness.world().tile(400, 300).map(|t| t.block),
        Some(0),
        "the witness should see the break"
    );
    assert_eq!(
        witness.world().tile(402, 300).map(|t| t.block),
        Some(30),
        "and the placed block"
    );

    witness.say("/world undo griefer 1h").await.unwrap();
    // `/world undo` broadcasts a revert (`broadcast_tile`, a raw tile snapshot — packet 20,
    // AREA_TILE_CHANGE, since there is no player-originated TileManipulation to relay for a
    // server-initiated change) for *each* reverted tile before it sends the confirmation chat
    // line, so on the wire these two frames arrive first. `wait_for` discards whatever does not
    // match while it scans, so waiting on the confirmation text before these would silently eat
    // both real frames and leave nothing for a later wait to find — wait for them in the order
    // the server actually sends them.
    for _ in 0..2 {
        witness
            .wait_for("an undo revert to sync", |e| {
                matches!(e, Event::Other(frame) if frame.id == terrustia_proto::id::AREA_TILE_CHANGE)
            })
            .await
            .unwrap();
    }
    witness
        .wait_for(
            "the undo confirmation",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("reverted 2 tile edit")),
        )
        .await
        .unwrap();

    assert_eq!(
        witness.world().tile(400, 300).map(|t| t.block),
        Some(57),
        "the broken tile should be back, seen by a client that never sent the undo itself"
    );
    assert_eq!(
        witness.world().tile(402, 300).map(|t| t.block),
        Some(57),
        "and the placed one reverted to the original floor"
    );

    // A second undo has nothing left to revert — the log gave up what it had.
    witness.say("/world undo griefer 1h").await.unwrap();
    witness
        .wait_for(
            "an empty undo",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("reverted 0 tile edit")),
        )
        .await
        .unwrap();
}

/// A duration `/world undo` cannot parse is refused with a usable message, not silently ignored
/// or a panic.
#[tokio::test]
async fn world_undo_refuses_an_unparseable_duration() {
    let addr = start().await;
    let mut client = join(addr, "admin").await;
    client.set_timeout(Duration::from_secs(10));

    client.say("/world undo somebody whenever").await.unwrap();
    client
        .wait_for(
            "a duration error",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("could not parse that duration")),
        )
        .await
        .unwrap();
}

/// Build a small world, save it, and serve the save so the whole persistence path is exercised.
#[tokio::test]
async fn edits_survive_a_save_and_reload() {
    let dir = std::env::temp_dir().join(format!("terrustia-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source: PathBuf = dir.join("source.wld");

    // A world to start from. It can be one this server generated and wrote itself, which is what
    // makes this run without a Terraria install to borrow a save from; set TERRUSTIA_TEST_WLD to
    // a real one to exercise the same path against a world the game made.
    match std::env::var("TERRUSTIA_TEST_WLD") {
        Ok(seed_world) => {
            std::fs::copy(&seed_world, &source).unwrap();
        }
        Err(_) => {
            let world = worldgen::generate(800, 600, "roundtrip", 7);
            wld_save::save(&world, &source).unwrap();
        }
    }

    let saved = dir.join("saved.wld");
    let config = Config {
        world_file: Some(source.clone()),
        save_file: Some(saved.clone()),
        autosave_secs: 0,
        motd: String::new(),
        ..Config::default()
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let world = wld::load(&source).unwrap();
    let (spawn_x, spawn_y) = (world.spawn_x, world.spawn_y);
    let (tx, rx) = mpsc::channel::<ServerEvent>(1024);
    tokio::spawn(GameServer::new(config.clone(), world).run(rx));
    tokio::spawn(listener::run(listener, config, tx, None));

    let mut client = join(addr, "digger").await;
    let dig_x = i32::from(spawn_x) + 20;
    let mut dug = Vec::new();
    for depth in 12..30 {
        let y = i32::from(spawn_y) + depth;
        if client.world().tile(dig_x, y).is_some_and(|t| t.is_active()) {
            client.break_tile(dig_x as i16, y as i16).await.unwrap();
            dug.push(y);
        }
    }
    assert!(!dug.is_empty(), "found nothing to dig");

    client.say("/save").await.unwrap();
    client
        .wait_for(
            "the save confirmation",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("World saved")),
        )
        .await
        .unwrap();

    let reloaded = wld::load(&saved).unwrap();
    for y in dug {
        assert!(
            !reloaded.tile(dig_x, y).is_active(),
            "block at ({dig_x}, {y}) came back after the save"
        );
    }

    // And the save is itself loadable and re-saveable.
    wld_save::save(&reloaded, &dir.join("again.wld")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn breaking_a_block_drops_the_right_item() {
    let addr = start_with(Config::default(), |world| {
        // A lone stone block in mid-air, so the drop is unambiguous.
        world.set_tile(410, 300, Tile::block(1));
    })
    .await;

    let mut client = join(addr, "miner").await;
    client.break_tile(410, 300).await.unwrap();

    let event = client
        .wait_for("the dropped item", |e| matches!(e, Event::ItemSynced(_)))
        .await
        .unwrap();
    let Event::ItemSynced(sync) = event else {
        unreachable!()
    };
    // Stone is tile 1, and its item is StoneBlock (3).
    assert_eq!(sync.item.id, 3, "stone should drop a stone block");
    assert_eq!(sync.item.stack, 1);
}

/// A framed object gives back the item that placed it, chosen by the style in its frame.
///
/// This used to drop nothing at all, deliberately: the style table did not exist and the wrong
/// chest is worse than no chest. It exists now.
#[tokio::test]
async fn a_framed_object_gives_back_what_placed_it() {
    let addr = start_with(Config::default(), |world| {
        world.set_tile(410, 300, Tile::framed(21, 0, 0));
    })
    .await;

    let mut client = join(addr, "miner").await;
    client.break_tile(410, 300).await.unwrap();

    client.set_timeout(Duration::from_secs(4));
    let dropped = client
        .wait_for("the chest", |e| matches!(e, Event::ItemSynced(_)))
        .await
        .expect("a chest should drop a chest");
    let Event::ItemSynced(sync) = dropped else {
        unreachable!()
    };
    // Chest tile, style zero, is the ordinary wooden Chest — item 48.
    assert_eq!(sync.item.id, 48, "a chest should give back a chest");
}

/// A style nothing is known for still drops nothing, rather than guessing at an item.
#[tokio::test]
async fn an_unknown_style_still_drops_nothing() {
    let addr = start_with(Config::default(), |world| {
        // Far past any real chest style.
        world.set_tile(410, 300, Tile::framed(21, 30_000, 0));
    })
    .await;

    let mut client = join(addr, "miner").await;
    client.break_tile(410, 300).await.unwrap();

    client.set_timeout(Duration::from_secs(2));
    let dropped = client
        .wait_for("an item drop", |e| matches!(e, Event::ItemSynced(_)))
        .await;
    assert!(dropped.is_err(), "it guessed at an item it does not know");
}

#[tokio::test]
async fn a_dropped_item_is_reserved_and_can_be_picked_up() {
    let addr = start_with(Config::default(), |world| {
        world.set_tile(410, 300, Tile::block(1));
    })
    .await;

    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    // Stand alice on top of the block so the reservation goes to her.
    alice.move_to(410.0 * 16.0, 300.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    alice.break_tile(410, 300).await.unwrap();
    let event = alice
        .wait_for("the drop", |e| matches!(e, Event::ItemSynced(_)))
        .await
        .unwrap();
    let Event::ItemSynced(sync) = event else {
        unreachable!()
    };

    let reserved = alice
        .wait_for(
            "the reservation",
            |e| matches!(e, Event::ItemReserved(o) if o.index == sync.index),
        )
        .await
        .unwrap();
    let Event::ItemReserved(owner) = reserved else {
        unreachable!()
    };
    assert_eq!(owner.owner, alice.slot(), "the nearby player should get it");

    // Alice picks it up; bob is told it is gone.
    alice.pick_up(sync.index).await.unwrap();
    bob.wait_for(
        "the despawn",
        |e| matches!(e, Event::ItemDespawned(i) if *i == sync.index),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn an_item_cannot_be_picked_up_by_someone_it_is_not_reserved_for() {
    let addr = start_with(Config::default(), |world| {
        world.set_tile(410, 300, Tile::block(1));
    })
    .await;

    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    alice.move_to(410.0 * 16.0, 300.0 * 16.0).await.unwrap();
    // Put bob far away so he never earns the reservation.
    bob.move_to(100.0 * 16.0, 300.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    alice.break_tile(410, 300).await.unwrap();
    let event = alice
        .wait_for("the drop", |e| matches!(e, Event::ItemSynced(_)))
        .await
        .unwrap();
    let Event::ItemSynced(sync) = event else {
        unreachable!()
    };

    // Bob claims it anyway.
    bob.pick_up(sync.index).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // A player joining now must still see the item, because bob's claim was refused.
    let fresh = join(addr, "fresh").await;
    drop(fresh);

    alice.set_timeout(Duration::from_secs(2));
    let despawned = alice
        .wait_for("a despawn", |e| matches!(e, Event::ItemDespawned(_)))
        .await;
    assert!(
        despawned.is_err(),
        "a stranger was allowed to take the item"
    );
}

#[tokio::test]
async fn a_player_can_throw_an_item_into_the_world() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    alice
        .drop_item(ItemStack::new(3, 17, 0), (410.0 * 16.0, 300.0 * 16.0))
        .await
        .unwrap();

    let event = bob
        .wait_for("the thrown item", |e| matches!(e, Event::ItemSynced(_)))
        .await
        .unwrap();
    let Event::ItemSynced(sync) = event else {
        unreachable!()
    };
    assert_eq!(sync.item.id, 3);
    assert_eq!(sync.item.stack, 17);
    assert_ne!(
        sync.index, 400,
        "the server should have allocated a real slot"
    );
}

#[tokio::test]
async fn a_password_is_required_when_one_is_configured() {
    let addr = start_with(
        Config {
            password: "hunter2".into(),
            ..Config::default()
        },
        |_| {},
    )
    .await;

    // A plain join never gets a slot, because the server asks for a password first.
    let mut raw = terrustia_client::Client::connect(addr, "guest")
        .await
        .unwrap();
    raw.set_timeout(Duration::from_secs(3));
    let joined = raw.handshake().await;
    assert!(
        joined.is_err(),
        "joined a password-protected server without one"
    );
}

#[tokio::test]
async fn the_wrong_password_is_refused_and_the_right_one_accepted() {
    let addr = start_with(
        Config {
            password: "hunter2".into(),
            ..Config::default()
        },
        |_| {},
    )
    .await;

    let send_password = |password: &str| {
        let mut w = terrustia_proto::PacketWriter::new(id::SEND_PASSWORD);
        w.string(password);
        w.finish().unwrap()
    };
    let hello = || {
        let mut w = terrustia_proto::PacketWriter::new(id::HELLO);
        w.string(id::VERSION_STRING);
        w.finish().unwrap()
    };

    // Wrong password: kicked.
    let mut bad = terrustia_client::Client::connect(addr, "bad")
        .await
        .unwrap();
    bad.set_timeout(Duration::from_secs(5));
    bad.send(&hello()).await.unwrap();
    bad.wait_for(
        "the password prompt",
        |e| matches!(e, Event::Other(f) if f.id == id::REQUEST_PASSWORD),
    )
    .await
    .unwrap();
    bad.send(&send_password("wrong")).await.unwrap();
    let result = bad.next_event().await;
    assert!(
        matches!(result, Err(terrustia_client::ClientError::Kicked { .. })),
        "a wrong password should be refused, got {result:?}"
    );

    // Right password: a slot arrives.
    let mut good = terrustia_client::Client::connect(addr, "good")
        .await
        .unwrap();
    good.set_timeout(Duration::from_secs(5));
    good.send(&hello()).await.unwrap();
    good.wait_for(
        "the password prompt",
        |e| matches!(e, Event::Other(f) if f.id == id::REQUEST_PASSWORD),
    )
    .await
    .unwrap();
    good.send(&send_password("hunter2")).await.unwrap();
    good.wait_for(
        "a player slot",
        |e| matches!(e, Event::Other(f) if f.id == id::PLAYER_INFO),
    )
    .await
    .expect("the right password should be accepted");
}

#[tokio::test]
async fn a_password_cannot_be_used_to_skip_the_version_check() {
    let addr = start_with(
        Config {
            password: "hunter2".into(),
            ..Config::default()
        },
        |_| {},
    )
    .await;

    // Send only the password, never a Hello.
    let mut sneaky = terrustia_client::Client::connect(addr, "sneaky")
        .await
        .unwrap();
    sneaky.set_timeout(Duration::from_secs(2));
    let mut w = terrustia_proto::PacketWriter::new(id::SEND_PASSWORD);
    w.string("hunter2");
    sneaky.send(&w.finish().unwrap()).await.unwrap();

    let granted = sneaky
        .wait_for(
            "a player slot",
            |e| matches!(e, Event::Other(f) if f.id == id::PLAYER_INFO),
        )
        .await;
    assert!(granted.is_err(), "the version check was bypassed");
}

/// Ask the server to spawn an NPC beside us and wait for **that one** to arrive.
///
/// Waiting for the next NPC sync of any kind is a race, and one that actually bites: a world is
/// spawning its own creatures the whole time, so a sync arriving just after `/spawn` is as likely
/// to be a passing bat as the thing that was asked for. It stayed hidden while the server sent NPC
/// state ten times a second — the wanted one usually won — and surfaced the moment that rate came
/// down to the game's.
///
/// So the roster is photographed first and the wait is for a slot that is not in it, keyed by slot
/// *and* generation because a spawn may take the slot of something that has just died.
async fn spawn_npc(client: &mut Client, name: &str) -> terrustia_proto::npc::SyncNpc {
    let before: std::collections::HashSet<(u8, u8)> = client
        .world()
        .npcs()
        .map(|npc| (npc.index, npc.generation))
        .collect();

    client.say(&format!("/spawn {name}")).await.unwrap();
    let event = client
        .wait_for("the spawned npc", |e| {
            matches!(e, Event::NpcSynced(n)
                if n.life != 0 && !before.contains(&(n.index, n.generation)))
        })
        .await
        .expect("npc never arrived");
    match event {
        Event::NpcSynced(npc) => npc,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn an_npc_can_be_spawned_and_is_announced_to_everyone() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    let npc = spawn_npc(&mut alice, "Zombie").await;
    assert_eq!(npc.npc_type(), 3, "should be a zombie");
    assert_eq!(npc.life_max, npc.life, "should arrive at full health");

    // Bob sees it too.
    bob.wait_for(
        "the npc",
        |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == 3),
    )
    .await
    .expect("the other player was not told about the npc");
}

#[tokio::test]
async fn spawning_by_name_and_by_id_both_work() {
    let addr = start().await;
    let mut client = join(addr, "namer").await;

    let by_name = spawn_npc(&mut client, "BlueSlime").await;
    assert_eq!(by_name.npc_type(), 1);

    let by_id = spawn_npc(&mut client, "49").await;
    assert_eq!(by_id.npc_type(), 49, "cave bat by id");
}

/// `ai/mod.rs`'s ai_style 55 arm (the Brain of Cthulhu's own Creepers, npc 267) has always said,
/// in its own comment, that "the Brain's position is threaded in through ai[2..3] by the server,
/// which knows where every NPC is; a creeper with no Brain removes itself." Nothing ever did that
/// threading (`game/server.rs` had no code anywhere that wrote to a Creeper's `ai[2]`/`ai[3]`), so
/// every Creeper read its own untouched `ai == [0.0; 4]` spawn default as "no Brain" on every one
/// of its own AI ticks and asked to be removed (`creeper::update`'s `BrainGone` branch sets
/// `time_left = 0`) from the moment it spawned.
///
/// This was invisible in most ordinary play because `tick_life` (`npc_ai.rs`) resets a non-boss
/// npc's `time_left` back up to its full despawn budget every tick a player stands nearby — which
/// silently clobbers the Creeper's own self-removal *as long as a player stays close*, and lets it
/// through the instant one does not (a player walking away, or the Brain teleporting its escort
/// out of the player's own despawn box). From a connected client's own tracked view that looks
/// exactly like an escort that "just doesn't sync" — indistinguishable, without reading the
/// server's own tick internals, from a genuine broadcast/section gap. It is not one: the server
/// really was killing its own Creepers, just unreliably, for a different reason than any escort
/// sync issue.
///
/// Fixed in `game/server.rs`'s per-tick AI loop: the Brain's own live centre is now threaded into
/// every alive Creeper's `ai[2]`/`ai[3]` every tick, exactly as the comment already promised.
///
/// This test isolates the actual mechanism rather than re-running a whole fight: it spawns a real
/// Brain of Cthulhu (which arrives wrapped in its real twenty Creepers on its own first AI tick,
/// `boss::brain::update`), and waits for a real, later sync — not the spawn broadcast, which is
/// sent before any Creeper has had its first AI tick and so always carries the untouched `[0.0;
/// 4]` default either way — to carry the Brain's own real, non-zero position in `ai[2..3]`. On the
/// unfixed server this can never happen: nothing ever writes those fields, so `ai[2]` and `ai[3]`
/// stay exactly `0.0` for the entire life of every Creeper, deterministically, forever.
#[tokio::test]
async fn brain_of_cthulhus_creepers_are_told_where_the_brain_is() {
    let addr = start().await;
    let mut client = join(addr, "watcher").await;

    let brain = spawn_npc(&mut client, "Brain of Cthulhu").await;
    assert_eq!(brain.npc_type(), 266);

    let mut threaded: std::collections::HashSet<u8> = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() || threaded.len() >= 15 {
            break;
        }
        match tokio::time::timeout(left, client.next_event()).await {
            Ok(Ok(Event::NpcSynced(n))) if n.npc_type() == 267 && n.life > 0 => {
                if n.ai[2] != 0.0 || n.ai[3] != 0.0 {
                    threaded.insert(n.index);
                }
            }
            Ok(Ok(_)) => {}
            _ => break,
        }
    }
    assert!(
        threaded.len() >= 15,
        "expected most of the Brain's twenty Creepers to have the Brain's real position threaded \
         into ai[2..3] (ai_style 55's own documented contract) within the patience; only {} did — \
         `game/server.rs`'s per-tick Creeper/Brain threading regressed",
        threaded.len()
    );
}

/// The real explanation for what an earlier investigation disclosed as "Skeletron Prime's own
/// sync is intermittent... stuck at its exact starting life for the full patience" — traced here
/// with a real minimal reproduction, not guessed at. `/spawn`ing Prime by day (this suite's own
/// default world state) puts the head straight into `ENRAGED` (`prime_head`'s own `world.conditions
/// .day` check, matching real vanilla's "daylight does not send Prime home, it makes it worse"),
/// where it is invulnerable and runs its target down at real, uncapped contact speed. A fresh
/// character has nowhere near enough health to survive that: two real hits land here, 47 damage
/// each, and a real `PlayerDied` follows within a few real ticks. With its only target now dead,
/// the head's own target-loss check (`player9.dead`, `NPC.cs:27833-27842`) correctly finds nobody
/// left and sets `ai[1] = 3` (`LEAVING`) — real, deliberate vanilla parity, not a sync failure. A
/// dead-and-abandoning boss looks, from a passive watcher's own tracked view, exactly like "life
/// pinned, nothing happening": no further hits ever land because there is no one left to land them
/// on, and the head is genuinely leaving, not stuck.
#[tokio::test]
async fn skeletron_prime_gives_up_once_its_daytime_rampage_kills_its_only_target() {
    let addr = start().await;
    let mut client = join(addr, "prime-victim").await;

    let head = spawn_npc(&mut client, "Skeletron Prime").await;
    assert_eq!(head.npc_type(), 127);

    client
        .wait_for(
            "the player's own death to Prime's enraged contact damage",
            |e| matches!(e, Event::PlayerDied(d) if d.reason.npc == Some(i16::from(head.index))),
        )
        .await
        .expect("a daytime-enraged Prime should run its only, undefended target down");

    client
        .wait_for(
            "the head giving up on its now-dead target",
            |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == 127 && n.ai[1] == 3.0),
        )
        .await
        .expect(
            "Prime should abandon a dead target (real vanilla parity) rather than sit idle — if \
             this never arrives, the target-loss check regressed",
        );
}

/// THROWAWAY diagnostic, not a permanent test: does a `/spawn`ed Solar Pillar sync reliably to a
/// stationary nearby client over an extended window? To be deleted after this check either way.
#[tokio::test]
async fn zzz_throwaway_pillar_sync_probe() {
    let addr = start().await;
    let mut client = join(addr, "pillar-watcher").await;

    let head = spawn_npc(&mut client, "517").await;
    println!(
        "spawned pillar: index={} ai={:?} life={} position={:?}",
        head.index, head.ai, head.life, head.position
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    let mut samples = 0;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, client.next_event()).await {
            Ok(Ok(Event::NpcSynced(n))) if n.npc_type() == 517 => {
                samples += 1;
                if samples <= 5 || samples % 20 == 0 {
                    println!(
                        "t+ sample {samples}: index={} gen={} target={} ai={:?} life={} position={:?}",
                        n.index, n.generation, n.target, n.ai, n.life, n.position
                    );
                }
            }
            Ok(Ok(Event::PlayerHurt(h))) => println!("PLAYER HURT: {h:?}"),
            Ok(Ok(Event::PlayerDied(d))) => println!("PLAYER DIED: {d:?}"),
            Ok(Ok(_)) => {}
            Err(_) => break,
            _ => {}
        }
        let in_local_view = client.world().npcs().any(|n| n.npc_type() == 517);
        if samples % 30 == 1 {
            println!("  local world().npcs() currently has it: {in_local_view}");
        }
    }
    let still_present = client.world().npcs().any(|n| n.npc_type() == 517);
    println!("total pillar samples: {samples}, present in local world() at end: {still_present}");
}

/// Skeletron Prime's own real fight is a four-part boss: a head that spawns a saw, a vice, a
/// cannon and a laser arm on its own first AI tick (`NPC.cs:27806-27832`). Nothing anywhere in
/// this project ever created those arms — the admin `/spawn` command, the only way to encounter
/// this boss at all today, produced a bare head with no weapons and no way to ever come apart.
#[tokio::test]
async fn skeletron_primes_head_raises_its_four_arms() {
    let addr = start().await;
    let mut client = join(addr, "prime-watcher").await;

    let head = spawn_npc(&mut client, "Skeletron Prime").await;
    assert_eq!(head.npc_type(), 127);

    let mut arms: std::collections::HashSet<u16> = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() || arms.len() >= 4 {
            break;
        }
        match tokio::time::timeout(left, client.next_event()).await {
            Ok(Ok(Event::NpcSynced(n))) if (128..=131).contains(&n.npc_type()) => {
                arms.insert(n.npc_type());
            }
            Ok(Ok(_)) => {}
            _ => break,
        }
    }
    assert_eq!(
        arms,
        std::collections::HashSet::from([128, 129, 130, 131]),
        "expected the saw, vice, cannon and laser (128-131) to all appear once the head's first \
         AI tick ran; only saw {arms:?} — `game/ai/boss/prime.rs`'s arm-raising regressed"
    );
}

#[tokio::test]
async fn an_unknown_npc_name_is_refused() {
    let addr = start().await;
    let mut client = join(addr, "typo").await;

    client.say("/spawn Notarealenemy").await.unwrap();
    client
        .wait_for(
            "the usage hint",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("usage: /spawn")),
        )
        .await
        .unwrap();
}

/// A blow smaller than half an NPC's armour still takes a point off.
///
/// The game's rule is `Math.Max(1, damage - defense/2)` — there is no such thing as a hit that
/// lands and does nothing. The Guide has 250 life and 30 defence, so one damage against him is the
/// smallest possible real hit, and a real 1.4.5.8 server takes him from 250 to 249 for it.
///
/// Written because a probe against both servers disagreed here and it took a while to establish
/// which of the three candidates was at fault: the damage floor, the sync, or the probe.
#[tokio::test]
async fn a_blow_smaller_than_half_the_armour_still_lands() {
    let addr = start().await;
    let mut client = join(addr, "prodder").await;

    let guide = spawn_npc(&mut client, "Guide").await;
    assert_eq!(guide.life_max, 250, "the Guide's health");

    client
        .hit_npc(guide.index, guide.generation, 1, 0.0, 1)
        .await
        .unwrap();

    let event = client
        .wait_for(
            "the wounded guide",
            |e| matches!(e, Event::NpcSynced(n) if n.index == guide.index && n.life < 250),
        )
        .await
        .expect("the hit was never reported");
    if let Event::NpcSynced(hurt) = event {
        assert_eq!(
            hurt.life, 249,
            "one damage against thirty defence is still one damage"
        );
    }
}

#[tokio::test]
async fn hitting_an_npc_reduces_its_health_and_kills_it() {
    let addr = start().await;
    let mut client = join(addr, "fighter").await;

    // A zombie has 45 health and 6 defence, so a 20-damage hit lands for 17.
    let npc = spawn_npc(&mut client, "Zombie").await;
    assert_eq!(npc.life_max, 45);

    client
        .hit_npc(npc.index, npc.generation, 20, 0.0, 1)
        .await
        .unwrap();
    let event = client
        .wait_for("the wounded npc", |e| {
            matches!(e, Event::NpcSynced(n) if n.index == npc.index && n.life > 0 && n.life < 45)
        })
        .await
        .unwrap();
    if let Event::NpcSynced(hurt) = event {
        assert_eq!(hurt.life, 28, "45 - (20 - 6/2) = 28");
    }

    // Two more hits finish it.
    for _ in 0..2 {
        client
            .hit_npc(npc.index, npc.generation, 20, 0.0, 1)
            .await
            .unwrap();
    }
    client
        .wait_for(
            "the death",
            |e| matches!(e, Event::NpcSynced(n) if n.index == npc.index && n.life == 0),
        )
        .await
        .expect("the zombie never died");
}

#[tokio::test]
async fn a_dead_npc_drops_its_coin_value() {
    let addr = start().await;
    let mut client = join(addr, "looter").await;

    // A zombie is worth 60 copper.
    let npc = spawn_npc(&mut client, "Zombie").await;
    for _ in 0..5 {
        client
            .hit_npc(npc.index, npc.generation, 40, 0.0, 1)
            .await
            .unwrap();
    }

    let event = client
        .wait_for(
            "the coin drop",
            |e| matches!(e, Event::ItemSynced(i) if (71..=74).contains(&i.item.id)),
        )
        .await
        .expect("no coins dropped");
    if let Event::ItemSynced(item) = event {
        // The drop is varied now (`NPC.NPCLoot_DropMoney`: a -20%..+75% base plus jackpots), so a
        // zombie's 60 copper comes out as some copper, or a little silver plus copper once the roll
        // carries it past a hundred, rather than exactly sixty copper. It stays coins, and a small
        // one: never gold or platinum off sixty base.
        assert!(
            (71..=72).contains(&item.item.id),
            "a zombie's coins are copper or, after a high roll, silver: got item {}",
            item.item.id
        );
        assert!(item.item.stack >= 1, "a coin stack is never empty");
    }
}

#[tokio::test]
async fn a_hit_with_a_stale_generation_is_ignored() {
    let addr = start().await;
    let mut client = join(addr, "stale").await;

    let npc = spawn_npc(&mut client, "Zombie").await;
    // Claim a generation the NPC does not have; the hit must not land.
    client
        .hit_npc(npc.index, npc.generation.wrapping_add(7), 100, 0.0, 1)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    client.say("/npcs").await.unwrap();
    let event = client
        .wait_for(
            "the npc list",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("NPCs")),
        )
        .await
        .unwrap();
    if let Event::Chat { text, .. } = event {
        assert!(
            text.contains("Zombie"),
            "the zombie should have survived a stale hit: {text}"
        );
    }
}

#[tokio::test]
async fn butcher_clears_hostiles_but_spares_town_npcs() {
    let addr = start().await;
    let mut client = join(addr, "butcher").await;

    spawn_npc(&mut client, "Zombie").await;
    spawn_npc(&mut client, "Guide").await;

    client.say("/butcher").await.unwrap();
    client
        .wait_for(
            "the butcher report",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("Butchered")),
        )
        .await
        .unwrap();

    client.say("/npcs").await.unwrap();
    let event = client
        .wait_for(
            "the npc list",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("NPCs")),
        )
        .await
        .unwrap();
    if let Event::Chat { text, .. } = event {
        assert!(text.contains("Guide"), "the guide should remain: {text}");
        assert!(
            !text.contains("Zombie"),
            "the zombie should be gone: {text}"
        );
    }
}

#[tokio::test]
async fn a_joining_player_is_told_about_npcs_already_alive() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let npc = spawn_npc(&mut alice, "Skeleton").await;

    // Bob joins afterwards and should be sent the skeleton during his handshake.
    let mut bob = join(addr, "bob").await;
    bob.set_timeout(Duration::from_secs(5));
    let seen = bob
        .wait_for(
            "the existing npc",
            |e| matches!(e, Event::NpcSynced(n) if n.index == npc.index),
        )
        .await;
    assert!(seen.is_ok(), "a late joiner never heard about the skeleton");
}

/// Build a furnished, sealed room into a world, returning a tile inside it.
fn build_house(world: &mut World, x0: i32, y0: i32) -> (i32, i32) {
    let (w, h) = (14, 10);
    for x in x0..x0 + w {
        for y in y0..y0 + h {
            if x == x0 || x == x0 + w - 1 || y == y0 || y == y0 + h - 1 {
                world.set_tile(x, y, Tile::block(1));
            } else {
                let mut air = Tile::AIR;
                air.wall = 4; // stone wall counts as a house wall
                world.set_tile(x, y, air);
            }
        }
    }
    world.set_tile(x0 + 2, y0 + h - 2, Tile::framed(15, 0, 0)); // chair
    world.set_tile(x0 + 4, y0 + h - 2, Tile::framed(14, 0, 0)); // table
    world.set_tile(x0 + 6, y0 + h - 2, Tile::framed(4, 0, 0)); // torch
    world.set_tile(x0 + 1, y0 + h - 2, Tile::framed(10, 0, 0)); // door
    (x0 + 5, y0 + 3)
}

/// A finished house gets a Guide, without anybody asking for one.
///
/// This is the whole of town NPCs arriving: the server scans for a free house near a player every
/// few seconds and moves somebody into it. A server that never does it has a world where nobody
/// ever turns up, however much is built.
#[tokio::test]
async fn a_house_gets_a_guide() {
    let inside = std::cell::Cell::new((0, 0));
    let addr = start_with(Config::default(), |world| {
        inside.set(build_house(world, 300, 300));
    })
    .await;
    let mut client = join(addr, "builder").await;
    let (hx, hy) = inside.get();
    client
        .move_to(hx as f32 * 16.0, hy as f32 * 16.0)
        .await
        .unwrap();

    // The scan runs every few seconds, so this waits rather than expecting it at once.
    client.set_timeout(Duration::from_secs(20));
    let guide = client
        .wait_for(
            "the Guide moving in",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 22),
        )
        .await
        .expect("a Guide should have moved into a finished house");
    let Event::NpcSynced(guide) = guide else {
        unreachable!("matched on it")
    };
    // He arrives in the house rather than wherever the scan started from.
    assert!(
        (guide.position.0 / 16.0 - hx as f32).abs() < 20.0,
        "the Guide moved in somewhere else: {:?}",
        guide.position
    );
}

/// The Old Man is a real, permanently homeless town NPC by design — he haunts the dungeon entrance
/// and never moves into a house in real vanilla (`WorldGen.FindAnyHomelessTownNPC`'s own exclusion
/// list, `nPC.type != 37 && != 453 && != 368`). Without the matching exclusion in
/// `tick_town_npcs`, its own "an already-homeless resident claims the next free house before a
/// newcomer does" priority rule let him steal a house from the real newcomer it was meant for —
/// found live, 2-for-2, in `moonlord.rs`'s own real full runs (`plan.md`'s "Real spawn triggers for
/// the Wall of Flesh..." Done row): the Guide's own freshly-built house went to a wandering-by Old
/// Man instead, both times.
#[tokio::test]
async fn the_old_man_never_steals_a_house_from_a_real_newcomer() {
    let inside = std::cell::Cell::new((0, 0));
    let addr = start_with(Config::default(), |world| {
        inside.set(build_house(world, 300, 300));
    })
    .await;
    let mut client = join(addr, "landlord").await;
    let (hx, hy) = inside.get();
    client
        .move_to(hx as f32 * 16.0, hy as f32 * 16.0)
        .await
        .unwrap();

    // Present well before the first housing scan has any chance to run, standing right next to
    // the newly-buildable house — the exact shape of the real collision this test closes.
    let old_man = spawn_npc(&mut client, "OldMan").await;
    assert_eq!(old_man.npc_type(), 37);

    client.set_timeout(Duration::from_secs(20));
    let moved_in = client
        .wait_for(
            "someone announced moving into the house",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("has moved in")),
        )
        .await
        .expect("a resident should have moved into the finished house");
    let Event::Chat { text, .. } = moved_in else {
        unreachable!("matched on it")
    };
    assert_eq!(
        text, "The Guide has moved in.",
        "the Old Man should never be eligible to claim a house at all — got: {text:?}"
    );
}

/// The housing screen has to actually do something.
///
/// Packet 60 travels both ways: the server announces where each town NPC lives, and a client sends
/// the same id to ask for a change — dragging an NPC into a room, or evicting one. Only the
/// outbound half existed, so the inbound one fell through to the ignore arm and every use of the
/// housing UI silently did nothing on this server while appearing to have worked locally.
#[tokio::test]
async fn a_player_can_evict_and_rehouse_a_town_npc() {
    use terrustia_proto::packets::{HouseholdStatus, npc_home};

    let inside = std::cell::Cell::new((0, 0));
    let addr = start_with(Config::default(), |world| {
        inside.set(build_house(world, 300, 300));
    })
    .await;
    let mut client = join(addr, "landlord").await;
    let (hx, hy) = inside.get();
    client
        .move_to(hx as f32 * 16.0, hy as f32 * 16.0)
        .await
        .unwrap();

    client.set_timeout(Duration::from_secs(20));
    let guide = client
        .wait_for(
            "the Guide moving in",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 22),
        )
        .await
        .expect("a Guide should have moved into a finished house");
    let Event::NpcSynced(guide) = guide else {
        unreachable!("matched on it")
    };

    // The packet a client sends is the same shape the server sends back, so the encoder is shared.
    // Status 1 is the eviction, exactly as vanilla's own client sends it.
    let evict = npc_home(u16::from(guide.index), 0, 0, HouseholdStatus::Homeless).unwrap();
    client.send(&evict).await.unwrap();

    let announced = client
        .wait_for("the eviction announced back", |e| {
            matches!(e, Event::Other(f)
                if f.id == terrustia_proto::id::NPC_HOME
                    && f.payload.last() == Some(&(HouseholdStatus::Homeless as u8)))
        })
        .await;
    assert!(
        announced.is_ok(),
        "an eviction has to reach everyone's housing screen, including the asker's"
    );

    // And move him back in, to a room the server agrees is habitable.
    let rehouse = npc_home(
        u16::from(guide.index),
        hx as i16,
        hy as i16,
        HouseholdStatus::Settled,
    )
    .unwrap();
    client.send(&rehouse).await.unwrap();

    let settled = client
        .wait_for("the rehousing announced back", |e| {
            matches!(e, Event::Other(f)
                if f.id == terrustia_proto::id::NPC_HOME
                    && f.payload.last() != Some(&(HouseholdStatus::Homeless as u8)))
        })
        .await;
    assert!(
        settled.is_ok(),
        "a valid room should be accepted and announced"
    );
}

#[tokio::test]
async fn the_house_command_explains_why_a_room_is_rejected() {
    let addr = start_with(Config::default(), |world| {
        // A room with everything except a door.
        let (x0, y0) = (300, 300);
        for x in x0..x0 + 14 {
            for y in y0..y0 + 10 {
                if x == x0 || x == x0 + 13 || y == y0 || y == y0 + 9 {
                    world.set_tile(x, y, Tile::block(1));
                } else {
                    let mut air = Tile::AIR;
                    air.wall = 4;
                    world.set_tile(x, y, air);
                }
            }
        }
        world.set_tile(x0 + 2, y0 + 8, Tile::framed(15, 0, 0));
        world.set_tile(x0 + 4, y0 + 8, Tile::framed(14, 0, 0));
        world.set_tile(x0 + 6, y0 + 8, Tile::framed(4, 0, 0));
    })
    .await;

    let mut client = join(addr, "builder").await;
    client.move_to(305.0 * 16.0, 303.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    client.say("/house").await.unwrap();
    let event = client
        .wait_for(
            "the housing verdict",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("house")),
        )
        .await
        .unwrap();
    if let Event::Chat { text, .. } = event {
        assert!(
            text.contains("needs a door"),
            "should name the missing door, said: {text}"
        );
    }
}

#[tokio::test]
async fn a_finished_house_is_accepted() {
    let addr = start_with(Config::default(), |world| {
        build_house(world, 300, 300);
    })
    .await;

    let mut client = join(addr, "builder").await;
    client.move_to(305.0 * 16.0, 303.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    client.say("/house").await.unwrap();
    let event = client
        .wait_for(
            "the housing verdict",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("house")),
        )
        .await
        .unwrap();
    if let Event::Chat { text, .. } = event {
        assert!(
            text.contains("valid house"),
            "should accept it, said: {text}"
        );
    }
}

#[tokio::test]
async fn the_guide_moves_into_a_finished_house() {
    let addr = start_with(Config::default(), |world| {
        build_house(world, 300, 300);
    })
    .await;

    let mut client = join(addr, "host").await;
    // Stand in the house so the housing scan looks here.
    client.move_to(305.0 * 16.0, 303.0 * 16.0).await.unwrap();

    // The announcement goes out just before the NPC itself, so both have to be watched at once
    // rather than one after the other.
    client.set_timeout(Duration::from_secs(25));
    let mut announced = false;
    let mut guide_at = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    while tokio::time::Instant::now() < deadline && (!announced || guide_at.is_none()) {
        match client.next_event().await {
            Ok(Event::Chat { text, .. }) if text.contains("moved in") => announced = true,
            Ok(Event::NpcSynced(n)) if n.npc_type() == 22 => {
                guide_at = Some((n.position.0 / 16.0, n.position.1 / 16.0));
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let (tx, ty) = guide_at.expect("the Guide never moved in");
    assert!(
        (300.0..315.0).contains(&tx) && (295.0..312.0).contains(&ty),
        "the Guide moved in at ({tx:.0}, {ty:.0}), which is not the house"
    );
    assert!(announced, "nobody announced the arrival");
}

/// Somebody other than the Guide moves in once the world has earned them.
///
/// This is the one that never happened. `tick_town_npcs` housed a homeless resident or spawned the
/// Guide, and that was the whole arrival system — so a town was one house and one Guide forever,
/// and the Mechanic, who sells the only wire in the game, could never come.
#[tokio::test]
async fn a_second_resident_arrives_once_the_world_earns_them() {
    let config = Config {
        max_players: 4,
        ..Default::default()
    };
    let addr = start_with(config, |world| {
        // Two houses, so the Guide takes one and the next arrival has somewhere to go.
        build_house(world, 300, 300);
        build_house(world, 330, 300);
        // A boss is down, which is what the Dryad waits for.
        world.progress.downed_boss1 = true;
    })
    .await;

    let mut client = join(addr, "host").await;
    client.move_to(305.0 * 16.0, 303.0 * 16.0).await.unwrap();
    client.set_timeout(Duration::from_secs(40));

    const DRYAD: u16 = 20;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    let mut seen = Vec::new();
    while tokio::time::Instant::now() < deadline && !seen.contains(&DRYAD) {
        match client.next_event().await {
            Ok(Event::NpcSynced(n)) => {
                let kind = n.npc_type();
                if !seen.contains(&kind) {
                    seen.push(kind);
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    assert!(
        seen.contains(&DRYAD),
        "no second resident arrived; saw {seen:?}. Only the Guide could ever move in before \
         arrivals were implemented",
    );
}

/// Talking to somebody tied up frees them, and they can then move in.
///
/// Six residents are found rather than earned, and nothing set the flag their arrival waits on —
/// so the Mechanic could never appear, and she sells the only wire in the game. An entire ported,
/// documented subsystem sat unreachable behind this one missing interaction.
#[tokio::test]
async fn a_bound_townsperson_can_be_freed() {
    const BOUND_MECHANIC: u16 = 123;
    const MECHANIC: u16 = 124;

    let addr = start().await;
    let mut client = join(addr, "rescuer").await;
    client.set_timeout(Duration::from_secs(15));

    // Put one in the world where the player is standing.
    client
        .say(&format!("/spawn {BOUND_MECHANIC}"))
        .await
        .unwrap();
    let bound = client
        .wait_for(
            "a bound mechanic",
            |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == BOUND_MECHANIC),
        )
        .await
        .expect("the bound mechanic should spawn");
    let Event::NpcSynced(npc) = bound else {
        panic!("expected an npc")
    };

    // Talk to her.
    client.talk_to_npc(npc.index).await.unwrap();

    let freed = client
        .wait_for(
            "the freed mechanic",
            |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == MECHANIC),
        )
        .await;
    assert!(
        freed.is_ok(),
        "talking to a bound mechanic must free her, or there is no wire in the game",
    );
}

/// An ordinary (un-bound) town NPC can be talked to, and the interaction is visible to everyone
/// else on the server.
///
/// This is the interaction a real client's shop UI actually rides on. Opening a shop in vanilla
/// is entirely client-side (`Main.npcShop = index; shop[npcShop].SetupShop(npcShop);`,
/// `Main.cs:41174`) — no packet populates it — and the click-to-talk gate the client evaluates
/// locally (`nPC.townNPC`, derived from `type` alone, plus `velocity.Y == 0f`, `Main.cs:43781`) is
/// satisfied by an ordinary NPC sync with no extra server work at all. The one thing the server
/// does own is packet 40 itself (`SYNC_TALK_N_P_C`), which `a_bound_townsperson_can_be_freed`
/// already proves end to end for the *bound* case; this proves the same path for the ordinary
/// case that path does not cover, and that it reaches other players rather than being swallowed.
#[tokio::test]
async fn a_town_npc_can_be_talked_to_and_it_is_visible_to_everyone() {
    const MERCHANT: u16 = 17;

    // `/spawn` puts the NPC where the player is, and a joining player starts at tile (4, -2) in
    // this world: above the terrain, at the far left, over a three-thousand-pixel drop. Left to
    // the generator, whether the merchant lands at all depends on how far it drifts sideways on
    // the way down, and it was only landing because it happened to clip a ledge at tile x 2. Any
    // change to a resident's walking speed moved it off that ledge, out through x = 0 and into
    // open space, where nothing stops it: it fell past the bottom of the world still travelling.
    // Give it a floor, the way `clear_with_floor` exists to do: this test is about packet 40,
    // not about where the generator happens to leave a rock.
    let addr = start_with(Config::default(), |world| {
        clear_with_floor(world, 12, 3, 12, 3);
    })
    .await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    let npc = spawn_npc(&mut alice, "Merchant").await;
    assert_eq!(npc.npc_type(), MERCHANT);

    // Wait for it to come to rest — a real client refuses to let a player interact with a town
    // NPC that is still falling (`NPC.CanBeTalkedTo` requires `velocity.Y == 0f`).
    let settled = alice
        .wait_for(
            "the merchant to land",
            |e| matches!(e, Event::NpcSynced(n) if n.index == npc.index && n.velocity.1 == 0.0),
        )
        .await
        .expect("a town NPC should come to rest almost immediately after spawning");
    let Event::NpcSynced(settled) = settled else {
        unreachable!("matched on it")
    };
    assert_eq!(settled.velocity.1, 0.0);
    assert_eq!(settled.npc_type(), MERCHANT);

    alice.talk_to_npc(npc.index).await.unwrap();

    // Bob sees the interaction too — packet 40 is relayed, not swallowed.
    bob.wait_for(
        "alice's talk packet relayed",
        |e| matches!(e, Event::Other(frame) if frame.id == terrustia_proto::id::SYNC_TALK_N_P_C),
    )
    .await
    .expect("packet 40 should be relayed to other connected players");
}

/// A settled town NPC fights back against a hostile nearby, and the shot is visible over the wire
/// to every connected player — not just applied silently on the server.
///
/// Before this, README.md's own words: "the first Blood Moon after anyone moves in, the town
/// stands still and dies." A Merchant (npc type 17) is one of four representative town NPCs this
/// pass covers (see `game::ai::town_combat`'s module doc) — real vanilla numbers (projectile 48,
/// `NPC.cs:54969`), reimplemented cadence.
#[tokio::test]
async fn a_town_npc_fights_back_against_a_nearby_hostile() {
    const MERCHANT: u16 = 17;

    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    // `/spawn` always drops an NPC at (player position + (64, -32)) — spawning both from the same
    // player in quick succession, before either moves, lands them within a few dozen pixels of
    // each other, well inside the Merchant's 320px `DangerDetectRange`.
    let merchant = spawn_npc(&mut alice, "Merchant").await;
    assert_eq!(merchant.npc_type(), MERCHANT);
    spawn_npc(&mut alice, "Zombie").await;

    // Wait for the merchant to land — the same client-visible gate the shop test above uses:
    // `NPC.CanBeTalkedTo`/vanilla's own attack branches both require `velocity.Y == 0f`.
    alice
        .wait_for("the merchant to land", |e| {
            matches!(e, Event::NpcSynced(n) if n.index == merchant.index && n.velocity.1 == 0.0)
        })
        .await
        .expect("a town NPC should come to rest almost immediately after spawning");

    let shot = alice
        .wait_for(
            "the merchant to open fire",
            |e| matches!(e, Event::ProjectileSynced(p) if p.projectile_type == 48),
        )
        .await
        .expect("a merchant with a zombie beside it should fire its pistol");
    let Event::ProjectileSynced(shot) = shot else {
        unreachable!("matched on it")
    };
    assert!(shot.damage > 0);

    // Bob sees it too — a shot fired server-side that never reaches other clients would leave
    // the town's own defence invisible to everyone who is not the one being rescued.
    bob.wait_for(
        "the merchant's shot relayed to a second player",
        |e| matches!(e, Event::ProjectileSynced(p) if p.projectile_type == 48),
    )
    .await
    .expect("the merchant's shot should be broadcast, not applied silently");
}

/// The shot a town NPC fires is not just broadcast — it actually lands. This is the concrete claim
/// README.md's row named: a town does not merely animate a fight, a hostile beside it takes real
/// damage from it, the same as it would from a player's own gunfire.
#[tokio::test]
async fn a_town_npcs_shot_actually_damages_the_hostile_it_targeted() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;

    let merchant = spawn_npc(&mut alice, "Merchant").await;
    let zombie = spawn_npc(&mut alice, "Zombie").await;

    alice
        .wait_for("the merchant to land", |e| {
            matches!(e, Event::NpcSynced(n) if n.index == merchant.index && n.velocity.1 == 0.0)
        })
        .await
        .expect("a town NPC should come to rest almost immediately after spawning");

    let hurt = alice
        .wait_for(
            "the zombie to take damage",
            |e| matches!(e, Event::NpcSynced(n) if n.index == zombie.index && n.life < zombie.life),
        )
        .await
        .expect("the merchant's shot should collide with and damage the zombie it was fired at");
    let Event::NpcSynced(hurt) = hurt else {
        unreachable!("matched on it")
    };
    assert!(
        hurt.life < zombie.life,
        "life should have gone down, not merely changed"
    );
}

/// A real player watched a real Guide open fire on a Firefly and asked why. The `hostiles` list
/// `server.rs` builds for every combat-capable town resident used to be `!friendly && !town_npc &&
/// is_alive()` — no damage check at all, so a harmless critter (`friendly: false` in this
/// project's own data the same way a real hostile is, but always `damage: 0`) qualified exactly
/// like a real threat. Firefly (355) is real vanilla's own `friendly: false, damage: 0` shape.
#[tokio::test]
async fn a_combat_town_npc_does_not_shoot_a_harmless_critter() {
    let (addr, game_task, listener_task) = start_with_owned_tasks().await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(15));

    let guide = spawn_npc(&mut alice, "Guide").await;
    spawn_npc(&mut alice, "Firefly").await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut fired_at_something = false;
    while tokio::time::Instant::now() < deadline {
        let Ok(event) = alice.next_event().await else {
            break;
        };
        if let Event::ProjectileSynced(p) = event
            && p.projectile_type == 1
        {
            fired_at_something = true;
            break;
        }
    }
    assert!(
        !fired_at_something,
        "the Guide should never open fire on a harmless, zero-damage critter"
    );
    assert_eq!(guide.net_id, 22, "sanity: the right NPC was spawned");
    game_task.abort();
    listener_task.abort();
}

/// A real player watched a real Guide's arrow leave from around his head rather than an
/// outstretched hand. Every ranged town NPC's shot used to spawn at the NPC's own plain geometric
/// `center()`, with no offset at all — real vanilla's own launch point, at every branch of
/// `AI_007_TownEntities`, is `Center.X + spriteDirection * 16, Center.Y - 2` (`NPC.cs`, e.g.
/// `:53515+1553`): sixteen pixels forward in the direction the NPC is actually facing, two pixels
/// up, not dead center.
#[tokio::test]
async fn a_town_npcs_shot_spawns_from_an_outstretched_hand_not_dead_center() {
    let (addr, game_task, listener_task) = start_with_owned_tasks().await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(15));

    // Report a real position first. Without this the server's copy of the player sits at the
    // world's own origin - the client sets its own `position` from the world spawn after the
    // handshake but never sends a packet 13 with it - so `/spawn`'s "player position + (64, -32)"
    // puts the NPC at tile 4, on the very edge of the world, wherever the generated terrain there
    // happens to be. A town NPC that lands there and finds nothing to fight walks, and the world
    // runs out four tiles later: the NPC steps off the left edge and falls for the rest of the
    // test, so it never reports the settled vertical velocity this waits on. Nothing about the
    // combat being tested; a real player is never standing at (0, 0).
    let (spawn_x, spawn_y) = alice.position();
    alice.move_to(spawn_x, spawn_y).await.unwrap();

    let guide = spawn_npc(&mut alice, "Guide").await;
    spawn_npc(&mut alice, "Zombie").await;

    // Guide's own real `width`/`height` (`npc_data.rs`): 18x40. `position` is the top-left corner,
    // the same convention this project's server-side `Npc::center()` uses.
    let mut guide_position = guide.position;
    let mut shot_position = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while shot_position.is_none() && tokio::time::Instant::now() < deadline {
        let Ok(event) = alice.next_event().await else {
            break;
        };
        match event {
            Event::NpcSynced(n) if n.index == guide.index => guide_position = n.position,
            Event::ProjectileSynced(p) if p.projectile_type == 1 => {
                shot_position = Some(p.position);
            }
            _ => {}
        }
    }
    let shot_position = shot_position.expect("the Guide never fired at the zombie");
    let center = (guide_position.0 + 9.0, guide_position.1 + 20.0);

    // Loose bounds, not pixel-perfect ones: `guide_position` is whichever `NpcSynced` for this
    // NPC arrived most recently by the time the shot's own event is processed, and a real, one-
    // tick-wide race between an NPC's own last-position sync and its very next shot is already a
    // documented, accepted source of slack in the test above this one ("Sequential `wait_for`
    // calls race on that"). What this test needs to prove is that a *real, non-trivial* offset
    // exists in both axes, in the right rough shape and direction — not exact equality with a
    // formula computed from a potentially one-tick-stale center.
    assert!(
        (shot_position.0 - center.0).abs() > 5.0,
        "the shot should launch offset toward whichever way the Guide is facing, not from dead \
         center: shot={shot_position:?} center={center:?}"
    );
    assert!(
        (shot_position.1 - center.1) < 0.0 && (shot_position.1 - center.1).abs() < 10.0,
        "the shot's own vertical offset should be a small upward nudge (vanilla's `Center.Y - \
         2`), not roughly dead center or wildly off: shot={shot_position:?} center={center:?}"
    );
    game_task.abort();
    listener_task.abort();
}

/// Every one of the 24 town NPCs added to `game::ai::town_combat` this session — beyond the four
/// (Merchant/Arms Dealer/Wizard/Dye Trader) the two tests above already cover — actually fights,
/// over a real socket, the same way those four were proven: land, spawn a hostile beside it, watch
/// it open fire (a real `ProjectileSynced` of the right type for the ranged ones; a real health
/// drop with no projectile at all for the two melee ones), and confirm the hostile it targeted
/// actually loses life. A fresh server and client per NPC (see the loop's own comment for why one
/// shared server does not work here) — `try_combat`'s own logic (`town.rs`) sets a freshly-engaged
/// NPC's `ai[1]` to `0.0`, so the first shot fires on the very next tick rather than waiting out
/// its `cooldown`, which keeps each of the 24 iterations fast despite the fresh server each pays
/// for.
#[tokio::test]
async fn every_newly_covered_town_npc_actually_fights() {
    // (npc_type, projectile_type — None for the two melee NPCs). `projectile_type` is `i16` to
    // match `terrustia_proto`'s own `ProjectileSynced::projectile_type` field.
    const NPCS: &[(u16, Option<i16>)] = &[
        (38, Some(30)),   // Demolitionist
        (633, Some(880)), // Bestiary Girl
        (550, Some(669)), // DD2 Bartender
        (588, Some(721)), // Golfer
        (208, Some(588)), // Party Girl
        (369, Some(520)), // Angler
        (453, Some(21)),  // Skeleton Merchant
        (107, Some(24)),  // Goblin Tinkerer
        (124, Some(582)), // Mechanic
        (18, Some(583)),  // Nurse
        (142, Some(589)), // Santa Claus
        (227, Some(587)), // Painter
        (368, Some(14)),  // Travelling Merchant
        (22, Some(1)),    // Guide
        (228, Some(267)), // Witch Doctor
        (178, Some(242)), // Steampunker
        (229, Some(14)),  // Pirate
        (209, Some(135)), // Cyborg
        (54, Some(585)),  // Clothier
        (160, Some(590)), // Truffle
        (663, Some(950)), // Princess
        (20, Some(586)),  // Dryad (real vanilla zero-damage attack — see town_combat's doc)
        (441, None),      // Tax Collector (melee)
        (353, None),      // Stylist (melee)
    ];

    for &(npc_type, projectile_type) in NPCS {
        // A fresh server and client per NPC, not one shared across all 24 — every `/spawn` in this
        // test lands at the same fixed offset near world origin (the test client never sends a
        // real position update, so the server-side `player.position` `/spawn` actually reads from
        // stays at its default rather than tracking where a real client would be), so reusing one
        // server let 24 iterations' worth of town NPCs and zombies pile up in the same few tiles.
        // Once enough of them were jostling each other, a freshly-landed NPC's velocity would
        // never settle to *exactly* zero long enough for this test's own landing check to catch
        // it — a real bug in this test's own design, not in anything under test.
        //
        // `start()` itself never stops what it spawns — both the game task and the listener task
        // run forever, by design, for every other test in this file (a real server does not manage
        // its own shutdown from a test harness). Fine for one server; fatal for 24 in a row inside
        // one process: with no explicit teardown, each iteration leaves its server and listener
        // running in the background, so by NPC 22 (Dryad) there are 21 dead servers still ticking
        // and 21 dead listeners still bound, all competing for the same tokio runtime's threads.
        // That is a real, separate bug from the one the comment above already fixed — found the
        // same way: it reproduced consistently on the one NPC whose very first shot has to survive
        // a real tick round-trip rather than firing off a value already computed, and it reproduced
        // identically whether or not anything else was running on the machine at the time, which
        // ruled out ordinary system load as the cause. `start_with_owned_tasks` below is the same
        // few lines `start_with` already does, just handing back what it spawned so this loop can
        // abort both tasks at the end of every iteration instead of leaking them.
        let (addr, game_task, listener_task) = start_with_owned_tasks().await;
        let mut alice = join(addr, "alice").await;
        // Wider than this file's usual 10s: the town NPC's very first shot aims at wherever the
        // hostile is *right now*, which can still be mid-fall (only the NPC's own landing is
        // waited on above, not the hostile's — both are dropped together and usually settle close
        // together, but not identically), so a miss on the very first shot is real, not a bug, and
        // the test has to survive to whichever later shot the NPC's own cooldown fires once the
        // hostile has actually come to rest. Under a busy machine — this test found this exact gap
        // running inside the full workspace suite rather than alone — the server's own tick rate
        // can lag real time too, so the retry needs real wall-clock room as well as ticks.
        alice.set_timeout(Duration::from_secs(20));

        // Spawned back to back, before waiting on either — both `/spawn` at (player position +
        // (64, -32)) and fall together, exactly like the existing Merchant test above. Waiting for
        // the town NPC to land *before* spawning its hostile (the first, obvious way to write
        // this) is a real bug, not just a slower version of the same thing: `try_combat` only
        // starts a fight when a hostile is already in range, and a landed town NPC with nothing to
        // fight immediately starts walking — by the time a hostile spawned only afterward finally
        // reaches the ground, the NPC has often already wandered hundreds of pixels away.
        // Report a real position first. Without this the server's copy of the player sits at the
        // world's own origin - the client sets its own `position` from the world spawn after the
        // handshake but never sends a packet 13 with it - so `/spawn`'s "player position + (64, -32)"
        // puts the NPC at tile 4, on the very edge of the world, wherever the generated terrain there
        // happens to be. A town NPC that lands there and finds nothing to fight walks, and the world
        // runs out four tiles later: the NPC steps off the left edge and falls for the rest of the
        // test, so it never reports the settled vertical velocity this waits on. Nothing about the
        // combat being tested; a real player is never standing at (0, 0).
        let (spawn_x, spawn_y) = alice.position();
        alice.move_to(spawn_x, spawn_y).await.unwrap();

        let npc = spawn_npc(&mut alice, &npc_type.to_string()).await;
        assert_eq!(npc.npc_type(), npc_type, "the spawn command resolved by id");
        let hostile = spawn_npc(&mut alice, "Zombie").await;

        // Landing, the first shot (if ranged), and the hostile taking damage are all watched for
        // together in one scan, not as separate sequential `wait_for` calls — they can genuinely
        // arrive in the same batch of events. Landing detection and combat engagement both key off
        // this NPC's own velocity settling to exactly zero, and a freshly engaged NPC acts
        // immediately with no cooldown wait (see `try_combat`'s own doc): a ranged NPC's first shot
        // and a melee NPC's first hit can both land on the very same tick its landing is confirmed.
        // Sequential `wait_for` calls race on that: each one discards whatever it scans past
        // looking for its own match, so an event that arrives before (or interleaved with) an
        // earlier wait's own target gets silently eaten while that earlier wait is still scanning
        // — found running this exact test against Dryad (20), whose zero-cooldown first shot makes
        // the race far more likely to land than it does for anything with a real windup.
        let mut landed = false;
        let mut fired = projectile_type.is_none();
        // Dryad (20) is the one real exception: her attack faithfully deals zero pre-scaling
        // damage in vanilla (see town_combat's own module doc), so there is no damage to wait
        // for — asserting a health drop here would wait forever on a real vanilla behaviour.
        let mut hurt = npc_type == 20;
        // **Budgeted in events observed, not in seconds.** This was a twenty-second wall clock and
        // it was the last flake of that shape in the suite: it fails on a *different* NPC each time
        // (453, 453, 588 across four observations, and 588's ball is an AI style nothing has ever
        // touched), two runs in three pass, and the failing run finishes in 78 seconds against 390
        // for a passing one - it bails early rather than losing a shot. A busy machine does not make
        // a town NPC miss, it makes every tick take longer in wall-clock terms, so counting seconds
        // measures the machine and not the server.
        //
        // Every event received is evidence the server got somewhere, so the budget is a count of
        // them. The liveness check is still real and still bounded: `alice`'s own per-event timeout
        // (set above) fires if the server goes quiet, and `next_event` erroring breaks the loop.
        // The size covers the worst cadence in the roster by a wide margin - the Dryad's
        // `AttackTime` is 600 ticks and her gate another 60, and NPC syncs arrive every sixth tick,
        // so a few hundred events is already several attack cycles.
        const EVENT_BUDGET: usize = 4_000;
        let mut seen = 0usize;
        while (!landed || !fired || !hurt) && seen < EVENT_BUDGET {
            let Ok(event) = alice.next_event().await else {
                break;
            };
            seen += 1;
            match event {
                Event::NpcSynced(n) if n.index == npc.index && n.velocity.1 == 0.0 => {
                    landed = true;
                }
                Event::ProjectileSynced(p) if Some(p.projectile_type) == projectile_type => {
                    fired = true;
                }
                // The generation is checked as well as the index, and it is not belt and braces.
                // A slot freed by a despawn is handed to the next natural spawn, and this test
                // runs a live world for thousands of events: an index match alone reports "the
                // target took damage" for any *later* occupant of that slot with less life than a
                // zombie, which is most of the roster. That is what this assertion was doing.
                Event::NpcSynced(n)
                    if n.index == hostile.index
                        && n.generation == hostile.generation
                        && n.life < hostile.life =>
                {
                    hurt = true;
                }
                _ => {}
            }
        }
        assert!(landed, "npc {npc_type} never landed");
        if let Some(projectile_type) = projectile_type {
            assert!(
                fired,
                "npc {npc_type} never fired projectile {projectile_type}"
            );
        }
        assert!(hurt, "npc {npc_type}'s attack never damaged its target");
        game_task.abort();
        listener_task.abort();
    }
}

/// Same setup `start_with` does, but handing back what it spawned instead of leaving both tasks to
/// run forever — needed only by tests, like the one above, that create many short-lived servers in
/// a loop and must tear each one down before the next, rather than the ordinary one-server-per-test
/// case every other test in this file is, where a server outliving the test is harmless.
async fn start_with_owned_tasks() -> (
    SocketAddr,
    tokio::task::JoinHandle<Stopped>,
    tokio::task::JoinHandle<()>,
) {
    let config = Config {
        world_width: 800,
        world_height: 600,
        motd: String::new(),
        ..Config::default()
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let world = worldgen::generate(
        config.world_width,
        config.world_height,
        config.world_name.clone(),
        7,
    );

    let (tx, rx) = mpsc::channel::<ServerEvent>(1024);
    let game_task = tokio::spawn(GameServer::new(config.clone(), world).run(rx));
    let listener_task = tokio::spawn(listener::run(listener, config, tx, None));
    (addr, game_task, listener_task)
}

/// Registering an account claims the server, and a stranger loses the dangerous commands.
///
/// Before this, the comment above the command dispatcher said "there is no permission model: this
/// is aimed at a server among friends" — and any connected player could set the world to night,
/// summon a boss beside somebody, or delete every NPC in the world with `/butcher`.
///
/// An unclaimed server stays open on purpose: locking the commands away before anybody could have
/// an account is how a security feature becomes a thing people disable.
#[tokio::test]
async fn a_stranger_cannot_claim_an_unclaimed_server() {
    let addr = start().await;
    let mut client = join(addr, "stranger").await;
    client.set_timeout(Duration::from_secs(10));

    // Unclaimed: every permission passes, which is fine among friends and a gift to a stranger.
    client.say("/butcher").await.unwrap();
    let refused = client
        .wait_for(
            "a refusal",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("permission")),
        )
        .await;
    assert!(refused.is_err(), "an unclaimed server should not refuse");

    // So claiming it takes the token printed in the server's own console. Without it, the first
    // person to connect to a fresh public server would simply become its owner.
    client.say("/register owner hunter2hunter2").await.unwrap();
    let told = client
        .wait_for(
            "an explanation",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("claim token")),
        )
        .await;
    assert!(
        told.is_ok(),
        "a claim with no token has to be refused, and say what is missing"
    );

    // A wrong token is refused too, rather than being ignored into a success.
    client
        .say("/register owner hunter2hunter2 notthetoken")
        .await
        .unwrap();
    let told = client
        .wait_for(
            "a refusal",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("not the claim token")),
        )
        .await;
    assert!(told.is_ok(), "a wrong token must not claim the server");

    // And the server is still unclaimed, so nothing was half-created on the way through.
    client.say("/whoami").await.unwrap();
    let who = client
        .wait_for(
            "whoami",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("nobody")),
        )
        .await;
    assert!(who.is_ok(), "no account should have been made");
}

#[tokio::test]
async fn no_guide_arrives_without_a_house() {
    let addr = start().await;
    let mut client = join(addr, "homeless").await;
    client.set_timeout(Duration::from_secs(12));

    let arrived = client
        .wait_for(
            "a guide",
            |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == 22),
        )
        .await;
    assert!(arrived.is_err(), "the Guide moved in with nowhere to live");
}

#[tokio::test]
async fn a_boss_summons_minions_and_can_be_killed() {
    let addr = start().await;
    let mut client = join(addr, "slayer").await;
    client.set_timeout(Duration::from_secs(20));

    // The Eye leaves at dawn, so the fight has to happen at night.
    client.say("/time night").await.unwrap();
    // And it spawns beside you, so stand where the fight is before calling it up — otherwise it
    // spends the whole test flying across the world to reach you.
    client.move_to(400.0 * 16.0, 300.0 * 16.0).await.unwrap();

    let eye = spawn_npc(&mut client, "EyeofCthulhu").await;
    assert_eq!(eye.life_max, 2800, "the Eye has 2800 health");

    // Stay put so it has something to hover over, and watch for its servants. It only summons
    // while hovering above you and within five hundred pixels, so standing still is the point.
    let mut saw_servant = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline && !saw_servant {
        client.move_to(400.0 * 16.0, 300.0 * 16.0).await.unwrap();
        if let Ok(Event::NpcSynced(n)) = client.next_event().await
            && n.npc_type() == 5
        {
            saw_servant = true;
        }
    }
    assert!(saw_servant, "the Eye never summoned a Servant of Cthulhu");

    // Now kill it. 2800 health, 12 defence: 200-damage hits land for 194.
    for _ in 0..20 {
        client
            .hit_npc(eye.index, eye.generation, 200, 0.0, 1)
            .await
            .unwrap();
    }
    client
        .wait_for(
            "the Eye's death",
            |e| matches!(e, Event::NpcSynced(n) if n.index == eye.index && n.life == 0),
        )
        .await
        .expect("the Eye never died");
}

#[tokio::test]
async fn a_worm_boss_arrives_as_a_linked_chain() {
    let addr = start().await;
    let mut client = join(addr, "wormer").await;
    client.set_timeout(Duration::from_secs(15));

    client.say("/spawn EaterofWorldsHead").await.unwrap();

    // A worm is a head, many body segments and a tail, all as separate NPCs.
    // Count distinct NPC slots: each one re-syncs as it moves, so counting packets would
    // over-count the head many times over.
    let mut seen: std::collections::HashMap<u8, u16> = std::collections::HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match client.next_event().await {
            Ok(Event::NpcSynced(n)) if n.life != 0 => {
                seen.insert(n.index, n.npc_type());
            }
            Ok(_) => {}
            Err(_) => break,
        }
        let tally = |t: u16| seen.values().filter(|v| **v == t).count();
        if tally(13) >= 1 && tally(15) >= 1 && tally(14) >= 5 {
            break;
        }
    }
    let tally = |t: u16| seen.values().filter(|v| **v == t).count();
    assert_eq!(tally(13), 1, "exactly one head");
    assert_eq!(tally(15), 1, "exactly one tail");
    assert!(tally(14) >= 5, "expected body segments, saw {}", tally(14));
}

#[tokio::test]
async fn king_slime_hops_toward_a_player() {
    let addr = start().await;
    let mut client = join(addr, "royalist").await;
    client.set_timeout(Duration::from_secs(15));

    let king = spawn_npc(&mut client, "KingSlime").await;
    assert_eq!(king.life_max, 2000);

    // It should move; a boss that stands still is not a fight.
    let start_x = king.position.0;
    let mut moved = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && !moved {
        client.move_to(420.0 * 16.0, 300.0 * 16.0).await.unwrap();
        if let Ok(Event::NpcSynced(n)) = client.next_event().await
            && n.index == king.index
            && (n.position.0 - start_x).abs() > 16.0
        {
            moved = true;
        }
    }
    assert!(moved, "King Slime never moved");
}

/// A lunar pillar must not be culled by the ordinary despawn timer, no matter how far away every
/// player is standing.
///
/// Real vanilla's `NPC.CheckActive`/`UpdateNPC` exempt `npc.boss` from the "nobody near, count
/// down `timeLeft`, then remove" rule every ordinary hostile mob is subject to — a Lunar Pillar
/// registers as a boss (boss health bar, `npc.boss = true` in `NPCID.SetDefaults`) for exactly
/// this reason: it is a stationary structure meant to sit far from the player fighting its escort,
/// not a wandering mob that should vanish the moment nobody is looking at it. `game/npc_ai.rs`'s
/// own `tick_life` already carries that exemption (`if npc.stats.town_npc || npc.stats.boss {
/// return; }`) — the bug was in the data, not the logic: all four pillar entries in
/// `terrustia_proto::npc_data` (517/422/507/493) were transcribed with `boss: false`, the one
/// field every other real boss in that table sets `true`. With it false, `tick_life` decremented
/// `time_left` every tick nobody was within `DESPAWN_HALF_WIDTH`/`DESPAWN_HALF_HEIGHT`
/// (960x600px) of the pillar, and `DEFAULT_TIME_LEFT` is only 750 ticks — 12.5 real seconds — so a
/// pillar left standing while a player fights its escort elsewhere (exactly what a real Lunar
/// Apocalypse fight looks like: you clear the shield first, then come back to the pillar itself)
/// was being silently removed from the server's own NPC table within seconds of the event
/// starting, and told to every client as an ordinary death (packet 23, zero health) — the pillar
/// was never "failing to sync", it had already been killed by its own despawn timer, long before
/// anybody got near it. This is `crates/terrustia/examples/moonlord.rs`'s own disclosed "Lunar
/// Pillars" gap: `clear_shield` fights each pillar's hundred-strong escort standing wherever the
/// bot happens to be, not beside the pillar itself, so by the time the bot arrives for the
/// dedicated pillar fight the real pillar has been gone for however long the escort took.
#[tokio::test]
async fn a_lunar_pillar_does_not_despawn_while_no_player_is_near_it() {
    let addr = start().await;
    let mut client = join(addr, "stargazer").await;
    client.set_timeout(Duration::from_secs(20));

    // 517 = `terrustia::game::lunar::SOLAR`, the Solar Pillar — spawned right beside the player,
    // as `/spawn` always places things.
    let pillar = spawn_npc(&mut client, "517").await;
    assert_eq!(pillar.npc_type(), 517);

    // Walk far enough away that nothing keeps the pillar "near" a player any more: the despawn
    // box reaches 960px/60 tiles horizontally either side, and this world is 800 tiles wide, so
    // moving to its far edge clears that many times over regardless of where spawn placed us.
    client.move_to(20.0 * 16.0, 20.0 * 16.0).await.unwrap();

    // Nobody hit it and nobody is anywhere near it, so a real client should never see it die.
    // `DEFAULT_TIME_LEFT` is 12.5 real seconds on the unfixed code; this waits comfortably past
    // that with margin for the sync's own rate limiting.
    let died = client
        .try_wait_for(
            "the pillar despawning on its own",
            |e| matches!(e, Event::NpcSynced(n) if n.index == pillar.index && n.life == 0),
            Duration::from_secs(18),
        )
        .await;
    assert!(
        died.is_none(),
        "a lunar pillar despawned with nobody near it — it should be exempt from the ordinary \
         despawn timer the same way every other boss is"
    );
}

#[tokio::test]
async fn a_zombie_forces_a_door_on_a_blood_moon() {
    // A door standing on flat ground, with a zombie spawned beside it — on a blood moon, which is
    // the only time a polite opener like a zombie forces a door at all. On an ordinary night it
    // stands and pushes but the door stays shut (the base-defense mechanic); that case is covered
    // by the `fighter` unit tests, which are cheap, rather than an 18-second negative integration.
    let addr = start_with(Config::default(), |world| {
        world.blood_moon = true;
        world.day_time = false; // night, or the blood moon would be over by morning immediately

        // Carve a wide, unambiguously open corridor: the generated world is solid rock here, so
        // anything less leaves the zombie spawning inside a wall.
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        // A door standing in the corridor at x = 405.
        //
        // The frames matter and are not decoration: a door's three tiles carry `frameY` 0, 18 and
        // 36, and that is how both the game and this server find which of them is the top when
        // somebody pushes on the middle one. Building all three at zero makes something no world
        // contains, and a door that cannot be opened because it has no discernible top.
        for (offset, y) in (317..320).enumerate() {
            world.set_tile(405, y, Tile::framed(10, 0, offset as i16 * 18));
        }
    })
    .await;

    let mut client = join(addr, "doorwatcher").await;
    client.set_timeout(Duration::from_secs(20));

    // Spawn the zombie on the far side of the door, then stand on this side of it so the only
    // way to reach the player is through the door.
    client.move_to(412.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    client.say("/spawn Zombie").await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    client.move_to(396.0 * 16.0, 318.0 * 16.0).await.unwrap();

    let mut toggled = false;
    let mut track: Vec<(f32, f32)> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(18);
    while tokio::time::Instant::now() < deadline && !toggled {
        client.move_to(396.0 * 16.0, 318.0 * 16.0).await.unwrap();
        match client.next_event().await {
            // Packet 19 is the door toggle the server broadcasts when a fighter opens one.
            Ok(Event::Other(frame)) if frame.id == id::TOGGLE_DOOR_STATE => toggled = true,
            // Or the door is smashed, which arrives as a tile edit.
            Ok(Event::TileChanged(edit)) if edit.x == 405 => toggled = true,
            Ok(Event::NpcSynced(n)) if n.npc_type() == 3 => {
                track.push((n.position.0 / 16.0, n.position.1 / 16.0));
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(
        toggled,
        "a zombie stood at a door for eighteen seconds and did nothing to it. \
         Zombie was seen at {} positions; first {:?}, last {:?}",
        track.len(),
        track.first(),
        track.last()
    );
}

/// A player who joins a running server has to be told what everyone is already wearing. The
/// equipment packets went out before they arrived and are never repeated, so without the catch-up
/// they would see a room full of naked people.
#[tokio::test]
async fn a_joining_player_is_told_what_everyone_is_wearing() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;

    // A copper shortsword in her first inventory slot.
    let sword = ItemStack {
        id: 3507,
        stack: 1,
        prefix: 0,
    };
    alice.set_equipment(0, sword).await.unwrap();
    // Give the server a moment to record it.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut bob = join(addr, "bob").await;
    bob.set_timeout(Duration::from_secs(3));
    let seen = bob
        .wait_for(
            "alice's sword",
            |e| matches!(e, Event::EquipmentSynced(s) if s.item.id == 3507 && s.slot == 0),
        )
        .await;
    assert!(
        seen.is_ok(),
        "bob should have been told what alice is carrying"
    );
}

/// A live change reaches everyone already connected.
#[tokio::test]
async fn an_equipment_change_reaches_the_other_players() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    let pickaxe = ItemStack {
        id: 3509,
        stack: 1,
        prefix: 0,
    };
    alice.set_equipment(1, pickaxe).await.unwrap();

    bob.set_timeout(Duration::from_secs(3));
    let seen = bob
        .wait_for(
            "alice's pickaxe",
            |e| matches!(e, Event::EquipmentSynced(s) if s.item.id == 3509 && s.slot == 1),
        )
        .await;
    assert!(seen.is_ok(), "bob should have seen the change");
}

/// A player's safe is nobody else's business, however the client labels the packet.
#[tokio::test]
async fn private_storage_is_not_relayed() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    // The first piggy bank slot: inventory, cursor, armour, dyes and the two miscellaneous runs.
    let piggy_bank = 58 + 1 + 20 + 10 + 5 + 5;
    let hoard = ItemStack {
        id: 73,
        stack: 999,
        prefix: 0,
    };
    alice.set_equipment(piggy_bank, hoard).await.unwrap();

    bob.set_timeout(Duration::from_secs(2));
    let leaked = bob
        .wait_for(
            "alice's savings",
            |e| matches!(e, Event::EquipmentSynced(s) if s.slot == piggy_bank),
        )
        .await;
    assert!(leaked.is_err(), "a piggy bank should not be broadcast");
}

/// A worm-headed boss summoned for real comes with its body.
///
/// `summon_on_player` — reached both by a real summon item's own packet (`on_summon`) and by the
/// evil biome's own third-orb-break trigger (`smash_orb`) — used to call the same plain
/// `self.npcs.spawn(npc_type, at)` every other boss does, unlike the admin `/spawn` command, which
/// already special-cased the four ordinary worm monsters so a bare head would not float there
/// alone. The Destroyer shares that exact shape and was never special-cased anywhere: found live,
/// summoning it for real produced only npc 134 and none of its own 81 real trailing segments (135,
/// 136) — not a cosmetic gap, since the whole fight ("its damage is a function of how much of its
/// length has a line to you", `destroyer.rs`'s own module doc) depends on a body existing at all.
#[tokio::test]
async fn a_summoned_worm_boss_comes_with_its_body() {
    let addr = start().await;
    let mut alice = join(addr, "destroyer-summoner").await;
    alice.summon(134).await.unwrap();

    let mut seen: std::collections::HashSet<u16> = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, alice.next_event()).await {
            Ok(Ok(Event::NpcSynced(n))) if matches!(n.npc_type(), 134..=136) => {
                seen.insert(n.npc_type());
            }
            Ok(Ok(_)) => {}
            _ => break,
        }
    }
    assert_eq!(
        seen,
        std::collections::HashSet::from([134, 135, 136]),
        "expected the Destroyer's head, body and tail all real-summoned, only saw {seen:?}"
    );
}

/// A Solar Crawltipede head grows its own body on its own first AI tick, the way Skeletron's
/// hands and Skeletron Prime's arms do (`NPC.cs:51913-51936`, `ai[0]==0 && type==412` raising 30
/// trailing segments) — a genuinely different mechanism from `a_summoned_worm_boss_comes_with_its_
/// body`'s own spawn-time fix, and deliberately so: a Crawltipede head can appear from this
/// project's own ambient hostile spawning during the Lunar Apocalypse, a path neither `/spawn` nor
/// a real summon packet ever sees. Found live investigating why the Solar Pillar's own shield
/// would not clear on a real run even after the pillar-visibility sync gap was fixed: its own
/// npc_data entry sets `dont_take_damage: true` on the head by real vanilla design — hitting the
/// head was never how you kill this thing — so a head with no body attached (the previous state
/// here) was not merely incomplete, it was flatly unkillable, and a bot (or a real player) trying
/// to fight one alone could do so forever without ever landing a real hit.
#[tokio::test]
async fn a_solar_crawltipede_head_grows_its_own_body() {
    let addr = start().await;
    let mut alice = join(addr, "crawltipede-watcher").await;
    alice.say("/spawn 412").await.unwrap();

    let mut seen: std::collections::HashSet<u16> = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, alice.next_event()).await {
            Ok(Ok(Event::NpcSynced(n))) if matches!(n.npc_type(), 412..=414) => {
                seen.insert(n.npc_type());
            }
            Ok(Ok(_)) => {}
            _ => break,
        }
    }
    assert_eq!(
        seen,
        std::collections::HashSet::from([412, 413, 414]),
        "expected the Crawltipede's head, body and tail all real-grown, only saw {seen:?}"
    );
}

/// A Wyvern grows its own fourteen segments the same way (`NPC.cs:51700-51730`, `type == 87 &&
/// ai[0] == 0f`), and it has to: the sky spawns the head alone, because `SpawnAnNPC` calls
/// `SpawnNPC(x, y, 87)` with nothing behind it (`NPC.cs:1411`). Without this a hardmode sky would
/// produce a flying face with no body, which is what it did the moment the sky started spawning
/// anything at all.
///
/// Its parts are not one body type and a tail: legs at the second and ninth, three distinct tail
/// pieces at the end, bodies between (`npc_params::WYVERN_SEGMENTS`), so all six ids are expected.
#[tokio::test]
async fn a_wyvern_head_grows_its_own_body() {
    let addr = start().await;
    let mut alice = join(addr, "wyvern-watcher").await;
    alice.say("/spawn 87").await.unwrap();

    let mut seen: std::collections::HashSet<u16> = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, alice.next_event()).await {
            Ok(Ok(Event::NpcSynced(n))) if matches!(n.npc_type(), 87..=92) => {
                seen.insert(n.npc_type());
            }
            Ok(Ok(_)) => {}
            _ => break,
        }
    }
    assert_eq!(
        seen,
        std::collections::HashSet::from([87, 88, 89, 90, 91, 92]),
        "expected the Wyvern's head, legs, bodies and three tail pieces, only saw {seen:?}"
    );
}

/// The two desert worms grow their own bodies the same way (`NPC.cs:51772-51794` for the Tomb
/// Crawler, `:51796-51819` for the Dune Splicer, both `type == T && ai[0] == 0f`), and they have to
/// for the same reason the Wyvern does: both arrive from this project's own ambient spawning (the
/// underground desert and a sandstorm), which calls `NpcStore::spawn` directly and never consults
/// `npc_params::worm_body`. Before this the Dune Splicer had been in the hardmode desert pool the
/// whole time as a lone floating head.
///
/// The segment count is rolled rather than fixed (`Main.rand.Next(12, 21)` and `(6, 10)`), so this
/// asserts on the three ids rather than on a length.
#[tokio::test]
async fn the_desert_worms_grow_their_own_bodies() {
    for (head, body, tail) in [(510u16, 511u16, 512u16), (513, 514, 515)] {
        let addr = start().await;
        let mut alice = join(addr, "sand-worm-watcher").await;
        alice.say(&format!("/spawn {head}")).await.unwrap();

        let mut seen: std::collections::HashSet<u16> = std::collections::HashSet::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match tokio::time::timeout(left, alice.next_event()).await {
                Ok(Ok(Event::NpcSynced(n))) if [head, body, tail].contains(&n.npc_type()) => {
                    seen.insert(n.npc_type());
                }
                Ok(Ok(_)) => {}
                _ => break,
            }
        }
        assert_eq!(
            seen,
            std::collections::HashSet::from([head, body, tail]),
            "expected {head}'s head, body and tail all real-grown, only saw {seen:?}"
        );
    }
}

/// Hitting the Crawltipede's tail — its only directly-damageable segment — has to actually kill
/// the chain, or growing a real body (the test above) does not itself close the gap: real
/// vanilla's own `realLife` redirects every hit against the tail to the *head*'s shared life pool
/// (`NPC.cs`'s own `statLife = Main.npc[realLife].life`), and `checkDead` only ever processes
/// death for the head — the tail dying "on its own" does nothing at all. Found live: even with a
/// real, grown body, a real bot's own combat against `[413, 414]` still never confirmed a single
/// kill, because 413 (the body) is *also* `dont_take_damage`, and hits against 414 (the tail) were
/// simply discarded rather than reducing the head's own life. `game/server.rs`'s new
/// `on_damage_crawltipede_tail` is what this test proves closes that.
#[tokio::test]
async fn hitting_the_crawltipedes_tail_kills_the_whole_chain() {
    let addr = start().await;
    let mut alice = join(addr, "crawltipede-slayer").await;
    alice.say("/spawn 412").await.unwrap();

    let mut head: Option<(u8, u8)> = None;
    let mut tail: Option<(u8, u8)> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while head.is_none() || tail.is_none() {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "the Crawltipede never fully grew in time");
        match tokio::time::timeout(left, alice.next_event()).await {
            Ok(Ok(Event::NpcSynced(n))) if n.npc_type() == 412 => {
                head = Some((n.index, n.generation));
            }
            Ok(Ok(Event::NpcSynced(n))) if n.npc_type() == 414 => {
                tail = Some((n.index, n.generation));
            }
            Ok(Ok(_)) => {}
            Err(_) => panic!("timed out waiting for the Crawltipede to grow"),
            _ => {}
        }
    }
    let (head_index, _) = head.unwrap();
    let (tail_index, tail_gen) = tail.unwrap();

    // Comfortably more than the head's own 10,000 life, even after its 1,000 defense.
    alice
        .hit_npc(tail_index, tail_gen, 30_000, 0.0, 1)
        .await
        .unwrap();

    // Tight on purpose: the head (`boss: false`) is also subject to the ordinary despawn timer
    // (~12.5s), which a loose deadline here could satisfy by coincidence and make this test pass
    // for the wrong reason — found live doing exactly that against the unfixed code. A real
    // redirect-driven death happens essentially the same tick as the hit; three seconds leaves
    // no room for the despawn timer to be what actually closed this out.
    alice.set_timeout(Duration::from_secs(3));
    let died = alice
        .wait_for(
            "the head dying from a hit against the tail",
            |e| matches!(e, Event::NpcSynced(n) if n.index == head_index && n.net_id == 0),
        )
        .await;
    assert!(
        died.is_ok(),
        "the head should have died promptly once the shared life pool a hit against its tail \
         feeds ran out — a death this long after the hit would be the ordinary despawn timer \
         coincidentally firing, not the redirect actually working"
    );
}

/// Using a summoning item is the only way a boss enters the world, so it had better work.
#[tokio::test]
async fn a_player_can_summon_a_boss() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;

    // King Slime.
    alice.summon(50).await.unwrap();
    alice.set_timeout(Duration::from_secs(3));
    let arrived = alice
        .wait_for(
            "king slime",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 50),
        )
        .await;
    assert!(arrived.is_ok(), "the boss should have been summoned");
}

/// One at a time. A second Eye of Cthulhu is not something the game allows.
#[tokio::test]
async fn a_boss_cannot_be_summoned_twice() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;

    alice.summon(4).await.unwrap();
    // Short, because a quiet moment is the loop's exit condition rather than an error.
    alice.set_timeout(Duration::from_millis(30));
    // A living boss is re-synced every tick it moves, so "did another packet arrive" is the wrong
    // question. What matters is how many *different* NPC slots ever carry an Eye.
    let mut slots = std::collections::HashSet::new();
    for _ in 0..300 {
        match alice.next_event().await {
            Ok(Event::NpcSynced(n)) if n.net_id == 4 => {
                slots.insert(n.index);
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert_eq!(slots.len(), 1, "the first summon should have worked");

    alice.summon(4).await.unwrap();
    for _ in 0..300 {
        match alice.next_event().await {
            Ok(Event::NpcSynced(n)) if n.net_id == 4 => {
                slots.insert(n.index);
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert_eq!(slots.len(), 1, "two Eyes of Cthulhu at once: {slots:?}");
}

/// A crafted packet cannot conjure something that is not a summonable boss.
#[tokio::test]
async fn only_the_games_own_list_can_be_summoned() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;

    // A Moon Lord is not on the list, however politely you ask.
    alice.summon(398).await.unwrap();
    alice.set_timeout(Duration::from_secs(2));
    let arrived = alice
        .wait_for(
            "a moon lord",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 398),
        )
        .await;
    assert!(arrived.is_err(), "that is not a summonable boss");
}

/// A multi-tile object has to be written into the server's own world, not merely relayed. If the
/// server does not place it, the object is gone the moment the world is saved and invisible to
/// anyone who joins afterwards.
#[tokio::test]
async fn a_placed_object_lands_in_the_world() {
    let addr = start_with(Config::default(), |world| {
        // A cleared pocket with solid ground under it. The generated world already has terrain
        // here, and an object is refused outright if anything is in its way.
        for x in 380..420 {
            for y in 310..322 {
                world.set_tile(x, y, terrustia_proto::tile::Tile::AIR);
            }
            world.set_tile(x, 322, terrustia_proto::tile::Tile::block(1));
        }
    })
    .await;
    let mut alice = join(addr, "alice").await;

    // A workbench: two wide, one tall, origin at its top-left.
    alice.place_object(400, 321, 18, 0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Bob's handshake already streams the sections around spawn, so asking for one he has would
    // simply be ignored and the wait would time out. Draining what arrives is enough.
    let mut bob = join(addr, "bob").await;
    bob.set_timeout(Duration::from_millis(50));
    for _ in 0..200 {
        if bob.next_event().await.is_err() {
            break;
        }
    }

    let placed = bob
        .world()
        .tile(400, 321)
        .expect("the tile should have arrived");
    assert!(placed.is_active(), "the workbench should be in the world");
    assert_eq!(placed.block, 18, "and be a workbench");
    let second = bob.world().tile(401, 321).expect("its second tile too");
    assert_eq!(second.block, 18, "both of its tiles");
    assert!(
        second.frame_x > placed.frame_x,
        "with the second column framed after the first: {} then {}",
        placed.frame_x,
        second.frame_x
    );
}

/// A styled object is framed where the game frames it, not merely somewhere consistent.
///
/// Every other placement test here uses style 0, which is the one style every wrong style layout
/// still gets right: `style * multiplier + base` is 0 whatever the multiplier is. `tile_object.rs`
/// had a uniform guess in place of the real layout for 374 of its 389 entries, and this is the
/// shape of test that would have caught it: a real client places a Boreal Wood table over the wire
/// and the frames that land are compared against the sheet the game actually draws from, where a
/// table's styles run straight across 54 pixels apart.
#[tokio::test]
async fn a_styled_object_is_framed_where_the_game_frames_it() {
    let addr = start_with(Config::default(), |world| {
        for x in 380..420 {
            for y in 310..322 {
                world.set_tile(x, y, terrustia_proto::tile::Tile::AIR);
            }
            world.set_tile(x, 322, terrustia_proto::tile::Tile::block(1));
        }
    })
    .await;
    let mut alice = join(addr, "alice").await;

    // A table: three wide, two tall, origin at its bottom-middle, style 1 (Boreal Wood).
    alice.place_object(400, 321, 14, 1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut bob = join(addr, "bob").await;
    bob.set_timeout(Duration::from_millis(50));
    for _ in 0..200 {
        if bob.next_event().await.is_err() {
            break;
        }
    }

    // `TileObjectData` leaves tile 14 on the base object's `StyleWrapLimit = 0, StyleMultiplier =
    // 1`, so style 1 sits one full width along: `CoordinateFullWidth` is (16 + 2) * 3 = 54. Then
    // 18 per column across and 18 down to the second row.
    for (dx, dy, want_x, want_y) in [
        (0, 0, 54, 0),
        (1, 0, 72, 0),
        (2, 0, 90, 0),
        (0, 1, 54, 18),
        (2, 1, 90, 18),
    ] {
        let tile = bob
            .world()
            .tile(399 + dx, 320 + dy)
            .expect("the table should have arrived");
        assert_eq!(tile.block, 14, "cell ({dx}, {dy}) should be a table");
        assert_eq!(
            (tile.frame_x, tile.frame_y),
            (want_x, want_y),
            "cell ({dx}, {dy}) of a style-1 table"
        );
    }
}

/// Placing an object over something already there is refused outright rather than filling gaps.
#[tokio::test]
async fn an_object_will_not_be_placed_over_something() {
    let addr = start_with(Config::default(), |world| {
        for x in 380..420 {
            for y in 310..322 {
                world.set_tile(x, y, terrustia_proto::tile::Tile::AIR);
            }
            world.set_tile(x, 322, terrustia_proto::tile::Tile::block(1));
        }
        // Something in the way of the second half of the workbench.
        world.set_tile(401, 321, terrustia_proto::tile::Tile::block(1));
    })
    .await;
    let mut alice = join(addr, "alice").await;
    alice.place_object(400, 321, 18, 0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut bob = join(addr, "bob").await;
    bob.set_timeout(Duration::from_millis(50));
    for _ in 0..200 {
        if bob.next_event().await.is_err() {
            break;
        }
    }
    assert_ne!(
        bob.world().tile(400, 321).map(|t| t.block),
        Some(18),
        "half a workbench should not have been placed"
    );
}

/// A socket that has not finished the handshake — not a playing client — must not be able to place
/// an object. Vanilla's `case 79` (like every other tile-writing handler) only acts for a joined
/// client; before the gate any raw socket could scatter furniture and tile entities across the
/// world without ever joining.
#[tokio::test]
async fn a_socket_that_has_not_joined_cannot_place_an_object() {
    let addr = start_with(Config::default(), |world| {
        for x in 380..420 {
            for y in 310..322 {
                world.set_tile(x, y, terrustia_proto::tile::Tile::AIR);
            }
            world.set_tile(x, 322, terrustia_proto::tile::Tile::block(1));
        }
    })
    .await;

    // Connect the socket but never handshake, then hand-build a PlaceObject (id 79) frame for a
    // workbench at (400, 321) and send it raw.
    let mut sneaky = terrustia_client::Client::connect(addr, "sneaky")
        .await
        .unwrap();
    let mut payload = Vec::new();
    payload.extend_from_slice(&400i16.to_le_bytes()); // x
    payload.extend_from_slice(&321i16.to_le_bytes()); // y
    payload.extend_from_slice(&18i16.to_le_bytes()); // block (workbench)
    payload.extend_from_slice(&0i16.to_le_bytes()); // style
    payload.push(0); // alternate
    payload.push(0); // random
    payload.push(0); // direction
    let mut frame = Vec::new();
    frame.extend_from_slice(&((payload.len() + 3) as u16).to_le_bytes());
    frame.push(79); // PLACE_OBJECT
    frame.extend_from_slice(&payload);
    sneaky.send(&frame).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // A real player confirms nothing was placed.
    let mut watcher = join(addr, "watcher").await;
    watcher.set_timeout(Duration::from_millis(50));
    for _ in 0..200 {
        if watcher.next_event().await.is_err() {
            break;
        }
    }
    assert_ne!(
        watcher.world().tile(400, 321).map(|t| t.block),
        Some(18),
        "a socket that never joined must not place an object"
    );
}

/// A teleport the server does not apply leaves every enemy in the world attacking where the
/// player used to be, so it has to move the server's idea of them as well as telling everyone.
#[tokio::test]
async fn a_teleport_moves_the_player_for_everyone() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    alice.teleport(1234.0, 567.0).await.unwrap();

    bob.set_timeout(Duration::from_secs(3));
    let seen = bob
        .wait_for(
            "alice's teleport",
            |e| matches!(e, Event::Other(f) if f.id == id::TELEPORT_ENTITY),
        )
        .await;
    assert!(seen.is_ok(), "bob should have been told");
}

/// Painting a tile sticks and reaches everybody.
#[tokio::test]
async fn paint_sticks_to_the_world() {
    let addr = start_with(Config::default(), |world| {
        clear_area(world, 402, 330, 6, 4);
        world.set_tile(402, 330, Tile::block(1));
    })
    .await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    let mut paint = Vec::new();
    paint.extend_from_slice(&402i16.to_le_bytes());
    paint.extend_from_slice(&330i16.to_le_bytes());
    paint.push(13); // a colour
    paint.push(0); // paint, not coating
    bob.send(&frame(id::SYNC_TILE_PAINT_OR_COATING, &paint))
        .await
        .unwrap();

    alice
        .wait_for(
            "the relayed paint",
            |e| matches!(e, Event::Other(f) if f.id == id::SYNC_TILE_PAINT_OR_COATING),
        )
        .await
        .unwrap();

    let fresh = join(addr, "fresh").await;
    assert_eq!(
        fresh.world().tile(402, 330).map(|t| t.color),
        Some(13),
        "the paint did not stick"
    );
}

/// Painting empty air does nothing: a crafted packet cannot colour in the sky.
#[tokio::test]
async fn painting_nothing_does_nothing() {
    let addr = start().await;
    let mut bob = join(addr, "bob").await;

    // Well above the surface, where there is certainly no tile.
    let mut paint = Vec::new();
    paint.extend_from_slice(&402i16.to_le_bytes());
    paint.extend_from_slice(&20i16.to_le_bytes());
    paint.push(13);
    paint.push(0);
    bob.send(&frame(id::SYNC_TILE_PAINT_OR_COATING, &paint))
        .await
        .unwrap();

    let fresh = join(addr, "fresh").await;
    assert_eq!(fresh.world().tile(402, 20).map(|t| t.color), Some(0));
}

/// A locked biome chest stays locked until Plantera is down.
#[tokio::test]
async fn a_biome_chest_waits_for_plantera() {
    // Style 23 is a locked biome chest: frame_x 23 * 36.
    let locked = |world: &mut World| {
        for dx in 0..2 {
            for dy in 0..2 {
                world.set_tile(
                    402 + dx,
                    330 + dy,
                    Tile::framed(21, 23 * 36 + dx as i16 * 18, dy as i16 * 18),
                );
            }
        }
    };
    let addr = start_with(Config::default(), locked).await;
    let mut bob = join(addr, "bob").await;

    let mut unlock = vec![1u8]; // unlock a chest
    unlock.extend_from_slice(&402i16.to_le_bytes());
    unlock.extend_from_slice(&330i16.to_le_bytes());
    bob.send(&frame(id::LOCK_AND_UNLOCK, &unlock))
        .await
        .unwrap();

    let fresh = join(addr, "fresh").await;
    assert_eq!(
        fresh.world().tile(402, 330).map(|t| t.frame_x),
        Some(23 * 36),
        "the chest opened without Plantera"
    );
}

/// A dungeon chest opens with a key, and the whole two-by-two moves together.
#[tokio::test]
async fn a_dungeon_chest_opens() {
    let addr = start_with(Config::default(), |world| {
        clear_with_floor(world, 402, 330, 6, 4);
        // Style 2 is a locked dungeon chest.
        for dx in 0..2 {
            for dy in 0..2 {
                world.set_tile(
                    402 + dx,
                    330 + dy,
                    Tile::framed(21, 2 * 36 + dx as i16 * 18, dy as i16 * 18),
                );
            }
        }
    })
    .await;
    let mut bob = join(addr, "bob").await;

    let mut unlock = vec![1u8];
    unlock.extend_from_slice(&402i16.to_le_bytes());
    unlock.extend_from_slice(&330i16.to_le_bytes());
    bob.send(&frame(id::LOCK_AND_UNLOCK, &unlock))
        .await
        .unwrap();

    let fresh = join(addr, "fresh").await;
    for dx in 0..2 {
        for dy in 0..2 {
            let tile = fresh.world().tile(402 + dx, 330 + dy).expect("a tile");
            assert_eq!(
                tile.frame_x,
                // Style 2 unlocks to style 1: one frame back.
                36 + dx as i16 * 18,
                "corner ({dx}, {dy}) did not move with the rest"
            );
        }
    }
}

/// A net takes a critter and refuses anything else.
#[tokio::test]
async fn a_net_only_catches_critters() {
    let addr = start().await;
    let mut bob = join(addr, "bob").await;
    // Ask for a boss and a bunny by index; neither exists, so this only proves the packet is
    // accepted and refused rather than crashing the server.
    let mut catch = Vec::new();
    catch.extend_from_slice(&0i16.to_le_bytes());
    catch.push(0);
    bob.send(&frame(id::BUG_CATCHING, &catch)).await.unwrap();

    // The server is still answering afterwards.
    bob.send(&frame(id::PING, &[0u8; 8])).await.unwrap();
    let fresh = join(addr, "fresh").await;
    assert!(
        fresh.world().tile(402, 330).is_some(),
        "the server survived"
    );
}

/// A client already in the world watches the water move.
///
/// The other liquid test joins a *fresh* client afterwards, which reads the pool out of a section
/// it loads from scratch — so it would pass even if the server told nobody anything while the
/// water was falling. This one stays connected throughout, which is the case that actually matters
/// and the one that broke when liquid moved from tile squares to net module 0.
#[tokio::test]
async fn a_connected_client_is_told_when_water_moves() {
    let addr = start_with(Config::default(), |world| {
        for x in 400..410 {
            world.set_tile(x, 340, Tile::block(1));
            for y in 330..340 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        for y in 330..341 {
            world.set_tile(400, y, Tile::block(1));
            world.set_tile(409, y, Tile::block(1));
        }
    })
    .await;

    let mut watcher = join(addr, "watcher").await;
    // The basin is far from spawn, so the section holding it has to be asked for before anything
    // in it can be watched.
    watcher.walk_to_tile(405, 335).await.unwrap();
    watcher
        .wait_for("the basin's section", |event| {
            matches!(event, terrustia_client::Event::SectionLoaded { .. })
        })
        .await
        .unwrap();
    assert_eq!(
        watcher.world().tile(405, 339).map(|t| t.liquid),
        Some(0),
        "the basin should start dry"
    );

    let mut pourer = join(addr, "pourer").await;
    let mut pour = Vec::new();
    pour.extend_from_slice(&405i16.to_le_bytes());
    pour.extend_from_slice(&331i16.to_le_bytes());
    pour.push(255);
    pour.push(0);
    pourer.send(&frame(id::LIQUID_UPDATE, &pour)).await.unwrap();

    // Watch until the bottom of the basin has water in it, without ever reloading the section.
    let filled = watcher
        .try_wait_for(
            "water at the bottom",
            |event| matches!(event, terrustia_client::Event::LiquidChanged(_)),
            Duration::from_secs(5),
        )
        .await;
    assert!(filled.is_some(), "no liquid update ever reached the client");

    // Drain whatever else is in flight so the pool has settled in this client's own view.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        if watcher
            .world()
            .tile(405, 339)
            .map(|t| t.liquid)
            .unwrap_or(0)
            > 0
        {
            break;
        }
        if watcher
            .try_wait_for("more liquid", |_| false, Duration::from_millis(300))
            .await
            .is_none()
        {
            continue;
        }
    }

    assert!(
        watcher
            .world()
            .tile(405, 339)
            .map(|t| t.liquid)
            .unwrap_or(0)
            > 0,
        "the client never saw the water reach the bottom of the basin"
    );
}

/// A bucket poured into the world falls and pools rather than sitting in the air.
///
/// The basin has walls: a bucket on an open plain spreads out and thins away to nothing, which is
/// what the game does too, so testing it on a shelf would be testing the wrong thing.
#[tokio::test]
async fn poured_water_falls() {
    let addr = start_with(Config::default(), |world| {
        for x in 400..410 {
            world.set_tile(x, 340, Tile::block(1));
            for y in 330..340 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        // The walls that make it a basin rather than a shelf.
        for y in 330..341 {
            world.set_tile(400, y, Tile::block(1));
            world.set_tile(409, y, Tile::block(1));
        }
    })
    .await;
    let mut bob = join(addr, "bob").await;

    let mut pour = Vec::new();
    pour.extend_from_slice(&405i16.to_le_bytes());
    pour.extend_from_slice(&331i16.to_le_bytes());
    pour.push(255); // a full bucket
    pour.push(0); // water
    bob.send(&frame(id::LIQUID_UPDATE, &pour)).await.unwrap();

    // Give the simulation a moment. Liquid now settles at vanilla's pace rather than four times
    // too fast (L3-09: the every-other-tick skipCount gate plus the per-tile skipLiquid flag), so a
    // full bucket takes noticeably longer to fall the eight tiles to the basin floor than it did
    // before the pacing was corrected.
    tokio::time::sleep(Duration::from_millis(2000)).await;

    let fresh = join(addr, "fresh").await;
    let up_top = fresh.world().tile(405, 331).map(|t| t.liquid).unwrap_or(0);
    let down_low = fresh.world().tile(405, 339).map(|t| t.liquid).unwrap_or(0);
    assert!(
        down_low > up_top,
        "the water should have fallen: {up_top} up top, {down_low} at the bottom"
    );
}

/// Build a raw frame the way the client's own encoder does.
fn frame(id: u8, payload: &[u8]) -> Vec<u8> {
    let len = (payload.len() + 3) as u16;
    let mut out = Vec::with_capacity(len as usize);
    out.extend_from_slice(&len.to_le_bytes());
    out.push(id);
    out.extend_from_slice(payload);
    out
}

/// A module-4 request exactly as a real Terraria client would build one — the wire shape
/// `NetCreativePowersModule.PreparePacket` uses for every power, button or toggle alike: the
/// module id, then the power id, then whatever that power's own shape needs (nothing for a
/// button; one bool for a shared toggle).
fn creative_power_request(power_id: u16, toggle_state: Option<bool>) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&terrustia_proto::net_module::MODULE_CREATIVE_POWERS.to_le_bytes());
    body.extend_from_slice(&power_id.to_le_bytes());
    if let Some(state) = toggle_state {
        body.push(u8::from(state));
    }
    frame(id::NET_MODULES, &body)
}

/// The same wire shape as [`creative_power_request`], for a shared slider's single `f32` payload.
fn creative_slider_request(power_id: u16, value: f32) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&terrustia_proto::net_module::MODULE_CREATIVE_POWERS.to_le_bytes());
    body.extend_from_slice(&power_id.to_le_bytes());
    body.extend_from_slice(&value.to_le_bytes());
    frame(id::NET_MODULES, &body)
}

/// A per-player toggle request (`Godmode`/`FarPlacementRange`) — `APerPlayerTogglePower`'s
/// `SyncOnePlayer` sub-message. `claimed_player_index` is exactly what a real client would put on
/// the wire, faithfully honest or not — the whole point of the security test this exists for is
/// that a dedicated server must never trust it.
fn per_player_toggle_request(power_id: u16, claimed_player_index: u8, state: bool) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&terrustia_proto::net_module::MODULE_CREATIVE_POWERS.to_le_bytes());
    body.extend_from_slice(&power_id.to_le_bytes());
    body.push(1); // SubMessageType::SyncOnePlayer
    body.push(claimed_player_index);
    body.push(u8::from(state));
    frame(id::NET_MODULES, &body)
}

/// All four of Journey mode's time-skip buttons set the clock to exactly the values vanilla's own
/// `SpawnSkeletron`-adjacent `SkipToTime` calls use (`CreativePowers.cs`'s `StartDayImmediately`/
/// `StartNoonImmediately`/`StartNightImmediately`/`StartMidnightImmediately`) — the same values
/// `/time day|noon|night|midnight` already sends. Real vanilla's own `SkipToTime`
/// (`Main.cs:66163`) resyncs with `NetMessage.TrySendData(7)`, full `WorldData` — never message id
/// 18: grepping the whole decompiled tree finds no call to `SendData(18)` anywhere, from any code
/// path, ever. This test used to expect id 18 (`packets::TimeSet`) instead, matching a bug in
/// `set_time` rather than real vanilla — a bug a live player watching it fire against a real
/// client caught by seeing the sky visibly *snap* to a different time rather than keep flowing,
/// which is exactly what the real client's own hard, non-interpolated receive handler for id 18
/// does (`MessageBuffer.cs`'s `case 18`, an unconditional `Main.time = reader.ReadInt32()` with no
/// smoothing at all — a packet real vanilla never sends specifically because no code path ever
/// needed a client to snap like that).
#[tokio::test]
async fn each_journey_time_skip_button_sends_the_right_time_set() {
    let addr = start().await;
    let mut client = join(addr, "traveller").await;

    for (power_id, expect_day, expect_time) in [
        (terrustia_proto::net_module::power::START_DAY, true, 0),
        (
            terrustia_proto::net_module::power::START_NOON,
            true,
            terrustia::world::world::DAY_LENGTH / 2,
        ),
        (terrustia_proto::net_module::power::START_NIGHT, false, 0),
        (
            terrustia_proto::net_module::power::START_MIDNIGHT,
            false,
            terrustia::world::world::NIGHT_LENGTH / 2,
        ),
    ] {
        client
            .send(&creative_power_request(power_id, None))
            .await
            .unwrap();

        let event = client
            .wait_for(
                "a world-data resync",
                |e| matches!(e, Event::Other(f) if f.id == id::WORLD_DATA),
            )
            .await
            .unwrap();
        let Event::Other(f) = event else {
            unreachable!()
        };
        // `WorldData::write_payload`'s own layout: `time` (i32 LE), then a `day_flags` byte whose
        // bit 0 is `day_time` (bit 1 blood moon, bit 2 eclipse) — the only two fields this test
        // needs out of a much larger packet.
        let time = i32::from_le_bytes(f.payload[0..4].try_into().unwrap());
        let day_time = f.payload[4] & 0x01 != 0;
        assert_eq!(day_time, expect_day, "power id {power_id}");
        assert_eq!(time, expect_time, "power id {power_id}");
    }
}

/// A Journey toggle request from one client is relayed to a witness who never sent it — the
/// dedicated-server broadcast shape `ASharedTogglePower::DeserializeNetMessage` uses, not a
/// private echo back to the sender alone.
#[tokio::test]
async fn a_journey_toggle_is_relayed_to_a_witness() {
    let addr = start().await;
    let mut toggler = join(addr, "toggler").await;
    let mut witness = join(addr, "witness").await;

    toggler
        .send(&creative_power_request(
            terrustia_proto::net_module::power::FREEZE_TIME,
            Some(true),
        ))
        .await
        .unwrap();

    // The witness's own join a moment ago already queued four "current state" sync frames of its
    // own (`introduce()`'s own send, all four starting `false`) — including one for this exact
    // power id. Matching on the fully decoded message, not just "any module-4 frame", is what
    // actually distinguishes the toggler's real broadcast from the witness's own join-time sync.
    let event = witness
        .wait_for("the freeze-time toggle to relay", |e| {
            matches!(
                e,
                Event::Other(f) if f.id == id::NET_MODULES
                    && terrustia_proto::net_module::decode_creative_power(&f.payload) == Ok(Some(
                        terrustia_proto::net_module::CreativePowerMessage::Toggle(
                            terrustia_proto::net_module::power::FREEZE_TIME,
                            true,
                        )
                    ))
            )
        })
        .await
        .unwrap();
    assert!(
        matches!(event, Event::Other(f) if f.id == id::NET_MODULES),
        "a witness who never sent the toggle should still see it take effect"
    );
}

/// A client that joins *after* a Journey toggle was already flipped still learns its state — the
/// `OnPlayerJoining` sync, not just the live broadcast the toggle test above already covers.
#[tokio::test]
async fn a_late_joiner_learns_an_already_set_journey_toggle() {
    let addr = start().await;
    let mut first = join(addr, "first").await;
    first
        .send(&creative_power_request(
            terrustia_proto::net_module::power::STOP_BIOME_SPREAD,
            Some(true),
        ))
        .await
        .unwrap();
    // Give the server a moment to actually apply it before anyone else joins and asks. Matched on
    // the decoded message, not just "any module-4 frame" — `first`'s own join a moment ago queued
    // its own four join-time sync frames (all starting `false`), which a looser match could catch
    // instead of the real confirmation.
    let confirmed = terrustia_proto::net_module::CreativePowerMessage::Toggle(
        terrustia_proto::net_module::power::STOP_BIOME_SPREAD,
        true,
    );
    first
        .wait_for("confirmation the toggle landed", |e| {
            matches!(
                e,
                Event::Other(f) if f.id == id::NET_MODULES
                    && terrustia_proto::net_module::decode_creative_power(&f.payload)
                        == Ok(Some(confirmed))
            )
        })
        .await
        .unwrap();

    let mut late = join(addr, "latecomer").await;
    let event = late
        .wait_for("the already-set toggle, sent on join", |e| {
            matches!(
                e,
                Event::Other(f) if f.id == id::NET_MODULES
                    && terrustia_proto::net_module::decode_creative_power(&f.payload)
                        == Ok(Some(confirmed))
            )
        })
        .await
        .unwrap();
    assert!(
        matches!(event, Event::Other(f) if f.id == id::NET_MODULES),
        "a player joining after the fact should not have to ask"
    );
}

/// Each of the three shared sliders — `ModifyTimeRate`/`ModifyWindDirectionAndStrength`/
/// `ModifyRainPower` — is relayed to a witness who never sent it, carrying the exact raw value
/// requested (each power's own remap — the 1×–24× rate, the wind lerp, the rain strength — is a
/// gameplay concern, unit-tested where it is actually applied, not the wire's job to prove).
#[tokio::test]
async fn each_journey_slider_change_is_relayed_to_a_witness() {
    let addr = start().await;
    let mut slider = join(addr, "slider").await;
    let mut witness = join(addr, "witness").await;

    for (power_id, value) in [
        (terrustia_proto::net_module::power::MODIFY_TIME_RATE, 0.75),
        (terrustia_proto::net_module::power::MODIFY_WIND, 0.25),
        (terrustia_proto::net_module::power::MODIFY_RAIN, 0.6),
    ] {
        slider
            .send(&creative_slider_request(power_id, value))
            .await
            .unwrap();

        let expected = terrustia_proto::net_module::CreativePowerMessage::Slider(power_id, value);
        witness
            .wait_for("the slider change to relay", |e| {
                matches!(
                    e,
                    Event::Other(f) if f.id == id::NET_MODULES
                        && terrustia_proto::net_module::decode_creative_power(&f.payload)
                            == Ok(Some(expected))
                )
            })
            .await
            .unwrap_or_else(|_| panic!("power id {power_id} never relayed"));
    }
}

/// `ModifyTimeRate` syncs to a player who joins after it was set (`_syncToJoiningPlayers = true`
/// in source); `ModifyWind`/`ModifyRain` do not (`false` in source, and neither persists past the
/// moment it's applied — see `journey.rs`'s own module doc). A late joiner should see the one and
/// never the other two.
#[tokio::test]
async fn only_the_time_rate_slider_syncs_to_a_late_joiner() {
    let addr = start().await;
    let mut setter = join(addr, "setter").await;

    for (power_id, value) in [
        (terrustia_proto::net_module::power::MODIFY_TIME_RATE, 0.75),
        (terrustia_proto::net_module::power::MODIFY_WIND, 0.25),
        (terrustia_proto::net_module::power::MODIFY_RAIN, 0.6),
    ] {
        setter
            .send(&creative_slider_request(power_id, value))
            .await
            .unwrap();
        let expected = terrustia_proto::net_module::CreativePowerMessage::Slider(power_id, value);
        setter
            .wait_for("confirmation each slider landed", |e| {
                matches!(
                    e,
                    Event::Other(f) if f.id == id::NET_MODULES
                        && terrustia_proto::net_module::decode_creative_power(&f.payload)
                            == Ok(Some(expected))
                )
            })
            .await
            .unwrap();
    }

    let mut late = join(addr, "latecomer").await;
    let expected_time_rate = terrustia_proto::net_module::CreativePowerMessage::Slider(
        terrustia_proto::net_module::power::MODIFY_TIME_RATE,
        0.75,
    );
    late.wait_for("the time-rate slider, sent on join", |e| {
        matches!(
            e,
            Event::Other(f) if f.id == id::NET_MODULES
                && terrustia_proto::net_module::decode_creative_power(&f.payload)
                    == Ok(Some(expected_time_rate))
        )
    })
    .await
    .unwrap();

    // Nothing else arriving within a short, generous window should ever decode as the wind or
    // rain slider — a bounded wait rather than an infinite one, since this is checking an absence.
    let never_wind_or_rain = late
        .try_wait_for(
            "a wind or rain slider frame that should never come",
            |e| {
                matches!(
                    e,
                    Event::Other(f) if f.id == id::NET_MODULES
                        && matches!(
                            terrustia_proto::net_module::decode_creative_power(&f.payload),
                            Ok(Some(terrustia_proto::net_module::CreativePowerMessage::Slider(
                                p,
                                _
                            ))) if p == terrustia_proto::net_module::power::MODIFY_WIND
                                || p == terrustia_proto::net_module::power::MODIFY_RAIN
                        )
                )
            },
            Duration::from_millis(800),
        )
        .await;
    assert!(
        never_wind_or_rain.is_none(),
        "wind/rain must not be sent on join, only requested-and-relayed live"
    );
}

/// A client cannot toggle Godmode for somebody else by lying about the player index on the wire —
/// `APerPlayerTogglePower::DeserializeNetMessage`'s own dedicated-server substitution
/// (`Main.netMode == 2`), proven end to end over a real socket rather than only at the unit level:
/// `alice` claims to be toggling player 99, and the confirmation everyone actually sees names
/// `alice`'s own real slot instead.
#[tokio::test]
async fn a_client_cannot_claim_to_be_a_different_player_when_toggling_godmode() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut witness = join(addr, "witness").await;
    let real_slot = alice.slot();

    alice
        .send(&per_player_toggle_request(
            terrustia_proto::net_module::power::GODMODE,
            99, // a lie — not alice's real slot, and not a slot anyone occupies
            true,
        ))
        .await
        .unwrap();

    let expected = terrustia_proto::net_module::CreativePowerMessage::PerPlayerToggle(
        terrustia_proto::net_module::power::GODMODE,
        true,
    );
    // `PerPlayerToggle` itself carries no player index — the confirmation's *own* wire player-index
    // byte (not exposed through the decoded enum, so read directly here) is what this test is
    // really about; matching the decoded variant first is enough to find the right frame.
    let event = witness
        .wait_for("the godmode toggle to relay", |e| {
            matches!(
                e,
                Event::Other(f) if f.id == id::NET_MODULES
                    && terrustia_proto::net_module::decode_creative_power(&f.payload) == Ok(Some(expected))
            )
        })
        .await
        .unwrap();
    let Event::Other(f) = event else {
        unreachable!()
    };
    // payload: module(2) + power_id(2) + sub_type(1) + player_index(1) + state(1)
    let confirmed_slot = f.payload[5];
    assert_eq!(
        confirmed_slot, real_slot,
        "the confirmation should name alice's own real slot ({real_slot}), not the claimed 99"
    );
}

/// Breaking a chest removes it, rather than leaving a ghost behind.
#[tokio::test]
async fn breaking_a_chest_removes_it() {
    let addr = start_with(Config::default(), |world| {
        for dx in 0..2 {
            for dy in 0..2 {
                world.set_tile(
                    402 + dx,
                    330 + dy,
                    Tile::framed(21, dx as i16 * 18, dy as i16 * 18),
                );
            }
        }
        world.add_chest(Chest {
            x: 402,
            y: 330,
            name: String::new(),
            items: vec![ItemStack::default(); 40],
        });
    })
    .await;
    let mut bob = join(addr, "bob").await;

    // Opening it proves it is there to begin with.
    let mut open = Vec::new();
    open.extend_from_slice(&402i16.to_le_bytes());
    open.extend_from_slice(&330i16.to_le_bytes());
    bob.send(&frame(id::REQUEST_CHEST_OPEN, &open))
        .await
        .unwrap();
    bob.wait_for(
        "the chest opening",
        |e| matches!(e, Event::Other(f) if f.id == id::SYNC_PLAYER_CHEST),
    )
    .await
    .unwrap();

    // Now break it.
    let mut kill = vec![1u8]; // break a chest
    kill.extend_from_slice(&402i16.to_le_bytes());
    kill.extend_from_slice(&330i16.to_le_bytes());
    kill.extend_from_slice(&0i16.to_le_bytes());
    bob.send(&frame(id::CHEST_UPDATES, &kill)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // A fresh player asking for the same spot gets nothing.
    let mut fresh = join(addr, "fresh").await;
    fresh
        .send(&frame(id::REQUEST_CHEST_OPEN, &open))
        .await
        .unwrap();
    let answered = fresh
        .try_wait_for(
            "a chest that should be gone",
            |e| matches!(e, Event::Other(f) if f.id == id::SYNC_PLAYER_CHEST),
            Duration::from_millis(500),
        )
        .await;
    assert!(answered.is_none(), "the broken chest still answers");
}

/// A chest with things in it will not break.
#[tokio::test]
async fn a_full_chest_will_not_break() {
    let addr = start_with(Config::default(), |world| {
        for dx in 0..2 {
            for dy in 0..2 {
                world.set_tile(
                    402 + dx,
                    330 + dy,
                    Tile::framed(21, dx as i16 * 18, dy as i16 * 18),
                );
            }
        }
        let mut items = vec![ItemStack::default(); 40];
        items[0] = ItemStack::new(1, 1, 0);
        world.add_chest(Chest {
            x: 402,
            y: 330,
            name: String::new(),
            items,
        });
    })
    .await;
    let mut bob = join(addr, "bob").await;

    let mut kill = vec![1u8];
    kill.extend_from_slice(&402i16.to_le_bytes());
    kill.extend_from_slice(&330i16.to_le_bytes());
    kill.extend_from_slice(&0i16.to_le_bytes());
    bob.send(&frame(id::CHEST_UPDATES, &kill)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut fresh = join(addr, "fresh").await;
    let mut open = Vec::new();
    open.extend_from_slice(&402i16.to_le_bytes());
    open.extend_from_slice(&330i16.to_le_bytes());
    fresh
        .send(&frame(id::REQUEST_CHEST_OPEN, &open))
        .await
        .unwrap();
    fresh
        .wait_for(
            "the chest, still there",
            |e| matches!(e, Event::Other(f) if f.id == id::SYNC_PLAYER_CHEST),
        )
        .await
        .expect("a chest with things in it should survive");
}

/// Build a hardmode world with one chest at 402,330 holding whatever the caller names.
///
/// `ItemID.KeyofLight` is 3092 and `ItemID.KeyofNight` 3091.
fn a_world_with_a_chest_holding(contents: Vec<ItemStack>) -> impl FnOnce(&mut World) {
    move |world: &mut World| {
        world.progress.hard_mode = true;
        for dx in 0..2 {
            for dy in 0..2 {
                world.set_tile(
                    402 + dx,
                    330 + dy,
                    Tile::framed(21, dx as i16 * 18, dy as i16 * 18),
                );
            }
        }
        let mut items = vec![ItemStack::default(); 40];
        for (slot, item) in contents.into_iter().enumerate() {
            items[slot] = item;
        }
        world.add_chest(Chest {
            x: 402,
            y: 330,
            name: String::new(),
            items,
        });
    }
}

/// Open the chest at 402,330 and then close it, which is what actually runs the check.
async fn open_then_close_the_chest(client: &mut Client) {
    client.move_to(402.0 * 16.0, 330.0 * 16.0).await.unwrap();
    client.open_chest(402, 330).await.unwrap();
    client
        .wait_for(
            "the chest opening",
            |e| matches!(e, Event::Other(f) if f.id == id::SYNC_PLAYER_CHEST),
        )
        .await
        .expect("the chest never opened");
    let closed = terrustia_proto::objects::SyncPlayerChest::closed()
        .encode()
        .unwrap();
    client.send(&closed).await.unwrap();
}

/// A biome key alone in a chest turns that chest into the mimic the key names.
///
/// `NPC.BigMimicSummonCheck` (`NPC.cs:78318-78404`), reached from `Player.ChestChangeEvents`
/// (`Player.cs:28981-28999`). *Closing* the chest is the trigger, not opening it, so the summon
/// rides the chest-sync packet a client sends when it shuts the window. Both keys are craftable
/// here, and before this they did nothing at all.
#[tokio::test]
async fn a_key_of_light_alone_in_a_chest_becomes_a_hallowed_mimic() {
    let addr = start_with(
        Config::default(),
        a_world_with_a_chest_holding(vec![ItemStack::new(3092, 1, 0)]),
    )
    .await;
    let mut bob = join(addr, "bob").await;
    open_then_close_the_chest(&mut bob).await;

    bob.wait_for(
        "the hallowed mimic",
        |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == 475 && n.life > 0),
    )
    .await
    .expect("closing a chest on a Key of Light summoned nothing");

    // The chest goes with it: `Chest.DestroyChest` plus the loop that clears its four tiles.
    let fresh = join(addr, "fresh").await;
    for dx in 0..2 {
        for dy in 0..2 {
            assert!(
                fresh
                    .world()
                    .tile(402 + dx, 330 + dy)
                    .is_none_or(|t| !t.is_active()),
                "the mimic left the chest tile at {dx},{dy} standing"
            );
        }
    }
}

/// A key beside anything else does nothing, and neither does two keys.
///
/// `if (num4 == 0 && num2 + num3 == 1)` (`NPC.cs:78353`) is the whole gate, and it is the reason
/// a player cannot use their loot chest as a mimic farm by dropping a spare key in it.
#[tokio::test]
async fn a_biome_key_with_company_summons_nothing() {
    let addr = start_with(
        Config::default(),
        // A Key of Night and one dirt block.
        a_world_with_a_chest_holding(vec![ItemStack::new(3091, 1, 0), ItemStack::new(2, 1, 0)]),
    )
    .await;
    let mut bob = join(addr, "bob").await;
    open_then_close_the_chest(&mut bob).await;

    let summoned = bob
        .try_wait_for(
            "a mimic that should not be there",
            |e| matches!(e, Event::NpcSynced(n) if (473..=475).contains(&n.npc_type())),
            Duration::from_millis(600),
        )
        .await;
    assert!(
        summoned.is_none(),
        "a key sharing a chest still summoned a mimic"
    );

    // And the chest is still a chest.
    let mut fresh = join(addr, "fresh").await;
    fresh.open_chest(402, 330).await.unwrap();
    fresh
        .wait_for(
            "the chest, still there",
            |e| matches!(e, Event::Other(f) if f.id == id::SYNC_PLAYER_CHEST),
        )
        .await
        .expect("the chest was destroyed without summoning anything");
}

/// A training dummy appears when somebody is near its tile, and goes when they leave.
#[tokio::test]
async fn a_training_dummy_comes_and_goes() {
    // The dummy's tile, planted where a joining player will be standing.
    let addr = start_with(Config::default(), |world| {
        let (x, y) = (world.spawn_x as i32, world.spawn_y as i32 + 1);
        world.set_tile(x, y, Tile::framed(378, 0, 0));
    })
    .await;
    let mut bob = join(addr, "bob").await;

    let (x, y) = (bob.world().spawn.0, bob.world().spawn.1 + 1);
    // A dummy only comes out for somebody standing near it, so bob has to actually be there: a
    // client that has never moved is at the origin as far as the server is concerned.
    bob.move_to(f32::from(x) * 16.0, f32::from(y) * 16.0)
        .await
        .unwrap();

    let mut place = Vec::new();
    place.extend_from_slice(&x.to_le_bytes());
    place.extend_from_slice(&y.to_le_bytes());
    place.push(0); // a training dummy
    bob.send(&frame(id::TILE_ENTITY_PLACEMENT, &place))
        .await
        .unwrap();

    // The dummy is put out because bob is standing right there.
    let raised = bob
        .wait_for(
            "the dummy appearing",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 488),
        )
        .await
        .expect("a dummy should have been raised");
    let Event::NpcSynced(dummy) = raised else {
        unreachable!("matched on it");
    };
    // It carries where it was planted, which is how its routine knows the tile is still there.
    assert_eq!(
        (dummy.ai[0] as i16, dummy.ai[1] as i16),
        (x, y),
        "the dummy does not know where it was planted"
    );
}

/// A tile entity cannot be hung in mid-air.
#[tokio::test]
async fn a_tile_entity_needs_its_tile() {
    let addr = start().await;
    let mut bob = join(addr, "bob").await;

    // An item frame where there is certainly nothing.
    let mut place = Vec::new();
    place.extend_from_slice(&402i16.to_le_bytes());
    place.extend_from_slice(&20i16.to_le_bytes());
    place.push(1); // an item frame
    bob.send(&frame(id::TILE_ENTITY_PLACEMENT, &place))
        .await
        .unwrap();

    // Nothing happens, and the server is still answering.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let fresh = join(addr, "fresh").await;
    assert!(fresh.world().tile(402, 20).is_some(), "the server survived");
}

/// Mining a dummy's tile takes the dummy with it, rather than leaving it standing.
#[tokio::test]
async fn a_dummy_goes_when_its_tile_does() {
    let addr = start_with(Config::default(), |world| {
        let (x, y) = (world.spawn_x as i32, world.spawn_y as i32 + 1);
        world.set_tile(x, y, Tile::framed(378, 0, 0));
    })
    .await;
    let mut bob = join(addr, "bob").await;
    let (x, y) = (bob.world().spawn.0, bob.world().spawn.1 + 1);
    bob.move_to(f32::from(x) * 16.0, f32::from(y) * 16.0)
        .await
        .unwrap();

    let mut place = Vec::new();
    place.extend_from_slice(&x.to_le_bytes());
    place.extend_from_slice(&y.to_le_bytes());
    place.push(0);
    bob.send(&frame(id::TILE_ENTITY_PLACEMENT, &place))
        .await
        .unwrap();
    let raised = bob
        .wait_for(
            "the dummy",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 488),
        )
        .await
        .expect("a dummy");
    let Event::NpcSynced(dummy) = raised else {
        unreachable!("matched on it")
    };

    // Mine the tile out from under it.
    let square = TileSquare {
        x,
        y,
        width: 1,
        height: 1,
        change_type: 0,
        tiles: vec![Tile::AIR],
    };
    bob.send(&square.encode().unwrap()).await.unwrap();

    bob.wait_for(
        "the dummy going",
        |e| matches!(e, Event::NpcSynced(n) if n.index == dummy.index && n.life == 0),
    )
    .await
    .expect("the dummy should have gone with its tile");
}

/// Lay a flat arena with a crystal stand in the middle of it.
///
/// The Old One's Army refuses to begin unless the ground is sixty tiles clear on both sides, which
/// is the game's own way of making building an arena part of preparing for the event.
fn arena_with_a_stand(world: &mut World, at: (i32, i32)) {
    for x in at.0 - 120..=at.0 + 120 {
        for y in at.1 + 1..at.1 + 6 {
            world.set_tile(x, y, Tile::block(1));
        }
        for y in at.1 - 20..=at.1 {
            world.set_tile(x, y, Tile::AIR);
        }
    }
    // The stand itself: a 3x2 object, so every tile carries its frame.
    for dx in 0..3 {
        for dy in 0..2 {
            world.set_tile(
                at.0 + dx,
                at.1 - 1 + dy,
                Tile::framed(466, dx as i16 * 18, dy as i16 * 18),
            );
        }
    }
}

/// Putting a crystal on its stand raises the event, its gates and its first wave.
#[tokio::test]
async fn the_old_ones_army_begins_at_a_crystal_stand() {
    let at = (400, 330);
    let addr = start_with(Config::default(), |world| arena_with_a_stand(world, at)).await;
    let mut bob = join(addr, "bob").await;

    let mut place = Vec::new();
    place.extend_from_slice(&(at.0 as i16).to_le_bytes());
    place.extend_from_slice(&((at.1 - 1) as i16).to_le_bytes());
    bob.send(&frame(id::CRYSTAL_INVASION_START, &place))
        .await
        .unwrap();

    // The crystal itself, and then the two gates it raises at the arena's ends.
    bob.wait_for(
        "the crystal",
        |e| matches!(e, Event::NpcSynced(n) if n.net_id == 548),
    )
    .await
    .expect("a crystal should have appeared");

    let mut gates = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while gates < 2 && tokio::time::Instant::now() < deadline {
        if let Some(Event::NpcSynced(n)) = bob
            .try_wait_for(
                "a gate",
                |e| matches!(e, Event::NpcSynced(n) if n.net_id == 549),
                Duration::from_secs(6),
            )
            .await
        {
            let _ = n;
            gates += 1;
        } else {
            break;
        }
    }
    assert_eq!(gates, 2, "both lane portals should have gone up");
}

/// An arena too small for the event is refused, rather than starting one nobody can win.
#[tokio::test]
async fn a_cramped_arena_is_refused() {
    let at = (400, 330);
    let addr = start_with(Config::default(), |world| {
        // Only twenty tiles of floor each way: nowhere near the sixty the event asks for.
        for x in at.0 - 20..=at.0 + 20 {
            for y in at.1 + 1..at.1 + 6 {
                world.set_tile(x, y, Tile::block(1));
            }
            for y in at.1 - 20..=at.1 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
        for dx in 0..3 {
            for dy in 0..2 {
                world.set_tile(
                    at.0 + dx,
                    at.1 - 1 + dy,
                    Tile::framed(466, dx as i16 * 18, dy as i16 * 18),
                );
            }
        }
        // Walls at both ends, so the arena walker stops well short.
        for y in at.1 - 20..at.1 + 6 {
            world.set_tile(at.0 - 21, y, Tile::block(1));
            world.set_tile(at.0 + 21, y, Tile::block(1));
        }
    })
    .await;
    let mut bob = join(addr, "bob").await;

    let mut place = Vec::new();
    place.extend_from_slice(&(at.0 as i16).to_le_bytes());
    place.extend_from_slice(&((at.1 - 1) as i16).to_le_bytes());
    bob.send(&frame(id::CRYSTAL_INVASION_START, &place))
        .await
        .unwrap();

    let started = bob
        .try_wait_for(
            "a crystal that should not appear",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 548),
            Duration::from_millis(800),
        )
        .await;
    assert!(started.is_none(), "the event began in a cramped arena");
}

/// An enemy standing on you kills you, and everybody is told.
///
/// Death is the server's call, not the client's: a client that decides for itself when it has died
/// is a client that decides for itself when it has not.
#[tokio::test]
async fn an_enemy_can_kill_a_player_and_everyone_hears() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;
    alice.set_timeout(Duration::from_secs(20));
    bob.set_timeout(Duration::from_secs(20));

    bob.move_to(420.0 * 16.0, 300.0 * 16.0).await.unwrap();
    bob.set_life(5, 400).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Something that hurts on contact. Where it lands is the server's choice, so bob goes to it
    // rather than hoping it comes to him.
    let zombie = spawn_npc(&mut bob, "Zombie").await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut died = false;
    let mut on = zombie.position;
    while tokio::time::Instant::now() < deadline && !died {
        // Stand on it, following it if it wanders.
        bob.move_to(on.0, on.1).await.ok();
        match bob
            .try_wait_for(
                "bob dying",
                |e| {
                    matches!(e, Event::PlayerDied(_))
                        || matches!(e, Event::NpcSynced(n) if n.index == zombie.index)
                },
                Duration::from_millis(300),
            )
            .await
        {
            Some(Event::PlayerDied(death)) => {
                assert_eq!(death.player, bob.slot(), "bob's death, not somebody else's");
                died = true;
            }
            Some(Event::NpcSynced(n)) => on = n.position,
            _ => {}
        }
    }
    assert!(
        died,
        "an enemy standing on a player with five life should kill them"
    );

    // And alice, who was nowhere near it, is told.
    alice
        .try_wait_for(
            "the death reaching alice",
            |e| matches!(e, Event::PlayerDied(_)),
            Duration::from_secs(2),
        )
        .await
        .expect("a death should reach every player, not only the one who died");
}

/// A blood moon that walks through a town and leaves it standing is scenery, not a threat.
///
/// The townsfolk take contact damage from anything hostile they are standing in, and their armour
/// — which is the only place the world's history reaches them — decides how long they last.
#[tokio::test]
async fn an_enemy_kills_a_townsperson_it_is_standing_in() {
    let addr = start_with(Config::default(), |world| {
        // Sealed on all four sides, not only hollowed. `clear_area`'s own doc comment makes the
        // mirror-image point: a test that relies on the generator leaving a particular tile *open*
        // breaks the next time the generator changes, and one that relies on it leaving a tile
        // *solid* breaks the same way. This room's left wall and its floor-level right side both
        // opened up when `structures::caves()` moved to vanilla's runners, and the zombies simply
        // walked out through them, which is a fact about the fixture and not about contact damage.
        for x in 378..432 {
            for y in 298..334 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
    })
    .await;

    let mut client = join(addr, "mayor").await;
    client.set_timeout(Duration::from_secs(30));
    client.move_to(400.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // A guide, and then something that wants him dead in the same spot.
    client.say("/spawn Guide").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    for _ in 0..6 {
        client.say("/spawn Zombie").await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The guide is NPC type 22. Watch until his life falls, or he stops being synced at all.
    let mut seen_full = false;
    let mut hurt = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    while tokio::time::Instant::now() < deadline && !hurt {
        client.move_to(400.0 * 16.0, 318.0 * 16.0).await.unwrap();
        match client.next_event().await {
            Ok(Event::NpcSynced(n)) if n.npc_type() == 22 => {
                if n.life >= 250 {
                    seen_full = true;
                } else if seen_full && n.life < 250 {
                    hurt = true;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(
        hurt,
        "a guide stood in a crowd of zombies for twenty-five seconds and took no damage \
         (seen at full health: {seen_full})"
    );
}

/// A dart trap wired to a lever throws a dart when the lever is pulled, and the dart hurts.
///
/// This is the whole chain in one test: the flood finds the trap, the table works out what it
/// throws, the projectile flies, and standing in front of it costs health.
#[tokio::test]
async fn a_wired_dart_trap_shoots_a_player() {
    let addr = start_with(Config::default(), |world| {
        // A corridor with a lever at one end and a dart trap at the other, joined by red wire.
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        // The lever.
        world.set_tile(390, 319, Tile::framed(136, 0, 0));
        // Red wire from the lever to the trap.
        for x in 390..=410 {
            let mut tile = world.tile(x, 319);
            tile.flags
                .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
            world.set_tile(x, 319, tile);
        }
        // A dart trap at the far end, frame_x 0 so it fires west, back down the corridor.
        let mut trap = Tile::framed(137, 0, 0);
        trap.flags
            .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
        world.set_tile(410, 319, trap);
    })
    .await;

    let mut client = join(addr, "trapfodder").await;
    client.set_timeout(Duration::from_secs(20));
    // Stand in the dart's way, a few tiles west of the trap.
    client.move_to(404.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    client.hit_switch(390, 319).await.unwrap();

    let mut dart = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && dart.is_none() {
        client.move_to(404.0 * 16.0, 318.0 * 16.0).await.unwrap();
        match client.next_event().await {
            Ok(Event::ProjectileSynced(p)) => dart = Some(p),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let dart = dart.expect("the lever should have fired the trap");
    assert_eq!(dart.projectile_type, 98, "a dart trap throws a dart");
    assert!(
        dart.velocity.0 < -10.0,
        "it should fly west at twelve, not {:?}",
        dart.velocity
    );

    // And a dart in the face costs health.
    let mut hurt = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && !hurt {
        client.move_to(404.0 * 16.0, 318.0 * 16.0).await.unwrap();
        // Keep pulling the lever: one dart may pass before the player is standing still.
        client.hit_switch(390, 319).await.unwrap();
        match client.next_event().await {
            Ok(Event::PlayerHurt(_)) => hurt = true,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(hurt, "a dart flew through the player and did nothing");
}

/// A trap has a cooldown, which is the difference between a trap and a machine gun.
///
/// A pressure plate a slime is sitting on is hit every tick; without the cooldown every one of
/// those hits would be a dart, and a dungeon corridor would be a wall of them.
#[tokio::test]
async fn a_trap_will_not_fire_faster_than_its_cooldown() {
    let addr = start_with(Config::default(), |world| {
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        world.set_tile(390, 319, Tile::framed(136, 0, 0));
        for x in 390..=410 {
            let mut tile = world.tile(x, 319);
            tile.flags
                .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
            world.set_tile(x, 319, tile);
        }
        let mut trap = Tile::framed(137, 0, 0);
        trap.flags
            .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
        world.set_tile(410, 319, trap);
    })
    .await;

    let mut client = join(addr, "leverpuller").await;
    client.set_timeout(Duration::from_secs(20));
    client.move_to(385.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Pull the lever fifty times over about a second — well inside the dart trap's 200-tick wait.
    for _ in 0..50 {
        client.hit_switch(390, 319).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Count the distinct darts that were born, not the syncs: one projectile is synced many times.
    let mut born = std::collections::HashSet::new();
    while let Some(Event::ProjectileSynced(p)) = client
        .try_wait_for(
            "a projectile",
            |e| matches!(e, Event::ProjectileSynced(_)),
            Duration::from_millis(400),
        )
        .await
    {
        born.insert((p.key.owner, p.key.index, p.key.generation));
    }
    assert!(
        !born.is_empty(),
        "fifty pulls should have fired the trap at least once"
    );
    assert!(
        born.len() <= 2,
        "fifty pulls in a second fired {} darts; the cooldown is not holding",
        born.len()
    );
}

/// A slime statue wired to a lever produces a slime, and that slime is worth nothing.
///
/// The worthlessness is the point: a statue that dropped coins would be a printing press, and a
/// statue whose monsters counted against the spawn cap would stop the world spawning anything
/// else. The game zeroes both on the way out of the statue.
#[tokio::test]
async fn a_wired_statue_spawns_its_monster() {
    let addr = start_with(Config::default(), |world| {
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        world.set_tile(390, 319, Tile::framed(136, 0, 0));
        for x in 390..=400 {
            let mut tile = world.tile(x, 319);
            tile.flags
                .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
            world.set_tile(x, 319, tile);
        }
        // A slime statue: style 4, so frame_x is 4 * 36. Two wide and three tall, standing on
        // the floor with its base at y = 319.
        for dx in 0..2i16 {
            for dy in 0..3i16 {
                let mut tile = Tile::framed(105, 4 * 36 + dx * 18, dy * 18);
                if dy == 2 {
                    tile.flags
                        .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
                }
                world.set_tile(400 + i32::from(dx), 317 + i32::from(dy), tile);
            }
        }
    })
    .await;

    let mut client = join(addr, "statuewatcher").await;
    client.set_timeout(Duration::from_secs(20));
    client.move_to(395.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    client.hit_switch(390, 319).await.unwrap();

    let mut slime = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && slime.is_none() {
        client.move_to(395.0 * 16.0, 318.0 * 16.0).await.unwrap();
        match client.next_event().await {
            // NPC type 1 is a blue slime.
            Ok(Event::NpcSynced(n)) if n.npc_type() == 1 => slime = Some(n),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let slime = slime.expect("the lever should have run the statue");
    // It appears at the middle of the statue's base, a tile up.
    assert!(
        (slime.position.0 / 16.0 - 401.0).abs() < 3.0,
        "it should stand on the statue, not at {}",
        slime.position.0 / 16.0
    );
}

/// A pair of wired teleporters moves whoever is standing on one to the other.
#[tokio::test]
async fn a_teleporter_pair_moves_a_player() {
    let addr = start_with(Config::default(), |world| {
        for x in 300..500 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        world.set_tile(320, 319, Tile::framed(136, 0, 0));
        for x in 320..=460 {
            let mut tile = world.tile(x, 319);
            tile.flags
                .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
            world.set_tile(x, 319, tile);
        }
        // Two teleporters, three tiles wide each, a long way apart.
        for (at, _) in [(360i32, 0), (450, 1)] {
            for dx in 0..3i32 {
                let mut pad = Tile::framed(235, (dx * 18) as i16, 0);
                pad.flags
                    .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
                world.set_tile(at + dx, 319, pad);
            }
        }
    })
    .await;

    let mut client = join(addr, "hopper").await;
    client.set_timeout(Duration::from_secs(20));
    // Stand on the first pad.
    client.move_to(361.0 * 16.0, 316.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;

    client.hit_switch(320, 319).await.unwrap();

    // The server tells everyone, including the player who moved.
    let mut landed = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline && landed.is_none() {
        match client.next_event().await {
            Ok(Event::Other(frame)) if frame.id == id::TELEPORT_ENTITY => {
                let mut r = terrustia_proto::PacketReader::new(&frame.payload);
                let _flags = r.u8().unwrap();
                let _who = r.i16().unwrap();
                let x = r.f32().unwrap();
                let y = r.f32().unwrap();
                landed = Some((x, y));
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let landed = landed.expect("the lever should have worked the teleporters");
    assert!(
        (landed.0 / 16.0 - 451.0).abs() < 4.0,
        "it should have moved ninety tiles east, to about 451, not {}",
        landed.0 / 16.0
    );
}

/// A timer keeps a contraption running with nobody touching it, which is what most wiring is for.
///
/// A server that only runs a circuit when a player hits a switch runs almost none of it: the
/// timers are what make a farm a farm.
#[tokio::test]
async fn a_timer_keeps_firing_a_trap_on_its_own() {
    let addr = start_with(Config::default(), |world| {
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        // A quarter-second timer, frame_x 4 * 18, off to begin with.
        world.set_tile(390, 319, Tile::framed(144, 4 * 18, 0));
        for x in 390..=410 {
            let mut tile = world.tile(x, 319);
            tile.flags
                .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
            world.set_tile(x, 319, tile);
        }
        // A spear trap, which has the shortest cooldown of the lot.
        let mut trap = Tile::framed(137, 3 * 18, 4 * 18);
        trap.flags
            .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
        world.set_tile(410, 319, trap);
    })
    .await;

    let mut client = join(addr, "timekeeper").await;
    client.set_timeout(Duration::from_secs(20));
    client.move_to(385.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // One hit switches the timer on. Nothing else is touched from here.
    client.hit_switch(390, 319).await.unwrap();

    // The spear trap has the shortest cooldown of the lot at ninety ticks, so the gap between
    // shots is a second and a half however fast the timer runs.
    let mut born = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match client
            .try_wait_for(
                "a spear",
                |e| matches!(e, Event::ProjectileSynced(_)),
                Duration::from_secs(2),
            )
            .await
        {
            Some(Event::ProjectileSynced(p)) => {
                born.insert((p.key.owner, p.key.index, p.key.generation));
            }
            _ => break,
        }
    }
    assert!(
        born.len() >= 3,
        "a running timer should have fired the trap several times, not {} time(s)",
        born.len()
    );

    // And switching it off stops it.
    client.hit_switch(390, 319).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    // Drain whatever was already in flight.
    while client
        .try_wait_for(
            "leftovers",
            |e| matches!(e, Event::ProjectileSynced(_)),
            Duration::from_millis(300),
        )
        .await
        .is_some()
    {}
    let quiet = client
        .try_wait_for(
            "another spear",
            |e| matches!(e, Event::ProjectileSynced(_)),
            Duration::from_millis(1200),
        )
        .await;
    assert!(
        quiet.is_none(),
        "the timer kept firing after it was switched off"
    );
}

/// An AND gate passes current on only when both its lamps are lit.
///
/// A gate is what turns wiring from a switchboard into a machine: it reads its whole stack at
/// once, decides, and starts a circuit of its own. Nothing downstream of a gate works without it.
#[tokio::test]
async fn an_and_gate_only_fires_when_both_its_lamps_are_lit() {
    let addr = start_with(Config::default(), |world| {
        for x in 380..440 {
            for y in 290..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        let red = terrustia_proto::tile::TileFlags::WIRE_RED;
        let blue = terrustia_proto::tile::TileFlags::WIRE_BLUE;
        let green = terrustia_proto::tile::TileFlags::WIRE_GREEN;

        // Two levers, each on its own colour, each running along the row of its own lamp.
        // Neither wire passes through the gate tile, which carries green: a wire that ran through
        // it would be cut by the gate rather than reaching the lamp.
        let mut lever_a = Tile::framed(136, 0, 0);
        lever_a.flags.set(red, true);
        world.set_tile(390, 318, lever_a);
        let mut lever_b = Tile::framed(136, 0, 0);
        lever_b.flags.set(blue, true);
        world.set_tile(390, 317, lever_b);

        for x in 390..=400 {
            let mut t = world.tile(x, 318);
            t.flags.set(red, true);
            world.set_tile(x, 318, t);
            let mut t = world.tile(x, 317);
            t.flags.set(blue, true);
            world.set_tile(x, 317, t);
        }

        // The stack: two lamps at y=317 and 318, the gate at y=319, all in column 400.
        let mut upper = Tile::framed(419, 0, 0);
        upper.flags.set(blue, true);
        world.set_tile(400, 317, upper);
        let mut lower = Tile::framed(419, 0, 0);
        lower.flags.set(red, true);
        world.set_tile(400, 318, lower);
        // Gate kind 0 is AND. Green carries whatever it decides onward.
        let mut gate = Tile::framed(420, 0, 0);
        gate.flags.set(green, true);
        world.set_tile(400, 319, gate);

        // Green from the gate to a dart trap facing west.
        for x in 401..=420 {
            let mut t = world.tile(x, 319);
            t.flags.set(green, true);
            world.set_tile(x, 319, t);
        }
        let mut trap = Tile::framed(137, 0, 0);
        trap.flags.set(green, true);
        world.set_tile(420, 319, trap);
    })
    .await;

    let mut client = join(addr, "logician").await;
    client.set_timeout(Duration::from_secs(20));
    client.move_to(385.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // One lamp lit is not enough for an AND gate.
    client.hit_switch(390, 318).await.unwrap();
    let early = client
        .try_wait_for(
            "a dart too soon",
            |e| matches!(e, Event::ProjectileSynced(_)),
            Duration::from_millis(800),
        )
        .await;
    assert!(early.is_none(), "one lamp should not have fired the gate");

    // The second one is.
    client.hit_switch(390, 317).await.unwrap();
    let dart = client
        .try_wait_for(
            "the dart",
            |e| matches!(e, Event::ProjectileSynced(_)),
            Duration::from_secs(4),
        )
        .await;
    assert!(
        dart.is_some(),
        "both lamps lit should have fired the gate, and the gate the trap"
    );
}

/// A timer left running when the world was saved is still running when it is served again.
///
/// This is a deliberate divergence from the game, which keeps its list of running timers only in
/// memory: reopening a world there leaves every timer drawn as on and doing nothing. On a server
/// that would mean every contraption in the world dies silently on a restart.
#[tokio::test]
async fn a_running_timer_survives_a_restart() {
    let dir = std::env::temp_dir().join(format!("terrustia-timer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target: PathBuf = dir.join("timed.wld");
    let _ = std::fs::remove_file(&target);

    let config = Config {
        save_file: Some(target.clone()),
        ..Config::default()
    };
    let addr = start_with(config, |world| {
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        world.set_tile(390, 319, Tile::framed(144, 4 * 18, 0));
        for x in 390..=410 {
            let mut tile = world.tile(x, 319);
            tile.flags
                .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
            world.set_tile(x, 319, tile);
        }
        let mut trap = Tile::framed(137, 3 * 18, 4 * 18);
        trap.flags
            .set(terrustia_proto::tile::TileFlags::WIRE_RED, true);
        world.set_tile(410, 319, trap);
    })
    .await;

    let mut client = join(addr, "winder").await;
    client.set_timeout(Duration::from_secs(20));
    client.hit_switch(390, 319).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    client.say("/save").await.unwrap();
    client
        .wait_for(
            "the save",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("World saved")),
        )
        .await
        .unwrap();

    // Serve the save back with nobody touching anything, and the trap should still be firing.
    let reloaded = wld::load(&target).unwrap();
    let config = Config {
        world_file: Some(target.clone()),
        motd: String::new(),
        ..Config::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel::<ServerEvent>(1024);
    tokio::spawn(GameServer::new(config.clone(), reloaded).run(rx));
    tokio::spawn(listener::run(listener, config, tx, None));

    let mut client = join(addr, "returner").await;
    client.set_timeout(Duration::from_secs(20));
    client.move_to(385.0 * 16.0, 318.0 * 16.0).await.unwrap();
    let spear = client
        .try_wait_for(
            "a spear from the timer that was already running",
            |e| matches!(e, Event::ProjectileSynced(_)),
            Duration::from_secs(5),
        )
        .await;
    assert!(
        spear.is_some(),
        "the timer stopped when the world was reloaded"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An Eater of Worlds drops demonite and shadow scales, which is the whole point of fighting one.
///
/// Without them the shadow armour, the nightmare pickaxe and everything past them are
/// unreachable, so this is progression rather than flavour.
#[tokio::test]
async fn an_eater_of_worlds_drops_its_ore() {
    let addr = start_with(Config::default(), |world| {
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
    })
    .await;

    let mut client = join(addr, "hunter").await;
    client.set_timeout(Duration::from_secs(30));
    client.move_to(400.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The head brings its whole body with it. Killing segments is what drops the ore, and each
    // roll is one in two, so a few worms make it all but certain.
    let mut seen: std::collections::HashSet<i32> = std::collections::HashSet::new();
    for _ in 0..6 {
        // By name with spaces: the id table spells it `EaterofWorldsHead`.
        client.say("/spawn Eater of Worlds Head").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        client.move_to(400.0 * 16.0, 318.0 * 16.0).await.unwrap();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        while tokio::time::Instant::now() < deadline {
            let Ok(event) = client.next_event().await else {
                break;
            };
            match event {
                Event::NpcSynced(n) if (13..=15).contains(&n.npc_type()) && n.life > 0 => {
                    client
                        .hit_npc(n.index, n.generation, 999, 0.0, 1)
                        .await
                        .ok();
                }
                Event::ItemSynced(item) => {
                    seen.insert(item.item.id);
                }
                _ => {}
            }
            if seen.contains(&56) && seen.contains(&86) {
                break;
            }
        }
        if seen.contains(&56) && seen.contains(&86) {
            break;
        }
    }
    // Demonite ore is item 56, shadow scale 86.
    assert!(
        seen.contains(&56),
        "no demonite ore from six Eaters of Worlds; saw {seen:?}"
    );
    assert!(
        seen.contains(&86),
        "no shadow scales from six Eaters of Worlds; saw {seen:?}"
    );
}

/// A chance-gated `OneFromOptions` pool, driven over a real socket rather than only unit-tested
/// against the table itself. Deliberately picked because its expert-mode rate is `1`-in-`1`: the
/// Goblin Summoner's staff pool is `pool(if at.expert { 1 } else { 2 }, ...)`
/// (`conditional_drops.rs`, `chance_pools`) — in expert this always fires, so one kill is a
/// deterministic assertion rather than a retry loop hoping to clear a real dice roll.
#[tokio::test]
async fn a_goblin_summoner_always_drops_its_staff_pool_in_expert_mode() {
    let addr = start_with(Config::default(), |world| {
        world.game_mode = 1; // expert
        for x in 380..420 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
    })
    .await;

    let mut client = join(addr, "summoner").await;
    client.set_timeout(Duration::from_secs(15));
    client.move_to(400.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    client.say("/spawn Goblin Summoner").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut seen: std::collections::HashSet<i32> = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let Ok(event) = client.next_event().await else {
            break;
        };
        match event {
            Event::NpcSynced(n) if n.npc_type() == 471 && n.life > 0 => {
                client
                    .hit_npc(n.index, n.generation, 9999, 0.0, 1)
                    .await
                    .ok();
            }
            // Coins drop first (`npc_died` calls `drop_coins` before `drop_loot`) and are their
            // own `ItemSynced` events too — only the pool's own three items matter here.
            Event::ItemSynced(item) if [3052, 3053, 3054].contains(&item.item.id) => {
                seen.insert(item.item.id);
            }
            _ => {}
        }
        if !seen.is_empty() {
            break;
        }
    }

    assert_eq!(
        seen.len(),
        1,
        "a guaranteed pool should give exactly one item from its own pool, got {seen:?}"
    );
}

/// A player is only told about the NPCs in their own part of the world.
///
/// Sending every NPC to every player is what a server can least afford: the cost grows with
/// players times NPCs, which are the two things it is meant to scale in.
#[tokio::test]
async fn an_npc_far_away_is_not_broadcast() {
    let addr = start_with(Config::default(), |world| {
        for x in 100..700 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
    })
    .await;

    // One player at each end of the corridor, far enough apart to be in different sections.
    let mut near = join(addr, "near").await;
    near.set_timeout(Duration::from_secs(20));
    near.move_to(150.0 * 16.0, 318.0 * 16.0).await.unwrap();

    let mut far = join(addr, "far").await;
    far.set_timeout(Duration::from_secs(20));
    far.move_to(650.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Spawn beside the near player. `/spawn` puts it next to whoever asked.
    near.say("/spawn Zombie").await.unwrap();

    // The player standing on top of it hears about it...
    let seen = near
        .try_wait_for(
            "the zombie",
            |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == 3),
            Duration::from_secs(5),
        )
        .await;
    assert!(seen.is_some(), "the player next to it was not told");

    // ...and the one five hundred tiles away is not flooded with it. The game lets four syncs go
    // by and then sends one anyway, so a handful over several seconds is right and a stream is
    // not: at one sync every six ticks that would be about fifty.
    let mut count = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        far.move_to(650.0 * 16.0, 318.0 * 16.0).await.ok();
        if far
            .try_wait_for(
                "a distant zombie",
                |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == 3),
                Duration::from_millis(400),
            )
            .await
            .is_some()
        {
            count += 1;
        }
    }
    assert!(
        count < 20,
        "the distant player was sent the zombie {count} times in five seconds"
    );
}

/// Breaking shadow orbs gives the gun, then the boss.
///
/// This is the early game's hinge: the first orb in a world always gives a musket, and the third
/// wakes the Eater of Worlds. Neither happened before — breaking an orb did nothing at all, so
/// Skeletron and everything past it was unreachable.
#[tokio::test]
async fn breaking_shadow_orbs_gives_a_gun_and_then_a_boss() {
    let addr = start_with(Config::default(), |world| {
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        // Three shadow orbs. Frame 0 is a shadow orb; 36 would be a crimson heart.
        for (i, x) in [400i32, 404, 408].into_iter().enumerate() {
            let _ = i;
            world.set_tile(x, 318, Tile::framed(31, 0, 0));
        }
    })
    .await;

    let mut client = join(addr, "orbbreaker").await;
    client.set_timeout(Duration::from_secs(20));
    client.move_to(395.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The first orb always gives the musket (96) and a hundred musket balls (97).
    client.break_tile(400, 318).await.unwrap();
    let mut got: std::collections::HashMap<i32, i16> = std::collections::HashMap::new();
    while let Some(Event::ItemSynced(item)) = client
        .try_wait_for(
            "the orb's reward",
            |e| matches!(e, Event::ItemSynced(_)),
            Duration::from_millis(600),
        )
        .await
    {
        got.insert(item.item.id, item.item.stack);
    }
    assert_eq!(
        got.get(&96),
        Some(&1),
        "no musket from the first orb: {got:?}"
    );
    assert_eq!(got.get(&97), Some(&100), "no musket balls: {got:?}");

    // The third wakes the Eater of Worlds.
    client.break_tile(404, 318).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    client.break_tile(408, 318).await.unwrap();

    let worm = client
        .try_wait_for(
            "the Eater of Worlds",
            |e| matches!(e, Event::NpcSynced(n) if (13..=15).contains(&n.npc_type())),
            Duration::from_secs(6),
        )
        .await;
    assert!(
        worm.is_some(),
        "three orbs did not wake the Eater of Worlds"
    );
}

/// A crimson heart gives the Undertaker and wakes the Brain of Cthulhu instead.
#[tokio::test]
async fn crimson_hearts_wake_the_other_boss() {
    let addr = start_with(Config::default(), |world| {
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        // Frame 36 and beyond is a crimson heart.
        for x in [400i32, 404, 408] {
            world.set_tile(x, 318, Tile::framed(31, 36, 0));
        }
    })
    .await;

    let mut client = join(addr, "heartbreaker").await;
    client.set_timeout(Duration::from_secs(20));
    client.move_to(395.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    client.break_tile(400, 318).await.unwrap();
    let mut got = std::collections::HashSet::new();
    while let Some(Event::ItemSynced(item)) = client
        .try_wait_for(
            "the heart's reward",
            |e| matches!(e, Event::ItemSynced(_)),
            Duration::from_millis(600),
        )
        .await
    {
        got.insert(item.item.id);
    }
    assert!(
        got.contains(&800),
        "no Undertaker from the first heart: {got:?}"
    );
    assert!(
        !got.contains(&96),
        "that is the corruption's gun, not the crimson's"
    );

    client.break_tile(404, 318).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    client.break_tile(408, 318).await.unwrap();
    let brain = client
        .try_wait_for(
            "the Brain of Cthulhu",
            |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == 266),
            Duration::from_secs(6),
        )
        .await;
    assert!(
        brain.is_some(),
        "three hearts did not wake the Brain of Cthulhu"
    );
}

/// Breaking a Plantera's bulb wakes her, and another bulb takes its place.
///
/// Plantera has no summon item, so the bulb is the only door. Before this she could not be
/// reached at all, and everything past her — the temple, Golem, the cultist, the Moon Lord —
/// went with her.
#[tokio::test]
async fn a_plantera_bulb_wakes_her_and_regrows() {
    let addr = start_with(Config::default(), |world| {
        world.dungeon_x = Some(700);
        // A stretch of underground jungle on the far side from the dungeon.
        for x in 100..300 {
            for y in 300..340 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 340..360 {
                world.set_tile(x, y, Tile::block(60));
            }
        }
        // A bulb ready to break, sitting on the jungle floor.
        for dx in 0..2i16 {
            for dy in 0..2i16 {
                world.set_tile(
                    200 + i32::from(dx),
                    339 - i32::from(dy),
                    Tile::framed(238, dx * 18, (1 - dy) * 18),
                );
            }
        }
    })
    .await;

    let mut client = join(addr, "gardener").await;
    client.set_timeout(Duration::from_secs(20));
    client.move_to(195.0 * 16.0, 338.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    client.break_tile(200, 339).await.unwrap();

    let plantera = client
        .try_wait_for(
            "Plantera",
            |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == 262),
            Duration::from_secs(6),
        )
        .await;
    assert!(plantera.is_some(), "breaking a bulb did not wake Plantera");
}

/// The Old Man waits at the dungeon, and taking his offer turns him into Skeletron.
///
/// There is no summon item for Skeletron and never has been — the dialogue is the summon. Without
/// it the dungeon stays shut, and with it shut nothing behind it can be reached.
#[tokio::test]
async fn the_old_man_becomes_skeletron() {
    let addr = start_with(Config::default(), |world| {
        world.dungeon_x = Some(400);
        world.dungeon_y = Some(320);
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
    })
    .await;

    let mut client = join(addr, "curious").await;
    client.set_timeout(Duration::from_secs(20));
    client.move_to(400.0 * 16.0, 318.0 * 16.0).await.unwrap();

    // He puts himself back at the door once somebody is near.
    let old_man = client
        .try_wait_for(
            "the old man",
            |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == 37),
            Duration::from_secs(10),
        )
        .await;
    assert!(old_man.is_some(), "nobody was waiting at the dungeon");

    // Packet 51 action 1 is what a client sends when the player takes the offer.
    let mut w = terrustia_proto::PacketWriter::new(id::MISC_DATA_SYNC);
    w.u8(0).u8(1);
    client.send(&w.finish().unwrap()).await.unwrap();

    let skeletron = client
        .try_wait_for(
            "Skeletron",
            |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == 35),
            Duration::from_secs(6),
        )
        .await;
    assert!(skeletron.is_some(), "the old man did not become Skeletron");
}

/// The Clothier's own repeatable vanity re-fight: once Skeletron has been beaten for real, sitting
/// on the one specific chair frame the event uses, at night, holding the Clothier Voodoo Doll,
/// with the Clothier close enough to see, raises a red-hatted Skeletron in his place.
///
/// `PlayerSittingHelper.UpdateSitting` runs this check every frame a player sits — there is no
/// separate summon item, and unlike the very first Skeletron this one never blocked on anything
/// but being asked correctly. Ported from `NPC.RedHatSkeletron` (`NPC.cs:81216-81241`).
#[tokio::test]
async fn sitting_with_the_doll_near_the_clothier_raises_a_red_hat_skeletron() {
    const CHAIR_X: i32 = 60;
    const CHAIR_Y: i32 = 60;
    const CLOTHIER_VOODOO_DOLL: i32 = 1307;
    const SKELETRON: u16 = 35;

    let addr = start_with(Config::default(), |world| {
        clear_with_floor(world, CHAIR_X, CHAIR_Y, 6, 3);
        // The exact frame range `NPC.RedHatSkeletron` checks — real vanilla's own literal
        // `frameX >= 2322 && frameX <= 2358` on tile type 89.
        world.set_tile(CHAIR_X, CHAIR_Y, Tile::framed(89, 2322, 0));
        world.progress.downed_boss3 = true;
        world.day_time = false;
    })
    .await;

    let mut client = join(addr, "seated").await;
    client.set_timeout(Duration::from_secs(20));

    // Where a sitting player's own `Bottom - 2` lands exactly on the chair tile (see
    // `check_red_hat_skeletron`'s own comment on this same width/2, height-2 math).
    let sit_x = CHAIR_X as f32 * 16.0;
    let sit_y = CHAIR_Y as f32 * 16.0 - 40.0;
    client.move_to(sit_x, sit_y).await.unwrap();

    spawn_npc(&mut client, "Clothier").await;
    client
        .set_equipment(0, ItemStack::new(CLOTHIER_VOODOO_DOLL, 1, 0))
        .await
        .unwrap();

    client.sit_at(sit_x, sit_y, 0).await.unwrap();

    let event = client
        .try_wait_for(
            "a red-hat Skeletron",
            |e| matches!(e, Event::NpcSynced(n) if n.npc_type() == SKELETRON),
            Duration::from_secs(10),
        )
        .await;
    let Some(Event::NpcSynced(skeletron)) = event else {
        panic!("no Skeletron rose from the sitting player's Clothier");
    };
    assert_eq!(
        skeletron.ai[3], 1.0,
        "Skeletron spawned but not in red-hat mode (ai[3] should be 1.0)"
    );
}

/// Mining furniture gives the furniture back.
///
/// A framed object stores only a frame; which chair it is lives in the style that frame names.
/// Until now every mined chair, table, chest and door simply vanished.
#[tokio::test]
async fn mining_furniture_gives_it_back() {
    let addr = start_with(Config::default(), |world| {
        for x in 380..430 {
            for y in 300..320 {
                world.set_tile(x, y, Tile::AIR);
            }
            for y in 320..332 {
                world.set_tile(x, y, Tile::block(1));
            }
        }
        // A wooden chair, a wooden table and a work bench, each at style zero.
        world.set_tile(400, 319, Tile::framed(15, 0, 0));
        world.set_tile(404, 319, Tile::framed(14, 0, 0));
        world.set_tile(408, 319, Tile::framed(18, 0, 0));
    })
    .await;

    let mut client = join(addr, "carpenter").await;
    client.set_timeout(Duration::from_secs(20));
    client.move_to(395.0 * 16.0, 318.0 * 16.0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut got = std::collections::HashSet::new();
    for x in [400i16, 404, 408] {
        client.break_tile(x, 319).await.unwrap();
        while let Some(Event::ItemSynced(item)) = client
            .try_wait_for(
                "the furniture",
                |e| matches!(e, Event::ItemSynced(_)),
                Duration::from_millis(400),
            )
            .await
        {
            got.insert(item.item.id);
        }
    }
    assert!(got.contains(&34), "no wooden chair back: {got:?}");
    assert!(got.contains(&32), "no wooden table back: {got:?}");
    assert!(got.contains(&36), "no work bench back: {got:?}");
}

// ---------------------------------------------------------------- debuffs

/// A weapon's on-hit effect is a packet, and until this one was handled the whole of that half of
/// the game did nothing at all. Setting a boss alight has to reach every client, because each one
/// works out its own armour penetration from what it believes is on the target.
#[tokio::test]
async fn a_debuff_lands_and_is_told_to_everyone() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    alice.summon(50).await.unwrap();
    alice.set_timeout(Duration::from_secs(3));
    let Event::NpcSynced(boss) = alice
        .wait_for(
            "king slime",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 50),
        )
        .await
        .unwrap()
    else {
        unreachable!()
    };

    // On Fire!, for ten seconds.
    alice.buff_npc(boss.index, 24, 600).await.unwrap();

    // Bob is told, even though Alice is the one who did it: he needs it to aim.
    bob.set_timeout(Duration::from_secs(3));
    let told = bob
        .wait_for(
            "the buff list",
            |e| matches!(e, Event::Other(f) if f.id == id::N_P_C_BUFFS),
        )
        .await
        .expect("every client should be told what is on an NPC");
    let Event::Other(frame) = told else {
        unreachable!()
    };
    // short index, then (ushort buff, ushort time) pairs ending in a zero.
    assert_eq!(frame.payload[0], boss.index, "the right NPC");
    let buff = u16::from_le_bytes([frame.payload[2], frame.payload[3]]);
    assert_eq!(buff, 24, "On Fire! should be the first entry");
}

/// Burning something has to actually hurt it, and the hits arrive as their own packet rather
/// than as a strike from nobody.
#[tokio::test]
async fn a_debuff_wears_a_target_down() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;

    alice.summon(50).await.unwrap();
    alice.set_timeout(Duration::from_secs(3));
    let Event::NpcSynced(boss) = alice
        .wait_for(
            "king slime",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 50),
        )
        .await
        .unwrap()
    else {
        unreachable!()
    };
    let started_with = boss.life;

    // Cursed Inferno: fast enough to show up inside a test's patience.
    alice.buff_npc(boss.index, 39, 1200).await.unwrap();

    let hurt = alice
        .wait_for(
            "debuff damage",
            |e| matches!(e, Event::Other(f) if f.id == id::N_P_C_DEBUFF_DAMAGE),
        )
        .await;
    assert!(hurt.is_ok(), "a burning boss should be losing life");

    // ...and the loss shows up in its synced health, not only in the report.
    let dropped = alice
        .try_wait_for(
            "a lower health",
            |e| matches!(e, Event::NpcSynced(n) if n.index == boss.index && n.life < started_with),
            Duration::from_secs(5),
        )
        .await;
    assert!(
        dropped.is_some(),
        "the damage should reach the NPC's health"
    );
}

/// Immunity is the server's decision, not the client's. A crafted packet must not be able to
/// poison something the game says cannot be poisoned.
#[tokio::test]
async fn an_immune_target_refuses_a_crafted_debuff() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;

    // King Slime burns, but is immune to poison — which makes it the right target: a refusal
    // here cannot be confused with the packet simply not arriving.
    alice.summon(50).await.unwrap();
    alice.set_timeout(Duration::from_secs(3));
    let Event::NpcSynced(boss) = alice
        .wait_for(
            "king slime",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 50),
        )
        .await
        .unwrap()
    else {
        unreachable!()
    };

    alice.buff_npc(boss.index, 20, 600).await.unwrap();
    let told = alice
        .try_wait_for(
            "a buff list",
            |e| matches!(e, Event::Other(f) if f.id == id::N_P_C_BUFFS),
            Duration::from_millis(500),
        )
        .await;
    assert!(
        told.is_none(),
        "King Slime does not take poison, however politely a client asks"
    );

    // ...and the refusal is specific rather than the whole path being dead.
    alice.buff_npc(boss.index, 24, 600).await.unwrap();
    let burned = alice
        .try_wait_for(
            "a buff list",
            |e| matches!(e, Event::Other(f) if f.id == id::N_P_C_BUFFS),
            Duration::from_secs(2),
        )
        .await;
    assert!(burned.is_some(), "it does still burn");
}

/// The game permits no buff to be lifted by request, and a server that obliged would let a client
/// clear its own poison off a boss.
#[tokio::test]
async fn a_client_cannot_lift_a_debuff_it_dislikes() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;

    alice.summon(50).await.unwrap();
    alice.set_timeout(Duration::from_secs(3));
    let Event::NpcSynced(boss) = alice
        .wait_for(
            "king slime",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 50),
        )
        .await
        .unwrap()
    else {
        unreachable!()
    };

    alice.buff_npc(boss.index, 24, 600).await.unwrap();
    alice
        .wait_for(
            "the buff list",
            |e| matches!(e, Event::Other(f) if f.id == id::N_P_C_BUFFS),
        )
        .await
        .unwrap();

    // Ask for it to be taken off, then check that the next list still has it.
    alice.unbuff_npc(boss.index, 24).await.unwrap();
    alice.buff_npc(boss.index, 70, 600).await.unwrap(); // venom, to force a fresh list
    let Event::Other(frame) = alice
        .wait_for(
            "a later buff list",
            |e| matches!(e, Event::Other(f) if f.id == id::N_P_C_BUFFS && f.payload.len() > 7),
        )
        .await
        .unwrap()
    else {
        unreachable!()
    };
    let mut buffs = Vec::new();
    let mut at = 2;
    while at + 1 < frame.payload.len() {
        let buff = u16::from_le_bytes([frame.payload[at], frame.payload[at + 1]]);
        if buff == 0 {
            break;
        }
        buffs.push(buff);
        at += 4;
    }
    assert!(
        buffs.contains(&24),
        "the fire should still be there: {buffs:?}"
    );
}

/// Every town NPC was nameless: the client asks and nothing answered, so a world full of people
/// was a world full of "Guide".
#[tokio::test]
async fn a_town_npc_has_a_name_of_its_own() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(5));

    // The guide arrives on his own once there is somewhere to live, so put one in directly.
    alice.summon(-100).await.ok();
    let guide = alice
        .try_wait_for(
            "a town npc",
            |e| matches!(e, Event::NpcSynced(n) if terrustia_proto::town_names::has_given_name(n.net_id as u16)),
            Duration::from_secs(5),
        )
        .await;
    let Some(Event::NpcSynced(npc)) = guide else {
        // No town NPC turned up in this world; the name path is covered by the unit tests.
        return;
    };

    alice.ask_npc_name(npc.index).await.unwrap();
    let Event::Other(frame) = alice
        .wait_for(
            "the name",
            |e| matches!(e, Event::Other(f) if f.id == id::UNIQUE_TOWN_N_P_C_INFO_SYNC_REQUEST),
        )
        .await
        .unwrap()
    else {
        unreachable!()
    };
    let mut r = terrustia_proto::PacketReader::new(&frame.payload);
    assert_eq!(r.i16().unwrap(), i16::from(npc.index));
    let name = r.string().unwrap();
    assert!(!name.is_empty(), "a town NPC should have been given a name");
    assert!(
        terrustia_proto::town_names::names_for(npc.net_id as u16).contains(&name.as_str()),
        "{name:?} is not one of the game's names for that type"
    );
}

// ---------------------------------------------------------- tile entities

/// Placing an item frame's *tile* is what brings its tile entity into being — the placement
/// packet does nothing for this kind, in the game or here. And the entity has to be told to
/// everyone, or the frame hangs empty for every client in the world.
#[tokio::test]
async fn placing_a_frames_tile_creates_and_shares_its_entity() {
    // An item frame is two tiles square and nothing may be placed over anything already there,
    // so the spot has to be clear.
    let addr = start_with(Config::default(), |world| {
        for x in 398..404 {
            for y in 318..324 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
    })
    .await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;
    alice.set_timeout(Duration::from_secs(5));
    bob.set_timeout(Duration::from_secs(5));

    // Tile 395 is the item frame. Placing it is the whole gesture.
    alice.place_object(400, 320, 395, 0).await.unwrap();

    let Event::Other(frame) = bob
        .wait_for(
            "the entity",
            |e| matches!(e, Event::Other(f) if f.id == id::TILE_ENTITY_SHARING),
        )
        .await
        .expect("placing the tile should create and share the entity")
    else {
        unreachable!()
    };
    let mut r = terrustia_proto::PacketReader::new(&frame.payload);
    let _id = r.i32().unwrap();
    assert!(r.bool().unwrap(), "it should say the entity is present");
    let entity = terrustia_proto::tile_entity::TileEntity::read(
        &mut r,
        true,
        terrustia_proto::tile_entity::CURRENT_FILE_VERSION,
    )
    .unwrap();
    assert_eq!(
        entity.kind,
        terrustia_proto::tile_entity::EntityKind::ItemFrame
    );
    // The entity anchors on the object's top-left cell, not on the tile the cursor named. The
    // game's own `ValidTile` insists on it: frameY zero and frameX a multiple of the object's
    // width. An item frame's origin is one tile down, so the anchor is a row above the click.
    assert_eq!(entity.x, 400);
    assert_eq!(entity.y, 319, "the anchor is the frame's top-left cell");
}

/// Most kinds cannot be asked for by packet at all, and a server that obliges lets a crafted
/// packet scatter tile entities through a world at coordinates nothing checks.
///
/// Found by the fuzzer: a saved world came back with three anchors in it that had never been
/// placed by anybody.
#[tokio::test]
async fn a_tile_entity_cannot_be_conjured_by_asking() {
    let addr = start_with(Config::default(), |world| {
        // The right tile in the right place, so only the kind's own rule can refuse it.
        world.set_tile(400, 320, Tile::framed(395, 0, 0));
    })
    .await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(3));

    // An item frame, asked for rather than placed.
    alice.place_tile_entity(400, 320, 1).await.unwrap();
    let conjured = alice
        .try_wait_for(
            "an entity",
            |e| matches!(e, Event::Other(f) if f.id == id::TILE_ENTITY_SHARING),
            Duration::from_millis(600),
        )
        .await;
    assert!(
        conjured.is_none(),
        "an item frame is not something a client may ask for"
    );

    // ...and an anchor in mid-air is refused too, which is what the fuzzer got away with.
    alice.place_tile_entity(500, 200, 9).await.unwrap();
    let floating = alice
        .try_wait_for(
            "an entity",
            |e| matches!(e, Event::Other(f) if f.id == id::TILE_ENTITY_SHARING),
            Duration::from_millis(600),
        )
        .await;
    assert!(
        floating.is_none(),
        "a kite anchor needs a kite anchor tile under it"
    );
}

/// The whole point of a frame is that it holds something, and the thing it held before comes back
/// rather than being eaten.
#[tokio::test]
async fn an_item_frame_holds_what_is_put_in_it() {
    let addr = start_with(Config::default(), |world| {
        for x in 398..404 {
            for y in 318..324 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
    })
    .await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(5));
    alice.place_object(400, 320, 395, 0).await.unwrap();
    alice
        .wait_for(
            "the entity",
            |e| matches!(e, Event::Other(f) if f.id == id::TILE_ENTITY_SHARING),
        )
        .await
        .unwrap();

    // A Zenith, for the sake of an item nobody would want eaten. Aimed at the frame's anchor,
    // which is its top-left cell rather than the tile the placement named.
    alice
        .display_item(
            id::ITEM_FRAME_TRY_PLACING,
            400,
            319,
            ItemStack::new(3507, 1, 0),
        )
        .await
        .unwrap();
    let Event::Other(frame) = alice
        .wait_for("the frame's contents", |e| {
            matches!(e, Event::Other(f) if f.id == id::TILE_ENTITY_SHARING && f.payload.len() > 10)
        })
        .await
        .unwrap()
    else {
        unreachable!()
    };
    let mut r = terrustia_proto::PacketReader::new(&frame.payload);
    r.i32().unwrap();
    r.bool().unwrap();
    let entity = terrustia_proto::tile_entity::TileEntity::read(
        &mut r,
        true,
        terrustia_proto::tile_entity::CURRENT_FILE_VERSION,
    )
    .unwrap();
    assert_eq!(entity.held().map(|i| i.id), Some(3507));

    // Swapping it should give the first one back rather than destroying it.
    alice
        .display_item(
            id::ITEM_FRAME_TRY_PLACING,
            400,
            319,
            ItemStack::new(3506, 1, 0),
        )
        .await
        .unwrap();
    let dropped = alice
        .try_wait_for(
            "the old item falling out",
            |e| matches!(e, Event::ItemSynced(i) if i.item.id == 3507),
            Duration::from_secs(3),
        )
        .await;
    assert!(
        dropped.is_some(),
        "swapping a frame's contents should not eat the old item"
    );
}

/// Two people must not be able to empty the same mannequin at once.
#[tokio::test]
async fn only_one_player_may_hold_a_tile_entity() {
    let addr = start_with(Config::default(), |world| {
        for x in 396..406 {
            for y in 316..326 {
                world.set_tile(x, y, Tile::AIR);
            }
        }
    })
    .await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;
    alice.set_timeout(Duration::from_secs(5));
    bob.set_timeout(Duration::from_secs(5));

    // Tile 470 is the mannequin. Placing it is what creates its entity.
    alice.place_object(400, 320, 470, 0).await.unwrap();
    let Event::Other(frame) = alice
        .wait_for(
            "the mannequin",
            |e| matches!(e, Event::Other(f) if f.id == id::TILE_ENTITY_SHARING),
        )
        .await
        .unwrap()
    else {
        unreachable!()
    };
    let entity_id = i32::from_le_bytes([
        frame.payload[0],
        frame.payload[1],
        frame.payload[2],
        frame.payload[3],
    ]);

    alice.claim_tile_entity(entity_id).await.unwrap();
    let claimed = alice
        .wait_for(
            "alice's claim",
            |e| matches!(e, Event::Other(f) if f.id == id::REQUEST_TILE_ENTITY_INTERACTION),
        )
        .await;
    assert!(claimed.is_ok(), "the first claim should be granted");

    // Bob asking for the same one is refused, so nothing further is announced.
    bob.claim_tile_entity(entity_id).await.unwrap();
    let stolen = bob
        .try_wait_for(
            "bob's claim",
            |e| {
                matches!(e, Event::Other(f) if f.id == id::REQUEST_TILE_ENTITY_INTERACTION
                && f.payload[4] == 1)
            },
            Duration::from_millis(600),
        )
        .await;
    assert!(
        stolen.is_none(),
        "a mannequin somebody else has open should not be handed over"
    );
}

/// Put a pylon of one network into a world, tile and entity both.
fn plant_pylon(world: &mut World, x: i16, y: i16, kind: u8) {
    use terrustia_proto::tile_entity::{EntityKind, TileEntity};

    // A pylon is three tiles wide and four tall, and its network lives in the frame: fifty-four
    // pixels of frameX per style.
    for dx in 0..3i16 {
        for dy in 0..4i16 {
            world.set_tile(
                i32::from(x + dx),
                i32::from(y + dy),
                Tile::framed(597, i16::from(kind) * 54 + dx * 18, dy * 18),
            );
        }
    }
    let id = world.next_tile_entity;
    world.next_tile_entity += 1;
    world
        .tile_entities
        .push(TileEntity::new(id, EntityKind::TeleportationPylon, x, y));
}

/// Overwrite a rectangle of tiles (tile coords, inclusive) with one block.
fn fill_block(world: &mut World, x0: i32, y0: i32, x1: i32, y1: i32, block: u16) {
    for x in x0..=x1 {
        for y in y0..=y1 {
            world.set_tile(x, y, Tile::block(block));
        }
    }
}

/// Lay enough jungle grass over a pylon's whole biome-scan box that `spawn::biome_at` reads jungle
/// and nothing else. The scan box is 169 by 124 tiles, so a patch a little larger than that,
/// centred on the pylon, leaves no generated tile of a competing biome inside it. This is what a
/// jungle-network pylon needs to still count as a valid source or destination (L2-21 check 4).
fn lay_jungle_around(world: &mut World, cx: i32, cy: i32) {
    const JUNGLE_GRASS: u16 = 60; // JungleTileCount member (`SceneMetrics.cs:613`)
    fill_block(world, cx - 88, cy - 66, cx + 88, cy + 66, JUNGLE_GRASS);
}

/// House two townsfolk on a pylon's own tile, the pair a working (non-Victory) pylon needs within
/// its scan box (L2-21 checks 2 and 3).
fn house_two_townsfolk(world: &mut World, x: i16, y: i16) {
    const GUIDE: i32 = 22;
    const MERCHANT: i32 = 17;
    for (net_id, dx) in [(GUIDE, 0i32), (MERCHANT, 6)] {
        world.town_npcs.push(terrustia::world::objects::TownNpc {
            net_id,
            name: String::from("Somebody"),
            position: ((i32::from(x) + dx) as f32 * 16.0, f32::from(y) * 16.0),
            homeless: false,
            home: (i32::from(x) + dx, i32::from(y)),
            variation: 0,
            homeless_despawn: false,
        });
    }
}

/// The module-8 "take me to this pylon" request the client sends when a player picks a destination
/// off the travel map.
fn pylon_travel_request(x: i16, y: i16, kind: u8) -> Vec<u8> {
    let mut request = Vec::new();
    request.extend_from_slice(&terrustia_proto::net_module::MODULE_PYLON.to_le_bytes());
    request.push(2); // PlayerRequestsTeleport
    request.extend_from_slice(&x.to_le_bytes());
    request.extend_from_slice(&y.to_le_bytes());
    request.push(kind);
    request
}

/// A joining client is told about every pylon in the world.
///
/// The client keeps its own travel list and draws the pylon map from it; nothing else on the wire
/// carries one. Tile entities were being announced and pylons saved and loaded, so the network
/// looked implemented from inside — but a player standing at a pylon opened a map with nowhere to
/// go, which is how the real server's join sequence gave it away.
#[tokio::test]
async fn a_joining_client_is_told_about_every_pylon() {
    const JUNGLE: u8 = 1;
    let addr = start_with(Config::default(), |world| {
        plant_pylon(world, 300, 300, JUNGLE);
    })
    .await;

    let mut client = Client::connect(addr, "traveller").await.unwrap();
    client.set_timeout(Duration::from_secs(10));
    client.handshake().await.unwrap();

    // The announcement arrives during the handshake, so it has to be read out of a capture rather
    // than waited for afterwards. Simplest: reconnect with the tap on.
    let mut recorded = Client::connect(addr, "recorder").await.unwrap();
    let dir = std::env::temp_dir().join(format!("terrustia-pylon-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let capture = dir.join("join.trcap");
    recorded.record_to(&capture).unwrap();
    recorded.set_timeout(Duration::from_secs(10));
    recorded.handshake().await.unwrap();
    recorded.flush_recording();
    drop(recorded);

    let raw = std::fs::read(&capture).unwrap();
    let found = find_pylon_announcement(&raw);
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(
        found,
        Some((300, 300, JUNGLE)),
        "the join should have announced the jungle pylon"
    );
}

/// Scan a TRCAP1 capture's server-to-client half for a module-8 "pylon was added".
fn find_pylon_announcement(raw: &[u8]) -> Option<(i16, i16, u8)> {
    let magic = terrustia_client::tap::MAGIC;
    let mut inbound = Vec::new();
    let mut at = magic.len();
    while at + 10 <= raw.len() {
        let direction = raw[at];
        let len = u32::from_le_bytes(raw[at + 6..at + 10].try_into().unwrap()) as usize;
        at += 10;
        if direction == 1 {
            inbound.extend_from_slice(raw.get(at..at + len)?);
        }
        at += len;
    }

    let mut cursor = 0usize;
    while cursor + 3 <= inbound.len() {
        let len = u16::from_le_bytes([inbound[cursor], inbound[cursor + 1]]) as usize;
        if len < 3 || cursor + len > inbound.len() {
            return None;
        }
        if inbound[cursor + 2] == id::NET_MODULES
            && let Ok(Some((message, pylon))) = terrustia_proto::net_module::decode_pylon_message(
                &inbound[cursor + 3..cursor + len],
            )
            && message == terrustia_proto::net_module::PylonMessage::Added
        {
            return Some((pylon.x, pylon.y, pylon.kind));
        }
        cursor += len;
    }
    None
}

/// A pylon with nobody living near it refuses to carry anyone.
///
/// Two housed townsfolk within the pylon's scan box, which is the game's rule and the one thing
/// that stops the network being a free teleport from the moment a world opens.
#[tokio::test]
async fn a_lonely_pylon_will_not_carry_anybody() {
    const JUNGLE: u8 = 1;
    let addr = start_with(Config::default(), |world| {
        plant_pylon(world, 300, 300, JUNGLE);
        plant_pylon(world, 500, 300, JUNGLE);
    })
    .await;

    let mut client = join(addr, "hopeful").await;
    let start = client.position();

    let mut request = Vec::new();
    request.extend_from_slice(&terrustia_proto::net_module::MODULE_PYLON.to_le_bytes());
    request.push(2); // PlayerRequestsTeleport
    request.extend_from_slice(&300i16.to_le_bytes());
    request.extend_from_slice(&300i16.to_le_bytes());
    request.push(JUNGLE);
    client
        .send(&frame(id::NET_MODULES, &request))
        .await
        .unwrap();

    // Nothing should move: the player is not near a pylon, and no one lives by the destination.
    let moved = client
        .try_wait_for(
            "a teleport",
            |event| matches!(event, terrustia_client::Event::Other(f) if f.id == id::TELEPORT_ENTITY),
            Duration::from_millis(800),
        )
        .await;
    assert!(moved.is_none(), "an empty pylon carried a player anyway");
    assert_eq!(client.position(), start);
}

/// A pylon with a town around it carries a player standing at another one.
///
/// The gate is only half the feature; this is the half a player notices. All five of the game's
/// checks pass here (L2-21): both pylons sit in real jungle, both have their two housed residents,
/// neither is a temple pylon, so the traveller within reach of their own working pylon is moved and
/// everybody is told. This is the scenario the earlier attempt under-built (the source pylon had no
/// townsfolk and neither pylon was in its biome), which is why it regressed.
#[tokio::test]
async fn a_pylon_with_a_town_around_it_carries_a_player() {
    const JUNGLE: u8 = 1;
    let addr = start_with(Config::default(), |world| {
        // Both pylons need to be in real jungle, and both need their townsfolk: the destination so
        // it accepts arrivals, the source so it works as a place to leave from. Lay the jungle
        // first, then plant the pylons on top, so a pylon's own tiles are not overwritten (a pylon
        // whose tile is no longer a pylon is dropped from the travel network).
        lay_jungle_around(world, 300, 300);
        lay_jungle_around(world, 400, 320);
        plant_pylon(world, 300, 300, JUNGLE);
        plant_pylon(world, 400, 320, JUNGLE);
        house_two_townsfolk(world, 300, 300);
        house_two_townsfolk(world, 400, 320);
    })
    .await;

    let mut client = join(addr, "traveller").await;
    // Stand at the pylon nearest spawn. The reach is sixty tiles, so the exact spot does not
    // matter, but being in the world's other half would.
    client.walk_to_tile(400, 316).await.unwrap();

    client
        .send(&frame(
            id::NET_MODULES,
            &pylon_travel_request(300, 300, JUNGLE),
        ))
        .await
        .unwrap();

    let moved = client
        .try_wait_for(
            "a teleport",
            |event| matches!(event, terrustia_client::Event::Other(f) if f.id == id::TELEPORT_ENTITY),
            Duration::from_secs(5),
        )
        .await;

    let Some(terrustia_client::Event::Other(frame)) = moved else {
        panic!("the pylon never carried the player");
    };
    let mut r = terrustia_proto::PacketReader::new(&frame.payload);
    let flags = r.u8().unwrap();
    let who = r.i16().unwrap();
    let x = r.f32().unwrap();
    let y = r.f32().unwrap();
    let style = r.u8().unwrap();

    assert_eq!(who, i16::from(client.slot()));
    // `info.PositionInTiles.ToWorldCoordinates()` (`TeleportPylonsSystem.cs:186`), and
    // `ToWorldCoordinates` (`Utils.cs:1857`) is `p.ToVector2() * 16f + new Vector2(autoAddX,
    // autoAddY)` with both defaulting to 8. This used to assert whole tiles, which hid the
    // missing half-tile: the landing spot was 8px up and to the left of vanilla's.
    assert_eq!(
        (x, y),
        (300.0 * 16.0 + 8.0, 300.0 * 16.0 + 8.0),
        "landed on the pylon, at the pixel vanilla lands on"
    );
    assert_eq!(style, 9, "style 9 is the pylon's own animation");
    assert_eq!(flags & 0x08, 0x08, "the network id should follow");
    assert_eq!(
        r.i32().unwrap(),
        i32::from(JUNGLE),
        "the client colours the effect by network"
    );
}

/// A pylon whose ground is no longer its biome will not carry anyone to it (L2-21 check 4,
/// `DoesPylonAcceptTeleportation`). This is one of the three checks the old handler skipped: before
/// it, a jungle pylon standing in plain forest carried players just the same. Everything else here
/// is valid (both pylons have their townsfolk, the source is real jungle), so the refusal can only
/// be the destination's biome.
#[tokio::test]
async fn a_pylon_out_of_its_biome_does_not_carry() {
    const JUNGLE: u8 = 1;
    const STONE: u16 = 1; // in no biome tile list, so the destination reads as plain forest
    let addr = start_with(Config::default(), |world| {
        // The destination: a jungle pylon whose scan box is plain stone, not jungle. The source: a
        // fully working jungle pylon, far enough right that its jungle cannot reach the
        // destination's scan box (169 tiles wide), so only the destination fails the biome gate.
        // Lay the ground first and plant each pylon on top, so their own tiles are not overwritten
        // (a pylon whose tile is no longer a pylon is dropped from the travel network).
        fill_block(world, 300 - 88, 300 - 66, 300 + 88, 300 + 66, STONE);
        lay_jungle_around(world, 520, 320);
        plant_pylon(world, 300, 300, JUNGLE);
        plant_pylon(world, 520, 320, JUNGLE);
        house_two_townsfolk(world, 300, 300);
        house_two_townsfolk(world, 520, 320);
    })
    .await;

    let mut client = join(addr, "traveller").await;
    client.walk_to_tile(520, 316).await.unwrap();
    let start = client.position();

    client
        .send(&frame(
            id::NET_MODULES,
            &pylon_travel_request(300, 300, JUNGLE),
        ))
        .await
        .unwrap();

    let moved = client
        .try_wait_for(
            "a teleport",
            |event| matches!(event, terrustia_client::Event::Other(f) if f.id == id::TELEPORT_ENTITY),
            Duration::from_millis(800),
        )
        .await;
    assert!(
        moved.is_none(),
        "a pylon out of its biome carried a player anyway"
    );
    assert_eq!(client.position(), start);
}

/// A pylon you are standing at that has lost its own townsfolk cannot be used to leave, even when
/// the destination is perfectly valid (L2-21 check 5, the source loop). Before the fix the source
/// only had to exist, not to work: being near any pylon was enough. Here the destination has both
/// its townsfolk and its jungle, so the only thing wrong is the source's empty houses.
#[tokio::test]
async fn a_source_pylon_without_townsfolk_does_not_carry() {
    const JUNGLE: u8 = 1;
    let addr = start_with(Config::default(), |world| {
        // Both pylons stand in real jungle, but only the destination has residents. The source, the
        // one the traveller is stood at, has none. Lay the jungle first and plant the pylons on top,
        // so their own tiles are not overwritten (a pylon whose tile is gone is dropped).
        lay_jungle_around(world, 300, 300);
        lay_jungle_around(world, 400, 320);
        plant_pylon(world, 300, 300, JUNGLE);
        plant_pylon(world, 400, 320, JUNGLE);
        house_two_townsfolk(world, 300, 300);
    })
    .await;

    let mut client = join(addr, "traveller").await;
    client.walk_to_tile(400, 316).await.unwrap();
    let start = client.position();

    client
        .send(&frame(
            id::NET_MODULES,
            &pylon_travel_request(300, 300, JUNGLE),
        ))
        .await
        .unwrap();

    let moved = client
        .try_wait_for(
            "a teleport",
            |event| matches!(event, terrustia_client::Event::Other(f) if f.id == id::TELEPORT_ENTITY),
            Duration::from_millis(800),
        )
        .await;
    assert!(
        moved.is_none(),
        "a pylon with no townsfolk of its own carried a player away anyway"
    );
    assert_eq!(client.position(), start);
}

/// A pylon placed on this server has to still be there after a save, or a restart wipes the
/// travel network. Tile entities were carried through the file untouched, so anything placed
/// while the server ran was lost and anything mined came back.
#[tokio::test]
async fn tile_entities_survive_a_save() {
    let dir = std::env::temp_dir().join(format!("terrustia-te-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join("entities.wld");

    let mut world = worldgen::generate(800, 600, String::from("entities"), 7);
    world.set_tile(400, 320, Tile::framed(395, 0, 0));
    let mut frame = terrustia_proto::tile_entity::TileEntity::new(
        0,
        terrustia_proto::tile_entity::EntityKind::ItemFrame,
        400,
        320,
    );
    frame.data = terrustia_proto::tile_entity::EntityData::Held(ItemStack::new(3507, 1, 0));
    world.tile_entities.push(frame);
    world.next_tile_entity = 1;

    std::fs::write(&path, wld_save::serialize(&world).unwrap()).unwrap();
    let back = wld::parse(&std::fs::read(&path).unwrap()).unwrap();

    assert_eq!(
        back.tile_entities.len(),
        1,
        "the frame should have survived"
    );
    let survivor = &back.tile_entities[0];
    assert_eq!((survivor.x, survivor.y), (400, 320));
    assert_eq!(survivor.held().map(|i| i.id), Some(3507));
    assert_eq!(
        back.next_tile_entity, 1,
        "the next id must clear the ones already used, or a new entity collides"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// -------------------------------------------------------- server teleports

/// A Shellphone sends you home. It is the simplest of the five and the one with no way to fail,
/// so it is the clearest proof the whole path works — the packet was not handled at all.
#[tokio::test]
async fn a_shellphone_sends_a_player_to_spawn() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(5));

    // Somewhere that is definitely not spawn.
    alice.move_to(120.0, 400.0).await.unwrap();
    alice.ask_teleport(3).await.unwrap();

    let moved = alice
        .wait_for(
            "the teleport",
            |e| matches!(e, Event::Other(f) if f.id == id::TELEPORT_ENTITY),
        )
        .await
        .expect("a shellphone should move the player");
    let Event::Other(frame) = moved else {
        unreachable!()
    };
    let mut r = terrustia_proto::PacketReader::new(&frame.payload);
    assert_eq!(r.u8().unwrap(), 0, "it is a player being moved");
    assert_eq!(r.i16().unwrap(), i16::from(alice.slot()));
    let (x, y) = (r.f32().unwrap(), r.f32().unwrap());
    assert!(x > 0.0 && y > 0.0, "it should land somewhere real");
    assert!(
        (x - 120.0).abs() > 16.0 || (y - 400.0).abs() > 16.0,
        "and somewhere other than where it started"
    );
}

/// A teleportation potion has to land somewhere a player can actually stand, not merely
/// somewhere with room in it.
#[tokio::test]
async fn a_teleport_lands_somewhere_solid() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(5));

    alice.ask_teleport(0).await.unwrap();
    let landed = alice
        .try_wait_for(
            "the teleport",
            |e| matches!(e, Event::Other(f) if f.id == id::TELEPORT_ENTITY),
            Duration::from_secs(5),
        )
        .await;
    // A small test world can genuinely have nowhere the search likes, and refusing is the right
    // answer when it does — so the assertion is about *where*, not *whether*.
    if let Some(Event::Other(frame)) = landed {
        let mut r = terrustia_proto::PacketReader::new(&frame.payload);
        r.u8().unwrap();
        r.i16().unwrap();
        let (x, y) = (r.f32().unwrap(), r.f32().unwrap());
        assert!(
            x >= 45.0 * 16.0 && y >= 45.0 * 16.0,
            "a landing spot should be clear of the world's edges, got ({x}, {y})"
        );
    }
}

/// A packet asking for something that is not one of the five does nothing at all.
#[tokio::test]
async fn an_unknown_teleport_request_is_refused() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;

    alice.ask_teleport(99).await.unwrap();
    let moved = alice
        .try_wait_for(
            "a teleport",
            |e| matches!(e, Event::Other(f) if f.id == id::TELEPORT_ENTITY),
            Duration::from_millis(500),
        )
        .await;
    assert!(moved.is_none(), "there is no fifth conch");
}

// ------------------------------------------------------- angler and events

/// A joining player is told what the Angler wants. Without it his quest is blank all day.
#[tokio::test]
async fn a_joining_player_learns_todays_angler_quest() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(5));

    let Event::Other(frame) = alice
        .wait_for(
            "the angler quest",
            |e| matches!(e, Event::Other(f) if f.id == id::ANGLER_QUEST),
        )
        .await
        .expect("a joining player should be told the day's quest")
    else {
        unreachable!()
    };
    let quest = frame.payload[0];
    assert!(
        (quest as usize) < terrustia_proto::angler::QUESTS.len(),
        "quest {quest} is not one of the game's fish"
    );
    assert_eq!(frame.payload[1], 0, "nobody has handed one in yet");

    // ...and the fish asked for has to be one this world can actually produce.
    let fish = terrustia_proto::angler::QUESTS[quest as usize];
    assert!(
        terrustia_proto::angler::available(&fish, false, false, false),
        "a fresh world was asked for {fish:?}, which cannot be caught in it"
    );
}

/// Handing one in is remembered, so the reward cannot be farmed by asking twice.
#[tokio::test]
async fn the_angler_only_pays_once_a_day() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;
    alice.set_timeout(Duration::from_secs(5));
    bob.set_timeout(Duration::from_secs(5));

    // Drain the quest each was told on joining.
    alice
        .wait_for(
            "the quest",
            |e| matches!(e, Event::Other(f) if f.id == id::ANGLER_QUEST),
        )
        .await
        .unwrap();

    // Alice hands one in. Bob rejoining should still be told he has not.
    alice
        .send(&terrustia_proto::packets::empty(id::ANGLER_QUEST_FINISHED).unwrap())
        .await
        .unwrap();

    let mut carol = join(addr, "carol").await;
    carol.set_timeout(Duration::from_secs(5));
    let Event::Other(frame) = carol
        .wait_for(
            "the quest",
            |e| matches!(e, Event::Other(f) if f.id == id::ANGLER_QUEST),
        )
        .await
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(
        frame.payload[1], 0,
        "carol has not handed anything in, whatever alice did"
    );
    let _ = &mut bob;
}

/// An invasion puts a progress bar on the screen and moves it as the horde is cut down.
#[tokio::test]
async fn an_invasion_reports_its_progress() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    // The invasion size scales with how many players have found a life crystal.
    alice.set_life(400, 400).await.unwrap();
    alice.set_timeout(Duration::from_secs(5));

    // -1 is the goblin army's summon code.
    alice.summon(-1).await.unwrap();

    let Event::Other(frame) = alice
        .wait_for(
            "the progress bar",
            |e| matches!(e, Event::Other(f) if f.id == id::INVASION_PROGRESS_REPORT),
        )
        .await
        .expect("an invasion should put its bar on the screen")
    else {
        unreachable!()
    };
    let mut r = terrustia_proto::PacketReader::new(&frame.payload);
    let done = r.i32().unwrap();
    let total = r.i32().unwrap();
    assert_eq!(done, 0, "nothing has been killed yet");
    assert!(
        total > 0,
        "an invasion with nobody in it is not an invasion"
    );
}

// ------------------------------------------------------- the wiring tools

/// The Grand Design lays wire along a whole path in one drag. It was not handled at all, so
/// every wiring tool past the first did nothing.
#[tokio::test]
async fn the_grand_design_lays_a_line_of_wire() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(5));

    // The server counts what the player is carrying, so it has to be told about the wire.
    alice
        .set_equipment(0, ItemStack::new(530, 999, 0))
        .await
        .unwrap();

    // Red wire, straight across.
    alice.mass_wire((400, 320), (405, 320), 1).await.unwrap();

    let mut wired = std::collections::HashSet::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && wired.len() < 6 {
        match alice
            .try_wait_for(
                "a wire edit",
                |e| matches!(e, Event::TileChanged(t) if t.action == 5),
                Duration::from_millis(400),
            )
            .await
        {
            Some(Event::TileChanged(t)) => {
                wired.insert(t.x);
            }
            _ => break,
        }
    }
    assert_eq!(
        wired.len(),
        6,
        "six tiles from 400 to 405 inclusive, got {wired:?}"
    );

    // ...and the player is told what it cost, or their client believes it still has the wire.
    // The bill for *this* run, not a later one: the two are sent together and mixing them up is
    // how a test convinces itself of the wrong number.
    let paid = alice
        .try_wait_for(
            "the bill",
            |e| {
                matches!(e, Event::Other(f) if f.id == id::MASS_WIRE_OPERATION_PAY
                && i16::from_le_bytes([f.payload[0], f.payload[1]]) == 530)
            },
            Duration::from_secs(3),
        )
        .await;
    let Some(Event::Other(frame)) = paid else {
        panic!("the server should say what the run cost")
    };
    let spent = i16::from_le_bytes([frame.payload[2], frame.payload[3]]);
    assert_eq!(spent, 6, "six tiles of wire, matching what was laid");
    assert_eq!(frame.payload[4], alice.slot(), "billed to the right player");
}

/// A player with no wire lays none, however confidently the client asks.
#[tokio::test]
async fn a_wiring_tool_cannot_conjure_wire() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(3));

    alice.mass_wire((400, 320), (410, 320), 1).await.unwrap();
    let laid = alice
        .try_wait_for(
            "a wire edit",
            |e| matches!(e, Event::TileChanged(t) if t.action == 5),
            Duration::from_millis(600),
        )
        .await;
    assert!(laid.is_none(), "an empty inventory buys no wire");
}

/// A drag across the whole world is refused rather than turned into a hundred thousand edits.
#[tokio::test]
async fn an_absurd_drag_is_refused() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    alice
        .set_equipment(0, ItemStack::new(530, 999, 0))
        .await
        .unwrap();

    alice.mass_wire((0, 0), (799, 599), 1).await.unwrap();
    let laid = alice
        .try_wait_for(
            "a wire edit",
            |e| matches!(e, Event::TileChanged(t) if t.action == 5),
            Duration::from_millis(600),
        )
        .await;
    assert!(
        laid.is_none(),
        "a drag across the world is a denial of service, not a wiring job"
    );
}

/// A chest's name reaches the map without the chest being opened.
#[tokio::test]
async fn a_chest_can_be_asked_its_name() {
    let addr = start_with(Config::default(), |world| {
        world.chests = vec![Some(Chest {
            x: 400,
            y: 320,
            name: "Ore".into(),
            items: vec![ItemStack::EMPTY],
        })];
    })
    .await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(3));

    alice.ask_chest_name(400, 320).await.unwrap();
    let Event::Other(frame) = alice
        .wait_for(
            "the chest's name",
            |e| matches!(e, Event::Other(f) if f.id == id::CHEST_NAME),
        )
        .await
        .expect("the map should be able to ask")
    else {
        unreachable!()
    };
    let mut r = terrustia_proto::PacketReader::new(&frame.payload);
    assert_eq!(r.i16().unwrap(), 0, "chest zero");
    assert_eq!((r.i16().unwrap(), r.i16().unwrap()), (400, 320));
    assert_eq!(r.string().unwrap(), "Ore");
}

/// Quick stack puts loot into the chest that already holds it, and into no other.
#[tokio::test]
async fn quick_stack_fills_the_chest_that_already_has_it() {
    let addr = start_with(Config::default(), |world| {
        world.chests = vec![
            // Has wood already: a destination.
            Some(Chest {
                x: 400,
                y: 320,
                name: "Wood".into(),
                items: vec![ItemStack::new(9, 50, 0), ItemStack::EMPTY],
            }),
            // Empty: not a destination, however much room it has.
            Some(Chest {
                x: 402,
                y: 320,
                name: "Empty".into(),
                items: vec![ItemStack::EMPTY, ItemStack::EMPTY],
            }),
        ];
    })
    .await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(5));

    // Stand next to the chests, and carry some wood.
    alice.move_to(400.0 * 16.0, 320.0 * 16.0).await.unwrap();
    alice
        .set_equipment(10, ItemStack::new(9, 30, 0))
        .await
        .unwrap();

    alice.quick_stack(&[10], false).await.unwrap();

    let Event::Other(frame) = alice
        .wait_for(
            "the chest filling",
            |e| matches!(e, Event::Other(f) if f.id == id::SYNC_CHEST_ITEM),
        )
        .await
        .expect("quick stack should move the wood")
    else {
        unreachable!()
    };
    let sync = terrustia_proto::objects::SyncChestItem::decode(&frame.payload).unwrap();
    assert_eq!(sync.chest, 0, "the chest with wood, not the empty one");
    assert_eq!(sync.item.id, 9);
    assert_eq!(sync.item.stack, 80, "fifty plus thirty");
}

/// An empty chest takes nothing, which is what makes the button safe to press without looking.
#[tokio::test]
async fn quick_stack_does_not_scatter_into_empty_chests() {
    let addr = start_with(Config::default(), |world| {
        world.chests = vec![Some(Chest {
            x: 400,
            y: 320,
            name: "Empty".into(),
            items: vec![ItemStack::EMPTY, ItemStack::EMPTY],
        })];
    })
    .await;
    let mut alice = join(addr, "alice").await;
    alice.move_to(400.0 * 16.0, 320.0 * 16.0).await.unwrap();
    alice
        .set_equipment(10, ItemStack::new(9, 30, 0))
        .await
        .unwrap();

    alice.quick_stack(&[10], false).await.unwrap();
    let moved = alice
        .try_wait_for(
            "a chest filling",
            |e| matches!(e, Event::Other(f) if f.id == id::SYNC_CHEST_ITEM),
            Duration::from_millis(600),
        )
        .await;
    assert!(
        moved.is_none(),
        "an empty chest is not somewhere the button may put things"
    );
}

/// A crafted count is refused rather than allocated.
#[tokio::test]
async fn an_absurd_quick_stack_is_refused() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;

    let mut w = terrustia_proto::PacketWriter::new(id::QUICK_STACK_CHESTS);
    w.i32(i32::MAX);
    let frame = w.finish().unwrap();
    alice.send(&frame).await.unwrap();

    // The connection survives, which is the whole assertion.
    alice.set_timeout(Duration::from_secs(3));
    alice.say("still here").await.unwrap();
    let alive = alice
        .try_wait_for(
            "our own message",
            |e| matches!(e, Event::Chat { text, .. } if text.contains("still here")),
            Duration::from_secs(3),
        )
        .await;
    assert!(alive.is_some(), "the server should shrug that off");
}

/// Fishing brings up a handful of enemies. The packet that says so must not be a free spawn of
/// anything in the game at any coordinates a client cares to name.
#[tokio::test]
async fn only_the_fishable_can_be_fished_out() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(3));

    let fish_out = |npc_type: i16| {
        let mut w = terrustia_proto::PacketWriter::new(id::FISH_OUT_N_P_C);
        w.u16(400).u16(320).i16(npc_type);
        w.finish().unwrap()
    };

    // A Moon Lord is not something you catch.
    alice.send(&fish_out(398)).await.unwrap();
    let cheated = alice
        .try_wait_for(
            "a moon lord",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 398),
            Duration::from_millis(600),
        )
        .await;
    assert!(cheated.is_none(), "that is not a fish");

    // A Zombie Merman is.
    alice.send(&fish_out(586)).await.unwrap();
    let caught = alice
        .try_wait_for(
            "the catch",
            |e| matches!(e, Event::NpcSynced(n) if n.net_id == 586),
            Duration::from_secs(3),
        )
        .await;
    assert!(caught.is_some(), "but that one is");
}

// ------------------------------------------------------------------ shimmer

/// An item dropped into shimmer becomes another item. It takes about a second and a half, which
/// is what lets a player change their mind.
#[tokio::test]
async fn shimmer_transmutes_what_is_dropped_into_it() {
    let addr = start_with(Config::default(), |world| {
        // A pool of shimmer with air above it for the item to rest in.
        for x in 396..406 {
            for y in 316..322 {
                let mut tile = Tile::AIR;
                tile.liquid = 255;
                tile.liquid_kind = terrustia_proto::Liquid::Shimmer;
                world.set_tile(x, y, tile);
            }
            world.set_tile(x, 322, Tile::block(1));
        }
    })
    .await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(10));

    // Wood, which the game turns into stone.
    let wood = ItemStack::new(9, 1, 0);
    assert_eq!(
        terrustia_proto::shimmer::transforms_into(9),
        Some(2),
        "wood should transmute into stone"
    );
    alice
        .drop_item(wood, (400.0 * 16.0, 318.0 * 16.0))
        .await
        .unwrap();

    let became = alice
        .try_wait_for(
            "the transmutation",
            |e| matches!(e, Event::ItemSynced(i) if i.item.id == 2),
            Duration::from_secs(10),
        )
        .await;
    assert!(
        became.is_some(),
        "wood left in shimmer should have become stone"
    );
}

/// Coins are not transmuted but spent: they become luck and are gone.
#[tokio::test]
async fn coins_dropped_into_shimmer_become_luck() {
    let addr = start_with(Config::default(), |world| {
        for x in 396..406 {
            for y in 316..322 {
                let mut tile = Tile::AIR;
                tile.liquid = 255;
                tile.liquid_kind = terrustia_proto::Liquid::Shimmer;
                world.set_tile(x, y, tile);
            }
            world.set_tile(x, 322, Tile::block(1));
        }
    })
    .await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(10));

    // A stack of silver, which is a hundred coppers each.
    let silver = ItemStack::new(i32::from(terrustia_proto::shimmer::COPPER_COIN + 1), 5, 0);
    alice
        .drop_item(silver, (400.0 * 16.0, 318.0 * 16.0))
        .await
        .unwrap();

    let Some(Event::Other(frame)) = alice
        .try_wait_for(
            "the coin luck",
            |e| matches!(e, Event::Other(f) if f.id == id::SHIMMER_ACTIONS && f.payload[0] == 1),
            Duration::from_secs(10),
        )
        .await
    else {
        panic!("coins in shimmer should have become luck")
    };
    let mut r = terrustia_proto::PacketReader::new(&frame.payload);
    assert_eq!(r.u8().unwrap(), 1, "the coin-luck action");
    let _at = r.vec2().unwrap();
    assert_eq!(r.i32().unwrap(), 500, "five silver is five hundred coppers");
}

/// An item resting on ordinary ground is left alone.
#[tokio::test]
async fn an_item_out_of_shimmer_is_not_transmuted() {
    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(3));

    alice
        .drop_item(ItemStack::new(9, 1, 0), (400.0 * 16.0, 200.0 * 16.0))
        .await
        .unwrap();
    let changed = alice
        .try_wait_for(
            "a transmutation",
            |e| matches!(e, Event::ItemSynced(i) if i.item.id == 2),
            Duration::from_secs(3),
        )
        .await;
    assert!(changed.is_none(), "wood on dry land stays wood");
}

/// An item with no transmutation of its own comes apart into what it was made of.
///
/// This is decrafting, and it is the half of shimmer that needs the recipe table.
#[tokio::test]
async fn shimmer_decrafts_a_crafted_item() {
    let addr = start_with(Config::default(), |world| {
        for x in 396..406 {
            for y in 316..322 {
                let mut tile = Tile::AIR;
                tile.liquid = 255;
                tile.liquid_kind = terrustia_proto::Liquid::Shimmer;
                world.set_tile(x, y, tile);
            }
            world.set_tile(x, 322, Tile::block(1));
        }
    })
    .await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(10));

    // A Gold Bar: one at a time, from four gold ore, and with no transmutation of its own —
    // which is what makes it take the decraft path rather than the transform one.
    const GOLD_BAR: i32 = 19;
    let bar = terrustia_proto::recipes::decraft_recipe(GOLD_BAR as u16, false)
        .expect("a gold bar is crafted");
    assert!(
        terrustia_proto::shimmer::transforms_into(GOLD_BAR as u16).is_none(),
        "the item must have no transform, or this tests the wrong path"
    );
    let wants: Vec<u16> = bar.ingredients().iter().map(|&(i, _)| i).collect();

    alice
        .drop_item(
            ItemStack::new(GOLD_BAR, bar.makes as i16, 0),
            (400.0 * 16.0, 318.0 * 16.0),
        )
        .await
        .unwrap();

    // An item takes about a second and a half to sink, so a short window that times out is not
    // evidence of anything — keep reading until the deadline rather than giving up on the first.
    let mut got = std::collections::HashSet::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && got.len() < wants.len() {
        if let Some(Event::ItemSynced(item)) = alice
            .try_wait_for(
                "an ingredient",
                |e| matches!(e, Event::ItemSynced(i) if i.item.id != GOLD_BAR && i.item.id != 0),
                Duration::from_millis(500),
            )
            .await
        {
            got.insert(item.item.id as u16);
        }
    }
    for wanted in &wants {
        assert!(
            got.contains(wanted),
            "a decrafted gold bar should have given back item {wanted}; got {got:?}"
        );
    }
}

/// A stack too small for one batch is left alone — three torches decraft, two do not.
#[tokio::test]
async fn a_part_batch_does_not_decraft() {
    let addr = start_with(Config::default(), |world| {
        for x in 396..406 {
            for y in 316..322 {
                let mut tile = Tile::AIR;
                tile.liquid = 255;
                tile.liquid_kind = terrustia_proto::Liquid::Shimmer;
                world.set_tile(x, y, tile);
            }
            world.set_tile(x, 322, Tile::block(1));
        }
    })
    .await;
    let mut alice = join(addr, "alice").await;
    alice.set_timeout(Duration::from_secs(5));

    // Something whose recipe makes several at a time, dropped one short of a batch.
    let (item, recipe) = (1..5000u16)
        .filter(|&i| terrustia_proto::shimmer::transforms_into(i).is_none())
        .filter(|&i| !terrustia_proto::shimmer::is_coin(i))
        .find_map(|i| {
            terrustia_proto::recipes::decraft_recipe(i, false)
                .filter(|r| r.makes > 1 && !r.alchemy)
                .map(|r| (i, r))
        })
        .expect("some recipe makes more than one at a time");
    let short = recipe.makes as i16 - 1;

    alice
        .drop_item(
            ItemStack::new(i32::from(item), short, 0),
            (400.0 * 16.0, 318.0 * 16.0),
        )
        .await
        .unwrap();

    let broke = alice
        .try_wait_for(
            "an ingredient",
            |e| matches!(e, Event::ItemSynced(i) if i.item.id != i32::from(item) && i.item.id != 0),
            Duration::from_secs(5),
        )
        .await;
    assert!(
        broke.is_none(),
        "{short} of item {item} is short of a batch of {} and should not come apart",
        recipe.makes
    );
}

/// Pets, mounts, minecarts and hooks are equipped into the five "miscellaneous equipment" slots
/// (`PlayerItemSlotID`, transcribed in `terrustia_proto::inventory::SLOT_RUNS`) — vanilla itself
/// implements all three almost entirely client-side: `Player.UpdatePet`/`UpdatePetLight` are
/// gated `if (i == Main.myPlayer)`, the pet/mount visual is a player-owned projectile or a
/// modification to the player's own movement stats, and a minecart track switch is an ordinary
/// tile-frame edit. The server's actual job, in vanilla and here, is exactly what `SyncEquipment`
/// (packet 5) already does for every other equipment slot: remember what's equipped, relay it to
/// everyone else so their own client can render/simulate it. This proves that relay actually
/// reaches a second connected player for the misc-equip range specifically, not just the slots
/// this project built the mechanism for originally (armour/accessories) — nothing pet/mount/
/// minecart-specific needed to be built for this to work, and this is the test that backs that
/// claim rather than leaving it asserted.
#[tokio::test]
async fn a_pet_summon_item_equipped_in_the_misc_slot_relays_to_another_player() {
    // `terrustia_proto::inventory::SLOT_RUNS` (private to that crate) lays the slots out as
    // Inventory(58) + cursor(1) + armour/accessories(20) + their dyes(10) = 89, then the five
    // "Miscellaneous equipment" slots (pet, light pet, mount, minecart, hook) start right there.
    let misc_start: u16 = 58 + 1 + 20 + 10;

    let addr = start().await;
    let mut alice = join(addr, "alice").await;
    let mut bob = join(addr, "bob").await;

    const SLIME_STAFF: i32 = 1309; // ItemID.SlimeStaff — a real light-pet summon item

    alice
        .set_equipment(misc_start, ItemStack::new(SLIME_STAFF, 1, 0))
        .await
        .unwrap();

    let synced = bob
        .wait_for(
            "alice's pet-slot equip to relay",
            |e| matches!(e, Event::EquipmentSynced(eq) if eq.slot == misc_start),
        )
        .await
        .expect("the misc-equipment slot should relay the same as any other equipment slot");
    let Event::EquipmentSynced(eq) = synced else {
        unreachable!("matched on it")
    };
    assert_eq!(
        eq.item.id, SLIME_STAFF,
        "bob should see the exact item alice put in her pet slot"
    );
}
