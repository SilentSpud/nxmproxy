use std::{ffi::c_void, io::{Error, ErrorKind}, mem, path::Path};

use crate::logger::Logger;

use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::ERROR_MORE_DATA,
        System::Registry::{
            HKEY, HKEY_CLASSES_ROOT, KEY_ALL_ACCESS, KEY_QUERY_VALUE, REG_CREATE_KEY_DISPOSITION,
            REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW,
            RegGetValueW, RegOpenKeyExW, RegSetValueExW,
        },
        UI::{
            Shell::{CommandLineToArgvW, SHELLEXECUTEINFOW, ShellExecuteExW},
            WindowsAndMessaging::SW_SHOW,
        },
    },
};

// copied from
// https://github.com/microsoft/windows-samples-rs/blob/master/webview2_win32/src/pwstr.rs
fn string_from_pwstr(source: PWSTR) -> String {
    if source.is_null() {
        String::new()
    } else {
        let mut buffer = Vec::new();
        let mut pwz = source.0;

        unsafe {
            while *pwz != 0 {
                buffer.push(*pwz);
                pwz = pwz.add(1);
            }
        }

        String::from_utf16(&buffer).expect("Failed to convert from windows api")
    }
}

fn to_wide(input: &str) -> Vec<u16> {
    let mut res: Vec<u16> = input.encode_utf16().collect();
    res.push(0);
    res
}

pub fn parse_commandline(command_line: &str) -> Result<(String, Vec<String>), Error> {
    Logger::trace(format!("parse_commandline input length={}", command_line.len()));
    Logger::debug(format!("parsing command line: {}", command_line));
    let exe: String;
    let mut args: Vec<String> = vec![];

    unsafe {
        let mut num_args: i32 = 0;
        let command_line_w = to_wide(command_line);
        let parsed = CommandLineToArgvW(PCWSTR(command_line_w.as_ptr()), &mut num_args);
        if parsed.is_null() {
            Logger::error("CommandLineToArgvW failed");
            return Err(Error::last_os_error());
        }

        exe = string_from_pwstr(*parsed);

        for i in 1..num_args {
            args.push(string_from_pwstr(*parsed.offset(i as isize)));
        }
    }

    Logger::trace(format!("CommandLineToArgvW parsed {} arg(s)", args.len() + 1));
    Logger::debug(format!("parsed executable and {} argument(s)", args.len()));

    return Ok((exe, args))
}

pub fn get_protocol_handler(protocol: &str) -> Result<String, Error> {
    Logger::debug(format!("querying protocol handler for '{}'", protocol));
    let result: String;
    unsafe {
        let mut hkey: HKEY = HKEY::default();
        let sub_key = to_wide(format!(r"{0}\shell\open\command", protocol).as_str());
        let mut res = RegOpenKeyExW(
            HKEY_CLASSES_ROOT,
            PCWSTR(sub_key.as_ptr()),
            Some(0),
            KEY_QUERY_VALUE,
            &mut hkey,
        );
        if res.0 != 0 {
            Logger::error(format!("RegOpenKeyExW failed: {}", res.0));
            return Err(Error::from_raw_os_error(res.0 as i32));
        }

        let mut buffer: Vec<u16> = vec![0; 256];
        let mut data_size: u32 = (buffer.capacity() * 2) as u32;
        // preflight to find out the required buffer size
        res = RegGetValueW(
            hkey,
            PCWSTR::null(),
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut c_void),
            Some(&mut data_size),
        );

        buffer.resize((data_size / 2 - 1) as usize, 0);

        // more data, buffer was too small
        if res == ERROR_MORE_DATA {
            Logger::trace("registry value exceeded initial buffer, retrying with resized buffer");
            res = RegGetValueW(
                hkey,
                PCWSTR::null(),
                PCWSTR::null(),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr() as *mut c_void),
                Some(&mut data_size),
            );
            // cut off 0 terminator
            buffer.resize((data_size / 2 - 1) as usize, 0);
        }

        if res.0 != 0 {
            Logger::error(format!("RegGetValueW failed: {}", res.0));
            return Err(Error::from_raw_os_error(res.0 as i32));
        }

        result = String::from_utf16(&buffer).expect("failed to convert registry value");

        res = RegCloseKey(hkey);

        if res.0 != 0 {
            Logger::warn(format!("RegCloseKey failed after read: {}", res.0));
            return Err(Error::from_raw_os_error(res.0 as i32));
        }
    }

    Logger::trace(format!("protocol handler command size={} chars", result.len()));
    Logger::info(format!("resolved protocol handler for '{}'", protocol));

    Ok(result)
}

pub fn set_protocol_handler(protocol: &str, command: &str) -> Result<(), Error> {
    Logger::trace(format!("set_protocol_handler command length={}", command.len()));
    Logger::info(format!("setting protocol handler for '{}'", protocol));
    unsafe {
        let mut hkey: HKEY = HKEY::default();
        let sub_key = to_wide(format!(r"{0}\shell\open\command", protocol).as_str());
        let mut dispo: REG_CREATE_KEY_DISPOSITION = REG_CREATE_KEY_DISPOSITION(0);
        let mut res = RegCreateKeyExW(
            HKEY_CLASSES_ROOT,
            PCWSTR(sub_key.as_ptr()),
            Some(0),
            PWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut hkey,
            Some(&mut dispo),
        );

        if res.0 != 0 {
            Logger::error(format!("RegCreateKeyExW failed: {}", res.0));
            return Err(Error::from_raw_os_error(res.0 as i32));
        }

        let command_u16 = to_wide(command);
        let command_u8 = std::slice::from_raw_parts(
            command_u16.as_ptr() as *const u8,
            command_u16.len() * 2,
        );

        res = RegSetValueExW(hkey, PCWSTR::null(), Some(0), REG_SZ, Some(command_u8));

        if res.0 != 0 {
            Logger::error(format!("RegSetValueExW failed: {}", res.0));
            return Err(Error::from_raw_os_error(res.0 as i32));
        }

        res = RegCloseKey(hkey);

        if res.0 != 0 {
            Logger::warn(format!("RegCloseKey failed after write: {}", res.0));
            return Err(Error::from_raw_os_error(res.0 as i32));
        }
    }

    Logger::info(format!("protocol handler for '{}' updated", protocol));

    Ok(())
}

pub fn spawn_elevated(exe: &str, args: Vec<&str>) -> Result<(), Error> {
    Logger::warn(format!(
        "spawning elevated process '{}' with {} argument(s)",
        exe,
        args.len()
    ));
    let cwd = Path::new(&exe).parent().unwrap().to_str().unwrap();

    let verb = to_wide("runas");
    let file = to_wide(exe);
    let directory = to_wide(cwd);
    let parameters = to_wide(args.join(" ").as_str());

    let mut exec_info = SHELLEXECUTEINFOW::default();
    exec_info.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    exec_info.lpVerb = PCWSTR(verb.as_ptr());
    exec_info.lpFile = PCWSTR(file.as_ptr());
    exec_info.lpDirectory = PCWSTR(directory.as_ptr());
    exec_info.lpParameters = PCWSTR(parameters.as_ptr());
    exec_info.nShow = SW_SHOW.0;

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
