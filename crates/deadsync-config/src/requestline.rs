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

/// Declares a small setting saved by name: its variants in menu order, the
/// first being the default.
macro_rules! named_choice {
    ($(#[$doc:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub enum $name {
            #[default]
            $($variant),+
        }

        impl $name {
            /// Every value, in the order the options row lists them.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            pub fn parse(raw: &str) -> Option<Self> {
                let raw = raw.trim();
                Self::ALL.iter().copied().find(|v| v.as_str().eq_ignore_ascii_case(raw))
            }

            pub fn index(self) -> usize {
                Self::ALL.iter().position(|v| *v == self).unwrap_or(0)
            }

            pub fn from_index(index: usize) -> Self {
                Self::ALL.get(index).copied().unwrap_or_default()
            }
        }
    };
}

named_choice! {
    /// The sound a new request makes. Off unless chosen.
    RequestlineSound {
        Off = "Off",
        Coin = "Coin",
        Chime = "Chime",
        DingDong = "DingDong",
        NeedleDrop = "NeedleDrop",
        Selector = "Selector",
    }
}

impl RequestlineSound {
    /// The sound effect to play, or `None` for Off.
    pub const fn asset_path(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Coin => Some("assets/sounds/requestline/coin.ogg"),
            Self::Chime => Some("assets/sounds/requestline/chime.ogg"),
            Self::DingDong => Some("assets/sounds/requestline/dingdong.ogg"),
            Self::NeedleDrop => Some("assets/sounds/requestline/needle.ogg"),
            Self::Selector => Some("assets/sounds/requestline/selector.ogg"),
        }
    }
}

named_choice! {
    /// When a request arrives mid-song: hold the sound until the song is over,
    /// or play it at once. The toast shows at once either way.
    RequestlineSoundTiming {
        Hold = "Hold",
        Now = "Now",
    }
}

named_choice! {
    /// The emoji on a new request's toast. Off shows no toast.
    RequestlineVisual {
        Off = "Off",
        Notes = "Notes",
        Ticket = "Ticket",
        Disc = "Disc",
        Bell = "Bell",
        Slots = "Slots",
    }
}

impl RequestlineVisual {
    /// The emoji drawn on the toast, or `None` for Off. All five are in
    /// deadsync's bitmap emoji font.
    pub const fn emoji(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Notes => Some("\u{1F3B5}"),
            Self::Ticket => Some("\u{1F39F}"),
            Self::Disc => Some("\u{1F4BF}"),
            Self::Bell => Some("\u{1F514}"),
            Self::Slots => Some("\u{1F3B0}"),
        }
    }
}

named_choice! {
    /// Where the toast sits. Auto takes the corner opposite the requests
    /// profile's side (P1 bottom right, P2 bottom left).
    RequestlineToastCorner {
        Auto = "Auto",
        TopLeft = "TopLeft",
        TopRight = "TopRight",
        BottomLeft = "BottomLeft",
        BottomRight = "BottomRight",
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequestlineOptions {
    pub enabled: bool,
    pub side: RequestlineSide,
    pub sound: RequestlineSound,
    pub sound_timing: RequestlineSoundTiming,
    pub visual: RequestlineVisual,
    pub corner: RequestlineToastCorner,
}

const SECTION: &str = "Requestline";

pub fn load(conf: &SimpleIni, default: RequestlineOptions) -> RequestlineOptions {
    RequestlineOptions {
        enabled: load_bool_option(conf, SECTION, "Enabled", default.enabled),
        side: conf
            .get(SECTION, "Profile")
            .and_then(RequestlineSide::parse)
            .unwrap_or(default.side),
        sound: conf
            .get(SECTION, "NotifySound")
            .and_then(RequestlineSound::parse)
            .unwrap_or(default.sound),
        sound_timing: conf
            .get(SECTION, "SoundDuringSongs")
            .and_then(RequestlineSoundTiming::parse)
            .unwrap_or(default.sound_timing),
        visual: conf
            .get(SECTION, "NotifyVisual")
            .and_then(RequestlineVisual::parse)
            .unwrap_or(default.visual),
        corner: conf
            .get(SECTION, "ToastCorner")
            .and_then(RequestlineToastCorner::parse)
            .unwrap_or(default.corner),
    }
}

pub fn push_options(content: &mut String, options: RequestlineOptions) {
    push_section(content, "[Requestline]");
    push_bool(content, "Enabled", options.enabled);
    push_line(content, "Profile", options.side.as_str());
    push_line(content, "NotifySound", options.sound.as_str());
    push_line(content, "SoundDuringSongs", options.sound_timing.as_str());
    push_line(content, "NotifyVisual", options.visual.as_str());
    push_line(content, "ToastCorner", options.corner.as_str());
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
        let defaults = RequestlineOptions::default();
        assert_eq!(defaults.sound, RequestlineSound::Off, "alerts are opt-in");
        assert_eq!(defaults.visual, RequestlineVisual::Off);
        assert_eq!(defaults.sound_timing, RequestlineSoundTiming::Hold);
        assert_eq!(defaults.corner, RequestlineToastCorner::Auto);
        let options = RequestlineOptions {
            enabled: true,
            side: RequestlineSide::P2,
            sound: RequestlineSound::NeedleDrop,
            sound_timing: RequestlineSoundTiming::Now,
            visual: RequestlineVisual::Bell,
            corner: RequestlineToastCorner::TopLeft,
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
        assert_eq!(RequestlineSound::parse(" chime "), Some(RequestlineSound::Chime));
        assert_eq!(RequestlineSound::from_index(99), RequestlineSound::Off);
        assert_eq!(RequestlineVisual::Slots.index(), 5);
        assert!(RequestlineSound::ALL[1..].iter().all(|s| s.asset_path().is_some()));
        assert!(RequestlineVisual::ALL[1..].iter().all(|v| v.emoji().is_some()));
    }
}
