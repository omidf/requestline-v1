//! Requestline in Select Music: the page's queue as a wheel view, and the
//! Requestline... menu that reaches it.
//!
//! The queue is shown through the playlist sort under a reserved id, built in
//! memory from the shell's snapshot. Nothing is written to the profile's
//! playlists, and it is not listed under Playlists....

use super::playlists::build_playlist_song_lookup;
use super::{
    MusicWheelEntry, State, WheelSortMode, apply_wheel_sort, focus_song_from_search,
    select_music_menu, set_selected_steps_index_for_sync,
};
use crate::screens::components::select_music::request_chip::{RequestChip, Side as ChipSide};
use crate::views::SelectMusicRequestlineView;
use deadlib_present::actors::TextContent;
use deadsync_chart::SongData;
use deadsync_online::requestline::{QueueEntry, Status};
use deadsync_simfile::playlist::PlaylistSongLookup;
use std::sync::Arc;

/// The playlist id the Requests view is applied under. Never a file name.
pub(super) const REQUESTS_PLAYLIST_ID: &str = "requestline:queue";

const S: &str = "SelectMusicMenu";

#[derive(Default)]
pub(super) struct Requests {
    view: Option<SelectMusicRequestlineView>,
    /// Resolves `Pack/SongDir` ids, built from the group entries it was made for.
    lookup: Option<(Arc<[MusicWheelEntry]>, PlaylistSongLookup)>,
    /// Queue positions the local library can play, in queue order.
    playable: Vec<(usize, Arc<SongData>)>,
    /// The Requests view: one header and the playable songs. Empty when none.
    pub(super) entries: Arc<[MusicWheelEntry]>,
    /// After a cancel, the request to land on once the server's new queue
    /// arrives.
    focus_after_change: Option<Arc<SongData>>,
    /// When the queue last grew (`session_elapsed`), for the footer chip's
    /// attention morph.
    grew_at: Option<f32>,
}

/// Whether the shell's view differs from what Select Music last applied.
pub fn requestline_view_stale(
    state: &State,
    enabled: bool,
    status: &Status,
    generation: u64,
    side: deadsync_config::requestline::RequestlineSide,
    icon: &str,
) -> bool {
    state.requests.view.as_ref().is_none_or(|view| {
        view.enabled != enabled
            || &view.status != status
            || view.generation != generation
            || view.side != side
            || view.icon != icon
    })
}

pub(super) fn apply_view(state: &mut State, view: SelectMusicRequestlineView) {
    let queue_changed = state
        .requests
        .view
        .as_ref()
        .is_none_or(|old| old.queue != view.queue);
    let before = queue(state).len();
    state.requests.view = Some(view);
    if queue(state).len() > before {
        state.requests.grew_at = Some(state.session_elapsed);
    }
    if queue_changed {
        rebuild(state);
        if showing(state) {
            if state.requests.entries.is_empty() {
                back_to_library(state);
            } else {
                reapply(state);
                if let Some(song) = state.requests.focus_after_change.take() {
                    focus_song_from_search(state, &song);
                }
            }
        }
        state.requests.focus_after_change = None;
    }
}

/// Nothing left to play: back to the wheel's usual sort, on the last song
/// played.
fn back_to_library(state: &mut State) {
    state.active_playlist_id = None;
    let sort = WheelSortMode::from(state.policy.interaction.initial_sort);
    // apply_wheel_sort ignores a sort that is already active.
    state.sort_mode = WheelSortMode::Playlist;
    apply_wheel_sort(state, if sort == WheelSortMode::Playlist { WheelSortMode::Group } else { sort });
    if let Some(song) = last_played_song(state) {
        focus_song_from_search(state, &song);
    }
}

/// The song the joined player last started, from their profile.
fn last_played_song(state: &State) -> Option<Arc<SongData>> {
    use deadsync_profile::PlayerSide;
    let side = [PlayerSide::P1, PlayerSide::P2]
        .into_iter()
        .find(|side| state.session.side_joined(*side))?;
    let profile = deadsync_profile::runtime_profile_for_side(side);
    let path = profile
        .last_played(state.session.play_style)
        .song_music_path
        .clone()?;
    state.group_entries.iter().find_map(|entry| match entry {
        MusicWheelEntry::Song(song)
            if song
                .music_path
                .as_ref()
                .is_some_and(|p| p.to_string_lossy() == path) =>
        {
            Some(Arc::clone(song))
        }
        _ => None,
    })
}

/// The request the wheel is on, as (its place in the queue, its place among
/// the playable ones), when the Requests view is showing.
fn highlighted(state: &State) -> Option<(usize, usize)> {
    if !showing(state) {
        return None;
    }
    let MusicWheelEntry::Song(song) = state.entries.get(state.selected_index)? else {
        return None;
    };
    state
        .requests
        .playable
        .iter()
        .position(|(_, playable)| Arc::ptr_eq(playable, song))
        .map(|at| (state.requests.playable[at].0, at))
}

/// The playable request after `at`, else the one before: where the wheel
/// lands once the request at `at` moves away.
fn neighbour(state: &State, at: usize) -> Option<Arc<SongData>> {
    let playable = &state.requests.playable;
    playable
        .get(at + 1)
        .or_else(|| at.checked_sub(1).and_then(|before| playable.get(before)))
        .map(|(_, song)| Arc::clone(song))
}

/// Cancel the highlighted request: it leaves the queue unplayed.
pub(super) fn cancel_highlighted(state: &mut State) -> Option<String> {
    let (index, at) = highlighted(state)?;
    let id = queue(state)[index].id.clone();
    state.requests.focus_after_change = neighbour(state, at);
    Some(id)
}

/// The footer's pending-requests chip: shown by the requests profile's name
/// while the game is linked.
pub(super) fn chip(state: &State) -> Option<RequestChip<'_>> {
    use deadsync_config::requestline::RequestlineSide;
    let view = state.requests.view.as_ref().filter(|view| view.enabled)?;
    let open = linked(state)?;
    let (side, idx) = match view.side {
        RequestlineSide::P1 => (ChipSide::P1, 0),
        RequestlineSide::P2 => (ChipSide::P2, 1),
    };
    if !state.session.joined[idx] || state.session.guest[idx] {
        return None;
    }
    Some(RequestChip {
        side,
        name: &state.profiles.display_names[idx],
        count: queue(state).len(),
        open,
        icon: view.icon,
        since_new: state
            .requests
            .grew_at
            .map(|at| state.session_elapsed - at),
    })
}

fn queue(state: &State) -> &[QueueEntry] {
    state
        .requests
        .view
        .as_ref()
        .and_then(|view| view.queue.as_deref())
        .map_or(&[], |q| q.entries.as_slice())
}

fn linked(state: &State) -> Option<bool> {
    match state.requests.view.as_ref().map(|view| &view.status) {
        Some(Status::Linked { requests_open, .. }) => Some(*requests_open),
        _ => None,
    }
}

/// Resolve the queue against the library and rebuild the Requests view.
fn rebuild(state: &mut State) {
    let stale_lookup = state
        .requests
        .lookup
        .as_ref()
        .is_none_or(|(built_for, _)| !Arc::ptr_eq(built_for, &state.group_entries));
    if stale_lookup {
        // Playlists resolve against the grouped catalog; so do requests.
        let lookup = build_playlist_song_lookup(&state.group_entries, &state.song_scan_roots);
        state.requests.lookup = Some((Arc::clone(&state.group_entries), lookup));
    }
    let Some((_, lookup)) = state.requests.lookup.as_ref() else {
        return;
    };
    let playable: Vec<(usize, Arc<SongData>)> = queue(state)
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            deadsync_simfile::playlist::find_song(lookup, &entry.song_id).map(|song| (index, song))
        })
        .collect();
    let entries: Arc<[MusicWheelEntry]> = if playable.is_empty() {
        Arc::from([])
    } else {
        let mut entries = Vec::with_capacity(playable.len() + 1);
        entries.push(MusicWheelEntry::PackHeader {
            name: crate::i18n::tr(S, "RequestsHeader"),
            original_index: 0,
            banner_path: None,
            song_count: playable.len(),
            pack_key: None,
            parent_series: None,
        });
        entries.extend(
            playable
                .iter()
                .map(|(_, song)| MusicWheelEntry::Song(Arc::clone(song))),
        );
        entries.into()
    };
    state.requests.playable = playable;
    state.requests.entries = entries;
}

/// The library was reloaded under the queue: resolve it again before use.
fn refresh_if_library_changed(state: &mut State) {
    if state
        .requests
        .lookup
        .as_ref()
        .is_some_and(|(built_for, _)| !Arc::ptr_eq(built_for, &state.group_entries))
    {
        rebuild(state);
    }
}

pub(super) fn showing(state: &State) -> bool {
    state.sort_mode == WheelSortMode::Playlist
        && state.active_playlist_id.as_deref() == Some(REQUESTS_PLAYLIST_ID)
}

/// The Requests view's entries for the playlist sort, or `None` when there is
/// nothing playable (the sort then falls back to Group).
pub(super) fn playlist_entries(state: &State) -> Option<Arc<[MusicWheelEntry]>> {
    (!state.requests.entries.is_empty()).then(|| Arc::clone(&state.requests.entries))
}

/// Apply the Requests view again with the current entries; the selected song
/// stays selected when it is still requested.
fn reapply(state: &mut State) {
    // apply_wheel_sort ignores a sort that is already active.
    state.sort_mode = WheelSortMode::Group;
    apply_wheel_sort(state, WheelSortMode::Playlist);
}

/// Show the Requests view. `false` when nothing in the queue is playable here.
fn show(state: &mut State) -> bool {
    refresh_if_library_changed(state);
    if state.requests.entries.is_empty() {
        return false;
    }
    state.active_playlist_id = Some(REQUESTS_PLAYLIST_ID.to_owned());
    reapply(state);
    showing(state)
}

/// Show the Requests view on the first request this library can play, with
/// its chart selected when the viewer picked one.
pub(super) fn play_next(state: &mut State) -> bool {
    refresh_if_library_changed(state);
    let Some((index, song)) = state.requests.playable.first().cloned() else {
        return false;
    };
    let chart_hash = queue(state)
        .get(index)
        .and_then(|entry| entry.chart.as_ref())
        .map(|chart| chart.hash.clone());
    if !show(state) {
        return false;
    }
    focus_song_from_search(state, &song);
    let chart_type = state.session.play_style.chart_type();
    if let Some(steps) = chart_hash
        .as_deref()
        .and_then(|hash| song.steps_index_for_chart_hash(chart_type, hash))
    {
        set_selected_steps_index_for_sync(state, steps);
    }
    true
}

/// The requests switch's new value: the opposite of what the page says now.
pub(super) fn toggled_open(state: &State) -> Option<bool> {
    linked(state).map(|open| !open)
}

fn item(top: Arc<str>, bottom: Arc<str>, action: select_music_menu::Action) -> select_music_menu::Item {
    select_music_menu::Item {
        top_label: TextContent::Shared(top),
        bottom_label: TextContent::Shared(bottom),
        action,
    }
}

/// Cancel for the request the wheel is on, listed first in the menu.
pub(super) fn highlighted_items(state: &State) -> Vec<select_music_menu::Item> {
    use select_music_menu::Action;
    let Some((index, _)) = highlighted(state) else {
        return Vec::new();
    };
    if linked(state).is_none() {
        return Vec::new();
    }
    let entry = &queue(state)[index];
    let title_for = crate::i18n::tr_fmt(
        S,
        "RequestTitleFor",
        &[("title", entry.title.as_str()), ("name", entry.name.as_str())],
    );
    vec![item(crate::i18n::tr(S, "CancelRequest"), title_for, Action::CancelRequest)]
}

/// The Requestline... category's items, or `None` when Requestline is off.
pub(super) fn menu_items(state: &State) -> Option<Vec<select_music_menu::Item>> {
    use select_music_menu::Action;
    let view = state.requests.view.as_ref().filter(|view| view.enabled)?;
    let Some(open) = linked(state) else {
        // Not connected: say why, and offer nothing to do.
        return Some(vec![item(
            crate::i18n::tr(S, "RequestlineStatus"),
            crate::screens::options::requestline_status_label(&view.status),
            Action::RequestlineStatus,
        )]);
    };

    let queue = queue(state);
    let count = queue.len().to_string();
    let queue_top = crate::i18n::tr_fmt(S, "RequestQueue", &[("count", &count)]);
    let next = state.requests.playable.first().map(|(index, _)| &queue[*index]);
    let queue_bottom = match next {
        Some(entry) => crate::i18n::tr_fmt(
            S,
            "NextRequest",
            &[("title", entry.title.as_str()), ("name", entry.name.as_str())],
        ),
        None if queue.is_empty() => crate::i18n::tr(S, "QueueIsEmpty"),
        None => crate::i18n::tr(S, "NoneInLibrary"),
    };
    // How things stand, and under it what pressing it does.
    let (switch_top, switch_bottom) = if open {
        ("RequestsStatusOpen", "TurnRequestsOff")
    } else {
        ("RequestsStatusClosed", "TurnRequestsOn")
    };
    Some(vec![
        item(queue_top, queue_bottom, Action::PlayNextRequest),
        item(
            crate::i18n::tr(S, switch_top),
            crate::i18n::tr(S, switch_bottom),
            Action::ToggleRequests,
        ),
    ])
}
