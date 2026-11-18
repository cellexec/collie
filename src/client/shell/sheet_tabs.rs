//! collie: Kopfzeile des Sheets. Chats und Inbox teilen sich ein Sheet; die
//! Kopfzeile zeigt beide als Tabs, `[` und `]` oder ein Klick wechseln.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use super::render::{display_width, put_text};
use super::user_inbox::{put_spans, ICON_INBOX};
use super::*;
use crate::protocol::ClientShellUserInbox;

const CHATS_LABEL: &str = " 󰭹 chats";
const DIVIDER: &str = " │ ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SheetTab {
    #[default]
    Chats,
    Inbox,
}

/// Klickflächen der beiden Tabs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SheetTabs {
    pub(super) chats: Rect,
    pub(super) inbox: Rect,
    /// Erste Spalte hinter den Tabs.
    pub(super) end: u16,
}

impl SheetTabs {
    pub(super) fn hit(&self, point: (u16, u16)) -> Option<SheetTab> {
        if super::contains(self.chats, point) {
            Some(SheetTab::Chats)
        } else if super::contains(self.inbox, point) {
            Some(SheetTab::Inbox)
        } else {
            None
        }
    }
}

/// Zähler hinter `inbox`: wichtige rot, optionale grau.
fn inbox_counts(counts: &ClientShellUserInbox, palette: &Palette) -> Vec<(String, Style)> {
    let mut spans = Vec::new();
    if counts.important > 0 {
        spans.push((
            format!(" {}", counts.important),
            Style::default()
                .fg(palette.red)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if counts.optional > 0 {
        spans.push((
            format!(" {}", counts.optional),
            Style::default().fg(palette.overlay0),
        ));
    }
    spans
}

/// Lage der Tabs ab `x`, ohne zu zeichnen; Maus und Zeichnen nutzen sie
/// gleichermaßen.
pub(super) fn sheet_tabs(x: u16, y: u16, counts: &ClientShellUserInbox) -> SheetTabs {
    let chats_width = display_width(CHATS_LABEL);
    let inbox_x = x
        .saturating_add(chats_width)
        .saturating_add(display_width(DIVIDER));
    let counts_width: u16 = [counts.important, counts.optional]
        .iter()
        .filter(|count| **count > 0)
        .map(|count| display_width(&format!(" {count}")))
        .sum();
    let inbox_width = display_width(&inbox_label()).saturating_add(counts_width);
    SheetTabs {
        chats: Rect::new(x, y, chats_width.saturating_add(1), 1),
        inbox: Rect::new(inbox_x, y, inbox_width, 1),
        end: inbox_x.saturating_add(inbox_width),
    }
}

fn inbox_label() -> String {
    format!("{ICON_INBOX} inbox")
}

/// Zeichnet ` 󰭹 chats │ 󰇮 inbox 3 2` ab `x`. Der aktive Tab ist hell und
/// fett, der andere grau.
#[allow(clippy::too_many_arguments)] // Position, Zähler und Stil gehören zusammen.
pub(super) fn render_sheet_tabs(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    right: u16,
    counts: &ClientShellUserInbox,
    active: SheetTab,
    focused: bool,
    palette: &Palette,
) -> SheetTabs {
    let tabs = sheet_tabs(x, y, counts);
    let bright = Style::default()
        .fg(if focused {
            palette.accent
        } else {
            palette.text
        })
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(palette.overlay0);
    let style = |tab: SheetTab| if tab == active { bright } else { dim };
    let width = |from: u16| right.saturating_sub(from);
    put_text(buffer, x, y, width(x), CHATS_LABEL, style(SheetTab::Chats));
    let divider_x = x.saturating_add(display_width(CHATS_LABEL));
    put_text(
        buffer,
        divider_x,
        y,
        width(divider_x),
        DIVIDER,
        Style::default().fg(palette.surface1),
    );
    let label = inbox_label();
    put_text(
        buffer,
        tabs.inbox.x,
        y,
        width(tabs.inbox.x),
        &label,
        style(SheetTab::Inbox),
    );
    let counts_x = tabs.inbox.x.saturating_add(display_width(&label));
    put_spans(buffer, counts_x, y, right, &inbox_counts(counts, palette));
    SheetTabs {
        end: tabs.end.min(right),
        ..tabs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol().to_owned())
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn both_tabs_share_one_line_with_the_inbox_counts() {
        let config = ClientShellConfig::from_config(&crate::config::Config::default());
        let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 1));
        let counts = ClientShellUserInbox {
            important: 3,
            optional: 2,
            ..Default::default()
        };
        let tabs = render_sheet_tabs(
            &mut buffer,
            0,
            0,
            60,
            &counts,
            SheetTab::Inbox,
            true,
            &config.palette,
        );
        assert_eq!(line(&buffer, 0), " 󰭹 chats │ 󰇮 inbox 3 2");
        assert_eq!(tabs.hit((2, 0)), Some(SheetTab::Chats));
        assert_eq!(tabs.hit((tabs.inbox.x + 1, 0)), Some(SheetTab::Inbox));
        assert_eq!(tabs.hit((tabs.end + 1, 0)), None);
        assert_eq!(
            buffer[(tabs.inbox.x, 0)].style().fg,
            Some(config.palette.accent)
        );
        assert_eq!(buffer[(1, 0)].style().fg, Some(config.palette.overlay0));
    }
}
