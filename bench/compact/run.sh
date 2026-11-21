set -eu
export NCPU=4 DATA_DIR=/tmp/shop-data
for variant in baseline compact; do
    if [ "$variant" = compact ]; then
        cp ../bench/compact/*.rs src/
        cargo test --locked
    fi
    sh training/build.sh
    cp target/release/shopping-sessions "/tmp/$variant"
    rm -rf /tmp/shop-profile /tmp/training /tmp/training-out
done

gen=training/generator/target/release/generate
for seed in 42 98 99; do
    events=20000000
    if [ "$seed" = 99 ]; then events=60000000; fi
    "$gen" --seed "$seed" --events "$events" --clicks "$((events/4))" --output "$DATA_DIR"
    for variant in baseline compact; do OUTPUT_DIR="/tmp/out-$variant" "/tmp/$variant"; done
    for variant in baseline compact compact baseline baseline compact compact baseline; do
        OUTPUT_DIR="/tmp/out-$variant" /usr/bin/time -o /tmp/timings -a -f "$seed $variant %e %M" "/tmp/$variant"
    done
    python scripts/compare.py /tmp/out-baseline /tmp/out-compact
    rm -rf "$DATA_DIR"
done
cat /tmp/timings
"$gen" --seed 1 --events 1000000 --clicks 250000 --output "$DATA_DIR"
OUTPUT_DIR=/tmp/out-dev /tmp/compact
python ../bench/digest.py /tmp/out-dev > /tmp/digest
diff ../bench/dev-digest /tmp/digest
python scripts/run_fixtures.py fixtures -- /tmp/compact
