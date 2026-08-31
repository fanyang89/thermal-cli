use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::ValueEnum;
use push_notify::{DeliveryReport, Dispatcher, Notification};

use crate::display::{TemperatureState, classify, threshold_for};
use crate::sensors::{Sensor, SensorKind};

const RESOLVE_HYSTERESIS_C: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum NotifyLevel {
    Warm,
    Hot,
    Critical,
}

impl NotifyLevel {
    fn state(self) -> TemperatureState {
        match self {
            Self::Warm => TemperatureState::Warm,
            Self::Hot => TemperatureState::Hot,
            Self::Critical => TemperatureState::Critical,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlertKind {
    Overheated,
    Resolved,
}

pub(crate) struct AlertDelivery {
    pub(crate) kind: AlertKind,
    pub(crate) sensor_labels: Vec<String>,
    pub(crate) report: DeliveryReport,
}

/// Tracks per-component alert state.
///
/// A component becomes active only after at least one channel accepts its
/// overheat notification. It remains active until a resolved notification is
/// delivered after the component cools below the configured alert level.
pub(crate) struct AlertTracker {
    minimum: TemperatureState,
    retry_interval: Duration,
    active: HashMap<PathBuf, Sensor>,
    retry_not_before: Option<Instant>,
}

impl AlertTracker {
    pub(crate) fn new(minimum: NotifyLevel, retry_interval: Duration) -> Self {
        Self {
            minimum: minimum.state(),
            retry_interval,
            active: HashMap::new(),
            retry_not_before: None,
        }
    }

    pub(crate) fn process(
        &mut self,
        dispatcher: &Dispatcher,
        sensors: &[Sensor],
        hostname: &str,
        throttled: bool,
    ) -> Vec<AlertDelivery> {
        self.process_at(dispatcher, sensors, hostname, Instant::now(), throttled)
    }

    fn process_at(
        &mut self,
        dispatcher: &Dispatcher,
        sensors: &[Sensor],
        hostname: &str,
        now: Instant,
        throttled: bool,
    ) -> Vec<AlertDelivery> {
        if self.retry_not_before.is_some_and(|retry_at| now < retry_at) {
            return Vec::new();
        }

        let current: HashMap<&PathBuf, &Sensor> = sensors
            .iter()
            .map(|sensor| (&sensor.source, sensor))
            .collect();
        let mut overheated: Vec<Sensor> = sensors
            .iter()
            .filter(|sensor| {
                classify(sensor) >= self.minimum && !self.active.contains_key(&sensor.source)
            })
            .cloned()
            .collect();
        let has_cpu_alert = overheated
            .iter()
            .any(|sensor| sensor.kind == SensorKind::Cpu)
            || self
                .active
                .values()
                .any(|sensor| sensor.kind == SensorKind::Cpu);
        if throttled
            && !has_cpu_alert
            && let Some(sensor) = sensors
                .iter()
                .filter(|sensor| {
                    sensor.kind == SensorKind::Cpu && !self.active.contains_key(&sensor.source)
                })
                .max_by(|left, right| left.temperature_c.total_cmp(&right.temperature_c))
        {
            overheated.push(sensor.clone());
        }
        let mut resolved: Vec<Sensor> = self
            .active
            .keys()
            .filter_map(|source| current.get(source).copied())
            .filter(|sensor| {
                !sensor.alarm
                    && !(throttled && sensor.kind == SensorKind::Cpu)
                    && sensor.temperature_c
                        <= threshold_for(sensor, self.minimum) - RESOLVE_HYSTERESIS_C
            })
            .cloned()
            .collect();
        resolved.sort_by(|left, right| left.label.cmp(&right.label));

        let mut deliveries = Vec::new();
        let mut failed = false;

        if !overheated.is_empty() {
            let report = dispatcher.send(&build_notification(
                AlertKind::Overheated,
                &overheated,
                hostname,
                throttled,
            ));
            if report.delivered.is_empty() {
                failed = true;
            } else {
                for sensor in &overheated {
                    self.active.insert(sensor.source.clone(), sensor.clone());
                }
            }
            deliveries.push(AlertDelivery {
                kind: AlertKind::Overheated,
                sensor_labels: labels(&overheated),
                report,
            });
        }

        if !resolved.is_empty() {
            let report = dispatcher.send(&build_notification(
                AlertKind::Resolved,
                &resolved,
                hostname,
                false,
            ));
            if report.delivered.is_empty() {
                failed = true;
            } else {
                for sensor in &resolved {
                    self.active.remove(&sensor.source);
                }
            }
            deliveries.push(AlertDelivery {
                kind: AlertKind::Resolved,
                sensor_labels: labels(&resolved),
                report,
            });
        }

        self.retry_not_before = failed.then_some(now + self.retry_interval);
        deliveries
    }
}

pub(crate) fn test_notification(hostname: &str) -> Notification {
    Notification::new(
        format!("Thermal notification test: {}", plain_text(hostname, 40)),
        format!(
            "Test notification from thermal-cli daemon mode.\n\nHost: `{}`",
            markdown_text(hostname, 80)
        ),
    )
}

fn build_notification(
    kind: AlertKind,
    sensors: &[Sensor],
    hostname: &str,
    throttled: bool,
) -> Notification {
    let (title_prefix, introduction) = match (kind, throttled) {
        (AlertKind::Overheated, true) => {
            ("Thermal throttle", "CPU thermal throttling was detected.")
        }
        (AlertKind::Overheated, false) => (
            "Thermal alert",
            "One or more hardware components are overheating.",
        ),
        (AlertKind::Resolved, _) => (
            "Thermal resolved",
            "Hardware temperatures returned below the alert threshold.",
        ),
    };
    let title = format!("{title_prefix}: {}", plain_text(hostname, 40));
    let mut body = format!(
        "{introduction}\n\nHost: `{}`\n\n| Sensor | Temperature | State |\n| --- | ---: | --- |\n",
        markdown_text(hostname, 80)
    );
    for sensor in sensors {
        body.push_str(&format!(
            "| {} | {:.1} °C | {} |\n",
            markdown_text(&sensor.label, 60),
            sensor.temperature_c,
            classify(sensor).label()
        ));
    }
    Notification::new(title, body)
}

fn labels(sensors: &[Sensor]) -> Vec<String> {
    sensors.iter().map(|sensor| sensor.label.clone()).collect()
}

fn plain_text(value: &str, max_chars: usize) -> String {
    value
        .replace(['\r', '\n'], " ")
        .chars()
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn markdown_text(value: &str, max_chars: usize) -> String {
    plain_text(value, max_chars)
        .replace('|', "\\|")
        .replace('`', "'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use push_notify::{Channel, DeliveryError};
    use std::sync::{Arc, Mutex};

    struct RecordingChannel {
        received: Arc<Mutex<Vec<Notification>>>,
        fail: bool,
    }

    impl Channel for RecordingChannel {
        fn name(&self) -> &str {
            "test"
        }

        fn send(&self, notification: &Notification) -> Result<(), DeliveryError> {
            self.received.lock().unwrap().push(notification.clone());
            if self.fail {
                Err(DeliveryError::new("test", "unavailable"))
            } else {
                Ok(())
            }
        }
    }

    fn dispatcher(received: &Arc<Mutex<Vec<Notification>>>, fail: bool) -> Dispatcher {
        let mut dispatcher = Dispatcher::new();
        dispatcher.add(RecordingChannel {
            received: Arc::clone(received),
            fail,
        });
        dispatcher
    }

    fn sensor(temperature_c: f64) -> Sensor {
        Sensor {
            label: "CPU Package".to_owned(),
            kind: SensorKind::Cpu,
            temperature_c,
            high_c: Some(80.0),
            critical_c: Some(100.0),
            alarm: false,
            frequency_mhz: Some(3_600.0),
            source: PathBuf::from("/sys/test"),
        }
    }

    #[test]
    fn sends_overheat_then_resolved_for_a_component() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = dispatcher(&received, false);
        let mut tracker = AlertTracker::new(NotifyLevel::Hot, Duration::from_secs(60));
        let start = Instant::now();

        let overheat = tracker.process_at(&dispatcher, &[sensor(85.0)], "host", start, false);
        let unchanged = tracker.process_at(
            &dispatcher,
            &[sensor(90.0)],
            "host",
            start + Duration::from_secs(1),
            false,
        );
        let within_hysteresis = tracker.process_at(
            &dispatcher,
            &[sensor(78.0)],
            "host",
            start + Duration::from_secs(2),
            false,
        );
        let resolved = tracker.process_at(
            &dispatcher,
            &[sensor(74.0)],
            "host",
            start + Duration::from_secs(3),
            false,
        );

        assert_eq!(overheat.len(), 1);
        assert_eq!(overheat[0].kind, AlertKind::Overheated);
        assert!(unchanged.is_empty());
        assert!(within_hysteresis.is_empty());
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].kind, AlertKind::Resolved);
        let notifications = received.lock().unwrap();
        assert_eq!(notifications.len(), 2);
        assert!(notifications[0].title().starts_with("Thermal alert"));
        assert!(notifications[1].title().starts_with("Thermal resolved"));
    }

    #[test]
    fn does_not_resolve_a_sensor_that_disappears() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = dispatcher(&received, false);
        let mut tracker = AlertTracker::new(NotifyLevel::Hot, Duration::from_secs(60));
        let start = Instant::now();

        tracker.process_at(&dispatcher, &[sensor(85.0)], "host", start, false);
        let deliveries = tracker.process_at(
            &dispatcher,
            &[],
            "host",
            start + Duration::from_secs(1),
            false,
        );

        assert!(deliveries.is_empty());
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[test]
    fn retries_failed_alerts_after_retry_interval() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = dispatcher(&received, true);
        let mut tracker = AlertTracker::new(NotifyLevel::Hot, Duration::from_secs(60));
        let start = Instant::now();

        let first = tracker.process_at(&dispatcher, &[sensor(85.0)], "host", start, false);
        let throttled = tracker.process_at(
            &dispatcher,
            &[sensor(85.0)],
            "host",
            start + Duration::from_secs(59),
            false,
        );
        let retry = tracker.process_at(
            &dispatcher,
            &[sensor(85.0)],
            "host",
            start + Duration::from_secs(60),
            false,
        );

        assert_eq!(first.len(), 1);
        assert!(throttled.is_empty());
        assert_eq!(retry.len(), 1);
        assert_eq!(received.lock().unwrap().len(), 2);
    }

    #[test]
    fn sends_alert_when_cpu_throttling_counter_increases() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = dispatcher(&received, false);
        let mut tracker = AlertTracker::new(NotifyLevel::Hot, Duration::from_secs(60));
        let start = Instant::now();

        let alert = tracker.process_at(&dispatcher, &[sensor(45.0)], "host", start, true);
        let resolved = tracker.process_at(
            &dispatcher,
            &[sensor(45.0)],
            "host",
            start + Duration::from_secs(1),
            false,
        );

        assert_eq!(alert.len(), 1);
        assert_eq!(resolved.len(), 1);
        let notifications = received.lock().unwrap();
        assert!(notifications[0].title().starts_with("Thermal throttle"));
        assert!(notifications[0].body().contains("thermal throttling"));
        assert!(notifications[1].title().starts_with("Thermal resolved"));
    }

    #[test]
    fn ignores_temperatures_below_requested_level() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = dispatcher(&received, false);
        let mut tracker = AlertTracker::new(NotifyLevel::Hot, Duration::from_secs(60));

        assert!(
            tracker
                .process(&dispatcher, &[sensor(70.0)], "host", false)
                .is_empty()
        );
        assert!(received.lock().unwrap().is_empty());
    }

    #[test]
    fn escapes_markdown_table_content() {
        let mut value = sensor(101.0);
        value.label = "CPU|`Package".to_owned();
        let notification = build_notification(AlertKind::Overheated, &[value], "host|`name", false);

        assert!(notification.body().contains("host\\|'name"));
        assert!(
            notification
                .body()
                .contains("| CPU\\|'Package | 101.0 °C | CRITICAL |")
        );
    }
}
