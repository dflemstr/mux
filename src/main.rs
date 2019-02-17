#![feature(await_macro, async_await, futures_api)]
#![warn(clippy::all, clippy::pedantic)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate structopt;
#[macro_use]
extern crate tokio;

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
    Data(bytes::Bytes),
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

    if let Some(mut log) = dirs::cache_dir() {
        log.push("mux");
        fs::create_dir_all(&log)?;
        log.push("session.log");

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
            .chain(fern::log_file(&log)?)
            .apply()?;
    }

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

    let mut processes = args
        .into_iter()
        .enumerate()
        .map(move |(index, args)| process::Process::spawn(index, command.clone(), args))
        .collect::<Result<Vec<_>, failure::Error>>()?;

    let mut tty_output = tty::Tty::open()?.into_raw_mode()?;
    let tty_input = tty_output.try_clone()?;

    let mut terminal = await!(create_terminal(tty_output))?;
    terminal.hide_cursor()?;

    let events = read_events(tty_input)?;

    let stdin = run_gui(&mut processes, terminal, events);

    await!(forward_stdin(&mut processes, stdin))?;

    let exit_statuses_future = futures::future::join_all(processes.into_iter().map(|p| {
        let i = p.index;
        p.child
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

fn run_gui(
    _processes: &mut Vec<process::Process>,
    mut terminal: tui::Terminal<impl tui::backend::Backend + 'static>,
    events: impl futures::stream::Stream<Item = Event, Error = failure::Error>,
) -> impl futures::Stream<Item = bytes::Bytes, Error = failure::Error> {
    use futures::stream::Stream;

    events
        .and_then(move |event| {
            terminal.draw(|mut _f| {})?;
            Ok(event)
        })
        .filter_map(|event| match event {
            Event::Data(data) => Some(data),
            _ => None,
        })
}

fn read_events(
    read: impl std::io::Read + Send + 'static,
) -> Result<
    impl futures::stream::Stream<Item = Event, Error = failure::Error> + Send + 'static,
    failure::Error,
> {
    use futures::stream::Stream;
    use termion::input::TermReadEventsAndRaw;

    let event_iterator = read.events_and_raw();

    let raw_events_stream = blocking_iter_to_stream(event_iterator);

    let events = raw_events_stream
        .and_then(move |event| {
            Ok(match event? {
                (event @ termion::event::Event::Mouse(_), _) => Some(Event::Term(event)),
                (termion::event::Event::Key(termion::event::Key::Ctrl('d')), _)
                | (termion::event::Event::Key(termion::event::Key::Ctrl('c')), _) => None,
                (_, data) => Some(Event::Data(data.into())),
            })
        })
        .take_while(|o| futures::future::ok(o.is_some()))
        .map(Option::unwrap);

    Ok(events)
}

fn blocking_iter_to_stream<A>(
    iter: impl Iterator<Item = A>,
) -> impl futures::stream::Stream<Item = A, Error = failure::Error>
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

async fn forward_stdin(
    managed_processes: &mut Vec<process::Process>,
    stdin: impl futures::stream::Stream<Item = bytes::Bytes, Error = failure::Error> + Send + 'static,
) -> Result<(), failure::Error> {
    let in_txs = managed_processes
        .iter_mut()
        .map(|p| p.stdin.take().unwrap())
        .collect::<Vec<_>>();

    let in_fanout_tx = fanout::Fanout::new(in_txs);

    await!(stdin.forward(in_fanout_tx))?;

    Ok(())
}

async fn create_terminal(
    output: impl std::io::Write,
) -> Result<tui::Terminal<impl tui::backend::Backend>, failure::Error> {
    let mouse_terminal = termion::input::MouseTerminal::from(output);
    let alternate_screen_terminal = termion::screen::AlternateScreen::from(mouse_terminal);
    let backend = tui::backend::TermionBackend::new(alternate_screen_terminal);

    let terminal = tui::Terminal::new(backend)?;
    Ok(terminal)
}
