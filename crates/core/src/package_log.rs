// SPDX-License-Identifier: MIT OR Apache-2.0
//! Structured, versioned package-operation log.
//!
//! One JSON object per completed operation is appended to
//! `package-operations.jsonl` in the state directory. The schema is additive:
//! readers ignore fields they do not know and use [`VERSION`] to decide what
//! they understand. Every string is redacted before it is written; environment
//! values are never part of an event.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const SCHEMA: &str = "forge.package.operation";
pub const VERSION: u32 = 1;
pub const FILE_NAME: &str = "package-operations.jsonl";
/// Rotate once the current file reaches this size. The previous file is kept
/// as `package-operations.jsonl.1`.
pub const MAX_BYTES: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Fetch,
    Build,
    Install,
    Upgrade,
    Rollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Ok,
    Failed,
    Declined,
    Cancelled,
    Unchanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    Index,
    Url,
    Path,
    Git,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

impl Source {
    pub fn new(kind: SourceKind) -> Self {
        Self {
            kind,
            location: None,
            revision: None,
        }
    }

    pub fn location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(redact(&location.into()));
        self
    }

    pub fn revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = Some(redact(&revision.into()));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub schema: String,
    pub version: u32,
    pub operation: Operation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    pub status: Status,
    pub started_at_ms: u64,
    pub duration_us: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub detail: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A start time for one operation. It is cheap to keep beside asynchronous
/// work and produces an [`Event`] when that work finishes.
#[derive(Debug, Clone, Copy)]
pub struct Timer {
    started: Instant,
    started_at_ms: u64,
}

impl Timer {
    pub fn start() -> Self {
        Self {
            started: Instant::now(),
            started_at_ms: unix_ms(),
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn finish(self, operation: Operation, status: Status, package: Option<&str>) -> Event {
        Event::new(operation, status, package, self.elapsed()).started_at(self.started_at_ms)
    }
}

impl Event {
    pub fn new(
        operation: Operation,
        status: Status,
        package: Option<&str>,
        elapsed: Duration,
    ) -> Self {
        let duration_us = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        let started_at_ms = unix_ms().saturating_sub(elapsed.as_millis() as u64);
        Self {
            schema: SCHEMA.to_string(),
            version: VERSION,
            operation,
            package: package.map(redact),
            status,
            started_at_ms,
            duration_us,
            source: None,
            detail: BTreeMap::new(),
            error: None,
        }
    }

    fn started_at(mut self, started_at_ms: u64) -> Self {
        self.started_at_ms = started_at_ms;
        self
    }

    pub fn source(mut self, source: Source) -> Self {
        self.source = Some(source);
        self
    }

    pub fn detail(mut self, key: &str, value: impl Into<String>) -> Self {
        self.detail.insert(key.to_string(), redact(&value.into()));
        self
    }

    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(redact(&error.into()));
        self
    }

    pub fn to_json_line(&self) -> Result<String, String> {
        serde_json::to_string(&self.redacted()).map_err(|e| e.to_string())
    }

    fn redacted(&self) -> Self {
        let mut event = self.clone();
        event.package = event.package.as_deref().map(redact);
        if let Some(source) = &mut event.source {
            source.location = source.location.as_deref().map(redact);
            source.revision = source.revision.as_deref().map(redact);
        }
        for value in event.detail.values_mut() {
            *value = redact(value);
        }
        event.error = event.error.as_deref().map(redact);
        event
    }
}

pub fn log_path() -> Option<PathBuf> {
    crate::config::state_dir().map(|dir| dir.join(FILE_NAME))
}

pub fn append(event: &Event) -> Result<(), String> {
    let path = log_path().ok_or_else(|| "no state directory".to_string())?;
    append_to(&path, event)
}

pub fn append_to(path: &Path, event: &Event) -> Result<(), String> {
    append_with_limit(path, event, MAX_BYTES)
}

pub fn read_events() -> Result<Vec<Event>, String> {
    let path = log_path().ok_or_else(|| "no state directory".to_string())?;
    read_events_from(&path)
}

pub fn read_events_from(path: &Path) -> Result<Vec<Event>, String> {
    let mut events = Vec::new();
    for_each_line(path, |line| {
        if let Ok(event) = serde_json::from_str(line) {
            events.push(event);
        }
    })?;
    Ok(events)
}

pub fn read_json_lines() -> Result<String, String> {
    let path = log_path().ok_or_else(|| "no state directory".to_string())?;
    read_json_lines_from(&path)
}

pub fn read_json_lines_from(path: &Path) -> Result<String, String> {
    let mut out = String::new();
    for_each_line(path, |line| {
        if serde_json::from_str::<serde_json::Value>(line).is_ok() {
            out.push_str(line);
            out.push('\n');
        }
    })?;
    Ok(out)
}

pub fn render_text(events: &[Event]) -> String {
    let mut out = format!("Package operations ({SCHEMA} v{VERSION})\n");
    if events.is_empty() {
        out.push_str("\nNo package operations recorded.\n");
        return out;
    }
    out.push('\n');
    for event in events {
        let package = event.package.as_deref().unwrap_or("(index)");
        out.push_str(&format!(
            "{}  {:<8} {:<10} {:<9} {:>10.3} ms\n",
            format_time(event.started_at_ms),
            operation_name(event.operation),
            package,
            status_name(event.status),
            event.duration_us as f64 / 1000.0
        ));
        if let Some(source) = &event.source {
            out.push_str(&format!("  source: {}", source_name(source.kind)));
            if let Some(location) = &source.location {
                out.push(' ');
                out.push_str(location);
            }
            if let Some(revision) = &source.revision {
                out.push_str(&format!(" @ {revision}"));
            }
            out.push('\n');
        }
        for (key, value) in &event.detail {
            out.push_str(&format!("  {key}: {value}\n"));
        }
        if let Some(error) = &event.error {
            out.push_str(&format!("  error: {error}\n"));
        }
    }
    out
}

/// The short name used in the log's plain text.
pub fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::Fetch => "fetch",
        Operation::Build => "build",
        Operation::Install => "install",
        Operation::Upgrade => "upgrade",
        Operation::Rollback => "rollback",
    }
}

/// The short name used in the log's plain text.
pub fn status_name(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Failed => "failed",
        Status::Declined => "declined",
        Status::Cancelled => "cancelled",
        Status::Unchanged => "unchanged",
    }
}

fn source_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Index => "index",
        SourceKind::Url => "url",
        SourceKind::Path => "path",
        SourceKind::Git => "git",
    }
}

/// A UTC timestamp for the log's plain text.
pub fn format_time(ms: u64) -> String {
    let secs = ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX);
    let rest = secs % 86_400;
    let date = crate::org_time::Date::from_days(days);
    format!(
        "{} {:02}:{:02}:{:02}Z",
        date.iso(),
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn for_each_line(path: &Path, mut f: impl FnMut(&str)) -> Result<(), String> {
    for path in [rotated(path), path.to_path_buf()] {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        for line in text.lines() {
            let line = line.trim();
            if !line.is_empty() {
                f(line);
            }
        }
    }
    Ok(())
}

fn rotated(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| FILE_NAME.into());
    name.push(".1");
    path.with_file_name(name)
}

fn append_with_limit(path: &Path, event: &Event, max_bytes: u64) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    repair_tail(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len >= max_bytes {
        rotate(path)?;
    }
    let mut record = event.to_json_line()?.into_bytes();
    record.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(&record)
        .and_then(|()| file.sync_data())
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn rotate(path: &Path) -> Result<(), String> {
    let backup = rotated(path);
    match std::fs::remove_file(&backup) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{}: {error}", backup.display())),
    }
    std::fs::rename(path, &backup).map_err(|e| format!("{}: {e}", path.display()))
}

/// A process killed during an append may leave a final line without its
/// newline. Drop that partial line before adding another record.
fn repair_tail(path: &Path) -> std::io::Result<()> {
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    if data.is_empty() || data.last() == Some(&b'\n') {
        return Ok(());
    }
    let keep = data
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |at| at + 1);
    file.set_len(keep as u64)?;
    file.seek(SeekFrom::Start(keep as u64))?;
    file.sync_data()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

static URL_USERINFO: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://)([^/\s@]+)@").expect("valid URL userinfo regex")
});
static SENSITIVE_QUERY: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)([?&](?:access[_-]?token|auth|authorization|client[_-]?secret|credential|key|password|passwd|private[_-]?token|secret|sig|signature|token)=)[^&#\s]+",
    )
    .expect("valid sensitive query regex")
});
static SENSITIVE_VALUE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b([a-z0-9_-]*(?:token|secret|password|passwd|api[_-]?key|credential)[a-z0-9_-]*\s*[:=]\s*)([^\s,;]+)",
    )
    .expect("valid sensitive value regex")
});
static BEARER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(bearer\s+)[a-z0-9._~+/=-]+").expect("valid bearer regex")
});
static TOKEN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16})\b",
    )
    .expect("valid token regex")
});

/// Hide credentials and tokens from anything written to the log. Environment
/// values are not logged at all, so there is nothing to redact there.
pub fn redact(text: &str) -> String {
    let text = URL_USERINFO.replace_all(text, "$1***@");
    let text = SENSITIVE_QUERY.replace_all(&text, "$1***");
    let text = SENSITIVE_VALUE.replace_all(&text, "$1***");
    let text = BEARER.replace_all(&text, "$1***");
    TOKEN.replace_all(&text, "***").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_file() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "forge-package-log-{}-{}-{n}.jsonl",
            std::process::id(),
            unix_ms()
        ))
    }

    fn event(operation: Operation, status: Status) -> Event {
        Event::new(operation, status, Some("demo"), Duration::from_millis(12))
    }

    #[test]
    fn schema_is_versioned_and_additive() {
        let line = event(Operation::Fetch, Status::Ok)
            .source(
                Source::new(SourceKind::Git)
                    .location("https://example.com/demo")
                    .revision("v1"),
            )
            .detail("version", "1.2.3")
            .to_json_line()
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["schema"], SCHEMA);
        assert_eq!(value["version"], VERSION);
        assert_eq!(value["operation"], "fetch");
        assert_eq!(value["status"], "ok");
        assert_eq!(value["package"], "demo");
        assert_eq!(value["source"]["kind"], "git");

        let future = r#"{"schema":"forge.package.operation","version":1,"operation":"install","status":"ok","started_at_ms":1,"duration_us":2,"future_field":{"kept":true}}"#;
        let event: Event = serde_json::from_str(future).unwrap();
        assert_eq!(event.operation, Operation::Install);
    }

    #[test]
    fn redacts_url_credentials_queries_headers_and_env_values() {
        let line = event(Operation::Install, Status::Failed)
            .source(
                Source::new(SourceKind::Git)
                    .location("https://alice:hunter2@example.com/repo?token=secret-token&x=1")
                    .revision("https://bob:swordfish@example.com/tag"),
            )
            .detail("env", "GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz012345")
            .detail(
                "header",
                "Authorization: Bearer top-secret\npassword = swordfish",
            )
            .error("download failed: https://carol:pa55w0rd@example.com/x?access_token=abc123")
            .to_json_line()
            .unwrap();
        for secret in [
            "hunter2",
            "secret-token",
            "swordfish",
            "ghp_abcdefghijklmnopqrstuvwxyz012345",
            "top-secret",
            "pa55w0rd",
            "abc123",
        ] {
            assert!(!line.contains(secret), "{secret} leaked in {line}");
        }
        assert!(line.contains("***"));
    }

    #[test]
    fn appends_importable_json_lines() {
        let path = temp_file();
        let _ = std::fs::remove_file(&path);
        append_to(&path, &event(Operation::Fetch, Status::Ok)).unwrap();
        append_to(
            &path,
            &event(Operation::Build, Status::Failed).error("nope"),
        )
        .unwrap();
        let events = read_events_from(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].operation, Operation::Fetch);
        assert_eq!(events[1].status, Status::Failed);
        let lines = read_json_lines_from(&path).unwrap();
        assert_eq!(lines.lines().count(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn repairs_a_partial_final_line_before_appending() {
        let path = temp_file();
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"{\"schema\":\"forge.package.operation\"").unwrap();
        append_to(&path, &event(Operation::Install, Status::Ok)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("{\"schema\":\"forge.package.operation\"\n"));
        assert_eq!(read_events_from(&path).unwrap().len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rotates_a_full_file_and_reads_both_generations() {
        let path = temp_file();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(rotated(&path));
        let first = event(Operation::Fetch, Status::Ok);
        append_with_limit(&path, &first, 1).unwrap();
        append_with_limit(&path, &first, 1).unwrap();
        assert!(path.is_file());
        assert!(rotated(&path).is_file());
        let events = read_events_from(&path).unwrap();
        assert_eq!(events.len(), 2);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(rotated(&path));
    }

    #[test]
    fn renders_operations_and_timings() {
        let text = render_text(&[
            event(Operation::Fetch, Status::Ok).detail("version", "1.2.3"),
            event(Operation::Rollback, Status::Failed).error("rolled back"),
        ]);
        assert!(text.contains("forge.package.operation v1"));
        assert!(text.contains("fetch"));
        assert!(text.contains("12.000 ms"));
        assert!(text.contains("rollback"));
        assert!(text.contains("error: rolled back"));
    }
}
