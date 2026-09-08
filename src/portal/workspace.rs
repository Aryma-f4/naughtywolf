use super::{
    csrf_token_matches, issue_csrf_token,
    templates::{self, escape_html as esc},
};
use crate::{
    AppError,
    auth::{AuthenticatedUser, middleware::AuthenticatedUserGuard, rbac::Role},
    checks::{CheckInput, Runner, SurfaceReconInput},
    db::{
        models::{Asset, Callback, CheckRun, Operation},
        repositories::Repository,
    },
    policy::authorize_asset_run,
};
use axum::{
    Form, Json,
    extract::{FromRequest, Query, Request, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};
use tokio::sync::Semaphore;
use tower_sessions::Session;

#[derive(Serialize)]
pub struct GraphNode {
    id: String,
    kind: &'static str,
    label: String,
    detail: String,
    operation: String,
    status: String,
    href: String,
    facts: Vec<(String, String)>,
}
#[derive(Serialize)]
pub struct GraphEdge {
    source: String,
    target: String,
    label: &'static str,
}
#[derive(Serialize)]
pub struct TopologyData {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

fn node(
    id: String,
    kind: &'static str,
    label: String,
    detail: String,
    operation: String,
    status: String,
    href: String,
) -> GraphNode {
    GraphNode {
        id,
        kind,
        label,
        detail,
        operation,
        status,
        href,
        facts: Vec::new(),
    }
}
fn status<T: std::fmt::Debug>(value: T) -> String {
    format!("{value:?}").to_lowercase()
}

// Relationships come only from persisted foreign keys and saved observations.
fn topology(
    operations: &[Operation],
    assets: &[Asset],
    callbacks: &[Callback],
    runs: &[CheckRun],
) -> TopologyData {
    let mut data = TopologyData {
        nodes: vec![],
        edges: vec![],
    };
    let operation_ids: HashSet<_> = operations.iter().map(|op| op.id.as_str()).collect();
    let asset_map: HashMap<_, _> = assets.iter().map(|a| (a.id.as_str(), a)).collect();
    for op in operations {
        data.nodes.push(node(
            format!("operation:{}", op.id),
            "operation",
            op.name.clone(),
            op.purpose.clone(),
            op.id.clone(),
            status(op.status),
            format!("/operations/{}", op.id),
        ));
    }
    for asset in assets {
        let id = format!("asset:{}", asset.id);
        let mut n = node(
            id.clone(),
            "asset",
            asset.name.clone(),
            asset.address.clone(),
            asset.operation_id.clone(),
            status(asset.status),
            format!("/recon?asset={}", asset.id),
        );
        n.facts = vec![
            ("Address".into(), asset.address.clone()),
            ("Type".into(), asset.kind.clone()),
            ("Owner".into(), asset.owner.clone()),
        ];
        data.nodes.push(n);
        if operation_ids.contains(asset.operation_id.as_str()) {
            data.edges.push(GraphEdge {
                source: format!("operation:{}", asset.operation_id),
                target: id,
                label: "contains",
            });
        }
    }
    for callback in callbacks {
        let id = format!("callback:{}", callback.id);
        let operation = callback.operation_id.clone().unwrap_or_default();
        let mut n = node(
            id.clone(),
            "callback",
            callback.host.clone(),
            format!("{} · {}", callback.os, callback.protocol),
            operation.clone(),
            status(callback.status),
            format!("/callbacks/{}", callback.id),
        );
        n.facts = vec![
            ("User".into(), callback.user_name.clone()),
            ("Process".into(), callback.process.clone()),
            (
                "Platform".into(),
                format!("{} / {}", callback.os, callback.arch),
            ),
            ("Protocol".into(), callback.protocol.clone()),
            ("Last seen".into(), callback.last_seen.clone()),
        ];
        data.nodes.push(n);
        let parent_asset = callback
            .asset_id
            .as_deref()
            .and_then(|id| asset_map.get(id))
            .filter(|a| a.operation_id == operation);
        let parent = parent_asset.map(|a| format!("asset:{}", a.id)).or_else(|| {
            operation_ids
                .contains(operation.as_str())
                .then(|| format!("operation:{operation}"))
        });
        if let Some(source) = parent {
            data.edges.push(GraphEdge {
                source,
                target: id,
                label: "registered callback",
            });
        }
    }
    let mut reviewed = HashSet::new();
    for run in runs.iter().filter(|r| r.check_id == "surface-recon") {
        let Some(asset) = asset_map.get(run.asset_id.as_str()) else {
            continue;
        };
        if !reviewed.insert(&run.asset_id) {
            continue;
        }
        if let Some(findings) = run
            .result_json
            .as_ref()
            .and_then(|v| v["findings"].as_array())
        {
            for (i, finding) in findings
                .iter()
                .filter(|f| f["code"] == "dns.address")
                .take(16)
                .enumerate()
            {
                let Some(ip) = finding["message"]
                    .as_str()
                    .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
                else {
                    continue;
                };
                let id = format!("address:{}:{i}", run.id);
                let mut n = node(
                    id.clone(),
                    "address",
                    ip.to_owned(),
                    "Saved DNS observation".into(),
                    asset.operation_id.clone(),
                    "observed".into(),
                    format!("/recon?asset={}", asset.id),
                );
                n.facts = vec![
                    (
                        "Observed at".into(),
                        run.finished_at
                            .clone()
                            .unwrap_or_else(|| run.created_at.clone()),
                    ),
                    ("Source asset".into(), asset.name.clone()),
                ];
                data.nodes.push(n);
                data.edges.push(GraphEdge {
                    source: format!("asset:{}", asset.id),
                    target: id,
                    label: "resolved to",
                });
            }
        }
    }
    data
}

async fn records(
    repo: &Repository,
    user: &AuthenticatedUser,
) -> Result<(Vec<Operation>, Vec<Asset>, Vec<CheckRun>), AppError> {
    let admin = user.role == Role::Admin;
    let (ops, assets, runs) = tokio::try_join!(
        repo.list_operations_visible_to(&user.id, admin),
        repo.list_assets_visible_to(&user.id, admin),
        repo.list_check_runs_visible_to(&user.id, admin)
    )?;
    Ok((ops, assets, runs))
}

pub async fn topology_data(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repo): State<Repository>,
) -> Result<Json<TopologyData>, AppError> {
    let (ops, assets, runs) = records(&repo, &user).await?;
    let callbacks = if user.role.allows(Role::Operator) {
        repo.list_callbacks_visible_to(&user.id, user.role == Role::Admin)
            .await?
    } else {
        vec![]
    };
    Ok(Json(topology(&ops, &assets, &callbacks, &runs)))
}

pub async fn topology_page(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repo): State<Repository>,
) -> Result<Html<String>, AppError> {
    let (_, assets, _) = records(&repo, &user).await?;
    let fallback = if assets.is_empty() {
        "<p>No assets yet. Add an asset to an operation, or explore the labelled sample graph.</p>"
            .into()
    } else {
        format!(
            "<ul>{}</ul>",
            assets
                .iter()
                .take(50)
                .map(|a| format!("<li>{} — {}</li>", esc(&a.name), esc(&a.address)))
                .collect::<String>()
        )
    };
    let body = format!(
        r#"<section class="topology-workspace" data-topology>
<div class="topology-toolbar"><div class="view-switch" aria-label="Visualization view"><button type="button" data-view="graph" aria-pressed="true">Graph</button><button type="button" data-view="list" aria-pressed="false">List</button></div><label class="sr-only" for="graph-search">Search nodes</label><input id="graph-search" type="search" placeholder="Find a host, address, or operation…" data-graph-search><label class="sr-only" for="graph-operation">Operation</label><select id="graph-operation" data-graph-operation><option value="">All operations</option></select><label class="sr-only" for="graph-status">Node status</label><select id="graph-status" data-graph-status><option value="">All statuses</option><option value="active">Active</option><option value="beacon">Beacon</option><option value="dormant">Dormant</option><option value="lost">Lost</option><option value="observed">Observed</option></select><button type="button" class="btn btn-ghost" data-graph-refresh>Refresh</button></div>
<div class="graph-summary"><span><i class="legend-dot operation-dot"></i><b data-count="operation">0</b> operations</span><span><i class="legend-dot asset-dot"></i><b data-count="asset">0</b> assets</span><span><i class="legend-dot callback-dot"></i><b data-count="callback">0</b> callbacks</span><span><i class="legend-dot address-dot"></i><b data-count="address">0</b> addresses</span><span class="graph-snapshot" data-graph-snapshot>Loading snapshot…</span></div>
<div class="graph-sample-banner" data-sample-banner hidden>Sample data · illustrative relationships only <button type="button" data-graph-live>Return to workspace</button></div>
<div class="graph-layout"><div class="graph-canvas-wrap"><div class="graph-canvas" data-graph-canvas tabindex="0" aria-label="Relationship graph. Drag to pan; use the zoom buttons or arrow keys. Select a node for details."><svg data-graph-svg viewBox="0 0 1000 680" aria-label="Operation, asset and callback relationships"><g data-graph-layer></g></svg></div><div class="graph-list" data-graph-list hidden></div><div class="graph-empty" data-graph-empty hidden><span class="empty-orbit" aria-hidden="true">◎</span><h2>Your field of view starts here.</h2><p>Add scoped assets to build your map, or explore a sample with multiple callbacks.</p><button type="button" data-graph-demo>Explore sample graph</button><a href="/operations">Open operations ↗</a></div><div class="graph-controls"><button type="button" data-zoom="in" aria-label="Zoom in">+</button><button type="button" data-zoom="out" aria-label="Zoom out">−</button><button type="button" data-zoom="fit">Fit</button><span data-graph-visible role="status" aria-live="polite"></span></div></div><aside class="node-inspector" data-node-inspector aria-label="Selected node details"><p class="eyebrow">NODE INSPECTOR</p><div data-node-detail><span class="inspector-symbol" aria-hidden="true">⌖</span><h2>Every connection<br>has context.</h2><p>Select a node to see its identity, status, and recorded relationships.</p></div><div class="inspector-footnote">Lines represent saved associations and DNS observations. They do not imply a network route or pivot.</div></aside></div>
<div class="graph-bottom"><span>Drag to pan · + / − to zoom · 0 to fit</span><a href="/recon">Open recon workspace ↗</a></div><details class="graph-fallback"><summary>Asset inventory without visualization</summary>{fallback}</details><noscript><p>Enable JavaScript for the interactive graph. The asset inventory above remains available.</p></noscript></section>"#
    );
    Ok(Html(templates::app_page(
        "Topology", &user, "topology", &body,
    )))
}

#[derive(Default, Deserialize)]
pub struct ReconQuery {
    asset: Option<String>,
}

pub async fn recon_page(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repo): State<Repository>,
    session: Session,
    Query(query): Query<ReconQuery>,
) -> Result<Html<String>, AppError> {
    recon_html(&repo, &user, &session, query.asset.as_deref(), None)
        .await
        .map(Html)
}

async fn recon_html(
    repo: &Repository,
    user: &AuthenticatedUser,
    session: &Session,
    selected: Option<&str>,
    error: Option<&str>,
) -> Result<String, AppError> {
    let (ops, assets, runs) = records(repo, user).await?;
    let selected = selected.filter(|id| !id.is_empty());
    if selected.is_some_and(|id| !assets.iter().any(|a| a.id == id)) {
        return Err(AppError::NotFound);
    }
    let selected = selected.unwrap_or("");
    let can_run = user.role.allows(Role::Operator);
    let csrf = issue_csrf_token(session).await?;
    let options = assets
        .iter()
        .map(|a| {
            format!(
                r#"<option value="{}"{}>{} · {}</option>"#,
                esc(&a.id),
                if a.id == selected { " selected" } else { "" },
                esc(&a.name),
                esc(&a.address)
            )
        })
        .collect::<String>();
    let history: Vec<_> = runs
        .iter()
        .filter(|r| {
            r.check_id == "surface-recon" && (selected.is_empty() || r.asset_id == selected)
        })
        .collect();
    let observed: HashSet<_> = history
        .iter()
        .filter_map(|r| r.result_json.as_ref())
        .filter_map(|r| r["findings"].as_array())
        .flatten()
        .filter(|f| f["code"] == "dns.address")
        .filter_map(|f| f["message"].as_str())
        .collect();
    let cards = history.iter().take(20).map(|run| {
        let asset = assets.iter().find(|a| a.id == run.asset_id).map(|a| a.name.as_str()).unwrap_or("Asset");
        let findings = run.result_json.as_ref().and_then(|r| r["findings"].as_array()).map(|findings| findings.iter().take(40).map(|f| format!("<div><dt>{}</dt><dd>{}</dd></div>", esc(f["code"].as_str().unwrap_or("observation")), esc(f["message"].as_str().unwrap_or("")))).collect::<String>()).unwrap_or_default();
        let failure = run.error_summary.as_deref().map(|msg| format!("<p class=\"form-error\">{}</p>", esc(msg))).unwrap_or_default();
        format!(r#"<article class="recon-result"><header><div><span class="eyebrow">{}</span><h3>{}</h3></div><span class="status-pill status-{}">{}</span></header>{failure}<dl class="recon-findings">{findings}</dl><footer><span>Run {}</span><a href="/topology">View map ↗</a></footer></article>"#, esc(&run.created_at), esc(asset), if run.state == crate::db::models::RunState::Succeeded { "success" } else { "warning" }, status(run.state), esc(&run.id))
    }).collect::<String>();
    let history_empty = if history.is_empty() {
        "<div class=\"recon-empty\"><span aria-hidden=\"true\">⌕</span><h3>Turn an address into context.</h3><p>Run a scoped check to collect DNS and HTTP observations. Saved findings will appear here and enrich the topology.</p></div>"
    } else {
        ""
    };
    let error = error
        .map(|e| format!("<p class=\"form-error\" role=\"alert\">{}</p>", esc(e)))
        .unwrap_or_default();
    let no_assets = if assets.is_empty() {
        "<p class=\"recon-scope-note\">No assets yet. <a href=\"/operations\">Add an asset to an operation ↗</a></p>"
    } else {
        ""
    };
    let disabled = if !can_run || assets.is_empty() {
        " disabled"
    } else {
        ""
    };
    let permission = if can_run {
        "Runs require an active asset in an active operation you can access."
    } else {
        "Viewer access: inspect saved observations. An Operator or Admin can run recon."
    };
    let op_links = ops
        .iter()
        .filter(|op| op.status != crate::db::models::OperationStatus::Active)
        .take(5)
        .map(|op| {
            format!(
                "<a href=\"/operations/{}\">Review {} scope ↗</a>",
                esc(&op.id),
                esc(&op.name)
            )
        })
        .collect::<String>();
    let body = format!(
        r#"<section class="recon-workspace" data-recon><div class="recon-heading"><div><p class="eyebrow">OBSERVE / ENRICH / CONNECT</p><h2>Give every target<br><em>a little more context.</em></h2><p>Collect bounded observations from a selected asset and bring the results into your map.</p></div><a class="button" href="/topology">Open topology ↗</a></div><div class="recon-stats"><span><b>{}</b> scoped assets</span><span><b>{}</b> recorded runs</span><span><b>{}</b> observed addresses</span></div><div class="recon-columns"><section class="recon-launch panel"><span class="eyebrow">NEW OBSERVATION</span><h2>Start with a target.</h2>{error}{no_assets}<form method="post" action="/recon/run" data-recon-form><input type="hidden" name="csrf_token" value="{}"><div class="form-field"><label for="recon-asset">Scoped asset</label><select id="recon-asset" name="asset_id" required{disabled}><option value="">Choose an asset</option>{options}</select></div><div class="form-field"><label for="recon-mode">Observation profile</label><select id="recon-mode" name="mode"{disabled}><option value="dns">DNS · resolved addresses</option><option value="http">DNS + HTTP / TLS</option></select></div><p class="field-hint">HTTP mode sends one HEAD request to the asset origin. HTTPS validates the certificate and hostname. Redirects are recorded without following them.</p><button type="submit" class="primary-action"{disabled}>Run reconnaissance <span aria-hidden="true">↗</span></button><p data-recon-progress role="status" aria-live="polite"></p><p class="recon-scope-note">{permission}</p></form><div class="recon-scope-links">{op_links}</div><form class="recon-history-filter" method="get" action="/recon"><label for="recon-history-asset">Filter saved results</label><select id="recon-history-asset" name="asset"><option value="">All visible assets</option>{options}</select><button type="submit" class="btn btn-ghost">View history</button></form></section><section class="recon-history"><div class="section-heading"><h2>Observation history</h2><span>LATEST 20 RUNS</span></div>{history_empty}{cards}</section></div></section>"#,
        assets.len(),
        history.len(),
        observed.len(),
        esc(&csrf)
    );
    Ok(templates::app_page("Reconnaissance", user, "recon", &body))
}

#[derive(Deserialize)]
struct ReconForm {
    asset_id: String,
    mode: String,
    csrf_token: Option<String>,
}
static RECON_SLOTS: OnceLock<Semaphore> = OnceLock::new();

pub async fn run_recon(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repo): State<Repository>,
    session: Session,
    request: Request,
) -> Result<Response, AppError> {
    user.require(Role::Operator)?;
    let Form(form) = Form::<ReconForm>::from_request(request, &repo)
        .await
        .map_err(|_| AppError::Validation("Invalid form".into()))?;
    if !csrf_token_matches(&session, form.csrf_token.as_deref()).await?
        || !matches!(form.mode.as_str(), "dns" | "http")
    {
        return Err(AppError::Validation("Invalid request".into()));
    }
    authorize_asset_run(&repo, &user, &form.asset_id).await?;
    let asset = repo
        .find_asset(&form.asset_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if let Err(error) = crate::checks::recon::target_url(&asset.address) {
        return Ok((
            StatusCode::BAD_REQUEST,
            Html(recon_html(&repo, &user, &session, Some(&asset.id), Some(error)).await?),
        )
            .into_response());
    }
    let _permit = RECON_SLOTS
        .get_or_init(|| Semaphore::new(4))
        .try_acquire()
        .map_err(|_| AppError::Conflict("Recon capacity reached".into()))?;
    let operation = asset.operation_id.clone();
    let correlation = uuid::Uuid::new_v4().to_string();
    audit(
        &repo,
        &user,
        &operation,
        &asset.id,
        "recon.requested",
        ("success", format!("profile={}", form.mode)),
        &correlation,
    )
    .await?;
    let result = Runner::new(repo.clone(), user.clone())
        .start(
            asset,
            "surface-recon",
            CheckInput::SurfaceRecon(SurfaceReconInput {
                probe_http: form.mode == "http",
            }),
        )
        .await?;
    audit(
        &repo,
        &user,
        &operation,
        &form.asset_id,
        "recon.finished",
        (
            if result.state == crate::db::models::RunState::Succeeded {
                "success"
            } else {
                "failure"
            },
            format!(
                "run_id={}; state={}",
                result.run_id.as_deref().unwrap_or("unavailable"),
                status(result.state)
            ),
        ),
        &correlation,
    )
    .await?;
    Ok(Redirect::to(&format!("/recon?asset={}", form.asset_id)).into_response())
}

async fn audit(
    repo: &Repository,
    user: &AuthenticatedUser,
    operation: &str,
    asset: &str,
    action: &str,
    outcome: (&str, String),
    correlation: &str,
) -> Result<(), AppError> {
    sqlx::query("INSERT INTO audit_events (id, actor_id, operation_id, action, target_type, target_id, outcome, parameter_summary, correlation_id) VALUES (?, ?, ?, ?, 'asset', ?, ?, ?, ?)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(&user.id).bind(operation).bind(action).bind(asset).bind(outcome.0).bind(outcome.1).bind(correlation).execute(&repo.pool).await.map_err(|_| AppError::Internal)?;
    Ok(())
}
