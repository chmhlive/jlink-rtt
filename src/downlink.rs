/// Downlink payload parsing and escape handling for RTT commands.

/// Parse hex string like "0102030A", "01 02 03 0a", "0x01 0x02", or "0x01, 0x02" into bytes.
pub fn parse_hex_string(s: &str) -> Result<Vec<u8>, String> {
    let tokens: Vec<&str> = s
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .collect();

    if tokens.is_empty() {
        return Err("[ERROR] Hex command string is empty.".to_string());
    }

    let mut hex_digits = String::new();
    for token in tokens {
        if token == "0x" || token == "0X" {
            continue;
        }
        let t = if token.starts_with("0x") || token.starts_with("0X") {
            &token[2..]
        } else {
            token
        };
        if t.is_empty() {
            return Err(format!("[ERROR] Empty hex token in '{}'.", s));
        }
        hex_digits.push_str(t);
    }

    if hex_digits.len() % 2 != 0 {
        return Err(format!(
            "[ERROR] Hex string has odd length ({} digits): '{}'. Must be even number of hex digits.",
            hex_digits.len(),
            hex_digits
        ));
    }
    let mut bytes = Vec::with_capacity(hex_digits.len() / 2);
    for i in (0..hex_digits.len()).step_by(2) {
        let chunk = &hex_digits[i..i + 2];
        let byte = u8::from_str_radix(chunk, 16).map_err(|_| {
            format!("[ERROR] Invalid hex characters '{}' in string '{}'.", chunk, s)
        })?;
        bytes.push(byte);
    }
    Ok(bytes)
}

/// Unescape string: supports \n \r \t \\ \xHH. Errors on invalid \x or trailing \.
pub fn unescape_string(s: &str) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            return Err(
                "[ERROR] Invalid escape at end of string: trailing '\\'.\n[INFO] Valid escapes: \\n \\r \\t \\\\ \\xHH".to_string(),
            );
        }
        match bytes[i + 1] {
            b'n' => {
                out.push(b'\n');
                i += 2;
            }
            b'r' => {
                out.push(b'\r');
                i += 2;
            }
            b't' => {
                out.push(b'\t');
                i += 2;
            }
            b'\\' => {
                out.push(b'\\');
                i += 2;
            }
            b'x' => {
                if i + 3 >= bytes.len() {
                    return Err(
                        "[ERROR] Invalid \\x escape: need 2 hex digits (e.g. \\x0D\\x0A).".to_string(),
                    );
                }
                let hex = &s[i + 2..i + 4];
                let byte = u8::from_str_radix(hex, 16).map_err(|_| {
                    format!("[ERROR] Invalid \\x escape: '{}' is not valid hex.", hex)
                })?;
                out.push(byte);
                i += 4;
            }
            other => {
                return Err(format!(
                    "[ERROR] Unknown escape '\\{}'.\n[INFO] Valid escapes: \\n \\r \\t \\\\ \\xHH",
                    other as char
                ));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unescape_plain() {
        assert_eq!(unescape_string("hello world").unwrap(), b"hello world");
    }

    #[test]
    fn test_unescape_special_chars() {
        assert_eq!(unescape_string("a\\nb\\rc\\td\\\\").unwrap(), b"a\nb\rc\td\\");
    }

    #[test]
    fn test_unescape_hex() {
        assert_eq!(unescape_string("cmd\\x0D\\x0A").unwrap(), b"cmd\r\n");
    }

    #[test]
    fn test_unescape_trailing_slash_fails() {
        assert!(unescape_string("bad\\").is_err());
    }

    #[test]
    fn test_unescape_invalid_hex_fails() {
        assert!(unescape_string("\\xZZ").is_err());
    }

    #[test]
    fn test_parse_hex_simple() {
        assert_eq!(parse_hex_string("0102030a").unwrap(), vec![1, 2, 3, 10]);
    }

    #[test]
    fn test_parse_hex_with_spaces_and_prefix() {
        assert_eq!(parse_hex_string("0x 01 02 0A FF").unwrap(), vec![1, 2, 10, 255]);
    }

    #[test]
    fn test_parse_hex_multiple_0x_prefixes() {
        assert_eq!(parse_hex_string("0x01 0x02 0x03 0x0a").unwrap(), vec![1, 2, 3, 10]);
    }

    #[test]
    fn test_parse_hex_comma_separated() {
        assert_eq!(parse_hex_string("0x01, 0x02, 0x0A").unwrap(), vec![1, 2, 10]);
    }

    #[test]
    fn test_parse_hex_odd_fails() {
        assert!(parse_hex_string("123").is_err());
    }

    #[test]
    fn test_parse_hex_invalid_chars_fails() {
        assert!(parse_hex_string("01GG").is_err());
    }
}
