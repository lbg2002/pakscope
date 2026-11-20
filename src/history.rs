//! Local audit log for user-initiated uninstall and update actions.
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_HISTORY_ITEMS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: String,
    pub action: String,
    pub app_name: String,
    pub app_id: String,
    pub source: String,
    pub command: String,
    pub success: bool,
    pub detail: String,
}

/// Format Unix epoch seconds into `YYYY-MM-DD HH:MM:SS UTC` using Howard Hinnant's civil calendar algorithm.
pub fn format_unix_seconds(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{year:04}-{m:02}-{d:02} {hour:02}:{minute:02}:{second:02} UTC")
}

pub fn now_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_unix_seconds(secs)
}

pub fn history_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return Some(p.join("pakscope").join("history.json"));
        }
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join(".local").join("share").join("pakscope").join("history.json"))
}

pub fn load_history() -> Vec<HistoryEntry> {
    let Some(path) = history_path() else {
        return Vec::new();
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn record_event(
    action: &str,
    app_name: &str,
    app_id: &str,
    source: &str,
    command: &str,
    success: bool,
    detail: &str,
) {
    let Some(path) = history_path() else {
        return;
    };
    let mut entries = load_history();
    let trimmed_detail: String = detail.chars().take(600).collect();
    entries.insert(
        0,
        HistoryEntry {
            timestamp: now_timestamp(),
            action: action.to_string(),
            app_name: app_name.to_string(),
            app_id: app_id.to_string(),
            source: source.to_string(),
            command: command.to_string(),
            success,
            detail: trimmed_detail,
        },
    );
    entries.truncate(MAX_HISTORY_ITEMS);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&entries) {
        let _ = fs::write(path, json);
    }
}

pub fn clear_history() -> Result<(), String> {
    let Some(path) = history_path() else {
        return Ok(());
    };
    if path.exists() {
        fs::remove_file(path).map_err(|e| format!("清空历史记录失败：{e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_known_date_format_correctly() {
        assert_eq!(format_unix_seconds(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_unix_seconds(1_700_000_000), "2023-11-14 22:13:20 UTC");
    }
}
