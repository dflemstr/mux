#![feature(await_macro, async_await, futures_api)]
#![warn(clippy::all, clippy::pedantic)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate structopt;
#[macro_use]
extern crate tokio;

mod args;
mod fanout;
mod managed_process;
mod options;
mod pty;
mod tty;

enum Event {
    Term(termion::event::Event),
}

fn main() {
    use std::process;

    pretty_env_logger::init_timed();

    if let Err(err) = run() {
        error!("{:?}", err);
        process::exit(1)
    }
}

fn run() -> Result<(), failure::Error> {
    use structopt::StructOpt;

    let options = options::Options::from_args();

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on_all(tokio_async_await::compat::backward::Compat::new(
        run_with_options(options),
    ))?;

    Ok(())
}

async fn run_with_options(options: options::Options) -> Result<(), failure::Error> {
    use futures::future::Future;
    use futures::stream::Stream;

    let delimiter = parse_delimiter(options.null, options.delimiter);

    let arg_template = parse_arg_template(&options.initial_args, &options.replace);

    let raw_args = await!(args::generate(options.arg_file, delimiter))?;

    let args: Vec<Vec<String>> = await!(raw_args
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map(|a| generate_final_args(a, &arg_template))
        .collect())?;

    let command = options.command;

    let mut managed_processes = args
        .into_iter()
        .enumerate()
        .map(move |(index, args)| {
            managed_process::ManagedProcess::spawn(index, command.clone(), args)
        })
        .collect::<Result<Vec<_>, failure::Error>>()?;

    let mut terminal = await!(create_terminal())?;
    terminal.hide_cursor()?;

    let (input_source, events) = await!(spawn_input())?;

    spawn_stdin_forwarder(&mut managed_processes, input_source);

    spawn_gui(&mut managed_processes, terminal, events);

    let exit_statuses_future = futures::future::join_all(managed_processes.into_iter().map(|p| {
        let i = p.index;
        p.child_process
            .inspect(move |x| debug!("process {} exited with {}", i, x))
            .map(move |x| (i, x))
    }));

    let exit_statuses = await!(exit_statuses_future)?;

    debug!("all processes finished");

    for (index, exit_status) in exit_statuses {
        if !exit_status.success() {
            return Err(failure::err_msg(format!(
                "process with index {} failed with {}",
                index, exit_status
            )));
        }
    }

    Ok(())
}

fn spawn_gui<B, E>(
    _managed_processes: &mut Vec<managed_process::ManagedProcess>,
    _terminal: tui::Terminal<B>,
    _events: E,
) where
    B: tui::backend::Backend,
    E: futures::stream::Stream<Item = Event, Error = failure::Error>,
{
}

async fn run_gui<B, E>(mut terminal: tui::Terminal<B>, events: E) -> Result<(), failure::Error>
where
    B: tui::backend::Backend,
    E: futures::stream::Stream<Item = Event, Error = failure::Error>,
{
    await!(events.for_each(|_event| {
        terminal.draw(|mut _f| {})?;
        Ok(())
    }))
}

async fn spawn_input() -> Result<
    (
        impl futures::stream::Stream<Item = bytes::Bytes, Error = failure::Error> + Send + 'static,
        impl futures::stream::Stream<Item = Event, Error = failure::Error> + Send + 'static,
    ),
    failure::Error,
> {
    use futures::future::Future;
    use futures::stream::Stream;
    use termion::input::TermReadEventsAndRaw;

    let tty = await!(tokio::fs::File::open("/dev/tty"))?;
    let event_iterator = tty.events_and_raw();

    let raw_events_stream = blocking_iter_to_stream(event_iterator);

    let (data_tx, data_rx) = futures::sync::mpsc::unbounded();
    let (events_tx, events_rx) = futures::sync::mpsc::unbounded();

    tokio::spawn(
        raw_events_stream
            .for_each(move |event| {
                match event? {
                    (event @ termion::event::Event::Mouse(_), _) => {
                        events_tx.unbounded_send(Event::Term(event))?
                    }
                    (_, data) => data_tx.unbounded_send(data.into())?,
                }
                Ok(())
            })
            .map_err(|e| error!("failed to read raw events stream: {}", e)),
    );

    let data_rx = data_rx.map_err(|_| failure::err_msg("failed to receive input data"));
    let events_rx = events_rx.map_err(|_| failure::err_msg("failed to receive event data"));

    Ok((data_rx, events_rx))
}

fn blocking_iter_to_stream<I, A>(
    iter: I,
) -> impl futures::stream::Stream<Item = A, Error = failure::Error>
where
    I: Iterator<Item = A>,
{
    use futures::stream::Stream;
    use std::sync;
    let iter = sync::Arc::new(sync::Mutex::new(iter));

    futures::stream::poll_fn(move || {
        let iter = sync::Arc::clone(&iter);
        tokio_threadpool::blocking(move || {
            let mut iter = iter.lock().unwrap();
            iter.next()
        })
    })
    .map_err(failure::Error::from)
}

fn spawn_stdin_forwarder<S>(
    managed_processes: &mut Vec<managed_process::ManagedProcess>,
    stdin_source: S,
) where
    S: futures::stream::Stream<Item = bytes::Bytes, Error = failure::Error> + Send + 'static,
{
    use futures::future::Future;
    use futures::sink::Sink;

    let in_txs = managed_processes
        .iter_mut()
        .map(|p| p.in_tx.take().unwrap())
        .collect();

    let in_fanout_tx = fanout::Fanout::new(in_txs)
        .sink_map_err(|_| failure::err_msg("failed to write to stdin fanout queue"));

    tokio::spawn(
        stdin_source
            .forward(in_fanout_tx)
            .map_err(move |e| error!("failed to forward stdin to fanout queue: {}", e))
            .map(|_| debug!("stopped stdin to fanout queue forwarder")),
    );
}

async fn create_terminal() -> Result<tui::Terminal<impl tui::backend::Backend>, failure::Error> {
    use termion::raw::IntoRawMode;

    let tty = await!(tokio::fs::OpenOptions::new().write(true).open("/dev/tty"))?;
    let raw_terminal = tty.into_raw_mode()?;
    let mouse_terminal = termion::input::MouseTerminal::from(raw_terminal);
    let alternate_screen_terminal = termion::screen::AlternateScreen::from(mouse_terminal);
    let backend = tui::backend::TermionBackend::new(alternate_screen_terminal);

    let terminal = tui::Terminal::new(backend)?;
    Ok(terminal)
}

fn generate_final_args(arg: String, command_parts: &[Vec<String>]) -> Vec<String> {
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

fn parse_arg_template(initial_args: &[String], replace: &Option<String>) -> Vec<Vec<String>> {
    initial_args
        .split(|part| replace.as_ref().map_or_else(|| part == "{}", |s| part == s))
        .map(|s| s.to_vec())
        .collect::<Vec<_>>()
}
