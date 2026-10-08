# Contributing

ChromaWeave is an experimental codec and camera application. Keep changes focused and preserve wire compatibility unless a format change is explicitly intended.

## Checks

Use Rust 1.88 or later. A C toolchain is needed to build Zstandard. From the repository root:

```sh
cargo fmt --package chromaweave -- --check
cargo fmt --manifest-path ios/RustBridge/Cargo.toml --package cw-codec-ffi -- --check
cargo clippy --locked --all-targets --features tools -- -D warnings
cargo test --release --locked
cargo test --release --locked --features tools
cargo test --release --locked --manifest-path ios/RustBridge/Cargo.toml
cargo test --release --locked --manifest-path vendor/reed-solomon/Cargo.toml
```

The default core, the `tools` feature, and the iOS bridge have separate dependency boundaries. Check all affected paths. [iOS build instructions](ios/README.md) cover simulator tests and device signing.

Rust unit tests live in `src/tests/` and are included as private child modules of the implementation they exercise. Keep helpers private rather than widening the API for tests. Public API and compatibility tests live in `tests/`; the bridge follows the same unit-test layout under `ios/RustBridge/src/tests/`.

Normal tests use committed deterministic fixtures. Ignored tests that require external camera images or perform profiling are optional; they are not substitutes for the committed regression suite. Do not add personal captures, device identifiers, signing credentials, generated frameworks, or build output to a contribution.

## Compatibility and performance

Changes to packet layout, symbol mapping, pilots, ECC, or compression must explain their compatibility impact and update [FORMAT.md](FORMAT.md). Existing fixture expectations come from a recorded checkpoint. Do not regenerate them merely to make a changed implementation pass.

Use regression cases that exercise the failure being fixed. For numeric or performance changes, compare decoded bytes and acceptance/rejection behavior as well as time. A faster decode that loses valid frames can reduce transfer goodput. Follow the [measurement guidance](docs/benchmarking.md).

Keep implementation boundaries small and explicit. The [architecture guide](docs/architecture.md) identifies the owners of codec, capture, transfer, and FFI state. Comments should explain necessary invariants rather than narrate code.

## Contributions and licensing

Contributions are submitted under Apache-2.0 unless explicitly stated otherwise. Preserve third-party notices and document modifications to vendored code. Use plain imperative commit subjects without attribution trailers. Publishing this source tree does not publish any separately ignored experiments or camera archives.
