//! An unbounded set of streams

use std::fmt;

/// An unbounded set of streams
///
/// This "combinator" provides the ability to maintain a set of streams
/// and drive them all to completion.
///
/// Streams are pushed into this set and their realized values are
/// yielded as they become ready. Streams will only be polled when they
/// generate notifications. This allows to coordinate a large number of streams.
///
/// Note that you can create a ready-made `SelectAll` via the
/// `select_all` function in the `stream` module, or you can start with an
/// empty set with the `SelectAll::new` constructor.
#[must_use = "streams do nothing unless polled"]
pub struct SelectAll<S> {
    inner: futures::stream::FuturesUnordered<futures::stream::StreamFuture<S>>,
}

impl<S: fmt::Debug> fmt::Debug for SelectAll<S> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "SelectAll {{ ... }}")
    }
}

impl<S: futures::stream::Stream> SelectAll<S> {
    /// Constructs a new, empty `SelectAll`
    ///
    /// The returned `SelectAll` does not contain any streams and, in this
    /// state, `SelectAll::poll` will return `Ok(Async::Ready(None))`.
    pub fn new() -> Self {
        Self {
            inner: futures::stream::FuturesUnordered::new(),
        }
    }

    /// Returns the number of streams contained in the set.
    ///
    /// This represents the total number of in-flight streams.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the set contains no streams
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Push a stream into the set.
    ///
    /// This function submits the given stream to the set for managing. This
    /// function will not call `poll` on the submitted stream. The caller must
    /// ensure that `SelectAll::poll` is called in order to receive task
    /// notifications.
    pub fn push(&mut self, stream: S) {
        self.inner.push(stream.into_future());
    }
}

impl<S: futures::stream::Stream> futures::stream::Stream for SelectAll<S> {
    type Item = S::Item;
    type Error = S::Error;

    fn poll(&mut self) -> futures::Poll<Option<Self::Item>, Self::Error> {
        match self.inner.poll().map_err(|(err, _)| err)? {
            futures::Async::NotReady => Ok(futures::Async::NotReady),
            futures::Async::Ready(Some((Some(item), remaining))) => {
                self.push(remaining);
                Ok(futures::Async::Ready(Some(item)))
            }
            futures::Async::Ready(_) => Ok(futures::Async::Ready(None)),
        }
    }
}

/// Convert a list of streams into a `Stream` of results from the streams.
///
/// This essentially takes a list of streams (e.g. a vector, an iterator, etc.)
/// and bundles them together into a single stream.
/// The stream will yield items as they become available on the underlying
/// streams internally, in the order they become available.
///
/// Note that the returned set can also be used to dynamically push more
/// futures into the set as they become available.
pub fn select_all<I>(streams: I) -> SelectAll<I::Item>
where
    I: IntoIterator,
    I::Item: futures::stream::Stream,
{
    let mut set = SelectAll::new();

    for stream in streams {
        set.push(stream);
    }

    set
}

pub fn blocking_iter_to_stream<A>(
    mut iter: impl Iterator<Item = A> + Send + 'static,
) -> impl futures::stream::Stream<Item = A, Error = tokio_threadpool::BlockingError>
where
    A: Send + fmt::Debug + 'static,
{
    futures::sync::mpsc::spawn(
        futures::stream::poll_fn(move || {
            tokio_threadpool::blocking(|| {
                debug!("awaiting next element");
                let item = iter.next();
                debug!("read next element: {:?}", item);
                item
            })
        }),
        &tokio::executor::DefaultExecutor::current(),
        0,
    )
}
