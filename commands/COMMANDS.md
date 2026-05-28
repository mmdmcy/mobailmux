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
mbx start <slot> [directory] [prompt...]
mbx new <slot> [directory] [prompt...]
mbx resume <slot> [directory]
mbx stop <slot>
mbx stop all
mbx list
mbx check
mbx commands
```

## Working Directory Rule

When no directory is provided, `mbx start`, `mbx new`, and `mbx resume` use the
current directory.

If a slot already exists but is sitting at a shell prompt, `mbx start <slot>`
starts Codex inside that idle slot using the current directory from the command
that invoked `mbx`.
