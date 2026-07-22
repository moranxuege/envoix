use std::collections::HashMap;

use envoix_types::{LandedName, OfferedName, Secret};

#[test]
fn no_secret_reaches_display() {
    let secret = Secret::new("sentinel-shared-token");

    let debug = format!("{secret:?}");
    let display = format!("{secret}");

    assert_eq!(debug, "Secret([redacted])");
    assert_eq!(display, "Secret([redacted])");
    assert!(!debug.contains(secret.expose()));
    assert!(!display.contains(secret.expose()));
}

#[test]
fn offered_vs_landed_name_typed() {
    let offered = OfferedName::from_untrusted("../../etc/passwd");
    let landed = LandedName::new("passwd (1)");

    assert_eq!(offered.as_str(), "passwd");
    assert_eq!(landed.as_str(), "passwd (1)");
    assert_ne!(offered.as_str(), landed.as_str());

    let mut records_by_offered_name = HashMap::new();
    records_by_offered_name.insert(offered.clone(), "record-7");
    assert_eq!(
        records_by_offered_name.get(&OfferedName::from_untrusted("passwd")),
        Some(&"record-7")
    );

    assert_eq!(OfferedName::from_untrusted("..").as_str(), "unnamed");
    assert_eq!(
        OfferedName::from_untrusted(r"C:\provider\photo.jpg").as_str(),
        "photo.jpg"
    );
    assert!(serde_json::from_str::<OfferedName>(r#""../passwd""#).is_err());
}
