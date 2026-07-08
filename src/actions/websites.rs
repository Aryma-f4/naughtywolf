use serde::Serialize;

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::commonpb;

/// A simplified website representation for API responses.
#[derive(Debug, Clone, Serialize)]
pub struct WebsiteResponse {
    pub id: String,
    pub name: String,
    pub content_count: usize,
    pub total_size: u64,
}

/// Fetch all websites from the Sliver server.
pub async fn list_websites(
    conn: &mut SliverConnection,
) -> Result<Vec<WebsiteResponse>, String> {
    let response = conn
        .client
        .websites(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("websites failed: {e}"))?;

    let sites = response.into_inner();

    Ok(sites
        .websites
        .into_iter()
        .map(|w| {
            let total_size: u64 = w.contents.values().map(|c| c.size).sum();
            WebsiteResponse {
                id: w.id,
                name: w.name,
                content_count: w.contents.len(),
                total_size,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_website_response_struct() {
        let r = WebsiteResponse {
            id: "abc123".into(),
            name: "My Site".into(),
            content_count: 3,
            total_size: 4096,
        };
        assert_eq!(r.id, "abc123");
        assert_eq!(r.name, "My Site");
        assert_eq!(r.content_count, 3);
        assert_eq!(r.total_size, 4096);
    }
}
