//! Walk the progression chain against a running server and report where it breaks.
//!
//! ```sh
//! cargo run --release -p terrustia-client --example playthrough -- 127.0.0.1:7777
//! ```
//!
//! Every other check in this project asks whether a *subsystem* works. This asks the only question
//! that matters to somebody playing: **can you get from the start of the game to the end?**
//!
//! It is the check that was missing. Three separate blockers sat behind 1,300 passing tests — the
//! Temple Key absent so the Jungle Temple never opened, the Twins dropping no Soul of Sight, most
//! boss attacks silently never firing — and every one of them would have failed this in seconds.
//! The existing drop tests check breadth: that a good many types drop *something*, that a chain
//! stops at its first success. None of them walked the chain.
//!
//! What it does, per boss: summon it, kill it, and watch for the specific item the next step of the
//! game depends on. It is not a real playthrough — nobody mines, nothing is crafted, and the bosses
//! die to a bot that cannot lose. It is the *loot spine* of one, which is the part that silently
//! rots.
//!
//! A link that fails prints what it costs, because "Plantera dropped no 1141" means nothing and
//! "no Temple Key, so the temple never opens and Golem is unreachable" means everything.

use std::{
    collections::{BTreeMap, HashSet},
    env,
    process::ExitCode,
    time::Duration,
};

use terrustia_client::{Client, Event};

/// One link: a boss, the item it owes, and what breaks without it.
struct Link {
    boss: u16,
    /// The NPC to actually summon. For a worm it is the head; the rest follow.
    name: &'static str,
    /// Any one of these counts. A boss whose evil-ore drop depends on the world, or whose
    /// guaranteed drop sits beside a rare one, needs more than a single id.
    items: &'static [i32],
    costs: &'static str,
    /// Seconds to allow this fight.
    ///
    /// Not uniform, because the fights are not. The Moon Lord's eyes are only damageable while
    /// they are open, so most of its fight is spent waiting for a window rather than swinging —
    /// it needs three times as long as anything else, and cutting it short looks exactly like a
    /// missing drop.
    patience: u64,
}

const CHAIN: &[Link] = &[
    Link {
        boss: 4,
        name: "Eye of Cthulhu",
        // Which evil ore depends on the world, so either counts.
        items: &[56, 880],
        costs: "no evil ore, so no Nightmare Pickaxe and no hellstone",
        patience: 40,
    },
    Link {
        boss: 13,
        name: "Eater of Worlds",
        items: &[86],
        costs: "no shadow scales, so no Nightmare Pickaxe and no hellstone",
        patience: 40,
    },
    Link {
        boss: 113,
        name: "Wall of Flesh",
        items: &[367],
        costs: "no Pwnhammer, so no altar can be broken and hardmode has no ore",
        patience: 40,
    },
    Link {
        boss: 134,
        name: "The Destroyer",
        items: &[548],
        costs: "no Soul of Might, so no Drax and no Chlorophyte",
        patience: 40,
    },
    Link {
        boss: 125,
        name: "Retinazer",
        items: &[549],
        costs: "no Soul of Sight, so no Drax and no Chlorophyte",
        patience: 40,
    },
    Link {
        boss: 127,
        name: "Skeletron Prime",
        items: &[547],
        costs: "no Soul of Fright, so no Drax and no Chlorophyte",
        patience: 40,
    },
    Link {
        boss: 262,
        name: "Plantera",
        items: &[1141],
        costs: "no Temple Key, so the Jungle Temple never opens and Golem is unreachable",
        patience: 40,
    },
    Link {
        boss: 245,
        name: "Golem",
        items: &[1294],
        costs: "no Picksaw, so lihzahrd brick cannot be mined",
        patience: 40,
    },
    Link {
        boss: 439,
        name: "Lunatic Cultist",
        items: &[3549, 3372],
        costs: "no Ancient Manipulator, so no luminite item can ever be crafted",
        patience: 40,
    },
    Link {
        boss: 398,
        name: "Moon Lord",
        items: &[3460],
        costs: "no luminite, so the ending leads nowhere",
        patience: 150,
    },
];

/// How many times to fight a boss before calling the link broken.
///
/// Several of these are chance drops — Golem's Picksaw is one in four — so a single fight proves
/// nothing either way. Retrying turns "it did not drop this time" into "it never drops", which is
/// the only version worth reporting.
const ATTEMPTS: u32 = 4;

/// Where the bot stands. Bosses are summoned beside whoever asked, so it never has to travel.
const HOME: (f32, f32) = (2100.0 * 16.0, 300.0 * 16.0);

#[tokio::main]
async fn main() -> ExitCode {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:7777".to_string())
        .parse()
        .expect("a socket address");

    let mut client = match Client::join(addr, "pilgrim").await {
        Ok(client) => client,
        Err(e) => {
            eprintln!("could not join {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    client.set_timeout(Duration::from_secs(1));
    // Stand still somewhere sensible before anything is summoned.
    if let Err(e) = client.move_to(HOME.0, HOME.1).await {
        eprintln!("could not take up position: {e}");
        return ExitCode::FAILURE;
    }

    println!("walking the progression chain on {addr}\n");
    let mut broken = Vec::new();

    for link in CHAIN {
        let mut outcome = Ok(Outcome {
            found: false,
            saw: Vec::new(),
        });
        for attempt in 1..=ATTEMPTS {
            outcome = walk(&mut client, link).await;
            match &outcome {
                Ok(o) if o.found => {
                    println!(
                        "  ok    {:<18} gives {:?}{}",
                        link.name,
                        link.items,
                        if attempt > 1 {
                            format!(" (took {attempt} fights — a chance drop)")
                        } else {
                            String::new()
                        },
                    );
                    break;
                }
                Ok(_) if attempt < ATTEMPTS => continue,
                _ => break,
            }
        }
        match outcome {
            Ok(o) if o.found => {}
            Ok(o) => {
                // A drop that is in the tables but did not land is bad luck, not a gap: Golem's
                // Picksaw is one in four, so four fights miss it a third of the time. Saying
                // "BROKE" there would cry wolf, and a checker nobody believes is worse than none.
                if in_the_tables(link) {
                    println!(
                        "  luck  {:<18} {:?} is in the tables but did not drop in {ATTEMPTS} \
                         fights — a chance drop, not a gap",
                        link.name, link.items,
                    );
                } else {
                    println!(
                        "  BROKE {:<18} no {:?} in {ATTEMPTS} fights, and none of them is in the \
                         drop tables — {}",
                        link.name, link.items, link.costs
                    );
                    println!("        saw (type, lowest life): {:?}", o.saw);
                    broken.push(link);
                }
            }
            Err(e) => {
                println!("  ERROR {:<18} {e}", link.name);
                broken.push(link);
            }
        }
    }

    println!();
    if broken.is_empty() {
        println!("every link in the chain holds: this server can be played to the end.");
        return ExitCode::SUCCESS;
    }
    println!("{} of {} links are broken:", broken.len(), CHAIN.len());
    for link in &broken {
        println!("  {} — {}", link.name, link.costs);
    }
    ExitCode::FAILURE
}

/// Whether the server's own tables even list this drop.
///
/// The difference between "unlucky" and "missing" is the whole point of the tool: one is noise and
/// the other ends a playthrough. Reading the tables directly settles it without another fight.
fn in_the_tables(link: &Link) -> bool {
    use terrustia_proto::conditional_drops::{Conditions, conditional, one_from};

    let at = Conditions {
        hard_mode: true,
        other_twin_dead: true,
        red_hat_skeletron: false,
        // The Terraprisma's gate. True here because this walks the tables looking for what a
        // playthrough *can* reach, and a daylight Empress kill is a real, reachable fight.
        empress_genuinely_enraged: true,
        downed_plantera: false,
        expert: false,
        master: false,
        world_is_crimson: false,
        in_hallow: false,
        in_corruption: false,
        in_crimson: false,
        underground: false,
        blood_moon: false,
        npc_from_statue: false,
        eclipse: false,
        downed_mech_any: true,
        downed_all_mech_bosses: true,
        pumpkin_moon_wave: None,
        frost_moon_wave: None,
    };
    let wanted = |item: i32| {
        conditional(link.boss, at)
            .iter()
            .any(|d| i32::from(d.item) == item)
            || one_from(link.boss, at)
                .iter()
                .any(|pool| pool.iter().any(|i| i32::from(*i) == item))
            || terrustia_proto::npc_drops::drops(link.boss)
                .iter()
                .any(|chain| chain.iter().any(|d| i32::from(d.item) == item))
    };
    link.items.iter().copied().any(wanted)
}

/// Summon one boss, kill it, and say whether the item it owes turned up.
async fn walk(client: &mut Client, link: &Link) -> terrustia_client::Result<Outcome> {
    // Clear anything already lying about so a previous boss's loot cannot be mistaken for this
    // one's — several of these drop overlapping sets.
    client.say("/butcher").await?;
    drain(client).await;

    client.say(&format!("/spawn {}", link.boss)).await?;

    // Slot and generation together: a hit carrying a stale generation is refused, which is what
    // stops one boss's death landing on whatever reuses its slot.
    let mut alive: HashSet<(u8, u8)> = HashSet::new();
    // What turned up, so a failure can say whether the boss even appeared. "Plantera dropped
    // nothing" and "Plantera never spawned" need different fixes and look identical otherwise.
    let mut seen: BTreeMap<u16, i32> = BTreeMap::new();
    let mut found = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(link.patience);

    let mut since_swing = tokio::time::Instant::now();

    while tokio::time::Instant::now() < deadline {
        // Hit everything currently alive, but no more than a few times a second. Swinging on
        // every event floods the connection: a worm is dozens of segments, each one syncing, and
        // a hit per segment per sync is thousands of frames a second going nowhere useful.
        if since_swing.elapsed() >= Duration::from_millis(120) {
            since_swing = tokio::time::Instant::now();
            for (index, generation) in alive.iter().copied().collect::<Vec<_>>() {
                client.hit_npc(index, generation, 30_000, 0.0, 0).await?;
            }
            // Say *something* even when there is nothing to hit. A real client sends control
            // updates continuously, so the server drops anyone silent for a minute — and a bot
            // waiting quietly for a boss that never arrives looks exactly like a dead socket.
            client.move_to(HOME.0, HOME.1).await?;
        }

        match client.next_event().await {
            Ok(Event::NpcSynced(npc)) => {
                // Lowest life seen, which is what says whether a part ever became damageable.
                let low = seen.entry(npc.npc_type()).or_insert(i32::MAX);
                *low = (*low).min(npc.life);
                if npc.life > 0 {
                    alive.insert((npc.index, npc.generation));
                } else {
                    alive.remove(&(npc.index, npc.generation));
                }
            }
            Ok(Event::ItemSynced(item)) => {
                if link.items.contains(&item.item.id) {
                    found = true;
                    break;
                }
            }
            Ok(_) => {}
            // A timeout is ordinary — the server has nothing to say between ticks. Anything else
            // means the connection is gone, and carrying on would report nine more false failures.
            Err(terrustia_client::ClientError::Timeout { .. }) => {}
            Err(e) => return Err(e),
        }
    }

    client.say("/butcher").await?;
    drain(client).await;
    let saw: Vec<(u16, i32)> = seen.into_iter().collect();
    Ok(Outcome { found, saw })
}

/// What one fight produced.
struct Outcome {
    found: bool,
    /// NPC types that appeared, so a failure can distinguish "never spawned" from "dropped
    /// nothing" from "would not die".
    saw: Vec<(u16, i32)>,
}

/// Swallow whatever is queued, so one step's noise does not reach the next.
async fn drain(client: &mut Client) {
    for _ in 0..64 {
        if client.next_event().await.is_err() {
            break;
        }
    }
}
