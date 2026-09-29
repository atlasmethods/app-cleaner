//! Row-level cleaning of SQLite databases (browser history, cookies, form data).
//!
//! Reads (analysis) open the file `immutable=1`: no locks, no journal/WAL handling and
//! therefore no side effects on disk at all. Writes open read-write with a zero busy
//! timeout; a locked database means the owning app is running and is reported as
//! [`DbError::InUse`] instead of waiting or forcing anything. All changes happen in one
//! `BEGIN IMMEDIATE` transaction and `VACUUM` runs only after a successful commit.

use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, ErrorCode, OpenFlags};
use std::path::Path;

use crate::features::cleaner::rules::Family;

#[derive(Debug)]
pub enum DbError {
    /// Database is locked (the app is using it).
    InUse,
    Other(String),
}

impl DbError {
    pub fn message(&self) -> String {
        match self {
            DbError::InUse => "in use".to_string(),
            DbError::Other(m) => m.clone(),
        }
    }
}

fn classify(e: rusqlite::Error) -> DbError {
    match &e {
        rusqlite::Error::SqliteFailure(f, _)
            if matches!(f.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) =>
        {
            DbError::InUse
        }
        _ => DbError::Other(e.to_string()),
    }
}

/// "no such table/column" - a schema difference between browser versions, not a failure.
fn is_missing_schema(e: &rusqlite::Error) -> bool {
    match e {
        rusqlite::Error::SqliteFailure(_, Some(msg)) => {
            msg.starts_with("no such table") || msg.starts_with("no such column")
        }
        _ => false,
    }
}

/// `file:` URI for `path` with `immutable=1`; every byte outside `[A-Za-z0-9-._~/:]` is
/// percent-encoded so `?`, `#` and `%` in paths cannot change its meaning.
pub fn immutable_uri(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let s = if s.starts_with('/') {
        s
    } else {
        format!("/{s}")
    };
    let mut out = String::from("file:");
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out.push_str("?immutable=1");
    out
}

fn open_read_only(path: &Path) -> Result<Connection, DbError> {
    Connection::open_with_flags(
        immutable_uri(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(classify)
}

/// Stubs for notification-only SQL functions that Firefox's `places.sqlite` triggers
/// call. Without them a plain DELETE on those tables fails with "no such function".
fn register_stub_functions(conn: &Connection) -> rusqlite::Result<()> {
    for name in [
        "note_sync_change",
        "store_last_inserted_id",
        "notify_frecency",
    ] {
        conn.create_scalar_function(name, -1, FunctionFlags::SQLITE_UTF8, |_ctx| {
            Ok(rusqlite::types::Null)
        })?;
    }
    Ok(())
}

fn open_read_write(path: &Path) -> Result<Connection, DbError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(classify)?;
    conn.busy_timeout(std::time::Duration::from_millis(0))
        .map_err(classify)?;
    register_stub_functions(&conn).map_err(classify)?;
    Ok(conn)
}

fn count(conn: &Connection, q: &str) -> Result<u64, DbError> {
    match conn.query_row(q, [], |r| r.get::<_, Option<i64>>(0)) {
        Ok(v) => Ok(v.unwrap_or(0).max(0) as u64),
        Err(e) if is_missing_schema(&e) => Ok(0),
        Err(e) => Err(classify(e)),
    }
}

/// Number of rows `count_query` reports (a single-value query), read without side effects.
pub fn count_rows(db: &Path, count_query: &str) -> Result<u64, DbError> {
    let conn = open_read_only(db)?;
    count(&conn, count_query)
}

/// What a write-clean did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanOutcome {
    pub rows: u64,
    /// `VACUUM` could not run (data is still removed; the file just did not shrink).
    pub vacuum_skipped: bool,
}

fn finish(conn: &Connection, result: Result<u64, DbError>) -> Result<CleanOutcome, DbError> {
    match result {
        Ok(rows) => {
            if let Err(e) = conn.execute_batch("COMMIT") {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(classify(e));
            }
            let vacuum_skipped = conn.execute_batch("VACUUM").is_err();
            Ok(CleanOutcome {
                rows,
                vacuum_skipped,
            })
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Run `statements` in one transaction; returns rows as counted by `count_query` just
/// before deleting. Statements that hit a missing table/column are skipped (schema
/// differs between browser versions); any other error rolls everything back.
pub fn clean_db(
    db: &Path,
    statements: &[String],
    count_query: &str,
) -> Result<CleanOutcome, DbError> {
    let conn = open_read_write(db)?;
    conn.execute_batch("BEGIN IMMEDIATE").map_err(classify)?;
    let result = (|| {
        let rows = count(&conn, count_query)?;
        for s in statements {
            match conn.execute(s, []) {
                Ok(_) => {}
                Err(e) if is_missing_schema(&e) => {}
                Err(e) => return Err(classify(e)),
            }
        }
        Ok(rows)
    })();
    finish(&conn, result)
}

// ---------------------------------------------------------------- cookies

/// Table and host column for a browser family.
fn cookie_table(family: Family) -> (&'static str, &'static str) {
    match family {
        Family::Chromium => ("cookies", "host_key"),
        Family::Firefox => ("moz_cookies", "host"),
    }
}

/// Normalize a cookie host (`.Example.com` -> `example.com`).
pub fn normalize_host(host: &str) -> String {
    host.trim().trim_start_matches('.').to_ascii_lowercase()
}

/// Does `host` fall under `domain` (equal, or a subdomain of it)? Both normalized.
/// Numeric IPv4 hosts only ever match themselves.
pub fn host_matches_domain(host: &str, domain: &str) -> bool {
    if host == domain {
        return true;
    }
    if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return false;
    }
    host.len() > domain.len()
        && host.ends_with(domain)
        && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
}

pub fn host_matches_any(host: &str, domains: &[String]) -> bool {
    domains.iter().any(|d| host_matches_domain(host, d))
}

/// Per-host cookie counts (raw host strings), read side-effect free.
pub fn cookie_counts(db: &Path, family: Family) -> Result<Vec<(String, u64)>, DbError> {
    let (table, col) = cookie_table(family);
    let conn = open_read_only(db)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {col}, COUNT(*) FROM {table} GROUP BY {col}"
        ))
        .map_err(classify)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(classify)?;
    let mut out = Vec::new();
    for r in rows {
        let (h, c) = r.map_err(classify)?;
        out.push((h, c.max(0) as u64));
    }
    Ok(out)
}

/// Which cookies to delete.
pub enum CookieSelect<'a> {
    /// Everything except hosts under any of these domains.
    AllExcept(&'a [String]),
    /// Only hosts under any of these domains.
    Only(&'a [String]),
}

impl CookieSelect<'_> {
    fn deletes(&self, host: &str) -> bool {
        let h = normalize_host(host);
        match self {
            CookieSelect::AllExcept(keep) => !host_matches_any(&h, keep),
            CookieSelect::Only(del) => host_matches_any(&h, del),
        }
    }
}

/// Number of cookies `select` would delete (side-effect free).
pub fn count_cookies(db: &Path, family: Family, select: &CookieSelect) -> Result<u64, DbError> {
    Ok(cookie_counts(db, family)?
        .into_iter()
        .filter(|(h, _)| select.deletes(h))
        .map(|(_, c)| c)
        .sum())
}

/// Delete the selected cookies in one transaction; returns how many were removed.
pub fn delete_cookies(
    db: &Path,
    family: Family,
    select: &CookieSelect,
) -> Result<CleanOutcome, DbError> {
    let (table, col) = cookie_table(family);
    let conn = open_read_write(db)?;
    conn.execute_batch("BEGIN IMMEDIATE").map_err(classify)?;
    let result = (|| -> Result<u64, DbError> {
        let hosts: Vec<String> = {
            let mut stmt = conn
                .prepare(&format!("SELECT DISTINCT {col} FROM {table}"))
                .map_err(classify)?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(classify)?;
            rows.collect::<Result<_, _>>().map_err(classify)?
        };
        let mut removed = 0u64;
        let mut del = conn
            .prepare(&format!("DELETE FROM {table} WHERE {col} = ?1"))
            .map_err(classify)?;
        for h in hosts.iter().filter(|h| select.deletes(h)) {
            removed += del.execute([h]).map_err(classify)? as u64;
        }
        Ok(removed)
    })();
    finish(&conn, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with(sql: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t.sqlite");
        let c = Connection::open(&p).unwrap();
        c.execute_batch(sql).unwrap();
        (d, p)
    }

    #[test]
    fn uri_encoding_is_safe() {
        let u = immutable_uri(Path::new("/tmp/a b/c#d?e%f.db"));
        assert_eq!(u, "file:/tmp/a%20b/c%23d%3Fe%25f.db?immutable=1");
        assert!(immutable_uri(Path::new(r"C:\Users\x\db")).starts_with("file:/C:/Users/x/db"));
    }

    #[test]
    fn read_only_paths_with_special_chars_work_and_do_not_modify() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("we ird#dir?%");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x y.sqlite");
        {
            let c = Connection::open(&p).unwrap();
            c.execute_batch("CREATE TABLE t(a); INSERT INTO t VALUES (1),(2),(3);")
                .unwrap();
        }
        let before = std::fs::read(&p).unwrap();
        assert_eq!(count_rows(&p, "SELECT COUNT(*) FROM t").unwrap(), 3);
        assert_eq!(std::fs::read(&p).unwrap(), before);
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(names.len(), 1, "no journal/wal files may appear");
    }

    #[test]
    fn missing_table_counts_zero_and_statement_is_skipped() {
        let (_d, p) = db_with("CREATE TABLE a(x); INSERT INTO a VALUES (1),(2);");
        assert_eq!(count_rows(&p, "SELECT COUNT(*) FROM nope").unwrap(), 0);
        let out = clean_db(
            &p,
            &["DELETE FROM nope".into(), "DELETE FROM a".into()],
            "SELECT COUNT(*) FROM a",
        )
        .unwrap();
        assert_eq!(out.rows, 2);
        assert_eq!(count_rows(&p, "SELECT COUNT(*) FROM a").unwrap(), 0);
    }

    #[test]
    fn failing_statement_rolls_everything_back() {
        let (_d, p) = db_with("CREATE TABLE a(x); INSERT INTO a VALUES (1),(2);");
        let r = clean_db(
            &p,
            &["DELETE FROM a".into(), "THIS IS NOT SQL".into()],
            "SELECT COUNT(*) FROM a",
        );
        assert!(matches!(r, Err(DbError::Other(_))));
        assert_eq!(count_rows(&p, "SELECT COUNT(*) FROM a").unwrap(), 2);
    }

    #[test]
    fn locked_database_is_in_use_and_unharmed() {
        let (_d, p) = db_with("CREATE TABLE a(x); INSERT INTO a VALUES (1),(2);");
        let holder = Connection::open(&p).unwrap();
        holder.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let r = clean_db(&p, &["DELETE FROM a".into()], "SELECT COUNT(*) FROM a");
        assert!(matches!(r, Err(DbError::InUse)), "{r:?}");
        let r = delete_cookies(&p, Family::Chromium, &CookieSelect::Only(&[]));
        assert!(matches!(r, Err(DbError::InUse)), "{r:?}");
        holder.execute_batch("ROLLBACK").unwrap();
        drop(holder);
        assert_eq!(count_rows(&p, "SELECT COUNT(*) FROM a").unwrap(), 2);
        let ic: String = Connection::open(&p)
            .unwrap()
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ic, "ok");
    }

    #[test]
    fn host_matching() {
        assert!(host_matches_domain("google.com", "google.com"));
        assert!(host_matches_domain("accounts.google.com", "google.com"));
        assert!(!host_matches_domain("notgoogle.com", "google.com"));
        assert!(!host_matches_domain("google.com.evil.io", "google.com"));
        assert!(!host_matches_domain("com", "google.com"));
        assert!(!host_matches_domain("192.168.1.1", "168.1.1"));
        assert!(host_matches_domain("192.168.1.1", "192.168.1.1"));
        assert_eq!(normalize_host(".GitHub.com "), "github.com");
    }

    const COOKIE_SQL: &str = "CREATE TABLE cookies(host_key TEXT, name TEXT, value TEXT);
        INSERT INTO cookies VALUES ('.google.com','a','1'),('accounts.google.com','b','2'),
        ('google.com','c','3'),('notgoogle.com','d','4'),('.evil.io','e','5'),('.evil.io','f','6');";

    #[test]
    fn chromium_cookie_keep_and_only_selection() {
        let (_d, p) = db_with(COOKIE_SQL);
        let keep = vec!["google.com".to_string()];
        assert_eq!(
            count_cookies(&p, Family::Chromium, &CookieSelect::AllExcept(&keep)).unwrap(),
            3
        );
        let out = delete_cookies(&p, Family::Chromium, &CookieSelect::AllExcept(&keep)).unwrap();
        assert_eq!(out.rows, 3);
        let left: Vec<String> = {
            let c = Connection::open(&p).unwrap();
            let mut s = c
                .prepare("SELECT host_key FROM cookies ORDER BY host_key")
                .unwrap();
            s.query_map([], |r| r.get(0))
                .unwrap()
                .map(|x| x.unwrap())
                .collect()
        };
        assert_eq!(
            left,
            vec![".google.com", "accounts.google.com", "google.com"]
        );

        let out = delete_cookies(
            &p,
            Family::Chromium,
            &CookieSelect::Only(&["google.com".to_string()]),
        )
        .unwrap();
        assert_eq!(out.rows, 3);
        assert_eq!(count_rows(&p, "SELECT COUNT(*) FROM cookies").unwrap(), 0);
    }
}
