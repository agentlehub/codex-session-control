# Desktop support

Codex Session Control requires the unofficial community Codex Desktop build from
[ilysenko/codex-desktop-linux](https://github.com/ilysenko/codex-desktop-linux),
with `shared-app-server-socket` enabled. It does not work with OpenAI's official
ChatGPT Desktop app. Desktop must remain open because it owns the app-server and
shared socket.

## Supported Desktop

The community build is the only supported Desktop. Its app-server and private
shared socket are the authority used by the local plugin.

## Install it

Follow the community project's
[installation instructions](https://github.com/ilysenko/codex-desktop-linux)
and enable its
[`shared-app-server-socket` feature](https://github.com/ilysenko/codex-desktop-linux/blob/main/linux-features/shared-app-server-socket/README.md).
Those instructions belong to the community project so they stay accurate as its
build process changes.

## Enable the plugin

From a stable CSC checkout, run the [local installer](../README.md#install),
then enable the Codex Session Control plugin in Desktop's Plugins UI. Create a
new task to load its tools; tasks already open retain the tool set and plugin
process they loaded.

Disabling, re-enabling, installing, or restaging the plugin changes only newly
created tasks or CLI sessions.

## Use from Desktop and CLI

Create a task in Desktop, or launch the community build's CLI with:

```bash
codex-desktop --cli
```

Desktop must remain open while using the plugin because it owns the app-server
and shared socket.

## How the connection works

The plugin receives `XDG_RUNTIME_DIR`, `CODEX_LINUX_APP_ID`, and
`CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET` from its manifest. Users normally should
not set them manually. Do not copy their values into reports or configuration.

For every operation, the loaded plugin resolves and validates the private socket
from that context, then connects to Desktop. If Desktop or its socket restarts,
retry the operation from the existing loaded plugin; it reconnects on its next
operation. A restart alone does not require a new CLI session or Desktop task.

## Troubleshooting

If the plugin is not visible, Desktop is closed, or a compatibility warning
appears, see [Troubleshooting](troubleshooting.md).
