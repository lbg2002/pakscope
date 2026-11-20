use pakscope::scanner::{scan_apps_with_options, ScanOptions};
use pakscope::settings::Settings;
use pakscope::updater::{check_for_updates, UpdateCheckResult, CURRENT_VERSION};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("pakscope-scan {CURRENT_VERSION}");
        return;
    }
    if args.iter().any(|a| a == "--check-update") {
        match check_for_updates() {
            Ok(UpdateCheckResult::UpdateAvailable(rel)) => {
                println!("发现新版本: {} (当前 v{}) -> {}", rel.tag_name, CURRENT_VERSION, rel.html_url);
            }
            Ok(UpdateCheckResult::UpToDate { current_version, latest }) => {
                println!("当前已是最新版本: v{} (GitHub 最新 {})", current_version, latest.tag_name);
            }
            Ok(UpdateCheckResult::NoReleasesYet { current_version, releases_url }) => {
                println!("当前运行 v{}，仓库尚未发布正式 Release: {}", current_version, releases_url);
            }
            Err(err) => {
                eprintln!("检查更新失败: {err}");
                std::process::exit(1);
            }
        }
        return;
    }

    let mut opts = ScanOptions::from_settings(&Settings::load());
    if args.iter().any(|a| a == "--all" || a == "--include-cli") {
        opts.include_cli = true;
    }
    let apps = scan_apps_with_options(&opts);
    if args.iter().any(|arg| arg == "--json") {
        match serde_json::to_string_pretty(&apps) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("JSON 序列化失败: {e}");
                std::process::exit(1);
            }
        }
    } else if args.iter().any(|arg| arg == "--csv") {
        println!("名称,包ID,来源,范围,版本,体积KiB,路径");
        for app in apps {
            let esc = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
            println!(
                "{},{},{},{},{},{},{}",
                esc(&app.name),
                esc(&app.id),
                esc(app.source.label()),
                esc(app.scope.label()),
                esc(&app.version),
                app.size_kib.map(|k| k.to_string()).unwrap_or_default(),
                esc(&app.path)
            );
        }
    } else {
        println!("名称\t包ID\t来源\t版本\t路径");
        for app in apps {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                app.name.replace('\t', " "),
                app.id.replace('\t', " "),
                app.source.label(),
                app.version,
                app.path
            );
        }
    }
}
