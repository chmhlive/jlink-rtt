mod config;
mod devices;
mod downlink;
mod preflight;
mod orchestrator;

use clap::Parser;
use config::{CliArgs, AppConfig};
use std::fs;
use std::process::Command;

fn print_config(config: &AppConfig) {
    println!("PROJECT_ROOT={}", config.project_root.display());
    println!("JLINK_LOG_FILE={}", config.log_file);
    println!("JLINK_GDB_LOG_FILE={}", config.gdb_log_file);
}

const EXAMPLES: &str = "\
jlink-rtt usage examples (replace nRF52840_xxAA with your device):

# 1. Find the exact device name
jlink-rtt --search-device nrf52840

# 2. Timed capture (auto-stop after 60s)
jlink-rtt --device nRF52840_xxAA --log-out rtt.log --log-timeout 60

# 3. Pattern-triggered capture
jlink-rtt --device nRF52840_xxAA --log-match \"Application started\" --log-timeout 30

# 4. Continuous stream, then stop from another shell
jlink-rtt --device nRF52840_xxAA --log-out rtt.log
jlink-rtt --stop

# 5. Interactive console
jlink-rtt --device nRF52840_xxAA --log-out rtt.log -i

# 6. Send a command to a running session
jlink-rtt --send \"help\"
jlink-rtt --send \"0102030a\" --hex

# 7. Multi-probe / custom ports
jlink-rtt --device nRF52840_xxAA --serial 000683041131 --gdb-port 2331 --rtt-port 19021

# 8. Show project root and log paths
jlink-rtt --print-config
";

fn print_examples() {
    println!("{}", EXAMPLES.trim_end());
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
        .unwrap_or_else(|| {
            if cfg!(target_os = "windows") {
                r".\jlink-rtt.exe".to_string()
            } else {
                "./jlink-rtt".to_string()
            }
        })
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

    let ctrl_file = temp_dir.join("rtt_ctrl.port");
    let _ = fs::remove_file(&ctrl_file);

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

async fn handle_send(config: &AppConfig, cmd: &str) {
    let temp_dir = config::get_project_temp_dir(&config.project_root);
    let port_file = temp_dir.join("rtt_ctrl.port");
    if !port_file.is_file() {
        eprintln!("[ERROR] No running RTT session found to send command.");
        eprintln!("[INFO] Start an RTT session first with: jlink-rtt --log-out rtt.log");
        std::process::exit(1);
    }
    let port_str = match fs::read_to_string(&port_file) {
        Ok(s) => s.trim().to_string(),
        Err(e) => {
            eprintln!("[ERROR] Failed to read control port file {}: {}", port_file.display(), e);
            std::process::exit(1);
        }
    };
    let port: u16 = match port_str.parse() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("[ERROR] Invalid port in {}: '{}'.", port_file.display(), port_str);
            std::process::exit(1);
        }
    };

    let payload: Vec<u8> = if config.send_hex {
        match downlink::parse_hex_string(cmd) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        }
    } else {
        match downlink::unescape_string(cmd) {
            Ok(mut b) => {
                if !config.no_newline && !b.ends_with(b"\n") && !b.ends_with(b"\r") {
                    b.push(b'\n');
                }
                b
            }
            Err(e) => {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        }
    };

    use tokio::io::{AsyncWriteExt, AsyncBufReadExt};
    use std::time::Duration;

    let connect_future = tokio::net::TcpStream::connect(("127.0.0.1", port));
    let mut socket = match tokio::time::timeout(Duration::from_secs(2), connect_future).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let _ = fs::remove_file(&port_file);
            eprintln!("[ERROR] Failed to connect to RTT session control port {}: {}", port, e);
            eprintln!("[INFO] The previous RTT session may have terminated. Cleaned stale control port.");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("[ERROR] Timed out connecting to RTT session control port {}.", port);
            std::process::exit(1);
        }
    };

    let len_bytes = (payload.len() as u32).to_be_bytes();
    if let Err(e) = socket.write_all(&len_bytes).await {
        eprintln!("[ERROR] Failed to write length to control socket: {}", e);
        std::process::exit(1);
    }
    if let Err(e) = socket.write_all(&payload).await {
        eprintln!("[ERROR] Failed to write payload to control socket: {}", e);
        std::process::exit(1);
    }

    let mut resp = String::new();
    let mut reader = tokio::io::BufReader::new(socket);
    let read_future = reader.read_line(&mut resp);
    match tokio::time::timeout(Duration::from_secs(2), read_future).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            eprintln!("[ERROR] Failed to read response from RTT session: {}", e);
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("[ERROR] Timed out waiting for response from RTT session.");
            std::process::exit(1);
        }
    }

    if resp.starts_with("OK") {
        eprintln!("[INFO] Downlink command sent ({} bytes).", payload.len());
        std::process::exit(0);
    } else {
        eprintln!("[ERROR] Downlink failed: {}", resp.trim());
        std::process::exit(1);
    }
}

#[tokio::main]
async fn main() {
    let args = CliArgs::parse();

    if args.example {
        print_examples();
        std::process::exit(0);
    }

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

    // Mode dispatch: send
    if let Some(ref cmd) = config.send {
        handle_send(&config, cmd).await;
    }

    // Mode dispatch: search_device
    if let Some(ref pattern) = config.search_device {
        handle_search_device(pattern, &config.project_root);
    }

    if config.print_config {
        print_config(&config);
        std::process::exit(0);
    }

    // Normal capture mode check: if DEVICE is missing
    if config.device.is_none() {
        let exe_name = get_current_exe_name();
        println!("[INFO] No --device given.");
        println!("[INFO] Scan the project for the DEVICE name (e.g. NRF52840_XXAA).");
        println!("[INFO] Use --search-device to confirm the exact name:");
        println!("[INFO]   {} --search-device PATTERN", exe_name);

        let serials = preflight::detect_jlink_serials();
        let mut capture_cmd = format!(
            "{} --device DEVICE_NAME --if {} --speed {} --host {} --gdb-port {} --rtt-port {} --ready-timeout {}",
            exe_name, config.jlink_if, config.speed, config.host, config.gdb_port, config.rtt_port, config.ready_timeout
        );

        if serials.len() == 1 {
            capture_cmd.push_str(&format!(" --serial {}", serials[0]));
        } else if serials.len() > 1 {
            println!("[INFO] Multiple J-Link probes detected. Ask the user which serial to use.");
            println!("[INFO] Available serials:");
            for s in &serials {
                println!("[INFO]   {}", s);
            }
            capture_cmd.push_str(" --serial SERIAL_NUMBER");
        }

        println!("[INFO] Then run:");
        println!("[INFO]   {}", capture_cmd);
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

    let timeout_secs = config.ready_timeout;

    // Wait for GDB port ready
    if let Err(e) = orchestrator.wait_for_port(&config.host, &config.gdb_port, "GDB", timeout_secs).await {
        eprintln!("{}", e);
        let delay = orchestrator::Orchestrator::release_delay(&config);
        orchestrator.shutdown(delay).await;
        std::process::exit(1);
    }

    // Reset and resume target
    if let Err(e) = orchestrator.resume_target(&config, &tools).await {
        eprintln!("{}", e);
        let delay = orchestrator::Orchestrator::release_delay(&config);
        orchestrator.shutdown(delay).await;
        std::process::exit(1);
    }

    // Wait for RTT port ready
    if let Err(e) = orchestrator.wait_for_port(&config.host, &config.rtt_port, "RTT", timeout_secs).await {
        eprintln!("{}", e);
        let delay = orchestrator::Orchestrator::release_delay(&config);
        orchestrator.shutdown(delay).await;
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
                let delay = orchestrator::Orchestrator::release_delay(&config);
                orchestrator.shutdown(delay).await;
                drop(orchestrator);
                std::process::exit(1);
            }
        }
        _ = signal_fut => {}
    }

    let delay = orchestrator::Orchestrator::release_delay(&config);
    orchestrator.shutdown(delay).await;
    drop(orchestrator);
    std::process::exit(0);
}


