# Anyview Windows 编译与自检脚本：编译应用并运行界面自检与冷启动测试。
# 用法：双击同目录下的「编译测试.cmd」。不需要管理员权限，不安装任何东西：
#   项目里带的 .NET SDK 压缩包解压到 %TEMP%\anyview-build，NuGet 包也下载到那里，编译、测试都在那里做；
#   测试图片复制到那里再用（删除之类的操作只动副本）；不再需要时双击「清理.cmd」整个删掉。
# 自检报告写入 reports\windows\<时间>\。
# 兼容 Windows PowerShell 5.1（系统自带）；文件必须保存为带 BOM 的 UTF-8、CRLF 换行。

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$stamp = Get-Date -Format "yyyy-MM-dd'T'HH-mm-ss"
$results = Join-Path $root "reports\windows\$stamp"
$base = Join-Path ([IO.Path]::GetTempPath()) 'anyview-build'
$sdkVersion = '10.0.404'
$sdkZip = Join-Path $root "out\windows-toolchain\dotnet-sdk-$sdkVersion-win-x64.zip"
$sdkSha512 = '330ddd2665b43cd2e428216a0d8ea1ac7bb19b5434fdc530e62010dc3b52823a620bf95489d628e1ed6f5122401793ee4887cca4a019122361a1231497f632ee'
$coreDll = Join-Path $root 'out\windows\anyview_ffi.dll'
$bindings = Join-Path $root 'out\windows\bindings\anyview_ffi.cs'
$sharedFixtures = Join-Path $root 'tests\fixtures\generated'
$photo = Join-Path $root 'tests\fixtures\real\photo_12mp_iguana.jpg'
$dotnetDir = Join-Path $base 'dotnet'
$dotnet = Join-Path $dotnetDir 'dotnet.exe'
$src = Join-Path $base 'src'
$app = Join-Path $base 'app'
$runs = Join-Path $base 'runs'
$logs = Join-Path $base 'logs'
$fx = Join-Path $base 'fixtures'
$appTemp = Join-Path $base 'apptemp'
$nugetConfig = Join-Path $base 'nuget.config'
$exe = Join-Path $app 'Anyview.exe'
$t1Count = 5
$utf8 = New-Object System.Text.UTF8Encoding($false)
# 只跑其中几项（开发时用：改了一处只想看有关的那几项，不用等全部）：环境变量 ANYVIEW_TEST_ONLY 写项目的名字，逗号分隔
# （如 churn,visible）。这时不按报告核对、不量冷启动，结果目录的名字后面加 -only。
$onlyCases = @(([string]$env:ANYVIEW_TEST_ONLY).Split(',') | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' })
if ($onlyCases.Count -gt 0) { $results = "$results-only" }

# 按 Windows 命令行的规则给一个参数加引号。
function ConvertTo-Argument([string]$Text) {
    if ($Text.Length -gt 0 -and $Text -notmatch '[\s"]') { return $Text }
    $escaped = [regex]::Replace($Text, '(\\*)"', '$1$1\"')
    $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
    return '"' + $escaped + '"'
}

function Write-Utf8([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text, $utf8)
}

# 读一个可能还在被写入的文本文件；暂时读不了（被占用）时返回空串，下一次再读。
function Read-Text([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return '' }
    try {
        $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
    } catch [IO.IOException] {
        return ''
    }
    try {
        $reader = New-Object IO.StreamReader($stream, $utf8)
        return $reader.ReadToEnd()
    } finally {
        $stream.Dispose()
    }
}

# 打印 Path 里新写出的完整行（Final 时连最后没有换行的一行也打印），返回已经打印的行数。
function Show-NewLines([string]$Path, [int]$Printed, [bool]$Final) {
    $lines = (Read-Text $Path) -split "`r?`n"
    $count = $lines.Count - 1
    if ($Final -and $lines[$lines.Count - 1] -ne '') { $count = $lines.Count }
    for ($k = $Printed; $k -lt $count; $k++) { Write-Host "  $($lines[$k])" }
    return [Math]::Max($Printed, $count)
}

# 编译时用的环境：.NET、NuGet 的所有缓存、设置和临时文件都放在 $base 里，不碰用户目录；不发遥测。
# 只在运行 dotnet 时设置（Use-BuildEnvironment），运行测试程序时恢复原样（Restore-Environment），让程序在常规环境里启动。
$buildEnvironment = [ordered]@{
    DOTNET_CLI_HOME = (Join-Path $base 'home')
    DOTNET_CLI_TELEMETRY_OPTOUT = '1'
    DOTNET_NOLOGO = '1'
    DOTNET_SKIP_FIRST_TIME_EXPERIENCE = '1'
    DOTNET_GENERATE_ASPNET_CERTIFICATE = 'false'
    DOTNET_ADD_GLOBAL_TOOLS_TO_PATH = 'false'
    DOTNET_CLI_WORKLOAD_UPDATE_NOTIFY_DISABLE = '1'
    DOTNET_MULTILEVEL_LOOKUP = '0'
    NUGET_PACKAGES = (Join-Path $base 'nuget\packages')
    NUGET_HTTP_CACHE_PATH = (Join-Path $base 'nuget\http-cache')
    NUGET_PLUGINS_CACHE_PATH = (Join-Path $base 'nuget\plugins-cache')
    MSBUILDDISABLENODEREUSE = '1'
    APPDATA = (Join-Path $base 'home\AppData\Roaming')
    LOCALAPPDATA = (Join-Path $base 'home\AppData\Local')
    TEMP = (Join-Path $base 'tmp')
    TMP = (Join-Path $base 'tmp')
    PATH = ($dotnetDir + ';' + $env:PATH)
}
$savedEnvironment = @{}

function Use-BuildEnvironment {
    foreach ($name in $buildEnvironment.Keys) {
        $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        [Environment]::SetEnvironmentVariable($name, $buildEnvironment[$name], 'Process')
    }
    foreach ($name in 'APPDATA', 'LOCALAPPDATA', 'TEMP', 'DOTNET_CLI_HOME') {
        New-Item -ItemType Directory -Force -Path $buildEnvironment[$name] | Out-Null
    }
}

function Restore-Environment {
    foreach ($name in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
    }
}

# 运行一个命令行工具：输出边运行边显示，结束后连同标准错误一起存成 <Log>.txt（结果目录里）。
function Invoke-Tool {
    param([string]$Name, [string]$Log, [string]$Exe, [string[]]$Arguments = @(), [int]$TimeoutSeconds = 1800,
          [string]$WorkingDirectory = $base)
    Write-Host ''
    Write-Host "== $Name" -ForegroundColor Cyan
    $out = Join-Path $logs "$Log.out"
    $err = Join-Path $logs "$Log.err"
    $start = @{
        FilePath = $Exe; WorkingDirectory = $WorkingDirectory; NoNewWindow = $true; PassThru = $true
        RedirectStandardOutput = $out; RedirectStandardError = $err
    }
    if ($Arguments.Count -gt 0) { $start.ArgumentList = ($Arguments | ForEach-Object { ConvertTo-Argument $_ }) -join ' ' }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $printed = 0
    $status = '正常'
    try {
        $process = Start-Process @start
        $null = $process.Handle  # 先取句柄，进程结束后才读得到返回码
        while (-not $process.WaitForExit(500)) {
            $printed = Show-NewLines $out $printed $false
            if ($watch.Elapsed.TotalSeconds -gt $TimeoutSeconds) {
                $process.Kill()
                $status = "超时（超过 $TimeoutSeconds 秒，已终止）"
                break
            }
        }
        $process.WaitForExit()
        if ($status -eq '正常' -and $process.ExitCode -ne 0) { $status = "失败（返回码 $($process.ExitCode)）" }
    } catch {
        $status = "无法运行：$($_.Exception.Message)"
    }
    $printed = Show-NewLines $out $printed $true
    $text = Read-Text $out
    $errorText = Read-Text $err
    if ($errorText.Trim().Length -gt 0) {
        Write-Host $errorText.TrimEnd() -ForegroundColor Yellow
        $text += "`r`n--- 标准错误 ---`r`n" + $errorText
    }
    Write-Utf8 (Join-Path $results "$Log.txt") $text
    $seconds = [Math]::Round($watch.Elapsed.TotalSeconds, 1)
    $color = if ($status -eq '正常') { 'Green' } else { 'Red' }
    Write-Host "-- ${Name}：${status}，用时 $seconds 秒" -ForegroundColor $color
    return [pscustomobject]@{ Name = $Name; Status = $status; Seconds = $seconds; Output = $text }
}

# 某段时间里应用程序日志中的崩溃记录（程序未写出报告时辅助排查原因）。
function Save-Events([datetime]$Since, [string]$Path) {
    $lines = New-Object System.Collections.Generic.List[string]
    $providers = @('Application Error', '.NET Runtime', 'Windows Error Reporting', 'Application Hang', 'SideBySide')
    try {
        $events = @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = $Since } -ErrorAction SilentlyContinue |
            Where-Object { $providers -contains $_.ProviderName } | Select-Object -First 8)
        foreach ($e in $events) {
            $lines.Add("[$($e.TimeCreated.ToString('HH:mm:ss'))] $($e.ProviderName) $($e.Id)")
            $lines.Add([string]$e.Message)
            $lines.Add('')
        }
    } catch {
        $lines.Add("读不到事件日志：$($_.Exception.Message)")
    }
    if ($lines.Count -eq 0) { $lines.Add('这段时间的应用程序日志里没有崩溃记录。') }
    Write-Utf8 $Path (($lines -join "`r`n") + "`r`n")
}

function Get-SystemReport {
    $lines = New-Object System.Collections.Generic.List[string]
    try {
        $os = Get-CimInstance Win32_OperatingSystem
        $lines.Add("系统：$($os.Caption) $($os.Version)（内部版本 $($os.BuildNumber)）")
        $lines.Add("内存：$([Math]::Round($os.TotalVisibleMemorySize / 1MB, 1)) GB")
    } catch {
        $lines.Add("系统信息读取失败：$($_.Exception.Message)")
    }
    try {
        foreach ($cpu in @(Get-CimInstance Win32_Processor)) {
            $lines.Add("CPU：$($cpu.Name.Trim())，$($cpu.NumberOfCores) 核 $($cpu.NumberOfLogicalProcessors) 线程，最高 $($cpu.MaxClockSpeed) MHz")
        }
    } catch {
        $lines.Add("CPU 信息读取失败：$($_.Exception.Message)")
    }
    try {
        foreach ($gpu in @(Get-CimInstance Win32_VideoController)) {
            $lines.Add("显卡：$($gpu.Name)，驱动 $($gpu.DriverVersion)，$($gpu.CurrentHorizontalResolution)x$($gpu.CurrentVerticalResolution)")
        }
    } catch { }
    try {
        foreach ($disk in @(Get-PhysicalDisk)) {
            $lines.Add("硬盘：$($disk.FriendlyName)，$($disk.MediaType)，$($disk.BusType)")
        }
    } catch { }
    try {
        $plan = @(Get-CimInstance -Namespace 'root\cimv2\power' -ClassName Win32_PowerPlan -Filter 'IsActive = TRUE')
        if ($plan.Count -gt 0) { $lines.Add("电源计划：$($plan[0].ElementName)") }
    } catch { }
    try {
        $battery = @(Get-CimInstance Win32_Battery)
        if ($battery.Count -gt 0) {
            $power = if ($battery[0].BatteryStatus -eq 2) { '接着电源' } else { '在用电池（性能可能被限制）' }
            $lines.Add("电池：$power")
        }
    } catch { }
    $lines.Add("PowerShell：$($PSVersionTable.PSVersion)")
    $lines.Add("项目目录：$root")
    $lines.Add("临时目录：$base")
    return $lines
}

# 自测报告里的一个时刻（从进程创建算起的毫秒数）；没有时为空。
function Get-Mark($Report, [string]$Name) {
    if ($null -eq $Report) { return $null }
    foreach ($m in @($Report.marks)) {
        if ($m.name -eq $Name) { return [double]$m.ms }
    }
    return $null
}

function Get-Field($Report, [string]$Name) {
    if ($null -eq $Report) { return $null }
    $property = $Report.PSObject.Properties[$Name]
    if ($null -eq $property) { return $null }
    return $property.Value
}

function Get-Median($Values) {
    $list = @($Values | Where-Object { $null -ne $_ } | Sort-Object)
    if ($list.Count -eq 0) { return $null }
    return $list[[int][Math]::Floor(($list.Count - 1) / 2)]
}

function Format-Number($Value, [string]$Unit) {
    if ($null -eq $Value) { return '—' }
    return ('{0:0}' -f [double]$Value) + ' ' + $Unit
}

function Format-Size([double]$Bytes) {
    return ('{0:0}' -f ($Bytes / 1MB)) + ' MB'
}

# 几次运行里某个时刻（从进程创建算起）或报告字段（-Field）的中位数。
function Get-RunMedian($Runs, [string]$Name, [switch]$Field) {
    $values = @($Runs | ForEach-Object {
        if ($Field) { Get-Field $_.Report $Name } else { Get-Mark $_.Report $Name }
    })
    return Get-Median $values
}


# 报告里的一个值：键路径用点分隔，数字是数组下标（如 steps.9.errorPage、steps.5.windows.1.background）；没有时为空。
function Get-Value($Report, [string]$Path) {
    $node = $Report
    foreach ($part in $Path.Split('.')) {
        if ($null -eq $node) { return $null }
        $index = 0
        if ([int]::TryParse($part, [ref]$index)) {
            $items = @($node)
            if ($index -ge $items.Count) { return $null }
            $node = $items[$index]
        } else {
            $property = $node.PSObject.Properties[$part]
            if ($null -eq $property) { return $null }
            $node = $property.Value
        }
    }
    return $node
}

# 自检的"真的用鼠标拖动"（move 那一项）用 SendInput 按下左键：程序被强行终止、崩溃在按着的时候，系统会一直当左键按着
# （注入的按键系统不会自己松开）。这时补一个松开。只在左键按着时发（左右键对调时看右边的键，与程序里相同）。
function Release-InjectedButton {
    try {
        if (-not ('AnyviewMouseRelease' -as [type])) {
            Add-Type -IgnoreWarnings -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class AnyviewMouseRelease {
    [StructLayout(LayoutKind.Sequential)]
    struct MOUSEINPUT { public int dx; public int dy; public uint data; public uint flags; public uint time; public IntPtr extra; }
    [StructLayout(LayoutKind.Sequential)]
    struct INPUT { public uint type; public MOUSEINPUT mi; }
    [DllImport("user32.dll")] static extern uint SendInput(uint count, INPUT[] inputs, int size);
    [DllImport("user32.dll")] static extern short GetAsyncKeyState(int key);
    [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
    public static bool Release() {
        bool swapped = GetSystemMetrics(23) != 0;
        if ((GetAsyncKeyState(swapped ? 2 : 1) & 0x8000) == 0) { return false; }
        INPUT[] input = new INPUT[1];
        input[0].type = 0;
        input[0].mi.flags = swapped ? 0x0010u : 0x0004u;
        return SendInput(1, input, Marshal.SizeOf(typeof(INPUT))) == 1;
    }
}
'@
        }
        $null = [AnyviewMouseRelease]::Release()
    } catch {
    }
}

# 一个进程现在占的显存（MB）：任务管理器里的「专用 GPU 内存」加「共享 GPU 内存」，从系统的性能计数器读（每块显卡一项，
# 加起来）。读不出来（没有这个计数器、名字被翻译过）时为空。
function Get-GpuMemoryMb([int]$ProcessId) {
    try {
        $sample = Get-Counter -Counter "\GPU Process Memory(pid_${ProcessId}_*)\Dedicated Usage", "\GPU Process Memory(pid_${ProcessId}_*)\Shared Usage" -ErrorAction Stop
        return [Math]::Round((($sample.CounterSamples | Measure-Object -Property CookedValue -Sum).Sum) / 1MB, 1)
    } catch {
        return $null
    }
}

# 运行一个自检：--self-test 让程序按步骤操作、截图核对，写出 selftest.json 后自己退出。通过与否看报告里的结果
# （报告写完后程序才退出；退出时出的问题只记一笔）。Settings 不为空时先写进自检目录的 settings.toml
# （程序只用这个设置文件，不碰你自己的设置）。程序运行时临时目录指到 anyview-build 里（解开的压缩包等），
# 万一程序没收拾好，清理时一起删掉。
function Invoke-Case([string]$Name, [string]$Title, [string[]]$Arguments, [string]$Settings = '', [int]$ExpectSounds = -1, [bool]$SampleGpu = $false) {
    $dir = Join-Path $runs $Name
    if (Test-Path -LiteralPath $dir) { Remove-Item -LiteralPath $dir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $dir, $appTemp | Out-Null
    if ($Settings.Length -gt 0) { Write-Utf8 (Join-Path $dir 'settings.toml') $Settings }
    $all = @('--self-test', $dir) + $Arguments
    $since = (Get-Date).AddSeconds(-2)
    $runStatus = ''
    $exitCode = $null
    $gpuFirst = $null
    $gpuLast = $null
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $savedTemp = $env:TEMP
    $savedTmp = $env:TMP
    try {
        $env:TEMP = $appTemp
        $env:TMP = $appTemp
        $argumentText = ($all | ForEach-Object { ConvertTo-Argument $_ }) -join ' '
        $process = Start-Process -FilePath $exe -ArgumentList $argumentText -WorkingDirectory $app -PassThru
        $null = $process.Handle
        $waitMs = 120000
        if ($SampleGpu) {
            # 边等边量这个进程占的显存（开关窗口那两项用）：程序刚显示出第一张图时的、结束前最后量到的。
            $deadline = [DateTime]::UtcNow.AddMilliseconds($waitMs)
            while (-not $process.WaitForExit(200) -and [DateTime]::UtcNow -lt $deadline) {
                $mb = Get-GpuMemoryMb $process.Id
                if ($null -ne $mb -and $mb -gt 0) {
                    if ($null -eq $gpuFirst) { $gpuFirst = $mb }
                    $gpuLast = $mb
                }
            }
            $waitMs = [Math]::Max(1, [int]($deadline - [DateTime]::UtcNow).TotalMilliseconds)
        }
        if (-not $process.WaitForExit($waitMs)) {
            $process.Kill()
            $runStatus = '超时（120 秒没结束，已终止）'
        } else {
            $process.WaitForExit()
            $exitCode = $process.ExitCode
        }
    } catch {
        $runStatus = "无法运行：$($_.Exception.Message)"
    } finally {
        $env:TEMP = $savedTemp
        $env:TMP = $savedTmp
    }
    # 真拖动的那一项结束以后、程序被终止或崩溃以后（0 通过、1 没通过，别的是崩溃），左键还按着就松开（兜底）。
    if ($Name -eq 'move' -or $null -eq $exitCode -or ($exitCode -ne 0 -and $exitCode -ne 1)) { Release-InjectedButton }
    $seconds = [Math]::Round($watch.Elapsed.TotalSeconds, 1)
    $target = Join-Path $results $Name
    New-Item -ItemType Directory -Force -Path $target | Out-Null
    foreach ($file in @(Get-ChildItem -LiteralPath $dir -File)) {
        if ($file.Name -ne 'settings.toml') { Copy-Item -LiteralPath $file.FullName -Destination $target }
    }
    $report = $null
    $problems = New-Object System.Collections.Generic.List[string]
    $json = Join-Path $dir 'selftest.json'
    $result = $null
    if (Test-Path -LiteralPath $json) {
        try {
            $report = (Read-Text $json) | ConvertFrom-Json
            $result = Get-Field $report 'result'
            foreach ($p in @(Get-Field $report 'problems')) {
                if ($null -ne $p) { $problems.Add([string]$p) }
            }
        } catch {
            $runStatus = "报告读不了：$($_.Exception.Message)"
        }
    }
    if ($runStatus.Length -gt 0) {
        $status = $runStatus
    } elseif ($null -eq $result) {
        $status = if ($null -ne $exitCode -and $exitCode -ne 0) { "没有写出报告（返回码 $exitCode）" } else { '没有写出报告' }
    } elseif ($result -eq 'ok') {
        $status = '正常'
        if ($null -ne $exitCode -and $exitCode -ne 0) { $problems.Add("报告已写完，程序退出时返回码 $exitCode") }
    } else {
        $status = "不通过：$result"
    }
    # 翻页音效：放了几次（自检不出声，只记下来）。
    if ($ExpectSounds -ge 0 -and $null -ne $report) {
        $sounds = @(Get-Field $report 'sounds' | Where-Object { $null -ne $_ }).Count
        if ($sounds -ne $ExpectSounds) {
            $problems.Add("翻页音效放了 $sounds 次，应为 $ExpectSounds 次")
            if ($status -eq '正常') { $status = "不通过：翻页音效放了 $sounds 次，应为 $ExpectSounds 次" }
        }
    }
    if (Test-Path -LiteralPath (Join-Path $dir 'crash.txt')) {
        # 程序里出过异常：即使报告写着通过也不算通过。
        $problems.Add('有异常记录，见 crash.txt')
        if ($status -eq '正常') { $status = '不通过：出过异常（见 crash.txt）' }
    }
    if ($status -ne '正常' -or $problems.Count -gt 0) { Save-Events $since (Join-Path $target 'events.txt') }
    $color = if ($status -eq '正常') { 'Green' } else { 'Red' }
    Write-Host "-- ${Title}：${status}（$seconds 秒）" -ForegroundColor $color
    foreach ($p in @($problems | Select-Object -First 6)) { Write-Host "     $p" -ForegroundColor Yellow }
    return [pscustomobject]@{ Name = $Name; Title = $Title; Status = $status; Seconds = $seconds; Report = $report; Problems = $problems.ToArray()
        GpuFirst = $gpuFirst; GpuLast = $gpuLast }
}

# 临时文件夹里的东西会被系统清理掉一部分：Windows 的「存储感知」等会删掉临时文件夹里一段时间没动过的文件。
# Windows 环境下 .NET SDK、NuGet 包的大部分文件就这样没了，只剩空文件夹和零星几个文件，
# 而「解压过没有」原来只看两个文件在不在。所以准备好以后记下文件数（Save-FileCount），下次先核对（Test-Complete）：
# 没有记录、或者文件比记下的少，就整个重来，不拿残缺的接着用。文件只会被删、不会无故变多，多了不算坏。
function Get-FileCount([string]$Dir) {
    if (-not (Test-Path -LiteralPath $Dir)) { return 0 }
    return @(Get-ChildItem -LiteralPath $Dir -Recurse -File -Force -ErrorAction SilentlyContinue).Count
}

function Save-FileCount([string]$Dir, [string]$Record) {
    Write-Utf8 $Record ([string](Get-FileCount $Dir))
}

function Test-Complete([string]$Dir, [string]$Record) {
    $expected = 0
    if (-not [int]::TryParse((Read-Text $Record).Trim(), [ref]$expected)) { return $false }
    return $expected -gt 0 -and (Get-FileCount $Dir) -ge $expected
}

# 把文件的修改、访问时间改成现在（每次测试都改）。tar 解出来的 SDK 文件访问时间是 1970 年、修改时间是微软打包的那天，
# NuGet 包也是打包时的时间：系统清理临时文件时把它们当成很久没用的，刚解压完就删（测完不到 10 分钟，
# SDK 的 5578 个文件只剩 2054 个）。改成现在以后，一个星期内不测才会被清掉，那时上面的核对会发现、重新准备。
function Set-FilesUsedNow([string]$Dir) {
    if (-not (Test-Path -LiteralPath $Dir)) { return }
    $now = [DateTime]::UtcNow
    foreach ($file in @(Get-ChildItem -LiteralPath $Dir -Recurse -File -Force -ErrorAction SilentlyContinue)) {
        try {
            [IO.File]::SetLastWriteTimeUtc($file.FullName, $now)
            [IO.File]::SetLastAccessTimeUtc($file.FullName, $now)
        } catch {
            # 正在被用着的文件改不了，不要紧：它反正不会被清理。
        }
    }
}

# 编译或准备失败时：写出摘要，结束。
function Stop-WithFailure([string]$What, $Step) {
    $summary.Add('')
    $summary.Add("没有完成：$What")
    if ($null -ne $Step) {
        $summary.Add("${What}：$($Step.Status)。输出的最后几行：")
        $tail = @(($Step.Output -split "`r?`n") | Where-Object { $_.Trim().Length -gt 0 } | Select-Object -Last 25)
        foreach ($line in $tail) { $summary.Add("  $line") }
    }
    Write-Utf8 (Join-Path $results 'summary.txt') (($summary -join "`r`n") + "`r`n")
    Write-Host ''
    Write-Host "没有完成：${What}。结果写在 $results" -ForegroundColor Red
    exit 1
}


Write-Host 'Anyview Windows 版：编译、自检' -ForegroundColor Cyan
Write-Host "不安装任何东西：.NET SDK、NuGet 包、编好的程序、测试图片的副本都放在 ${base}；不再需要时双击「清理.cmd」删掉。"
Write-Host '第一次运行要下载约 200 MB 的 NuGet 包；以后再运行只要几分钟。'
Write-Host '自检时图片窗口会弹出 30 多次（还有设置、选择图片、帮助窗口），每次自己操作、关掉，期间不用操作（也别点它）。'

foreach ($required in @($sdkZip, $coreDll, $bindings, $photo, (Join-Path $sharedFixtures 'sort'), (Join-Path $root 'apps\windows\Anyview\Anyview.csproj'))) {
    if (-not (Test-Path -LiteralPath $required)) {
        throw "找不到 ${required}。"
    }
}
$build = [int](Get-CimInstance Win32_OperatingSystem).BuildNumber
if ($build -lt 17763) {
    throw "Windows App SDK 1.8 需要 Windows 10 1809（内部版本 17763）或更新的系统，这台是 ${build}。"
}
New-Item -ItemType Directory -Force -Path $base, $logs, $runs | Out-Null
try {
    New-Item -ItemType Directory -Force -Path $results | Out-Null
} catch {
    $results = Join-Path $base "results\$stamp"
    New-Item -ItemType Directory -Force -Path $results | Out-Null
    Write-Host "无法写入项目报告目录，结果改存至临时目录：$results" -ForegroundColor Yellow
}

$system = Get-SystemReport
Write-Host ''
foreach ($line in $system) { Write-Host "  $line" }
Write-Utf8 (Join-Path $results 'system.txt') (($system -join "`r`n") + "`r`n")
$summary = New-Object System.Collections.Generic.List[string]
$summary.Add("Anyview Windows 版自检 $stamp")
foreach ($line in $system) { $summary.Add($line) }

# 1. .NET SDK：核对项目里带的压缩包，解压到临时目录（已经解压过、文件一个不少就直接用）。
$sdkRecord = Join-Path $base 'dotnet.files'
$sdkReady = (Test-Path -LiteralPath (Join-Path $dotnetDir "sdk\$sdkVersion\dotnet.dll")) -and (Test-Path -LiteralPath $dotnet) -and
    (Test-Complete $dotnetDir $sdkRecord)
if (-not $sdkReady) {
    if (Test-Path -LiteralPath $dotnetDir) {
        $note = "上次解压的 .NET SDK 不完整或没有核对记录（还剩 $(Get-FileCount $dotnetDir) 个文件；系统会清理临时文件夹里的旧文件），重新解压"
        Write-Host $note -ForegroundColor Yellow
        $summary.Add($note)
    }
    Write-Host ''
    Write-Host "== 准备 .NET SDK ${sdkVersion}（复制、核对 SHA-512、解压，约 1 分钟）" -ForegroundColor Cyan
    $localZip = Join-Path $base 'dotnet-sdk.zip'
    Copy-Item -LiteralPath $sdkZip -Destination $localZip -Force
    $hash = (Get-FileHash -LiteralPath $localZip -Algorithm SHA512).Hash
    if ($hash -ne $sdkSha512) {
        Remove-Item -LiteralPath $localZip -Force
        throw '.NET SDK 压缩包的 SHA-512 对不上，不用它。'
    }
    if (Test-Path -LiteralPath $dotnetDir) { Remove-Item -LiteralPath $dotnetDir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $dotnetDir | Out-Null
    $tar = Join-Path $env:SystemRoot 'System32\tar.exe'
    if (Test-Path -LiteralPath $tar) {
        $step = Invoke-Tool '解压 .NET SDK' 'sdk-extract' $tar @('-xf', $localZip, '-C', $dotnetDir) -TimeoutSeconds 900
        if ($step.Status -ne '正常') { Stop-WithFailure '解压 .NET SDK' $step }
    } else {
        Expand-Archive -LiteralPath $localZip -DestinationPath $dotnetDir -Force
    }
    Remove-Item -LiteralPath $localZip -Force
    Save-FileCount $dotnetDir $sdkRecord
}
Set-FilesUsedNow $dotnetDir

# 2. 复制源代码和生成的绑定到临时目录进行编译。
if (Test-Path -LiteralPath $src) { Remove-Item -LiteralPath $src -Recurse -Force }
Copy-Item -LiteralPath (Join-Path $root 'apps\windows') -Destination $src -Recurse
foreach ($leftover in @(Get-ChildItem -LiteralPath $src -Recurse -Directory | Where-Object { $_.Name -eq 'bin' -or $_.Name -eq 'obj' })) {
    if (Test-Path -LiteralPath $leftover.FullName) { Remove-Item -LiteralPath $leftover.FullName -Recurse -Force }
}
$localBindings = Join-Path $base 'bindings\anyview_ffi.cs'
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $localBindings) | Out-Null
Copy-Item -LiteralPath $bindings -Destination $localBindings -Force
$csproj = Join-Path $src 'Anyview\Anyview.csproj'
Write-Utf8 $nugetConfig @'
<?xml version="1.0" encoding="utf-8"?>
<configuration>
  <packageSources>
    <clear />
    <add key="nuget.org" value="https://api.nuget.org/v3/index.json" protocolVersion="3" />
  </packageSources>
</configuration>
'@

# 3. 下载 NuGet 包、编译（自包含，C# 的部分预先编译成本机代码：Native AOT）。上次下载的包不完整时（同样是被系统清理过）整个删掉重新下载：
#    NuGet 只看每个包的标记文件在不在，包里的文件少了它不知道，编译时才报找不到文件。
$packages = $buildEnvironment['NUGET_PACKAGES']
$packagesRecord = Join-Path $base 'nuget-packages.files'
$nugetDir = Join-Path $base 'nuget'
if ((Test-Path -LiteralPath $nugetDir) -and -not (Test-Complete $packages $packagesRecord)) {
    $note = "上次下载的 NuGet 包不完整或没有核对记录（还剩 $(Get-FileCount $packages) 个文件），删掉重新下载（约 200 MB）"
    Write-Host $note -ForegroundColor Yellow
    $summary.Add($note)
    foreach ($stale in @($nugetDir, $buildEnvironment['DOTNET_CLI_HOME'], $packagesRecord)) {
        if (Test-Path -LiteralPath $stale) { Remove-Item -LiteralPath $stale -Recurse -Force }
    }
}
# 预先编译成本机代码（Native AOT）最后一步要微软的链接器和 Windows SDK 的库：这台电脑上要有 Visual Studio（或它的
# “生成工具”）的“使用 C++ 的桌面开发”。没有就先说清楚，不等编译到最后才报一串看不懂的错。这一样东西项目里带不了。
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vcTools = ''
if (Test-Path -LiteralPath $vswhere) {
    $vcTools = [string](& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1)
}
if ($vcTools -eq '') {
    Stop-WithFailure '找不到 Visual Studio 的 C++ 生成工具（编译 Windows 版要用它的链接器；装 Visual Studio 生成工具时勾“使用 C++ 的桌面开发”）' $null
}
$summary.Add("C++ 生成工具：$vcTools")
Use-BuildEnvironment
$null = Invoke-Tool '.NET SDK 信息' 'dotnet-info' $dotnet @('--info') -TimeoutSeconds 120 -WorkingDirectory $src
$restore = Invoke-Tool '下载 NuGet 包（第一次较慢）' 'restore' $dotnet @('restore', $csproj, '--configfile', $nugetConfig, '-v:minimal') -TimeoutSeconds 2400 -WorkingDirectory $src
if ($restore.Status -ne '正常') { Restore-Environment; Stop-WithFailure '下载 NuGet 包' $restore }
Save-FileCount $packages $packagesRecord
Set-FilesUsedNow $packages
if (Test-Path -LiteralPath $app) { Remove-Item -LiteralPath $app -Recurse -Force }
$publish = Invoke-Tool '编译（预先编译成本机代码）' 'publish' $dotnet @('publish', $csproj, '-c', 'Release', '--no-restore', '-o', $app, '-v:minimal', '-p:UseSharedCompilation=false', "-p:AnyviewBindings=$localBindings") -TimeoutSeconds 2400 -WorkingDirectory $src
$null = Invoke-Tool '关掉编译服务' 'build-server-shutdown' $dotnet @('build-server', 'shutdown') -TimeoutSeconds 120 -WorkingDirectory $src
Restore-Environment
if ($publish.Status -ne '正常' -or -not (Test-Path -LiteralPath $exe)) { Stop-WithFailure '编译' $publish }
Copy-Item -LiteralPath $coreDll -Destination $app -Force
# 可选 HEIC 解码库 heif.dll（libheif + libde265，LGPL）：如 out\windows 中存在则一并复制到核心旁；
# 没有就不带，HEIC 仍然靠系统的解码器。
$heifDll = Join-Path $root 'out\windows\heif.dll'
$heifShipped = Test-Path -LiteralPath $heifDll
if ($heifShipped) { Copy-Item -LiteralPath $heifDll -Destination $app -Force }
# 调试符号（Anyview.pdb，二十多兆）只留在这里查问题用，不算进程序的大小，安装程序也不带它。
$files = @(Get-ChildItem -LiteralPath $app -Recurse -File | Where-Object { $_.Extension -ne '.pdb' })
$appBytes = ($files | Measure-Object -Property Length -Sum).Sum
$summary.Add("程序：$($files.Count) 个文件，共 $(Format-Size $appBytes)（C# 的部分和 .NET 运行时编成了一个 Anyview.exe；Windows App SDK、Win2D 随程序带上；核心 anyview_ffi.dll $(Format-Size (Get-Item -LiteralPath $coreDll).Length)）")

# 4. 测试图片：复制到本机（自检会改动的话也只动副本；也免得隔着网络读）。用系统自带的 robocopy 只复制有变化的文件，
#    第二次起几乎不花时间；many（一万张小图，性能测试用）这一轮用不到，不复制——隔着网络一个一个复制一万个文件要好几分钟。
Write-Host ''
Write-Host '== 复制测试图片（只复制有变化的）' -ForegroundColor Cyan
$robocopy = Join-Path $env:SystemRoot 'System32\robocopy.exe'
$manyCopy = Join-Path $fx 'many'
if (Test-Path -LiteralPath $manyCopy) { Remove-Item -LiteralPath $manyCopy -Recurse -Force }
$copyWatch = [Diagnostics.Stopwatch]::StartNew()
$copyOutput = & $robocopy $sharedFixtures $fx /MIR /XD many /FFT /R:1 /W:1 /MT:16 /NFL /NDL /NP /NJH | Out-String
$copyCode = $LASTEXITCODE
Write-Utf8 (Join-Path $results 'copy-fixtures.txt') $copyOutput
# robocopy 的返回码 0–7 都是成功（1 表示复制了文件），8 起才是出错。
if ($copyCode -ge 8) {
    Stop-WithFailure '复制测试图片' ([pscustomobject]@{ Status = "失败（robocopy 返回码 $copyCode）"; Output = $copyOutput })
}
Write-Host "-- 复制测试图片：正常，用时 $([Math]::Round($copyWatch.Elapsed.TotalSeconds, 1)) 秒" -ForegroundColor Green
$photoDir = Join-Path $base 'photo'
New-Item -ItemType Directory -Force -Path $photoDir | Out-Null
$localPhoto = Join-Path $photoDir 'photo_12mp_iguana.jpg'
if (-not (Test-Path -LiteralPath $localPhoto) -or (Get-Item -LiteralPath $localPhoto).Length -ne (Get-Item -LiteralPath $photo).Length) {
    Copy-Item -LiteralPath $photo -Destination $localPhoto -Force
}

# 真实测试图片：从项目 tests\fixtures\real 复制到临时目录并逐个校验 SHA-256。
$realShare = Join-Path $root 'tests\fixtures\real'
$realManifest = Join-Path $root 'tools\testdata\real-images.json'
$real = Join-Path $base 'real'
$realReady = $false
if (Test-Path -LiteralPath $realShare) {
    Write-Host ''
    Write-Host '== 准备真实测试图片（复制到本机、核对 SHA-256，第一次约 190 MB）' -ForegroundColor Cyan
    $realProblems = New-Object System.Collections.Generic.List[string]
    try {
        $manifest = (Read-Text $realManifest) | ConvertFrom-Json
        New-Item -ItemType Directory -Force -Path $real | Out-Null
        foreach ($image in @($manifest.images)) {
            $target = Join-Path $real $image.name
            $hash = ''
            if (Test-Path -LiteralPath $target) { $hash = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant() }
            if ($hash -ne $image.sha256) {
                Copy-Item -LiteralPath (Join-Path $realShare $image.name) -Destination $target -Force
                $hash = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant()
            }
            if ($hash -ne $image.sha256) {
                $realProblems.Add("$($image.name) 的 SHA-256 与清单不符")
                Remove-Item -LiteralPath $target -Force
            }
        }
        if (@($manifest.images).Count -eq 0) { $realProblems.Add("读不出清单 $realManifest") }
    } catch {
        $realProblems.Add($_.Exception.Message)
    }
    if ($realProblems.Count -eq 0) {
        $realReady = $true
        Write-Host '-- 真实测试图片：正常' -ForegroundColor Green
    } else {
        Write-Host "-- 真实测试图片：$($realProblems -join '；')（跳过真实图片和性能）" -ForegroundColor Red
        $summary.Add("真实测试图片没准备好，跳过真实图片和性能：$($realProblems -join '；')")
    }
} else {
    $summary.Add('未找到真实测试图片 tests\fixtures\real，跳过真实图片与性能项')
}
# 1 万个小文件的文件夹：在本机生成（隔着网络复制一万个文件要好几分钟），已有就不再生成。
$manyLocal = Join-Path $base 'many'
if ($realReady -and @(Get-ChildItem -LiteralPath $manyLocal -File -ErrorAction SilentlyContinue).Count -ne 10000) {
    if (Test-Path -LiteralPath $manyLocal) { Remove-Item -LiteralPath $manyLocal -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $manyLocal | Out-Null
    $tiny = [IO.File]::ReadAllBytes((Join-Path $fx 'sort\1.png'))
    for ($i = 0; $i -lt 10000; $i++) { [IO.File]::WriteAllBytes((Join-Path $manyLocal ('{0:00000}.png' -f $i)), $tiny) }
}

# 5. 自检（步骤与 macOS 的自检相同）。
Write-Host ''
Write-Host '== 自检（窗口会自己打开、操作、关掉，不用动）' -ForegroundColor Cyan
$cases = New-Object System.Collections.Generic.List[object]
function Add-Case([string]$Name, [string]$Title, [string[]]$Arguments, [string]$Settings = '', [int]$ExpectSounds = -1, [bool]$SampleGpu = $false) {
    if ($onlyCases.Count -gt 0 -and $onlyCases -notcontains $Name) { return }
    $cases.Add((Invoke-Case $Name $Title $Arguments $Settings $ExpectSounds $SampleGpu))
    Start-Sleep -Milliseconds 500
}
$fxs = $fx + '\'
# 浏览、压缩包这两项查的是到达首尾时"询问"的那种做法（默认是直接回到首张）：先把设置写成"询问"。
$askAtEnds = "version = 2`n[general]`nat_boundary = `"ask`"`nat_archive_boundary = `"ask`"`n"
Add-Case 'empty' '空态' @()
Add-Case 'nav' '浏览：下一张、下一张、末张、首张' @('--open', "${fxs}sort\1.png", '--steps', 'next_image,next_image,last_image,first_image')
Add-Case 'photo' '大图：1200 万像素 JPEG' @('--open', "${fxs}basic\photo_4000x3000.jpg")
$viewSteps = 'rotate_cw,flip_horizontal,rotate_none,zoom_in,zoom_in,pan_down,edge_none,edge_soften,fit_window,' +
    'next_image,fit_width,pan_down,rotate_ccw,flip_vertical,zoom_out,zoom_original,rotate_180'
Add-Case 'view' '看图操作：旋转、翻转、缩放、平移、显示方式（逐步截图核对）' @('--open', "${fxs}view\quadrants_600x400.png", '--steps', $viewSteps)
Add-Case 'overlay' '悬浮界面：锁定控制栏、图片信息、进度条跳转、透明棋盘格' @('--open', "${fxs}basic\transparent_800x600.png", '--steps', 'toggle_bar_lock,toggle_info,bar_jump:0.5,bar_jump:0.17,toggle_info,toggle_bar_lock,hover_bottom')
Add-Case 'browse' '浏览：降序、随机与退回、下一个/上一个文件夹、到达末张时的询问' @('--open', "${fxs}sort\1.png", '--steps', 'sort_desc,first_image,sort_desc,random_forward,random_back,next_container,prev_container,last_image,next_image,first_image') $askAtEnds
$qualitySteps = 'zoom_in,interp_lanczos,wait_exact,rotate_cw,wait_exact,flip_horizontal,wait_exact,' +
    'edge_soften_sharpen,wait_exact,invert,wait_exact,gamma,wait_exact,zoom_lock,next_image,wait_exact,' +
    'zoom_lock,invert,gamma,interp_bilinear,edge_soften'
Add-Case 'quality' '高品质：Lanczos 精确重绘、减淡+锐化、反相、Gamma' @('--open', "${fxs}view\quadrants_600x400.png", '--steps', $qualitySteps)
Add-Case 'color' '色彩：带 ICC 的图片转换到 sRGB' @('--open', "${fxs}color\linear_gray_128.png")
# 使用监视器的 ICC 设置：用环境变量给一个线性 Gamma 的「显示器」配置文件（与 Linux 的自测相同），打开设置后
# 灰度 128 应转成约 55，关掉后回到 128。系统「颜色管理」里真的给显示器设的那个照样取，记在每一项的报告里。
$monitorSteps = 'settings_open,settings_page:image,settings_set:image.monitor_icc=true,settings_ok,wait:400,' +
    'settings_open,settings_page:image,settings_set:image.monitor_icc=false,settings_ok,wait:400'
$env:ANYVIEW_TEST_MONITOR_ICC = "${fxs}color\linear_monitor.icc"
try {
    Add-Case 'monitor_icc' '使用监视器的 ICC 设置：按显示器的配置文件转换颜色' @('--open', "${fxs}color\gray_128.png", '--steps', $monitorSteps)
} finally {
    Remove-Item Env:ANYVIEW_TEST_MONITOR_ICC -ErrorAction SilentlyContinue
}
Add-Case 'window' '窗口跟着图片：缩放、旋转、最大化后切图、改窗口大小、全屏' @('--open', "${fxs}basic\photo_4000x3000.jpg", '--steps', 'zoom_in,rotate_cw,maximize,next_image,maximize,window:700x500,next_image,fullscreen,next_image,fullscreen')
# 拖动窗口基准测试：自动移动窗口 60 次测量每步耗时，对比两种无边框方案；
# 再用 SendInput 真的拖几次（图片、顶端那一条、普通 Win32 窗口对照、再拖图片）、拉一次顶边，核对松开后窗口不跟着光标走。
# 鼠标会自己动几秒。耗时只看摘要里的数字，不算自检结果。
Add-Case 'move' '拖动窗口：自动移动、真的用鼠标拖和拉顶边（鼠标会自己动）、松开后不跟着光标走' @('--open', "${fxs}basic\photo_4000x3000.jpg", '--steps', 'move_bench,drag_real')
Add-Case 'preview' '快速预览：先显示内嵌缩略图，再换成清晰图' @('--open', "${fxs}preview\thumbnail_3000x2000.jpg")
$inputSteps = 'key:Right,key:Left,key:End,key:Home,key:Ctrl+Up,key:Ctrl+Down,mouse:wheel_down,mouse:wheel_up,' +
    'mouse:forward,mouse:back,mouse:right,mouse:double:0.5,mouse:double:0.5,key:Enter,key:Enter,key:Escape'
Add-Case 'input' '按键和鼠标：Windows 键位、滚轮、侧键、右键菜单、双击最大化、回车全屏、Esc 退出' @('--open', "${fxs}sort\1.png", '--steps', $inputSteps)
# 压缩包：ZIP 里翻页、到末尾询问、下一个压缩包到 7Z（固实）、RAR，再打开 ZIP 回到上次那一页，从压缩包里复制到图片文件夹，
# 清掉记住的位置。解出的临时文件在 anyview-build 的 apptemp 里，换压缩包、退出时删掉。
$archiveCopies = Join-Path $base 'archive-copies'
if (Test-Path -LiteralPath $archiveCopies) { Remove-Item -LiteralPath $archiveCopies -Recurse -Force }
New-Item -ItemType Directory -Force -Path $archiveCopies | Out-Null
$archiveSteps = 'next_image,last_image,next_image,next_container,next_image,next_container,' +
    "open:${fxs}archives\a_book.cbz,settings_open,settings_page:misc,settings_set:misc.folder1=$archiveCopies,settings_ok,copy_to_folder1," +
    'settings_open,settings_page:bookmarks,settings_button:bookmarks.clear_archive:clear,settings_cancel'
Add-Case 'archive' '压缩包：ZIP、7Z、RAR 里翻页、相邻压缩包、记住位置、复制出来' @('--open', "${fxs}archives\a_book.cbz", '--steps', $archiveSteps) $askAtEnds
# 被安全上限拦下的文件：翻到解压后 65 MB 的 SVGZ 时先问（自检不弹对话框，下一步充当回答），选「仍然打开」后显示出来。
Add-Case 'limits' '安全上限：被拦下的文件询问后仍然打开' @('--open', "${fxs}limits\a.png", '--steps', 'next_image,limit_open')
# 启动时打开的就是这个文件（从资源管理器双击大文件）：窗口的内容还没装好时对话框弹不出来，要等装好了再问。
Add-Case 'limits-start' '安全上限：启动时就打开被拦下的文件，也要询问' @('--open', "${fxs}limits\b_65mb.svgz", '--steps', 'limit_open')
# 翻页效果：停在一半时按效果核对中间画面，再照常做完一次（等效果结束后核对画面）。
$fadeSettings = "[image]`r`npage_effect = `"fade`"`r`n[misc]`r`npage_sound = true`r`n"
Add-Case 'effects' '翻页效果：淡出的中间画面、翻页音效（自检不出声）' @('--open', "${fxs}effects\1_red.png", '--steps', 'transition_hold:0.5,next_image,transition_hold:off,next_image,prev_image') $fadeSettings 3
$slideSettings = "[image]`r`npage_effect = `"slide`"`r`n"
Add-Case 'effects-slide' '翻页效果：滑动的中间画面' @('--open', "${fxs}effects\1_red.png", '--steps', 'transition_hold:0.5,next_image,transition_hold:off,next_image') $slideSettings 0
$pageSettings = "[image]`r`npage_effect = `"page`"`r`n"
Add-Case 'effects-page' '翻页效果：翻书的中间画面（往后、往前）' @('--open', "${fxs}effects\1_red.png", '--steps', 'transition_hold:0.5,next_image,transition_hold:off,transition_hold:0.5,prev_image,transition_hold:off,next_image') $pageSettings 0
$next = (@(1..15) | ForEach-Object { 'next_image' }) -join ','
$next8 = (@(1..8) | ForEach-Object { 'next_image' }) -join ','
# modern\ 有 19 张：打开第一张后再翻 18 次。
$next18 = (@(1..18) | ForEach-Object { 'next_image' }) -join ','
$formatSteps = "$next,open:${fxs}modern\avif_anim.avif,$next18,open:${fxs}raw\a_tiff.nef,$next8"
Add-Case 'formats' '各种格式：DDS、PCX、PSD、AVIF、EXR、Radiance HDR、HEIC、JPEG 2000、JPEG XL、JPEG XR、CLIP、SVG、RAW' @('--open', "${fxs}formats\dds_bc7_dx10.dds", '--steps', $formatSteps)
# 生成信息：ComfyUI 的 PNG 上按 P 打开面板，往后翻到 WebP、JPEG（EXIF 里的 A1111 参数），再到没有生成信息的 plain.png。
$generationSteps = 'toggle_info,key:P,generation:state,next_image,generation:state,next_image,generation:state,' +
    'next_image,next_image,next_image,generation:state,generation:close'
Add-Case 'generation' '生成信息：按 P 打开面板，ComfyUI、A1111（PNG、WebP、JPEG）的提示词' @('--open', "${fxs}generation\comfy.png", '--steps', $generationSteps)
Add-Case 'pages' '双页阅览：两页一组、横图单独、往回翻、左←右' @('--open', "${fxs}pages\01.png", '--steps', 'key:7,next_image,next_image,prev_image,prev_image,key:6,key:6')
# 动图：GIF 播放、空格暂停、逐帧前后、APNG、GIF 透明处的背景色（文件里的品红，再改成白色）、隐藏动画信息控制（与 macOS 相同）。
$animSteps = 'wait_frames:3,key:Space,wait:300,key:Period,key:Comma,next_image,wait_frames:2,next_image,key:Space,' +
    'settings_open,settings_page:image,settings_set:image.gif_background=white,settings_ok,' +
    'settings_open,settings_page:browse,settings_set:browse.hide_animation_info=true,settings_ok'
Add-Case 'anim' '动图：播放、暂停、逐帧、APNG、GIF 透明背景、隐藏动画信息控制' @('--open', "${fxs}anim\a_frames.gif", '--steps', $animSteps)
# 设置（docs/SETTINGS.md）：逐页截图；改背景色、隐藏控制栏和倍率提示、透明背景、Esc、语言，缓存大小填错时"确定"拦下；
# 生效后核对画面、按键和设置文件；再打开时放弃修改。设置文件在自检的目录里，不碰你自己的设置。
$settingsSteps = 'settings_open,settings_page:browse,settings_set:browse.background_color=#336699,' +
    'settings_set:browse.hide_control_bar=true,settings_set:browse.hide_zoom_info=true,' +
    'settings_page:image,settings_set:image.cache_mb=50,settings_set:image.alpha_background=white,' +
    'settings_ok_invalid,settings_set:image.cache_mb=512,settings_page:general,' +
    'settings_set:general.esc_quits=false,settings_page:language,settings_set:language.language=en,' +
    'settings_ok,zoom_out,hover_bottom,key:Escape,settings_open,settings_page:mouse,' +
    'settings_set:mouse.double_click_maximize=false,settings_cancel,settings_open,settings_page:misc,' +
    'settings_page:formats,settings_page:keyboard,settings_page:bookmarks,settings_cancel'
Add-Case 'settings' '设置窗口：逐页截图、改设置、确定生效、填错时拦下、放弃修改' @('--open', "${fxs}basic\transparent_800x600.png", '--steps', $settingsSteps)
$settingsFile = Join-Path $runs 'settings\settings.toml'
Add-Case 'settings_reload' '设置：重新启动后设置还在' @('--open', "${fxs}basic\transparent_800x600.png", '--settings', $settingsFile, '--steps', 'settings_open,settings_cancel,zoom_out')
# 键盘、鼠标映射：关掉 PageDown 那一行，Q 映射为放大、→ 映射为缩小，滚轮向下放大、右键下一张、单击两侧翻页；
# 确定后逐个按键、点鼠标，核对执行的动作。
$mappingSteps = 'settings_open,settings_page:keyboard,settings_set:keyboard.mappings#0=off,' +
    'settings_set:keyboard.mappings#2=on|Q|zoom_in,settings_set:keyboard.mappings#3=on|Right|zoom_out,' +
    'settings_page:mouse,settings_set:mouse.wheel_down=on|zoom_in,settings_set:mouse.right=on|next_image,' +
    'settings_set:mouse.click_sides=true,settings_ok,key:Q,key:Right,key:PageDown,mouse:wheel_down,' +
    'mouse:right,mouse:left:0.1,mouse:double:0.9,mouse:double:0.5'
Add-Case 'mapping' '键盘、鼠标映射：设好后逐个按键、点鼠标核对' @('--open', "${fxs}view\quadrants_600x400.png", '--steps', $mappingSteps)
# 选择图片窗口：打开时选中当前图片、缩略图带生成缩略图；单击别的行主窗口不切换；按大小排序后选择不变；
# 隐藏、再显示缩略图导航；确定后切过去；再打开、取消不切换；压缩包里也能用。
$pickerSteps = 'picker,picker_select:4,picker_sort:sort_size,picker_hide:on,picker_hide:off,picker_ok,' +
    "picker,picker_select:0,picker_cancel,open:${fxs}archives\a_book.cbz,picker,picker_select:2,picker_ok"
Add-Case 'picker' '选择图片窗口：缩略图、列表、排序、隐藏缩略图、确定、取消、压缩包' @('--open', "${fxs}sort\2.png", '--steps', $pickerSteps)
# 背景图片、标题栏隐藏时显示文件名、帮助窗口：设好后窗口四周（和关掉图片后的空态）是背景图片，顶端显示文件名；
# 打开"键盘快捷键""支持的格式"两个帮助窗口；关掉背景图片；最后打开"开源许可"（放在最后，前面各步的序号不变）。
$backgroundSteps = 'settings_open,settings_page:browse,settings_set:browse.background_image=true,' +
    "settings_set:browse.background_image_path=${fxs}effects\2_blue.png," +
    'settings_set:browse.title_filename=true,settings_ok,wait:600,close_image,help:shortcuts,help:formats,' +
    'settings_open,settings_page:browse,settings_set:browse.background_image=false,settings_ok,help:licenses,about_licenses'
Add-Case 'background' '背景图片、标题栏隐藏时显示文件名、帮助窗口' @('--open', "${fxs}sort\1.png", '--steps', $backgroundSteps)
# 文件操作：在临时副本上做。编辑器、桌面图片、打印只记下交出的内容，不真的打开程序、不改你的桌面、不打印。
$fileops = Join-Path $base 'fileops'
if (Test-Path -LiteralPath $fileops) { Remove-Item -LiteralPath $fileops -Recurse -Force }
$fileopsWork = Join-Path $fileops 'work'
$fileopsF1 = Join-Path $fileops 'f1'
$fileopsF2 = Join-Path $fileops 'f2'
New-Item -ItemType Directory -Force -Path $fileopsWork, $fileopsF1, $fileopsF2 | Out-Null
foreach ($name in @('1.png', '2.png', '10.png')) { Copy-Item -LiteralPath (Join-Path $fx "sort\$name") -Destination $fileopsWork }
$editor = Join-Path $env:SystemRoot 'System32\notepad.exe'
$fileopsSteps = "settings_open,settings_page:misc,settings_set:misc.folder1=$fileopsF1," +
    "settings_set:misc.folder2=$fileopsF2,settings_set:misc.editor=$editor," +
    'settings_set:misc.no_screensaver=true,settings_ok,copy_to_folder1,copy_to_folder1,' +
    'copy_to_folder2,open_in_editor,set_wallpaper,print,move_to_folder1,' +
    "add_file:${fxs}sort\中文.png,remove_file:10.png"
Add-Case 'fileops' '文件操作：复制、移动、编辑器、桌面图片、打印（只记录）、文件夹变化自动刷新' @('--open', (Join-Path $fileopsWork '1.png'), '--steps', $fileopsSteps)
# 窗口：从资源管理器再打开时开新窗口、改设置两个窗口都变、Esc 只关第二个窗口；勾上"不允许多个实例"后在原窗口换图；
# 吸附边缘关着时不动、打开后拖到边缘附近贴上去（也模拟在画面上拖一次）；显示标题栏后窗口按图片加标题栏；
# 最后一个窗口里按 Esc 退出（自检记下后结束）。
$windowsSteps = "finder_open:${fxs}sort\1.png," +
    'settings_open,settings_page:browse,settings_set:browse.background_color=#336699,settings_ok,' +
    'key_in:1:Escape,' +
    'settings_open,settings_page:general,settings_set:general.single_instance=true,settings_ok,' +
    "finder_open:${fxs}sort\2.png,move_window:7:stay," +
    'settings_open,settings_page:general,settings_set:general.snap_window=true,' +
    'settings_page:browse,settings_set:browse.show_main_window=true,settings_ok,' +
    'next_image,move_window:7:snap,move_window:40:stay,drag_window:9:snap,key:Escape'
Add-Case 'windows' '窗口：从资源管理器打开开新窗口、复用当前窗口、吸附边缘、显示标题栏、Esc' @('--open', "${fxs}view\quadrants_600x400.png", '--steps', $windowsSteps)
# 菜单栏：设置里把「不显示菜单栏」关掉以后，窗口顶端多一行菜单（内容与 macOS 的菜单栏相同），图片区在它下面、
# 窗口跟着图片时多出这一行；用菜单翻页、开关图片信息；「打开最近使用」里记着打开过的；全屏时不显示；再勾上就没有了。
$menuSettings = "version = 2`n[browse]`nhide_menu_bar = false`n"
$menuSteps = "menu_bar:next_image,menu_bar:toggle_info,open:${fxs}view\quadrants_600x400.png,menu_recent:1,menu_bar_describe," +
    'fullscreen,fullscreen,settings_open,settings_page:browse,settings_set:browse.hide_menu_bar=true,settings_ok'
Add-Case 'menubar' '菜单栏：显示、用菜单操作、打开最近使用、全屏时不显示、关掉' @('--open', "${fxs}sort\1.png", '--steps', $menuSteps) $menuSettings
# 窗口看不见时：打开「看图时阻止屏幕保护程序和显示器睡眠」后最小化就不阻止，取回后再阻止；全屏的窗口也一样。
$visibleSteps = 'settings_open,settings_page:misc,settings_set:misc.no_screensaver=true,settings_ok,minimize_restore,fullscreen,minimize_restore,fullscreen'
Add-Case 'visible' '窗口看不见时：最小化就不阻止显示器睡眠，取回后再阻止' @('--open', "${fxs}view\quadrants_600x400.png", '--steps', $visibleSteps)
# 开关窗口：关掉的窗口，WinUI 不回收它的对象，窗口拿着的大东西要程序自己放掉。开一个新窗口、显示出来就关，
# 30 次用 1200 万像素的照片（窗口大，看显存），再 10 次用 800x600 的图（上传用的数组只留 16 MB 以内的，看它放没放）。
# 对照：关窗口时的收尾一步都不做（ANYVIEW_TEST_LEAK=all），必须量得出漏——量不出就是这项检查不管用了。
# 另开关 10 个什么都不放的空窗口，只记录 WinUI 自己回收了几个（哪天框架改了，这里看得出来）。
# 显存由这个脚本在外面量（Invoke-Case 的 SampleGpu）：开头停 1.5 秒好量到起点，最后停 1.5 秒好量到终点。
$churnSteps = "wait:1500,churn:30:${fxs}basic\photo_4000x3000.jpg,wait:1500,churn:10:${fxs}basic\transparent_800x600.png,churn_bare:10"
Add-Case 'churn' '开关窗口 40 次：关掉的窗口占的显存、内存有没有放掉' @('--open', "${fxs}view\quadrants_600x400.png", '--steps', $churnSteps) '' -1 $true
$env:ANYVIEW_TEST_LEAK = 'all'
try {
    Add-Case 'churn-keep' '开关窗口的对照：关窗口时不做收尾（应当量得出漏）' @('--open', "${fxs}view\quadrants_600x400.png", '--steps', $churnSteps) '' -1 $true
} finally {
    Remove-Item Env:ANYVIEW_TEST_LEAK -ErrorAction SilentlyContinue
}
# 系统集成有关的设置：上下文菜单页、格式关联的两个按钮，只有用安装包装的程序才有、才能用。这里的程序
# 没有登记到系统，用环境变量让它当自己装了（ANYVIEW_TEST_INSTALLED=1），只查设置窗口这一层；
# 真的登记到系统以后的检查在 package.ps1 -Action test。
$installedSteps = 'settings_open,settings_page:context_menu,settings_set:context_menu.enabled=true,settings_set:context_menu.thumb_psd=false,settings_page:formats,settings_button:formats.buttons_windows:associate,settings_button:formats.buttons_windows:classic,settings_ok'
$env:ANYVIEW_TEST_INSTALLED = '1'
try {
    Add-Case 'installed' '当作装好的程序：上下文菜单页、格式关联的两个按钮' @('--open', "${fxs}sort\1.png", '--steps', $installedSteps)
} finally {
    Remove-Item Env:ANYVIEW_TEST_INSTALLED -ErrorAction SilentlyContinue
}
Add-Case 'not-installed' '没装的程序：没有上下文菜单页，格式关联的按钮不起作用' @('--open', "${fxs}sort\1.png", '--steps', 'settings_open,settings_page:formats,settings_button:formats.buttons_windows:associate,settings_cancel')
# 设置文件坏了：备份、用默认值并提示。
$brokenSettings = Join-Path $base 'broken-settings'
if (Test-Path -LiteralPath $brokenSettings) { Remove-Item -LiteralPath $brokenSettings -Recurse -Force }
New-Item -ItemType Directory -Force -Path $brokenSettings | Out-Null
Write-Utf8 (Join-Path $brokenSettings 'settings.toml') "version = 1`n[general`nconfirm_delete = false`n"
Add-Case 'settings_broken' '设置：设置文件损坏时备份、用默认值并提示' @('--open', "${fxs}basic\transparent_800x600.png", '--settings', (Join-Path $brokenSettings 'settings.toml'), '--steps', 'settings_open,settings_cancel')
# 文件：复制到剪贴板、删除（与 macOS 相同）。在临时副本上做：自检时删除不进回收站、直接删掉副本，剪贴板只核对要放进去的内容。
$filesWork = Join-Path $base 'files-work'
if (Test-Path -LiteralPath $filesWork) { Remove-Item -LiteralPath $filesWork -Recurse -Force }
New-Item -ItemType Directory -Force -Path $filesWork | Out-Null
foreach ($name in @('1.png', '2.png', '10.png')) { Copy-Item -LiteralPath (Join-Path $fx "sort\$name") -Destination $filesWork }
Add-Case 'files' '文件：复制到剪贴板、删除（用临时副本）' @('--open', (Join-Path $filesWork '2.png'), '--steps', 'copy_to_clipboard,delete_file,next_image,first_image,delete_file') $askAtEnds
# 当 tests\fixtures\real 存在时执行真实图片与性能测试。
if ($realReady) {
    $realSteps = 'zoom_100,zoom_lock,toggle_info' + (',next_image' * 9)
    Add-Case 'real' '真实图片：10 张开放许可的照片，逐张以 100% 打开核对' @('--open', (Join-Path $real 'gif_ozone.gif'), '--sample-at-100', '--steps', $realSteps)
    # 性能：10 份相同的 1200 万像素照片（a–j）、另一个文件夹里的 x，以及 1 万个小文件的文件夹。
    # 静止量 6 秒（macOS、Linux 是 3 秒）：Windows 给进程记 CPU 时间是一格一格记的（约 15.6 毫秒一格），3 秒里一格就是 0.52%、
    # 两格 1.04%——界面线程、.NET 后台编译各赶上一格就压在 1% 的线上，同一份程序时过时不过。6 秒里一格 0.26%。
    $perf = Join-Path $base 'perf'
    if (Test-Path -LiteralPath $perf) { Remove-Item -LiteralPath $perf -Recurse -Force }
    New-Item -ItemType Directory -Force -Path (Join-Path $perf 'work'), (Join-Path $perf 'other') | Out-Null
    # 10 份：预读默认往后 5 张，跳到末张时它不在预读范围里，测得的是未缓存切图耗时。
    foreach ($name in @('a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j')) { Copy-Item -LiteralPath $localPhoto -Destination (Join-Path $perf "work\$name.jpg") }
    Copy-Item -LiteralPath $localPhoto -Destination (Join-Path $perf 'other\x.jpg')
    $perfSteps = 'wait:1500,bar_jump:1.0,wait:1500,prev_image,interp_lanczos,wait_exact,interp_bilinear,idle:6000,' +
        "open:$(Join-Path $perf 'other\x.jpg'),open:$manyLocal"
    Add-Case 'perf' '性能：切图、精确重绘、空闲占用、1 万张的文件夹' @('--open', (Join-Path $perf 'work\a.jpg'), '--sample-at-100', '--steps', $perfSteps)
}

if ($onlyCases.Count -gt 0) {
    $summary.Add('')
    $summary.Add("只跑了这几项（ANYVIEW_TEST_ONLY=$($onlyCases -join ',')），没有按报告核对、没有量冷启动：")
    foreach ($c in $cases) {
        $summary.Add("  $($c.Title)：$($c.Status)（$($c.Seconds) 秒）")
        foreach ($problem in @($c.Problems | Select-Object -First 8)) { $summary.Add("      $problem") }
        # 量了显存的项（开关窗口）：开头、结尾各占多少。
        if ($null -ne $c.GpuFirst -and $null -ne $c.GpuLast) { $summary.Add("      显存 $($c.GpuFirst) → $($c.GpuLast) MB") }
    }
    Write-Utf8 (Join-Path $results 'summary.txt') (($summary -join "`r`n") + "`r`n")
    Write-Host ''
    foreach ($line in $summary) { Write-Host "  $line" }
    Write-Host "结果已写到 $results" -ForegroundColor Green
    exit [int](@($cases | Where-Object { $_.Status -ne '正常' }).Count -gt 0)
}

# 5b. 按报告核对各项的结果（与 macOS 的一键测试相同的核对）。
$checks = New-Object System.Collections.Generic.List[string]
$checkFailures = 0
function Get-CaseReport([string]$Name) {
    foreach ($c in $cases) { if ($c.Name -eq $Name) { return $c.Report } }
    return $null
}
function Add-Check([string]$Title, $Problems, [string[]]$Good) {
    $list = @($Problems | Where-Object { $null -ne $_ -and $_ -ne '' })
    if ($list.Count -eq 0) {
        $checks.Add("  ✓ ${Title}：$($Good -join '')")
        return 0
    }
    $checks.Add("  ✗ ${Title}：$($list -join '；')")
    return 1
}

$report = Get-CaseReport 'settings'
$p = New-Object System.Collections.Generic.List[string]
if ((Get-Value $report 'steps.9.errorPage') -ne 'image') { $p.Add("缓存大小填 50 时没有拦下（$(Get-Value $report 'steps.9.error')）") }
if ((Get-Value $report 'steps.15.uiSettings.backgroundColor') -ne '#336699') { $p.Add('背景色没有生效') }
if ((Get-Value $report 'steps.15.uiSettings.hideControlBar') -ne $true) { $p.Add('不显示控制栏没有生效') }
if ((Get-Value $report 'steps.15.uiSettings.hideZoomInfo') -ne $true) { $p.Add('不显示倍率没有生效') }
if ((Get-Value $report 'steps.15.uiSettings.alphaBackground') -ne 'White') { $p.Add('白色透明背景没有生效') }
if ((Get-Value $report 'steps.15.lang') -ne 'En') { $p.Add('语言没有换成英文') }
$settingsText = [string](Get-Value $report 'steps.15.settingsText')
foreach ($needle in @('background_color = "#336699"', 'esc_quits = false', 'cache_mb = 512', 'language = "en"')) {
    if (-not $settingsText.Contains($needle)) { $p.Add("设置文件里没有 $needle") }
}
$corner = Get-Value $report 'steps.16.cornerColor'
if ($corner -ne '#336699') { $p.Add("缩小后背景色是 $corner，应为 #336699") }
if ((Get-Value $report 'steps.17.overlay.barShown') -ne $false) { $p.Add('设置了不显示控制栏，指针移到底部时还是显示了') }
$escapeAction = Get-Value $report 'steps.18.action'
if ($null -ne $escapeAction -and $escapeAction -ne '') { $p.Add("关掉「按 ESC 退出」后 Esc 还对应 $escapeAction") }
if ((Get-Value $report 'steps.19.settingsPages.0') -ne 'general General') { $p.Add('再打开设置时不是英文') }
if ((Get-Value $report 'steps.22.dirtyBeforeCancel') -ne $true) { $p.Add('放弃修改前草稿没有改动') }
$reload = Get-CaseReport 'settings_reload'
if ((Get-Value $reload 'steps.1.uiSettings.backgroundColor') -ne '#336699' -or (Get-Value $reload 'steps.1.lang') -ne 'En') { $p.Add('重新启动后设置没有保留') }
if ((Get-Value $reload 'steps.3.cornerColor') -ne '#336699') { $p.Add('重新启动后背景色不对') }
$brokenBackup = @(Get-ChildItem -LiteralPath $brokenSettings -File | Where-Object { $_.Name -like 'settings-broken-*' } | Select-Object -First 1)
$brokenNotice = [string](Get-Value (Get-CaseReport 'settings_broken') 'steps.0.notice')
if ($brokenBackup.Count -eq 0 -or -not $brokenNotice.Contains('settings-broken-')) { $p.Add("损坏的设置文件没有备份或没有提示（$brokenNotice）") }
$checkFailures += Add-Check '设置' $p @('缓存大小填 50 被拦下；背景色、隐藏控制栏和倍率、白色透明背景、Esc 不退出、英文都生效并写进文件；',
    '重新启动后保留；损坏的文件已备份并提示。设置窗口的截图在 settings\step-*-settings.png')

$report = Get-CaseReport 'mapping'
$p = New-Object System.Collections.Generic.List[string]
foreach ($expect in @(@(11, 'zoom_in', ''), @(12, 'zoom_out', ''), @(13, '', ''), @(14, 'zoom_in', ''), @(15, 'next_image', 'tall_400x3000.png'),
        @(16, 'prev_image', 'quadrants_600x400.png'), @(17, '', ''), @(18, 'maximize', ''))) {
    $action = [string](Get-Value $report "steps.$($expect[0]).action")
    $name = [string](Get-Value $report "steps.$($expect[0]).name")
    if ($action -ne $expect[1] -or ($expect[2] -ne '' -and $name -ne $expect[2])) {
        $p.Add("第 $($expect[0]) 步动作 $action、图片 $name（应为 $($expect[1]) $($expect[2])）")
    }
}
$rows = (@(Get-Value $report 'steps.5.settings.page.items.0.value') -join ' ')
if ($rows -notmatch '将替代|Replaces') { $p.Add('→ 那一行没有提示将替代下一张') }
$checkFailures += Add-Check '映射' $p @('Q 放大、→ 缩小（提示将替代下一张）、关掉的 PageDown 不再翻文件夹、滚轮向下放大、右键下一张、',
    '单击左侧翻回上一张、两侧双击不最大化、中间双击最大化')

$report = Get-CaseReport 'archive'
$p = New-Object System.Collections.Generic.List[string]
$names = (@(0, 1, 2, 4, 5, 6, 7) | ForEach-Object { [string](Get-Value $report "steps.$_.name") }) -join '|'
$wantNames = 'a_book.cbz › 1.png|a_book.cbz › 2.png|a_book.cbz › 3.png|b_book.cb7 › 1.png|b_book.cb7 › 2.png|c_book.cbr › 1.png|a_book.cbz › 3.png'
if ($names -ne $wantNames) { $p.Add("翻页、换压缩包的顺序不对：$names") }
if ((Get-Value $report 'steps.3.boundary') -ne 'last') { $p.Add('ZIP 末尾没有询问') }
if ((Get-Value $report 'steps.0.deleteEnabled') -ne $false) { $p.Add('压缩包里的图片能删除') }
if (-not (Test-Path -LiteralPath (Join-Path $archiveCopies '3.png'))) { $p.Add('没有从压缩包里复制出来') }
if ((Get-Value $report 'steps.15.archivePositions') -ne 0) { $p.Add('没有清掉记住的位置') }
$checkFailures += Add-Check '压缩包' $p @('ZIP 里翻页、末尾询问、下一个压缩包到 7Z（固实）、RAR，再打开 ZIP 回到上次的第 3 页；',
    '压缩包里不能删除；复制出的 3.png 在图片文件夹里；清掉了记住的位置')

# 安全上限：步骤 1 是翻到那个 SVGZ（被拦下），步骤 2 是「仍然打开」。
$report = Get-CaseReport 'limits'
$p = New-Object System.Collections.Generic.List[string]
if ((Get-Value $report 'steps.1.blocked') -ne $true) { $p.Add('解压后 65 MB 的 SVGZ 没有被拦下') }
if ((Get-Value $report 'steps.2.name') -ne 'b_65mb.svgz' -or (Get-Value $report 'steps.2.width') -ne 64) {
    $p.Add("选仍然打开后显示的是 $(Get-Value $report 'steps.2.name')，宽 $(Get-Value $report 'steps.2.width')（应为 b_65mb.svgz，宽 64）")
}
# 对话框是真的弹出来的（自检在它弹出以后才记这一步）：两个按钮，默认按钮（回车）是取消。
if (@(Get-Value $report 'steps.1.dialog.buttons').Count -ne 2 -or (Get-Value $report 'steps.1.dialog.default') -ne 'Close') {
    $p.Add("询问的对话框不对：按钮 $(@(Get-Value $report 'steps.1.dialog.buttons') -join '、')，默认 $(Get-Value $report 'steps.1.dialog.default')（应为两个按钮、默认是取消）")
}
# 启动时就打开它：步骤 0（打开）被拦下、弹出询问，步骤 1 是「仍然打开」。
$start = Get-CaseReport 'limits-start'
if ((Get-Value $start 'steps.0.blocked') -ne $true) { $p.Add('启动时就打开那个 SVGZ：没有弹出询问') }
if ((Get-Value $start 'steps.1.name') -ne 'b_65mb.svgz' -or (Get-Value $start 'steps.1.width') -ne 64) {
    $p.Add("启动时就打开那个 SVGZ：选仍然打开后显示的是 $(Get-Value $start 'steps.1.name')，宽 $(Get-Value $start 'steps.1.width')")
}
$checkFailures += Add-Check '安全上限' $p @('解压后 65 MB 的 SVGZ 先被拦下、弹出询问（默认按钮是取消），选「仍然打开」后显示出来（64x48）；启动时就打开它也一样')

$report = Get-CaseReport 'picker'
$p = New-Object System.Collections.Generic.List[string]
if ((Get-Value $report 'steps.1.picker.rows') -ne 9 -or (Get-Value $report 'steps.1.picker.selected') -ne 1) { $p.Add('打开时没有选中当前图片') }
$ready = Get-Value $report 'steps.1.picker.thumbnailsReady'
$visible = Get-Value $report 'steps.1.picker.thumbnailsVisible'
if ($null -eq $visible -or $visible -eq 0 -or $ready -ne $visible) { $p.Add("缩略图没有生成：$ready/$visible") }
if ((Get-Value $report 'steps.2.picker.selectedName') -ne 'sort/B.png' -or (Get-Value $report 'steps.2.index') -ne 1) { $p.Add('单击后主窗口不该切换') }
$sortLabel = [string](Get-Value $report 'steps.3.picker.sortLabel')
if (($sortLabel -ne '按大小排序' -and $sortLabel -ne 'Sort by Size') -or (Get-Value $report 'steps.3.picker.selectedName') -ne 'sort/B.png') { $p.Add("排序后不对：$sortLabel") }
if ((Get-Value $report 'steps.4.picker.stripVisible') -ne $false -or (Get-Value $report 'steps.4.picker.hideSetting') -ne $true) { $p.Add('隐藏缩略图导航没有生效') }
if ((Get-Value $report 'steps.5.picker.stripVisible') -ne $true) { $p.Add('再显示缩略图导航没有生效') }
if ((Get-Value $report 'steps.6.name') -ne 'B.png' -or $null -ne (Get-Value $report 'steps.6.picker')) { $p.Add('确定后没有切到 B.png 并关掉窗口') }
if ((Get-Value $report 'steps.7.picker.selectedName') -ne 'sort/B.png') { $p.Add('再打开时没有选中当前图片') }
if ((Get-Value $report 'steps.9.name') -ne 'B.png' -or $null -ne (Get-Value $report 'steps.9.picker')) { $p.Add('取消后不该切换') }
if ((Get-Value $report 'steps.11.picker.rows') -ne 3) { $p.Add('压缩包里的列表不对') }
$chosen = [string](Get-Value $report 'steps.12.picker.selectedName')
$chosen = $chosen.Substring($chosen.LastIndexOf('/') + 1)
if ((Get-Value $report 'steps.13.name') -ne "a_book.cbz › $chosen") { $p.Add('压缩包里确定后不对') }
$checkFailures += Add-Check '选择图片窗口' $p @("打开时选中当前图片，缩略图 $ready 张已生成；单击只移动选择；按大小排序后选择不变；",
    "隐藏、显示缩略图导航；确定切到 B.png；取消不切换；压缩包里切到 $chosen")

$report = Get-CaseReport 'background'
$p = New-Object System.Collections.Generic.List[string]
if ((Get-Value $report 'steps.7.cornerColor') -ne '#2828C8' -or (Get-Value $report 'steps.7.background') -ne $true) { $p.Add("设好背景图片后四周不是它（$(Get-Value $report 'steps.7.cornerColor')）") }
if ((Get-Value $report 'steps.7.filename') -ne '1.png') { $p.Add('标题栏隐藏时没有显示文件名') }
if ((Get-Value $report 'steps.8.cornerColor') -ne '#2828C8') { $p.Add('空态的背景不是背景图片') }
$helpTitle = [string](Get-Value $report 'steps.9.help.title')
$helpRows = Get-Value $report 'steps.9.help.rows'
if (($helpTitle -ne '键盘快捷键' -and $helpTitle -ne 'Keyboard Shortcuts') -or $null -eq $helpRows -or $helpRows -lt 20) { $p.Add("键盘快捷键窗口不对（$helpTitle，$helpRows 行）") }
if ((Get-Value $report 'steps.10.help.headings') -ne 5) { $p.Add('支持的格式窗口不对') }
if ((Get-Value $report 'steps.14.cornerColor') -ne '#202020' -or (Get-Value $report 'steps.14.background') -ne $false) { $p.Add('关掉背景图片后四周不是背景色') }
$licenseTitle = [string](Get-Value $report 'steps.15.help.title')
$licenseLength = Get-Value $report 'steps.15.help.textLength'
if (($licenseTitle -ne '开源许可' -and $licenseTitle -ne 'Open Source Licenses') -or $null -eq $licenseLength -or $licenseLength -lt 200000 -or (Get-Value $report 'steps.15.help.anyviewLicense') -ne $true) {
    $p.Add("开源许可窗口不对（$licenseTitle，$licenseLength 字）")
}
# “关于”：程序名、版本下面一行写许可证，按钮是“开源许可”“确定”；按“开源许可”打开开源许可窗口。
$aboutMessage = [string](Get-Value $report 'steps.16.about.message')
$aboutButtons = @(Get-Value $report 'steps.16.about.buttons')
if ($aboutMessage -notlike 'Anyview *MIT*' -or $aboutButtons.Count -ne 2 -or [string](Get-Value $report 'steps.16.help.title') -ne [string]$aboutButtons[0] -or
    (Get-Value $report 'steps.16.help.anyviewLicense') -ne $true) {
    $p.Add("关于对话框里的开源许可不对（$aboutMessage；$($aboutButtons -join '、')）")
}
$checkFailures += Add-Check '背景图片等' $p @('图片四周和空态都是背景图片，关掉后回到背景色；标题栏隐藏时顶端显示文件名；',
    "帮助窗口：键盘快捷键 $helpRows 行、支持的格式 5 类、开源许可 $([int]($licenseLength / 1024)) K 字")

$report = Get-CaseReport 'fileops'
$p = New-Object System.Collections.Generic.List[string]
[string[]]$f1Names = @(Get-ChildItem -LiteralPath $fileopsF1 -File | ForEach-Object { $_.Name })
[Array]::Sort($f1Names, [StringComparer]::Ordinal)
$f1 = $f1Names -join '|'
$f2 = (@(Get-ChildItem -LiteralPath $fileopsF2 -File | ForEach-Object { $_.Name }) -join '|')
if ($f1 -ne '1 (2).png|1 (3).png|1.png' -or $f2 -ne '1.png') { $p.Add("复制、移动的结果不对：f1=$f1 f2=$f2") }
$externals = (@(11, 12, 13) | ForEach-Object { [string](Get-Value $report "steps.$_.external.kind") }) -join ' '
if ($externals -ne 'editor wallpaper print') { $p.Add("编辑器、桌面图片、打印没有交出：$externals") }
if ((Get-Value $report 'steps.14.name') -ne '2.png') { $p.Add('移动后没有显示下一张') }
if ((Get-Value $report 'steps.15.total') -ne 3 -or (Get-Value $report 'steps.16.total') -ne 2) { $p.Add('文件夹变化没有自动刷新') }
if ((Get-Value $report 'steps.8.preventingSleep') -ne $true) { $p.Add('打开设置后没有阻止显示器睡眠') }
# 打印只记下来、不弹对话框；对话框之前「取默认打印机的设置、按图片改纸张方向」那一步照做（横图 ok:2、竖图 ok:1；没有打印机时 none）。
$printDefaults = [string](Get-Value $report 'steps.13.external.printDefaults')
if ($printDefaults -ne 'ok:2 ok:1' -and $printDefaults -ne 'none none') { $p.Add("打印前取默认打印机的设置、改纸张方向不对：${printDefaults}（应为 ok:2 ok:1）") }
$printText = if ($printDefaults -eq 'none none') { '没有默认打印机，纸张方向没法预先改' } else { '打印前横图改成横向纸、竖图竖向' }
$checkFailures += Add-Check '文件操作' $p @('复制两次（第二次是「1 (2).png」）、复制到图片文件夹 2、移动后显示下一张；编辑器、桌面图片、打印已交出；',
    "加一张、删一张后自动刷新；看图时阻止显示器睡眠；$printText")

$report = Get-CaseReport 'windows'
$p = New-Object System.Collections.Generic.List[string]
foreach ($expect in @(
        @(1, 'how', 'new', '从资源管理器打开没有开新窗口'),
        @(1, 'windowCount', 2, '打开后不是两个窗口'),
        @(5, 'windows.1.background', '#336699', '第二个窗口的背景色没跟着设置变'),
        @(6, 'windowCount', 1, 'Esc 没有只关掉第二个窗口'),
        @(11, 'how', 'reused', '勾上不允许多个实例后没有在当前窗口里换图'),
        @(11, 'name', '2.png', '复用的窗口没有显示 2.png'),
        @(11, 'windowCount', 1, '复用时又开了窗口'),
        @(18, 'windows.0.titleBar', $true, '显示标题栏没有生效'),
        @(23, 'quit', $true, '最后一个窗口里按 Esc 没有退出'))) {
    $value = Get-Value $report "steps.$($expect[0]).$($expect[1])"
    if ($value -ne $expect[2]) { $p.Add("$($expect[3])（第 $($expect[0]) 步 $value）") }
}
$frameHeight = Get-Value $report 'steps.19.windows.0.frame.3'
$contentHeight = Get-Value $report 'steps.19.windows.0.content.1'
$titleBar = if ($null -ne $frameHeight -and $null -ne $contentHeight) { [Math]::Round([double]$frameHeight - [double]$contentHeight) } else { -1 }
if ($titleBar -lt 20 -or $titleBar -gt 60) { $p.Add("显示标题栏后窗口没有多出标题栏的高度（$titleBar）") }
$offset = (@(Get-Value $report 'steps.1.centerOffset') -join ',')
$checkFailures += Add-Check '窗口' $p @("从资源管理器打开开了新窗口（中心偏移 $offset），设置两个窗口都生效，Esc 只关第二个窗口；",
    "勾上不允许多个实例后在原窗口换图；吸附边缘关着时不动、打开后贴边（拖动也贴边）；显示标题栏后多出 $titleBar；最后一个窗口里 Esc 退出")

# 菜单栏：步骤 0 打开（设置里已经让它显示），1 用菜单翻到下一张，2 用菜单打开图片信息，3 打开另一个文件夹里的图，
# 4 从「打开最近使用」回到第一个文件，5 是菜单栏的内容，6 进全屏、7 退出，8–11 在设置里勾上「不显示菜单栏」。
$report = Get-CaseReport 'menubar'
$p = New-Object System.Collections.Generic.List[string]
function Get-MenuRow($Report, [int]$Step) {
    $client = Get-Value $Report "steps.$Step.window.clientHeight"
    $content = Get-Value $Report "steps.$Step.window.contentHeight"
    if ($null -eq $client -or $null -eq $content) { return -1 }
    return [Math]::Round([double]$client - [double]$content, 1)
}
$menuRow = Get-MenuRow $report 0
if ((Get-Value $report 'steps.0.menuBar') -ne $true) { $p.Add('菜单栏没有显示') }
if ($menuRow -lt 31 -or $menuRow -gt 34) { $p.Add("图片区应当比窗口矮一行菜单（32），实际差 $menuRow") }
if ((Get-Value $report 'steps.1.name') -ne '2.png') { $p.Add("用菜单翻到下一张以后是 $(Get-Value $report 'steps.1.name')") }
if ((Get-Value $report 'steps.2.overlay.infoShown') -ne $true) { $p.Add('用菜单没有打开图片信息') }
if ((Get-Value $report 'steps.4.name') -ne '1.png') { $p.Add("从打开最近使用回去以后是 $(Get-Value $report 'steps.4.name')（应为 1.png）") }
$menuTitles = @(Get-Value $report 'steps.5.titles' | Where-Object { $null -ne $_ })
$menuIds = @(Get-Value $report 'steps.5.items' | Where-Object { $null -ne $_ } | ForEach-Object { [string](Get-Value $_ 'id') })
if ($menuTitles.Count -ne 7) { $p.Add("顶层应当有 7 个菜单，实际 $($menuTitles.Count) 个：$($menuTitles -join '、')") }
foreach ($wanted in @('open_file', 'settings', 'quit', 'about', 'copy_to_clipboard', 'next_image', 'rotate_cw', 'fullscreen', 'help_shortcuts', 'clear_recent')) {
    if ($menuIds -notcontains $wanted) { $p.Add("菜单栏里没有 $wanted") }
}
foreach ($unwanted in @('hide_app', 'hide_others', 'show_all')) {
    if ($menuIds -contains $unwanted) { $p.Add("菜单栏里不该有 $unwanted") }
}
$recentInMenu = @(Get-Value $report 'steps.5.recentInMenu' | Where-Object { $null -ne $_ })
if ($recentInMenu.Count -ne 2 -or -not ([string]$recentInMenu[0]).EndsWith('sort\1.png') -or -not ([string]$recentInMenu[1]).EndsWith('quadrants_600x400.png')) {
    $p.Add("打开最近使用里应当是 1.png、quadrants_600x400.png（最近的在前），实际 $($recentInMenu -join '、')")
}
if ((Get-Value $report 'steps.6.window.fullScreen') -ne $true -or (Get-Value $report 'steps.6.menuBar') -ne $false) { $p.Add('全屏时菜单栏还在') }
if ((Get-Value $report 'steps.7.menuBar') -ne $true) { $p.Add('退出全屏以后菜单栏没有回来') }
$menuRowOff = Get-MenuRow $report 11
if ((Get-Value $report 'steps.11.menuBar') -ne $false -or $menuRowOff -gt 3) { $p.Add("勾上不显示菜单栏以后菜单栏还在（图片区比窗口矮 $menuRowOff）") }
$checkFailures += Add-Check '菜单栏' $p @("顶端一行菜单（$($menuTitles -join '、')），图片区在它下面（矮 $menuRow）；用菜单翻页、打开图片信息；",
    '打开最近使用里记着打开过的两个、能回去；全屏时不显示；在设置里勾上以后没有了')

# 系统集成有关的设置：步骤 1 是打开设置（记下有哪些页）。
$report = Get-CaseReport 'installed'
$p = New-Object System.Collections.Generic.List[string]
$pagesInstalled = @(Get-Value $report 'steps.1.settingsPages') -join '、'
if ($pagesInstalled -notmatch 'context_menu') { $p.Add("当作装好时设置里没有上下文菜单页（$pagesInstalled）") }
$externalKinds = @(@(Get-Field $report 'externals') | ForEach-Object { Get-Field $_ 'kind' })
if ($externalKinds -notcontains 'default-apps' -or $externalKinds -notcontains 'associations-by-app') { $p.Add("格式关联的两个按钮没有都起作用（记下的：$($externalKinds -join '、')）") }
$savedSettings = Read-Text (Join-Path $runs 'installed\settings.toml')
if ($savedSettings -notmatch '(?s)\[context_menu\].*enabled = true' -or $savedSettings -notmatch 'thumb_psd = false') { $p.Add('上下文菜单页的开关没有写进设置文件') }
$report = Get-CaseReport 'not-installed'
$pagesPlain = @(Get-Value $report 'steps.1.settingsPages') -join '、'
if ($pagesPlain -match 'context_menu') { $p.Add('没装的程序的设置里不该有上下文菜单页') }
if (@(Get-Field $report 'externals' | Where-Object { $null -ne $_ }).Count -gt 0) { $p.Add('没装的程序的格式关联按钮不该有反应') }
$checkFailures += Add-Check '系统集成有关的设置' $p @('当作装好时：有上下文菜单页，开关写进设置文件，格式关联的两个按钮各打开系统设置的一页；',
    '没装时：没有上下文菜单页，两个按钮不起作用')

# 窗口看不见时：步骤 4 是设置的「确定」，步骤 5 是最小化再取回（中间记下最小化时的状态）。
$report = Get-CaseReport 'visible'
$p = New-Object System.Collections.Generic.List[string]
if ((Get-Value $report 'steps.4.preventingSleep') -ne $true) { $p.Add('打开设置后没有阻止显示器睡眠') }
if ((Get-Value $report 'steps.5.minimized') -ne $true) { $p.Add('窗口没有最小化') }
if ((Get-Value $report 'steps.5.preventingSleepMinimized') -ne $false) { $p.Add('最小化后还在阻止显示器睡眠') }
if ((Get-Value $report 'steps.5.restored') -ne $true) { $p.Add('窗口没有取回') }
if ((Get-Value $report 'steps.5.preventingSleep') -ne $true) { $p.Add('取回窗口后没有再阻止显示器睡眠') }
# 步骤 6 进入全屏，步骤 7 是全屏时最小化再取回，步骤 8 退出全屏。
if ((Get-Value $report 'steps.6.window.fullScreen') -ne $true) { $p.Add('没有进入全屏') }
if ((Get-Value $report 'steps.7.minimized') -ne $true) { $p.Add('全屏的窗口没有最小化') }
if ((Get-Value $report 'steps.7.preventingSleepMinimized') -ne $false) { $p.Add('全屏的窗口最小化后还在阻止显示器睡眠') }
if ((Get-Value $report 'steps.7.restored') -ne $true) { $p.Add('全屏的窗口没有取回') }
if ((Get-Value $report 'steps.7.preventingSleep') -ne $true) { $p.Add('全屏的窗口取回后没有再阻止显示器睡眠') }
if ((Get-Value $report 'steps.7.window.fullScreen') -ne $true) { $p.Add('取回后不是全屏了') }
$checkFailures += Add-Check '窗口看不见时' $p @('打开阻止显示器睡眠后，最小化就不阻止，取回后再阻止；全屏的窗口也一样')
$checks.Add("    系统通知的窗口变化（最小化 | 取回；P 呈现方式、S 大小、M 位置、V 可见、Z 层次、A/D 激活）：普通窗口 $(Get-Value $report 'steps.5.changes')；全屏 $(Get-Value $report 'steps.7.changes')")

# 开关窗口：步骤 2 是 30 次大窗口，步骤 4 是 10 次小图，步骤 5 是 10 个空窗口（1、3 是停顿）。收尾做了的：画布控件
# 回收了、没有留着上传用的数组、显存涨得不到对照的一半；对照（不做收尾）：画布控件、数组都还在。
# 画布控件允许留 1 个：显示过带透明的图片（画过棋盘格）以后总有 1 个留着（
# 1 个，不随窗口数增加；不带透明的图片是 0 个），是谁拿着没查出来。
# 显存是这个脚本在外面量的（两项都量到了才比；对照涨得不多——屏幕小、窗口小——时不作判断）。
$report = Get-CaseReport 'churn'
$control = Get-CaseReport 'churn-keep'
$p = New-Object System.Collections.Generic.List[string]
$closed = Get-Value $report 'steps.4.closedWindows'
if ($closed -ne 42) { $p.Add("应当关掉 42 个窗口（两次各多一个不算的），实际 $closed") }
$aliveControls = Get-Value $report 'steps.4.aliveControls'
if ($null -eq $aliveControls -or $aliveControls -gt 1) { $p.Add("关掉的窗口里还有 $aliveControls 个画布控件没回收（最多允许 1 个）") }
if ((Get-Value $report 'steps.2.aliveControls') -ne 0) { $p.Add("30 个大窗口关掉以后还有 $(Get-Value $report 'steps.2.aliveControls') 个画布控件没回收") }
if ((Get-Value $report 'steps.4.heldBuffers') -ne 0) { $p.Add("关掉的窗口里还有 $(Get-Value $report 'steps.4.heldBuffers') 个留着上传用的数组") }
if (-not ((Get-Value $control 'steps.4.aliveControls') -gt 0)) { $p.Add('对照没有量出画布控件留着（这项检查不管用了）') }
if (-not ((Get-Value $control 'steps.4.heldBuffers') -gt 0)) { $p.Add('对照没有量出上传用的数组留着（这项检查不管用了）') }
function Get-GpuGrowth([string]$Name) {
    foreach ($c in $cases) {
        if ($c.Name -ne $Name) { continue }
        $first = $c.PSObject.Properties['GpuFirst']
        $last = $c.PSObject.Properties['GpuLast']
        if ($null -eq $first -or $null -eq $last -or $null -eq $first.Value -or $null -eq $last.Value) { return $null }
        return [Math]::Round([double]$last.Value - [double]$first.Value)
    }
    return $null
}
$gpu = Get-GpuGrowth 'churn'
$gpuControl = Get-GpuGrowth 'churn-keep'
$gpuText = '显存没有量到'
if ($null -ne $gpu -and $null -ne $gpuControl) {
    $gpuText = "显存多了 $gpu MB（对照 $gpuControl MB）"
    if ($gpuControl -lt 100) { $gpuText += '，对照涨得不多，这一项不作判断' }
    elseif ($gpu -gt $gpuControl / 2) { $p.Add("显存多了 $gpu MB，对照 $gpuControl MB：收尾以后应当不到对照的一半") }
}
$checkFailures += Add-Check '开关窗口' $p @("关掉的 42 个窗口：画布控件还在的 $aliveControls 个、留着上传用的数组的 0 个（对照：$(Get-Value $control 'steps.4.aliveControls') 个、$(Get-Value $control 'steps.4.heldBuffers') 个）")
$checks.Add("    30 个大窗口加 10 个小窗口以后：$gpuText")
$checks.Add("    WinUI 自己不回收的：关掉的 42 个窗口对象还有 $(Get-Value $report 'steps.4.aliveWindows') 个在；10 个什么都不放的空窗口关掉以后还有 $(Get-Value $report 'steps.5.aliveWindows') 个在（只记录）")

# 拖动窗口（drag_real）：左键松开以后才轮到程序开始拖动时不交给系统；真拖的每一次松开后都停下（不跟着光标走）。
$report = Get-CaseReport 'move'
$p = New-Object System.Collections.Generic.List[string]
$dragStep = $null
foreach ($stepItem in @(Get-Value $report 'steps')) { if ($null -ne $stepItem -and (Get-Value $stepItem 'step') -eq 'drag_real') { $dragStep = $stepItem } }
$realDrags = @()
if ($null -eq $dragStep) {
    $p.Add('没有真拖（看 move 那一项的报告）')
} else {
    # 没核对上（比如自检时碰了鼠标、左键按着）也不算通过。
    $guard = [string](Get-Value $dragStep 'guard')
    if (-not $guard.StartsWith('左键松开以后才开始拖动：没有开始移动')) { $p.Add($guard) }
    $realDrags = @(Get-Value $dragStep 'realDrags' | Where-Object { $null -ne $_ })
    # 每一种拖法都要真的开始了移动：没开始（被别的窗口挡住、左键的状态读错了）就等于没量到、没核对。
    # 顶端那一条只记录：模拟的鼠标系统认不出是拖动（手拖正常，看 reports\windows\drags 里的记录），但也不能停不下来。
    if ($realDrags.Count -lt 4) { $p.Add("只真拖了 $($realDrags.Count) 次（应为 4 次）") }
    foreach ($d in $realDrags) {
        if ((Get-Value $d 'required') -ne $false -and (Get-Value $d 'entered') -ne $true) { $p.Add("$(Get-Value $d 'name')：没有开始移动（$(Get-Value $d 'note')）") }
        if ((Get-Value $d 'stillFollowing') -eq $true) { $p.Add("$(Get-Value $d 'name')：松开以后窗口还跟着光标走") }
    }
    if ((Get-Value $dragStep 'resizeOk') -ne $true) { $p.Add([string](Get-Value $dragStep 'resize')) }
}
$stripText = '顶端那一条没量到'
foreach ($d in $realDrags) {
    if ((Get-Value $d 'required') -eq $false) {
        $stripText = if ((Get-Value $d 'entered') -eq $true) { '模拟拖顶端那一条也由系统移动了' } else { '模拟拖顶端那一条被系统当作单击（只记录，手拖看拖动记录）' }
    }
}
$checkFailures += Add-Check '拖动窗口' $p @("左键松开以后才轮到程序开始移动时不开始；拖图片、对照窗口都开始了移动，松开后都停下；拉顶边改大小正常；$stripText")


# 与 macOS 相同的其余核对（顺序、大图、悬浮界面、浏览、文件、高品质、色彩、更多格式、动图、快速预览、真实图片、性能）。
$report = Get-CaseReport 'nav'
$order = (@(0, 1, 2, 3, 4) | ForEach-Object { [string](Get-Value $report "steps.$_.name") }) -join ' '
$p = New-Object System.Collections.Generic.List[string]
if ($order -ne '1.png 2.png 10.png 中文.png 1.png') { $p.Add("$order（应为 1.png 2.png 10.png 中文.png 1.png）") }
$checkFailures += Add-Check '浏览顺序' $p @($order)

$report = Get-CaseReport 'photo'
$p = New-Object System.Collections.Generic.List[string]
$fitW = Get-Value $report 'steps.0.window.fitWidth'
$fitH = Get-Value $report 'steps.0.window.fitHeight'
if ($null -eq $fitW -or $null -eq $fitH) { $p.Add('打开图片后窗口没有按图片调整') }
$checkFailures += Add-Check '大图' $p @("解码 $(Format-Number (Get-Value $report 'steps.0.decodeMs') 'ms')，打开到出图 $(Format-Number (Get-Value $report 'steps.0.imageMs') 'ms')；",
    "4000x3000 的照片打开后窗口为 ${fitW}x${fitH}（有效像素）")

$report = Get-CaseReport 'overlay'
$jumps = "$(Get-Value $report 'steps.3.name') $(Get-Value $report 'steps.4.name')"
$p = New-Object System.Collections.Generic.List[string]
if ($jumps -ne 'plain_640x480.bmp palette_320x240.gif') { $p.Add("进度条跳转到 $jumps（应为 plain_640x480.bmp palette_320x240.gif）") }
$checkFailures += Add-Check '悬浮界面' $p @("进度条跳转 $jumps")

$report = Get-CaseReport 'browse'
$browse = (@(2, 5, 6, 7, 10) | ForEach-Object { [string](Get-Value $report "steps.$_.name") }) -join ' '
$p = New-Object System.Collections.Generic.List[string]
if ($browse -ne '中文.png 中文.png quadrants_600x400.png 1.png 1.png') { $p.Add("结果不对：$browse") }
$checkFailures += Add-Check '浏览' $p @('降序、随机退回、相邻文件夹、末张询问都对')

$p = New-Object System.Collections.Generic.List[string]
[string[]]$left = @(Get-ChildItem -LiteralPath $filesWork -File -ErrorAction SilentlyContinue | ForEach-Object { $_.Name })
if (($left -join ' ') -ne '10.png') { $p.Add("剩下 $($left -join ' ')（应只剩 10.png）") }
$clipboard = @(Get-Value (Get-CaseReport 'files') 'steps.1.clipboard' | Where-Object { $null -ne $_ })
if ($clipboard.Count -eq 0) { $p.Add('没有复制到剪贴板') }
$checkFailures += Add-Check '文件' $p @("复制到剪贴板（$($clipboard -join '、')）；删掉了 1.png、2.png，剩下 10.png")

$report = Get-CaseReport 'quality'
$exactSteps = @(Get-Field $report 'steps' | Where-Object { $null -ne $_ -and (Get-Field $_ 'step') -eq 'wait_exact' })
$exactShown = @($exactSteps | Where-Object { (Get-Field $_ 'exactShown') -eq $true })
$exactWorst = (@($exactShown | ForEach-Object { Get-Value $_ 'exact.renderMs' } | Where-Object { $null -ne $_ }) | Measure-Object -Maximum).Maximum
$p = New-Object System.Collections.Generic.List[string]
if ($exactShown.Count -ne 7) { $p.Add("精确重绘次数不对：$($exactShown.Count)（应为 7）") }
$checkFailures += Add-Check '高品质' $p @("精确重绘 7 次，最慢 $(Format-Number $exactWorst 'ms')")

$center = [string](Get-Value (Get-CaseReport 'color') 'steps.0.samples.centerColor')
$p = New-Object System.Collections.Generic.List[string]
$gray = -1
if ($center -match '^#[0-9A-Fa-f]{6}$') { $gray = [Convert]::ToInt32($center.Substring(1, 2), 16) }
if ($gray -lt 184 -or $gray -gt 192) { $p.Add("$center（灰度 128 按线性 ICC 应约为 #BCBCBC）") }
$checkFailures += Add-Check '色彩' $p @("灰度 128 按线性 ICC 显示为 $center（约 188）")

# 监视器 ICC：第 0 步打开，第 5 步是打开设置后的 wait，第 10 步是关掉后的 wait（与 Linux 的自测相同）。
# 系统给显示器设的配置文件只记录：有的显示器（虚拟显示器、远程桌面）系统不给。
$report = Get-CaseReport 'monitor_icc'
$p = New-Object System.Collections.Generic.List[string]
$grays = @(@(0, 5, 10) | ForEach-Object {
    $c = [string](Get-Value $report "steps.$_.samples.centerColor")
    if ($c -match '^#[0-9A-Fa-f]{6}$') { [Convert]::ToInt32($c.Substring(1, 2), 16) } else { -1 }
})
if ($grays[0] -lt 126 -or $grays[0] -gt 130 -or $grays[1] -lt 52 -or $grays[1] -gt 58 -or $grays[2] -lt 126 -or $grays[2] -gt 130) {
    $p.Add("画面中心的灰度应为 128 → 约 55 → 128，实际 $($grays -join ' → ')")
}
$systemProfile = [string](Get-Value (Get-CaseReport 'color') 'displayProfile.system')
$systemBytes = Get-Value (Get-CaseReport 'color') 'displayProfile.bytes'
$systemText = if ($systemProfile.Length -gt 0) { "系统给这块显示器设的配置文件：$([IO.Path]::GetFileName($systemProfile))（读到 $systemBytes 字节）" } else { '系统没有给这块显示器设配置文件' }
$checkFailures += Add-Check '监视器 ICC' $p @("打开设置后灰度 128 按线性 Gamma 的显示器配置文件显示为 $($grays[1])（约 55），关掉后回到 $($grays[2])；$systemText")

# 更多格式：每一步都要显示出来（HEIC 没装扩展时除外），RAW 的方向按尺寸核对；系统的解码器解不了时记下原因。
# 生成信息：面板跟着图片换内容；没有生成信息的 plain.png 上面板显示“没有”、P 不可用；最后关掉。
$report = Get-CaseReport 'generation'
$p = New-Object System.Collections.Generic.List[string]
$genSteps = @(Get-Field $report 'steps')
$cabin = 'a cozy cabin, warm light, ' + [char]0x96EA
foreach ($want in @(@(3, 'ComfyUI', 'a red fox in snow'), @(5, 'Stable Diffusion WebUI', $cabin), @(7, 'Stable Diffusion WebUI', $cabin))) {
    $g = if ($genSteps.Count -gt $want[0]) { Get-Field $genSteps[$want[0]] 'generation' } else { $null }
    if ($null -eq $g -or -not (Get-Field $g 'open') -or (Get-Field $g 'tool') -ne $want[1] -or (Get-Field $g 'prompt') -ne $want[2] -or -not (Get-Field $g 'enabled')) {
        $p.Add("第 $($want[0]) 步面板不对")
    }
}
$plain = if ($genSteps.Count -gt 11) { Get-Field $genSteps[11] 'generation' } else { $null }
if ($null -eq $plain -or -not (Get-Field $plain 'open') -or $null -ne (Get-Field $plain 'tool') -or (Get-Field $plain 'enabled')) { $p.Add('没有生成信息的 plain.png 上不对') }
$closed = if ($genSteps.Count -gt 12) { Get-Field $genSteps[12] 'generation' } else { $null }
if ($null -eq $closed -or (Get-Field $closed 'open')) { $p.Add('面板没有关掉') }
$checkFailures += Add-Check '生成信息' $p @('按 P 打开面板，ComfyUI 的 PNG、WebP 和 JPEG 的 EXIF 里的 A1111 参数都显示出来，翻到没有的图片时显示“没有”')
$report = Get-CaseReport 'formats'
$p = New-Object System.Collections.Generic.List[string]
$formatList = New-Object System.Collections.Generic.List[string]
$wicNotes = New-Object System.Collections.Generic.List[string]
$formatSteps = @(Get-Field $report 'steps')
for ($i = 0; $i -lt $formatSteps.Count; $i++) {
    $stepRecord = $formatSteps[$i]
    $name = [string](Get-Field $stepRecord 'name')
    $format = [string](Get-Field $stepRecord 'format')
    $size = "$(Get-Field $stepRecord 'width')x$(Get-Field $stepRecord 'height')"
    $wic = Get-Field $stepRecord 'wicError'
    if ($null -ne $wic) { $wicNotes.Add("${name}：$wic") }
    if ($format -eq '' -or $null -ne (Get-Field $stepRecord 'failed')) {
        # HEIC：带着 heif.dll 时必须显示出来；没带时要看系统装没装扩展，显示不出来只记下。
        if ($heifShipped -or $name -notmatch '\.(heic|heif|hif|heics)$') { $p.Add("第 $i 步没有显示出来（$name）") }
        continue
    }
    if ($name -eq 'e_fuji.raf' -and $size -ne '720x1080') { $p.Add("RAF 方向不对 $size") }
    if ($name -eq 'g_rotated.dng' -and $size -ne '768x1024') { $p.Add("DNG 方向不对 $size") }
    # 高动态范围、广色域的 AVIF由核心自己解（按 nclx 换成 sRGB），不交给 WIC：像素格式是核心写的。
    $pixel = [string](Get-Field $stepRecord 'pixelFormat')
    if ($name -eq 'avif_pq.avif' -and $pixel -ne 'YUV444P10 PQ') { $p.Add("avif_pq.avif 应由核心解（YUV444P10 PQ），实际 ${pixel}") }
    if ($name -eq 'avif_p3.avif' -and $pixel -ne 'YUV444P8') { $p.Add("avif_p3.avif 应由核心解（YUV444P8），实际 ${pixel}") }
    if (-not $formatList.Contains($format)) { $formatList.Add($format) }
}
$formatList.Sort([StringComparer]::Ordinal)
$heifText = if ($heifShipped) { '；HEIC 由程序带着的 heif.dll 解' } else { '；没带 heif.dll，HEIC 靠系统的解码器' }
$checkFailures += Add-Check '更多格式' $p @("$($formatSteps.Count) 张都显示出来；格式：$($formatList -join ' ')$heifText")
foreach ($note in $wicNotes) { $checks.Add("    系统的解码器（WIC）解不了，改用核心的：$note") }

# 动图：步骤序号见上面的 $animSteps（0 是打开图片）。
$report = Get-CaseReport 'anim'
$p = New-Object System.Collections.Generic.List[string]
$paused = Get-Value $report 'steps.2.animationInfo.index'
$seen = Get-Value $report 'steps.1.frames'
if ($null -eq $seen -or $seen -lt 3) { $p.Add('GIF 没有在播放') }
if ((Get-Value $report 'steps.2.animationInfo.playing') -ne $false) { $p.Add('空格没有暂停') }
if ((Get-Value $report 'steps.3.animationInfo.index') -ne $paused) { $p.Add('暂停后帧还在换') }
if ($null -ne $paused -and (Get-Value $report 'steps.4.animationInfo.index') -ne (($paused + 1) % 4)) { $p.Add('下一帧不对') }
if ((Get-Value $report 'steps.5.animationInfo.index') -ne $paused) { $p.Add('上一帧不对') }
if ((Get-Value $report 'steps.6.name') -ne 'b_frames.apng') { $p.Add('没有切到 APNG') }
if ((Get-Value $report 'steps.8.name') -ne 'c_transparent.gif') { $p.Add('没有切到透明 GIF') }
$animText = [string](Get-Value $report 'steps.17.animation')
if ($animText -ne '' -or $null -eq (Get-Value $report 'steps.17')) { $p.Add("隐藏动画信息控制后还在显示（$animText）") }
$pausedFrame = if ($null -ne $paused) { $paused + 1 } else { '?' }
$checkFailures += Add-Check '动图' $p @("GIF 播放、空格暂停（停在第 $pausedFrame 帧）、逐帧前后、APNG 播放；透明处按文件里的背景色和设置的白色核对画面；",
    "动画信息控制显示「$(Get-Value $report 'steps.1.animation')」，设置后隐藏")

$report = Get-CaseReport 'preview'
$previewMs = Get-Value $report 'steps.0.previewMs'
$p = New-Object System.Collections.Generic.List[string]
if ($null -eq $previewMs) { $p.Add('快速预览没有出现') }
$checkFailures += Add-Check '快速预览' $p @("$(Format-Number $previewMs 'ms') 出预览，$(Format-Number (Get-Value $report 'steps.0.imageMs') 'ms') 换为清晰图")

if ($realReady) {
    $report = Get-CaseReport 'real'
    $checks.Add('  真实图片（100% 时逐张核对画面）：')
    $shooting = New-Object System.Collections.Generic.List[string]
    foreach ($stepRecord in @(Get-Field $report 'steps')) {
        $decode = Get-Field $stepRecord 'decodeMs'
        if ($null -eq $decode) { continue }
        $name = [string](Get-Field $stepRecord 'name')
        $proxy = if ((Get-Field $stepRecord 'hasProxy') -eq $true) { '，代理纹理' } else { '' }
        $checks.Add("    $name  $(Get-Field $stepRecord 'width')x$(Get-Field $stepRecord 'height')  解码 $(Format-Number $decode 'ms')$proxy")
        # 拍摄信息：图片信息第 5 行起。
        if ($name -eq 'photo_12mp_iguana.jpg' -and $shooting.Count -eq 0) {
            $lines = @(Get-Value $stepRecord 'overlay.infoText')
            for ($k = 4; $k -lt $lines.Count; $k++) { if ([string]$lines[$k] -ne '') { $shooting.Add([string]$lines[$k]) } }
        }
    }
    $p = New-Object System.Collections.Generic.List[string]
    if ($shooting.Count -lt 4) { $p.Add("拍摄信息不全：$($shooting -join ' | ')") }
    $checkFailures += Add-Check '拍摄信息' $p @($shooting -join ' | ')
    # 性能指标与冷启动数据写入报告供参考，不作为功能自检失败条件。
    $report = Get-CaseReport 'perf'
    $perfRows = @(
        @('运行中打开新图片（出预览）', 'steps.9.previewMs', 100, 'ms'),
        @('运行中打开新图片（清晰图）', 'steps.9.imageMs', $null, 'ms'),
        @('切到已预加载的图片（下一帧）', 'steps.4.imageMs', 16.7, 'ms'),
        @('切到未缓存的 1200 万像素 JPEG（出预览）', 'steps.2.previewMs', 50, 'ms'),
        @('切到未缓存的 1200 万像素 JPEG（清晰图）', 'steps.2.imageMs', 150, 'ms'),
        @('静止后精确重绘（Lanczos）', 'steps.6.exact.renderMs', 150, 'ms'),
        @('打开 1 万张的文件夹（扫描 + 第一张）', 'steps.10.imageMs', 1000, 'ms'),
        @('静止 6 秒的 CPU 占用', 'steps.8.idleCpuPercent', 1, '%'),
        @('静止时的内存（工作集）', 'steps.8.workingSetMb', $null, 'MB'),
        @('其中图片缓存（上限 1024 MB）', 'steps.8.cacheMb', $null, 'MB'),
        @('内存峰值（含自检截图等临时内存）', 'steps.8.peakWorkingSetMb', $null, 'MB'))
    $checks.Add('  性能（本机实测）：')
    foreach ($row in $perfRows) {
        $value = Get-Value $report $row[1]
        $shown = if ($null -eq $value) { '没有数据' } elseif ($row[3] -eq '%') { ('{0:0.0} %' -f [double]$value) } else { Format-Number $value $row[3] }
        if ($null -eq $row[2]) {
            $checks.Add("      $($row[0])：$shown")
        } else {
            $mark = if ($null -ne $value -and [double]$value -le [double]$row[2]) { '✓' } else { '✗' }
            $checks.Add("    $mark $($row[0])：$shown（目标 ≤ $($row[2]) $($row[3])）")
        }
    }
}


# 6. 冷启动：打开 1200 万像素 JPEG $t1Count 次，出图后静止 1.5 秒量内存。
Write-Host ''
Write-Host '== 冷启动测试' -ForegroundColor Cyan
$t1Runs = New-Object System.Collections.Generic.List[object]
for ($i = 1; $i -le $t1Count; $i++) {
    $t1Runs.Add((Invoke-Case "t1-$i" "冷启动第 $i 次" @('--open', $localPhoto, '--steps', 'idle:1500', '--sample-at-100')))
    Start-Sleep -Seconds 2
}

# 7. 摘要。
$summary.Add('')
$passed = @($cases | Where-Object { $_.Status -eq '正常' }).Count
$summary.Add("自检：$passed / $($cases.Count) 项通过")
foreach ($c in $cases) {
    $summary.Add("  $($c.Title)：$($c.Status)")
    foreach ($problem in @($c.Problems | Select-Object -First 8)) { $summary.Add("      $problem") }
}
$summary.Add('')
$checkTotal = @($checks | Where-Object { $_ -match '^  [✓✗] ' }).Count
$summary.Add("按报告核对：$($checkTotal - $checkFailures) / $checkTotal 项相符")
foreach ($line in $checks) { $summary.Add($line) }
# 只算自检通过的几次。冷启动耗时与 macOS 相同，按预览交给界面的时刻算；另记第一次画出来的时刻（上传纹理、画完一帧）。
$ok = @($t1Runs | Where-Object { $_.Status -eq '正常' })
$t1Values = @($ok | ForEach-Object {
    $value = Get-Mark $_.Report 'previewSet'
    if ($null -eq $value) { $value = Get-Mark $_.Report 'imageSet' }
    $value
} | Where-Object { $null -ne $_ })
$t1 = Get-Median $t1Values
$summary.Add('')
if ($null -eq $t1) {
    $summary.Add('冷启动打开 1200 万像素 JPEG：没有量到')
} else {
    $verdict = if ($t1 -le 300) { '达标' } else { '未达标' }
    $summary.Add("冷启动打开 1200 万像素 JPEG（出预览，$($t1Values.Count) 次的中位数）：$(Format-Number $t1 'ms')，目标 ≤ 300 ms，${verdict}")
    $summary.Add("  各次（ms）：$((@($t1Values | ForEach-Object { '{0:0}' -f $_ })) -join '、')")
    $summary.Add("  第一次画出来（中位数）：预览 $(Format-Number (Get-RunMedian $ok 'previewDrawn') 'ms')，清晰图 $(Format-Number (Get-RunMedian $ok 'imageDrawn') 'ms')；窗口显示 $(Format-Number (Get-RunMedian $ok 'windowShown') 'ms')")
    $parts = @('main', 'coreReady', 'gpuStart', 'gpuReady', 'launched', 'windowBase', 'canvasCreated', 'overlaysBuilt', 'emptyBuilt', 'hostBuilt', 'contentSet', 'eventsAttached', 'uiSettingsRead', 'backgroundApplied', 'topmostApplied', 'titleBarExtended', 'titleBarColored', 'titleBarApplied', 'windowConstructed', 'windowCreated', 'windowShown', 'canvasDevice', 'previewSet', 'imageSet', 'previewDrawn', 'imageDrawn', 'overlaysReady') | ForEach-Object {
        $value = Get-RunMedian $ok $_
        if ($null -ne $value) { "$_ $('{0:0}' -f $value)" }
    }
    $summary.Add("  分解（中位数，从进程创建算起的毫秒）：$($parts -join '、')")
    $memory = @($ok | ForEach-Object {
        $steps = @(Get-Field $_.Report 'steps')
        if ($steps.Count -gt 1) { Get-Field $steps[1] 'workingSetMb' }
    } | Where-Object { $null -ne $_ })
    $summary.Add("  内存（出图后静止 1.5 秒，工作集中位数）：$(Format-Number (Get-Median $memory) 'MB')")
    $adapters = @(Get-Field $ok[0].Report 'adapters' | Where-Object { $null -ne $_ })
    if ($adapters.Count -gt 0) { $summary.Add("  显卡（DXGI 的顺序，画布用第 0 块）：$($adapters -join '；')") }
}
foreach ($run in $t1Runs) { $summary.Add("  $($run.Title)：$($run.Status)") }
# 拖动窗口：两种去掉标题栏的办法，每移动一步的耗时和这段时间里发生的事。
$moveReport = Get-CaseReport 'move'
if ($null -ne $moveReport) {
    $summary.Add('')
    $summary.Add('拖动窗口（自动移动 60 次；win32 = 现在的办法，winui = 以前的 ExtendsContentIntoTitleBar）：')
    foreach ($name in @('win32', 'winui')) {
        $prefix = "steps.1.moveBench.$name"
        if ($null -eq (Get-Value $moveReport "$prefix.moveMsMedian")) { $summary.Add("  ${name}：没有量到"); continue }
        $summary.Add("  ${name}：每步移动 中位 $(Get-Value $moveReport "$prefix.moveMsMedian") ms、95% $(Get-Value $moveReport "$prefix.moveMs95") ms、最长 $(Get-Value $moveReport "$prefix.moveMsMax") ms；" +
            "计时器晚到 中位 $(Get-Value $moveReport "$prefix.lateMsMedian") ms、最长 $(Get-Value $moveReport "$prefix.lateMsMax") ms；" +
            "窗口变化 $(Get-Value $moveReport "$prefix.windowChanges") 次，XAML 根变化 $(Get-Value $moveReport "$prefix.rootChanges") 次，画布改大小 $(Get-Value $moveReport "$prefix.canvasResizes") 次，" +
            "UI 自动化来问 $(Get-Value $moveReport "$prefix.automationQueries") 次，XAML 画了 $(Get-Value $moveReport "$prefix.frames") 帧，画布重画 $(Get-Value $moveReport "$prefix.draws") 次")
    }
    # 真的用鼠标拖（drag_real）：每一种拖法一行；对照是同一台电脑上不经过 WinUI 的普通窗口，看系统本来有多顺。
    $dragStep = $null
    foreach ($stepItem in @(Get-Value $moveReport 'steps')) { if ($null -ne $stepItem -and (Get-Value $stepItem 'step') -eq 'drag_real') { $dragStep = $stepItem } }
    if ($null -eq $dragStep) {
        $summary.Add('  真的用鼠标拖：没有量到')
    } else {
        $summary.Add('')
        $summary.Add('真的用鼠标拖（自检用 SendInput 按下、每毫秒动 1 像素走一圈再松开；窗口落后光标几像素就是落后几毫秒；图片是程序自己移动，顶端那一条和对照窗口是系统移动）：')
        $summary.Add("  $(Get-Value $dragStep 'dragEnvironment')")
        $summary.Add("  顶端那一条、上边框交给系统：$(Get-Value $dragStep 'nonClient')；$(Get-Value $dragStep 'hitTest')")
        foreach ($d in @(Get-Value $dragStep 'realDrags' | Where-Object { $null -ne $_ })) {
            $summary.Add("  $(Get-Value $d 'text')")
            $samples = @(Get-Value $d 'loop.uiSamples' | Where-Object { $null -ne $_ })
            if ($samples.Count -gt 0) { $summary.Add("    这段时间里窗口的线程在哪个模块（采样）：$($samples -join '；')") }
        }
        $summary.Add("  $(Get-Value $dragStep 'resize')")
        $summary.Add("  $(Get-Value $dragStep 'guard')")
        $uia = @(Get-Value $dragStep 'uiaProcesses' | Where-Object { $null -ne $_ })
        if ($uia.Count -gt 0) { $summary.Add("  加载了 UI 自动化的程序（一直来问窗口内容的在其中）：$($uia -join '、')") }
    }
}
# 静止时 CPU 占用明细（用于排查空闲占用偏高）：性能项静止 6 秒、冷启动第 1 次静止 1.5 秒的明细。
$summary.Add('')
$summary.Add('静止时的 CPU 占用明细：')
foreach ($pair in @(@('perf', 'steps.8', '性能那一项静止 6 秒'), @('t1-1', 'steps.1', '冷启动第 1 次静止 1.5 秒'))) {
    $report = $null
    foreach ($c in $cases) { if ($c.Name -eq $pair[0]) { $report = $c.Report } }
    foreach ($c in $t1Runs) { if ($c.Name -eq $pair[0]) { $report = $c.Report } }
    $cpu = Get-Value $report "$($pair[1]).idleCpuPercent"
    if ($null -eq $cpu) { continue }
    $threads = @(Get-Value $report "$($pair[1]).idleThreads" | Where-Object { $null -ne $_ })
    $summary.Add("  $($pair[2])：CPU $cpu %，JIT $(Get-Value $report "$($pair[1]).idleJitMs") ms（$(Get-Value $report "$($pair[1]).idleJitMethods") 个方法），" +
        "垃圾回收 $(Get-Value $report "$($pair[1]).idleGcCount") 次，画布重画 $(Get-Value $report "$($pair[1]).idleDraws") 次，XAML 画了 $(Get-Value $report "$($pair[1]).idleFrames") 帧，" +
        "$(Get-Value $report "$($pair[1]).idleCallbacks")；线程：$($threads -join '；')")
    $samples = @(Get-Value $report "$($pair[1]).idleUiSamples" | Where-Object { $null -ne $_ })
    if ($samples.Count -gt 0) { $summary.Add("    之后采样界面线程 2 秒（在哪个模块）：$($samples -join '；')") }
}
Write-Utf8 (Join-Path $results 'summary.txt') (($summary -join "`r`n") + "`r`n")

$tempBytes = (Get-ChildItem -LiteralPath $base -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
Write-Host ''
Write-Host '== 摘要' -ForegroundColor Cyan
foreach ($line in $summary) { Write-Host "  $line" }
Write-Host ''
Write-Host "结果已写到 $results" -ForegroundColor Green
Write-Host "临时文件在 ${base}（共 $(Format-Size $tempBytes)）。下次测试还会用到；不再需要时双击「清理.cmd」删掉。"
