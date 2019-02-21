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
