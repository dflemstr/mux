pub struct Ui<B, E>
where
    B: tui::backend::Backend,
{
    state: State,
    terminal: tui::Terminal<B>,
    events: E,
}

pub enum Event {
    Input(termion::event::Event, bytes::BytesMut),
    Output(usize, bytes::BytesMut),
    Exit(usize, std::process::ExitStatus),
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
}

impl<B, E> Ui<B, E>
where
    B: tui::backend::Backend,
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
    ) -> impl futures::stream::Stream<Item = bytes::BytesMut, Error = failure::Error> {
        use futures::stream::Stream;

        let mut state = self.state;
        let mut terminal = self.terminal;

        self.events
            .and_then(move |event| {
                let data = match event {
                    Event::Output(idx, data) => {
                        state.on_data(idx, data.freeze())?;
                        None
                    }
                    Event::Exit(idx, status) => {
                        state.on_exit(idx, status)?;
                        None
                    }
                    Event::Input(_, data) => Some(data),
                };

                terminal.draw(|mut f| {
                    f.render(&mut state, f.size());
                })?;

                Ok(data)
            })
            .filter_map(|data| data)
    }

    pub fn draw(&mut self) -> Result<(), failure::Error> {
        let state = &mut self.state;
        self.terminal.draw(|mut f| {
            f.render(state, f.size());
        })?;
        Ok(())
    }
}

impl State {
    fn new(processes: Vec<ProcessState>) -> Self {
        Self { processes }
    }

    fn on_data(&mut self, index: usize, data: bytes::Bytes) -> Result<(), failure::Error> {
        self.processes[index].on_data(data)
    }

    fn on_exit(
        &mut self,
        index: usize,
        status: std::process::ExitStatus,
    ) -> Result<(), failure::Error> {
        self.processes[index].on_exit(status)
    }
}

impl tui::widgets::Widget for State {
    fn draw(&mut self, area: tui::layout::Rect, buf: &mut tui::buffer::Buffer) {
        let num_processes = self.processes.len();

        let chunks = tui::layout::Layout::default()
            .direction(tui::layout::Direction::Horizontal)
            .constraints(vec![
                tui::layout::Constraint::Percentage(
                    (100.0 / num_processes as f64) as u16
                );
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

        let mut terminal_emulator = terminal_emulator::term::Term::new(&terminal_emulator::Config::default(), terminal_emulator::term::SizeInfo {
            width: 80.0,
            height: 24.0,
            cell_width: 1.0,
            cell_height: 1.0,
            padding_x: 0.0,
            padding_y: 0.0,
            dpr: 1.0
        });
        let processor = terminal_emulator::Processor::new();
        let exit_status = None;

        terminal_emulator.set_title(&settings.initial_title);
        let title = settings.initial_title;

        Self { terminal_emulator, processor, title, exit_status }
    }

    fn on_data(&mut self, data: bytes::Bytes) -> Result<(), failure::Error> {
        for byte in data {
            // TODO: maybe do something smarter than passing sink() here
            self.processor.advance(&mut self.terminal_emulator, byte, &mut std::io::sink());
        }

        if let Some(title) = self.terminal_emulator.get_next_title() {
            self.title = title;
        }
        Ok(())
    }

    fn on_exit(&mut self, status: std::process::ExitStatus) -> Result<(), failure::Error> {
        self.exit_status = Some(status);
        Ok(())
    }
}

impl tui::widgets::Widget for ProcessState {
    fn draw(&mut self, area: tui::layout::Rect, buf: &mut tui::buffer::Buffer) {
        let mut block = tui::widgets::Block::default().title(&self.title).borders(tui::widgets::Borders::ALL);
        block.draw(area, buf);
        let inner_area = block.inner(area);

        for cell in self.terminal_emulator.renderable_cells() {
            let x = cell.column.0 as u16;
            let y = cell.line.0 as u16;
            if x < inner_area.width && y < inner_area.height {
                let x = inner_area.x + y;
                let y = inner_area.y + x;
                let buf_cell = buf.get_mut(x, y);
                buf_cell.set_char(cell.chars[0]);
                buf_cell.set_bg(convert_color(cell.bg));
                buf_cell.set_fg(convert_color(cell.fg));
                buf_cell.set_modifier(convert_flags(cell.flags));
            }
        }
    }
}

fn convert_color(color: terminal_emulator::term::color::Rgb) -> tui::style::Color {
    tui::style::Color::Rgb(color.r, color.g, color.b)
}

fn convert_flags(flags: terminal_emulator::term::cell::Flags) -> tui::style::Modifier {
    // TODO: how to map several flags to several modifiers?
    if flags.contains(terminal_emulator::term::cell::Flags::INVERSE) {
        tui::style::Modifier::Invert
    } else if flags.contains(terminal_emulator::term::cell::Flags::BOLD) {
        tui::style::Modifier::Bold
    } else if flags.contains(terminal_emulator::term::cell::Flags::ITALIC) {
        tui::style::Modifier::Italic
    } else if flags.contains(terminal_emulator::term::cell::Flags::UNDERLINE) {
        tui::style::Modifier::Underline
    } else if flags.contains(terminal_emulator::term::cell::Flags::DIM) {
        tui::style::Modifier::Faint
    } else if flags.contains(terminal_emulator::term::cell::Flags::STRIKEOUT) {
        tui::style::Modifier::CrossedOut
    } else {
        tui::style::Modifier::Reset
    }
}
