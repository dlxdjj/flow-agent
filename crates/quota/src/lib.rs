//! Privacy-bounded quota adapters with explicit schema and freshness gates.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;
use thiserror::Error;

const CACHE_SCHEMA_VERSION: u32 = 1;
const CLAUDE_SOURCE: &str = "statusline";
const CODEX_SOURCE: &str = "rollout_experimental";
const FRESH_FOR_MS: u64 = 30 * 60 * 1_000;
const MAX_CLOCK_SKEW_MS: u64 = 5 * 60 * 1_000;
const MAX_STATUSLINE_BYTES: u64 = 256 * 1_024;
const MAX_ROLLOUT_TAIL_BYTES: u64 = 2 * 1_024 * 1_024;
const MAX_SESSION_META_BYTES: u64 = 128 * 1_024;
const MAX_ROLLOUT_FILES: usize = 256;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum QuotaError {
    #[error("quota input exceeds {0} bytes")]
    TooLarge(u64),
    #[error("unsafe symbolic link refused: {0}")]
    SymlinkRefused(PathBuf),
    #[error("quota JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("quota I/O failed for {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaEntry {
    pub provider: String,
    pub window: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl QuotaEntry {
    fn available(
        provider: &str,
        window: impl Into<String>,
        used_pct: f64,
        resets_at: u64,
        source: &str,
        captured_at: u64,
    ) -> Self {
        let used_pct = used_pct.clamp(0.0, 100.0);
        Self {
            provider: provider.to_owned(),
            window: window.into(),
            status: "available".to_owned(),
            used_pct: Some(used_pct),
            remaining_pct: Some(100.0 - used_pct),
            resets_at: Some(resets_at),
            source: source.to_owned(),
            window_minutes: None,
            limit_id: None,
            limit_name: None,
            plan_type: None,
            captured_at: Some(captured_at),
            reason: None,
        }
    }

    fn unavailable(provider: &str, window: &str, source: &str, reason: impl Into<String>) -> Self {
        Self {
            provider: provider.to_owned(),
            window: window.to_owned(),
            status: "unavailable".to_owned(),
            used_pct: None,
            remaining_pct: None,
            resets_at: None,
            source: source.to_owned(),
            window_minutes: None,
            limit_id: None,
            limit_name: None,
            plan_type: None,
            captured_at: None,
            reason: Some(reason.into()),
        }
    }

    fn with_metadata(
        mut self,
        window_minutes: Option<u64>,
        limit_id: Option<String>,
        limit_name: Option<String>,
        plan_type: Option<String>,
    ) -> Self {
        self.window_minutes = window_minutes;
        self.limit_id = limit_id;
        self.limit_name = limit_name;
        self.plan_type = plan_type;
        self
    }

    fn mark_stale(mut self, now_ms: u64) -> Self {
        let captured_at = self.captured_at.unwrap_or_default();
        let minutes = now_ms.saturating_sub(captured_at) / 60_000;
        self.status = "stale".to_owned();
        self.reason = Some(format!("最后一次有效数据（{minutes} 分钟前）"));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaPaths {
    pub flow_home: PathBuf,
    pub codex_sessions: PathBuf,
}

impl QuotaPaths {
    pub fn discover() -> Self {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let flow_home = env::var_os("FLOW_AGENT_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".flow-agent"));
        let codex_home = env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        Self {
            flow_home,
            codex_sessions: codex_home.join("sessions"),
        }
    }

    pub fn claude_cache(&self) -> PathBuf {
        self.flow_home.join("cache/claude-rl.json")
    }
}

#[derive(Debug, Clone)]
pub struct QuotaCollector {
    paths: QuotaPaths,
}

impl QuotaCollector {
    pub fn new(paths: QuotaPaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &QuotaPaths {
        &self.paths
    }

    pub fn collect(&self, now_ms: u64) -> Vec<QuotaEntry> {
        let mut entries = self.collect_claude(now_ms);
        entries.extend(self.collect_codex(now_ms));
        entries
    }

    pub fn collect_claude(&self, now_ms: u64) -> Vec<QuotaEntry> {
        let path = self.paths.claude_cache();
        let bytes = match read_bounded(&path, MAX_STATUSLINE_BYTES) {
            Ok(bytes) => bytes,
            Err(QuotaError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                return unavailable_windows(
                    "claude",
                    CLAUDE_SOURCE,
                    &["5h", "7d"],
                    "额度缓存不存在；请开启 Claude 额度桥并完成一次对话",
                )
            }
            Err(error) => {
                return unavailable_windows(
                    "claude",
                    CLAUDE_SOURCE,
                    &["5h", "7d"],
                    format!("额度缓存不可读：{error}"),
                )
            }
        };
        let cache = match serde_json::from_slice::<CacheDocument>(&bytes) {
            Ok(cache)
                if cache.schema_version == CACHE_SCHEMA_VERSION
                    && cache.provider == "claude"
                    && cache.source == CLAUDE_SOURCE =>
            {
                cache
            }
            Ok(_) => {
                return unavailable_windows(
                    "claude",
                    CLAUDE_SOURCE,
                    &["5h", "7d"],
                    "额度缓存版本不兼容",
                )
            }
            Err(_) => {
                return unavailable_windows(
                    "claude",
                    CLAUDE_SOURCE,
                    &["5h", "7d"],
                    "额度缓存解析失败",
                )
            }
        };
        if cache.captured_at > now_ms.saturating_add(MAX_CLOCK_SKEW_MS) {
            return unavailable_windows(
                "claude",
                CLAUDE_SOURCE,
                &["5h", "7d"],
                "额度缓存时间晚于本机时间",
            );
        }
        let stale = now_ms.saturating_sub(cache.captured_at) > FRESH_FOR_MS;
        let entries = cache
            .windows
            .into_iter()
            .filter(|window| {
                !window.window.is_empty()
                    && window.used_pct.is_finite()
                    && (0.0..=100.0).contains(&window.used_pct)
                    && window.resets_at > 0
            })
            .map(|window| {
                let entry = QuotaEntry::available(
                    "claude",
                    window.window,
                    window.used_pct,
                    window.resets_at,
                    CLAUDE_SOURCE,
                    cache.captured_at,
                )
                .with_metadata(window.window_minutes, None, window.label, None);
                if stale {
                    entry.mark_stale(now_ms)
                } else {
                    entry
                }
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            unavailable_windows(
                "claude",
                CLAUDE_SOURCE,
                &["5h", "7d"],
                "额度缓存没有可验证窗口",
            )
        } else {
            entries
        }
    }

    pub fn collect_codex(&self, now_ms: u64) -> Vec<QuotaEntry> {
        let mut files = Vec::new();
        collect_rollouts(&self.paths.codex_sessions, 0, &mut files);
        files.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
        if files.is_empty() {
            return vec![QuotaEntry::unavailable(
                "codex",
                "unknown",
                CODEX_SOURCE,
                "未找到 Codex rollout 文件",
            )];
        }
        for (path, modified_at) in files {
            if modified_at > now_ms.saturating_add(MAX_CLOCK_SKEW_MS)
                || read_codex_version(&path).ok().flatten().is_none()
            {
                continue;
            }
            let Ok(mut entries) = read_codex_limits(&path, modified_at) else {
                continue;
            };
            if entries.is_empty() {
                continue;
            }
            if now_ms.saturating_sub(modified_at) > FRESH_FOR_MS {
                entries = entries
                    .into_iter()
                    .map(|entry| entry.mark_stale(now_ms))
                    .collect();
            }
            return entries;
        }
        vec![QuotaEntry::unavailable(
            "codex",
            "unknown",
            CODEX_SOURCE,
            "rollout 中没有可验证的额度窗口",
        )]
    }
}

fn unavailable_windows(
    provider: &str,
    source: &str,
    windows: &[&str],
    reason: impl Into<String>,
) -> Vec<QuotaEntry> {
    let reason = reason.into();
    windows
        .iter()
        .map(|window| QuotaEntry::unavailable(provider, window, source, reason.clone()))
        .collect()
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheDocument {
    schema_version: u32,
    provider: String,
    source: String,
    captured_at: u64,
    windows: Vec<CacheWindow>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheWindow {
    window: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_minutes: Option<u64>,
    used_pct: f64,
    resets_at: u64,
}

pub fn capture_claude_statusline(
    input: &[u8],
    cache_path: &Path,
    now_ms: u64,
) -> Result<Vec<QuotaEntry>, QuotaError> {
    if input.len() as u64 > MAX_STATUSLINE_BYTES {
        return Err(QuotaError::TooLarge(MAX_STATUSLINE_BYTES));
    }
    let payload: Value = serde_json::from_slice(input)?;
    let mut windows = Vec::new();
    let Some(rate_limits) = payload.get("rate_limits").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    for (raw_name, window) in rate_limits {
        let Some(used_pct) = window.get("used_percentage").and_then(Value::as_f64) else {
            continue;
        };
        let Some(resets_at) = window.get("resets_at").and_then(Value::as_u64) else {
            continue;
        };
        if !used_pct.is_finite() || !(0.0..=100.0).contains(&used_pct) || resets_at == 0 {
            continue;
        }
        let Some(name) = claude_window_id(raw_name) else {
            continue;
        };
        windows.push(CacheWindow {
            window: name.clone(),
            label: window
                .get("limit_name")
                .or_else(|| window.get("name"))
                .and_then(Value::as_str)
                .and_then(bounded_label),
            window_minutes: window
                .get("window_minutes")
                .and_then(Value::as_u64)
                .or_else(|| canonical_window_minutes(&name)),
            used_pct,
            resets_at,
        });
    }
    if windows.is_empty() {
        return Ok(Vec::new());
    }
    let cache = CacheDocument {
        schema_version: CACHE_SCHEMA_VERSION,
        provider: "claude".to_owned(),
        source: CLAUDE_SOURCE.to_owned(),
        captured_at: now_ms,
        windows,
    };
    let mut bytes = serde_json::to_vec_pretty(&cache)?;
    bytes.push(b'\n');
    atomic_write(cache_path, &bytes, 0o600)?;
    Ok(cache
        .windows
        .into_iter()
        .map(|window| {
            let CacheWindow {
                window,
                label,
                window_minutes,
                used_pct,
                resets_at,
            } = window;
            QuotaEntry::available("claude", window, used_pct, resets_at, CLAUDE_SOURCE, now_ms)
                .with_metadata(window_minutes, None, label, None)
        })
        .collect())
}

pub fn statusline_text(entries: &[QuotaEntry]) -> String {
    let parts = entries
        .iter()
        .filter_map(|entry| {
            entry
                .remaining_pct
                .map(|remaining| format!("{} 剩余 {:.0}%", entry.window, remaining))
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        "Flow Agent · 额度等待首次响应".to_owned()
    } else {
        format!("Flow Agent · {}", parts.join(" · "))
    }
}

#[derive(Debug, Deserialize)]
struct SessionMetaEnvelope {
    #[serde(rename = "type")]
    kind: String,
    payload: SessionMetaPayload,
}

#[derive(Debug, Deserialize)]
struct SessionMetaPayload {
    cli_version: Option<String>,
}

fn read_codex_version(path: &Path) -> Result<Option<String>, QuotaError> {
    refuse_symlink(path)?;
    let file = File::open(path).map_err(|source| QuotaError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(file.take(MAX_SESSION_META_BYTES));
    let mut line = String::new();
    while reader
        .read_line(&mut line)
        .map_err(|source| QuotaError::Io {
            path: path.to_path_buf(),
            source,
        })?
        > 0
    {
        if line.contains("\"session_meta\"") {
            if let Ok(envelope) = serde_json::from_str::<SessionMetaEnvelope>(&line) {
                if envelope.kind == "session_meta" {
                    return Ok(envelope.payload.cli_version);
                }
            }
        }
        line.clear();
    }
    Ok(None)
}

#[derive(Debug, Deserialize)]
struct EventEnvelope {
    #[serde(rename = "type")]
    kind: String,
    payload: EventPayload,
}

#[derive(Debug, Deserialize)]
struct EventPayload {
    #[serde(rename = "type")]
    kind: String,
    rate_limits: Option<CodexLimits>,
}

#[derive(Debug, Deserialize)]
struct CodexLimits {
    limit_id: Option<String>,
    limit_name: Option<String>,
    plan_type: Option<String>,
    primary: Option<CodexWindow>,
    secondary: Option<CodexWindow>,
}

#[derive(Debug, Deserialize)]
struct CodexWindow {
    used_percent: f64,
    window_minutes: u64,
    resets_at: u64,
}

fn read_codex_limits(path: &Path, captured_at: u64) -> Result<Vec<QuotaEntry>, QuotaError> {
    refuse_symlink(path)?;
    let mut file = File::open(path).map_err(|source| QuotaError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let length = file
        .metadata()
        .map_err(|source| QuotaError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let start = length.saturating_sub(MAX_ROLLOUT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|source| QuotaError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut bytes = Vec::with_capacity((length - start) as usize);
    file.read_to_end(&mut bytes)
        .map_err(|source| QuotaError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut lines = bytes.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    for line in lines.into_iter().rev() {
        if !contains_bytes(line, b"\"token_count\"") || !contains_bytes(line, b"\"rate_limits\"") {
            continue;
        }
        let Ok(envelope) = serde_json::from_slice::<EventEnvelope>(line) else {
            continue;
        };
        if envelope.kind != "event_msg" || envelope.payload.kind != "token_count" {
            continue;
        }
        let Some(limits) = envelope.payload.rate_limits else {
            continue;
        };
        let limit_id = limits.limit_id.and_then(|value| bounded_label(&value));
        let limit_name = limits.limit_name.and_then(|value| bounded_label(&value));
        let plan_type = limits.plan_type.and_then(|value| bounded_label(&value));
        let mut entries = Vec::new();
        for window in [limits.primary, limits.secondary].into_iter().flatten() {
            if !window.used_percent.is_finite()
                || !(0.0..=100.0).contains(&window.used_percent)
                || window.window_minutes == 0
                || window.resets_at == 0
            {
                continue;
            }
            entries.push(
                QuotaEntry::available(
                    "codex",
                    format!("{}m", window.window_minutes),
                    window.used_percent,
                    window.resets_at,
                    CODEX_SOURCE,
                    captured_at,
                )
                .with_metadata(
                    Some(window.window_minutes),
                    limit_id.clone(),
                    limit_name.clone(),
                    plan_type.clone(),
                ),
            );
        }
        return Ok(entries);
    }
    Ok(Vec::new())
}

fn claude_window_id(value: &str) -> Option<String> {
    let canonical = match value {
        "five_hour" => "5h".to_owned(),
        "seven_day" => "7d".to_owned(),
        other => other
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
            .take(48)
            .collect(),
    };
    (!canonical.is_empty()).then_some(canonical)
}

fn canonical_window_minutes(window: &str) -> Option<u64> {
    match window {
        "5h" => Some(300),
        "7d" => Some(10_080),
        _ => window
            .strip_suffix('m')
            .and_then(|minutes| minutes.parse::<u64>().ok())
            .filter(|minutes| *minutes > 0),
    }
}

fn bounded_label(value: &str) -> Option<String> {
    let normalized = value
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(64)
        .collect::<String>();
    (!normalized.is_empty()).then_some(normalized)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn collect_rollouts(path: &Path, depth: usize, output: &mut Vec<(PathBuf, u64)>) {
    if depth > 5 || output.len() >= MAX_ROLLOUT_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
    for entry in entries {
        if output.len() >= MAX_ROLLOUT_FILES {
            break;
        }
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_rollouts(&path, depth + 1, output);
        } else if metadata.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.starts_with("rollout-"))
        {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .and_then(|value| value.as_millis().try_into().ok())
                .unwrap_or(0);
            output.push((path, modified));
        }
    }
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, QuotaError> {
    refuse_symlink(path)?;
    let metadata = fs::metadata(path).map_err(|source| QuotaError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > limit {
        return Err(QuotaError::TooLarge(limit));
    }
    fs::read(path).map_err(|source| QuotaError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn refuse_symlink(path: &Path) -> Result<(), QuotaError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(QuotaError::SymlinkRefused(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(QuotaError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), QuotaError> {
    refuse_symlink(path)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        let mut builder = DirBuilder::new();
        builder
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|source| QuotaError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
    }
    refuse_symlink(parent)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("quota");
    let temporary = parent.join(format!(
        ".{name}.flow-agent.{}.{}.tmp",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)
            .map_err(|source| QuotaError::Io {
                path: temporary.clone(),
                source,
            })?;
        file.write_all(bytes).map_err(|source| QuotaError::Io {
            path: temporary.clone(),
            source,
        })?;
        file.sync_all().map_err(|source| QuotaError::Io {
            path: temporary.clone(),
            source,
        })?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(mode)).map_err(|source| {
            QuotaError::Io {
                path: temporary.clone(),
                source,
            }
        })?;
        fs::rename(&temporary, path).map_err(|source| QuotaError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn root(name: &str) -> PathBuf {
        let path = PathBuf::from("/tmp").join(format!(
            "flow-agent-quota-{name}-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn claude_capture_persists_only_valid_rate_limit_fields() {
        let root = root("claude");
        let cache = root.join("cache/claude-rl.json");
        let payload = br#"{
          "session_id":"secret-session",
          "cwd":"/private/customer-project",
          "transcript_path":"/private/transcript.jsonl",
          "rate_limits":{
            "five_hour":{"used_percentage":23.5,"resets_at":1784140000},
            "seven_day":{"used_percentage":41.2,"resets_at":1784740000},
            "fable":{"used_percentage":9.0,"resets_at":1784800000,"name":"Fable","window_minutes":1440}
          }
        }"#;
        let entries = capture_claude_statusline(payload, &cache, 1_784_130_000_000).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.window == "5h")
                .and_then(|entry| entry.remaining_pct),
            Some(76.5)
        );
        let fable = entries
            .iter()
            .find(|entry| entry.window == "fable")
            .unwrap();
        assert_eq!(fable.limit_name.as_deref(), Some("Fable"));
        assert_eq!(fable.window_minutes, Some(1_440));
        let saved = fs::read_to_string(&cache).unwrap();
        assert!(!saved.contains("secret-session"));
        assert!(!saved.contains("customer-project"));
        assert!(!saved.contains("transcript"));
        assert_eq!(
            fs::metadata(&cache).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let collected = QuotaCollector::new(QuotaPaths {
            flow_home: root.clone(),
            codex_sessions: root.join("none"),
        })
        .collect_claude(1_784_130_100_000);
        assert_eq!(collected[1].status, "available");
    }

    #[test]
    fn stale_claude_cache_preserves_last_value_but_incompatible_data_stays_unavailable() {
        let stale_root = root("stale");
        let paths = QuotaPaths {
            flow_home: stale_root.clone(),
            codex_sessions: stale_root.join("none"),
        };
        capture_claude_statusline(
            br#"{"rate_limits":{"five_hour":{"used_percentage":50,"resets_at":1784140000}}}"#,
            &paths.claude_cache(),
            1_000,
        )
        .unwrap();
        let stale = QuotaCollector::new(paths.clone()).collect_claude(FRESH_FOR_MS + 2_000);
        assert_eq!(stale[0].status, "stale");
        assert_eq!(stale[0].used_pct, Some(50.0));
        assert_eq!(stale[0].remaining_pct, Some(50.0));
        assert_eq!(stale[0].resets_at, Some(1_784_140_000));
        fs::write(
            paths.claude_cache(),
            br#"{"schemaVersion":99,"provider":"claude","source":"statusline","capturedAt":1,"windows":[]}"#,
        )
        .unwrap();
        let incompatible = QuotaCollector::new(paths).collect_claude(2);
        assert_eq!(incompatible[0].status, "unavailable");
        assert_eq!(incompatible[0].remaining_pct, None);

        let future_root = root("future-claude");
        let future_paths = QuotaPaths {
            flow_home: future_root.clone(),
            codex_sessions: future_root.join("none"),
        };
        capture_claude_statusline(
            br#"{"rate_limits":{"five_hour":{"used_percentage":50,"resets_at":1784140000}}}"#,
            &future_paths.claude_cache(),
            MAX_CLOCK_SKEW_MS + 10,
        )
        .unwrap();
        let future = QuotaCollector::new(future_paths).collect_claude(1);
        assert_eq!(future[0].status, "unavailable");
        assert_eq!(future[0].remaining_pct, None);
    }

    fn write_rollout(root: &Path, version: &str, rate_limits: &str) -> PathBuf {
        let directory = root.join("2026/07/15");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("rollout-fixture.jsonl");
        let limits: Value = serde_json::from_str(rate_limits).unwrap();
        let meta = serde_json::json!({
            "type": "session_meta",
            "payload": {
                "cli_version": version,
                "base_instructions": "must never be surfaced"
            }
        });
        let private_record = serde_json::json!({
            "type": "response_item",
            "payload": { "content": "private prompt" }
        });
        let limit_record = serde_json::json!({
            "type": "event_msg",
            "payload": { "type": "token_count", "rate_limits": limits }
        });
        let text = format!("{meta}\n{private_record}\n{limit_record}\n");
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn codex_rollout_is_shape_validated_and_returns_every_limit_window() {
        let root = root("codex");
        write_rollout(
            &root,
            "0.144.4",
            r#"{"limit_id":"codex","primary":{"used_percent":12.0,"window_minutes":300,"resets_at":1784140000},"secondary":{"used_percent":44.0,"window_minutes":10080,"resets_at":1784740000}}"#,
        );
        let now = fs::metadata(root.join("2026/07/15/rollout-fixture.jsonl"))
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let collector = QuotaCollector::new(QuotaPaths {
            flow_home: root.join("flow"),
            codex_sessions: root.clone(),
        });
        let entries = collector.collect_codex(now);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].window, "300m");
        assert_eq!(entries[0].remaining_pct, Some(88.0));
        assert_eq!(entries[1].window, "10080m");
        assert_eq!(entries[1].remaining_pct, Some(56.0));

        write_rollout(&root, "0.145.0", "null");
        let incompatible = collector.collect_codex(now + 1);
        assert_eq!(incompatible[0].status, "unavailable");
        assert!(incompatible[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("没有可验证"));
        assert_eq!(incompatible[0].used_pct, None);
    }

    #[test]
    fn current_codex_rollout_fixture_matches_the_gated_adapter() {
        let root = root("codex-fixture");
        let directory = root.join("2026/07/15");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("rollout-fixture.jsonl"),
            include_bytes!("../../../fixtures/codex/0.144.4/rate-limits-rollout.jsonl"),
        )
        .unwrap();
        let captured_at = fs::metadata(directory.join("rollout-fixture.jsonl"))
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let entries = QuotaCollector::new(QuotaPaths {
            flow_home: root.join("flow"),
            codex_sessions: root.clone(),
        })
        .collect_codex(captured_at);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].used_pct, Some(12.0));
        assert_eq!(entries[0].window, "300m");
        assert_eq!(entries[1].used_pct, Some(44.0));
        assert_eq!(entries[1].window, "10080m");

        let future = QuotaCollector::new(QuotaPaths {
            flow_home: root.join("future-flow"),
            codex_sessions: directory
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf(),
        })
        .collect_codex(captured_at.saturating_sub(MAX_CLOCK_SKEW_MS + 1));
        assert_eq!(future[0].status, "unavailable");
        assert_eq!(future[0].remaining_pct, None);
    }

    #[test]
    fn codex_0_144_5_weekly_fixture_matches_the_local_rollout_schema() {
        let root = root("codex-0-144-5");
        let directory = root.join("2026/07/16");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("rollout-fixture.jsonl");
        fs::write(
            &path,
            include_bytes!("../../../fixtures/codex/0.144.5/rate-limits-rollout.jsonl"),
        )
        .unwrap();
        let captured_at = fs::metadata(&path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let entries = QuotaCollector::new(QuotaPaths {
            flow_home: root.join("flow"),
            codex_sessions: root.clone(),
        })
        .collect_codex(captured_at);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].window, "10080m");
        assert_eq!(entries[0].used_pct, Some(8.0));
        assert_eq!(entries[0].remaining_pct, Some(92.0));
    }

    #[test]
    fn codex_0_144_2_desktop_fixture_matches_the_local_rollout_schema() {
        let root = root("codex-0-144-2-desktop");
        let directory = root.join("2026/07/16");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("rollout-fixture.jsonl");
        fs::write(
            &path,
            include_bytes!("../../../fixtures/codex/0.144.2/rate-limits-rollout.jsonl"),
        )
        .unwrap();
        let captured_at = fs::metadata(&path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let entries = QuotaCollector::new(QuotaPaths {
            flow_home: root.join("flow"),
            codex_sessions: root.clone(),
        })
        .collect_codex(captured_at);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].window, "10080m");
        assert_eq!(entries[0].used_pct, Some(49.0));
        assert_eq!(entries[0].remaining_pct, Some(51.0));
    }

    #[test]
    fn rollout_scan_cap_prefers_newest_lexical_session_paths() {
        let root = root("rollout-cap");
        for index in 0..=MAX_ROLLOUT_FILES {
            let directory = root.join(format!("session-{index:03}"));
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join(format!("rollout-{index:03}.jsonl")), b"{}\n").unwrap();
        }

        let mut files = Vec::new();
        collect_rollouts(&root, 0, &mut files);
        assert_eq!(files.len(), MAX_ROLLOUT_FILES);
        assert!(files
            .iter()
            .any(|(path, _)| path.to_string_lossy().contains("session-256")));
        assert!(!files
            .iter()
            .any(|(path, _)| path.to_string_lossy().contains("session-000")));
    }
}
