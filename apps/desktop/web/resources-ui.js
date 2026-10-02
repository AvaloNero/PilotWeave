((scope) => {
  "use strict";
  const escape = (value) => String(value ?? "").replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#039;");
  const kindLabel = (kind) => ({ mcp: "MCP (HTTPS)", skill: "Skill", instructions: "Instructions" })[kind] ?? "Unsupported";
  const date = (value) => Number.isNaN(Date.parse(value)) ? "Unknown" : new Date(value).toLocaleString();
  let context;

  function render(overview, error, native, blocked = false) {
    const writable = native && !blocked;
    const records = overview?.resources ?? [];
    return `<section class="usage-panel"><div class="step-heading"><h2>Shared resources</h2><button class="button primary small" data-resource-action="add" ${!writable ? "disabled" : ""}>Add resource</button></div>
      <p class="install-copy">Author a resource locally, then review publication to public client paths. Secrets, local MCP commands, arbitrary paths, and client authentication are not imported. GitHub Copilot app, named profiles and custom roots remain manual.</p>
      ${!native ? '<p class="preview-banner">Unavailable in browser preview — no native catalog or client files are read.</p>' : ""}
      ${error ? `<p class="inline-error">${escape(error)}</p>` : ""}
      ${blocked ? '<p class="inline-error">Managed writes are disabled during recovery. Review recovery before editing or publishing resources.</p>' : ""}
      ${overview?.detail ? `<p class="muted">${escape(overview.detail)}</p>` : ""}
      ${native && !overview ? '<p class="usage-empty">Resource storage is unavailable.</p>' : ""}
      ${native && overview && records.length === 0 ? '<p class="usage-empty">No authored resources yet. Saving a resource does not publish it.</p>' : ""}
      <div class="client-grid">${records.map((record) => {
        const bindings = (overview.bindings ?? []).filter((binding) => binding.resourceId === record.id);
        const unpublished = bindings.some((binding) => Date.parse(binding.deployedAt) < Date.parse(record.updatedAt));
        return `<article class="client-card resource-card"><h3>${escape(record.name)}</h3><p>${escape(kindLabel(record.kind))} · ${bindings.length ? `${bindings.length} last-published binding(s)` : "Not published"}${unpublished ? " · Local changes need publication" : ""}</p>
          <p class="muted">${bindings.map((binding) => `${escape(binding.targetId)} · ${escape(date(binding.deployedAt))}`).join("<br>") || "No client configuration has been written."}</p>
          <div class="install-panel-actions"><button class="button ghost small" data-resource-action="edit" data-resource-id="${escape(record.id)}" ${!writable ? "disabled" : ""}>Edit</button><button class="button small" data-resource-action="publish" data-resource-id="${escape(record.id)}" ${!writable ? "disabled" : ""}>Preview publication</button><button class="button ghost small" data-resource-action="remove" data-resource-id="${escape(record.id)}" ${!writable ? "disabled" : ""}>Revoke and delete…</button></div></article>`;
      }).join("")}</div></section>`;
  }

  function renderPlan(plan) {
    const labels = { create: "Create owned projection", update: "Update owned projection", remove: "Remove owned projection", unchanged: "No change needed", manual: "Manual / unavailable" };
    return `<ul class="resource-plan-operations">${plan.operations.map((op) => `<li><strong>${escape(op.title)}</strong> · ${escape(labels[op.change] ?? "Unsupported change")}
      ${op.destination ? `<p class="muted"><code>${escape(op.destination)}</code></p>` : ""}<p>${escape(op.detail)}</p>
      ${op.contentPreview != null ? `<details><summary>Reviewed owned content</summary><pre class="usage-code">${escape(op.contentPreview)}</pre></details>` : ""}</li>`).join("")}</ul>${plan.operations.length ? "" : "No published bindings exist; only the local catalog entry will be removed."}`;
  }

  function editor(record) {
    const native = context.native;
    if (!native) return;
    context.openModal({ title: record ? "Edit authored resource" : "Add authored resource", wide: true,
      body: `<form id="resource-form"><label class="field-label">Name<input class="text-input" name="name" maxlength="128" required value="${escape(record?.name)}"></label><label class="field-label">Kind<select class="text-input" name="kind" ${record ? "disabled" : ""}>${["instructions", "skill", "mcp"].map((kind) => `<option value="${kind}" ${record?.kind === kind ? "selected" : ""}>${kindLabel(kind)}</option>`).join("")}</select></label><label class="field-label">Content or credential-free HTTPS MCP URL<textarea class="text-input" name="body" rows="12" maxlength="65536" required>${escape(record?.body)}</textarea></label><p class="muted">Instructions apply to all files. Skills get native-generated name/description frontmatter; enter their Markdown body here. Supporting scripts/assets are managed manually. MCP headers, keys, URL credentials and query strings are not supported. Saving does not write client files.</p></form>`,
      footer: '<button class="button ghost" data-resource-close>Cancel</button><button class="button primary" id="save-resource">Save locally</button>',
      onOpen(root) {
        root.querySelector("[data-resource-close]").addEventListener("click", context.closeModal);
        const save = root.querySelector("#save-resource");
        save.addEventListener("click", async () => {
          if (save.disabled) return;
          const form = root.querySelector("#resource-form"); if (!form.reportValidity()) return;
          save.disabled = true;
          try {
            await context.invoke("upsert_resource", { input: { id: record?.id ?? null, name: form.elements.name.value, kind: form.elements.kind.value, body: form.elements.body.value } });
            if (root.isConnected) context.closeModal();
            context.showToast("Resource saved locally; review publication separately"); await context.refresh();
          } catch (error) { context.showToast(error?.message ?? String(error), "error"); }
          finally { save.disabled = false; }
        });
      },
    });
  }

  function review(record, revokeAndDelete) {
    if (!context.native) return;
    context.openModal({ title: revokeAndDelete ? "Review resource revocation" : "Review resource publication", wide: true,
      body: `<p>${escape(record.name)}. Native ownership and live fingerprints will be rechecked. This preview expires after 15 minutes and is single-use. ${revokeAndDelete ? "Only proven owned configuration is removed; user entries remain." : "Only supported targets are written. Manual and unsupported clients are not counted as synchronized."}</p><div id="resource-plan" role="status">Generate a preview to review every target.</div>`,
      footer: '<button class="button ghost" data-resource-close>Cancel</button><button class="button primary" id="review-resource">Preview changes</button>',
      onOpen(root) {
        root.querySelector("[data-resource-close]").addEventListener("click", context.closeModal);
        const session = new scope.PilotWeavePlanSession(); const action = root.querySelector("#review-resource");
        action.addEventListener("click", async () => {
          if (action.disabled) return; action.disabled = true;
          try {
            if (!session.plan) {
              const generation = session.begin();
              const plan = await context.invoke("preview_resource_sync", { resourceId: record.id, revokeAndDelete });
              if (!root.isConnected || !session.accept(generation, plan)) return;
              root.querySelector("#resource-plan").innerHTML = renderPlan(plan);
              action.textContent = revokeAndDelete ? "Confirm revoke and delete" : "Confirm publication";
            } else {
              const args = session.consume(true);
              await context.invoke("apply_resource_plan", args);
              if (root.isConnected) context.closeModal();
              context.showToast(revokeAndDelete ? "Owned resource configuration removed" : "Published to supported targets; manual clients still require setup");
              await context.refresh();
            }
          } catch (error) { session.invalidate(); action.textContent = "Preview changes"; if (root.isConnected) root.querySelector("#resource-plan").textContent = error?.message ?? String(error); context.showToast(error?.message ?? String(error), "error"); }
          finally { action.disabled = false; }
        });
      },
    });
  }

  function configure(value) {
    context = value;
    document.addEventListener("click", (event) => {
      const button = event.target.closest("[data-resource-action]"); if (!button || !context.native) return;
      const action = button.dataset.resourceAction;
      if (action === "add") { editor(null); return; }
      const record = context.getData()?.resources?.resources?.find((record) => record.id === button.dataset.resourceId);
      if (!record) return;
      if (action === "edit") editor(record);
      else review(record, action === "remove");
    });
  }
  if (typeof module !== "undefined" && module.exports) module.exports = { render, renderPlan, kindLabel };
  else scope.PilotWeaveResources = { render, configure };
})(globalThis);
