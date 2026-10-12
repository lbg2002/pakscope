//! PakScope GTK4/libadwaita desktop interface. No package mutations in v0.1.
use adw::gtk;
use adw::prelude::*;
use gtk::glib;
use pakscope::scanner::{scan_apps, AppEntry, Source};
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
    let widget = gtk::Image::from_icon_name(name);
    widget.set_pixel_size(px);
    widget
}

fn icon_for(item: &AppEntry) -> gtk::Image {
    if item.icon.starts_with('/') && Path::new(&item.icon).is_file() {
        let widget = gtk::Image::from_file(&item.icon);
        widget.set_pixel_size(32);
        widget
    } else {
        image(if item.icon.is_empty() { "application-x-executable-symbolic" } else { &item.icon }, 32)
    }
}

fn make_card(caption: &str, value: &gtk::Label, symbolic_icon: &str, class: &str) -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 8);
    outer.add_css_class("stat-card");
    outer.add_css_class(class);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = label(caption, "stat-caption");
    title.set_hexpand(true);
    top.append(&title);
    top.append(&image(symbolic_icon, 20));
    value.add_css_class("stat-number");
    outer.append(&top);
    outer.append(value);
    outer
}

fn info_row(title: &str, value: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 5);
    row.add_css_class("information-row");
    row.append(&label(title, "info-label"));
    let text = label(if value.is_empty() { "未提供" } else { value }, "info-value");
    text.set_wrap(true);
    text.set_selectable(true);
    row.append(&text);
    row
}

fn details_dialog(parent: &adw::ApplicationWindow, item: AppEntry) {
    let dialog = gtk::Window::builder()
        .title(format!("{} · 应用详情", item.name))
        .transient_for(parent)
        .modal(true)
        .default_width(540)
        .default_height(525)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let header = gtk::HeaderBar::new();
    let title = label(&item.name, "detail-title");
    header.set_title_widget(Some(&title));
    content.append(&header);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inner.set_margin_start(24);
    inner.set_margin_end(24);
    inner.set_margin_top(12);
    inner.set_margin_bottom(24);
    inner.add_css_class("detail-sheet");
    let provenance = format!("{} · {}", item.source.label(), item.confidence);
    for (title, value) in [
        ("安装来源与识别依据", provenance.as_str()),
        ("软件包 / 应用 ID", item.id.as_str()),
        ("版本", item.version.as_str()),
        ("安装位置（APT 为共享目录，不能当作卸载路径）", item.path.as_str()),
        ("桌面启动文件", item.launcher.as_str()),
        ("启动命令（来自 desktop 文件，未执行）", item.command.as_str()),
    ] { inner.append(&info_row(title, value)); }
    let size = item.size_kib.map(|k| format!("约 {:.1} MiB（仅当前来源提供的指标）", k as f64 / 1024.0))
        .unwrap_or_else(|| "未计算，防止误把共享运行时算作单个应用".to_string());
    inner.append(&info_row("报告的体积", &size));
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_margin_top(20);
    let copy = gtk::Button::with_label("复制路径");
    copy.add_css_class("pill");
    let path = item.path.clone();
    copy.connect_clicked(move |_| {
        if let Some(display) = gtk::gdk::Display::default() { display.clipboard().set_text(&path); }
    });
    let open = gtk::Button::with_label("打开所在目录");
    open.add_css_class("suggested-action");
    open.add_css_class("pill");
    let open_path = item.path.clone();
    // Never execute the app's Exec= field or any shell. xdg-open gets one validated path argument.
    open.set_sensitive(Path::new(&open_path).exists() && !open_path.contains('('));
    open.connect_clicked(move |_| {
        let p = Path::new(&open_path);
        let target = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
        let _ = std::process::Command::new("xdg-open").arg(target).spawn();
    });
    if !item.launcher.is_empty() && Path::new(&item.launcher).exists() {
        let browse = gtk::Button::with_label("启动器位置");
        browse.add_css_class("pill");
        let launcher_file = item.launcher.clone();
        browse.connect_clicked(move |_| {
            if let Some(parent) = Path::new(&launcher_file).parent() {
                let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
            }
        });
        actions.append(&browse);
    }
    actions.append(&copy);
    actions.append(&open);
    inner.append(&actions);
    if item.source == Source::Apt {
        let files_button = gtk::Button::with_label("查看该 DEB 包安装的文件清单");
        files_button.set_halign(gtk::Align::Start);
        files_button.set_margin_top(12);
        let package_id = item.id.clone();
        let title = item.name.clone();
        let parent_dialog = dialog.clone();
        files_button.connect_clicked(move |_| {
            let output = std::process::Command::new("dpkg-query")
                .args(["-L", &package_id]).output();
            let body = match output {
                Ok(result) if result.status.success() => String::from_utf8_lossy(&result.stdout).to_string(),
                Ok(result) => format!("查询失败：{}", String::from_utf8_lossy(&result.stderr)),
                Err(err) => format!("无法调用 dpkg-query：{err}"),
            };
            let win = gtk::Window::builder().transient_for(&parent_dialog)
                .modal(true).title(format!("{title} · 文件清单"))
                .default_width(650).default_height(530).build();
            let scroller = gtk::ScrolledWindow::new();
            let text = gtk::TextView::new();
            text.set_editable(false);
            text.set_monospace(true);
            text.set_top_margin(14);
            text.set_left_margin(14);
            text.set_wrap_mode(gtk::WrapMode::None);
            text.buffer().set_text(&body);
            scroller.set_child(Some(&text));
            win.set_child(Some(&scroller));
            win.present();
        });
        inner.append(&files_button);
    }
    inner.append(&label("安全提示：v0.1 仅展示信息，不支持删除或卸载。", "footnote"));
    scroll.set_child(Some(&inner));
    content.append(&scroll);
    dialog.set_child(Some(&content));
    dialog.present();
}

fn app_row(parent: &adw::ApplicationWindow, item: &AppEntry) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(true);
    row.set_selectable(false);
    row.add_css_class("app-row");
    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 15);
    layout.set_margin_start(18);
    layout.set_margin_end(18);
    layout.set_margin_top(12);
    layout.set_margin_bottom(12);
    let iconwrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    iconwrap.add_css_class("app-icon");
    iconwrap.append(&icon_for(item));
    layout.append(&iconwrap);
    let meta = gtk::Box::new(gtk::Orientation::Vertical, 5);
    meta.set_hexpand(true);
    let name = label(&item.name, "app-name");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    meta.append(&name);
    let subtitle = format!("{}  ·  {}", item.id, item.version);
    let second = label(&subtitle, "app-caption");
    second.set_ellipsize(gtk::pango::EllipsizeMode::End);
    meta.append(&second);
    layout.append(&meta);
    let chip = label(item.source.label(), "source-chip");
    chip.add_css_class(&format!("source-{}", item.source.as_key()));
    chip.set_valign(gtk::Align::Center);
    layout.append(&chip);
    layout.append(&image("go-next-symbolic", 18));
    row.set_child(Some(&layout));
    let item = item.clone();
    let window = parent.clone();
    row.connect_activate(move |_| details_dialog(&window, item.clone()));
    row
}

fn refill(parent: &adw::ApplicationWindow, list: &gtk::ListBox, entries: &[AppEntry], text: &str, filter: u32, count: &gtk::Label) {
    while let Some(child) = list.first_child() { list.remove(&child); }
    let query = text.trim().to_lowercase();
    let mut visible = 0;
    for app in entries {
        let matches_source = match filter {
            1 => app.source == Source::Apt,
            2 => app.source == Source::Snap,
            3 => app.source == Source::Flatpak,
            4 => app.source == Source::AppImage,
            5 => app.source == Source::Manual,
            _ => true,
        };
        if !matches_source { continue; }
        if !query.is_empty() && ![&app.name, &app.id, &app.path, &app.version]
            .iter().any(|s| s.to_lowercase().contains(&query)) { continue; }
        list.append(&app_row(parent, app));
        visible += 1;
    }
    count.set_text(&format!("找到 {visible} 个应用"));
    if visible == 0 {
        let empty = gtk::ListBoxRow::new();
        empty.set_activatable(false);
        let text = label("暂无匹配的应用。可以换个关键词或来源筛选。", "empty-message");
        text.set_margin_top(38);
        text.set_margin_bottom(38);
        text.set_margin_start(24);
        text.set_margin_end(24);
        empty.set_child(Some(&text));
        list.append(&empty);
    }
}

fn start_scan(
    data: Rc<RefCell<Vec<AppEntry>>>,
    window: adw::ApplicationWindow,
    list: gtk::ListBox,
    query: gtk::SearchEntry,
    origin: gtk::DropDown,
    visible: gtk::Label,
    total: gtk::Label,
    apt: gtk::Label,
    other: gtk::Label,
    status: gtk::Label,
    refresh: gtk::Button,
) {
    refresh.set_sensitive(false);
    status.set_text("正在后台扫描本机软件来源…");
    let (sender, receiver): (mpsc::Sender<Vec<AppEntry>>, Receiver<Vec<AppEntry>>) = mpsc::channel();
    std::thread::spawn(move || { let _ = sender.send(scan_apps()); });
    glib::timeout_add_local(Duration::from_millis(110), move || match receiver.try_recv() {
        Ok(new_items) => {
            total.set_text(&new_items.len().to_string());
            apt.set_text(&new_items.iter().filter(|app| app.source == Source::Apt).count().to_string());
            other.set_text(&new_items.iter().filter(|app| app.source != Source::Apt).count().to_string());
            *data.borrow_mut() = new_items;
            refill(&window, &list, &data.borrow(), &query.text(), origin.selected(), &visible);
            status.set_text("已完成 · 本地只读扫描");
            refresh.set_sensitive(true);
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            status.set_text("扫描异常，请重新尝试");
            refresh.set_sensitive(true);
            glib::ControlFlow::Break
        }
    });
}

fn sidebar_button(text: &str, symbol: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("sidebar-button");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 11);
    line.append(&image(symbol, 18));
    line.append(&label(text, "sidebar-button-label"));
    button.set_child(Some(&line));
    button
}

fn build_ui(app: &adw::Application) {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(STYLES);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("PakScope · 包览")
        .default_width(1110)
        .default_height(740)
        .build();

    let toolbar = adw::ToolbarView::new();
    let bar = adw::HeaderBar::new();
    let title = adw::WindowTitle::new("PakScope · 包览", "让已安装的软件一目了然");
    bar.set_title_widget(Some(&title));
    let refresh = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("重新扫描")
        .build();
    bar.pack_end(&refresh);
    toolbar.add_top_bar(&bar);

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.add_css_class("app-root");
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 12);
    sidebar.set_size_request(221, -1);
    sidebar.add_css_class("sidebar");
    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    brand.set_margin_bottom(15);
    brand.append(&image("package-x-generic-symbolic", 29));
    let brandtext = gtk::Box::new(gtk::Orientation::Vertical, 2);
    brandtext.append(&label("PAKSCOPE", "brand-name"));
    brandtext.append(&label("包览 · 软件资产中心", "muted"));
    brand.append(&brandtext);
    sidebar.append(&brand);
    sidebar.append(&label("应用来源", "section-caption"));
    let sources = [
        ("全部应用", "view-grid-symbolic"),
        ("APT / DEB", "package-x-generic-symbolic"),
        ("Snap", "applications-system-symbolic"),
        ("Flatpak", "folder-download-symbolic"),
        ("AppImage", "application-x-executable-symbolic"),
        ("手动 / 未识别", "folder-symbolic"),
    ];

    let maincol = gtk::Box::new(gtk::Orientation::Vertical, 18);
    maincol.add_css_class("main-column");
    maincol.set_hexpand(true);
    maincol.set_vexpand(true);

    let hero = gtk::Box::new(gtk::Orientation::Horizontal, 15);
    hero.add_css_class("hero");
    let herotext = gtk::Box::new(gtk::Orientation::Vertical, 8);
    herotext.set_hexpand(true);
    herotext.append(&label("认识你的每一款软件", "hero-title"));
    herotext.append(&label("安装来源、版本信息、程序位置，一次看清。", "hero-subtitle"));
    let status = label("准备开始扫描…", "hero-status");
    herotext.append(&status);
    hero.append(&herotext);
    hero.append(&image("folder-saved-search-symbolic", 72));
    maincol.append(&hero);

    let total = label("—", "");
    let apt = label("—", "");
    let other = label("—", "");
    let stats = gtk::Box::new(gtk::Orientation::Horizontal, 13);
    for child in [
        make_card("发现应用", &total, "view-grid-symbolic", "stat-blue"),
        make_card("APT / DEB", &apt, "package-x-generic-symbolic", "stat-violet"),
        make_card("其他来源", &other, "drive-harddisk-symbolic", "stat-green"),
    ] { child.set_hexpand(true); stats.append(&child); }
    maincol.append(&stats);

    let titleline = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let section = label("已安装应用", "list-heading");
    section.set_hexpand(true);
    titleline.append(&section);
    let visible = label("扫描中…", "muted");
    titleline.append(&visible);
    maincol.append(&titleline);

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("搜索软件名称、包名、安装位置…"));
    search.set_hexpand(true);
    search.add_css_class("search-control");
    let origin = gtk::DropDown::from_strings(&[
        "全部来源", "APT / DEB", "Snap", "Flatpak", "AppImage", "手动 / 未识别"
    ]);
    origin.add_css_class("source-filter");
    controls.append(&search);
    controls.append(&origin);
    maincol.append(&controls);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("inventory-list");
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_min_content_height(220);
    scroll.set_child(Some(&list));
    maincol.append(&scroll);
    let footer = label("🔒  安全只读模式 · 不修改系统、不删除软件、不需要管理员权限", "footnote");
    maincol.append(&footer);
    let sidebar_buttons: Rc<RefCell<Vec<gtk::Button>>> = Rc::new(RefCell::new(Vec::new()));
    for (idx, (name, sym)) in sources.iter().enumerate() {
        let button = sidebar_button(name, sym);
        if idx == 0 { button.add_css_class("active"); }
        let dropdown = origin.clone();
        button.connect_clicked(move |_| dropdown.set_selected(idx as u32));
        sidebar.append(&button);
        sidebar_buttons.borrow_mut().push(button);
    }
    sidebar.append(&gtk::Box::new(gtk::Orientation::Vertical, 1));
    sidebar.append(&label("当前版本", "section-caption"));
    sidebar.append(&label("v0.1.0 · 只读预览版", "muted"));
    let notice = gtk::Box::new(gtk::Orientation::Vertical, 5);
    notice.add_css_class("sidebar-notice");
    notice.append(&label("下一步：安全卸载", "notice-title"));
    let notice_text = label("卸载影响预览与残留管理将在后续版本加入。", "muted");
    notice_text.set_wrap(true);
    notice.append(&notice_text);
    sidebar.append(&notice);

    root.append(&sidebar);
    root.append(&maincol);
    toolbar.set_content(Some(&root));
    window.set_content(Some(&toolbar));

    let data = Rc::new(RefCell::new(Vec::<AppEntry>::new()));
    {
        let (data, window, list, origin, visible) = (data.clone(), window.clone(), list.clone(), origin.clone(), visible.clone());
        search.connect_search_changed(move |entry| {
            refill(&window, &list, &data.borrow(), &entry.text(), origin.selected(), &visible);
        });
    }
    {
        let (data, window, list, search, visible, buttons) = (
            data.clone(), window.clone(), list.clone(), search.clone(), visible.clone(), sidebar_buttons.clone()
        );
        origin.connect_selected_notify(move |source| {
            for (idx, button) in buttons.borrow().iter().enumerate() {
                if idx as u32 == source.selected() { button.add_css_class("active"); }
                else { button.remove_css_class("active"); }
            }
            refill(&window, &list, &data.borrow(), &search.text(), source.selected(), &visible);
        });
    }
    {
        let (data, window, list, search, origin, visible, total, apt, other, status) = (
            data.clone(), window.clone(), list.clone(), search.clone(), origin.clone(),
            visible.clone(), total.clone(), apt.clone(), other.clone(), status.clone());
        refresh.connect_clicked(move |button| start_scan(
            data.clone(), window.clone(), list.clone(), search.clone(), origin.clone(),
            visible.clone(), total.clone(), apt.clone(), other.clone(), status.clone(), button.clone()
        ));
    }
    window.present();
    start_scan(data, window, list, search, origin, visible, total, apt, other, status, refresh);
}

fn main() {
    let app = adw::Application::builder().application_id("io.github.lbg2002.PakScope").build();
    app.connect_activate(build_ui);
    app.run();
}
