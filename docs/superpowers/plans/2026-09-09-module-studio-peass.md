# Module Studio and PEASS Implementation Plan

> Execute this plan inline with tests written before production behavior.

## 1. Add implant module contracts

- Add failing tests for command recognition, custom-source limits, UTF-8 base64 decoding, interpreter mapping, output truncation, CVE extraction, and user enumeration parsing.
- Implement `crates/implant/src/modules.rs` with bounded helpers and task-result construction.
- Export the module and route recognized commands from `BeaconRuntime::run_one`.

## 2. Add safe custom-code execution

- Test valid and invalid languages, oversized source, malformed base64, timeout propagation, and temporary-file cleanup.
- Write decoded source to a private temporary file.
- Invoke the selected interpreter directly and return bounded output.

## 3. Add user enumeration

- Test Unix passwd parsing and normalized JSON output.
- Add fixed Unix and Windows collectors without operator-controlled command interpolation.
- Return structured, read-only account details through normal task results.

## 4. Add PEASS assessment

- Test platform asset selection, allowed redirect hosts, download-size enforcement, digest reporting, and CVE token extraction using local fixtures.
- Download the official latest asset with a bounded streaming response.
- Execute with direct arguments, delete the artifact, and prepend provenance plus candidate CVEs to the result.

## 5. Build Module Studio UI

- Add a failing portal template test for all three module actions and the no-auto-exploit boundary.
- Add failing JavaScript tests for UTF-8 source encoding and task payload creation.
- Render quick-action cards and the custom-code editor in callback detail.
- Refactor callback JavaScript into idempotent task submission used by the command dock and Module Studio.
- Add responsive red-theme styling without animating or replacing navigation.

## 6. Document and verify

- Document module behavior, limits, PEASS provenance, callback egress requirement, and audit boundary.
- Link the guide from README.
- Run focused Rust and JavaScript tests, then `cargo test --workspace`, all Node tests, and `cargo fmt --all -- --check`.
