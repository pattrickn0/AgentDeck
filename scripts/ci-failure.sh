#!/usr/bin/env bash
# Sums up the failure of a CI step in annotations (`::error::`). A job's log only downloads when signed in to
# GitHub; the annotations come from the public API (`check-runs/<job>/annotations`), so whoever has no token
# (an agent, the GitHub tab without a credential) still sees why the job went red.
# Usage: scripts/ci-failure.sh <log>
set -u
log="${1:?uso: ci-failure.sh <log>}"
[ -f "$log" ] || { echo "::error::no log ($log): the failure happened before the tests"; exit 0; }

# Without colors (CARGO_TERM_COLOR=always) and only the lines that say what broke.
relevant=$(sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g' "$log" \
  | sed -n -E '/panicked at/{N;N;N;p;d}; /^(error|warning)(\[|:)|^ *--> |^---- .* ----$|^test .* FAILED$|^ *(left|right):|^error: test failed/p' \
  | head -n 200)
[ -n "$relevant" ] || relevant=$(sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g' "$log" | tail -n 40)

# One annotation per block of 20 lines (GitHub keeps up to 10 per step); `%`, CR and LF escaped.
printf '%s\n' "$relevant" | split -l 20 - "${RUNNER_TEMP:-/tmp}/ci-failure-"
for part in "${RUNNER_TEMP:-/tmp}"/ci-failure-*; do
  msg=$(sed -e 's/%/%25/g' -e 's/\r/%0D/g' "$part" | sed -e ':a' -e 'N' -e '$!ba' -e 's/\n/%0A/g')
  echo "::error title=$(basename "$log")::$msg"
done
