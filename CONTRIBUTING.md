# Contributing

Preserve the exact thirteen-tool public contract and the stateless stdio
entrypoint. Each independent operation must validate and connect to the
Desktop-owned endpoint; Codex Session Control must not acquire lifecycle or
authority ownership.

## Development

Use the pinned Rust toolchain and locked dependencies. Before opening a pull
request, run:

```bash
./scripts/check.sh
```

Every check must pass. Keep pull requests focused and include fresh output from
the relevant checks.

Use test-driven development for behavior changes: write the smallest failing
test, make it pass with the narrowest implementation, and rerun the affected
suite. Preserve at-most-once mutation behavior: an `outcome_unknown` result is
not permission to replay an action.

The repository gate covers native x86-64 and AArch64 plugin behavior. Any live
validation must use disposable tasks that are safe to mutate and must never
expose task content, credentials, socket paths, or environment values in
artifacts.

## Pull requests

Use the [pull request template](.github/PULL_REQUEST_TEMPLATE.md) and
Conventional Commits, for example `fix(mcp): preserve ambiguous dispatch`.
Describe user-visible or security impact and follow [SECURITY.md](SECURITY.md)
for security-sensitive changes instead of discussing vulnerabilities in a
public issue.
