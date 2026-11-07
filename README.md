# jlink-rtt

A high-performance, cross-platform (Windows & Linux/WSL) Rust command-line tool to orchestrate SEGGER J-Link GDB Server and J-Link Commander for chip reset and RTT log capture.

## Features

- **No GDB dependency**: Uses J-Link Commander (`JLink.exe` / `JLinkExe`) scripts directly for target reset and resume. No GDB debuggers needed.
- **Cross-platform**: Works natively on both Windows and Linux (including WSL2 with USB passthrough).
- **Auto-stop on match**: Can stream output to stdout and file, and exit automatically once a target text pattern is matched (with custom timeouts).
- **Device database search**: `--search-device <pattern>` queries J-Link's internal device database (no hardware needed) so you can find the exact `--device` name.
- **Port safety**: Verifies socket availability and warns about collisions. Handles orphan process termination gracefully.

## Installation

Ensure you have [SEGGER J-Link Software](https://www.segger.com/downloads/jlink/) installed and in your `PATH`.

```bash
cargo build --release
```

The compiled binary will be available at `target/release/jlink-rtt`.

## Usage

> **Timeout model**: `--log-timeout` carries the **overall RTT interaction timeout**
> (keyword match wait limit in match mode, timed capture duration otherwise; `0` or
> unset = continuous stream, except match mode defaults to 30s).
> The port ready timeout is `--ready-timeout` (default 10). Invalid (non-numeric)
> timeout values fail loudly instead of falling back to an unbounded capture, and
> `--ready-timeout 0` is rejected (note the asymmetry: `--log-timeout 0` stays
> valid and means an unlimited stream). A timed capture that ends early because
> the RTT connection closed before the window elapsed also exits **non-zero**
> (fail-closed): the log may be incomplete. CLI exposes no aliases:
> `--log-match`, `--log-timeout`, `--rtt-delay`, `--ready-timeout` (plus `-i` for
> `--interactive`).

### Capture Logs

The tool reads no config files: every parameter comes from CLI flags. `--device`
takes an exact J-Link device name; use `--search-device <pattern>` to find it.
The examples below assume `nRF52840_xxAA` — replace it with your target.

```bash
# Stream output and exit when "START HERE" is captured (waits up to 30s)
jlink-rtt --device nRF52840_xxAA --log-match "START HERE" --log-timeout 30 --rtt-delay 1.5

# Capture logs for 60 seconds, then stop automatically
jlink-rtt --device nRF52840_xxAA --log-out rtt.log --log-timeout 60

# Stream output indefinitely
jlink-rtt --device nRF52840_xxAA --log-out rtt.log

# Send command to running RTT session via downlink (from another terminal or test script)
jlink-rtt --send "help"                          # auto-appends \n, supports \n \r \t \\ \xHH
jlink-rtt --send "status" --no-newline          # send without trailing \n
jlink-rtt --send "0102030a" --hex               # hex bytes (supports 0102030a, 0x01 0x02, etc.)

# Interactive mode: forward terminal input lines directly to RTT downlink
jlink-rtt --device nRF52840_xxAA --log-out rtt.log -i

# Stop a running session
jlink-rtt --stop
```

For more options:
```bash
jlink-rtt --help
jlink-rtt --example     # print usage examples
```

## Parameter Resolution

All parameters come from CLI flags plus built-in defaults. The tool does not
read, generate, or merge any config file (no `.prj.env`, no `.prj.local.env`).

- Show project root and log paths: `jlink-rtt --print-config`.
- Multi-probe setups: pass `--serial <SN>` explicitly on every invocation.

## CLI Param Map

| Purpose | CLI | Default |
|---|---|---|
| Target device | `--device` | (required) |
| J-Link interface | `--if` | `SWD` |
| J-Link speed (kHz) | `--speed` | `4000` |
| J-Link serial | `--serial` | auto-detect |
| Listen host | `--host` | `127.0.0.1` |
| GDB server port | `--gdb-port` | `2331` |
| RTT telnet port | `--rtt-port` | `19021` |
| Match pattern | `--log-match` | (none, stream) |
| Overall timeout | `--log-timeout` (match wait cap, or stream duration) | `30` (match) / continuous (stream) |
| Release delay | `--rtt-delay` | `1.5` |
| Port ready timeout | `--ready-timeout` | `10` |
| GDB server log | `--log` | `<tmp_dir>/jlink_gdb_server.log` |
| Reset/resume log | `--gdb-log` | `<tmp_dir>/jlink_gdb_resume.log` |

After each capture the tool waits `--rtt-delay` seconds for USB/port release, so back-to-back `jlink-rtt` or `flash` no longer hits `Port 19021 already in use`.

## Running Tests

An automated self-coverage simulator test suite is included:
```bash
./jlink_rtt_no_hardware_test.sh
```

## License

MIT
