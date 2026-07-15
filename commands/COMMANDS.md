# Command Contract

`commands/` is the upstream interface for Mobailmux automation. Other packages
should call `mbx` or consume a future machine-readable interface from this
directory instead of duplicating tmux slot logic.

Run this for the live command list:

```bash
mbx commands
```

## Commands

```bash
mbx start <slot> [directory] [prompt...]
mbx s <slot> [directory] [prompt...]
mbx new|n <slot> [directory] [prompt...]
mbx resume|r <slot> [directory]
mbx stop <slot>
mbx q <slot>
mbx stop all
mbx q all
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
default. `mbx list` shows only currently running sessions. A slot is a reusable
tmux workspace, not a permanent Codex session, project, or conversation.
Mobailmux enables tmux mouse mode (`mouse on`) for the server so wheel scrolling
works in every managed slot.

Slots use letter names `a` through `j`. Legacy aliases `one` through `ten` and
`1` through `10` still work.

New tmux sessions use `codex-1` through `codex-10` for compatibility with the
older command surface. Existing migration-era `plugdeck-a` through `plugdeck-j`
sessions are still detected and managed. These names do not bind a slot to
Codex.

## Generic tmux slots

`mbx resume <slot>` and `mbx r <slot>` target the selected tmux session directly.
They never inspect Codex history, invoke `codex resume`, or choose a global
latest conversation. If the tmux session does not exist, they create a plain
shell workspace in the requested directory (or the current directory).
After attaching, the slot can run any shell command, editor, Codex process, or
other program.

`mbx start <slot>` and `mbx new <slot>` remain optional Codex launcher
conveniences. `start` attaches to a running slot, starts Codex in an idle shell
slot, or creates a new slot with Codex. `new` replaces the slot and starts a
fresh Codex process. Neither command creates a permanent slot-to-Codex
assignment.

`mbx stop <slot>` and `mbx q <slot>` kill the selected tmux session. After a
slot is stopped, the next `mbx r <slot>` creates a new tmux shell workspace
because there is no session left to resume. `mbx s <slot>` is the short form
of the optional Codex-launching `mbx start <slot>` command.

## Working Directory Rule

When no directory is provided, `mbx start`, `mbx new`, and a missing `mbx
resume` slot use the current directory. Resuming an existing tmux slot keeps
the directory already active inside that slot.

If a slot already exists but is sitting at a shell prompt, `mbx start <slot>`
starts Codex inside that idle slot using the current directory from the command
that invoked `mbx`.
