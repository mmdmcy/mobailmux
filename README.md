# Mobailmux

Mobailmux is an ultra-small wrapper around tmux. It gives you ten persistent,
plain terminal slots named `a` through `j`.

It has no agent integration, configuration service, database, browser UI,
automatic commands, or terminal-output scraping. A slot is just a normal shell;
use it for Pi, Codex, another harness, a development server, or ordinary command
line work.

## Install

Requirements: Bash and tmux.

```bash
commands/install.sh
```

This creates `~/.local/bin/mbx`. The default installation is a symlink to this
checkout. Use `commands/install.sh --copy` for a standalone copy.

## Use

```bash
mbx r a        # create terminal a, or return to it
mbx r b        # create terminal b, or return to it
mbx check      # estimate which commands are working or quiet
mbx check b    # check terminal b
mbx status     # inspect all ten terminals from outside tmux
mbx status b   # inspect terminal b
mbx stop a     # stop terminal a
mbx stop all   # stop every Mobailmux terminal
```

A new slot starts as an untouched shell in your home directory. From there,
`cd` wherever you want and run whatever you want. Mobailmux does not type a
command, launch a harness, clear the screen, change an existing slot's working
directory, or restart a process.

Detach without stopping your work with `Ctrl-b d`. Running `mbx r a` later
returns to exactly the terminal you left.

Mouse support is enabled for each Mobailmux slot, including existing slots when
you resume them. Use the mouse wheel or a touchpad to scroll through tmux
history; press `q` or `Esc` to leave scrollback mode.

## Status

`mbx status` works inside or outside tmux:

```text
SLOT  STATE    ATTACHED  COMMAND        DIRECTORY
a     IDLE     no        bash           /terminal-home
b     ACTIVE   no        pi             /projects/site
c     EMPTY    -         -              -
```

- `IDLE`: the terminal is at a shell prompt and ready for another command.
- `ACTIVE`: a foreground command currently owns the terminal.
- `EXITED`: tmux is retaining a pane whose command exited.
- `EMPTY`: the slot does not exist.

This is deliberately tmux-level status. An interactive AI harness remains
`ACTIVE` while it is open, even when it is waiting at its own prompt; Mobailmux
does not install hooks or inspect its output to guess whether an agent turn is
finished.

## Activity Check

`mbx check` adds a deliberately conservative heuristic based on tmux's native
last-activity timestamp:

```text
SLOT  SIGNAL   SILENT    COMMAND        DIRECTORY
a     DONE     -         bash           /terminal-home
b     WORKING  4s        pi             /projects/site
c     QUIET    83s       codex          /projects/app
```

- `DONE`: the foreground command ended and the shell is back.
- `WORKING`: the terminal produced activity within the last 5 seconds.
- `QUIET`: a foreground command is still open, but its terminal has been silent
  for at least 5 seconds.

`QUIET` is a useful "probably done or waiting" signal, not proof. An agent can
be silently thinking, waiting on a network request, or running a command that
does not print output. Conversely, an idle TUI may occasionally redraw itself.
The command never reads the terminal text; it only reads tmux's activity time.
Full-screen TUI redraws count as activity, including counters such as Codex's
`Thinking... 17s`, even when they repeatedly overwrite the same line.

Set `MBX_QUIET_SECONDS` to change the default 5-second threshold:

```bash
MBX_QUIET_SECONDS=120 mbx check
```

## Check

```bash
shellcheck commands/bin/mbx commands/install.sh
python3 -m unittest discover -s tests -v
```
