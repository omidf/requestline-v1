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
}

/// Whether the shell's view differs from what Select Music last applied.
pub fn requestline_view_stale(
    state: &State,
    enabled: bool,
    status: &Status,
    generation: u64,
) -> bool {
    state.requests.view.as_ref().is_none_or(|view| {
        view.enabled != enabled
            || &view.status != status
            || view.generation != generation
    })
}

pub(super) fn apply_view(state: &mut State, view: SelectMusicRequestlineView) {
    let queue_changed = state
        .requests
        .view
        .as_ref()
        .is_none_or(|old| old.queue != view.queue);
    state.requests.view = Some(view);
    if queue_changed {
        rebuild(state);
        if showing(state) {
            reapply(state);
        }
    }
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

pub(super) fn view_requests(state: &mut State) -> bool {
    show(state)
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
    let empty = crate::i18n::tr(S, "QueueIsEmpty");
    let next = state.requests.playable.first().map(|(index, _)| &queue[*index]);
    let next_label = match next {
        Some(entry) => crate::i18n::tr_fmt(
            S,
            "RequestTitleFor",
            &[("title", entry.title.as_str()), ("name", entry.name.as_str())],
        ),
        None => Arc::clone(&empty),
    };
    let missing = queue.len() - state.requests.playable.len();
    let count = queue.len().to_string();
    let missing_count = missing.to_string();
    let in_queue = match (queue.len(), missing) {
        (0, _) => Arc::clone(&empty),
        (_, 0) => crate::i18n::tr_fmt(S, "RequestsInQueue", &[("count", &count)]),
        _ => crate::i18n::tr_fmt(
            S,
            "RequestsInQueueMissing",
            &[("count", &count), ("missing", &missing_count)],
        ),
    };
    let (switch_top, switch_bottom) = if open {
        ("RequestsOn", "RequestsOnHelp")
    } else {
        ("RequestsOff", "RequestsOffHelp")
    };
    Some(vec![
        item(crate::i18n::tr(S, "PlayNextRequest"), next_label, Action::PlayNextRequest),
        item(crate::i18n::tr(S, "ViewRequests"), in_queue, Action::ViewRequests),
        item(
            crate::i18n::tr(S, switch_top),
            crate::i18n::tr(S, switch_bottom),
            Action::ToggleRequests,
        ),
    ])
}
