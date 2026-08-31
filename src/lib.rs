mod display;
pub mod sensors;

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use display::Unit;
use supports_color::Stream;

#[derive(Debug, Parser)]
#[command(
    name = "thermal-cli",
    version,
    about = "Display hardware temperatures in a polished terminal dashboard",
    after_help = "Examples:\n  thermal-cli\n  thermal-cli --watch\n  thermal-cli --watch --interval 1 --fahrenheit"
)]
pub struct Args {
    /// Continuously refresh the dashboard
    #[arg(short, long)]
    watch: bool,

    /// Refresh interval in seconds (used with --watch)
    #[arg(short, long, default_value_t = 2.0, value_parser = parse_interval)]
    interval: f64,

    /// Display temperatures in Fahrenheit
    #[arg(short, long)]
    fahrenheit: bool,

    /// Disable ANSI colors
    #[arg(long)]
    no_color: bool,

    /// Override the sysfs root (useful for containers and testing)
    #[arg(long, default_value = "/sys", hide = true)]
    sysfs_root: PathBuf,
}

pub fn run() -> Result<()> {
    run_with_args(Args::parse())
}

pub fn run_with_args(args: Args) -> Result<()> {
    let stdout = io::stdout();
    let interactive = stdout.is_terminal();
    let color = !args.no_color && supports_color::on(Stream::Stdout).is_some();
    let unit = if args.fahrenheit {
        Unit::Fahrenheit
    } else {
        Unit::Celsius
    };
    let hostname = hostname::get()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let hostname = if hostname.is_empty() {
        "localhost".to_owned()
    } else {
        hostname
    };

    if !args.watch {
        let sensors = sensors::discover(&args.sysfs_root).with_context(|| {
            format!(
                "failed to read temperature sensors from {}",
                args.sysfs_root.display()
            )
        })?;
        println!("{}", display::render(&sensors, unit, color, &hostname));
        return Ok(());
    }

    let running = Arc::new(AtomicBool::new(true));
    let signal_flag = Arc::clone(&running);
    ctrlc::set_handler(move || signal_flag.store(false, Ordering::SeqCst))
        .context("failed to install Ctrl+C handler")?;

    let mut output = stdout.lock();
    while running.load(Ordering::SeqCst) {
        let sensors = sensors::discover(&args.sysfs_root).with_context(|| {
            format!(
                "failed to read temperature sensors from {}",
                args.sysfs_root.display()
            )
        })?;
        if interactive {
            write!(output, "\x1b[2J\x1b[H")?;
        }
        writeln!(
            output,
            "{}",
            display::render(&sensors, unit, color, &hostname)
        )?;
        if interactive {
            writeln!(output, "  Press Ctrl+C to stop")?;
        }
        output.flush()?;
        sleep_interruptibly(&running, Duration::from_secs_f64(args.interval));
    }

    Ok(())
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
    fn validates_refresh_interval() {
        assert_eq!(parse_interval("0.2"), Ok(0.2));
        assert_eq!(parse_interval("3600"), Ok(3600.0));
        assert!(parse_interval("0.1").is_err());
        assert!(parse_interval("nan").is_err());
        assert!(parse_interval("fast").is_err());
    }
}
