use super::*;

#[test]
fn defaults_resume_on_auto_path_no_relay() {
    let options = TransferOptions::default();
    assert!(options.resume);
    assert_eq!(options.path, PathPolicy::Auto);
    assert_eq!(options.relay, None);
    assert_eq!(options.listen_addrs, None);
}
