# Contributing

Preserve the public MCP and plugin interfaces and Codex-owned data. Do not modify Codex state that Codex Session Control does not own.

## Development

Use the pinned Rust toolchain and locked dependencies. Before opening a pull request, run:

```bash
./scripts/check.sh
```

Every check must pass. Fix all failures before opening the pull request.

If you change MCP tool names, inputs, or descriptions, review the affected tool catalog before opening a pull request. Check that the names, descriptions, and inputs are accurate and easy to understand.

## Design boundaries

Keep Codex Session Control a local, stateless plugin that connects only to the supported Desktop-owned socket. Do not add a service, daemon, second endpoint, session store, or automatic action retries.

## Pull requests

Keep each pull request focused and use the [pull request template](.github/PULL_REQUEST_TEMPLATE.md). Include fresh output from the relevant checks. Do not commit generated release files, credentials, Codex state, or machine-specific paths. Use Conventional Commits, for example `fix(mcp): preserve ambiguous dispatch`.

Explain any user-visible, compatibility, security, runtime, or release impact and link the relevant issue. Follow [SECURITY.md](SECURITY.md) for security-sensitive changes instead of discussing vulnerabilities in a public issue.
