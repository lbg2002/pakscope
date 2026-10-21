//! PakScope 0.2 — compact native Linux application management.
mod uninstall_ui;
use adw::gtk;
use adw::prelude::*;
use gtk::glib;
use pakscope::scanner::{scan_apps, AppEntry, Source};
use pakscope::operations::RemovalMode;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

const STYLES: &str = include_str!("../assets/style.css");

fn label(text: &str, class: &str) -> gtk::Label {
    let w = gtk::Label::new(Some(text));
    w.set_xalign(0.0);
    if !class.is_empty() { w.add_css_class(class); }
    w
}
fn image(name: &str, px: i32) -> gtk::Image {
    let w = gtk::Image::from_icon_name(name);
    w.set_pixel_size(px);
    w
}
fn icon_for(app: &AppEntry, px: i32) -> gtk::Image {
    if app.icon.starts_with('/') && Path::new(&app.icon).is_file() {
        let w = gtk::Image::from_file(&app.icon);
        w.set_pixel_size(px);
        w
    } else {
        image(if app.icon.is_empty() { "application-x-executable-symbolic" } else { &app.icon }, px)
    }
}
fn clear(box_: &gtk::Box) {
    while let Some(child) = box_.first_child() { box_.remove(&child); }
}
fn info_row(title: &str, value: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
    row.add_css_class("information-row");
    row.append(&label(title, "info-label"));
    let content = label(if value.is_empty() { "未确定" } else { value }, "info-value");
    content.set_wrap(true);
    content.set_selectable(true);
    row.append(&content);
    row
}
fn message(parent: &adw::ApplicationWindow, title: &str, text: &str) {
    let dialog = gtk::MessageDialog::builder()
        .transient_for(parent).modal(true).text(title).secondary_text(text).build();
    dialog.add_button("确定", gtk::ResponseType::Close);
    dialog.connect_response(|d, _| d.close());
    dialog.present();
}
fn icon_button(name: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder().icon_name(name).tooltip_text(tooltip).build();
    button.add_css_class("flat");
    button
}
fn source_index(app: &AppEntry) -> u32 {
    match app.source {
        Source::Apt => 1,
        Source::Snap => 2,
        Source::Flatpak => 3,
        Source::AppImage => 4,
        Source::Manual => 5,
    }
}
fn summary(apps: &[AppEntry]) -> String {
    let mut counts = [0usize; 6];
    for item in apps { counts[source_index(item) as usize] += 1; }
    format!("共 {} 款  ·  APT {}  ·  Snap {}  ·  Flatpak {}  ·  AppImage {}  ·  其他 {}",
        apps.len(), counts[1], counts[2], counts[3], counts[4], counts[5])
}
fn show_dpkg_file_list(parent: &adw::ApplicationWindow, app: &AppEntry) {
    if app.source != Source::Apt { return; }
    let result = std::process::Command::new("dpkg-query").args(["-L", &app.id]).output();
    let body = match result {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        Ok(out) => format!("查询失败：{}", String::from_utf8_lossy(&out.stderr)),
        Err(e) => format!("查询失败：{e}"),
    };
    let dialog = gtk::Window::builder().transient_for(parent).modal(true)
        .title(format!("{} · 已安装文件", app.name))
        .default_width(700).default_height(560).build();
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_monospace(true);
    view.set_left_margin(18);
    view.set_top_margin(18);
    view.set_wrap_mode(gtk::WrapMode::None);
    view.buffer().set_text(&body);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&view));
    dialog.set_child(Some(&scroll));
    dialog.present();
}
fn detail_content(parent: &adw::ApplicationWindow, app: &AppEntry, refresh: &gtk::Button) -> gtk::Box {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 13);
    body.add_css_class("detail-body");
    let identity = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let iconwrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    iconwrap.add_css_class("app-icon");
    iconwrap.append(&icon_for(app, 34));
    identity.append(&iconwrap);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 5);
    titles.set_hexpand(true);
    let name = label(&app.name, "detail-name");
    name.set_wrap(true);
    titles.append(&name);
    let source = label(app.source.label(), "source-chip");
    source.add_css_class(&format!("source-{}", app.source.as_key()));
    source.set_halign(gtk::Align::Start);
    titles.append(&source);
    identity.append(&titles);
    body.append(&identity);
    let size = app.size_kib.map(|k| format!("约 {:.1} MiB（来源报告值）", k as f64 / 1024.0))
        .unwrap_or_else(|| "未提供".into());
    for (title, value) in [
        ("软件包 / 应用 ID", app.id.as_str()),
        ("版本", app.version.as_str()),
        ("安装来源和识别依据", app.confidence.as_str()),
        ("执行文件 / 应用路径（如可确定）", app.path.as_str()),
        ("桌面启动器", app.launcher.as_str()),
        ("启动命令（仅展示）", app.command.as_str()),
        ("安装体积", size.as_str()),
    ] { body.append(&info_row(title, value)); }
    let actions = gtk::FlowBox::new();
    actions.set_selection_mode(gtk::SelectionMode::None);
    actions.set_column_spacing(7);
    actions.set_row_spacing(7);
    let copy = gtk::Button::with_label("复制路径");
    copy.add_css_class("pill");
    copy.set_sensitive(!app.path.is_empty());
    let copy_path = app.path.clone();
    copy.connect_clicked(move |_| {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&copy_path);
        }
    });
    actions.append(&copy);
    let open = gtk::Button::with_label("打开目录");
    open.add_css_class("pill");
    let browse_path = app.path.clone();
    open.set_sensitive(Path::new(&browse_path).exists());
    open.connect_clicked(move |_| {
        let p = Path::new(&browse_path);
        let target = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
        if target.exists() {
            let _ = std::process::Command::new("xdg-open").arg(target).spawn();
        }
    });
    actions.append(&open);
    if app.source == Source::Apt {
        let files = gtk::Button::with_label("包文件清单");
        files.add_css_class("pill");
        let app_copy = app.clone();
        let window = parent.clone();
        files.connect_clicked(move |_| show_dpkg_file_list(&window, &app_copy));
        actions.append(&files);
    }
    if !app.launcher.is_empty() {
        let launcher = gtk::Button::with_label("定位启动器");
        launcher.add_css_class("pill");
        let launcher_path = app.launcher.clone();
        launcher.set_sensitive(Path::new(&launcher_path).exists());
        launcher.connect_clicked(move |_| {
            if let Some(dir) = Path::new(&launcher_path).parent() {
                let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
            }
        });
        actions.append(&launcher);
    }
    if app.source != Source::Manual {
        let remove = gtk::Button::with_label("卸载");
        remove.add_css_class("destructive-action");
        let (win, item, refresh) = (parent.clone(), app.clone(), refresh.clone());
        remove.connect_clicked(move |_| uninstall_ui::open(&win, &item, RemovalMode::KeepConfig, &refresh));
        actions.append(&remove);
        if app.source == Source::Apt {
            let purge = gtk::Button::with_label("清除配置并卸载");
            let (win, item, refresh) = (parent.clone(), app.clone(), refresh.clone());
            purge.connect_clicked(move |_| uninstall_ui::open(&win, &item, RemovalMode::Purge, &refresh));
            actions.append(&purge);
        }
    }
    body.append(&actions);
    body
}
fn show_details(parent: &adw::ApplicationWindow, panel: &gtk::Box, pane: &gtk::Paned,
    row_list: &gtk::ListBox, app: &AppEntry, refresh: &gtk::Button) {
    // On small windows, use a transient detail window instead of crushing the inventory.
    if parent.width() > 0 && parent.width() < 880 {
        let dialog = gtk::Window::builder().transient_for(parent).modal(true)
            .title(format!("{} · 应用详情", app.name))
            .default_width(470).default_height(600).build();
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&detail_content(parent, app, refresh)));
        dialog.set_child(Some(&scroll));
        dialog.present();
        return;
    }
    clear(panel);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("detail-header");
    let heading = label("应用详情", "panel-title");
    heading.set_hexpand(true);
    head.append(&heading);
    let close = icon_button("window-close-symbolic", "关闭详情");
    let panel_close = panel.clone();
    let list_close = row_list.clone();
    // Closing returns width to the list; hidden pane is not a fixed blank region.
    let host_close = pane.clone();
    close.connect_clicked(move |_| {
        panel_close.set_visible(false);
        list_close.unselect_all();
        host_close.set_position(1000);
    });
    head.append(&close);
    panel.append(&head);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&detail_content(parent, app, refresh)));
    panel.append(&scroll);
    panel.set_visible(true);
    // Default to a 340 px details pane; users can drag the separator.
    pane.set_position((pane.width() - 340).max(370));
}
fn app_row(app: &AppEntry) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(true);
    row.set_selectable(true);
    row.add_css_class("app-row");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 13);
    line.set_margin_start(13);
    line.set_margin_end(13);
    line.set_margin_top(10);
    line.set_margin_bottom(10);
    let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tile.add_css_class("app-icon");
    tile.append(&icon_for(app, 28));
    line.append(&tile);
    let meta = gtk::Box::new(gtk::Orientation::Vertical, 4);
    meta.set_hexpand(true);
    let name = label(&app.name, "app-name");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    meta.append(&name);
    let second = label(&format!("{}  ·  {}", app.id, app.version), "app-caption");
    second.set_ellipsize(gtk::pango::EllipsizeMode::End);
    meta.append(&second);
    line.append(&meta);
    let source = label(app.source.label(), "source-chip");
    source.add_css_class(&format!("source-{}", app.source.as_key()));
    source.set_valign(gtk::Align::Center);
    line.append(&source);
    line.append(&image("go-next-symbolic", 15));
    row.set_child(Some(&line));
    row
}
fn refill(list: &gtk::ListBox, source: &[AppEntry], items: &Rc<RefCell<Vec<AppEntry>>>,
    query: &str, filter: u32, sort: u32, visible: &gtk::Label, panel: &gtk::Box) {
    let needle = query.trim().to_lowercase();
    let mut filtered: Vec<AppEntry> = source.iter().filter(|a| {
        (filter == 0 || source_index(a) == filter)
            && (needle.is_empty() || [&a.name, &a.id, &a.path, &a.version]
                .iter().any(|s| s.to_lowercase().contains(&needle)))
    }).cloned().collect();
    if sort == 1 { filtered.sort_by_key(|a| (source_index(a), a.name.to_lowercase())); }
    if sort == 2 { filtered.sort_by_key(|a| std::cmp::Reverse(a.name.to_lowercase())); }
    list.unselect_all();
    panel.set_visible(false);
    while let Some(child) = list.first_child() { list.remove(&child); }
    for app in &filtered { list.append(&app_row(app)); }
    visible.set_text(&format!("{} 项", filtered.len()));
    if filtered.is_empty() {
        let empty = gtk::ListBoxRow::new();
        empty.set_activatable(false);
        empty.set_selectable(false);
        let prompt = label("没有匹配的应用", "empty-message");
        prompt.set_margin_top(35);
        prompt.set_margin_bottom(35);
        prompt.set_margin_start(24);
        empty.set_child(Some(&prompt));
        list.append(&empty);
    }
    *items.borrow_mut() = filtered;
}
fn settings_dialog(window: &adw::ApplicationWindow, sidebar: &gtk::Paned) {
    let dialog = gtk::Window::builder().title("PakScope · 设置")
        .transient_for(window).modal(true).default_width(430).default_height(250).build();
    let layout = gtk::Box::new(gtk::Orientation::Vertical, 16);
    layout.set_margin_start(24); layout.set_margin_end(24);
    layout.set_margin_top(22); layout.set_margin_bottom(22);
    layout.append(&label("外观", "panel-title"));
    let theme = gtk::DropDown::from_strings(&["跟随系统", "浅色", "深色"]);
    let manager = adw::StyleManager::default();
    theme.set_selected(match manager.color_scheme() {
        adw::ColorScheme::ForceLight | adw::ColorScheme::PreferLight => 1,
        adw::ColorScheme::ForceDark | adw::ColorScheme::PreferDark => 2,
        _ => 0,
    });
    theme.connect_selected_notify(move |w| {
        adw::StyleManager::default().set_color_scheme(match w.selected() {
            1 => adw::ColorScheme::ForceLight,
            2 => adw::ColorScheme::ForceDark,
            _ => adw::ColorScheme::Default,
        });
    });
    layout.append(&theme);
    layout.append(&label("侧边栏", "panel-title"));
    let reset = gtk::Button::with_label("恢复默认宽度");
    let p = sidebar.clone();
    reset.connect_clicked(move |_| p.set_position(210));
    layout.append(&reset);
    let about = gtk::Button::with_label("关于 PakScope 0.2.0");
    let parent = window.clone();
    about.connect_clicked(move |_| message(&parent, "PakScope · 包览",
        "版本 0.2.0 · 轻量软件管理器。卸载前预览命令，APT 模拟检查影响。"));
    layout.append(&about);
    dialog.set_child(Some(&layout));
    dialog.present();
}
fn export_json(window: &adw::ApplicationWindow, data: &Rc<RefCell<Vec<AppEntry>>>) {
    let dialog = gtk::FileDialog::new();
    dialog.set_title("导出软件清单");
    dialog.set_initial_name(Some("pakscope-apps.json"));
    let apps = data.borrow().clone();
    let win = window.clone();
    dialog.save(Some(window), None::<&gtk::gio::Cancellable>, move |file| {
        if let Ok(file) = file {
            if let Some(path) = file.path() {
                match serde_json::to_string_pretty(&apps)
                    .map_err(|e| e.to_string())
                    .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string())) {
                    Ok(()) => message(&win, "导出完成", &path.display().to_string()),
                    Err(error) => message(&win, "导出失败", &error),
                }
            }
        }
    });
}
fn diagnose(window: &adw::ApplicationWindow, data: &Rc<RefCell<Vec<AppEntry>>>, scan_status: &gtk::Label) {
    let text = format!("{}\n\n{}\n\nAPT/DEB 目前仅展示拥有桌面启动器的软件；来源不明的软件不支持自动卸载。",
        scan_status.text(), summary(&data.borrow()));
    message(window, "扫描诊断", &text);
}
fn start_scan(data: Rc<RefCell<Vec<AppEntry>>>, visible_items: Rc<RefCell<Vec<AppEntry>>>,
    list: gtk::ListBox, search: gtk::SearchEntry, source: gtk::DropDown, sort: gtk::DropDown,
    count: gtk::Label, totals: gtk::Label, status: gtk::Label, refresh: gtk::Button,
    panel: gtk::Box) {
    refresh.set_sensitive(false);
    status.set_text("扫描中…");
    let (sender, receiver): (mpsc::Sender<Vec<AppEntry>>, Receiver<Vec<AppEntry>>) = mpsc::channel();
    std::thread::spawn(move || { let _ = sender.send(scan_apps()); });
    glib::timeout_add_local(Duration::from_millis(120), move || match receiver.try_recv() {
        Ok(entries) => {
            totals.set_text(&summary(&entries));
            *data.borrow_mut() = entries;
            refill(&list, &data.borrow(), &visible_items, &search.text(),
                source.selected(), sort.selected(), &count, &panel);
            status.set_text("扫描完成");
            refresh.set_sensitive(true);
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            status.set_text("扫描失败");
            refresh.set_sensitive(true);
            glib::ControlFlow::Break
        }
    });
}
fn sidebar_action(text: &str, icon: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("sidebar-button");
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    body.append(&image(icon, 18));
    let title = label(text, "sidebar-button-label");
    title.set_hexpand(true);
    body.append(&title);
    button.set_child(Some(&body));
    button
}
fn build_ui(app: &adw::Application) {
    let css = gtk::CssProvider::new();
    css.load_from_string(STYLES);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    let window = adw::ApplicationWindow::builder().application(app)
        .title("PakScope · 包览").default_width(1200).default_height(760).build();
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new("PakScope · 包览", "")));
    let refresh = icon_button("view-refresh-symbolic", "重新扫描");
    header.pack_end(&refresh);
    toolbar.add_top_bar(&header);

    let side_pane = gtk::Paned::new(gtk::Orientation::Horizontal);
    side_pane.set_position(210);
    side_pane.set_shrink_start_child(false);
    side_pane.set_shrink_end_child(false);
    side_pane.set_wide_handle(true);
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 8);
    sidebar.add_css_class("sidebar");
    sidebar.set_size_request(155, -1);
    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    brand.append(&image("package-x-generic-symbolic", 30));
    let identity = gtk::Box::new(gtk::Orientation::Vertical, 4);
    identity.append(&label("PAKSCOPE", "brand-name"));
    brand.append(&identity);
    sidebar.append(&brand);
    let applications = sidebar_action("已安装应用", "view-list-symbolic");
    applications.add_css_class("active");
    sidebar.append(&applications);
    let scan = sidebar_action("重新扫描", "view-refresh-symbolic");
    sidebar.append(&scan);
    let export = sidebar_action("导出清单", "document-save-symbolic");
    sidebar.append(&export);
    let diagnostics = sidebar_action("扫描诊断", "dialog-information-symbolic");
    sidebar.append(&diagnostics);
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    sidebar.append(&spacer);
    let settings = sidebar_action("设置", "emblem-system-symbolic");
    settings.set_tooltip_text(Some("主题、侧边栏和关于"));
    sidebar.append(&settings);
    side_pane.set_start_child(Some(&sidebar));

    let content_pane = gtk::Paned::new(gtk::Orientation::Horizontal);
    content_pane.set_position(700);
    content_pane.set_shrink_start_child(false);
    content_pane.set_shrink_end_child(true);
    content_pane.set_wide_handle(true);
    let main = gtk::Box::new(gtk::Orientation::Vertical, 15);
    main.add_css_class("main-column");
    main.set_size_request(350, -1);
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    heading.append(&label("已安装应用", "list-heading"));
    let count = label("…", "count-label");
    heading.append(&count);
    main.append(&heading);
    let totals = label("", "stats-line");
    totals.set_wrap(true);
    main.append(&totals);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("搜索名称、包名、路径…"));
    search.set_hexpand(true);
    search.add_css_class("search-control");
    controls.append(&search);
    let source = gtk::DropDown::from_strings(&["全部来源", "APT / DEB", "Snap", "Flatpak", "AppImage", "手动 / 未识别"]);
    source.add_css_class("source-filter");
    source.set_tooltip_text(Some("按安装来源筛选"));
    controls.append(&source);
    let sort = gtk::DropDown::from_strings(&["名称 A–Z", "按来源", "名称 Z–A"]);
    sort.add_css_class("sort-filter");
    sort.set_tooltip_text(Some("排序方式"));
    controls.append(&sort);
    main.append(&controls);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("inventory-list");
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&list));
    main.append(&scroll);
    let status = label("", "muted");
    status.set_visible(false);
    main.append(&status);
    content_pane.set_start_child(Some(&main));
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.add_css_class("detail-panel");
    panel.set_size_request(260, -1);
    panel.set_visible(false);
    content_pane.set_end_child(Some(&panel));
    side_pane.set_end_child(Some(&content_pane));
    toolbar.set_content(Some(&side_pane));
    window.set_content(Some(&toolbar));

    let data = Rc::new(RefCell::new(Vec::<AppEntry>::new()));
    let visible_items = Rc::new(RefCell::new(Vec::<AppEntry>::new()));
    // Selection works on a single mouse click. GTK also handles arrow-key selection.
    {
        let (window, panel, pane, rows, items, refresh) = (window.clone(), panel.clone(), content_pane.clone(), list.clone(), visible_items.clone(), refresh.clone());
        list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                if let Some(app) = items.borrow().get(row.index() as usize) {
                    show_details(&window, &panel, &pane, &rows, app, &refresh);
                }
            }
        });
    }
    // Enter/Space activation also opens details, including an already selected row.
    {
        let (window, panel, pane, rows, items, refresh) = (window.clone(), panel.clone(), content_pane.clone(), list.clone(), visible_items.clone(), refresh.clone());
        list.connect_row_activated(move |_, row| {
            if let Some(app) = items.borrow().get(row.index() as usize) {
                show_details(&window, &panel, &pane, &rows, app, &refresh);
            }
        });
    }
    {
        let (data, items, list, source, sort, count, panel) = (data.clone(), visible_items.clone(), list.clone(), source.clone(), sort.clone(), count.clone(), panel.clone());
        search.connect_search_changed(move |entry| refill(&list, &data.borrow(), &items, &entry.text(), source.selected(), sort.selected(), &count, &panel));
    }
    {
        let (data, items, list, search, sort, count, panel) = (data.clone(), visible_items.clone(), list.clone(), search.clone(), sort.clone(), count.clone(), panel.clone());
        source.connect_selected_notify(move |w| refill(&list, &data.borrow(), &items, &search.text(), w.selected(), sort.selected(), &count, &panel));
    }
    {
        let (data, items, list, search, source, count, panel) = (data.clone(), visible_items.clone(), list.clone(), search.clone(), source.clone(), count.clone(), panel.clone());
        sort.connect_selected_notify(move |w| refill(&list, &data.borrow(), &items, &search.text(), source.selected(), w.selected(), &count, &panel));
    }
    {
        let args = (data.clone(), visible_items.clone(), list.clone(), search.clone(), source.clone(), sort.clone(), count.clone(), totals.clone(), status.clone(), panel.clone());
        refresh.connect_clicked(move |b| {
            let (d, v, l, q, s, order, c, stats, state, details) = args.clone();
            start_scan(d, v, l, q, s, order, c, stats, state, b.clone(), details);
        });
    }
    {
        let b = refresh.clone();
        scan.connect_clicked(move |_| b.emit_clicked());
    }
    {
        let (search, source, sort) = (search.clone(), source.clone(), sort.clone());
        applications.connect_clicked(move |_| {
            search.set_text("");
            source.set_selected(0);
            sort.set_selected(0);
            search.grab_focus();
        });
    }
    {
        let (win, data) = (window.clone(), data.clone());
        export.connect_clicked(move |_| export_json(&win, &data));
    }
    {
        let (win, data, status) = (window.clone(), data.clone(), status.clone());
        diagnostics.connect_clicked(move |_| diagnose(&win, &data, &status));
    }
    {
        let (win, pane) = (window.clone(), side_pane.clone());
        settings.connect_clicked(move |_| settings_dialog(&win, &pane));
    }
    window.present();
    start_scan(data, visible_items, list, search, source, sort, count, totals, status, refresh, panel);
}
fn main() {
    let app = adw::Application::builder().application_id("io.github.lbg2002.PakScope").build();
    app.connect_activate(build_ui);
    app.run();
}
