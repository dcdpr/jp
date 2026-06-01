use std::time::Duration;

use futures::{StreamExt as _, stream};

use super::with_idle_timeout;
use crate::{StreamError, StreamErrorKind, event::Event};

#[tokio::test(start_paused = true)]
async fn idle_timeout_fires_on_silent_stream() {
    let inner = stream::pending::<Result<Event, StreamError>>().boxed();
    let mut wrapped = with_idle_timeout(inner, Duration::from_secs(5));

    let err = wrapped
        .next()
        .await
        .expect("an item before the stream ends")
        .expect_err("a timeout error");
    assert_eq!(err.kind, StreamErrorKind::Timeout);

    assert!(
        wrapped.next().await.is_none(),
        "stream ends after the idle timeout fires"
    );
}

#[tokio::test(start_paused = true)]
async fn active_stream_passes_through_without_timeout() {
    let inner = stream::iter(vec![Ok(Event::flush(0)), Ok(Event::flush(1))]).boxed();
    let mut wrapped = with_idle_timeout(inner, Duration::from_secs(5));

    assert!(wrapped.next().await.expect("first item").is_ok());
    assert!(wrapped.next().await.expect("second item").is_ok());
    assert!(wrapped.next().await.is_none(), "inner stream exhausted");
}
