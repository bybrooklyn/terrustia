use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use terrustia::{
    config::Config,
    console,
    game::{GameServer, ServerEvent, Stopped},
    net::listener,
    term::{self, Palette},
    world::{wld, worldgen},
};
use tokio::{signal, sync::mpsc};
use tracing::{error, info, warn};
use tracing_subscriber::{filter::Targets, layer::SubscriberExt, util::SubscriberInitExt};

/// Events queued from all connections before the game task applies backpressure.
const EVENT_QUEUE: usize = 4096;

#[tokio::main]
async fn main() -> ExitCode {
    // `Targets` understands the same `terrustia=debug,info` syntax as `EnvFilter` but is a plain
    // prefix matcher, so it costs no regex engine.
    let filter = std::env::var("TERRUSTIA_LOG")
        .ok()
        .and_then(|spec| spec.parse::<Targets>().ok())
        .unwrap_or_else(|| Targets::new().with_default(tracing::Level::INFO));
    let palette = Palette::detect();
    tracing_subscriber::registry()
        .with(term::TermLayer::new(palette))
        .with(filter)
        .init();

    // `terrustia update` is a subcommand, not a flag: it does its own thing entirely (check
    // GitHub, verify, download, apply) and never starts a server. Handled before `Args::parse`
    // even sees the rest of the arguments, the same way a bare word ahead of any flag would
    // otherwise just be reported as "unrecognised argument".
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if raw_args.first().map(String::as_str) == Some("update") {
        return match terrustia::update::run_update_command(&raw_args[1..]).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                error!("{e}");
                ExitCode::FAILURE
            }
        };
    }

    match run(palette).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(palette: Palette) -> Result<(), Box<dyn std::error::Error>> {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let mut args = Args::parse(raw_args.iter().cloned())?;
    if args.help {
        print_usage(palette);
        return Ok(());
    }
    if args.list_worlds {
        print_worlds(palette);
        return Ok(());
    }

    // Opt-in-triggered, not the zero-flag default path: `--setup` always asks; a genuinely
    // fresh, no-flags launch only asks when `should_auto_trigger` recognises the specific shape
    // of "just downloaded the raw binary and ran it right where it landed" — see `setup.rs`'s own
    // module doc for exactly what that check is and, just as importantly, is not. Either way this
    // only ever changes which config file `--config` effectively points at from here on; every
    // later precedence rule (environment, an explicit flag) keeps working unchanged.
    if args.setup || terrustia::setup::should_auto_trigger(raw_args.is_empty()) {
        let config_path = tokio::task::spawn_blocking(terrustia::setup::run_wizard)
            .await
            .map_err(|e| format!("the setup wizard panicked: {e}"))??;
        args.config = config_path;
    }

    print!(
        "{}",
        term::banner(
            palette,
            env!("CARGO_PKG_VERSION"),
            GAME_VERSION,
            terrustia_proto::id::CUR_RELEASE
        )
    );

    let mut config = Config::load(&args.config)?;
    // Layered between the file and the CLI flags below, matching every other host convention:
    // defaults < file < environment < explicit flag. Docker/automation-friendly config that needs
    // no file on disk and no shell around the process to pass flags either.
    config.apply_env()?;
    if let Some(listen) = args.listen {
        config.listen = listen;
    }
    // One-way, like every other boolean flag here: `--panel` turns it on, and turning it off again
    // is the config file's or the environment's job. A `--no-panel` would be the only negative flag
    // on the surface and nobody has wanted one.
    if args.panel {
        config.panel_enabled = true;
    }
    if let Some(seed) = &args.seed {
        // Keeps `config.seed` meaningful for anything that reads it besides generation itself
        // (e.g. a numeric `--seed` still round-trips exactly). The word/secret-seed path below,
        // through `worldgen::build_from_text`, re-derives this same number from `seed` itself
        // rather than reading it back from here — see that function's own doc comment for why a
        // typed seed is not pre-split into "the number" and "the text" before reaching it.
        config.seed = worldgen::secret_seed::numeric_seed(seed);
    }
    if let Some(world_file) = args.world {
        config.world_file = Some(world_file);
    }
    if let Some(name) = args.new_world {
        let destination = terrustia::worlds::new_world_path(&name)?;
        if destination.exists() {
            return Err(format!(
                "a world named \"{name}\" already exists at {} — pick another name, or serve it \
                 with --world {name}",
                destination.display()
            )
            .into());
        }
        // The world directory itself may not exist yet — nothing has ever saved there, which on
        // a fresh machine (nobody has run Terraria itself, or this is a fresh headless install)
        // is the ordinary case, not an error to stop on.
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "could not create the world directory {}: {e}",
                    parent.display()
                )
            })?;
        }
        // `--new` means "generate fresh", full stop — it must win even over a `world_file` that
        // came from the config file or environment (layered in above, before any CLI flag is
        // read), or the match below would silently load that stale world instead of generating
        // one, only redirecting where the result gets saved. `args.world` can never be the reason
        // `config.world_file` is set here: `Args::parse` already rejects `--world` and `--new`
        // together, so the `if let Some(world_file) = args.world` block above never ran when this
        // one did.
        config.world_file = None;
        config.world_name = name;
        config.save_file = Some(destination);
    }
    if let Some(save_file) = args.save {
        config.save_file = Some(save_file);
    }
    // `Config::load` already ran `validate` once, but only against whatever `world_file` the file
    // and environment layers left in place — `validate` skips width/height/section-alignment
    // checks entirely whenever `world_file.is_some()` (a loaded world brings its own dimensions,
    // so those checks don't apply to it). `--new`, just above, can clear `world_file` after that
    // first validation already ran, which would otherwise let a stale config's own out-of-range
    // `world_width`/`world_height` reach real generation completely unvalidated — re-run it now
    // against the final, fully-layered config so that case is covered too.
    config.validate()?;

    // Minecraft-style placement: a generated world with nowhere else to go persists into the
    // server's own worlds/ directory, so a server run from a folder sets that folder up rather than
    // serving something that vanishes on shutdown. An explicit --world, --new, --save, or a config
    // save_file all win over this, since they already leave a world_file or a save target in place.
    if config.world_file.is_none()
        && config.save_target().is_none()
        && let Ok(path) = terrustia::worlds::new_world_path(&config.world_name)
    {
        config.save_file = Some(path);
    }
    // Create the directory a world will save into before the first save reaches for it, so worlds/
    // exists the moment it is needed rather than failing the first autosave.
    if let Some(parent) = config
        .save_target()
        .and_then(|t| t.parent().map(|p| p.to_path_buf()))
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(&parent).map_err(|e| {
            format!(
                "cannot create the world directory {}: {e}",
                parent.display()
            )
        })?;
    }
    // Last, once every layer above has had its say about where saves go: if that place already
    // holds a world and nothing named one to load, load it. This is what makes a restart resume
    // rather than regenerate — see `Config::resume_from_save_target` for the three flows that
    // reached the `None` arm below on every boot and autosaved a fresh world over a real save.
    // Placed before the stage label so "loading world" and the match below cannot disagree.
    config.resume_from_save_target();

    let started = Instant::now();
    // One spinner covers the single slow step, generating or loading the world. It clears when the
    // world is ready and the boot card below takes its place; every other boot step is quick and
    // reports itself as a row in that card rather than as its own ✓ line.
    let world_stage = term::Stage::begin(
        palette,
        match &config.world_file {
            Some(_) => "loading world",
            None => "generating world",
        },
    );
    // `World::secret_seeds` is read straight off the world either way now — a loaded world's own
    // real flag bytes (`wld.rs`'s own read path), or freshly detected here for a generated one —
    // rather than threaded through as a separate `Built`-only value that a loaded world could
    // never have. See `worldgen::secret_seed`'s own module doc for the real persistence mechanism.
    let world = match &config.world_file {
        Some(path) => wld::load(path)?,
        None => {
            let (world, _built) = match &args.seed {
                // The one real entry point that actually reaches
                // `worldgen::secret_seed::SecretSeeds::detect` — every other caller of
                // `worldgen::build`/`generate` across this workspace never has real seed text to
                // give it and is left alone.
                Some(seed_text) => worldgen::build_from_text(
                    config.world_width,
                    config.world_height,
                    config.world_name.clone(),
                    seed_text,
                ),
                None => worldgen::build(
                    config.world_width,
                    config.world_height,
                    config.world_name.clone(),
                    config.seed,
                ),
            };
            world
        }
    };
    world_stage.clear();

    // Bind before starting the game task so a port clash fails fast, and before the card so a
    // failure here can never print beneath a "ready" line that would be a lie. `listener::bind`
    // turns a bare errno (a port in use, or `os error 28` socket exhaustion) into a message that
    // says what to do about it.
    let listener = listener::bind(config.listen, "--listen").await?;

    let recorder = match &args.record {
        Some(path) => Some(terrustia::net::record::Recorder::create(path)?),
        None => None,
    };

    let (events_tx, events_rx) = mpsc::channel::<ServerEvent>(EVENT_QUEUE);

    // Started before the card so the panel's on/off state is a fact the card can state. Opt-in, so
    // a bind failure *here* — at boot, before anything is actually serving — is a configuration
    // mistake worth failing loudly on rather than silently running without it. Once up, ownership
    // passes to `panel::supervise` below, which handles every later start/stop (the console's
    // `panel` command) without that same all-or-nothing behaviour — see its own doc comment for why
    // a runtime toggle failure should not take the rest of the server down too.
    // Both states say what to do next. Off used to be the bare word "off", which named a feature
    // and then gave the reader nowhere to go: there was no flag for it at all, and the two ways in
    // were a config key and an environment variable that this card never mentioned. On used to say
    // "loopback only" without the address, so anyone wanting to open it had to go and find the
    // default port. The claim-token warning printed moments later already sets the house standard
    // by giving the exact command to type.
    let (initial_panel, panel_state) = if config.panel_enabled {
        let handle = terrustia::panel::run(config.clone(), events_tx.clone()).await?;
        (
            Some(handle),
            format!("http://{} · loopback only", config.panel_listen),
        )
    } else {
        (None, "off · start it with --panel".to_string())
    };

    // The settled facts, one aligned key/value card on a single left margin: what the world is,
    // where it listens, where it saves, and whether the panel is up. Paths are shown short — a
    // server-owned world in worlds/ reads as `worlds/Name.wld`, the way a Minecraft server names its
    // own files, and anything under home collapses to `~`.
    let evil = if world.crimson {
        "crimson"
    } else {
        "corruption"
    };
    let mut rows: Vec<(&str, String)> = vec![(
        "world",
        format!(
            "{} · {} × {} · {}",
            world.name,
            world.width(),
            world.height(),
            evil
        ),
    )];
    if world.secret_seeds.any() {
        rows.push(("seed", world.secret_seeds.active_names().join(", ")));
    }
    rows.push((
        "listening",
        format!("{} · up to {} players", config.listen, config.max_players),
    ));
    rows.push((
        "saves to",
        match config.save_target() {
            None => "nowhere, this world will not be saved".to_string(),
            Some(path) => terrustia::worlds::display_path(path),
        },
    ));
    rows.push((
        "autosave",
        if config.autosave_secs == 0 {
            "off".to_string()
        } else {
            format!("every {}s", config.autosave_secs)
        },
    ));
    rows.push(("web panel", panel_state.to_string()));
    print!("{}", term::info_block(palette, &rows));
    print!("{}", term::ready_line(palette, started.elapsed()));
    let (panel_toggle_tx, panel_toggle_rx) = mpsc::unbounded_channel();
    // Handle kept and aborted below, alongside `accept`/`console` — this task holds its own clone
    // of `events_tx` for as long as it runs (it has to, to start the panel on a later toggle), so
    // leaving it unaborted here was a real, found-by-actually-testing-it deadlock: `main`'s own
    // `drop(events_tx)` during shutdown was never actually the *last* sender while this task kept
    // running, so the game task's `events.recv() => None => break` exit path could never fire, and
    // a real SIGTERM sat there logging "shutting down" while the game loop kept ticking and
    // autosaving forever, never actually stopping — exactly what `packaging/terrustia.service`'s
    // `TimeoutStopSec=90` exists to eventually paper over with a hard kill, defeating the graceful
    // shutdown save that whole unit is built around. Found by actually sending a real `SIGTERM` to
    // a real running process while verifying that unit's `ExecStart` path, not by inspection.
    //
    // That fix was still incomplete whenever the panel was actually *running* at shutdown time (not
    // just wired up with `initial_panel: None`, ready to be started later): `.abort()` below only
    // cancels `supervise`'s own outer task, and cancelling it used to just drop `supervise`'s local
    // handle to the real inner axum-serving task — which detaches rather than stops it, leaking a
    // live clone of `events_tx` right back into the same deadlock this comment already describes,
    // just one level down. Closed structurally in `panel::supervise` itself (see its own
    // `PanelHandle` type's doc comment) rather than here, since nothing `main` could do to this
    // outer `JoinHandle` fixes an inner one it never sees.
    // Whether the panel is up *right now*, which is not the same question as `config.panel_enabled`
    // once the console's `panel` command can toggle it. A world switch is a real process restart
    // (see `relaunch_into`), so the replacement has to be told; without this the panel silently
    // disappears across a switch while `Worlds.svelte` is promising the page will reconnect.
    let panel_live =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(initial_panel.is_some()));
    let panel_supervisor = tokio::spawn(terrustia::panel::supervise(
        config.clone(),
        events_tx.clone(),
        panel_toggle_rx,
        initial_panel,
        panel_live.clone(),
    ));

    // Check-and-notify only, entirely in the background: never blocks startup, never downloads a
    // full binary, never applies anything. `update_notice` is set at most once, by
    // `update::boot_check`, and taken at most once — by the first recognised admin's login, in
    // `game::server`'s `note_finished_auth` — see that field's own doc comment on `GameServer`.
    let update_notice = std::sync::Arc::new(std::sync::Mutex::new(None));
    if config.update_check_enabled {
        tokio::spawn(terrustia::update::boot_check(update_notice.clone()));
    }

    // Also entirely background and non-fatal: a router with no UPnP, or none at all, logs a clear
    // fallback message and moves on — see `upnp.rs`'s own module doc for the full behaviour. Never
    // touches the panel's own bind, which stays loopback-only regardless of anything here.
    if config.upnp_enabled {
        tokio::spawn(terrustia::upnp::attempt(config.listen));
    }

    let game_server = GameServer::new(config.clone(), world)
        .with_panel_toggle(panel_toggle_tx)
        .with_update_notice(update_notice)
        .with_palette(palette);
    // Cloned out before `run` consumes `game_server` — see the field's own doc comment in
    // `game::server` for why a shared cell, read only after the task ends, is how a world switch
    // requested from the panel reaches this function at all.
    let world_switch = game_server.world_switch_handle();
    // `config` is about to be moved into the accept loop below; a relaunch only needs the address
    // back, and the console needs where to remember its own history, so that is all that is kept.
    let listen_addr = config.listen;
    // Sibling to the world's save path, the same way the admin store is (`world.admin.toml`) —
    // see `console::spawn`'s own doc comment. `None` when there is nowhere to save to at all,
    // which just means history stays in-memory only for this run, as it always has.
    let console_history_path = config
        .save_target()
        .map(|p| p.with_extension("console_history"));
    let mut game = tokio::spawn(game_server.run(events_rx));

    let accept = tokio::spawn(listener::run(listener, config, events_tx.clone(), recorder));

    // Whoever has the terminal already has the world file, so the console is not gated. Reading
    // stdin has to be its own task: a blocking read would otherwise hold up the accept loop, and a
    // closed stdin (a service with no terminal) simply ends the task rather than the server.
    let console = console::spawn(events_tx.clone(), args.headless, console_history_path);

    // Dropping the last sender is what tells the game task to stop. The handle is borrowed rather
    // than moved so it is still here afterwards to be waited on.
    // A crash and a clean stop used to be indistinguishable from out here, so a server that had
    // panicked still exited 0 and no supervisor restarted it.
    let mut crashed = false;
    // `ended = &mut game` already resolves `game`'s `JoinHandle` when the game task stops on its
    // own (a console `stop`, among other things) — awaiting it again below would poll a
    // `JoinHandle` a second time after it already completed, which panics. This flag is `true`
    // only when the signal branch fired instead, which is the one case where `game` is still
    // pending and genuinely needs waiting on.
    let still_running = tokio::select! {
        reason = stop_signal() => {
            info!(reason, "shutting down");
            true
        }
        ended = &mut game => {
            match ended {
                Ok(Stopped::Cleanly) => info!("game task ended"),
                Ok(Stopped::Panicked) => {
                    error!("the game loop stopped because something panicked");
                    crashed = true;
                }
                Err(e) if e.is_cancelled() => info!("game task cancelled"),
                Err(e) => {
                    error!(error = %e, "the game task died");
                    crashed = true;
                }
            }
            false
        }
    };

    accept.abort();
    console.abort();
    panel_supervisor.abort();
    drop(events_tx);
    // Wait for the game task to finish. It saves the world on its way out, and returning here
    // without waiting would drop the runtime mid-write — which is a shutdown that quietly loses
    // everything since the last autosave.
    if still_running {
        match game.await {
            Ok(Stopped::Panicked) => crashed = true,
            Ok(Stopped::Cleanly) => {}
            Err(e) if e.is_cancelled() => {}
            Err(e) => {
                error!(error = %e, "the game task did not shut down cleanly");
                crashed = true;
            }
        }
    }
    if crashed {
        // Non-zero, so `Restart=on-failure` and container restart policies actually fire.
        return Err("the server stopped because of a crash".into());
    }

    // The world has already been saved by the ordinary shutdown path above — a switch is just an
    // ordinary clean stop with a note left behind about what to serve next. See
    // `game::server::GameServer::pending_world_switch`'s doc comment for why this cannot be a
    // hot-swap of the in-memory `World` and has to be a real process restart instead.
    let requested = world_switch
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    if let Some(new_world) = requested {
        info!(world = %new_world.display(), "restarting into the requested world");
        return relaunch_into(
            &new_world,
            &args.config,
            listen_addr,
            panel_live.load(std::sync::atomic::Ordering::Relaxed),
        )
        .map_err(|e| format!("could not restart into {}: {e}", new_world.display()).into());
    }
    Ok(())
}

/// Replace this process with a fresh one pointed at `world`, keeping the config file and listen
/// address the operator already chose. Never returns on success: on Unix `exec` replaces the
/// process image outright, same PID, which is what lets a supervisor (systemd, a container
/// restart policy) see this as the same service continuing rather than one stopping and another
/// starting. There is no equivalent primitive on Windows, so there the best available shape is a
/// detached child followed by this process exiting — a real PID change a process-monitor keyed on
/// PID would need to notice, the same platform gap `main`'s own `ctrl_close`/`ctrl_shutdown`
/// handling already lives with.
///
/// `panel` carries the panel across the restart. It is one-way, matching the flag: `--panel` turns
/// the panel on and there is no `--no-panel` to turn it off again, so a panel the operator stopped
/// from the console comes back if the config file or the environment says it should be on. That
/// asymmetry is the flag surface's, not this function's, and the case that actually broke people
/// was the other one: switching worlds from the panel used to `exec` a replacement without the
/// flag, so the panel deleted itself and the page it was serving waited for a reconnect that could
/// never come.
/// The arguments the replacement process is started with, split out from the two platform bodies
/// below so there is one list rather than two that have to be kept in step, and so it can be
/// tested without a process actually being replaced by it.
fn relaunch_args(
    world: &Path,
    config_path: &Path,
    listen: std::net::SocketAddr,
    panel: bool,
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec![
        "--config".into(),
        config_path.into(),
        "--listen".into(),
        listen.to_string().into(),
        "--world".into(),
        world.into(),
    ];
    if panel {
        args.push("--panel".into());
    }
    args
}

#[cfg(unix)]
fn relaunch_into(
    world: &Path,
    config_path: &Path,
    listen: std::net::SocketAddr,
    panel: bool,
) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    let error = std::process::Command::new(exe)
        .args(relaunch_args(world, config_path, listen, panel))
        .exec();
    // `exec` only returns here on failure — a successful call replaces this process and never
    // reaches this line at all.
    Err(error)
}

#[cfg(not(unix))]
fn relaunch_into(
    world: &Path,
    config_path: &Path,
    listen: std::net::SocketAddr,
    panel: bool,
) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .args(relaunch_args(world, config_path, listen, panel))
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    /// Every long flag `Args::parse` matches on, read out of this file's own source at compile
    /// time. Reflection is not available, and a hand-kept second list would drift exactly the way
    /// the help text already did, so the parser's source is the authority.
    ///
    /// The arms are all string literals in a `match`, of the form `"--name" =>` or
    /// `"-x" | "--name" =>`, so scanning for a `"--` and taking to the closing quote finds them
    /// all. Two literals in this file are deliberately not parser arms: the catch-all's error
    /// message and the doc comments, neither of which is followed by a quote-then-`=>`, which is
    /// what the shape check below requires.
    fn flags_the_parser_accepts() -> Vec<String> {
        let source = include_str!("main.rs");
        // `rsplit_once`, not `split_once`: that signature appears twice in this file, once as the
        // real definition and once as this very string literal, and the literal comes first.
        // Splitting on the first occurrence anchored the scan inside this test module, where the
        // next `fn` is a few lines down, so it scanned nothing and found no flags at all. The
        // scanner-finds-something test below is what caught that, which is the whole reason it
        // is there.
        let (_, body) = source
            .rsplit_once("fn parse(args: impl Iterator<Item = String>)")
            .expect("Args::parse's signature; update this scanner if it is renamed");
        let body = body.split_once("\n    fn ").map_or(body, |(head, _)| head);
        let mut found = Vec::new();
        for (index, _) in body.match_indices("\"--") {
            let rest = &body[index + 1..];
            let Some(end) = rest.find('"') else { continue };
            let flag = &rest[..end];
            // Only a match arm counts: the literal has to be followed by `=>` or by another
            // alternative, past any whitespace. This is what keeps error-message text out.
            let after = rest[end + 1..].trim_start();
            if after.starts_with("=>") || after.starts_with('|') {
                found.push(flag.to_string());
            }
        }
        found.sort();
        found.dedup();
        found
    }

    /// The check that makes the class impossible rather than fixing one instance of it.
    #[test]
    fn every_flag_the_parser_accepts_is_documented_in_help() {
        let documented: Vec<&str> = usage_options().iter().map(|(name, _, _)| *name).collect();
        let mut missing = Vec::new();
        for flag in flags_the_parser_accepts() {
            if !documented.iter().any(|d| d.contains(&flag)) {
                missing.push(flag);
            }
        }
        assert!(
            missing.is_empty(),
            "these flags are accepted by Args::parse but absent from --help: {missing:?}. Add \
             them to usage_options, or the server keeps offering a flag it will not tell anyone \
             about."
        );
    }

    /// The scanner has to actually find things, or the test above passes by finding nothing and
    /// is worth less than no test at all. This is the guard against that failure mode, which this
    /// project has now hit in four separate checkers.
    #[test]
    fn the_flag_scanner_finds_the_parsers_real_arms() {
        let flags = flags_the_parser_accepts();
        assert!(
            flags.len() >= 10,
            "expected the parser to have at least ten long flags, scanner found {}: {flags:?}",
            flags.len()
        );
        for expected in ["--world", "--panel", "--headless", "--seed", "--setup"] {
            assert!(
                flags.iter().any(|f| f == expected),
                "the scanner missed {expected}, so it cannot be trusted to catch a missing one"
            );
        }
    }
}

#[cfg(test)]
mod relaunch_tests {
    use super::*;

    fn args(panel: bool) -> Vec<String> {
        relaunch_args(
            Path::new("/w/Next.wld"),
            Path::new("/etc/terrustia.toml"),
            "127.0.0.1:7777".parse().expect("a literal address"),
            panel,
        )
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
    }

    /// The bug this exists for: a world switch `exec`s a replacement, and without `--panel` the
    /// panel that asked for the switch deleted itself while its own page waited to reconnect.
    #[test]
    fn a_running_panel_survives_a_world_switch() {
        assert!(
            args(true).contains(&"--panel".to_string()),
            "the replacement process must be told to bring the panel back up"
        );
    }

    /// And the other way, or every switch would turn on a panel the operator never asked for.
    #[test]
    fn a_panel_that_is_not_running_is_not_started_by_a_switch() {
        assert!(!args(false).contains(&"--panel".to_string()));
    }

    #[test]
    fn the_world_config_and_listen_address_all_carry_over() {
        let a = args(false);
        for expected in [
            "--config",
            "/etc/terrustia.toml",
            "--listen",
            "127.0.0.1:7777",
            "--world",
            "/w/Next.wld",
        ] {
            assert!(a.contains(&expected.to_string()), "missing {expected}");
        }
    }
}

/// List the worlds the server keeps in its own `worlds/` directory.
///
/// Enough to pick one by name without opening a file manager: the size the header claims, and how
/// recently it was played. Reading each header is a few hundred bytes and worth it, since a list of
/// bare filenames does not tell you which of three saves is the one you want.
fn print_worlds(palette: Palette) {
    let p = palette;
    let dir = terrustia::worlds::worlds_dir();
    let worlds = terrustia::worlds::list();
    if worlds.is_empty() {
        println!("no worlds in {} yet", dir.display());
        return;
    }
    println!("{}\n", p.paint(term::sgr::DIM, &dir.display().to_string()));
    for path in worlds {
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        // The dimensions come from the header, which is cheap to read and the only way to tell a
        // small world from a large one without opening it in the game.
        let dims = match wld::load(&path) {
            Ok(w) => format!("{} x {}", w.width(), w.height()),
            Err(_) => "unreadable".to_string(),
        };
        println!(
            "  {} {}   {}",
            p.paint(term::sgr::BOLD, &format!("{name:<32}")),
            p.paint(term::sgr::DIM, &format!("{dims:>12}")),
            p.paint(term::sgr::DIM, &format!("{:>6} MB", size / 1_048_576)),
        );
    }
    println!("\nserve one with:  terrustia --world <name>");
}

/// Wait for whichever signal asks the server to stop, and say which it was.
///
/// A process manager sends `SIGTERM`, not `SIGINT`: systemd, Docker and Kubernetes all stop a
/// service that way. Handling only Ctrl-C means every managed shutdown kills the server outright
/// and the world is lost back to its last autosave.
async fn stop_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal as unix_signal};
        let mut term = match unix_signal(SignalKind::terminate()) {
            Ok(s) => s,
            // Without a handler, Ctrl-C alone is better than refusing to start.
            Err(e) => {
                warn!(error = %e, "cannot listen for SIGTERM; only Ctrl-C will stop cleanly");
                let _ = signal::ctrl_c().await;
                return "ctrl-c";
            }
        };
        tokio::select! {
            _ = signal::ctrl_c() => "ctrl-c",
            _ = term.recv() => "SIGTERM",
        }
    }
    // Windows has no signals. It has four separate console control events, and a service or a
    // container stop sends one of the ones that are *not* Ctrl-C — so listening for Ctrl-C alone
    // meant a managed shutdown skipped the save entirely and lost everything since the last
    // autosave. `ctrl_close` is the console window closing; `ctrl_shutdown` is the machine going
    // down; `ctrl_break` is Ctrl+Break, and is also what reaches a child spawned into its own
    // process group, which is the only way one process can ask another to stop gracefully here at
    // all (there is no `kill -TERM` to send). All are worth catching, and all give only a short
    // grace period, which is why the shutdown save has to already be quick.
    #[cfg(windows)]
    {
        use tokio::signal::windows;

        let mut brk = windows::ctrl_break().ok();
        let mut close = match windows::ctrl_close() {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "cannot listen for the close event; only Ctrl-C stops cleanly");
                let _ = signal::ctrl_c().await;
                return "ctrl-c";
            }
        };
        let mut shutdown = match windows::ctrl_shutdown() {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "cannot listen for the shutdown event");
                tokio::select! {
                    _ = signal::ctrl_c() => return "ctrl-c",
                    _ = close.recv() => return "console closing",
                }
            }
        };
        // Registering Ctrl+Break is allowed to fail without taking the other three down with it,
        // so an unavailable one parks on a future that never completes rather than resolving at
        // once and reporting a stop nobody asked for.
        let ctrl_break = async {
            match brk.as_mut() {
                Some(b) => {
                    b.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = signal::ctrl_c() => "ctrl-c",
            _ = close.recv() => "console closing",
            _ = shutdown.recv() => "system shutting down",
            () = ctrl_break => "ctrl-break",
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = signal::ctrl_c().await;
        "ctrl-c"
    }
}

struct Args {
    config: PathBuf,
    listen: Option<std::net::SocketAddr>,
    /// The raw text typed after `--seed` — a plain number, or one of vanilla's real secret-seed
    /// magic strings/numbers, or any other text. Kept as text (not pre-parsed to a number) because
    /// a magic string has to reach `worldgen::secret_seed::SecretSeeds::detect` unmodified; see
    /// `main`'s own use of it via `worldgen::generate_from_text`.
    seed: Option<String>,
    world: Option<PathBuf>,
    /// Generate a fresh world under this name, written into the server's own `worlds/` directory,
    /// following Terraria's own space-to-underscore filename convention.
    new_world: Option<String>,
    /// Where to write the world, for a generated one that has nowhere else to go.
    save: Option<PathBuf>,
    /// Where to record every byte of every connection, for checking against a real client.
    record: Option<PathBuf>,
    /// List the worlds in the server's own `worlds/` directory, and stop.
    list_worlds: bool,
    /// Always run the interactive setup wizard — see `setup.rs`.
    setup: bool,
    /// Run without the interactive sticky console: start and serve straight away, with only the
    /// plain line reader for input. For services and daemons that have no terminal to be sticky
    /// about, and for anyone who just wants it to autostart. `-h` stays `--help`, so this is
    /// `--headless` with no short form.
    headless: bool,
    /// Start the web panel, the same as setting `panel_enabled` in the config file or
    /// `TERRUSTIA_PANEL_ENABLED=1`.
    ///
    /// It exists because the boot card names the panel on every start and, until this, gave no way
    /// to act on it: the row said `off` and the only ways in were a config key and an environment
    /// variable, neither of which the card mentioned. Every other thing on that card has a flag.
    panel: bool,
    help: bool,
}

impl Args {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut parsed = Self {
            config: PathBuf::from("terrustia.toml"),
            listen: None,
            seed: None,
            world: None,
            new_world: None,
            save: None,
            record: None,
            list_worlds: false,
            setup: false,
            headless: false,
            panel: false,
            help: false,
        };
        let mut args = args.peekable();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => parsed.help = true,
                "-c" | "--config" => {
                    parsed.config = args.next().ok_or("--config needs a path")?.into();
                }
                "-l" | "--listen" => {
                    let value = args.next().ok_or("--listen needs an address")?;
                    parsed.listen = Some(
                        value
                            .parse()
                            .map_err(|_| format!("not a socket address: {value}"))?,
                    );
                }
                "-w" | "--world" => {
                    let given = args.next().ok_or("--world needs a name or a path")?;
                    parsed.world = Some(terrustia::worlds::resolve(&given));
                }
                "-n" | "--new" => {
                    parsed.new_world = Some(args.next().ok_or("--new needs a world name")?);
                }
                "--worlds" => parsed.list_worlds = true,
                "--setup" => parsed.setup = true,
                "--headless" => parsed.headless = true,
                "--panel" => parsed.panel = true,
                "--save" => {
                    parsed.save = Some(args.next().ok_or("--save needs a path")?.into());
                }
                "--record" => {
                    parsed.record = Some(args.next().ok_or("--record needs a path")?.into());
                }
                "-s" | "--seed" => {
                    // Any text is a valid seed, matching real vanilla's own free-text seed field:
                    // a plain number reproduces that number, anything else (including the seven
                    // secret-seed magic strings) is hashed into one instead — see
                    // `worldgen::secret_seed`'s own module doc. No longer validated as numeric
                    // here, deliberately: `--seed "get fixed boi"` used to be rejected as "not a
                    // number", which was correct for the old numbers-only contract and wrong for
                    // this one.
                    parsed.seed = Some(args.next().ok_or("--seed needs a value")?);
                }
                other => return Err(format!("unrecognised argument: {other}")),
            }
        }
        if parsed.world.is_some() && parsed.new_world.is_some() {
            return Err("--world and --new cannot both be given — pick one world to serve".into());
        }
        Ok(parsed)
    }
}

/// The version of the game this server speaks to.
///
/// Both releases, in fact: 1.4.5.7 and 1.4.5.8 differ on the wire only in the number they announce
/// and in four bytes at the end of packet 7, and refusing the older one would strand anybody who
/// has not updated for no reason at all. See `id::SUPPORTED_RELEASES`.
const GAME_VERSION: &str = "1.4.5.8";

/// Every option `--help` lists: the flag spelling, what it does, and its default (empty for none).
///
/// Split out from [`print_usage`] so a test can check it against the flags `Args::parse` actually
/// accepts. `--panel` was added to the parser and to the boot card, which told the operator to
/// "start it with --panel", while `--help` never mentioned it: the card named a flag the help did
/// not have. That is the same shape as the bug the card row was itself written to fix, so the
/// answer is a check rather than one more careful edit.
fn usage_options() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        ("-c, --config <PATH>", "Config file", "terrustia.toml"),
        ("-l, --listen <ADDR>", "Address to bind", "0.0.0.0:7777"),
        (
            "-w, --world <NAME|PATH>",
            "Serve an existing world, by name or by path",
            "",
        ),
        (
            "-n, --new <NAME>",
            "Generate a fresh world, saved into the server's worlds/ directory",
            "",
        ),
        (
            "    --worlds",
            "List the worlds in the server's worlds/ directory",
            "",
        ),
        (
            "    --save <PATH>",
            "Where to write the world; a loaded one saves back over itself",
            "",
        ),
        (
            "-s, --seed <TEXT>",
            "World generation seed — a number, or one of vanilla's seven secret seeds \
             (\"get fixed boi\" among them)",
            "random",
        ),
        (
            "    --record <PATH>",
            "Record every connection's bytes, for checking against a real client",
            "",
        ),
        (
            "    --setup",
            "Interactive first-run wizard: writes a terrustia.toml and starts",
            "",
        ),
        (
            "    --headless",
            "Start and serve without the interactive console (for services)",
            "",
        ),
        (
            "    --panel",
            "Start the web admin panel, on loopback only (see panel_listen)",
            "off",
        ),
        ("-h, --help", "Show this message", ""),
    ]
}

fn print_usage(palette: Palette) {
    let heading = |text: &str| palette.paint(term::sgr::BOLD, text);
    let flag = |text: &str| palette.paint(term::sgr::BRIGHT_CYAN, text);
    let note = |text: &str| palette.paint(term::sgr::DIM, text);
    let options = usage_options();

    println!(
        "{} {}\n",
        heading("terrustia"),
        note(&format!("an async Terraria {GAME_VERSION} server"))
    );
    println!(
        "{}\n    terrustia [OPTIONS]\n    terrustia update [--check]\n",
        heading("USAGE")
    );
    println!("{}", heading("OPTIONS"));
    for (name, what, default) in options {
        let tail = if default.is_empty() {
            String::new()
        } else {
            note(&format!(" [default: {default}]"))
        };
        println!("    {} {what}{tail}", flag(&format!("{name:<32}")));
    }
    println!("\n{}", heading("ENVIRONMENT"));
    println!(
        "    {} Log filter, e.g. debug or terrustia=debug",
        flag(&format!("{:<32}", "TERRUSTIA_LOG"))
    );
    println!(
        "    {} Turn colour off, or force it on through a pipe",
        flag(&format!("{:<32}", "NO_COLOR / CLICOLOR_FORCE"))
    );
    println!(
        "\n    {} config, no file needed — see terrustia.toml.example for every",
        note("TERRUSTIA_<KEY> overrides")
    );
    println!(
        "    key (TERRUSTIA_LISTEN, TERRUSTIA_MAX_PLAYERS, TERRUSTIA_WORLD_NAME, ...); a config \
         file"
    );
    println!("    still wins if given, and a CLI flag wins over both.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_when_no_arguments_are_given() {
        let args = Args::parse(std::iter::empty()).unwrap();
        assert_eq!(args.config, PathBuf::from("terrustia.toml"));
        assert!(args.listen.is_none());
        assert!(!args.help);
    }

    #[test]
    fn flags_are_parsed() {
        let args = Args::parse(
            ["--listen", "127.0.0.1:1234", "--seed", "9", "-c", "x.toml"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert_eq!(args.listen.unwrap().port(), 1234);
        assert_eq!(args.seed.as_deref(), Some("9"));
        assert_eq!(args.config, PathBuf::from("x.toml"));
    }

    /// `--seed` takes any text now, not just a number — matching real vanilla's own free-text
    /// seed field, and the actual trigger for its seven secret seeds. A non-numeric seed used to
    /// be rejected ("not a number"); it is ordinary, valid input now, exactly as much as `9` is.
    #[test]
    fn a_word_seed_is_accepted() {
        let args = Args::parse(["--seed", "get fixed boi"].into_iter().map(String::from)).unwrap();
        assert_eq!(args.seed.as_deref(), Some("get fixed boi"));
    }

    #[test]
    fn bad_input_is_reported_rather_than_ignored() {
        for bad in [
            vec!["--listen"],
            vec!["--listen", "not-an-address"],
            vec!["--nonsense"],
        ] {
            assert!(
                Args::parse(bad.iter().map(|s| s.to_string())).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn new_world_name_is_parsed() {
        let args = Args::parse(["--new", "My Fork World"].into_iter().map(String::from)).unwrap();
        assert_eq!(args.new_world.as_deref(), Some("My Fork World"));
        assert!(args.world.is_none());
    }

    #[test]
    fn new_and_world_together_are_rejected() {
        assert!(Args::parse(["--new", "A", "--world", "B"].into_iter().map(String::from)).is_err());
    }

    #[test]
    fn new_needs_a_name() {
        assert!(Args::parse(["--new"].into_iter().map(String::from)).is_err());
    }
}
