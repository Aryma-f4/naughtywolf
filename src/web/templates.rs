use axum::response::Html;
use serde::Serialize;

#[derive(Serialize)]
pub struct PageContext {
    pub title: String,
    pub current_page: &'static str,
    pub username: String,
    pub role: String,
    pub profile_name: Option<String>,
}

pub fn render_page(ctx: &PageContext, content: &str) -> Html<String> {
    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title} — NaughtyWolf</title>
    <link rel="stylesheet" href="/static/style.css">
</head>
<body>
<div class="app-layout">
    <nav class="sidebar">
        <div class="sidebar-header">
            <h2>🐺 NaughtyWolf</h2>
            <p>Sliver Console</p>
        </div>
        <ul class="nav-list">
            <li class="nav-item {active_dash}" onclick="window.location='/dashboard'">
                <span class="icon">📊</span><span>Dashboard</span>
            </li>
            <li class="nav-item {active_sessions}" onclick="window.location='/sessions'">
                <span class="icon">💻</span><span>Sessions</span>
            </li>
            <li class="nav-item {active_beacons}" onclick="window.location='/beacons'">
                <span class="icon">📡</span><span>Beacons</span>
            </li>
            <li class="nav-item {active_listeners}" onclick="window.location='/listeners'">
                <span class="icon">👂</span><span>Listeners</span>
            </li>
            <li class="nav-item {active_payloads}" onclick="window.location='/payloads'">
                <span class="icon">📦</span><span>Payloads</span>
            </li>
            <li class="nav-item {active_websites}" onclick="window.location='/websites'">
                <span class="icon">🌐</span><span>Websites</span>
            </li>
            <li class="nav-item {active_loot}" onclick="window.location='/loot'">
                <span class="icon">💰</span><span>Loot</span>
            </li>
            <li class="nav-item {active_creds}" onclick="window.location='/creds'">
                <span class="icon">🔑</span><span>Credentials</span>
            </li>
            <li class="nav-item {active_events}" onclick="window.location='/events'">
                <span class="icon">⚡</span><span>Events</span>
            </li>
            <li class="nav-item {active_audit}" onclick="window.location='/audit'">
                <span class="icon">📋</span><span>Audit</span>
            </li>
            <li class="nav-item {active_admin}" onclick="window.location='/admin'">
                <span class="icon">⚙️</span><span>Admin</span>
            </li>
        </ul>
    </nav>
    <div class="main-content">
        <header class="topbar">
            <div class="topbar-left">
                <h3>{title}</h3>
                {profile_badge}
            </div>
            <div class="topbar-right">
                <span class="user-info">{username} ({role})</span>
                <a href="/logout" class="logout-link">Logout</a>
            </div>
        </header>
        <main class="content-area">
            {content}
        </main>
    </div>
</div>
</body>
</html>"#,
        title = ctx.title,
        active_dash = if ctx.current_page == "dashboard" { "active" } else { "" },
        active_sessions = if ctx.current_page == "sessions" { "active" } else { "" },
        active_beacons = if ctx.current_page == "beacons" { "active" } else { "" },
        active_listeners = if ctx.current_page == "listeners" { "active" } else { "" },
        active_payloads = if ctx.current_page == "payloads" { "active" } else { "" },
        active_websites = if ctx.current_page == "websites" { "active" } else { "" },
        active_loot = if ctx.current_page == "loot" { "active" } else { "" },
        active_creds = if ctx.current_page == "creds" { "active" } else { "" },
        active_events = if ctx.current_page == "events" { "active" } else { "" },
        active_audit = if ctx.current_page == "audit" { "active" } else { "" },
        active_admin = if ctx.current_page == "admin" { "active" } else { "" },
        profile_badge = match &ctx.profile_name {
            Some(name) => format!(r#"<span class="profile-badge">{}</span>"#, name),
            None => String::new(),
        },
        username = ctx.username,
        role = ctx.role,
    ))
}
