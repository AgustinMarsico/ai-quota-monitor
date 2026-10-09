import GLib from "gi://GLib";
import Gio from "gi://Gio";
import St from "gi://St";
import Clutter from "gi://Clutter";
import { Extension } from "resource:///org/gnome/shell/extensions/extension.js";
import * as Main from "resource:///org/gnome/shell/ui/main.js";
import * as PanelMenu from "resource:///org/gnome/shell/ui/panelMenu.js";
import * as PopupMenu from "resource:///org/gnome/shell/ui/popupMenu.js";
import { compact, details, validate } from "./view.mjs";

// GJS requires explicit promisification; the original extensions omitted this.
Gio._promisify(
  Gio.File.prototype,
  "load_contents_async",
  "load_contents_finish",
);
export default class AIQuotaExtension extends Extension {
  enable() {
    this._indicator = new PanelMenu.Button(0, this.metadata.name, false);
    this._label = new St.Label({
      text: "AI quota …",
      y_align: Clutter.ActorAlign.CENTER,
      style_class: "ai-quota-label",
    });
    this._indicator.add_child(this._label);
    Main.panel.addToStatusArea(this.uuid, this._indicator);
    this._busy = false;
    this._generation = (this._generation || 0) + 1;
    this._read();
    this._timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 30, () => {
      this._read();
      return GLib.SOURCE_CONTINUE;
    });
  }
  async _read() {
    if (this._busy) return;
    this._busy = true;
    const generation = this._generation;
    let snapshot;
    try {
      const path = GLib.build_filenamev([
        GLib.getenv("XDG_CACHE_HOME") ||
          GLib.build_filenamev([GLib.get_home_dir(), ".cache"]),
        "ai-quota",
        "status.json",
      ]);
      const [bytes] =
        await Gio.File.new_for_path(path).load_contents_async(null);
      if (bytes.length > 4194304) throw new Error("Cache too large");
      snapshot = validate(JSON.parse(new TextDecoder().decode(bytes)));
    } catch {
      snapshot = null;
    }
    if (generation !== this._generation) return;
    this._busy = false;
    if (!this._indicator) return;
    this._label.set_text(snapshot ? compact(snapshot) : "AI quota ?");
    const menu = this._indicator.menu;
    menu.removeAll();
    for (const line of (snapshot
      ? details(snapshot)
      : "No readings. Run ai-quota refresh."
    ).split("\n")) {
      if (line)
        menu.addMenuItem(
          new PopupMenu.PopupMenuItem(line, { reactive: false }),
        );
    }
    menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
    for (const [title, uri] of [
      ["Open ChatGPT usage", "https://chatgpt.com/settings/usage"],
      ["Open OpenCode console", "https://opencode.ai/console"],
    ]) {
      const item = new PopupMenu.PopupMenuItem(title);
      item.connect("activate", () => {
        try {
          Gio.AppInfo.launch_default_for_uri(uri, null);
        } catch {}
      });
      menu.addMenuItem(item);
    }
  }
  disable() {
    this._generation++;
    if (this._timer) GLib.Source.remove(this._timer);
    this._timer = null;
    this._indicator?.destroy();
    this._indicator = null;
    this._label = null;
  }
}
