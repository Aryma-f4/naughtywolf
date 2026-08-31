use crate::{
    auth::{AuthenticatedUser, rbac::Role},
    db::{
        models::{
            Asset, AuditEvent, Callback, CallbackStatus, CheckRun, EventRule, Evidence,
            InstalledService, Operation, OperationStatus, RunState,
        },
        repositories::PortalUser,
    },
    payload::{BuildJob, PayloadMeta},
    portal::{DashboardSummary, OperationReportSummary},
};

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
        r#"<main class="login-shell"><section class="login-brand-panel" aria-labelledby="login-brand-title"><div class="brand-lockup"><span class="brand-mark" aria-hidden="true">NW</span><span class="brand-name"><strong>NaughtyWolf</strong><small>Operator Portal</small></span></div><div class="login-brand-copy"><p class="eyebrow">Authorized Operations Workspace</p><h1 id="login-brand-title">Operate with scope, evidence, and accountability.</h1><p>Manage authorized lab records from one local control surface.</p></div><ul class="trust-list" aria-label="Portal capabilities"><li>Scoped operations</li><li>Append-only audit</li><li>Verified evidence</li></ul></section><section class="login-form-panel" aria-labelledby="login-title"><form class="login-card" method="post" action="/login"><p class="eyebrow">Local operator access</p><h2 id="login-title">Sign in</h2><p class="login-intro">Use your assigned local account.</p>{error}<input type="hidden" name="csrf_token" value="{csrf_token}"><div class="form-field"><label for="username">Username</label><input id="username" name="username" autocomplete="username" required></div><div class="form-field"><label for="password">Password</label><input id="password" name="password" type="password" autocomplete="current-password" required></div><button class="primary-action" type="submit">Enter workspace</button><p class="login-guardrail">Authorized lab access only</p></form></section></main>"#,
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
            vec![("eventing", "/eventing", "Eventing")],
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
    let operator_only = ["callbacks", "services", "eventing", "search", "payloads"];
    if !user.role.allows(Role::Operator) {
        for group in groups.iter_mut() {
            group
                .1
                .retain(|(name, _, _)| !operator_only.contains(name));
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
            "<a class=\"quick-link\" href=\"{href}\"{current}><span class=\"material-symbols-outlined nav-icon\" aria-hidden=\"true\">{}</span><span>{label}</span></a>",
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
                        "<a class=\"nav-link\" href=\"{href}\"{current}><span class=\"material-symbols-outlined nav-icon\" aria-hidden=\"true\">{}</span><span class=\"nav-label\">{label}</span></a>",
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
            r##"<div class="portal-shell"><a class="skip-link" href="#main-content">Skip to main content</a><header class="portal-topbar" data-topbar><a class="portal-brand" href="/dashboard"><span class="brand-mark" aria-hidden="true">NW</span><span class="brand-name"><strong>NaughtyWolf</strong><small>Operator Console</small></span></a><nav class="topbar-quick" aria-label="Quick access">{quick_links}</nav><div class="operator-identity"><span class="operator-name">{username}</span><span class="role-badge">{role}</span><form method="post" action="/logout"><button class="logout-button" type="submit">Sign out</button></form></div></header><nav class="primary-nav" aria-label="Primary navigation" data-primary-nav>{nav}</nav><main id="main-content" class="portal-main"><div class="page-heading"><p class="eyebrow">Authorized workspace</p><h1>{title}</h1></div>{body}</main></div>"##,
            username = escape_html(&user.username),
            role = escape_html(&user.role.to_string()),
            title = escape_html(title),
            body = body,
            quick_links = quick_links,
        ),
    )
}

pub fn dashboard_page(user: &AuthenticatedUser, summary: &DashboardSummary) -> String {
    app_page(
        "Dashboard",
        user,
        "dashboard",
        &format!(
            "<section class=\"metric-grid\" aria-label=\"Scoped summary\"><article class=\"metric-card\"><span>Operations</span><strong>{}</strong><small>Authorized lab operations</small></article><article class=\"metric-card\"><span>Assets</span><strong>{}</strong><small>Scoped targets</small></article><article class=\"metric-card\"><span>Check runs</span><strong>{}</strong><small>Executed checks</small></article><article class=\"metric-card\"><span>Evidence</span><strong>{}</strong><small>Stored artifacts</small></article><article class=\"metric-card\"><span>Audit records</span><strong>{}</strong><small>Append-only events</small></article></section>",
            summary.operation_count,
            summary.asset_count,
            summary.run_count,
            summary.evidence_count,
            summary.audit_count,
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
    ("x86_64-unknown-linux-musl", "Linux x86-64 (musl)", "linux", "amd64"),
    ("x86_64-unknown-linux-gnu", "Linux x86-64 (glibc)", "linux", "amd64"),
    ("aarch64-unknown-linux-musl", "Linux ARM64 (musl)", "linux", "arm64"),
    ("x86_64-pc-windows-gnu", "Windows x86-64", "windows", "amd64"),
    ("aarch64-apple-darwin", "macOS Apple Silicon", "macos", "arm64"),
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
                escape_html(&format!("{} ({}/{}, {})", j.name, j.os, j.arch, if j.target.is_empty() { "native".into() } else { j.target.clone() }))
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
    let visible_platform = if f_target.is_empty() && f_os == current_os() && f_arch == current_arch_label() {
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
        btn_label = if editor.is_some() { "Recompile implant" } else { "Build implant" },
        protocol_options = protocol_options,
    );
    app_page("Payloads", user, "payloads", &body)
}

pub fn callbacks_page(user: &AuthenticatedUser, callbacks: &[Callback]) -> String {
    let rows = callbacks
        .iter()
        .map(|c| {
            let (status_label, status_class) = callback_status(c.status);
            let status = status_pill(status_label, status_class);
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_html(&c.host),
                escape_html(&c.user_name),
                escape_html(&c.process),
                escape_html(&format!("{}/{}", c.os, c.arch)),
                escape_html(&c.protocol),
                status,
                escape_html(&c.last_seen),
                c.operation_id.as_deref().map(|id| format!("<code>{}</code>", escape_html(&id[..id.len().min(8)]))).unwrap_or_else(|| "—".into()),
                escape_html(&c.created_at),
            )
        })
        .collect::<String>();
    let table = if callbacks.is_empty() {
        "<section class=\"empty-state panel\"><p>No callbacks yet. Build a payload and run the implant to see beacons here.</p></section>".to_owned()
    } else {
        format!(
            "<div class=\"table-scroll\"><table class=\"data-table callbacks-table\"><caption>Active callbacks</caption><thead><tr><th scope=\"col\">Host</th><th scope=\"col\">User</th><th scope=\"col\">Process</th><th scope=\"col\">OS/Arch</th><th scope=\"col\">Proto</th><th scope=\"col\">Status</th><th scope=\"col\">Last seen</th><th scope=\"col\">Op</th><th scope=\"col\">First seen</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    app_page("Callbacks", user, "callbacks", &format!("<p>Inbound C2 sessions and beacons.</p>{table}"))
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
        "<section class=\"empty-state panel\"><p>No installed services reported yet.</p></section>".to_owned()
    } else {
        format!(
            "<div class=\"table-scroll\"><table class=\"data-table services-table\"><caption>Installed services</caption><thead><tr><th scope=\"col\">Name</th><th scope=\"col\">Install path</th><th scope=\"col\">State</th><th scope=\"col\">Started</th><th scope=\"col\">Asset</th><th scope=\"col\">Created</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    app_page("Services", user, "services", &format!("<p>Persistence services installed across targets.</p>{table}"))
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

fn nav_icon(name: &str) -> &'static str {
    match name {
        "dashboard" => "dashboard",
        "operations" => "explore",
        "callbacks" => "bolt",
        "services" => "storage",
        "eventing" => "notifications_active",
        "inventory" => "inventory_2",
        "checks" => "fact_check",
        "audit" => "receipt_long",
        "evidence" => "folder_open",
        "reports" => "description",
        "payloads" => "rocket_launch",
        "admin" => "admin_panel_settings",
        "search" => "search",
        _ => "",
    }
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

const APP_ASSETS: &str = r#"<link rel="preconnect" href="https://fonts.googleapis.com"><link rel="preconnect" href="https://fonts.gstatic.com" crossorigin><link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Material+Symbols+Outlined:opsz,wght,FILL,GRAD@20..48,100..700,0..1,-50..200&display=block"><script defer src="/static/anime.min.js"></script><script defer src="/static/admin.js"></script>"#;

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
