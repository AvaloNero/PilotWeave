(() => {
  "use strict";

  const invokeNative = window.__TAURI__?.core?.invoke ?? null;
  const isDesktop = typeof invokeNative === "function";
  const content = document.querySelector("#content");
  const pageTitle = document.querySelector("#page-title");
  const modalRoot = document.querySelector("#modal-root");
  const toastRoot = document.querySelector("#toast-root");

  if (!content || !pageTitle || !modalRoot || !toastRoot) return;

  let status = null;
  let loading = false;
  let renderVersion = 0;
  let scheduled = false;

  const browserStatus = {
    state: "missing",
    identity: null,
    hasSecret: false,
    scopes: [],
    billingCapability: "unknown",
    billingDetail:
      "Authorize PilotWeave before requesting personal Billing data",
    detail: "Browser preview has no separate GitHub authorization",
    validatedAt: null,
    recovery: null,
    cleanupWarning: null,
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

  function isSettingsRoute() {
    return pageTitle.textContent?.trim() === "Settings";
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
    if (!isSettingsRoute()) return;
    let panel = content.querySelector("#github-authorization-panel");
    if (!panel) {
      panel = document.createElement("section");
      panel.id = "github-authorization-panel";
      panel.className = "github-authorization-panel";
      content.prepend(panel);
    }
    if (panel.dataset.renderVersion === String(renderVersion)) return;
    panel.dataset.renderVersion = String(renderVersion);
    panel.innerHTML = renderPanel();
  }

  function renderPanel() {
    const value = status;
    const readOnly = value?.state === "readOnlyRecovery";
    const canRefresh = Boolean(value?.hasSecret) && !readOnly;
    const canClear = Boolean(value?.hasSecret || value?.identity) && !readOnly;
    const identity = value?.identity;
    const scopes = value?.scopes ?? [];

    return `
      <div class="github-auth-header">
        <div>
          <p class="eyebrow">PILOTWEAVE-OWNED AUTHORIZATION</p>
          <h2>Personal GitHub Billing access</h2>
          <p>This authorization is separate from VS Code, Copilot CLI, the GitHub Copilot app, and GitHub CLI sign-in. PilotWeave stores only the token in the operating-system credential store and keeps bounded identity/capability metadata in its own file.</p>
        </div>
        <div class="github-auth-actions">
          <button class="button ghost small" data-github-auth-action="refresh" ${loading || !canRefresh ? "disabled" : ""}>${loading ? "Checking…" : "Validate again"}</button>
          <button class="button primary small" data-github-auth-action="authorize" ${loading || readOnly ? "disabled" : ""}>${value?.hasSecret ? "Replace authorization" : "Authorize PilotWeave"}</button>
          <button class="button danger small" data-github-auth-action="clear" ${loading || !canClear ? "disabled" : ""}>Clear</button>
        </div>
      </div>
      ${value?.recovery ? renderRecovery(value.recovery) : ""}
      ${value?.cleanupWarning ? renderCleanupWarning(value.cleanupWarning) : ""}
      ${loading && !value ? '<div class="github-auth-loading">Reading separate authorization state…</div>' : ""}
      ${!loading && !value ? '<div class="github-auth-loading">Authorization status has not been loaded.</div>' : ""}
      ${value ? `
        <div class="github-auth-grid">
          <article class="github-auth-identity">
            <div class="github-auth-card-heading">
              <span>Validated identity</span>
              <span class="github-auth-state ${stateClass(value.state)}">${escapeHtml(stateLabel(value.state))}</span>
            </div>
            <strong>${identity ? `${escapeHtml(identity.login)} · ${escapeHtml(identity.host)}` : "Not authorized"}</strong>
            <p>${escapeHtml(value.detail)}</p>
            <div class="github-auth-meta">
              <span>${identity?.userId != null ? `GitHub user ID ${escapeHtml(identity.userId)}` : "Stable user ID unavailable"}</span>
              <span>${value.validatedAt ? `Validated ${escapeHtml(formatDateTime(value.validatedAt))}` : "Never validated"}</span>
            </div>
          </article>
          <article class="github-auth-billing">
            <div class="github-auth-card-heading">
              <span>Personal Billing capability</span>
              <span class="github-auth-state ${billingClass(value.billingCapability)}">${escapeHtml(billingLabel(value.billingCapability))}</span>
            </div>
            <strong>${billingTitle(value.billingCapability)}</strong>
            <p>${escapeHtml(value.billingDetail)}</p>
            <div class="github-auth-permission">Required for premium-request reports: fine-grained token with <strong>Plan · read</strong> user permission.</div>
          </article>
        </div>
        <div class="github-auth-scopes">
          <div><strong>Reported OAuth scopes</strong><span>Fine-grained tokens may not populate the classic X-OAuth-Scopes header; endpoint probing remains authoritative.</span></div>
          <div class="github-auth-scope-list">${scopes.length ? scopes.slice(0, 12).map((scope) => `<span>${escapeHtml(scope)}</span>`).join("") : '<span class="muted-scope">Not reported</span>'}</div>
        </div>
      ` : ""}
      <div class="github-auth-boundary">
        <strong>Security boundary</strong>
        <span>No client OAuth token, browser cookie, SecretStorage entry, refresh token, password, or passkey is read or copied. Failed replacement attempts leave a previously valid PilotWeave authorization unchanged.</span>
      </div>
    `;
  }

  function renderRecovery(reason) {
    return `<div class="github-auth-warning danger"><strong>Read-only recovery</strong><span>${escapeHtml(reason)}</span></div>`;
  }

  function renderCleanupWarning(reason) {
    return `<div class="github-auth-warning"><strong>Credential cleanup needs attention</strong><span>${escapeHtml(reason)}</span></div>`;
  }

  async function invoke(command, args = {}) {
    if (isDesktop) return invokeNative(command, args);
    await new Promise((resolve) => setTimeout(resolve, 140));
    switch (command) {
      case "get_github_authorization_status":
        return structuredClone(browserStatus);
      case "authorize_github": {
        const token = String(args.token ?? "");
        if (!token || token.trim() !== token || /[\r\n\0]/.test(token)) {
          throw new Error("Enter a single-line GitHub token without surrounding whitespace");
        }
        Object.assign(browserStatus, {
          state: "verified",
          identity: {
            host: "github.com",
            login: "preview-user",
            userId: 1001,
            avatarUrl: null,
          },
          hasSecret: true,
          scopes: [],
          billingCapability: "available",
          billingDetail:
            "Browser preview simulated access to personal premium-request Billing",
          detail:
            "Browser preview simulated a separate authorization; no token was persisted",
          validatedAt: new Date().toISOString(),
          recovery: null,
          cleanupWarning: null,
        });
        return structuredClone(browserStatus);
      }
      case "refresh_github_authorization":
        if (!browserStatus.hasSecret) return structuredClone(browserStatus);
        browserStatus.validatedAt = new Date().toISOString();
        return structuredClone(browserStatus);
      case "clear_github_authorization":
        Object.assign(browserStatus, {
          state: "missing",
          identity: null,
          hasSecret: false,
          scopes: [],
          billingCapability: "unknown",
          billingDetail:
            "Authorize PilotWeave before requesting personal Billing data",
          detail: "Browser preview has no separate GitHub authorization",
          validatedAt: null,
          recovery: null,
          cleanupWarning: null,
        });
        return structuredClone(browserStatus);
      default:
        throw new Error(`Unsupported GitHub authorization preview command: ${command}`);
    }
  }

  async function refreshStatus({ quiet = false } = {}) {
    if (loading) return;
    loading = true;
    markChanged();
    try {
      const value = await invoke("get_github_authorization_status");
      validateStatus(value);
      status = value;
      if (!quiet) showToast("GitHub authorization status refreshed");
    } catch (error) {
      showToast(error?.message ?? String(error), "error");
    } finally {
      loading = false;
      markChanged();
    }
  }

  function validateStatus(value) {
    if (!value || typeof value.state !== "string") {
      throw new Error("GitHub authorization returned an invalid response");
    }
    if (!Array.isArray(value.scopes)) value.scopes = [];
  }

  function openAuthorizeModal() {
    modalRoot.innerHTML = `
      <div class="modal-backdrop github-auth-modal-backdrop" role="presentation">
        <section class="modal github-auth-modal" role="dialog" aria-modal="true" aria-label="Authorize PilotWeave with GitHub">
          <header class="modal-header">
            <div><p class="eyebrow">SEPARATE AUTHORIZATION</p><h2>Authorize PilotWeave</h2></div>
            <button class="icon-button" data-github-auth-close aria-label="Close">×</button>
          </header>
          <div class="modal-body">
            <div class="github-auth-modal-copy">
              Create a fine-grained personal access token for your own account with <strong>Plan · read</strong> user permission. PilotWeave first validates <code>GET /user</code>, then probes the personal premium-request Billing endpoint. The token is never written to JSON or returned by the backend.
            </div>
            <form id="github-auth-form" class="form-grid">
              <div class="form-field full">
                <label for="github-auth-token">Fine-grained GitHub token</label>
                <input id="github-auth-token" name="token" type="password" autocomplete="new-password" required maxlength="65536" placeholder="github_pat_…" />
                <small>A rejected replacement never overwrites an existing valid PilotWeave token.</small>
              </div>
            </form>
          </div>
          <footer class="modal-footer">
            <button class="button ghost" data-github-auth-close>Cancel</button>
            <button class="button primary" data-github-auth-submit>Validate and store</button>
          </footer>
        </section>
      </div>`;

    modalRoot
      .querySelectorAll("[data-github-auth-close]")
      .forEach((button) => button.addEventListener("click", closeModal));
    modalRoot
      .querySelector(".github-auth-modal-backdrop")
      ?.addEventListener("click", (event) => {
        if (event.target === event.currentTarget) closeModal();
      });
    modalRoot
      .querySelector("[data-github-auth-submit]")
      ?.addEventListener("click", submitAuthorization);
    modalRoot.querySelector("#github-auth-token")?.focus();
  }

  async function submitAuthorization() {
    const form = modalRoot.querySelector("#github-auth-form");
    if (!form?.reportValidity()) return;
    const tokenInput = form.querySelector("#github-auth-token");
    const token = tokenInput.value;
    const submit = modalRoot.querySelector("[data-github-auth-submit]");
    if (submit) {
      submit.disabled = true;
      submit.textContent = "Validating…";
    }
    try {
      const value = await invoke("authorize_github", { token });
      tokenInput.value = "";
      validateStatus(value);
      status = value;
      closeModal();
      showToast(
        value.state === "verified"
          ? "Separate GitHub authorization stored"
          : value.detail,
        value.state === "verified" ? "success" : "error",
      );
      markChanged();
    } catch (error) {
      tokenInput.value = "";
      showToast(error?.message ?? String(error), "error");
      if (submit) {
        submit.disabled = false;
        submit.textContent = "Validate and store";
      }
    }
  }

  function openClearModal() {
    modalRoot.innerHTML = `
      <div class="modal-backdrop github-auth-modal-backdrop" role="presentation">
        <section class="modal github-auth-modal" role="dialog" aria-modal="true" aria-label="Clear GitHub authorization">
          <header class="modal-header"><h2>Clear separate GitHub authorization</h2><button class="icon-button" data-github-auth-close aria-label="Close">×</button></header>
          <div class="modal-body"><p class="github-auth-modal-copy">This removes PilotWeave's bounded authorization metadata and requests deletion of its dedicated operating-system credential entry. It does not sign out VS Code, Copilot CLI, the GitHub Copilot app, or GitHub CLI.</p></div>
          <footer class="modal-footer"><button class="button ghost" data-github-auth-close>Cancel</button><button class="button danger" data-github-auth-confirm-clear>Clear authorization</button></footer>
        </section>
      </div>`;
    modalRoot
      .querySelectorAll("[data-github-auth-close]")
      .forEach((button) => button.addEventListener("click", closeModal));
    modalRoot
      .querySelector("[data-github-auth-confirm-clear]")
      ?.addEventListener("click", clearAuthorization);
  }

  async function clearAuthorization() {
    const button = modalRoot.querySelector("[data-github-auth-confirm-clear]");
    if (button) button.disabled = true;
    try {
      const value = await invoke("clear_github_authorization");
      validateStatus(value);
      status = value;
      closeModal();
      showToast(
        value.cleanupWarning ?? "Separate GitHub authorization cleared",
        value.cleanupWarning ? "error" : "success",
      );
      markChanged();
    } catch (error) {
      showToast(error?.message ?? String(error), "error");
      if (button) button.disabled = false;
    }
  }

  async function revalidate() {
    if (loading) return;
    loading = true;
    markChanged();
    try {
      const value = await invoke("refresh_github_authorization");
      validateStatus(value);
      status = value;
      showToast(
        value.state === "verified" ? "Authorization validated" : value.detail,
        value.state === "verified" ? "success" : "error",
      );
    } catch (error) {
      showToast(error?.message ?? String(error), "error");
    } finally {
      loading = false;
      markChanged();
    }
  }

  function closeModal() {
    const token = modalRoot.querySelector("#github-auth-token");
    if (token) token.value = "";
    modalRoot.innerHTML = "";
  }

  function stateLabel(value) {
    return {
      missing: "Missing",
      verified: "Verified",
      unauthorized: "Unauthorized",
      forbidden: "Forbidden",
      networkError: "Network error",
      schemaError: "Schema error",
      conflict: "Conflict",
      readOnlyRecovery: "Read-only recovery",
    }[value] ?? value;
  }

  function stateClass(value) {
    return {
      verified: "verified",
      missing: "neutral",
      unauthorized: "danger",
      forbidden: "danger",
      networkError: "warning",
      schemaError: "danger",
      conflict: "warning",
      readOnlyRecovery: "danger",
    }[value] ?? "neutral";
  }

  function billingLabel(value) {
    return {
      available: "Available",
      insufficientPermission: "Permission required",
      notCovered: "Not covered",
      unavailable: "Unavailable",
      unknown: "Unknown",
    }[value] ?? value;
  }

  function billingClass(value) {
    return {
      available: "verified",
      insufficientPermission: "warning",
      notCovered: "neutral",
      unavailable: "danger",
      unknown: "neutral",
    }[value] ?? "neutral";
  }

  function billingTitle(value) {
    return {
      available: "Premium-request reports can be fetched",
      insufficientPermission: "Identity is valid; Plan read is missing",
      notCovered: "Personal Billing is not exposed for this account",
      unavailable: "The capability probe could not be completed",
      unknown: "Billing capability has not been probed",
    }[value] ?? "Billing capability is unknown";
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
    const button = event.target.closest("[data-github-auth-action]");
    if (!button) return;
    switch (button.dataset.githubAuthAction) {
      case "authorize":
        openAuthorizeModal();
        break;
      case "refresh":
        revalidate();
        break;
      case "clear":
        openClearModal();
        break;
    }
  });

  const contentObserver = new MutationObserver(scheduleEnsurePanel);
  contentObserver.observe(content, { childList: true });
  const titleObserver = new MutationObserver(() => {
    if (isSettingsRoute()) {
      ensurePanel();
      if (!status && !loading) refreshStatus({ quiet: true });
    }
  });
  titleObserver.observe(pageTitle, {
    childList: true,
    characterData: true,
    subtree: true,
  });

  if (isSettingsRoute()) {
    ensurePanel();
    refreshStatus({ quiet: true });
  }
})();
