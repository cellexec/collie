//! Keyboard focus for the sidebar.
//!
//! Navigate mode is the sidebar's keyboard focus: the spaces and agents
//! sections each keep a cursor, moving it previews the target in the pane
//! surface, and Enter commits the selection. All of this is TUI presentation
//! state; the server only sees ordinary focus requests.

use super::*;
use crate::api::schema::AgentStatus;
use crossterm::event::KeyModifiers;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SidebarSection {
    #[default]
    Spaces,
    Agents,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SidebarAgentCursor {
    pub(super) endpoint_id: ClientEndpointId,
    pub(super) pane_id: String,
}

/// Focus to restore when the sidebar is left with Esc.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SidebarOrigin {
    endpoint_id: ClientEndpointId,
    workspace_id: Option<String>,
    pane_id: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct SidebarFocusState {
    pub(super) section: SidebarSection,
    pub(super) agent_cursor: Option<SidebarAgentCursor>,
    pub(super) workspace_query: TextEditor,
    pub(super) agent_query: TextEditor,
    /// The inline `/` filter of the focused section receives typed keys.
    pub(super) editing: bool,
    pub(super) agent_status: Option<AgentStatus>,
    origin: Option<SidebarOrigin>,
    /// The sidebar was collapsed and is shown only while it has focus.
    drawer: bool,
    /// First key of a fixed two-key sequence (`gg`, `za`, `dd`).
    pending: Option<char>,
}

impl SidebarFocusState {
    pub(super) fn pending_close(&self) -> bool {
        self.pending == Some('d')
    }

    pub(super) fn render_view(
        &self,
        mode: ClientShellMode,
        prefix_return_navigate: bool,
    ) -> SidebarRenderView<'_> {
        SidebarRenderView {
            focused: mode == ClientShellMode::Navigate
                || (mode == ClientShellMode::Prefix && prefix_return_navigate),
            section: self.section,
            agent_cursor: self.agent_cursor.as_ref(),
            workspace_query: self.workspace_query.as_str(),
            agent_query: self.agent_query.as_str(),
            agent_status: self.agent_status,
            editing: self.editing,
        }
    }
}

/// Read-only projection of the sidebar focus for rendering.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SidebarRenderView<'a> {
    pub(super) focused: bool,
    pub(super) section: SidebarSection,
    pub(super) agent_cursor: Option<&'a SidebarAgentCursor>,
    pub(super) workspace_query: &'a str,
    pub(super) agent_query: &'a str,
    pub(super) agent_status: Option<AgentStatus>,
    pub(super) editing: bool,
}

impl SidebarRenderView<'_> {
    pub(super) fn section_active(&self, section: SidebarSection) -> bool {
        self.focused && self.section == section
    }

    pub(super) fn agent_selected(&self, endpoint_id: &ClientEndpointId, pane_id: &str) -> bool {
        self.section_active(SidebarSection::Agents)
            && self.agent_cursor.is_some_and(|cursor| {
                &cursor.endpoint_id == endpoint_id && cursor.pane_id == pane_id
            })
    }

    pub(super) fn agent_visible(
        &self,
        snapshot: &ClientShellSnapshot,
        agent: &crate::protocol::ClientShellAgent,
    ) -> bool {
        agent_matches_filter(snapshot, agent, self.agent_status, self.agent_query)
    }

    /// Header suffix describing the section's filter, if any.
    pub(super) fn filter_label(&self, section: SidebarSection) -> Option<String> {
        let query = match section {
            SidebarSection::Spaces => self.workspace_query,
            SidebarSection::Agents => self.agent_query,
        };
        let editing = self.editing && self.section_active(section);
        let mut parts = Vec::new();
        if section == SidebarSection::Agents {
            if let Some(status) = self.agent_status {
                parts.push(agent_status_label(status).to_owned());
            }
        }
        if editing || !query.is_empty() {
            parts.push(format!("/{query}{}", if editing { "▏" } else { "" }));
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

fn agent_status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Blocked => "blocked",
        AgentStatus::Working => "working",
        AgentStatus::Idle => "idle",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    }
}

fn query_matches(query: &str, haystack: &[Option<&str>]) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let haystack = haystack
        .iter()
        .flatten()
        .map(|value| value.to_lowercase())
        .collect::<Vec<_>>();
    query
        .split_whitespace()
        .all(|word| haystack.iter().any(|value| value.contains(word)))
}

pub(super) fn workspace_matches_filter(workspace: &ClientShellWorkspace, query: &str) -> bool {
    query_matches(
        query,
        &[
            Some(workspace.label.as_str()),
            workspace.branch.as_deref(),
            workspace
                .worktree
                .as_ref()
                .map(|worktree| worktree.label.as_str()),
        ],
    )
}

pub(super) fn agent_matches_filter(
    snapshot: &ClientShellSnapshot,
    agent: &crate::protocol::ClientShellAgent,
    status: Option<AgentStatus>,
    query: &str,
) -> bool {
    if status.is_some_and(|status| agent.agent_status != status) {
        return false;
    }
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == agent.workspace_id)
        .map(|workspace| workspace.label.as_str());
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == agent.pane_id)
        .and_then(|pane| pane.label.as_deref());
    query_matches(
        query,
        &[
            agent.name.as_deref(),
            agent.display_agent.as_deref(),
            agent.agent.as_deref(),
            agent.title.as_deref(),
            agent.terminal_title_stripped.as_deref(),
            workspace,
            pane,
        ],
    )
}

/// A key bound in `[keys]` for sidebar focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarCommand {
    Leader,
    Leave,
    Spaces,
    Agents,
    Up,
    Down,
    Bottom,
    Filter,
    New,
    Rename,
    Close,
    Sort,
    StateFilter,
    MoveDown,
    MoveUp,
    Narrower,
    Wider,
    GrowSpaces,
    GrowAgents,
    Menu,
    Help,
    PaneLeft,
    PaneDown,
    PaneUp,
    PaneRight,
}

fn classify_sidebar_key(
    keys: &crate::config::NavigateKeybinds,
    key: &crate::input::TerminalKey,
) -> Option<SidebarCommand> {
    use SidebarCommand as Command;
    [
        (&keys.leader, Command::Leader),
        (&keys.leave, Command::Leave),
        (&keys.spaces, Command::Spaces),
        (&keys.agents, Command::Agents),
        (&keys.workspace_up, Command::Up),
        (&keys.workspace_down, Command::Down),
        (&keys.bottom, Command::Bottom),
        (&keys.filter, Command::Filter),
        (&keys.new, Command::New),
        (&keys.rename, Command::Rename),
        (&keys.close, Command::Close),
        (&keys.sort, Command::Sort),
        (&keys.state_filter, Command::StateFilter),
        (&keys.move_down, Command::MoveDown),
        (&keys.move_up, Command::MoveUp),
        (&keys.narrower, Command::Narrower),
        (&keys.wider, Command::Wider),
        (&keys.grow_spaces, Command::GrowSpaces),
        (&keys.grow_agents, Command::GrowAgents),
        (&keys.menu, Command::Menu),
        (&keys.help, Command::Help),
        (&keys.pane_left, Command::PaneLeft),
        (&keys.pane_down, Command::PaneDown),
        (&keys.pane_up, Command::PaneUp),
        (&keys.pane_right, Command::PaneRight),
    ]
    .into_iter()
    .find_map(|(bindings, command)| bindings.matches_direct_key(key).then_some(command))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarStep {
    Delta(isize),
    Page(isize),
    First,
    Last,
}

const SIDEBAR_WIDTH_STEP: u16 = 2;
const SIDEBAR_SPLIT_STEP: f32 = 0.05;

impl ClientShellState {
    /// Navigate mode entry from `workspace_picker`.
    pub(super) fn enter_sidebar_focus(&mut self, outcome: &mut ClientShellInput) {
        let origin = SidebarOrigin {
            endpoint_id: self.active_endpoint_id.clone(),
            workspace_id: self
                .snapshot
                .as_deref()
                .and_then(|snapshot| snapshot.focused_workspace_id.clone()),
            pane_id: self.focused_pane_id(),
        };
        let focus = &mut self.sidebar_focus;
        focus.pending = None;
        focus.editing = false;
        focus.origin = Some(origin);
        if self.sidebar_collapsed && !self.mobile_layout_active() {
            self.sidebar_focus.drawer = true;
            self.sidebar_collapsed = false;
            self.invalidate_pane_surface();
            outcome.resize = true;
        }
        if !self.sidebar_agent_cursor_valid() {
            self.sidebar_focus.agent_cursor = self.focused_sidebar_agent();
        }
        outcome.repaint = true;
    }

    /// Collapse a drawer sidebar once its focus has gone elsewhere.
    pub(super) fn sync_sidebar_drawer(&mut self, outcome: &mut ClientShellInput) {
        if self
            .sidebar_focus
            .render_view(self.mode, self.prefix_return_navigate)
            .focused
            || !self.sidebar_focus.drawer
        {
            return;
        }
        self.sidebar_focus.drawer = false;
        self.sidebar_focus.origin = None;
        self.sidebar_focus.pending = None;
        self.sidebar_focus.editing = false;
        if !self.sidebar_collapsed {
            self.sidebar_collapsed = true;
            self.invalidate_pane_surface();
            outcome.resize = true;
            outcome.repaint = true;
        }
    }

    /// A picker jump moved focus to a pane; the sidebar gives up focus with it.
    pub(super) fn leave_sidebar_after_jump(&mut self, outcome: &mut ClientShellInput) {
        self.leave_sidebar(false, outcome);
    }

    fn leave_sidebar(&mut self, restore: bool, outcome: &mut ClientShellInput) {
        let origin = self.sidebar_focus.origin.take();
        self.sidebar_focus.pending = None;
        self.sidebar_focus.editing = false;
        if let Some(origin) = origin.filter(|origin| {
            restore && origin.endpoint_id == self.active_endpoint_id && self.snapshot.is_some()
        }) {
            let snapshot = self.snapshot.as_deref();
            let pane_exists = origin.pane_id.as_deref().is_some_and(|pane_id| {
                snapshot.is_some_and(|snapshot| {
                    snapshot.panes.iter().any(|pane| pane.pane_id == pane_id)
                })
            });
            let focused_pane = self.focused_pane_id();
            if pane_exists && focused_pane != origin.pane_id {
                if let Some(pane_id) = origin.pane_id {
                    self.focus_or_activate(
                        origin.endpoint_id,
                        ClientEndpointFocusTarget::Pane(pane_id),
                        outcome,
                    );
                }
            } else if !pane_exists {
                let focused_workspace =
                    snapshot.and_then(|snapshot| snapshot.focused_workspace_id.clone());
                if let Some(workspace_id) = origin.workspace_id.filter(|workspace_id| {
                    Some(workspace_id) != focused_workspace.as_ref()
                        && snapshot.is_some_and(|snapshot| {
                            snapshot
                                .workspaces
                                .iter()
                                .any(|workspace| &workspace.workspace_id == workspace_id)
                        })
                }) {
                    self.focus_or_activate(
                        origin.endpoint_id,
                        ClientEndpointFocusTarget::Workspace(workspace_id),
                        outcome,
                    );
                }
            }
        }
        self.mode = self.copy_or_terminal_mode();
        self.navigate_workspace_id = None;
        self.sync_sidebar_drawer(outcome);
        outcome.repaint = true;
    }

    fn commit_sidebar(&mut self, outcome: &mut ClientShellInput) {
        match self.sidebar_focus.section {
            SidebarSection::Spaces => {
                self.sidebar_focus.origin = None;
                self.accept_navigate_workspace(outcome);
                if self.mode != ClientShellMode::Navigate {
                    self.sidebar_focus.pending = None;
                    self.sidebar_focus.editing = false;
                    self.sync_sidebar_drawer(outcome);
                }
            }
            SidebarSection::Agents => {
                if let Some(cursor) = self
                    .sidebar_focus
                    .agent_cursor
                    .clone()
                    .filter(|_| self.sidebar_agent_cursor_valid())
                {
                    if !self.focus_or_activate(
                        cursor.endpoint_id,
                        ClientEndpointFocusTarget::Pane(cursor.pane_id),
                        outcome,
                    ) {
                        return;
                    }
                }
                self.leave_sidebar(false, outcome);
            }
        }
    }

    pub(super) fn route_sidebar_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        self.pending_workspace_highlight = None;
        if self.sidebar_focus.editing {
            self.route_sidebar_filter_key(key, outcome);
            return;
        }
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        let close_key = self
            .config
            .keybinds
            .keybinds
            .navigate
            .close
            .matches_direct_key(key);
        if let Some(pending) = self.sidebar_focus.pending.take() {
            outcome.repaint = true;
            match (pending, code, modifiers.is_empty()) {
                ('g', KeyCode::Char('g'), true) => {
                    self.step_sidebar(SidebarStep::First, outcome);
                    return;
                }
                ('z', KeyCode::Char('a'), true) => {
                    self.toggle_sidebar_group(outcome);
                    return;
                }
                ('d', _, _) if close_key => {
                    self.close_sidebar_agent(outcome);
                    return;
                }
                _ => {}
            }
        }

        if code == KeyCode::Esc && modifiers.is_empty() {
            self.escape_sidebar(outcome);
            return;
        }
        if self.config.keybinds.matches_prefix(key) {
            self.enter_prefix_from_sidebar(outcome);
            return;
        }
        if code == KeyCode::Enter && modifiers.is_empty() {
            self.commit_sidebar(outcome);
            return;
        }

        let command = classify_sidebar_key(&self.config.keybinds.keybinds.navigate, key);
        if let Some(command) = command {
            self.run_sidebar_command(command, outcome);
            return;
        }
        if super::input::is_ctrl_bracket_key(key) {
            self.escape_sidebar(outcome);
            return;
        }

        match (code, modifiers) {
            (KeyCode::Char('g'), m) if m.is_empty() => {
                self.sidebar_focus.pending = Some('g');
            }
            (KeyCode::Char('z'), m) if m.is_empty() => {
                self.sidebar_focus.pending = Some('z');
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                self.step_sidebar(SidebarStep::Page(1), outcome);
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.step_sidebar(SidebarStep::Page(-1), outcome);
            }
            _ => return,
        }
        outcome.repaint = true;
    }

    fn enter_prefix_from_sidebar(&mut self, outcome: &mut ClientShellInput) {
        self.sidebar_focus.pending = None;
        self.prefix_return_navigate = true;
        self.prefix_sequence = None;
        self.mode = ClientShellMode::Prefix;
        outcome.repaint = true;
    }

    fn run_sidebar_command(&mut self, command: SidebarCommand, outcome: &mut ClientShellInput) {
        use crate::input::{KeybindAction, KeybindMatch};
        use SidebarCommand as Command;

        outcome.repaint = true;
        let section = self.sidebar_focus.section;
        let acts_on_target = matches!(
            command,
            Command::New
                | Command::Rename
                | Command::Close
                | Command::MoveDown
                | Command::MoveUp
                | Command::PaneLeft
                | Command::PaneDown
                | Command::PaneUp
                | Command::PaneRight
        );
        if acts_on_target && self.workspace_preview_action_blocked() {
            self.push_endpoint_notice(
                ClientEndpointNoticeKind::Rejected,
                "navigate_endpoint_inactive",
                "Confirm workspace first",
                "Select an available workspace and press Enter before using workspace or pane actions",
            );
            return;
        }
        match command {
            Command::Leader => self.enter_prefix_from_sidebar(outcome),
            Command::Leave => self.leave_sidebar(false, outcome),
            Command::Spaces => self.set_sidebar_section(SidebarSection::Spaces),
            Command::Agents => self.set_sidebar_section(SidebarSection::Agents),
            Command::Up => self.step_sidebar(SidebarStep::Delta(-1), outcome),
            Command::Down => self.step_sidebar(SidebarStep::Delta(1), outcome),
            Command::Bottom => self.step_sidebar(SidebarStep::Last, outcome),
            Command::Filter => self.sidebar_focus.editing = true,
            Command::New => match section {
                SidebarSection::Spaces => {
                    self.record_binding(KeybindMatch::Action(KeybindAction::NewWorkspace), outcome)
                }
                SidebarSection::Agents => self.open_agent_commands(outcome),
            },
            Command::Rename => match section {
                SidebarSection::Spaces => self.record_binding(
                    KeybindMatch::Action(KeybindAction::RenameWorkspace),
                    outcome,
                ),
                SidebarSection::Agents => {
                    if let Some(cursor) = self.local_sidebar_agent_cursor() {
                        self.open_rename_pane_overlay_for(&cursor.pane_id);
                    }
                }
            },
            Command::Close => match section {
                SidebarSection::Spaces => self
                    .record_binding(KeybindMatch::Action(KeybindAction::CloseWorkspace), outcome),
                SidebarSection::Agents => {
                    if self.local_sidebar_agent_cursor().is_some() {
                        self.sidebar_focus.pending = Some('d');
                    }
                }
            },
            Command::Sort => {
                self.toggle_agent_panel_sort(outcome);
                self.reveal_sidebar_agent();
            }
            Command::StateFilter => {
                self.sidebar_focus.agent_status = match self.sidebar_focus.agent_status {
                    None => Some(AgentStatus::Blocked),
                    Some(AgentStatus::Blocked) => Some(AgentStatus::Working),
                    Some(AgentStatus::Working) => Some(AgentStatus::Idle),
                    Some(_) => None,
                };
                self.sidebar_focus.section = SidebarSection::Agents;
                self.agent_scroll = 0;
                self.reselect_sidebar_agent();
            }
            Command::MoveDown => self.move_sidebar_workspace(false, outcome),
            Command::MoveUp => self.move_sidebar_workspace(true, outcome),
            Command::Narrower => self.adjust_sidebar_width(false, outcome),
            Command::Wider => self.adjust_sidebar_width(true, outcome),
            Command::GrowSpaces => self.adjust_sidebar_split(SIDEBAR_SPLIT_STEP, outcome),
            Command::GrowAgents => self.adjust_sidebar_split(-SIDEBAR_SPLIT_STEP, outcome),
            Command::Menu => self.toggle_global_menu(),
            Command::Help => {
                self.record_binding(KeybindMatch::Action(KeybindAction::Help), outcome)
            }
            Command::PaneLeft | Command::PaneDown | Command::PaneUp | Command::PaneRight => {
                let action = match command {
                    Command::PaneLeft => KeybindAction::FocusPaneLeft,
                    Command::PaneDown => KeybindAction::FocusPaneDown,
                    Command::PaneUp => KeybindAction::FocusPaneUp,
                    _ => KeybindAction::FocusPaneRight,
                };
                self.record_binding(KeybindMatch::Action(action), outcome);
            }
        }
    }

    fn escape_sidebar(&mut self, outcome: &mut ClientShellInput) {
        let focus = &mut self.sidebar_focus;
        let filtered = match focus.section {
            SidebarSection::Spaces => !focus.workspace_query.is_empty(),
            SidebarSection::Agents => !focus.agent_query.is_empty() || focus.agent_status.is_some(),
        };
        if filtered {
            match focus.section {
                SidebarSection::Spaces => focus.workspace_query.clear(),
                SidebarSection::Agents => {
                    focus.agent_query.clear();
                    focus.agent_status = None;
                }
            }
            outcome.repaint = true;
            return;
        }
        self.leave_sidebar(true, outcome);
    }

    fn route_sidebar_filter_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        outcome.repaint = true;
        let section = self.sidebar_focus.section;
        if (code == KeyCode::Esc && modifiers.is_empty()) || super::input::is_ctrl_bracket_key(key)
        {
            self.sidebar_focus.editing = false;
            match section {
                SidebarSection::Spaces => self.sidebar_focus.workspace_query.clear(),
                SidebarSection::Agents => self.sidebar_focus.agent_query.clear(),
            }
            self.reselect_sidebar_section();
            return;
        }
        if code == KeyCode::Enter && modifiers.is_empty() {
            self.sidebar_focus.editing = false;
            match section {
                SidebarSection::Spaces => self.preview_navigate_workspace(outcome),
                SidebarSection::Agents => self.preview_sidebar_agent(outcome),
            }
            return;
        }
        if matches!(
            (code, modifiers),
            (KeyCode::Down, _) | (KeyCode::Char('n' | 'j'), KeyModifiers::CONTROL)
        ) {
            self.step_sidebar(SidebarStep::Delta(1), outcome);
            return;
        }
        if matches!(
            (code, modifiers),
            (KeyCode::Up, _) | (KeyCode::Char('p' | 'k'), KeyModifiers::CONTROL)
        ) {
            self.step_sidebar(SidebarStep::Delta(-1), outcome);
            return;
        }
        let editor = match section {
            SidebarSection::Spaces => &mut self.sidebar_focus.workspace_query,
            SidebarSection::Agents => &mut self.sidebar_focus.agent_query,
        };
        if editor.handle_key(key) == Some(true) {
            self.reselect_sidebar_section();
        }
    }

    pub(super) fn insert_sidebar_filter_text(&mut self, text: &str) -> bool {
        if self.mode != ClientShellMode::Navigate || !self.sidebar_focus.editing {
            return false;
        }
        let inserted = match self.sidebar_focus.section {
            SidebarSection::Spaces => self.sidebar_focus.workspace_query.insert(text),
            SidebarSection::Agents => self.sidebar_focus.agent_query.insert(text),
        };
        if inserted {
            self.reselect_sidebar_section();
        }
        inserted
    }

    /// Keep the cursor of the focused section on a visible row after a filter change.
    fn reselect_sidebar_section(&mut self) {
        match self.sidebar_focus.section {
            SidebarSection::Spaces => {
                let targets = self.navigate_workspace_targets();
                let current_visible = self
                    .navigate_workspace_id
                    .as_ref()
                    .is_some_and(|selected| targets.contains(selected));
                if !current_visible {
                    self.navigate_workspace_id = targets.into_iter().next();
                    self.workspace_scroll = 0;
                }
            }
            SidebarSection::Agents => {
                self.agent_scroll = 0;
                self.reselect_sidebar_agent();
            }
        }
    }

    fn set_sidebar_section(&mut self, section: SidebarSection) {
        self.sidebar_focus.section = section;
        self.sidebar_focus.pending = None;
        match section {
            SidebarSection::Spaces => {
                if self.navigate_workspace_id.is_none() {
                    self.navigate_workspace_id = self.focused_navigation_target();
                }
                self.reveal_navigation_workspace = true;
            }
            SidebarSection::Agents => {
                self.reselect_sidebar_agent();
                self.reveal_sidebar_agent();
            }
        }
    }

    fn step_sidebar(&mut self, step: SidebarStep, outcome: &mut ClientShellInput) {
        match self.sidebar_focus.section {
            SidebarSection::Spaces => {
                match step {
                    SidebarStep::Delta(delta) => self.move_navigate_workspace(delta),
                    SidebarStep::Page(direction) => {
                        let rows = self.sidebar_page_rows(self.hits.workspace_body.height, 2);
                        self.move_navigate_workspace_clamped(direction * rows);
                    }
                    SidebarStep::First => self.jump_navigate_workspace(false),
                    SidebarStep::Last => self.jump_navigate_workspace(true),
                }
                self.preview_navigate_workspace(outcome);
            }
            SidebarSection::Agents => {
                let targets = self.sidebar_agent_targets();
                if targets.is_empty() {
                    self.sidebar_focus.agent_cursor = None;
                    return;
                }
                let last = targets.len() - 1;
                let current = self
                    .sidebar_focus
                    .agent_cursor
                    .as_ref()
                    .and_then(|cursor| targets.iter().position(|target| target == cursor));
                let next = match (step, current) {
                    (SidebarStep::First, _) => 0,
                    (SidebarStep::Last, _) => last,
                    (SidebarStep::Delta(delta), Some(current)) => {
                        (current as isize + delta).rem_euclid(targets.len() as isize) as usize
                    }
                    (SidebarStep::Page(direction), Some(current)) => {
                        let rows = self.sidebar_page_rows(self.hits.agent_body.height, 2);
                        (current as isize + direction * rows).clamp(0, last as isize) as usize
                    }
                    (SidebarStep::Delta(delta) | SidebarStep::Page(delta), None) => {
                        if delta < 0 {
                            last
                        } else {
                            0
                        }
                    }
                };
                self.sidebar_focus.agent_cursor = targets.into_iter().nth(next);
                self.reveal_sidebar_agent();
                self.preview_sidebar_agent(outcome);
            }
        }
    }

    fn sidebar_page_rows(&self, body_height: u16, row_height: u16) -> isize {
        (body_height / row_height.max(1) / 2).max(1) as isize
    }

    /// Show the workspace under the cursor in the pane surface.
    pub(super) fn preview_navigate_workspace(&mut self, outcome: &mut ClientShellInput) {
        // With several machines a focus change can hand the client over to
        // another endpoint, so the preview stays a highlight there.
        if self.multi_endpoint_active() {
            return;
        }
        let Some(target) = self.navigate_workspace_id.clone() else {
            return;
        };
        if target.endpoint_id != self.active_endpoint_id || !self.navigation_target_valid(&target) {
            return;
        }
        let focused = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.focused_workspace_id.as_deref())
            == Some(target.workspace_id.as_str());
        if !focused {
            self.focus_or_activate(
                target.endpoint_id,
                ClientEndpointFocusTarget::Workspace(target.workspace_id),
                outcome,
            );
        }
    }

    /// Show the agent under the cursor in the pane surface.
    fn preview_sidebar_agent(&mut self, outcome: &mut ClientShellInput) {
        if self.multi_endpoint_active() {
            return;
        }
        let Some(cursor) = self.local_sidebar_agent_cursor() else {
            return;
        };
        if self.focused_pane_id().as_deref() != Some(cursor.pane_id.as_str()) {
            self.focus_or_activate(
                cursor.endpoint_id,
                ClientEndpointFocusTarget::Pane(cursor.pane_id),
                outcome,
            );
        }
    }

    /// Agents shown in the sidebar, in display order, after the sidebar filter.
    pub(super) fn sidebar_agent_targets(&self) -> Vec<SidebarAgentCursor> {
        let status = self.sidebar_focus.agent_status;
        let query = self.sidebar_focus.agent_query.as_str();
        if self.endpoints.len() > 1 {
            return super::aggregate_navigation::aggregate_agent_rows(
                &self.endpoints,
                &self.active_endpoint_id,
                self.config.agent_panel_sort,
            )
            .into_iter()
            .filter(|row| {
                !row.endpoint.stale()
                    && agent_matches_filter(row.endpoint.snapshot, row.agent, status, query)
            })
            .map(|row| SidebarAgentCursor {
                endpoint_id: row.endpoint.endpoint_id.clone(),
                pane_id: row.agent.pane_id.clone(),
            })
            .collect();
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return Vec::new();
        };
        super::agent_sidebar::ordered_agent_pane_ids(snapshot, self.config.agent_panel_sort)
            .into_iter()
            .filter(|pane_id| {
                snapshot
                    .agents
                    .iter()
                    .find(|agent| &agent.pane_id == pane_id)
                    .is_some_and(|agent| agent_matches_filter(snapshot, agent, status, query))
            })
            .map(|pane_id| SidebarAgentCursor {
                endpoint_id: self.active_endpoint_id.clone(),
                pane_id,
            })
            .collect()
    }

    fn sidebar_agent_cursor_valid(&self) -> bool {
        self.sidebar_focus
            .agent_cursor
            .as_ref()
            .is_some_and(|cursor| self.sidebar_agent_targets().contains(cursor))
    }

    fn focused_sidebar_agent(&self) -> Option<SidebarAgentCursor> {
        let snapshot = self.snapshot.as_deref()?;
        let targets = self.sidebar_agent_targets();
        targets
            .iter()
            .find(|target| {
                target.endpoint_id == self.active_endpoint_id
                    && snapshot.focused_pane_id.as_deref() == Some(target.pane_id.as_str())
            })
            .cloned()
            .or_else(|| targets.into_iter().next())
    }

    fn reselect_sidebar_agent(&mut self) {
        if !self.sidebar_agent_cursor_valid() {
            self.sidebar_focus.agent_cursor = self.focused_sidebar_agent();
        }
    }

    /// The agent cursor when it targets the active endpoint and still exists.
    fn local_sidebar_agent_cursor(&self) -> Option<SidebarAgentCursor> {
        self.sidebar_focus
            .agent_cursor
            .clone()
            .filter(|cursor| cursor.endpoint_id == self.active_endpoint_id)
            .filter(|cursor| {
                self.snapshot.as_deref().is_some_and(|snapshot| {
                    snapshot
                        .agents
                        .iter()
                        .any(|agent| agent.pane_id == cursor.pane_id)
                })
            })
    }

    fn reveal_sidebar_agent(&mut self) {
        let body_height = self.hits.agent_body.height;
        if body_height == 0 {
            return;
        }
        let Some(cursor) = self.sidebar_focus.agent_cursor.clone() else {
            return;
        };
        let targets = self.sidebar_agent_targets();
        let Some(target) = targets.iter().position(|target| target == &cursor) else {
            return;
        };
        let heights = targets
            .iter()
            .map(|target| {
                self.endpoints
                    .iter()
                    .find(|endpoint| endpoint.endpoint_id == target.endpoint_id)
                    .and_then(|endpoint| endpoint.snapshot.as_deref())
                    .or(self.snapshot.as_deref())
                    .and_then(|snapshot| {
                        super::agent_sidebar::agent_row(
                            snapshot,
                            &target.pane_id,
                            &self.config,
                            None,
                        )
                    })
                    .map_or(1, |row| row.rows.len().clamp(1, u16::MAX as usize) as u16)
            })
            .collect::<Vec<_>>();
        let mut gaps = vec![self.config.agents.row_gap; targets.len()];
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

    fn close_sidebar_agent(&mut self, outcome: &mut ClientShellInput) {
        let Some(cursor) = self.local_sidebar_agent_cursor() else {
            return;
        };
        let targets = self.sidebar_agent_targets();
        let next = targets
            .iter()
            .position(|target| target == &cursor)
            .and_then(|index| {
                targets
                    .get(index + 1)
                    .or_else(|| index.checked_sub(1).and_then(|prev| targets.get(prev)))
            })
            .cloned();
        self.push_endpoint_method(
            crate::api::schema::Method::PaneClose(crate::api::schema::PaneTarget {
                pane_id: cursor.pane_id,
            }),
            outcome,
        );
        self.sidebar_focus.agent_cursor = next;
        outcome.repaint = true;
    }

    fn toggle_sidebar_group(&mut self, outcome: &mut ClientShellInput) {
        if self.sidebar_focus.section != SidebarSection::Spaces {
            return;
        }
        let Some(target) = self
            .navigate_workspace_id
            .clone()
            .filter(|target| self.navigation_target_valid(target))
        else {
            return;
        };
        let Some(snapshot) = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == target.endpoint_id)
            .and_then(|endpoint| endpoint.snapshot.as_deref())
        else {
            return;
        };
        let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == target.workspace_id)
        else {
            return;
        };
        let Some(worktree) = workspace.worktree.as_ref() else {
            return;
        };
        let key = worktree.key.clone();
        let grouped = snapshot
            .workspaces
            .iter()
            .filter_map(|workspace| workspace.worktree.as_ref())
            .any(|candidate| candidate.key == key && candidate.is_linked_worktree);
        if !grouped {
            return;
        }
        // Collapsing a group from one of its linked worktrees moves the cursor
        // to the group's root so it stays on a visible row.
        let root = worktree.is_linked_worktree.then(|| {
            snapshot
                .workspaces
                .iter()
                .find(|candidate| {
                    candidate.worktree.as_ref().is_some_and(|candidate| {
                        candidate.key == key && !candidate.is_linked_worktree
                    })
                })
                .map(|candidate| candidate.workspace_id.clone())
        });
        let collapsing = !self.group_is_collapsed(&target.endpoint_id, &key);
        self.toggle_collapsed_group(&target.endpoint_id, key);
        if collapsing {
            if let Some(Some(root)) = root {
                self.navigate_workspace_id = self.navigation_target(&target.endpoint_id, &root);
                self.preview_navigate_workspace(outcome);
            }
        }
        self.persist_chrome_preferences(outcome);
        outcome.repaint = true;
    }

    fn move_sidebar_workspace(&mut self, up: bool, outcome: &mut ClientShellInput) {
        if self.sidebar_focus.section != SidebarSection::Spaces {
            return;
        }
        let Some(target) = self.navigate_workspace_id.clone().filter(|target| {
            target.endpoint_id == self.active_endpoint_id && self.navigation_target_valid(target)
        }) else {
            return;
        };
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let roots = snapshot
            .workspaces
            .iter()
            .filter(|workspace| {
                !workspace
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| worktree.is_linked_worktree)
            })
            .map(|workspace| workspace.workspace_id.as_str())
            .collect::<Vec<_>>();
        let Some(position) = roots.iter().position(|id| *id == target.workspace_id) else {
            return;
        };
        let before = if up {
            let Some(previous) = position.checked_sub(1) else {
                return;
            };
            Some(roots[previous].to_owned())
        } else {
            if position + 1 >= roots.len() {
                return;
            }
            roots.get(position + 2).map(|id| (*id).to_owned())
        };
        if let Some(method) = self.workspace_move_method(&target.workspace_id, before.as_deref()) {
            self.push_endpoint_method(method, outcome);
        }
    }

    fn adjust_sidebar_width(&mut self, wider: bool, outcome: &mut ClientShellInput) {
        let (min, max) = crate::config::validated_sidebar_bounds(
            self.config.sidebar_min_width,
            self.config.sidebar_max_width,
        )
        .unwrap_or((18, 36));
        let width = if wider {
            self.sidebar_width.saturating_add(SIDEBAR_WIDTH_STEP)
        } else {
            self.sidebar_width.saturating_sub(SIDEBAR_WIDTH_STEP)
        }
        .clamp(min, max);
        if width != self.sidebar_width {
            self.sidebar_width = width;
            self.sidebar_width_manual = true;
            self.invalidate_pane_surface();
            outcome.resize = true;
            self.persist_chrome_preferences(outcome);
        }
    }

    fn adjust_sidebar_split(&mut self, delta: f32, outcome: &mut ClientShellInput) {
        let ratio = (self.sidebar_section_split + delta).clamp(0.1, 0.9);
        if (self.sidebar_section_split - ratio).abs() > f32::EPSILON {
            self.sidebar_section_split = ratio;
            self.sidebar_section_split_manual = true;
            self.persist_chrome_preferences(outcome);
        }
    }

    pub(super) fn toggle_agent_panel_sort(&mut self, outcome: &mut ClientShellInput) {
        self.config.agent_panel_sort = match self.config.agent_panel_sort {
            crate::config::AgentPanelSortConfig::Spaces => {
                crate::config::AgentPanelSortConfig::Priority
            }
            crate::config::AgentPanelSortConfig::Priority => {
                crate::config::AgentPanelSortConfig::Spaces
            }
        };
        self.agent_panel_sort_manual = true;
        self.agent_scroll = 0;
        self.persist_chrome_preferences(outcome);
        outcome.repaint = true;
    }
}

impl ClientShellState {
    /// The key after `search` (`prefix+s`): `a` picks an agent, `s` picks a space.
    pub(super) fn route_prefix_sequence_key(
        &mut self,
        sequence: ClientPrefixSequence,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        if !modifiers.is_empty() {
            return;
        }
        match (sequence, code) {
            (ClientPrefixSequence::Search, KeyCode::Char('a')) => {
                self.open_navigator_picker(ClientNavigatorScope::Agents);
            }
            (ClientPrefixSequence::Search, KeyCode::Char('s')) => {
                self.open_navigator_picker(ClientNavigatorScope::Spaces);
            }
            _ => {}
        }
        outcome.repaint = true;
    }

    /// A telescope-style picker: the session navigator limited to one kind of
    /// row, with the search field focused.
    pub(super) fn open_navigator_picker(&mut self, scope: ClientNavigatorScope) {
        self.open_navigator_overlay();
        if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
            navigator.scope = scope;
            navigator.search_focused = true;
            let rows =
                render::client_navigator_rows(&self.endpoints, &self.active_endpoint_id, navigator);
            let current = match scope {
                ClientNavigatorScope::Spaces => {
                    let focused = self
                        .snapshot
                        .as_deref()
                        .and_then(|snapshot| snapshot.focused_workspace_id.clone());
                    rows.iter().find(|row| {
                        matches!(
                            &row.target,
                            ClientNavigatorTarget::Workspace { endpoint_id, workspace_id }
                                if endpoint_id == &self.active_endpoint_id
                                    && Some(workspace_id) == focused.as_ref()
                        )
                    })
                }
                _ => rows.iter().find(|row| row.current),
            }
            .or_else(|| rows.first())
            .map(|row| row.target.clone());
            navigator.selected = current;
        }
    }
}

impl ClientShellState {
    pub(super) fn mode_bar_context(&self) -> render::ModeBarContext {
        render::ModeBarContext {
            sidebar_section: self.sidebar_focus.section,
            sidebar_editing: self.sidebar_focus.editing,
            sidebar_pending_close: self.sidebar_focus.pending_close(),
            prefix_sequence: self.prefix_sequence,
        }
    }
}
