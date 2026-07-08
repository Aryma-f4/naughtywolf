use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::{clientpb, commonpb};

/// Fetch all active sessions from the Sliver server.
pub async fn list_sessions(conn: &mut SliverConnection) -> Result<Vec<clientpb::Session>, String> {
    let response = conn
        .client
        .get_sessions(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("get_sessions failed: {e}"))?;

    Ok(response.into_inner().sessions)
}
