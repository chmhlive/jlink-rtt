---
name: jlink-rtt
description: Read SEGGER J-Link RTT logs with RTT project config, reset/attach modes, pattern matching, and troubleshooting.
---

# J-Link RTT

## Quick Path

**Run the tool first — The tool does all and tells you exactly what to do next**

Use the compiled `jlink-rtt` binary (or `jlink-rtt.exe` on Windows). Do not rewrite JLinkGDBServer/JLinkExe/TCP socket orchestration.

Always run from the target project root. Three modes:

```bash
JLINK_RTT_BIN="./jlink-rtt" # Or ".\jlink-rtt.exe" on Windows
RTT_LOG="/tmp/\$(basename "\$PWD")_rtt.log"
```

**Timed capture** — exit when a specific pattern appears in RTT output, or timeout after N seconds:

```bash
"\${JLINK_RTT_BIN}" --out "\${RTT_LOG}" --match "Application started" --match-timeout 30
echo "exit=\$?"
echo "log=\${RTT_LOG}"
```

**Continuous stream** — no timeout, runs until stopped. Stop by running `--stop` from another shell:

**Start:**
```bash
"\${JLINK_RTT_BIN}" --out "\${RTT_LOG}" &
# read log periodically to observe output
cat "\${RTT_LOG}"  # or use read file tools, repeat as needed
```

**Stop:**
```bash
"\${JLINK_RTT_BIN}" --stop
echo "log=\${RTT_LOG}"
```

The tool handles all pre-flight checks internally. Its output is self-contained: every `[ERROR]` line is followed by `[INFO]` lines describing what to do next — follow them directly, no lookup or translation needed.

- **Do not read project files, check for `.jlink-rtt.env`, or run `lsusb` before running the tool.** Just run it and respond to the output.
- When the tool exits 0 with `[INFO]` instructions (e.g. no config found), follow the instructions: scan the project for the requested value, ask the user if not found, then run the command it prints.
- When the tool exits non-zero, read the `[ERROR]` + `[INFO]` lines and relay them to the user as the next action.
- For all options: `\${JLINK_RTT_BIN} --help`

## Device Name Resolution

`--init --device` accepts fuzzy names (e.g. `nrf52840`, `stm32f407`). The tool queries the J-Link device database via `JLinkExe ExpDevList` (or `JLink.exe` on Windows, no hardware needed) and auto-resolves:

- **Unique match** → uses the exact device name (e.g. `nrf52840` → `nRF52840_xxAA`)
- **Multiple matches** → prints all candidates as `[INFO]` hints, exits non-zero
- **No match** → prints `[ERROR]` + `[INFO]` with search suggestions

Use `--search-device <pattern>` to browse the database interactively.