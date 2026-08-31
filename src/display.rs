use std::collections::HashMap;
use std::path::PathBuf;

use crate::sensors::{Sensor, SensorKind};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const BRIGHT_RED: &str = "\x1b[91m";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Celsius,
    Fahrenheit,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SessionPeak {
    pub(crate) label: String,
    pub(crate) temperature_c: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TemperatureState {
    Normal,
    Warm,
    Hot,
    Critical,
}

impl TemperatureState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Warm => "WARM",
            Self::Hot => "HOT",
            Self::Critical => "CRITICAL",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Self::Normal => GREEN,
            Self::Warm => YELLOW,
            Self::Hot => RED,
            Self::Critical => BRIGHT_RED,
        }
    }
}

pub fn render(sensors: &[Sensor], unit: Unit, color: bool, hostname: &str) -> String {
    render_dashboard(sensors, unit, color, hostname, None, None, false)
}

pub(crate) fn render_interactive(
    sensors: &[Sensor],
    unit: Unit,
    color: bool,
    hostname: &str,
    previous: &HashMap<PathBuf, f64>,
    peak: Option<&SessionPeak>,
    throttled: bool,
) -> String {
    render_dashboard(
        sensors,
        unit,
        color,
        hostname,
        Some(previous),
        peak,
        throttled,
    )
}

fn render_dashboard(
    sensors: &[Sensor],
    unit: Unit,
    color: bool,
    hostname: &str,
    previous: Option<&HashMap<PathBuf, f64>>,
    peak: Option<&SessionPeak>,
    throttled: bool,
) -> String {
    let mut output = String::new();
    let border = |text: &str| paint(text, CYAN, color);
    let title = paint(&format!("{:<44}", "◆  THERMAL MONITOR"), BOLD, color);
    let host = paint(&format!("{:>27}", truncate(hostname, 25)), DIM, color);

    output.push_str(&border(
        "╭────────────────────────────────────────────────────────────────────────────╮",
    ));
    output.push('\n');
    output.push_str(&format!(
        "{}  {} {}  {}\n",
        border("│"),
        title,
        host,
        border("│")
    ));
    output.push_str(&border(
        "├────────────────────────────────────────────────────────────────────────────┤",
    ));
    output.push('\n');

    if sensors.is_empty() {
        let message = paint(
            &format!("{:<74}", "◌  No temperature sensors found"),
            DIM,
            color,
        );
        output.push_str(&format!("{}  {}{}\n", border("│"), message, border("│")));
    } else {
        for sensor in sensors {
            output.push_str(&render_sensor(sensor, unit, color, previous));
            output.push('\n');
        }
    }

    output.push_str(&border(
        "├────────────────────────────────────────────────────────────────────────────┤",
    ));
    output.push('\n');
    let summary = if previous.is_some() {
        interactive_summary(peak, unit, throttled)
    } else {
        summary(sensors, unit)
    };
    let summary = format!("{:<74}", truncate(&summary, 74));
    output.push_str(&format!(
        "{}  {}{}\n",
        border("│"),
        paint(&summary, if throttled { BRIGHT_RED } else { DIM }, color),
        border("│")
    ));
    output.push_str(&border(
        "╰────────────────────────────────────────────────────────────────────────────╯",
    ));
    output
}

fn render_sensor(
    sensor: &Sensor,
    unit: Unit,
    color: bool,
    previous: Option<&HashMap<PathBuf, f64>>,
) -> String {
    let state = classify(sensor);
    let indicator = paint("●", state.color(), color);
    let (temperature, suffix) = convert(sensor.temperature_c, unit);
    let state_label = paint(&format!("{:<8}", state.label()), state.color(), color);
    let border = |text: &str| paint(text, CYAN, color);

    if let Some(previous) = previous {
        let label = truncate(&sensor.label, 23);
        let bar = temperature_bar(sensor.temperature_c, sensor.critical_c, 16);
        let colored_bar = paint(&bar, state.color(), color);
        let trend = temperature_trend(
            previous
                .get(&sensor.source)
                .map(|value| sensor.temperature_c - value),
            unit,
            color,
        );
        return format!(
            "{}  {}  {:<23} {}  {:>6.1} {}  {}  {}  {}",
            border("│"),
            indicator,
            label,
            colored_bar,
            temperature,
            suffix,
            trend,
            state_label,
            border("│")
        );
    }

    let label = truncate(&sensor.label, 29);
    let bar = temperature_bar(sensor.temperature_c, sensor.critical_c, 18);
    let colored_bar = paint(&bar, state.color(), color);
    format!(
        "{}  {}  {:<29} {}  {:>6.1} {}  {}  {}",
        border("│"),
        indicator,
        label,
        colored_bar,
        temperature,
        suffix,
        state_label,
        border("│")
    )
}

fn temperature_trend(delta_c: Option<f64>, unit: Unit, color: bool) -> String {
    let Some(delta_c) = delta_c else {
        return paint("·   --", DIM, color);
    };
    let delta = match unit {
        Unit::Celsius => delta_c,
        Unit::Fahrenheit => delta_c * 9.0 / 5.0,
    }
    .clamp(-99.9, 99.9);

    if delta.abs() < 0.05 {
        paint("→  0.0", DIM, color)
    } else if delta > 0.0 {
        paint(&format!("↑{delta:+5.1}"), RED, color)
    } else {
        paint(&format!("↓{delta:+5.1}"), CYAN, color)
    }
}

pub(crate) fn classify(sensor: &Sensor) -> TemperatureState {
    if sensor.alarm || sensor.temperature_c >= threshold_for(sensor, TemperatureState::Critical) {
        TemperatureState::Critical
    } else if sensor.temperature_c >= threshold_for(sensor, TemperatureState::Hot) {
        TemperatureState::Hot
    } else if sensor.temperature_c >= threshold_for(sensor, TemperatureState::Warm) {
        TemperatureState::Warm
    } else {
        TemperatureState::Normal
    }
}

pub(crate) fn threshold_for(sensor: &Sensor, state: TemperatureState) -> f64 {
    let critical = sensor.critical_c.unwrap_or(match sensor.kind {
        SensorKind::Cpu => 105.0,
        SensorKind::Storage => 90.0,
        SensorKind::Other => 95.0,
    });
    let default_hot = match sensor.kind {
        SensorKind::Cpu => 85.0,
        SensorKind::Storage => 75.0,
        SensorKind::Other => 80.0,
    };
    // Some drivers expose the emergency limit as both max and crit. Such a
    // value is not a useful early-warning threshold.
    let hot = sensor
        .high_c
        .filter(|high| *high < critical - 1.0)
        .unwrap_or(default_hot)
        .min(critical - 5.0);
    let default_warm = match sensor.kind {
        SensorKind::Cpu => 70.0,
        SensorKind::Storage => 60.0,
        SensorKind::Other => (hot - 15.0).max(50.0),
    };
    match state {
        TemperatureState::Normal => f64::NEG_INFINITY,
        TemperatureState::Warm => default_warm.min(hot - 5.0),
        TemperatureState::Hot => hot,
        TemperatureState::Critical => critical,
    }
}

fn temperature_bar(temperature_c: f64, critical_c: Option<f64>, width: usize) -> String {
    let ceiling = critical_c.unwrap_or(100.0).max(40.0);
    let ratio = ((temperature_c - 20.0) / (ceiling - 20.0)).clamp(0.0, 1.0);
    let filled = (ratio * width as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn convert(temperature_c: f64, unit: Unit) -> (f64, &'static str) {
    match unit {
        Unit::Celsius => (temperature_c, "°C"),
        Unit::Fahrenheit => (temperature_c * 9.0 / 5.0 + 32.0, "°F"),
    }
}

fn interactive_summary(peak: Option<&SessionPeak>, unit: Unit, throttled: bool) -> String {
    let prefix = if throttled { "⚠ THROTTLED  ·  " } else { "" };
    match peak {
        Some(peak) => {
            let (temperature, suffix) = convert(peak.temperature_c, unit);
            format!(
                "{prefix}Session max: {} ({temperature:.1} {suffix})",
                truncate(&peak.label, if throttled { 30 } else { 45 })
            )
        }
        None => format!("{prefix}Waiting for temperature samples"),
    }
}

fn summary(sensors: &[Sensor], unit: Unit) -> String {
    let hottest = sensors
        .iter()
        .max_by(|left, right| left.temperature_c.total_cmp(&right.temperature_c));
    match hottest {
        Some(sensor) => {
            let (temperature, suffix) = convert(sensor.temperature_c, unit);
            format!(
                "{} sensors  ·  Hottest: {} ({temperature:.1} {suffix})",
                sensors.len(),
                truncate(&sensor.label, 34),
            )
        }
        None => "Sensor source: Linux sysfs".to_owned(),
    }
}

fn paint(text: &str, style: &str, enabled: bool) -> String {
    if enabled {
        format!("{style}{text}{RESET}")
    } else {
        text.to_owned()
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    value
        .chars()
        .take(max_chars.saturating_sub(1))
        .chain(std::iter::once('…'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sensor(temperature_c: f64) -> Sensor {
        Sensor {
            label: "CPU Package".to_owned(),
            kind: SensorKind::Cpu,
            temperature_c,
            high_c: Some(80.0),
            critical_c: Some(100.0),
            alarm: false,
            source: PathBuf::from("/sys/test"),
        }
    }

    #[test]
    fn classifies_temperature_against_sensor_thresholds() {
        assert_eq!(classify(&sensor(40.0)), TemperatureState::Normal);
        assert_eq!(classify(&sensor(70.0)), TemperatureState::Warm);
        assert_eq!(classify(&sensor(85.0)), TemperatureState::Hot);
        assert_eq!(classify(&sensor(101.0)), TemperatureState::Critical);
    }

    #[test]
    fn uses_early_warning_thresholds_when_cpu_max_equals_critical() {
        let mut cpu = sensor(69.0);
        cpu.high_c = Some(105.0);
        cpu.critical_c = Some(105.0);

        assert_eq!(classify(&cpu), TemperatureState::Normal);
        cpu.temperature_c = 70.0;
        assert_eq!(classify(&cpu), TemperatureState::Warm);
        cpu.temperature_c = 85.0;
        assert_eq!(classify(&cpu), TemperatureState::Hot);
        cpu.temperature_c = 105.0;
        assert_eq!(classify(&cpu), TemperatureState::Critical);
    }

    #[test]
    fn reports_a_hardware_alarm_as_critical() {
        let mut cpu = sensor(45.0);
        cpu.alarm = true;

        assert_eq!(classify(&cpu), TemperatureState::Critical);
    }

    #[test]
    fn converts_to_fahrenheit() {
        assert_eq!(convert(0.0, Unit::Fahrenheit), (32.0, "°F"));
        assert_eq!(convert(100.0, Unit::Fahrenheit), (212.0, "°F"));
    }

    #[test]
    fn renders_plain_output_without_ansi_sequences() {
        let output = render(&[sensor(52.5)], Unit::Celsius, false, "workstation");

        assert!(output.contains("THERMAL MONITOR"));
        assert!(output.contains("CPU Package"));
        assert!(output.contains("52.5 °C"));
        assert!(output.contains("NORMAL"));
        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn renders_temperature_change_in_interactive_mode() {
        let current = sensor(53.5);
        let mut previous = HashMap::new();
        previous.insert(current.source.clone(), 52.0);

        let peak = SessionPeak {
            label: "CPU Package".to_owned(),
            temperature_c: 87.0,
        };
        let output = render_interactive(
            &[current],
            Unit::Celsius,
            false,
            "workstation",
            &previous,
            Some(&peak),
            true,
        );

        assert!(output.contains("↑ +1.5"));
        assert!(output.contains("THROTTLED"));
        assert!(output.contains("Session max: CPU Package (87.0 °C)"));
        assert!(output.lines().all(|line| line.chars().count() == 78));
    }

    #[test]
    fn converts_interactive_delta_to_fahrenheit() {
        let current = sensor(53.0);
        let mut previous = HashMap::new();
        previous.insert(current.source.clone(), 52.0);

        let output = render_interactive(
            &[current],
            Unit::Fahrenheit,
            false,
            "workstation",
            &previous,
            None,
            false,
        );

        assert!(output.contains("↑ +1.8"));
    }

    #[test]
    fn truncates_long_labels() {
        assert_eq!(truncate("abcdefghijklmnopqrstuvwxyz", 8), "abcdefg…");
    }
}
