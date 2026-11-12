# 参与贡献

欢迎提 issue 和 pull request。*Issues and pull requests are welcome in English or Chinese.*

## 开始之前

- 较大的改动请先开 issue 说明想法，避免做了却合不进来
- 架构、命令和容易踩的坑写在 [AGENTS.md](AGENTS.md)，动手前先读一遍，尤其是 “Rules that are easy to break” 一节
- 问题反馈请附上相机型号、文件格式和系统版本；涉及某个文件解析错误时，能提供样张最好（注意照片里的 GPS 等隐私信息）

## 本地开发

需要 Rust（stable）、Node 22、pnpm。

```bash
pnpm install
```

```bash
pnpm tauri dev
```

提交前请跑：

```bash
cargo test -p sift-core
```

```bash
cargo clippy --workspace
```

```bash
pnpm typecheck
```

样张测试需要先 `bash samples/fetch.sh`（约 1.3 GB，来自 raw.pixls.us）；没有样张时这些测试直接通过。

## 约定

- 界面文案都放在 `src/lib/i18n.ts`，中英文两份都要改
- 新增 npm 包至少发布 7 天后才会被解析（`pnpm-workspace.yaml`）；Rust crate 也请按同样标准固定版本
- 新依赖的许可证需与 GPL-3.0 兼容并在 `about.toml` 的允许列表内（GPL-2.0-only、SSPL、带非商用条款的许可都不行）；随应用分发的模型、字体等资源要在 `NOTICE` 和 `THIRD_PARTY_NOTICES.md` 里写明来源和许可
- 不要提交尼康 SDK 的任何内容（`/vendor/` 已忽略），也不要提交依据其头文件或文档写的代码：SDK 的接口受尼康许可的保密条款约束，驱动它的 `sift-nikon` 在另一个不公开的仓库里，本仓库只有 `crates/sift-core/src/nikon.rs` 里的通信协议
- 不要提交样张、个人路径、内网地址或任何密钥
- 提交信息用英文祈使句，一行说清做了什么

提交 pull request 即表示你同意按本项目的 [GPL-3.0-or-later](LICENSE) 许可（含 NOTICE 中关于尼康 SDK 的附加许可）贡献代码。

## 持续集成

`.github/workflows/ci.yml` 在每次推送和 pull request 时运行：

- Linux 上的核心测试与 clippy
- 类型检查与前端构建
- Windows 安装包（快速配置：不开 LTO、16 个 codegen unit、构建在 Dev Drive 上），可在该次运行的 Artifacts 里下载

macOS 构建只在手动触发时运行（Actions → ci → Run workflow，勾选 macOS）。

## 发版

1. 设置版本号（同时改 `package.json`、`Cargo.toml`、`tauri.conf.json` 和 `Cargo.lock`）：

   ```bash
   node scripts/version.mjs 0.2.0
   ```

2. 在 `CHANGELOG.md` 里把 “未发布” 下的条目移到 `## [0.2.0] - 日期` 下，提交
3. 打标签并推送：

   ```bash
   git tag v0.2.0 && git push origin main v0.2.0
   ```

4. `release.yml` 会核对版本号、用完整 release 配置构建 macOS（通用二进制 `.dmg`）和 Windows（x64 `-setup.exe`）安装包、生成 `THIRD-PARTY-LICENSES.html` 和 `SHA256SUMS.txt`，并创建一个 **草稿** Release。检查无误后在 GitHub 上点发布。带 `-` 的标签（如 `v0.2.0-beta.1`）会标成预发布。

### macOS 签名（可选）

不配置时，macOS 包做的是 ad-hoc 签名，用户首次打开需要在 “系统设置 → 隐私与安全性” 里点 “仍要打开”。有 Apple Developer 账号时，在仓库的 Actions secrets 里配置以下几项即可自动签名并公证：

- `APPLE_CERTIFICATE`：Developer ID Application 证书导出的 `.p12`，base64 编码
- `APPLE_CERTIFICATE_PASSWORD`：`.p12` 的密码
- `APPLE_SIGNING_IDENTITY`：如 `Developer ID Application: 名字 (TEAMID)`
- `APPLE_ID`、`APPLE_PASSWORD`（App 专用密码）、`APPLE_TEAM_ID`：用于公证，不配则只签名不公证

Windows 包目前未签名，SmartScreen 会提示 “未知发布者”。

### 尼康引擎（仅官方构建）

仓库配置了 `NIKON_DEPLOY_KEY`（不公开的 `sift-nikon` 仓库的只读部署密钥）时，构建会检出该仓库并运行其中的 `assemble.sh`，把 helper 和尼康的运行库放进 `vendor/nikon-runtime/`，再用里面生成的 Tauri 配置一起打包。没有这个密钥（例如 fork）时构建照常进行，只是不含尼康引擎。
