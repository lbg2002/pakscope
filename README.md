# 📦 PakScope · 包览

**原生、轻量、安全的 Ubuntu 软件管理器**

Rust + GTK4 / Libadwaita · 中文优先 · 无 Electron · 无常驻后台服务

[![Build and verify](https://github.com/lbg2002/pakscope/actions/workflows/ci.yml/badge.svg)](https://github.com/lbg2002/pakscope/actions/workflows/ci.yml)  ![Version](https://img.shields.io/badge/Preview-v0.2.0-658d83)  ![License](https://img.shields.io/badge/License-MIT-888)

**[GitHub Actions 构建包](https://github.com/lbg2002/pakscope/actions)** · [交互原型（v0.1.1）](docs/UI_PREVIEW.html) · [开发计划](docs/PRODUCT_PLAN.md)

## 主要功能

| 软件查看 | 安全管理 |
|---|---|
| APT / Snap / Flatpak / AppImage 扫描 | 卸载前展示、复制实际等效的终端命令 |
| 中文应用名称、搜索、来源筛选和排序 | APT 预演影响与执行前再次校验 |
| 包版本、启动命令、所属文件与路径 | APT 卸载 / 清除配置模式 |
| JSON / TSV 软件清单导出 | Snap / Flatpak 分类卸载、AppImage 受限删除 |
| 可拖动侧栏、响应式详情、浅深色主题 | 全程确认、Polkit 授权、结果反馈 |

软件详情页提供 **卸载**；APT 还提供 **清除配置并卸载**。卸载弹窗会展示可复制的终端指令和系统可能受影响的内容。

## 🛡️ 安全规则

- **APT**：执行 `apt-get -s` 模拟卸载，展示受影响包；不通过模拟则禁止自动执行。确认后再次模拟并比对结果，再通过 Polkit 提权执行。
- **Snap**：通过 Snap 原生命令卸载。由于无法提供相同级别的可靠预演，界面明确提示用户谨慎确认。
- **Flatpak**：识别用户安装与系统安装，分别使用 `--user` / `--system`；系统级操作需授权。
- **AppImage**：仅允许删除 HOME 的 `Applications`、`应用程序`、`.local/bin` 和 `bin` 目录中识别出的普通 AppImage 文件；不自动删除配置。
- **手动/未识别来源**：禁止猜测归属、随意删除文件或文件夹。
- 命令通过 `std::process::Command` 和独立参数传递，**不通过 shell 执行包名**。

**注意**：APT 的事前模拟不保证操作期间其他进程无法更改包数据库。操作前仍应检查受影响包，并备份重要数据。自动化测试不会进行真实软件卸载。

## 安装与编译

Ubuntu 24.04 或更新版本，推荐 GNOME 桌面。需要系统 GTK4 与 Libadwaita 开发包、Rust stable。

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libgtk-4-dev libadwaita-1-dev libglib2.0-dev git
git clone https://github.com/lbg2002/pakscope.git
cd pakscope
cargo test --no-default-features
cargo check --all-targets
cargo run --release --bin pakscope
```

构建 DEB：

```bash
bash scripts/build-deb.sh
sudo apt install ./dist/pakscope_0.2.0_$(dpkg --print-architecture).deb
```

如果 v0.2 尚在 PR 开发分支中，请先 `git switch feature/v0.2-safe-uninstall-20261021`，或者从该 PR 的成功 GitHub Actions 构建中获取预览安装包。

## 测试

CI 自动执行后端单元测试、GUI 编译检查、DEB 打包，以及 Xvfb 下的 GTK 启动冒烟测试。**真实卸载和 Polkit 交互需在测试机上手工验证**。

## 已知边界与后续路线

目前 APT / Snap 重点识别带 `.desktop` 文件的图形应用，不等同于完整的 `dpkg -l` 系统包管理。Flatpak 显示已安装应用，AppImage 仅扫描受限目录。多来源同款软件的去重识别、软件升级、批量管理、空间分析及系统包专家模式仍在后续版本规划中，**v0.2 不声称已经实现**。

视觉采用克制的现代宋韵表达，墨色文本、温和底色与黛青点缀，不占用工作区域放置固定说明文案。

MIT License · [反馈问题](https://github.com/lbg2002/pakscope/issues)
