use std::mem;

pub struct Fanout<S>
where
    S: futures::sink::Sink,
{
    downstreams: Vec<Downstream<S>>,
}

impl<S> Fanout<S>
where
    S: futures::sink::Sink,
{
    pub fn new(sinks: impl IntoIterator<Item = S>) -> Self {
        let downstreams = sinks.into_iter().map(Downstream::new).collect();

        Self { downstreams }
    }
}

impl<S> futures::sink::Sink for Fanout<S>
where
    S: futures::sink::Sink,
    S::SinkItem: Clone,
{
    type SinkItem = S::SinkItem;
    type SinkError = S::SinkError;

    fn start_send(
        &mut self,
        item: Self::SinkItem,
    ) -> Result<futures::AsyncSink<Self::SinkItem>, Self::SinkError> {
        for downstream in &mut self.downstreams {
            downstream.keep_flushing()?;
        }

        if self.downstreams.iter().all(Downstream::is_ready) {
            for downstream in &mut self.downstreams {
                downstream.state = downstream.sink.start_send(item.clone())?;
            }
            Ok(futures::AsyncSink::Ready)
        } else {
            Ok(futures::AsyncSink::NotReady(item))
        }
    }

    fn poll_complete(&mut self) -> Result<futures::Async<()>, Self::SinkError> {
        for downstream in &mut self.downstreams {
            if downstream.poll_complete()?.is_not_ready() {
                return Ok(futures::Async::NotReady);
            }
        }
        Ok(futures::Async::Ready(()))
    }

    fn close(&mut self) -> Result<futures::Async<()>, Self::SinkError> {
        for downstream in &mut self.downstreams {
            if downstream.close()?.is_not_ready() {
                return Ok(futures::Async::NotReady);
            }
        }
        Ok(futures::Async::Ready(()))
    }
}

#[derive(Debug)]
struct Downstream<S>
where
    S: futures::sink::Sink,
{
    sink: S,
    state: futures::AsyncSink<S::SinkItem>,
}

impl<S> Downstream<S>
where
    S: futures::sink::Sink,
{
    fn new(sink: S) -> Self {
        Self {
            sink,
            state: futures::AsyncSink::Ready,
        }
    }

    fn is_ready(&self) -> bool {
        self.state.is_ready()
    }

    fn keep_flushing(&mut self) -> Result<(), S::SinkError> {
        if let futures::AsyncSink::NotReady(item) =
            mem::replace(&mut self.state, futures::AsyncSink::Ready)
        {
            self.state = self.sink.start_send(item)?;
        }
        Ok(())
    }

    fn poll_complete(&mut self) -> futures::Poll<(), S::SinkError> {
        self.keep_flushing()?;
        let async_state = self.sink.poll_complete()?;
        // Only if all values have been sent _and_ the underlying
        // sink is completely flushed, signal readiness.
        if self.state.is_ready() && async_state.is_ready() {
            Ok(futures::Async::Ready(()))
        } else {
            Ok(futures::Async::NotReady)
        }
    }

    fn close(&mut self) -> futures::Poll<(), S::SinkError> {
        self.keep_flushing()?;
        // If all items have been flushed, initiate close.
        if self.state.is_ready() {
            self.sink.close()
        } else {
            Ok(futures::Async::NotReady)
        }
    }
}
