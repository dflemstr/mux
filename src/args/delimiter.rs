pub struct DelimiterCodec {
    delimiter: Option<u8>,
    next_index: usize,
}

impl DelimiterCodec {
    pub fn new(delimiter: Option<u8>) -> Self {
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
