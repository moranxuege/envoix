use super::{MAX_CHUNK_SIZE, MIN_CHUNK_SIZE, validate_chunk_size};

#[test]
fn accepts_in_range_powers_of_two() {
    assert!(validate_chunk_size(MIN_CHUNK_SIZE).is_ok());
    assert!(validate_chunk_size(64 * 1024).is_ok());
    assert!(validate_chunk_size(MAX_CHUNK_SIZE).is_ok());
}

#[test]
fn rejects_out_of_range_or_non_power_of_two() {
    assert!(validate_chunk_size(MIN_CHUNK_SIZE - 1).is_err());
    assert!(validate_chunk_size(MAX_CHUNK_SIZE * 2).is_err());
    assert!(validate_chunk_size(24 * 1024).is_err()); // in range, not a power of two
}
