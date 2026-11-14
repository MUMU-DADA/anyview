//! Anyview 的 Linux 界面：GTK4，不使用 libadwaita。
//!
//! 用法：`anyview [图片或文件夹]`
//! 缩略图生成器：`anyview --thumbnail <图片> <输出的 PNG> <边长>`（文件管理器调用）
//! 自测：`anyview --self-test 目录 [--open 路径] [--steps 动作,动作,…] [--hold-ms 毫秒] [--settings 文件]`
//!       （apps/linux/scripts/selftest.sh 使用，写出 selftest.json 和截图后退出）

mod canvas;
mod default_viewer;
mod flatpak_thumbnailer;
mod generation_window;
mod help_window;
mod instance;
mod menu;
mod monitor_icc;
mod overlays;
mod picker;
mod sandbox;
mod selftest;
mod settings_window;
mod sound;
mod window;
mod window_state;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

/// main 开始的时刻（冷启动测量）。
pub static MAIN_START: OnceLock<Instant> = OnceLock::new();

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// 应用 ID。
const APP_ID: &str = "local.anyview.Anyview";

const CSS: &str = "
window.anyview-main { background-color: #202020; }
label.anyview-hint { color: #9e9e9e; font-size: 15px; }
label.anyview-status { color: #6e6e6e; font-size: 12px; }
/* 开源许可：许可证原文按 80 列排版，用等宽字体（主题的 .monospace 在有的环境里不起作用）。 */
textview.anyview-mono, textview.anyview-mono text { font-family: monospace; font-size: 10pt; }
label.anyview-notice {
  color: #f2f2f2;
  background-color: rgba(0, 0, 0, 0.62);
  border-radius: 6px;
  padding: 6px 12px;
  font-size: 13px;
}
box.anyview-anim {
  background-color: rgba(28, 28, 28, 0.72);
  border-radius: 999px;
  padding: 2px 12px 2px 4px;
}
box.anyview-anim button { background: none; border: none; box-shadow: none; color: #e8e8e8; min-width: 28px; min-height: 28px; }
box.anyview-anim button:hover { background-color: rgba(255, 255, 255, 0.14); border-radius: 999px; }
label.anyview-anim-text { color: #e8e8e8; font-size: 12px; }
box.anyview-bar {
  background-color: rgba(28, 28, 28, 0.82);
  border: 1px solid rgba(255, 255, 255, 0.10);
  border-radius: 36px;
  padding: 6px 10px;
}
box.anyview-bar button, box.anyview-info button, button.anyview-arrow {
  background: none;
  border: none;
  box-shadow: none;
  color: #e8e8e8;
}
box.anyview-bar button:hover, box.anyview-info button:hover { background-color: rgba(255, 255, 255, 0.14); }
box.anyview-bar button:disabled { color: rgba(232, 232, 232, 0.35); }
box.anyview-bar button.anyview-bar-big {
  min-width: 40px;
  min-height: 40px;
  border-radius: 999px;
  border: 1px solid rgba(255, 255, 255, 0.18);
}
box.anyview-bar button.anyview-bar-small { min-width: 24px; min-height: 22px; padding: 0 4px; border-radius: 6px; }
box.anyview-bar button.anyview-bar-lock { padding: 0 6px; border-radius: 6px; font-size: 12px; }
button.anyview-bar-lock.locked label.anyview-lock-dot { color: #7cc4ff; }
separator.anyview-bar-separator { margin: 4px 4px; background-color: rgba(255, 255, 255, 0.2); min-width: 1px; }
button.anyview-arrow { color: rgba(200, 200, 200, 0.9); padding: 8px 2px; }
button.anyview-arrow:hover { color: #ffffff; }
box.anyview-info button.anyview-info-button {
  min-width: 28px;
  min-height: 28px;
  border-radius: 999px;
  border: 2px solid rgba(255, 255, 255, 0.9);
}
label.anyview-info-text {
  color: #ffffff;
  font-size: 15px;
  text-shadow: 0 0 2px #000000, 0 0 2px #000000, 1px 1px 1px #000000;
}
label.anyview-info-text.anyview-info-small { font-size: 12px; }
box.anyview-invalid entry { outline: 2px solid #d64545; outline-offset: -2px; }
box.anyview-invalid-row { background-color: alpha(#d64545, 0.16); border-radius: 8px; }
button.anyview-swatch { padding: 1px; min-width: 0; min-height: 0; border-radius: 999px; }
/* 设置窗口：带图标的分类列表，右边按组排成圆角卡片。颜色都从主题的文字颜色调出来，浅色、深色主题都能用。 */
box.anyview-settings-sidebar { background-color: alpha(currentColor, 0.04); border-right: 1px solid alpha(currentColor, 0.10); padding: 14px 10px; }
box.anyview-settings-sidebar list { background: none; }
box.anyview-settings-sidebar row { background: none; border-radius: 7px; padding: 7px 10px; margin-bottom: 2px; }
box.anyview-settings-sidebar row:hover { background-color: alpha(currentColor, 0.06); }
box.anyview-settings-sidebar row:selected { background-color: alpha(currentColor, 0.13); color: inherit; }
box.anyview-settings-page { margin: 22px 32px 18px 32px; }
label.anyview-settings-title { font-size: 1.55em; font-weight: 600; margin: 2px 0 14px 0; }
label.anyview-settings-lead { opacity: 0.65; margin: -6px 0 8px 0; }
label.anyview-settings-caption { font-size: 0.88em; font-weight: 600; opacity: 0.6; margin: 18px 0 7px 4px; }
box.anyview-settings-card { background-color: alpha(currentColor, 0.045); border: 1px solid alpha(currentColor, 0.08); border-radius: 10px; }
box.anyview-settings-card > separator { background-color: alpha(currentColor, 0.08); min-height: 1px; }
box.anyview-settings-row { padding: 8px 14px; min-height: 28px; }
label.anyview-settings-detail { font-size: 0.88em; opacity: 0.6; }
label.anyview-settings-shortcut { opacity: 0.55; }
label.anyview-settings-note { opacity: 0.7; padding: 4px 0; }
box.anyview-settings-footer { border-top: 1px solid alpha(currentColor, 0.10); padding: 12px 20px 12px 24px; }
button.anyview-key { min-width: 104px; font-weight: 600; }
label.anyview-zoom {
  color: #f2f2f2;
  background-color: rgba(0, 0, 0, 0.55);
  border-radius: 8px;
  padding: 8px 16px;
  font-size: 20px;
}
";

#[derive(Clone, Default)]
pub struct LaunchOptions {
    pub open: Option<PathBuf>,
    /// 设置文件（`--settings`）；自测没给时用报告目录里的 settings.toml，不碰用户自己的设置。
    pub settings: Option<PathBuf>,
    pub self_test: Option<selftest::Plan>,
}

fn parse_args() -> LaunchOptions {
    let mut open = None;
    let mut settings = None;
    let mut dir: Option<PathBuf> = None;
    let mut steps = Vec::new();
    let mut hold_ms = 0;
    let mut sample_at_100 = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--self-test" => dir = args.next().map(PathBuf::from),
            "--open" => open = args.next().map(PathBuf::from),
            "--settings" => settings = args.next().map(PathBuf::from),
            "--steps" => {
                steps = args
                    .next()
                    .map(|s| selftest::parse_steps(&s))
                    .unwrap_or_default()
            }
            "--hold-ms" => hold_ms = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--sample-at-100" => sample_at_100 = true,
            other if !other.starts_with("--") => open = Some(PathBuf::from(other)),
            _ => {}
        }
    }
    if settings.is_none() {
        settings = dir.as_ref().map(|d| d.join("settings.toml"));
    }
    LaunchOptions {
        open,
        settings,
        self_test: dir.map(|dir| selftest::Plan {
            dir,
            steps,
            hold_ms,
            sample_at_100,
        }),
    }
}

/// 缩略图生成器（freedesktop 的缩略图规范，`.thumbnailer` 文件写
/// `Exec=anyview --thumbnail %i %o %s`）：解码输入文件，缩到长边不超过 `size`，写成 PNG。不初始化 GTK、不读设置。
/// 成功返回 0，解不了返回 1，参数不对返回 2。
fn run_thumbnailer(args: &[String]) -> glib::ExitCode {
    let [input, output, size] = args else {
        eprintln!("用法：anyview --thumbnail <图片> <输出的 PNG> <边长>");
        return glib::ExitCode::from(2);
    };
    let Ok(size) = size.parse::<u32>() else {
        eprintln!("边长不对：{size}");
        return glib::ExitCode::from(2);
    };
    // 系统里有 libheif 时由它解 HEIC、AVIF。
    anyview_core::heif::register();
    let result = anyview_core::thumbnail::file_thumbnail(std::path::Path::new(input), size)
        .and_then(|image| anyview_core::thumbnail::encode_png(&image))
        .and_then(|png| std::fs::write(output, png).map_err(|e| e.to_string()));
    match result {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("{input}：{reason}");
            glib::ExitCode::FAILURE
        }
    }
}

fn main() -> glib::ExitCode {
    MAIN_START.get_or_init(Instant::now);
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().map(String::as_str) == Some("--thumbnail") {
        return run_thumbnailer(&raw[1..]);
    }
    // 打包用：打出安装包里的桌面文件（MIME 类型列表在核心里，只有一份），不初始化 GTK。
    if raw.first().map(String::as_str) == Some("--print-desktop-entry") {
        print!("{}", default_viewer::packaged_desktop_entry());
        return glib::ExitCode::SUCCESS;
    }
    let options = parse_args();
    // 系统里有 libheif 时由它解 HEIC、AVIF；要在打开第一张图、扫描文件夹之前登记。
    anyview_core::heif::register();
    // 先建核心、读设置。
    let mut early = window::EarlyCore::new(options.settings.as_deref());
    // 不允许多个实例：已经有开着的就把图片交给它，自己退出（这时还没初始化 GTK，很快）。
    if early.reuse_window()
        && let Some(socket) = instance::socket_path()
        && instance::forward(&socket, options.open.as_deref())
    {
        return glib::ExitCode::SUCCESS;
    }
    // 开始读图：解码与 GTK 初始化、建窗口同时进行，窗口出来时图往往已经好了。
    if let Some(path) = options.open.as_deref() {
        early.open(path);
    }
    let early = std::cell::RefCell::new(Some(early));
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        // 每个窗口一个进程；"不允许多个实例"由我们自己的套接字处理（instance.rs），
        // 不用 GApplication 的会话总线（无头测试环境也没有会话总线）。
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| load_css());
    app.connect_activate(move |app| {
        if let Some(early) = early.borrow_mut().take() {
            window::build(app, &options, early);
        }
    });
    // 参数由我们自己解析，不交给 GApplication。
    let program = std::env::args().next().unwrap_or_else(|| "anyview".into());
    app.run_with_args(&[program])
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        use_app_icon(&display);
    }
}

/// 窗口的图标：按应用 ID 在图标主题里找。装好的程序图标在系统的 hicolor 主题里；
/// 直接从仓库里运行的开发版另外把仓库的 apps/linux/data/icons 加进查找路径（只加路径，用到时才读文件，不拖慢启动）。
fn use_app_icon(display: &gdk::Display) {
    gtk::Window::set_default_icon_name(APP_ID);
    let exe = std::env::current_exe().ok();
    let repo_icons = exe
        .as_deref()
        .into_iter()
        .flat_map(std::path::Path::ancestors)
        .map(|dir| dir.join("apps/linux/data/icons"))
        .find(|dir| dir.join("hicolor").is_dir());
    if let Some(dir) = repo_icons {
        gtk::IconTheme::for_display(display).add_search_path(dir);
    }
}
