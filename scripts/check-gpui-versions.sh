#!/usr/bin/env bash
# Builds and tests gpui-tui against each given gpui-pre version, in scratch copies of the
# workspace, so the real manifests and lockfile stay untouched.
#
# Usage: scripts/check-gpui-versions.sh [VERSION...]
# Without arguments, checks the versions gpui-tui currently supports.

set -euo pipefail

SUPPORTED_VERSIONS=(0.3.9)
CURRENT_PIN="=0.3.9"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scratch_root="${GPUI_TUI_MATRIX_DIR:-${TMPDIR:-/tmp}/gpui-tui-version-matrix}"
versions=("$@")
if [[ ${#versions[@]} -eq 0 ]]; then
    versions=("${SUPPORTED_VERSIONS[@]}")
fi

# Fetch dependencies through Socket Firewall when it is installed.
cargo_fetching=(cargo)
if command -v sfw >/dev/null; then
    cargo_fetching=(sfw cargo)
fi

declare -A results
for version in "${versions[@]}"; do
    workspace="$scratch_root/$version"
    echo "==> gpui-pre $version ($workspace)"
    mkdir -p "$workspace"
    rsync -a --delete --exclude /target --exclude /.git "$repo_root/" "$workspace/"

    # Pin every gpui-pre crate to this version and resolve a fresh lockfile.
    sed -i "s/version = \"$CURRENT_PIN\"/version = \"=$version\"/" "$workspace/Cargo.toml"
    rm -f "$workspace/Cargo.lock"

    if (
        cd "$workspace"
        export CARGO_TARGET_DIR="$scratch_root/target"
        "${cargo_fetching[@]}" generate-lockfile &&
            "${cargo_fetching[@]}" fetch &&
            cargo test --workspace --offline
    ); then
        results[$version]=ok
    else
        results[$version]=FAILED
    fi
done

echo
echo "gpui-pre version results:"
status=0
for version in "${versions[@]}"; do
    echo "  $version: ${results[$version]}"
    [[ ${results[$version]} == ok ]] || status=1
done
exit $status
