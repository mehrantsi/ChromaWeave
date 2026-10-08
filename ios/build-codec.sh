#!/bin/bash
set -euo pipefail

CW_IOS_ROOT="$(cd "$(dirname "$0")" && pwd)"
CW_CARGO_FLAGS=(--locked)
CW_TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim)
for CW_ARGUMENT in "$@"; do
    case "$CW_ARGUMENT" in
        --offline) CW_CARGO_FLAGS+=(--offline) ;;
        --simulator-only) CW_TARGETS=(aarch64-apple-ios-sim) ;;
        --help)
            echo "Usage: $0 [--offline] [--simulator-only]"
            exit 0
            ;;
        *) echo "Unknown argument: $CW_ARGUMENT" >&2; exit 2 ;;
    esac
done

CW_INPUT_HASH="$(python3 "$CW_IOS_ROOT/codec-build-state.py" fingerprint)"

export CARGO_TARGET_DIR="$CW_IOS_ROOT/build/rust"
export IPHONEOS_DEPLOYMENT_TARGET=18.0
CW_XCODE_ARGUMENTS=()
for CW_TARGET in "${CW_TARGETS[@]}"; do
    cargo build "${CW_CARGO_FLAGS[@]}" --release --manifest-path "$CW_IOS_ROOT/RustBridge/Cargo.toml" --target "$CW_TARGET"
    CW_XCODE_ARGUMENTS+=(-library "$CARGO_TARGET_DIR/$CW_TARGET/release/libcw_codec_ffi.a" -headers "$CW_IOS_ROOT/RustBridge/include")
done

mkdir -p "$CW_IOS_ROOT/Frameworks"
CW_FRAMEWORK="$CW_IOS_ROOT/Frameworks/CWCodec.xcframework"
CW_STAGE="$(mktemp -d "$CW_IOS_ROOT/Frameworks/.CWCodec.XXXXXX")"
cleanup() {
    if [[ -e "$CW_STAGE/previous.xcframework" && ! -e "$CW_FRAMEWORK" ]]; then
        if ! mv "$CW_STAGE/previous.xcframework" "$CW_FRAMEWORK"; then
            echo "Previous framework preserved at $CW_STAGE/previous.xcframework" >&2
            return
        fi
    fi
    rm -rf "$CW_STAGE"
}
trap cleanup EXIT

xcodebuild -create-xcframework "${CW_XCODE_ARGUMENTS[@]}" -output "$CW_STAGE/CWCodec.xcframework"
python3 "$CW_IOS_ROOT/codec-build-state.py" record "$CW_STAGE/CWCodec.xcframework" "$CW_INPUT_HASH" "${CW_TARGETS[@]}"
if [[ -e "$CW_FRAMEWORK" ]]; then
    mv "$CW_FRAMEWORK" "$CW_STAGE/previous.xcframework"
fi
mv "$CW_STAGE/CWCodec.xcframework" "$CW_FRAMEWORK"
