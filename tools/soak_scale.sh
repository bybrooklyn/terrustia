#!/usr/bin/env bash
#
# A scale soak: put N headless clients on one world in a single join burst, hold for a while, and
# then check the things a person would notice. This is the 255-player qualification run in a form
# that can be repeated; tools/soak_ci.sh is the small three-player version that runs per commit.
#
# Every client here connects from 127.0.0.1, so the server's per-address connection cap (a real
# anti-DoS limit, default 8) would refuse all but a handful. Real players arrive from distinct
# addresses, so for this local test we lift the per-address cap to the player count. Nothing else
# about the server is loosened.
#
#   ./tools/soak_scale.sh [players] [seconds]
#
# Defaults: 255 players, a 1800s (30 minute) hold. Exits non-zero if the server panics, dies, or
# saves nothing, or if fewer than 90% of the clients get on.

set -uo pipefail

PLAYERS="${1:-255}"
HOLD="${2:-1800}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
BIN="$ROOT/target/release/terrustia"
SOAK="$ROOT/target/release/examples/soak"

# Set SOAK_KEEP=1 to leave the run directory behind. A run that is never looked at again is a run
# whose evidence is gone: the tick samples, the server log and the per-client logs all live in here,
# and once the trap has fired there is no way back to them.
KEEP="${SOAK_KEEP:-0}"
cleanup() {
  kill "${SRV:-}" "${SAMPLER:-}" 2>/dev/null
  if [ "$KEEP" = 1 ]; then
    echo "run directory kept: $WORK"
  else
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

[ -x "$BIN" ]  || { echo "build first: cargo build --release -p terrustia --bin terrustia"; exit 1; }
[ -x "$SOAK" ] || { echo "build first: cargo build --release -p terrustia-client --example soak"; exit 1; }

PORT=$((20000 + RANDOM % 20000))
echo "=== server: max_players=$PLAYERS, world 4200x1200, port $PORT ==="
# Lift the per-address cap (all clients share 127.0.0.1) and give max_connections a little headroom
# over the player count. Every other limit is left at its default.
# The tick module goes to debug so its per-window `tick window` line is emitted; that line is the
# only source of the server's own processor cost per tick, and the release bar is stated in terms of
# it. Everything else stays at info, so this does not turn the run into a wall of logging.
# Overridable so a diagnostic run can widen it (for example
# TERRUSTIA_LOG=info,terrustia::game::server=debug to see what each save actually copied)
# without editing this script.
TERRUSTIA_LOG="${TERRUSTIA_LOG:-info,terrustia::game::server::tick=debug}" \
  TERRUSTIA_MAX_PLAYERS="$PLAYERS" \
  TERRUSTIA_MAX_CONNECTIONS="$((PLAYERS + 16))" \
  TERRUSTIA_MAX_CONNECTIONS_PER_ADDRESS="$PLAYERS" \
  "$BIN" --headless --new soakscale --listen "127.0.0.1:$PORT" --save "$WORK/soak.wld" \
  > "$WORK/server.log" 2>&1 &
SRV=$!

# Wait for the server to be listening by its own log line, not a guessed sleep.
ready=0
for _ in $(seq 1 120); do
  grep -q "accepting connections" "$WORK/server.log" 2>/dev/null && { ready=1; break; }
  kill -0 "$SRV" 2>/dev/null || { echo "FAIL: server exited during startup"; tail -20 "$WORK/server.log"; exit 1; }
  sleep 1
done
[ "$ready" = 1 ] || { echo "FAIL: server never became ready"; tail -20 "$WORK/server.log"; exit 1; }

rss_kib() { ps -o rss= -p "$1" 2>/dev/null | tr -d ' '; }
MEM_START=$(rss_kib "$SRV")
echo "server ready; RSS at start: $((MEM_START/1024)) MiB"

echo "=== launching $PLAYERS clients in one burst for ${HOLD}s ==="
# One process holding every connection as a task, rather than one process per player. The fan-out
# version cost about 13.7 MiB per client process (measured: 24 of them, 328 MiB), nearly all of it
# per-process overhead, so a 255-player run needed roughly 3.5 GB of test rig before the server
# under test allocated anything. That is what killed two 255-player runs on 2026-09-06: the
# operating system's memory watchdog took the process group at 14:43 and 12:19 of a 30-minute hold,
# while the server itself was peaking at 562 and 296 MiB against a 1 GiB ceiling. The harness was
# the thing being measured. The client spreads its own players down the column, so the depth
# argument is unused here.
"$SOAK" "127.0.0.1:$PORT" "$HOLD" 0 soak "$PLAYERS" > "$WORK/clients.log" 2>&1 &
CPID=$!
echo "all $PLAYERS clients launched (one process, pid $CPID)"

# Sample the server's RSS every two minutes across the hold, so a leak shows as a rising curve
# rather than a single end number.
: > "$WORK/mem.log"
( t=0
  while kill -0 "$SRV" 2>/dev/null; do
    echo "t=$(printf '%5d' "$t")s  RSS=$(( $(rss_kib "$SRV")/1024 )) MiB  rig=$(( $(rss_kib "$CPID")/1024 )) MiB"
    sleep 120; t=$((t+120))
  done ) >> "$WORK/mem.log" 2>&1 &
SAMPLER=$!

wait "$CPID"
# The client process prints its own tally and prints it whether or not it exits zero, so read the
# line rather than inferring a count from one exit code covering every player.
ok=$(grep -oE 'held [0-9]+' "$WORK/clients.log" | grep -oE '[0-9]+' | tail -1)
ok="${ok:-0}"
kicked=$((PLAYERS - ok))
kill "$SAMPLER" 2>/dev/null
MEM_END=$(rss_kib "$SRV")

echo ""
echo "=== RESULTS ==="
echo "clients exited clean: $ok / $PLAYERS   (non-zero: $kicked)"
peak_slot=$(grep -oE 'slot=[0-9]+' "$WORK/server.log" | grep -oE '[0-9]+' | sort -n | tail -1)
echo "peak slot the server assigned: ${peak_slot:-none}"
echo "server RSS: start $((MEM_START/1024)) MiB -> end $(( ${MEM_END:-0}/1024 )) MiB"
echo "--- RSS curve over the hold (plateau vs leak) ---"; cat "$WORK/mem.log" 2>/dev/null || echo "(no samples)"
# "Stable memory" is one of the release bars, so judge it rather than printing a curve and hoping
# somebody reads it. That is exactly how a run climbing 120 MiB to 689 MiB over thirty minutes was
# recorded as a pass.
#
# A ceiling rather than a growth rate, because the curve under load rises and falls rather than
# climbing: a growth ratio fires on the shape and says nothing about the size. What matters for a
# release is how much the server actually holds 255 players within, so that is what this asserts.
#
# A thirty-minute run cannot separate a slow leak from burst working set, so this tests the ceiling
# and not leak freedom. Leak detection belongs to the extended pre-release soak.
MEM_CEILING_MIB=1024
mem_peak=""
if [ -s "$WORK/mem.log" ]; then
  # The curve samples every two minutes and the end figure is taken separately, so consider both:
  # a peak that falls between samples is missed either way, which is worth knowing when reading a
  # result that only just passes.
  mem_peak=$( { grep -oE 'RSS=[0-9]+' "$WORK/mem.log" | grep -oE '[0-9]+'
                echo $(( ${MEM_END:-0} / 1024 )); } | sort -n | tail -1)
fi
echo "--- server tick cost: its own cpu vs any external stall ---"
# Every `cpu_us` the server logged, in order. Each sample is already the *worst* tick of a ten-second
# window (the loop maxes into `worst_tick` and takes it), so a percentile here is a percentile of
# window maxima: a stricter reading than a true per-tick percentile, not a looser one. A 30-minute
# hold yields about 180 samples.
#
# The `tick window` filter is load-bearing and not decoration. That debug line fires once per window
# unconditionally, and the over-budget `warn!` and the stalled-window `info!` below it are
# *additional* lines for the same window carrying the same `cpu_us` value, not alternatives to it.
# Grepping the whole log for `cpu_us=` therefore counted every heavy or stalled window twice, and
# those duplicates land precisely at the tail where p95 and p99 are read: a 30-minute run showed 179
# `tick window` lines against 183 `cpu_us=` matches, the excess being exactly the four stall lines.
# One window, one sample. The warn/info lines stay as operator signal; they are not the sample
# source.
BUDGET_US=16667
grep 'tick window' "$WORK/server.log" | grep -oE 'cpu_us=[0-9]+' | grep -oE '[0-9]+' \
  | sort -n > "$WORK/cpu_us.txt"
tick_samples=$(wc -l < "$WORK/cpu_us.txt" | tr -d ' ')
tick_p99=""
if [ "${tick_samples:-0}" -gt 0 ]; then
  read -r tick_p99 tick_line < <(awk -v budget="$BUDGET_US" '
    { v[NR] = $1 }
    END {
      n = NR
      # Nearest-rank on an already-sorted list; clamp so a short run cannot index past the end.
      r50 = int((n + 1) / 2);        if (r50 < 1) r50 = 1; if (r50 > n) r50 = n
      r95 = int(0.95 * n + 0.9999);  if (r95 < 1) r95 = 1; if (r95 > n) r95 = n
      r99 = int(0.99 * n + 0.9999);  if (r99 < 1) r99 = 1; if (r99 > n) r99 = n
      # The leading value is the p99 on its own, for the caller to read into a variable; the rest
      # is the human line.
      printf "%d over %d samples (worst tick per 10s window): median %d us | p95 %d us | p99 %d us | max %d us  (budget %d us/tick)\n", \
        v[r99], n, v[r50], v[r95], v[r99], v[n], budget
    }' "$WORK/cpu_us.txt")
  echo "tick cost $tick_line"
else
  echo "NO tick-cost samples: the 'tick window' line is debug-level and did not reach the log."
  echo "  Without it the release bar (p99 tick under budget) cannot be judged. Check that the"
  echo "  server was started with terrustia::game::server::tick=debug in TERRUSTIA_LOG."
fi
grep -c "held off the processor" "$WORK/server.log" | xargs echo "external-stall warnings (test box, not the server):"

# Shut the server down and confirm it writes a world on the way out.
kill "$SRV" 2>/dev/null
for _ in $(seq 1 30); do kill -0 "$SRV" 2>/dev/null || break; sleep 1; done
saved_bytes=$(stat -f%z "$WORK/soak.wld" 2>/dev/null || stat -c%s "$WORK/soak.wld" 2>/dev/null || echo 0)

echo ""
echo "=== VERDICT ==="
fail=0
panics=$(grep -icE "panic|thread .* panicked" "$WORK/server.log")
[ "$panics" -eq 0 ] && echo "PASS  no panics" || { echo "FAIL  $panics panic line(s)"; fail=1; }
# Ninety percent, rounded up. Integer division truncates, which at 255 put the bar at 229 (89.8%)
# and passed a run that missed the stated bar by half a client.
required_ok=$(( (PLAYERS * 9 + 9) / 10 ))
# A client the server shed is not a client that held, and its own exit code cannot see that it was
# shed. `send_bytes` removes the player from the world the moment its outbound queue fills, but the
# socket only closes once `write_loop` has drained everything already queued behind that decision,
# which at `outbound_queue(255)` is about a million frames. So the client keeps reading a backlog
# for the rest of the hold, never sees EOF, never sees a write fail, and exits zero.
#
# That is not hypothetical and it is not rare. Two 255-player half-hours measured here shed 3 and 5
# clients respectively, with the queue at 1,052,626 and 1,052,669 of its 1,052,672 capacity; every
# one of the eight printed `done after 1800s` and exited zero, and both runs were recorded as
# `255/255 clients connected and held`. Counting only exit codes made this clause unable to fail
# for the one reason a real server sheds players under load.
#
# Subtracting is deliberately conservative: a client that was shed *and* noticed in time would be
# counted against twice, which can only make this bar stricter, never looser.
shed=$(grep -c "outbound queue full" "$WORK/server.log")
held=$(( ok - shed ))
[ "$held" -lt 0 ] && held=0
if [ "$held" -ge "$required_ok" ]; then
  echo "PASS  $held/$PLAYERS clients connected and held ($ok exited clean, $shed shed by the server)"
else
  echo "FAIL  only $held/$PLAYERS clients held, need $required_ok ($ok exited clean, $shed shed by the server)"
  fail=1
fi
if [ "$saved_bytes" -gt 0 ]; then echo "PASS  world saved on shutdown ($saved_bytes bytes)"; else echo "FAIL  no world written on shutdown"; fail=1; fi
# The release bar names p99 tick cost, so the run judges it rather than printing a number for a
# person to eyeball. No samples is a failure and not a pass: an absent measurement used to be
# indistinguishable from a good one, which is how "peak cpu_us 3889 us" came to be quoted as tick
# cost when it was only ever the cost of whichever tick happened to coincide with a machine stall.
if [ -n "$tick_p99" ]; then
  if [ "$tick_p99" -le "$BUDGET_US" ]; then
    echo "PASS  p99 tick ${tick_p99} us within the ${BUDGET_US} us budget"
  else
    echo "FAIL  p99 tick ${tick_p99} us over the ${BUDGET_US} us budget"; fail=1
  fi
else
  echo "FAIL  no tick-cost samples; the p99-under-budget bar could not be judged"; fail=1
fi
if [ -n "$mem_peak" ]; then
  if [ "$mem_peak" -le "$MEM_CEILING_MIB" ]; then
    echo "PASS  peak memory ${mem_peak} MiB within the ${MEM_CEILING_MIB} MiB ceiling"
  else
    echo "FAIL  peak memory ${mem_peak} MiB over the ${MEM_CEILING_MIB} MiB ceiling"; fail=1
  fi
else
  echo "note  no RSS samples, memory not judged"
fi
[ "$fail" -eq 0 ] && echo "=== soak PASSED ===" || echo "=== soak FAILED ==="
exit "$fail"
