import fs from "node:fs";
import vm from "node:vm";
import assert from "node:assert/strict";
import { compact, details, validate } from "../integrations/shared/view.mjs";
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
let prom = false,
  removed = false;
const now = Math.floor(Date.now() / 1000);
const sample = {
  schema_version: 1,
  generated_at: now,
  stale_seconds: 600,
  providers: [
    {
      provider: "codex",
      profile: "default",
      state: "fresh",
      source: "fixture",
      error: null,
      last_success_at: now,
      last_attempt_at: now,
      quotas: [
        {
          window: "primary",
          label: "5h",
          duration_minutes: 300,
          used_percent: 22,
          remaining_percent: 78,
          resets_at: null,
        },
      ],
    },
  ],
};
let indicator;
class Item {
  constructor(text) {
    this.text = text;
  }
  connect() {}
}
class Menu {
  constructor() {
    this.items = [];
  }
  removeAll() {
    this.items = [];
  }
  addMenuItem(x) {
    this.items.push(x);
  }
}
const context = {
  compact,
  details,
  validate,
  TextDecoder,
  GLib: {
    getenv: () => null,
    get_home_dir: () => "/fixture",
    build_filenamev: (p) => p.join("/"),
    timeout_add_seconds: () => 1,
    Source: { remove: () => (removed = true) },
    SOURCE_CONTINUE: 1,
  },
  Gio: {
    _promisify: () => (prom = true),
    File: class {
      static new_for_path() {
        return {
          load_contents_async: async () => [
            new TextEncoder().encode(JSON.stringify(sample)),
          ],
        };
      }
    },
    AppInfo: { launch_default_for_uri() {} },
  },
  Clutter: { ActorAlign: { CENTER: 1 } },
  St: {
    Label: class {
      constructor(x) {
        this.text = x.text;
      }
      set_text(t) {
        this.text = t;
      }
    },
  },
  PanelMenu: {
    Button: class {
      constructor() {
        this.menu = new Menu();
      }
      add_child(x) {
        this.child = x;
      }
      destroy() {
        this.destroyed = true;
      }
    },
  },
  PopupMenu: { PopupMenuItem: Item, PopupSeparatorMenuItem: class {} },
  Main: { panel: { addToStatusArea: (_, i) => (indicator = i) } },
  Extension: class {
    constructor() {
      this.metadata = { name: "AI Quota" };
      this.uuid = "ai-quota@local";
    }
  },
  console,
};
vm.runInNewContext(
  `${source}\nthis.AIQuotaExtension=AIQuotaExtension`,
  context,
);
assert.ok(prom, "GJS async read explicitly promisified");
const extension = new context.AIQuotaExtension();
extension.enable();
await new Promise((r) => setImmediate(r));
assert.match(indicator.child.text, /78%/);
assert.ok(indicator.menu.items.some((i) => i.text?.includes("not reported")));
sample.providers[0].last_success_at = now - 1000;
await extension._read();
assert.match(indicator.child.text, /~/);
extension.disable();
assert.ok(removed);
assert.ok(indicator.destroyed);
console.log(
  "PASS: GNOME mocked lifecycle, cached quotas, absent reset and stale data",
);
