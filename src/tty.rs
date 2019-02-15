pub struct Tty {
    file: tokio::fs::File,
}

impl Tty {
    pub async fn open() -> Result<Self, failure::Error> {
        let options = tokio::fs::OpenOptions::new().read(true).write(true);
        let file = await!(options.open("/dev/tty"))?;
        Ok(Self { file })
    }
}