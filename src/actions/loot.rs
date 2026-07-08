use serde::Serialize;

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::commonpb;

/// A simplified loot representation for API responses.
#[derive(Debug, Clone, Serialize)]
pub struct LootResponse {
    pub id: String,
    pub name: String,
    pub file_type: String,
    pub size: i64,
}

fn file_type_name(t: i32) -> &'static str {
    match t {
        0 => "binary",
        1 => "text_file",
        2 => "credential",
        _ => "unknown",
    }
}

/// Fetch all loot from the Sliver server.
pub async fn list_loot(conn: &mut SliverConnection) -> Result<Vec<LootResponse>, String> {
    let response = conn
        .client
        .loot_all(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("loot_all failed: {e}"))?;

    let all = response.into_inner();

    Ok(all
        .loot
        .into_iter()
        .map(|l| LootResponse {
            id: l.id,
            name: l.name,
            file_type: file_type_name(l.file_type).to_string(),
            size: l.size,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loot_response_struct() {
        let r = LootResponse {
            id: "loot1".into(),
            name: "config.yaml".into(),
            file_type: "text_file".into(),
            size: 1024,
        };
        assert_eq!(r.id, "loot1");
        assert_eq!(r.name, "config.yaml");
        assert_eq!(r.file_type, "text_file");
        assert_eq!(r.size, 1024);
    }

    #[test]
    fn test_file_type_name() {
        assert_eq!(file_type_name(0), "binary");
        assert_eq!(file_type_name(1), "text_file");
        assert_eq!(file_type_name(2), "credential");
        assert_eq!(file_type_name(99), "unknown");
    }
}
