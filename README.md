# jlink-rtt

A high-performance, cross-platform (Windows & Linux/WSL) Rust command-line tool to orchestrate SEGGER J-Link GDB Server and J-Link Commander for chip reset and RTT log capture.

## Features

- **No GDB dependency**: Uses J-Link Commander (`JLink.exe` / `JLinkExe`) scripts directly for target reset and resume. No GDB debuggers needed.
- **Cross-platform**: Works natively on both Windows and Linux (including WSL2 with USB passthrough).
- **Auto-stop on match**: Can stream output to stdout and file, and exit automatically once a target text pattern is matched (with custom timeouts).
- **Fuzzy device resolution**: Autocompletes target device names (e.g., `nrf52840` to `nRF52840_xxAA`) by scanning J-Link's internal device database.
- **Port safety**: Verifies socket availability and warns about collisions. Handles orphan process termination gracefully.

## Installation

Ensure you have [SEGGER J-Link Software](https://www.segger.com/downloads/jlink/) installed and in your `PATH`.

```bash
cargo build --release
```

The compiled binary will be available at `target/release/jlink-rtt`.

## Usage

> **Timeout model**: `--rtt-timeout` carries the **overall RTT interaction timeout**
> (keyword match wait limit in match mode, timed capture duration otherwise; `0` or
> unset = continuous stream, except match mode defaults to 30s).
> The port ready timeout is `--ready-timeout` (config key `RTT_READY_TIMEOUT`,
> default 10). Invalid (non-numeric) timeout values fail loudly instead of
> falling back to an unbounded capture, and `--ready-timeout 0` is rejected
> (note the asymmetry: `--rtt-timeout 0` stays valid and means an unlimited
> stream). A timed capture that ends early because the RTT connection closed
> before the window elapsed also exits **non-zero** (fail-closed): the log may
> be incomplete. Only canonical keys are read: `RTT_MATCH`, `RTT_TIMEOUT`,
> `RTT_DELAY`. CLI exposes no aliases: `--rtt-match`, `--rtt-timeout`,
> `--rtt-delay`, `--ready-timeout` (plus `-i` for `--interactive`).

### 1. Initialize Project Config

From your target project directory:
```bash
jlink-rtt --init --device nrf52840
```
This generates or appends RTT debug settings to `.prj.env`. Adjust the options inside as needed.

### 2. Capture Logs

```bash
# Stream output and exit when "START HERE" is captured (waits up to 30s)
jlink-rtt --rtt-match "START HERE" --rtt-timeout 30 --rtt-delay 1.5

# Capture logs for 60 seconds, then stop automatically
jlink-rtt --out rtt.log --rtt-timeout 60

# Stream output indefinitely
jlink-rtt --out rtt.log

# Send command to running RTT session via downlink (from another terminal or test script)
jlink-rtt --send "help"                          # auto-appends \n, supports \n \r \t \\ \xHH
jlink-rtt --send "status" --no-newline          # send without trailing \n
jlink-rtt --send "0102030a" --hex               # hex bytes (supports 0102030a, 0x01 0x02, etc.)

# Interactive mode: forward terminal input lines directly to RTT downlink
jlink-rtt --out rtt.log -i

# Stop a running session
jlink-rtt --stop
```

For more options:
```bash
jlink-rtt --help
```

## Config Resolution

Precedence: `CLI args > .prj.local.env (local private, git-ignored) > .prj.env (shared) > built-in defaults`.

- Put per-machine `JLINK_SERIAL` in `.prj.local.env` to isolate multi-probe debugging without touching shared `.prj.env`:
  ```bash
  # .prj.local.env (never commit)
  JLINK_SERIAL=20161223
  ```
- Verify overlay: `jlink-rtt --print-config` shows `LOCAL_CONFIG_FILE=` plus resolved `JLINK_SERIAL`.
- `--config <FILE>` uses single-file mode (no local overlay).

## Param Map (canonical keys, no legacy fallback)

| Purpose | Key / CLI | Default |
|---|---|---|
| Match pattern | `RTT_MATCH` / `--rtt-match` | (none, stream) |
| Overall timeout | `RTT_TIMEOUT` / `--rtt-timeout` (match wait cap, or stream duration) | `30` (match) / continuous (stream) |
| Release delay | `RTT_DELAY` / `--rtt-delay` | `1.5` |
| Port ready timeout | `RTT_READY_TIMEOUT` / `--ready-timeout` | `10` |
| Serial | `JLINK_SERIAL` / `--serial` | auto-detect |
| GDB/RTT ports | `GDB_PORT`/`RTT_PORT`, `LISTEN_HOST` | `2331`/`19021`/`127.0.0.1` |

After each capture the tool waits `RTT_DELAY` seconds for USB/port release, so back-to-back `jlink-rtt` or `flash` no longer hits `Port 19021 already in use`.

## Running Tests

An automated self-coverage simulator test suite is included:
```bash
./jlink_rtt_no_hardware_test.sh
```

## License

MIT
