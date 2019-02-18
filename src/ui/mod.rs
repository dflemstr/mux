// mod ansi;
// mod cell;

pub struct Ui {
    state: State,
}

struct State {
    processes: Vec<ProcessState>,
}

struct ProcessState {
    output: String,
}

impl vte::Perform for ProcessState {
    fn print(&mut self, _: char) {
        unimplemented!()
    }

    fn execute(&mut self, byte: u8) {
        unimplemented!()
    }

    fn hook(&mut self, params: &[i64], intermediates: &[u8], ignore: bool) {
        unimplemented!()
    }

    fn put(&mut self, byte: u8) {
        unimplemented!()
    }

    fn unhook(&mut self) {
        unimplemented!()
    }

    fn osc_dispatch(&mut self, params: &[&[u8]]) {
        unimplemented!()
    }

    fn csi_dispatch(&mut self, params: &[i64], intermediates: &[u8], ignore: bool, _: char) {
        unimplemented!()
    }

    fn esc_dispatch(&mut self, params: &[i64], intermediates: &[u8], ignore: bool, byte: u8) {
        unimplemented!()
    }
}
