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
      title: "Overview",
      subtitle: "One model setup across every Copilot surface.",
      add: true,
    },
    connections: {
      title: "Connections",
      subtitle: "Client-neutral provider endpoints, credentials, and model catalogs.",
      add: true,
    },
    clients: {
      title: "Clients",
      subtitle: "Detected Copilot surfaces and their deployment capabilities.",
      add: false,
    },
    resources: {
      title: "Resources",
      subtitle: "Shared Skills, MCP servers, and instructions are the next milestone.",
      add: false,
    },
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
        return createMockPlan(connection, targets.filter(Boolean));
      }
      case "apply_deployment": {
        const connection = demoState.connections.find(
          (item) => item.id === args.connectionId,
        );
        if (!connection) throw new Error("Unknown connection");
        const targets = args.targetIds
          .map((id) => demoState.clients.find((item) => item.id === id))
          .filter(Boolean);
        const plan = createMockPlan(connection, targets);
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
        throw new Error(`Unsupported browser-preview command: ${command}`);
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
      snapshot = await invoke("get_dashboard");
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
    const renderers = {
      overview: renderOverview,
      connections: renderConnections,
      clients: renderClients,
      resources: renderResources,
      activity: renderActivity,
      settings: renderSettings,
    };
    content.innerHTML = renderers[route]();
  }

  function renderOverview() {
    const connections = snapshot.connections;
    const clients = snapshot.clients;
    const detectedClients = clients.filter((client) => client.detected).length;
    const writableClients = clients.filter(
      (client) => client.detected && client.supportsWrite,
    ).length;
    const modelCount = connections.reduce(
      (total, connection) =>
        total + connection.models.filter((item) => item.enabled).length,
      0,
    );
    return `
      ${storageWarningBanner()}
      <div class="hero">
        <div class="hero-copy">
          <p class="eyebrow">DESIRED STATE, NOT ANOTHER SWITCHER</p>
          <h2>Connect a provider once, then deploy it safely to every Copilot surface.</h2>
          <p>PilotWeave keeps the source of truth client-neutral. VS Code profiles, Copilot CLI, and the GitHub Copilot app are deployment targets with explicit capabilities and safety boundaries.</p>
          <div class="inline-actions">
            <button class="button primary" data-action="add-connection">Add your first connection</button>
            <button class="button ghost" data-action="route" data-route="clients">Review detected clients</button>
          </div>
        </div>
        <div class="hero-flow" aria-label="PilotWeave data flow">
          <div class="flow-node"><span>Connections</span><strong>${connections.length}</strong></div>
          <div class="flow-arrow"></div>
          <div class="flow-node"><span>Model catalog</span><strong>${modelCount} enabled</strong></div>
          <div class="flow-arrow"></div>
          <div class="flow-node"><span>Client deployments</span><strong>${writableClients} writable</strong></div>
        </div>
      </div>

      <div class="stats-grid">
        ${statCard("Connections", connections.length)}
        ${statCard("Enabled models", modelCount)}
        ${statCard("Detected clients", detectedClients)}
        ${statCard("Deployment records", snapshot.deployments.length)}
      </div>

      <div class="section-heading">
        <div><h2>Client surfaces</h2><p class="section-copy">What PilotWeave can see and safely manage on this machine.</p></div>
        <button class="button ghost small" data-action="route" data-route="clients">View all</button>
      </div>
      <div class="client-grid">${clients.slice(0, 3).map(clientCard).join("")}</div>

      <div class="section-heading">
        <div><h2>Connections</h2><p class="section-copy">Reusable endpoints and model catalogs.</p></div>
        <button class="button ghost small" data-action="route" data-route="connections">Manage</button>
      </div>
      ${connections.length ? `<div class="card-grid">${connections.slice(0, 4).map(connectionCard).join("")}</div>` : emptyConnections()}
    `;
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
    return `
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

  function renderResources() {
    return `
      <div class="milestone-panel">
        <div class="empty-icon">✦</div>
        <p class="eyebrow">NEXT MILESTONE</p>
        <h2>Shared Skills, MCP, and Instructions</h2>
        <p>The product shell already reserves this domain, but the MVP does not claim resource deployment is complete. The next implementation will model resources once and bind them to VS Code profiles, Copilot CLI, the GitHub Copilot app, and repositories.</p>
        <div class="badges" style="justify-content:center">
          <span class="badge">Skills</span><span class="badge">MCP</span><span class="badge">Instructions</span><span class="badge">Repository scope</span>
        </div>
      </div>`;
  }

  function renderActivity() {
    if (!snapshot.deployments.length) {
      return `
        <div class="empty-state">
          <div class="empty-icon">↗</div>
          <h2>No deployment activity yet</h2>
          <p>Preview and apply a connection to create a local, secret-free audit record.</p>
          <button class="button primary" data-action="route" data-route="connections">Open connections</button>
        </div>`;
    }
    const connectionById = Object.fromEntries(
      snapshot.connections.map((connection) => [connection.id, connection]),
    );
    const clientById = Object.fromEntries(
      snapshot.clients.map((client) => [client.id, client]),
    );
    return `
      <div class="table-wrap">
        <table>
          <thead><tr><th>Time</th><th>Connection</th><th>Target</th><th>Status</th><th>Detail</th></tr></thead>
          <tbody>
            ${snapshot.deployments
              .map(
                (record) => `<tr>
                  <td>${escapeHtml(formatDate(record.createdAt))}</td>
                  <td>${escapeHtml(connectionById[record.connectionId]?.name ?? record.connectionId)}</td>
                  <td>${escapeHtml(clientById[record.targetId]?.name ?? record.targetId)}</td>
                  <td><span class="status-pill ${escapeHtml(record.status)}">${escapeHtml(statusLabel(record.status))}</span></td>
                  <td>${escapeHtml(record.detail)}</td>
                </tr>`,
              )
              .join("")}
          </tbody>
        </table>
      </div>`;
  }

  function renderSettings() {
    return `
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
          <span class="badge ${connection.hasSecret ? "secure" : ""}">${connection.hasSecret ? "Credential stored" : "No credential"}</span>
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
    modalRoot.querySelector("[data-modal-close]")?.addEventListener("click", closeModal);
    onOpen?.(modalRoot);
  }

  function closeModal() {
    modalRoot.innerHTML = "";
  }

  function openConnectionForm(connection = null) {
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
            await invoke("delete_connection", { connectionId: connection.id });
            closeModal();
            showToast("Connection deleted");
            await refresh();
          } catch (error) {
            showToast(error?.message ?? String(error), "error");
          }
        });
      },
    });
  }

  function openDeployment(connection) {
    const targets = snapshot.clients;
    openModal({
      title: `Deploy ${connection.name}`,
      wide: true,
      body: `
        <p class="muted" style="font-size:11px;line-height:1.6">Select concrete client targets. PilotWeave will rebuild the plan again immediately before applying it.</p>
        <div class="target-list">
          ${targets
            .map(
              (target) => `<div class="target-option">
                <input id="target-${escapeHtml(target.id)}" type="checkbox" name="deployment-target" value="${escapeHtml(target.id)}" ${target.detected ? "checked" : "disabled"} />
                <label for="target-${escapeHtml(target.id)}"><strong>${escapeHtml(target.name)}</strong><small>${escapeHtml(target.detail)} · ${escapeHtml(target.supportsWrite ? "writable" : "manual/read-only")}</small></label>
                <span class="status-pill ${escapeHtml(target.status)}">${escapeHtml(statusLabel(target.status))}</span>
              </div>`,
            )
            .join("")}
        </div>
        <hr class="preview-separator" />
        <div id="deployment-preview"><p class="muted" style="font-size:11px">Preview has not been generated.</p></div>`,
      footer: `<button class="button ghost" data-modal-close>Cancel</button><button class="button primary" id="preview-deployment">Preview changes</button>`,
      onOpen(root) {
        root.querySelectorAll("[data-modal-close]").forEach((button) =>
          button.addEventListener("click", closeModal),
        );
        const actionButton = root.querySelector("#preview-deployment");
        let selectedTargetIds = [];
        let plan = null;
        actionButton.addEventListener("click", async () => {
          try {
            if (!plan) {
              selectedTargetIds = Array.from(
                root.querySelectorAll('input[name="deployment-target"]:checked'),
              ).map((input) => input.value);
              if (!selectedTargetIds.length) {
                showToast("Select at least one target", "error");
                return;
              }
              plan = await invoke("preview_deployment", {
                connectionId: connection.id,
                targetIds: selectedTargetIds,
              });
              root.querySelector("#deployment-preview").innerHTML = `
                <div class="operation-list">${plan.operations.map(operationCard).join("")}</div>`;
              actionButton.textContent = "Apply supported changes";
            } else {
              actionButton.disabled = true;
              const result = await invoke("apply_deployment", {
                connectionId: connection.id,
                targetIds: selectedTargetIds,
              });
              closeModal();
              const applied = result.records.filter((item) => item.status === "applied").length;
              const skipped = result.records.filter((item) => item.status === "skipped").length;
              const failed = result.records.filter((item) => item.status === "failed").length;
              showToast(`Deployment complete: ${applied} applied, ${skipped} skipped, ${failed} failed`, failed ? "error" : "success");
              await refresh();
            }
          } catch (error) {
            actionButton.disabled = false;
            showToast(error?.message ?? String(error), "error");
          }
        });
      },
    });
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

  refresh();
})();
