//! Minimal Supabase (PostgREST) insert client shared by telemetry and the
//! dataset recorder. Credentials come from the environment:
//! `ABLETON_MCP_SUPABASE_URL` and `ABLETON_MCP_SUPABASE_ANON_KEY`.

use serde_json::Value;
use std::time::Duration;

#[derive(Debug)]
pub enum RowError {
    /// Postgres unique violation (23505): the row is already there.
    Duplicate,
    Other(String),
}

pub struct SupabaseClient {
    url: String,
    key: String,
    session_id: Option<String>,
    agent: ureq::Agent,
}

impl SupabaseClient {
    pub fn new(url: &str, key: &str, session_id: Option<&str>, timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build();
        Self {
            url: url.trim_end_matches('/').to_string(),
            key: key.to_string(),
            session_id: session_id.map(str::to_string),
            agent: config.into(),
        }
    }

    /// `POST /rest/v1/{table}` with `Prefer: return=minimal`. The anon key
    /// holds INSERT only, so insert is the only operation offered.
    pub fn insert(&self, table: &str, row: &Value) -> Result<(), RowError> {
        let endpoint = format!("{}/rest/v1/{}", self.url, table);
        let mut request = self
            .agent
            .post(&endpoint)
            .header("apikey", &self.key)
            .header("Authorization", &format!("Bearer {}", self.key))
            .header("Prefer", "return=minimal");
        if let Some(sid) = &self.session_id {
            request = request.header("x-session-id", sid);
        }
        let mut response = request
            .send_json(row)
            .map_err(|e| RowError::Other(e.to_string()))?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(());
        }
        let body = response.body_mut().read_to_string().unwrap_or_default();
        if status == 409 || body.contains("23505") || body.to_lowercase().contains("duplicate key")
        {
            return Err(RowError::Duplicate);
        }
        Err(RowError::Other(format!(
            "HTTP {status}: {}",
            body.chars().take(300).collect::<String>()
        )))
    }
}

/// Build a client for dataset writes. `session_id` is sent as the
/// `x-session-id` header; RLS uses it to scope writes to the caller's own
/// session.
pub fn create_supabase_client(session_id: Option<&str>) -> Option<SupabaseClient> {
    let config = crate::telemetry::TelemetryConfig::from_env();
    if !config.has_credentials() {
        tracing::warn!("Dataset: Supabase credentials not configured");
        return None;
    }
    Some(SupabaseClient::new(
        &config.supabase_url,
        &config.supabase_anon_key,
        session_id,
        Duration::from_secs(10),
    ))
}

/// Reuse the anonymous customer UUID from product telemetry.
pub fn get_customer_uuid() -> Option<String> {
    Some(
        crate::telemetry::get_telemetry()
            .customer_uuid()
            .to_string(),
    )
}
