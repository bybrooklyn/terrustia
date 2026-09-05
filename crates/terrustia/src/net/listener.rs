use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{net::TcpListener, sync::mpsc};
use tracing::{debug, error, info, warn};

use crate::{config::Config, game::ServerEvent, net::connection};

/// Bind a listening socket, turning the kernel's terse refusal into a message that says what to do
/// about it. A raw `Os { code: 28 }` in a server log tells an operator nothing; the same failure
/// carrying "the OS is out of socket resources, raise the open-file limit" tells them where to look.
/// Used for both the game port and the web panel's own bind, so neither surfaces a bare errno.
///
/// `how_to_change` names the knob that moves *this* address, and the caller has to supply it
/// because this function cannot know which of the two it is being used for. It used to hard-code
/// `--listen` in the address-in-use advice, which is the game port: a panel that failed to bind
/// told the operator to change the wrong thing, and `--listen` would have moved the game and left
/// the panel exactly where it was.
pub async fn bind(addr: SocketAddr, how_to_change: &str) -> std::io::Result<TcpListener> {
    TcpListener::bind(addr)
        .await
        .map_err(|e| explain_bind_failure(addr, how_to_change, e))
}

/// Rewrite a bind failure into an explanation, keeping the original [`std::io::ErrorKind`] so a
/// caller that matches on the kind still can.
fn explain_bind_failure(
    addr: SocketAddr,
    how_to_change: &str,
    e: std::io::Error,
) -> std::io::Error {
    use std::io::ErrorKind;
    let advice = match e.kind() {
        ErrorKind::AddrInUse => format!(
            "{addr} is already in use; another server is probably bound there. Stop it, or choose a \
             different address with {how_to_change}."
        ),
        ErrorKind::PermissionDenied => format!(
            "not permitted to bind {addr}; ports below 1024 need elevated privileges. Pick a port \
             at or above 1024, or grant the privilege to bind a low one."
        ),
        ErrorKind::AddrNotAvailable => format!(
            "{addr} is not an address this machine holds; bind 0.0.0.0 to listen on every \
             interface, or use an address the host actually has."
        ),
        // ENOSPC (error 28) on a bind is not the disk: the kernel has no socket or port resources
        // left to hand out, which on a busy machine means too many sockets are already open. Matched
        // on `raw_os_error` rather than an `ErrorKind`, since std does not give this one a stable
        // kind on every platform.
        _ if e.raw_os_error() == Some(28) => format!(
            "the operating system has no socket resources left to bind {addr} (error 28, \"no space \
             left on device\"): too many sockets and ports are already open across the machine, not \
             a full disk. Close other servers, or raise the open-file limit (for example \
             `ulimit -n`), then try again."
        ),
        _ => format!("could not bind {addr}: {e}"),
    };
    std::io::Error::new(e.kind(), advice)
}

/// How many sockets are open, and from where.
///
/// The accept loop used to be unconditional: every socket that connected immediately got two
/// tasks, a sixteen-kilobyte read buffer and an outbound queue, none of which required the other
/// end to have spoken a word of the protocol. Opening sockets is cheap and there was no ceiling,
/// so one machine could exhaust this one's memory and file descriptors without ever handshaking.
///
/// A count rather than a semaphore, because the per-address limit needs the breakdown anyway, and
/// a few hundred entries is nothing to walk.
#[derive(Default)]
struct OpenConnections {
    per_address: HashMap<IpAddr, usize>,
    total: usize,
}

/// Held for as long as a connection is open; releases its place when dropped.
///
/// A guard rather than a pair of calls, so a connection task that panics or returns early still
/// gives its slot back. Leaking these is exactly the failure the limit exists to prevent.
pub struct ConnectionSlot {
    open: Arc<Mutex<OpenConnections>>,
    address: IpAddr,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        let mut open = match self.open.lock() {
            Ok(open) => open,
            // A poisoned lock means another task panicked while holding it. Losing the count is
            // worse than the panic, so take it anyway.
            Err(poisoned) => poisoned.into_inner(),
        };
        open.total = open.total.saturating_sub(1);
        if let Some(count) = open.per_address.get_mut(&self.address) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.per_address.remove(&self.address);
            }
        }
    }
}

/// Take a place, or say why not.
fn claim(
    open: &Arc<Mutex<OpenConnections>>,
    address: IpAddr,
    max_total: usize,
    max_per_address: usize,
) -> Result<ConnectionSlot, &'static str> {
    let mut guard = match open.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.total >= max_total {
        return Err("the server is full of connections");
    }
    let count = guard.per_address.entry(address).or_insert(0);
    if *count >= max_per_address {
        return Err("too many connections from one address");
    }
    *count += 1;
    guard.total += 1;
    Ok(ConnectionSlot {
        open: Arc::clone(open),
        address,
    })
}

/// How long to wait after the *second* consecutive `accept()` failure, doubling from there.
const ACCEPT_BACKOFF_START_MS: u64 = 5;
/// The ceiling on that wait. Half a second is long enough that a sticky failure costs the machine
/// two syscalls a second instead of as many as a core can issue, and short enough that the listener
/// is back in service within one wait of the condition clearing.
const ACCEPT_BACKOFF_MAX_MS: u64 = 500;

/// How long to pause before retrying `accept()` after `consecutive_failures` failures in a row.
///
/// `None` for the first one, deliberately. The ordinary failure is a single transient refusal - a
/// client that vanished between the SYN and the accept, a momentary descriptor shortage - and it is
/// gone by the next call. Sleeping for it would add latency to the case that fixes itself, which is
/// the case that happens.
///
/// The second failure in a row is different: it says the condition is not transient. Descriptor
/// exhaustion (`EMFILE`/`ENFILE`) is the one that matters, because the loop's own response to it
/// used to be to try again immediately, and `accept()` on an exhausted process returns immediately
/// too. That is a hot loop: one core pinned, a log line per iteration, and the machine given no
/// slack to close whatever would free a descriptor. A short doubling wait turns it into a quiet
/// retry that gets out of the way of its own recovery.
///
/// A pure function of the count so the schedule can be pinned by a test without a socket: the loop
/// below only has to reset the count on success, which is the one thing the shape of it makes hard
/// to get wrong.
fn accept_backoff(consecutive_failures: u32) -> Option<Duration> {
    // 0 and 1 wait not at all; 2 is the first that waits, and waits the starting amount.
    let doublings = consecutive_failures.checked_sub(2)?;
    let mut millis = ACCEPT_BACKOFF_START_MS;
    // Capped at each step rather than shifted and clamped, so there is no width to overflow. 16 is
    // far past the point the cap is reached; the `min` only exists to bound the loop.
    for _ in 0..doublings.min(16) {
        millis = (millis * 2).min(ACCEPT_BACKOFF_MAX_MS);
    }
    Some(Duration::from_millis(millis))
}

/// What a repeating `accept()` failure usually means, said once rather than never.
///
/// The per-failure line is the raw error, which for the common case (`EMFILE`, "too many open
/// files") names a limit without saying whose or what to do about it. This is the same courtesy
/// [`explain_bind_failure`] extends to a refused bind.
fn explain_sticky_accept(e: &std::io::Error) -> &'static str {
    match e.raw_os_error() {
        // EMFILE (24) is this process's own descriptor limit; ENFILE (23) is the machine's.
        Some(24) => {
            "this process has no file descriptors left. Raise its limit (`ulimit -n`, or \
             LimitNOFILE= in a systemd unit), or lower max_connections so the server refuses \
             sockets before the kernel does."
        }
        Some(23) => {
            "the machine has no file descriptors left, across every process. Something else on \
             this host is probably leaking them."
        }
        // ENOBUFS (macOS 55 / Linux 105) and ENOMEM (12) both mean the kernel could not find room
        // for the new socket.
        Some(12) | Some(55) | Some(105) => {
            "the kernel has no memory or buffer space left for another socket. The machine is \
             under real pressure; lowering max_connections will reduce this server's share of it."
        }
        _ => {
            "the listening socket keeps refusing connections. Until it stops, this server is \
             retrying on a backoff rather than spinning."
        }
    }
}

/// Accept connections until the listener fails or the game task goes away.
pub async fn run(
    listener: TcpListener,
    config: Config,
    events: mpsc::Sender<ServerEvent>,
    recorder: Option<crate::net::record::Recorder>,
) {
    let limits = connection::Limits {
        idle: Duration::from_secs(config.idle_timeout_secs),
        handshake: Duration::from_secs(config.handshake_timeout_secs),
        outbound_queue: connection::outbound_queue(config.max_players),
        // One counter for the whole listener, cloned into every connection it serves. This is the
        // handle that makes the sum bounded rather than each queue bounded separately.
        queued_total: std::sync::Arc::default(),
    };
    let open = Arc::new(Mutex::new(OpenConnections::default()));
    match listener.local_addr() {
        Ok(addr) => info!(%addr, "accepting connections"),
        Err(e) => debug!(error = %e, "listener has no local address"),
    }

    // Reset on every accepted connection, so a one-off failure between two healthy ones never
    // builds towards a wait.
    let mut consecutive_failures: u32 = 0;

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                consecutive_failures = 0;
                let slot = match claim(
                    &open,
                    addr.ip(),
                    config.max_connections,
                    config.max_connections_per_address,
                ) {
                    Ok(slot) => slot,
                    Err(why) => {
                        // Dropped without ceremony. Anything else — a message, a graceful close —
                        // is work this server does on behalf of whoever is flooding it.
                        warn!(%addr, why, "refusing a connection");
                        drop(stream);
                        continue;
                    }
                };
                debug!(%addr, "connection accepted");
                let events = events.clone();
                tokio::spawn(connection::serve(
                    stream,
                    addr,
                    events,
                    limits.clone(),
                    recorder.clone(),
                    slot,
                ));
            }
            Err(e) => {
                // A per-connection failure (out of descriptors, client vanished mid-handshake) must
                // not take the whole listener down.
                error!(error = %e, "accept failed");
                if events.is_closed() {
                    return;
                }
                // Only a *repeating* failure is worth waiting for, and only the first repeat is
                // worth a second line: past that the wait itself is what keeps the log quiet.
                if let Some(delay) = accept_backoff(consecutive_failures.saturating_add(1)) {
                    if consecutive_failures == 1 {
                        warn!(
                            advice = explain_sticky_accept(&e),
                            "accept has failed twice in a row; backing off between retries"
                        );
                    }
                    tokio::time::sleep(delay).await;
                }
                consecutive_failures = consecutive_failures.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(last: u8) -> IpAddr {
        IpAddr::from([127, 0, 0, last])
    }

    #[test]
    fn the_server_stops_accepting_once_it_is_full() {
        let open = Arc::new(Mutex::new(OpenConnections::default()));
        let held: Vec<_> = (0..4)
            .map(|i| claim(&open, addr(i), 4, 4).expect("within both limits"))
            .collect();
        assert!(
            claim(&open, addr(9), 4, 4).is_err(),
            "the total is a ceiling, or one machine can open sockets until this one runs out"
        );
        drop(held);
        assert!(claim(&open, addr(9), 4, 4).is_ok(), "space frees up again");
    }

    #[test]
    fn one_address_cannot_take_every_place() {
        let open = Arc::new(Mutex::new(OpenConnections::default()));
        let _held: Vec<_> = (0..2)
            .map(|_| claim(&open, addr(1), 100, 2).expect("within the per-address limit"))
            .collect();
        assert!(
            claim(&open, addr(1), 100, 2).is_err(),
            "one address is capped even when the server has room"
        );
        assert!(
            claim(&open, addr(2), 100, 2).is_ok(),
            "and everyone else is unaffected by it"
        );
    }

    /// The guard has to release on every path, including a task that unwinds.
    #[test]
    fn a_slot_is_released_even_if_its_task_panics() {
        let open = Arc::new(Mutex::new(OpenConnections::default()));
        let taken = std::panic::catch_unwind({
            let open = Arc::clone(&open);
            move || {
                let _slot = claim(&open, addr(1), 1, 1).expect("the only place");
                panic!("a connection task falling over");
            }
        });
        assert!(taken.is_err(), "the panic should have happened");
        assert!(
            claim(&open, addr(1), 1, 1).is_ok(),
            "a panicking connection must not leak its place, or the server fills up with ghosts"
        );
    }

    /// A bind failure should name the address and say what to do, not surface a bare errno. The
    /// ENOSPC case is the one that prompted this: `os error 28` on a port bind is socket-resource
    /// exhaustion, and the message has to say so rather than let an operator chase a full disk.
    #[test]
    fn a_bind_failure_explains_itself() {
        use std::io::{Error, ErrorKind};
        let a: SocketAddr = "0.0.0.0:7777".parse().unwrap();

        let in_use = explain_bind_failure(a, "--listen", Error::from(ErrorKind::AddrInUse));
        assert_eq!(in_use.kind(), ErrorKind::AddrInUse, "kind is preserved");
        assert!(in_use.to_string().contains("7777") && in_use.to_string().contains("in use"));

        let denied = explain_bind_failure(a, "--listen", Error::from(ErrorKind::PermissionDenied));
        assert!(denied.to_string().contains("privilege"));

        let unavailable =
            explain_bind_failure(a, "--listen", Error::from(ErrorKind::AddrNotAvailable));
        assert!(unavailable.to_string().contains("0.0.0.0"));

        let no_space = explain_bind_failure(a, "--listen", Error::from_raw_os_error(28));
        let msg = no_space.to_string();
        assert!(
            msg.contains("error 28") && msg.contains("socket") && msg.contains("not a full disk"),
            "ENOSPC must be explained as socket exhaustion, got: {msg}"
        );

        let other = explain_bind_failure(a, "--listen", Error::from(ErrorKind::ConnectionReset));
        assert!(other.to_string().contains("could not bind"));
    }

    /// The backoff schedule, pinned as a pure function.
    ///
    /// The bug it exists for: `run`'s error arm used to `continue` with no delay at all, so a
    /// *sticky* `accept()` failure - descriptor exhaustion, which returns immediately every time -
    /// became a hot loop that pinned a core and wrote a log line per iteration, while giving the
    /// machine no slack to close whatever would free a descriptor. Deleting the `sleep` in `run`
    /// does not fail any test on its own, because a real fd exhaustion is not something a unit test
    /// can arrange; what is testable, and what actually decides the behaviour, is the schedule.
    #[test]
    fn the_accept_backoff_spares_the_one_off_failure_and_caps_the_sticky_one() {
        assert_eq!(
            accept_backoff(0),
            None,
            "no failures, no wait - this is the ordinary path"
        );
        assert_eq!(
            accept_backoff(1),
            None,
            "a single failure is transient and must cost no latency at all"
        );

        // From the second failure on: doubling, from the starting wait.
        let schedule: Vec<u64> = (2..=12)
            .map(|n| {
                accept_backoff(n)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            schedule,
            vec![5, 10, 20, 40, 80, 160, 320, 500, 500, 500, 500],
            "the schedule should double from {ACCEPT_BACKOFF_START_MS}ms and stop at \
             {ACCEPT_BACKOFF_MAX_MS}ms"
        );

        // Monotonic, and never past the cap, however long the condition lasts. `u32::MAX` is the
        // saturating counter's own ceiling, so it is a value `run` really can reach.
        let mut previous = Duration::ZERO;
        for n in 2..=64 {
            let delay = accept_backoff(n).expect("a repeat failure always waits");
            assert!(delay >= previous, "the wait must never shrink");
            assert!(
                delay <= Duration::from_millis(ACCEPT_BACKOFF_MAX_MS),
                "the wait must never exceed the cap"
            );
            previous = delay;
        }
        assert_eq!(
            accept_backoff(u32::MAX),
            Some(Duration::from_millis(ACCEPT_BACKOFF_MAX_MS)),
            "a failure count that has saturated must still be the cap, not an overflow"
        );
    }

    /// A repeating accept failure names the limit that is actually exhausted.
    #[test]
    fn a_sticky_accept_failure_says_which_limit_ran_out() {
        use std::io::Error;
        assert!(explain_sticky_accept(&Error::from_raw_os_error(24)).contains("ulimit -n"));
        assert!(
            explain_sticky_accept(&Error::from_raw_os_error(23)).contains("across every process")
        );
        assert!(explain_sticky_accept(&Error::from_raw_os_error(12)).contains("buffer space"));
        assert!(
            explain_sticky_accept(&Error::from(std::io::ErrorKind::ConnectionAborted))
                .contains("backoff")
        );
    }
}
