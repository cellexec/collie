# Nutzer-Inbox

Status: umgesetzt (collie), global seit `feat/global-user-inbox`. Ergänzt
`master-chat.md`.

Ein Master Chat bekommt laufend `[collie inbox]`-Prompts seiner Children.
Seine Antworten an dich rutschen dabei im Chat nach oben weg. Die
Nutzer-Inbox sammelt alles, was Chats dir sagen wollen, an einem eigenen Ort,
bis du es mit `dd` erledigst. Es gibt eine Inbox für alle Chats.

```mermaid
flowchart LR
  U[Du: Prompt] --> M[Master Chat]
  C[Children] -- "[collie inbox]" --> M
  M -- "Antwort auf deinen Prompt, automatisch" --> I[(Nutzer-Inbox)]
  M -- "herdr chat tell-user" --> I
  A[jeder andere Chat] -- "herdr chat tell-user" --> I
  I --> Z[Zähler im Sidebar-Kopf]
  Z --> O[Overlay]
  O -- "dd, Formular, Wizard, r" --> R[Rückmeldungen]
  R -- "[collie user]" --> M
  R -- "[collie user]" --> A
```

## Modell

| Fakt | Ort | Gespeichert |
| --- | --- | --- |
| Einträge der Nutzer-Inbox | Server, Space des schreibenden Chats (`user_inbox`) | ja |
| Rückmeldungen an den Chat | Server, derselbe Space (`user_replies`) | ja |
| Zähler und Revision über alle Chats | Snapshot (`user_inbox`) | nein |
| Overlay, Cursor, Markierungen, `u`-Verlauf | TUI | nein |

| Art | Quelle | Offen, solange |
| --- | --- | --- |
| `antwort` | Stop-Hook: Antwort auf einen Prompt, der nicht mit `[collie ` beginnt | ungelesen |
| `nachricht` | `herdr chat tell-user` | ungelesen |
| `formular` | `herdr chat tell-user --kind formular --file F` | nicht beantwortet |
| `wizard` | `herdr chat tell-user --kind wizard --file S` | nicht mit Exit-Code 0 durchgelaufen |

- Einträge sind wichtig oder optional (`--optional`). `antwort` ist immer
  wichtig.
- Ein Turn erzeugt eine `antwort`. Der Schlüssel ist die UUID des Prompts im
  Transcript; ein späterer Stop im selben Turn ersetzt den Text.
- Einträge verschwinden durch `dd`. Beantwortete Formulare und mit Exit-Code
  0 durchgelaufene Wizards verschwinden sofort. Die letzten 50 gelöschten
  bleiben für `u` erhalten.
- Gespeichert wird je Chat, damit Rückmeldungen beim richtigen Agent
  ankommen. IDs sind nur je Chat eindeutig; die TUI adressiert Einträge mit
  Workspace-ID und ID.

## Server-API

| Methode | Wirkung |
| --- | --- |
| `chat.tell_user` | legt `nachricht`, `formular` oder `wizard` ab |
| `chat.user_inbox` | nicht gelöschte Einträge eines Chats; mit `all` aller Chats, je mit `workspace_id`, `chat` und `master` |
| `chat.user_inbox_act` | `read`, `done`, `restore`, `open`, `reply`, `answered`, `wizard_result`, `reply_written`, `flush`; mit `all` kommt die globale Liste zurück und `flush` gilt für alle Chats |
| `pane.report_chat` | neue Felder `answer`, `answer_prompt`, `answer_key` vom Stop-Hook |

- `open` und `reply` öffnen einen Tab im Inbox-Space und tippen dort den
  Befehl ein: nvim für Formular und Rückfrage, `bash` für den Wizard. Der Tab
  meldet sein Ergebnis mit `herdr chat user-inbox-result` und schließt sich.
- Der Inbox-Space entsteht beim ersten `open` (Name `Inbox`, Merkmal
  `user_inbox_space`) und verschwindet mit seinem letzten Tab. Er steht weder
  in der Chat- noch in der Space-Liste. Ist eine Aufgabe schon offen, springt
  `open` zu ihrem Tab.
- Ein Formular gilt als beantwortet, wenn sich die Datei beim Speichern
  geändert hat. `:q` ohne Änderung lässt es offen.

## Rückmeldungen an den Chat

| Auslöser | Zeile im Prompt | Zustellung |
| --- | --- | --- |
| `dd` auf Nachricht | `#3 nachricht „…“: abgearbeitet` | beim Verlassen des Overlays |
| `dd` auf offenes Formular | `…: verworfen, ohne Antwort gelöscht` | beim Verlassen des Overlays |
| `dd` auf Antwort | keine | – |
| Formular gespeichert | `…: beantwortet, die Antworten stehen in <datei>` | sofort |
| Wizard beendet | `…: durchgelaufen` oder `abgebrochen mit Exit-Code N` | sofort |
| `r` | `…: Rückfrage: <text>` | sofort |

- Der Server bündelt fällige Rückmeldungen zu einem Prompt
  `[collie user] Rückmeldungen aus meiner Inbox:`, sobald der Chat idle ist,
  die letzte Rückmeldung 2 Sekunden alt ist und der Nutzer seit 5 Sekunden
  nicht im Chat getippt hat.
- `dd`-Meldungen warten, bis die TUI `flush` schickt, spätestens 30 Minuten.
- Bekommt ein Master im selben Durchlauf seine Child-Inbox, folgen die
  Rückmeldungen beim nächsten Idle.

## Tagesübersicht

| Punkt | Verhalten |
| --- | --- |
| Wann | werktags ab `master.daily_digest` (Ortszeit, Standard `08:00`); leer oder `off` schaltet ab. Einmal je Tag, auch nach einem Neustart: der Tag steht in `digest_day` des Chats, der sie bekam |
| Inhalt | Liste in drei Abschnitten: blockiert, wartet auf dich (ungelesen, nicht gesnoozt), fertig geworden seit dem Termin des letzten Werktags; montags also seit Freitag |
| Mit laufendem Master | der Server schickt dem Master `[collie digest] <Titel>` mit der Liste; sobald er idle ist und der Nutzer 5 s nicht getippt hat. Der Master legt eine Zusammenfassung mit `tell-user` ab |
| Ohne Master | der Server legt die Liste als wichtige Nachricht `Tagesübersicht Mi 18.11.` beim ersten Master oder beim ersten Chat ab |
| Master zu lange beschäftigt | nach 2 Stunden oder wenn sein Agent nicht mehr läuft, kommt die Liste ohne Zusammenfassung |

## TUI

| Ort | Anzeige |
| --- | --- |
| Kopfzeile des Sheets | zwei Tabs ` 󰭹 chats │ 󰇮 inbox 3 2`: wichtig rot, optional grau; der aktive Tab ist hell, ein Klick auf den anderen wechselt |
| eingeklappte Sidebar (Leiste) | `󰇮 3 2` unter der Zahl der Chats; Master zeigen die Krone statt des Kürzels |
| Inbox offen | dasselbe Sheet wie die Chats im Tab `inbox`, aber fast volle Breite, links die Tabelle (Markierung, Status, Art, Chat, Titel, Alter; wichtig zuerst, neueste zuerst), rechts der Text als gesetztes Markdown; rechts bleibt ein Streifen frei, ein Klick dorthin schließt |
| Aufgabe läuft | das Sheet dockt links an, die Panes rücken nach rechts; Tastatur und Panes gehören dem Tab im Inbox-Space. Beide Tabs bleiben per Klick erreichbar; ein Klick auf einen Chat verlässt den Inbox-Space |
| Aufgabe fertig | schließt sich ihr Tab, ist die Inbox wieder ganz offen; wechselst du in einen anderen Space, schließt sie |

| Taste | Wirkung |
| --- | --- |
| `I`, `[` oder `]` in den Chats, `prefix+i` | Inbox öffnen |
| `[` oder `]` in der Inbox | zurück zu den Chats |
| `j`/`k`, `g`/`G`, Mausrad | Cursor; beim Lesen scrollen |
| `enter` | Nachricht lesen (Fokus auf den Text), Formular oder Wizard als Tab öffnen |
| `prefix+i`, Klick auf die angedockte Liste | während einer Aufgabe zurück in die Inbox |
| Klick auf `chats` im angedockten Sheet | Chat-Karten statt Liste; Klick auf eine Karte öffnet den Chat |
| `space`, `m` | markieren; die erste Spalte zeigt dann eine Checkbox |
| `d` `d` | markierte oder aktuelle Einträge erledigen; das erste `d` streicht sie rot durch |
| `u` | letztes `dd` zurückholen |
| `r` | Rückfrage in nvim |
| Zoom-Taste (Standard `prefix+z`) | während einer Aufgabe: angedockte Liste aus- und wieder einblenden, die Aufgabe füllt dann den Bildschirm |
| `ctrl+d`/`ctrl+u`, Bild auf/ab | Text scrollen |
| `esc`, `q` | schließen und zurück in den Chat, aus dem die Inbox kam; zurückgehaltene Rückmeldungen gehen raus |

Der Zähler zählt ungelesene Nachrichten und Antworten sowie offene Formulare
und Wizards. Es gibt keinen Toast und keinen Ton.

## Agent-Verhalten

- Der Skill `collie-master` schreibt vor, Formulare und Wizards nur noch über
  die Inbox abzulegen und nie selbst zu öffnen. Tätigkeiten für den Nutzer
  gehen als Wizard in die Inbox, Entscheidungen als Formular. Text im Chat
  zählt nie als Übergabe.
- Meldet das Stop-Hook eines Masters eine Antwort mit Aufgaben oder Fragen
  für den Nutzer (`! `-Befehle, PowerShell-Blöcke, Wendungen wie „bitte“ oder
  „sag Bescheid“, Zeilen mit `?` am Ende) und hat der Master in diesem Turn
  nicht `chat.tell_user` aufgerufen, schickt der Server ihm einen Prompt, der
  mit `[collie hinweis]` beginnt. Er geht raus, sobald der Master idle ist.
  Auf den Turn, den der Hinweis auslöst, folgt kein weiterer.
- Jeder andere Chat darf `herdr chat tell-user` ebenso nutzen.
- Neue Hook-Felder und Skill kommen mit `collie integration install claude`.
