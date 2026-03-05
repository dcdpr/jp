use core::fmt;

use chrono::{DateTime, FixedOffset, Local, Utc};
use crossterm::style::Stylize as _;

const DEFAULT_TIME_FMT: &str = "%Y-%m-%d %H:%M:%S";

pub struct DateTimeFmt {
    pub timestamp: DateTime<Utc>,
    pub offset: FixedOffset,
    pub format: &'static str,
    pub color: bool,
}

impl DateTimeFmt {
    #[must_use]
    pub fn new(timestamp: DateTime<Utc>) -> Self {
        Self {
            timestamp,
            offset: *Local::now().offset(),
            format: DEFAULT_TIME_FMT,
            color: true,
        }
    }
}

impl fmt::Display for DateTimeFmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let diff = Utc::now() - self.timestamp;
        let is_past = diff.num_seconds() < 0;

        let dur = (Utc::now() - self.timestamp)
            .abs()
            .to_std()
            .unwrap_or_default();
        let mut fmt = timeago::Formatter::new();
        if is_past {
            fmt.ago("");
        }

        let ago = fmt.convert(dur);
        let dt = self
            .timestamp
            .with_timezone(&self.offset)
            .format(self.format);

        if self.color {
            write!(f, "{ago} ({})", dt.to_string().italic())
        } else {
            write!(f, "{ago} ({dt})")
        }
    }
}
