# DeepSpeed

An AI-powered background system optimizer built in Rust. Works on **macOS, Linux, and Windows** with any CPU architecture and memory size. Originally designed for the Apple M2 with 8GB unified memory, where memory pressure and swap usage have the biggest impact on day-to-day performance.

---

## How it works

DeepSpeed runs as a lightweight background daemon. Every 30 seconds it samples your system state using native OS tools and the cross-platform `sysinfo` crate, then applies optimizations automatically.

**Two-layer engine:**

1. **Rule-based optimizer** (always active, zero API cost) — fires every cycle. Renices memory-heavy background processes, optionally purges disk cache, and sends native OS notifications when pressure is high.

2. **AI escalation** (Claude API, called sparingly) — triggered only when memory stays critical for 90+ seconds OR swap exceeds 70%. Claude analyzes the full system snapshot and returns structured JSON recommendations. A 10-minute cooldown prevents back-to-back calls. Typical usage: 0–2 API calls per day.

---

## Features

- Monitors memory, swap, CPU, kernel memory, and cache per-process
- Detects OS-native memory pressure level:
  - **macOS** — `memory_pressure` command
  - **Linux** — `/proc/pressure/memory` (PSI, kernel 4.20+), falls back to % thresholds
  - **Windows** — computed from available memory %
- Detects thermal throttling:
  - **macOS** — `pmset -g therm` (CPU speed limit)
  - **Linux** — `/sys/class/thermal/thermal_zone*/temp`
  - **Windows** — `wmic MSAcpi_ThermalZoneTemperature`
- Priority lowering (renice) for heavy non-system processes:
  - **macOS / Linux** — `renice` (Unix nice values)
  - **Windows** — PowerShell `PriorityClass = BelowNormal/Idle`
- Optional disk cache purge:
  - **macOS** — `purge`
  - **Linux** — `/proc/sys/vm/drop_caches` (requires passwordless sudo for tee)
  - **Windows** — not supported
- Native OS notifications:
  - **macOS** — AppleScript / Notification Center
  - **Linux** — `notify-send` (libnotify)
  - **Windows** — PowerShell system tray balloon
- Auto-start daemon on login:
  - **macOS** — launchd agent
  - **Linux** — systemd user service
  - **Windows** — Task Scheduler
- Works on any memory size — thresholds are percentage-based
- Works on any CPU (Apple Silicon, Intel, AMD, ARM)
- Single small binary, minimal CPU/IO footprint

---

## Installation

**Prerequisite:** [Rust toolchain](https://rustup.rs)

### macOS / Linux

```bash
git clone <repo>
cd DeepSpeed
bash scripts/install.sh
```

### Windows

```powershell
git clone <repo>
cd DeepSpeed
.\scripts\install.ps1
```

The installer will:
1. Build the release binary with `cargo build --release`
2. Copy it to a user-writable bin directory (no admin rights needed)
3. Add that directory to your `PATH`
4. Copy the default config to the platform config directory
5. Register and start the background daemon

---

## Configuration

| Platform | Config file location |
|---|---|
| macOS / Linux | `~/.config/deepspeed/deepspeed.toml` |
| Windows | `%APPDATA%\deepspeed\deepspeed.toml` |

```toml
[general]
monitor_interval_secs = 30    # How often to sample metrics
log_level = "info"
notifications_enabled = true

[thresholds]
memory_warning_pct = 80.0     # Start renicing at this memory %
memory_critical_pct = 90.0    # Escalate to AI after 3 cycles at this level
swap_warning_pct = 40.0
swap_critical_pct = 70.0      # Always escalates to AI
process_heavy_memory_mb = 500 # Renice candidates above this MB

[actions]
allow_purge_cache = false      # macOS/Linux only; set true to enable
allow_renice = true
renice_value = 10              # Unix: nice 0–19; Windows maps to BelowNormal/Idle
min_process_age_secs = 120     # Don't touch freshly started processes

[ai]
api_key = ""                   # Or set ANTHROPIC_API_KEY env var
model = "claude-haiku-4-5-20251001"
min_confidence = 0.75
action_cooldown_secs = 600     # Min seconds between AI calls
```

**For large-memory systems** (16 GB+), raise `process_heavy_memory_mb` to 1000–2000 so only truly heavy processes are targeted.

To enable AI mode:
```bash
# Option 1: config file
[ai]
api_key = "sk-ant-..."

# Option 2: environment variable
export ANTHROPIC_API_KEY=sk-ant-...   # macOS / Linux
$env:ANTHROPIC_API_KEY = "sk-ant-..."  # Windows PowerShell
```

---

## CLI commands

```bash
deepspeed status      # Live system snapshot (memory, swap, CPU, top processes)
deepspeed config      # Print the active resolved configuration
deepspeed install     # Register/reinstall the background daemon
deepspeed uninstall   # Stop and remove the daemon
deepspeed start       # Run in the foreground (for debugging / testing)
```

---

## Logs

| Platform | Log location |
|---|---|
| macOS | `~/Library/Logs/deepspeed.log` |
| Linux (systemd) | `journalctl --user -u deepspeed -f` |
| Linux (file) | `~/.local/share/deepspeed/deepspeed.log` |
| Windows | `%LOCALAPPDATA%\DeepSpeed\deepspeed.log` |

---

## What DeepSpeed will and won't do

| Action | Allowed |
|---|---|
| Lower process CPU/memory priority (renice) | Yes |
| Purge disk cache | Yes, if opted in (macOS/Linux) |
| Send native OS notifications | Yes |
| Kill or force-quit processes | Never |
| Touch OS system processes | Never |
| Make network requests other than Claude API | Never |

System processes are always protected per platform: macOS (WindowServer, Dock, Finder…), Linux (systemd, kthreadd, Xorg, pipewire…), Windows (System, csrss.exe, lsass.exe, explorer.exe…).

---

## Project structure

```
DeepSpeed/
├── Cargo.toml
├── .gitignore
├── src/
│   ├── main.rs          CLI + daemon loop + platform-specific daemon install
│   ├── monitor.rs       System metrics with platform-specific backends
│   ├── optimizer.rs     Rule-based engine — no API cost
│   ├── ai_engine.rs     Claude escalation — called only when rules fail
│   ├── actions.rs       renice/priority, cache purge, OS notifications
│   └── config.rs        TOML config with platform-correct paths
├── config/
│   └── deepspeed.toml   Default config template (tracked in git)
└── scripts/
    ├── install.sh        macOS + Linux installer
    └── install.ps1       Windows PowerShell installer
```

---

## Uninstall

```bash
# macOS / Linux
deepspeed uninstall
rm ~/.local/bin/deepspeed
rm -rf ~/.config/deepspeed

# Windows (PowerShell)
deepspeed uninstall
Remove-Item "$env:LOCALAPPDATA\DeepSpeed" -Recurse
Remove-Item "$env:APPDATA\deepspeed" -Recurse
```
