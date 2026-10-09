use super::*;
use crate::api::schema::Method;

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
