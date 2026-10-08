# Measuring performance and reliability

Always pair timing with exact payload recovery. Compare the same source bytes, code profile, pitch, display footprint, camera configuration, and recovery policy. Report failed attempts and duplicates, not only successful decoder calls.

## Host checks

Run the committed corpus before and after a change:

```sh
cargo test --release --locked --test release_corpus
cargo test --release --locked --test camera_regression
```

The fixtures pin packet bytes, raster pixels, transfer envelopes, and expected decoded payloads. The real-camera fixture is a cropped and downscaled project photograph; replay measures decoder processing and excludes camera acquisition. Fixture provenance and generation details are under `tests/fixtures`.

The `bench` CLI compares synthetic CW and QR captures. It is a deterministic development model, not evidence of real print or phone performance:

```sh
cargo run --release --locked --features tools -- bench --trials 10 --output /tmp/cw-comparison.json
```

## Phone transfer

Generate a known input and save its SHA-256, code dimensions, application chunk size, group size, ECC, and requested FPS. Keep browser scaling, screen brightness, viewing distance, lens, and ambient light fixed across paired tests. The player records presentation timestamps and measured playback; these are browser timings, not optical measurements.

Use several complete runs after the phone reaches a comparable temperature. Report:

- Acquisition delay, from presentation or a defined user action to first accepted chunk.
- Decoder-call time, queue residence, dropped camera frames, and overwritten pending buffers.
- Unique source chunks, cached repair symbols, reconstructed chunks, duplicates, and failed attempts.
- Final exact payload verification and completed-file goodput.

The app's final goodput is `original_bytes * 8 / elapsed_seconds`, in decimal Mbit/s. Its elapsed time begins with the first accepted chunk's buffer and ends after full-file validation. Acquisition before that first acceptance is excluded. A brief live-rate peak is not completed-file goodput.

For a 1,228,800-byte transfer, 20 KiB chunks at 12 FPS with twenty source plus four repair symbols per group offer 1.6384 Mbit/s of useful payload per complete cycle at 1× repair hold. Ten KiB chunks at 20 FPS with sixty plus four offer 1.536 Mbit/s. Losses, transitions, duplicated observations, and retries reduce received goodput; larger hold multipliers consume additional airtime.

A development run on an iPhone 17 Pro Max completed the 20 KiB / 12 FPS / 1× case at 1.587128 Mbit/s with a matching full-file SHA-256. Other runs varied. This result demonstrates one setup and timing window, not a device-independent throughput guarantee. Personal logs and full captures are excluded from the source distribution.

Matched CW/JAB development photos were a one-capture-per-profile case study. Such a sample cannot establish comparative reliability or maximum density. A comparative report needs repeated trials, disclosed ECC and module dimensions, failure stages, and the reference decoder version.
