use std::sync::Arc;
use time::OffsetDateTime;

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

pub(crate) type SharedClock = Arc<dyn Clock>;
