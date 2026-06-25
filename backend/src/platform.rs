//! Canonical messaging platforms (single source of truth). The DB and API use
//! the string form; parse at the boundary with `Platform::parse`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Line,
    Facebook,
    Instagram,
    Shopee,
}

impl Platform {
    pub const ALL: [Platform; 4] = [
        Platform::Line,
        Platform::Facebook,
        Platform::Instagram,
        Platform::Shopee,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Line => "line",
            Platform::Facebook => "facebook",
            Platform::Instagram => "instagram",
            Platform::Shopee => "shopee",
        }
    }

    pub fn parse(s: &str) -> Option<Platform> {
        match s {
            "line" => Some(Platform::Line),
            "facebook" => Some(Platform::Facebook),
            "instagram" => Some(Platform::Instagram),
            "shopee" => Some(Platform::Shopee),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_all_four() {
        for p in Platform::ALL {
            assert_eq!(Platform::parse(p.as_str()), Some(p));
        }
        assert_eq!(Platform::ALL.len(), 4);
    }

    #[test]
    fn as_str_values() {
        assert_eq!(Platform::Line.as_str(), "line");
        assert_eq!(Platform::Facebook.as_str(), "facebook");
        assert_eq!(Platform::Instagram.as_str(), "instagram");
        assert_eq!(Platform::Shopee.as_str(), "shopee");
    }

    #[test]
    fn unknown_and_whatsapp_are_none() {
        assert_eq!(Platform::parse("whatsapp"), None);
        assert_eq!(Platform::parse(""), None);
        assert_eq!(Platform::parse("LINE"), None); // case-sensitive, canonical only
    }
}
