//! 全局单连接管理：`Mutex<Connection>`。
//!
//! 数据库文件路径由 Flutter 初始化时通过 [`init_db`] 传入（Dart 侧用
//! `path_provider` 取应用文档目录）。未初始化时调用 [`with_conn`] 会返回
//! [`FinditError::DbNotInitialized`]。

pub mod migrations;

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

use rusqlite::Connection;

use crate::core::error::{FinditError, FinditResult};

/// 全局唯一的数据库连接。`rusqlite::Connection` 是 `Send` 但非 `Sync`，
/// 用 `Mutex` 包裹后即为 `Sync`，可作为全局静态量。
static DB: LazyLock<Mutex<Option<Connection>>> = LazyLock::new(|| Mutex::new(None));

/// 照片存放目录（跟随 [`init_db`] 初始化，为 `db_dir/photos`）。
static PHOTOS_DIR: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

/// 数据库所在目录（跟随 [`init_db`] 初始化；备份恢复后据此重开连接）。
static DB_DIR: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

/// 数据库文件名（位于传入的目录下）。
pub const DB_FILE_NAME: &str = "findit.db";

/// 照片子目录名（位于 `db_dir` 下）。
pub const PHOTOS_DIR_NAME: &str = "photos";

/// 打开（或创建）数据库并应用迁移。
///
/// `db_dir` 为目录路径；不存在时会被创建。重复调用会替换当前连接。
/// 同时初始化照片目录 `db_dir/photos`（不存在则创建）。
pub fn init_db(db_dir: &str) -> FinditResult<()> {
    let dir = Path::new(db_dir);
    std::fs::create_dir_all(dir)?;
    let db_path = dir.join(DB_FILE_NAME);

    let photos_dir = dir.join(PHOTOS_DIR_NAME);
    std::fs::create_dir_all(&photos_dir)?;
    *lock_photos_dir() = Some(photos_dir);
    *lock_db_dir() = Some(dir.to_path_buf());

    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    migrations::run_migrations(&conn)?;

    let mut guard = lock_db();
    *guard = Some(conn);
    Ok(())
}

/// 关闭当前连接（主要用于测试与重置）。
pub fn close_db() {
    let mut guard = lock_db();
    *guard = None;
    *lock_photos_dir() = None;
    // 注意：DB_DIR 保留，供备份恢复失败后重新打开原库使用。
}

/// 直接放入一个已配置好的连接（仅供测试使用）。
#[cfg(test)]
pub fn set_conn_for_test(conn: Connection) {
    let mut guard = lock_db();
    *guard = Some(conn);
}

/// 设置照片目录（仅供测试使用）。
#[cfg(test)]
pub fn set_photos_dir_for_test(dir: PathBuf) {
    *lock_photos_dir() = Some(dir);
}

fn lock_photos_dir<'a>() -> MutexGuard<'a, Option<PathBuf>> {
    PHOTOS_DIR.lock().expect("照片目录互斥锁中毒")
}

fn lock_db_dir<'a>() -> MutexGuard<'a, Option<PathBuf>> {
    DB_DIR.lock().expect("数据库目录互斥锁中毒")
}

/// 当前数据库目录。未通过 [`init_db`] 初始化时返回 [`FinditError::DbNotInitialized`]。
pub fn db_dir() -> FinditResult<PathBuf> {
    let guard = lock_db_dir();
    guard
        .clone()
        .ok_or(FinditError::DbNotInitialized)
}

/// 当前照片目录。未通过 [`init_db`] 初始化时返回 [`FinditError::DbNotInitialized`]。
pub fn photos_dir() -> FinditResult<PathBuf> {
    let guard = lock_photos_dir();
    guard
        .clone()
        .ok_or(FinditError::DbNotInitialized)
}

fn lock_db<'a>() -> MutexGuard<'a, Option<Connection>> {
    DB.lock().expect("数据库互斥锁中毒")
}

/// 在持有全局连接的情况下执行业务逻辑。
///
/// 注意：闭包在持锁期间运行，业务逻辑不应在内部长时间阻塞。
pub fn with_conn<F, R>(f: F) -> FinditResult<R>
where
    F: FnOnce(&Connection) -> FinditResult<R>,
{
    let guard = lock_db();
    match guard.as_ref() {
        Some(conn) => f(conn),
        None => Err(FinditError::DbNotInitialized),
    }
}
