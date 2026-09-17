use super::*;

#[test]
fn copy_mode_count_prefix_accumulates_and_zero_disambiguates() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    state.handle_raw_events(vec![copy_mode_key('2')]);
    assert_eq!(
        state.copy_mode.as_ref().unwrap().pending_count,
        Some(2),
        "digits start a count prefix without moving the cursor"
    );
    state.handle_raw_events(vec![copy_mode_key('0')]);
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_count, Some(20));

    state.handle_raw_events(vec![copy_mode_key('j')]);
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_count, None);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.row, 1);

    state.copy_mode.as_mut().unwrap().cursor.col = 2;
    state.handle_raw_events(vec![copy_mode_key('0')]);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.col, 0);
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_count, None);
}

#[test]
fn copy_mode_count_sends_one_atomic_horizontal_motion() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    state.copy_mode.as_mut().unwrap().cursor.col = 0;
    state.handle_raw_events(vec![copy_mode_key('2')]);
    let motion = state.handle_raw_events(vec![copy_mode_key('l')]);
    assert!(
        motion.actions.is_empty(),
        "counted h/l move locally without an endpoint request"
    );
    assert_eq!(state.pending_requests.len(), 0);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.col, 2);

    state.handle_raw_events(vec![copy_mode_key('3')]);
    state.handle_raw_events(vec![copy_mode_key('h')]);
    assert_eq!(
        state.copy_mode.as_ref().unwrap().cursor.col,
        0,
        "3h clamps at column 0"
    );
}

#[test]
fn copy_mode_count_queues_repeated_word_motions() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;
    state.handle_raw_events(vec![copy_mode_key('3')]);
    let motion = state.handle_raw_events(vec![copy_mode_key('w')]);
    let [ClientShellAction::Endpoint { request, .. }] = &motion.actions[..] else {
        panic!("counted word motion should dispatch the first endpoint request");
    };
    let mut request_id = request.id.clone();

    for step in 1..=3 {
        let (repaint, actions) = state.handle_endpoint_result(
            "boot-1",
            &request_id,
            Ok(crate::api::schema::ResponseResult::PaneCopyMotion {
                pane_id: "pane_1".into(),
                cursor: crate::api::schema::PaneTextPoint {
                    row: origin.row,
                    col: origin.col + step,
                },
                content_revision: 0,
            }),
        );
        assert!(repaint);
        if step == 3 {
            assert!(actions.is_empty(), "queue must be drained after 3 motions");
        } else {
            let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
                panic!("expected the next queued motion, {} remaining", 3 - step);
            };
            assert!(matches!(
                &request.method,
                crate::api::schema::Method::PaneCopyMotion(params)
                    if params.motion == crate::api::schema::PaneCopyMotion::NextWordStart
            ));
            request_id = request.id.clone();
        }
    }
}

#[test]
fn copy_mode_count_moves_to_later_line_before_line_end() {
    let mut state = copy_mode_state_with_scroll(20, 20);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.row, 1);
    state.handle_raw_events(vec![copy_mode_key('3')]);
    let motion = state.handle_raw_events(vec![copy_mode_key('$')]);
    assert!(motion.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneCopyMotion(params)
                    if params.cursor.row == 3
                        && params.motion == crate::api::schema::PaneCopyMotion::LineEnd
            )
    )));
}

#[test]
fn copy_mode_count_g_jumps_to_absolute_row_and_reveals() {
    let mut state = copy_mode_state_with_scroll(0, 20);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.row, 21);
    state.handle_raw_events(vec![copy_mode_key('5')]);
    state.handle_raw_events(vec![copy_mode_key('G')]);
    let copy_mode = state.copy_mode.as_ref().unwrap();
    assert_eq!(copy_mode.cursor.row, 4, "5G jumps to absolute row 4");
    assert_eq!(
        copy_mode.offset_from_bottom, 16,
        "scrolling must follow the jumped cursor"
    );

    state.handle_raw_events(vec![copy_mode_key('G')]);
    state.handle_raw_events(vec![copy_mode_key('5')]);
    state.handle_raw_events(vec![copy_mode_key('g')]);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.row, 4);
}

#[test]
fn copy_mode_h_m_l_jump_within_the_viewport_and_keep_the_column() {
    let mut state = copy_mode_state_with_scroll(0, 20);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.row, 21);

    state.handle_raw_events(vec![copy_mode_key('H')]);
    let copy_mode = state.copy_mode.as_ref().unwrap();
    assert_eq!(copy_mode.cursor.row, 20);
    assert_eq!(copy_mode.cursor.col, 1);

    state.handle_raw_events(vec![copy_mode_key('M')]);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.row, 21);

    state.handle_raw_events(vec![copy_mode_key('L')]);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.row, 21);

    state.handle_raw_events(vec![copy_mode_key('2')]);
    state.handle_raw_events(vec![copy_mode_key('H')]);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.row, 21);

    state.handle_raw_events(vec![copy_mode_key('2')]);
    state.handle_raw_events(vec![copy_mode_key('L')]);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor.row, 20);
}

#[test]
fn copy_mode_count_queues_repeated_searches() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    state.handle_raw_events(vec![copy_mode_key('3')]);
    state.handle_raw_events(vec![copy_mode_key('/')]);
    assert_eq!(
        state
            .copy_mode
            .as_ref()
            .and_then(|copy_mode| copy_mode.search_prompt.as_ref())
            .map(|prompt| prompt.count),
        Some(3)
    );
    let typed = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "needle",
    ))]);
    assert!(matches!(
        &typed.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneCopySearch(params)
                    if params.query == "needle" && params.previous.is_none()
            )
    ));
    let commit = state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Enter,
        KeyModifiers::empty(),
    ))]);
    // Enter queues the repeats as one compact chain entry.
    assert!(commit.actions.is_empty());
    assert_eq!(state.copy_operation_queue.len(), 1);
    assert!(matches!(
        state.copy_operation_queue.front(),
        Some(ClientCopyOperation::Search {
            repeat: true,
            remaining: 2,
            ..
        })
    ));
}

#[test]
fn copy_mode_count_sends_one_atomic_find() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    state.handle_raw_events(vec![copy_mode_key('3')]);
    state.handle_raw_events(vec![copy_mode_key('f')]);
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_count, Some(3));
    let find = state.handle_raw_events(vec![copy_mode_key('o')]);
    let [ClientShellAction::Endpoint { request, .. }] = &find.actions[..] else {
        panic!("3fo should dispatch one find");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyObject(params)
            if params.request
                == crate::api::schema::PaneCopyObjectRequest::Find {
                    ch: 'o',
                    direction: crate::api::schema::PaneCopySearchDirection::Forward,
                    till: false,
                    count: 3,
                    repeat: false,
                }
    ));
    assert!(state.copy_operation_queue.is_empty());
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_count, None);
}

#[test]
fn copy_mode_counted_text_object_is_one_outer_object_request() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    state.handle_raw_events(vec![copy_mode_key('2')]);
    state.handle_raw_events(vec![copy_mode_key('i')]);
    let object = state.handle_raw_events(vec![copy_mode_key('(')]);
    assert!(matches!(
        &object.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneCopyObject(params)
                    if params.request
                        == crate::api::schema::PaneCopyObjectRequest::Object {
                            inside: true,
                            open: '(',
                            close: ')',
                            count: 2,
                            word: None,
                        }
            )
    ));
    assert!(state.copy_operation_queue.is_empty());
}

#[test]
fn copy_mode_esc_cancels_standalone_count_without_exiting() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    state.handle_raw_events(vec![copy_mode_key('3')]);
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    assert_eq!(state.mode, ClientShellMode::Copy);
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_count, None);
}

#[test]
fn copy_repeat_counts_are_compact_lazy_and_terminating() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;
    for digit in "10000".chars() {
        state.handle_raw_events(vec![copy_mode_key(digit)]);
    }
    let motion = state.handle_raw_events(vec![copy_mode_key('w')]);
    let request_id = match &motion.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("counted word motion should dispatch"),
    };
    assert!(
        state.copy_operation_queue.is_empty(),
        "repeat chains must not preallocate one entry per count"
    );
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneCopyMotion {
            pane_id: "pane_1".into(),
            cursor: origin,
            content_revision: 0,
        }),
    );
    assert!(
        actions.is_empty(),
        "no-progress motion terminates the chain"
    );
    assert!(state.copy_operation_queue.is_empty());
    assert!(!state.copy_operation_in_flight);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor, origin);

    // Esc before any response cancels the active chain immediately.
    let mut state = copy_mode_state_with_scroll(0, 20);
    for digit in "20000".chars() {
        state.handle_raw_events(vec![copy_mode_key(digit)]);
    }
    let motion = state.handle_raw_events(vec![copy_mode_key('w')]);
    let request_id = match &motion.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("counted word motion should dispatch"),
    };
    assert!(state.copy_operation_queue.is_empty());
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(state.copy_mode.is_none());
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneCopyMotion {
            pane_id: "pane_1".into(),
            cursor: crate::api::schema::PaneTextPoint { row: 5, col: 2 },
            content_revision: 0,
        }),
    );
    assert!(actions.is_empty(), "late completion must be ignored");

    let mut state = copy_mode_state_with_scroll(0, 20);
    for digit in "10000".chars() {
        state.handle_raw_events(vec![copy_mode_key(digit)]);
    }
    state.handle_raw_events(vec![copy_mode_key('/')]);
    let typed = state.handle_raw_events(vec![RawInputEvent::Text(crate::input::TextCommit::new(
        "needle in a long haystack",
    ))]);
    let live_id = match &typed.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("live search should dispatch"),
    };
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Enter,
        KeyModifiers::empty(),
    ))]);
    assert_eq!(
        state.copy_operation_queue.len(),
        1,
        "a large search count must stay a single compact entry"
    );
    assert!(matches!(
        state.copy_operation_queue.front(),
        Some(ClientCopyOperation::Search {
            repeat: true,
            remaining: 9999,
            ..
        })
    ));
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &live_id,
        Ok(copy_search_result(
            vec![copy_match(21, 0), copy_match(25, 0)],
            Some(0),
        )),
    );
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("the compact repeat chain should dispatch after the live result");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopySearch(params)
            if params.previous == Some(copy_match(21, 0))
    ));
}
