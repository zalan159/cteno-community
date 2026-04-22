//! File Read Timestamp Tracker
//!
//! Tracks the mtime of files when they are read, so that write/edit tools
//! can detect if a file was externally modified between read and write.
//! This prevents agents from accidentally overwriting changes made by
//! linters, formatters, or the user.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::SystemTime;

/// Global file read timestamp registry.
///
/// Key: `(session_id, canonical_file_path)` — session-scoped to avoid
/// cross-session interference.
/// Value: mtime at the time the file was last read by that session.
static FILE_READ_TIMESTAMPS: Mutex<Option<HashMap<(String, String), SystemTime>>> =
    Mutex::new(None);

fn with_map<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<(String, String), SystemTime>) -> R,
{
    let mut guard = FILE_READ_TIMESTAMPS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

/// Record the mtime of a file at the time it was read.
pub fn record_file_read(session_id: &str, path: &str, mtime: SystemTime) {
    let key = (session_id.to_string(), path.to_string());
    with_map(|map| {
        map.insert(key, mtime);
    });
}

/// Get the recorded mtime for a file from when it was last read by the given session.
pub fn get_file_read_time(session_id: &str, path: &str) -> Option<SystemTime> {
    let key = (session_id.to_string(), path.to_string());
    with_map(|map| map.get(&key).copied())
}

/// Clear all tracked timestamps for a session (call on session teardown).
pub fn clear_session_timestamps(session_id: &str) {
    with_map(|map| {
        map.retain(|(sid, _), _| sid != session_id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    // Tests use unique session/path prefixes to avoid global-state interference
    // when Rust runs tests in parallel within the same process.

    #[test]
    fn test_record_and_get() {
        let now = SystemTime::now();
        record_file_read("rg-sess-1", "/tmp/rg-a.txt", now);
        assert_eq!(get_file_read_time("rg-sess-1", "/tmp/rg-a.txt"), Some(now));
        assert_eq!(get_file_read_time("rg-sess-2", "/tmp/rg-a.txt"), None);
        assert_eq!(get_file_read_time("rg-sess-1", "/tmp/rg-b.txt"), None);
    }

    #[test]
    fn test_overwrite() {
        let t1 = SystemTime::now();
        let t2 = t1 + Duration::from_secs(5);
        record_file_read("ow-sess-1", "/tmp/ow-a.txt", t1);
        record_file_read("ow-sess-1", "/tmp/ow-a.txt", t2);
        assert_eq!(get_file_read_time("ow-sess-1", "/tmp/ow-a.txt"), Some(t2));
    }

    #[test]
    fn test_clear_session() {
        let now = SystemTime::now();
        record_file_read("cl-sess-1", "/tmp/cl-a.txt", now);
        record_file_read("cl-sess-1", "/tmp/cl-b.txt", now);
        record_file_read("cl-sess-2", "/tmp/cl-a.txt", now);

        clear_session_timestamps("cl-sess-1");

        assert_eq!(get_file_read_time("cl-sess-1", "/tmp/cl-a.txt"), None);
        assert_eq!(get_file_read_time("cl-sess-1", "/tmp/cl-b.txt"), None);
        assert_eq!(get_file_read_time("cl-sess-2", "/tmp/cl-a.txt"), Some(now));
    }
}
