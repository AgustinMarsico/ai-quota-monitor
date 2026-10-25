// GNOME-only presentation. The central cache remains the sole quota source.
function safeText(text) {
  return text.replace(/[\x00-\x1f\x7f]/g, "").slice(0, 512);
}

export const MODES = ["compact", "minimal", "full"];
export const MODE_NAMES = {
  compact: "Compacto",
  minimal: "Mínimo",
  full: "Completo",
};

const WINDOWS = {
  codex: [
    ["5h", "5 horas", 300],
    ["7d", "7 días", 10080],
  ],
  "opencode-go": [
    ["5h", "5 horas", 300],
    ["7d", "7 días", 10080],
    ["m", "Mensual", 43200],
  ],
};

function abbreviation(q) {
  if (q.duration_minutes === 300) return "5h";
  if (q.duration_minutes === 10080) return "7d";
  if (
    q.duration_minutes === 43200 ||
    /^(month|monthly|mensual|m)$/i.test(q.label)
  )
    return "m";
  return safeText(q.label).slice(0, 16);
}

export function remaining(q) {
  const value = q?.remaining_percent;
  return typeof value === "number" &&
    Number.isFinite(value) &&
    value >= 0 &&
    value <= 100
    ? value
    : null;
}

export function minimum(quotas) {
  return quotas.reduce((best, q) => {
    const value = remaining(q);
    return value !== null && (best === null || value < remaining(best))
      ? q
      : best;
  }, null);
}

function dateText(timestamp) {
  return timestamp == null
    ? "No informada"
    : new Date(timestamp * 1000).toLocaleString();
}

export function resetText(timestamp, now) {
  if (timestamp == null) return "No informado por la fuente";
  const minutes = Math.ceil(Math.max(0, timestamp - now) / 60);
  const countdown =
    minutes === 0
      ? "vencido; requiere nueva consulta"
      : `en ${Math.floor(minutes / 1440)}d ${Math.floor((minutes % 1440) / 60)}h ${minutes % 60}m`;
  return `${dateText(timestamp)} · ${countdown}`;
}

export function providerView(p, snapshot, now) {
  const outdated =
    p.state === "stale" ||
    p.last_success_at === null ||
    p.last_attempt_at === null ||
    now - p.last_success_at > snapshot.stale_seconds ||
    p.last_success_at > now + 60 ||
    p.quotas.some((q) => q.resets_at != null && q.resets_at <= now) ||
    now - snapshot.generated_at > snapshot.stale_seconds ||
    snapshot.generated_at > now + 60;
  const failed = !!p.error || p.state === "error";
  const unavailable =
    p.state === "unavailable" || !p.quotas.some((q) => remaining(q) !== null);
  const status = failed
    ? "Consulta fallida"
    : unavailable
      ? "Sin datos"
      : outdated
        ? "Datos obsoletos"
        : "Actualizado";
  const marker = failed ? "!" : unavailable ? "?" : outdated ? "~" : "";
  const name =
    p.provider === "codex"
      ? "Codex"
      : p.provider === "opencode-go"
        ? "OpenCode Go"
        : safeText(p.provider);
  const profile = p.profile === "default" ? "" : ` [${safeText(p.profile)}]`;
  const rows = (WINDOWS[p.provider] || []).map(([short, title, duration]) => {
    const quota = p.quotas.find(
      (q) => q.duration_minutes === duration || abbreviation(q) === short,
    );
    return { short, title, quota: quota || null };
  });
  // Preserve any additional windows reported by the cache.
  for (const quota of p.quotas) {
    if (!rows.some((row) => row.quota === quota))
      rows.push({
        short: abbreviation(quota),
        title: safeText(quota.label),
        quota,
      });
  }
  const windows = rows.map((row) => {
    const value = remaining(row.quota);
    return {
      ...row,
      value,
      warning: value !== null && value < 20,
      text:
        value === null
          ? "No disponible"
          : `${Number(value.toFixed(1))}%${marker ? ` ${marker}` : ""}`,
    };
  });
  const advanced = [
    `Proveedor: ${safeText(p.provider)} · Perfil: ${safeText(p.profile)}`,
    `Fuente: ${safeText(p.source)}`,
    `Última consulta exitosa: ${dateText(p.last_success_at)}`,
    `Frescura: ${status}${failed && outdated ? " · Datos obsoletos" : ""}`,
    ...windows.map(
      (w) =>
        `${w.title} — Restablecimiento: ${resetText(w.quota?.resets_at, now)}`,
    ),
    `Error: ${p.error ? safeText(p.error.message) : "Ninguno"}`,
  ];
  return {
    key: JSON.stringify([p.provider, p.profile]),
    name: name + profile,
    shortName: (p.provider === "opencode-go" ? "Go" : name) + profile,
    initial:
      (p.provider === "codex"
        ? "C"
        : p.provider === "opencode-go"
          ? "G"
          : name.slice(0, 1)) + profile,
    status,
    marker,
    windows,
    advanced,
    hasError: !!p.error,
    selected: minimum(p.quotas),
  };
}

export function panelText(providers, mode = "compact") {
  if (!MODES.includes(mode)) mode = "compact";
  return (
    providers
      .map((p) => {
        const suffix =
          p.marker && (p.marker !== "?" || p.selected) ? ` ${p.marker}` : "";
        if (mode === "full")
          return `${p.shortName} ${p.windows.map((w) => `${w.short} ${w.value === null ? "?" : `${Math.round(w.value)}%`}`).join(" / ") || "?"}${suffix}`;
        const value = remaining(p.selected);
        const percent = value === null ? "?" : `${Math.round(value)}%`;
        if (mode === "minimal") return `${p.initial} ${percent}${suffix}`;
        return `${p.shortName}${p.selected ? ` ${abbreviation(p.selected)}` : ""} ${percent}${suffix}`;
      })
      .join(" · ") || "AI quota ?"
  );
}

export function model(snapshot, mode = "compact", now = Date.now() / 1000) {
  const providers = snapshot
    ? snapshot.providers.map((p) => providerView(p, snapshot, now))
    : [];
  return {
    providers,
    text: panelText(providers, mode),
    warning: providers.some((p) => p.windows.some((w) => w.warning)),
    attention: !providers.length || providers.some((p) => p.marker),
    accessible:
      providers
        .map(
          (p) =>
            `${p.name}: ${p.status}; ${p.windows.map((w) => `${w.title}: ${w.text}`).join("; ")}`,
        )
        .join(". ") || "Caché central no disponible o inválida",
  };
}
