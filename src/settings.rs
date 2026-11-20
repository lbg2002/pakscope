//! Persistent user preferences stored in ~/.config/pakscope/settings.json.
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeMode,
    pub custom_appimage_dirs: Vec<String>,
    pub scan_downloads_appimage: bool,
    pub include_cli_packages: bool,
    pub sidebar_width: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            custom_appimage_dirs: Vec::new(),
            scan_downloads_appimage: true,
            include_cli_packages: false,
            sidebar_width: 224,
        }
    }
}

impl Settings {
    pub fn config_path() -> Option<PathBuf> {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            let p = PathBuf::from(xdg);
            if p.is_absolute() {
                return Some(p.join("pakscope").join("settings.json"));
            }
        }
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|h| h.join(".config").join("pakscope").join("settings.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path().ok_or("无法定位用户配置目录")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
        }
        let content = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, content).map_err(|e| format!("写入设置失败：{e}"))
    }

    pub fn add_appimage_dir(&mut self, raw: &str) -> Result<String, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("目录路径不能为空".into());
        }
        let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or("无法解析主目录")?;
            home.join(rest)
        } else {
            PathBuf::from(trimmed)
        };
        if !expanded.is_absolute() {
            return Err("请输入绝对路径（或以 ~/ 开头）".into());
        }
        let canon = expanded
            .canonicalize()
            .map_err(|e| format!("目录不存在或不可访问：{e}"))?;
        if !canon.is_dir() {
            return Err("目标路径不是文件夹".into());
        }
        let blocked = ["/proc", "/sys", "/dev", "/etc", "/boot", "/run"];
        let text = canon.to_string_lossy().to_string();
        if blocked.iter().any(|b| text == *b || text.starts_with(&format!("{b}/"))) || text == "/" {
            return Err("禁止将系统核心目录或根目录设为 AppImage 扫描目录".into());
        }
        if !self.custom_appimage_dirs.contains(&text) {
            self.custom_appimage_dirs.push(text.clone());
        }
        Ok(text)
    }

    pub fn remove_appimage_dir(&mut self, target: &str) {
        self.custom_appimage_dirs.retain(|d| d != target);
    }

    pub fn is_managed_custom_dir(&self, candidate: &Path) -> bool {
        self.custom_appimage_dirs.iter().any(|d| {
            Path::new(d)
                .canonicalize()
                .is_ok_and(|root| candidate.starts_with(root))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_json_deserialize() {
        let parsed: Settings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(parsed.theme, ThemeMode::Dark);
        assert!(parsed.scan_downloads_appimage);
        assert!(!parsed.include_cli_packages);
        assert_eq!(parsed.sidebar_width, 224);
    }

    #[test]
    fn remove_custom_dir_is_idempotent() {
        let mut s = Settings::default();
        s.custom_appimage_dirs.push("/tmp/example".into());
        s.remove_appimage_dir("/tmp/example");
        assert!(s.custom_appimage_dirs.is_empty());
    }
}
