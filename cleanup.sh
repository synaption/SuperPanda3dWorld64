#!/usr/bin/env bash
# Drop the debug build tree. Release artifacts and the dependency cache stay,
# so the next `cargo run --release` is still incremental.
rm -rf "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/target/debug"
#cargo clean
