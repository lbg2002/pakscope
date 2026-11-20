//! Source-aware safe removal & cleanup operations.
//! Enforces strict identifier rules, essential/core protection, APT dry-run verification,
//! optional orphan dependency / sandbox data cleanup, and non-shell process execution.
use crate::scanner::{protection_status, AppEntry, Source};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalMode {
    KeepConfig,
    Purge,
}

#[derive(Clone, Debug)]
pub struct RemovalOptions {
    pub mode: RemovalMode,
    pub apt_autoremove: bool,
    pub snap_purge: bool,
    pub flatpak_delete_data: bool,
    pub appimage_use_trash: bool,
    pub remove_desktop_launcher: bool,
    pub trash_sandbox_data: bool,
    pub custom_appimage_dirs: Vec<String>,
}

impl RemovalOptions {
    pub fn with_mode(mode: RemovalMode) -> Self {
        Self {
            mode,
            apt_autoremove: false,
            snap_purge: mode == RemovalMode::Purge,
            flatpak_delete_data: mode == RemovalMode::Purge,
            appimage_use_trash: true,
            remove_desktop_launcher: true,
            trash_sandbox_data: false,
            custom_appimage_dirs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Command {
        program: &'static str,
        args: Vec<String>,
    },
    AppImage {
        path: PathBuf,
        use_trash: bool,
        allowed_custom_dirs: Vec<String>,
    },
    UserDesktopShortcut {
        path: PathBuf,
        use_trash: bool,
    },
}

#[derive(Clone, Debug)]
pub struct RemovalPlan {
    pub command: String,
    pub note: String,
    pub action: Action,
    pub cleanup_launcher: Option<PathBuf>,
    pub cleanup_sandbox_dirs: Vec<PathBuf>,
    pub requires_apt_preview: bool,
}

#[derive(Clone, Debug)]
pub struct AptSimulation {
    pub summary_text: String,
    pub affected_count: usize,
    pub blocked_critical: Vec<String>,
}

pub fn valid_id(id: &str, source: &Source) -> bool {
    if id.is_empty() || id.len() > 200 || id.starts_with('-') || id.contains("..") {
        return false;
    }
    match source {
        Source::Apt => {
            let mut parts = id.split(':');
            let name = parts.next().unwrap_or("");
            let arch = parts.next();
            parts.next().is_none()
                && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ".+-".contains(c))
                && !name.ends_with('+')
                && !name.ends_with('-')
                && arch
                    .map(|a| {
                        !a.is_empty()
                            && a.chars()
                                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                    })
                    .unwrap_or(true)
        }
        Source::Snap => id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        Source::Flatpak => {
            id.contains('.')
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        }
        _ => false,
    }
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn validate_appimage_path(path: &str, custom_dirs: &[String]) -> Result<PathBuf, String> {
    let target = Path::new(path);
    if !target.is_absolute() || !path.to_ascii_lowercase().ends_with(".appimage") {
        return Err("不是有效的 AppImage 绝对路径".into());
    }
    let metadata =
        std::fs::symlink_metadata(target).map_err(|e| format!("文件无法访问：{e}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("目标不是普通文件或是符号链接，禁止删除".into());
    }
    let real = target.canonicalize().map_err(|e| e.to_string())?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("无法识别主目录，不允许自动删除")?;
    let builtin_roots = ["Applications", "应用程序", ".local/bin", "bin", "Downloads", "下载"];
    let in_builtin = builtin_roots
        .iter()
        .any(|p| home.join(p).canonicalize().is_ok_and(|root| real.starts_with(root)));
    let in_custom = custom_dirs.iter().any(|d| {
        Path::new(d)
            .canonicalize()
            .is_ok_and(|root| real.starts_with(root))
    });
    if !in_builtin && !in_custom {
        return Err("AppImage 不在受管理目录内（可在「设置」中添加自定义目录或手动处理）".into());
    }
    Ok(real)
}

fn validate_user_desktop_launcher(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("未提供桌面启动器路径".into());
    }
    let target = Path::new(path);
    if !target.is_absolute() || target.extension().and_then(|e| e.to_str()) != Some("desktop") {
        return Err("不是有效的 .desktop 启动器文件".into());
    }
    let meta =
        std::fs::symlink_metadata(target).map_err(|e| format!("启动器文件无法访问：{e}"))?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err("启动器不是普通文件或是符号链接，禁止自动移除".into());
    }
    let real = target.canonicalize().map_err(|e| e.to_string())?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("无法确定用户主目录")?;
    let allowed_dir = home
        .join(".local/share/applications")
        .canonicalize()
        .map_err(|_| "用户桌面快捷方式目录不存在".to_string())?;
    if !real.starts_with(&allowed_dir) {
        return Err("该 .desktop 位于系统目录且未知包归属，禁止直接删除".into());
    }
    Ok(real)
}

fn validate_sandbox_dir(path: &str, app: &AppEntry) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let target = Path::new(path);
    let meta = std::fs::symlink_metadata(target).ok()?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return None;
    }
    let real = target.canonicalize().ok()?;
    let expected = match app.source {
        Source::Flatpak => home.join(".var/app").join(&app.id),
        Source::Snap => home.join("snap").join(&app.id),
        _ => return None,
    };
    let expected_canon = expected.canonicalize().ok()?;
    if real == expected_canon {
        Some(real)
    } else {
        None
    }
}

pub fn removal_plan(app: &AppEntry, mode: RemovalMode) -> Result<RemovalPlan, String> {
    removal_plan_with_options(app, &RemovalOptions::with_mode(mode))
}

pub fn removal_plan_with_options(
    app: &AppEntry,
    opts: &RemovalOptions,
) -> Result<RemovalPlan, String> {
    if app.is_protected || protection_status(&app.id, false).0 {
        let reason = app
            .protection_reason
            .as_deref()
            .unwrap_or("操作系统或桌面关键保护包");
        return Err(format!("已启用安全锁：{reason}，禁止通过图形界面卸载。"));
    }

    let cleanup_sandbox_dirs: Vec<PathBuf> = if opts.trash_sandbox_data {
        app.user_data
            .iter()
            .filter(|d| d.safe_to_trash)
            .filter_map(|d| validate_sandbox_dir(&d.path, app))
            .collect()
    } else {
        Vec::new()
    };

    match app.source {
        Source::Apt | Source::Snap | Source::Flatpak => {
            if !valid_id(&app.id, &app.source) {
                return Err("包标识不安全或无法确认，禁止自动卸载".into());
            }
        }
        Source::Manual => {
            let shortcut = validate_user_desktop_launcher(&app.launcher).map_err(|_| {
                "无法确定安装来源，且启动器不在用户个人目录下，请人工核实安装位置后手动处理".to_string()
            })?;
            let cmd = if opts.appimage_use_trash {
                format!("gio trash -- {}", quote(&shortcut.to_string_lossy()))
            } else {
                format!("rm -- {}", quote(&shortcut.to_string_lossy()))
            };
            return Ok(RemovalPlan {
                command: cmd,
                note: "仅清理 ~/.local/share/applications 下的个人快捷方式入口，不会删除未知程序目录或用户文档。".into(),
                action: Action::UserDesktopShortcut {
                    path: shortcut,
                    use_trash: opts.appimage_use_trash,
                },
                cleanup_launcher: None,
                cleanup_sandbox_dirs: Vec::new(),
                requires_apt_preview: false,
            });
        }
        Source::AppImage => {
            let path = validate_appimage_path(&app.path, &opts.custom_appimage_dirs)?;
            let cleanup_launcher = if opts.remove_desktop_launcher && !app.launcher.is_empty() {
                validate_user_desktop_launcher(&app.launcher).ok()
            } else {
                None
            };
            let op_cmd = if opts.appimage_use_trash {
                "gio trash --"
            } else {
                "rm --"
            };
            let mut cmd = format!("{op_cmd} {}", quote(&path.to_string_lossy()));
            if let Some(ref desk) = cleanup_launcher {
                cmd.push_str(&format!("\n{op_cmd} {}", quote(&desk.to_string_lossy())));
            }
            let note = if opts.appimage_use_trash {
                "默认将 AppImage 本体（及勾选的关联快捷方式）安全移入系统回收站，可随时从回收站恢复。"
            } else {
                "将直接永久删除该 AppImage 文件（不经过回收站），不会误删 ~/.config 中的个人数据。"
            };
            return Ok(RemovalPlan {
                command: cmd,
                note: note.into(),
                action: Action::AppImage {
                    path,
                    use_trash: opts.appimage_use_trash,
                    allowed_custom_dirs: opts.custom_appimage_dirs.clone(),
                },
                cleanup_launcher,
                cleanup_sandbox_dirs,
                requires_apt_preview: false,
            });
        }
    }

    match app.source {
        Source::Apt => {
            let op = if opts.mode == RemovalMode::Purge {
                "purge"
            } else {
                "remove"
            };
            let mut args = vec![
                "/usr/bin/apt-get".to_string(),
                "-y".to_string(),
                op.to_string(),
            ];
            let mut cmd = format!("sudo apt-get {op}");
            if opts.apt_autoremove {
                args.push("--auto-remove".to_string());
                cmd.push_str(" --auto-remove");
            }
            args.push("--".to_string());
            args.push(app.id.clone());
            cmd.push_str(&format!(" -- {}", quote(&app.id)));

            Ok(RemovalPlan {
                command: cmd,
                note: "确认后将再次运行 APT 模拟比对依赖，若涉及系统核心包将自动拦截，通过后经 Polkit 提权执行。".into(),
                action: Action::Command {
                    program: "/usr/bin/pkexec",
                    args,
                },
                cleanup_launcher: None,
                cleanup_sandbox_dirs,
                requires_apt_preview: true,
            })
        }
        Source::Snap => {
            let mut args = vec![
                "/usr/bin/snap".to_string(),
                "remove".to_string(),
            ];
            let mut cmd = String::from("sudo snap remove");
            if opts.snap_purge {
                args.push("--purge".to_string());
                cmd.push_str(" --purge");
            }
            args.push(app.id.clone());
            cmd.push_str(&format!(" {}", quote(&app.id)));
            let note = if opts.snap_purge {
                "已启用 --purge：卸载 Snap 应用并直接清除快照数据，释放全部快照空间。"
            } else {
                "标准卸载：Snap 默认保留一份快照以便日后恢复，勾选「不保留快照彻底清理」可释放空间。"
            };
            Ok(RemovalPlan {
                command: cmd,
                note: note.into(),
                action: Action::Command {
                    program: "/usr/bin/pkexec",
                    args,
                },
                cleanup_launcher: None,
                cleanup_sandbox_dirs,
                requires_apt_preview: false,
            })
        }
        Source::Flatpak => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or("无法确定主目录")?;
            let user = Path::new(&app.path).starts_with(home.join(".local/share/flatpak/app"));
            let system = Path::new(&app.path).starts_with("/var/lib/flatpak/app");
            if !user && !system {
                return Err("Flatpak 安装范围不明确，禁止自动卸载".into());
            }
            let scope = if user { "--user" } else { "--system" };
            let mut args = vec![
                "uninstall".to_string(),
                scope.to_string(),
                "--noninteractive".to_string(),
            ];
            let mut flags_str = scope.to_string();
            if opts.flatpak_delete_data {
                args.push("--delete-data".to_string());
                flags_str.push_str(" --delete-data");
            }
            args.push(app.id.clone());
            let (program, command) = if user {
                (
                    "/usr/bin/flatpak",
                    format!("flatpak uninstall {flags_str} {}", quote(&app.id)),
                )
            } else {
                args.insert(0, "/usr/bin/flatpak".into());
                (
                    "/usr/bin/pkexec",
                    format!("sudo flatpak uninstall {flags_str} {}", quote(&app.id)),
                )
            };
            let note = if opts.flatpak_delete_data {
                "卸载指定 Flatpak 应用并同步清除 ~/.var/app 下的专属沙盒数据（保留共享 Runtime）。"
            } else {
                "仅卸载指定 Flatpak 应用本体，保留共享 Runtime 与 ~/.var/app 中的个人配置。"
            };
            Ok(RemovalPlan {
                command,
                note: note.into(),
                action: Action::Command { program, args },
                cleanup_launcher: None,
                cleanup_sandbox_dirs,
                requires_apt_preview: false,
            })
        }
        _ => unreachable!(),
    }
}

pub fn apt_simulate(app: &AppEntry, opts: &RemovalOptions) -> Result<AptSimulation, String> {
    if app.source != Source::Apt || !valid_id(&app.id, &app.source) {
        return Err("不是可模拟的 APT 软件包".into());
    }
    let op = if opts.mode == RemovalMode::Purge {
        "purge"
    } else {
        "remove"
    };
    let mut sim_args = vec!["-s", op];
    if opts.apt_autoremove {
        sim_args.push("--auto-remove");
    }
    sim_args.push("--");
    sim_args.push(&app.id);

    let output = Command::new("/usr/bin/apt-get")
        .args(&sim_args)
        .output()
        .map_err(|e| format!("APT 模拟检查未启动：{e}"))?;
    if !output.status.success() {
        return Err(format!(
            "APT 模拟失败：{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut affected = Vec::new();
    let mut blocked_critical = Vec::new();

    for line in text.lines() {
        if line.starts_with("Remv ") || line.starts_with("Purg ") || line.starts_with("Inst ") {
            affected.push(line.to_string());
            if line.starts_with("Remv ") || line.starts_with("Purg ") {
                if let Some(pkg_token) = line.split_whitespace().nth(1) {
                    if protection_status(pkg_token, false).0 && !blocked_critical.contains(&pkg_token.to_string()) {
                        blocked_critical.push(pkg_token.to_string());
                    }
                }
            }
        }
    }

    if affected.is_empty() {
        return Err("没有检测到可移除的软件包，已阻止执行".into());
    }

    let mut summary = String::new();
    if !blocked_critical.is_empty() {
        summary.push_str(&format!(
            "⛔ 安全拦截：卸载此包将连带移除关键系统/桌面组件 [{}]，已自动锁定执行按钮！\n\n",
            blocked_critical.join(", ")
        ));
    } else if affected.len() > 15 {
        summary.push_str(&format!(
            "⚠️ 注意：本次操作将连带影响 {} 个软件包，请仔细核对下方列表后再确认！\n\n",
            affected.len()
        ));
    }
    summary.push_str(&format!(
        "预计受影响的软件包（共 {} 项）：\n{}",
        affected.len(),
        affected.join("\n")
    ));

    Ok(AptSimulation {
        summary_text: summary,
        affected_count: affected.len(),
        blocked_critical,
    })
}

pub fn apt_preview(app: &AppEntry, mode: RemovalMode) -> Result<String, String> {
    let sim = apt_simulate(app, &RemovalOptions::with_mode(mode))?;
    if !sim.blocked_critical.is_empty() {
        return Err(sim.summary_text);
    }
    Ok(sim.summary_text)
}

fn trash_or_delete_path(path: &Path, use_trash: bool) -> Result<String, String> {
    if use_trash {
        let out = Command::new("gio")
            .args(["trash", "--"])
            .arg(path)
            .output()
            .map_err(|e| format!("无法调用 gio trash（可取消勾选移入回收站改为直接删除）：{e}"))?;
        if out.status.success() {
            Ok(format!("已移入回收站：{}", path.display()))
        } else {
            Err(format!(
                "移入回收站失败：{}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    } else {
        std::fs::remove_file(path).map_err(|e| format!("删除文件失败：{e}"))?;
        Ok(format!("已永久删除：{}", path.display()))
    }
}

pub fn execute(plan: &RemovalPlan) -> Result<String, String> {
    let mut logs = Vec::new();
    match &plan.action {
        Action::AppImage {
            path,
            use_trash,
            allowed_custom_dirs,
        } => {
            let checked = validate_appimage_path(&path.to_string_lossy(), allowed_custom_dirs)?;
            if checked != *path {
                return Err("文件在确认后发生变化，已取消操作".into());
            }
            logs.push(trash_or_delete_path(path, *use_trash)?);
            if let Some(ref launcher) = plan.cleanup_launcher {
                if let Ok(valid_desk) = validate_user_desktop_launcher(&launcher.to_string_lossy()) {
                    if valid_desk == *launcher {
                        if let Ok(msg) = trash_or_delete_path(launcher, *use_trash) {
                            logs.push(format!("关联快捷方式：{msg}"));
                        }
                    }
                }
            }
        }
        Action::UserDesktopShortcut { path, use_trash } => {
            let checked = validate_user_desktop_launcher(&path.to_string_lossy())?;
            if checked != *path {
                return Err("快捷方式路径在确认后发生变化，已取消操作".into());
            }
            logs.push(trash_or_delete_path(path, *use_trash)?);
        }
        Action::Command { program, args } => {
            let out = Command::new(program)
                .args(args)
                .output()
                .map_err(|e| format!("命令启动失败：{e}"))?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !out.status.success() {
                return Err(
                    format!("退出状态：{}\n{}\n{}", out.status, stdout, stderr)
                        .chars()
                        .take(8000)
                        .collect(),
                );
            }
            logs.push(format!(
                "操作完成\n{}",
                stdout.chars().take(7500).collect::<String>()
            ));
        }
    }

    for sandbox in &plan.cleanup_sandbox_dirs {
        if sandbox.exists() && !sandbox.is_symlink() {
            if let Ok(out) = Command::new("gio").args(["trash", "--"]).arg(sandbox).output() {
                if out.status.success() {
                    logs.push(format!("沙盒数据已移入回收站：{}", sandbox.display()));
                }
            }
        }
    }

    Ok(logs.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: Source, id: &str, path: &str) -> AppEntry {
        AppEntry::minimal(source, id, path)
    }

    #[test]
    fn apt_remove_purge_and_autoremove_plans() {
        let plan = removal_plan(&entry(Source::Apt, "gimp:amd64", ""), RemovalMode::KeepConfig).unwrap();
        assert_eq!(plan.command, "sudo apt-get remove -- 'gimp:amd64'");
        assert!(plan.requires_apt_preview);

        let mut opts = RemovalOptions::with_mode(RemovalMode::Purge);
        opts.apt_autoremove = true;
        let purge = removal_plan_with_options(&entry(Source::Apt, "gimp", ""), &opts).unwrap();
        assert_eq!(purge.command, "sudo apt-get purge --auto-remove -- 'gimp'");
    }

    #[test]
    fn block_protected_system_packages() {
        let mut core = entry(Source::Apt, "gnome-shell", "/usr/bin/gnome-shell");
        core.is_protected = true;
        assert!(removal_plan(&core, RemovalMode::KeepConfig).is_err());
        assert!(removal_plan(&entry(Source::Apt, "ubuntu-desktop", ""), RemovalMode::KeepConfig).is_err());
    }

    #[test]
    fn reject_untrusted_ids_and_unknown_ownership() {
        for id in ["-y", "*", "a b", "app-", "app+", "foo/../../etc", "x\n-y"] {
            assert!(
                removal_plan(&entry(Source::Apt, id, ""), RemovalMode::KeepConfig).is_err(),
                "{id}"
            );
        }
        assert!(removal_plan(&entry(Source::Manual, "misc", "/usr/bin/misc"), RemovalMode::KeepConfig).is_err());
    }

    #[test]
    fn scoped_flatpak_and_snap_options() {
        let user = std::env::var("HOME").unwrap_or_else(|_| "/home/test".into());
        let mut opts = RemovalOptions::with_mode(RemovalMode::Purge);
        opts.flatpak_delete_data = true;
        let plan = removal_plan_with_options(
            &entry(
                Source::Flatpak,
                "org.gimp.GIMP",
                &format!("{user}/.local/share/flatpak/app/org.gimp.GIMP"),
            ),
            &opts,
        )
        .unwrap();
        assert!(plan.command.contains("--user --delete-data"));
        assert!(matches!(plan.action, Action::Command { program: "/usr/bin/flatpak", .. }));

        opts.snap_purge = true;
        let snap_plan = removal_plan_with_options(&entry(Source::Snap, "firefox", ""), &opts).unwrap();
        assert!(snap_plan.command.contains("snap remove --purge 'firefox'"));
    }

    #[test]
    fn protect_appimage_paths() {
        assert!(removal_plan(&entry(Source::AppImage, "app", "/etc/hosts"), RemovalMode::KeepConfig).is_err());
        assert!(removal_plan(&entry(Source::Flatpak, "org.example.App", "/random/location"), RemovalMode::KeepConfig).is_err());
    }
}
