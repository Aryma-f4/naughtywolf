# NaughtyWolf → Empire-Aligned Business Flow

## Phase 1: Core Infrastructure (Week 1)

### Task 1.1: Agent Lifecycle
- **Current:** Sessions + Beacons pages list Sliver implants
- **Empire:** Agents have check-in history, stale/dead tracking, tags, tasks, files
- **Changes:**
  - Add `/api/agents` unified endpoint merging sessions + beacons
  - Add check-in history (timeline of check-in times)
  - Add agent tags (via `tag_api.py` pattern)
  - Add stale detection (>5min no check-in = stale)
  - Add agent rename/archive/delete
  - Add Agent detail page: info, check-in graph, task history

### Task 1.2: Task Execution (Agent Tasks)
- **Empire:** Tasks run on agents (shell, module, script)
- **Sliver Equivalent:** Sliver has `clientpb.ImplantTask` and RPCs for sending tasks to sessions
- **Changes:**
  - Add `POST /api/agents/{id}/tasks/shell` — execute shell command on session
  - Add `POST /api/agents/{id}/tasks/execute` — execute program on session
  - Add `GET /api/agents/{id}/tasks` — list task history
  - Add `GET /api/tasks/{id}` — get task result/output
  - SPA: Agent detail page with task list + shell output + file browser

### Task 1.3: Real-Time WebSocket
- **Empire:** Uses Socket.IO for real-time agent updates, task results, new agents
- **Changes:**
  - Add WebSocket endpoint using `tokio-tungstenite` or Axum WebSocket
  - Events: `new_agent`, `task_result`, `agent_checkin`, `listener_started`, `listener_stopped`
  - SPA: replace REST polling with WebSocket for live dashboard updates
  - Keep existing SSE `/api/events` for Sliver event stream

## Phase 2: Empire Feature Parity (Week 2)

### Task 2.1: Host & Network Tracking
- **Empire:** Hosts page tracks all hosts seen by agents, their processes, network connections
- **Changes:**
  - Add `/api/hosts` — list all unique hosts from sessions/beacons
  - Add `/api/hosts/{id}` — host detail (processes, connections, users)
  - Add `/api/agents/{id}/processes` — list processes on agent
  - Add `/api/agents/{id}/network` — network connections
  - SPA: Hosts page with drill-down to agents

### Task 2.2: Downloads & File Operations
- **Empire:** Downloads API serves files exfiltrated from agents
- **Changes:**
  - Add `/api/agents/{id}/files/ls` — list directory
  - Add `/api/agents/{id}/files/download` — download file from agent
  - Add `/api/agents/{id}/files/upload` — upload file to agent
  - Keep existing `/api/payloads/download/{name}` for payload download
  - SPA: File browser in agent detail page

### Task 2.3: Credential Management (Enhanced)
- **Empire:** Credentials API with collection, hash type, cracking status
- **Changes:**
  - Extend `/api/creds` with hashcat mode, plaintext password field
  - Add `POST /api/creds` — add credential
  - Add `PUT /api/creds/{id}` — edit/plaintext
  - Add `POST /api/creds/{id}/crack` — mark as cracked
  - SPA: Credential management page (add, edit, crack, filter by collection)

### Task 2.4: Module System
- **Empire:** Modules are Python scripts that run on agents for post-exploitation
- **Sliver Equivalent:** Extensions/Armory, but limited
- **Changes:**
  - Add `/api/modules` — list available Sliver extensions
  - Add `/api/modules/{name}` — module detail
  - Add `POST /api/agents/{id}/modules/{name}` — execute module on agent
  - SPA: Module browser (search, filter, execute on agent)

### Task 2.5: Stagers (Payload Generation v2)
- **Empire:** Stagers generate payloads (launcher, DLL, macro, etc.)
- **Changes:**
  - Redesign `/api/payloads/generate` to match Empire stager flow
  - Add stager templates (similar to Empire `stager_template_api.py`)
  - Add `GET /api/payloads/templates` — list templates
  - Add `POST /api/payloads/generate` — generate with template
  - Fix: clean build dir before generate to avoid "rename import dir" error
  - Fix: handle gRPC message size for large binaries
  - SPA: Stager creation wizard

## Phase 3: Advanced Features (Week 3)

### Task 3.1: Pivot & Tunneling
- **Empire:** No direct pivot support
- **Sliver:** Has pivots (TCP, named pipe), port forwarding, SOCKS
- **Changes:**
  - Add `POST /api/agents/{id}/pivot` — create pivot listener
  - Add `GET /api/pivots` — list all pivots
  - Add `POST /api/agents/{id}/socks/start` — start SOCKS proxy
  - Add `POST /api/agents/{id}/socks/stop` — stop SOCKS proxy
  - Add `POST /api/agents/{id}/portfwd/{id}` — add port forward rule
  - SPA: Pivot graph visualization, SOCKS toggle, port forward table

### Task 3.2: Reporting & Export
- **Empire:** No native reporting
- **Changes:**
  - Add `GET /api/reports/sessions` — session report
  - Add `GET /api/reports/credentials` — credential report
  - Add `GET /api/reports/hosts` — host inventory report
  - Add `GET /api/reports/timeline` — operation timeline
  - Export formats: JSON, CSV
  - SPA: Report generation page with filter options + download button

### Task 3.3: Dashboard v2 — Live Ops View
- **Current:** Simple metric cards
- **Empire-like:** Live updating dashboard with operation overview
- **Changes:**
  - Add WebSocket-connected dashboard
  - Add operation timer (time since first agent)
  - Add active beacon timeline (check-in graph)
  - Add top targets (most interacted hosts)
  - Add quick actions (generate stager, start listener)
  - Add notification feed for new agents/task completion

### Task 3.4: Admin & Configuration v2
- **Empire:** Admin API for user management, settings, plugins
- **Changes:**
  - Add `POST /api/admin/users` — create user (Empire-style JWT)
  - Add `PUT /api/admin/users/{id}` — update role
  - Add `GET /api/admin/settings` — server config
  - Add `PUT /api/admin/settings` — update config
  - Add `/api/health` — server health endpoint
  - Add `/api/meta` — server info (Empire-style meta endpoint)
  - SPA: Admin settings page (config editor, health check)

### Task 3.5: Tagging System
- **Empire:** Unified tagging for agents, hosts, credentials, listeners
- **Changes:**
  - Add `/api/tags` — CRUD tags
  - Add tag endpoints to agents, hosts, credentials, listeners
  - SPA: Tag badges + filter by tag on list pages

## Phase 4: Quality & Polish (Week 4)

### Task 4.1: Tests
- **Empire:** pytest with conftest.py
- **Add:**
  - Integration tests for all Empire-aligned endpoints
  - Test coverage with DB fixtures
  - WebSocket test helpers

### Task 4.2: Documentation
- **Empire:** MkDocs documentation
- **Add:**
  - API documentation (following Empire's patterns)
  - User guide (operation workflow)
  - Developer guide (how to add modules/stagers)

### Task 4.3: Deployment
- Current: systemd service + VPS
- **Add:**
  - Docker Compose (like Empire's `.github/docker-compose.yml`)
  - Health checks
  - Graceful shutdown with drain
  - Prometheus metrics (optional)

---

## Empire API Reference (for alignment)

| Empire Endpoint | NaughtyWolf Equivalent | Status |
|----------------|----------------------|--------|
| `/api/v2/agents/` | `/api/sessions` + `/api/beacons` | ⚠️ Partial |
| `/api/v2/agents/{uid}/tasks/` | — | ❌ Missing |
| `/api/v2/listeners/` | `/api/listeners` | ⚠️ Basic |
| `/api/v2/stagers/` | `/api/payloads/generate` | ❌ Broken |
| `/api/v2/credentials/` | `/api/creds` | ⚠️ Basic |
| `/api/v2/hosts/` | — | ❌ Missing |
| `/api/v2/downloads/` | `/api/payloads/download/{name}` | ⚠️ Basic |
| `/api/v2/modules/` | — | ❌ Missing |
| `/api/v2/users/` | `/api/users` | ✅ Working |
| `/api/v2/admin/` | — | ❌ Missing |
| `/api/v2/plugins/` | — | ❌ Not applicable |
| `/api/v2/health/` | — | ❌ Missing |
| `/api/v2/meta/` | — | ❌ Missing |
| `/api/v2/tags/` | — | ❌ Missing |
| `/api/v2/obfuscation/` | — | ❌ Missing |
| `/api/v2/bypass/` | — | ❌ Not applicable |
| `/api/v2/websocket/` | `/api/events` (SSE) | ⚠️ SSE only |
| `/api/v2/ip/` | — | ❌ Missing |
| `/api/v2/profile/` | — | ❌ Missing |

**Total: 18 Empire endpoint categories — NaughtyWolf has 4 working, 3 partial, 11 missing.**
