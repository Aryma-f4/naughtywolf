use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::actions::sessions;
use crate::sliver::connection::SliverConnection;

/// Generic report query params.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct ReportQuery {
    /// Optional ISO timestamp lower bound.
    pub since: Option<String>,
    /// Optional ISO timestamp upper bound.
    pub until: Option<String>,
    /// Cap on rows returned (default 1000).
    pub limit: Option<i64>,
    /// Export format — "json" (default) or "csv".
    pub format: Option<String>,
}

/// Single session row in a sessions report.
#[derive(Debug, Clone, Serialize)]
pub struct SessionReportRow {
    pub id: String,
    pub hostname: String,
    pub username: String,
    pub transport: String,
    pub remote_address: String,
    pub os: String,
    pub arch: String,
    pub is_dead: bool,
    pub last_checkin: i64,
}

/// Single credential row in a credentials report.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CredReportRow {
    pub id: String,
    pub cred_type: String,
    pub domain: String,
    pub username: String,
    pub host: String,
    pub is_cracked: bool,
    pub hash_type: Option<String>,
    pub source: String,
    pub created_at: String,
}

/// Single host row in a hosts report (aggregated from credentials).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct HostReportRow {
    pub hostname: String,
    pub agent_count: i64,
    pub last_seen: Option<String>,
}

/// Single timeline event (audit log entry).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TimelineRow {
    pub id: String,
    pub user_id: Option<String>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub result_status: String,
    pub created_at: String,
}

/// Pull current session list from the Sliver daemon and convert to report rows.
pub async fn report_sessions(
    conn: &mut SliverConnection,
    _q: ReportQuery,
) -> Result<Vec<SessionReportRow>, String> {
    let sessions = sessions::list_sessions(conn).await?;
    Ok(sessions
        .into_iter()
        .map(|s| SessionReportRow {
            id: s.id,
            hostname: s.hostname,
            username: s.username,
            transport: s.transport,
            remote_address: s.remote_address,
            os: s.os,
            arch: s.arch,
            is_dead: s.is_dead,
            last_checkin: s.last_checkin,
        })
        .collect())
}

pub async fn report_credentials(pool: &PgPool, q: ReportQuery) -> Result<Vec<CredReportRow>, String> {
    let limit = q.limit.unwrap_or(1000).clamp(1, 10_000);
    let since = q.since.unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    let until = q.until.unwrap_or_else(|| "9999-12-31T23:59:59Z".to_string());
    let rows = sqlx::query_as::<_, CredReportRow>(
        "SELECT id::text AS id, cred_type, domain, username, host, is_cracked, hash_type, source, created_at::text
         FROM credentials
         WHERE created_at >= $1::timestamptz AND created_at <= $2::timestamptz
         ORDER BY created_at DESC
         LIMIT $3",
    )
    .bind(&since)
    .bind(&until)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to query credentials report: {e}"))?;
    Ok(rows)
}

pub async fn report_hosts(pool: &PgPool, q: ReportQuery) -> Result<Vec<HostReportRow>, String> {
    let limit = q.limit.unwrap_or(1000).clamp(1, 10_000);
    let rows = sqlx::query_as::<_, HostReportRow>(
        "SELECT host AS hostname, COUNT(*)::bigint AS agent_count, MAX(created_at)::text AS last_seen
         FROM credentials
         WHERE host != ''
         GROUP BY host
         ORDER BY MAX(created_at) DESC
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to query hosts report: {e}"))?;
    Ok(rows)
}

pub async fn report_timeline(pool: &PgPool, q: ReportQuery) -> Result<Vec<TimelineRow>, String> {
    let limit = q.limit.unwrap_or(1000).clamp(1, 10_000);
    let since = q.since.unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    let until = q.until.unwrap_or_else(|| "9999-12-31T23:59:59Z".to_string());
    let rows = sqlx::query_as::<_, TimelineRow>(
        "SELECT id::text AS id, user_id::text AS user_id, action, target_type, target_id, result_status, created_at::text
         FROM audit_events
         WHERE created_at >= $1::timestamptz AND created_at <= $2::timestamptz
         ORDER BY created_at DESC
         LIMIT $3",
    )
    .bind(&since)
    .bind(&until)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to query timeline: {e}"))?;
    Ok(rows)
}

/// Serialize any report rows to CSV using serde_json reflection.
pub fn to_csv<T: Serialize>(rows: &[T]) -> Result<String, String> {
    let mut out = String::new();
    if rows.is_empty() {
        return Ok(out);
    }
    let v0 = serde_json::to_value(&rows[0]).map_err(|e| format!("CSV convert: {e}"))?;
    let arr = v0.as_object()
        .and_then(|o| o.values().next())
        .and_then(|v| v.as_object())
        .ok_or_else(|| "cannot infer columns".to_string())?;
    let header: Vec<String> = arr.keys().cloned().collect();
    out.push_str(&header.join(","));
    out.push('\n');
    for row in rows {
        let v = serde_json::to_value(row).map_err(|e| format!("CSV row: {e}"))?;
        let obj = v.as_object()
            .and_then(|o| o.values().next())
            .and_then(|v| v.as_object())
            .ok_or_else(|| "row not an object".to_string())?;
        let line: Vec<String> = header.iter().map(|k| csv_escape(obj.get(k))).collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    Ok(out)
}

fn csv_escape(v: Option<&serde_json::Value>) -> String {
    match v {
        None => String::new(),
        Some(serde_json::Value::String(s)) => {
            if s.contains(',') || s.contains('"') || s.contains('\n') {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.clone()
            }
        }
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Row {
        id: String,
        name: String,
        active: bool,
    }

    #[test]
    fn test_to_csv_basic() {
        let rows = vec![
            Row { id: "1".into(), name: "alice".into(), active: true },
            Row { id: "2".into(), name: "bob".into(), active: false },
        ];
        let csv = to_csv(&rows).unwrap();
        assert!(csv.contains("id,name,active"));
        assert!(csv.contains("alice"));
        assert!(csv.contains("false"));
    }

    #[test]
    fn test_csv_escape() {
        assert_eq!(csv_escape(None), "");
        assert_eq!(csv_escape(Some(&serde_json::json!("plain"))), "plain");
        assert_eq!(csv_escape(Some(&serde_json::json!("with,comma"))), "\"with,comma\"");
        assert_eq!(csv_escape(Some(&serde_json::json!("with\"quote"))), "\"with\"\"quote\"");
    }

    #[test]
    fn test_csv_empty_rows() {
        let empty: Vec<Row> = vec![];
        let csv = to_csv(&empty).unwrap();
        assert_eq!(csv, "");
    }
}

