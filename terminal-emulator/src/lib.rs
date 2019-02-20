#[macro_use]
extern crate log;

mod ansi;
mod config;
mod grid;
mod index;
mod mode;
mod selection;
mod term;

/// Facade around [winit's `MouseCursor`](glutin::MouseCursor)
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum MouseCursor {
    Arrow,
    Text,
}
