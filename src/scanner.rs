//! Multi-source unprivileged Linux application & package inventory.
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    Apt,
    Snap,
    Flatpak,
    AppImage,
    Manual,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Apt => "APT / DEB",
            Self::Snap => "Snap",
            Self::Flatpak => "Flatpak",
            Self::AppImage => "AppImage",
            Self::Manual => "手动 / 本地",
        }
    }

    pub fn as_key(&self) -> &'static str {
        match self {
            Self::Apt => "apt",
            Self::Snap => "snap",
            Self::Flatpak => "flatpak",
            Self::AppImage => "appimage",
            Self::Manual => "manual",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallScope {
    System,
    User,
    #[default]
    Unknown,
}

impl InstallScope {
    pub fn label(&self) -> &'static str {
        match self {
            Self::System => "系统级",
            Self::User => "用户级",
            Self::Unknown => "未确定",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserDataPath {
    pub label: String,
    pub path: String,
    pub size_kib: Option<u64>,
    pub safe_to_trash: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub generic_name: String,
    pub summary: String,
    pub version: String,
    pub source: Source,
    pub scope: InstallScope,
    pub path: String,
    pub launcher: String,
    pub icon: String,
    pub command: String,
    pub categories: Vec<String>,
    pub size_kib: Option<u64>,
    pub size_note: String,
    pub confidence: String,
    pub is_protected: bool,
    pub protection_reason: Option<String>,
    pub is_cli: bool,
    pub broken_launcher: bool,
    pub duplicate_sources: Vec<String>,
    pub user_data: Vec<UserDataPath>,
}

impl AppEntry {
    pub fn minimal(source: Source, id: &str, path: &str) -> Self {
        Self {
            id: id.into(),
            name: id.into(),
            generic_name: String::new(),
            summary: String::new(),
            version: "1.0".into(),
            source,
            scope: InstallScope::System,
            path: path.into(),
            launcher: String::new(),
            icon: String::new(),
            command: String::new(),
            categories: Vec::new(),
            size_kib: None,
            size_note: String::new(),
            confidence: String::new(),
            is_protected: false,
            protection_reason: None,
            is_cli: false,
            broken_launcher: false,
            duplicate_sources: Vec::new(),
            user_data: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScanOptions {
    pub include_cli: bool,
    pub scan_downloads_appimage: bool,
    pub custom_appimage_dirs: Vec<String>,
}

impl ScanOptions {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            include_cli: s.include_cli_packages,
            scan_downloads_appimage: s.scan_downloads_appimage,
            custom_appimage_dirs: s.custom_appimage_dirs.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct DesktopMeta {
    name: String,
    chinese_name: String,
    generic_name: String,
    chinese_generic: String,
    comment: String,
    chinese_comment: String,
    icon: String,
    exec: String,
    try_exec: String,
    categories: Vec<String>,
    flatpak_id: String,
    snap_name: String,
    appimage_version: String,
    hidden: bool,
    no_display: bool,
    is_application: bool,
}

fn desktop_parse(content: &str) -> Option<DesktopMeta> {
    let mut meta = DesktopMeta::default();
    let mut section = "";
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line;
            continue;
        }
        if section != "[Desktop Entry]" || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_val)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let val = raw_val.trim();
        match key {
            "Name" => meta.name = val.to_string(),
            "Name[zh_CN]" | "Name[zh_SG]" | "Name[zh]" if !val.is_empty() => {
                if meta.chinese_name.is_empty() || key == "Name[zh_CN]" {
                    meta.chinese_name = val.to_string();
                }
            }
            "GenericName" => meta.generic_name = val.to_string(),
            "GenericName[zh_CN]" | "GenericName[zh]" if !val.is_empty() => {
                meta.chinese_generic = val.to_string();
            }
            "Comment" => meta.comment = val.to_string(),
            "Comment[zh_CN]" | "Comment[zh]" if !val.is_empty() => {
                meta.chinese_comment = val.to_string();
            }
            "Icon" => meta.icon = val.to_string(),
            "Exec" => meta.exec = val.to_string(),
            "TryExec" => meta.try_exec = val.to_string(),
            "Categories" => {
                meta.categories = val
                    .split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
            }
            "X-Flatpak" => meta.flatpak_id = val.to_string(),
            "X-SnapInstanceName" => meta.snap_name = val.to_string(),
            "X-AppImage-Version" => meta.appimage_version = val.to_string(),
            "Hidden" => meta.hidden = val.eq_ignore_ascii_case("true"),
            "NoDisplay" => meta.no_display = val.eq_ignore_ascii_case("true"),
            "Type" => meta.is_application = val == "Application",
            _ => {}
        }
    }
    if meta.hidden || meta.no_display || !meta.is_application {
        return None;
    }
    if !meta.chinese_name.is_empty() {
        meta.name = meta.chinese_name.clone();
    }
    if !meta.chinese_generic.is_empty() {
        meta.generic_name = meta.chinese_generic.clone();
    }
    if !meta.chinese_comment.is_empty() {
        meta.comment = meta.chinese_comment.clone();
    }
    if meta.name.is_empty() {
        return None;
    }
    Some(meta)
}

/// Tokenize a Desktop Entry `Exec=` line without invoking any shell.
pub fn tokenize_exec(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if !in_single => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            c if c.is_whitespace() && !in_single && !in_double => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// Extract the primary executable token from `Exec=`, skipping `env` and `KEY=VALUE` assignments.
fn extract_command_token(exec: &str) -> Option<String> {
    let tokens = tokenize_exec(exec);
    let mut idx = 0;
    while idx < tokens.len() {
        let t = &tokens[idx];
        let base = Path::new(t).file_name().and_then(|s| s.to_str()).unwrap_or(t);
        if base == "env" {
            idx += 1;
            continue;
        }
        if idx > 0 && !t.starts_with('/') && t.contains('=') && !t.starts_with('-') {
            idx += 1;
            continue;
        }
        if t.starts_with('-') && idx > 0 {
            idx += 1;
            continue;
        }
        if t.starts_with('%') {
            return None;
        }
        return Some(t.clone());
    }
    None
}

/// Resolve an `Exec=` or `TryExec=` command to an existing file on disk if determinable.
fn executable_from_desktop(exec: &str, try_exec: &str) -> (String, bool) {
    let candidate_str = if !try_exec.trim().is_empty() {
        Some(try_exec.trim().to_string())
    } else {
        extract_command_token(exec)
    };
    let Some(cmd) = candidate_str else {
        return (String::new(), false);
    };
    let base = Path::new(&cmd).file_name().and_then(|s| s.to_str()).unwrap_or(&cmd);
    if matches!(base, "sh" | "bash" | "dash" | "python" | "python3" | "gio" | "xdg-open") {
        return (String::new(), false);
    }
    let p = Path::new(&cmd);
    if p.is_absolute() {
        if p.is_file() {
            return (p.display().to_string(), false);
        }
        return (String::new(), true);
    }
    if cmd.contains('/') {
        return (String::new(), false);
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let file = dir.join(&cmd);
            if file.is_file() {
                return (file.display().to_string(), false);
            }
        }
    }
    (String::new(), false)
}

fn detect_flatpak_from_exec(meta: &DesktopMeta) -> Option<String> {
    if !meta.flatpak_id.is_empty() {
        return Some(meta.flatpak_id.clone());
    }
    let tokens = tokenize_exec(&meta.exec);
    let pos = tokens.iter().position(|t| {
        Path::new(t)
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|b| b == "flatpak")
    })?;
    if tokens.get(pos + 1).map(String::as_str) != Some("run") {
        return None;
    }
    tokens[pos + 2..]
        .iter()
        .find(|t| !t.starts_with('-') && !t.starts_with('%') && t.contains('.'))
        .cloned()
}

fn detect_snap_from_exec(meta: &DesktopMeta) -> Option<String> {
    if !meta.snap_name.is_empty() {
        return Some(meta.snap_name.clone());
    }
    let tokens = tokenize_exec(&meta.exec);
    for (i, t) in tokens.iter().enumerate() {
        if let Some(rest) = t.strip_prefix("/snap/bin/") {
            let pkg = rest.split('.').next().unwrap_or(rest);
            if !pkg.is_empty() {
                return Some(pkg.to_string());
            }
        }
        if Path::new(t).file_name().and_then(|s| s.to_str()) == Some("snap")
            && tokens.get(i + 1).map(String::as_str) == Some("run")
        {
            if let Some(target) = tokens.get(i + 2..).into_iter().flatten().find(|a| !a.starts_with('-')) {
                let pkg = target.split('.').next().unwrap_or(target);
                if !pkg.is_empty() {
                    return Some(pkg.to_string());
                }
            }
        }
    }
    None
}

fn detect_appimage_target(meta: &DesktopMeta) -> Option<PathBuf> {
    if !meta.try_exec.is_empty() && meta.try_exec.to_ascii_lowercase().ends_with(".appimage") {
        return Some(PathBuf::from(meta.try_exec.trim()));
    }
    for token in tokenize_exec(&meta.exec) {
        if token.to_ascii_lowercase().ends_with(".appimage") {
            return Some(PathBuf::from(token));
        }
    }
    None
}

fn desktop_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("desktop"))
        .collect()
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

struct DpkgIndex {
    desktop_owners: HashMap<String, String>,
    binary_owners: HashMap<String, String>,
}

fn insert_usr_merged_alias(map: &mut HashMap<String, String>, path: &str, pkg: &str) {
    map.insert(path.to_string(), pkg.to_string());
    if let Some(rest) = path.strip_prefix("/bin/") {
        map.entry(format!("/usr/bin/{rest}")).or_insert_with(|| pkg.to_string());
    } else if let Some(rest) = path.strip_prefix("/usr/bin/") {
        map.entry(format!("/bin/{rest}")).or_insert_with(|| pkg.to_string());
    }
}

fn dpkg_index() -> DpkgIndex {
    let mut desktop_owners = HashMap::new();
    let mut binary_owners = HashMap::new();
    let Ok(dir) = fs::read_dir("/var/lib/dpkg/info") else {
        return DpkgIndex { desktop_owners, binary_owners };
    };
    for entry in dir.flatten() {
        let p = entry.path();
        let Some(filename) = p.file_name().and_then(|s| s.to_str()) else { continue };
        let Some(package) = filename.strip_suffix(".list") else { continue };
        let Ok(content) = fs::read_to_string(&p) else { continue };
        for line in content.lines() {
            if line.ends_with(".desktop") && line.contains("/applications/") {
                desktop_owners.insert(line.to_string(), package.to_string());
            } else if line.starts_with("/usr/bin/")
                || line.starts_with("/usr/games/")
                || line.starts_with("/bin/")
                || line.starts_with("/opt/")
            {
                insert_usr_merged_alias(&mut binary_owners, line, package);
            }
        }
    }
    DpkgIndex { desktop_owners, binary_owners }
}

#[derive(Clone, Debug)]
struct DpkgMeta {
    version: String,
    size_kib: Option<u64>,
    essential: bool,
    section: String,
    summary: String,
}

fn dpkg_metadata() -> HashMap<String, DpkgMeta> {
    let mut info = HashMap::new();
    let Ok(out) = Command::new("dpkg-query")
        .args([
            "-W",
            "-f=${db:Status-Abbrev}\t${binary:Package}\t${Version}\t${Installed-Size}\t${Essential}\t${Section}\t${binary:Summary}\n",
        ])
        .output()
    else {
        return info;
    };
    if !out.status.success() {
        return info;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 7 || !cols[0].starts_with("ii") {
            continue;
        }
        let meta = DpkgMeta {
            version: cols[2].to_string(),
            size_kib: cols[3].parse::<u64>().ok(),
            essential: cols[4].eq_ignore_ascii_case("yes"),
            section: cols[5].to_string(),
            summary: cols[6].to_string(),
        };
        info.insert(cols[1].to_string(), meta.clone());
        if let Some(name) = cols[1].split(':').next() {
            info.entry(name.to_string()).or_insert(meta);
        }
    }
    info
}

/// Read `/var/lib/apt/extended_states` to determine packages auto-installed as dependencies.
fn apt_auto_packages() -> HashSet<String> {
    let mut auto_pkgs = HashSet::new();
    let Ok(content) = fs::read_to_string("/var/lib/apt/extended_states") else {
        return auto_pkgs;
    };
    let mut cur_pkg = String::new();
    let mut cur_arch = String::new();
    let mut is_auto = false;
    for line in content.lines().chain(std::iter::once("")) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if is_auto && !cur_pkg.is_empty() {
                auto_pkgs.insert(cur_pkg.clone());
                if !cur_arch.is_empty() {
                    auto_pkgs.insert(format!("{cur_pkg}:{cur_arch}"));
                }
            }
            cur_pkg.clear();
            cur_arch.clear();
            is_auto = false;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Package:") {
            cur_pkg = rest.trim().to_string();
        } else if let Some(rest) = trimmed.strip_prefix("Architecture:") {
            cur_arch = rest.trim().to_string();
        } else if let Some(rest) = trimmed.strip_prefix("Auto-Installed:") {
            is_auto = rest.trim() == "1";
        }
    }
    auto_pkgs
}

pub fn protection_status(pkg_id: &str, essential_flag: bool) -> (bool, Option<String>) {
    if essential_flag {
        return (true, Some("dpkg Essential 系统必需核心包".into()));
    }
    let base = pkg_id.split(':').next().unwrap_or(pkg_id);
    const CORE_PROTECTED: &[&str] = &[
        "apt",
        "dpkg",
        "sudo",
        "systemd",
        "systemd-sysv",
        "ubuntu-desktop",
        "ubuntu-desktop-minimal",
        "ubuntu-minimal",
        "ubuntu-standard",
        "gnome-shell",
        "gnome-session",
        "gdm3",
        "libc6",
        "base-files",
        "bash",
        "coreutils",
        "login",
        "passwd",
        "util-linux",
        "mount",
        "network-manager",
        "polkitd",
        "pkexec",
        "policykit-1",
        "udev",
        "init",
        "dbus",
        "xwayland",
    ];
    if CORE_PROTECTED.contains(&base)
        || base.starts_with("linux-image-")
        || base.starts_with("grub-")
    {
        return (true, Some("操作系统或 GNOME 桌面核心组件".into()));
    }
    (false, None)
}

fn dir_size_kib_bounded(root: &Path, limit: &mut usize) -> u64 {
    if *limit == 0 || !root.exists() {
        return 0;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    let mut bytes = 0u64;
    for entry in entries.flatten() {
        if *limit == 0 {
            break;
        }
        *limit -= 1;
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_file() {
            bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        } else if ft.is_dir() {
            bytes += dir_size_kib_bounded(&entry.path(), limit) * 1024;
        }
    }
    bytes / 1024
}

fn discover_user_data(source: Source, id: &str) -> Vec<UserDataPath> {
    let Some(h) = home() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    match source {
        Source::Flatpak => {
            let p = h.join(".var/app").join(id);
            if p.is_dir() && !p.is_symlink() {
                let mut budget = 1500;
                let kib = dir_size_kib_bounded(&p, &mut budget);
                found.push(UserDataPath {
                    label: "Flatpak 沙盒用户数据".into(),
                    path: p.display().to_string(),
                    size_kib: Some(kib),
                    safe_to_trash: true,
                });
            }
        }
        Source::Snap => {
            let p = h.join("snap").join(id);
            if p.is_dir() && !p.is_symlink() {
                let mut budget = 1500;
                let kib = dir_size_kib_bounded(&p, &mut budget);
                found.push(UserDataPath {
                    label: "Snap 用户数据目录".into(),
                    path: p.display().to_string(),
                    size_kib: Some(kib),
                    safe_to_trash: true,
                });
            }
        }
        Source::Apt | Source::AppImage | Source::Manual => {
            let clean = id
                .split(':')
                .next()
                .unwrap_or(id)
                .trim_end_matches("-bin")
                .trim_end_matches("-stable");
            if clean.len() >= 3 && !clean.contains('/') {
                for (label_name, sub) in [
                    ("用户配置 (~/.config)", ".config"),
                    ("本地数据 (~/.local/share)", ".local/share"),
                    ("应用缓存 (~/.cache)", ".cache"),
                ] {
                    let candidate = h.join(sub).join(clean);
                    if candidate.is_dir() && !candidate.is_symlink() {
                        let mut budget = 800;
                        let kib = dir_size_kib_bounded(&candidate, &mut budget);
                        found.push(UserDataPath {
                            label: label_name.into(),
                            path: candidate.display().to_string(),
                            size_kib: Some(kib),
                            safe_to_trash: false,
                        });
                    }
                }
            }
        }
    }
    found
}

#[derive(Clone)]
struct PairedLauncher {
    launcher_path: String,
    meta: DesktopMeta,
}

pub fn parse_human_size_kib(raw: &str) -> Option<u64> {
    let cleaned = raw.replace('\u{a0}', " ");
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    let split_idx = trimmed
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != ',')
        .unwrap_or(trimmed.len());
    let num_str = trimmed[..split_idx].trim().replace(',', ".");
    let unit = trimmed[split_idx..].trim().to_ascii_lowercase();
    let val = num_str.parse::<f64>().ok()?;
    let kib = match unit.as_str() {
        "b" | "bytes" => val / 1024.0,
        "kb" => val * 1000.0 / 1024.0,
        "kib" | "k" | "" => val,
        "mb" => val * 1_000_000.0 / 1024.0,
        "mib" | "m" => val * 1024.0,
        "gb" => val * 1_000_000_000.0 / 1024.0,
        "gib" | "g" => val * 1024.0 * 1024.0,
        _ => return None,
    };
    Some(kib.round() as u64)
}

/// Check ELF + AppImage magic header bytes without executing the binary.
pub fn is_verified_appimage(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut header = [0u8; 16];
    let Ok(read) = file.read(&mut header) else {
        return false;
    };
    if read < 12 || &header[0..4] != b"\x7fELF" {
        return false;
    }
    &header[8..10] == b"AI" || read == 16
}

fn extract_version_from_filename(stem: &str) -> Option<String> {
    for part in stem.split(['-', '_', ' ']) {
        let trimmed = part.strip_prefix('v').or_else(|| part.strip_prefix('V')).unwrap_or(part);
        if trimmed.chars().next().is_some_and(|c| c.is_ascii_digit())
            && trimmed.contains('.')
            && trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
        {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn resolve_apt_owner<'a>(
    index: &'a DpkgIndex,
    launcher_path: &str,
    exec_path: &str,
) -> Option<(&'a String, &'static str)> {
    if let Some(pkg) = index.desktop_owners.get(launcher_path) {
        return Some((pkg, "dpkg 文件清单直接匹配 (.desktop)"));
    }
    if !exec_path.is_empty() {
        if let Some(pkg) = index.binary_owners.get(exec_path) {
            return Some((pkg, "通过可执行文件路径匹配 dpkg 软件包"));
        }
        if let Ok(canon) = Path::new(exec_path).canonicalize() {
            let canon_str = canon.to_string_lossy().to_string();
            if let Some(pkg) = index.binary_owners.get(&canon_str) {
                return Some((pkg, "解析软链接/alternatives 后匹配 dpkg 软件包"));
            }
        }
    }
    None
}

fn scan_desktop_and_apt(
    opts: &ScanOptions,
    appimage_launchers: &mut HashMap<PathBuf, PairedLauncher>,
) -> Vec<AppEntry> {
    let index = dpkg_index();
    let dpkg_meta = dpkg_metadata();
    let mut apps = Vec::new();
    let mut seen_apt_pkgs = HashSet::new();

    let mut dirs = vec![
        (PathBuf::from("/usr/share/applications"), InstallScope::System),
        (PathBuf::from("/usr/local/share/applications"), InstallScope::System),
    ];
    if let Some(root) = home() {
        dirs.push((root.join(".local/share/applications"), InstallScope::User));
    }

    for (dir, default_scope) in dirs {
        for file in desktop_files(&dir) {
            let Ok(content) = fs::read_to_string(&file) else { continue };
            let Some(meta) = desktop_parse(&content) else { continue };

            if detect_flatpak_from_exec(&meta).is_some() || detect_snap_from_exec(&meta).is_some() {
                continue;
            }

            let launcher_str = file.to_string_lossy().to_string();
            if let Some(appimg_target) = detect_appimage_target(&meta) {
                if appimg_target.is_file() {
                    if let Ok(canon) = appimg_target.canonicalize() {
                        appimage_launchers.insert(
                            canon,
                            PairedLauncher {
                                launcher_path: launcher_str,
                                meta,
                            },
                        );
                        continue;
                    }
                } else if default_scope == InstallScope::User {
                    apps.push(AppEntry {
                        id: file
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        name: meta.name,
                        generic_name: meta.generic_name,
                        summary: meta.comment,
                        version: if meta.appimage_version.is_empty() {
                            "—".into()
                        } else {
                            meta.appimage_version
                        },
                        source: Source::Manual,
                        scope: InstallScope::User,
                        path: String::new(),
                        launcher: launcher_str,
                        icon: meta.icon,
                        command: meta.exec,
                        categories: meta.categories,
                        size_kib: None,
                        size_note: String::new(),
                        confidence: "失效的用户桌面快捷方式（目标 AppImage 已不存在，可安全清理快捷方式）".into(),
                        is_protected: false,
                        protection_reason: None,
                        is_cli: false,
                        broken_launcher: true,
                        duplicate_sources: Vec::new(),
                        user_data: Vec::new(),
                    });
                    continue;
                }
            }

            let (exec_path, exec_missing) = executable_from_desktop(&meta.exec, &meta.try_exec);
            let owner_match = resolve_apt_owner(&index, &launcher_str, &exec_path);

            if let Some((pkg_name, reason)) = owner_match {
                seen_apt_pkgs.insert(pkg_name.clone());
                if let Some(base) = pkg_name.split(':').next() {
                    seen_apt_pkgs.insert(base.to_string());
                }
                let pkg_info = dpkg_meta.get(pkg_name);
                let version = pkg_info
                    .map(|m| m.version.clone())
                    .unwrap_or_else(|| "—".into());
                let size_kib = pkg_info.and_then(|m| m.size_kib);
                let essential = pkg_info.is_some_and(|m| m.essential);
                let summary = if !meta.comment.is_empty() {
                    meta.comment.clone()
                } else {
                    pkg_info.map(|m| m.summary.clone()).unwrap_or_default()
                };
                let (is_protected, protection_reason) = protection_status(pkg_name, essential);
                let user_data = discover_user_data(Source::Apt, pkg_name);
                apps.push(AppEntry {
                    id: pkg_name.clone(),
                    name: meta.name,
                    generic_name: meta.generic_name,
                    summary,
                    version,
                    source: Source::Apt,
                    scope: InstallScope::System,
                    path: exec_path,
                    launcher: launcher_str,
                    icon: meta.icon,
                    command: meta.exec,
                    categories: meta.categories,
                    size_kib,
                    size_note: if size_kib.is_some() {
                        "dpkg 报告的解包体积 (Installed-Size)".into()
                    } else {
                        String::new()
                    },
                    confidence: reason.into(),
                    is_protected,
                    protection_reason,
                    is_cli: false,
                    broken_launcher: false,
                    duplicate_sources: Vec::new(),
                    user_data,
                });
            } else {
                let stem = file
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let confidence = if exec_missing && default_scope == InstallScope::User {
                    "失效的用户快捷方式（目标执行程序不存在，可清理快捷方式）"
                } else if exec_path.starts_with("/opt/") {
                    "位于 /opt 的第三方或独立安装程序（无 dpkg 包归属）"
                } else if default_scope == InstallScope::User {
                    "用户目录下的本地桌面快捷方式（~/.local/share/applications）"
                } else {
                    "系统桌面入口未关联 dpkg 包清单，可能由脚本或源码手动安装"
                };
                let user_data = discover_user_data(Source::Manual, &stem);
                apps.push(AppEntry {
                    id: stem,
                    name: meta.name,
                    generic_name: meta.generic_name,
                    summary: meta.comment,
                    version: "—".into(),
                    source: Source::Manual,
                    scope: default_scope,
                    path: exec_path,
                    launcher: launcher_str,
                    icon: meta.icon,
                    command: meta.exec,
                    categories: meta.categories,
                    size_kib: None,
                    size_note: String::new(),
                    confidence: confidence.into(),
                    is_protected: false,
                    protection_reason: None,
                    is_cli: false,
                    broken_launcher: exec_missing && default_scope == InstallScope::User,
                    duplicate_sources: Vec::new(),
                    user_data,
                });
            }
        }
    }

    if opts.include_cli {
        let auto_pkgs = apt_auto_packages();
        const SKIP_SECTIONS: &[&str] = &[
            "libs",
            "oldlibs",
            "libdevel",
            "doc",
            "localization",
            "translations",
            "kernel",
            "metapackages",
            "fonts",
            "introspection",
        ];
        for (pkg, meta) in &dpkg_meta {
            if pkg.contains(':') && dpkg_meta.contains_key(pkg.split(':').next().unwrap_or("")) {
                continue;
            }
            let base = pkg.split(':').next().unwrap_or(pkg);
            if seen_apt_pkgs.contains(pkg) || seen_apt_pkgs.contains(base) {
                continue;
            }
            if auto_pkgs.contains(pkg) || auto_pkgs.contains(base) {
                continue;
            }
            let sec_tail = meta.section.rsplit('/').next().unwrap_or(&meta.section);
            if SKIP_SECTIONS.contains(&sec_tail)
                || base.starts_with("lib")
                || base.starts_with("gir1.2-")
                || base.ends_with("-dev")
                || base.ends_with("-common")
                || base.ends_with("-data")
            {
                continue;
            }
            let bin_candidate = format!("/usr/bin/{base}");
            let path = if Path::new(&bin_candidate).is_file() {
                bin_candidate
            } else {
                String::new()
            };
            let (is_protected, protection_reason) = protection_status(pkg, meta.essential);
            apps.push(AppEntry {
                id: pkg.clone(),
                name: base.to_string(),
                generic_name: "命令行 / 系统软件包".into(),
                summary: meta.summary.clone(),
                version: meta.version.clone(),
                source: Source::Apt,
                scope: InstallScope::System,
                path: path.clone(),
                launcher: String::new(),
                icon: "utilities-terminal-symbolic".into(),
                command: base.to_string(),
                categories: vec![sec_tail.to_string()],
                size_kib: meta.size_kib,
                size_note: "dpkg 报告的解包体积 (Installed-Size)".into(),
                confidence: "专家模式：手动安装的 APT 软件包".into(),
                is_protected,
                protection_reason,
                is_cli: true,
                broken_launcher: false,
                duplicate_sources: Vec::new(),
                user_data: Vec::new(),
            });
        }
    }

    apps
}

#[derive(Clone)]
struct SnapRecord {
    version: String,
    rev: String,
    notes: String,
}

fn is_snap_runtime_or_base(name: &str, notes: &str) -> bool {
    notes.split(',').any(|n| matches!(n.trim(), "base" | "snapd" | "gadget" | "kernel"))
        || matches!(name, "bare" | "snapd" | "gtk-common-themes" | "snap-store-proxy")
        || name.starts_with("core")
        || name.starts_with("gnome-")
        || name.starts_with("kf5-")
        || name.starts_with("kf6-")
        || name.starts_with("mesa-")
}

fn snap_squashfs_size_kib(pkg: &str, rev: &str) -> Option<u64> {
    if rev.is_empty() {
        return None;
    }
    let snap_file = PathBuf::from(format!("/var/lib/snapd/snaps/{pkg}_{rev}.snap"));
    fs::metadata(snap_file).ok().map(|m| m.len() / 1024)
}

fn normalize_snap_icon(raw_icon: &str, pkg: &str) -> String {
    if raw_icon.contains("${SNAP}") {
        let expanded = raw_icon.replace("${SNAP}", &format!("/snap/{pkg}/current"));
        if Path::new(&expanded).is_file() {
            return expanded;
        }
    }
    raw_icon.to_string()
}

fn snap_apps(opts: &ScanOptions) -> Vec<AppEntry> {
    let Ok(output) = Command::new("snap").arg("list").output() else {
        return vec![];
    };
    if !output.status.success() {
        return vec![];
    }
    let mut records: HashMap<String, SnapRecord> = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 3 {
            let notes = cols.get(5).copied().unwrap_or("").to_string();
            records.insert(
                cols[0].to_string(),
                SnapRecord {
                    version: cols[1].to_string(),
                    rev: cols[2].to_string(),
                    notes,
                },
            );
        }
    }

    let mut matched_pkgs = HashSet::new();
    let mut items = Vec::new();
    for file in desktop_files(Path::new("/var/lib/snapd/desktop/applications")) {
        let Ok(content) = fs::read_to_string(&file) else { continue };
        let Some(meta) = desktop_parse(&content) else { continue };
        let stem = file.file_stem().unwrap_or_default().to_string_lossy();
        let pkg = if !meta.snap_name.is_empty() {
            meta.snap_name.as_str()
        } else {
            stem.split('_').next().unwrap_or("")
        };
        let Some(rec) = records.get(pkg) else { continue };
        matched_pkgs.insert(pkg.to_string());
        let size_kib = snap_squashfs_size_kib(pkg, &rec.rev);
        let user_data = discover_user_data(Source::Snap, pkg);
        items.push(AppEntry {
            id: pkg.to_string(),
            name: meta.name,
            generic_name: meta.generic_name,
            summary: meta.comment,
            version: rec.version.clone(),
            source: Source::Snap,
            scope: InstallScope::System,
            path: format!("/snap/{pkg}/current"),
            launcher: file.display().to_string(),
            icon: normalize_snap_icon(&meta.icon, pkg),
            command: meta.exec,
            categories: meta.categories,
            size_kib,
            size_note: if size_kib.is_some() {
                "Snap 压缩包镜像体积 (.snap)".into()
            } else {
                String::new()
            },
            confidence: "snap list 与 snapd 桌面启动器匹配".into(),
            is_protected: pkg == "snapd",
            protection_reason: if pkg == "snapd" {
                Some("Snap 核心守护组件".into())
            } else {
                None
            },
            is_cli: false,
            broken_launcher: false,
            duplicate_sources: Vec::new(),
            user_data,
        });
    }

    if opts.include_cli {
        for (pkg, rec) in &records {
            if matched_pkgs.contains(pkg) || is_snap_runtime_or_base(pkg, &rec.notes) {
                continue;
            }
            let size_kib = snap_squashfs_size_kib(pkg, &rec.rev);
            let user_data = discover_user_data(Source::Snap, pkg);
            items.push(AppEntry {
                id: pkg.clone(),
                name: pkg.clone(),
                generic_name: "Snap 命令行包".into(),
                summary: String::new(),
                version: rec.version.clone(),
                source: Source::Snap,
                scope: InstallScope::System,
                path: format!("/snap/{pkg}/current"),
                launcher: String::new(),
                icon: "utilities-terminal-symbolic".into(),
                command: format!("/snap/bin/{pkg}"),
                categories: Vec::new(),
                size_kib,
                size_note: if size_kib.is_some() {
                    "Snap 压缩包镜像体积 (.snap)".into()
                } else {
                    String::new()
                },
                confidence: "专家模式：snap list 已安装命令行组件".into(),
                is_protected: false,
                protection_reason: None,
                is_cli: true,
                broken_launcher: false,
                duplicate_sources: Vec::new(),
                user_data,
            });
        }
    }

    items
}

fn find_flatpak_exported_icon(id: &str, user_scope: bool) -> Option<String> {
    let mut export_roots = Vec::new();
    if user_scope {
        if let Some(h) = home() {
            export_roots.push(h.join(".local/share/flatpak/exports/share/icons/hicolor"));
        }
    }
    export_roots.push(PathBuf::from("/var/lib/flatpak/exports/share/icons/hicolor"));
    for root in export_roots {
        for size_dir in ["scalable/apps", "128x128/apps", "64x64/apps", "256x256/apps"] {
            for ext in ["svg", "png"] {
                let candidate = root.join(size_dir).join(format!("{id}.{ext}"));
                if candidate.is_file() {
                    return Some(candidate.display().to_string());
                }
            }
        }
    }
    None
}

fn find_flatpak_exported_desktop(id: &str, user_scope: bool) -> Option<(PathBuf, DesktopMeta)> {
    let mut candidates = Vec::new();
    if user_scope {
        if let Some(h) = home() {
            candidates.push(
                h.join(".local/share/flatpak/exports/share/applications")
                    .join(format!("{id}.desktop")),
            );
        }
    }
    candidates.push(
        PathBuf::from("/var/lib/flatpak/exports/share/applications").join(format!("{id}.desktop")),
    );
    for path in candidates {
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Some(meta) = desktop_parse(&raw) {
                return Some((path, meta));
            }
        }
    }
    None
}

fn flatpak_apps() -> Vec<AppEntry> {
    let output = Command::new("flatpak")
        .args([
            "list",
            "--app",
            "--columns=application,name,version,installation,size,origin",
        ])
        .output()
        .or_else(|_| {
            Command::new("flatpak")
                .args(["list", "--app", "--columns=application,name,version,installation"])
                .output()
        });
    let Ok(output) = output else {
        return vec![];
    };
    if !output.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 4 || cols[0].trim().is_empty() || !cols[0].contains('.') {
                return None;
            }
            let id = cols[0].trim().to_string();
            let user_scope = cols[3].trim() == "user";
            let scope = if user_scope {
                InstallScope::User
            } else {
                InstallScope::System
            };
            let location = if user_scope {
                home().map(|h| h.join(".local/share/flatpak/app").join(&id))
            } else {
                Some(PathBuf::from("/var/lib/flatpak/app").join(&id))
            };
            let path = location.map(|p| p.display().to_string()).unwrap_or_default();
            let size_kib = cols.get(4).and_then(|s| parse_human_size_kib(s));
            let origin = cols.get(5).map(|s| s.trim()).unwrap_or("");

            let exported = find_flatpak_exported_desktop(&id, user_scope);
            let name = exported
                .as_ref()
                .map(|(_, m)| m.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| {
                    if cols[1].trim().is_empty() {
                        id.clone()
                    } else {
                        cols[1].trim().to_string()
                    }
                });
            let generic_name = exported
                .as_ref()
                .map(|(_, m)| m.generic_name.clone())
                .unwrap_or_default();
            let summary = exported
                .as_ref()
                .map(|(_, m)| m.comment.clone())
                .unwrap_or_default();
            let categories = exported
                .as_ref()
                .map(|(_, m)| m.categories.clone())
                .unwrap_or_default();
            let launcher = exported
                .as_ref()
                .map(|(p, _)| p.display().to_string())
                .unwrap_or_default();
            let icon = find_flatpak_exported_icon(&id, user_scope).unwrap_or_else(|| id.clone());
            let confidence = if origin.is_empty() {
                format!("flatpak list --app ({})", scope.label())
            } else {
                format!("flatpak list --app · 源 {origin} ({})", scope.label())
            };
            let user_data = discover_user_data(Source::Flatpak, &id);

            Some(AppEntry {
                name,
                generic_name,
                summary,
                version: if cols[2].trim().is_empty() {
                    "—".into()
                } else {
                    cols[2].trim().to_string()
                },
                id: id.clone(),
                source: Source::Flatpak,
                scope,
                path,
                launcher,
                icon,
                command: format!("flatpak run {id}"),
                categories,
                size_kib,
                size_note: if size_kib.is_some() {
                    "Flatpak 报告的应用本体体积（不含共享 Runtime）".into()
                } else {
                    String::new()
                },
                confidence,
                is_protected: false,
                protection_reason: None,
                is_cli: false,
                broken_launcher: false,
                duplicate_sources: Vec::new(),
                user_data,
            })
        })
        .collect()
}

fn appimages(
    opts: &ScanOptions,
    paired_launchers: &mut HashMap<PathBuf, PairedLauncher>,
) -> Vec<AppEntry> {
    let mut roots = Vec::new();
    if let Some(h) = home() {
        for sub in ["Applications", "应用程序", ".local/bin", "bin"] {
            roots.push(h.join(sub));
        }
        if opts.scan_downloads_appimage {
            for sub in ["Downloads", "下载"] {
                roots.push(h.join(sub));
            }
        }
    }
    for custom in &opts.custom_appimage_dirs {
        roots.push(PathBuf::from(custom));
    }

    let mut visited_files = HashSet::new();
    let mut apps = Vec::new();
    for root in roots {
        scan_appimage_dir(&root, 2, &mut visited_files, paired_launchers, &mut apps);
    }

    // Also include AppImages referenced by user .desktop files even if stored outside default dirs.
    for (canon_path, paired) in paired_launchers.drain() {
        if visited_files.insert(canon_path.clone()) && canon_path.is_file() {
            apps.push(build_appimage_entry(&canon_path, Some(paired)));
        }
    }
    apps
}

fn build_appimage_entry(path: &Path, paired: Option<PairedLauncher>) -> AppEntry {
    let raw_stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let verified = is_verified_appimage(path);
    let size_kib = path.metadata().ok().map(|m| m.len() / 1024);
    let (name, generic_name, summary, version, launcher, icon, categories) =
        if let Some(p) = paired {
            let ver = if !p.meta.appimage_version.is_empty() {
                p.meta.appimage_version
            } else {
                extract_version_from_filename(&raw_stem).unwrap_or_else(|| "—".into())
            };
            let ic = if p.meta.icon.is_empty() {
                "application-x-executable".into()
            } else {
                p.meta.icon
            };
            (
                p.meta.name,
                p.meta.generic_name,
                p.meta.comment,
                ver,
                p.launcher_path,
                ic,
                p.meta.categories,
            )
        } else {
            let ver = extract_version_from_filename(&raw_stem).unwrap_or_else(|| "—".into());
            (
                raw_stem.clone(),
                "便携式 AppImage 应用".into(),
                String::new(),
                ver,
                String::new(),
                "application-x-executable".into(),
                Vec::new(),
            )
        };
    let confidence = if verified {
        if launcher.is_empty() {
            "已校验 ELF/AppImage 文件头签名".into()
        } else {
            "已校验 AppImage 文件头并关联桌面快捷方式".into()
        }
    } else {
        "后缀为 .AppImage（未检测到标准 ELF 头，请谨慎核实）".into()
    };
    let user_data = discover_user_data(Source::AppImage, &raw_stem);
    AppEntry {
        id: raw_stem,
        name,
        generic_name,
        summary,
        version,
        source: Source::AppImage,
        scope: InstallScope::User,
        path: path.display().to_string(),
        launcher,
        icon,
        command: path.display().to_string(),
        categories,
        size_kib,
        size_note: "AppImage 单文件实际磁盘体积".into(),
        confidence,
        is_protected: false,
        protection_reason: None,
        is_cli: false,
        broken_launcher: false,
        duplicate_sources: Vec::new(),
        user_data,
    }
}

fn scan_appimage_dir(
    root: &Path,
    depth: usize,
    visited: &mut HashSet<PathBuf>,
    paired_launchers: &mut HashMap<PathBuf, PairedLauncher>,
    apps: &mut Vec<AppEntry>,
) {
    if depth == 0 || apps.len() >= 500 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            scan_appimage_dir(&path, depth - 1, visited, paired_launchers, apps);
        } else if path.is_file()
            && path
                .file_name()
                .map(|s| s.to_string_lossy().to_ascii_lowercase().ends_with(".appimage"))
                .unwrap_or(false)
        {
            let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !visited.insert(canon.clone()) {
                continue;
            }
            let paired = paired_launchers.remove(&canon);
            apps.push(build_appimage_entry(&canon, paired));
        }
    }
}

fn canonical_app_key(app: &AppEntry) -> String {
    let name_norm: String = app
        .name
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    if name_norm.len() >= 2 {
        return name_norm;
    }
    app.id
        .rsplit('.')
        .next()
        .unwrap_or(&app.id)
        .split(':')
        .next()
        .unwrap_or(&app.id)
        .to_lowercase()
}

fn annotate_multi_source_duplicates(apps: &mut [AppEntry]) {
    let mut by_key: HashMap<String, Vec<Source>> = HashMap::new();
    for app in apps.iter() {
        let key = canonical_app_key(app);
        if key.is_empty() {
            continue;
        }
        let list = by_key.entry(key).or_default();
        if !list.contains(&app.source) {
            list.push(app.source);
        }
    }
    for app in apps.iter_mut() {
        let key = canonical_app_key(app);
        if let Some(sources) = by_key.get(&key) {
            if sources.len() > 1 {
                app.duplicate_sources = sources.iter().map(|s| s.label().to_string()).collect();
            }
        }
    }
}

/// Default inventory scan reading user settings if present.
pub fn scan_apps() -> Vec<AppEntry> {
    let settings = Settings::load();
    scan_apps_with_options(&ScanOptions::from_settings(&settings))
}

/// Configurable inventory scan: never elevates privileges, modifies packages, or deletes files.
pub fn scan_apps_with_options(opts: &ScanOptions) -> Vec<AppEntry> {
    let mut paired_launchers = HashMap::new();
    let mut apps = scan_desktop_and_apt(opts, &mut paired_launchers);
    apps.extend(snap_apps(opts));
    apps.extend(flatpak_apps());
    apps.extend(appimages(opts, &mut paired_launchers));

    let mut seen = HashSet::new();
    apps.retain(|app| {
        seen.insert((
            app.source.as_key(),
            app.id.clone(),
            app.launcher.clone(),
            app.path.clone(),
        ))
    });
    annotate_multi_source_duplicates(&mut apps);
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_language_and_flags() {
        let item = desktop_parse(
            "[Desktop Entry]\nType=Application\nName=Example\nName[zh_CN]=示例\nComment[zh_CN]=测试工具\nCategories=Utility;System;\nIcon=example\nExec=example %U\n",
        )
        .unwrap();
        assert_eq!(item.name, "示例");
        assert_eq!(item.comment, "测试工具");
        assert_eq!(item.categories, vec!["Utility", "System"]);
        assert!(desktop_parse("[Desktop Entry]\nType=Application\nName=Hidden\nNoDisplay=true\n").is_none());
        assert!(desktop_parse("[Desktop Entry]\nType=Link\nName=Site\n").is_none());
    }

    #[test]
    fn exec_tokenizer_handles_env_and_quotes() {
        assert_eq!(
            extract_command_token("env BAMF_DESKTOP_FILE_HINT=/foo.desktop /snap/bin/firefox %u").as_deref(),
            Some("/snap/bin/firefox")
        );
        assert_eq!(
            extract_command_token("\"/opt/My App/launcher\" --flag %F").as_deref(),
            Some("/opt/My App/launcher")
        );
        assert_eq!(executable_from_desktop("/does/not/exist %f", ""), (String::new(), true));
        assert_eq!(executable_from_desktop("", ""), (String::new(), false));
    }

    #[test]
    fn detect_flatpak_and_snap_wrappers() {
        let fp = desktop_parse(
            "[Desktop Entry]\nType=Application\nName=GIMP\nExec=/usr/bin/flatpak run --branch=stable org.gimp.GIMP @@u %U @@\n",
        )
        .unwrap();
        assert_eq!(detect_flatpak_from_exec(&fp).as_deref(), Some("org.gimp.GIMP"));

        let sn = desktop_parse(
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=env BAMF=1 /snap/bin/firefox %u\n",
        )
        .unwrap();
        assert_eq!(detect_snap_from_exec(&sn).as_deref(), Some("firefox"));
    }

    #[test]
    fn parse_human_sizes_accurately() {
        assert_eq!(parse_human_size_kib("100 KiB"), Some(100));
        assert_eq!(parse_human_size_kib("1.5 MiB"), Some(1536));
        assert_eq!(parse_human_size_kib("142.4\u{a0}MB"), Some(139063));
        assert_eq!(parse_human_size_kib(""), None);
    }

    #[test]
    fn source_labels_and_protection() {
        assert_eq!(Source::Apt.label(), "APT / DEB");
        assert_eq!(Source::Manual.as_key(), "manual");
        assert!(protection_status("gnome-shell:amd64", false).0);
        assert!(protection_status("custom-tool", true).0);
        assert!(!protection_status("gimp:amd64", false).0);
    }
}
