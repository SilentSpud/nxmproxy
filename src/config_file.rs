use std::{
    collections::{BTreeMap, HashMap},
    fs::{File, read_to_string, rename},
    io::{Error, ErrorKind, Write},
    path::Path,
};

use crate::logger::Logger;
use crate::pipe::is_valid_pipe_name;

const MAX_CONFIG_BYTES: u64 = 1 << 20; // 1 MiB

#[derive(Debug)]
pub struct Config {
    pub games: HashMap<String, String>,
    pub managers: HashMap<String, String>,
    pub pipes: HashMap<String, String>,
}

static EMPTY_CONFIG: &str = r#"
[games]

[managers]

[pipes]
"#;

fn unescape_quoted(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut escape = false;
    for ch in input.chars() {
        if escape {
            let mapped = match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                other => other,
            };
            out.push(mapped);
            escape = false;
        } else if ch == '\\' {
            escape = true;
        } else {
            out.push(ch);
        }
    }
    out
}

fn parse_value(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        unescape_quoted(&trimmed[1..trimmed.len() - 1])
    } else {
        trimmed.to_string()
    }
}

fn parse_key_value(line: &str) -> Option<(String, String)> {
    let (raw_key, raw_value) = line.split_once('=')?;
    let key = parse_value(raw_key);
    let value = parse_value(raw_value);
    Some((key, value))
}

fn escape_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn append_section(output: &mut String, section_name: &str, values: &HashMap<String, String>) {
    output.push('[');
    output.push_str(section_name);
    output.push_str("]\n");

    let sorted: BTreeMap<&String, &String> = values.iter().collect();
    for (key, value) in sorted {
        output.push('"');
        output.push_str(&escape_value(key));
        output.push_str("\" = \"");
        output.push_str(&escape_value(value));
        output.push_str("\"\n");
    }
    output.push('\n');
}

fn parse_config(config_data: &str) -> Result<Config, Error> {
    Logger::debug("parsing config file contents");
    let mut config = Config {
        games: HashMap::new(),
        managers: HashMap::new(),
        pipes: HashMap::new(),
    };

    enum Section {
        None,
        Games,
        Managers,
        Pipes,
    }

    let mut current = Section::None;
    for raw_line in config_data.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            let section = &line[1..line.len() - 1].trim();
            Logger::trace(format!("entered config section [{}]", section));
            current = match *section {
                "games" => Section::Games,
                "managers" => Section::Managers,
                "pipes" => Section::Pipes,
                _ => Section::None,
            };
            continue;
        }

        let (key, value) = parse_key_value(line).ok_or_else(|| {
            Logger::warn(format!("invalid config line encountered: {}", line));
            Error::new(
                ErrorKind::InvalidData,
                format!("Invalid config line: {}", line),
            )
        })?;
        Logger::trace(format!("parsed config entry key '{}'", key));

        match current {
            Section::Games => {
                config.games.insert(key, value);
            }
            Section::Managers => {
                config.managers.insert(key, value);
            }
            Section::Pipes => {
                config.pipes.insert(key, value);
            }
            Section::None => {
                Logger::error("key/value pair outside a known config section");
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "Key/value outside of known section",
                ));
            }
        }
    }

    Logger::info(format!(
        "config parsed: {} game mappings, {} managers, {} pipes",
        config.games.len(),
        config.managers.len(),
        config.pipes.len()
    ));
    Ok(config)
}

fn to_config_text(config: &Config) -> String {
    let mut output = String::new();
    append_section(&mut output, "games", &config.games);
    append_section(&mut output, "managers", &config.managers);
    append_section(&mut output, "pipes", &config.pipes);
    output
}

impl Config {
    /// read config from disc
    pub fn read(config_path: &Path) -> Result<Config, Error> {
        Logger::debug(format!("reading config from {}", config_path.display()));
        let config_path_file = config_path.join("config.toml");

        // Guard against maliciously large config files before reading into memory.
        if let Ok(meta) = std::fs::metadata(&config_path_file) {
            if meta.len() > MAX_CONFIG_BYTES {
                Logger::error(format!("config file exceeds size limit ({} bytes)", meta.len()));
                return Err(Error::new(ErrorKind::InvalidData, "config file too large"));
            }
        }

        let config_data = match read_to_string(&config_path_file) {
            Ok(s) => s,
            Err(e) => match e.kind() {
                ErrorKind::NotFound => {
                    Logger::warn("config file not found, using empty default config");
                    EMPTY_CONFIG.to_string()
                }
                _ => {
                    Logger::error(format!("failed to read config file: {}", e));
                    return Err(e);
                }
            },
        };
        parse_config(&config_data)
    }

    /// write configuration to disc
    pub fn write_config(&self, config_path: &Path) -> Result<(), String> {
        Logger::debug(format!("writing config to {}", config_path.display()));
        let updated_config = to_config_text(self);

        let config_file_path = config_path
            .join("config.toml")
            .to_string_lossy()
            .to_string();
        let config_temp_path = config_file_path.clone() + ".tmp";

        let mut buffer = File::create(&config_temp_path).map_err(|e| {
            let msg = format!("Failed to create temp config file: {}", e);
            Logger::error(&msg);
            msg
        })?;

        buffer.write_all(updated_config.as_bytes()).map_err(|e| {
            let msg = format!("Failed to write config file: {}", e);
            Logger::error(&msg);
            msg
        })?;

        rename(&config_temp_path, &config_file_path).map_err(|e| {
            let msg = format!("Failed to apply config file changes: {}", e);
            Logger::error(&msg);
            msg
        })?;

        Logger::info("configuration persisted successfully");

        Ok(())
    }

    /// assign a game to be handled by a manager
    pub fn assign(&mut self, manager: &str, game: &str) -> Result<(), String> {
        Logger::debug(format!(
            "assigning manager '{}' to game '{}'",
            manager, game
        ));
        self.games.insert(game.to_string(), manager.to_string());
        Ok(())
    }

    /// register a manager
    pub fn register(&mut self, manager: &str, command: &str) -> Result<(), String> {
        Logger::debug(format!("registering manager '{}'", manager));
        self.managers
            .insert(manager.to_string(), command.to_string());
        Ok(())
    }

    /// deregister a manager
    pub fn deregister(&mut self, manager: &str) -> Result<(), String> {
        Logger::warn(format!("removing manager '{}'", manager));
        self.managers.remove(manager);
        self.pipes.remove(manager);
        Ok(())
    }

    /// register a named pipe to send urls to
    pub fn register_pipe(&mut self, manager: &str, pipe: &str) -> Result<(), String> {
        if !is_valid_pipe_name(pipe) {
            Logger::error(format!("invalid pipe name '{}'", pipe));
            return Err("Pipe name may only contain A-Z, a-z, 0-9, '.', '_' or '-' and must be 1-128 characters".to_string());
        }

        if !self.managers.contains_key(manager) {
            Logger::error(format!(
                "cannot register pipe '{}' because manager '{}' does not exist",
                pipe, manager
            ));
            return Err(format!(r#"Manager {} is not registered"#, manager));
        }
        Logger::info(format!(
            "registering pipe '{}' for manager '{}'",
            pipe, manager
        ));
        self.pipes.insert(manager.to_string(), pipe.to_string());
        Ok(())
    }

    pub fn resolve(&self, game: &str) -> Result<String, String> {
        let manager: String;

        if self.games.contains_key(game) {
            manager = self.games[game].to_string();
        } else if self.games.contains_key("_") {
            Logger::warn(format!("using fallback manager for game '{}'", game));
            manager = self.games["_"].to_string();
        } else {
            Logger::error(format!("no manager mapping found for game '{}'", game));
            return Err(format!("No manager for game {}", game));
        }

        Logger::debug(format!("resolved game '{}' to manager '{}'", game, manager));

        return Ok(manager);
    }
}
