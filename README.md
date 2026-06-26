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

### 1. Initialize Project Config

From your target project directory:
```bash
jlink-rtt --init --device nrf52840
```
This generates a local `.jlink-rtt.env` file. Adjust the options inside as needed.

### 2. Capture Logs

```bash
# Stream output and exit when "START HERE" is captured
jlink-rtt --match "START HERE" --match-timeout 30

# Stream output indefinitely
jlink-rtt --out rtt.log

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
