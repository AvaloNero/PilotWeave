(() => {
  "use strict";

  const invokeNative = window.__TAURI__?.core?.invoke ?? null;
  const isDesktop = typeof invokeNative === "function";
  const content = document.querySelector("#content");
  const modalRoot = document.querySelector("#modal-root");
  const toastRoot = document.querySelector("#toast-root");
  const pageTitle = document.querySelector("#page-title");
  const pageSubtitle = document.querySelector("#page-subtitle");
  const addConnectionButton = document.querySelector("#add-connection-button");
  const refreshButton = document.querySelector("#refresh-button");
  const runtimeLabel = document.querySelector("#runtime-label");
  const runtimeDetail = document.querySelector("#runtime-detail");
  const runtimeDot = document.querySelector(".runtime-dot");

  const routes = {
    overview: {
      title: "Home",
      subtitle: "One account. One model setup. Every supported client.",
      add: false,
    },
    connections: {
      title: "Connections",
      subtitle: "Your provider endpoints, credentials, and models.",
      add: true,
    },
    clients: {
      title: "Client diagnostics",
      subtitle: "Detected Copilot surfaces and their deployment capabilities.",
      add: false,
    },
    usage: { title: "Usage", subtitle: "Official quota, personal Billing, and local observations with source coverage.", add: false },
    about: { title: "About", subtitle: "PilotWeave", add: false },
    activity: {
      title: "Activity",
      subtitle: "Local deployment audit history without secret values.",
      add: false,
    },
    settings: {
      title: "Settings",
      subtitle: "Runtime, state ownership, and safety boundaries.",
      add: false,
    },
  };

  let route = "overview";
  let snapshot = null;
  let loading = false;
  const data = { errors: {} };
  const usageSections = () => [
    ["localUsage", "get_usage_overview", { query: window.PilotWeaveUsage.query() }],
    ["sources", "get_usage_sources"], ["quota", "get_official_runtime_usage"],
    ["billing", "get_github_billing", window.PilotWeaveUsage.billingQuery()], ["catalog", "get_price_catalog"], ["runs", "get_usage_runs"],
  ];
  const sectionVersions = {};
  async function loadSections(sections) {
    const versions = sections.map(([key]) => (sectionVersions[key] = (sectionVersions[key] ?? 0) + 1));
    const results = await Promise.allSettled(sections.map(([, command, args]) => invoke(command, args)));
    results.forEach((result, i) => {
      const key = sections[i][0];
      if (versions[i] !== sectionVersions[key]) return;
      if (result.status === "fulfilled") { data[key] = result.value; delete data.errors[key]; }
      else { data[key] = null; data.errors[key] = result.reason?.message ?? String(result.reason); }
    });
  }
  async function refreshUsage() { await loadSections(usageSections()); render(); }


  function nowIso() {
    return new Date().toISOString();
  }

  function model(id, name) {
    return {
      id: crypto.randomUUID(),
      modelId: id,
      name,
      enabled: true,
      capabilities: {
        toolCalling: true,
        vision: null,
        reasoning: null,
        contextWindow: null,
        maxOutputTokens: null,
      },
    };
  }

  function createDemoSnapshot() {
    const createdAt = nowIso();
    return {
      version: 1,
      statePath: "Browser preview — no filesystem access",
      stateRecovery: null,
      usageDb: {
        state: "unavailable",
        detail: "Browser preview has no native usage database",
        path: null,
        schemaVersion: null,
      },
      connections: [
        {
          id: "demo-openrouter",
          name: "OpenRouter",
          baseUrl: "https://openrouter.ai/api/v1",
          providerKind: "openai",
          protocol: "chat-completions",
          headers: { "HTTP-Referer": "https://pilotweave.dev" },
          models: [
            model("anthropic/claude-sonnet-4", "Claude Sonnet 4"),
            model("openai/gpt-5", "GPT-5"),
          ],
          secretRef: "connection:demo-openrouter",
          hasSecret: true,
          createdAt,
          updatedAt: createdAt,
        },
      ],
      clients: [
        {
          id: "vscode:stable:default",
          kind: "vs-code-copilot",
          name: "Visual Studio Code · Default",
          detail: "Default profile language-model catalog",
          path: "~/Library/Application Support/Code/User/chatLanguageModels.json",
          detected: true,
          supportsWrite: true,
          status: "available",
          diagnostic: null,
        },
        {
          id: "copilot-cli:user-environment",
          kind: "copilot-cli",
          name: "GitHub Copilot CLI",
          detail: "User-level provider environment",
          path: "/usr/local/bin/copilot",
          detected: true,
          supportsWrite: true,
          status: "available",
          diagnostic: null,
        },
        {
          id: "github-copilot-app:local",
          kind: "github-copilot-app",
          name: "GitHub Copilot app",
          detail: "Installation detected; provider management is manual in this MVP",
          path: "/Applications/GitHub Copilot.app",
          detected: true,
          supportsWrite: false,
          status: "read-only",
          diagnostic:
            "PilotWeave will not write private app state or credential storage without a stable external interface",
        },
      ],
      deployments: [],
    };
  }

  function loadDemoState() {
    const raw = sessionStorage.getItem("pilotweave-demo");
    if (!raw) return createDemoSnapshot();
    try {
      return JSON.parse(raw);
    } catch {
      return createDemoSnapshot();
    }
  }

  let demoState = loadDemoState();
  const demoPlans = new Map();

  function saveDemoState() {
    sessionStorage.setItem("pilotweave-demo", JSON.stringify(demoState));
  }

  async function invoke(command, args = {}) {
    if (isDesktop) return invokeNative(command, args);
    await new Promise((resolve) => setTimeout(resolve, 90));
    return invokeMock(command, args);
  }

  function invokeMock(command, args) {
    switch (command) {
      case "get_dashboard":
        return structuredClone(demoState);
      case "upsert_connection": {
        const input = args.input;
        const existing = input.id
          ? demoState.connections.find((item) => item.id === input.id)
          : null;
        const id = existing?.id ?? crypto.randomUUID();
        const timestamp = nowIso();
        const connection = {
          id,
          name: input.name.trim(),
          baseUrl: input.baseUrl.trim().replace(/\/$/, ""),
          providerKind: input.providerKind,
          protocol: input.protocol,
          headers: input.headers,
          models: input.models,
          secretRef: existing?.secretRef ?? `connection:${id}`,
          hasSecret: input.clearSecret
            ? false
            : Boolean(input.apiKey?.trim()) || Boolean(existing?.hasSecret),
          createdAt: existing?.createdAt ?? timestamp,
          updatedAt: timestamp,
        };
        const index = demoState.connections.findIndex((item) => item.id === id);
        if (index >= 0) demoState.connections[index] = connection;
        else demoState.connections.push(connection);
        demoState.connections.sort((a, b) => a.name.localeCompare(b.name));
        saveDemoState();
        return structuredClone(connection);
      }
      case "delete_connection": {
        demoState.connections = demoState.connections.filter(
          (item) => item.id !== args.connectionId,
        );
        demoState.deployments = demoState.deployments.filter(
          (item) => item.connectionId !== args.connectionId,
        );
        saveDemoState();
        return true;
      }
      case "preview_deployment": {
        const connection = demoState.connections.find(
          (item) => item.id === args.connectionId,
        );
        if (!connection) throw new Error("Unknown connection");
        const targets = args.targetIds.map((id) =>
          demoState.clients.find((item) => item.id === id),
        );
        if (!targets.length || targets.some((target) => !target)) throw new Error("Unknown deployment target");
        for (const [id, stored] of demoPlans) {
          if (Date.now() >= stored.expiresAt) demoPlans.delete(id);
        }
        if (demoPlans.size >= 32) throw new Error("Too many pending previews");
        const plan = createMockPlan(connection, targets);
        demoPlans.set(plan.id, {
          plan: structuredClone(plan),
          connectionRevision: JSON.stringify(connection),
          targetRevision: JSON.stringify(targets),
          expiresAt: Date.now() + 15 * 60 * 1000,
        });
        return plan;
      }
      case "apply_deployment_plan": {
        const stored = demoPlans.get(args.planId);
        demoPlans.delete(args.planId);
        if (!stored || Date.now() >= stored.expiresAt || args.confirmed !== true) {
          throw new Error("Preview expired or was consumed; preview again");
        }
        const { plan } = stored;
        const connection = demoState.connections.find(
          (item) => item.id === plan.connectionId,
        );
        if (!connection) throw new Error("Unknown connection");
        const targets = plan.targetIds
          .map((id) => demoState.clients.find((item) => item.id === id))
          .filter(Boolean);
        if (stored.connectionRevision !== JSON.stringify(connection) || stored.targetRevision !== JSON.stringify(targets)) {
          throw new Error("PlanChanged: preview again");
        }
        const records = plan.operations.map((operation) => ({
          id: crypto.randomUUID(),
          planId: plan.id,
          connectionId: connection.id,
          targetId: operation.targetId,
          targetKind: operation.targetKind,
          status: operation.supported ? "applied" : "skipped",
          detail: operation.supported
            ? "Browser preview simulated this deployment; no client configuration was written"
            : operation.description,
          createdAt: nowIso(),
        }));
        demoState.deployments.unshift(...records);
        demoState.deployments = demoState.deployments.slice(0, 200);
        saveDemoState();
        return { planId: plan.id, records };
      }
      default:
        return window.PilotWeaveUsage.preview(command, demoState);
    }
  }

  function createMockPlan(connection, targets) {
    return {
      id: crypto.randomUUID(),
      connectionId: connection.id,
      connectionName: connection.name,
      targetIds: targets.map((target) => target.id),
      createdAt: nowIso(),
      operations: targets.map((target) => ({
        id: crypto.randomUUID(),
        targetId: target.id,
        targetKind: target.kind,
        title: `Deploy ${connection.name} to ${target.name}`,
        description: target.supportsWrite
          ? `Update ${target.path ?? target.name}`
          : "Manual action required in the client's provider settings",
        changes: [
          `Publish ${connection.models.filter((item) => item.enabled).length} enabled model(s)`,
          "Preserve non-PilotWeave configuration",
          target.supportsWrite
            ? "Create a rollback-safe managed projection"
            : "Do not mutate unsupported private client state",
        ],
        supported: target.detected && target.supportsWrite,
        requiresRestart: target.kind === "copilot-cli",
      })),
    };
  }

  function escapeHtml(value) {
    return String(value ?? "")
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#039;");
  }

  function formatDate(value) {
    if (!value) return "—";
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return "—";
    return new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }).format(date);
  }

  function initials(name) {
    return String(name)
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((part) => part[0]?.toUpperCase())
      .join("");
  }

  function statusLabel(status) {
    return {
      available: "Available",
      "not-installed": "Not installed",
      "read-only": "Read only",
      error: "Error",
      applied: "Applied",
      skipped: "Skipped",
      failed: "Failed",
    }[status] ?? status;
  }

  function clientIcon(kind) {
    return {
      "vs-code-copilot": "⌘",
      "copilot-cli": ">_",
      "github-copilot-app": "◈",
    }[kind] ?? "◇";
  }

  function showToast(message, type = "success") {
    const toast = document.createElement("div");
    toast.className = `toast ${type}`;
    toast.textContent = message;
    toastRoot.append(toast);
    setTimeout(() => toast.remove(), 4200);
  }

  function setRuntimeState() {
    runtimeDot.classList.toggle("online", true);
    runtimeLabel.textContent = isDesktop ? "Native backend" : "Browser preview";
    runtimeDetail.textContent = isDesktop
      ? "Local writes enabled"
      : "No filesystem writes";
  }

  async function refresh() {
    if (loading) return;
    loading = true;
    refreshButton.disabled = true;
    content.innerHTML = '<div class="loading"><div><div class="spinner"></div>Loading local state…</div></div>';
    try {
      await loadSections([["dashboard", "get_dashboard"], ["components", "get_installation_status"], ["setup", "get_setup_status"], ["authorization", "get_github_authorization_status"], ...usageSections()]);
      if (!data.dashboard) throw new Error(data.errors.dashboard);
      snapshot = data.dashboard;
      window.PilotWeaveInstaller.hydrate(data.components);
      window.PilotWeaveAccount.hydrate(data.setup?.accounts);
      window.PilotWeaveGithubAuth.hydrate(data.authorization);
      setRuntimeState();
      render();
    } catch (error) {
      content.innerHTML = `
        <div class="empty-state">
          <div class="empty-icon">!</div>
          <h2>Could not load PilotWeave</h2>
          <p>${escapeHtml(error?.message ?? error)}</p>
          <button class="button" data-action="refresh">Try again</button>
        </div>`;
      showToast(error?.message ?? String(error), "error");
    } finally {
      loading = false;
      refreshButton.disabled = false;
    }
  }

  function setRoute(nextRoute) {
    if (!routes[nextRoute]) return;
    route = nextRoute;
    document.querySelectorAll(".nav-item").forEach((button) => {
      button.classList.toggle("active", button.dataset.route === route);
    });
    render();
  }

  function render() {
    if (!snapshot) return;
    const meta = routes[route];
    pageTitle.textContent = meta.title;
    pageSubtitle.textContent = meta.subtitle;
    addConnectionButton.hidden = !meta.add;
    addConnectionButton.disabled = Boolean(snapshot.stateRecovery);
    const renderers = {
      overview: renderOverview,
      connections: renderConnections,
      clients: renderClients,
      usage: () => window.PilotWeaveUsage.render(snapshot, data, isDesktop),
      about: () => '<section class="setting-row"><div><h2>PilotWeave</h2><p>An independent Copilot setup tool. Pre-release; not affiliated with GitHub.</p><p>Configuration is local. Client authentication stays in each official client.</p></div></section>',
      activity: renderActivity,
      settings: renderSettings,
    };
    content.innerHTML = (!isDesktop && !["overview", "usage"].includes(route) ? '<div class="preview-banner">Browser preview · disposable sample data · no native computer access</div>' : "") + renderers[route]();
    content.dataset.route = route;
    window.PilotWeaveInstaller.mount();
    window.PilotWeaveAccount.mount();
    window.PilotWeaveGithubAuth.mount();
  }

  function renderOverview() {
    return storageWarningBanner() + window.PilotWeaveSetup.render(snapshot, data, isDesktop);
  }

  function renderConnections() {
    return `
      <div class="security-note">
        <div class="note-icon">◆</div>
        <div>
          <strong>Credentials stay out of PilotWeave state JSON</strong>
          <p>The desktop backend stores API keys in the operating-system credential store. The UI receives only a has-secret flag, and native adapters fetch the secret during apply.</p>
        </div>
      </div>
      ${snapshot.connections.length ? `<div class="card-grid">${snapshot.connections.map(connectionCard).join("")}</div>` : emptyConnections()}
    `;
  }

  function renderClients() {
    return `<section id="installation-panel" class="install-panel"></section><section id="account-panel" class="account-panel"></section>
      <div class="security-note">
        <div class="note-icon">◎</div>
        <div>
          <strong>Capabilities are explicit per target</strong>
          <p>Read-only does not mean broken. It means PilotWeave detected the client but will not mutate an unsupported private store.</p>
        </div>
      </div>
      <div class="client-grid">${snapshot.clients.map(clientCard).join("")}</div>
    `;
  }

  function renderActivity() {
    const entries = [
      ...snapshot.deployments.map((r) => ({ time: r.createdAt, kind: "Deployment / rollback", source: r.targetId, status: r.status, detail: r.detail, route: "connections" })),
      ...(data.setup?.accounts?.loginRuns ?? []).map((r) => ({ time: r.finishedAt ?? r.startedAt, kind: "Official sign-in", source: r.requestedSurfaces.join(", "), status: r.status, detail: r.summary, route: "clients" })),
      ...(data.runs ?? []).map((r) => ({ time: r.finishedAt ?? r.startedAt, kind: "Import / refresh", source: r.sourceId, status: r.status, detail: r.detail, route: "usage" })),
    ].sort((a, b) => Date.parse(b.time) - Date.parse(a.time));
    return `<section class="usage-panel"><div class="step-heading"><h2>Recent activity</h2><span>Local, redacted history</span></div>${data.errors.runs ? `<p class="inline-error">${escapeHtml(data.errors.runs)}</p>` : ""}
      ${entries.length ? `<div class="usage-table-wrap"><table class="usage-table"><thead><tr><th>Time</th><th>Operation</th><th>Target / source</th><th>Status</th><th>Detail</th><th>Next step</th></tr></thead><tbody>${entries.slice(0, 200).map((r) => `<tr><td>${escapeHtml(formatDate(r.time))}</td><td>${escapeHtml(r.kind)}</td><td>${escapeHtml(r.source)}</td><td>${escapeHtml(window.PilotWeaveUsage.label(r.status))}</td><td>${escapeHtml(r.detail)}</td><td><button class="button ghost small" data-action="route" data-route="${r.route}">Review</button></td></tr>`).join("")}</tbody></table></div>` : '<p class="usage-empty">No recorded activity yet.</p>'}</section>`;
  }

  function renderSettings() {
    return `<section id="github-authorization-panel"></section>
      <div class="settings-list">
        ${settingRow("Runtime", isDesktop ? "Tauri native backend" : "Browser preview", isDesktop ? "Client configuration writes can be applied after preview." : "All deployment applies are simulated and remain inside this tab.")}
        ${settingRow("State file", snapshot.statePath, "Non-secret connections, deployments, and schema version.")}
        ${settingRow("State schema", `v${snapshot.version}`, "Migrations are rejected when state is newer than the running build.")}
        ${settingRow("State recovery", snapshot.stateRecovery ? "Read-only recovery" : "Healthy", snapshot.stateRecovery ?? "The primary state file loaded successfully; writes are enabled.")}
        ${settingRow("Usage database", usageDbLabel(), snapshot.usageDb?.detail ?? "Usage storage status was not reported.")}
        ${settingRow("Credential storage", isDesktop ? "Operating-system credential store" : "Simulated has-secret flag", "Secrets are never returned by get_dashboard.")}
        ${settingRow("GitHub Copilot app", "Read-only provider adapter", "Detection is implemented; private provider-store mutation is intentionally disabled.")}
      </div>`;
  }

  function usageDbLabel() {
    const db = snapshot.usageDb;
    if (!db) return "Unknown";
    return db.state === "available"
      ? `Ready (schema v${db.schemaVersion})`
      : "Unavailable";
  }

  function storageWarningBanner() {
    const warnings = [];
    if (snapshot.deploymentRecovery) warnings.push(snapshot.deploymentRecovery);
    if (snapshot.stateRecovery) {
      warnings.push(
        `Connection storage is in read-only recovery: ${snapshot.stateRecovery}`,
      );
    }
    if (snapshot.usageDb && snapshot.usageDb.state !== "available") {
      warnings.push(`Usage database is unavailable: ${snapshot.usageDb.detail}`);
    }
    if (!warnings.length) return "";
    return `<div class="security-note">
      <div class="note-icon">⚠</div>
      <div>
        <strong>Storage needs attention</strong>
        ${warnings.map((warning) => `<p>${escapeHtml(warning)}</p>`).join("")}
        ${snapshot.deploymentRecovery ? '<button class="button" data-action="preview-recovery">Review safe recovery</button>' : ""}
      </div>
    </div>`;
  }

  function statCard(label, value) {
    return `<div class="stat-card"><span>${escapeHtml(label)}</span><strong>${escapeHtml(value)}</strong></div>`;
  }

  function settingRow(title, value, copy) {
    return `<div class="setting-row"><div><h3>${escapeHtml(title)}</h3><p>${escapeHtml(copy)}</p></div><div class="setting-value" title="${escapeHtml(value)}">${escapeHtml(value)}</div></div>`;
  }

  function clientCard(client) {
    return `
      <article class="client-card">
        <div class="client-header">
          <div class="client-icon">${escapeHtml(clientIcon(client.kind))}</div>
          <span class="status-pill ${escapeHtml(client.status)}">${escapeHtml(statusLabel(client.status))}</span>
        </div>
        <h3>${escapeHtml(client.name)}</h3>
        <p>${escapeHtml(client.detail)}</p>
        <div class="path-line" title="${escapeHtml(client.path ?? client.diagnostic ?? "")}">${escapeHtml(client.path ?? client.diagnostic ?? "No local path")}</div>
      </article>`;
  }

  function connectionCard(connection) {
    const enabled = connection.models.filter((item) => item.enabled);
    return `
      <article class="card">
        <div class="card-header">
          <div class="connection-title">
            <div class="connection-avatar">${escapeHtml(initials(connection.name))}</div>
            <div><h3>${escapeHtml(connection.name)}</h3><span class="muted" style="font-size:10px">${escapeHtml(connection.providerKind)} · ${escapeHtml(connection.protocol)}</span></div>
          </div>
          <span class="badge ${connection.hasSecret ? "secure" : ""}">${snapshot.credentialStatuses?.[connection.id]?.state === "unavailable" ? "Credential store unavailable" : connection.hasSecret ? "Credential stored" : "No credential"}</span>
        </div>
        <div class="endpoint" title="${escapeHtml(connection.baseUrl)}">${escapeHtml(connection.baseUrl)}</div>
        <div class="model-list">
          ${enabled.slice(0, 4).map((item) => `<span class="model-chip">${escapeHtml(item.name)}</span>`).join("")}
          ${enabled.length > 4 ? `<span class="model-chip">+${enabled.length - 4}</span>` : ""}
        </div>
        <div class="card-actions">
          <button class="button ghost small" data-action="edit-connection" data-id="${escapeHtml(connection.id)}">Edit</button>
          <button class="button primary small" data-action="deploy-connection" data-id="${escapeHtml(connection.id)}">Deploy</button>
          <button class="button danger small" data-action="delete-connection" data-id="${escapeHtml(connection.id)}">Delete</button>
        </div>
      </article>`;
  }

  function emptyConnections() {
    return `
      <div class="empty-state">
        <div class="empty-icon">◇</div>
        <h2>No connections configured</h2>
        <p>Add an OpenAI-compatible, Azure, Anthropic, local, or custom endpoint. A single connection can then be projected differently into each Copilot client.</p>
        <button class="button primary" data-action="add-connection">Add connection</button>
      </div>`;
  }

  function openModal({ title, body, footer = "", wide = false, onOpen }) {
    modalRoot.innerHTML = `
      <div class="modal-backdrop" role="presentation">
        <section class="modal ${wide ? "wide" : ""}" role="dialog" aria-modal="true" aria-label="${escapeHtml(title)}">
          <header class="modal-header"><h2>${escapeHtml(title)}</h2><button class="icon-button" data-modal-close aria-label="Close">×</button></header>
          <div class="modal-body">${body}</div>
          ${footer ? `<footer class="modal-footer">${footer}</footer>` : ""}
        </section>
      </div>`;
    const backdrop = modalRoot.querySelector(".modal-backdrop");
    backdrop.addEventListener("click", (event) => {
      if (event.target === backdrop) closeModal();
    });
    modalRoot.querySelectorAll("[data-modal-close]").forEach((button) => button.addEventListener("click", closeModal));
    onOpen?.(modalRoot);
  }

  function closeModal() {
    modalRoot.innerHTML = "";
  }

  function openConnectionForm(connection = null) {
    if (snapshot.stateRecovery || snapshot.deploymentRecovery) {
      showToast("Connection changes are disabled during read-only recovery", "error");
      return;
    }
    const modelsText = connection?.models
      ?.map((item) => `${item.modelId} | ${item.name}`)
      .join("\n") ?? "";
    const headersText = connection
      ? Object.entries(connection.headers)
          .map(([name, value]) => `${name}: ${value}`)
          .join("\n")
      : "";
    openModal({
      title: connection ? `Edit ${connection.name}` : "Add connection",
      wide: true,
      body: `
        <form id="connection-form" class="form-grid">
          <input type="hidden" name="id" value="${escapeHtml(connection?.id ?? "")}" />
          <div class="form-field">
            <label for="connection-name">Name</label>
            <input id="connection-name" name="name" required value="${escapeHtml(connection?.name ?? "")}" placeholder="OpenRouter" />
          </div>
          <div class="form-field">
            <label for="provider-kind">Provider kind</label>
            <select id="provider-kind" name="providerKind">
              ${selectOptions(["openai", "azure", "anthropic", "local", "custom"], connection?.providerKind ?? "openai")}
            </select>
          </div>
          <div class="form-field full">
            <label for="base-url">Endpoint URL</label>
            <input id="base-url" name="baseUrl" type="url" required value="${escapeHtml(connection?.baseUrl ?? "")}" placeholder="https://api.example.com/v1" />
            <small>Use the provider base or terminal inference endpoint expected by the selected client adapter.</small>
          </div>
          <div class="form-field">
            <label for="protocol">Wire protocol</label>
            <select id="protocol" name="protocol">
              ${selectOptions(["chat-completions", "responses", "messages"], connection?.protocol ?? "chat-completions")}
            </select>
          </div>
          <div class="form-field">
            <label for="api-key">API key</label>
            <input id="api-key" name="apiKey" type="password" autocomplete="new-password" placeholder="${connection?.hasSecret ? "Leave blank to keep current credential" : "Optional for local endpoints"}" />
            <small>The desktop backend stores this in the OS credential store.</small>
          </div>
          ${connection ? `<div class="form-field full"><label><input name="clearSecret" type="checkbox" style="width:auto;height:auto;margin-right:7px" />Remove the stored credential</label></div>` : ""}
          <div class="form-field full">
            <label for="models">Models</label>
            <textarea id="models" name="models" required placeholder="model-id | Display Name">${escapeHtml(modelsText)}</textarea>
            <small>One model per line. Display name is optional: <code>upstream/model-id | Friendly name</code>.</small>
          </div>
          <div class="form-field full">
            <label for="headers">Request headers</label>
            <textarea id="headers" name="headers" placeholder="X-Custom-Header: value&#10;Authorization: Bearer \${apiKey}">${escapeHtml(headersText)}</textarea>
            <small>One header per line. Use <code>\${apiKey}</code> as a deployment-time placeholder.</small>
          </div>
        </form>`,
      footer: `<button class="button ghost" data-modal-close>Cancel</button><button class="button primary" id="save-connection">${connection ? "Save changes" : "Add connection"}</button>`,
      onOpen(root) {
        root.querySelectorAll("[data-modal-close]").forEach((button) =>
          button.addEventListener("click", closeModal),
        );
        root.querySelector("#save-connection").addEventListener("click", async () => {
          const form = root.querySelector("#connection-form");
          if (!form.reportValidity()) return;
          const data = new FormData(form);
          try {
            const input = parseConnectionForm(data, connection);
            await invoke("upsert_connection", { input });
            closeModal();
            showToast(connection ? "Connection updated" : "Connection added");
            await refresh();
          } catch (error) {
            showToast(error?.message ?? String(error), "error");
          }
        });
      },
    });
  }

  function selectOptions(values, selected) {
    return values
      .map(
        (value) => `<option value="${escapeHtml(value)}" ${value === selected ? "selected" : ""}>${escapeHtml(value)}</option>`,
      )
      .join("");
  }

  function parseConnectionForm(data, existing) {
    const modelLines = String(data.get("models") ?? "")
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
    if (!modelLines.length) throw new Error("Add at least one model");
    const existingByModelId = Object.fromEntries(
      (existing?.models ?? []).map((item) => [item.modelId, item]),
    );
    const models = modelLines.map((line) => {
      const [rawId, ...nameParts] = line.split("|");
      const modelId = rawId.trim();
      if (!modelId) throw new Error(`Invalid model line: ${line}`);
      const name = nameParts.join("|").trim() || modelId;
      return {
        id: existingByModelId[modelId]?.id ?? crypto.randomUUID(),
        modelId,
        name,
        enabled: existingByModelId[modelId]?.enabled ?? true,
        capabilities: existingByModelId[modelId]?.capabilities ?? {
          toolCalling: true,
          vision: null,
          reasoning: null,
          contextWindow: null,
          maxOutputTokens: null,
        },
      };
    });
    const headers = {};
    String(data.get("headers") ?? "")
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .forEach((line) => {
        const index = line.indexOf(":");
        if (index <= 0) throw new Error(`Header must contain a colon: ${line}`);
        const name = line.slice(0, index).trim();
        const value = line.slice(index + 1).trim();
        headers[name] = value;
      });
    const id = String(data.get("id") ?? "").trim();
    return {
      id: id || null,
      name: String(data.get("name") ?? ""),
      baseUrl: String(data.get("baseUrl") ?? ""),
      providerKind: String(data.get("providerKind") ?? "openai"),
      protocol: String(data.get("protocol") ?? "chat-completions"),
      headers,
      models,
      apiKey: String(data.get("apiKey") ?? "") || null,
      clearSecret: data.get("clearSecret") === "on",
    };
  }

  function openDeleteConfirmation(connection) {
    if (snapshot.stateRecovery || snapshot.deploymentRecovery) {
      showToast("Connection changes are disabled during read-only recovery", "error");
      return;
    }
    openModal({
      title: "Delete connection",
      body: `<p class="muted" style="line-height:1.7">Delete <strong style="color:var(--text)">${escapeHtml(connection.name)}</strong>, its local deployment history, and its credential-store entry? Native client configuration is not automatically removed in this MVP.</p>`,
      footer: `<button class="button ghost" data-modal-close>Cancel</button><button class="button danger" id="confirm-delete">Delete connection</button>`,
      onOpen(root) {
        root.querySelectorAll("[data-modal-close]").forEach((button) =>
          button.addEventListener("click", closeModal),
        );
        root.querySelector("#confirm-delete").addEventListener("click", async () => {
          try {
            const result = await invoke("delete_connection", { connectionId: connection.id });
            closeModal();
            showToast(result?.credentialCleanupWarning ?? "Connection deleted", result?.credentialCleanupWarning ? "warning" : "success");
            await refresh();
          } catch (error) {
            showToast(error?.message ?? String(error), "error");
          }
        });
      },
    });
  }

  function openDeployment(connection) {
    if (snapshot.stateRecovery || snapshot.deploymentRecovery) {
      showToast("Managed writes are disabled during read-only recovery", "error");
      return;
    }
    const targets = snapshot.clients;
    openModal({
      title: `Deploy ${connection.name}`,
      wide: true,
      body: `
        <p class="muted" style="font-size:11px;line-height:1.6">Review the concrete targets before confirming. This exact preview expires after 15 minutes and can be applied once. Changed state requires a new preview.</p>
        <details><summary>Review individual targets and VS Code profiles</summary><div class="target-list">
          ${targets
            .map(
              (target) => `<div class="target-option">
                <input id="target-${escapeHtml(target.id)}" type="checkbox" name="deployment-target" value="${escapeHtml(target.id)}" ${target.detected && target.supportsWrite ? "checked" : target.detected ? "" : "disabled"} />
                <label for="target-${escapeHtml(target.id)}"><strong>${escapeHtml(target.name)}</strong><small>${escapeHtml(target.detail)} · ${escapeHtml(target.supportsWrite ? "writable" : "manual/read-only")}</small></label>
                <span class="status-pill ${escapeHtml(target.status)}">${escapeHtml(statusLabel(target.status))}</span>
              </div>`,
            )
            .join("")}
        </div>
        </details><hr class="preview-separator" />
        <div id="deployment-preview"><p class="muted" style="font-size:11px">Preview has not been generated.</p></div>`,
      footer: `<button class="button ghost" data-modal-close>Cancel</button><button class="button primary" id="preview-deployment">Preview changes</button>`,
      onOpen(root) {
        root.querySelectorAll("[data-modal-close]").forEach((button) =>
          button.addEventListener("click", closeModal),
        );
        const actionButton = root.querySelector("#preview-deployment");
        const review = window.PilotWeaveDeploymentReview.createSession(invoke);
        root.querySelectorAll('input[name="deployment-target"]').forEach((input) => {
          input.addEventListener("change", () => {
            review.invalidate();
            actionButton.textContent = "Preview changes";
            root.querySelector("#deployment-preview").textContent = "Selection changed. Generate a new preview.";
          });
        });
        actionButton.addEventListener("click", async () => {
          if (review.busy) return;
          actionButton.disabled = true;
          try {
            if (!review.plan) {
              const selectedTargetIds = Array.from(
                root.querySelectorAll('input[name="deployment-target"]:checked'),
              ).map((input) => input.value);
              if (!selectedTargetIds.length) {
                showToast("Select at least one target", "error");
                return;
              }
              const plan = await review.preview(connection.id, selectedTargetIds);
              if (!plan) return;
              root.querySelector("#deployment-preview").innerHTML = `
                <div class="operation-list">${plan.operations.map(operationCard).join("")}</div>`;
              actionButton.textContent = "Apply supported changes";
            } else {
              const result = await review.apply();
              closeModal();
              const applied = result.records.filter((item) => item.status === "applied").length;
              const skipped = result.records.filter((item) => item.status === "skipped").length;
              const failed = result.records.filter((item) => item.status === "failed").length;
              showToast(`Deployment complete: ${applied} applied, ${skipped} skipped, ${failed} failed`, failed ? "error" : "success");
              await refresh();
            }
          } catch (error) {
            review.invalidate();
            root.querySelector("#deployment-preview").textContent = "The previous preview cannot be reused. Review a new preview before applying.";
            showToast(error?.message ?? String(error), "error");
          } finally {
            actionButton.disabled = false;
            actionButton.textContent = review.plan ? "Apply supported changes" : "Preview changes";
          }
        });
      },
    });
  }


  function openAccountConfirmation() {
    if (!isDesktop || !data.setup) return;
    const evidenceFingerprint = data.setup.evidenceFingerprint;
    openModal({ title: "Confirm the same GitHub account", body: `
      <p>Check the account displayed in VS Code Copilot, Copilot CLI, and the Copilot app. This records your confirmation separately from automatic verification.</p>
      <label class="setup-select">GitHub login<input id="confirmed-login" maxlength="39" value="${escapeHtml(data.setup.target?.login ?? "")}" placeholder="Your github.com login" /></label>
      <p class="muted">PilotWeave uses GitHub's public user API to resolve the stable account ID. No Billing authorization is required.</p>
      <label class="account-confirmation"><input type="checkbox" id="confirm-all-accounts" /><span>I checked that all three clients display this same github.com account.</span></label>`,
      footer: '<button class="button ghost" data-modal-close>Cancel</button><button class="button primary" id="save-account-confirmation" disabled>Save my confirmation</button>',
      onOpen(root) {
        const button = root.querySelector("#save-account-confirmation");
        root.querySelector("#confirm-all-accounts").addEventListener("change", (e) => { button.disabled = !e.target.checked; });
        button.addEventListener("click", async () => {
          button.disabled = true;
          try { await invoke("confirm_account_alignment", { login: root.querySelector("#confirmed-login").value.trim(), evidenceFingerprint, confirmed: true }); closeModal(); await refresh(); }
          catch (error) { showToast(error?.message ?? String(error), "error"); button.disabled = false; }
        });
      },
    });
  }
  function openManualSetup() {
    const connection = window.PilotWeaveSetup.derive(snapshot, data, isDesktop).selected;
    if (!connection) { openConnectionForm(); return; }
    openModal({ title: "Set up the Copilot app provider", body: `
      <ol class="manual-steps"><li>Open the Copilot app and its <strong>Model providers</strong> settings.</li>
      <li>Add <strong>${escapeHtml(connection.name)}</strong>, endpoint <code>${escapeHtml(connection.baseUrl)}</code>, and model <code>${escapeHtml(connection.models.find((m) => m.enabled)?.modelId ?? "Unknown")}</code>. Enter your provider key in the app if requested.</li>
      <li>Select the provider and model in the app, then verify they work.</li></ol><p>PilotWeave cannot verify or automatically change the app's private provider configuration. Completion here is your manual confirmation.</p>`,
      footer: `<button class="button ghost" data-modal-close>Close</button><button class="button primary" id="confirm-manual" ${!isDesktop ? "disabled" : ""}>I completed manual setup</button>`,
      onOpen(root) { root.querySelector("#confirm-manual").addEventListener("click", async (event) => {
        event.target.disabled = true;
        try { await invoke("confirm_manual_provider", { connectionId: connection.id, confirmed: true }); closeModal(); await refresh(); }
        catch (error) { showToast(error?.message ?? String(error), "error"); event.target.disabled = false; }
      }); },
    });
  }
  async function openRecovery(action = "restore") {
    if (!isDesktop) return;
    try {
      const plan = await invoke("preview_deployment_recovery", { action });
      const keepCurrent = plan.action === "keepCurrent";
      const blocked = !keepCurrent && !plan.view.committed && (plan.view.conflictCount > 0 || plan.view.unknownResourceCount > 0 || plan.view.unreadableResourceCount > 0);
      openModal({
        title: keepCurrent ? "Review keeping current files" : "Review interrupted deployment recovery",
        body: keepCurrent
          ? `<p>Keep all ${plan.view.resourceCount} recorded resources exactly as they are. This removes the pending rollback journal and ends the deployment write block.</p><p><strong>The pending changes cannot be rolled back by PilotWeave after this operation.</strong> This does not verify client configuration or mark the deployment successful. Review the current client setup before deploying again.</p><label><input id="confirm-recovery" type="checkbox" style="width:auto" />I reviewed this choice and accept keeping the current files and discarding pending rollback data.</label>`
          : `<p>${plan.view.resourceCount} physical resources were recorded; ${plan.view.conflictCount} have external changes, ${plan.view.unknownResourceCount ?? 0} are no longer detected or cannot be located, and ${plan.view.unreadableResourceCount ?? 0} cannot be read safely.</p><p>${plan.view.committed ? "The audit was committed. This only removes the completed journal." : blocked ? "Automatic restoration is blocked. Resolve the affected targets and preview again, or separately review keeping all current files to end recovery." : "Unchanged PilotWeave writes will be restored. A new external edit will stop safe restoration and retain the journal for another review."}</p><label><input id="confirm-recovery" type="checkbox" style="width:auto" ${blocked ? "disabled" : ""} />I reviewed and approve this recovery operation.</label>`,
        footer: `<button class="button ghost" data-modal-close>Cancel</button>${!keepCurrent && !plan.view.committed ? '<button class="button ghost" id="review-keep-current">Review keeping current files…</button>' : ""}<button class="button primary" id="apply-recovery" disabled>${keepCurrent ? "Keep files and end recovery" : "Recover"}</button>`,
        onOpen(root) {
          const button = root.querySelector("#apply-recovery");
          root.querySelector("#confirm-recovery").addEventListener("change", (event) => { button.disabled = blocked || !event.target.checked; });
          root.querySelector("#review-keep-current")?.addEventListener("click", async (event) => {
            event.target.disabled = true;
            await openRecovery("keepCurrent");
          });
          button.addEventListener("click", async () => {
            button.disabled = true;
            try {
              await invoke("apply_deployment_recovery", { planId: plan.id, confirmed: true });
              closeModal();
              showToast(keepCurrent ? "Current files retained; review client setup before deploying again" : "Recovery completed; original audit may require review");
              await refresh();
            } catch (error) {
              closeModal();
              showToast(`${error?.message ?? error} Preview again before retrying.`, "error");
              await refresh();
            }
          });
        },
      });
    } catch (error) { showToast(error?.message ?? String(error), "error"); }
  }


  function operationCard(operation) {
    return `<article class="operation-card">
      <div class="card-header"><div><strong>${escapeHtml(operation.title)}</strong><p>${escapeHtml(operation.description)}</p></div><span class="status-pill ${operation.supported ? "available" : "read-only"}">${operation.supported ? "Will apply" : "Manual"}</span></div>
      <ul>${operation.changes.map((change) => `<li>${escapeHtml(change)}</li>`).join("")}</ul>
      ${operation.requiresRestart ? '<p style="margin-top:9px;color:var(--warning)">A new terminal or client process is required.</p>' : ""}
    </article>`;
  }

  document.querySelector("#primary-nav").addEventListener("click", (event) => {
    const button = event.target.closest("[data-route]");
    if (button) setRoute(button.dataset.route);
  });

  content.addEventListener("click", (event) => {
    const action = event.target.closest("[data-action]");
    if (!action) return;
    const connection = snapshot?.connections.find((item) => item.id === action.dataset.id);
    switch (action.dataset.action) {
      case "setup-install":
        window.PilotWeaveInstaller.preview((data.components ?? []).filter((c) => ["missing", "broken"].includes(c.status)).map((c) => c.id)); break;
      case "setup-signin": window.PilotWeaveAccount.preview(); break;
      case "setup-accounts": openAccountConfirmation(); break;
      case "setup-deploy": {
        const selected = window.PilotWeaveSetup.derive(snapshot, data, isDesktop).selected;
        if (selected) openDeployment(selected); else openConnectionForm(); break;
      }
      case "setup-manual": openManualSetup(); break;
      case "preview-recovery": openRecovery(); break;
      case "add-connection":
        openConnectionForm();
        break;
      case "edit-connection":
        if (connection) openConnectionForm(connection);
        break;
      case "deploy-connection":
        if (connection) openDeployment(connection);
        break;
      case "delete-connection":
        if (connection) openDeleteConfirmation(connection);
        break;
      case "route":
        setRoute(action.dataset.route);
        break;
      case "refresh":
        refresh();
        break;
    }
  });

  addConnectionButton.addEventListener("click", () => openConnectionForm());
  refreshButton.addEventListener("click", refresh);

  content.addEventListener("change", async (event) => {
    if (event.target.id !== "setup-connection") return;
    try { await invoke("select_setup_connection", { connectionId: event.target.value }); await refresh(); }
    catch (error) { showToast(error?.message ?? String(error), "error"); }
  });
  document.addEventListener("pilotweave:refresh-setup", () => refresh());
  window.PilotWeaveUsage.configure({ invoke, refresh: refreshUsage, render, showToast, openModal, closeModal, getData: () => data, native: isDesktop });
  refresh();
})();
