# 右键菜单、控制栏与悬浮界面

本文说明看图窗口内的右键菜单、底部控制栏、边缘切图箭头、图片信息浮层、通知与缩放倍率提示的结构和交互。动作 ID 见 [ACTIONS.md](ACTIONS.md)，macOS 顶部菜单栏见 [PLATFORMS.md](PLATFORMS.md#菜单栏)。

## 右键菜单

在画布任意位置右键打开。菜单树由核心统一提供数据，三端按平台生成原生菜单（`NSMenu`、`MenuFlyout`、`GtkPopoverMenu`），自动同步勾选状态、启用状态与当前生效的快捷键。

### 主菜单

`——` 表示分隔线：

```text
打开图片                open_file
打开文件夹              open_folder
关闭                    close_image
选择图片                select_image
定位文件                reveal_file
——
无                      edge_none            ┐
减淡                    edge_soften          ├ 像素边缘处理（单选）
减淡+锐化               edge_soften_sharpen  ┘
——
浏览导航 ›
插值算法 ›
图片效果 ›
排序方式 ›
——
浏览方式 ›
缩放工具 ›
图像旋转 ›
——
EXIF 信息               toggle_info
生成信息                show_generation_info（当前图片包含 AI 生成参数时可用）
设为桌面壁纸…           set_wallpaper
删除                    delete_file
复制到图片文件夹        copy_to_folder1
用图像编辑器打开        open_in_editor
打印                    print
复制到剪贴板            copy_to_clipboard
——
设置                    settings
关于                    about
退出                    quit
```

### 子菜单

| 子菜单 | 包含动作 |
| --- | --- |
| 浏览导航 | `next_image`、`prev_image`、`first_image`、`last_image`、`forward_10`、`back_10`、`random_forward`、`random_back`、`next_container`、`prev_container` |
| 插值算法 | `interp_bilinear`、`interp_bicubic`、`interp_lanczos`（单选） |
| 图片效果 | `invert`、`gamma`、`flip_vertical`、`flip_horizontal`（各自开关） |
| 排序方式 | `sort_name`、`sort_mtime`、`sort_size`、`sort_type`（单选）—— `sort_desc` |
| 浏览方式 | `stretch_small` —— `zoom_original`、`fit_window`、`fit_width`（单选）、`double_page_ltr`、`double_page_rtl` |
| 缩放工具 | `zoom_in`、`zoom_out`、`zoom_lock`、`zoom_100`、`zoom_in_1`、`zoom_out_1` |
| 图像旋转 | `rotate_none`、`rotate_ccw`、`rotate_cw`、`rotate_180` |

### 启用规则

- 无打开图片时，依赖当前图片的命令自动灰显；全局查看选项、排序与设置始终可用。
- 压缩包内的图片不可删除或移动（仅支持复制）。

## 底部控制栏

鼠标移至窗口下方触发区时淡入显示，移开后自动隐藏；锁定后常驻。

```text
┌───────────────────────────────────────────────────────────┐
│ (打开) (选择)  ━━━━━━━●━━━━━━━━━━━━━━  (‹) (›)             │
│                ＋  －  │ ↶  ↷  │ 锁定 ●                    │
└───────────────────────────────────────────────────────────┘
```

| 位置 | 元素 | 对应动作 |
| --- | --- | --- |
| 左 1 | 打开按钮 | `open_file` |
| 左 2 | 选择图片按钮 | `select_image` |
| 中上 | 浏览进度条 | 拖动或点击按序号跳转（悬停显示 `序号/总数`） |
| 中下左 | 放大 / 缩小 | `zoom_in` / `zoom_out` |
| 中下中 | 逆时针 / 顺时针旋转 90° | `rotate_ccw` / `rotate_cw` |
| 中下右 | 锁定开关 | `toggle_bar_lock` |
| 右 1 | 上一张按钮 | 左键 `prev_image`，右键 `back_10` |
| 右 2 | 下一张按钮 | 左键 `next_image`，右键 `forward_10` |

- 指针触发区与显示/隐藏时序规则由核心模块 `anyview_core::overlay` 统一计算，各端负责原生外观与图标渲染（macOS 用 SF Symbols + 毛玻璃/Liquid Glass，Windows 用 Segoe Fluent Icons + 亚克力，Linux 用系统符号图标 + GTK OSD）。
- 设置开启「不显示控制栏」时彻底隐藏；开启「不显示鼠标悬浮工具提示」时隐藏按钮提示。

## 边缘切图箭头

鼠标移至窗口左侧或右侧触发区时，垂直居中显示半透明细线箭头（`‹` / `›`），点击翻页；可在设置中关闭。

## 图片信息浮层（左上角）

按 `Tab`（或 macOS `⌘I`）切换显示，位于窗口左上角，采用白字深色描边无底板样式，附带关闭与一键复制按钮：

```text
(×) (复制)
文件名: sample_photo.png [72/1862]
图片大小: 1.8MB
修改日期: 2026/09/11 09:18:06
图片信息: 1414x2048 (PNG,RGBA32)
```

图片包含相机 EXIF 时在下方追加拍摄参数；无 EXIF 时不出现空行。可在设置中切换小字体。

## 通知与缩放倍率提示

- **短提示通知**：用于“已是最后一张”“已复制到图片文件夹”“已移到废纸篓”等反馈，默认在右上角淡出（可在设置中改为居中显示）。
- **缩放倍率提示**：缩放时在画面中央短暂显示当前倍率（如 `150%`），可在设置中关闭。
