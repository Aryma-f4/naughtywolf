/* Finite, interruptible Anime.js accents. Content stays visible without JavaScript. */
(() => {
  const engine = window.anime;
  if (!engine) return;
  const preference = window.matchMedia('(prefers-reduced-motion: reduce)');
  const active = new Map();
  const allowed = () => !preference.matches && !document.hidden;
  const restore = (node, record) => {
    for (const [name, value] of record.attributes) {
      if (value === null) node.removeAttribute(name); else node.setAttribute(name, value);
    }
    active.delete(node);
  };
  function stop(nodes) {
    engine.remove(nodes);
    for (const node of nodes) if (active.has(node)) restore(node, active.get(node));
  }
  function run(nodes, options) {
    const targets = [...nodes];
    if (!targets.length || !allowed()) return;
    stop(targets);
    const token = {};
    for (const node of targets) active.set(node, {
      token,
      attributes: ['style', 'stroke-dasharray', 'stroke-dashoffset'].map(name => [name, node.getAttribute(name)]),
    });
    engine({ targets, easing: 'easeOutCubic', ...options, complete: () => {
      for (const node of targets) {
        const record = active.get(node);
        if (record?.token === token) restore(node, record);
      }
    } });
  }
  function clearPage() {
    stop([...active.keys()].filter(node => node.closest('main')));
  }
  function draw(nodes, duration = 1000) {
    run(nodes, { strokeDashoffset: [engine.setDashoffset, 0], duration, delay: engine.stagger(85) });
  }
  function page(main) {
    clearPage();
    if (!main || !allowed()) return;
    run(main.querySelectorAll('.page-heading > *, .login-brand-copy, .login-card'), {
      opacity: [0, 1], translateY: [12, 0], duration: 650, delay: engine.stagger(65),
    });
    const sections = main.querySelectorAll('.overview-hero, .metric-card, .field-map, .field-note, .topology-toolbar, .graph-summary, .recon-heading, .recon-stats, .recon-launch, .recon-history, .table-wrap, .form-panel, .empty-state');
    run([...sections].slice(0, 24), {
      opacity: [0, 1], translateY: [16, 0], duration: 650,
      delay: (_, index) => 100 + Math.min(index * 45, 360),
    });
    draw(main.querySelectorAll('.orbit-core path, .login-orbit path'), 1500);
    // Preserve the CSS centering transform while rotating the decorative ring.
    run(main.querySelectorAll('.orbit-ring-middle'), {
      translateX: '-50%', translateY: '-50%', rotate: [-30, 35, -30],
      duration: 2400, easing: 'easeInOutSine',
    });
    run(main.querySelectorAll('.orbit-point'), {
      scale: [1, 1.35, 1], opacity: [.6, 1, 1], duration: 1800,
      delay: engine.stagger(160), easing: 'easeInOutSine',
    });
  }
  function graph(layer) {
    const cards = layer.querySelectorAll('.node-card');
    // Dense inventories remain immediately usable; filters provide focus.
    if (cards.length > 80 || !allowed()) return;
    run(layer.querySelectorAll('.graph-edge'), {
      strokeDashoffset: [engine.setDashoffset, 0], duration: 750,
      delay: (_, index) => Math.min(index * 22, 260),
    });
    run(cards, { opacity: [0, 1], duration: 500, delay: (_, index) => Math.min(index * 20, 300) });
  }
  function detail(container) {
    run(container.children, { opacity: [0, 1], translateY: [5, 0], duration: 320, delay: engine.stagger(25) });
  }
  function enter(event) {
    if (!allowed() || (event.pointerType && event.pointerType !== 'mouse')) return;
    const brand = event.target.closest('.portal-brand, .brand-lockup, .orbit-core');
    if (brand && !brand.contains(event.relatedTarget)) draw(brand.querySelectorAll('path'), 800);
    const card = event.target.closest('.metric-card, .workflow-steps > a, .primary-action, .text-action');
    if (!card || card.contains(event.relatedTarget)) return;
    const arrow = card.querySelector('.metric-link span, .step-symbol svg, span[aria-hidden="true"]');
    if (arrow) run([arrow], { translateX: [0, 4, 0], duration: 520, easing: 'easeInOutSine' });
  }
  document.addEventListener('pointerover', enter);
  document.addEventListener('focusin', enter);
  preference.addEventListener('change', () => { if (preference.matches) stop([...active.keys()]); });
  document.addEventListener('visibilitychange', () => { if (document.hidden) stop([...active.keys()]); });
  window.addEventListener('pagehide', () => stop([...active.keys()]));
  window.NWMotion = { page, graph, detail, clearPage, clearGraph: layer => stop([...active.keys()].filter(node => layer.contains(node))) };
  const login = document.querySelector('.login-shell');
  if (login) page(login);
  // The brand draws once per document, never on content/menu navigation.
  draw(document.querySelectorAll('.portal-brand .brand-mark path, .brand-lockup .brand-mark path'), 1200);
})();
