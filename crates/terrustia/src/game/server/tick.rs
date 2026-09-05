//! The game loop: the actor's `select!`, the sixty-hertz tick, and what a tick cost.
//!
//! [`GameServer::run`] owns the loop itself; [`GameServer::tick`] is one frame of the world, which
//! does nothing on its own beyond calling each system in the order vanilla does and timing it. The
//! measurement types live here too, next to the only code that reads them.

use std::time::{Duration, Instant};

use tokio::{
    sync::mpsc,
    time::{MissedTickBehavior, interval},
};
use tracing::{debug, error, info, warn};

use crate::game::clock;

use super::{GameServer, SYNC_FULL, SYNC_STREAM, ServerEvent, Stopped};

/// Vanilla runs at 60 ticks per second and the clock packets assume it.
pub(super) const TICK: Duration = Duration::from_nanos(16_666_667);
/// Ticks in a second, for turning the tick counter into a human uptime on the status footer.
const TICKS_PER_SECOND: u64 = 60;
/// How often the live status footer is refreshed — about once a second, off the tick counter.
const STATUS_EVERY: u64 = 60;

/// How often the worst tick in the window is reported, when it is worth reporting.
const TICK_REPORT_EVERY: u64 = 600;

/// How often the outbound queues are sampled for their depth, ten times a second.
///
/// The sample walks every connection, so it is not free, and it does not need to be finer: a
/// backlog deep enough to account for hundreds of megabytes lasts for seconds, not for a sixth of
/// one. Reported per window as `queue_peak`.
const QUEUE_SAMPLE_EVERY: u64 = 6;

/// How often the tick looks for connections that took a slot and never finished joining.
///
/// Once a second. The sweep is a walk over the slot table - at most 255 entries, most of them
/// `None` on any real server - against a deadline measured in tens of seconds, so doing it sixty
/// times as often would be sixty times the work for no extra precision anybody could observe.
const REAP_EVERY: u64 = 60;

/// The parts of a tick, in the order they run.
///
/// What used to be one `World` phase was thirteen separate systems sharing a lap, so a warning
/// saying `phase=world` narrowed the cause down to "somewhere in most of the tick". A two-hour
/// idle run reported that phase eating half the budget with two NPCs and nobody connected; the
/// cause turned out to be the autosave's world copy, which is now its own entry and would have
/// been obvious from the first warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    /// Copying the world for the background save. Runs on the tick, once every autosave.
    Snapshot,
    Liquids,
    Growth,
    Spread,
    Weather,
    /// The clock, tile entities, wiring timers, lunar events and the biome census.
    World,
    Sections,
    Items,
    Npcs,
    Projectiles,
    Damage,
    Spawning,
    Housing,
    Sync,
}

impl Phase {
    pub(super) const NAMES: [&'static str; 14] = [
        "snapshot",
        "liquids",
        "growth",
        "spread",
        "weather",
        "world",
        "sections",
        "items",
        "npcs",
        "projectiles",
        "damage",
        "spawning",
        "housing",
        "sync",
    ];
}

/// Times one phase of a tick, on the same clock the tick's own total uses.
///
/// A named type rather than two lines inline, because those two lines were wrong for months and
/// nothing could see it: phases were timed with `Instant` while the tick total came from
/// `clock::Cpu`, so the warning line compared wall microseconds against CPU microseconds and could
/// report a phase costing more than the whole tick containing it. Every phase figure ever logged
/// was inflated by however long that phase spent descheduled.
///
/// Wrapping it makes the mistake unavailable — there is nowhere here to put an `Instant` — and it
/// makes the property that matters testable on its own, which is the part that counts. Asserting
/// "no phase exceeds its tick" does *not* catch this: on an idle machine the two clocks agree, so
/// that assertion passes against the broken code, which is exactly how it survived so long.
struct PhaseClock(clock::Cpu);

impl PhaseClock {
    fn start() -> Self {
        Self(clock::Cpu::now())
    }

    /// Processor time since the last lap.
    fn lap(&mut self) -> Duration {
        let now = clock::Cpu::now();
        let elapsed = now.since(self.0);
        self.0 = now;
        elapsed
    }
}

/// Where one tick's time went.
///
/// `cpu` is what the tick cost; `wall` is how long it took to happen. They differ by however long
/// the OS gave this core to something else, which on a machine that is also running the game can
/// be tens of milliseconds. Keeping them apart is what stops a busy laptop from being reported as
/// a slow server.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct TickCost {
    pub(super) cpu: Duration,
    pub(super) wall: Duration,
    pub(super) phases: [Duration; Phase::NAMES.len()],
}

impl TickCost {
    /// The phase that took the longest, which is the one worth naming in a warning.
    fn worst_phase(&self) -> (&'static str, Duration) {
        self.phases
            .iter()
            .enumerate()
            .max_by_key(|(_, d)| **d)
            .map_or(("none", Duration::ZERO), |(i, d)| (Phase::NAMES[i], *d))
    }
}

/// The footer's save-failure clause: empty while saves are working, a red count when they are not.
///
/// Split out so it can be checked without standing up a server and reading a global. The rule it
/// encodes is that a healthy server's footer is byte for byte the one it always was, and that a
/// failing one says so somewhere that does not scroll.
fn save_failure_note(palette: crate::term::Palette, failures: u32) -> String {
    if failures == 0 {
        return String::new();
    }
    palette.paint(
        crate::term::sgr::BRIGHT_RED,
        &format!("   saves failing ({failures})"),
    )
}

impl GameServer {
    /// Refresh the live status footer: who is online, how long the server has been up, the last
    /// tick's cost, and whether saves are failing. Called about once a second from
    /// [`Self::note_tick_cost`]. Cheap, and a no-op on screen when there is no interactive prompt
    /// to sit above (a piped or service console).
    fn update_status(&self) {
        let p = self.palette;
        let online = self
            .players
            .iter()
            .flatten()
            .filter(|player| player.is_playing())
            .count();
        // Uptime off the tick counter rather than a wall clock: this is a health read, not a
        // timestamp, and one that never has to reach for `Instant::now` on the hot path.
        let secs = self.ticks / TICKS_PER_SECOND;
        let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
        // The span since this last drew, not one arbitrary tick out of the sixty. See
        // `GameServer::status_span` for why a single sample reads as instability that is not there.
        let (low, high) = self.status_span;
        let tick = if low > high {
            // No tick has been costed yet, only reachable on the very first draw.
            "tick -".to_string()
        } else if low == high {
            format!("tick {}µs", high.as_micros())
        } else {
            format!("tick {}-{}µs", low.as_micros(), high.as_micros())
        };
        use crate::term::sgr;
        let dot_colour = if online > 0 {
            sgr::BRIGHT_GREEN
        } else {
            sgr::DIM
        };
        // Coloured by the same worst-sample budget check `note_tick_cost` warns on below, so the
        // footer and the warning agree about what "a lot of the budget" means: green under half,
        // yellow up to the full budget, red once a tick has actually gone over it.
        let tick_colour = if high * 2 <= TICK {
            sgr::DIM
        } else if high <= TICK {
            sgr::BRIGHT_YELLOW
        } else {
            sgr::BRIGHT_RED
        };
        // A failing save is the one condition an operator has to notice, and it was the one the
        // terminal made easiest to miss: a single `error!` that scrolls away behind ordinary log
        // traffic, while the panel pins it twice (a header badge and an overview card). The
        // footer is the only part of the terminal that does not scroll, so it belongs here too.
        // Only ever shown when non-zero, so a healthy server's footer is exactly what it was.
        let saves = save_failure_note(p, self.save_failures);
        let status = format!(
            "  {} {} online   {}   {}{saves}",
            p.paint(dot_colour, "●"),
            p.paint(sgr::BOLD, &online.to_string()),
            p.paint(sgr::DIM, &format!("up {h:02}:{m:02}:{s:02}")),
            p.paint(tick_colour, &tick),
        );
        crate::term::set_status(&status);
    }

    pub async fn run(mut self, mut events: mpsc::Receiver<ServerEvent>) -> Stopped {
        // Whoever lived here when the world was last saved lives here again.
        self.restore_town_npcs();
        // Before the first tick, or `tick_lunar`'s own "a pillar that was active and is not
        // standing has fallen" diff reads an empty roster as every tower having just been beaten.
        self.restore_lunar_pillars();
        // Before the first tick, or a Journey world's toggles/sliders stay at their in-memory
        // defaults until someone flips them again this session.
        self.restore_journey_powers();
        self.announce_claim_token();

        let mut ticker = interval(TICK);
        // Catching up on missed ticks would fast-forward the world clock after any stall.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let mut outcome = Stopped::Cleanly;
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    // Wrapped for the same reason the tick below is, and more urgently: this is
                    // the path every byte from an untrusted client travels. It was left bare, so
                    // a panic anywhere under `handle_packet` — or in any of the ~130 AI routines
                    // beneath it — unwound straight out of this loop, past the shutdown save at
                    // the bottom of the function, taking everything since the last autosave.
                    Some(event) => {
                        let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                            || self.handle_event(event),
                        ));
                        if handled.is_err() {
                            error!("handling a packet panicked; saving the world and stopping");
                            outcome = Stopped::Panicked;
                            break;
                        }
                    }
                    None => break,
                },
                _ = ticker.tick() => {
                    // A panic in here would otherwise take the world with it. The game is a
                    // single task and the shutdown save below lives inside it, so an unwind
                    // straight out of the loop loses everything since the last autosave. Catching
                    // it turns that into a clean stop that still writes the world out.
                    //
                    // `AssertUnwindSafe` is the honest choice rather than a safe one: the server's
                    // state may well be inconsistent after a panic. That is exactly why this saves
                    // and stops rather than carrying on.
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.tick())) {
                        Ok(cost) => {
                            self.note_tick_cost(cost);
                            if self.stopping {
                                break;
                            }
                        }
                        Err(_) => {
                            error!("the game loop panicked; saving the world and stopping");
                            outcome = Stopped::Panicked;
                            break;
                        }
                    }
                }
            }
        }

        // The channel closing is the shutdown signal, so this is the last chance to persist.
        //
        // Let a background save finish first if one is in flight. Both write through a temporary
        // file and rename, so neither can leave a half-written world — but the shutdown save has
        // the newer state and must land last, and two renames racing would decide that by
        // scheduling rather than by which is newer.
        if let Some(running) = self.saving.take() {
            let _ = running.await;
        }
        self.save_world("shutdown");
        info!("game loop stopped");
        outcome
    }

    /// Keep an eye on how much of the sixteen-millisecond budget a tick is actually using.
    ///
    /// A server that is quietly overrunning its budget looks identical to one that is not, right
    /// up until the world starts running slow. Reporting the worst tick in each ten-second window,
    /// and only when it is over half the budget, makes that visible without a line a second.
    ///
    /// Two different problems can push a tick over its budget and they need different answers, so
    /// they get different messages: work that costs too much processor is this server's bug, and a
    /// tick that took a long time without using the processor is the machine being busy elsewhere.
    /// The breakdown comes with the first one, because "a tick took 26 ms" is a mystery and "the
    /// spawn scan took 26 ms" is a bug report.
    ///
    /// The per-window `tick window` line is `debug`, so an ordinary server stays quiet; the release
    /// qualification bar ("p99 tick under budget") needs it, so `tools/soak_scale.sh` turns it on
    /// for this module alone with `TERRUSTIA_LOG=info,terrustia::game::server::tick=debug`. Every
    /// line here names the worst tick's processor time `cpu_us`, so one field carries the
    /// measurement whatever level it came out at.
    ///
    /// `tick window` is the *only* sample source, and the two lines below it are additions to the
    /// same window rather than alternatives to it: a window that is both reported and over budget
    /// emits two lines carrying the identical `cpu_us`. Anything measuring tick cost from a log
    /// must therefore select the `tick window` line first, or it double-counts exactly the heavy
    /// windows and skews its own tail. `tools/soak_scale.sh` does; it once did not.
    fn note_tick_cost(&mut self, cost: TickCost) {
        self.last_tick = cost;
        self.status_span.0 = self.status_span.0.min(cost.cpu);
        self.status_span.1 = self.status_span.1.max(cost.cpu);
        if self.ticks.is_multiple_of(STATUS_EVERY) {
            self.update_status();
            self.status_span = (Duration::MAX, Duration::ZERO);
        }
        if cost.cpu > self.worst_tick.cpu {
            self.worst_tick = cost;
        }
        self.worst_stall = self.worst_stall.max(cost.wall.saturating_sub(cost.cpu));

        // Sample how deep the outbound queues have got. Ten times a second rather than every tick:
        // the walk is over every connection, and a burst deep enough to matter for memory lasts
        // far longer than a sixth of a second, so a finer sample would cost more than it tells.
        if self.ticks.is_multiple_of(QUEUE_SAMPLE_EVERY) {
            let deepest = self
                .players
                .iter()
                .flatten()
                .filter(|p| p.is_playing())
                .map(|p| p.out.max_capacity().saturating_sub(p.out.capacity()))
                .max()
                .unwrap_or(0);
            self.queue_high_water = self.queue_high_water.max(deepest);
        }

        if !self.ticks.is_multiple_of(TICK_REPORT_EVERY) {
            return;
        }
        let worst = std::mem::take(&mut self.worst_tick);
        let stall = std::mem::take(&mut self.worst_stall);
        let queue_peak = std::mem::take(&mut self.queue_high_water);
        debug!(
            cpu_us = worst.cpu.as_micros() as u64,
            wall_us = worst.wall.as_micros() as u64,
            stall_us = stall.as_micros() as u64,
            phase = worst.worst_phase().0,
            npcs = self.npcs.len(),
            sync_full = SYNC_FULL.load(std::sync::atomic::Ordering::Relaxed),
            sync_stream = SYNC_STREAM.load(std::sync::atomic::Ordering::Relaxed),
            // The shared skip ledger, for the same reason `sync_full` and `sync_stream` are here:
            // it is a map keyed partly by projectile identity, which unlike an NPC index or a
            // player slot is not drawn from a small fixed range, so it is the one structure on
            // this path that could grow without bound. Naming its size in the window line is what
            // turns "memory is climbing" into an answer instead of a hunt. Measured at 20k to 27k
            // entries across a 255-player hold, which is the expected 255x255 plus the NPCs.
            skips = self.skips.len(),
            // The deepest single connection's backlog in this window, against a capacity of
            // `outbound_queue(max_players)`. Memory under load is queued frames, so this is the
            // number that says whether a high RSS is backlog or something else entirely.
            queue_peak,
            "tick window"
        );
        if worst.cpu * 2 > TICK {
            let (phase, phase_cost) = worst.worst_phase();
            warn!(
                // The same quantity the `tick window` line above reports, deliberately under the
                // same field name: one name for one measurement, so a person reading the log does
                // not have to know that `worst_us` here and `cpu_us` there meant the same thing.
                // This line is a *repeat* of that window's number for an operator's attention, not
                // a second measurement, so a tool sampling tick cost must read `tick window` and
                // not this. See the doc comment above.
                cpu_us = worst.cpu.as_micros() as u64,
                budget_us = TICK.as_micros() as u64,
                phase,
                phase_us = phase_cost.as_micros() as u64,
                npcs = self.npcs.len(),
                projectiles = self.projectiles.len(),
                "ticks are using a lot of their budget"
            );
        } else if stall > TICK * 6 {
            // Not a warning: nothing here is wrong, the machine is just busy. The threshold is six
            // ticks (~100 ms) rather than one, because a single-tick stall is a dropped frame nobody
            // notices, and an idle laptop that naps for a moment should not narrate it. A stall this
            // size is a real hitch a player feels, and worth one quiet line per ten-second window.
            info!(
                stall_us = stall.as_micros() as u64,
                cpu_us = worst.cpu.as_micros() as u64,
                "the game loop was held off the processor; the machine is busy elsewhere"
            );
        }
    }

    /// Take back any slot whose connection never finished joining.
    ///
    /// The connection task already has a handshake deadline, and it is not enough on its own. It
    /// stops applying once a connection has sent more than `net::connection::HANDSHAKE_FRAMES` (64)
    /// frames, because past that point the connection is treated as an established session under
    /// the ordinary idle timeout. So a client that sends sixty-five frames of *anything* - any
    /// packet at all, repeated, none of it advancing the handshake - is no longer on a deadline,
    /// and holds its player slot for as long as it keeps sending a byte inside the idle window.
    ///
    /// Sockets are separately capped (`max_connections`, `max_connections_per_address`), so that
    /// half is bounded. Player slots were not: `max_players` is at most 255 and a slot is handed
    /// out at `Join`, long before anybody has said who they are. Enough of these and the server is
    /// full of connections that will never play, and real players are told it is full.
    ///
    /// So the game task keeps its own clock. Anything that is not yet `Playing` and has been
    /// connected longer than the configured handshake timeout is kicked and its slot freed. The
    /// timeout is the same one the connection uses, and the game-side clock starts *later* (at
    /// `Join`, not at `accept`), so an ordinary slow join is always caught by the connection's own
    /// deadline first and never gets here. `handshake_timeout_secs = 0` turns this off, matching
    /// what a zero means elsewhere in [`crate::config::Config`] (`autosave_secs`).
    fn reap_stalled_handshakes(&mut self) {
        let deadline = Duration::from_secs(self.config.handshake_timeout_secs);
        if deadline.is_zero() {
            return;
        }
        // Collected first: kicking removes a player, which would invalidate an in-flight iterator.
        let stalled: Vec<(u8, std::net::SocketAddr)> = self
            .players
            .iter()
            .flatten()
            .filter(|player| !player.is_playing() && player.connected_at.elapsed() > deadline)
            .map(|player| (player.slot, player.addr))
            .collect();
        for (slot, addr) in stalled {
            warn!(
                slot,
                %addr,
                after_secs = deadline.as_secs(),
                "a connection took a slot and never finished joining; taking the slot back"
            );
            self.kick(slot, "took too long to finish joining");
        }
    }

    pub(super) fn tick(&mut self) -> TickCost {
        let mut cost = TickCost::default();
        let began = Instant::now();
        let cpu_began = clock::Cpu::now();
        // Phases are timed on the *same* clock as the tick total, which they were not: the total
        // came from `clock::Cpu` and the laps from `Instant`, so the warning line compared CPU
        // microseconds against wall microseconds and could report a phase costing more than the
        // whole tick that contained it. Every phase figure ever logged was inflated by however
        // long that phase spent descheduled. Nine extra thread-clock reads a tick is nothing —
        // it is a vDSO call — and it makes the phases add up to the total, which is the only way
        // the breakdown means anything.
        let mut clock = PhaseClock::start();
        let mut lap = |cost: &mut TickCost, phase: Phase| {
            cost.phases[phase as usize] += clock.lap();
        };

        self.ticks += 1;
        let was_day = self.world.day_time;
        // Journey mode's `FreezeTime` (`Main.cs:6342` gates the whole day/night update the same
        // way). The clock — and everything below keyed off it turning midnight or dawn — simply
        // does not run this tick; nothing here needs its own separate "and skip that too" branch.
        // `ModifyTimeRate` (`Main.cs:6343`'s own `targetTimeRate`) is the other half of the same
        // gate in source — applied here as the tick count itself rather than a separate branch,
        // since `tick_time`'s own loop already handles more than one day/night flip in one call.
        if !self.journey.freeze_time {
            self.world.tick_time(self.journey.time_rate());
            self.tick_slime_rain();
        }
        // Dawn puts the moons away and takes the blood moon with them, and rolls for an eclipse.
        if self.world.day_time && !was_day {
            self.stop_moon();
            self.world.blood_moon = false;
            // `Main.checkXMas(); Main.checkHalloween();` sit in this same dawn block
            // (`Main.cs:66375-66376`, two lines after `bloodMoon = false`), so a server left running
            // across midnight on the ninth of October starts spawning the Halloween roster on its
            // own rather than needing a restart.
            self.world.refresh_calendar();
            self.roll_dawn_events();
            self.broadcast_world_data();
        }
        // Dusk rolls for a blood moon, which needs somebody with more than a hundred and twenty
        // life to be worth having.
        if !self.world.day_time && was_day {
            self.roll_dusk_events();
        }
        if !self.world.day_time && was_day && self.world.eclipse {
            self.world.eclipse = false;
            self.announce("The solar eclipse is over.");
            self.broadcast_world_data();
        }
        self.tick_party();

        // Everything above this point is the world clock and whatever turning day or night sets
        // off: the dawn and dusk rolls, the slime rain, the party, stopping the moon, and two
        // `broadcast_world_data` calls. That is `World`'s own description ("the clock, tile
        // entities, wiring timers, lunar events and the biome census"), and it was being charged
        // to `Snapshot` instead, because this was the tick's *first* lap and a lap bills
        // everything since the previous one.
        //
        // That made the phase say the opposite of what it was added to say. `Snapshot` was split
        // out so an expensive save could not hide inside a bucket of systems; instead the bucket
        // moved inside `Snapshot`, which then read as the most expensive phase in the tick on
        // ticks where no save ran at all. A 255-player run reporting `phase=snapshot
        // phase_us=23377` had taken a snapshot costing a fraction of that.
        lap(&mut cost, Phase::World);

        if let Some(every) = self.autosave_ticks
            && self.ticks.is_multiple_of(every)
        {
            self.save_world_in_background("autosave");
        }
        // An armed save copies a few sections a tick until the buffer has caught up, then fires.
        // Does nothing at all when no save is waiting, so an ordinary tick pays nothing for it.
        self.tick_snapshot_drain();
        // Its own phase because it is the single most expensive thing the tick does, and it was
        // hidden inside a bucket of thirteen systems.
        lap(&mut cost, Phase::Snapshot);
        self.note_finished_save();
        self.note_finished_auth();
        self.reclaim_snapshot_buffer();
        if self.ticks.is_multiple_of(REAP_EVERY) {
            self.reap_stalled_handshakes();
        }
        self.tick_tile_spam();
        // What the world is worth fighting at, refreshed before anything can spawn. Cheap, and
        // keeping it here means no spawn site has to remember to scale.
        let difficulty = self.effective_difficulty();
        self.npcs.set_scaling(crate::game::npc::Scaling {
            difficulty,
            players: self
                .players
                .iter()
                .flatten()
                .filter(|p| p.is_playing())
                .count() as u32,
            // `NPC.ScaleStats_ForExpertHardmode` (`NPC.cs:18183`, `:18581`) reads both of these.
            hard_mode: self.world.progress.hard_mode,
            downed_plant_boss: self.world.progress.downed_plantera,
        });
        // Hostile-shot damage is no longer pre-scaled at launch: the wire carries the base and the
        // difficulty multiplier (and the flat x2) is applied where the game applies it, at the
        // moment the shot hits a player (`tick_contact_damage`), so a real client scales it once.

        self.tick_liquids();
        lap(&mut cost, Phase::Liquids);
        // Growth and biome spread are one world-wide sampling loop, exactly as vanilla's
        // `UpdateWorld` runs both from the same sampled tiles (`WorldGen.cs:72082`). The Spread
        // phase is timed as ~0 straight after, so the breakdown keeps its familiar shape.
        self.tick_world_update();
        lap(&mut cost, Phase::Growth);
        lap(&mut cost, Phase::Spread);
        self.tick_weather();
        lap(&mut cost, Phase::Weather);
        // Whatever is left: the tile entities, the mech cooldowns, the wiring timers, the lunar
        // event and the biome census. Individually small; kept together so the breakdown does not
        // become a wall of near-zero lines.
        self.tick_tile_entities();
        self.tick_mech_cooldowns();
        self.tick_detonators();
        self.tick_timers();
        self.tick_lunar();
        self.tick_census();
        lap(&mut cost, Phase::World);

        self.flush_dirty_sections();
        // Vanilla's own per-tick section push (`Main.cs:65601`), queueing into the same drain the
        // join stream uses so both share one bounded budget.
        self.check_player_sections();
        self.drain_section_streams();
        lap(&mut cost, Phase::Sections);
        self.tick_items();
        lap(&mut cost, Phase::Items);
        self.tick_npc_buffs();
        self.tick_town_regen();
        self.tick_npcs();
        lap(&mut cost, Phase::Npcs);
        self.tick_projectiles();
        self.tick_powders();
        lap(&mut cost, Phase::Projectiles);
        self.tick_contact_damage();
        lap(&mut cost, Phase::Damage);
        self.tick_spawning();
        lap(&mut cost, Phase::Spawning);
        self.tick_town_npcs();
        self.tick_travelling_merchant();
        self.tick_old_man();
        self.tick_cultist_tablet();
        lap(&mut cost, Phase::Housing);

        lap(&mut cost, Phase::Sync);

        cost.cpu = clock::Cpu::now().since(cpu_began);
        cost.wall = began.elapsed();
        cost
    }
}

/// Journey mode's `FreezeTime` actually stops the clock — not just the toggle sticking, the real
/// gameplay effect (`tick()`'s own gate on `self.journey.freeze_time`, mirroring `Main.cs:6342`'s
/// gate on the same power in source).
#[cfg(test)]
mod freeze_time {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "freeze time probe")
    }

    #[test]
    fn frozen_time_does_not_advance_across_many_ticks() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.journey.freeze_time = true;
        let (day_time, time) = (server.world.day_time, server.world.time);

        for _ in 0..500 {
            server.tick();
        }

        assert_eq!(
            (server.world.day_time, server.world.time),
            (day_time, time),
            "the clock should not have moved a single tick while frozen"
        );
    }

    #[test]
    fn unfreezing_lets_it_advance_again() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.journey.freeze_time = true;
        let before = server.world.time;
        for _ in 0..10 {
            server.tick();
        }
        assert_eq!(server.world.time, before, "still frozen so far");

        server.journey.freeze_time = false;
        for _ in 0..10 {
            server.tick();
        }
        assert!(
            server.world.time > before,
            "the clock should have moved once unfrozen, got {} from a start of {before}",
            server.world.time
        );
    }
}

/// Journey mode's `ModifyTimeRate` actually changes how fast the clock runs — `tick()`'s own
/// `self.journey.time_rate()` argument to `tick_time`, mirroring `Main.cs:6343`'s own
/// `targetTimeRate` read in source.
#[cfg(test)]
mod modify_time_rate {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "time rate probe")
    }

    #[test]
    fn the_top_of_the_slider_advances_the_clock_twenty_four_times_as_fast() {
        let mut baseline = GameServer::new(Config::default(), tiny_world());
        let mut sped_up = GameServer::new(Config::default(), tiny_world());
        sped_up.journey.time_rate_slider = 1.0; // the slider's real top: 24x

        // Deltas, not absolute values: `new()`'s own startup work (angler quest roll and friends)
        // can leave `world.time` non-zero before the first real tick, which a bare before/after-one-
        // tick comparison would otherwise fold into the "24x" ratio and make it come out wrong.
        let (before_baseline, before_sped) = (baseline.world.time, sped_up.world.time);
        baseline.tick();
        sped_up.tick();
        let (moved_baseline, moved_sped) = (
            baseline.world.time - before_baseline,
            sped_up.world.time - before_sped,
        );

        assert_eq!(
            moved_baseline, 1,
            "an ordinary tick should move the clock by exactly one"
        );
        assert_eq!(
            moved_sped, 24,
            "one tick at the slider's top should move the clock 24 real ticks' worth"
        );
    }
}

/// Do the tick's phases and its total actually describe the same thing?
///
/// They did not. `worst_us` came from `clock::Cpu` and `phase_us` from `Instant`, so the warning
/// line compared CPU microseconds against wall microseconds. A real two-hour run logged three
/// ticks where the phase cost *more than the whole tick containing it* — which is impossible, and
/// meant every phase figure was inflated by however long that phase spent descheduled. All of
/// Stage 2's measurement rests on these numbers, so the invariant is pinned here.
#[cfg(test)]
mod tick_accounting {
    use super::*;
    use crate::config::Config;
    use crate::world::World;

    #[tokio::test]
    async fn no_phase_can_cost_more_than_its_own_tick() {
        let mut server = GameServer::new(Config::default(), World::empty(600, 400, "accounting"));

        for _ in 0..20 {
            let cost = server.tick();
            let (name, worst) = cost.worst_phase();
            assert!(
                worst <= cost.cpu,
                "phase {name} cost {worst:?} of a tick that cost {:?} — the two are being \
                 measured on different clocks again",
                cost.cpu
            );

            // And the parts must add up to the whole, not merely each be smaller than it.
            let summed: Duration = cost.phases.iter().sum();
            assert!(
                summed <= cost.cpu,
                "the phases sum to {summed:?} but the tick cost {:?}",
                cost.cpu
            );
        }
    }

    /// Wall clock is still recorded separately, because telling "we are slow" from "the machine
    /// is busy" is the reason this instrumentation exists at all.
    #[tokio::test]
    async fn wall_clock_is_still_measured_apart_from_processor_time() {
        let mut server = GameServer::new(Config::default(), World::empty(300, 200, "accounting"));
        let cost = server.tick();
        assert!(
            cost.wall >= cost.cpu,
            "a tick cannot use more processor than it took: cpu {:?}, wall {:?}",
            cost.cpu,
            cost.wall
        );
    }

    /// Every phase has a name, so a breakdown can never print an index.
    #[test]
    fn every_phase_is_named() {
        assert_eq!(Phase::NAMES.len(), Phase::Sync as usize + 1);
    }

    /// The property the fix actually turns on: time spent off the processor is not phase time.
    ///
    /// This is the test that catches the bug, and the reason the two above do not. On an idle
    /// machine wall clock and CPU clock agree, so "no phase exceeds its tick" passes happily
    /// against the broken code — verified by reverting the fix and watching it stay green.
    /// Sleeping forces the two clocks apart on purpose, which is the only reliable way to tell
    /// them apart without a loaded machine.
    #[test]
    fn a_phase_does_not_charge_for_time_spent_descheduled() {
        let mut clock = PhaseClock::start();
        std::thread::sleep(Duration::from_millis(40));
        let charged = clock.lap();
        assert!(
            charged < Duration::from_millis(5),
            "a phase that slept for 40ms was charged {charged:?}; phases are on the wall clock \
             again, which inflates every figure the breakdown prints"
        );
    }

    /// And it does still charge for work, so the clock is not simply stuck at zero.
    ///
    /// Doubling rather than a fixed batch, for the reason `game::clock`'s own
    /// `work_costs_processor_time` gives: Windows reports thread CPU time in ~15.6 ms scheduler
    /// ticks, so a fixed four million multiplies asserts the clock's resolution rather than that
    /// it charges at all, and read exactly zero there. A stuck clock still fails, at the deadline.
    #[test]
    fn a_phase_does_charge_for_work() {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut rounds = 4_000_000u64;
        loop {
            let mut clock = PhaseClock::start();
            let mut total = 0u64;
            for i in 0..rounds {
                total = total.wrapping_add(i * i);
            }
            std::hint::black_box(total);
            if clock.lap() > Duration::ZERO {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "a whole second of real work was charged nothing: the phase clock is stuck"
            );
            rounds *= 2;
        }
    }
}

/// A connection that takes a slot and never finishes joining must not keep it (P2c).
///
/// The escape route the reaper closes: `net::connection::read_loop` only holds a connection to the
/// handshake deadline while it has sent at most `HANDSHAKE_FRAMES` (64) frames. The sixty-fifth
/// frame - of anything at all - promotes it to "established session" and puts it on the idle
/// timeout instead, which resets on every byte. Its player slot was then held for as long as it
/// cared to trickle.
#[cfg(test)]
mod handshake_reaper {
    use super::*;
    use crate::config::Config;
    use crate::game::player::ConnState;
    use bytes::Bytes;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "reaper probe")
    }

    /// A server whose handshake deadline is one second, so a test never has to wait one.
    fn server_with_a_short_deadline() -> GameServer {
        GameServer::new(
            Config {
                handshake_timeout_secs: 1,
                ..Config::default()
            },
            tiny_world(),
        )
    }

    /// Take a slot the way `ServerEvent::Join` does, and leave the connection in `state`.
    ///
    /// The receiver is handed back and must be kept alive: dropping it closes the outbound queue,
    /// and the server treats a closed queue as a connection that has already gone - which would
    /// remove the player for a reason that has nothing to do with the reaper.
    fn join(server: &mut GameServer, state: ConnState) -> (u8, mpsc::Receiver<Bytes>) {
        let (tx, rx) = mpsc::channel(16);
        let (slot, _epoch) = server
            .allocate_slot(
                "127.0.0.1:5000".parse().expect("a literal"),
                tx,
                tokio::sync::oneshot::channel().0,
            )
            .expect("a free slot");
        server.player_mut(slot).expect("the slot just taken").state = state;
        (slot, rx)
    }

    /// Backdate a connection so it is already past the deadline.
    fn make_it_old(server: &mut GameServer, slot: u8) {
        let player = server.player_mut(slot).expect("the slot under test");
        player.connected_at = Instant::now()
            .checked_sub(Duration::from_secs(30))
            .expect("a monotonic clock at least thirty seconds old");
    }

    #[test]
    fn a_slot_held_before_playing_past_the_deadline_is_reclaimed() {
        let mut server = server_with_a_short_deadline();
        // `Identified` is past the point a slot is assigned and short of `Playing`: the client has
        // sent a name and then stopped, which is exactly the shape that used to hold a slot for
        // ever once it had sent enough frames to escape the connection-level deadline.
        let (slot, _held) = join(&mut server, ConnState::Identified);
        make_it_old(&mut server, slot);

        server.reap_stalled_handshakes();

        assert!(
            server.players[slot as usize].is_none(),
            "a connection that never finished joining must not keep its slot"
        );
    }

    #[test]
    fn every_pre_playing_state_is_reclaimed_not_just_one() {
        for state in [
            ConnState::Greeting,
            ConnState::SlotAssigned,
            ConnState::Identified,
            ConnState::WorldSent,
            ConnState::TilesSent,
        ] {
            let mut server = server_with_a_short_deadline();
            let (slot, _held) = join(&mut server, state);
            make_it_old(&mut server, slot);
            server.reap_stalled_handshakes();
            assert!(
                server.players[slot as usize].is_none(),
                "a connection stuck at {state:?} must be reclaimed too"
            );
        }
    }

    #[test]
    fn a_playing_player_is_never_reaped_however_long_they_have_been_here() {
        let mut server = server_with_a_short_deadline();
        let (slot, _held) = join(&mut server, ConnState::Playing);
        make_it_old(&mut server, slot);

        server.reap_stalled_handshakes();

        assert!(
            server.players[slot as usize].is_some(),
            "somebody who is actually playing has finished the handshake; a long session is not a \
             stalled one"
        );
    }

    #[test]
    fn a_join_still_inside_the_deadline_is_left_alone() {
        let mut server = server_with_a_short_deadline();
        let (slot, _held) = join(&mut server, ConnState::Identified);
        // Not backdated: an ordinary join in progress.

        server.reap_stalled_handshakes();

        assert!(
            server.players[slot as usize].is_some(),
            "a join that is merely in progress must be given its full deadline"
        );
    }

    /// Zero means off, the same as it does for `autosave_secs`.
    #[test]
    fn a_zero_timeout_turns_the_reaper_off_rather_than_kicking_everyone_instantly() {
        let mut server = GameServer::new(
            Config {
                handshake_timeout_secs: 0,
                ..Config::default()
            },
            tiny_world(),
        );
        let (slot, _held) = join(&mut server, ConnState::Identified);
        make_it_old(&mut server, slot);

        server.reap_stalled_handshakes();

        assert!(server.players[slot as usize].is_some());
    }

    /// And it is actually wired into the tick, rather than only being callable.
    ///
    /// Sixty ticks, because the sweep runs once a second rather than every frame. Deleting the
    /// `reap_stalled_handshakes` call from `tick` fails this one and leaves the rest passing, which
    /// is the point of having it.
    #[test]
    fn the_tick_runs_the_reaper_on_its_own() {
        let mut server = server_with_a_short_deadline();
        let (slot, _held) = join(&mut server, ConnState::Identified);
        make_it_old(&mut server, slot);

        for _ in 0..REAP_EVERY {
            server.tick();
        }

        assert!(
            server.players[slot as usize].is_none(),
            "the tick must reclaim a stalled slot without anybody asking it to"
        );
    }

    /// The freed slot is genuinely available again, which is the whole point of freeing it.
    #[test]
    fn the_reclaimed_slot_can_be_handed_to_somebody_else() {
        let mut server = GameServer::new(
            Config {
                handshake_timeout_secs: 1,
                max_players: 1,
                ..Config::default()
            },
            tiny_world(),
        );
        let (slot, _held) = join(&mut server, ConnState::Identified);
        let (tx, _rx) = mpsc::channel(16);
        assert!(
            server
                .allocate_slot(
                    "127.0.0.1:5001".parse().expect("a literal"),
                    tx,
                    tokio::sync::oneshot::channel().0,
                )
                .is_none(),
            "the one-slot server should be full while the stalled connection holds it"
        );

        make_it_old(&mut server, slot);
        server.reap_stalled_handshakes();

        let (tx, _rx) = mpsc::channel(16);
        assert!(
            server
                .allocate_slot(
                    "127.0.0.1:5001".parse().expect("a literal"),
                    tx,
                    tokio::sync::oneshot::channel().0,
                )
                .is_some(),
            "a real player must be able to have the slot back"
        );
    }
}

/// A world that cannot be written must not cost the server, and must not be a secret.
///
/// The rule the whole of this is built on: a failed save is a condition to survive, not to die of.
/// The world in memory is still the good one and the previous save on disk is still intact, so
/// stopping would throw away exactly the state the operator is trying to keep. What must happen
/// instead is that it retries, that the log says so from the first failure, that the panel can see
/// it, and that once it stops being a blip the people whose progress is at risk are told in chat.
#[cfg(test)]
mod failing_saves {
    use super::*;
    use crate::config::Config;
    use crate::game::player::{ConnState, Player};
    use crate::game::server::SAVE_FAILURES_BEFORE_ALARM;
    use bytes::Bytes;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "failing saves probe")
    }

    /// The panel pinned a failing save twice; the terminal had one `error!` that scrolls away.
    /// The footer is the only part of the terminal that does not scroll.
    #[test]
    fn the_status_footer_names_failing_saves() {
        let note = save_failure_note(crate::term::Palette::PLAIN, 3);
        assert!(
            note.contains("saves failing (3)"),
            "an operator watching the footer must see it: {note:?}"
        );
    }

    /// And a healthy server's footer is exactly what it always was, with nothing added.
    #[test]
    fn a_healthy_server_adds_nothing_to_the_footer() {
        assert_eq!(save_failure_note(crate::term::Palette::PLAIN, 0), "");
    }

    /// A player in `ConnState::Playing`, with their outbound queue kept so what the server said to
    /// them can be read back. Deep enough that nothing under test can fill it: a full queue drops
    /// the connection, which would make an assertion about a missing message a lie.
    fn seat_player(server: &mut GameServer, slot: u8) -> mpsc::Receiver<Bytes> {
        let (tx, rx) = mpsc::channel(64);
        let mut player = Player::new(slot, "127.0.0.1:4000".parse().expect("a literal"), tx);
        player.state = ConnState::Playing;
        server.players[slot as usize] = Some(player);
        rx
    }

    /// How many queued frames carry this text.
    ///
    /// A `NetworkText::literal` goes onto the wire as a length-prefixed UTF-8 string, so the words
    /// are in the frame verbatim and a substring search over the bytes needs no decoder. Drains the
    /// queue, so each call asks about what has been said *since the last call*.
    fn frames_saying(rx: &mut mpsc::Receiver<Bytes>, needle: &str) -> usize {
        let mut found = 0;
        while let Ok(frame) = rx.try_recv() {
            if frame
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
            {
                found += 1;
            }
        }
        found
    }

    /// Report a finished background save without one having run, so the escalation can be walked
    /// through failure by failure.
    fn report(server: &mut GameServer, outcome: Result<u64, ()>) {
        server
            .save_results
            .0
            .send(outcome)
            .expect("the game task owns the receiving end");
        server.note_finished_save();
    }

    #[test]
    fn a_single_failed_autosave_warns_and_retries_without_telling_the_players() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let mut rx = seat_player(&mut server, 0);
        server.save_reason = "autosave";

        report(&mut server, Err(()));

        assert_eq!(server.save_failures, 1, "the failure must be counted");
        assert!(!server.stopping, "a failed save must never stop the server");
        assert_eq!(
            frames_saying(&mut rx, "at risk"),
            0,
            "one failure is a blip; interrupting everybody's game for it teaches them to ignore \
             the message that matters"
        );
    }

    #[test]
    fn consecutive_failures_escalate_to_an_in_game_warning() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let mut rx = seat_player(&mut server, 0);
        server.save_reason = "autosave";

        for n in 1..SAVE_FAILURES_BEFORE_ALARM {
            report(&mut server, Err(()));
            assert_eq!(server.save_failures, n);
            assert_eq!(
                frames_saying(&mut rx, "at risk"),
                0,
                "nothing should be broadcast before failure {SAVE_FAILURES_BEFORE_ALARM}"
            );
        }

        report(&mut server, Err(()));
        assert_eq!(server.save_failures, SAVE_FAILURES_BEFORE_ALARM);
        assert_eq!(
            frames_saying(&mut rx, "at risk"),
            1,
            "crossing the threshold must reach the players, not only the log"
        );
        assert!(!server.stopping, "and still must not stop the server");

        // Every failure past the threshold repeats it: somebody who joined since the first warning
        // has no other way of knowing the server is in this state.
        report(&mut server, Err(()));
        assert_eq!(frames_saying(&mut rx, "at risk"), 1);
    }

    #[test]
    fn a_save_that_works_again_clears_the_state_and_says_so() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let mut rx = seat_player(&mut server, 0);
        server.save_reason = "autosave";

        for _ in 0..SAVE_FAILURES_BEFORE_ALARM {
            report(&mut server, Err(()));
        }
        assert_eq!(frames_saying(&mut rx, "at risk"), 1, "the alarm went out");

        report(&mut server, Ok(12));
        assert_eq!(server.save_failures, 0, "one success clears the state");
        assert_eq!(
            frames_saying(&mut rx, "working again"),
            1,
            "whoever heard the alarm is owed the all-clear"
        );

        // And the next failure starts from the beginning rather than from where it left off.
        report(&mut server, Err(()));
        assert_eq!(server.save_failures, 1);
        assert_eq!(frames_saying(&mut rx, "at risk"), 0);
    }

    /// The all-clear is owed to the people who heard the alarm, and to nobody else.
    #[test]
    fn a_recovery_nobody_was_warned_about_is_not_announced() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let mut rx = seat_player(&mut server, 0);
        server.save_reason = "autosave";

        report(&mut server, Err(()));
        report(&mut server, Ok(9));

        assert_eq!(server.save_failures, 0);
        assert_eq!(
            frames_saying(&mut rx, "working again"),
            0,
            "announcing the recovery would be the first the players had heard of the problem"
        );
    }

    #[test]
    fn the_panel_status_carries_the_saves_failing_indicator() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.save_reason = "autosave";

        let ask = |server: &mut GameServer| {
            let (reply, mut rx) = tokio::sync::oneshot::channel();
            server.handle_event(ServerEvent::PanelStatus { reply });
            rx.try_recv().expect("the panel is always answered")
        };

        assert_eq!(ask(&mut server).save_failures, 0, "healthy is zero");
        report(&mut server, Err(()));
        report(&mut server, Err(()));
        assert_eq!(ask(&mut server).save_failures, 2);
        report(&mut server, Ok(3));
        assert_eq!(ask(&mut server).save_failures, 0, "a success clears it");
    }

    /// The same escalation, driven by a real filesystem failure through the real save path.
    ///
    /// The tests above report outcomes down the channel directly, which pins the state machine but
    /// takes it on trust that a genuine unwritable directory produces `Err` at the other end. This
    /// one makes the directory unwritable for real, runs the actual background save the tick runs,
    /// and waits for it - so the whole chain from `wld_save::save` through `spawn_blocking` to the
    /// broadcast is exercised end to end. It also proves the retry: the save that follows the
    /// permissions being put back succeeds, with no restart in between.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_unwritable_directory_escalates_and_then_recovers_on_its_own() {
        let dir = crate::safe_write::tests::temp_dir("autosave-readonly");
        let path = dir.join("world.wld");
        let config = Config {
            save_file: Some(path.clone()),
            ..Config::default()
        };
        let mut server = GameServer::new(config, tiny_world());
        let mut rx = seat_player(&mut server, 0);

        // One save while everything is fine, so there is a good world on disk to protect.
        async fn save_once(server: &mut GameServer) {
            server.save_world_in_background("autosave");
            if let Some(handle) = server.saving.take() {
                handle.await.expect("the writer thread must not panic");
            }
            server.note_finished_save();
        }
        save_once(&mut server).await;
        assert_eq!(server.save_failures, 0, "the first save should have worked");
        let good = std::fs::read(&path).expect("a world on disk");

        let Some(guard) = crate::safe_write::tests::ReadOnlyDir::new(&dir) else {
            eprintln!("skipping: this environment cannot make a directory read-only");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        for _ in 0..SAVE_FAILURES_BEFORE_ALARM {
            save_once(&mut server).await;
        }
        assert_eq!(server.save_failures, SAVE_FAILURES_BEFORE_ALARM);
        assert!(!server.stopping, "a full disk must not stop the server");
        assert_eq!(
            frames_saying(&mut rx, "at risk"),
            1,
            "the players must have been told"
        );
        assert_eq!(
            std::fs::read(&path).expect("reading the world back"),
            good,
            "and the last good save must be exactly as it was"
        );

        // The disk comes back. Nothing restarts; the next autosave simply works.
        drop(guard);
        save_once(&mut server).await;
        assert_eq!(server.save_failures, 0, "the retry must have succeeded");
        assert_eq!(
            frames_saying(&mut rx, "working again"),
            1,
            "and the all-clear must have gone out"
        );
        // The recovered save is a real world, not merely a file that exists. (Its bytes match the
        // previous one, and should: nothing about this world changed between the two saves.)
        let bytes = std::fs::read(&path).expect("reading the world back");
        crate::world::wld::parse(&bytes).expect("the recovered save must be loadable");
        assert!(
            !path.with_extension("wld.tmp").exists(),
            "and the failed attempts must not have left scratch files behind"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// An autosave must not copy the whole changed world inside one tick.
///
/// A real 1h49m run spiked `phase=snapshot` to 24,808 us against a 16,666 us budget, on a world
/// with two NPCs and nobody connected, where a normal tick was 103 us. The copy is now drained a
/// few sections a tick and the save fires only on a tick where nothing is left marked, which is
/// also the tick on which the buffer is bit-identical to the live world.
#[cfg(test)]
mod deferred_snapshot {
    use super::*;
    use crate::config::Config;
    use crate::game::server::SNAPSHOT_DRAIN_PER_TICK;

    /// Ten sections across by four down. Big enough that a drain of eight a tick cannot finish in
    /// the tick that armed the save, which is the whole point.
    fn wide_world() -> crate::world::World {
        crate::world::World::empty(2000, 600, "deferred snapshot probe")
    }

    #[tokio::test]
    async fn a_save_waits_for_the_snapshot_buffer_and_still_lands_whole() {
        let dir = crate::safe_write::tests::temp_dir("deferred-snapshot");
        let path = dir.join("world.wld");
        let mut server = GameServer::new(
            Config {
                save_file: Some(path.clone()),
                ..Config::default()
            },
            wide_world(),
        );

        // One edit in every section, so the drain has real work and every section has a witness
        // tile whose absence from the file on disk would be a torn save.
        let (across, down) = (server.world.sections_x(), server.world.sections_y());
        let sections = (across * down) as usize;
        assert!(
            sections > SNAPSHOT_DRAIN_PER_TICK,
            "the probe world has to be wider than one tick's drain"
        );
        for sy in 0..down {
            for sx in 0..across {
                server
                    .world
                    .set_tile(sx * 200, sy * 150, terrustia_proto::Tile::block(1));
            }
        }
        assert_eq!(server.world.snapshot_pending(), sections);

        server.save_world_in_background("autosave");
        assert!(
            server.saving.is_none(),
            "asking for a save must not copy {sections} sections inside the tick that asked"
        );
        assert_eq!(
            server.pending_save,
            Some("autosave"),
            "it must be armed, not dropped"
        );

        let mut ticks = 0;
        while server.saving.is_none() {
            server.tick();
            ticks += 1;
            assert!(ticks < 100, "the drain never fired the save");
        }
        assert_eq!(
            ticks,
            sections.div_ceil(SNAPSHOT_DRAIN_PER_TICK),
            "the drain should take exactly as many ticks as the cap says"
        );
        assert_eq!(
            server.world.snapshot_pending(),
            0,
            "and it must fire on a tick where the buffer had caught up, not before"
        );

        server
            .saving
            .take()
            .expect("a save was started")
            .await
            .expect("the writer thread must not panic");

        let saved = crate::world::wld::load(&path).expect("the save must be loadable");
        for sy in 0..down {
            for sx in 0..across {
                assert_eq!(
                    saved.tile(sx * 200, sy * 150).block,
                    1,
                    "section {sx},{sy} was not in the file: the save tore"
                );
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A world that dirties faster than the drain clears it must still get saved.
    ///
    /// Without the deadline this postpones for ever, which risks more than a stutter does.
    #[tokio::test]
    async fn a_world_that_never_settles_saves_anyway_once_the_deadline_passes() {
        use crate::game::server::SNAPSHOT_DRAIN_DEADLINE;

        let dir = crate::safe_write::tests::temp_dir("deferred-snapshot-livelock");
        let path = dir.join("world.wld");
        let mut server = GameServer::new(
            Config {
                save_file: Some(path.clone()),
                ..Config::default()
            },
            wide_world(),
        );

        let (across, down) = (server.world.sections_x(), server.world.sections_y());
        let dirty_everything = |server: &mut GameServer| {
            for sy in 0..down {
                for sx in 0..across {
                    server
                        .world
                        .set_tile(sx * 200, sy * 150, terrustia_proto::Tile::block(1));
                }
            }
        };

        dirty_everything(&mut server);
        server.save_world_in_background("autosave");

        let mut ticks = 0;
        while server.saving.is_none() {
            dirty_everything(&mut server);
            server.tick();
            ticks += 1;
            assert!(
                ticks <= SNAPSHOT_DRAIN_DEADLINE + 1,
                "the deadline must have forced the save through by now"
            );
        }
        assert_eq!(
            ticks, SNAPSHOT_DRAIN_DEADLINE,
            "and it must wait the full deadline first, not give up early"
        );

        server
            .saving
            .take()
            .expect("a save was started")
            .await
            .expect("the writer thread must not panic");
        crate::world::wld::load(&path).expect("the forced save must still be a real world");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
