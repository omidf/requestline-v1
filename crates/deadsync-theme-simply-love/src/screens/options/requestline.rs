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
    match &state.requestline_status {
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
