# Task 3 report: SQLite-backed local authentication

## Scope delivered

- Rebuilt authentication around the Task 1--2 SQLite `Repository` and `users`
  table. `AuthenticatedUser` now carries a string database identifier and
  exposes `require(Role)`, which returns `AppError::Forbidden` for an
  unauthorized role.
- Added `Role::allows`, preserving the existing role hierarchy.
- Added credential authentication that only returns enabled users after Argon2
  verification; disabled users cannot obtain a session candidate.
- Converted session identity storage from UUID to SQLite-compatible string
  user IDs and made session writes return `AppError` rather than only logging
  a failed session update.
- Removed the Sliver profile CLI and all profile command exposure. The local
  CLI provides `user create`, `user reset-password`, `user disable`, and
  `user list` against SQLite.
- Password input is either a terminal prompt or the explicit
  `--password-stdin` flag for CI. It is neither a command-line argument nor
  included in an audit record or normal CLI output.
- User creation writes a transactional `user.created` audit record. Its
  summary includes the username and role only.
- The standalone server initializes a migrated `SqliteStore`, has an
  eight-hour inactivity expiry, honors `cookie_secure`, signs session cookies,
  and rejects session secrets shorter than 32 bytes. It exposes only a local
  health endpoint; no Sliver/C2 routes were reactivated.

## TDD evidence

### RED 1 -- role and password behavior

Added the required `operator_cannot_manage_users` and
`hashed_password_verifies_only_the_original_secret` tests to
`tests/auth_test.rs` before authentication was exposed through the library.

Command:

```text
cargo test --test auth_test
```

Observed failure:

```text
error[E0433]: failed to resolve: could not find `auth` in `naughtywolf`
 --> tests/auth_test.rs:1:18
```

This was the expected missing authentication surface, before adding the public
module and `Role::allows`.

### GREEN 1 -- role and password behavior

Command:

```text
cargo test --test auth_test
```

Observed output:

```text
running 2 tests
test operator_cannot_manage_users ... ok
test hashed_password_verifies_only_the_original_secret ... ok
test result: ok. 2 passed; 0 failed
```

### RED 2 -- disabled users

Added the required disabled-user session test against a real in-memory SQLite
database, migrations, and `SqliteStore` session layer before implementing the
authentication service/session API.

Command:

```text
cargo test --test auth_test
```

Observed failure:

```text
error[E0432]: unresolved import `naughtywolf::auth::authenticate`
error[E0061]: this method takes 3 arguments but 1 argument was supplied
```

The test required the missing enabled-user authentication function and the
SQLite string-ID session-login API.

### GREEN 2 -- disabled users

Command:

```text
cargo test --test auth_test
```

Observed output:

```text
running 3 tests
test operator_cannot_manage_users ... ok
test disabled_user_cannot_start_a_session ... ok
test hashed_password_verifies_only_the_original_secret ... ok
test result: ok. 3 passed; 0 failed
```

## Verification

```text
cargo test --lib auth::
running 7 tests
test result: ok. 7 passed; 0 failed

cargo check --bin naughtywolf
Finished `dev` profile [unoptimized + debuginfo]

git diff --check
(no output; exit 0)
```

CLI smoke test on a disposable SQLite database:

```text
User 'smoke-admin' created with role 'admin' (id: ...)
user.created|user|...|username=smoke-admin, role=admin|success
Password reset for 'smoke-admin'
User 'smoke-admin' disabled
smoke-admin|admin|1
```

The `user create --help` and `user reset-password --help` output shows only
the explicit `--password-stdin` CI option; no `--password` command-line
option exists.

## Required broad-command result

The exact requested command below remains blocked by pre-existing,
out-of-scope legacy test targets, before it can execute the auth filter:

```text
cargo test auth::
```

It fails compiling `tests/integration_test.rs`, `tests/db_migration_test.rs`,
and `tests/cli_user_test.rs`, which still import PostgreSQL (`PgPool`) and
the removed `naughtywolf::sliver` / `naughtywolf::web` modules. Those files
were not modified because this task explicitly excludes retaining or
reactivating Sliver/C2 features. The focused auth and library-auth commands
above are green.

## Files changed

- `Cargo.toml`, `Cargo.lock`: enable signed session cookies and the time/key
  support needed for a 32-byte derived key and inactivity expiration.
- `src/lib.rs`: expose the auth and CLI modules to the binary/tests.
- `src/auth/mod.rs`, `src/auth/middleware.rs`, `src/auth/rbac.rs`: SQLite
  authentication, string session identity, authorization guard API.
- `src/cli/mod.rs`, `src/cli/users.rs`: standalone local user administration.
- `src/cli/profiles.rs`: deleted.
- `src/main.rs`: CLI dispatch and SQLite-backed signed session server setup.
- `tests/auth_test.rs`: required role/password and disabled-user session
  integration coverage.

## Self-review

- SQLite SQL uses `?` parameters; no PostgreSQL placeholders, enum casts, or
  `now()` expressions remain in the Task 3 auth/CLI/server paths.
- Passwords are hashed with the existing Argon2 implementation and are not
  logged, displayed, or captured in the audit parameter summary.
- Creation and audit insertion share one transaction, avoiding a user with no
  `user.created` audit event after a partial write.
- The 32-byte validation precedes `Key::derive_from`, preventing its panic;
  `with_signed` makes the configured secret material operational rather than
  merely checked.
- The disabled-user test runs with real SQLite persistence and a real
  `SqliteStore`, not a mock.
- No pre-existing dirty user-owned files are included in the task commit.

## Concerns

- Existing sessions are not revoked when an account is subsequently disabled;
  the explicit Task 3 requirement and regression test cover preventing a
  disabled account from starting a new session. Session revocation would need
  a later, explicit session-user revalidation policy.
- The old PostgreSQL/Sliver integration tests prevent the broad filtered cargo
  command from compiling. They require a separate cleanup task rather than
  reintroducing the removed C2 surface here.

## Fix round 1 -- review corrections

### Finding 1: obsolete integration targets blocked `cargo test auth::`

The failing targets were inspected before removal. `tests/db_migration_test.rs`
and `tests/cli_user_test.rs` imported PostgreSQL APIs that are no longer
enabled; `tests/integration_test.rs` exercised the retired Sliver/web router.
SQLite migration coverage already exists in `tests/config_db_test.rs`, and the
current local-user behavior is covered by `tests/auth_test.rs` plus the CLI
smoke test below. The three obsolete target files were retired; no C2 module,
route, or dependency was restored.

GREEN command:

```text
cargo test auth::
running 7 auth unit tests: all passed
running 4 auth integration tests: 0 run (filter), target compiled successfully
running config_db_test and repository_test: target compilation succeeded
test result: ok
```

### Finding 2: a post-login disable did not invalidate authorization

RED regression test added before changing the extractor:

```text
cargo test --test auth_test user_disabled_after_login_cannot_use_an_existing_session
assertion `left == right` failed
left: 204
right: 401
```

The regression uses a real in-memory SQLite users table and a real
`SqliteStore`: it logs in an enabled user, captures the issued cookie, marks
the account disabled in the database, and requests a protected route with the
old cookie.

The extractor now reloads the enabled identity by session user ID on every
request. A missing or disabled row deletes the session and returns 401; a
database failure returns the existing sanitized `AppError` response. Returning
the fresh identity also means role and username changes are observed on the
next request instead of trusting stale session fields.

GREEN command:

```text
cargo test --test auth_test user_disabled_after_login_cannot_use_an_existing_session
running 1 test
test user_disabled_after_login_cannot_use_an_existing_session ... ok
test result: ok. 1 passed; 0 failed
```

### Finding 3: local CLI commands unnecessarily required a session secret

RED smoke command was run from a temporary working directory, preventing the
repository `.env` file from supplying a value:

```text
env -u NAUGHTYWOLF_SESSION_SECRET ... naughtywolf user create ...
Error: Missing required environment variable: NAUGHTYWOLF_SESSION_SECRET
```

`Config::database_url_from_env` now provides the minimal database-only
configuration used before dispatch. `main.rs` creates and migrates the SQLite
pool from that value, while only the `serve` branch calls `Config::from_env`
and therefore requires/uses the signed-session secret.

GREEN smoke command, again from a clean temporary working directory:

```text
env -u NAUGHTYWOLF_SESSION_SECRET ... naughtywolf user create --username no-secret --password-stdin
User 'no-secret' created with role 'operator' (id: ...)
sqlite3 ... "SELECT username, disabled FROM users WHERE username = 'no-secret'"
no-secret|0
```

### Fix-round verification

```text
cargo test --test auth_test
running 4 tests
test result: ok. 4 passed; 0 failed

cargo test auth::
test result: ok; auth unit tests passed and all remaining test targets compiled
```

### Files changed in this correction

- `src/auth/mod.rs`: added fresh enabled-user lookup by database ID.
- `src/auth/middleware.rs`: revalidates the session identity against SQLite on
  every protected request and deletes stale sessions.
- `src/config.rs`, `src/main.rs`: separate CLI database configuration from the
  server/session configuration.
- `tests/auth_test.rs`: added the post-login disable regression and protected
  route fixture.
- `tests/db_migration_test.rs`, `tests/cli_user_test.rs`,
  `tests/integration_test.rs`: deleted obsolete PostgreSQL/Sliver-only test
  targets that prevented the required filtered test command from compiling.

### Fix-round self-review and concerns

- The extractor’s database lookup is parameterized and checks `disabled = 0`;
  it does not trust the stored session role or username for authorization.
- The session is explicitly deleted when a user is disabled or removed, so the
  next request cannot reuse the stale cookie.
- `serve` continues to enforce the 32-byte session-secret requirement before
  deriving the signing key; local CLI commands neither read nor need it.
- No C2 behavior was restored. The retired tests were exclusively obsolete
  PostgreSQL/Sliver coverage; current SQLite migration tests remain.
- Remaining concern: every protected request now performs one SQLite lookup,
  which is the deliberate security tradeoff requested for immediate disable
  enforcement.
