# ChromaWeave

CW Luma 10 is an experimental visual data code for images and screen-to-camera transfers. It includes a Rust codec, command-line tools, and a Rust-backed iPhone scanner.

Each cell carries ten coded bits: four two-bit luminance samples and two shared chroma bits. Finders, metadata, and calibration pilots are integrated into the grid. Standard and Low select error-correction strength within the same layout. The format is documented in [FORMAT.md](FORMAT.md).

## Quick start

Requires Rust 1.88 or later and a C compiler for Zstandard.

```sh
printf '%s\n' 'Hello from ChromaWeave' > /tmp/cw-message.txt
cargo run --release --locked --features tools -- encode /tmp/cw-message.txt /tmp/cw.png --ecc standard --cell-pixels 8
cargo run --release --locked --features tools -- decode /tmp/cw.png /tmp/recovered.txt
```

Standard uses RS(255,159); Low uses RS(255,207). `--cell-pixels` must be even, between 2 and 64. Larger cells are easier to capture. A two-pixel cell is the digital raster minimum and is generally unsuitable for camera scanning.

Optional lossless compression uses Zstandard level 3 only when it saves an error-correction data block:

```sh
cargo run --release --locked --features tools -- encode /tmp/cw-message.txt /tmp/cw-compressed.png --ecc low --compression auto
```

Compression is off by default. The decoder restores the original bytes, with a 16 MiB decompression limit. Already compressed media may gain little.

## Rust API

The default library operates on in-memory RGB images without the CLI, file codecs, or comparison dependencies.

```rust
let encoded = chromaweave::encode(b"hello", 8)?;
let decoded = chromaweave::decode(&encoded.image)?;
assert_eq!(decoded.payload, b"hello");
```

Use `encode_with_options` for ECC and compression. `scan` exposes detection status when decoding fails; `PhotoRecovery` combines up to four observations of one code. `TransferScanner` reuses geometry across changing codes. `codec` exposes lower-level operations on aligned grids.

The `tools` feature enables the executable, PNG/JPEG I/O, synthetic captures, and QR comparison utilities. The API and wire format remain experimental; the committed compatibility corpus guards the current format.

## iPhone scanner

The [iOS app](ios/README.md) supports Live, Photo, and Transfer modes, text/binary inspection, image and animated-GIF previews, and original-byte export. It also generates static codes. Building requires macOS, Xcode, and Rust's iOS targets.

Live scanning and continuous transfers have worked through 20 KiB per code in development tests. Large static codes around 50–123 KiB remain unreliable through the camera, even when direct image decoding succeeds. Real print performance is unverified. There is no established general density or reliability advantage over other color-code systems.

## Animated transfer

Generate a looping player with four repair symbols per group of twenty source chunks:

```sh
cargo run --release --locked --features tools -- transfer input.bin /tmp/cw-transfer --chunk-bytes 20480 --fps 12 --ecc low --cell-pixels 8 --repair-group 20
```

Open `/tmp/cw-transfer/index.html`, select Transfer on the phone, and start playback. The player preserves physical pixel size and reports when the code does not fit the display. The receiver accepts source and repair chunks in any order, retains them across loops, and delivers the file only after complete SHA-256 validation. Files are limited to 32 MiB. Transfers are uncompressed.

Transfers use CWT with groups of twenty sources by default; `--repair-group 60` selects groups of sixty. Each group adds four repair symbols. The repair-hold control can repeat repair symbols; 1× is the default.

A completed 20 KiB / 12 FPS screen-to-phone transfer measured 1.587 Mbit/s in development. That measurement covers the first accepted chunk through full-file validation, excluding initial acquisition. It is a measured run, not a sustained-rate or reliability guarantee. See [measurement guidance](docs/benchmarking.md) for timing boundaries and reproducible checks.

## Development

```sh
cargo test --release --locked
cargo test --release --locked --features tools
cargo test --release --locked --manifest-path ios/RustBridge/Cargo.toml
```

See [architecture](docs/architecture.md), [contributing](CONTRIBUTING.md), and the [compatibility corpus](tests/fixtures/README.md). Generated experiments and personal captures are not required for the normal test suite.

## License

[Apache-2.0](LICENSE). The vendored Reed–Solomon implementation remains MIT-licensed; see [NOTICE](NOTICE) and its [modification record](vendor/reed-solomon/PATCHES.md).
