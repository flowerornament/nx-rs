#[must_use]
pub fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}
