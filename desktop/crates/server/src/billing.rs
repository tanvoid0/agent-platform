//! Regional price catalog. Amounts differ by purchasing power; they are not
//! FX of one USD price. Region is decided from card country and billing
//! country — never from IP.

use serde::Serialize;

use crate::env_opt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Region {
    Bd,
    Row,
    Gb,
    Us,
}

impl Region {
    pub fn as_str(self) -> &'static str {
        match self {
            Region::Us => "US",
            Region::Gb => "GB",
            Region::Bd => "BD",
            Region::Row => "ROW",
        }
    }

    pub fn from_country(iso2: &str) -> Self {
        match iso2.trim().to_ascii_uppercase().as_str() {
            "US" | "USA" | "PR" | "GU" | "VI" | "AS" | "MP" => Region::Us,
            "GB" | "UK" | "GG" | "JE" | "IM" => Region::Gb,
            "BD" => Region::Bd,
            _ if iso2.trim().is_empty() => Region::Row,
            _ => Region::Row,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PriceQuote {
    pub region: String,
    pub currency: String,
    pub amount_minor: i64,
    pub stripe_price_id: Option<String>,
}

/// Higher of card-issuing country vs billing-address country.
pub fn resolve_region(card_country: Option<&str>, billing_country: Option<&str>) -> Region {
    let card = card_country.filter(|s| !s.trim().is_empty()).map(Region::from_country);
    let billing = billing_country.filter(|s| !s.trim().is_empty()).map(Region::from_country);
    match (card, billing) {
        (Some(a), Some(b)) => a.max(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => Region::Row,
    }
}

pub fn quote(card_country: Option<&str>, billing_country: Option<&str>) -> PriceQuote {
    let region = resolve_region(card_country, billing_country);
    PriceQuote {
        region: region.as_str().to_string(),
        currency: currency_for(region).to_string(),
        amount_minor: amount_for(region),
        stripe_price_id: price_id_for(region),
    }
}

fn currency_for(region: Region) -> &'static str {
    match region {
        Region::Gb => "gbp",
        Region::Us | Region::Bd | Region::Row => "usd",
    }
}

/// Minor units (cents / pence). Override with `AGENT_PLATFORM_AMOUNT_{US,GB,BD,ROW}`.
fn amount_for(region: Region) -> i64 {
    let (var, default) = match region {
        Region::Us => ("AGENT_PLATFORM_AMOUNT_US", 2000),
        Region::Gb => ("AGENT_PLATFORM_AMOUNT_GB", 1600),
        Region::Bd => ("AGENT_PLATFORM_AMOUNT_BD", 400),
        Region::Row => ("AGENT_PLATFORM_AMOUNT_ROW", 1200),
    };
    env_opt(var).and_then(|s| s.parse().ok()).filter(|n| *n > 0).unwrap_or(default)
}

pub fn price_id_for(region: Region) -> Option<String> {
    let var = match region {
        Region::Us => "AGENT_PLATFORM_PRICE_US",
        Region::Gb => "AGENT_PLATFORM_PRICE_GB",
        Region::Bd => "AGENT_PLATFORM_PRICE_BD",
        Region::Row => "AGENT_PLATFORM_PRICE_ROW",
    };
    env_opt(var)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn us_card_plus_bd_address_is_us() {
        let q = quote(Some("US"), Some("BD"));
        assert_eq!(q.region, "US");
        assert_eq!(q.currency, "usd");
    }

    #[test]
    fn bd_card_and_bd_address_is_bd() {
        let q = quote(Some("BD"), Some("BD"));
        assert_eq!(q.region, "BD");
    }

    #[test]
    fn missing_sides_use_the_other() {
        assert_eq!(quote(Some("GB"), None).region, "GB");
        assert_eq!(quote(None, Some("US")).region, "US");
        assert_eq!(quote(None, None).region, "ROW");
    }
}
