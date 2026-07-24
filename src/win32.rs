use std::{ffi::c_void, io::{Error, ErrorKind}, mem, path::Path};

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
    let exe: String;
    let mut args: Vec<String> = vec![];

    unsafe {
        let mut num_args: i32 = 0;
        let command_line_w = to_wide(command_line);
        let parsed = CommandLineToArgvW(PCWSTR(command_line_w.as_ptr()), &mut num_args);
        if parsed.is_null() {
            return Err(Error::last_os_error());
        }

        exe = string_from_pwstr(*parsed);

        for i in 1..num_args {
            args.push(string_from_pwstr(*parsed.offset(i as isize)));
        }
    }

    return Ok((exe, args))
}

pub fn get_protocol_handler(protocol: &str) -> Result<String, Error> {
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
            return Err(Error::from_raw_os_error(res.0 as i32));
        }

        result = String::from_utf16(&buffer).expect("failed to convert registry value");

        res = RegCloseKey(hkey);

        if res.0 != 0 {
            return Err(Error::from_raw_os_error(res.0 as i32));
        }
    }

    Ok(result)
}

pub fn set_protocol_handler(protocol: &str, command: &str) -> Result<(), Error> {
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
            return Err(Error::from_raw_os_error(res.0 as i32));
        }

        let command_u16 = to_wide(command);
        let command_u8 = std::slice::from_raw_parts(
            command_u16.as_ptr() as *const u8,
            command_u16.len() * 2,
        );

        res = RegSetValueExW(hkey, PCWSTR::null(), Some(0), REG_SZ, Some(command_u8));

        if res.0 != 0 {
            return Err(Error::from_raw_os_error(res.0 as i32));
        }

        res = RegCloseKey(hkey);

        if res.0 != 0 {
            return Err(Error::from_raw_os_error(res.0 as i32));
        }
    }

    Ok(())
}

pub fn spawn_elevated(exe: &str, args: Vec<&str>) -> Result<(), Error> {
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
        ShellExecuteExW(&mut exec_info).map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
    }
}
