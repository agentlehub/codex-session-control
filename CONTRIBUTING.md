# Contributing

Preserve the public MCP and plugin interfaces and Codex-owned data. Do not
modify Codex state that Codex Session Control does not own.

## Development

Use the pinned Rust toolchain and locked dependencies. Before opening a pull
request, run:

```bash
./scripts/check.sh
```

Every check must pass. Fix all failures before opening the pull request.

Use test-driven development for behavior changes: write the smallest failing
test, make it pass with the narrowest implementation, and rerun the affected
suite. Preserve at-most-once mutation behavior: an `outcome_unknown` result is
not permission to replay an action.

The repository gate covers native x86-64 and AArch64 plugin behavior. Any live
validation must use disposable tasks that are safe to mutate and must never
expose task content, credentials, socket paths, or environment values in
artifacts.

## Design boundaries

Preserve those boundaries. Do not add a service, daemon, second endpoint,
lifecycle owner, automatic mutation replay, or an unverified Desktop path.

## Pull requests

Keep each pull request focused and use the
[pull request template](.github/PULL_REQUEST_TEMPLATE.md). Include fresh output
from the relevant checks. Do not commit generated release files, credentials,
Codex state, or machine-specific paths. Use Conventional Commits, for example
`fix(mcp): preserve ambiguous dispatch`.

Explain any user-visible, compatibility, security, lifecycle, or release impact
and link the relevant issue. Follow [SECURITY.md](SECURITY.md) for
security-sensitive changes instead of discussing vulnerabilities in a public
issue.
