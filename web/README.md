# Mobailmux Web

This directory is reserved for the optional Mobailmux web UI.

The web UI should stay standalone and downstream from `commands/`:

- no dependency on the Mattermost connector
- no required secrets in this directory
- no duplicated tmux or Codex slot logic

When implemented, this package should use the command contract in
`../commands/COMMANDS.md` and document its own install, run, and build commands
here.
