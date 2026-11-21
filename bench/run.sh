set -eu
export NCPU="$(nproc)" DATA_DIR=/tmp/shopdata CARGO_TARGET_DIR=/tmp/shopbuild
gen=bench/gen/target/release/generate
variants=(scored block filter dense fast)
for v in "${variants[@]}"; do
    git restore --source HEAD src
    case "$v" in
      scored) git restore --source 8ee81c78eac1b41054d53c209f7ece3a04df8f8f src ;;
      block) ;;
      filter|dense) cp bench/"$v".rs src/main.rs; cp bench/dedup.rs src/ ;;
      fast) cp bench/{main,ids,dedup}.rs src/ ;;
    esac
    cargo build --release --locked
    cp "$CARGO_TARGET_DIR/release/shopping-sessions" "/tmp/$v"
done
measure() {
  OUTPUT_DIR="/tmp/out-$1" /usr/bin/time -a -o /tmp/timing -f "$seed $1 %e %M" "/tmp/$1"
  tail -1 /tmp/timing
}
for seed in 42 98; do
    "$gen" --seed "$seed" --events 20000000 --clicks 5000000 --output "$DATA_DIR"
    OUTPUT_DIR=/tmp/out-scored /tmp/scored
    for round in 0 1 2; do
        for n in 0 1 2 3 4; do measure "${variants[$(((n+round*2)%5))]}"; done
    done
    for v in block filter dense fast; do python scripts/compare.py /tmp/out-scored "/tmp/out-$v"; done
    rm -rf "$DATA_DIR"
done
winner=$(python - <<'PY'
from collections import defaultdict
from statistics import median
data=defaultdict(list)
for line in open('/tmp/timing'):
    seed,v,t,r=line.split()
    data[seed,v].append(float(t))
print(min(['block','filter','dense','fast'],key=lambda v: sum(median(data[s,v])/median(data[s,'scored']) for s in ('42','98'))))
PY
)
echo "WINNER $winner"
seed=99
"$gen" --seed "$seed" --events 60000000 --clicks 15000000 --output "$DATA_DIR"
OUTPUT_DIR=/tmp/out-scored /tmp/scored
for v in scored block "$winner" "$winner" block scored block scored "$winner"; do measure "$v"; done
python scripts/compare.py /tmp/out-scored "/tmp/out-$winner"
rm -rf "$DATA_DIR"
"$gen" --seed 1 --events 1000000 --clicks 250000 --output "$DATA_DIR"
OUTPUT_DIR=/tmp/public "/tmp/$winner"
python bench/digest.py /tmp/public > /tmp/dev-digest
diff bench/dev-digest /tmp/dev-digest
python scripts/run_fixtures.py fixtures -- "/tmp/$winner"
cat /tmp/timing
