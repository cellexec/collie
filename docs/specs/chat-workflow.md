# Chat-Workflow

Status: beschlossen, Umsetzung auf `feat/chat-workflow` (collie).
Baut auf `chat-sidebar-overlay.md` auf.

Sieben Erweiterungen, die collie zum Hauptwerkzeug für parallele Agent-Chats
machen. Reihenfolge der Umsetzung wie unten.

## 1. Automatische Titel

- Der Chat-Hook (`stop`) erzeugt nach dem ersten Turn einen Titel und danach
  alle 3 Turns einen neuen. Der Zähler liegt pro Claude-Session in
  `$XDG_STATE_HOME/collie/titles/<session>` (Default `~/.local/state`).
- Der Hook forkt einen abgekoppelten Prozess und kehrt sofort zurück. Der
  Agent wartet nie auf den Titel.
- Der Prozess ruft `claude -p --model haiku --no-session-persistence
  --setting-sources "" --tools ""` mit dem bisherigen Titel und dem Ende des
  Transcripts auf. Die Umgebung ohne `HERDR_*`-Variablen verhindert, dass
  Hooks des Kind-Prozesses an collie melden.
- Der Titel geht per `pane.report_chat` mit neuem Feld `title` an den Server.
  Der setzt ihn als Space-Namen, wie `workspace.rename`. Ein manueller Name
  wird beim nächsten automatischen Titel überschrieben.
- Höchstens 6 Wörter, in der Sprache des Chats, ohne Anführungszeichen.

## 2. Gestoppte Chats

- Nach einem Neustart stellt collie alle Spaces wieder her, wie bisher. Neu:
  Mit `session.resume_agents_lazily = true` (collie-Default) startet der
  gespeicherte Resume-Befehl eines Agents erst, wenn sein Space aktiv wird.
- Bis dahin ist der Chat gestoppt. Der Server meldet das als
  `stopped_agent` (Name des Agents) in `WorkspaceInfo` und
  `ClientShellWorkspace`.
- Die Karte zeigt ein graues Stopp-Symbol und in Zeile 3 `gestoppt`, sonst
  die letzte Nachricht. Gestoppte Chats stehen in der Hauptliste, sortiert
  wie alle anderen.
- `enter` auf der Karte aktiviert den Space und startet damit den Agent.

## 3. Wartet auf Antwort

- `pane.report_chat` bekommt das Feld `awaiting`: ein kurzer Text, worauf der
  Agent wartet, etwa `Formular: 7 Fragen`. Leer löscht ihn.
- Der Server löscht `awaiting`, sobald der Agent in Tab 1 wieder `Working`
  wird.
- Die Karte zeigt dafür ein eigenes Symbol in Warnfarbe und in Zeile 3 den
  Text. Die Linie der eingeklappten Sidebar zählt den Zustand wie `Blocked`.
- Neuer CLI-Befehl `herdr pane report-chat <pane> [--awaiting <text>]
  [--last-message <text>] [--blocked-reason <text>] [--title <text>]`.
- Der `formular`-Skill meldet beim Öffnen `Formular: <k> Fragen` und löscht
  es nach `:wq`. Das nvim-Pane bleibt, wie es ist.

## 4. Hinweise innerhalb von collie

- collie-Default `ui.toast.delivery = "herdr"`. Der vorhandene Toast meldet,
  wenn ein Chat im Hintergrund fertig wird oder blockiert.

## 5. Chat-Statuszeile

- Neuer Eintrag `{ type = "chat" }` für `ui.tab_bar_right`, im collie-Default
  vor `zoom`. Er zeigt für den fokussierten Space: Verzeichnis des Agents
  (mit `~`), Modell und Kontextbelegung, etwa
  `~/projects/collie  Opus 5.5  84k/1M`.
- Quelle ist eine Claude-Code-Statusline, die `collie integration install
  claude` einträgt, wenn noch keine gesetzt ist. Sie ruft den Chat-Hook mit
  `status` auf. Der meldet `model`, `context_tokens` und `context_window` per
  `pane.report_chat`, nur wenn sich etwas geändert hat, und gibt nichts aus.
- Die Werte leben nur zur Laufzeit und werden nicht gespeichert.

## 6. Mehrere Chats auf einmal

In der ausgefahrenen Chat-Sidebar:

| Taste | Wirkung |
| --- | --- |
| `x` | markiert die Karte unter dem Cursor oder hebt die Markierung auf, der Cursor rückt weiter |
| `p` | Prompt an alle markierten Chats, ohne Markierung an den Chat unter dem Cursor |
| `O` | startet einen Orchestrator für die markierten Chats |

- Markierte Karten zeigen links oben ein grünes `✓`. Die Kopfzeile zeigt die Zahl.
  `space` bleibt die Leader-Taste der Sidebar.
  `esc` hebt alle Markierungen auf, bevor es die Sidebar einfährt.
- `p` öffnet eine Eingabe, vorbelegt mit dem Abschluss-Prompt: `Fasse deinen
  Stand zusammen: Was ist erledigt, was ist offen, und was fehlt noch, damit
  dieser Chat geschlossen werden kann? Arbeite auf diesen Abschluss hin.`
  `enter` schickt den Text per neuer Methode `chat.prompt` an jeden Agent.
  Der Server tippt ihn wie `agent.prompt` und drückt nach 300 ms Enter.
- `O` ruft die neue Methode `chat.orchestrate` mit den Pane-IDs auf. Der
  Server legt einen Space `Orchestrator` im Home-Verzeichnis an und startet
  dort `claude` mit einem Prompt. Der Prompt nennt jeden Chat mit Pane-ID,
  Titel, Verzeichnis, Branch, Status und letzter Nachricht und erklärt die
  CLI: `herdr pane read`, `herdr pane run`, `herdr agent`, `herdr --skill`.

## 7. Suche über alle Chats

- Neuer Befehl `herdr chat search`. Er liest alle Transcripts unter
  `~/.claude/projects`, eine Zeile pro Nutzer- oder Agent-Nachricht, neueste
  zuerst, und übergibt sie an `fzf` mit Vorschau (Telescope-Layout:
  Eingabe oben, Liste links, Vorschau rechts).
- Ein Cache unter `$XDG_CACHE_HOME/collie/chat-search/` hält die extrahierten
  Zeilen pro Transcript, gültig solange Größe und mtime gleich sind.
- Auswahl eines Treffers:
  - Läuft die Session schon in einem Pane, fokussiert collie dieses Pane.
  - Sonst legt collie einen Space im Verzeichnis der Session an, fokussiert
    ihn und startet `claude --resume <session>`.
- collie-Default: `prefix+/` öffnet die Suche als Popup mit 90 % Größe.
