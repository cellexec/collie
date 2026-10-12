use super::*;
use crate::api::schema::Method;
use crossterm::event::{MouseButton, MouseEventKind};

/// Two workspaces with one agent each: `claude` (blocked) in ws_1, `codex`
/// (working) in ws_2. ws_1 / pane_1 is focused.
fn agents_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    let mut second = projected.workspaces[0].clone();
    second.workspace_id = "ws_2".into();
    second.number = 2;
    second.label = "second".into();
    second.focused = false;
    projected.workspaces.push(second);
    let mut tab = projected.tabs[0].clone();
    tab.tab_id = "tab_2".into();
    tab.workspace_id = "ws_2".into();
    tab.focused = false;
    projected.tabs.push(tab);
    let mut pane = projected.panes[0].clone();
    pane.pane_id = "pane_2".into();
    pane.workspace_id = "ws_2".into();
    pane.tab_id = "tab_2".into();
    pane.focused = false;
    projected.panes.push(pane);
    let agent =
        |pane_id: &str, workspace_id: &str, tab_id: &str, name: &str, status| ClientShellAgent {
            pane_id: pane_id.into(),
            workspace_id: workspace_id.into(),
            tab_id: tab_id.into(),
            name: Some(name.into()),
            display_agent: None,
            agent: Some(name.into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: status,
            state_change_seq: 1,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: pane_id == "pane_1",
        };
    projected.agents = vec![
        agent("pane_1", "ws_1", "tab_1", "claude", AgentStatus::Blocked),
        agent("pane_2", "ws_2", "tab_2", "codex", AgentStatus::Working),
    ];
    projected
}

fn sidebar_state() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(agents_snapshot()));
    state.set_pane_surface(surface());
    state.compose(100, 28).expect("frame");
    state
}

fn press(state: &mut ClientShellState, bytes: &[u8]) -> ClientShellInput {
    state.handle_input_bytes(bytes)
}

fn focus_sidebar(state: &mut ClientShellState) {
    press(state, &[0x02]);
    press(state, b"w");
    assert_eq!(state.mode, ClientShellMode::Navigate);
}

fn methods(outcome: &ClientShellInput) -> Vec<&Method> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => Some(&request.method),
            _ => None,
        })
        .collect()
}

fn agent_cursor(state: &ClientShellState) -> Option<&str> {
    state
        .sidebar_focus
        .agent_cursor
        .as_ref()
        .map(|cursor| cursor.pane_id.as_str())
}

#[test]
fn j_and_k_move_the_spaces_cursor_with_a_live_preview() {
    let mut state = sidebar_state();
    focus_sidebar(&mut state);

    let down = press(&mut state, b"j");
    assert!(matches!(
        methods(&down).as_slice(),
        [Method::WorkspaceFocus(target)] if target.workspace_id == "ws_2"
    ));
    assert_eq!(state.mode, ClientShellMode::Navigate);

    let up = press(&mut state, b"k");
    // The snapshot still shows ws_1 focused, so returning needs no request.
    assert!(methods(&up).is_empty());
    assert_eq!(
        state.navigate_workspace_id,
        state.navigation_target(&ClientEndpointId::Local, "ws_1")
    );
}

#[test]
fn esc_restores_the_pane_that_had_focus_before_the_sidebar() {
    let mut state = sidebar_state();
    focus_sidebar(&mut state);
    press(&mut state, b"j");
    let mut moved = agents_snapshot();
    moved.focused_workspace_id = Some("ws_2".into());
    moved.focused_tab_id = Some("tab_2".into());
    moved.focused_pane_id = Some("pane_2".into());
    moved.revision = 2;
    state.set_snapshot(Box::new(moved));

    let esc = press(&mut state, b"\x1b");

    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(matches!(
        methods(&esc).as_slice(),
        [Method::PaneFocus(target)] if target.pane_id == "pane_1"
    ));
}

#[test]
fn agents_section_moves_over_agents_and_enter_returns_to_the_panes() {
    let mut state = sidebar_state();
    focus_sidebar(&mut state);

    press(&mut state, b"2");
    assert_eq!(
        state.sidebar_focus.section,
        crate::client::shell::sidebar_focus::SidebarSection::Agents
    );
    assert_eq!(agent_cursor(&state), Some("pane_1"));

    let down = press(&mut state, b"j");
    assert_eq!(agent_cursor(&state), Some("pane_2"));
    assert!(matches!(
        methods(&down).as_slice(),
        [Method::PaneFocus(target)] if target.pane_id == "pane_2"
    ));

    press(&mut state, b"\r");
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn state_filter_cycles_and_esc_clears_it_before_leaving() {
    let mut state = sidebar_state();
    focus_sidebar(&mut state);

    press(&mut state, b"f");
    assert_eq!(state.sidebar_focus.agent_status, Some(AgentStatus::Blocked));
    assert_eq!(state.sidebar_agent_targets().len(), 1);
    press(&mut state, b"f");
    assert_eq!(state.sidebar_focus.agent_status, Some(AgentStatus::Working));
    assert_eq!(agent_cursor(&state), Some("pane_2"));

    press(&mut state, b"\x1b");
    assert_eq!(state.sidebar_focus.agent_status, None);
    assert_eq!(state.mode, ClientShellMode::Navigate);
    press(&mut state, b"\x1b");
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn slash_filters_the_focused_section_inline() {
    let mut state = sidebar_state();
    focus_sidebar(&mut state);

    press(&mut state, b"/");
    assert!(state.sidebar_focus.editing);
    press(&mut state, b"sec");
    assert_eq!(state.sidebar_focus.workspace_query.as_str(), "sec");
    assert_eq!(
        state.navigate_workspace_id,
        state.navigation_target(&ClientEndpointId::Local, "ws_2")
    );
    let frame = state.compose(100, 28).expect("frame");
    let text = frame
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(text.contains("/sec"));
    let visible = state
        .hits
        .workspaces
        .iter()
        .map(|hit| hit.workspace_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(visible, ["ws_2"]);

    press(&mut state, b"\r");
    assert!(!state.sidebar_focus.editing);
    assert_eq!(state.sidebar_focus.workspace_query.as_str(), "sec");
    press(&mut state, b"\x1b");
    assert!(state.sidebar_focus.workspace_query.is_empty());
    assert_eq!(state.mode, ClientShellMode::Navigate);
}

#[test]
fn closing_an_agent_needs_the_close_key_twice() {
    let mut state = sidebar_state();
    focus_sidebar(&mut state);
    press(&mut state, b"2");

    let first = press(&mut state, b"d");
    assert!(methods(&first).is_empty());
    let second = press(&mut state, b"d");
    assert!(matches!(
        methods(&second).as_slice(),
        [Method::PaneClose(target)] if target.pane_id == "pane_1"
    ));
}

#[test]
fn leader_runs_prefix_bindings_and_returns_to_the_sidebar() {
    let mut state = sidebar_state();
    focus_sidebar(&mut state);

    press(&mut state, b" ");
    assert_eq!(state.mode, ClientShellMode::Prefix);
    let split = press(&mut state, b"v");
    assert!(matches!(methods(&split).as_slice(), [Method::PaneSplit(_)]));
    assert_eq!(state.mode, ClientShellMode::Navigate);
}

#[test]
fn search_sequence_opens_scoped_pickers() {
    let mut state = sidebar_state();

    press(&mut state, &[0x02]);
    press(&mut state, b"s");
    assert_eq!(state.prefix_sequence, Some(ClientPrefixSequence::Search));
    let frame = state.compose(100, 28).expect("frame");
    let text = frame
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(text.contains("agents"));
    press(&mut state, b"a");
    let Some(ClientShellOverlay::Navigator(picker)) = state.overlay.as_ref() else {
        panic!("agent picker should open");
    };
    assert_eq!(picker.scope, ClientNavigatorScope::Agents);
    assert!(picker.search_focused);
    let rows = render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, picker);
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|row| matches!(row.target, ClientNavigatorTarget::Pane { .. })));

    press(&mut state, b"\x1b");
    assert!(state.overlay.is_none());
    press(&mut state, &[0x02]);
    press(&mut state, b"s");
    press(&mut state, b"s");
    let Some(ClientShellOverlay::Navigator(picker)) = state.overlay.as_ref() else {
        panic!("space picker should open");
    };
    assert_eq!(picker.scope, ClientNavigatorScope::Spaces);
    let rows = render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, picker);
    assert!(rows
        .iter()
        .all(|row| matches!(row.target, ClientNavigatorTarget::Workspace { .. })));
}

#[test]
fn settings_moved_to_prefix_comma() {
    let mut state = sidebar_state();
    press(&mut state, &[0x02]);
    press(&mut state, b",");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(_))
    ));
}

#[test]
fn agent_commands_start_in_a_new_tab_and_type_the_command() {
    let mut config = Config::default();
    config.ui.sidebar.agent_commands = vec![crate::config::AgentCommandConfig {
        name: "claude".into(),
        command: "claude --resume".into(),
    }];
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(agents_snapshot()));
    state.set_pane_surface(surface());
    state.compose(100, 28).expect("frame");
    focus_sidebar(&mut state);
    press(&mut state, b"2");

    press(&mut state, b"n");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::AgentCommands(_))
    ));
    let start = press(&mut state, b"\r");
    let [ClientShellAction::Endpoint { request, .. }] = &start.actions[..] else {
        panic!("agent command should create a tab");
    };
    assert!(matches!(
        &request.method,
        Method::TabCreate(params)
            if params.workspace_id.as_deref() == Some("ws_1") && params.label.as_deref() == Some("claude")
    ));
    assert_eq!(state.mode, ClientShellMode::Terminal);

    let root_pane = serde_json::from_value(serde_json::json!({
        "pane_id": "pane_9",
        "terminal_id": "term_9",
        "workspace_id": "ws_1",
        "tab_id": "tab_9",
        "focused": true,
        "agent_status": "unknown",
        "revision": 1,
    }))
    .expect("pane info");
    let tab = serde_json::from_value(serde_json::json!({
        "tab_id": "tab_9",
        "workspace_id": "ws_1",
        "number": 2,
        "label": "claude",
        "focused": true,
        "pane_count": 1,
        "agent_status": "unknown",
    }))
    .expect("tab info");
    let request_id = request.id.clone();
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::TabCreated { tab, root_pane }),
    );
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("the command should be typed into the new pane");
    };
    assert!(matches!(
        &request.method,
        Method::PaneSendInput(params)
            if params.pane_id == "pane_9" && params.text == "claude --resume" && params.keys == ["Enter"]
    ));
}

#[test]
fn reentering_through_the_leader_keeps_the_restore_point() {
    let mut state = sidebar_state();
    focus_sidebar(&mut state);
    press(&mut state, b"j");
    let mut moved = agents_snapshot();
    moved.focused_workspace_id = Some("ws_2".into());
    moved.focused_tab_id = Some("tab_2".into());
    moved.focused_pane_id = Some("pane_2".into());
    moved.revision = 2;
    state.set_snapshot(Box::new(moved));

    press(&mut state, b" ");
    press(&mut state, b"w");
    assert_eq!(state.mode, ClientShellMode::Navigate);
    assert_eq!(
        state.navigate_workspace_id,
        state.navigation_target(&ClientEndpointId::Local, "ws_2")
    );
    let esc = press(&mut state, b"\x1b");
    assert!(matches!(
        methods(&esc).as_slice(),
        [Method::PaneFocus(target)] if target.pane_id == "pane_1"
    ));
}

#[test]
fn opening_a_filtered_space_clears_the_filter() {
    let mut state = sidebar_state();
    focus_sidebar(&mut state);
    press(&mut state, b"/");
    press(&mut state, b"sec");
    press(&mut state, b"\r");

    press(&mut state, b"\r");
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(state.sidebar_focus.workspace_query.is_empty());
}

#[test]
fn new_space_returns_focus_to_the_panes() {
    let mut state = sidebar_state();
    focus_sidebar(&mut state);
    let outcome = press(&mut state, b"n");
    assert!(matches!(
        methods(&outcome).as_slice(),
        [Method::WorkspaceCreate(params)] if params.focus
    ));
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn panes_sit_behind_a_backdrop_while_the_sidebar_has_focus() {
    let mut state = sidebar_state();
    let layout = state.layout(100, 28);
    let pane_cell = usize::from(layout.pane_surface.y) * 100 + usize::from(layout.pane_surface.x);
    let before = state.compose(100, 28).expect("frame").cells[pane_cell].clone();

    focus_sidebar(&mut state);
    let frame = state.compose(100, 28).expect("frame");
    let after = &frame.cells[pane_cell];
    assert_eq!(after.symbol, before.symbol);
    assert_ne!(after.bg, before.bg);
    assert_eq!(after.bg >> 24, 0x02, "backdrop colors are RGB");
    let channels = |packed: u32| [(packed >> 16) & 0xFF, (packed >> 8) & 0xFF, packed & 0xFF];
    assert!(channels(after.fg)
        .iter()
        .all(|channel| *channel <= 255 * 45 / 100));
    assert!(frame.cursor.is_none());
}

fn mouse(
    state: &mut ClientShellState,
    kind: crossterm::event::MouseEventKind,
    (column, row): (u16, u16),
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

fn workspace_point(state: &ClientShellState, workspace_id: &str) -> (u16, u16) {
    let hit = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
        .expect("workspace row");
    (hit.rect.x + 2, hit.rect.y)
}

fn agent_point(state: &ClientShellState, pane_id: &str) -> (u16, u16) {
    let (rect, _) = state
        .hits
        .agents
        .iter()
        .find(|(_, id)| id == pane_id)
        .expect("agent row");
    (rect.x + 2, rect.y)
}

#[test]
fn hovering_rows_moves_the_cursor_and_section_while_focused() {
    use crossterm::event::MouseEventKind;
    let mut state = sidebar_state();
    let point = workspace_point(&state, "ws_2");
    let unfocused = mouse(&mut state, MouseEventKind::Moved, point);
    assert!(methods(&unfocused).is_empty());

    focus_sidebar(&mut state);
    state.compose(100, 28).expect("frame");
    let hover = mouse(&mut state, MouseEventKind::Moved, point);
    assert_eq!(
        state.navigate_workspace_id,
        state.navigation_target(&ClientEndpointId::Local, "ws_2")
    );
    assert!(matches!(
        methods(&hover).as_slice(),
        [Method::WorkspaceFocus(target)] if target.workspace_id == "ws_2"
    ));

    let point = agent_point(&state, "pane_2");
    let hover = mouse(&mut state, MouseEventKind::Moved, point);
    assert_eq!(
        state.sidebar_focus.section,
        crate::client::shell::sidebar_focus::SidebarSection::Agents
    );
    assert_eq!(agent_cursor(&state), Some("pane_2"));
    assert!(matches!(
        methods(&hover).as_slice(),
        [Method::PaneFocus(target)] if target.pane_id == "pane_2"
    ));
    assert_eq!(state.mode, ClientShellMode::Navigate);
}

#[test]
fn clicking_a_row_opens_it_and_returns_focus_to_the_panes() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let mut state = sidebar_state();
    focus_sidebar(&mut state);
    press(&mut state, b"/");
    press(&mut state, b"sec");
    press(&mut state, b"\r");
    state.compose(100, 28).expect("frame");
    let point = workspace_point(&state, "ws_2");

    mouse(&mut state, MouseEventKind::Down(MouseButton::Left), point);
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(state.sidebar_focus.workspace_query.is_empty());
    let up = mouse(&mut state, MouseEventKind::Up(MouseButton::Left), point);
    assert!(matches!(
        methods(&up).as_slice(),
        [Method::WorkspaceFocus(target)] if target.workspace_id == "ws_2"
    ));
}

fn agents_only_state_with(snapshot: ClientShellSnapshot) -> (ClientShellState, Vec<String>) {
    let mut config = Config::default();
    config.ui.sidebar.agents_only = true;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());
    let frame = state.compose(100, 28).expect("frame");
    (state, frame_rows(&frame))
}

/// ws_1 wartet (`claude`, blocked), ws_2 arbeitet (`codex`).
fn agents_only_state() -> (ClientShellState, Vec<String>) {
    agents_only_state_with(agents_snapshot())
}

fn hit_panes(state: &ClientShellState) -> Vec<&str> {
    state
        .hits
        .agents
        .iter()
        .map(|(_, pane_id)| pane_id.as_str())
        .collect()
}

fn home() -> String {
    crate::worktree::expand_tilde_path("~")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn agents_only_sidebar_lists_waiting_chats_without_spaces() {
    let (state, rows) = agents_only_state();
    assert!(state.hits.workspaces.is_empty());
    assert_eq!(hit_panes(&state), ["pane_1"]);
    assert!(rows[0].contains("┌ chats "), "{}", rows[0]);
    assert!(rows.iter().all(|row| !row.contains("spaces")));
    assert!(rows.iter().all(|row| !row.contains("codex")));
    let footer = &rows[27];
    assert!(
        footer.contains(" new ") && footer.contains(" menu "),
        "{footer}"
    );
}

#[test]
fn agents_only_sidebar_keeps_focus_on_agents() {
    let (mut state, _) = agents_only_state();
    focus_sidebar(&mut state);
    let agents = crate::client::shell::sidebar_focus::SidebarSection::Agents;
    assert_eq!(state.sidebar_focus.section, agents);
    press(&mut state, b"1");
    assert_eq!(state.sidebar_focus.section, agents);
}

#[test]
fn h_toggles_between_waiting_and_hidden_chats() {
    let (mut state, _) = agents_only_state();
    focus_sidebar(&mut state);
    assert_eq!(agent_cursor(&state), Some("pane_1"));

    press(&mut state, b"h");
    let rows = frame_rows(&state.compose(100, 28).expect("frame"));
    assert!(rows[0].contains("┌ hidden "), "{}", rows[0]);
    assert_eq!(hit_panes(&state), ["pane_2"]);
    assert_eq!(agent_cursor(&state), Some("pane_2"));

    press(&mut state, b"h");
    state.compose(100, 28).expect("frame");
    assert_eq!(hit_panes(&state), ["pane_1"]);
}

#[test]
fn snoozed_chats_move_to_the_hidden_view() {
    let mut snapshot = agents_snapshot();
    snapshot.agents[1].agent_status = AgentStatus::Idle;
    snapshot.workspaces[1].snoozed_until = Some(crate::workspace::unix_millis_now() + 3_600_000);
    let (mut state, _) = agents_only_state_with(snapshot);
    assert_eq!(hit_panes(&state), ["pane_1"]);

    focus_sidebar(&mut state);
    press(&mut state, b"h");
    let rows = frame_rows(&state.compose(100, 28).expect("frame"));
    assert_eq!(hit_panes(&state), ["pane_2"]);
    assert!(rows
        .iter()
        .any(|row| row.contains("second") && row.contains(" 1h")));
}

#[test]
fn chats_sort_by_waiting_time_and_s_flips_the_order() {
    let mut snapshot = agents_snapshot();
    snapshot.agents[1].agent_status = AgentStatus::Done;
    snapshot.workspaces[0].attention_since = Some(2_000);
    snapshot.workspaces[1].attention_since = Some(1_000);
    let (mut state, rows) = agents_only_state_with(snapshot);
    assert_eq!(hit_panes(&state), ["pane_2", "pane_1"]);
    assert!(rows[0].contains(" oldest "), "{}", rows[0]);

    focus_sidebar(&mut state);
    press(&mut state, b"s");
    let rows = frame_rows(&state.compose(100, 28).expect("frame"));
    assert_eq!(hit_panes(&state), ["pane_1", "pane_2"]);
    assert!(rows[0].contains(" newest "), "{}", rows[0]);
}

#[test]
fn chat_without_agent_is_listed_by_its_space() {
    let mut snapshot = agents_snapshot();
    snapshot.agents.truncate(1);
    let (state, rows) = agents_only_state_with(snapshot);
    assert_eq!(hit_panes(&state), ["pane_1", "pane_2"]);
    assert!(rows.iter().any(|row| row.contains("second")));
}

#[test]
fn closing_a_chat_closes_its_space() {
    let (mut state, _) = agents_only_state();
    focus_sidebar(&mut state);
    press(&mut state, b"d");
    let outcome = press(&mut state, b"d");
    assert!(matches!(
        methods(&outcome).as_slice(),
        [Method::WorkspaceClose(params)] if params.workspace_id == "ws_1" && !params.close_group
    ));
}

#[test]
fn closing_a_working_chat_asks_first() {
    let (mut state, _) = agents_only_state();
    focus_sidebar(&mut state);
    press(&mut state, b"h");
    press(&mut state, b"d");
    let outcome = press(&mut state, b"d");
    assert!(methods(&outcome).is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ConfirmClose(_))
    ));
}

#[test]
fn agents_only_new_opens_a_chat_in_home() {
    let (mut state, _) = agents_only_state();
    focus_sidebar(&mut state);
    let outcome = press(&mut state, b"n");
    let home = home();
    assert!(matches!(
        methods(&outcome).as_slice(),
        [Method::WorkspaceCreate(params)]
            if params.focus && params.cwd.as_deref() == Some(home.as_str())
    ));
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn agents_only_new_button_opens_a_chat_in_home() {
    let (mut state, _) = agents_only_state();
    let new = state.hits.new_workspace;
    assert!(!new.is_empty());
    let down = mouse(
        &mut state,
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        (new.x, new.y),
    );
    let up = mouse(
        &mut state,
        crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
        (new.x, new.y),
    );
    let mut outcome = down;
    outcome.actions.extend(up.actions);
    assert!(matches!(
        methods(&outcome).as_slice(),
        [Method::WorkspaceCreate(params)] if params.cwd.is_some()
    ));
}

#[test]
fn chat_status_and_label_come_from_the_agent_in_the_first_tab() {
    let mut snapshot = agents_snapshot();
    let mut tab = snapshot.tabs[0].clone();
    tab.tab_id = "tab_3".into();
    tab.number = 2;
    snapshot.tabs.push(tab);
    let mut pane = snapshot.panes[0].clone();
    pane.pane_id = "pane_3".into();
    pane.tab_id = "tab_3".into();
    snapshot.panes.push(pane);
    let mut side_agent = snapshot.agents[1].clone();
    side_agent.pane_id = "pane_3".into();
    side_agent.workspace_id = "ws_1".into();
    side_agent.tab_id = "tab_3".into();
    snapshot.agents.push(side_agent);
    snapshot.agents[0].terminal_title_stripped = Some("fix login bug".into());

    let rows = crate::client::shell::chats::chat_rows(&snapshot);
    let chat = rows
        .iter()
        .find(|row| row.workspace_id == "ws_1")
        .expect("chat");
    assert_eq!(chat.pane_id, "pane_1");
    assert_eq!(chat.status, AgentStatus::Blocked);
    assert_eq!(chat.label, "fix login bug");

    snapshot.workspaces[0].custom_label = true;
    snapshot.workspaces[0].label = "login".into();
    let rows = crate::client::shell::chats::chat_rows(&snapshot);
    assert_eq!(rows[0].label, "login");
}

#[test]
fn z_snoozes_the_chat_under_the_cursor() {
    let (mut state, _) = agents_only_state();
    focus_sidebar(&mut state);
    press(&mut state, b"z");
    assert!(matches!(state.overlay, Some(ClientShellOverlay::Snooze(_))));

    let before = crate::workspace::unix_millis_now();
    let outcome = press(&mut state, b"1");
    assert!(state.overlay.is_none());
    assert!(matches!(
        methods(&outcome).as_slice(),
        [Method::WorkspaceSnooze(params)]
            if params.workspace_id == "ws_1"
                && params.until.is_some_and(|until| until >= before + 3_600_000)
    ));
}

#[test]
fn z_then_u_clears_a_snooze() {
    let mut snapshot = agents_snapshot();
    snapshot.workspaces[0].snoozed_until = Some(crate::workspace::unix_millis_now() + 60_000);
    let (mut state, _) = agents_only_state_with(snapshot);
    focus_sidebar(&mut state);
    press(&mut state, b"h");
    assert_eq!(agent_cursor(&state), Some("pane_1"));
    press(&mut state, b"z");
    let outcome = press(&mut state, b"u");
    assert!(matches!(
        methods(&outcome).as_slice(),
        [Method::WorkspaceSnooze(params)] if params.workspace_id == "ws_1" && params.until.is_none()
    ));
}

#[test]
fn hidden_view_ends_with_sidebar_focus() {
    let (mut state, _) = agents_only_state();
    focus_sidebar(&mut state);
    press(&mut state, b"h");
    press(&mut state, b"\x1b");
    state.compose(100, 28).expect("frame");
    assert_eq!(hit_panes(&state), ["pane_1"]);
    focus_sidebar(&mut state);
    assert!(!state.sidebar_focus.show_hidden);
}

fn chat_overlay_state_with(snapshot: ClientShellSnapshot) -> ClientShellState {
    let mut config = Config::default();
    config.ui.sidebar.agents_only = true;
    config.ui.sidebar.overlay = true;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());
    state.compose(100, 28).expect("frame");
    state
}

fn chat_overlay_state() -> ClientShellState {
    chat_overlay_state_with(agents_snapshot())
}

#[test]
fn chat_overlay_collapses_to_a_line_and_opens_without_resize() {
    let mut state = chat_overlay_state();
    let collapsed = state.surface_size(100, 28);
    assert_eq!(collapsed.cols, 99);
    assert_eq!(state.hits.sidebar_line, Rect::new(0, 0, 1, 28));
    assert!(state.hits.agents.is_empty());

    let prefix = press(&mut state, &[0x02]);
    let open = press(&mut state, b"w");
    assert!(!prefix.resize && !open.resize);
    assert_eq!(state.surface_size(100, 28), collapsed);
    let rows = frame_rows(&state.compose(100, 28).expect("frame"));
    assert_eq!(state.hits.chat_overlay, Rect::new(0, 0, 50, 28));
    assert_eq!(hit_panes(&state), ["pane_1", "pane_2"]);
    assert!(rows[0].contains("chats"), "{}", rows[0]);

    press(&mut state, &[0x02]);
    let close = press(&mut state, b"w");
    assert!(!close.resize);
    assert_eq!(state.mode, ClientShellMode::Terminal);
    state.compose(100, 28).expect("frame");
    assert_eq!(state.hits.chat_overlay, Rect::default());
}

#[test]
fn chat_overlay_moves_the_cursor_without_preview() {
    let mut snapshot = agents_snapshot();
    snapshot.agents[1].agent_status = AgentStatus::Done;
    snapshot.workspaces[0].attention_since = Some(1_000);
    snapshot.workspaces[1].attention_since = Some(2_000);
    let mut state = chat_overlay_state_with(snapshot);
    focus_sidebar(&mut state);
    assert_eq!(agent_cursor(&state), Some("pane_1"));

    let step = press(&mut state, b"j");
    assert_eq!(agent_cursor(&state), Some("pane_2"));
    assert!(methods(&step).is_empty());

    let commit = press(&mut state, b"\r");
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(matches!(
        methods(&commit).as_slice(),
        [Method::PaneFocus(target)] if target.pane_id == "pane_2"
    ));
}

#[test]
fn chat_overlay_esc_leaves_without_focus_changes() {
    let mut state = chat_overlay_state();
    focus_sidebar(&mut state);
    let leave = press(&mut state, b"\x1b");
    assert_eq!(state.mode, ClientShellMode::Terminal);
    assert!(methods(&leave).is_empty());
}

#[test]
fn chat_overlay_opens_on_line_click_and_closes_on_pane_click() {
    let mut state = chat_overlay_state();
    mouse(&mut state, MouseEventKind::Down(MouseButton::Left), (0, 5));
    assert_eq!(state.mode, ClientShellMode::Navigate);
    state.compose(100, 28).expect("frame");

    let card = state.hits.agents[0].0;
    mouse(&mut state, MouseEventKind::Moved, (card.x + 2, card.y));
    assert_eq!(agent_cursor(&state), Some("pane_1"));

    mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        (80, 10),
    );
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn chat_overlay_click_on_a_card_opens_the_chat() {
    let mut state = chat_overlay_state();
    focus_sidebar(&mut state);
    state.compose(100, 28).expect("frame");
    let card = state.hits.agents[0].0;
    mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        (card.x + 2, card.y),
    );
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn chat_overlay_opens_after_hover_and_closes_when_the_mouse_leaves() {
    let mut state = chat_overlay_state();
    mouse(&mut state, MouseEventKind::Moved, (0, 5));
    assert_eq!(state.mode, ClientShellMode::Terminal);
    let deadline = state.chat_line_hover_deadline.expect("hover deadline");

    let mut outcome = ClientShellInput::default();
    state.tick_chat_overlay(deadline, &mut outcome);
    assert_eq!(state.mode, ClientShellMode::Navigate);
    assert!(state.sidebar_focus.hover_opened);
    state.compose(100, 28).expect("frame");

    mouse(&mut state, MouseEventKind::Moved, (10, 5));
    assert_eq!(state.mode, ClientShellMode::Navigate);
    mouse(&mut state, MouseEventKind::Moved, (80, 5));
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn chat_overlay_stays_open_after_a_key_even_when_hover_opened_it() {
    let mut state = chat_overlay_state();
    mouse(&mut state, MouseEventKind::Moved, (0, 5));
    let deadline = state.chat_line_hover_deadline.expect("hover deadline");
    state.tick_chat_overlay(deadline, &mut ClientShellInput::default());
    state.compose(100, 28).expect("frame");

    press(&mut state, b"j");
    mouse(&mut state, MouseEventKind::Moved, (80, 5));
    assert_eq!(state.mode, ClientShellMode::Navigate);
}

#[test]
fn chat_overlay_width_changes_in_percent_steps() {
    let mut state = chat_overlay_state();
    focus_sidebar(&mut state);
    press(&mut state, b">");
    press(&mut state, b">");
    press(&mut state, b">");
    state.compose(200, 28).expect("frame");
    assert_eq!(state.chat_overlay_percent, 60);
    assert_eq!(state.hits.chat_overlay.width, 100);
    press(&mut state, b"<");
    state.compose(200, 28).expect("frame");
    assert_eq!(state.hits.chat_overlay.width, 100);
    assert_eq!(state.chat_overlay_percent, 55);
}

#[test]
fn chat_line_pulses_when_a_chat_newly_waits() {
    let mut state = chat_overlay_state();
    let now = std::time::Instant::now();
    state.tick_chat_overlay(now, &mut ClientShellInput::default());
    assert!(state.chat_pulse_started.is_none());

    let mut snapshot = agents_snapshot();
    snapshot.agents[1].agent_status = AgentStatus::Done;
    snapshot.workspaces[1].attention_since = Some(crate::workspace::unix_millis_now());
    state.set_snapshot(Box::new(snapshot));
    let mut outcome = ClientShellInput::default();
    state.tick_chat_overlay(now, &mut outcome);
    assert!(state.chat_pulse_started.is_some());
    assert!(outcome.repaint);
}

#[test]
fn chat_cards_show_context_and_the_blocked_reason() {
    let mut snapshot = agents_snapshot();
    snapshot.workspaces[0].git_ahead_behind = Some((2, 0));
    snapshot.workspaces[0].attention_since = Some(crate::workspace::unix_millis_now() - 5 * 60_000);
    let mut tab = snapshot.tabs[0].clone();
    tab.tab_id = "tab_1b".into();
    tab.number = 2;
    snapshot.tabs.push(tab);
    let mut state = chat_overlay_state_with(snapshot);
    focus_sidebar(&mut state);
    let rows = frame_rows(&state.compose(100, 28).expect("frame"));
    let card = state.hits.agents[0].0;
    assert_eq!(card.height, 3);
    let lines = &rows[usize::from(card.y)..usize::from(card.y) + 3];
    assert!(lines[0].contains("client-shell"), "{}", lines[0]);
    assert!(lines[0].contains(" 5m "), "{}", lines[0]);
    for part in ["claude", "repo", "main ↑2", "+1 shell"] {
        assert!(lines[1].contains(part), "{part}: {}", lines[1]);
    }
    assert!(lines[2].contains("wartet auf Eingabe"), "{}", lines[2]);
}

#[test]
fn chat_overlay_lists_running_chats_below_and_j_walks_into_them() {
    let mut state = chat_overlay_state();
    focus_sidebar(&mut state);
    let rows = frame_rows(&state.compose(100, 28).expect("frame"));
    assert_eq!(hit_panes(&state), ["pane_1", "pane_2"]);
    let running = state.hits.agents[1].0;
    assert_eq!(running.height, 1);
    assert!(rows[usize::from(running.y) - 1].contains("läuft 1  snooze 0"));
    assert!(rows[usize::from(running.y)].contains("second"));

    press(&mut state, b"j");
    assert_eq!(agent_cursor(&state), Some("pane_2"));
    press(&mut state, b"h");
    assert_eq!(agent_cursor(&state), Some("pane_1"));
    press(&mut state, b"h");
    assert_eq!(agent_cursor(&state), Some("pane_2"));
}

#[test]
fn chat_overlay_shows_nothing_to_do_when_no_chat_waits() {
    let mut snapshot = agents_snapshot();
    snapshot.agents[0].agent_status = AgentStatus::Working;
    let mut state = chat_overlay_state_with(snapshot);
    focus_sidebar(&mut state);
    let rows = frame_rows(&state.compose(100, 28).expect("frame"));
    assert!(rows.iter().any(|row| row.contains("Nichts zu tun")));
    assert!(rows.iter().any(|row| row.contains("läuft 2  snooze 0")));
}

#[test]
fn chat_cards_show_reported_messages_unread_state_and_runtime() {
    let now = crate::workspace::unix_millis_now();
    let mut snapshot = agents_snapshot();
    snapshot.workspaces[0].blocked_reason = Some("Freigabe: Bash".into());
    snapshot.workspaces[0].attention_seen = Some(false);
    snapshot.workspaces[1].working_since = Some(now - 3 * 60_000);
    let mut third = snapshot.workspaces[0].clone();
    third.workspace_id = "ws_3".into();
    third.label = "third".into();
    third.focused = false;
    third.attention_seen = Some(true);
    third.blocked_reason = None;
    third.last_message = Some("Soll ich committen?".into());
    snapshot.workspaces.push(third);
    let mut tab = snapshot.tabs[0].clone();
    tab.tab_id = "tab_3".into();
    tab.workspace_id = "ws_3".into();
    snapshot.tabs.push(tab);
    let mut pane = snapshot.panes[0].clone();
    pane.pane_id = "pane_3".into();
    pane.workspace_id = "ws_3".into();
    pane.tab_id = "tab_3".into();
    snapshot.panes.push(pane);
    let mut agent = snapshot.agents[0].clone();
    agent.pane_id = "pane_3".into();
    agent.workspace_id = "ws_3".into();
    agent.tab_id = "tab_3".into();
    agent.agent_status = AgentStatus::Done;
    snapshot.agents.push(agent);

    let mut state = chat_overlay_state_with(snapshot);
    focus_sidebar(&mut state);
    let rows = frame_rows(&state.compose(100, 28).expect("frame"));
    let card = |pane_id: &str| {
        let rect = state
            .hits
            .agents
            .iter()
            .find(|(_, id)| id == pane_id)
            .map(|(rect, _)| *rect)
            .expect("card");
        rows[usize::from(rect.y)..usize::from(rect.y + rect.height)].to_vec()
    };

    let blocked = card("pane_1");
    assert!(blocked[0].contains('●'), "{}", blocked[0]);
    assert!(blocked[2].contains("Freigabe: Bash"), "{}", blocked[2]);
    let done = card("pane_3");
    assert!(done[0].contains('○'), "{}", done[0]);
    assert!(done[2].contains("Soll ich committen?"), "{}", done[2]);
    let running = card("pane_2");
    assert!(running[0].contains(" 3m"), "{}", running[0]);
}

#[test]
fn chat_cards_show_stopped_and_awaiting_chats() {
    let mut snapshot = agents_snapshot();
    // ws_1: der Agent wartet auf ein Formular.
    snapshot.agents[0].agent_status = AgentStatus::Done;
    snapshot.workspaces[0].awaiting = Some("Formular: 3 Fragen".into());
    // ws_2: der Agent ist gespeichert, aber noch nicht gestartet. Der Server
    // kennt ihn schon mit Namen und altem Status.
    snapshot.agents[1].agent_status = AgentStatus::Idle;
    snapshot.workspaces[1].stopped_agent = Some("claude".into());
    snapshot.workspaces[1].last_message = Some("Tests laufen grün".into());

    let mut state = chat_overlay_state_with(snapshot);
    focus_sidebar(&mut state);
    let rows = frame_rows(&state.compose(100, 28).expect("frame"));
    let card = |pane_id: &str| {
        let rect = state
            .hits
            .agents
            .iter()
            .find(|(_, id)| id == pane_id)
            .map(|(rect, _)| *rect)
            .expect("card");
        rows[usize::from(rect.y)..usize::from(rect.y + rect.height)].to_vec()
    };

    let awaiting = card("pane_1");
    assert!(awaiting[0].contains('?'), "{}", awaiting[0]);
    assert!(
        awaiting[2].contains("wartet auf Antwort: Formular: 3 Fragen"),
        "{}",
        awaiting[2]
    );
    let stopped = card("pane_2");
    assert!(stopped[0].contains('■'), "{}", stopped[0]);
    assert!(stopped[1].contains("codex"), "{}", stopped[1]);
    assert!(
        stopped[2].contains("gestoppt: Tests laufen grün"),
        "{}",
        stopped[2]
    );
}

#[test]
fn x_marks_chats_and_p_prompts_all_marked_chats() {
    let mut snapshot = agents_snapshot();
    snapshot.agents[1].agent_status = AgentStatus::Idle;
    let mut state = chat_overlay_state_with(snapshot);
    focus_sidebar(&mut state);
    assert_eq!(agent_cursor(&state), Some("pane_1"));

    press(&mut state, b"x");
    assert_eq!(agent_cursor(&state), Some("pane_2"));
    press(&mut state, b"x");
    let rows = frame_rows(&state.compose(100, 28).expect("frame"));
    assert!(rows[0].contains("2 marked"), "{}", rows[0]);

    press(&mut state, b"p");
    let Some(ClientShellOverlay::Rename(prompt)) = state.overlay.as_ref() else {
        panic!("prompt overlay");
    };
    assert_eq!(
        prompt.input.as_str(),
        super::super::chat_marks::WRAP_UP_PROMPT
    );
    let outcome = press(&mut state, b"\r");
    assert!(matches!(
        methods(&outcome).as_slice(),
        [Method::ChatPrompt(params)]
            if params.pane_ids == ["pane_1", "pane_2"]
                && params.text == super::super::chat_marks::WRAP_UP_PROMPT
    ));
    assert!(state.sidebar_focus.marked.is_empty());
}

#[test]
fn esc_clears_marks_before_leaving_and_o_orchestrates_the_cursor_chat() {
    let mut state = chat_overlay_state();
    focus_sidebar(&mut state);
    press(&mut state, b"x");
    press(&mut state, b"\x1b");
    assert!(state.sidebar_focus.marked.is_empty());
    assert_eq!(state.mode, ClientShellMode::Navigate);

    press(&mut state, b"k");
    let outcome = press(&mut state, b"O");
    assert!(matches!(
        methods(&outcome).as_slice(),
        [Method::ChatOrchestrate(params)] if params.pane_ids == ["pane_1"]
    ));
    assert_ne!(state.mode, ClientShellMode::Navigate);
}
