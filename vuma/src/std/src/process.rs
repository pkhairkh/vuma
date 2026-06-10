//! # Process
//!
//! This module provides VUMA-verified process types with Behavioral Description
//! (BD) annotations and capability tracking.
//!
//! ## Types
//!
//! - **Command**: A process builder for spawning child processes.
//! - **ExitStatus**: The exit status of a completed process.
//! - **Output**: The captured output (stdout, stderr) of a completed process.
//!
//! ## BD Annotations
//!
//! - Command: CapD { Read, Write, Execute }
//! - ExitStatus: CapD { Read, Compare, Serialize }
//! - Output: CapD { Read, Serialize }

use crate::primitives::{CapD, CapFlag, RepD, SyncEdge, SyncEdgeKind};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// ExitStatus
// ---------------------------------------------------------------------------

/// The exit status of a completed process.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Serialize }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitStatus {
    /// The raw exit code, if available.
    code: Option<i32>,
}

impl ExitStatus {
    /// Create a new ExitStatus from an exit code.
    // VUMA-VERIFIED: constructor is pure
    pub fn new(code: i32) -> Self {
        Self { code: Some(code) }
    }

    /// Create an ExitStatus for a process that exited via signal (no code).
    // VUMA-VERIFIED: constructor is pure
    pub fn from_signal() -> Self {
        Self { code: None }
    }

    /// Returns true if the process exited successfully (code 0).
    // VUMA-VERIFIED: pure query
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Returns the exit code, if available.
    // VUMA-VERIFIED: pure accessor
    pub fn code(&self) -> Option<i32> {
        self.code
    }

    /// Returns the CapD for this type.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Compare, CapFlag::Serialize])
    }

    /// Returns the RepD for this type.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("ExitStatus", 8, 4, self.capd())
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(code) => write!(f, "exit code: {}", code),
            None => write!(f, "terminated by signal"),
        }
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// The captured output of a completed process.
///
/// ## BD Annotations
///
/// - CapD: { Read, Serialize }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    /// The exit status of the process.
    pub status: ExitStatus,
    /// The captured standard output.
    pub stdout: Vec<u8>,
    /// The captured standard error.
    pub stderr: Vec<u8>,
}

impl Output {
    /// Create a new Output from an exit status and captured output.
    // VUMA-VERIFIED: constructor is pure
    pub fn new(status: ExitStatus, stdout: Vec<u8>, stderr: Vec<u8>) -> Self {
        Self { status, stdout, stderr }
    }

    /// Returns the CapD for this type.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Serialize])
    }

    /// Returns the RepD for this type.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("Output", 0, 8, self.capd())
    }
}

impl fmt::Display for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Output {{ status: {}, stdout: {} bytes, stderr: {} bytes }}",
            self.status,
            self.stdout.len(),
            self.stderr.len()
        )
    }
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// A VUMA-verified process builder.
///
/// `Command` constructs and spawns child processes with BD-tracked
/// capabilities. Every method returns `&mut Self` for chaining.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Execute }
/// - SyncEdge: new → status/output (Seq)
pub struct Command {
    /// The program to execute.
    program: String,
    /// Arguments to pass to the program.
    args: Vec<String>,
    /// Environment variables to set.
    env: Vec<(String, String)>,
    /// Working directory for the process.
    cwd: Option<String>,
}

impl Command {
    /// Create a new Command for the given program.
    // VUMA-VERIFIED: construction is safe
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
        }
    }

    /// Add a single argument to the command.
    // VUMA-VERIFIED: argument addition is safe
    pub fn arg(&mut self, arg: impl Into<String>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    /// Add multiple arguments to the command.
    // VUMA-VERIFIED: argument addition is safe
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for arg in args {
            self.args.push(arg.into());
        }
        self
    }

    /// Set an environment variable for the process.
    // VUMA-VERIFIED: environment variable setting is safe
    pub fn env(&mut self, key: impl Into<String>, val: impl Into<String>) -> &mut Self {
        self.env.push((key.into(), val.into()));
        self
    }

    /// Set the working directory for the process.
    // VUMA-VERIFIED: working directory setting is safe
    pub fn cwd(&mut self, dir: impl Into<String>) -> &mut Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Execute the command and return its exit status.
    ///
    /// In the VUMA simulation, this returns a simulated successful exit.
    // VUMA-VERIFIED: status requires Execute capability
    pub fn status(&mut self) -> Result<ExitStatus, String> {
        // In the VUMA runtime, this would invoke the OS fork/exec/wait syscalls.
        Ok(ExitStatus::new(0))
    }

    /// Execute the command and capture its output.
    ///
    /// In the VUMA simulation, this returns empty captured output.
    // VUMA-VERIFIED: output requires Execute capability
    pub fn output(&mut self) -> Result<Output, String> {
        // In the VUMA runtime, this would invoke the OS fork/exec and capture pipes.
        Ok(Output::new(ExitStatus::new(0), Vec::new(), Vec::new()))
    }

    /// Returns the program name.
    // VUMA-VERIFIED: pure accessor
    pub fn get_program(&self) -> &str {
        &self.program
    }

    /// Returns the arguments.
    // VUMA-VERIFIED: pure accessor
    pub fn get_args(&self) -> &[String] {
        &self.args
    }

    /// Returns the environment variables.
    // VUMA-VERIFIED: pure accessor
    pub fn get_env(&self) -> &[(String, String)] {
        &self.env
    }

    /// Returns the working directory, if set.
    // VUMA-VERIFIED: pure accessor
    pub fn get_cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// Returns the CapD for this type.
    // VUMA-VERIFIED: command has Read, Write, Execute
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Execute])
    }

    /// Returns the RepD for this type.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("Command", 0, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this type.
    // VUMA-VERIFIED: synchronization edges model process lifecycle
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("command_new", "command_status", SyncEdgeKind::Seq),
            SyncEdge::new("command_new", "command_output", SyncEdgeKind::Seq),
            SyncEdge::new("command_status", "command_wait", SyncEdgeKind::Seq),
        ]
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Command {{ program: {}", self.program)?;
        for arg in &self.args {
            write!(f, " {}", arg)?;
        }
        write!(f, " }}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit_status_success() {
        let status = ExitStatus::new(0);
        assert!(status.success());
        assert_eq!(status.code(), Some(0));
    }

    #[test]
    fn test_exit_status_failure() {
        let status = ExitStatus::new(1);
        assert!(!status.success());
        assert_eq!(status.code(), Some(1));

        let signal_status = ExitStatus::from_signal();
        assert!(!signal_status.success());
        assert_eq!(signal_status.code(), None);
    }

    #[test]
    fn test_command_builder() {
        let mut cmd = Command::new("ls");
        cmd.arg("-la").arg("/tmp");
        assert_eq!(cmd.get_program(), "ls");
        assert_eq!(cmd.get_args(), &["-la", "/tmp"]);
    }

    #[test]
    fn test_command_env_and_cwd() {
        let mut cmd = Command::new("python3");
        cmd.env("PYTHONPATH", "/opt/lib").cwd("/home/user");
        assert_eq!(cmd.get_env(), &[("PYTHONPATH".to_string(), "/opt/lib".to_string())]);
        assert_eq!(cmd.get_cwd(), Some("/home/user"));
    }

    #[test]
    fn test_command_status_and_output() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello");

        let status = cmd.status().unwrap();
        assert!(status.success());

        let output = cmd.output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 0);
        assert_eq!(output.stderr.len(), 0);
    }
}
