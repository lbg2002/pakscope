//! Safe, local package removal plans. No shell evaluation or arbitrary directory deletion.
use crate::scanner::{AppEntry, Source};
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct RemovalPlan {
    pub app: AppEntry,
    pub program: &'static str,
    pub args: Vec<String>,
    pub display: String,
    pub preview: String,
    pub privileged: bool,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b".+_:-".contains(&b))
        && value.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
}

fn shell_arg(value: &str) -> String {
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._:+-=~".contains(&b)) {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\\'', "'\\\\''"))
}

fn display_command(privileged: bool, program: &str, args: &[String]) -> String {
    let mut rendered = if privileged { "sudo ".to_string() } else { String::new() };
    rendered.push_str(program.rsplit('/').next().unwrap_or(program));
    for arg in args {
        rendered.push(' ');
        rendered.push_str(&shell_arg(arg));
    }
    rendered
}

fn protected_package(id: &str) -> bool {
    const PROTECTED: &[&str] = &[
        "apt", "dpkg", "sudo", "systemd", "libc6", "base-files", "ubuntu-minimal",
        "ubuntu-standard", "ubuntu-desktop", "ubuntu-desktop-minimal", "gdm3",
        "gnome-shell", "network-manager", "linux-generic", "linux-image-generic",
    ];
    PROTECTED.contains(&id.split(':').next().unwrap_or(id))
}

fn apt_preview(id: &str) -> Result<String, String> {
    if protected_package(id) {
        return Err("该软件属于受保护的系统组件，无法从 PakScope 卸载。".into());
    }
    let status = Command::new("/usr/bin/dpkg-query").args(["-s", id])
        .output().map_err(|e| format!("无法检查包状态：{e}"))?;
    if !status.status.success() {
        return Err("无法确认 dpkg 包状态，已阻止卸载。".into());
    }
    let state = String::from_utf8_lossy(&status.stdout);
    if state.lines().any(|line| {
        line.eq_ignore_ascii_case("Essential: yes") || line.eq_ignore_ascii_case("Protected: yes")
    }) {
        return Err("这是 Essential / Protected 系统包，禁止在此卸载。".into());
    }
    let output = Command::new("/usr/bin/apt-get").args(["-s", "remove", "--", id])
        .output().map_err(|e| format!("APT 模拟失败：{e}"))?;
    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        return Err(format!("APT 模拟失败：{}", String::from_utf8_lossy(&output.stderr)));
    }
    let removed: Vec<&str> = raw.lines().filter_map(|line| {
        line.strip_prefix("Remv ").and_then(|rest| rest.split_whitespace().next())
    }).collect();
    if removed.len() != 1 || removed[0].split(':').next() != id.split(':').next() {
        return Err(format!(
            "APT 模拟显示卸载目标不唯一或存在依赖连锁删除，已阻止操作。\\n{}",
            raw.chars().take(2200).collect::<String>()
        ));
    }
    if raw.lines().any(|line| line.starts_with("Inst ")) {
        return Err("APT 模拟包含额外安装变更，已阻止卸载。".into());
    }
    Ok(format!(
        "APT 已完成模拟，确认仅卸载目标包。用户配置与数据不由 PakScope 清理。\\n\\n{}",
        raw.chars().take(4500).collect::<String>()
    ))
}

fn appimage_path_allowed(path: &str) -> bool {
    let Some(home) = std::env::var_os("HOME") else { return false; };
    let home = Path::new(&home);
    let path = Path::new(path);
    if !path.is_absolute() || path.extension().and_then(|e| e.to_str())
        .is_none_or(|e| !e.eq_ignore_ascii_case("appimage")) {
        return false;
    }
    let Ok(metadata) = path.symlink_metadata() else { return false; };
    if !metadata.file_type().is_file() { return false; }
    let Ok(real) = path.canonicalize() else { return false; };
    ["Applications", "应用程序", ".local/bin", "bin"].iter().any(|folder| {
        let base = home.join(folder);
        base.canonicalize().is_ok_and(|resolved| real.starts_with(&resolved))
    })
}

/// Build an explicit removal plan. Unsupported or uncertain sources stay read-only.
pub fn prepare(app: &AppEntry) -> Result<RemovalPlan, String> {
    let (program, args, preview, privileged) = match app.source {
        Source::Apt => {
            if !valid_id(&app.id) { return Err("软件包 ID 无效，禁止卸载。".into()); }
            let details = apt_preview(&app.id)?;
            ("/usr/bin/apt-get", vec!["remove".into(), "--".into(), app.id.clone()],
                details, true)
        }
        Source::Snap => {
            if !valid_id(&app.id) { return Err("Snap 名称无效。".into()); }
            ("/usr/bin/snap", vec!["remove".into(), app.id.clone()],
                "Snap 无可靠的原生卸载模拟。确认后由 Snap 管理器处理；可能创建自动快照，其他用户数据未由 PakScope 清理。".into(), true)
        }
        Source::Flatpak => {
            if !valid_id(&app.id) || !app.id.contains('.') { return Err("Flatpak 应用 ID 无效。".into()); }
            let user = app.path.starts_with(
                &std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share/flatpak/app"))
                    .map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
            let system = app.path.starts_with("/var/lib/flatpak/app/");
            if !user && !system { return Err("Flatpak 安装范围不明，无法安全卸载。".into()); }
            let scope = if user { "--user" } else { "--system" };
            ("/usr/bin/flatpak", vec!["uninstall".into(), "--noninteractive".into(),
                scope.into(), app.id.clone()],
                format!("卸载 {} 范围内的 Flatpak 应用，不主动清理用户数据或其他运行时。", if user { "用户" } else { "系统" }), !user)
        }
        Source::AppImage => {
            if !appimage_path_allowed(&app.path) {
                return Err("无法证明 AppImage 位于可管理目录内，禁止移除。".into());
            }
            ("/usr/bin/gio", vec!["trash".into(), app.path.clone()],
                "仅将这一个 AppImage 文件移入回收站，不删除目录、配置或其他桌面入口。".into(), false)
        }
        Source::Manual => return Err("来源尚未确认，请使用对应安装器手动管理。".into()),
    };
    let display = display_command(privileged, program, &args);
    Ok(RemovalPlan { app: app.clone(), program, args, display, preview, privileged })
}

/// Execute only the prepared native-manager command; re-check the plan immediately before APT removal.
pub fn execute(plan: &RemovalPlan) -> Result<String, String> {
    let rechecked = prepare(&plan.app)?;
    if rechecked.program != plan.program || rechecked.args != plan.args
        || rechecked.privileged != plan.privileged {
        return Err("软件安装信息发生变化，请刷新列表后重新确认。".into());
    }
    let mut command = if plan.privileged {
        let mut c = Command::new("/usr/bin/pkexec");
        c.arg(plan.program);
        c
    } else {
        Command::new(plan.program)
    };
    command.args(&plan.args);
    if matches!(plan.app.source, Source::Apt) {
        // The UI already confirmed the simulated single-package removal.
        command.arg("-y");
    }
    let output = command.output().map_err(|e| format!("无法启动包管理器：{e}"))?;
    let report = format!("{}\\n{}", String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(format!("卸载未完成（退出状态：{}）。\\n{}", output.status, report));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validate_package_names() {
        assert!(valid_id("foo:amd64"));
        assert!(valid_id("org.example.App"));
        assert!(!valid_id("foo;rm -rf /"));
        assert!(!valid_id("-danger"));
        assert!(!valid_id(""));
    }
    #[test]
    fn protected_packages_are_blocked() {
        assert!(protected_package("systemd"));
        assert!(protected_package("libc6:amd64"));
        assert!(!protected_package("libreoffice-writer"));
    }
    #[test]
    fn display_quotes_paths() {
        assert_eq!(shell_arg("/tmp/example file.AppImage"), "'/tmp/example file.AppImage'");
    }
}
