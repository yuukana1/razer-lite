# Razer Lite

> [!WARNING]
> **这是个 AI slop。** 一句话让 AI 生成的，个人写着玩，代码没认真审过。不保证能用，用坏了概不负责。与 Razer 官方无关。

不装 Razer Synapse 也能用雷蛇鼠标的基础功能：侧键 / 滚轮映射、DPI 档位、回报率、休眠时间、电量提醒、开机静默自启、固件更新（下载并校验官方更新程序后运行）。Rust 写的，连雷蛇驱动都不用装，用 Windows 自带的 HID 驱动和鼠标通信。

只在 Razer Viper V3 HyperSpeed 上测过。

## 内存占用

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/memory-dark.png">
  <img alt="常驻内存对比（专用内存）：雷蛇官方软件 1018.6 MB（14 个进程）；Razer Lite 后台 1.9 MB（1 个进程）；Razer Lite 设置窗口 14.7 MB（仅打开时）" src="docs/memory-light.png" width="820">
</picture>

## 构建安装

```powershell
cargo build --release
powershell -ExecutionPolicy Bypass -File install.ps1   # 加 -Uninstall 卸载
```

HID 协议参考 [OpenRazer](https://github.com/openrazer/openrazer)。
