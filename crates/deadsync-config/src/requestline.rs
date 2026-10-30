//! requestline: song requests through ArrowCloud. Machine-wide, off by default.
use crate::ini::SimpleIni;
use crate::options::load_bool_option;
use crate::writer::{push_bool, push_line, push_section};

/// Which player's ArrowCloud profile requestline uses on this machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RequestlineSide {
    #[default]
    P1,
    P2,
}

impl RequestlineSide {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::P1 => "P1",
            Self::P2 => "P2",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "p1" | "1" => Some(Self::P1),
            "p2" | "2" => Some(Self::P2),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequestlineOptions {
    pub enabled: bool,
    pub side: RequestlineSide,
}

const SECTION: &str = "Requestline";

pub fn load(conf: &SimpleIni, default: RequestlineOptions) -> RequestlineOptions {
    RequestlineOptions {
        enabled: load_bool_option(conf, SECTION, "Enabled", default.enabled),
        side: conf
            .get(SECTION, "Profile")
            .and_then(RequestlineSide::parse)
            .unwrap_or(default.side),
    }
}

pub fn push_options(content: &mut String, options: RequestlineOptions) {
    push_section(content, "[Requestline]");
    push_bool(content, "Enabled", options.enabled);
    push_line(content, "Profile", options.side.as_str());
    content.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requestline_preferences_round_trip_and_default_off() {
        let mut ini = SimpleIni::new();
        assert_eq!(
            load(&ini, RequestlineOptions::default()),
            RequestlineOptions::default()
        );
        assert!(!RequestlineOptions::default().enabled);
        let options = RequestlineOptions {
            enabled: true,
            side: RequestlineSide::P2,
        };
        let cfg = crate::app_config::Config {
            requestline: options,
            ..Default::default()
        };
        let saved = crate::save::build_saved_app_config_file(
            &cfg,
            &deadsync_input::Keymap::default(),
            "default",
            &[],
            &[],
            "",
            "",
            "",
            "",
        );
        ini.load_str(&saved);
        assert_eq!(
            crate::load::load_app_config(&ini, Default::default()).requestline,
            options
        );
        assert_eq!(RequestlineSide::parse(" p1 "), Some(RequestlineSide::P1));
        assert_eq!(RequestlineSide::parse("P3"), None);
    }
}
