// Copyright 2016 Joe Wilm, The Alacritty Project Contributors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
//! Exports the `Term` type which is a high-level API for the Grid
use std::cmp::min;
use std::ops::{Index, IndexMut, Range};
use std::time::{Duration, Instant};
use std::{io, ptr};

use arraydeque::ArrayDeque;
use unicode_width::UnicodeWidthChar;

use crate::ansi::{
    self, Attr, CharsetIndex, Color, CursorStyle, Handler, MouseCursor, NamedColor, StandardCharset,
};
use crate::grid::{
    BidirectionalIterator, DisplayIter, Grid, IndexRegion, Indexed, Scroll, ViewportPosition,
};
use crate::index;
use crate::selection::{self, Locations, Selection};
use crate::term::cell::{Cell, LineLength};

pub mod cell;

/// A type that can expand a given point to a region
///
/// Usually this is implemented for some 2-D array type since
/// points are two dimensional indices.
pub trait Search {
    /// Find the nearest semantic boundary _to the left_ of provided point.
    fn semantic_search_left(&self, _: index::Point<usize>) -> index::Point<usize>;
    /// Find the nearest semantic boundary _to the point_ of provided point.
    fn semantic_search_right(&self, _: index::Point<usize>) -> index::Point<usize>;
    /// Find the nearest URL boundary in both directions.
    fn url_search(&self, _: index::Point<usize>) -> Option<String>;
}

impl Search for Term {
    fn semantic_search_left(&self, mut point: index::Point<usize>) -> index::Point<usize> {
        // Limit the starting point to the last line in the history
        point.line = min(point.line, self.grid.len() - 1);

        let mut iter = self.grid.iter_from(point);
        let last_col = self.grid.num_cols() - index::Column(1);

        while let Some(cell) = iter.prev() {
            if self.semantic_escape_chars.contains(cell.c) {
                break;
            }

            if iter.cur.col == last_col && !cell.flags.contains(cell::Flags::WRAPLINE) {
                break; // cut off if on new line or hit escape char
            }

            point = iter.cur;
        }

        point
    }

    fn semantic_search_right(&self, mut point: index::Point<usize>) -> index::Point<usize> {
        // Limit the starting point to the last line in the history
        point.line = min(point.line, self.grid.len() - 1);

        let mut iter = self.grid.iter_from(point);
        let last_col = self.grid.num_cols() - index::Column(1);

        while let Some(cell) = iter.next() {
            if self.semantic_escape_chars.contains(cell.c) {
                break;
            }

            point = iter.cur;

            if iter.cur.col == last_col && !cell.flags.contains(cell::Flags::WRAPLINE) {
                break; // cut off if on new line or hit escape char
            }
        }

        point
    }

    fn url_search(&self, _: index::Point<usize>) -> Option<String> {
        None // TODO
    }
}

impl selection::Dimensions for Term {
    fn dimensions(&self) -> index::Point {
        index::Point {
            col: self.grid.num_cols(),
            line: self.grid.num_lines(),
        }
    }
}

/// Iterator that yields cells needing render
///
/// Yields cells that require work to be displayed (that is, not a an empty
/// background cell). Additionally, this manages some state of the grid only
/// relevant for rendering like temporarily changing the cell with the cursor.
///
/// This manages the cursor during a render. The cursor location is inverted to
/// draw it, and reverted after drawing to maintain state.
pub struct RenderableCellsIter<'a> {
    inner: DisplayIter<'a, Cell>,
    grid: &'a Grid<Cell>,
    cursor: &'a index::Point,
    cursor_offset: usize,
    mode: TermMode,
    selection: Option<index::RangeInclusive<index::Linear>>,
    cursor_cells: ArrayDeque<[Indexed<Cell>; 3]>,
}

impl<'a> RenderableCellsIter<'a> {
    /// Create the renderable cells iterator
    ///
    /// The cursor and terminal mode are required for properly displaying the
    /// cursor.
    fn new<'b>(
        grid: &'b Grid<Cell>,
        cursor: &'b index::Point,
        mode: TermMode,
        selection: Option<Locations>,
        cursor_style: CursorStyle,
    ) -> RenderableCellsIter<'b> {
        let cursor_offset = grid.line_to_offset(cursor.line);
        let inner = grid.display_iter();

        let mut selection_range = None;
        if let Some(loc) = selection {
            // Get on-screen lines of the selection's locations
            let start_line = grid.buffer_line_to_visible(loc.start.line);
            let end_line = grid.buffer_line_to_visible(loc.end.line);

            // Get start/end locations based on what part of selection is on screen
            let locations = match (start_line, end_line) {
                (ViewportPosition::Visible(start_line), ViewportPosition::Visible(end_line)) => {
                    Some((start_line, loc.start.col, end_line, loc.end.col))
                }
                (ViewportPosition::Visible(start_line), ViewportPosition::Above) => {
                    Some((start_line, loc.start.col, index::Line(0), index::Column(0)))
                }
                (ViewportPosition::Below, ViewportPosition::Visible(end_line)) => {
                    Some((grid.num_lines(), index::Column(0), end_line, loc.end.col))
                }
                (ViewportPosition::Below, ViewportPosition::Above) => Some((
                    grid.num_lines(),
                    index::Column(0),
                    index::Line(0),
                    index::Column(0),
                )),
                _ => None,
            };

            if let Some((start_line, start_col, end_line, end_col)) = locations {
                // start and end *lines* are swapped as we switch from buffer to
                // index::Line coordinates.
                let mut end = index::Point {
                    line: start_line,
                    col: start_col,
                };
                let mut start = index::Point {
                    line: end_line,
                    col: end_col,
                };

                if start > end {
                    ::std::mem::swap(&mut start, &mut end);
                }

                let cols = grid.num_cols();
                let start = index::Linear(start.line.0 * cols.0 + start.col.0);
                let end = index::Linear(end.line.0 * cols.0 + end.col.0);

                // Update the selection
                selection_range = Some(index::RangeInclusive::new(start, end));
            }
        }

        RenderableCellsIter {
            cursor,
            cursor_offset,
            grid,
            inner,
            mode,
            selection: selection_range,
            cursor_cells: ArrayDeque::new(),
        }
        .initialize(cursor_style)
    }

    fn push_cursor_cells(&mut self, original: Cell, cursor: Cell, wide: Cell) {
        // Prints the char under the cell if cursor is situated on a non-empty cell
        self.cursor_cells
            .push_back(Indexed {
                line: self.cursor.line,
                column: self.cursor.col,
                inner: original,
            })
            .expect("won't exceed capacity");

        // Prints the cursor
        self.cursor_cells
            .push_back(Indexed {
                line: self.cursor.line,
                column: self.cursor.col,
                inner: cursor,
            })
            .expect("won't exceed capacity");

        // If cursor is over a wide (2 cell size) character,
        // print the second cursor cell
        if self.is_wide_cursor(&cursor) {
            self.cursor_cells
                .push_back(Indexed {
                    line: self.cursor.line,
                    column: self.cursor.col + 1,
                    inner: wide,
                })
                .expect("won't exceed capacity");
        }
    }

    fn populate_block_cursor(&mut self) {
        let text_color = Color::Named(NamedColor::CursorText);
        let cursor_color = Color::Named(NamedColor::Cursor);

        let original_cell = self.grid[self.cursor];

        let mut cursor_cell = self.grid[self.cursor];
        cursor_cell.fg = text_color;
        cursor_cell.bg = cursor_color;

        let mut wide_cell = cursor_cell;
        wide_cell.c = ' ';

        self.push_cursor_cells(original_cell, cursor_cell, wide_cell);
    }

    fn populate_char_cursor(&mut self, cursor_cell_char: char, wide_cell_char: char) {
        let original_cell = self.grid[self.cursor];

        let mut cursor_cell = self.grid[self.cursor];
        let cursor_color = Color::Named(NamedColor::Cursor);
        cursor_cell.c = cursor_cell_char;
        cursor_cell.fg = cursor_color;

        let mut wide_cell = cursor_cell;
        wide_cell.c = wide_cell_char;

        self.push_cursor_cells(original_cell, cursor_cell, wide_cell);
    }

    fn populate_underline_cursor(&mut self) {
        self.populate_char_cursor('_', '_');
    }

    fn populate_beam_cursor(&mut self) {
        self.populate_char_cursor('|', ' ');
    }

    fn populate_box_cursor(&mut self) {
        self.populate_char_cursor('█', ' ');
    }

    #[inline]
    fn is_wide_cursor(&self, cell: &Cell) -> bool {
        cell.flags.contains(cell::Flags::WIDE_CHAR) && (self.cursor.col + 1) < self.grid.num_cols()
    }

    /// Populates list of cursor cells with the original cell
    fn populate_no_cursor(&mut self) {
        self.cursor_cells
            .push_back(Indexed {
                line: self.cursor.line,
                column: self.cursor.col,
                inner: self.grid[self.cursor],
            })
            .expect("won't exceed capacity");
    }

    fn initialize(mut self, cursor_style: CursorStyle) -> Self {
        if self.cursor_is_visible() {
            match cursor_style {
                CursorStyle::HollowBlock => {
                    self.populate_box_cursor();
                }
                CursorStyle::Block => {
                    self.populate_block_cursor();
                }
                CursorStyle::Beam => {
                    self.populate_beam_cursor();
                }
                CursorStyle::Underline => {
                    self.populate_underline_cursor();
                }
            }
        } else {
            self.populate_no_cursor();
        }
        self
    }

    /// Check if the cursor should be rendered.
    #[inline]
    fn cursor_is_visible(&self) -> bool {
        self.mode.contains(mode::TermMode::SHOW_CURSOR) && self.grid.contains(self.cursor)
    }

    fn compute_fg(&self, fg: Color, cell: &Cell) -> Color {
        use self::cell::Flags;
        match fg {
            Color::Spec(rgb) => Color::Spec(rgb),
            Color::Named(ansi) => {
                match cell.flags & Flags::DIM_BOLD {
                    // If no bright foreground is set, treat it like the BOLD flag doesn't exist
                    self::cell::Flags::DIM_BOLD if ansi == NamedColor::Foreground => {
                        Color::Named(NamedColor::DimForeground)
                    }
                    self::cell::Flags::DIM | self::cell::Flags::DIM_BOLD => {
                        Color::Named(ansi.to_dim())
                    }
                    // None of the above, keep original color.
                    _ => Color::Named(ansi),
                }
            }
            Color::Indexed(idx) => {
                let idx = match (cell.flags & Flags::DIM_BOLD, idx) {
                    (self::cell::Flags::BOLD, 0..=7) => idx + 8,
                    (self::cell::Flags::DIM, 8..=15) => idx - 8,
                    // TODO
                    // (self::cell::Flags::DIM, 0..=7) => idx as usize + 260,
                    _ => idx,
                };

                Color::Indexed(idx)
            }
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct RenderableCell {
    /// A _Display_ line (not necessarily an _Active_ line)
    pub line: index::Line,
    pub column: index::Column,
    pub chars: [char; cell::MAX_ZEROWIDTH_CHARS + 1],
    pub fg: Color,
    pub bg: Color,
    pub flags: cell::Flags,
}

impl<'a> Iterator for RenderableCellsIter<'a> {
    type Item = RenderableCell;

    /// Gets the next renderable cell
    ///
    /// Skips empty (background) cells and applies any flags to the cell state
    /// (eg. invert fg and bg colors).
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Handle cursor
            let cell = if self.cursor_offset == self.inner.offset()
                && self.inner.column() == self.cursor.col
            {
                // Cursor cell
                let mut cell = self.cursor_cells.pop_front().unwrap();
                cell.line = self.inner.line();

                // Since there may be multiple cursor cells (for a wide
                // char), only update iteration position after all cursor
                // cells have been drawn.
                if self.cursor_cells.is_empty() {
                    self.inner.next();
                }
                cell
            } else {
                use crate::index::Contains;

                let cell = self.inner.next()?;

                let index = index::Linear(cell.line.0 * self.grid.num_cols().0 + cell.column.0);

                let selected = self
                    .selection
                    .as_ref()
                    .map(|range| range.contains_(index))
                    .unwrap_or(false);

                // Skip empty cells
                if cell.is_empty() && !selected {
                    continue;
                }

                cell
            };

            // Apply inversion and lookup RGB values
            let fg = self.compute_fg(cell.fg, &cell);
            let bg = cell.bg;

            return Some(RenderableCell {
                line: cell.line,
                column: cell.column,
                flags: cell.flags,
                chars: cell.chars(),
                fg,
                bg,
            });
        }
    }
}

pub mod mode {
    use bitflags::bitflags;

    bitflags! {
        pub struct TermMode: u16 {
            const SHOW_CURSOR         = 0b00_0000_0000_0001;
            const APP_CURSOR          = 0b00_0000_0000_0010;
            const APP_KEYPAD          = 0b00_0000_0000_0100;
            const MOUSE_REPORT_CLICK  = 0b00_0000_0000_1000;
            const BRACKETED_PASTE     = 0b00_0000_0001_0000;
            const SGR_MOUSE           = 0b00_0000_0010_0000;
            const MOUSE_MOTION        = 0b00_0000_0100_0000;
            const LINE_WRAP           = 0b00_0000_1000_0000;
            const LINE_FEED_NEW_LINE  = 0b00_0001_0000_0000;
            const ORIGIN              = 0b00_0010_0000_0000;
            const INSERT              = 0b00_0100_0000_0000;
            const FOCUS_IN_OUT        = 0b00_1000_0000_0000;
            const ALT_SCREEN          = 0b01_0000_0000_0000;
            const MOUSE_DRAG          = 0b10_0000_0000_0000;
            const ANY                 = 0b11_1111_1111_1111;
            const NONE                = 0;
        }
    }

    impl Default for TermMode {
        fn default() -> TermMode {
            TermMode::SHOW_CURSOR | TermMode::LINE_WRAP
        }
    }
}

pub use self::mode::TermMode;

trait CharsetMapping {
    fn map(&self, c: char) -> char {
        c
    }
}

impl CharsetMapping for StandardCharset {
    /// Switch/Map character to the active charset. Ascii is the common case and
    /// for that we want to do as little as possible.
    #[inline]
    fn map(&self, c: char) -> char {
        match *self {
            StandardCharset::Ascii => c,
            StandardCharset::SpecialCharacterAndLineDrawing => match c {
                '`' => '◆',
                'a' => '▒',
                'b' => '\t',
                'c' => '\u{000c}',
                'd' => '\r',
                'e' => '\n',
                'f' => '°',
                'g' => '±',
                'h' => '\u{2424}',
                'i' => '\u{000b}',
                'j' => '┘',
                'k' => '┐',
                'l' => '┌',
                'm' => '└',
                'n' => '┼',
                'o' => '⎺',
                'p' => '⎻',
                'q' => '─',
                'r' => '⎼',
                's' => '⎽',
                't' => '├',
                'u' => '┤',
                'v' => '┴',
                'w' => '┬',
                'x' => '│',
                'y' => '≤',
                'z' => '≥',
                '{' => 'π',
                '|' => '≠',
                '}' => '£',
                '~' => '·',
                _ => c,
            },
        }
    }
}

#[derive(Default, Copy, Clone)]
struct Charsets([StandardCharset; 4]);

impl Index<CharsetIndex> for Charsets {
    type Output = StandardCharset;
    fn index(&self, index: CharsetIndex) -> &StandardCharset {
        &self.0[index as usize]
    }
}

impl IndexMut<CharsetIndex> for Charsets {
    fn index_mut(&mut self, index: CharsetIndex) -> &mut StandardCharset {
        &mut self.0[index as usize]
    }
}

#[derive(Default, Copy, Clone)]
pub struct Cursor {
    /// The location of this cursor
    pub point: index::Point,

    /// Template cell when using this cursor
    template: Cell,

    /// Currently configured graphic character sets
    charsets: Charsets,
}

pub struct VisualBell {
    /// Visual bell duration
    duration: Duration,

    /// The last time the visual bell rang, if at all
    start_time: Option<Instant>,
}

impl VisualBell {
    pub fn new() -> VisualBell {
        VisualBell {
            duration: Duration::from_secs(1),
            start_time: None,
        }
    }

    /// Ring the visual bell, and return its intensity.
    pub fn ring(&mut self) -> f64 {
        let now = Instant::now();
        self.start_time = Some(now);
        0.0
    }

    /// Get the currently intensity of the visual bell. The bell's intensity
    /// ramps down from 1.0 to 0.0 at a rate determined by the bell's duration.
    pub fn intensity(&self) -> f64 {
        0.0
    }

    /// Check whether or not the visual bell has completed "ringing".
    pub fn completed(&mut self) -> bool {
        match self.start_time {
            Some(earlier) => {
                if Instant::now().duration_since(earlier) >= self.duration {
                    self.start_time = None;
                }
                false
            }
            None => true,
        }
    }
}

pub struct Term {
    /// The grid
    grid: Grid<Cell>,

    /// Tracks if the next call to input will need to first handle wrapping.
    /// This is true after the last column is set with the input function. Any function that
    /// implicitly sets the line or column needs to set this to false to avoid wrapping twice.
    /// input_needs_wrap ensures that cursor.col is always valid for use into indexing into
    /// arrays. Without it we would have to sanitize cursor.col every time we used it.
    input_needs_wrap: bool,

    /// Got a request to set title; it's buffered here until next draw.
    ///
    /// Would be nice to avoid the allocation...
    next_title: Option<String>,

    /// Got a request to set the mouse cursor; it's buffered here until the next draw
    next_mouse_cursor: Option<MouseCursor>,

    /// Alternate grid
    alt_grid: Grid<Cell>,

    /// Alt is active
    alt: bool,

    /// The cursor
    cursor: Cursor,

    /// The graphic character set, out of `charsets`, which ASCII is currently
    /// being mapped to
    active_charset: CharsetIndex,

    /// Tabstops
    tabs: TabStops,

    /// Mode flags
    mode: TermMode,

    /// Scroll region
    scroll_region: Range<index::Line>,

    /// Size
    size_info: SizeInfo,

    pub dirty: bool,

    pub visual_bell: VisualBell,
    pub next_is_urgent: Option<bool>,

    /// Saved cursor from main grid
    cursor_save: Cursor,

    /// Saved cursor from alt grid
    cursor_save_alt: Cursor,

    semantic_escape_chars: String,

    /// Current style of the cursor
    cursor_style: Option<CursorStyle>,

    /// Default style for resetting the cursor
    default_cursor_style: CursorStyle,

    dynamic_title: bool,

    /// Number of spaces in one tab
    tabspaces: usize,

    /// Automatically scroll to bottom when new lines are added
    auto_scroll: bool,

    /// Hint that Alacritty should be closed
    should_exit: bool,
}

/// Terminal size info
#[derive(Debug, Copy, Clone)]
pub struct SizeInfo {
    /// Terminal window width
    pub width: f32,

    /// Terminal window height
    pub height: f32,

    /// Width of individual cell
    pub cell_width: f32,

    /// Height of individual cell
    pub cell_height: f32,

    /// Horizontal window padding
    pub padding_x: f32,

    /// Horizontal window padding
    pub padding_y: f32,

    /// DPI factor of the current window
    pub dpr: f64,
}

impl SizeInfo {
    #[inline]
    pub fn lines(&self) -> index::Line {
        index::Line(((self.height - 2. * self.padding_y) / self.cell_height) as usize)
    }

    #[inline]
    pub fn cols(&self) -> index::Column {
        index::Column(((self.width - 2. * self.padding_x) / self.cell_width) as usize)
    }

    pub fn contains_point(&self, x: usize, y: usize) -> bool {
        x < (self.width - self.padding_x) as usize
            && x >= self.padding_x as usize
            && y < (self.height - self.padding_y) as usize
            && y >= self.padding_y as usize
    }

    pub fn pixels_to_coords(&self, x: usize, y: usize) -> index::Point {
        let col =
            index::Column(x.saturating_sub(self.padding_x as usize) / (self.cell_width as usize));
        let line =
            index::Line(y.saturating_sub(self.padding_y as usize) / (self.cell_height as usize));

        index::Point {
            line: min(line, index::Line(self.lines().saturating_sub(1))),
            col: min(col, index::Column(self.cols().saturating_sub(1))),
        }
    }
}

impl Term {
    pub fn selection(&self) -> &Option<Selection> {
        &self.grid.selection
    }

    pub fn selection_mut(&mut self) -> &mut Option<Selection> {
        &mut self.grid.selection
    }

    #[inline]
    pub fn get_next_title(&mut self) -> Option<String> {
        self.next_title.take()
    }

    pub fn scroll_display(&mut self, scroll: Scroll) {
        self.grid.scroll_display(scroll);
        self.dirty = true;
    }

    #[inline]
    pub fn get_next_mouse_cursor(&mut self) -> Option<MouseCursor> {
        self.next_mouse_cursor.take()
    }

    pub fn new(size: SizeInfo) -> Term {
        let num_cols = size.cols();
        let num_lines = size.lines();

        let semantic_escape_chars = "".to_owned();
        let history_size = 1024; // TODO
        let default_cursor_style = ansi::CursorStyle::Block;
        let dynamic_title = true;
        let auto_scroll = true;
        let grid = Grid::new(num_lines, num_cols, history_size, Cell::default());
        let alt = Grid::new(
            num_lines,
            num_cols,
            0, /* scroll history */
            Cell::default(),
        );

        let tabspaces = 4;
        let tabs = TabStops::new(grid.num_cols(), tabspaces);

        let scroll_region = index::Line(0)..grid.num_lines();

        Term {
            next_title: None,
            next_mouse_cursor: None,
            dirty: false,
            visual_bell: VisualBell::new(),
            next_is_urgent: None,
            input_needs_wrap: false,
            grid,
            alt_grid: alt,
            alt: false,
            active_charset: Default::default(),
            cursor: Default::default(),
            cursor_save: Default::default(),
            cursor_save_alt: Default::default(),
            tabs,
            mode: Default::default(),
            scroll_region,
            size_info: size,
            semantic_escape_chars,
            cursor_style: None,
            default_cursor_style,
            dynamic_title,
            tabspaces,
            auto_scroll,
            should_exit: false,
        }
    }

    #[inline]
    pub fn needs_draw(&self) -> bool {
        self.dirty
    }

    pub fn selection_to_string(&self) -> Option<String> {
        /// Need a generic push() for the Append trait
        trait PushChar {
            fn push_char(&mut self, c: char);
            fn maybe_newline(&mut self, grid: &Grid<Cell>, line: usize, ending: index::Column) {
                if ending != index::Column(0)
                    && !grid[line][ending - 1].flags.contains(cell::Flags::WRAPLINE)
                {
                    self.push_char('\n');
                }
            }
        }

        impl PushChar for String {
            #[inline]
            fn push_char(&mut self, c: char) {
                self.push(c);
            }
        }

        use std::ops::Range;

        trait Append: PushChar {
            fn append(
                &mut self,
                grid: &Grid<Cell>,
                tabs: &TabStops,
                line: usize,
                cols: Range<index::Column>,
            );
        }

        impl Append for String {
            fn append(
                &mut self,
                grid: &Grid<Cell>,
                tabs: &TabStops,
                mut line: usize,
                cols: Range<index::Column>,
            ) {
                // Select until last line still within the buffer
                line = min(line, grid.len() - 1);

                let grid_line = &grid[line];
                let line_length = grid_line.line_length();
                let line_end = min(line_length, cols.end + 1);

                if line_end.0 == 0 && cols.end >= grid.num_cols() - 1 {
                    self.push('\n');
                } else if cols.start < line_end {
                    let mut tab_mode = false;

                    for col in index::Range::from(cols.start..line_end) {
                        let cell = grid_line[col];

                        if tab_mode {
                            // Skip over whitespace until next tab-stop once a tab was found
                            if tabs[col] {
                                tab_mode = false;
                            } else if cell.c == ' ' {
                                continue;
                            }
                        }

                        if !cell.flags.contains(cell::Flags::WIDE_CHAR_SPACER) {
                            self.push(cell.c);
                            for c in (&cell.chars()[1..]).iter().filter(|c| **c != ' ') {
                                self.push(*c);
                            }
                        }

                        if cell.c == '\t' {
                            tab_mode = true;
                        }
                    }

                    if cols.end >= grid.num_cols() - 1 {
                        self.maybe_newline(grid, line, line_end);
                    }
                }
            }
        }

        let alt_screen = self.mode.contains(TermMode::ALT_SCREEN);
        let selection = self.grid.selection.clone()?;
        let span = selection.to_span(self, alt_screen)?;

        let mut res = String::new();

        let Locations { mut start, mut end } = span.to_locations();

        if start > end {
            ::std::mem::swap(&mut start, &mut end);
        }

        let line_count = end.line - start.line;
        let max_col = index::Column(usize::max_value() - 1);

        match line_count {
            // Selection within single line
            0 => {
                res.append(&self.grid, &self.tabs, start.line, start.col..end.col);
            }

            // Selection ends on line following start
            1 => {
                // Ending line
                res.append(&self.grid, &self.tabs, end.line, end.col..max_col);

                // Starting line
                res.append(
                    &self.grid,
                    &self.tabs,
                    start.line,
                    index::Column(0)..start.col,
                );
            }

            // Multi line selection
            _ => {
                // Ending line
                res.append(&self.grid, &self.tabs, end.line, end.col..max_col);

                let middle_range = (start.line + 1)..(end.line);
                for line in middle_range.rev() {
                    res.append(&self.grid, &self.tabs, line, index::Column(0)..max_col);
                }

                // Starting line
                res.append(
                    &self.grid,
                    &self.tabs,
                    start.line,
                    index::Column(0)..start.col,
                );
            }
        }

        Some(res)
    }

    /// Convert the given pixel values to a grid coordinate
    ///
    /// The mouse coordinates are expected to be relative to the top left. The
    /// line and column returned are also relative to the top left.
    ///
    /// Returns None if the coordinates are outside the screen
    pub fn pixels_to_coords(&self, x: usize, y: usize) -> Option<index::Point> {
        if self.size_info.contains_point(x, y) {
            Some(self.size_info.pixels_to_coords(x, y))
        } else {
            None
        }
    }

    /// Access to the raw grid data structure
    ///
    /// This is a bit of a hack; when the window is closed, the event processor
    /// serializes the grid state to a file.
    pub fn grid(&self) -> &Grid<Cell> {
        &self.grid
    }

    // Mutable access for swapping out the grid during tests
    #[cfg(test)]
    pub fn grid_mut(&mut self) -> &mut Grid<Cell> {
        &mut self.grid
    }

    /// Iterate over the *renderable* cells in the terminal
    ///
    /// A renderable cell is any cell which has content other than the default
    /// background color.  Cells with an alternate background color are
    /// considered renderable as are cells with any text content.
    pub fn renderable_cells(&self) -> RenderableCellsIter {
        let alt_screen = self.mode.contains(TermMode::ALT_SCREEN);
        let selection = self
            .grid
            .selection
            .as_ref()
            .and_then(|s| s.to_span(self, alt_screen))
            .map(|span| span.to_locations());

        let cursor = self.cursor_style.unwrap_or(self.default_cursor_style);

        RenderableCellsIter::new(&self.grid, &self.cursor.point, self.mode, selection, cursor)
    }

    /// Resize terminal to new dimensions
    pub fn resize(&mut self, size: &SizeInfo) {
        debug!("Resizing terminal");

        // Bounds check; lots of math assumes width and height are > 0
        if size.width as usize <= 2 * self.size_info.padding_x as usize
            || size.height as usize <= 2 * self.size_info.padding_y as usize
        {
            return;
        }

        let old_cols = self.grid.num_cols();
        let old_lines = self.grid.num_lines();
        let mut num_cols = size.cols();
        let mut num_lines = size.lines();

        self.size_info = *size;

        if old_cols == num_cols && old_lines == num_lines {
            debug!("Term::resize dimensions unchanged");
            return;
        }

        self.grid.selection = None;
        self.alt_grid.selection = None;

        // Should not allow less than 1 col, causes all sorts of checks to be required.
        if num_cols <= index::Column(1) {
            num_cols = index::Column(2);
        }

        // Should not allow less than 1 line, causes all sorts of checks to be required.
        if num_lines <= index::Line(1) {
            num_lines = index::Line(2);
        }

        // Scroll up to keep cursor in terminal
        if self.cursor.point.line >= num_lines {
            let lines = self.cursor.point.line - num_lines + 1;
            self.grid
                .scroll_up(&(index::Line(0)..old_lines), lines, &self.cursor.template);
        }

        // Scroll up alt grid as well
        if self.cursor_save_alt.point.line >= num_lines {
            let lines = self.cursor_save_alt.point.line - num_lines + 1;
            self.alt_grid.scroll_up(
                &(index::Line(0)..old_lines),
                lines,
                &self.cursor_save_alt.template,
            );
        }

        // Move prompt down when growing if scrollback lines are available
        if num_lines > old_lines {
            if self.mode.contains(TermMode::ALT_SCREEN) {
                let growage = min(
                    num_lines - old_lines,
                    index::Line(self.alt_grid.scroll_limit()),
                );
                self.cursor_save.point.line += growage;
            } else {
                let growage = min(num_lines - old_lines, index::Line(self.grid.scroll_limit()));
                self.cursor.point.line += growage;
            }
        }

        debug!(
            "New num_cols is {} and num_lines is {}",
            num_cols, num_lines
        );

        // Resize grids to new size
        self.grid.resize(num_lines, num_cols, &Cell::default());
        self.alt_grid.resize(num_lines, num_cols, &Cell::default());

        // Reset scrolling region to new size
        self.scroll_region = index::Line(0)..self.grid.num_lines();

        // Ensure cursors are in-bounds.
        self.cursor.point.col = min(self.cursor.point.col, num_cols - 1);
        self.cursor.point.line = min(self.cursor.point.line, num_lines - 1);
        self.cursor_save.point.col = min(self.cursor_save.point.col, num_cols - 1);
        self.cursor_save.point.line = min(self.cursor_save.point.line, num_lines - 1);
        self.cursor_save_alt.point.col = min(self.cursor_save_alt.point.col, num_cols - 1);
        self.cursor_save_alt.point.line = min(self.cursor_save_alt.point.line, num_lines - 1);

        // Recreate tabs list
        self.tabs = TabStops::new(self.grid.num_cols(), self.tabspaces);
    }

    #[inline]
    pub fn size_info(&self) -> &SizeInfo {
        &self.size_info
    }

    #[inline]
    pub fn mode(&self) -> &TermMode {
        &self.mode
    }

    #[inline]
    pub fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    pub fn swap_alt(&mut self) {
        if self.alt {
            let template = &self.cursor.template;
            self.grid.region_mut(..).each(|c| c.reset(template));
        }

        self.alt = !self.alt;
        ::std::mem::swap(&mut self.grid, &mut self.alt_grid);
    }

    /// Scroll screen down
    ///
    /// Text moves down; clear at bottom
    /// Expects origin to be in scroll range.
    #[inline]
    fn scroll_down_relative(&mut self, origin: index::Line, mut lines: index::Line) {
        trace!(
            "Scrolling down relative: origin={}, lines={}",
            origin,
            lines
        );
        lines = min(lines, self.scroll_region.end - self.scroll_region.start);
        lines = min(lines, self.scroll_region.end - origin);

        // Scroll between origin and bottom
        self.grid.scroll_down(
            &(origin..self.scroll_region.end),
            lines,
            &self.cursor.template,
        );
    }

    /// Scroll screen up
    ///
    /// Text moves up; clear at top
    /// Expects origin to be in scroll range.
    #[inline]
    fn scroll_up_relative(&mut self, origin: index::Line, lines: index::Line) {
        trace!("Scrolling up relative: origin={}, lines={}", origin, lines);
        let lines = min(lines, self.scroll_region.end - self.scroll_region.start);

        // Scroll from origin to bottom less number of lines
        self.grid.scroll_up(
            &(origin..self.scroll_region.end),
            lines,
            &self.cursor.template,
        );
    }

    fn deccolm(&mut self) {
        // Setting 132 column font makes no sense, but run the other side effects
        // Clear scrolling region
        let scroll_region = index::Line(0)..self.grid.num_lines();
        self.set_scrolling_region(scroll_region);

        // Clear grid
        let template = self.cursor.template;
        self.grid.region_mut(..).each(|c| c.reset(&template));
    }

    #[inline]
    pub fn exit(&mut self) {
        self.should_exit = true;
    }

    #[inline]
    pub fn should_exit(&self) -> bool {
        self.should_exit
    }
}

impl ansi::TermInfo for Term {
    #[inline]
    fn lines(&self) -> index::Line {
        self.grid.num_lines()
    }

    #[inline]
    fn cols(&self) -> index::Column {
        self.grid.num_cols()
    }
}

impl ansi::Handler for Term {
    /// Set the window title
    #[inline]
    fn set_title(&mut self, title: &str) {
        if self.dynamic_title {
            self.next_title = Some(title.to_owned());
        }
    }

    /// Set the mouse cursor
    #[inline]
    fn set_mouse_cursor(&mut self, cursor: MouseCursor) {
        self.next_mouse_cursor = Some(cursor);
    }

    #[inline]
    fn set_cursor_style(&mut self, style: Option<CursorStyle>) {
        trace!("Setting cursor style {:?}", style);
        self.cursor_style = style;
    }

    /// A character to be displayed
    #[inline]
    fn input(&mut self, c: char) {
        // If enabled, scroll to bottom when character is received
        if self.auto_scroll {
            self.scroll_display(Scroll::Bottom);
        }

        if self.input_needs_wrap {
            if !self.mode.contains(mode::TermMode::LINE_WRAP) {
                return;
            }

            trace!("Wrapping input");

            {
                let location = index::Point {
                    line: self.cursor.point.line,
                    col: self.cursor.point.col,
                };

                let cell = &mut self.grid[&location];
                cell.flags.insert(cell::Flags::WRAPLINE);
            }

            if (self.cursor.point.line + 1) >= self.scroll_region.end {
                self.linefeed();
            } else {
                self.cursor.point.line += 1;
            }

            self.cursor.point.col = index::Column(0);
            self.input_needs_wrap = false;
        }

        // Number of cells the char will occupy
        if let Some(width) = c.width() {
            let num_cols = self.grid.num_cols();

            // If in insert mode, first shift cells to the right.
            if self.mode.contains(mode::TermMode::INSERT)
                && self.cursor.point.col + width < num_cols
            {
                let line = self.cursor.point.line;
                let col = self.cursor.point.col;
                let line = &mut self.grid[line];

                let src = line[col..].as_ptr();
                let dst = line[(col + width)..].as_mut_ptr();
                unsafe {
                    // memmove
                    ptr::copy(src, dst, (num_cols - col - width).0);
                }
            }

            // Handle zero-width characters
            if width == 0 {
                let col = self.cursor.point.col.0.saturating_sub(1);
                let line = self.cursor.point.line;
                if self.grid[line][index::Column(col)]
                    .flags
                    .contains(cell::Flags::WIDE_CHAR_SPACER)
                {
                    col.saturating_sub(1);
                }
                self.grid[line][index::Column(col)].push_extra(c);
                return;
            }

            let cell = &mut self.grid[&self.cursor.point];
            *cell = self.cursor.template;
            cell.c = self.cursor.charsets[self.active_charset].map(c);

            // Handle wide chars
            if width == 2 {
                cell.flags.insert(cell::Flags::WIDE_CHAR);

                if self.cursor.point.col + 1 < num_cols {
                    self.cursor.point.col += 1;
                    let spacer = &mut self.grid[&self.cursor.point];
                    *spacer = self.cursor.template;
                    spacer.flags.insert(cell::Flags::WIDE_CHAR_SPACER);
                }
            }
        }

        if (self.cursor.point.col + 1) < self.grid.num_cols() {
            self.cursor.point.col += 1;
        } else {
            self.input_needs_wrap = true;
        }
    }

    #[inline]
    fn goto(&mut self, line: index::Line, col: index::Column) {
        trace!("Going to: line={}, col={}", line, col);
        let (y_offset, max_y) = if self.mode.contains(mode::TermMode::ORIGIN) {
            (self.scroll_region.start, self.scroll_region.end - 1)
        } else {
            (index::Line(0), self.grid.num_lines() - 1)
        };

        self.cursor.point.line = min(line + y_offset, max_y);
        self.cursor.point.col = min(col, self.grid.num_cols() - 1);
        self.input_needs_wrap = false;
    }

    #[inline]
    fn goto_line(&mut self, line: index::Line) {
        trace!("Going to line: {}", line);
        self.goto(line, self.cursor.point.col)
    }

    #[inline]
    fn goto_col(&mut self, col: index::Column) {
        trace!("Going to column: {}", col);
        self.goto(self.cursor.point.line, col)
    }

    #[inline]
    fn insert_blank(&mut self, count: index::Column) {
        // Ensure inserting within terminal bounds

        let count = min(count, self.size_info.cols() - self.cursor.point.col);

        let source = self.cursor.point.col;
        let destination = self.cursor.point.col + count;
        let num_cells = (self.size_info.cols() - destination).0;

        let line = &mut self.grid[self.cursor.point.line];

        unsafe {
            let src = line[source..].as_ptr();
            let dst = line[destination..].as_mut_ptr();

            ptr::copy(src, dst, num_cells);
        }

        // Cells were just moved out towards the end of the line; fill in
        // between source and dest with blanks.
        let template = self.cursor.template;
        for c in &mut line[source..destination] {
            c.reset(&template);
        }
    }

    #[inline]
    fn move_up(&mut self, lines: index::Line) {
        trace!("Moving up: {}", lines);
        let move_to = index::Line(self.cursor.point.line.0.saturating_sub(lines.0));
        self.goto(move_to, self.cursor.point.col)
    }

    #[inline]
    fn move_down(&mut self, lines: index::Line) {
        trace!("Moving down: {}", lines);
        let move_to = self.cursor.point.line + lines;
        self.goto(move_to, self.cursor.point.col)
    }

    #[inline]
    fn identify_terminal<W: io::Write>(&mut self, writer: &mut W) {
        let _ = writer.write_all(b"\x1b[?6c");
    }

    #[inline]
    fn device_status<W: io::Write>(&mut self, writer: &mut W, arg: usize) {
        trace!("Reporting device status: {}", arg);
        match arg {
            5 => {
                let _ = writer.write_all(b"\x1b[0n");
            }
            6 => {
                let pos = self.cursor.point;
                let _ = write!(writer, "\x1b[{};{}R", pos.line + 1, pos.col + 1);
            }
            _ => debug!("unknown device status query: {}", arg),
        };
    }

    #[inline]
    fn move_forward(&mut self, cols: index::Column) {
        trace!("Moving forward: {}", cols);
        self.cursor.point.col = min(self.cursor.point.col + cols, self.grid.num_cols() - 1);
        self.input_needs_wrap = false;
    }

    #[inline]
    fn move_backward(&mut self, cols: index::Column) {
        trace!("Moving backward: {}", cols);
        self.cursor.point.col -= min(self.cursor.point.col, cols);
        self.input_needs_wrap = false;
    }

    #[inline]
    fn move_down_and_cr(&mut self, lines: index::Line) {
        trace!("Moving down and cr: {}", lines);
        let move_to = self.cursor.point.line + lines;
        self.goto(move_to, index::Column(0))
    }

    #[inline]
    fn move_up_and_cr(&mut self, lines: index::Line) {
        trace!("Moving up and cr: {}", lines);
        let move_to = index::Line(self.cursor.point.line.0.saturating_sub(lines.0));
        self.goto(move_to, index::Column(0))
    }

    #[inline]
    fn put_tab(&mut self, mut count: i64) {
        trace!("Putting tab: {}", count);

        while self.cursor.point.col < self.grid.num_cols() && count != 0 {
            count -= 1;

            let cell = &mut self.grid[&self.cursor.point];
            if cell.c == ' ' {
                cell.c = self.cursor.charsets[self.active_charset].map('\t');
            }

            loop {
                if (self.cursor.point.col + 1) == self.grid.num_cols() {
                    break;
                }

                self.cursor.point.col += 1;

                if self.tabs[self.cursor.point.col] {
                    break;
                }
            }
        }

        self.input_needs_wrap = false;
    }

    /// Backspace `count` characters
    #[inline]
    fn backspace(&mut self) {
        trace!("Backspace");
        if self.cursor.point.col > index::Column(0) {
            self.cursor.point.col -= 1;
            self.input_needs_wrap = false;
        }
    }

    /// Carriage return
    #[inline]
    fn carriage_return(&mut self) {
        trace!("Carriage return");
        self.cursor.point.col = index::Column(0);
        self.input_needs_wrap = false;
    }

    /// Linefeed
    #[inline]
    fn linefeed(&mut self) {
        trace!("Linefeed");
        let next = self.cursor.point.line + 1;
        if next == self.scroll_region.end {
            self.scroll_up(index::Line(1));
        } else if next < self.grid.num_lines() {
            self.cursor.point.line += 1;
        }
    }

    /// Set current position as a tabstop
    #[inline]
    fn bell(&mut self) {
        trace!("Bell");
        self.visual_bell.ring();
        self.next_is_urgent = Some(true);
    }

    #[inline]
    fn substitute(&mut self) {
        trace!("[unimplemented] Substitute");
    }

    /// Run LF/NL
    ///
    /// LF/NL mode has some interesting history. According to ECMA-48 4th
    /// edition, in LINE FEED mode,
    ///
    /// > The execution of the formatter functions LINE FEED (LF), FORM FEED
    /// (FF), LINE TABULATION (VT) cause only movement of the active position in
    /// the direction of the line progression.
    ///
    /// In NEW LINE mode,
    ///
    /// > The execution of the formatter functions LINE FEED (LF), FORM FEED
    /// (FF), LINE TABULATION (VT) cause movement to the line home position on
    /// the following line, the following form, etc. In the case of LF this is
    /// referred to as the New index::Line (NL) option.
    ///
    /// Additionally, ECMA-48 4th edition says that this option is deprecated.
    /// ECMA-48 5th edition only mentions this option (without explanation)
    /// saying that it's been removed.
    ///
    /// As an emulator, we need to support it since applications may still rely
    /// on it.
    #[inline]
    fn newline(&mut self) {
        self.linefeed();

        if self.mode.contains(mode::TermMode::LINE_FEED_NEW_LINE) {
            self.carriage_return();
        }
    }

    #[inline]
    fn set_horizontal_tabstop(&mut self) {
        trace!("Setting horizontal tabstop");
        let column = self.cursor.point.col;
        self.tabs[column] = true;
    }

    #[inline]
    fn scroll_up(&mut self, lines: index::Line) {
        let origin = self.scroll_region.start;
        self.scroll_up_relative(origin, lines);
    }

    #[inline]
    fn scroll_down(&mut self, lines: index::Line) {
        let origin = self.scroll_region.start;
        self.scroll_down_relative(origin, lines);
    }

    #[inline]
    fn insert_blank_lines(&mut self, lines: index::Line) {
        use crate::index::Contains;
        trace!("Inserting blank {} lines", lines);
        if self.scroll_region.contains_(self.cursor.point.line) {
            let origin = self.cursor.point.line;
            self.scroll_down_relative(origin, lines);
        }
    }

    #[inline]
    fn delete_lines(&mut self, lines: index::Line) {
        use crate::index::Contains;
        trace!("Deleting {} lines", lines);
        if self.scroll_region.contains_(self.cursor.point.line) {
            let origin = self.cursor.point.line;
            self.scroll_up_relative(origin, lines);
        }
    }

    #[inline]
    fn erase_chars(&mut self, count: index::Column) {
        trace!(
            "Erasing chars: count={}, col={}",
            count,
            self.cursor.point.col
        );
        let start = self.cursor.point.col;
        let end = min(start + count, self.grid.num_cols());

        let row = &mut self.grid[self.cursor.point.line];
        let template = self.cursor.template; // Cleared cells have current background color set
        for c in &mut row[start..end] {
            c.reset(&template);
        }
    }

    #[inline]
    fn delete_chars(&mut self, count: index::Column) {
        // Ensure deleting within terminal bounds
        let count = min(count, self.size_info.cols());

        let start = self.cursor.point.col;
        let end = min(start + count, self.grid.num_cols() - 1);
        let n = (self.size_info.cols() - end).0;

        let line = &mut self.grid[self.cursor.point.line];

        unsafe {
            let src = line[end..].as_ptr();
            let dst = line[start..].as_mut_ptr();

            ptr::copy(src, dst, n);
        }

        // Clear last `count` cells in line. If deleting 1 char, need to delete
        // 1 cell.
        let template = self.cursor.template;
        let end = self.size_info.cols() - count;
        for c in &mut line[end..] {
            c.reset(&template);
        }
    }

    #[inline]
    fn move_backward_tabs(&mut self, count: i64) {
        trace!("Moving backward {} tabs", count);

        for _ in 0..count {
            let mut col = self.cursor.point.col;
            for i in (0..(col.0)).rev() {
                if self.tabs[index::Column(i)] {
                    col = index::Column(i);
                    break;
                }
            }
            self.cursor.point.col = col;
        }
    }

    #[inline]
    fn move_forward_tabs(&mut self, count: i64) {
        trace!("[unimplemented] Moving forward {} tabs", count);
    }

    #[inline]
    fn save_cursor_position(&mut self) {
        trace!("Saving cursor position");
        let cursor = if self.alt {
            &mut self.cursor_save_alt
        } else {
            &mut self.cursor_save
        };

        *cursor = self.cursor;
    }

    #[inline]
    fn restore_cursor_position(&mut self) {
        trace!("Restoring cursor position");
        let source = if self.alt {
            &self.cursor_save_alt
        } else {
            &self.cursor_save
        };

        self.cursor = *source;
        self.cursor.point.line = min(self.cursor.point.line, self.grid.num_lines() - 1);
        self.cursor.point.col = min(self.cursor.point.col, self.grid.num_cols() - 1);
    }

    #[inline]
    fn clear_line(&mut self, mode: ansi::LineClearMode) {
        trace!("Clearing line: {:?}", mode);
        let mut template = self.cursor.template;
        template.flags ^= template.flags;

        let col = self.cursor.point.col;

        match mode {
            ansi::LineClearMode::Right => {
                let row = &mut self.grid[self.cursor.point.line];
                for cell in &mut row[col..] {
                    cell.reset(&template);
                }
            }
            ansi::LineClearMode::Left => {
                let row = &mut self.grid[self.cursor.point.line];
                for cell in &mut row[..=col] {
                    cell.reset(&template);
                }
            }
            ansi::LineClearMode::All => {
                let row = &mut self.grid[self.cursor.point.line];
                for cell in &mut row[..] {
                    cell.reset(&template);
                }
            }
        }
    }

    #[inline]
    fn clear_screen(&mut self, mode: ansi::ClearMode) {
        trace!("Clearing screen: {:?}", mode);
        let mut template = self.cursor.template;
        template.flags ^= template.flags;

        // Remove active selections
        self.grid.selection = None;

        match mode {
            ansi::ClearMode::Below => {
                for cell in &mut self.grid[self.cursor.point.line][self.cursor.point.col..] {
                    cell.reset(&template);
                }
                if self.cursor.point.line < self.grid.num_lines() - 1 {
                    self.grid
                        .region_mut((self.cursor.point.line + 1)..)
                        .each(|cell| cell.reset(&template));
                }
            }
            ansi::ClearMode::All => self.grid.region_mut(..).each(|c| c.reset(&template)),
            ansi::ClearMode::Above => {
                // If clearing more than one line
                if self.cursor.point.line > index::Line(1) {
                    // Fully clear all lines before the current line
                    self.grid
                        .region_mut(..self.cursor.point.line)
                        .each(|cell| cell.reset(&template));
                }
                // Clear up to the current column in the current line
                let end = min(self.cursor.point.col + 1, self.grid.num_cols());
                for cell in &mut self.grid[self.cursor.point.line][..end] {
                    cell.reset(&template);
                }
            }
            // If scrollback is implemented, this should clear it
            ansi::ClearMode::Saved => self.grid.clear_history(),
        }
    }

    #[inline]
    fn clear_tabs(&mut self, mode: ansi::TabulationClearMode) {
        trace!("Clearing tabs: {:?}", mode);
        match mode {
            ansi::TabulationClearMode::Current => {
                let column = self.cursor.point.col;
                self.tabs[column] = false;
            }
            ansi::TabulationClearMode::All => {
                self.tabs.clear_all();
            }
        }
    }

    // Reset all important fields in the term struct
    #[inline]
    fn reset_state(&mut self) {
        self.input_needs_wrap = false;
        self.next_title = None;
        self.next_mouse_cursor = None;
        self.alt = false;
        self.cursor = Default::default();
        self.active_charset = Default::default();
        self.mode = Default::default();
        self.next_is_urgent = None;
        self.cursor_save = Default::default();
        self.cursor_save_alt = Default::default();
        self.cursor_style = None;
        self.grid.clear_history();
        self.grid.region_mut(..).each(|c| c.reset(&Cell::default()));
    }

    #[inline]
    fn reverse_index(&mut self) {
        trace!("Reversing index");
        // if cursor is at the top
        if self.cursor.point.line == self.scroll_region.start {
            self.scroll_down(index::Line(1));
        } else {
            self.cursor.point.line -= min(self.cursor.point.line, index::Line(1));
        }
    }

    /// set a terminal attribute
    #[inline]
    fn terminal_attribute(&mut self, attr: Attr) {
        trace!("Setting attribute: {:?}", attr);
        match attr {
            Attr::Foreground(color) => self.cursor.template.fg = color,
            Attr::Background(color) => self.cursor.template.bg = color,
            Attr::Reset => {
                self.cursor.template.fg = Color::Named(NamedColor::Foreground);
                self.cursor.template.bg = Color::Named(NamedColor::Background);
                self.cursor.template.flags = cell::Flags::empty();
            }
            Attr::Reverse => self.cursor.template.flags.insert(cell::Flags::INVERSE),
            Attr::CancelReverse => self.cursor.template.flags.remove(cell::Flags::INVERSE),
            Attr::Bold => self.cursor.template.flags.insert(cell::Flags::BOLD),
            Attr::CancelBold => self.cursor.template.flags.remove(cell::Flags::BOLD),
            Attr::Dim => self.cursor.template.flags.insert(cell::Flags::DIM),
            Attr::CancelBoldDim => self
                .cursor
                .template
                .flags
                .remove(cell::Flags::BOLD | cell::Flags::DIM),
            Attr::Italic => self.cursor.template.flags.insert(cell::Flags::ITALIC),
            Attr::CancelItalic => self.cursor.template.flags.remove(cell::Flags::ITALIC),
            Attr::Underscore => self.cursor.template.flags.insert(cell::Flags::UNDERLINE),
            Attr::CancelUnderline => self.cursor.template.flags.remove(cell::Flags::UNDERLINE),
            Attr::Hidden => self.cursor.template.flags.insert(cell::Flags::HIDDEN),
            Attr::CancelHidden => self.cursor.template.flags.remove(cell::Flags::HIDDEN),
            Attr::Strike => self.cursor.template.flags.insert(cell::Flags::STRIKEOUT),
            Attr::CancelStrike => self.cursor.template.flags.remove(cell::Flags::STRIKEOUT),
            _ => {
                debug!("Term got unhandled attr: {:?}", attr);
            }
        }
    }

    #[inline]
    fn set_mode(&mut self, mode: ansi::Mode) {
        trace!("Setting mode: {:?}", mode);
        match mode {
            ansi::Mode::SwapScreenAndSetRestoreCursor => {
                self.mode.insert(mode::TermMode::ALT_SCREEN);
                self.save_cursor_position();
                if !self.alt {
                    self.swap_alt();
                }
                self.save_cursor_position();
            }
            ansi::Mode::ShowCursor => self.mode.insert(mode::TermMode::SHOW_CURSOR),
            ansi::Mode::CursorKeys => self.mode.insert(mode::TermMode::APP_CURSOR),
            ansi::Mode::ReportMouseClicks => {
                self.mode.insert(mode::TermMode::MOUSE_REPORT_CLICK);
                self.set_mouse_cursor(MouseCursor::Arrow);
            }
            ansi::Mode::ReportCellMouseMotion => {
                self.mode.insert(mode::TermMode::MOUSE_DRAG);
                self.set_mouse_cursor(MouseCursor::Arrow);
            }
            ansi::Mode::ReportAllMouseMotion => {
                self.mode.insert(mode::TermMode::MOUSE_MOTION);
                self.set_mouse_cursor(MouseCursor::Arrow);
            }
            ansi::Mode::ReportFocusInOut => self.mode.insert(mode::TermMode::FOCUS_IN_OUT),
            ansi::Mode::BracketedPaste => self.mode.insert(mode::TermMode::BRACKETED_PASTE),
            ansi::Mode::SgrMouse => self.mode.insert(mode::TermMode::SGR_MOUSE),
            ansi::Mode::LineWrap => self.mode.insert(mode::TermMode::LINE_WRAP),
            ansi::Mode::LineFeedNewLine => self.mode.insert(mode::TermMode::LINE_FEED_NEW_LINE),
            ansi::Mode::Origin => self.mode.insert(mode::TermMode::ORIGIN),
            ansi::Mode::DECCOLM => self.deccolm(),
            ansi::Mode::Insert => self.mode.insert(mode::TermMode::INSERT), // heh
            ansi::Mode::BlinkingCursor => {
                trace!("... unimplemented mode");
            }
        }
    }

    #[inline]
    fn unset_mode(&mut self, mode: ansi::Mode) {
        trace!("Unsetting mode: {:?}", mode);
        match mode {
            ansi::Mode::SwapScreenAndSetRestoreCursor => {
                self.mode.remove(mode::TermMode::ALT_SCREEN);
                self.restore_cursor_position();
                if self.alt {
                    self.swap_alt();
                }
                self.restore_cursor_position();
            }
            ansi::Mode::ShowCursor => self.mode.remove(mode::TermMode::SHOW_CURSOR),
            ansi::Mode::CursorKeys => self.mode.remove(mode::TermMode::APP_CURSOR),
            ansi::Mode::ReportMouseClicks => {
                self.mode.remove(mode::TermMode::MOUSE_REPORT_CLICK);
                self.set_mouse_cursor(MouseCursor::Text);
            }
            ansi::Mode::ReportCellMouseMotion => {
                self.mode.remove(mode::TermMode::MOUSE_DRAG);
                self.set_mouse_cursor(MouseCursor::Text);
            }
            ansi::Mode::ReportAllMouseMotion => {
                self.mode.remove(mode::TermMode::MOUSE_MOTION);
                self.set_mouse_cursor(MouseCursor::Text);
            }
            ansi::Mode::ReportFocusInOut => self.mode.remove(mode::TermMode::FOCUS_IN_OUT),
            ansi::Mode::BracketedPaste => self.mode.remove(mode::TermMode::BRACKETED_PASTE),
            ansi::Mode::SgrMouse => self.mode.remove(mode::TermMode::SGR_MOUSE),
            ansi::Mode::LineWrap => self.mode.remove(mode::TermMode::LINE_WRAP),
            ansi::Mode::LineFeedNewLine => self.mode.remove(mode::TermMode::LINE_FEED_NEW_LINE),
            ansi::Mode::Origin => self.mode.remove(mode::TermMode::ORIGIN),
            ansi::Mode::DECCOLM => self.deccolm(),
            ansi::Mode::Insert => self.mode.remove(mode::TermMode::INSERT),
            ansi::Mode::BlinkingCursor => {
                trace!("... unimplemented mode");
            }
        }
    }

    #[inline]
    fn set_scrolling_region(&mut self, region: Range<index::Line>) {
        trace!("Setting scrolling region: {:?}", region);
        self.scroll_region.start = min(region.start, self.grid.num_lines());
        self.scroll_region.end = min(region.end, self.grid.num_lines());
        self.goto(index::Line(0), index::Column(0));
    }

    #[inline]
    fn set_keypad_application_mode(&mut self) {
        trace!("Setting keypad application mode");
        self.mode.insert(mode::TermMode::APP_KEYPAD);
    }

    #[inline]
    fn unset_keypad_application_mode(&mut self) {
        trace!("Unsetting keypad application mode");
        self.mode.remove(mode::TermMode::APP_KEYPAD);
    }

    #[inline]
    fn set_active_charset(&mut self, index: CharsetIndex) {
        trace!("Setting active charset {:?}", index);
        self.active_charset = index;
    }

    #[inline]
    fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset) {
        trace!("Configuring charset {:?} as {:?}", index, charset);
        self.cursor.charsets[index] = charset;
    }

    /// Set the clipboard
    #[inline]
    fn set_clipboard(&mut self, _string: &str) {
        // TODO
    }

    #[inline]
    fn dectest(&mut self) {
        trace!("Dectesting");
        let mut template = self.cursor.template;
        template.c = 'E';

        self.grid.region_mut(..).each(|c| c.reset(&template));
    }
}

struct TabStops {
    tabs: Vec<bool>,
}

impl TabStops {
    fn new(num_cols: index::Column, tabspaces: usize) -> TabStops {
        TabStops {
            tabs: index::Range::from(index::Column(0)..num_cols)
                .map(|i| (*i as usize) % tabspaces == 0)
                .collect::<Vec<bool>>(),
        }
    }

    fn clear_all(&mut self) {
        unsafe {
            ptr::write_bytes(self.tabs.as_mut_ptr(), 0, self.tabs.len());
        }
    }
}

impl Index<index::Column> for TabStops {
    type Output = bool;

    fn index(&self, index: index::Column) -> &bool {
        &self.tabs[index.0]
    }
}

impl IndexMut<index::Column> for TabStops {
    fn index_mut(&mut self, index: index::Column) -> &mut bool {
        self.tabs.index_mut(index.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{Cell, SizeInfo, Term};
    use crate::term::cell;

    use crate::ansi::{self, CharsetIndex, Handler, StandardCharset};
    use crate::grid::{Grid, Scroll};
    use crate::index;
    use crate::selection::Selection;
    use std::mem;

    #[test]
    fn semantic_selection_works() {
        let size = SizeInfo {
            width: 21.0,
            height: 51.0,
            cell_width: 3.0,
            cell_height: 3.0,
            padding_x: 0.0,
            padding_y: 0.0,
            dpr: 1.0,
        };
        let mut term = Term::new(size);
        let mut grid: Grid<Cell> = Grid::new(index::Line(3), index::Column(5), 0, Cell::default());
        for i in 0..5 {
            for j in 0..2 {
                grid[index::Line(j)][index::Column(i)].c = 'a';
            }
        }
        grid[index::Line(0)][index::Column(0)].c = '"';
        grid[index::Line(0)][index::Column(3)].c = '"';
        grid[index::Line(1)][index::Column(2)].c = '"';
        grid[index::Line(0)][index::Column(4)]
            .flags
            .insert(cell::Flags::WRAPLINE);

        let mut escape_chars = String::from("\"");

        mem::swap(&mut term.grid, &mut grid);
        mem::swap(&mut term.semantic_escape_chars, &mut escape_chars);

        {
            *term.selection_mut() = Some(Selection::semantic(index::Point {
                line: 2,
                col: index::Column(1),
            }));
            assert_eq!(term.selection_to_string(), Some(String::from("aa")));
        }

        {
            *term.selection_mut() = Some(Selection::semantic(index::Point {
                line: 2,
                col: index::Column(4),
            }));
            assert_eq!(term.selection_to_string(), Some(String::from("aaa")));
        }

        {
            *term.selection_mut() = Some(Selection::semantic(index::Point {
                line: 1,
                col: index::Column(1),
            }));
            assert_eq!(term.selection_to_string(), Some(String::from("aaa")));
        }
    }

    #[test]
    fn line_selection_works() {
        let size = SizeInfo {
            width: 21.0,
            height: 51.0,
            cell_width: 3.0,
            cell_height: 3.0,
            padding_x: 0.0,
            padding_y: 0.0,
            dpr: 1.0,
        };
        let mut term = Term::new(size);
        let mut grid: Grid<Cell> = Grid::new(index::Line(1), index::Column(5), 0, Cell::default());
        for i in 0..5 {
            grid[index::Line(0)][index::Column(i)].c = 'a';
        }
        grid[index::Line(0)][index::Column(0)].c = '"';
        grid[index::Line(0)][index::Column(3)].c = '"';

        mem::swap(&mut term.grid, &mut grid);

        *term.selection_mut() = Some(Selection::lines(index::Point {
            line: 0,
            col: index::Column(3),
        }));
        assert_eq!(term.selection_to_string(), Some(String::from("\"aa\"a\n")));
    }

    #[test]
    fn selecting_empty_line() {
        let size = SizeInfo {
            width: 21.0,
            height: 51.0,
            cell_width: 3.0,
            cell_height: 3.0,
            padding_x: 0.0,
            padding_y: 0.0,
            dpr: 1.0,
        };
        let mut term = Term::new(size);
        let mut grid: Grid<Cell> = Grid::new(index::Line(3), index::Column(3), 0, Cell::default());
        for l in 0..3 {
            if l != 1 {
                for c in 0..3 {
                    grid[index::Line(l)][index::Column(c)].c = 'a';
                }
            }
        }

        mem::swap(&mut term.grid, &mut grid);

        let mut selection = Selection::simple(
            index::Point {
                line: 2,
                col: index::Column(0),
            },
            index::Side::Left,
        );
        selection.update(
            index::Point {
                line: 0,
                col: index::Column(2),
            },
            index::Side::Right,
        );
        *term.selection_mut() = Some(selection);
        assert_eq!(term.selection_to_string(), Some("aaa\n\naaa\n".into()));
    }

    #[test]
    fn input_line_drawing_character() {
        let size = SizeInfo {
            width: 21.0,
            height: 51.0,
            cell_width: 3.0,
            cell_height: 3.0,
            padding_x: 0.0,
            padding_y: 0.0,
            dpr: 1.0,
        };
        let mut term = Term::new(size);
        let cursor = index::Point::new(index::Line(0), index::Column(0));
        term.configure_charset(
            CharsetIndex::G0,
            StandardCharset::SpecialCharacterAndLineDrawing,
        );
        term.input('a');

        assert_eq!(term.grid()[&cursor].c, '▒');
    }

    #[test]
    fn clear_saved_lines() {
        let size = SizeInfo {
            width: 21.0,
            height: 51.0,
            cell_width: 3.0,
            cell_height: 3.0,
            padding_x: 0.0,
            padding_y: 0.0,
            dpr: 1.0,
        };
        let mut term: Term = Term::new(size);

        // Add one line of scrollback
        term.grid.scroll_up(
            &(index::Line(0)..index::Line(1)),
            index::Line(1),
            &Cell::default(),
        );

        // Clear the history
        term.clear_screen(ansi::ClearMode::Saved);

        // Make sure that scrolling does not change the grid
        let mut scrolled_grid = term.grid.clone();
        scrolled_grid.scroll_display(Scroll::Top);
        assert_eq!(term.grid, scrolled_grid);
    }
}

#[cfg(all(test, feature = "bench"))]
mod benches {
    extern crate test;

    use std::fs::File;
    use std::io::Read;
    use std::mem;
    use std::path::Path;

    use crate::config::Config;
    use crate::grid::Grid;
    use crate::message_bar::MessageBuffer;

    use super::cell::Cell;
    use super::{SizeInfo, Term};

    fn read_string<P>(path: P) -> String
    where
        P: AsRef<Path>,
    {
        let mut res = String::new();
        File::open(path.as_ref())
            .unwrap()
            .read_to_string(&mut res)
            .unwrap();

        res
    }

    /// Benchmark for the renderable cells iterator
    ///
    /// The renderable cells iterator yields cells that require work to be
    /// displayed (that is, not a an empty background cell). This benchmark
    /// measures how long it takes to process the whole iterator.
    ///
    /// When this benchmark was first added, it averaged ~78usec on my macbook
    /// pro. The total render time for this grid is anywhere between ~1500 and
    /// ~2000usec (measured imprecisely with the visual meter).
    #[bench]
    fn render_iter(b: &mut test::Bencher) {
        // Need some realistic grid state; using one of the ref files.
        let serialized_grid = read_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/ref/vim_large_window_scroll/grid.json"
        ));
        let serialized_size = read_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/ref/vim_large_window_scroll/size.json"
        ));

        let mut grid: Grid<Cell> = json::from_str(&serialized_grid).unwrap();
        let size: SizeInfo = json::from_str(&serialized_size).unwrap();

        let config = Config::default();

        let mut terminal = Term::new(size);
        mem::swap(&mut terminal.grid, &mut grid);

        b.iter(|| {
            let iter = terminal.renderable_cells(&config, false);
            for cell in iter {
                test::black_box(cell);
            }
        })
    }
}
