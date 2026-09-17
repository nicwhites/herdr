use super::*;
mod block;
mod core;
mod counts;
mod objects;
mod search;

fn copy_mode_state_with_scroll(
    offset_from_bottom: u64,
    max_offset_from_bottom: u64,
) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.panes[0].scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
        offset_from_bottom,
        max_offset_from_bottom,
        viewport_rows: 2,
    });
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("composed frame");
    let mut enter = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CopyMode),
        &mut enter,
    );
    state
}

fn copy_mode_key(ch: char) -> RawInputEvent {
    RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char(ch),
        KeyModifiers::empty(),
    ))
}

fn copy_match(row: u32, col: u16) -> crate::api::schema::PaneTextRange {
    crate::api::schema::PaneTextRange {
        start: crate::api::schema::PaneTextPoint { row, col },
        end: crate::api::schema::PaneTextPoint { row, col: col + 2 },
    }
}
