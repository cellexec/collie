# Chat-Sidebar

Status: beschlossen, Umsetzung auf `feat/agents-only-sidebar` (collie).
Baut auf `ui.sidebar.agents_only` auf (siehe `keyboard-sidebar-focus.md`).

## Problem

Die Agents-Liste zeigt alles, was ein Agent ist, mit einem Symbol für jeden
Zustand. Sie beantwortet nicht die eine Frage, die zählt: Wo bin ich dran?
Laufende Agents, erledigte Chats und Chats, die gerade nerven, stehen
gleichberechtigt in der Liste.

## Modell

- Ein Chat ist ein Space. Tab 1 ist der Agent, Tab 2 und folgende sind
  temporäre Shells für Nebenarbeit.
- Jeder Space ist eine Zeile, auch ohne erkannten Agent (frisch angelegt oder
  Agent beendet). So geht kein Chat verloren.
- Den Status der Zeile bestimmt der erste Agent in Tab 1. Weitere Agents im
  Space werden ignoriert.
- Zeilentext: Terminal-Titel des Agents (Claude Code schreibt das Thema
  hinein), sonst Space-Name. Ein selbst vergebener Space-Name gewinnt immer.
- Erledigte Chats werden geschlossen, nicht ausgeblendet.

## Sichtbarkeit

| Zustand | Hauptliste | `h`-Ansicht | Symbol |
| --- | --- | --- | --- |
| `Blocked` | ja | nein | `●` rot |
| `Done` / `Idle` | ja | nein | `●` Akzent |
| `Unknown` / kein Agent | ja | nein | keins |
| `Working` | nein | ja | Spinner |
| gesnoozt | nein | ja | gedimmt, Restzeit rechts |

- `Done` und gesehenes `Idle` sehen gleich aus. Das `●` bleibt, bis der Agent
  wieder arbeitet oder der Chat geschlossen wird.
- Beginnt ein Chat zu arbeiten, verschwindet er sofort aus der Hauptliste. Das
  Pane rechts bleibt offen. Beim Wechsel in die Sidebar steht der Cursor auf
  dem ersten Eintrag.
- `h` zeigt ausschließlich die versteckten Chats (laufend und gesnoozt),
  nochmal `h` schaltet zurück. Der Spinner läuft nur dort.
- Die eingeklappte Sidebar zeigt die Hauptliste.

## Tasten

| Taste | Wirkung |
| --- | --- |
| `n` | neuer Space in `~` mit leerer Shell |
| `dd` | schließt den ganzen Space; bei `Working` mit Rückfrage |
| `z` | Snooze: `1` 1 h, `2` 4 h, `3` morgen 08:00, `4` 1 Tag, `u` aufheben |
| `h` | Hauptliste / versteckte Chats |
| `s` | Wartezeit: älteste zuerst / neueste zuerst |
| `/` | Textfilter in der aktiven Ansicht |
| `enter` | öffnet den Chat; ein Snooze bleibt bestehen |

`f` entfällt. „Morgen 08:00" heißt heute 08:00, wenn es noch vor 08:00 ist.

## Server

Wartezeit und Snooze sind geteilter Session-Zustand und liegen deshalb auf dem
Server, nicht im Client.

- `Workspace.attention_since`: Uhrzeit (Unix-Millisekunden), seit der der Chat
  auf den Nutzer wartet. Gesetzt beim Anlegen des Space und bei jedem Wechsel
  des Agents in Tab 1 von `Working` zu einem anderen Zustand.
- `Workspace.snoozed_until`: Uhrzeit, bis zu der der Chat versteckt ist.
- Beide werden mit der Session gespeichert und überleben einen Neustart.
- Die Felder erscheinen optional in `ClientShellWorkspace` und `WorkspaceInfo`.
  Neue JSON-Felder bleiben optional, der Generation-1-Codec bleibt gültig.
- Neue Methode `workspace.snooze` mit `workspace_id` und `until` (`null` hebt
  den Snooze auf). Sie wird für Clients beworben; ein alter Server ohne die
  Methode deaktiviert nur `z`.
- Ein abgelaufener Snooze sortiert den Chat nach seiner echten Wartezeit ein.

## Umsetzung

1. Server-Felder, Persistenz, `workspace.snooze`.
2. Liste pro Space, Sichtbarkeit, Symbole, `h`, `s`, `n` als neuer Space.
3. Snooze-Auswahl, Restzeit, Spinner.
