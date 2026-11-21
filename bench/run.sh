set -eu
export NCPU="$(nproc)" DATA_DIR=/tmp/shop-input
export CARGO_TARGET_DIR=/tmp/shop-target
targets=(base copy inplace)
if grep -qw avx512_vnni /proc/cpuinfo && grep -qw avx512bw /proc/cpuinfo; then
    targets+=(copy-cascade inplace-cascade)
fi
for variant in "${targets[@]}"; do
    git restore src/main.rs src/output.rs
    unset RUSTFLAGS
    if [ "$variant" != base ]; then git apply "bench/final-${variant%-cascade}.patch"; fi
    if [[ "$variant" == *-cascade ]]; then export RUSTFLAGS='-C target-cpu=cascadelake'; fi
    cargo build --release --locked
    cp "$CARGO_TARGET_DIR/release/shopping-sessions" "/tmp/shop-$variant"
done
# Warm the input cache before rotating timed runs.
OUTPUT_DIR=/tmp/out-base /tmp/shop-base
for round in 1 2 3; do
    for ((n=0;n<${#targets[@]};n++)); do
        variant="${targets[$(((n+round-1)%${#targets[@]}))]}"
        export OUTPUT_DIR="/tmp/out-$variant"
        /usr/bin/time -f "RESULT $variant %e %M" "/tmp/shop-$variant"
    done
done
for variant in "${targets[@]:1}"; do
    python scripts/compare.py /tmp/out-base "/tmp/out-$variant"
done
PROFILE_OUTPUT=1 OUTPUT_DIR=/tmp/profile /tmp/shop-copy
