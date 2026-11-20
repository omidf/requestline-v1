//! requestline rows inside ArrowCloud Options: on/off, which player's
//! ArrowCloud profile to use, and a read-only status line.
use super::*;
use deadsync_config::requestline::RequestlineSide;
use deadsync_online::requestline::Status;

pub(super) const fn requestline_side_choice_index(side: RequestlineSide) -> usize {
    match side {
        RequestlineSide::P1 => 0,
        RequestlineSide::P2 => 1,
    }
}

pub(super) const fn requestline_side_from_choice(idx: usize) -> RequestlineSide {
    match idx {
        1 => RequestlineSide::P2,
        _ => RequestlineSide::P1,
    }
}

pub(super) fn requestline_effect(state: &State) -> ThemeEffect {
    online_config_effect(crate::SimplyLoveOnlineConfigRequest::Requestline(
        state.requestline,
    ))
}

/// A new-request alert row changed: store it. `None` for any other row;
/// otherwise the sound to preview, if this was the sound row.
pub(super) fn apply_requestline_alert_choice(
    state: &mut State,
    row: SubRowId,
    index: usize,
) -> Option<Option<&'static str>> {
    use deadsync_config::requestline::{
        RequestlineSound, RequestlineSoundTiming, RequestlineToastCorner, RequestlineVisual,
    };
    let alerts = &mut state.requestline;
    match row {
        SubRowId::RequestlineNotifySound => {
            alerts.sound = RequestlineSound::from_index(index);
            return Some(alerts.sound.asset_path());
        }
        SubRowId::RequestlineSoundTiming => {
            alerts.sound_timing = RequestlineSoundTiming::from_index(index);
        }
        SubRowId::RequestlineNotifyVisual => alerts.visual = RequestlineVisual::from_index(index),
        SubRowId::RequestlineToastCorner => alerts.corner = RequestlineToastCorner::from_index(index),
        _ => return None,
    }
    Some(None)
}

/// With Requestline off only its on/off row shows. With it on, the
/// sound-timing row only matters with a sound on, the corner row only with a
/// toast on.
pub(super) fn requestline_row_shown(state: &State, row: SubRowId) -> bool {
    use deadsync_config::requestline::{RequestlineSound, RequestlineVisual};
    let choice = |id| {
        get_choice_by_id(
            &state.sub[SubmenuKind::ArrowCloud].choice_indices,
            ARROWCLOUD_OPTIONS_ROWS,
            id,
        )
        .unwrap_or(0)
    };
    let refines_requestline = matches!(
        row,
        SubRowId::RequestlineProfile
            | SubRowId::RequestlineStatus
            | SubRowId::RequestlineNotifySound
            | SubRowId::RequestlineSoundTiming
            | SubRowId::RequestlineNotifyVisual
            | SubRowId::RequestlineToastCorner
    );
    if refines_requestline && !yes_no_from_choice(choice(SubRowId::RequestlineEnabled)) {
        return false;
    }
    match row {
        SubRowId::RequestlineSoundTiming => {
            RequestlineSound::from_index(choice(SubRowId::RequestlineNotifySound))
                != RequestlineSound::Off
        }
        SubRowId::RequestlineToastCorner => {
            RequestlineVisual::from_index(choice(SubRowId::RequestlineNotifyVisual))
                != RequestlineVisual::Off
        }
        _ => true,
    }
}

/// requestline is an ArrowCloud feature: its rows go dim while ArrowCloud is off.
pub(super) fn arrowcloud_enabled(state: &State) -> bool {
    get_choice_by_id(
        &state.sub[SubmenuKind::ArrowCloud].choice_indices,
        ARROWCLOUD_OPTIONS_ROWS,
        SubRowId::EnableArrowCloud,
    )
    .is_some_and(yes_no_from_choice)
}

/// Shell hands Options the latest requestline status each frame it is open.
pub fn sync_requestline_status(state: &mut State, status: Status) {
    if state.requestline_status != status {
        state.requestline_status = status;
        clear_render_cache(state);
    }
}

/// The read-only text on the Requests Status row.
pub(super) fn requestline_status_text(state: &State) -> Arc<str> {
    const S: &str = "OptionsRequestline";
    if !arrowcloud_enabled(state) {
        return tr(S, "StatusArrowCloudOff");
    }
    if !state.requestline.enabled {
        return tr(S, "StatusOff");
    }
    requestline_status_label(&state.requestline_status)
}

/// What a client status means, in the words the Requests Status row uses.
/// Select Music's Requestline... menu says the same when not connected.
pub(crate) fn requestline_status_label(status: &Status) -> Arc<str> {
    const S: &str = "OptionsRequestline";
    match status {
        // On, but nothing has been checked yet this session.
        Status::Off => tr(S, "StatusPending"),
        Status::NoKey => tr(S, "StatusNoKey"),
        Status::Checking => tr(S, "StatusChecking"),
        Status::Linked {
            page,
            requests_open: true,
        } => tr_fmt(S, "StatusLinkedOpen", &[("page", page.as_str())]),
        Status::Linked { page, .. } => tr_fmt(S, "StatusLinkedClosed", &[("page", page.as_str())]),
        Status::NotSetUp => tr(S, "StatusNotSetUp"),
        Status::BadKey => tr(S, "StatusBadKey"),
        Status::Upgrade => tr(S, "StatusUpgrade"),
        Status::Unreachable => tr(S, "StatusUnreachable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_index(id: SubRowId) -> usize {
        ARROWCLOUD_OPTIONS_ROWS
            .iter()
            .position(|row| row.id == id)
            .unwrap()
    }

    fn set_arrowcloud(state: &mut State, enabled: bool) {
        set_choice_by_id(
            &mut state.sub[SubmenuKind::ArrowCloud].choice_indices,
            ARROWCLOUD_OPTIONS_ROWS,
            SubRowId::EnableArrowCloud,
            yes_no_choice_index(enabled),
        );
    }

    #[test]
    fn requestline_rows_emit_persistable_settings() {
        let mut state = super::super::tests::init();
        set_arrowcloud(&mut state, true);
        state.view = OptionsView::Submenu(SubmenuKind::ArrowCloud);
        state.sub_selected = row_index(SubRowId::RequestlineEnabled);
        let effect = apply_submenu_choice_delta(&mut state, &AssetManager::new(), 1, NavWrap::Wrap);
        assert!(effect.is_some());
        assert!(state.requestline.enabled);
        state.sub_selected = row_index(SubRowId::RequestlineProfile);
        apply_submenu_choice_delta(&mut state, &AssetManager::new(), 1, NavWrap::Wrap);
        assert_eq!(state.requestline.side, RequestlineSide::P2);
    }

    fn visible(state: &State) -> Vec<SubRowId> {
        submenu_visible_row_indices(state, SubmenuKind::ArrowCloud, ARROWCLOUD_OPTIONS_ROWS)
            .into_iter()
            .map(|i| ARROWCLOUD_OPTIONS_ROWS[i].id)
            .collect()
    }

    fn set_requestline(state: &mut State, enabled: bool) {
        set_choice_by_id(
            &mut state.sub[SubmenuKind::ArrowCloud].choice_indices,
            ARROWCLOUD_OPTIONS_ROWS,
            SubRowId::RequestlineEnabled,
            yes_no_choice_index(enabled),
        );
    }

    #[test]
    fn requestline_off_hides_everything_but_its_switch() {
        use deadsync_config::requestline::{RequestlineSound, RequestlineVisual};
        let mut state = super::super::tests::init();
        set_arrowcloud(&mut state, true);
        // Every refinement on, so only the switch decides.
        for (row, index) in [
            (SubRowId::RequestlineNotifySound, RequestlineSound::Coin.index()),
            (SubRowId::RequestlineNotifyVisual, RequestlineVisual::Notes.index()),
        ] {
            set_choice_by_id(
                &mut state.sub[SubmenuKind::ArrowCloud].choice_indices,
                ARROWCLOUD_OPTIONS_ROWS,
                row,
                index,
            );
        }
        let requestline_rows = [
            SubRowId::RequestlineProfile,
            SubRowId::RequestlineStatus,
            SubRowId::RequestlineNotifySound,
            SubRowId::RequestlineSoundTiming,
            SubRowId::RequestlineNotifyVisual,
            SubRowId::RequestlineToastCorner,
        ];
        set_requestline(&mut state, false);
        let shown = visible(&state);
        assert!(shown.contains(&SubRowId::RequestlineEnabled));
        assert!(shown.contains(&SubRowId::EnableArrowCloud));
        for row in requestline_rows {
            assert!(!shown.contains(&row), "{row:?} shown while Requestline is off");
        }
        set_requestline(&mut state, true);
        let shown = visible(&state);
        for row in requestline_rows {
            assert!(shown.contains(&row), "{row:?} hidden while Requestline is on");
        }
    }

    #[test]
    fn alert_rows_store_settings_and_preview_the_sound() {
        use deadsync_config::requestline::{RequestlineSound, RequestlineVisual};
        let mut state = super::super::tests::init();
        set_arrowcloud(&mut state, true);
        set_requestline(&mut state, true);
        state.view = OptionsView::Submenu(SubmenuKind::ArrowCloud);
        // Off by default, so the rows that refine them stay out of the way.
        assert!(!visible(&state).contains(&SubRowId::RequestlineSoundTiming));
        assert!(!visible(&state).contains(&SubRowId::RequestlineToastCorner));

        state.sub_selected = visible(&state)
            .iter()
            .position(|id| *id == SubRowId::RequestlineNotifySound)
            .unwrap();
        state.pending_sfx.clear();
        let effect = apply_submenu_choice_delta(&mut state, &AssetManager::new(), 1, NavWrap::Wrap);
        assert!(effect.is_some(), "persisted like the other requestline rows");
        assert_eq!(state.requestline.sound, RequestlineSound::Coin);
        assert!(
            state.pending_sfx.contains(&"assets/sounds/requestline/coin.ogg"),
            "{:?}",
            state.pending_sfx
        );
        assert!(visible(&state).contains(&SubRowId::RequestlineSoundTiming));

        state.sub_selected = visible(&state)
            .iter()
            .position(|id| *id == SubRowId::RequestlineNotifyVisual)
            .unwrap();
        apply_submenu_choice_delta(&mut state, &AssetManager::new(), 1, NavWrap::Wrap);
        assert_eq!(state.requestline.visual, RequestlineVisual::Notes);
        assert!(visible(&state).contains(&SubRowId::RequestlineToastCorner));
    }

    #[test]
    fn requestline_rows_are_disabled_while_arrowcloud_is_off() {
        let mut state = super::super::tests::init();
        set_arrowcloud(&mut state, false);
        assert!(is_submenu_row_disabled(
            &state,
            SubmenuKind::ArrowCloud,
            SubRowId::RequestlineEnabled
        ));
        assert_eq!(
            requestline_status_text(&state),
            tr("OptionsRequestline", "StatusArrowCloudOff")
        );
        set_arrowcloud(&mut state, true);
        assert!(!is_submenu_row_disabled(
            &state,
            SubmenuKind::ArrowCloud,
            SubRowId::RequestlineEnabled
        ));
    }

    #[test]
    fn requestline_status_row_follows_settings_and_server() {
        let mut state = super::super::tests::init();
        set_arrowcloud(&mut state, true);
        state.requestline.enabled = false;
        sync_requestline_status(&mut state, Status::NotSetUp);
        assert_eq!(
            requestline_status_text(&state),
            tr("OptionsRequestline", "StatusOff")
        );
        state.requestline.enabled = true;
        assert_eq!(
            requestline_status_text(&state),
            tr("OptionsRequestline", "StatusNotSetUp")
        );
        sync_requestline_status(
            &mut state,
            Status::Linked {
                page: "bromid".into(),
                requests_open: true,
            },
        );
        assert_eq!(
            requestline_status_text(&state),
            tr_fmt(
                "OptionsRequestline",
                "StatusLinkedOpen",
                &[("page", "bromid")]
            )
        );
    }
}
