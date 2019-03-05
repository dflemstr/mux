#[derive(Default)]
pub struct VerticalTabs<'a> {
    block: Option<tui::widgets::Block<'a>>,
    titles: Vec<Title<'a>>,
    selected: usize,
    scroll: usize,
    style: tui::style::Style,
    highlight_style: tui::style::Style,
}

#[derive(Default)]
pub struct Title<'a> {
    text: &'a str,
    symbols: Vec<tui::widgets::Text<'a>>,
    style: tui::style::Style,
}

pub enum MouseAction {
    Select(usize),
    ScrollUp,
    ScrollDown,
}

#[derive(Debug)]
struct Layout {
    scroll_up_area: tui::layout::Rect,
    select_area: tui::layout::Rect,
    scroll_down_area: tui::layout::Rect,
}

impl<'a> VerticalTabs<'a> {
    pub fn block(mut self, block: tui::widgets::Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    pub fn titles(mut self, titles: Vec<Title<'a>>) -> Self {
        self.titles = titles;
        self
    }

    pub fn select(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    pub fn scroll(mut self, scroll: usize) -> Self {
        self.scroll = scroll;
        self
    }

    pub fn style(mut self, style: tui::style::Style) -> Self {
        self.style = style;
        self
    }

    pub fn highlight_style(mut self, style: tui::style::Style) -> Self {
        self.highlight_style = style;
        self
    }

    fn has_scroll_up(&self, _area: tui::layout::Rect) -> bool {
        self.scroll > 0
    }

    fn has_scroll_down(&self, area: tui::layout::Rect) -> bool {
        self.titles.len() > self.scroll + area.height as usize - 2
    }

    pub fn on_mouse_event(
        &self,
        area: tui::layout::Rect,
        event: &termion::event::MouseEvent,
    ) -> Option<MouseAction> {
        let (x, y) = super::mouse_event_coords(event);
        let layout = self.layout(area);

        if super::contains_point(layout.scroll_up_area, x, y) {
            Some(MouseAction::ScrollUp)
        } else if super::contains_point(layout.scroll_down_area, x, y) {
            Some(MouseAction::ScrollDown)
        } else if super::contains_point(layout.select_area, x, y) {
            Some(MouseAction::Select(
                (self.scroll + y as usize - layout.select_area.y as usize)
                    .min(self.titles.len() - 1),
            ))
        } else {
            None
        }
    }

    fn layout(&self, area: tui::layout::Rect) -> Layout {
        let tabs_area = match self.block {
            Some(ref b) => b.inner(area),
            None => area,
        };

        let has_scroll_up = self.has_scroll_up(tabs_area);
        let has_scroll_down = self.has_scroll_down(tabs_area);
        let scroll_up_offset = if has_scroll_up { 1 } else { 0 };
        let scroll_down_offset = if has_scroll_down { 1 } else { 0 };

        let scroll_up_area = tui::layout::Rect {
            x: tabs_area.x,
            y: tabs_area.y,
            width: tabs_area.width,
            height: scroll_up_offset,
        };
        let select_area = tui::layout::Rect {
            x: tabs_area.x,
            y: tabs_area.y + scroll_up_offset,
            width: tabs_area.width,
            height: tabs_area.height - scroll_up_offset - scroll_down_offset,
        };
        let scroll_down_area = tui::layout::Rect {
            x: tabs_area.x,
            y: tabs_area.y + tabs_area.height - scroll_down_offset,
            width: tabs_area.width,
            height: scroll_down_offset,
        };

        Layout {
            scroll_up_area,
            select_area,
            scroll_down_area,
        }
    }
}

impl<'a> tui::widgets::Widget for VerticalTabs<'a> {
    fn draw(&mut self, area: tui::layout::Rect, buf: &mut tui::buffer::Buffer) {
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf.get_mut(x, y).reset();
            }
        }

        if let Some(ref mut b) = self.block {
            b.draw(area, buf);
        }

        let Layout {
            scroll_up_area,
            select_area,
            scroll_down_area,
        } = self.layout(area);

        if scroll_up_area.area() > 0 {
            self.background(scroll_up_area, buf, tui::style::Color::DarkGray);
            let cell = buf.get_mut(
                scroll_up_area.x + scroll_up_area.width / 2,
                scroll_up_area.y,
            );
            cell.symbol = "▲".to_owned();
            cell.style.fg = tui::style::Color::Gray;
        }

        if scroll_down_area.area() > 0 {
            self.background(scroll_down_area, buf, tui::style::Color::DarkGray);
            let cell = buf.get_mut(
                scroll_down_area.x + scroll_down_area.width / 2,
                scroll_down_area.y,
            );
            cell.symbol = "▼".to_owned();
            cell.style.fg = tui::style::Color::Gray;
        }

        if select_area.area() == 0 {
            return;
        }

        self.background(select_area, buf, self.style.bg);

        for (i, title) in self.titles.iter_mut().enumerate() {
            let style = if i == self.selected {
                self.highlight_style
            } else {
                self.style
            };
            let title_area = tui::layout::Rect {
                x: select_area.x,
                y: select_area.y + (i as isize - self.scroll as isize).max(0) as u16,
                width: select_area.width,
                height: 1,
            };
            title.style = style;
            if select_area.intersects(title_area) {
                title.draw(title_area, buf);
            }
        }
    }
}

impl<'a> Title<'a> {
    pub fn text(mut self, text: &'a str) -> Self {
        self.text = text;
        self
    }

    pub fn symbols(mut self, symbols: Vec<tui::widgets::Text<'a>>) -> Self {
        self.symbols = symbols;
        self
    }

    pub fn style(mut self, style: tui::style::Style) -> Self {
        self.style = style;
        self
    }
}

impl<'a> tui::widgets::Widget for Title<'a> {
    fn draw(&mut self, mut area: tui::layout::Rect, buf: &mut tui::buffer::Buffer) {
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf.get_mut(x, y).reset();
            }
        }

        for symbol in &self.symbols {
            let (string, style) = match *symbol {
                tui::widgets::Text::Raw(ref string) => (&*string, tui::style::Style::default()),
                tui::widgets::Text::Styled(ref string, style) => (&*string, style),
            };

            let char_count =
                unicode_segmentation::UnicodeSegmentation::graphemes(&**string, true).count();
            let x = area.right() - char_count as u16;
            if x >= area.x {
                buf.set_string(x, area.y, string, style);
            }
            area.width -= char_count as u16 + 1;
        }

        if unicode_segmentation::UnicodeSegmentation::graphemes(self.text, true).count()
            <= area.width as usize
        {
            buf.set_stringn(area.x, area.y, self.text, area.width as usize, self.style);
        } else {
            buf.set_stringn(
                area.x,
                area.y,
                self.text,
                area.width as usize - 1,
                self.style,
            );
            buf.set_string(area.right() - 1, area.y, "…", self.style);
        }
    }
}
