use crate::{
    auth::{AuthenticatedUser, rbac::Role},
    db::models::Operation,
    portal::DashboardSummary,
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
    .chain((user.role == Role::Admin).then_some(("admin", "/admin", "Admin")))
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
    let body = if operations.is_empty() {
        let create_link = user
            .role
            .allows(Role::Operator)
            .then_some("<a class=\"button\" href=\"/operations/new\">Create operation</a>")
            .unwrap_or_default();
        format!(
            "<section class=\"zero-state\"><p>No scoped operations yet</p>{create_link}</section>"
        )
    } else {
        let items = operations
            .iter()
            .map(|operation| {
                format!(
                    "<li><h2>{}</h2><p>{}</p><p class=\"muted\">Status: {:?}</p></li>",
                    escape_html(&operation.name),
                    escape_html(&operation.purpose),
                    operation.status,
                )
            })
            .collect::<String>();
        format!("<ul class=\"record-list\">{items}</ul>")
    };

    app_page("Operations", user, "operations", &body)
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

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
