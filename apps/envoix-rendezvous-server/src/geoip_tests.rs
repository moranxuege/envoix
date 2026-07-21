use super::annotate;

#[test]
fn annotate_combines_location_and_isp() {
    assert_eq!(
        annotate(
            Some("Shanghai, CN".into()),
            Some("China Mobile (AS9808)".into())
        ),
        Some("Shanghai, CN · China Mobile (AS9808)".into())
    );
}

#[test]
fn annotate_falls_back_to_whichever_is_present() {
    assert_eq!(annotate(Some("CA, US".into()), None), Some("CA, US".into()));
    assert_eq!(
        annotate(None, Some("Comcast (AS7922)".into())),
        Some("Comcast (AS7922)".into())
    );
    assert_eq!(annotate(None, None), None);
}

// Manual, real-database check: `ENVOIX_GEOIP_ASN=<db.mmdb> cargo test -p
// envoix-rendezvous-server -- --ignored --nocapture describe_real_ips`.
#[test]
#[ignore = "requires ENVOIX_GEOIP_ASN pointing at a real .mmdb"]
fn describe_real_ips() {
    use super::GeoIp;
    use std::path::Path;
    let db = std::env::var("ENVOIX_GEOIP_ASN").expect("set ENVOIX_GEOIP_ASN");
    let city = std::env::var("ENVOIX_GEOIP_CITY").ok();
    let geo = GeoIp::load(city.as_deref().map(Path::new), Some(Path::new(&db)))
        .unwrap()
        .unwrap();
    for ip in ["117.135.95.10", "73.47.70.209"] {
        println!("{ip} -> {:?}", geo.describe(ip.parse().unwrap()));
    }
    assert!(geo.describe("117.135.95.10".parse().unwrap()).is_some());
}
