//! Requestline's pending-requests chip, beside the requests profile's name in
//! Select Music's footer.
//!
//! At rest it reads "3 requests". When new requests come in it morphs to the
//! popup's icon and the count ("🎵 3") and back, a few times, to catch the
//! streamer's eye, then settles.

use crate::act;
use crate::screens::components::shared::screen_bar;
use deadlib_assets::AssetManager;
use deadlib_present::actors::Actor;
use deadlib_present::font;
use deadlib_present::space::{screen_height, screen_width};

/// Just over the footer bar (120) and its texts.
const CHIP_Z: i16 = 125;
const HEIGHT: f32 = 16.0;
/// Between the name and the chip.
const GAP_TO_NAME: f32 = 5.0;
const PAD: f32 = 5.0;
/// Between the count and its label or icon.
const INNER_GAP: f32 = 3.0;
const COUNT_ZOOM: f32 = 0.7;
const LABEL_ZOOM: f32 = 0.6;
/// The emoji about 11px tall (Miso's emoji fallback is about 23px at zoom 1).
const ICON_SIZE: f32 = 11.0;
const ICON_ZOOM: f32 = ICON_SIZE / 23.0;

/// How the attention morph runs: this many times, one every `PERIOD` seconds,
/// each a `MORPH` to the icon, a hold, and a `MORPH` back.
const CYCLES: f32 = 3.0;
const PERIOD: f32 = 5.0;
const MORPH: f32 = 0.35;
const HOLD: f32 = 1.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    P1,
    P2,
}

#[derive(Debug, Clone, Copy)]
pub struct RequestChip<'a> {
    pub side: Side,
    /// The requests profile's name as the footer shows it.
    pub name: &'a str,
    pub count: usize,
    pub open: bool,
    pub icon: &'a str,
    /// Seconds since new requests came in, while the morph may still run.
    pub since_new: Option<f32>,
}

/// How far the chip is toward the icon form at `since` seconds after new
/// requests came in: 0 reads "3 requests", 1 reads "🎵 3".
#[must_use]
pub fn icon_amount(since: f32) -> f32 {
    if !(0.0..CYCLES * PERIOD).contains(&since) {
        return 0.0;
    }
    let t = since % PERIOD;
    let linear = if t < MORPH {
        t / MORPH
    } else if t < MORPH + HOLD {
        1.0
    } else if t < MORPH + HOLD + MORPH {
        1.0 - (t - MORPH - HOLD) / MORPH
    } else {
        0.0
    };
    // Ease in and out.
    linear * linear * (3.0 - 2.0 * linear)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    (b - a).mul_add(t, a)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

pub fn push(actors: &mut Vec<Actor>, chip: &RequestChip<'_>, accent: [f32; 4], assets: &AssetManager) {
    let count = chip.count.to_string();
    let label = crate::i18n::tr(
        "RequestChip",
        if chip.count == 1 { "One" } else { "Many" },
    );
    let Some((name_w, count_w, label_w)) = assets.with_fonts(|fonts| {
        let miso = fonts.get("miso")?;
        let measure = |s: &str| font::measure_line_width_logical(miso, s, fonts) as f32;
        Some((measure(chip.name), measure(&count), measure(&label)))
    }) else {
        return;
    };
    let (margin_x, name_zoom) = screen_bar::footer_side_text_metrics();
    let name_w = name_w * name_zoom;
    let count_w = count_w * COUNT_ZOOM;
    let label_w = label_w * LABEL_ZOOM;

    let t = chip.since_new.map_or(0.0, icon_amount);
    let words_w = PAD + count_w + INNER_GAP + label_w + PAD;
    let icon_w = PAD + ICON_SIZE + INNER_GAP + count_w + PAD;
    let width = lerp(words_w, icon_w, t);
    let left = match chip.side {
        Side::P1 => margin_x + name_w + GAP_TO_NAME,
        Side::P2 => screen_width() - margin_x - name_w - GAP_TO_NAME - width,
    };
    let center_y = screen_height() - 16.0;

    let rgb = [accent[0], accent[1], accent[2]];
    let (fill, count_rgb) = if chip.open {
        // Dark with a light count at rest; the accent itself with a white
        // count in the icon form, where the dark emoji needs a light ground.
        (
            mix(mix([0.0; 3], rgb, 0.3), rgb, t),
            mix(mix(rgb, [1.0; 3], 0.35), [1.0; 3], t),
        )
    } else {
        // Closed: grey, so it reads as off at a glance.
        (mix([0.29; 3], [0.6; 3], t), [1.0; 3])
    };
    let count_x = left + PAD + (ICON_SIZE + INNER_GAP) * t;
    let label_x = left + PAD + count_w + INNER_GAP;
    // The label goes before the pill has shrunk past it.
    let label_a = 2.0f32.mul_add(-t, 1.0).max(0.0);
    let icon_a = 2.0f32.mul_add(t, -1.0).max(0.0);

    actors.reserve(4);
    actors.push(act!(quad:
        align(0.0, 0.5): xy(left, center_y): zoomto(width, HEIGHT):
        diffuse(fill[0], fill[1], fill[2], 1.0): z(CHIP_Z)
    ));
    actors.push(act!(text:
        align(0.0, 0.5): xy(count_x, center_y): font("miso"): zoom(COUNT_ZOOM):
        settext(count): diffuse(count_rgb[0], count_rgb[1], count_rgb[2], 1.0):
        horizalign(left): z(CHIP_Z + 1)
    ));
    if label_a > 0.0 {
        actors.push(act!(text:
            align(0.0, 0.5): xy(label_x, center_y): font("miso"): zoom(LABEL_ZOOM):
            settext(label.to_string()): diffuse(1.0, 1.0, 1.0, label_a):
            horizalign(left): z(CHIP_Z + 1)
        ));
    }
    if icon_a > 0.0 {
        actors.push(act!(text:
            align(0.5, 0.5): xy(left + PAD + ICON_SIZE * 0.5, center_y): font("miso"):
            zoom(ICON_ZOOM): settext(chip.icon.to_owned()): diffuse(1.0, 1.0, 1.0, icon_a):
            z(CHIP_Z + 1)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_morph_runs_three_times_then_rests_on_the_words() {
        assert_eq!(icon_amount(-1.0), 0.0);
        assert_eq!(icon_amount(0.0), 0.0);
        assert_eq!(icon_amount(1.0), 1.0, "holding the icon");
        assert_eq!(icon_amount(3.0), 0.0, "back to the words");
        let peaks = (0..200)
            .map(|i| i as f32 * 0.1)
            .filter(|&t| icon_amount(t) == 1.0 && icon_amount(t - 0.1) < 1.0)
            .count();
        assert_eq!(peaks, 3);
        assert_eq!(icon_amount(15.0), 0.0);
        assert_eq!(icon_amount(60.0), 0.0);
    }

    #[test]
    fn the_morph_is_smooth() {
        let mut last = icon_amount(0.0);
        for i in 1..1500 {
            let now = icon_amount(i as f32 * 0.01);
            assert!((now - last).abs() < 0.1, "jump at {}s", i as f32 * 0.01);
            last = now;
        }
    }
}
