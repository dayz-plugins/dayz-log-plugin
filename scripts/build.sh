#!/usr/bin/env bash
# Build gate for the DayZ game event log plugin.
#
# Steps, in order: format check, clippy on the host, tests, docs, clippy for the Windows
# target, release build of gamelog.dll, then an optional deploy. Nothing here
# filters compiler output; a step that fails stops the script.
#
#   scripts/build.sh                 # full gate, no deploy
#   scripts/build.sh --deploy        # gate, then install gamelog.dll into $DAYZ_DIR
#   scripts/build.sh --fast          # skip the host gate, build the artifacts only
#   scripts/build.sh --check         # host gate only, no Windows build
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
container="${BUILD_BOX:-build-box}"
target="x86_64-pc-windows-msvc"
dayz_dir="${DAYZ_DIR:-/run/media/system/Data/Games/Steam/steamapps/common/DayZ}"
loader_repo="${DAYZ_LOADER_DIR:-$repo_root/../dayz-plugin-loader}"

deploy=0
fast=0
check_only=0
for arg in "$@"; do
    case "$arg" in
        --deploy) deploy=1 ;;
        --fast) fast=1 ;;
        --check) check_only=1 ;;
        -h | --help)
            sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown option: $arg" >&2
            exit 2
            ;;
    esac
done

# The SDK is a path dependency on a sibling checkout rather than a published crate, so say so
# plainly instead of letting cargo fail with a path it did not print.
if [[ ! -f "$loader_repo/crates/dayz-plugin-sdk/Cargo.toml" ]]; then
    echo "no dayz-plugin-loader checkout at $loader_repo; set DAYZ_LOADER_DIR" >&2
    exit 1
fi

# Everything runs inside the container: the host is an immutable OS without a toolchain.
run() {
    echo "+ $*"
    distrobox enter "$container" -- bash -c "cd '$repo_root' && $*"
}

if [[ $fast -eq 0 ]]; then
    run cargo fmt --all --check
    run cargo clippy --workspace --all-targets --all-features
    run cargo test --workspace
    run cargo doc --workspace --no-deps
fi

if [[ $check_only -eq 1 ]]; then
    echo "host gate passed"
    exit 0
fi

# The plugin only compiles for Windows.
run cargo xwin clippy -p dayz-log-plugin --target "$target" --all-targets
run cargo xwin build --release -p dayz-log-plugin --target "$target"

dll="$repo_root/target/$target/release/gamelog.dll"
for artifact in "$dll"; do
    if [[ ! -f "$artifact" ]]; then
        echo "missing build output: $artifact" >&2
        exit 1
    fi
    printf '%-12s %8s bytes  %s\n' "$(basename "$artifact")" "$(stat -c %s "$artifact")" \
        "$(date -r "$artifact" '+%F %T')"
done

if [[ $deploy -eq 0 ]]; then
    exit 0
fi

if [[ ! -f "$dayz_dir/DayZ_x64.exe" ]]; then
    echo "no DayZ_x64.exe in $dayz_dir; set DAYZ_DIR" >&2
    exit 1
fi
mkdir -p "$dayz_dir/plugins"
cp -f "$dll" "$dayz_dir/plugins/gamelog.dll"
echo "deployed $dayz_dir/plugins/gamelog.dll"
echo
echo "in the game's console: gamelog.where, gamelog.stats, or 'game' for the loader's side"
