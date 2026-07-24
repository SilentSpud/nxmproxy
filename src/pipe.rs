use std::{fs::File, io::{Error, ErrorKind, Write}};

use crate::logger::Logger;

/// Only allow pipe names that cannot escape the `\\.\pipe\` prefix.
pub fn is_valid_pipe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn write_pipe(pipe: &str, value: &str) -> Result<usize, Error> {
    if !is_valid_pipe_name(pipe) {
        return Err(Error::new(ErrorKind::InvalidInput, "invalid pipe name"));
    }
    Logger::debug(format!("opening named pipe '{}'", pipe));
    return File::create(format!(r"\\.\pipe\{}", pipe))?.write(value.as_bytes());
}

pub fn try_write_pipe(pipe: &str, value: &str) -> bool {
    match write_pipe(pipe, value) {
        Ok(bytes) => {
            Logger::info(format!("wrote {} bytes to named pipe '{}'", bytes, pipe));
            true
        }
        Err(e) => {
            Logger::warn(format!("failed to write pipe '{}': {}", pipe, e));
            false
        }
    }
}
