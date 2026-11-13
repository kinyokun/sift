# Sift

快速选片工具：打开一个文件夹，翻看、标记、打星、套滤镜、导出。支持 JPEG 和各家 RAW，RAW 直接用相机内嵌的预览图，翻页不等解码。

Rust 核心 + Tauri 2 外壳 + React/TypeScript 界面，面向 macOS、Windows、iOS、Android。

*Sift is a fast photo culling app for JPEG and RAW: browse, flag, rate, try looks, export. English UI included; this README is in Chinese.*

Sift 起源于 [Anicrop Studio](https://github.com/R4ksh4/AnicropStudio)（R4ksh4，Apache-2.0）——一个为漫展返图打造的 Windows 选片工作台。Sift 用 Rust 和 TypeScript 重写了它，带到 macOS、Windows、iOS 和 Android 上；眼部特写、原厂对焦点、文件内打星、按选片找回 RAW、对比、构图线、参数相框这些功能和思路都来自 Anicrop Studio。见 [致谢](#致谢)。

## 下载

在 [Releases](https://github.com/kinyokun/sift/releases) 下载：

- macOS（Apple 芯片和 Intel 通用）：`Sift_<版本>_universal.dmg`
- Windows 10 / 11（64 位）：`Sift_<版本>_x64-setup.exe`

安装包目前没有经过 Apple 公证 / 微软签名：

- macOS 首次打开若提示无法验证开发者，到 “系统设置 → 隐私与安全性” 底部点 “仍要打开”
- Windows 若出现 SmartScreen 提示，点 “更多信息 → 仍要运行”

每个 Release 附有 `SHA256SUMS.txt` 可校验文件。

## 功能

**选片**

- 网格浏览与全屏查看，RAW+JPEG 自动配对成一张；可按文件名、拍摄时间、星级、文件大小排序
- 连拍和像素偏移拍摄的照片自动叠成一组，查看页里可在组内逐张挑；实况照片显示成一张并可播放，macOS 上可以用连拍做实况照片
- 多选：点照片左上角的圆圈逐张勾选，按住鼠标拖动框选，Shift 选一段，⌘/Ctrl 点选；选中多张后底部出现操作栏，可一次打星、标记、套滤镜。手机上长按照片进入多选
- 选中 / 排除 / 0–5 星；星级写入文件内的 XMP（写不进去时写 `.xmp` 旁车文件），RAW 文件本身从不改写
- 100% 查看、眼部特写（人眼，以及猫、狗、鸟、马、羊、牛等动物的眼睛）、对焦点显示（尼康、佳能、索尼、富士）
- 左右对比两张照片，可同步缩放和平移
- 构图线、裁剪预览（确认后导出时生效）
- 直方图，以及画面上的 **过曝 / 欠曝警告**（红色过曝、蓝色欠曝，可随时开关）
- 进度按文件夹自动保存，下次打开接着选

**滤镜**

- 导入 `.cube`、`.3dl`、HaldCLUT `.png` 和尼康 `.NP3`
- 有 RAW 的照片从 RAW 渲染，不会叠在机内风格上
- **滤镜对比墙**：一张照片同时看全部滤镜的效果，点一张即套用
- **批量套用**：在网格里选中多张，一次设好滤镜
- **存入滤镜库**：照片是用云创 / 自定义配方拍的，可以把这份配方从照片里取出来存成 NP3，套到别的照片上
- 套了滤镜或裁剪的照片导出为渲染后的 JPEG，其余原样复制

**尼康官方引擎**

官方发布的 macOS 和 64 位 Windows 安装包内置了尼康的 NEF/NRW Image SDK：NEF 的原片和 NP3 滤镜由尼康自己的引擎渲染，自定义曲线也一样，与机内直出一致，装好即用。

这部分经尼康授权随 Sift 分发，不是开源软件，使用条款见 [NIKON_SDK_TERMS.md](NIKON_SDK_TERMS.md)。它不在本仓库里：自己从源码构建的 Sift 不含这个引擎，NP3 按参数近似还原，效果会有差别，其余功能不受影响。

**其他**

- 导出：复制 / 重命名、按文件名找回 RAW、带参数的相框
- 局域网传输：手机扫码下载原片或把照片发回电脑
- 快捷键全部可改（设置 → 快捷键）

## 常用快捷键

- `A` / `D` 或 `←` / `→`：上一张 / 下一张
- `↑` / `↓`：上一组 / 下一组（连拍、像素偏移）
- `P` / `X` / `U`：选中 / 排除 / 取消标记
- `0`–`5`：星级
- `Z`：100% / 适合窗口
- `E`：眼部特写
- `F`：对焦点
- `N`：原片（无滤镜）
- `⌘A` / `⌘D`：上一个 / 下一个滤镜（Windows 为 Ctrl）
- `W`：滤镜对比墙
- `H`：过曝 / 欠曝警告
- `C`：对比
- `R`：裁剪预览
- `I`：信息栏
- `空格`：隐藏界面
- `` ` ``：全部快捷键

## 支持的格式

- 图片：JPEG、PNG、WebP、BMP、GIF、TIFF
- RAW：CR3、CR2、NEF、NRW、ARW、SR2、SRF、DNG、RAF、ORF、RW2、PEF、3FR、IIQ、ERF、SRW

尼康 High Efficiency（HE / HE\*）NEF 的“无滤镜原片”在 Apple 平台用系统解码器；Windows 和 Android 自带的解码器读不了这种文件，仍可用内嵌预览选片。

## 构建

需要 Rust（stable）、Node 22、pnpm。

```bash
pnpm install
```

```bash
pnpm tauri dev
```

```bash
pnpm tauri build
```

- macOS 安装包在 `target/release/bundle/dmg/`，Windows 在 `target/release/bundle/nsis/`
- 每次推送和 pull request，GitHub Actions 都会构建一个 Windows 测试安装包，在该次运行的 Artifacts 里下载
- 正式版本由推送 `v*` 标签触发构建，流程见 [CONTRIBUTING.md](CONTRIBUTING.md#发版)
- iOS / Android 的命令见 [AGENTS.md](AGENTS.md)

## 开发

```bash
cargo test -p sift-core
```

```bash
cargo clippy --workspace
```

```bash
pnpm typecheck
```

目录：

- `crates/sift-core`：不依赖界面的核心。容器解析、内嵌预览、EXIF、对焦点、星级、人脸与动物检测、RAW 显影、滤镜、尼康引擎、直方图、导出
- `src-tauri`：命令、`sift://` 图片协议、局域网传输
- `src`：界面。状态在 `lib/store.ts`，文案在 `lib/i18n.ts`（中 / 英）

检查单个文件、无窗口截图检查界面的方法，以及容易踩的坑，都写在 [AGENTS.md](AGENTS.md)。参与贡献请看 [CONTRIBUTING.md](CONTRIBUTING.md)，安全问题请按 [SECURITY.md](SECURITY.md) 私下报告。

## 致谢

- [Anicrop Studio](https://github.com/R4ksh4/AnicropStudio)：Sift 的起点，作者 R4ksh4 及其贡献者（Apache-2.0）
- [rawler / dnglab](https://github.com/dnglab/dnglab)：RAW 显影（LGPL-2.1）
- [tract](https://github.com/sonos/tract)：在本机运行识别模型
- [Tauri](https://tauri.app)、[React](https://react.dev)、[zustand](https://github.com/pmndrs/zustand)
- [raw.pixls.us](https://raw.pixls.us)：测试用样张
- 识别模型与字体见下方

## 许可

Sift 以 [GPL-3.0-or-later](LICENSE) 许可开源：可以自由使用、修改和再分发，但分发修改版或基于 Sift 的作品时，必须同样以 GPL-3.0 开源并提供完整源码。版权、来源声明和附加许可见 [NOTICE](NOTICE)，第三方组件见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)；每个 Release 附带全部 Rust 依赖的许可全文 `THIRD-PARTY-LICENSES.html`。

Anicrop Studio 采用 Apache-2.0，Apache-2.0 允许其衍生作品以 GPL-3.0 分发；所用依赖（含 LGPL-2.1 的 rawler、MPL-2.0 的若干 crate）都与 GPL-3.0 兼容。

尼康 SDK 归尼康所有，经尼康授权内置在官方安装包里，不适用 GPL，使用条款见 [NIKON_SDK_TERMS.md](NIKON_SDK_TERMS.md)；驱动它的 `sift-nikon` 程序因尼康许可的保密条款无法公开源码，是一个独立的可执行文件，Sift 通过本机套接字和它通信。NOTICE 里按 GPL-3.0 第 7 条附加了一条许可，允许这样组合分发。Nikon、Canon、Sony、Fujifilm 等是各自所有者的商标，Sift 与这些公司没有关联。

随应用分发的识别模型与字体（在 `crates/sift-core/assets`）：

- YuNet 人脸检测（OpenCV Zoo，MIT）
- YOLOX-Tiny 物体检测（Megvii，Apache-2.0），用来找动物
- RTMPose-m AP-10K 动物关键点（OpenMMLab MMPose，Apache-2.0；AP-10K 数据集为 CC BY 4.0），用来找动物的眼睛
- Inter 字体（SIL OFL 1.1），用于参数相框
