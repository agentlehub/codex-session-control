# Contributing

Preserve the public CLI and MCP interfaces, the saved `CODEX_HOME`, and the configured app-server socket. Do not modify Codex state that Codex Session Control does not own.

## Development

Use the pinned Rust toolchain and locked dependencies. Before opening a pull request, run:

```bash
./scripts/check.sh
```

Every check must pass. Fix all failures before opening the pull request.

If you change CLI commands, options, or help text, review the affected help output before opening a pull request. Run `cargo run --locked -- --help` for top-level changes or `cargo run --locked -- <command> --help` for a specific command. Check that the usage, descriptions, commands, and options are accurate and easy to understand.

Do not run the following systemd integration tests on a development workstation. GitHub Actions runs them in an isolated environment:

```text
live_normal_home_shared_authority
live_normal_home_restart_boundaries
live_normal_home_projection_preservation
live_normal_home_uninstall_preservation
```

## Design boundaries

Keep the direct systemd-managed Codex app-server, at-most-once actions, and strict Desktop verification. Do not add another daemon or endpoint, cross-service coordination, automatic action retries, or an unverified Desktop path.

## Pull requests

Keep each pull request focused and use the [pull request template](.github/PULL_REQUEST_TEMPLATE.md). Include fresh output from the relevant checks. Do not commit generated release files, credentials, Codex state, or machine-specific paths. Use Conventional Commits, for example `fix(mcp): preserve ambiguous dispatch`.

Explain any user-visible, compatibility, security, lifecycle, or release impact and link the relevant issue. Follow [SECURITY.md](SECURITY.md) for security-sensitive changes instead of discussing vulnerabilities in a public issue.
