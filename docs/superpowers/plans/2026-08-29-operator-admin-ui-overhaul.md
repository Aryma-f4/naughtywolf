# Operator Admin UI Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the active portal's minimal login and admin presentation with a polished, responsive security-operations UI using centered top navigation on desktop and bottom navigation on mobile.

**Architecture:** Keep the existing Rust/axum server-rendered portal and its route/auth/data contracts. Change only the active templates, local stylesheet, and optional progressive enhancement script; do not edit or revive the legacy SPA under `static/app.html` and `static/style.css`.

**Tech Stack:** Rust 2024, axum HTML responses, hand-authored semantic HTML, local CSS, dependency-free progressive JavaScript, existing tower/route tests.

**Spec:** `docs/superpowers/specs/2026-08-29-operator-admin-ui-design.md`

## Global Constraints

- Active UI stack only: `src/portal/templates.rs`, `static/admin.css`, `static/admin.js`, `src/portal.rs`, and `tests/portal_routes_test.rs`.
- Do not edit `static/login.html`, `static/app.html`, `static/style.css`, or `static/app.js`; they belong to the inactive legacy SPA.
- No backend, database, authorization, CSRF, session, scoping, C2 protocol, or route behavior changes.
- No external CDN, font, icon, CSS, JavaScript, frontend framework, or build step.
- Desktop navigation is centered in the top header; mobile navigation is a fixed bottom dock; no left sidebar.
- UI remains fully functional without JavaScript.
- Preserve POST logout, visible labels, autocomplete, generic login errors, captions, header scopes, skip link, and `aria-current`.
- Minimum target size is 44×44 CSS pixels; support 320px viewport, reduced motion, keyboard focus, and print.
- Use only factual data already supplied by current handlers; do not add fake telemetry, charts, or security claims.
- The repository is already dirty and the prior handoff says not to commit unless the user explicitly authorizes it. Each task ends with a review checkpoint, not an automatic commit.

---

### Task 1: Lock the active UI contract and redesign login

**Files:**
- Modify: `tests/portal_routes_test.rs`
- Modify: `src/portal/templates.rs`
- Modify: `static/admin.css`

**Interfaces:**
- Consumes: `templates::page_shell(title, content)`, `templates::login_page(error, csrf_token)`, and `/static/admin.css` from `portal::public_router()`.
- Produces: stable classes `login-shell`, `login-brand-panel`, `login-form-panel`, `login-card`, `brand-mark`, `trust-list`, and `form-field` used by Task 5 visual QA.

- [ ] **Step 1: Add a failing route/template test for the active login markup**

Add this test beside `login_page_has_labeled_credentials_and_local_styles`:

```rust
#[test]
fn login_template_uses_the_operator_layout_without_external_assets() {
    let body = templates::login_page(Some("Invalid username or password"), "csrf-token");

    for class_name in [
        "login-shell",
        "login-brand-panel",
        "login-form-panel",
        "login-card",
        "brand-mark",
        "trust-list",
        "form-field",
    ] {
        assert!(body.contains(class_name), "missing {class_name}");
    }
    assert!(body.contains("Authorized lab access only"));
    assert!(body.contains("name=\"csrf_token\" value=\"csrf-token\""));
    assert!(body.contains("autocomplete=\"username\""));
    assert!(body.contains("autocomplete=\"current-password\""));
    assert!(body.contains("role=\"alert\""));
    assert!(!body.contains("https://"));
}
```

- [ ] **Step 2: Run the focused test and confirm the intended failure**

Run:

```bash
cargo test --test portal_routes_test login_template_uses_the_operator_layout_without_external_assets -- --nocapture
```

Expected: FAIL because `login-shell` is absent from the current minimal form markup.

- [ ] **Step 3: Replace only the login template body with the approved semantic structure**

Keep the existing escaped `error` string and produce the body with this exact structure:

```rust
let content = format!(
    r#"<main class="login-shell"><section class="login-brand-panel" aria-labelledby="login-brand-title"><div class="brand-lockup"><span class="brand-mark" aria-hidden="true">NW</span><span class="brand-name"><strong>NaughtyWolf</strong><small>Operator Portal</small></span></div><div class="login-brand-copy"><p class="eyebrow">Authorized Operations Workspace</p><h1 id="login-brand-title">Operate with scope, evidence, and accountability.</h1><p>Manage authorized lab records from one local control surface.</p></div><ul class="trust-list" aria-label="Portal capabilities"><li>Scoped operations</li><li>Append-only audit</li><li>Verified evidence</li></ul></section><section class="login-form-panel" aria-labelledby="login-title"><form class="login-card" method="post" action="/login"><p class="eyebrow">Local operator access</p><h2 id="login-title">Sign in</h2><p class="login-intro">Use your assigned local account.</p>{error}<input type="hidden" name="csrf_token" value="{csrf_token}"><div class="form-field"><label for="username">Username</label><input id="username" name="username" autocomplete="username" required></div><div class="form-field"><label for="password">Password</label><input id="password" name="password" type="password" autocomplete="current-password" required></div><button class="primary-action" type="submit">Enter workspace</button><p class="login-guardrail">Authorized lab access only</p></form></section></main>"#,
    error = error,
    csrf_token = escape_html(csrf_token),
);
page_shell("Sign in", &content)
```

- [ ] **Step 4: Implement the login portion of the local design system**

Replace the current light root tokens with these exact foundations, then style all login classes from Step 3 using them:

```css
:root {
  color-scheme: dark;
  --bg: #060a12;
  --surface: #0b1220;
  --surface-raised: #101a2b;
  --surface-soft: #0d1726;
  --text: #e5edf7;
  --muted: #8da0b8;
  --dim: #60728b;
  --border: #213047;
  --border-strong: #334a68;
  --accent: #35bdf6;
  --accent-strong: #0ea5e9;
  --accent-ink: #03111a;
  --success: #34d399;
  --warning: #fbbf24;
  --danger: #fb7185;
  --focus: #f8d66d;
  --radius-sm: 0.375rem;
  --radius-md: 0.75rem;
  --radius-lg: 1rem;
  --shadow: 0 1.5rem 4rem rgb(0 0 0 / 35%);
  --content-width: 86rem;
  --font-ui: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  --font-mono: "SFMono-Regular", Consolas, "Liberation Mono", monospace;
}
```

Required login behavior:

```css
.login-shell { min-height: 100svh; display: grid; grid-template-columns: minmax(0, 1.08fr) minmax(24rem, .92fr); background: var(--bg); }
.login-brand-panel { position: relative; display: flex; flex-direction: column; justify-content: space-between; min-height: 100%; padding: clamp(2rem, 6vw, 5rem); overflow: hidden; border-right: 1px solid var(--border); }
.login-brand-panel::before { content: ""; position: absolute; inset: 0; pointer-events: none; opacity: .22; background-image: linear-gradient(var(--border) 1px, transparent 1px), linear-gradient(90deg, var(--border) 1px, transparent 1px); background-size: 42px 42px; mask-image: linear-gradient(to bottom right, #000, transparent 78%); }
.login-form-panel { display: grid; place-items: center; padding: clamp(1.25rem, 5vw, 4rem); background: var(--surface); }
.login-card { position: relative; z-index: 1; display: grid; width: min(100%, 28rem); gap: 1rem; padding: clamp(1.5rem, 4vw, 2.5rem); border: 1px solid var(--border); border-radius: var(--radius-lg); background: var(--surface-raised); box-shadow: var(--shadow); }
@media (max-width: 767px) { .login-shell { grid-template-columns: 1fr; } .login-brand-panel { min-height: auto; gap: 2rem; padding: 1.5rem; border-right: 0; border-bottom: 1px solid var(--border); } .trust-list { display: none; } .login-form-panel { padding: 1.25rem; } }
```

Add these exact supporting rules:

```css
* { box-sizing: border-box; }
html, body { min-width: 320px; margin: 0; color: var(--text); background: var(--bg); font-family: var(--font-ui); line-height: 1.5; }
.brand-lockup { position: relative; z-index: 1; display: flex; align-items: center; gap: .75rem; }
.brand-mark { display: grid; width: 2.75rem; aspect-ratio: 1; place-items: center; border: 1px solid var(--accent); border-radius: var(--radius-sm); color: var(--accent); background: rgb(53 189 246 / 8%); font: 800 .85rem/1 var(--font-mono); letter-spacing: .08em; }
.brand-name { display: grid; line-height: 1.1; } .brand-name strong { letter-spacing: .03em; } .brand-name small { margin-top: .25rem; color: var(--muted); font-size: .7rem; letter-spacing: .12em; text-transform: uppercase; }
.login-brand-copy { position: relative; z-index: 1; max-width: 42rem; } .login-brand-copy h1 { max-width: 16ch; margin: .75rem 0 1rem; font-size: clamp(2.5rem, 6vw, 5.5rem); line-height: .98; letter-spacing: -.045em; } .login-brand-copy > p:last-child { max-width: 42rem; color: var(--muted); font-size: 1.05rem; }
.eyebrow { margin: 0; color: var(--accent); font: 700 .72rem/1.4 var(--font-mono); letter-spacing: .14em; text-transform: uppercase; }
.trust-list { position: relative; z-index: 1; display: flex; flex-wrap: wrap; gap: .65rem; margin: 0; padding: 0; list-style: none; } .trust-list li { padding: .45rem .65rem; border: 1px solid var(--border); border-radius: 999px; color: var(--muted); background: rgb(11 18 32 / 72%); font-size: .78rem; }
.login-card h2 { margin: 0; font-size: 2rem; } .login-intro, .login-guardrail { margin: 0; color: var(--muted); } .login-guardrail { padding-top: .25rem; font-size: .75rem; text-align: center; }
.form-field { display: grid; gap: .45rem; } label { color: var(--text); font-size: .82rem; font-weight: 700; }
input, select { width: 100%; min-height: 44px; padding: .7rem .8rem; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); color: var(--text); background: var(--bg); font: inherit; } input:hover, select:hover { border-color: var(--dim); } input:focus, select:focus { border-color: var(--accent); }
.primary-action, .button, button { display: inline-flex; min-height: 44px; align-items: center; justify-content: center; padding: .7rem 1rem; border: 1px solid var(--accent); border-radius: var(--radius-sm); color: var(--accent-ink); background: var(--accent); font: 800 .82rem/1 var(--font-ui); text-decoration: none; cursor: pointer; }
.primary-action:hover, .button:hover, button:hover { background: #7dd3fc; }
.form-error { margin: 0; padding: .75rem; border: 1px solid rgb(251 113 133 / 55%); border-left: 3px solid var(--danger); border-radius: var(--radius-sm); color: #fecdd3; background: rgb(251 113 133 / 8%); }
:where(a, button, input, select):focus-visible { outline: 3px solid var(--focus); outline-offset: 3px; }
```

- [ ] **Step 5: Run login and public-page tests**

Run:

```bash
cargo test --test portal_routes_test login -- --nocapture
cargo test public_pages_link_local_styles_and_do_not_expose_operator_console_copy -- --nocapture
```

Expected: all selected tests PASS; authentication error copy remains generic.

- [ ] **Step 6: Review checkpoint**

Inspect only this task's diff, record RED/GREEN evidence, and do not commit unless the user explicitly authorizes commits.

---

### Task 2: Build the centered top navigation and mobile bottom dock

**Files:**
- Modify: `tests/portal_routes_test.rs`
- Modify: `src/portal/templates.rs`
- Modify: `static/admin.css`
- Modify: `static/admin.js`

**Interfaces:**
- Consumes: `app_page(title, user, active_nav, body)` and the existing eight role-aware navigation tuples.
- Produces: `portal-shell`, `portal-topbar`, `primary-nav`, `nav-link`, `operator-identity`, `role-badge`, and `portal-main`; every authenticated page in Task 3 consumes this shell.

- [ ] **Step 1: Add failing shell-contract tests**

Add these tests using the existing `AuthenticatedUser` and `Role` imports:

```rust
#[test]
fn authenticated_shell_uses_top_navigation_and_post_logout() {
    let user = AuthenticatedUser {
        id: "operator-id".into(),
        username: "operator-user".into(),
        role: Role::Operator,
    };
    let body = templates::app_page("Dashboard", &user, "dashboard", "<p>body</p>");

    assert!(body.contains("class=\"portal-shell\""));
    assert!(body.contains("class=\"portal-topbar\""));
    assert!(body.contains("class=\"primary-nav\""));
    assert!(body.contains("aria-label=\"Primary navigation\""));
    assert!(body.contains("href=\"/dashboard\" aria-current=\"page\""));
    assert!(body.contains("method=\"post\" action=\"/logout\""));
    assert!(body.contains("operator-user"));
    assert!(!body.contains("sidebar"));
}

#[test]
fn admin_navigation_remains_role_scoped() {
    let viewer = AuthenticatedUser {
        id: "viewer-id".into(),
        username: "viewer-user".into(),
        role: Role::Viewer,
    };
    let admin = AuthenticatedUser {
        id: "admin-id".into(),
        username: "admin-user".into(),
        role: Role::Admin,
    };

    assert!(!templates::app_page("Dashboard", &viewer, "dashboard", "")
        .contains("href=\"/admin/users\""));
    assert!(templates::app_page("Admin", &admin, "admin", "")
        .contains("href=\"/admin/users\" aria-current=\"page\""));
}
```

- [ ] **Step 2: Run the shell tests and confirm failure**

Run:

```bash
cargo test --test portal_routes_test authenticated_shell_uses_top_navigation_and_post_logout -- --nocapture
cargo test --test portal_routes_test admin_navigation_remains_role_scoped -- --nocapture
```

Expected: the first test FAILS because `portal-shell` and `primary-nav` do not exist; the second FAILS because the active Admin anchor does not yet use the new `nav-link` contract.

- [ ] **Step 3: Replace `dock` markup generation with icon-plus-label navigation**

Keep the existing names, hrefs, ordering, and Admin role condition. Add this fixed icon helper; all paths are local inline SVG and never contain user input:

```rust
fn nav_icon(name: &str) -> &'static str {
    match name {
        "dashboard" => r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/></svg>"#,
        "operations" => r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="3"/><path d="M12 2v4M22 12h-4M12 22v-4M2 12h4"/></svg>"#,
        "inventory" => r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M4 7l8-4 8 4-8 4-8-4Z"/><path d="M4 7v10l8 4 8-4V7M12 11v10"/></svg>"#,
        "checks" => r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="9"/><path d="m8 12 2.5 2.5L16 9"/></svg>"#,
        "audit" => r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M7 3h10v18H7zM10 8h4M10 12h4M10 16h3"/></svg>"#,
        "evidence" => r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M6 3h9l3 3v15H6zM15 3v4h4"/><path d="m9 14 2 2 4-5"/></svg>"#,
        "reports" => r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M4 20V10h4v10M10 20V4h4v16M16 20v-7h4v7M2 20h20"/></svg>"#,
        "admin" => r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M12 3 5 6v5c0 4.5 2.8 8.1 7 10 4.2-1.9 7-5.5 7-10V6l-7-3Z"/><path d="M9.5 11.5 11 13l3.5-4"/></svg>"#,
        _ => "",
    }
}
```

Generate each link with this exact format so only `name == active_nav` receives `aria-current`:

```rust
let current = (name == active_nav)
    .then_some(" aria-current=\"page\"")
    .unwrap_or("");
format!(
    "<a class=\"nav-link\" href=\"{href}\"{current}><span class=\"nav-icon\" aria-hidden=\"true\">{}</span><span class=\"nav-label\">{label}</span></a>",
    nav_icon(name),
)
```

- [ ] **Step 4: Replace `app_page` shell markup without changing its signature**

Generate the shell with this exact format; `nav` is the trusted anchors built in Step 3 and `body` is the existing server-rendered inner page markup:

```rust
page_shell(
    title,
    &format!(
        r#"<div class="portal-shell"><a class="skip-link" href="#main-content">Skip to main content</a><header class="portal-topbar" data-topbar><a class="portal-brand" href="/dashboard"><span class="brand-mark" aria-hidden="true">NW</span><span class="brand-name"><strong>NaughtyWolf</strong><small>Operator Portal</small></span></a><nav class="primary-nav" aria-label="Primary navigation" data-primary-nav>{nav}</nav><div class="operator-identity"><span class="operator-name">{username}</span><span class="role-badge">{role}</span><form method="post" action="/logout"><button class="logout-button" type="submit">Sign out</button></form></div></header><main id="main-content" class="portal-main"><div class="page-heading"><p class="eyebrow">Authorized workspace</p><h1>{title}</h1></div>{body}</main></div>"#,
        nav = nav,
        username = escape_html(&user.username),
        role = escape_html(&user.role.to_string()),
        title = escape_html(title),
        body = body,
    ),
)
```

Do not add hamburger-only navigation: every destination remains reachable without JavaScript.

- [ ] **Step 5: Implement desktop-top/mobile-bottom responsive CSS**

Use these layout rules as the invariant:

```css
.portal-topbar { position: sticky; top: 0; z-index: 20; display: grid; grid-template-columns: minmax(12rem, 1fr) minmax(0, auto) minmax(12rem, 1fr); align-items: center; gap: 1rem; min-height: 4.5rem; padding: .75rem clamp(1rem, 3vw, 2rem); border-bottom: 1px solid var(--border); background: rgb(6 10 18 / 92%); backdrop-filter: blur(18px); }
.primary-nav { display: flex; justify-self: center; max-width: min(62vw, 58rem); gap: .25rem; overflow-x: auto; scrollbar-width: none; }
.nav-link { display: inline-flex; min-width: max-content; min-height: 44px; align-items: center; gap: .5rem; padding: .625rem .75rem; border: 1px solid transparent; border-radius: var(--radius-sm); color: var(--muted); text-decoration: none; }
.nav-link[aria-current="page"] { color: var(--text); border-color: var(--border-strong); background: var(--surface-raised); box-shadow: inset 0 -2px 0 var(--accent); }
.operator-identity { display: flex; justify-self: end; align-items: center; gap: .75rem; }
.portal-main { width: min(calc(100% - 2rem), var(--content-width)); margin-inline: auto; padding: clamp(1.5rem, 4vw, 3rem) 0 4rem; }
@media (max-width: 767px) {
  .portal-topbar { grid-template-columns: 1fr auto; min-height: 3.75rem; }
  .primary-nav { position: fixed; z-index: 30; inset: auto 0 0; justify-self: stretch; max-width: none; padding: .5rem max(.5rem, env(safe-area-inset-right)) calc(.5rem + env(safe-area-inset-bottom)) max(.5rem, env(safe-area-inset-left)); border-top: 1px solid var(--border); background: rgb(6 10 18 / 96%); box-shadow: 0 -1rem 2.5rem rgb(0 0 0 / 30%); }
  .nav-link { flex: 0 0 auto; flex-direction: column; gap: .2rem; min-width: 4.5rem; padding: .4rem .55rem; font-size: .68rem; }
  .portal-main { padding-bottom: 7rem; }
  .operator-name { display: none; }
}
```

Add these exact shell-support rules:

```css
.portal-brand { display: inline-flex; align-items: center; gap: .65rem; color: var(--text); text-decoration: none; }
.portal-brand:hover { color: var(--text); } .portal-brand .brand-mark { width: 2.35rem; }
.nav-icon { width: 1.05rem; height: 1.05rem; flex: 0 0 auto; } .nav-icon svg { display: block; width: 100%; height: 100%; }
.nav-link:hover { color: var(--text); border-color: var(--border); background: rgb(53 189 246 / 6%); }
.role-badge { padding: .3rem .5rem; border: 1px solid var(--border-strong); border-radius: 999px; color: var(--accent); font: 700 .68rem/1 var(--font-mono); text-transform: uppercase; }
.logout-button { color: var(--muted); border-color: var(--border); background: transparent; } .logout-button:hover { color: #fecdd3; border-color: var(--danger); background: rgb(251 113 133 / 7%); }
.portal-topbar[data-scrolled="true"] { border-bottom-color: var(--border-strong); box-shadow: 0 .75rem 2rem rgb(0 0 0 / 20%); }
.page-heading { margin-bottom: 1.5rem; } .page-heading h1 { margin: .4rem 0 0; font-size: clamp(1.75rem, 4vw, 2.75rem); line-height: 1.05; letter-spacing: -.035em; }
.skip-link { position: fixed; z-index: 100; top: .5rem; left: .5rem; transform: translateY(-160%); padding: .65rem .8rem; color: var(--accent-ink); background: var(--focus); } .skip-link:focus { transform: translateY(0); }
@media (max-width: 767px) { .operator-identity { gap: .4rem; } .operator-identity .role-badge { display: none; } .logout-button { padding-inline: .65rem; } }
```

At 320px the body itself must not overflow.

- [ ] **Step 6: Replace obsolete dock-position JavaScript with optional active-item centering**

Use this dependency-free script:

```javascript
(() => {
  const nav = document.querySelector("[data-primary-nav]");
  const active = nav?.querySelector('[aria-current="page"]');
  active?.scrollIntoView({ block: "nearest", inline: "center" });

  const topbar = document.querySelector("[data-topbar]");
  if (!topbar) return;
  const sync = () => { topbar.dataset.scrolled = window.scrollY > 8 ? "true" : "false"; };
  sync();
  window.addEventListener("scroll", sync, { passive: true });
})();
```

Do not store layout state in local storage.

- [ ] **Step 7: Verify shell and authorization behavior**

Run:

```bash
cargo test --test portal_routes_test authenticated_shell -- --nocapture
cargo test --test portal_routes_test admin_navigation_remains_role_scoped -- --nocapture
cargo test --test portal_routes_test viewer_cannot_open_admin_page -- --nocapture
```

Expected: all selected tests PASS.

- [ ] **Step 8: Review checkpoint**

Inspect markup at 320px and desktop widths, record results, and do not commit without user authorization.

---

### Task 3: Apply the operations design system to dashboard and record pages

**Files:**
- Modify: `tests/portal_routes_test.rs`
- Modify: `src/portal/templates.rs`
- Modify: `static/admin.css`

**Interfaces:**
- Consumes: the Task 2 shell and existing page function signatures/data types.
- Produces: `metric-grid`, `metric-card`, `panel`, `record-grid`, `data-table`, `status-pill`, `empty-state`, `form-panel`, and `page-actions` component classes.

- [ ] **Step 1: Add failing component-contract tests for real data**

Add:

```rust
#[test]
fn dashboard_renders_five_real_metric_cards() {
    let user = AuthenticatedUser {
        id: "viewer-id".into(),
        username: "viewer-user".into(),
        role: Role::Viewer,
    };
    let summary = DashboardSummary {
        operation_count: 1,
        asset_count: 2,
        run_count: 3,
        evidence_count: 4,
        audit_count: 5,
    };
    let body = templates::dashboard_page(&user, &summary);

    assert_eq!(body.matches("class=\"metric-card\"").count(), 5);
    for value in ["1", "2", "3", "4", "5"] {
        assert!(body.contains(&format!(">{value}</strong>")));
    }
    assert!(!body.contains("chart"));
}
```

Extend the existing admin-user rendering test with assertions for `class="data-table admin-users-table"`, textual `Enabled`/`Disabled`, and `class="status-pill`.

- [ ] **Step 2: Run focused tests and confirm the intended failure**

Run:

```bash
cargo test --test portal_routes_test dashboard_renders_five_real_metric_cards -- --nocapture
cargo test --test portal_routes_test admin_user_table_never_renders_password_hashes -- --nocapture
```

Expected: the dashboard test FAILS because current markup uses `summary-grid`, and the extended admin test FAILS because status pills are absent.

- [ ] **Step 3: Update page markup while preserving every function signature and escaped value**

Apply these exact patterns:

- `dashboard_page`: `<section class="metric-grid" aria-label="Scoped summary">` with five `<article class="metric-card">`; label in `<span>`, count in `<strong>`, factual helper copy in `<small>`.
- `operations_page`: `page-actions` plus `<ul class="record-grid">`; each operation becomes `<li class="record-card">` with name, purpose, textual status pill, and existing Add asset authorization.
- `checks_page`, `audit_page`, `evidence_page`, `admin_users_page`: keep captions/headers and use `class="data-table checks-table"`, `class="data-table audit-table"`, `class="data-table evidence-table"`, and `class="data-table admin-users-table"` respectively. Render state/status through the fixed helpers below.
- `reports_page`: keep real counts and change to `<section class="report-grid">` with `panel report-card` articles.
- `operation_form_page`, `asset_form_page`: add `form-panel panel`, wrap each label/input pair in `form-field`, and keep values, maximum lengths, error descriptions, CSRF, methods, and actions unchanged.
- every empty result: use `<section class="empty-state panel">` with the existing factual message.

Import `OperationStatus` and use these exact fixed helpers; no database string may become a CSS class:

```rust
fn operation_status(status: OperationStatus) -> (&'static str, &'static str) {
    match status {
        OperationStatus::Planned => ("Planned", "warning"),
        OperationStatus::Active => ("Active", "success"),
        OperationStatus::Closed => ("Closed", "neutral"),
    }
}

fn run_status(state: RunState) -> (&'static str, &'static str) {
    match state {
        RunState::Queued => ("Queued", "neutral"),
        RunState::Running => ("Running", "warning"),
        RunState::Succeeded => ("Succeeded", "success"),
        RunState::Failed => ("Failed", "danger"),
        RunState::Cancelled => ("Cancelled", "danger"),
    }
}

fn audit_status(outcome: &str) -> &'static str {
    match outcome {
        "success" => "success",
        "failure" | "failed" | "denied" | "error" => "danger",
        _ => "neutral",
    }
}

fn status_pill(label: &str, class_name: &'static str) -> String {
    format!(
        "<span class=\"status-pill status-{class_name}\">{}</span>",
        escape_html(label),
    )
}
```

For Admin accounts, pass `("Disabled", "danger")` when `account.disabled` and `("Enabled", "success")` otherwise.

- [ ] **Step 4: Implement reusable page component CSS**

Implement the produced classes with these invariants:

```css
.metric-grid { display: grid; grid-template-columns: repeat(5, minmax(0, 1fr)); gap: 1rem; }
.metric-card, .panel, .record-card { border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--surface); box-shadow: 0 .75rem 2rem rgb(0 0 0 / 16%); }
.metric-card { position: relative; min-height: 9rem; padding: 1.25rem; overflow: hidden; }
.metric-card::before { content: ""; position: absolute; inset: 0 auto 0 0; width: 3px; background: var(--accent); }
.metric-card strong { display: block; margin-block: .7rem .3rem; color: var(--text); font: 700 clamp(2rem, 4vw, 3rem)/1 var(--font-mono); }
.record-grid, .report-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 20rem), 1fr)); gap: 1rem; margin: 0; padding: 0; list-style: none; }
.status-pill { display: inline-flex; align-items: center; min-height: 1.75rem; padding: .2rem .55rem; border: 1px solid currentColor; border-radius: 999px; font-size: .72rem; font-weight: 700; letter-spacing: .04em; text-transform: uppercase; }
.status-success { color: var(--success); } .status-warning { color: var(--warning); } .status-danger { color: var(--danger); } .status-neutral { color: var(--muted); }
.table-scroll { overflow-x: auto; border: 1px solid var(--border); border-radius: var(--radius-md); background: var(--surface); }
.data-table { width: 100%; min-width: 46rem; border-collapse: collapse; }
.data-table tbody tr:hover { background: rgb(53 189 246 / 5%); }
@media (max-width: 1100px) { .metric-grid { grid-template-columns: repeat(3, minmax(0, 1fr)); } }
@media (max-width: 640px) { .metric-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); } .metric-card:last-child { grid-column: 1 / -1; } }
```

Add these exact supporting component rules:

```css
.panel, .record-card, .report-card { padding: 1.25rem; }
.record-card h2, .report-card h2 { margin: 0 0 .5rem; font-size: 1.05rem; } .record-card p, .report-card p { color: var(--muted); }
.page-actions { display: flex; justify-content: flex-end; gap: .75rem; margin-bottom: 1rem; }
.empty-state { display: grid; min-height: 12rem; place-items: center; color: var(--muted); text-align: center; }
.table-scroll { max-width: 100%; } caption { padding: 1rem; color: var(--muted); font-weight: 700; text-align: left; }
th, td { padding: .8rem 1rem; border-top: 1px solid var(--border); text-align: left; vertical-align: top; } th { color: var(--muted); font-size: .72rem; letter-spacing: .06em; text-transform: uppercase; white-space: nowrap; }
td code, .mono { color: #bae6fd; font-family: var(--font-mono); overflow-wrap: anywhere; }
.muted { color: var(--muted); }
.form-panel { display: grid; max-width: 44rem; gap: 1rem; } .form-panel .primary-action, .form-panel > button { justify-self: start; }
.account-controls, .account-controls form { display: flex; flex-wrap: wrap; align-items: end; gap: .5rem; } .account-controls label { width: 100%; } .account-controls select { width: auto; min-width: 9rem; }
.secondary-button { color: var(--accent); border-color: var(--border-strong); background: transparent; } .secondary-button:hover { color: var(--text); background: rgb(53 189 246 / 7%); }
.admin-users-table .secondary-button { color: #fecdd3; border-color: rgb(251 113 133 / 45%); } .admin-users-table .secondary-button:hover { border-color: var(--danger); background: rgb(251 113 133 / 8%); }
.report-counts { display: grid; grid-template-columns: repeat(auto-fit, minmax(7rem, 1fr)); gap: .65rem; margin: 1rem 0 0; } .report-counts div { padding: .7rem; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--surface-soft); } .report-counts dt { color: var(--muted); font-size: .72rem; } .report-counts dd { margin: .25rem 0 0; font: 700 1.35rem/1 var(--font-mono); }
```

- [ ] **Step 5: Verify all portal rendering and mutation tests**

Run:

```bash
cargo test --test portal_routes_test -- --nocapture
```

Expected: all portal route tests PASS, including CSRF, role, scoping, and admin mutations.

- [ ] **Step 6: Review checkpoint**

Check that no raw database value enters a class name, inspect diff for route/form changes, and do not commit without authorization.

---

### Task 4: Finish responsive, focus, error, reduced-motion, and print states

**Files:**
- Modify: `static/admin.css`
- Modify: `tests/portal_routes_test.rs`

**Interfaces:**
- Consumes: all classes produced by Tasks 1–3.
- Produces: complete interaction-state coverage and the final responsive stylesheet.

- [ ] **Step 1: Add a failing accessibility contract test**

Add:

```rust
#[test]
fn form_pages_keep_labels_errors_and_descriptions() {
    let user = AuthenticatedUser {
        id: "admin-id".into(),
        username: "admin-user".into(),
        role: Role::Admin,
    };
    let body = templates::operation_form_page(
        &user,
        "csrf-token",
        Some("The request is invalid."),
        "Retained name",
        "Retained purpose",
    );

    assert!(body.contains("role=\"alert\""));
    assert!(body.contains("aria-describedby=\"operation-form-error\""));
    assert!(body.contains("value=\"Retained name\""));
    assert!(body.contains("value=\"Retained purpose\""));
    assert_eq!(body.matches("class=\"form-field\"").count(), 2);
}
```

- [ ] **Step 2: Run the test and verify it fails only on the new component contract**

Run:

```bash
cargo test --test portal_routes_test form_pages_keep_labels_errors_and_descriptions -- --nocapture
```

Expected: FAIL on `form-field` count while existing error/value assertions pass.

- [ ] **Step 3: Complete CSS interaction and media states**

Ensure `static/admin.css` includes all of the following working rules:

```css
:where(a, button, input, select):focus-visible { outline: 3px solid var(--focus); outline-offset: 3px; }
:where(button, .button, .nav-link) { min-height: 44px; }
.form-error { border: 1px solid color-mix(in srgb, var(--danger) 55%, var(--border)); border-left-width: 3px; color: #fecdd3; background: rgb(251 113 133 / 8%); }
@media (prefers-reduced-motion: reduce) { *, *::before, *::after { scroll-behavior: auto !important; transition-duration: .01ms !important; animation-duration: .01ms !important; animation-iteration-count: 1 !important; } }
@media print { body { color: #111827; background: #fff; } .portal-topbar, .primary-nav, .print-note, .page-actions { display: none !important; } .portal-main { width: 100%; padding: 0; } .panel, .report-card { color: #111827; background: #fff; box-shadow: none; break-inside: avoid; } }
```

Add these exact boundary rules; do not hide visible labels at any breakpoint:

```css
html, body { max-width: 100%; overflow-x: clip; }
.primary-nav, .table-scroll { -webkit-overflow-scrolling: touch; }
.table-scroll { overflow-x: auto; }
code, td, .operator-name { overflow-wrap: anywhere; }
button:disabled, input:disabled, select:disabled { cursor: not-allowed; opacity: .55; }
a:not(.nav-link):hover { color: #7dd3fc; }
@media (max-width: 767px) {
  .primary-nav { padding-right: max(.5rem, env(safe-area-inset-right)); padding-bottom: calc(.5rem + env(safe-area-inset-bottom)); padding-left: max(.5rem, env(safe-area-inset-left)); }
  .data-table { min-width: 42rem; }
}
```

- [ ] **Step 4: Run focused and complete portal tests**

Run:

```bash
cargo test --test portal_routes_test form_pages_keep_labels_errors_and_descriptions -- --nocapture
cargo test --test portal_routes_test -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Review checkpoint**

Use browser devtools accessibility inspection for the login and one table page; record any violations and fixes. Do not commit without user authorization.

---

### Task 5: Browser visual QA and final regression proof

**Files:**
- Modify only if QA finds a defect: `src/portal/templates.rs`, `static/admin.css`, `static/admin.js`, `tests/portal_routes_test.rs`
- Document verification in the executing agent's task report; do not add screenshot binaries to git

**Interfaces:**
- Consumes: completed server-rendered UI from Tasks 1–4.
- Produces: acceptance evidence for the design spec.

- [ ] **Step 1: Start the existing portal with an authorized local development account**

Use the repository's existing local database/account if available. Start with:

```bash
NAUGHTYWOLF_DATABASE_URL='sqlite:naughtywolf.db?mode=rwc' \
NAUGHTYWOLF_SESSION_SECRET='ui-review-only-secret-at-least-32-bytes' \
NAUGHTYWOLF_COOKIE_SECURE=false \
cargo run -- serve
```

Do not create, disable, or alter a real user merely for visual QA. If no local test account exists, report that authenticated visual QA needs a user-provided disposable account; public login QA and automated authenticated route tests can still proceed.

- [ ] **Step 2: Inspect the login at four exact viewport sizes**

Open `/login` at 1440×900, 1024×768, 390×844, and 320×568. Verify:

- desktop two-panel composition and mobile stacked composition
- CSS is loaded from `/static/admin.css` with no external requests
- labels, focus outline, generic error, and button are visible
- no body-level horizontal scroll
- copy says `Authorized lab access only`

- [ ] **Step 3: Inspect authenticated pages at desktop and mobile sizes**

At 1440×900 and 390×844 inspect `/dashboard`, `/operations`, `/checks`, `/evidence`, `/reports`, and `/admin/users` when role permits. Verify top-centered desktop navigation, fixed bottom mobile navigation, active item visibility, POST logout, table-only horizontal scrolling, long IDs, status pills with text, empty states, and admin controls.

- [ ] **Step 4: Correct each visual defect with a focused regression test where behavior/markup is involved**

For a markup or accessibility defect, first add a failing test to `tests/portal_routes_test.rs`, run it to observe the expected failure, make the smallest template fix, and rerun it. Pure CSS spacing/color adjustments do not require source-text tests; verify them at all four viewports instead.

- [ ] **Step 5: Run final formatting and test gates**

Run:

```bash
rustfmt --edition 2024 --check src/portal/templates.rs tests/portal_routes_test.rs
cargo test --test portal_routes_test
cargo test --workspace
cargo build --workspace
git diff --check
```

Expected: all commands exit 0 except that a pre-existing workspace-wide formatting failure outside the two touched Rust files must be reported rather than broadly rewriting unrelated dirty C2 files. The build must introduce no new warnings.

- [ ] **Step 6: Final acceptance checklist**

Confirm every item explicitly:

- active `/login` uses the new local stylesheet and operator layout
- desktop navigation is top/center; mobile navigation is fixed bottom
- there is no sidebar in active portal markup
- Viewer/Operator never receive the Admin link
- forms, CSRF, logout method, scoping, and admin mutations are unchanged
- portal works with JavaScript disabled
- 320px, reduced-motion, focus, table overflow, and print are covered
- no legacy SPA file was edited
- no external asset or dependency was added

- [ ] **Step 7: Handoff checkpoint**

Provide the user a concise file list, test evidence, viewport QA result, remaining limitations, and the exact dirty-tree status. Do not commit, merge, or push unless the user separately authorizes it.
