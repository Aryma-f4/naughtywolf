use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::{clientpb, commonpb};

/// Fetch all active beacons from the Sliver server.
pub async fn list_beacons(conn: &mut SliverConnection) -> Result<Vec<clientpb::Beacon>, String> {
    let response = conn
        .client
        .get_beacons(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("get_beacons failed: {e}"))?;

    Ok(response.into_inner().beacons)
}
