# Security

Codex Session Control is a local, per-user tool. It relies on Unix user separation and validates file ownership, type, path, and permissions. It cannot protect against a hostile process already running as the same Linux user.

## Files, paths, and permissions

Codex Session Control gets `$HOME` from the current Linux account and uses `/run/user/<uid>` as its runtime directory. Before changing the installation or opening an MCP connection, it rejects identity mismatches, symbolic links, incorrect owners or file types, unsafe directory permissions, and paths outside those locations.

The selected `CODEX_HOME` belongs to Codex, not Codex Session Control. Codex Session Control owns only its executable, product configuration, release manifest, systemd user service, installed plugin files, Desktop connection file, and runtime directory. It does not own authentication, sessions, Codex configuration, or unrelated plugins.

Product directories use mode `0700`, and sensitive files use `0600`. The configured endpoint must be a Unix socket owned by the current user, with no group or other access. Codex Session Control does not change socket permissions after Codex creates it.

The systemd user service starts the configured Codex executable directly with the saved Codex home and socket. It does not use a shell or intermediary daemon.

## Authentication

Setup does not run `codex login`, copy credentials, or create another login. Authentication is not required to install or start Codex Session Control. Users sign in through Codex CLI or the supported Desktop build. Codex Session Control never reads, transforms, synchronizes, or stores credentials.

## MCP access

Any MCP client running under the same Linux user can invoke the tools. Access depends on Linux user permissions; MCP metadata, annotations, and caller identifiers do not authenticate the caller.

Every request goes to the local socket saved in Codex Session Control's configuration. Caller metadata cannot redirect it. The MCP server cannot approve prompts or provide interactive input on the user's behalf.

See [Architecture](architecture.md#mcp-connections) for transport, timeout, and at-most-once behavior.

## Release verification

Installation and updates use releases published by [`agentlehub/codex-session-control`](https://github.com/agentlehub/codex-session-control). Downloads use HTTPS, and release tags must not change after publication.

Before a binary is installed, it is verified against the exact SHA-256 checksum in that release's `SHA256SUMS` file. Updates also verify the asset name and size, executable architecture, and product version.

## Reporting vulnerabilities

Do not open a public issue for a suspected vulnerability. Follow the private reporting instructions in [SECURITY.md](../SECURITY.md).
