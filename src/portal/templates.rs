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
    page_shell("Sign in", MOTION_ASSETS, &content)
}

pub fn app_page(title: &str, user: &AuthenticatedUser, active_nav: &str, body: &str) -> String {
    let mut groups: Vec<(&str, Vec<(&str, &str, &str)>)> = vec![
        (
            "Command",
            vec![
                ("dashboard", "/dashboard", "Dashboard"),
                ("topology", "/topology", "Topology"),
                ("recon", "/recon", "Recon"),
                ("callbacks", "/callbacks", "Active Callbacks"),
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
                ("payloads", "/payloads", "Create Payload"),
                ("search", "/search", "Search"),
                ("guide", "/guide", "Operator Guide"),
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
        ("callbacks", "/callbacks", "Active"),
        ("events", "/events", "Events"),
        ("payloads", "/payloads", "Create"),
        ("guide", "/guide", "Guide"),
    ]
    .iter()
    .filter(|(name, _, _)| matches!(*name, "dashboard" | "guide") || can_operator)
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
                    let feature_class = if name == "payloads" {
                        " nav-link-featured"
                    } else {
                        ""
                    };
                    format!(
                        "<a class=\"nav-link{feature_class}\" data-nav-item=\"{name}\" href=\"{href}\"{current}><span class=\"nav-icon\" aria-hidden=\"true\">{}</span><span class=\"nav-label\">{label}</span></a>",
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
            r##"<div class="portal-shell"><a class="skip-link" href="#main-content">Skip to main content</a><header class="portal-topbar" data-topbar><a class="portal-brand" href="/dashboard"><span class="brand-mark" aria-hidden="true">{wolf}</span><span class="brand-name"><strong>naughtywolf<span class="brand-period">.</span></strong><small>Operator fieldnotes</small></span></a><nav class="topbar-quick" aria-label="Quick access">{quick_links}</nav><div class="operator-identity"><span class="operator-avatar" aria-hidden="true">{initial}</span><span class="operator-name">{username}</span><span class="role-badge">{role}</span><form method="post" action="/logout"><button class="logout-button" type="submit">Sign out <span aria-hidden="true">↗</span></button></form></div></header><nav class="primary-nav" aria-label="Primary navigation" data-primary-nav><div class="nav-workspace"><span class="workspace-symbol" aria-hidden="true">N / W</span><div><strong>Local workspace</strong><small>Scoped by design</small></div></div>{nav}<div class="nav-footnote"><span class="eyebrow">A sharper instinct.</span><p>Observe carefully.<br>Leave a clear record.</p><span class="nav-edition">NW — FIELD EDITION 01</span></div></nav><main id="main-content" class="portal-main" tabindex="-1"><div class="page-heading"><div><p class="eyebrow">Workspace <span aria-hidden="true">/</span> {title}</p><h1>{heading}</h1><p class="page-description">{description}</p></div><span class="page-stamp">{stamp}</span></div>{body}<footer class="workspace-footer"><span>NAUGHTYWOLF<span class="brand-period">.</span> <span class="footer-divider">/</span> OPERATOR FIELDNOTES</span><span>Made for the details.</span></footer></main></div>"##,
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

pub fn guide_page(user: &AuthenticatedUser) -> String {
    let body = r##"
<section class="guide-page" data-guide-page>
  <header class="guide-hero panel">
    <div><p class="eyebrow">OPERATOR GUIDE / START HERE</p><h2>From a blank workspace<br>to a traceable assessment.</h2><p>Follow one connected workflow for an authorized lab. Each step points to the exact NaughtyWolf workspace you need next.</p></div>
    <nav class="guide-jump" aria-label="Guide sections"><a href="#guide-setup">Setup</a><a href="#guide-workflow">Workflow</a><a href="#guide-payload">Payloads</a><a href="#guide-callback">Callbacks</a><a href="#guide-deploy">Deploy</a></nav>
  </header>

  <section id="guide-setup" class="guide-section panel"><div class="guide-section-index">00</div><div><p class="eyebrow">BEFORE YOU BEGIN</p><h3>Prepare the workspace.</h3><p>Set a stable SQLite database URL and session secret, create an administrator, then start the portal. Keep the same session secret between restarts so signed sessions remain valid.</p><pre><code>cp .env.example .env
cargo run -p naughtywolf -- user create --username admin --role admin
cargo run -p naughtywolf -- serve</code></pre><div class="guide-note"><strong>Scope first</strong><span>Use NaughtyWolf only for systems you own or have explicit permission to assess.</span></div></div></section>

  <section id="guide-workflow" class="guide-section"><div class="guide-section-index">01—06</div><div><p class="eyebrow">THE WALKTHROUGH</p><h3>One operation, six deliberate steps.</h3><div class="guide-steps">
    <article><span>01</span><h4>1. Define scope</h4><p>Create an operation with a clear purpose and boundary. Every asset, check, and record stays attached to that context.</p><a href="/operations/new">Create an operation ↗</a></article>
    <article><span>02</span><h4>2. Add authorized assets</h4><p>Register a hostname, domain, or IP that belongs to the approved operation. Use a label your team will recognize later.</p><a href="/operations">Open operations ↗</a></article>
    <article><span>03</span><h4>3. Run reconnaissance</h4><p>Select a scoped asset, choose DNS or DNS plus HTTP/TLS observation, and save the result into check history.</p><a href="/recon">Open recon ↗</a></article>
    <article><span>04</span><h4>4. Build a payload</h4><p>Choose the exact target platform, enter a reachable callback address, select transport, and review beacon timing.</p><a href="/payloads">Open Payload Studio ↗</a></article>
    <article><span>05</span><h4>5. Interact with callbacks</h4><p>Open a registered session to run user enumeration, a PEASS assessment, or bounded custom code from Module Studio.</p><a href="/callbacks">Open callbacks ↗</a></article>
    <article><span>06</span><h4>6. Preserve evidence</h4><p>Review task output, stored artifacts, reports, and the append-only audit trail before closing the operation.</p><a href="/evidence">Review evidence ↗</a></article>
  </div></div></section>

  <section id="guide-payload" class="guide-section panel"><div class="guide-section-index">P</div><div><p class="eyebrow">PAYLOAD FIELD GUIDE</p><h3>Choose values the target can actually use.</h3><div class="guide-reference">
    <div><strong>LHOST</strong><p>Use a DNS name such as <code>c2.lab.example</code> when records and TLS are managed, or an IPv4/IPv6 address such as <code>10.20.0.5</code> for a direct lab route. Enter only the host, without <code>http://</code> or a path.</p></div>
    <div><strong>LPORT</strong><p>Use the port exposed by your listener or reverse proxy. Confirm routing and lab-firewall rules from the target network before building.</p></div>
    <div><strong>Protocol</strong><p>HTTP uses the portal callback endpoint, TCP uses the raw listener, and GSocket uses its relay with a loopback local forward.</p></div>
    <div><strong>Timing</strong><p>A shorter interval gives faster feedback but more traffic. Add modest jitter below the base interval to avoid synchronized lab callbacks.</p></div>
  </div><a class="button" href="/payloads">Build with guided fields <span aria-hidden="true">↗</span></a></div></section>

  <section id="guide-callback" class="guide-section panel"><div class="guide-section-index">M</div><div><p class="eyebrow">MODULE STUDIO</p><h3>Assess, then decide.</h3><p>User enumeration is read-only. PEASS downloads the current official platform asset, reports its SHA-256 and CVE references, and leaves automatic exploitation disabled. Custom code is sent as UTF-8 base64, written to a private temporary file, run with a timeout, and removed afterward.</p><div class="guide-note"><strong>Review the output</strong><span>A privilege-escalation candidate is a lead. Validate host context and impact before taking a privilege-changing action.</span></div><a href="/callbacks">Choose a callback ↗</a></div></section>

  <section id="guide-deploy" class="guide-section"><div class="guide-section-index">D</div><div><p class="eyebrow">DEPLOYMENT</p><h3>Take the same workflow to Coolify.</h3><p>Deploy the root Docker Compose file, configure unique session, C2, and GSocket secrets, attach HTTPS to the app service, and keep the raw listener private to the Compose network.</p><div class="guide-links"><a href="https://github.com/Aryma-f4/naughtywolf/blob/develop/docs/coolify.md" target="_blank" rel="noreferrer">Coolify guide ↗</a><a href="https://github.com/Aryma-f4/naughtywolf/blob/develop/docs/modules.md" target="_blank" rel="noreferrer">Module reference ↗</a></div></div></section>
</section>"##;
    app_page("Operator Guide", user, "guide", body)
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
                url_encode(&b.public_id),
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

    let (f_os, f_arch, f_target, f_name, f_lhost, f_lport, f_protocol, f_interval, f_jitter) =
        match editor {
            Some(m) => (
                m.os.clone(),
                m.arch.clone(),
                m.target.clone(),
                m.name.clone(),
                m.lhost.clone(),
                m.lport,
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
        r#"{error_html}{errors_html}{building_html}
<section class="payload-studio" aria-labelledby="payload-studio-title">
  <header class="payload-studio-head panel">
    <span class="payload-creation-icon" aria-hidden="true">{payload_icon}</span>
    <div><p class="eyebrow">BUILD STUDIO / NATIVE RUNTIME</p><h2 id="payload-studio-title">Shape the field kit.</h2><p>Choose a target, configure its callback, tune beacon timing, then review the exact build profile.</p></div>
    <span class="payload-studio-mark">NW / BUILD</span>
  </header>
  <form class="payload-wizard panel" method="post" action="/payloads/generate" data-payload-form data-payload-wizard>
    <input type="hidden" name="csrf_token" value="{token}">
    <nav class="payload-progress" aria-label="Payload creation progress">
      <button type="button" data-wizard-tab="0"><span>01</span><strong>Target</strong><small>OS &amp; identity</small></button>
      <button type="button" data-wizard-tab="1"><span>02</span><strong>Callback</strong><small>Endpoint &amp; protocol</small></button>
      <button type="button" data-wizard-tab="2"><span>03</span><strong>Timing</strong><small>Beacon behavior</small></button>
      <button type="button" data-wizard-tab="3"><span>04</span><strong>Review</strong><small>Confirm &amp; build</small></button>
    </nav>
    <div class="payload-step" data-payload-step="0">
      <div class="payload-step-copy"><span>STEP 01</span><h3>Select the target.</h3><p>Name this build and choose the operating system and architecture that will receive it.</p></div>
      <div class="payload-fields">
        <div class="form-field"><label for="payload-name">Payload name</label><input id="payload-name" name="name" value="{f_name}" placeholder="finance-lab-linux-amd64" maxlength="80" required><p class="muted field-hint"><strong>Example:</strong> <code>finance-lab-linux-amd64</code>. Use an operation, purpose, and platform label that stays recognizable in the artifact library.</p></div>
        <div class="form-field"><label for="payload-platform">Target platform</label><select id="payload-platform">{platform_options}</select><p class="muted field-hint" id="payload-platform-hint">{visible_platform}</p><p class="muted field-hint"><strong>Best practice:</strong> match both the operating system and CPU architecture of the authorized host. An amd64 build will not run natively on arm64.</p><input type="hidden" name="os" id="payload-os" value="{f_os}"><input type="hidden" name="arch" id="payload-arch" value="{f_arch}"><input type="hidden" name="target" id="payload-target" value="{f_target}"></div>
      </div>
    </div>
    <div class="payload-step" data-payload-step="1">
      <div class="payload-step-copy"><span>STEP 02</span><h3>Configure the callback.</h3><p>Point the build at an authorized native runtime and use the same shared secret on both sides.</p></div>
      <div class="payload-fields payload-fields-grid">
        <div class="form-field payload-field-wide"><label for="payload-lhost">Callback host (LHost)</label><input id="payload-lhost" name="lhost" value="{f_lhost}" placeholder="c2.lab.example or 10.20.0.5" required><p class="muted field-hint"><strong>Accepted:</strong> A DNS name such as <code>c2.lab.example</code>, or an IPv4 or IPv6 address reachable from the target. Enter the host only—no scheme, port, or URL path.</p></div>
        <div class="form-field"><label for="payload-lport">Callback port</label><input id="payload-lport" name="lport" type="number" value="{f_lport}" min="1" max="65535" placeholder="8443" required><p class="muted field-hint"><strong>Example:</strong> <code>443</code>, <code>8443</code>, or the private raw-listener port. Choose a port allowed by your lab firewall and exposed by the matching listener.</p></div>
        <div class="form-field"><label for="payload-proto">Protocol</label><select id="payload-proto" name="protocol">{protocol_options}</select><p class="muted field-hint"><strong>HTTP</strong> connects to the portal, <strong>TCP</strong> needs the raw listener, and <strong>GSocket</strong> uses the relay and a local forward.</p></div>
        <aside class="timing-note payload-field-full"><span class="status-dot"></span><div><strong>Shared secret (NW_PSK)</strong><p>The build uses the active server secret automatically, so the implant and NaughtyWolf listener cannot drift out of sync.</p></div></aside>
        <div class="payload-gsocket-fields payload-field-full" data-gsocket-fields hidden>
          <div class="form-field"><label for="payload-gsocket-secret">GSocket tunnel secret</label><div class="input-with-btn"><input id="payload-gsocket-secret" name="gsocket_secret" placeholder="generate-a-separate-relay-secret" disabled><button type="button" class="btn btn-sm btn-ghost" data-gsocket-secret-generate>Generate secret</button></div><p class="muted field-hint"><strong>Best practice:</strong> use a separate random value shared only with gs-netcat. It is never shown in the review or artifact list.</p></div>
          <div class="form-field"><label for="payload-gsocket-port">Local forward port</label><input id="payload-gsocket-port" name="gsocket_local_port" type="number" value="4630" min="1" max="65535" placeholder="4630" disabled><p class="muted field-hint"><strong>Example:</strong> <code>4630</code>. Choose an unused loopback port; it does not need to be publicly exposed.</p></div>
          <p class="muted payload-field-full">The callback host is pinned to loopback because gs-netcat owns the relay connection. Install gs-netcat on the authorized lab host or set NW_GS_NETCAT to its path.</p>
        </div>
      </div>
    </div>
    <div class="payload-step" data-payload-step="2">
      <div class="payload-step-copy"><span>STEP 03</span><h3>Tune the beacon.</h3><p>Set a predictable lab cadence. Jitter adds a randomized delay around the base interval.</p></div>
      <div class="payload-fields payload-fields-grid">
        <div class="form-field"><label for="payload-interval">Beacon interval (ms)</label><input id="payload-interval" name="interval_ms" type="number" value="{f_interval}" min="10" placeholder="5000" required><p class="muted field-hint"><strong>Example:</strong> <code>5000</code> means a five-second base interval. Shorter values give faster feedback and generate more traffic.</p></div>
        <div class="form-field"><label for="payload-jitter">Jitter (ms)</label><input id="payload-jitter" name="jitter_ms" type="number" value="{f_jitter}" min="0" placeholder="1000" required><p class="muted field-hint"><strong>Example:</strong> <code>1000</code> adds up to ±1 second. Keep jitter below the base interval for predictable lab behavior.</p></div>
        <aside class="timing-note payload-field-full"><span class="status-dot"></span><div><strong>Runtime note</strong><p>TCP and gsocket profiles require the corresponding native listener. Build output does not create an automatic portal-to-native bridge.</p></div></aside>
      </div>
    </div>
    <div class="payload-step" data-payload-step="3">
      <div class="payload-step-copy"><span>STEP 04</span><h3>Review the build.</h3><p>Confirm the non-secret profile below. The shared secret is intentionally omitted from this summary.</p></div>
      <div class="payload-review" data-payload-review>
        <div><span>Identity</span><strong>{f_name}</strong></div>
        <div><span>Target</span><strong>{f_os} / {f_arch}</strong></div>
        <div><span>Callback</span><strong>{review_protocol} · {f_lhost}:{f_lport}</strong></div>
        <div><span>Timing</span><strong>{f_interval} ms · ±{f_jitter} ms</strong></div>
      </div>
    </div>
    <footer class="payload-wizard-actions"><button type="button" class="btn btn-ghost" data-wizard-back>Back</button><span>Need examples? Read the <a href="/guide#guide-payload">payload field guide</a>. Only build for systems you are authorized to test.</span><button type="button" data-wizard-next>Continue <span aria-hidden="true">→</span></button><button type="submit" data-wizard-build>{btn_label} <span aria-hidden="true">↗</span></button></footer>
  </form>
</section>
<section class="payload-library" aria-labelledby="payload-library-title"><div class="section-heading"><div><p class="eyebrow">ARTIFACT LIBRARY</p><h2 id="payload-library-title">Built payloads</h2></div><span>{build_count:02} FILES</span></div>{list}</section>"#,
        token = escape_html(csrf_token),
        f_name = escape_html(&f_name),
        f_lhost = escape_html(&f_lhost),
        f_lport = f_lport,
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
        review_protocol = escape_html(&f_protocol.to_uppercase()),
        payload_icon = nav_icon("payloads"),
        build_count = builds.len(),
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
                "<tr data-task-id=\"{}\" data-task-command=\"{}\"><td><code>{}</code></td><td>{}</td><td data-task-state>{}</td><td>{}</td><td data-task-output>{}</td></tr>",
                escape_html(&t.id),
                escape_html(&t.command),
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

    let persisted_output = {
        let mut entries = String::new();
        for task in tasks
            .iter()
            .rev()
            .filter(|task| matches!(task.status.as_str(), "completed" | "error"))
        {
            let output = task
                .result_output
                .as_deref()
                .and_then(|encoded| {
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .ok()
                })
                .map(|decoded| String::from_utf8_lossy(&decoded).into_owned())
                .unwrap_or_default();
            entries.push_str(&format!(
                "<div class=\"entry\" data-task-id=\"{}\"><div class=\"meta\">{} - {} at {}</div><div class=\"output\">{}</div></div>",
                escape_html(&task.id),
                escape_html(&task.command),
                escape_html(&task.status),
                escape_html(task.completed_at.as_deref().unwrap_or("unknown time")),
                if output.is_empty() {
                    "(no output)".to_owned()
                } else {
                    escape_html(&output)
                },
            ));
        }
        if entries.is_empty() {
            "<div class=\"results-placeholder\"><span aria-hidden=\"true\">▣</span><strong>No output yet</strong><p>Task output will appear here as responses arrive.</p></div>".to_owned()
        } else {
            entries
        }
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
        "<header class=\"callback-header\"><div class=\"callback-meta\"><p class=\"eyebrow\">INTERACT / {}</p><h2>{}</h2><p class=\"muted\">{}@{} &mdash; {} {}</p></div><div class=\"callback-header-actions\">{}<a class=\"btn btn-ghost\" href=\"/callbacks\">All callbacks</a></div></header>",
        escape_html(&callback.id[..callback.id.len().min(8)]),
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

    let session_context = format!(
        "<aside class=\"session-context panel\"><p class=\"eyebrow\">Session context</p><dl><div><dt>Host</dt><dd>{}</dd></div><div><dt>User</dt><dd>{}</dd></div><div><dt>Process</dt><dd>{}</dd></div><div><dt>Platform</dt><dd>{} / {}</dd></div><div><dt>Protocol</dt><dd>{}</dd></div><div><dt>Last check-in</dt><dd>{}</dd></div></dl><a href=\"/topology\">Locate in topology ↗</a></aside>",
        escape_html(&callback.host),
        escape_html(&callback.user_name),
        escape_html(&callback.process),
        escape_html(&callback.os),
        escape_html(&callback.arch),
        escape_html(&callback.protocol.to_uppercase()),
        escape_html(&callback.last_seen),
    );

    let online = callback.is_online(time::OffsetDateTime::now_utc());
    let process_capable = callback
        .capabilities_json
        .as_ref()
        .map(|capabilities| capabilities.process_browser)
        .unwrap_or(false);
    let file_capable = callback
        .capabilities_json
        .as_ref()
        .map(|capabilities| capabilities.file_browser)
        .unwrap_or(false);
    let transfer_capable = callback
        .capabilities_json
        .as_ref()
        .map(|capabilities| capabilities.file_transfer)
        .unwrap_or(false);
    let file_default_path = if callback.os.to_ascii_lowercase().contains("windows") {
        r"C:\"
    } else {
        "/"
    };
    let file_default_path_url = if file_default_path == "/" {
        "%2F"
    } else {
        "C%3A%5C"
    };
    let tasking_panel = format!(
        r##"<section class="callback-tasking panel" data-callback-tasking data-live-output
          data-tasks-endpoint="/api/callbacks/{callback_id}/tasks"
          data-events-endpoint="/api/callbacks/{callback_id}/events"
          data-csrf-token="{csrf}" data-online="{online}">
          <header class="callback-tasking-title"><div><p class="eyebrow">TASKING / PERSISTENT LEDGER</p><h3>Tasking</h3><p>Authoritative callback history, reconciled after every connection.</p></div><div><span class="connection-label">Connection</span><span class="stream-state" data-connection-state role="status">Connecting</span></div></header>
          <nav class="callback-tabs" aria-label="Callback workspace tabs"><a data-callback-tab="tasking" aria-current="page" href="?tab=tasking#tasking">Tasking</a><a data-callback-tab="processes" href="?tab=processes#processes">Processes</a><a data-callback-tab="files" href="?tab=files&amp;path={file_default_path_url}#files">Files</a><span>Metadata</span></nav>
          <p class="offline-queue" data-offline-queue hidden></p>
          <div class="task-filters">
            <label>Search task history<input type="search" data-task-search placeholder="Command, operator, or output"></label>
            <label>State<select data-task-state-filter><option value="">All states</option><option value="pending">Queued</option><option value="delivering">Delivering</option><option value="delivered">Delivered</option><option value="processing">Processing</option><option value="completed">Completed</option><option value="error">Error</option><option value="cancelled">Cancelled</option></select></label>
            <label>Errors<select data-task-error-filter><option value="">All output</option><option value="errors">Errors only</option></select></label>
          </div>
          <div class="callback-console-grid"><aside class="session-context-wrap">{session_context}</aside><div class="task-timeline"><div class="task-empty" data-task-empty><strong>No task history yet</strong><p>Submit a command from the dock below.</p></div><div class="task-card-list" data-task-list></div><button class="btn btn-ghost load-older" type="button" data-load-older hidden>Load older</button></div></div>
          <section id="processes" class="process-panel panel" data-process-panel data-processes-endpoint="/api/callbacks/{callback_id}/processes" data-process-capable="{process_capable}" data-process-online="{online}">
            <header class="process-panel-head"><div><p class="eyebrow">PROCESS CONTROL / AUDITED</p><h3>Processes</h3><p>Latest structured snapshot from this callback.</p></div><div class="process-controls"><span data-process-snapshot-age>Loading snapshot…</span><button type="button" class="btn btn-ghost" data-process-refresh>Refresh</button></div></header>
            <p class="process-message" data-process-message role="status" aria-live="polite"></p>
            <div class="process-toolbar"><label>Search processes<input type="search" data-process-search placeholder="PID, name, executable, or user"></label><a data-process-task-link href="#tasking" hidden>Open control task in Tasking</a></div>
            <div class="table-scroll"><table class="data-table process-table"><caption class="sr-only">Callback processes</caption><thead><tr><th><button type="button" data-process-sort="pid">PID</button></th><th><button type="button" data-process-sort="parent_pid">Parent PID</button></th><th><button type="button" data-process-sort="name">Name</button></th><th><button type="button" data-process-sort="executable">Executable</button></th><th><button type="button" data-process-sort="user">User</button></th><th><button type="button" data-process-sort="architecture">Architecture</button></th><th><button type="button" data-process-sort="cpu_percent">CPU</button></th><th><button type="button" data-process-sort="memory_bytes">Memory</button></th><th><button type="button" data-process-sort="started_at">Started</button></th><th>Action</th></tr></thead><tbody data-process-table-body></tbody></table></div>
            <p class="process-empty" data-process-empty hidden>No processes match this search or snapshot.</p>
            <aside class="process-detail" data-process-detail aria-live="polite"><p>Select a process to inspect its latest values.</p></aside>
            <div class="process-confirm" data-process-confirm hidden role="dialog" aria-modal="true" aria-labelledby="process-confirm-title"><h4 id="process-confirm-title">Confirm process termination</h4><p data-process-confirm-text></p><button type="button" class="btn btn-ghost" data-process-confirm-cancel>Cancel</button><button type="button" class="btn btn-danger" data-process-confirm-submit>Terminate exact PID</button></div>
          </section>
          <section id="files" class="file-panel panel" data-file-panel data-files-endpoint="/api/callbacks/{callback_id}/files" data-transfers-endpoint="/api/callbacks/{callback_id}/transfers" data-file-capable="{file_capable}" data-transfer-capable="{transfer_capable}" data-default-path="{file_default_path}">
            <header class="file-panel-head"><div><p class="eyebrow">FILESYSTEM CONTROL / AUDITED</p><h3>Files</h3><p>Latest structured directory snapshot from this callback.</p></div><div class="file-controls"><span data-file-snapshot-age>Loading snapshot…</span><button type="button" class="btn btn-ghost" data-file-refresh>Refresh</button></div></header>
            <p class="file-message" data-file-message role="status" aria-live="polite"></p>
            <form class="file-path-form" data-file-path-form><button type="button" class="btn btn-ghost" data-file-parent aria-label="Open parent directory">Parent</button><label class="sr-only" for="callback-file-path">Remote path</label><input id="callback-file-path" type="text" data-file-path autocomplete="off" required><button type="submit" class="btn btn-ghost">Open path</button></form>
            <nav class="file-breadcrumbs" data-file-breadcrumbs aria-label="Remote path breadcrumbs"></nav>
            <div class="file-toolbar"><form data-file-mkdir-form><label>New directory<input type="text" data-file-mkdir-name autocomplete="off" required></label><button type="submit" class="btn btn-ghost">Mkdir</button></form><a data-file-task-link href="#tasking" hidden>Open filesystem task in Tasking</a></div>
            <div class="file-transfer-controls" aria-label="File transfers"><form data-file-upload-form><label>Remote destination<input type="text" data-file-upload-destination autocomplete="off" {transfer_disabled}></label><label>Local file<input type="file" data-file-upload-input {transfer_disabled}></label><button type="submit" class="btn btn-ghost" data-file-upload {transfer_disabled}>{transfer_upload_label}</button></form><button type="button" class="btn btn-ghost" data-file-download {transfer_disabled}>{transfer_download_label}</button></div>
            <section class="file-transfer-list" data-file-transfer-list aria-label="Transfer progress" aria-live="polite"></section>
            <div class="table-scroll"><table class="data-table file-table"><caption class="sr-only">Callback filesystem</caption><thead><tr><th><button type="button" data-file-sort="name">Name</button></th><th><button type="button" data-file-sort="kind">Kind</button></th><th><button type="button" data-file-sort="size">Size</button></th><th><button type="button" data-file-sort="modified_at">Modified</button></th><th><button type="button" data-file-sort="permissions">Permissions</button></th><th><button type="button" data-file-sort="owner">Owner</button></th><th>Actions</th></tr></thead><tbody data-file-table-body></tbody></table></div>
            <p class="file-empty" data-file-empty hidden>No entries are available for this directory snapshot.</p>
            <div class="file-confirm" data-file-confirm hidden role="dialog" aria-modal="true" aria-labelledby="file-confirm-title"><h4 id="file-confirm-title">Confirm exact filesystem target</h4><p data-file-confirm-text></p><div data-file-move-fields hidden><label>Exact destination<input type="text" data-file-move-destination autocomplete="off"></label></div><div data-file-delete-fields hidden><label><input type="checkbox" data-file-delete-recursive> Delete directory contents recursively</label></div><button type="button" class="btn btn-ghost" data-file-confirm-cancel>Cancel</button><button type="button" class="btn btn-danger" data-file-confirm-submit>Queue exact action</button></div>
          </section>
          <noscript><section class="task-history-panel">{rows}</section><section id="task-results">{persisted_output}</section></noscript>
          <div class="command-dock panel"><div class="command-dock-label"><span aria-hidden="true">&gt;_</span><div><strong>Task this callback</strong><small>Use ↑ and ↓ for persisted command history</small></div></div>
            <form id="task-form" data-task-form><input type="hidden" name="csrf_token" value="{csrf}"><div class="command-input"><label class="sr-only" for="command">Command</label><span aria-hidden="true">$</span><input list="command-suggestions" type="text" id="command" data-command-input autocomplete="off" placeholder="Task an authorized lab agent…" required><datalist id="command-suggestions">{suggestions}</datalist></div><label class="sr-only" for="args">Arguments</label><input class="command-args" type="text" id="args" data-command-arguments placeholder="Arguments (optional)"><button type="submit" class="btn btn-primary">Execute <span aria-hidden="true">↗</span></button></form>
          </div>
        </section>"##,
        callback_id = escape_html(&callback.id),
        csrf = escape_html(csrf_token),
        online = online,
        process_capable = process_capable,
        file_capable = file_capable,
        transfer_capable = transfer_capable,
        transfer_disabled = if transfer_capable { "" } else { "disabled" },
        transfer_upload_label = if transfer_capable {
            "Upload file"
        } else {
            "Transfer support is being initialized"
        },
        transfer_download_label = if transfer_capable {
            "Download a listed file"
        } else {
            "Transfer support is being initialized"
        },
        file_default_path = escape_html(file_default_path),
        file_default_path_url = file_default_path_url,
        session_context = session_context,
        rows = rows,
        persisted_output = persisted_output,
        suggestions = suggestions_html,
    );

    let module_studio = format!(
        r##"<section class="module-studio panel" data-module-studio data-task-endpoint="/api/callbacks/{callback_id}/tasks" data-csrf-token="{csrf}">
        <header><div><p class="eyebrow">MODULE STUDIO / ASSESSMENT</p><h3>Turn a callback into a clear next step.</h3><p>Run structured discovery, review privilege-escalation candidates, or execute a small operator-authored script.</p></div><span class="module-platform">{os} / {arch}</span></header>
        <div class="module-grid">
          <article class="module-card"><span class="module-index">01</span><div><h4>User enumeration</h4><p>Inventory local accounts and login capability with a read-only collector.</p></div><button type="button" class="btn btn-primary" data-module-command="nw/user-enum" data-module-timeout="45000">Enumerate users</button></article>
          <article class="module-card module-card-featured"><span class="module-index">02</span><div><h4>PEASS assessment</h4><p>Fetch the official latest platform asset, record its SHA-256, and surface possible escalation paths.</p></div><button type="button" class="btn btn-primary" data-module-command="nw/peas-audit" data-module-timeout="300000">Run assessment</button></article>
          <article class="module-card module-custom"><span class="module-index">03</span><div><h4>Custom code</h4><p>Run up to 24 KiB through a selected interpreter with timeout and output limits.</p></div><button type="button" class="btn btn-ghost" data-code-toggle aria-expanded="false">Open editor</button></article>
        </div>
        <form class="custom-code-form" data-custom-code-form hidden>
          <div class="form-field"><label for="module-language">Interpreter</label><select id="module-language" name="language"><option value="shell">Shell</option><option value="python">Python 3</option><option value="powershell">PowerShell</option></select></div>
          <div class="form-field"><label for="module-timeout">Timeout</label><select id="module-timeout" name="timeout"><option value="30000">30 seconds</option><option value="60000" selected>60 seconds</option><option value="120000">2 minutes</option></select></div>
          <div class="form-field module-code-field"><label for="module-source">Source</label><textarea id="module-source" name="source" maxlength="24576" rows="9" spellcheck="false" placeholder="# Authorized lab code…" required></textarea><small><span data-source-count>0</span> / 24,576 characters</small></div>
          <footer><p><strong>Assessment boundary.</strong> Automatic exploitation stays disabled. Review findings before taking any privilege-changing action.</p><button type="submit" class="btn btn-primary">Run custom code <span aria-hidden="true">↗</span></button></footer>
        </form>
        <p class="module-status" data-module-status role="status" aria-live="polite"></p>
      </section>"##,
        callback_id = escape_html(&callback.id),
        csrf = escape_html(csrf_token),
        os = escape_html(&callback.os.to_uppercase()),
        arch = escape_html(&callback.arch.to_uppercase()),
    );

    let content = format!(
        "<div class=\"callback-detail-shell\">{callback_detail}{tasking_panel}{module_studio}</div>"
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
    let count = |status| callbacks.iter().filter(|c| c.status == status).count();
    let active = count(CallbackStatus::Active);
    let beacon = count(CallbackStatus::Beacon);
    let dormant = count(CallbackStatus::Dormant);
    let lost = count(CallbackStatus::Lost);
    let rows = callbacks
        .iter()
        .map(|c| {
            let (status_label, status_class) = callback_status(c.status);
            let status = status_pill(status_label, status_class);
            format!(
                "<tr class=\"callback-row status-{status_class}\"><td><span class=\"callback-host\"><i aria-hidden=\"true\"></i><strong>{}</strong></span></td><td>{}</td><td>{}</td><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td><a href=\"/callbacks/{}\" class=\"btn btn-sm btn-ghost\">Interact &rarr;</a></td></tr>",
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
        "<section class=\"empty-state panel callback-empty\"><span class=\"empty-callback-icon\" aria-hidden=\"true\">ϟ</span><h2>No callbacks in view.</h2><p>Create a compatible native payload and run it on an authorized lab host to see sessions here.</p><a class=\"button\" href=\"/payloads\">Create payload ↗</a></section>".to_owned()
    } else {
        format!(
            "<div class=\"callback-table-shell panel\"><div class=\"table-scroll\"><table class=\"data-table callbacks-table\"><caption>Active callbacks</caption><thead><tr><th scope=\"col\">Host</th><th scope=\"col\">User</th><th scope=\"col\">Process</th><th scope=\"col\">OS / Arch</th><th scope=\"col\">Protocol</th><th scope=\"col\">State</th><th scope=\"col\">Last check-in</th><th scope=\"col\">Operation</th><th scope=\"col\">First seen</th><th scope=\"col\"><span class=\"sr-only\">Action</span></th></tr></thead><tbody>{rows}</tbody></table></div></div>"
        )
    };
    app_page(
        "Active Callbacks",
        user,
        "callbacks",
        &format!(
            "<section class=\"callback-workspace\" data-callback-workspace><header class=\"callback-workspace-head\"><div><p class=\"eyebrow\">SESSION BOARD / LIVE INVENTORY</p><h2>Every callback.<br><em>Ready to inspect.</em></h2><p>Registered sessions, their current state, and the operation context behind each connection.</p></div><div class=\"callback-workspace-actions\"><a class=\"btn btn-ghost\" href=\"/topology\">Open topology</a><a class=\"button\" href=\"/payloads\">Create payload <span aria-hidden=\"true\">↗</span></a></div></header><div class=\"callback-summary\" data-callback-summary><article><span>Total sessions</span><strong>{total:02}</strong><small>Registered callbacks</small></article><article class=\"summary-active\"><span>Active</span><strong>{active:02}</strong><small>Connected now</small></article><article><span>Beaconing</span><strong>{beacon:02}</strong><small>Periodic check-in</small></article><article><span>Dormant / lost</span><strong>{inactive:02}</strong><small>Needs attention</small></article></div>{table}</section>",
            total = callbacks.len(),
            inactive = dormant + lost,
        ),
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
        "topology" => "Every asset. Every callback. One connected field of view.",
        "recon" => "Turn scoped observations into a clearer picture.",
        "dashboard" => "A little perspective. Every operation, in one place.",
        "operations" => "Clear scope. Deliberate work. A place for every operation.",
        "inventory" => "The assets that make up your field of view.",
        "checks" => "A traceable record of what you have observed.",
        "evidence" => "The details that turn observations into findings.",
        "reports" => "Bring the complete story into view.",
        "audit" => "Every action leaves a record. Follow it here.",
        "admin" => "The people and permissions behind your workspace.",
        "callbacks" => "Your connections, organized and in context.",
        "payloads" => "Build a native field kit with every choice in view.",
        "guide" => "A practical path from first setup to a documented result.",
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
        "topology" => {
            r#"<circle cx="12" cy="5" r="3"/><circle cx="5" cy="19" r="3"/><circle cx="19" cy="19" r="3"/><path d="m10 8-4 8m8-8 4 8M8 19h8"/>"#
        }
        "recon" => r#"<circle cx="11" cy="11" r="8"/><path d="m17 17 4 4M11 7v8M7 11h8"/>"#,
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
        "payloads" => {
            r#"<path d="m10.5 4.5 7.5 4.2v8l-7.5 4.3L3 16.7v-8Zm0 8.5L18 8.7M10.5 13 3 8.7m7.5 4.3v8M18.5 2v5M16 4.5h5"/>"#
        }
        "admin" => r#"<path d="m12 2 8 4v6c0 6-8 10-8 10S4 18 4 12V6Z"/><path d="m8 12 3 3 5-6"/>"#,
        "search" => r#"<circle cx="10.5" cy="10.5" r="7.5"/><path d="m16 16 5 5"/>"#,
        "guide" => {
            r#"<path d="M4 4.5A3.5 3.5 0 0 1 7.5 1H12v19H7.5A3.5 3.5 0 0 0 4 23.5Zm16 0A3.5 3.5 0 0 0 16.5 1H12v19h4.5a3.5 3.5 0 0 1 3.5 3.5Z"/>"#
        }
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

const MOTION_ASSETS: &str = r#"<script defer src="/static/anime.min.js"></script><script defer src="/static/motion.js"></script>"#;
const APP_ASSETS: &str = r#"<link rel="stylesheet" href="/static/workspace.css"><link rel="stylesheet" href="/static/callback-workspace.css"><script defer src="/static/anime.min.js"></script><script defer src="/static/motion.js"></script><script defer src="/static/workspace.js"></script><script defer src="/static/payload-wizard.js"></script><script defer src="/static/module_studio.js"></script><script defer src="/static/callback-workspace.js"></script><script defer src="/static/admin.js"></script>"#;

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

pub(super) fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
