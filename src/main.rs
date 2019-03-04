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
mod options;
mod process;
mod sinks;
mod streams;
mod tty;
mod ui;

fn main() {
    use std::process;

    if let Err(err) = run() {
        eprintln!("{}", err);
        eprintln!("{}", err.backtrace());
        process::exit(1)
    }
}

fn run() -> Result<(), failure::Error> {
    use futures::future::Future;
    use std::fs;
    use std::sync;
    use structopt::StructOpt;

    log_panics::init();

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

    info!("starting");

    let result = sync::Arc::new(sync::Mutex::new(None));
    let result_clone = sync::Arc::clone(&result);
    tokio::run(
        tokio_async_await::compat::backward::Compat::new(run_with_options(options))
            .then(move |r| futures::future::ok(*result_clone.lock().unwrap() = Some(r))),
    );

    info!("done");

    let mut guard = result.lock().unwrap();
    guard.take().unwrap_or_else(|| {
        Err(failure::err_msg(
            "an async panic occurred (check log file for more info)",
        ))
    })
}

async fn run_with_options(mut options: options::Options) -> Result<(), failure::Error> {
    use futures::future::Future;
    use futures::stream::Stream;

    let template_placeholder = options.replace.clone().unwrap_or_else(|| "{}".to_owned());
    let args = await!(args::read(&mut options))?;
    let command = options.command;

    let processes = args
        .iter()
        .map(|args| process::Process::spawn(&command, &args.all))
        .collect::<Result<Vec<_>, _>>()?;

    debug!("spawned {} processes", processes.len());

    let (process_writes, process_reads): (Vec<_>, Vec<_>) =
        processes.into_iter().map(|p| p.split()).unzip();

    let mut tty_output = tty::Tty::open()?.into_raw_mode()?;
    let tty_input = tty_output.try_clone()?;

    debug!("opened tty");

    let mut terminal = await!(create_terminal(tty_output))?;
    terminal.hide_cursor()?;

    debug!("created terminal");

    let events = read_events(tty_input);
    let input = await!(run_gui(
        process_reads,
        terminal,
        events,
        args.into_iter()
            .map(|args| args.specific)
            .collect::<Vec<_>>(),
        template_placeholder,
    ))?;

    let rest = await!(forward_stdin(process_writes, input))?;

    debug!("stdin closed, discarding further input");

    await!(rest.into_future().map_err(|(e, _)| e))?;

    debug!("end of input");

    Ok(())
}

async fn run_gui(
    process_reads: Vec<process::Read>,
    terminal: tui::Terminal<impl tui::backend::Backend + 'static>,
    user_input: impl futures::stream::Stream<Item = ui::Event, Error = failure::Error>,
    args: Vec<String>,
    template_placeholder: String,
) -> Result<impl futures::Stream<Item = ui::Action, Error = failure::Error>, failure::Error> {
    use futures::future::Future;
    use futures::stream::Stream;
    use std::sync;
    use std::time;

    let (outputs, exits): (Vec<_>, Vec<_>) = process_reads
        .into_iter()
        .map(|p| (p.output, p.exit))
        .unzip();

    let output = streams::select_all(
        outputs
            .into_iter()
            .enumerate()
            .map(|(i, o)| o.map(move |b| ui::Event::ProcessOutput(i, b.freeze()))),
    );

    let exit = futures::stream::futures_unordered(
        exits
            .into_iter()
            .enumerate()
            .map(|(i, e)| e.map(move |e| ui::Event::ProcessExit(i, e))),
    );

    let processes = args.into_iter().map(|arg| ui::ProcessSettings {
        initial_title: format!("{}={}", template_placeholder, arg),
    });

    let mut ui = ui::Ui::new(terminal, processes)?;

    await!(futures::future::poll_fn(|| tokio_threadpool::blocking(
        || ui.draw()
    )))??;

    let ui = sync::Arc::new(sync::Mutex::new(ui));

    let resize_ui = sync::Arc::clone(&ui);
    let resizes = tokio::timer::Interval::new_interval(time::Duration::from_millis(10))
        .filter_map(move |_| {
            if resize_ui.lock().unwrap().check_resized() {
                Some(ui::Event::Resized)
            } else {
                None
            }
        })
        .map_err(failure::Error::from);

    let events = user_input
        .chain(futures::stream::once(Ok(ui::Event::EndOfUserInput)))
        .select(output)
        .select(exit)
        .select(resizes)
        .take_while(|e| futures::future::ok(*e != ui::Event::EndOfUserInput));

    Ok(events
        .and_then(move |event| {
            let event = sync::Arc::new(event);
            let ui = sync::Arc::clone(&ui);
            futures::future::poll_fn(move || {
                let event = sync::Arc::clone(&event);
                let ui = sync::Arc::clone(&ui);
                tokio_threadpool::blocking(move || ui.lock().unwrap().on_event(&event))
            })
            .map_err(failure::Error::from)
            .and_then(|r| r)
            .map(futures::stream::iter_ok)
        })
        .flatten())
}

fn read_events(
    read: impl std::io::Read + Send + 'static,
) -> impl futures::stream::Stream<Item = ui::Event, Error = failure::Error> + Send + 'static {
    use futures::stream::Stream;
    use termion::input::TermReadEventsAndRaw;

    let event_iterator = read.events_and_raw();

    let raw_events_stream = streams::blocking_iter_to_stream(
        event_iterator
            .inspect(|e| debug!("received tty event: {:?}", e))
            .take_while(|e| match e {
                Ok((termion::event::Event::Key(termion::event::Key::Ctrl('t')), _)) => false,
                _ => true,
            }),
    )
    .map_err(failure::Error::from);

    raw_events_stream
        .and_then(move |event| match event? {
            (event, data) => Ok(ui::Event::UserInput(event, data.into())),
        })
        .fuse()
}

async fn forward_stdin(
    inputs: Vec<process::Write>,
    stdin: impl futures::stream::Stream<Item = ui::Action, Error = failure::Error> + Send + 'static,
) -> Result<
    impl futures::stream::Stream<Item = (), Error = failure::Error> + Send + 'static,
    failure::Error,
> {
    use futures::sink::Sink;

    let (rest, _) = await!(
        stdin.forward(sinks::Fanout::new(inputs.into_iter().enumerate().map(
            |(my_index, p)| {
                p.input
                    .with_flat_map(move |data| {
                        futures::stream::iter_ok(match data {
                            ui::Action::ProcessInputAll { data, .. } => Some(data),
                            ui::Action::ProcessInput { data, .. } => Some(data),
                            // TODO: find a way to process other events
                            _ => None,
                        })
                    })
                    .with_flat_map(move |data: ui::Action| {
                        futures::stream::iter_ok(if data.matches_index(my_index) {
                            Some(data)
                        } else {
                            None
                        })
                    })
            }
        )))
    )?;

    Ok(rest.map(|_| ()))
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
