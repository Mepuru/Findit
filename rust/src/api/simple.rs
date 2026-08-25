use crate::core::error::FinditError;

/// FRB 初始化钩子：在 `RustLib.init()` 时自动执行。
///
/// 返回 `Result<bool, FinditError>` 而非 `()`：FRB 2.11 codegen 不会为 `()`
/// 生成 `impl SseEncode`（init 线代码的 Err 侧恒为 `()` 或返回类型），
/// 导致生成的线代码无法编译；用该签名规避，Dart 侧忽略返回值。
#[flutter_rust_bridge::frb(init)]
pub fn init_app() -> Result<bool, FinditError> {
    // 默认工具（日志等），可按需扩展。
    flutter_rust_bridge::setup_default_user_utils();
    Ok(true)
}
