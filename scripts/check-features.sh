#!/usr/bin/env bash
# The feature sets ferry's crates are built in, each one clippy with -D warnings.
#
# The workspace gate builds `--workspace --all-features`, which unifies every feature into one
# build and so never sees the build a program makes on its own. Each set below is checked here,
# alone, so a feature that leaves a method unused when it is the only one on fails this script.
#
# Output: one line per build, `ok <set>` or `FAIL <set>`, with the first compiler errors of a
# failed build and the path of its log (the logs go to $CARGO_TARGET_DIR/check-features/, so
# nothing is written into the tree). The exit status is the verdict.
set -uo pipefail
cd "$(dirname "$0")/.."

# Each entry: "<set> :: <cargo arguments>". Every build is `cargo clippy <arguments>
# --lib --bins -- -D warnings`: the targets a program compiles. Test targets are left out on
# purpose: a test target's helpers are dead code in a default build.
BUILDS=(
  # The daemon alone with its default features: `-p` alone, so no other package's features are
  # unified in. Then with the Photos Picker, the one feature it has that is off by default.
  "syncd (default) :: -p syncd"
  "syncd (photos-picker) :: -p syncd --features photos-picker"
  # The libraries syncd runs its datasets with (their `testing` features are for tests only).
  "ferry-engine (default) :: -p ferry-engine"
  "ferry-pim-mirror (default) :: -p ferry-pim-mirror"
  # The bus interfaces, alone and with the Photos Picker.
  "ferry-bus (default) :: -p ferry-bus"
  "ferry-bus (photos-picker) :: -p ferry-bus --features photos-picker"
  # The helper and the libraries it is built from, alone.
  "ferry-mountd (default) :: -p ferry-mountd"
  "ferry-drive-fuse (default) :: -p ferry-drive-fuse"
)

logs="${CARGO_TARGET_DIR:-target}/check-features"
mkdir -p "$logs"
fail=0
n=0
for build in "${BUILDS[@]}"; do
  n=$((n + 1))
  set_name=${build%% :: *}
  read -r -a args <<<"${build#* :: }"
  log="$logs/$n.log"
  if cargo clippy "${args[@]}" --lib --bins -- -D warnings >"$log" 2>&1; then
    echo "ok $set_name"
  else
    echo "FAIL $set_name"
    echo "  log: $log"
    grep -m 5 -E '^(error|warning)' "$log" | sed 's/^/  /'
    fail=1
  fi
done
exit "$fail"
