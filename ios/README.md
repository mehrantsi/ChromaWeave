# iPhone app

The app captures CW Luma 10 with a Rust decoder, displays recovered text and binary data, previews supported images and animated GIFs, and exports the original bytes. It supports static-code generation and out-of-order animated transfers. It does not decode JABCode.

## Build

Use an Apple Silicon Mac, Xcode with an iOS 18 or later SDK, Python 3, and Rust 1.88 or later. Install the Rust targets once:

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
```

From the repository root, build the Rust bridge before opening or building the app:

```sh
./ios/build-codec.sh
open ios/ChromaWeave.xcodeproj
```

The script uses the committed Cargo lockfiles and replaces the generated XCFramework only after packaging succeeds. Repeated builds are supported. `--offline` uses cached dependencies; `--simulator-only` builds only the ARM64 simulator slice. Generated frameworks and build output are ignored.

Xcode validates the framework's source fingerprint and platform before compiling Swift. After modifying Rust, its manifests, vendor code, bridge headers, or the build script, rerun `build-codec.sh`. A stale or missing bridge produces an explicit error instead of silently linking old code.

Select the ChromaWeave scheme. For a physical device, select your own signing team and a bundle identifier provisioned for it. No personal signing team or device identifier is stored in the project. Allow camera access on first launch.

## Simulator checks

Choose an available iPhone simulator from `xcrun simctl list devices available`, then run:

```sh
./ios/build-codec.sh --simulator-only
xcodebuild -project ios/ChromaWeave.xcodeproj -scheme ChromaWeave \
  -configuration Debug -destination 'platform=iOS Simulator,name=YOUR_SIMULATOR' \
  -derivedDataPath ios/build/tests CODE_SIGNING_ALLOWED=NO test
xcodebuild -project ios/ChromaWeave.xcodeproj -scheme ChromaWeave \
  -configuration Release -sdk iphonesimulator \
  -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath ios/build/release CODE_SIGNING_ALLOWED=NO build
```

Hosted tests cover transfer repair, strict bridge parsing, session ownership, stale-generation cancellation, photo normalization, paired archives, and diagnostic cleanup. Simulator tests cannot validate camera autofocus, display moiré, or screen-to-phone throughput.

Development replay and self-test launch arguments compile only in Debug through `CW_DEVELOPMENT`. Release builds exclude these routes. Ordinary upgrades preserve saved captures; Debug `--reset-diagnostics` explicitly removes app-owned diagnostic artifacts while preserving decoded exports and replay fixtures.

## Capture and transfer

Live detects and decodes automatically. Photo captures a still image and pairs it with diagnostics from the same attempt. Transfer retains validated chunks and repair symbols regardless of their arrival order, combines them across playback loops, and exposes live and completed-file goodput.

Live and Transfer use the physical Main camera, continuous autofocus, and a supported 4K/60 FPS format where available. Keep beyond the lens's minimum focus distance and leave all four finders visible. Capture format, focus, timing, buffer drops, and decoder outcomes are recorded in diagnostics. Phone sleep is disabled while active capture requires it and restored when capture stops.

Display generated codes at native pixel size. Resizing a dense image to fit a window can destroy the cell detail before the camera sees it. Start with 10 or 20 KiB transfer chunks rather than a large static media payload. See [benchmarking](../docs/benchmarking.md) for rate definitions and test controls.

## Ownership

See [architecture](../docs/architecture.md) for the acquisition, decode, diagnostic, and FFI boundaries. Rust handles are owned by scoped Swift session objects. Generation tickets prevent queued work from publishing after a mode change or stop; decoder work remains serialized.
