use std::{
    fs::{File, OpenOptions},
    io::Write,
    sync::{Mutex, OnceLock},
};

use log::{Level, LevelFilter, Log, Metadata, Record};

static INIT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();
static FILE_LOGGER: FileLogger = FileLogger {
    file: Mutex::new(None),
};

struct FileLogger {
    file: Mutex<Option<File>>,
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        if cfg!(debug_assertions) {
            metadata.level() <= Level::Trace
        } else {
            metadata.level() <= Level::Info
        }
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let level = match record.level() {
            Level::Trace => "trace",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        };

        let mut guard = self.file.lock().unwrap();
        if let Some(file) = guard.as_mut() {
            let _ = writeln!(file, "{}: {}", level, record.args());
        } else {
            eprintln!("{}: {}", level, record.args());
        }
    }

    fn flush(&self) {}
}

pub struct Logger;

impl Logger {
    pub fn init(file_path: &str) -> Result<(), String> {
        let result = INIT_RESULT.get_or_init(|| {
            let level = if cfg!(debug_assertions) { LevelFilter::Trace } else { LevelFilter::Info };

            log::set_logger(&FILE_LOGGER)
                .map(|()| log::set_max_level(level))
                .map_err(|e| format!("Failed to initialize logger backend: {}", e))?;

            match OpenOptions::new()
                .write(true)
                .append(true)
                .create(true)
                .open(file_path)
            {
                Ok(file) => {
                    let mut guard = FILE_LOGGER.file.lock().unwrap();
                    *guard = Some(file);
                    Ok(())
                }
                Err(e) => {
                    eprintln!(
                        "warn: failed to open log file '{}': {}, falling back to stderr logger",
                        file_path, e
                    );
                    Ok(())
                }
            }
        });

        result.clone()
    }

    #[cfg(debug_assertions)]
    pub fn trace<S: AsRef<str>>(message: S) {
        log::trace!("{}", message.as_ref());
    }

    #[cfg(not(debug_assertions))] // Disable trace logging in release builds
    pub fn trace<S: AsRef<str>>(_message: S) {}

    #[cfg(debug_assertions)]
    pub fn debug<S: AsRef<str>>(message: S) {
        log::debug!("{}", message.as_ref());
    }

    #[cfg(not(debug_assertions))] // Disable debug logging in release builds
    pub fn debug<S: AsRef<str>>(_message: S) {}

    pub fn info<S: AsRef<str>>(message: S) {
        log::info!("{}", message.as_ref());
    }

    pub fn warn<S: AsRef<str>>(message: S) {
        log::warn!("{}", message.as_ref());
    }

    pub fn error<S: AsRef<str>>(message: S) {
        log::error!("{}", message.as_ref());
    }
}
