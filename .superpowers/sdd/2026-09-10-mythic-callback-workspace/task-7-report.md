# Task 7 Report — Cross-Platform Filesystem Control

- Start: clean `develop` at `d8304f9`; no inherited edits.
- RED (implant): `cargo test -p nw-implant --test callback_controls filesystem_ -- --nocapture` failed to compile because `nw_implant::filesystem` and `File{Entry,ListV1,MutationV1,ControlError}` did not exist.
- RED (filesystem safety): the exact-destination test showed Unix `rename` overwrote an existing file; the implementation now returns stable `already_exists` and preserves both files.
- RED (projection/API): `cargo test -p naughtywolf --test callback_workspace_test filesystem_ -- --nocapture` failed because `latest_file_snapshot` and typed routes did not exist; a mismatched successful mutation was also initially accepted instead of rolling back.
- RED (browser/template): Node reported 16 pass / 4 fail for missing path restoration, navigation, confirmations, and refresh; the Rust template test failed on missing `data-file-panel`.
- GREEN (implant): typed Rust-only `list`, `stat`, `mkdir`, `move_path`, and `delete`; exact command dispatch/arity/boolean checks; absolute UTF-8 entries; stable errors; Unix owner/mode; empty/NUL rejection; nonrecursive refusal and exact-subtree recursive deletion entirely inside fresh temp trees.
- GREEN (server): strict typed routes enforce operator role, callback scope, CSRF, normalized absolute paths, exact-path audit details, reserved-command isolation, typed mutation-result matching, and atomic/monotonic `c2_file_snapshots` upserts keyed by callback plus normalized requested path.
- GREEN (UI): URL-backed file path/tab state including popstate, Linux/drive/UNC breadcrumbs and parent navigation, path bar, safe text-only rows, sorting, refresh, mkdir/move/delete exact-target dialogs, snapshot staleness/capability/task links, and both transfers disabled with exact copy `Transfer support is being initialized`.
- Final specified output: profile control 3/3; implant filesystem 6/6; portal filesystem 6/6; `node --check static/callback-workspace.js` exit 0.
- Regression output: implant callback controls 12/12; portal callback workspace 20/20; DOM workspace 20/20; escalated `cargo test --workspace` all suites/doc-tests passed (sandbox-only first attempt had two loopback `Operation not permitted` failures).
- Lint: `cargo clippy -p nw-profile -p nw-implant -p naughtywolf --all-targets` exited 0; remaining warnings pre-exist outside Task 7. `git diff --check` is clean.
- Cross-target concern: `x86_64-pc-windows-gnu` is listed installed, but `cargo check -p nw-implant --target x86_64-pc-windows-gnu` fails with `E0463: can't find crate for core`; Windows std installation is broken on the active toolchain.
- Files: brief-listed Rust/server/template/JS/CSS/test files, plus `src/callback_workspace/tasks.rs` (reserve typed filesystem commands), `tests/callback_workspace_test.cjs` (DOM/security behavior and browser-faithful `textContent`), and this report. No push performed.
