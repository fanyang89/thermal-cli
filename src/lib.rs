mod alerts;
mod display;
mod install;
pub mod sensors;

use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use alerts::{AlertDelivery, AlertKind, AlertTracker, NotifyLevel};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use display::{SessionPeak, Unit};
use push_notify::{Dispatcher, ServerChan};
use supports_color::Stream;

#[derive(Debug, Parser)]
#[command(
    name = "thermal-cli",
    version,
    about = "Display hardware temperatures in a polished terminal dashboard",
    after_help = "Examples:\n  thermal-cli\n  thermal-cli -i\n  thermal-cli -i --interval 1 --fahrenheit\n  SERVERCHAN_SENDKEY=SCT... thermal-cli --daemon\n  SERVERCHAN_SENDKEY=SCT... thermal-cli --daemon --test-notification\n  sudo /path/to/thermal-cli install"
)]
pub struct Args {
    /// Interactively display live temperatures and changes
    #[arg(
        short = 'i',
        long,
        visible_alias = "watch",
        visible_short_alias = 'w',
        conflicts_with = "daemon"
    )]
    interactive: bool,

    /// Run headless temperature alert monitoring
    #[arg(short, long)]
    daemon: bool,

    /// Refresh interval in seconds (used with --interactive or --daemon)
    #[arg(short = 'r', long, default_value_t = 2.0, value_parser = parse_interval)]
    interval: f64,

    /// Display temperatures in Fahrenheit
    #[arg(short, long)]
    fahrenheit: bool,

    /// Disable ANSI colors
    #[arg(long)]
    no_color: bool,

    /// ServerChan SendKey used by manually started daemon mode
    #[arg(
        long,
        env = "SERVERCHAN_SENDKEY",
        value_name = "SENDKEY",
        hide_env_values = true
    )]
    serverchan_sendkey: Option<String>,

    /// Minimum temperature state that triggers a daemon alert
    #[arg(long, value_enum, default_value_t = NotifyLevel::Hot)]
    notify_level: NotifyLevel,

    /// Seconds before retrying a failed notification
    #[arg(long, default_value_t = 60, value_parser = parse_retry_interval)]
    notify_retry_interval: u64,

    /// Send a daemon-mode test notification and exit
    #[arg(long, requires = "daemon")]
    test_notification: bool,

    /// Override the sysfs root (useful for containers and testing)
    #[arg(long, default_value = "/sys", hide = true)]
    sysfs_root: PathBuf,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Install and enable the systemd daemon service
    Install(InstallArgs),
}

#[derive(Debug, clap::Args)]
struct InstallArgs {
    /// Read the raw ServerChan SendKey from this file
    #[arg(long, value_name = "PATH")]
    sendkey_file: Option<PathBuf>,

    /// Enable the service without starting it
    #[arg(long)]
    no_start: bool,
}

pub fn run() -> Result<()> {
    run_with_args(Args::parse())
}

pub fn run_with_args(args: Args) -> Result<()> {
    if let Some(Commands::Install(options)) = &args.command {
        return install::run(install::Options {
            no_start: options.no_start,
            sendkey_file: options.sendkey_file.as_deref(),
        });
    }

    let hostname = hostname::get()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let hostname = if hostname.is_empty() {
        "localhost".to_owned()
    } else {
        hostname
    };

    if args.daemon {
        return run_daemon(&args, &hostname);
    }

    run_dashboard(&args, &hostname)
}

fn run_dashboard(args: &Args, hostname: &str) -> Result<()> {
    let stdout = io::stdout();
    let tty = stdout.is_terminal();
    let color = !args.no_color && supports_color::on(Stream::Stdout).is_some();
    let unit = if args.fahrenheit {
        Unit::Fahrenheit
    } else {
        Unit::Celsius
    };

    if !args.interactive {
        let sensors = discover_sensors(&args.sysfs_root)?;
        println!("{}", display::render(&sensors, unit, color, hostname));
        return Ok(());
    }

    let running = shutdown_flag()?;
    let mut output = stdout.lock();
    let mut previous = HashMap::new();
    let mut previous_throttle = None;
    let mut throttled_since_start = false;
    let mut session_peak: Option<SessionPeak> = None;
    while running.load(Ordering::SeqCst) {
        let sensors = discover_sensors(&args.sysfs_root)?;
        let throttle = sensors::throttle_counters(&args.sysfs_root);
        throttled_since_start |=
            previous_throttle.is_some_and(|previous| throttle.increased_since(previous));
        previous_throttle = Some(throttle);
        for sensor in &sensors {
            if session_peak
                .as_ref()
                .is_none_or(|peak| sensor.temperature_c > peak.temperature_c)
            {
                session_peak = Some(SessionPeak {
                    label: sensor.label.clone(),
                    temperature_c: sensor.temperature_c,
                });
            }
        }
        if tty {
            write!(output, "\x1b[2J\x1b[H")?;
        }
        writeln!(
            output,
            "{}",
            display::render_interactive(
                &sensors,
                unit,
                color,
                hostname,
                &previous,
                session_peak.as_ref(),
                throttled_since_start,
            )
        )?;
        if tty {
            writeln!(
                output,
                "  ↻ {:.1}s   Δ previous sample   Ctrl+C stop",
                args.interval
            )?;
        }
        output.flush()?;
        previous = sensors
            .iter()
            .map(|sensor| (sensor.source.clone(), sensor.temperature_c))
            .collect();
        sleep_interruptibly(&running, Duration::from_secs_f64(args.interval));
    }

    Ok(())
}

fn run_daemon(args: &Args, hostname: &str) -> Result<()> {
    let sendkey = configured_sendkey(args.serverchan_sendkey.as_deref())?;
    let dispatcher = notification_dispatcher(sendkey.as_deref())?;
    if args.test_notification {
        return send_test_notification(&dispatcher, hostname);
    }

    let running = shutdown_flag()?;
    let mut alerts = AlertTracker::new(
        args.notify_level,
        Duration::from_secs(args.notify_retry_interval),
    );
    if dispatcher.is_empty() {
        eprintln!("thermal-cli daemon started; notifications disabled");
    } else {
        eprintln!("thermal-cli daemon started; notifications enabled");
    }
    let mut previous_throttle = None;

    while running.load(Ordering::SeqCst) {
        match discover_sensors(&args.sysfs_root) {
            Ok(sensors) if !dispatcher.is_empty() => {
                let throttle = sensors::throttle_counters(&args.sysfs_root);
                let throttled =
                    previous_throttle.is_some_and(|previous| throttle.increased_since(previous));
                previous_throttle = Some(throttle);
                for delivery in alerts.process(&dispatcher, &sensors, hostname, throttled) {
                    report_delivery(&delivery);
                }
            }
            Ok(_) => {}
            Err(error) => eprintln!("warning: {error:#}"),
        }
        sleep_interruptibly(&running, Duration::from_secs_f64(args.interval));
    }

    eprintln!("thermal-cli daemon stopped");
    Ok(())
}

fn discover_sensors(sysfs_root: &Path) -> Result<Vec<sensors::Sensor>> {
    sensors::discover(sysfs_root).with_context(|| {
        format!(
            "failed to read temperature sensors from {}",
            sysfs_root.display()
        )
    })
}

fn shutdown_flag() -> Result<Arc<AtomicBool>> {
    let running = Arc::new(AtomicBool::new(true));
    let signal_flag = Arc::clone(&running);
    ctrlc::set_handler(move || signal_flag.store(false, Ordering::SeqCst))
        .context("failed to install Ctrl+C handler")?;
    Ok(running)
}

fn configured_sendkey(value: Option<&str>) -> Result<Option<String>> {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        return Ok(Some(value.trim().to_owned()));
    }
    let Some(directory) = std::env::var_os("CREDENTIALS_DIRECTORY") else {
        return Ok(None);
    };
    read_systemd_credential(Path::new(&directory))
}

fn read_systemd_credential(directory: &Path) -> Result<Option<String>> {
    let path = directory.join(install::CREDENTIAL_NAME);
    let value = fs::read_to_string(&path)
        .with_context(|| format!("failed to read systemd credential {}", path.display()))?;
    let value = value.trim();
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

fn notification_dispatcher(sendkey: Option<&str>) -> Result<Dispatcher> {
    let mut dispatcher = Dispatcher::new();
    if let Some(sendkey) = sendkey.filter(|value| !value.trim().is_empty()) {
        dispatcher.add(ServerChan::new(sendkey).context("invalid ServerChan configuration")?);
    }
    Ok(dispatcher)
}

fn send_test_notification(dispatcher: &Dispatcher, hostname: &str) -> Result<()> {
    if dispatcher.is_empty() {
        bail!("notifications are disabled; configure a ServerChan SendKey first");
    }
    let report = dispatcher.send(&alerts::test_notification(hostname));
    if !report.failures.is_empty() {
        let failures = report
            .failures
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        bail!("test notification failed: {failures}");
    }
    println!(
        "Test notification sent via {}.",
        report.delivered.join(", ")
    );
    Ok(())
}

fn report_delivery(delivery: &AlertDelivery) {
    for error in &delivery.report.failures {
        eprintln!("warning: {error}");
    }
    if delivery.report.delivered.is_empty() {
        return;
    }

    let event = match delivery.kind {
        AlertKind::Overheated => "overheat alert",
        AlertKind::Resolved => "resolved notice",
    };
    eprintln!(
        "{event} sent via {}: {}",
        delivery.report.delivered.join(", "),
        delivery.sensor_labels.join(", ")
    );
}

fn parse_interval(value: &str) -> Result<f64, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|_| "interval must be a number".to_owned())?;
    if !seconds.is_finite() || !(0.2..=3600.0).contains(&seconds) {
        return Err("interval must be between 0.2 and 3600 seconds".to_owned());
    }
    Ok(seconds)
}

fn parse_retry_interval(value: &str) -> Result<u64, String> {
    let seconds = value
        .parse::<u64>()
        .map_err(|_| "retry interval must be a whole number".to_owned())?;
    if !(1..=86_400).contains(&seconds) {
        return Err("retry interval must be between 1 and 86400 seconds".to_owned());
    }
    Ok(seconds)
}

fn sleep_interruptibly(running: &AtomicBool, duration: Duration) {
    let step = Duration::from_millis(50);
    let mut remaining = duration;
    while running.load(Ordering::SeqCst) && !remaining.is_zero() {
        let current = remaining.min(step);
        thread::sleep(current);
        remaining = remaining.saturating_sub(current);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_configuration_is_valid() {
        Args::command().debug_assert();
    }

    #[test]
    fn parses_install_subcommand() {
        let args = Args::try_parse_from([
            "thermal-cli",
            "install",
            "--sendkey-file",
            "/root/serverchan-sendkey",
            "--no-start",
        ])
        .unwrap();

        let Some(Commands::Install(options)) = args.command else {
            panic!("install command was not parsed");
        };
        assert_eq!(
            options.sendkey_file,
            Some(PathBuf::from("/root/serverchan-sendkey"))
        );
        assert!(options.no_start);
    }

    #[test]
    fn daemon_and_interactive_modes_are_exclusive() {
        assert!(Args::try_parse_from(["thermal-cli", "-i", "--daemon"]).is_err());
        assert!(Args::try_parse_from(["thermal-cli", "--test-notification"]).is_err());
        assert!(Args::try_parse_from(["thermal-cli", "--daemon", "--test-notification"]).is_ok());
    }

    #[test]
    fn accepts_interactive_mode_and_legacy_watch_aliases() {
        assert!(
            Args::try_parse_from(["thermal-cli", "-i"])
                .unwrap()
                .interactive
        );
        assert!(
            Args::try_parse_from(["thermal-cli", "--interactive"])
                .unwrap()
                .interactive
        );
        assert!(
            Args::try_parse_from(["thermal-cli", "--watch"])
                .unwrap()
                .interactive
        );
        assert!(
            Args::try_parse_from(["thermal-cli", "-w"])
                .unwrap()
                .interactive
        );
    }

    #[test]
    fn validates_refresh_interval() {
        assert_eq!(parse_interval("0.2"), Ok(0.2));
        assert_eq!(parse_interval("3600"), Ok(3600.0));
        assert!(parse_interval("0.1").is_err());
        assert!(parse_interval("nan").is_err());
        assert!(parse_interval("fast").is_err());
    }

    #[test]
    fn validates_notification_retry_interval() {
        assert_eq!(parse_retry_interval("1"), Ok(1));
        assert_eq!(parse_retry_interval("86400"), Ok(86_400));
        assert!(parse_retry_interval("0").is_err());
        assert!(parse_retry_interval("86401").is_err());
        assert!(parse_retry_interval("1.5").is_err());
    }

    #[test]
    fn ignores_an_empty_serverchan_environment_value() {
        assert!(notification_dispatcher(Some("  ")).unwrap().is_empty());
    }

    #[test]
    fn test_notification_reports_when_notifications_are_disabled() {
        let dispatcher = Dispatcher::new();

        let error = send_test_notification(&dispatcher, "host").unwrap_err();

        assert!(error.to_string().contains("notifications are disabled"));
    }

    #[test]
    fn reads_serverchan_sendkey_from_systemd_credentials() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "thermal-cli-credential-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(install::CREDENTIAL_NAME),
            "SCTcredential123\n",
        )
        .unwrap();

        let value = read_systemd_credential(&directory).unwrap();

        fs::remove_dir_all(directory).unwrap();
        assert_eq!(value.as_deref(), Some("SCTcredential123"));
    }
}
