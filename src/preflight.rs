use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Command;
use std::net::TcpStream;
use std::time::Duration;
use std::net::ToSocketAddrs;

#[allow(dead_code)]
pub struct DetectedTools {
    pub jlink_gdb_server: PathBuf,
    pub nc: PathBuf,
}

pub fn find_executable(cmd: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let mut candidates = vec![cmd.to_string()];
    if cfg!(target_os = "windows") {
        candidates.insert(0, format!("{}.exe", cmd));
    }
    
    for path in std::env::split_paths(&paths) {
        for cand in &candidates {
            let candidate = path.join(cand);
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if candidate.is_file() {
                    if let Ok(meta) = candidate.metadata() {
                        // Check if executable by owner, group, or others
                        if meta.mode() & 0o111 != 0 {
                            return Some(candidate);
                        }
                    }
                }
            }
            #[cfg(not(unix))]
            {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

pub fn detect_tools(
    override_jlink: &Option<String>,
    override_nc: &Option<String>,
) -> Result<DetectedTools, String> {
    // 1. Detect JLinkGDBServer
    let jlink_gdb_server = if let Some(path_str) = override_jlink {
        let path = PathBuf::from(path_str);
        if !path.is_file() {
            return Err(format!("[ERROR] Missing required command: {}.\n[INFO] Re-run the command with a valid path.", path_str));
        }
        path
    } else {
        find_executable("JLinkGDBServer")
            .or_else(|| find_executable("JLinkGDBServerCL"))
            .or_else(|| find_executable("JLinkGDBServerCLExe"))
            .ok_or_else(|| {
                "[ERROR] Missing required command. Tried JLinkGDBServer, JLinkGDBServerCLExe.\n\
                 [INFO] Install SEGGER J-Link tools and add to PATH.\n\
                 [INFO] Or override individually: --jlink-gdb-server <path>".to_string()
            })?
    };

    // 2. Detect nc (Optional, keep for backward args compatibility)
    let nc = if let Some(path_str) = override_nc {
        let path = PathBuf::from(path_str);
        if !path.is_file() {
            return Err(format!("[ERROR] Missing required command: {}.\n[INFO] Re-run the command with a valid path.", path_str));
        }
        path
    } else {
        find_executable("nc")
            .or_else(|| find_executable("ncat"))
            .unwrap_or_else(|| PathBuf::from("nc"))
    };

    Ok(DetectedTools {
        jlink_gdb_server,
        nc,
    })
}

pub fn ensure_port_free(host: &str, port: &str, name: &str) -> Result<(), String> {
    let addr_str = format!("{}:{}", host, port);
    let addrs = match addr_str.to_socket_addrs() {
        Ok(a) => a,
        Err(e) => return Err(format!("[ERROR] Invalid address {}: {}", addr_str, e)),
    };

    for addr in addrs {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok() {
            return Err(format!(
                "[ERROR] {} port {} is already in use.\n\
                 [INFO] Find and stop the stale process: lsof -i :{} || ss -tlnp | grep :{}\n\
                 [INFO] Or use a different port: --{}-port <PORT>",
                name, addr_str, port, port, name.to_lowercase()
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn detect_jlink_serials() -> Vec<String> {
    let output = Command::new("lsusb")
        .args(&["-v", "-d", "1366:"])
        .output();
        
    let mut serials = Vec::new();
    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if line.to_lowercase().contains("iserial") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    let s = parts[2].trim().to_string();
                    if !s.is_empty() && s != "0" {
                        serials.push(s);
                    }
                }
            }
        }
    }
    serials.sort();
    serials.dedup();
    serials
}

#[cfg(not(target_os = "linux"))]
pub fn detect_jlink_serials() -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "linux")]
pub fn check_usb_device() {
    let output = Command::new("lsusb").output();
    let has_jlink = if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
        text.contains("segger") || text.contains("j-link") || text.contains("1366:")
    } else {
        false
    };

    if !has_jlink {
        eprintln!("[WARN] No SEGGER/J-Link USB device detected.");
        eprintln!("[INFO] Ask the user to check: USB connection, permissions, or USB passthrough.");
        eprintln!("[INFO] If using a remote J-Link, this warning can be ignored.");
    }
}

#[cfg(not(target_os = "linux"))]
pub fn check_usb_device() {
}

pub fn perform_preflight(config: &crate::config::AppConfig) -> Result<DetectedTools, String> {
    // 1. Detect toolchains
    let tools = detect_tools(&config.jlink_gdb_server, &config.nc)?;

    // 2. Check USB connection warning
    check_usb_device();

    // 3. Ensure ports are free
    ensure_port_free(&config.host, &config.gdb_port, "GDB")?;
    ensure_port_free(&config.host, &config.rtt_port, "RTT")?;

    Ok(tools)
}
