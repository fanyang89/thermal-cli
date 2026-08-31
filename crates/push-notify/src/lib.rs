//! Extensible push notification delivery.
//!
//! Implement [`Channel`] to add another provider, then register it with a
//! [`Dispatcher`]. The built-in [`ServerChan`] channel supports both ServerChan
//! Turbo (`SCT...`) and ServerChan³ (`sctp...`) SendKeys.

mod serverchan;

pub use serverchan::ServerChan;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    title: String,
    body: String,
}

impl Notification {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn body(&self) -> &str {
        &self.body
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("{channel} delivery failed: {message}")]
pub struct DeliveryError {
    channel: String,
    message: String,
}

impl DeliveryError {
    pub fn new(channel: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            message: message.into(),
        }
    }

    pub fn channel(&self) -> &str {
        &self.channel
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A push delivery backend.
///
/// Implementations should not include credentials in errors because callers
/// may print failures to terminal or service logs.
pub trait Channel: Send + Sync {
    fn name(&self) -> &str;
    fn send(&self, notification: &Notification) -> Result<(), DeliveryError>;
}

#[derive(Default)]
pub struct Dispatcher {
    channels: Vec<Box<dyn Channel>>,
}

impl Dispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add<C>(&mut self, channel: C)
    where
        C: Channel + 'static,
    {
        self.channels.push(Box::new(channel));
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// Attempts every registered channel, even if one delivery fails.
    pub fn send(&self, notification: &Notification) -> DeliveryReport {
        let mut report = DeliveryReport::default();
        for channel in &self.channels {
            match channel.send(notification) {
                Ok(()) => report.delivered.push(channel.name().to_owned()),
                Err(error) => report.failures.push(error),
            }
        }
        report
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeliveryReport {
    pub delivered: Vec<String>,
    pub failures: Vec<DeliveryError>,
}

impl DeliveryReport {
    pub fn is_success(&self) -> bool {
        !self.delivered.is_empty() && self.failures.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct RecordingChannel {
        name: &'static str,
        received: Arc<Mutex<Vec<Notification>>>,
        fail: bool,
    }

    impl Channel for RecordingChannel {
        fn name(&self) -> &str {
            self.name
        }

        fn send(&self, notification: &Notification) -> Result<(), DeliveryError> {
            self.received.lock().unwrap().push(notification.clone());
            if self.fail {
                Err(DeliveryError::new(self.name, "unavailable"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn dispatcher_delivers_to_every_channel() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let mut dispatcher = Dispatcher::new();
        dispatcher.add(RecordingChannel {
            name: "primary",
            received: Arc::clone(&received),
            fail: false,
        });
        dispatcher.add(RecordingChannel {
            name: "backup",
            received: Arc::clone(&received),
            fail: true,
        });

        let report = dispatcher.send(&Notification::new("Alert", "CPU is hot"));

        assert_eq!(received.lock().unwrap().len(), 2);
        assert_eq!(report.delivered, ["primary"]);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].channel(), "backup");
    }
}
