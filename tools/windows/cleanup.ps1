# 删除「编译测试.cmd」在这台电脑上留下的临时文件：%TEMP%\anyview-build
# （.NET SDK、NuGet 包、编好的程序、测试图片副本、测试记录与手动测试配置）。项目目录中的测试报告（reports\windows）保留。
# 用法：双击同目录下的「清理.cmd」。兼容 Windows PowerShell 5.1；文件必须保存为带 BOM 的 UTF-8、CRLF 换行。

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$temp = [IO.Path]::GetTempPath()
$base = Join-Path $temp 'anyview-build'
Write-Host 'Anyview：删除 Windows 测试留下的临时文件' -ForegroundColor Cyan
# 不经过脚本、直接运行 Anyview.exe 时可能留下的：解开压缩包用的临时目录（程序退出时本来会删）、出错记录。
foreach ($extra in @((Join-Path $temp 'anyview-archives'), (Join-Path $temp 'anyview-crash.txt'))) {
    if (Test-Path -LiteralPath $extra) {
        Remove-Item -LiteralPath $extra -Recurse -Force -ErrorAction SilentlyContinue
        Write-Host "已删除 $extra"
    }
}
$settingsDir = Join-Path $env:APPDATA 'Anyview'
if (Test-Path -LiteralPath $settingsDir) {
    Write-Host "注意：${settingsDir} 是直接运行 Anyview.exe（不经过脚本）时存的设置，这里不删；不要的话可以手动删掉。" -ForegroundColor Yellow
}
if (-not (Test-Path -LiteralPath $base)) {
    Write-Host "没有要删的：${base} 不存在。" -ForegroundColor Green
    exit 0
}

# 安装程序的检查（package.ps1 -Action test）中途被打断时，系统里可能还登记着装在这个文件夹里的 Anyview：先让它的
# 卸载程序撤掉登记，不然注册表里留着指向已删文件的项。你自己正常装的 Anyview 不在这个文件夹里，不动。
$registered = (Get-ItemProperty -LiteralPath 'HKCU:\Software\Anyview' -ErrorAction SilentlyContinue).InstallDir
if ($registered -and $registered.StartsWith($base, [StringComparison]::OrdinalIgnoreCase)) {
    $uninstaller = Join-Path $registered 'uninstall.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        Write-Host "卸载检查时装在这里的 Anyview（${registered}）"
        try { Start-Process -FilePath $uninstaller -ArgumentList '/S', ('_?=' + $registered) -Wait } catch { Write-Host "卸载失败：$($_.Exception.Message)" -ForegroundColor Red }
    }
}

# 先关掉从这里运行的程序（编译服务、Anyview 窗口），不然文件删不掉。
$prefix = $base.TrimEnd('\') + '\'
foreach ($process in @(Get-Process)) {
    $path = $null
    try { $path = $process.Path } catch { }
    if ($path -and $path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        Write-Host "关掉 $($process.ProcessName)（进程 $($process.Id)）"
        try { Stop-Process -Id $process.Id -Force } catch { }
    }
}
Start-Sleep -Seconds 1

$bytes = (Get-ChildItem -LiteralPath $base -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
if ($null -eq $bytes) { $bytes = 0 }
try {
    Remove-Item -LiteralPath $base -Recurse -Force
} catch {
    # 路径太长等情况：交给 cmd 的 rmdir（加 \\?\ 前缀，不受 260 个字符的限制）。
    $null = & cmd.exe /c "rmdir /s /q `"\\?\$base`""
}
if (Test-Path -LiteralPath $base) {
    Write-Host "有些文件没删掉（可能还在被使用）：${base}。关掉相关程序或重启后再运行一次。" -ForegroundColor Red
    exit 1
}
Write-Host "已删除 ${base}，共 $('{0:0}' -f ($bytes / 1MB)) MB。项目目录中的测试报告未动。" -ForegroundColor Green
