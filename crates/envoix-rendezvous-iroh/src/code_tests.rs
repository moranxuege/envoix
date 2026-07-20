use super::*;

#[test]
fn split_separates_nameplate_from_full_code() {
    let (room, password) = split_code("7831-amber-comet");
    assert_eq!(room, "7831");
    // The password is the WHOLE code, so the broker (which only sees the
    // room id) never learns the word entropy.
    assert_eq!(password, "7831-amber-comet");
}

#[test]
fn generated_code_round_trips_through_split() {
    let code = generate_code(2).unwrap();
    let (room, password) = split_code(&code);
    assert_eq!(password, code);
    assert!(code.starts_with(room));
    // "<6 digits>-word-word" -> 3 dash-separated parts.
    assert_eq!(code.split('-').count(), 3);
    assert_eq!(room.len(), 6);
    assert!(room.chars().all(|c| c.is_ascii_digit()));
}
