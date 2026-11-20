# 动作目录与快捷键

右键菜单、底部控制栏、顶部菜单栏、键盘映射和鼠标映射共用同一套动作目录（定义于 `crates/anyview-core/src/actions.rs`）。菜单排列见 [MENUS.md](MENUS.md)，macOS 菜单栏布局见 [PLATFORMS.md](PLATFORMS.md#菜单栏)。

## 约定

- **ID**：代码内固定动作标识符，不随界面翻译变化。
- **类型**：命令（执行一次）、开关（勾选/取消）、单选（同组互斥）。
- **入口**：`R` = 右键菜单，`B` = 底部控制栏，`K` = 键鼠自定义映射下拉列表。
- 列出多个默认键位时，第一个显示在菜单中，其余同样生效；`—` 表示无默认键位，可在设置中自行绑定。

## macOS 键位映射规则

1. 修饰键对应：`Ctrl` → `⌘`，`Alt` → `⌥`，`Shift` → `⇧`。
2. 与 macOS 通用快捷键冲突的 `⌘` 组合改用 `⇧⌘`（如 `⌘F`、`⌘G`、`⌘M`、`⌘I`、`⌘A`），按键本身含 `⇧` 的改用 `⌥⌘`。
3. 常用无修饰键看图按键跨平台保持一致：方向键、`Home`、`End`、`+`/`=`、`-`、`*`、数字键、`Z`、`U`、`I`、`S`、`[`、`]`、`Tab`、`↩`。
4. 常规系统操作遵循 macOS 习惯：`⌘O` 打开、`⌘,` 设置、`⌘⌫` 移到废纸篓、`⌘Q` 退出、`⌘W` 关闭窗口、`⌘M` 最小化、`⌘0`/`⌘9`/`⌘+`/`⌘-` 缩放、`⌘L`/`⌘R` 旋转、`⌃⌘F` 全屏。
5. 避开系统保留快捷键：`⌃←`/`→`/`↑`/`↓`、`⌃空格`、`⌘空格`、`⌘Tab`、`⇧⌘3`/`4`/`5`、`⌃⌘Q`。
6. 菜单与按钮提示中的快捷键与用户自定义键盘映射同源，修改后实时更新。

## 动作列表

### 文件与程序

| ID | 名称 | 类型 | 入口 | Windows / Linux | macOS |
| --- | --- | --- | --- | --- | --- |
| `open_file` | 打开图片 | 命令 | R B K | `F2` | `⌘O` |
| `open_folder` | 打开文件夹 | 命令 | R K | `F` | `⇧⌘O`、`F` |
| `close_image` | 关闭（回到空态） | 命令 | R | `F4` | — |
| `select_image` | 选择图片 | 命令 | R B K | `G` | `G` |
| `reveal_file` | 定位文件 | 命令 | R | `Ctrl+Enter` | `⌘↩` |
| `set_wallpaper` | 设为桌面壁纸… | 命令 | R | — | — |
| `delete_file` | 删除（移到回收站/废纸篓） | 命令 | R | `Del` | `⌘⌫` |
| `copy_to_folder1` | 复制到图片文件夹 | 命令 | R K | `Ins` | — |
| `copy_to_folder2` | 复制到图片文件夹 2 | 命令 | K | — | — |
| `move_to_folder1` | 移动到图片文件夹 | 命令 | K | — | — |
| `open_in_editor` | 用图像编辑器打开 | 命令 | R | `Ctrl+E` | `⌘E` |
| `print` | 打印 | 命令 | R | `Ctrl+P` | `⌘P` |
| `copy_to_clipboard` | 复制到剪贴板 | 命令 | R | `Ctrl+C` | `⌘C` |
| `settings` | 设置 | 命令 | R | `F5` | `⌘,` |
| `about` | 关于 | 命令 | R | `F1` | 应用菜单 |
| `help_shortcuts` | 键盘快捷键 | 命令 | 帮助菜单 / 关于窗口 | — | — |
| `help_formats` | 支持的格式 | 命令 | 帮助菜单 / 关于窗口 | — | — |
| `help_licenses` | 开源许可 | 命令 | 帮助菜单 / 关于窗口 | — | — |
| `quit` | 退出 | 命令 | R K | `X` | `⌘Q` |
| `close_or_quit` | 关闭窗口（最后窗口时退出） | 命令 | — | `Esc` | `⎋` |
| `anim_toggle` | 暂停/继续动画 | 开关 | 动图控制 / 显示菜单 | `Space` | `Space` |
| `anim_prev_frame` | 上一帧 | 命令 | 动图控制 / 显示菜单 | `,` | `,` |
| `anim_next_frame` | 下一帧 | 命令 | 动图控制 / 显示菜单 | `.` | `.` |

### 窗口与界面

| ID | 名称 | 类型 | 入口 | Windows / Linux | macOS |
| --- | --- | --- | --- | --- | --- |
| `fullscreen` | 全屏 | 开关 | K | `Enter`、`F11` | `⌃⌘F`、`↩` |
| `maximize` | 最大化/还原 | 命令 | K | — | 窗口 › 缩放 |
| `minimize` | 最小化 | 命令 | K | — | `⌘M` |
| `always_on_top` | 总在最前 | 开关 | 设置·一般 | `Ctrl+A` | `⇧⌘A` |
| `context_menu` | 上下文菜单 | 命令 | K | — | — |
| `toggle_bar_lock` | 锁定控制栏 | 开关 | B | `~` | `` ` `` |
| `toggle_info` | EXIF / 文件信息 | 开关 | R K | `Tab` | `⌘I`、`Tab` |
| `show_generation_info` | AI 生成信息 | 命令 | R K | `P` | `P` |
| `toggle_title_filename` | 标题栏/菜单隐藏时显示文件名 | 开关 | 设置·浏览 | `Shift+Tab` | `⇧Tab` |

### 像素边缘处理（单选组）

| ID | 名称 | 类型 | 入口 | Windows / Linux | macOS |
| --- | --- | --- | --- | --- | --- |
| `edge_none` | 无（None） | 单选 | R | `U` | `U` |
| `edge_soften` | 减淡（Soften） | 单选 | R | `I` | `I` |
| `edge_soften_sharpen` | 减淡+锐化（Soften+Sharpen） | 单选 | R | `S` | `S` |

### 浏览导航

| ID | 名称 | 类型 | 入口 | Windows / Linux | macOS |
| --- | --- | --- | --- | --- | --- |
| `next_image` | 下一张图片 | 命令 | R B K | `→` | `→` |
| `prev_image` | 上一张图片 | 命令 | R B K | `←` | `←` |
| `first_image` | 首张图片 | 命令 | R K | `Home` | `Home` |
| `last_image` | 末尾图片 | 命令 | R K | `End` | `End` |
| `forward_10` | 向后 10 张图片 | 命令 | R B K | `Ctrl+PageDown` | `⌘PageDown` |
| `back_10` | 向前 10 张图片 | 命令 | R B K | `Ctrl+PageUp` | `⌘PageUp` |
| `random_forward` | 向后随机跳转 | 命令 | R K | `Ctrl+Alt+PageDown` | `⌥⌘PageDown` |
| `random_back` | 向前随机跳转 | 命令 | R K | `Ctrl+Alt+PageUp` | `⌥⌘PageUp` |
| `next_container` | 下一个文件夹/压缩包 | 命令 | R K | `]`（默认键盘映射含 `PageDown`） | `]`（同左） |
| `prev_container` | 上一个文件夹/压缩包 | 命令 | R K | `[`（默认键盘映射含 `PageUp`） | `[`（同左） |
| `next_page_double` | 下一页（双页微调一页） | 命令 | K | — | — |
| `prev_page_double` | 上一页（双页微调一页） | 命令 | K | — | — |

### 插值算法与图片效果

| ID | 名称 | 类型 | 入口 | Windows / Linux | macOS |
| --- | --- | --- | --- | --- | --- |
| `interp_bilinear` | Bilinear 插值（快） | 单选 | R、设置 | — | — |
| `interp_bicubic` | Bicubic 插值（高品质） | 单选 | R、设置 | — | — |
| `interp_lanczos` | Lanczos 插值（高品质） | 单选 | R、设置 | — | — |
| `invert` | 反相 | 开关 | R | `Ctrl+I` | `⇧⌘I` |
| `gamma` | Gamma 补偿 | 开关 | R | `Ctrl+G` | `⇧⌘G` |
| `flip_vertical` | 垂直翻转 | 开关 | R | `Ctrl+F` | `⇧⌘F` |
| `flip_horizontal` | 水平翻转 | 开关 | R | `Ctrl+M` | `⇧⌘M` |

### 排序方式与浏览方式

| ID | 名称 | 类型 | 入口 | Windows / Linux | macOS |
| --- | --- | --- | --- | --- | --- |
| `sort_name` | 按文件名 | 单选 | R、选择图片窗口 | — | — |
| `sort_mtime` | 按修改日期 | 单选 | R、选择图片窗口 | — | — |
| `sort_size` | 按大小 | 单选 | R、选择图片窗口 | — | — |
| `sort_type` | 按类型 | 单选 | R、选择图片窗口 | — | — |
| `sort_desc` | 降序 | 开关 | R | — | — |
| `stretch_small` | 拉伸小图 | 开关 | R | `Z` | `Z` |
| `zoom_original` | 原始大小 (100%) | 单选 | R | `0` | `⌘0`、`0` |
| `fit_window` | 适合窗口 | 单选 | R | `9` | `⌘9`、`9` |
| `fit_width` | 适合宽度 | 单选 | R | `8` | `8` |
| `double_page_ltr` | 智能双页阅览 (左→右) | 开关 | R | `7` | `7` |
| `double_page_rtl` | 智能双页阅览 (左←右) | 开关 | R | `6` | `6` |

### 缩放、旋转与平移

| ID | 名称 | 类型 | 入口 | Windows / Linux | macOS |
| --- | --- | --- | --- | --- | --- |
| `zoom_in` | 放大 | 命令 | R B K | `+`、`=` | `⌘+`、`+`、`⌘=`、`=` |
| `zoom_out` | 缩小 | 命令 | R B K | `-` | `⌘-`、`-` |
| `zoom_lock` | 锁定缩放 | 开关 | R | `Ctrl+K` | `⌘K` |
| `zoom_100` | 100% | 命令 | R | `*` | `*` |
| `zoom_in_1` | 放大 (1%) | 命令 | R | `Ctrl++`、`Ctrl+=` | `⌥⌘+`、`⌥⌘=` |
| `zoom_out_1` | 缩小 (1%) | 命令 | R | `Ctrl+-` | `⌥⌘-` |
| `rotate_none` | 不旋转 | 命令（勾选显示当前角度） | R | `Alt+Up` | `⌥↑` |
| `rotate_ccw` | 逆时针旋转 90° | 命令（勾选显示当前角度） | R B | `Alt+Left` | `⌘L`、`⌥←` |
| `rotate_cw` | 顺时针旋转 90° | 命令（勾选显示当前角度） | R B | `Alt+Right` | `⌘R`、`⌥→` |
| `rotate_180` | 旋转 180° | 命令（勾选显示当前角度） | R | `Alt+Down` | `⌥↓` |
| `pan_up` / `pan_down` | 上移 / 下移 | 命令 | K | `↑` / `↓` | `↑` / `↓` |
| `pan_left` / `pan_right` | 左移 / 右移 | 命令 | K | — | — |
| `none` | 无操作（映射屏蔽用） | 专用 | 鼠标映射 | — | — |

## 鼠标与触控板默认动作

| 输入 | 默认动作 |
| --- | --- |
| 左键拖动 | 移动窗口 / 平移图片 |
| 左键双击 | 最大化/还原 |
| 右键 | `context_menu` |
| 中键 | 无操作（拖动平移） |
| 鼠标侧键 X / Y | `prev_image` / `next_image` |
| 滚轮向上 / 向下 | `prev_image` / `next_image` |
| `⌘` / `Ctrl` + 滚轮 | 以指针为中心缩放 |
| 触控板双指滑动 | 平移 |
| 触控板捏合 | 以指针为中心缩放 |
| 触控板双指左右轻扫 | 翻页（图片宽度未超出窗口时） |
| 触控板双指轻点两下 | 在“适合窗口”与 100% 之间切换 |
| 触控板旋转手势 | 按 90° 步进旋转 |

自定义鼠标映射在「设置 › 鼠标」中配置（见 [SETTINGS.md](SETTINGS.md#6-鼠标)）。

## macOS 平台措辞对照

| 动作 ID | 通用文案 | macOS 文案 |
| --- | --- | --- |
| `reveal_file` | 定位文件 | 在访达中显示 |
| `delete_file` | 删除 | 移到废纸篓 |
| `set_wallpaper` | 设为桌面壁纸… | 设为桌面图片 |
| `copy_to_clipboard` | 复制到剪贴板 | 拷贝 |
| `settings` | 设置 | 设置… |
| `quit` | 退出 | 退出 Anyview |
| `about` | 关于 | 关于 Anyview |
