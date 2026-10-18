//! Where a line goes: the loader's console, this plugin's own file, and a toast.
//!
//! The file is this plugin's own rather than the shared loader log, because the two are read
//! for different reasons. The loader log is prose for a person working out why something
//! broke; this is one line per event, in a fixed order, for working out what happened in a
//! session — and keeping it separate means it can be tailed, diffed and thrown away without
//! touching the loader's own log.
//!
//! It is opened once and kept open. Opening per line would be tidier to reason about and
//! would also mean a file system call on the game's own thread for every event, which is the
//! one thing a hook on `event.raise` must not do.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Name of the file inside the loader's log directory.
const FILE_NAME: &str = "game-events.log";
/// Lines written before the file is flushed, so a crash loses little without a flush per line.
const FLUSH_EVERY: u64 = 16;

/// The log file, and what is known about why it is not open.
pub struct LogFile {
    path: PathBuf,
    /// `None` once opening has failed: the failure is reported once and then the file is
    /// simply not written, rather than retried on the game's thread every event.
    handle: Mutex<Option<File>>,
    written: Mutex<u64>,
}

impl LogFile {
    /// Open, or append to, the event log beside the loader's own.
    ///
    /// `config_dir` is what the loader hands a plugin — `<game>/uniloader/config` — and the
    /// log directory is its sibling. Derived rather than asked for because the host API offers
    /// the config directory and not the log one, and inventing a path somewhere else would put
    /// this file where nobody looks for it.
    pub fn open(config_dir: &Path) -> LogFile {
        let logs = config_dir
            .parent()
            .map_or_else(|| PathBuf::from("logs"), |root| root.join("logs"));
        let path = logs.join(FILE_NAME);
        let handle = std::fs::create_dir_all(&logs)
            .and_then(|()| OpenOptions::new().create(true).append(true).open(&path));
        let handle = match handle {
            Ok(file) => Some(file),
            Err(e) => {
                log::warn!("cannot write {}: {e}", path.display());
                None
            }
        };
        if handle.is_some() {
            log::info!("writing the event log to {}", path.display());
        }
        LogFile {
            path,
            handle: Mutex::new(handle),
            written: Mutex::new(0),
        }
    }

    /// Where the file is, for the `where` command.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How many lines this session has written.
    pub fn written(&self) -> u64 {
        *self
            .written
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Append one line, giving up on the file if the write fails.
    ///
    /// A failed write closes the file rather than being retried: a full disk or a revoked
    /// handle does not fix itself between two events, and the alternative is an error per
    /// event for the rest of the session.
    pub fn write_line(&self, line: &str) {
        let mut guard = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(file) = guard.as_mut() else {
            return;
        };
        let mut count = self
            .written
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(e) = writeln!(file, "{line}") {
            log::warn!("giving up on {}: {e}", self.path.display());
            *guard = None;
            return;
        }
        *count += 1;
        if count.is_multiple_of(FLUSH_EVERY) {
            let _ = file.flush();
        }
    }

    /// Flush what is buffered, for `stop` and for the `flush` command.
    pub fn flush(&self) {
        let mut guard = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(file) = guard.as_mut() {
            let _ = file.flush();
        }
    }
}

/// Seconds and milliseconds since the Unix epoch, as `<seconds>.<millis>`.
///
/// A wall clock date would be friendlier to read and needs a date library to format; this is
/// sortable, diffable and exact, which is what a line in an event log is for. The loader's own
/// log carries the human readable time for the same session.
pub fn stamp() -> String {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}", since.as_secs(), since.subsec_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_is_seconds_and_three_millisecond_digits() {
        let stamp = stamp();
        let Some((seconds, millis)) = stamp.split_once('.') else {
            panic!("a stamp has one dot: {stamp}");
        };
        assert!(seconds.parse::<u64>().is_ok(), "{stamp}");
        assert_eq!(millis.len(), 3, "{stamp}");
        assert!(millis.parse::<u16>().is_ok_and(|m| m < 1000), "{stamp}");
    }

    #[test]
    fn the_log_lands_beside_the_loader_log_rather_than_in_the_config_directory() {
        let dir = std::env::temp_dir().join("dayz-log-plugin-test-paths");
        let config = dir.join("uniloader").join("config");
        let file = LogFile::open(&config);
        assert_eq!(
            file.path().file_name().and_then(|n| n.to_str()),
            Some(FILE_NAME)
        );
        assert_eq!(
            file.path().parent(),
            Some(dir.join("uniloader").join("logs").as_path())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lines_are_counted_and_an_unopenable_file_is_simply_not_written() {
        let dir = std::env::temp_dir().join("dayz-log-plugin-test-write");
        let _ = std::fs::remove_dir_all(&dir);
        let config = dir.join("uniloader").join("config");
        let file = LogFile::open(&config);
        file.write_line("one");
        file.write_line("two");
        file.flush();
        assert_eq!(file.written(), 2);
        let written = std::fs::read_to_string(file.path()).unwrap_or_default();
        assert_eq!(written, "one\ntwo\n");

        // A directory where the file should be: opening fails, and writing is a no-op rather
        // than an error per line.
        let dir2 = std::env::temp_dir().join("dayz-log-plugin-test-blocked");
        let _ = std::fs::remove_dir_all(&dir2);
        let logs = dir2.join("uniloader").join("logs");
        let _ = std::fs::create_dir_all(logs.join(FILE_NAME));
        let blocked = LogFile::open(&dir2.join("uniloader").join("config"));
        blocked.write_line("nowhere");
        assert_eq!(blocked.written(), 0);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }
}
