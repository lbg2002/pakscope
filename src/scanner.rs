use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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
            Self::Manual => "手动 / 未识别",
        }
    }
    pub fn as_key(&self) -> &'static str {
        match self {
            Self::Apt => "apt", Self::Snap => "snap", Self::Flatpak => "flatpak",
            Self::AppImage => "appimage", Self::Manual => "manual",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source: Source,
    pub path: String,
    pub launcher: String,
    pub icon: String,
    pub command: String,
    pub size_kib: Option<u64>,
    pub confidence: String,
}

#[derive(Default)]
struct DesktopMeta {
    name: String,
    chinese_name: String,
    icon: String,
    exec: String,
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
        if section != "[Desktop Entry]" || line.starts_with('#') { continue; }
        let Some((key, val)) = line.split_once('=') else { continue; };
        let val = val.trim();
        match key {
            "Name" => meta.name = val.to_string(),
            "Name[zh_CN]" | "Name[zh]" if !val.is_empty() => meta.chinese_name = val.to_string(),
            "Icon" => meta.icon = val.to_string(),
            "Exec" => meta.exec = val.to_string(),
            "Hidden" => meta.hidden = val.eq_ignore_ascii_case("true"),
            "NoDisplay" => meta.no_display = val.eq_ignore_ascii_case("true"),
            "Type" => meta.is_application = val == "Application",
            _ => {}
        }
    }
    if meta.hidden || meta.no_display || !meta.is_application || meta.name.is_empty() { return None; }
    if !meta.chinese_name.is_empty() { meta.name = meta.chinese_name.clone(); }
    Some(meta)
}

fn desktop_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir).ok().into_iter().flatten()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("desktop"))
        .collect()
}

fn home() -> Option<PathBuf> { std::env::var_os("HOME").map(PathBuf::from) }

/// Read dpkg file lists directly to avoid invoking hundreds of subprocesses.
/// A desktop file not present in the ownership index is never reported as an APT package.
fn dpkg_owners() -> HashMap<String, String> {
    let mut owners = HashMap::new();
    let Ok(dir) = fs::read_dir("/var/lib/dpkg/info") else { return owners; };
    for entry in dir.flatten() {
        let p = entry.path();
        let Some(filename) = p.file_name().and_then(|s| s.to_str()) else { continue; };
        let Some(package) = filename.strip_suffix(".list") else { continue; };
        let Ok(content) = fs::read_to_string(&p) else { continue; };
        for line in content.lines() {
            if line.ends_with(".desktop") && line.contains("/applications/") {
                owners.insert(line.to_string(), package.to_string());
            }
        }
    }
    owners
}

fn dpkg_versions() -> HashMap<String, (String, Option<u64>)> {
    let mut info = HashMap::new();
    let Ok(out) = Command::new("dpkg-query").args([
        "-W", "-f=${db:Status-Abbrev}\\t${binary:Package}\\t${Version}\\t${Installed-Size}\\n"
    ]).output() else { return info; };
    if !out.status.success() { return info; }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 4 || !cols[0].starts_with("ii") { continue; }
        let details = (cols[2].to_string(), cols[3].parse::<u64>().ok());
        info.insert(cols[1].to_string(), details.clone());
        // Files in /var/lib/dpkg/info can omit the native architecture suffix.
        if let Some(name) = cols[1].split(':').next() {
            info.entry(name.to_string()).or_insert(details);
        }
    }
    info
}

/// Resolve an Exec field without executing it. Unknown, wrapped or environment-driven
/// launch commands intentionally yield no claimed installation path.
fn executable_from_desktop(exec: &str) -> String {
    let Some(command) = exec.split_whitespace().next() else { return String::new(); };
    if command.starts_with('\"') || command == "env" || command.starts_with('%') { return String::new(); }
    let p = Path::new(command);
    if p.is_absolute() {
        return if p.is_file() { p.display().to_string() } else { String::new() };
    }
    if command.contains('/') { return String::new(); }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let file = dir.join(command);
            if file.is_file() { return file.display().to_string(); }
        }
    }
    String::new()
}

fn apt_and_manual() -> Vec<AppEntry> {
    let owners = dpkg_owners();
    let versions = dpkg_versions();
    let mut apps = Vec::new();
    let mut dirs = vec![PathBuf::from("/usr/share/applications"), PathBuf::from("/usr/local/share/applications")];
    if let Some(root) = home() { dirs.push(root.join(".local/share/applications")); }
    for dir in dirs {
        for file in desktop_files(&dir) {
            let Ok(content) = fs::read_to_string(&file) else { continue; };
            let Some(meta) = desktop_parse(&content) else { continue; };
            let filename = file.to_string_lossy().to_string();
            let owner = owners.get(&filename);
            let source = if owner.is_some() { Source::Apt } else { Source::Manual };
            let id = owner.cloned().unwrap_or_else(|| file.file_stem().unwrap_or_default().to_string_lossy().to_string());
            let (version, size) = owner.and_then(|o| versions.get(o)).cloned().unwrap_or(("—".to_string(), None));
            apps.push(AppEntry {
                id, name: meta.name, version, source,
                path: executable_from_desktop(&meta.exec),
                launcher: filename, icon: meta.icon, command: meta.exec,
                size_kib: size,
                confidence: if owner.is_some() { "dpkg 文件清单匹配" } else { "未找到 dpkg 文件归属，可能为手动安装" }.into(),
            });
        }
    }
    apps
}

fn snap_apps() -> Vec<AppEntry> {
    let Ok(output) = Command::new("snap").arg("list").output() else { return vec![]; };
    if !output.status.success() { return vec![]; }
    let mut versions = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 2 { versions.insert(cols[0].to_string(), cols[1].to_string()); }
    }
    let mut items = Vec::new();
    for file in desktop_files(Path::new("/var/lib/snapd/desktop/applications")) {
        let Ok(content) = fs::read_to_string(&file) else { continue; };
        let Some(meta) = desktop_parse(&content) else { continue; };
        let stem = file.file_stem().unwrap_or_default().to_string_lossy();
        let pkg = stem.split('_').next().unwrap_or("");
        let Some(version) = versions.get(pkg) else { continue; };
        items.push(AppEntry {
            id: pkg.to_string(), name: meta.name, version: version.clone(), source: Source::Snap,
            path: format!("/snap/{pkg}/current"), launcher: file.display().to_string(),
            icon: meta.icon, command: meta.exec, size_kib: None,
            confidence: "snap list 与桌面启动器匹配".into(),
        });
    }
    items
}

fn flatpak_apps() -> Vec<AppEntry> {
    let Ok(output) = Command::new("flatpak").args(["list", "--app", "--columns=application,name,version,installation"]).output() else { return vec![]; };
    if !output.status.success() { return vec![]; }
    String::from_utf8_lossy(&output.stdout).lines().filter_map(|line| {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 4 || cols[0].trim().is_empty() || !cols[0].contains('.') { return None; }
        let id = cols[0].to_string();
        let location = if cols[3].trim() == "user" {
            home().map(|h| h.join(".local/share/flatpak/app").join(&id))
        } else { Some(PathBuf::from("/var/lib/flatpak/app").join(&id)) };
        let path = location.map(|p| p.display().to_string()).unwrap_or_default();
        Some(AppEntry {
            name: if cols[1].trim().is_empty() { id.clone() } else { cols[1].to_string() },
            version: if cols[2].trim().is_empty() { "—".into() } else { cols[2].to_string() },
            id: id.clone(), source: Source::Flatpak, path, launcher: String::new(),
            icon: id, command: String::new(), size_kib: None,
            confidence: "flatpak list --app 返回的安装记录".into(),
        })
    }).collect()
}

fn appimages() -> Vec<AppEntry> {
    let mut roots = Vec::new();
    if let Some(h) = home() {
        for sub in ["Applications", "应用程序", ".local/bin", "bin"] { roots.push(h.join(sub)); }
    }
    let mut apps = Vec::new();
    for root in roots {
        scan_appimage_dir(&root, 2, &mut apps);
    }
    apps
}
fn scan_appimage_dir(root: &Path, depth: usize, apps: &mut Vec<AppEntry>) {
    if depth == 0 || apps.len() >= 500 { return; }
    let Ok(entries) = fs::read_dir(root) else { return; };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_symlink() { continue; }
        if path.is_dir() { scan_appimage_dir(&path, depth - 1, apps); }
        else if path.is_file() && path.file_name().map(|s| s.to_string_lossy().to_ascii_lowercase().ends_with(".appimage")).unwrap_or(false) {
            let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            let size_kib = path.metadata().ok().map(|m| m.len() / 1024);
            apps.push(AppEntry {
                id: name.clone(), name, version: "—".into(), source: Source::AppImage,
                path: path.display().to_string(), launcher: String::new(), icon: "application-x-executable".into(),
                command: path.display().to_string(), size_kib,
                confidence: "扫描指定目录发现 .AppImage 文件（未验证可执行格式）".into(),
            });
        }
    }
}

/// Inventory only: no elevation, package changes, filesystem writes, or deletion.
pub fn scan_apps() -> Vec<AppEntry> {
    let mut apps = apt_and_manual();
    apps.extend(snap_apps());
    apps.extend(flatpak_apps());
    apps.extend(appimages());
    let mut seen = HashSet::new();
    apps.retain(|app| seen.insert((app.source.as_key(), app.id.clone(), app.launcher.clone())));
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_language_and_flags() {
        let item = desktop_parse("[Desktop Entry]\nType=Application\nName=Example\nName[zh_CN]=示例\nIcon=example\nExec=example %U\n").unwrap();
        assert_eq!(item.name, "示例");
        assert_eq!(item.exec, "example %U");
        assert!(desktop_parse("[Desktop Entry]\nType=Application\nName=Hidden\nNoDisplay=true\n").is_none());
        assert!(desktop_parse("[Desktop Entry]\nType=Link\nName=Site\n").is_none());
    }
    #[test]
    fn executable_resolver_rejects_uncertain_commands() {
        assert_eq!(executable_from_desktop("env FOO=1 app %U"), "");
        assert_eq!(executable_from_desktop("/does/not/exist %f"), "");
        assert_eq!(executable_from_desktop(""), "");
    }
    #[test]
    fn source_labels_match_ui() {
        assert_eq!(Source::Apt.label(), "APT / DEB");
        assert_eq!(Source::Manual.as_key(), "manual");
    }
}
