mod ansi;
mod cell;
mod color;
mod index;

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
    title: String,
    mouse_cursor: ansi::MouseCursor,
    cursor_style: Option<ansi::CursorStyle>,
    output: String,
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
        let title = settings.initial_title;
        let output = String::new();
        let mouse_cursor = ansi::MouseCursor::Arrow;
        let cursor_style = None;

        Self { title, output, mouse_cursor, cursor_style }
    }

    fn on_data(&mut self, data: bytes::Bytes) -> Result<(), failure::Error> {
        use std::str;
        self.output.push_str(str::from_utf8(&data)?);
        Ok(())
    }

    fn on_exit(&mut self, status: std::process::ExitStatus) -> Result<(), failure::Error> {
        self.output
            .push_str(&format!("\nprocess exited with {}", status));
        Ok(())
    }
}

impl tui::widgets::Widget for ProcessState {
    fn draw(&mut self, area: tui::layout::Rect, buf: &mut tui::buffer::Buffer) {
        tui::widgets::Paragraph::new([tui::widgets::Text::Raw(self.output.as_str().into())].iter())
            .block(
                tui::widgets::Block::default()
                    .borders(tui::widgets::Borders::ALL)
                    .title(&self.title),
            )
            .draw(area, buf);
    }
}

impl ansi::Handler for ProcessState {

    /// OSC to set window title
    fn set_title(&mut self, title: &str) {
        self.title = title.to_owned();
    }

    /// Set the window's mouse cursor
    fn set_mouse_cursor(&mut self, mouse_cursor: ansi::MouseCursor) {
        self.mouse_cursor = mouse_cursor;
    }

    /// Set the cursor style
    fn set_cursor_style(&mut self, cursor_style: Option<ansi::CursorStyle>) {
        self.cursor_style = cursor_style;
    }

    /// A character to be displayed
    fn input(&mut self, _c: char) {}

    /// Set cursor to position
    fn goto(&mut self, _: index::Line, _: index::Column) {}

    /// Set cursor to specific row
    fn goto_line(&mut self, _: index::Line) {}

    /// Set cursor to specific column
    fn goto_col(&mut self, _: index::Column) {}

    /// Insert blank characters in current line starting from cursor
    fn insert_blank(&mut self, _: index::Column) {}

    /// Move cursor up `rows`
    fn move_up(&mut self, _: index::Line) {}

    /// Move cursor down `rows`
    fn move_down(&mut self, _: index::Line) {}

    /// Identify the terminal (should write back to the pty stream)
    ///
    /// TODO this should probably return an io::Result
    fn identify_terminal<W: std::io::Write>(&mut self, _: &mut W) {}

    // Report device status
    fn device_status<W: std::io::Write>(&mut self, _: &mut W, _: usize) {}

    /// Move cursor forward `cols`
    fn move_forward(&mut self, _: index::Column) {}

    /// Move cursor backward `cols`
    fn move_backward(&mut self, _: index::Column) {}

    /// Move cursor down `rows` and set to column 1
    fn move_down_and_cr(&mut self, _: index::Line) {}

    /// Move cursor up `rows` and set to column 1
    fn move_up_and_cr(&mut self, _: index::Line) {}

    /// Put `count` tabs
    fn put_tab(&mut self, _count: i64) {}

    /// Backspace `count` characters
    fn backspace(&mut self) {}

    /// Carriage return
    fn carriage_return(&mut self) {}

    /// index::Linefeed
    fn linefeed(&mut self) {}

    /// Ring the bell
    ///
    /// Hopefully this is never implemented
    fn bell(&mut self) {}

    /// Substitute char under cursor
    fn substitute(&mut self) {}

    /// Newline
    fn newline(&mut self) {}

    /// Set current position as a tabstop
    fn set_horizontal_tabstop(&mut self) {}

    /// Scroll up `rows` rows
    fn scroll_up(&mut self, _: index::Line) {}

    /// Scroll down `rows` rows
    fn scroll_down(&mut self, _: index::Line) {}

    /// Insert `count` blank lines
    fn insert_blank_lines(&mut self, _: index::Line) {}

    /// Delete `count` lines
    fn delete_lines(&mut self, _: index::Line) {}

    /// Erase `count` chars in current line following cursor
    ///
    /// Erase means resetting to the default state (default colors, no content,
    /// no mode flags)
    fn erase_chars(&mut self, _: index::Column) {}

    /// Delete `count` chars
    ///
    /// Deleting a character is like the delete key on the keyboard - everything
    /// to the right of the deleted things is shifted left.
    fn delete_chars(&mut self, _: index::Column) {}

    /// Move backward `count` tabs
    fn move_backward_tabs(&mut self, _count: i64) {}

    /// Move forward `count` tabs
    fn move_forward_tabs(&mut self, _count: i64) {}

    /// Save current cursor position
    fn save_cursor_position(&mut self) {}

    /// Restore cursor position
    fn restore_cursor_position(&mut self) {}

    /// Clear current line
    fn clear_line(&mut self, _mode: ansi::LineClearMode) {}

    /// Clear screen
    fn clear_screen(&mut self, _mode: ansi::ClearMode) {}

    /// Clear tab stops
    fn clear_tabs(&mut self, _mode: ansi::TabulationClearMode) {}

    /// Reset terminal state
    fn reset_state(&mut self) {}

    /// Reverse Index
    ///
    /// Move the active position to the same horizontal position on the
    /// preceding line. If the active position is at the top margin, a scroll
    /// down is performed
    fn reverse_index(&mut self) {}

    /// set a terminal attribute
    fn terminal_attribute(&mut self, _attr: ansi::Attr) {}

    /// Set mode
    fn set_mode(&mut self, _mode: ansi::Mode) {}

    /// Unset mode
    fn unset_mode(&mut self, _: ansi::Mode) {}

    /// DECSTBM - Set the terminal scrolling region
    fn set_scrolling_region(&mut self, _: std::ops::Range<index::Line>) {}

    /// DECKPAM - Set keypad to applications mode (ESCape instead of digits)
    fn set_keypad_application_mode(&mut self) {}

    /// DECKPNM - Set keypad to numeric mode (digits instead of ESCape seq)
    fn unset_keypad_application_mode(&mut self) {}

    /// Set one of the graphic character sets, G0 to G3, as the active charset.
    ///
    /// 'Invoke' one of G0 to G3 in the GL area. Also referred to as shift in,
    /// shift out and locking shift depending on the set being activated
    fn set_active_charset(&mut self, _: ansi::CharsetIndex) {}

    /// Assign a graphic character set to G0, G1, G2 or G3
    ///
    /// 'Designate' a graphic character set as one of G0 to G3, so that it can
    /// later be 'invoked' by `set_active_charset`
    fn configure_charset(&mut self, _: ansi::CharsetIndex, _: ansi::StandardCharset) {}

    /// Set an indexed color value
    fn set_color(&mut self, _: usize, _: color::Rgb) {}

    /// Reset an indexed color to original value
    fn reset_color(&mut self, _: usize) {}

    /// Set the clipboard
    fn set_clipboard(&mut self, _: &str) {}

    /// Run the dectest routine
    fn dectest(&mut self) {}
}
