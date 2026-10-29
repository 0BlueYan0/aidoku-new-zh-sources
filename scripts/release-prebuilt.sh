#!/usr/bin/env bash
# Packages a source whose code is kept out of this repository and copies the result to
# prebuilt/<id>.aix, which CI lists with the other sources.
#
#   scripts/release-prebuilt.sh zh.bakamh2
#
# The source must be checked out at sources/<id> (ignored by this repository). Runs the same
# checks as scripts/check-pr.sh plus the unit tests. When sources/<id>/.release-hidden lists
# strings (one per line), the packaged wasm must not contain any of them.
set -euo pipefail

TOOLCHAIN="1.99.0"
root="$(cd "$(dirname "$0")/.." && pwd)"
id="${1:?usage: scripts/release-prebuilt.sh <source id>}"
dir="$root/sources/$id"

[[ -f "$dir/Cargo.toml" ]] || { echo "$dir/Cargo.toml is missing" >&2; exit 1; }
if git -C "$root" ls-files --error-unmatch "sources/$id" >/dev/null 2>&1; then
	echo "sources/$id is tracked by this repository; its code would be published" >&2
	exit 1
fi
export RUSTUP_TOOLCHAIN="$TOOLCHAIN"

cd "$dir"
cargo fmt --check
cargo clippy --quiet --target wasm32-unknown-unknown -- -D warnings
cargo test --quiet
RUSTFLAGS=-Dwarnings aidoku package
aidoku verify package.aix

if [[ -f .release-hidden ]]; then
	unpacked="$(mktemp -d)"
	trap 'rm -rf "$unpacked"' EXIT
	unzip -q package.aix Payload/main.wasm -d "$unpacked"
	leaked=0
	while IFS= read -r hidden; do
		[[ -n "$hidden" ]] || continue
		if grep -qaF -- "$hidden" "$unpacked/Payload/main.wasm"; then
			echo "main.wasm still contains: $hidden" >&2
			leaked=1
		fi
	done < .release-hidden
	(( leaked == 0 )) || exit 1
fi

mkdir -p "$root/prebuilt"
cp package.aix "$root/prebuilt/$id.aix"
echo "prebuilt/$id.aix updated"
