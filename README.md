# Learnloom

Learnloom 是使用 Tauri、React 和 Rust 构建的 Windows 本地学习知识库。

当前源码版本：**0.4.1 候选版**。适合受控试用，尚未完成全部公开发布验收。产品许可证尚未指定。

## 功能

- 保存和导入 UTF-8 文本、Markdown，云端整理为知识页面和知识点。
- 以阅读排版查看原始资料、Wiki、来源和关联页面。
- 知识问答与追问：库内依据可追溯；资料不足时允许单独标注的 AI 补充，未经联网核验。
- 每日复习、简答题批改、学习表现和整库备份恢复。
- 知识库由用户选择保存位置；模型密钥存 Windows 系统凭据库。
- 上下文容量可配置为 2048～1048576；4096 仅为默认值。实际容量取决于模型服务。

## 构建

准备 Windows x64、Node.js 22.12+、项目 `rust-toolchain.toml` 指定的 Rust 工具链、MSVC C++ 构建工具、Windows SDK 和 WebView2 Runtime。

```powershell
npm ci --legacy-peer-deps
npm run tauri -- dev
# 构建 NSIS 安装包
npm run tauri -- build --bundles nsis
```

安装包输出到 `src-tauri/target/release/bundle/nsis/`。`scripts/env.ps1` 是可选的本地工具缓存配置，不是通用构建的必需步骤。图标矢量稿位于 `src/assets/learnloom.svg`；重新生成 PNG/ICO 需要 Python 和 Pillow。

## 离线测试

```powershell
npm test
cargo test --locked --manifest-path src-tauri/Cargo.toml --no-default-features --features m0-tools -- --test-threads=1
```

默认测试使用合成内容和本地 HTTP mock，不调用付费模型。4项依赖内部历史验收产物的回放测试显式标记为 ignored，只在拥有对应本地证据时运行；它们不属于默认通过数量。其余测试输入随源码提供。`src-tauri/spikes` 中的真实模型验收工具需另外配置个人凭据和预算，不应当作离线测试运行。

## 试用与数据

首次启动后选择专用知识库目录，再填写兼容的 HTTPS 模型服务、模型 ID 和自己的 API Key，点击“保存并测试连接”。整理、问答、出题及批改会向该服务发送所需内容，并可能产生费用。

**升级前请整库备份。** 0.4.1使用基础数据库版本2、扩展迁移5；0.4.0不能打开已升级库，回退需要升级前的整库备份。卸载时若选择删除应用数据，配置目录中的知识库也可能被删除，请把备份放到库外。

本仓库不包含安装包、个人知识库、API密钥、调用账本、内部验收报告或历史模型输出。模型生成与批改可能出错；AI补充不等于知识库依据。干净Windows依赖安装矩阵、签名及分发验收尚未完成。
