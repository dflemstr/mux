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
use std::sync;

mod args;
mod options;
mod pty;

fn main() -> Result<(), failure::Error> {
    use structopt::StructOpt;

    pretty_env_logger::init_timed();

    let options = options::Options::from_args();

    let result = sync::Arc::new(sync::Mutex::new(None));
    tokio::run_async(run_safe(options, result.clone()));

    let mut maybe_result = result.lock().unwrap();
    (*maybe_result)
        .take()
        .expect("execution aborted due to panic")
}

async fn run_safe(
    options: options::Options,
    result: sync::Arc<sync::Mutex<Option<Result<(), failure::Error>>>>,
) {
    *result.lock().unwrap() = Some(await!(run(options)));
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

    let mut child_processes = args
        .into_iter()
        .enumerate()
        .map(move |(index, args)| {
            use tokio_process::CommandExt;
            let mut child_process = process::Command::new(command.clone())
                .args(args)
                .stdin(process::Stdio::piped())
                .spawn_async()?;

            let stdin_sink = if let Some(stdin) = child_process.stdin().take() {
                let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();

                let (trigger, input_rx) = stream_cancel::Valved::new(input_rx);

                let stdin_sink =
                    tokio::codec::FramedWrite::new(stdin, tokio::codec::BytesCodec::new());

                tokio::spawn(
                    input_rx
                        .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, format!("failed to read from stdin queue: {:?}", e)))
                        .forward(stdin_sink)
                        .map_err(move |e| {
                            error!(
                                "failed to forward stdin broadcast queue to process {}: {}",
                                index, e
                            )
                        })
                        .map(move |_| debug!("stopped stdin broadcast queue to process {} forwarder", index)),
                );

                Some((trigger, input_tx))
            } else { None };

            Ok((stdin_sink, child_process))
        })
        .collect::<Result<Vec<_>, failure::Error>>()?;

    let (trigger, stdin_source) = stream_cancel::Valved::new(tokio::codec::FramedRead::new(
        await!(tokio::fs::File::open("/dev/tty"))?,
        tokio::codec::BytesCodec::new(),
    ));

    let stdin_sinks = child_processes.iter_mut().flat_map(|(stdin_sink, _)| stdin_sink.take()).collect::<Vec<_>>();

    tokio::spawn(
        stdin_source
            .map(|b| b.freeze())
            .fold(stdin_sinks, |stdin_sinks, b| {
                futures::future::join_all(stdin_sinks.into_iter().map(move |s| s.send(b.clone())))
                    .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, format!("failed to write to stdin queue: {:?}", e)))
            })
            .map_err(move |e| error!("failed to forward stdin to broadcast queue: {}", e))
            .map(|_| debug!("stopped stdin to broadcast queue forwarder")),
    );

    await!(futures::future::join_all(child_processes.into_iter().map(|(_, c)| c)))?;

    debug!("all processes finished");

    drop(trigger);

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
