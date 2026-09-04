(() => {
  "use strict";

  const invokeNative = window.__TAURI__?.core?.invoke ?? null;
  const isDesktop = typeof invokeNative === "function";
  const content = document.querySelector("#content");
  const pageTitle = document.querySelector("#page-title");
  const modalRoot = document.querySelector("#modal-root");
  const toastRoot = document.querySelector("#toast-root");

  if (!content || !pageTitle || !modalRoot || !toastRoot) return;

  const componentOrder = [
    "vscode",
    "vscode-copilot-extension",
    "copilot-cli",
    "github-copilot-app",
  ];

  let observations = null;
  let loading = false;
  let lastResult = null;
  let renderVersion = 0;
  let scheduled = false;

  const browserObservations = [
    {
      id: "vscode",
      name: "Visual Studio Code",
      status: "ready",
      detail: "Browser preview: simulated installed component",
      version: "preview",
    },
    {
      id: "vscode-copilot-extension",
      name: "GitHub Copilot extension",
      status: "ready",
      detail: "Browser preview: simulated installed extension",
      version: "preview",
    },
    {
      id: "copilot-cli",
      name: "GitHub Copilot CLI",
      status: "missing",
      detail: "Browser preview: simulated missing component",
      version: null,
    },
    {
      id: "github-copilot-app",
      name: "GitHub Copilot app",
      status: "missing",
      detail: "Browser preview: simulated missing component",
      version: null,
    },
  ];

  function escapeHtml(value) {
    return String(value ?? "")
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#039;");
  }

  function showToast(message, type = "success") {
    const toast = document.createElement("div");
    toast.className = `toast ${type}`;
    toast.textContent = message;
    toastRoot.append(toast);
    setTimeout(() => toast.remove(), 4200);
  }

  function statusLabel(status) {
    return {
      ready: "Ready",
      missing: "Missing",
      unsupported: "Unsupported",
      broken: "Needs repair",
      completedAndVerified: "Installed and verified",
      processSucceededVerificationFailed: "Verification failed",
      skippedAlreadyReady: "Already ready",
      skippedDependencyFailed: "Dependency unavailable",
      failed: "Failed",
    }[status] ?? status;
  }

  function statusClass(status) {
    return {
      ready: "ready",
      completedAndVerified: "ready",
      skippedAlreadyReady: "ready",
      missing: "missing",
      broken: "broken",
      processSucceededVerificationFailed: "broken",
      failed: "broken",
      skippedDependencyFailed: "broken",
      unsupported: "unsupported",
    }[status] ?? "unsupported";
  }

  function ordered(values) {
    return [...values].sort(
      (left, right) =>
        componentOrder.indexOf(left.id) - componentOrder.indexOf(right.id),
    );
  }

  function isClientsRoute() {
    return pageTitle.textContent?.trim() === "Clients";
  }

  function markChanged() {
    renderVersion += 1;
    ensurePanel();
  }

  function scheduleEnsurePanel() {
    if (scheduled) return;
    scheduled = true;
    queueMicrotask(() => {
      scheduled = false;
      ensurePanel();
    });
  }

  function ensurePanel() {
    if (!isClientsRoute()) return;
    let panel = content.querySelector("#installation-panel");
    if (!panel) {
      panel = document.createElement("section");
      panel.id = "installation-panel";
      panel.className = "install-panel";
      content.prepend(panel);
    }
    if (panel.dataset.renderVersion === String(renderVersion)) return;
    panel.dataset.renderVersion = String(renderVersion);
    panel.innerHTML = renderPanel();
  }

  function renderPanel() {
    const values = observations ? ordered(observations) : [];
    const actionable = values.filter((item) =>
      ["missing", "broken"].includes(item.status),
    );
    const readyCount = values.filter((item) => item.status === "ready").length;
    const unsupportedCount = values.filter(
      (item) => item.status === "unsupported",
    ).length;

    return `
      <div class="install-panel-header">
        <div>
          <p class="eyebrow">WORKSTATION SETUP</p>
          <h2>Install and verify Copilot surfaces</h2>
          <p class="install-copy">PilotWeave builds a native, one-shot plan from a fixed allowlist. The frontend never supplies executable paths, package IDs, or shell commands.</p>
        </div>
        <div class="install-panel-actions">
          <button class="button ghost small" data-install-action="refresh" ${loading ? "disabled" : ""}>${loading ? "Checking…" : "Refresh status"}</button>
          <button class="button primary small" data-install-action="install-all" ${loading || actionable.length === 0 ? "disabled" : ""}>Install all missing${actionable.length ? ` (${actionable.length})` : ""}</button>
        </div>
      </div>
      <div class="install-summary" aria-label="Installation summary">
        <span><strong>${values.length ? readyCount : "—"}</strong> ready</span>
        <span><strong>${values.length ? actionable.length : "—"}</strong> actionable</span>
        <span><strong>${values.length ? unsupportedCount : "—"}</strong> unsupported</span>
        <span class="install-runtime">${isDesktop ? "Native Windows installation is enabled when supported" : "Browser preview only — no process will run"}</span>
      </div>
      ${loading && values.length === 0 ? '<div class="install-loading">Discovering components…</div>' : ""}
      ${!loading && values.length === 0 ? '<div class="install-loading">Installation status has not been loaded.</div>' : ""}
      ${values.length ? `<div class="install-grid">${values.map(renderComponent).join("")}</div>` : ""}
      ${lastResult ? renderLastResult(lastResult) : ""}
    `;
  }

  function renderComponent(item) {
    const canInstall = ["missing", "broken"].includes(item.status);
    return `
      <article class="install-card">
        <div class="install-card-heading">
          <div class="install-component-icon" aria-hidden="true">${componentIcon(item.id)}</div>
          <span class="install-status ${statusClass(item.status)}">${escapeHtml(statusLabel(item.status))}</span>
        </div>
        <h3>${escapeHtml(item.name)}</h3>
        <p>${escapeHtml(item.detail)}</p>
        <div class="install-card-footer">
          <span>${item.version ? `Version ${escapeHtml(item.version)}` : "Version unavailable"}</span>
          ${canInstall ? `<button class="button ghost small" data-install-action="install-one" data-component-id="${escapeHtml(item.id)}" ${loading ? "disabled" : ""}>${item.status === "broken" ? "Repair" : "Install"}</button>` : ""}
        </div>
      </article>
    `;
  }

  function componentIcon(id) {
    return {
      vscode: "VS",
      "vscode-copilot-extension": "GH",
      "copilot-cli": ">_",
      "github-copilot-app": "◆",
    }[id] ?? "◇";
  }

  function renderLastResult(result) {
    const successes = result.results.filter((item) =>
      ["completedAndVerified", "skippedAlreadyReady"].includes(item.status),
    ).length;
    const failures = result.results.length - successes;
    return `
      <div class="install-result-summary">
        <div>
          <strong>Last installation run</strong>
          <span>Plan ${escapeHtml(result.planId)}</span>
        </div>
        <div><strong>${successes}</strong> successful · <strong>${failures}</strong> need attention</div>
      </div>
    `;
  }

  async function invoke(command, args = {}) {
    if (isDesktop) return invokeNative(command, args);
    await new Promise((resolve) => setTimeout(resolve, 120));
    return invokeBrowserMock(command, args);
  }

  function invokeBrowserMock(command, args) {
    switch (command) {
      case "get_installation_status":
        return structuredClone(browserObservations);
      case "preview_install": {
        const requested = args.componentIds?.length
          ? args.componentIds
          : browserObservations
              .filter((item) => ["missing", "broken"].includes(item.status))
              .map((item) => item.id);
        const operations = requested.map((id) => {
          const item = browserObservations.find((value) => value.id === id);
          return {
            id: crypto.randomUUID(),
            componentId: id,
            componentName: item?.name ?? id,
            strategy:
              id === "vscode-copilot-extension"
                ? "vsCodeExtension"
                : "wingetPackage",
            source:
              id === "github-copilot-app"
                ? "WinGet: GitHub.CopilotApp"
                : `Preview allowlist: ${id}`,
            requiresElevation: false,
            description:
              "Browser preview simulation; the native backend owns the real command and arguments",
          };
        });
        const now = new Date();
        return {
          id: crypto.randomUUID(),
          requestedComponentIds: requested,
          operations,
          createdAt: now.toISOString(),
          expiresAt: new Date(now.getTime() + 15 * 60 * 1000).toISOString(),
        };
      }
      case "apply_install_plan": {
        const requested = currentModalPlan?.requestedComponentIds ?? [];
        for (const id of requested) {
          const item = browserObservations.find((value) => value.id === id);
          if (item) {
            item.status = "ready";
            item.detail = "Browser preview: simulated installation completed";
            item.version = "preview";
          }
        }
        return {
          planId: args.planId,
          results: requested.map((id) => ({
            componentId: id,
            status: "completedAndVerified",
            detail:
              "Browser preview simulated a verified result; no process was started",
          })),
          observations: structuredClone(browserObservations),
        };
      }
      default:
        throw new Error(`Unsupported installer preview command: ${command}`);
    }
  }

  async function refreshStatus({ quiet = false } = {}) {
    if (loading) return;
    loading = true;
    markChanged();
    try {
      observations = await invoke("get_installation_status");
      if (!Array.isArray(observations)) {
        throw new Error("Installation discovery returned an invalid response");
      }
      if (!quiet) showToast("Installation status refreshed");
    } catch (error) {
      showToast(error?.message ?? String(error), "error");
    } finally {
      loading = false;
      markChanged();
    }
  }

  let currentModalPlan = null;

  async function previewInstall(componentIds) {
    if (loading || componentIds.length === 0) return;
    loading = true;
    markChanged();
    try {
      const plan = await invoke("preview_install", { componentIds });
      currentModalPlan = plan;
      openPlanModal(plan);
    } catch (error) {
      showToast(error?.message ?? String(error), "error");
    } finally {
      loading = false;
      markChanged();
    }
  }

  function openPlanModal(plan) {
    const operationRows = plan.operations.length
      ? plan.operations
          .map(
            (operation) => `
              <div class="install-plan-operation">
                <div>
                  <strong>${escapeHtml(operation.componentName)}</strong>
                  <span>${escapeHtml(operation.description)}</span>
                </div>
                <div class="install-plan-meta">
                  <span>${escapeHtml(operation.source)}</span>
                  <span>${operation.requiresElevation ? "Elevation may be required" : "User-level operation"}</span>
                </div>
              </div>`,
          )
          .join("")
      : '<div class="install-plan-empty">Every selected component is already ready. No process will run.</div>';

    modalRoot.innerHTML = `
      <div class="modal-backdrop install-modal-backdrop" role="presentation">
        <section class="modal wide install-modal" role="dialog" aria-modal="true" aria-label="Review installation plan">
          <header class="modal-header">
            <div><p class="eyebrow">ONE-SHOT NATIVE PLAN</p><h2>Review installation plan</h2></div>
            <button class="icon-button" data-install-modal-close aria-label="Close">×</button>
          </header>
          <div class="modal-body">
            <div class="install-plan-security">
              Package identities and argument vectors are compiled into the Rust backend. This plan expires at ${escapeHtml(formatDateTime(plan.expiresAt))} and can be consumed once.
            </div>
            <div class="install-plan-list">${operationRows}</div>
          </div>
          <footer class="modal-footer">
            <button class="button ghost" data-install-modal-close>Cancel</button>
            <button class="button primary" data-install-confirm ${plan.operations.length === 0 ? "disabled" : ""}>Install and verify</button>
          </footer>
        </section>
      </div>`;

    modalRoot
      .querySelectorAll("[data-install-modal-close]")
      .forEach((button) => button.addEventListener("click", closeModal));
    modalRoot
      .querySelector(".install-modal-backdrop")
      ?.addEventListener("click", (event) => {
        if (event.target === event.currentTarget) closeModal();
      });
    modalRoot
      .querySelector("[data-install-confirm]")
      ?.addEventListener("click", () => applyPlan(plan));
  }

  function formatDateTime(value) {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return "an unknown time";
    return new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(date);
  }

  function closeModal() {
    modalRoot.innerHTML = "";
    currentModalPlan = null;
  }

  async function applyPlan(plan) {
    const confirm = modalRoot.querySelector("[data-install-confirm]");
    if (confirm) {
      confirm.disabled = true;
      confirm.textContent = "Installing…";
    }
    try {
      const result = await invoke("apply_install_plan", { planId: plan.id });
      lastResult = result;
      observations = result.observations;
      currentModalPlan = null;
      showResultModal(result);
      showToast("Installation run completed");
      markChanged();
    } catch (error) {
    closeModal();
    showToast(
      `${error?.message ?? String(error)} Generate a new preview before retrying.`,
      "error",
    );
  }

  }

  function showResultModal(result) {
    modalRoot.innerHTML = `
      <div class="modal-backdrop install-modal-backdrop" role="presentation">
        <section class="modal wide install-modal" role="dialog" aria-modal="true" aria-label="Installation results">
          <header class="modal-header">
            <div><p class="eyebrow">POST-INSTALL REDISCOVERY</p><h2>Installation results</h2></div>
            <button class="icon-button" data-install-modal-close aria-label="Close">×</button>
          </header>
          <div class="modal-body">
            <div class="install-result-list">
              ${result.results
                .map(
                  (item) => `
                    <div class="install-result-row">
                      <span class="install-status ${statusClass(item.status)}">${escapeHtml(statusLabel(item.status))}</span>
                      <div><strong>${escapeHtml(componentName(item.componentId))}</strong><p>${escapeHtml(item.detail)}</p></div>
                    </div>`,
                )
                .join("")}
            </div>
          </div>
          <footer class="modal-footer">
            <button class="button primary" data-install-modal-close>Done</button>
          </footer>
        </section>
      </div>`;
    modalRoot
      .querySelectorAll("[data-install-modal-close]")
      .forEach((button) => button.addEventListener("click", closeModal));
  }

  function componentName(id) {
    return (
      observations?.find((item) => item.id === id)?.name ??
      browserObservations.find((item) => item.id === id)?.name ??
      id
    );
  }

  document.addEventListener("click", (event) => {
    const button = event.target.closest("[data-install-action]");
    if (!button) return;
    const action = button.dataset.installAction;
    if (action === "refresh") {
      refreshStatus();
      return;
    }
    if (action === "install-one") {
      previewInstall([button.dataset.componentId]);
      return;
    }
    if (action === "install-all") {
      const ids = (observations ?? [])
        .filter((item) => ["missing", "broken"].includes(item.status))
        .map((item) => item.id);
      previewInstall(ids);
    }
  });

  const contentObserver = new MutationObserver(scheduleEnsurePanel);
  contentObserver.observe(content, { childList: true });
  const titleObserver = new MutationObserver(() => {
    if (isClientsRoute()) {
      ensurePanel();
      if (!observations && !loading) refreshStatus({ quiet: true });
    }
  });
  titleObserver.observe(pageTitle, {
    childList: true,
    characterData: true,
    subtree: true,
  });

  if (isClientsRoute()) {
    ensurePanel();
    refreshStatus({ quiet: true });
  }
})();
