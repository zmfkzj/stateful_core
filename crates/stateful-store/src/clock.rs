use std::sync::{Arc, Mutex};

use time::OffsetDateTime;

pub trait Clock: std::fmt::Debug + Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

#[derive(Debug)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

#[derive(Debug, Clone)]
pub struct FixedClock {
    value: Arc<Mutex<OffsetDateTime>>,
}

impl FixedClock {
    pub fn new(value: &str) -> Self {
        Self {
            value: Arc::new(Mutex::new(parse_time(value))),
        }
    }

    pub fn set(&self, value: &str) {
        *self.value.lock().expect("clock mutex should lock") = parse_time(value);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        *self.value.lock().expect("clock mutex should lock")
    }
}

pub(crate) fn format_time(value: OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .expect("time should format")
}

fn parse_time(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .expect("fixed clock value should parse")
}
