//! collie: Methoden für mehrere Chats auf einmal, siehe
//! docs/specs/chat-workflow.md.

use bytes::Bytes;

use crate::api::schema::{
    ChatOrchestrateParams, ChatPromptParams, PaneSendInputParams, ResponseResult,
    WorkspaceCreateParams,
};
use crate::app::App;

use super::responses::{encode_error, encode_success};

/// Pause zwischen Text und Enter, wie bei `agent.prompt`.
const CHAT_PROMPT_SUBMIT_DELAY: std::time::Duration = std::time::Duration::from_millis(300);
/// Umgebungsvariable, über die der Orchestrator seinen Start-Prompt bekommt.
const ORCHESTRATOR_PROMPT_ENV: &str = "COLLIE_ORCHESTRATOR_PROMPT";
const ORCHESTRATOR_LABEL: &str = "Orchestrator";

/// Was der Orchestrator über einen Chat erfährt.
struct OrchestratedChat {
    pane_id: String,
    title: String,
    cwd: Option<String>,
    branch: Option<String>,
    status: String,
    last_message: Option<String>,
}

impl App {
    /// Schickt denselben Prompt an die Agents mehrerer Panes. Panes ohne
    /// laufendes Terminal werden übersprungen.
    pub(super) fn handle_chat_prompt(&mut self, id: String, params: ChatPromptParams) -> String {
        if params.text.trim().is_empty() {
            return encode_error(id, "empty_chat_prompt", "chat prompt must not be empty");
        }
        let mut sent = 0;
        for public_id in &params.pane_ids {
            let Some((ws_idx, pane_id)) = self.parse_pane_id(public_id) else {
                continue;
            };
            let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
                continue;
            };
            let (text, enter) =
                super::super::api_helpers::encode_api_submission_parts(runtime, &params.text);
            match runtime.queue_user_input_submission(
                Bytes::from(text),
                Bytes::from(enter),
                CHAT_PROMPT_SUBMIT_DELAY,
                None,
            ) {
                Ok(_) => sent += 1,
                Err(err) => {
                    tracing::warn!(pane = %public_id, err = %err, "chat prompt not submitted");
                }
            }
        }
        if sent == 0 {
            return encode_error(id, "pane_not_found", "no chat could receive the prompt");
        }
        encode_success(id, ResponseResult::Ok {})
    }

    /// Legt einen Space an und startet dort `claude` mit einem Prompt, der die
    /// Chats beschreibt und die CLI erklärt.
    pub(super) fn handle_chat_orchestrate(
        &mut self,
        id: String,
        params: ChatOrchestrateParams,
    ) -> String {
        let chats = params
            .pane_ids
            .iter()
            .filter_map(|pane_id| self.orchestrated_chat(pane_id))
            .collect::<Vec<_>>();
        if chats.is_empty() {
            return encode_error(id, "pane_not_found", "no chat to orchestrate");
        }
        let cwd = std::env::var("HOME").ok().filter(|home| !home.is_empty());
        let created = self.handle_workspace_create(
            id.clone(),
            WorkspaceCreateParams {
                source_workspace_id: None,
                cwd,
                focus: true,
                label: Some(ORCHESTRATOR_LABEL.to_owned()),
                env: [(
                    ORCHESTRATOR_PROMPT_ENV.to_owned(),
                    orchestrator_prompt(&chats),
                )]
                .into(),
            },
        );
        let pane_id = serde_json::from_str::<serde_json::Value>(&created)
            .ok()
            .and_then(|response| {
                response
                    .pointer("/result/root_pane/pane_id")?
                    .as_str()
                    .map(str::to_owned)
            });
        let Some(pane_id) = pane_id else {
            return created;
        };
        self.handle_pane_send_input(
            format!("{id}:start"),
            PaneSendInputParams {
                pane_id,
                text: format!("claude \"${ORCHESTRATOR_PROMPT_ENV}\""),
                keys: vec!["Enter".into()],
            },
        );
        created
    }

    fn orchestrated_chat(&self, public_id: &str) -> Option<OrchestratedChat> {
        let (ws_idx, pane_id) = self.parse_pane_id(public_id)?;
        let info = self.workspace_info(ws_idx);
        let ws = self.state.workspaces.get(ws_idx)?;
        let cwd = ws.chat.cwd.clone().or_else(|| {
            let terminal_id = ws.terminal_id(pane_id)?;
            let terminal = self.state.terminals.get(terminal_id)?;
            Some(terminal.cwd.display().to_string())
        });
        let status = if info.stopped_agent.is_some() {
            "gestoppt".to_owned()
        } else {
            serde_json::to_value(info.agent_status)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned())
        };
        Some(OrchestratedChat {
            pane_id: public_id.to_owned(),
            title: info.label,
            cwd,
            branch: ws.branch(),
            status,
            last_message: ws.last_message.clone(),
        })
    }
}

/// Start-Prompt des Orchestrators.
fn orchestrator_prompt(chats: &[OrchestratedChat]) -> String {
    let mut prompt = String::from(
        "Du bist der Orchestrator für die folgenden Chats in collie. Jeder Chat ist ein \
         Coding-Agent in einem eigenen Space. Die CLI von collie heißt `herdr`.\n\nChats:\n",
    );
    for chat in chats {
        prompt.push_str(&format!("- Pane `{}`: {}\n", chat.pane_id, chat.title));
        if let Some(cwd) = &chat.cwd {
            prompt.push_str(&format!("  Verzeichnis: {cwd}\n"));
        }
        if let Some(branch) = &chat.branch {
            prompt.push_str(&format!("  Branch: {branch}\n"));
        }
        prompt.push_str(&format!("  Status: {}\n", chat.status));
        if let Some(message) = &chat.last_message {
            prompt.push_str(&format!("  Letzte Nachricht: {message}\n"));
        }
    }
    prompt.push_str(
        "\nWerkzeuge:\n\
         - `herdr pane read <pane> --source recent --lines 120` liest, was ein Agent zuletzt geschrieben hat.\n\
         - `herdr agent prompt <pane> \"<text>\" --wait` schickt einem Agent eine Nachricht und wartet, bis er fertig ist.\n\
         - `herdr agent wait <pane>` wartet, bis ein Agent fertig ist oder blockiert.\n\
         - `herdr agent list` und `herdr workspace list` zeigen den Stand aller Agents und Spaces.\n\
         - `herdr workspace close <workspace>` schließt einen Chat.\n\
         - `herdr --skill` zeigt die vollständige Anleitung der CLI.\n\
         \nRegeln:\n\
         - Frag mich, bevor du Chats schließt, mergst, pushst oder etwas löschst.\n\
         - Koordiniere Agents, die an ähnlichen Dingen arbeiten, damit sie sich nicht in die Quere kommen.\n\
         \nLies zuerst jeden Chat und fasse kurz zusammen, woran er arbeitet und was offen ist. \
         Dann warte auf meine Anweisung.\n",
    );
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestrator_prompt_lists_every_chat_with_known_facts() {
        let prompt = orchestrator_prompt(&[
            OrchestratedChat {
                pane_id: "w1:p1".into(),
                title: "Snooze umbauen".into(),
                cwd: Some("/p/collie".into()),
                branch: Some("feat/snooze".into()),
                status: "done".into(),
                last_message: Some("Fertig.".into()),
            },
            OrchestratedChat {
                pane_id: "w2:p1".into(),
                title: "Tests".into(),
                cwd: None,
                branch: None,
                status: "gestoppt".into(),
                last_message: None,
            },
        ]);
        assert!(prompt.contains("Pane `w1:p1`: Snooze umbauen"));
        assert!(prompt.contains("Branch: feat/snooze"));
        assert!(prompt.contains("Letzte Nachricht: Fertig."));
        assert!(prompt.contains("Pane `w2:p1`: Tests\n  Status: gestoppt"));
        assert!(prompt.contains("herdr agent prompt"));
    }
}
