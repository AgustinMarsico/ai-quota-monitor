import type { TuiPlugin, TuiPluginModule } from "@opencode-ai/plugin/tui";
import { createSignal } from "solid-js";
import { readFile, stat } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { compact, details, validate, type Snapshot } from "../shared/view.mjs";

const tui: TuiPlugin = async (api, options) => {
  const path =
    typeof options?.cachePath === "string"
      ? options.cachePath
      : join(
          process.env.XDG_CACHE_HOME || join(homedir(), ".cache"),
          "ai-quota/status.json",
        );
  const [text, setText] = createSignal("AI quota: ?");
  let snapshot: Snapshot | undefined;
  let busy = false;
  const read = async () => {
    if (busy || api.lifecycle.signal.aborted) return;
    busy = true;
    try {
      if ((await stat(path)).size > 4194304) throw new Error("Cache too large");
      snapshot = validate(JSON.parse(await readFile(path, "utf8")));
      if (!api.lifecycle.signal.aborted) setText(compact(snapshot));
    } catch {
      snapshot = undefined;
      if (!api.lifecycle.signal.aborted)
        setText("AI quota: ? (cache unavailable)");
    } finally {
      busy = false;
    }
  };
  await read();
  api.slots.register({
    slots: {
      app_bottom: () => (
        <box paddingLeft={1}>
          <text fg={api.theme.current.textMuted}>{text()}</text>
        </box>
      ),
    },
  });
  const unregister = api.keymap.registerLayer({
    commands: [
      {
        namespace: "palette",
        name: "ai-quota.status",
        title: "AI quota: show details",
        desc: "Read cached provider quota",
        category: "AI quota",
        slashName: "quota",
        run: () => {
          api.ui.dialog.replace(() =>
            api.ui.DialogAlert({
              title: "AI quota — remaining",
              message: snapshot
                ? details(snapshot)
                : "No readable cache. Run ai-quota refresh.",
            }),
          );
        },
      },
    ],
  });
  // Local reads only; no refresh process, credentials, endpoint or network in this plugin.
  const timer = setInterval(() => {
    void read();
  }, 5000);
  api.lifecycle.onDispose(() => {
    clearInterval(timer);
    unregister();
  });
};
export default { id: "ai-quota", tui } satisfies TuiPluginModule;
