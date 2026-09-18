use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

/// Selection kind a copy-mode key can start.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CopySelectionKind {
    Character,
    Linewise,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CopyViewportLine {
    Top,
    Middle,
    Bottom,
}

/// Outcome of applying a pane.copy_search response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CopySearchApply {
    Applied,
    /// A newer query superseded the response.
    Superseded,
    /// The response no longer matches pane, cursor, or content state.
    Stale,
}

/// Count-multiplied cell delta for plain motions; saturated into i16.
fn copy_motion_times(count: Option<u32>) -> i16 {
    count.unwrap_or(1).min(i16::MAX as u32) as i16
}

fn copy_line_count_delta(count: Option<u32>) -> i16 {
    count.unwrap_or(1).saturating_sub(1).min(i16::MAX as u32) as i16
}

impl ClientShellState {
    pub(super) fn reset_copy_pipeline(&mut self) {
        self.copy_session_generation = self.copy_session_generation.saturating_add(1);
        self.copy_operation_in_flight = false;
        self.copy_operation_queue.clear();
        self.copy_input_queue.clear();
    }

    fn retire_cancelled_copy_search(&mut self) {
        let session_generation = self.copy_session_generation;
        let search_in_flight = self.pending_requests.values().any(|pending| {
            matches!(
                &pending.kind,
                PendingEndpointKind::CopySearch {
                    session_generation: pending_session,
                    ..
                } if *pending_session == session_generation
            )
        });
        if search_in_flight {
            self.copy_session_generation = self.copy_session_generation.saturating_add(1);
            self.copy_operation_in_flight = false;
        }
    }

    pub(super) fn enter_copy_mode(&mut self, outcome: &mut ClientShellInput) -> bool {
        let pane_id = match self.focused_pane_id() {
            Some(pane_id) => pane_id,
            None => return false,
        };
        if self
            .copy_mode
            .as_ref()
            .is_some_and(|copy_mode| copy_mode.pane_id == pane_id)
        {
            self.mode = ClientShellMode::Copy;
            return true;
        }
        if self.copy_mode.is_some() {
            self.exit_copy_mode(false, outcome);
        }
        let Some(hit) = self
            .hits
            .panes
            .iter()
            .find(|hit| hit.pane_id == pane_id)
            .cloned()
        else {
            return false;
        };
        let Some(metrics) = hit.scroll else {
            return false;
        };
        let viewport_top = metrics
            .max_offset_from_bottom
            .saturating_sub(metrics.offset_from_bottom)
            .min(u32::MAX as usize) as u32;
        let cursor = self
            .pane_surface
            .as_ref()
            .and_then(|surface| {
                let pane = surface.panes.iter().find(|pane| pane.pane_id == pane_id)?;
                let cursor = surface
                    .frame
                    .cursor
                    .as_ref()
                    .filter(|cursor| cursor.visible)?;
                let inner = pane.inner_rect;
                (cursor.x >= inner.x
                    && cursor.x < inner.x.saturating_add(inner.width)
                    && cursor.y >= inner.y
                    && cursor.y < inner.y.saturating_add(inner.height))
                .then_some(crate::api::schema::PaneTextPoint {
                    row: viewport_top.saturating_add(u32::from(cursor.y - inner.y)),
                    col: cursor.x - inner.x,
                })
            })
            .unwrap_or(crate::api::schema::PaneTextPoint {
                row: viewport_top
                    .saturating_add(u32::from(hit.inner_rect.height.saturating_sub(1))),
                col: 0,
            });
        self.selection = None;
        self.stop_selection_autoscroll();
        self.selection_highlight_clear_deadline = None;
        self.reset_copy_pipeline();
        let (content_revision, alternate_screen_active) = self
            .pane_surface
            .as_ref()
            .and_then(|surface| surface.panes.iter().find(|pane| pane.pane_id == pane_id))
            .map_or((0, false), |pane| {
                (pane.content_revision, pane.alternate_screen_active)
            });
        self.copy_mode = Some(ClientCopyModeState {
            pane_id,
            content_revision,
            geometry: (hit.inner_rect.width, hit.inner_rect.height),
            alternate_screen_active,
            cursor,
            offset_from_bottom: metrics.offset_from_bottom,
            max_offset_from_bottom: metrics.max_offset_from_bottom,
            entry_offset_from_bottom: metrics.offset_from_bottom,
            selection: None,
            search_prompt: None,
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
        self.mode = ClientShellMode::Copy;
        true
    }

    pub(super) fn route_copy_mode_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        if self.route_copy_search_prompt_key(key, outcome) {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                if self.copy_mode.as_ref().is_some_and(|copy_mode| {
                    copy_mode.pending_count.is_some()
                        || copy_mode.pending_find.is_some()
                        || copy_mode.pending_text_object.is_some()
                }) {
                    if let Some(copy_mode) = self.copy_mode.as_mut() {
                        copy_mode.pending_find = None;
                        copy_mode.pending_text_object = None;
                        copy_mode.pending_count = None;
                    }
                    outcome.repaint = true;
                    return;
                }
                let should_clear = self.copy_mode.as_ref().is_some_and(|copy_mode| {
                    copy_mode.selection.is_some()
                        || !copy_mode.search_query.is_empty()
                        || !copy_mode.search_matches.is_empty()
                        || copy_mode.search_direction.is_some()
                });
                if should_clear {
                    if let Some(copy_mode) = self.copy_mode.as_mut() {
                        copy_mode.selection = None;
                        copy_mode.search_query.clear();
                        copy_mode.search_direction = None;
                        copy_mode.search_matches.clear();
                        copy_mode.search_total = 0;
                        copy_mode.search_current = None;
                        copy_mode.search_current_global = None;
                        copy_mode.search_generation = copy_mode.search_generation.saturating_add(1);
                        copy_mode.copy_after_search = false;
                    }
                    self.selection = None;
                    self.reset_copy_pipeline();
                } else {
                    self.exit_copy_mode(false, outcome);
                }
                outcome.repaint = true;
                return;
            }
            KeyCode::Enter => {
                if !self.defer_copy_until_search_result() {
                    self.exit_copy_mode(true, outcome);
                }
                return;
            }
            KeyCode::Left => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.request_copy_horizontal(false, count, outcome);
                return;
            }
            KeyCode::Down => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.move_copy_cursor(copy_motion_times(count), 0, outcome);
                return;
            }
            KeyCode::Up => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.move_copy_cursor(-copy_motion_times(count), 0, outcome);
                return;
            }
            KeyCode::Right => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.request_copy_horizontal(true, count, outcome);
                return;
            }
            KeyCode::PageUp => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.move_copy_page(-1, false, count, outcome);
                return;
            }
            KeyCode::PageDown => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.move_copy_page(1, false, count, outcome);
                return;
            }
            KeyCode::Home => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.move_copy_cursor(copy_line_count_delta(count), 0, outcome);
                self.set_copy_cursor_col(0);
                self.sync_copy_selection();
                outcome.repaint = true;
                return;
            }
            KeyCode::End => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.move_copy_cursor(copy_line_count_delta(count), 0, outcome);
                self.request_copy_motion(crate::api::schema::PaneCopyMotion::LineEnd, outcome);
                return;
            }
            _ => {}
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('b'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.move_copy_page(-1, false, count, outcome);
                return;
            }
            (KeyCode::Char('f'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.move_copy_page(1, false, count, outcome);
                return;
            }
            (KeyCode::Char('u'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.move_copy_page(-1, true, count, outcome);
                return;
            }
            (KeyCode::Char('d'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                let count = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.move_copy_page(1, true, count, outcome);
                return;
            }
            (KeyCode::Char('v'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = self.take_copy_count();
                self.clear_copy_pending_prefixes();
                self.begin_copy_selection(CopySelectionKind::Block);
                return;
            }
            _ => {}
        }

        let Some(command) = crate::copy_mode::copy_mode_command_char(key.clone()) else {
            return;
        };

        if let Some((forward, till)) = self
            .copy_mode
            .as_ref()
            .and_then(|copy_mode| copy_mode.pending_find)
        {
            if let Some(copy_mode) = self.copy_mode.as_mut() {
                copy_mode.pending_find = None;
                copy_mode.last_find = Some(ClientCopyLastFind {
                    ch: command,
                    forward,
                    till,
                });
            }
            let count = self.take_copy_count();
            self.request_copy_find(command, forward, till, count, false, outcome);
            outcome.repaint = true;
            return;
        }

        // Digits 1-9 start a count prefix; 0 only continues one (bare 0 means column 0 below).
        if let Some(digit) = command.to_digit(10).filter(|digit| {
            *digit > 0
                || self
                    .copy_mode
                    .as_ref()
                    .is_some_and(|copy_mode| copy_mode.pending_count.is_some())
        }) {
            if let Some(copy_mode) = self.copy_mode.as_mut() {
                // ponytail: cap accumulated counts so a runaway prefix cannot
                // queue an unbounded number of motions.
                copy_mode.pending_count = Some(
                    copy_mode
                        .pending_count
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(digit)
                        .min(10_000),
                );
            }
            return;
        }

        let pending_text_object = self
            .copy_mode
            .as_mut()
            .and_then(|copy_mode| copy_mode.pending_text_object.take());
        match command {
            'q' => self.exit_copy_mode(false, outcome),
            'y' => {
                if !self.defer_copy_until_search_result() {
                    self.exit_copy_mode(true, outcome);
                }
            }
            'v' | ' ' => {
                let _ = self.take_copy_count();
                self.begin_copy_selection(CopySelectionKind::Character);
            }
            'V' => {
                let _ = self.take_copy_count();
                self.begin_copy_selection(CopySelectionKind::Linewise);
            }
            'h' => {
                let count = self.take_copy_count();
                self.request_copy_horizontal(false, count, outcome)
            }
            'j' => {
                let count = self.take_copy_count();
                self.move_copy_cursor(copy_motion_times(count), 0, outcome)
            }
            'k' => {
                let count = self.take_copy_count();
                self.move_copy_cursor(-copy_motion_times(count), 0, outcome)
            }
            'l' => {
                let count = self.take_copy_count();
                self.request_copy_horizontal(true, count, outcome)
            }
            'g' => {
                let count = self.take_copy_count();
                self.move_copy_history(count.is_none(), count, outcome);
            }
            'G' => {
                let count = self.take_copy_count();
                self.move_copy_history(false, count, outcome)
            }
            '0' => {
                let _ = self.take_copy_count();
                self.set_copy_cursor_col(0);
                self.sync_copy_selection();
                outcome.repaint = true;
            }
            '$' => {
                let count = self.take_copy_count();
                self.move_copy_cursor(copy_line_count_delta(count), 0, outcome);
                self.request_copy_motion(crate::api::schema::PaneCopyMotion::LineEnd, outcome)
            }
            '^' => {
                let count = self.take_copy_count();
                self.move_copy_cursor(copy_line_count_delta(count), 0, outcome);
                self.request_copy_motion(crate::api::schema::PaneCopyMotion::FirstNonBlank, outcome)
            }
            '/' => {
                let count = self.take_copy_count();
                self.open_copy_search(crate::api::schema::PaneCopySearchDirection::Forward, count);
            }
            '?' => {
                let count = self.take_copy_count();
                self.open_copy_search(crate::api::schema::PaneCopySearchDirection::Backward, count);
            }
            'n' => {
                let count = self.take_copy_count();
                self.repeat_copy_search(false, count, outcome)
            }
            'N' => {
                let count = self.take_copy_count();
                self.repeat_copy_search(true, count, outcome)
            }
            'w' | 'W' => {
                if let Some(around) = pending_text_object {
                    // Word text objects have no nesting, so the count is discarded;
                    // open/close are placeholders ignored for word requests.
                    let _ = self.take_copy_count();
                    self.request_copy_object(
                        crate::api::schema::PaneCopyObjectRequest::Object {
                            inside: !around,
                            open: 'w',
                            close: 'w',
                            count: 1,
                            word: Some(command == 'W'),
                        },
                        outcome,
                    );
                } else {
                    let count = self.take_copy_count();
                    self.request_copy_motion_repeat(
                        if command == 'W' {
                            crate::api::schema::PaneCopyMotion::NextBigWordStart
                        } else {
                            crate::api::schema::PaneCopyMotion::NextWordStart
                        },
                        count,
                        outcome,
                    );
                }
            }
            'b' => {
                let count = self.take_copy_count();
                self.request_copy_motion_repeat(
                    crate::api::schema::PaneCopyMotion::PreviousWordStart,
                    count,
                    outcome,
                )
            }
            'e' => {
                let count = self.take_copy_count();
                self.request_copy_motion_repeat(
                    crate::api::schema::PaneCopyMotion::NextWordEnd,
                    count,
                    outcome,
                )
            }
            'B' => {
                let count = self.take_copy_count();
                self.request_copy_motion_repeat(
                    crate::api::schema::PaneCopyMotion::PreviousBigWordStart,
                    count,
                    outcome,
                )
            }
            'E' => {
                let count = self.take_copy_count();
                self.request_copy_motion_repeat(
                    crate::api::schema::PaneCopyMotion::NextBigWordEnd,
                    count,
                    outcome,
                )
            }
            'f' => self.set_copy_pending_find(true, false),
            'F' => self.set_copy_pending_find(false, false),
            't' => self.set_copy_pending_find(true, true),
            'T' => self.set_copy_pending_find(false, true),
            ';' => {
                if let Some(last_find) = self
                    .copy_mode
                    .as_ref()
                    .and_then(|copy_mode| copy_mode.last_find)
                {
                    let count = self.take_copy_count();
                    self.request_copy_find(
                        last_find.ch,
                        last_find.forward,
                        last_find.till,
                        count,
                        true,
                        outcome,
                    );
                } else {
                    let _ = self.take_copy_count();
                    return;
                }
            }
            ',' => {
                if let Some(last_find) = self
                    .copy_mode
                    .as_ref()
                    .and_then(|copy_mode| copy_mode.last_find)
                {
                    let count = self.take_copy_count();
                    self.request_copy_find(
                        last_find.ch,
                        !last_find.forward,
                        last_find.till,
                        count,
                        true,
                        outcome,
                    );
                } else {
                    let _ = self.take_copy_count();
                    return;
                }
            }
            'i' => self.set_copy_pending_text_object(false),
            'a' => self.set_copy_pending_text_object(true),
            '{' | '}' | '(' | ')' | '[' | ']' => {
                if let Some(around) = pending_text_object {
                    let count = self.take_copy_count().unwrap_or(1);
                    let (open, close) = match command {
                        '(' | ')' => ('(', ')'),
                        '[' | ']' => ('[', ']'),
                        _ => ('{', '}'),
                    };
                    self.request_copy_object(
                        crate::api::schema::PaneCopyObjectRequest::Object {
                            inside: !around,
                            open,
                            close,
                            count,
                            word: None,
                        },
                        outcome,
                    );
                } else {
                    let motion = match command {
                        '{' => crate::api::schema::PaneCopyMotion::PreviousParagraph,
                        '}' => crate::api::schema::PaneCopyMotion::NextParagraph,
                        _ => {
                            let _ = self.take_copy_count();
                            return;
                        }
                    };
                    let count = self.take_copy_count();
                    self.request_copy_motion_repeat(motion, count, outcome);
                }
            }
            'H' => {
                let count = self.take_copy_count();
                self.move_copy_viewport_line(CopyViewportLine::Top, count, outcome);
            }
            'M' => {
                let count = self.take_copy_count();
                self.move_copy_viewport_line(CopyViewportLine::Middle, count, outcome);
            }
            'L' => {
                let count = self.take_copy_count();
                self.move_copy_viewport_line(CopyViewportLine::Bottom, count, outcome);
            }
            '|' => {
                let width = self.copy_hit().map_or(1, |hit| hit.inner_rect.width.max(1));
                let col = self.take_copy_count().map_or(0, |n| {
                    (n.saturating_sub(1)).min(u32::from(width.saturating_sub(1))) as u16
                });
                self.set_copy_cursor_col(col);
                self.sync_copy_selection();
                outcome.repaint = true;
            }
            _ => {
                let _ = self.take_copy_count();
                return;
            }
        }
        outcome.repaint = true;
    }

    fn route_copy_search_prompt_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(prompt) = self
            .copy_mode
            .as_ref()
            .and_then(|copy_mode| copy_mode.search_prompt.as_ref())
        else {
            return false;
        };
        let mut submit = None;
        match key.code {
            KeyCode::Esc => {
                if let Some(copy_mode) = self.copy_mode.as_mut() {
                    copy_mode.search_prompt = None;
                    // Invalidate the prompt's queries so a cancelled search cannot move the cursor.
                    copy_mode.search_generation = copy_mode.search_generation.saturating_add(1);
                    copy_mode.copy_after_search = false;
                }
                self.copy_operation_queue
                    .retain(|operation| !matches!(operation, ClientCopyOperation::Search { .. }));
                self.retire_cancelled_copy_search();
                self.dispatch_next_copy_operation(outcome);
            }
            KeyCode::Enter => {
                submit = Some((prompt.query.to_string(), prompt.direction, prompt.count));
                if let Some(copy_mode) = self.copy_mode.as_mut() {
                    copy_mode.search_prompt = None;
                }
            }
            _ => {
                let changed = self
                    .copy_mode
                    .as_mut()
                    .and_then(|copy_mode| copy_mode.search_prompt.as_mut())
                    .and_then(|prompt| prompt.query.handle_key(key));
                if changed == Some(true) {
                    self.refresh_copy_search_live_query(outcome);
                }
            }
        }
        if let Some((query, direction, count)) = submit {
            self.commit_copy_search(query, direction, count, outcome);
        }
        outcome.repaint = true;
        true
    }

    pub(super) fn insert_copy_search_text(
        &mut self,
        text: &str,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.mode != ClientShellMode::Copy
            || self.overlay.is_some()
            || self.popup_terminal_id.is_some()
            || self.popup_pending
        {
            return false;
        }
        let changed = self
            .copy_mode
            .as_mut()
            .and_then(|copy_mode| copy_mode.search_prompt.as_mut())
            .map(|prompt| prompt.query.insert(text));
        match changed {
            Some(true) => {
                self.refresh_copy_search_live_query(outcome);
                true
            }
            Some(false) => true,
            None => false,
        }
    }

    fn refresh_copy_search_live_query(&mut self, outcome: &mut ClientShellInput) {
        let Some(prompt) = self
            .copy_mode
            .as_ref()
            .and_then(|copy_mode| copy_mode.search_prompt.as_ref())
        else {
            return;
        };
        let query = prompt.query.as_str().to_string();
        let direction = prompt.direction;
        if let Some(copy_mode) = self.copy_mode.as_mut() {
            copy_mode.search_generation = copy_mode.search_generation.saturating_add(1);
        }
        self.copy_operation_queue
            .retain(|operation| !matches!(operation, ClientCopyOperation::Search { .. }));
        if query.is_empty() {
            if let Some(copy_mode) = self.copy_mode.as_mut() {
                copy_mode.search_query.clear();
                copy_mode.search_matches.clear();
                copy_mode.search_total = 0;
                copy_mode.search_current = None;
                copy_mode.search_current_global = None;
            }
            return;
        }
        self.copy_operation_queue
            .push_back(ClientCopyOperation::Search {
                query,
                direction,
                repeat: false,
                remaining: 1,
            });
        self.dispatch_next_copy_operation(outcome);
    }

    fn open_copy_search(
        &mut self,
        direction: crate::api::schema::PaneCopySearchDirection,
        count: Option<u32>,
    ) {
        let Some(copy_mode) = self.copy_mode.as_mut() else {
            return;
        };
        copy_mode.pending_find = None;
        copy_mode.pending_text_object = None;
        copy_mode.search_prompt = Some(ClientCopySearchPrompt {
            direction,
            query: TextEditor::default(),
            count: count.unwrap_or(1),
        });
    }

    fn repeat_copy_search(
        &mut self,
        reverse: bool,
        count: Option<u32>,
        outcome: &mut ClientShellInput,
    ) {
        let Some(copy_mode) = self.copy_mode.as_ref() else {
            return;
        };
        if copy_mode.search_query.is_empty() {
            return;
        }
        let Some(direction) = copy_mode.search_direction else {
            return;
        };
        let direction = if reverse {
            match direction {
                crate::api::schema::PaneCopySearchDirection::Forward => {
                    crate::api::schema::PaneCopySearchDirection::Backward
                }
                crate::api::schema::PaneCopySearchDirection::Backward => {
                    crate::api::schema::PaneCopySearchDirection::Forward
                }
            }
        } else {
            direction
        };
        self.copy_operation_queue
            .push_back(ClientCopyOperation::Search {
                query: copy_mode.search_query.clone(),
                direction,
                repeat: true,
                remaining: count.unwrap_or(1).max(1),
            });
        self.dispatch_next_copy_operation(outcome);
    }

    fn defer_copy_until_search_result(&mut self) -> bool {
        let Some(copy_mode) = self.copy_mode.as_ref() else {
            return false;
        };
        let pane_id = copy_mode.pane_id.clone();
        let generation = copy_mode.search_generation;
        let session = self.copy_session_generation;
        let pending = self.pending_requests.values().any(|pending| {
            matches!(
                &pending.kind,
                PendingEndpointKind::CopySearch {
                    pane_id: pending_pane,
                    generation: pending_generation,
                    session_generation: pending_session,
                    ..
                } if pending_pane == &pane_id
                    && *pending_generation == generation
                    && *pending_session == session
            )
        }) || self
            .copy_operation_queue
            .iter()
            .any(|operation| matches!(operation, ClientCopyOperation::Search { .. }));
        if pending {
            if let Some(copy_mode) = self.copy_mode.as_mut() {
                copy_mode.copy_after_search = true;
            }
        }
        pending
    }

    /// Enter commit for the search prompt; a live prompt-edit result commits in place.
    fn commit_copy_search(
        &mut self,
        query: String,
        direction: crate::api::schema::PaneCopySearchDirection,
        count: u32,
        outcome: &mut ClientShellInput,
    ) {
        if self.copy_mode.is_none() {
            return;
        }
        if query.is_empty() {
            self.copy_operation_queue
                .retain(|operation| !matches!(operation, ClientCopyOperation::Search { .. }));
            self.retire_cancelled_copy_search();
            if let Some(copy_mode) = self.copy_mode.as_mut() {
                copy_mode.search_generation = copy_mode.search_generation.saturating_add(1);
                copy_mode.copy_after_search = false;
            }
            return;
        }
        let cached_commit = self.copy_mode.as_ref().is_some_and(|copy_mode| {
            copy_mode.search_query == query
                && copy_mode.search_direction == Some(direction)
                && copy_mode.search_cache_generation == Some(copy_mode.search_generation)
                && !self.copy_search_request_outstanding()
        });
        if cached_commit {
            let target = self.copy_mode.as_ref().and_then(|copy_mode| {
                copy_mode
                    .search_current
                    .and_then(|index| copy_mode.search_matches.get(index).copied())
            });
            if let Some(target) = target {
                if let Some(copy_mode) = self.copy_mode.as_mut() {
                    copy_mode.cursor = target.start;
                }
                self.reveal_copy_cursor(outcome, true);
                self.sync_copy_selection();
                outcome.repaint = true;
            }
        } else if !self.copy_search_request_outstanding() {
            self.copy_operation_queue
                .push_back(ClientCopyOperation::Search {
                    query: query.clone(),
                    direction,
                    repeat: false,
                    remaining: 1,
                });
        }
        if count > 1 {
            self.copy_operation_queue
                .push_back(ClientCopyOperation::Search {
                    query: query.clone(),
                    direction,
                    repeat: true,
                    remaining: count - 1,
                });
        }
        self.dispatch_next_copy_operation(outcome);
    }

    fn copy_search_request_outstanding(&self) -> bool {
        self.copy_operation_queue
            .iter()
            .any(|operation| matches!(operation, ClientCopyOperation::Search { .. }))
            || self.pending_requests.values().any(|pending| {
                matches!(
                    &pending.kind,
                    PendingEndpointKind::CopySearch {
                        session_generation,
                        ..
                    } if *session_generation == self.copy_session_generation
                )
            })
    }

    pub(super) fn apply_copy_search_result(
        &mut self,
        pane_id: &str,
        origin: crate::api::schema::PaneTextPoint,
        query: String,
        direction: crate::api::schema::PaneCopySearchDirection,
        repeat: bool,
        remaining: u32,
        generation: u64,
        result: ClientCopySearchResult,
        outcome: &mut ClientShellInput,
    ) -> CopySearchApply {
        let search_queued = self
            .copy_operation_queue
            .iter()
            .any(|operation| matches!(operation, ClientCopyOperation::Search { .. }));
        let Some(copy_mode) = self.copy_mode.as_mut() else {
            return CopySearchApply::Stale;
        };
        if copy_mode.pane_id != pane_id
            || copy_mode.cursor != origin
            || copy_mode.content_revision != result.content_revision
        {
            return CopySearchApply::Stale;
        }
        // A queued different-query search is newer, so this response is superseded even after the prompt closes.
        if self.copy_operation_queue.iter().any(|operation| {
            matches!(operation, ClientCopyOperation::Search { query: queued, .. } if *queued != query)
        }) {
            return CopySearchApply::Superseded;
        }
        if copy_mode
            .search_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.query.as_str() != query)
        {
            return CopySearchApply::Superseded;
        }
        if copy_mode.search_generation != generation {
            return if search_queued {
                CopySearchApply::Superseded
            } else {
                CopySearchApply::Stale
            };
        }
        let current = result.current.filter(|index| *index < result.matches.len());
        let chain_continues = repeat && remaining > 1 && current.is_some();
        copy_mode.search_query = query.clone();
        if !repeat {
            copy_mode.search_direction = Some(direction);
        }
        copy_mode.search_matches = result.matches;
        copy_mode.search_total = result.total;
        copy_mode.search_current = current;
        copy_mode.search_current_global = result.current_global;
        copy_mode.search_cache_generation = Some(copy_mode.search_generation);
        let prompt_open = copy_mode.search_prompt.is_some();
        let target = current.and_then(|index| copy_mode.search_matches.get(index).copied());
        let copy_after_search = if prompt_open || search_queued || chain_continues {
            false
        } else {
            std::mem::take(&mut copy_mode.copy_after_search)
        };
        // While the prompt is open the cursor stays anchored so refining the
        // query cannot skip the match the server picked from the entry point.
        if !prompt_open {
            if let Some(target) = target {
                copy_mode.cursor = target.start;
                self.reveal_copy_cursor(outcome, true);
                self.sync_copy_selection();
            }
            if copy_after_search {
                self.exit_copy_mode(true, outcome);
                return CopySearchApply::Applied;
            }
        }
        if chain_continues {
            self.copy_operation_queue
                .push_front(ClientCopyOperation::Search {
                    query,
                    direction,
                    repeat: true,
                    remaining: remaining - 1,
                });
        }
        outcome.repaint = true;
        CopySearchApply::Applied
    }

    pub(super) fn complete_copy_operation(
        &mut self,
        session_generation: u64,
        continue_queue: bool,
        outcome: &mut ClientShellInput,
    ) {
        if self.copy_session_generation != session_generation {
            return;
        }
        self.copy_operation_in_flight = false;
        if continue_queue && self.copy_mode.is_some() {
            self.dispatch_next_copy_operation(outcome);
            self.dispatch_queued_copy_input(outcome);
        } else {
            self.copy_operation_queue.clear();
            self.copy_input_queue.clear();
        }
    }

    fn dispatch_queued_copy_input(&mut self, outcome: &mut ClientShellInput) {
        while !self.copy_operation_in_flight {
            let Some(key) = self.copy_input_queue.pop_front() else {
                return;
            };
            self.handle_key(key, outcome);
        }
    }

    pub(super) fn cancel_deferred_copy_after_search(&mut self, generation: u64) {
        if let Some(copy_mode) = self
            .copy_mode
            .as_mut()
            .filter(|copy_mode| copy_mode.search_generation == generation)
        {
            copy_mode.copy_after_search = false;
        }
    }

    /// Re-anchor the copy cursor after a pane geometry or screen-mode change.
    pub(super) fn reclamp_copy_cursor_after_surface_change(
        &mut self,
        pane_id: &str,
        inner: crate::protocol::SurfaceRect,
        screen_switched: bool,
        frame_cursor: Option<crate::protocol::CursorState>,
    ) {
        let Some(copy_mode) = self
            .copy_mode
            .as_mut()
            .filter(|copy_mode| copy_mode.pane_id == pane_id)
        else {
            return;
        };
        copy_mode.cursor.col = copy_mode.cursor.col.min(inner.width.saturating_sub(1));
        let viewport_top = copy_mode
            .max_offset_from_bottom
            .saturating_sub(copy_mode.offset_from_bottom) as u32;
        if screen_switched {
            // Absolute rows from the other screen are stranded; re-anchor from the visible cursor.
            let anchored = frame_cursor.filter(|cursor| {
                cursor.visible
                    && cursor.x >= inner.x
                    && cursor.x < inner.x.saturating_add(inner.width)
                    && cursor.y >= inner.y
                    && cursor.y < inner.y.saturating_add(inner.height)
            });
            if let Some(cursor) = anchored {
                copy_mode.cursor.row = viewport_top.saturating_add(u32::from(cursor.y - inner.y));
                copy_mode.cursor.col = cursor.x - inner.x;
            } else {
                copy_mode.cursor.row =
                    viewport_top.saturating_add(u32::from(inner.height.saturating_sub(1)));
            }
        } else {
            let total_rows = copy_mode
                .max_offset_from_bottom
                .saturating_add(usize::from(inner.height))
                .max(1);
            copy_mode.cursor.row = copy_mode
                .cursor
                .row
                .min(total_rows.saturating_sub(1).min(u32::MAX as usize) as u32);
        }
    }

    fn copy_hit(&self) -> Option<PaneHit> {
        let pane_id = self.copy_mode.as_ref()?.pane_id.as_str();
        self.hits
            .panes
            .iter()
            .find(|hit| hit.pane_id == pane_id)
            .cloned()
    }

    fn move_copy_cursor(&mut self, row_delta: i16, col_delta: i16, outcome: &mut ClientShellInput) {
        let Some(hit) = self.copy_hit() else {
            self.exit_copy_mode(false, outcome);
            return;
        };
        let Some(copy_mode) = self.copy_mode.as_mut() else {
            return;
        };
        if col_delta < 0 {
            copy_mode.cursor.col = copy_mode
                .cursor
                .col
                .saturating_sub(col_delta.unsigned_abs());
        } else if col_delta > 0 {
            copy_mode.cursor.col = copy_mode
                .cursor
                .col
                .saturating_add(col_delta as u16)
                .min(hit.inner_rect.width.saturating_sub(1));
        }
        let total_rows = copy_mode
            .max_offset_from_bottom
            .saturating_add(hit.inner_rect.height as usize)
            .max(1);
        if row_delta < 0 {
            copy_mode.cursor.row = copy_mode
                .cursor
                .row
                .saturating_sub(u32::from(row_delta.unsigned_abs()));
        } else if row_delta > 0 {
            copy_mode.cursor.row = copy_mode
                .cursor
                .row
                .saturating_add(u32::from(row_delta as u16))
                .min(total_rows.saturating_sub(1).min(u32::MAX as usize) as u32);
        }
        self.reveal_copy_cursor(outcome, false);
        self.sync_copy_selection();
        outcome.repaint = true;
    }

    fn move_copy_page(
        &mut self,
        direction: i8,
        half_page: bool,
        count: Option<u32>,
        outcome: &mut ClientShellInput,
    ) {
        let Some(hit) = self.copy_hit() else {
            return;
        };
        let lines = crate::copy_mode::copy_mode_page_lines(hit.inner_rect.height, half_page)
            .saturating_mul(count.unwrap_or(1) as usize);
        let Some((pane_id, next_offset)) = self.copy_mode.as_mut().map(|copy_mode| {
            if direction < 0 {
                copy_mode.cursor.row = copy_mode.cursor.row.saturating_sub(lines as u32);
                copy_mode.offset_from_bottom = copy_mode
                    .offset_from_bottom
                    .saturating_add(lines)
                    .min(copy_mode.max_offset_from_bottom);
            } else {
                let last_row = copy_mode
                    .max_offset_from_bottom
                    .saturating_add(hit.inner_rect.height as usize)
                    .saturating_sub(1)
                    .min(u32::MAX as usize) as u32;
                copy_mode.cursor.row = copy_mode
                    .cursor
                    .row
                    .saturating_add(lines as u32)
                    .min(last_row);
                copy_mode.offset_from_bottom = copy_mode.offset_from_bottom.saturating_sub(lines);
            }
            (copy_mode.pane_id.clone(), copy_mode.offset_from_bottom)
        }) else {
            return;
        };
        self.push_pane_scroll_offset(pane_id, next_offset, outcome);
        self.sync_copy_selection();
        outcome.repaint = true;
    }

    fn move_copy_history(&mut self, top: bool, count: Option<u32>, outcome: &mut ClientShellInput) {
        let Some(hit) = self.copy_hit() else {
            return;
        };
        let Some((pane_id, offset_from_bottom, counted)) =
            self.copy_mode.as_mut().map(|copy_mode| {
                let last_row = copy_mode
                    .max_offset_from_bottom
                    .saturating_add(hit.inner_rect.height as usize)
                    .saturating_sub(1)
                    .min(u32::MAX as usize) as u32;
                if top {
                    copy_mode.cursor.row = 0;
                    copy_mode.offset_from_bottom = copy_mode.max_offset_from_bottom;
                } else {
                    copy_mode.cursor.row = match count {
                        // vim: NG jumps to absolute row N-1, clamped to the last row.
                        Some(count) => count.saturating_sub(1).min(last_row),
                        None => last_row,
                    };
                    if count.is_none() {
                        copy_mode.offset_from_bottom = 0;
                    }
                }
                (
                    copy_mode.pane_id.clone(),
                    copy_mode.offset_from_bottom,
                    count.is_some() && !top,
                )
            })
        else {
            return;
        };
        if counted {
            self.reveal_copy_cursor(outcome, false);
        } else {
            self.push_pane_scroll_offset(pane_id, offset_from_bottom, outcome);
        }
        self.sync_copy_selection();
        outcome.repaint = true;
    }

    fn move_copy_viewport_line(
        &mut self,
        line: CopyViewportLine,
        count: Option<u32>,
        outcome: &mut ClientShellInput,
    ) {
        let Some(hit) = self.copy_hit() else {
            return;
        };
        let height = u32::from(hit.inner_rect.height);
        let Some(copy_mode) = self.copy_mode.as_mut() else {
            return;
        };
        let top = copy_mode
            .max_offset_from_bottom
            .saturating_sub(copy_mode.offset_from_bottom) as u32;
        let last_row = copy_mode
            .max_offset_from_bottom
            .saturating_add(hit.inner_rect.height as usize)
            .saturating_sub(1)
            .min(u32::MAX as usize) as u32;
        let count_offset = count
            .unwrap_or(1)
            .saturating_sub(1)
            .min(height.saturating_sub(1));
        let row = match line {
            CopyViewportLine::Top => top.saturating_add(count_offset),
            CopyViewportLine::Middle => top + height / 2,
            CopyViewportLine::Bottom => top
                .saturating_add(height.saturating_sub(1))
                .saturating_sub(count_offset),
        };
        copy_mode.cursor.row = row.min(last_row);
        self.reveal_copy_cursor(outcome, false);
        self.sync_copy_selection();
        outcome.repaint = true;
    }

    fn set_copy_cursor_col(&mut self, col: u16) {
        if let Some(copy_mode) = self.copy_mode.as_mut() {
            copy_mode.cursor.col = col;
        }
    }

    fn reveal_copy_cursor(&mut self, outcome: &mut ClientShellInput, reserve_mode_bar_row: bool) {
        let Some(hit) = self.copy_hit() else {
            return;
        };
        let request = self.copy_mode.as_mut().and_then(|copy_mode| {
            let current_top = copy_mode
                .max_offset_from_bottom
                .saturating_sub(copy_mode.offset_from_bottom) as u32;
            let max_cursor_row = hit
                .inner_rect
                .height
                .saturating_sub(if reserve_mode_bar_row { 2 } else { 1 });
            let bottom = current_top.saturating_add(u32::from(max_cursor_row));
            let desired_top = if copy_mode.cursor.row < current_top {
                copy_mode.cursor.row
            } else if copy_mode.cursor.row > bottom {
                copy_mode
                    .cursor
                    .row
                    .saturating_sub(u32::from(max_cursor_row))
            } else {
                current_top
            };
            let offset = copy_mode
                .max_offset_from_bottom
                .saturating_sub(desired_top as usize);
            if offset == copy_mode.offset_from_bottom {
                return None;
            }
            copy_mode.offset_from_bottom = offset;
            Some((copy_mode.pane_id.clone(), offset))
        });
        if let Some((pane_id, offset)) = request {
            self.push_pane_scroll_offset(pane_id, offset, outcome);
        }
    }

    fn begin_copy_selection(&mut self, kind: CopySelectionKind) {
        let end_col = self
            .copy_hit()
            .map_or(0, |hit| hit.inner_rect.width.saturating_sub(1));
        let Some(copy_mode) = self.copy_mode.as_mut() else {
            return;
        };
        if let Some(active) = copy_mode.selection {
            let active_kind = match active {
                ClientCopySelection::Character { .. } => CopySelectionKind::Character,
                ClientCopySelection::Linewise { .. } => CopySelectionKind::Linewise,
                ClientCopySelection::Block { .. } => CopySelectionKind::Block,
            };
            if active_kind != kind {
                // Vim keeps the original anchor when the visual kind changes.
                let anchor = match active {
                    ClientCopySelection::Character { anchor }
                    | ClientCopySelection::Block { anchor } => anchor,
                    ClientCopySelection::Linewise { anchor_row } => {
                        crate::api::schema::PaneTextPoint {
                            row: anchor_row,
                            col: 0,
                        }
                    }
                };
                copy_mode.selection = Some(match kind {
                    CopySelectionKind::Character => ClientCopySelection::Character { anchor },
                    CopySelectionKind::Linewise => ClientCopySelection::Linewise {
                        anchor_row: anchor.row,
                    },
                    CopySelectionKind::Block => ClientCopySelection::Block { anchor },
                });
                self.sync_copy_selection();
                return;
            }
        }
        if kind == CopySelectionKind::Linewise {
            copy_mode.selection = Some(ClientCopySelection::Linewise {
                anchor_row: copy_mode.cursor.row,
            });
            self.selection = Some(crate::selection::Selection::line_range(
                copy_mode.pane_id.clone(),
                copy_mode.cursor.row,
                copy_mode.cursor.row,
                end_col,
            ));
        } else {
            let anchor = copy_mode.cursor;
            copy_mode.selection = Some(if kind == CopySelectionKind::Block {
                ClientCopySelection::Block { anchor }
            } else {
                ClientCopySelection::Character { anchor }
            });
            self.selection = Some(if kind == CopySelectionKind::Block {
                crate::selection::Selection::block_range(
                    copy_mode.pane_id.clone(),
                    (anchor.row, anchor.col),
                    (anchor.row, anchor.col),
                )
            } else {
                crate::selection::Selection::absolute_anchor(
                    copy_mode.pane_id.clone(),
                    (anchor.row, anchor.col),
                )
            });
        }
    }

    pub(super) fn sync_copy_selection(&mut self) {
        let Some(copy_mode) = self.copy_mode.as_ref() else {
            return;
        };
        let Some(selection) = copy_mode.selection else {
            return;
        };
        self.selection = Some(match selection {
            ClientCopySelection::Character { anchor } => {
                crate::selection::Selection::absolute_range(
                    copy_mode.pane_id.clone(),
                    (anchor.row, anchor.col),
                    (copy_mode.cursor.row, copy_mode.cursor.col),
                )
            }
            ClientCopySelection::Linewise { anchor_row } => {
                crate::selection::Selection::line_range(
                    copy_mode.pane_id.clone(),
                    anchor_row,
                    copy_mode.cursor.row,
                    self.copy_hit()
                        .map_or(0, |hit| hit.inner_rect.width.saturating_sub(1)),
                )
            }
            ClientCopySelection::Block { anchor } => crate::selection::Selection::block_range(
                copy_mode.pane_id.clone(),
                (anchor.row, anchor.col),
                (copy_mode.cursor.row, copy_mode.cursor.col),
            ),
        });
    }

    fn request_copy_motion(
        &mut self,
        motion: crate::api::schema::PaneCopyMotion,
        outcome: &mut ClientShellInput,
    ) {
        if self.copy_mode.is_none() {
            return;
        }
        self.copy_operation_queue
            .push_back(ClientCopyOperation::Motion {
                motion,
                remaining: 1,
            });
        self.dispatch_next_copy_operation(outcome);
    }

    fn request_copy_motion_repeat(
        &mut self,
        motion: crate::api::schema::PaneCopyMotion,
        count: Option<u32>,
        outcome: &mut ClientShellInput,
    ) {
        if self.copy_mode.is_none() {
            return;
        }
        self.copy_operation_queue
            .push_back(ClientCopyOperation::Motion {
                motion,
                remaining: count.unwrap_or(1).max(1),
            });
        self.dispatch_next_copy_operation(outcome);
    }

    fn request_copy_find(
        &mut self,
        ch: char,
        forward: bool,
        till: bool,
        count: Option<u32>,
        repeat: bool,
        outcome: &mut ClientShellInput,
    ) {
        self.request_copy_object(
            crate::api::schema::PaneCopyObjectRequest::Find {
                ch,
                direction: if forward {
                    crate::api::schema::PaneCopySearchDirection::Forward
                } else {
                    crate::api::schema::PaneCopySearchDirection::Backward
                },
                till,
                count: count.unwrap_or(1),
                repeat,
            },
            outcome,
        );
    }

    fn request_copy_horizontal(
        &mut self,
        forward: bool,
        count: Option<u32>,
        outcome: &mut ClientShellInput,
    ) {
        let delta = copy_motion_times(count);
        self.move_copy_cursor(0, if forward { delta } else { -delta }, outcome);
    }

    fn request_copy_object(
        &mut self,
        request: crate::api::schema::PaneCopyObjectRequest,
        outcome: &mut ClientShellInput,
    ) {
        if self.copy_mode.is_none() {
            return;
        }
        self.copy_operation_queue
            .push_back(ClientCopyOperation::CopyObject(request));
        self.dispatch_next_copy_operation(outcome);
    }

    fn take_copy_count(&mut self) -> Option<u32> {
        self.copy_mode.as_mut()?.pending_count.take()
    }

    /// Cancel a pending f/F/t/T target or text-object prefix.
    fn clear_copy_pending_prefixes(&mut self) {
        if let Some(copy_mode) = self.copy_mode.as_mut() {
            copy_mode.pending_find = None;
            copy_mode.pending_text_object = None;
        }
    }

    fn set_copy_pending_find(&mut self, forward: bool, till: bool) {
        if let Some(copy_mode) = self.copy_mode.as_mut() {
            copy_mode.pending_find = Some((forward, till));
        }
    }

    fn set_copy_pending_text_object(&mut self, around: bool) {
        if let Some(copy_mode) = self.copy_mode.as_mut() {
            copy_mode.pending_text_object = Some(around);
        }
    }

    pub(super) fn dispatch_next_copy_operation(&mut self, outcome: &mut ClientShellInput) {
        if self.copy_operation_in_flight {
            return;
        }
        while let Some(operation) = self.copy_operation_queue.pop_front() {
            let Some(copy_mode) = self.copy_mode.as_mut() else {
                self.copy_operation_queue.clear();
                self.copy_input_queue.clear();
                return;
            };
            let session_generation = self.copy_session_generation;
            let pane_id = copy_mode.pane_id.clone();
            let origin = copy_mode.cursor;
            let (method, kind) = match operation {
                ClientCopyOperation::Motion { motion, remaining } => (
                    crate::api::schema::Method::PaneCopyMotion(
                        crate::api::schema::PaneCopyMotionParams {
                            pane_id: pane_id.clone(),
                            cursor: origin,
                            motion,
                            content_revision: Some(copy_mode.content_revision),
                        },
                    ),
                    PendingEndpointKind::CopyMotion {
                        pane_id,
                        origin,
                        motion,
                        remaining,
                        session_generation,
                    },
                ),
                ClientCopyOperation::CopyObject(request) => (
                    crate::api::schema::Method::PaneCopyObject(
                        crate::api::schema::PaneCopyObjectParams {
                            pane_id: pane_id.clone(),
                            cursor: origin,
                            request,
                            content_revision: Some(copy_mode.content_revision),
                        },
                    ),
                    PendingEndpointKind::CopyObject {
                        pane_id,
                        origin,
                        request,
                        session_generation,
                    },
                ),
                ClientCopyOperation::Search {
                    query,
                    direction,
                    repeat,
                    remaining,
                } => {
                    if query.is_empty() {
                        continue;
                    }
                    copy_mode.search_generation = copy_mode.search_generation.saturating_add(1);
                    let generation = copy_mode.search_generation;
                    let previous = repeat
                        .then(|| {
                            copy_mode
                                .search_current
                                .and_then(|index| copy_mode.search_matches.get(index).copied())
                                .filter(|text_match| text_match.start == copy_mode.cursor)
                        })
                        .flatten();
                    (
                        crate::api::schema::Method::PaneCopySearch(
                            crate::api::schema::PaneCopySearchParams {
                                pane_id: pane_id.clone(),
                                query: query.clone(),
                                direction,
                                cursor: origin,
                                content_revision: copy_mode.content_revision,
                                previous,
                            },
                        ),
                        PendingEndpointKind::CopySearch {
                            pane_id,
                            origin,
                            query,
                            direction,
                            repeat,
                            remaining,
                            generation,
                            session_generation,
                        },
                    )
                }
            };
            self.copy_operation_in_flight = true;
            if !self.push_endpoint_method_with_kind(method, kind, outcome) {
                self.copy_operation_in_flight = false;
                self.copy_operation_queue.clear();
                self.copy_input_queue.clear();
            }
            return;
        }
    }

    pub(super) fn apply_copy_motion_target(
        &mut self,
        pane_id: &str,
        origin: crate::api::schema::PaneTextPoint,
        cursor: crate::api::schema::PaneTextPoint,
        content_revision: u64,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(copy_mode) = self.copy_mode.as_mut() else {
            return false;
        };
        if copy_mode.pane_id != pane_id
            || copy_mode.cursor != origin
            || copy_mode.content_revision != content_revision
        {
            return false;
        }
        copy_mode.cursor = cursor;
        self.reveal_copy_cursor(outcome, false);
        self.sync_copy_selection();
        outcome.repaint = true;
        true
    }

    pub(super) fn apply_copy_object_range(
        &mut self,
        pane_id: &str,
        origin: crate::api::schema::PaneTextPoint,
        range: crate::api::schema::PaneTextRange,
        content_revision: u64,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(copy_mode) = self.copy_mode.as_mut() else {
            return false;
        };
        if copy_mode.pane_id != pane_id
            || copy_mode.cursor != origin
            || copy_mode.content_revision != content_revision
        {
            return false;
        }
        copy_mode.selection = Some(ClientCopySelection::Character {
            anchor: range.start,
        });
        copy_mode.cursor = range.end;
        self.reveal_copy_cursor(outcome, false);
        self.sync_copy_selection();
        outcome.repaint = true;
        true
    }

    pub(super) fn exit_copy_mode(&mut self, copy: bool, outcome: &mut ClientShellInput) {
        let live_selection = self
            .selection
            .as_ref()
            .is_some_and(crate::selection::Selection::is_visible);
        if copy && !live_selection {
            if let Some((pane_id, text_match)) = self.copy_mode.as_ref().and_then(|copy_mode| {
                copy_mode
                    .search_current
                    .and_then(|index| copy_mode.search_matches.get(index).copied())
                    .map(|text_match| (copy_mode.pane_id.clone(), text_match))
            }) {
                self.selection = Some(crate::selection::Selection::absolute_range(
                    pane_id,
                    (text_match.start.row, text_match.start.col),
                    (text_match.end.row, text_match.end.col),
                ));
            }
        }
        let Some(copy_mode) = self.copy_mode.take() else {
            return;
        };
        self.reset_copy_pipeline();
        if copy
            && self
                .selection
                .as_ref()
                .is_some_and(crate::selection::Selection::is_visible)
        {
            // A visible explicit selection is live. If the fallback above supplied
            // a search match, retain the revision that established its boundaries.
            self.request_selection_copy(outcome, live_selection);
        }
        self.selection = None;
        self.selection_highlight_clear_deadline = None;
        self.push_pane_scroll_offset(
            copy_mode.pane_id,
            copy_mode.entry_offset_from_bottom,
            outcome,
        );
        self.mode = ClientShellMode::Terminal;
        outcome.repaint = true;
    }
}
