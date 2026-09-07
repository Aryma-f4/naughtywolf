(() => {
  const isSPALink = (a) => {
    const href = (a.getAttribute("href") || "").trim();
    if (!href.startsWith("/")) return false;
    if (href === "/login" || href === "/logout") return false;
    if (href.includes("/payloads/download/")) return false;
    return true;
  };

  let buildTimer = null;
  let buildDismissed = false;

  const entryAnims = () => {
    if (typeof anime === "undefined" || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    anime({ targets: document.querySelector(".page-heading"), translateY: [-10, 0], opacity: [0, 1], easing: "easeOutCubic", duration: 380, delay: 60 });
    anime({ targets: ".portal-main > :not(.page-heading)", translateY: [12, 0], opacity: [0, 1], easing: "easeOutCubic", duration: 420, delay: anime.stagger(40, { start: 110 }) });
  };

  const initMain = () => {
    document.querySelectorAll("[data-dismiss]").forEach((btn) => {
      btn.addEventListener("click", () => btn.closest("[data-dismissible]")?.remove());
    });
    bindPayloadForm();
  };

  const bindPayloadForm = () => {
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
    const rnd = document.getElementById("payload-psk-random");
    if (rnd) {
      rnd.addEventListener("click", () => {
        const p = document.getElementById("payload-psk");
        if (p) p.value = crypto.randomUUID().replace(/-/g, "") + crypto.randomUUID().slice(0, 12);
      });
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
      navigate(window.location.pathname + window.location.search, true);
    }, 3000);
  };

  const updateActive = (href) => {
    const path = href.split("?")[0];
    document.querySelectorAll(".nav-link, .quick-link").forEach((link) => {
      const match = (link.getAttribute("href") || "").split("?")[0] === path;
      if (match) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    });
  };

  async function navigate(href, silent) {
    const res = await fetch(href, { headers: { "X-Requested-With": "naughtywolf" }, credentials: "same-origin" });
    if (!res.ok) { window.location.href = href; return; }
    if (new URL(res.url).pathname === "/login") { window.location.href = "/login"; return; }
    const html = await res.text();
    const doc = new DOMParser().parseFromString(html, "text/html");
    const nextMain = doc.querySelector("main");
    const currentMain = document.querySelector("main");
    if (nextMain && currentMain) currentMain.replaceWith(nextMain);
    const t = doc.querySelector("title")?.textContent;
    if (t) document.title = t;
    updateActive(href);
    window.scrollTo({ top: 0 });
    history.pushState({}, "", href);
    initMain();
    if (!silent) entryAnims();
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
      navigate(window.location.pathname + window.location.search, true);
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
    if (!form) return;

    const sseEndpoint = form.dataset.sseEndpoint || null;
    const logEl = document.getElementById("results-log");

    // SSE connection for real-time task results.
    if (sseEndpoint && logEl) {
      const evtSource = new EventSource(sseEndpoint);
      evtSource.addEventListener("message", (event) => {
        const data = JSON.parse(event.data);
        const entry = document.createElement("div");
        entry.className = "entry";
        const meta = document.createElement("div");
        meta.className = "meta";
        meta.textContent = `${data.command} - ${data.status} at ${data.completed_at}`;
        const output = document.createElement("div");
        output.className = "output";
        output.textContent = data.output || "(no output)";
        entry.append(meta, output);
        logEl.prepend(entry);
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

        // Optimistically add the task to the results stream.
        const resultsStream = document.querySelector(".results-stream");
        if (resultsStream) {
          const placeholder = document.createElement("div");
          placeholder.className = "entry";
          // data.command and data.status come from our own JSON API response (same-origin),
          // not from raw user HTML — safe to set as text.
          const meta = document.createElement("div");
          meta.className = "meta";
          meta.textContent = `${data.command} - ${data.status}`;
          placeholder.appendChild(meta);
          resultsStream.prepend(placeholder);
        }
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
    const history = [];
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
          history.unshift(cmdInput.value);
          if (history.length > 50) history.pop();
          historyIndex = -1;
        }
      });
    }
  };

  initMain();
  initCallbackSSE();
  initCommandHistory();
  pollBuilds();
  entryAnims();
})();
