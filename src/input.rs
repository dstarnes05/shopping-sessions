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
