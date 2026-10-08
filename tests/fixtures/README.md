# Release regression fixtures

These project fixtures are distributed under Apache-2.0. No external fixture
paths, network access, or `tools` feature are needed to run
`cargo test --release --offline --test release_corpus` with cached dependencies.

## Deterministic checkpoint corpus

`release/` uses codec checkpoint
`b73e3d7c98884191a58b0bd1123009040c0d13a0`, retained in the project history.
It records the canonical `CWT\0` transfer markers, headers, source data, and
repair bytes.
The manifest records payload, packet/RS, and raw RGB SHA-256 values plus layouts.
Lossless PNGs are decoded and compared pixel-for-pixel, so PNG container encoding is not the compatibility
contract. Wire files contain complete padded RS blocks. Checkpoint-private
`codewords` was independently checked against every stored wire file.

The twenty optical cases cover Standard and Low ECC, empty and one-byte input,
RS data-block boundaries, minimum-grid capacities and the next byte, binary
input, selected Zstandard compression, compressible input that saves no RS
block, and incompressible automatic input. The eight transfer cases cover both
10/20 KiB chunks and 20/60-source groups, three-byte final chunks, and partial
final groups. Every CWT envelope has a digest of the checkpoint output; complete short and final-group CWT symbols are retained for independent
parsing and recovery of up to four missing source shards.

`synthetic-projective.png` is a **synthetic** capture with rotation, perspective,
and clutter. It is not a camera photograph. Its 1,007-byte SplitMix64 payload
and capture parameters are in `../support/release_cases.rs` and
`../support/checkpoint_capture.rs`.

With the recorded checkpoint object available in Git and dependencies cached,
regenerate into a new empty directory:

```sh
tests/regenerate_release_corpus.sh /tmp/cw-release-corpus
```

The script extracts the checkpoint into a temporary checkout, builds in a
separate target directory, exports the fixtures, verifies private checkpoint
codewords, and removes the temporary checkout. Synthetic capture generation
enables the `tools` feature. Compare against `release/`; do not regenerate
goldens from a changed encoder to accept a format change.

## Camera photograph

`camera-code-only.png` is derived from the owner's project camera photograph of
a random 20,480-byte Low ECC symbol. The owner authorized its inclusion under
the project license. The code-only crop was reviewed before distribution.
`camera-provenance.json` records original/output digests, crop coordinates,
resize parameters, payload digest, privacy review, and rights assumptions.
The private original is intentionally absent.

The original 3,024 × 4,032 RGB image was cropped to `(95, 640, 2890, 3460)`
using Pillow's left/top/right/bottom convention, then resized with LANCZOS to
1,398 × 1,410 and saved as a fresh RGB PNG without metadata. This removes page
text, surrounding UI, cursor, and photograph metadata; no third-party GIF is
included. Exact payload recovery is checked for the original crop, rotation,
reflection, `TransferScanner`, and `PhotoRecovery`. These geometric variants
come from one exposure, not independent camera captures.

Camera resampling cannot be regenerated without the private original. The
committed PNG and SHA-256 are the regression input; its payload digest was
validated with the checkpoint decoder. Existing external-camera hooks remain
optional for private captures.

`finder-basin.png` predates this corpus and is a project-generated synthetic
finder fixture retained unchanged.
