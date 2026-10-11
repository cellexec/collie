# Chat-Sidebar als Overlay

Status: beschlossen, Umsetzung auf `feat/chat-sidebar-overlay` (collie).
Baut auf `chat-sidebar.md` und `keyboard-sidebar-focus.md` auf.

## Problem

Die Chat-Liste ist eine Spalte mit Titeln. Sie belegt dauerhaft 18 bis 36
Spalten, zeigt aber zu wenig, um ohne Öffnen zu entscheiden, welcher Chat
als Nächstes dran ist. Der heutige Drawer (`sidebar_collapsed_mode = hidden`
plus `prefix+w`) resized beim Ein- und Ausfahren alle PTYs. Agents zeichnen
dabei neu, und die Arbeit skaliert mit der Zahl der Panes.

## Aktivierung

Neue Option `ui.sidebar.overlay = true`. Sie wirkt nur zusammen mit
`ui.sidebar.agents_only`. Ohne sie bleibt das bisherige Verhalten samt
`sidebar_collapsed_mode` und `sidebar_width`.

## Eingeklappt: Linie

- Links steht eine Linie von 1 Spalte Breite. Sie ist eine eigene
  Layout-Spalte; die Panes sind dauerhaft 1 Spalte schmaler, die Linie
  verdeckt nichts.
- Farbe: rot, wenn ein Chat `Blocked` ist; Akzent, wenn ein Chat `Done` oder
  `Idle` ist und wartet; sonst gedimmt. Rot gewinnt.
- Wird ein Chat neu fertig oder blockiert, pulsiert die Linie dreimal hell und
  normal über etwa 1,5 s und bleibt dann in ihrer Dauerfarbe.
- Maus: Klick fährt aus. Hover fährt nach 300 ms aus.

## Ausgefahren: Overlay

- Das Overlay liegt über den Panes. Kein PTY wird resized, das Pane-Layout
  ändert sich nicht.
- Breite: 45 % der Terminalbreite, mindestens 50, höchstens 100 Spalten.
  `<`/`>` ändern den Prozentwert in 5er-Schritten zwischen 30 und 70 %. Der
  Wert wird gespeichert wie heute die manuelle Sidebar-Breite.
- Die Panes rechts liegen im Backdrop wie heute bei Sidebar-Fokus. Der linke
  Teil des Panes bleibt verdeckt.
- Keine Live-Vorschau: `j`/`k` bewegt nur den Cursor, erst `enter` wechselt
  den Chat. `esc` muss deshalb keinen Fokus zurücksetzen.

## Ein- und Ausfahren

| Auslöser | Wirkung |
| --- | --- |
| `prefix+w` | fährt aus und fokussiert die Sidebar; erneut fährt ein |
| Klick auf die Linie | fährt aus |
| Hover auf der Linie, 300 ms | fährt aus |
| `enter` | öffnet den Chat unter dem Cursor und fährt ein |
| `esc` | löscht zuerst einen aktiven `/`-Filter, sonst fährt ein |
| `i` | fährt ein, ohne etwas zu ändern |
| Klick in die Panes | fährt ein, dann läuft der normale Klick |
| Klick auf eine Karte | öffnet den Chat und fährt ein |

Ein per Hover geöffnetes Overlay fährt wieder ein, wenn die Maus es ohne
Tastendruck verlässt. Nach dem ersten Tastendruck verhält es sich wie nach
`prefix+w`. Hover über einer Karte hebt sie hervor und setzt den Cursor.

## Karten

Jeder Chat der Hauptliste ist eine Karte mit 3 Zeilen. Alle Karten sind gleich
hoch.

```
▌ ● 󰚩 Refactor der Snooze-Logik                    12m
▌     collie   feat/snooze ↑2  +1 shell
▌   wartet auf Freigabe: Bash(cargo test)
```

| Zeile | Inhalt |
| --- | --- |
| 1 | Status-Symbol, Agent-Icon, Titel, Wartezeit rechts |
| 2 | Repo, Branch, ahead/behind, Worktree-Marker, Zahl der Neben-Tabs |
| 3 | bei `Blocked` der Grund, sonst die letzte Nachricht |

- Abtrennung: leicht getönter Hintergrund pro Karte (`surface_dim`), eine
  Leerzeile Abstand. Die Karte unter dem Cursor hat links einen Akzentbalken
  `▌`.
- Sortierung bleibt flach nach Wartezeit (`s` dreht sie), ohne Abschnitte.
  Der Status steckt nur in der Farbe.
- Wartezeit relativ (`12m`, `3h`, `2d`). Farbe nach Alter: grau, gelb ab 15
  Minuten, orange ab 1 Stunde.
- Ungelesen: gefülltes `●` und fetter Titel. Gelesen: hohles `○`, normaler
  Titel. Die Symbolfarbe folgt weiter dem Status (`chat-sidebar.md`).
- Repo: Basename von `foreground_cwd` des Agent-Panes, sonst `cwd`, sonst das
  Worktree-Label. Branch und ahead/behind kommen vom Space; der Branch wird im
  Overlay-Modus periodisch aktualisiert, auch ohne konfiguriertes
  `Branch`-Token.
- Neben-Tabs: `+N shells` für Tabs ab Nummer 2, fehlt bei 0.
- Zeile 3: die letzte nicht-leere Zeile der letzten Assistant-Nachricht,
  Markdown entfernt, auf Kartenbreite gekürzt. Leer, wenn nichts bekannt ist.
- Icons kommen fest verdrahtet aus dem Nerd Font, ohne Unicode-Fallback.

## Laufende und gesnoozte Chats

- Unter der Hauptliste steht ein kompakter Abschnitt mit einer Zeile pro Chat:
  Spinner bzw. Snooze-Restzeit, Titel, rechts die Laufzeit seit
  `working_since` bzw. die Restzeit.
- Er nimmt höchstens ein Drittel der Höhe ein und scrollt darin.
- `j`/`k` läuft durchgehend von der Hauptliste in den Abschnitt. `h` springt
  zum ersten Eintrag des Abschnitts und zurück.

## Leere Hauptliste

Wartet kein Chat, steht zentriert ein Nerd-Font-Icon, darunter `Nichts zu tun`
und die Zähler der laufenden und gesnoozten Chats.

## Server

Alle neuen Felder sind geteilter Session-Zustand, werden mit der Session
gespeichert und erscheinen optional in `ClientShellWorkspace` und
`WorkspaceInfo`. Der Generation-1-Codec bleibt gültig.

| Feld | Bedeutung | Gesetzt |
| --- | --- | --- |
| `last_message` | letzte nicht-leere Zeile der letzten Assistant-Nachricht | `pane.report_chat` aus dem Stop-Hook |
| `blocked_reason` | kurzer Grund für `Blocked` | `pane.report_chat` aus dem Notification-Hook; gelöscht, sobald der Agent `Blocked` verlässt |
| `working_since` | Unix-Millisekunden seit Beginn von `Working` | beim Wechsel des Agents in Tab 1 zu `Working` |
| `attention_seen` | Nutzer hatte den Chat seit `attention_since` offen | beim Setzen von `attention_since` wahr, wenn der Space gerade aktiv ist, sonst `false`; `true` beim Fokussieren des Space |

- Neue Methode `pane.report_chat` mit `pane_id`, `last_message` und
  `blocked_reason`. Ein fehlendes Feld bleibt unverändert, ein leeres löscht.
  Meldungen aus anderen Tabs als Tab 1 ändern nichts. Der Text wird auf eine
  Zeile mit höchstens 200 Zeichen gekürzt.
- Die herdr-Integration für Claude bleibt unverändert. Ihr Installer richtet
  nur `SessionStart` ein und entfernt fremde herdr-Hooks auf anderen Events.
- Das Hook-Skript `scripts/collie/claude-chat-hook.sh` liegt im Repo und wird
  von Hand in `~/.claude/settings.json` eingetragen:

  ```json
  "hooks": {
    "Stop": [{"hooks": [{"type": "command", "command": "~/projects/collie/scripts/collie/claude-chat-hook.sh stop"}]}],
    "Notification": [{"hooks": [{"type": "command", "command": "~/projects/collie/scripts/collie/claude-chat-hook.sh notification"}]}]
  }
  ```

  `stop` nimmt `last_assistant_message` aus der Hook-Eingabe, sonst die letzte
  Antwort aus dem Ende des Transcripts. `notification` meldet nur
  Freigabe-Anfragen (`permission_prompt`), etwa `Freigabe: Bash`.
- Agents ohne diese Hooks zeigen in Zeile 3 nichts.

## Performance

- Das Overlay rendert nur Client-seitig und löst keinen Resize aus.
- Karten lesen nur Snapshot-Felder. Kein Bildschirm-Read, kein Dateizugriff im
  Render-Pfad.
- Puls und Spinner fordern Frames nur an, solange sie laufen.

## Umsetzung

1. Overlay, Linie mit Puls, Ein- und Ausfahren, Hover, keine Live-Vorschau.
2. Karten mit vorhandenen Daten: Titel, Wartezeit mit Farbe, Repo, Branch,
   Neben-Tabs, unterer Abschnitt, leere Liste.
3. Server-Felder mit Persistenz, `pane.report_chat`, Hook-Skript für Claude.
4. Zeile 3, Laufzeit und Ungelesen-Marker im Client.
