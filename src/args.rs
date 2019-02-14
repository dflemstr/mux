use std::path;

struct DelimiterCodec {
    delimiter: Option<u8>,
    next_index: usize,
}

enum Args<F, I> {
    File(F),
    Stdin(I),
}

pub async fn generate(
    arg_file: Option<path::PathBuf>,
    delimiter: Option<u8>,
) -> Result<impl futures::Stream<Item = bytes::Bytes, Error = failure::Error>, failure::Error> {
    let codec = DelimiterCodec::new(delimiter);

    if let Some(arg_file) = arg_file {
        let file = await!(tokio::fs::File::open(arg_file))?;
        let frames = tokio::codec::FramedRead::new(file, codec);
        Ok(Args::File(frames))
    } else {
        Ok(Args::Stdin(tokio::codec::FramedRead::new(
            tokio::io::stdin(),
            codec,
        )))
    }
}

impl DelimiterCodec {
    fn new(delimiter: Option<u8>) -> Self {
        let next_index = 0;
        Self {
            delimiter,
            next_index,
        }
    }
}

impl tokio::codec::Decoder for DelimiterCodec {
    type Item = bytes::Bytes;
    type Error = failure::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let offset = match self.delimiter {
            Some(d) => memchr::memchr(d, &src[self.next_index..]),
            None => src[self.next_index..]
                .iter()
                .position(|b| b.is_ascii_whitespace()),
        };

        if let Some(offset) = offset {
            let delimiter_index = offset + self.next_index;
            self.next_index = 0;
            let bytes = src.split_to(delimiter_index + 1).freeze();
            // Remove the delimiter
            let bytes = bytes.slice_to(bytes.len() - 1);

            // Don't emit empty strings
            if bytes.is_empty() {
                Ok(None)
            } else {
                Ok(Some(bytes))
            }
        } else {
            self.next_index = src.len();
            Ok(None)
        }
    }

    fn decode_eof(&mut self, buf: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        Ok(match self.decode(buf)? {
            Some(frame) => Some(frame),
            None => {
                // No terminating delimiter - return remaining data, if any
                if buf.is_empty() {
                    None
                } else {
                    let bytes = buf.take().freeze();
                    self.next_index = 0;
                    Some(bytes)
                }
            }
        })
    }
}

impl<F, I, A, E> futures::Stream for Args<F, I>
where
    F: futures::Stream<Item = A, Error = E>,
    I: futures::Stream<Item = A, Error = E>,
{
    type Item = A;
    type Error = E;

    fn poll(&mut self) -> Result<futures::Async<Option<Self::Item>>, Self::Error> {
        match *self {
            Args::File(ref mut f) => f.poll(),
            Args::Stdin(ref mut i) => i.poll(),
        }
    }
}
