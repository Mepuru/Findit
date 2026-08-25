//! 应用设置（app_settings）键值对。

use rusqlite::{params, Connection};

use crate::core::error::{FinditError, FinditResult};

/// 读取设置项；不存在返回 `None`。
pub fn get_setting(conn: &Connection, key: &str) -> FinditResult<Option<String>> {
    let key = key.trim();
    if key.is_empty() {
        return Err(FinditError::Validation("设置键不能为空".to_string()));
    }
    let result = conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    );
    match result {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 写入设置项（存在则覆盖）。
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> FinditResult<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(FinditError::Validation("设置键不能为空".to_string()));
    }
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db::migrations::run_migrations;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn get_set_roundtrip_and_overwrite() {
        let conn = setup();
        assert_eq!(get_setting(&conn, "ollama_url").unwrap(), None);

        set_setting(&conn, "ollama_url", "http://192.168.1.10:11434").unwrap();
        assert_eq!(
            get_setting(&conn, "ollama_url").unwrap(),
            Some("http://192.168.1.10:11434".to_string())
        );

        set_setting(&conn, "ollama_url", "http://localhost:11434").unwrap();
        assert_eq!(
            get_setting(&conn, "ollama_url").unwrap(),
            Some("http://localhost:11434".to_string())
        );
    }

    #[test]
    fn empty_key_rejected() {
        let conn = setup();
        assert!(matches!(
            get_setting(&conn, "  "),
            Err(FinditError::Validation(_))
        ));
        assert!(matches!(
            set_setting(&conn, "", "v"),
            Err(FinditError::Validation(_))
        ));
    }
}
