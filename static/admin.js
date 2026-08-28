(() => {
  const dock = document.querySelector("[data-dock]");
  if (!dock) return;

  const storageKey = "naughtywolf-dock-position";
  const savedPosition = window.localStorage.getItem(storageKey);
  if (savedPosition) dock.dataset.position = savedPosition;

  dock.addEventListener("click", (event) => {
    const link = event.target.closest("[data-dock-position]");
    if (link) window.localStorage.setItem(storageKey, link.dataset.dockPosition);
  });
})();
