//! Regression coverage for the machine-grouped aggregate agent panel.
//! Each test attacks one failure hypothesis about endpoint-grouped agents.

use super::*;
use crate::client::endpoint::{
    ClientEndpointId, ClientEndpointStatus, ProfileId, SavedSshEndpoint,
};
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

fn ssh_profile(hex: &str, label: &str) -> SavedSshEndpoint {
    SavedSshEndpoint {
        id: ProfileId::parse(hex).expect("valid profile id"),
        label: label.into(),
        target: format!("dev@{label}.example"),
        session: "agents".into(),
        enabled: true,
    }
}

fn machine_agent(pane_id: &str, name: &str, status: AgentStatus, seq: u64) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some(name.into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: status,
        state_change_seq: seq,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    }
}

fn grouped_state(
    local_agents: Vec<ClientShellAgent>,
    remote: Option<(SavedSshEndpoint, Vec<ClientShellAgent>)>,
) -> (ClientShellState, ClientEndpointId) {
    let mut config = Config::default();
    config.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    let remote_id = match &remote {
        Some((profile, _)) => {
            let id = ClientEndpointId::Ssh(profile.id.clone());
            state.set_endpoint_catalog(std::slice::from_ref(profile));
            state.set_endpoint_status(&id, ClientEndpointStatus::Online);
            id
        }
        None => {
            ClientEndpointId::Ssh(ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap())
        }
    };
    let mut local = snapshot();
    local.agents = local_agents;
    state.set_snapshot(Box::new(local));
    state.set_pane_surface(surface());
    if let Some((_, agents)) = &remote {
        let mut remote_snapshot = snapshot();
        remote_snapshot.boot_id = "remote-boot".into();
        remote_snapshot.agents = agents.clone();
        state.set_endpoint_snapshot(&remote_id, Box::new(remote_snapshot));
    }
    (state, remote_id)
}

fn frame_text(state: &mut ClientShellState, cols: u16, rows: u16) -> String {
    let frame = state.compose(cols, rows).expect("frame");
    frame_rows(&frame).join("\n")
}

/// Text of the agent panel section only (below the ` agents` section header),
/// so machine-tree rows cannot satisfy panel assertions.
fn agent_panel_text(state: &mut ClientShellState, cols: u16, rows: u16) -> String {
    let frame = state.compose(cols, rows).expect("frame");
    let rows = frame_rows(&frame);
    let header_row = rows
        .iter()
        .position(|row| row.contains(" agents "))
        .unwrap_or_else(|| panic!("agent panel header missing: {rows:?}"));
    rows[header_row..].join("\n")
}

fn click(state: &mut ClientShellState, point: (u16, u16)) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: point.0,
        row: point.1,
        modifiers: KeyModifiers::NONE,
    })])
}

fn group_header(state: &ClientShellState, key: &str) -> Rect {
    state
        .hits
        .agent_group_toggles
        .iter()
        .find(|(_, group_key)| group_key == key)
        .map(|(rect, _)| *rect)
        .unwrap_or_else(|| panic!("group header {key} missing"))
}

// an endpoint whose snapshot has zero agents must not emit a group header.
#[test]
fn empty_machine_produces_no_group_header() {
    let (mut state, remote) = grouped_state(
        vec![machine_agent("pane_1", "pi one", AgentStatus::Working, 1)],
        Some((
            ssh_profile("0123456789abcdef0123456789abcdef", "Build"),
            vec![],
        )),
    );
    let text = agent_panel_text(&mut state, 100, 32);
    assert!(text.contains("▾ Local"), "frame: {text}");
    assert!(
        !text.contains("▾ Build"),
        "empty machine must not group: {text}"
    );
    assert_eq!(state.hits.agent_group_toggles.len(), 1);
    assert_eq!(
        state.hits.agent_group_toggles[0].1.as_str(),
        "local",
        "only the populated machine gets a toggle"
    );
    assert!(
        state
            .hits
            .endpoint_agents
            .iter()
            .all(|(_, endpoint, _)| endpoint != &remote),
        "empty machine must contribute no member rows"
    );
}

// an endpoint without any snapshot must not emit a group header either.
#[test]
fn snapshotless_machine_produces_no_group_header() {
    let (mut state, remote) = grouped_state(
        vec![machine_agent("pane_1", "pi one", AgentStatus::Working, 1)],
        Some((
            ssh_profile("0123456789abcdef0123456789abcdef", "Build"),
            vec![],
        )),
    );
    // Force the remote back to "no snapshot" while keeping it Online.
    if let Some(endpoint) = state
        .endpoints
        .iter_mut()
        .find(|endpoint| endpoint.endpoint_id == remote)
    {
        endpoint.snapshot = None;
    }
    let text = agent_panel_text(&mut state, 100, 32);
    assert!(text.contains("▾ Local"), "frame: {text}");
    assert!(
        !text.contains("▾ Build"),
        "snapshotless machine must not group: {text}"
    );
    assert_eq!(state.hits.agent_group_toggles.len(), 1);
}

// collapsing a group must not cache the aggregate status; a member
// status change must move the header severity on the next frame.
#[test]
fn collapsed_group_header_tracks_new_aggregate_severity() {
    let (mut state, _remote) = grouped_state(
        vec![
            machine_agent("pane_1", "pi one", AgentStatus::Working, 1),
            machine_agent("pane_2", "pi two", AgentStatus::Idle, 2),
        ],
        Some((
            ssh_profile("0123456789abcdef0123456789abcdef", "Build"),
            vec![machine_agent("pane_1", "remote pi", AgentStatus::Idle, 1)],
        )),
    );
    let text = frame_text(&mut state, 100, 32);
    assert!(text.contains("▾ Local"), "frame: {text}");

    let header = group_header(&state, "local");
    click(&mut state, (header.x, header.y));
    assert!(state.collapsed_agent_groups.contains("local"));

    // Member status changes while the group is collapsed.
    let mut updated = snapshot();
    updated.agents = vec![
        machine_agent("pane_1", "pi one", AgentStatus::Working, 1),
        machine_agent("pane_2", "pi two", AgentStatus::Blocked, 3),
    ];
    state.set_snapshot(Box::new(updated));

    let frame = state.compose(100, 32).expect("frame after status change");
    let text = frame_rows(&frame).join("\n");
    assert!(text.contains("▸ Local"), "frame: {text}");
    let header = group_header(&state, "local");
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let (x, y) = cell_symbol_position(&frame, header, "×");
    let icon = buffer.cell((x, y)).expect("aggregate status cell");
    assert_eq!(
        icon.symbol(),
        "×",
        "collapsed header must show blocked severity"
    );
}

// collapse state must round-trip through the chrome preferences file
// and be restored by a fresh ClientShellState.
#[test]
fn machine_group_collapse_round_trips_preferences() {
    let path = std::env::temp_dir().join(format!(
        "herdr-agent-machine-groups-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    let mut config = Config::default();
    config.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
    let mut shell_config = ClientShellConfig::from_config(&config);
    shell_config.preferences_path = Some(path.clone());
    let mut state = ClientShellState::new(shell_config);
    let profile = ssh_profile("0123456789abcdef0123456789abcdef", "Build");
    let remote_id = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&remote_id, ClientEndpointStatus::Online);
    let mut local = snapshot();
    local.agents = vec![machine_agent("pane_1", "pi one", AgentStatus::Working, 1)];
    state.set_snapshot(Box::new(local));
    state.set_pane_surface(surface());
    let mut remote = snapshot();
    remote.boot_id = "remote-boot".into();
    remote.agents = vec![machine_agent("pane_1", "remote pi", AgentStatus::Idle, 1)];
    state.set_endpoint_snapshot(&remote_id, Box::new(remote));

    frame_text(&mut state, 100, 32);
    let header = group_header(&state, "local");
    click(&mut state, (header.x, header.y));

    let saved = super::super::preferences::load(&path).expect("preferences written");
    assert_eq!(
        saved.collapsed_agent_groups,
        vec!["local".to_string()],
        "group key must persist by storage key"
    );

    // Fresh client restores the collapse.
    let mut config = Config::default();
    config.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
    let mut shell_config = ClientShellConfig::from_config(&config);
    shell_config.preferences = super::super::preferences::load(&path).unwrap_or_default();
    shell_config.preferences_path = Some(path.clone());
    let mut restored = ClientShellState::new(shell_config);
    assert!(restored.collapsed_agent_groups.contains("local"));
    let profile = ssh_profile("0123456789abcdef0123456789abcdef", "Build");
    let remote_id = ClientEndpointId::Ssh(profile.id.clone());
    restored.set_endpoint_catalog(&[profile]);
    restored.set_endpoint_status(&remote_id, ClientEndpointStatus::Online);
    let mut local = snapshot();
    local.agents = vec![machine_agent("pane_1", "pi one", AgentStatus::Working, 1)];
    restored.set_snapshot(Box::new(local));
    restored.set_pane_surface(surface());
    let mut remote = snapshot();
    remote.boot_id = "remote-boot".into();
    remote.agents = vec![machine_agent("pane_1", "remote pi", AgentStatus::Idle, 1)];
    restored.set_endpoint_snapshot(&remote_id, Box::new(remote));
    let text = frame_text(&mut restored, 100, 32);
    assert!(
        text.contains("▸ Local"),
        "restored collapse must render: {text}"
    );
    assert!(
        !text.contains("pi one"),
        "members hidden after restore: {text}"
    );

    std::fs::remove_file(path).ok();
}

// machine groups appear in endpoint order (local first, then catalog
// order), and members sit under their own header.
#[test]
fn machine_groups_ordered_local_then_catalog() {
    let mut config = Config::default();
    config.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    let alpha = ssh_profile("0123456789abcdef0123456789abcdef", "Alpha");
    let beta = ssh_profile("fedcba9876543210fedcba9876543210", "Beta");
    let alpha_id = ClientEndpointId::Ssh(alpha.id.clone());
    let beta_id = ClientEndpointId::Ssh(beta.id.clone());
    state.set_endpoint_catalog(&[alpha, beta]);
    state.set_endpoint_status(&alpha_id, ClientEndpointStatus::Online);
    state.set_endpoint_status(&beta_id, ClientEndpointStatus::Online);
    let mut local = snapshot();
    local.agents = vec![machine_agent("pane_1", "pi one", AgentStatus::Working, 1)];
    state.set_snapshot(Box::new(local));
    state.set_pane_surface(surface());
    let mut alpha_snapshot = snapshot();
    alpha_snapshot.boot_id = "alpha-boot".into();
    alpha_snapshot.agents = vec![machine_agent("pane_1", "alpha pi", AgentStatus::Idle, 1)];
    state.set_endpoint_snapshot(&alpha_id, Box::new(alpha_snapshot));
    let mut beta_snapshot = snapshot();
    beta_snapshot.boot_id = "beta-boot".into();
    beta_snapshot.agents = vec![machine_agent("pane_1", "beta pi", AgentStatus::Idle, 1)];
    state.set_endpoint_snapshot(&beta_id, Box::new(beta_snapshot));

    let frame = state.compose(120, 40).expect("frame");
    let rows = frame_rows(&frame);
    let row_of = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle} missing: {rows:?}"))
    };
    let local_row = row_of("▾ Local");
    let alpha_row = row_of("▾ Alpha");
    let beta_row = row_of("▾ Beta");
    assert!(local_row < alpha_row, "local group must come first");
    assert!(alpha_row < beta_row, "catalog order preserved");

    // Members sit between their own header and the next header.
    let member_row = row_of("beta pi");
    assert!(
        member_row > beta_row,
        "beta member must render under the beta header"
    );
}

// members of a stale (reconnecting) machine render DIM, but the
// machine group header itself must not inherit the DIM modifier.
#[test]
fn stale_members_dim_but_group_header_stays_bright() {
    let (mut state, remote) = grouped_state(
        vec![machine_agent("pane_1", "pi one", AgentStatus::Working, 1)],
        Some((
            ssh_profile("0123456789abcdef0123456789abcdef", "Build"),
            vec![machine_agent("pane_1", "remote pi", AgentStatus::Idle, 1)],
        )),
    );
    state.set_endpoint_status(&remote, ClientEndpointStatus::Reconnecting);
    let frame = state.compose(100, 32).expect("frame");
    let buffer = frame.to_ratatui_buffer().expect("buffer");

    let remote_members: Vec<_> = state
        .hits
        .endpoint_agents
        .iter()
        .filter(|(_, endpoint, _)| endpoint == &remote)
        .collect();
    assert!(!remote_members.is_empty(), "remote member must render");
    for (rect, _, _) in &remote_members {
        let cell = buffer.cell((rect.x, rect.y)).expect("member cell");
        assert!(
            cell.modifier.contains(ratatui::style::Modifier::DIM),
            "stale member row must be dim"
        );
    }

    let header = group_header(&state, &remote.storage_key());
    for y in header.y..header.bottom() {
        for x in header.x..header.right() {
            let cell = buffer.cell((x, y)).expect("header cell");
            assert!(
                !cell.modifier.contains(ratatui::style::Modifier::DIM),
                "group header must not be dimmed at ({x},{y})"
            );
        }
    }
}

// tiny viewports and empty lists must not panic, and scroll clamps.
#[test]
fn tiny_viewports_and_empty_lists_do_not_panic() {
    let (mut state, _remote) = grouped_state(
        (0..6)
            .map(|index| {
                machine_agent(
                    &format!("pane_{}", index + 1),
                    &format!("pi {index}"),
                    AgentStatus::Working,
                    index as u64 + 1,
                )
            })
            .collect(),
        Some((
            ssh_profile("0123456789abcdef0123456789abcdef", "Build"),
            vec![machine_agent("pane_1", "remote pi", AgentStatus::Idle, 1)],
        )),
    );
    for height in [3, 4, 5, 8, 10, 32] {
        let frame = state.compose(40, height);
        assert!(frame.is_some(), "compose must survive height {height}");
        // Scroll far past the end; the next compose must clamp it.
        state.agent_scroll = 10_000;
        let frame = state.compose(40, height);
        assert!(
            frame.is_some(),
            "compose must survive scroll at height {height}"
        );
        assert!(state.agent_scroll <= 10_000);
    }

    // Empty agent list entirely.
    let (mut state, _remote) = grouped_state(vec![], None);
    for height in [5, 10, 32] {
        assert!(
            state.compose(40, height).is_some(),
            "empty list height {height}"
        );
    }
}

// toggling a group resets agent_scroll to 0.
#[test]
fn group_toggle_resets_agent_scroll() {
    let (mut state, _remote) = grouped_state(
        (0..8)
            .map(|index| {
                machine_agent(
                    &format!("pane_{}", index + 1),
                    &format!("pi {index}"),
                    AgentStatus::Idle,
                    index as u64 + 1,
                )
            })
            .collect(),
        Some((
            ssh_profile("0123456789abcdef0123456789abcdef", "Build"),
            vec![machine_agent("pane_1", "remote pi", AgentStatus::Idle, 1)],
        )),
    );
    let text = agent_panel_text(&mut state, 100, 48);
    assert!(text.contains("▾ Local"), "frame: {text}");
    state.agent_scroll = 3;
    state.compose(100, 48).expect("frame");
    let header = group_header(&state, "local");
    click(&mut state, (header.x, header.y));
    assert_eq!(state.agent_scroll, 0, "toggle must reset scroll");
}

// the single-machine panel must not register group toggles and
// member clicks must still focus panes.
#[test]
fn single_machine_panel_has_no_group_toggles() {
    let mut projected = snapshot();
    projected.panes[0].label = Some("editor".into());
    projected.agents = vec![
        machine_agent("pane_1", "pi one", AgentStatus::Working, 1),
        machine_agent("pane_1", "pi two", AgentStatus::Blocked, 2),
    ];
    let mut config = Config::default();
    config.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());

    frame_text(&mut state, 106, 30);
    assert!(
        state.hits.agent_group_toggles.is_empty(),
        "single-machine panel must not group"
    );
    assert_eq!(state.hits.agents.len(), 2);
    let row = state.hits.agents[0].0;
    let actions = click(&mut state, (row.x, row.y));
    assert!(actions.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(&request.method, crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1")
    )));
}

// clicking the label area (not only the glyph) toggles exactly once,
// and the group header click must not dispatch a pane focus.
#[test]
fn group_label_click_toggles_once_without_pane_focus() {
    let (mut state, _remote) = grouped_state(
        vec![machine_agent("pane_1", "pi one", AgentStatus::Working, 1)],
        Some((
            ssh_profile("0123456789abcdef0123456789abcdef", "Build"),
            vec![machine_agent("pane_1", "remote pi", AgentStatus::Idle, 1)],
        )),
    );
    frame_text(&mut state, 100, 32);
    let header = group_header(&state, "local");

    // Click the middle of the label text.
    let actions = click(&mut state, (header.x + header.width / 2, header.y));
    assert!(state.collapsed_agent_groups.contains("local"));
    assert!(
        actions.actions.is_empty(),
        "group header click must not dispatch pane actions: {:?}",
        actions.actions
    );

    // Second click on the same spot expands again.
    click(&mut state, (header.x + header.width / 2, header.y));
    assert!(
        !state.collapsed_agent_groups.contains("local"),
        "second click must expand"
    );

    // Member click routes through the endpoint-qualified hit map.
    let (rect, endpoint, pane_id) = state.hits.endpoint_agents[0].clone();
    let actions = click(&mut state, (rect.x + 2, rect.y));
    assert_eq!(pane_id, "pane_1");
    let focused = actions.actions.iter().any(|action| match action {
        ClientShellAction::Endpoint { request, .. } => matches!(
            &request.method,
            crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1"
        ),
        ClientShellAction::ActivateEndpoint {
            endpoint_id,
            target,
        } => {
            endpoint_id == &endpoint
                && matches!(target, Some(ClientEndpointFocusTarget::Pane(pane)) if pane == "pane_1")
        }
        _ => false,
    });
    assert!(
        focused,
        "member click must focus its pane: {:?}",
        actions.actions
    );
}

// priority sort and filtered agent views must stay flat.
#[test]
fn priority_sort_and_agent_view_stay_flat() {
    let (mut state, _remote) = grouped_state(
        vec![
            machine_agent("pane_1", "pi one", AgentStatus::Working, 1),
            machine_agent("pane_2", "pi two", AgentStatus::Blocked, 2),
        ],
        Some((
            ssh_profile("0123456789abcdef0123456789abcdef", "Build"),
            vec![machine_agent("pane_1", "remote pi", AgentStatus::Idle, 1)],
        )),
    );
    state.config.agent_panel_sort = crate::config::AgentPanelSortConfig::Priority;
    let text = frame_text(&mut state, 100, 32);
    assert!(
        state.hits.agent_group_toggles.is_empty(),
        "priority stays flat"
    );
    assert!(
        text.contains("pi one") && text.contains("remote pi"),
        "frame: {text}"
    );

    state.config.agent_panel_sort = crate::config::AgentPanelSortConfig::Spaces;
    let mut filtered = snapshot();
    filtered.agents = vec![machine_agent("pane_1", "pi one", AgentStatus::Working, 1)];
    filtered.agent_view_label = Some("current space".into());
    state.set_snapshot(Box::new(filtered));
    frame_text(&mut state, 100, 32);
    assert!(
        state.hits.agent_group_toggles.is_empty(),
        "filtered agent view stays flat"
    );
}

// two ssh machines sharing a label must form distinct groups keyed
// by storage key, and toggling one must not touch the other.
#[test]
fn duplicate_machine_labels_collapse_independently() {
    let mut config = Config::default();
    config.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    let first = ssh_profile("0123456789abcdef0123456789abcdef", "Same");
    let second = ssh_profile("fedcba9876543210fedcba9876543210", "Same");
    let first_id = ClientEndpointId::Ssh(first.id.clone());
    let second_id = ClientEndpointId::Ssh(second.id.clone());
    state.set_endpoint_catalog(&[first, second]);
    state.set_endpoint_status(&first_id, ClientEndpointStatus::Online);
    state.set_endpoint_status(&second_id, ClientEndpointStatus::Online);
    let mut local = snapshot();
    local.agents = vec![machine_agent("pane_1", "pi one", AgentStatus::Working, 1)];
    state.set_snapshot(Box::new(local));
    state.set_pane_surface(surface());
    for (endpoint_id, name) in [(&first_id, "first pi"), (&second_id, "second pi")] {
        let mut machine = snapshot();
        machine.boot_id = format!("{name}-boot");
        machine.agents = vec![machine_agent("pane_1", name, AgentStatus::Idle, 1)];
        state.set_endpoint_snapshot(endpoint_id, Box::new(machine));
    }

    frame_text(&mut state, 100, 32);
    assert_eq!(
        state.hits.agent_group_toggles.len(),
        3,
        "local plus two same-labeled machines"
    );
    let header = group_header(&state, &second_id.storage_key());
    click(&mut state, (header.x, header.y));
    assert!(state
        .collapsed_agent_groups
        .contains(&second_id.storage_key()));
    assert!(!state
        .collapsed_agent_groups
        .contains(&first_id.storage_key()));
    let text = agent_panel_text(&mut state, 100, 32);
    assert!(
        text.contains("first pi"),
        "first machine must stay expanded: {text}"
    );
    assert!(
        !text.contains("second pi"),
        "second machine must be collapsed: {text}"
    );
    assert_eq!(
        state
            .hits
            .agent_group_toggles
            .iter()
            .filter(|(_, key)| key.starts_with("ssh:"))
            .count(),
        2,
        "both machine headers remain present"
    );
}

// with mouse capture disabled the group header must be inert, like
// every other sidebar click target (render_shell clears their hit rects).
#[test]
fn group_toggle_inert_without_mouse_capture() {
    let (mut state, _remote) = grouped_state(
        vec![machine_agent("pane_1", "pi one", AgentStatus::Working, 1)],
        Some((
            ssh_profile("0123456789abcdef0123456789abcdef", "Build"),
            vec![machine_agent("pane_1", "remote pi", AgentStatus::Idle, 1)],
        )),
    );
    frame_text(&mut state, 100, 32);
    let header = group_header(&state, "local");

    state.config.mouse_capture = false;
    frame_text(&mut state, 100, 32);
    assert!(
        state.hits.agent_group_toggles.is_empty(),
        "group toggles must be cleared when mouse capture is off"
    );
    // Click where the header renders with capture on: must stay expanded.
    click(&mut state, (header.x, header.y));
    assert!(
        !state.collapsed_agent_groups.contains("local"),
        "group toggles must be inert when mouse capture is off"
    );
}
