use super::render::put_text;
use super::*;

pub(super) fn render_collapsed(
    buffer: &mut Buffer,
    area: Rect,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    let rows = agent_rows(endpoints, active_endpoint_id, config);
    for (index, row) in rows.into_iter().take(area.height as usize).enumerate() {
        let rect = Rect::new(area.x, area.y + index as u16, area.width, 1);
        if row.agent.focused {
            buffer.set_style(rect, Style::default().bg(config.palette.active_row_bg));
        }
        let initial = row.machine_label.chars().next().unwrap_or('?');
        put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width,
            &format!(
                "{initial}{}",
                status_icon(row.agent.status, config.status_indicators)
            ),
            Style::default()
                .fg(if row.stale {
                    config.palette.overlay0
                } else {
                    status_color(row.agent.status, &config.palette)
                })
                .add_modifier(if row.stale {
                    Modifier::DIM
                } else {
                    Modifier::empty()
                }),
        );
        hits.endpoint_agents
            .push((rect, row.endpoint_id, row.agent.pane_id));
    }
}

pub(super) fn render_expanded(
    buffer: &mut Buffer,
    area: Rect,
    agent_view_label: Option<&str>,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    config: &ClientShellConfig,
    collapsed_agent_groups: &HashSet<String>,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    if !super::agent_sidebar::render_agent_panel_header(
        buffer,
        area,
        agent_view_label,
        config,
        hits,
    ) {
        return;
    }
    let rows = panel_rows(
        endpoints,
        active_endpoint_id,
        config,
        agent_view_label,
        collapsed_agent_groups,
    );
    super::agent_sidebar::render_agent_list(
        buffer,
        area,
        &rows,
        agent_view_label.map(|_| " no matching agents"),
        config,
        agent_scroll,
        hits,
        |row| match row {
            EndpointPanelRow::Group(_) => 1,
            EndpointPanelRow::Agent(row) => row.agent.rows.len(),
        },
        |buffer, rect, row, hits| match row {
            EndpointPanelRow::Group(group) => {
                hits.agent_group_toggles
                    .push((rect, group.group_key.clone()));
                super::agent_sidebar::render_agent_group_row(buffer, rect, group, config);
            }
            EndpointPanelRow::Agent(row) => {
                super::agent_sidebar::render_agent_row(buffer, rect, &row.agent, config);
                if row.stale {
                    buffer.set_style(
                        rect,
                        Style::default()
                            .fg(config.palette.overlay0)
                            .add_modifier(Modifier::DIM),
                    );
                }
                hits.endpoint_agents.push((
                    rect,
                    row.endpoint_id.clone(),
                    row.agent.pane_id.clone(),
                ));
            }
        },
    );
}

enum EndpointPanelRow {
    Group(super::agent_sidebar::AgentGroupRow),
    Agent(EndpointAgentRow),
}

impl ClientShellState {
    pub(super) fn reveal_endpoint_agent(
        &mut self,
        endpoint_id: &ClientEndpointId,
        pane_id: &str,
        body_height: u16,
    ) {
        if body_height == 0 {
            return;
        }
        let rows = panel_rows(
            &self.endpoints,
            &self.active_endpoint_id,
            &self.config,
            self.snapshot
                .as_deref()
                .and_then(|snapshot| snapshot.agent_view_label.as_deref()),
            &self.collapsed_agent_groups,
        );
        let Some(target) = rows.iter().position(|row| match row {
            EndpointPanelRow::Agent(agent) => {
                &agent.endpoint_id == endpoint_id && agent.agent.pane_id == pane_id
            }
            EndpointPanelRow::Group(_) => false,
        }) else {
            return;
        };
        let heights = rows
            .iter()
            .map(|row| match row {
                EndpointPanelRow::Group(_) => 1u16,
                EndpointPanelRow::Agent(agent) => {
                    agent.agent.rows.len().max(1).min(u16::MAX as usize) as u16
                }
            })
            .collect::<Vec<_>>();
        let mut gaps = vec![self.config.agents.row_gap; rows.len()];
        if let Some(last) = gaps.last_mut() {
            *last = 0;
        }
        self.agent_scroll = super::scroll::list_scroll_start_to_reveal(
            &heights,
            &gaps,
            body_height,
            self.agent_scroll,
            target,
        );
    }
}

struct EndpointAgentRow {
    endpoint_id: ClientEndpointId,
    machine_label: String,
    stale: bool,
    agent: super::agent_sidebar::AgentRow,
}

fn panel_rows(
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    config: &ClientShellConfig,
    agent_view_label: Option<&str>,
    collapsed_agent_groups: &HashSet<String>,
) -> Vec<EndpointPanelRow> {
    let agents = agent_rows(endpoints, active_endpoint_id, config);
    if agent_view_label.is_some()
        || config.agent_panel_sort == crate::config::AgentPanelSortConfig::Priority
    {
        return agents.into_iter().map(EndpointPanelRow::Agent).collect();
    }
    let mut groups: Vec<(super::agent_sidebar::AgentGroupRow, Vec<EndpointAgentRow>)> = Vec::new();
    for agent in agents {
        let group_key = agent.endpoint_id.storage_key();
        if let Some((_, members)) = groups
            .iter_mut()
            .find(|(group, _)| group.group_key == group_key)
        {
            members.push(agent);
        } else {
            groups.push((
                super::agent_sidebar::AgentGroupRow {
                    group_key: group_key.clone(),
                    label: agent.machine_label.clone(),
                    status: crate::api::schema::AgentStatus::Unknown,
                    collapsed: collapsed_agent_groups.contains(&group_key),
                },
                vec![agent],
            ));
        }
    }
    let mut rows = Vec::new();
    for (group, members) in groups {
        let collapsed = group.collapsed;
        rows.push(EndpointPanelRow::Group(
            super::agent_sidebar::AgentGroupRow {
                status: super::agent_sidebar::aggregate_agent_status(
                    members.iter().map(|row| row.agent.status),
                ),
                ..group
            },
        ));
        if !collapsed {
            rows.extend(members.into_iter().map(EndpointPanelRow::Agent));
        }
    }
    rows
}

fn agent_rows(
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    config: &ClientShellConfig,
) -> Vec<EndpointAgentRow> {
    super::aggregate_navigation::aggregate_agent_rows(
        endpoints,
        active_endpoint_id,
        config.agent_panel_sort,
    )
    .into_iter()
    .filter_map(|row| {
        let mut agent = super::agent_sidebar::agent_row(
            row.endpoint.snapshot,
            row.agent,
            config,
            Some(row.endpoint.label),
        )?;
        agent.focused &= row.endpoint.endpoint_id == active_endpoint_id;
        Some(EndpointAgentRow {
            endpoint_id: row.endpoint.endpoint_id.clone(),
            machine_label: row.endpoint.label.to_owned(),
            stale: row.endpoint.stale(),
            agent,
        })
    })
    .collect()
}
