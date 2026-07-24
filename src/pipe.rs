use std::{fs::File, io::{Error, Write}};

use crate::logger::Logger;

fn write_pipe(pipe: &str, value: &str) -> Result<usize, Error> {
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
