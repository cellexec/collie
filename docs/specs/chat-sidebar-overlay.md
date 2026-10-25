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

## Eingeklappt: Leiste auf breiten Terminals

Ab 110 Spalten Terminalbreite steht statt der Linie eine Leiste von 8 Spalten.
Darunter bleibt die Linie, auf dem Handy die mobile Kopfzeile.

```
 󰭹 2   ▕
       ▕
▌●  CO ▕
       ▕
▌?  SB ▕
       ▕
───────▕
 ⠹  HE ▕
 󰒲  DO ▕
```

- Kopf: Chat-Icon und Zahl der wartenden Chats.
- Je wartendem Chat eine Zeile: Wärmestreifen (Akzent für den offenen Chat),
  Status-Symbol wie auf der Karte, Repo-Kürzel als Chip in der Repo-Farbe.
  Ohne Repo kommt das Kürzel aus dem Titel. Passt die Liste, steht eine
  Leerzeile dazwischen; was nicht passt, zählt die letzte Zeile als `+N`.
- Unten, unter einer Trennlinie, laufende Chats mit Spinner und gesnoozte mit
  Snooze-Icon, das Kürzel als farbiger Text. Höchstens ein Drittel der Höhe.
- Der rechte Rand trägt die Farbe der Linie und pulsiert wie sie.
- Maus: Klick auf eine Zeile öffnet den Chat direkt. Klick auf eine freie
  Stelle fährt das Overlay aus. Darüberfahren fährt nichts aus, sonst läge das
  Overlay über der Zeile, die man anklicken will.
- Der Spinner zeichnet neu, solange ein Chat arbeitet und die Leiste steht.

## Eingeklappt: Linie

- Links steht eine Linie von 1 Spalte Breite. Sie ist eine eigene
  Layout-Spalte; die Panes sind dauerhaft 1 Spalte schmaler, die Linie
  verdeckt nichts.
- Oben steht je wartendem Chat sein Status-Symbol wie auf der Karte (`●`
  ungelesen, `○` gelesen, `?` Rückfrage, `■` gestoppt), in Kartenreihenfolge.
  Nach einer Lücke folgt je laufendem Chat ein `◌` in der Arbeitsfarbe.
  Gesnoozte Chats fehlen. Die Symbole drehen sich nicht, damit die Linie keine
  Frames anfordert.
- Darunter läuft die Linie weiter. Farbe: rot, wenn ein Chat `Blocked` ist;
  Akzent, wenn ein Chat `Done` oder `Idle` ist und wartet; sonst gedimmt. Rot
  gewinnt.
- Wird ein Chat neu fertig oder blockiert, pulsiert die Linie dreimal hell und
  normal über etwa 1,5 s und bleibt dann in ihrer Dauerfarbe.
- Maus: Klick fährt aus. Hover fährt nach 300 ms aus.

## Ausgefahren: Overlay

- Das Overlay liegt über den Panes. Kein PTY wird resized, das Pane-Layout
  ändert sich nicht.
- Breite: 45 % der Terminalbreite, mindestens 64, höchstens 100 Spalten.
  Bei 64 Spalten passt neben Repo und Branch der Kontext-Balken. Ist das
  Terminal schmaler, nimmt das Overlay die ganze Breite.
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
▌  CO  collie   +1      ▂███▁▁▁▁▁▅█  ▰▰▰▱▱ 64%    feat/snooze ↑2
▌ ● Refactor der Snooze-Logik                                  12m
▌ wartet auf Freigabe: Bash(cargo test), danach noch die Tests im …
```

| Zeile | Inhalt |
| --- | --- |
| 1 | Repo-Chip und Repo, Worktree-Marker, Zahl der Neben-Tabs; rechts Branch mit ahead/behind, davor Aktivitätskurve und Kontext-Balken. Ohne Repo stehen Agent-Icon und Agent-Name links. |
| 2 | Status-Symbol, Titel, Wartezeit rechts |
| 3 | bei `Blocked` der Grund, sonst die letzte Nachricht |

Titel und Zeile 3 enden abgeschnitten mit `…`. Wird Zeile 1 eng, weicht
zuerst die Aktivitätskurve, dann der Kontext-Balken. Der Branch bekommt
höchstens die halbe Zeile und wird erst danach gekürzt.

- Abtrennung: leicht getönter Hintergrund pro Karte (`surface_dim`), eine
  Leerzeile Abstand. Die Karte unter dem Cursor hat links einen Akzentbalken
  `▌`.
- Wärmestreifen: Die übrigen Karten tragen links einen Streifen, dessen Farbe
  fließend mit der Wartezeit wechselt: grau, gelb bei 15 Minuten, peach bei
  einer Stunde, rot ab vier Stunden.
- Nachleuchten: Wartet ein Chat neu, färbt sich seine Karte 1,5 Sekunden lang
  in Akzentfarbe, bei `Blocked` oder offener Rückfrage in Rot, und klingt dann
  ab. Grundlage ist `attention_since`; eine Zeit in der Zukunft leuchtet nicht.
- Repo-Farbe: Jedes Repo bekommt per FNV-Hash über den Namen einen festen
  Farbton aus der Palette, nie Rot oder Gelb. Das Kürzel (`SB` für
  `second-brain`, `CO` für `collie`) steht als Chip, der Name in derselben
  Farbe.
- Kontext-Balken: `▰▰▰▱▱ 64%` aus `context_tokens` und `context_window`.
  Grün, ab 70 % gelb, ab 90 % peach. Fehlt das Fenster, fehlt der Balken.
- Aktivitätskurve: 12 Spalten über die letzten 30 Minuten, je Spalte der
  Anteil Arbeitszeit aus `working_spans` und der laufenden Phase. Ohne Arbeit
  im Fenster fehlt sie. Balken und Kurve stehen nur dort, wo links mindestens
  24 Zellen frei bleiben; die Kurve weicht zuerst.
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
- Neben-Tabs: Shell-Icon und `+N` für Tabs ab Nummer 2, fehlt bei 0.
- Zeile 3: die letzte nicht-leere Zeile der letzten Assistant-Nachricht,
  Markdown entfernt, auf Kartenbreite gekürzt. Leer, wenn nichts bekannt ist.
- Icons kommen fest verdrahtet aus dem Nerd Font, ohne Unicode-Fallback.

## Laufende und gesnoozte Chats

- Unter der Hauptliste steht ein kompakter Abschnitt mit einer Zeile pro Chat:
  Spinner bzw. Snooze-Restzeit, Titel, rechts die Laufzeit seit
  `working_since` bzw. die Restzeit. Laufende Chats zeigen davor ihre
  Aktivitätskurve, wenn der Titel 20 Zellen behält.
- Er nimmt höchstens ein Drittel der Höhe ein und scrollt darin.
- `j`/`k` läuft durchgehend von der Hauptliste in den Abschnitt. `h` springt
  zum ersten Eintrag des Abschnitts und zurück.

## Leere Hauptliste

Wartet kein Chat, döst zentriert eine ASCII-Collie mit aufsteigenden `z`
(ein Frame pro 900 ms), darunter `alles erledigt` und die Zähler der laufenden
und gesnoozten Chats. Ist die Fläche kleiner als 10 Zeilen, steht stattdessen
ein Nerd-Font-Icon mit `Nichts zu tun`.

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
| `context_tokens`, `context_window` | Kontextbelegung des Agents in Tab 1 | `pane.report_chat`; nicht gespeichert |
| `working_spans` | abgeschlossene Arbeitsphasen der letzten 30 Minuten als `(start, ende)`, höchstens 64 | beim Wechsel des Agents in Tab 1 weg von `Working`; nicht gespeichert |

- Neue Methode `pane.report_chat` mit `pane_id`, `last_message` und
  `blocked_reason`. Ein fehlendes Feld bleibt unverändert, ein leeres löscht.
  Meldungen aus anderen Tabs als Tab 1 ändern nichts. Der Text wird auf eine
  Zeile mit höchstens 200 Zeichen gekürzt.
- Der herdr-Teil der Claude-Integration bleibt unverändert. Ihr Installer
  richtet nur `SessionStart` ein und entfernt fremde herdr-Hooks auf anderen
  Events.
- `collie integration install claude` legt danach
  `scripts/collie/claude-chat-hook.sh` als `~/.claude/hooks/collie-chat-hook.sh`
  ab und trägt es für `Stop` und `Notification` ein
  (`src/integration/collie_chat_hook.rs`). Hand-Einträge, die auf
  `claude-chat-hook.sh` zeigen, ersetzt es. `uninstall` entfernt beides.

  `stop` nimmt `last_assistant_message` aus der Hook-Eingabe, sonst die letzte
  Antwort aus dem Ende des Transcripts. `notification` meldet nur
  Freigabe-Anfragen (`permission_prompt`), etwa `Freigabe: Bash`.
- Agents ohne diese Hooks zeigen in Zeile 3 nichts.

## Performance

- Das Overlay rendert nur Client-seitig und löst keinen Resize aus.
- Karten lesen nur Snapshot-Felder. Kein Bildschirm-Read, kein Dateizugriff im
  Render-Pfad.
- Puls und Spinner fordern Frames nur an, solange sie laufen. Das gilt auch
  für das Nachleuchten der Karten und die dösende Collie; beide ticken nur bei
  offenem Overlay. Wärmestreifen und Kurve zeichnen minütlich neu.

## Umsetzung

1. Overlay, Linie mit Puls, Ein- und Ausfahren, Hover, keine Live-Vorschau.
2. Karten mit vorhandenen Daten: Titel, Wartezeit mit Farbe, Repo, Branch,
   Neben-Tabs, unterer Abschnitt, leere Liste.
3. Server-Felder mit Persistenz, `pane.report_chat`, Hook-Skript für Claude.
4. Zeile 3, Laufzeit und Ungelesen-Marker im Client.
