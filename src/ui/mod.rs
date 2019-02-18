mod ansi;
mod cell;
mod color;
mod index;
mod mouse;

pub struct Ui {
    state: State,
}

struct State {
    processes: Vec<ProcessState>,
}

struct ProcessState {
    output: String,
}

impl ansi::Handler for ProcessState {
}
