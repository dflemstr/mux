
pub struct Grid {
    pub rows: Vec<Row>,
}

pub struct Row {
    pub cells: Vec<super::cell::Cell>,
}
