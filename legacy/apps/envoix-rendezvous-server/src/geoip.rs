//! Optional GeoIP annotation of peer addresses for the broker log.
//!
//! The operator supplies MaxMind GeoLite2 or DB-IP Lite `.mmdb` files (both use
//! the MaxMind DB format); this crate reads them offline - no external service
//! is ever queried. When no database is configured, annotation is skipped.

use std::net::IpAddr;
use std::path::Path;

use anyhow::{Context, Result};
use maxminddb::{Reader, geoip2};

/// Loaded GeoIP databases. Both are optional and independent.
pub struct GeoIp {
    city: Option<Reader<Vec<u8>>>,
    asn: Option<Reader<Vec<u8>>>,
}

impl GeoIp {
    /// Load the given databases. Returns `None` when neither path is set, so the
    /// caller skips annotation entirely.
    pub fn load(city: Option<&Path>, asn: Option<&Path>) -> Result<Option<Self>> {
        if city.is_none() && asn.is_none() {
            return Ok(None);
        }
        let city = open(city, "city")?;
        let asn = open(asn, "ASN")?;
        Ok(Some(Self { city, asn }))
    }

    /// A human location + ISP string for `ip`, e.g.
    /// `Shanghai, CN · China Mobile Communications (AS9808)`, or `None` when
    /// neither database has data for the address.
    pub fn describe(&self, ip: IpAddr) -> Option<String> {
        let location = self.city.as_ref().and_then(|reader| location(reader, ip));
        let isp = self.asn.as_ref().and_then(|reader| isp(reader, ip));
        annotate(location, isp)
    }
}

fn open(path: Option<&Path>, kind: &str) -> Result<Option<Reader<Vec<u8>>>> {
    path.map(|path| {
        Reader::open_readfile(path)
            .with_context(|| format!("opening GeoIP {kind} database {}", path.display()))
    })
    .transpose()
}

/// `"{city or region}, {country}"` from a City database.
fn location(reader: &Reader<Vec<u8>>, ip: IpAddr) -> Option<String> {
    let city: geoip2::City = reader.lookup(ip).ok()?.decode().ok()??;
    let place = city.city.names.english.or_else(|| {
        city.subdivisions
            .first()
            .and_then(|sub| sub.names.english.or(sub.iso_code))
    });
    match (place, city.country.iso_code) {
        (Some(place), Some(country)) => Some(format!("{place}, {country}")),
        (Some(place), None) => Some(place.to_string()),
        (None, Some(country)) => Some(country.to_string()),
        (None, None) => None,
    }
}

/// `"{org} (AS{n})"` from an ASN database.
fn isp(reader: &Reader<Vec<u8>>, ip: IpAddr) -> Option<String> {
    let asn: geoip2::Asn = reader.lookup(ip).ok()?.decode().ok()??;
    let org = asn.autonomous_system_organization?;
    match asn.autonomous_system_number {
        Some(number) => Some(format!("{org} (AS{number})")),
        None => Some(org.to_string()),
    }
}

/// Join location and ISP into one annotation, or `None` if both are absent.
fn annotate(location: Option<String>, isp: Option<String>) -> Option<String> {
    match (location, isp) {
        (Some(location), Some(isp)) => Some(format!("{location} · {isp}")),
        (Some(one), None) | (None, Some(one)) => Some(one),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
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
}
