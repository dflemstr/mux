use crate::ui::ansi;

bitflags! {
    pub struct Flags: u16 {
        const INVERSE           = 0b00_0000_0001;
        const BOLD              = 0b00_0000_0010;
        const ITALIC            = 0b00_0000_0100;
        const UNDERLINE         = 0b00_0000_1000;
        const WRAPLINE          = 0b00_0001_0000;
        const WIDE_CHAR         = 0b00_0010_0000;
        const WIDE_CHAR_SPACER  = 0b00_0100_0000;
        const DIM               = 0b00_1000_0000;
        const DIM_BOLD          = 0b00_1000_0010;
        const HIDDEN            = 0b01_0000_0000;
        const STRIKEOUT         = 0b10_0000_0000;
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Cell {
    pub c: char,
    pub fg: ansi::Color,
    pub bg: ansi::Color,
    pub flags: Flags,
}

impl Default for Cell {
    fn default() -> Cell {
        Cell::new(
            ' ',
            ansi::Color::Named(ansi::NamedColor::Foreground),
            ansi::Color::Named(ansi::NamedColor::Background)
        )
    }
}
