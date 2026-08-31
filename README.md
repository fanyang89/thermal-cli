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
- Interactive `-i` mode with live temperature deltas and trend arrows
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

### systemd service

Run the installer as root. It prompts for the ServerChan SendKey without
echoing it to the terminal:

```sh
sudo "$(command -v thermal-cli)" install
# ServerChan SendKey (empty disables notifications):
```

Press Enter without entering a key to install the daemon with notifications
disabled. For non-interactive automation, provide a raw key file with
`--sendkey-file PATH`; an empty file also disables notifications.

The installer:

- Copies the running executable to `/usr/local/bin/thermal-cli`
- Stores a provided SendKey at `/etc/thermal-cli/serverchan-sendkey` with mode `0600`
- Installs and hardens `/etc/systemd/system/thermal-cli.service`
- Enables and starts the service

Use `--no-start` to enable the service without starting it. Re-running
`thermal-cli install` upgrades the binary and prompts for the notification
setting again. Empty input removes any installed SendKey. When notifications
are enabled, the unit passes the key through systemd's `LoadCredential=`
mechanism rather than a process environment variable.

## Usage

```sh
# One-time snapshot
thermal-cli

# Interactive live temperatures and per-sample changes
thermal-cli -i

# Refresh every second in Fahrenheit
thermal-cli -i --interval 1 --fahrenheit

# Plain output suitable for logs
thermal-cli --no-color
```

Run `thermal-cli --help` for all options.

## Push notifications

Push delivery is active only in headless daemon mode. Without a SendKey, the
daemon continues running with notifications disabled. For manual daemon runs,
set a ServerChan Turbo (`SCT...`) or ServerChan³ (`sctp...`) SendKey through
the environment:

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
near the threshold. Normal and interactive dashboard modes never send
notifications. The previous `--watch` and `-w` flags remain aliases for
`--interactive` and `-i`.

The default alert level is `hot`. Failed notifications are retried after 60
seconds by default:

```sh
thermal-cli --daemon --notify-level critical --notify-retry-interval 120
```

Daemon mode remains in the foreground so it works correctly with systemd,
runit, or another process supervisor. Use `thermal-cli install` to install the
systemd service.

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
