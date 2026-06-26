use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchQuality {
    Exact = 0,
    Prefix = 1,
    Contains = 2,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SearchResult {
    pub quality: MatchQuality,
    pub vendor: String,
    pub device: String,
}

fn get_jlink_cmd_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "JLink.exe"
    } else {
        "JLinkExe"
    }
}

fn get_null_device() -> &'static str {
    if cfg!(target_os = "windows") {
        "NUL"
    } else {
        "/dev/null"
    }
}

pub fn get_jlink_version() -> Result<String, String> {
    let cmd_name = get_jlink_cmd_name();
    let null_dev = get_null_device();
    
    // Run JLink to query version
    let output = Command::new(cmd_name)
        .args(&["-NoGui", "1", "-ExitOnError", "1", "-CommandFile", null_dev])
        .output()
        .map_err(|e| format!(
            "[ERROR] Failed to run {}: {}\n[INFO] Is SEGGER J-Link Software installed and in your PATH?\n[INFO] Download from: https://www.segger.com/downloads/jlink/", 
            cmd_name, e
        ))?;

    let stdout_err = String::from_utf8_lossy(&output.stdout);
    
    // Find version pattern like V7.94a or V7.80
    for line in stdout_err.lines() {
        if let Some(pos) = line.find(" V") {
            let start = pos + 1; // Position of 'V'
            let sub = &line[start..];
            if let Some(end) = sub.find(|c: char| c.is_whitespace()) {
                let ver = &sub[..end];
                if ver.starts_with('V') && ver.len() > 1 && ver.chars().nth(1).unwrap().is_ascii_digit() {
                    return Ok(ver.to_string());
                }
            } else if sub.starts_with('V') && sub.len() > 1 && sub.chars().nth(1).unwrap().is_ascii_digit() {
                return Ok(sub.trim().to_string());
            }
        }
    }

    Ok("unknown".to_string())
}

pub fn get_device_cache(project_root: &Path) -> Result<PathBuf, String> {
    let temp_dir = crate::config::get_project_temp_dir(project_root);
    let jlink_ver = get_jlink_version()?;
    let cmd_name = get_jlink_cmd_name();
    
    // Clean JLinkExe version string to be file-system friendly
    let clean_ver = jlink_ver.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>();
        
    let cache_file = temp_dir.join(format!("jlink_devices_{}.csv", clean_ver));
    
    if cache_file.is_file() {
        return Ok(cache_file);
    }

    // Export fresh device list
    let tmp_script = temp_dir.join("jlink_devlist_script.tmp");
    let cache_path_str = if cfg!(target_os = "windows") {
        cache_file.to_string_lossy().replace('\\', "/")
    } else {
        cache_file.to_string_lossy().to_string()
    };
    let cmd_content = format!("ExpDevList {}\nExit\n", cache_path_str);
    fs::write(&tmp_script, cmd_content)
        .map_err(|e| format!("Failed to create temporary J-Link command script: {}", e))?;

    let status = Command::new(cmd_name)
        .args(&["-NoGui", "1", "-ExitOnError", "1", "-CommandFile", &tmp_script.to_string_lossy()])
        .status()
        .map_err(|e| format!("Failed to execute {} to export device list: {}", cmd_name, e))?;

    let _ = fs::remove_file(&tmp_script);

    if !status.success() || !cache_file.is_file() {
        return Err(format!("[ERROR] Failed to export J-Link device list database using {}.", cmd_name));
    }

    Ok(cache_file)
}

pub fn search_devices(pattern: &str, project_root: &Path) -> Result<Vec<SearchResult>, String> {
    let cache_file = get_device_cache(project_root)?;
    let content = fs::read_to_string(&cache_file)
        .map_err(|e| format!("Failed to read device cache file: {}", e))?;

    let clean = |s: &str| -> String {
        s.trim().trim_matches('"').trim_matches('\'').trim().to_string()
    };

    let pat_lower = pattern.to_lowercase();
    let mut results = Vec::new();

    for (idx, line) in content.lines().enumerate() {
        if idx == 0 || line.trim().is_empty() {
            // Skip header and empty lines
            continue;
        }

        let parts: Vec<&str> = line.split("\", \"").collect();
        if parts.len() < 2 {
            continue;
        }

        let vendor = clean(parts[0]);
        let device = clean(parts[1]);
        let dev_lower = device.to_lowercase();

        if dev_lower == pat_lower {
            results.push(SearchResult {
                quality: MatchQuality::Exact,
                vendor,
                device,
            });
        } else if dev_lower.starts_with(&pat_lower) {
            results.push(SearchResult {
                quality: MatchQuality::Prefix,
                vendor,
                device,
            });
        } else if dev_lower.contains(&pat_lower) {
            results.push(SearchResult {
                quality: MatchQuality::Contains,
                vendor,
                device,
            });
        }
    }

    results.sort_by(|a, b| {
        match a.quality.cmp(&b.quality) {
            std::cmp::Ordering::Equal => a.device.to_lowercase().cmp(&b.device.to_lowercase()),
            other => other,
        }
    });

    Ok(results)
}

pub fn resolve_device_name(pattern: &str, project_root: &Path) -> Result<String, String> {
    let matches = search_devices(pattern, project_root)?;
    
    if matches.is_empty() {
        return Err(format!(
            "[ERROR] No J-Link device matches '{}'.\n[INFO] Try a broader pattern, e.g. 'nrf52' instead of 'nrf52840'.\n[INFO] Or confirm the exact name: cargo run -- --search-device <pattern>",
            pattern
        ));
    }

    let exact_matches: Vec<&SearchResult> = matches.iter().filter(|m| m.quality == MatchQuality::Exact).collect();
    
    if exact_matches.len() == 1 {
        let resolved = exact_matches[0].device.clone();
        eprintln!("[INFO] Resolved device: {} ({})", resolved, exact_matches[0].vendor);
        return Ok(resolved);
    }

    if matches.len() == 1 {
        let resolved = matches[0].device.clone();
        eprintln!("[INFO] Resolved device: {} ({})", resolved, matches[0].vendor);
        return Ok(resolved);
    }

    let mut hint = format!("[ERROR] Multiple J-Link devices match '{}' ({} found).\n", pattern, matches.len());
    hint.push_str("[INFO] Pick the correct device from the list below and re-run with --device <EXACT_NAME>:\n");
    for m in &matches {
        hint.push_str(&format!("[INFO]   {} | {}\n", m.vendor, m.device));
    }
    hint.push_str("\n[INFO] Example: cargo run -- --init --device <EXACT_NAME>");
    Err(hint)
}
