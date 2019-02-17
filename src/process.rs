use std::io;
use std::process;

pub struct Process {
    pub index: usize,
    pub stdin: Option<Stdin>,
    pub stdout: Option<Stdout>,
    pub stderr: Option<Stderr>,
    pub child: tokio_process::Child,
}

pub struct Stdin {
    is_child_closed: bool,
    sink: tokio::codec::FramedWrite<tokio_process::ChildStdin, tokio::codec::BytesCodec>,
}

pub struct Stdout(tokio::codec::FramedRead<tokio_process::ChildStdout, tokio::codec::BytesCodec>);
pub struct Stderr(tokio::codec::FramedRead<tokio_process::ChildStderr, tokio::codec::BytesCodec>);

impl Process {
    pub fn spawn(index: usize, command: String, args: Vec<String>) -> Result<Self, failure::Error> {
        use tokio_process::CommandExt;

        let mut child = process::Command::new(command)
            .args(args)
            .stdin(process::Stdio::piped())
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .spawn_async()?;

        let stdin = child.stdin().take().unwrap();
        let stdout = child.stdout().take().unwrap();
        let stderr = child.stderr().take().unwrap();

        let stdin = Some(Stdin::new(tokio::codec::FramedWrite::new(
            stdin,
            tokio::codec::BytesCodec::new(),
        )));
        let stdout = Some(Stdout(tokio::codec::FramedRead::new(
            stdout,
            tokio::codec::BytesCodec::new(),
        )));
        let stderr = Some(Stderr(tokio::codec::FramedRead::new(
            stderr,
            tokio::codec::BytesCodec::new(),
        )));

        Ok(Self {
            index,
            stdin,
            stdout,
            stderr,
            child,
        })
    }
}

impl Stdin {
    fn new(
        sink: tokio::codec::FramedWrite<tokio_process::ChildStdin, tokio::codec::BytesCodec>,
    ) -> Self {
        let is_child_closed = false;

        Self {
            sink,
            is_child_closed,
        }
    }
}

impl futures::sink::Sink for Stdin {
    type SinkItem = bytes::Bytes;
    type SinkError = failure::Error;

    fn start_send(
        &mut self,
        item: Self::SinkItem,
    ) -> Result<futures::AsyncSink<Self::SinkItem>, Self::SinkError> {
        if self.is_child_closed {
            Ok(futures::AsyncSink::Ready)
        } else {
            self.sink.start_send(item).or_else(|error| {
                if error.kind() == io::ErrorKind::BrokenPipe {
                    self.is_child_closed = true;
                    Ok(futures::AsyncSink::Ready)
                } else {
                    Err(failure::Error::from(error))
                }
            })
        }
    }

    fn poll_complete(&mut self) -> Result<futures::Async<()>, Self::SinkError> {
        if self.is_child_closed {
            Ok(futures::Async::Ready(()))
        } else {
            self.sink.poll_complete().or_else(|error| {
                if error.kind() == io::ErrorKind::BrokenPipe {
                    self.is_child_closed = true;
                    Ok(futures::Async::Ready(()))
                } else {
                    Err(failure::Error::from(error))
                }
            })
        }
    }

    fn close(&mut self) -> Result<futures::Async<()>, Self::SinkError> {
        self.sink.close().map_err(failure::Error::from)
    }
}

impl futures::stream::Stream for Stdout {
    type Item = bytes::BytesMut;
    type Error = failure::Error;

    fn poll(&mut self) -> Result<futures::Async<Option<Self::Item>>, Self::Error> {
        self.0.poll().map_err(failure::Error::from)
    }
}

impl futures::stream::Stream for Stderr {
    type Item = bytes::BytesMut;
    type Error = failure::Error;

    fn poll(&mut self) -> Result<futures::Async<Option<Self::Item>>, Self::Error> {
        self.0.poll().map_err(failure::Error::from)
    }
}
