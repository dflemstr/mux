use std::time;

pub struct Ui<B, E>
where
    B: tui::backend::Backend,
{
    state: State,
    terminal: tui::Terminal<B>,
    events: E,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Event {
    UserInput(termion::event::Event, bytes::BytesMut),
    EndOfUserInput,
    ProcessOutput(usize, bytes::BytesMut),
    ProcessExit(usize, std::process::ExitStatus),
    Tick,
}

#[derive(Clone, Debug)]
pub enum Data {
    ProcessInput {
        index: usize,
        data: bytes::Bytes,
    },
    ProcessInputAll {
        data: bytes::Bytes,
    },
    #[allow(dead_code)]
    ProcessTermResize {
        index: usize,
        width: u16,
        height: u16,
    },
}

pub struct ProcessSettings {
    pub initial_title: String,
}

struct State {
    processes: Vec<ProcessState>,
}

struct ProcessState {
    terminal_emulator: terminal_emulator::term::Term,
    processor: terminal_emulator::Processor,
    title: String,
    exit_status: Option<std::process::ExitStatus>,
    input: Vec<u8>,
}

impl<B, E> Ui<B, E>
where
    B: tui::backend::Backend + 'static,
    E: futures::stream::Stream<Item = Event, Error = failure::Error>,
{
    pub fn new(
        events: E,
        terminal: tui::Terminal<B>,
        processes: impl IntoIterator<Item = ProcessSettings>,
    ) -> Self {
        let processes = processes
            .into_iter()
            .map(ProcessState::from_settings)
            .collect();
        let state = State::new(processes);

        Self {
            state,
            terminal,
            events,
        }
    }

    pub fn into_frames(
        self,
    ) -> Result<impl futures::stream::Stream<Item = Data, Error = failure::Error>, failure::Error>
    {
        use futures::stream::Stream;
        use std::sync;

        let mut last_size = self.terminal.size()?;
        let state = sync::Arc::new(sync::Mutex::new(self.state));
        let terminal = sync::Arc::new(sync::Mutex::new(self.terminal));

        let size_terminal = sync::Arc::clone(&terminal);
        let resizes = tokio::timer::Interval::new_interval(time::Duration::from_millis(10))
            .filter_map(move |_| {
                let size_terminal = size_terminal.lock().unwrap();
                if let Ok(size) = size_terminal.size() {
                    if size != last_size {
                        last_size = size;
                        Some(Event::Tick)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .map_err(failure::Error::from);

        let frames = self
            .events
            .chain(futures::stream::once(Ok(Event::EndOfUserInput)))
            .select(resizes)
            .take_while(|e| futures::future::ok(*e != Event::EndOfUserInput))
            .and_then(move |event| {
                use futures::future::Future;

                let state = sync::Arc::clone(&state);
                let terminal = sync::Arc::clone(&terminal);

                let process_input_all = {
                    let mut state_guard = state.lock().unwrap();
                    match event {
                        Event::ProcessOutput(idx, data) => {
                            state_guard.on_data(idx, data.freeze());
                            None
                        }
                        Event::ProcessExit(idx, status) => {
                            state_guard.on_exit(idx, status);
                            None
                        }
                        Event::UserInput(_, data) => Some(data.freeze()),
                        Event::EndOfUserInput => None,
                        Event::Tick => None,
                    }
                };

                let process_inputs = state
                    .lock()
                    .unwrap()
                    .take_process_inputs()
                    .map(|(i, d)| (i, d.freeze()))
                    .collect::<Vec<_>>();

                futures::future::poll_fn(move || {
                    let state = sync::Arc::clone(&state);
                    let terminal = sync::Arc::clone(&terminal);

                    tokio_threadpool::blocking(move || {
                        trace!("drawing frame");
                        let mut terminal_guard = terminal.lock().unwrap();
                        terminal_guard.draw(move |mut f| {
                            let mut state_guard = state.lock().unwrap();
                            f.render(&mut *state_guard, f.size());
                        })
                    })
                })
                .map_err(failure::Error::from)
                .map(|_| {
                    futures::stream::iter_ok(
                        process_input_all
                            .into_iter()
                            .map(|data| Data::ProcessInputAll { data })
                            .chain(
                                process_inputs
                                    .into_iter()
                                    .map(|(index, data)| Data::ProcessInput { index, data }),
                            ),
                    )
                })
            })
            .flatten();

        Ok(frames)
    }

    pub fn draw(&mut self) -> Result<(), failure::Error> {
        let state = &mut self.state;
        self.terminal.draw(|mut f| {
            f.render(state, f.size());
        })?;
        Ok(())
    }
}

impl Data {
    pub fn matches_index(&self, other_index: usize) -> bool {
        match *self {
            Data::ProcessInput { index, .. } => index == other_index,
            Data::ProcessInputAll { .. } => true,
            Data::ProcessTermResize { index, .. } => index == other_index,
        }
    }
}

impl State {
    fn new(processes: Vec<ProcessState>) -> Self {
        Self { processes }
    }

    fn on_data(&mut self, index: usize, data: bytes::Bytes) {
        self.processes[index].on_data(data)
    }

    fn on_exit(&mut self, index: usize, status: std::process::ExitStatus) {
        self.processes[index].on_exit(status)
    }

    fn take_process_inputs<'a>(
        &'a mut self,
    ) -> impl Iterator<Item = (usize, bytes::BytesMut)> + 'a {
        self.processes
            .iter_mut()
            .enumerate()
            .flat_map(|(idx, process)| process.take_process_input().map(|d| (idx, d)))
    }
}

impl tui::widgets::Widget for State {
    fn draw(&mut self, area: tui::layout::Rect, buf: &mut tui::buffer::Buffer) {
        let num_processes = self.processes.len();

        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let chunks = tui::layout::Layout::default()
            .direction(tui::layout::Direction::Horizontal)
            .constraints(vec![
                tui::layout::Constraint::Ratio(1, num_processes as u32);
                num_processes
            ])
            .split(area);

        for (i, process) in self.processes.iter_mut().enumerate() {
            process.draw(chunks[i], buf);
        }
    }
}

impl ProcessState {
    fn from_settings(settings: ProcessSettings) -> Self {
        use terminal_emulator::Handler;

        let mut terminal_emulator =
            terminal_emulator::term::Term::new(terminal_emulator::term::SizeInfo {
                width: 80.0,
                height: 24.0,
                cell_width: 1.0,
                cell_height: 1.0,
                padding_x: 0.0,
                padding_y: 0.0,
                dpr: 1.0,
            });
        let processor = terminal_emulator::Processor::new();
        let exit_status = None;
        let input = Vec::new();

        terminal_emulator.set_title(&settings.initial_title);
        let title = settings.initial_title;

        Self {
            terminal_emulator,
            processor,
            title,
            exit_status,
            input,
        }
    }

    fn on_data(&mut self, data: bytes::Bytes) {
        for byte in data {
            // TODO: maybe do something smarter than passing sink() here
            self.processor
                .advance(&mut self.terminal_emulator, byte, &mut self.input);
        }

        if let Some(title) = self.terminal_emulator.get_next_title() {
            self.title = title;
        }
    }

    fn on_exit(&mut self, status: std::process::ExitStatus) {
        self.exit_status = Some(status);
    }

    fn take_process_input(&mut self) -> Option<bytes::BytesMut> {
        use std::mem;

        if self.input.is_empty() {
            None
        } else {
            let input = mem::replace(&mut self.input, Vec::new());
            Some(bytes::BytesMut::from(input))
        }
    }
}

impl tui::widgets::Widget for ProcessState {
    fn draw(&mut self, area: tui::layout::Rect, buf: &mut tui::buffer::Buffer) {
        let chunks = tui::layout::Layout::default()
            .direction(tui::layout::Direction::Vertical)
            .constraints(vec![
                tui::layout::Constraint::Min(0),
                tui::layout::Constraint::Length(if self.exit_status.is_none() { 0 } else { 1 }),
            ])
            .split(area);
        let main_chunk = chunks[0];
        let status_chunk = chunks[1];

        let mut block = tui::widgets::Block::default()
            .title(&self.title)
            .borders(tui::widgets::Borders::ALL);
        block.draw(main_chunk, buf);
        let inner_area = block.inner(main_chunk);

        for cell in self.terminal_emulator.renderable_cells() {
            #[allow(clippy::cast_possible_truncation)]
            let x = cell.column.0 as u16;
            #[allow(clippy::cast_possible_truncation)]
            let y = cell.line.0 as u16;
            if x < inner_area.width && y < inner_area.height {
                let x = inner_area.x + x;
                let y = inner_area.y + y;
                let buf_cell = buf.get_mut(x, y);
                buf_cell.set_char(cell.chars[0]);
                buf_cell.set_bg(convert_color(cell.bg));
                buf_cell.set_fg(convert_color(cell.fg));
                buf_cell.set_modifier(convert_flags(cell.flags));
            }
        }

        if let Some(exit_status) = self.exit_status {
            let style = if exit_status.success() {
                tui::style::Style::default()
                    .fg(tui::style::Color::Black)
                    .bg(tui::style::Color::Green)
                    .modifier(tui::style::Modifier::BOLD | tui::style::Modifier::DIM)
            } else {
                tui::style::Style::default()
                    .fg(tui::style::Color::Black)
                    .bg(tui::style::Color::Red)
                    .modifier(tui::style::Modifier::BOLD | tui::style::Modifier::DIM)
            };
            tui::widgets::Paragraph::new(
                [tui::widgets::Text::raw(format!(
                    "exited with {}",
                    exit_status
                ))]
                .as_ref()
                .iter(),
            )
            .style(style)
            .draw(status_chunk, buf);
        }
    }
}

fn convert_color(color: terminal_emulator::ansi::Color) -> tui::style::Color {
    match color {
        terminal_emulator::ansi::Color::Named(named) => match named {
            terminal_emulator::ansi::NamedColor::Black => tui::style::Color::Black,
            terminal_emulator::ansi::NamedColor::Red => tui::style::Color::Red,
            terminal_emulator::ansi::NamedColor::Green => tui::style::Color::Green,
            terminal_emulator::ansi::NamedColor::Yellow => tui::style::Color::Yellow,
            terminal_emulator::ansi::NamedColor::Blue => tui::style::Color::Blue,
            terminal_emulator::ansi::NamedColor::Magenta => tui::style::Color::Magenta,
            terminal_emulator::ansi::NamedColor::Cyan => tui::style::Color::Cyan,
            terminal_emulator::ansi::NamedColor::White => tui::style::Color::White,
            terminal_emulator::ansi::NamedColor::BrightBlack => tui::style::Color::DarkGray,
            terminal_emulator::ansi::NamedColor::BrightRed => tui::style::Color::LightRed,
            terminal_emulator::ansi::NamedColor::BrightGreen => tui::style::Color::LightGreen,
            terminal_emulator::ansi::NamedColor::BrightYellow => tui::style::Color::LightYellow,
            terminal_emulator::ansi::NamedColor::BrightBlue => tui::style::Color::LightBlue,
            terminal_emulator::ansi::NamedColor::BrightMagenta => tui::style::Color::LightMagenta,
            terminal_emulator::ansi::NamedColor::BrightCyan => tui::style::Color::LightCyan,
            terminal_emulator::ansi::NamedColor::BrightWhite => tui::style::Color::Gray,
            terminal_emulator::ansi::NamedColor::Foreground => tui::style::Color::Reset,
            terminal_emulator::ansi::NamedColor::Background => tui::style::Color::Reset,
            terminal_emulator::ansi::NamedColor::CursorText => tui::style::Color::Black,
            terminal_emulator::ansi::NamedColor::Cursor => tui::style::Color::Gray,
            terminal_emulator::ansi::NamedColor::DimBlack => tui::style::Color::Black,
            terminal_emulator::ansi::NamedColor::DimRed => tui::style::Color::Red,
            terminal_emulator::ansi::NamedColor::DimGreen => tui::style::Color::Green,
            terminal_emulator::ansi::NamedColor::DimYellow => tui::style::Color::Yellow,
            terminal_emulator::ansi::NamedColor::DimBlue => tui::style::Color::Blue,
            terminal_emulator::ansi::NamedColor::DimMagenta => tui::style::Color::Magenta,
            terminal_emulator::ansi::NamedColor::DimCyan => tui::style::Color::Cyan,
            terminal_emulator::ansi::NamedColor::DimWhite => tui::style::Color::White,
            terminal_emulator::ansi::NamedColor::BrightForeground => tui::style::Color::Reset,
            terminal_emulator::ansi::NamedColor::DimForeground => tui::style::Color::Reset,
        },
        terminal_emulator::ansi::Color::Spec(color) => {
            tui::style::Color::Rgb(color.r, color.g, color.b)
        }
        terminal_emulator::ansi::Color::Indexed(i) => tui::style::Color::Indexed(i),
    }
}

fn convert_flags(flags: terminal_emulator::term::cell::Flags) -> tui::style::Modifier {
    let mut result = tui::style::Modifier::empty();

    if flags.contains(terminal_emulator::term::cell::Flags::INVERSE) {
        result.insert(tui::style::Modifier::REVERSED);
    }
    if flags.contains(terminal_emulator::term::cell::Flags::BOLD) {
        result.insert(tui::style::Modifier::BOLD);
    }
    if flags.contains(terminal_emulator::term::cell::Flags::ITALIC) {
        result.insert(tui::style::Modifier::ITALIC);
    }
    if flags.contains(terminal_emulator::term::cell::Flags::UNDERLINE) {
        result.insert(tui::style::Modifier::UNDERLINED);
    }
    if flags.contains(terminal_emulator::term::cell::Flags::DIM) {
        result.insert(tui::style::Modifier::DIM);
    }
    if flags.contains(terminal_emulator::term::cell::Flags::HIDDEN) {
        result.insert(tui::style::Modifier::HIDDEN);
    }
    if flags.contains(terminal_emulator::term::cell::Flags::STRIKEOUT) {
        result.insert(tui::style::Modifier::CROSSED_OUT);
    }

    result
}
