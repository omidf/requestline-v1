//! Requestline's new-request popup: a small Simply Love panel in a corner.
//!
//! ```text
//! ┌──────────────────────────────┐
//! │ 🎵  Stay                      │
//! │     AutoPlayCPU  +2 more      │
//! └──────────────────────────────┘
//! ```
//!
//! Square-edged black panel, the emoji as its icon on the left beside both
//! lines, the title in white Miso and the requester under it in gray. Drawn
//! by the shell over every screen, gameplay included; it is in the stream
//! capture on purpose, so the requester sees their name. The shell owns the
//! timing and picks the corner; this draws one pose.

use crate::act;
use deadlib_assets::AssetManager;
use deadlib_present::actors::Actor;
use deadlib_present::font;
use deadlib_present::space::{screen_height, screen_width};
use deadsync_config::requestline::RequestlineToastCorner;

/// Over the screens and their transitions, under the screenshot flash
/// (32000) and the frame-stats and version overlays (32010+).
const TOAST_Z: i16 = 31900;
/// At most 240 wide in the 854x480 layout (about 28% of the width).
const WIDTH: f32 = 240.0;
const HEIGHT: f32 = 38.0;
/// In from the side edge.
const MARGIN_X: f32 = 16.0;
/// Clear of Simply Love's header and footer bars on menus. Where the toast
/// sits is still to be settled; this keeps it off the bars meanwhile.
const MARGIN_Y: f32 = 42.0;
const PAD: f32 = 8.0;
/// About 21px tall: the 48px emoji bitmaps draw 1:1 at 1080p.
const ICON_SIZE: f32 = 21.0;
/// Miso's emoji fallback draws about 23px tall at zoom 1 in this layout
/// (measured from a 720p capture).
const ICON_ZOOM: f32 = ICON_SIZE / 23.0;
const TITLE_ZOOM: f32 = 0.8;
const NAME_ZOOM: f32 = 0.6;
/// How far the panel travels as it slides in from its edge.
const SLIDE: f32 = 24.0;
const ELLIPSIS: &str = "…";

/// One frame of the popup.
pub struct RequestToast<'a> {
    pub emoji: &'a str,
    pub title: &'a str,
    pub requester: &'a str,
    /// Further requests shown with this one.
    pub more: usize,
    /// Never `Auto`: the shell picks the spot first.
    pub corner: RequestlineToastCorner,
    /// 0 hidden .. 1 fully shown.
    pub visibility: f32,
    /// Slide in from the edge; `false` only fades (during a song, where
    /// motion at the edge of vision pulls the eye off the arrows).
    pub slide: bool,
}

/// The text as it fits in `max_width` logical pixels at `zoom`, cut with an
/// ellipsis when it doesn't.
fn fit(text: &str, max_width: f32, zoom: f32, measure: &impl Fn(&str) -> f32) -> String {
    let text = text.trim();
    let fits = |s: &str| measure(s) * zoom <= max_width;
    if fits(text) {
        return text.to_owned();
    }
    // Longest prefix that still fits with the ellipsis after it.
    let ends: Vec<usize> = text.char_indices().map(|(i, _)| i).skip(1).collect();
    let (mut lo, mut hi) = (0usize, ends.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate = format!("{}{ELLIPSIS}", text[..ends[mid - 1]].trim_end());
        if fits(&candidate) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 {
        return ELLIPSIS.to_owned();
    }
    format!("{}{ELLIPSIS}", text[..ends[lo - 1]].trim_end())
}

/// The panel's top-left corner when fully shown, and which way it slides in
/// from (-1 from the left edge, 1 from the right).
fn origin(corner: RequestlineToastCorner, w: f32, h: f32) -> (f32, f32, f32) {
    let left = MARGIN_X;
    let right = w - MARGIN_X - WIDTH;
    let top = MARGIN_Y;
    let bottom = h - MARGIN_Y - HEIGHT;
    match corner {
        RequestlineToastCorner::TopLeft => (left, top, -1.0),
        RequestlineToastCorner::TopRight => (right, top, 1.0),
        RequestlineToastCorner::BottomLeft => (left, bottom, -1.0),
        RequestlineToastCorner::BottomRight | RequestlineToastCorner::Auto => {
            (right, bottom, 1.0)
        }
    }
}

pub fn push(actors: &mut Vec<Actor>, toast: &RequestToast<'_>, assets: &AssetManager) {
    let shown = toast.visibility.clamp(0.0, 1.0);
    if shown <= 0.0 {
        return;
    }
    let (x0, y, from) = origin(toast.corner, screen_width(), screen_height());
    let eased = 1.0 - (1.0 - shown) * (1.0 - shown);
    let x = if toast.slide {
        x0 + from * SLIDE * (1.0 - eased)
    } else {
        x0
    };
    let a = eased;

    let text_x = x + PAD + ICON_SIZE + PAD;
    let text_w = x + WIDTH - PAD - text_x;
    let mut second = toast.requester.trim().to_owned();
    if toast.more > 0 {
        second.push_str("  ");
        second.push_str(&crate::i18n::tr_fmt(
            "RequestToast",
            "More",
            &[("count", &toast.more.to_string())],
        ));
    }
    let (title, second) = assets.with_fonts(|fonts| match fonts.get("miso") {
        Some(miso) => {
            let measure = |s: &str| font::measure_line_width_logical(miso, s, fonts) as f32;
            (
                fit(toast.title, text_w, TITLE_ZOOM, &measure),
                fit(&second, text_w, NAME_ZOOM, &measure),
            )
        }
        None => (toast.title.trim().to_owned(), second.clone()),
    });

    actors.reserve(4);
    actors.push(act!(quad:
        align(0.0, 0.0): xy(x, y): zoomto(WIDTH, HEIGHT):
        diffuse(0.0, 0.0, 0.0, 0.85 * a): z(TOAST_Z)
    ));
    actors.push(act!(text:
        align(0.5, 0.5): xy(x + PAD + ICON_SIZE * 0.5, y + HEIGHT * 0.5): font("miso"):
        zoom(ICON_ZOOM): settext(toast.emoji.to_owned()):
        diffuse(1.0, 1.0, 1.0, a): z(TOAST_Z + 1)
    ));
    actors.push(act!(text:
        align(0.0, 0.0): xy(text_x, y + 4.0): font("miso"): zoom(TITLE_ZOOM):
        maxwidth(text_w / TITLE_ZOOM): settext(title):
        diffuse(1.0, 1.0, 1.0, a): horizalign(left): z(TOAST_Z + 1)
    ));
    actors.push(act!(text:
        align(0.0, 0.0): xy(text_x, y + 22.0): font("miso"): zoom(NAME_ZOOM):
        maxwidth(text_w / NAME_ZOOM): settext(second):
        diffuse(0.7, 0.7, 0.7, a): horizalign(left): z(TOAST_Z + 1)
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character 10 wide, for predictable fitting.
    fn measure(s: &str) -> f32 {
        s.chars().count() as f32 * 10.0
    }

    #[test]
    fn titles_are_cut_at_the_width_with_an_ellipsis() {
        assert_eq!(fit("Stay", 100.0, 1.0, &measure), "Stay");
        // 10 characters fit; with the ellipsis, 9 of the title.
        assert_eq!(fit("A Long Song Title", 100.0, 1.0, &measure), "A Long So…");
        // Zoom scales the measured width.
        assert_eq!(fit("A Long Song Title", 100.0, 0.5, &measure), "A Long Song Title");
        // Trailing spaces before the cut are dropped.
        assert_eq!(fit("Abcdefgh Ijk", 100.0, 1.0, &measure), "Abcdefgh…");
        assert_eq!(fit("トキメキの音符になって", 50.0, 1.0, &measure), "トキメキ…");
        assert_eq!(fit("Anything", 5.0, 1.0, &measure), "…");
    }

    #[test]
    fn each_corner_sits_inside_the_screen() {
        let (w, h) = (854.0, 480.0);
        for corner in RequestlineToastCorner::ALL {
            let (x, y, _) = origin(*corner, w, h);
            assert!(x >= MARGIN_X - 0.01 && x + WIDTH <= w - MARGIN_X + 0.01, "{corner:?}");
            assert!(y >= 0.0 && y + HEIGHT <= h, "{corner:?}");
        }
        let (left, _, from_left) = origin(RequestlineToastCorner::BottomLeft, w, h);
        let (right, _, from_right) = origin(RequestlineToastCorner::BottomRight, w, h);
        assert!(left < w / 2.0 && right > w / 2.0);
        assert!(from_left < 0.0 && from_right > 0.0, "slides in from the nearer edge");
    }
}
