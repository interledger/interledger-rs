// Copyright 2019 Coil Technologies
//
// Licensed under the Apache License, Version 2.0 (the "License"); you may not use
// this file except in compliance with the License. You may obtain a copy of the
// License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software distributed under the
// License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND,
// either express or implied. See the License for the specific language governing permissions and
// limitations under the License.

// This file is originally from https://github.com/coilhq/interledger-relay/blob/master/crates/interledger-relay/src/combinators/limit_stream.rs
// and is modified to accept Option value for limitation.

use std::error::Error;
use std::fmt;

use futures::prelude::*;

/// A stream combinator which returns a maximum number of bytes before failing.
#[derive(Debug)]
pub struct LimitStream<S> {
    is_limited: bool,
    remaining: usize,
    stream: S,
}

impl<S> LimitStream<S> {
    #[inline]
    pub fn new(max_bytes: Option<usize>, stream: S) -> Self {
        LimitStream {
            is_limited: max_bytes.is_some(),
            remaining: max_bytes.unwrap_or_default(),
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

        if !self.is_limited {
            return Ok(item);
        }

        if let Async::Ready(Some(chunk)) = &item {
            let chunk_size = chunk.as_ref().len();
            match self.remaining.checked_sub(chunk_size) {
                Some(remaining) => self.remaining = remaining,
                None => {
                    self.remaining = 0;
                    return Err(LimitStreamError::LimitExceeded);
                }
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
            LimitStreamError::StreamError(error) => write!(f, "StreamError({})", error),
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
            collect_limited_stream(buffer.clone(), Some(SIZE + 1)).unwrap(),
            buffer,
        );
        // Buffer size is equal to the limit.
        assert_eq!(
            collect_limited_stream(buffer.clone(), Some(SIZE)).unwrap(),
            buffer,
        );
        // Buffer size is above the limit.
        assert!({
            collect_limited_stream(buffer.clone(), Some(SIZE - 1))
                .unwrap_err()
                .is_limit_exceeded()
        });
        // Stream is not limited.
        assert_eq!(
            collect_limited_stream(buffer.clone(), None).unwrap(),
            buffer,
        );
    }

    fn collect_limited_stream(
        buffer: Bytes,
        limit: Option<usize>,
    ) -> Result<Bytes, LimitStreamError<hyper::Error>> {
        let stream = hyper::Body::from(buffer);
        LimitStream::new(limit, stream)
            .concat2()
            .wait()
            .map(Bytes::from)
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
