---
name: jlink-rtt
description: Read SEGGER J-Link RTT logs with CLI configuration, reset/attach modes, pattern matching, and troubleshooting.
---

# J-Link RTT

## Quick Path

**Run the tool first — The tool does all and tells you exactly what to do next**

Use the compiled `jlink-rtt` binary (or `jlink-rtt.exe` on Windows). Do not rewrite JLinkGDBServer/JLinkExe/TCP socket orchestration.

- **AI Tip**: When observing `RTT_LOG`, prefer using your own read/grep/search tools to inspect, filter or browse the log. Do not simply `cat` the entire file to avoid token overflow.

Always run from the target project root. The tool reads no config files: every parameter comes from CLI flags. Set `DEVICE` to the exact J-Link device name (use `--search-device` to find it). Define OS-dependent variables and choose exactly one capture mode below:

```bash
# Linux/WSL (Bash)
JLINK_RTT_BIN="<loaded-skill-base>/scripts/jlink-rtt"
DEVICE="nRF52840_xxAA"   # replace with your exact J-Link device name
PROJECT_TEMP_DIR="$(dirname "$("${JLINK_RTT_BIN}" --print-config | grep "^JLINK_LOG_FILE=" | cut -d'=' -f2)")"
RTT_LOG="${PROJECT_TEMP_DIR}/rtt.log"

# Windows (PowerShell)
JLINK_RTT_BIN="<loaded-skill-base>\scripts\jlink-rtt.exe"
$DEVICE = "nRF52840_xxAA"   # replace with your exact J-Link device name
$PROJECT_TEMP_DIR = Split-Path -Parent (& $JLINK_RTT_BIN --print-config | Select-String "JLINK_LOG_FILE=" | % { $_.Line.Split("=")[1] })
$RTT_LOG = Join-Path $PROJECT_TEMP_DIR "rtt.log"
```

**Timed capture** — auto-stop after N seconds, suitable for quick log collection:

```bash
"${JLINK_RTT_BIN}" --device "${DEVICE}" --log-out "${RTT_LOG}" --log-timeout 12   # adjust 12s as needed
echo "exit=$?"
echo "log=${RTT_LOG}"
```

A non-zero exit here means the capture window did **not** run to completion — the RTT connection closed early (e.g. the target was reset) and the log may be incomplete. Treat it as a capture failure, not a tool error.

**Pattern-triggered capture** — exit when a specific pattern appears in RTT output:

```bash
"${JLINK_RTT_BIN}" --device "${DEVICE}" --log-out "${RTT_LOG}" --log-match "Application started" --log-timeout 30 --rtt-delay 1.5
echo "exit=$?"
echo "log=${RTT_LOG}"
```

**Send command to running session** — inject commands into RTT downlink channel from host test scripts or another shell (payload limit 1 MB):

```bash
# Text command (automatically appends \n, supports \n \r \t \\ \xHH escapes)
"${JLINK_RTT_BIN}" --send "help"

# Hex binary payload (supports 0102030a, 0x01 0x02, etc.)
"${JLINK_RTT_BIN}" --send "0102030a" --hex

# Raw without appending \n
"${JLINK_RTT_BIN}" --send "status" --no-newline
```

**Interactive console** — forward keystrokes line-by-line directly to RTT downlink (raw text, escapes not parsed):

```bash
"${JLINK_RTT_BIN}" --device "${DEVICE}" --log-out "${RTT_LOG}" -i
```

**Continuous stream** — no timeout, runs until stopped. Stop by running `--stop` from another shell:

**Start (AI-Friendly & Non-blocking):**
```bash
# Start in background via double-fork to prevent hanging, write logs to startup.log
( "${JLINK_RTT_BIN}" --device "${DEVICE}" --log-out "${RTT_LOG}" > "${PROJECT_TEMP_DIR}/startup.log" 2>&1 & )

# Wait 1s and print startup log to verify if it started successfully (e.g. catch missing --device errors)
sleep 1
cat "${PROJECT_TEMP_DIR}/startup.log"

# Read RTT_LOG periodically to observe output
echo "${RTT_LOG}"  # or use read file tools, repeat as needed
```

**Stop:**
```bash
"${JLINK_RTT_BIN}" --stop
echo "log=${RTT_LOG}"
```

The tool handles all pre-flight checks internally. Its output is self-contained: every `[ERROR]` line is followed by `[INFO]` lines describing what to do next — follow them directly, no lookup or translation needed.

- **Do not read project files or run `lsusb` before running the tool.** Just run it and respond to the output.
- Use the loaded skill base path directly; do not list the scripts directory to verify it or guess another install path.
- When the tool exits 0 with `[INFO]` instructions (e.g. no `--device` given), follow the instructions: scan the project for the requested value, ask the user if not found, then run the command it prints.
- When the tool exits non-zero, read the `[ERROR]` + `[INFO]` lines and relay them to the user as the next action.
- Multi-probe: if multiple J-Links are connected or a stale serial is suspected, run the tool without `--device` first — its `[INFO]` output lists detected serials — then pass `--serial <SN>` explicitly on every invocation. The tool reads no config files.
- Exit buffering: every capture waits `--rtt-delay` (default 1.5s) after killing JLinkGDBServer for USB/port release. Do not mistake the trailing `Waiting ... for USB/port release` pause for a hang; back-to-back captures no longer need manual sleep.
- If `--send` reports "No running RTT session" when no session is active, delete stale `<tmp>/jlink-rtt-*/rtt_ctrl.port`.
- WSL + usbipd: if the probe disappears between captures (`lsusb` no longer lists `1366:`, and JLinkGDBServer reports `Could not connect to J-Link`), the usbip attachment was dropped by USB re-enumeration. Re-attach it (`usbconn jlink fix` in this setup) and retry; this is an environment issue, not a capture failure.
- For all options: `${JLINK_RTT_BIN} --help` (canonical flags only, no aliases: `--log-match`, `--log-timeout`, `--rtt-delay`, `--ready-timeout`).
- Quick usage examples: `${JLINK_RTT_BIN} --example`.

## Device Name Resolution

`--device` takes an exact J-Link device name (e.g. `nRF52840_xxAA`). To find it, search the J-Link device database (no hardware needed, `JLinkExe ExpDevList` / `JLink.exe` on Windows):

```bash
"${JLINK_RTT_BIN}" --search-device nrf52840
```

The command prints every match as `vendor | device`, ordered by match quality (exact, prefix, contains), and exits 0 when matches exist / non-zero with suggestions when none do. Pass the exact device string to `--device`.
