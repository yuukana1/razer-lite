# Razer Lite 安装脚本：复制到 %LOCALAPPDATA%\Programs\RazerLite，创建开始菜单快捷方式，开启开机自启并启动。
# 卸载：powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall
param([switch]$Uninstall)
$ErrorActionPreference = 'Stop'
$dest = Join-Path $env:LOCALAPPDATA 'Programs\RazerLite'
$menu = Join-Path ([Environment]::GetFolderPath('Programs')) 'Razer Lite'
$agent = Join-Path $dest 'razer-lite.exe'

if (Test-Path $agent) {
    & $agent --stop | Out-Null
    Start-Sleep -Milliseconds 800
}
Get-Process razer-lite-settings -ErrorAction SilentlyContinue | Stop-Process -Force

if ($Uninstall) {
    if (Test-Path $agent) { & $agent --autostart off | Out-Null }
    Remove-Item $menu -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item $dest -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "已卸载。配置文件保留在 $env:APPDATA\RazerLite（可手动删除）"
    return
}

$src = Join-Path $PSScriptRoot 'target\release'
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item (Join-Path $src 'razer-lite.exe'), (Join-Path $src 'razer-lite-settings.exe') $dest -Force

New-Item -ItemType Directory -Force $menu | Out-Null
$sh = New-Object -ComObject WScript.Shell
$lnk = $sh.CreateShortcut((Join-Path $menu 'Razer Lite 设置.lnk'))
$lnk.TargetPath = Join-Path $dest 'razer-lite-settings.exe'
$lnk.WorkingDirectory = $dest
$lnk.Save()
$lnk = $sh.CreateShortcut((Join-Path $menu 'Razer Lite 后台.lnk'))
$lnk.TargetPath = $agent
$lnk.WorkingDirectory = $dest
$lnk.Save()

# 已经是管理员模式自启的保持不变，否则开启普通自启
cmd /c "schtasks /Query /TN RazerLite >nul 2>&1"
if ($LASTEXITCODE -ne 0) { & $agent --autostart user | Out-Null }
Start-Process $agent
Write-Host "已安装到 $dest 并启动"
