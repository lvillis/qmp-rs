//! Event stream wrapper.

use futures_core::Stream;
use std::{
    convert::TryFrom,
    pin::Pin,
    task::{Context, Poll},
};

use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};

use crate::{
    error::{Error, Result},
    types::Event,
};

/// A clonable subscription stream of QMP events.
///
/// This type implements [`Stream`], but it does **not** expose Tokio types in
/// the public API.
#[derive(Debug)]
pub struct EventStream {
    inner: BroadcastStream<Event>,
}

fn clamp_missed(missed: u64) -> usize {
    usize::try_from(missed).unwrap_or(usize::MAX)
}

impl EventStream {
    pub(crate) fn new(rx: tokio::sync::broadcast::Receiver<Event>) -> Self {
        Self {
            inner: BroadcastStream::new(rx),
        }
    }

    /// Receive the next event.
    ///
    /// This is a convenience over the `Stream` interface when you prefer a
    /// simple `await`.
    pub async fn recv(&mut self) -> Result<Event> {
        use tokio_stream::StreamExt;

        match self.inner.next().await {
            Some(Ok(ev)) => Ok(ev),
            Some(Err(BroadcastStreamRecvError::Lagged(missed))) => Err(Error::EventLagged {
                missed: clamp_missed(missed),
            }),
            None => Err(Error::Disconnected),
        }
    }
}

impl Stream for EventStream {
    type Item = Result<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(ev))) => Poll::Ready(Some(Ok(ev))),
            Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(missed)))) => {
                Poll::Ready(Some(Err(Error::EventLagged {
                    missed: clamp_missed(missed),
                })))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
