//! 无头自测（apps/linux/scripts/selftest.sh）。
//!
//! - 不带 `--open`：空态自测。收到 Pong 后截图，核对背景色 #202020。
//! - 带 `--open`：打开路径，再按 `--steps` 依次执行动作（动作 ID，如 next_image、rotate_cw、zoom_in；
//!   也接受 next、prev、first、last、forward10、back10）。切图的步骤等图像就绪，其他步骤等画面更新，然后截图。
//!   每张截图都做"画面核对"：把图片上 16 个点按核心算出的位置映射到屏幕，读截图里的颜色与原图比较，
//!   验证平移、缩放、旋转、翻转确实按核心的计算画出来。
//!   透明处应显示棋盘格；半透明的点跳过；被悬浮界面挡住的点跳过。
//!   每一步还核对悬浮界面的状态：控制栏只在锁定时显示（无头环境没有指针），图片信息的开关和文字与核心一致，
//!   进度条滑块位置与序号一致。步骤 `bar_jump:0.5` 表示在进度条一半处单击（与松开鼠标时的处理相同）。
//!   到达首尾时的询问不弹对话框：记录下来，由下一步（如 first_image）充当用户的选择。
//!   删除（delete_file）不弹确认框，直接移到回收站，等下一张显示后核对文件已经不在原处；
//!   复制（copy_to_clipboard）核对剪贴板里同时有图像和文件，图像是原图（开着反相复制，正中的像素与原图相同）。
//!   步骤 `wait_exact` 等精确重绘盖上画面（最多 3 秒），再截图核对；记录位置和重绘耗时。
//!   性能测量：`wait:毫秒` 等一会儿；`idle:毫秒` 测这段时间的 CPU 占用和内存峰值；
//!   `open:路径` 在运行中打开新的图片或文件夹。第一张图记录从进程启动到出图的时间（冷启动）。
//!   大 JPEG 先显示快速预览时另记出预览的时间（msToPreview）；截图等清晰图。
//!   超大图（代理纹理）需要精确重绘时，等它盖上再截图（最多 3 秒）。
//!   `--sample-at-100`：只在 100% 时核对画面（真实照片缩小显示时，滤波后的颜色本来就与单个像素不同）。
//!   设置（docs/SETTINGS.md）：`settings_open` 打开设置窗口，`settings_page:页` 切页，`settings_set:键=值` 像用户那样
//!   改控件（开关 true/false、下拉框选项 ID、数字和路径的文字、颜色 #RRGGBB），`settings_ok` 点"确定"（要求生效），
//!   `settings_ok_invalid` 点"确定"（要求校验失败），`settings_reset` 点"重置"，`settings_cancel` 放弃修改并关窗口。
//!   设置窗口开着时每一步另截一张设置窗口的图，并记下这一页每一项的文字、是否可用和值。
//!   `key:按键` 模拟按下一个键（交给核心解析，记下对应的动作）；`hover_bottom` 模拟指针移到窗口底部。
//!   任何提示（如已到末尾）、动作不可用或核对不符都算不通过。最后写出 selftest.json 并退出，退出码 0 表示通过。

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyview_core::actions::{MenuEntry, MenuKind};
use anyview_core::overlay::progress_fraction;
use anyview_core::settings::{AlphaFill, Control, MouseInput};
use anyview_core::{ActionOutcome, DecodedImage, Layout, Platform, SettingsApplied, UiRequest};
use gtk::prelude::*;
use gtk::{glib, graphene};

use crate::window::{MouseTarget, Ui};

/// 窗口背景色。
const BACKGROUND_HEX: &str = "#202020";
/// 采样点离左上角 24 像素，避开窗口圆角，也不会碰到居中的文字。
const SAMPLE_POINT: (usize, usize) = (24, 24);
/// 整个自测的时限。
const TIME_LIMIT: Duration = Duration::from_secs(120);
/// 画面核对的颜色容差（每个通道）。采样点离色块边界较远，只有缩放滤波带来的细小差别。
const COLOR_TOLERANCE: i32 = 40;
/// 透明处的棋盘格颜色。
const CHECKER_COLORS: [[u8; 3]; 2] = [[255, 255, 255], [200, 200, 200]];

#[derive(Clone)]
pub struct Plan {
    pub dir: PathBuf,
    pub steps: Vec<String>,
    pub hold_ms: u64,
    /// 只在 100% 时核对画面。
    pub sample_at_100: bool,
}

pub fn parse_steps(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|step| {
            match step {
                "next" => "next_image",
                "prev" => "prev_image",
                "first" => "first_image",
                "last" => "last_image",
                "forward10" => "forward_10",
                "back10" => "back_10",
                other => other,
            }
            .to_string()
        })
        .collect()
}

pub struct Runner {
    plan: Plan,
    open: Option<PathBuf>,
    started: Instant,
    step_started: Instant,
    records: Vec<serde_json::Value>,
    failure: Option<String>,
    /// 正在等图像就绪的步骤（打开或切图）。
    waiting_image: bool,
    /// 刚移到回收站的文件：下一张显示后核对它已不在原处。
    deleted: Option<PathBuf>,
    /// 这一步出快速预览的时间（毫秒，从这一步开始算）和从进程启动算的时间。
    preview_ms: Option<f64>,
    preview_since_process: Option<f64>,
    /// 模拟了指针移到窗口底部（`hover_bottom`）：控制栏应当显示（除非设置了不显示）。
    hover_bar: bool,
    /// 最近一次"确定"生效的结果。
    applied: Option<SettingsApplied>,
    /// 这一步要等图像就绪、到时一起写进记录的内容（按键、鼠标步骤执行了翻页时）。
    pending_extra: Option<serde_json::Value>,
    /// 等别的进程交来图片（`spawn_open`）。
    expect_forward: bool,
    /// 这一步开始以后收到的动图帧数（`wait_frames`）。
    frames_seen: usize,
    /// 等复制完成的提示（`copy_to_folder1` 等）。
    waiting_notice: bool,
    /// 最近一次显示过的提示（复制、移动完成，门户交回的结果等；提示只显示 2 秒，后面的步骤从这里报告）。
    last_notice: Option<String>,
    /// 交给外部的操作（编辑器、桌面图片、打印）：自测时只记下，不真的做。
    externals: Vec<serde_json::Value>,
    /// 启动过的别的进程（`spawn_open`）：结束时核对它们都正常退出。
    children: Vec<(String, std::process::Child, Instant)>,
    /// 上一张和这一张图片中心的颜色（核对翻页效果的中间画面）。
    previous_color: Option<[u8; 3]>,
    current_color: Option<[u8; 3]>,
    finished: bool,
}

/// `opened_at`：启动时已经交给核心的打开的时刻；为空时由自测自己打开。
pub fn start(ui: &Rc<Ui>, plan: Plan, open: Option<PathBuf>, opened_at: Option<Instant>) {
    let _ = std::fs::create_dir_all(&plan.dir);
    let now = Instant::now();
    *ui.selftest.borrow_mut() = Some(Runner {
        plan,
        open: open.clone(),
        started: now,
        step_started: opened_at.unwrap_or(now),
        records: Vec::new(),
        failure: None,
        waiting_image: open.is_some(),
        deleted: None,
        preview_ms: None,
        preview_since_process: None,
        hover_bar: false,
        applied: None,
        pending_extra: None,
        expect_forward: false,
        frames_seen: 0,
        waiting_notice: false,
        last_notice: None,
        externals: Vec::new(),
        children: Vec::new(),
        previous_color: None,
        current_color: None,
        finished: false,
    });
    if let (Some(path), None) = (open, opened_at) {
        ui.open(&path);
    }
    let weak = Rc::downgrade(ui);
    glib::timeout_add_local_once(TIME_LIMIT, move || {
        if let Some(ui) = weak.upgrade() {
            fail(&ui, "超时");
        }
    });
}

/// 空态自测：收到 Pong 后等一会儿让窗口完成首帧，再截图。
pub fn on_pong(ui: &Rc<Ui>) {
    let empty_mode = ui
        .selftest
        .borrow()
        .as_ref()
        .is_some_and(|r| r.open.is_none() && !r.finished);
    if empty_mode {
        let weak = Rc::downgrade(ui);
        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            if let Some(ui) = weak.upgrade() {
                finish(&ui);
            }
        });
    }
}

/// 设置窗口的"确定"已经生效。
pub fn on_settings_applied(ui: &Rc<Ui>, applied: &SettingsApplied) {
    if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
        runner.applied = Some(applied.clone());
    }
}

/// 快速预览显示出来了：记下时间，继续等清晰图。
pub fn on_preview(ui: &Rc<Ui>) {
    let mut guard = ui.selftest.borrow_mut();
    let Some(runner) = guard.as_mut() else { return };
    if runner.finished || !runner.waiting_image || runner.preview_ms.is_some() {
        return;
    }
    runner.preview_ms = Some(runner.step_started.elapsed().as_secs_f64() * 1000.0);
    if runner.records.is_empty() {
        runner.preview_since_process = process_age_ms();
    }
}

pub fn on_image(ui: &Rc<Ui>, image: &Arc<DecodedImage>) {
    {
        let mut guard = ui.selftest.borrow_mut();
        let Some(runner) = guard.as_mut() else { return };
        if runner.finished || !runner.waiting_image {
            return;
        }
        runner.waiting_image = false;
        runner.previous_color = runner.current_color;
        runner.current_color =
            image_pixel(image, image.width / 2, image.height / 2).map(|(rgb, _)| rgb);
        let (index, total) = ui.current_position();
        let label = step_label(runner);
        let deleted_gone = runner.deleted.take().map(|path| !path.exists());
        // 第一张图：从进程启动、从 main 开始到出图的时间（冷启动）。
        let (since_process, since_main) = if runner.records.is_empty() {
            (process_age_ms(), Some(crate::MAIN_START.get().map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0)))
        } else {
            (None, None)
        };
        let preview_ms = runner.preview_ms.take();
        let preview_since_process = runner.preview_since_process.take();
        let extra = runner.pending_extra.take();
        runner.records.push(serde_json::json!({
            "step": label,
            "msSinceProcessStart": since_process,
            "msSinceMain": since_main,
            "msToPreview": preview_ms,
            "previewMsSinceProcessStart": preview_since_process,
            "hasProxy": image.proxy.is_some(),
            // 按图片调整的窗口大小和窗口现在的大小（逻辑像素；无头 sway 不一定照办，只记录）。
            "fitWindowPx": ui.last_fit.get().map(|(w, h)| [w, h]),
            "windowPx": [ui.window.width(), ui.window.height()],
            "deletedGone": deleted_gone,
            "notice": ui.notice_text(),
            "name": ui.current_name(),
            "index": index,
            "total": total,
            "msToImage": runner.step_started.elapsed().as_secs_f64() * 1000.0,
            "decodeMs": image.info.decode_ms,
            "width": image.width,
            "height": image.height,
            "format": image.info.format,
            "pixelFormat": image.info.pixel_format,
        }));
        if let (Some(serde_json::Value::Object(extra)), Some(record)) =
            (extra, runner.records.last_mut().and_then(|r| r.as_object_mut()))
        {
            record.extend(extra);
        }
    }
    capture_later(ui);
}

fn step_label(runner: &Runner) -> String {
    match runner.records.len() {
        0 => "open".to_string(),
        n => runner.plan.steps[n - 1].clone(),
    }
}

/// 等窗口完成布局和绘制再截图，然后进行下一步。
fn capture_later(ui: &Rc<Ui>) {
    let weak = Rc::downgrade(ui);
    // 留出淡入（150 ms）的时间。
    glib::timeout_add_local_once(Duration::from_millis(350), move || {
        if let Some(ui) = weak.upgrade() {
            capture_when_settled(&ui, Instant::now());
        }
    });
}

/// 超大图（代理纹理）放大显示时，GPU 画面本来就不清晰：等精确重绘盖上再截图（最多 3 秒）。
fn capture_when_settled(ui: &Rc<Ui>, started: Instant) {
    let proxied = ui.canvas.image().is_some_and(|i| i.proxy.is_some());
    // 翻页效果在走（没有停住）时等它做完。
    let turning = ui.canvas.transition_state().is_some() && ui.canvas.held_progress().is_none();
    let pending =
        turning || (proxied && ui.core.needs_exact() && ui.canvas.exact_shown().is_none());
    if !pending || started.elapsed() > Duration::from_secs(3) {
        capture_step(ui);
        return;
    }
    let weak = Rc::downgrade(ui);
    glib::timeout_add_local_once(Duration::from_millis(50), move || {
        if let Some(ui) = weak.upgrade() {
            capture_when_settled(&ui, started);
        }
    });
}

fn capture_step(ui: &Rc<Ui>) {
    let step = {
        let guard = ui.selftest.borrow();
        match guard.as_ref() {
            Some(runner) if !runner.finished => runner.records.len() - 1,
            _ => return,
        }
    };
    let file = format!("step-{step}.png");
    let shot = capture(&ui.window, &dir_of(ui).join(&file));
    // 设置窗口开着时另截一张，并记下这一页的内容。
    let settings = ui.settings_window.borrow().clone().map(|w| {
        let settings_file = format!("step-{step}-settings.png");
        let shot = capture(&w.window, &dir_of(ui).join(&settings_file));
        serde_json::json!({
            "screenshot": match &shot {
                Ok(_) => serde_json::json!(settings_file),
                Err(e) => serde_json::json!({ "error": e }),
            },
            "page": settings_page_json(ui, &w.current_page()),
            "dirty": ui.core.settings_dirty(),
        })
    });
    // 选择图片窗口开着时另截一张，并记下它的状态。
    let picker = ui.picker.borrow().clone().map(|p| {
        let picker_file = format!("step-{step}-picker.png");
        let shot = capture(&p.window, &dir_of(ui).join(&picker_file));
        let mut json = picker_json(ui, &p);
        json["screenshot"] = match &shot {
            Ok(_) => serde_json::json!(picker_file),
            Err(e) => serde_json::json!({ "error": e }),
        };
        json
    });
    let layout = ui.core.view_layout();
    // 翻页效果停在中间时（transition_hold），画面是两张叠在一起的：按效果核对几个点，不做平常的画面核对。
    let transition = ui.canvas.transition_state();
    let check = match (&shot, transition) {
        (Ok(shot), Some(state)) => transition_check(ui, shot, state),
        (Ok(shot), None) => check_samples(ui, shot),
        (Err(e), _) => serde_json::json!({ "checked": 0, "error": e }),
    };
    let skipped = check.get("skipped").is_some();
    let check_ok = skipped
        || (check["checked"].as_u64().unwrap_or(0) > 0
            && check["mismatches"].as_array().is_some_and(|m| m.is_empty()));
    let (overlays, overlay_problems) = overlay_state(ui);
    let runner_notice = ui
        .selftest
        .borrow()
        .as_ref()
        .and_then(|r| r.last_notice.clone());
    {
        let mut guard = ui.selftest.borrow_mut();
        let Some(runner) = guard.as_mut() else { return };
        let record = &mut runner.records[step];
        record["screenshot"] = match &shot {
            Ok(_) => serde_json::json!(file),
            Err(e) => serde_json::json!({ "error": e }),
        };
        record["layout"] = layout.map(layout_json).unwrap_or(serde_json::Value::Null);
        record["samples"] = check;
        record["overlays"] = overlays;
        // 窗口左上角的颜色：图片没盖住时就是背景色（设置里可改）。
        record["cornerColor"] = match &shot {
            Ok(shot) => serde_json::json!(shot.corner_color()),
            Err(_) => serde_json::Value::Null,
        };
        if let Some(settings) = settings {
            record["settings"] = settings;
        }
        record["picker"] = picker.unwrap_or(serde_json::Value::Null);
        record["index"] = serde_json::json!(ui.current_position().0);
        // 窗口大小：实际大小和程序最近一次设定的大小（无头 sway 不一定照办，只记录）。
        record["windowPx"] = serde_json::json!([ui.window.width(), ui.window.height()]);
        // 左键拖动这时会做什么：移动窗口还是平移图片。
        record["leftDragMovesWindow"] =
            serde_json::json!(ui.core.left_drag_moves_window(ui.window_movable()));
        record["expectedWindowPx"] = serde_json::json!(ui.last_fit.get().map(|(w, h)| [w, h]));
        // 动图：播放状态和左下角动画信息控制的文字。
        record["animation"] = match ui.core.animation() {
            Some(a) => serde_json::json!({ "index": a.index, "count": a.count, "playing": a.playing }),
            None => serde_json::Value::Null,
        };
        record["animationText"] = serde_json::json!(ui.overlays.animation_text());
        // 压缩包里不能删除；记住的压缩包位置有几个；最近一次复制、移动的提示。
        record["deleteEnabled"] = serde_json::json!(ui.core.action_state("delete_file").enabled);
        record["archivePositions"] = serde_json::json!(ui.core.archive_positions_count());
        record["lastNotice"] = serde_json::json!(runner_notice);
        record["inhibiting"] = serde_json::json!(ui.inhibiting());
        // 翻页音效放过几次、用的哪种方式。
        record["sounds"] = serde_json::json!(*ui.sound_log.borrow());
        // 背景图片。
        record["background"] = serde_json::json!(ui.canvas.has_background());
    }
    if !check_ok {
        fail(ui, &format!("第 {step} 步画面核对不符"));
        return;
    }
    if !overlay_problems.is_empty() {
        fail(
            ui,
            &format!("第 {step} 步悬浮界面不对：{}", overlay_problems.join("；")),
        );
        return;
    }
    advance(ui);
}

/// 执行下一个动作；没有了就结束。
fn advance(ui: &Rc<Ui>) {
    let next = {
        let mut guard = ui.selftest.borrow_mut();
        let Some(runner) = guard.as_mut() else { return };
        runner.step_started = Instant::now();
        runner.frames_seen = 0;
        runner.plan.steps.get(runner.records.len() - 1).cloned()
    };
    let Some(id) = next else {
        finish(ui);
        return;
    };
    if id == "wait_exact" {
        wait_exact(ui, Instant::now());
        return;
    }
    // 被上限拦下后用户的选择：仍然打开（等图片显示）、取消。
    if id == "limit_open" || id == "limit_cancel" {
        let Some(path) = ui.core.current_path() else {
            fail(ui, "没有当前图片");
            return;
        };
        if id == "limit_open" {
            if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
                runner.waiting_image = true;
            }
            ui.core.open_without_limits(&path);
        } else {
            ui.core.keep_limits(&path);
            record_step(ui, serde_json::json!({}));
            capture_later(ui);
        }
        return;
    }
    if let Some(ms) = id.strip_prefix("wait:").and_then(|v| v.parse::<u64>().ok()) {
        let weak = Rc::downgrade(ui);
        glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            if let Some(ui) = weak.upgrade() {
                record_step(&ui, serde_json::json!({}));
                capture_later(&ui);
            }
        });
        return;
    }
    if let Some(ms) = id.strip_prefix("idle:").and_then(|v| v.parse::<u64>().ok()) {
        // 静止不操作时 CPU 占用应接近 0%（没有定时重绘）。
        let cpu_before = cpu_seconds();
        let weak = Rc::downgrade(ui);
        glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            let Some(ui) = weak.upgrade() else { return };
            let percent = match (cpu_before, cpu_seconds()) {
                (Some(a), Some(b)) => Some((b - a) / (ms as f64 / 1000.0) * 100.0),
                _ => None,
            };
            record_step(
                &ui,
                serde_json::json!({
                    "idleCpuPercent": percent,
                    "peakRssMb": peak_rss_mb(),
                    // 静止时的内存占用；峰值里还有自测截图等临时内存。
                    "rssMb": rss_mb(),
                    // 图片缓存占用（遵守缓存上限，默认 1024 MB）。
                    "cacheMb": ui.core.cache_bytes() as f64 / 1_048_576.0,
                }),
            );
            capture_later(&ui);
        });
        return;
    }
    if let Some(path) = id.strip_prefix("open:") {
        if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
            runner.waiting_image = true;
        }
        ui.open(Path::new(path));
        return;
    }
    if settings_step(ui, &id) {
        return;
    }
    if picker_step(ui, &id) {
        return;
    }
    // generation:state 记下“生成信息”面板开着没有、显示的是什么，开着时截一张面板的图；generation:close 关掉它。
    // 面板由 key:P（或菜单）打开。
    if let Some(what) = id.strip_prefix("generation:") {
        if what == "close" {
            crate::generation_window::close();
        }
        let weak = Rc::downgrade(ui);
        glib::timeout_add_local_once(Duration::from_millis(400), move || {
            let Some(ui) = weak.upgrade() else { return };
            let (open, shown) = crate::generation_window::state();
            let step = ui.selftest.borrow().as_ref().map_or(0, |r| r.records.len());
            let file = format!("step-{step}-generation.png");
            let shot = crate::generation_window::window().map(|w| capture(&w, &dir_of(&ui).join(&file)));
            record_step(
                &ui,
                serde_json::json!({
                    "generation": {
                        "open": open,
                        "tool": shown.as_ref().map(|s| s.0.clone()),
                        "prompt": shown.as_ref().map(|s| s.1.clone()),
                        "enabled": ui.core.action_state("show_generation_info").enabled,
                        "screenshot": match &shot {
                            Some(Ok(_)) => serde_json::json!(file),
                            Some(Err(e)) => serde_json::json!({ "error": e }),
                            None => serde_json::Value::Null,
                        },
                    }
                }),
            );
            capture_later(&ui);
        });
        return;
    }
    // help:shortcuts、help:formats、help:licenses 打开帮助窗口，截一张帮助窗口的图、记下行数
    // （开源许可记下文字有多长、有没有 Anyview 的 MIT 许可证），然后关掉它。
    if let Some(kind) = id.strip_prefix("help:") {
        let (action, kind) = match kind {
            "shortcuts" => ("help_shortcuts", anyview_core::help::HelpKind::Shortcuts),
            "licenses" => ("help_licenses", anyview_core::help::HelpKind::Licenses),
            _ => ("help_formats", anyview_core::help::HelpKind::Formats),
        };
        ui.invoke(action);
        let weak = Rc::downgrade(ui);
        glib::timeout_add_local_once(Duration::from_millis(400), move || {
            let Some(ui) = weak.upgrade() else { return };
            let window = ui.help_windows.borrow().last().cloned();
            let Some(window) = window else {
                fail(&ui, "帮助窗口没有打开");
                return;
            };
            let step = ui.selftest.borrow().as_ref().map_or(0, |r| r.records.len());
            let file = format!("step-{step}-help.png");
            let shot = capture(&window, &dir_of(&ui).join(&file));
            let rows = ui.core.help_rows(kind, ui.lang());
            let (headings, lines) = crate::help_window::row_count(&rows);
            let text = ui.core.help_text(kind, ui.lang()).unwrap_or_default();
            record_step(
                &ui,
                serde_json::json!({
                    "help": {
                        "title": window.title().map(|t| t.to_string()),
                        "headings": headings,
                        "rows": lines,
                        "textLength": text.len(),
                        "anyviewLicense": text.contains("Copyright (c) 2026 Anyview contributors"),
                        "first": rows.iter().find(|r| !r.heading).map(|r| format!("{} {}", r.left, r.right)),
                        "screenshot": match &shot { Ok(_) => serde_json::json!(file), Err(e) => serde_json::json!({ "error": e }) },
                    }
                }),
            );
            window.close();
            capture_later(&ui);
        });
        return;
    }
    // edit_settings_file:键=TOML 值：像另一个进程那样直接改设置文件，等这个窗口读到（最多 5 秒）。
    if let Some(assignment) = id.strip_prefix("edit_settings_file:") {
        edit_settings_file(ui, assignment);
        return;
    }
    // transition_hold:进度 让下一次翻页效果停在这个进度（截图核对中间画面）；transition_hold:off 取消。
    if let Some(value) = id.strip_prefix("transition_hold:") {
        ui.canvas.hold_transition(value.parse::<f64>().ok());
        record_step(ui, serde_json::json!({}));
        capture_later(ui);
        return;
    }
    if instance_step(ui, &id) {
        return;
    }
    // add_file:源文件、remove_file:文件名：在当前图片的文件夹里加一个文件、删一个文件，
    // 等浏览集合的张数变了（文件夹变化自动刷新，最多 6 秒）。
    if let Some(src) = id.strip_prefix("add_file:") {
        let before = ui.current_position().1;
        let Some(folder) = ui.core.current_path().and_then(|p| p.parent().map(Path::to_path_buf)) else {
            fail(ui, "没有当前文件夹");
            return;
        };
        let src = Path::new(src);
        let target = folder.join(src.file_name().unwrap_or_default());
        if let Err(e) = std::fs::copy(src, &target) {
            fail(ui, &format!("复制 {} 失败：{e}", src.display()));
            return;
        }
        wait_total(ui, before, Instant::now());
        return;
    }
    if let Some(name) = id.strip_prefix("remove_file:") {
        let before = ui.current_position().1;
        let Some(folder) = ui.core.current_path().and_then(|p| p.parent().map(Path::to_path_buf)) else {
            fail(ui, "没有当前文件夹");
            return;
        };
        if let Err(e) = std::fs::remove_file(folder.join(name)) {
            fail(ui, &format!("删除 {name} 失败：{e}"));
            return;
        }
        wait_total(ui, before, Instant::now());
        return;
    }
    // wait_frames:N：等动图再换 N 帧（最多 5 秒），记下看到的帧数。
    if let Some(n) = id.strip_prefix("wait_frames:").and_then(|v| v.parse::<usize>().ok()) {
        wait_frames(ui, n, Instant::now());
        return;
    }
    // window:宽x高：把窗口设成这个大小（逻辑像素），模拟用户拖动窗口边缘。
    if let Some(size) = id.strip_prefix("window:") {
        let parts: Vec<i32> = size.split('x').filter_map(|v| v.parse().ok()).collect();
        if parts.len() != 2 {
            fail(ui, &format!("步骤写错了：{id}"));
            return;
        }
        ui.window.set_default_size(parts[0], parts[1]);
        record_step(ui, serde_json::json!({}));
        capture_later(ui);
        return;
    }
    if let Some(fraction) = id
        .strip_prefix("bar_jump:")
        .and_then(|f| f.parse::<f64>().ok())
    {
        match ui.overlays.jump_to_fraction(ui, fraction) {
            Some(_) => {
                if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
                    runner.waiting_image = true;
                }
            }
            None => fail(ui, &format!("进度条还不能跳转：{id}")),
        }
        return;
    }
    invoke_step(ui, &id, serde_json::json!({}));
}

/// 执行一个动作作为这一步，按结果等待图像或记录、截图。`extra` 一起写进这一步的记录。
fn invoke_step(ui: &Rc<Ui>, id: &str, extra: serde_json::Value) {
    // 关闭图片：回到空态，没有新图像要等。
    if id == "close_image" {
        ui.invoke(id);
        record_step(ui, extra);
        capture_later(ui);
        return;
    }
    match ui.invoke(id) {
        // 复制到图片文件夹：在后台复制，等完成的提示。
        ActionOutcome::Started if id.starts_with("copy_to_folder") => {
            if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
                runner.waiting_notice = true;
                runner.pending_extra = Some(extra);
            }
        }
        // 编辑器、桌面图片、打印：自测时只记下交给外部的内容。
        ActionOutcome::Ui(
            UiRequest::OpenInEditor { .. } | UiRequest::SetWallpaper { .. } | UiRequest::Print { .. },
        ) => {
            let last = ui
                .selftest
                .borrow()
                .as_ref()
                .and_then(|r| r.externals.last().cloned());
            let mut extra = extra;
            extra["external"] = last.unwrap_or(serde_json::Value::Null);
            record_step(ui, extra);
            capture_later(ui);
        }
        ActionOutcome::Started => {
            if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
                runner.waiting_image = true;
                runner.pending_extra = Some(extra);
            }
        }
        // “生成信息”面板：像用户按 P 一样打开面板，再记录（面板的内容由 generation:state 记）。
        ActionOutcome::Ui(UiRequest::ShowGenerationInfo) => {
            crate::generation_window::show(ui);
            record_step(ui, extra);
            capture_later(ui);
        }
        // 右键菜单是单独的弹出层，不在窗口截图里；由脚本用合成器截图（grim）查看。
        ActionOutcome::ViewChanged { .. }
        | ActionOutcome::OverlayChanged
        | ActionOutcome::Ui(UiRequest::ShowContextMenu) => {
            record_step(ui, extra);
            capture_later(ui);
        }
        // 最大化、还原，进入、退出全屏：窗口管理器决定结果，这里只记录。
        ActionOutcome::Ui(UiRequest::ToggleFullscreen) => {
            let mut extra = extra;
            extra["fullscreen"] = serde_json::json!(ui.window.is_fullscreen());
            record_step(ui, extra);
            capture_later(ui);
        }
        ActionOutcome::Ui(UiRequest::ToggleMaximize) => {
            let mut extra = extra;
            extra["maximized"] = serde_json::json!(ui.window.is_maximized());
            record_step(ui, extra);
            capture_later(ui);
        }
        ActionOutcome::Ui(UiRequest::TrashFile { path, .. }) => {
            if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
                runner.deleted = Some(PathBuf::from(path));
                runner.waiting_image = true;
            }
        }
        ActionOutcome::Ui(UiRequest::CopyImage { .. }) => {
            let clipboard = ui.window.clipboard();
            let formats = clipboard.formats();
            let image = formats.contains_type(gtk::gdk::Texture::static_type());
            let files = formats.contains_type(gtk::gdk::FileList::static_type());
            // 剪贴板里的是原图，不是画面上加了反相等效果的图像。比较三者正中那个像素（BGRA 预乘）。
            let copied = clipboard
                .content()
                .and_then(|p| p.value(gtk::gdk::Texture::static_type()).ok())
                .and_then(|v| v.get::<gtk::gdk::Texture>().ok())
                .map(|t| texture_center(&t));
            let original = ui.core.original_image().map(|i| image_center(&i));
            let displayed = ui.canvas.image().map(|i| image_center(&i));
            let rgba_hex = |c: Option<[u8; 4]>| {
                c.map(|c| format!("{:02X}{:02X}{:02X}{:02X}", c[2], c[1], c[0], c[3]))
            };
            if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
                let label = step_label(runner);
                runner.records.push(serde_json::json!({
                    "step": label,
                    "name": ui.current_name(),
                    "clipboard": {
                        "image": image,
                        "files": files,
                        "centerRGBA": rgba_hex(copied),
                        "originalCenterRGBA": rgba_hex(original),
                        "displayedCenterRGBA": rgba_hex(displayed),
                    },
                }));
            }
            if !(image && files) {
                fail(ui, &format!("剪贴板内容不全：图像 {image}，文件 {files}"));
                return;
            }
            let close = |a: [u8; 4], b: [u8; 4]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2);
            match (copied, original) {
                (Some(c), Some(o)) if close(c, o) => {}
                _ => {
                    fail(
                        ui,
                        &format!(
                            "剪贴板里的图像不是原图：正中 {:?}，原图 {:?}，画面 {:?}",
                            rgba_hex(copied),
                            rgba_hex(original),
                            rgba_hex(displayed)
                        ),
                    );
                    return;
                }
            }
            capture_later(ui);
        }
        other => fail(ui, &format!("动作 {id} 没有执行：{other:?}")),
    }
}

/// 复制、移动完成的提示。等着它的步骤这时记下、截图。
pub fn on_notice(ui: &Rc<Ui>, kind: &anyview_core::NoticeKind) {
    let text = kind.text(ui.lang());
    let waiting = {
        let mut guard = ui.selftest.borrow_mut();
        let Some(runner) = guard.as_mut() else { return };
        runner.last_notice = Some(text.clone());
        std::mem::take(&mut runner.waiting_notice)
    };
    if waiting {
        let extra = ui
            .selftest
            .borrow_mut()
            .as_mut()
            .and_then(|r| r.pending_extra.take())
            .unwrap_or_else(|| serde_json::json!({}));
        let mut extra = extra;
        extra["notice"] = serde_json::json!(text);
        record_step(ui, extra);
        capture_later(ui);
    }
}

/// 交给外部的操作（编辑器、桌面图片、打印）：自测时只记下来，返回 true（不真的做）。
pub fn on_external(ui: &Rc<Ui>, what: &str, detail: &str) -> bool {
    let mut guard = ui.selftest.borrow_mut();
    let Some(runner) = guard.as_mut() else {
        return false;
    };
    runner
        .externals
        .push(serde_json::json!({ "what": what, "detail": detail }));
    // 真实桌面的检查在用完即删的容器里真的做这几种（如设壁纸），用逗号分隔的名字指定；平时只记下来。
    !std::env::var("ANYVIEW_SELFTEST_RUN_EXTERNAL").is_ok_and(|names| names.split(',').any(|n| n == what))
}

/// 动图换了一帧。
pub fn on_frame(ui: &Rc<Ui>) {
    if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
        runner.frames_seen += 1;
    }
}

fn wait_total(ui: &Rc<Ui>, before: Option<usize>, started: Instant) {
    let total = ui.current_position().1;
    if total != before || started.elapsed() > Duration::from_secs(6) {
        record_step(
            ui,
            serde_json::json!({ "total": total, "ms": started.elapsed().as_secs_f64() * 1000.0 }),
        );
        if total == before {
            fail(ui, "6 秒内浏览集合没有变");
            return;
        }
        capture_later(ui);
        return;
    }
    let weak = Rc::downgrade(ui);
    glib::timeout_add_local_once(Duration::from_millis(100), move || {
        if let Some(ui) = weak.upgrade() {
            wait_total(&ui, before, started);
        }
    });
}

fn wait_frames(ui: &Rc<Ui>, n: usize, started: Instant) {
    let seen = ui.selftest.borrow().as_ref().map_or(0, |r| r.frames_seen);
    if seen >= n || started.elapsed() > Duration::from_secs(5) {
        record_step(
            ui,
            serde_json::json!({ "framesSeen": seen, "ms": started.elapsed().as_secs_f64() * 1000.0 }),
        );
        if seen < n {
            fail(ui, &format!("5 秒内只换了 {seen} 帧（应至少 {n} 帧）"));
            return;
        }
        capture_later(ui);
        return;
    }
    let weak = Rc::downgrade(ui);
    glib::timeout_add_local_once(Duration::from_millis(20), move || {
        if let Some(ui) = weak.upgrade() {
            wait_frames(&ui, n, started);
        }
    });
}

/// 别的进程交来了图片（不允许多个实例）：等它显示出来再记这一步。
pub fn on_forwarded(ui: &Rc<Ui>, path: &Path) {
    if let Some(runner) = ui.selftest.borrow_mut().as_mut()
        && runner.expect_forward
    {
        runner.expect_forward = false;
        runner.waiting_image = true;
        runner.pending_extra = Some(serde_json::json!({ "forwarded": path.display().to_string() }));
    }
}

/// 多个实例的步骤；不是这类步骤时返回 false。
/// - `spawn_separate:路径`：像从文件管理器那样再启动一个 anyview（带自己的自测，用同一个设置文件）。
///   "不允许多个实例"没勾时它自己开窗口、做完自测后退出，这个窗口的图片不变。
/// - `spawn_open:路径`：再启动一个 anyview 打开图片（不带自测）。勾上"不允许多个实例"后它把图片交给这个进程、
///   马上退出，这个窗口换成那张图片。
fn instance_step(ui: &Rc<Ui>, id: &str) -> bool {
    let (separate, path) = if let Some(path) = id.strip_prefix("spawn_separate:") {
        (true, PathBuf::from(path))
    } else if let Some(path) = id.strip_prefix("spawn_open:") {
        (false, PathBuf::from(path))
    } else {
        return false;
    };
    let Ok(exe) = std::env::current_exe() else {
        fail(ui, "找不到自己的程序文件");
        return true;
    };
    let mut command = std::process::Command::new(exe);
    command.arg(&path);
    if let Some(settings) = ui.core.settings_path() {
        command.arg("--settings").arg(settings);
    }
    let child_dir = ui
        .selftest
        .borrow()
        .as_ref()
        .map(|r| r.plan.dir.join(format!("child-{}", r.records.len())));
    if separate && let Some(dir) = &child_dir {
        command.arg("--self-test").arg(dir);
    }
    if let Some(dir) = &child_dir {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(log) = std::fs::File::create(dir.join("app.log")) {
            command.stdout(log);
        }
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            fail(ui, &format!("启动另一个 anyview 失败：{e}"));
            return true;
        }
    };
    let started = Instant::now();
    if !separate {
        if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
            runner.expect_forward = true;
            runner.children.push((id.to_string(), child, started));
        }
        return true;
    }
    // 自己开窗口的那个：等它做完自测退出（最多 30 秒），再核对这个窗口没变。
    let child = std::cell::RefCell::new(Some(child));
    let weak = Rc::downgrade(ui);
    let before = ui.current_name();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let Some(ui) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let mut slot = child.borrow_mut();
        let Some(running) = slot.as_mut() else {
            return glib::ControlFlow::Break;
        };
        let status = running.try_wait().ok().flatten();
        if status.is_none() && started.elapsed() < Duration::from_secs(30) {
            return glib::ControlFlow::Continue;
        }
        if status.is_none() {
            let _ = running.kill();
        }
        *slot = None;
        let code = status.and_then(|s| s.code());
        let child_ok = child_dir
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join("selftest.json")).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|r| r["ok"].as_bool())
            .unwrap_or(false);
        record_step(
            &ui,
            serde_json::json!({
                "childExit": code,
                "childSelfTestOk": child_ok,
                "childMs": started.elapsed().as_secs_f64() * 1000.0,
            }),
        );
        if code != Some(0) || !child_ok {
            fail(&ui, &format!("另一个 anyview 没有正常运行（退出码 {code:?}）"));
        } else if ui.current_name() != before {
            fail(&ui, "另一个 anyview 自己开窗口时，这个窗口的图片却变了");
        } else {
            capture_later(&ui);
        }
        glib::ControlFlow::Break
    });
    true
}

/// 记录一个不切图的步骤（名称、当前图片和附加数据）。
fn record_step(ui: &Ui, extra: serde_json::Value) {
    if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
        let label = step_label(runner);
        let mut record = serde_json::json!({ "step": label, "name": ui.current_name() });
        if let (Some(target), Some(extra)) = (record.as_object_mut(), extra.as_object()) {
            target.extend(extra.clone());
        }
        runner.records.push(record);
    }
}

/// 选择图片窗口的步骤；不是这类步骤时返回 false。
/// `picker` 打开窗口（与按 G 相同），`picker_select:N` 选中第 N 行（与单击、方向键相同），
/// `picker_sort:动作` 在排序菜单里选一项，`picker_hide:on|off` 勾选"隐藏缩略图导航"，
/// `picker_ok` 点"确定"（等新图片显示），`picker_cancel` 关窗口（不切换）。
/// 窗口开着时每一步等看得见的缩略图都生成好（最多 5 秒）再截图。
fn picker_step(ui: &Rc<Ui>, id: &str) -> bool {
    let picker = || ui.picker.borrow().clone();
    if id == "picker" {
        ui.invoke("select_image");
        if picker().is_none() {
            fail(ui, "选择图片窗口没有打开");
        } else {
            wait_picker(ui, Instant::now());
        }
        return true;
    }
    if !id.starts_with("picker_") {
        return false;
    }
    let Some(p) = picker() else {
        fail(ui, &format!("{id}：选择图片窗口没有打开"));
        return true;
    };
    if let Some(n) = id.strip_prefix("picker_select:") {
        match n.parse::<usize>() {
            Ok(n) if n < p.row_count() => {
                p.select(n);
                wait_picker(ui, Instant::now());
            }
            _ => fail(ui, &format!("步骤写错了：{id}")),
        }
    } else if let Some(sort) = id.strip_prefix("picker_sort:") {
        crate::picker::activate_sort(&p, sort);
        // 核心排序后发 Collection 事件，列表在主循环里重新载入。
        let weak = Rc::downgrade(ui);
        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            if let Some(ui) = weak.upgrade() {
                wait_picker(&ui, Instant::now());
            }
        });
    } else if let Some(value) = id.strip_prefix("picker_hide:") {
        p.set_hide_thumbnails(value == "on");
        wait_picker(ui, Instant::now());
    } else if id == "picker_ok" {
        if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
            runner.waiting_image = true;
        }
        p.choose();
    } else if id == "picker_cancel" {
        p.window.close();
        record_step(ui, serde_json::json!({}));
        capture_later(ui);
    } else {
        fail(ui, &format!("步骤写错了：{id}"));
    }
    true
}

/// 直接改设置文件里的一项（键形如 browse.background_color，值是 TOML 写法如 "#336699"），
/// 然后等核心从文件里重新读到它（多窗口共享设置）。
fn edit_settings_file(ui: &Rc<Ui>, assignment: &str) {
    let Some((key, value)) = assignment.split_once('=') else {
        fail(ui, &format!("步骤写错了：{assignment}"));
        return;
    };
    let Some(path) = ui.core.settings_path() else {
        fail(ui, "没有设置文件");
        return;
    };
    let before = ui.core.setting(key);
    let edited = (|| -> Result<(), String> {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let (section, field) = key.split_once('.').ok_or("键要写成 页.项")?;
        let parsed: toml::Table = format!("v = {value}").parse().map_err(|e: toml::de::Error| e.to_string())?;
        let value = parsed.get("v").cloned().ok_or("值写错了")?;
        let entry = table
            .entry(section.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        entry
            .as_table_mut()
            .ok_or("不是表")?
            .insert(field.to_string(), value);
        std::fs::write(&path, toml::to_string(&table).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    })();
    if let Err(e) = edited {
        fail(ui, &format!("改设置文件失败：{e}"));
        return;
    }
    wait_setting_change(ui, key.to_string(), before, Instant::now());
}

fn wait_setting_change(
    ui: &Rc<Ui>,
    key: String,
    before: Option<anyview_core::settings::Value>,
    started: Instant,
) {
    if ui.core.setting(&key) != before || started.elapsed() > Duration::from_secs(5) {
        record_step(
            ui,
            serde_json::json!({
                "reloadedMs": started.elapsed().as_millis() as u64,
                "reloaded": ui.core.setting(&key) != before,
                "uiSettings": ui_settings_json(ui),
            }),
        );
        capture_later(ui);
        return;
    }
    let weak = Rc::downgrade(ui);
    glib::timeout_add_local_once(Duration::from_millis(100), move || {
        if let Some(ui) = weak.upgrade() {
            wait_setting_change(&ui, key, before, started);
        }
    });
}

/// 等选择图片窗口里看得见的缩略图都生成好（最多 5 秒），再记录、截图。
fn wait_picker(ui: &Rc<Ui>, started: Instant) {
    let done = ui.picker.borrow().as_ref().is_none_or(|p| {
        let (ready, visible) = p.thumbnails_loaded();
        !p.strip_visible() || (visible > 0 && ready == visible)
    });
    if done || started.elapsed() > Duration::from_secs(5) {
        record_step(
            ui,
            serde_json::json!({ "pickerWaitMs": started.elapsed().as_millis() as u64 }),
        );
        capture_later(ui);
        return;
    }
    let weak = Rc::downgrade(ui);
    glib::timeout_add_local_once(Duration::from_millis(50), move || {
        if let Some(ui) = weak.upgrade() {
            wait_picker(&ui, started);
        }
    });
}

fn picker_json(ui: &Ui, p: &crate::picker::Picker) -> serde_json::Value {
    let selected = p.selected();
    let (ready, visible) = p.thumbnails_loaded();
    serde_json::json!({
        "rows": p.row_count(),
        "selected": selected,
        "selectedName": selected.and_then(|i| p.row(i)).map(|r| r.name),
        "firstName": p.row(0).map(|r| r.name),
        "firstSize": p.row(0).map(|r| r.size),
        "firstModified": p.row(0).map(|r| r.modified),
        "sortLabel": p.sort_label(),
        "stripVisible": p.strip_visible(),
        "hideSetting": ui.core.picker_hides_thumbnails(),
        "thumbnailsReady": ready,
        "thumbnailsVisible": visible,
    })
}

/// 设置和输入相关的步骤；不是这类步骤时返回 false。
fn settings_step(ui: &Rc<Ui>, id: &str) -> bool {
    let window = || ui.settings_window.borrow().clone();
    if id == "settings_open" {
        let outcome = ui.invoke("settings");
        if outcome != ActionOutcome::Ui(UiRequest::ShowSettings) {
            fail(ui, &format!("设置没有打开：{outcome:?}"));
            return true;
        }
        let pages: Vec<String> = ui
            .core
            .settings_pages(ui.lang())
            .iter()
            .map(|(id, title)| format!("{id} {title}"))
            .collect();
        record_step(
            ui,
            serde_json::json!({
                "settingsPages": pages,
                "uiSettings": ui_settings_json(ui),
                "lang": format!("{:?}", ui.lang()),
            }),
        );
    } else if let Some(page) = id.strip_prefix("settings_page:") {
        let Some(w) = window() else {
            fail(ui, "设置窗口没有打开");
            return true;
        };
        w.select_page(page);
        if w.current_page() != page {
            fail(ui, &format!("没有这一页：{page}"));
            return true;
        }
        record_step(ui, serde_json::json!({}));
    } else if let Some(spec) = id.strip_prefix("settings_button:") {
        // settings_button:键:按钮：按设置项里的按钮（如书签页的"删除"，；自测时不弹确认框）。
        let Some((key, button)) = spec.rsplit_once(':') else {
            fail(ui, &format!("步骤写错了：{id}"));
            return true;
        };
        let Some(w) = window() else {
            fail(ui, "设置窗口没有打开");
            return true;
        };
        w.press_button(key, button);
        let mut extra = serde_json::json!({ "archivePositions": ui.core.archive_positions_count() });
        // 格式关联页：记下结果的说明、几种类型现在的默认程序、桌面文件和记录（都在报告目录里）。
        if key == "formats.buttons_linux" {
            extra["report"] = serde_json::json!(w.take_report());
            if let Some(at) = crate::default_viewer::Locations::current() {
                extra["defaultViewer"] = crate::default_viewer::snapshot(&at);
            }
        }
        // Flatpak 版的文件管理器缩略图：记下结果的说明和登记的文件现在的样子。
        if key == anyview_core::settings::FORMAT_BUTTONS_FLATPAK_THUMBNAILS {
            extra["report"] = serde_json::json!(w.take_report());
            extra["thumbnailer"] = crate::flatpak_thumbnailer::snapshot();
        }
        record_step(ui, extra);
    } else if let Some(assignment) = id.strip_prefix("settings_set:") {
        let (Some(w), Some((key, value))) = (window(), assignment.split_once('=')) else {
            fail(ui, &format!("设置窗口没有打开，或步骤写错了：{id}"));
            return true;
        };
        if !w.set_for_test(key, value) {
            fail(ui, &format!("这一页没有 {key}，或它不能改成 {value}"));
            return true;
        }
        record_step(ui, serde_json::json!({ "dirty": ui.core.settings_dirty() }));
    } else if id == "settings_ok" || id == "settings_ok_invalid" {
        let Some(w) = window() else {
            fail(ui, "设置窗口没有打开");
            return true;
        };
        ui.settings_error.borrow_mut().take();
        let applied = w.ok();
        let error = ui.settings_error.borrow().clone();
        let expect_ok = id == "settings_ok";
        if applied != expect_ok {
            fail(ui, &format!("点\"确定\"的结果不对：生效 {applied}，提示 {error:?}"));
            return true;
        }
        let changed = ui
            .selftest
            .borrow_mut()
            .as_mut()
            .and_then(|r| r.applied.take());
        let file = ui.core.settings_path();
        let text = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok());
        record_step(
            ui,
            serde_json::json!({
                "applied": applied,
                "error": error,
                "errorPage": if applied { None } else { Some(w.current_page()) },
                "changed": changed.as_ref().map(|c| c.changed.clone()),
                "saveError": changed.and_then(|c| c.save_error),
                "uiSettings": ui_settings_json(ui),
                "lang": format!("{:?}", ui.lang()),
                "settingsFile": file.map(|f| f.display().to_string()),
                "settingsText": if applied { text } else { None },
            }),
        );
    } else if id == "settings_reset" {
        let Some(w) = window() else {
            fail(ui, "设置窗口没有打开");
            return true;
        };
        w.reset();
        record_step(ui, serde_json::json!({ "dirty": ui.core.settings_dirty() }));
    } else if id == "settings_cancel" {
        let Some(w) = window() else {
            fail(ui, "设置窗口没有打开");
            return true;
        };
        let dirty = ui.core.settings_dirty();
        w.discard();
        record_step(
            ui,
            serde_json::json!({ "dirtyBeforeCancel": dirty, "open": ui.settings_window.borrow().is_some() }),
        );
    } else if let Some(spec) = id.strip_prefix("key:") {
        // 与主窗口的按键处理相同：交给核心解析成动作再执行（键盘映射优先；Esc 退出受设置"按 ESC 退出"控制）。
        // 写法同动作目录，如 `key:Ctrl+K`。
        // 步骤用逗号分隔：逗号、句号键写成 Comma、Period。
        let spec = match spec {
            "Comma" => ",",
            "Period" => ".",
            other => other,
        };
        let Some(chord) = anyview_core::actions::Chord::parse(spec) else {
            fail(ui, &format!("按键写错了：{spec}"));
            return true;
        };
        let action = ui.core.action_for_key(&chord.key, chord.mods);
        let extra = serde_json::json!({ "key": spec, "action": action });
        match action {
            Some(action) => invoke_step(ui, action, extra),
            None => {
                record_step(ui, extra);
                capture_later(ui);
            }
        }
        return true;
    } else if let Some(input) = id.strip_prefix("mouse:") {
        // 鼠标：与真的点击走同样的规则（设置"鼠标"页）。`mouse:left:0.9` 是在画面九成宽处单击左键，
        // `mouse:double:0.5` 是双击；其余为 middle、right、back、forward、wheel_up、wheel_down。
        let (name, fraction) = input.split_once(':').unwrap_or((input, "0.5"));
        let fraction: f64 = fraction.parse().unwrap_or(0.5);
        let target = match name {
            "left" => ui.core.left_click_action(fraction).map(MouseTarget::Action),
            "double" => ui.core.left_double_click_action(fraction).map(MouseTarget::Action),
            other => {
                let input = match other {
                    "middle" => MouseInput::Middle,
                    "right" => MouseInput::Right,
                    "back" => MouseInput::Back,
                    "forward" => MouseInput::Forward,
                    "wheel_up" => MouseInput::WheelUp,
                    "wheel_down" => MouseInput::WheelDown,
                    _ => {
                        fail(ui, &format!("鼠标步骤写错了：{input}"));
                        return true;
                    }
                };
                Some(ui.mouse_target(input))
            }
        };
        let action = match target {
            Some(MouseTarget::Action(id)) => Some(id),
            Some(MouseTarget::ContextMenu) => Some("context_menu"),
            _ => None,
        };
        let extra = serde_json::json!({ "mouse": input, "action": action });
        match target {
            // 右键菜单是单独的弹出层，这里只记录要弹出菜单，不真的弹出。
            Some(MouseTarget::Action(id)) => invoke_step(ui, id, extra),
            _ => {
                record_step(ui, extra);
                capture_later(ui);
            }
        }
        return true;
    } else if id == "hover_bottom" {
        let (w, h) = (f64::from(ui.window.width()), f64::from(ui.window.height()));
        ui.overlays.pointer_moved(ui, w / 2.0, h - 20.0, w, h);
        if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
            runner.hover_bar = true;
        }
        record_step(ui, serde_json::json!({}));
    } else {
        return false;
    }
    capture_later(ui);
    true
}

/// 设置窗口一页的内容摘要：每项的键、文字、是否可用和值。
fn settings_page_json(ui: &Ui, page: &str) -> serde_json::Value {
    let Some(view) = ui.core.settings_page(page, ui.lang()) else {
        return serde_json::Value::Null;
    };
    let items: Vec<serde_json::Value> = view
        .items
        .iter()
        .map(|item| {
            let value = match &item.control {
                Control::Toggle(on) => serde_json::json!(on),
                Control::Choice { selected, .. } => serde_json::json!(selected),
                Control::Number { text, .. } | Control::Path { text, .. } => serde_json::json!(text),
                Control::Color { value, .. } => {
                    serde_json::json!(anyview_core::settings::color_text(*value))
                }
                Control::Mapping { selected, .. } => serde_json::json!(selected),
                // 快捷键：列出来的行（用过的）写成"on/off 按键 动作 提示"。
                Control::KeyMappings { rows, .. } => serde_json::json!(
                    rows.iter()
                        .map(|r| format!(
                            "{} {} {} {}",
                            if r.enabled { "on" } else { "off" },
                            r.key_label,
                            r.action,
                            r.note
                        )
                        .trim_end()
                        .to_string())
                        .collect::<Vec<_>>()
                ),
                _ => serde_json::Value::Null,
            };
            serde_json::json!({
                "key": item.key,
                "label": item.label,
                "section": item.section,
                "enabled": item.enabled,
                "value": value,
            })
        })
        .collect();
    serde_json::json!({ "id": view.id, "title": view.title, "sections": view.sections, "items": items })
}

fn ui_settings_json(ui: &Ui) -> serde_json::Value {
    let s = ui.core.ui_settings();
    serde_json::json!({
        "backgroundColor": anyview_core::settings::color_text(s.background_color),
        "alphaBackground": format!("{:?}", s.alpha_background),
        "hideControlBar": s.hide_control_bar,
        "hideSideArrows": s.hide_side_arrows,
        "hideZoomInfo": s.hide_zoom_info,
        "hideTooltips": s.hide_tooltips,
        "smallInfoFont": s.small_info_font,
        "cornerNotices": s.corner_notices,
        "doubleClickMaximize": s.double_click_maximize,
        "noRecent": s.no_recent,
        "windowFollowsImage": ui.core.window_follows_image(),
    })
}

/// 本进程启动至今的毫秒数（/proc/self/stat 的 starttime，按每秒 100 个时钟滴答计）。
fn process_age_ms() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let start_ticks: f64 = fields.get(19)?.parse().ok()?;
    let uptime: f64 = std::fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some((uptime - start_ticks / 100.0) * 1000.0)
}

/// 本进程用掉的 CPU 时间（用户 + 系统，秒）。
fn cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let user: f64 = fields.get(11)?.parse().ok()?;
    let system: f64 = fields.get(12)?.parse().ok()?;
    Some((user + system) / 100.0)
}

/// 内存峰值（VmHWM，MB）。
fn peak_rss_mb() -> Option<f64> {
    status_mb("VmHWM:")
}

/// 现在的内存占用（VmRSS，MB）。
fn rss_mb() -> Option<f64> {
    status_mb("VmRSS:")
}

fn status_mb(field: &str) -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with(field))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

/// 等精确重绘盖上画面：每 50 ms 看一次，最多 3 秒。
fn wait_exact(ui: &Rc<Ui>, started: Instant) {
    if let Some((x, y, w, h, render_ms)) = ui.canvas.exact_shown() {
        if let Some(runner) = ui.selftest.borrow_mut().as_mut() {
            let label = step_label(runner);
            runner.records.push(serde_json::json!({
                "step": label,
                "name": ui.current_name(),
                "exact": { "x": x, "y": y, "width": w, "height": h, "renderMs": render_ms },
            }));
        }
        capture_later(ui);
        return;
    }
    if started.elapsed() > Duration::from_secs(3) {
        fail(ui, "精确重绘 3 秒内没有出现");
        return;
    }
    let weak = Rc::downgrade(ui);
    glib::timeout_add_local_once(Duration::from_millis(50), move || {
        if let Some(ui) = weak.upgrade() {
            wait_exact(&ui, started);
        }
    });
}

/// 到达首尾时的询问。自测期间返回 true：不弹对话框，记录这一步，下一步充当用户的选择。
pub fn on_boundary(ui: &Rc<Ui>, forward: bool) -> bool {
    {
        let mut guard = ui.selftest.borrow_mut();
        let Some(runner) = guard.as_mut() else {
            return false;
        };
        if runner.finished || !runner.waiting_image {
            return true;
        }
        runner.waiting_image = false;
        let label = step_label(runner);
        runner.records.push(serde_json::json!({
            "step": label,
            "name": ui.current_name(),
            "boundary": if forward { "last" } else { "first" },
        }));
    }
    capture_later(ui);
    true
}

/// 图片被安全上限拦下时的询问。自测期间返回 true：不弹对话框，记录这一步（"blocked"），
/// 下一步 `limit_open`、`limit_cancel` 充当用户的选择。
pub fn on_limit(ui: &Rc<Ui>) -> bool {
    {
        let mut guard = ui.selftest.borrow_mut();
        let Some(runner) = guard.as_mut() else {
            return false;
        };
        if runner.finished || !runner.waiting_image {
            return true;
        }
        runner.waiting_image = false;
        let label = step_label(runner);
        runner.records.push(serde_json::json!({
            "step": label,
            "name": ui.current_name(),
            "blocked": true,
        }));
    }
    // 这时画面上没有图片（等用户回答），不截图核对，直接做下一步。
    let weak = Rc::downgrade(ui);
    glib::idle_add_local_once(move || {
        if let Some(ui) = weak.upgrade() {
            advance(&ui);
        }
    });
    true
}

/// 悬浮界面的状态，以及与核心状态不一致的地方。
fn overlay_state(ui: &Ui) -> (serde_json::Value, Vec<String>) {
    let o = &ui.overlays;
    let locked = ui.core.action_state("toggle_bar_lock").checked;
    let info_on = ui.core.action_state("toggle_info").checked;
    let lines = ui.core.info_lines(ui.lang());
    let info_visible = o.info.is_visible();
    let settings = ui.ui_settings.get();
    let hovering = ui.selftest.borrow().as_ref().is_some_and(|r| r.hover_bar);
    let mut problems = Vec::new();
    // 无头环境没有指针，控制栏只在锁定时（或模拟指针移到底部后）显示；刚解锁时按规则等 1 秒再淡出。
    // 设置"不显示控制栏"时任何时候都不显示。
    let wanted = !settings.hide_control_bar && (locked || hovering);
    let bar_ok = o.bar_fade.is_shown() == wanted || (!wanted && o.bar_hide_pending());
    if !bar_ok {
        problems.push(format!(
            "控制栏显示={}，锁定={locked}，指针在底部={hovering}，设置不显示={}",
            o.bar_fade.is_shown(),
            settings.hide_control_bar
        ));
    }
    if info_visible != (info_on && !lines.is_empty()) {
        problems.push(format!("图片信息显示={info_visible}，开关={info_on}"));
    }
    if info_visible && o.info_label.text() != lines.join("\n") {
        problems.push("图片信息的文字与核心不同".to_string());
    }
    let expected_knob = match ui.current_position() {
        (Some(index), Some(total)) => Some(progress_fraction(index, total)),
        _ => None,
    };
    if o.knob_fraction() != expected_knob {
        problems.push(format!(
            "进度条滑块 {:?}，应为 {expected_knob:?}",
            o.knob_fraction()
        ));
    }
    let state = serde_json::json!({
        "barHidden": settings.hide_control_bar,
        "barShown": o.bar_fade.is_shown(),
        "barHidePending": o.bar_hide_pending(),
        "barLocked": locked,
        "knob": o.knob_fraction(),
        "infoShown": info_visible,
        "infoText": if info_visible { lines } else { Vec::new() },
    });
    (state, problems)
}

fn layout_json(l: Layout) -> serde_json::Value {
    serde_json::json!({
        "centerX": l.center_x,
        "centerY": l.center_y,
        "width": l.width,
        "height": l.height,
        "scale": l.scale,
        "rotation": l.rotation,
        "flipHorizontal": l.flip_horizontal,
        "flipVertical": l.flip_vertical,
        "smooth": l.smooth,
        "zoom": l.zoom,
    })
}

/// 画面核对：图片上 4×4 个点（各自位于一个色块的中部）按核心的映射找到屏幕位置，比较颜色。
/// 翻页效果停在一半时的画面核对：两张图都是纯色（effects/），按效果看几个点的颜色。
/// 淡出：中间是两种颜色各半；滑动：往后翻左边是旧图、右边是新图（往前翻相反）；
/// 翻页：往后翻旧的一页以左边为轴翻起一半，右边露出新图、左边是变暗的旧图（往前翻新旧对调）。
fn transition_check(
    ui: &Ui,
    shot: &Screenshot,
    (effect, forward, progress): (anyview_core::transition::PageEffect, bool, f64),
) -> serde_json::Value {
    use anyview_core::transition::PageEffect;
    let (old, new) = {
        let guard = ui.selftest.borrow();
        let runner = guard.as_ref();
        (
            runner.and_then(|r| r.previous_color),
            runner.and_then(|r| r.current_color),
        )
    };
    let (Some(old), Some(new)) = (old, new) else {
        return serde_json::json!({ "checked": 0, "error": "不知道新旧两张图的颜色" });
    };
    // 采样点按图片在屏幕上的范围取（窗口可能比图片大，四周是背景色）。
    let Some(layout) = ui.core.view_layout() else {
        return serde_json::json!({ "checked": 0, "error": "没有画面参数" });
    };
    let left = layout.center_x - layout.width / 2.0;
    let y = layout.center_y.max(0.0) as usize;
    let at = |fx: f64| shot.rgb((left + layout.width * fx).max(0.0) as usize, y);
    let mix = |a: [u8; 3], b: [u8; 3], t: f64| -> [u8; 3] {
        std::array::from_fn(|i| (f64::from(a[i]) * (1.0 - t) + f64::from(b[i]) * t).round() as u8)
    };
    let dark = |c: [u8; 3]| {
        mix(c, [0, 0, 0], 0.35 * anyview_core::transition::ease(progress))
    };
    let (first, second) = if forward { (old, new) } else { (new, old) };
    let expected: Vec<(f64, [u8; 3])> = match effect {
        PageEffect::Fade => vec![(0.5, mix(old, new, anyview_core::transition::ease(progress)))],
        PageEffect::Slide => vec![(0.25, first), (0.75, second)],
        PageEffect::Page if forward => vec![(0.03, dark(old)), (0.95, new)],
        PageEffect::Page => vec![(0.03, dark(new)), (0.95, old)],
        PageEffect::None => Vec::new(),
    };
    let mut mismatches = Vec::new();
    let mut points = Vec::new();
    for (fx, want) in &expected {
        let got = at(*fx);
        let ok = got.is_some_and(|g| g.iter().zip(want).all(|(a, b)| (i32::from(*a) - i32::from(*b)).abs() <= 30));
        points.push(serde_json::json!({ "x": fx, "got": got.map(hex), "want": hex(*want) }));
        if !ok {
            mismatches.push(serde_json::json!({ "x": fx, "got": got.map(hex), "want": hex(*want) }));
        }
    }
    serde_json::json!({
        "checked": expected.len(),
        "mismatches": mismatches,
        "transition": { "effect": effect.id(), "forward": forward, "progress": progress, "points": points },
    })
}

fn check_samples(ui: &Ui, shot: &Screenshot) -> serde_json::Value {
    let Some(image) = ui.canvas.image() else {
        // 空态（关闭了图片）：没有要核对的画面。
        return if ui.core.current_path().is_none() {
            serde_json::json!({ "checked": 0, "skipped": "空态" })
        } else {
            serde_json::json!({ "checked": 0, "error": "没有图片" })
        };
    };
    let sample_at_100 = ui.selftest.borrow().as_ref().is_some_and(|r| r.plan.sample_at_100);
    if sample_at_100 && ui.core.view_layout().is_some_and(|l| (l.zoom - 1.0).abs() > 1e-9) {
        return serde_json::json!({ "checked": 0, "skipped": "只在 100% 时核对" });
    }
    // 放大显示（物理倍率大于 1，如高分屏上的 100%）时，屏幕像素的颜色由相邻的图片像素插值而来。
    let magnified = ui.core.view_layout().is_some_and(|l| l.scale > 1.0 + 1e-6);
    let scale = ui.canvas.physical_scale();
    let origin = ui
        .canvas
        .compute_point(&ui.window, &graphene::Point::new(0.0, 0.0))
        .unwrap_or_else(|| graphene::Point::new(0.0, 0.0));
    let texture_scale = f64::from(shot.width) / f64::from(ui.window.width().max(1));
    let (cw, ch) = (f64::from(ui.canvas.width()), f64::from(ui.canvas.height()));
    let covered = ui.covered_rects();
    // 图片中心在截图里的颜色（色彩管理的核对用）。
    let center = ui
        .core
        .view_image_to_screen(f64::from(image.width) / 2.0, f64::from(image.height) / 2.0)
        .and_then(|(x, y)| {
            let px = ((f64::from(origin.x()) + x / scale) * texture_scale) as usize;
            let py = ((f64::from(origin.y()) + y / scale) * texture_scale) as usize;
            shot.rgb(px, py).map(hex)
        });
    let mut checked = 0;
    let mut mismatches = Vec::new();
    // 采样点：先取图片上 4×4 个点（各在一个色块中部）；它们都不在窗口里时（如超大图的 100%），
    // 改在可见区域里均匀取 4×4 个点，再换回图片坐标。都取像素中心，避免落在像素边界上。
    let fractions = [0.125, 0.375, 0.625, 0.875];
    let (iw, ih) = (f64::from(image.width), f64::from(image.height));
    let center_of = |t: f64| t.floor() + 0.5;
    let mut points: Vec<(f64, f64)> = fractions
        .iter()
        .flat_map(|fy| fractions.iter().map(move |fx| (center_of(fx * iw), center_of(fy * ih))))
        .collect();
    let visible = |u: f64, v: f64| {
        ui.core
            .view_image_to_screen(u, v)
            .is_some_and(|(x, y)| x / scale >= 4.0 && y / scale >= 4.0 && x / scale <= cw - 4.0 && y / scale <= ch - 4.0)
    };
    if !points.iter().any(|&(u, v)| visible(u, v)) {
        let corners = [(0.0, 0.0), (iw, 0.0), (0.0, ih), (iw, ih)];
        let screen: Vec<(f64, f64)> = corners
            .iter()
            .filter_map(|&(u, v)| ui.core.view_image_to_screen(u, v))
            .collect();
        let (vw, vh) = (cw * scale, ch * scale);
        let left = screen.iter().map(|p| p.0).fold(f64::MAX, f64::min).max(0.0);
        let right = screen.iter().map(|p| p.0).fold(f64::MIN, f64::max).min(vw);
        let top = screen.iter().map(|p| p.1).fold(f64::MAX, f64::min).max(0.0);
        let bottom = screen.iter().map(|p| p.1).fold(f64::MIN, f64::max).min(vh);
        points = fractions
            .iter()
            .flat_map(|fy| fractions.iter().map(move |fx| (left + fx * (right - left), top + fy * (bottom - top))))
            .filter_map(|(x, y)| ui.core.view_screen_to_image(x, y))
            .filter(|&(u, v)| u >= 0.0 && v >= 0.0 && u < iw && v < ih)
            .map(|(u, v)| (center_of(u), center_of(v)))
            .collect();
    }
    for (u, v) in points {
        {
            let Some((x, y)) = ui.core.view_image_to_screen(u, v) else {
                continue;
            };
            let (lx, ly) = (x / scale, y / scale);
            if lx < 4.0 || ly < 4.0 || lx > cw - 4.0 || ly > ch - 4.0 {
                continue;
            }
            let (wx, wy) = (f64::from(origin.x()) + lx, f64::from(origin.y()) + ly);
            if covered
                .iter()
                .any(|r| r.contains_point(&graphene::Point::new(wx as f32, wy as f32)))
            {
                continue;
            }
            let px = (wx * texture_scale) as usize;
            let py = (wy * texture_scale) as usize;
            let Some(got) = shot.rgb(px, py) else {
                continue;
            };
            // 原图上应有的颜色范围：缩小或 1:1 时就是这个点的颜色；放大时是截图像素中心附近 2×2 个图片像素的
            // 颜色范围（插值的结果落在它们之间）。不透明处与原图比；全透明处应为棋盘格的两种颜色之一；半透明处跳过。
            let around = if magnified {
                let sx = ((px as f64 + 0.5) / texture_scale - f64::from(origin.x())) * scale;
                let sy = ((py as f64 + 0.5) / texture_scale - f64::from(origin.y())) * scale;
                let Some((iu, iv)) = ui.core.view_screen_to_image(sx, sy) else {
                    continue;
                };
                let clamp = |t: f64, size: u32| (t.floor() as i64).clamp(0, i64::from(size) - 1) as u32;
                let (i0, j0) = (clamp(iu - 0.5, image.width), clamp(iv - 0.5, image.height));
                let (i1, j1) = ((i0 + 1).min(image.width - 1), (j0 + 1).min(image.height - 1));
                [(i0, j0), (i1, j0), (i0, j1), (i1, j1)]
                    .iter()
                    .filter_map(|&(i, j)| image_pixel(&image, i, j))
                    .collect::<Vec<_>>()
            } else {
                image_pixel(&image, u as u32, v as u32).into_iter().collect()
            };
            if around.is_empty() {
                continue;
            }
            let ranges: Vec<([u8; 3], [u8; 3])> = if around.iter().all(|&(_, a)| a == 255) {
                let mut low = [255u8; 3];
                let mut high = [0u8; 3];
                for (color, _) in &around {
                    for i in 0..3 {
                        low[i] = low[i].min(color[i]);
                        high[i] = high[i].max(color[i]);
                    }
                }
                vec![(low, high)]
            } else if around.iter().all(|&(_, a)| a == 0) {
                // 透明处的背景：棋盘格的两种颜色之一，或设置的纯色（GIF 另有设置）。
                match ui.core.alpha_fill(&image) {
                    AlphaFill::Checker => CHECKER_COLORS.iter().map(|&c| (c, c)).collect(),
                    AlphaFill::Solid(rgb) => {
                        let c = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
                        vec![(c, c)]
                    }
                }
            } else {
                continue;
            };
            let expected = ranges[0].0;
            checked += 1;
            let diff = ranges
                .iter()
                .map(|(low, high)| {
                    (0..3)
                        .map(|i| {
                            let g = i32::from(got[i]);
                            (i32::from(low[i]) - g).max(g - i32::from(high[i])).max(0)
                        })
                        .max()
                        .unwrap_or(0)
                })
                .min()
                .unwrap_or(0);
            if diff > COLOR_TOLERANCE {
                mismatches.push(serde_json::json!({
                    "image": [u, v],
                    "screen": [px, py],
                    "expected": hex(expected),
                    "got": hex(got),
                }));
            }
        }
    }
    serde_json::json!({ "checked": checked, "mismatches": mismatches, "centerColor": center })
}

/// 原图在 (u, v) 处的颜色（预乘前的近似值无关紧要：只用不透明和全透明两种点）和 Alpha。
fn image_pixel(image: &DecodedImage, u: u32, v: u32) -> Option<([u8; 3], u8)> {
    let i = v as usize * image.stride + u as usize * 4;
    match image.pixels.get(i..i + 4) {
        Some(&[b, g, r, a]) => Some(([r, g, b], a)),
        _ => None,
    }
}

fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// 图像正中那个像素（BGRA 预乘，与缓冲区相同）。
fn image_center(image: &DecodedImage) -> [u8; 4] {
    let i = (image.height / 2) as usize * image.stride + (image.width / 2) as usize * 4;
    image.pixels.get(i..i + 4).map_or([0; 4], |p| [p[0], p[1], p[2], p[3]])
}

/// 纹理正中那个像素（下载成 GDK 的默认格式，小端机器上是 BGRA 预乘）。
fn texture_center(texture: &gtk::gdk::Texture) -> [u8; 4] {
    let (width, height) = (texture.width() as usize, texture.height() as usize);
    let stride = width * 4;
    let mut pixels = vec![0u8; stride * height];
    texture.download(&mut pixels, stride);
    let i = height / 2 * stride + width / 2 * 4;
    pixels.get(i..i + 4).map_or([0; 4], |p| [p[0], p[1], p[2], p[3]])
}

/// 界面显示了一条提示：记下来（`lastNotice`）。壁纸门户、“打开方式”等的结果过一会儿才回来，
/// 提示只显示 2 秒，后面的步骤截图时可能已经消失。
pub fn note_notice(ui: &Ui, text: &str) {
    // 自测正在改自己的记录时（借用中）不记，免得出错；那时的提示由自测自己记下。
    if let Ok(mut guard) = ui.selftest.try_borrow_mut()
        && let Some(runner) = guard.as_mut()
    {
        runner.last_notice = Some(text.to_string());
    }
}

pub fn on_failure(ui: &Rc<Ui>, message: &str) {
    let active = ui
        .selftest
        .borrow()
        .as_ref()
        .is_some_and(|r| !r.finished && r.open.is_some());
    if active {
        fail(ui, message);
    }
}

fn fail(ui: &Rc<Ui>, message: &str) {
    if let Some(runner) = ui.selftest.borrow_mut().as_mut()
        && runner.failure.is_none()
    {
        runner.failure = Some(message.to_string());
    }
    finish(ui);
}

fn dir_of(ui: &Ui) -> PathBuf {
    ui.selftest
        .borrow()
        .as_ref()
        .map(|r| r.plan.dir.clone())
        .unwrap_or_default()
}

/// 右键菜单的数据摘要：每项一行"标题  快捷键"，子菜单缩进。
fn context_menu_lines(ui: &Ui) -> Vec<String> {
    let mut depth = 0;
    let mut lines = Vec::new();
    for entry in anyview_core::actions::menu(MenuKind::Context, Platform::Linux, ui.lang()) {
        let indent = "  ".repeat(depth);
        match entry {
            MenuEntry::Action {
                title,
                shortcut,
                id,
                ..
            } => {
                let state = ui.core.action_state(id);
                let accel = shortcut
                    .map(|c| c.label(Platform::Linux))
                    .unwrap_or_default();
                let mark = if state.checked { "✓ " } else { "" };
                let grey = if state.enabled { "" } else { "（灰显）" };
                lines.push(
                    format!("{indent}{mark}{title}  {accel}{grey}")
                        .trim_end()
                        .to_string(),
                );
            }
            MenuEntry::Separator => lines.push(format!("{indent}——")),
            MenuEntry::SubmenuStart { title, .. } => {
                lines.push(format!("{indent}{title} ›"));
                depth += 1;
            }
            MenuEntry::SubmenuEnd => depth -= 1,
        }
    }
    lines
}

fn finish(ui: &Rc<Ui>) {
    let (dir, hold_ms) = {
        let mut guard = ui.selftest.borrow_mut();
        let Some(runner) = guard.as_mut() else { return };
        if runner.finished {
            return;
        }
        runner.finished = true;
        (runner.plan.dir.clone(), runner.plan.hold_ms)
    };

    let window = &ui.window;
    let mut report = serde_json::json!({
        "platform": "linux",
        "coreVersion": anyview_core::version(),
        "emptyStateHint": anyview_core::empty_state_hint(Platform::Linux, ui.lang()),
        "pongReceived": ui.ping_ms.get().is_some(),
        "pingRoundTripMs": ui.ping_ms.get(),
        "windowSize": [window.width(), window.height()],
        "scaleFactor": window.scale_factor(),
        "physicalScale": ui.canvas.physical_scale(),
        "gskRenderer": window.native().and_then(|n| n.renderer()).map(|r| r.type_().name().to_string()),
        "gtkVersion": format!("{}.{}.{}", gtk::major_version(), gtk::minor_version(), gtk::micro_version()),
        "contextMenu": context_menu_lines(ui),
    });

    // 交出图片的那些进程（spawn_open）：应当很快正常退出；还没退出的结束掉，算失败。
    let children: Vec<serde_json::Value> = {
        let mut guard = ui.selftest.borrow_mut();
        let runner = guard.as_mut().expect("自测状态存在");
        runner
            .children
            .iter_mut()
            .map(|(step, child, started)| {
                let mut status = child.try_wait().ok().flatten();
                for _ in 0..20 {
                    if status.is_some() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    status = child.try_wait().ok().flatten();
                }
                if status.is_none() {
                    let _ = child.kill();
                }
                serde_json::json!({
                    "step": step,
                    "exit": status.and_then(|s| s.code()),
                    "msUntilChecked": started.elapsed().as_secs_f64() * 1000.0,
                })
            })
            .collect()
    };
    let children_ok = children.iter().all(|c| c["exit"] == serde_json::json!(0));
    report["children"] = serde_json::json!(children);

    let ok = {
        let guard = ui.selftest.borrow();
        let runner = guard.as_ref().expect("自测状态存在");
        report["elapsedMs"] = serde_json::json!(runner.started.elapsed().as_secs_f64() * 1000.0);
        if runner.open.is_none() {
            // 空态自测：截图并核对背景色。
            let shot = capture(window, &dir.join("screenshot.png"));
            let background_ok = matches!(&shot, Ok(s) if s.corner_color() == BACKGROUND_HEX);
            report["screenshot"] = match &shot {
                Ok(s) => serde_json::json!({
                    "file": "screenshot.png",
                    "width": s.width,
                    "height": s.height,
                    "cornerColor": s.corner_color(),
                    "cornerColorOK": background_ok,
                }),
                Err(e) => serde_json::json!({ "error": e }),
            };
            ui.ping_ms.get().is_some() && background_ok
        } else {
            report["open"] =
                serde_json::json!(runner.open.as_ref().map(|p| p.display().to_string()));
            report["steps"] = serde_json::json!(runner.records);
            report["failure"] = serde_json::json!(runner.failure);
            runner.failure.is_none()
                && runner.records.len() == runner.plan.steps.len() + 1
                && children_ok
        }
    };
    report["ok"] = serde_json::json!(ok);

    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    if let Err(e) = std::fs::write(dir.join("selftest.json"), format!("{text}\n")) {
        eprintln!("写入自测报告失败：{e}");
    }
    println!("{text}");
    // 自测直接结束进程：先关掉压缩包（删掉解出的临时文件）。
    ui.core.shutdown();
    // 保持窗口一段时间，让外部截图工具（grim）也能截到。
    glib::timeout_add_local_once(Duration::from_millis(hold_ms), move || {
        std::process::exit(if ok { 0 } else { 1 });
    });
}

struct Screenshot {
    width: i32,
    height: i32,
    stride: usize,
    /// 预乘的 B8G8R8A8，背景不透明时即原色。
    pixels: Vec<u8>,
}

impl Screenshot {
    fn rgb(&self, x: usize, y: usize) -> Option<[u8; 3]> {
        let i = y * self.stride + x * 4;
        match self.pixels.get(i..i + 3) {
            Some(&[b, g, r]) => Some([r, g, b]),
            _ => None,
        }
    }

    fn corner_color(&self) -> String {
        let (x, y) = SAMPLE_POINT;
        self.rgb(x, y)
            .map(hex)
            .unwrap_or_else(|| "unknown".to_string())
    }
}

/// 用窗口自己的渲染器把窗口内容画成 PNG，不依赖合成器的截图能力。
fn capture(window: &impl IsA<gtk::Widget>, path: &Path) -> Result<Screenshot, String> {
    let (width, height) = (window.width(), window.height());
    if width <= 0 || height <= 0 {
        return Err(format!("窗口尺寸无效：{width}x{height}"));
    }
    let paintable = gtk::WidgetPaintable::new(Some(window.upcast_ref::<gtk::Widget>()));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot.to_node().ok_or("窗口没有可绘制的内容")?;
    let renderer = window
        .native()
        .and_then(|n| n.renderer())
        .ok_or("窗口还没有渲染器")?;
    let texture = renderer.render_texture(&node, None);
    texture.save_to_png(path).map_err(|e| e.to_string())?;

    let stride = texture.width() as usize * 4;
    let mut pixels = vec![0u8; stride * texture.height() as usize];
    texture.download(&mut pixels, stride);
    Ok(Screenshot {
        width: texture.width(),
        height: texture.height(),
        stride,
        pixels,
    })
}
