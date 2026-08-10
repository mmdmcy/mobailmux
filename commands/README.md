# Mobailmux Commands

`mbx` turns the existing lettered tmux slots into one mobile-friendly AI agent
hub. Every running agent appears in the same bottom bar as a clickable,
folder-named tab such as `[RUN] a:mobailmux` or `[DONE] b:portui`.

The command works over an ordinary Termius SSH connection. Projects continue
running if the phone disconnects, the app backgrounds, or tmux is detached.

## Install

```bash
cd ~/Documents/github/mobailmux/commands
./install.sh
```

The default install is a symlink at `~/.local/bin/mbx`, so changes in this
checkout are immediately available. Ensure `~/.local/bin` is on `PATH`.

Requirements:

- Bash 3.2 or newer
- tmux
- Pi or OpenCode only when using the agent-launching commands

## iPhone Workflow

Connect with Termius and run:

```bash
mbx
```

The `home` tab shows the controls. With tmux 3.7 or newer, the bottom bar has
three direct touch targets:

| Target | Action |
| --- | --- |
| `AGENTS` | Open the visual overview of every agent |
| `+NEW` | Open a clickable project list and start one |
| `STOP` | Confirm and stop the selected agent |

Project and overview menus are native tmux menus, so their rows are clickable
too. Mobailmux opens an already-running project or starts it in the first free
slot. By default, projects are read from `~/Documents/github`; override that
with `MBX_PROJECTS_DIR`. Long project lists are split into phone-sized pages.

Inside the hub:

| Action | Control |
| --- | --- |
| See agent overview | Tap `AGENTS` |
| Start a project | Tap `+NEW`, then tap a project |
| Stop an agent | Select its tab, tap `STOP`, then tap the confirmation |
| Switch projects | Tap a state-and-folder tab at the bottom |
| Leave without stopping anything | `Ctrl-b`, then `d` |
| Return later | Run `mbx` again |

The `home` tab provides a shell for running `mbx p`, `mbx status`, or other
management commands without leaving tmux. If it is closed accidentally, the
next bare `mbx` recreates the hub when needed.

`mbx p`, `mbx overview`, and `Ctrl-b w` remain keyboard fallbacks. On tmux
3.2 through 3.6, tapping `AGENTS` opens the combined action menu because those
versions cannot expose multiple independent custom status-bar buttons.

Termius must send terminal mouse events for finger taps to reach tmux. This is
automatic for pointer-capable sessions; if a specific Termius input mode uses
taps only for text selection, enable its mouse/pointer input or use the keyboard
fallbacks above.

## Agent States

| Badge | Meaning |
| --- | --- |
| `[READY]` | Shell or harness is initialized and has no completed task yet |
| `[RUN]` | The agent is actively processing a request |
| `[WAIT]` | The agent is waiting for permission or user input |
| `[DONE]` | The latest request settled successfully |
| `[ERR]` | The latest request or harness process failed |
| `[?]` | Legacy or externally started session without state instrumentation |

`DONE` and `ERR` remain visible until the next request starts, so finishing in
a background tab does not disappear before it is noticed.

## Common Commands

```bash
mbx                         # open the clickable project hub
mbx menu                    # open the clickable agent overview
mbx overview                # print all exact agent states
mbx p                       # choose a project by number
mbx s mobailmux             # start by folder name in the first free slot
mbx s b portui              # start portui in exact slot b
mbx s --harness opencode    # start the current folder with OpenCode
mbx r a                     # open slot a; create a shell there if missing
mbx q                       # stop the current project tab
mbx q b                     # stop exact tab b
mbx q all                   # stop every project but keep the home tab
mbx status                  # show slots, commands, and live paths
mbx list                    # show only running projects
```

`start`/`s` prefers Pi when it is installed and otherwise uses OpenCode. Use
`--harness pi|opencode` or `MOBAILMUX_DEFAULT_HARNESS` to select explicitly.
`new`/`n` replaces a running slot before launching. Omit the slot from `start`
or `new` to use the first free slot.

Project names under `MBX_PROJECTS_DIR` are accepted as path shorthand:

```bash
mbx s mobailmux
```

is equivalent to:

```bash
mbx s ~/Documents/github/mobailmux
```

## How It Works

Slots `a` through `j` remain independent tmux workspaces. Mobailmux links their
windows into one `mobailmux` hub so tmux can render them as a shared tab bar.
This also brings already-running `agent-N`, `codex-N`, and `plugdeck-a` through
`plugdeck-j` sessions into the hub without restarting them.

Each linked window stores its slot and project root as tmux metadata. Its name
is rendered with its state and `<slot>:<current-folder>`, and follows directory
changes. Mouse mode and status-bar styling are scoped to the Mobailmux hub
rather than applied globally to unrelated tmux sessions.

Mobailmux loads bundled lifecycle adapters only for harnesses it starts:

- The OpenCode plugin reads `session.status`, `session.idle`, `session.error`,
  and permission events.
- The Pi extension reads `project_trust`, `agent_start`, `agent_end`, and
  `agent_settled`.

They write state to the linked tmux window as `@mbx_state`; they do not inspect
or scrape terminal text. Existing OpenCode and project configuration continues
to load normally. If both `OPENCODE_CONFIG` and `OPENCODE_CONFIG_CONTENT` are
already set, `mbx` stops with an explicit error instead of launching without
lifecycle tracking.

Stopping a slot destroys the linked project window and its complete source tmux
session. It does not kill the hub or unrelated tmux sessions.

`--unsafe` enables OpenCode automatic approval. For Pi, the corresponding
`--approve` flag pre-approves project trust; it is not a general tool-permission
bypass. Use `--safe` to keep the harness's normal approval and trust prompts.

## Configuration

```text
MBX_PROJECTS_DIR=~/Documents/github
MBX_HUB_SESSION=mobailmux
MBX_MENU_PAGE_SIZE=auto
MBX_SESSION_PREFIX=agent
MOBAILMUX_DEFAULT_HARNESS=pi
MOBAILMUX_PI_BIN=pi
MOBAILMUX_OPENCODE_BIN=opencode
```

Run `mbx help` for the complete live command list and options. See
[COMMANDS.md](COMMANDS.md) for the automation contract.
