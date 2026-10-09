/** @typedef {{window:string,label:string,duration_minutes:number|null,used_percent:number,remaining_percent:number,resets_at:number|null}} Quota */
/** @typedef {{provider:string,profile:string,quotas:Quota[],last_success_at:number|null,error:{message:string}|null,source:string,state:string}} Provider */
/** @typedef {{schema_version:number,generated_at:number,stale_seconds:number,providers:Provider[]}} Snapshot */
/** Validate display-critical contract fields, without accepting missing values as zero.
 * @param {unknown} input
 * @returns {Snapshot}
 */
export function validate(input) {
  const s = /** @type {Snapshot} */ (input);
  if (
    !s ||
    s.schema_version !== 1 ||
    !Number.isFinite(s.generated_at) ||
    !Number.isFinite(s.stale_seconds) ||
    s.stale_seconds <= 0 ||
    !Array.isArray(s.providers)
  )
    throw new Error("Invalid cache contract");
  for (const p of s.providers) {
    if (
      typeof p.provider !== "string" ||
      typeof p.profile !== "string" ||
      !Array.isArray(p.quotas) ||
      (p.last_success_at !== null && !Number.isFinite(p.last_success_at)) ||
      !["fresh", "stale", "unavailable", "error"].includes(p.state)
    )
      throw new Error("Invalid provider contract");
    for (const q of p.quotas) {
      if (
        typeof q.window !== "string" ||
        typeof q.label !== "string" ||
        !Number.isFinite(q.used_percent) ||
        !Number.isFinite(q.remaining_percent) ||
        q.used_percent < 0 ||
        q.used_percent > 100 ||
        q.remaining_percent < 0 ||
        q.remaining_percent > 100 ||
        Math.abs(q.used_percent + q.remaining_percent - 100) > 0.000001 ||
        (q.resets_at !== null &&
          (!Number.isFinite(q.resets_at) || q.resets_at <= 0))
      )
        throw new Error("Invalid quota contract");
    }
  }
  return s;
}
/** @param {Provider} p @param {Snapshot} s @param {number} now */
export function stale(p, s, now) {
  return (
    !!p.error ||
    p.last_success_at === null ||
    now - p.last_success_at > s.stale_seconds ||
    p.last_success_at > now + 60 ||
    p.quotas.some((q) => q.resets_at !== null && q.resets_at <= now)
  );
}
/** @param {string} text */
export function safeText(text) {
  return text.replace(/[\x00-\x1f\x7f]/g, "").slice(0, 512);
}
/** @param {Snapshot} s @param {number} [now] */
export function compact(s, now = Date.now() / 1000) {
  return (
    s.providers
      .flatMap((p) => {
        const name =
          p.provider === "codex"
            ? "Codex"
            : p.provider === "opencode-go"
              ? "Go"
              : safeText(p.provider);
        const profile =
          p.profile === "default" ? "" : ` [${safeText(p.profile)}]`;
        if (!p.quotas.length) return [`${name}${profile}: ?`];
        return p.quotas.map(
          (q) =>
            `${name}${profile} ${safeText(q.label)}: ${Math.round(q.remaining_percent)}%${stale(p, s, now) ? " ~" : ""}`,
        );
      })
      .join(" | ") || "AI quota: ?"
  );
}
/** @param {Snapshot} s */
export function details(s) {
  return (
    s.providers
      .flatMap((p) =>
        p.quotas.map(
          (q) =>
            `${safeText(p.provider)} [${safeText(p.profile)}] ${safeText(q.label)}: ${q.remaining_percent.toFixed(1)}% remaining\nReset: ${q.resets_at === null ? "not reported" : new Date(q.resets_at * 1000).toLocaleString()}\nLast success: ${p.last_success_at === null ? "never" : new Date(p.last_success_at * 1000).toLocaleString()}\nSource: ${safeText(p.source)}${p.error ? `\nError: ${safeText(p.error.message)}` : ""}`,
        ),
      )
      .join("\n\n") || "No quota readings yet"
  );
}
