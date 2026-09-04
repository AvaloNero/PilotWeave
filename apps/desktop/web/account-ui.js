(() => {
  "use strict";

  const invokeNative = window.__TAURI__?.core?.invoke ?? null;
  const isDesktop = typeof invokeNative === "function";
  const content = document.querySelector("#content");
  const pageTitle = document.querySelector("#page-title");
  const modalRoot = document.querySelector("#modal-root");
  const toastRoot = document.querySelector("#toast-root");

  if (!content || !pageTitle || !modalRoot || !toastRoot) return;

  const surfaceOrder = [
    "vsCodeCopilot",
    "copilotCli",
    "githubCopilotApp",
  ];
  const selectableStates = new Set([
    "verified",
    "inferred",
    "actionRequired",
    "unknown",
    "conflict",
  ]);

  let accountStatus = null;
  let loading = false;
  let currentPlan = null;
  let renderVersion = 0;
  let scheduled = false;

  const browserIdentity = {
    host: "github.com",
    login: "preview-user",
    userId: 1001,
    avatarUrl: null,
  };
  const browserState = {
    anchor: {
      state: "verified",
      identity: browserIdentity,
      evidence: "Browser preview: simulated GitHub API identity",
      detail:
        "Preview data only. The native app queries an official API through a bounded GitHub CLI process.",
      observedAt: new Date().toISOString(),
    },
    surfaces: [
      {
        surface: "vsCodeCopilot",
        state: "actionRequired",
        identity: null,
        evidence: "Browser preview: VS Code SecretStorage was not inspected",
        detail: "Use the official VS Code Accounts interface to select github.com.",
        observedAt: new Date().toISOString(),
      },
      {
        surface: "copilotCli",
        state: "inferred",
        identity: browserIdentity,
        evidence: "Browser preview: simulated GitHub CLI authentication fallback",
        detail: "Verify the selected account inside Copilot CLI with /user.",
        observedAt: new Date().toISOString(),
      },
      {
        surface: "githubCopilotApp",
        state: "notInstalled",
        identity: null,
        evidence: "Browser preview: application not installed",
        detail: "Install the app before starting its official sign-in flow.",
        observedAt: new Date().toISOString(),
      },
    ],
    observedAt: new Date().toISOString(),
    loginRuns: [],
    historyRecovery: null,
  };

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
    let panel = content.querySelector("#account-panel");
    if (!panel) {
      panel = document.createElement("section");
      panel.id = "account-panel";
      panel.className = "account-panel";
      content.prepend(panel);
    }
    const installationPanel = content.querySelector("#installation-panel");
    if (installationPanel && panel.previousElementSibling !== installationPanel) {
      installationPanel.after(panel);
    }
    if (panel.dataset.renderVersion === String(renderVersion)) return;
    panel.dataset.renderVersion = String(renderVersion);
    panel.innerHTML = renderPanel();
  }

  function renderPanel() {
    const status = accountStatus;
    const surfaces = orderedSurfaces(status?.surfaces ?? []);
    const selectable = surfaces.filter((surface) =>
      selectableStates.has(surface.state),
    );
    const verifiedCount = surfaces.filter(
      (surface) => surface.state === "verified",
    ).length;
    const actionCount = surfaces.filter((surface) =>
      ["actionRequired", "unknown", "conflict", "inferred"].includes(
        surface.state,
      ),
    ).length;

    return `
      <div class="account-panel-header">
        <div>
          <p class="eyebrow">ACCOUNT ORCHESTRATION</p>
          <h2>Use official sign-in flows without copying credentials</h2>
          <p class="account-copy">PilotWeave keeps client authentication stores private. It opens fixed official flows, exposes identity evidence, and never reads VS Code SecretStorage, browser cookies, or client OAuth tokens.</p>
        </div>
        <div class="account-panel-actions">
          <button class="button ghost small" data-account-action="refresh" ${loading ? "disabled" : ""}>${loading ? "Checking…" : "Refresh accounts"}</button>
          <button class="button primary small" data-account-action="preview" ${loading || selectable.length === 0 || status?.historyRecovery ? "disabled" : ""}>Sign in and sync${selectable.length ? ` (${selectable.length})` : ""}</button>
        </div>
      </div>
      ${status?.historyRecovery ? renderRecovery(status.historyRecovery) : ""}
      <div class="account-summary" aria-label="Account observation summary">
        <span><strong>${surfaces.length ? verifiedCount : "—"}</strong> verified</span>
        <span><strong>${surfaces.length ? actionCount : "—"}</strong> need review</span>
        <span><strong>${surfaces.length ? surfaces.length : "—"}</strong> surfaces</span>
        <span class="account-runtime">${isDesktop ? "Native account probes and launchers" : "Browser preview only — no sign-in flow will open"}</span>
      </div>
      ${loading && !status ? '<div class="account-loading">Discovering account state…</div>' : ""}
      ${!loading && !status ? '<div class="account-loading">Account status has not been loaded.</div>' : ""}
      ${status ? renderAnchor(status.anchor) : ""}
      ${surfaces.length ? `<div class="account-grid">${surfaces.map(renderSurface).join("")}</div>` : ""}
      ${status ? renderHistory(status.loginRuns ?? []) : ""}
    `;
  }

  function renderRecovery(reason) {
    return `
      <div class="account-recovery">
        <strong>Sign-in history is read-only</strong>
        <span>${escapeHtml(reason)}</span>
      </div>`;
  }

  function renderAnchor(anchor) {
    const identity = anchor?.identity;
    return `
      <article class="account-anchor">
        <div class="account-anchor-heading">
          <div>
            <span class="account-kicker">TARGET ACCOUNT ANCHOR</span>
            <h3>${identity ? `${escapeHtml(identity.login)} · ${escapeHtml(identity.host)}` : "No verified github.com identity"}</h3>
          </div>
          <span class="account-status ${stateClass(anchor?.state)}">${escapeHtml(stateLabel(anchor?.state))}</span>
        </div>
        <p>${escapeHtml(anchor?.detail ?? "No account anchor observation is available.")}</p>
        <div class="account-evidence"><strong>Evidence</strong><span>${escapeHtml(anchor?.evidence ?? "Unavailable")}</span></div>
        <div class="account-meta">
          <span>${identity?.userId != null ? `GitHub user ID ${escapeHtml(identity.userId)}` : "Stable user ID unavailable"}</span>
          <span>Observed ${escapeHtml(formatDateTime(anchor?.observedAt))}</span>
        </div>
      </article>`;
  }

  function renderSurface(surface) {
    const identity = surface.identity;
    const selectable = selectableStates.has(surface.state);
    return `
      <article class="account-card">
        <div class="account-card-heading">
          <label class="account-surface-select">
            <input type="checkbox" name="account-surface" value="${escapeHtml(surface.surface)}" ${selectable ? "checked" : "disabled"} />
            <span class="account-surface-icon" aria-hidden="true">${escapeHtml(surfaceIcon(surface.surface))}</span>
          </label>
          <span class="account-status ${stateClass(surface.state)}">${escapeHtml(stateLabel(surface.state))}</span>
        </div>
        <h3>${escapeHtml(surfaceName(surface.surface))}</h3>
        <p class="account-identity">${identity ? `${escapeHtml(identity.login)} · ${escapeHtml(identity.host)}` : "Identity not verified"}</p>
        <p>${escapeHtml(surface.detail)}</p>
        <div class="account-evidence"><strong>Evidence</strong><span>${escapeHtml(surface.evidence)}</span></div>
        <div class="account-meta"><span>Observed ${escapeHtml(formatDateTime(surface.observedAt))}</span></div>
      </article>`;
  }

  function renderHistory(runs) {
    const recent = runs.slice(0, 5);
    return `
      <div class="account-history-heading">
        <div><h3>Recent sign-in runs</h3><p>Persisted summaries contain statuses and instructions, never client credentials.</p></div>
      </div>
      ${recent.length ? `<div class="account-history">${recent.map(renderRun).join("")}</div>` : '<div class="account-history-empty">No sign-in run has been launched yet.</div>'}
    `;
  }

  function renderRun(run) {
    const target = run.targetIdentity
      ? `${run.targetIdentity.login} · ${run.targetIdentity.host}`
      : "No verified target identity";
    return `
      <div class="account-run">
        <span class="account-status ${runStatusClass(run.status)}">${escapeHtml(runStatusLabel(run.status))}</span>
        <div>
          <strong>${escapeHtml(target)}</strong>
          <p>${escapeHtml(run.summary)}</p>
        </div>
        <div class="account-run-meta">
          <span>${escapeHtml(formatDateTime(run.finishedAt ?? run.startedAt))}</span>
          <span>${escapeHtml(run.requestedSurfaces?.map(surfaceName).join(", ") ?? "No surfaces")}</span>
        </div>
      </div>`;
  }

  async function invoke(command, args = {}) {
    if (isDesktop) return invokeNative(command, args);
    await new Promise((resolve) => setTimeout(resolve, 120));
    return invokeBrowserMock(command, args);
  }

  function invokeBrowserMock(command, args) {
    switch (command) {
      case "get_account_status":
        browserState.observedAt = new Date().toISOString();
        return structuredClone(browserState);
      case "preview_login": {
        const requested = args.surfaces?.length
          ? args.surfaces
          : surfaceOrder;
        const now = new Date();
        const operations = requested.map((surface) => {
          const observation = browserState.surfaces.find(
            (item) => item.surface === surface,
          );
          const supported = !["notInstalled", "unsupported"].includes(
            observation?.state,
          );
          return {
            surface,
            title: `Open ${surfaceName(surface)} sign-in`,
            description: supported
              ? "Browser preview: simulate the fixed official sign-in launcher"
              : "Install or resolve this surface before starting its sign-in flow",
            supported,
          };
        });
        return {
          id: crypto.randomUUID(),
          targetIdentity: browserState.anchor.identity,
          requestedSurfaces: requested,
          operations,
          createdAt: now.toISOString(),
          expiresAt: new Date(now.getTime() + 15 * 60 * 1000).toISOString(),
        };
      }
      case "apply_login_plan": {
        const plan = currentPlan;
        if (!plan || plan.id !== args.planId) {
          throw new Error("Browser preview sign-in plan is missing or consumed");
        }
        const now = new Date().toISOString();
        const steps = plan.operations.map((operation) => ({
          surface: operation.surface,
          status: operation.supported
            ? "actionRequired"
            : "skippedNotInstalled",
          detail: operation.supported
            ? "Browser preview simulated opening the official flow; complete it in the client and refresh"
            : operation.description,
        }));
        const launched = steps.filter(
          (step) => step.status === "actionRequired",
        ).length;
        const run = {
          id: crypto.randomUUID(),
          planId: plan.id,
          targetIdentity: plan.targetIdentity,
          requestedSurfaces: plan.requestedSurfaces,
          status: launched ? "actionRequired" : "failed",
          steps,
          summary: launched
            ? "Official sign-in flows were simulated; refresh after completing them"
            : "No sign-in flow could be launched",
          startedAt: now,
          finishedAt: now,
        };
        browserState.loginRuns.unshift(run);
        browserState.loginRuns = browserState.loginRuns.slice(0, 100);
        currentPlan = null;
        return {
          run,
          accountStatus: structuredClone(browserState),
        };
      }
      default:
        throw new Error(`Unsupported account preview command: ${command}`);
    }
  }

  async function refreshStatus({ quiet = false } = {}) {
    if (loading) return;
    loading = true;
    markChanged();
    try {
      const value = await invoke("get_account_status");
      validateStatus(value);
      accountStatus = value;
      if (!quiet) showToast("Account status refreshed");
    } catch (error) {
      showToast(error?.message ?? String(error), "error");
    } finally {
      loading = false;
      markChanged();
    }
  }

  function validateStatus(value) {
    if (!value || !value.anchor || !Array.isArray(value.surfaces)) {
      throw new Error("Account discovery returned an invalid response");
    }
    if (!Array.isArray(value.loginRuns)) {
      value.loginRuns = [];
    }
  }

  async function previewLogin() {
    if (loading || !accountStatus) return;
    if (accountStatus.historyRecovery) {
      showToast(
        "Resolve the read-only sign-in history before starting a new run",
        "error",
      );
      return;
    }
    const surfaces = Array.from(
      content.querySelectorAll('input[name="account-surface"]:checked'),
    ).map((input) => input.value);
    if (!surfaces.length) {
      showToast("Select at least one installed account surface", "error");
      return;
    }

    loading = true;
    markChanged();
    try {
      const plan = await invoke("preview_login", { surfaces });
      currentPlan = plan;
      openPlanModal(plan);
    } catch (error) {
      showToast(error?.message ?? String(error), "error");
    } finally {
      loading = false;
      markChanged();
    }
  }

  function openPlanModal(plan) {
    const target = plan.targetIdentity;
    const supportedCount = plan.operations.filter(
      (operation) => operation.supported,
    ).length;
    const needsManualTargetConfirmation = !target;
    modalRoot.innerHTML = `
      <div class="modal-backdrop account-modal-backdrop" role="presentation">
        <section class="modal wide account-modal" role="dialog" aria-modal="true" aria-label="Review account sign-in plan">
          <header class="modal-header">
            <div><p class="eyebrow">ONE-SHOT OFFICIAL FLOW</p><h2>Review sign-in plan</h2></div>
            <button class="icon-button" data-account-modal-close aria-label="Close">×</button>
          </header>
          <div class="modal-body">
            <div class="account-plan-security">
              This plan expires at ${escapeHtml(formatDateTime(plan.expiresAt))}, can be consumed once, and contains backend-owned executable paths and fixed arguments. The frontend will submit only its plan ID.
            </div>
            <div class="account-plan-target">
              <span>Target github.com identity</span>
              <strong>${target ? `${escapeHtml(target.login)} · ${escapeHtml(target.host)}` : "Not verified"}</strong>
              <p>${target ? "Use this same identity in every official client flow." : "PilotWeave cannot prove a common target yet. The run will only open official flows and will remain Action required until you verify the same github.com account."}</p>
            </div>
            ${needsManualTargetConfirmation ? `
              <label class="account-confirmation">
                <input type="checkbox" data-account-target-confirm />
                <span>I will select and verify the same github.com account in every launched client flow.</span>
              </label>` : ""}
            <div class="account-plan-list">
              ${plan.operations.map(renderPlanOperation).join("")}
            </div>
          </div>
          <footer class="modal-footer">
            <button class="button ghost" data-account-modal-close>Cancel</button>
            <button class="button primary" data-account-confirm ${supportedCount === 0 || needsManualTargetConfirmation ? "disabled" : ""}>Open official sign-in flows</button>
          </footer>
        </section>
      </div>`;

    modalRoot
      .querySelectorAll("[data-account-modal-close]")
      .forEach((button) => button.addEventListener("click", closeModal));
    modalRoot
      .querySelector(".account-modal-backdrop")
      ?.addEventListener("click", (event) => {
        if (event.target === event.currentTarget) closeModal();
      });
    const confirm = modalRoot.querySelector("[data-account-confirm]");
    modalRoot
      .querySelector("[data-account-target-confirm]")
      ?.addEventListener("change", (event) => {
        if (confirm) confirm.disabled = !event.target.checked || supportedCount === 0;
      });
    confirm?.addEventListener("click", () => applyPlan(plan));
  }

  function renderPlanOperation(operation) {
    return `
      <div class="account-plan-operation">
        <span class="account-status ${operation.supported ? "action" : "unsupported"}">${operation.supported ? "Will open" : "Unavailable"}</span>
        <div>
          <strong>${escapeHtml(operation.title)}</strong>
          <p>${escapeHtml(operation.description)}</p>
        </div>
      </div>`;
  }

  async function applyPlan(plan) {
    const confirm = modalRoot.querySelector("[data-account-confirm]");
    if (confirm) {
      confirm.disabled = true;
      confirm.textContent = "Opening…";
    }
    try {
      const result = await invoke("apply_login_plan", { planId: plan.id });
      validateStatus(result.accountStatus);
      currentPlan = null;
      accountStatus = result.accountStatus;
      showResultModal(result.run);
      showToast("Official sign-in launch completed");
      markChanged();
    } catch (error) {
    closeModal();
    showToast(
      `${error?.message ?? String(error)} Generate a new preview before retrying.`,
      "error",
    );
  }

  }

  function showResultModal(run) {
    modalRoot.innerHTML = `
      <div class="modal-backdrop account-modal-backdrop" role="presentation">
        <section class="modal wide account-modal" role="dialog" aria-modal="true" aria-label="Sign-in launch results">
          <header class="modal-header">
            <div><p class="eyebrow">OFFICIAL FLOW RESULTS</p><h2>${escapeHtml(runStatusLabel(run.status))}</h2></div>
            <button class="icon-button" data-account-modal-close aria-label="Close">×</button>
          </header>
          <div class="modal-body">
            <p class="account-result-summary">${escapeHtml(run.summary)}</p>
            <div class="account-result-list">
              ${run.steps.map(renderStep).join("")}
            </div>
          </div>
          <footer class="modal-footer">
            <button class="button ghost" data-account-modal-close>Close</button>
            <button class="button primary" data-account-result-refresh>Refresh account status</button>
          </footer>
        </section>
      </div>`;
    modalRoot
      .querySelectorAll("[data-account-modal-close]")
      .forEach((button) => button.addEventListener("click", closeModal));
    modalRoot
      .querySelector("[data-account-result-refresh]")
      ?.addEventListener("click", async () => {
        closeModal();
        await refreshStatus();
      });
  }

  function renderStep(step) {
    return `
      <div class="account-result-row">
        <span class="account-status ${stepStatusClass(step.status)}">${escapeHtml(stepStatusLabel(step.status))}</span>
        <div><strong>${escapeHtml(surfaceName(step.surface))}</strong><p>${escapeHtml(step.detail)}</p></div>
      </div>`;
  }

  function closeModal() {
    modalRoot.innerHTML = "";
    currentPlan = null;
  }

  function orderedSurfaces(values) {
    return [...values].sort(
      (left, right) =>
        surfaceOrder.indexOf(left.surface) - surfaceOrder.indexOf(right.surface),
    );
  }

  function surfaceName(surface) {
    return {
      vsCodeCopilot: "VS Code Copilot",
      copilotCli: "GitHub Copilot CLI",
      githubCopilotApp: "GitHub Copilot app",
    }[surface] ?? surface;
  }

  function surfaceIcon(surface) {
    return {
      vsCodeCopilot: "VS",
      copilotCli: ">_",
      githubCopilotApp: "◆",
    }[surface] ?? "◇";
  }

  function stateLabel(state) {
    return {
      verified: "Verified",
      inferred: "Inferred",
      actionRequired: "Action required",
      unknown: "Unknown",
      unsupported: "Unsupported",
      notInstalled: "Not installed",
      conflict: "Conflict",
    }[state] ?? state ?? "Unknown";
  }

  function stateClass(state) {
    return {
      verified: "verified",
      inferred: "inferred",
      actionRequired: "action",
      unknown: "unknown",
      unsupported: "unsupported",
      notInstalled: "not-installed",
      conflict: "conflict",
    }[state] ?? "unknown";
  }

  function runStatusLabel(status) {
    return {
      inProgress: "In progress",
      actionRequired: "Action required",
      partial: "Partial",
      failed: "Failed",
      completed: "Completed",
      interrupted: "Interrupted",
    }[status] ?? status;
  }

  function runStatusClass(status) {
    return {
      inProgress: "inferred",
      actionRequired: "action",
      partial: "unknown",
      failed: "conflict",
      completed: "verified",
      interrupted: "unknown",
    }[status] ?? "unknown";
  }

  function stepStatusLabel(status) {
    return {
      pending: "Pending",
      launched: "Launched",
      actionRequired: "Action required",
      skippedNotInstalled: "Not installed",
      unsupported: "Unsupported",
      failed: "Failed",
    }[status] ?? status;
  }

  function stepStatusClass(status) {
    return {
      pending: "unknown",
      launched: "inferred",
      actionRequired: "action",
      skippedNotInstalled: "not-installed",
      unsupported: "unsupported",
      failed: "conflict",
    }[status] ?? "unknown";
  }

  function formatDateTime(value) {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return "at an unknown time";
    return new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(date);
  }

  document.addEventListener("click", (event) => {
    const button = event.target.closest("[data-account-action]");
    if (!button) return;
    if (button.dataset.accountAction === "refresh") {
      refreshStatus();
      return;
    }
    if (button.dataset.accountAction === "preview") {
      previewLogin();
    }
  });

  const contentObserver = new MutationObserver(scheduleEnsurePanel);
  contentObserver.observe(content, { childList: true });
  const titleObserver = new MutationObserver(() => {
    if (isClientsRoute()) {
      ensurePanel();
      if (!accountStatus && !loading) refreshStatus({ quiet: true });
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
