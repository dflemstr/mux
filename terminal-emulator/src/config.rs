use crate::term;

#[derive(Debug, Default)]
pub struct Config {
    colors: Colors,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Colors {
    pub primary: PrimaryColors,
    pub cursor: CursorColors,
    pub normal: AnsiColors,
    pub bright: AnsiColors,
    pub dim: Option<AnsiColors>,
    pub indexed_colors: Vec<IndexedColor>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PrimaryColors {
    pub background: term::color::Rgb,
    pub foreground: term::color::Rgb,
    pub bright_foreground: Option<term::color::Rgb>,
    pub dim_foreground: Option<term::color::Rgb>,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct CursorColors {
    pub text: Option<term::color::Rgb>,
    pub cursor: Option<term::color::Rgb>,
}
/// The 8-colors sections of config
#[derive(Debug, PartialEq, Eq)]
pub struct AnsiColors {
    pub black: term::color::Rgb,
    pub red: term::color::Rgb,
    pub green: term::color::Rgb,
    pub yellow: term::color::Rgb,
    pub blue: term::color::Rgb,
    pub magenta: term::color::Rgb,
    pub cyan: term::color::Rgb,
    pub white: term::color::Rgb,
}

#[derive(Debug, PartialEq, Eq)]
pub struct IndexedColor {
    pub index: u8,
    pub color: term::color::Rgb,
}

impl Config {
    pub fn colors(&self) -> &Colors {
        &self.colors
    }
}

impl Default for PrimaryColors {
    fn default() -> Self {
        PrimaryColors {
            background: default_background(),
            foreground: default_foreground(),
            bright_foreground: Default::default(),
            dim_foreground: Default::default(),
        }
    }
}

impl Default for Colors {
    fn default() -> Colors {
        Colors {
            primary: Default::default(),
            cursor: Default::default(),
            normal: default_normal_colors(),
            bright: default_bright_colors(),
            dim: Default::default(),
            indexed_colors: Default::default(),
        }
    }
}

fn default_normal_colors() -> AnsiColors {
    AnsiColors {
        black: term::color::Rgb {
            r: 0x00,
            g: 0x00,
            b: 0x00,
        },
        red: term::color::Rgb {
            r: 0xd5,
            g: 0x4e,
            b: 0x53,
        },
        green: term::color::Rgb {
            r: 0xb9,
            g: 0xca,
            b: 0x4a,
        },
        yellow: term::color::Rgb {
            r: 0xe6,
            g: 0xc5,
            b: 0x47,
        },
        blue: term::color::Rgb {
            r: 0x7a,
            g: 0xa6,
            b: 0xda,
        },
        magenta: term::color::Rgb {
            r: 0xc3,
            g: 0x97,
            b: 0xd8,
        },
        cyan: term::color::Rgb {
            r: 0x70,
            g: 0xc0,
            b: 0xba,
        },
        white: term::color::Rgb {
            r: 0xea,
            g: 0xea,
            b: 0xea,
        },
    }
}

fn default_bright_colors() -> AnsiColors {
    AnsiColors {
        black: term::color::Rgb {
            r: 0x66,
            g: 0x66,
            b: 0x66,
        },
        red: term::color::Rgb {
            r: 0xff,
            g: 0x33,
            b: 0x34,
        },
        green: term::color::Rgb {
            r: 0x9e,
            g: 0xc4,
            b: 0x00,
        },
        yellow: term::color::Rgb {
            r: 0xe7,
            g: 0xc5,
            b: 0x47,
        },
        blue: term::color::Rgb {
            r: 0x7a,
            g: 0xa6,
            b: 0xda,
        },
        magenta: term::color::Rgb {
            r: 0xb7,
            g: 0x7e,
            b: 0xe0,
        },
        cyan: term::color::Rgb {
            r: 0x54,
            g: 0xce,
            b: 0xd6,
        },
        white: term::color::Rgb {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        },
    }
}
fn default_background() -> term::color::Rgb {
    term::color::Rgb { r: 0, g: 0, b: 0 }
}

fn default_foreground() -> term::color::Rgb {
    term::color::Rgb {
        r: 0xea,
        g: 0xea,
        b: 0xea,
    }
}
