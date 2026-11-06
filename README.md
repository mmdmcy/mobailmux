# Mobailmux

Mobailmux is an ultra-small wrapper around tmux. It gives you ten persistent,
plain terminal slots named `a` through `j`.

It has no configuration service, database, browser UI, or automatic commands;
its few settings are one plain text file (see [Settings](#settings)).
A slot is just a normal shell; use it for Claude Code, Codex, Pi, another
harness, a development server, or ordinary command line work. For AI agents,
three read-only commands show what runs in the other slots (see
[For AI Agents](#for-ai-agents)).

## Install

Requirements: Bash and tmux.

```bash
commands/install.sh
```

This creates `~/.local/bin/mbx`. The default installation is a symlink to this
checkout. Use `commands/install.sh --copy` for a standalone copy.

## Use

```bash
mbx ui         # sidebar of all terminals; tap one to switch
mbx r a        # create terminal a, or return to it
mbx r b        # create terminal b, or return to it
mbx check      # estimate which commands are working or quiet
mbx check b    # check terminal b
mbx status     # inspect all ten terminals from outside tmux
mbx status b   # inspect terminal b
mbx stop a     # stop terminal a
mbx stop all   # stop every Mobailmux terminal
mbx save       # save now (it also saves every 30s in the background)
mbx restore    # bring them back after a reboot (mbx ui does this itself)
mbx who        # which agent runs in each terminal, and what it works on
mbx log b      # the recent conversation of the agent in terminal b
mbx peek b     # what terminal b shows right now, as text
mbx config     # show the settings; mbx config layout top changes one
```

A new slot starts as an untouched shell in your home directory. From there,
`cd` wherever you want and run whatever you want. Mobailmux does not type a
command, launch a harness, clear the screen, change an existing slot's working
directory, or restart a process.

To close a terminal, quit what runs in it and type `exit` at its shell, or use
`mbx stop a`. Detach without stopping your work with `Ctrl-b d`. Running `mbx r a` later
returns to exactly the terminal you left.

Mouse support is enabled for each Mobailmux slot, including existing slots when
you resume them. Use the mouse wheel or a touchpad to scroll through tmux
history; press `q` or `Esc` to leave scrollback mode.

## Sidebar UI

`mbx ui` shows every running slot in a narrow sidebar, with the selected slot
beside it, all inside your current terminal. It needs nothing beyond tmux, so it
works the same over SSH from a phone as on the desktop.

```text
 MOBAILMUX      │mac@Mac ~/projects/site %
                │
 a ○ zsh        │
   ~            │
 b ● pi         │
   site         │
                │
 + new          │
```

On a window narrower than 80 columns, such as a phone in portrait over SSH,
the sidebar becomes one row of tabs above the slot, so the slot keeps the full
width. Each tab shows the slot, its signal, and its folder; folder names
shrink evenly until every tab fits.

```text
 a○ ~  b● site  c● api  +
──────────────────────────
mac@Mac ~/projects/site %
```

Turning the phone sideways switches to the sidebar and back. To keep one
layout, run `mbx config layout side` or `mbx config layout top`; to show the
program instead of the folder, `mbx config tab-label program` (see
[Settings](#settings)).

- Tap or click a slot to show it. `+ new` (or `+` on the tab row) creates the
  next empty slot.
- `○` shell prompt, `●` yellow working, `●` green quiet, `✕` exited — the same
  signals as `mbx check`.
- Keys, `Ctrl-b`, scrolling, and `mbx r <slot>` all work normally inside the
  shown slot.
- `Ctrl-b d` closes the UI. Your slots keep running.
- To close a slot for good, quit what runs in it (for example `/exit` in
  Claude Code or Codex), then type `exit` at the shell. The UI moves on to
  another slot, and the slot will not come back after a reboot.

Copying works with the mouse: highlight text and it lands in the system
clipboard (`pbcopy` on macOS, `wl-copy` or `xclip` on Linux), and so do copies
made by apps inside a slot, such as Claude Code's. mbx sets tmux's
`copy-command` and a `pane-set-clipboard` hook for this, because Apple's
Terminal ignores the OSC 52 copy sequence those apps send. With no clipboard
tool on the device (for example over SSH), copies are passed outward as OSC 52
for terminals that support it.

The UI is a throwaway tmux session that only views your slots, so each device
can open its own. `mbx config width 20` changes the sidebar width (default 16).

## Surviving a Reboot

tmux sessions cannot survive a shutdown, so Mobailmux remembers them instead.
A small background job inside the tmux server saves a snapshot every 30
seconds while any slot exists, whether or not you are attached; `mbx save`
saves immediately. The job starts with `mbx r`, `mbx ui`, `mbx save`, or
`mbx restore`, and ends with the tmux server. `mbx config save 60` changes
the interval. After a reboot, the first `mbx ui` (or `mbx restore`) recreates every
saved slot in its directory and types the command that resumes its agent:

- Claude Code: `claude --resume <session id>`, read from the running CLI's
  `~/.claude/sessions/<pid>.json`.
- Codex: `codex resume <thread id>` in the thread's own directory, from the
  command line or the thread lock no other slot claimed; otherwise the
  `codex resume` picker.
- Anything else: a plain shell in the same directory.

Permission and model flags such as `--dangerously-skip-permissions` or
`--model` carry over; prompts are never replayed. The snapshot is a plain
tab-separated file at `~/.local/state/mobailmux/slots.tsv` that you can edit.
`mbx stop` removes stopped slots from it, so they do not come back.

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

`mbx config quiet 120` changes the default 5-second threshold; for one run:

```bash
MBX_QUIET_SECONDS=120 mbx check
```

## Settings

`mbx config` shows every setting, its value, and where the value comes from:

```text
SETTING     VALUE    FROM              MEANING
layout      top      file              where mbx ui lists the slots; auto is tabs on top below tabs-below columns
tab-label   folder   default           what a tab shows beside its slot letter
tabs-below  80       default           auto layout: tabs on top when the window is narrower than this
width       16       default           width of the sidebar
quiet       5        default           silence before a slot counts as quiet (yellow dot turns green)
save        30       default           how often the slots are saved for after a reboot
```

```bash
mbx config layout top        # change one; refused unless the value is allowed
mbx config layout            # one setting, with its allowed values
mbx config layout --reset    # back to the default
mbx config --format json     # all of them, for scripts
```

| Setting      | Allowed                    | Default  | Variable            |
| ------------ | -------------------------- | -------- | ------------------- |
| `layout`     | `auto`, `side`, `top`      | `auto`   | `MBX_UI_LAYOUT`     |
| `tab-label`  | `folder`, `program`        | `folder` | `MBX_UI_TAB_LABEL`  |
| `tabs-below` | columns, 1 or more         | `80`     | `MBX_UI_TABS_BELOW` |
| `width`      | columns, at least 8        | `16`     | `MBX_UI_WIDTH`      |
| `quiet`      | seconds, 1 or more         | `5`      | `MBX_QUIET_SECONDS` |
| `save`       | seconds, 1 or more         | `30`     | `MBX_SAVE_SECONDS`  |

They live in `~/.config/mobailmux/config` (or `$XDG_CONFIG_HOME/mobailmux/config`)
as `setting=value` lines, which you can also edit by hand. mbx reads the file,
never runs it, and refuses a value that is not allowed, naming the file. An
open `mbx ui` follows a change at once, and the background saver uses the new
`save` interval on its next round. A setting's variable wins over the file,
for one shell or one run; `mbx config` then names the variable under FROM.

## For AI Agents

An agent in one slot can see the others, and so can you. All three commands
only read; `who` and `log` need `jq`.

```text
$ mbx who
SLOT  STATE  PROGRAM  NAME     FOLDER           DOING
a     busy   claude   api-a5   ~/projects/api   Rate limits for the API
b     idle   claude   site-8e  ~/projects/site  Login page fix
c*    busy   codex    -        ~/projects/docs  Agent guide
d     done   bash     -        ~                -
```

- `mbx who [slot] [--format json]` lists every running slot. `*` marks the
  slot you run it from. STATE is Claude Code's own status (`busy`, `idle`,
  ...) when the slot runs Claude Code. Otherwise it is the `mbx check`
  signal in lower case (`working`, `quiet`, `done`, `exited`). NAME is the
  Claude Code session name. DOING is the session's own title (Claude Code's
  title, or the Codex thread name), or else its last prompt. JSON adds the
  session id, transcript path, full last prompt, pid, and `attached`.
- `mbx log <slot> [-n N] [--format json]` shows the last N entries (default
  20) of the conversation, read from the agent's own transcript: Claude
  Code's `~/.claude/projects/*/<session>.jsonl` or Codex's rollout file.
  It shows prompts, messages from other sessions, replies, and tool calls
  (first line in text, in full in JSON). It reads from the end of the file,
  so transcripts of hundreds of megabytes stay fast.
- `mbx peek <slot> [-n N] [--format json]` prints the screen as plain text
  (`tmux capture-pane`), for any program. With `-n N` it prints the last N
  lines including scrollback.

Claude Code sessions already message each other with Claude Code's own
`SendMessage` tool. The address is the NAME that `mbx who` shows, which is
the same name `ListAgents` lists. Mobailmux adds no second channel, and it
never types into a slot to pass a message on.

`skills/mobailmux/SKILL.md` teaches an agent all of this. Link it into your
agents' skill folders (for example `~/.claude/skills/mobailmux`). After that,
"use mobailmux to see what my other sessions are doing" works in any session.

Measured on 2026-11-03, on a 4-core Linux machine at load average 12 with
seven Claude Code slots: `mbx who` took 1.3 to 1.6 s (1.4 s of CPU, 5 MB of
memory), and `mbx log` on a 59 MB transcript took 0.7 to 0.8 s. Nothing runs
in the background for these commands.

## Check

```bash
shellcheck commands/bin/mbx commands/install.sh
python3 -m unittest discover -s tests -v
```
