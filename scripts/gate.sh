#!/usr/bin/env bash
# The gate: cargo fmt --check, clippy (pedantic, -D warnings) and the tests on the host, then
# rustdoc, then clippy and a release build for x86_64-pc-windows-msvc. Runs inside the
# `build-box` distrobox and re-execs itself there from the host. Nothing is filtered: each
# step's full output is on screen and in target/gate-logs/<step>.log.
#
# uniloader and dayz-adapter must be checked out as siblings: ../../uniloader and
# ../dayz-adapter (which brings ../dayz-plugin-loader for the dayz-data crate).
#
#   scripts/gate.sh             full gate
#   scripts/gate.sh --host      host steps only (fmt, clippy, tests, docs)
#   scripts/gate.sh --deploy    full gate, then copy the DLLs into $DAYZ_DIR/plugins
#
# Env: UNILOADER_JOBS=N (default: 90% of the cores), UNILOADER_NICE=N (default 10),
#      DAYZ_DIR (default: the Steam library on the data drive).
set -euo pipefail

# What this repository builds, as file names under target/<triple>/release.
DLLS=(gamelog.dll)

SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
ROOT="$(cd "$(dirname "$SCRIPT")/.." && pwd)"
TARGET=x86_64-pc-windows-msvc

HOST_ONLY=0
DEPLOY=0
INNER=()
for arg in "$@"; do
    case "$arg" in
        --host) HOST_ONLY=1; INNER+=("$arg") ;;
        --deploy) DEPLOY=1 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

if [[ "${CONTAINER_ID:-}" != build-box ]]; then
    distrobox enter build-box -- bash -c "export PATH=/opt/rust/cargo/bin:\$PATH RUSTUP_HOME=/opt/rust/rustup CARGO_HOME=\$HOME/.cargo; $(printf '%q ' "$SCRIPT" "${INNER[@]}")"
    if (( DEPLOY )); then
        # On the host: the game folder is not necessarily visible inside the container.
        dayz="${DAYZ_DIR:-/run/media/system/Data/Games/Steam/steamapps/common/DayZ}"
        [[ -f "$dayz/DayZ_x64.exe" ]] || { echo "no DayZ_x64.exe in $dayz; set DAYZ_DIR" >&2; exit 1; }
        mkdir -p "$dayz/plugins"
        for dll in "${DLLS[@]}"; do
            install -m 0644 "$ROOT/target/$TARGET/release/$dll" "$dayz/plugins/$dll"
            echo "deployed $dayz/plugins/$dll"
        done
    fi
    exit 0
fi

LOGS="$ROOT/target/gate-logs"
mkdir -p "$LOGS"
export CARGO_BUILD_JOBS="${UNILOADER_JOBS:-$(( $(nproc) * 9 / 10 > 0 ? $(nproc) * 9 / 10 : 1 ))}"
renice -n "${UNILOADER_NICE:-10}" -p $$ > /dev/null

step() {
    local name="$1"
    shift
    echo "== $name"
    "$@" > >(tee "$LOGS/$name.log") 2>&1
}

cd "$ROOT"
step fmt cargo fmt --all --check
step clippy cargo clippy --workspace --all-targets -- -D warnings
step test cargo test --workspace
step doc env RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
if (( HOST_ONLY )); then
    echo "== host gate passed"
    exit 0
fi
step clippy-windows cargo xwin clippy --workspace --all-targets --target "$TARGET" -- -D warnings
step build-windows cargo xwin build --release --workspace --target "$TARGET"
for dll in "${DLLS[@]}"; do
    [[ -f "target/$TARGET/release/$dll" ]] || { echo "missing build output: $dll" >&2; exit 1; }
done
echo "== gate passed"
