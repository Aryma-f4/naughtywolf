use crate::{
    auth::{AuthenticatedUser, rbac::Role},
    db::{
        models::{
            Asset, AuditEvent, C2TaskWithResult, Callback, CallbackStatus, CheckRun, EventRule,
            Evidence, InstalledService, Operation, OperationStatus, RunState,
        },
        repositories::PortalUser,
    },
    payload::{BuildJob, PayloadMeta},
    portal::{DashboardSummary, OperationReportSummary},
};
use base64::Engine;

pub fn public_landing() -> String {
    page_shell(
        "NaughtyWolf",
        "",
        "<main class=\"public-page\"><p class=\"eyebrow\">Local security lab</p><h1>Practice with clear scope.</h1><p>Manage authorized operations, inventory, checks, evidence, and audit records in one local workspace.</p><a class=\"button\" href=\"/login\">Sign in</a></main>",
    )
}

pub fn login_page(error: Option<&str>, csrf_token: &str) -> String {
    let error = error
        .map(|message| {
            format!(
                "<p class=\"form-error\" role=\"alert\">{}</p>",
                escape_html(message)
            )
        })
        .unwrap_or_default();

    let content = format!(
        r#"<main class="login-shell">
<section class="login-brand-panel" aria-labelledby="login-brand-title">
  <div class="brand-lockup"><span class="brand-mark" aria-hidden="true">{wolf}</span><span class="brand-name"><strong>naughtywolf<span class="brand-period">.</span></strong><small>Independent by instinct</small></span></div>
  <div class="login-brand-copy"><p class="eyebrow">THE OPERATOR'S FIELDNOTES / 001</p><h1 id="login-brand-title">A sharper<br>instinct.<br><em>A clearer view.</em></h1><p>Your operations, evidence, and every detail in between.<br>One focused space to see the whole picture.</p></div>
  <div class="login-orbit" aria-hidden="true"><span></span><span></span><span></span><div>{wolf}</div><small>OBSERVE · CONNECT · DOCUMENT</small></div>
  <ul class="trust-list" aria-label="Portal capabilities"><li>01 / Scoped operations</li><li>02 / Verified evidence</li><li>03 / Traceable actions</li></ul>
</section>
<section class="login-form-panel" aria-labelledby="login-title"><div class="login-access-note"><span class="status-dot"></span> LOCAL WORKSPACE <span>NW / ACCESS</span></div><form class="login-card" method="post" action="/login"><span class="login-index" aria-hidden="true">[ 01 — ACCESS ]</span><p class="eyebrow">Good to have you back</p><h2 id="login-title">Find your focus.</h2><p class="login-intro">Sign in to your operator workspace.</p>{error}<input type="hidden" name="csrf_token" value="{csrf_token}"><div class="form-field"><label for="username">Username</label><input id="username" name="username" autocomplete="username" placeholder="Your operator name" required></div><div class="form-field"><label for="password">Password</label><input id="password" name="password" type="password" autocomplete="current-password" placeholder="Enter your password" required></div><button class="primary-action" type="submit">Enter workspace <span aria-hidden="true">↗</span></button><p class="login-guardrail">Authorized lab access only</p></form><div class="login-footer"><span>NAUGHTYWOLF / OPERATOR PORTAL</span><span>Stay curious. Stay in scope.</span></div></section></main>"#,
        wolf = wolf_mark(),
        error = error,
        csrf_token = escape_html(csrf_token),
    );
    page_shell("Sign in", "", &content)
}

pub fn app_page(title: &str, user: &AuthenticatedUser, active_nav: &str, body: &str) -> String {
    let mut groups: Vec<(&str, Vec<(&str, &str, &str)>)> = vec![
        (
            "Command",
            vec![
                ("dashboard", "/dashboard", "Dashboard"),
                ("callbacks", "/callbacks", "Callbacks"),
                ("operations", "/operations", "Operations"),
                ("inventory", "/inventory", "Assets"),
                ("checks", "/checks", "Checks"),
                ("services", "/services", "Services"),
            ],
        ),
        (
            "Collection",
            vec![
                ("evidence", "/evidence", "Evidence"),
                ("reports", "/reports", "Reports"),
                ("audit", "/audit", "Audit"),
            ],
        ),
        (
            "Automation",
            vec![
                ("eventing", "/eventing", "Eventing"),
                ("events", "/events", "Event Feed"),
            ],
        ),
        (
            "System",
            vec![
                ("payloads", "/payloads", "Payloads"),
                ("search", "/search", "Search"),
                ("admin", "/admin/users", "Admin"),
            ],
        ),
    ];

    // Role-gate operator-only and admin-only navigation links.
    let operator_only = [
        "callbacks",
        "services",
        "eventing",
        "events",
        "search",
        "payloads",
    ];
    if !user.role.allows(Role::Operator) {
        for group in groups.iter_mut() {
            group.1.retain(|(name, _, _)| !operator_only.contains(name));
        }
    }
    if user.role != Role::Admin {
        for group in groups.iter_mut() {
            group.1.retain(|(name, _, _)| *name != "admin");
        }
    }

    let can_operator = user.role.allows(Role::Operator);
    let quick_links = [
        ("dashboard", "/dashboard", "Dashboard"),
        ("callbacks", "/callbacks", "Callbacks"),
        ("events", "/events", "Events"),
        ("payloads", "/payloads", "Payloads"),
        ("search", "/search", "Search"),
    ]
    .iter()
    .filter(|(name, _, _)| *name == "dashboard" || can_operator)
    .map(|(name, href, label)| {
        let current = (name == &active_nav)
            .then_some(" aria-current=\"page\"")
            .unwrap_or("");
        format!(
            "<a class=\"quick-link\" href=\"{href}\"{current}><span class=\"nav-icon\" aria-hidden=\"true\">{}</span><span>{label}</span></a>",
            nav_icon(name),
        )
    })
    .collect::<String>();

    let nav = groups
        .into_iter()
        .map(|(group_name, items)| {
            let links = items
                .into_iter()
                .map(|(name, href, label)| {
                    let current = (name == active_nav)
                        .then_some(" aria-current=\"page\"")
                        .unwrap_or("");
                    format!(
                        "<a class=\"nav-link\" href=\"{href}\"{current}><span class=\"nav-icon\" aria-hidden=\"true\">{}</span><span class=\"nav-label\">{label}</span></a>",
                        nav_icon(name),
                    )
                })
                .collect::<String>();
            format!(
                "<div class=\"nav-group\"><span class=\"nav-group-title\">{}</span>{links}</div>",
                escape_html(group_name),
            )
        })
        .collect::<String>();

    page_shell(
        title,
        APP_ASSETS,
        &format!(
            r##"<div class="portal-shell"><a class="skip-link" href="#main-content">Skip to main content</a><header class="portal-topbar" data-topbar><a class="portal-brand" href="/dashboard"><span class="brand-mark" aria-hidden="true">{wolf}</span><span class="brand-name"><strong>naughtywolf<span class="brand-period">.</span></strong><small>Operator fieldnotes</small></span></a><nav class="topbar-quick" aria-label="Quick access">{quick_links}</nav><div class="operator-identity"><span class="operator-avatar" aria-hidden="true">{initial}</span><span class="operator-name">{username}</span><span class="role-badge">{role}</span><form method="post" action="/logout"><button class="logout-button" type="submit">Sign out <span aria-hidden="true">↗</span></button></form></div></header><nav class="primary-nav" aria-label="Primary navigation" data-primary-nav><div class="nav-workspace"><span class="workspace-symbol" aria-hidden="true">N / W</span><div><strong>Local workspace</strong><small>Scoped by design</small></div></div>{nav}<div class="nav-footnote"><span class="eyebrow">A sharper instinct.</span><p>Observe carefully.<br>Leave a clear record.</p><span class="nav-edition">NW — FIELD EDITION 01</span></div></nav><main id="main-content" class="portal-main"><div class="page-heading"><div><p class="eyebrow">Workspace <span aria-hidden="true">/</span> {title}</p><h1>{heading}</h1><p class="page-description">{description}</p></div><span class="page-stamp">{stamp}</span></div>{body}<footer class="workspace-footer"><span>NAUGHTYWOLF<span class="brand-period">.</span> <span class="footer-divider">/</span> OPERATOR FIELDNOTES</span><span>Made for the details.</span></footer></main></div>"##,
            wolf = wolf_mark(),
            initial = escape_html(
                &user
                    .username
                    .chars()
                    .next()
                    .unwrap_or('N')
                    .to_uppercase()
                    .to_string()
            ),
            heading = if active_nav == "dashboard" {
                "The overview.".to_owned()
            } else {
                escape_html(title)
            },
            description = page_description(active_nav),
            stamp = if active_nav == "dashboard" {
                "01 / OBSERVE"
            } else {
                "NW / FIELDNOTES"
            },
            username = escape_html(&user.username),
            role = escape_html(&user.role.to_string()),
            title = escape_html(title),
            body = body,
            quick_links = quick_links,
        ),
    )
}

pub fn dashboard_page(user: &AuthenticatedUser, summary: &DashboardSummary) -> String {
    let metrics = [
        ("Operations", summary.operation_count, "Authorized lab operations", "/operations", "operations"),
        ("Assets", summary.asset_count, "Scoped targets", "/inventory", "inventory"),
        ("Check runs", summary.run_count, "Executed checks", "/checks", "checks"),
        ("Evidence", summary.evidence_count, "Stored artifacts", "/evidence", "evidence"),
        ("Audit records", summary.audit_count, "Append-only events", "/audit", "audit"),
    ].iter().enumerate().map(|(i, (label, count, note, href, icon))| format!(
        r#"<article class="metric-card"><div class="metric-top"><span class="nav-icon" aria-hidden="true">{}</span><span class="metric-index">0{}</span></div><span>{label}</span><strong>{count}</strong><small>{note}</small><a class="metric-link" href="{href}" aria-label="View {label}"><span aria-hidden="true">↗</span></a></article>"#,
        nav_icon(icon), i + 1,
    )).collect::<String>();
    let first_action = if user.role == Role::Admin {
        r#"<a class="button" href="/operations/new">Create operation <span aria-hidden="true">↗</span></a>"#
    } else {
        r#"<a class="button" href="/operations">Explore operations <span aria-hidden="true">↗</span></a>"#
    };
    let scope_note = if summary.operation_count == 0 {
        "A clean slate. Your next operation starts here."
    } else {
        "Every operation has a story. Keep the details connected."
    };
    app_page(
        "Dashboard",
        user,
        "dashboard",
        &format!(
            r#"<section class="overview-hero" aria-labelledby="overview-title"><div class="hero-copy"><p class="eyebrow"><span class="status-dot"></span> YOUR WORKSPACE, IN FOCUS</p><h2 id="overview-title">See the whole picture.<br><em>Follow every detail.</em></h2><p>{scope_note}</p><div class="hero-actions">{first_action}<a class="text-action" href="/reports">Open reports <span aria-hidden="true">↗</span></a></div></div><div class="scope-orbit" aria-hidden="true"><div class="orbit-ring orbit-ring-outer"></div><div class="orbit-ring orbit-ring-middle"></div><div class="orbit-ring orbit-ring-inner"></div><div class="orbit-cross orbit-cross-h"></div><div class="orbit-cross orbit-cross-v"></div><div class="orbit-core">{wolf}</div><span class="orbit-point point-one"></span><span class="orbit-point point-two"></span><span class="orbit-label orbit-label-top">01 / SCOPE</span><span class="orbit-label orbit-label-bottom">02 / EVIDENCE</span><span class="orbit-coordinate">NW · ALL DETAILS CONNECTED</span></div></section>
        <div class="section-heading"><h2>By the numbers<span class="muted"> /</span></h2><span>RECORDS VISIBLE TO YOU</span></div><section class="metric-grid" aria-label="Scoped summary">{metrics}</section>
        <div class="overview-bottom"><section class="field-map panel" aria-labelledby="field-map-title"><div class="section-heading"><h2 id="field-map-title">A connected workflow</h2><span>01 → 03</span></div><p class="section-intro">From a defined scope to a record you can stand behind.</p><div class="workflow-steps"><a href="/operations"><span class="step-index">01 / DEFINE</span><span class="step-symbol" aria-hidden="true">{scope_icon}</span><h3>Set the scope.</h3><p>Give each operation a clear purpose and boundary.</p><span class="step-link">View operations ↗</span></a><a href="/inventory"><span class="step-index">02 / OBSERVE</span><span class="step-symbol" aria-hidden="true">{asset_icon}</span><h3>Know the details.</h3><p>Keep your assets and check history in context.</p><span class="step-link">Explore assets ↗</span></a><a href="/evidence"><span class="step-index">03 / DOCUMENT</span><span class="step-symbol" aria-hidden="true">{evidence_icon}</span><h3>Leave a record.</h3><p>Bring the evidence together for your next report.</p><span class="step-link">Review evidence ↗</span></a></div></section><aside class="field-note"><span class="eyebrow">THE FIELD NOTE / 001</span><span class="note-asterisk" aria-hidden="true">✳</span><h2>Good work.<br>Clear evidence.</h2><p>The strongest finding is the one you can trace. Keep your scope intentional and your records complete.</p><a href="/audit">Follow the audit trail <span aria-hidden="true">↗</span></a><span class="note-bottom">STAY CURIOUS. STAY IN SCOPE.</span></aside></div>"#,
            wolf = wolf_mark(),
            scope_icon = nav_icon("operations"),
            asset_icon = nav_icon("inventory"),
            evidence_icon = nav_icon("evidence"),
        ),
    )
}

pub fn operations_page(user: &AuthenticatedUser, operations: &[Operation]) -> String {
    let create_link = (user.role == Role::Admin)
        .then_some("<a class=\"button\" href=\"/operations/new\">Create operation</a>")
        .unwrap_or_default();
    let body = if operations.is_empty() {
        format!("<section class=\"empty-state panel\"><p>No scoped operations yet</p></section>")
    } else {
        let items = operations
            .iter()
            .map(|operation| {
                let (status_label, status_class) = operation_status(operation.status);
                let asset_link = user
                    .role
                    .allows(Role::Operator)
                    .then(|| {
                        format!(
                            "<a href=\"/operations/{}/assets/new\">Add asset</a>",
                            escape_html(&operation.id)
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "<li class=\"record-card\"><h2>{}</h2><p>{}</p>{}{asset_link}</li>",
                    escape_html(&operation.name),
                    escape_html(&operation.purpose),
                    status_pill(status_label, status_class),
                )
            })
            .collect::<String>();
        format!("<ul class=\"record-grid\">{items}</ul>")
    };

    app_page(
        "Operations",
        user,
        "operations",
        &format!("<div class=\"page-actions\">{create_link}</div>{body}"),
    )
}

pub fn checks_page(user: &AuthenticatedUser, runs: &[CheckRun]) -> String {
    let body = if runs.is_empty() {
        "<section class=\"empty-state panel\"><p>No scoped checks yet</p></section>".to_owned()
    } else {
        let rows = runs
            .iter()
            .map(|run| {
                let (state_label, state_class) = run_status(run.state);
                format!(
                    "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td><code>{}</code></td><td>{}</td></tr>",
                    escape_html(&run.id),
                    escape_html(&run.check_id),
                    status_pill(state_label, state_class),
                    escape_html(&run.operation_id),
                    escape_html(&run.created_at),
                )
            })
            .collect::<String>();
        format!(
            "<div class=\"table-scroll\"><table class=\"data-table checks-table\"><caption>Scoped check-run history</caption><thead><tr><th scope=\"col\">Run</th><th scope=\"col\">Check</th><th scope=\"col\">State</th><th scope=\"col\">Operation</th><th scope=\"col\">Created</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    app_page("Checks", user, "checks", &body)
}

pub fn audit_page(user: &AuthenticatedUser, events: &[AuditEvent]) -> String {
    let body = if events.is_empty() {
        "<section class=\"empty-state panel\"><p>No scoped audit records yet</p></section>"
            .to_owned()
    } else {
        let rows = events
            .iter()
            .map(|event| {
                let outcome_class = audit_status(&event.outcome);
                format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape_html(&event.action),
                    escape_html(event.actor_id.as_deref().unwrap_or("System")),
                    escape_html(&event.target_type),
                    escape_html(event.target_id.as_deref().unwrap_or("—")),
                    status_pill(&event.outcome, outcome_class),
                    escape_html(&event.created_at),
                )
            })
            .collect::<String>();
        format!(
            "<div class=\"table-scroll\"><table class=\"data-table audit-table\"><caption>Scoped, append-only audit history</caption><thead><tr><th scope=\"col\">Action</th><th scope=\"col\">Actor</th><th scope=\"col\">Target type</th><th scope=\"col\">Target</th><th scope=\"col\">Outcome</th><th scope=\"col\">Created</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    app_page("Audit", user, "audit", &body)
}

pub fn evidence_page(user: &AuthenticatedUser, records: &[Evidence]) -> String {
    let body = if records.is_empty() {
        "<section class=\"empty-state panel\"><p>No scoped evidence yet</p></section>".to_owned()
    } else {
        let rows = records
            .iter()
            .map(|evidence| {
                let hash_prefix = evidence.sha256.chars().take(12).collect::<String>();
                format!(
                    "<tr><td><code>{}</code></td><td>{}</td><td>{} bytes</td><td><code>{}</code></td><td>{}</td><td><a href=\"/evidence/{}/download\">Download</a></td></tr>",
                    escape_html(&evidence.id),
                    escape_html(&evidence.content_type),
                    evidence.byte_len,
                    escape_html(&hash_prefix),
                    escape_html(&evidence.created_at),
                    escape_html(&evidence.id),
                )
            })
            .collect::<String>();
        format!(
            "<div class=\"table-scroll\"><table class=\"data-table evidence-table\"><caption>Scoped evidence metadata</caption><thead><tr><th scope=\"col\">Evidence</th><th scope=\"col\">Content type</th><th scope=\"col\">Size</th><th scope=\"col\">SHA-256 prefix</th><th scope=\"col\">Created</th><th scope=\"col\">File</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    app_page("Evidence", user, "evidence", &body)
}

pub fn reports_page(user: &AuthenticatedUser, summaries: &[OperationReportSummary]) -> String {
    let body = if summaries.is_empty() {
        "<section class=\"empty-state panel\"><p>No scoped reports yet</p></section>".to_owned()
    } else {
        let reports = summaries
            .iter()
            .map(|summary| {
                format!(
                    "<article class=\"panel report-card\"><h2>{}</h2><p>{}</p><dl class=\"report-counts\"><div><dt>Assets</dt><dd>{}</dd></div><div><dt>Queued</dt><dd>{}</dd></div><div><dt>Running</dt><dd>{}</dd></div><div><dt>Succeeded</dt><dd>{}</dd></div><div><dt>Failed</dt><dd>{}</dd></div><div><dt>Cancelled</dt><dd>{}</dd></div><div><dt>Audit records</dt><dd>{}</dd></div></dl></article>",
                    escape_html(&summary.operation.name),
                    escape_html(&summary.operation.purpose),
                    summary.asset_count,
                    summary.queued_count,
                    summary.running_count,
                    summary.succeeded_count,
                    summary.failed_count,
                    summary.cancelled_count,
                    summary.audit_count,
                )
            })
            .collect::<String>();
        format!(
            "<p class=\"print-note\">Use your browser's print command for a printable copy.</p><section class=\"report-grid\" aria-label=\"Operation summaries\">{reports}</section>"
        )
    };
    app_page("Reports", user, "reports", &body)
}

pub fn admin_users_page(
    user: &AuthenticatedUser,
    users: &[PortalUser],
    csrf_token: &str,
    error: Option<&str>,
) -> String {
    let rows = users
        .iter()
        .map(|account| {
            let controls = if account.id == user.id {
                "<span class=\"muted\">Current account</span>".to_owned()
            } else {
                let role_options = [Role::Admin, Role::Operator, Role::Viewer]
                    .into_iter()
                    .map(|role| {
                        let selected = (role == account.role).then_some(" selected").unwrap_or("");
                        format!(
                            "<option value=\"{}\"{selected}>{}</option>",
                            role,
                            role,
                        )
                    })
                    .collect::<String>();
                let next_disabled = !account.disabled;
                let disabled_label = if account.disabled { "Enable" } else { "Disable" };
                format!(
                    "<div class=\"account-controls\"><form method=\"post\" action=\"/admin/users/{}/role\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><label for=\"role-{}\">Role</label><select id=\"role-{}\" name=\"role\">{role_options}</select><button type=\"submit\">Update role</button></form><form method=\"post\" action=\"/admin/users/{}/disabled\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><input type=\"hidden\" name=\"disabled\" value=\"{}\"><button class=\"secondary-button\" type=\"submit\">{disabled_label}</button></form></div>",
                    escape_html(&account.id),
                    escape_html(csrf_token),
                    escape_html(&account.id),
                    escape_html(&account.id),
                    escape_html(&account.id),
                    escape_html(csrf_token),
                    next_disabled,
                )
            };
            let (status_label, status_class) = if account.disabled {
                ("Disabled", "danger")
            } else {
                ("Enabled", "success")
            };
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{controls}</td></tr>",
                escape_html(&account.username),
                escape_html(&account.role.to_string()),
                status_pill(status_label, status_class),
                escape_html(&account.created_at),
            )
        })
        .collect::<String>();
    let error = form_error(error, "admin-users-error");
    let body = format!(
        "{error}<div class=\"table-scroll\"><table class=\"data-table admin-users-table\"><caption>Local user accounts</caption><thead><tr><th scope=\"col\">Username</th><th scope=\"col\">Role</th><th scope=\"col\">Status</th><th scope=\"col\">Created</th><th scope=\"col\">Controls</th></tr></thead><tbody>{rows}</tbody></table></div>"
    );
    app_page("Administration", user, "admin", &body)
}

pub fn operation_form_page(
    user: &AuthenticatedUser,
    csrf_token: &str,
    error: Option<&str>,
    name: &str,
    purpose: &str,
) -> String {
    let error = form_error(error, "operation-form-error");
    let described_by = (!error.is_empty())
        .then_some(" aria-describedby=\"operation-form-error\"")
        .unwrap_or_default();
    app_page(
        "Create operation",
        user,
        "operations",
        &format!(
            "<form class=\"form-panel panel\" method=\"post\" action=\"/operations\">{error}<input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><div class=\"form-field\"><label for=\"operation-name\">Name</label><input id=\"operation-name\" name=\"name\" value=\"{}\" maxlength=\"160\" required{described_by}></div><div class=\"form-field\"><label for=\"operation-purpose\">Purpose</label><input id=\"operation-purpose\" name=\"purpose\" value=\"{}\" maxlength=\"160\" required{described_by}></div><button type=\"submit\">Create operation</button></form>",
            escape_html(csrf_token),
            escape_html(name),
            escape_html(purpose),
        ),
    )
}

pub fn asset_form_page(
    user: &AuthenticatedUser,
    operation_id: &str,
    csrf_token: &str,
    error: Option<&str>,
    values: [&str; 4],
) -> String {
    let error = form_error(error, "asset-form-error");
    let described_by = (!error.is_empty())
        .then_some(" aria-describedby=\"asset-form-error\"")
        .unwrap_or_default();
    app_page(
        "Add asset",
        user,
        "inventory",
        &format!(
            "<form class=\"form-panel panel\" method=\"post\" action=\"/operations/{}/assets\">{error}<input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><div class=\"form-field\"><label for=\"asset-name\">Name</label><input id=\"asset-name\" name=\"name\" value=\"{}\" maxlength=\"160\" required{described_by}></div><div class=\"form-field\"><label for=\"asset-kind\">Kind</label><input id=\"asset-kind\" name=\"kind\" value=\"{}\" maxlength=\"160\" required{described_by}></div><div class=\"form-field\"><label for=\"asset-owner\">Owner</label><input id=\"asset-owner\" name=\"owner\" value=\"{}\" maxlength=\"160\" required{described_by}></div><div class=\"form-field\"><label for=\"asset-address\">Address</label><input id=\"asset-address\" name=\"address\" value=\"{}\" maxlength=\"160\" required{described_by}></div><button type=\"submit\">Add asset</button></form>",
            escape_html(operation_id),
            escape_html(csrf_token),
            escape_html(values[0]),
            escape_html(values[1]),
            escape_html(values[2]),
            escape_html(values[3]),
        ),
    )
}

fn current_os() -> String {
    std::env::consts::OS.to_string()
}

fn current_arch_label() -> String {
    match std::env::consts::ARCH {
        "x86_64" | "x86" => "amd64".into(),
        "aarch64" | "arm64" => "arm64".into(),
        other => other.to_string(),
    }
}

/// (target-triple, label, os, arch). Empty triple = host-native build.
const PLATFORMS: &[(&str, &str, &str, &str)] = &[
    ("", "This computer (native)", "current", "current"),
    (
        "x86_64-unknown-linux-musl",
        "Linux x86-64 (musl)",
        "linux",
        "amd64",
    ),
    (
        "x86_64-unknown-linux-gnu",
        "Linux x86-64 (glibc)",
        "linux",
        "amd64",
    ),
    (
        "aarch64-unknown-linux-musl",
        "Linux ARM64 (musl)",
        "linux",
        "arm64",
    ),
    (
        "x86_64-pc-windows-gnu",
        "Windows x86-64",
        "windows",
        "amd64",
    ),
    (
        "aarch64-apple-darwin",
        "macOS Apple Silicon",
        "macos",
        "arm64",
    ),
    ("x86_64-apple-darwin", "macOS Intel", "macos", "amd64"),
];

pub fn payloads_page(
    user: &AuthenticatedUser,
    builds: &[PayloadMeta],
    csrf_token: &str,
    error: Option<&str>,
    editor: Option<&PayloadMeta>,
    building: &[BuildJob],
    build_errors: &[(String, String, String)],
) -> String {
    let error_html = form_error(error, "payloads-form-error");

    let errors_html = build_errors
        .iter()
        .map(|(file, msg, _at)| {
            format!(
                "<div class=\"build-error panel\" role=\"alert\" data-dismissible=\"{}\"><div><strong>Build failed — {}</strong><pre>{}</pre></div><button type=\"button\" class=\"dismiss-btn\" data-dismiss=\"true\" aria-label=\"Dismiss\">&times;</button></div>",
                escape_html(file),
                escape_html(file),
                escape_html(msg),
            )
        })
        .collect::<String>();

    let rows = builds
        .iter()
        .map(|b| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}/{}</td><td>{}</td><td>{} bytes</td><td>{}</td><td><a class=\"btn btn-sm\" href=\"/payloads/download/{}\">Download</a> <a class=\"btn btn-sm\" href=\"/payloads/edit/{file}\">Edit</a></td></tr>",
                escape_html(&b.name),
                escape_html(&b.protocol),
                escape_html(&b.os),
                escape_html(&b.arch),
                escape_html(&format!("{}:{}", b.lhost, b.lport)),
                b.size,
                escape_html(&b.built_at),
                url_encode(&b.file),
                file = url_encode(&b.file),
            )
        })
        .collect::<String>();
    let list = if builds.is_empty() {
        "<section class=\"empty-state panel\"><p>No payloads built yet — generate one below.</p></section>".to_owned()
    } else {
        format!(
            "<div class=\"table-scroll\"><table class=\"data-table payloads-table\"><caption>Built payloads</caption><thead><tr><th scope=\"col\">Name</th><th scope=\"col\">Protocol</th><th scope=\"col\">OS/Arch</th><th scope=\"col\">Endpoint</th><th scope=\"col\">Size</th><th scope=\"col\">Built</th><th scope=\"col\">Actions</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };

    let now = chrono::Utc::now().to_rfc3339();
    let building_list = building
        .iter()
        .map(|j| {
            format!(
                "<li>{}</li>",
                escape_html(&format!(
                    "{} ({}/{}, {})",
                    j.name,
                    j.os,
                    j.arch,
                    if j.target.is_empty() {
                        "native".into()
                    } else {
                        j.target.clone()
                    }
                ))
            )
        })
        .collect::<String>();
    let building_html = if building.is_empty() {
        String::new()
    } else {
        format!(
            "<section class=\"building-banner panel\" id=\"building-banner\" data-building=\"true\" data-since=\"{now}\"><div class=\"spinner\" aria-hidden=\"true\"></div><div><h2>Building payload{s}</h2><ul>{building_list}</ul><p class=\"muted\">Keep this page open while the implant compiles.</p></div></section>",
            s = if building.len() == 1 { "" } else { "s" },
        )
    };

    let (f_os, f_arch, f_target, f_name, f_lhost, f_lport, f_psk, f_protocol, f_interval, f_jitter) =
        match editor {
            Some(m) => (
                m.os.clone(),
                m.arch.clone(),
                m.target.clone(),
                m.name.clone(),
                m.lhost.clone(),
                m.lport,
                m.psk.clone(),
                m.protocol.clone(),
                m.interval_ms,
                m.jitter_ms,
            ),
            None => (
                current_os(),
                current_arch_label(),
                String::new(),
                String::new(),
                String::new(),
                8081,
                String::new(),
                "http".into(),
                1000,
                200,
            ),
        };

    let platform_options = PLATFORMS
        .iter()
        .map(|(triple, label, os, arch)| {
            let (sel_os, sel_arch) = if triple.is_empty() {
                (current_os(), current_arch_label())
            } else {
                ((*os).to_string(), (*arch).to_string())
            };
            let selected = if f_target == *triple { " selected" } else { "" };
            format!(
                "<option value=\"{}\" data-os=\"{}\" data-arch=\"{}\"{selected}>{label}</option>",
                escape_html(triple),
                escape_html(&sel_os),
                escape_html(&sel_arch),
            )
        })
        .collect::<String>();
    let visible_platform =
        if f_target.is_empty() && f_os == current_os() && f_arch == current_arch_label() {
            "current".into()
        } else {
            format!("{}/{}", f_os, f_arch)
        };

    let protocol_options = [
        ("http", "HTTP"),
        ("https", "HTTPS"),
        ("tcp", "TCP (raw)"),
        ("gs", "gsocket tunnel"),
        ("dns", "DNS"),
    ]
    .iter()
    .map(|(v, label)| {
        let sel = if f_protocol == *v { " selected" } else { "" };
        format!("<option value=\"{v}\"{sel}>{label}</option>")
    })
    .collect::<String>();

    let body = format!(
        "{error_html}{errors_html}{building_html}<form class=\"form-panel panel\" method=\"post\" action=\"/payloads/generate\" data-payload-form><input type=\"hidden\" name=\"csrf_token\" value=\"{token}\"><div class=\"form-field\"><label for=\"payload-name\">Name</label><input id=\"payload-name\" name=\"name\" value=\"{f_name}\" placeholder=\"linux-implant\" maxlength=\"80\" required></div><div class=\"form-field\"><label for=\"payload-platform\">Target platform</label><select id=\"payload-platform\">{platform_options}</select><p class=\"muted field-hint\" id=\"payload-platform-hint\">{visible_platform}</p><input type=\"hidden\" name=\"os\" id=\"payload-os\" value=\"{f_os}\"><input type=\"hidden\" name=\"arch\" id=\"payload-arch\" value=\"{f_arch}\"><input type=\"hidden\" name=\"target\" id=\"payload-target\" value=\"{f_target}\"></div><div class=\"form-field\"><label for=\"payload-lhost\">Callback host (LHost)</label><input id=\"payload-lhost\" name=\"lhost\" value=\"{f_lhost}\" placeholder=\"127.0.0.1\" required></div><div class=\"form-field\"><label for=\"payload-lport\">Callback port (LPort)</label><input id=\"payload-lport\" name=\"lport\" type=\"number\" value=\"{f_lport}\" min=\"1\" max=\"65535\" required></div><div class=\"form-field\"><label for=\"payload-psk\">Shared secret (NW_PSK)</label><div class=\"input-with-btn\"><input id=\"payload-psk\" name=\"psk\" value=\"{f_psk}\" placeholder=\"share-a-lab-psk\" required><button type=\"button\" class=\"btn btn-sm\" id=\"payload-psk-random\" title=\"Generate random secret\">Random</button></div><p class=\"muted field-hint\">Must match the portal's C2 PSK to receive callbacks.</p></div><div class=\"form-field\"><label for=\"payload-proto\">Protocol</label><select id=\"payload-proto\" name=\"protocol\">{protocol_options}</select><p class=\"muted field-hint\">tcp/gs need the raw TCP listener (NW_TCP_BIND); gs rides TCP through a gsocket tunnel.</p></div><div class=\"form-field\"><label for=\"payload-interval\">Beacon interval (ms)</label><input id=\"payload-interval\" name=\"interval_ms\" type=\"number\" value=\"{f_interval}\" min=\"10\"></div><div class=\"form-field\"><label for=\"payload-jitter\">Jitter (ms)</label><input id=\"payload-jitter\" name=\"jitter_ms\" type=\"number\" value=\"{f_jitter}\" min=\"0\"></div><button type=\"submit\">{btn_label}</button></form><h2>Built payloads</h2>{list}",
        token = escape_html(csrf_token),
        f_name = escape_html(&f_name),
        f_lhost = escape_html(&f_lhost),
        f_lport = f_lport,
        f_psk = escape_html(&f_psk),
        f_interval = f_interval,
        f_jitter = f_jitter,
        f_os = escape_html(&f_os),
        f_arch = escape_html(&f_arch),
        f_target = escape_html(&f_target),
        btn_label = if editor.is_some() {
            "Recompile implant"
        } else {
            "Build implant"
        },
        protocol_options = protocol_options,
    );
    app_page("Payloads", user, "payloads", &body)
}

/// Mythic-style callback detail / interact page.
/// Features a tasking panel (command input + tab completion), task history
/// table with color-coded Mythic-style status pills, and SSE-powered
/// real-time task result streaming.
pub fn callback_detail_page(
    user: &AuthenticatedUser,
    callback: &Callback,
    tasks: &[C2TaskWithResult],
    csrf_token: &str,
) -> String {
    // Build task history rows with Mythic-style status pills.
    let rows = if tasks.is_empty() {
        "<section class=\"empty-state panel\"><p>No task history yet. Submit a command below.</p></section>".to_owned()
    } else {
        let mut out = String::new();
        for t in tasks {
            let (label, cls) = crate::db::models::TaskStatus::label_class_from_str(&t.status);
            let state_pill = format!(
                "<span class=\"status-pill status-{cls}\">{}</span>",
                escape_html(label)
            );
            let output_display = if let Some(ref out_text) = t.result_output {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(out_text)
                    .unwrap_or_default();
                escape_html(&String::from_utf8_lossy(&decoded))
            } else {
                "<span class=\"muted\">—</span>".to_owned()
            };
            out.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_html(&t.id[..t.id.len().min(8)]),
                escape_html(&t.command),
                state_pill,
                escape_html(&t.created_at),
                output_display,
            ));
        }
        format!(
            "<div class=\"table-scroll\"><table class=\"data-table task-history\"><caption>Task history</caption><thead><tr><th scope=\"col\">Task ID</th><th scope=\"col\">Command</th><th scope=\"col\">State</th><th scope=\"col\">Submitted</th><th scope=\"col\">Output</th></tr></thead><tbody>{out}</tbody></table></div>"
        )
    };

    // Command suggestions for the input <datalist>.
    let suggestions = [
        "ls",
        "pwd",
        "whoami",
        "hostname",
        "env",
        "netstat",
        "ifconfig",
        "id",
        "ps",
        "cat /etc/passwd",
        "curl",
        "wget",
        "bash",
        "zsh",
    ];
    let suggestions_html: String = suggestions
        .iter()
        .map(|cmd| {
            format!(
                "<option value=\"{}\">{}</option>",
                escape_html(cmd),
                escape_html(cmd)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let callback_detail = format!(
        "<div class=\"callback-header\"><div class=\"callback-meta\"><h2>{}</h2><p class=\"muted\">{}@{} &mdash; {} {}</p></div>{}</div>",
        escape_html(&callback.host),
        escape_html(&callback.user_name),
        escape_html(&callback.host),
        escape_html(&callback.os),
        escape_html(&callback.arch),
        {
            let (label, cls) = callback_status(callback.status);
            format!("<span class=\"status-pill status-{cls}\">{}</span>", label)
        }
    );

    let tasking_panel = format!(
        "<div class=\"panel tasking-panel\"><h3>Task This Callback</h3>
        <form id=\"task-form\" action=\"/c2/sessions/{}/tasks\" method=\"POST\" data-sse-endpoint=\"/c2/sessions/{}/tasks/sse\">
            <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
            <div class=\"field-group\">
                <label for=\"command\">Command</label>
                <input list=\"command-suggestions\" type=\"text\" id=\"command\" name=\"command\" autocomplete=\"off\" placeholder=\"e.g. whoami\" required>
                <datalist id=\"command-suggestions\">{}</datalist>
            </div>
            <div class=\"field-group\">
                <label for=\"args\">Arguments</label>
                <input type=\"text\" id=\"args\" name=\"args\" placeholder=\"optional\">
            </div>
            <div class=\"form-actions\">
                <button type=\"submit\" class=\"btn btn-primary\">Execute &rarr;</button>
                <button type=\"button\" id=\"refresh-btn\" class=\"btn btn-ghost\">Refresh</button>
            </div>
        </form></div>",
        &callback.id, &callback.id, escape_html(csrf_token), suggestions_html
    );

    let content = format!(
        "<div class=\"callback-detail-shell\">{}{}
        <div id=\"task-results\" class=\"panel results-stream\"><h3>Task Results <span class=\"muted\">(live)</span></h3>
        <div id=\"results-log\" class=\"results-log\"></div></div>
        {}</div>",
        callback_detail, tasking_panel, rows
    );

    app_page("Callback Interact", user, "callbacks", &content)
}

/// Mythic-style event feed page — operation-wide audit events with chat input.
pub fn event_feed_page(user: &AuthenticatedUser, events: &[AuditEvent]) -> String {
    let rows = if events.is_empty() {
        "<section class=\"empty-state panel\"><p>No events yet.</p></section>".to_owned()
    } else {
        format!(
            "<div class=\"table-scroll\"><table class=\"data-table\"><caption>Recent events</caption><thead><tr><th scope=\"col\">Time</th><th scope=\"col\">Actor</th><th scope=\"col\">Action</th><th scope=\"col\">Outcome</th></tr></thead><tbody>{}</tbody></table></div>",
            events
                .iter()
                .map(|e| format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape_html(&e.created_at),
                    escape_html(&e.actor_id.clone().unwrap_or_default()),
                    escape_html(&e.action),
                    escape_html(&e.outcome)
                ))
                .collect::<String>()
        )
    };
    let chat = "<div class=\"panel\"><h3>Event Feed</h3><div class=\"chat-input\"><input type=\"text\" placeholder=\"Type a note...\" disabled><button class=\"btn btn-ghost\" disabled>Send</button></div></div>";
    let _content = format!("<div class=\"event-feed-shell\">{}</div>", chat);
    let _ = rows; // rows would go in the main panel
    app_page("Event Feed", user, "events", &format!("{}{}", chat, rows))
}

pub fn callbacks_page(user: &AuthenticatedUser, callbacks: &[Callback]) -> String {
    let rows = callbacks
        .iter()
        .map(|c| {
            let (status_label, status_class) = callback_status(c.status);
            let status = status_pill(status_label, status_class);
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td><a href=\"/callbacks/{}\" class=\"btn btn-sm btn-ghost\">Interact &rarr;</a></td></tr>",
                escape_html(&c.host),
                escape_html(&c.user_name),
                escape_html(&c.process),
                escape_html(&format!("{}/{}", c.os, c.arch)),
                escape_html(&c.protocol),
                status,
                escape_html(&c.last_seen),
                c.operation_id.as_deref().map(|id| format!("<code>{}</code>", escape_html(&id[..id.len().min(8)]))).unwrap_or_else(|| "&mdash;".into()),
                escape_html(&c.created_at),
                escape_html(&c.id),
            )
        })
        .collect::<String>();
    let table = if callbacks.is_empty() {
        "<section class=\"empty-state panel\"><p>No callbacks yet. Build a payload and run the implant to see beacons here.</p></section>".to_owned()
    } else {
        format!(
            "<div class=\"table-scroll\"><table class=\"data-table callbacks-table\"><caption>Active callbacks</caption><thead><tr><th scope=\"col\">Host</th><th scope=\"col\">User</th><th scope=\"col\">Process</th><th scope=\"col\">OS/Arch</th><th scope=\"col\">Proto</th><th scope=\"col\">Status</th><th scope=\"col\">Last seen</th><th scope=\"col\">Op</th><th scope=\"col\">First seen</th><th scope=\"col\">Actions</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    app_page(
        "Callbacks",
        user,
        "callbacks",
        &format!("<p>Inbound C2 sessions and beacons.</p>{table}"),
    )
}

pub fn eventing_page(
    user: &AuthenticatedUser,
    rules: &[EventRule],
    csrf_token: &str,
    error: Option<&str>,
) -> String {
    let error_html = form_error(error, "eventing-form-error");
    let rows = rules
        .iter()
        .map(|r| {
            let enabled = if r.enabled {
                status_pill("Enabled", "success")
            } else {
                status_pill("Disabled", "neutral")
            };
            format!(
                "<tr><td>{}</td><td><code>{}</code></td><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_html(&r.name),
                escape_html(&r.trigger),
                escape_html(&r.command),
                escape_html(&r.target),
                enabled,
                escape_html(&r.created_at),
            )
        })
        .collect::<String>();
    let list = if rules.is_empty() {
        "<section class=\"empty-state panel\"><p>No automation rules yet. Define a rule to push a command to all active targets automatically.</p></section>".to_owned()
    } else {
        format!(
            "<div class=\"table-scroll\"><table class=\"data-table event-rules-table\"><caption>Automation rules</caption><thead><tr><th scope=\"col\">Name</th><th scope=\"col\">Trigger</th><th scope=\"col\">Command</th><th scope=\"col\">Target</th><th scope=\"col\">State</th><th scope=\"col\">Created</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    let body = format!(
        "{error_html}<form class=\"form-panel panel\" method=\"post\" action=\"/eventing\"><input type=\"hidden\" name=\"csrf_token\" value=\"{token}\"><div class=\"form-field\"><label for=\"event-name\">Rule name</label><input id=\"event-name\" name=\"name\" placeholder=\"quarantine-on-beacon\" maxlength=\"80\" required></div><div class=\"form-field\"><label for=\"event-trigger\">Trigger</label><input id=\"event-trigger\" name=\"trigger\" placeholder=\"new callback\" maxlength=\"120\" required></div><div class=\"form-field\"><label for=\"event-command\">Command to run on all targets</label><input id=\"event-command\" name=\"command\" placeholder=\"collect system state\" maxlength=\"200\" required></div><div class=\"form-field\"><label for=\"event-target\">Target scope</label><select id=\"event-target\" name=\"target\"><option value=\"all\">All targets</option><option value=\"all_active\">All active only</option></select></div><button type=\"submit\">Create rule</button></form><h2>Automation rules</h2>{list}",
        token = escape_html(csrf_token),
    );
    app_page("Eventing", user, "eventing", &body)
}

pub fn services_page(user: &AuthenticatedUser, services: &[InstalledService]) -> String {
    let rows = services
        .iter()
        .map(|s| {
            let running = if s.running {
                status_pill("Running", "success")
            } else {
                status_pill("Stopped", "neutral")
            };
            format!(
                "<tr><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_html(&s.name),
                escape_html(&s.install_path),
                running,
                s.started_at.as_deref().map(escape_html).unwrap_or_else(|| "—".into()),
                s.asset_id.as_deref().map(|id| format!("<code>{}</code>", escape_html(&id[..id.len().min(8)]))).unwrap_or_else(|| "—".into()),
                escape_html(&s.created_at),
            )
        })
        .collect::<String>();
    let table = if services.is_empty() {
        "<section class=\"empty-state panel\"><p>No installed services reported yet.</p></section>"
            .to_owned()
    } else {
        format!(
            "<div class=\"table-scroll\"><table class=\"data-table services-table\"><caption>Installed services</caption><thead><tr><th scope=\"col\">Name</th><th scope=\"col\">Install path</th><th scope=\"col\">State</th><th scope=\"col\">Started</th><th scope=\"col\">Asset</th><th scope=\"col\">Created</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    app_page(
        "Services",
        user,
        "services",
        &format!("<p>Persistence services installed across targets.</p>{table}"),
    )
}

pub fn search_page(
    user: &AuthenticatedUser,
    query: &str,
    operations: &[Operation],
    assets: &[Asset],
    callbacks: &[Callback],
) -> String {
    let mut results = String::new();
    if !operations.is_empty() || !assets.is_empty() || !callbacks.is_empty() {
        results.push_str(&format!("<h2>Operations ({})</h2>", operations.len()));
        for operation in operations {
            results.push_str(&format!(
                "<p class=\"search-hit\"><a href=\"/operations/{id}\">{name}</a><span class=\"muted\">{purpose}</span></p>",
                id = url_encode(&operation.id),
                name = escape_html(&operation.name),
                purpose = escape_html(&operation.purpose),
            ));
        }
        results.push_str(&format!("<h2>Assets ({})</h2>", assets.len()));
        for asset in assets {
            results.push_str(&format!(
                "<p class=\"search-hit\"><a href=\"/operations/{op}\">{name}</a><span class=\"muted\">{owner} · {kind}</span></p>",
                op = url_encode(asset.operation_id.as_str()),
                name = escape_html(&asset.name),
                owner = escape_html(&asset.owner),
                kind = escape_html(&asset.kind),
            ));
        }
        results.push_str(&format!("<h2>Callbacks ({})</h2>", callbacks.len()));
        for callback in callbacks {
            results.push_str(&format!(
                "<p class=\"search-hit\">{}<span class=\"muted\">{} · {}</span></p>",
                escape_html(&callback.host),
                escape_html(&callback.os),
                escape_html(callback_status(callback.status).0),
            ));
        }
    } else if query.is_empty() {
        results = "<section class=\"empty-state panel\"><p>Search across operations, assets, and callbacks.</p></section>".to_owned();
    } else {
        results = format!(
            "<section class=\"empty-state panel\"><p>No results for “{}”.</p></section>",
            escape_html(query)
        );
    }
    let body = format!(
        "<form class=\"search-form\" action=\"/search\" method=\"get\"><div class=\"form-field\"><label for=\"search-query\">Search</label><input id=\"search-query\" name=\"q\" value=\"{q}\" placeholder=\"host, operation, asset…\"></div><button type=\"submit\">Search</button></form>{results}",
        q = escape_html(query),
    );
    app_page("Search Proxy", user, "search", &body)
}

pub fn operation_detail_page(
    user: &AuthenticatedUser,
    operation: &Operation,
    csrf_token: &str,
    error: Option<&str>,
) -> String {
    let error_html = form_error(error, "operation-form-error");
    let status_active = operation.status == OperationStatus::Active;
    let status_planned = operation.status == OperationStatus::Planned;
    let status_closed = operation.status == OperationStatus::Closed;
    let body = format!(
        "{error_html}<form class=\"form-panel panel\" method=\"post\" action=\"/operations/{id}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{token}\"><div class=\"form-field\"><label for=\"op-name\">Name</label><input id=\"op-name\" name=\"name\" value=\"{name}\" maxlength=\"80\" required></div><div class=\"form-field\"><label for=\"op-purpose\">Purpose</label><input id=\"op-purpose\" name=\"purpose\" value=\"{purpose}\" maxlength=\"160\" required></div><div class=\"form-field\"><label for=\"op-scope\">Scope note</label><input id=\"op-scope\" name=\"scope_note\" value=\"{scope}\" maxlength=\"160\"></div><div class=\"form-field\"><label for=\"op-status\">Status</label><select id=\"op-status\" name=\"status\"><option value=\"planned\"{sel_planned}>Planned</option><option value=\"active\"{sel_active}>Active</option><option value=\"closed\"{sel_closed}>Closed</option></select></div><button type=\"submit\">Save changes</button></form>",
        id = url_encode(&operation.id),
        token = escape_html(csrf_token),
        name = escape_html(&operation.name),
        purpose = escape_html(&operation.purpose),
        scope = escape_html(&operation.scope_note),
        sel_planned = if status_planned { " selected" } else { "" },
        sel_active = if status_active { " selected" } else { "" },
        sel_closed = if status_closed { " selected" } else { "" },
    );
    app_page("Modify Operation", user, "operations", &body)
}

fn callback_status(status: CallbackStatus) -> (&'static str, &'static str) {
    match status {
        CallbackStatus::Active => ("Active", "success"),
        CallbackStatus::Beacon => ("Beacon", "warning"),
        CallbackStatus::Dormant => ("Dormant", "neutral"),
        CallbackStatus::Lost => ("Lost", "danger"),
    }
}

fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub fn empty_page(
    title: &str,
    user: &AuthenticatedUser,
    active_nav: &str,
    message: &str,
) -> String {
    app_page(
        title,
        user,
        active_nav,
        &format!(
            "<section class=\"empty-state panel\"><p>{}</p></section>",
            escape_html(message)
        ),
    )
}

pub fn admin_page(user: &AuthenticatedUser) -> String {
    app_page(
        "Administration",
        user,
        "admin",
        "<section class=\"empty-state panel\"><p>Administration controls are available to local administrators.</p></section>",
    )
}

fn wolf_mark() -> &'static str {
    r#"<svg viewBox="0 0 48 48" fill="none" aria-hidden="true"><path d="m7 8 12 8h10l12-8-3 24-14 10L10 32Z" stroke="currentColor" stroke-width="2" stroke-linejoin="round"/><path d="m7 8 10 21 7 13 7-13L41 8M17 29l-5-9 12 5 12-5-5 9M20 33h8l-4 4Z" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/></svg>"#
}

fn page_description(name: &str) -> &'static str {
    match name {
        "dashboard" => "A little perspective. Every operation, in one place.",
        "operations" => "Clear scope. Deliberate work. A place for every operation.",
        "inventory" => "The assets that make up your field of view.",
        "checks" => "A traceable record of what you have observed.",
        "evidence" => "The details that turn observations into findings.",
        "reports" => "Bring the complete story into view.",
        "audit" => "Every action leaves a record. Follow it here.",
        "admin" => "The people and permissions behind your workspace.",
        "callbacks" => "Your connections, organized and in context.",
        "events" => "A running record of workspace activity.",
        "search" => "Find the detail you are looking for.",
        _ => "Your workspace, with every detail in focus.",
    }
}

fn nav_icon(name: &str) -> String {
    let paths = match name {
        "dashboard" => {
            r#"<rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/>"#
        }
        "operations" => r#"<circle cx="12" cy="12" r="9"/><path d="m16 8-2.5 5.5L8 16l2.5-5.5Z"/>"#,
        "callbacks" => r#"<path d="m13 2-9 12h7l-1 8 10-13h-7Z"/>"#,
        "services" => {
            r#"<rect x="3" y="3" width="18" height="7" rx="2"/><rect x="3" y="14" width="18" height="7" rx="2"/><path d="M7 6.5h.01M7 17.5h.01M11 6.5h6M11 17.5h6"/>"#
        }
        "eventing" => {
            r#"<path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9M10 21h4M12 2V1"/>"#
        }
        "events" => r#"<path d="M3 12h4l3-8 4 16 3-8h4"/>"#,
        "inventory" => {
            r#"<path d="m12 3 9 5v9l-9 5-9-5V8Zm0 10 9-5M12 13 3 8m9 5v9M7.5 5.5l9 5"/>"#
        }
        "checks" => {
            r#"<rect x="5" y="4" width="14" height="17" rx="2"/><path d="M9 4V2h6v2M8 12l3 3 5-6"/>"#
        }
        "audit" => r#"<path d="M5 3h14v19l-3-2-4 2-4-2-3 2ZM8 8h8M8 12h8M8 16h5"/>"#,
        "evidence" => r#"<path d="M3 7V4h6l3 3h9v13H3ZM3 10h18"/>"#,
        "reports" => r#"<path d="M5 2h9l5 5v15H5ZM14 2v6h5M8 12h8M8 16h8"/>"#,
        "payloads" => r#"<path d="m12 3 9 5-9 5-9-5Zm-9 9 9 5 9-5M3 16l9 5 9-5"/>"#,
        "admin" => r#"<path d="m12 2 8 4v6c0 6-8 10-8 10S4 18 4 12V6Z"/><path d="m8 12 3 3 5-6"/>"#,
        "search" => r#"<circle cx="10.5" cy="10.5" r="7.5"/><path d="m16 16 5 5"/>"#,
        _ => r#"<circle cx="12" cy="12" r="8"/>"#,
    };
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{paths}</svg>"#
    )
}

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

const APP_ASSETS: &str = r#"<script defer src="/static/anime.min.js"></script><script defer src="/static/admin.js"></script>"#;

fn page_shell(title: &str, head_extra: &str, content: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{} · NaughtyWolf</title><link rel=\"stylesheet\" href=\"/static/admin.css\">{head_extra}</head><body>{content}</body></html>",
        escape_html(title),
    )
}

fn form_error(error: Option<&str>, id: &str) -> String {
    error
        .map(|message| {
            format!(
                "<p class=\"form-error\" role=\"alert\" id=\"{}\">{}</p>",
                escape_html(id),
                escape_html(message),
            )
        })
        .unwrap_or_default()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
