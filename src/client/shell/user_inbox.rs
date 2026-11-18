//! collie: Overlay der Nutzer-Inbox eines Master Chats, siehe
//! docs/specs/user-inbox.md. Die Einträge lädt die TUI per
//! `chat.user_inbox`, der Snapshot trägt nur Zähler und Revision.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use super::chat_cards::{age_color, age_label, ICON_CROWN};
use super::chat_telescope::{frame, telescope_rect, TelescopeRender};
use super::render::{display_width, put_text};
use super::*;
use crate::api::schema::{ChatUserInboxAction, UserInboxEntryInfo};
use crate::ui::truncate_end;

/// Symbol der Nutzer-Inbox an der Master-Karte und im Titel.
pub(super) const ICON_INBOX: &str = "󰇮";
/// Zeilen für den Text unter der Liste.
const MIN_DETAIL_LINES: u16 = 6;

#[derive(Debug)]
pub(super) struct ClientUserInbox {
    pub(super) master_workspace_id: String,
    /// Agent-Pane des Masters; Formulare und Wizards öffnen daneben.
    pub(super) master_pane_id: String,
    /// Wichtig zuerst, dann optional, jeweils neueste zuerst.
    pub(super) entries: Vec<UserInboxEntryInfo>,
    pub(super) loaded: bool,
    /// Revision der geladenen Einträge.
    pub(super) revision: u64,
    /// Revision, für die gerade eine Anfrage läuft.
    pub(super) requested: Option<u64>,
    /// Eintrag unter dem Cursor.
    pub(super) selected: Option<u64>,
    /// Mit `space` oder `m` markierte Einträge.
    pub(super) marked: Vec<u64>,
    /// Das erste `d` ist gedrückt.
    pub(super) pending_delete: bool,
    /// Gelöschte Einträge je `dd`, für `u`.
    pub(super) undo: Vec<Vec<u64>>,
    /// Der Text des Eintrags füllt das ganze Overlay.
    pub(super) reading: bool,
    pub(super) detail_scroll: usize,
    pub(super) scroll: usize,
}

fn sort_entries(entries: &mut [UserInboxEntryInfo]) {
    entries.sort_by(|a, b| b.important.cmp(&a.important).then(b.at.cmp(&a.at)));
}

fn selected_index(inbox: &ClientUserInbox) -> usize {
    inbox
        .selected
        .and_then(|id| inbox.entries.iter().position(|entry| entry.id == id))
        .unwrap_or(0)
}

/// Gelesen heißt bei Nachrichten geöffnet, bei Aufgaben erledigt.
fn pending(entry: &UserInboxEntryInfo) -> bool {
    if matches!(entry.kind.as_str(), "formular" | "wizard") {
        !entry.finished
    } else {
        !entry.read
    }
}

fn kind_symbol(kind: &str) -> &'static str {
    match kind {
        "antwort" => "↩",
        // Nerd-Font-Symbole wie Krone und Inbox: Checkliste und Zahnrad.
        "formular" => "\u{f0135}",
        "wizard" => "\u{f0493}",
        _ => "✉",
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "antwort" => "Antwort auf deinen Prompt",
        "formular" => "Formular",
        "wizard" => "Wizard",
        _ => "Nachricht",
    }
}

/// Zeilen des Texts, jede auf `width` umbrochen. Leerzeilen bleiben.
fn text_lines(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in line.split(' ') {
            let candidate = if current.is_empty() {
                word.to_owned()
            } else {
                format!("{current} {word}")
            };
            if display_width(&candidate) as usize <= width || current.is_empty() {
                current = candidate;
            } else {
                lines.push(truncate_end(&std::mem::take(&mut current), width));
                current = word.to_owned();
            }
        }
        lines.push(truncate_end(&current, width));
    }
    lines
}

pub(super) fn render_user_inbox(
    buffer: &mut Buffer,
    inbox: &ClientUserInbox,
    config: &ClientShellConfig,
) -> Option<TelescopeRender> {
    let palette = &config.palette;
    let now = crate::workspace::unix_millis_now();
    let rect = telescope_rect(buffer.area);
    let inner = frame(buffer, rect, palette)?;
    let base = Style::default().bg(palette.panel_bg);
    let selected = selected_index(inbox);

    let open = inbox.entries.iter().filter(|entry| pending(entry)).count();
    let title = format!(" {ICON_INBOX} Inbox · {ICON_CROWN} Master Chat · {open} offen ");
    put_text(
        buffer,
        rect.x + 2,
        rect.y,
        rect.width.saturating_sub(4),
        &title,
        base.fg(palette.yellow).add_modifier(Modifier::BOLD),
    );

    let footer_y = inner.bottom().saturating_sub(1);
    let footer = if inbox.pending_delete {
        (
            "d erledigt die markierten Einträge · jede andere Taste bricht ab".to_owned(),
            base.fg(palette.red).add_modifier(Modifier::BOLD),
        )
    } else if inbox.reading {
        (
            "j/k scrollen · enter/esc zur Liste · r Rückfrage · dd erledigt".to_owned(),
            base.fg(palette.overlay0),
        )
    } else {
        (
            "enter öffnen · space markieren · dd erledigt · u zurück · r Rückfrage · esc"
                .to_owned(),
            base.fg(palette.overlay0),
        )
    };
    put_text(buffer, inner.x, footer_y, inner.width, &footer.0, footer.1);

    let mut menu_rows = Vec::new();
    if !inbox.loaded || inbox.entries.is_empty() {
        let text = if inbox.loaded {
            "Nichts in der Inbox. Der Master legt Einträge mit `herdr chat tell-user` ab."
        } else {
            "lädt …"
        };
        put_text(
            buffer,
            inner.x,
            inner.y.saturating_add(1),
            inner.width,
            text,
            base.fg(palette.overlay0).add_modifier(Modifier::ITALIC),
        );
        return Some(TelescopeRender {
            area: rect,
            menu_rows,
        });
    }

    // Liste oben, Text unten. Beim Lesen füllt der Text alles.
    let body_bottom = footer_y.saturating_sub(1);
    let list_height = if inbox.reading {
        0
    } else {
        let wanted = inbox.entries.len() as u16 + 2;
        let room = body_bottom.saturating_sub(inner.y);
        wanted.min(room.saturating_sub(MIN_DETAIL_LINES + 1).max(room / 2))
    };
    let list = Rect::new(inner.x, inner.y, inner.width, list_height);
    if list.height > 0 {
        render_list(buffer, list, inbox, selected, now, palette, &mut menu_rows);
    }

    let detail_top = if inbox.reading {
        inner.y
    } else {
        for x in inner.x..inner.right() {
            put_text(buffer, x, list.bottom(), 1, "─", base.fg(palette.surface1));
        }
        list.bottom().saturating_add(1)
    };
    if let Some(entry) = inbox.entries.get(selected) {
        let detail = Rect::new(
            inner.x,
            detail_top,
            inner.width,
            body_bottom.saturating_sub(detail_top),
        );
        render_detail(buffer, detail, entry, inbox.detail_scroll, palette);
    }

    Some(TelescopeRender {
        area: rect,
        menu_rows,
    })
}

fn render_list(
    buffer: &mut Buffer,
    list: Rect,
    inbox: &ClientUserInbox,
    selected: usize,
    now: u64,
    palette: &Palette,
    menu_rows: &mut Vec<(Rect, usize)>,
) {
    let base = Style::default().bg(palette.panel_bg);
    // Zeilen: Kopfzeile je Gruppe, dann die Einträge.
    let mut lines: Vec<Option<usize>> = Vec::new();
    let mut group = None;
    for (index, entry) in inbox.entries.iter().enumerate() {
        if group != Some(entry.important) {
            group = Some(entry.important);
            lines.push(None);
        }
        lines.push(Some(index));
    }
    let selected_line = lines
        .iter()
        .position(|line| *line == Some(selected))
        .unwrap_or(0);
    let visible = usize::from(list.height);
    let mut scroll = inbox.scroll.min(lines.len().saturating_sub(1));
    if selected_line < scroll {
        scroll = selected_line.saturating_sub(1);
    } else if visible > 0 && selected_line >= scroll + visible {
        scroll = selected_line + 1 - visible;
    }
    let important = inbox.entries.iter().filter(|entry| entry.important).count();
    let optional = inbox.entries.len() - important;
    for (offset, line) in lines.iter().enumerate().skip(scroll).take(visible) {
        let y = list.y + (offset - scroll) as u16;
        let rect = Rect::new(list.x, y, list.width, 1);
        let Some(index) = *line else {
            let next = lines.get(offset + 1).copied().flatten();
            let is_important = next.is_some_and(|index| inbox.entries[index].important);
            let (label, color) = if is_important {
                (format!("Wichtig ({important})"), palette.red)
            } else {
                (format!("Optional ({optional})"), palette.overlay0)
            };
            put_text(
                buffer,
                rect.x,
                y,
                rect.width,
                &label,
                base.fg(color).add_modifier(Modifier::BOLD),
            );
            continue;
        };
        let entry = &inbox.entries[index];
        let is_selected = index == selected;
        let deleting = inbox.pending_delete
            && (inbox.marked.contains(&entry.id) || (inbox.marked.is_empty() && is_selected));
        let row_style = if deleting {
            base.bg(palette.surface0)
                .fg(palette.red)
                .add_modifier(Modifier::CROSSED_OUT)
        } else if is_selected {
            base.bg(palette.surface0)
        } else {
            base
        };
        buffer.set_style(rect, row_style);
        let mark = if inbox.marked.contains(&entry.id) {
            "▌"
        } else {
            " "
        };
        put_text(buffer, rect.x, y, 1, mark, row_style.fg(palette.green));
        let (dot, dot_color) = if pending(entry) {
            (
                "●",
                if entry.important {
                    palette.red
                } else {
                    palette.accent
                },
            )
        } else if matches!(entry.kind.as_str(), "formular" | "wizard") {
            ("✓", palette.green)
        } else {
            ("○", palette.overlay0)
        };
        put_text(buffer, rect.x + 2, y, 1, dot, row_style.fg(dot_color));
        put_text(
            buffer,
            rect.x + 4,
            y,
            1,
            kind_symbol(&entry.kind),
            row_style.fg(palette.subtext0),
        );
        let age = age_label(entry.at, now);
        let age_width = display_width(&age);
        let right = rect.right().saturating_sub(1);
        put_text(
            buffer,
            right.saturating_sub(age_width),
            y,
            age_width,
            &age,
            row_style.fg(age_color(entry.at, now, palette)),
        );
        let title_x = rect.x + 6;
        let title_width = right.saturating_sub(age_width + 1).saturating_sub(title_x);
        let title_style = if pending(entry) && !deleting {
            row_style.fg(palette.text).add_modifier(Modifier::BOLD)
        } else if deleting {
            row_style
        } else {
            row_style.fg(palette.subtext0)
        };
        put_text(
            buffer,
            title_x,
            y,
            title_width,
            &truncate_end(&entry.title, usize::from(title_width)),
            title_style,
        );
        menu_rows.push((rect, index));
    }
}

fn render_detail(
    buffer: &mut Buffer,
    rect: Rect,
    entry: &UserInboxEntryInfo,
    scroll: usize,
    palette: &Palette,
) {
    if rect.height == 0 {
        return;
    }
    let base = Style::default().bg(palette.panel_bg);
    let mut header = kind_label(&entry.kind).to_owned();
    if let Some(path) = entry.path.as_deref() {
        header.push_str(" · ");
        header.push_str(path);
    }
    let hint = match entry.kind.as_str() {
        "formular" if entry.finished => "beantwortet, enter öffnet erneut",
        "formular" => "enter öffnet das Formular in nvim",
        "wizard" if entry.finished => "durchgelaufen, enter startet erneut",
        "wizard" => "enter startet den Wizard",
        _ => "",
    };
    if !hint.is_empty() {
        header.push_str(" · ");
        header.push_str(hint);
    }
    put_text(
        buffer,
        rect.x,
        rect.y,
        rect.width,
        &truncate_end(&header, usize::from(rect.width)),
        base.fg(palette.overlay0).add_modifier(Modifier::BOLD),
    );
    let lines = text_lines(&entry.text, usize::from(rect.width));
    let room = usize::from(rect.height.saturating_sub(1));
    let scroll = scroll.min(lines.len().saturating_sub(room));
    for (offset, line) in lines.iter().skip(scroll).take(room).enumerate() {
        let style = if line.starts_with('#') {
            base.fg(palette.accent).add_modifier(Modifier::BOLD)
        } else if line.starts_with('|') {
            base.fg(palette.subtext0)
        } else {
            base.fg(palette.text)
        };
        put_text(
            buffer,
            rect.x,
            rect.y + 1 + offset as u16,
            rect.width,
            line,
            style,
        );
    }
    if scroll + room < lines.len() {
        let more = format!(" ↓ {} Zeilen ", lines.len() - scroll - room);
        let width = display_width(&more);
        put_text(
            buffer,
            rect.right().saturating_sub(width),
            rect.bottom().saturating_sub(1),
            width,
            &more,
            base.fg(palette.overlay0),
        );
    }
}

impl ClientShellState {
    /// Öffnet die Nutzer-Inbox eines Masters und lädt ihre Einträge.
    pub(super) fn open_user_inbox(
        &mut self,
        master_workspace_id: String,
        outcome: &mut ClientShellInput,
    ) {
        let Some(master) = self.snapshot.as_deref().and_then(|snapshot| {
            super::chats::all_chat_rows(snapshot)
                .into_iter()
                .find(|row| row.master && row.workspace_id == master_workspace_id)
        }) else {
            return;
        };
        self.last_master_workspace = Some(master_workspace_id.clone());
        if self.mode == ClientShellMode::Navigate {
            self.leave_sidebar(false, outcome);
        }
        self.overlay = Some(ClientShellOverlay::UserInbox(ClientUserInbox {
            master_workspace_id,
            master_pane_id: master.pane_id,
            entries: Vec::new(),
            loaded: false,
            revision: master.user_inbox.revision,
            requested: None,
            selected: None,
            marked: Vec::new(),
            pending_delete: false,
            undo: Vec::new(),
            reading: false,
            detail_scroll: 0,
            scroll: 0,
        }));
        self.request_user_inbox(outcome);
        outcome.repaint = true;
    }

    /// `prefix+i`: Inbox des fokussierten Masters, sonst des zuletzt
    /// geöffneten oder ersten Masters.
    pub(super) fn open_focused_user_inbox(&mut self, outcome: &mut ClientShellInput) -> bool {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return false;
        };
        let rows = super::chats::all_chat_rows(snapshot);
        let focused = rows.iter().find(|row| row.focused);
        let master = match focused {
            Some(row) if row.master => Some(row.workspace_id.clone()),
            Some(row) if row.parent.is_some() => row.parent.clone(),
            _ => self
                .last_master_workspace
                .clone()
                .filter(|last| {
                    rows.iter()
                        .any(|row| row.master && &row.workspace_id == last)
                })
                .or_else(|| {
                    rows.iter()
                        .find(|row| row.master)
                        .map(|row| row.workspace_id.clone())
                }),
        };
        let Some(master) = master else {
            return false;
        };
        self.open_user_inbox(master, outcome);
        true
    }

    fn request_user_inbox(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.as_mut() else {
            return;
        };
        let revision = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| master_revision(snapshot, &inbox.master_workspace_id))
            .unwrap_or(inbox.revision);
        inbox.requested = Some(revision);
        let method =
            crate::api::schema::Method::ChatUserInbox(crate::api::schema::ChatUserInboxParams {
                workspace_id: Some(inbox.master_workspace_id.clone()),
                caller_pane_id: None,
            });
        let workspace_id = inbox.master_workspace_id.clone();
        self.push_endpoint_method_with_kind(
            method,
            PendingEndpointKind::UserInbox {
                workspace_id,
                revision,
            },
            outcome,
        );
    }

    fn act_on_user_inbox(
        &mut self,
        action: ChatUserInboxAction,
        ids: Vec<u64>,
        outcome: &mut ClientShellInput,
    ) {
        let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.as_ref() else {
            return;
        };
        let workspace_id = inbox.master_workspace_id.clone();
        self.push_user_inbox_act(workspace_id, action, ids, outcome);
    }

    fn push_user_inbox_act(
        &mut self,
        workspace_id: String,
        action: ChatUserInboxAction,
        ids: Vec<u64>,
        outcome: &mut ClientShellInput,
    ) {
        let revision = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| master_revision(snapshot, &workspace_id))
            .unwrap_or_default();
        let method = crate::api::schema::Method::ChatUserInboxAct(
            crate::api::schema::ChatUserInboxActParams {
                workspace_id: Some(workspace_id.clone()),
                caller_pane_id: None,
                action,
                ids,
                exit_code: None,
                path: None,
            },
        );
        self.push_endpoint_method_with_kind(
            method,
            PendingEndpointKind::UserInbox {
                workspace_id,
                revision,
            },
            outcome,
        );
    }

    /// Antwort auf `chat.user_inbox` oder `chat.user_inbox_act`.
    pub(super) fn complete_user_inbox(
        &mut self,
        workspace_id: String,
        revision: u64,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> bool {
        let Ok(crate::api::schema::ResponseResult::ChatUserInbox { mut user_inbox }) = result
        else {
            return false;
        };
        let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.as_mut() else {
            return false;
        };
        if inbox.master_workspace_id != workspace_id {
            return false;
        }
        sort_entries(&mut user_inbox);
        // Der Cursor bleibt auf seinem Eintrag; ist der weg, auf derselben
        // Position.
        let position = selected_index(inbox);
        inbox.entries = user_inbox;
        if inbox
            .selected
            .is_none_or(|id| !inbox.entries.iter().any(|entry| entry.id == id))
        {
            inbox.selected = inbox
                .entries
                .get(position.min(inbox.entries.len().saturating_sub(1)))
                .map(|entry| entry.id);
            inbox.detail_scroll = 0;
        }
        inbox
            .marked
            .retain(|id| inbox.entries.iter().any(|entry| entry.id == *id));
        inbox.loaded = true;
        inbox.revision = revision;
        if inbox.requested == Some(revision) {
            inbox.requested = None;
        }
        true
    }

    /// Lädt die Inbox neu, wenn sich die Revision im Snapshot geändert hat,
    /// etwa weil der Master etwas abgelegt hat.
    pub(crate) fn tick_user_inbox(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.as_ref() else {
            return;
        };
        let Some(current) = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| master_revision(snapshot, &inbox.master_workspace_id))
        else {
            // Der Master ist weg.
            self.overlay = None;
            outcome.repaint = true;
            return;
        };
        if current != inbox.revision && inbox.requested != Some(current) {
            self.request_user_inbox(outcome);
        }
    }

    /// Schließt das Overlay. Zurückgehaltene Rückmeldungen gehen jetzt an
    /// den Master.
    fn close_user_inbox(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.take() else {
            return;
        };
        self.push_user_inbox_act(
            inbox.master_workspace_id,
            ChatUserInboxAction::Flush,
            Vec::new(),
            outcome,
        );
        outcome.repaint = true;
    }

    /// Formular, Wizard oder Rückfrage öffnen ein Pane neben dem Master. Das
    /// Overlay schließt und der Master-Space kommt nach vorn.
    fn open_user_inbox_pane(
        &mut self,
        action: ChatUserInboxAction,
        id: u64,
        outcome: &mut ClientShellInput,
    ) {
        let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.as_ref() else {
            return;
        };
        let master_pane = inbox.master_pane_id.clone();
        self.act_on_user_inbox(action, vec![id], outcome);
        self.close_user_inbox(outcome);
        self.focus_or_activate(
            self.active_endpoint_id.clone(),
            ClientEndpointFocusTarget::Pane(master_pane),
            outcome,
        );
    }

    pub(super) fn route_user_inbox_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.as_mut() else {
            return false;
        };
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        outcome.repaint = true;
        let index = selected_index(inbox);
        let current = inbox.entries.get(index).cloned();
        let last = inbox.entries.len().saturating_sub(1);
        let ctrl = modifiers == KeyModifiers::CONTROL;

        if inbox.pending_delete {
            inbox.pending_delete = false;
            if code == KeyCode::Char('d') && !ctrl {
                let ids = if inbox.marked.is_empty() {
                    current.iter().map(|entry| entry.id).collect::<Vec<_>>()
                } else {
                    std::mem::take(&mut inbox.marked)
                };
                if ids.is_empty() {
                    return true;
                }
                inbox.entries.retain(|entry| !ids.contains(&entry.id));
                inbox.selected = inbox
                    .entries
                    .get(index.min(inbox.entries.len().saturating_sub(1)))
                    .map(|entry| entry.id);
                inbox.detail_scroll = 0;
                inbox.reading = false;
                inbox.undo.push(ids.clone());
                self.act_on_user_inbox(ChatUserInboxAction::Done, ids, outcome);
            }
            return true;
        }

        let select = |inbox: &mut ClientUserInbox, index: usize| {
            let id = inbox.entries.get(index).map(|entry| entry.id);
            if id != inbox.selected {
                inbox.selected = id;
                inbox.detail_scroll = 0;
            }
        };
        match code {
            KeyCode::Esc | KeyCode::Char('q') if inbox.reading => inbox.reading = false,
            KeyCode::Esc | KeyCode::Char('q') => self.close_user_inbox(outcome),
            KeyCode::Down | KeyCode::Char('j') if inbox.reading => inbox.detail_scroll += 1,
            KeyCode::Up | KeyCode::Char('k') if inbox.reading => {
                inbox.detail_scroll = inbox.detail_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => select(inbox, (index + 1).min(last)),
            KeyCode::Up | KeyCode::Char('k') => select(inbox, index.saturating_sub(1)),
            KeyCode::Char('n') if ctrl => select(inbox, (index + 1).min(last)),
            KeyCode::Char('p') if ctrl => select(inbox, index.saturating_sub(1)),
            KeyCode::Char('d') if ctrl => inbox.detail_scroll += 10,
            KeyCode::Char('u') if ctrl => {
                inbox.detail_scroll = inbox.detail_scroll.saturating_sub(10);
            }
            KeyCode::PageDown => inbox.detail_scroll += 10,
            KeyCode::PageUp => inbox.detail_scroll = inbox.detail_scroll.saturating_sub(10),
            KeyCode::Char('g') => select(inbox, 0),
            KeyCode::Char('G') => select(inbox, last),
            KeyCode::Char(' ' | 'm') if !inbox.reading => {
                if let Some(entry) = current.as_ref() {
                    if let Some(position) = inbox.marked.iter().position(|id| *id == entry.id) {
                        inbox.marked.remove(position);
                    } else {
                        inbox.marked.push(entry.id);
                    }
                    select(inbox, (index + 1).min(last));
                }
            }
            KeyCode::Char('d') if current.is_some() || !inbox.marked.is_empty() => {
                inbox.pending_delete = true;
            }
            KeyCode::Char('u') => {
                if let Some(ids) = inbox.undo.pop() {
                    self.act_on_user_inbox(ChatUserInboxAction::Restore, ids, outcome);
                }
            }
            KeyCode::Char('r') => {
                if let Some(entry) = current {
                    self.open_user_inbox_pane(ChatUserInboxAction::Reply, entry.id, outcome);
                }
            }
            KeyCode::Enter => {
                let Some(entry) = current else {
                    return true;
                };
                if matches!(entry.kind.as_str(), "formular" | "wizard") {
                    self.open_user_inbox_pane(ChatUserInboxAction::Open, entry.id, outcome);
                } else if inbox.reading {
                    inbox.reading = false;
                } else {
                    inbox.reading = true;
                    if !entry.read {
                        if let Some(local) =
                            inbox.entries.iter_mut().find(|local| local.id == entry.id)
                        {
                            local.read = true;
                        }
                        self.act_on_user_inbox(ChatUserInboxAction::Read, vec![entry.id], outcome);
                    }
                }
            }
            _ => {}
        }
        true
    }

    /// Maus im Overlay: Klick wählt, Klick auf den gewählten Eintrag öffnet,
    /// Klick daneben schließt.
    pub(super) fn route_user_inbox_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let point = (mouse.column, mouse.row);
        let hit = self
            .hits
            .telescope_rows
            .iter()
            .find(|(rect, _)| super::contains(*rect, point))
            .map(|(_, index)| *index);
        let area = self.hits.user_inbox_area;
        let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.as_mut() else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(index) = hit else {
                    if !super::contains(area, point) {
                        self.close_user_inbox(outcome);
                    }
                    return true;
                };
                let id = inbox.entries.get(index).map(|entry| entry.id);
                if id.is_some() && id == inbox.selected {
                    let enter = crate::input::TerminalKey::new(KeyCode::Enter, KeyModifiers::NONE);
                    self.route_user_inbox_key(&enter, outcome);
                } else {
                    inbox.selected = id;
                    inbox.detail_scroll = 0;
                    outcome.repaint = true;
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let down = mouse.kind == MouseEventKind::ScrollDown;
                if inbox.reading {
                    inbox.detail_scroll = if down {
                        inbox.detail_scroll + 3
                    } else {
                        inbox.detail_scroll.saturating_sub(3)
                    };
                } else {
                    let index = selected_index(inbox);
                    let next = if down {
                        (index + 1).min(inbox.entries.len().saturating_sub(1))
                    } else {
                        index.saturating_sub(1)
                    };
                    inbox.selected = inbox.entries.get(next).map(|entry| entry.id);
                    inbox.detail_scroll = 0;
                }
                outcome.repaint = true;
            }
            _ => {}
        }
        true
    }
}

fn master_revision(snapshot: &ClientShellSnapshot, workspace_id: &str) -> Option<u64> {
    snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == workspace_id)
        .and_then(|workspace| workspace.team.as_ref())
        .filter(|team| team.master)
        .map(|team| team.user_inbox_revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64, important: bool, at: u64) -> UserInboxEntryInfo {
        UserInboxEntryInfo {
            id,
            at,
            kind: "nachricht".into(),
            important,
            title: format!("#{id}"),
            text: String::new(),
            path: None,
            read: false,
            finished: false,
        }
    }

    fn inbox_with(entries: Vec<UserInboxEntryInfo>) -> ClientUserInbox {
        ClientUserInbox {
            master_workspace_id: "w1".into(),
            master_pane_id: "w1:p1".into(),
            selected: entries.first().map(|entry| entry.id),
            entries,
            loaded: true,
            revision: 0,
            requested: None,
            marked: Vec::new(),
            pending_delete: false,
            undo: Vec::new(),
            reading: false,
            detail_scroll: 0,
            scroll: 0,
        }
    }

    fn screen(inbox: &ClientUserInbox) -> String {
        let config = ClientShellConfig::from_config(&crate::config::Config::default());
        let mut buffer = Buffer::empty(Rect::new(0, 0, 90, 24));
        render_user_inbox(&mut buffer, inbox, &config).expect("rendered");
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn overlay_groups_entries_and_shows_the_selected_text() {
        let mut answer = entry(1, true, 2);
        answer.kind = "antwort".into();
        answer.title = "Was ist seit gestern passiert?".into();
        answer.text = "## Stand\n\n| Child | Status |\n| Login | fertig |".into();
        let mut form = entry(2, true, 1);
        form.kind = "formular".into();
        form.title = "Migration jetzt?".into();
        form.path = Some("/tmp/f.md".into());
        let info = entry(3, false, 3);
        let mut entries = vec![form, info, answer];
        sort_entries(&mut entries);
        let mut inbox = inbox_with(entries);
        let text = screen(&inbox);
        if std::env::var_os("COLLIE_PRINT_INBOX").is_some() {
            eprintln!("{text}");
        }
        let wichtig = text.find("Wichtig (2)").expect("important group");
        let optional = text.find("Optional (1)").expect("optional group");
        assert!(wichtig < optional);
        assert!(
            text.find("Was ist seit gestern").unwrap() < text.find("Migration jetzt?").unwrap()
        );
        assert!(text.contains("| Login | fertig |"));
        assert!(text.contains("dd erledigt · u zurück · r Rückfrage · esc"));

        inbox.pending_delete = true;
        assert!(screen(&inbox).contains("jede andere Taste bricht ab"));
        inbox.pending_delete = false;
        inbox.reading = true;
        let reading = screen(&inbox);
        assert!(!reading.contains("Wichtig (2)"));
        assert!(reading.contains("## Stand"));
    }

    #[test]
    fn important_entries_come_first_newest_first() {
        let mut entries = vec![entry(1, false, 10), entry(2, true, 5), entry(3, true, 20)];
        sort_entries(&mut entries);
        let ids = entries.iter().map(|entry| entry.id).collect::<Vec<_>>();
        assert_eq!(ids, vec![3, 2, 1]);
    }

    #[test]
    fn tasks_stay_pending_until_finished() {
        let mut form = entry(1, true, 0);
        form.kind = "formular".into();
        form.read = true;
        assert!(pending(&form));
        form.finished = true;
        assert!(!pending(&form));
        let mut message = entry(2, true, 0);
        assert!(pending(&message));
        message.read = true;
        assert!(!pending(&message));
    }

    #[test]
    fn text_lines_keep_line_breaks_and_wrap_long_lines() {
        let lines = text_lines("eins zwei drei\n\nvier", 9);
        assert_eq!(lines, vec!["eins zwei", "drei", "", "vier"]);
    }
}
