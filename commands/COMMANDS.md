# Command Contract

`commands/` is the upstream terminal interface for Mobailmux automation. Other
packages should call `mbx` rather than duplicating its tmux slot and hub logic.

## Commands

```bash
mbx
mbx open|o
mbx overview [slot|all]
mbx menu
mbx project|projects|pick|p [options]
mbx start|s [slot] [options] [directory] [prompt...]
mbx new|n [slot] [options] [directory] [prompt...]
mbx resume|r [slot] [options] [directory]
mbx stop|q [slot|all]
mbx status|slots|sessions [slot|all]
mbx list
mbx check
mbx help|command|commands
```

Run `mbx commands` for the live command list.

## Hub Model

Slots use letters `a` through `j`; `one` through `ten` and `1` through `10`
remain accepted aliases. The slot processes remain independent tmux sessions,
but their windows are linked into the `mobailmux` hub at reserved window indexes
11 through 20. The management `home` window uses index 0.

The linked windows carry these tmux user options:

```text
@mbx_slot
@mbx_workdir
@mbx_state
@mbx_state_at
```

Their automatic rename format is:

```text
#{@mbx_slot}:#{b:pane_current_path}
```

`@mbx_state` is one of `READY`, `RUNNING`, `WAITING`, `DONE`, or `ERROR`.
Missing metadata is displayed as `UNKNOWN`; no process-name or terminal-output
guess is made. `@mbx_state_at` records the update time.

New source sessions use `agent-1` through `agent-10`. Existing `codex-1`
through `codex-10` and `plugdeck-a` through `plugdeck-j` sessions are detected
and linked without restarting them.

## Behavior

- Bare `mbx`, `mbx open`, and slotless `mbx resume` attach to the hub.
- `mbx menu` opens a native tmux overview with one row per agent.
- `mbx overview` prints agent state separately from client attachment state.
- `mbx project` lists direct child directories under `MBX_PROJECTS_DIR` and
  starts or opens the selected project.
- Canonical paths identify projects, so symlink aliases do not consume duplicate
  slots or menu rows.
- Slotless `start` and `new` allocate the first free slot.
- A bare directory name is resolved under `MBX_PROJECTS_DIR` when present.
- Starting in an idle shell changes to the requested directory before launching
  Pi or OpenCode.
- `mbx stop` with no argument stops the current managed tmux window.
- `mbx stop all` removes all project tabs but leaves the hub home window alive.
- `mbx status` reports every slot; `mbx list` reports only running slots.
- Any positive `#{session_attached}` count is reported as attached.
- Mouse and status options are scoped to the hub session. Hub-only mouse
  actions use a session key table and do not alter tmux's global root bindings.

On tmux 3.7+, status ranges `control|0`, `control|1`, and `control|2` map to
`AGENTS`, `+NEW`, and `STOP`. On tmux 3.2 through 3.6, one `StatusLeft` range
opens the combined agent menu. The hub key table includes standard pane,
window-tab, and status-wheel actions so normal touch interaction remains intact.
Project menus are paginated using the attached client's height, capped at 12
projects per page; `MBX_MENU_PAGE_SIZE` can set a fixed positive size.

Harness lifecycle adapters are installed under `libexec/mobailmux`. Pi is
launched with `-e pi-state.ts`. OpenCode receives the local plugin through an
additional config source without replacing normal global or project config.
Launching with both `OPENCODE_CONFIG` and `OPENCODE_CONFIG_CONTENT` already set
is rejected because no third config layer is available for the state plugin.

`--unsafe` maps to OpenCode `--auto` and Pi `--approve`. Pi's flag pre-approves
project trust only; its `project_trust` prompt is reported as `WAITING` when
normal trust behavior is enabled.

Killing a source session directly with raw tmux does not necessarily stop a
linked window because the hub still owns a link. `mbx q <slot>` removes both the
source session, including extra windows, and the linked hub window.
