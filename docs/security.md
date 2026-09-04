# Security

Codex Session Control is a local, per-user tool. It relies on Unix user
separation and validates file ownership, type, path, and permissions. It cannot
protect against a hostile process already running as the same Linux user.

## Endpoint trust boundary

For every independent operation, the plugin resolves and validates the
Desktop-owned endpoint afresh. The derived directory must be a same-user
directory with mode `0700`, and the socket must be a same-user Unix socket with
mode `0600` or `0700`. The plugin rejects symbolic links, unexpected file types
or owners, unsafe permissions, and validation races.

The explicit bridge socket takes precedence when supplied. The plugin never
puts its selected path or environment values in public errors. Redact these
values, credentials, and task content from reports.

The plugin neither owns the Desktop authority nor manages its lifecycle. It
does not use TCP, scan for sockets, or read legacy state.

## Authentication and data

Codex Session Control does not sign in, copy, synchronize, or store credentials.
Codex/Desktop owns authentication and task storage. The plugin creates no
second login or task database.

## MCP caller trust

Any same-user host that loads the plugin can read and mutate tasks through the
exposed tools. Caller metadata and annotations do not authenticate a caller, so
enable the plugin only for trusted hosts. The plugin cannot approve prompts or
provide interactive input on a caller's behalf.

The endpoint must come from the
[community Codex Desktop build](https://github.com/ilysenko/codex-desktop-linux)
with `shared-app-server-socket` enabled, not OpenAI's official ChatGPT Desktop
app.

See [Architecture](architecture.md) for at-most-once and `outcome_unknown`
semantics.

## Reporting vulnerabilities

Do not open a public issue for a suspected vulnerability. Follow the private
reporting instructions in [SECURITY.md](../SECURITY.md).
