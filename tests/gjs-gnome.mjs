// Native GJS + installed GNOME APIs, without touching the running Shell.
// Needs staging schemas (ARGV[0]), memory settings backend, Shell/Mutter library paths.
import Gio from "gi://Gio";
import GLib from "gi://GLib";
import Shell from "gi://Shell";
import GObject from "gi://GObject";
import { model } from "../integrations/gnome/ai-quota@local/presentation.mjs";

function check(value, message) {
  if (!value) throw new Error(message);
}

// Accessing Shell loads its registered resources, including the real BarLevel.
check(Shell.Global, "Shell introspection unavailable");
const { BarLevel } = await import("resource:///org/gnome/shell/ui/barLevel.js");
check(
  GObject.type_is_a(BarLevel.$gtype, GObject.TYPE_OBJECT),
  "Native BarLevel unavailable",
);
const popup = new TextDecoder().decode(
  Gio.resources_lookup_data("/org/gnome/shell/ui/popupMenu.js", 0).toArray(),
);
for (const api of [
  "PopupBaseMenuItem",
  "PopupMenuSection",
  "PopupSubMenuMenuItem",
  "setOrnament",
  "open-state-changed",
])
  check(popup.includes(api), `Installed Shell lacks ${api}`);
check(
  popup.includes("new St.ScrollView"),
  "Native submenu scrolling unavailable",
);
const schemaSource = Gio.SettingsSchemaSource.new_from_directory(
  ARGV[0],
  Gio.SettingsSchemaSource.get_default(),
  false,
);
const schema = schemaSource.lookup(
  "org.gnome.shell.extensions.ai-quota",
  false,
);
check(schema, "Extension schema not installed");
const settings = new Gio.Settings({ settings_schema: schema });
check(settings.get_string("display-mode") === "compact", "Wrong default mode");
for (const mode of ["compact", "minimal", "full"])
  check(
    schema.get_key("display-mode").range_check(new GLib.Variant("s", mode)),
    `Invalid mode ${mode}`,
  );
check(
  !schema.get_key("display-mode").range_check(new GLib.Variant("s", "other")),
  "Schema accepts invalid modes",
);
settings.set_string("display-mode", "minimal");
check(
  new Gio.Settings({ settings_schema: schema }).get_string("display-mode") ===
    "minimal",
  "Mode not retained across settings instances",
);
const now = 1800000000;
const snapshot = {
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
          remaining_percent: 0,
          used_percent: 100,
          resets_at: null,
        },
      ],
    },
  ],
};
check(
  model(snapshot, "compact", now).text === "Codex 5h 0%",
  "Native GJS model lost zero",
);
const [ok, output] = GLib.spawn_command_line_sync("gnome-shell --version");
check(ok, "Cannot inspect GNOME version");
const version = new TextDecoder().decode(output).trim();
const [testPath] = GLib.filename_from_uri(import.meta.url);
const metadataPath = GLib.build_filenamev([
  GLib.path_get_dirname(testPath),
  "../integrations/gnome/ai-quota@local/metadata.json",
]);
const [, metadataBytes] =
  Gio.File.new_for_path(metadataPath).load_contents(null);
const metadata = JSON.parse(new TextDecoder().decode(metadataBytes));
check(
  metadata["shell-version"].includes(version.match(/\d+/)[0]),
  "Installed Shell version not declared",
);
print(
  `PASS: ${version}, native GJS model, real BarLevel class, popup/scroll APIs and GSettings schema (memory backend)`,
);
