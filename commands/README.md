# Mobailmux Commands

This package installs `mbx`, a small command system for managing lettered
tmux sessions that run Codex.

It does not require the Mobailmux web UI.
It requires Bash, tmux, Codex, and `flock` (normally provided by util-linux).

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
- `mbx resume <slot>` resumes the last Codex conversation used in that slot.
- `mbx resume <slot> --session-id <id>` deliberately switches the slot to a
  different saved conversation.
- `mbx r <slot>` is shorthand for `mbx resume <slot>`.
- `mbx stop <slot>` stops one tmux session.
- `mbx stop all` stops all Mobailmux/Codex slots.
- `mbx status` shows every available session name and whether it is running.
- `mbx status <slot>` shows one session name.
- `mbx slots` and `mbx sessions` are aliases for `mbx status`.
- `mbx list` shows only currently running sessions.
- `mbx help`, `mbx command`, and `mbx commands` list the public command surface.

If a slot exists but is idle at a shell prompt, `mbx start <slot>` starts Codex
inside that idle slot from the directory where you ran `mbx`.

Internally, new sessions use `codex-1` through `codex-10`, with `a` mapped to
`codex-1` and `j` mapped to `codex-10`. During migration, `mbx` also detects
existing `plugdeck-a` through `plugdeck-j` sessions so old running slots remain
manageable.

Each new Codex session is remembered in
`${XDG_STATE_HOME:-~/.local/state}/mbx/slots` under its slot letter. This is
only the slot's last-used pointer: `mbx new <slot>` or a fresh `mbx start
<slot>` replaces it, so slots remain reusable across projects. Resume never
falls back to Codex's global `--last` selection. Tmux mouse mode is enabled by
default for the server managed by `mbx`, so mouse-wheel scrolling works in the
slots.

See [COMMANDS.md](COMMANDS.md) for the command contract used by downstream
Mobailmux packages.
