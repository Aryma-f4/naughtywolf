(() => {
  const isSPALink = (a) => {
    const href = (a.getAttribute("href") || "").trim();
    if (!href.startsWith("/") || href.startsWith("//")) return false;
    if (a.hasAttribute("download")) return false;
    const target = a.getAttribute("target");
    if (target && target.toLowerCase() !== "_self") return false;
    const path = new URL(href, window.location.href).pathname;
    if (path === "/login" || path === "/logout") return false;
    if (path.includes("/payloads/download/") || /^\/evidence\/[^/]+\/download$/.test(path)) return false;
    return true;
  };

  let buildTimer = null;
  let buildDismissed = false;
  let taskEventSource = null;

  const entryAnims = () => {
    window.NWMotion?.page(document.querySelector("main"));
  };

  const initMain = () => {
    document.querySelectorAll("[data-dismiss]").forEach((btn) => {
      btn.addEventListener("click", () => btn.closest("[data-dismissible]")?.remove());
    });
    bindPayloadForm();
    window.NWModuleStudio?.init(document.querySelector("main"));
    initCallbackSSE();
    initCommandHistory();
    window.NWWorkspace?.init(document.querySelector("main"));
  };

  const bindPayloadForm = () => {
    window.NWPayloadWizard?.init(document.querySelector("main"));
    const plat = document.getElementById("payload-platform");
    if (plat) {
      const os = document.getElementById("payload-os");
      const arch = document.getElementById("payload-arch");
      const target = document.getElementById("payload-target");
      const hint = document.getElementById("payload-platform-hint");
      const sync = () => {
        const opt = plat.options[plat.selectedIndex];
        os.value = opt.dataset.os;
        arch.value = opt.dataset.arch;
        target.value = opt.value;
        if (hint) hint.textContent = opt.value ? `${opt.dataset.os}/${opt.dataset.arch} (${opt.value})` : `native (${opt.dataset.os}/${opt.dataset.arch})`;
      };
      plat.addEventListener("change", sync);
      sync();
    }
  };

  const buildCard = (() => {
    const el = document.createElement("div");
    el.id = "build-card";
    el.setAttribute("role", "status");
    el.innerHTML = '<div class="spinner"></div><div class="build-card-body"><strong>Compiling implant&hellip;</strong><span class="muted">This can take a minute. Close it to keep browsing.</span></div><button type="button" class="dismiss-btn" data-dismiss-card="true" aria-label="Dismiss">&times;</button>';
    el.querySelector("[data-dismiss-card]").addEventListener("click", () => {
      buildDismissed = true;
      el.remove();
      if (buildTimer) { clearInterval(buildTimer); buildTimer = null; }
    });
    return el;
  })();
  const showBuildCard = () => { if (!buildCard.isConnected) document.body.appendChild(buildCard); };
  const hideBuildCard = () => { if (buildCard.isConnected) buildCard.remove(); };

  const pollBuilds = () => {
    const banner = document.querySelector("[data-building='true']");
    if (!banner || buildDismissed) { hideBuildCard(); return; }
    showBuildCard();
    if (buildTimer) return;
    buildTimer = setInterval(() => {
      if (buildDismissed) { hideBuildCard(); clearInterval(buildTimer); buildTimer = null; return; }
      if (!document.querySelector("[data-building='true']")) {
        hideBuildCard();
        clearInterval(buildTimer);
        buildTimer = null;
        return;
      }
      navigate(window.location.pathname + window.location.search, { historyMode: "none", silent: true, background: true });
    }, 3000);
  };

  const updateActive = (doc) => {
    const activeHrefs = new Set([...doc.querySelectorAll('.nav-link[aria-current="page"], .quick-link[aria-current="page"]')]
      .map(link => link.getAttribute("href")));
    document.querySelectorAll(".nav-link, .quick-link").forEach((link) => {
      if (activeHrefs.has(link.getAttribute("href"))) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    });
  };

  const showNavigationError = (href) => {
    document.getElementById("navigation-error")?.remove();
    const notice = document.createElement("div");
    notice.id = "navigation-error";
    notice.className = "navigation-error";
    notice.setAttribute("role", "alert");
    const message = document.createElement("span");
    message.textContent = "Couldn't load that page. Your current view is still available.";
    const retry = document.createElement("a");
    retry.href = href;
    retry.textContent = "Try again";
    const dismiss = document.createElement("button");
    dismiss.type = "button";
    dismiss.className = "dismiss-btn";
    dismiss.setAttribute("aria-label", "Dismiss navigation error");
    dismiss.textContent = "×";
    dismiss.addEventListener("click", () => notice.remove());
    notice.append(message, retry, dismiss);
    document.body.appendChild(notice);
  };

  let activeNavigation = null;
  async function navigate(href, { historyMode = "push", silent = false, background = false } = {}) {
    // Background refreshes must never interrupt an operator's menu selection.
    if (background && activeNavigation) return;
    activeNavigation?.abort();
    const controller = new AbortController();
    activeNavigation = controller;
    const timeout = setTimeout(() => controller.abort(), 15000);
    const currentMain = document.querySelector("main");
    if (!background) {
      currentMain?.setAttribute("aria-busy", "true");
      document.getElementById("navigation-error")?.remove();
    }
    try {
      const res = await fetch(href, {
        headers: { "X-Requested-With": "naughtywolf" },
        credentials: "same-origin",
        signal: controller.signal,
      });
      if (activeNavigation !== controller) return;
      if (res.status === 401 || new URL(res.url).pathname === "/login") {
        window.location.href = "/login";
        return;
      }
      if (!res.ok) throw new Error("Navigation request failed");
      if (!res.headers.get("content-type")?.includes("text/html")) {
        window.location.href = res.url;
        return;
      }
      const html = await res.text();
      if (activeNavigation !== controller) return;
      const doc = new DOMParser().parseFromString(html, "text/html");
      const nextMain = doc.querySelector("main");
      if (!nextMain || !currentMain) throw new Error("Page content is unavailable");
      window.NWMotion?.clearPage();
      currentMain.replaceWith(nextMain);
      const title = doc.querySelector("title")?.textContent;
      if (title) document.title = title;
      updateActive(doc);
      if (historyMode === "push" && href !== window.location.pathname + window.location.search + window.location.hash) {
        history.pushState({}, "", href);
      }
      initMain();
      if (!background) {
        window.scrollTo({ top: 0 });
        nextMain.setAttribute("tabindex", "-1");
        nextMain.focus({ preventScroll: true });
      }
      if (!silent) entryAnims();
    } catch (_) {
      if (activeNavigation === controller && !background) showNavigationError(href);
    } finally {
      clearTimeout(timeout);
      if (activeNavigation === controller) {
        document.querySelector("main")?.removeAttribute("aria-busy");
        activeNavigation = null;
      }
    }
  }

  document.addEventListener("click", (e) => {
    if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return;
    const a = e.target.closest("a");
    if (a && isSPALink(a)) {
      e.preventDefault();
      navigate(a.getAttribute("href"));
    }
  });

  window.addEventListener("popstate", () => {
    if (window.location.pathname !== "/login" && window.location.pathname !== "/logout") {
      navigate(window.location.pathname + window.location.search, { historyMode: "none", silent: true });
    }
  });

  const nav = document.querySelector("[data-primary-nav]");
  const active = nav?.querySelector('[aria-current="page"]');
  active?.scrollIntoView({ block: "nearest", inline: "center" });

  const topbar = document.querySelector("[data-topbar]");
  if (topbar) {
    const syncTopbar = () => { topbar.dataset.scrolled = window.scrollY > 8 ? "true" : "false"; };
    syncTopbar();
    window.addEventListener("scroll", syncTopbar, { passive: true });
  }

  /* ── Callback detail page (Mythic-style) ────────────────────────────── */
  const initCallbackSSE = () => {
    const form = document.getElementById("task-form");
    if (!form) {
      taskEventSource?.close();
      taskEventSource = null;
      return;
    }
    if (form.dataset.bound === "true") return;
    form.dataset.bound = "true";

    const sseEndpoint = form.dataset.sseEndpoint || null;
    const logEl = document.getElementById("results-log");

    const taskElement = (root, taskId) => [...root.querySelectorAll("[data-task-id]")]
      .find((element) => element.dataset.taskId === taskId);

    const upsertTaskResult = (data) => {
      if (!logEl || !data.id) return;
      logEl.querySelector(".results-placeholder")?.remove();
      let entry = taskElement(logEl, data.id);
      if (!entry) {
        entry = document.createElement("div");
        entry.className = "entry";
        entry.dataset.taskId = data.id;
        entry.append(document.createElement("div"), document.createElement("div"));
        entry.children[0].className = "meta";
        entry.children[1].className = "output";
        logEl.prepend(entry);
      }
      entry.children[0].textContent = `${data.command} - ${data.status}${data.completed_at ? ` at ${data.completed_at}` : ""}`;
      entry.children[1].textContent = data.output || (data.status === "pending" ? "Waiting for callback…" : "(no output)");
      entry.dataset.taskStatus = data.status;

      const ledgerRow = taskElement(document, data.id);
      if (ledgerRow && ledgerRow !== entry) {
        const state = ledgerRow.querySelector("[data-task-state]");
        const output = ledgerRow.querySelector("[data-task-output]");
        if (state) {
          state.textContent = "";
          const pill = document.createElement("span");
          pill.className = `status-pill status-${data.status === "completed" ? "success" : "danger"}`;
          pill.textContent = data.status === "completed" ? "Completed" : "Error";
          state.appendChild(pill);
        }
        if (output) output.textContent = data.output || "—";
      }
    };

    // SSE connection for real-time task results.
    if (sseEndpoint && logEl) {
      taskEventSource?.close();
      const evtSource = new EventSource(sseEndpoint);
      taskEventSource = evtSource;
      evtSource.addEventListener("message", (event) => {
        try {
          upsertTaskResult(JSON.parse(event.data));
        } catch (_) { /* ignore malformed events and keep the stream alive */ }
      });
      evtSource.onerror = () => { /* silently reconnect */ };
    }

    // Auto-submit task form via fetch (Mythic-style: stay on page).
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const formData = new FormData(form);
      const command = formData.get("command") || "";
      const argsStr = formData.get("args") || "";
      const args = argsStr ? argsStr.split(" ").filter((a) => a.trim()) : [];
      const csrfToken = formData.get("csrf_token") || "";

      try {
        const res = await fetch(form.action, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "X-CSRF-Token": csrfToken,
          },
          body: JSON.stringify({ command, args, timeout_ms: 30000 }),
        });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();

        upsertTaskResult(data);
      } catch (err) {
        console.error("Task submission failed:", err);
      }
    });

    // Refresh button reloads tasks.
    const refreshBtn = document.getElementById("refresh-btn");
    if (refreshBtn) {
      refreshBtn.addEventListener("click", () => window.location.reload());
    }
  };

  /* ── Command history (Mythic-style: up/down arrow in command input) ─── */
  const initCommandHistory = () => {
    const cmdInput = document.getElementById("command");
    if (!cmdInput) return;
    const history = [...document.querySelectorAll("[data-task-command]")]
      .map((row) => row.dataset.taskCommand)
      .filter(Boolean)
      .reverse();
    let historyIndex = -1;
    cmdInput.addEventListener("keydown", (e) => {
      if (e.key === "ArrowUp") {
        e.preventDefault();
        if (historyIndex < history.length - 1) {
          historyIndex++;
          cmdInput.value = history[history.length - 1 - historyIndex] || "";
        }
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        if (historyIndex > 0) {
          historyIndex--;
          cmdInput.value = history[history.length - 1 - historyIndex] || "";
        } else if (historyIndex === 0) {
          historyIndex = -1;
          cmdInput.value = "";
        }
      }
    });
    // Capture submitted commands into history.
    const form = cmdInput.closest("form");
    if (form) {
      form.addEventListener("submit", () => {
        if (cmdInput.value.trim()) {
          history.push(cmdInput.value);
          if (history.length > 50) history.shift();
          historyIndex = -1;
        }
      });
    }
  };

  window.NW = { navigate };
  initMain();
  pollBuilds();
  entryAnims();
})();
