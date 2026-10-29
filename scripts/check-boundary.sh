#!/usr/bin/env bash
# Crate boundaries, mechanically enforced (README.md says what each crate is; this is the
# mechanical form of who may depend on what).
#
# `cargo tree -i <dep>` exits 101 when the dependency is absent, which is precisely the state
# we want. Checking the exit status would therefore fail whenever the boundary holds, so we
# check for OUTPUT instead: any line naming the dependency is a leak.
#
# ferry reaches accounts through porter's crates, taken by git rev. The pure crates below (the
# sync contract, the replicas, the drive, the mount helper) reach no bus, no service and no secret
# store of porter's; only syncd, the bus crate and the test tooling name those, and syncd names no
# account crate at all (see ACCOUNTS ONLY THROUGH PORTER-CLIENT below).
set -uo pipefail
cd "$(dirname "$0")/.."

# RULES: what a crate reaches through ANY path (transitive, default features). The pure crates
# never reach a bus, a runtime, an HTTP client or a keyring. ferry-drive and ferry-drive-fuse reach
# tokio (a drive fetches in the background) but never a bus or a client. ferry-mountd, the mount
# helper outside syncd's sandbox, reaches no effect crate and none of porter's or ferry's own:
# std, rustix and listenfd only (no network, keyring or account data). ferry-bus is the interface
# definitions: zbus and tokio (through porter-dbus), nothing that shows or fetches.
EFFECTS="zbus zvariant tokio reqwest hyper ureq oo7 keyring secret-service interprocess latchkey ds-settings"
NO_BUS="zbus zvariant reqwest hyper ureq oo7 keyring secret-service interprocess latchkey ds-settings"
PORTER_HIDDEN="porter-dbus porter-callers porter-service porter-service-core porter-service-accounts porter-secrets porter-store porter-sheets porter-client porter-client-dbus porter-client-inproc"
# The engine judges who sees a dataset by porter-callers' `Caller` (which brings zbus along), and the
# PIM mirror asks accountd through porter-client; neither names the bus transport, the account
# service, a secret store or a sheet.
LIBRARY_HIDDEN="reqwest hyper ureq oo7 keyring secret-service interprocess latchkey ds-settings porter-dbus porter-service porter-service-core porter-service-accounts porter-secrets porter-store porter-sheets porter-client-dbus porter-client-inproc"
RULES=(
  "quickxorhash: $EFFECTS"
  "ferry-sync: $EFFECTS $PORTER_HIDDEN"
  "ferry-vdir: $EFFECTS $PORTER_HIDDEN"
  "ferry-pim-feed: $EFFECTS $PORTER_HIDDEN"
  "ferry-engine: $LIBRARY_HIDDEN porter-client"
  "ferry-pim-mirror: $LIBRARY_HIDDEN"
  "storage-webdav: $EFFECTS $PORTER_HIDDEN"
  "storage-graph: $EFFECTS $PORTER_HIDDEN"
  "storage-gdrive: $EFFECTS $PORTER_HIDDEN"
  "ferry-drive: $NO_BUS $PORTER_HIDDEN"
  "ferry-drive-fuse: $NO_BUS $PORTER_HIDDEN"
  "ferry-mountd: $EFFECTS fuser porter-ids porter-core porter-fs porter-http porter-secrets porter-service porter-client ferry-sync ferry-drive ferry-drive-fuse"
  "ferry-bus: reqwest hyper ureq oo7 keyring secret-service ds-settings"
)
fail=0

for rule in "${RULES[@]}"; do
  crate="${rule%%:*}"
  read -r -a forbidden <<<"${rule#*:}"
  # A crate that cargo cannot find would make every check below pass vacuously.
  if ! cargo tree -p "$crate" --depth 0 >/dev/null 2>&1; then
    echo "ERROR: cargo tree cannot resolve $crate; the boundary was not checked"
    fail=1
    continue
  fi
  leaked=0
  for dep in "${forbidden[@]}"; do
    if cargo tree -p "$crate" -i "$dep" -e normal,build 2>/dev/null | grep -q .; then
      echo "LEAK: $crate depends on $dep"
      cargo tree -p "$crate" -i "$dep" -e normal,build 2>/dev/null | head -20
      leaked=1
      fail=1
    fi
  done
  if [ "$leaked" -eq 0 ]; then
    echo "boundary holds: $crate reaches none of ${forbidden[*]}"
  fi
done

# ONE CLIENT: reqwest is named by no crate of this workspace. A program that sends over a direct
# connection takes porter-http-reqwest's `ReqwestHttp`, and the sync crates send through the
# `Http` seam they are handed.
named=$(grep -lE '^reqwest[ .=]' crates/*/Cargo.toml || true)
if [ -n "$named" ]; then
  echo "LEAK: reqwest is named in this workspace: $named"
  fail=1
else
  echo "boundary holds: no crate here names reqwest"
fi

# DELETED HELPERS: hand-rolled code that a crate or one shared function replaced must not come
# back. Each rule names the code and where its one home is (a path prefix that is allowed to hold
# it, if any); the allowlist is otherwise empty, and every crate is covered.
ratchet() {
  local what="$1" pattern="$2" home="${3:-}" hits
  hits=$(grep -rniE --include='*.rs' -e "$pattern" crates | grep -v "^${home:-/nonexistent}" || true)
  if [ -n "$hits" ]; then
    echo "$hits"
    echo "LEAK: $what is back; use ${4:-the crate that replaced it}"
    fail=1
  else
    echo "deleted: $what is not in any crate"
  fi
}
ratchet "a hand-rolled percent encoder or decoder" 'fn percent_(en|de)code\b' "" "porter_ids::percent"
ratchet "hand-rolled civil-date arithmetic" '(days_from_civil|civil_from_days|719_468)' "" "chrono"
ratchet "a hand-written MD5 (its first round constant)" '0xd76aa478' "" "md-5"
ratchet "a hand-written scratch directory type" '^[[:space:]]*(pub(\([a-z]+\))? )?struct Scratch\b' "" "test_kit::Scratch"

# ACCOUNTS ONLY THROUGH PORTER-CLIENT: accounts, grants, tokens and relays are accountd's. No crate
# of this workspace has porter's account stack as a normal or build dependency (the service, the
# secret stores, the registry store, the sheets, the provider families and the in-process transport
# are what accountd is built from); a program here reaches accountd through porter-client over
# porter-client-dbus. A test may host the stack in process: dev dependencies, and the test tooling
# (ferry-rig, ferry-fake-graph, syncd-fakes, which nothing ships), are outside the rule.
ACCOUNT_STACK="porter-service porter-service-core porter-service-accounts porter-service-grants porter-secrets porter-store porter-sheets porter-families porter-family-oauth porter-provider porter-client-inproc porter-identities porter-settings"
TOOLING="ferry-rig ferry-fake-graph syncd-fakes"
stack_leaks=""
for manifest in crates/*/Cargo.toml; do
  crate=$(basename "$(dirname "$manifest")")
  case " $TOOLING " in *" $crate "*) continue ;; esac
  reached=$(cargo tree -p "$crate" -e normal,build --prefix none 2>/dev/null | awk '{print $1}' | sort -u)
  for dep in $ACCOUNT_STACK; do
    if grep -qx "$dep" <<<"$reached"; then
      stack_leaks="$stack_leaks$crate reaches $dep; "
    fi
  done
done
if [ -n "$stack_leaks" ]; then
  echo "LEAK: a crate here builds on porter's account stack, not on porter-client: $stack_leaks"
  fail=1
else
  echo "boundary holds: accounts are reached only through porter-client"
fi

# The allowed edges between our own crates (ferry's, and porter's by git): each crate's DIRECT
# normal and build dependencies (all features), and nothing else. A dependency not listed is a
# leak; so is one the crate no longer has, so the table stays exact. Dev dependencies are outside
# it.
EDGES=(
  "quickxorhash:"
  "ferry-sync: porter-capability porter-ids porter-http"
  "ferry-vdir: porter-core porter-fs"
  "ferry-pim-feed: ferry-sync ferry-vdir porter-capability porter-core porter-http porter-ids"
  "ferry-engine: ferry-sync porter-callers porter-capability porter-core porter-ids"
  "ferry-pim-mirror: dav-proto ferry-engine ferry-pim-feed ferry-sync ferry-vdir porter-capability porter-client porter-core porter-http porter-http-relay porter-ids porter-match storage-webdav"
  "storage-webdav: dav-proto porter-capability porter-ids porter-http ferry-sync"
  "storage-graph: porter-capability porter-ids porter-http ferry-sync"
  "storage-gdrive: porter-capability porter-ids porter-http ferry-sync"
  "ferry-drive: porter-capability porter-ids porter-fs ferry-sync quickxorhash"
  "ferry-drive-fuse: ferry-drive ferry-mountd"
  "ferry-mountd:"
  "ferry-bus: porter-dbus porter-dbus-macros porter-ids"
  "ferry-fake-graph: porter-core porter-fake porter-fake-servers porter-ids porter-provider quickxorhash"
  "ferry-rig: ferry-bus ferry-fake-graph porter-dbus porter-fake-servers porter-rig"
  "syncd-fakes: ferry-bus porter-dbus"
  "syncd: ds-settings ferry-engine ferry-pim-feed ferry-pim-mirror ferry-vdir porter-client porter-client-dbus porter-core porter-match porter-capability porter-wire porter-ids porter-daemon porter-callers porter-dbus porter-fs porter-http porter-http-relay ferry-bus ferry-drive ferry-drive-fuse ferry-sync storage-gdrive storage-graph storage-webdav"
)
for edge in "${EDGES[@]}"; do
  crate="${edge%%:*}"
  read -r -a allowed <<<"${edge#*:}"
  found=$(cargo tree -p "$crate" --depth 1 -e normal,build --prefix none --all-features 2>/dev/null \
    | grep -E '\((/|https://github.com/PoHsuanLai/(porter|quire))' | awk '{print $1}' | grep -vx "$crate" | sort -u | tr '\n' ' ')
  want=$(printf '%s\n' "${allowed[@]}" | grep . | sort -u | tr '\n' ' ')
  if [ "$found" != "$want" ]; then
    echo "EDGE: $crate depends on [${found% }], the table allows [${want% }]"
    fail=1
  else
    echo "edges hold: $crate depends on [${found% }]"
  fi
done

# Every workspace member has a row above, so a new crate cannot slip in unchecked.
for member in $(sed -n 's#^  "crates/\(.*\)",$#\1#p' Cargo.toml); do
  printf '%s\n' "${EDGES[@]}" | grep -q "^$member:" || { echo "ERROR: $member has no row in EDGES"; fail=1; }
done

# syncd's library reads the environment in no function but `from_env` (the daemon's
# `Config::from_env` and the directory helpers it calls), so a program that runs the daemon
# inside itself gives it a `Config` and nothing else. The binary (main.rs) and the test-only files
# are outside the rule; `std::env::temp_dir` is a scratch name in tests, not a read of a variable.
# The awk remembers the last `fn` it saw, which is the function the next line is in.
env_reads=$(find crates/syncd/src -name '*.rs' \
  ! -name main.rs ! -name tests.rs ! -name testing.rs ! -name testkit.rs ! -path '*/tests/*' -print0 \
  | xargs -0 awk '
    FNR == 1 { current = "" }
    /^[[:space:]]*(pub(\([a-z]+\))? )?(async )?(unsafe )?fn [A-Za-z_0-9]+/ {
      match($0, /fn [A-Za-z_0-9]+/)
      current = substr($0, RSTART + 3, RLENGTH - 3)
    }
    /env::(var|var_os|vars|vars_os|args|args_os|current_dir|home_dir)\>/ \
      && current != "from_env" && $0 !~ /^[[:space:]]*\/\// {
      print FILENAME ":" FNR ": " $0
    }')
if [ -n "$env_reads" ]; then
  echo "$env_reads"
  echo "LEAK: syncd's library reads the environment outside a from_env function"
  fail=1
else
  echo "pure: syncd reads the environment only in from_env functions"
fi

# syncd's test-only caller knob (`SYNCD_PROC_ROOT`, feature `test-proc-root`) reads callers from a
# directory: it may not be on in a default build, nor in syncd's whole normal dependency tree.
if cargo tree -p syncd -e normal --prefix none -f '{p} {f}' 2>/dev/null | grep -q "test-proc-root"; then
  echo "LEAK: syncd's default build enables test-proc-root"
  fail=1
else
  echo "test-only: syncd's default build does not include test-proc-root"
fi
# porter-callers' test-only caller fixtures (feature `test-proc-root`, which turns on peephole's
# `fake`) are never in syncd's normal dependency tree.
if cargo tree -p syncd -e normal --prefix none -f '{p} {f}' 2>/dev/null | grep -E '^(porter-callers|peephole) ' | grep -Eq 'test-proc-root|fake'; then
  echo "LEAK: syncd's default build enables porter-callers test-proc-root or peephole fake"
  fail=1
else
  echo "test-only: syncd's default build has no caller fixtures"
fi

# Test tooling is never a dependency (normal or build) of a program: nothing names ferry-rig at
# all, it is never published, and nothing in dist/ mentions it. The fakes (ferry-fake-graph,
# syncd-fakes) are named only as dev dependencies, or by ferry-rig, which is test tooling itself.
for manifest in crates/*/Cargo.toml; do
  [ "$manifest" = "crates/ferry-rig/Cargo.toml" ] && continue
  if grep -q 'ferry-rig' "$manifest"; then
    echo "LEAK: $manifest names ferry-rig"
    fail=1
  fi
done
for fake in ferry-fake-graph syncd-fakes; do
  named=$(awk -v fake="$fake" '/^\[/ { dev = ($0 ~ /dev-dependencies\]$/) } !dev && $1 == fake && FILENAME != "crates/ferry-rig/Cargo.toml" { print FILENAME }' crates/*/Cargo.toml)
  if [ -n "$named" ]; then
    echo "LEAK: $fake is a normal dependency of: $named"
    fail=1
  else
    echo "test-only: $fake is a dev dependency at most, outside ferry-rig"
  fi
done
if ! grep -q '^publish.workspace = true' crates/ferry-rig/Cargo.toml || ! grep -q '^publish = false' Cargo.toml; then
  echo "LEAK: ferry-rig is publishable"
  fail=1
fi
if grep -rq 'ferry-rig' dist; then
  echo "LEAK: dist/ names ferry-rig"
  fail=1
else
  echo "test-only: nothing depends on ferry-rig, and dist/ does not name it"
fi

# LIBRARY ERRORS CAN GROW: every `pub enum` whose name ends in `Error`, in a library crate, is
# `#[non_exhaustive]`, so a variant added later breaks no caller's `match`. A library crate is
# one a shipped program links: the programs are the binaries dist/ names, the crates they reach
# through normal and build dependencies are the shipped ones, and of those a daemon (a crate
# with a `daemon` module) holds its own errors for itself. Test tooling is never reached from a
# shipped program, so it is outside the rule by what it is.
shipped_programs=$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | jq -r '.packages[] | .name as $p | .targets[] | select(.kind | index("bin")) | "\($p) \(.name)"' \
  | while read -r package program; do grep -rqF -- "$program" dist && echo "$package"; done | sort -u)
shipped=$(for package in $shipped_programs; do
  cargo tree -p "$package" -e normal,build --prefix none 2>/dev/null | grep -E '\(/' | awk '{print $1}'
done | sort -u)
grow_leaks=""
for crate in $shipped; do
  [ -f "crates/$crate/src/lib.rs" ] && [ ! -f "crates/$crate/src/daemon.rs" ] || continue
  found=$(find "crates/$crate/src" -name '*.rs' ! -name 'tests.rs' ! -name '*_tests.rs' ! -name 'testing.rs' \
    ! -name 'testkit.rs' ! -path '*/tests/*' ! -path '*/testkit/*' ! -path '*/testing/*' -print0 \
    | xargs -0 awk '
      FNR == 1 { exhaustive = 1 }
      /^[[:space:]]*#\[non_exhaustive\]/ { exhaustive = 0; next }
      /^[[:space:]]*pub enum [A-Za-z0-9_]*Error([^A-Za-z0-9_]|$)/ {
        if (exhaustive) print FILENAME ":" FNR ": " $0
        exhaustive = 1
        next
      }
      /^[[:space:]]*(pub(\([a-z]+\))? )?(fn|struct|enum|impl|trait|mod|use|const|static|type|union|extern|unsafe|async)[[:space:]]/ { exhaustive = 1 }
      /^}/ { exhaustive = 1 }')
  [ -n "$found" ] && grow_leaks="$grow_leaks$found
"
done
if [ -n "$grow_leaks" ]; then
  printf '%s' "$grow_leaks"
  echo "LEAK: a public error enum of a library crate is not #[non_exhaustive]"
  fail=1
else
  echo "grows: every public *Error enum of the libraries is #[non_exhaustive] ($(echo $shipped | wc -w) shipped crates looked at)"
fi

exit "$fail"
