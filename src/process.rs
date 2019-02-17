use std::io;
use std::process;
use crate::pty;

pub struct Process {
    pub index: usize,
    pub input: Option<Input>,
    pub output: Option<Output>,
    pub child: tokio_process::Child,
}

pub struct Input {
    is_child_closed: bool,
    sink: tokio::codec::FramedWrite<tokio::fs::File, tokio::codec::BytesCodec>,
}

pub struct Output(tokio::codec::FramedRead<tokio::fs::File, tokio::codec::BytesCodec>);

impl Process {
    pub fn spawn(index: usize, command: String, args: Vec<String>) -> Result<Self, failure::Error> {
        use tokio_process::CommandExt;
        use std::os::unix::io::FromRawFd;

        let pty = pty::openpty(80, 24)?;
        let slave_input_file = unsafe { std::fs::File::from_raw_fd(pty.slave) };
        let slave_output_file = slave_input_file.try_clone()?;
        let slave_error_file = slave_input_file.try_clone()?;
        let master_output_file = unsafe { std::fs::File::from_raw_fd(pty.master) };
        let master_input_file = master_output_file.try_clone()?;

        let child = process::Command::new(command)
            .args(args)
            .stdin(process::Stdio::from(slave_input_file))
            .stdout(process::Stdio::from(slave_output_file))
            .stderr(process::Stdio::from(slave_error_file))
            .spawn_async()?;

        let input = tokio::fs::File::from_std(master_input_file);
        let output = tokio::fs::File::from_std(master_output_file);

        let input = Some(Input::new(tokio::codec::FramedWrite::new(
            input,
            tokio::codec::BytesCodec::new(),
        )));
        let output = Some(Output(tokio::codec::FramedRead::new(
            output,
            tokio::codec::BytesCodec::new(),
        )));

        Ok(Self {
            index,
            input,
            output,
            child,
        })
    }
}

impl Input {
    fn new(
        sink: tokio::codec::FramedWrite<tokio::fs::File, tokio::codec::BytesCodec>,
    ) -> Self {
        let is_child_closed = false;

        Self {
            sink,
            is_child_closed,
        }
    }
}

impl futures::sink::Sink for Input {
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

impl futures::stream::Stream for Output {
    type Item = bytes::BytesMut;
    type Error = failure::Error;

    fn poll(&mut self) -> Result<futures::Async<Option<Self::Item>>, Self::Error> {
        self.0.poll().map_err(failure::Error::from)
    }
}
