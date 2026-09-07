const test = require("node:test");
const assert = require("node:assert/strict");
const PlanSession = require("../../apps/desktop/web/plan-session.js");
const plan = (id) => ({ id, operations: [], createdAt: "2026-09-01T00:00:00Z" });
const now = Date.parse("2026-09-01T00:01:00Z");

test("apply is bound to the exact reviewed plan and is one-shot", () => {
  const session = new PlanSession();
  assert.equal(session.accept(session.begin(), plan("reviewed")), true);
  assert.throws(() => session.consume(false, now));
  assert.deepEqual(session.consume(true, now), { planId: "reviewed", confirmed: true });
  assert.throws(() => session.consume(true, now));
});
test("target changes discard preview and late responses", () => {
  const session = new PlanSession();
  const generation = session.begin();
  session.invalidate();
  assert.equal(session.accept(generation, plan("stale")), false);
  assert.throws(() => session.consume(true, now));
});
test("out-of-order previews cannot replace a newer review", () => {
  const session = new PlanSession();
  const old = session.begin();
  const current = session.begin();
  assert.equal(session.accept(current, plan("current")), true);
  assert.equal(session.accept(old, plan("old")), false);
  assert.equal(session.consume(true, now).planId, "current");
});
test("expired plans are discarded rather than silently recomputed", () => {
  const session = new PlanSession();
  session.accept(session.begin(), plan("expired"));
  assert.throws(() => session.consume(true, now + 15 * 60000), /expired/);
  assert.equal(session.plan, null);
});
