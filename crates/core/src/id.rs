//! Random identifiers and secrets.

const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    if getrandom::fill(&mut buf).is_err() {
        // Extremely unlikely; fall back to hashing the clock with a
        // randomly seeded hasher so we never panic on id creation.
        use std::hash::{BuildHasher, Hasher};
        let state = std::collections::hash_map::RandomState::new();
        for chunk in buf.chunks_mut(8) {
            let mut h = state.build_hasher();
            h.write_u128(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default());
            let v = h.finish().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
    buf
}

/// Lowercase Crockford-style base32 string of `len` characters.
pub fn random_string(len: usize) -> String {
    random_bytes(len)
        .into_iter()
        .map(|b| ALPHABET[(b & 31) as usize] as char)
        .collect()
}

/// A new record id, e.g. `t-3k9x0q2m7d1a`.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", random_string(12))
}

/// A pairing token that is easy to type on a phone: `K7QF-2MZD-9XPA-T4RH`.
pub fn pairing_token() -> String {
    let s = random_string(16).to_uppercase();
    format!("{}-{}-{}-{}", &s[0..4], &s[4..8], &s[8..12], &s[12..16])
}
