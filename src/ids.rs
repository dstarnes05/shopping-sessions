//! A reversible encoding for common nine-byte IDs, plus an exact dictionary
//! for all other byte strings. Each column has its own namespace.
use std::{collections::HashMap, sync::Mutex};

const INTERNED: u64 = 1 << 63;

pub struct Ids {
    prefix: u8,
    other: Mutex<(HashMap<Box<[u8]>, u64>, Vec<Box<[u8]>>)>,
}

impl Ids {
    pub fn new(prefix: u8) -> Self {
        Self {
            prefix,
            other: Default::default(),
        }
    }

    pub fn encode(&self, bytes: &[u8]) -> u64 {
        if bytes.len() == 9 && bytes[0] == self.prefix && bytes[1] < 128 {
            return u64::from_be_bytes(bytes[1..].try_into().unwrap());
        }
        let mut dictionary = self.other.lock().unwrap();
        let (keys, names) = &mut *dictionary;
        if let Some(&key) = keys.get(bytes) {
            return key;
        }
        let key = INTERNED | names.len() as u64;
        keys.insert(bytes.into(), key);
        names.push(bytes.into());
        key
    }

    pub fn compare(&self, left: u64, right: u64) -> std::cmp::Ordering {
        if (left | right) & INTERNED == 0 {
            left.cmp(&right)
        } else {
            self.decode(left).cmp(&self.decode(right))
        }
    }

    pub fn decode(&self, key: u64) -> Box<[u8]> {
        if key & INTERNED != 0 {
            return self.other.lock().unwrap().1[(key ^ INTERNED) as usize].clone();
        }
        let mut name = [self.prefix; 9];
        name[1..].copy_from_slice(&key.to_be_bytes());
        name.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_and_interned_ids_preserve_bytes_and_order() {
        let ids = Ids::new(b'u');
        let names: &[&[u8]] = &[
            b"u12345678",
            b"U12345678",
            b"u",
            b"u123456789",
            b"u00000000",
            b"u\x801234567",
            b" u",
            b"u\0\0\0\0\0\0\0\0",
        ];
        for &left in names {
            let key = ids.encode(left);
            assert_eq!(&*ids.decode(key), left);
            for &right in names {
                assert_eq!(ids.compare(key, ids.encode(right)), left.cmp(right));
            }
        }
    }
}
