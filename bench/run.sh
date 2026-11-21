set -eu
export NCPU="$(nproc)" CARGO_TARGET_DIR=/tmp/shop-target
generate=bench/gen/target/release/generate
reset_source() {
    git restore src/main.rs src/input.rs
    rm -f src/block.rs src/radix.rs
}
build() {
    name=$1; shift; reset_source
    for change in "$@"; do git apply bench/"$change".patch; done
    cargo build --release --locked
    cp "$CARGO_TARGET_DIR/release/shopping-sessions" "/tmp/bin-$name"
}
measure() {
    variant=$1
    OUTPUT_DIR="/tmp/out-$variant" /usr/bin/time -a -o /tmp/results -f "$seed $variant %e %M" "/tmp/bin-$variant"
    tail -1 /tmp/results
}
variants=(base parser time radix click)
build base
for v in parser time radix click; do build "$v" "$v"; done
for seed in 42 98; do
    export DATA_DIR=/tmp/data-$seed
    "$generate" --seed "$seed" --events 20000000 --clicks 5000000 --output "$DATA_DIR"
    OUTPUT_DIR=/tmp/warm /tmp/bin-base
    for round in 0 1 2; do
        for n in 0 1 2 3 4; do measure "${variants[$(((n+2*round)%5))]}"; done
    done
    for v in parser time radix click; do python scripts/compare.py /tmp/out-base /tmp/out-"$v"; done
done
choices=$(python bench/choose_variants.py /tmp/results)
echo "SELECTED $choices"
build best $choices
for seed in 42 98; do
    export DATA_DIR=/tmp/data-$seed
    for v in base best best base base best; do measure "$v"; done
    python scripts/compare.py /tmp/out-base /tmp/out-best
done
python scripts/run_fixtures.py fixtures -- /tmp/bin-best
# Free both practice inputs before generating the final-scale dataset.
rm -rf /tmp/data-42 /tmp/data-98
seed=99
export DATA_DIR=/tmp/data-99
"$generate" --seed 99 --events 60000000 --clicks 15000000 --output "$DATA_DIR"
for v in base best best base base best; do measure "$v"; done
python scripts/compare.py /tmp/out-base /tmp/out-best
cat /tmp/results
