(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.NWModuleStudio = api;
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
  "use strict";

  const allowed = new Set(["nw/user-enum", "nw/peas-audit", "nw/exec-code"]);

  function encodeUtf8Base64(source) {
    if (typeof Buffer !== "undefined") return Buffer.from(source, "utf8").toString("base64");
    const bytes = new TextEncoder().encode(source);
    let binary = "";
    for (let i = 0; i < bytes.length; i += 0x8000) {
      binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    }
    return btoa(binary);
  }

  function buildModuleTask(command, timeout, language, source) {
    if (!allowed.has(command)) throw new Error("unsupported module command");
    const timeoutMs = Number(timeout);
    if (!Number.isFinite(timeoutMs) || timeoutMs < 1000 || timeoutMs > 300000) {
      throw new Error("invalid module timeout");
    }
    if (command !== "nw/exec-code") return { command, args: [], timeout_ms: timeoutMs };
    const size = new TextEncoder().encode(source || "").length;
    if (!size) throw new Error("custom source is required");
    if (size > 24576) throw new Error("custom source exceeds the 24 KiB limit");
    if (!["shell", "python", "powershell"].includes(language)) throw new Error("unsupported interpreter");
    return { command, args: [language, encodeUtf8Base64(source)], timeout_ms: timeoutMs };
  }

  async function submit(studio, task, button) {
    const status = studio.querySelector("[data-module-status]");
    const label = button?.textContent;
    if (button) { button.disabled = true; button.textContent = "Queueing…"; }
    if (status) status.textContent = `Queueing ${task.command}…`;
    try {
      const response = await fetch(studio.dataset.taskEndpoint, {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-CSRF-Token": studio.dataset.csrfToken },
        body: JSON.stringify(task),
      });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const result = await response.json();
      if (status) status.textContent = `${result.command || task.command} queued. Output will appear in the task ledger.`;
      studio.dispatchEvent(new CustomEvent("nw:task-submitted", { bubbles: true, detail: result }));
    } catch (error) {
      if (status) status.textContent = `Unable to queue module: ${error.message}`;
    } finally {
      if (button) { button.disabled = false; button.textContent = label; }
    }
  }

  function init(scope) {
    const studio = scope?.querySelector?.("[data-module-studio]");
    if (!studio || studio.dataset.bound === "true") return;
    studio.dataset.bound = "true";
    studio.querySelectorAll("[data-module-command]").forEach((button) => {
      button.addEventListener("click", () => submit(studio, buildModuleTask(button.dataset.moduleCommand, button.dataset.moduleTimeout), button));
    });
    const toggle = studio.querySelector("[data-code-toggle]");
    const form = studio.querySelector("[data-custom-code-form]");
    toggle?.addEventListener("click", () => {
      const opening = form.hidden;
      form.hidden = !opening;
      toggle.setAttribute("aria-expanded", String(opening));
      toggle.textContent = opening ? "Close editor" : "Open editor";
      if (opening) form.querySelector("textarea")?.focus();
    });
    const source = form?.querySelector("[name=source]");
    const count = form?.querySelector("[data-source-count]");
    source?.addEventListener("input", () => { if (count) count.textContent = String(new TextEncoder().encode(source.value).length); });
    form?.addEventListener("submit", (event) => {
      event.preventDefault();
      try {
        const data = new FormData(form);
        const task = buildModuleTask("nw/exec-code", data.get("timeout"), data.get("language"), data.get("source"));
        submit(studio, task, form.querySelector("button[type=submit]"));
      } catch (error) {
        const status = studio.querySelector("[data-module-status]");
        if (status) status.textContent = error.message;
      }
    });
  }

  return { encodeUtf8Base64, buildModuleTask, init };
});
