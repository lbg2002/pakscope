use pakscope::scanner::scan_apps;
fn main() {
    let apps = scan_apps();
    if std::env::args().any(|arg| arg == "--json") {
        match serde_json::to_string_pretty(&apps) {
            Ok(json) => println!("{json}"),
            Err(e) => { eprintln!("JSON 序列化失败: {e}"); std::process::exit(1); }
        }
    } else {
        println!("名称\t来源\t版本\t路径");
        for app in apps { println!("{}\t{}\t{}\t{}", app.name.replace('\t', " "), app.source.label(), app.version, app.path); }
    }
}
