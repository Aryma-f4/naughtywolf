# Topology and scoped reconnaissance

Build two additive portal pages without changing existing callback/task workflows.

- `/topology`: an operation → asset → callback relationship map. Explicit database associations are the only edges; unassigned callbacks remain unlinked. Latest saved DNS results add address nodes. Viewer accounts never receive callback details.
- Search, operation/status filters, graph/list modes, zoom, pan, fit, selection inspector, and an explicitly labelled sample dataset make large inventories readable. Bound on-screen rendering and disclose hidden node counts; keep filtering over the complete loaded dataset.
- `/recon`: select an existing asset and run DNS or DNS + HTTP HEAD. Use the existing Runner and check history, with existing active-operation/asset, role, and membership checks. CSRF is required before execution. Four concurrent requests, a bounded timeout, capped DNS results/output, no redirect following or proxy environment, and validated TLS for HTTPS.
- Persist findings and audit the request/completion. Do not store cookies or bodies. Surface observed HTTP headers/status and DNS addresses; never infer an exploit or pivot relationship.
- Keep working server-rendered content and normal POST forms without JavaScript. Integrate the client module with the existing content-navigation lifecycle. Verify role/scope boundaries, target parsing, result storage, filtering, and graph selection with tests and browser checks.
