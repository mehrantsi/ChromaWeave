# Architecture

## Rust

The crate root exposes framed encoding and automatic scanning. `codec` owns packet layout, Reed–Solomon interleaving, symbol coding, and aligned-grid operations. `integrated_layout` defines the reserved cells; `luma`, `luma_levels`, and `luma_channel` implement symbol synthesis and classification. `compression` owns the bounded Zstandard envelope.

`scanner` detects the integrated frame and orchestrates candidate orientation, preparation, payload decoding, and failure selection. Its private `sampling` module owns registration, lattice sampling, pilot construction, and equalization. `finder_refine`, `grid_refine`, `geometry`, and `spatial_equalizer` provide geometry and sampling. Recovery does not change the format: `recovery` retains compatible observations and corrected RS blocks, while `recovery_signal` and `recovery_fit` estimate and combine signal evidence. `recovery_signal` separates initial classification, local levels, binary chroma, and coupled luminance fitting/inference; the [model equations](recovery-model.md) define the numerical assumptions and coefficient layout. A contradictory recovered block rejects the added observation; full packet validation remains mandatory. An RS-valid block is a decoding hypothesis, not cryptographic authentication.

`transfer` handles CWT packet validation, serialization, and source splitting. `transfer_repair` implements group erasure coding. `TransferScanner` caches optical geometry; it does not combine payload observations from different animated frames.

The `tools` feature adds `main`, synthetic channel/capture models, comparisons, and transfer file/player generation. The default dependency graph excludes those tools and image-file decoders. Shared geometry has no dependency on simulation.

## iOS

`ChromaWeaveApp` owns the SwiftUI views. `CameraScanner` is the observable UI/lifecycle facade: it consumes typed `CameraScanOutput` events and publishes presentation state on the main queue. It does not forward mutable acquisition or decoder properties.

`CameraAcquisition` owns the AVFoundation session, device configuration, focus and capture observers, video delegate, and still-photo acquisition. `CameraDecodeCoordinator` owns pending buffers, decode scheduling, transfer assembly, recovery sessions, and capture archival. Live keeps the newest pending frame; Transfer retains at most two pending frames plus one being decoded.

`CameraScanState` contains the shared generation, scan mode, lifecycle flags, and presentation timing under one lock. Acquisition and decoding retain their session/capture/decode queue boundaries; owners check generation tickets before processing or publishing queued work. A mode change or cancellation invalidates old work without transferring ownership of decoder sessions to the UI.

`TransferReceiver` owns file assembly in Swift. It retains source and repair symbols independent of arrival order and validates the final SHA-256, invoking Rust for erasure recovery.

`ScanDiagnosticStore` stores scan diagnostics; `CameraArchives` persists paired images and diagnostics. Explicit diagnostic cleanup only removes app-owned artifacts. Ordinary upgrades preserve captures.

`Codec` is the FFI entry point. `CodecImages` normalizes image buffers; `CodecResponses` parses typed bridge responses; `CodecSessions` owns and releases Rust session handles. Payload presentation is separate in `PayloadContent` and `CodecModels`. The C ABI is in `ios/RustBridge/include/CWCodec.h`.

`DevelopmentReplay` and separate `TransferReceiverTestSupport` compile only with `CW_DEVELOPMENT` in Debug. Release builds have no fixture-replay route. Hosted XCTest exercises ownership, cancellation, buffer bounds, protected manual captures, transfer recovery, image normalization, response validation, and diagnostic persistence.

Rust unit tests live in `src/tests/` and `ios/RustBridge/src/tests/` as private child modules of the code they exercise. Public API and compatibility cases live in `tests/`. Fixture regeneration uses a checkpoint retained in the project history and checks fixed golden bytes; it does not depend on private captures or unreachable Git objects.

## Data boundary

```text
bytes → optional compression → packet + CRC → RS + interleave
      → Luma 10 cells + integrated frame → RGB image

camera → finders + metadata → geometry + pilot calibration
       → cell estimates → deinterleave + RS → CRC + padding checks
       → bounded decompression → original bytes

animated frames → validated chunks / cached repair symbols
                → out-of-order assembly → full-file SHA-256
```

Raster minima count coded bits, ECC, and reserved cells. They do not establish physical camera capacity. CRC and SHA checks detect accidental corruption; neither authenticates the sender.
