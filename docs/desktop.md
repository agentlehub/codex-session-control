# Desktop support

Codex Session Control uses upstream Codex Desktop with
`shared-app-server-socket` enabled. Desktop must be open: Desktop alone owns
the app-server and its shared socket lifecycle.

The plugin manifest forwards exactly these variables to the plugin process:

- `XDG_RUNTIME_DIR`
- `CODEX_LINUX_APP_ID`
- `CODEX_LINUX_APP_SERVER_BRIDGE_SOCKET`

Do not copy their values into reports or configuration. The plugin resolves the
socket from this forwarded context for each independent operation and rejects
an unavailable or unsafe endpoint.

## Plugins UI and task boundaries

Install or restage the plugin from a stable checkout, then open the Desktop
Plugins UI. Enable the Codex Session Control plugin there. Create a new task to
load its tools; tasks that were already open retain the tool set they loaded.

Disabling the plugin removes it from newly created tasks after the next task
boundary. Re-enabling it restores the plugin for newly created tasks. Neither
action changes an already running task's loaded process.

If Desktop is closed or its private shared socket is unavailable, open Desktop
and create a new task after the socket is ready. The plugin does not start or
replace Desktop's authority.
