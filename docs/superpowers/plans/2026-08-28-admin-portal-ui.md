# Admin Portal UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver a lightweight, responsive, role-aware administrative portal for the standalone security-lab platform.

**Architecture:** The active Axum binary will own a small server-rendered portal router rather than using the legacy operator-console SPA. A focused portal query module will read SQLite records with the authenticated identity and operation membership applied, while static local CSS and minimal JavaScript provide the responsive floating-dock experience. Forms submit to server routes that apply the same RBAC and operation-scope policy as every other mutation.

**Tech Stack:** Rust 2024, Axum 0.8, SQLx SQLite, tower-sessions, vanilla HTML/CSS/JavaScript, no external CDN or frontend build tooling.

**Spec:** `docs/superpowers/specs/2026-08-28-admin-portal-ui-design.md`

## Global Constraints

- Do not add C2, payload, agent, persistence, evasion, or Sliver-facing features or copy to the portal.
- Use only local assets; the browser must not require Node.js, a build step, or a network connection.
- Use a neutral public landing page and generic login-failure copy.
- Enforce every page and mutation on the server; client-side hidden controls are only a usability enhancement.
- Viewer is read-only; Operator manages only operations they belong to; Admin manages users and all operations.
- Preserve existing evidence-path validation and use the audit writer for every successful portal mutation.
- Support 320px-wide screens through desktop, minimum 44px interactive targets, semantic labels, focus visibility, contrast, and `prefers-reduced-motion`.
- Do not stage or overwrite unrelated pre-existing workspace changes.

---

## File structure

| File | Responsibility |
| --- | --- |
| `src/main.rs` | Compose the safe public and authenticated Axum routers, static routes, session layer, and portal handlers. |
| `src/portal.rs` | Role- and operation-scoped read models plus server-side permission helpers used by handlers. |
| `src/db/repositories.rs` | Narrow SQL methods needed by portal read models and audited mutations. |
| `src/audit.rs` | Reusable repository-backed audit insertion helper when needed by portal mutations. |
| `src/portal/templates.rs` | HTML escaping and page renderers for public, login, dashboard, list, form, and error states. |
| `static/admin.css` | Responsive portal design system, shell, dock, tables/cards, form, and reduced-motion styles. |
| `static/admin.js` | Small dock-state enhancement only; pages remain usable without JavaScript. |
| `tests/portal_routes_test.rs` | Public, authentication, RBAC, scoped data, mutation, and HTML contract tests. |
| `README.md` | Local SQLite setup, local user creation, safe feature list, and current portal URLs. |

## Task 1: Foundation — templates, static assets, and public routes

**Files:**
- Create: `src/portal.rs`
- Create: `src/portal/templates.rs`
- Create: `static/admin.css`
- Create: `static/admin.js`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`
- Test: `tests/portal_routes_test.rs`

**Interfaces:**
- Produces `pub fn portal::templates::public_landing() -> String` and `pub fn portal::templates::login_page(error: Option<&str>, csrf_token: &str) -> String`.
- Produces `pub fn public_router() -> Router` in `src/main.rs`, with `GET /`, `GET /login`, `GET /healthz`, `GET /static/admin.css`, and `GET /static/admin.js`.
- Consumes no authenticated state; later tasks add protected routes alongside this router.

- [ ] **Step 1: Write failing public-page contract tests**

```rust
#[tokio::test]
async fn public_pages_link_local_styles_and_do_not_expose_operator_console_copy() {
    let app = public_router();
    let response = app.oneshot(Request::get("/").body(Body::empty()).unwrap()).await.unwrap();
    let body = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("/static/admin.css"));
    assert!(!body.to_lowercase().contains("sliver"));
    assert!(!body.to_lowercase().contains("payload"));
}

#[tokio::test]
async fn login_page_has_labeled_credentials_and_local_styles() {
    let response = public_router().oneshot(Request::get("/login").body(Body::empty()).unwrap()).await.unwrap();
    let body = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("<label"));
    assert!(body.contains("autocomplete=\"username\""));
    assert!(body.contains("/static/admin.css"));
}

#[test]
fn invalid_login_template_is_generic_and_never_echoes_a_username() {
    let body = templates::login_page(Some("Invalid username or password"), "csrf-token");
    assert!(body.contains("Invalid username or password"));
    assert!(!body.contains("missing-user"));
}
```

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `cargo test --test portal_routes_test public_pages_link_local_styles_and_do_not_expose_operator_console_copy -- --exact`

Expected: FAIL because `public_router` and the portal assets do not exist.

- [ ] **Step 3: Implement minimal public templates and asset handlers**

```rust
pub fn public_landing() -> String {
    page_shell(
        "NaughtyWolf",
        "<main class=\"public-page\"><p class=\"eyebrow\">Local security lab</p><h1>Practice with clear scope.</h1><p>Manage authorized operations, inventory, checks, evidence, and audit records in one local workspace.</p><a class=\"button\" href=\"/login\">Sign in</a></main>",
    )
}

pub fn login_page(error: Option<&str>, csrf_token: &str) -> String {
    let error = error.map(|message| format!("<p class=\"form-error\" role=\"alert\">{}</p>", escape_html(message))).unwrap_or_default();
    page_shell("Sign in", &format!("<main class=\"login-page\"><form method=\"post\" action=\"/login\"><h1>Sign in</h1>{error}<input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><label for=\"username\">Username</label><input id=\"username\" name=\"username\" autocomplete=\"username\" required><label for=\"password\">Password</label><input id=\"password\" name=\"password\" type=\"password\" autocomplete=\"current-password\" required><button type=\"submit\">Sign in</button></form></main>", escape_html(csrf_token)))
}
```

Serve the CSS and JavaScript from `include_str!` handlers with `Content-Type: text/css; charset=utf-8` and `application/javascript; charset=utf-8`. Add `pub mod portal;` to `src/lib.rs`. Keep the public landing neutral and ensure no old SPA asset is linked by the active router.

- [ ] **Step 4: Add responsive and accessibility styles**

Implement CSS custom properties, 44px minimum `button`, `input`, and dock-link targets, visible `:focus-visible` outline, `max-width` content containers, a 320px-safe login layout, and:

```css
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after { animation-duration: 0.01ms !important; transition-duration: 0.01ms !important; }
}
```

Implement `admin.js` as an optional enhancement that stores only the user-selected dock position in `localStorage`; use server-rendered links so navigation works when JavaScript is disabled.

- [ ] **Step 5: Run focused and formatting checks**

Run: `cargo fmt --check && cargo test --test portal_routes_test`

Expected: PASS. Update the login handler to pass only `Some("Invalid username or password")` to this template after every failed authentication result. Add a `csrf_token` field to every form model. Store a random `Uuid::new_v4()` token in the signed server session when rendering a form, and reject a missing or non-matching submitted token before any authentication or mutation runs.

- [ ] **Step 6: Commit the foundation**

```bash
git add src/lib.rs src/main.rs src/portal.rs src/portal/templates.rs static/admin.css static/admin.js tests/portal_routes_test.rs
git commit -m "feat: add responsive portal foundation"
```

## Task 2: Scoped portal query models and repository reads

**Files:**
- Modify: `src/db/repositories.rs`
- Modify: `src/portal.rs`
- Modify: `tests/repository_test.rs`
- Test: `tests/portal_routes_test.rs`

**Interfaces:**
- Consumes `Repository`, `AuthenticatedUser`, `Operation`, `Asset`, `CheckRun`, `Evidence`, and `AuditEvent`.
- Produces `pub struct DashboardSummary { pub operation_count: i64, pub asset_count: i64, pub run_count: i64, pub evidence_count: i64, pub audit_count: i64 }`.
- Produces `pub async fn portal::dashboard_summary(repo: &Repository, user: &AuthenticatedUser) -> Result<DashboardSummary, AppError>` and `pub async fn portal::visible_operations(repo: &Repository, user: &AuthenticatedUser) -> Result<Vec<Operation>, AppError>`.
- Produces repository list functions that receive `user_id: &str` and `is_admin: bool`, rather than returning all records to non-admin callers.

- [ ] **Step 1: Write failing scope tests**

```rust
#[tokio::test]
async fn operator_summary_excludes_an_operation_without_membership() {
    let repo = test_repository().await;
    let allowed = repo.create_operation("Allowed", "lab").await.unwrap();
    let hidden = repo.create_operation("Hidden", "lab").await.unwrap();
    let operator = create_user(&repo, "op", Role::Operator).await;
    repo.add_member(&allowed.id, &operator.id).await.unwrap();

    let operations = visible_operations(&repo, &operator).await.unwrap();
    assert_eq!(operations.iter().map(|item| &item.id).collect::<Vec<_>>(), vec![&allowed.id]);
    assert!(!operations.iter().any(|item| item.id == hidden.id));
}
```

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `cargo test --test portal_routes_test operator_summary_excludes_an_operation_without_membership -- --exact`

Expected: FAIL because scoped portal query functions do not exist.

- [ ] **Step 3: Add narrow repository reads and the portal service**

Implement SQL that uses `operation_members` when `is_admin` is false. The resulting operations query must be ordered by `updated_at DESC, id`. Add equivalent scoped list/count queries for assets, check runs, evidence (through `check_runs.operation_id`), and audit events. Administrators use all-operation queries; non-admin users use a membership join. Do not accept an operation ID from the browser as a substitute for the authenticated user scope.

```rust
pub async fn visible_operations(repo: &Repository, user: &AuthenticatedUser) -> Result<Vec<Operation>, AppError> {
    repo.list_operations_visible_to(&user.id, user.role == Role::Admin).await
}
```

- [ ] **Step 4: Add zero-state summary test**

```rust
#[tokio::test]
async fn empty_database_produces_zero_dashboard_summary() {
    let repo = test_repository().await;
    let viewer = create_user(&repo, "viewer", Role::Viewer).await;
    assert_eq!(dashboard_summary(&repo, &viewer).await.unwrap(), DashboardSummary::default());
}
```

Derive `Default`, `Debug`, and `PartialEq` for `DashboardSummary` so this assertion is direct.

- [ ] **Step 5: Run focused repository and portal tests**

Run: `cargo test --test repository_test && cargo test --test portal_routes_test`

Expected: PASS.

- [ ] **Step 6: Commit scoped reads**

```bash
git add src/db/repositories.rs src/portal.rs tests/repository_test.rs tests/portal_routes_test.rs
git commit -m "feat: add scoped portal data queries"
```

## Task 3: Authenticated shell, role-aware pages, and protected routing

**Files:**
- Modify: `src/main.rs`
- Modify: `src/portal/templates.rs`
- Modify: `static/admin.css`
- Test: `tests/portal_routes_test.rs`

**Interfaces:**
- Consumes `AuthenticatedUserGuard`, `AuthenticatedUser`, `Repository`, and `portal::DashboardSummary`.
- Produces `GET /dashboard`, `/operations`, `/inventory`, `/checks`, `/audit`, `/evidence`, `/reports`, and `/admin`.
- Produces `pub fn portal::templates::app_page(title: &str, user: &AuthenticatedUser, active_nav: &str, body: &str) -> String`.
- The `/admin` handler requires `user.require(Role::Admin)` before rendering or querying data.

- [ ] **Step 1: Write failing protected-route tests**

```rust
#[tokio::test]
async fn anonymous_dashboard_request_is_rejected() {
    let response = authenticated_app(test_repository().await)
        .oneshot(Request::get("/dashboard").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn viewer_cannot_open_admin_page_even_if_they_request_its_url() {
    let app = app_with_logged_in_user(Role::Viewer).await;
    let response = app.oneshot(Request::get("/admin").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: Run focused tests to verify they fail**

Run: `cargo test --test portal_routes_test anonymous_dashboard_request_is_rejected -- --exact && cargo test --test portal_routes_test viewer_cannot_open_admin_page_even_if_they_request_its_url -- --exact`

Expected: FAIL because protected portal routes do not exist.

- [ ] **Step 3: Implement authenticated route handlers and shell renderer**

```rust
async fn dashboard(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    let summary = portal::dashboard_summary(&repository, &user).await?;
    Ok(Html(templates::dashboard_page(&user, &summary)))
}

async fn admin(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
) -> Result<Html<String>, AppError> {
    user.require(Role::Admin)?;
    Ok(Html(templates::admin_page(&user, &repository.list_users().await?)))
}
```

Use `app_page` for a common skip link, header (identity and role), dock, main heading, and logout form. Render only the allowed dock entries. Make zero states explicit, for example “No scoped operations yet”, with an Operator/Admin-only link to the operation form.

- [ ] **Step 4: Implement dock responsiveness**

```css
.portal-dock { position: fixed; inset: auto 1rem 1rem; }
@media (min-width: 900px) { .portal-dock[data-position="top"] { inset: 1rem 1rem auto; } }
@media (max-width: 599px) { .portal-dock { overflow-x: auto; } .dock-label { font-size: .72rem; } }
```

Set `data-position="top"` only after the optional JavaScript enhancement runs; safe default is the bottom dock.

- [ ] **Step 5: Run route and full test suites**

Run: `cargo fmt --check && cargo test --test portal_routes_test && cargo test`

Expected: PASS.

- [ ] **Step 6: Commit protected portal navigation**

```bash
git add src/main.rs src/portal/templates.rs static/admin.css tests/portal_routes_test.rs
git commit -m "feat: add role-aware admin portal pages"
```

## Task 4: Audited operations and inventory forms

**Files:**
- Modify: `src/audit.rs`
- Modify: `src/db/repositories.rs`
- Modify: `src/evidence.rs`
- Modify: `src/main.rs`
- Modify: `src/portal/templates.rs`
- Modify: `static/admin.css`
- Test: `tests/portal_routes_test.rs`
- Test: `tests/repository_test.rs`

**Interfaces:**
- Consumes `authorize_operation`, `AuthenticatedUserGuard`, `Repository`, and `AuditEntry`.
- Produces `POST /operations`, `GET /operations/new`, `POST /operations/:operation_id/assets`, and `GET /operations/:operation_id/assets/new`.
- Produces `Repository::create_operation_with_audit(name: &str, purpose: &str, actor_id: &str, correlation_id: &str) -> Result<Operation, AppError>`.
- Produces `Repository::create_asset_with_audit(operation_id: &str, name: &str, kind: &str, owner: &str, address: &str, actor_id: &str, correlation_id: &str) -> Result<Asset, AppError>`.

- [ ] **Step 1: Write failing authorization and audit tests**

```rust
#[tokio::test]
async fn viewer_cannot_submit_an_operation_form() {
    let app = app_with_logged_in_user(Role::Viewer).await;
    let response = app.oneshot(post_form("/operations", "name=Lab&purpose=Practice")).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn creating_an_asset_records_an_audit_event() {
    let repo = test_repository().await;
    let admin = create_user(&repo, "admin", Role::Admin).await;
    let operation = repo.create_operation("Lab", "Practice").await.unwrap();
    repo.create_asset_with_audit(&operation.id, "web-01", "web", "Lab", "127.0.0.1", &admin.id, "test-correlation").await.unwrap();
    assert_eq!(repo.count_audit_events().await.unwrap(), 1);
}
```

- [ ] **Step 2: Run focused tests to verify they fail**

Run: `cargo test --test portal_routes_test viewer_cannot_submit_an_operation_form -- --exact && cargo test --test repository_test creating_an_asset_records_an_audit_event -- --exact`

Expected: FAIL because form routes and audited create methods do not exist.

- [ ] **Step 3: Add transactional audited repository mutations**

Start a SQLite transaction, insert the domain row, construct an `AuditEntry` with action `operation.created` or `asset.created`, call a reusable `insert_audit` function, commit, and reload the created row. Roll back automatically on any error. Do not write the audit event from the HTML renderer.

- [ ] **Step 4: Add server-side form handlers**

```rust
async fn create_asset(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Path(operation_id): Path<String>,
    Form(form): Form<AssetForm>,
) -> Result<Redirect, AppError> {
    authorize_operation(&repository, &user, &operation_id, Role::Operator).await?;
    validate_asset_form(&form)?;
    repository.create_asset_with_audit(&operation_id, &form.name, &form.kind, &form.owner, &form.address, &user.id, &Uuid::new_v4().to_string()).await?;
    Ok(Redirect::to(&format!("/operations/{operation_id}")))
}
```

Use trimmed required fields and a maximum length of 160 characters for each value. Re-render the form with a generic 400 message for validation failures. Admin can create operations; Operator can add assets only to an operation authorized by `authorize_operation`.

- [ ] **Step 5: Add form presentation contracts**

Render `<label for>` pairs, `aria-describedby` for errors, CSRF-safe same-origin forms, and buttons only for roles allowed to submit them. Use `method="post"`; no mutation is initiated by GET.

- [ ] **Step 6: Run focused and full tests**

Run: `cargo fmt --check && cargo test --test repository_test && cargo test --test portal_routes_test && cargo test`

Expected: PASS.

- [ ] **Step 7: Commit operations and inventory management**

```bash
git add src/audit.rs src/db/repositories.rs src/main.rs src/portal/templates.rs static/admin.css tests/repository_test.rs tests/portal_routes_test.rs
git commit -m "feat: add audited portal inventory forms"
```

## Task 5: Administrative user controls, check history, evidence, reports, and documentation

**Files:**
- Modify: `src/db/repositories.rs`
- Modify: `src/main.rs`
- Modify: `src/portal.rs`
- Modify: `src/portal/templates.rs`
- Modify: `static/admin.css`
- Modify: `README.md`
- Test: `tests/portal_routes_test.rs`
- Test: `tests/repository_test.rs`

**Interfaces:**
- Consumes `Role`, `AuthenticatedUserGuard`, scoped portal queries, existing check runner records, evidence metadata, and audit events.
- Produces `GET /admin/users`, `POST /admin/users/:user_id/role`, `POST /admin/users/:user_id/disabled`, `GET /checks`, `GET /audit`, `GET /evidence`, `GET /evidence/:evidence_id/download`, and `GET /reports`.
- Produces `Repository::list_users() -> Result<Vec<PortalUser>, AppError>`, `Repository::set_user_role_with_audit(user_id: &str, role: Role, actor_id: &str, correlation_id: &str) -> Result<(), AppError>`, and `Repository::set_user_disabled_with_audit(user_id: &str, disabled: bool, actor_id: &str, correlation_id: &str) -> Result<(), AppError>`.
- `PortalUser` contains only `id`, `username`, `role`, `disabled`, and `created_at`; it never contains a password hash.

- [ ] **Step 1: Write failing admin and display tests**

```rust
#[tokio::test]
async fn operator_cannot_change_another_users_role() {
    let app = app_with_logged_in_user(Role::Operator).await;
    let response = app.oneshot(post_form("/admin/users/target/role", "role=viewer")).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_user_table_never_renders_password_hashes() {
    let app = app_with_logged_in_user(Role::Admin).await;
    let response = app.oneshot(Request::get("/admin/users").body(Body::empty()).unwrap()).await.unwrap();
    let body = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    assert!(!body.contains("password_hash"));
}

#[tokio::test]
async fn viewer_cannot_download_evidence_outside_their_operation_scope() {
    let app = app_with_logged_in_user(Role::Viewer).await;
    let response = app.oneshot(Request::get("/evidence/not-visible/download").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Run focused tests to verify they fail**

Run: `cargo test --test portal_routes_test operator_cannot_change_another_users_role -- --exact && cargo test --test portal_routes_test admin_user_table_never_renders_password_hashes -- --exact && cargo test --test portal_routes_test viewer_cannot_download_evidence_outside_their_operation_scope -- --exact`

Expected: FAIL because the admin-user routes and safe view model do not exist.

- [ ] **Step 3: Implement admin-only audited account controls**

Parse only `admin`, `operator`, and `viewer` through `Role::from_str`; reject every other submitted value with `AppError::Validation`. Prevent an administrator from changing their own role or disabling their own session account, so at least the current administrator remains available. In one database transaction, update `users.role` or `users.disabled`, insert `user.role_changed` or `user.disabled` audit event, and commit. Do not expose or modify password hashes in portal routes.

- [ ] **Step 4: Render check, audit, evidence, and report pages**

Render scoped check-run history, audit events, and evidence metadata using the query service from Task 2. For evidence, show ID, content type, byte length, hash prefix, and created time—never arbitrary filesystem paths. Add an authorized download handler that first resolves the evidence record through the scoped query service, then calls a new `EvidenceStore::read_verified(&Evidence) -> Result<Vec<u8>, AppError>` method so the existing path, regular-file, length, and SHA-256 validation remains in force before returning an `attachment` response. Reports page renders an operation-scoped printable HTML summary with asset counts, run states, and audit-event counts; it contains no remote integrations and no fabricated results.

- [ ] **Step 5: Update the setup and portal documentation**

Replace obsolete PostgreSQL/Sliver instructions in `README.md` with local SQLite setup. Include:

```bash
NAUGHTYWOLF_DATABASE_URL='sqlite:naughtywolf.db?mode=rwc' \
NAUGHTYWOLF_SESSION_SECRET='replace-with-at-least-32-random-bytes' \
NAUGHTYWOLF_COOKIE_SECURE=false \
cargo run -- user create --username admin --role admin

NAUGHTYWOLF_DATABASE_URL='sqlite:naughtywolf.db?mode=rwc' \
NAUGHTYWOLF_SESSION_SECRET='replace-with-at-least-32-random-bytes' \
NAUGHTYWOLF_COOKIE_SECURE=false \
cargo run -- serve
```

State that this local configuration is for development only and that production deployments require an HTTPS origin with `NAUGHTYWOLF_COOKIE_SECURE=true`.

- [ ] **Step 6: Run final validation**

Run: `cargo fmt --check && cargo test && git diff --check && rg -n -i "sliver|payload|c2|agent|evasion|persistence" src/main.rs src/portal.rs src/portal static/admin.css static/admin.js README.md`

Expected: tests PASS; the final search has no unsafe portal copy or code references. Any historical files outside this task’s active router are not staged by this task.

- [ ] **Step 7: Commit the administrative completion**

```bash
git add src/db/repositories.rs src/evidence.rs src/main.rs src/portal.rs src/portal/templates.rs static/admin.css README.md tests/repository_test.rs tests/portal_routes_test.rs
git commit -m "feat: complete safe admin portal"
```
