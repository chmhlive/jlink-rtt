use std::fs;
use std::path::PathBuf;
use tokio::process::{Child, Command};
use std::time::Duration;
use std::net::{TcpStream, ToSocketAddrs};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, AsyncBufReadExt};
use tokio::time::timeout;
use crate::config::AppConfig;
use crate::preflight::DetectedTools;

pub struct Orchestrator {
    gdb_server_child: Option<Child>,
    pid_file_path: PathBuf,
    log_file_path: PathBuf,
}

impl Orchestrator {
    pub fn new(config: &AppConfig) -> Self {
        let temp_dir = crate::config::get_project_temp_dir(&config.project_root);
        let pid_file_path = temp_dir.join("jlink_rtt.pid");
        Self {
            gdb_server_child: None,
            pid_file_path,
            log_file_path: PathBuf::from(&config.log_file),
        }
    }

    pub fn write_pid(&self) -> Result<(), String> {
        let pid = std::process::id();
        fs::write(&self.pid_file_path, pid.to_string())
            .map_err(|e| format!("[ERROR] Failed to write PID file {}: {}", self.pid_file_path.display(), e))?;
        Ok(())
    }

    pub async fn start_gdb_server(&mut self, config: &AppConfig, tools: &DetectedTools) -> Result<(), String> {
        // Prepare arguments for JLinkGDBServer
        let mut args = vec![
            "-device".to_string(),
            config.device.as_ref().cloned().unwrap_or_default(),
            "-if".to_string(),
            config.jlink_if.clone(),
            "-speed".to_string(),
            config.speed.clone(),
            "-port".to_string(),
            config.gdb_port.clone(),
            "-RTTTelnetPort".to_string(),
            config.rtt_port.clone(),
        ];

        if let Some(ref serial) = config.jlink_serial {
            args.push("-select".to_string());
            args.push(format!("USB={}", serial));
        }

        // Open log file for JLinkGDBServer
        let log_file = fs::File::create(&config.log_file)
            .map_err(|e| format!("[ERROR] Failed to create GDB server log file {}: {}", config.log_file, e))?;

        eprintln!("[INFO] Starting JLinkGDBServer for {} on GDB {}:{}, RTT {}:{}.",
            config.device.as_ref().unwrap_or(&"unknown".to_string()),
            config.host, config.gdb_port, config.host, config.rtt_port
        );

        let child = Command::new(&tools.jlink_gdb_server)
            .args(&args)
            .stdout(log_file.try_clone().unwrap())
            .stderr(log_file)
            .spawn()
            .map_err(|e| format!("[ERROR] Failed to spawn JLinkGDBServer: {}", e))?;

        self.gdb_server_child = Some(child);
        Ok(())
    }

    pub async fn wait_for_port(&mut self, host: &str, port: &str, name: &str, timeout_secs: u32) -> Result<(), String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs as u64);
        let addr_str = format!("{}:{}", host, port);
        
        let addr = match addr_str.to_socket_addrs() {
            Ok(mut addrs) => match addrs.next() {
                Some(a) => a,
                None => return Err(format!("Failed to resolve address: {}", addr_str)),
            },
            Err(e) => return Err(format!("Invalid address {}: {}", addr_str, e)),
        };

        while std::time::Instant::now() < deadline {
            // Check if the server child exited unexpectedly
            if let Some(ref mut child) = self.gdb_server_child {
                if let Ok(Some(status)) = child.try_wait() {
                    self.print_server_log();
                    return Err(format!(
                        "[ERROR] JLinkGDBServer stopped before {} port became ready. Exit status: {}",
                        name, status
                    ));
                }
            }

            // Try to connect to the port
            if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
                return Ok(());
            }

            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        self.print_server_log();
        Err(format!(
            "[ERROR] Timed out waiting for {} port {}.\n\
             [INFO] Check the JLinkGDBServer log above.\n\
             [INFO] Or increase the timeout: --timeout 20",
            name, addr_str
        ))
    }

    fn print_server_log(&self) {
        if let Ok(content) = fs::read_to_string(&self.log_file_path) {
            if !content.is_empty() {
                eprintln!("[ERROR] JLinkGDBServer log:");
                let lines: Vec<&str> = content.lines().collect();
                let start = if lines.len() > 160 { lines.len() - 160 } else { 0 };
                for line in &lines[start..] {
                    eprintln!("{}", line);
                }
            }
        }
    }

    pub async fn resume_target(&self, config: &AppConfig, _tools: &DetectedTools) -> Result<(), String> {
        if config.resume_target == "0" {
            eprintln!("[INFO] Skipping target resume.");
            return Ok(());
        }

        let temp_dir = crate::config::get_project_temp_dir(&config.project_root);
        let reset_script_path = temp_dir.join("jlink_reset.jlink");

        // Write J-Link Commander commands to file (r = reset, g = go, q = quit)
        let script_content = if config.reset_target == "1" {
            "r\ng\nq\n"
        } else {
            "g\nq\n"
        };

        fs::write(&reset_script_path, script_content)
            .map_err(|e| format!("[ERROR] Failed to create temporary J-Link reset script: {}", e))?;

        let cmd_name = if cfg!(target_os = "windows") {
            "JLink.exe"
        } else {
            "JLinkExe"
        };

        let mut args = vec![
            "-device".to_string(),
            config.device.as_ref().cloned().unwrap_or_default(),
            "-if".to_string(),
            config.jlink_if.clone(),
            "-speed".to_string(),
            config.speed.clone(),
            "-NoGui".to_string(),
            "1".to_string(),
            "-ExitOnError".to_string(),
            "1".to_string(),
            "-CommanderScript".to_string(),
            // Replace backslashes with forward slashes for Windows JLink tool script
            if cfg!(target_os = "windows") {
                reset_script_path.to_string_lossy().replace('\\', "/")
            } else {
                reset_script_path.to_string_lossy().to_string()
            },
        ];

        if let Some(ref serial) = config.jlink_serial {
            args.push("-SelectEmuBySN".to_string());
            args.push(serial.clone());
        }

        eprintln!("[INFO] Resetting and resuming target through J-Link Commander.");

        let resume_log_file = fs::File::create(&config.gdb_log_file)
            .map_err(|e| format!("[ERROR] Failed to create J-Link Commander resume log {}: {}", config.gdb_log_file, e))?;

        let status = Command::new(cmd_name)
            .args(&args)
            .stdout(resume_log_file.try_clone().unwrap())
            .stderr(resume_log_file)
            .status()
            .await
            .map_err(|e| format!("[ERROR] Failed to execute J-Link Commander: {}", e))?;

        let _ = fs::remove_file(&reset_script_path);

        if !status.success() {
            if let Ok(content) = fs::read_to_string(&config.gdb_log_file) {
                eprintln!("[ERROR] J-Link Commander resume log:");
                let lines: Vec<&str> = content.lines().collect();
                let start = if lines.len() > 160 { lines.len() - 160 } else { 0 };
                for line in &lines[start..] {
                    eprintln!("{}", line);
                }
            }
            return Err("[ERROR] Failed to reset/resume target through J-Link Commander.\n[INFO] Check the resume log above.".to_string());
        }

        Ok(())
    }

    pub async fn run_rtt_capture(&self, config: &AppConfig) -> Result<(), String> {
        let addr_str = format!("{}:{}", config.host, config.rtt_port);
        let addr = match addr_str.to_socket_addrs() {
            Ok(mut addrs) => match addrs.next() {
                Some(a) => a,
                None => return Err(format!("Failed to resolve address: {}", addr_str)),
            },
            Err(e) => return Err(format!("Invalid address {}: {}", addr_str, e)),
        };

        if config.rtt_match_pattern.is_some() {
            eprintln!("[INFO] Connecting to RTT telnet port {}; waiting for match: {}", addr_str, config.rtt_match_pattern.as_ref().unwrap());
        } else {
            eprintln!("[INFO] Connecting to RTT telnet port {}.", addr_str);
            eprintln!("[INFO] Streaming until interrupted. To stop, send SIGINT (Ctrl+C or kill -INT <pid>).");
        }

        let mut tcp_stream = tokio::net::TcpStream::connect(&addr)
            .await
            .map_err(|e| format!("[ERROR] Failed to connect to RTT port {}: {}", addr_str, e))?;

        // Open out file if set
        let mut out_file = if let Some(ref path_str) = config.rtt_out_file {
            let f = tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(true)
                .open(path_str)
                .await
                .map_err(|e| format!("[ERROR] Failed to open RTT output file {}: {}", path_str, e))?;
            Some(f)
        } else {
            None
        };

        let has_pattern = config.rtt_match_pattern.is_some();
        let match_timeout_secs = config.rtt_match_timeout.parse::<u64>().unwrap_or(30);

        if has_pattern {
            let pattern = config.rtt_match_pattern.as_ref().unwrap().clone();
            let mut reader = BufReader::new(tcp_stream);
            let mut line = String::new();

            let read_future = async {
                loop {
                    line.clear();
                    let bytes = reader.read_line(&mut line).await
                        .map_err(|e| format!("Error reading from RTT: {}", e))?;
                    if bytes == 0 {
                        return Ok(false); // Connection closed without match
                    }

                    // Write to stdout
                    print!("{}", line);
                    let _ = tokio::io::stdout().flush().await;

                    // Write to out file
                    if let Some(ref mut f) = out_file {
                        if let Err(e) = f.write_all(line.as_bytes()).await {
                            return Err(format!("Failed to write to out file: {}", e));
                        }
                    }

                    if line.contains(&pattern) {
                        return Ok(true); // Matched!
                    }
                }
            };

            match timeout(Duration::from_secs(match_timeout_secs), read_future).await {
                Ok(Ok(true)) => {
                    eprintln!("[INFO] Matched RTT pattern: {}", pattern);
                    Ok(())
                }
                Ok(Ok(false)) => {
                    Err("RTT connection closed before pattern was matched.".to_string())
                }
                Ok(Err(e)) => Err(e),
                Err(_) => {
                    let mut err_msg = format!(
                        "[ERROR] Timed out waiting for RTT pattern after {}s: {}\n",
                        match_timeout_secs, pattern
                    );
                    err_msg.push_str("[INFO] Check the RTT output above for what was captured.\n");
                    err_msg.push_str("[INFO] Or extend the timeout: --match-timeout 60\n");
                    err_msg.push_str("[INFO] Or re-run without --match and without timeout to stream continuously, stop with SIGINT.");
                    Err(err_msg)
                }
            }
        } else {
            // Streaming mode: read in blocks and write directly
            let mut buf = [0u8; 1024];
            loop {
                let bytes = tcp_stream.read(&mut buf).await
                    .map_err(|e| format!("Error reading from RTT: {}", e))?;
                if bytes == 0 {
                    break;
                }

                let chunk = &buf[..bytes];
                let _ = tokio::io::stdout().write_all(chunk).await;
                let _ = tokio::io::stdout().flush().await;

                if let Some(ref mut f) = out_file {
                    if let Err(e) = f.write_all(chunk).await {
                        return Err(format!("Failed to write to out file: {}", e));
                    }
                }
            }
            Ok(())
        }
    }
}

impl Drop for Orchestrator {
    fn drop(&mut self) {
        if let Some(mut child) = self.gdb_server_child.take() {
            eprintln!("[INFO] Stopping JLinkGDBServer.");
            let _ = child.start_kill();
        }
        let _ = fs::remove_file(&self.pid_file_path);
    }
}
