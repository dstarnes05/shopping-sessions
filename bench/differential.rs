
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
                assert_eq!(Dates::default().parse(&value),timestamp(&value));
            }
            value[index] = saved;
        }
    }
}
