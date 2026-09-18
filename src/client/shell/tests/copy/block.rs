use super::*;

#[test]
fn copy_mode_block_selection_anchors_extends_and_yanks_read_block() {
    let mut state = copy_mode_state_with_scroll(0, 20);
    let metrics = crate::pane::ScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 20,
        viewport_rows: 2,
    };

    // ctrl+v starts a blockwise selection anchored at the cursor.
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('v'),
        KeyModifiers::CONTROL,
    ))]);
    let origin = state.copy_mode.as_ref().unwrap().cursor;
    let anchor = state
        .copy_mode
        .as_ref()
        .and_then(|copy_mode| copy_mode.selection)
        .expect("block selection active");
    assert!(
        matches!(anchor, ClientCopySelection::Block { anchor } if anchor == origin),
        "unexpected anchor {anchor:?}"
    );

    // 2j 3l extends the block two rows down and three columns right.
    state.handle_raw_events(vec![copy_mode_key('2'), copy_mode_key('j')]);
    state.handle_raw_events(vec![copy_mode_key('3'), copy_mode_key('l')]);
    let end = state.copy_mode.as_ref().unwrap().cursor;
    let selection = state.selection.as_ref().expect("projected selection");
    assert!(selection.is_block());
    assert_eq!(
        selection.ordered_cells(),
        ((origin.row, origin.col), (end.row, end.col))
    );
    assert!(selection.contains(1, end.col, Some(metrics)), "inside cell");
    assert!(
        !selection.contains(1, end.col + 1, Some(metrics)),
        "same row outside band"
    );
    assert!(!selection.contains(4, 0, Some(metrics)), "row below block");

    // y yanks through the additive blockwise method and exits copy mode.
    let copy = state.handle_raw_events(vec![copy_mode_key('y')]);
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(state.copy_mode.is_none());
    assert!(state.selection.is_none());
    assert!(copy.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneSelectionReadBlock(params)
                    if params.pane_id == "pane_1"
                        && params.anchor.row == origin.row
                        && params.anchor.col == origin.col
                        && params.cursor.row == end.row
                        && params.cursor.col == end.col
                        && params.content_revision.is_none()
            )
    )));
}

#[test]
fn copy_mode_pressing_v_switches_block_to_character_and_esc_clears() {
    let mut state = copy_mode_state_with_scroll(0, 20);

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('v'),
        KeyModifiers::CONTROL,
    ))]);
    let origin = state.copy_mode.as_ref().unwrap().cursor;
    state.handle_raw_events(vec![copy_mode_key('j')]);
    let cursor = state.copy_mode.as_ref().unwrap().cursor;

    // Switching kind keeps the original anchor (vim behavior).
    state.handle_raw_events(vec![copy_mode_key('v')]);
    let selection = state
        .copy_mode
        .as_ref()
        .and_then(|copy_mode| copy_mode.selection)
        .expect("selection still active");
    assert!(
        matches!(selection, ClientCopySelection::Character { anchor } if anchor == origin),
        "unexpected selection {selection:?}"
    );
    assert_eq!(
        state
            .selection
            .as_ref()
            .map(crate::selection::Selection::ordered_cells),
        Some(((origin.row, origin.col), (cursor.row, cursor.col)))
    );
    assert!(!state.selection.as_ref().expect("selection").is_block());

    // Esc clears the selection without leaving copy mode.
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    assert_eq!(state.mode, ClientShellMode::Copy);
    assert!(state
        .copy_mode
        .as_ref()
        .is_some_and(|copy_mode| copy_mode.selection.is_none()));
    assert!(state.selection.is_none());
}
