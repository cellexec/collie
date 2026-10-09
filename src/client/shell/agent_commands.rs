//! `n` in the focused agents section: start a configured agent command in a new tab.

use super::*;
use crossterm::event::KeyModifiers;

impl ClientShellState {
    pub(super) fn open_agent_commands(&mut self, outcome: &mut ClientShellInput) {
        let Some(workspace_id) = self.workspace_action_id() else {
            return;
        };
        if self.config.agent_commands.is_empty() {
            self.record_binding(
                crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewTab),
                outcome,
            );
            return;
        }
        let labels = self
            .config
            .agent_commands
            .iter()
            .map(|entry| {
                let name = entry.name.trim();
                if name.is_empty() {
                    entry.command.trim().to_owned()
                } else {
                    name.to_owned()
                }
            })
            .collect();
        self.overlay = Some(ClientShellOverlay::AgentCommands(
            ClientAgentCommandsOverlay {
                workspace_id,
                highlighted: 0,
                labels,
            },
        ));
        outcome.repaint = true;
    }

    pub(super) fn route_agent_commands_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(ClientShellOverlay::AgentCommands(picker)) = self.overlay.as_mut() else {
            return false;
        };
        let count = picker.labels.len();
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        outcome.repaint = true;
        match code {
            KeyCode::Esc => self.overlay = None,
            KeyCode::Up | KeyCode::Char('k') => {
                picker.highlighted = picker.highlighted.saturating_sub(1);
            }
            KeyCode::Char('p') if modifiers == KeyModifiers::CONTROL => {
                picker.highlighted = picker.highlighted.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                picker.highlighted = (picker.highlighted + 1).min(count.saturating_sub(1));
            }
            KeyCode::Char('n') if modifiers == KeyModifiers::CONTROL => {
                picker.highlighted = (picker.highlighted + 1).min(count.saturating_sub(1));
            }
            KeyCode::Enter => {
                let index = picker.highlighted;
                self.start_agent_command(index, outcome);
            }
            KeyCode::Char(digit @ '1'..='9') if modifiers.is_empty() => {
                let index = usize::from(digit as u8 - b'1');
                if index < count {
                    self.start_agent_command(index, outcome);
                }
            }
            _ => {}
        }
        true
    }

    fn start_agent_command(&mut self, index: usize, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::AgentCommands(picker)) = self.overlay.take() else {
            return;
        };
        let Some(entry) = self.config.agent_commands.get(index).cloned() else {
            return;
        };
        let label = (!entry.name.trim().is_empty()).then(|| entry.name.trim().to_owned());
        let sent = self.push_endpoint_method_with_kind(
            crate::api::schema::Method::TabCreate(crate::api::schema::TabCreateParams {
                workspace_id: Some(picker.workspace_id),
                cwd: None,
                focus: true,
                label,
                env: Default::default(),
            }),
            PendingEndpointKind::AgentCommandTab {
                command: entry.command,
            },
            outcome,
        );
        if sent && self.mode == ClientShellMode::Navigate {
            self.mode = self.copy_or_terminal_mode();
            self.navigate_workspace_id = None;
            self.sync_sidebar_drawer(outcome);
        }
        outcome.repaint = true;
    }

    /// Type the picked command into the new tab's shell.
    pub(super) fn complete_agent_command_tab(
        &mut self,
        command: String,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let Ok(crate::api::schema::ResponseResult::TabCreated { root_pane, .. }) = result else {
            return (true, Vec::new());
        };
        let command = command.trim();
        if command.is_empty() {
            return (false, Vec::new());
        }
        let mut outcome = ClientShellInput::default();
        self.push_endpoint_method(
            crate::api::schema::Method::PaneSendInput(crate::api::schema::PaneSendInputParams {
                pane_id: root_pane.pane_id,
                text: command.to_owned(),
                keys: vec!["Enter".to_owned()],
            }),
            &mut outcome,
        );
        (outcome.repaint, outcome.actions)
    }
}
