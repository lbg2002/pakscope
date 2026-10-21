//! Destructive operations stay opt-in: strict identifiers, preview, dry-run, confirmation.
use crate::scanner::{AppEntry, Source};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalMode { KeepConfig, Purge }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Command { program: &'static str, args: Vec<String> },
    AppImage { path: PathBuf },
}

#[derive(Clone, Debug)]
pub struct RemovalPlan {
    pub command: String,
    pub note: &'static str,
    pub action: Action,
    pub requires_apt_preview: bool,
}

fn valid_id(id: &str, source: &Source) -> bool {
    if id.is_empty() || id.len() > 200 || id.starts_with('-') || id.contains("..") { return false; }
    match source {
        Source::Apt => {
            // No apt-get pattern/regex, wildcard, trailing operation modifiers or shell syntax.
            let mut parts = id.split(':');
            let name = parts.next().unwrap_or("");
            let arch = parts.next();
            parts.next().is_none()
                && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ".+-".contains(c))
                && !name.ends_with(['+', '-'])
                && arch.map(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')).unwrap_or(true)
        }
        Source::Snap => id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        Source::Flatpak => id.contains('.') && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
        _ => false,
    }
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn appimage_path(path: &str) -> Result<PathBuf, String> {
    let target = Path::new(path);
    if !target.is_absolute() || !path.to_ascii_lowercase().ends_with(".appimage") {
        return Err("不是有效的 AppImage 绝对路径".into());
    }
    let metadata = std::fs::symlink_metadata(target).map_err(|e| format!("文件无法访问：{e}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("目标不是普通文件或是符号链接，禁止删除".into());
    }
    let real = target.canonicalize().map_err(|e| e.to_string())?;
    let home = std::env::var_os("HOME").map(PathBuf::from)
        .ok_or("无法识别主目录，不允许自动删除")?;
    let roots = ["Applications", "应用程序", ".local/bin", "bin"];
    if !roots.iter().any(|p| home.join(p).canonicalize().is_ok_and(|root| real.starts_with(root))) {
        return Err("AppImage 不在受管理目录内，需手动处理".into());
    }
    Ok(real)
}

pub fn removal_plan(app: &AppEntry, mode: RemovalMode) -> Result<RemovalPlan, String> {
    match app.source {
        Source::Apt | Source::Snap | Source::Flatpak => {
            if !valid_id(&app.id, &app.source) {
                return Err("包标识不安全或无法确认，禁止自动卸载".into());
            }
        }
        Source::Manual => return Err("无法确定安装来源，请先确认安装方式".into()),
        Source::AppImage => {
            let path = appimage_path(&app.path)?;
            return Ok(RemovalPlan {
                command: format!("rm -- {}", quote(&path.to_string_lossy())),
                note: "只删除这一 AppImage 文件；不会自动删除配置、数据或桌面快捷方式。",
                action: Action::AppImage { path },
                requires_apt_preview: false,
            });
        }
    }
    match app.source {
        Source::Apt => {
            let op = if mode == RemovalMode::Purge { "purge" } else { "remove" };
            Ok(RemovalPlan {
                command: format!("sudo apt-get {op} -- {}", quote(&app.id)),
                note: "确认后使用图形提权执行；APT 可能影响关联包，执行前必须通过模拟检查。",
                action: Action::Command {
                    program: "/usr/bin/pkexec",
                    args: vec!["/usr/bin/apt-get".into(), "-y".into(), op.into(), "--".into(), app.id.clone()],
                },
                requires_apt_preview: true,
            })
        }
        Source::Snap => Ok(RemovalPlan {
            command: format!("sudo snap remove {}", quote(&app.id)),
            note: "Snap 不提供与 APT 相同的模拟卸载，本操作可能保留自动快照。",
            action: Action::Command { program: "/usr/bin/pkexec", args: vec!["/usr/bin/snap".into(), "remove".into(), app.id.clone()] },
            requires_apt_preview: false,
        }),
        Source::Flatpak => {
            let home = std::env::var_os("HOME").map(PathBuf::from).ok_or("无法确定主目录")?;
            let user = Path::new(&app.path).starts_with(home.join(".local/share/flatpak/app"));
            let system = Path::new(&app.path).starts_with("/var/lib/flatpak/app");
            if !user && !system { return Err("Flatpak 安装范围不明确，禁止自动卸载".into()); }
            let scope = if user { "--user" } else { "--system" };
            let mut args = vec!["uninstall".into(), scope.into(), "--noninteractive".into(), app.id.clone()];
            let (program, command) = if user {
                ("/usr/bin/flatpak", format!("flatpak uninstall {scope} {}", quote(&app.id)))
            } else {
                args.insert(0, "/usr/bin/flatpak".into());
                ("/usr/bin/pkexec", format!("sudo flatpak uninstall {scope} {}", quote(&app.id)))
            };
            Ok(RemovalPlan {
                command,
                note: "仅卸载指定应用，不自动删除共享运行时；Flatpak 无可靠的预删除影响模拟。",
                action: Action::Command { program, args },
                requires_apt_preview: false,
            })
        }
        _ => unreachable!(),
    }
}

pub fn apt_preview(app: &AppEntry, mode: RemovalMode) -> Result<String, String> {
    if app.source != Source::Apt || !valid_id(&app.id, &app.source) {
        return Err("不是可模拟的 APT 软件包".into());
    }
    let op = if mode == RemovalMode::Purge { "purge" } else { "remove" };
    let output = Command::new("/usr/bin/apt-get")
        .args(["-s", op, "--", &app.id])
        .output().map_err(|e| format!("APT 模拟检查未启动：{e}"))?;
    if !output.status.success() {
        return Err(format!("APT 模拟失败：{}", String::from_utf8_lossy(&output.stderr)));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let affected: Vec<&str> = text.lines()
        .filter(|s| s.starts_with("Remv ") || s.starts_with("Purg ") || s.starts_with("Inst "))
        .collect();
    if affected.is_empty() { return Err("没有检测到可移除的软件包，已阻止执行".into()); }
    Ok(format!("预计受影响的软件包（{}）：\n{}", affected.len(), affected.join("\n")))
}

pub fn execute(plan: &RemovalPlan) -> Result<String, String> {
    match &plan.action {
        Action::AppImage { path } => {
            let checked = appimage_path(&path.to_string_lossy())?;
            if checked != *path { return Err("文件在确认后发生变化，已取消删除".into()); }
            std::fs::remove_file(path).map_err(|e| format!("删除失败：{e}"))?;
            Ok("AppImage 文件已删除".into())
        }
        Action::Command { program, args } => {
            let out = Command::new(program).args(args).output()
                .map_err(|e| format!("命令启动失败：{e}"))?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                Ok(format!("操作完成\n{}", stdout.chars().take(8000).collect::<String>()))
            } else {
                Err(format!("退出状态：{}\n{}\n{}", out.status, stdout, stderr)
                    .chars().take(8000).collect())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(source: Source, id: &str, path: &str) -> AppEntry {
        AppEntry {
            source, id: id.into(), path: path.into(), name: id.into(),
            version: "1".into(), launcher: String::new(), icon: String::new(),
            command: String::new(), size_kib: None, confidence: String::new(),
        }
    }
    #[test]
    fn apt_remove_preview_and_execution_arguments() {
        let plan = removal_plan(&entry(Source::Apt, "gimp:amd64", ""), RemovalMode::KeepConfig).unwrap();
        assert_eq!(plan.command, "sudo apt-get remove -- 'gimp:amd64'");
        assert!(plan.requires_apt_preview);
        assert_eq!(plan.action, Action::Command {
            program: "/usr/bin/pkexec",
            args: vec!["/usr/bin/apt-get", "-y", "remove", "--", "gimp:amd64"].into_iter().map(String::from).collect()
        });
        let purge = removal_plan(&entry(Source::Apt, "gimp", ""), RemovalMode::Purge).unwrap();
        assert!(purge.command.contains(" purge -- "));
    }
    #[test]
    fn reject_untrusted_ids_and_unknown_ownership() {
        for id in ["-y", "*", "a b", "app-", "app+", "foo/../../etc", "x\n-y"] {
            assert!(removal_plan(&entry(Source::Apt, id, ""), RemovalMode::KeepConfig).is_err(), "{id}");
        }
        assert!(removal_plan(&entry(Source::Manual, "misc", "/usr/bin/misc"), RemovalMode::KeepConfig).is_err());
    }
    #[test]
    fn scoped_flatpak_and_snap() {
        let user = std::env::var("HOME").unwrap_or_else(|_| "/home/test".into());
        let plan = removal_plan(&entry(Source::Flatpak, "org.gimp.GIMP", &format!("{user}/.local/share/flatpak/app/org.gimp.GIMP")), RemovalMode::KeepConfig).unwrap();
        assert!(plan.command.contains("--user"));
        assert!(matches!(plan.action, Action::Command { program: "/usr/bin/flatpak", .. }));
        let plan = removal_plan(&entry(Source::Snap, "firefox", ""), RemovalMode::KeepConfig).unwrap();
        assert!(plan.command.contains("snap remove"));
    }
    #[test]
    fn protect_appimage_paths() {
        assert!(removal_plan(&entry(Source::AppImage, "app", "/etc/hosts"), RemovalMode::KeepConfig).is_err());
        assert!(removal_plan(&entry(Source::Flatpak, "org.example.App", "/random/location"), RemovalMode::KeepConfig).is_err());
    }
}
