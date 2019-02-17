#![feature(await_macro, async_await, futures_api)]
#![warn(clippy::all, clippy::pedantic)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate structopt;
#[macro_use]
extern crate tokio;

#[cfg(target_os = "redox")]
#[path = "sys/redox/mod.rs"]
mod sys;

#[cfg(unix)]
#[path = "sys/unix/mod.rs"]
mod sys;

mod args;
mod fanout;
mod options;
mod process;
mod pty;
mod tty;

enum Event {
    Term(termion::event::Event),
}

fn main() {
    use std::process;

    if let Err(err) = run() {
        error!("{:?}", err);
        process::exit(1)
    }
}

fn run() -> Result<(), failure::Error> {
    use std::fs;
    use structopt::StructOpt;

    let options = options::Options::from_args();

    let cache_dir = dirs::cache_dir().expect("no suitable cache dir found").join("mux");
    fs::create_dir_all(&cache_dir)?;

    fern::Dispatch::new()
        .level(match options.log_verbose {
            0 => log::LevelFilter::Error,
            1 => log::LevelFilter::Warn,
            2 => log::LevelFilter::Info,
            3 => log::LevelFilter::Debug,
            _ => log::LevelFilter::Trace,
        })
        .format(|out, message, record| {
            out.finish(format_args!(
                "{}[{}][{}] {}",
                chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                record.target(),
                record.level(),
                message
            ))
        })
        .chain(fern::log_file(cache_dir.join("session.log"))?)
        .apply()?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on_all(tokio_async_await::compat::backward::Compat::new(
        run_with_options(options),
    ))?;

    Ok(())
}

async fn run_with_options(mut options: options::Options) -> Result<(), failure::Error> {
    use futures::future::Future;

    let args = await!(args::read(&mut options))?;
    let command = options.command;

    let mut managed_processes = args
        .into_iter()
        .enumerate()
        .map(move |(index, args)| process::Process::spawn(index, command.clone(), args))
        .collect::<Result<Vec<_>, failure::Error>>()?;

    let mut tty_output = tty::Tty::open()?.into_raw_mode()?;
    let tty_input = tty_output.try_clone()?;

    let mut terminal = await!(create_terminal(tty_output))?;
    terminal.hide_cursor()?;

    let (input_source, events) = await!(spawn_input(tty_input))?;

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
    _managed_processes: &mut Vec<process::Process>,
    terminal: tui::Terminal<B>,
    events: E,
) where
    B: tui::backend::Backend + Send + 'static,
    E: futures::stream::Stream<Item = Event, Error = failure::Error> + Send + 'static,
{
    tokio::spawn_async(
        async {
            await!(run_gui(terminal, events))
                .unwrap_or_else(|err| error!("failed to render GUI: {}", err))
        },
    )
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

async fn spawn_input<R>(
    read: R,
) -> Result<
    (
        impl futures::stream::Stream<Item = bytes::Bytes, Error = failure::Error> + Send + 'static,
        impl futures::stream::Stream<Item = Event, Error = failure::Error> + Send + 'static,
    ),
    failure::Error,
>
where
    R: std::io::Read + Send + 'static,
{
    use futures::future::Future;
    use futures::stream::Stream;
    use termion::input::TermReadEventsAndRaw;

    let event_iterator = read.events_and_raw();

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

fn spawn_stdin_forwarder<S>(managed_processes: &mut Vec<process::Process>, stdin_source: S)
where
    S: futures::stream::Stream<Item = bytes::Bytes, Error = failure::Error> + Send + 'static,
{
    use futures::future::Future;
    use futures::sink::Sink;

    let in_txs = managed_processes
        .iter_mut()
        .map(|p| p.in_tx.take().unwrap())
        .collect::<Vec<_>>();

    let in_fanout_tx = fanout::Fanout::new(in_txs)
        .sink_map_err(|_| failure::err_msg("failed to write to stdin fanout queue"));

    tokio::spawn(
        stdin_source
            .forward(in_fanout_tx)
            .map_err(move |e| error!("failed to forward stdin to fanout queue: {}", e))
            .map(|_| debug!("stopped stdin to fanout queue forwarder")),
    );
}

async fn create_terminal<W>(
    output: W,
) -> Result<tui::Terminal<impl tui::backend::Backend>, failure::Error>
where
    W: std::io::Write,
{
    let mouse_terminal = termion::input::MouseTerminal::from(output);
    let alternate_screen_terminal = termion::screen::AlternateScreen::from(mouse_terminal);
    let backend = tui::backend::TermionBackend::new(alternate_screen_terminal);

    let terminal = tui::Terminal::new(backend)?;
    Ok(terminal)
}
