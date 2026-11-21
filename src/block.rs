//! Parse newline-bounded CSV chunks using the challenge's restricted quoting grammar.
use std::arch::x86_64::*;

#[target_feature(enable = "avx2")]
pub unsafe fn visit<const N: usize>(chunk: &[u8], mut visit: impl FnMut([&[u8]; N])) {
    let mut fields = [&[][..]; N];
    let mut index = 0;
    let mut start = 0;
    let mut quotes = 0;
    let mut valid = true;
    let mut base = 0;
    while base <= chunk.len() {
        let remaining = chunk.len() - base;
        let mut tail = [0u8; 32];
        let ptr = if remaining < 32 {
            tail[..remaining].copy_from_slice(&chunk[base..]);
            tail[remaining] = b'\n';
            tail.as_ptr()
        } else {
            chunk[base..].as_ptr()
        };
        // Both paths provide 32 readable bytes. The tail's synthetic newline
        // terminates a final row when the input has no trailing newline.
        let bytes = _mm256_loadu_si256(ptr.cast());
        let eq = |b| _mm256_cmpeq_epi8(bytes, _mm256_set1_epi8(b));
        let newlines = _mm256_movemask_epi8(eq(10)) as u32;
        let mut delimiters = newlines | _mm256_movemask_epi8(eq(44)) as u32;
        let mut quote_mask = _mm256_movemask_epi8(eq(34)) as u32;
        while delimiters != 0 {
            let bit = delimiters & delimiters.wrapping_neg();
            let end = base + bit.trailing_zeros() as usize;
            let before = bit - 1;
            quotes += (quote_mask & before).count_ones() as usize;
            quote_mask &= !before;
            if index < N {
                let raw = &chunk[start..end];
                match quotes {
                    0 => fields[index] = raw,
                    2 if raw.len() >= 2 && raw[0] == b'"' && raw[raw.len() - 1] == b'"' => {
                        fields[index] = &raw[1..raw.len() - 1];
                    }
                    _ => valid = false,
                }
            }
            index += 1;
            start = end + 1;
            quotes = 0;
            if newlines & bit != 0 {
                if valid && index == N {
                    visit(fields);
                }
                valid = true;
                index = 0;
            }
            delimiters &= delimiters - 1;
        }
        quotes += quote_mask.count_ones() as usize;
        base += 32;
    }
}
