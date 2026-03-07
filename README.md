# DeepSpeed

An AI-powered background system optimizer for macOS, built in Rust. Designed specifically for the M2 chip with 8GB unified memory — the configuration where memory pressure and swap usage have the biggest impact on day-to-day performance.

---

## How it works

DeepSpeed runs as a lightweight background daemon. Every 30 seconds it samples your system state using native macOS tools (`vm_stat`, `memory_pressure`, `pmset`) and the `sysinfo` crate, then applies optimizations automatically.

**Two-layer engine:**

1. **Rule-based optimizer** (always active, zero API cost) — fires on every cycle. Renices memory-heavy background processes, optionally purges disk cache, and sends macOS notifications when pressure is high.

2. **AI escalation** (Claude API, called sparingly) — triggered only when memory stays critical for 90+ seconds OR swap exceeds 70%. Claude analyzes the full system snapshot and returns structured JSON recommendations. A 10-minute cooldown prevents back-to-back calls. Typical usage: 0–2 API calls per day.

---

## Features

- Monitors memory, swap, CPU, wired memory, compressed memory, and per-process usage
- Detects macOS memory pressure level (`normal` / `warning` / `critical`)
- Detects thermal throttling via `pmset`
- Renices heavy non-system processes automatically (configurable threshold)
- Optional disk cache purge (`purge`) when memory is critical
- macOS native notifications via AppleScript
- Runs as a `launchd` agent — starts on login, restarts if it crashes
- Single 4MB binary, minimal CPU/IO footprint (`LowPriorityIO` + `Background` process type)
- Rule-only mode works without any API key

---

## Installation

**Prerequisites:** Rust toolchain (`rustup.rs`)

```bash
git clone <repo>
cd DeepSpeed
bash scripts/install.sh
```

This will:
1. Build the release binary
2. Install it to `~/.local/bin/deepspeed`
3. Add `~/.local/bin` to your `PATH` in `~/.zprofile` (if not already there)
4. Copy the default config to `~/.config/deepspeed/deepspeed.toml`
5. Install and start the `launchd` agent

---

## Configuration

Config file: `~/.config/deepspeed/deepspeed.toml`

```toml
[general]
monitor_interval_secs = 30    # How often to sample metrics
ai_interval_secs = 300        # (unused in escalation mode — kept for reference)
log_level = "info"
notifications_enabled = true

[thresholds]
memory_warning_pct = 80.0     # Start renicing at this memory %
memory_critical_pct = 90.0    # Escalate to AI after 3 cycles at this level
swap_warning_pct = 40.0
swap_critical_pct = 70.0      # Always escalates to AI
process_heavy_memory_mb = 500 # Processes above this are renice candidates

[actions]
allow_purge_cache = false      # Set true to enable `purge` on critical memory
allow_renice = true
renice_value = 10              # nice value applied (0=normal, 19=lowest priority)
protected_processes = ["Finder", "WindowServer", "Dock", ...]
min_process_age_secs = 120     # Don't touch processes younger than this

[ai]
api_key = ""                   # Or set ANTHROPIC_API_KEY env var
model = "claude-haiku-4-5-20251001"
max_tokens = 1024
min_confidence = 0.75          # Ignore AI recommendations below this confidence
action_cooldown_secs = 600     # Min seconds between AI calls
```

To enable AI mode, add your Anthropic API key:

```bash
# Option 1: config file
echo 'api_key = "sk-ant-..."' >> ~/.config/deepspeed/deepspeed.toml

# Option 2: environment variable (add to ~/.zshrc or ~/.zprofile)
export ANTHROPIC_API_KEY=sk-ant-...
```

---

## CLI commands

```bash
deepspeed status      # Live system snapshot (memory, swap, CPU, top processes)
deepspeed config      # Print the active resolved configuration
deepspeed install     # Install/reinstall the launchd agent
deepspeed uninstall   # Stop and remove the daemon
deepspeed start       # Run in the foreground (for debugging)
```

---

## Logs

```bash
tail -f ~/Library/Logs/deepspeed.log
```

---

## What DeepSpeed will and won't do

| Action | Allowed |
|---|---|
| Lower process CPU/memory priority (renice) | Yes |
| Purge disk cache (`purge`) | Yes, if opted in |
| Send macOS notifications | Yes |
| Kill or force-quit processes | Never |
| Touch system processes (kernel, WindowServer, Dock…) | Never |
| Make network requests other than Claude API | Never |

---

## Project structure

```
DeepSpeed/
├── Cargo.toml
├── src/
│   ├── main.rs          CLI + daemon loop + launchd install/uninstall
│   ├── monitor.rs       System metrics (vm_stat, memory_pressure, pmset, sysinfo)
│   ├── optimizer.rs     Rule-based engine — no API cost
│   ├── ai_engine.rs     Claude escalation — called only when rules fail
│   ├── actions.rs       renice, cache purge, macOS notifications
│   └── config.rs        TOML config loader
├── config/
│   └── deepspeed.toml   Default config template
└── scripts/
    └── install.sh       Build + install + launchd setup
```

---

## Uninstall

```bash
deepspeed uninstall    # stops daemon and removes launchd plist
rm ~/.local/bin/deepspeed
rm -rf ~/.config/deepspeed
```
