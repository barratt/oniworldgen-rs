//! Port of KLEI's `Hash.SDBMLower(string)` — SDBM hash of the
//! lowercased input, returned as u32.

pub fn sdbm_lower(input: &str) -> u32 {
    let mut hash: u32 = 0;
    for c in input.chars() {
        for lc in c.to_lowercase() {
            let code = lc as u32;
            // hash = code + (hash << 6) + (hash << 16) - hash
            // Order matters due to wrapping.
            hash = code
                .wrapping_add(hash.wrapping_shl(6))
                .wrapping_add(hash.wrapping_shl(16))
                .wrapping_sub(hash);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values for the game's SDBMLower hash.
    #[test]
    fn spot_check() {
        assert_eq!(sdbm_lower(""), 0);
        assert_eq!(sdbm_lower("a"), 97);
        assert_eq!(sdbm_lower("abc"), 807_794_786);
        assert_eq!(sdbm_lower("hello"), 684_824_882);
        assert_eq!(sdbm_lower("traits/BouldersSmall"), 3_800_582_199);
        assert_eq!(sdbm_lower("ABC"), sdbm_lower("abc"));
    }
}
