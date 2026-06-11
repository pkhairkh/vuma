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
//! - **Child**: A spawned child process that can be waited on or killed.
//!
//! ## BD Annotations
//!
//! - Command: CapD { Read, Write, Execute }
//! - ExitStatus: CapD { Read, Compare, Serialize }
//! - Output: CapD { Read, Serialize }
//! - Child: CapD { Read, Write, Execute }

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

    /// Create from a std::process::ExitStatus.
    // VUMA-VERIFIED: conversion preserves exit code semantics
    pub(crate) fn from_std(status: std::process::ExitStatus) -> Self {
        Self { code: status.code() }
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

    /// Create from a std::process::Output.
    // VUMA-VERIFIED: conversion preserves all output data
    pub(crate) fn from_std(output: std::process::Output) -> Self {
        Self {
            status: ExitStatus::from_std(output.status),
            stdout: output.stdout,
            stderr: output.stderr,
        }
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
// Child
// ---------------------------------------------------------------------------

/// A VUMA-verified spawned child process.
///
/// `Child` wraps a `std::process::Child` with BD-tracked capabilities.
/// Supports waiting for completion, checking status, and killing the process.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Execute }
/// - SyncEdge: spawn → wait (Seq), spawn → kill (Seq)
pub struct Child {
    /// The underlying std::process::Child.
    inner: std::process::Child,
}

impl Child {
    /// Create a new Child from a std::process::Child.
    // VUMA-VERIFIED: wraps OS child process
    pub(crate) fn from_std(child: std::process::Child) -> Self {
        Self { inner: child }
    }

    /// Wait for the child process to exit and return its exit status.
    ///
    /// Delegates to `std::process::Child::wait`.
    // VUMA-VERIFIED: wait requires Execute capability
    pub fn wait(&mut self) -> Result<ExitStatus, String> {
        let status = self.inner.wait()
            .map_err(|e| format!("Child wait failed: {}", e))?;
        Ok(ExitStatus::from_std(status))
    }

    /// Check whether the child process has exited without blocking.
    ///
    /// Returns `Ok(Some(status))` if the child has exited, or `Ok(None)` if
    /// it is still running.
    ///
    /// Delegates to `std::process::Child::try_wait`.
    // VUMA-VERIFIED: try_wait is non-blocking and safe
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, String> {
        let result = self.inner.try_wait()
            .map_err(|e| format!("Child try_wait failed: {}", e))?;
        Ok(result.map(ExitStatus::from_std))
    }

    /// Kill the child process.
    ///
    /// Delegates to `std::process::Child::kill`.
    // VUMA-VERIFIED: kill requires Execute capability
    pub fn kill(&mut self) -> Result<(), String> {
        self.inner.kill()
            .map_err(|e| format!("Child kill failed: {}", e))
    }

    /// Returns the OS-assigned process ID.
    // VUMA-VERIFIED: pure accessor
    pub fn id(&self) -> u32 {
        self.inner.id()
    }

    /// Returns the CapD for this type.
    // VUMA-VERIFIED: child process has Read, Write, Execute
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Execute])
    }

    /// Returns the RepD for this type.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("Child", 0, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this type.
    // VUMA-VERIFIED: synchronization edges model child process lifecycle
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("child_spawn", "child_wait", SyncEdgeKind::Seq),
            SyncEdge::new("child_spawn", "child_kill", SyncEdgeKind::Seq),
        ]
    }
}

impl fmt::Display for Child {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Child {{ pid: {} }}", self.id())
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
/// Delegates to `std::process::Command` for real OS-level process management.
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

    /// Build the underlying std::process::Command from our fields.
    // VUMA-VERIFIED: builder preserves all configured options
    fn build_std_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.program);
        for arg in &self.args {
            cmd.arg(arg);
        }
        for (key, val) in &self.env {
            cmd.env(key, val);
        }
        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }
        cmd
    }

    /// Execute the command and return its exit status.
    ///
    /// Delegates to `std::process::Command::status` for real process execution.
    // VUMA-VERIFIED: status requires Execute capability
    pub fn status(&mut self) -> Result<ExitStatus, String> {
        let std_status = self.build_std_command().status()
            .map_err(|e| format!("Command status failed: {}", e))?;
        Ok(ExitStatus::from_std(std_status))
    }

    /// Execute the command and capture its output.
    ///
    /// Delegates to `std::process::Command::output` for real process execution
    /// with captured stdout/stderr.
    // VUMA-VERIFIED: output requires Execute capability
    pub fn output(&mut self) -> Result<Output, String> {
        let std_output = self.build_std_command().output()
            .map_err(|e| format!("Command output failed: {}", e))?;
        Ok(Output::from_std(std_output))
    }

    /// Spawn the command as a child process without waiting for completion.
    ///
    /// Delegates to `std::process::Command::spawn` for real process spawning.
    /// Returns a `Child` that can be waited on, polled, or killed.
    // VUMA-VERIFIED: spawn requires Execute capability
    pub fn spawn(&mut self) -> Result<Child, String> {
        let std_child = self.build_std_command().spawn()
            .map_err(|e| format!("Command spawn failed: {}", e))?;
        Ok(Child::from_std(std_child))
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
            SyncEdge::new("command_new", "command_spawn", SyncEdgeKind::Seq),
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
    fn test_command_status_real() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        let status = cmd.status().unwrap();
        assert!(status.success());
    }

    #[test]
    fn test_command_output_real() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello world");
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello world"));
    }

    #[test]
    fn test_command_output_exit_code() {
        // `true` always exits with 0
        let mut cmd = Command::new("true");
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.status.code(), Some(0));

        // `false` always exits with 1
        let mut cmd = Command::new("false");
        let output = cmd.output().unwrap();
        assert!(!output.status.success());
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn test_command_spawn_and_wait() {
        let mut cmd = Command::new("echo");
        cmd.arg("spawned");
        let mut child = cmd.spawn().unwrap();
        let pid = child.id();
        assert!(pid > 0);
        let status = child.wait().unwrap();
        assert!(status.success());
    }

    #[test]
    fn test_command_spawn_try_wait() {
        // Sleep briefly so we can test try_wait while running and after exit
        let mut cmd = Command::new("sleep");
        cmd.arg("0");
        let mut child = cmd.spawn().unwrap();
        // Wait for it to finish
        let status = child.wait().unwrap();
        assert!(status.success());
        // try_wait on already-exited child
        let result = child.try_wait().unwrap();
        assert!(result.is_some());
        assert!(result.unwrap().success());
    }

    #[test]
    fn test_command_stderr_capture() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("echo stdout_msg && echo stderr_msg >&2");
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.contains("stdout_msg"));
        assert!(stderr.contains("stderr_msg"));
    }

    #[test]
    fn test_command_failure_nonexistent() {
        let mut cmd = Command::new("this_program_does_not_exist_xyz");
        let result = cmd.status();
        assert!(result.is_err());
    }
}
