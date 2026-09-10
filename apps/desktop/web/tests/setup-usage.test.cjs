const { test } = require("node:test");
const assert = require("node:assert/strict");
const setup = require("../setup-ui.js");
const usage = require("../usage-ui.js");
const connection = { id: "connection", name: "Fixture", models: [{ modelId: "gpt-5", name: "GPT-5", enabled: true }] };
const snapshot = { connections: [connection], clients: [{ id: "vscode", detected: true, supportsWrite: true }, { id: "cli", detected: true, supportsWrite: true }, { id: "app", detected: true, supportsWrite: false }] };
function data() { return { components: [1, 2, 3, 4].map((id) => ({ id, status: "ready" })), setup: { preferences: { connectionId: connection.id }, alignment: "userConfirmedSameAccount", manualConfirmed: true, targets: [{ targetId: "vscode", state: "inSync" }, { targetId: "cli", state: "inSync" }] }, errors: {} }; }

test("usage authorization does not block core setup and preview never claims real readiness", () => {
  const d = data(); d.errors.billing = "No authorization"; d.errors.localUsage = "Unavailable";
  assert.equal(setup.derive(snapshot, d, true).complete, 4);
  assert.equal(setup.derive(snapshot, d, false).complete, 0);
  assert.equal(setup.derive(snapshot, d, true).next.route, "usage");
});
test("missing apps, inferred accounts, stale deployment and manual setup choose a concrete next action", () => {
  const d = data();
  d.components[0].status = "missing"; assert.equal(setup.derive(snapshot, d).next.action, "setup-install");
  d.components[0].status = "ready"; d.setup.alignment = "partiallyVerified"; assert.equal(setup.derive(snapshot, d).next.action, "setup-accounts");
  d.setup.alignment = "userConfirmedSameAccount"; d.setup.targets[0].state = "outOfDate"; assert.equal(setup.derive(snapshot, d).next.action, "setup-deploy");
  d.setup.targets[0].state = "inSync"; d.setup.manualConfirmed = false; assert.equal(setup.derive(snapshot, d).next.action, "setup-manual");
  d.errors.setup = "Scan failed"; assert.equal(setup.derive(snapshot, d).scanned, false);
});
test("money rendering preserves decimal precision and unknown is distinct from exact zero", () => {
  assert.equal(usage.money("0.0000000000000000000000001234"), "$0.0000000000000000000000001234");
  assert.equal(usage.money("0"), "$0"); assert.match(usage.money(null), /Unavailable/);
  assert.equal(usage.value(0), "0"); assert.match(usage.value(null), /Unknown/);
});

test("Home puts editable connection controls first and scopes sign-in per client", () => {
  const d = data();
  d.setup.accounts = {surfaces:[{surface:'vsCodeCopilot',state:'actionRequired'}]};
  const html = setup.render(snapshot,d,true);
  assert.ok(html.indexOf('Connection and models') < html.indexOf('Scan this computer'));
  assert.match(html, /data-action="edit-connection" data-id="connection"/);
  assert.match(html, /data-action="add-connection"/);
  assert.match(html, /data-action="setup-signin" data-surface="vsCodeCopilot"/);
  assert.doesNotMatch(html, /Open official sign-in flows/);
  assert.match(html, /Provider setup confirmed by you/);
  const recovery=setup.render({...snapshot,stateRecovery:'Needs recovery'},d,true);
  assert.match(recovery,/data-action="edit-connection" data-id="connection" disabled/);
});

test("unknown Copilot detection is an issue to review, never an installation action", () => {
  const d = data(); d.components[1].status = "unknown";
  const state = setup.derive(snapshot, d);
  assert.equal(state.installed, false);
  assert.equal(state.missing.length, 0);
  assert.equal(state.next.route, "clients");
  assert.notEqual(state.next.action, "setup-install");
});

test("interrupted deployment and read-only recovery cannot report core setup ready", () => {
  const pending = { ...snapshot, deploymentRecovery: "An interrupted write needs review" };
  assert.equal(setup.derive(pending, data()).deployed, false);
  assert.equal(setup.derive(pending, data()).complete, 3);
  assert.equal(setup.derive(pending, data()).next.action, "preview-recovery");
  assert.equal(setup.derive({ ...snapshot, stateRecovery: "Invalid primary" }, data()).next.route, "settings");
});
test("model union retains configured, runtime, Billing and unpriced local models without route guessing", () => {
  const d = { localUsage: { models: [{ rawModel: "gpt-5", canonicalModel: null, route: "unknown", confidence: "unknown", surfaces: ["shared-copilot-runtime"], totals: { records: 1 }, quality: [] }] }, quota: { latest: { models: [{ id: "gpt-5", policy: "enabled" }] } }, billing: { families: [{ endpointFamily: "premiumRequest", latest: { status: "available", periodStart: "2026-09-01", items: [{ model: "billing-only", quantity: "0", unit: "requests", netAmountUsd: "0" }] } }] } };
  const rows = usage.modelUnion(snapshot, d);
  assert.equal(rows.length, 4); assert.equal(rows.filter((r) => r.rawModel === "gpt-5").length, 3);
  assert.equal(rows.find((r) => r.route === "unknown").totals.records, 1);
  assert.equal(rows.find((r) => r.route === "byok").totals, null);
  assert.equal(rows.find((r) => r.rawModel === "billing-only").official[0].netAmountUsd, "0");
});
test("distinct attribution-confidence groups are never silently overwritten", () => {
  const models = ["explicit", "exactConnectionIdentity"].map((confidence) => ({ rawModel: "gpt-5", route: "byok", confidence, origins: [], totals: { records: 1 } }));
  assert.equal(usage.modelUnion(snapshot, { localUsage: { models } }).length, 2);
});
test("remote strings and source setup paths are escaped and cannot inject markup", () => {
  assert.equal(setup.escape('<img src=x onerror="alert(1)">'), "&lt;img src=x onerror=&quot;alert(1)&quot;&gt;");
  assert.doesNotMatch(usage.setupInstructions({ id: "vscode-otel", pathHint: "<script>attack()</script>", setupPath: 'C:\\<img onerror="bad">' }), /<script>|<img/);
  const markup = usage.render(snapshot, { errors: { localUsage: "<script>bad</script>" }, billing: { families: [] } }, false);
  assert.match(markup, /Browser preview/); assert.doesNotMatch(markup, /<script>/); assert.match(markup, /disabled/);
});
