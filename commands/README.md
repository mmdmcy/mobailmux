# Mobailmux Commands

This package installs `mbx`, a small command system for managing numbered
tmux sessions that run Codex.

It does not require the Mobailmux web UI or Mattermost connector.

## Install

```bash
./install.sh
```

To replace the old `aione`, `ailist`, `aistopone`, and related helpers:

```bash
./install.sh --remove-legacy
```

Legacy commands are moved to a timestamped backup folder under:

```text
~/.local/share/mobailmux/
```

## Usage

```bash
mbx start a [directory] [prompt...]
mbx s a [directory] [prompt...]
mbx new a [directory] [prompt...]
mbx n a [directory] [prompt...]
mbx resume a [directory]
mbx r a [directory]
mbx stop a
mbx stop all
mbx status [a|all]
mbx list
mbx slots [a|all]
mbx sessions [a|all]
mbx check
mbx help
mbx command
mbx commands
```

A slot is a named Codex session backed by tmux. Slots use letter names by
default. The old word and number aliases still work:

```bash
mbx start a
mbx r b
mbx start one
mbx start 1
```

When no directory is given, `mbx` uses the directory you are currently in:

```bash
cd ~/projects/example
mbx start a
```

Useful options:

```bash
mbx start a --safe ~/Documents/github/example
mbx s b --unsafe --no-attach ~/projects/example "check the docs"
mbx new c ~/Documents/github/example
mbx start d --dry-run ~/Documents/github/example
```

## Behavior

- `mbx start <slot>` attaches to an existing slot, or starts it if missing.
- `mbx s <slot>` is shorthand for `mbx start <slot>`.
- `mbx new <slot>` restarts the slot from scratch.
- `mbx n <slot>` is shorthand for `mbx new <slot>`.
- `mbx resume <slot>` runs `codex resume --last` in that slot.
- `mbx r <slot>` is shorthand for `mbx resume <slot>`.
- `mbx stop <slot>` stops one tmux session.
- `mbx stop all` stops all numbered Mobailmux/Codex slots.
- `mbx status` shows every available session name and whether it is running.
- `mbx status <slot>` shows one session name.
- `mbx slots` and `mbx sessions` are aliases for `mbx status`.
- `mbx list` shows only currently running sessions.
- `mbx help`, `mbx command`, and `mbx commands` list the public command surface.

If a slot exists but is idle at a shell prompt, `mbx start <slot>` starts Codex
inside that idle slot from the directory where you ran `mbx`.

Internally, sessions keep the existing `codex-1` through `codex-9` tmux names,
with `a` mapped to `codex-1`, so existing running sessions remain manageable
during the migration.

See [COMMANDS.md](COMMANDS.md) for the command contract used by downstream
Mobailmux packages.
