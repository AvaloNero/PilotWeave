// Optional end-to-end UI check. Runs against an ephemeral HTTP server and a
// fresh headless browser profile. The native API is replaced with in-memory
// fixtures; no installed client's data or configuration is accessed.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const http = require("node:http");
const os = require("node:os");
const { chromium } = require(process.env.PILOTWEAVE_PLAYWRIGHT ?? "playwright");
const webRoot = path.resolve(__dirname, "..");
const output = fs.mkdtempSync(path.join(os.tmpdir(), "pilotweave-ui-"));
const mime = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript" };
const server = http.createServer((req, res) => {
  const name = new URL(req.url, "http://localhost").pathname;
  const target = path.resolve(webRoot, `.${name === "/" ? "/index.html" : name}`);
  if (!target.startsWith(webRoot + path.sep) || !fs.existsSync(target)) { res.writeHead(404); res.end(); return; }
  res.setHeader("Content-Type", mime[path.extname(target)] ?? "text/plain");
  res.end(fs.readFileSync(target));
});
async function nativeFixture() {
  const now = new Date().toISOString(), day = now.slice(0, 10);
  const connection = { id: "connection", name: "Example provider", baseUrl: "https://example.invalid/v1", providerKind: "openai", protocol: "chat-completions", headers: {}, models: [{ id: "model", modelId: "gpt-5", name: "GPT-5", enabled: true, capabilities: {} }], secretRef: "fixture", hasSecret: true, createdAt: now, updatedAt: now };
  const clients = [
    { id: "vscode", name: "VS Code · Default", kind: "vs-code-copilot", detected: true, supportsWrite: true, status: "available", detail: "Fixture profile", path: "Fixture" },
    { id: "cli", name: "Copilot CLI", kind: "copilot-cli", detected: true, supportsWrite: true, status: "available", detail: "Fixture runtime", path: "Fixture" },
    { id: "app", name: "Copilot app", kind: "github-copilot-app", detected: true, supportsWrite: false, status: "read-only", detail: "Manual configuration", path: "Fixture" },
  ];
  const components = ["vscode", "vscode-copilot-extension", "copilot-cli", "github-copilot-app"].map((id) => ({ id, name: id, status: "ready", version: "fixture", detail: "Isolated UI fixture" }));
  const identity = { login: "fixture", host: "github.com", userId: 1 };
  const accounts = { anchor: { state: "verified", identity, evidence: "Fake anchor", detail: "Fake API identity", observedAt: now }, surfaces: ["vsCodeCopilot", "copilotCli", "githubCopilotApp"].map((surface) => ({ surface, state: "actionRequired", identity: null, evidence: "Manual confirmation needed", detail: "Fixture", observedAt: now })), observedAt: now, loginRuns: [] };
  const setup = { scannedAt: now, target: identity, alignment: "partiallyVerified", evidenceFingerprint: "fixture-evidence", accounts, preferences: { connectionId: connection.id }, targets: clients.map((c) => ({ targetId: c.id, state: c.supportsWrite ? "notDeployed" : "manual", detail: "Fixture" })), manualConfirmed: false };
  const totals = { records: 1, requests: 1, inputReported: 1000, normalizedInput: 1000, freshInput: 200, cacheRead: 800, cacheWrite: 0, output: 40, cacheHitRate: "0.8", cacheCoveredRecords: 1, estimateUsd: "0.00075", pricedRecords: 1, unpricedRecords: 0, unresolvedModels: 0, unknownSemantics: 0, pricedTokens: 1040, unpricedTokens: null, priceSnapshots: ["fixture-price"], pricedRequests: 1, unpricedRequests: null };
  const record = { id: "hashed-record", rawModel: "gpt-5", canonicalModel: "openai/gpt-5", counterKind: "requestDelta", sourceId: "vscode-otel", route: "unknown", confidence: "unknown", inputReported: 1000, output: 40, estimateUsd: "0.00075", priceSnapshotId: "fixture-price", startedAt: now, finishedAt: now, surfaces: ["vs-code-copilot"], quality: [] };
  const sources = ["copilot-session-events", "vscode-otel", "github-copilot-app"].map((id) => ({ id, name: id, enabled: false, status: id === "github-copilot-app" ? "unsupported" : "disabled", detail: "Isolated UI fixture", pathHint: "Fixed fixture inbox", setupPath: id === "vscode-otel" ? "C:\\Fixture\\vscode-otel.jsonl" : null, parser: "fixture-v1", recordCount: 0 }));
  const quota = { latest: { id: "quota", status: "available", detail: "Official RPC fixture", account: "fixture@github.com", fetchedAt: now, runtimeVersion: "0.0.fixture", modelStatus: "available", models: [{ id: "gpt-5", name: "GPT-5", policy: "enabled" }], quotas: [{ key: "premium_interactions", used: "0", entitlement: "300", unlimited: false, remainingPercentage: "100", resetAt: "2026-10-01" }] }, lastSuccessful: null, stale: false };
  const authorization = { state: "verified", identity, hasSecret: true, scopes: [], billingCapability: "unknown", billingDetail: "Fixture", detail: "Fixture", validatedAt: now };
  const billing = { authorization, account: identity, families: [{ endpointFamily: "premiumRequest", latest: { id: "billing", status: "available", periodStart: `${day.slice(0, 7)}-01T00:00:00Z`, periodEnd: "2026-10-01T00:00:00Z", fetchedAt: now, apiVersion: "2026-03-10", coverage: "personalAccountOnly", items: [{ model: "GPT-5", sku: "Copilot Premium Request", quantity: "3.25", unit: "requests", netAmountUsd: "0", grossAmountUsd: "0.125", discountAmountUsd: "0.125" }] } }, { endpointFamily: "aiCredit", latest: { status: "notCovered", error: "Personal endpoint does not cover this plan", fetchedAt: now } }] };
  const catalog = { catalog: { id: "fixture-price", sourceVersion: "fixture", fetchedAt: now, parserVersion: 1, detail: "Public rate comparison fixture", models: [{ model: "openai/gpt-5", supported: true, tiers: [{ minInput: 0, input: "1.25", output: "10", cacheRead: "0.125", cacheWrite: null }] }] }, latest: { status: "available", finishedAt: now, detail: "Fixture catalog" }, stale: false };
  let deploymentRecovery = null;
  let recoveryIssue = {}, recoveryPlan = null;
  window.__setRecovery = (issue = {}) => { deploymentRecovery = "An interrupted deployment needs review"; recoveryIssue = issue; };
  window.__calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args = {}) => {
    window.__calls.push({ cmd, args });
    if (cmd === "get_dashboard") return { version: 1, connections: [connection], clients, deployments: [], stateRecovery: null, deploymentRecovery, statePath: "Fixture only", usageDb: { state: "available", schemaVersion: 2 } };
    if (cmd === "get_installation_status") return components;
    if (cmd === "get_setup_status") return structuredClone(setup);
    if (cmd === "get_github_authorization_status") return authorization;
    if (cmd === "get_usage_sources") return structuredClone(sources);
    if (cmd === "get_usage_runs") return [];
    if (cmd === "get_official_runtime_usage") return quota;
    if (cmd === "get_github_billing") return billing;
    if (cmd === "get_price_catalog") return catalog;
    if (cmd === "get_usage_overview") return { start: args.query.start, end: args.query.end, status: "partial", totals, records: [record], totalRecords: 1, coverageStart: now, coverageEnd: now, lastImported: now, models: [{ ...record, totals }], days: [{ key: day, totals }], routes: [], surfaces: [], providers: [] };
    if (cmd === "confirm_account_alignment") { assertNative(args.confirmed && args.evidenceFingerprint === "fixture-evidence"); setup.alignment = "userConfirmedSameAccount"; setup.preferences.accountConfirmation = { confirmedAt: now }; return; }
    if (cmd === "confirm_manual_provider") { setup.manualConfirmed = true; return; }
    if (cmd === "set_usage_source_enabled") { const source = sources.find((s) => s.id === args.sourceId); source.enabled = args.enabled; source.status = args.enabled ? "missing" : "disabled"; return; }
    if (cmd === "clear_local_usage") { assertNative(args.confirmed); sources.find((s) => s.id === args.sourceId).enabled = false; return; }
    if (cmd === "sync_local_usage" || cmd === "cancel_usage_sync") return [];
    if (["refresh_official_runtime_usage", "refresh_price_catalog", "refresh_personal_github_usage"].includes(cmd)) return null;
    if (cmd === "preview_deployment_recovery") { assertNative(["restore", "keepCurrent"].includes(args.action)); recoveryPlan = { id: `reviewed-recovery-${args.action}`, action: args.action, view: { resourceCount: 2, conflictCount: 0, unknownResourceCount: 0, unreadableResourceCount: 0, committed: false, ...recoveryIssue } }; return structuredClone(recoveryPlan); }
    if (cmd === "apply_deployment_recovery") { assertNative(recoveryPlan && args.planId === recoveryPlan.id && args.confirmed && Object.keys(args).length === 2); recoveryPlan = null; deploymentRecovery = null; return true; }
    if (cmd === "preview_deployment") return { id: "reviewed-plan", connectionId: connection.id, operations: args.targetIds.map((id) => ({ targetId: id, title: id, description: "Fixture operation", supported: true, changes: ["Fixture change"] })) };
    if (cmd === "apply_deployment_plan") { assertNative(args.planId === "reviewed-plan" && args.confirmed === true && Object.keys(args).length === 2); setup.targets.forEach((t) => { if (t.targetId !== "app") t.state = "inSync"; }); return { records: [{ status: "applied" }] }; }
    throw Error(`Unmocked command: ${cmd}`);
  } }, event: { listen: async () => () => {} } };
  function assertNative(condition) { if (!condition) throw Error("Incorrect native command arguments"); }
}
(async () => {
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const browser = await chromium.launch({ headless: true, executablePath: process.env.PILOTWEAVE_BROWSER });
  const errors = [];
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1100 } });
    page.on("pageerror", (err) => errors.push(String(err)));
    const url = `http://127.0.0.1:${server.address().port}/`;
    await page.goto(url); await page.getByText("Preview only · open the desktop app", { exact: true }).waitFor();
    assert.equal(await page.locator(".setup-progress strong").textContent(), "Preview");
    await page.screenshot({ path: path.join(output, "home-preview.png"), fullPage: true });
    for (const route of ["usage", "connections", "clients", "activity", "settings", "about"]) {
      if (["clients", "activity", "settings", "about"].includes(route)) await page.locator(".more-nav").evaluate((e) => { e.open = true; });
      await page.locator(`#primary-nav [data-route="${route}"]`).click();
      await page.locator(`#content[data-route="${route}"]`).waitFor();
      assert.ok(await page.locator(".preview-banner").count());
    }
    await page.close();
    const native = await browser.newPage({ viewport: { width: 1440, height: 1100 } }); native.on("pageerror", (err) => errors.push(String(err)));
    await native.addInitScript(nativeFixture); await native.goto(url);
    await native.getByRole("button", { name: "Verify the same GitHub account", exact: true }).waitFor();
    await native.getByRole("button", { name: "Confirm accounts", exact: true }).click(); await native.locator("#confirm-all-accounts").check(); await native.locator("#save-account-confirmation").click();
    await native.getByRole("button", { name: "Preview and deploy Example provider", exact: true }).waitFor();
    await native.getByRole("button", { name: "Preview and deploy Example provider", exact: true }).click();
    assert.equal(await native.locator('input[name="deployment-target"]:checked').count(), 2);
    await native.locator("#preview-deployment").click(); await native.getByRole("button", { name: "Apply supported changes", exact: true }).click();
    await native.getByRole("button", { name: "Complete Copilot app setup", exact: true }).waitFor(); await native.getByRole("button", { name: "Complete Copilot app setup", exact: true }).click(); await native.locator("#confirm-manual").click();
    await native.getByText("Your core Copilot setup is ready", { exact: true }).waitFor(); await native.screenshot({ path: path.join(output, "home-ready-fixture.png"), fullPage: true });
    await native.locator('#primary-nav [data-route="usage"]').click();
    await native.locator('[data-usage-action="runtime"]').click(); await native.locator('[data-usage-action="billing"]').click();
    await native.screenshot({ path: path.join(output, "usage-official-fixture.png"), fullPage: true });
    await native.locator('[data-usage-tab="models"]').click(); assert.ok(await native.getByText("gpt-5", { exact: true }).count());
    await native.screenshot({ path: path.join(output, "usage-models-fixture.png"), fullPage: true });
    await native.locator('[data-usage-tab="sessions"]').click(); await native.locator('[data-usage-action="record"]').click(); assert.ok((await native.locator(".usage-code").textContent()).includes("hashed-record")); await native.locator(".modal-footer [data-modal-close]").click();
    await native.locator('[data-usage-tab="sources"]').click();
    await native.locator('[data-usage-action="enable-source"][data-source-id="vscode-otel"]').click(); assert.ok((await native.locator(".usage-code").textContent()).includes("captureContent")); await native.locator("#usage-confirm").click();
    await native.locator('[data-usage-action="disable-source"][data-source-id="vscode-otel"]').waitFor(); await native.locator('[data-usage-action="sync"]').click(); await native.locator('[data-usage-action="prices"]').click();
    await native.screenshot({ path: path.join(output, "usage-sources-fixture.png"), fullPage: true });
    await native.locator('[data-usage-action="clear-source"][data-source-id="vscode-otel"]').click(); await native.locator("#usage-confirm").click();
    await native.locator('#usage-filters [name="model"]').fill("gpt-5"); await native.locator('#usage-filters button[type="submit"]').click();
    await native.waitForFunction(() => window.__calls.some((c) => c.cmd === "get_usage_overview" && c.args.query.model === "gpt-5"));
    await native.evaluate(() => window.__setRecovery());
    await native.locator('#primary-nav [data-route="overview"]').click(); await native.locator('#refresh-button').click();
    await native.getByRole("button", { name: "Review interrupted deployment", exact: true }).click();
    assert.equal(await native.locator('#apply-recovery').isDisabled(), true);
    await native.locator('#confirm-recovery').check(); await native.locator('#apply-recovery').click();
    await native.getByText("Your core Copilot setup is ready", { exact: true }).waitFor();
    for (const issue of [{ conflictCount: 1 }, { unknownResourceCount: 1 }]) {
      await native.evaluate((value) => window.__setRecovery(value), issue);
      await native.locator('#refresh-button').click();
      await native.getByRole("button", { name: "Review interrupted deployment", exact: true }).click();
      assert.equal(await native.locator('#apply-recovery').isDisabled(), true);
      assert.equal(await native.locator('#confirm-recovery').isDisabled(), true);
      await native.locator('#review-keep-current').click();
      await native.getByText("Review keeping current files", { exact: true }).waitFor();
      assert.equal(await native.locator('#apply-recovery').isDisabled(), true);
      assert.equal(await native.locator('#confirm-recovery').isChecked(), false);
      await native.locator('#confirm-recovery').check(); await native.locator('#apply-recovery').click();
      await native.getByText("Your core Copilot setup is ready", { exact: true }).waitFor();
      const calls = await native.evaluate(() => window.__calls.filter((c) => c.cmd === "preview_deployment_recovery").slice(-2));
      assert.deepEqual(calls.map((c) => c.args.action), ["restore", "keepCurrent"]);
    }
    assert.equal(await native.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    assert.deepEqual(errors, []); console.log(JSON.stringify({ status: "passed", scenarios: ["preview routes", "account confirmation", "reviewed deployment", "manual provider", "quota and Billing", "models and details", "source consent/import/clear", "price refresh", "filters", "reviewed recovery", "external conflict retention", "missing target retention"], output }));
  } finally { await browser.close(); server.close(); }
})().catch((err) => { console.error(err); server.close(); process.exitCode = 1; });
