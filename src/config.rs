use clap::Parser;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Parser, Debug, Clone)]
#[command(author, version, about = "J-Link RTT Rust Orchestrator", long_about = None)]
pub struct CliArgs {
    #[arg(long, help = "Load explicit .prj.env file")]
    pub config: Option<String>,

    #[arg(long, help = "Limit default config search to this project root")]
    pub project_root: Option<String>,

    #[arg(long, help = "Print resolved config and exit")]
    pub print_config: bool,

    #[arg(long, help = "Create .prj.env with current settings and exit")]
    pub init: bool,

    #[arg(long, help = "Override auto-detected JLinkGDBServer command")]
    pub jlink_gdb_server: Option<String>,

    #[arg(long, help = "Override auto-detected GDB command")]
    pub gdb: Option<String>,

    #[arg(long, help = "Override auto-detected nc command")]
    pub nc: Option<String>,

    #[arg(long, help = "J-Link target device, required unless configured")]
    pub device: Option<String>,

    #[arg(long, value_name = "INTERFACE", help = "J-Link interface, default: SWD")]
    pub r#if: Option<String>,

    #[arg(long, value_name = "KHZ", help = "J-Link speed in kHz, default: 4000")]
    pub speed: Option<String>,

    #[arg(long, value_name = "SERIAL", help = "J-Link serial number, optional")]
    pub serial: Option<String>,

    #[arg(long, help = "Local host for GDB/RTT ports, default: 127.0.0.1")]
    pub host: Option<String>,

    #[arg(long, value_name = "PORT", help = "GDB server port, default: 2331")]
    pub gdb_port: Option<String>,

    #[arg(long, value_name = "PORT", help = "RTT telnet port, default: 19021")]
    pub rtt_port: Option<String>,

    #[arg(long = "rtt-timeout", value_name = "SECONDS", help = "Overall RTT interaction timeout in seconds (match wait limit in match mode, timed capture duration otherwise; absent/0 = continuous stream; match mode defaults to 30)")]
    pub timeout: Option<String>,

    #[arg(long, value_name = "SECONDS", help = "Port ready timeout, default: 10")]
    pub ready_timeout: Option<String>,

    #[arg(long, value_name = "FILE", help = "JLinkGDBServer log file, default: <tmp_dir>/jlink_gdb_server.log")]
    pub log: Option<String>,

    #[arg(long, value_name = "FILE", help = "GDB resume log file, default: <tmp_dir>/jlink_gdb_resume.log")]
    pub gdb_log: Option<String>,

    #[arg(long, value_name = "FILE", help = "Save RTT output to file while streaming stdout")]
    pub out: Option<String>,

    #[arg(long = "rtt-match", value_name = "PATTERN", help = "Exit 0 after this fixed text appears in RTT output")]
    pub rtt_match: Option<String>,

    #[arg(long = "rtt-delay", value_name = "SEC", help = "Delay after session exit for USB/port release, default: 1.5")]
    pub rtt_delay: Option<f64>,

    #[arg(long, help = "Do not reset the target before reading RTT")]
    pub no_reset: bool,

    #[arg(long, help = "Do not connect GDB to resume the target")]
    pub no_resume: bool,

    #[arg(long, help = "Stop a running RTT session (kills JLinkGDBServer, triggers clean shutdown")]
    pub stop: bool,

    #[arg(long, value_name = "COMMAND", help = "Send a command to a running RTT session downlink and exit")]
    pub send: Option<String>,

    #[arg(long, help = "Treat --send COMMAND as a hex string (e.g. '0102030a')")]
    pub hex: bool,

    #[arg(long, help = "Do not append newline when sending with --send")]
    pub no_newline: bool,

    #[arg(short, long, help = "Enable interactive stdin (type commands directly to RTT downlink)")]
    pub interactive: bool,

    #[arg(long, value_name = "PATTERN", help = "Search J-Link device database for PATTERN")]
    pub search_device: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub config_file: Option<PathBuf>,
    pub local_config_file: Option<PathBuf>,
    pub project_root: PathBuf,
    pub jlink_gdb_server: Option<String>,
    pub gdb: Option<String>,
    pub nc: Option<String>,
    pub host: String,
    pub device: Option<String>,
    pub jlink_if: String,
    pub speed: String,
    pub jlink_serial: Option<String>,
    pub gdb_port: String,
    pub rtt_port: String,
    pub ready_timeout: u32,
    pub log_file: String,
    pub gdb_log_file: String,
    pub rtt_out_file: Option<String>,
    pub rtt_match_pattern: Option<String>,
    pub rtt_timeout: Option<u64>,
    pub rtt_delay: String,
    pub reset_target: String,  // "0" or "1"
    pub resume_target: String, // "0" or "1"
    
    // Command flags
    pub print_config: bool,
    pub init: bool,
    pub stop: bool,
    pub send: Option<String>,
    pub send_hex: bool,
    pub no_newline: bool,
    pub interactive: bool,
    pub search_device: Option<String>,
}

/// Strip Windows specific UNC path prefixes like `\\?\` or `\\?\UNC\` to make paths user-friendly
pub fn clean_windows_path(path: PathBuf) -> PathBuf {
    if cfg!(target_os = "windows") {
        let path_str = path.to_string_lossy();
        if path_str.starts_with(r"\\?\UNC\") {
            // Example: \\?\UNC\wsl.localhost\ub24\vhdx\ -> \\wsl.localhost\ub24\vhdx\
            PathBuf::from(format!(r"\\{}", &path_str[8..]))
        } else if path_str.starts_with(r"\\?\") {
            // Example: \\?\D:\nordic\ -> D:\nordic\
            PathBuf::from(path_str[4..].to_string())
        } else {
            path
        }
    } else {
        path
    }
}

pub fn get_project_temp_dir(project_root: &Path) -> PathBuf {
    let system_temp = std::env::temp_dir();
    let project_name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let clean_name = project_name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>();

    let mut hasher = DefaultHasher::new();
    project_root.to_string_lossy().hash(&mut hasher);
    let hash = format!("{:x}", hasher.finish());

    let path = system_temp.join(format!("jlink-rtt-{}-{}", clean_name, hash));
    let _ = std::fs::create_dir_all(&path);
    path
}

impl AppConfig {
    pub fn resolve(args: CliArgs) -> Result<Self, String> {
        let cwd = std::env::current_dir()
            .map_err(|e| format!("Failed to get current directory: {}", e))?;
        // Canonicalize cwd to unify prefix (especially for UNC paths like \\?\UNC\ on Windows)
        let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
        let cwd = clean_windows_path(cwd);

        // 1. Resolve project root
        let project_root = Self::resolve_project_root(&args, &cwd)?;

        // Check if current directory is at or below project root
        if !Self::path_is_at_or_below(&cwd, &project_root) {
            return Err(format!(
                "[ERROR] Current directory is outside project root: {}\n[INFO] Run from within the project, or pass --project-root <DIR>.",
                project_root.display()
            ));
        }

        // 2. Get project-specific temp directory
        let temp_dir = get_project_temp_dir(&project_root);

        // 3. Find and load config file (base .prj.env + overlay .prj.local.env)
        let (config_file, local_config_file, config_map) = Self::load_config(&args, &cwd, &project_root)?;

        // 4. Build configuration with precedence: CLI > .prj.local.env > .prj.env > Defaults
        // config_map already contains merged overlay, so CLI > merged map > default.
        // Only canonical TODO keys are read (RTT_MATCH / RTT_TIMEOUT / RTT_DELAY).
        let get_val = |key: &str, cli_val: Option<String>, default: &str| -> String {
            cli_val
                .or_else(|| config_map.get(key).cloned())
                .unwrap_or_else(|| default.to_string())
        };

        let get_opt_val = |key: &str, cli_val: Option<String>| -> Option<String> {
            cli_val.or_else(|| config_map.get(key).cloned())
        };

        // GDB/Reset flags precedence: CLI --no-reset overrides reset_target in env
        let reset_target = if args.no_reset {
            "0".to_string()
        } else {
            config_map.get("RESET_TARGET").cloned().unwrap_or_else(|| "1".to_string())
        };

        let resume_target = if args.no_resume {
            "0".to_string()
        } else {
            config_map.get("RESUME_TARGET").cloned().unwrap_or_else(|| "1".to_string())
        };

        let jlink_gdb_server = args.jlink_gdb_server.or_else(|| config_map.get("JLINK_GDB_SERVER").cloned());
        let gdb = args.gdb.or_else(|| config_map.get("GDB").cloned());
        let nc = args.nc.or_else(|| config_map.get("NC").cloned());

        let host = get_val("LISTEN_HOST", args.host, "127.0.0.1");
        let device = get_opt_val("JLINK_DEVICE", args.device);
        let jlink_if = get_val("JLINK_IF", args.r#if, "SWD");
        let speed = get_val("JLINK_SPEED", args.speed, "4000");
        let jlink_serial = get_opt_val("JLINK_SERIAL", args.serial);
        let gdb_port = get_val("GDB_PORT", args.gdb_port, "2331");
        let rtt_port = get_val("RTT_PORT", args.rtt_port, "19021");
        let raw_ready_timeout = get_val("RTT_READY_TIMEOUT", args.ready_timeout, "10");
        // 就绪等待超时同样在装配阶段校验, 与 RTT_TIMEOUT 的 fail-loud 标准一致;
        // 空值视为未配置取默认; 0 无意义 (主流程刚 spawn 自己的 GDB Server,
        // 单次探测必然失败), 与非数字一样直接报错
        let ready_timeout = match raw_ready_timeout.trim() {
            "" => 10u32,
            value => match value.parse::<u32>() {
                Ok(secs) if secs >= 1 => Ok(secs),
                _ => Err(format!(
                    "[ERROR] Invalid --ready-timeout / RTT_READY_TIMEOUT value '{}'. \
                     It must be a positive integer number of seconds (at least 1).",
                    value
                )),
            }?,
        };

        // Construct dynamic defaults based on the project temp directory
        let default_log = temp_dir.join("jlink_gdb_server.log").to_string_lossy().to_string();
        let default_gdb_log = temp_dir.join("jlink_gdb_resume.log").to_string_lossy().to_string();
        
        let log_file = get_val("JLINK_LOG_FILE", args.log, &default_log);
        let gdb_log_file = get_val("JLINK_GDB_LOG_FILE", args.gdb_log, &default_gdb_log);
        
        let rtt_out_file = get_opt_val("RTT_OUT_FILE", args.out);
        // 匹配关键词 (RTT_MATCH, 与 pyt 同口径, 无别名无旧键):
        // 显式 --rtt-match "" 报错; RTT_MATCH 空值视作未配置 (留空表示持续监听)
        let rtt_match_pattern = match args.rtt_match {
            Some(raw) => {
                if raw.is_empty() {
                    return Err(
                        "[ERROR] --rtt-match pattern cannot be empty. Omit --rtt-match (or leave \
                         RTT_MATCH unset) for continuous streaming, or provide a \
                         non-empty keyword."
                            .to_string(),
                    );
                }
                Some(raw)
            }
            None => config_map
                .get("RTT_MATCH")
                .cloned()
                .filter(|v| !v.is_empty()),
        };

        // 统一交互超时 (与 pyt --rtt-timeout 同口径, 无别名无旧键):
        // --rtt-timeout / RTT_TIMEOUT; 匹配模式=等待上限 (缺省 30s), 纯抓取=定时时长;
        // 0 表示不限时持续流; 非数字直接报错
        let unified_raw = get_opt_val("RTT_TIMEOUT", args.timeout)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        fn parse_timeout_secs(value: &str, flag: &str) -> Result<Option<u64>, String> {
            match value.parse::<u64>() {
                // "0" 与缺省等价, 归一为不限时
                Ok(0) => Ok(None),
                Ok(secs) => Ok(Some(secs)),
                Err(_) => Err(format!(
                    "[ERROR] Invalid {} value '{}'. \
                     It must be a non-negative integer number of seconds \
                     (0 or unset = continuous stream).",
                    flag, value
                )),
            }
        }
        let rtt_timeout = if rtt_match_pattern.is_some() {
            match unified_raw {
                Some(v) => parse_timeout_secs(&v, "--rtt-timeout / RTT_TIMEOUT")?,
                None => Some(30),
            }
        } else {
            match unified_raw {
                Some(v) => parse_timeout_secs(&v, "--rtt-timeout / RTT_TIMEOUT")?,
                None => None,
            }
        };

        // 释放延时 (TODO 新键 RTT_DELAY, 无旧键兼容): 默认 1.5s
        let rtt_delay = args.rtt_delay.map(|v| v.to_string())
            .or_else(|| config_map.get("RTT_DELAY").cloned())
            .unwrap_or_else(|| "1.5".to_string());

        Ok(AppConfig {
            config_file,
            local_config_file,
            project_root,
            jlink_gdb_server,
            gdb,
            nc,
            host,
            device,
            jlink_if,
            speed,
            jlink_serial,
            gdb_port,
            rtt_port,
            ready_timeout,
            log_file,
            gdb_log_file,
            rtt_out_file,
            rtt_match_pattern,
            rtt_timeout,
            rtt_delay,
            reset_target,
            resume_target,
            print_config: args.print_config,
            init: args.init,
            stop: args.stop,
            send: args.send,
            send_hex: args.hex,
            no_newline: args.no_newline,
            interactive: args.interactive,
            search_device: args.search_device,
        })
    }

    fn resolve_project_root(args: &CliArgs, cwd: &Path) -> Result<PathBuf, String> {
        if let Some(ref root_str) = args.project_root {
            let path = PathBuf::from(root_str);
            let abs_path = std::fs::canonicalize(&path)
                .map_err(|_| format!("[ERROR] Invalid project root: {}.\n[INFO] Pass a valid directory with --project-root <DIR>.", root_str))?;
            let abs_path = clean_windows_path(abs_path);
            if !abs_path.is_dir() {
                return Err(format!("[ERROR] Invalid project root: {}.\n[INFO] Pass a valid directory with --project-root <DIR>.", root_str));
            }
            return Ok(abs_path);
        }

        // Try to find git root
        if let Some(git_root) = Self::find_git_root() {
            if let Ok(abs_git_root) = std::fs::canonicalize(&git_root) {
                let abs_git_root = clean_windows_path(abs_git_root);
                return Ok(abs_git_root);
            }
        }

        // Fallback to current working directory
        Ok(cwd.to_path_buf())
    }

    fn find_git_root() -> Option<PathBuf> {
        let output = Command::new("git")
            .args(&["rev-parse", "--show-toplevel"])
            .output()
            .ok()?;
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path_str.is_empty() {
                return Some(PathBuf::from(path_str));
            }
        }
        None
    }

    fn path_is_at_or_below(path: &Path, root: &Path) -> bool {
        path.starts_with(root)
    }

    fn parse_env_file(path: &Path) -> Result<HashMap<String, String>, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("[ERROR] Failed to read config file {}: {}", path.display(), e))?;

        let mut map = HashMap::new();
        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            if let Some(pos) = trimmed.find('=') {
                let key = trimmed[..pos].trim();
                let val = trimmed[pos + 1..].trim();

                // Key validation: must start with letter/underscore and contain alphanumeric/underscore
                if key.is_empty()
                    || !key.chars().next().map_or(false, |c| c.is_ascii_alphabetic() || c == '_')
                    || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    return Err(format!(
                        "[ERROR] Invalid config line in {}:{}: {}\n[INFO] Fix or remove the line, then re-run. Valid format: KEY=VALUE",
                        path.display(),
                        line_num + 1,
                        trimmed
                    ));
                }

                // Strip single/double quotes from value
                let mut val_str = val.to_string();
                if (val_str.starts_with('"') && val_str.ends_with('"'))
                    || (val_str.starts_with('\'') && val_str.ends_with('\''))
                {
                    if val_str.len() >= 2 {
                        val_str = val_str[1..val_str.len() - 1].to_string();
                    }
                }

                map.insert(key.to_string(), val_str);
            } else {
                return Err(format!(
                    "[ERROR] Invalid config line in {}:{}: {}\n[INFO] Fix or remove the line, then re-run. Valid format: KEY=VALUE",
                    path.display(),
                    line_num + 1,
                    trimmed
                ));
            }
        }
        Ok(map)
    }

    fn load_config(
        args: &CliArgs,
        cwd: &Path,
        project_root: &Path,
    ) -> Result<(Option<PathBuf>, Option<PathBuf>, HashMap<String, String>), String> {
        // Explicit --config: single-file mode, no local overlay (explicit wins).
        if let Some(ref path_str) = args.config {
            let path = PathBuf::from(path_str);
            if !path.is_file() {
                return Err(format!(
                    "[ERROR] Config file not found: {}.\n[INFO] Check the --config path, or run without --config to auto-discover .prj.env.",
                    path_str
                ));
            }
            let map = Self::parse_env_file(&path)?;
            return Ok((Some(path), None, map));
        }

        // Auto-discover base .prj.env upward until project_root.
        let config_path = Self::find_default_config(cwd, project_root);

        let mut map = HashMap::new();
        if let Some(ref path) = config_path {
            let base = Self::parse_env_file(path)?;
            map.extend(base);
        }

        // Overlay .prj.local.env (local private, never committed).
        // Candidates: sibling of discovered base + project_root/.prj.local.env (dedup).
        let mut local_candidates: Vec<PathBuf> = Vec::new();
        if let Some(ref base_path) = config_path {
            if let Some(parent) = base_path.parent() {
                let sib = parent.join(".prj.local.env");
                if sib.is_file() {
                    local_candidates.push(sib);
                }
            }
        }
        let root_local = project_root.join(".prj.local.env");
        if root_local.is_file() && !local_candidates.contains(&root_local) {
            local_candidates.push(root_local);
        }

        let mut local_config_path: Option<PathBuf> = None;
        for cand in local_candidates {
            let overlay = Self::parse_env_file(&cand)?;
            map.extend(overlay);
            local_config_path = Some(cand);
        }

        Ok((config_path, local_config_path, map))
    }

    fn find_default_config(cwd: &Path, project_root: &Path) -> Option<PathBuf> {
        let mut current = cwd.to_path_buf();
        loop {
            let candidate = current.join(".prj.env");
            if candidate.is_file() {
                return Some(candidate);
            }
            if current == project_root {
                break;
            }
            if let Some(parent) = current.parent() {
                current = parent.to_path_buf();
            } else {
                break;
            }
        }
        None
    }
}
