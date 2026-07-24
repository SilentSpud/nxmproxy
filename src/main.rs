use config_file::Config;
use win32::{get_protocol_handler, parse_commandline, set_protocol_handler};
use std::ffi::OsStr;
use std::fs::DirBuilder;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{
    env,
    io::Error,
    process::Stdio,
};

use crate::pipe::try_write_pipe;
use crate::logger::Logger;
use crate::win32::spawn_elevated;

mod win32;
mod config_file;
mod pipe;
mod logger;

/// NXM URLs longer than this are rejected to prevent DoS and injection probing.
const MAX_URL_LEN: usize = 8192;
/// Placeholder substituted into the manager command template before Win32 parsing,
/// so that the URL is never part of the token structure seen by CommandLineToArgvW.
const URL_PLACEHOLDER: &str = "__NXMPROXY_URL_PLACEHOLDER__";

enum Options {
    Url {
        url: String,
    },
    Register {
        manager: String,
        command: String,
    },
    Deregister {
        manager: String,
    },
    Assign {
        manager: String,
        game: String,
    },
    Pipe {
        manager: String,
        pipe: String,
    },
    Install,
    Test,
}

/// create a directory (structure) if necessary
fn ensure_dir(path: &Path) -> Result<(), Error> {
    DirBuilder::new().recursive(true).create(path)
}

fn usage() -> &'static str {
    "Usage:\n  nxmproxy url <nxm-url>\n  nxmproxy register <manager> <command>\n  nxmproxy deregister <manager>\n  nxmproxy assign <manager> <game>\n  nxmproxy pipe <manager> <pipe>\n  nxmproxy install\n  nxmproxy test"
}

fn parse_options() -> Result<Options, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    Logger::debug(format!("received {} command-line argument(s)", args.len()));
    if args.is_empty() {
        Logger::warn("no command-line arguments provided");
        return Err(usage().to_string());
    }

    let command = args[0].to_ascii_lowercase();
    match command.as_str() {
        "url" if args.len() == 2 => Ok(Options::Url {
            url: args[1].clone(),
        }),
        "register" if args.len() == 3 => Ok(Options::Register {
            manager: args[1].clone(),
            command: args[2].clone(),
        }),
        "deregister" if args.len() == 2 => Ok(Options::Deregister {
            manager: args[1].clone(),
        }),
        "assign" if args.len() == 3 => Ok(Options::Assign {
            manager: args[1].clone(),
            game: args[2].clone(),
        }),
        "pipe" if args.len() == 3 => Ok(Options::Pipe {
            manager: args[1].clone(),
            pipe: args[2].clone(),
        }),
        "install" if args.len() == 1 => Ok(Options::Install),
        "test" if args.len() == 1 => Ok(Options::Test),
        _ => {
            Logger::warn(format!("invalid command-line argument pattern, count={}", args.len()));
            Err(usage().to_string())
        }
    }
}

fn parse_nxm_game(url: &str) -> Result<&str, String> {
    if url.len() > MAX_URL_LEN {
        Logger::warn(format!("rejected overlong url (len={})", url.len()));
        return Err("URL too long".to_string());
    }

    let rest = url
        .strip_prefix("nxm://")
        .ok_or_else(|| {
            Logger::warn(format!("rejected non-nxm url: {}", url));
            "Not an nxm url".to_string()
        })?;
    let game = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| {
            Logger::warn(format!("failed to parse game from url: {}", url));
            "Invalid url".to_string()
        })?;
    Logger::debug(format!("parsed nxm game '{}' from url", game));
    Ok(game)
}

fn sanitize_url_for_log(url: &str) -> &str {
    // Log only up to the first query/fragment delimiter to avoid leaking tokens.
    let end = url.find(['?', '#']).unwrap_or(url.len());
    &url[..end]
}

/// Parse the manager command template, then substitute the URL into the already-parsed
/// tokens so that URL content never influences command-line tokenization.
fn build_manager_invocation(template: &str, url: &str) -> Result<(String, Vec<String>), String> {
    let templated = if template.contains("%1") {
        template.replace("%1", URL_PLACEHOLDER)
    } else {
        Logger::warn("manager command template has no %1 placeholder; url appended as argument");
        format!("{} {}", template, URL_PLACEHOLDER)
    };

    let (exe, args) = parse_commandline(&templated)
        .map_err(|e| format!("Failed to parse command line: {}", e))?;

    let exe = exe.replace(URL_PLACEHOLDER, url);
    let args = args
        .into_iter()
        .map(|a| a.replace(URL_PLACEHOLDER, url))
        .collect();

    Ok((exe, args))
}

/// handle download url with the appropriate manager
fn download(config: &Config, url: &str) -> Result<(), String> {
    Logger::debug(format!("starting download workflow for url: {}", url));
    let game = parse_nxm_game(url)?;

    let manager = config
        .resolve(game)
        .map_err(|e| {
            Logger::error(format!("failed to resolve manager for game '{}': {}", game, e));
            e
        })?;

    Logger::info(format!(
        "downloading url: {}, game: {}, manager: {}",
        sanitize_url_for_log(url), game, manager
    ));

    if config.pipes.contains_key(&manager) {
        Logger::info(format!("trying pipe: {}", config.pipes[&manager]));
        if try_write_pipe(&config.pipes[&manager], url) {
            Logger::info(format!("dispatched url to manager '{}' via pipe", manager));
            return Ok(());
        } else {
            Logger::warn("pipe write failed, falling back to process spawn");
        }
    }

    let command_template = &config.managers[&manager];
    Logger::debug("preparing manager invocation");
    let (exe, args) = build_manager_invocation(command_template, url).map_err(|e| {
        Logger::error(format!("failed to build manager invocation: {}", e));
        e
    })?;

    Logger::trace(format!("spawning manager exe '{}' with {} arg(s)", exe, args.len()));
    Command::new(&exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            Logger::error(format!("failed to spawn manager process: {}", e));
            format!("Failed to spawn manager: {}", e)
        })?;

    Logger::info(format!("spawned manager '{}' for game '{}'", manager, game));

    Ok(())
}

/// set up nxmproxy as the handler for nxm links
fn install() -> Result<(), Error> {
    let command = std::env::current_exe()
        .map(|path| path.to_string_lossy().to_string())?;

    Logger::info(format!("registering as nxm handler: {}", command));
    let res =
        set_protocol_handler("nxm", format!("\"{}\" url \"%1\"", command).as_str());

    if let Err(err) = res {
        if err.raw_os_error() == Some(5) {
            Logger::warn("registering requires elevation");
            let self_exe = std::env::current_exe()
                .expect("failed to determine current exe");

            return spawn_elevated(self_exe.to_str().unwrap(), vec!["install"]);
        } else {
            Logger::error(format!("failed to register protocol handler: {}", err));
            Err(err)
        }
    } else {
        Logger::info("protocol handler registration successful");
        Ok(())
    }
}

/// check whether nxmproxy (any instance) is assigned as the nxm handler
fn test_installed() -> Result<bool, Error> {
    let handler = get_protocol_handler("nxm");
    match handler {
        Ok(commandline) => {
            Logger::debug(format!("current nxm protocol handler command: {}", commandline));
            let (exe_path, _args) = parse_commandline(commandline.as_str())
                .map_err(|e| {
                    Logger::error(format!("failed to parse handler command line: {}", e));
                    e
                })?;
            let exe_name = Path::new(&exe_path)
                .file_name()
                .unwrap_or_default();
            Ok(exe_name == OsStr::new("nxmproxy.exe"))
        },
        Err(e) => {
            Logger::error(format!("failed to query nxm protocol handler: {}", e));
            Err(e)
        }
    }
}

fn applocal_path() -> PathBuf {
    if let Some(path) = env::var_os("LOCALAPPDATA") {
        Logger::debug("using LOCALAPPDATA for configuration root");
        return PathBuf::from(path).join("nxmproxy");
    }
    if let Some(path) = env::var_os("APPDATA") {
        Logger::warn("LOCALAPPDATA not set, falling back to APPDATA");
        return PathBuf::from(path).join("nxmproxy");
    }
    Logger::warn("LOCALAPPDATA and APPDATA are unset, falling back to local directory");
    PathBuf::from(".").join("nxmproxy")
}

fn main_impl() -> Result<i32, String> {
    let opt = parse_options()?;

    let config_path_buf = applocal_path();
    let config_path = config_path_buf.as_path();
    Logger::info(format!("using configuration directory: {}", config_path.display()));

    ensure_dir(config_path).map_err(|e| {
        let msg = format!("Failed to create project directory: {}", e);
        Logger::error(&msg);
        msg
    })?;

    let mut config = Config::read(config_path).map_err(|e| {
        let msg = format!("Failed to read config: {}", e);
        Logger::error(&msg);
        msg
    })?;
    Logger::debug("configuration loaded successfully");

    return match opt {
        Options::Url { url } => download(&config, &url).and(Ok(0)),
        Options::Assign { manager, game } => {
            Logger::info(format!("assign manager '{}' to game '{}'", manager, game));
            config.assign(&manager, &game)?;
            return config.write_config(config_path).and(Ok(0));
        }
        Options::Register { manager, command } => {
            Logger::info(format!("register manager '{}'", manager));
            config.register(&manager, &command)?;
            return config.write_config(config_path).and(Ok(0));
        }
        Options::Pipe { manager, pipe } => {
            Logger::info(format!("register pipe '{}' for manager '{}'", pipe, manager));
            config.register_pipe(&manager, &pipe)?;
            return config.write_config(config_path).and(Ok(0));
        }
        Options::Deregister { manager } => {
            Logger::warn(format!("deregister manager '{}'", manager));
            config.deregister(&manager)?;
            return config.write_config(config_path).and(Ok(0));
        }
        Options::Install {} => {
            Logger::info("install command requested");
            match install() {
                Ok(()) => Ok(0),
                Err(e) => Err(e.to_string()),
            }
        }
        Options::Test {} => {
            Logger::info("test command requested");
            match test_installed() {
                Ok(is_installed) => {
                    println!("is installed: {}", is_installed);
                    Ok(if is_installed { 0 } else { 1 })
                },
                Err(e) => Err(e.to_string()),
            }
        }
    }
}

fn main() -> Result<(), String> {
    let config_path_buf = applocal_path();
    let config_path = config_path_buf.as_path();
    if let Err(e) = Logger::init(config_path.join("nxm.log").as_path().to_str().unwrap()) {
        eprintln!("error: failed to initialize logger: {}", e);
    }

    Logger::debug("logger initialized");
    Logger::info("nxmproxy starting");

    match main_impl() {
        Ok(res) => {
            Logger::info(format!("nxmproxy exiting with code {}", res));
            std::process::exit(res);
        }
        Err(e) => {
            Logger::error(format!("Failed to process commandline: {}", e));
            std::process::exit(1);
        }
    }
}

