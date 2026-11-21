set -eu
export NCPU="$(nproc)" DATA_DIR=/tmp/shop-input
export CARGO_TARGET_DIR=/tmp/shop-target
for variant in base inplace fewer plain plain256; do
    git restore src/main.rs src/output.rs
    if [ "$variant" != base ]; then git apply bench/inplace.patch; fi
    case "$variant" in
        fewer|plain) sed -i 's/PARTITIONS_PER_FILE: usize = 1;/PARTITIONS_PER_FILE: usize = 8;/' src/main.rs ;;
    esac
    case "$variant" in
        plain|plain256) sed -i 's/Compression::SNAPPY/Compression::UNCOMPRESSED/' src/output.rs ;;
    esac
    cargo build --release --locked
    cp "$CARGO_TARGET_DIR/release/shopping-sessions" "/tmp/shop-$variant"
done
targets=(base inplace fewer plain plain256)
if grep -qw avx512_vnni /proc/cpuinfo && grep -qw avx512bw /proc/cpuinfo; then
    git restore src/main.rs src/output.rs
    RUSTFLAGS='-C target-cpu=cascadelake' cargo build --release --locked
    cp "$CARGO_TARGET_DIR/release/shopping-sessions" /tmp/shop-cascade
    targets+=(cascade)
else
    echo 'Cascade Lake ISA unavailable on this runner'
fi
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

PROFILE_OUTPUT=1 OUTPUT_DIR=/tmp/profile /tmp/shop-inplace
