# Command Contract

`commands/` is the upstream interface for Mobailmux automation. Other packages
should call `mbx` or consume a future machine-readable interface from this
directory instead of duplicating tmux or Codex session logic.

Run this for the live command list:

```bash
mbx commands
```

## Commands

```bash
mbx start|s <slot> [directory] [prompt...]
mbx new|n <slot> [directory] [prompt...]
mbx resume|r <slot> [directory]
mbx stop <slot>
mbx stop all
mbx status [slot|all]
mbx list
mbx slots [slot|all]
mbx sessions [slot|all]
mbx check
mbx help
mbx command
mbx commands
```

`mbx status`, `mbx slots`, and `mbx sessions` show all fixed session names by
default. `mbx list` shows only currently running sessions. A slot is a named
Codex session backed by tmux.

Slots use letter names `a` through `i`, matching Plugdeck Agents. Legacy
aliases `one` through `nine` and `1` through `9` still work.

## Working Directory Rule

When no directory is provided, `mbx start`, `mbx new`, and `mbx resume` use the
current directory.

If a slot already exists but is sitting at a shell prompt, `mbx start <slot>`
starts Codex inside that idle slot using the current directory from the command
that invoked `mbx`.
