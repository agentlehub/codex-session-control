# Security

Codex Session Control is a local, per-user tool. It relies on Unix user separation and validates file ownership, type, path, and permissions. It cannot protect against a hostile process already running as the same Linux user.

## Endpoint trust boundary

Desktop starts and owns the app-server. Codex Session Control connects only to the private shared socket provided by the supported community Desktop build. It does not start Desktop, listen on TCP, or scan for other sockets.

For each operation, the plugin validates a same-user endpoint. The directory must have mode `0700`, and the Unix socket must have mode `0600` or `0700`. The plugin rejects symbolic links, unexpected file types or owners, unsafe permissions, and validation races.

## Authentication and data

Codex Session Control does not sign in, copy, synchronize, or store credentials. Codex Desktop owns authentication and task storage. The plugin creates no second login or task database.

## MCP caller trust

Any same-user host that loads the plugin can read and change tasks through the exposed tools. Caller metadata and annotations do not authenticate a caller, so enable the plugin only for trusted hosts. The plugin cannot approve prompts or provide interactive input on a caller's behalf.

The endpoint must come from the supported community build. See [Desktop support](desktop.md) for the required build and feature, and [Architecture](architecture.md) for at-most-once and `outcome_unknown` behavior.

## Reporting vulnerabilities

Do not open a public issue for a suspected vulnerability. Follow the private reporting instructions in [SECURITY.md](../SECURITY.md).
