use rusqlite::Connection;

const DB_PATH: &str = "koo_editor.db";

pub fn open() -> rusqlite::Result<Connection> {
    let conn = Connection::open(DB_PATH)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    Ok(conn)
}
