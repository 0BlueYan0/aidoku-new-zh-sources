#!/usr/bin/env bash
# Runs the checks of .github/workflows/pr.yaml (the same as Aidoku-Community/sources) on this
# machine, so a PR is not opened only to find out on GitHub.
#
#   scripts/check-pr.sh                 sources changed against origin/main, committed or not
#   scripts/check-pr.sh --all           every source
#   scripts/check-pr.sh zh.komiic2 ...  the named sources
#
# Uses Rust 1.99.0 like the workflow (rustup toolchain install 1.99.0 -c clippy -c rustfmt
# -t wasm32-unknown-unknown). Needs the aidoku CLI on PATH. Run it from Git Bash on Windows.
set -uo pipefail

TOOLCHAIN="1.99.0"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root" || exit 1

if ! rustup run "$TOOLCHAIN" rustc --version >/dev/null 2>&1; then
	echo "Rust $TOOLCHAIN is missing: rustup toolchain install $TOOLCHAIN -c clippy -c rustfmt -t wasm32-unknown-unknown" >&2
	exit 1
fi
export RUSTUP_TOOLCHAIN="$TOOLCHAIN"

sources=()
if [[ "${1:-}" == "--all" ]]; then
	for dir in sources/*/; do
		sources+=("${dir%/}")
	done
elif (( $# )); then
	for name in "$@"; do
		sources+=("sources/${name#sources/}")
	done
else
	base="$(git merge-base origin/main HEAD)"
	readarray -t sources < <(
		{
			git diff --name-only "$base" 2>/dev/null
			git ls-files --others --exclude-standard
		} | awk -F/ 'NF >= 2 && $1 == "sources" { print $1 "/" $2 }' | sort -u
	)
fi

# prebuilt/*.aix are sources packaged elsewhere (scripts/release-prebuilt.sh); pr.yaml
# verifies them with the rest.
shopt -s nullglob
prebuilt=(prebuilt/*.aix)
shopt -u nullglob

if (( ${#sources[@]} == 0 && ${#prebuilt[@]} == 0 )); then
	echo "No source changed."
	exit 0
fi

failures=()
packages=("${prebuilt[@]}")

for source in "${sources[@]}"; do
	[[ -f "$source/Cargo.toml" ]] || continue
	echo "== $source"

	if ! (cd "$source" && cargo fmt --check >/dev/null); then
		failures+=("$source: cargo fmt --check (run cargo fmt in $source)")
	fi

	# Saved site pages and their tests are not in git, so the workflow formats without
	# them. A `#[path]` module rustfmt cannot find fails there and passes here unless the
	# folders are moved aside for a second run (2026-10-30, PR #39).
	hidden=()
	for dir in "$source/src/fixture_tests" "$source/src/fixtures" "$source/tests/fixtures"; do
		if [[ -d "$dir" ]] && mv "$dir" "$dir.check-pr-hidden"; then
			hidden+=("$dir")
		fi
	done
	trap 'for dir in "${hidden[@]}"; do mv "$dir.check-pr-hidden" "$dir"; done' INT TERM EXIT
	if (( ${#hidden[@]} )) && ! (cd "$source" && cargo fmt --check >/dev/null 2>&1); then
		failures+=("$source: cargo fmt --check without the local fixtures (add #[rustfmt::skip] above the fixture_tests mod)")
	fi
	for dir in "${hidden[@]}"; do
		mv "$dir.check-pr-hidden" "$dir"
	done
	hidden=()
	trap - INT TERM EXIT

	if ! (cd "$source" && cargo clippy --quiet --target wasm32-unknown-unknown -- -D warnings); then
		failures+=("$source: cargo clippy -D warnings")
	fi

	if package_output="$(cd "$source" && RUSTFLAGS=-Dwarnings aidoku package 2>&1)"; then
		packages+=("$source/package.aix")
	else
		echo "$package_output"
		failures+=("$source: RUSTFLAGS=-Dwarnings aidoku package")
	fi

	if [[ -d "$source/res" ]]; then
		while IFS= read -r -d '' file; do
			if [[ -s "$file" && -n "$(tail -c 1 "$file")" ]]; then
				failures+=("$file: missing final newline")
			fi
			if grep -qE $'^\t* +' "$file"; then
				failures+=("$file: indentation uses spaces (line $(grep -nE $'^\t* +' "$file" | head -1 | cut -d: -f1))")
			fi
		done < <(find "$source/res" -type f -name '*.json' -print0)
	fi
done

if (( ${#packages[@]} )); then
	verify_output="$(aidoku verify "${packages[@]}" 2>&1)"
	verify_status=$?
	if (( verify_status )) || grep -q "\.\.\. no" <<< "$verify_output"; then
		echo "$verify_output"
		failures+=("aidoku verify")
	fi
fi

echo
if (( ${#failures[@]} )); then
	echo "FAILED (${#failures[@]}):"
	printf '  %s\n' "${failures[@]}"
	exit 1
fi
echo "All checks passed for ${#sources[@]} source(s)."
