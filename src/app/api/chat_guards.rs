//! collie: Schutz davor, dass Aufgaben und Prompts im Chat verloren gehen.
//!
//! - Endet ein Master-Turn mit Aufgaben oder Fragen für den Nutzer, ohne dass
//!   der Master etwas per `chat.tell_user` abgelegt hat, bekommt er einmal
//!   einen Hinweis.
//! - Steht in einem Chat ein Claude-Code-Dialog offen, tippt collie keinen
//!   Prompt hinein. Der Dialog würde ihn verschlucken.

use std::time::Instant;

use crate::app::App;
use crate::workspace::{ChatInboxEntry, ChatInboxKind};

/// Kopfzeile des Hinweises an den Master. Beginnt wie alle collie-Prompts mit
/// `[collie `, damit die Antwort darauf nicht in der Nutzer-Inbox landet.
pub(crate) const NUDGE_HEADER: &str = "[collie hinweis]";
/// Text des Hinweises.
const NUDGE_TEXT: &str = "Deine letzte Antwort enthält Aufgaben oder Fragen für den Nutzer, \
aber nichts ging in die Inbox. Lege sie als Wizard oder Formular per tell-user ab. \
War nichts davon für den Nutzer bestimmt, antworte nur mit ok.";
/// So viele nicht leere Zeilen vom unteren Rand sucht collie nach dem Footer
/// eines Dialogs.
const DIALOG_FOOTER_LINES: usize = 10;
/// Wendungen, mit denen eine Antwort den Nutzer etwas tun oder entscheiden
/// lässt. Kleingeschrieben.
const USER_TASK_PHRASES: [&str; 20] = [
    "auf dem laptop",
    "auf deinem laptop",
    "bitte",
    "für dich",
    "deine aufgabe",
    "sag bescheid",
    "sag mir bescheid",
    "gib bescheid",
    "gib mir bescheid",
    "melde dich",
    "du musst",
    "musst du",
    "kannst du",
    "könntest du",
    "dein go",
    "dein ok",
    "deine freigabe",
    "deine entscheidung",
    "entscheide",
    "führ das aus",
];
/// Wendungen, mit denen eine Antwort auf einen Eintrag verweist, der schon
/// in der Inbox liegt. Kleingeschrieben.
const INBOX_REFERENCES: [&str; 4] = [
    "in deiner inbox",
    "in der inbox",
    "in deine inbox",
    "liegt in der inbox",
];
/// Sprachen von Codeblöcken, die nur der Nutzer ausführen kann.
const USER_SHELL_LANGS: [&str; 5] = ["powershell", "pwsh", "ps1", "cmd", "bat"];

impl App {
    /// `chat.tell_user` wurde aus diesem Chat aufgerufen.
    pub(super) fn note_told_user(&mut self, index: usize) {
        if let Some(ws) = self.state.workspaces.get_mut(index) {
            ws.chat.told_user = true;
        }
    }

    /// Das Stop-Hook meldet das Ende eines Turns. Enthält die Antwort eines
    /// Masters Aufgaben für den Nutzer und hat er in diesem Turn nichts in
    /// die Inbox gelegt, wartet ab jetzt ein Hinweis auf ihn. Auf den Turn,
    /// den der Hinweis selbst auslöst, folgt keiner mehr.
    pub(super) fn note_master_turn_end(
        &mut self,
        index: usize,
        answer: Option<&str>,
        prompt: Option<&str>,
    ) {
        let Some(ws) = self.state.workspaces.get_mut(index) else {
            return;
        };
        let told = std::mem::take(&mut ws.chat.told_user);
        if !ws.chat.team.is_master() || told {
            return;
        }
        let Some(answer) = answer else {
            return;
        };
        if prompt.is_some_and(|prompt| prompt.trim_start().starts_with(NUDGE_HEADER))
            || !answer_has_user_tasks(answer)
        {
            return;
        }
        // Verweist die Antwort nur auf etwas, das schon offen in der Inbox
        // liegt, etwa „wartet auf deine Freigabe“, ist nichts verloren.
        // Neue Befehle gehen trotzdem immer in einen Hinweis.
        let open_entries = ws.chat.team.user_inbox.iter().any(|entry| {
            entry.kind != crate::workspace::UserInboxKind::Antwort
                && !entry.finished
                && !entry.deleted
        });
        if !answer_has_user_commands(answer) && (open_entries || answer_points_to_inbox(answer)) {
            return;
        }
        ws.chat.inbox_nudge_at = Some(crate::workspace::unix_millis_now());
        tracing::info!(
            workspace = %ws.id,
            "master answer holds tasks for the user outside the inbox"
        );
        self.chat_inbox_deadline = Some(Instant::now());
    }

    /// Stellt wartende Hinweise zu, sobald der Master frei ist. Liefert
    /// `(geändert, wartet)`.
    pub(super) fn deliver_inbox_nudges(&mut self, millis: u64) -> (bool, bool) {
        let mut changed = false;
        let mut waiting = false;
        for master in 0..self.state.workspaces.len() {
            let Some(at) = self.state.workspaces[master].chat.inbox_nudge_at else {
                continue;
            };
            match self.user_replies_ready(master, at, millis) {
                Some(true) => {
                    let sent = self.chat_agent_pane(master).is_some_and(|pane| {
                        self.submit_to_pane(master, pane, &format!("{NUDGE_HEADER}\n{NUDGE_TEXT}"))
                    });
                    if sent {
                        self.state.workspaces[master].chat.inbox_nudge_at = None;
                        changed = true;
                    } else {
                        waiting = true;
                    }
                }
                Some(false) => waiting = true,
                // Ein gestoppter Master braucht den Hinweis nicht mehr.
                None => self.state.workspaces[master].chat.inbox_nudge_at = None,
            }
        }
        (changed, waiting)
    }

    /// Titel eines offenen Claude-Code-Dialogs im Pane, sonst `None`. Liest
    /// den unteren Puffer wie die Erkennung, nicht den scrollbaren Viewport.
    pub(super) fn chat_dialog_open(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
    ) -> Option<String> {
        let runtime = self.lookup_runtime_sender(ws_idx, pane_id)?;
        dialog_title(&runtime.detection_text())
    }

    /// Ein Prompt kam wegen eines offenen Dialogs nicht an. Ein Child meldet
    /// das seinem Master als `blocked`.
    pub(super) fn report_chat_dialog(&mut self, ws_idx: usize, title: &str) {
        let Some(ws) = self.state.workspaces.get_mut(ws_idx) else {
            return;
        };
        let Some(parent) = ws.chat.team.parent().map(str::to_owned) else {
            return;
        };
        let from = ws.id.clone();
        let text = format!("Dialog offen: {title}. collie hat den Prompt nicht eingetippt.");
        let Some(master) = self.state.workspaces.iter_mut().find(|ws| ws.id == parent) else {
            return;
        };
        let known = master
            .chat
            .team
            .pending_inbox()
            .any(|entry| entry.from == from && entry.text.as_deref() == Some(text.as_str()));
        if known {
            return;
        }
        master.chat.team.push_inbox(ChatInboxEntry {
            at: crate::workspace::unix_millis_now(),
            from,
            kind: ChatInboxKind::Blocked,
            text: Some(text),
            delivered: false,
        });
        self.chat_inbox_deadline = Some(Instant::now());
        self.schedule_session_save();
    }
}

/// Enthält eine Antwort neue Befehle: `! `-Zeilen oder Codeblöcke?
pub(crate) fn answer_has_user_commands(answer: &str) -> bool {
    answer.lines().map(str::trim).any(|line| {
        line.starts_with("```")
            || line
                .trim_start_matches(['`', '-', '*', ' '])
                .starts_with("! ")
    })
}

/// Verweist eine Antwort auf die Inbox des Nutzers?
pub(crate) fn answer_points_to_inbox(answer: &str) -> bool {
    let lower = answer.to_lowercase();
    INBOX_REFERENCES.iter().any(|phrase| lower.contains(phrase))
}

/// Enthält eine Antwort erkennbar Aufgaben oder Fragen für den Nutzer?
/// Bewusst grob: Ein Hinweis zu viel kostet einen kurzen Turn, eine
/// verlorene Aufgabe kostet den Nutzer Tage.
pub(crate) fn answer_has_user_tasks(answer: &str) -> bool {
    let mut in_code = false;
    for raw in answer.lines() {
        let line = raw.trim();
        if let Some(lang) = line.strip_prefix("```") {
            if !in_code {
                let lang = lang.trim().to_lowercase();
                if USER_SHELL_LANGS.contains(&lang.as_str()) {
                    return true;
                }
            }
            in_code = !in_code;
            continue;
        }
        // `! befehl` tippt der Nutzer in Claude Code ein.
        let command = line.trim_start_matches(['`', '-', '*', ' ']);
        if command.starts_with("! ") {
            return true;
        }
        if in_code {
            continue;
        }
        let prose = line.trim_end_matches(['*', '_', '`', ' ']);
        if prose.ends_with('?') {
            return true;
        }
    }
    let lower = answer.to_lowercase();
    USER_TASK_PHRASES
        .iter()
        .any(|phrase| lower.contains(phrase))
}

/// Titel eines offenen Auswahl-Dialogs von Claude Code. Solche Dialoge
/// enden mit einem Footer wie `Enter to confirm · Esc to cancel`. Als Titel
/// gilt die letzte Frage darüber, sonst `Claude-Code-Dialog`.
pub(crate) fn dialog_title(screen: &str) -> Option<String> {
    let lines = screen
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let bottom = &lines[lines.len().saturating_sub(DIALOG_FOOTER_LINES)..];
    let footer = bottom.iter().rposition(|line| {
        line.to_lowercase()
            .trim_end_matches(['.', ')'])
            .ends_with("esc to cancel")
    })?;
    let footer = lines.len() - bottom.len() + footer;
    let title = lines[..footer]
        .iter()
        .rev()
        .take(40)
        .find(|line| line.ends_with('?'))
        .map(|line| line.trim_start_matches(['│', '|', ' ']).to_owned())
        .unwrap_or_else(|| "Claude-Code-Dialog".to_owned());
    Some(title)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::workspace::ChatRole;

    #[test]
    fn user_tasks_are_found_in_commands_questions_and_phrases() {
        let tasks = [
            "Erzeuge den Schlüssel:\n\n! ssh-keygen -t ed25519",
            "Auf dem Laptop:\n```powershell\nGet-Service sshd\n```",
            "```bash\nssh -p 2222 localhost\n```\nDas führst du auf dem Laptop aus.",
            "Soll ich #54 mergen?",
            "Soll ich **#54 mergen?**",
            "Sag Bescheid, wenn der Tunnel steht.",
            "Bitte gib das Secret frei.",
            "Das ist deine Aufgabe.",
            "- `! herdr update`",
        ];
        for answer in tasks {
            assert!(answer_has_user_tasks(answer), "{answer}");
        }
    }

    #[test]
    fn status_reports_without_tasks_need_no_inbox() {
        let reports = [
            "w4M ist fertig. #54 ist gemergt.",
            "| Child | Status |\n| --- | --- |\n| w4M | fertig |",
            // Ein Fragezeichen in Code und eine Bash-Zeile des Masters selbst.
            "Ich habe geprüft:\n```bash\ntest -f x || echo fehlt?\n```\nAlles grün.",
        ];
        for answer in reports {
            assert!(!answer_has_user_tasks(answer), "{answer}");
        }
    }

    #[test]
    fn dialog_title_finds_the_footer_at_the_bottom() {
        let screen = "\
● Fertig.

 Teach auto mode about your environment?
 Claude Code reads this project.

 ❯ 1. Yes
   2. No

 Enter to confirm · Esc to cancel
";
        assert_eq!(
            dialog_title(screen).as_deref(),
            Some("Teach auto mode about your environment?")
        );
        assert_eq!(
            dialog_title("Wähle\n❯ A\n  B\nEsc to cancel").as_deref(),
            Some("Claude-Code-Dialog")
        );
    }

    /// Bildschirm des echten Dialogs aus Claude Code 2.1.331.
    pub(crate) const TEACH_AUTO_MODE_SCREEN: &str = "\
● Fertig, PR ist offen.

────────────────────────────────────────────────────────────────────────
 Teach auto mode about your environment?

 Claude Code reads this project, your recent Claude sessions, and
 optionally your shell history and other repositories.

 How you use Claude here
 ❯ ◉ Open source
   ◯ Hobby
   ◯ Mixed

 ◻ Also scan shell history
 ◻ Also scan your other repos

 ↑/↓ to navigate · Space to toggle · Esc to cancel
";

    #[test]
    fn the_teach_auto_mode_dialog_counts_as_open() {
        assert_eq!(
            dialog_title(TEACH_AUTO_MODE_SCREEN).as_deref(),
            Some("Teach auto mode about your environment?")
        );
    }

    #[test]
    fn dialog_title_ignores_prompts_and_old_dialogs() {
        // Der Text eines Prompts nennt den Footer nur.
        let prompt = "> Dialoge mit 'Esc to cancel' erkennen\n● Mache ich.\n────\n❯ \n────";
        assert_eq!(dialog_title(prompt), None);
        // Ein beantworteter Dialog steht weit über dem Prompt.
        let mut old = String::from("Weiter?\nEnter to confirm · Esc to cancel\n");
        for line in 0..20 {
            old.push_str(&format!("Ausgabe {line}\n"));
        }
        old.push_str("❯ \n");
        assert_eq!(dialog_title(&old), None);
    }

    fn master_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![crate::workspace::Workspace::test_new("master")];
        app.state.workspaces[0].chat.team.role = ChatRole::Master;
        app
    }

    #[test]
    fn a_master_turn_with_user_tasks_outside_the_inbox_gets_one_nudge() {
        let mut app = master_app();
        let answer = "Führe auf dem Laptop aus:\n! ssh-keygen";

        app.note_master_turn_end(0, Some(answer), Some("[collie inbox]\n- w2 (done)"));
        assert!(app.state.workspaces[0].chat.inbox_nudge_at.is_some());

        // Der Turn nach dem Hinweis löst keinen weiteren aus.
        app.state.workspaces[0].chat.inbox_nudge_at = None;
        app.note_master_turn_end(0, Some(answer), Some(&format!("{NUDGE_HEADER}\n…")));
        assert!(app.state.workspaces[0].chat.inbox_nudge_at.is_none());

        // Mit tell-user im selben Turn gibt es keinen Hinweis, im nächsten
        // Turn ohne tell-user wieder.
        app.note_told_user(0);
        app.note_master_turn_end(0, Some(answer), Some("Wie geht es weiter?"));
        assert!(app.state.workspaces[0].chat.inbox_nudge_at.is_none());
        app.note_master_turn_end(0, Some(answer), Some("Wie geht es weiter?"));
        assert!(app.state.workspaces[0].chat.inbox_nudge_at.is_some());

        // Nur Master bekommen Hinweise.
        let mut app = master_app();
        app.state.workspaces[0].chat.team.role = ChatRole::None;
        app.note_master_turn_end(0, Some(answer), Some("x"));
        assert!(app.state.workspaces[0].chat.inbox_nudge_at.is_none());
    }

    fn open_entry(kind: crate::workspace::UserInboxKind) -> crate::workspace::UserInboxEntry {
        crate::workspace::UserInboxEntry {
            id: 0,
            at: 1,
            kind,
            important: true,
            title: "Merge #54 freigeben".into(),
            text: String::new(),
            path: None,
            key: None,
            read: false,
            finished: false,
            deleted: false,
        }
    }

    #[test]
    fn references_to_open_inbox_entries_need_no_nudge() {
        let waiting = "#54 wartet auf deine Freigabe.";
        assert!(answer_has_user_tasks(waiting));

        // Ohne offenen Eintrag feuert der Hinweis.
        let mut app = master_app();
        app.note_master_turn_end(0, Some(waiting), Some("[collie inbox]"));
        assert!(app.state.workspaces[0].chat.inbox_nudge_at.is_some());

        // Eine automatische Antwort zählt nicht als offener Eintrag.
        let mut app = master_app();
        app.state.workspaces[0]
            .chat
            .team
            .push_user_inbox(open_entry(crate::workspace::UserInboxKind::Antwort));
        app.note_master_turn_end(0, Some(waiting), Some("[collie inbox]"));
        assert!(app.state.workspaces[0].chat.inbox_nudge_at.is_some());

        // Mit offenem Formular ist der Verweis kein Fehlalarm.
        let mut app = master_app();
        app.state.workspaces[0]
            .chat
            .team
            .push_user_inbox(open_entry(crate::workspace::UserInboxKind::Formular));
        app.note_master_turn_end(0, Some(waiting), Some("[collie inbox]"));
        assert!(app.state.workspaces[0].chat.inbox_nudge_at.is_none());

        // Neue Befehle feuern trotz offener Einträge.
        app.note_master_turn_end(
            0,
            Some("Bitte auf dem Laptop:\n! ssh-keygen"),
            Some("[collie inbox]"),
        );
        assert!(app.state.workspaces[0].chat.inbox_nudge_at.is_some());

        // Ein Verweis auf die Inbox reicht auch ohne offenen Eintrag.
        let mut app = master_app();
        app.note_master_turn_end(
            0,
            Some("Die Freigabe für #54 liegt in deiner Inbox, bitte dort beantworten."),
            Some("[collie inbox]"),
        );
        assert!(app.state.workspaces[0].chat.inbox_nudge_at.is_none());
    }

    #[test]
    fn a_dialog_in_a_child_is_reported_once_as_blocked() {
        let mut app = master_app();
        let master = app.state.workspaces[0].id.clone();
        let mut child = crate::workspace::Workspace::test_new("child");
        child.chat.team.role = ChatRole::Child { parent: master };
        app.state.workspaces.push(child);

        app.report_chat_dialog(1, "Teach auto mode about your environment?");
        app.report_chat_dialog(1, "Teach auto mode about your environment?");
        let inbox = &app.state.workspaces[0].chat.team.inbox;
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].kind, ChatInboxKind::Blocked);
        assert!(inbox[0]
            .text
            .as_deref()
            .is_some_and(|text| text.starts_with("Dialog offen: Teach auto mode")));
    }
}
