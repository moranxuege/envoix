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
#[path = "geoip_tests.rs"]
mod tests;
