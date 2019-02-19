use std::path;

use crate::options;

mod delimiter;

pub struct Args {
    pub all: Vec<String>,
    pub specific: String,
}

#[must_use = "streams do nothing unless polled"]
enum Source<F, I> {
    File(F),
    Stdin(I),
}

pub async fn read(options: &mut options::Options) -> Result<Vec<Args>, failure::Error> {
    use futures::stream::Stream;

    let delimiter = parse_delimiter(options.null, options.delimiter);

    let arg_template = parse_arg_template(&options.initial_args, &options.replace);

    let raw_args = await!(generate_raw(options.arg_file.take(), delimiter))?;

    let args: Vec<Args> = await!(raw_args
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map(|a| generate_final_args(a, &arg_template))
        .collect())?;

    Ok(args)
}

fn generate_final_args(arg: String, command_parts: &[Vec<String>]) -> Args {
    let specific = arg.clone();
    if command_parts.len() == 1 {
        let mut all = command_parts.iter().next().unwrap().clone();
        all.push(arg);
        Args { all, specific }
    } else {
        let all = command_parts.join(&arg);
        Args { all, specific }
    }
}

fn parse_delimiter(null: bool, delimiter: Option<u8>) -> Option<u8> {
    if null {
        Some(0)
    } else if let Some(d) = delimiter {
        Some(d)
    } else {
        None
    }
}

fn parse_arg_template(initial_args: &[String], replace: &Option<String>) -> Vec<Vec<String>> {
    initial_args
        .split(|part| replace.as_ref().map_or_else(|| part == "{}", |s| part == s))
        .map(|s| s.to_vec())
        .collect::<Vec<_>>()
}

async fn generate_raw(
    arg_file: Option<path::PathBuf>,
    delimiter: Option<u8>,
) -> Result<impl futures::Stream<Item = bytes::Bytes, Error = failure::Error>, failure::Error> {
    let codec = delimiter::Codec::new(delimiter);

    if let Some(arg_file) = arg_file {
        let file = await!(tokio::fs::File::open(arg_file))?;
        let frames = tokio::codec::FramedRead::new(file, codec);
        Ok(Source::File(frames))
    } else {
        Ok(Source::Stdin(tokio::codec::FramedRead::new(
            tokio::io::stdin(),
            codec,
        )))
    }
}

impl<F, I, A, E> futures::Stream for Source<F, I>
where
    F: futures::Stream<Item = A, Error = E>,
    I: futures::Stream<Item = A, Error = E>,
{
    type Item = A;
    type Error = E;

    fn poll(&mut self) -> Result<futures::Async<Option<Self::Item>>, Self::Error> {
        match *self {
            Source::File(ref mut f) => f.poll(),
            Source::Stdin(ref mut i) => i.poll(),
        }
    }
}
