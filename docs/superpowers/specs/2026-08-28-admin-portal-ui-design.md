# Admin Portal UI Design

## Purpose

Replace the current bare landing and login pages with a lightweight,
responsive administrative portal for the standalone authorized security-lab
platform. The portal presents only local administrative and defensive-lab
workflows: operation scope, asset inventory, built-in check results, audit
events, evidence, reports, and role administration. It does not expose or
describe C2, payload generation, agents, persistence, or evasion features.

## Scope

This slice provides the application shell, public landing page, login page,
and role-aware screens for the existing SQLite-backed concepts.

### Public surface

- `GET /` is a neutral product landing page. It contains no C2-related
  terminology or implementation hints.
- `GET /login` presents the local sign-in form.
- Failed sign-in renders the same form with a generic inline error message.

### Authenticated portal

- The authenticated home is `/dashboard`.
- A compact floating dock is fixed near the bottom on narrow screens. On wider
  screens it can move to the top edge, while retaining the same semantic
  navigation and keyboard focus order.
- Navigation contains Dashboard, Operations, Inventory, Checks, Audit,
  Evidence, Reports, and Admin. It uses text labels and simple inline SVG
  icons, not icon-only controls.
- The dashboard is useful with an empty database: it displays zero-state
  metric cards and explains the next safe setup action (create an operation,
  then add scoped assets). It never invents activity.
- Operations, Inventory, Checks, Audit, Evidence, and Reports present the
  stored records and empty states. The first UI slice may use server-rendered
  data instead of a client-side API.
- Admin includes user list/status and role information. Only an Admin can see
  or access it.

## RBAC and enforcement

The UI mirrors, but never replaces, server-side policy enforcement.

| Role | UI capability |
| --- | --- |
| Viewer | Read dashboard, scoped inventory, check history, audit, evidence metadata, and reports. |
| Operator | Viewer capabilities plus forms for operations, scoped assets, and built-in check runs in an allowed operation. |
| Admin | Operator capabilities plus user status and role administration. |

- Menu items and action buttons are omitted when the authenticated role lacks
  permission.
- Direct navigation to a restricted screen returns a forbidden response or a
  safe redirect; it must not rely on client-side hiding.
- Every successful mutation continues to emit an audit event through the
  existing audit writer.

## Presentation and responsive behavior

- Use plain HTML, a single local CSS stylesheet, and small vanilla JavaScript
  only where interaction requires it. No frontend framework or external CDN.
- The visual language is editorial and calm: warm neutral background, dark
  ink, a single moss/teal accent, restrained shadows, readable system fonts,
  and clear spacing. Avoid neon "cyber" styling.
- Desktop (at least 900px): two-column dashboard sections and a top floating
  dock.
- Tablet (600–899px): one or two column cards according to available width;
  bottom floating dock with horizontally scrollable labels if necessary.
- Mobile (below 600px): single column content, bottom dock, 44px minimum
  interactive targets, no horizontal page overflow, tables turn into labelled
  record cards or scroll within a clearly labelled region.
- Respect `prefers-reduced-motion`, visible focus rings, semantic headings,
  labels for form controls, and sufficient contrast.

## Technical structure

- Extract UI HTML into `src/web/templates.rs` or dedicated static templates;
  route handlers remain thin in the active Axum router.
- Serve `/static/admin.css` and `/static/admin.js` directly from the Rust
  binary with correct content types. The browser must not require Node, a
  build step, or a network connection.
- Add a small authenticated `PortalContext` query/service layer that derives
  dashboard counts and lists from the existing repository models. It must use
  the authenticated user and operation scope for every query.
- Preserve the current `app_router` test seam and add route-level tests for
  public, authenticated, and role-restricted pages.

## Error handling and security

- Login failures remain generic and do not reveal whether a username exists.
- Page errors use a concise in-app message; server logs retain diagnostic
  detail.
- Dynamic values are HTML-escaped before rendering.
- Evidence pages expose only metadata and authorized download links. Existing
  evidence path validation remains authoritative.
- No privileged browser action is trusted without a matching authenticated
  and authorized server route.

## Acceptance criteria

- `/`, `/login`, and the authenticated dashboard have usable visual styling.
- The portal is functional at 320px width through desktop width.
- Admin navigation and user/role controls cannot be accessed by Viewer or
  Operator accounts.
- Dashboard and list pages render coherent zero states from an empty SQLite
  database.
- The project compiles and route-level tests verify public pages, login
  failure rendering, authenticated redirect behavior, and role denial.
