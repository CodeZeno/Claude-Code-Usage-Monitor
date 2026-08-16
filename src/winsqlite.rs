//! Minimal read-only wrapper around SQLite shipped with Windows 10 and 11.
//!
//! Keep this intentionally narrow: the monitor only needs to retrieve one text
//! value from an application-owned database. Linking as a raw DLL import avoids
//! bundling SQLite or depending on a Windows SDK import library at build time.

use std::ffi::{c_char, c_int, c_uchar, CStr, CString};
use std::fmt;
use std::path::Path;
use std::ptr;

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_OPEN_READ_ONLY: c_int = 0x0000_0001;

#[repr(C)]
struct Sqlite3 {
    _private: [u8; 0],
}

#[repr(C)]
struct Sqlite3Stmt {
    _private: [u8; 0],
}

#[link(name = "winsqlite3", kind = "raw-dylib")]
unsafe extern "C" {
    fn sqlite3_open_v2(
        filename: *const c_char,
        database: *mut *mut Sqlite3,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close(database: *mut Sqlite3) -> c_int;
    fn sqlite3_errmsg(database: *mut Sqlite3) -> *const c_char;
    fn sqlite3_busy_timeout(database: *mut Sqlite3, milliseconds: c_int) -> c_int;
    fn sqlite3_prepare_v2(
        database: *mut Sqlite3,
        sql: *const c_char,
        sql_bytes: c_int,
        statement: *mut *mut Sqlite3Stmt,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_bind_text(
        statement: *mut Sqlite3Stmt,
        index: c_int,
        value: *const c_char,
        value_bytes: c_int,
        destructor: Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
    ) -> c_int;
    fn sqlite3_step(statement: *mut Sqlite3Stmt) -> c_int;
    fn sqlite3_column_text(statement: *mut Sqlite3Stmt, column: c_int) -> *const c_uchar;
    fn sqlite3_column_bytes(statement: *mut Sqlite3Stmt, column: c_int) -> c_int;
    fn sqlite3_finalize(statement: *mut Sqlite3Stmt) -> c_int;

    #[cfg(test)]
    fn sqlite3_exec(
        database: *mut Sqlite3,
        sql: *const c_char,
        callback: Option<
            unsafe extern "C" fn(
                *mut std::ffi::c_void,
                c_int,
                *mut *mut c_char,
                *mut *mut c_char,
            ) -> c_int,
        >,
        context: *mut std::ffi::c_void,
        error_message: *mut *mut c_char,
    ) -> c_int;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

struct Connection {
    raw: *mut Sqlite3,
}

impl Connection {
    fn open_read_only(path: &Path) -> Result<Self, Error> {
        let filename = CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| Error("SQLite database path contains a NUL byte".into()))?;
        let mut raw = ptr::null_mut();
        let result = unsafe {
            sqlite3_open_v2(
                filename.as_ptr(),
                &mut raw,
                SQLITE_OPEN_READ_ONLY,
                ptr::null(),
            )
        };
        if result == SQLITE_OK && !raw.is_null() {
            let connection = Self { raw };
            let result = unsafe { sqlite3_busy_timeout(connection.raw, 1_000) };
            if result != SQLITE_OK {
                return Err(error_message(
                    connection.raw,
                    "unable to set SQLite busy timeout",
                    result,
                ));
            }
            return Ok(connection);
        }

        let error = error_message(raw, "unable to open SQLite database", result);
        if !raw.is_null() {
            unsafe {
                let _ = sqlite3_close(raw);
            }
        }
        Err(error)
    }

    fn prepare(&self, sql: &str) -> Result<Statement<'_>, Error> {
        let sql =
            CString::new(sql).map_err(|_| Error("SQLite statement contains a NUL byte".into()))?;
        let mut raw = ptr::null_mut();
        let result =
            unsafe { sqlite3_prepare_v2(self.raw, sql.as_ptr(), -1, &mut raw, ptr::null_mut()) };
        if result != SQLITE_OK || raw.is_null() {
            return Err(error_message(
                self.raw,
                "unable to prepare SQLite statement",
                result,
            ));
        }
        Ok(Statement {
            raw,
            connection: self,
        })
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        unsafe {
            let _ = sqlite3_close(self.raw);
        }
    }
}

struct Statement<'connection> {
    raw: *mut Sqlite3Stmt,
    connection: &'connection Connection,
}

impl Statement<'_> {
    fn bind_text(&mut self, index: c_int, value: &CStr) -> Result<(), Error> {
        let value_bytes = c_int::try_from(value.to_bytes().len())
            .map_err(|_| Error("SQLite parameter is too large".into()))?;
        // SQLITE_STATIC is safe here because the caller keeps `value` alive
        // until after sqlite3_step returns.
        let result =
            unsafe { sqlite3_bind_text(self.raw, index, value.as_ptr(), value_bytes, None) };
        if result == SQLITE_OK {
            Ok(())
        } else {
            Err(error_message(
                self.connection.raw,
                "unable to bind SQLite parameter",
                result,
            ))
        }
    }

    fn optional_text(&mut self, column: c_int) -> Result<Option<String>, Error> {
        match unsafe { sqlite3_step(self.raw) } {
            SQLITE_DONE => Ok(None),
            SQLITE_ROW => {
                let text = unsafe { sqlite3_column_text(self.raw, column) };
                if text.is_null() {
                    return Ok(None);
                }
                let bytes = unsafe { sqlite3_column_bytes(self.raw, column) };
                let bytes = usize::try_from(bytes)
                    .map_err(|_| Error("SQLite returned an invalid text length".into()))?;
                let value = unsafe { std::slice::from_raw_parts(text, bytes) };
                String::from_utf8(value.to_vec())
                    .map(Some)
                    .map_err(|_| Error("SQLite returned text that is not UTF-8".into()))
            }
            result => Err(error_message(
                self.connection.raw,
                "unable to read SQLite row",
                result,
            )),
        }
    }
}

impl Drop for Statement<'_> {
    fn drop(&mut self) {
        unsafe {
            let _ = sqlite3_finalize(self.raw);
        }
    }
}

fn error_message(database: *mut Sqlite3, context: &str, result: c_int) -> Error {
    let detail = if database.is_null() {
        None
    } else {
        let message = unsafe { sqlite3_errmsg(database) };
        (!message.is_null()).then(|| unsafe { CStr::from_ptr(message) }.to_string_lossy())
    };
    match detail {
        Some(detail) => Error(format!("{context} ({result}): {detail}")),
        None => Error(format!("{context} ({result})")),
    }
}

/// Query column zero from the first row of a read-only, one-parameter query.
pub(crate) fn query_optional_text(
    path: &Path,
    sql: &str,
    parameter: &str,
) -> Result<Option<String>, Error> {
    let parameter = CString::new(parameter)
        .map_err(|_| Error("SQLite parameter contains a NUL byte".into()))?;
    let connection = Connection::open_read_only(path)?;
    let mut statement = connection.prepare(sql)?;
    statement.bind_text(1, &parameter)?;
    statement.optional_text(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    const SQLITE_OPEN_READ_WRITE: c_int = 0x0000_0002;
    const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;

    #[test]
    fn reads_an_optional_text_value_through_windows_sqlite() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "claude-code-usage-monitor-winsqlite-{}-{unique}.db",
            std::process::id()
        ));
        create_database(&path);

        let query = "SELECT value FROM ItemTable WHERE key = ?1";
        assert_eq!(
            query_optional_text(&path, query, "cursorAuth/accessToken").unwrap(),
            Some("test-token".into())
        );
        assert_eq!(query_optional_text(&path, query, "missing").unwrap(), None);

        std::fs::remove_file(path).unwrap();
    }

    fn create_database(path: &Path) {
        let filename = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        let mut database = ptr::null_mut();
        let result = unsafe {
            sqlite3_open_v2(
                filename.as_ptr(),
                &mut database,
                SQLITE_OPEN_READ_WRITE | SQLITE_OPEN_CREATE,
                ptr::null(),
            )
        };
        assert_eq!(result, SQLITE_OK);
        let sql = CString::new(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);\
             INSERT INTO ItemTable VALUES ('cursorAuth/accessToken', 'test-token');",
        )
        .unwrap();
        let result = unsafe {
            sqlite3_exec(
                database,
                sql.as_ptr(),
                None,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        assert_eq!(result, SQLITE_OK);
        assert_eq!(unsafe { sqlite3_close(database) }, SQLITE_OK);
    }
}
