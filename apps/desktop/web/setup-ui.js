(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.PilotWeaveSetup = api;
})(globalThis, () => {
  "use strict";
  const escape = (v) => String(v ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#039;");
  const label = (s) => ({ inSync: "In sync", notDeployed: "Not deployed", outOfDate: "Out of date", notInstalled: "Not installed", userConfirmedSameAccount: "Same account · user confirmed", verifiedSameAccount: "Same account · verified", partiallyVerified: "Requires confirmation", notSignedIn: "Choose and confirm an account", actionRequired: "Action required", successfulEmpty: "No observations imported", networkError: "Network unavailable", schemaError: "Unsupported data schema" })[s] ?? String(s ?? "Unknown").replace(/([a-z])([A-Z])/g, "$1 $2").replace(/^./, (c) => c.toUpperCase());
  const date = (s) => s && !Number.isNaN(Date.parse(s)) ? new Date(s).toLocaleString() : "Unavailable";
  const value = (n) => n == null ? "Unknown" : String(n);
  function derive(snapshot, data, native = true) {
    const setup = data.setup;
    const components = data.components ?? [];
    const scanned = native && Boolean(setup && components.length >= 4) && !data.errors?.setup && !data.errors?.components;
    const installed = scanned && components.length >= 4 && components.every((c) => c.status === "ready");
    const missing = components.filter((c) => ["missing", "broken"].includes(c.status));
    const aligned = scanned && ["verifiedSameAccount", "userConfirmedSameAccount"].includes(setup?.alignment);
    const selected = snapshot.connections.find((c) => c.id === setup?.preferences?.connectionId) ?? snapshot.connections[0];
    const writable = snapshot.clients.filter((c) => c.detected && c.supportsWrite);
    const manual = snapshot.clients.filter((c) => c.detected && !c.supportsWrite);
    const inSync = writable.filter((c) => setup?.targets?.find((t) => t.targetId === c.id)?.state === "inSync").length;
    const deployed = scanned && !snapshot.stateRecovery && !snapshot.deploymentRecovery && Boolean(selected) && writable.length > 0 && inSync === writable.length && (!manual.length || setup?.manualConfirmed);
    const complete = [scanned, installed, aligned, deployed].filter(Boolean).length;
    let next = { action: "route", route: "usage", text: "View usage insights" };
    if (!scanned) next = { action: "refresh", text: native ? "Scan this computer" : "Preview only · open the desktop app" };
    else if (snapshot.deploymentRecovery) next = { action: "preview-recovery", text: "Review interrupted deployment" };
    else if (snapshot.stateRecovery) next = { action: "route", route: "settings", text: "Review storage recovery" };
    else if (!installed) next = missing.length ? { action: "setup-install", text: `Install ${missing.length} missing components` } : { action: "route", route: "clients", text: "Review component issues" };
    else if (!aligned) next = { action: "setup-accounts", text: "Verify the same GitHub account" };
    else if (!selected) next = { action: "add-connection", text: "Add a model connection" };
    else if (!deployed) next = inSync === writable.length && manual.length ? { action: "setup-manual", text: "Complete Copilot app setup" } : { action: "setup-deploy", text: `Preview and deploy ${selected.name}` };
    return { scanned, installed, missing, aligned, selected, writable, manual, inSync, deployed, complete, next };
  }
  const badge = (state) => `<span class="setup-badge ${["ready", "inSync", "verified", "verifiedSameAccount", "userConfirmedSameAccount"].includes(state) ? "good" : ""}">${escape(label(state))}</span>`;
  function connectionCard(snapshot, data, s, native) {
    const blocked = Boolean(snapshot.stateRecovery || snapshot.deploymentRecovery);
    const models = s.selected?.models?.filter(m => m.enabled) ?? [];
    return `<section class="setup-step setup-connection-card" aria-label="Connection and models"><div class="step-body">
      <div class="step-heading"><h2>Connection and models</h2>${badge(s.deployed ? "ready" : "actionRequired")}</div>
      <p>Choose or edit your provider, then preview the configuration changes for your clients.</p>
      <div class="setup-connection-controls"><label class="setup-select">Current connection<select id="setup-connection" ${!snapshot.connections.length || blocked ? "disabled" : ""}>${snapshot.connections.map(c => `<option value="${escape(c.id)}" ${c.id === s.selected?.id ? "selected" : ""}>${escape(c.name)}</option>`).join("") || '<option>No connections yet</option>'}</select></label>
      <button class="button ghost small" data-action="edit-connection" data-id="${escape(s.selected?.id)}" ${!s.selected || blocked ? "disabled" : ""}>Edit connection</button><button class="button ghost small" data-action="add-connection" ${blocked ? "disabled" : ""}>Add connection</button></div>
      ${s.selected ? `<p class="setup-endpoint">${escape(s.selected.baseUrl ?? "")} · ${escape(s.selected.protocol ?? "")}</p><div class="model-list">${models.slice(0, 8).map(m => `<span class="model-chip">${escape(m.name)}</span>`).join("")}${models.length > 8 ? `<span class="model-chip">+${models.length - 8} more</span>` : ""}</div><p>${models.length} enabled models · Edit connection to add, remove or discover models.</p>` : '<p>Add a URL and API key to discover models and create your first connection.</p>'}
      <div class="inline-actions"><button class="button primary small" data-action="setup-deploy" ${!native || !s.selected || !s.writable.length || blocked ? "disabled" : ""}>${s.deployed ? "Review deployment" : `Preview deployment to ${s.writable.length} clients`}</button>${s.manual.length ? `<button class="button ghost small" data-action="setup-manual">${data.setup?.manualConfirmed ? "Review manual setup" : "Copilot app: manual setup"}</button>` : ""}</div>
      <details class="setup-target-details" ${!s.deployed ? "open" : ""}><summary>Deployment status · ${s.inSync}/${s.writable.length} automatic targets in sync${s.manual.length ? ` · ${s.manual.length} manual` : ""}</summary><div class="setup-rows">${snapshot.clients.map(c => { const state = data.setup?.targets?.find(t => t.targetId === c.id); const confirmed = !c.supportsWrite && c.detected && data.setup?.manualConfirmed; return `<div><span>${escape(c.name)}</span>${badge(confirmed ? "userConfirmed" : state?.state ?? "unknown")}<small>${escape(confirmed ? "Provider setup confirmed by you for this connection" : state?.detail ?? "Configuration has not been checked")}</small></div>`; }).join("")}</div></details>
    </div></section>`;
  }
  function render(snapshot, data, native) {
    const s = derive(snapshot, data, native);
    const account = data.setup?.target;
    const local = data.localUsage;
    const runtime = ["available", "successfulEmpty"].includes(data.quota?.latest?.status) ? data.quota.latest : data.quota?.lastSuccessful ?? data.quota?.latest;
    const quota = runtime?.status === "available" ? runtime.quotas?.find((q) => q.key === "premium_interactions") ?? runtime.quotas?.[0] : null;
    const next = s.next;
    return `
      ${!native ? '<div class="preview-banner" role="status">Browser preview · disposable sample data · no computer scan, installation or sign-in</div>' : ""}
      <section class="setup-hero"><div><p class="eyebrow">YOUR COPILOT SETUP</p><h2>${s.complete === 4 ? "Your core Copilot setup is ready" : "Set up GitHub Copilot on this computer"}</h2><p>Install apps, confirm one GitHub account, deploy one model setup, and track usage.</p></div>
        <div class="setup-progress"><strong>${native ? `${s.complete} of 4` : "Preview"}</strong><span>Core steps ready</span><button class="button primary" data-action="${next.action}" data-route="${next.route ?? ""}" ${!native ? "disabled" : ""}>${escape(next.text)}</button></div></section>
      ${connectionCard(snapshot, data, s, native)}
      <details class="setup-checks" ${!s.installed || !s.aligned ? "open" : ""}><summary>Installation and accounts · ${s.installed && s.aligned ? "Ready" : "Needs attention"}</summary><div class="setup-steps">
        <section class="setup-step"><div class="step-number">1</div><div class="step-body"><div class="step-heading"><h2>Scan this computer</h2>${badge(s.scanned ? "ready" : "unknown")}</div><p>${s.scanned ? `Last scanned ${escape(date(data.setup.scannedAt))}` : "A complete native scan is required to determine readiness."}</p>${data.errors?.setup ? `<p class="inline-error">${escape(data.errors.setup)}</p>` : ""}${data.setup?.preferencesError ? `<p class="inline-error">Saved setup preferences are unavailable: ${escape(data.setup.preferencesError)}</p>` : ""}<button class="button ghost small" data-action="refresh">Rescan</button></div></section>
        <section class="setup-step"><div class="step-number">2</div><div class="step-body"><div class="step-heading"><h2>Install Copilot apps</h2>${badge(s.installed ? "ready" : "actionRequired")}</div>
          <div class="setup-rows">${(data.components ?? []).map((c) => `<div><span>${escape(c.name)}</span>${badge(c.status)}${["missing", "broken"].includes(c.status) ? `<button class="button ghost small" data-install-action="install-one" data-component-id="${escape(c.id)}" ${!native ? "disabled" : ""}>${c.status === "broken" ? "Repair" : "Install"}</button>` : ""}</div>`).join("") || '<p>Component status unavailable</p>'}</div>
          <button class="button ghost small" data-install-action="install-all" ${!native || !s.missing.length ? "disabled" : ""}>Install all missing${s.missing.length ? ` (${s.missing.length})` : ""}</button></div></section>
        <section class="setup-step"><div class="step-number">3</div><div class="step-body"><div class="step-heading"><h2>Verify one GitHub account</h2>${badge(data.setup?.alignment ?? "unknown")}</div>
          <p>Target: <strong>${escape(account ? `${account.login}@${account.host}` : "Choose an account in the confirmation dialog")}</strong></p>
          <div class="setup-rows">${(data.setup?.accounts?.surfaces ?? []).map((a) => `<div><span>${escape(({ vsCodeCopilot: "VS Code Copilot", copilotCli: "Copilot CLI", githubCopilotApp: "Copilot app" })[a.surface] ?? a.surface)}</span>${badge(s.aligned && data.setup.alignment === "userConfirmedSameAccount" && a.state !== "verified" ? "userConfirmed" : a.state)}<button class="button ghost small" data-action="setup-signin" data-surface="${escape(a.surface)}" ${!native || a.state === "notInstalled" ? "disabled" : ""}>${a.surface === "vsCodeCopilot" ? "VS Code sign-in" : "Review sign-in"}</button><details><summary>Evidence · ${escape(label(a.state))}</summary><p>${escape(a.evidence)}</p><p>${escape(a.detail)}</p></details></div>`).join("")}</div>
          ${s.aligned && data.setup?.alignment === "userConfirmedSameAccount" ? `<p class="muted">Confirmed by you ${escape(date(data.setup.preferences.accountConfirmation?.confirmedAt))}. This confirmation expires after 30 days or when observed evidence changes.</p>` : ""}
          <div class="inline-actions"><button class="button ghost small" data-action="setup-accounts" ${!native ? "disabled" : ""}>Confirm accounts</button></div></div></section>
      </div></details>
      <section class="usage-home"><div class="step-heading"><h2>Usage insights</h2><span class="setup-badge">Optional · does not block core setup</span></div><div class="usage-home-grid">
        <div><h3>Official runtime quota</h3><strong>${quota ? (quota.unlimited ? "Unlimited" : `${escape(value(quota.used))} / ${escape(value(quota.entitlement))}`) : escape(label(runtime?.status ?? "unavailable"))}</strong><p>${quota ? `${escape(quota.key)} · reset ${escape(quota.resetAt ?? "unknown")}` : "Refresh from the signed-in Copilot runtime in Usage."}</p><small>${runtime ? `${escape(runtime.account ?? "Account unavailable")} · ${escape(date(runtime.fetchedAt))}${data.quota?.stale ? " · Stale" : ""}` : "Not refreshed"}</small></div>
        <div><h3>Local observations</h3><strong>${local?.totalRecords ? `${escape(value(local.totals.inputReported))} input · ${escape(value(local.totals.output))} output` : data.errors?.localUsage || !local || local.status === "unavailable" ? "Unavailable" : "No observations imported"}</strong><p>Cache hit: ${local?.totals.cacheHitRate == null ? "Unknown" : `${(Number(local.totals.cacheHitRate) * 100).toFixed(1)}% of covered input`}</p><small>${local?.start ? `${escape(local.start)} – ${escape(local.end)} (UTC) · ` : ""}API-equivalent estimate: ${local?.totals.estimateUsd == null ? "Unavailable" : `$${escape(local.totals.estimateUsd)} · ${local.totals.pricedRecords}/${local.totalRecords} records priced`} · ${escape(date(local?.lastImported))}</small></div></div><button class="button ghost small" data-action="route" data-route="usage">Open usage and data sources</button></section>`;
  }
  return { escape, label, date, derive, render };
});
