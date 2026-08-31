# thermal-cli

A polished, dependency-light hardware temperature monitor for Linux terminals.

```text
╭────────────────────────────────────────────────────────────────────────────╮
│  ◆  THERMAL MONITOR                                           workstation  │
├────────────────────────────────────────────────────────────────────────────┤
│  ●  Coretemp · Core 0             ██████░░░░░░░░░░░░    48.0 °C  NORMAL    │
│  ●  Coretemp · Package id 0       ██████████░░░░░░░░    62.5 °C  NORMAL    │
├────────────────────────────────────────────────────────────────────────────┤
│  2 sensors  ·  Hottest: Coretemp · Package id 0 (62.5 °C)                  │
╰────────────────────────────────────────────────────────────────────────────╯
```

## Features

- Reads standard Linux `hwmon` sensors with thermal-zone fallback
- Uses hardware-provided high and critical thresholds when available
- Color-coded state, temperature bars, and an at-a-glance summary
- Live watch mode with responsive Ctrl+C handling
- Celsius and Fahrenheit output
- Automatic color detection with `NO_COLOR` support
- Graceful handling of unavailable or malformed sensors

## Install

```sh
cargo install --path .
```

## Usage

```sh
# One-time snapshot
thermal-cli

# Refresh every two seconds
thermal-cli --watch

# Refresh every second in Fahrenheit
thermal-cli --watch --interval 1 --fahrenheit

# Plain output suitable for logs
thermal-cli --no-color
```

Run `thermal-cli --help` for all options.

## Platform support

`thermal-cli` currently targets Linux. It reads temperature data from
`/sys/class/hwmon` and falls back to `/sys/class/thermal` when no hwmon
sensors are exposed. Some virtual machines and containers do not expose host
temperature sensors; in that case, the dashboard reports that no sensors were
found.
