use crate::{Candidate, Event};

const SIDE: u32 = 1 << 31;
const INVALID: u32 = u32::MAX;

struct Packed {
    id: u64,
    user: u64,
    ingest: u32,
    time: u32,
}

#[derive(Default)]
pub struct Batch {
    records: Vec<Packed>,
    extra: Vec<(i64, Event)>,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn push(&mut self, start: i64, row: Candidate) {
        // Ingest timestamps and the manifest start have whole-second precision.
        let seconds = (row.ingest - start) / 1000 + i64::from(SIDE);
        let elapsed = row.event.time.saturating_sub(start);
        let time = if u32::try_from(seconds).is_ok() && row.event.time == i64::MIN {
            INVALID
        } else if u32::try_from(seconds).is_ok()
            && (0..i64::from(SIDE)).contains(&elapsed)
            && row.event.revenue == 0
        {
            elapsed as u32
        } else {
            // Each builder only contains rows from one bounded input chunk.
            let index = self.extra.len() as u32;
            self.extra.push((row.ingest, row.event));
            SIDE | index
        };
        self.records.push(Packed {
            id: row.id,
            user: row.user,
            ingest: seconds as u32,
            time,
        });
    }
}

impl Packed {
    fn expand(self, start: i64, extra: &[(i64, Event)]) -> Candidate {
        let (ingest, event) = if self.time >= SIDE && self.time != INVALID {
            extra[(self.time ^ SIDE) as usize]
        } else {
            let time = if self.time == INVALID {
                i64::MIN
            } else {
                start + i64::from(self.time)
            };
            (
                start + (i64::from(self.ingest) - i64::from(SIDE)) * 1000,
                Event {
                    time,
                    revenue: 0,
                    category: 0,
                },
            )
        };
        Candidate {
            id: self.id,
            user: self.user,
            ingest,
            event,
        }
    }
}

// If a slot was visited once, the corresponding full ID occurred once too.
// Resolve every collision (including true duplicates) using the original keys.
pub fn winners(chunks: Vec<Batch>, start: i64, mut emit: impl FnMut(&Candidate)) {
    let count: usize = chunks.iter().map(|b| b.records.len()).sum();
    if count == 0 {
        return;
    }
    let words = count.div_ceil(8).next_power_of_two();
    let shift = 58 - words.trailing_zeros();
    let slot = |id: u64| {
        // The first eight hash bits have already selected the partition.
        let hash = id.wrapping_mul(0x9e3779b97f4a7c15).rotate_left(8);
        let index = (hash >> shift) as usize;
        (index / 64, 1u64 << (index % 64))
    };
    let mut seen = vec![0u64; words];
    let mut repeated = vec![0u64; words];
    for record in chunks.iter().flat_map(|b| &b.records) {
        let (word, bit) = slot(record.id);
        repeated[word] |= seen[word] & bit;
        seen[word] |= bit;
    }

    let mut unresolved = Vec::new();
    for batch in chunks {
        for packed in batch.records {
            let record = packed.expand(start, &batch.extra);
            let (word, bit) = slot(record.id);
            if repeated[word] & bit == 0 {
                emit(&record);
            } else {
                unresolved.push(record);
            }
        }
    }
    unresolved.sort_unstable_by_key(|record| record.id);
    for copies in unresolved.chunk_by(|a, b| a.id == b.id) {
        emit(copies.iter().min_by_key(|record| record.ingest).unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Event;

    #[test]
    fn packed_rows_round_trip_at_encoding_boundaries() {
        assert_eq!(std::mem::size_of::<Packed>(), 24);
        let start = 1_790_000_000_000;
        for seconds in [
            -2_147_483_649i64,
            -2_147_483_648,
            0,
            2_147_483_647,
            2_147_483_648,
        ] {
            let ingest = start + seconds * 1000;
            for time in [
                i64::MIN,
                start,
                start + i64::from(SIDE) - 1,
                start + i64::from(SIDE),
            ] {
                for revenue in [0, 10_000_000] {
                    if revenue != 0 && time == i64::MIN {
                        continue;
                    }
                    let category = if revenue > 0 { 17 } else { 0 };
                    let event = Event {
                        time,
                        revenue,
                        category,
                    };
                    let mut batch = Batch::default();
                    batch.push(
                        start,
                        Candidate {
                            id: u64::MAX,
                            user: 19,
                            ingest,
                            event,
                        },
                    );
                    let row = batch.records.pop().unwrap().expand(start, &batch.extra);
                    assert_eq!((row.id, row.user, row.ingest), (u64::MAX, 19, ingest));
                    assert_eq!(
                        (row.event.time, row.event.revenue, row.event.category),
                        (time, revenue, category)
                    );
                }
            }
        }
    }

    #[test]
    fn colliding_slots_and_duplicates_across_chunks_keep_exact_winners() {
        // For this six-record input, the slot is the next six hash bits.
        let slot = |id: u64| id.wrapping_mul(0x9e3779b97f4a7c15).rotate_left(8) >> 58;
        let first = 1u64;
        let collision = (2..).find(|&id| slot(id) == slot(first)).unwrap();
        let other = (2..).find(|&id| slot(id) != slot(first)).unwrap();
        let row = |id, ingest, time| Candidate {
            id,
            ingest,
            user: id,
            event: Event {
                time,
                revenue: 0,
                category: 0,
            },
        };
        let chunks = vec![
            vec![row(first, 5, 10), row(collision, 8, 20)],
            vec![row(other, 2, 30), row(first, 1, i64::MIN)],
            vec![row(collision, 7, 25), row(first, 4, 15)],
        ];
        let mut got = Vec::new();
        let chunks = chunks
            .into_iter()
            .map(|c| {
                let mut b = Batch::default();
                for mut r in c {
                    r.ingest *= 1000;
                    b.push(0, r);
                }
                b
            })
            .collect();
        winners(chunks, 0, |r| {
            got.push((r.id, r.ingest / 1000, r.event.time))
        });
        let mut expected = vec![(first, 1, i64::MIN), (collision, 7, 25), (other, 2, 30)];
        got.sort_unstable();
        expected.sort_unstable();
        assert_eq!(got, expected);
    }
}
