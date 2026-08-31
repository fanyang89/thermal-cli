use std::collections::HashMap;
use std::path::PathBuf;

use crate::sensors::Sensor;

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
    render_dashboard(sensors, unit, color, hostname, None)
}

pub(crate) fn render_interactive(
    sensors: &[Sensor],
    unit: Unit,
    color: bool,
    hostname: &str,
    previous: &HashMap<PathBuf, f64>,
) -> String {
    render_dashboard(sensors, unit, color, hostname, Some(previous))
}

fn render_dashboard(
    sensors: &[Sensor],
    unit: Unit,
    color: bool,
    hostname: &str,
    previous: Option<&HashMap<PathBuf, f64>>,
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
    let summary = summary(sensors, unit);
    output.push_str(&format!(
        "{}  {}{}\n",
        border("│"),
        paint(&format!("{summary:<74}"), DIM, color),
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
    if sensor.temperature_c >= threshold_for(sensor, TemperatureState::Critical) {
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
    let critical = sensor.critical_c.unwrap_or(95.0);
    let hot = sensor.high_c.unwrap_or(80.0).min(critical - 1.0);
    match state {
        TemperatureState::Normal => f64::NEG_INFINITY,
        TemperatureState::Warm => (hot - 15.0).max(50.0),
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
            temperature_c,
            high_c: Some(80.0),
            critical_c: Some(100.0),
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

        let output = render_interactive(&[current], Unit::Celsius, false, "workstation", &previous);

        assert!(output.contains("↑ +1.5"));
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
        );

        assert!(output.contains("↑ +1.8"));
    }

    #[test]
    fn truncates_long_labels() {
        assert_eq!(truncate("abcdefghijklmnopqrstuvwxyz", 8), "abcdefg…");
    }
}
