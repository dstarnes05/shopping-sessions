use crate::Candidate;

// If a slot was visited once, the corresponding full ID occurred once too.
// Resolve every collision (including true duplicates) using the original keys.
pub fn winners(chunks: Vec<Vec<Candidate>>, mut emit: impl FnMut(&Candidate)) {
    let count: usize = chunks.iter().map(Vec::len).sum();
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
    for record in chunks.iter().flatten() {
        let (word, bit) = slot(record.id);
        repeated[word] |= seen[word] & bit;
        seen[word] |= bit;
    }

    let mut unresolved = Vec::new();
    for record in chunks.into_iter().flatten() {
        let (word, bit) = slot(record.id);
        if repeated[word] & bit == 0 {
            emit(&record);
        } else {
            unresolved.push(record);
        }
    }
    unresolved.sort_unstable_by_key(|record| record.id);
    for copies in unresolved.chunk_by(|a, b| a.id == b.id) {
        emit(copies.iter().min_by_key(|record| record.ingest).unwrap());
    }
}
