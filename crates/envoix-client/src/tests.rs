use super::*;

#[test]
fn parses_human_readable_chunk_sizes() {
    assert_eq!(parse_chunk_size("16K").unwrap(), 16 * 1024);
    assert_eq!(parse_chunk_size("16KB").unwrap(), 16 * 1024);
    assert_eq!(parse_chunk_size("1M").unwrap(), 1024 * 1024);
    assert_eq!(parse_chunk_size("1MB").unwrap(), 1024 * 1024);
    assert_eq!(parse_chunk_size("16384B").unwrap(), 16 * 1024);
}

#[test]
fn rejects_bare_out_of_range_or_non_power_of_two_chunk_sizes() {
    assert!(matches!(
        parse_chunk_size("65536"),
        Err(CoreError::InvalidInput(_))
    ));
    assert!(matches!(
        parse_chunk_size("15K"),
        Err(CoreError::InvalidInput(_))
    ));
    assert!(matches!(
        parse_chunk_size("17M"),
        Err(CoreError::InvalidInput(_))
    ));
    assert!(matches!(
        parse_chunk_size("24K"),
        Err(CoreError::InvalidInput(_))
    ));
    assert!(matches!(
        parse_chunk_size("1MiB"),
        Err(CoreError::InvalidInput(_))
    ));
}

#[test]
fn parses_in_range_windows() {
    assert_eq!(parse_window("1MB").unwrap(), MIN_DATA_STREAM_WINDOW);
    assert_eq!(parse_window("32MB").unwrap(), 32 * 1024 * 1024);
    assert_eq!(parse_window("128MB").unwrap(), MAX_DATA_STREAM_WINDOW);
    // Unlike the chunk size, the window need not be a power of two.
    assert_eq!(parse_window("48MB").unwrap(), 48 * 1024 * 1024);
}

#[test]
fn rejects_out_of_range_or_unitless_windows() {
    // Below MIN, above MAX, and missing a unit are all rejected (never clamped).
    assert!(matches!(
        parse_window("512KB"),
        Err(CoreError::InvalidInput(_))
    ));
    assert!(matches!(
        parse_window("256MB"),
        Err(CoreError::InvalidInput(_))
    ));
    assert!(matches!(
        parse_window("16"),
        Err(CoreError::InvalidInput(_))
    ));
}
