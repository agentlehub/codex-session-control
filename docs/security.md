# Security

Codex Session Control relies on Unix user separation. A hostile process already
running as the same Linux user is outside its protection boundary.

## Endpoint trust boundary

For every independent operation, the plugin resolves and validates the
Desktop-owned endpoint afresh. The derived directory must be a same-user
directory with mode `0700`, and the socket must be a same-user Unix socket with
mode `0600` or `0700`. The plugin rejects symbolic links, unexpected file types or
owners, unsafe permissions, and validation races.

The explicit bridge socket takes precedence when supplied. The plugin never
puts its selected path or environment values in public errors. Redact these
values, credentials, and task content from reports.

The plugin neither owns the Desktop authority nor manages its lifecycle. It
does not use TCP, scan for sockets, or read legacy state.

## MCP caller trust

Any MCP client running as the same Linux user can invoke the tools. Caller
metadata and annotations do not authenticate a caller. The plugin cannot
approve prompts or provide interactive input on a caller's behalf.

See [Architecture](architecture.md) for at-most-once and `outcome_unknown`
semantics.

## Reporting vulnerabilities

Do not open a public issue for a suspected vulnerability. Follow the private
reporting instructions in [SECURITY.md](../SECURITY.md).
