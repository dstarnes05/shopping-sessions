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
- Scan events and clicks in parallel using newline-aligned byte ranges. On AVX2-capable x86-64 hosts, classify commas, quotes and newlines in 32-byte blocks. Encode common IDs reversibly in 64 bits and intern other byte strings. Transfer batches of compact event records to event-ID partitions. Count hash-slot occupancy to prove which IDs occur once, then resolve all collisions by sorting full IDs and selecting the earliest copy, including invalid copies.
- Group surviving events and valid clicks by canonical person, merge the sorted person groups, and sweep the inclusive seven-day click window using per-campaign counts and a list of active campaigns.
- Aggregate campaign-day results per shard and merge them. Each writer handles eight ownership partitions, producing up to 32 Snappy-compressed user-day files with bounded row groups.

Deduplication and person groups stay in memory. The binary reports elapsed time for the event scan, deduplication and grouping, and session aggregation with Parquet output.

Set `PROFILE_OUTPUT=1` to measure grouping and session aggregation, user-row preparation, and Parquet encoding and writing. The user-table measurements sum elapsed time across workers. Campaign output and the full output phase report wall time. Leave profiling off for timing comparisons.
