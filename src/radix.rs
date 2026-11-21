use crate::Candidate;

// The upper eight hash bits already select the owning partition. Refine by
// the next byte, then sort cache-sized buckets by exact ID.
pub fn group(records: &mut [Candidate]) {
    if records.len() < 2048 {
        records.sort_unstable_by_key(|r| r.id);
        return;
    }
    let bucket = |id: u64| ((id.wrapping_mul(0x9e3779b97f4a7c15) >> 48) & 255) as usize;
    let mut ends = [0usize; 256];
    for r in records.iter() {
        ends[bucket(r.id)] += 1;
    }
    let mut next = [0usize; 256];
    let mut offset = 0;
    for i in 0..256 {
        next[i] = offset;
        offset += ends[i];
        ends[i] = offset;
    }
    let mut start = 0;
    for i in 0..256 {
        while next[i] < ends[i] {
            let pos = next[i];
            let dest = bucket(records[pos].id);
            records.swap(pos, next[dest]);
            next[dest] += 1;
        }
        records[start..ends[i]].sort_unstable_by_key(|r| r.id);
        start = ends[i];
    }
}
