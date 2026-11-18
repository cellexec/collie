//! collie: `z` in der Chat-Liste versteckt einen Chat für eine Weile.

use super::*;
use crossterm::event::KeyModifiers;

/// Dauern der Snooze-Auswahl, in Anzeigereihenfolge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SnoozeChoice {
    Hour,
    FourHours,
    Morning,
    Monday,
    Week,
    /// Freies Datum, eingetippt im Picker.
    Date,
    Clear,
}

const SNOOZE_CHOICES: [SnoozeChoice; 7] = [
    SnoozeChoice::Hour,
    SnoozeChoice::FourHours,
    SnoozeChoice::Morning,
    SnoozeChoice::Monday,
    SnoozeChoice::Week,
    SnoozeChoice::Date,
    SnoozeChoice::Clear,
];

const HOUR_MS: u64 = 3_600_000;
const DAY_SECS: u64 = 24 * 3_600;
/// Uhrzeit von „morgen früh" in Sekunden seit Mitternacht.
const MORNING_SECS: u64 = 8 * 3_600;
/// Uhrzeit, wenn ein Datum ohne Uhrzeit kommt.
const MORNING_HOUR: u8 = 8;

#[derive(Debug)]
pub(super) struct ClientSnoozeOverlay {
    pub(super) workspace_id: String,
    pub(super) highlighted: usize,
    /// Ob die Auswahl „heute 08:00" statt „morgen 08:00" anbietet.
    pub(super) morning_today: bool,
    /// Ortszeit beim Öffnen; ohne sie gibt es nur relative Dauern.
    pub(super) local_now: Option<time::PrimitiveDateTime>,
    /// Eingabe eines freien Datums, solange `d` aktiv ist.
    pub(super) date_input: Option<String>,
}

const WEEKDAYS: [&str; 7] = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"];

/// `Mi 25.11. 08:00`
fn date_label(at: time::PrimitiveDateTime) -> String {
    format!(
        "{} {:02}.{:02}. {:02}:{:02}",
        WEEKDAYS[usize::from(at.weekday().number_days_from_monday())],
        at.day(),
        u8::from(at.month()),
        at.hour(),
        at.minute()
    )
}

impl ClientSnoozeOverlay {
    pub(super) fn labels(&self) -> Vec<String> {
        let at = |choice| {
            self.local_now
                .and_then(|now| local_target(choice, now))
                .map(|target| format!(" ({})", date_label(target)))
                .unwrap_or_default()
        };
        SNOOZE_CHOICES
            .iter()
            .map(|choice| match choice {
                SnoozeChoice::Hour => " 1  1 hour".to_owned(),
                SnoozeChoice::FourHours => " 2  4 hours".to_owned(),
                SnoozeChoice::Morning if self.morning_today => " 3  today 08:00".to_owned(),
                SnoozeChoice::Morning => " 3  tomorrow 08:00".to_owned(),
                SnoozeChoice::Monday => format!(" 4  next monday{}", at(SnoozeChoice::Monday)),
                SnoozeChoice::Week => format!(" 5  in a week{}", at(SnoozeChoice::Week)),
                SnoozeChoice::Date => match (&self.date_input, self.local_now) {
                    (Some(input), Some(now)) => {
                        let target =
                            parse_snooze_date(input, now).map_or("?".to_owned(), date_label);
                        format!(" d  {input:<12}▏ → {target}")
                    }
                    _ => " d  date… (25.11. 14:00, fr, +3)".to_owned(),
                },
                SnoozeChoice::Clear => " u  unsnooze".to_owned(),
            })
            .collect()
    }
}

/// Ortszeit-Ziel der festen Datums-Optionen.
fn local_target(
    choice: SnoozeChoice,
    now: time::PrimitiveDateTime,
) -> Option<time::PrimitiveDateTime> {
    let morning = |date: time::Date| {
        time::Time::from_hms(MORNING_HOUR, 0, 0)
            .ok()
            .map(|time| time::PrimitiveDateTime::new(date, time))
    };
    match choice {
        SnoozeChoice::Monday => {
            let days = 7 - i64::from(now.weekday().number_days_from_monday());
            morning(now.date().checked_add(time::Duration::days(days))?)
        }
        SnoozeChoice::Week => morning(now.date().checked_add(time::Duration::days(7))?),
        _ => None,
    }
}

/// Liest ein freies Datum: `25.11.`, `25.11.2026`, `2026-11-25`, `+3`,
/// `heute`, `morgen`, ein Wochentag (`fr`, `fri`), jeweils optional mit
/// Uhrzeit (`14:00`, `14`). Ohne Uhrzeit gilt 08:00. Ein Datum ohne Jahr, das
/// schon vorbei ist, meint das nächste Jahr. Das Ergebnis liegt in der
/// Zukunft.
pub(super) fn parse_snooze_date(
    input: &str,
    now: time::PrimitiveDateTime,
) -> Option<time::PrimitiveDateTime> {
    let mut tokens = input.split_whitespace().collect::<Vec<_>>();
    let mut clock = None;
    if tokens.len() > 1 {
        clock = parse_clock(tokens.last()?);
        if clock.is_some() {
            tokens.pop();
        }
    }
    let (date, explicit_year) = match tokens.as_slice() {
        [single] => {
            // Nur eine Uhrzeit: heute, sonst morgen.
            if single.contains(':') {
                let time = parse_clock(single)?;
                let today = time::PrimitiveDateTime::new(now.date(), time);
                let at = if today > now {
                    today
                } else {
                    today.checked_add(time::Duration::days(1))?
                };
                return Some(at);
            }
            parse_date(single, now.date())?
        }
        _ => return None,
    };
    let time = clock.unwrap_or(time::Time::from_hms(MORNING_HOUR, 0, 0).ok()?);
    let mut at = time::PrimitiveDateTime::new(date, time);
    if at <= now && !explicit_year && input.contains('.') {
        at = at.replace_year(at.year() + 1).ok()?;
    }
    (at > now).then_some(at)
}

fn parse_clock(token: &str) -> Option<time::Time> {
    let (hour, minute) = match token.split_once(':') {
        Some((hour, minute)) => (hour.parse().ok()?, minute.parse().ok()?),
        None => (token.parse().ok()?, 0),
    };
    time::Time::from_hms(hour, minute, 0).ok()
}

/// Datum und ob ein Jahr angegeben war.
fn parse_date(token: &str, today: time::Date) -> Option<(time::Date, bool)> {
    let token = token.to_lowercase();
    if let Some(days) = token.strip_prefix('+') {
        let days = days.trim_end_matches('d').parse::<i64>().ok()?;
        return Some((today.checked_add(time::Duration::days(days))?, false));
    }
    match token.as_str() {
        "heute" | "today" => return Some((today, false)),
        "morgen" | "tomorrow" => return Some((today.next_day()?, false)),
        _ => {}
    }
    const NAMES: [[&str; 2]; 7] = [
        ["mo", "mon"],
        ["di", "tue"],
        ["mi", "wed"],
        ["do", "thu"],
        ["fr", "fri"],
        ["sa", "sat"],
        ["so", "sun"],
    ];
    if let Some(weekday) = NAMES.iter().position(|names| {
        names
            .iter()
            .any(|name| token.len() >= 2 && (token.starts_with(name) || name.starts_with(&token)))
    }) {
        // Der nächste solche Tag nach heute.
        let current = i64::from(today.weekday().number_days_from_monday());
        let mut days = (weekday as i64 - current).rem_euclid(7);
        if days == 0 {
            days = 7;
        }
        return Some((today.checked_add(time::Duration::days(days))?, false));
    }
    if let Some((year, rest)) = token.split_once('-') {
        let mut parts = rest.split('-');
        let month = parts.next()?.parse::<u8>().ok()?;
        let day = parts.next()?.parse::<u8>().ok()?;
        let date = time::Date::from_calendar_date(
            year.parse().ok()?,
            time::Month::try_from(month).ok()?,
            day,
        )
        .ok()?;
        return Some((date, true));
    }
    let mut parts = token.split('.').filter(|part| !part.is_empty());
    let day = parts.next()?.parse::<u8>().ok()?;
    let month = parts.next()?.parse::<u8>().ok()?;
    let (year, explicit) = match parts.next() {
        Some(year) => {
            let year = year.parse::<i32>().ok()?;
            (if year < 100 { 2000 + year } else { year }, true)
        }
        None => (today.year(), false),
    };
    let date =
        time::Date::from_calendar_date(year, time::Month::try_from(month).ok()?, day).ok()?;
    Some((date, explicit))
}

/// Sekunden bis zum nächsten 08:00 Ortszeit. Vor 08:00 ist das heute.
pub(super) fn secs_until_morning(local_secs_of_day: u64) -> u64 {
    if local_secs_of_day < MORNING_SECS {
        MORNING_SECS - local_secs_of_day
    } else {
        DAY_SECS - local_secs_of_day + MORNING_SECS
    }
}

fn local_secs_of_day() -> Option<u64> {
    let local = crate::platform::local_datetime()?;
    Some(
        u64::from(local.hour()) * 3_600
            + u64::from(local.minute()) * 60
            + u64::from(local.second()),
    )
}

/// Unix-Millisekunden eines Ortszeit-Ziels, gerechnet über den Abstand zur
/// Ortszeit jetzt. Eine Zeitumstellung dazwischen verschiebt es um höchstens
/// eine Stunde.
fn until_millis(
    target: time::PrimitiveDateTime,
    local_now: time::PrimitiveDateTime,
    now: u64,
) -> Option<u64> {
    let millis = u64::try_from((target - local_now).whole_milliseconds()).ok()?;
    Some(now + millis)
}

/// Ende eines Snooze ab `now` (Unix-Millisekunden); `None` hebt ihn auf.
fn snooze_until(choice: SnoozeChoice, now: u64, local_secs_of_day: u64) -> Option<u64> {
    match choice {
        SnoozeChoice::Hour => Some(now + HOUR_MS),
        SnoozeChoice::FourHours => Some(now + 4 * HOUR_MS),
        SnoozeChoice::Morning => Some(now + secs_until_morning(local_secs_of_day) * 1_000),
        // Ohne Ortszeit: ganze Tage ab jetzt.
        SnoozeChoice::Monday | SnoozeChoice::Week => Some(now + 7 * 24 * HOUR_MS),
        SnoozeChoice::Date | SnoozeChoice::Clear => None,
    }
}

impl ClientShellState {
    pub(super) fn open_snooze_picker(&mut self, outcome: &mut ClientShellInput) {
        let Some(cursor) = self.sidebar_focus.agent_cursor.clone() else {
            return;
        };
        let Some(row) = self.sidebar_chat_row(&cursor.pane_id) else {
            return;
        };
        let method = crate::api::schema::Method::WorkspaceSnooze(
            crate::api::schema::WorkspaceSnoozeParams {
                workspace_id: String::new(),
                until: None,
            },
        );
        if !self.supports_endpoint_method(&method) {
            outcome.repaint |= self.push_endpoint_notice(
                ClientEndpointNoticeKind::Unsupported,
                "workspace.snooze",
                "Action unavailable",
                "This server cannot snooze chats yet. Update and restart it to enable this action.",
            );
            return;
        }
        let morning_today = local_secs_of_day().is_some_and(|secs| secs < MORNING_SECS);
        self.overlay = Some(ClientShellOverlay::Snooze(ClientSnoozeOverlay {
            workspace_id: row.workspace_id,
            highlighted: 0,
            morning_today,
            local_now: crate::platform::local_datetime(),
            date_input: None,
        }));
        outcome.repaint = true;
    }

    pub(super) fn route_snooze_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(ClientShellOverlay::Snooze(picker)) = self.overlay.as_mut() else {
            return false;
        };
        let last = SNOOZE_CHOICES.len() - 1;
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        outcome.repaint = true;
        if let Some(input) = picker.date_input.as_mut() {
            match code {
                KeyCode::Esc => picker.date_input = None,
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Enter => self.apply_snooze(SnoozeChoice::Date, outcome),
                KeyCode::Char(ch)
                    if (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT)
                        && input.chars().count() < 24 =>
                {
                    input.push(ch);
                }
                _ => {}
            }
            return true;
        }
        let date_index = SNOOZE_CHOICES
            .iter()
            .position(|choice| *choice == SnoozeChoice::Date)
            .unwrap_or(last);
        match code {
            KeyCode::Esc => self.overlay = None,
            KeyCode::Up | KeyCode::Char('k') => {
                picker.highlighted = picker.highlighted.saturating_sub(1);
            }
            KeyCode::Char('p') if modifiers == KeyModifiers::CONTROL => {
                picker.highlighted = picker.highlighted.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                picker.highlighted = (picker.highlighted + 1).min(last);
            }
            KeyCode::Char('n') if modifiers == KeyModifiers::CONTROL => {
                picker.highlighted = (picker.highlighted + 1).min(last);
            }
            KeyCode::Enter => {
                let choice = SNOOZE_CHOICES[picker.highlighted.min(last)];
                if choice == SnoozeChoice::Date {
                    picker.date_input = picker.local_now.map(|_| String::new());
                } else {
                    self.apply_snooze(choice, outcome);
                }
            }
            KeyCode::Char(digit @ '1'..='5') if modifiers.is_empty() => {
                let choice = SNOOZE_CHOICES[usize::from(digit as u8 - b'1')];
                self.apply_snooze(choice, outcome);
            }
            KeyCode::Char('d') if modifiers.is_empty() => {
                picker.highlighted = date_index;
                picker.date_input = picker.local_now.map(|_| String::new());
            }
            KeyCode::Char('u') if modifiers.is_empty() => {
                self.apply_snooze(SnoozeChoice::Clear, outcome);
            }
            _ => {}
        }
        true
    }

    fn apply_snooze(&mut self, choice: SnoozeChoice, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Snooze(picker)) = self.overlay.as_ref() else {
            return;
        };
        let now = crate::workspace::unix_millis_now();
        let target = picker.local_now.and_then(|local_now| {
            let target = match choice {
                SnoozeChoice::Date => parse_snooze_date(picker.date_input.as_deref()?, local_now)?,
                _ => local_target(choice, local_now)?,
            };
            until_millis(target, local_now, now)
        });
        // Ein Datum, das nicht passt, lässt den Picker offen.
        if choice == SnoozeChoice::Date && target.is_none() {
            return;
        }
        let Some(ClientShellOverlay::Snooze(picker)) = self.overlay.take() else {
            return;
        };
        let until = target.or_else(|| snooze_until(choice, now, local_secs_of_day().unwrap_or(0)));
        // Der Chat wechselt die Ansicht; der Cursor rückt auf den Nachbarn.
        let targets = self.sidebar_agent_targets();
        let next = self
            .sidebar_focus
            .agent_cursor
            .as_ref()
            .and_then(|cursor| targets.iter().position(|target| target == cursor))
            .and_then(|index| {
                targets
                    .get(index + 1)
                    .or_else(|| index.checked_sub(1).and_then(|prev| targets.get(prev)))
            })
            .cloned();
        self.push_endpoint_method(
            crate::api::schema::Method::WorkspaceSnooze(
                crate::api::schema::WorkspaceSnoozeParams {
                    workspace_id: picker.workspace_id,
                    until,
                },
            ),
            outcome,
        );
        self.sidebar_focus.agent_cursor = next;
        outcome.repaint = true;
    }

    /// Neu zeichnen, wenn ein Snooze abläuft, der Spinner weiterdreht, die
    /// Restzeit springt oder eine Karte nachleuchtet. Im leeren Overlay döst
    /// die Collie.
    pub(crate) fn tick_chat_sidebar(&mut self) -> bool {
        if !self.spaces_hidden() {
            return false;
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return false;
        };
        let now = crate::workspace::unix_millis_now();
        let snoozed = snapshot
            .workspaces
            .iter()
            .filter(|workspace| workspace.snoozed_until.is_some_and(|until| until > now))
            .count() as u64;
        let mut key = snoozed;
        let view = self
            .sidebar_focus
            .render_view(self.mode, self.prefix_return_navigate);
        // collie: Die Leiste zeigt laufende Chats mit Spinner.
        let rail = self.chat_overlay_enabled() && self.hits.sidebar_line.width > 1;
        if (view.show_hidden && !self.sidebar_collapsed) || self.chat_overlay_open() || rail {
            let spinning = snapshot
                .agents
                .iter()
                .any(|agent| agent.agent_status == crate::api::schema::AgentStatus::Working);
            if spinning {
                key = key
                    .wrapping_mul(31)
                    .wrapping_add(now / super::chats::SPINNER_FRAME_MS);
            }
            if snoozed > 0 {
                key = key.wrapping_mul(31).wrapping_add(now / 60_000);
            }
        }
        if self.chat_overlay_open() {
            // Wärmestreifen und Aktivitätskurve wandern minütlich weiter.
            key = key.wrapping_mul(31).wrapping_add(now / 60_000);
            if snapshot
                .workspaces
                .iter()
                .any(|workspace| super::chat_visuals::pulsing(workspace.attention_since, now))
            {
                key = key
                    .wrapping_mul(31)
                    .wrapping_add(now / super::chats::SPINNER_FRAME_MS);
            }
            if super::chats::visible_chat_rows(snapshot, false, false, view.agent_query, now)
                .is_empty()
            {
                key = key
                    .wrapping_mul(31)
                    .wrapping_add(now / super::chat_visuals::DOZE_FRAME_MS);
            }
        }
        if let Some(phase) = self.chat_pulse_phase(now) {
            key = key.wrapping_mul(31).wrapping_add(phase + 1);
        }
        let changed = self.chat_tick_key != Some(key);
        self.chat_tick_key = Some(key);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morning_is_today_before_eight_and_tomorrow_after() {
        assert_eq!(secs_until_morning(7 * 3_600), 3_600);
        assert_eq!(secs_until_morning(8 * 3_600), DAY_SECS);
        assert_eq!(secs_until_morning(22 * 3_600), 10 * 3_600);
    }

    #[test]
    fn snooze_choices_map_to_deadlines() {
        let now = 1_000_000;
        assert_eq!(
            snooze_until(SnoozeChoice::Hour, now, 0),
            Some(now + HOUR_MS)
        );
        assert_eq!(
            snooze_until(SnoozeChoice::Morning, now, 7 * 3_600),
            Some(now + HOUR_MS)
        );
        assert_eq!(snooze_until(SnoozeChoice::Clear, now, 0), None);
    }

    /// Mittwoch, 18.11.2026, 15:30.
    fn local_now() -> time::PrimitiveDateTime {
        time::PrimitiveDateTime::new(
            time::Date::from_calendar_date(2026, time::Month::November, 18).unwrap(),
            time::Time::from_hms(15, 30, 0).unwrap(),
        )
    }

    fn parsed(input: &str) -> Option<String> {
        parse_snooze_date(input, local_now()).map(date_label)
    }

    #[test]
    fn free_dates_parse_in_common_forms() {
        assert_eq!(parsed("25.11.").as_deref(), Some("Mi 25.11. 08:00"));
        assert_eq!(parsed("25.11. 14:00").as_deref(), Some("Mi 25.11. 14:00"));
        assert_eq!(parsed("25.11.2026 9").as_deref(), Some("Mi 25.11. 09:00"));
        assert_eq!(parsed("2026-12-01").as_deref(), Some("Di 01.12. 08:00"));
        assert_eq!(parsed("+3").as_deref(), Some("Sa 21.11. 08:00"));
        assert_eq!(parsed("fr").as_deref(), Some("Fr 20.11. 08:00"));
        assert_eq!(parsed("mi").as_deref(), Some("Mi 25.11. 08:00"));
        assert_eq!(parsed("morgen 10:30").as_deref(), Some("Do 19.11. 10:30"));
        assert_eq!(parsed("17:00").as_deref(), Some("Mi 18.11. 17:00"));
        assert_eq!(parsed("09:00").as_deref(), Some("Do 19.11. 09:00"));
    }

    #[test]
    fn past_dates_roll_into_next_year_or_fail() {
        let next = parse_snooze_date("1.3.", local_now()).unwrap();
        assert_eq!(next.year(), 2027);
        assert_eq!(parsed("1.3.2026"), None);
        assert_eq!(parsed("heute"), None);
        assert_eq!(parsed("31.02."), None);
        assert_eq!(parsed("quatsch"), None);
    }

    #[test]
    fn monday_and_week_land_on_eight_in_the_morning() {
        let now = local_now();
        assert_eq!(
            local_target(SnoozeChoice::Monday, now)
                .map(date_label)
                .as_deref(),
            Some("Mo 23.11. 08:00")
        );
        assert_eq!(
            local_target(SnoozeChoice::Week, now)
                .map(date_label)
                .as_deref(),
            Some("Mi 25.11. 08:00")
        );
        let target = local_target(SnoozeChoice::Week, now).unwrap();
        assert_eq!(
            until_millis(target, now, 1_000),
            Some(1_000 + (6 * 24 * 3_600 + 16 * 3_600 + 30 * 60) * 1_000)
        );
    }
}
