# Troubleshooting

First check that you are using the unofficial community [Codex Desktop Linux build](https://github.com/ilysenko/codex-desktop-linux) with its [`shared-app-server-socket` feature](https://github.com/ilysenko/codex-desktop-linux/blob/main/linux-features/shared-app-server-socket/README.md) enabled. Codex Session Control does not work with OpenAI's official ChatGPT Desktop app.

## Desktop is closed or the shared socket is unavailable

Open Desktop and wait until it is ready. If Desktop or its socket restarted, retry the operation. The plugin reconnects on its next operation, so a restart alone does not require a new CLI session or Desktop task.

## The plugin is not visible or enabled

In Desktop's Plugins UI, confirm that Codex Session Control is enabled. Create a new Desktop task or CLI session after enabling, disabling, installing, or updating the plugin.

## The local plugin is stale

From the Codex Session Control checkout, update and install the plugin again:

```bash
git pull --ff-only
./scripts/install-local-plugin.sh
```

Start a new Desktop task or CLI session afterward.

## An MCP operation reports an error

Include the exact error, whether Desktop was open, and whether the plugin was visible and enabled in a [bug report](https://github.com/agentlehub/codex-session-control/issues/new?template=bug.yml). Remove credentials and private task content.

## An MCP mutation reports `outcome_unknown`

The request may have reached Codex. Do not retry blindly because that could repeat the action. Inspect the task with `thread_read` or `threads_list`, then decide what to do from its current state.

## A compatibility warning appears

Update both the community Desktop build and the Codex Session Control checkout, then run the local plugin installer again and start a new Desktop task or CLI session. Reinstalling Codex Session Control alone cannot update Desktop's bundled app-server.

## Codex is signed out

Sign in through the community Desktop build. Codex Session Control does not copy or manage credentials.
