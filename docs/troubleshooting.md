# Troubleshooting

First check that you are using the unofficial community [Codex Desktop Linux build](https://github.com/ilysenko/codex-desktop-linux) with its [`shared-app-server-socket` feature](https://github.com/ilysenko/codex-desktop-linux/blob/main/linux-features/shared-app-server-socket/README.md) enabled. Codex Session Control does not work with OpenAI's official ChatGPT Desktop app.

## The local plugin is stale

From the Codex Session Control checkout, update and install the plugin again:

```bash
git pull --ff-only
./scripts/install-local-plugin.sh
```

Start a new Desktop task or CLI session afterward.

## A compatibility warning appears

Codex Session Control was tested against a different Codex version than the one bundled with Desktop. The operation may succeed, but compatibility is not guaranteed.

Update both the community Desktop build and Codex Session Control. If the warning remains after updating, follow the reporting steps below.

## Report a problem

If updating does not solve the problem, open a [bug report](https://github.com/agentlehub/codex-session-control/issues/new?template=bug.yml). Include the exact error or warning and the versions of Codex Session Control and the community Desktop build. Remove credentials and private task content.
