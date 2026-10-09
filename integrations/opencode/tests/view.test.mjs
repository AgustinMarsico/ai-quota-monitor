import test from "node:test";
import assert from "node:assert/strict";
import { compact, details, validate } from "../../shared/view.mjs";
const fixture = () => ({
  schema_version: 1,
  generated_at: 1000,
  stale_seconds: 600,
  providers: [
    {
      provider: "codex",
      profile: "default",
      source: "fixture",
      state: "fresh",
      error: null,
      last_success_at: 1000,
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
});
test("remaining, not used, and absent reset", () => {
  const s = validate(fixture());
  assert.equal(compact(s, 1000), "Codex 5h: 78%");
  assert.match(details(s), /not reported/);
});
test("offline age and expired windows never restore quota", () => {
  const s = fixture();
  assert.match(compact(s, 1700), /78% ~/);
  s.providers[0].quotas[0].resets_at = 1000;
  assert.match(compact(s, 1001), /78% ~/);
});
test("contract rejects zero substitution and future versions", () => {
  const s = fixture();
  s.schema_version = 2;
  assert.throws(() => validate(s));
  s.schema_version = 1;
  delete s.providers[0].quotas[0].remaining_percent;
  assert.throws(() => validate(s));
});
test("multi-provider, monthly and unavailable", () => {
  const s = fixture();
  s.providers.push({
    ...s.providers[0],
    provider: "opencode-go",
    quotas: [
      {
        ...s.providers[0].quotas[0],
        window: "monthly",
        label: "month",
        duration_minutes: null,
      },
    ],
  });
  assert.match(compact(s, 1000), /Go month: 78%/);
  s.providers[1].quotas = [];
  assert.match(compact(s, 1000), /Go: \?/);
});
