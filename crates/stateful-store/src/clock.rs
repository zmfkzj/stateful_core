use std::{sync::{Arc, Mutex}, time::Duration as StdDuration};
use time::{Duration, OffsetDateTime};

pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

#[derive(Debug, Clone)]
pub struct FixedClock {
    now: OffsetDateTime,
}

impl FixedClock {
    pub const fn new(now: OffsetDateTime) -> Self {
        Self { now }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.now
    }
}

#[derive(Debug, Clone)]
pub struct MutableClock {
    now: Arc<Mutex<OffsetDateTime>>,
}

impl MutableClock {
    pub fn from_system_now() -> Self {
        Self { now: Arc::new(Mutex::new(OffsetDateTime::now_utc())) }
    }

    pub fn advance(&self, duration: StdDuration) {
        let duration = Duration::try_from(duration).expect("test clock duration must fit time::Duration");
        *self.now.lock().expect("test clock lock") += duration;
    }
}

impl Clock for MutableClock {
    fn now(&self) -> OffsetDateTime {
        *self.now.lock().expect("test clock lock")
    }
}

pub(crate) type SharedClock = Arc<dyn Clock>;
