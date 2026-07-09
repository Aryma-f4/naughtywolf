# NaughtyWolf UI SPA Polish Design

Date: 2026-07-09

## Goal

Convert NaughtyWolf from a collection of server-rendered HTML pages into a polished lightweight single-page application (SPA) with a Neo-cyber C2 console visual direction and a Cobalt Strike-inspired chain graph view showing listeners, hosts, sessions, beacons, and pivot-style relationships.

The app should no longer feel like quick scaffolded HTML. It should feel intentional: consistent shell, reusable UI patterns, status-aware tables, useful empty states, and a graph workspace that looks like an operator console.

## Chosen Direction

Use a lightweight SPA served by the existing Axum backend:

- Axum remains the Rust API/auth/session server.
- A single authenticated route serves the SPA shell.
- Static frontend files live under `static/`.
- Client-side router renders pages without full-page reloads.
- No Node, no Next.js build pipeline, no separate frontend app yet.

This gives a Next.js-like user experience while preserving the current simple Rust deployment model.

## Scope

### Included

- SPA shell with client-side navigation.
- Neo-cyber visual system across all primary pages.
- Cobalt Strike-style chain graph visualization page.
- Consistent components for cards, tables, forms, buttons, badges, empty states, loading states, and error states.
- Dashboard redesign using the new system.
- Pages covered:
  - Dashboard
  - Chain Graph
  - Sessions
  - Beacons
  - Listeners
  - Payloads
  - Websites
  - Loot
  - Credentials
  - Events
  - Audit
  - Admin
- Existing `/api/*` endpoints remain the data source.
- Existing server-rendered login page remains, but is restyled to match the new visual language.

### Excluded

- Full Next.js app.
- Node-based bundling.
- Drag/drop graph editing.
- Persisted custom graph layouts.
- Real pivot inference beyond the data available from current APIs.
- Replacing existing Axum session auth.

## Current Context

Current UI files:

- `static/style.css`: global CSS for login, sidebar, tables, cards, buttons.
- `static/login.html`: server-rendered login page.
- `src/web/templates.rs`: server-side app shell with sidebar/topbar layout.
- `src/web/routes.rs`: per-page HTML strings and inline scripts.
- `src/web/api.rs`: JSON APIs used by pages.

The current implementation works but mixes page markup, scripts, and Rust route code. This makes the UI feel inconsistent and hard to polish globally. SPA conversion should move app UI behavior into static frontend files and keep Rust focused on auth and API delivery.

## Architecture

### Server Responsibilities

Keep Rust/Axum responsible for:

- Login/logout and session creation.
- Authentication guard.
- Serving the SPA shell to authenticated users.
- Serving static assets.
- Existing JSON APIs.

Add/adjust routes:

- `/app` serves the SPA shell.
- `/` and legacy authenticated page routes can redirect to `/app#/dashboard` or keep working during migration.
- `/graph` should route to `/app#/graph` or be represented in client routing only.

The old server-rendered page routes may remain as a compatibility fallback during this iteration, but primary navigation should use the SPA.

### Frontend Files

Add:

- `static/app.html`
  - Single root container.
  - Links `style.css` and `app.js`.
- `static/app.js`
  - Client-side router.
  - API helper.
  - Render functions for pages.
  - Shared component helper functions.
  - Graph rendering logic using native SVG.

Keep and rewrite:

- `static/style.css`
  - Full visual system.
  - SPA shell styles.
  - Graph styles.
  - Existing login styles, updated to match Neo-cyber direction.

## Visual System

### Personality

Neo-cyber C2 console:

- Dark layered background.
- Cobalt/cyan primary glow.
- Magenta/red danger accents.
- Green active state.
- Amber warning state.
- Subtle grid/radar texture.
- Compact information density.
- Serious operator-console feel, not playful emoji UI.

### Core Tokens

Use CSS variables for:

- Background layers: app, panel, raised panel, overlay.
- Borders: subtle, active, danger.
- Text: primary, muted, dim.
- Status colors: active, warning, danger, unknown.
- Glow colors: cyan, magenta, green.
- Radius, shadow, spacing, font sizes.

### Components

Standardize:

- `.app-shell`
- `.side-rail`
- `.brand-block`
- `.nav-link`
- `.status-strip`
- `.page-header`
- `.metric-grid`
- `.metric-card`
- `.panel`
- `.panel-header`
- `.data-table`
- `.badge`
- `.btn`, `.btn-primary`, `.btn-danger`, `.btn-ghost`
- `.form-grid`, `.field`, `.input`, `.select`
- `.empty-state`
- `.toast` / inline `.notice`
- `.terminal-panel`

## SPA Behavior

### Routing

Use hash routing for simplicity:

- `#/dashboard`
- `#/graph`
- `#/sessions`
- `#/beacons`
- `#/listeners`
- `#/payloads`
- `#/websites`
- `#/loot`
- `#/creds`
- `#/events`
- `#/audit`
- `#/admin`

Hash routing avoids server rewrite complexity and works with the current Axum/static setup.

### Data Loading

`app.js` should provide:

- `apiGet(path)`
- `apiPost(path, body)`
- `apiDelete(path)` when listener kill lands
- `renderLoading()`
- `renderError(message)`
- `renderEmpty(title, detail)`

Every page should handle:

- loading state
- API success
- API error
- empty list state

### Navigation

Clicking navigation links should:

- update hash route
- render page content without reload
- update active nav state
- update topbar title/status

## Chain Graph Visualization

### Page

Route: `#/graph`

Name: `Chain Graph`

Purpose: Show operator-relevant infrastructure relationships in a Cobalt Strike-inspired visualization.

### Data Sources

Use existing APIs:

- `GET /api/listeners`
- `GET /api/sessions`
- `GET /api/beacons`
- `GET /api/sliver/status`

### Graph Model

Nodes:

- Sliver server / profile root.
- Listener nodes from `/api/listeners`.
- Session host nodes from `/api/sessions`.
- Beacon host nodes from `/api/beacons`.
- Unknown placeholder nodes when data is missing but a relationship should be shown.

Edges:

- Root/profile → listeners.
- Listeners → sessions/beacons when transport/protocol match can be inferred.
- Session/beacon → host identity.

Because current API data does not include exact listener-to-implant parentage, initial edges are best-effort and labeled as inferred. Do not claim exact pivot chains unless data supports them.

### Layout

Use native SVG with grouped columns:

- Column 1: Sliver profile/root.
- Column 2: listeners.
- Column 3: active sessions/beacons.
- Column 4: host details / future pivots.

Style:

- Neon node borders.
- Status-colored rings.
- Animated/soft glowing edges.
- Small labels for host, user, transport, status.
- Dead/unknown nodes muted or red.

Interactions:

- Hover node: highlight edges and show details panel.
- Click node: pin details in side panel.
- Refresh button reloads graph data.

## Page-Level Changes

### Dashboard

- Replace plain cards with metric cards.
- Add graph preview panel linking to Chain Graph.
- Add event feed styled as terminal output.
- Show connection/profile status prominently.

### Sessions and Beacons

- Use standardized data tables.
- Status badges for active/dead.
- Better empty state when Sliver is disconnected or no implants exist.

### Listeners

- Use polished create/list/kill UI once listener feature lands.
- Until then, table and action panel should still look intentional.

### Payloads

- Rebuild payload generator form into panel/card system.
- Show generation status with inline notice.
- Existing API stays unchanged.

### Websites, Loot, Credentials

- Use consistent tables and empty states.
- Keep current fields.

### Events

- Terminal-like stream panel with monospace text, timestamps, and status colors.

### Admin and Audit

- Admin gets cleaner user table and role badges.
- Audit keeps placeholder data for now but should look like a real audit workspace.

## Error Handling and UX Rules

- No page should show raw JSON or browser-default errors.
- API failure should render a styled error state.
- Disconnected Sliver should be treated as an operational state, not broken UI.
- Empty tables must show a clear empty state.
- Dangerous actions use red styling and confirmation when implemented.
- SPA should not expose pages to unauthenticated users; server auth remains authoritative.

## Security

This is an authenticated local C2 management UI. The SPA must not weaken server-side checks:

- All create/kill/admin operations still require backend RBAC.
- Client-side role checks only hide UI; they are not security controls.
- Do not put secrets into static JS.
- Do not add arbitrary command execution to client APIs.

## Testing Plan

Automated:

- `cargo check`
- `cargo test`
- `cargo clippy`

Manual browser smoke:

- Login page renders with new theme.
- Login redirects into SPA.
- Navigation between SPA pages does not reload document.
- Dashboard loads stats or clean disconnected state.
- Chain Graph renders with empty/disconnected data.
- Sessions/beacons/listeners/payloads render loading, empty, and error states.
- Logout works.
- Browser console has no fatal JS errors.

## Implementation Order

1. Add `static/app.html` SPA shell.
2. Add `static/app.js` with router, API helpers, shared components, and basic pages.
3. Update `src/web/routes.rs` and/or root redirect so authenticated users land on `/app#/dashboard`.
4. Rewrite `static/style.css` into the Neo-cyber design system while preserving login styles.
5. Add Chain Graph renderer using native SVG.
6. Port dashboard/list table pages into SPA render functions.
7. Keep legacy routes as fallback until SPA is verified.
8. Run automated checks.
9. Run dev server and manual browser/API smoke checks.

## Acceptance Criteria

- User can access `http://127.0.0.1:8080/app#/dashboard` after login.
- App navigation behaves as SPA without full page reloads.
- All major pages share one polished Neo-cyber design system.
- Chain Graph page exists and renders listener/session/beacon topology from existing APIs.
- Empty/disconnected states look intentional.
- Existing APIs and authentication continue working.
- `cargo test` and `cargo clippy` pass.
