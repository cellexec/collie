//! collie: Overlay der globalen Nutzer-Inbox, siehe docs/specs/user-inbox.md.
//! Die Einträge aller Chats stehen als Tabelle wie im Telescope; darunter
//! der Text des gewählten Eintrags als gesetztes Markdown. Die Einträge lädt
//! die TUI per `chat.user_inbox` mit `all`, der Snapshot trägt nur Zähler und
//! Revision.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use super::chat_cards::{age_color, age_label, ICON_CROWN};
use super::chat_telescope::TelescopeRender;
use super::markdown::{render_markdown, MdLine};
use super::render::{display_width, put_text};
use super::*;
use crate::api::schema::{ChatUserInboxAction, UserInboxEntryInfo};
use crate::ui::truncate_end;

/// Symbol der Nutzer-Inbox in der Sidebar und im Titel.
pub(super) const ICON_INBOX: &str = "󰇮";
/// Markierte Einträge in der ersten Spalte.
const ICON_CHECKED: &str = "󰄲";
/// Freier Streifen rechts der offenen Inbox; ein Klick dorthin schließt.
const BACKDROP_COLS: u16 = 6;

/// Einträge sind nur je Chat eindeutig.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EntryKey {
    pub(super) workspace_id: String,
    pub(super) id: u64,
}

impl EntryKey {
    fn of(entry: &UserInboxEntryInfo) -> Self {
        Self {
            workspace_id: entry.workspace_id.clone(),
            id: entry.id,
        }
    }

    fn matches(&self, entry: &UserInboxEntryInfo) -> bool {
        self.id == entry.id && self.workspace_id == entry.workspace_id
    }
}

#[derive(Debug)]
pub(super) struct ClientUserInbox {
    /// Wichtig zuerst, dann optional, jeweils neueste zuerst.
    pub(super) entries: Vec<UserInboxEntryInfo>,
    pub(super) loaded: bool,
    /// Revision der geladenen Einträge.
    pub(super) revision: u64,
    /// Revision, für die gerade eine Anfrage läuft.
    pub(super) requested: Option<u64>,
    /// Eintrag unter dem Cursor.
    pub(super) selected: Option<EntryKey>,
    /// Mit `space` oder `m` markierte Einträge.
    pub(super) marked: Vec<EntryKey>,
    /// Das erste `d` ist gedrückt.
    pub(super) pending_delete: bool,
    /// Gelöschte Einträge je `dd`, für `u`.
    pub(super) undo: Vec<Vec<EntryKey>>,
    /// Der Text des Eintrags füllt das ganze Overlay.
    pub(super) reading: bool,
    pub(super) detail_scroll: usize,
    pub(super) scroll: usize,
    /// Pane, aus dem die Inbox geöffnet wurde; `esc` kehrt dorthin zurück.
    pub(super) origin_pane: Option<String>,
}

/// collie: Die Inbox-Liste links, während eine Aufgabe als Tab im
/// Inbox-Space läuft. Tastatur und Panes gehören der Aufgabe.
#[derive(Debug)]
pub(super) struct InboxDock {
    pub(super) inbox: ClientUserInbox,
    /// Sichtbarer Tab des angedockten Sheets.
    pub(super) tab: super::sheet_tabs::SheetTab,
    /// Tabs des Inbox-Space beim letzten Blick; `None`, bis der neue Tab im
    /// Snapshot fokussiert ist.
    pub(super) space_tabs: Option<usize>,
    pub(super) since: std::time::Instant,
    /// Mit der Zoom-Taste (`prefix+f`) ausgeblendet: die Aufgabe füllt den
    /// ganzen Bildschirm, bis die Taste die Liste wieder zeigt.
    pub(super) hidden: bool,
}

/// So lange wartet die Dock-Ansicht auf den neuen Tab, bevor sie aufgibt.
const DOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

impl ClientUserInbox {
    fn new(revision: u64) -> Self {
        Self {
            entries: Vec::new(),
            loaded: false,
            revision,
            requested: None,
            selected: None,
            marked: Vec::new(),
            pending_delete: false,
            undo: Vec::new(),
            reading: false,
            detail_scroll: 0,
            scroll: 0,
            origin_pane: None,
        }
    }

    fn is_marked(&self, entry: &UserInboxEntryInfo) -> bool {
        self.marked.iter().any(|key| key.matches(entry))
    }
}

fn sort_entries(entries: &mut [UserInboxEntryInfo]) {
    entries.sort_by(|a, b| b.important.cmp(&a.important).then(b.at.cmp(&a.at)));
}

fn selected_index(inbox: &ClientUserInbox) -> usize {
    inbox
        .selected
        .as_ref()
        .and_then(|key| inbox.entries.iter().position(|entry| key.matches(entry)))
        .unwrap_or(0)
}

fn is_task(entry: &UserInboxEntryInfo) -> bool {
    matches!(entry.kind.as_str(), "formular" | "wizard")
}

/// Gelesen heißt bei Nachrichten geöffnet, bei Aufgaben erledigt.
fn pending(entry: &UserInboxEntryInfo) -> bool {
    if is_task(entry) {
        !entry.finished
    } else {
        !entry.read
    }
}

fn kind_symbol(kind: &str) -> &'static str {
    match kind {
        "antwort" => "↩",
        // Nerd-Font-Symbole wie Krone und Inbox: Dokument und Zahnrad.
        "formular" => "󰈙",
        "wizard" => "\u{f0493}",
        _ => "✉",
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "antwort" => "Antwort",
        "formular" => "Formular",
        "wizard" => "Wizard",
        _ => "Nachricht",
    }
}

/// Zähler der globalen Inbox für die Sidebar: Symbol, wichtige rot,
/// optionale grau. `None`, wenn nichts offen ist.
pub(super) fn inbox_badge(snapshot: &ClientShellSnapshot, palette: &Palette) -> Option<MdLine> {
    let counts = &snapshot.user_inbox;
    if counts.important + counts.optional == 0 {
        return None;
    }
    let mut parts = Vec::new();
    let icon_color = if counts.important > 0 {
        palette.red
    } else {
        palette.overlay0
    };
    parts.push((
        ICON_INBOX.to_owned(),
        Style::default().fg(icon_color).add_modifier(Modifier::BOLD),
    ));
    if counts.important > 0 {
        parts.push((
            format!(" {}", counts.important),
            Style::default()
                .fg(palette.red)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if counts.optional > 0 {
        parts.push((
            format!(" {}", counts.optional),
            Style::default().fg(palette.overlay0),
        ));
    }
    Some(parts)
}

/// Zeichnet gestylte Stücke ab `x`; liefert die Spalte dahinter.
pub(super) fn put_spans(buffer: &mut Buffer, x: u16, y: u16, right: u16, spans: &MdLine) -> u16 {
    let mut x = x;
    for (text, style) in spans {
        if x >= right {
            break;
        }
        let width = display_width(text).min(right - x);
        put_text(buffer, x, y, width, text, *style);
        x = x.saturating_add(display_width(text));
    }
    x.min(right)
}

pub(super) fn badge_width(spans: &MdLine) -> u16 {
    spans.iter().map(|(text, _)| display_width(text)).sum()
}

/// Spaltenbreiten: Markierung, Status, Art, Chat, Titel, Alter.
/// Schmal steht die Art nur als Symbol da.
fn columns(width: u16) -> [u16; 6] {
    let kind = if width >= 64 { 11 } else { 1 };
    let fixed = 1 + 1 + kind + 5 + 5;
    let flex = width.saturating_sub(fixed);
    let chat = (flex / 3).clamp(6, 22);
    let title = flex.saturating_sub(chat);
    [1, 1, kind, chat, title, 5]
}

/// Fläche der offenen Inbox: volle Höhe, fast volle Breite. Rechts bleibt
/// ein Streifen frei; ein Klick dorthin schließt.
pub(super) fn inbox_rect(screen: Rect) -> Rect {
    let width = screen
        .width
        .saturating_sub(BACKDROP_COLS)
        .max(screen.width.min(40));
    Rect::new(screen.x, screen.y, width, screen.height)
}

/// Breite der angedockten Liste, während eine Aufgabe läuft.
pub(super) fn dock_width(cols: u16) -> u16 {
    (cols * 2 / 5).clamp(36, 72).min(cols.saturating_sub(20))
}

/// Fläche wie das Chat-Sheet von `prefix+w`: Sidebar-Hintergrund, rechts
/// eine dünne Kante, kein Rahmen. Gibt die Innenfläche unter Kopfzeile und
/// Leerzeile zurück.
fn sheet(buffer: &mut Buffer, rect: Rect, palette: &Palette) -> Option<Rect> {
    if rect.width < 8 || rect.height < 4 {
        return None;
    }
    // Die Inbox liegt im selben Puffer wie die Panes: erst leeren, sonst
    // scheint deren Text durch.
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.reset();
            }
        }
    }
    super::render::render_sidebar_background(buffer, rect, palette);
    Some(sheet_inner(rect))
}

fn sheet_inner(rect: Rect) -> Rect {
    Rect::new(
        rect.x.saturating_add(1),
        rect.y.saturating_add(2),
        rect.width.saturating_sub(3),
        rect.height.saturating_sub(2),
    )
}

/// Fläche zwischen Kopf- und Fußzeile der offenen Inbox.
fn inbox_body(inner: Rect) -> Rect {
    let footer_y = inner.bottom().saturating_sub(1);
    Rect::new(
        inner.x,
        inner.y,
        inner.width,
        footer_y.saturating_sub(1).saturating_sub(inner.y),
    )
}

/// Tabelle und Lesebereich. Im Lesemodus füllt der Text die ganze Fläche.
fn split_body(body: Rect, reading: bool) -> (Option<Rect>, Rect) {
    if reading {
        return (None, body);
    }
    let table_width = (body.width * 9 / 20).clamp(40.min(body.width), body.width);
    let table = Rect::new(body.x, body.y, table_width, body.height);
    let detail_x = table.right().saturating_add(3);
    let detail = Rect::new(
        detail_x,
        body.y,
        body.right().saturating_sub(detail_x),
        body.height,
    );
    (Some(table), detail)
}

/// Zeilen über dem Text: Kopf, im Lesemodus der Titel, eine Leerzeile.
fn detail_text_top(reading: bool) -> u16 {
    if reading {
        3
    } else {
        2
    }
}

fn detail_lines(entry: &UserInboxEntryInfo, width: u16, palette: &Palette) -> Vec<MdLine> {
    let base = Style::default().bg(palette.sidebar_bg);
    render_markdown(
        &entry.text,
        usize::from(width),
        palette,
        base.fg(palette.text),
    )
}

/// Größter sinnvoller Scroll-Versatz des Textes in `detail`.
fn detail_max_scroll(
    entry: &UserInboxEntryInfo,
    detail: Rect,
    reading: bool,
    palette: &Palette,
) -> usize {
    let room = usize::from(detail.height.saturating_sub(detail_text_top(reading)));
    detail_lines(entry, detail.width, palette)
        .len()
        .saturating_sub(room)
}

/// Kopfzeile mit den Tabs Chats und Inbox, rechts die Zahl der
/// Markierungen.
fn render_title(
    buffer: &mut Buffer,
    rect: Rect,
    inbox: &ClientUserInbox,
    counts: &crate::protocol::ClientShellUserInbox,
    focused: bool,
    palette: &Palette,
) {
    let base = Style::default().bg(palette.sidebar_bg);
    super::sheet_tabs::render_sheet_tabs(
        buffer,
        rect.x,
        rect.y,
        rect.right().saturating_sub(1),
        counts,
        super::sheet_tabs::SheetTab::Inbox,
        focused,
        palette,
    );
    if !inbox.marked.is_empty() {
        let label = format!(" {ICON_CHECKED} {} markiert ", inbox.marked.len());
        let width = display_width(&label);
        put_text(
            buffer,
            rect.right().saturating_sub(width + 1),
            rect.y,
            width,
            &label,
            base.fg(palette.green),
        );
    }
}

/// Leere Inbox oder Ladehinweis; `true`, wenn er gezeichnet wurde.
fn render_empty(
    buffer: &mut Buffer,
    inner: Rect,
    inbox: &ClientUserInbox,
    palette: &Palette,
) -> bool {
    if inbox.loaded && !inbox.entries.is_empty() {
        return false;
    }
    let text = if inbox.loaded {
        "Nichts in der Inbox. Chats legen Einträge mit `herdr chat tell-user` ab."
    } else {
        "lädt …"
    };
    put_text(
        buffer,
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        text,
        Style::default()
            .bg(palette.sidebar_bg)
            .fg(palette.overlay0)
            .add_modifier(Modifier::ITALIC),
    );
    true
}

/// Die offene Inbox: links die Tabelle, rechts der Lesebereich.
pub(super) fn render_user_inbox(
    buffer: &mut Buffer,
    inbox: &ClientUserInbox,
    snapshot: Option<&ClientShellSnapshot>,
    config: &ClientShellConfig,
) -> Option<TelescopeRender> {
    let palette = &config.palette;
    let now = crate::workspace::unix_millis_now();
    let rect = inbox_rect(buffer.area);
    let inner = sheet(buffer, rect, palette)?;
    let base = Style::default().bg(palette.sidebar_bg);
    let selected = selected_index(inbox);
    let counts = snapshot
        .map(|snapshot| snapshot.user_inbox.clone())
        .unwrap_or_default();
    render_title(buffer, rect, inbox, &counts, true, palette);

    let footer_y = inner.bottom().saturating_sub(1);
    let footer = if inbox.pending_delete {
        (
            "d erledigt · jede andere Taste bricht ab",
            base.fg(palette.red).add_modifier(Modifier::BOLD),
        )
    } else if inbox.reading {
        (
            "j/k ^d/^u g/G scrollen · y kopieren · n in nvim · r Rückfrage · dd erledigt · q/esc zurück",
            base.fg(palette.overlay0),
        )
    } else {
        (
            "enter lesen · y kopieren · n in nvim · space markieren · dd erledigt · u zurück · r Rückfrage · [ ] chats · esc",
            base.fg(palette.overlay0),
        )
    };
    put_text(buffer, inner.x, footer_y, inner.width, footer.0, footer.1);

    let mut menu_rows = Vec::new();
    let body = inbox_body(inner);
    if render_empty(buffer, body, inbox, palette) {
        return Some(TelescopeRender {
            area: rect,
            menu_rows,
        });
    }

    // Links die Tabelle, rechts der Text des gewählten Eintrags. Im
    // Lesemodus steht nur der Text da, umgebrochen über die ganze Breite.
    let (table, detail) = split_body(body, inbox.reading);
    if let Some(table) = table {
        render_table(
            buffer,
            table,
            inbox,
            selected,
            snapshot,
            now,
            palette,
            &mut menu_rows,
        );
        let divider_x = table.right().saturating_add(1);
        for y in body.y..body.bottom() {
            put_text(buffer, divider_x, y, 1, "│", base.fg(palette.surface1));
        }
    }
    if let Some(entry) = inbox.entries.get(selected) {
        render_detail(
            buffer,
            detail,
            entry,
            &chat_label(snapshot, entry),
            inbox.detail_scroll,
            inbox.reading,
            palette,
        );
    }

    Some(TelescopeRender {
        area: rect,
        menu_rows,
    })
}

/// Die angedockte Liste, während eine Aufgabe im Inbox-Space läuft.
pub(super) fn render_inbox_dock(
    buffer: &mut Buffer,
    area: Rect,
    inbox: &ClientUserInbox,
    snapshot: Option<&ClientShellSnapshot>,
    config: &ClientShellConfig,
) -> Option<TelescopeRender> {
    let palette = &config.palette;
    let now = crate::workspace::unix_millis_now();
    let inner = sheet(buffer, area, palette)?;
    let base = Style::default().bg(palette.sidebar_bg);
    let counts = snapshot
        .map(|snapshot| snapshot.user_inbox.clone())
        .unwrap_or_default();
    render_title(buffer, area, inbox, &counts, false, palette);
    let footer_y = inner.bottom().saturating_sub(1);
    put_text(
        buffer,
        inner.x,
        footer_y,
        inner.width,
        "prefix+i oder Klick: zurück in die Inbox",
        base.fg(palette.overlay0),
    );
    let mut menu_rows = Vec::new();
    let body = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        footer_y.saturating_sub(1).saturating_sub(inner.y),
    );
    if !render_empty(buffer, body, inbox, palette) {
        render_table(
            buffer,
            body,
            inbox,
            selected_index(inbox),
            snapshot,
            now,
            palette,
            &mut menu_rows,
        );
    }
    Some(TelescopeRender { area, menu_rows })
}

/// Name des Chats aus dem Snapshot, sonst aus dem Eintrag.
fn chat_label(snapshot: Option<&ClientShellSnapshot>, entry: &UserInboxEntryInfo) -> String {
    let label = snapshot
        .and_then(|snapshot| {
            snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == entry.workspace_id)
        })
        .map_or(entry.chat.as_str(), |workspace| workspace.label.as_str());
    if entry.master {
        format!("{ICON_CROWN} {label}")
    } else {
        label.to_owned()
    }
}

#[allow(clippy::too_many_arguments)]
fn render_table(
    buffer: &mut Buffer,
    table: Rect,
    inbox: &ClientUserInbox,
    selected: usize,
    snapshot: Option<&ClientShellSnapshot>,
    now: u64,
    palette: &Palette,
    menu_rows: &mut Vec<(Rect, usize)>,
) {
    let base = Style::default().bg(palette.sidebar_bg);
    let widths = columns(table.width);
    let headers = [
        "",
        "",
        if widths[2] > 1 { "Art" } else { "" },
        "Chat",
        "Titel",
        "Alter",
    ];
    let mut x = table.x;
    for (header, width) in headers.iter().zip(widths) {
        put_text(
            buffer,
            x,
            table.y,
            width,
            header,
            base.fg(palette.overlay0).add_modifier(Modifier::BOLD),
        );
        x = x.saturating_add(width + 1);
    }

    let rows = Rect::new(
        table.x,
        table.y + 1,
        table.width,
        table.height.saturating_sub(1),
    );
    let visible = usize::from(rows.height);
    let mut scroll = inbox.scroll.min(inbox.entries.len().saturating_sub(1));
    if selected < scroll {
        scroll = selected;
    } else if visible > 0 && selected >= scroll + visible {
        scroll = selected + 1 - visible;
    }
    for (index, entry) in inbox.entries.iter().enumerate().skip(scroll).take(visible) {
        let y = rows.y + (index - scroll) as u16;
        let line = Rect::new(rows.x, y, rows.width, 1);
        let is_selected = index == selected;
        let marked = inbox.is_marked(entry);
        let deleting = inbox.pending_delete && (marked || (inbox.marked.is_empty() && is_selected));
        let row_style = if deleting {
            base.bg(palette.surface0)
                .fg(palette.red)
                .add_modifier(Modifier::CROSSED_OUT)
        } else if is_selected {
            base.bg(palette.surface0)
        } else {
            base
        };
        buffer.set_style(line, row_style);
        let mut x = line.x;
        let mut cell = |buffer: &mut Buffer, column: usize, text: &str, style: Style| {
            let width = widths[column];
            put_text(
                buffer,
                x,
                y,
                width,
                &truncate_end(text, usize::from(width)),
                style,
            );
            x = x.saturating_add(width + 1);
        };
        // Spalte 1: markiert als Checkbox, sonst leer.
        cell(
            buffer,
            0,
            if marked { ICON_CHECKED } else { "" },
            row_style.fg(palette.green).add_modifier(Modifier::BOLD),
        );
        let (dot, dot_color) = if !pending(entry) {
            ("○", palette.overlay0)
        } else if entry.important {
            ("●", palette.red)
        } else {
            ("●", palette.accent)
        };
        cell(buffer, 1, dot, row_style.fg(dot_color));
        let kind = if widths[2] > 1 {
            format!("{} {}", kind_symbol(&entry.kind), kind_label(&entry.kind))
        } else {
            kind_symbol(&entry.kind).to_owned()
        };
        cell(buffer, 2, &kind, row_style.fg(palette.subtext0));
        let chat_color = if entry.master {
            palette.yellow
        } else {
            palette.mauve
        };
        cell(
            buffer,
            3,
            &chat_label(snapshot, entry),
            row_style.fg(chat_color),
        );
        let title_style = if deleting {
            row_style
        } else if pending(entry) {
            row_style.fg(palette.text).add_modifier(Modifier::BOLD)
        } else {
            row_style.fg(palette.subtext0)
        };
        cell(buffer, 4, &entry.title, title_style);
        cell(
            buffer,
            5,
            &age_label(entry.at, now),
            row_style.fg(age_color(entry.at, now, palette)),
        );
        menu_rows.push((line, index));
    }
}

fn render_detail(
    buffer: &mut Buffer,
    rect: Rect,
    entry: &UserInboxEntryInfo,
    chat: &str,
    scroll: usize,
    reading: bool,
    palette: &Palette,
) {
    if rect.height == 0 {
        return;
    }
    let base = Style::default().bg(palette.sidebar_bg);
    let mut header = format!("{} · {chat}", kind_label(&entry.kind));
    if let Some(path) = entry.path.as_deref() {
        header.push_str(" · ");
        header.push_str(path);
    }
    let hint = match entry.kind.as_str() {
        "formular" => "enter öffnet das Formular in nvim",
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
    let top = detail_text_top(reading);
    if reading && rect.height > 1 {
        put_text(
            buffer,
            rect.x,
            rect.y + 1,
            rect.width,
            &truncate_end(&entry.title, usize::from(rect.width)),
            base.fg(palette.text).add_modifier(Modifier::BOLD),
        );
    }
    let lines = detail_lines(entry, rect.width, palette);
    let room = usize::from(rect.height.saturating_sub(top));
    let scroll = scroll.min(lines.len().saturating_sub(room));
    for (offset, line) in lines.iter().skip(scroll).take(room).enumerate() {
        put_spans(
            buffer,
            rect.x,
            rect.y + top + offset as u16,
            rect.right(),
            line,
        );
    }
    if scroll > 0 {
        let less = format!(" ↑ {scroll} Zeilen ");
        let width = display_width(&less);
        put_text(
            buffer,
            rect.right().saturating_sub(width),
            rect.y,
            width,
            &less,
            base.fg(palette.overlay0),
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

/// Gruppiert Schlüssel nach Chat, für eine Aktion je Chat.
fn by_chat(keys: &[EntryKey]) -> Vec<(String, Vec<u64>)> {
    let mut groups: Vec<(String, Vec<u64>)> = Vec::new();
    for key in keys {
        match groups
            .iter_mut()
            .find(|(workspace, _)| *workspace == key.workspace_id)
        {
            Some((_, ids)) => ids.push(key.id),
            None => groups.push((key.workspace_id.clone(), vec![key.id])),
        }
    }
    groups
}

impl ClientShellState {
    /// Öffnet die globale Nutzer-Inbox und lädt ihre Einträge. Ist sie
    /// angedockt, kommt sie wieder ganz nach vorn.
    pub(super) fn open_user_inbox(&mut self, outcome: &mut ClientShellInput) {
        if self.inbox_dock.is_some() {
            self.undock_user_inbox(true, outcome);
            return;
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let revision = snapshot.user_inbox.revision;
        let in_space = snapshot.user_inbox.space_workspace_id.is_some()
            && snapshot.focused_workspace_id == snapshot.user_inbox.space_workspace_id;
        let origin_pane = (!in_space).then(|| self.focused_pane_id()).flatten();
        if self.mode == ClientShellMode::Navigate {
            self.leave_sidebar(false, outcome);
        }
        let mut inbox = ClientUserInbox::new(revision);
        inbox.origin_pane = origin_pane;
        self.overlay = Some(ClientShellOverlay::UserInbox(inbox));
        self.request_user_inbox(outcome);
        outcome.repaint = true;
    }

    /// Zoom-Taste, solange die Inbox angedockt ist: blendet die Liste aus
    /// oder wieder ein, statt das Pane zu zoomen. Liefert, ob sie das tat.
    pub(super) fn toggle_inbox_dock(&mut self, outcome: &mut ClientShellInput) -> bool {
        let Some(dock) = self.inbox_dock.as_mut() else {
            return false;
        };
        dock.hidden = !dock.hidden;
        self.invalidate_pane_surface();
        outcome.resize = true;
        outcome.repaint = true;
        true
    }

    /// Die Inbox, ob offen oder angedockt.
    fn user_inbox_mut(&mut self) -> Option<&mut ClientUserInbox> {
        match self.overlay.as_mut() {
            Some(ClientShellOverlay::UserInbox(inbox)) => Some(inbox),
            _ => self.inbox_dock.as_mut().map(|dock| &mut dock.inbox),
        }
    }

    /// Dockt die Inbox ab: `back` holt sie ganz nach vorn, sonst schließt
    /// sie, und zurückgehaltene Rückmeldungen gehen raus.
    fn undock_user_inbox(&mut self, back: bool, outcome: &mut ClientShellInput) {
        let Some(dock) = self.inbox_dock.take() else {
            return;
        };
        self.invalidate_pane_surface();
        outcome.resize = true;
        outcome.repaint = true;
        if back {
            if self.mode == ClientShellMode::Navigate {
                self.leave_sidebar(false, outcome);
            }
            self.overlay = Some(ClientShellOverlay::UserInbox(dock.inbox));
            self.request_user_inbox(outcome);
        } else {
            self.push_user_inbox_act(None, ChatUserInboxAction::Flush, Vec::new(), outcome);
        }
    }

    /// Maus auf dem angedockten Sheet. Die Tabs wechseln die Ansicht. Im
    /// Inbox-Tab holt ein Klick die Inbox nach vorn, mit dem angeklickten
    /// Eintrag unter dem Cursor. Im Chat-Tab öffnet ein Klick den Chat und
    /// verlässt damit den Inbox-Space.
    pub(super) fn route_inbox_dock_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.overlay.is_some() {
            return false;
        }
        let Some(tab) = self.inbox_dock.as_ref().map(|dock| dock.tab) else {
            return false;
        };
        let point = (mouse.column, mouse.row);
        let area = self.hits.inbox_dock_area;
        if !super::contains(area, point) {
            return false;
        }
        let counts = self
            .snapshot
            .as_deref()
            .map(|snapshot| snapshot.user_inbox.clone())
            .unwrap_or_default();
        let clicked_tab = super::sheet_tabs::sheet_tabs(area.x, area.y, &counts).hit(point);
        let left = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        if left {
            if let Some(next) = clicked_tab.filter(|next| *next != tab) {
                if let Some(dock) = self.inbox_dock.as_mut() {
                    dock.tab = next;
                }
                outcome.repaint = true;
                return true;
            }
        }
        match tab {
            super::sheet_tabs::SheetTab::Chats => {
                self.route_docked_chats_mouse(mouse, point, outcome);
            }
            super::sheet_tabs::SheetTab::Inbox if left => {
                let hit = self
                    .hits
                    .inbox_dock_rows
                    .iter()
                    .find(|(rect, _)| super::contains(*rect, point))
                    .map(|(_, index)| *index);
                if let (Some(index), Some(dock)) = (hit, self.inbox_dock.as_mut()) {
                    dock.inbox.selected = dock.inbox.entries.get(index).map(EntryKey::of);
                    dock.inbox.detail_scroll = 0;
                }
                self.undock_user_inbox(true, outcome);
            }
            super::sheet_tabs::SheetTab::Inbox => {}
        }
        true
    }

    /// Chat-Tab im angedockten Sheet: Klick öffnet den Chat, das Rad
    /// scrollt die Karten.
    fn route_docked_chats_mouse(
        &mut self,
        mouse: MouseEvent,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let pane = self
                    .hits
                    .agents
                    .iter()
                    .find(|(rect, _)| super::contains(*rect, point))
                    .map(|(_, pane_id)| pane_id.clone());
                if let Some(pane_id) = pane {
                    self.focus_or_activate(
                        self.active_endpoint_id.clone(),
                        ClientEndpointFocusTarget::Pane(pane_id),
                        outcome,
                    );
                    outcome.repaint = true;
                }
            }
            MouseEventKind::ScrollUp => {
                self.agent_scroll = self.agent_scroll.saturating_sub(1);
                outcome.repaint = true;
            }
            MouseEventKind::ScrollDown => {
                self.agent_scroll = self
                    .agent_scroll
                    .saturating_add(1)
                    .min(self.hits.agent_max_scroll);
                outcome.repaint = true;
            }
            _ => {}
        }
    }

    /// `[` oder `]` und der Klick auf `chats`: Die Inbox schließt, das
    /// Sheet zeigt die Chats.
    fn switch_user_inbox_to_chats(&mut self, outcome: &mut ClientShellInput) {
        self.close_user_inbox(outcome);
        self.open_chat_overlay(false, outcome);
    }

    fn request_user_inbox(&mut self, outcome: &mut ClientShellInput) {
        let revision = self
            .snapshot
            .as_deref()
            .map(|snapshot| snapshot.user_inbox.revision);
        let Some(inbox) = self.user_inbox_mut() else {
            return;
        };
        let revision = revision.unwrap_or(inbox.revision);
        inbox.requested = Some(revision);
        let method =
            crate::api::schema::Method::ChatUserInbox(crate::api::schema::ChatUserInboxParams {
                workspace_id: None,
                caller_pane_id: None,
                all: true,
            });
        self.push_endpoint_method_with_kind(
            method,
            PendingEndpointKind::UserInbox { revision },
            outcome,
        );
    }

    fn act_on_user_inbox(
        &mut self,
        action: ChatUserInboxAction,
        keys: &[EntryKey],
        outcome: &mut ClientShellInput,
    ) {
        for (workspace_id, ids) in by_chat(keys) {
            self.push_user_inbox_act(Some(workspace_id), action, ids, outcome);
        }
    }

    fn push_user_inbox_act(
        &mut self,
        workspace_id: Option<String>,
        action: ChatUserInboxAction,
        ids: Vec<u64>,
        outcome: &mut ClientShellInput,
    ) {
        let revision = self
            .snapshot
            .as_deref()
            .map(|snapshot| snapshot.user_inbox.revision)
            .unwrap_or_default();
        let method = crate::api::schema::Method::ChatUserInboxAct(
            crate::api::schema::ChatUserInboxActParams {
                workspace_id,
                caller_pane_id: None,
                action,
                ids,
                exit_code: None,
                path: None,
                all: true,
            },
        );
        self.push_endpoint_method_with_kind(
            method,
            PendingEndpointKind::UserInbox { revision },
            outcome,
        );
    }

    /// Antwort auf `chat.user_inbox` oder `chat.user_inbox_act`.
    pub(super) fn complete_user_inbox(
        &mut self,
        revision: u64,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> bool {
        let Ok(crate::api::schema::ResponseResult::ChatUserInbox { mut user_inbox }) = result
        else {
            return false;
        };
        let Some(inbox) = self.user_inbox_mut() else {
            return false;
        };
        sort_entries(&mut user_inbox);
        // Der Cursor bleibt auf seinem Eintrag; ist der weg, auf derselben
        // Position.
        let position = selected_index(inbox);
        inbox.entries = user_inbox;
        if inbox
            .selected
            .as_ref()
            .is_none_or(|key| !inbox.entries.iter().any(|entry| key.matches(entry)))
        {
            inbox.selected = inbox
                .entries
                .get(position.min(inbox.entries.len().saturating_sub(1)))
                .map(EntryKey::of);
            inbox.detail_scroll = 0;
        }
        let entries = &inbox.entries;
        inbox
            .marked
            .retain(|key| entries.iter().any(|entry| key.matches(entry)));
        inbox.loaded = true;
        inbox.revision = revision;
        if inbox.requested == Some(revision) {
            inbox.requested = None;
        }
        true
    }

    /// Lädt die Inbox neu, wenn sich die Revision im Snapshot geändert hat,
    /// etwa weil ein Chat etwas abgelegt hat.
    pub(crate) fn tick_user_inbox(&mut self, outcome: &mut ClientShellInput) {
        self.tick_inbox_dock(outcome);
        let Some(current) = self
            .snapshot
            .as_deref()
            .map(|snapshot| snapshot.user_inbox.revision)
        else {
            return;
        };
        let Some(inbox) = self.user_inbox_mut() else {
            return;
        };
        if current != inbox.revision && inbox.requested != Some(current) {
            self.request_user_inbox(outcome);
        }
    }

    /// Folgt der laufenden Aufgabe: Schließt sich ihr Tab, kommt die Inbox
    /// zurück. Wechselt der Nutzer woandershin, schließt sie.
    fn tick_inbox_dock(&mut self, outcome: &mut ClientShellInput) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let Some(dock) = self.inbox_dock.as_mut() else {
            return;
        };
        let space = snapshot.user_inbox.space_workspace_id.as_deref();
        let tabs = space.map_or(0, |space| {
            snapshot
                .tabs
                .iter()
                .filter(|tab| tab.workspace_id == space)
                .count()
        });
        let focused = space.is_some() && snapshot.focused_workspace_id.as_deref() == space;
        match dock.space_tabs {
            None if focused && tabs > 0 => dock.space_tabs = Some(tabs),
            None if dock.since.elapsed() > DOCK_WAIT => self.undock_user_inbox(true, outcome),
            None => {}
            // Ein Tab weniger: die Aufgabe ist fertig.
            Some(known) if tabs < known => self.undock_user_inbox(true, outcome),
            Some(_) if !focused => self.undock_user_inbox(false, outcome),
            Some(known) if tabs > known => dock.space_tabs = Some(tabs),
            Some(_) => {}
        }
    }

    /// Schließt das Overlay. Zurückgehaltene Rückmeldungen gehen jetzt an
    /// die Chats.
    fn close_user_inbox(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.take() else {
            return;
        };
        self.push_user_inbox_act(None, ChatUserInboxAction::Flush, Vec::new(), outcome);
        outcome.repaint = true;
        // Zurück dorthin, wo die Inbox geöffnet wurde.
        let origin = inbox.origin_pane.filter(|pane| {
            self.snapshot
                .as_deref()
                .is_some_and(|snapshot| snapshot.panes.iter().any(|known| &known.pane_id == pane))
        });
        if let Some(pane) = origin {
            if self.focused_pane_id().as_deref() != Some(pane.as_str()) {
                self.focus_or_activate(
                    self.active_endpoint_id.clone(),
                    ClientEndpointFocusTarget::Pane(pane),
                    outcome,
                );
            }
        }
    }

    /// Formular, Wizard oder Rückfrage laufen als Tab im Inbox-Space. Die
    /// Inbox dockt links an, bis der Tab sich schließt.
    fn open_user_inbox_pane(
        &mut self,
        action: ChatUserInboxAction,
        key: EntryKey,
        outcome: &mut ClientShellInput,
    ) {
        self.act_on_user_inbox(action, std::slice::from_ref(&key), outcome);
        let Some(ClientShellOverlay::UserInbox(mut inbox)) = self.overlay.take() else {
            return;
        };
        inbox.reading = false;
        inbox.pending_delete = false;
        self.inbox_dock = Some(InboxDock {
            inbox,
            tab: super::sheet_tabs::SheetTab::Inbox,
            space_tabs: None,
            since: std::time::Instant::now(),
            hidden: false,
        });
        self.invalidate_pane_surface();
        outcome.resize = true;
        outcome.repaint = true;
    }

    /// Größter Scroll-Versatz und halbe Seite des Lesebereichs, gemessen an
    /// der zuletzt gezeichneten Fläche. Vor dem ersten Bild ohne Grenze.
    fn user_inbox_detail_bounds(&self) -> (usize, usize) {
        let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.as_ref() else {
            return (0, 1);
        };
        let area = self.hits.user_inbox_area;
        let (_, detail) = split_body(inbox_body(sheet_inner(area)), inbox.reading);
        let room = detail.height.saturating_sub(detail_text_top(inbox.reading));
        let page = usize::from(room / 2).max(1);
        if detail.width == 0 || room == 0 {
            return (usize::MAX, page);
        }
        let max = inbox.entries.get(selected_index(inbox)).map_or(0, |entry| {
            detail_max_scroll(entry, detail, inbox.reading, &self.config.palette)
        });
        (max, page)
    }

    pub(super) fn route_user_inbox_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let (max_scroll, page) = self.user_inbox_detail_bounds();
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
                let keys = if inbox.marked.is_empty() {
                    current.iter().map(EntryKey::of).collect::<Vec<_>>()
                } else {
                    std::mem::take(&mut inbox.marked)
                };
                if keys.is_empty() {
                    return true;
                }
                inbox
                    .entries
                    .retain(|entry| !keys.iter().any(|key| key.matches(entry)));
                inbox.selected = inbox
                    .entries
                    .get(index.min(inbox.entries.len().saturating_sub(1)))
                    .map(EntryKey::of);
                inbox.detail_scroll = 0;
                inbox.reading = false;
                inbox.undo.push(keys.clone());
                self.act_on_user_inbox(ChatUserInboxAction::Done, &keys, outcome);
            }
            return true;
        }

        let select = |inbox: &mut ClientUserInbox, index: usize| {
            let key = inbox.entries.get(index).map(EntryKey::of);
            if key != inbox.selected {
                inbox.selected = key;
                inbox.detail_scroll = 0;
            }
        };
        // Scrollt den Text, ohne über sein Ende hinauszulaufen.
        let scroll = |inbox: &mut ClientUserInbox, delta: isize| {
            let current = inbox.detail_scroll.min(max_scroll);
            inbox.detail_scroll = current.saturating_add_signed(delta).min(max_scroll);
        };
        let page = isize::try_from(page).unwrap_or(isize::MAX);
        match code {
            KeyCode::Esc | KeyCode::Char('q') if inbox.reading => inbox.reading = false,
            KeyCode::Esc | KeyCode::Char('q') => self.close_user_inbox(outcome),
            KeyCode::Char('[' | ']') if !ctrl => self.switch_user_inbox_to_chats(outcome),
            KeyCode::Down | KeyCode::Char('j') if inbox.reading => scroll(inbox, 1),
            KeyCode::Up | KeyCode::Char('k') if inbox.reading => scroll(inbox, -1),
            KeyCode::Char('g') if inbox.reading => inbox.detail_scroll = 0,
            KeyCode::Char('G') if inbox.reading => inbox.detail_scroll = max_scroll,
            KeyCode::Down | KeyCode::Char('j') => select(inbox, (index + 1).min(last)),
            KeyCode::Up | KeyCode::Char('k') => select(inbox, index.saturating_sub(1)),
            KeyCode::Char('n') if ctrl => select(inbox, (index + 1).min(last)),
            KeyCode::Char('p') if ctrl => select(inbox, index.saturating_sub(1)),
            KeyCode::Char('d') if ctrl => scroll(inbox, page),
            KeyCode::Char('u') if ctrl => scroll(inbox, -page),
            KeyCode::Char('f') if ctrl => scroll(inbox, page * 2),
            KeyCode::Char('b') if ctrl => scroll(inbox, -page * 2),
            KeyCode::PageDown => scroll(inbox, page * 2),
            KeyCode::PageUp => scroll(inbox, -page * 2),
            KeyCode::Char('g') => select(inbox, 0),
            KeyCode::Char('G') => select(inbox, last),
            KeyCode::Char('y') if !ctrl => {
                if let Some(entry) = current {
                    outcome
                        .actions
                        .push(ClientShellAction::ClipboardWrite(entry.text.into_bytes()));
                }
            }
            KeyCode::Char('n') if !ctrl => {
                // Nachrichten öffnet der Server als Markdown-Datei im Editor,
                // Formulare und Wizards wie mit `enter`.
                if let Some(entry) = current {
                    self.open_user_inbox_pane(
                        ChatUserInboxAction::Open,
                        EntryKey::of(&entry),
                        outcome,
                    );
                }
            }
            KeyCode::Char(' ' | 'm') if !inbox.reading => {
                if let Some(entry) = current.as_ref() {
                    if let Some(position) = inbox.marked.iter().position(|key| key.matches(entry)) {
                        inbox.marked.remove(position);
                    } else {
                        inbox.marked.push(EntryKey::of(entry));
                    }
                    select(inbox, (index + 1).min(last));
                }
            }
            KeyCode::Char('d') if current.is_some() || !inbox.marked.is_empty() => {
                inbox.pending_delete = true;
            }
            KeyCode::Char('u') => {
                if let Some(keys) = inbox.undo.pop() {
                    self.act_on_user_inbox(ChatUserInboxAction::Restore, &keys, outcome);
                }
            }
            KeyCode::Char('r') => {
                if let Some(entry) = current {
                    self.open_user_inbox_pane(
                        ChatUserInboxAction::Reply,
                        EntryKey::of(&entry),
                        outcome,
                    );
                }
            }
            KeyCode::Enter => {
                let Some(entry) = current else {
                    return true;
                };
                if is_task(&entry) {
                    self.open_user_inbox_pane(
                        ChatUserInboxAction::Open,
                        EntryKey::of(&entry),
                        outcome,
                    );
                } else if inbox.reading {
                    inbox.reading = false;
                } else {
                    inbox.reading = true;
                    if !entry.read {
                        if let Some(local) = inbox
                            .entries
                            .iter_mut()
                            .find(|local| EntryKey::of(&entry).matches(local))
                        {
                            local.read = true;
                        }
                        self.act_on_user_inbox(
                            ChatUserInboxAction::Read,
                            &[EntryKey::of(&entry)],
                            outcome,
                        );
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
        let (max_scroll, _) = self.user_inbox_detail_bounds();
        let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.as_mut() else {
            return false;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let counts = self
                    .snapshot
                    .as_deref()
                    .map(|snapshot| snapshot.user_inbox.clone())
                    .unwrap_or_default();
                if super::sheet_tabs::sheet_tabs(area.x, area.y, &counts).hit(point)
                    == Some(super::sheet_tabs::SheetTab::Chats)
                {
                    self.switch_user_inbox_to_chats(outcome);
                    return true;
                }
                let Some(ClientShellOverlay::UserInbox(inbox)) = self.overlay.as_mut() else {
                    return false;
                };
                let Some(index) = hit else {
                    if !super::contains(area, point) {
                        self.close_user_inbox(outcome);
                    }
                    return true;
                };
                let key = inbox.entries.get(index).map(EntryKey::of);
                if key.is_some() && key == inbox.selected {
                    let enter = crate::input::TerminalKey::new(KeyCode::Enter, KeyModifiers::NONE);
                    self.route_user_inbox_key(&enter, outcome);
                } else {
                    inbox.selected = key;
                    inbox.detail_scroll = 0;
                    outcome.repaint = true;
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let down = mouse.kind == MouseEventKind::ScrollDown;
                // Rechts der Tabelle scrollt das Rad den Text.
                let table_right = self
                    .hits
                    .telescope_rows
                    .iter()
                    .map(|(rect, _)| rect.right())
                    .max()
                    .unwrap_or(0);
                if inbox.reading || point.0 > table_right {
                    let current = inbox.detail_scroll.min(max_scroll);
                    inbox.detail_scroll = if down {
                        current.saturating_add(3).min(max_scroll)
                    } else {
                        current.saturating_sub(3)
                    };
                } else {
                    let index = selected_index(inbox);
                    let next = if down {
                        (index + 1).min(inbox.entries.len().saturating_sub(1))
                    } else {
                        index.saturating_sub(1)
                    };
                    inbox.selected = inbox.entries.get(next).map(EntryKey::of);
                    inbox.detail_scroll = 0;
                }
                outcome.repaint = true;
            }
            _ => {}
        }
        true
    }
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
            workspace_id: "w1".into(),
            chat: "Planung".into(),
            master: true,
        }
    }

    fn inbox_with(entries: Vec<UserInboxEntryInfo>) -> ClientUserInbox {
        let mut inbox = ClientUserInbox::new(0);
        inbox.selected = entries.first().map(EntryKey::of);
        inbox.entries = entries;
        inbox.loaded = true;
        inbox
    }

    #[test]
    fn the_sheet_hides_the_panes_below_it() {
        let config = ClientShellConfig::from_config(&crate::config::Config::default());
        let mut buffer = Buffer::empty(Rect::new(0, 0, 120, 20));
        for cell in buffer.content.iter_mut() {
            cell.set_symbol("X");
        }
        let inbox = inbox_with(vec![entry(1, true, 1)]);
        render_user_inbox(&mut buffer, &inbox, None, &config).expect("rendered");
        let sheet = inbox_rect(buffer.area);
        for y in sheet.y..sheet.bottom() {
            for x in sheet.x..sheet.right() {
                assert_ne!(buffer[(x, y)].symbol(), "X", "pane text at {x},{y}");
            }
        }
        // Der Streifen rechts bleibt Hintergrund.
        assert_eq!(buffer[(119, 5)].symbol(), "X");
    }

    fn screen(inbox: &ClientUserInbox) -> String {
        let config = ClientShellConfig::from_config(&crate::config::Config::default());
        let mut buffer = Buffer::empty(Rect::new(0, 0, 160, 30));
        render_user_inbox(&mut buffer, inbox, None, &config).expect("rendered");
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
    fn overlay_is_a_table_with_rendered_markdown_beside_it() {
        let mut answer = entry(1, true, 2);
        answer.kind = "antwort".into();
        answer.title = "Was ist seit gestern passiert?".into();
        answer.text = "## Stand\n\n| Child | Status |\n|---|---|\n| Login | **fertig** |".into();
        let mut form = entry(2, true, 1);
        form.kind = "formular".into();
        form.title = "Migration jetzt?".into();
        form.path = Some("/tmp/f.md".into());
        let mut child = entry(1, false, 3);
        child.workspace_id = "w2".into();
        child.chat = "Login".into();
        child.master = false;
        let mut entries = vec![form, child, answer];
        sort_entries(&mut entries);
        let mut inbox = inbox_with(entries);
        inbox.marked.push(EntryKey {
            workspace_id: "w2".into(),
            id: 1,
        });
        let text = screen(&inbox);
        if std::env::var_os("COLLIE_PRINT_INBOX").is_some() {
            eprintln!("{text}");
        }
        assert!(text.contains("Art"), "{text}");
        assert!(text.contains("Chat"), "{text}");
        assert!(text.contains("󰆥 Planung"), "{text}");
        // Die markierte Zeile trägt die Checkbox in der ersten Spalte.
        let marked_row = text
            .lines()
            .find(|line| line.contains("Nachricht"))
            .expect("row of the marked entry");
        assert!(
            marked_row
                .trim_start_matches(['│', ' '])
                .starts_with(ICON_CHECKED),
            "{text}"
        );
        assert!(
            text.find("Was ist seit gestern").unwrap() < text.find("Migration jetzt?").unwrap()
        );
        // Markdown ist gesetzt, nicht roh.
        assert!(text.contains("Child │ Status"), "{text}");
        assert!(text.contains("Login │ fertig"), "{text}");
        assert!(!text.contains("## Stand"), "{text}");
        assert!(text.contains("dd erledigt · u zurück · r Rückfrage · [ ] chats · esc"));

        inbox.pending_delete = true;
        assert!(screen(&inbox).contains("jede andere Taste bricht ab"));
        inbox.pending_delete = false;
        inbox.reading = true;
        let reading = screen(&inbox);
        // Der Text füllt die ganze Fläche, der Titel steht darüber.
        assert!(!reading.contains("Titel"), "{reading}");
        assert!(
            reading.contains("Was ist seit gestern passiert?"),
            "{reading}"
        );
        assert!(reading.contains("Stand"));
        assert!(reading.contains("q/esc zurück"));
        // Rechts bleibt der Streifen zum Schließen frei.
        let first = reading.lines().next().unwrap_or_default();
        assert!(display_width(first) <= 160 - BACKDROP_COLS, "{first}");
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
    fn actions_go_to_the_chat_of_each_entry() {
        let key = |workspace: &str, id| EntryKey {
            workspace_id: workspace.into(),
            id,
        };
        let groups = by_chat(&[key("w1", 1), key("w2", 1), key("w1", 3)]);
        assert_eq!(
            groups,
            vec![("w1".to_owned(), vec![1, 3]), ("w2".to_owned(), vec![1])]
        );
    }
}
