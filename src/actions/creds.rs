use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::commonpb;

/// A credential for API responses. Combines Sliver gRPC creds + local DB creds.
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

// ── Local DB-backed credential CRUD (Empire-style) ────────

#[derive(Debug, Deserialize)]
pub struct CreateCredRequest {
    pub cred_type: Option<String>,
    pub domain: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub host: Option<String>,
    pub os: Option<String>,
    pub sid: Option<String>,
    pub notes: Option<String>,
    pub source: Option<String>,
    pub agent_id: Option<String>,
    pub hash: Option<String>,
    pub hash_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCredRequest {
    pub cred_type: Option<String>,
    pub domain: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub host: Option<String>,
    pub notes: Option<String>,
    pub hash: Option<String>,
    pub hash_type: Option<String>,
    pub is_cracked: Option<bool>,
}

fn str_field(v: Option<String>) -> String {
    v.unwrap_or_default()
}

fn map_db_cred(c: &crate::db::models::Credential) -> CredResponse {
    CredResponse {
        id: c.id.to_string(),
        username: format!("{}@{}", c.username, c.domain),
        plaintext: c.password.clone(),
        hash: c.hash.clone().unwrap_or_default(),
        hash_type: c.hash_type.clone().unwrap_or_default(),
        is_cracked: c.is_cracked,
        collection: c.source.clone(),
    }
}

pub async fn list_local_creds(pool: &PgPool) -> Result<Vec<CredResponse>, String> {
    let rows = sqlx::query_as::<_, crate::db::models::Credential>(
        "SELECT id, cred_type, domain, username, password, host, os, sid, notes, source, agent_id, is_cracked, hash, hash_type, created_at, updated_at FROM credentials ORDER BY created_at DESC"
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to list credentials: {e}"))?;
    Ok(rows.iter().map(map_db_cred).collect())
}

pub async fn create_cred(pool: &PgPool, req: CreateCredRequest) -> Result<CredResponse, String> {
    let id = Uuid::new_v4();
    let cred_type = str_field(req.cred_type.or_else(|| Some("plaintext".to_string())));
    let domain = str_field(req.domain);
    let username = str_field(req.username);
    let password = str_field(req.password);
    let host = str_field(req.host);
    let os = str_field(req.os);
    let sid = str_field(req.sid);
    let notes = str_field(req.notes);
    let source = str_field(req.source.or_else(|| Some("manual".to_string())));
    let agent_id = req.agent_id;
    let hash = req.hash;
    let hash_type = req.hash_type;

    let row = sqlx::query_as::<_, crate::db::models::Credential>(
        r#"INSERT INTO credentials (id, cred_type, domain, username, password, host, os, sid, notes, source, agent_id, hash, hash_type, is_cracked)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, false)
           RETURNING id, cred_type, domain, username, password, host, os, sid, notes, source, agent_id, is_cracked, hash, hash_type, created_at, updated_at"#,
    )
    .bind(id)
    .bind(cred_type)
    .bind(domain)
    .bind(username)
    .bind(password)
    .bind(host)
    .bind(os)
    .bind(sid)
    .bind(notes)
    .bind(source)
    .bind(agent_id)
    .bind(hash)
    .bind(hash_type)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Failed to create credential: {e}"))?;
    Ok(map_db_cred(&row))
}

pub async fn update_cred(pool: &PgPool, id: &str, req: UpdateCredRequest) -> Result<CredResponse, String> {
    let cred_id = Uuid::parse_str(id).map_err(|e| format!("Invalid credential id: {e}"))?;
    let cred_type = req.cred_type;
    let domain = req.domain;
    let username = req.username;
    let password = req.password;
    let host = req.host;
    let notes = req.notes;
    let hash = req.hash;
    let hash_type = req.hash_type;
    let is_cracked = req.is_cracked;

    let row = sqlx::query_as::<_, crate::db::models::Credential>(
        r#"UPDATE credentials SET
            cred_type = COALESCE($2, cred_type),
            domain = COALESCE($3, domain),
            username = COALESCE($4, username),
            password = COALESCE($5, password),
            host = COALESCE($6, host),
            notes = COALESCE($7, notes),
            hash = COALESCE($8, hash),
            hash_type = COALESCE($9, hash_type),
            is_cracked = COALESCE($10, is_cracked),
            updated_at = now()
           WHERE id = $1
           RETURNING id, cred_type, domain, username, password, host, os, sid, notes, source, agent_id, is_cracked, hash, hash_type, created_at, updated_at"#,
    )
    .bind(cred_id)
    .bind(cred_type)
    .bind(domain)
    .bind(username)
    .bind(password)
    .bind(host)
    .bind(notes)
    .bind(hash)
    .bind(hash_type)
    .bind(is_cracked)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Failed to update credential: {e}"))?;
    Ok(map_db_cred(&row))
}

pub async fn delete_cred(pool: &PgPool, id: &str) -> Result<(), String> {
    let cred_id = Uuid::parse_str(id).map_err(|e| format!("Invalid credential id: {e}"))?;
    let res = sqlx::query("DELETE FROM credentials WHERE id = $1")
        .bind(cred_id)
        .execute(pool)
        .await
        .map_err(|e| format!("Failed to delete credential: {e}"))?;
    if res.rows_affected() == 0 {
        return Err(format!("Credential '{id}' not found"));
    }
    Ok(())
}

pub async fn mark_cracked(pool: &PgPool, id: &str, plaintext: &str) -> Result<CredResponse, String> {
    let cred_id = Uuid::parse_str(id).map_err(|e| format!("Invalid credential id: {e}"))?;
    let row = sqlx::query_as::<_, crate::db::models::Credential>(
        r#"UPDATE credentials SET password = $2, is_cracked = true, updated_at = now()
           WHERE id = $1
           RETURNING id, cred_type, domain, username, password, host, os, sid, notes, source, agent_id, is_cracked, hash, hash_type, created_at, updated_at"#,
    )
    .bind(cred_id)
    .bind(plaintext)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Failed to mark cracked: {e}"))?;
    Ok(map_db_cred(&row))
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

    #[test]
    fn test_str_field() {
        assert_eq!(str_field(Some("foo".to_string())), "foo");
        assert_eq!(str_field(None), "");
        assert_eq!(str_field(Some("".to_string())), "");
    }
}
