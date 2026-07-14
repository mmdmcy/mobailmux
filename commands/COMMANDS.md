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
Codex session backed by tmux. Mobailmux enables tmux mouse mode (`mouse on`)
for the server so wheel scrolling works in every managed slot.

Slots use letter names `a` through `j`. Legacy aliases `one` through `ten` and
`1` through `10` still work.

New tmux sessions use `codex-1` through `codex-10`. Existing migration-era
`plugdeck-a` through `plugdeck-j` sessions are still detected and managed.

## Last-Used Conversation

When `mbx start` or `mbx new` launches Codex, `mbx` remembers the new
conversation as that slot's last-used conversation under
`${XDG_STATE_HOME:-~/.local/state}/mbx/slots`. This is a mutable pointer, not a
permanent project or session assignment. A later fresh start replaces it, so
slots can be reused for different projects and conversations.

`mbx resume` uses the remembered ID instead of global `codex resume --last`.
Use `mbx resume <slot> --session-id <id>` when deliberately moving another
saved conversation into a slot. If a slot has no remembered conversation yet,
run `mbx start <slot>` or `mbx new <slot>` once.

## Working Directory Rule

When no directory is provided, `mbx start`, `mbx new`, and `mbx resume` use the
current directory.

If a slot already exists but is sitting at a shell prompt, `mbx start <slot>`
starts Codex inside that idle slot using the current directory from the command
that invoked `mbx`.
