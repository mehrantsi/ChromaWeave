#!/bin/sh
set -eu

checkpoint=b73e3d7c98884191a58b0bd1123009040c0d13a0
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output=${1:?Usage: tests/regenerate_release_corpus.sh EMPTY_OUTPUT_DIRECTORY}
mkdir -p "$output"
output=$(CDPATH= cd -- "$output" && pwd)
if [ -n "$(ls -A "$output")" ]; then
    printf '%s\n' 'Output directory must be empty.' >&2
    exit 1
fi
baseline=$(mktemp -d "${TMPDIR:-/tmp}/cw-release-baseline.XXXXXX")
trap 'rm -rf "$baseline"' EXIT HUP INT TERM

git -C "$repo" archive "$checkpoint" | tar -x -C "$baseline"
mkdir -p "$baseline/tests/support" "$baseline/tests/fixtures"
cp "$repo/tests/fixtures/camera-code-only.png" "$baseline/tests/fixtures/camera-code-only.png"
cp "$repo/tests/fixtures/camera-provenance.json" "$baseline/tests/fixtures/camera-provenance.json"
cp "$repo/tests/release_corpus.rs" "$baseline/tests/release_corpus.rs"
cp "$repo/tests/support/release_cases.rs" "$baseline/tests/support/release_cases.rs"
cp "$repo/tests/support/checkpoint_capture.rs" "$baseline/tests/release_capture_generation.rs"
cat "$repo/tests/support/checkpoint_wire.rs" >> "$baseline/src/codec.rs"
export CW_CORPUS_OUTPUT="$output"
export CW_RELEASE_CASES="$repo/tests/support/release_cases.rs"
export CARGO_TARGET_DIR="$repo/target/opensource-baseline"
cargo test --release --offline --manifest-path "$baseline/Cargo.toml" --test release_corpus generate_release_corpus -- --ignored --exact
cargo test --release --offline --manifest-path "$baseline/Cargo.toml" --lib checkpoint_private_codewords_match_distributable_wire
cargo test --release --offline --manifest-path "$baseline/Cargo.toml" --features tools --test release_capture_generation generate_synthetic_projective_capture -- --exact
