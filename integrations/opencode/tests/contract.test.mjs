import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import Ajv from "ajv";
const root = new URL("../../../", import.meta.url);
const schema = JSON.parse(
  fs.readFileSync(new URL("schema/status-v1.schema.json", root), "utf8"),
);
const validate = new Ajv({ strict: true }).compile(schema);
test("Rust CLI JSON matches independently published schema", () => {
  const temp = fs.mkdtempSync(
    path.join(new URL("target/", root).pathname, "contract-"),
  );
  try {
    const json = JSON.parse(
      execFileSync(
        new URL("target/debug/ai-quota", root).pathname,
        [
          "status",
          "--json",
          "--config",
          `${temp}/config.json`,
          "--cache",
          `${temp}/status.json`,
        ],
        { encoding: "utf8" },
      ),
    );
    assert.equal(validate(json), true, JSON.stringify(validate.errors));
    const invalid = structuredClone(json);
    delete invalid.providers[0].last_success_at;
    assert.equal(validate(invalid), false);
  } finally {
    fs.rmSync(temp, { recursive: true, force: true });
  }
});
