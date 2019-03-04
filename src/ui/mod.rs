mod vertical_tabs;

pub struct Ui<B>
where
    B: tui::backend::Backend,
{
    state: State,
    terminal: tui::Terminal<B>,
    last_size: tui::layout::Rect,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Event {
    UserInput(termion::event::Event, bytes::Bytes),
    EndOfUserInput,
    ProcessOutput(usize, bytes::Bytes),
    ProcessExit(usize, std::process::ExitStatus),
    Resized,
}

#[derive(Clone, Debug)]
pub enum Action {
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
    selected: usize,
    scroll: usize,
}

struct ProcessState {
    terminal_emulator: terminal_emulator::term::Term,
    processor: terminal_emulator::Processor,
    title: String,
    exit_status: Option<std::process::ExitStatus>,
    input: Vec<u8>,
}

impl<B> Ui<B>
where
    B: tui::backend::Backend + 'static,
{
    pub fn new(
        terminal: tui::Terminal<B>,
        processes: impl IntoIterator<Item = ProcessSettings>,
    ) -> Result<Self, failure::Error> {
        let processes = processes
            .into_iter()
            .map(ProcessState::from_settings)
            .collect();
        let state = State::new(processes);
        let last_size = terminal.size()?;

        Ok(Self {
            state,
            terminal,
            last_size,
        })
    }

    pub fn check_resized(&mut self) -> bool {
        if let Ok(size) = self.terminal.size() {
            let result = size != self.last_size;
            self.last_size = size;
            result
        } else {
            false
        }
    }

    pub fn on_event(&mut self, event: &Event) -> Result<Vec<Action>, failure::Error> {
        let mut process_input_all = None;
        let process_input_all_ref = &mut process_input_all;

        let state_ref = &mut self.state;
        self.terminal.draw(move |mut frame| {
            match event {
                Event::ProcessOutput(idx, data) => {
                    state_ref.on_data(*idx, data.clone());
                }
                Event::ProcessExit(idx, status) => {
                    state_ref.on_exit(*idx, *status);
                }
                Event::UserInput(event, user_input) => {
                    let handled_input = state_ref.on_user_input(frame.size(), event);
                    if !handled_input {
                        *process_input_all_ref = Some(user_input.clone());
                    }
                }
                _ => {}
            };

            frame.render(state_ref, frame.size());
        })?;

        let result = process_input_all
            .into_iter()
            .map(|data| Action::ProcessInputAll { data })
            .chain(
                self.state
                    .take_process_inputs()
                    .map(|(index, data)| Action::ProcessInput {
                        index,
                        data: data.freeze(),
                    }),
            )
            .collect();

        Ok(result)
    }

    pub fn draw(&mut self) -> Result<(), failure::Error> {
        let state = &mut self.state;
        self.terminal.draw(|mut f| {
            f.render(state, f.size());
        })?;
        Ok(())
    }
}

impl Action {
    pub fn matches_index(&self, other_index: usize) -> bool {
        match *self {
            Action::ProcessInput { index, .. } => index == other_index,
            Action::ProcessInputAll { .. } => true,
            Action::ProcessTermResize { index, .. } => index == other_index,
        }
    }
}

impl State {
    fn new(processes: Vec<ProcessState>) -> Self {
        let selected = 0;
        let scroll = 0;
        Self {
            processes,
            selected,
            scroll,
        }
    }

    fn on_data(&mut self, index: usize, data: bytes::Bytes) {
        self.processes[index].on_data(data)
    }

    fn on_exit(&mut self, index: usize, status: std::process::ExitStatus) {
        self.processes[index].on_exit(status)
    }

    fn on_user_input(&mut self, area: tui::layout::Rect, event: &termion::event::Event) -> bool {
        match *event {
            termion::event::Event::Key(_) => false,
            termion::event::Event::Mouse(m) => {
                let (tabs_area, process_area) = self.layout(area);
                let (x, y) = mouse_event_coords(&m);

                if contains_point(tabs_area, x, y) {
                    match self.tabs().on_mouse_event(tabs_area, &m) {
                        Some(vertical_tabs::MouseAction::Select(selected)) => {
                            self.selected = selected;
                        }
                        Some(vertical_tabs::MouseAction::ScrollUp) => {
                            self.scroll = 0.max(self.scroll as isize - 1) as usize;
                        }
                        Some(vertical_tabs::MouseAction::ScrollDown) => {
                            self.scroll = ((self.processes.len() as isize - area.height as isize
                                + 2)
                            .min(self.scroll as isize)
                                + 1) as usize;
                        }
                        None => {}
                    }
                    true
                } else if contains_point(process_area, x, y) {
                    self.processes[self.selected].on_user_input(process_area, event)
                } else {
                    false
                }
            }
            termion::event::Event::Unsupported(_) => false,
        }
    }

    fn layout(&self, area: tui::layout::Rect) -> (tui::layout::Rect, tui::layout::Rect) {
        let parts = tui::layout::Layout::default()
            .direction(tui::layout::Direction::Horizontal)
            .constraints(
                [
                    tui::layout::Constraint::Length(40),
                    tui::layout::Constraint::Percentage(100),
                ]
                .as_ref(),
            )
            .split(area);

        (parts[0], parts[1])
    }

    fn tabs(&self) -> vertical_tabs::VerticalTabs {
        vertical_tabs::VerticalTabs::default()
            .titles(
                self.processes
                    .iter()
                    .map(|p| p.tab_title())
                    .collect::<Vec<_>>(),
            )
            .block(tui::widgets::Block::default().borders(tui::widgets::Borders::RIGHT))
            .style(tui::style::Style::default())
            .highlight_style(
                tui::style::Style::default()
                    .modifier(tui::style::Modifier::BOLD | tui::style::Modifier::UNDERLINED),
            )
            .select(self.selected)
            .scroll(self.scroll)
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
        let (tabs_area, process_area) = self.layout(area);

        self.tabs().draw(tabs_area, buf);

        self.processes[self.selected].draw(process_area, buf);
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

    fn on_user_input(&mut self, _area: tui::layout::Rect, _event: &termion::event::Event) -> bool {
        true
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

    fn tab_title(&self) -> vertical_tabs::Title {
        let mut title = vertical_tabs::Title::default()
            .text(&self.title)
            .style(tui::style::Style::default());

        if let Some(ref exit_status) = self.exit_status {
            let style = if exit_status.success() {
                tui::style::Style::default()
                    .fg(tui::style::Color::Green)
                    .modifier(tui::style::Modifier::BOLD)
            } else {
                tui::style::Style::default()
                    .fg(tui::style::Color::Red)
                    .modifier(tui::style::Modifier::BOLD)
            };
            let symbol = if let Some(code) = exit_status.code() {
                format!("🗙 {}", code).into()
            } else {
                "☇".into()
            };

            title = title.symbols(vec![tui::widgets::Text::Styled(symbol, style)])
        }

        title
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

        for cell in self.terminal_emulator.renderable_cells() {
            #[allow(clippy::cast_possible_truncation)]
            let x = cell.column.0 as u16;
            #[allow(clippy::cast_possible_truncation)]
            let y = cell.line.0 as u16;
            if x < main_chunk.width && y < main_chunk.height {
                let x = main_chunk.x + x;
                let y = main_chunk.y + y;
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

fn contains_point(rect: tui::layout::Rect, x: u16, y: u16) -> bool {
    rect.x <= x && rect.y <= y && rect.right() > x && rect.bottom() > y
}

fn mouse_event_coords(event: &termion::event::MouseEvent) -> (u16, u16) {
    match event {
        termion::event::MouseEvent::Press(_, x, y) => (x - 1, y - 1),
        termion::event::MouseEvent::Release(x, y) => (x - 1, y - 1),
        termion::event::MouseEvent::Hold(x, y) => (x - 1, y - 1),
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
