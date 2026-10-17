//! Attaching an operator's terminal to a callback.
//!
//! Every route here is a thin wrapper around the live channel: the request is
//! turned into one [`Live`] frame and written to the callback's open socket. If
//! that socket is not there the operator is told so plainly rather than having
//! the request silently become a queued task — an interactive shell that starts
//! five seconds later is not an interactive shell.
//!
//! Authorization, scoping and CSRF match every other callback endpoint: an
//! operator may only reach a callback their operation contains.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use nw_profile::msgs::Live;
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, time::Duration};
use tower_sessions::Session;
use uuid::Uuid;

use crate::{
    AppError,
    auth::{AuthenticatedUser, middleware::AuthenticatedUserGuard, rbac::Role},
    c2_live::{LiveError, LiveRegistry},
    callback_workspace::tasks::{require_csrf, require_visible_callback},
    db::repositories::Repository,
    shell_hub::{ShellHub, ShellView},
};

/// How the request reached the callback.
///
/// `push` means the frame went out this instant over an open socket; `queued`
/// means there is no live connection, so the shell starts when the callback next
/// checks in. The UI says which happened, because it changes what the operator
/// should expect from the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryChannel {
    Push,
    Queued,
}

#[derive(Debug, Serialize)]
pub struct ShellOpened {
    pub shell: ShellView,
    pub delivery: DeliveryChannel,
}

#[derive(Debug, Deserialize)]
pub struct OpenShellRequest {
    /// Empty or absent means "the platform's own shell".
    #[serde(default)]
    pub program: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct ShellInputRequest {
    #[serde(default)]
    pub shell_id: Option<String>,
    pub data: String,
}

#[derive(Debug, Deserialize)]
pub struct ShellIdRequest {
    pub shell_id: String,
}

#[derive(Debug, Serialize)]
pub struct ShellInputAccepted {
    pub shell_id: Uuid,
    pub delivery: DeliveryChannel,
    pub bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct ShellsSnapshot {
    pub shells: Vec<ShellView>,
    /// A socket is open right now, so an attach is immediate.
    pub live: bool,
    /// This callback's carrier can hold a live channel at all. False means
    /// attaching will always wait for a beacon, so the UI can say so up front
    /// instead of leaving the operator wondering why nothing happened.
    pub can_attach: bool,
}

/// One burst of keystrokes, and the ceiling on it.
///
/// A paste or a `cat` of a large file is legitimately bigger than a line, but
/// not unbounded: this is a request body arriving from a browser.
const MAX_INPUT_BYTES: usize = 64 * 1024;

/// Reject anything that is not a plain command line for the implant to run.
///
/// The program is operator input arriving from a browser and becomes an
/// argument to a process spawn on the callback host, so it is bounded and
/// checked here rather than trusted.
fn validate_program(program: &str) -> Result<String, AppError> {
    let program = program.trim();
    if program.is_empty() || program.chars().count() > 256 {
        return Err(AppError::Validation("invalid shell program".to_owned()));
    }
    if program.chars().any(char::is_control) || program.contains('\0') {
        return Err(AppError::Validation("invalid shell program".to_owned()));
    }
    Ok(program.to_owned())
}

fn validate_args(args: &[String]) -> Result<Vec<String>, AppError> {
    if args.len() > 32 {
        return Err(AppError::Validation("too many shell arguments".to_owned()));
    }
    for arg in args {
        if arg.chars().count() > 1024 || arg.contains('\0') {
            return Err(AppError::Validation("invalid shell argument".to_owned()));
        }
    }
    Ok(args.to_vec())
}

fn parse_shell_id(raw: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(raw).map_err(|_| AppError::Validation("invalid shell id".to_owned()))
}

fn decode_key(encoded: &str) -> Result<[u8; 32], AppError> {
    let raw = STANDARD.decode(encoded).map_err(|_| AppError::Internal)?;
    <[u8; 32]>::try_from(raw.as_slice()).map_err(|_| AppError::Internal)
}

/// Seal one live frame for this session and write it to the callback's socket.
async fn push_to_session(
    repository: &Repository,
    live: &LiveRegistry,
    session_id: &str,
    frame: Live,
) -> Result<DeliveryChannel, AppError> {
    let parsed = Uuid::parse_str(session_id).map_err(|_| AppError::NotFound)?;
    let encoded = repository
        .session_key_for(session_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let key = decode_key(&encoded)?;
    match live.push(&key, parsed, frame).await {
        Ok(()) => Ok(DeliveryChannel::Push),
        // No open socket: the shell starts when the callback next checks in.
        Err(LiveError::NotConnected) => Ok(DeliveryChannel::Queued),
        Err(error) => {
            tracing::error!(%error, "live frame could not be encoded");
            Err(AppError::Internal)
        }
    }
}

async fn require_operator(
    user: &AuthenticatedUser,
    repository: &Repository,
    session_id: &str,
) -> Result<(), AppError> {
    user.require(Role::Operator)?;
    require_visible_callback(repository, user, session_id).await
}

/// Whether this callback can be pushed to right now.
fn is_live(live: &LiveRegistry, session_id: &str) -> bool {
    Uuid::parse_str(session_id)
        .map(|id| live.is_live(id))
        .unwrap_or(false)
}

/// Whether the callback is registered on a carrier that can hold a live
/// channel at all. Used to tell "attach later" apart from "never possible".
async fn can_be_live(repository: &Repository, session_id: &str) -> bool {
    repository
        .find_callback(session_id)
        .await
        .ok()
        .flatten()
        .and_then(|callback| callback.capabilities_json)
        .is_some_and(|capabilities| capabilities.live_channel)
}

/// `GET /api/callbacks/{session_id}/shell` — what is attached right now.
pub async fn list_shells(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Extension(live): Extension<LiveRegistry>,
    Extension(hub): Extension<ShellHub>,
    Path(session_id): Path<String>,
) -> Result<Json<ShellsSnapshot>, AppError> {
    require_operator(&user, &repository, &session_id).await?;
    Ok(Json(ShellsSnapshot {
        shells: hub.shells(&session_id),
        live: is_live(&live, &session_id),
        can_attach: can_be_live(&repository, &session_id).await,
    }))
}

/// `POST /api/callbacks/{session_id}/shell` — attach a shell.
///
/// The id is minted server-side so the implant's confirmation has a name to
/// arrive under and a browser cannot smuggle in an id it invented.
#[allow(clippy::too_many_arguments)]
pub async fn open_shell(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Extension(live): Extension<LiveRegistry>,
    Extension(hub): Extension<ShellHub>,
    session: Session,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<OpenShellRequest>,
) -> Result<Json<ShellOpened>, AppError> {
    require_operator(&user, &repository, &session_id).await?;
    require_csrf(&session, &headers).await?;

    let program = match request.program.as_deref() {
        Some(program) if !program.trim().is_empty() => validate_program(program)?,
        _ => String::new(),
    };
    let args = validate_args(request.args.as_deref().unwrap_or(&[]))?;

    let shell_id = Uuid::new_v4();
    hub.expect_shell(&session_id, shell_id, &program, &args);
    let delivery = match push_to_session(
        &repository,
        &live,
        &session_id,
        Live::ShellOpen {
            shell_id,
            program: program.clone(),
            args: args.clone(),
        },
    )
    .await
    {
        Ok(delivery) => delivery,
        Err(error) => {
            hub.fail(&session_id, shell_id, error.to_string());
            return Err(error);
        }
    };
    if delivery == DeliveryChannel::Queued {
        tracing::info!(
            session_id,
            "callback has no live channel; shell waits for a beacon"
        );
    }
    let shell = hub
        .shells(&session_id)
        .into_iter()
        .find(|view| view.id == shell_id)
        .unwrap_or(ShellView {
            id: shell_id,
            program,
            args,
            pty: false,
            state: "starting",
            opened_at: chrono::Utc::now().to_rfc3339(),
            exit_code: None,
            scrollback: String::new(),
        });
    Ok(Json(ShellOpened { shell, delivery }))
}

/// `POST /api/callbacks/{session_id}/shell/input` — send keystrokes.
#[allow(clippy::too_many_arguments)]
pub async fn shell_input(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Extension(live): Extension<LiveRegistry>,
    Extension(hub): Extension<ShellHub>,
    session: Session,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ShellInputRequest>,
) -> Result<Json<ShellInputAccepted>, AppError> {
    require_operator(&user, &repository, &session_id).await?;
    require_csrf(&session, &headers).await?;

    // Keystrokes include control characters (\r, \x03, escape sequences), so
    // the only bound that matters is the size of one burst.
    if request.data.is_empty() || request.data.len() > MAX_INPUT_BYTES {
        return Err(AppError::Validation("invalid shell input".to_owned()));
    }
    let bytes = request.data.len();
    let shell_id = match request.shell_id.as_deref() {
        Some(raw) => parse_shell_id(raw)?,
        // Typing goes to whatever is attached, which is what an operator means.
        None => hub
            .active_shell(&session_id)
            .ok_or(AppError::Conflict("no shell is attached".to_owned()))?,
    };
    let delivery = push_to_session(
        &repository,
        &live,
        &session_id,
        Live::ShellInput {
            shell_id,
            data: request.data,
        },
    )
    .await?;
    Ok(Json(ShellInputAccepted {
        shell_id,
        delivery,
        bytes,
    }))
}

/// `POST /api/callbacks/{session_id}/shell/close` — detach.
#[allow(clippy::too_many_arguments)]
pub async fn close_shell(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Extension(live): Extension<LiveRegistry>,
    Extension(hub): Extension<ShellHub>,
    session: Session,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ShellIdRequest>,
) -> Result<StatusCode, AppError> {
    require_operator(&user, &repository, &session_id).await?;
    require_csrf(&session, &headers).await?;
    let shell_id = parse_shell_id(&request.shell_id)?;
    let _ = hub;
    let _ = push_to_session(
        &repository,
        &live,
        &session_id,
        Live::ShellClose { shell_id },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/callbacks/{session_id}/shell/stream` — output as it happens.
///
/// Server-sent events rather than a second socket: the browser reads one
/// stream, and keystrokes go back over the POST above. Reconnecting replays the
/// snapshot, so a refresh reattaches to what is already running.
pub async fn shell_stream(
    AuthenticatedUserGuard(user): AuthenticatedUserGuard,
    State(repository): State<Repository>,
    Extension(hub): Extension<ShellHub>,
    Path(session_id): Path<String>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>> + Send>, AppError> {
    require_operator(&user, &repository, &session_id).await?;
    let subscription = hub.subscribe(&session_id);
    let stream = futures::stream::unfold(subscription, |mut subscription| async move {
        let message = subscription.recv().await?;
        let payload = serde_json::to_string(&message).ok()?;
        let event = Event::default().event("shell").data(payload);
        Some((Ok(event), subscription))
    });
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_bounded_and_free_of_control_characters() {
        assert_eq!(validate_program("  /bin/bash ").unwrap(), "/bin/bash");
        assert!(validate_program("").is_err());
        assert!(validate_program(&"x".repeat(257)).is_err());
        assert!(validate_program("/bin/sh\nrm -rf /").is_err());
        assert!(validate_program("/bin/\0sh").is_err());
    }

    #[test]
    fn arguments_are_bounded() {
        let args = vec!["-c".to_owned(), "id".to_owned()];
        assert_eq!(validate_args(&args).unwrap(), args);
        let too_many: Vec<String> = std::iter::repeat_n("x".to_owned(), 33).collect();
        assert!(validate_args(&too_many).is_err());
        assert!(validate_args(&["x".repeat(1025).to_owned()]).is_err());
        assert!(validate_args(&["ok\0bad".to_owned()]).is_err());
    }

    #[test]
    fn shell_ids_must_be_uuids() {
        assert!(parse_shell_id("not-a-uuid").is_err());
        let id = Uuid::new_v4();
        assert_eq!(parse_shell_id(&id.to_string()).unwrap(), id);
    }

    #[test]
    fn the_delivery_channel_is_serialized_for_the_ui() {
        assert_eq!(
            serde_json::to_string(&DeliveryChannel::Push).unwrap(),
            "\"push\""
        );
        assert_eq!(
            serde_json::to_string(&DeliveryChannel::Queued).unwrap(),
            "\"queued\""
        );
    }
}
