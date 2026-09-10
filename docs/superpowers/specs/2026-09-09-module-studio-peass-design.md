# Module Studio, PEASS Assessment, and User Enumeration Design

## Goal

Add a focused Module Studio to each callback so an authorized operator can run read-only user enumeration, request a current PEASS assessment, or execute a small custom script without leaving the callback workspace.

## Product behavior

The callback detail page gains three actions:

- **Enumerate users** gathers local account and logged-in-user information using fixed, read-only commands or operating-system files.
- **PEASS assessment** downloads the matching official latest PEASS release on the callback host, records its SHA-256 digest, runs it with a bounded timeout and output size, and returns findings through normal task history.
- **Custom code** accepts a small script, selects a supported interpreter, and sends UTF-8 source as base64. The implant writes it to a temporary file, invokes the interpreter with direct process arguments, and deletes the file afterward.

The privilege escalation workflow stops after assessment. It highlights possible paths and CVE references for operator review; it never launches an exploit or changes privileges automatically.

## Implant boundary

New built-in commands live in a dedicated `modules` module:

- `nw/user-enum`
- `nw/peas-audit`
- `nw/exec-code <shell|powershell|python> <base64-source>`

The runtime dispatches these before the generic command runner. Every result keeps the original task UUID and uses the existing sealed callback transport.

Limits protect callback stability:

- custom source: 24 KiB
- PEASS artifact: 64 MiB
- returned stdout/stderr: 2 MiB combined
- timeout: the task timeout, with a safe default

PEASS downloads follow redirects only to GitHub-owned release hosts. The implant reports the final URL and SHA-256 digest before the tool output. Temporary scripts and binaries are removed after execution.

## Interface

Module Studio appears between the callback header and console grid. Quick-action cards describe their effects and expose a single clear button. The custom-code card expands into an interpreter selector, timeout control, editor, and run button. A visible assessment boundary explains that PEASS provides candidates for review and does not exploit them.

All module requests reuse the existing task JSON endpoint and CSRF token. JavaScript initialization is idempotent and reruns after in-app navigation so the fixed navigation shell remains untouched.

## Verification

Unit tests cover source validation, interpreter selection, output truncation, PEASS URL/host policy, CVE extraction, and user enumeration parsing. Portal tests assert the Module Studio contract, while browser-side tests verify UTF-8 base64 encoding and module task payloads. Workspace tests and formatting remain the final gate.
