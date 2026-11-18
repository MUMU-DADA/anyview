# Anyview Windows 打包与验证脚本：
#   package.ps1 -Action stage     将 编译测试.cmd 编好测过的程序（%TEMP%\anyview-build\app）整理到 out\windows\app，
#                                 供 tools/windows/build-installer.sh 与 build-portable.sh 打包。
#   package.ps1 -Action test      将安装程序静默安装到 %TEMP%\anyview-build\pkg\install-test，核对文件、注册表、
#                                 缩略图、右键菜单与“打开方式”，再静默卸载并核对清理干净。
#                                 结果写入 reports\windows\<时间>-installer\summary.txt。
#   package.ps1 -Action portable  解压验证免安装版 out\windows\Anyview-<版本>-portable.zip，
#                                 核对配置写入程序旁 data 目录且不修改用户配置目录与注册表，
#                                 结果写入 reports\windows\<时间>-portable\summary.txt。
# test 会真的往当前用户的注册表里登记再撤销，所以电脑上已经装着 Anyview 时不跑（免得把你在用的那份的登记换掉）。
# 写给 Windows PowerShell 5.1：本文件是带 BOM 的 UTF-8。
param([ValidateSet('stage', 'test', 'portable')][string]$Action = 'stage')
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$base = Join-Path ([IO.Path]::GetTempPath()) 'anyview-build'
$app = Join-Path $base 'app'
$pkg = Join-Path $base 'pkg'
$staged = Join-Path $root 'out\windows\app'
$installDir = Join-Path $pkg 'install-test\Anyview'
$filesDir = Join-Path $pkg 'files'
$utf8 = New-Object System.Text.UTF8Encoding($false)
$stamp = Get-Date -Format "yyyy-MM-dd'T'HH-mm-ss"
$results = Join-Path $root ('reports\windows\' + $stamp + $(if ($Action -eq 'portable') { '-portable' } else { '-installer' }))
$lines = New-Object System.Collections.Generic.List[string]
$failed = 0

function Say([string]$Text) {
    Write-Host $Text
    $lines.Add($Text)
}

function Check([string]$Name, [bool]$Passed, [string]$Detail = '') {
    if ($Passed) {
        Say ('  通过  ' + $Name)
    } else {
        $script:failed++
        if ($Detail -ne '') { Say ('  没通过  ' + $Name + '：' + $Detail) } else { Say ('  没通过  ' + $Name) }
    }
}

function Wait-Until([scriptblock]$Condition, [int]$Seconds) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return [bool](& $Condition)
}

# ---- 整理构建产物到 out\windows\app ----
function Invoke-Stage {
    foreach ($needed in 'Anyview.exe', 'anyview_ffi.dll', 'Microsoft.ui.xaml.dll') {
        if (-not (Test-Path -LiteralPath (Join-Path $app $needed))) { throw ('没有编好的程序（' + $app + ' 里缺 ' + $needed + '）：先运行 编译测试.cmd') }
    }
    $coreHere = (Get-FileHash -LiteralPath (Join-Path $app 'anyview_ffi.dll') -Algorithm SHA256).Hash
    $coreShared = (Get-FileHash -LiteralPath (Join-Path $root 'out\windows\anyview_ffi.dll') -Algorithm SHA256).Hash
    if ($coreHere -ne $coreShared) { throw '构建目录中的核心 DLL 版本与 out\windows 不一致：请先重新运行 编译测试.cmd' }
    New-Item -ItemType Directory -Force -Path $staged | Out-Null
    & robocopy.exe $app $staged /MIR /XF *.pdb /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -ge 8) { throw ('复制失败（robocopy ' + $LASTEXITCODE + '）') }
    $count = @(Get-ChildItem -LiteralPath $staged -Recurse -File).Count
    $size = [math]::Round((Get-ChildItem -LiteralPath $staged -Recurse -File | Measure-Object Length -Sum).Sum / 1MB)
    Say ('已整理到 ' + $staged + '：' + $count + ' 个文件，' + $size + ' MB。接下来可运行 tools/windows/build-installer.sh 或 build-portable.sh')
}

# ---- 检查用的 C#（Windows PowerShell 自带的编译器，C# 5 的写法） ----
$helperSource = @'
using System;
using System.Text;
using System.Runtime.InteropServices;

public static class AnyviewInstallerCheck {
    [ComImport, Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IShellItemImageFactory { [PreserveSig] int GetImage(SIZE size, int flags, out IntPtr bitmap); }
    [ComImport, Guid("43826d1e-e718-42ee-bc55-a1e261c37bfe"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IShellItem { [PreserveSig] int BindToHandler(IntPtr context, ref Guid handler, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out IContextMenu menu); }
    [ComImport, Guid("000214e4-0000-0000-c000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IContextMenu {
        [PreserveSig] int QueryContextMenu(IntPtr menu, uint index, uint first, uint last, uint flags);
        [PreserveSig] int InvokeCommand(ref CMINVOKECOMMANDINFO info);
    }
    [StructLayout(LayoutKind.Sequential)]
    struct CMINVOKECOMMANDINFO { public int cbSize; public int fMask; public IntPtr hwnd; public IntPtr lpVerb; public IntPtr lpParameters; public IntPtr lpDirectory; public int nShow; public int dwHotKey; public IntPtr hIcon; }
    [ComImport, Guid("973810ae-9599-4b88-9e4d-6ee98c9552da"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IEnumAssocHandlers { [PreserveSig] int Next(uint count, [MarshalAs(UnmanagedType.Interface)] out IAssocHandler handler, out uint fetched); }
    [ComImport, Guid("f04061ac-1659-4a3f-a954-775aa57fc083"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IAssocHandler {
        [PreserveSig] int GetName([MarshalAs(UnmanagedType.LPWStr)] out string name);
        [PreserveSig] int GetUIName([MarshalAs(UnmanagedType.LPWStr)] out string name);
    }
    [DllImport("shell32", CharSet = CharSet.Unicode)] static extern int SHAssocEnumHandlers(string extension, int filter, [MarshalAs(UnmanagedType.Interface)] out IEnumAssocHandlers handlers);

    // 系统为这个扩展名列出的“可以用来打开它的程序”（“打开方式”、选默认程序的列表都从这里来），各个程序显示的名字。
    public static string Handlers(string extension) {
        IEnumAssocHandlers handlers;
        int hr = SHAssocEnumHandlers(extension, 0, out handlers);
        if (hr < 0) return "0x" + hr.ToString("X8");
        StringBuilder all = new StringBuilder();
        while (true) {
            IAssocHandler handler; uint fetched;
            if (handlers.Next(1, out handler, out fetched) != 0 || fetched == 0) break;
            string name;
            if (handler.GetUIName(out name) >= 0) all.Append(name).Append('|');
            Marshal.ReleaseComObject(handler);
        }
        Marshal.ReleaseComObject(handlers);
        return all.ToString();
    }

    [StructLayout(LayoutKind.Sequential)] struct SIZE { public int cx, cy; }
    [StructLayout(LayoutKind.Sequential)] struct BITMAP { public int type, width, height, widthBytes; public ushort planes, bitsPixel; public IntPtr bits; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct SHELLEXECUTEINFO {
        public int cbSize; public uint fMask; public IntPtr hwnd; public string lpVerb; public string lpFile; public string lpParameters; public string lpDirectory;
        public int nShow; public IntPtr hInstApp; public IntPtr lpIDList; public string lpClass; public IntPtr hkeyClass; public uint dwHotKey; public IntPtr hIcon; public IntPtr hProcess;
    }

    [DllImport("shlwapi", CharSet = CharSet.Unicode)] static extern int AssocQueryStringW(int flags, int what, string assoc, string extra, StringBuilder output, ref int size);
    [DllImport("shell32", CharSet = CharSet.Unicode, EntryPoint = "SHCreateItemFromParsingName")] static extern int SHCreateImageFactory(string path, IntPtr context, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out IShellItemImageFactory item);
    [DllImport("shell32", CharSet = CharSet.Unicode, EntryPoint = "SHCreateItemFromParsingName")] static extern int SHCreateShellItem(string path, IntPtr context, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out IShellItem item);
    [DllImport("shell32", CharSet = CharSet.Unicode, SetLastError = true)] static extern bool ShellExecuteExW(ref SHELLEXECUTEINFO info);
    [DllImport("gdi32")] static extern int GetObjectW(IntPtr handle, int size, out BITMAP bitmap);
    [DllImport("gdi32")] static extern bool DeleteObject(IntPtr handle);
    [DllImport("user32")] static extern IntPtr CreatePopupMenu();
    [DllImport("user32")] static extern bool DestroyMenu(IntPtr menu);
    [DllImport("user32")] static extern int GetMenuItemCount(IntPtr menu);
    [DllImport("user32", CharSet = CharSet.Unicode)] static extern int GetMenuStringW(IntPtr menu, uint item, StringBuilder text, int size, uint flags);

    // 资源管理器自己的那条路：按文件问系统要缩略图（只要缩略图，没有就失败，不拿图标顶替）。
    public static string ShellThumbnail(string path, int side) {
        Guid iid = new Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"); IShellItemImageFactory factory;
        int hr = SHCreateImageFactory(path, IntPtr.Zero, ref iid, out factory);
        if (hr < 0) return "item 0x" + hr.ToString("X8");
        SIZE size; size.cx = side; size.cy = side; IntPtr bitmap;
        hr = factory.GetImage(size, 0x8, out bitmap);
        Marshal.ReleaseComObject(factory);
        if (hr < 0) return "0x" + hr.ToString("X8");
        BITMAP b; GetObjectW(bitmap, Marshal.SizeOf(typeof(BITMAP)), out b);
        DeleteObject(bitmap);
        return "ok " + b.width + "x" + b.height;
    }

    // 系统现在给这个扩展名用哪个缩略图组件（类号）；没有时为空串。
    public static string ThumbnailHandler(string extension) {
        StringBuilder output = new StringBuilder(512); int size = output.Capacity;
        return AssocQueryStringW(0, 16, extension, "{e357fccd-a995-4576-b01f-234630154e96}", output, ref size) < 0 ? "" : output.ToString();
    }

    // 资源管理器给这个文件的右键菜单里的各项文字（不弹出菜单，只让系统把菜单填好再读出来）。
    public static string ContextMenu(string path) {
        Guid itemIid = new Guid("43826d1e-e718-42ee-bc55-a1e261c37bfe"); IShellItem item;
        int hr = SHCreateShellItem(path, IntPtr.Zero, ref itemIid, out item);
        if (hr < 0) return "item 0x" + hr.ToString("X8");
        Guid handler = new Guid("3981e225-f559-11d3-8e3a-00c04f6837d5"), menuIid = new Guid("000214e4-0000-0000-c000-000000000046"); IContextMenu menu;
        hr = item.BindToHandler(IntPtr.Zero, ref handler, ref menuIid, out menu);
        if (hr < 0) return "menu 0x" + hr.ToString("X8");
        IntPtr popup = CreatePopupMenu();
        hr = menu.QueryContextMenu(popup, 0, 1, 0x7FFF, 0);
        StringBuilder all = new StringBuilder();
        int count = GetMenuItemCount(popup);
        for (int i = 0; i < count; i++) {
            StringBuilder text = new StringBuilder(260);
            if (GetMenuStringW(popup, (uint)i, text, text.Capacity, 0x400) > 0) all.Append(text).Append('|');
        }
        DestroyMenu(popup);
        Marshal.ReleaseComObject(menu); Marshal.ReleaseComObject(item);
        return hr < 0 ? "query 0x" + hr.ToString("X8") : all.ToString();
    }

    // 在这个文件的右键菜单里点某一项（按那一项登记的名字）：和用户在资源管理器里点它走的是同一条路。
    public static string InvokeMenu(string path, string verb) {
        Guid itemIid = new Guid("43826d1e-e718-42ee-bc55-a1e261c37bfe"); IShellItem item;
        int hr = SHCreateShellItem(path, IntPtr.Zero, ref itemIid, out item);
        if (hr < 0) return "item 0x" + hr.ToString("X8");
        Guid handler = new Guid("3981e225-f559-11d3-8e3a-00c04f6837d5"), menuIid = new Guid("000214e4-0000-0000-c000-000000000046"); IContextMenu menu;
        hr = item.BindToHandler(IntPtr.Zero, ref handler, ref menuIid, out menu);
        if (hr < 0) return "menu 0x" + hr.ToString("X8");
        IntPtr popup = CreatePopupMenu();
        menu.QueryContextMenu(popup, 0, 1, 0x7FFF, 0);
        CMINVOKECOMMANDINFO info = new CMINVOKECOMMANDINFO(); info.cbSize = Marshal.SizeOf(typeof(CMINVOKECOMMANDINFO));
        info.fMask = 0x400; info.lpVerb = Marshal.StringToHGlobalAnsi(verb); info.nShow = 1;
        hr = menu.InvokeCommand(ref info);
        Marshal.FreeHGlobal(info.lpVerb); DestroyMenu(popup);
        Marshal.ReleaseComObject(menu); Marshal.ReleaseComObject(item);
        return hr < 0 ? "0x" + hr.ToString("X8") : "ok";
    }

    // 用指定的文件类型登记（progId）打开一个文件：等于在“打开方式”里选了它，不改默认程序，也不弹“选择程序”。
    public static string OpenWith(string file, string progId) {
        SHELLEXECUTEINFO info = new SHELLEXECUTEINFO(); info.cbSize = Marshal.SizeOf(typeof(SHELLEXECUTEINFO));
        info.fMask = 0x1 | 0x400; info.lpVerb = "open"; info.lpFile = file; info.lpClass = progId; info.nShow = 1;
        return ShellExecuteExW(ref info) ? "ok" : "error " + Marshal.GetLastWin32Error();
    }
}
'@

# 当前用户的注册表里和文件类型有关的部分（各扩展名、SystemFileAssociations、RegisteredApplications）的全部值：
# 安装前记一份，卸载后再记一份，应当一样。
function Add-KeyDump([string]$Path, $Lines) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Path)
    if (-not $key) { return }
    foreach ($name in $key.GetValueNames()) {
        $Lines.Add($Path + '|' + $name + '|' + [string]$key.GetValue($name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames))
    }
    foreach ($sub in $key.GetSubKeyNames()) { Add-KeyDump ($Path + '\' + $sub) $Lines }
    $key.Close()
}
function Get-RegistryDump {
    $dump = New-Object System.Collections.Generic.List[string]
    $classes = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Classes')
    foreach ($name in $classes.GetSubKeyNames()) {
        if ($name.StartsWith('.')) { Add-KeyDump ('Software\Classes\' + $name) $dump }
    }
    $classes.Close()
    Add-KeyDump 'Software\Classes\SystemFileAssociations' $dump
    Add-KeyDump 'Software\RegisteredApplications' $dump
    return $dump
}
function Read-Value([string]$Path, [string]$Name) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Path)
    if (-not $key) { return $null }
    $value = $key.GetValue($Name)
    $key.Close()
    return $value
}
function Test-Key([string]$Path) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Path)
    if ($key) { $key.Close(); return $true }
    return $false
}

function Get-InstalledProcess {
    Get-CimInstance Win32_Process -Filter "Name = 'Anyview.exe'" | Where-Object { $_.ExecutablePath -like ($installDir + '\*') }
}
# 关掉装在检查用的文件夹里的 Anyview（先好好关，不行再结束进程）。
function Close-Installed {
    $mine = { Get-Process -Name Anyview -ErrorAction SilentlyContinue | Where-Object { $_.Path -like ($installDir + '\*') } }
    foreach ($process in (& $mine)) { $null = $process.CloseMainWindow() }
    if (-not (Wait-Until { -not (& $mine) } 8)) { & $mine | ForEach-Object { Stop-Process -Id $_.Id -Force } }
}
function Get-ThumbnailHosts {
    @(Get-Process -Name dllhost -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Modules | Where-Object { $_.FileName -like ($installDir + '\*') } } catch { $false }
    })
}
function Invoke-Anyview([string[]]$Arguments) {
    $process = Start-Process -FilePath (Join-Path $installDir 'Anyview.exe') -ArgumentList $Arguments -PassThru -Wait
    return $process.ExitCode
}

function Invoke-Test {
    $setup = Get-ChildItem -LiteralPath (Join-Path $root 'out\windows') -Filter 'Anyview-*-setup.exe' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
    if (-not $setup) { throw 'out\windows 中未找到安装程序：请先运行 tools/windows/build-installer.sh' }
    $existing = Read-Value 'Software\Anyview' 'InstallDir'
    if ($existing -and $existing -ne $installDir) { throw ('这台电脑上装着 Anyview（' + $existing + '）：这个检查要登记再撤销，会把它的登记换掉。先卸载它') }
    Add-Type -TypeDefinition $helperSource
    $fixtures = Join-Path $root 'tests\fixtures\generated'
    New-Item -ItemType Directory -Force -Path $pkg | Out-Null
    if (Test-Path -LiteralPath $filesDir) { Remove-Item -LiteralPath $filesDir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $filesDir | Out-Null
    # 每次用新的文件名：系统把做过的缩略图存了起来，同一个文件再问拿到的是存的，看不出这次有没有真的做。
    $tag = Get-Date -Format 'HHmmss'
    $samples = [ordered]@{}
    $sources = @('zthumbs\quadrants.qoi', 'formats\psd_rgb8_rle.psd', 'formats\psb_rgb8_rle.psb', 'modern\clip_canvas.clip', 'basic\lossless_640x480.webp',
        'modern\svg_quadrants.svg', 'modern\svgz_quadrants.svgz', 'modern\jxl_still.jxl', 'modern\avif_still.avif', 'modern\heic_still.heic', 'modern\jp2_still.jp2',
        'modern\exr_rgba32f.exr', 'formats\pcx_24bit.pcx', 'basic\photo_4000x3000.jpg') | ForEach-Object { Join-Path $fixtures $_ }
    $raw = Get-ChildItem -LiteralPath (Join-Path $root 'tests\fixtures\raw') -File -ErrorAction SilentlyContinue | Where-Object { $_.Extension -match '^\.(dng|cr2|nef|arw|kdc)$' } | Select-Object -First 1
    if ($raw) { $sources += $raw.FullName }
    foreach ($source in $sources) {
        if (-not (Test-Path -LiteralPath $source)) { continue }
        $copy = Join-Path $filesDir ('t' + $tag + '-' + [IO.Path]::GetFileName($source))
        Copy-Item -LiteralPath $source -Destination $copy -Force
        $samples[[IO.Path]::GetExtension($source).TrimStart('.').ToLowerInvariant()] = $copy
    }
    $localSetup = Join-Path $pkg 'setup.exe'
    Copy-Item -LiteralPath $setup.FullName -Destination $localSetup -Force
    $hash = (Get-FileHash -LiteralPath $localSetup -Algorithm SHA256).Hash.ToLowerInvariant()
    Say ('安装程序：' + $setup.Name + '，' + [math]::Round($setup.Length / 1MB, 1) + ' MB，SHA-256 ' + $hash)
    $before = Get-RegistryDump
    $tempCopiesBefore = @(Get-ChildItem -LiteralPath ([IO.Path]::GetTempPath()) -Directory -Filter '~nsu*' -Force -ErrorAction SilentlyContinue | ForEach-Object { $_.Name })
    $clsid = '{1DC1F4D7-6312-479A-9043-BCA81CE0234B}'
    # 下面有两步是像平常那样启动 Anyview（右键菜单、“打开方式”），它会在 %APPDATA%\Anyview 里记最近打开的文件和窗口位置。
    # 你自己已经有这个文件夹时先存一份、最后放回去；没有时最后由卸载程序删掉（顺便查“连设置一起删”）。
    if ($script:settingsExisted) {
        if (Test-Path -LiteralPath $settingsBackup) { Remove-Item -LiteralPath $settingsBackup -Recurse -Force }
        Copy-Item -LiteralPath $userSettings -Destination $settingsBackup -Recurse -Force
    }

    Say ''
    Say '一、静默安装'
    if (Test-Path -LiteralPath (Split-Path -Parent $installDir)) { Remove-Item -LiteralPath (Split-Path -Parent $installDir) -Recurse -Force }
    # /D= 要放在最后，路径不加引号（NSIS 的规定）。
    $install = Start-Process -FilePath $localSetup -ArgumentList '/S', ('/D=' + $installDir) -PassThru -Wait
    Check '安装程序返回成功' ($install.ExitCode -eq 0) ('返回码 ' + $install.ExitCode)
    $installedFiles = @(Get-ChildItem -LiteralPath $installDir -Recurse -File -ErrorAction SilentlyContinue)
    $present = (Test-Path -LiteralPath (Join-Path $installDir 'Anyview.exe')) -and (Test-Path -LiteralPath (Join-Path $installDir 'anyview_ffi.dll')) -and (Test-Path -LiteralPath (Join-Path $installDir 'uninstall.exe'))
    Check '程序、核心 DLL、卸载程序都装进去了，没有调试符号' ($present -and @($installedFiles | Where-Object { $_.Extension -eq '.pdb' }).Count -eq 0) ($installedFiles.Count.ToString() + ' 个文件')
    # 许可证：Anyview 的 MIT 许可证和 Windows 版的第三方软件许可证装在 Anyview.exe 旁边。
    $licenseFiles = @(@('LICENSE.txt', 'THIRD-PARTY-NOTICES.txt') | ForEach-Object { Join-Path $installDir $_ })
    $licensesOk = (@($licenseFiles | Where-Object { Test-Path -LiteralPath $_ }).Count -eq 2) -and
        ((Get-Content -LiteralPath $licenseFiles[1] -TotalCount 1 -Encoding UTF8) -like 'Anyview for Windows*')
    Check '许可证（LICENSE.txt、THIRD-PARTY-NOTICES.txt）装在程序旁边' $licensesOk
    Say ('  （' + $installedFiles.Count + ' 个文件，' + [math]::Round(($installedFiles | Measure-Object Length -Sum).Sum / 1MB) + ' MB）')
    $shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Anyview.lnk'
    Check '“开始”菜单里有快捷方式，桌面上没有（没勾）' ((Test-Path -LiteralPath $shortcut) -and -not (Test-Path -LiteralPath (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Anyview.lnk')))
    Check '记下了安装位置' ((Read-Value 'Software\Anyview' 'InstallDir') -eq $installDir) ([string](Read-Value 'Software\Anyview' 'InstallDir'))
    $uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\Anyview'
    $version = [string](Read-Value $uninstallKey 'DisplayVersion')
    $sizeKb = [int](Read-Value $uninstallKey 'EstimatedSize')
    Check '“应用和功能”里有 Anyview（名字、版本、大小、卸载命令）' ((Read-Value $uninstallKey 'DisplayName') -eq 'Anyview' -and $version -ne '' -and $sizeKb -gt 30000 -and
        (Read-Value $uninstallKey 'UninstallString') -eq ('"' + $installDir + '\uninstall.exe"')) ('版本 ' + $version + '，' + $sizeKb + ' KB')
    Check '“默认应用”里能选 Anyview（登记了功能表）' ((Read-Value 'Software\RegisteredApplications' 'Anyview') -eq 'Software\Anyview\Capabilities' -and
        (Read-Value 'Software\Anyview\Capabilities\FileAssociations' '.jpg') -eq 'Anyview.Image')
    foreach ($extension in '.jpg', '.psd', '.zip') {
        $handlers = [AnyviewInstallerCheck]::Handlers($extension)
        Check ('系统列出的能打开 ' + $extension + ' 的程序里有 Anyview（“打开方式”、选默认程序时的列表）') ($handlers.Contains('Anyview|')) $handlers
    }
    Check '装完没有 Anyview 留在后台' (-not (Get-InstalledProcess))

    Say ''
    Say '二、装好的程序认得自己是装好的'
    $run = Join-Path $pkg 'run-installed'
    if (Test-Path -LiteralPath $run) { Remove-Item -LiteralPath $run -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $run | Out-Null
    $steps = 'idle:300,settings_open,settings_page:context_menu,settings_page:formats,settings_button:formats.buttons_windows:associate,settings_button:formats.buttons_windows:classic,settings_cancel'
    $null = Start-Process -FilePath (Join-Path $installDir 'Anyview.exe') -ArgumentList '--self-test', ('"' + $run + '"'), '--open', ('"' + $samples['qoi'] + '"'), '--steps', $steps -PassThru
    $report = Join-Path $run 'selftest.json'
    $null = Wait-Until { Test-Path -LiteralPath $report } 60
    Start-Sleep -Milliseconds 500
    if (Test-Path -LiteralPath $report) {
        $json = [IO.File]::ReadAllText($report, $utf8) | ConvertFrom-Json
        Check '自检通过' ($json.result -eq 'ok' -and @($json.problems).Count -eq 0) ('result ' + $json.result + '；' + (@($json.problems) -join '；'))
        $opened = $json.steps | Where-Object { $_.step -eq 'settings_open' } | Select-Object -First 1
        $pages = @($opened.settingsPages) -join '、'
        Check '设置窗口里有“上下文菜单”页（装好后才有）' ($pages -match 'context_menu') $pages
        $kinds = @($json.externals | ForEach-Object { $_.kind }) -join '、'
        Check '格式关联页的两个按钮能用' ($kinds -match 'default-apps' -and $kinds -match 'associations-by-app') ('记下的外部操作：' + $kinds)
    } else {
        Check '自检通过' $false ('没有写出 ' + $report)
    }
    $null = Wait-Until { -not (Get-InstalledProcess) } 15
    Close-Installed

    Say ''
    Say '三、资源管理器的缩略图（按文件问系统要，和资源管理器走的是同一条路）'
    $ours = @()
    $others = @()
    foreach ($extension in $samples.Keys) {
        if ($extension -eq 'jpg') { continue }
        $handler = [AnyviewInstallerCheck]::ThumbnailHandler('.' + $extension)
        if ($handler -eq $clsid) { $ours += $extension } else { $others += ('.' + $extension + '（' + $(if ($handler) { $handler } else { '没有' }) + '）') }
    }
    Check '系统不认识的格式里，至少一半现在由 Anyview 做缩略图' ($ours.Count * 2 -ge ($samples.Count - 1)) ('归 Anyview 的：' + ($ours -join ' '))
    # 按 256 像素要：RAW 这类系统自己有属性读取器的格式，小尺寸时系统先去拿文件里内嵌的小图、不问缩略图组件。
    foreach ($extension in $ours) {
        $result = [AnyviewInstallerCheck]::ShellThumbnail($samples[$extension], 256)
        Check ('.' + $extension) ($result -match '^ok ') $result
    }
    if ($others.Count -gt 0) { Say ('  （默认程序自己带缩略图组件、轮不到 Anyview 的：' + ($others -join ' ') + '）') }
    Check '常见格式的缩略图仍由系统做（.jpg）' ([AnyviewInstallerCheck]::ThumbnailHandler('.jpg') -ne $clsid -and [AnyviewInstallerCheck]::ShellThumbnail($samples['jpg'], 256) -match '^ok ')
    Check '缩略图是系统在单独的进程（dllhost）里用我们的 DLL 做的' ((Get-ThumbnailHosts).Count -gt 0)

    Say ''
    Say '四、右键菜单、“打开方式”'
    $shown = [AnyviewInstallerCheck]::ContextMenu($samples['qoi'])
    Check '右键菜单默认没有 Anyview 那一项（设置里没开）' ($shown -notmatch 'Anyview 浏览|Browse with Anyview' -and $shown -notmatch '^(item|menu|query) ') $shown
    $settingsDir = Join-Path $pkg 'settings-on'
    New-Item -ItemType Directory -Force -Path $settingsDir | Out-Null
    $settingsFile = Join-Path $settingsDir 'settings.toml'
    [IO.File]::WriteAllText($settingsFile, "[context_menu]`nenabled = true`nthumb_psd = false`n`n[language]`nlanguage = `"zh-Hans`"`n", $utf8)
    $code = Invoke-Anyview @('--register', '--version', $version, '--size-kb', $sizeKb, '--settings', ('"' + $settingsFile + '"'))
    Check '按新的设置重新登记成功' ($code -eq 0) ('返回码 ' + $code)
    $shown = [AnyviewInstallerCheck]::ContextMenu($samples['qoi'])
    Check '设置里开了以后，图片的右键菜单里有“用 Anyview 浏览图片”' ($shown.Contains('用 Anyview 浏览图片|')) $shown
    $shown = [AnyviewInstallerCheck]::ContextMenu($samples['jpg'])
    Check '常见格式（.jpg）的右键菜单里也有' ($shown.Contains('用 Anyview 浏览图片|')) $shown
    Check '关掉 PSD 的缩略图后，PSD 不再由 Anyview 做' ([AnyviewInstallerCheck]::ThumbnailHandler('.psd') -ne $clsid)
    $executed = [AnyviewInstallerCheck]::InvokeMenu($samples['clip'], 'Anyview.Browse')
    $null = Wait-Until { Get-InstalledProcess } 15
    $process = Get-InstalledProcess | Select-Object -First 1
    $commandLine = ''
    if ($process) { $commandLine = [string]$process.CommandLine }
    Check '点右键菜单的那一项：Anyview 启动并打开选中的文件' ($executed -eq 'ok' -and $commandLine.Contains($samples['clip'])) ('执行 ' + $executed + '；命令行 ' + $commandLine)
    $titleOk = Wait-Until { @(Get-Process -Name Anyview -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowTitle -like ('*' + [IO.Path]::GetFileName($samples['clip']) + '*') }).Count -gt 0 } 15
    Check '窗口标题是那个文件' $titleOk (@(Get-Process -Name Anyview -ErrorAction SilentlyContinue | ForEach-Object { $_.MainWindowTitle }) -join '、')
    Close-Installed
    $executed = [AnyviewInstallerCheck]::OpenWith($samples['jpg'], 'Anyview.Image')
    $null = Wait-Until { Get-InstalledProcess } 15
    $process = Get-InstalledProcess | Select-Object -First 1
    $commandLine = ''
    if ($process) { $commandLine = [string]$process.CommandLine }
    Check '在“打开方式”里选 Anyview 打开一张 JPEG：命令行里是这个文件' ($executed -eq 'ok' -and $commandLine.Contains($samples['jpg'])) ('执行 ' + $executed + '；命令行 ' + $commandLine)
    Close-Installed
    [IO.File]::WriteAllText($settingsFile, "[context_menu]`nenabled = false`n", $utf8)
    $code = Invoke-Anyview @('--register', '--version', $version, '--size-kb', $sizeKb, '--settings', ('"' + $settingsFile + '"'))
    $shown = [AnyviewInstallerCheck]::ContextMenu($samples['qoi'])
    Check '设置里关掉后右键菜单里没有这一项了' ($code -eq 0 -and $shown -notmatch 'Anyview 浏览' -and $shown -notmatch '^(item|menu|query) ') $shown
    Check '再做一张缩略图，让系统的缩略图进程占着我们的 DLL（下面卸载要删得掉）' ([AnyviewInstallerCheck]::ShellThumbnail($samples['svgz'], 96) -match '^ok |^0x')

    Say ''
    Say '五、再装一遍到同一个位置（升级）'
    # 这时系统的缩略图进程正占着我们的 DLL：安装程序要先让它退出，文件才换得掉。
    $again = Start-Process -FilePath $localSetup -ArgumentList '/S', ('/D=' + $installDir) -PassThru -Wait
    Check '安装程序返回成功' ($again.ExitCode -eq 0) ('返回码 ' + $again.ExitCode)
    Check '登记还在，安装位置没变' ((Read-Value 'Software\Anyview' 'InstallDir') -eq $installDir -and (Read-Value $uninstallKey 'DisplayName') -eq 'Anyview')
    $fresh = Join-Path $filesDir ('u' + $tag + '-quadrants.qoi')
    Copy-Item -LiteralPath $samples['qoi'] -Destination $fresh -Force
    $result = [AnyviewInstallerCheck]::ShellThumbnail($fresh, 256)
    Check '升级后缩略图照常' ($result -match '^ok ') $result

    Say ''
    Say '六、静默卸载（和平常卸载走同一条路，只是不显示窗口）'
    # 和平常卸载一样：卸载程序把自己复制到临时文件夹再运行（这样才删得掉安装文件夹里的那个），所以这里启动的那个马上就
    # 返回，要等安装文件夹消失。加 /PURGE 是连设置一起删（平常卸载时会问）；你自己有设置时不加，设置由下面放回原样。
    $uninstallArguments = @('/S')
    if (-not $script:settingsExisted) { $uninstallArguments += '/PURGE' }
    $null = Start-Process -FilePath (Join-Path $installDir 'uninstall.exe') -ArgumentList $uninstallArguments -PassThru
    $gone = Wait-Until { -not (Test-Path -LiteralPath $installDir) } 60
    $left = @(Get-ChildItem -LiteralPath $installDir -Recurse -Force -ErrorAction SilentlyContinue)
    Check '装进去的文件、卸载程序和安装文件夹都删了' $gone (($left | Select-Object -First 5 | ForEach-Object { $_.FullName.Substring($installDir.Length) }) -join '、')
    Remove-Item -LiteralPath (Split-Path -Parent $installDir) -Recurse -Force -ErrorAction SilentlyContinue
    $tempCopies = { @(Get-ChildItem -LiteralPath ([IO.Path]::GetTempPath()) -Directory -Filter '~nsu*' -Force -ErrorAction SilentlyContinue | Where-Object { $tempCopiesBefore -notcontains $_.Name }) }
    $cleaned = Wait-Until { (& $tempCopies).Count -eq 0 } 20
    Check '卸载程序在临时文件夹里的副本也删了' $cleaned ((& $tempCopies | ForEach-Object { $_.Name }) -join '、')
    Check '快捷方式删了' (-not (Test-Path -LiteralPath $shortcut))
    $ownKeys = @('Software\Anyview', 'Software\Classes\Anyview.Image', 'Software\Classes\Anyview.Archive', ('Software\Classes\CLSID\' + $clsid), 'Software\Classes\Applications\Anyview.exe', $uninstallKey) | Where-Object { Test-Key $_ }
    Check 'Anyview 自己的注册表项都没有了' (@($ownKeys).Count -eq 0) (@($ownKeys) -join '、')
    $after = Get-RegistryDump
    $difference = @(Compare-Object -ReferenceObject @($before) -DifferenceObject @($after) | ForEach-Object { $_.SideIndicator + ' ' + $_.InputObject })
    Check ('注册表里文件类型那一块和安装前一模一样（核对了 ' + @($before).Count + ' 个值）') ($difference.Count -eq 0) (($difference | Select-Object -First 6) -join '；')
    Check '没有还占着我们的 DLL 的进程' ((Get-ThumbnailHosts).Count -eq 0)
    Check '缩略图用的临时文件夹没有留下' (-not (Test-Path -LiteralPath (Join-Path ([IO.Path]::GetTempPath()) 'Anyview-thumbnails')))
    if ($script:settingsExisted) {
        Say '  （你自己的 Anyview 设置：检查前存了一份，结束时放回原样）'
    } else {
        Check '连设置一起删：%APPDATA%\Anyview 没有留下' (-not (Test-Path -LiteralPath $userSettings))
    }
}

# 把你自己的设置放回检查前的样子（检查前没有这个文件夹时，把检查中产生的删掉）。
function Restore-Settings {
    if ($script:settingsExisted) {
        if (Test-Path -LiteralPath $settingsBackup) {
            if (Test-Path -LiteralPath $userSettings) { Remove-Item -LiteralPath $userSettings -Recurse -Force }
            Copy-Item -LiteralPath $settingsBackup -Destination $userSettings -Recurse -Force
            Remove-Item -LiteralPath $settingsBackup -Recurse -Force
        }
    } elseif (Test-Path -LiteralPath $userSettings) {
        Remove-Item -LiteralPath $userSettings -Recurse -Force
    }
}

# ---- 免安装版----
$portableDir = Join-Path $pkg 'portable-test'
# 系统的用户文件夹（%APPDATA%\Anyview）现在的样子：每个文件的路径、大小、修改时间。
function Get-UserSettingsState {
    if (-not (Test-Path -LiteralPath $userSettings)) { return '' }
    return (@(Get-ChildItem -LiteralPath $userSettings -Recurse -File -Force -ErrorAction SilentlyContinue | ForEach-Object {
        $_.FullName + '|' + $_.Length + '|' + $_.LastWriteTimeUtc.Ticks
    }) -join "`n")
}
function Invoke-PortableTest {
    $zip = Get-ChildItem -LiteralPath (Join-Path $root 'out\windows') -Filter 'Anyview-*-portable.zip' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
    if (-not $zip) { throw 'out\windows 中未找到免安装版压缩包：请先运行 tools/windows/build-portable.sh' }
    $mine = { Get-Process -Name Anyview -ErrorAction SilentlyContinue | Where-Object { $_.Path -like ($portableDir + '\*') } }
    New-Item -ItemType Directory -Force -Path $pkg | Out-Null
    if (Test-Path -LiteralPath $portableDir) { Remove-Item -LiteralPath $portableDir -Recurse -Force }
    $hash = (Get-FileHash -LiteralPath $zip.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    Say ('免安装版：' + $zip.Name + '，' + [math]::Round($zip.Length / 1MB, 1) + ' MB，SHA-256 ' + $hash)
    $registryBefore = Get-RegistryDump
    $settingsBefore = Get-UserSettingsState
    Expand-Archive -LiteralPath $zip.FullName -DestinationPath $portableDir -Force
    $folder = Join-Path $portableDir 'Anyview'
    $exe = Join-Path $folder 'Anyview.exe'
    $files = @(Get-ChildItem -LiteralPath $folder -Recurse -File -ErrorAction SilentlyContinue)
    Check '解压出来是一个 Anyview 文件夹，里面有程序和 portable.txt，没有调试符号' ((Test-Path -LiteralPath $exe) -and (Test-Path -LiteralPath (Join-Path $folder 'portable.txt')) -and @($files | Where-Object { $_.Extension -eq '.pdb' }).Count -eq 0)
    Check '带着许可证（LICENSE.txt、THIRD-PARTY-NOTICES.txt）' ((Test-Path -LiteralPath (Join-Path $folder 'LICENSE.txt')) -and
        ((Get-Content -LiteralPath (Join-Path $folder 'THIRD-PARTY-NOTICES.txt') -TotalCount 1 -Encoding UTF8) -like 'Anyview for Windows*'))
    Say ('  （' + $files.Count + ' 个文件，' + [math]::Round(($files | Measure-Object Length -Sum).Sum / 1MB) + ' MB）')
    # 打开一张图片（会记进“最近打开”），等窗口出来，再像用户那样关掉（关窗口时记窗口位置）。
    $sample = Join-Path $root 'tests\fixtures\generated\basic\photo_4000x3000.jpg'
    $process = Start-Process -FilePath $exe -ArgumentList ('"' + $sample + '"') -PassThru
    $shown = Wait-Until { $process.Refresh(); $process.MainWindowHandle -ne [IntPtr]::Zero } 20
    Check '程序打开了窗口' $shown
    Start-Sleep -Seconds 2
    foreach ($running in (& $mine)) { $null = $running.CloseMainWindow() }
    $closed = Wait-Until { -not (& $mine) } 10
    if (-not $closed) { & $mine | ForEach-Object { Stop-Process -Id $_.Id -Force } }
    Check '程序正常关掉了' $closed
    $data = Join-Path $folder 'data'
    $written = @(Get-ChildItem -LiteralPath $data -File -ErrorAction SilentlyContinue | ForEach-Object { $_.Name })
    Check '设置、最近打开的都存在程序旁边的 data 文件夹里' (($written -contains 'settings.toml') -or ($written -contains 'recent.toml')) ($written -join '、')
    Say ('  （data 里：' + ($written -join '、') + '）')
    Check '没有往系统的用户文件夹（%APPDATA%\Anyview）里写东西' ((Get-UserSettingsState) -eq $settingsBefore)
    $diff = @(Compare-Object $registryBefore (Get-RegistryDump))
    Check '注册表里的文件类型、“打开方式”、默认应用都没有变' ($diff.Count -eq 0) (($diff | Select-Object -First 3 | ForEach-Object { [string]$_.InputObject }) -join '；')
    Remove-Item -LiteralPath $portableDir -Recurse -Force -ErrorAction SilentlyContinue
    Check '整个文件夹删得掉（没有被占着的文件）' (-not (Test-Path -LiteralPath $portableDir))
}

# 出了错、中途被打断：把检查时装的那一份撤掉（已经卸载干净时什么也不做）。
function Invoke-Rescue {
    $exe = Join-Path $installDir 'Anyview.exe'
    if ((Read-Value 'Software\Anyview' 'InstallDir') -eq $installDir) {
        Close-Installed
        if (Test-Path -LiteralPath $exe) { $null = Invoke-Anyview @('--unregister') }
        Remove-Item -LiteralPath (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Anyview.lnk') -Force -ErrorAction SilentlyContinue
        Say '（检查没有走完：已经撤掉检查时装的那一份的登记）'
    }
}

$userSettings = Join-Path $env:APPDATA 'Anyview'
$settingsBackup = Join-Path $pkg 'settings-backup'
$settingsExisted = Test-Path -LiteralPath $userSettings
try {
    if ($Action -eq 'stage') {
        Invoke-Stage
    } elseif ($Action -eq 'portable') {
        Say ('Anyview 免安装版的检查  ' + $stamp)
        try { Invoke-PortableTest } finally {
            if (Test-Path -LiteralPath $portableDir) {
                Get-Process -Name Anyview -ErrorAction SilentlyContinue | Where-Object { $_.Path -like ($portableDir + '\*') } | ForEach-Object { Stop-Process -Id $_.Id -Force }
                Remove-Item -LiteralPath $portableDir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }
        Say ''
        if ($failed -eq 0) { Say '结果：全部通过' } else { Say ('结果：' + $failed + ' 项没通过') }
    } else {
        Say ('Anyview 安装程序的检查  ' + $stamp)
        try { Invoke-Test } finally { Invoke-Rescue; Restore-Settings }
        Say ''
        if ($failed -eq 0) { Say '结果：全部通过' } else { Say ('结果：' + $failed + ' 项没通过') }
    }
} catch {
    $failed++
    Say ('出错：' + $_.Exception.Message)
} finally {
    if ($Action -ne 'stage') {
        New-Item -ItemType Directory -Force -Path $results | Out-Null
        [IO.File]::AppendAllText((Join-Path $results 'summary.txt'), (($lines -join "`r`n") + "`r`n"), $utf8)
        Write-Host ('结果写在 ' + (Join-Path $results 'summary.txt'))
    }
}
if ($failed -gt 0) { exit 1 }
exit 0
