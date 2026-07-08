use serde::Serialize;

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::commonpb;

/// A simplified credential representation for API responses.
#[derive(Debug, Clone, Serialize)]
pub struct CredResponse {
    pub id: String,
    pub username: String,
    pub plaintext: String,
    pub hash: String,
    pub hash_type: String,
    pub is_cracked: bool,
    pub collection: String,
}

fn hash_type_name(t: i32) -> &'static str {
    match t {
        0 => "md5",
        1 => "sha1",
        2 => "sha256",
        3 => "sha512",
        4 => "bcrypt",
        5 => "lm",
        6 => "ntlm",
        _ => "unknown",
    }
}

/// Fetch all credentials from the Sliver server.
pub async fn list_creds(conn: &mut SliverConnection) -> Result<Vec<CredResponse>, String> {
    let response = conn
        .client
        .creds(tonic::Request::new(commonpb::Empty {}))
        .await
        .map_err(|e| format!("creds failed: {e}"))?;

    let all = response.into_inner();

    Ok(all
        .credentials
        .into_iter()
        .map(|c| CredResponse {
            id: c.id,
            username: c.username,
            plaintext: c.plaintext,
            hash: c.hash,
            hash_type: hash_type_name(c.hash_type).to_string(),
            is_cracked: c.is_cracked,
            collection: c.collection,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cred_response_struct() {
        let r = CredResponse {
            id: "cred1".into(),
            username: "admin".into(),
            plaintext: "".into(),
            hash: "5f4dcc3b5aa765d61d8327deb882cf99".into(),
            hash_type: "md5".into(),
            is_cracked: true,
            collection: "default".into(),
        };
        assert_eq!(r.id, "cred1");
        assert_eq!(r.username, "admin");
        assert_eq!(r.hash_type, "md5");
        assert!(r.is_cracked);
        assert_eq!(r.collection, "default");
    }

    #[test]
    fn test_hash_type_name() {
        assert_eq!(hash_type_name(0), "md5");
        assert_eq!(hash_type_name(1), "sha1");
        assert_eq!(hash_type_name(2), "sha256");
        assert_eq!(hash_type_name(3), "sha512");
        assert_eq!(hash_type_name(4), "bcrypt");
        assert_eq!(hash_type_name(5), "lm");
        assert_eq!(hash_type_name(6), "ntlm");
        assert_eq!(hash_type_name(99), "unknown");
    }
}
