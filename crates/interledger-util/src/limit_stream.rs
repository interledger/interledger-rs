// Copyright 2019 @sentientwaffle
//
// Released under the MIT license
// [https://opensource.org/licenses/mit-license.php](https://opensource.org/licenses/mit-license.php)

use std::error::Error;
use std::fmt;

use futures::prelude::*;

/// A stream combinator which returns a maximum number of bytes before failing.
#[derive(Debug)]
pub struct LimitStream<S> {
    remaining: usize,
    stream: S,
}

impl<S> LimitStream<S> {
    #[inline]
    pub fn new(max_bytes: usize, stream: S) -> Self {
        LimitStream {
            remaining: max_bytes,
            stream,
        }
    }
}

impl<S> Stream for LimitStream<S>
    where
        S: Stream,
        S::Item: AsRef<[u8]>,
        S::Error: Error,
{
    type Item = S::Item;
    type Error = LimitStreamError<S::Error>;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let item = self.stream.poll()?;

        if let Async::Ready(Some(chunk)) = &item {
            let chunk_size = chunk.as_ref().len();
            match self.remaining.checked_sub(chunk_size) {
                Some(remaining) => self.remaining = remaining,
                None => {
                    self.remaining = 0;
                    return Err(LimitStreamError::LimitExceeded);
                },
            }
        }

        Ok(item)
    }
}

#[derive(Debug, PartialEq)]
pub enum LimitStreamError<E> {
    LimitExceeded,
    StreamError(E),
}

impl<E: Error + 'static> Error for LimitStreamError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self {
            LimitStreamError::LimitExceeded => None,
            LimitStreamError::StreamError(error) => Some(error),
        }
    }
}

impl<E: Error> From<E> for LimitStreamError<E> {
    fn from(error: E) -> Self {
        LimitStreamError::StreamError(error)
    }
}

impl<E: Error> fmt::Display for LimitStreamError<E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LimitStreamError::LimitExceeded => f.write_str("LimitExceeded"),
            LimitStreamError::StreamError(error) => {
                write!(f, "StreamError({})", error)
            },
        }
    }
}

#[cfg(test)]
mod test_limit_stream {
    use bytes::Bytes;

    use super::*;

    #[test]
    fn test_stream() {
        const SIZE: usize = 256;
        let buffer = Bytes::from(&[0x00; SIZE][..]);

        // Buffer size is below limit.
        assert_eq!(
            collect_limited_stream(buffer.clone(), SIZE + 1).unwrap(),
            buffer,
        );
        // Buffer size is equal to the limit.
        assert_eq!(
            collect_limited_stream(buffer.clone(), SIZE).unwrap(),
            buffer,
        );
        // Buffer size is above the limit.
        assert!({
            collect_limited_stream(buffer.clone(), SIZE - 1)
                .unwrap_err()
                .is_limit_exceeded()
        });
    }

    fn collect_limited_stream(buffer: Bytes, limit: usize)
                              -> Result<Bytes, LimitStreamError<hyper::Error>>
    {
        let stream = hyper::Body::from(buffer);
        LimitStream::new(limit, stream)
            .concat2()
            .wait()
            .map(|chunk| Bytes::from(chunk))
    }

    impl<E: Error> LimitStreamError<E> {
        fn is_limit_exceeded(&self) -> bool {
            match self {
                LimitStreamError::LimitExceeded => true,
                _ => false,
            }
        }
    }
}