//! Requestline new-request alerts: a sound and a corner popup, both opt-in
//! (ArrowCloud Options > New Request Sound / New Request Popup).
//!
//! One popup at a time: a request while it shows updates it in place (the
//! latest title, "+N more") and restarts its timer. Requests within `GROUP` of
//! each other make one sound. During gameplay the popup still shows at once,
//! but with "Hold Until Song Ends" the sound waits until gameplay is over, so
//! a chime never costs anyone a run.

use super::{App, CurrentScreen};
use deadlib_present::actors::Actor;
use deadsync_config::requestline::{
    RequestlineOptions, RequestlineSound, RequestlineSoundTiming, RequestlineToastCorner,
    RequestlineVisual,
};
use deadsync_profile::PlayerSide;
use deadsync_theme_simply_love::screens::components::shared::request_toast::ToastSpot;
use deadsync_online::requestline::QueueEntry;
use std::time::{Duration, Instant};

/// Arrivals this close together make one sound.
const GROUP: Duration = Duration::from_secs(2);
const FADE_IN: Duration = Duration::from_millis(200);
/// How long the popup stays after the last request it shows arrived.
const HOLD: Duration = Duration::from_secs(4);
const FADE_OUT: Duration = Duration::from_millis(300);

/// A requested song's titles, from the local library when it has the song
/// (for the translit setting), else the title the site sent.
#[derive(Debug)]
struct Titles {
    native: String,
    translit: String,
}

impl Titles {
    fn of(entry: &QueueEntry) -> Self {
        let local = deadsync_simfile::runtime_cache::get_song_cache()
            .iter()
            .flat_map(|pack| pack.songs.iter())
            .find(|song| {
                deadsync_online::requestline::song_id(&song.simfile_path).as_deref()
                    == Some(entry.song_id.as_str())
            })
            .map(|song| (song.display_title(false).to_owned(), song.display_title(true).to_owned()));
        let (native, translit) = local.unwrap_or_else(|| (entry.title.clone(), entry.title.clone()));
        Self { native, translit }
    }

    fn shown(&self, translit: bool) -> &str {
        if translit { &self.translit } else { &self.native }
    }
}

#[derive(Debug)]
struct Toast {
    title: Titles,
    requester: String,
    more: usize,
    shown_at: Instant,
    last_arrival: Instant,
}

#[derive(Debug, Default)]
pub(super) struct RequestAlerts {
    toast: Option<Toast>,
    last_arrival: Option<Instant>,
    /// A sound owed for requests that arrived mid-song.
    sound_held: bool,
}

impl RequestAlerts {
    /// New requests arrived. Returns whether to play the sound now.
    fn arrive(
        &mut self,
        fresh: &[QueueEntry],
        options: &RequestlineOptions,
        in_song: bool,
        now: Instant,
    ) -> bool {
        let Some(latest) = fresh.last() else {
            return false;
        };
        let grouped = self
            .last_arrival
            .is_some_and(|at| now.saturating_duration_since(at) < GROUP);
        self.last_arrival = Some(now);

        if options.visual != RequestlineVisual::Off {
            match self.toast.as_mut() {
                // Still on screen: update it in place and restart its timer.
                Some(toast) => {
                    toast.more += fresh.len();
                    toast.title = Titles::of(latest);
                    toast.requester.clone_from(&latest.name);
                    toast.last_arrival = now;
                }
                None => {
                    self.toast = Some(Toast {
                        title: Titles::of(latest),
                        requester: latest.name.clone(),
                        more: fresh.len() - 1,
                        shown_at: now,
                        last_arrival: now,
                    });
                }
            }
        }

        if grouped || options.sound == RequestlineSound::Off {
            return false;
        }
        if in_song && options.sound_timing == RequestlineSoundTiming::Hold {
            self.sound_held = true;
            return false;
        }
        true
    }

    /// Once gameplay is over, the held sound is due (if a sound is still on).
    fn release(&mut self, options: &RequestlineOptions, in_song: bool) -> bool {
        if !self.sound_held || in_song {
            return false;
        }
        self.sound_held = false;
        options.sound != RequestlineSound::Off
    }

    /// 0..1 how shown the popup is now; `None` once it is gone.
    fn visibility(&mut self, now: Instant) -> Option<f32> {
        let toast = self.toast.as_ref()?;
        let since_shown = now.saturating_duration_since(toast.shown_at);
        let fade_in = (since_shown.as_secs_f32() / FADE_IN.as_secs_f32()).min(1.0);
        let gone_at = toast.last_arrival + HOLD;
        let fade_out = if now <= gone_at {
            1.0
        } else {
            1.0 - now.saturating_duration_since(gone_at).as_secs_f32() / FADE_OUT.as_secs_f32()
        };
        if fade_out <= 0.0 {
            self.toast = None;
            return None;
        }
        Some(fade_in.min(fade_out))
    }
}

/// Auto keeps the popup off the notefields: one player on P1 puts it bottom
/// right, one on P2 bottom left, two players bottom center between them.
const fn spot(corner: RequestlineToastCorner, p1_joined: bool, p2_joined: bool) -> ToastSpot {
    match (corner, p1_joined, p2_joined) {
        (RequestlineToastCorner::Auto, true, true) => ToastSpot::BottomCenter,
        (RequestlineToastCorner::Auto, false, true) => ToastSpot::BottomLeft,
        (RequestlineToastCorner::Auto, _, _) => ToastSpot::BottomRight,
        (corner, _, _) => ToastSpot::corner(corner),
    }
}

impl App {
    /// Every frame: take new requests, play what is due, draw the popup.
    pub(super) fn append_requestline_alert_actors(&mut self, actors: &mut Vec<Actor>, now: Instant) {
        let options = deadsync_config::runtime::get().requestline;
        let in_song = self.state.screens.current_screen == CurrentScreen::Gameplay;
        // Always drained, so turning alerts on later doesn't replay old ones.
        let fresh = deadsync_online::requestline::take_new_requests();
        let alerts = &mut self.requestline_alerts;
        let now_due = alerts.arrive(&fresh, &options, in_song, now);
        let held_due = alerts.release(&options, in_song);
        if (now_due || held_due)
            && let Some(path) = options.sound.asset_path()
        {
            log::info!(
                "requestline: new-request sound {}{}",
                options.sound.as_str(),
                if held_due { " (held until the song ended)" } else { "" }
            );
            self.ui_sfx.play(&mut self.audio, path);
        }

        let alerts = &mut self.requestline_alerts;
        let (Some(visibility), Some(emoji)) = (alerts.visibility(now), options.visual.emoji())
        else {
            return;
        };
        let Some(toast) = alerts.toast.as_ref() else {
            return;
        };
        let translit = deadsync_config::runtime::get().translated_titles;
        deadsync_theme_simply_love::screens::components::shared::request_toast::push(
            actors,
            &deadsync_theme_simply_love::screens::components::shared::request_toast::RequestToast {
                emoji,
                title: toast.title.shown(translit),
                requester: &toast.requester,
                more: toast.more,
                spot: spot(
                    options.corner,
                    deadsync_profile::runtime_session_side_joined(PlayerSide::P1),
                    deadsync_profile::runtime_session_side_joined(PlayerSide::P2),
                ),
                visibility,
                slide: !in_song,
            },
            &self.asset_manager,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> QueueEntry {
        QueueEntry {
            id: id.to_owned(),
            song_id: format!("p/{id}"),
            title: format!("Song {id}"),
            name: format!("viewer {id}"),
            chart: None,
        }
    }

    fn on() -> RequestlineOptions {
        RequestlineOptions {
            enabled: true,
            sound: RequestlineSound::Chime,
            visual: RequestlineVisual::Notes,
            ..Default::default()
        }
    }

    #[test]
    fn nothing_happens_with_alerts_off() {
        let mut alerts = RequestAlerts::default();
        let now = Instant::now();
        let off = RequestlineOptions::default();
        assert!(!alerts.arrive(&[entry("a")], &off, false, now));
        assert_eq!(alerts.visibility(now), None);
        assert!(!alerts.release(&off, false));
    }

    #[test]
    fn a_burst_is_one_alert_with_one_sound() {
        let mut alerts = RequestAlerts::default();
        let t0 = Instant::now();
        assert!(alerts.arrive(&[entry("a")], &on(), false, t0));
        assert!(!alerts.arrive(&[entry("b"), entry("c")], &on(), false, t0 + Duration::from_secs(1)));
        let toast = alerts.toast.as_ref().unwrap();
        assert_eq!((toast.title.shown(false), toast.more), ("Song c", 2), "latest, plus the rest");

        // Past the sound's grouping but while the popup still shows: a new
        // sound, and the same popup counts on with its timer restarted.
        let t3 = t0 + Duration::from_secs(3);
        assert!(alerts.arrive(&[entry("d")], &on(), false, t3));
        assert_eq!(alerts.toast.as_ref().unwrap().more, 3);
        assert_eq!(alerts.visibility(t3 + HOLD - Duration::from_millis(10)), Some(1.0));

        // Once the popup is gone, the next request starts a fresh one.
        assert_eq!(alerts.visibility(t3 + HOLD + FADE_OUT * 2), None);
        let later = t3 + Duration::from_secs(20);
        assert!(alerts.arrive(&[entry("e")], &on(), false, later));
        assert_eq!(alerts.toast.as_ref().unwrap().more, 0);
    }

    #[test]
    fn mid_song_the_popup_shows_but_the_sound_waits_for_the_song_to_end() {
        let mut alerts = RequestAlerts::default();
        let t0 = Instant::now();
        assert!(!alerts.arrive(&[entry("a")], &on(), true, t0), "held");
        assert!(alerts.visibility(t0 + Duration::from_secs(1)).is_some(), "popup at once");
        assert!(!alerts.arrive(&[entry("b")], &on(), true, t0 + Duration::from_secs(30)));
        assert!(!alerts.release(&on(), true), "still in the song");
        assert!(alerts.release(&on(), false), "once, after the song");
        assert!(!alerts.release(&on(), false));

        let now = RequestlineOptions {
            sound_timing: RequestlineSoundTiming::Now,
            ..on()
        };
        assert!(alerts.arrive(&[entry("c")], &now, true, t0 + Duration::from_secs(60)));
    }

    #[test]
    fn turning_the_sound_off_mid_song_drops_the_held_one() {
        let mut alerts = RequestAlerts::default();
        alerts.arrive(&[entry("a")], &on(), true, Instant::now());
        let quiet = RequestlineOptions {
            sound: RequestlineSound::Off,
            ..on()
        };
        assert!(!alerts.release(&quiet, false));
    }

    #[test]
    fn the_popup_fades_in_holds_and_goes() {
        let mut alerts = RequestAlerts::default();
        let t0 = Instant::now();
        alerts.arrive(&[entry("a")], &on(), false, t0);
        assert_eq!(alerts.visibility(t0), Some(0.0));
        assert_eq!(alerts.visibility(t0 + Duration::from_secs(2)), Some(1.0));
        let fading = alerts.visibility(t0 + HOLD + FADE_OUT / 2).unwrap();
        assert!(fading > 0.0 && fading < 1.0);
        assert_eq!(alerts.visibility(t0 + HOLD + FADE_OUT * 2), None);
        assert!(alerts.toast.is_none());
    }

    #[test]
    fn auto_keeps_the_popup_off_the_notefields() {
        let auto = RequestlineToastCorner::Auto;
        assert_eq!(spot(auto, true, false), ToastSpot::BottomRight, "P1 alone");
        assert_eq!(spot(auto, false, true), ToastSpot::BottomLeft, "P2 alone");
        assert_eq!(spot(auto, true, true), ToastSpot::BottomCenter, "two players");
        // A corner the streamer picked stays put.
        assert_eq!(spot(RequestlineToastCorner::TopLeft, true, true), ToastSpot::TopLeft);
    }
}
