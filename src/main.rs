#![feature(await_macro, async_await, futures_api)]
#![warn(clippy::all, clippy::pedantic)]

#[macro_use]
extern crate bitflags;
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
mod stream_utils;
mod tty;
mod ui;

enum Event {
    Term(termion::event::Event),
    Input(bytes::BytesMut),
    Output(usize, bytes::BytesMut),
}

fn main() {
    use std::process;

    if let Err(err) = run() {
        eprintln!("{}", err);
        eprintln!("{}", err.backtrace());
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

    debug!("starting");

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on_all(tokio_async_await::compat::backward::Compat::new(
        run_with_options(options),
    ))?;

    debug!("done");

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

    debug!("spawned {} processes", processes.len());

    let mut tty_output = tty::Tty::open()?.into_raw_mode()?;
    let tty_input = tty_output.try_clone()?;

    debug!("opened tty");

    let mut terminal = await!(create_terminal(tty_output))?;
    terminal.hide_cursor()?;

    debug!("created terminal");

    let events = read_events(tty_input)?;

    let stdin = run_gui(&mut processes, terminal, events);

    debug!("beginning to forward stdin");

    await!(forward_stdin(&mut processes, stdin))?;

    debug!("done forwarding stdin; waiting for processes to finish");

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
    processes: &mut Vec<process::Process>,
    mut terminal: tui::Terminal<impl tui::backend::Backend + 'static>,
    events: impl futures::stream::Stream<Item = Event, Error = failure::Error>,
) -> impl futures::Stream<Item = bytes::BytesMut, Error = failure::Error> {
    use futures::stream::Stream;
    use std::str;

    let mut state = processes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            assert_eq!(i, p.index);
            String::new()
        })
        .collect::<Vec<_>>();
    let num_processes = processes.len();
    let outputs = processes
        .iter_mut()
        .map(|p| p.output.take().unwrap())
        .collect::<Vec<_>>();

    let output = stream_utils::select_all(
        outputs
            .into_iter()
            .enumerate()
            .map(|(i, o)| o.map(move |b| (i, b))),
    )
    .map(|(idx, data)| Event::Output(idx, data));

    events
        .select(output)
        .and_then(move |event| {
            match &event {
                Event::Output(idx, data) => {
                    state[*idx].push_str(str::from_utf8(&data)?);
                }
                _ => {}
            };

            terminal.draw(|mut f| {
                use tui::widgets::Widget;

                let chunks = tui::layout::Layout::default()
                    .direction(tui::layout::Direction::Horizontal)
                    .constraints(vec![
                        tui::layout::Constraint::Percentage(
                            (100.0 / num_processes as f64) as u16
                        );
                        num_processes
                    ])
                    .split(f.size());

                for (i, output) in state.iter().enumerate() {
                    tui::widgets::Paragraph::new(
                        output
                            .lines()
                            .map(|line| tui::widgets::Text::Raw(line.into()))
                            .collect::<Vec<_>>()
                            .iter(),
                    )
                    .block(
                        tui::widgets::Block::default()
                            .borders(tui::widgets::Borders::ALL)
                            .title(&format!("{}", i)),
                    )
                    .render(&mut f, chunks[i]);
                }
            })?;

            Ok(event)
        })
        .filter_map(|event| match event {
            Event::Input(data) => Some(data),
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

    let raw_events_stream =
        stream_utils::blocking_iter_to_stream(event_iterator).map_err(failure::Error::from);

    let events = raw_events_stream
        .and_then(move |event| {
            Ok(match event? {
                (event @ termion::event::Event::Mouse(_), _) => Some(Event::Term(event)),
                (termion::event::Event::Key(termion::event::Key::Ctrl('d')), _) => None,
                (_, data) => Some(Event::Input(data.into())),
            })
        })
        .take_while(|o| futures::future::ok(o.is_some()))
        .map(Option::unwrap);

    Ok(events)
}

async fn forward_stdin(
    managed_processes: &mut Vec<process::Process>,
    stdin: impl futures::stream::Stream<Item = bytes::BytesMut, Error = failure::Error> + Send + 'static,
) -> Result<(), failure::Error> {
    use futures::stream::Stream;

    let in_txs = managed_processes
        .iter_mut()
        .map(|p| p.input.take().unwrap())
        .collect::<Vec<_>>();

    let in_fanout_tx = fanout::Fanout::new(in_txs);

    await!(stdin.map(bytes::BytesMut::freeze).forward(in_fanout_tx))?;

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
