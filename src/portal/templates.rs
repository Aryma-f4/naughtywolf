use crate::{
    auth::{AuthenticatedUser, rbac::Role},
    db::{
        models::{AuditEvent, CheckRun, Evidence, Operation},
        repositories::PortalUser,
    },
    portal::{DashboardSummary, OperationReportSummary},
};

pub fn public_landing() -> String {
    page_shell(
        "NaughtyWolf",
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

    page_shell(
        "Sign in",
        &format!(
            "<main class=\"login-page\"><form method=\"post\" action=\"/login\"><h1>Sign in</h1>{error}<input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><label for=\"username\">Username</label><input id=\"username\" name=\"username\" autocomplete=\"username\" required><label for=\"password\">Password</label><input id=\"password\" name=\"password\" type=\"password\" autocomplete=\"current-password\" required><button type=\"submit\">Sign in</button></form></main>",
            escape_html(csrf_token)
        ),
    )
}

pub fn app_page(title: &str, user: &AuthenticatedUser, active_nav: &str, body: &str) -> String {
    let dock = [
        ("dashboard", "/dashboard", "Dashboard"),
        ("operations", "/operations", "Operations"),
        ("inventory", "/inventory", "Inventory"),
        ("checks", "/checks", "Checks"),
        ("audit", "/audit", "Audit"),
        ("evidence", "/evidence", "Evidence"),
        ("reports", "/reports", "Reports"),
    ]
    .into_iter()
    .chain(
        (user.role == Role::Admin).then_some(("admin", "/admin/users", "Admin")),
    )
    .map(|(name, href, label)| {
        let current = (name == active_nav).then_some(" aria-current=\"page\"").unwrap_or("");
        format!(
            "<a class=\"dock-link\" href=\"{href}\"{current}><span class=\"dock-label\">{label}</span></a>"
        )
    })
    .collect::<String>();

    page_shell(
        title,
        &format!(
            "<a class=\"skip-link\" href=\"#main-content\">Skip to main content</a><header class=\"portal-header\"><a class=\"portal-brand\" href=\"/dashboard\">NaughtyWolf</a><p class=\"portal-identity\"><span>{}</span><span class=\"role-badge\">{}</span></p><form method=\"post\" action=\"/logout\"><button class=\"logout-button\" type=\"submit\">Sign out</button></form></header><nav class=\"portal-dock\" data-dock aria-label=\"Portal\">{dock}</nav><main id=\"main-content\" class=\"portal-main\"><h1>{}</h1>{body}</main>",
            escape_html(&user.username),
            escape_html(&user.role.to_string()),
            escape_html(title),
        ),
    )
}

pub fn dashboard_page(user: &AuthenticatedUser, summary: &DashboardSummary) -> String {
    app_page(
        "Dashboard",
        user,
        "dashboard",
        &format!(
            "<section class=\"summary-grid\" aria-label=\"Scoped summary\"><article><h2>Operations</h2><p>{}</p></article><article><h2>Assets</h2><p>{}</p></article><article><h2>Check runs</h2><p>{}</p></article><article><h2>Evidence</h2><p>{}</p></article><article><h2>Audit records</h2><p>{}</p></article></section>",
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
        format!("<section class=\"zero-state\"><p>No scoped operations yet</p></section>")
    } else {
        let items = operations
            .iter()
            .map(|operation| {
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
                    "<li><h2>{}</h2><p>{}</p><p class=\"muted\">Status: {:?}</p>{asset_link}</li>",
                    escape_html(&operation.name),
                    escape_html(&operation.purpose),
                    operation.status,
                )
            })
            .collect::<String>();
        format!("<ul class=\"record-list\">{items}</ul>")
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
        "<section class=\"zero-state\"><p>No scoped checks yet</p></section>".to_owned()
    } else {
        let rows = runs
            .iter()
            .map(|run| {
                format!(
                    "<tr><td><code>{}</code></td><td>{}</td><td>{:?}</td><td><code>{}</code></td><td>{}</td></tr>",
                    escape_html(&run.id),
                    escape_html(&run.check_id),
                    run.state,
                    escape_html(&run.operation_id),
                    escape_html(&run.created_at),
                )
            })
            .collect::<String>();
        format!(
            "<div class=\"table-scroll\"><table><caption>Scoped check-run history</caption><thead><tr><th scope=\"col\">Run</th><th scope=\"col\">Check</th><th scope=\"col\">State</th><th scope=\"col\">Operation</th><th scope=\"col\">Created</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    app_page("Checks", user, "checks", &body)
}

pub fn audit_page(user: &AuthenticatedUser, events: &[AuditEvent]) -> String {
    let body = if events.is_empty() {
        "<section class=\"zero-state\"><p>No scoped audit records yet</p></section>".to_owned()
    } else {
        let rows = events
            .iter()
            .map(|event| {
                format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape_html(&event.action),
                    escape_html(event.actor_id.as_deref().unwrap_or("System")),
                    escape_html(&event.target_type),
                    escape_html(event.target_id.as_deref().unwrap_or("—")),
                    escape_html(&event.outcome),
                    escape_html(&event.created_at),
                )
            })
            .collect::<String>();
        format!(
            "<div class=\"table-scroll\"><table><caption>Scoped, append-only audit history</caption><thead><tr><th scope=\"col\">Action</th><th scope=\"col\">Actor</th><th scope=\"col\">Target type</th><th scope=\"col\">Target</th><th scope=\"col\">Outcome</th><th scope=\"col\">Created</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    app_page("Audit", user, "audit", &body)
}

pub fn evidence_page(user: &AuthenticatedUser, records: &[Evidence]) -> String {
    let body = if records.is_empty() {
        "<section class=\"zero-state\"><p>No scoped evidence yet</p></section>".to_owned()
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
            "<div class=\"table-scroll\"><table><caption>Scoped evidence metadata</caption><thead><tr><th scope=\"col\">Evidence</th><th scope=\"col\">Content type</th><th scope=\"col\">Size</th><th scope=\"col\">SHA-256 prefix</th><th scope=\"col\">Created</th><th scope=\"col\">File</th></tr></thead><tbody>{rows}</tbody></table></div>"
        )
    };
    app_page("Evidence", user, "evidence", &body)
}

pub fn reports_page(user: &AuthenticatedUser, summaries: &[OperationReportSummary]) -> String {
    let body = if summaries.is_empty() {
        "<section class=\"zero-state\"><p>No scoped reports yet</p></section>".to_owned()
    } else {
        let reports = summaries
            .iter()
            .map(|summary| {
                format!(
                    "<article class=\"report-card\"><h2>{}</h2><p>{}</p><dl class=\"report-counts\"><div><dt>Assets</dt><dd>{}</dd></div><div><dt>Queued</dt><dd>{}</dd></div><div><dt>Running</dt><dd>{}</dd></div><div><dt>Succeeded</dt><dd>{}</dd></div><div><dt>Failed</dt><dd>{}</dd></div><div><dt>Cancelled</dt><dd>{}</dd></div><div><dt>Audit records</dt><dd>{}</dd></div></dl></article>",
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
            "<p class=\"print-note\">Use your browser’s print command for a printable copy.</p><section class=\"report-list\" aria-label=\"Operation summaries\">{reports}</section>"
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
            let status = if account.disabled { "Disabled" } else { "Enabled" };
            format!(
                "<tr><td>{}</td><td>{}</td><td>{status}</td><td>{}</td><td>{controls}</td></tr>",
                escape_html(&account.username),
                escape_html(&account.role.to_string()),
                escape_html(&account.created_at),
            )
        })
        .collect::<String>();
    let error = form_error(error, "admin-users-error");
    let body = format!(
        "{error}<div class=\"table-scroll\"><table><caption>Local user accounts</caption><thead><tr><th scope=\"col\">Username</th><th scope=\"col\">Role</th><th scope=\"col\">Status</th><th scope=\"col\">Created</th><th scope=\"col\">Controls</th></tr></thead><tbody>{rows}</tbody></table></div>"
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
            "<form class=\"record-form\" method=\"post\" action=\"/operations\">{error}<input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><label for=\"operation-name\">Name</label><input id=\"operation-name\" name=\"name\" value=\"{}\" maxlength=\"160\" required{described_by}><label for=\"operation-purpose\">Purpose</label><input id=\"operation-purpose\" name=\"purpose\" value=\"{}\" maxlength=\"160\" required{described_by}><button type=\"submit\">Create operation</button></form>",
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
            "<form class=\"record-form\" method=\"post\" action=\"/operations/{}/assets\">{error}<input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><label for=\"asset-name\">Name</label><input id=\"asset-name\" name=\"name\" value=\"{}\" maxlength=\"160\" required{described_by}><label for=\"asset-kind\">Kind</label><input id=\"asset-kind\" name=\"kind\" value=\"{}\" maxlength=\"160\" required{described_by}><label for=\"asset-owner\">Owner</label><input id=\"asset-owner\" name=\"owner\" value=\"{}\" maxlength=\"160\" required{described_by}><label for=\"asset-address\">Address</label><input id=\"asset-address\" name=\"address\" value=\"{}\" maxlength=\"160\" required{described_by}><button type=\"submit\">Add asset</button></form>",
            escape_html(operation_id),
            escape_html(csrf_token),
            escape_html(values[0]),
            escape_html(values[1]),
            escape_html(values[2]),
            escape_html(values[3]),
        ),
    )
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
            "<section class=\"zero-state\"><p>{}</p></section>",
            escape_html(message)
        ),
    )
}

pub fn admin_page(user: &AuthenticatedUser) -> String {
    app_page(
        "Administration",
        user,
        "admin",
        "<section class=\"zero-state\"><p>Administration controls are available to local administrators.</p></section>",
    )
}

fn page_shell(title: &str, content: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{} · NaughtyWolf</title><link rel=\"stylesheet\" href=\"/static/admin.css\"><script src=\"/static/admin.js\" defer></script></head><body>{content}</body></html>",
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
