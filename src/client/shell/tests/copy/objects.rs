use super::*;

#[test]
fn non_character_motion_keys_clear_pending_find_and_text_object_prefixes() {
    let mut state = copy_mode_state_with_scroll(0, 20);
    let left_key = || {
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Left,
            KeyModifiers::empty(),
        ))
    };

    state.handle_raw_events(vec![copy_mode_key('2')]);
    state.handle_raw_events(vec![copy_mode_key('f')]);
    assert_eq!(
        state.copy_mode.as_ref().unwrap().pending_find,
        Some((true, false))
    );
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_count, Some(2));
    let left = state.handle_raw_events(vec![left_key()]);
    assert!(left.actions.is_empty(), "Left moves locally, no endpoint");
    assert_eq!(
        state.copy_mode.as_ref().unwrap().cursor.col,
        0,
        "2h clamps at column 0"
    );
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_find, None);
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_count, None);

    let x = state.handle_raw_events(vec![copy_mode_key('x')]);
    assert!(x.actions.is_empty());
    assert!(state.copy_operation_queue.iter().all(|operation| {
        !matches!(
            operation,
            ClientCopyOperation::CopyObject(crate::api::schema::PaneCopyObjectRequest::Find { .. })
        )
    }));
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_find, None);

    state.handle_raw_events(vec![copy_mode_key('i')]);
    assert_eq!(
        state.copy_mode.as_ref().unwrap().pending_text_object,
        Some(false)
    );
    let left = state.handle_raw_events(vec![left_key()]);
    assert!(left.actions.is_empty(), "Left moves locally, no endpoint");
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_text_object, None);
    let brace = state.handle_raw_events(vec![copy_mode_key('{')]);
    let [ClientShellAction::Endpoint { request, .. }] = &brace.actions[..] else {
        panic!("brace after Left should dispatch a paragraph motion")
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyMotion(params)
            if params.motion == crate::api::schema::PaneCopyMotion::PreviousParagraph
    ));
}

#[test]
fn copy_mode_find_till_and_repeats_use_the_copy_object_endpoint() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;

    state.handle_raw_events(vec![copy_mode_key('f')]);
    let find = state.handle_raw_events(vec![copy_mode_key('o')]);
    let [ClientShellAction::Endpoint { request, .. }] = &find.actions[..] else {
        panic!("find should dispatch a copy-object request");
    };
    let request_id = request.id.clone();
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyObject(params)
            if params.cursor == origin
                && params.request
                    == crate::api::schema::PaneCopyObjectRequest::Find {
                        ch: 'o',
                        direction: crate::api::schema::PaneCopySearchDirection::Forward,
                        till: false,
                        count: 1,
                        repeat: false,
                    }
    ));
    let found = crate::api::schema::PaneTextPoint {
        row: origin.row,
        col: 3,
    };
    let (repaint, _) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneCopyObject {
            pane_id: "pane_1".into(),
            range: crate::api::schema::PaneTextRange {
                start: found,
                end: found,
            },
            content_revision: 0,
        }),
    );
    assert!(repaint);
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor, found);

    // `;` repeats the last find forward.
    let repeat = state.handle_raw_events(vec![copy_mode_key(';')]);
    let [ClientShellAction::Endpoint { request, .. }] = &repeat.actions[..] else {
        panic!("; should repeat the last find");
    };
    let request_id = request.id.clone();
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyObject(params)
            if params.request
                == crate::api::schema::PaneCopyObjectRequest::Find {
                    ch: 'o',
                    direction: crate::api::schema::PaneCopySearchDirection::Forward,
                    till: false,
                    count: 1,
                    repeat: true,
                }
    ));

    // `,` repeats it reversed.
    let _ = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneCopyObject {
            pane_id: "pane_1".into(),
            range: crate::api::schema::PaneTextRange {
                start: found,
                end: found,
            },
            content_revision: 0,
        }),
    );
    let reverse = state.handle_raw_events(vec![copy_mode_key(',')]);
    let [ClientShellAction::Endpoint { request, .. }] = &reverse.actions[..] else {
        panic!(", should repeat the last find reversed");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyObject(params)
            if params.request
                == crate::api::schema::PaneCopyObjectRequest::Find {
                    ch: 'o',
                    direction: crate::api::schema::PaneCopySearchDirection::Backward,
                    till: false,
                    count: 1,
                    repeat: true,
                }
    ));
}

#[test]
fn copy_mode_pending_text_object_selects_the_object_range() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;

    state.handle_raw_events(vec![copy_mode_key('i')]);
    let object = state.handle_raw_events(vec![copy_mode_key('{')]);
    let [ClientShellAction::Endpoint { request, .. }] = &object.actions[..] else {
        panic!("i + {{ should dispatch a copy-object request");
    };
    let request_id = request.id.clone();
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyObject(params)
            if params.cursor == origin
                && params.request
                    == crate::api::schema::PaneCopyObjectRequest::Object {
                        inside: true,
                        open: '{',
                        close: '}',
                        count: 1,
                        word: None,
                    }
    ));
    let range = crate::api::schema::PaneTextRange {
        start: crate::api::schema::PaneTextPoint {
            row: origin.row,
            col: 1,
        },
        end: crate::api::schema::PaneTextPoint {
            row: origin.row,
            col: 4,
        },
    };
    let (repaint, _) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneCopyObject {
            pane_id: "pane_1".into(),
            range,
            content_revision: 0,
        }),
    );
    assert!(repaint);
    let copy_mode = state.copy_mode.as_ref().unwrap();
    assert_eq!(
        copy_mode.selection,
        Some(ClientCopySelection::Character {
            anchor: range.start
        })
    );
    assert_eq!(copy_mode.cursor, range.end);
    assert!(state.selection.is_some(), "y must have a live range");

    state.handle_raw_events(vec![copy_mode_key('a')]);
    let around = state.handle_raw_events(vec![copy_mode_key('(')]);
    let [ClientShellAction::Endpoint { request, .. }] = &around.actions[..] else {
        panic!("a( should dispatch a copy-object request");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyObject(params)
            if params.request
                == crate::api::schema::PaneCopyObjectRequest::Object {
                    inside: false,
                    open: '(',
                    close: ')',
                    count: 1,
                    word: None,
                }
    ));
    let _ = state.handle_endpoint_result(
        "boot-1",
        &request.id.clone(),
        Ok(crate::api::schema::ResponseResult::PaneCopyObject {
            pane_id: "pane_1".into(),
            range,
            content_revision: 0,
        }),
    );

    state.handle_raw_events(vec![copy_mode_key('i')]);
    assert_eq!(
        state.copy_mode.as_ref().unwrap().pending_text_object,
        Some(false)
    );
    state.handle_raw_events(vec![copy_mode_key('x')]);
    assert_eq!(state.copy_mode.as_ref().unwrap().pending_text_object, None);
}

#[test]
fn copy_mode_closing_delimiter_routes_to_the_open_close_pair() {
    let mut state = copy_mode_state_with_scroll(0, 0);

    state.handle_raw_events(vec![copy_mode_key('i')]);
    let inside = state.handle_raw_events(vec![copy_mode_key('}')]);
    let [ClientShellAction::Endpoint { request, .. }] = &inside.actions[..] else {
        panic!("i + }} should dispatch a copy-object request");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyObject(params)
            if params.request
                == crate::api::schema::PaneCopyObjectRequest::Object {
                    inside: true,
                    open: '{',
                    close: '}',
                    count: 1,
                    word: None,
                }
    ));
    let request_id = request.id.clone();
    let _ = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneCopyObject {
            pane_id: "pane_1".into(),
            range: crate::api::schema::PaneTextRange {
                start: crate::api::schema::PaneTextPoint { row: 0, col: 0 },
                end: crate::api::schema::PaneTextPoint { row: 0, col: 1 },
            },
            content_revision: 0,
        }),
    );

    state.handle_raw_events(vec![copy_mode_key('a')]);
    let around = state.handle_raw_events(vec![copy_mode_key('}')]);
    let [ClientShellAction::Endpoint { request, .. }] = &around.actions[..] else {
        panic!("a + }} should dispatch a copy-object request");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyObject(params)
            if params.request
                == crate::api::schema::PaneCopyObjectRequest::Object {
                    inside: false,
                    open: '{',
                    close: '}',
                    count: 1,
                    word: None,
                }
    ));

    let request_id = request.id.clone();
    let _ = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneCopyObject {
            pane_id: "pane_1".into(),
            range: crate::api::schema::PaneTextRange {
                start: crate::api::schema::PaneTextPoint { row: 0, col: 0 },
                end: crate::api::schema::PaneTextPoint { row: 0, col: 1 },
            },
            content_revision: 0,
        }),
    );
    let bare = state.handle_raw_events(vec![copy_mode_key('}')]);
    let [ClientShellAction::Endpoint { request, .. }] = &bare.actions[..] else {
        panic!("bare }} should dispatch a copy-motion request");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyMotion(params)
            if params.motion == crate::api::schema::PaneCopyMotion::NextParagraph
    ));
}

#[test]
fn stale_copy_object_result_cancels_queued_copy_input() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    let origin = state.copy_mode.as_ref().unwrap().cursor;
    state.handle_raw_events(vec![copy_mode_key('i')]);
    let object = state.handle_raw_events(vec![copy_mode_key('{')]);
    state.handle_raw_events(vec![copy_mode_key('y')]);
    assert_eq!(state.copy_input_queue.len(), 1);

    let mut changed = state.pane_surface.clone().unwrap();
    changed.surface_revision += 1;
    changed.panes[0].content_revision = 2;
    state.set_pane_surface(changed);
    let request_id = match &object.actions[0] {
        ClientShellAction::Endpoint { request, .. } => request.id.clone(),
        _ => unreachable!(),
    };
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneCopyObject {
            pane_id: "pane_1".into(),
            range: crate::api::schema::PaneTextRange {
                start: origin,
                end: crate::api::schema::PaneTextPoint {
                    row: origin.row,
                    col: origin.col + 2,
                },
            },
            content_revision: 0,
        }),
    );
    assert!(actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Copy);
    assert!(state.copy_mode.as_ref().unwrap().selection.is_none());
    assert!(state.copy_input_queue.is_empty());
}

#[test]
fn copy_object_find_no_match_continues_queue_but_stale_content_cancels() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    let origin = state.copy_mode.as_ref().unwrap().cursor;

    state.handle_raw_events(vec![copy_mode_key('f')]);
    let find = state.handle_raw_events(vec![copy_mode_key('z')]);
    let find_id = match &find.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => {
            assert!(matches!(
                &request.method,
                crate::api::schema::Method::PaneCopyObject(params)
                    if matches!(
                        &params.request,
                        crate::api::schema::PaneCopyObjectRequest::Find { ch, .. } if *ch == 'z'
                    )
            ));
            request.id.clone()
        }
        _ => panic!("f + z should dispatch a copy-object find request"),
    };

    state.handle_raw_events(vec![copy_mode_key('w')]);
    state.handle_raw_events(vec![copy_mode_key('l')]);
    assert_eq!(state.copy_input_queue.len(), 2);

    // A find that matches nothing is a benign no-op.
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &find_id,
        Err(ClientShellEndpointError {
            code: Some("copy_object_unavailable".into()),
            message: "requested text object or character was not found".into(),
        }),
    );
    assert_eq!(state.copy_mode.as_ref().unwrap().cursor, origin);
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("benign find no-match should continue queued input");
    };
    let motion_id = match &request.method {
        crate::api::schema::Method::PaneCopyMotion(_) => request.id.clone(),
        _ => panic!("queued w should replay as a copy-motion request"),
    };
    assert_eq!(state.copy_input_queue.len(), 1);

    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &motion_id,
        Ok(crate::api::schema::ResponseResult::PaneCopyMotion {
            pane_id: "pane_1".into(),
            cursor: crate::api::schema::PaneTextPoint {
                row: origin.row,
                col: origin.col + 1,
            },
            content_revision: 0,
        }),
    );
    assert!(actions.is_empty(), "queued l must replay as a local move");
    assert!(!state.copy_operation_in_flight);
    assert!(state.copy_operation_queue.is_empty());
    assert!(state.copy_input_queue.is_empty());
    assert_eq!(
        state.copy_mode.as_ref().unwrap().cursor,
        crate::api::schema::PaneTextPoint {
            row: origin.row,
            col: origin.col + 2,
        },
        "the replayed local l advances one more column"
    );

    // stale_content is never benign: it still cancels queued operations/input.
    state.handle_raw_events(vec![copy_mode_key('f')]);
    let find = state.handle_raw_events(vec![copy_mode_key('z')]);
    let stale_id = match &find.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("f + z should dispatch a copy-object find request"),
    };
    state.handle_raw_events(vec![copy_mode_key('w')]);
    assert_eq!(state.copy_input_queue.len(), 1);
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &stale_id,
        Err(ClientShellEndpointError {
            code: Some("stale_content".into()),
            message: "pane content changed".into(),
        }),
    );
    assert!(actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Copy);
    assert!(!state.copy_operation_in_flight);
    assert!(state.copy_operation_queue.is_empty());
    assert!(state.copy_input_queue.is_empty());
}

#[test]
fn copy_mode_esc_cancels_pending_find_and_object_without_exiting() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    state.handle_raw_events(vec![copy_mode_key('3')]);
    state.handle_raw_events(vec![copy_mode_key('f')]);
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    let copy_mode = state.copy_mode.as_ref().expect("copy mode stays open");
    assert_eq!(copy_mode.pending_find, None);
    assert_eq!(copy_mode.pending_count, None);

    state.handle_raw_events(vec![copy_mode_key('i')]);
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    ))]);
    let copy_mode = state.copy_mode.as_ref().expect("copy mode stays open");
    assert_eq!(copy_mode.pending_text_object, None);
}

#[test]
fn copy_mode_pending_text_object_routes_w_and_w_to_word_objects() {
    let mut state = copy_mode_state_with_scroll(0, 0);
    let origin = state.copy_mode.as_ref().expect("copy mode").cursor;

    state.handle_raw_events(vec![copy_mode_key('i')]);
    let inside = state.handle_raw_events(vec![copy_mode_key('w')]);
    let [ClientShellAction::Endpoint { request, .. }] = &inside.actions[..] else {
        panic!("i + w should dispatch a word text-object request");
    };
    let request_id = request.id.clone();
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyObject(params)
            if params.cursor == origin
                && params.request
                    == crate::api::schema::PaneCopyObjectRequest::Object {
                        inside: true,
                        open: 'w',
                        close: 'w',
                        count: 1,
                        word: Some(false),
                    }
    ));
    let _ = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneCopyObject {
            pane_id: "pane_1".into(),
            range: crate::api::schema::PaneTextRange {
                start: origin,
                end: origin,
            },
            content_revision: 0,
        }),
    );

    state.handle_raw_events(vec![copy_mode_key('a')]);
    let around = state.handle_raw_events(vec![copy_mode_key('W')]);
    let [ClientShellAction::Endpoint { request, .. }] = &around.actions[..] else {
        panic!("a + W should dispatch a big-word text-object request");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyObject(params)
            if params.request
                == crate::api::schema::PaneCopyObjectRequest::Object {
                    inside: false,
                    open: 'w',
                    close: 'w',
                    count: 1,
                    word: Some(true),
                }
    ));
}

#[test]
fn copy_mode_bare_w_and_w_still_dispatch_word_motions() {
    let mut state = copy_mode_state_with_scroll(0, 0);

    let word = state.handle_raw_events(vec![copy_mode_key('w')]);
    let [ClientShellAction::Endpoint { request, .. }] = &word.actions[..] else {
        panic!("bare w should dispatch a word motion");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyMotion(params)
            if params.motion == crate::api::schema::PaneCopyMotion::NextWordStart
    ));
    let _ = state.handle_endpoint_result(
        "boot-1",
        &request.id.clone(),
        Ok(crate::api::schema::ResponseResult::PaneCopyMotion {
            pane_id: "pane_1".into(),
            cursor: crate::api::schema::PaneTextPoint { row: 0, col: 1 },
            content_revision: 0,
        }),
    );

    let big = state.handle_raw_events(vec![copy_mode_key('W')]);
    let [ClientShellAction::Endpoint { request, .. }] = &big.actions[..] else {
        panic!("bare W should dispatch a big-word motion");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneCopyMotion(params)
            if params.motion == crate::api::schema::PaneCopyMotion::NextBigWordStart
    ));
}
