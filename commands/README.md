# Mobailmux Commands

This package installs `mbx`, a small command system for managing reusable
lettered tmux workspaces. Codex launching is an optional convenience.

It does not require the Mobailmux web UI.
The generic slot commands require Bash and tmux. `mbx start`, `mbx new`, and
`mbx check` additionally use Codex.

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
mbx q a
mbx stop all
mbx q all
mbx status [a|all]
mbx list
mbx slots [a|all]
mbx sessions [a|all]
mbx check
mbx help
mbx command
mbx commands
```

A slot is a reusable tmux workspace, not a Codex-specific session. Slots use
letter names by default. The old word and number aliases still work:

```bash
mbx start a
mbx s a
mbx r b
mbx q b
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
mbx start b --unsafe --no-attach ~/projects/example "check the docs"
mbx new c ~/Documents/github/example
mbx start d --dry-run ~/Documents/github/example
```

## Behavior

- `mbx start <slot>` attaches to an existing slot, or starts it if missing.
- `mbx s <slot>` is shorthand for `mbx start <slot>`.
- `mbx new <slot>` replaces the slot and starts a fresh Codex process.
- `mbx n <slot>` is shorthand for `mbx new <slot>`.
- `mbx resume <slot>` attaches to the selected tmux slot, creating a plain
  shell slot if it is missing.
- `mbx r <slot>` is shorthand for `mbx resume <slot>`.
- `mbx stop <slot>` stops one tmux session.
- `mbx q <slot>` is shorthand for `mbx stop <slot>`.
- `mbx stop all` stops all Mobailmux slots.
- `mbx status` shows every available session name and whether it is running.
- `mbx status <slot>` shows one session name.
- `mbx slots` and `mbx sessions` are aliases for `mbx status`.
- `mbx list` shows only currently running sessions.
- `mbx help`, `mbx command`, and `mbx commands` list the public command surface.

If a slot exists but is idle at a shell prompt, `mbx start <slot>` starts Codex
inside that idle slot from the directory where you ran `mbx`. Once a slot is
attached, it can be used for any command or project.

Internally, new tmux sessions use `codex-1` through `codex-10`, with `a` mapped
to `codex-1` and `j` mapped to `codex-10`. During migration, `mbx` also detects
existing `plugdeck-a` through `plugdeck-j` sessions so old running slots remain
manageable. `mbx resume` attaches to the exact tmux session and never consults
Codex conversation history. Tmux mouse mode is enabled by default for the
server managed by `mbx`, so mouse-wheel scrolling works in every slot.

After `mbx stop <slot>`, the next `mbx r <slot>` creates a new shell workspace;
there is no old tmux session left to resume.

See [COMMANDS.md](COMMANDS.md) for the command contract used by downstream
Mobailmux packages.
