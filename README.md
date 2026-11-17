# AI Router

Tauri + Rust 桌面应用，将本机已通过 ChatGPT 登录的 Codex 接到腾讯 WorkBuddy 的自定义模型入口。Windows 版本已构建和验证；macOS 源码已适配，尚未在 Mac 上构建或测试。

```text
WorkBuddy → http://127.0.0.1:18321/v1 → AI Router → codex app-server → OpenAI
```

使用同一账户的 Codex 用量窗口；不会把用量转换或充值到 WorkBuddy 账户。模型推理需要网络。

## 使用

1. 在本机 Codex 中使用 ChatGPT 登录。终端运行 `codex login status` 可检查状态。
2. 打开 `dist/AI-Router-0.2.6-Tauri-Windows.exe`。Windows 需有 Microsoft Edge WebView2 Runtime。
3. 在 AI Router 点击 **启动服务**，再点击 **测试连接**。测试会产生一条真实 Codex 请求。
4. WorkBuddy → 设置 → 模型 → 自定义 API / Custom，选择 OpenAI 兼容协议：

| 配置项 | 填写内容 |
|---|---|
| Base URL | `http://127.0.0.1:18321/v1` |
| API Key | 点击 AI Router 的 **复制** 获取本机密钥 |
| 模型名称 | `codex-local`，跟随 AI Router 中选择的默认模型 |
| 默认推理强度 | 在 AI Router 中选择；客户端不填写时使用，初始为高（`high`） |
| 工具调用 | 开启 |
| 图片输入 | 需要时开启，取决于选用模型 |
| 自定义协议开关 | 保持关闭，使用标准 `/chat/completions` 路径 |

如果 WorkBuddy 开启了自定义协议并要求完整请求地址，填写 `http://127.0.0.1:18321/v1/chat/completions`；不要让客户端重复拼接 `/v1`。

在“模型与推理强度”中选择当前账户目录里的模型和它支持的强度，点击“应用选择”后新请求立即生效。也可以在 WorkBuddy 填写具体模型名，届时客户端指定的模型优先；客户端明确指定的推理强度也会优先。目录不代表所有请求均获授权，实际推理结果决定是否可用。

关闭 AI Router 窗口即停止服务。首版没有系统托盘、后台常驻、开机启动、多账户或局域网分享。

## 当前验证情况

- Windows 本机 Codex `0.169.0-alpha.11.1`：ChatGPT 登录状态、模型目录、用量查询通过。
- 真实接口测试：文本回答、SSE 流式输出、动态工具调用及工具结果回传通过。
- Rust HTTP 测试使用模拟 Codex 覆盖鉴权、会话延续、并行工具结果、异常和 SSE；旧版 JavaScript 测试单独保留。
- Windows Tauri 窗口已验证账号读取、服务启动、停止和重启；旧版 WorkBuddy 的本机密钥已验证可访问新版模型列表。
- **尚未在 WorkBuddy 客户端内完成端到端联调**。本版本是可运行的协议适配实验版，不保证所有 WorkBuddy 工具与模型参数兼容。

## 接口和兼容范围

- `GET /v1/models`：模型目录，包含指向当前所选默认模型的别名 `codex-local`。不指定模型时同样使用所选默认模型。
- `POST /v1/chat/completions`：文本、`image_url` 图片、非流式及 SSE 流式、`function` 工具。
- `GET /health`：进程健康状态。上述接口均需要 `Authorization: Bearer <本机密钥>`。
- WorkBuddy 的工具映射为 Codex `dynamicTools`；调用由 WorkBuddy 执行，结果通过原始 app-server 请求回传。单次可返回多个工具调用。
- 活动工具调用按调用 ID 找回会话，允许客户端调整历史消息。会话仅在内存中保存，闲置 10 分钟清理，最多 24 个。服务重启或会话清理后，无法恢复原生工具调用，会将客户端提供的历史作为文本重建上下文；这不是原生会话的完整恢复。
- 新请求自带的旧 assistant 消息会作为带角色标记的文本上下文传入；已经匹配的会话沿用 Codex 原生历史。首版不提供任意历史的原始 Responses item 导入。
- 支持 `tool_choice=auto/none`；暂不支持强制工具、音频、Responses API、JSON Schema 输出、`n>1`、logprobs、seed、stop。
- `reasoning_effort` 使用界面所选默认值，初始为 `high`；客户端显式传入时采用客户端值，Codex 负责判断是否支持。`temperature`、`top_p`、`max_tokens`、`max_completion_tokens` 等采样/输出上限参数不映射，实际生成遵循 Codex；不要用这些字段作为费用上限。
- 180 秒为单次接口响应超时；客户端断开会取消该会话的 Codex 任务。
- Codex app-server 和动态工具属于实验接口，升级 Codex 后应重新运行真实冒烟测试。

## 本机数据

Tauri 版设置存放在系统应用数据目录，本机 API Key 保存于 Windows 凭据管理器或 macOS 钥匙串。Windows 首次运行会读取旧版设置和本机密钥，旧版数据不会删除。服务只监听 `127.0.0.1`，拒绝网页 Origin；不保存提示词、模型正文或 Codex 登录令牌。界面运行记录保存在内存；会话匹配诊断日志写入应用数据目录的 `logs/session-events.jsonl`，只包含数量、匹配结果和时间等元数据，不包含消息正文、工具参数或密钥；达到 1 MiB 时轮转并保留一份旧文件。

Codex 仍负责其自身凭据、令牌刷新及本机账户数据。桥接会话使用 `ephemeral`，工具请求仅保留内存状态。底层 Codex 自身的诊断/存储行为由其实现决定。

默认不给 Codex 会话选择电脑执行环境，使用只读策略并关闭 shell 等能力；本机文件与命令任务应交回 WorkBuddy 自身的工具执行。

## 开发

需要 Node.js 22+、npm、Rust 和平台的 Tauri 构建依赖，以及本机 Codex。应用会自动检查 PATH、本机常见目录和 Microsoft Store 的 Codex 安装位置，不依赖开发终端注入的 PATH。连接设置中可查看实际使用的程序路径，也可手动指定 `codex.exe`。若只有 npm 的 `codex.cmd` 启动器，请选择实际 `codex.exe`，或使用 `CODEX_BIN` 配置无界面服务和测试。

```powershell
npm install
npm run tauri:dev
npm run tauri:build
cargo test --manifest-path src-tauri/Cargo.toml
cargo build --manifest-path src-tauri/Cargo.toml --example fake_codex --example server_smoke
npm run test:rust-bridge
npm test             # 原 Electron 版本测试
```

Windows 的 `npm run tauri:build` 生成对应安装包；macOS 需在 Mac 上安装 Xcode 和 Rust 后执行，签名、公证和实际功能尚待验证。旧版 Electron 的 `npm start`、`npm run dist` 和 `npm run service` 仍保留。

## 官方协议资料

- [WorkBuddy 模型配置](https://www.workbuddy.cn/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Model)
- [Codex app-server](https://learn.chatgpt.com/docs/app-server)

本项目是独立的本机集成工具，不代表腾讯或 OpenAI 官方产品。
