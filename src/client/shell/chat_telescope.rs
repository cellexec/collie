//! collie: Telescope eines Master Chats. Ein zentriertes Modal zeigt die
//! Children als Tabelle; `j`/`k` wählt, `enter` öffnet. Dazu der Dialog, der
//! beim Schließen eines Masters fragt, was mit seinen Children passiert.
//! Siehe docs/specs/master-chat.md.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use super::chat_cards::{age_color, age_label, ICON_CROWN};
use super::chat_visuals::{
    activity_buckets, context_color, context_gauge, context_ratio, repo_color, spark_symbol,
    SPARK_BUCKETS,
};
use super::chats::{chat_symbol, child_rows, ChatRow};
use super::render::{display_width, put_text};
use super::*;
use crate::api::schema::AgentStatus;
use crate::ui::truncate_end;

/// Anteil des Bildschirms, den das Telescope einnimmt.
const TELESCOPE_PERCENT: u16 = 90;
/// Zeilen für Auftrag und letzte Nachricht unter der Tabelle.
const DETAIL_LINES: u16 = 4;

#[derive(Debug)]
pub(super) struct ClientChatTelescope {
    pub(super) master_workspace_id: String,
    /// Workspace des Childs unter dem Cursor.
    pub(super) selected: Option<String>,
    pub(super) query: String,
    pub(super) filtering: bool,
    /// Flach nach Wartezeit statt nach Status gruppiert.
    pub(super) flat: bool,
    /// `d` wartet auf Bestätigung.
    pub(super) confirm_close: bool,
    pub(super) scroll: usize,
}

/// Was mit den Children passiert, wenn ihr Master schließt.
#[derive(Debug)]
pub(super) struct ClientMasterCloseOverlay {
    pub(super) workspace_id: String,
    pub(super) highlighted: usize,
}

const MASTER_CLOSE_CHOICES: [&str; 3] = [
    " c  Children schließen",
    " r  Children in die Hauptliste übernehmen",
    " esc  abbrechen",
];

pub(super) struct TelescopeRender {
    pub(super) area: Rect,
    pub(super) menu_rows: Vec<(Rect, usize)>,
}

/// Rang beim Sortieren nach Status: blockiert, wartend, laufend, gestoppt.
fn status_rank(row: &ChatRow) -> u8 {
    if row.stopped {
        3
    } else if row.needs_reply() {
        0
    } else if row.status == AgentStatus::Working {
        2
    } else {
        1
    }
}

/// Die Children eines Masters in Anzeigereihenfolge.
pub(super) fn telescope_rows(
    snapshot: &ClientShellSnapshot,
    telescope: &ClientChatTelescope,
) -> Vec<ChatRow> {
    let mut rows = child_rows(snapshot, &telescope.master_workspace_id)
        .into_iter()
        .filter(|row| {
            let query = telescope.query.to_lowercase();
            query.is_empty()
                || [
                    Some(row.label.as_str()),
                    row.repo.as_deref(),
                    row.branch.as_deref(),
                    row.assignment.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|text| text.to_lowercase().contains(&query))
        })
        .collect::<Vec<_>>();
    // Wer am längsten wartet, steht oben.
    rows.sort_by_key(|row| row.attention_since);
    if !telescope.flat {
        rows.sort_by_key(status_rank);
    }
    rows
}

fn selected_index(rows: &[ChatRow], telescope: &ClientChatTelescope) -> usize {
    telescope
        .selected
        .as_ref()
        .and_then(|selected| rows.iter().position(|row| &row.workspace_id == selected))
        .unwrap_or(0)
}

fn telescope_rect(screen: Rect) -> Rect {
    let width = (u32::from(screen.width) * u32::from(TELESCOPE_PERCENT) / 100) as u16;
    let height = (u32::from(screen.height) * u32::from(TELESCOPE_PERCENT) / 100) as u16;
    Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

/// Rahmen mit Hintergrund; gibt die Innenfläche zurück.
fn frame(buffer: &mut Buffer, rect: Rect, palette: &Palette) -> Option<Rect> {
    if rect.width < 4 || rect.height < 4 {
        return None;
    }
    let background = Style::default()
        .bg(palette.panel_bg)
        .remove_modifier(Modifier::DIM);
    let border = background.fg(palette.accent);
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let symbol = match (
                x == rect.x,
                x + 1 == rect.right(),
                y == rect.y,
                y + 1 == rect.bottom(),
            ) {
                (true, _, true, _) => "┌",
                (_, true, true, _) => "┐",
                (true, _, _, true) => "└",
                (_, true, _, true) => "┘",
                (_, _, true, _) | (_, _, _, true) => "─",
                (true, _, _, _) | (_, true, _, _) => "│",
                _ => " ",
            };
            let style = if symbol == " " { background } else { border };
            buffer[(x, y)].set_symbol(symbol).set_style(style);
        }
    }
    Some(Rect::new(
        rect.x + 2,
        rect.y + 1,
        rect.width.saturating_sub(4),
        rect.height.saturating_sub(2),
    ))
}

/// Spaltenbreiten: Status, Titel, Repo, Branch, Aktivität, Kontext, Idle,
/// Nachricht.
fn columns(width: u16) -> [u16; 8] {
    let fixed = 2 + 14 + 18 + SPARK_BUCKETS as u16 + 9 + 6 + 7;
    let flex = width.saturating_sub(fixed);
    let title = (flex * 2 / 5).max(10);
    let message = flex.saturating_sub(title);
    [2, title, 14, 18, SPARK_BUCKETS as u16, 9, 6, message]
}

pub(super) fn render_chat_telescope(
    buffer: &mut Buffer,
    telescope: &ClientChatTelescope,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
) -> Option<TelescopeRender> {
    let palette = &config.palette;
    let now = crate::workspace::unix_millis_now();
    let rect = telescope_rect(buffer.area);
    let inner = frame(buffer, rect, palette)?;
    let base = Style::default().bg(palette.panel_bg);
    let rows = telescope_rows(snapshot, telescope);
    let selected = selected_index(&rows, telescope);

    // Titel im Rahmen.
    let master_label = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == telescope.master_workspace_id)
        .map_or("Master Chat", |workspace| workspace.label.as_str());
    let title = format!(" {ICON_CROWN} {master_label} · {} Chats ", rows.len());
    put_text(
        buffer,
        rect.x + 2,
        rect.y,
        rect.width.saturating_sub(4),
        &title,
        base.fg(palette.yellow).add_modifier(Modifier::BOLD),
    );
    let sort = if telescope.flat {
        " wartezeit "
    } else {
        " status "
    };
    put_text(
        buffer,
        rect.right().saturating_sub(display_width(sort) + 2),
        rect.y,
        display_width(sort),
        sort,
        base.fg(palette.overlay0),
    );

    // Kopfzeile der Tabelle.
    let widths = columns(inner.width);
    let headers = [
        "",
        "Titel",
        "Repo",
        "Branch",
        "Aktivität",
        "Kontext",
        "Idle",
        "Letzte Nachricht",
    ];
    let mut x = inner.x;
    for (header, width) in headers.iter().zip(widths) {
        put_text(
            buffer,
            x,
            inner.y,
            width,
            header,
            base.fg(palette.overlay0).add_modifier(Modifier::BOLD),
        );
        x = x.saturating_add(width + 1);
    }

    // Tabelle, Detailbereich und Fußzeile.
    let footer_y = inner.bottom().saturating_sub(1);
    let detail_top = footer_y.saturating_sub(DETAIL_LINES + 1);
    let table = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        detail_top.saturating_sub(inner.y + 1),
    );
    let visible = usize::from(table.height);
    let mut scroll = telescope.scroll.min(rows.len().saturating_sub(1));
    if selected < scroll {
        scroll = selected;
    } else if visible > 0 && selected >= scroll + visible {
        scroll = selected + 1 - visible;
    }
    let mut menu_rows = Vec::new();
    if rows.is_empty() {
        let empty = if telescope.query.is_empty() {
            "Noch keine Children. Der Master startet sie mit `herdr chat spawn`."
        } else {
            "Kein Child passt zum Filter."
        };
        put_text(
            buffer,
            table.x,
            table.y.saturating_add(1),
            table.width,
            empty,
            base.fg(palette.overlay0).add_modifier(Modifier::ITALIC),
        );
    }
    for (offset, row) in rows.iter().enumerate().skip(scroll).take(visible) {
        let y = table.y + (offset - scroll) as u16;
        let line = Rect::new(inner.x, y, inner.width, 1);
        let is_selected = offset == selected;
        let line_style = if is_selected {
            base.bg(palette.surface0)
        } else {
            base
        };
        buffer.set_style(line, line_style);
        render_row(
            buffer,
            line,
            &widths,
            row,
            is_selected,
            now,
            palette,
            line_style,
        );
        menu_rows.push((line, offset));
    }

    // Detailbereich: Auftrag und letzte Nachricht des Childs unter dem Cursor.
    for x in inner.x..inner.right() {
        put_text(buffer, x, detail_top, 1, "─", base.fg(palette.surface1));
    }
    if let Some(row) = rows.get(selected) {
        let label_style = base.fg(palette.overlay0).add_modifier(Modifier::BOLD);
        let mut y = detail_top + 1;
        for (label, text) in [
            ("Auftrag  ", row.assignment.as_deref()),
            (
                "Zuletzt  ",
                row.blocked_reason
                    .as_deref()
                    .filter(|_| row.status == AgentStatus::Blocked)
                    .or(row.last_message.as_deref()),
            ),
        ] {
            let Some(text) = text else {
                continue;
            };
            put_text(buffer, inner.x, y, 9, label, label_style);
            let width = inner.width.saturating_sub(9);
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            for chunk in wrap(&text, usize::from(width), 2) {
                if y >= footer_y {
                    break;
                }
                put_text(buffer, inner.x + 9, y, width, &chunk, base.fg(palette.text));
                y += 1;
            }
        }
    }

    // Fußzeile: Filter, Rückfrage oder Tasten.
    let (footer, style) = if telescope.confirm_close {
        let title = rows.get(selected).map_or("", |row| row.label.as_str());
        (
            format!("{title} schließen? y ja · n nein"),
            base.fg(status_color(AgentStatus::Blocked, palette))
                .add_modifier(Modifier::BOLD),
        )
    } else if telescope.filtering || !telescope.query.is_empty() {
        (format!("/{}", telescope.query), base.fg(palette.accent))
    } else {
        (
            "j/k wählen · enter öffnen · p prompt · x stop/resume · d schließen · s sortieren · / filter · esc".to_owned(),
            base.fg(palette.overlay0),
        )
    };
    put_text(buffer, inner.x, footer_y, inner.width, &footer, style);

    Some(TelescopeRender {
        area: rect,
        menu_rows,
    })
}

#[allow(clippy::too_many_arguments)]
fn render_row(
    buffer: &mut Buffer,
    line: Rect,
    widths: &[u16; 8],
    row: &ChatRow,
    selected: bool,
    now: u64,
    palette: &Palette,
    base: Style,
) {
    let mut x = line.x;
    let mut cell = |buffer: &mut Buffer, index: usize, text: &str, style: Style| {
        let width = widths[index];
        put_text(
            buffer,
            x,
            line.y,
            width,
            &truncate_end(text, usize::from(width)),
            style,
        );
        x = x.saturating_add(width + 1);
    };
    match chat_symbol(row, now, palette) {
        Some((symbol, style)) => cell(buffer, 0, symbol, style.bg(base.bg.unwrap_or_default())),
        None => cell(buffer, 0, "", base),
    }
    let title_style = if selected || row.unread {
        base.fg(palette.text).add_modifier(Modifier::BOLD)
    } else {
        base.fg(palette.subtext0)
    };
    cell(buffer, 1, &row.label, title_style);
    let repo = row.repo.as_deref().unwrap_or("");
    cell(buffer, 2, repo, base.fg(repo_color(repo, palette)));
    cell(
        buffer,
        3,
        row.branch.as_deref().unwrap_or(""),
        base.fg(palette.mauve),
    );
    // Aktivität: die Kurve der letzten 30 Minuten.
    let working_since = row
        .working_since
        .filter(|_| row.status == AgentStatus::Working);
    let buckets = activity_buckets(&row.working_spans, working_since, now);
    let spark = buckets
        .iter()
        .map(|value| spark_symbol(*value).unwrap_or("▁"))
        .collect::<String>();
    cell(
        buffer,
        4,
        &spark,
        base.fg(status_color(AgentStatus::Working, palette)),
    );
    match context_ratio(row.context_tokens, row.context_window) {
        Some(ratio) => cell(
            buffer,
            5,
            &format!(
                "{} {:>2}%",
                context_gauge(ratio),
                (ratio * 100.0).round() as u32
            ),
            base.fg(context_color(ratio, palette)),
        ),
        None => cell(buffer, 5, "", base),
    }
    let (idle, idle_color) = if row.stopped {
        ("■".to_owned(), palette.overlay0)
    } else if row.status == AgentStatus::Working {
        (
            row.working_since
                .map(|since| age_label(since, now))
                .unwrap_or_default(),
            status_color(AgentStatus::Working, palette),
        )
    } else if row.attention_since == 0 {
        (String::new(), palette.overlay0)
    } else {
        (
            age_label(row.attention_since, now),
            age_color(row.attention_since, now, palette),
        )
    };
    cell(buffer, 6, &idle, base.fg(idle_color));
    let (message, style) = if row.stopped {
        (
            "gestoppt".to_owned(),
            base.fg(palette.overlay0).add_modifier(Modifier::ITALIC),
        )
    } else if row.awaiting_reply() {
        (
            format!(
                "wartet auf Antwort: {}",
                row.awaiting.as_deref().unwrap_or_default()
            ),
            base.fg(palette.yellow),
        )
    } else if row.status == AgentStatus::Blocked {
        (
            row.blocked_reason
                .clone()
                .unwrap_or_else(|| "wartet auf Eingabe".to_owned()),
            base.fg(status_color(AgentStatus::Blocked, palette)),
        )
    } else {
        (
            row.last_message.clone().unwrap_or_default(),
            base.fg(palette.overlay1),
        )
    };
    cell(buffer, 7, &message, style);
}

/// Bricht `text` in höchstens `max_lines` Zeilen der Breite `width` um. Die
/// letzte Zeile endet abgeschnitten mit `…`.
fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split(' ') {
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if unicode_width::UnicodeWidthStr::width(candidate.as_str()) <= width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        current = word.to_owned();
        if lines.len() == max_lines {
            break;
        }
    }
    if !current.is_empty() && lines.len() < max_lines {
        lines.push(current);
    }
    if lines.len() == max_lines {
        let used = lines.join(" ");
        if unicode_width::UnicodeWidthStr::width(used.as_str()) < text.len() {
            if let Some(last) = lines.last_mut() {
                *last = truncate_end(&format!("{last} …"), width);
            }
        }
    }
    lines.truncate(max_lines);
    lines
}

pub(super) fn render_master_close(
    buffer: &mut Buffer,
    overlay: &ClientMasterCloseOverlay,
    palette: &Palette,
) -> Option<TelescopeRender> {
    let screen = buffer.area;
    let width = MASTER_CLOSE_CHOICES
        .iter()
        .map(|label| display_width(label))
        .max()
        .unwrap_or(20)
        .saturating_add(6)
        .min(screen.width);
    let height = (MASTER_CLOSE_CHOICES.len() as u16 + 4).min(screen.height);
    let rect = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let inner = frame(buffer, rect, palette)?;
    let base = Style::default().bg(palette.panel_bg);
    put_text(
        buffer,
        rect.x + 2,
        rect.y,
        rect.width.saturating_sub(4),
        &format!(" {ICON_CROWN} Master Chat schließen "),
        base.fg(palette.yellow).add_modifier(Modifier::BOLD),
    );
    let mut menu_rows = Vec::new();
    for (index, label) in MASTER_CLOSE_CHOICES.iter().enumerate() {
        let y = inner.y + 1 + index as u16;
        if y >= inner.bottom() {
            break;
        }
        let line = Rect::new(inner.x, y, inner.width, 1);
        let style = if index == overlay.highlighted {
            base.bg(palette.accent)
                .fg(palette.panel_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            base.fg(palette.text)
        };
        buffer.set_style(line, style);
        put_text(buffer, line.x, line.y, line.width, label, style);
        menu_rows.push((line, index));
    }
    Some(TelescopeRender {
        area: rect,
        menu_rows,
    })
}

impl ClientShellState {
    /// Öffnet das Telescope eines Masters. `select` setzt den Cursor auf ein
    /// Child.
    pub(super) fn open_chat_telescope(
        &mut self,
        master_workspace_id: String,
        select: Option<String>,
        outcome: &mut ClientShellInput,
    ) {
        self.last_master_workspace = Some(master_workspace_id.clone());
        if self.mode == ClientShellMode::Navigate {
            self.leave_sidebar(false, outcome);
        }
        self.overlay = Some(ClientShellOverlay::Telescope(ClientChatTelescope {
            master_workspace_id,
            selected: select,
            query: String::new(),
            filtering: false,
            flat: false,
            confirm_close: false,
            scroll: 0,
        }));
        outcome.repaint = true;
    }

    /// `prefix+m`: Telescope des fokussierten Masters, des Masters des
    /// fokussierten Childs, sonst des zuletzt geöffneten oder ersten Masters.
    pub(super) fn open_focused_chat_telescope(&mut self, outcome: &mut ClientShellInput) -> bool {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return false;
        };
        let rows = super::chats::all_chat_rows(snapshot);
        let focused = rows.iter().find(|row| row.focused);
        let target = match focused {
            Some(row) if row.master => Some((row.workspace_id.clone(), None)),
            Some(row) if row.parent.is_some() => row
                .parent
                .clone()
                .map(|parent| (parent, Some(row.workspace_id.clone()))),
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
                })
                .map(|master| (master, None)),
        };
        let Some((master, select)) = target else {
            return false;
        };
        self.open_chat_telescope(master, select, outcome);
        true
    }

    pub(super) fn route_telescope_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if let Some(ClientShellOverlay::MasterClose(_)) = self.overlay {
            self.route_master_close_key(key, outcome);
            return true;
        }
        let Some(ClientShellOverlay::Telescope(telescope)) = self.overlay.as_ref() else {
            return false;
        };
        let rows = self
            .snapshot
            .as_deref()
            .map(|snapshot| telescope_rows(snapshot, telescope))
            .unwrap_or_default();
        let index = selected_index(&rows, telescope);
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        outcome.repaint = true;
        let Some(ClientShellOverlay::Telescope(telescope)) = self.overlay.as_mut() else {
            return false;
        };

        if telescope.confirm_close {
            telescope.confirm_close = false;
            if matches!(code, KeyCode::Char('y' | 'd')) {
                if let Some(row) = rows.get(index) {
                    let method = crate::api::schema::Method::ChatClose(
                        crate::api::schema::ChatCloseParams {
                            workspace_id: row.workspace_id.clone(),
                            children: crate::api::schema::ChatCloseChildren::Close,
                            caller_pane_id: None,
                        },
                    );
                    self.push_endpoint_method(method, outcome);
                }
            }
            return true;
        }

        if telescope.filtering {
            match code {
                KeyCode::Esc => {
                    telescope.filtering = false;
                    telescope.query.clear();
                }
                KeyCode::Enter => telescope.filtering = false,
                KeyCode::Backspace => {
                    telescope.query.pop();
                }
                KeyCode::Char(ch) if !modifiers.contains(KeyModifiers::CONTROL) => {
                    telescope.query.push(ch);
                    telescope.selected = None;
                }
                _ => {}
            }
            return true;
        }

        let select = |telescope: &mut ClientChatTelescope, index: usize| {
            telescope.selected = rows.get(index).map(|row| row.workspace_id.clone());
        };
        let last = rows.len().saturating_sub(1);
        match code {
            KeyCode::Esc if !telescope.query.is_empty() => telescope.query.clear(),
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = None,
            KeyCode::Down | KeyCode::Char('j') => select(telescope, (index + 1).min(last)),
            KeyCode::Up | KeyCode::Char('k') => select(telescope, index.saturating_sub(1)),
            KeyCode::Char('n') if modifiers == KeyModifiers::CONTROL => {
                select(telescope, (index + 1).min(last));
            }
            KeyCode::Char('p') if modifiers == KeyModifiers::CONTROL => {
                select(telescope, index.saturating_sub(1));
            }
            KeyCode::Char('g') => select(telescope, 0),
            KeyCode::Char('G') => select(telescope, last),
            KeyCode::Char('s') => telescope.flat = !telescope.flat,
            KeyCode::Char('/') => telescope.filtering = true,
            KeyCode::Char('d') if !rows.is_empty() => telescope.confirm_close = true,
            KeyCode::Enter => {
                if let Some(row) = rows.get(index) {
                    self.open_telescope_child(row.pane_id.clone(), outcome);
                }
            }
            KeyCode::Char('x') => {
                if let Some(row) = rows.get(index) {
                    let params = crate::api::schema::ChatWorkspaceParams {
                        workspace_id: Some(row.workspace_id.clone()),
                        caller_pane_id: None,
                    };
                    let method = if row.stopped {
                        crate::api::schema::Method::ChatResume(params)
                    } else {
                        crate::api::schema::Method::ChatStop(params)
                    };
                    self.push_endpoint_method(method, outcome);
                }
            }
            KeyCode::Char('p') => {
                if let Some(row) = rows.get(index) {
                    self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                        title: "prompt child",
                        input: TextEditor::new("", false),
                        target: ClientRenameTarget::ChatPrompt {
                            report_workspace_id: Some(row.workspace_id.clone()),
                            pane_ids: vec![row.pane_id.clone()],
                        },
                    }));
                }
            }
            _ => {}
        }
        true
    }

    fn open_telescope_child(&mut self, pane_id: String, outcome: &mut ClientShellInput) {
        self.overlay = None;
        self.focus_or_activate(
            self.active_endpoint_id.clone(),
            ClientEndpointFocusTarget::Pane(pane_id),
            outcome,
        );
        outcome.repaint = true;
    }

    /// Maus im Telescope oder im Schließen-Dialog. Gibt `true` zurück, wenn
    /// eines davon offen ist.
    pub(super) fn route_telescope_mouse(
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
        match self.overlay.as_mut() {
            Some(ClientShellOverlay::MasterClose(dialog)) => {
                match mouse.kind {
                    MouseEventKind::Moved => {
                        if let Some(index) = hit {
                            dialog.highlighted = index;
                            outcome.repaint = true;
                        }
                    }
                    MouseEventKind::Down(MouseButton::Left) => match hit {
                        Some(index) => self.choose_master_close(index, outcome),
                        None => {
                            self.overlay = None;
                            outcome.repaint = true;
                        }
                    },
                    _ => {}
                }
                true
            }
            Some(ClientShellOverlay::Telescope(telescope)) => {
                let rows = self
                    .snapshot
                    .as_deref()
                    .map(|snapshot| telescope_rows(snapshot, telescope))
                    .unwrap_or_default();
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        match hit.and_then(|index| rows.get(index)) {
                            Some(row) => {
                                let pane_id = row.pane_id.clone();
                                self.open_telescope_child(pane_id, outcome);
                            }
                            None => {
                                self.overlay = None;
                                outcome.repaint = true;
                            }
                        }
                    }
                    MouseEventKind::Moved => {
                        if let Some(row) = hit.and_then(|index| rows.get(index)) {
                            if telescope.selected.as_ref() != Some(&row.workspace_id) {
                                telescope.selected = Some(row.workspace_id.clone());
                                outcome.repaint = true;
                            }
                        }
                    }
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                        let index = selected_index(&rows, telescope);
                        let next = if mouse.kind == MouseEventKind::ScrollUp {
                            index.saturating_sub(1)
                        } else {
                            (index + 1).min(rows.len().saturating_sub(1))
                        };
                        telescope.selected = rows.get(next).map(|row| row.workspace_id.clone());
                        outcome.repaint = true;
                    }
                    _ => {}
                }
                true
            }
            _ => false,
        }
    }

    /// Schließt einen Master. Hat er Children, fragt ein Dialog, was mit
    /// ihnen passiert.
    pub(super) fn request_master_close(
        &mut self,
        workspace_id: String,
        children: usize,
        outcome: &mut ClientShellInput,
    ) {
        if children == 0 {
            self.push_endpoint_method(
                crate::api::schema::Method::ChatClose(crate::api::schema::ChatCloseParams {
                    workspace_id,
                    children: crate::api::schema::ChatCloseChildren::Close,
                    caller_pane_id: None,
                }),
                outcome,
            );
            return;
        }
        self.overlay = Some(ClientShellOverlay::MasterClose(ClientMasterCloseOverlay {
            workspace_id,
            highlighted: 0,
        }));
        outcome.repaint = true;
    }

    fn route_master_close_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        let Some(ClientShellOverlay::MasterClose(dialog)) = self.overlay.as_mut() else {
            return;
        };
        outcome.repaint = true;
        let last = MASTER_CLOSE_CHOICES.len() - 1;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = None,
            KeyCode::Down | KeyCode::Char('j') => {
                dialog.highlighted = (dialog.highlighted + 1).min(last);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                dialog.highlighted = dialog.highlighted.saturating_sub(1);
            }
            KeyCode::Char('c') => self.choose_master_close(0, outcome),
            KeyCode::Char('r') => self.choose_master_close(1, outcome),
            KeyCode::Enter => {
                let index = dialog.highlighted;
                self.choose_master_close(index, outcome);
            }
            _ => {}
        }
    }

    fn choose_master_close(&mut self, index: usize, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::MasterClose(dialog)) = self.overlay.take() else {
            return;
        };
        let children = match index {
            0 => crate::api::schema::ChatCloseChildren::Close,
            1 => crate::api::schema::ChatCloseChildren::Release,
            _ => {
                outcome.repaint = true;
                return;
            }
        };
        self.push_endpoint_method(
            crate::api::schema::Method::ChatClose(crate::api::schema::ChatCloseParams {
                workspace_id: dialog.workspace_id,
                children,
                caller_pane_id: None,
            }),
            outcome,
        );
        outcome.repaint = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_words_and_marks_the_cut() {
        assert_eq!(wrap("eins zwei drei", 9, 2), ["eins zwei", "drei"]);
        let cut = wrap("eins zwei drei vier fünf", 9, 2);
        assert_eq!(cut.len(), 2);
        assert!(cut[1].ends_with('…'));
    }

    #[test]
    fn columns_fill_the_width() {
        let widths = columns(160);
        let total = widths.iter().sum::<u16>() + widths.len() as u16 - 1;
        assert!(total <= 160 + 1);
        assert!(widths[1] >= 10);
    }
}
