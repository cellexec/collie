# Keyboard sidebar focus

Status: accepted, implemented on `feat/keyboard-sidebar` (fork `cellexec/herdr`).

## Problem

Herdr is usable from the keyboard only as long as the work happens inside a
pane. Switching agents, switching spaces, creating spaces, filtering and
sorting the agent list all need the mouse. Navigate mode (`prefix+w`) moves a
cursor over spaces, but it ignores the agent list, `j/k` move pane focus
instead of the list, and it mixes list keys with prefix actions.

Target users are vim/tmux users who expect a k9s/lazygit-style list: a visible
focus, single-key actions, and no mouse.

## Focus model

There are two focus zones: the sidebar (left) and the panes (right). Navigate
mode becomes sidebar focus.

- Enter sidebar focus: `prefix+w` only. If the sidebar is hidden, it is shown
  as a drawer while focused and hidden again on exit.
- Leave sidebar focus:
  - `enter` commits the selection and moves focus to the panes.
  - `esc` first clears an active `/` filter; otherwise it leaves and restores
    the workspace and pane focus that were active before entering.
  - `i` leaves like `enter` without changing anything else.
- Indicator: the active zone gets an accent border/header. While the panes are
  active, the sidebar cursor row is drawn muted.
- Navigate mode no longer runs prefix actions without the prefix and no longer
  binds `1`-`9` to workspaces. The prefix keeps working inside the sidebar.

## Sidebar keys

Rebindable through flat `sidebar_*` and `navigate_*` fields in `[keys]`, the
existing convention for navigate-mode keys. Conflicts are reported by the
existing keybinding registry. `gg`, `za`, `ctrl+d`, `ctrl+u`, Esc and Enter are
fixed, like the copy-mode keys.

| Key | Action |
| --- | --- |
| `1` / `2` | focus the spaces / agents section; each keeps its own cursor |
| `j` / `k`, `gg` / `G`, `ctrl+d` / `ctrl+u` | move; the panes show a live preview |
| `/` | filter the focused section inline; `enter` ends input and keeps the filter |
| `enter` | commit and move focus to the panes |
| `n` | spaces: new space. agents: pick from `[[ui.sidebar.agent_commands]]`, start in a new tab; without config, open a new tab |
| `r` / `d` | rename / close: a space asks for confirmation, an agent pane closes on `dd` |
| `s` | toggle agent sort: grouped / priority |
| `f` | cycle agent state filter: all, blocked, working, idle |
| `i` | return focus to the panes without changing the selection |
| `za` | toggle the worktree group under the cursor |
| `J` / `K` | move the space under the cursor down / up |
| `<` / `>` | shrink / grow sidebar width |
| `+` / `-` | move the divider between spaces and agents |
| `m` | open the global menu (keyboard-operable, including release notes) |
| `?` | help |
| `space` | acts as the prefix: the same binding tree as `prefix` |

### Live preview

Moving the cursor in the spaces section switches the shown workspace. Moving it
in the agents section switches to that agent's workspace and tab and focuses its
pane. Focus stays in the sidebar. `esc` restores the state from before entering.
With several machines connected the preview stays a highlight, because a focus
change there can hand the client over to another endpoint.

### Agent commands

```toml
[[ui.sidebar.agent_commands]]
name = "claude"
command = "claude"

[[ui.sidebar.agent_commands]]
name = "codex"
command = "codex"
```

The picked command is typed into the shell of a new tab in the selected space,
followed by Enter. `1`-`9` pick an entry directly.

## Leader sequences and picker

- `search = "prefix+s"` starts a sequence. A which-key popup and the mode bar
  list the continuations; the continuations themselves are fixed.
- Settings move from `prefix+s` to `prefix+,`.
- `prefix+s a` opens a telescope-style picker over agents, `prefix+s s` over
  spaces. The picker reuses the goto navigator: typing filters immediately,
  `ctrl+j/k` and `ctrl+n/p` move, `enter` jumps, `esc` closes.

## Out of scope

- A machines section in the sidebar.
- Resetting a pane name, swapping with an arbitrary pane, the right-click
  passthrough toggle.
- A direct (non-prefix) focus toggle such as `ctrl+space`.

## Delivery

1. Sidebar focus with live preview, sections, indicator, drawer.
2. Sidebar actions and inline filter.
3. Leader sequences, which-key popup, settings on `prefix+,`.
4. Picker.
5. Docs and changelog.
