# NaughtyWolf Operator Admin UI Design

Date: 2026-08-29
Status: approved from the user's direct request for an execution-ready plan
Scope: visual and interaction overhaul of the active server-rendered portal

## Problem

The active portal already links `/static/admin.css`, but its login and authenticated pages use a minimal light theme and generic layouts. The repository also contains an older SPA shell in `static/login.html`, `static/app.html`, and `static/style.css`; those files are not the active `/login` and authenticated portal path served by `src/main.rs`.

This split makes it easy to improve the wrong UI. The redesign must target only the active stack:

- HTML: `src/portal/templates.rs`
- CSS: `static/admin.css`
- progressive enhancement: `static/admin.js`
- routes/assets: `src/portal.rs`
- behavior tests: `tests/portal_routes_test.rs`

## Goal

Create a polished, original security-operations interface inspired by the information density and clarity of mature C2 consoles, without copying Cobalt Strike or Mythic branding, layout, or assets.

## Non-goals

- No new C2 web backend, session streaming, task dispatch, WebSocket, or API work.
- No edits to the legacy SPA files `static/login.html`, `static/app.html`, `static/style.css`, or `static/app.js`.
- No external font, icon, CSS, or JavaScript CDN.
- No frontend build tool or JavaScript framework.
- No auth, CSRF, role, repository, route, or database behavior changes.
- No fake live telemetry or security claims that the backend does not provide.

## Chosen direction

Use a dark tactical operations theme with restrained cyan as the primary accent, amber for warnings, green for healthy/enabled state, and red only for destructive/failed state. Surfaces are deep navy, borders are crisp, typography combines the local system UI stack with a monospace stack for IDs and technical values. A subtle CSS grid texture can appear in the login brand panel; avoid copied product visuals and excessive neon effects.

Three navigation approaches were considered:

1. **Centered top navigation on desktop plus bottom navigation on mobile — chosen.** It preserves content width, avoids a generic left sidebar, and matches the user's preference.
2. Persistent bottom dock on every viewport. It is distinctive but wastes desktop vertical space and scales poorly across eight destinations.
3. Collapsible left rail. It handles many items but was explicitly rejected because it would resemble common admin templates and the old SPA.

## Information architecture

Keep the existing role-aware destinations and URLs:

- Dashboard → `/dashboard`
- Operations → `/operations`
- Inventory → `/inventory`
- Checks → `/checks`
- Evidence → `/evidence`
- Audit → `/audit`
- Reports → `/reports`
- Admin → `/admin/users`, Admin role only

The authenticated shell contains one semantic `<header>` with three desktop zones:

- left: compact NW mark, `NaughtyWolf`, and `Operator Portal`
- center: horizontally scrollable primary `<nav>` with the active page marked by `aria-current="page"`
- right: username, role badge, and the existing POST logout form

At widths below 768px, the brand and identity remain in the top header while the same navigation becomes a fixed bottom dock. It scrolls horizontally and the active item is centered by progressive JavaScript enhancement. There is no sidebar.

## Login experience

Desktop login is a two-panel composition:

- brand panel: NW mark, `Authorized Operations Workspace`, one short product statement, and three factual capability labels: scoped operations, append-only audit, verified evidence
- form panel: compact sign-in card with username, password, generic error region, CSRF hidden input, and a clear submit button

Mobile login stacks the brand summary above the form. Labels remain visible; placeholders never replace labels. Username/password autocomplete and the existing generic authentication error remain unchanged. Copy must say `Authorized lab access only`, not `secure connection` or any unsupported transport claim.

## Authenticated pages

- Dashboard uses five metric cards from the existing `DashboardSummary`; it does not invent charts or trends.
- Operations and reports use responsive card grids with strong hierarchy.
- Checks, audit, evidence, and admin users keep semantic tables inside horizontal scroll containers.
- Status values use text plus color-coded pills; color is never the only signal.
- Forms use a consistent panel, label, help/error, input, and action layout.
- Empty states use a bordered panel and a clear factual message.
- Admin user controls visually separate role changes from enable/disable actions; existing authorization and CSRF behavior remain authoritative.

## Interaction and accessibility

- Minimum interactive target: 44 by 44 CSS pixels.
- Visible `:focus-visible` outline on every interactive control.
- Skip link remains first in the authenticated body.
- Active navigation uses both `aria-current` and visual treatment.
- Form errors use `role="alert"`; invalid forms retain `aria-describedby`.
- Tables retain captions and scoped headers.
- Layout supports 320px width without body-level horizontal overflow; only navigation and table wrappers may scroll horizontally.
- `prefers-reduced-motion` removes nonessential transitions.
- Print styles continue hiding navigation/header and preserving report cards.

## JavaScript boundary

`static/admin.js` remains optional progressive enhancement. It may center the active navigation item and set a `data-scrolled` state on the sticky top bar. Navigation, logout, forms, and page content must work with JavaScript disabled. Remove the obsolete local-storage dock-position behavior because the new layout has one responsive position controlled by CSS.

## Testing and visual verification

Automated tests assert the active template contract, not CSS implementation details:

- login contains the stylesheet, split-layout classes, visible labels, autocomplete, CSRF, and accessible error behavior
- authenticated shell contains header, centered semantic navigation, mobile-compatible dock class, POST logout form, role identity, active `aria-current`, and no sidebar
- Admin destination appears only for Admin users
- dashboard renders the five real counts in metric-card markup
- existing security, scoping, CSRF, and admin mutation tests remain green

Visual QA must inspect `/login`, `/dashboard`, `/operations`, and `/admin/users` at 1440×900, 1024×768, 390×844, and 320×568. Verify focus, long IDs, long usernames, table scrolling, empty states, and generic login errors.

## Acceptance criteria

1. The active login page visibly uses the new local design system without external assets.
2. Desktop uses a centered top navigation; mobile uses a fixed bottom navigation; no authenticated page uses a left sidebar.
3. All existing portal destinations, authorization, CSRF, logout method, and data scoping behave unchanged.
4. Admin user management is usable at desktop and mobile widths.
5. The portal remains server-rendered and functional without JavaScript.
6. Focus, reduced-motion, table overflow, print, and 320px layouts are supported.
7. Focused portal tests and `cargo test --workspace` pass; no new warnings are introduced.

