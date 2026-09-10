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

  const entryAnims = () => {
    window.NWMotion?.page(document.querySelector("main"));
  };

  const initMain = () => {
    document.querySelectorAll("[data-dismiss]").forEach((btn) => {
      btn.addEventListener("click", () => btn.closest("[data-dismissible]")?.remove());
    });
    bindPayloadForm();
    window.NWModuleStudio?.init(document.querySelector("main"));
    window.NWCallbackWorkspace?.init(document.querySelector("main"));
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

  window.NW = { navigate };
  initMain();
  pollBuilds();
  entryAnims();
})();
