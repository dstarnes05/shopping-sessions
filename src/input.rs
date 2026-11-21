use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
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

const SCAN_CHUNK: u64 = 2 * 1024 * 1024;

pub fn scan_chunks(paths: &[PathBuf], visit: impl Fn(&[u8]) + Sync) -> crate::Result<()> {
    use rayon::prelude::*;

    let mut chunks = Vec::new();
    for path in paths {
        let len = fs::metadata(path)?.len();
        for start in (0..len).step_by(SCAN_CHUNK as usize) {
            chunks.push((path, start, (start + SCAN_CHUNK).min(len)));
        }
    }
    chunks
        .into_par_iter()
        .try_for_each_init(Vec::new, |buffer, (path, start, end)| {
            let reader = BufReader::with_capacity(64 * 1024, fs::File::open(path)?);
            read_chunk(reader, start, end, buffer)?;
            visit(buffer);
            Ok(())
        })
}

fn read_chunk(
    mut reader: impl BufRead + Seek,
    start: u64,
    end: u64,
    buffer: &mut Vec<u8>,
) -> io::Result<()> {
    buffer.clear();
    let first = if start == 0 {
        // The first byte range owns the header, which is never a data row.
        reader.read_until(b'\n', buffer)? as u64
    } else {
        reader.seek(SeekFrom::Start(start - 1))?;
        // At a line boundary this consumes just the preceding newline.
        // Otherwise the range where the line began owns it.
        start - 1 + reader.read_until(b'\n', buffer)? as u64
    };
    buffer.clear();
    if first < end {
        reader.by_ref().take(end - first).read_to_end(buffer)?;
        if buffer.last().is_some_and(|&byte| byte != b'\n') {
            reader.read_until(b'\n', buffer)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn ranges_cover_data_exactly_once() {
        let input =
            b"first,second\nleft,right\n\nvery long line across several ranges,x\nq,r\nfinal,row";
        let expected = &input[b"first,second\n".len()..];
        for chunk_size in 1..=input.len() {
            let mut collected = Vec::new();
            let mut buffer = Vec::new();
            for start in (0..input.len()).step_by(chunk_size) {
                let end = (start + chunk_size).min(input.len());
                read_chunk(Cursor::new(input), start as u64, end as u64, &mut buffer).unwrap();
                collected.extend_from_slice(&buffer);
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
