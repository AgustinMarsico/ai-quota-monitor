import fs from "node:fs";
import vm from "node:vm";
import assert from "node:assert/strict";
import { validate } from "../integrations/shared/view.mjs";
import {
  model,
  minimum,
  remaining,
  resetText,
  MODES,
  MODE_NAMES,
} from "../integrations/gnome/ai-quota@local/presentation.mjs";

const now = 1800000000;
const quota = (label, duration, value, resets = null) => ({
  window: label,
  label,
  duration_minutes: duration,
  remaining_percent: value,
  used_percent: 100 - value,
  resets_at: resets,
});
const provider = (name, quotas) => ({
  provider: name,
  profile: "default",
  state: "fresh",
  source: "fixture",
  error: null,
  last_success_at: now,
  last_attempt_at: now,
  quotas,
});
const fixture = () => ({
  schema_version: 1,
  generated_at: now,
  stale_seconds: 600,
  providers: [
    provider("codex", [
      quota("5h", 300, 90, now + 3600),
      quota("7d", 10080, 77),
    ]),
    provider("opencode-go", [
      quota("5h", 300, 95),
      quota("7d", 10080, 85),
      quota("month", 43200, 99),
    ]),
  ],
});
const sample = fixture();
assert.equal(model(sample, "compact", now).text, "Codex 7d 77% · Go 7d 85%");
assert.equal(model(sample, "minimal", now).text, "C 77% · G 85%");
assert.equal(
  model(sample, "full", now).text,
  "Codex 5h 90% / 7d 77% · Go 5h 95% / 7d 85% / m 99%",
);
assert.equal(
  model(sample, "invalid", now).text,
  model(sample, "compact", now).text,
);
sample.providers[0].quotas[0] = quota("5h", 300, 0);
assert.equal(model(sample, "compact", now).text, "Codex 5h 0% · Go 7d 85%");
assert.ok(model(sample, "compact", now).warning);
assert.equal(remaining({ remaining_percent: 0 }), 0);
for (const value of [null, undefined, NaN, Infinity, -1, 101, "0"])
  assert.equal(remaining({ remaining_percent: value }), null);
assert.equal(
  minimum([
    { remaining_percent: null },
    quota("7d", 10080, 77),
    quota("5h", 300, 0),
  ]).label,
  "5h",
);
assert.equal(minimum([]), null);
assert.equal(minimum([{ remaining_percent: null }]), null);
const monthly = fixture();
monthly.providers[1].quotas[2] = quota("month", 43200, 0);
assert.equal(model(monthly, "compact", now).text, "Codex 7d 77% · Go m 0%");
assert.equal(
  minimum([quota("5h", 300, 77.4), quota("7d", 10080, 77.1)]).label,
  "7d",
  "Compare before rounding",
);
assert.equal(
  minimum([quota("5h", 300, 77), quota("7d", 10080, 77)]).label,
  "5h",
  "Ties keep cache order",
);
assert.match(model(fixture(), "compact", now + 601).text, /77% ~.*85% ~/);
const absent = fixture();
absent.providers[0].quotas = [];
absent.providers[1].quotas.pop();
assert.equal(model(absent, "compact", now).text, "Codex ? · Go 7d 85%");
assert.match(model(absent, "full", now).text, /Codex 5h \? \/ 7d \?.*m \?/);
assert.equal(model(absent, "minimal", now).text, "C ? · G 85%");
assert.equal(
  model(absent, "compact", now).providers[1].windows[2].text,
  "No disponible",
);
assert.equal(model(null).text, "AI quota ?");
assert.equal(model({ ...sample, providers: [] }).text, "AI quota ?");
assert.match(resetText(null, now), /No informado/);
assert.match(resetText(now + 3600, now), /en 0d 1h 0m/);
assert.match(resetText(now - 1, now), /vencido/);
assert.throws(() => validate({ ...fixture(), schema_version: 2 }));
const invalid = fixture();
invalid.providers[0].quotas[0].remaining_percent = null;
assert.throws(() => validate(invalid), /quota/);
for (const mutate of [
  (s) => {
    s.providers[0].last_success_at = now - 601;
  },
  (s) => {
    s.providers[0].last_success_at = null;
  },
  (s) => {
    s.providers[0].last_success_at = now + 61;
  },
  (s) => {
    s.providers[0].state = "stale";
  },
  (s) => {
    s.providers[0].quotas[0].resets_at = now;
  },
  (s) => {
    s.generated_at = now - 601;
  },
]) {
  const s = fixture();
  mutate(s);
  const view = model(s, "compact", now);
  assert.match(view.text, /Codex 7d 77% ~/);
  assert.equal(view.providers[0].status, "Datos obsoletos");
}
const failed = fixture();
failed.providers[0].error = { message: "Network failed" };
failed.providers[0].state = "error";
assert.match(model(failed, "minimal", now).text, /C 77% !/);
assert.equal(
  model(failed, "compact", now).providers[0].status,
  "Consulta fallida",
);
failed.providers[0].quotas = [];
assert.match(model(failed, "compact", now).text, /Codex \? !/);
const threshold = fixture();
threshold.providers[0].quotas[0] = quota("5h", 300, 20);
assert.equal(model(threshold, "compact", now).warning, false);
threshold.providers[0].quotas[0] = quota("5h", 300, 19.9);
assert.equal(model(threshold, "compact", now).warning, true);
const profiles = fixture();
profiles.providers[0].profile = "work";
assert.match(model(profiles, "minimal", now).text, /C \[work\]/);
assert.equal(
  model(fixture(), "compact", now).providers[1].advanced.filter((line) =>
    line.includes("No informado por la fuente"),
  ).length,
  3,
);
console.log(
  "PASS: GNOME presentation: modes, minimum, period changes, zero, missing, freshness, errors, resets, profiles and threshold",
);

const source = fs
  .readFileSync(
    new URL(
      "../integrations/gnome/ai-quota@local/extension.js",
      import.meta.url,
    ),
    "utf8",
  )
  .replace(/^import .*;$/gm, "")
  .replace("export default class AIQuotaExtension", "class AIQuotaExtension");

class Actor {
  constructor(params = {}) {
    Object.assign(this, { text: "", visible: true }, params);
    this.children = [];
    this.classes = new Set();
    this.clutter_text = {};
    this.signals = new Map();
    this.writes = 0;
  }
  add_child(actor) {
    this.children.push(actor);
  }
  set_text(text) {
    this.text = text;
    this.writes++;
  }
  connect(name, fn) {
    this.signals.set(name, fn);
    return name;
  }
  emit(name, ...args) {
    this.signals.get(name)?.(this, ...args);
  }
  add_style_class_name(name) {
    this.classes.add(name);
  }
  remove_style_class_name(name) {
    this.classes.delete(name);
  }
  has_style_class_name(name) {
    return this.classes.has(name);
  }
  destroy() {
    this.destroyed = true;
  }
}
class Item extends Actor {
  constructor(text, params) {
    super(params);
    this.label = new Actor({ text });
    this.add_child(this.label);
  }
  setOrnament(value) {
    this.ornament = value;
  }
}
class Menu extends Actor {
  constructor() {
    super();
    this.items = [];
    this.actor = new Actor();
    this.isOpen = false;
    this.rebuilds = 0;
  }
  removeAll() {
    this.items = [];
    this.rebuilds++;
  }
  addMenuItem(item) {
    this.items.push(item);
  }
  open() {
    this.isOpen = true;
    this.emit("open-state-changed", true);
  }
}
class SubMenu extends Item {
  constructor(text) {
    super(text);
    this.menu = new Menu();
  }
}
const settings = new Actor();
settings.mode = "compact";
settings.get_string = () => settings.mode;
settings.set_string = (_key, value) => {
  settings.mode = value;
  settings.emit("changed::display-mode");
};
settings.disconnect = (name) => settings.signals.delete(name);
let indicator,
  prom = false,
  removed = false,
  cache = fixture(),
  readFailure = false;
let pendingRead = null;
const launched = [];
const context = {
  model,
  MODES,
  MODE_NAMES,
  validate,
  TextDecoder,
  Date: class extends Date {
    static now() {
      return now * 1000;
    }
  },
  // Inject the deterministic clock into the actual presentation model.
  GLib: {
    getenv: () => null,
    get_home_dir: () => "/fixture",
    build_filenamev: (p) => p.join("/"),
    timeout_add_seconds: () => 1,
    Source: {
      remove: () => {
        removed = true;
      },
    },
    SOURCE_CONTINUE: 1,
  },
  Gio: {
    _promisify: () => {
      prom = true;
    },
    File: class {
      static new_for_path(path) {
        assert.equal(path, "/fixture/.cache/ai-quota/status.json");
        return {
          load_contents_async: async () => {
            if (pendingRead) await pendingRead;
            if (readFailure) throw new Error("Unreadable");
            return [new TextEncoder().encode(JSON.stringify(cache))];
          },
        };
      }
    },
    AppInfo: { launch_default_for_uri: (uri) => launched.push(uri) },
  },
  Clutter: { ActorAlign: { CENTER: 1, END: 2 } },
  Pango: { EllipsizeMode: { NONE: 0, END: 3 }, WrapMode: { WORD_CHAR: 2 } },
  St: { Label: Actor },
  BarLevel: { BarLevel: Actor },
  PanelMenu: {
    Button: class extends Actor {
      constructor() {
        super();
        this.menu = new Menu();
      }
    },
  },
  PopupMenu: {
    PopupMenuItem: Item,
    PopupBaseMenuItem: Actor,
    PopupMenuSection: Menu,
    PopupSubMenuMenuItem: SubMenu,
    PopupSeparatorMenuItem: Actor,
    Ornament: { DOT: 1, NONE: 0 },
  },
  Main: {
    panel: {
      addToStatusArea: (_uuid, i) => {
        indicator = i;
      },
    },
  },
  Extension: class {
    constructor() {
      this.metadata = { name: "AI Quota" };
      this.uuid = "ai-quota@local";
    }
    getSettings() {
      return settings;
    }
  },
};
context.model = (s, mode) => model(s, mode, now);
vm.runInNewContext(
  `${source}\nthis.AIQuotaExtension=AIQuotaExtension`,
  context,
);
assert.ok(prom, "GJS read is explicitly promisified");
const extension = new context.AIQuotaExtension();
extension.enable();
await new Promise((resolve) => setImmediate(resolve));
assert.equal(indicator.children[0].text, "Codex 7d 77% · Go 7d 85%");
assert.equal(
  indicator.menu.items.length,
  6,
  "Main menu is sections, two submenus, separator and links",
);
assert.equal(
  extension._providersSection.items.length,
  7,
  "Two headings and five quota rows",
);
assert.equal(extension._advanced.label.text, "Detalles avanzados");
assert.equal(extension._advanced.menu.isOpen, false);
assert.equal(
  extension._advanced.menu.items.length,
  17,
  "Reset, metadata and reserved error rows for every provider/window",
);
assert.equal(
  extension._advanced.menu.items.filter((i) => i.visible).length,
  15,
);
assert.ok(
  extension._advanced.menu.items.some((i) =>
    i.label.text.includes("No informado por la fuente"),
  ),
);
assert.ok(extension._providerActors[0].windows[0].bar.visible);
assert.equal(extension._providerActors[0].windows[0].bar.value, 0.9);
assert.ok(
  !extension._providersSection.items.some((i) =>
    i.label?.text.includes("Restablecimiento"),
  ),
);
const originalRow = extension._providerActors[0].windows[0].row;
const rebuilds = extension._providersSection.rebuilds;
const writes = indicator.children[0].writes;
await extension._read();
assert.equal(
  extension._providersSection.rebuilds,
  rebuilds,
  "Identical cache does not rebuild menu",
);
assert.equal(
  indicator.children[0].writes,
  writes,
  "Identical text is not rewritten",
);
extension._advanced.menu.open();
cache.providers[0].quotas[0] = quota("5h", 300, 0, now + 3600);
await extension._read();
assert.equal(indicator.children[0].text, "Codex 5h 0% · Go 7d 85%");
assert.equal(
  extension._providerActors[0].windows[0].row,
  originalRow,
  "Quota changes keep actors",
);
assert.equal(
  extension._advanced.menu.isOpen,
  true,
  "An open submenu stays open",
);
assert.equal(extension._providerActors[0].windows[0].bar.value, 0);
assert.match(extension._providerActors[0].windows[0].percent.text, /⚠ 0%/);
assert.ok(indicator.children[0].classes.has("ai-quota-warning"));
extension._modeItems.get("minimal").emit("activate");
assert.equal(settings.mode, "minimal");
assert.equal(indicator.children[0].text, "C 0% · G 85%");
assert.equal(extension._modeItems.get("minimal").ornament, 1);
extension._modeItems.get("full").emit("activate");
assert.match(indicator.children[0].text, /Codex 5h 0% \/ 7d 77%/);
cache.providers[0].last_success_at = now - 1000;
await extension._read();
assert.match(indicator.children[0].text, /~/);
assert.equal(extension._providerActors[0].status.text, "Datos obsoletos");
assert.ok(
  extension._providerActors[0].advanced.some((i) =>
    i.label.text.includes("Datos obsoletos"),
  ),
);
assert.equal(extension._providerActors[0].windows[0].bar.opacity, 120);
cache.providers[0].error = { message: "Network failed" };
await extension._read();
assert.equal(
  extension._providerActors[0].windows[0].row,
  originalRow,
  "Query failures keep actors",
);
assert.equal(extension._providerActors[0].advanced.at(-1).visible, true);
assert.match(
  extension._providerActors[0].advanced.at(-1).label.text,
  /Network failed/,
);
cache.providers[1].quotas.pop();
await extension._read();
assert.equal(
  extension._providerActors[1].windows[2].percent.text,
  "No disponible",
);
assert.equal(extension._providerActors[1].windows[2].bar.visible, false);
for (const item of indicator.menu.items.slice(-2)) item.emit("activate");
assert.deepEqual(launched, [
  "https://chatgpt.com/settings/usage",
  "https://opencode.ai/console",
]);
readFailure = true;
await extension._read();
assert.equal(indicator.children[0].text, "AI quota ?");
assert.equal(extension._providersSection.items.length, 1);
readFailure = false;
cache.schema_version = 99;
await extension._read();
assert.equal(indicator.children[0].text, "AI quota ?");
cache = fixture();
await extension._read();
let finishRead;
pendingRead = new Promise((resolve) => {
  finishRead = resolve;
});
const inFlight = extension._read();
const oldIndicator = indicator;
extension.disable();
assert.ok(removed);
assert.ok(oldIndicator.destroyed);
assert.equal(settings.signals.size, 0);
extension.enable();
assert.equal(settings.mode, "full", "Mode survives extension disable/enable");
finishRead();
await inFlight;
await new Promise((resolve) => setImmediate(resolve));
assert.match(indicator.children[0].text, /Codex 5h 90%/);
extension.disable();
console.log(
  "PASS: GNOME lifecycle: compact native menu, advanced submenu, bars, warnings, mode changes, stable actors, cache failures, disable during read",
);
