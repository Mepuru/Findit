//! 全局数据库连接管理：写连接 `Mutex<Connection>` + 专用读连接（P-L7）。
//!
//! 数据库文件路径由 Flutter 初始化时通过 [`init_db`] 传入（Dart 侧用
//! `path_provider` 取应用文档目录）。未初始化时调用 [`with_conn`] /
//! [`with_read_conn`] 会返回 [`FinditError::DbNotInitialized`]。

pub mod migrations;

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

use rusqlite::Connection;

use crate::core::error::{FinditError, FinditResult};

/// 全局写连接。`rusqlite::Connection` 是 `Send` 但非 `Sync`，
/// 用 `Mutex` 包裹后即为 `Sync`，可作为全局静态量。
///
/// P-L7 取舍说明：真正的「读连接池 + 写连接」拆分需要把全部读路径
/// （语义/关键词搜索、列表加载等）迁移到读连接上，涉及 `core/search/*`、
/// `core/repo/*`、`api/*` 多处他线文件，超出本任务文件所有权。当前落地：
/// ① 专用读连接 [`with_read_conn`] 已在 [`init_db`] 建立（WAL 下与写连接
/// 并行读，互不阻塞）；② `PRAGMA cache_size` 调优。业务调用仍走
/// [`with_conn`]，读连接作为拆分基础设施保留，热读路径可增量迁移。
static DB: LazyLock<Mutex<Option<Connection>>> = LazyLock::new(|| Mutex::new(None));

/// 专用读连接（P-L7）：WAL 模式下与写连接共库，读事务不阻塞写、写不阻塞读。
/// 生命周期与 [`DB`] 同步（[`init_db`] 建立 / [`close_db`] 关闭）。
static READ_DB: LazyLock<Mutex<Option<Connection>>> = LazyLock::new(|| Mutex::new(None));

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

    let (conn, read_conn) = open_db_connections(&db_path)?;

    // 语义搜索内存索引缓存随库重置（新库/恢复库的向量内容不同）。
    crate::core::search::semantic::invalidate_embedding_cache();

    let mut guard = lock_db();
    *guard = Some(conn);
    *lock_read_db() = Some(read_conn);
    Ok(())
}

/// 打开写连接（PRAGMA + 迁移 + 索引）与专用读连接（P-L7）。
///
/// 纯函数（不触碰全局状态，便于测试）：`db_path` 指向库文件。
/// 写连接侧完成：WAL / synchronous / busy_timeout / foreign_keys /
/// cache_size 调优（P-L7）、迁移、FTS5 索引、待回填部分索引（P-L2）；
/// 读连接侧完成：busy_timeout / foreign_keys / cache_size（journal_mode
/// 为库级持久属性，无需重复设置）。
fn open_db_connections(db_path: &Path) -> FinditResult<(Connection, Connection)> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // P-L7：页缓存调优（负值 = KiB；-16384 ≈ 16 MiB），缓解语义全量读
    // 向量等场景的缓存抖动。
    conn.pragma_update(None, "cache_size", -16384)?;

    migrations::run_migrations(&conn)?;

    // 关键词搜索的 FTS5 索引（幂等建表 + 回填存量；恢复旧库后同样适用）。
    crate::core::search::keyword::ensure_fts_schema(&conn)?;

    // P-L2：待回填扫描的部分索引（幂等；新库/存量库/恢复库均生效）。
    // 全量向量回填的待办扫描 `WHERE embedding IS NULL ORDER BY id` 由
    // 近似 O(N²) 行访问降为 O(N)。items 表已由迁移创建，此处必然可用。
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_items_pending_embedding \
         ON items(id) WHERE embedding IS NULL;",
    )?;

    // P-L7：建立专用读连接（与写连接同库并行读；journal_mode 为库级持久
    // 属性，读连接无需重复设置）。读连接同样调优页缓存，以缓解语义
    // 全量读向量等热读路径的缓存抖动。
    let read_conn = Connection::open(db_path)?;
    read_conn.pragma_update(None, "busy_timeout", 5000)?;
    read_conn.pragma_update(None, "foreign_keys", "ON")?;
    read_conn.pragma_update(None, "cache_size", -16384)?;

    Ok((conn, read_conn))
}

/// 关闭当前连接（主要用于测试与重置）。
pub fn close_db() {
    let mut guard = lock_db();
    *guard = None;
    // 读连接与写连接同生命周期（P-L7）。
    *lock_read_db() = None;
    *lock_photos_dir() = None;
    // 语义搜索内存索引缓存随库关闭失效（恢复流程会重开新库）。
    crate::core::search::semantic::invalidate_embedding_cache();
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

fn lock_read_db<'a>() -> MutexGuard<'a, Option<Connection>> {
    READ_DB.lock().expect("读连接互斥锁中毒")
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

/// 在专用读连接上执行只读逻辑（P-L7：WAL 下读写连接拆分）。
///
/// 与 [`with_conn`] 互不阻塞；调用方必须保证闭包内不执行写语句
/// （SQLite 对同库多连接写入需抢占写锁，读连接上写会退化为串行且易
/// 触发 `database is locked`，只读场景才应使用本函数）。
pub fn with_read_conn<F, R>(f: F) -> FinditResult<R>
where
    F: FnOnce(&Connection) -> FinditResult<R>,
{
    let guard = lock_read_db();
    match guard.as_ref() {
        Some(conn) => f(conn),
        None => Err(FinditError::DbNotInitialized),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时目录，测试结束自动清理。
    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "findit-db-test-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// P-L7 / P-L2：读写连接拆分基础设施与索引/缓存调优的回归测试。
    ///
    /// 直接测 [`open_db_connections`]（纯函数），避免触碰全局静态量
    /// （`init_db`/`close_db` 会与 `photo::tests::item_photo_lifecycle`
    /// 等全局状态用例并行竞争）。
    #[test]
    fn db_split_connections_share_committed_data() {
        let tmp = TempDir::new();
        let db_path = tmp.0.join(DB_FILE_NAME);
        let (write_conn, read_conn) = open_db_connections(&db_path).unwrap();

        // 写连接可用（建表 + 写入）。
        write_conn
            .execute(
                "CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, v TEXT)",
                [],
            )
            .unwrap();
        write_conn.execute("INSERT INTO t (v) VALUES ('a')", []).unwrap();

        // 读连接能看到写连接提交的数据（WAL 下读快照最新提交）。
        let n: i64 = read_conn
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);

        // P-L2：部分索引已建立。
        let idx: i64 = read_conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'index' AND name = 'idx_items_pending_embedding'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1, "应已创建待回填部分索引");

        // P-L7：cache_size 已调优（负值 KiB → 绝对值 = KiB 数）。
        let cache: i64 = read_conn
            .query_row("PRAGMA cache_size", [], |r| r.get(0))
            .unwrap();
        assert!(
            cache < 0 && cache.abs() >= 16384,
            "cache_size 应不小于 16MiB，实际 {cache}"
        );
    }
}
