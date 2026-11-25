#!/usr/bin/env bash
# Run the test suite; on failure, also report the failing tests' output as a GitHub annotation
# so it shows on the PR's checks page without opening the log.
set -uo pipefail
log=$(mktemp)
cargo test --locked --all-targets --no-fail-fast 2>&1 | tee "$log"
status=${PIPESTATUS[0]}
if [ "$status" -ne 0 ]; then
  details=$(sed -e 's/\x1b\[[0-9;]*m//g' -e 's/\r$//' "$log" \
    | grep -E -A20 '^---- |^error(\[|:)' | head -150 \
    | sed -e 's/%/%25/g' | awk '{ printf "%s%%0A", $0 }')
  echo "::error title=cargo test failed::${details:-see the log}"
fi
exit "$status"
