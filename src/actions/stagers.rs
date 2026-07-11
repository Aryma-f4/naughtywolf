use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::actions::payloads;
use crate::db::models::StagerTemplate;

/// A simplified stager template exposed for API responses.
#[derive(Debug, Clone, Serialize)]
pub struct StagerResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub goos: String,
    pub goarch: String,
    pub format: i32,
    pub protocol: String,
    pub is_beacon: bool,
    pub obfuscate: bool,
    pub sample_count: i32,
}

impl From<StagerTemplate> for StagerResponse {
    fn from(t: StagerTemplate) -> Self {
        StagerResponse {
            id: t.id.to_string(),
            name: t.name,
            description: t.description,
            goos: t.goos,
            goarch: t.goarch,
            format: t.format,
            protocol: t.protocol,
            is_beacon: t.is_beacon,
            obfuscate: t.obfuscate,
            sample_count: t.sample_count,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateStagerRequest {
    pub name: String,
    pub description: Option<String>,
    pub goos: Option<String>,
    pub goarch: Option<String>,
    pub format: Option<i32>,
    pub protocol: Option<String>,
    pub is_beacon: Option<bool>,
    pub obfuscate: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateStagerRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub goos: Option<String>,
    pub goarch: Option<String>,
    pub format: Option<i32>,
    pub protocol: Option<String>,
    pub is_beacon: Option<bool>,
    pub obfuscate: Option<bool>,
}

pub async fn list_stagers(pool: &PgPool) -> Result<Vec<StagerResponse>, String> {
    let rows = sqlx::query_as::<_, StagerTemplate>(
        "SELECT id, name, description, goos, goarch, format, protocol, is_beacon, obfuscate, sample_count, created_at, updated_at FROM stager_templates ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to list stagers: {e}"))?;
    Ok(rows.into_iter().map(StagerResponse::from).collect())
}

pub async fn create_stager(pool: &PgPool, req: CreateStagerRequest) -> Result<StagerResponse, String> {
    let id = Uuid::new_v4();
    let desc = req.description.unwrap_or_default();
    let goos = req.goos.unwrap_or_else(|| "linux".to_string());
    let goarch = req.goarch.unwrap_or_else(|| "amd64".to_string());
    let format = req.format.unwrap_or(2);
    let protocol = req.protocol.unwrap_or_else(|| "mtls".to_string());
    let is_beacon = req.is_beacon.unwrap_or(false);
    let obfuscate = req.obfuscate.unwrap_or(true);

    let row = sqlx::query_as::<_, StagerTemplate>(
        r#"INSERT INTO stager_templates (id, name, description, goos, goarch, format, protocol, is_beacon, obfuscate)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
           RETURNING id, name, description, goos, goarch, format, protocol, is_beacon, obfuscate, sample_count, created_at, updated_at"#,
    )
    .bind(id)
    .bind(&req.name)
    .bind(&desc)
    .bind(&goos)
    .bind(&goarch)
    .bind(format)
    .bind(&protocol)
    .bind(is_beacon)
    .bind(obfuscate)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Failed to create stager: {e}"))?;
    Ok(StagerResponse::from(row))
}

pub async fn update_stager(pool: &PgPool, id: &str, req: UpdateStagerRequest) -> Result<StagerResponse, String> {
    let stager_id = Uuid::parse_str(id).map_err(|e| format!("Invalid stager id: {e}"))?;
    let row = sqlx::query_as::<_, StagerTemplate>(
        r#"UPDATE stager_templates SET
            name = COALESCE($2, name),
            description = COALESCE($3, description),
            goos = COALESCE($4, goos),
            goarch = COALESCE($5, goarch),
            format = COALESCE($6, format),
            protocol = COALESCE($7, protocol),
            is_beacon = COALESCE($8, is_beacon),
            obfuscate = COALESCE($9, obfuscate),
            updated_at = now()
           WHERE id = $1
           RETURNING id, name, description, goos, goarch, format, protocol, is_beacon, obfuscate, sample_count, created_at, updated_at"#,
    )
    .bind(stager_id)
    .bind(&req.name)
    .bind(&req.description)
    .bind(&req.goos)
    .bind(&req.goarch)
    .bind(req.format)
    .bind(&req.protocol)
    .bind(req.is_beacon)
    .bind(req.obfuscate)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Failed to update stager: {e}"))?;
    Ok(StagerResponse::from(row))
}

pub async fn delete_stager(pool: &PgPool, id: &str) -> Result<(), String> {
    let stager_id = Uuid::parse_str(id).map_err(|e| format!("Invalid stager id: {e}"))?;
    let res = sqlx::query("DELETE FROM stager_templates WHERE id = $1")
        .bind(stager_id)
        .execute(pool)
        .await
        .map_err(|e| format!("Failed to delete stager: {e}"))?;
    if res.rows_affected() == 0 {
        return Err(format!("Stager '{id}' not found"));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct GenerateFromStagerRequest {
    pub lhost: String,
    pub lport: u16,
    pub save_dir: Option<String>,
}

/// Generate an implant using a saved stager template.
pub async fn generate_from_stager(
    pool: &PgPool,
    stager_id: &str,
    req: GenerateFromStagerRequest,
) -> Result<payloads::GenerateResponse, String> {
    let id = Uuid::parse_str(stager_id).map_err(|e| format!("Invalid stager id: {e}"))?;
    let template = sqlx::query_as::<_, StagerTemplate>(
        "SELECT id, name, description, goos, goarch, format, protocol, is_beacon, obfuscate, sample_count, created_at, updated_at FROM stager_templates WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("Failed to load stager: {e}"))?;

    let save_dir = req.save_dir.unwrap_or_else(|| "./payloads".to_string());
    std::fs::create_dir_all(&save_dir).map_err(|e| format!("mkdir failed: {e}"))?;

    let rc_cmd = format!(
        "generate --name {n} --os {go} --arch {ga} --format {f} --{proto} {l}:{p} --save {d}\n",
        n = template.name,
        go = template.goos,
        ga = template.goarch,
        f = match template.format {
            0 => "shared",
            1 => "shellcode",
            3 => "service",
            _ => "exe",
        },
        proto = template.protocol,
        l = req.lhost,
        p = req.lport,
        d = save_dir,
    );

    let bin_raw = std::env::var("SLIVER_SERVER_PATH").unwrap_or_else(|_| "sliver-server".to_string());
    let parts: Vec<&str> = bin_raw.split_whitespace().collect();
    let (bin_cmd, bin_args) = parts.split_first().unwrap_or((&"sliver-server", &[]));

    let rc_path = format!("/tmp/stager_rc_{}.txt", std::process::id());
    let _ = std::fs::write(&rc_path, &rc_cmd);

    let output = tokio::process::Command::new(bin_cmd)
        .args(bin_args)
        .args(["--rc", &rc_path])
        .output()
        .await
        .map_err(|e| format!("sliver-server failed: {e}"))?;

    let _ = std::fs::remove_file(&rc_path);

    if output.status.success() {
        let dl = scan_save_dir(&template.name, std::path::Path::new(&save_dir));
        // Increment sample_count
        sqlx::query("UPDATE stager_templates SET sample_count = sample_count + 1 WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .ok();
        Ok(payloads::GenerateResponse {
            success: true,
            message: format!("Generated '{}' from stager template", template.name),
            implant_name: Some(template.name),
            output_path: dl.map(|f| format!("/api/payloads/download/{}", f)),
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(payloads::GenerateResponse {
            success: false,
            message: format!("sliver-server: {}", &stderr[..200.min(stderr.len())]),
            implant_name: None,
            output_path: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stager_response_from_template() {
        let t = StagerTemplate {
            id: Uuid::new_v4(),
            name: "linux-mtls".into(),
            description: "Linux mTLS stager".into(),
            goos: "linux".into(),
            goarch: "amd64".into(),
            format: 2,
            protocol: "mtls".into(),
            is_beacon: false,
            obfuscate: true,
            sample_count: 0,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let r = StagerResponse::from(t);
        assert_eq!(r.goos, "linux");
        assert_eq!(r.format, 2);
    }
}

/// Scan the save directory for a binary matching the given name prefix.
/// Returns the first filename that contains the name (for download URL).
fn scan_save_dir(name: &str, dir: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let fname = e.file_name().to_string_lossy().to_string();
        if (fname.contains(name) || fname == name)
            && e.metadata().map(|m| m.is_file() && m.len() > 0).unwrap_or(false)
        {
            return Some(fname);
        }
    }
    None
}
