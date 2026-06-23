# Security Notes

- Bind Mobailmux to a private interface, not `0.0.0.0`.
- Keep `.env`, local TOML, databases, uploads, and logs out of Git.
- Use a password hash in configuration, not a plaintext password.
- The Rust web UI can start local Codex jobs. Restrict workdirs and keep the
  service on loopback, LAN, VPN, or tailnet access.
- Run `cargo run -- audit-public` before publishing. It checks tracked files
  for ignored private paths, common secret markers, Tailscale-style private
  IPs, private key material, and optional local denylist terms.
- Install local Git hooks with `cargo run -- audit-public --install-hook`.

Host-specific denylist terms can be stored in ignored files:

```text
docs/private/audit-denylist.txt
.mobailmux/audit-denylist.txt
```
