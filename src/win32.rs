use std::{io::{Error, ErrorKind}, mem, path::Path};

use winreg::{enums::HKEY_CLASSES_ROOT, RegKey};

use crate::logger::Logger;

use windows::{
    core::PCWSTR,
    Win32::UI::{
        Shell::{SHELLEXECUTEINFOW, ShellExecuteExW},
        WindowsAndMessaging::SW_SHOW,
    },
};

fn to_wide(input: &str) -> Vec<u16> {
    let mut res: Vec<u16> = input.encode_utf16().collect();
    res.push(0);
    res
}

/// Tokenize a Windows-style command line string.
///
/// Implements the documented `CommandLineToArgvW` algorithm — in safe Rust so no
/// `unsafe` block is needed. Rules from the Win32 documentation:
/// * Tokens are separated by unquoted whitespace (space or tab).
/// * Double quotes toggle quoted mode; whitespace inside quotes is literal.
/// * `N` backslashes before a `"`: `N/2` literal `\` + quote toggles mode (even N),
///   or `(N-1)/2` literal `\` + literal `"` (odd N).
/// * Backslashes not immediately before a `"` are always literal.
fn tokenize_commandline(input: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if !in_quotes && matches!(chars[i], ' ' | '\t') {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            i += 1;
            continue;
        }

        if chars[i] == '\\' {
            let start = i;
            while i < chars.len() && chars[i] == '\\' { i += 1; }
            let n = i - start;
            if i < chars.len() && chars[i] == '"' {
                for _ in 0..n / 2 { current.push('\\'); }
                if n % 2 == 0 { in_quotes = !in_quotes; } else { current.push('"'); }
                i += 1;
            } else {
                for _ in 0..n { current.push('\\'); }
            }
            continue;
        }

        if chars[i] == '"' { in_quotes = !in_quotes; i += 1; continue; }

        current.push(chars[i]);
        i += 1;
    }

    if !current.is_empty() { tokens.push(current); }
    tokens
}

pub fn parse_commandline(command_line: &str) -> Result<(String, Vec<String>), Error> {
    Logger::trace(format!("parse_commandline input length={}", command_line.len()));
    Logger::debug("parsing command line");

    if command_line.len() > 32_767 {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "command line exceeds Windows length limit",
        ));
    }

    let mut tokens = tokenize_commandline(command_line);
    if tokens.is_empty() {
        return Err(Error::new(ErrorKind::InvalidInput, "empty command line"));
    }

    let exe = tokens.remove(0);
    Logger::debug(format!("parsed executable and {} argument(s)", tokens.len()));
    Ok((exe, tokens))
}

pub fn get_protocol_handler(protocol: &str) -> Result<String, Error> {
    Logger::debug(format!("querying protocol handler for '{}'", protocol));
    let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
    let sub_key = format!(r"{}\shell\open\command", protocol);
    let key = hkcr.open_subkey(&sub_key)?;
    let value: String = key.get_value("")?;
    Logger::info(format!("resolved protocol handler for '{}'", protocol));
    Ok(value)
}

pub fn set_protocol_handler(protocol: &str, command: &str) -> Result<(), Error> {
    Logger::info(format!("setting protocol handler for '{}'", protocol));
    let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
    let sub_key = format!(r"{}\shell\open\command", protocol);
    let (key, _) = hkcr.create_subkey(&sub_key)?;
    key.set_value("", &command.to_string())?;
    Logger::info(format!("protocol handler for '{}' updated", protocol));
    Ok(())
}

pub fn spawn_elevated(exe: &str, args: Vec<&str>) -> Result<(), Error> {
    Logger::warn(format!(
        "spawning elevated process '{}' with {} argument(s)",
        exe,
        args.len()
    ));

    let exe_path = Path::new(exe);
    let cwd = exe_path
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or(".");

    let verb = to_wide("runas");
    let file = to_wide(exe_path.to_str().unwrap_or(exe));
    let directory = to_wide(cwd);
    let parameters = to_wide(args.join(" ").as_str());

    let mut exec_info = SHELLEXECUTEINFOW::default();
    exec_info.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    exec_info.lpVerb = PCWSTR(verb.as_ptr());
    exec_info.lpFile = PCWSTR(file.as_ptr());
    exec_info.lpDirectory = PCWSTR(directory.as_ptr());
    exec_info.lpParameters = PCWSTR(parameters.as_ptr());
    exec_info.nShow = SW_SHOW.0;

    // No safe way to do this :/
    unsafe {
        ShellExecuteExW(&mut exec_info)
            .map(|_| {
                Logger::info("elevated process launched");
            })
            .map_err(|e| {
                Logger::error(format!("failed to launch elevated process: {}", e));
                Error::new(ErrorKind::Other, e.to_string())
            })
    }
}
