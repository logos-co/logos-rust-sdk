#!/usr/bin/env bash
# logos-rust-sdk integration test — drives the provider/caller module pair
# through a headless logoscore daemon.
#
# Sourced by BOTH flake.nix (nix flake check / ws test) and tests/flake.nix
# (the CI job), so the two cannot drift: the assertions live here once and the
# flakes differ only in how they pin and build the modules.
#
# Environment:
#   MODULES_DIR — directory of installed modules to hand logoscore (-m)
#   out         — nix output dir; a summary is written to $out/result.txt
set -uo pipefail

: "${MODULES_DIR:?MODULES_DIR must be set}"
: "${out:?out must be set}"
mkdir -p "$out"
export QT_QPA_PLATFORM=offscreen

# Inline (`-c`) mode is legacy; drive a logoscore daemon and call via the
# `call` client subcommand. A persistent daemon keeps the Qt event loop running
# so the cross-module IPC reply is delivered reliably.
export LOGOSCORE_CONFIG_DIR="$(mktemp -d)"
DAEMON_PID=""
cleanup() {
  logoscore --config-dir "$LOGOSCORE_CONFIG_DIR" stop >/dev/null 2>&1 || true
  [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
  rm -rf "$LOGOSCORE_CONFIG_DIR"
}
trap cleanup EXIT

fail() {
  echo "IPC test FAILED: $*" >&2
  cat "$LOGOSCORE_CONFIG_DIR/daemon.log" >&2 || true
  exit 1
}

logoscore -D --config-dir "$LOGOSCORE_CONFIG_DIR" -m "$MODULES_DIR" \
  >"$LOGOSCORE_CONFIG_DIR/daemon.log" 2>&1 &
DAEMON_PID=$!
# `status` is the definitive readiness probe; no need to poke at the daemon's
# internal state file.
ready=0
for _i in $(seq 1 100); do
  if logoscore --config-dir "$LOGOSCORE_CONFIG_DIR" status >/dev/null 2>&1; then
    ready=1; break
  fi
  kill -0 "$DAEMON_PID" 2>/dev/null || break
  sleep 0.2
done
[ "$ready" = 1 ] || fail "logoscore daemon failed to start"

# load-module does not auto-resolve dependencies; load provider, then caller.
logoscore --config-dir "$LOGOSCORE_CONFIG_DIR" load-module sdk_test_provider_module
logoscore --config-dir "$LOGOSCORE_CONFIG_DIR" load-module sdk_test_caller_module

# Call a module method and print the raw JSON envelope; abort on a client error
# rather than letting an empty string flow into an assertion.
call_json() {
  local module="$1"; shift
  local out_json
  if ! out_json=$(logoscore --json --config-dir "$LOGOSCORE_CONFIG_DIR" \
        call "$module" "$@" 2>"$LOGOSCORE_CONFIG_DIR/call.err"); then
    cat "$LOGOSCORE_CONFIG_DIR/call.err" >&2 || true
    fail "logoscore call $module $* failed"
  fi
  printf '%s' "$out_json"
}

# The integer in the envelope's "result" field (handles a leading minus).
call_int() {
  local module="$1"; shift
  local envelope value
  envelope=$(call_json "$module" "$@")
  value=$(printf '%s' "$envelope" \
    | sed -n 's/.*"result"[[:space:]]*:[[:space:]]*\(-\{0,1\}[0-9][0-9]*\).*/\1/p')
  [ -n "$value" ] || fail "no integer result in: $envelope"
  printf '%s' "$value"
}

# Assert lo <= value <= hi, reporting the actual number.
in_range() {
  local what="$1" value="$2" lo="$3" hi="$4"
  if [ "$value" -lt "$lo" ] || [ "$value" -gt "$hi" ]; then
    fail "$what: expected ${lo}..${hi}, got ${value}"
  fi
  echo "  OK  $what = ${value} (expected ${lo}..${hi})"
}

# ─────────────────────────────────────────────────────── 1. plain IPC round trip
# Match the value exactly so "8" isn't satisfied by e.g. "80".
result=$(call_json sdk_test_caller_module call_add 5 3)
echo "call result: $result"
printf '%s' "$result" | grep -qE '"result"[[:space:]]*:[[:space:]]*8[[:space:]]*[,}]' \
  || fail "expected sdk_test_caller_module.call_add(5,3) == 8: $result"

# ──────────────────────────────────────────────── 2. binary event round trip
# The caller subscribed to the provider's blobReady(seq, payload: bstr) event in
# on_context_ready; ask the provider to emit a 4096-byte deterministic blob,
# then read back what the subscription actually received. This exercises the
# generated bstr-event emitter end-to-end — a payload that dropped or corrupted
# in codegen would surface here as size 0 or a wrong checksum. (The blob is byte
# i = (i*7+11)&0xff; the caller's checksum is sum(payload[i] * (i%31 + 1)) =
# 8354754 for 4096 bytes.)
emitted=$(call_json sdk_test_provider_module emit_blob 4096)
echo "emit_blob result: $emitted"
printf '%s' "$emitted" | grep -qE '"result"[[:space:]]*:[[:space:]]*4096[[:space:]]*[,}]' \
  || fail "expected emit_blob(4096) == 4096: $emitted"

# The event is delivered to the caller's listener thread off the Qt event loop,
# so poll briefly for it rather than assuming it landed.
size="" ; checksum=""
for _i in $(seq 1 50); do
  size=$(logoscore --json --config-dir "$LOGOSCORE_CONFIG_DIR" \
         call sdk_test_caller_module last_blob_size 2>/dev/null || true)
  printf '%s' "$size" | grep -qE '"result"[[:space:]]*:[[:space:]]*4096[[:space:]]*[,}]' && break
  sleep 0.2
done
checksum=$(call_json sdk_test_caller_module last_blob_checksum)
echo "last_blob_size: $size"
echo "last_blob_checksum: $checksum"
printf '%s' "$size" | grep -qE '"result"[[:space:]]*:[[:space:]]*4096[[:space:]]*[,}]' \
  || fail "binary event payload not received intact (expected size 4096): $size"
printf '%s' "$checksum" | grep -qE '"result"[[:space:]]*:[[:space:]]*8354754[[:space:]]*[,}]' \
  || fail "binary event payload corrupted (expected checksum 8354754): $checksum"

# ──────────────────────────────────────────────────── 3. per-call timeout
#
# The lp_* C ABI has always taken timeout_ms; the Rust surface hardcoded 0 (=
# "use the protocol default", currently 20s). These assertions exist because a
# timeout that is ACCEPTED AND IGNORED is worse than none — it reads as a
# guarantee. So the question is answered with a clock, not with a getter: the
# provider's sleep(ms) blocks for a time the caller KNOWS outlives its timeout.
#
# The timeout binds to the CALL, not to the client: the caller module holds ONE
# provider client for the whole process (its address is readable with
# provider_client_addr, and asserted constant below), and every measurement here
# goes through that one object. So each result can only have got its bound from
# the entry point that was called.
#
#   timed_call(sleep_ms, timeout_ms) -> elapsed ms, NEGATED if the call
#   actually completed. timeout_ms > 0 takes the generated
#   `sleep_with_timeout`; timeout_ms <= 0 takes the untouched `sleep`.
echo
echo "--- per-call timeout ---"

# Warm the path first: the very first call to the provider pays the capability
# handshake and the QtRO replica acquire, and that must not sit inside anybody's
# measured window.
warm=$(call_int sdk_test_caller_module timed_call 100 0)
echo "warm-up timed_call(sleep=100ms, no timeout) -> ${warm}ms (negative = completed)"
[ "$warm" -lt 0 ] || fail "warm-up call did not complete: ${warm}"

client_addr=$(call_int sdk_test_caller_module provider_client_addr)
[ "$client_addr" != "0" ] || fail "no provider client in the caller module"

# 3a. SYNC, bounded. A 6s call under an 800ms timeout must come back at roughly
#     800ms: not at 6000 (timeout ignored), not at 20000 (default), and not
#     never.
sync_bounded=$(call_int sdk_test_caller_module timed_call 6000 800)
echo "sync  timed_call(sleep=6000ms, timeout=800ms)  -> ${sync_bounded}ms"
[ "$sync_bounded" -gt 0 ] || fail "a 6s call COMPLETED under an 800ms timeout (elapsed ${sync_bounded}ms) — the timeout was ignored"
in_range "sync bounded elapsed" "$sync_bounded" 700 3000

sleep 7   # let the provider finish the sleep it is still serving

# 3b. SYNC, default path, through the SAME client. The same 6s call with no
#     timeout asked for must still complete — proof that the plumbing did not
#     start imposing a bound where none was requested, and that the bound from
#     3a did not stick to the client.
sync_default=$(call_int sdk_test_caller_module timed_call 6000 0)
echo "sync  timed_call(sleep=6000ms, no timeout)     -> ${sync_default}ms"
[ "$sync_default" -lt 0 ] || fail "a 6s call with NO timeout failed after ${sync_default}ms — the default path changed, or a previous call's timeout stuck to the client"
in_range "sync default-path elapsed" $(( -sync_default )) 5000 12000

# 3c. THE POINT OF THE PER-CALL FORM: two calls, one client, two different
#     bounds, back to back with no client rebuilt in between. A whole-client
#     timeout cannot express this — it would need a second client, or would
#     make the second bound overwrite the first. Here each call carries its own,
#     and the two land in disjoint bands.
#
#     (drain_ms is slept inside the module between them: the provider is
#     single-dispatch and is still serving call A's sleep after A gives up, so
#     without it B would measure the queue instead of its own bound.)
pair_started=$(call_int sdk_test_caller_module same_client_two_timeouts 4000 700 2600 4000)
[ "$pair_started" = "1" ] || fail "same_client_two_timeouts did not run: ${pair_started}"
pair_a=$(call_int sdk_test_caller_module last_pair_a_ms)
pair_b=$(call_int sdk_test_caller_module last_pair_b_ms)
echo "same-client pair (sleep=4000ms): A timeout=700ms -> ${pair_a}ms, B timeout=2600ms -> ${pair_b}ms"
[ "$pair_a" -gt 0 ] || fail "call A COMPLETED under a 700ms timeout (elapsed ${pair_a}ms)"
[ "$pair_b" -gt 0 ] || fail "call B COMPLETED under a 2600ms timeout (elapsed ${pair_b}ms)"
# Disjoint, and in the right order. Checked BEFORE the individual bands because
# this is the load-bearing claim: if the timeout bound to the CLIENT rather than
# the call, both calls would have come back at whichever bound won.
[ $(( pair_b - pair_a )) -gt 1200 ] || fail "two different timeouts on one client produced the same elapsed time (A=${pair_a}ms, B=${pair_b}ms) — the timeout is binding to the client, not the call"
echo "  OK  two calls on ONE client took their OWN timeouts (Δ = $(( pair_b - pair_a ))ms)"
in_range "same-client call A (700ms bound)"  "$pair_a" 500  1800
in_range "same-client call B (2600ms bound)" "$pair_b" 2200 3900

sleep 3   # the provider is still finishing call B's sleep

# 3d. ASYNC, bounded. Same proof on the other surface: the generated
#     sleep_async_with_timeout under an 800ms timeout must reach its callback at
#     ~800ms.
started=$(call_int sdk_test_caller_module start_timed_call_async 6000 800)
[ "$started" = "0" ] || fail "start_timed_call_async refused the timeout: ${started}"
ok=-1
for _i in $(seq 1 100); do
  ok=$(call_int sdk_test_caller_module last_async_ok)
  [ "$ok" != "-1" ] && break
  sleep 0.2
done
[ "$ok" != "-1" ] || fail "async call under an 800ms timeout never reached its callback"
async_bounded=$(call_int sdk_test_caller_module last_async_elapsed_ms)
echo "async start_timed_call_async(sleep=6000ms, timeout=800ms) -> ${async_bounded}ms, completed=${ok}"
[ "$ok" = "0" ] || fail "a 6s async call COMPLETED under an 800ms timeout (elapsed ${async_bounded}ms)"
in_range "async bounded elapsed" "$async_bounded" 700 3000

sleep 7   # drain the provider again

# 3e. ASYNC, a DIFFERENT bound on the same client. The async twin of 3c: the
#     previous call's 800ms must not have stuck to anything.
started=$(call_int sdk_test_caller_module start_timed_call_async 6000 3000)
[ "$started" = "0" ] || fail "start_timed_call_async refused the timeout: ${started}"
ok=-1
for _i in $(seq 1 100); do
  ok=$(call_int sdk_test_caller_module last_async_ok)
  [ "$ok" != "-1" ] && break
  sleep 0.2
done
[ "$ok" != "-1" ] || fail "async call under a 3000ms timeout never reached its callback"
async_bounded2=$(call_int sdk_test_caller_module last_async_elapsed_ms)
echo "async start_timed_call_async(sleep=6000ms, timeout=3000ms) -> ${async_bounded2}ms, completed=${ok}"
[ "$ok" = "0" ] || fail "a 6s async call COMPLETED under a 3000ms timeout (elapsed ${async_bounded2}ms)"
in_range "async second bound elapsed" "$async_bounded2" 2600 5000
[ $(( async_bounded2 - async_bounded )) -gt 1200 ] || fail "two different async timeouts on one client produced the same elapsed time (${async_bounded}ms vs ${async_bounded2}ms)"

sleep 5   # drain

# 3f. ASYNC, default path, against a call that outlives the DEFAULT. This is the
#     other half of "the default is unchanged": a 25s call with no timeout
#     asked for must fail at the protocol's own 20s, which is what timeout_ms=0
#     selects. (It has to be async: a 25s synchronous method would itself be cut
#     off by logoscore's own 20s call timeout, hiding the very thing measured.)
started=$(call_int sdk_test_caller_module start_timed_call_async 25000 0)
[ "$started" = "0" ] || fail "start_timed_call_async(25000, 0) did not start: ${started}"
ok=-1
for _i in $(seq 1 200); do
  ok=$(call_int sdk_test_caller_module last_async_ok)
  [ "$ok" != "-1" ] && break
  sleep 0.25
done
[ "$ok" != "-1" ] || fail "async call on the default path never reached its callback"
default_elapsed=$(call_int sdk_test_caller_module last_async_elapsed_ms)
echo "async start_timed_call_async(sleep=25000ms, no timeout) -> ${default_elapsed}ms, completed=${ok}"
[ "$ok" = "0" ] || fail "a 25s call completed on the default path — the default is not 20s"
in_range "default-path (20s) elapsed" "$default_elapsed" 18000 24000

sleep 6   # the provider is still finishing its 25s sleep

# 3g. A duration the ABI cannot express is REFUSED, not clamped. 500µs would
#     truncate to 0ms, which on this ABI MEANS "use the default" — so accepting
#     it would silently turn a sub-millisecond bound into 20 seconds.
refusal=$(call_json sdk_test_caller_module refused_timeout_reason 500)
echo "refused_timeout_reason(500us) -> $refusal"
printf '%s' "$refusal" | grep -q "DEFAULT" \
  || fail "a 500us timeout was not refused with a reason naming the default it would have become: $refusal"
echo "  OK  sub-millisecond timeout refused rather than rounded into the 20s default"

# 3h. Every measurement above went through the SAME client object. Checked, not
#     assumed: if the fixture had quietly rebuilt a client per call, "the
#     timeout binds to the call" would be untested.
client_addr_after=$(call_int sdk_test_caller_module provider_client_addr)
[ "$client_addr_after" = "$client_addr" ] \
  || fail "the caller rebuilt its provider client mid-run (${client_addr} -> ${client_addr_after}); the per-call assertions above prove nothing"
echo "  OK  all measurements went through one client (addr ${client_addr})"

# 3i. No bound leaked. After all of the above, an ordinary call with no timeout
#     still works exactly as it did in step 1.
after=$(call_json sdk_test_caller_module call_add 5 3)
printf '%s' "$after" | grep -qE '"result"[[:space:]]*:[[:space:]]*8[[:space:]]*[,}]' \
  || fail "call_add(5,3) broke after bounded calls — a timeout leaked: $after"
echo "  OK  default-path call still works after bounded calls"

{
  echo "IPC test passed: sdk_test_provider_module.add(5,3) returned 8 via IPC"
  echo "Binary event passed: blobReady payload received as 4096 bytes, checksum 8354754"
  echo "Per-call timeout passed:"
  echo "  sync  sleep 6000ms under an 800ms timeout failed after ${sync_bounded}ms"
  echo "  sync  sleep 6000ms with NO timeout still completed, after $(( -sync_default ))ms"
  echo "  one client, two bounds: 700ms -> ${pair_a}ms and 2600ms -> ${pair_b}ms"
  echo "  async sleep 6000ms under 800ms -> ${async_bounded}ms, under 3000ms -> ${async_bounded2}ms"
  echo "  default path (timeout_ms <= 0) still waits the protocol's 20s: ${default_elapsed}ms"
  echo "  sub-millisecond timeout refused, not rounded into the default"
} > "$out/result.txt"
cat "$out/result.txt"
