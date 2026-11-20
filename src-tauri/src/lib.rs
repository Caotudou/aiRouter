//! Cross-platform desktop controller, local API, and Codex protocol bridge.
pub mod bridge;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod diagnostics;
pub mod discovery;
#[cfg(all(feature = "desktop", windows))]
pub mod legacy_windows;
pub mod protocol;
pub mod rpc;
pub mod server;
pub mod usage;
