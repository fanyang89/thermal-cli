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
- Daemon-only ServerChan alerts with per-component overheat and resolved events
- Configurable alert severity and failed-delivery retry interval
- Extensible notification channels through the `push-notify` crate

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

## Push notifications

Push delivery is active only in headless daemon mode. Set a ServerChan Turbo
(`SCT...`) or ServerChan³ (`sctp...`) SendKey through the environment:

```sh
export SERVERCHAN_SENDKEY='SCT...'

# Verify delivery from daemon mode
thermal-cli --daemon --test-notification

# Monitor continuously without rendering the dashboard
thermal-cli --daemon
```

The daemon sends one overheat alert when a component enters the configured
state. After that component cools 5 °C below the alert threshold, it sends one
`resolved` notification. This hysteresis prevents notifications from flapping
near the threshold. Normal and `--watch` dashboard modes never send
notifications.

The default alert level is `hot`. Failed notifications are retried after 60
seconds by default:

```sh
thermal-cli --daemon --notify-level critical --notify-retry-interval 120
```

Daemon mode remains in the foreground so it works correctly with systemd,
runit, or another process supervisor. For example:

```ini
[Service]
EnvironmentFile=/etc/thermal-cli.env
ExecStart=/usr/local/bin/thermal-cli --daemon
Restart=on-failure
```

SendKeys are never included in delivery errors. Prefer the environment variable
over `--serverchan-sendkey` to avoid storing credentials in shell history.

### Adding another channel

The local `push-notify` crate keeps providers independent from the temperature
monitor. Implement `push_notify::Channel`, then register the implementation
with `push_notify::Dispatcher`. The dispatcher attempts every registered
channel even when another channel fails.

## Platform support

`thermal-cli` currently targets Linux. It reads temperature data from
`/sys/class/hwmon` and falls back to `/sys/class/thermal` when no hwmon
sensors are exposed. Some virtual machines and containers do not expose host
temperature sensors; in that case, the dashboard reports that no sensors were
found.
