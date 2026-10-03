use std::fs;
use std::path::PathBuf;
use tokio::process::{Child, Command};
use std::time::Duration;
use std::net::{TcpStream, ToSocketAddrs};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, AsyncBufReadExt};
use tokio::time::timeout_at;
use crate::config::AppConfig;
use crate::preflight::DetectedTools;

pub struct Orchestrator {
    gdb_server_child: Option<Child>,
    pid_file_path: PathBuf,
    ctrl_port_path: PathBuf,
    log_file_path: PathBuf,
}

impl Orchestrator {
    pub fn new(config: &AppConfig) -> Self {
        let temp_dir = crate::config::get_project_temp_dir(&config.project_root);
        let pid_file_path = temp_dir.join("jlink_rtt.pid");
        let ctrl_port_path = temp_dir.join("rtt_ctrl.port");
        Self {
            gdb_server_child: None,
            pid_file_path,
            ctrl_port_path,
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

        loop {
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

            // Try to connect to the port; deadline 判定后置保证至少尝试一次
            if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        self.print_server_log();
        Err(format!(
            "[ERROR] Timed out waiting for {} port {}.\n\
             [INFO] Check the JLinkGDBServer log above.\n\
             [INFO] Or increase the timeout: --ready-timeout 20",
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
}

/// RAII guard ensuring background tasks are aborted and control port file is cleaned up on any exit path.
struct TaskGuard {
    writer: tokio::task::JoinHandle<()>,
    ctrl: tokio::task::JoinHandle<()>,
    stdin: Option<tokio::task::JoinHandle<()>>,
    ctrl_port_path: PathBuf,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.ctrl_port_path);
        self.ctrl.abort();
        if let Some(ref t) = self.stdin {
            t.abort();
        }
        self.writer.abort();
    }
}

/// 匹配模式读取循环的结局: 命中 / 连接关闭 / 统一超时到点
enum MatchOutcome {
    Matched,
    Closed,
    TimedOut,
}

/// 无换行残留缓冲上限: \r 行尾或二进制 RTT 输出永不出现 \n,
/// 超限整体写入文件并清空, 防止缓冲无界增长 (跨边界关键词漏匹配, 与截断同理)
const MATCH_LINE_BUFFER_MAX: usize = 1024 * 1024;

/// 将一段数据同步写入终端回显与 out 文件; 写入永远完整执行, 不被超时取消
async fn emit_bytes(data: &[u8], out_file: &mut Option<tokio::fs::File>) -> Result<(), String> {
    let _ = tokio::io::stdout().write_all(data).await;
    let _ = tokio::io::stdout().flush().await;
    if let Some(f) = out_file {
        f.write_all(data)
            .await
            .map_err(|e| format!("Failed to write to out file: {}", e))?;
    }
    Ok(())
}

impl Orchestrator {
    pub async fn run_rtt_capture(&self, config: &AppConfig) -> Result<(), String> {
        let addr_str = format!("{}:{}", config.host, config.rtt_port);
        let addr = match addr_str.to_socket_addrs() {
            Ok(mut addrs) => match addrs.next() {
                Some(a) => a,
                None => return Err(format!("Failed to resolve address: {}", addr_str)),
            },
            Err(e) => return Err(format!("Invalid address {}: {}", addr_str, e)),
        };

        // Open out file first if specified, failing fast before establishing network connections or background tasks
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

        // 统一交互超时: 匹配模式下为关键词等待上限, 纯抓取模式下为定时自退时长;
        // resolve 阶段已完成数值校验, 缺省(None)表示不限时
        let unified_timeout: Option<Duration> = config.rtt_timeout.map(Duration::from_secs);

        if config.rtt_match_pattern.is_some() {
            eprintln!("[INFO] Connecting to RTT telnet port {}; waiting for match: {}", addr_str, config.rtt_match_pattern.as_ref().unwrap());
        } else {
            eprintln!("[INFO] Connecting to RTT telnet port {}.", addr_str);
            if let Some(dur) = unified_timeout {
                eprintln!("[INFO] Streaming for {}s, then stopping automatically.", dur.as_secs());
            } else {
                eprintln!("[INFO] Streaming until interrupted. To stop, send SIGINT (Ctrl+C or kill -INT <pid>).");
            }
        }

        let tcp_stream = tokio::net::TcpStream::connect(&addr)
            .await
            .map_err(|e| format!("[ERROR] Failed to connect to RTT port {}: {}", addr_str, e))?;

        // Split TCP stream into read and write halves for bidirectional RTT communication
        let (read_half, mut write_half) = tcp_stream.into_split();

        // MPSC channel for downlink commands (from local control socket or stdin)
        let (downlink_tx, mut downlink_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);

        // Dedicated writer task for RTT downlink
        let writer_task = tokio::spawn(async move {
            while let Some(data) = downlink_rx.recv().await {
                if let Err(e) = write_half.write_all(&data).await {
                    eprintln!("[ERROR] Failed to write to RTT downlink: {}", e);
                    break;
                }
                let _ = write_half.flush().await;
            }
        });

        // Bind local control TCP listener on 127.0.0.1:0
        let ctrl_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("[ERROR] Failed to bind local control port: {}", e))?;
        let ctrl_port = ctrl_listener
            .local_addr()
            .map_err(|e| format!("[ERROR] Failed to get local control address: {}", e))?
            .port();
        let _ = fs::write(&self.ctrl_port_path, ctrl_port.to_string());

        let ctrl_tx = downlink_tx.clone();
        let ctrl_task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = ctrl_listener.accept().await {
                let tx = ctrl_tx.clone();
                tokio::spawn(async move {
                    let mut len_buf = [0u8; 4];
                    if socket.read_exact(&mut len_buf).await.is_err() {
                        return;
                    }
                    let len = u32::from_be_bytes(len_buf) as usize;
                    if len > 1024 * 1024 {
                        let _ = socket.write_all(b"ERR payload too large\n").await;
                        return;
                    }
                    let mut payload = vec![0u8; len];
                    if socket.read_exact(&mut payload).await.is_err() {
                        return;
                    }
                    if tx.send(payload).await.is_ok() {
                        let _ = socket.write_all(format!("OK {}\n", len).as_bytes()).await;
                    } else {
                        let _ = socket.write_all(b"ERR downlink closed\n").await;
                    }
                });
            }
        });

        // Stdin interactive task if --interactive (-i) is specified
        let stdin_task = if config.interactive {
            eprintln!("[INFO] Interactive stdin enabled. Type commands and press Enter to send via RTT downlink.");
            let stdin_tx = downlink_tx.clone();
            Some(tokio::spawn(async move {
                let mut reader = BufReader::new(tokio::io::stdin()).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let mut bytes = line.into_bytes();
                    bytes.push(b'\n');
                    if stdin_tx.send(bytes).await.is_err() {
                        break;
                    }
                }
            }))
        } else {
            None
        };

        let _task_guard = TaskGuard {
            writer: writer_task,
            ctrl: ctrl_task,
            stdin: stdin_task,
            ctrl_port_path: self.ctrl_port_path.clone(),
        };

        // 截止时间只作用于 read 等待 (timeout_at 按剩余时间逐次包裹),
        // stdout 与 out_file 写入永远完整执行, 避免取消点落在 write_all 上丢失尾部日志
        let read_deadline = unified_timeout.map(|dur| tokio::time::Instant::now() + dur);
        let capture_started = std::time::Instant::now();

        let has_pattern = config.rtt_match_pattern.is_some();

        let result = if has_pattern {
            let pattern = config.rtt_match_pattern.as_ref().unwrap().clone();
            let pattern_bytes = pattern.as_bytes();
            let mut reader = BufReader::new(read_half);
            let mut buffer: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 1024];

            // 手动缓冲 + 按行切分 (对齐 pylib.rtt_stream 的 flush_residual 设计):
            // read 的取消安全语义保证数据不丢, deadline 只约束单次 read 等待,
            // 残留无换行尾行 (固件打印关键词后静默的最常见形态) 在到点/断连时统一写入文件并复核
            let match_result = async {
                loop {
                    // 消化缓冲内全部完整行 (含 \n), 逐行写入文件并复核 pattern
                    while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                        let line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
                        emit_bytes(&line_bytes, &mut out_file).await?;
                        if line_bytes
                            .windows(pattern_bytes.len())
                            .any(|w| w == pattern_bytes)
                        {
                            // 命中后同批已到达的后续字节 (通常是命中点的上下文日志) 一并写入文件
                            if !buffer.is_empty() {
                                emit_bytes(&buffer, &mut out_file).await?;
                            }
                            return Ok(MatchOutcome::Matched);
                        }
                    }

                    // 缓冲已无完整行, 继续读取; deadline 仅约束 read 等待
                    let read = reader.read(&mut chunk);
                    let bytes = match read_deadline {
                        Some(deadline) => match timeout_at(deadline, read).await {
                            Ok(result) => {
                                result.map_err(|e| format!("Error reading from RTT: {}", e))?
                            }
                            Err(_) => {
                                if !buffer.is_empty() {
                                    emit_bytes(&buffer, &mut out_file).await?;
                                    if buffer
                                        .windows(pattern_bytes.len())
                                        .any(|w| w == pattern_bytes)
                                    {
                                        return Ok(MatchOutcome::Matched);
                                    }
                                }
                                return Ok(MatchOutcome::TimedOut);
                            }
                        },
                        None => read
                            .await
                            .map_err(|e| format!("Error reading from RTT: {}", e))?,
                    };
                    if bytes == 0 {
                        // 连接关闭: 残留无换行尾行同样完整写入文件并复核 pattern
                        if !buffer.is_empty() {
                            emit_bytes(&buffer, &mut out_file).await?;
                            if buffer
                                .windows(pattern_bytes.len())
                                .any(|w| w == pattern_bytes)
                            {
                                return Ok(MatchOutcome::Matched);
                            }
                        }
                        return Ok(MatchOutcome::Closed);
                    }
                    buffer.extend_from_slice(&chunk[..bytes]);
                    if buffer.len() >= MATCH_LINE_BUFFER_MAX {
                        // 无换行连续输出使缓冲到达上限: 写入时保留尾部 pattern 长度-1 字节,
                        // 跨切点关键词留待后续 read 补全后仍可整窗命中
                        let keep = (pattern_bytes.len() - 1).min(buffer.len() - 1);
                        let tail_start = buffer.len() - keep;
                        emit_bytes(&buffer[..tail_start], &mut out_file).await?;
                        if buffer
                            .windows(pattern_bytes.len())
                            .any(|w| w == pattern_bytes)
                        {
                            // 命中后剩余尾字节照常写入文件 (对齐命中后上下文写入约定)
                            if keep > 0 {
                                emit_bytes(&buffer[tail_start..], &mut out_file).await?;
                            }
                            return Ok(MatchOutcome::Matched);
                        }
                        buffer.drain(..tail_start);
                    }
                }
            };

            match match_result.await {
                Ok(MatchOutcome::Matched) => {
                    eprintln!("[INFO] Matched RTT pattern: {}", pattern);
                    Ok(())
                }
                Ok(MatchOutcome::Closed) => {
                    Err("RTT connection closed before pattern was matched.".to_string())
                }
                Ok(MatchOutcome::TimedOut) => {
                    let secs = unified_timeout.map(|d| d.as_secs()).unwrap_or(0);
                    let mut err_msg = format!(
                        "[ERROR] Timed out waiting for RTT pattern after {}s: {}\n",
                        secs, pattern
                    );
                    err_msg.push_str("[INFO] Check the RTT output above for what was captured.\n");
                    err_msg.push_str("[INFO] Or extend the timeout: --timeout 60\n");
                    err_msg.push_str(
                        "[INFO] Or re-run without --match to stream continuously, stop with SIGINT.",
                    );
                    Err(err_msg)
                }
                Err(e) => Err(e),
            }
        } else {
            // Streaming mode: read in blocks and write directly
            let mut reader = read_half;
            let mut buf = [0u8; 1024];
            loop {
                let read = reader.read(&mut buf);
                let bytes = match read_deadline {
                    Some(deadline) => match timeout_at(deadline, read).await {
                        Ok(result) => {
                            result.map_err(|e| format!("Error reading from RTT: {}", e))?
                        }
                        Err(_) => {
                            // 抓满统一超时: 计划内正常完成
                            eprintln!(
                                "[INFO] RTT capture duration elapsed ({}s); stopping.",
                                unified_timeout.map(|d| d.as_secs()).unwrap_or(0)
                            );
                            break Ok(());
                        }
                    },
                    None => read.await.map_err(|e| format!("Error reading from RTT: {}", e))?,
                };
                if bytes == 0 {
                    let elapsed = capture_started.elapsed().as_secs();
                    match unified_timeout {
                        Some(requested) => {
                            // 有超时却提前断连: 抓取被截断属异常, fail-closed 报失败
                            // (单点打印: 错误串伴随 exit 1 由 main 统一输出)
                            break Err(format!(
                                "[ERROR] RTT connection closed after {}s of the requested {}s; capture is incomplete.",
                                elapsed,
                                requested.as_secs()
                            ));
                        }
                        None => {
                            eprintln!("[INFO] RTT connection closed; streaming ended.");
                            break Ok(());
                        }
                    }
                }

                if let Err(e) = emit_bytes(&buf[..bytes], &mut out_file).await {
                    break Err(e);
                }
            }
        };

        if let Some(ref mut f) = out_file {
            let _ = f.sync_data().await;
        }

        result
    }
}

impl Drop for Orchestrator {
    fn drop(&mut self) {
        if let Some(mut child) = self.gdb_server_child.take() {
            eprintln!("[INFO] Stopping JLinkGDBServer.");
            let _ = child.start_kill();
        }
        let _ = fs::remove_file(&self.pid_file_path);
        let _ = fs::remove_file(&self.ctrl_port_path);
    }
}

impl Orchestrator {
    /// Graceful shutdown: terminate GDB server, wait for exit, then buffer
    /// `delay_secs` for OS to reclaim USB handle / TCP ports.
    pub async fn shutdown(&mut self, delay_secs: f64) {
        if let Some(mut child) = self.gdb_server_child.take() {
            eprintln!("[INFO] Stopping JLinkGDBServer.");
            let _ = child.start_kill();
            // Wait up to 5s for child exit; ignore timeout (Drop already sent kill).
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        }
        if delay_secs > 0.0 {
            eprintln!("[INFO] Waiting {:.1}s for USB/port release (RTT_DELAY={}).", delay_secs, delay_secs);
            tokio::time::sleep(Duration::from_secs_f64(delay_secs)).await;
        }
    }

    pub fn release_delay(config: &AppConfig) -> f64 {
        config.rtt_delay.parse::<f64>().unwrap_or(1.5)
    }
}
