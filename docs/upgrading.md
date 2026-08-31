# Upgrading from a historical 0.3.x installation

This is a one-time, historical 0.3.x cutover. The checkout installer does not perform automatic migration or legacy cleanup.

1. Remove the old plugin registration, remove its marketplace, and disable the
   old user unit in this order:

   ```bash
   codex plugin remove codex-session-control@codex-session-control-local
   codex plugin marketplace remove codex-session-control-local
   systemctl --user disable --now codex-session-control.service
   ```

2. Start upstream Codex Desktop with `shared-app-server-socket` enabled and
   verify that its private shared socket is available.

3. From the stable new checkout, run:

   ```bash
   ./scripts/install-local-plugin.sh
   ```

4. Start a new CLI/Desktop task so the host loads the staged plugin.

5. Verify the same thirteen-tool catalog and confirm there is no old CSC authority.

Native plugin removal does not delete the checkout or its staged binary, and it
does not terminate processes already owned by running sessions or tasks.
