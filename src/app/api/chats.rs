//! collie: Methoden für mehrere Chats und für Master Chats, siehe
//! docs/specs/chat-workflow.md und docs/specs/master-chat.md.

use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::api::schema::{
    ChatAdoptParams, ChatCloseChildren, ChatCloseParams, ChatInboxEntryInfo, ChatInboxParams,
    ChatMasterCreateParams, ChatPromptParams, ChatReportParams, ChatSpawnParams, ChatTeamInfo,
    ChatTeamRole, ChatWorkspaceParams, PaneSendInputParams, ResponseResult, WorkspaceCloseParams,
    WorkspaceCreateParams,
};
use crate::app::App;
use crate::workspace::{ChatInboxEntry, ChatInboxKind, ChatRole};

use super::responses::{encode_error, encode_success};

/// Pause zwischen Text und Enter, wie bei `agent.prompt`.
const CHAT_PROMPT_SUBMIT_DELAY: Duration = Duration::from_millis(300);
/// Fester Name eines Master Chats.
pub(crate) const MASTER_LABEL: &str = "Master Chat";
/// Umgebungsvariable, über die ein neuer Agent seinen Start-Prompt bekommt.
const CHAT_PROMPT_ENV: &str = "COLLIE_CHAT_PROMPT";
/// So lange wartet die Inbox nach dem letzten Eintrag.
const INBOX_QUIET_MS: u64 = 5_000;
/// So lange wartet die Inbox, nachdem der Nutzer im Master getippt hat.
const INBOX_TYPING_MS: u64 = 5_000;
/// So oft prüft der Server eine wartende Inbox erneut.
const INBOX_RETRY: Duration = Duration::from_secs(1);
/// So lange nach dem Start beantwortet collie die Vertrauensfrage.
const TRUST_WATCH_MS: u64 = 120_000;
/// Text der Vertrauensfrage von Claude Code, an dem collie sie erkennt.
const TRUST_PROMPT_MARKERS: [&str; 2] = ["Yes, I trust this folder", "Do you trust the files"];
/// Höchstlänge eines Auftrags.
const MAX_ASSIGNMENT_CHARS: usize = 4_000;
/// Höchstlänge einer Report-Nachricht.
const MAX_REPORT_CHARS: usize = 2_000;
/// So viele Ebenen sucht der Server über dem Aufrufer nach einem Pane-Prozess.
const MAX_CALLER_ANCESTORS: usize = 32;

/// Start-Prompt eines Master Chats.
const MASTER_PROMPT: &str = "Du bist ab jetzt der Master Chat in collie. Lade zuerst den Skill \
collie-master und arbeite nach ihm. Du nimmst meine Aufgaben an, verteilst sie mit \
`herdr chat spawn` an Child-Chats, sammelst ihre Rückmeldungen aus deiner Inbox und legst mir \
gebündelt vor, was du nicht selbst entscheiden kannst. Bestätige kurz und warte dann auf meine \
erste Aufgabe.";

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
            if let Err(response) = self.guard_child_caller(&id, ws_idx) {
                return response;
            }
            if self.submit_to_pane(ws_idx, pane_id, &params.text) {
                sent += 1;
            }
        }
        if sent == 0 {
            return encode_error(id, "pane_not_found", "no chat could receive the prompt");
        }
        encode_success(id, ResponseResult::Ok {})
    }

    /// Tippt einen Prompt in ein Pane und drückt danach Enter.
    pub(super) fn submit_to_pane(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
        text: &str,
    ) -> bool {
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return false;
        };
        let (text, enter) = super::super::api_helpers::encode_api_submission_parts(runtime, text);
        match runtime.queue_user_input_submission(
            Bytes::from(text),
            Bytes::from(enter),
            CHAT_PROMPT_SUBMIT_DELAY,
            None,
        ) {
            Ok(_) => true,
            Err(err) => {
                tracing::warn!(err = %err, "chat prompt not submitted");
                false
            }
        }
    }

    // ---- Master Chat --------------------------------------------------------

    /// Legt einen Master Chat an und startet dort `claude`.
    pub(super) fn handle_chat_master_create(
        &mut self,
        id: String,
        params: ChatMasterCreateParams,
    ) -> String {
        if let Some(response) = self.reject_child_caller(&id) {
            return response;
        }
        let cwd = params
            .cwd
            .or_else(|| std::env::var("HOME").ok().filter(|home| !home.is_empty()));
        let created = self.create_chat_workspace(&id, cwd, params.focus, MASTER_LABEL, None);
        let (master_id, pane_id) = match created {
            Ok(created) => created,
            Err(response) => return response,
        };
        let Some(index) = self.parse_workspace_id(&master_id) else {
            return encode_error(id, "workspace_not_found", "master vanished");
        };
        self.state.workspaces[index].chat.team.role = ChatRole::Master;
        self.watch_trust_prompt(index);
        for workspace_id in params.adopt_workspace_ids {
            if let Some(child) = self.parse_workspace_id(&workspace_id) {
                self.adopt_child(index, child);
            }
        }
        self.type_chat_agent_command(&pane_id, "claude", MASTER_PROMPT);
        self.schedule_session_save();
        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(index),
            },
        )
    }

    /// Macht einen bestehenden Chat zum Master.
    pub(super) fn handle_chat_promote(
        &mut self,
        id: String,
        params: ChatWorkspaceParams,
    ) -> String {
        if let Some(response) = self.reject_child_caller(&id) {
            return response;
        }
        let index = match self.chat_target(&id, &params) {
            Ok(index) => index,
            Err(response) => return response,
        };
        let ws = &mut self.state.workspaces[index];
        if ws.chat.team.parent().is_some() {
            return encode_error(id, "chat_is_child", "a child chat cannot become a master");
        }
        if !ws.chat.team.is_master() {
            ws.chat.team.role = ChatRole::Master;
            ws.set_custom_name(MASTER_LABEL.to_owned());
            let workspace_id = ws.id.clone();
            crate::logging::workspace_renamed(&workspace_id);
            if let Some(pane_id) = self.chat_agent_pane(index) {
                if self.chat_agent_running(index) {
                    self.submit_to_pane(index, pane_id, MASTER_PROMPT);
                } else if let Some(public) = self.public_pane_id(index, pane_id) {
                    self.type_chat_agent_command(&public, "claude", MASTER_PROMPT);
                    self.watch_trust_prompt(index);
                }
            }
            self.schedule_session_save();
        }
        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(index),
            },
        )
    }

    /// Macht einen Master wieder zum normalen Chat. Seine Children kommen in
    /// die Hauptliste.
    pub(super) fn handle_chat_demote(&mut self, id: String, params: ChatWorkspaceParams) -> String {
        if let Some(response) = self.reject_child_caller(&id) {
            return response;
        }
        let index = match self.chat_target(&id, &params) {
            Ok(index) => index,
            Err(response) => return response,
        };
        if !self.state.workspaces[index].chat.team.is_master() {
            return encode_error(id, "chat_not_master", "chat is not a master");
        }
        self.release_children(index);
        let ws = &mut self.state.workspaces[index];
        ws.chat.team = Default::default();
        ws.custom_name = None;
        self.schedule_session_save();
        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(index),
            },
        )
    }

    /// Hängt Chats als Children an einen Master.
    pub(super) fn handle_chat_adopt(&mut self, id: String, params: ChatAdoptParams) -> String {
        if let Some(response) = self.reject_child_caller(&id) {
            return response;
        }
        let Some(master) = self.parse_workspace_id(&params.master_workspace_id) else {
            return encode_error(id, "workspace_not_found", "master not found");
        };
        if !self.state.workspaces[master].chat.team.is_master() {
            return encode_error(id, "chat_not_master", "target is not a master");
        }
        let mut adopted = 0;
        for workspace_id in &params.workspace_ids {
            if let Some(child) = self.parse_workspace_id(workspace_id) {
                adopted += usize::from(self.adopt_child(master, child));
            }
        }
        if adopted == 0 {
            return encode_error(id, "nothing_adopted", "no chat could become a child");
        }
        self.schedule_session_save();
        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(master),
            },
        )
    }

    /// Startet ein Child für einen Master.
    pub(super) fn handle_chat_spawn(&mut self, id: String, params: ChatSpawnParams) -> String {
        if let Some(response) = self.reject_child_caller(&id) {
            return response;
        }
        let master = match self.chat_target(
            &id,
            &ChatWorkspaceParams {
                workspace_id: params.master_workspace_id.clone(),
                caller_pane_id: params.caller_pane_id.clone(),
            },
        ) {
            Ok(index) => index,
            Err(response) => return response,
        };
        if !self.state.workspaces[master].chat.team.is_master() {
            return encode_error(
                id,
                "chat_not_master",
                "only a master chat can spawn children",
            );
        }
        let running = self.running_children(master);
        if running >= self.max_running_children {
            return encode_error(
                id,
                "too_many_children",
                format!(
                    "{running} children are running, the limit is {}; stop one first",
                    self.max_running_children
                ),
            );
        }
        if params.check_only {
            return encode_success(id, ResponseResult::Ok {});
        }
        let prompt = params.prompt.trim();
        if prompt.is_empty() {
            return encode_error(id, "empty_chat_prompt", "a child needs a prompt");
        }
        let master_id = self.state.workspaces[master].id.clone();
        let assignment = prompt
            .chars()
            .take(MAX_ASSIGNMENT_CHARS)
            .collect::<String>();
        let agent = params.agent.as_deref().unwrap_or("claude").to_owned();
        if let (Some(workspace_id), Some(known)) = (
            params.workspace_id.as_deref(),
            params.known_workspace_ids.as_deref(),
        ) {
            self.close_spare_worktree_parent(workspace_id, known);
        }
        let (child_id, pane_id, has_env) = match params.workspace_id.as_deref() {
            Some(workspace_id) => {
                let Some(index) = self.parse_workspace_id(workspace_id) else {
                    return encode_error(id, "workspace_not_found", "workspace not found");
                };
                let ws = &self.state.workspaces[index];
                if ws.chat.team.is_master() || ws.chat.team.parent().is_some() {
                    return encode_error(id, "chat_has_role", "workspace already has a chat role");
                }
                let Some(pane_id) = ws
                    .tabs
                    .first()
                    .and_then(|tab| self.public_pane_id(index, tab.root_pane))
                else {
                    return encode_error(id, "pane_not_found", "workspace has no pane");
                };
                (ws.id.clone(), pane_id, false)
            }
            None => {
                let cwd = params.cwd.clone().or_else(|| self.chat_cwd(master));
                let briefing = child_prompt(&master_id, &assignment);
                match self.create_chat_workspace(
                    &id,
                    cwd,
                    false,
                    params.title.as_deref().unwrap_or("Child"),
                    Some(briefing),
                ) {
                    Ok((child_id, pane_id)) => (child_id, pane_id, true),
                    Err(response) => return response,
                }
            }
        };
        let Some(child) = self.parse_workspace_id(&child_id) else {
            return encode_error(id, "workspace_not_found", "child vanished");
        };
        let ws = &mut self.state.workspaces[child];
        ws.chat.team.role = ChatRole::Child {
            parent: master_id.clone(),
        };
        ws.chat.team.assignment = Some(assignment.clone());
        self.watch_trust_prompt(child);
        let ws = &mut self.state.workspaces[child];
        match params.title {
            Some(title) => ws.set_custom_name(title),
            // Ohne Titel benennt der Chat-Hook das Child nach dem ersten Turn.
            None if has_env => ws.custom_name = None,
            None => {}
        }
        if has_env {
            self.start_agent_with_env_prompt(&pane_id, &agent);
        } else {
            self.type_chat_agent_command(&pane_id, &agent, &child_prompt(&master_id, &assignment));
        }
        self.schedule_session_save();
        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(child),
            },
        )
    }

    /// Beendet den Agent eines Chats. Der Space bleibt als gestoppter Chat
    /// stehen und lässt sich mit `chat.resume` fortsetzen.
    pub(super) fn handle_chat_stop(&mut self, id: String, params: ChatWorkspaceParams) -> String {
        let index = match self.chat_target(&id, &params) {
            Ok(index) => index,
            Err(response) => return response,
        };
        if let Err(response) = self.guard_child_caller(&id, index) {
            return response;
        }
        if self.stopped_chat_agent(index).is_some() {
            return encode_success(
                id,
                ResponseResult::WorkspaceInfo {
                    workspace: self.workspace_info(index),
                },
            );
        }
        let Some(pane_id) = self.chat_agent_pane(index) else {
            return encode_error(id, "agent_not_found", "chat has no agent");
        };
        let Some(terminal_id) = self.state.workspaces[index].terminal_id(pane_id).cloned() else {
            return encode_error(id, "agent_not_found", "chat has no terminal");
        };
        let Some(plan) = self
            .state
            .terminals
            .get(&terminal_id)
            .and_then(terminal_resume_plan)
        else {
            return encode_error(
                id,
                "no_resume_plan",
                "the agent reported no session; it could not be resumed after a stop",
            );
        };
        if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
            terminal.pending_agent_resume_plan = Some(plan);
        }
        self.stopping_chat_panes.insert(pane_id);
        self.shutdown_terminal_runtime(terminal_id);
        let ws = &mut self.state.workspaces[index];
        ws.working_since = None;
        ws.blocked_reason = None;
        ws.chat.awaiting = None;
        self.chat_resume_workspaces
            .remove(&self.state.workspaces[index].id);
        self.schedule_session_save();
        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(index),
            },
        )
    }

    /// Startet den gestoppten Agent eines Chats, auch wenn sein Space nicht
    /// aktiv ist.
    pub(super) fn handle_chat_resume(&mut self, id: String, params: ChatWorkspaceParams) -> String {
        let index = match self.chat_target(&id, &params) {
            Ok(index) => index,
            Err(response) => return response,
        };
        if let Err(response) = self.guard_child_caller(&id, index) {
            return response;
        }
        if self.stopped_chat_agent(index).is_none() {
            return encode_error(id, "chat_not_stopped", "chat is not stopped");
        }
        if let Some(master) = self.chat_master(index) {
            let running = self.running_children(master);
            if running >= self.max_running_children {
                return encode_error(
                    id,
                    "too_many_children",
                    format!(
                        "{running} children are running, the limit is {}; stop one first",
                        self.max_running_children
                    ),
                );
            }
        }
        self.chat_resume_workspaces
            .insert(self.state.workspaces[index].id.clone());
        self.watch_trust_prompt(index);
        self.pending_agent_resume_deadline = Some(Instant::now());
        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(index),
            },
        )
    }

    /// Schließt einen Chat. Ein Master schließt seine Children mit oder gibt
    /// sie in die Hauptliste.
    pub(super) fn handle_chat_close(&mut self, id: String, params: ChatCloseParams) -> String {
        let index = match self.chat_target(
            &id,
            &ChatWorkspaceParams {
                workspace_id: Some(params.workspace_id.clone()),
                caller_pane_id: params.caller_pane_id.clone(),
            },
        ) {
            Ok(index) => index,
            Err(response) => return response,
        };
        if let Err(response) = self.guard_child_caller(&id, index) {
            return response;
        }
        let mut close_ids = Vec::new();
        if self.state.workspaces[index].chat.team.is_master() {
            match params.children {
                ChatCloseChildren::Close => {
                    close_ids.extend(
                        self.chat_children(index)
                            .into_iter()
                            .map(|child| self.state.workspaces[child].id.clone()),
                    );
                }
                ChatCloseChildren::Release => self.release_children(index),
            }
        }
        close_ids.push(self.state.workspaces[index].id.clone());
        let mut failure = None;
        for workspace_id in close_ids {
            let response = self.handle_workspace_close(
                format!("{id}:{workspace_id}"),
                WorkspaceCloseParams {
                    workspace_id,
                    close_group: false,
                },
            );
            if response.contains("\"error\"") {
                failure = Some(response);
            }
        }
        if let Some(error) = failure
            .and_then(|failure| serde_json::from_str::<serde_json::Value>(&failure).ok())
            .and_then(|failure| failure.get("error").cloned())
        {
            let field = |name: &str| {
                error
                    .get(name)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("workspace_close_failed")
                    .to_owned()
            };
            return encode_error(id, &field("code"), field("message"));
        }
        encode_success(id, ResponseResult::Ok {})
    }

    /// Nachricht eines Childs, oder des Nutzers über ein Child, an die Inbox
    /// des Masters.
    pub(super) fn handle_chat_report(&mut self, id: String, params: ChatReportParams) -> String {
        let text = params.text.trim();
        if text.is_empty() {
            return encode_error(id, "empty_chat_report", "report must not be empty");
        }
        let child = match self.chat_target(
            &id,
            &ChatWorkspaceParams {
                workspace_id: params.workspace_id.clone(),
                caller_pane_id: params.caller_pane_id.clone(),
            },
        ) {
            Ok(index) => index,
            Err(response) => return response,
        };
        if let Err(response) = self.guard_child_caller(&id, child) {
            return response;
        }
        let Some(master) = self.chat_master(child) else {
            return encode_error(
                id,
                "chat_not_child",
                "only a child chat reports to a master",
            );
        };
        let from = self.state.workspaces[child].id.clone();
        self.state.workspaces[master]
            .chat
            .team
            .push_inbox(ChatInboxEntry {
                at: crate::workspace::unix_millis_now(),
                from,
                kind: if params.from_user {
                    ChatInboxKind::User
                } else {
                    ChatInboxKind::Report
                },
                text: Some(text.chars().take(MAX_REPORT_CHARS).collect()),
                delivered: false,
            });
        self.chat_inbox_deadline = Some(Instant::now());
        self.schedule_session_save();
        encode_success(id, ResponseResult::Ok {})
    }

    /// Inbox eines Masters.
    pub(super) fn handle_chat_inbox(&mut self, id: String, params: ChatInboxParams) -> String {
        let index = match self.chat_target(
            &id,
            &ChatWorkspaceParams {
                workspace_id: params.workspace_id.clone(),
                caller_pane_id: params.caller_pane_id.clone(),
            },
        ) {
            Ok(index) => index,
            Err(response) => return response,
        };
        if !self.state.workspaces[index].chat.team.is_master() {
            return encode_error(id, "chat_not_master", "chat is not a master");
        }
        let entries = self.state.workspaces[index]
            .chat
            .team
            .inbox
            .iter()
            .filter(|entry| params.all || !entry.delivered)
            .map(|entry| self.inbox_entry_info(entry))
            .collect();
        encode_success(id, ResponseResult::ChatInbox { entries })
    }

    // ---- Inbox-Zustellung ---------------------------------------------------

    /// Der Nutzer hat in ein Pane dieses Space getippt.
    pub(crate) fn note_chat_user_input(&mut self, ws_idx: usize) {
        if let Some(ws) = self.state.workspaces.get_mut(ws_idx) {
            if ws.chat.team.is_master() {
                ws.chat.last_user_input = Some(crate::workspace::unix_millis_now());
            }
        }
    }

    /// Stellt fällige Inboxen zu und plant die nächste Prüfung. Liefert, ob
    /// sich sichtbarer Zustand geändert hat.
    pub(crate) fn handle_chat_inbox_tasks(&mut self, now: Instant) -> bool {
        if !self.chat_restart_noted {
            self.chat_restart_noted = true;
            self.note_stopped_children_after_restart();
        }
        if self
            .chat_inbox_deadline
            .is_some_and(|deadline| now < deadline)
        {
            return false;
        }
        self.chat_inbox_deadline = None;
        let millis = crate::workspace::unix_millis_now();
        let mut changed = false;
        let mut waiting = false;
        let masters = (0..self.state.workspaces.len())
            .filter(|&index| {
                let team = &self.state.workspaces[index].chat.team;
                team.is_master() && team.pending_inbox().next().is_some()
            })
            .collect::<Vec<_>>();
        let mut delivered = Vec::new();
        for master in masters {
            match self.inbox_ready(master, millis) {
                Some(true) => {
                    if self.deliver_inbox(master) {
                        changed = true;
                        delivered.push(master);
                    }
                }
                Some(false) => waiting = true,
                // Ein gestoppter Master staut die Inbox bis zum Resume.
                None => {}
            }
        }
        // Ein Master, der gerade die Child-Inbox bekommen hat, bekommt die
        // Rückmeldungen des Nutzers erst beim nächsten Idle.
        let (replies_changed, replies_waiting) = self.deliver_user_replies(millis, &delivered);
        changed |= replies_changed;
        waiting |= replies_waiting;
        let (digest_changed, digest_waiting) = self.run_daily_digest(millis);
        changed |= digest_changed;
        waiting |= digest_waiting;
        let stale = self
            .chat_resume_workspaces
            .iter()
            .filter(|workspace_id| {
                self.parse_workspace_id(workspace_id)
                    .is_none_or(|index| self.stopped_chat_agent(index).is_none())
            })
            .cloned()
            .collect::<Vec<_>>();
        for workspace_id in stale {
            self.chat_resume_workspaces.remove(&workspace_id);
        }
        if self.answer_trust_prompts(millis) {
            waiting = true;
        }
        if waiting {
            self.chat_inbox_deadline = Some(now + INBOX_RETRY);
        } else if self.has_user_replies()
            || self
                .state
                .workspaces
                .iter()
                .any(|ws| ws.chat.team.is_master() && ws.chat.team.pending_inbox().next().is_some())
        {
            // Gestoppte Master prüfen wir selten.
            self.chat_inbox_deadline = Some(now + INBOX_RETRY * 10);
        }
        // collie: Zur nächsten Tagesübersicht wieder aufwachen.
        if let Some(digest) = self.next_digest_check(now) {
            self.chat_inbox_deadline = Some(
                self.chat_inbox_deadline
                    .map_or(digest, |deadline| deadline.min(digest)),
            );
        }
        changed
    }

    /// collie beantwortet die Vertrauensfrage eines Agents, den ein Master
    /// gestartet hat. Das Verzeichnis hat der Master selbst gewählt.
    fn watch_trust_prompt(&mut self, index: usize) {
        if !self.trust_child_folders {
            return;
        }
        if let Some(ws) = self.state.workspaces.get_mut(index) {
            ws.chat.trust_watch_until = Some(crate::workspace::unix_millis_now() + TRUST_WATCH_MS);
        }
        self.chat_inbox_deadline = Some(Instant::now());
    }

    /// Sucht die Vertrauensfrage in beobachteten Panes und bestätigt sie.
    /// Liefert, ob noch Panes beobachtet werden.
    fn answer_trust_prompts(&mut self, millis: u64) -> bool {
        let mut watching = false;
        for index in 0..self.state.workspaces.len() {
            let Some(until) = self.state.workspaces[index].chat.trust_watch_until else {
                continue;
            };
            if millis > until {
                self.state.workspaces[index].chat.trust_watch_until = None;
                continue;
            }
            let Some(pane_id) = self.chat_agent_pane(index) else {
                continue;
            };
            let Some(runtime) = self.lookup_runtime_sender(index, pane_id) else {
                watching = true;
                continue;
            };
            let text = runtime.visible_text();
            if !TRUST_PROMPT_MARKERS
                .iter()
                .any(|marker| text.contains(marker))
            {
                watching = true;
                continue;
            }
            // Die Auswahl steht auf „No, exit"; eine Zeile tiefer liegt „Yes".
            match super::super::api_helpers::encode_api_input(
                runtime,
                "",
                &["Down".to_owned(), "Enter".to_owned()],
            ) {
                Ok(bytes) => {
                    if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
                        tracing::warn!(err = %err, "trust prompt not answered");
                    }
                }
                Err(key) => tracing::warn!(key = %key, "trust prompt keys unsupported"),
            }
            tracing::info!(
                workspace = %self.state.workspaces[index].id,
                "answered the folder trust prompt of a chat agent"
            );
            self.state.workspaces[index].chat.trust_watch_until = None;
        }
        watching
    }

    /// `Some(true)`, wenn die Inbox jetzt zugestellt werden kann,
    /// `Some(false)`, wenn sie noch warten muss, `None` bei gestopptem Master.
    fn inbox_ready(&self, master: usize, millis: u64) -> Option<bool> {
        if self.stopped_chat_agent(master).is_some() || !self.chat_agent_running(master) {
            return None;
        }
        let ws = &self.state.workspaces[master];
        let last = ws.chat.team.pending_inbox().map(|entry| entry.at).max()?;
        if millis.saturating_sub(last) < INBOX_QUIET_MS {
            return Some(false);
        }
        if ws
            .chat
            .last_user_input
            .is_some_and(|typed| millis.saturating_sub(typed) < INBOX_TYPING_MS)
        {
            return Some(false);
        }
        let status = self.workspace_info(master).agent_status;
        Some(matches!(
            status,
            crate::api::schema::AgentStatus::Idle | crate::api::schema::AgentStatus::Done
        ))
    }

    fn deliver_inbox(&mut self, master: usize) -> bool {
        let Some(pane_id) = self.chat_agent_pane(master) else {
            return false;
        };
        let pending = self.state.workspaces[master]
            .chat
            .team
            .pending_inbox()
            .cloned()
            .collect::<Vec<_>>();
        let lines = inbox_lines(&pending, |entry| self.inbox_entry_info(entry));
        if lines.is_empty() || !self.submit_to_pane(master, pane_id, &inbox_prompt(&lines)) {
            return false;
        }
        for entry in &mut self.state.workspaces[master].chat.team.inbox {
            entry.delivered = true;
        }
        self.schedule_session_save();
        true
    }

    /// Nach einem Neustart erfährt jeder Master, welche Children gestoppt sind.
    fn note_stopped_children_after_restart(&mut self) {
        let now = crate::workspace::unix_millis_now();
        for master in 0..self.state.workspaces.len() {
            if !self.state.workspaces[master].chat.team.is_master() {
                continue;
            }
            let stopped = self
                .chat_children(master)
                .into_iter()
                .filter(|&child| self.stopped_chat_agent(child).is_some())
                .map(|child| {
                    format!(
                        "{} {}",
                        self.state.workspaces[child].id,
                        self.workspace_info(child).label
                    )
                })
                .collect::<Vec<_>>();
            if stopped.is_empty() {
                continue;
            }
            self.state.workspaces[master]
                .chat
                .team
                .push_inbox(ChatInboxEntry {
                    at: now,
                    from: String::new(),
                    kind: ChatInboxKind::Restart,
                    text: Some(format!(
                        "collie wurde neu gestartet. Gestoppte Children: {}. Setze sie bei Bedarf mit `herdr chat resume <id>` fort.",
                        stopped.join(", ")
                    )),
                    delivered: false,
                });
            self.chat_inbox_deadline = Some(Instant::now());
        }
    }

    // ---- Hilfen -------------------------------------------------------------

    /// Master-Chat-Fakten für `WorkspaceInfo`.
    pub(crate) fn chat_team_info(&self, index: usize) -> Option<ChatTeamInfo> {
        let team = &self.state.workspaces.get(index)?.chat.team;
        match &team.role {
            ChatRole::None => None,
            ChatRole::Master => Some(ChatTeamInfo {
                role: ChatTeamRole::Master,
                parent_workspace_id: None,
                assignment: None,
                child_count: self.chat_children(index).len(),
                inbox_pending: team.pending_inbox().count(),
                user_inbox_important: team.user_inbox_counts().0,
                user_inbox_optional: team.user_inbox_counts().1,
                user_inbox_revision: team.user_inbox_revision,
                inbox_latest: team
                    .inbox
                    .iter()
                    .rev()
                    .find(|entry| entry.kind != ChatInboxKind::Done)
                    .map(|entry| {
                        let info = self.inbox_entry_info(entry);
                        match info.text {
                            Some(text) if info.title.is_empty() => text,
                            Some(text) => format!("{}: {text}", info.title),
                            None => format!("{}: {}", info.title, info.kind),
                        }
                    }),
            }),
            ChatRole::Child { parent } => Some(ChatTeamInfo {
                role: ChatTeamRole::Child,
                parent_workspace_id: Some(parent.clone()),
                assignment: team.assignment.clone(),
                child_count: 0,
                inbox_pending: 0,
                inbox_latest: None,
                user_inbox_important: 0,
                user_inbox_optional: 0,
                user_inbox_revision: 0,
            }),
        }
    }

    fn inbox_entry_info(&self, entry: &ChatInboxEntry) -> ChatInboxEntryInfo {
        let child = (!entry.from.is_empty())
            .then(|| self.parse_workspace_id(&entry.from))
            .flatten();
        let title = child
            .map(|child| {
                self.state.workspaces[child]
                    .display_name_from(&self.state.terminals, &self.terminal_runtimes)
            })
            .unwrap_or_default();
        // Fertig- und Blockiert-Meldungen lesen den Stand erst jetzt, denn der
        // Hook meldet letzte Nachricht und Grund oft kurz nach dem Wechsel.
        let text = entry.text.clone().or_else(|| {
            let ws = &self.state.workspaces[child?];
            match entry.kind {
                ChatInboxKind::Blocked => ws.blocked_reason.clone(),
                ChatInboxKind::Done => ws.last_message.clone(),
                _ => None,
            }
        });
        ChatInboxEntryInfo {
            at: entry.at,
            workspace_id: entry.from.clone(),
            title,
            kind: entry.kind.as_str().to_owned(),
            text,
            delivered: entry.delivered,
        }
    }

    /// Master eines Childs.
    pub(crate) fn chat_master(&self, index: usize) -> Option<usize> {
        let parent = self.state.workspaces.get(index)?.chat.team.parent()?;
        self.parse_workspace_id(parent)
    }

    /// Children eines Masters in Space-Reihenfolge.
    pub(crate) fn chat_children(&self, master: usize) -> Vec<usize> {
        let Some(master_id) = self.state.workspaces.get(master).map(|ws| ws.id.as_str()) else {
            return Vec::new();
        };
        self.state
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, ws)| ws.chat.team.parent() == Some(master_id))
            .map(|(index, _)| index)
            .collect()
    }

    /// `worktree.create` legt einen Space für das Haupt-Repo an, wenn es noch
    /// keinen gibt. Für ein Child bliebe er als leere Shell stehen. Schließt
    /// ihn wieder, wenn er erst mit diesem Worktree entstanden ist.
    fn close_spare_worktree_parent(&mut self, workspace_id: &str, known: &[String]) {
        let Some(child) = self.parse_workspace_id(workspace_id) else {
            return;
        };
        let Some(key) = self.state.workspaces[child]
            .worktree_space()
            .filter(|space| space.is_linked_worktree)
            .map(|space| space.key.clone())
        else {
            return;
        };
        let spare = (0..self.state.workspaces.len()).find(|&index| {
            let ws = &self.state.workspaces[index];
            index != child
                && !known.contains(&self.public_workspace_id(index))
                && ws
                    .worktree_space()
                    .is_some_and(|space| !space.is_linked_worktree && space.key == key)
                && ws.chat.team.role == ChatRole::None
                && !ws.chat.team.user_inbox_space
                && ws.tabs.len() == 1
                && self.workspace_info(index).pane_count == 1
        });
        let Some(spare) = spare else {
            return;
        };
        let spare_id = self.public_workspace_id(spare);
        let info = self.workspace_info(spare);
        self.state.close_workspaces(vec![spare]);
        self.shutdown_detached_terminal_runtimes();
        self.emit_event(crate::api::schema::EventEnvelope {
            event: crate::api::schema::EventKind::WorkspaceClosed,
            data: crate::api::schema::EventData::WorkspaceClosed {
                workspace_id: spare_id,
                workspace: Some(info),
            },
        });
    }

    fn running_children(&self, master: usize) -> usize {
        self.chat_children(master)
            .into_iter()
            .filter(|&child| self.stopped_chat_agent(child).is_none())
            .count()
    }

    fn adopt_child(&mut self, master: usize, child: usize) -> bool {
        if master == child {
            return false;
        }
        let master_id = self.state.workspaces[master].id.clone();
        let ws = &mut self.state.workspaces[child];
        if ws.chat.team.is_master() {
            return false;
        }
        ws.chat.team.role = ChatRole::Child { parent: master_id };
        true
    }

    fn release_children(&mut self, master: usize) {
        for child in self.chat_children(master) {
            let team = &mut self.state.workspaces[child].chat.team;
            team.role = ChatRole::None;
            team.assignment = None;
        }
    }

    /// Pane des Agents in Tab 1, sonst das Root-Pane von Tab 1.
    pub(crate) fn chat_agent_pane(&self, index: usize) -> Option<crate::layout::PaneId> {
        let tab = self.state.workspaces.get(index)?.tabs.first()?;
        tab.layout
            .pane_ids()
            .into_iter()
            .find(|pane_id| {
                tab.panes
                    .get(pane_id)
                    .and_then(|pane| self.state.terminals.get(&pane.attached_terminal_id))
                    .is_some_and(|terminal| {
                        terminal.effective_agent_label().is_some()
                            || terminal.pending_agent_resume_plan.is_some()
                    })
            })
            .or(Some(tab.root_pane))
    }

    /// Ein Agent läuft in Tab 1.
    pub(super) fn chat_agent_running(&self, index: usize) -> bool {
        let Some(tab) = self
            .state
            .workspaces
            .get(index)
            .and_then(|ws| ws.tabs.first())
        else {
            return false;
        };
        tab.layout.pane_ids().into_iter().any(|pane_id| {
            tab.panes.get(&pane_id).is_some_and(|pane| {
                self.terminal_runtimes
                    .get(&pane.attached_terminal_id)
                    .is_some()
                    && self
                        .state
                        .terminals
                        .get(&pane.attached_terminal_id)
                        .is_some_and(|terminal| terminal.effective_agent_label().is_some())
            })
        })
    }

    /// Arbeitsverzeichnis des Agents eines Space.
    pub(super) fn chat_cwd(&self, index: usize) -> Option<String> {
        let ws = self.state.workspaces.get(index)?;
        ws.chat.cwd.clone().or_else(|| {
            let tab = ws.tabs.first()?;
            let pane = tab.panes.get(&tab.root_pane)?;
            let terminal = self.state.terminals.get(&pane.attached_terminal_id)?;
            Some(terminal.cwd.display().to_string())
        })
    }

    /// Legt einen Space mit Namen an. `prompt` landet in [`CHAT_PROMPT_ENV`].
    /// Liefert Space- und Pane-ID.
    fn create_chat_workspace(
        &mut self,
        id: &str,
        cwd: Option<String>,
        focus: bool,
        label: &str,
        prompt: Option<String>,
    ) -> Result<(String, String), String> {
        let created = self.handle_workspace_create(
            format!("{id}:create"),
            WorkspaceCreateParams {
                source_workspace_id: None,
                cwd,
                focus,
                label: Some(label.to_owned()),
                env: prompt
                    .map(|prompt| (CHAT_PROMPT_ENV.to_owned(), prompt))
                    .into_iter()
                    .collect(),
            },
        );
        let response = serde_json::from_str::<serde_json::Value>(&created).ok();
        let ids = response.as_ref().and_then(|response| {
            Some((
                response
                    .pointer("/result/workspace/workspace_id")?
                    .as_str()?
                    .to_owned(),
                response
                    .pointer("/result/root_pane/pane_id")?
                    .as_str()?
                    .to_owned(),
            ))
        });
        ids.ok_or_else(|| {
            response
                .and_then(|response| {
                    let error = response.get("error")?;
                    Some(encode_error(
                        id.to_owned(),
                        error.get("code")?.as_str()?,
                        error.get("message")?.as_str()?.to_owned(),
                    ))
                })
                .unwrap_or_else(|| {
                    encode_error(id.to_owned(), "workspace_create_failed", "no workspace")
                })
        })
    }

    /// Tippt `<agent> '<prompt>'` in die Shell eines Panes.
    fn type_chat_agent_command(&mut self, pane_id: &str, agent: &str, prompt: &str) {
        self.handle_pane_send_input(
            "chat:start".into(),
            PaneSendInputParams {
                pane_id: pane_id.to_owned(),
                text: format!("{} {}", shell_word(agent), shell_word(prompt)),
                keys: vec!["Enter".into()],
            },
        );
    }

    /// Tippt `<agent> "$COLLIE_CHAT_PROMPT"` in die Shell eines Panes.
    fn start_agent_with_env_prompt(&mut self, pane_id: &str, agent: &str) {
        self.handle_pane_send_input(
            "chat:start".into(),
            PaneSendInputParams {
                pane_id: pane_id.to_owned(),
                text: format!("{} \"${CHAT_PROMPT_ENV}\"", shell_word(agent)),
                keys: vec!["Enter".into()],
            },
        );
    }

    /// Ziel-Space aus Workspace-ID, Aufrufer-Pane oder Aufrufer-Prozess.
    pub(super) fn chat_target(
        &self,
        id: &str,
        params: &ChatWorkspaceParams,
    ) -> Result<usize, String> {
        if let Some(workspace_id) = params.workspace_id.as_deref() {
            return self.parse_workspace_id(workspace_id).ok_or_else(|| {
                encode_error(
                    id.to_owned(),
                    "workspace_not_found",
                    format!("workspace {workspace_id} not found"),
                )
            });
        }
        if let Some(pane_id) = params.caller_pane_id.as_deref() {
            return self
                .parse_pane_id(pane_id)
                .map(|(index, _)| index)
                .ok_or_else(|| {
                    encode_error(
                        id.to_owned(),
                        "pane_not_found",
                        format!("pane {pane_id} not found"),
                    )
                });
        }
        self.caller_workspace().ok_or_else(|| {
            encode_error(
                id.to_owned(),
                "workspace_required",
                "name a workspace or run this inside a collie pane",
            )
        })
    }

    /// Space, in dessen Pane der Prozess hinter der API-Anfrage läuft.
    pub(crate) fn caller_workspace(&self) -> Option<usize> {
        let mut pid = self.api_caller_pid?;
        let shells = self
            .state
            .workspaces
            .iter()
            .enumerate()
            .flat_map(|(index, ws)| {
                ws.tabs.iter().flat_map(move |tab| {
                    tab.panes
                        .values()
                        .map(move |pane| (index, pane.attached_terminal_id.clone()))
                })
            })
            .filter_map(|(index, terminal_id)| {
                let child_pid = self.terminal_runtimes.get(&terminal_id)?.child_pid()?;
                Some((child_pid, index))
            })
            .collect::<std::collections::HashMap<_, _>>();
        for _ in 0..MAX_CALLER_ANCESTORS {
            if let Some(index) = shells.get(&pid) {
                return Some(*index);
            }
            pid = crate::platform::parent_pid(pid).filter(|parent| *parent > 1)?;
        }
        None
    }

    /// Kommt die Anfrage aus einem Child-Chat, darf sie nur den eigenen Space
    /// verändern.
    pub(crate) fn guard_child_caller(&self, id: &str, target: usize) -> Result<(), String> {
        if self.api_caller_pid.is_none()
            || !self
                .state
                .workspaces
                .iter()
                .any(|ws| ws.chat.team.parent().is_some())
        {
            return Ok(());
        }
        match self.caller_workspace() {
            Some(caller)
                if caller != target
                    && self.state.workspaces[caller].chat.team.parent().is_some() =>
            {
                Err(encode_error(
                    id.to_owned(),
                    "child_chat_forbidden",
                    "a child chat may only change its own workspace; report to your master with `herdr chat report`",
                ))
            }
            _ => Ok(()),
        }
    }

    /// Fehlerantwort, wenn ein Child-Chat einen fremden Space verändern will.
    /// Die `chat.*`-Methoden prüfen selbst.
    pub(crate) fn child_caller_rejection(
        &self,
        request: &crate::api::schema::Request,
    ) -> Option<String> {
        use crate::api::schema::Method;
        self.api_caller_pid?;
        let pane = |pane_id: &str| self.parse_pane_id(pane_id).map(|(index, _)| index);
        let agent = |target: &str| {
            self.resolve_agent_target(target)
                .or_else(|_| self.resolve_terminal_target(target))
                .ok()
                .map(|target| target.ws_idx)
        };
        let target = match &request.method {
            Method::AgentPrompt(params) => agent(&params.target),
            Method::AgentSendKeys(params) => agent(&params.target),
            Method::AgentStart(params) => pane(&params.pane_id),
            Method::PaneSendText(params) => pane(&params.pane_id),
            Method::PaneSendKeys(params) => pane(&params.pane_id),
            Method::PaneSendInput(params) => pane(&params.pane_id),
            Method::PaneClose(params) => pane(&params.pane_id),
            Method::WorkspaceClose(params) => self.parse_workspace_id(&params.workspace_id),
            Method::WorkspaceRename(params) => self.parse_workspace_id(&params.workspace_id),
            _ => None,
        }?;
        self.guard_child_caller(&request.id, target).err()
    }

    /// Lehnt Anfragen ab, die ein Child-Chat nie stellen darf.
    fn reject_child_caller(&self, id: &str) -> Option<String> {
        let caller = self.caller_workspace()?;
        self.state.workspaces[caller].chat.team.parent().map(|_| {
            encode_error(
                id.to_owned(),
                "child_chat_forbidden",
                "a child chat cannot manage chats; report to your master with `herdr chat report`",
            )
        })
    }
}

/// Resume-Plan aus der Session, die der Agent gemeldet hat.
fn terminal_resume_plan(
    terminal: &crate::terminal::TerminalState,
) -> Option<crate::agent_resume::AgentResumePlan> {
    if let Some(resume) = terminal.reported_resume() {
        return Some(resume.plan(&terminal.cwd));
    }
    if let Some(authority) = terminal.hook_authority.as_ref() {
        if let Some(session_ref) = authority.session_ref.as_ref() {
            return crate::agent_resume::plan(
                &authority.source,
                &authority.agent_label,
                session_ref,
            );
        }
    }
    let session = terminal.persisted_agent_session.as_ref()?;
    crate::agent_resume::plan(&session.source, &session.agent, &session.session_ref)
}

/// Auftrag mit Briefing für ein Child.
fn child_prompt(master_id: &str, assignment: &str) -> String {
    format!(
        "Du bist ein Child-Chat in collie. Dein Master Chat ist der Space `{master_id}`. \
         Regeln:\n\
         - Arbeite nur an deinem Auftrag unten.\n\
         - Melde dich nur bei deinem Master: `herdr chat report \"<nachricht>\"`. \
         Steuere keine anderen Chats.\n\
         - Brauchst du eine Entscheidung, melde die Frage per `herdr chat report` und warte, \
         bis dein Master antwortet.\n\
         - Bist du fertig, melde kurz das Ergebnis und was noch offen ist.\n\
         \nAuftrag:\n{assignment}"
    )
}

/// Zeilen der Inbox-Zustellung. Von Fertig- und Blockiert-Meldungen eines
/// Childs zählt nur die letzte.
fn inbox_lines(
    pending: &[ChatInboxEntry],
    info: impl Fn(&ChatInboxEntry) -> ChatInboxEntryInfo,
) -> Vec<String> {
    let mut lines = Vec::new();
    for (index, entry) in pending.iter().enumerate() {
        let superseded = matches!(entry.kind, ChatInboxKind::Done | ChatInboxKind::Blocked)
            && pending[index + 1..].iter().any(|later| {
                later.from == entry.from
                    && matches!(later.kind, ChatInboxKind::Done | ChatInboxKind::Blocked)
            });
        if superseded {
            continue;
        }
        let info = info(entry);
        let who = if info.workspace_id.is_empty() {
            "collie".to_owned()
        } else {
            format!("{} {}", info.workspace_id, info.title)
        };
        let text = info.text.unwrap_or_default();
        lines.push(
            format!("- {who} ({}): {text}", info.kind)
                .trim_end()
                .to_owned(),
        );
    }
    lines
}

fn inbox_prompt(lines: &[String]) -> String {
    format!("[collie inbox]\n{}", lines.join("\n"))
}

/// Ein Wort für die Shell, wenn nötig in einfachen Anführungszeichen.
pub(super) fn shell_word(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/'))
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{Method, Request};

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![
            crate::workspace::Workspace::test_new("master"),
            crate::workspace::Workspace::test_new("child"),
        ];
        app.state.active = Some(0);
        app
    }

    fn call(app: &mut App, method: Method) -> serde_json::Value {
        let response = app.handle_api_request(Request {
            id: "req".into(),
            method,
        });
        serde_json::from_str(&response).expect("json response")
    }

    #[test]
    fn promote_adopt_report_inbox_and_demote() {
        let mut app = test_app();
        let master = app.state.workspaces[0].id.clone();
        let child = app.state.workspaces[1].id.clone();

        let promoted = call(
            &mut app,
            Method::ChatPromote(ChatWorkspaceParams {
                workspace_id: Some(master.clone()),
                caller_pane_id: None,
            }),
        );
        assert_eq!(promoted["result"]["workspace"]["label"], MASTER_LABEL);
        assert_eq!(promoted["result"]["workspace"]["team"]["role"], "master");

        let adopted = call(
            &mut app,
            Method::ChatAdopt(ChatAdoptParams {
                master_workspace_id: master.clone(),
                workspace_ids: vec![child.clone(), master.clone()],
            }),
        );
        assert_eq!(adopted["result"]["workspace"]["team"]["child_count"], 1);

        let report = call(
            &mut app,
            Method::ChatReport(ChatReportParams {
                workspace_id: Some(child.clone()),
                text: "Migration jetzt oder später?".into(),
                ..Default::default()
            }),
        );
        assert!(report.get("error").is_none(), "{report}");
        let not_child = call(
            &mut app,
            Method::ChatReport(ChatReportParams {
                workspace_id: Some(master.clone()),
                text: "x".into(),
                ..Default::default()
            }),
        );
        assert_eq!(not_child["error"]["code"], "chat_not_child");

        let inbox = call(
            &mut app,
            Method::ChatInbox(ChatInboxParams {
                workspace_id: Some(master.clone()),
                ..Default::default()
            }),
        );
        assert_eq!(inbox["result"]["entries"][0]["kind"], "report");
        assert_eq!(inbox["result"]["entries"][0]["workspace_id"], child);
        let info = app.chat_team_info(0).expect("master info");
        assert_eq!(info.inbox_pending, 1);
        assert!(info
            .inbox_latest
            .is_some_and(|latest| latest.ends_with("Migration jetzt oder später?")));

        // Ein Master ignoriert automatische Titel.
        let pane_id = app
            .public_pane_id(0, app.state.workspaces[0].tabs[0].root_pane)
            .unwrap();
        call(
            &mut app,
            Method::PaneReportChat(crate::api::schema::PaneReportChatParams {
                pane_id,
                title: Some("Anderer Titel".into()),
                ..Default::default()
            }),
        );
        assert_eq!(
            app.state.workspaces[0].custom_name.as_deref(),
            Some(MASTER_LABEL)
        );

        call(
            &mut app,
            Method::ChatDemote(ChatWorkspaceParams {
                workspace_id: Some(master),
                caller_pane_id: None,
            }),
        );
        assert!(app
            .state
            .workspaces
            .iter()
            .all(|ws| ws.chat.team.is_default()));
    }

    #[test]
    fn spawn_respects_the_running_children_limit() {
        let mut app = test_app();
        let master = app.state.workspaces[0].id.clone();
        app.state.workspaces[0].chat.team.role = ChatRole::Master;
        app.state.workspaces[1].chat.team.role = ChatRole::Child {
            parent: master.clone(),
        };
        app.max_running_children = 1;
        let refused = call(
            &mut app,
            Method::ChatSpawn(ChatSpawnParams {
                master_workspace_id: Some(master.clone()),
                prompt: "Baue X".into(),
                check_only: true,
                ..Default::default()
            }),
        );
        assert_eq!(refused["error"]["code"], "too_many_children");

        app.max_running_children = 2;
        let allowed = call(
            &mut app,
            Method::ChatSpawn(ChatSpawnParams {
                master_workspace_id: Some(master),
                prompt: "Baue X".into(),
                check_only: true,
                ..Default::default()
            }),
        );
        assert!(allowed.get("error").is_none(), "{allowed}");
    }

    #[test]
    fn spawn_closes_the_repo_space_that_the_worktree_opened() {
        let membership = |linked: bool| crate::workspace::WorktreeSpaceMembership {
            key: "/repo/.git".into(),
            label: "repo".into(),
            repo_root: "/repo".into(),
            checkout_path: if linked {
                "/wt/feat".into()
            } else {
                "/repo".into()
            },
            is_linked_worktree: linked,
        };
        let mut app = test_app();
        let mut worktree = crate::workspace::Workspace::test_new("feat");
        worktree.worktree_space = Some(membership(true));
        let mut spare = crate::workspace::Workspace::test_new("repo");
        spare.worktree_space = Some(membership(false));
        app.state.workspaces.push(spare);
        app.state.workspaces.push(worktree);
        let known = vec![app.public_workspace_id(0), app.public_workspace_id(1)];
        let worktree_id = app.public_workspace_id(3);

        // Ein Repo-Space, den es vorher schon gab, bleibt.
        let mut all = known.clone();
        all.push(app.public_workspace_id(2));
        app.close_spare_worktree_parent(&worktree_id, &all);
        assert_eq!(app.state.workspaces.len(), 4);

        app.close_spare_worktree_parent(&worktree_id, &known);
        assert_eq!(app.state.workspaces.len(), 3);
        assert!(app
            .state
            .workspaces
            .iter()
            .all(|ws| ws.custom_name.as_deref() != Some("repo")
                && ws
                    .worktree_space()
                    .is_none_or(|space| space.is_linked_worktree)));
    }

    #[test]
    fn closing_a_master_releases_or_closes_its_children() {
        let mut app = test_app();
        let master = app.state.workspaces[0].id.clone();
        app.state.workspaces[0].chat.team.role = ChatRole::Master;
        app.state.workspaces[1].chat.team.role = ChatRole::Child {
            parent: master.clone(),
        };
        let closed = call(
            &mut app,
            Method::ChatClose(ChatCloseParams {
                workspace_id: master,
                children: ChatCloseChildren::Release,
                caller_pane_id: None,
            }),
        );
        assert!(closed.get("error").is_none(), "{closed}");
        assert_eq!(app.state.workspaces.len(), 1);
        assert!(app.state.workspaces[0].chat.team.is_default());
    }

    #[test]
    fn a_child_going_idle_reports_to_its_master() {
        let mut app = test_app();
        let master = app.state.workspaces[0].id.clone();
        app.state.workspaces[0].chat.team.role = ChatRole::Master;
        app.state.workspaces[1].chat.team.role = ChatRole::Child { parent: master };
        app.state.workspaces[1].last_message = Some("Fertig.".into());
        app.state.track_chat_state_for_test(
            1,
            crate::detect::AgentState::Working,
            crate::detect::AgentState::Idle,
        );
        let inbox = &app.state.workspaces[0].chat.team.inbox;
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].kind, ChatInboxKind::Done);
        let info = app.inbox_entry_info(&inbox[0].clone());
        assert_eq!(info.text.as_deref(), Some("Fertig."));
    }

    fn entry(from: &str, kind: ChatInboxKind, text: Option<&str>) -> ChatInboxEntry {
        ChatInboxEntry {
            at: 1,
            from: from.into(),
            kind,
            text: text.map(str::to_owned),
            delivered: false,
        }
    }

    #[test]
    fn inbox_lines_keep_only_the_latest_state_of_a_child() {
        let pending = [
            entry("w2", ChatInboxKind::Blocked, None),
            entry("w2", ChatInboxKind::Report, Some("Frage")),
            entry("w2", ChatInboxKind::Done, None),
            entry("", ChatInboxKind::Restart, Some("neu gestartet")),
        ];
        let lines = inbox_lines(&pending, |entry| ChatInboxEntryInfo {
            at: entry.at,
            workspace_id: entry.from.clone(),
            title: if entry.from.is_empty() {
                String::new()
            } else {
                "Fix".into()
            },
            kind: entry.kind.as_str().into(),
            text: entry.text.clone().or(Some("fertig".into())),
            delivered: false,
        });
        assert_eq!(
            lines,
            [
                "- w2 Fix (report): Frage",
                "- w2 Fix (done): fertig",
                "- collie (restart): neu gestartet",
            ]
        );
        assert!(inbox_prompt(&lines).starts_with("[collie inbox]\n- w2"));
    }

    #[test]
    fn child_prompt_names_the_master_and_the_report_command() {
        let prompt = child_prompt("w1", "Baue X");
        assert!(prompt.contains("Space `w1`"));
        assert!(prompt.contains("herdr chat report"));
        assert!(prompt.ends_with("Auftrag:\nBaue X"));
    }

    #[test]
    fn shell_word_quotes_prompts() {
        assert_eq!(shell_word("claude"), "claude");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
    }
}
