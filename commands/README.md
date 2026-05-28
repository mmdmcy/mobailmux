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
mbx start one [directory] [prompt...]
mbx new one [directory] [prompt...]
mbx resume one [directory]
mbx stop one
mbx stop all
mbx list
mbx check
mbx commands
```

Slots can be written as words or numbers:

```bash
mbx start one
mbx start 1
```

When no directory is given, `mbx` uses the directory you are currently in:

```bash
cd ~/projects/example
mbx start one
```

Useful options:

```bash
mbx start one --safe ~/Documents/github/example
mbx start two --unsafe --no-attach ~/projects/example "check the docs"
mbx new three ~/Documents/github/example
mbx start four --dry-run ~/Documents/github/example
```

## Behavior

- `mbx start <slot>` attaches to an existing slot, or starts it if missing.
- `mbx new <slot>` restarts the slot from scratch.
- `mbx resume <slot>` runs `codex resume --last` in that slot.
- `mbx stop <slot>` stops one tmux session.
- `mbx stop all` stops all numbered Mobailmux/Codex slots.
- `mbx list` shows the running slots, current process, and directory.
- `mbx commands` lists the public command surface.

If a slot exists but is idle at a shell prompt, `mbx start <slot>` starts Codex
inside that idle slot from the directory where you ran `mbx`.

Internally, sessions keep the existing `codex-1` through `codex-9` tmux names
so existing running sessions remain manageable during the migration.

See [COMMANDS.md](COMMANDS.md) for the command contract used by downstream
Mobailmux packages.
