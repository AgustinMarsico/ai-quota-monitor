import Gio from "gi://Gio";
import { compact, validate } from "../integrations/shared/view.mjs";
Gio._promisify(
  Gio.File.prototype,
  "load_contents_async",
  "load_contents_finish",
);
const [bytes] = await Gio.File.new_for_path(ARGV[0]).load_contents_async(null);
const snapshot = validate(JSON.parse(new TextDecoder().decode(bytes)));
const text = compact(snapshot);
if (!text.includes("Codex 5h: 78%") || !text.includes("Go 5h: 91%"))
  throw new Error("Native GJS cache rendering failed");
print(`PASS: native GJS/Gio async cache read: ${text}`);
