// TODO: Actually benchmark whether these do anything.
//       RapidXML does use lookup tables too though.

#[cfg(test)] // not used outside tests
pub const RAPIDXML_WHITESPACE: &[u8] = b" \n\r\t";
// NOTE: ':' is additionally treated as unsupported.
//       This is to implement prefixed names, and is not how RapidXML works.
pub const RAPIDXML_INVALID_NAME: &[u8] = b" \n\r\t/>?\0:";
pub const RAPIDXML_INVALID_ATTRNAME: &[u8] = b" \n\r\t/<>=?!\0:";

// TODO: benchmark
#[inline]
pub fn is_whitespace(chr: u8) -> bool {
    const LUT: [u8; 8] = [b' ', b'\t', b'\n', 0, 0, b'\r', 0, 0];
    LUT[(chr & 0b111) as usize] == chr
}

const fn make_big_lut(values: &[u8]) -> [bool; 256] {
    let mut result = [false; 256];

    let mut i = 0;
    while i < values.len() {
        result[values[i] as usize] = true;
        i += 1;
    }

    result
}

pub fn is_invalid_name(chr: u8) -> bool {
    const LUT: [bool; 256] = make_big_lut(RAPIDXML_INVALID_NAME);
    LUT[chr as usize]
}

pub fn is_invalid_attribute_name(chr: u8) -> bool {
    const LUT: [bool; 256] = make_big_lut(RAPIDXML_INVALID_ATTRNAME);
    LUT[chr as usize]
}

#[cfg(test)]
mod test {
    fn test_lut_fn(truthy: &[u8], fun: impl Fn(u8) -> bool) {
        for chr in 0..u8::MAX {
            assert_eq!(fun(chr), truthy.contains(&chr))
        }
    }

    #[test]
    fn is_whitespace() {
        test_lut_fn(super::RAPIDXML_WHITESPACE, super::is_whitespace);
    }

    #[test]
    fn is_invalid_element_name() {
        test_lut_fn(super::RAPIDXML_INVALID_NAME, super::is_invalid_name);
    }

    #[test]
    fn is_invalid_attribute_name() {
        test_lut_fn(super::RAPIDXML_INVALID_ATTRNAME, super::is_invalid_attribute_name);
    }
}
