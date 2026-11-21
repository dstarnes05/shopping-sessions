#!/bin/sh
set -eu

# Build portable binaries on hosts that cannot execute the scorer's instruction set.
if ! grep -qw avx512_vnni /proc/cpuinfo; then
    exec cargo build --release --locked
fi

cargo build --release --locked --manifest-path training/generator/Cargo.toml
training/generator/target/release/generate --seed 7 --events 500000 --clicks 125000 --output /tmp/training
rustup component add llvm-tools-preview
export RUSTFLAGS="-C target-cpu=cascadelake"
cargo rustc --release --locked -- -Cprofile-generate=/tmp/shop-profile
NCPU=4 DATA_DIR=/tmp/training OUTPUT_DIR=/tmp/training-out target/release/shopping-sessions
host=$(rustc -vV | sed -n 's/^host: //p')
"$(rustc --print sysroot)/lib/rustlib/$host/bin/llvm-profdata" merge /tmp/shop-profile -o /tmp/trained.profdata
cargo rustc --release --locked -- -Cprofile-use=/tmp/trained.profdata
