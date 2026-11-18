//! collie: Tagesübersicht, siehe docs/specs/user-inbox.md.
//!
//! Werktags zur eingestellten Uhrzeit (`master.daily_digest`, Standard
//! `08:00`) sammelt der Server, was seit der letzten Übersicht fertig wurde
//! und was blockiert ist oder auf den Nutzer wartet. Läuft ein Master, fasst
//! er die Liste zusammen und legt sie selbst in die Inbox. Ohne Master legt
//! der Server die Liste als Nachricht ab.

use std::time::{Duration, Instant};

use time::{Date, PrimitiveDateTime, Time, Weekday};

use crate::api::schema::AgentStatus;
use crate::app::App;
use crate::workspace::{DigestPrompt, UserInboxEntry, UserInboxKind};

/// Kopfzeile des Prompts an den Master.
const DIGEST_HEADER: &str = "[collie digest]";
/// So lange wartet die Übersicht auf einen beschäftigten Master. Danach
/// kommt die Liste ohne Zusammenfassung.
const MASTER_WAIT_MS: u64 = 2 * 60 * 60_000;
/// Spätestens dann prüft der Server die Uhr erneut, auch über
/// Zeitumstellungen hinweg.
const MAX_SLEEP: Duration = Duration::from_secs(60 * 60);

#[cfg(test)]
thread_local! {
    /// Uhr für Tests; ohne Wert gibt es in Tests keine Übersicht.
    static TEST_NOW: std::cell::Cell<Option<PrimitiveDateTime>> =
        const { std::cell::Cell::new(None) };
}

/// Ortszeit; in Tests die gesetzte Testuhr.
fn local_now() -> Option<PrimitiveDateTime> {
    #[cfg(test)]
    {
        TEST_NOW.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        crate::platform::local_datetime()
    }
}

/// `08:00` oder `8:30`; leer oder `off` schaltet die Übersicht ab.
pub(crate) fn parse_digest_time(value: &str) -> Option<Time> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("off") {
        return None;
    }
    let (hour, minute) = value.split_once(':').unwrap_or((value, "0"));
    Time::from_hms(hour.trim().parse().ok()?, minute.trim().parse().ok()?, 0).ok()
}

fn workday(date: Date) -> bool {
    !matches!(date.weekday(), Weekday::Saturday | Weekday::Sunday)
}

/// Termin des letzten Werktags vor `date`.
fn previous_slot(date: Date, at: Time) -> Option<PrimitiveDateTime> {
    let mut day = date.previous_day()?;
    while !workday(day) {
        day = day.previous_day()?;
    }
    Some(PrimitiveDateTime::new(day, at))
}

/// Ist die Übersicht für heute fällig, liefert das den Tag und den Beginn des
/// Zeitraums, den sie abdeckt: den Termin des letzten Werktags.
pub(crate) fn digest_due(now: PrimitiveDateTime, at: Time) -> Option<(Date, PrimitiveDateTime)> {
    if !workday(now.date()) || now.time() < at {
        return None;
    }
    Some((now.date(), previous_slot(now.date(), at)?))
}

/// Nächster Termin nach `now`.
pub(crate) fn next_slot(now: PrimitiveDateTime, at: Time) -> Option<PrimitiveDateTime> {
    let mut day = now.date();
    if now.time() >= at {
        day = day.next_day()?;
    }
    while !workday(day) {
        day = day.next_day()?;
    }
    Some(PrimitiveDateTime::new(day, at))
}

fn day_key(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// `Tagesübersicht Mi 18.11.`
pub(crate) fn digest_title(date: Date) -> String {
    let weekday = match date.weekday() {
        Weekday::Monday => "Mo",
        Weekday::Tuesday => "Di",
        Weekday::Wednesday => "Mi",
        Weekday::Thursday => "Do",
        Weekday::Friday => "Fr",
        Weekday::Saturday => "Sa",
        Weekday::Sunday => "So",
    };
    format!(
        "Tagesübersicht {weekday} {:02}.{:02}.",
        date.day(),
        u8::from(date.month())
    )
}

/// Ein Chat, wie ihn die Übersicht sieht.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DigestChat {
    pub(crate) label: String,
    pub(crate) status: AgentStatus,
    pub(crate) attention_since: Option<u64>,
    pub(crate) attention_seen: bool,
    pub(crate) snoozed_until: Option<u64>,
    /// Grund der Blockade, worauf der Agent wartet oder seine letzte Zeile.
    pub(crate) note: Option<String>,
}

fn line(chat: &DigestChat) -> String {
    match chat
        .note
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty())
    {
        Some(note) => format!("- **{}**: {note}", chat.label),
        None => format!("- **{}**", chat.label),
    }
}

/// Die Liste als Markdown: blockiert, wartet auf dich, fertig geworden.
/// `since` und `now` in Unix-Millisekunden.
pub(crate) fn digest_markdown(chats: &[DigestChat], since: u64, now: u64) -> String {
    let resting = |chat: &DigestChat| matches!(chat.status, AgentStatus::Done | AgentStatus::Idle);
    let snoozed = |chat: &DigestChat| chat.snoozed_until.is_some_and(|until| until > now);
    let blocked = chats
        .iter()
        .filter(|chat| chat.status == AgentStatus::Blocked)
        .map(line)
        .collect::<Vec<_>>();
    let waiting = chats
        .iter()
        .filter(|chat| resting(chat) && !chat.attention_seen && !snoozed(chat))
        .map(line)
        .collect::<Vec<_>>();
    let done = chats
        .iter()
        .filter(|chat| {
            resting(chat)
                && chat.attention_seen
                && chat
                    .attention_since
                    .is_some_and(|at| at >= since && at < now)
        })
        .map(line)
        .collect::<Vec<_>>();
    let mut sections = Vec::new();
    for (heading, lines) in [
        ("Blockiert", blocked),
        ("Wartet auf dich", waiting),
        ("Fertig geworden", done),
    ] {
        if !lines.is_empty() {
            sections.push(format!("## {heading}\n\n{}", lines.join("\n")));
        }
    }
    if sections.is_empty() {
        "Nichts Neues: kein Chat ist blockiert, keiner wartet auf dich, und seit der letzten Übersicht ist nichts fertig geworden.".to_owned()
    } else {
        sections.join("\n\n")
    }
}

/// Prompt an den Master: Liste plus Auftrag zur Zusammenfassung.
fn master_prompt(title: &str, list: &str) -> String {
    format!(
        "{DIGEST_HEADER} {title}\n\n{list}\n\nFasse das für den Nutzer in wenigen Sätzen zusammen: was ist fertig, was blockiert, womit sollte er heute anfangen. Lege die Zusammenfassung mit `herdr chat tell-user --title \"{title}\" -` in seine Inbox und antworte sonst nichts."
    )
}

impl App {
    /// Uhrzeit der Übersicht aus der Config; `None`, wenn abgeschaltet.
    fn digest_time(&self) -> Option<Time> {
        parse_digest_time(&self.daily_digest)
    }

    /// Erstellt fällige Übersichten und stellt sie zu. Liefert `(geändert,
    /// wartet)` wie die übrige Inbox-Zustellung.
    pub(super) fn run_daily_digest(&mut self, millis: u64) -> (bool, bool) {
        let mut changed = false;
        let mut waiting = false;
        let (delivered, pending) = self.deliver_digest_prompts(millis);
        changed |= delivered;
        waiting |= pending;
        let (Some(at), Some(local)) = (self.digest_time(), local_now()) else {
            return (changed, waiting);
        };
        let Some((today, start)) = digest_due(local, at) else {
            return (changed, waiting);
        };
        let key = day_key(today);
        if self
            .state
            .workspaces
            .iter()
            .any(|ws| ws.chat.team.digest_day.as_deref() == Some(key.as_str()))
        {
            return (changed, waiting);
        }
        let since = u64::try_from((local - start).whole_milliseconds())
            .map_or(0, |ago| millis.saturating_sub(ago));
        let title = digest_title(today);
        let list = digest_markdown(&self.digest_chats(), since, millis);
        if let Some(master) = self.running_master() {
            let team = &mut self.state.workspaces[master].chat.team;
            team.digest_day = Some(key);
            team.digest_prompt = Some(DigestPrompt {
                at: millis,
                title,
                list,
            });
            waiting = true;
        } else if let Some(holder) = self.digest_holder() {
            self.state.workspaces[holder].chat.team.digest_day = Some(key);
            self.push_digest_entry(holder, &title, list, millis);
            changed = true;
        }
        self.schedule_session_save();
        (changed, waiting)
    }

    /// Wann der Server die Uhr wieder prüfen muss.
    pub(super) fn next_digest_check(&self, now: Instant) -> Option<Instant> {
        let at = self.digest_time()?;
        let local = local_now()?;
        let next = next_slot(local, at)?;
        let wait = Duration::try_from(next - local).unwrap_or(Duration::ZERO);
        Some(now + wait.min(MAX_SLEEP))
    }

    /// Schickt wartende Master-Prompts, sobald der Master frei ist. Ist er
    /// zu lange beschäftigt oder gestoppt, kommt die Liste ohne
    /// Zusammenfassung.
    fn deliver_digest_prompts(&mut self, millis: u64) -> (bool, bool) {
        let mut changed = false;
        let mut waiting = false;
        for master in 0..self.state.workspaces.len() {
            let Some(prompt) = self.state.workspaces[master]
                .chat
                .team
                .digest_prompt
                .clone()
            else {
                continue;
            };
            let ready = self.user_replies_ready(master, prompt.at, millis);
            let delivered = ready == Some(true)
                && self.chat_agent_pane(master).is_some_and(|pane| {
                    self.submit_to_pane(master, pane, &master_prompt(&prompt.title, &prompt.list))
                });
            if delivered {
                self.state.workspaces[master].chat.team.digest_prompt = None;
                self.schedule_session_save();
                changed = true;
            } else if ready.is_none() || millis.saturating_sub(prompt.at) >= MASTER_WAIT_MS {
                self.state.workspaces[master].chat.team.digest_prompt = None;
                self.push_digest_entry(master, &prompt.title, prompt.list, millis);
                self.schedule_session_save();
                changed = true;
            } else {
                waiting = true;
            }
        }
        (changed, waiting)
    }

    fn push_digest_entry(&mut self, holder: usize, title: &str, text: String, millis: u64) {
        self.state.workspaces[holder]
            .chat
            .team
            .push_user_inbox(UserInboxEntry {
                id: 0,
                at: millis,
                kind: UserInboxKind::Nachricht,
                important: true,
                title: title.to_owned(),
                text,
                path: None,
                key: None,
                read: false,
                finished: false,
                deleted: false,
            });
    }

    /// Ein laufender Master, der die Übersicht zusammenfassen kann.
    fn running_master(&self) -> Option<usize> {
        (0..self.state.workspaces.len()).find(|&index| {
            self.state.workspaces[index].chat.team.is_master()
                && self.stopped_chat_agent(index).is_none()
                && self.chat_agent_running(index)
        })
    }

    /// Chat, in dessen Inbox die Liste liegt: ein Master, sonst der erste
    /// Chat außer dem Inbox-Space.
    fn digest_holder(&self) -> Option<usize> {
        let workspaces = &self.state.workspaces;
        (0..workspaces.len())
            .find(|&index| workspaces[index].chat.team.is_master())
            .or_else(|| {
                (0..workspaces.len()).find(|&index| !workspaces[index].chat.team.user_inbox_space)
            })
    }

    fn digest_chats(&self) -> Vec<DigestChat> {
        (0..self.state.workspaces.len())
            .filter(|&index| !self.state.workspaces[index].chat.team.user_inbox_space)
            .map(|index| {
                let info = self.workspace_info(index);
                let note = if info.agent_status == AgentStatus::Blocked {
                    info.blocked_reason.or(info.awaiting).or(info.last_message)
                } else {
                    info.last_message
                };
                DigestChat {
                    label: info.label,
                    status: info.agent_status,
                    attention_since: info.attention_since,
                    attention_seen: info.attention_seen.unwrap_or(true),
                    snoozed_until: info.snoozed_until,
                    note,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `dt(2026, 11, 18, 8, 0)`
    fn dt(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> PrimitiveDateTime {
        let month = time::Month::try_from(month).expect("month");
        PrimitiveDateTime::new(
            Date::from_calendar_date(year, month, day).expect("date"),
            Time::from_hms(hour, minute, 0).expect("time"),
        )
    }

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
            crate::workspace::Workspace::test_new("plain"),
            crate::workspace::Workspace::test_new("master"),
        ];
        app.state.workspaces[1].chat.team.role = crate::workspace::ChatRole::Master;
        app
    }

    #[test]
    fn without_a_running_master_the_list_lands_in_the_inbox_once_a_day() {
        let mut app = test_app();
        TEST_NOW.with(|now| now.set(Some(dt(2026, 11, 18, 7, 59))));
        assert_eq!(app.run_daily_digest(1_000), (false, false));
        assert!(app.state.workspaces[1].chat.team.user_inbox.is_empty());

        TEST_NOW.with(|now| now.set(Some(dt(2026, 11, 18, 9, 0))));
        assert_eq!(app.run_daily_digest(2_000), (true, false));
        let team = &app.state.workspaces[1].chat.team;
        assert_eq!(team.digest_day.as_deref(), Some("2026-11-18"));
        assert_eq!(team.user_inbox.len(), 1);
        assert_eq!(team.user_inbox[0].title, "Tagesübersicht Mi 18.11.");
        assert!(team.user_inbox[0].important);
        assert!(app.state.workspaces[0].chat.team.user_inbox.is_empty());

        // Ein zweiter Lauf am selben Tag legt nichts mehr ab.
        assert_eq!(app.run_daily_digest(3_000), (false, false));
        assert_eq!(app.state.workspaces[1].chat.team.user_inbox.len(), 1);

        // Abgeschaltet: nichts, auch am nächsten Tag nicht.
        app.daily_digest = "off".into();
        TEST_NOW.with(|now| now.set(Some(dt(2026, 11, 19, 9, 0))));
        assert_eq!(app.run_daily_digest(4_000), (false, false));
        assert!(app.next_digest_check(Instant::now()).is_none());
        TEST_NOW.with(|now| now.set(None));
    }

    #[test]
    fn a_pending_master_prompt_falls_back_to_the_list() {
        let mut app = test_app();
        app.state.workspaces[1].chat.team.digest_prompt = Some(DigestPrompt {
            at: 0,
            title: "Tagesübersicht Mi 18.11.".into(),
            list: "## Blockiert".into(),
        });
        // Der Master hat kein laufendes Agent-Pane: die Liste kommt direkt.
        assert_eq!(app.run_daily_digest(1_000), (true, false));
        let team = &app.state.workspaces[1].chat.team;
        assert!(team.digest_prompt.is_none());
        assert_eq!(team.user_inbox[0].text, "## Blockiert");
    }

    #[test]
    fn digest_time_parses_and_switches_off() {
        assert_eq!(parse_digest_time("08:00"), Time::from_hms(8, 0, 0).ok());
        assert_eq!(parse_digest_time(" 7:30 "), Time::from_hms(7, 30, 0).ok());
        assert_eq!(parse_digest_time("9"), Time::from_hms(9, 0, 0).ok());
        assert_eq!(parse_digest_time(""), None);
        assert_eq!(parse_digest_time("off"), None);
        assert_eq!(parse_digest_time("25:00"), None);
    }

    #[test]
    fn the_digest_is_due_on_workdays_after_its_time() {
        let at = Time::from_hms(8, 0, 0).expect("time");
        // Mittwoch vor acht: noch nicht.
        assert_eq!(digest_due(dt(2026, 11, 18, 7, 59), at), None);
        // Mittwoch danach: ab Dienstag acht.
        assert_eq!(
            digest_due(dt(2026, 11, 18, 9, 15), at),
            Some((dt(2026, 11, 18, 0, 0).date(), dt(2026, 11, 17, 8, 0)))
        );
        // Montag deckt das Wochenende ab Freitag ab.
        assert_eq!(
            digest_due(dt(2026, 11, 23, 8, 0), at).map(|(_, start)| start),
            Some(dt(2026, 11, 20, 8, 0))
        );
        // Samstag nie.
        assert_eq!(digest_due(dt(2026, 11, 21, 10, 0), at), None);
    }

    #[test]
    fn the_next_slot_skips_the_weekend() {
        let at = Time::from_hms(8, 0, 0).expect("time");
        assert_eq!(
            next_slot(dt(2026, 11, 18, 7, 0), at),
            Some(dt(2026, 11, 18, 8, 0))
        );
        assert_eq!(
            next_slot(dt(2026, 11, 20, 8, 0), at),
            Some(dt(2026, 11, 23, 8, 0))
        );
    }

    #[test]
    fn the_title_names_the_weekday_and_date() {
        assert_eq!(
            digest_title(dt(2026, 11, 18, 0, 0).date()),
            "Tagesübersicht Mi 18.11."
        );
    }

    fn chat(label: &str, status: AgentStatus, since: u64, seen: bool) -> DigestChat {
        DigestChat {
            label: label.into(),
            status,
            attention_since: Some(since),
            attention_seen: seen,
            snoozed_until: None,
            note: Some(format!("{label} sagt hallo")),
        }
    }

    #[test]
    fn the_list_sorts_chats_into_blocked_waiting_and_done() {
        let chats = [
            chat("Login", AgentStatus::Blocked, 50, false),
            chat("Snooze-Fix", AgentStatus::Done, 150, false),
            chat("Doku", AgentStatus::Done, 150, true),
            chat("Alt", AgentStatus::Done, 50, true),
            chat("Läuft", AgentStatus::Working, 150, true),
            DigestChat {
                snoozed_until: Some(1_000),
                ..chat("Schläft", AgentStatus::Done, 150, false)
            },
        ];
        let text = digest_markdown(&chats, 100, 200);
        assert_eq!(
            text,
            "## Blockiert\n\n- **Login**: Login sagt hallo\n\n\
             ## Wartet auf dich\n\n- **Snooze-Fix**: Snooze-Fix sagt hallo\n\n\
             ## Fertig geworden\n\n- **Doku**: Doku sagt hallo"
        );
    }

    #[test]
    fn an_empty_day_says_so() {
        assert!(digest_markdown(&[], 0, 1).starts_with("Nichts Neues"));
    }

    #[test]
    fn the_master_prompt_is_a_collie_prompt_with_the_title() {
        let prompt = master_prompt("Tagesübersicht Mi 18.11.", "## Blockiert");
        assert!(prompt.starts_with("[collie digest] Tagesübersicht Mi 18.11."));
        assert!(prompt.contains("--title \"Tagesübersicht Mi 18.11.\""));
    }
}
