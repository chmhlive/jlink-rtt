mod config;
mod devices;
mod preflight;
mod orchestrator;

use clap::Parser;
use config::{CliArgs, AppConfig};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn print_config(config: &AppConfig) {
    if let Some(ref file) = config.config_file {
        println!("CONFIG_FILE={}", file.display());
    } else {
        println!("CONFIG_FILE=");
    }
    println!("PROJECT_ROOT={}", config.project_root.display());
    
    if let Some(ref srv) = config.jlink_gdb_server {
        println!("JLINK_GDB_SERVER={}", srv);
    }
    if let Some(ref gdb) = config.gdb {
        println!("GDB={}", gdb);
    }
    if let Some(ref nc) = config.nc {
        println!("NC={}", nc);
    }
    println!("HOST={}", config.host);
    if let Some(ref dev) = config.device {
        println!("DEVICE={}", dev);
    } else {
        println!("DEVICE=");
    }
    println!("JLINK_IF={}", config.jlink_if);
    println!("SPEED={}", config.speed);
    if let Some(ref serial) = config.jlink_serial {
        println!("JLINK_SERIAL={}", serial);
    } else {
        println!("JLINK_SERIAL=");
    }
    println!("GDB_PORT={}", config.gdb_port);
    println!("RTT_PORT={}", config.rtt_port);
    println!("RTT_READY_TIMEOUT={}", config.ready_timeout);
    println!("LOG_FILE={}", config.log_file);
    println!("GDB_LOG_FILE={}", config.gdb_log_file);
    if let Some(ref out) = config.rtt_out_file {
        println!("RTT_OUT_FILE={}", out);
    } else {
        println!("RTT_OUT_FILE=");
    }
    if let Some(ref pattern) = config.rtt_match_pattern {
        println!("RTT_MATCH_PATTERN={}", pattern);
    } else {
        println!("RTT_MATCH_PATTERN=");
    }
    println!("RTT_MATCH_TIMEOUT={}", config.rtt_match_timeout);
    println!("RESET_TARGET={}", config.reset_target);
    println!("RESUME_TARGET={}", config.resume_target);
}

fn handle_search_device(pattern: &str, project_root: &std::path::Path) {
    match devices::search_devices(pattern, project_root) {
        Ok(results) => {
            if results.is_empty() {
                println!("[INFO] No J-Link devices match '{}'.", pattern);
                println!("[INFO] Try a broader pattern, e.g. 'nrf52' instead of 'nrf52840'.");
                std::process::exit(1);
            }
            for r in results {
                println!("{} | {}", r.vendor, r.device);
            }
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}

fn get_current_exe_name() -> String {
    std::env::args().next()
        .and_then(|path| {
            std::path::Path::new(&path)
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| {
                    if cfg!(target_os = "windows") {
                        format!(r".\{}", s)
                    } else {
                        format!("./{}", s)
                    }
                })
        })
        .unwrap_or_else(|| {
            if cfg!(target_os = "windows") {
                r".\jlink-rtt.exe".to_string()
            } else {
                "./jlink-rtt".to_string()
            }
        })
}

fn handle_init(mut config: AppConfig, explicit_config_path: Option<String>) {
    let exe_name = get_current_exe_name();
    let device_pattern = match config.device.as_ref() {
        Some(d) => d,
        None => {
            eprintln!("[ERROR] DEVICE is required for --init.");
            eprintln!("[INFO] Scan the project for the DEVICE name (e.g. NRF52840_XXAA).");
            eprintln!("[INFO] Use --search-device to confirm the exact name:");
            eprintln!("[INFO]   {} --search-device <pattern>", exe_name);
            std::process::exit(1);
        }
    };

    // Resolve fuzzy device name
    let resolved_device = match devices::resolve_device_name(device_pattern, &config.project_root) {
        Ok(name) => name,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    config.device = Some(resolved_device.clone());

    // Determine target config path
    let config_path = match explicit_config_path {
        Some(path) => PathBuf::from(path),
        None => config.project_root.join(".jlink-rtt.env"),
    };

    if config_path.is_file() {
        eprintln!("[ERROR] Config file already exists: {}", config_path.display());
        eprintln!("[INFO] To inspect it, run:");
        eprintln!("[INFO]   {} --print-config", exe_name);
        eprintln!("[INFO] To re-create, remove the file first: rm {}", config_path.display());
        std::process::exit(1);
    }

    // Auto-detect J-Link serial if not explicitly set
    if config.jlink_serial.is_none() {
        let serials = preflight::detect_jlink_serials();
        if serials.len() == 1 {
            config.jlink_serial = Some(serials[0].clone());
        }
    }

    // Prepare config file contents
    let mut content = format!(
        "# J-Link RTT project configuration\n\
         DEVICE={}\n\
         JLINK_IF={}\n\
         SPEED={}\n\
         HOST={}\n\
         GDB_PORT={}\n\
         RTT_PORT={}\n\
         RTT_READY_TIMEOUT={}\n\
         LOG_FILE={}\n\
         GDB_LOG_FILE={}\n",
        resolved_device,
        config.jlink_if,
        config.speed,
        config.host,
        config.gdb_port,
        config.rtt_port,
        config.ready_timeout,
        config.log_file,
        config.gdb_log_file,
    );

    if let Some(ref serial) = config.jlink_serial {
        content.push_str(&format!("JLINK_SERIAL={}\n", serial));
    }

    if let Err(e) = fs::write(&config_path, content) {
        eprintln!("[ERROR] Failed to write config file: {}", e);
        std::process::exit(1);
    }

    config.config_file = Some(config_path.clone());
    println!("[INFO] Created config: {}", config_path.display());
    println!("[INFO] Config created. Now run the capture command again:");
    let default_out = if cfg!(target_os = "windows") { "rtt.log" } else { "/tmp/rtt.log" };
    println!("[INFO]   {} --out {}", exe_name, default_out);

    print_config(&config);
    std::process::exit(0);
}

#[cfg(unix)]
fn kill_process(pid: &str) {
    let _ = Command::new("kill").arg(pid).status();
}

#[cfg(not(unix))]
fn kill_process(pid: &str) {
    let _ = Command::new("taskkill").args(&["/F", "/PID", pid]).status();
}

#[cfg(unix)]
fn pkill_gdb_server(config: &AppConfig) -> bool {
    let pkill_patterns = [
        format!("JLinkGDBServerCLExe.*-port {}.*-RTTTelnetPort {}", config.gdb_port, config.rtt_port),
        format!("JLinkGDBServer.*-port {}.*-RTTTelnetPort {}", config.gdb_port, config.rtt_port),
    ];

    let mut killed_any = false;
    for pat in &pkill_patterns {
        if let Ok(status) = Command::new("pkill")
            .args(&["-f", pat])
            .status() {
            if status.success() {
                killed_any = true;
            }
        }
    }
    killed_any
}

#[cfg(not(unix))]
fn pkill_gdb_server(_config: &AppConfig) -> bool {
    let _ = Command::new("taskkill").args(&["/F", "/IM", "JLinkGDBServer.exe"]).status();
    let _ = Command::new("taskkill").args(&["/F", "/IM", "JLinkGDBServerCLExe.exe"]).status();
    true
}

fn handle_stop(config: &AppConfig) {
    let temp_dir = config::get_project_temp_dir(&config.project_root);
    let pid_file = temp_dir.join("jlink_rtt.pid");
    let mut stopped_any = false;

    if pid_file.is_file() {
        if let Ok(pid_str) = fs::read_to_string(&pid_file) {
            let pid = pid_str.trim();
            if !pid.is_empty() {
                eprintln!("[INFO] Stopping running orchestrator session (PID {})...", pid);
                kill_process(pid);
                stopped_any = true;
            }
        }
        let _ = fs::remove_file(&pid_file);
    }

    // Stop JLinkGDBServer instances associated with these ports
    let pkill_killed = pkill_gdb_server(config);
    
    stopped_any = stopped_any || pkill_killed;

    // Note: since taskkill status is hard to inspect safely on Windows without parsing,
    // we always output stop info as long as stopped_any is set, or if we attempted pkill.
    if stopped_any || cfg!(not(unix)) {
        println!("[INFO] Stop signal sent to RTT session (port {}).", config.rtt_port);
        std::process::exit(0);
    } else {
        eprintln!("[WARN] No running RTT session found for this project.");
        std::process::exit(1);
    }
}

#[tokio::main]
async fn main() {
    let args = CliArgs::parse();
    let explicit_config = args.config.clone();
    
    let config = match AppConfig::resolve(args) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Mode dispatch: stop
    if config.stop {
        handle_stop(&config);
    }

    // Mode dispatch: search_device
    if let Some(ref pattern) = config.search_device {
        handle_search_device(pattern, &config.project_root);
    }

    // Mode dispatch: init
    if config.init {
        handle_init(config.clone(), explicit_config);
    }
    
    if config.print_config {
        print_config(&config);
        std::process::exit(0);
    }

    // Normal capture mode check: if DEVICE is missing
    if config.device.is_none() {
        let exe_name = get_current_exe_name();
        println!("[INFO] No .jlink-rtt.env found and no --device given.");
        println!("[INFO] Scan the project for the DEVICE name (e.g. NRF52840_XXAA).");
        println!("[INFO] Use --search-device to confirm the exact name:");
        println!("[INFO]   {} --search-device <pattern>", exe_name);
        
        let serials = preflight::detect_jlink_serials();
        let mut init_cmd = format!(
            "{} --init --device <DEVICE> --if {} --speed {} --host {} --gdb-port {} --rtt-port {} --timeout {}",
            exe_name, config.jlink_if, config.speed, config.host, config.gdb_port, config.rtt_port, config.ready_timeout
        );

        if serials.len() == 1 {
            init_cmd.push_str(&format!(" --serial {}", serials[0]));
        } else if serials.len() > 1 {
            println!("[INFO] Multiple J-Link probes detected. Ask the user which serial to use.");
            println!("[INFO] Available serials:");
            for s in &serials {
                println!("[INFO]   {}", s);
            }
            init_cmd.push_str(" --serial <SERIAL>");
        }

        println!("[INFO] Then run:");
        println!("[INFO]   {}", init_cmd);
        println!("[INFO] Review all parameters above before executing. If the project uses a different interface (e.g. JTAG), adjust --if.");
        std::process::exit(0);
    }

    // Perform tools discovery and preflight checks before running orchestration
    let tools = match preflight::perform_preflight(&config) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    // Instantiate orchestrator to manage child processes and cleanup
    let mut orchestrator = orchestrator::Orchestrator::new(&config);
    if let Err(e) = orchestrator.write_pid() {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    // Start JLinkGDBServer
    if let Err(e) = orchestrator.start_gdb_server(&config, &tools).await {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    let timeout_secs = config.ready_timeout.parse::<u32>().unwrap_or(10);

    // Wait for GDB port ready
    if let Err(e) = orchestrator.wait_for_port(&config.host, &config.gdb_port, "GDB", timeout_secs).await {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    // Reset and resume target
    if let Err(e) = orchestrator.resume_target(&config, &tools).await {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    // Wait for RTT port ready
    if let Err(e) = orchestrator.wait_for_port(&config.host, &config.rtt_port, "RTT", timeout_secs).await {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    // Start RTT capture
    let orchestrator_ref = &orchestrator;
    let config_ref = &config;
    let capture_fut = orchestrator_ref.run_rtt_capture(config_ref);

    #[cfg(unix)]
    let signal_fut = async {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigint = signal(SignalKind::interrupt()).unwrap();
        let mut sigterm = signal(SignalKind::terminate()).unwrap();
        let mut sighup = signal(SignalKind::hangup()).unwrap();

        tokio::select! {
            _ = sigint.recv() => { eprintln!("[INFO] Received SIGINT. Cleaning up..."); }
            _ = sigterm.recv() => { eprintln!("[INFO] Received SIGTERM. Cleaning up..."); }
            _ = sighup.recv() => { eprintln!("[INFO] Received SIGHUP. Cleaning up..."); }
        }
    };

    #[cfg(not(unix))]
    let signal_fut = async {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("[INFO] Received Ctrl-C. Cleaning up...");
    };

    tokio::select! {
        res = capture_fut => {
            if let Err(e) = res {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        }
        _ = signal_fut => {}
    }
}


