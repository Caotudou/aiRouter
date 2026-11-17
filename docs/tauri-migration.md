# Tauri + Rust 迁移

0.2.0 已接入桌面控制台、本机 HTTP 服务、Codex app-server、流式输出、多轮会话和 WorkBuddy 动态工具。Windows 会迁移旧版设置和本机 API Key，旧版保留。默认模型为 `gpt-6.1-sol`，默认推理强度为 `high`。

Windows 已构建发布版并验证窗口账号状态、服务启停、旧密钥访问、真实文本和流式请求；模拟 app-server 验证了并行工具和错误处理。此前已通过真实 Codex 工具调用往返测试。仍需在 WorkBuddy 客户端中完成实际任务级联调，并在 Mac 上构建与验证。Codex 用量受账户限制。

运行、兼容范围和构建方法见项目 README。当前 Windows 发布文件是独立 exe，使用系统 WebView2 Runtime；macOS 源码包含程序发现与钥匙串支持，尚未产出 macOS 安装包。
