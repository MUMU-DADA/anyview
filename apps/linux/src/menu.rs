//! 右键菜单：按核心的菜单数据生成 GtkPopoverMenu。
//!
//! 每个动作对应窗口上的一个 GAction（`av.<ID>`，下划线换成连字符）。开关和单选用布尔状态显示勾选，
//! 这样同组的每一项可以单独灰显（尚未实现的项）。勾选与可用状态在弹出前从核心刷新。

use std::rc::Rc;

use anyview_core::actions::{self, ALT, ActionKind, CMD, CTRL, Chord, MenuEntry, MenuKind, SHIFT};
use anyview_core::{AppCore, Lang};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::window::Ui;

pub const GROUP: &str = "av";

pub fn action_name(id: &str) -> String {
    id.replace('_', "-")
}

/// 为目录中的全部动作建立 GAction 并装到窗口上；激活时交给核心执行。
pub fn install_actions(ui: &Rc<Ui>) {
    let group = &ui.actions;
    for def in actions::ACTIONS {
        let name = action_name(def.id);
        let action = match def.kind {
            ActionKind::Command => gio::SimpleAction::new(&name, None),
            ActionKind::Toggle | ActionKind::Choice => {
                gio::SimpleAction::new_stateful(&name, None, &false.to_variant())
            }
        };
        let weak = Rc::downgrade(ui);
        let id = def.id;
        action.connect_activate(move |_, _| {
            if let Some(ui) = weak.upgrade() {
                ui.invoke(id);
            }
        });
        group.add_action(&action);
    }
    ui.window.insert_action_group(GROUP, Some(group));
}

/// 从核心刷新每个动作的可用与勾选状态（MENUS"启用规则"）。
pub fn refresh_states(ui: &Ui, group: &gio::SimpleActionGroup) {
    for def in actions::ACTIONS {
        let Some(action) = group
            .lookup_action(&action_name(def.id))
            .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
        else {
            continue;
        };
        let state = ui.core.action_state(def.id);
        action.set_enabled(state.enabled);
        if def.kind != ActionKind::Command {
            action.set_state(&state.checked.to_variant());
        }
    }
}

/// 把扁平的菜单数据转成 GMenu：分隔线之间的项放进同一个分区（section）。
/// 快捷键是按键盘映射实际生效的按键（ACTIONS 规则 7），设置改了以后重新生成。
pub fn build_model(core: &AppCore, lang: Lang) -> gio::Menu {
    struct Level {
        menu: gio::Menu,
        section: gio::Menu,
        title: String,
    }
    impl Level {
        fn new(title: String) -> Self {
            Self {
                menu: gio::Menu::new(),
                section: gio::Menu::new(),
                title,
            }
        }
        fn flush(&mut self) {
            if self.section.n_items() > 0 {
                self.menu.append_section(None, &self.section);
                self.section = gio::Menu::new();
            }
        }
    }

    let mut stack = vec![Level::new(String::new())];
    for entry in core.menu(MenuKind::Context, lang) {
        match entry {
            MenuEntry::Action {
                id,
                title,
                shortcut,
                ..
            } => {
                let item =
                    gio::MenuItem::new(Some(&title), Some(&format!("{GROUP}.{}", action_name(id))));
                if let Some(accel) = shortcut.as_ref().and_then(accel_for) {
                    item.set_attribute_value("accel", Some(&accel.to_variant()));
                }
                stack
                    .last_mut()
                    .expect("至少有顶层")
                    .section
                    .append_item(&item);
            }
            MenuEntry::Separator => stack.last_mut().expect("至少有顶层").flush(),
            MenuEntry::SubmenuStart { title, .. } => stack.push(Level::new(title)),
            MenuEntry::SubmenuEnd => {
                let mut level = stack.pop().expect("子菜单成对出现");
                level.flush();
                let parent = stack.last_mut().expect("子菜单有上一级");
                parent
                    .section
                    .append_submenu(Some(&level.title), &level.menu);
            }
        }
    }
    let mut top = stack.pop().expect("至少有顶层");
    top.flush();
    top.menu
}

/// 快捷键转成 GTK 的加速键写法（只用于在菜单上显示，按键统一由核心解析）。
pub fn accel_for(chord: &Chord) -> Option<String> {
    let key = match chord.key.as_str() {
        "PageUp" => "Page_Up".to_string(),
        "PageDown" => "Page_Down".to_string(),
        "Enter" => "Return".to_string(),
        "Backspace" => "BackSpace".to_string(),
        "Space" => "space".to_string(),
        named if named.chars().count() > 1 => named.to_string(),
        text => {
            let c = text.chars().next()?;
            let name = match c {
                '+' => "plus",
                '-' => "minus",
                '*' => "asterisk",
                '=' => "equal",
                '[' => "bracketleft",
                ']' => "bracketright",
                '`' => "grave",
                '~' => "asciitilde",
                ',' => "comma",
                '.' => "period",
                '/' => "slash",
                ';' => "semicolon",
                c if c.is_ascii_alphanumeric() => {
                    return Some(prefix(chord) + &c.to_ascii_lowercase().to_string());
                }
                _ => return None,
            };
            name.to_string()
        }
    };
    Some(prefix(chord) + &key)
}

fn prefix(chord: &Chord) -> String {
    let mut text = String::new();
    for (flag, name) in [
        (CTRL, "<Control>"),
        (ALT, "<Alt>"),
        (SHIFT, "<Shift>"),
        (CMD, "<Super>"),
    ] {
        if chord.mods & flag != 0 {
            text.push_str(name);
        }
    }
    text
}

/// 在画布的 (x, y)（逻辑像素）处弹出右键菜单。
pub fn popup(ui: &Rc<Ui>, x: f64, y: f64) {
    refresh_states(ui, &ui.actions);
    let popover =
        gtk::PopoverMenu::from_model_full(&*ui.menu_model.borrow(), gtk::PopoverMenuFlags::NESTED);
    popover.set_parent(&ui.canvas);
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.connect_closed(|popover| {
        let popover = popover.clone();
        // 菜单项的动作在关闭之后才激活，延后解除父控件。
        glib::idle_add_local_once(move || popover.unparent());
    });
    popover.popup();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accels_use_gdk_key_names() {
        let accel = |spec| accel_for(&Chord::parse(spec).unwrap());
        assert_eq!(
            accel("Ctrl+PageDown").as_deref(),
            Some("<Control>Page_Down")
        );
        assert_eq!(accel("+").as_deref(), Some("plus"));
        assert_eq!(accel("Ctrl++").as_deref(), Some("<Control>plus"));
        assert_eq!(accel("Alt+Left").as_deref(), Some("<Alt>Left"));
        assert_eq!(accel("Z").as_deref(), Some("z"));
        assert_eq!(accel("*").as_deref(), Some("asterisk"));
        assert_eq!(accel("~").as_deref(), Some("asciitilde"));
        assert_eq!(accel("[").as_deref(), Some("bracketleft"));
        assert_eq!(accel("F2").as_deref(), Some("F2"));
    }
}
