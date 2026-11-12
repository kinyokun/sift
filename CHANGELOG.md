# Changelog

格式参照 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。发版时把 “未发布” 下的条目移到新版本号下面，release 工作流会把那一节作为 GitHub Release 的说明。

## [Unreleased]

## [0.2.0] - 2026-11-12

### 变更

- 官方 macOS / Windows 安装包内置尼康 NEF/NRW Image SDK，NEF 与 NP3 滤镜装好即按机内效果渲染，不用再自己下载安装（使用条款见 `NIKON_SDK_TERMS.md`）
- 设置里去掉了安装 / 移除 SDK 和下载页入口；以前安装到应用数据目录的 SDK 副本会在启动时清掉
- 尼康引擎改由独立的 `sift-nikon` 程序驱动，本仓库不再含任何依据 SDK 头文件写的代码；从源码自行构建的版本不含该引擎

## [0.1.0] - 2026-11-12

第一个版本。

- 网格与全屏选片，RAW+JPEG 配对，按文件名 / 拍摄时间 / 星级 / 大小排序，多选、框选与批量操作
- 选中 / 排除 / 0–5 星，写入文件内 XMP 或 `.xmp` 旁车，RAW 从不改写；进度按文件夹自动保存
- 100% 查看、人眼与动物眼部特写、尼康 / 佳能 / 索尼 / 富士对焦点、左右对比、构图线、裁剪
- 直方图与过曝 / 欠曝警告
- 滤镜：`.cube`、`.3dl`、HaldCLUT、尼康 `.NP3`；滤镜对比墙、批量套用、从照片取出配方
- 可选的尼康 NEF/NRW Image SDK 渲染（macOS、64 位 Windows），设置里可直达尼康下载页
- 导出（复制 / 重命名 / 找回 RAW / 参数相框）、局域网传输、自定义快捷键
- 中文 / 英文界面

[Unreleased]: https://github.com/kinyokun/sift/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/kinyokun/sift/releases/tag/v0.2.0
