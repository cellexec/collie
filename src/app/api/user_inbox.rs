//! collie: Nutzer-Inbox, siehe docs/specs/user-inbox.md.
//!
//! Jeder Chat kann Nachrichten, Formulare und Wizard-Aufgaben für den Nutzer
//! ablegen; Antworten eines Masters auf Prompts des Nutzers landen
//! automatisch dort. Gespeichert wird je Chat, damit Rückmeldungen beim
//! richtigen Agent ankommen; die TUI zeigt alle Chats als eine Inbox. Was der
//! Nutzer damit macht, geht gebündelt als `[collie user]`-Prompt zurück.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::api::schema::{
    ChatTellUserParams, ChatUserInboxActParams, ChatUserInboxAction, ChatUserInboxParams,
    ChatWorkspaceParams, PaneRenameParams, PaneSendInputParams, PaneSplitParams, ResponseResult,
    SplitDirection, UserInboxEntryInfo,
};
use crate::app::App;
use crate::workspace::{UserInboxEntry, UserInboxKind, UserReply};

use super::chats::shell_word;
use super::responses::{encode_error, encode_success};

/// Kopfzeile des Prompts, mit dem Rückmeldungen beim Master ankommen.
pub(crate) const USER_REPLY_HEADER: &str = "[collie user]";
/// Prompts, die collie selbst schickt, erzeugen keine Antwort in der Inbox.
const COLLIE_PROMPT_PREFIX: &str = "[collie ";
/// So lange wartet eine Rückmeldung, damit mehrere zusammen ankommen.
const REPLY_QUIET_MS: u64 = 2_000;
/// So lange wartet die Zustellung, nachdem der Nutzer im Master getippt hat.
const REPLY_TYPING_MS: u64 = 5_000;
/// Spätestens dann gehen zurückgehaltene Rückmeldungen raus, auch wenn die
/// TUI das Verlassen des Overlays nie gemeldet hat.
const HELD_REPLY_MAX_MS: u64 = 30 * 60_000;
/// Höchstlänge eines Titels.
const MAX_TITLE_CHARS: usize = 80;
/// Höchstlänge einer Rückfrage.
const MAX_REPLY_CHARS: usize = 4_000;
/// Erste Zeile einer Rückfrage-Datei; sie gehört nicht zum Text.
const REPLY_FILE_HEADER: &str = "# Rückfrage zu: ";

impl App {
    /// `chat.tell_user`: Ein Chat legt etwas in die Nutzer-Inbox.
    pub(super) fn handle_chat_tell_user(
        &mut self,
        id: String,
        params: ChatTellUserParams,
    ) -> String {
        let master = match self.user_inbox_chat(&id, params.workspace_id, params.caller_pane_id) {
            Ok(index) => index,
            Err(response) => return response,
        };
        let Some(kind) =
            UserInboxKind::parse(&params.kind).filter(|kind| *kind != UserInboxKind::Antwort)
        else {
            return encode_error(
                id,
                "invalid_user_inbox_kind",
                "kind must be nachricht, formular or wizard",
            );
        };
        let text = params.text.trim().to_owned();
        let path = match (kind.is_task(), params.path.as_deref()) {
            (true, None) => {
                return encode_error(
                    id,
                    "user_inbox_path_required",
                    "formular and wizard need --file",
                );
            }
            (true, Some(path)) => match absolute_file(path) {
                Some(path) => Some(path),
                None => {
                    return encode_error(
                        id,
                        "user_inbox_file_not_found",
                        format!("{path} is not a readable file"),
                    );
                }
            },
            (false, _) => None,
        };
        if text.is_empty() && path.is_none() {
            return encode_error(id, "empty_user_inbox_entry", "message must not be empty");
        }
        let title = params
            .title
            .as_deref()
            .and_then(one_line)
            .or_else(|| one_line(&text))
            .or_else(|| {
                path.as_deref()
                    .and_then(|path| Path::new(path).file_name())
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        let entry_id = self.state.workspaces[master]
            .chat
            .team
            .push_user_inbox(UserInboxEntry {
                id: 0,
                at: crate::workspace::unix_millis_now(),
                kind,
                important: params.important,
                title,
                text,
                path,
                key: None,
                read: false,
                finished: false,
                deleted: false,
            });
        tracing::info!(
            workspace = %self.state.workspaces[master].id,
            entry = entry_id,
            kind = kind.as_str(),
            "chat wrote to the user inbox"
        );
        self.schedule_session_save();
        self.user_inbox_response(id, Some(master))
    }

    /// `chat.user_inbox`: Einträge, die der Nutzer noch nicht gelöscht hat;
    /// mit `all` die aller Chats.
    pub(super) fn handle_chat_user_inbox(
        &mut self,
        id: String,
        params: ChatUserInboxParams,
    ) -> String {
        if params.all {
            return self.user_inbox_response(id, None);
        }
        match self.user_inbox_chat(&id, params.workspace_id, params.caller_pane_id) {
            Ok(master) => self.user_inbox_response(id, Some(master)),
            Err(response) => response,
        }
    }

    /// `chat.user_inbox_act`: Was der Nutzer mit Einträgen macht.
    pub(super) fn handle_chat_user_inbox_act(
        &mut self,
        id: String,
        params: ChatUserInboxActParams,
    ) -> String {
        let scope = |master: usize| (!params.all).then_some(master);
        if params.all && params.action == ChatUserInboxAction::Flush {
            for ws in &mut self.state.workspaces {
                let team = &mut ws.chat.team;
                if team.user_replies.iter().any(|reply| reply.held) {
                    for reply in &mut team.user_replies {
                        reply.held = false;
                    }
                    team.touch_user_inbox();
                }
            }
            self.chat_inbox_deadline = Some(Instant::now());
            self.schedule_session_save();
            return self.user_inbox_response(id, None);
        }
        let master = match self.user_inbox_chat(
            &id,
            params.workspace_id.clone(),
            params.caller_pane_id.clone(),
        ) {
            Ok(index) => index,
            Err(response) => return response,
        };
        let now = crate::workspace::unix_millis_now();
        let result = match params.action {
            ChatUserInboxAction::Read => {
                self.update_user_entries(master, &params.ids, |entry| entry.read = true);
                Ok(())
            }
            ChatUserInboxAction::Done => {
                self.finish_user_entries(master, &params.ids, now);
                Ok(())
            }
            ChatUserInboxAction::Restore => {
                self.update_user_entries(master, &params.ids, |entry| entry.deleted = false);
                let team = &mut self.state.workspaces[master].chat.team;
                team.user_replies
                    .retain(|reply| !(reply.held && params.ids.contains(&reply.entry_id)));
                Ok(())
            }
            ChatUserInboxAction::Open => self.open_user_entry(master, single(&params.ids)),
            ChatUserInboxAction::Reply => self.open_user_reply(master, single(&params.ids)),
            ChatUserInboxAction::Answered => {
                self.note_form_answered(master, single(&params.ids), now)
            }
            ChatUserInboxAction::WizardResult => self.note_wizard_result(
                master,
                single(&params.ids),
                params.exit_code.unwrap_or(-1),
                now,
            ),
            ChatUserInboxAction::ReplyWritten => {
                self.note_reply_written(master, single(&params.ids), params.path.as_deref(), now)
            }
            ChatUserInboxAction::Flush => {
                for reply in &mut self.state.workspaces[master].chat.team.user_replies {
                    reply.held = false;
                }
                Ok(())
            }
        };
        if let Err((code, message)) = result {
            return encode_error(id, code, message);
        }
        self.state.workspaces[master].chat.team.touch_user_inbox();
        self.chat_inbox_deadline = Some(Instant::now());
        self.schedule_session_save();
        self.user_inbox_response(id, scope(master))
    }

    /// collie: Das Stop-Hook meldet die volle Antwort des Agents. Bei einem
    /// Master und einem Prompt vom Nutzer wird sie ein `antwort`-Eintrag.
    pub(super) fn record_master_answer(
        &mut self,
        index: usize,
        answer: Option<String>,
        prompt: Option<String>,
        key: Option<String>,
    ) {
        let Some(ws) = self.state.workspaces.get_mut(index) else {
            return;
        };
        if !ws.chat.team.is_master() {
            return;
        }
        let (Some(answer), Some(prompt)) = (answer, prompt) else {
            return;
        };
        let answer = answer.trim();
        let prompt = prompt.trim();
        if answer.is_empty() || prompt.is_empty() || prompt.starts_with(COLLIE_PROMPT_PREFIX) {
            return;
        }
        let key = key
            .filter(|key| !key.trim().is_empty())
            .unwrap_or_else(|| format!("prompt:{prompt}"));
        ws.chat.team.push_user_inbox(UserInboxEntry {
            id: 0,
            at: crate::workspace::unix_millis_now(),
            kind: UserInboxKind::Antwort,
            important: true,
            title: one_line(prompt).unwrap_or_default(),
            text: answer.to_owned(),
            path: None,
            key: Some(key),
            read: false,
            finished: false,
            deleted: false,
        });
    }

    /// Stellt fällige Rückmeldungen zu. Liefert `(geändert, wartet)`.
    pub(super) fn deliver_user_replies(&mut self, millis: u64, skip: &[usize]) -> (bool, bool) {
        let mut changed = false;
        let mut waiting = false;
        for master in 0..self.state.workspaces.len() {
            let team = &mut self.state.workspaces[master].chat.team;
            if team.user_replies.is_empty() {
                continue;
            }
            for reply in &mut team.user_replies {
                if reply.held && millis.saturating_sub(reply.at) >= HELD_REPLY_MAX_MS {
                    reply.held = false;
                }
            }
            let Some(last) = team.deliverable_user_replies().map(|reply| reply.at).max() else {
                // Nur zurückgehaltene Rückmeldungen: später erneut prüfen.
                waiting = true;
                continue;
            };
            if skip.contains(&master) {
                waiting = true;
                continue;
            }
            match self.user_replies_ready(master, last, millis) {
                Some(true) => changed |= self.submit_user_replies(master),
                Some(false) => waiting = true,
                None => {}
            }
        }
        (changed, waiting)
    }

    pub(super) fn has_user_replies(&self) -> bool {
        self.state
            .workspaces
            .iter()
            .any(|ws| !ws.chat.team.user_replies.is_empty())
    }

    /// Zähler und Revision der globalen Inbox: `(wichtig, optional,
    /// revision)`. Die Revision ändert sich mit jeder Änderung in einem Chat.
    pub(crate) fn global_user_inbox(&self) -> (usize, usize, u64) {
        self.state
            .workspaces
            .iter()
            .fold((0, 0, 0u64), |(important, optional, revision), ws| {
                let team = &ws.chat.team;
                let (i, o) = team.user_inbox_counts();
                (
                    important + i,
                    optional + o,
                    revision
                        .wrapping_mul(31)
                        .wrapping_add(team.user_inbox_revision),
                )
            })
    }

    // ---- Hilfen -------------------------------------------------------------

    fn user_inbox_chat(
        &self,
        id: &str,
        workspace_id: Option<String>,
        caller_pane_id: Option<String>,
    ) -> Result<usize, String> {
        self.chat_target(
            id,
            &ChatWorkspaceParams {
                workspace_id,
                caller_pane_id,
            },
        )
    }

    /// Einträge eines Chats oder, mit `None`, aller Chats.
    fn user_inbox_response(&self, id: String, chat: Option<usize>) -> String {
        let user_inbox = self
            .state
            .workspaces
            .iter()
            .enumerate()
            .filter(|(index, _)| chat.is_none_or(|chat| chat == *index))
            .filter(|(_, ws)| ws.chat.team.user_inbox.iter().any(|entry| !entry.deleted))
            .flat_map(|(index, ws)| {
                let master = ws.chat.team.is_master();
                let label = self.workspace_info(index).label;
                ws.chat
                    .team
                    .user_inbox
                    .iter()
                    .filter(|entry| !entry.deleted)
                    .map(move |entry| UserInboxEntryInfo {
                        id: entry.id,
                        at: entry.at,
                        kind: entry.kind.as_str().to_owned(),
                        important: entry.important,
                        title: entry.title.clone(),
                        text: entry.text.clone(),
                        path: entry.path.clone(),
                        read: entry.read,
                        finished: entry.finished,
                        workspace_id: ws.id.clone(),
                        chat: label.clone(),
                        master,
                    })
            })
            .collect();
        encode_success(id, ResponseResult::ChatUserInbox { user_inbox })
    }

    fn update_user_entries(
        &mut self,
        master: usize,
        ids: &[u64],
        update: impl Fn(&mut UserInboxEntry),
    ) {
        let team = &mut self.state.workspaces[master].chat.team;
        for id in ids {
            if let Some(entry) = team.user_inbox_entry_mut(*id) {
                update(entry);
            }
        }
    }

    /// `dd`: Einträge verschwinden, der Master erfährt davon, sobald der
    /// Nutzer das Overlay verlässt. Antworten auf eigene Prompts melden wir
    /// nicht, die Reaktion darauf ist der nächste Prompt des Nutzers.
    fn finish_user_entries(&mut self, master: usize, ids: &[u64], now: u64) {
        let team = &mut self.state.workspaces[master].chat.team;
        for id in ids {
            let Some(entry) = team.user_inbox_entry_mut(*id) else {
                continue;
            };
            if entry.deleted {
                continue;
            }
            entry.deleted = true;
            let outcome = match (entry.kind, entry.finished) {
                (UserInboxKind::Antwort, _) => None,
                (UserInboxKind::Nachricht, _) => Some("abgearbeitet"),
                (UserInboxKind::Formular, false) => Some("verworfen, ohne Antwort gelöscht"),
                (UserInboxKind::Wizard, false) => Some("erledigt"),
                (_, true) => Some("abgehakt"),
            };
            let Some(outcome) = outcome else {
                continue;
            };
            let text = format!("{}: {outcome}", entry_label(entry));
            team.user_replies.push(UserReply {
                at: now,
                entry_id: *id,
                text,
                held: true,
            });
        }
    }

    fn user_entry(
        &self,
        master: usize,
        id: Option<u64>,
    ) -> Result<&UserInboxEntry, (&'static str, String)> {
        let id = id.ok_or((
            "user_inbox_entry_required",
            "name exactly one entry".to_owned(),
        ))?;
        self.state.workspaces[master]
            .chat
            .team
            .user_inbox
            .iter()
            .find(|entry| entry.id == id)
            .ok_or((
                "user_inbox_entry_not_found",
                format!("entry {id} not found"),
            ))
    }

    /// `Enter` auf einem Formular oder Wizard: öffnet ein Pane neben dem
    /// Master. Nachrichten öffnet die TUI selbst.
    fn open_user_entry(
        &mut self,
        master: usize,
        id: Option<u64>,
    ) -> Result<(), (&'static str, String)> {
        let entry = self.user_entry(master, id)?.clone();
        let workspace = self.state.workspaces[master].id.clone();
        let herdr = herdr_command();
        let path = entry.path.clone().ok_or((
            "user_inbox_not_openable",
            "only formular and wizard entries open a pane".to_owned(),
        ))?;
        let result = |flag: &str| {
            format!(
                "{herdr} chat user-inbox-result {} --workspace {} {flag}",
                entry.id,
                shell_word(&workspace)
            )
        };
        let command = match entry.kind {
            UserInboxKind::Formular => format!(
                " f={file}; before=$(cksum < \"$f\"); nvim '+call search(\"^> \")' -- \"$f\"; \
                 if [ \"$(cksum < \"$f\")\" != \"$before\" ]; then {answered}; fi; exit",
                file = shell_word(&path),
                answered = result("--answered"),
            ),
            UserInboxKind::Wizard => format!(
                " bash {file}; code=$?; {done}; \
                 printf '\\n[collie] Wizard beendet, Exit-Code %s. Enter schließt das Pane. ' \"$code\"; \
                 read -r _; exit",
                file = shell_word(&path),
                done = result("--exit \"$code\""),
            ),
            _ => {
                return Err((
                    "user_inbox_not_openable",
                    "only formular and wizard entries open a pane".to_owned(),
                ));
            }
        };
        self.run_in_master_pane(master, &command, &entry.title)?;
        self.update_user_entries(master, &[entry.id], |entry| entry.read = true);
        Ok(())
    }

    /// `r`: Rückfrage zu einem Eintrag, geschrieben in nvim.
    fn open_user_reply(
        &mut self,
        master: usize,
        id: Option<u64>,
    ) -> Result<(), (&'static str, String)> {
        let entry = self.user_entry(master, id)?.clone();
        let workspace = self.state.workspaces[master].id.clone();
        let file = reply_file(entry.id).map_err(|err| ("user_inbox_reply_failed", err))?;
        std::fs::write(&file, format!("{REPLY_FILE_HEADER}{}\n\n", entry.title))
            .map_err(|err| ("user_inbox_reply_failed", err.to_string()))?;
        let file = file.to_string_lossy().into_owned();
        let command = format!(
            " nvim '+normal G' -- {file}; {herdr} chat user-inbox-result {id} --workspace {ws} --reply {file}; exit",
            file = shell_word(&file),
            herdr = herdr_command(),
            id = entry.id,
            ws = shell_word(&workspace),
        );
        self.run_in_master_pane(master, &command, "Rückfrage")
    }

    fn note_form_answered(
        &mut self,
        master: usize,
        id: Option<u64>,
        now: u64,
    ) -> Result<(), (&'static str, String)> {
        let entry = self.user_entry(master, id)?.clone();
        let path = entry.path.clone().unwrap_or_default();
        // Erledigte Formulare verschwinden aus der Inbox.
        self.update_user_entries(master, &[entry.id], |entry| {
            entry.finished = true;
            entry.read = true;
            entry.deleted = true;
        });
        self.push_user_reply(
            master,
            entry.id,
            format!(
                "{}: beantwortet, die Antworten stehen in {path}",
                entry_label(&entry)
            ),
            now,
        );
        Ok(())
    }

    fn note_wizard_result(
        &mut self,
        master: usize,
        id: Option<u64>,
        exit_code: i32,
        now: u64,
    ) -> Result<(), (&'static str, String)> {
        let entry = self.user_entry(master, id)?.clone();
        // Ein fehlgeschlagener Wizard bleibt offen, damit der Nutzer ihn nach
        // einer Korrektur erneut starten kann.
        let succeeded = exit_code == 0;
        // Ein durchgelaufener Wizard verschwindet aus der Inbox.
        self.update_user_entries(master, &[entry.id], |entry| {
            entry.finished = succeeded;
            entry.read = true;
            entry.deleted = succeeded;
        });
        let outcome = if succeeded {
            "durchgelaufen (Exit-Code 0)".to_owned()
        } else {
            format!("abgebrochen mit Exit-Code {exit_code}, bitte prüfen")
        };
        self.push_user_reply(
            master,
            entry.id,
            format!("{}: {outcome}", entry_label(&entry)),
            now,
        );
        Ok(())
    }

    fn note_reply_written(
        &mut self,
        master: usize,
        id: Option<u64>,
        path: Option<&str>,
        now: u64,
    ) -> Result<(), (&'static str, String)> {
        let entry = self.user_entry(master, id)?.clone();
        let path = path.ok_or((
            "user_inbox_path_required",
            "reply needs --reply FILE".to_owned(),
        ))?;
        let text = std::fs::read_to_string(path)
            .map_err(|err| ("user_inbox_reply_failed", err.to_string()))?;
        let _ = std::fs::remove_file(path);
        let text = reply_text(&text);
        if text.is_empty() {
            return Ok(());
        }
        self.push_user_reply(
            master,
            entry.id,
            format!("{}: Rückfrage: {text}", entry_label(&entry)),
            now,
        );
        Ok(())
    }

    fn push_user_reply(&mut self, master: usize, entry_id: u64, text: String, now: u64) {
        self.state.workspaces[master]
            .chat
            .team
            .user_replies
            .push(UserReply {
                at: now,
                entry_id,
                text,
                held: false,
            });
    }

    /// Teilt das Agent-Pane des Masters und tippt dort einen Befehl ein.
    fn run_in_master_pane(
        &mut self,
        master: usize,
        command: &str,
        label: &str,
    ) -> Result<(), (&'static str, String)> {
        let failed = |message: &str| ("user_inbox_pane_failed", message.to_owned());
        let agent = self
            .chat_agent_pane(master)
            .and_then(|pane| self.public_pane_id(master, pane))
            .ok_or_else(|| failed("master has no pane"))?;
        let split = self.handle_pane_split(
            "user-inbox:split".into(),
            PaneSplitParams {
                workspace_id: None,
                target_pane_id: Some(agent),
                direction: SplitDirection::Right,
                ratio: Some(0.5),
                cwd: self.chat_cwd(master),
                focus: true,
                right_click: Default::default(),
                env: Default::default(),
            },
        );
        let pane = serde_json::from_str::<serde_json::Value>(&split)
            .ok()
            .and_then(|response| {
                Some(
                    response
                        .pointer("/result/pane/pane_id")?
                        .as_str()?
                        .to_owned(),
                )
            })
            .ok_or_else(|| failed("could not split the master pane"))?;
        self.handle_pane_rename(
            "user-inbox:rename".into(),
            PaneRenameParams {
                pane_id: pane.clone(),
                label: Some(label.chars().take(40).collect()),
            },
        );
        self.handle_pane_send_input(
            "user-inbox:run".into(),
            PaneSendInputParams {
                pane_id: pane,
                text: command.to_owned(),
                keys: vec!["Enter".into()],
            },
        );
        Ok(())
    }

    fn user_replies_ready(&self, master: usize, last: u64, millis: u64) -> Option<bool> {
        if self.stopped_chat_agent(master).is_some() || !self.chat_agent_running(master) {
            return None;
        }
        if millis.saturating_sub(last) < REPLY_QUIET_MS {
            return Some(false);
        }
        let ws = &self.state.workspaces[master];
        if ws
            .chat
            .last_user_input
            .is_some_and(|typed| millis.saturating_sub(typed) < REPLY_TYPING_MS)
        {
            return Some(false);
        }
        let status = self.workspace_info(master).agent_status;
        Some(matches!(
            status,
            crate::api::schema::AgentStatus::Idle | crate::api::schema::AgentStatus::Done
        ))
    }

    fn submit_user_replies(&mut self, master: usize) -> bool {
        let Some(pane_id) = self.chat_agent_pane(master) else {
            return false;
        };
        let lines = self.state.workspaces[master]
            .chat
            .team
            .deliverable_user_replies()
            .map(|reply| format!("- {}", reply.text))
            .collect::<Vec<_>>();
        if lines.is_empty() || !self.submit_to_pane(master, pane_id, &user_reply_prompt(&lines)) {
            return false;
        }
        self.state.workspaces[master]
            .chat
            .team
            .user_replies
            .retain(|reply| reply.held);
        self.schedule_session_save();
        true
    }
}

fn single(ids: &[u64]) -> Option<u64> {
    match ids {
        [id] => Some(*id),
        _ => None,
    }
}

/// `#3 formular „Titel“`
fn entry_label(entry: &UserInboxEntry) -> String {
    format!("#{} {} „{}“", entry.id, entry.kind.as_str(), entry.title)
}

fn user_reply_prompt(lines: &[String]) -> String {
    format!(
        "{USER_REPLY_HEADER} Rückmeldungen aus meiner Inbox:\n{}",
        lines.join("\n")
    )
}

/// Erste nicht leere Zeile, ohne Markdown-Zeichen am Anfang, gekürzt.
fn one_line(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['#', '>', '-', '*', ' '])
                .trim()
        })
        .find(|line| !line.is_empty())?;
    let mut title = line.chars().take(MAX_TITLE_CHARS).collect::<String>();
    if line.chars().count() > MAX_TITLE_CHARS {
        title.push('…');
    }
    Some(title)
}

/// Text einer Rückfrage-Datei ohne die Kopfzeile.
fn reply_text(content: &str) -> String {
    content
        .lines()
        .filter(|line| !line.starts_with(REPLY_FILE_HEADER))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .chars()
        .take(MAX_REPLY_CHARS)
        .collect()
}

fn absolute_file(path: &str) -> Option<String> {
    let path = std::fs::canonicalize(path).ok()?;
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

fn reply_file(id: u64) -> Result<PathBuf, String> {
    let dir = crate::session::data_dir().join("user-inbox");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(dir.join(format!(
        "rueckfrage-{id}-{}.md",
        crate::workspace::unix_millis_now()
    )))
}

/// Pfad des laufenden Binaries für Befehle in neuen Panes. Nach einem
/// Neubau ist es gelöscht; dann gilt `herdr` aus dem `PATH`.
fn herdr_command() -> String {
    std::env::current_exe()
        .ok()
        .filter(|exe| exe.is_file())
        .map(|exe| shell_word(&exe.to_string_lossy()))
        .unwrap_or_else(|| "herdr".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{Method, Request};
    use crate::workspace::ChatRole;

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
            crate::workspace::Workspace::test_new("plain"),
        ];
        app.state.workspaces[0].chat.team.role = ChatRole::Master;
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

    fn tell(app: &mut App, kind: &str, text: &str, path: Option<String>) -> serde_json::Value {
        let workspace_id = app.state.workspaces[0].id.clone();
        call(
            app,
            Method::ChatTellUser(ChatTellUserParams {
                workspace_id: Some(workspace_id),
                kind: kind.into(),
                important: true,
                text: text.into(),
                path,
                ..Default::default()
            }),
        )
    }

    fn act(app: &mut App, action: ChatUserInboxAction, ids: Vec<u64>) -> serde_json::Value {
        let workspace_id = app.state.workspaces[0].id.clone();
        call(
            app,
            Method::ChatUserInboxAct(ChatUserInboxActParams {
                workspace_id: Some(workspace_id),
                caller_pane_id: None,
                action,
                ids,
                exit_code: None,
                path: None,
                all: false,
            }),
        )
    }

    #[test]
    fn tell_user_checks_kind_target_and_file() {
        let mut app = test_app();
        let created = tell(&mut app, "nachricht", "## Child fertig\nDetails", None);
        assert_eq!(created["result"]["user_inbox"][0]["title"], "Child fertig");
        assert_eq!(created["result"]["user_inbox"][0]["kind"], "nachricht");
        assert_eq!(
            tell(&mut app, "antwort", "x", None)["error"]["code"],
            "invalid_user_inbox_kind"
        );
        assert_eq!(
            tell(&mut app, "formular", "x", None)["error"]["code"],
            "user_inbox_path_required"
        );
        assert_eq!(
            tell(&mut app, "wizard", "x", Some("/does/not/exist".into()))["error"]["code"],
            "user_inbox_file_not_found"
        );
        // Auch ein Chat ohne Team schreibt in die Inbox; die globale Inbox
        // zeigt beide Chats.
        let plain = app.state.workspaces[1].id.clone();
        let other = call(
            &mut app,
            Method::ChatTellUser(ChatTellUserParams {
                workspace_id: Some(plain.clone()),
                kind: "nachricht".into(),
                text: "x".into(),
                ..Default::default()
            }),
        );
        assert_eq!(
            other["result"]["user_inbox"][0]["workspace_id"],
            plain.as_str()
        );
        assert_eq!(app.chat_team_info(0).unwrap().user_inbox_important, 1);
        let all = call(
            &mut app,
            Method::ChatUserInbox(ChatUserInboxParams {
                all: true,
                ..Default::default()
            }),
        );
        let entries = all["result"]["user_inbox"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry["master"] == true));
        assert!(entries.iter().any(|entry| entry["master"] == false));
        assert_eq!(app.global_user_inbox().0 + app.global_user_inbox().1, 2);
    }

    #[test]
    fn done_entries_report_to_the_master_once_the_user_leaves() {
        let mut app = test_app();
        tell(&mut app, "nachricht", "A", None);
        let file = std::env::temp_dir().join(format!("collie-form-{}.md", std::process::id()));
        std::fs::write(&file, "# F\n").unwrap();
        tell(
            &mut app,
            "formular",
            "B",
            Some(file.to_string_lossy().into_owned()),
        );
        let done = act(&mut app, ChatUserInboxAction::Done, vec![1, 2]);
        assert_eq!(done["result"]["user_inbox"].as_array().unwrap().len(), 0);
        let team = &app.state.workspaces[0].chat.team;
        let texts = team
            .user_replies
            .iter()
            .map(|reply| reply.text.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            texts,
            vec![
                "#1 nachricht „A“: abgearbeitet".to_owned(),
                "#2 formular „B“: verworfen, ohne Antwort gelöscht".to_owned(),
            ]
        );
        assert!(team.user_replies.iter().all(|reply| reply.held));
        assert_eq!(team.deliverable_user_replies().count(), 0);

        // `u` holt einen Eintrag zurück; seine Meldung fällt weg.
        let restored = act(&mut app, ChatUserInboxAction::Restore, vec![2]);
        assert_eq!(restored["result"]["user_inbox"][0]["id"], 2);
        assert_eq!(app.state.workspaces[0].chat.team.user_replies.len(), 1);

        act(&mut app, ChatUserInboxAction::Flush, Vec::new());
        assert_eq!(
            app.state.workspaces[0]
                .chat
                .team
                .deliverable_user_replies()
                .count(),
            1
        );

        act(&mut app, ChatUserInboxAction::Answered, vec![2]);
        let team = &app.state.workspaces[0].chat.team;
        assert!(team
            .user_inbox
            .iter()
            .any(|entry| entry.id == 2 && entry.finished && entry.deleted));
        assert!(team
            .user_replies
            .last()
            .unwrap()
            .text
            .contains("beantwortet"));
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_failed_wizard_stays_open() {
        let mut app = test_app();
        let file = std::env::temp_dir().join(format!("collie-wizard-{}.sh", std::process::id()));
        std::fs::write(&file, "exit 1\n").unwrap();
        tell(
            &mut app,
            "wizard",
            "Setup",
            Some(file.to_string_lossy().into_owned()),
        );
        let workspace_id = app.state.workspaces[0].id.clone();
        let result = |code| {
            Method::ChatUserInboxAct(ChatUserInboxActParams {
                workspace_id: Some(workspace_id.clone()),
                caller_pane_id: None,
                action: ChatUserInboxAction::WizardResult,
                ids: vec![1],
                exit_code: Some(code),
                path: None,
                all: false,
            })
        };
        call(&mut app, result(2));
        assert!(!app.state.workspaces[0].chat.team.user_inbox[0].finished);
        assert!(!app.state.workspaces[0].chat.team.user_inbox[0].deleted);
        assert!(app.state.workspaces[0].chat.team.user_replies[0]
            .text
            .contains("Exit-Code 2"));
        // Ein durchgelaufener Wizard verschwindet.
        let done = call(&mut app, result(0));
        assert!(app.state.workspaces[0].chat.team.user_inbox[0].finished);
        assert_eq!(done["result"]["user_inbox"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn only_answers_to_user_prompts_of_a_master_land_in_the_inbox() {
        let mut app = test_app();
        let answer = |app: &mut App, index, prompt: &str, key: &str| {
            app.record_master_answer(
                index,
                Some("## Stand\nAlles gut.".into()),
                Some(prompt.into()),
                Some(key.into()),
            );
        };
        answer(&mut app, 0, "[collie inbox]\n- w2 Child (done)", "k1");
        answer(&mut app, 0, "[collie user] Rückmeldungen", "k2");
        answer(&mut app, 1, "Was ist der Stand?", "k3");
        assert!(app.state.workspaces[0].chat.team.user_inbox.is_empty());
        assert!(app.state.workspaces[1].chat.team.user_inbox.is_empty());
        answer(&mut app, 0, "Was ist seit gestern passiert?", "k4");
        answer(&mut app, 0, "Was ist seit gestern passiert?", "k4");
        let inbox = &app.state.workspaces[0].chat.team.user_inbox;
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].kind, UserInboxKind::Antwort);
        assert!(inbox[0].important);
        assert_eq!(inbox[0].title, "Was ist seit gestern passiert?");
    }

    #[test]
    fn one_line_strips_markdown_and_shortens() {
        assert_eq!(
            one_line("\n## Zusammenfassung\nText").as_deref(),
            Some("Zusammenfassung")
        );
        let long = "x".repeat(100);
        let title = one_line(&long).unwrap();
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS + 1);
        assert!(title.ends_with('…'));
        assert_eq!(one_line("  \n "), None);
    }

    #[test]
    fn reply_text_drops_the_header() {
        assert_eq!(
            reply_text("# Rückfrage zu: Deploy\n\nSchritt 3 schlägt fehl.\n"),
            "Schritt 3 schlägt fehl."
        );
        assert_eq!(reply_text("# Rückfrage zu: Deploy\n\n"), "");
    }

    #[test]
    fn reply_prompt_starts_with_the_collie_prefix() {
        let prompt = user_reply_prompt(&["- #1 nachricht „A“: abgearbeitet".into()]);
        assert!(prompt.starts_with(COLLIE_PROMPT_PREFIX));
        assert!(prompt.ends_with("abgearbeitet"));
    }
}
