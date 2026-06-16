//! Structured logging for VUMA.
//!
//! This module provides a structured logging system with configurable
//! verbosity levels. Log output follows the format:
//!
//! ```text
//! [TIMESTAMP] [LEVEL] [STAGE] message
//! ```
//!
//! # Levels
//!
//! | Level  | Description                                      |
//! |--------|--------------------------------------------------|
//! | error  | Critical failures that prevent compilation        |
//! | warn   | Non-fatal issues that may affect output quality   |
//! | info   | General compilation progress messages             |
//! | debug  | Detailed stage-by-stage information (--verbose)   |
//! | trace  | Very detailed internal diagnostics                 |
//!
//! # CLI Flags
//!
//! - `--verbose` / `-v` — Enable debug-level logging
//! - `--quiet` / `-q` — Suppress all output except errors
//!
//! # Example
//!
//! ```rust,ignore
//! use vuma::logging::{VumaLogger, LogLevel};
//!
//! let logger = VumaLogger::new(LogLevel::Info);
//! logger.info("parse", "Starting parse stage");
//! logger.debug("parse", "Tokenizing input");  // won't print at Info level
//! logger.error("codegen", "Register allocation failed");
//! ```

use std::fmt;
use std::io::Write;
use std::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════════
// LogLevel
// ═══════════════════════════════════════════════════════════════════════════

/// Logging verbosity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    /// Only errors — suppresses all other output.
    Error = 0,
    /// Errors + warnings.
    Warn = 1,
    /// Errors + warnings + informational messages (default).
    Info = 2,
    /// Errors + warnings + info + debug (--verbose / -v).
    Debug = 3,
    /// All messages, including trace-level diagnostics.
    Trace = 4,
}

impl LogLevel {
    /// Parse a log level from a string.
    pub fn from_str_level(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "error" => Some(LogLevel::Error),
            "warn" | "warning" => Some(LogLevel::Warn),
            "info" => Some(LogLevel::Info),
            "debug" => Some(LogLevel::Debug),
            "trace" => Some(LogLevel::Trace),
            _ => None,
        }
    }

    /// Returns the short tag used in log output.
    pub fn tag(&self) -> &'static str {
        match self {
            LogLevel::Error => "ERROR",
            LogLevel::Warn => "WARN ",
            LogLevel::Info => "INFO ",
            LogLevel::Debug => "DEBUG",
            LogLevel::Trace => "TRACE",
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogLevel::Error => write!(f, "error"),
            LogLevel::Warn => write!(f, "warn"),
            LogLevel::Info => write!(f, "info"),
            LogLevel::Debug => write!(f, "debug"),
            LogLevel::Trace => write!(f, "trace"),
        }
    }
}

impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Info
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// VumaLogger
// ═══════════════════════════════════════════════════════════════════════════

/// Thread-safe structured logger for VUMA.
///
/// All log output follows the format:
/// ```text
/// [TIMESTAMP] [LEVEL] [STAGE] message
/// ```
///
/// The logger is thread-safe via an internal `Mutex`.
pub struct VumaLogger {
    level: Mutex<LogLevel>,
}

impl VumaLogger {
    /// Create a new logger with the given minimum log level.
    pub fn new(level: LogLevel) -> Self {
        Self {
            level: Mutex::new(level),
        }
    }

    /// Create a logger for quiet mode (errors only).
    pub fn quiet() -> Self {
        Self::new(LogLevel::Error)
    }

    /// Create a logger for verbose mode (debug and above).
    pub fn verbose() -> Self {
        Self::new(LogLevel::Debug)
    }

    /// Create a logger with default (info) level.
    pub fn default_logger() -> Self {
        Self::new(LogLevel::Info)
    }

    /// Get the current log level.
    pub fn level(&self) -> LogLevel {
        *self.level.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Set the log level.
    pub fn set_level(&self, level: LogLevel) {
        let mut guard = self.level.lock().unwrap_or_else(|e| e.into_inner());
        *guard = level;
    }

    /// Log a message at the given level and stage.
    ///
    /// The message is only printed if `level` is at or above the
    /// configured minimum level.
    pub fn log(&self, level: LogLevel, stage: &str, message: &str) {
        let min_level = self.level();
        if level <= min_level {
            let timestamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f");
            let output = format!("[{}] [{}] [{}] {}", timestamp, level.tag(), stage, message);

            if level == LogLevel::Error {
                let _ = writeln!(std::io::stderr(), "{}", output);
            } else if level <= LogLevel::Warn {
                let _ = writeln!(std::io::stderr(), "{}", output);
            } else {
                let _ = writeln!(std::io::stderr(), "{}", output);
            }
        }
    }

    /// Log an error message.
    pub fn error(&self, stage: &str, message: &str) {
        self.log(LogLevel::Error, stage, message);
    }

    /// Log a warning message.
    pub fn warn(&self, stage: &str, message: &str) {
        self.log(LogLevel::Warn, stage, message);
    }

    /// Log an informational message.
    pub fn info(&self, stage: &str, message: &str) {
        self.log(LogLevel::Info, stage, message);
    }

    /// Log a debug message (only shown with --verbose).
    pub fn debug(&self, stage: &str, message: &str) {
        self.log(LogLevel::Debug, stage, message);
    }

    /// Log a trace message (only shown at the highest verbosity).
    pub fn trace(&self, stage: &str, message: &str) {
        self.log(LogLevel::Trace, stage, message);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Global logger
// ═══════════════════════════════════════════════════════════════════════════

use std::sync::OnceLock;

static GLOBAL_LOGGER: OnceLock<VumaLogger> = OnceLock::new();

/// Initialize the global logger with the given level.
///
/// Should be called once at program startup.
pub fn init_logger(level: LogLevel) {
    let _ = GLOBAL_LOGGER.get_or_init(|| VumaLogger::new(level));
}

/// Get a reference to the global logger.
///
/// If `init_logger` has not been called, defaults to `Info` level.
pub fn global_logger() -> &'static VumaLogger {
    GLOBAL_LOGGER.get_or_init(VumaLogger::default_logger)
}

/// Convenience macro for logging to the global logger.
#[macro_export]
macro_rules! log_error {
    ($stage:expr, $($arg:tt)*) => {
        $crate::logging::global_logger().error($stage, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($stage:expr, $($arg:tt)*) => {
        $crate::logging::global_logger().warn($stage, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_info {
    ($stage:expr, $($arg:tt)*) => {
        $crate::logging::global_logger().info($stage, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_debug {
    ($stage:expr, $($arg:tt)*) => {
        $crate::logging::global_logger().debug($stage, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_trace {
    ($stage:expr, $($arg:tt)*) => {
        $crate::logging::global_logger().trace($stage, &format!($($arg)*))
    };
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration with the `log` crate
// ═══════════════════════════════════════════════════════════════════════════

/// A `log::Log` implementation that forwards to the VUMA structured logger.
pub struct VumaLogBridge;

impl log::Log for VumaLogBridge {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        let level = match metadata.level() {
            log::Level::Error => LogLevel::Error,
            log::Level::Warn => LogLevel::Warn,
            log::Level::Info => LogLevel::Info,
            log::Level::Debug => LogLevel::Debug,
            log::Level::Trace => LogLevel::Trace,
        };
        level <= global_logger().level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let level = match record.level() {
                log::Level::Error => LogLevel::Error,
                log::Level::Warn => LogLevel::Warn,
                log::Level::Info => LogLevel::Info,
                log::Level::Debug => LogLevel::Debug,
                log::Level::Trace => LogLevel::Trace,
            };
            let stage = record
                .target()
                .strip_prefix("vuma::")
                .unwrap_or(record.target());
            global_logger().log(level, stage, &format!("{}", record.args()));
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Error < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Trace);
    }

    #[test]
    fn test_log_level_from_str() {
        assert_eq!(LogLevel::from_str_level("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::from_str_level("warn"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_str_level("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::from_str_level("debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::from_str_level("trace"), Some(LogLevel::Trace));
        assert_eq!(LogLevel::from_str_level("unknown"), None);
    }

    #[test]
    fn test_log_level_tags() {
        assert_eq!(LogLevel::Error.tag(), "ERROR");
        assert_eq!(LogLevel::Warn.tag(), "WARN ");
        assert_eq!(LogLevel::Info.tag(), "INFO ");
        assert_eq!(LogLevel::Debug.tag(), "DEBUG");
        assert_eq!(LogLevel::Trace.tag(), "TRACE");
    }

    #[test]
    fn test_logger_creation() {
        let logger = VumaLogger::new(LogLevel::Info);
        assert_eq!(logger.level(), LogLevel::Info);

        let quiet = VumaLogger::quiet();
        assert_eq!(quiet.level(), LogLevel::Error);

        let verbose = VumaLogger::verbose();
        assert_eq!(verbose.level(), LogLevel::Debug);
    }

    #[test]
    fn test_logger_set_level() {
        let logger = VumaLogger::new(LogLevel::Info);
        logger.set_level(LogLevel::Debug);
        assert_eq!(logger.level(), LogLevel::Debug);
    }

    #[test]
    fn test_log_format() {
        // This test verifies that log messages are formatted correctly.
        // The actual output goes to stderr, so we just ensure no panics.
        let logger = VumaLogger::new(LogLevel::Trace);
        logger.error("test-stage", "test error message");
        logger.warn("test-stage", "test warning message");
        logger.info("test-stage", "test info message");
        logger.debug("test-stage", "test debug message");
        logger.trace("test-stage", "test trace message");
    }

    #[test]
    fn test_quiet_suppresses_info() {
        // At Error level, info/debug/trace messages should not be printed
        let logger = VumaLogger::quiet();
        // These should all be suppressed (no panic, no output)
        logger.info("test", "should not print");
        logger.debug("test", "should not print");
        logger.trace("test", "should not print");
        // This should print
        logger.error("test", "should print");
    }

    #[test]
    fn test_global_logger() {
        // Just verify we can access the global logger without panic
        let logger = global_logger();
        logger.info("test", "global logger works");
    }
}
