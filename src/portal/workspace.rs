use super::{
    csrf_token_matches, issue_csrf_token,
    templates::{self, escape_html as esc},
};
use crate::{
    AppError,
    auth::{AuthenticatedUser, middleware::AuthenticatedUserGuard, rbac::Role},
    checks::{CheckInput, DirectoryEnumInput, ObjectClass, Runner, SurfaceReconInput},
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
    /// Seconds since the last check-in, so a node that looks alive can be aged.
    #[serde(skip_serializing_if = "Option::is_none")]
    age_seconds: Option<i64>,
    /// True when the callback holds an open socket, so actions on it are instant.
    #[serde(default)]
    instant: bool,
}
#[derive(Serialize)]
pub struct GraphEdge {
    source: String,
    target: String,
    label: &'static str,
}
/// What the operator is looking at, in one line, before reading any node.
#[derive(Serialize, Default)]
pub struct TopologyCounts {
    pub operations: usize,
    pub assets: usize,
    pub callbacks: usize,
    pub active: usize,
    /// Callbacks reachable without waiting for a beacon.
    pub instant: usize,
    /// Tasks queued or running on a callback right now.
    pub tasks_in_flight: usize,
    /// Shells attached to a callback right now.
    pub shells: usize,
}
#[derive(Serialize)]
pub struct TopologyData {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    counts: TopologyCounts,
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
        age_seconds: None,
        instant: false,
    }
}
fn status<T: std::fmt::Debug>(value: T) -> String {
    format!("{value:?}").to_lowercase()
}

/// Live/dormant label for a callback node, independent of the stored status
/// column that no sweeper ever demotes.
fn live_state(callback: &Callback, now: time::OffsetDateTime) -> String {
    if callback.is_online(now) {
        "active".to_owned()
    } else {
        "lost".to_owned()
    }
}

/// How long ago a callback last checked in, from the recorded timestamp.
fn check_in_age(last_seen: &str, now: time::OffsetDateTime) -> Option<i64> {
    time::OffsetDateTime::parse(last_seen, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|seen| (now - seen).whole_seconds())
}

/// What is happening on callbacks right now, gathered once for the whole graph.
///
/// A graph of registered hosts is inventory; these three numbers are what make
/// it operational — which hosts answer instantly, which are already working on
/// something, and which have a shell an operator can type into.
#[derive(Default)]
pub struct CallbackActivity {
    /// Callbacks with an open live socket.
    pub live: HashSet<String>,
    /// Tasks queued or running, per session.
    pub tasks: HashMap<String, usize>,
    /// Attached shells, per session.
    pub shells: HashMap<String, usize>,
}

// Relationships come only from persisted foreign keys and saved observations.
fn topology(
    operations: &[Operation],
    assets: &[Asset],
    callbacks: &[Callback],
    runs: &[CheckRun],
    activity: &CallbackActivity,
) -> TopologyData {
    let mut data = TopologyData {
        nodes: vec![],
        edges: vec![],
        counts: TopologyCounts::default(),
    };
    // `callbacks.status` is only ever written as "active" on check-in, so a
    // stored status never reports a host that went silent. Derive liveness
    // instead, using the same window as the callbacks board, so the graph
    // cannot show a three-day-old beacon as live.
    let now = time::OffsetDateTime::now_utc();
    let operation_ids: HashSet<_> = operations.iter().map(|op| op.id.as_str()).collect();
    let asset_map: HashMap<_, _> = assets.iter().map(|a| (a.id.as_str(), a)).collect();
    data.counts.operations = operations.len();
    data.counts.assets = assets.len();
    data.counts.callbacks = callbacks.len();
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
        let online = callback.is_online(now);
        let mut n = node(
            id.clone(),
            "callback",
            callback.host.clone(),
            format!("{} · {}", callback.os, callback.protocol),
            operation.clone(),
            live_state(callback, now),
            format!("/callbacks/{}", callback.id),
        );
        if online {
            data.counts.active += 1;
        }
        let tasks = activity
            .tasks
            .get(callback.id.as_str())
            .copied()
            .unwrap_or(0);
        let shells = activity
            .shells
            .get(callback.id.as_str())
            .copied()
            .unwrap_or(0);
        let instant = activity.live.contains(callback.id.as_str());
        if instant {
            data.counts.instant += 1;
        }
        data.counts.tasks_in_flight += tasks;
        data.counts.shells += shells;
        // What an operator needs when deciding where to act: how old this is,
        // whether it answers instantly, and whether something is already
        // running on it.
        n.age_seconds = check_in_age(&callback.last_seen, now);
        n.instant = instant;
        n.facts = vec![
            ("User".into(), callback.user_name.clone()),
            ("Process".into(), callback.process.clone()),
            (
                "Platform".into(),
                format!("{} / {}", callback.os, callback.arch),
            ),
            ("Protocol".into(), callback.protocol.clone()),
            (
                "Channel".into(),
                if instant {
                    "Instant (open socket)".to_owned()
                } else {
                    format!(
                        "Beacon every ~{}s",
                        callback.interval_ms.unwrap_or(0) / 1000
                    )
                },
            ),
            ("Tasks in flight".into(), tasks.to_string()),
            ("Shells attached".into(), shells.to_string()),
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
                target: id.clone(),
                label: "registered callback",
            });
        }
        // The address the callback reported is what the operator will actually
        // reach, so it belongs in the graph next to the host that reported it.
        if let Some(address) = callback
            .local_addr
            .as_deref()
            .map(str::trim)
            .filter(|address| !address.is_empty() && address.parse::<std::net::IpAddr>().is_ok())
        {
            let address_id = format!("address:{address}");
            if !data.nodes.iter().any(|existing| existing.id == address_id) {
                let mut address_node = node(
                    address_id.clone(),
                    "address",
                    address.to_owned(),
                    "Reported by a callback".into(),
                    operation.clone(),
                    if online {
                        "active".into()
                    } else {
                        "lost".to_owned()
                    },
                    format!("/callbacks/{}", callback.id),
                );
                address_node.facts = vec![
                    ("Source".into(), callback.host.clone()),
                    ("User".into(), callback.user_name.clone()),
                ];
                data.nodes.push(address_node);
            }
            data.edges.push(GraphEdge {
                source: id,
                target: address_id,
                label: "reports",
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
    live: Option<axum::Extension<crate::c2_live::LiveRegistry>>,
    hub: Option<axum::Extension<crate::shell_hub::ShellHub>>,
) -> Result<Json<TopologyData>, AppError> {
    let (ops, assets, runs) = records(&repo, &user).await?;
    let callbacks = if user.role.allows(Role::Operator) {
        repo.list_callbacks_visible_to(&user.id, user.role == Role::Admin)
            .await?
    } else {
        vec![]
    };
    let activity = activity(&repo, &callbacks, live, hub).await;
    Ok(Json(topology(&ops, &assets, &callbacks, &runs, &activity)))
}

/// Gather what is happening on the visible callbacks right now.
///
/// One query for task counts and two in-memory lookups: a graph that only shows
/// registered hosts is inventory, and this is what makes it operational.
async fn activity(
    repo: &Repository,
    callbacks: &[Callback],
    live: Option<axum::Extension<crate::c2_live::LiveRegistry>>,
    hub: Option<axum::Extension<crate::shell_hub::ShellHub>>,
) -> CallbackActivity {
    let mut activity = CallbackActivity {
        live: live
            .map(|live| {
                live.live_sessions()
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect()
            })
            .unwrap_or_default(),
        ..CallbackActivity::default()
    };
    for callback in callbacks {
        let shells = hub
            .as_ref()
            .map(|hub| {
                hub.shells(&callback.id)
                    .into_iter()
                    .filter(|shell| shell.state != "closed")
                    .count()
            })
            .unwrap_or(0);
        if shells > 0 {
            activity.shells.insert(callback.id.clone(), shells);
        }
    }
    activity.tasks = repo.tasks_in_flight_by_session().await.unwrap_or_default();
    activity
}

pub async fn topology_page(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repo): State<Repository>,
) -> Result<Html<String>, AppError> {
    let (_, assets, _) = records(&repo, &user).await?;
    let fallback = if assets.is_empty() {
        "<p>No assets yet. Recorded operation and callback relationships appear in the live graph.</p>"
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
<div class="topology-toolbar"><div class="view-switch" aria-label="Visualization view"><button type="button" data-view="graph" aria-pressed="true">Graph</button><button type="button" data-view="list" aria-pressed="false">List</button></div><label class="sr-only" for="graph-search">Search nodes</label><input id="graph-search" type="search" placeholder="Find a host, address, or operation…" data-graph-search><label class="sr-only" for="graph-operation">Operation</label><select id="graph-operation" data-graph-operation><option value="">All operations</option></select><label class="sr-only" for="graph-status">Node status</label><select id="graph-status" data-graph-status><option value="">All statuses</option><option value="active">Active</option><option value="instant">Instant channel</option><option value="busy">Working</option><option value="lost">Lost</option><option value="observed">Observed</option></select><button type="button" class="btn btn-ghost" data-graph-refresh>Refresh</button></div>
<div class="graph-summary"><span><i class="legend-dot operation-dot"></i><b data-count="operation">0</b> operations</span><span><i class="legend-dot asset-dot"></i><b data-count="asset">0</b> assets</span><span><i class="legend-dot callback-dot"></i><b data-count="callback">0</b> callbacks</span><span><i class="legend-dot address-dot"></i><b data-count="address">0</b> addresses</span><span class="graph-live" data-live-summary>0 reachable instantly</span><span class="graph-busy" data-busy-summary>0 tasks in flight</span><span class="graph-shell" data-shell-summary>0 shells attached</span><span class="graph-snapshot" data-graph-snapshot>Loading snapshot…</span></div>
<div class="graph-layout"><div class="graph-canvas-wrap"><div class="graph-canvas" data-graph-canvas tabindex="0" aria-label="Relationship graph. Drag to pan; use the zoom buttons or arrow keys. Select a node for details."><svg data-graph-svg viewBox="0 0 1000 680" aria-label="Operation, asset and callback relationships"><g data-graph-layer></g></svg></div><div class="graph-list" data-graph-list hidden></div><div class="graph-empty" data-graph-empty hidden><span class="empty-orbit" aria-hidden="true">◎</span><h2>Your field of view starts here.</h2><p>Recorded operations and callbacks will appear here when available.</p><a href="/operations">Open operations ↗</a></div><div class="graph-controls"><button type="button" data-zoom="in" aria-label="Zoom in">+</button><button type="button" data-zoom="out" aria-label="Zoom out">−</button><button type="button" data-zoom="fit">Fit</button><span data-graph-visible role="status" aria-live="polite"></span></div></div><aside class="node-inspector" data-node-inspector aria-label="Selected node details"><p class="eyebrow">NODE INSPECTOR</p><div data-node-detail><span class="inspector-symbol" aria-hidden="true">⌖</span><h2>Every connection<br>has context.</h2><p>Select a node to see its identity, status, and recorded relationships.</p></div><div class="inspector-footnote">Lines represent saved associations and DNS observations. They do not imply a network route or pivot.</div></aside></div>
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
static DIRECTORY_SLOTS: OnceLock<Semaphore> = OnceLock::new();

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

// ── Directory enumeration ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DirectoryPageQuery {
    asset: Option<String>,
}

#[derive(Deserialize)]
struct DirectoryForm {
    asset_id: String,
    base_dn: String,
    bind_dn: String,
    bind_password: String,
    object_class: String,
    limit: String,
    csrf_token: Option<String>,
}

fn object_class_from(value: &str) -> Option<ObjectClass> {
    match value {
        "user" => Some(ObjectClass::User),
        "group" => Some(ObjectClass::Group),
        "computer" => Some(ObjectClass::Computer),
        "all" => Some(ObjectClass::All),
        _ => None,
    }
}

pub async fn directory_page(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repo): State<Repository>,
    session: Session,
    Query(query): Query<DirectoryPageQuery>,
) -> Result<Html<String>, AppError> {
    directory_html(&repo, &user, &session, query.asset.as_deref(), None)
        .await
        .map(Html)
}

async fn directory_html(
    repo: &Repository,
    user: &AuthenticatedUser,
    session: &Session,
    selected: Option<&str>,
    error: Option<&str>,
) -> Result<String, AppError> {
    let (_ops, assets, runs) = records(repo, user).await?;
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
        .filter(|run| {
            run.check_id == "directory-enum" && (selected.is_empty() || run.asset_id == selected)
        })
        .collect();

    // Posture findings first: those are the lines that tell you whether
    // credential-theft tooling would succeed here.
    let mut posture: Vec<(String, String)> = Vec::new();
    let mut counts: Vec<(String, String)> = Vec::new();
    let mut listed = 0usize;
    for run in history.iter().rev() {
        let Some(result) = run.result_json.as_ref() else {
            continue;
        };
        let Some(findings) = result.get("findings").and_then(|f| f.as_array()) else {
            continue;
        };
        for finding in findings {
            let (Some(code), Some(message)) = (
                finding.get("code").and_then(|c| c.as_str()),
                finding.get("message").and_then(|m| m.as_str()),
            ) else {
                continue;
            };
            if code.starts_with("posture.") {
                posture.push((code.to_owned(), message.to_owned()));
            } else if code.starts_with("directory.count.") {
                counts.push((
                    code.trim_start_matches("directory.count.").to_owned(),
                    message.to_owned(),
                ));
            } else if code.starts_with("directory.") && listed < 60 {
                listed += 1;
            }
        }
    }

    let error_html = error
        .map(|message| format!(r#"<p class="form-error" role="alert">{}</p>"#, esc(message)))
        .unwrap_or_default();
    let disabled = if !can_run || assets.is_empty() {
        " disabled"
    } else {
        ""
    };
    let posture_html = if posture.is_empty() {
        r#"<p class="recon-scope-note">No posture findings in the recorded runs.</p>"#.to_owned()
    } else {
        format!(
            r#"<ul class="report-list">{}</ul>"#,
            posture
                .iter()
                .map(|(code, message)| {
                    format!(
                        r#"<li><b>{}</b><br><span>{}</span></li>"#,
                        esc(code),
                        esc(message)
                    )
                })
                .collect::<String>()
        )
    };
    let counts_html = if counts.is_empty() {
        String::new()
    } else {
        format!(
            r#"<div class="recon-stats">{}</div>"#,
            counts
                .iter()
                .map(|(label, value)| format!("<span><b>{}</b> {}</span>", esc(value), esc(label)))
                .collect::<String>()
        )
    };

    let body = format!(
        r#"<section class="recon-workspace" data-recon><div class="recon-heading"><div><p class="eyebrow">DIRECTORY / ENUMERATE</p><h2>Map the domain<br><em>before anyone else does.</em></h2><p>Enumerate users, groups, and computers over LDAP, and surface the posture conditions that make credential-theft tooling succeed.</p></div><a class="button" href="/inventory">Open inventory ↗</a></div><div class="recon-columns"><section class="recon-launch panel"><span class="eyebrow">NEW ENUMERATION</span><h2>Point at a domain controller.</h2>{error_html}<form method="post" action="/directory/run" data-recon-form><input type="hidden" name="csrf_token" value="{csrf}"><div class="form-field"><label for="directory-asset">Scoped asset</label><select id="directory-asset" name="asset_id" required{disabled}><option value="">Choose an asset</option>{options}</select></div><div class="form-field"><label for="directory-base">Base DN</label><input id="directory-base" name="base_dn" placeholder="DC=corp,DC=example" maxlength="256"{disabled}><p class="field-hint">Leave blank to read the naming context from the rootDSE.</p></div><div class="form-field"><label for="directory-object">Object class</label><select id="directory-object" name="object_class"{disabled}><option value="user">Users</option><option value="group">Groups</option><option value="computer">Computers</option><option value="all">Everything</option></select></div><div class="form-field"><label for="directory-bind">Bind DN</label><input id="directory-bind" name="bind_dn" placeholder="corp\svc-recon" maxlength="256" autocomplete="off"{disabled}><p class="field-hint">Leave blank for an anonymous bind. Anonymous read access is itself a finding.</p></div><div class="form-field"><label for="directory-password">Bind password</label><input id="directory-password" name="bind_password" type="password" maxlength="256" autocomplete="new-password"{disabled}><p class="field-hint">Never stored: the password is stripped from the run before it is persisted, and a password forces ldaps.</p></div><div class="form-field"><label for="directory-limit">Entry limit</label><input id="directory-limit" name="limit" type="number" min="1" max="200" value="100"{disabled}></div><button type="submit" class="primary-action"{disabled}>Enumerate directory <span aria-hidden="true">↗</span></button><p class="recon-scope-note">{permission}</p></form></section><section class="recon-history"><div class="section-heading"><h2>Posture</h2><span>{history_count} runs</span></div>{counts_html}{posture_html}<p class="inspector-footnote">This check never requests passwords, hashes, or ticket material. It reports posture only.</p></section></div></section>"#,
        csrf = esc(&csrf),
        disabled = disabled,
        options = options,
        error_html = error_html,
        permission = if can_run {
            ""
        } else {
            "Your role cannot start directory runs."
        },
        posture_html = posture_html,
        counts_html = counts_html,
        history_count = history.len(),
    );
    Ok(templates::app_page("Directory", user, "directory", &body))
}

pub async fn run_directory(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repo): State<Repository>,
    session: Session,
    request: Request,
) -> Result<Response, AppError> {
    user.require(Role::Operator)?;
    let Form(form) = Form::<DirectoryForm>::from_request(request, &repo)
        .await
        .map_err(|_| AppError::Validation("Invalid form".into()))?;
    let Some(object_class) = object_class_from(&form.object_class) else {
        return Err(AppError::Validation("Invalid object class".into()));
    };
    if !csrf_token_matches(&session, form.csrf_token.as_deref()).await? {
        return Err(AppError::Validation("Invalid request".into()));
    }
    let limit = form
        .limit
        .trim()
        .parse::<usize>()
        .unwrap_or(100)
        .clamp(1, 200);
    for value in [&form.base_dn, &form.bind_dn] {
        if value.len() > 256 {
            return Err(AppError::Validation("Field is too long".into()));
        }
    }

    authorize_asset_run(&repo, &user, &form.asset_id).await?;
    let asset = repo
        .find_asset(&form.asset_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let operation = asset.operation_id.clone();
    let correlation = uuid::Uuid::new_v4().to_string();
    audit(
        &repo,
        &user,
        &operation,
        &asset.id,
        "directory.requested",
        ("success", format!("object_class={}", form.object_class)),
        &correlation,
    )
    .await?;

    if let Err(error) =
        crate::checks::directory::endpoint(&asset.address, !form.bind_password.is_empty())
    {
        return Ok((
            StatusCode::BAD_REQUEST,
            Html(directory_html(&repo, &user, &session, Some(&asset.id), Some(error)).await?),
        )
            .into_response());
    }

    let _permit = DIRECTORY_SLOTS
        .get_or_init(|| Semaphore::new(2))
        .try_acquire()
        .map_err(|_| AppError::Conflict("Directory capacity reached".into()))?;

    let asset_id = asset.id.clone();
    let result = Runner::new(repo.clone(), user.clone())
        .start(
            asset,
            "directory-enum",
            CheckInput::DirectoryEnum(DirectoryEnumInput {
                base_dn: form.base_dn.trim().to_owned(),
                bind_dn: form.bind_dn.trim().to_owned(),
                bind_password: form.bind_password,
                object_class,
                limit,
            }),
        )
        .await?;
    audit(
        &repo,
        &user,
        &operation,
        &asset_id,
        "directory.finished",
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
    Ok(Redirect::to(&format!("/directory?asset={}", form.asset_id)).into_response())
}

#[cfg(test)]
mod topology_liveness_tests {
    use super::*;
    use crate::db::models::CallbackStatus;
    use time::format_description::well_known::Rfc3339;

    fn callback_seen_at(last_seen: &str, interval_ms: Option<i64>) -> Callback {
        Callback {
            id: "session-1".into(),
            asset_id: None,
            operation_id: None,
            host: "web-01".into(),
            user_name: "u".into(),
            process: "p".into(),
            arch: "x86_64".into(),
            os: "linux".into(),
            os_version: None,
            executable_path: None,
            local_addr: None,
            implant_version: None,
            interval_ms,
            jitter_ms: None,
            capabilities_json: None,
            protocol: "http".into(),
            // Deliberately "active": the stored column is never demoted, which
            // is exactly the bug these tests pin down.
            status: CallbackStatus::Active,
            last_seen: last_seen.into(),
            created_at: last_seen.into(),
        }
    }

    fn ago(seconds: i64) -> String {
        (time::OffsetDateTime::now_utc() - time::Duration::seconds(seconds))
            .format(&Rfc3339)
            .expect("valid timestamp")
    }

    #[test]
    fn a_recent_checkin_reads_as_active() {
        let now = time::OffsetDateTime::now_utc();
        let state = live_state(&callback_seen_at(&ago(5), Some(10_000)), now);
        assert_eq!(state, "active");
    }

    #[test]
    fn a_silent_checkin_reads_as_lost_even_though_stored_status_is_active() {
        let now = time::OffsetDateTime::now_utc();
        let state = live_state(&callback_seen_at(&ago(3_600), Some(10_000)), now);
        assert_eq!(state, "lost");
    }

    #[test]
    fn topology_node_uses_liveness_not_the_stored_status() {
        let stale = callback_seen_at(&ago(86_400), Some(10_000));
        let data = topology(&[], &[], &[stale], &[], &CallbackActivity::default());
        let node = data
            .nodes
            .iter()
            .find(|n| n.kind == "callback")
            .expect("callback node present");
        assert_eq!(node.status, "lost");
    }

    #[test]
    fn a_live_callback_node_stays_active_in_the_graph() {
        let fresh = callback_seen_at(&ago(2), Some(10_000));
        let data = topology(&[], &[], &[fresh], &[], &CallbackActivity::default());
        let node = data
            .nodes
            .iter()
            .find(|n| n.kind == "callback")
            .expect("callback node present");
        assert_eq!(node.status, "active");
    }

    /// A graph of registered hosts is inventory. These are the numbers that say
    /// which host to act on first.
    #[test]
    fn a_callback_node_reports_whether_it_answers_instantly_and_what_it_is_running() {
        let callback = callback_seen_at(&ago(2), Some(10_000));
        let mut activity = CallbackActivity::default();
        activity.live.insert(callback.id.clone());
        activity.tasks.insert(callback.id.clone(), 2);
        activity.shells.insert(callback.id.clone(), 1);

        let data = topology(&[], &[], &[callback], &[], &activity);
        let node = data
            .nodes
            .iter()
            .find(|n| n.kind == "callback")
            .expect("callback node present");

        assert!(node.instant, "an open socket means actions are instant");
        assert_eq!(data.counts.instant, 1);
        assert_eq!(data.counts.active, 1);
        assert_eq!(data.counts.tasks_in_flight, 2);
        assert_eq!(data.counts.shells, 1);
        let fact = |name: &str| {
            node.facts
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(fact("Tasks in flight").as_deref(), Some("2"));
        assert_eq!(fact("Shells attached").as_deref(), Some("1"));
        assert_eq!(fact("Channel").as_deref(), Some("Instant (open socket)"));
    }

    #[test]
    fn a_callback_without_a_socket_says_it_works_on_its_beacon_interval() {
        let callback = callback_seen_at(&ago(2), Some(10_000));
        let data = topology(&[], &[], &[callback], &[], &CallbackActivity::default());
        let node = data
            .nodes
            .iter()
            .find(|n| n.kind == "callback")
            .expect("callback node present");
        assert!(!node.instant);
        let channel = node
            .facts
            .iter()
            .find(|(key, _)| key == "Channel")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        assert!(channel.starts_with("Beacon every"), "{channel}");
    }

    #[test]
    fn a_reported_address_becomes_a_node_the_callback_points_at() {
        let mut callback = callback_seen_at(&ago(2), Some(10_000));
        callback.local_addr = Some("10.20.30.40".into());
        let data = topology(&[], &[], &[callback], &[], &CallbackActivity::default());

        let address = data
            .nodes
            .iter()
            .find(|n| n.kind == "address")
            .expect("the reported address is in the graph");
        assert_eq!(address.label, "10.20.30.40");
        assert!(
            data.edges
                .iter()
                .any(|edge| edge.source.starts_with("callback:") && edge.target == address.id),
            "the callback points at its own address"
        );
    }

    /// Two callbacks behind one address share a node; a duplicated address node
    /// would read as two hosts.
    #[test]
    fn a_shared_address_is_one_node() {
        let mut first = callback_seen_at(&ago(2), Some(10_000));
        first.local_addr = Some("10.20.30.40".into());
        let mut second = callback_seen_at(&ago(3), Some(10_000));
        second.local_addr = Some("10.20.30.40".into());
        let data = topology(
            &[],
            &[],
            &[first, second],
            &[],
            &CallbackActivity::default(),
        );
        assert_eq!(data.nodes.iter().filter(|n| n.kind == "address").count(), 1);
        assert_eq!(
            data.edges.iter().filter(|e| e.label == "reports").count(),
            2,
            "both callbacks still point at it"
        );
    }

    #[test]
    fn a_callback_age_is_reported_so_a_stale_node_can_be_spotted() {
        let fresh = callback_seen_at(&ago(30), Some(10_000));
        let data = topology(&[], &[], &[fresh], &[], &CallbackActivity::default());
        let node = data.nodes.iter().find(|n| n.kind == "callback").unwrap();
        let age = node.age_seconds.expect("age is reported");
        assert!((25..=60).contains(&age), "unexpected age {age}");
    }
}
