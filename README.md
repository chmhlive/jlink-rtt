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

> **BREAKING CHANGE**: `--timeout` now carries the **overall RTT interaction timeout**
> (keyword match wait limit in match mode, timed capture duration otherwise; `0` or
> unset = continuous stream). The port ready timeout moved to `--ready-timeout`
> (config key `RTT_READY_TIMEOUT`, unchanged). `--match-timeout` and the
> `RTT_MATCH_TIMEOUT` config key have been **removed** — migrate to `--timeout` /
> `RTT_TIMEOUT`. Invalid (non-numeric or negative) timeout values now fail loudly
> instead of falling back to an unbounded capture, and `--ready-timeout 0` is now
> rejected (note the asymmetry: `--timeout 0` stays valid and means an unlimited
> stream). A timed capture that ends early because the RTT connection closed
> before the window elapsed also exits **non-zero** (fail-closed): the log may
> be incomplete.

### 1. Initialize Project Config

From your target project directory:
```bash
jlink-rtt --init --device nrf52840
```
This generates or appends RTT debug settings to `.prj.env`. Adjust the options inside as needed.

### 2. Capture Logs

```bash
# Stream output and exit when "START HERE" is captured (waits up to 30s)
jlink-rtt --match "START HERE" --timeout 30

# Capture logs for 60 seconds, then stop automatically
jlink-rtt --out rtt.log --timeout 60

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

## Running Tests

An automated self-coverage simulator test suite is included:
```bash
./jlink_rtt_no_hardware_test.sh
```

## License

MIT
