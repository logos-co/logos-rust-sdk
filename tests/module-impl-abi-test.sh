#!/usr/bin/env bash
# logos-rust-sdk module-impl C ABI check — assert the Rust provider scaffold
# DEFINES every module-impl export logos-protocol DECLARES, in every
# configuration the emitter can produce.
#
# Why this exists. logos-protocol only DECLARES the module-impl C ABI
# (cpp/logos_module_impl.h); each language backend GENERATES the definitions.
# Those are independent facts and the gap has shipped from this repo twice —
# logos_module_grant_host_services at protocol 0.3, the teardown pair at 0.5.
# Both times a Rust module linked clean and then died at dlopen() on Linux
# under nixpkgs' -Wl,-z,now eager binding, while macOS (-undefined
# dynamic_lookup) never bound the symbol and reported a perfectly green build.
#
# The declared list and the protocol version both come from ONE logos-protocol
# build output (packages.module-impl-abi), so there is no version arithmetic
# here and nothing to keep in sync: the header is itself versioned, so "what
# this protocol requires" IS "what this header declares".
#
# Environment:
#   LIDL_GEN     — the logos-lidl-gen binary under test
#   ABI_MANIFEST — logos-protocol's module-impl-abi output (exports.txt,
#                  version, bin/logos-module-impl-diff)
#   CONTRACT     — a .lidl contract to generate the scaffold from
#   out          — nix output dir; a summary is written to $out/result.txt
set -uo pipefail

: "${LIDL_GEN:?LIDL_GEN must be set}"
: "${ABI_MANIFEST:?ABI_MANIFEST must be set}"
: "${CONTRACT:?CONTRACT must be set}"
: "${out:?out must be set}"
mkdir -p "$out"

fail() {
  echo "module-impl ABI check FAILED: $*" >&2
  exit 1
}

declared="$ABI_MANIFEST/exports.txt"
diff_exports="$ABI_MANIFEST/bin/logos-module-impl-diff"
[ -r "$declared" ]     || fail "no exports.txt in $ABI_MANIFEST"
[ -x "$diff_exports" ] || fail "no logos-module-impl-diff in $ABI_MANIFEST"

# ───────────────────────────────────────────── the version, and why it is checked
#
# ANTI-VACUITY, and the sharpest trap in this repo. Unlike the C++ backend there
# is no preprocessor: which exports exist is decided at CODEGEN time by parsing
# this string (grant_host_services_block gates on >= 0.3, teardown_block on
# >= 0.5). Both parse with `.parse().ok().unwrap_or(0)`, so an unparseable
# version FAILS OPEN to (0,0) — below every gate — and lidl-gen then exits 0
# having emitted only the seven founding exports. Omitting the flag entirely
# defaults to "0.1.0" with the same result.
#
# So a typo here would not fail the check; it would silently move it to protocol
# 0.0 and then compare seven-against-seven... except the DECLARED side would
# still be the real ten, so it would go red for the wrong reason and send the
# reader hunting a codegen bug that does not exist. Validate the string where
# the failure can say so.
version=$(cat "$ABI_MANIFEST/version")
case "$version" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) fail "protocol version '$version' from $ABI_MANIFEST/version is not X.Y.Z;
         lidl-gen would parse it as (0,0), emit the pre-0.3 export set, and exit 0" ;;
esac

# ─────────────────────────────────────────────────── the four configurations
#
# The emitter has two independent switches and BOTH change the scaffold, so a
# gap can hide in three of the four combinations while the default looks fine.
# Nothing else enforces that the four agree on the export set.
#
# Labels are carried into the failure output: a reader who sees this go red
# needs to know WHICH configuration broke, since the fix is usually in a block
# that only one of them reaches.
configs=(
  "default-trait,single|"
  "default-trait,multi|--concurrency multi"
  "no-trait,single|--no-trait"
  "no-trait,multi|--no-trait --concurrency multi"
)

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

generated=()
labels=()

for spec in "${configs[@]}"; do
  label="${spec%%|*}"
  flags="${spec#*|}"
  slug=$(printf '%s' "$label" | tr ',' '_')
  scaffold="$workdir/$slug.rs"

  # The .lidl is POSITIONAL args[1] and must precede the flags.
  # shellcheck disable=SC2086  # $flags is a deliberate word-split flag list
  "$LIDL_GEN" "$CONTRACT" --provider $flags --protocol-version "$version" \
      -o "$scaffold" >/dev/null \
    || fail "[$label] logos-lidl-gen exited non-zero"
  [ -s "$scaffold" ] || fail "[$label] logos-lidl-gen produced an empty scaffold"

  generated+=("$scaffold")
  labels+=("$label")
done

# ───────────────────────────────────────── ANTI-VACUITY: the four must differ
#
# lidl-gen matches its flags with `args.iter().any(|a| a == "--no-trait")` and
# IGNORES anything it does not recognise, exiting 0. So a mistyped or mis-quoted
# flag above does not fail — it silently regenerates the DEFAULT configuration
# under another label, and the check then reports four green configurations
# having examined one. That is not hypothetical: the first draft of this loop
# passed $flags through a variable that word-split wrongly, and three of the
# four "configurations" were byte-identical to the default.
#
# The four scaffolds genuinely differ (trait emission and the Arc/&self
# concurrency shape both alter the file), so pairwise distinctness is a cheap,
# exact witness that the flags reached the generator.
for i in "${!generated[@]}"; do
  for j in "${!generated[@]}"; do
    [ "$i" -lt "$j" ] || continue
    if cmp -s "${generated[$i]}" "${generated[$j]}"; then
      fail "[${labels[$i]}] and [${labels[$j]}] generated byte-identical scaffolds;
         the flags did not reach lidl-gen (it ignores unknown flags and exits 0),
         so these are not two configurations and the check below would be vacuous"
    fi
  done
done

# ────────────────────────────────────────────────────────── extract and diff
#
# Anchor on the DEFINITION form, `pub extern "C" fn logos_module_<name>`, and
# nothing looser. Two distinct traps:
#
#   * The scaffold also emits `extern "Rust" { pub(super) fn
#     logos_module_install(); }` — the author-supplied install hook, which is
#     NOT a member of this ABI.
#   * More dangerous: a looser anchor would count a DECLARATION as a
#     definition. An `extern "C" { fn logos_module_about_to_unload(); }` import
#     is exactly the shape of the bug this file exists to catch, and matching
#     it would turn a missing definition into a green check.
#
# Every configuration is diffed even after one fails. A gap is usually in a
# block that some configurations reach and others do not, so "which of the four
# broke" is the first thing worth knowing — and short-circuiting on the first
# would hide it behind a re-run per configuration.
broken=()
for i in "${!generated[@]}"; do
  label="${labels[$i]}"
  defined="$workdir/defined-$i.txt"

  # Comments are stripped first. A doc comment mentioning the anchor would
  # INFLATE the defined set, and inflation is the direction that hides a missing
  # definition — the one thing this check exists to catch. No generated comment
  # carries it today; this makes that a property rather than a coincidence.
  grep -vE '^[[:space:]]*(//|/\*|\*)' "${generated[$i]}" \
    | grep -oE 'pub extern "C" fn logos_module_[a-z0-9_]+' \
    | sed 's/.*fn //' | sort -u > "$defined"

  # logos-module-impl-diff refuses an empty file on either side rather than
  # reporting "nothing missing", so a collapsed extraction fails loudly here.
  if "$diff_exports" "$declared" "$defined" \
       "logos-rust-sdk provider scaffold ($label), protocol $version" \
       "lidl-gen/src/rustgen_provider.rs — see grant_host_services_block / teardown_block"
  then
    echo "  OK  $label: $(wc -l < "$defined" | tr -d ' ') exports defined"
  else
    broken+=("$label")
  fi
done

[ ${#broken[@]} -eq 0 ] || fail "incomplete module-impl C ABI in ${#broken[@]} of ${#configs[@]} configuration(s): ${broken[*]}"

{
  echo "module-impl C ABI check passed at protocol $version"
  echo "Declared by logos-protocol ($(wc -l < "$declared" | tr -d ' ') exports):"
  sed 's/^/  /' "$declared"
  echo "Defined by the Rust provider scaffold in all $(( ${#configs[@]} )) configurations:"
  for label in "${labels[@]}"; do echo "  $label"; done
} > "$out/result.txt"
cat "$out/result.txt"
