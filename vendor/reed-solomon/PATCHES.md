# Local changes to reed-solomon 0.2.1

Source: the published `reed-solomon` 0.2.1 crate from
https://github.com/mersinvald/reed-solomon-rs. The upstream MIT license is retained
in `LICENSE`. Library source and tests are copied unchanged except for
`src/decoder.rs`.

The decoder's syndrome calculation and corruption check use a static 64 KiB
GF(256) multiplication table for polynomial evaluation at a fixed point. The
primitive polynomial remains 0x11d. Evaluation order, returned values, early
rejection, correction limits, and validation checks remain unchanged. Added tests
compare every multiplication pair and polynomial evaluations against the original
field operations. Other polynomial operations retain the upstream implementation.

The unused `rustc-serialize` development dependency is omitted; upstream benchmark
examples are not included. Library tests and the two integration tests run offline.
The Cargo package version remains 0.2.1; ChromaWeave selects this copy explicitly
through its path dependency, including when built through the iOS bridge.
