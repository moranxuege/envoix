use super::*;

fn addrs(list: &[&str]) -> Vec<SocketAddr> {
    list.iter().map(|s| s.parse().unwrap()).collect()
}

#[test]
fn empty_filter_keeps_everything() {
    let f = CandidateFilter::default();
    assert!(f.is_empty());
    let a = addrs(&["10.0.0.1:1", "1.2.3.4:2"]);
    assert_eq!(f.apply(a.clone()), a);
}

#[test]
fn deny_removes_matching_cidrs() {
    let f =
        CandidateFilter::from_lists(&[], &["10.0.0.0/8".into(), "fe80::/10".into()]).unwrap();
    let kept = f.apply(addrs(&[
        "10.0.0.1:1",
        "1.2.3.4:2",
        "[fe80::1]:3",
        "[2409:8a1e::1]:4",
    ]));
    assert_eq!(kept, addrs(&["1.2.3.4:2", "[2409:8a1e::1]:4"]));
}

#[test]
fn allow_is_an_inclusion_whitelist() {
    // Only the CN2-routed interface's addresses survive.
    let f = CandidateFilter::from_lists(&["203.0.113.0/24".into()], &[]).unwrap();
    let kept = f.apply(addrs(&["203.0.113.9:1", "1.2.3.4:2", "10.0.0.1:3"]));
    assert_eq!(kept, addrs(&["203.0.113.9:1"]));
}

#[test]
fn deny_still_applies_within_allow() {
    let f = CandidateFilter::from_lists(&["203.0.113.0/24".into()], &["203.0.113.9/32".into()])
        .unwrap();
    let kept = f.apply(addrs(&["203.0.113.9:1", "203.0.113.10:2"]));
    assert_eq!(kept, addrs(&["203.0.113.10:2"]));
}

#[test]
fn bare_ip_is_a_host_route() {
    let f = CandidateFilter::from_lists(&[], &["2.0.0.1".into()]).unwrap();
    let kept = f.apply(addrs(&["2.0.0.1:1", "2.0.0.2:2"]));
    assert_eq!(kept, addrs(&["2.0.0.2:2"]));
}

#[test]
fn rejects_garbage_cidr() {
    assert!(CandidateFilter::from_lists(&["not-a-cidr".into()], &[]).is_err());
}
