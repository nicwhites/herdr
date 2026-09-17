use super::*;

#[test]
fn pasted_help_and_copy_queries_normalize_single_line_text() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.overlay = Some(ClientShellOverlay::Help(ClientHelpOverlay {
        query: TextEditor::default(),
        search_focused: true,
        scroll: 0,
    }));

    assert!(state.insert_overlay_text("work\nspace"));
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Help(ClientHelpOverlay { ref query, .. }))
            if query.as_str() == "work space"
    ));

    state.overlay = None;
    state.mode = ClientShellMode::Copy;
    state.copy_mode = Some(ClientCopyModeState {
        pane_id: "pane_1".into(),
        content_revision: 0,
        geometry: (80, 24),
        alternate_screen_active: false,
        cursor: crate::api::schema::PaneTextPoint { row: 0, col: 0 },
        offset_from_bottom: 0,
        max_offset_from_bottom: 0,
        entry_offset_from_bottom: 0,
        selection: None,
        search_prompt: Some(ClientCopySearchPrompt {
            direction: crate::api::schema::PaneCopySearchDirection::Forward,
            query: TextEditor::default(),
            count: 1,
        }),
        search_query: String::new(),
        search_direction: None,
        search_matches: Vec::new(),
        search_total: 0,
        search_current: None,
        search_current_global: None,
        search_generation: 0,
        search_cache_generation: None,
        copy_after_search: false,
        pending_count: None,
        pending_find: None,
        pending_text_object: None,
        last_find: None,
    });

    assert!(state.insert_copy_search_text("needle\r\n", &mut ClientShellInput::default()));
    assert_eq!(
        state
            .copy_mode
            .as_ref()
            .and_then(|copy_mode| copy_mode.search_prompt.as_ref())
            .map(|prompt| prompt.query.as_str()),
        Some("needle ")
    );
}

#[test]
fn empty_keyboard_anchor_keeps_search_fallback_revision_guard() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.panes[0].scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 0,
        viewport_rows: 2,
    });
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("composed frame");
    state.handle_input_bytes(b"\x02[");
    let search = state.handle_input_bytes(b"/LIVE\r");
    let [ClientShellAction::Endpoint { request, .. }] = &search.actions[..] else {
        panic!("search request");
    };
    let request_id = request.id.clone();
    let found = crate::api::schema::PaneTextRange {
        start: crate::api::schema::PaneTextPoint { row: 0, col: 0 },
        end: crate::api::schema::PaneTextPoint { row: 0, col: 3 },
    };
    // The first keystroke's live search is answered after the prompt closed.
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(copy_search_result(Vec::new(), None)),
    );
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("latest queued live search should dispatch");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopySearch(params) if params.query == "LIVE"
    ));
    let request_id = request.id.clone();
    state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(copy_search_result(vec![found], Some(0))),
    );
    state.handle_input_bytes(b"v");
    assert!(!state.selection.as_ref().unwrap().is_visible());
    let copy = state.handle_input_bytes(b"y");
    assert!(copy.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(&request.method, crate::api::schema::Method::PaneSelectionRead(params)
                if params.anchor == found.start
                    && params.cursor == found.end
                    && params.content_revision == Some(0))
    )));
}

#[test]
fn copy_search_owns_prompt_repeat_highlights_selection_and_restore() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.panes[0].scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 20,
        viewport_rows: 2,
    });
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("composed frame");
    let mut enter = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CopyMode),
        &mut enter,
    );
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('?'),
        KeyModifiers::SHIFT,
    ))]);
    assert!(state.copy_mode.as_ref().is_some_and(|mode| {
        mode.search_prompt.as_ref().is_some_and(|prompt| {
            prompt.direction == crate::api::schema::PaneCopySearchDirection::Backward
        })
    }));
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    assert!(state
        .copy_mode
        .as_ref()
        .is_some_and(|mode| mode.search_prompt.is_none()));

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('/'),
        KeyModifiers::empty(),
    ))]);
    let typed = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "junk",
    ))]);
    let superseded_id = match &typed.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("prompt edits should dispatch live searches"),
    };
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
    ))]);
    state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "nee",
    ))]);
    state.handle_raw_events(vec![RawInputEvent::Paste("dleX".into())]);
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Backspace,
        KeyModifiers::empty(),
    ))]);
    assert_eq!(
        state
            .copy_mode
            .as_ref()
            .and_then(|mode| mode.search_prompt.as_ref())
            .map(|prompt| prompt.query.as_str()),
        Some("needle")
    );

    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &superseded_id,
        Ok(copy_search_result(Vec::new(), None)),
    );
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("latest queued search should dispatch after a superseded response");
    };
    let request_id = request.id.clone();
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopySearch(params)
            if params.pane_id == "pane_1"
                && params.query == "needle"
                && params.direction == crate::api::schema::PaneCopySearchDirection::Forward
                && params.cursor == origin
                && params.previous.is_none()
    ));
    let matches = vec![
        crate::api::schema::PaneTextRange {
            start: crate::api::schema::PaneTextPoint { row: 5, col: 2 },
            end: crate::api::schema::PaneTextPoint { row: 5, col: 7 },
        },
        crate::api::schema::PaneTextRange {
            start: crate::api::schema::PaneTextPoint { row: 15, col: 1 },
            end: crate::api::schema::PaneTextPoint { row: 15, col: 6 },
        },
    ];
    let (repaint, _) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(copy_search_result(matches.clone(), Some(0))),
    );
    assert!(repaint);

    let mut scrolled_surface = state.pane_surface.clone().expect("pane surface");
    scrolled_surface.panes[0]
        .scroll
        .as_mut()
        .expect("scroll metrics")
        .offset_from_bottom = 15;
    state.set_pane_surface(scrolled_surface);
    let frame = state.compose(106, 20).expect("live search frame");
    let hit = state.hits.panes[0].clone();
    let restored = frame.to_ratatui_buffer().expect("live search frame buffer");
    let highlighted = restored
        .cell((hit.inner_rect.x + 2, hit.inner_rect.y))
        .expect("highlighted search cell");
    assert_eq!(highlighted.bg, state.config.palette.accent);
    assert_eq!(
        state.copy_mode.as_ref().map(|mode| mode.cursor),
        Some(origin)
    );

    let commit = state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Enter,
        KeyModifiers::empty(),
    ))]);
    assert!(commit.actions.is_empty());
    assert_eq!(
        state.copy_mode.as_ref().map(|mode| mode.cursor),
        Some(matches[0].start)
    );

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('v'),
        KeyModifiers::empty(),
    ))]);
    let repeat = state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('n'),
        KeyModifiers::empty(),
    ))]);
    let [ClientShellAction::Endpoint { request, .. }] = &repeat.actions[..] else {
        panic!("repeat should use endpoint search");
    };
    let repeat_id = request.id.clone();
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopySearch(params)
            if params.direction == crate::api::schema::PaneCopySearchDirection::Forward
                && params.previous == Some(matches[0])
    ));
    let (_, repeat_actions) = state.handle_endpoint_result(
        "boot-1",
        &repeat_id,
        Ok(copy_search_result(matches.clone(), Some(1))),
    );
    if let Some(scroll_id) = repeat_actions.iter().find_map(|action| match action {
        ClientShellAction::Endpoint { request, .. }
            if matches!(request.method, crate::api::schema::Method::PaneScroll(_)) =>
        {
            Some(request.id.clone())
        }
        _ => None,
    }) {
        state.handle_endpoint_result("boot-1", &scroll_id, Ok(pane_scroll_result(6, 20, 2)));
    }
    assert_eq!(
        state.copy_mode.as_ref().map(|mode| mode.cursor.row),
        Some(15)
    );
    assert!(state
        .selection
        .as_ref()
        .is_some_and(crate::selection::Selection::is_visible));

    let reverse = state.handle_raw_events(vec![RawInputEvent::Key(
        crate::input::TerminalKey::new(KeyCode::Char('N'), KeyModifiers::SHIFT),
    )]);
    let [ClientShellAction::Endpoint { request, .. }] = &reverse.actions[..] else {
        panic!("reverse search should use endpoint search");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopySearch(params)
            if params.direction == crate::api::schema::PaneCopySearchDirection::Backward
                && params.previous == Some(matches[1])
    ));
    let (_, reverse_actions) = state.handle_endpoint_result(
        "boot-1",
        &request.id,
        Ok(copy_search_result(matches.clone(), Some(0))),
    );
    if let Some(scroll_id) = reverse_actions.iter().find_map(|action| match action {
        ClientShellAction::Endpoint { request, .. }
            if matches!(request.method, crate::api::schema::Method::PaneScroll(_)) =>
        {
            Some(request.id.clone())
        }
        _ => None,
    }) {
        state.handle_endpoint_result("boot-1", &scroll_id, Ok(pane_scroll_result(15, 20, 2)));
    }

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    assert_eq!(state.mode, ClientShellMode::Copy);
    assert!(state
        .copy_mode
        .as_ref()
        .is_some_and(|mode| mode.search_query.is_empty() && mode.selection.is_none()));
    let exit = state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(exit.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneScroll(params)
                    if params.offset_from_bottom == 0
            )
    )));
}

#[test]
fn superseded_live_search_skips_response_but_keeps_latest_moving() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.panes[0].scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 20,
        viewport_rows: 2,
    });
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("composed frame");
    let mut enter = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CopyMode),
        &mut enter,
    );
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('/'),
        KeyModifiers::empty(),
    ))]);
    let first = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "a",
    ))]);
    let first_id = match &first.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => {
            assert!(matches!(
                &request.method,
                crate::api::schema::Method::PaneCopySearch(params) if params.query == "a"
            ));
            request.id.clone()
        }
        _ => panic!("first live search should dispatch"),
    };

    let edit = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "b",
    ))]);
    assert!(edit.actions.is_empty());
    assert!(matches!(
        state.copy_operation_queue.front(),
        Some(ClientCopyOperation::Search { query, repeat: false, .. }) if query.as_str() == "ab"
    ));

    let found = vec![crate::api::schema::PaneTextRange {
        start: crate::api::schema::PaneTextPoint { row: 21, col: 0 },
        end: crate::api::schema::PaneTextPoint { row: 21, col: 2 },
    }];
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &first_id,
        Ok(copy_search_result(found.clone(), Some(0))),
    );
    let latest_id = match &actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => {
            assert!(matches!(
                &request.method,
                crate::api::schema::Method::PaneCopySearch(params) if params.query == "ab"
            ));
            request.id.clone()
        }
        _ => panic!("latest queued search should dispatch after supersession"),
    };
    assert!(state
        .copy_mode
        .as_ref()
        .is_some_and(|mode| mode.search_matches.is_empty() && mode.search_query.is_empty()));
    assert_eq!(
        state.copy_mode.as_ref().map(|mode| mode.cursor),
        Some(origin)
    );

    let mut changed = state.pane_surface.clone().expect("pane surface");
    changed.surface_revision += 1;
    changed.panes[0].content_revision = 2;
    state.set_pane_surface(changed);
    let (_, actions) =
        state.handle_endpoint_result("boot-1", &latest_id, Ok(copy_search_result(found, Some(0))));
    assert!(actions.is_empty());
    assert!(!state.copy_operation_in_flight);
    assert!(state.copy_operation_queue.is_empty());
    assert!(state
        .copy_mode
        .as_ref()
        .is_some_and(|mode| mode.search_matches.is_empty()));
}

#[test]
fn copy_search_prompt_esc_cancels_queued_and_in_flight_searches() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;

    state.handle_raw_events(vec![copy_mode_key('/')]);
    let first = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "a",
    ))]);
    let first_id = match &first.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => {
            assert!(matches!(
                &request.method,
                crate::api::schema::Method::PaneCopySearch(params) if params.query == "a"
            ));
            request.id.clone()
        }
        _ => panic!("first live search should dispatch"),
    };

    let edit = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "b",
    ))]);
    assert!(edit.actions.is_empty());
    assert!(matches!(
        state.copy_operation_queue.front(),
        Some(ClientCopyOperation::Search { .. })
    ));

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    assert!(state
        .copy_mode
        .as_ref()
        .is_some_and(|mode| mode.search_prompt.is_none()));
    assert!(state
        .copy_operation_queue
        .iter()
        .all(|operation| !matches!(operation, ClientCopyOperation::Search { .. })));

    let found = vec![crate::api::schema::PaneTextRange {
        start: crate::api::schema::PaneTextPoint { row: 21, col: 0 },
        end: crate::api::schema::PaneTextPoint { row: 21, col: 2 },
    }];
    let (_, actions) =
        state.handle_endpoint_result("boot-1", &first_id, Ok(copy_search_result(found, Some(0))));
    assert!(actions.is_empty());
    assert!(!state.copy_operation_in_flight);
    assert!(state.copy_operation_queue.is_empty());
    assert_eq!(
        state.copy_mode.as_ref().map(|mode| mode.cursor),
        Some(origin)
    );
}

#[test]
fn enter_commit_leaves_superseded_live_response_unapplied() {
    let mut state = copy_mode_state_with_scroll(0, 20);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;

    state.handle_raw_events(vec![copy_mode_key('/')]);
    let first = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "a",
    ))]);
    let first_id = match &first.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => {
            assert!(matches!(
                &request.method,
                crate::api::schema::Method::PaneCopySearch(params) if params.query == "a"
            ));
            request.id.clone()
        }
        _ => panic!("first live search should dispatch"),
    };

    let edit = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "b",
    ))]);
    assert!(edit.actions.is_empty());
    assert!(matches!(
        state.copy_operation_queue.front(),
        Some(ClientCopyOperation::Search { query, repeat: false, .. }) if query.as_str() == "ab"
    ));

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Enter,
        KeyModifiers::empty(),
    ))]);
    assert!(state
        .copy_mode
        .as_ref()
        .is_some_and(|mode| mode.search_prompt.is_none()));

    let found = vec![crate::api::schema::PaneTextRange {
        start: crate::api::schema::PaneTextPoint { row: 21, col: 0 },
        end: crate::api::schema::PaneTextPoint { row: 21, col: 2 },
    }];
    let (_, actions) =
        state.handle_endpoint_result("boot-1", &first_id, Ok(copy_search_result(found, Some(0))));
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("queued newer search should dispatch after supersession")
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopySearch(params)
            if params.query == "ab"
                && params.direction == crate::api::schema::PaneCopySearchDirection::Forward
    ));
    assert_eq!(
        state.copy_mode.as_ref().map(|mode| mode.cursor),
        Some(origin)
    );
    assert!(state
        .copy_mode
        .as_ref()
        .is_some_and(|mode| mode.search_query.is_empty() && mode.search_matches.is_empty()));
}

#[test]
fn enter_with_erased_query_invalidates_in_flight_search() {
    let mut state = copy_mode_state_with_scroll(0, 20);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;

    state.handle_raw_events(vec![copy_mode_key('/')]);
    let first = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "a",
    ))]);
    let first_id = match &first.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("first live search should dispatch"),
    };

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Backspace,
        KeyModifiers::empty(),
    ))]);
    assert!(state.copy_mode.as_ref().is_some_and(|mode| mode
        .search_prompt
        .as_ref()
        .is_some_and(|prompt| prompt.query.as_str().is_empty())));

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Enter,
        KeyModifiers::empty(),
    ))]);
    assert!(state
        .copy_mode
        .as_ref()
        .is_some_and(|mode| mode.search_prompt.is_none()));
    assert!(state
        .copy_operation_queue
        .iter()
        .all(|operation| !matches!(operation, ClientCopyOperation::Search { .. })));

    let found = vec![crate::api::schema::PaneTextRange {
        start: crate::api::schema::PaneTextPoint { row: 21, col: 0 },
        end: crate::api::schema::PaneTextPoint { row: 21, col: 2 },
    }];
    let (_, actions) =
        state.handle_endpoint_result("boot-1", &first_id, Ok(copy_search_result(found, Some(0))));
    assert!(actions.is_empty());
    assert_eq!(
        state.copy_mode.as_ref().map(|mode| mode.cursor),
        Some(origin)
    );
    assert!(state
        .copy_mode
        .as_ref()
        .is_some_and(|mode| mode.search_query.is_empty() && mode.search_matches.is_empty()));
}

#[test]
fn enter_after_edit_cycle_back_to_same_query_rejects_original_in_flight_search() {
    let mut state = copy_mode_state_with_scroll(0, 20);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;

    state.handle_raw_events(vec![copy_mode_key('/')]);
    let first = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "a",
    ))]);
    let first_id = match &first.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => {
            assert!(matches!(
                &request.method,
                crate::api::schema::Method::PaneCopySearch(params) if params.query == "a"
            ));
            request.id.clone()
        }
        _ => panic!("first live search should dispatch"),
    };

    // The re-created "a" query is a new search generation, not the original in-flight one.
    state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "b",
    ))]);
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Backspace,
        KeyModifiers::empty(),
    ))]);
    assert!(matches!(
        state.copy_operation_queue.front(),
        Some(ClientCopyOperation::Search { query, repeat: false, .. }) if query.as_str() == "a"
    ));

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Enter,
        KeyModifiers::empty(),
    ))]);

    let found = vec![crate::api::schema::PaneTextRange {
        start: crate::api::schema::PaneTextPoint { row: 21, col: 0 },
        end: crate::api::schema::PaneTextPoint { row: 21, col: 2 },
    }];
    let (_, actions) =
        state.handle_endpoint_result("boot-1", &first_id, Ok(copy_search_result(found, Some(0))));
    let replacement_id = match &actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => {
            assert!(matches!(
                &request.method,
                crate::api::schema::Method::PaneCopySearch(params) if params.query == "a"
            ));
            request.id.clone()
        }
        _ => panic!("same-query replacement should dispatch after supersession"),
    };
    assert_eq!(
        state.copy_mode.as_ref().map(|mode| mode.cursor),
        Some(origin)
    );
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &replacement_id,
        Ok(copy_search_result(
            vec![crate::api::schema::PaneTextRange {
                start: crate::api::schema::PaneTextPoint { row: 5, col: 2 },
                end: crate::api::schema::PaneTextPoint { row: 5, col: 7 },
            }],
            Some(0),
        )),
    );
    assert_eq!(
        state.copy_mode.as_ref().map(|mode| mode.cursor),
        Some(crate::api::schema::PaneTextPoint { row: 5, col: 2 })
    );
    assert!(actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(&request.method, crate::api::schema::Method::PaneScroll(_))
    )));
}

#[test]
fn new_content_revision_invalidates_copy_search_coordinates() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.panes[0].scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 0,
        viewport_rows: 2,
    });
    state.set_pane_surface(pane_surface.clone());
    state.compose(106, 20).expect("composed frame");
    let mut enter = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CopyMode),
        &mut enter,
    );
    let copy_mode = state.copy_mode.as_mut().expect("copy mode");
    copy_mode.search_query = "needle".into();
    copy_mode
        .search_matches
        .push(crate::api::schema::PaneTextRange {
            start: crate::api::schema::PaneTextPoint { row: 0, col: 0 },
            end: crate::api::schema::PaneTextPoint { row: 0, col: 1 },
        });
    copy_mode.search_total = 1;
    copy_mode.search_current = Some(0);
    copy_mode.search_current_global = Some(0);

    pane_surface.panes[0].content_revision = 2;
    state.set_pane_surface(pane_surface);
    let copy_mode = state.copy_mode.as_ref().expect("copy mode retained");
    assert!(copy_mode.search_matches.is_empty());
    assert_eq!(copy_mode.search_total, 0);
    assert_eq!(copy_mode.search_current, None);
}

#[test]
fn enter_commit_waits_for_fresh_search_instead_of_cached_result() {
    let mut state = copy_mode_state_with_scroll(0, 20);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;

    state.handle_raw_events(vec![copy_mode_key('2')]);
    state.handle_raw_events(vec![copy_mode_key('/')]);
    let first = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "a",
    ))]);
    let first_id = match &first.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("first live search should dispatch"),
    };

    state.handle_endpoint_result(
        "boot-1",
        &first_id,
        Ok(copy_search_result(
            vec![copy_match(21, 0), copy_match(25, 0)],
            Some(0),
        )),
    );
    assert!(state
        .copy_mode
        .as_ref()
        .is_some_and(|mode| mode.search_query == "a"));
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor, origin);

    let edit = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "b",
    ))]);
    let second_id = match &edit.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => {
            assert!(matches!(
                &request.method,
                crate::api::schema::Method::PaneCopySearch(params) if params.query == "ab"
            ));
            request.id.clone()
        }
        _ => panic!("edited query should dispatch after the first result settled"),
    };
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Backspace,
        KeyModifiers::empty(),
    ))]);
    assert!(matches!(
        state.copy_operation_queue.front(),
        Some(ClientCopyOperation::Search { query, repeat: false, .. }) if query.as_str() == "a"
    ));

    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Enter,
        KeyModifiers::empty(),
    ))]);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor, origin);

    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &second_id,
        Ok(copy_search_result(
            vec![copy_match(21, 0), copy_match(25, 0)],
            Some(0),
        )),
    );
    let replacement_id = match &actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => {
            assert!(matches!(
                &request.method,
                crate::api::schema::Method::PaneCopySearch(params) if params.query == "a"
            ));
            request.id.clone()
        }
        _ => panic!("fresh latest query should dispatch after Enter"),
    };
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor, origin);
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &replacement_id,
        Ok(copy_search_result(
            vec![copy_match(21, 0), copy_match(25, 0)],
            Some(0),
        )),
    );
    assert_eq!(
        state.copy_mode.as_ref().unwrap().cursor,
        copy_match(21, 0).start,
        "first fresh advance"
    );
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("count-1 repeat should dispatch after the fresh search");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopySearch(params) if params.query == "a"
    ));
    let repeat_id = request.id.clone();
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &repeat_id,
        Ok(copy_search_result(
            vec![copy_match(21, 0), copy_match(25, 0)],
            Some(1),
        )),
    );
    assert!(
        actions.is_empty(),
        "count 2 is exhausted after two advances"
    );
    assert_eq!(
        state.copy_mode.as_ref().unwrap().cursor,
        copy_match(25, 0).start,
        "no extra advance from the cached result"
    );
}

#[test]
fn cancelled_search_prompt_frees_the_pipeline_immediately() {
    let mut state = copy_mode_state_with_scroll(0, 20);
    state.handle_raw_events(vec![copy_mode_key('/')]);
    let first = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "a",
    ))]);
    let first_id = match &first.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("live search should dispatch"),
    };
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    assert!(!state.copy_operation_in_flight, "pipeline must be retired");
    state.handle_raw_events(vec![copy_mode_key('q')]);
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(state.copy_mode.is_none());
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &first_id,
        Ok(copy_search_result(vec![copy_match(21, 0)], Some(0))),
    );
    assert!(actions.is_empty(), "late success must be ignored");

    // A late timeout for the retired query must not raise a user notice.
    let mut state = copy_mode_state_with_scroll(0, 20);
    state.handle_raw_events(vec![copy_mode_key('/')]);
    let first = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "a",
    ))]);
    let first_id = match &first.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("live search should dispatch"),
    };
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &first_id,
        Err(ClientShellEndpointError {
            code: Some("endpoint_timeout".into()),
            message: "timed out".into(),
        }),
    );
    assert!(actions.is_empty());
    assert!(
        state.visible_endpoint_notice.is_none(),
        "retired pipelines must not emit bogus notices"
    );

    // Empty-query Enter frees the pipeline the same way.
    let mut state = copy_mode_state_with_scroll(0, 20);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;
    state.handle_raw_events(vec![copy_mode_key('/')]);
    let first = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "a",
    ))]);
    let first_id = match &first.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("live search should dispatch"),
    };
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Backspace,
        KeyModifiers::empty(),
    ))]);
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Enter,
        KeyModifiers::empty(),
    ))]);
    assert!(!state.copy_operation_in_flight);
    state.handle_raw_events(vec![copy_mode_key('k')]);
    assert_eq!(
        state.copy_mode.as_ref().unwrap().cursor.row,
        origin.row - 1,
        "motion keys must run immediately after cancellation"
    );
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &first_id,
        Ok(copy_search_result(vec![copy_match(21, 0)], Some(0))),
    );
    assert!(actions.is_empty());
    assert_eq!(
        state.copy_mode.as_ref().unwrap().cursor.row,
        origin.row - 1,
        "the stale response must not move the cursor"
    );
}

#[test]
fn coordinate_invalid_search_response_cancels_queued_searches() {
    let mut state = copy_mode_state_with_scroll(0, 20);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;

    state.handle_raw_events(vec![copy_mode_key('/')]);
    let first = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "a",
    ))]);
    let first_id = match &first.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("live search should dispatch"),
    };
    state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "b",
    ))]);
    assert!(matches!(
        state.copy_operation_queue.front(),
        Some(ClientCopyOperation::Search { query, .. }) if query.as_str() == "ab"
    ));

    let mut changed = state.pane_surface.clone().unwrap();
    changed.surface_revision += 1;
    changed.panes[0].content_revision = 2;
    state.set_pane_surface(changed);

    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &first_id,
        Ok(copy_search_result(vec![copy_match(21, 0)], Some(0))),
    );
    assert!(
        actions.is_empty(),
        "queued searches must not dispatch from a coordinate-invalid response"
    );
    assert!(state.copy_operation_queue.is_empty());
    assert!(!state.copy_operation_in_flight);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor, origin);
}

#[test]
fn cached_commit_requeries_after_surface_invalidation() {
    for invalidation in ["resize", "content"] {
        let mut state = copy_mode_state_with_scroll(0, 20);
        let origin = state.copy_mode.as_ref().expect("copy mode").cursor;
        state.handle_raw_events(vec![copy_mode_key('/')]);
        let typed = state.handle_raw_events(vec![RawInputEvent::Text(
            crate::input::TextCommit::new("a"),
        )]);
        let live_id = match &typed.actions[..] {
            [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
            _ => panic!("live search should dispatch"),
        };
        state.handle_endpoint_result(
            "boot-1",
            &live_id,
            Ok(copy_search_result(
                vec![copy_match(21, 0), copy_match(25, 0)],
                Some(0),
            )),
        );
        assert_eq!(state.copy_mode.as_ref().unwrap().cursor, origin);

        let mut changed = state.pane_surface.clone().unwrap();
        changed.surface_revision += 1;
        match invalidation {
            "resize" => changed.panes[0].inner_rect.width = 3,
            "content" => changed.panes[0].content_revision = 2,
            _ => unreachable!(),
        }
        state.set_pane_surface(changed);

        let commit = state.handle_raw_events(vec![RawInputEvent::Key(
            crate::input::TerminalKey::new(KeyCode::Enter, KeyModifiers::empty()),
        )]);
        let [ClientShellAction::Endpoint { request, .. }] = &commit.actions[..] else {
            panic!("Enter after {invalidation} must dispatch a fresh search");
        };
        assert!(matches!(
            &request.method,
            crate::api::schema::Method::PaneCopySearch(params) if params.query == "a"
        ));
    }
}

#[test]
fn valid_cached_no_match_enter_does_not_requery() {
    let mut state = copy_mode_state_with_scroll(0, 20);
    // An accepted no-match result is a valid cache: Enter must not send a duplicate query.
    for _ in 0..2 {
        state.handle_raw_events(vec![copy_mode_key('/')]);
        let typed = state.handle_raw_events(vec![RawInputEvent::Text(
            crate::input::TextCommit::new("a"),
        )]);
        let live_id = match &typed.actions[..] {
            [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
            _ => panic!("live search should dispatch"),
        };
        state.handle_endpoint_result("boot-1", &live_id, Ok(copy_search_result(Vec::new(), None)));
        assert!(state
            .copy_mode
            .as_ref()
            .is_some_and(|mode| mode.search_query == "a" && mode.search_matches.is_empty()));
        let commit = state.handle_raw_events(vec![RawInputEvent::Key(
            crate::input::TerminalKey::new(KeyCode::Enter, KeyModifiers::empty()),
        )]);
        assert!(
            commit.actions.is_empty(),
            "no duplicate query for a no-match"
        );
        assert!(state.copy_operation_queue.is_empty());
        assert!(state.pending_requests.is_empty());
        assert_eq!(state.mode, ClientShellMode::Copy);
    }
}
