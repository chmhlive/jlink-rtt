#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
log_info() { printf '[INFO] %s\n' "$*"; }
fail() { printf '[ERROR] %s\n' "$*" >&2; exit 1; }

log_info "Building jlink-rtt Rust binary in release mode..."
cargo build --release

BINARY_PATH="${SCRIPT_DIR}/target/release/jlink-rtt"
TMP_DIR="$(mktemp -d)"

cleanup() {
    rm -rf "${TMP_DIR}"
    # Kill any left-behind fake server from --stop test.
    kill "${FAKE_JLINK_PID:-}" 2>/dev/null || true
    pkill -f "python3.*simulate_ports" 2>/dev/null || true
}
trap cleanup EXIT

dump_debug() {
    local status=$?

    if ((status != 0)); then
        printf '[ERROR] Test failed with status %s\n' "${status}" >&2
        for file in \
            "${TMP_DIR}/rtt_output.log" \
            "${TMP_DIR}/captured_rtt.log" \
            "${TMP_DIR}/jlink.log" \
            "${TMP_DIR}/gdb.log" \
            "${TMP_DIR}/print_config.log" \
            "${TMP_DIR}/env_ignored.log" \
            "${TMP_DIR}/no_config.log" \
            "${TMP_DIR}/init_output.log" \
            "${TMP_DIR}/existing_init_output.log" \
            "${TMP_DIR}/no_config_output.log" \
            "${TMP_DIR}/no_config_serial_output.log" \
            "${TMP_DIR}/no_probe_output.log" \
            "${TMP_DIR}/capture_ok_output.log" \
            "${TMP_DIR}/match_timeout_output.log" \
            "${TMP_DIR}/python_simulator.log" \
            "${TMP_DIR}/stop_output.log"; do
            if [[ -f "${file}" ]]; then
                printf '\n--- %s ---\n' "${file}" >&2
                sed -n '1,160p' "${file}" >&2
            fi
        done
    fi

    cleanup
    exit "${status}"
}
trap dump_debug EXIT

mkdir -p "${TMP_DIR}/bin" "${TMP_DIR}/project/subdir"

# Fake host tools let the test cover orchestration without USB/J-Link hardware.
cat > "${TMP_DIR}/bin/JLinkGDBServer" <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail

printf '%s\n' "$*" > "${JLINK_RTT_TEST_TMP}/jlink_args"
touch "${JLINK_RTT_TEST_TMP}/server_started"

# Parse ports from args
gdb_port=2331
rtt_port=19021

while [[ $# -gt 0 ]]; do
    case "$1" in
        -port)
            gdb_port="$2"
            shift 2
            ;;
        -RTTTelnetPort)
            rtt_port="$2"
            shift 2
            ;;
        *)
            shift
            ;;
    esac
done

# Pass ports to python script via environment variables to avoid quotes and newlines issues in bash -c
export GDB_PORT_ENV="${gdb_port}"
export RTT_PORT_ENV="${rtt_port}"

# Start python simulator in background
python3 -c '
# simulate_ports
import os, socket, time, threading
gdb_port = int(os.environ["GDB_PORT_ENV"])
rtt_port = int(os.environ["RTT_PORT_ENV"])

def monitor_parent():
    parent_pid = os.getppid()
    print(f"[DEBUG] Python PID: {os.getpid()}, Parent PID: {parent_pid}")
    while True:
        try:
            with open(f"/proc/{parent_pid}/stat", "r") as f:
                stat = f.read().split()
                if stat[2] == "Z":
                    print(f"[DEBUG] Parent {parent_pid} became zombie. Exiting.")
                    os._exit(0)
        except IOError:
            print(f"[DEBUG] Parent {parent_pid} stat not found. Exiting.")
            os._exit(0)
        time.sleep(0.05)

def listen_gdb(port):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", port))
    s.listen(5)
    while True:
        try:
            conn, addr = s.accept()
            conn.close()
        except Exception:
            break

def handle_rtt_client(conn, data):
    try:
        conn.sendall(data)
        time.sleep(5)
        conn.close()
    except Exception:
        pass

def listen_rtt(port, data):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", port))
    s.listen(5)
    while True:
        try:
            conn, addr = s.accept()
            t = threading.Thread(target=handle_rtt_client, args=(conn, data))
            t.daemon = True
            t.start()
        except Exception:
            break

# Monitor parent thread
t_mon = threading.Thread(target=monitor_parent)
t_mon.daemon = True
t_mon.start()

# Listen on GDB port in thread
t = threading.Thread(target=listen_gdb, args=(gdb_port,))
t.daemon = True
t.start()

# Listen on RTT port
listen_rtt(rtt_port, b"boot line\nApplication started\n")
' > "${JLINK_RTT_TEST_TMP}/python_simulator.log" 2>&1 &

cleanup_srv() {
    rm -f "${JLINK_RTT_TEST_TMP}/server_started"
    exit 0
}
trap cleanup_srv TERM INT

while true; do
    sleep 1
done
EOF

cat > "${TMP_DIR}/bin/nc" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF

chmod +x "${TMP_DIR}/bin/JLinkGDBServer" "${TMP_DIR}/bin/nc"

# Fake JLinkExe for device database resolution (used by --init fuzzy matching) and reset orchestration
cat > "${TMP_DIR}/bin/JLinkExe" <<'JLEOF'
#!/usr/bin/env bash
set -Eeuo pipefail

printf '%s\n' "$*" >> "${JLINK_RTT_TEST_TMP:-/tmp}/jlink_run_args" 2>/dev/null || true

printf 'SEGGER J-Link Commander V9.99a (Compiled Jan 1 2026 00:00:00)\n'
if [[ "${*}" == *"/dev/null"* || "${*}" == *"/dev/zero"* ]]; then
    exit 0
fi
# Parse -CommandFile or -CommanderScript to find the script, then extract ExpDevList target path.
cmd_file=""
for arg in "$@"; do
    if [[ "${arg}" == "-CommandFile" || "${arg}" == "-CommanderScript" ]]; then continue; fi
    if [[ -f "${arg}" ]]; then cmd_file="${arg}"; break; fi
done
if [[ -n "${cmd_file}" ]]; then
    # Copy script content to verification log before it gets cleaned up by orchestrator
    cat "${cmd_file}" >> "${JLINK_RTT_TEST_TMP:-/tmp}/jlink_run_commands" 2>/dev/null || true
    
    csv_path="$(sed -n 's/^ExpDevList[[:space:]]\+//p' "${cmd_file}" | head -1)"
    if [[ -n "${csv_path}" ]]; then
        cat > "${csv_path}" <<CSV
"Manufacturer", "Device", "Core", {Flash areas}, {RAM areas}
"Nordic Semi", "nRF52840_xxAA", "Cortex-M4", { {0x00000000, 0x00100000} }, {0x20000000, 0x00040000}
"Nordic Semi", "nRF52833_xxAA", "Cortex-M4", { {0x00000000, 0x00080000} }, {0x20000000, 0x00020000}
"Nordic Semi", "nRF52832_xxAA", "Cortex-M4", { {0x00000000, 0x00080000} }, {0x20000000, 0x00010000}
"ST", "STM32F407IG", "Cortex-M4", { {0x08000000, 0x00100000} }, {0x20000000, 0x00020000}
CSV
    fi
fi
exit 0
JLEOF
chmod +x "${TMP_DIR}/bin/JLinkExe"

cat > "${TMP_DIR}/project/.jlink-rtt.env" <<EOF
DEVICE=NRF52840_XXAA
JLINK_IF=SWD
SPEED=4000
HOST=127.0.0.1
GDB_PORT=32331
RTT_PORT=39021
RTT_READY_TIMEOUT=2
LOG_FILE=${TMP_DIR}/jlink.log
GDB_LOG_FILE=${TMP_DIR}/gdb.log
EOF

OUTPUT_FILE="${TMP_DIR}/rtt_output.log"
OUT_FILE="${TMP_DIR}/captured_rtt.log"

log_info "Test 1: Run RTT capture with matching pattern exit..."
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "Application started" \
        --match-timeout 3 \
        --out "${OUT_FILE}" \
        > "${OUTPUT_FILE}" 2>&1
) && pattern_ok=1 || pattern_ok=0

((pattern_ok == 1)) || fail "Pattern-triggered capture did not exit 0."

grep -Fq 'Application started' "${OUTPUT_FILE}" || fail "RTT output was not forwarded."
grep -Fq 'Application started' "${OUT_FILE}" || fail "RTT output was not saved."
grep -Fq 'Matched RTT pattern: Application started' "${OUTPUT_FILE}" || fail "Pattern-triggered match message missing."
grep -Fq -- '-device NRF52840_XXAA' "${TMP_DIR}/jlink_args" || fail "JLink device argument is missing."
grep -Fq -- '-RTTTelnetPort 39021' "${TMP_DIR}/jlink_args" || fail "RTT port argument is missing."
grep -Fq 'r' "${TMP_DIR}/jlink_run_commands" || fail "JLink Commander reset command (r) is missing."
grep -Fq 'g' "${TMP_DIR}/jlink_run_commands" || fail "JLink Commander go command (g) is missing."

# --- pattern-triggered timeout / close ---
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 2: Non-existent pattern mismatch exit..."
MATCH_TIMEOUT_OUT="${TMP_DIR}/match_timeout_output.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "NONEXISTENT_PATTERN" \
        --match-timeout 2 \
        > "${MATCH_TIMEOUT_OUT}" 2>&1
) && fail "Pattern-triggered timeout/close should exit non-zero." || true

(grep -Fq 'Timed out waiting for RTT pattern' "${MATCH_TIMEOUT_OUT}" || grep -Fq 'closed before pattern' "${MATCH_TIMEOUT_OUT}") \
    || fail "Pattern-triggered timeout/close message missing."

# --- print config ---
log_info "Test 3: Print config verification..."
PRINT_CONFIG="${TMP_DIR}/print_config.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    DEVICE=ENV_DEVICE \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --print-config \
        --device CLI_DEVICE \
        > "${PRINT_CONFIG}" 2>&1
)

grep -Fq 'CONFIG_FILE='"${TMP_DIR}"'/project/.jlink-rtt.env' "${PRINT_CONFIG}" || fail "Config file was not discovered within project root."
grep -Fq 'DEVICE=CLI_DEVICE' "${PRINT_CONFIG}" || fail "Command line did not override config."

# DEVICE=ENV_DEVICE is intentional: RTT settings must not be overridden by env vars.
log_info "Test 4: Ignore environmental variables override..."
ENV_IGNORED="${TMP_DIR}/env_ignored.log"
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    DEVICE=ENV_DEVICE \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --print-config \
        > "${ENV_IGNORED}" 2>&1
)

grep -Fq 'DEVICE=NRF52840_XXAA' "${ENV_IGNORED}" || fail "Config DEVICE was not used."
if grep -Fq 'DEVICE=ENV_DEVICE' "${ENV_IGNORED}"; then
    fail "Environment DEVICE unexpectedly overrode config."
fi

# Non-git directories only check the current directory unless --project-root is explicit.
log_info "Test 5: Scope limit config discovery..."
mkdir -p "${TMP_DIR}/outside/subdir"
cat > "${TMP_DIR}/outside/.jlink-rtt.env" <<EOF
DEVICE=SHOULD_NOT_LOAD
EOF

NO_CONFIG="${TMP_DIR}/no_config.log"
(
    cd "${TMP_DIR}/outside/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" --print-config > "${NO_CONFIG}" 2>&1
)

if grep -Fq 'DEVICE=SHOULD_NOT_LOAD' "${NO_CONFIG}"; then
    fail "Non-git search escaped current directory without an explicit project root."
fi

# --- --init mode ---
log_info "Test 6: Init configuration file mode..."
INIT_DIR="${TMP_DIR}/init_project"
mkdir -p "${INIT_DIR}"
INIT_CONFIG="${INIT_DIR}/.jlink-rtt.env"
INIT_OUT="${TMP_DIR}/init_output.log"

(
    cd "${INIT_DIR}"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${INIT_DIR}" \
        --init \
        --device NRF52840_XXAA \
        > "${INIT_OUT}" 2>&1
)

grep -Fq 'DEVICE=nRF52840_xxAA' "${INIT_CONFIG}" || fail "--init did not write DEVICE."
grep -Fq 'JLINK_IF=SWD' "${INIT_CONFIG}" || fail "--init did not write JLINK_IF."
grep -Fq 'SPEED=4000' "${INIT_CONFIG}" || fail "--init did not write SPEED."
grep -Fq 'Created config:' "${INIT_OUT}" || fail "--init did not print config created message."

# --- no-config message ---
log_info "Test 7: Informative guide on no config file..."
NO_CFG_DIR="${TMP_DIR}/no_config_project"
mkdir -p "${NO_CFG_DIR}"
NO_CFG_OUT="${TMP_DIR}/no_config_output.log"

(
    cd "${NO_CFG_DIR}"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${NO_CFG_DIR}" \
        > "${NO_CFG_OUT}" 2>&1
)

grep -Fq 'No .jlink-rtt.env found' "${NO_CFG_OUT}" || fail "No-config did not print missing config message."
grep -Fq 'Scan the project for the DEVICE name' "${NO_CFG_OUT}" || fail "No-config did not print scan-project hint."
grep -Fq -- '--init --device' "${NO_CFG_OUT}" || fail "No-config did not print --init command hint."

# --- no-config with single J-Link probe (auto-detect serial) ---
log_info "Test 8: Auto-detect serial in init hint..."
cat > "${TMP_DIR}/bin/lsusb" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == "-v" ]]; then
    printf 'Bus 001 Device 004: ID 1366:1024 SEGGER J-Link\n'
    printf '  iSerial                 3 000683041131\n'
    exit 0
fi
printf 'Bus 001 Device 004: ID 1366:1024 SEGGER J-Link\n'
exit 0
EOF
chmod +x "${TMP_DIR}/bin/lsusb"

NO_CFG_SERIAL_OUT="${TMP_DIR}/no_config_serial_output.log"
(
    cd "${NO_CFG_DIR}"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${NO_CFG_DIR}" \
        > "${NO_CFG_SERIAL_OUT}" 2>&1
)

grep -Fq -- '--serial 000683041131' "${NO_CFG_SERIAL_OUT}" || fail "No-config did not auto-detect serial in init command."

# --- no_probe warning (lsusb returns nothing) ---
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 9: Warning when no USB probes found..."
cat > "${TMP_DIR}/bin/lsusb" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "${TMP_DIR}/bin/lsusb"

NO_PROBE_OUT="${TMP_DIR}/no_probe_output.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "Application started" \
        --match-timeout 3 \
        > "${NO_PROBE_OUT}" 2>&1
)

grep -Fq 'No SEGGER/J-Link USB device detected' "${NO_PROBE_OUT}" || fail "Missing USB not detected warning."

# --- --init with existing config should die with hint ---
log_info "Test 10: Fail --init on existing config file..."
EXISTING_INIT_OUT="${TMP_DIR}/existing_init_output.log"
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --init \
        --device NRF52840_XXAA \
        > "${EXISTING_INIT_OUT}" 2>&1 || true
)
if ! grep -Fq 'Config file already exists' "${EXISTING_INIT_OUT}"; then
    fail "--init on existing config did not report conflict."
fi

# --- --stop kills running session ---
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 11: Stop command kills running session..."
STOP_OUT="${TMP_DIR}/stop_output.log"

# Start fake JLinkGDBServer in background with matching ports.
JLINK_RTT_TEST_TMP="${TMP_DIR}" PATH="${TMP_DIR}/bin:${PATH}" \
"${TMP_DIR}/bin/JLinkGDBServer" -port 32331 -RTTTelnetPort 39021 &
FAKE_JLINK_PID=$!

# Wait for fake server to be ready (up to 3s).
for _ in $(seq 1 30); do
    [[ -f "${TMP_DIR}/server_started" ]] && break
    sleep 0.1
done

# First --stop should kill the server by port match, exit 0.
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --gdb-port 32331 \
        --rtt-port 39021 \
        --stop \
        > "${STOP_OUT}" 2>&1
) || fail "--stop failed."

grep -Fq 'Stop signal sent' "${STOP_OUT}" || fail "--stop did not report success."

# Verify the fake server was killed (timeout in case process lingers).
wait_sec=10
while kill -0 "${FAKE_JLINK_PID}" 2>/dev/null; do
    sleep 0.5
    ((wait_sec--))
    if ((wait_sec <= 0)); then
        fail "--stop should have killed JLinkGDBServer (PID ${FAKE_JLINK_PID})."
    fi
done

# --- --stop on idle session (no matching process) should exit 1 ---
log_info "Test 12: Stop command on idle session fails..."
pkill -f "JLinkGDBServer.*-port 32331.*-RTTTelnetPort 39021" 2>/dev/null || true

# Test PID file cleanup
project_temp_dir="$(find /tmp -maxdepth 1 -type d -name "jlink-rtt-project-*" | head -n 1)"
if [[ -n "${project_temp_dir}" ]]; then
    touch "${project_temp_dir}/jlink_rtt.pid"
fi
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --gdb-port 32331 \
        --rtt-port 39021 \
        --stop \
        > "${STOP_OUT}" 2>&1
) && fail "--stop on idle session should exit 1." || true

grep -Fq 'No running RTT session' "${STOP_OUT}" || fail "--stop should report no session."

if [[ -n "${project_temp_dir}" && -f "${project_temp_dir}/jlink_rtt.pid" ]]; then
    fail "--stop did not clean up stale PID file."
fi

log_info "All jlink-rtt automated coverage tests passed successfully!"
