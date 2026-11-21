use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub fn files(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "csv") {
            paths.push(path);
        }
    }
    paths.sort_unstable();
    Ok(paths)
}

pub fn rows<const N: usize>(bytes: &[u8]) -> impl Iterator<Item = [&[u8]; N]> {
    bytes
        .split(|&byte| byte == b'\n')
        .skip(1)
        .filter_map(fields)
}

pub fn fields<const N: usize>(line: &[u8]) -> Option<[&[u8]; N]> {
    let mut parts = line.split(|&byte| byte == b',');
    let mut fields = [&[][..]; N];
    for field in &mut fields {
        let raw = parts.next()?;
        *field = if raw.first() == Some(&b'"') {
            raw.strip_prefix(b"\"")?.strip_suffix(b"\"")?
        } else {
            raw
        };
        if field.contains(&b'"') {
            return None;
        }
    }
    if line.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(fields)
}

pub fn decimal(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0_i64, |value, &byte| {
        if !byte.is_ascii_digit() {
            return None;
        }
        value.checked_mul(10)?.checked_add(i64::from(byte - b'0'))
    })
}

pub const DAY: i64 = 86_400_000;
pub const WEEK: i64 = 7 * DAY;

pub fn timestamp(bytes: &[u8]) -> Option<i64> {
    if bytes.len() != 20 && bytes.len() != 25 {
        return decimal(bytes);
    }
    if bytes[4] != b'-' {
        return decimal(bytes);
    }
    if bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year = decimal(&bytes[..4])?;
    let month = decimal(&bytes[5..7])?;
    let day = decimal(&bytes[8..10])?;
    let hour = decimal(&bytes[11..13])?;
    let minute = decimal(&bytes[14..16])?;
    let second = decimal(&bytes[17..19])?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&month)
        || day == 0
        || day > month_days[(month - 1) as usize]
        || hour >= 24
        || minute >= 60
        || second >= 60
    {
        return None;
    }
    let offset = match &bytes[19..] {
        b"Z" => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let hours = decimal(&[*h1, *h2])?;
            let minutes = decimal(&[*m1, *m2])?;
            if hours >= 24 || minutes >= 60 {
                return None;
            }
            (hours * 60 + minutes) * if *sign == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year.rem_euclid(400);
    let march_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * march_month + 2) / 5 + day - 1;
    let days = era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100
        + day_of_year
        - 719_468;
    Some(days * DAY + ((hour * 60 + minute - offset) * 60 + second) * 1000)
}

// Parse the common 13-digit epoch form. All other lengths use the full grammar.
fn epoch13(bytes: &[u8]) -> Option<i64> {
    let word = u64::from_le_bytes(bytes[..8].try_into().unwrap());
    const HIGH: u64 = 0xf0f0f0f0f0f0f0f0;
    const ASCII: u64 = 0x3030303030303030;
    if word & HIGH != ASCII || (word.wrapping_add(0x0606060606060606) & HIGH) != ASCII {
        return None;
    }
    let digits = word ^ ASCII;
    let pairs = (digits.wrapping_mul(10) + (digits >> 8)) & 0x00ff00ff00ff00ff;
    let quads = (pairs.wrapping_mul(100) + (pairs >> 16)) & 0x0000ffff0000ffff;
    let prefix = (quads & 0xffff) * 10000 + (quads >> 32);
    let mut value = prefix as i64;
    for &byte in &bytes[8..] {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + i64::from(byte - b'0');
    }
    Some(value)
}

const SCAN_CHUNK: u64 = 2 * 1024 * 1024;

pub fn scan_chunks(paths: &[PathBuf], visit: impl Fn(&[u8]) + Sync) -> crate::Result<()> {
    use rayon::prelude::*;

    let mut maps = Vec::new();
    for path in paths {
        let file = fs::File::open(path)?;
        if file.metadata()?.len() != 0 {
            // Scorer inputs are immutable for the lifetime of the run.
            maps.push(unsafe { memmap2::MmapOptions::new().map(&file)? });
        }
    }
    let chunks: Vec<_> = maps
        .iter()
        .flat_map(|map| {
            (0..map.len())
                .step_by(SCAN_CHUNK as usize)
                .map(move |start| (map, start))
        })
        .collect();
    chunks.into_par_iter().for_each(|(map, start)| {
        let end = (start + SCAN_CHUNK as usize).min(map.len());
        visit(data_range(map, start, end));
    });
    Ok(())
}

fn data_range(bytes: &[u8], start: usize, end: usize) -> &[u8] {
    // Byte zero belongs to the header. Other ranges own lines whose first
    // byte lies inside the range, including a final row without a newline.
    let boundary = |pos: usize| {
        let from = pos.saturating_sub(1);
        memchr::memchr(b'\n', &bytes[from..]).map_or(bytes.len(), |n| from + n + 1)
    };
    let first = boundary(start);
    let last = if end == bytes.len() {
        end
    } else {
        boundary(end)
    };
    &bytes[first..last]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_data_exactly_once() {
        let input =
            b"first,second\nleft,right\n\nvery long line across several ranges,x\nq,r\nfinal,row";
        let expected = &input[b"first,second\n".len()..];
        for chunk_size in 1..=input.len() {
            let mut collected = Vec::new();

            for start in (0..input.len()).step_by(chunk_size) {
                let end = (start + chunk_size).min(input.len());
                collected.extend_from_slice(data_range(input, start, end));
            }
            assert_eq!(collected, expected, "chunk size {chunk_size}");
        }
    }
}

pub fn data_rows<const N: usize>(chunk: &[u8]) -> impl Iterator<Item = [&[u8]; N]> {
    let mut start = 0;
    memchr::memchr_iter(b'\n', chunk)
        .chain(std::iter::once(chunk.len()))
        .filter_map(move |end| {
            let line = chunk.get(start..end)?;
            start = end + 1;
            fields(line)
        })
}

// Cache validated calendar dates within each chunk. A cache miss goes through
// the full timestamp grammar, including calendar and offset validation.
#[derive(Default)]
pub struct Dates {
    entries: [Option<([u8; 11], i64)>; 32],
}

impl Dates {
    pub fn parse(&mut self, value: &[u8]) -> Option<i64> {
        if value.len() == 13 {
            return epoch13(value);
        }
        if value.len() != 20 || value[19] != b'Z' {
            return timestamp(value);
        }
        let key: &[u8; 11] = value[..11].try_into().unwrap();
        let index = (usize::from(key[8]) * 10 + usize::from(key[9])) % 32;
        let midnight = match self.entries[index] {
            Some((date, midnight)) if date == *key => midnight,
            _ => {
                let time = timestamp(value)?;
                self.entries[index] = Some((*key, time.div_euclid(DAY) * DAY));
                return Some(time);
            }
        };
        fn pair(a: u8, b: u8) -> Option<i64> {
            (a.is_ascii_digit() && b.is_ascii_digit())
                .then(|| i64::from((a - b'0') * 10 + b - b'0'))
        }
        if value[13] != b':' || value[16] != b':' {
            return None;
        }
        let hour = pair(value[11], value[12])?;
        let minute = pair(value[14], value[15])?;
        let second = pair(value[17], value[18])?;
        (hour < 24 && minute < 60 && second < 60)
            .then(|| midnight + ((hour * 60 + minute) * 60 + second) * 1000)
    }
}

pub fn visit_rows<const N: usize>(chunk: &[u8], visit: impl FnMut([&[u8]; N])) {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("avx2") {
        // The runtime check satisfies visit's target-feature precondition.
        return unsafe { crate::block::visit(chunk, visit) };
    }
    data_rows::<N>(chunk).for_each(visit)
}

#[cfg(test)]
mod differential {
    use super::*;
    #[test]
    fn block_fields_match_reference() {
        // Shift the same quoted, malformed, empty and long fields across each SIMD lane.
        for shift in 0..96 {
            let mut data = vec![b'x'; shift];
            data.extend_from_slice(b",b,c,d\n\nae,b,c,d\n\"q\",\"\",c,d\na,\"b\"c,c,d\na,\"x\"\",c,d\na,,,\n,,,\na,\"b,c,d\n1,2,3,4\r\n");
            data.extend_from_slice(&[b'x'; 101]);
            data.extend_from_slice(b",two,three,\"four\"");
            for end in 0..=data.len() {
                let chunk = &data[..end];
                let expected: Vec<_> = data_rows::<4>(chunk).collect();
                let mut index = 0;
                visit_rows::<4>(chunk, |row| {
                    assert_eq!(expected[index], row);
                    index += 1;
                });
                assert_eq!(index, expected.len());
            }
        }
    }

    #[test]
    fn epoch_digits_match_generic_parser() {
        let mut value = *b"1791071890000";
        for index in 0..value.len() {
            let saved = value[index];
            for byte in 0..=255 {
                value[index] = byte;
                assert_eq!(Dates::default().parse(&value), timestamp(&value));
            }
            value[index] = saved;
        }
    }
}
