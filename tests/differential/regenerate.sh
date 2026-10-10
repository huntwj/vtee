#!/bin/sh
# Regenerates the oracle fixtures (tests/fixtures/differential/oracle_*.json) from cases.json by running
# every case through the avt and vt100 emulators. Needs Docker (or a local Rust toolchain with network
# access to fetch the two crates); the generator is a standalone crate, not part of vtee's build.
#
#   sh tests/differential/regenerate.sh
#
# After changing cases.json or the oracles, triage new disagreements:
#   VTEE_DIFF_REPORT=/tmp/report.json cargo test --test differential
# then add a verdict for each to tests/fixtures/differential/adjudications.json (see docs/development/testing.md).
set -e
cd "$(dirname "$0")/oracle"
if command -v docker >/dev/null 2>&1; then
  root="$(cd ../../.. && pwd)"
  MSYS_NO_PATHCONV=1 docker run --rm -v "$root:/work" -w /work/tests/differential/oracle \
    -e CARGO_TARGET_DIR=/tmp/target rust:slim cargo run --release
else
  cargo run --release
fi
