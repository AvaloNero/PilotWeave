//! Read-only Copilot SDK protocol 3 RPC over a private stdio connection.
//! Never creates/resumes a session, sends a prompt, or retrieves client tokens.
use super::{bounded_id, db_error, store, types::DataStatus};
use crate::{
    decimal::ExactDecimal, error::AppResult, native_process, safe_file, usage_db::UsageDb,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaItem {
    pub key: String,
    pub unlimited: bool,
    pub entitlement: Option<ExactDecimal>,
    pub used: Option<ExactDecimal>,
    pub remaining_percentage: Option<ExactDecimal>,
    pub reset_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeModel {
    pub id: String,
    pub name: String,
    pub policy: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub id: String,
    pub status: DataStatus,
    pub detail: String,
    pub account: Option<String>,
    pub runtime_version: Option<String>,
    pub parser_version: u32,
    pub fetched_at: DateTime<Utc>,
    pub quotas: Vec<QuotaItem>,
    pub models: Vec<RuntimeModel>,
    pub model_status: DataStatus,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeView {
    pub latest: Option<RuntimeSnapshot>,
    pub last_successful: Option<RuntimeSnapshot>,
    pub stale: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeError(pub DataStatus, pub &'static str);
pub type RpcResult<T> = Result<T, RuntimeError>;
pub trait ReadOnlyRpc {
    fn request(&mut self, method: &'static str, params: Value) -> RpcResult<Value>;
}

fn schema() -> RuntimeError {
    RuntimeError(
        DataStatus::SchemaError,
        "Copilot runtime returned an unsupported schema",
    )
}

pub fn snapshot(rpc: &mut impl ReadOnlyRpc) -> RuntimeSnapshot {
    let mut result = RuntimeSnapshot {
        id: Uuid::new_v4().to_string(),
        status: DataStatus::Unavailable,
        detail: String::new(),
        account: None,
        runtime_version: None,
        parser_version: 1,
        fetched_at: Utc::now(),
        quotas: vec![],
        models: vec![],
        model_status: DataStatus::Unavailable,
    };
    let operation = (|| -> RpcResult<()> {
        let hello = rpc.request(
            "connect",
            json!({"enableGitHubTelemetryForwarding":false,"supportedTaskKinds":[]}),
        )?;
        if hello["protocolVersion"].as_u64() != Some(3) {
            return Err(RuntimeError(DataStatus::Unsupported,"This build supports Copilot SDK protocol 3; update the client or use its own /usage view"));
        }
        result.runtime_version =
            Some(bounded_id(hello["version"].as_str().ok_or_else(schema)?).map_err(|_| schema())?);
        let auth = rpc.request("auth.getStatus", json!({}))?;
        if !auth["isAuthenticated"].as_bool().ok_or_else(schema)? {
            return Err(RuntimeError(
                DataStatus::Unauthorized,
                "Sign in with the official Copilot CLI, then refresh runtime quota",
            ));
        }
        let host = auth["host"].as_str().ok_or_else(schema)?;
        if !matches!(
            host,
            "github.com" | "https://github.com" | "https://github.com/"
        ) {
            return Err(RuntimeError(
                DataStatus::Unsupported,
                "Runtime quota supports github.com accounts only",
            ));
        }
        let login = bounded_id(auth["login"].as_str().ok_or_else(schema)?).map_err(|_| schema())?;
        result.account = Some(format!("{login}@github.com"));
        let quota_result = rpc.request("account.getQuota", json!({}));
        // Model availability remains independent from quota retrieval.
        match rpc
            .request("models.list", json!({}))
            .and_then(|v| parse_models(&v))
        {
            Ok(models) => {
                result.model_status = if models.is_empty() {
                    DataStatus::SuccessfulEmpty
                } else {
                    DataStatus::Available
                };
                result.models = models;
            }
            Err(e) => result.model_status = e.0,
        }
        result.quotas = parse_quotas(&quota_result?)?;
        result.status = if result.quotas.is_empty() {
            DataStatus::SuccessfulEmpty
        } else {
            DataStatus::Available
        };
        result.detail="Official Copilot runtime quota. Values retain their quota key; this is separate from personal Billing and local token totals.".into();
        Ok(())
    })();
    if let Err(e) = operation {
        result.status = e.0;
        result.detail = e.1.into();
    }
    result
}

fn decimal(v: Option<&Value>, negative: bool) -> RpcResult<Option<ExactDecimal>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let text = match v {
                Value::Number(n) => n.to_string(),
                Value::String(s) if s.len() <= 64 => s.clone(),
                _ => return Err(schema()),
            };
            let d = ExactDecimal::parse(&text).ok_or_else(schema)?;
            if (!negative && d.get() < Decimal::ZERO)
                || d.get().abs() > Decimal::from(1_000_000_000_000u64)
            {
                return Err(schema());
            }
            Ok(Some(d))
        }
    }
}

pub fn parse_quotas(value: &Value) -> RpcResult<Vec<QuotaItem>> {
    let items = value["quotaSnapshots"]
        .as_object()
        .filter(|v| v.len() <= 64)
        .ok_or_else(schema)?;
    items
        .iter()
        .map(|(key, v)| {
            let key = bounded_id(key).map_err(|_| schema())?;
            let unlimited = v["isUnlimitedEntitlement"].as_bool().ok_or_else(schema)?;
            let entitlement = decimal(v.get("entitlementRequests"), true)?;
            if entitlement.is_some_and(|e| {
                e.get() < Decimal::ZERO && (e.get() != Decimal::from(-1) || !unlimited)
            }) {
                return Err(schema());
            }
            let percent = decimal(v.get("remainingPercentage"), false)?;
            if percent.is_some_and(|p| p.get() > Decimal::from(100)) {
                return Err(schema());
            }
            let reset_at = v
                .get("resetDate")
                .filter(|v| !v.is_null())
                .map(|v| {
                    v.as_str()
                        .filter(|s| {
                            DateTime::parse_from_rfc3339(s).is_ok()
                                || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
                        })
                        .map(str::to_string)
                        .ok_or_else(schema)
                })
                .transpose()?;
            Ok(QuotaItem {
                key,
                unlimited,
                entitlement: entitlement.filter(|v| v.get() >= Decimal::ZERO),
                used: decimal(v.get("usedRequests"), false)?,
                remaining_percentage: percent,
                reset_at,
            })
        })
        .collect()
}

fn parse_models(v: &Value) -> RpcResult<Vec<RuntimeModel>> {
    let rows = v["models"]
        .as_array()
        .filter(|a| a.len() <= 512)
        .ok_or_else(schema)?;
    rows.iter()
        .map(|m| {
            Ok(RuntimeModel {
                id: super::bounded_model_id(m["id"].as_str().ok_or_else(schema)?)
                    .map_err(|_| schema())?,
                name: crate::redact::redact_text(
                    m["name"]
                        .as_str()
                        .filter(|s| s.len() <= 160)
                        .ok_or_else(schema)?,
                ),
                policy: m
                    .get("policy")
                    .and_then(|p| p.get("state"))
                    .map(|v| bounded_id(v.as_str().ok_or_else(schema)?).map_err(|_| schema()))
                    .transpose()?,
            })
        })
        .collect()
}

pub fn save(db: &mut UsageDb, s: &RuntimeSnapshot) -> AppResult<()> {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        crate::test_support::fault("runtime-save")?;
    }

    let tx = db.conn.transaction().map_err(db_error)?;
    tx.execute("INSERT INTO official_quota_snapshots(id,account_hint,runtime_version,parser_version,fetched_at,status,error) VALUES (?1,?2,?3,1,?4,?5,?6)",params![s.id,s.account,s.runtime_version,s.fetched_at.to_rfc3339(),store::encode(&s.status)?,(!s.status.successful()).then_some(&s.detail)]).map_err(db_error)?;
    tx.execute(
        "INSERT INTO runtime_usage_payloads VALUES (?1,?2)",
        params![s.id, store::encode(s)?],
    )
    .map_err(db_error)?;
    for q in &s.quotas {
        tx.execute(
            "INSERT INTO official_quota_items VALUES (?1,?2,?3,?4)",
            params![s.id, q.key, store::encode(q)?, format!("runtime:{}", q.key)],
        )
        .map_err(db_error)?;
    }
    // Keep successful snapshots even across a long sequence of failures.
    tx.execute("DELETE FROM official_quota_snapshots WHERE id NOT IN (SELECT id FROM official_quota_snapshots ORDER BY fetched_at DESC,rowid DESC LIMIT 100) AND id NOT IN (SELECT id FROM official_quota_snapshots WHERE status IN ('\"available\"','\"successfulEmpty\"') ORDER BY fetched_at DESC,rowid DESC LIMIT 100)",[]).map_err(db_error)?;
    tx.commit().map_err(db_error)
}

pub fn view(db: &UsageDb) -> AppResult<RuntimeView> {
    let latest:Option<RuntimeSnapshot>=db.conn.query_row("SELECT p.payload FROM runtime_usage_payloads p JOIN official_quota_snapshots s ON s.id=p.snapshot_id ORDER BY s.fetched_at DESC,s.rowid DESC LIMIT 1",[],|r|r.get(0)).optional().map_err(db_error)?.map(store::decode).transpose()?;
    let stale = latest.as_ref().is_some_and(|s| {
        !s.status.successful() || Utc::now() - s.fetched_at > chrono::Duration::minutes(15)
    });
    let last_successful = if latest.as_ref().is_some_and(|s| !s.status.successful()) {
        let account = latest.as_ref().and_then(|s| s.account.as_ref());
        db.conn.query_row("SELECT p.payload FROM runtime_usage_payloads p JOIN official_quota_snapshots s ON s.id=p.snapshot_id WHERE s.status IN ('\"available\"','\"successfulEmpty\"') AND (?1 IS NULL OR s.account_hint=?1) ORDER BY s.fetched_at DESC,s.rowid DESC LIMIT 1",[account],|r|r.get(0)).optional().map_err(db_error)?.map(store::decode).transpose()?
    } else {
        None
    };
    Ok(RuntimeView {
        latest,
        last_successful,
        stale,
    })
}

pub fn refresh(cancel: &AtomicBool) -> RuntimeSnapshot {
    #[cfg(feature = "local-e2e")]
    if crate::test_support::active() {
        return snapshot(&mut crate::test_support::Rpc);
    }

    match StdioRpc::start(cancel) {
        Ok(mut rpc) => snapshot(&mut rpc),
        Err(e) => RuntimeSnapshot {
            id: Uuid::new_v4().to_string(),
            status: e.0,
            detail: e.1.into(),
            account: None,
            runtime_version: None,
            parser_version: 1,
            fetched_at: Utc::now(),
            quotas: vec![],
            models: vec![],
            model_status: DataStatus::Unavailable,
        },
    }
}

struct StdioRpc<'a> {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<RpcResult<Value>>,
    next_id: u32,
    deadline: Instant,
    cancel: &'a AtomicBool,
}
impl<'a> StdioRpc<'a> {
    fn start(cancel: &'a AtomicBool) -> RpcResult<Self> {
        let executable = native_process::resolve_on_path(if cfg!(windows) {
            &["copilot.exe"]
        } else {
            &["copilot"]
        })
        .ok_or(RuntimeError(
            DataStatus::Missing,
            "A native Copilot CLI executable was not found; npm shell wrappers are not executed",
        ))?;
        safe_file::ensure_regular_or_missing(&executable).map_err(|_| {
            RuntimeError(
                DataStatus::Unsupported,
                "Copilot runtime path failed regular-file checks",
            )
        })?;
        let mut command = Command::new(executable);
        command
            .args([
                "--headless",
                "--no-auto-update",
                "--stdio",
                "--log-level",
                "error",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        native_process::sanitize_child_environment(&mut command, false);
        command.env("COPILOT_OTEL_ENABLED", "false");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|_| {
            RuntimeError(
                DataStatus::Unavailable,
                "Copilot runtime could not be started",
            )
        })?;
        let stdin = child.stdin.take().ok_or_else(schema)?;
        let stdout = child.stdout.take().ok_or_else(schema)?;
        let (tx, rx) = mpsc::sync_channel(4);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            for _ in 0..128 {
                let packet = read_frame(&mut reader);
                let failed = packet.is_err();
                if tx.send(packet).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            rx,
            next_id: 0,
            deadline: Instant::now() + Duration::from_secs(35),
            cancel,
        })
    }
}
impl Drop for StdioRpc<'_> {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl ReadOnlyRpc for StdioRpc<'_> {
    fn request(&mut self, method: &'static str, params: Value) -> RpcResult<Value> {
        if !matches!(
            method,
            "connect" | "auth.getStatus" | "account.getQuota" | "models.list"
        ) {
            return Err(schema());
        }
        self.next_id += 1;
        let id = self.next_id;
        let body =
            serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .map_err(|_| schema())?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())
            .and_then(|_| self.stdin.write_all(&body))
            .and_then(|_| self.stdin.flush())
            .map_err(|_| RuntimeError(DataStatus::Unavailable, "Copilot RPC connection closed"))?;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(RuntimeError(
                    DataStatus::Canceled,
                    "Runtime refresh canceled",
                ));
            }
            if Instant::now() >= self.deadline {
                return Err(RuntimeError(
                    DataStatus::NetworkError,
                    "Copilot quota refresh timed out; last successful data retained",
                ));
            }
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(packet) => {
                    let packet = packet?;
                    if packet.get("method").is_some() {
                        continue;
                    }
                    if packet["id"].as_u64() != Some(id.into()) {
                        return Err(schema());
                    }
                    if let Some(error) = packet.get("error") {
                        return Err(if error["code"].as_i64() == Some(-32601) {
                            RuntimeError(
                                DataStatus::Unsupported,
                                "This runtime does not implement the official quota/model RPC",
                            )
                        } else {
                            RuntimeError(DataStatus::Unavailable,"Copilot rejected the read-only RPC; inspect the official client and refresh")
                        });
                    }
                    return packet.get("result").cloned().ok_or_else(schema);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => {
                    return Err(RuntimeError(
                        DataStatus::Unavailable,
                        "Copilot runtime disconnected",
                    ))
                }
            }
        }
    }
}

pub fn read_frame(reader: &mut impl BufRead) -> RpcResult<Value> {
    let mut length = None;
    let mut header_bytes = 0;
    loop {
        let mut line = Vec::new();
        let n = (&mut *reader)
            .take(8193)
            .read_until(b'\n', &mut line)
            .map_err(|_| schema())?;
        header_bytes += n;
        if n == 0 || header_bytes > 8192 {
            return Err(schema());
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        let line = std::str::from_utf8(&line).map_err(|_| schema())?;
        if let Some(n) = line.strip_prefix("Content-Length:") {
            if length.is_some() {
                return Err(schema());
            }
            length = Some(
                n.trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n <= 2 * 1024 * 1024)
                    .ok_or_else(schema)?,
            );
        }
    }
    let mut body = vec![0; length.ok_or_else(schema)?];
    reader.read_exact(&mut body).map_err(|_| schema())?;
    serde_json::from_slice(&body).map_err(|_| schema())
}
