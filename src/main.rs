#![feature(await_macro, async_await, futures_api)]
#![warn(clippy::all, clippy::pedantic)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate structopt;
#[macro_use]
extern crate tokio;

use std::io;
use std::process;

mod args;
mod options;
mod pty;

fn main() -> Result<(), failure::Error> {
    use structopt::StructOpt;

    pretty_env_logger::init_timed();

    let options = options::Options::from_args();

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on_all(tokio_async_await::compat::backward::Compat::new(run(
        options,
    )))?;
    Ok(())
}

async fn run(options: options::Options) -> Result<(), failure::Error> {
    use futures::future::Future;
    use futures::sink::Sink;
    use futures::stream::Stream;

    let delimiter = parse_delimiter(options.null, options.delimiter);

    let arg_template = parse_arg_template(options.initial_args, &options.replace);

    let raw_args = await!(args::generate(options.arg_file, delimiter))?;

    let args: Vec<Vec<String>> = await!(raw_args
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map(|a| generate_final_args(a, &arg_template))
        .collect())?;

    let command = options.command;

    struct ManagedProcess {
        index: usize,
        input_kill_trigger: stream_cancel::Trigger,
        input_tx: tokio::sync::mpsc::UnboundedSender<bytes::Bytes>,
        child_process: tokio_process::Child,
    }

    let managed_processes = args
        .into_iter()
        .enumerate()
        .map(move |(index, args)| {
            use tokio_process::CommandExt;
            let mut child_process = process::Command::new(command.clone())
                .args(args)
                .stdin(process::Stdio::piped())
                .spawn_async()?;

            let stdin = child_process.stdin().take().unwrap();
            let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();

            let (input_kill_trigger, input_rx) = stream_cancel::Valved::new(input_rx);

            let stdin_sink = tokio::codec::FramedWrite::new(stdin, tokio::codec::BytesCodec::new());

            tokio::spawn(
                input_rx
                    .map_err(move |_| {
                        io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            format!("failed to read from stdin queue for process {}", index),
                        )
                    })
                    .forward(stdin_sink)
                    .map_err(move |e| {
                        error!(
                            "failed to forward stdin broadcast queue to process {}: {}",
                            index, e
                        )
                    })
                    .map(move |_| {
                        debug!(
                            "stopped stdin broadcast queue to process {} forwarder",
                            index
                        )
                    }),
            );

            Ok(ManagedProcess {
                index,
                input_kill_trigger,
                input_tx,
                child_process,
            })
        })
        .collect::<Result<Vec<_>, failure::Error>>()?;

    let (trigger, stdin_source) = stream_cancel::Valved::new(tokio::codec::FramedRead::new(
        await!(tokio::fs::File::open("/dev/tty"))?,
        tokio::codec::BytesCodec::new(),
    ));

    let (process_handles, input_txs) = managed_processes
        .into_iter()
        .map(|p| ((p.index, p.input_kill_trigger, p.child_process), p.input_tx))
        .unzip::<_, _, Vec<_>, Vec<_>>();

    tokio::spawn(
        stdin_source
            .map(|b| b.freeze())
            .fold(input_txs, |input_txs, b| {
                futures::future::join_all(input_txs.into_iter().map(move |s| s.send(b.clone())))
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "failed to write to stdin queue")
                    })
            })
            .map_err(move |e| error!("failed to forward stdin to broadcast queue: {}", e))
            .map(|_| debug!("stopped stdin to broadcast queue forwarder")),
    );

    let child_triggers = await!(futures::future::join_all(process_handles.into_iter().map(
        |(i, t, c)| c.map(move |x| {
            debug!("process {} exited with {}", i, x);
            t
        })
    )))?;

    for child_trigger in child_triggers {
        drop(child_trigger);
    }

    drop(trigger);

    debug!("all processes finished");

    Ok(())
}

fn generate_final_args(arg: String, command_parts: &Vec<Vec<String>>) -> Vec<String> {
    if command_parts.len() == 1 {
        let mut c = command_parts.iter().next().unwrap().clone();
        c.push(arg);
        c
    } else {
        command_parts.join(&arg)
    }
}

fn parse_delimiter(null: bool, delimiter: Option<u8>) -> Option<u8> {
    if null {
        Some(0)
    } else if let Some(d) = delimiter {
        Some(d)
    } else {
        None
    }
}

fn parse_arg_template(initial_args: Vec<String>, replace: &Option<String>) -> Vec<Vec<String>> {
    initial_args
        .split(|part| replace.as_ref().map_or_else(|| part == "{}", |s| part == s))
        .map(|s| s.to_vec())
        .collect::<Vec<_>>()
}
