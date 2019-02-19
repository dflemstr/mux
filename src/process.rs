use std::ffi;
use std::io;
use std::process;

pub struct Process {
    pub input: Input,
    pub output: Output,
    pub exit: Exit,
}

pub struct Write {
    pub input: Input,
}

pub struct Read {
    pub output: Output,
    pub exit: Exit,
}

pub struct Input {
    sink: Option<
        tokio::codec::FramedWrite<
            tokio::io::WriteHalf<tokio_pty_process::AsyncPtyMaster>,
            tokio::codec::BytesCodec,
        >,
    >,
}

#[must_use = "streams do nothing unless polled"]
pub struct Output {
    stream: Option<
        tokio::codec::FramedRead<
            tokio::io::ReadHalf<tokio_pty_process::AsyncPtyMaster>,
            tokio::codec::BytesCodec,
        >,
    >,
}

pub struct Exit {
    future: tokio_pty_process::Child,
}

impl Process {
    pub fn spawn(
        command: impl AsRef<ffi::OsStr>,
        args: &[impl AsRef<ffi::OsStr>],
    ) -> Result<Self, failure::Error> {
        use tokio::io::AsyncRead;
        use tokio_pty_process::CommandExt;

        let pty = tokio_pty_process::AsyncPtyMaster::open()?;

        let child = process::Command::new(command)
            .args(args)
            .spawn_pty_async(&pty)?;

        let (output, input) = pty.split();

        let input = Input::new(tokio::codec::FramedWrite::new(
            input,
            tokio::codec::BytesCodec::new(),
        ));
        let output = Output::new(tokio::codec::FramedRead::new(
            output,
            tokio::codec::BytesCodec::new(),
        ));
        let exit = Exit::new(child);

        Ok(Self {
            input,
            output,
            exit,
        })
    }

    pub fn split(self) -> (Write, Read) {
        let Self {
            input,
            output,
            exit,
        } = self;

        (Write { input }, Read { output, exit })
    }
}

impl Input {
    fn new(
        sink: tokio::codec::FramedWrite<
            tokio::io::WriteHalf<tokio_pty_process::AsyncPtyMaster>,
            tokio::codec::BytesCodec,
        >,
    ) -> Self {
        let sink = Some(sink);

        Self { sink }
    }
}

impl Output {
    fn new(
        stream: tokio::codec::FramedRead<
            tokio::io::ReadHalf<tokio_pty_process::AsyncPtyMaster>,
            tokio::codec::BytesCodec,
        >,
    ) -> Self {
        let stream = Some(stream);

        Self { stream }
    }
}

impl Exit {
    fn new(future: tokio_pty_process::Child) -> Self {
        Self { future }
    }
}

impl futures::sink::Sink for Input {
    type SinkItem = bytes::Bytes;
    type SinkError = failure::Error;

    fn start_send(
        &mut self,
        item: Self::SinkItem,
    ) -> Result<futures::AsyncSink<Self::SinkItem>, Self::SinkError> {
        if let Some(ref mut sink) = self.sink {
            sink.start_send(item).or_else(|error| {
                debug!("error in process input start_send: {}", error);
                if error.kind() == io::ErrorKind::BrokenPipe {
                    self.sink = None;
                    Ok(futures::AsyncSink::Ready)
                } else {
                    Err(failure::Error::from(error))
                }
            })
        } else {
            Ok(futures::AsyncSink::Ready)
        }
    }

    fn poll_complete(&mut self) -> Result<futures::Async<()>, Self::SinkError> {
        if let Some(ref mut sink) = self.sink {
            sink.poll_complete().or_else(|error| {
                debug!("error in process input poll_complete: {}", error);
                if error.kind() == io::ErrorKind::BrokenPipe {
                    self.sink = None;
                    Ok(futures::Async::Ready(()))
                } else {
                    Err(failure::Error::from(error))
                }
            })
        } else {
            Ok(futures::Async::Ready(()))
        }
    }

    fn close(&mut self) -> Result<futures::Async<()>, Self::SinkError> {
        if let Some(ref mut sink) = self.sink {
            sink.close().map_err(failure::Error::from)
        } else {
            Ok(futures::Async::Ready(()))
        }
    }
}

impl futures::stream::Stream for Output {
    type Item = bytes::BytesMut;
    type Error = failure::Error;

    fn poll(&mut self) -> Result<futures::Async<Option<Self::Item>>, Self::Error> {
        if let Some(ref mut stream) = self.stream {
            stream.poll().or_else(|error| {
                debug!("error in process output poll: {}", error);
                if error.raw_os_error() == Some(5) {
                    self.stream = None;
                    Ok(futures::Async::Ready(None))
                } else {
                    Err(failure::Error::from(error))
                }
            })
        } else {
            Ok(futures::Async::Ready(None))
        }
    }
}

impl futures::future::Future for Exit {
    type Item = std::process::ExitStatus;
    type Error = failure::Error;

    fn poll(&mut self) -> Result<futures::Async<Self::Item>, Self::Error> {
        self.future.poll_exit().map_err(failure::Error::from)
    }
}
