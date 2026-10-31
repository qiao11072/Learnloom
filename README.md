<p align="center">
  <img src="src/assets/learnloom.svg" width="88" height="88" alt="Learnloom 图标" />
</p>

# Learnloom · 让知识持续生长

> 把零散资料整理成自己的知识库，再通过阅读、问答与每日复习巩固理解。原始资料、知识页面和学习记录保存在本机，AI 帮助整理内容、回答问题和批改练习。

基于 **Tauri + React + Rust + SQLite** 构建的 Windows 桌面应用。

**当前版本：0.4.1 候选版。** 已实现主要学习流程，适合受控试用；尚未完成全部公开发布验收。

## 快速开始

### 下载安装包

前往 [0.4.1-rc1 预发布页面](https://github.com/qiao11072/Learnloom/releases/tag/v0.4.1-rc1)，下载 `Learnloom_0.4.1_x64-setup.exe`，适用于 Windows x64。校验文件与第三方许可材料随版本提供。

该候选包尚未签名，干净 Windows 安装矩阵仍有未验证项。缺少 WebView2 时需要联网安装依赖。升级前请整库备份，首次试用建议使用独立知识库。

### 从源码启动

环境要求：

- Windows x64，以及 Microsoft Edge WebView2 Runtime。
- Node.js 22.12 或更新版本。
- [rust-toolchain.toml](rust-toolchain.toml) 指定的 Rust 工具链。
- MSVC C++ 构建工具与 Windows SDK。

```powershell
git clone https://github.com/qiao11072/Learnloom.git
cd Learnloom
npm ci --legacy-peer-deps
npm run tauri -- dev
```

`scripts/env.ps1` 用于原开发环境的工具缓存配置；使用系统已安装的工具链时无需执行。

### 第一次使用

1. **建立知识库**：保存位置初始为空白，点击“选择文件夹”，选择专用空目录并创建知识库；也可以打开已有的 Learnloom 知识库。
2. **连接模型**：填写 HTTPS 服务地址、实际模型 ID 和自己的 API Key，点击“保存并测试连接”。上下文容量按服务支持范围填写，支持 `2048～1048576`；`4096` 只是默认值。
3. **保存并整理资料**：粘贴正文，或导入 UTF-8 `.txt`、`.md`、`.markdown` 文件，保存后交给 AI 整理。
4. **阅读与提问**：阅读原始资料和知识页面，在“知识问答”中提问或追问，展开来源查看引用版本。
5. **复习与回顾**：获取今日题组、作答并查看批改，再到“学习表现”查看知识点表现和历史记录。

可以稍后配置模型，先保存和阅读本地资料。整理、问答、出题和批改需要云端模型，会向所配置的服务发送必要内容，并可能产生费用。

### 构建安装包

```powershell
npm run tauri -- build --bundles nsis
```

Windows NSIS 安装包输出到 `src-tauri/target/release/bundle/nsis/`。安装与运行依赖 WebView2；缺少运行时时，内嵌安装引导程序仍需联网获取依赖。

## 功能矩阵

“已实现”表示当前代码具备该功能，不代表所有发布环境或模型服务均已验证。

| 功能 | 状态 | 说明 |
| --- | --- | --- |
| 📥 资料保存与整理 | 已实现 | 导入文本或 Markdown，原文先保存，再生成知识页面和知识点 |
| 📖 原文与知识阅读 | 已实现 | 标题、段落、列表、表格和代码块按阅读模式展示，可继续查看来源和关联页面 |
| 💬 知识问答与追问 | 已实现 | 会话保存在当前知识库，支持流式回答、取消、显式重试和版本化引用 |
| ✦ AI 补充说明 | 已实现 | 库内证据不足时可补充一般知识，单独标注，未经联网核验 |
| 📝 每日复习与批改 | 已实现 | 每天最多 5 道简答题，支持草稿、0/1/2 分批改、参考答案和解析 |
| 📊 学习表现 | 已实现 | 查看近期成绩、薄弱知识点、连续作答表现和历史答案 |
| 💾 整库备份与恢复 | 已实现 | 备份原始资料、Wiki 版本和学习记录，恢复到新的空目录并校验 |
| ⚙️ 模型与生成偏好 | 已实现 | DeepSeek 预设或自定义兼容服务、连接测试、生成语言和上下文容量配置 |

知识不足或部分候选无效时，题组可能少于 5 题，界面显示实际题数。学习表现来自最近最多 5 次有效作答，不代表精确掌握概率。

## 文件说明

| 文件 / 目录 | 作用 |
| --- | --- |
| [src/main.tsx](src/main.tsx) | 应用入口、首次配置、知识页面与原始资料界面 |
| [src/features/](src/features/) | 问答、每日复习、学习表现及模型设置 |
| [src/components/Markdown.tsx](src/components/Markdown.tsx) | 原始资料与回答的 Markdown 阅读渲染 |
| [src-tauri/src/](src-tauri/src/) | 知识库、模型调用、检索、学习记录及备份等 Rust 逻辑 |
| [src-tauri/migrations/](src-tauri/migrations/) | 数据库结构与迁移 |
| [src-tauri/tauri.conf.json](src-tauri/tauri.conf.json) | 桌面窗口、资源权限策略与 Windows 打包配置 |
| [src-tauri/tests/](src-tauri/tests/) · [tests/](tests/) | 回归测试与合成测试数据 |
| [src-tauri/spikes/](src-tauri/spikes/) | 开发验收工具，包含需单独配置的真实模型测试 |
| [src/assets/learnloom.svg](src/assets/learnloom.svg) | 图标矢量稿；PNG / ICO 由 `scripts/create-icon.py` 生成 |

## 技术栈

| 层次 | 技术 |
| --- | --- |
| 桌面 | Tauri 2、Windows WebView2 |
| 界面 | React、TypeScript、Vite |
| 本地业务 | Rust、Tokio |
| 数据存储 | SQLite、文件系统、Windows 系统凭据库 |
| 内容渲染 | react-markdown、remark-gfm |
| 模型接入 | HTTPS 云端 OpenAI 兼容接口；DeepSeek 预设 |
| 测试与打包 | Vitest、Rust tests、NSIS |

## 开发与测试

```powershell
# 前端类型检查与构建
npm run build

# 前端回归
npm test

# Rust 离线业务测试，不调用付费模型
cargo test --locked --manifest-path src-tauri/Cargo.toml --no-default-features --features m0-tools -- --test-threads=1
```

默认测试使用合成内容和本地 HTTP mock。4 项依赖内部历史证据的回放测试标记为 `ignored`，不计入默认通过数。首次安装依赖可能需要联网；已有缓存时可给 Cargo 命令增加 `--offline`。

`src-tauri/spikes` 中的真实模型工具会产生外部调用，需另外配置个人凭据和预算。重新生成应用图标需要 Python 与 Pillow。

## 数据与升级

- **存放位置**：知识库保存在你选择的目录。原文在 `sources/`，知识页面版本在 `wiki/`，数据库和学习记录在 `.learnloom/`。
- **模型凭据**：API Key 存入 Windows 系统凭据库；换电脑后需重新配置。
- **备份恢复**：在“基础设置”中备份整个知识库到库外空目录；恢复时选择另一个空目录，校验完成后再打开。
- **版本升级**：升级前先整库备份。0.4.1 使用基础数据库版本 2、扩展迁移 5；0.4.0 不能直接打开升级后的库，回退需要升级前的整库备份。
- **卸载**：普通卸载保留知识库；若勾选删除应用数据，配置目录内的数据也会删除。备份应放在配置目录之外。

## 当前边界

- 当前支持 Windows x64 和文本 / Markdown；尚不支持 PDF / OCR、本地模型或自动更新。
- AI 整理、补充和批改可能出错；有效引用与结构校验不能保证所有内容的语义正确。
- 配置 1M 上下文不等于服务已通过 1M 输入满载验证；其他兼容模型服务仍需实际测试。
- 干净 Windows 的依赖安装矩阵、签名及分发验收尚未完成。
- 仓库不包含个人知识库、API 密钥、调用账本、内部验收报告或历史模型输出。产品许可证尚未指定。

## 反馈

遇到问题或有改进建议，欢迎提交 [Issue](https://github.com/qiao11072/Learnloom/issues)。请说明应用版本、复现步骤与错误提示，避免附带 API Key 或个人知识库内容。
