use std::time::{Duration, Instant};

use clap::ValueEnum;
use push_notify::{DeliveryReport, Dispatcher, Notification};

use crate::display::{TemperatureState, classify};
use crate::sensors::Sensor;

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

pub(crate) struct AlertTracker {
    minimum: TemperatureState,
    cooldown: Duration,
    last_attempt: Option<Instant>,
}

impl AlertTracker {
    pub(crate) fn new(minimum: NotifyLevel, cooldown: Duration) -> Self {
        Self {
            minimum: minimum.state(),
            cooldown,
            last_attempt: None,
        }
    }

    pub(crate) fn send_if_due(
        &mut self,
        dispatcher: &Dispatcher,
        sensors: &[Sensor],
        hostname: &str,
    ) -> Option<DeliveryReport> {
        self.send_if_due_at(dispatcher, sensors, hostname, Instant::now())
    }

    fn send_if_due_at(
        &mut self,
        dispatcher: &Dispatcher,
        sensors: &[Sensor],
        hostname: &str,
        now: Instant,
    ) -> Option<DeliveryReport> {
        let alerting: Vec<&Sensor> = sensors
            .iter()
            .filter(|sensor| classify(sensor) >= self.minimum)
            .collect();
        if alerting.is_empty()
            || self
                .last_attempt
                .is_some_and(|last| now.saturating_duration_since(last) < self.cooldown)
        {
            return None;
        }

        self.last_attempt = Some(now);
        let notification = build_notification(&alerting, hostname);
        Some(dispatcher.send(&notification))
    }
}

pub(crate) fn test_notification(hostname: &str) -> Notification {
    Notification::new(
        format!("Thermal notification test: {}", plain_text(hostname, 40)),
        format!(
            "Test notification from thermal-cli.\n\nHost: `{}`",
            markdown_text(hostname, 80)
        ),
    )
}

fn build_notification(sensors: &[&Sensor], hostname: &str) -> Notification {
    let title = format!("Thermal alert: {}", plain_text(hostname, 40));
    let mut body = format!(
        "Hardware temperature threshold reached.\n\nHost: `{}`\n\n| Sensor | Temperature | State |\n| --- | ---: | --- |\n",
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
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    struct RecordingChannel(Arc<Mutex<Vec<Notification>>>);

    impl Channel for RecordingChannel {
        fn name(&self) -> &str {
            "test"
        }

        fn send(&self, notification: &Notification) -> Result<(), DeliveryError> {
            self.0.lock().unwrap().push(notification.clone());
            Ok(())
        }
    }

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
    fn sends_only_at_or_above_requested_level() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let mut dispatcher = Dispatcher::new();
        dispatcher.add(RecordingChannel(Arc::clone(&received)));
        let mut tracker = AlertTracker::new(NotifyLevel::Hot, Duration::from_secs(60));

        assert!(
            tracker
                .send_if_due(&dispatcher, &[sensor(70.0)], "host")
                .is_none()
        );
        assert!(
            tracker
                .send_if_due(&dispatcher, &[sensor(85.0)], "host")
                .is_some()
        );
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[test]
    fn applies_cooldown_between_attempts() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let mut dispatcher = Dispatcher::new();
        dispatcher.add(RecordingChannel(Arc::clone(&received)));
        let mut tracker = AlertTracker::new(NotifyLevel::Critical, Duration::from_secs(60));
        let start = Instant::now();

        assert!(
            tracker
                .send_if_due_at(&dispatcher, &[sensor(101.0)], "host", start)
                .is_some()
        );
        assert!(
            tracker
                .send_if_due_at(
                    &dispatcher,
                    &[sensor(101.0)],
                    "host",
                    start + Duration::from_secs(59)
                )
                .is_none()
        );
        assert!(
            tracker
                .send_if_due_at(
                    &dispatcher,
                    &[sensor(101.0)],
                    "host",
                    start + Duration::from_secs(60)
                )
                .is_some()
        );
        assert_eq!(received.lock().unwrap().len(), 2);
    }

    #[test]
    fn escapes_markdown_table_content() {
        let notification = build_notification(&[&sensor(101.0)], "host|`name");

        assert!(notification.body().contains("host\\|'name"));
        assert!(
            notification
                .body()
                .contains("| CPU Package | 101.0 °C | CRITICAL |")
        );
    }
}
