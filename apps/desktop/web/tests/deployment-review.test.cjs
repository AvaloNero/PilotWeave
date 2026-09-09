const { test } = require("node:test");
const assert = require("node:assert/strict");
const { createSession } = require("../deployment-review.js");

test("apply submits only the exact reviewed ID and confirmation, then discards it", async () => {
  const calls = [];
  const session = createSession(async (command, args) => {
    calls.push({ command, args });
    return command === "preview_deployment" ? { id: "reviewed-id", operations: [] } : {};
  });
  await session.preview("connection", ["target"]);
  await session.apply();
  assert.deepEqual(calls[1], {
    command: "apply_deployment_plan",
    args: { planId: "reviewed-id", confirmed: true },
  });
  await assert.rejects(session.apply(), /Preview/);
  assert.equal(calls.length, 2);
});

test("a failed apply cannot replay its preview", async () => {
  const session = createSession(async (command) => {
    if (command === "preview_deployment") return { id: "one" };
    throw new Error("PlanChanged");
  });
  await session.preview("c", ["t"]);
  await assert.rejects(session.apply(), /PlanChanged/);
  assert.equal(session.plan, null);
  assert.equal(session.busy, false);
  await assert.rejects(session.apply(), /Preview/);
});

test("selection changes invalidate both a ready preview and a late response", async () => {
  let resolve;
  const session = createSession(() => new Promise((done) => { resolve = done; }));
  const pending = session.preview("c", ["old-target"]);
  session.invalidate();
  resolve({ id: "stale" });
  assert.equal(await pending, null);
  await assert.rejects(session.apply(), /Preview/);
  const ready = session.preview("c", ["new-target"]);
  resolve({ id: "ready" });
  await ready;
  session.invalidate();
  await assert.rejects(session.apply(), /Preview/);
});

test("duplicate preview and apply clicks cannot start concurrent requests", async () => {
  let resolve;
  let calls = 0;
  const session = createSession(() => {
    calls += 1;
    return new Promise((done) => { resolve = done; });
  });
  const preview = session.preview("c", ["t"]);
  await assert.rejects(session.preview("c", ["t"]), /in progress/);
  resolve({ id: "one" });
  await preview;
  const apply = session.apply();
  await assert.rejects(session.apply(), /in progress/);
  resolve({});
  await apply;
  assert.equal(calls, 2);
});
