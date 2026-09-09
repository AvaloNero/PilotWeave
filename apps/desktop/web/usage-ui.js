(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.PilotWeaveUsage = api;
})(globalThis, () => {
  "use strict";
  const e = (v) => String(v ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#039;");
  const label = (s) => ({ officialGithub: "GitHub official", OfficialGithub: "GitHub official", byok: "BYOK", Byok: "BYOK", successfulEmpty: "Successful · no observations", userConfirmedSameAccount: "User confirmed", exactConnectionIdentity: "Exact connection identity", notCovered: "Not covered", insufficientPermission: "Insufficient permission", sessionCumulative: "Cumulative session/model", requestDelta: "Request", "vs-code-copilot": "VS Code Copilot", "shared-copilot-runtime": "Copilot shared runtime" })[s] ?? String(s ?? "Unknown").replace(/([a-z])([A-Z])/g, "$1 $2").replace(/^./, (s) => s.toUpperCase());
  const value = (v) => v == null ? '<span class="unknown">Unknown</span>' : e(v);
  // Money arrives as an exact decimal string. Never convert it to Number.
  const money = (v) => v == null ? '<span class="unknown">Unavailable</span>' : `$${e(v)}`;
  const percent = (v) => v == null ? "Unknown" : `${(Number(v) * 100).toFixed(1)}%`;
  const time = (v) => v && !Number.isNaN(Date.parse(v)) ? new Date(v).toLocaleString() : "Not available";
  const success = (v) => ["available", "successfulEmpty"].includes(v);
  const badge = (s) => `<span class="setup-badge ${success(s) ? "good" : ""}">${e(label(s))}</span>`;
  const today = () => new Date().toISOString().slice(0, 10);
  const start = new Date(); start.setUTCDate(start.getUTCDate() - 29);
  const filters = { start: start.toISOString().slice(0, 10), end: today(), surface: null, route: null, connectionId: null, model: null, confidence: null, sourceId: null, sourceStatus: null, page: 0, pageSize: 50 };
  let tab = "official", billingMonth = today().slice(0, 7), modelPage = 0, priceSearch = "";
  let context = null;
  const busy = new Set();
  const progress = new Map();
  const options = (items, selected, all = "All") => `<option value="">${e(all)}</option>${items.map(([id, name]) => `<option value="${e(id)}" ${id === selected ? "selected" : ""}>${e(name)}</option>`).join("")}`;
  const button = (action, text, disabled = false, extra = "") => `<button class="button ghost small" data-usage-action="${action}" ${disabled ? "disabled" : ""} ${extra}>${e(text)}</button>`;
  const sectionError = (data, key) => data.errors?.[key] ? `<p class="inline-error" role="alert">${e(data.errors[key])}</p>` : "";
  const empty = (text) => `<p class="usage-empty">${e(text)}</p>`;
  function modelUnion(snapshot, data) {
    const rows = new Map();
    const get = (raw, route, confidence = null) => {
      if (confidence === null) {
        const existing = [...rows.values()].find((r) => r.rawModel === raw && r.route === route);
        if (existing) return existing;
      }
      const key = `${raw}\0${route}\0${confidence ?? "catalog"}`;
      if (!rows.has(key)) rows.set(key, { rawModel: raw, canonicalModel: null, route, confidence: "unknown", surfaces: [], origins: [], totals: null, quality: [], official: [], availability: [] });
      return rows.get(key);
    };
    for (const m of data.localUsage?.models ?? []) {
      const row = get(m.rawModel, m.route, m.confidence);
      Object.assign(row, m); row.origins.push("Local observations");
    }
    const runtime = data.quota?.latest?.models?.length ? data.quota.latest : data.quota?.lastSuccessful;
    for (const m of runtime?.models ?? []) {
      const row = get(m.id, "officialGithub"); row.origins.push("Runtime catalog"); row.availability.push(m.policy ?? "Policy not reported");
    }
    for (const c of snapshot.connections ?? []) for (const m of c.models) {
      const row = get(m.modelId, "byok"); row.origins.push(`Configured: ${c.name}`); row.availability.push(m.enabled ? "Configured enabled" : "Configured disabled");
    }
    for (const family of data.billing?.families ?? []) {
      const source = success(family.latest?.status) ? family.latest : family.lastSuccessful;
      for (const item of source?.items ?? []) {
        if (!item.model && !item.sku) continue;
        const row = get(item.model ?? `SKU: ${item.sku}`, "officialGithub");
        row.origins.push("Personal Billing"); row.official.push({ family: family.endpointFamily, ...item, stale: !success(family.latest?.status), period: source.periodStart });
      }
    }
    return [...rows.values()].map((r) => ({ ...r, origins: [...new Set(r.origins)], availability: [...new Set(r.availability)] })).sort((a, b) => a.rawModel.localeCompare(b.rawModel) || a.route.localeCompare(b.route));
  }
  function coverage(t) {
    if (!t) return "No local observations available";
    return `${t.pricedRecords}/${t.records} records priced · ${t.unresolvedModels} unresolved models · ${t.unknownSemantics} records with unknown input semantics`;
  }
  function metrics(t) {
    const cards = [["Reported input", value(t?.inputReported)], ["Normalized input", value(t?.normalizedInput)], ["Fresh input", value(t?.freshInput)], ["Cache read", value(t?.cacheRead)], ["Cache write", value(t?.cacheWrite)], ["Output", value(t?.output)], ["Cache hit rate", e(percent(t?.cacheHitRate))], ["API-equivalent estimate", money(t?.estimateUsd)]];
    return `<div class="usage-metrics">${cards.map(([name, v]) => `<div><span>${name}</span><strong>${v}</strong></div>`).join("")}</div>`;
  }
  function filterForm(snapshot, data) {
    return `<form id="usage-filters" class="usage-filters"><label>From (UTC)<input name="start" type="date" value="${e(filters.start)}" required /></label><label>To (UTC)<input name="end" type="date" value="${e(filters.end)}" required max="${today()}" /></label>
      <label>Client<select name="surface">${options([["vs-code-copilot", "VS Code Copilot"], ["shared-copilot-runtime", "CLI / app shared runtime"]], filters.surface)}</select></label>
      <label>Route<select name="route">${options([["officialGithub", "GitHub official"], ["byok", "BYOK"], ["unknown", "Unknown"]], filters.route)}</select></label>
      <label>Connection<select name="connectionId">${options(snapshot.connections.map((c) => [c.id, c.name]), filters.connectionId)}</select></label>
      <label>Model<input name="model" maxlength="160" value="${e(filters.model ?? "")}" placeholder="Exact raw or canonical ID" /></label>
      <label>Confidence<select name="confidence">${options([["explicit", "Explicit source evidence"], ["exactConnectionIdentity", "Exact connection identity"], ["unknown", "Unknown"]], filters.confidence)}</select></label>
      <label>Source<select name="sourceId">${options((data.sources ?? []).map((s) => [s.id, s.name]), filters.sourceId)}</select></label>
      <label>Last source status<select name="sourceStatus">${options(["available", "partial", "successfulEmpty", "disabled", "missing", "schemaError", "canceled", "interrupted"].map((s) => [s, label(s)]), filters.sourceStatus)}</select></label>
      <button class="button primary small" type="submit">Apply filters</button></form>`;
  }
  function overview(snapshot, data) {
    const local = data.localUsage;
    return `<section class="usage-panel"><div class="step-heading"><h2>Local token observations</h2>${badge(local?.status ?? "unavailable")}</div>${sectionError(data, "localUsage")}
      ${metrics(local?.totals)}<p>${e(coverage(local?.totals))}</p><details><summary>Coverage and calculation</summary>
      <p>${e(local?.detail ?? "Enable a supported source and import metadata to see local usage.")}</p>
      <p>Cache hit rate = summed cache-read tokens ÷ summed normalized input over records where both are known. ${value(local?.totals?.cacheCoveredRecords)} records are covered. Missing values remain unknown.</p>
      <p>Priced requests: ${value(local?.totals?.pricedRequests)} · Unpriced requests: ${value(local?.totals?.unpricedRequests)} · Priced tokens: ${value(local?.totals?.pricedTokens)} · Unpriced tokens: ${value(local?.totals?.unpricedTokens)}</p>
      <p>Observed interval: ${e(time(local?.coverageStart))} – ${e(time(local?.coverageEnd))}. Last import: ${e(time(local?.lastImported))}.</p>
      <p>Estimates are a comparison to published text-token API rates. They are separate from GitHub Billing amounts. Current client configuration never assigns a route to historical usage.</p>
      <p>Bound price snapshots: ${(local?.totals?.priceSnapshots ?? []).map((s) => `<code>${e(s)}</code>`).join(" ") || "None"}</p></details>
      ${filterForm(snapshot, data)}</section>`;
  }
  function official(data, native) {
    const runtime = data.quota?.latest, previous = data.quota?.lastSuccessful;
    const shown = success(runtime?.status) ? runtime : previous;
    return `<section class="usage-panel"><div class="step-heading"><h2>Official runtime quota</h2>${button("runtime", busy.has("runtime") ? "Reading runtime…" : "Refresh quota and models", !native || busy.has("runtime"))}${button("cancel", "Cancel running jobs", !native || (!busy.has("runtime") && !busy.has("billing")))}</div>
      ${sectionError(data, "quota")}<div class="inline-actions">${badge(runtime?.status ?? "unavailable")}${data.quota?.stale ? badge("stale") : ""}</div><p>${e(runtime?.detail ?? "Refresh uses the official Copilot runtime signed in on this computer.")}</p>
      ${shown ? `<p><strong>${e(shown.account ?? "Account unknown")}</strong> · Runtime ${e(shown.runtimeVersion ?? "unknown")} · Observed ${e(time(shown.fetchedAt))}${shown !== runtime ? " · Last successful snapshot; current account may be unavailable" : ""}</p>
      <div class="usage-table-wrap"><table class="usage-table"><thead><tr><th>Quota / unit key</th><th>Used</th><th>Entitlement</th><th>Remaining</th><th>Reset</th></tr></thead><tbody>${shown.quotas.map((q) => `<tr><td>${e(q.key)}</td><td>${value(q.used)}</td><td>${q.unlimited ? "Unlimited" : value(q.entitlement)}</td><td>${q.remainingPercentage == null ? "Unknown" : `${e(q.remainingPercentage)}%`}</td><td>${e(q.resetAt ?? "Not reported")}</td></tr>`).join("")}</tbody></table></div>${!shown.quotas.length ? empty("The runtime returned an empty quota map. No zero entitlement is inferred.") : ""}` : empty("Runtime quota has not been obtained. Missing, unsupported and unauthenticated states are distinct.")}
      <p class="muted">Plan name is not exposed by this supported quota response. Quota keys retain their runtime units. Model catalog: ${e(label(runtime?.modelStatus ?? "unavailable"))}.</p></section>
      <section class="usage-panel"><div class="step-heading"><h2>Personal GitHub Billing</h2><div class="inline-actions"><label>Month<input type="month" id="billing-month" value="${e(billingMonth)}" max="${today().slice(0, 7)}" /></label>${button("billing", busy.has("billing") ? "Refreshing…" : "Refresh personal Billing", !native || busy.has("billing") || !data.billing?.authorization?.hasSecret)}</div></div>
        ${sectionError(data, "billing")}<p>${data.billing?.account ? `${e(data.billing.account.login)}@${e(data.billing.account.host)} · GitHub user ID ${e(data.billing.account.userId)}` : "Separate PilotWeave authorization is required for personal Billing."} ${button("authorize", "Manage authorization", !native)}</p>
        <p class="muted">Personal account coverage only. Organization-paid or inaccessible usage remains Not covered / Unknown. AI credits and premium requests are different units and are never added.</p>
        ${(data.billing?.families ?? [{ endpointFamily: "aiCredit" }, { endpointFamily: "premiumRequest" }]).map(billingFamily).join("")}</section>`;
  }
  function billingFamily(f) {
    const latest = f.latest, shown = success(latest?.status) ? latest : f.lastSuccessful;
    const stale = shown && (shown !== latest || Date.now() - Date.parse(shown.fetchedAt) > 15 * 60000);
    return `<article class="billing-family"><div class="step-heading"><h3>${f.endpointFamily === "aiCredit" ? "AI credits" : "Premium requests"}</h3>${badge(latest?.status ?? "unavailable")}${stale ? badge("stale") : ""}</div>
      <p>${e(latest?.error ?? (latest ? label(latest.coverage) : "No snapshot for this account and month"))}</p>${latest?.retryAt ? `<p>GitHub retry time: ${e(time(latest.retryAt))}</p>` : ""}
      <small>Latest attempt ${e(time(latest?.fetchedAt))}${shown ? ` · Displaying ${e(time(shown.fetchedAt))} · API ${e(shown.apiVersion)} · Period ${e(shown.periodStart.slice(0, 10))} to ${e(shown.periodEnd.slice(0, 10))} (exclusive)` : ""}</small>
      ${shown?.items?.length ? `<div class="usage-table-wrap"><table class="usage-table"><thead><tr><th>Model / SKU</th><th>Quantity</th><th>Unit</th><th>Gross USD</th><th>Discount USD</th><th>Official net USD</th><th>Allowance / remaining</th></tr></thead><tbody>${shown.items.map((i) => `<tr><td>${e(i.model ?? i.sku ?? i.product ?? "Not reported")}<small>${e(i.sku ?? "")}</small></td><td>${value(i.quantity)}</td><td>${e(i.unit)}</td><td>${money(i.grossAmountUsd)}</td><td>${money(i.discountAmountUsd)}</td><td>${money(i.netAmountUsd)}</td><td>${value(i.allowance)} / ${value(i.remaining)}<small>Reset: ${e(i.resetAt ?? "Not reported")}</small></td></tr>`).join("")}</tbody></table></div>${shown.itemsTruncated ? `<p>Showing the first ${shown.items.length} of ${shown.totalItemCount} items. No total is inferred from this partial list.</p>` : ""}` : empty(shown?.status === "successfulEmpty" ? "Successful query with no items. This does not establish zero usage outside personal coverage." : "No authoritative amounts are available.")}</article>`;
  }
  function models(snapshot, data) {
    const union = modelUnion(snapshot, data).filter((r) => (!filters.route || filters.route === r.route) && (!filters.model || r.rawModel === filters.model || r.canonicalModel === filters.model));
    const pageCount = Math.max(1, Math.ceil(union.length / 40)); modelPage = Math.min(modelPage, pageCount - 1);
    return `<section class="usage-panel"><div class="step-heading"><h2>Models across all available sources</h2><span>${union.length} model / route entries</span></div><p>Local figures follow the filters above. Configured models, the runtime catalog and the selected Billing month remain visible when local metrics are unavailable. Presence in a catalog does not prove a request used that route.</p>
      <div class="usage-table-wrap"><table class="usage-table model-table"><thead><tr><th>Raw / canonical model</th><th>Route / confidence</th><th>Clients / source</th><th>Records / requests</th><th>Input reported / normalized</th><th>Fresh input</th><th>Cache read / write</th><th>Output</th><th>Cache hit</th><th>API-equivalent estimate</th><th>Official Billing</th><th>Coverage / quality</th></tr></thead><tbody>${union.slice(modelPage * 40, modelPage * 40 + 40).map((m) => {
        const t = m.totals;
        return `<tr><td><strong>${e(m.rawModel)}</strong><small>${e(m.canonicalModel ?? "Canonical ID unresolved")}</small></td><td>${e(label(m.route))}<small>${e(label(m.confidence))}</small></td><td>${m.surfaces.map((s) => e(label(s))).join(", ") || "Not observed"}<small>${m.origins.map(e).join(" · ")}</small></td><td>${value(t?.records)} / ${value(t?.requests)}</td><td>${value(t?.inputReported)} / ${value(t?.normalizedInput)}</td><td>${value(t?.freshInput)}</td><td>${value(t?.cacheRead)} / ${value(t?.cacheWrite)}</td><td>${value(t?.output)}</td><td>${e(percent(t?.cacheHitRate))}</td><td>${money(t?.estimateUsd)}</td><td>${m.official.map((i) => `${value(i.quantity)} ${e(i.unit)} · ${money(i.netAmountUsd)}${i.stale ? " (stale)" : ""}`).join("<br>") || "Not available"}</td><td><details><summary>${e(coverage(t))}</summary><p>${m.quality.map(e).join("<br>") || "No local quality observations"}</p><p>${m.availability.map(e).join(" · ")}</p></details></td></tr>`;
      }).join("")}</tbody></table></div>${!union.length ? empty("No matching models. Add a Connection, refresh the runtime, or import a supported source.") : ""}
      <div class="usage-pagination">${button("models-prev", "Previous", modelPage === 0)}<span>Page ${modelPage + 1} / ${pageCount}</span>${button("models-next", "Next", modelPage >= pageCount - 1)}</div></section>
      ${breakdown("Daily observations (UTC)", data.localUsage?.days)}${breakdown("Client / shared runtime", data.localUsage?.surfaces)}${breakdown("Route", data.localUsage?.routes)}${breakdown("Reported provider", data.localUsage?.providers)}`;
  }
  function breakdown(title, rows) {
    return `<details class="usage-panel"><summary>${e(title)}</summary>${rows?.length ? `<div class="usage-table-wrap"><table class="usage-table"><thead><tr><th>Group</th><th>Records</th><th>Input</th><th>Output</th><th>Cache hit</th><th>Estimate USD</th><th>Priced records</th></tr></thead><tbody>${rows.map((r) => `<tr><td>${e(label(r.key))}</td><td>${r.totals.records}</td><td>${value(r.totals.inputReported)}</td><td>${value(r.totals.output)}</td><td>${e(percent(r.totals.cacheHitRate))}</td><td>${money(r.totals.estimateUsd)}</td><td>${r.totals.pricedRecords}/${r.totals.records}</td></tr>`).join("")}</tbody></table></div>` : empty("No observations for these filters.")}</details>`;
  }
  function sessions(data) {
    const rows = data.localUsage?.records ?? [], count = data.localUsage?.totalRecords ?? 0;
    const pages = Math.max(1, Math.ceil(count / filters.pageSize));
    return `<section class="usage-panel"><div class="step-heading"><h2>Sessions and requests</h2><span>${count} matching observations</span></div><p>Session/model totals replace earlier cumulative values. Individual VS Code request spans use a stable deduplication key. Session and request IDs are hashed.</p>
      <div class="usage-table-wrap"><table class="usage-table"><thead><tr><th>Finished</th><th>Model / record kind</th><th>Source / route</th><th>Input / output</th><th>Estimate</th><th>Details</th></tr></thead><tbody>${rows.map((r) => `<tr><td>${e(time(r.finishedAt))}</td><td>${e(r.rawModel)}<small>${e(label(r.counterKind))}</small></td><td>${e(r.sourceId)}<small>${e(label(r.route))} · ${e(label(r.confidence))}</small></td><td>${value(r.inputReported)} / ${value(r.output)}</td><td>${money(r.estimateUsd)}</td><td>${button("record", "Inspect", false, `data-record-id="${e(r.id)}"`)}</td></tr>`).join("")}</tbody></table></div>
      ${!rows.length ? empty("No imported observations in this range. Source setup and import results are available under Data sources.") : ""}<div class="usage-pagination">${button("prev", "Previous", filters.page === 0)}<span>Page ${filters.page + 1} / ${pages}</span>${button("next", "Next", filters.page >= pages - 1)}</div></section>`;
  }
  function sources(data, native) {
    return `<section class="usage-panel"><div class="step-heading"><h2>Data sources and coverage</h2><div class="inline-actions">${button("sync", busy.has("sync") ? "Importing…" : "Import enabled sources", !native || busy.has("sync") || !(data.sources ?? []).some((s) => s.enabled))}${button("cancel", "Cancel running jobs", !native || (!busy.has("sync") && !busy.has("runtime") && !busy.has("billing") && !busy.has("prices")))}</div></div>
      <p>Import is opt-in and local. PilotWeave reads only fixed supported sources and stores allowlisted metadata. Original logs may contain conversation content; raw lines and content are never retained in PilotWeave.</p>${sectionError(data, "sources")}
      <div class="usage-progress" aria-live="polite">${[...progress.values()].map((r) => `<p>${e(r.sourceId)}: ${e(label(r.status))} · ${r.filesSeen} files · ${r.recordsSeen} observations processed</p>`).join("")}</div>
      ${(data.sources ?? []).map((s) => `<article class="usage-source"><div class="step-heading"><h3>${e(s.name)}</h3>${badge(s.status)}</div><p>${e(s.detail)}</p><small>${e(s.pathHint)} · Parser ${e(s.parser)}</small><p>${s.recordCount} retained records · Last scan ${e(time(s.lastScanAt))} · Last successful scan ${e(time(s.lastSuccessAt))}</p><div class="inline-actions">
        ${button(s.enabled ? "disable-source" : "enable-source", s.enabled ? "Disable import" : "Enable import…", !native || s.status === "unsupported" || busy.has("sync"), `data-source-id="${e(s.id)}"`)}
        ${button("source-info", "Setup and privacy", false, `data-source-id="${e(s.id)}"`)}${button("clear-source", "Clear imported metadata…", !native || busy.has("sync"), `data-source-id="${e(s.id)}"`)}</div></article>`).join("") || empty("Source status is unavailable.")}
      <details><summary>Coverage limits</summary><p>CLI shutdown data has cumulative model counters but does not establish total-vs-fresh input semantics or which client used the shared runtime. VS Code only contributes supported exported chat spans after opt-in. Missing cache buckets, unknown routes and active sessions limit coverage. Disabling import retains metadata; clearing it disables the source and removes only PilotWeave's imported data. To stop VS Code export, turn off its OTel setting as well.</p></details></section>${prices(data, native)}`;
  }
  function prices(data, native) {
    const view = data.catalog, catalog = view?.catalog;
    const rows = (catalog?.models ?? []).filter((m) => m.model.toLowerCase().includes(priceSearch.toLowerCase()));
    return `<section class="usage-panel"><div class="step-heading"><h2>Price catalog and historical estimates</h2>${button("prices", busy.has("prices") ? "Refreshing…" : "Refresh official OpenRouter catalog", !native || busy.has("prices"))}${button("cancel", "Cancel running jobs", !native || !busy.has("prices"))}</div>
      ${sectionError(data, "catalog")}<p>${e(view?.latest?.detail ?? "Fetch a catalog to bind compatible local observations to an immutable price snapshot.")}</p>${view?.stale ? badge("stale") : ""}
      ${catalog ? `<p>${e(catalog.detail)}</p><p>Checked ${e(time(view?.checkedAt))} · Snapshot created ${e(time(catalog.fetchedAt))} · Parser v${catalog.parserVersion} · <a href="https://openrouter.ai/api/v1/models" target="_blank" rel="noreferrer">Official source</a></p><details><summary>Snapshot identity</summary><code>${e(catalog.id)}</code><p>Source version: ${e(catalog.sourceVersion)}</p><p>Existing estimates retain their bound snapshot. Missing token buckets or ambiguous context tiers keep estimates unavailable. Zero-priced models remain distinguishable from missing rates.</p></details>
      <form id="price-filter" class="inline-actions"><label>Find a rate<input name="priceSearch" value="${e(priceSearch)}" maxlength="160" placeholder="Model ID" /></label><button class="button ghost small">Find</button></form>
      <p>Showing ${Math.min(rows.length, 80)} of ${rows.length} matching models. Rates are USD per million text tokens.</p><div class="usage-table-wrap"><table class="usage-table"><thead><tr><th>Canonical model</th><th>Input threshold</th><th>Input</th><th>Cache read</th><th>Cache write / 1 h</th><th>Output</th></tr></thead><tbody>${rows.slice(0, 80).flatMap((m) => m.supported ? m.tiers.map((t) => `<tr><td>${e(m.model)}</td><td>≥ ${t.minInput}</td><td>${money(t.input)}</td><td>${money(t.cacheRead)}</td><td>${money(t.cacheWrite)} / ${money(t.cacheWrite1h)}</td><td>${money(t.output)}</td></tr>`) : [`<tr><td>${e(m.model)}</td><td colspan="5">Unsupported pricing schema; no estimate</td></tr>`]).join("")}</tbody></table></div>` : empty("No price snapshot. Estimates remain unavailable until compatible rates and token buckets are known.")}</section>`;
  }
  function render(snapshot, data, native) {
    return `${!native ? '<div class="preview-banner">Browser preview · no native usage, quota or Billing access</div>' : ""}${overview(snapshot, data)}
      <nav class="usage-tabs" aria-label="Usage views">${[["official", "Official subscription / usage"], ["models", "Model statistics"], ["sessions", "Sessions / details"], ["sources", "Data sources / prices"]].map(([id, name]) => `<button class="${tab === id ? "active" : ""}" data-usage-tab="${id}" aria-current="${tab === id ? "page" : "false"}">${name}</button>`).join("")}</nav>
      ${{ official: () => official(data, native), models: () => models(snapshot, data), sessions: () => sessions(data), sources: () => sources(data, native) }[tab]()}`;
  }
  function setupInstructions(source) {
    const settings = source.id === "vscode-otel" ? { "github.copilot.chat.otel.enabled": true, "github.copilot.chat.otel.exporterType": "file", "github.copilot.chat.otel.outfile": source.setupPath, "github.copilot.chat.otel.captureContent": false } : null;
    return `<p>${e(source.pathHint)}</p>${settings ? `<p>After enabling import, add these settings yourself in VS Code's User settings JSON, then restart VS Code. PilotWeave creates its own inbox directory; it does not change VS Code settings. These settings affect future observations only.</p><pre class="usage-code">${e(JSON.stringify(settings, null, 2))}</pre><p>To stop exporting, set <code>github.copilot.chat.otel.enabled</code> to <code>false</code>.</p>` : source.id === "copilot-session-events" ? "<p>Use the official Copilot runtime normally. Finished sessions with supported v1 shutdown model metrics can be imported from the fixed session directory. A session shared by CLI and the app is counted once with client attribution unknown.</p>" : "<p>No stable separate app usage source is supported. PilotWeave does not inspect private app stores.</p>"}
      <p>Raw conversation content can coexist in source logs even when content capture is disabled. PilotWeave persists only model/provider IDs, hashed session/request IDs, numeric counters, times and derived metadata. It never uploads logs. Disable import to stop reading; Clear imported metadata removes this source's records and cursors from PilotWeave.</p>`;
  }
  function confirm(title, body, text, action) {
    context.openModal({ title, body, wide: true, footer: `<button class="button ghost" data-modal-close>Cancel</button><button class="button primary" id="usage-confirm">${e(text)}</button>`, onOpen(root) {
      root.querySelector("#usage-confirm").addEventListener("click", async (event) => {
        event.target.disabled = true;
        try { await action(); context.closeModal(); await context.refresh(); }
        catch (err) { context.showToast(err?.message ?? String(err), "error"); event.target.disabled = false; }
      });
    } });
  }
  async function run(key, command, args) {
    if (!context.native || busy.has(key)) return;
    busy.add(key); context.render();
    try { await context.invoke(command, args); }
    catch (err) { context.showToast(err?.message ?? String(err), "error"); }
    finally { busy.delete(key); await context.refresh(); }
  }
  async function action(buttonElement) {
    const name = buttonElement.dataset.usageAction, sourceId = buttonElement.dataset.sourceId;
    const data = context.getData(), source = data.sources?.find((s) => s.id === sourceId);
    if (name === "record") {
      const record = data.localUsage?.records.find((r) => r.id === buttonElement.dataset.recordId);
      if (record) context.openModal({ title: "Imported metadata", wide: true, body: `<p>Allowlisted metadata only. Identifiers below are hashes; original source lines and conversation content are never retained.</p><pre class="usage-code">${e(JSON.stringify(record, null, 2))}</pre>`, footer: '<button class="button ghost" data-modal-close>Close</button>' });
    } else if (name === "source-info" && source) {
      context.openModal({ title: `${source.name} setup`, wide: true, body: setupInstructions(source), footer: '<button class="button ghost" data-modal-close>Close</button>' });
    } else if (name === "enable-source" && source && context.native) {
      confirm(`Enable ${source.name} import`, setupInstructions(source), "Enable local metadata import", () => context.invoke("set_usage_source_enabled", { sourceId, enabled: true }));
    } else if (name === "disable-source") {
      await run("sources", "set_usage_source_enabled", { sourceId, enabled: false });
    } else if (name === "clear-source" && source && context.native) {
      confirm(`Clear ${source.name} metadata`, `<p>Remove ${source.recordCount} imported records, this source's cursors and import history, then disable its import. Original client logs, Billing snapshots and price snapshots are retained. Re-enabling and importing later can recreate records from existing logs.</p>`, "Clear imported metadata", () => context.invoke("clear_local_usage", { sourceId, confirmed: true }));
    } else if (name === "runtime") await run("runtime", "refresh_official_runtime_usage");
    else if (name === "billing") await run("billing", "refresh_personal_github_usage", billingQuery());
    else if (name === "prices") await run("prices", "refresh_price_catalog");
    else if (name === "sync") { progress.clear(); await run("sync", "sync_local_usage", { sourceIds: [] }); }
    else if (name === "cancel" && context.native) await context.invoke("cancel_usage_sync");
    else if (name === "authorize") globalThis.PilotWeaveGithubAuth?.openAuthorize?.();
    else if (name === "prev" || name === "next") { filters.page = Math.max(0, filters.page + (name === "next" ? 1 : -1)); await context.refresh(); }
    else if (name === "models-prev" || name === "models-next") { modelPage = Math.max(0, modelPage + (name === "models-next" ? 1 : -1)); context.render(); }
  }
  function configure(api) {
    context = api;
    document.addEventListener("click", (event) => {
      const tabButton = event.target.closest("[data-usage-tab]");
      if (tabButton) { tab = tabButton.dataset.usageTab; context.render(); }
      const target = event.target.closest("[data-usage-action]");
      if (target && !target.disabled) action(target).catch((err) => context.showToast(err?.message ?? String(err), "error"));
    });
    document.addEventListener("submit", async (event) => {
      if (event.target.id === "usage-filters") {
        event.preventDefault(); const form = new FormData(event.target);
        for (const field of Object.keys(filters)) if (!["page", "pageSize"].includes(field)) filters[field] = String(form.get(field) ?? "").trim() || null;
        filters.page = 0; modelPage = 0; await context.refresh();
      } else if (event.target.id === "price-filter") { event.preventDefault(); priceSearch = String(new FormData(event.target).get("priceSearch") ?? ""); context.render(); }
    });
    document.addEventListener("change", async (event) => {
      if (event.target.id === "billing-month" && event.target.value) { billingMonth = event.target.value; await context.refresh(); }
    });
    globalThis.__TAURI__?.event?.listen("usage-progress", (event) => {
      progress.set(event.payload.sourceId, event.payload);
      const region = document.querySelector(".usage-progress");
      if (region) region.textContent = [...progress.values()].map((r) => `${label(r.status)} · ${r.sourceId} · ${r.filesSeen} files · ${r.recordsSeen} observations processed`).join("\n");
    }).catch((err) => context.showToast(`Import progress unavailable: ${String(err)}`, "error"));
  }
  function billingQuery() { const [year, month] = billingMonth.split("-").map(Number); return { year, month }; }
  function preview(command, snapshot) {
    const unavailable = { status: "unavailable", detail: "Browser preview has no native data" };
    if (command === "get_usage_overview") return { ...unavailable, start: filters.start, end: filters.end, totals: { records: 0, pricedRecords: 0, unpricedRecords: 0, unresolvedModels: 0, unknownSemantics: 0, priceSnapshots: [] }, models: [], days: [], routes: [], surfaces: [], providers: [], records: [], totalRecords: 0 };
    if (command === "get_official_runtime_usage") return { latest: null, lastSuccessful: null, stale: false };
    if (command === "get_price_catalog") return { catalog: null, latest: null, stale: false };
    if (command === "get_github_billing") return { authorization: { hasSecret: false }, account: null, families: [] };
    if (command === "get_usage_runs") return [];
    if (command === "get_usage_sources") return ["copilot-session-events", "vscode-otel", "github-copilot-app"].map((id) => ({ id, name: label(id), enabled: false, status: id === "github-copilot-app" ? "unsupported" : "disabled", detail: "Native source inspection is disabled in browser preview", pathHint: "Native path unavailable", setupPath: null, parser: "Preview only", recordCount: 0 }));
    if (command === "get_setup_status") return { preferences: { connectionId: snapshot.connections[0]?.id }, target: null, alignment: "notSignedIn", accounts: globalThis.PilotWeaveAccount.previewStatus(), targets: [], manualConfirmed: false, scannedAt: null };
    if (command === "get_installation_status") return globalThis.PilotWeaveInstaller.previewStatus();
    if (command === "get_github_authorization_status") return globalThis.PilotWeaveGithubAuth.previewStatus();
    throw new Error("This operation requires the native desktop app");
  }
  return { configure, render, query: () => ({ ...filters }), billingQuery, preview, modelUnion, money, value, percent, setupInstructions, label, metrics, coverage };
});
