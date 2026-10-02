const { test } = require("node:test");
const assert = require("node:assert/strict");
const { render, renderPlan } = require("../resources-ui.js");
test("resource data is escaped and publication is not represented as verified client consumption", () => {
  const html = render({ resources: [{ id: '" onclick="attack()', name: "<img src=x onerror=attack()>", kind: "instructions", updatedAt: "2026-10-02T00:00:00Z" }],
    bindings: [{ resourceId: '" onclick="attack()', targetId: "<script>attack()</script>", deployedAt: "2026-10-01T00:00:00Z" }], detail: "Last-published only" }, null, true);
  assert.ok(html.includes("&lt;img")); assert.ok(html.includes("&lt;script&gt;"));
  assert.ok(!html.includes("<script>")); assert.ok(!html.includes('id="" onclick='));
  assert.ok(html.includes("last-published")); assert.ok(html.includes("Local changes need publication")); assert.ok(html.includes("remain manual"));
});
test("resources unavailable in browser or storage recovery are not fabricated empty native catalogs", () => {
  const browser = render(null, null, false); assert.ok(browser.includes("Unavailable in browser preview")); assert.ok(!browser.includes("No authored resources yet"));
  const unavailable = render(null, "Read-only recovery", true); assert.ok(unavailable.includes("Resource storage is unavailable"));
  const empty = render({ resources: [], bindings: [] }, null, true); assert.ok(empty.includes("Saving a resource does not publish it"));
});
test("review renders native semantic changes and owned content as escaped text", () => {
  const html = renderPlan({ operations: [{ title: "CLI <script>attack()</script>", change: "create", destination: 'file:fixture/<img src=x>', detail: "Public target", contentPreview: '</pre><script>attack()</script>' }, { title: "Manual app", change: "manual", detail: "No stable path" }] });
  assert.ok(html.includes("Create owned projection"));
  assert.ok(html.includes("Manual / unavailable"));
  assert.ok(html.includes("Reviewed owned content"));
  assert.ok(html.includes("&lt;/pre&gt;&lt;script&gt;"));
  assert.ok(!html.includes("<script>"));
  assert.ok(!html.includes("<img"));
});
