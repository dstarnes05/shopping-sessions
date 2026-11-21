set -eu
export NCPU="$(nproc)" DATA_DIR=/tmp/shopdata CARGO_TARGET_DIR=/tmp/shopbuild
gen=bench/gen/target/release/generate
variants=(base count batches mapped row tuned pgo)
for v in "${variants[@]}"; do
    git restore src Cargo.lock Cargo.toml
    unset RUSTFLAGS
    case "$v" in
      count) cp bench/count.rs src/main.rs ;;
      batches|mapped|row|tuned|pgo) cp bench/main.rs src/main.rs ;;
    esac
    case "$v" in mapped|row|tuned|pgo) cp bench/mapped.toml Cargo.toml; cp bench/mapped.lock Cargo.lock; cp bench/input.rs src/ ;; esac
    case "$v" in row|tuned|pgo) cp bench/block.rs src/ ;; esac
    case "$v" in tuned|pgo) export RUSTFLAGS='-C target-cpu=cascadelake';; esac
    if [ "$v" = pgo ]; then
        rustup component add llvm-tools-preview
        cargo rustc --release --locked -- -Cprofile-generate=/tmp/shop-profile
        "$gen" --seed 7 --events 500000 --clicks 125000 --output /tmp/training
        DATA_DIR=/tmp/training OUTPUT_DIR=/tmp/training-out "$CARGO_TARGET_DIR/release/shopping-sessions"
        host=$(rustc -vV | sed -n 's/^host: //p')
        "$(rustc --print sysroot)/lib/rustlib/$host/bin/llvm-profdata" merge /tmp/shop-profile -o /tmp/trained.profdata
        cargo rustc --release --locked -- -Cprofile-use=/tmp/trained.profdata
    else
        cargo build --release --locked
    fi
    cp "$CARGO_TARGET_DIR/release/shopping-sessions" "/tmp/$v"
done
measure() {
  OUTPUT_DIR="/tmp/out-$1" /usr/bin/time -a -o /tmp/timing -f "$seed $1 %e %M" "/tmp/$1"
}
for seed in 42 98; do
    "$gen" --seed "$seed" --events 20000000 --clicks 5000000 --output "$DATA_DIR"
    OUTPUT_DIR=/tmp/out-base /tmp/base
    for round in 0 1 2; do
        for n in 0 1 2 3 4 5 6; do measure "${variants[$(((n+round*3)%7))]}"; done
    done
    for v in "${variants[@]:1}"; do python scripts/compare.py /tmp/out-base "/tmp/out-$v"; done
    rm -rf "$DATA_DIR"
done
winner=$(python - <<'PY'
from collections import defaultdict
from statistics import median
data=defaultdict(list)
for line in open('/tmp/timing'):
    seed,v,t,r=line.split()
    data[seed,v].append(float(t))
print(min(['base','count','batches','mapped','row','tuned','pgo'],key=lambda v: sum(median(data[s,v])/median(data[s,'base']) for s in ('42','98'))))
PY
)
echo "WINNER $winner"
seed=99
"$gen" --seed 99 --events 60000000 --clicks 15000000 --output "$DATA_DIR"
OUTPUT_DIR=/tmp/out-base /tmp/base
for v in base "$winner" "$winner" base base "$winner"; do measure "$v"; done
python scripts/compare.py /tmp/out-base "/tmp/out-$winner"
rm -rf "$DATA_DIR"
"$gen" --seed 1 --events 1000000 --clicks 250000 --output "$DATA_DIR"
OUTPUT_DIR=/tmp/public "/tmp/$winner"
python bench/digest.py /tmp/public > /tmp/dev-digest
diff bench/dev-digest /tmp/dev-digest
python scripts/run_fixtures.py fixtures -- "/tmp/$winner"
cat /tmp/timing
