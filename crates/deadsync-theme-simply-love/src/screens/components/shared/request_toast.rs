//! Requestline's new-request popup: a small two-line card in a screen corner.
//!
//! ```text
//! 🎵 Stay
//!    AutoPlayCPU  +2 more
//! ```
//!
//! Drawn by the shell over every screen, gameplay included, so it stays small
//! and out of the middle. The shell owns timing; this only draws one pose.

use crate::act;
use deadlib_present::actors::Actor;
use deadlib_present::space::{screen_height, screen_width};
use deadsync_config::requestline::RequestlineToastCorner;

/// Over the screens and their transitions, under the frame-stats and version
/// overlays (32010+) and the screenshot flash (32000).
const TOAST_Z: i16 = 31900;
const WIDTH: f32 = 230.0;
const HEIGHT: f32 = 40.0;
/// Room above the bottom screen bar and below the top one.
const MARGIN_X: f32 = 10.0;
const MARGIN_Y: f32 = 42.0;
const PAD_X: f32 = 8.0;
const ACCENT_W: f32 = 3.0;
/// How far the card travels as it slides in from its edge.
const SLIDE: f32 = 24.0;
/// Titles past this many characters are cut with an ellipsis, so the card
/// never grows into the playfield.
const TITLE_CHARS: usize = 28;
const NAME_CHARS: usize = 24;

/// One frame of the popup.
pub struct RequestToast<'a> {
    pub emoji: &'a str,
    pub title: &'a str,
    pub requester: &'a str,
    /// Further requests that arrived with this one.
    pub more: usize,
    /// Never `Auto`: the shell picks the side first.
    pub corner: RequestlineToastCorner,
    /// 0 hidden .. 1 fully shown; also drives the slide.
    pub visibility: f32,
}

fn cut(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars - 1).collect();
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

/// The card's top-left corner for a fully shown popup.
fn origin(corner: RequestlineToastCorner, w: f32, h: f32) -> (f32, f32, f32) {
    // The last value is the slide direction: toward the nearer side edge.
    let left = (MARGIN_X, -1.0);
    let right = (w - MARGIN_X - WIDTH, 1.0);
    let top = MARGIN_Y;
    let bottom = h - MARGIN_Y - HEIGHT;
    match corner {
        RequestlineToastCorner::TopLeft => (left.0, top, left.1),
        RequestlineToastCorner::TopRight => (right.0, top, right.1),
        RequestlineToastCorner::BottomLeft => (left.0, bottom, left.1),
        RequestlineToastCorner::BottomRight | RequestlineToastCorner::Auto => {
            (right.0, bottom, right.1)
        }
    }
}

pub fn push(actors: &mut Vec<Actor>, toast: &RequestToast<'_>) {
    let shown = toast.visibility.clamp(0.0, 1.0);
    if shown <= 0.0 {
        return;
    }
    let (x0, y, toward_edge) = origin(toast.corner, screen_width(), screen_height());
    // Ease out: fast at first, settling into place.
    let eased = 1.0 - (1.0 - shown) * (1.0 - shown);
    let x = x0 + toward_edge * SLIDE * (1.0 - eased);
    let a = eased;

    let title = format!("{} {}", toast.emoji, cut(toast.title, TITLE_CHARS));
    let mut second = cut(toast.requester, NAME_CHARS);
    if toast.more > 0 {
        second.push_str("  ");
        second.push_str(&crate::i18n::tr_fmt(
            "RequestToast",
            "More",
            &[("count", &toast.more.to_string())],
        ));
    }
    let text_x = x + ACCENT_W + PAD_X;
    let text_w = WIDTH - ACCENT_W - PAD_X * 2.0;

    actors.reserve(4);
    actors.push(act!(quad:
        align(0.0, 0.0): xy(x, y): zoomto(WIDTH, HEIGHT):
        diffuse(0.0, 0.0, 0.0, 0.82 * a): z(TOAST_Z)
    ));
    // Requestline's purple, as on the site.
    actors.push(act!(quad:
        align(0.0, 0.0): xy(x, y): zoomto(ACCENT_W, HEIGHT):
        diffuse(0.62, 0.45, 1.0, a): z(TOAST_Z + 1)
    ));
    actors.push(act!(text:
        align(0.0, 0.0): xy(text_x, y + 4.0): font("miso"): zoom(0.8):
        maxwidth(text_w / 0.8): settext(title):
        diffuse(1.0, 1.0, 1.0, a): horizalign(left): z(TOAST_Z + 2)
    ));
    actors.push(act!(text:
        align(0.0, 0.0): xy(text_x + 20.0, y + 23.0): font("miso"): zoom(0.6):
        maxwidth((text_w - 20.0) / 0.6): settext(second):
        diffuse(0.78, 0.78, 0.82, 0.85 * a): horizalign(left): z(TOAST_Z + 2)
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_titles_are_cut_with_an_ellipsis() {
        assert_eq!(cut("Stay", 28), "Stay");
        let long = "A Very Long Song Title That Keeps On Going";
        let short = cut(long, 28);
        assert!(short.ends_with('…'));
        assert_eq!(short.chars().count(), 28);
        assert_eq!(cut("トキメキの音符になって (箱崎星梨花 13歳 P@RTY mix)", 10).chars().count(), 10);
    }

    #[test]
    fn each_corner_sits_inside_the_screen() {
        let (w, h) = (854.0, 480.0);
        for corner in RequestlineToastCorner::ALL {
            let (x, y, _) = origin(*corner, w, h);
            assert!(x >= 0.0 && x + WIDTH <= w, "{corner:?}");
            assert!(y >= 0.0 && y + HEIGHT <= h, "{corner:?}");
        }
        let (left, _, _) = origin(RequestlineToastCorner::BottomLeft, w, h);
        let (right, _, _) = origin(RequestlineToastCorner::BottomRight, w, h);
        assert!(left < w / 2.0 && right > w / 2.0);
    }
}
