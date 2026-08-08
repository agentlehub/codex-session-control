# Troubleshooting

Run the read-only check first:

```bash
codex-session-control status
```

The output explains what is wrong and tells you which command to run next. Do not delete or edit Codex Session Control files manually to force recovery.

## The selected home is wrong

The first successful setup saves the selected `CODEX_HOME`. Changing the environment variable later does not switch the installation to another home. If `status` reports conflicting saved configuration, restore the matching files or uninstall and set up the intended Codex home again.

## Codex is signed out

An active login is not required before setup. Launch `codex-session-control codex` or the supported Desktop build and complete Codex's normal sign-in flow. Codex Session Control does not copy or manage credentials.

## Codex CLI cannot connect

If `codex-session-control codex` cannot connect, run:

```bash
codex-session-control status
```

Follow the reported repair command. Do not point Codex at another socket or add `--remote` manually; that can start two Codex services for the same sessions.

## The service is disabled or unavailable

If `status` reports a disabled service, run:

```bash
codex-session-control enable
```

If it reports a failed service, inspect the logs with `journalctl --user -u codex-session-control`, fix the reported problem, and rerun the suggested command. `status` does not change the installation.

## The installed command is not found

The executable installs to `$HOME/.local/bin/codex-session-control`. Add that directory to your shell's `PATH`, or run the absolute command printed during installation. The installer does not edit shell profiles.

## Updating while sessions are active

A change to the Codex executable or systemd service may require a restart. Review every listed active session and continue only if interrupting them is acceptable. Non-interactive updates cannot approve an interruption.

A service restart interrupts active Codex turns. They do not resume automatically. Active goals are not paused or cleared and may continue when a client resumes the session. Pause any goal that must not continue before approving the restart.

## A lifecycle command refuses to avoid disconnecting this task

If a restart-required `update`, active `disable`, or active `uninstall` reports that it is running through the managed app-server, rerun the printed command from an independent terminal that is not attached through Codex Session Control.

If a restart-required update cannot prove caller identity, repair or upgrade the systemd user environment so that `systemctl --user whoami` works, then rerun the printed update command from an independent terminal. Do not stop the service first to work around an update refusal.

For `disable` and `uninstall` only, an error may print an independent stop-then-rerun sequence. Run that exact sequence from an independent terminal:

```bash
systemctl --user stop codex-session-control.service
codex-session-control disable # or uninstall
```

An external socket or socket-parent error means the Desktop backend connection is unavailable. It does not show that `auth.json` was deleted; Codex Session Control does not manage Codex credentials.

## An MCP mutation reports `outcome_unknown`

The request may have reached Codex. Do not retry blindly because that could repeat the action. Inspect the session with `thread_read` or `threads_list`, then decide what to do from its current state.

## A compatibility warning appears

A compatibility warning means your installed Codex CLI version has not been tested with this release. Codex Session Control may still work normally, but compatibility is not guaranteed.

## Desktop does not attach

Run `codex-session-control status`. If it reports that no supported Desktop build was found, follow [Desktop support](desktop.md) and rerun `codex-session-control setup`. After `setup` or `enable`, fully quit and reopen Desktop; an already-running Desktop process does not switch connections.
