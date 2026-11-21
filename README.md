# Shopping Sessions

Rust pipeline for the Shopping Sessions hackathon. It reads the mounted CSVs, resolves linked identities, deduplicates events, and computes the two daily Parquet tables.

## Run locally

Build with Rust 1.87 or newer. The supplied verification scripts also require Python and Polars.

```sh
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
cargo build --release --locked
NCPU=8 .venv/bin/python scripts/run_fixtures.py fixtures -- target/release/shopping-sessions
```

Generate the public dataset using the challenge generator. Unpack the public answer separately.

```sh
generate --seed 1 --events 1000000 --clicks 250000 --output dev
DATA_DIR=dev OUTPUT_DIR=out NCPU=8 target/release/shopping-sessions
.venv/bin/python scripts/compare.py out dev-answer
```

Use an empty output directory for each run. The Docker image runs as UID 1000 with `/data` and `/output` as the default paths. Use `make build` to build linux/amd64 and `make fixtures` or `make run-dev` to exercise the image under the scorer-style restrictions. Activate the virtual environment before running the fixture target.

## Pipeline

- Resolve linked identities with union-find and preserve the byte-wise minimum ID.
- Scan events and clicks in parallel using newline-aligned byte ranges. Encode common IDs reversibly in 64 bits and intern other byte strings. Partition compact event records by event ID and sort each partition to select the earliest copy, including invalid copies.
- Group surviving events and valid clicks by canonical person, sort each person's records, and sweep the inclusive seven-day click window as sessions close.
- Aggregate campaign-day results per shard and merge them. Write user-day results in bounded Parquet row groups.

Deduplication and person groups stay in memory. The binary reports elapsed time for the event scan, deduplication and grouping, and session aggregation with Parquet output.
