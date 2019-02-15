use std::process;

pub struct ManagedProcess {
    pub index: usize,
    pub in_tx: Option<tokio::sync::mpsc::UnboundedSender<bytes::Bytes>>,
    pub out_rx: Option<tokio::sync::mpsc::UnboundedReceiver<bytes::BytesMut>>,
    pub err_rx: Option<tokio::sync::mpsc::UnboundedReceiver<bytes::BytesMut>>,
    pub child_process: tokio_process::Child,
}

impl ManagedProcess {
    pub fn spawn(index: usize, command: String, args: Vec<String>) -> Result<Self, failure::Error> {
        use futures::future::Future;
        use futures::sink::Sink;
        use futures::stream::Stream;
        use tokio_process::CommandExt;

        let mut child_process = process::Command::new(command)
            .args(args)
            .stdin(process::Stdio::piped())
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .spawn_async()?;

        let stdin = child_process.stdin().take().unwrap();
        let stdout = child_process.stdout().take().unwrap();
        let stderr = child_process.stderr().take().unwrap();
        let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel();
        let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
        let (err_tx, err_rx) = tokio::sync::mpsc::unbounded_channel();

        let in_rx = in_rx.map_err(move |_| {
            failure::err_msg(format!(
                "failed to read from stdin channel for process {}",
                index
            ))
        });
        let out_tx = out_tx.sink_map_err(move |_| {
            failure::err_msg(format!(
                "failed to write to stdout channel for process {}",
                index
            ))
        });
        let err_tx = err_tx.sink_map_err(move |_| {
            failure::err_msg(format!(
                "failed to write to stderr channel for process {}",
                index
            ))
        });

        let stdin_sink = tokio::codec::FramedWrite::new(stdin, tokio::codec::BytesCodec::new())
            .sink_map_err(failure::Error::from);
        let stdout_source = tokio::codec::FramedRead::new(stdout, tokio::codec::BytesCodec::new())
            .map_err(failure::Error::from);
        let stderr_source = tokio::codec::FramedRead::new(stderr, tokio::codec::BytesCodec::new())
            .map_err(failure::Error::from);

        tokio::spawn(
            in_rx
                .forward(stdin_sink)
                .map_err(move |e| {
                    error!(
                        "failed to forward stdin channel to process {}: {}",
                        index, e
                    )
                })
                .map(move |_| debug!("stopped stdin channel to process {}", index)),
        );
        tokio::spawn(
            stdout_source
                .forward(out_tx)
                .map_err(move |e| {
                    error!(
                        "failed to forward stdout to channel from process {}: {}",
                        index, e
                    )
                })
                .map(move |_| debug!("stopped stdout channel from process {}", index)),
        );
        tokio::spawn(
            stderr_source
                .forward(err_tx)
                .map_err(move |e| {
                    error!(
                        "failed to forward stderr to channel from process {}: {}",
                        index, e
                    )
                })
                .map(move |_| debug!("stopped stderr channel from process {}", index)),
        );

        let in_tx = Some(in_tx);
        let out_rx = Some(out_rx);
        let err_rx = Some(err_rx);

        Ok(Self {
            index,
            in_tx,
            out_rx,
            err_rx,
            child_process,
        })
    }
}
