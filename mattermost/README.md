# Mobailmux Mattermost

This directory is reserved for the optional Mattermost connector.

The connector should stay standalone and downstream from `commands/`:

- no dependency on the web UI
- no committed bot tokens, channel IDs, or private server URLs
- no duplicated tmux or Codex slot logic

When implemented, this package should use the command contract in
`../commands/COMMANDS.md` and document its own configuration and service setup
here.
