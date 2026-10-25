import GLib from "gi://GLib";
import Gio from "gi://Gio";
import St from "gi://St";
import Clutter from "gi://Clutter";
import Pango from "gi://Pango";
import { Extension } from "resource:///org/gnome/shell/extensions/extension.js";
import * as Main from "resource:///org/gnome/shell/ui/main.js";
import * as PanelMenu from "resource:///org/gnome/shell/ui/panelMenu.js";
import * as PopupMenu from "resource:///org/gnome/shell/ui/popupMenu.js";
import * as BarLevel from "resource:///org/gnome/shell/ui/barLevel.js";
import { validate } from "./view.mjs";
import { model, MODES, MODE_NAMES } from "./presentation.mjs";

// GJS requires explicit promisification; the original extensions omitted this.
Gio._promisify(
  Gio.File.prototype,
  "load_contents_async",
  "load_contents_finish",
);
export default class AIQuotaExtension extends Extension {
  enable() {
    this._settings = this.getSettings();
    this._snapshot = null;
    this._structure = null;
    this._providerActors = [];
    this._indicator = new PanelMenu.Button(0, this.metadata.name, false);
    this._label = new St.Label({
      text: "AI quota …",
      y_align: Clutter.ActorAlign.CENTER,
      style_class: "ai-quota-label",
    });
    this._label.clutter_text.ellipsize = Pango.EllipsizeMode.END;
    this._indicator.add_child(this._label);
    Main.panel.addToStatusArea(this.uuid, this._indicator);
    this._buildMenu();
    this._settingsChanged = this._settings.connect(
      "changed::display-mode",
      () => this._render(),
    );
    this._indicator.menu.connect("open-state-changed", (_menu, open) => {
      if (open) {
        this._render();
        this._read();
      }
    });
    this._render();
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
    this._snapshot = snapshot;
    this._render();
  }

  _buildMenu() {
    const menu = this._indicator.menu;
    menu.actor.add_style_class_name("ai-quota-menu");
    this._providersSection = new PopupMenu.PopupMenuSection();
    menu.addMenuItem(this._providersSection);
    this._advanced = new PopupMenu.PopupSubMenuMenuItem(
      "Detalles avanzados",
      false,
    );
    menu.addMenuItem(this._advanced);
    this._advanced.menu.connect("open-state-changed", (_menu, open) => {
      if (open) this._render();
    });
    this._modeMenu = new PopupMenu.PopupSubMenuMenuItem("Modo visual", false);
    this._modeItems = new Map();
    for (const mode of MODES) {
      const item = new PopupMenu.PopupMenuItem(MODE_NAMES[mode]);
      item.connect("activate", () =>
        this._settings.set_string("display-mode", mode),
      );
      this._modeMenu.menu.addMenuItem(item);
      this._modeItems.set(mode, item);
    }
    menu.addMenuItem(this._modeMenu);
    menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
    for (const [title, uri] of [
      ["ChatGPT Usage", "https://chatgpt.com/settings/usage"],
      ["OpenCode Console", "https://opencode.ai/console"],
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

  _textItem(menu, text, advanced = false) {
    const item = new PopupMenu.PopupMenuItem(text, {
      reactive: false,
      can_focus: false,
    });
    item.add_style_class_name(
      advanced ? "ai-quota-detail" : "ai-quota-heading",
    );
    item.label.clutter_text.ellipsize = advanced
      ? Pango.EllipsizeMode.NONE
      : Pango.EllipsizeMode.END;
    if (advanced) {
      item.label.clutter_text.line_wrap = true;
      item.label.clutter_text.line_wrap_mode = Pango.WrapMode.WORD_CHAR;
    }
    menu.addMenuItem(item);
    return item;
  }

  _rebuildProviders(providers) {
    // Rebuild only on topology changes, never for percentages or countdowns.
    this._providersSection.removeAll();
    this._advanced.menu.removeAll();
    this._providerActors = providers.map((p) => {
      const heading = this._textItem(this._providersSection, p.name);
      const status = new St.Label({
        style_class: "ai-quota-status",
        x_expand: true,
        x_align: Clutter.ActorAlign.END,
        y_align: Clutter.ActorAlign.CENTER,
      });
      heading.add_child(status);
      const windows = p.windows.map((w) => {
        const row = new PopupMenu.PopupBaseMenuItem({
          reactive: false,
          can_focus: false,
        });
        row.add_style_class_name("ai-quota-row");
        const title = new St.Label({
          text: w.title,
          x_expand: true,
          y_align: Clutter.ActorAlign.CENTER,
        });
        title.add_style_class_name("ai-quota-window-title");
        title.clutter_text.ellipsize = Pango.EllipsizeMode.END;
        const bar = new BarLevel.BarLevel({
          style_class: "barlevel ai-quota-bar",
          reactive: false,
          can_focus: false,
          y_align: Clutter.ActorAlign.CENTER,
        });
        const percent = new St.Label({
          style_class: "ai-quota-percent",
          y_align: Clutter.ActorAlign.CENTER,
        });
        row.add_child(title);
        row.add_child(bar);
        row.add_child(percent);
        this._providersSection.addMenuItem(row);
        return { row, bar, percent };
      });
      this._textItem(this._advanced.menu, p.name);
      const advanced = p.advanced.map((text) =>
        this._textItem(this._advanced.menu, text, true),
      );
      return { heading, status, windows, advanced };
    });
    if (!providers.length) {
      this._textItem(
        this._providersSection,
        "Sin lecturas de la caché central",
      );
      this._textItem(
        this._advanced.menu,
        "Caché ausente, ilegible, inválida o sin proveedores.",
        true,
      );
    }
  }

  _setText(label, text) {
    if (label.text !== text) label.set_text(text);
  }

  _style(actor, name, enabled) {
    if (enabled && !actor.has_style_class_name(name))
      actor.add_style_class_name(name);
    if (!enabled && actor.has_style_class_name(name))
      actor.remove_style_class_name(name);
  }

  _render() {
    if (!this._indicator) return;
    const mode = this._settings.get_string("display-mode");
    const view = model(this._snapshot, mode);
    this._setText(this._label, view.text);
    this._indicator.accessible_name = view.accessible;
    this._style(this._label, "ai-quota-warning", view.warning);
    this._style(
      this._label,
      "ai-quota-attention",
      !view.warning && view.attention,
    );
    const structure = JSON.stringify(
      view.providers.map((p) => [
        p.key,
        p.windows.map((w) => [w.short, w.title]),
        p.advanced.length,
      ]),
    );
    if (structure !== this._structure) {
      this._rebuildProviders(view.providers);
      this._structure = structure;
    }
    view.providers.forEach((p, i) => {
      const actors = this._providerActors[i];
      this._setText(actors.heading.label, p.name);
      this._setText(actors.status, p.status);
      p.windows.forEach((w, j) => {
        const { row, bar, percent } = actors.windows[j];
        this._setText(percent, `${w.warning ? "⚠ " : ""}${w.text}`);
        this._style(row, "ai-quota-warning", w.warning);
        bar.visible = w.value !== null;
        bar.opacity = p.marker ? 120 : 255;
        if (w.value !== null && bar.value !== w.value / 100)
          bar.value = w.value / 100;
        row.accessible_name = `${p.name}, ${w.title}: ${w.text}, ${p.status}`;
      });
      actors.advanced.at(-1).visible = p.hasError;
      if (this._advanced.menu.isOpen)
        p.advanced.forEach((text, j) =>
          this._setText(actors.advanced[j].label, text),
        );
    });
    this._setText(
      this._modeMenu.label,
      `Modo visual: ${MODE_NAMES[mode] || MODE_NAMES.compact}`,
    );
    for (const [key, item] of this._modeItems)
      item.setOrnament(
        key === mode ? PopupMenu.Ornament.DOT : PopupMenu.Ornament.NONE,
      );
  }

  disable() {
    this._generation++;
    if (this._timer) GLib.Source.remove(this._timer);
    this._timer = null;
    if (this._settingsChanged) this._settings.disconnect(this._settingsChanged);
    this._settingsChanged = null;
    this._settings = null;
    this._indicator?.destroy();
    this._indicator = null;
    this._label = null;
    this._snapshot = null;
    this._providerActors = [];
    this._providersSection = null;
    this._advanced = null;
    this._modeMenu = null;
    this._modeItems = null;
    this._structure = null;
  }
}
