use core::fmt;

use crossterm::style::Stylize as _;
use time::{
    UtcDateTime, UtcOffset, format_description::BorrowedFormatItem, macros::format_description,
};

const DEFAULT_TIME_FMT: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");

pub struct DateTimeFmt {
    pub timestamp: UtcDateTime,
    pub offset: UtcOffset,
    pub timeago: Option<TimeAgo>,
    pub format: &'static [BorrowedFormatItem<'static>],
    pub color: bool,
}

pub enum TimeAgo {
    Now,
    From(UtcDateTime),
}

impl DateTimeFmt {
    #[must_use]
    pub fn new(timestamp: UtcDateTime) -> Self {
        Self {
            timestamp,
            offset: UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC),
            timeago: Some(TimeAgo::Now),
            format: DEFAULT_TIME_FMT,
            color: true,
        }
    }

    #[must_use]
    pub fn with_offset(self, offset: UtcOffset) -> Self {
        Self { offset, ..self }
    }

    #[must_use]
    pub fn with_timeago(self, timeago: Option<TimeAgo>) -> Self {
        Self { timeago, ..self }
    }

    #[must_use]
    pub fn with_time_format(self, format: &'static [BorrowedFormatItem<'static>]) -> Self {
        Self { format, ..self }
    }
}

impl fmt::Display for DateTimeFmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let diff = UtcDateTime::now() - self.timestamp;
        let is_past = diff.is_negative();

        let dur = (UtcDateTime::now() - self.timestamp).unsigned_abs();
        let mut fmt = timeago::Formatter::new();
        if is_past {
            fmt.ago("");
        }

        let ago = fmt.convert(dur);
        let dt = self
            .timestamp
            .to_offset(self.offset)
            .format(&self.format)
            .unwrap_or_else(|_| String::new());

        if self.color {
            write!(f, "{ago} ({})", dt.italic())
        } else {
            write!(f, "{ago} ({dt})")
        }
    }
}
