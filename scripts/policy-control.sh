#!/usr/bin/env bash
# Positive control for the clippy bans: every sanctioned site must still
# warn when the lints are forced on, and every configured path must resolve.
set -euo pipefail
export LC_ALL=C   # identical sort order on macOS and Ubuntu
export CARGO_TERM_COLOR=never   # CI sets "always"; colour codes would break the parsing below
cd "$(dirname "$0")/.."
log=target/policy-control.log
expected=crates/policy/control-expected.txt
mkdir -p target
cargo clippy --workspace --locked --no-default-features --features stable --lib --bins -- \
  --force-warn clippy::disallowed_methods \
  --force-warn clippy::print_stdout \
  --force-warn clippy::print_stderr > "$log" 2>&1 || { cat "$log" >&2; exit 1; }
if grep -E 'does not refer to a reachable|expected a function, found' "$log"; then
  echo "policy-control: a clippy.toml entry resolves to nothing; fix the path" >&2
  exit 1
fi
actual=$(awk '
  /^warning: use of (a disallowed method|`e?print(ln)?!`)/ { msg = $0; sub(/^warning: /, "", msg); next }
  msg != "" && /^ *--> / { loc = $2; sub(/:[0-9]+:[0-9]+$/, "", loc); print loc "\t" msg; msg = "" }
' "$log" | sort | uniq -c | awk '{ n = $1; sub(/^ *[0-9]+ /, ""); print n "\t" $0 }')
if ! diff <(printf '%s\n' "$actual") "$expected"; then
  echo "policy-control: the sanctioned sites changed (left: actual, right: $expected)." >&2
  echo "If a site was added or removed on purpose, update $expected in the same change." >&2
  exit 1
fi
echo "policy-control: $(printf '%s\n' "$actual" | awk -F'\t' '{n += $1} END {print n}') sanctioned sites still fire"
