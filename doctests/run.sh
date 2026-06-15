#!/usr/bin/env bash
#
# Execute the logos-rust-sdk doc-test(s) end-to-end and regenerate their Markdown.
#
# There is one spec:
#   cross-language-composition.test.yaml — writes three modules from scratch on
#       the builder-driven cdylib path (no build.rs): a Rust provider, a C++
#       provider, and a Rust consumer that ties them together. Builds each into
#       an .lgx, installs them with lgpm, and drives the consumer through a
#       headless logoscore daemon to exercise the whole Rust-SDK surface — module
#       context, sync + async typed calls, event subscription, and concrete +
#       interface dependencies.
#
# Unlike a module repo's own doc-test, this spec does NOT pin logos-rust-sdk to
# the commit under test: the tutorial module consumes the SDK + lidl-gen as
# cargo git dependencies at a pinned rev (the published authoring surface, like
# any real module repo would). To verify the SDK against the working tree, run
# the integration test instead:
#   nix build 'path:../tests#checks.x86_64-linux.ipc-test' --override-input logos-rust-sdk path:..
#
# The runner is the shared `doctest` CLI
# (https://github.com/logos-co/logos-doctest), invoked directly via its flake.
# Each spec runs into ./outputs/ via --output-dir; `doctest generate` renders the
# .md; `doctest clean` then strips build artifacts, keeping only the .md.
#
# To run against a local logos-doctest checkout instead of the published flake,
# set DOCTEST, e.g.:  DOCTEST="nix run path:../../logos-doctest --" ./run.sh
#
# The build needs a filesystem that both lets git create loose objects and is not
# mounted noexec (it git-adds a generated module and dlopen()s the .so it builds).
# Normally that's ./outputs and the build runs in place. In sandboxes where the
# checkout can't host it — e.g. a `fakeowner` bind mount that rejects git's 0444
# object temp files, or a noexec mount that fails dlopen — the script auto-stages
# the build in $XDG_RUNTIME_DIR / $HOME / /var/tmp and copies the cleaned result
# back into ./outputs. Force a specific staging dir with DOCTEST_BUILD_DIR.
#
set -euo pipefail

# Run from this doctests/ directory regardless of where the script is invoked from.
cd "$(dirname "$0")"

# The doctest CLI. Override by exporting DOCTEST (space-separated command).
read -r -a DOCTEST <<< "${DOCTEST:-nix run github:logos-co/logos-doctest --}"
OUTPUT_DIR="./outputs"

# ── Pick a filesystem that can actually host the build ───────────────────────
# The spec writes a module from scratch, `git init && git add`s it, builds it
# into a loadable .so, and dlopen()s it through a logoscore daemon. That demands
# a filesystem that BOTH (a) lets git create loose objects and (b) is not mounted
# noexec. Some sandboxes break one or the other: a `fakeowner` bind mount (Docker
# Desktop / OrbStack, as used for /workspace) rejects git's read-only (0444)
# object temp files with "insufficient permission for adding an object", and a
# noexec mount (often /tmp) makes dlopen fail with "failed to map segment from
# shared object". On a normal CI filesystem ./outputs passes both checks and the
# build runs in place, exactly as before. Otherwise we stage the build in the
# first directory that passes and copy the cleaned result back into ./outputs.
#
# Force a specific staging directory by exporting DOCTEST_BUILD_DIR.
probe_dir() {
  # $1: directory to test. Succeeds only if git-object creation AND exec both work.
  local d="$1" t ok=1
  t="$(mktemp -d "${d%/}/.doctest-probe.XXXXXX" 2>/dev/null)" || return 1
  ( cd "$t" && echo probe > f && git init -q . && git add f ) >/dev/null 2>&1 || ok=0
  if [ "$ok" = 1 ]; then
    # Exec test: create a file here and execve it directly. This trips on a
    # noexec mount exactly as dlopen()ing the built .so would. We write our own
    # script instead of `cp`ing a system binary so the test doesn't assume a
    # path like /bin/true (absent on macOS) and false-negative a usable dir.
    { printf '#!/bin/sh\nexit 0\n' > "$t/x" && chmod +x "$t/x" && "$t/x"; } >/dev/null 2>&1 || ok=0
  fi
  chmod -R u+w "$t" 2>/dev/null || true
  rm -rf "$t"
  [ "$ok" = 1 ]
}

STAGING=""   # non-empty when BUILD_DIR is a throwaway dir to copy back from
BUILD_DIR="${DOCTEST_BUILD_DIR:-}"
if [ -n "$BUILD_DIR" ]; then
  mkdir -p "$BUILD_DIR"
elif probe_dir "."; then
  BUILD_DIR="$OUTPUT_DIR"
else
  echo "==> ./ can't host the build (fakeowner and/or noexec); staging elsewhere" >&2
  for base in "${XDG_RUNTIME_DIR:-}" "$HOME" /var/tmp; do
    [ -n "$base" ] && [ -d "$base" ] || continue
    if probe_dir "$base"; then
      BUILD_DIR="$(mktemp -d "${base%/}/logos-rust-sdk-doctest.XXXXXX")"
      STAGING="$BUILD_DIR"
      break
    fi
  done
  if [ -z "$BUILD_DIR" ]; then
    # No staging candidate passed either. Rather than refuse to run, fall back to
    # building in ./outputs anyway — the probe can false-negative (e.g. a quirky
    # exec check), and an in-place build is what we'd do normally. If the host
    # truly can't host the build, the spec's own steps will report the real error.
    echo "==> No staging dir passed the probe; building in ${OUTPUT_DIR} (set DOCTEST_BUILD_DIR to override)" >&2
    BUILD_DIR="$OUTPUT_DIR"
  else
    echo "==> Staging build in ${BUILD_DIR}" >&2
  fi
fi

cleanup_staging() {
  if [ -n "$STAGING" ] && [ -e "$STAGING" ]; then
    chmod -R u+w "$STAGING" 2>/dev/null || true
    rm -rf "$STAGING"
  fi
}
trap cleanup_staging EXIT

echo "==> Clearing previous ${OUTPUT_DIR}/"
# A prior run copies module artifacts out of the read-only nix store, so the
# directories land read-only (r-x) too. `rm -rf` can't delete files inside a
# directory it can't write to, so restore write permission first.
for d in "${OUTPUT_DIR}" "${BUILD_DIR}"; do
  if [ -e "${d}" ]; then
    chmod -R u+w "${d}" 2>/dev/null || true
  fi
done
rm -rf "${OUTPUT_DIR}"
mkdir -p "${OUTPUT_DIR}"
# When staging, BUILD_DIR is a fresh mktemp dir; clear any leftover contents so
# the spec starts from a clean tree just like ./outputs would.
if [ "${BUILD_DIR}" != "${OUTPUT_DIR}" ]; then
  rm -rf "${BUILD_DIR:?}/"* "${BUILD_DIR:?}/".[!.]* 2>/dev/null || true
fi

# Each spec gets ITS OWN subdirectory: the specs write module sources, git
# repos, modules/ install dirs and a default-config logoscore daemon state —
# sharing one dir makes the second spec trip over the first one's leftovers.
for spec in *.test.yaml; do
  name="$(basename "${spec%.test.yaml}")"
  mkdir -p "${BUILD_DIR}/${name}"
  echo "==> Running ${spec} into ${BUILD_DIR}/${name}/"
  "${DOCTEST[@]}" run "${spec}" \
    --verbose \
    --continue-on-fail \
    --output-dir "${BUILD_DIR}/${name}/"

  echo "==> Generating ${BUILD_DIR}/${name}/${name}.md"
  "${DOCTEST[@]}" generate "${spec}" \
    -o "${BUILD_DIR}/${name}/${name}.md"
done

# The spec writes the module source INTO outputs/ (like logos-tutorial), so the
# committed tree is the generated source (flake.nix, metadata.json, CMakeLists.txt,
# rust-lib/, .gitignore) plus the rendered .md — with all build artifacts stripped.
# `doctest clean`'s defaults cover .git/, modules/, *.so/*.dylib, flake.lock, and
# the out-links named lm/logos/pm/result*. Several of this spec's leftovers fall
# outside those defaults, so add them explicitly:
#   --also lgpm          the lgpm out-link (default knows `pm`, not `lgpm`)
#   --also calc-lgx      the rust_calc_module .lgx out-link
#   --also greeter-lgx   the cpp_greeter_module .lgx out-link
#   --also orch-lgx      the rust_orchestrator_module .lgx out-link
#   --also logs.txt      the daemon log (default glob is *.log, not logs.txt)
# Module dirs are copied out of the read-only nix store, so they land read-only
# (r-x); restore write permission first or `clean` can't delete inside them.
chmod -R u+w "${BUILD_DIR}" 2>/dev/null || true

echo "==> Cleaning build artifacts from ${BUILD_DIR}/ (keeps generated source + .md)"
"${DOCTEST[@]}" clean "${BUILD_DIR}" \
  --also lgpm \
  --also calc-lgx \
  --also greeter-lgx \
  --also orch-lgx \
  --also logs.txt \
  --verbose

# If we staged the build off-tree, copy the cleaned result back into ./outputs.
# Use -R (not -a): the destination is the checkout, which may be a `fakeowner`
# mount that rejects chmod/chown, so preserving source perms/owner would fail.
# After clean, only normal user-writable source files and the .md remain.
if [ "${BUILD_DIR}" != "${OUTPUT_DIR}" ]; then
  echo "==> Copying cleaned output from ${BUILD_DIR}/ back to ${OUTPUT_DIR}/"
  cp -R "${BUILD_DIR}/." "${OUTPUT_DIR}/"
fi

echo "==> Done. Cleaned output (generated source + rendered docs) is in ${OUTPUT_DIR}/"
