#!/usr/bin/env bash
#
# Execute the logos-rust-sdk doc-test(s) end-to-end and regenerate their Markdown.
#
# There is one spec:
#   rust-provider-module.test.yaml — writes a pure-Rust Logos module from scratch
#       (the `provider` half of logos-rust-example-module / this repo's tests/),
#       builds it into an .lgx, installs it with lgpm, and drives it through a
#       headless logoscore daemon, calling its Rust-backed methods over IPC.
#
# Unlike a module repo's own doc-test, this spec does NOT pin
# logos-rust-sdk to the commit under test: the provider module is pure Rust and
# does not consume the SDK (only a *caller* module would). The spec exercises the
# Rust-module build + IPC pipeline that the SDK is built on. To verify the SDK
# itself against the working tree, run the integration test instead:
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
set -euo pipefail

# Run from this doctests/ directory regardless of where the script is invoked from.
cd "$(dirname "$0")"

# The doctest CLI. Override by exporting DOCTEST (space-separated command).
read -r -a DOCTEST <<< "${DOCTEST:-nix run github:logos-co/logos-doctest --}"
OUTPUT_DIR="./outputs"

echo "==> Clearing previous ${OUTPUT_DIR}/"
# A prior run copies module artifacts out of the read-only nix store, so the
# directories land read-only (r-x) too. `rm -rf` can't delete files inside a
# directory it can't write to, so restore write permission first.
if [ -e "${OUTPUT_DIR}" ]; then
  chmod -R u+w "${OUTPUT_DIR}" 2>/dev/null || true
fi
rm -rf "${OUTPUT_DIR}"
mkdir -p "${OUTPUT_DIR}"

for spec in *.test.yaml; do
  name="$(basename "${spec%.test.yaml}")"
  echo "==> Running ${spec} into ${OUTPUT_DIR}/"
  "${DOCTEST[@]}" run "${spec}" \
    --verbose \
    --continue-on-fail \
    --output-dir "${OUTPUT_DIR}/"

  echo "==> Generating ${OUTPUT_DIR}/${name}.md"
  "${DOCTEST[@]}" generate "${spec}" \
    -o "${OUTPUT_DIR}/${name}.md"
done

# The spec writes the module source INTO outputs/ (like logos-tutorial), so the
# committed tree is the generated source (flake.nix, metadata.json, CMakeLists.txt,
# rust-lib/, .gitignore) plus the rendered .md — with all build artifacts stripped.
# `doctest clean`'s defaults cover .git/, modules/, *.so/*.dylib, flake.lock, and
# the out-links named lm/logos/pm/result*. Two of this spec's leftovers fall
# outside those defaults, so add them explicitly:
#   --also lgpm          the lgpm out-link (default knows `pm`, not `lgpm`)
#   --also provider-lgx  the .lgx out-link (non-standard name)
#   --also logs.txt      the daemon log (default glob is *.log, not logs.txt)
echo "==> Cleaning build artifacts from ${OUTPUT_DIR}/ (keeps generated source + .md)"
"${DOCTEST[@]}" clean "${OUTPUT_DIR}" \
  --also lgpm \
  --also provider-lgx \
  --also logs.txt \
  --verbose

echo "==> Done. Cleaned output (generated source + rendered docs) is in ${OUTPUT_DIR}/"
