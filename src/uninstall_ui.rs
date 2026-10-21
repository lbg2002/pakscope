//! Confirmation UI for opt-in software removal. No shell and no GUI work off-thread.
use adw::gtk;
use adw::prelude::*;
use gtk::glib;
use pakscope::operations::{apt_preview, execute, removal_plan, RemovalMode};
use pakscope::scanner::AppEntry;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

pub fn open(parent: &adw::ApplicationWindow, app: &AppEntry,
    mode: RemovalMode, refresh: &gtk::Button) {
    let plan = match removal_plan(app, mode) {
        Ok(plan) => plan,
        Err(err) => {
            let dialog = gtk::MessageDialog::builder().transient_for(parent).modal(true)
                .text("无法安全卸载").secondary_text(&err).build();
            dialog.add_button("关闭", gtk::ResponseType::Close);
            dialog.connect_response(|dialog, _| dialog.close());
            dialog.present();
            return;
        }
    };
    let dialog = gtk::Window::builder().title(format!("卸载 · {}", app.name))
        .transient_for(parent).modal(true).default_width(540).default_height(420).build();
    let layout = gtk::Box::new(gtk::Orientation::Vertical, 12);
    layout.set_margin_start(20);
    layout.set_margin_end(20);
    layout.set_margin_top(18);
    layout.set_margin_bottom(18);
    let heading = gtk::Label::new(Some(&format!("{}  ·  {}", app.name, app.source.label())));
    heading.set_xalign(0.0);
    heading.set_wrap(true);
    heading.add_css_class("title-3");
    layout.append(&heading);
    let command = gtk::Label::new(Some(&plan.command));
    command.set_xalign(0.0);
    command.set_selectable(true);
    command.set_wrap(true);
    command.add_css_class("command-preview");
    layout.append(&command);
    let note = gtk::Label::new(Some(plan.note));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("dim-label");
    layout.append(&note);
    let impact = gtk::TextView::new();
    impact.set_editable(false);
    impact.set_cursor_visible(false);
    impact.set_monospace(true);
    impact.set_wrap_mode(gtk::WrapMode::WordChar);
    impact.buffer().set_text(if plan.requires_apt_preview {
        "正在模拟 APT 卸载…"
    } else {
        "此来源不支持完整的卸载影响预演，请确认软件名称及命令。"
    });
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_min_content_height(110);
    scroll.set_child(Some(&impact));
    layout.append(&scroll);
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_selectable(true);
    layout.append(&status);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    controls.set_halign(gtk::Align::End);
    let copy = gtk::Button::with_label("复制指令");
    let text = plan.command.clone();
    copy.connect_clicked(move |_| {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&text);
        }
    });
    controls.append(&copy);
    let close = gtk::Button::with_label("取消");
    let close_window = dialog.clone();
    close.connect_clicked(move |_| close_window.close());
    controls.append(&close);
    let execute_button = gtk::Button::with_label("确认卸载");
    execute_button.add_css_class("destructive-action");
    execute_button.set_sensitive(!plan.requires_apt_preview);
    controls.append(&execute_button);
    layout.append(&controls);
    dialog.set_child(Some(&layout));

    let checked = Rc::new(RefCell::new(None::<String>));
    if plan.requires_apt_preview {
        let (sender, receiver) = mpsc::channel();
        let item = app.clone();
        std::thread::spawn(move || { let _ = sender.send(apt_preview(&item, mode)); });
        let (checked, button, impact) = (checked.clone(), execute_button.clone(), impact.clone());
        glib::timeout_add_local(Duration::from_millis(100), move || match receiver.try_recv() {
            Ok(Ok(summary)) => {
                impact.buffer().set_text(&summary);
                *checked.borrow_mut() = Some(summary);
                button.set_sensitive(true);
                glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                impact.buffer().set_text(&format!("模拟失败：{err}\n不能自动执行，可复制指令自行检查。"));
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                impact.buffer().set_text("模拟检查中断，已禁用执行。");
                glib::ControlFlow::Break
            }
        });
    }

    let (item, refresh) = (app.clone(), refresh.clone());
    execute_button.connect_clicked(move |button| {
        button.set_sensitive(false);
        close.set_sensitive(false);
        status.set_text("正在执行…");
        let expected = checked.borrow().clone();
        let planned = plan.clone();
        let item = item.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            // Recheck just before changing the system; refuse if the APT plan drifted.
            let result = if planned.requires_apt_preview {
                match apt_preview(&item, mode) {
                    Ok(actual) if Some(actual) == expected => execute(&planned),
                    Ok(_) => Err("软件包依赖情况已改变，请重新打开并确认卸载。".into()),
                    Err(err) => Err(format!("二次模拟失败：{err}")),
                }
            } else { execute(&planned) };
            let _ = sender.send(result);
        });
        let (button, close, impact, status, refresh) =
            (button.clone(), close.clone(), impact.clone(), status.clone(), refresh.clone());
        glib::timeout_add_local(Duration::from_millis(120), move || match receiver.try_recv() {
            Ok(Ok(output)) => {
                status.set_text("卸载完成");
                impact.buffer().set_text(&output);
                close.set_label("关闭");
                close.set_sensitive(true);
                button.set_visible(false);
                refresh.emit_clicked();
                glib::ControlFlow::Break
            }
            Ok(Err(error)) => {
                status.set_text("卸载失败");
                impact.buffer().set_text(&error);
                close.set_label("关闭");
                close.set_sensitive(true);
                button.set_visible(false);
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                status.set_text("执行结果未知，请核查软件是否仍存在。");
                close.set_sensitive(true);
                button.set_visible(false);
                glib::ControlFlow::Break
            }
        });
    });
    dialog.present();
}
