/* The reviewed plan is single-use in the UI as well as in native memory. */
((scope) => {
  "use strict";

  function createSession(invoke) {
    let plan = null;
    let busy = false;
    let generation = 0;
    return {
      get plan() { return plan; },
      get busy() { return busy; },
      invalidate() {
        generation += 1;
        plan = null;
      },
      async preview(connectionId, targetIds) {
        if (busy) throw new Error("A deployment request is already in progress");
        plan = null;
        const request = ++generation;
        busy = true;
        try {
          const result = await invoke("preview_deployment", { connectionId, targetIds });
          if (request === generation) plan = result;
          return plan;
        } finally {
          busy = false;
        }
      },
      async apply() {
        if (busy) throw new Error("A deployment request is already in progress");
        if (!plan) throw new Error("Preview the current selection before applying it");
        const planId = plan.id;
        // Even a failed/uncertain native call consumes the UI preview. Never retry it.
        plan = null;
        generation += 1;
        busy = true;
        try {
          return await invoke("apply_deployment_plan", { planId, confirmed: true });
        } finally {
          busy = false;
        }
      },
    };
  }

  if (typeof module !== "undefined" && module.exports) module.exports = { createSession };
  else scope.PilotWeaveDeploymentReview = { createSession };
})(globalThis);
