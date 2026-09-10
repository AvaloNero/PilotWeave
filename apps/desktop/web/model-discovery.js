(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.ModelDiscovery = api;
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
  "use strict";
  const LIMIT = 128;

  function parseHeaders(text) {
    const headers = Object.create(null);
    const seen = new Set();
    for (const line of text.split(/\r?\n/).map(s => s.trim()).filter(Boolean)) {
      const colon = line.indexOf(":");
      if (colon <= 0) throw new Error("Each request header needs a name and colon.");
      const name = line.slice(0, colon).trim();
      if (seen.has(name.toLowerCase())) throw new Error("Duplicate request header names are not allowed.");
      seen.add(name.toLowerCase());
      headers[name] = line.slice(colon + 1).trim();
    }
    return headers;
  }

  function modelIds(text) {
    return new Set(text.split(/\r?\n/).map(line => line.split("|")[0].trim().toLowerCase()).filter(Boolean));
  }

  function mergeModels(text, selected) {
    const ids = modelIds(text);
    const added = [];
    for (const model of selected) {
      const id = model.modelId;
      const name = model.name;
      if (typeof id !== "string" || typeof name !== "string" || !id.trim() || /[\s|]/.test(id) || /[\r\n|]/.test(name)) {
        throw new Error("The catalog contains an invalid model entry.");
      }
      if (ids.has(id.toLowerCase())) continue;
      if (ids.size >= LIMIT) throw new Error(`A connection can contain at most ${LIMIT} models. Select fewer models.`);
      ids.add(id.toLowerCase());
      added.push(`${id} | ${name}`);
    }
    return added.length ? [text.trimEnd(), ...added].filter(Boolean).join("\n") : text;
  }

  // Edits invalidate the response immediately, including while a native request runs.
  function session(invoke, publish) {
    let version = 0, disposed = false, running = false;
    return {
      invalidate() { version++; },
      dispose() { disposed = true; version++; },
      get running() { return running; },
      async fetch(input) {
        if (disposed || running) return false;
        const current = ++version;
        running = true;
        try {
          const result = await invoke("discover_connection_models", { input });
          if (!disposed && current === version) publish(result);
        } catch {
          if (!disposed && current === version) publish({ status: "networkError", models: [], detail: "Model discovery is unavailable. Retry or enter model IDs manually." });
        } finally { running = false; }
        return true;
      },
    };
  }

  function mount(form, { invoke, native, connection }) {
    const panel = form.querySelector("#model-discovery");
    const status = panel.querySelector("[data-model-status]");
    const fetchButton = panel.querySelector("[data-model-fetch]");
    const results = panel.querySelector("[data-model-results]");
    const list = panel.querySelector("[data-model-list]");
    const search = panel.querySelector("[data-model-search]");
    const addButton = panel.querySelector("[data-model-add]");
    const modelsField = form.elements.models;
    const discoveredIds = new Set();
    let catalog = [], selected = new Set(), timer, disposed = false, changed = false;
    const text = message => { status.textContent = message; };
    function render() {
      const existing = modelIds(modelsField.value);
      const filter = search.value.trim().toLowerCase();
      list.replaceChildren();
      for (const model of catalog) {
        if (!`${model.modelId} ${model.name}`.toLowerCase().includes(filter)) continue;
        const label = document.createElement("label");
        const checkbox = document.createElement("input");
        checkbox.type = "checkbox";
        checkbox.disabled = existing.has(model.modelId.toLowerCase());
        checkbox.checked = checkbox.disabled || selected.has(model.modelId);
        checkbox.addEventListener("change", () => {
          if (checkbox.checked) selected.add(model.modelId); else selected.delete(model.modelId);
          updateAdd();
        });
        const name = document.createElement("span");
        name.textContent = model.name === model.modelId ? model.modelId : `${model.name} — ${model.modelId}`;
        label.append(checkbox, name);
        list.append(label);
      }
      updateAdd();
    }
    function updateAdd() {
      const existing = modelIds(modelsField.value);
      for (const id of selected) if (existing.has(id.toLowerCase())) selected.delete(id);
      addButton.disabled = selected.size === 0;
      addButton.textContent = `Add selected (${selected.size})`;
    }
    const job = session(invoke, result => {
      if (!form.isConnected) return;
      catalog = result.models ?? [];
      selected.clear();
      results.hidden = catalog.length === 0;
      panel.dataset.status = result.status;
      text(`${catalog.length ? `${catalog.length} models found. ` : ""}${result.detail}`);
      render();
    });
    function ready() {
      const url = form.elements.baseUrl;
      return url.value.trim() && url.checkValidity();
    }
    async function fetch(manual = false) {
      if (disposed || !form.isConnected || !native || !ready()) return;
      if (job.running) { if (manual) text("A model query is already running. Please wait."); return; }
      if (!manual && !form.elements.apiKey.value && !connection?.hasSecret && form.elements.providerKind.value !== "local") return;
      let headers;
      try { headers = parseHeaders(form.elements.headers.value); }
      catch (error) { text(error.message); return; }
      changed = false;
      fetchButton.disabled = true;
      panel.dataset.status = "loading";
      text("Fetching the provider's model catalog…");
      await job.fetch({
        connectionId: connection?.id ?? null,
        baseUrl: form.elements.baseUrl.value.trim(),
        providerKind: form.elements.providerKind.value,
        protocol: form.elements.protocol.value,
        headers,
        apiKey: form.elements.apiKey.value || null,
        clearSecret: form.elements.clearSecret?.checked ?? false,
      });
      if (!disposed && form.isConnected) {
        fetchButton.disabled = false;
        if (changed) schedule();
      }
    }
    function schedule() {
      clearTimeout(timer);
      timer = setTimeout(() => {
        // Wait until the user leaves credential/URL/header editing; never send partial keys.
        if (![form.elements.baseUrl, form.elements.apiKey, form.elements.headers].includes(document.activeElement)) void fetch();
      }, 600);
    }
    function invalidate() {
      job.invalidate(); changed = true;
      catalog = []; selected.clear(); results.hidden = true;
      panel.dataset.status = "idle";
      text(native ? "Finish editing the URL and key to fetch models automatically, or use Fetch models. Existing models are kept." : "Model discovery is available in the desktop app. Enter model IDs manually in browser preview.");
    }
    const fields = ["baseUrl", "apiKey", "providerKind", "protocol", "headers", "clearSecret"].map(name => form.elements[name]).filter(Boolean);
    for (const field of fields) {
      field.addEventListener("input", () => { invalidate(); schedule(); });
      field.addEventListener("change", () => { invalidate(); schedule(); });
      field.addEventListener("blur", schedule);
    }
    fetchButton.addEventListener("click", () => { clearTimeout(timer); void fetch(true); });
    search.addEventListener("input", render);
    modelsField.addEventListener("input", render);
    addButton.addEventListener("click", () => {
      try {
        const additions = catalog.filter(model => selected.has(model.modelId));
        modelsField.value = mergeModels(modelsField.value, additions);
        for (const model of additions) discoveredIds.add(model.modelId);
        selected.clear(); render();
        text("Selected models added below. Review them, then save the connection.");
      } catch (error) { text(error.message); }
    });
    invalidate(); changed = false;
    fetchButton.disabled = !native;
    return { discoveredIds, dispose() { disposed = true; clearTimeout(timer); job.dispose(); } };
  }
  return { parseHeaders, modelIds, mergeModels, session, mount };
});
