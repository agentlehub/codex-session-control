# Troubleshooting

First verify that you run the
[community Codex Desktop build](https://github.com/ilysenko/codex-desktop-linux),
not OpenAI's official ChatGPT Desktop app, with `shared-app-server-socket`
enabled.

## Desktop is closed or the shared socket is unavailable

Open Desktop and wait for its private shared socket to become available. The
plugin does not start Desktop or scan for an alternative endpoint; it derives
and validates its fixed endpoint from the Desktop context.

If Desktop or its socket restarted, retry the operation from the existing loaded
plugin. It reconnects on its next operation, so a restart alone does not require
a new CLI session or Desktop task.

## The plugin is not visible or enabled

In the host's plugin controls, confirm that the local Codex Session Control
plugin is enabled. Plugin visibility is loaded at a new session or task boundary,
so create a new CLI session or Desktop task after enabling, disabling, installing,
or restaging it.

## The staged plugin is stale

From a stable checkout that tracks a remote branch, update to the intended
revision and stage a fresh native binary:

```bash
git pull --ff-only
./scripts/install-local-plugin.sh
```

Start a new CLI session or Desktop task afterward. If the installer reports a
registration collision, keep the checkout intact and resolve the conflicting
local plugin registration before trying again.

## An MCP operation reports an error

Capture the exact stderr or MCP error after redacting socket paths, environment
values, credentials, and task content. Include whether Desktop was open,
whether the plugin was visible and enabled, and whether a new session or task
was used. This evidence distinguishes an endpoint problem from a cached plugin
or host-visibility problem.

## An MCP mutation reports `outcome_unknown`

The request may have reached Codex. Do not retry blindly because that could
repeat the action. Inspect the thread with `thread_read` or `threads_list`, then
decide what to do from its current state.

## A compatibility warning appears

A compatibility warning means the Desktop-owned authority reported a protocol
version outside the version validated by this checkout. The plugin may still
work, but compatibility is not guaranteed. Restaging CSC alone cannot change
the Desktop app-server version. Align the community Desktop build's bundled
app-server version with the version supported by this checkout, then restage
the plugin.
