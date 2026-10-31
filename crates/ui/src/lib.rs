// Keep the queued IPC protocol independently testable without the executable's
// Swift converter linkage and WebView startup.
pub mod ipc;
