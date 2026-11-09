//! Native file and command tools for Boid, using the server account's permissions.
//! The working directory supplies context, not filesystem confinement.

#[cfg(not(unix))]
compile_error!("Boid currently requires a POSIX execution environment.");

mod files;
pub mod guard;
mod process;

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

pub const OUTPUT_LIMIT: usize = 32 * 1024;

#[derive(Debug)]
pub struct Output {
    pub text: String,
    pub truncated: bool,
}

impl Output {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            text: String::from_utf8_lossy(&bytes[..bytes.len().min(OUTPUT_LIMIT)]).into_owned(),
            truncated: bytes.len() > OUTPUT_LIMIT,
        }
    }
}

#[derive(Debug)]
pub struct ExecOutput {
    pub status: ExitStatus,
    pub stdout: Output,
    pub stderr: Output,
    pub timed_out: bool,
}

pub struct Tools {
    pub(crate) cwd: PathBuf,
}

impl Tools {
    pub fn new(cwd: impl AsRef<Path>) -> io::Result<Self> {
        let cwd = cwd.as_ref().canonicalize()?;
        if !cwd.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Expected a working directory.",
            ));
        }
        Ok(Self { cwd })
    }

    pub(crate) fn path(&self, path: impl AsRef<Path>) -> PathBuf {
        self.cwd.join(path)
    }
}
