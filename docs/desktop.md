# Desktop support

Codex Session Control requires the unofficial community [Codex Desktop Linux build](https://github.com/ilysenko/codex-desktop-linux) with its [`shared-app-server-socket` feature](https://github.com/ilysenko/codex-desktop-linux/blob/main/linux-features/shared-app-server-socket/README.md) enabled. It does not work with OpenAI's official ChatGPT Desktop app. Desktop must remain open because it owns the app-server and shared socket.

## Install Codex Desktop

Follow the community project's [installation instructions](https://github.com/ilysenko/codex-desktop-linux) and enable the [`shared-app-server-socket` feature](https://github.com/ilysenko/codex-desktop-linux/blob/main/linux-features/shared-app-server-socket/README.md).

## Install and enable the plugin

Run the [local plugin installer](../README.md#install) from the Codex Session Control checkout. In Desktop's Plugins UI, enable Codex Session Control, then create a new task. Changes to the plugin take effect only in new Desktop tasks and CLI sessions.

## Use from Desktop and CLI

Create a task in Desktop, or launch the community build's CLI with:

```bash
codex-desktop --cli
```

## Connection behavior

The plugin connects to Desktop's private shared socket and does not start Desktop. If Desktop or its socket restarts, retry the operation. The plugin reconnects on its next operation, so a restart alone does not require a new CLI session or Desktop task.
