use plugin::{Plugin, IlpRequest};
use rustls::{ClientConfig, ClientSession, ServerConfig, ServerSession};
use std::io::{Read, Write, Error, ErrorKind, Cursor};
use byteorder::{WriteBytesExt, ReadBytesExt};
use oer::{ReadOerExt, WriteOerExt};
use ilp::{IlpPacket, IlpPrepare, IlpReject};
use futures::{Future, Async};
use bytes::Bytes;
use ring::rand::{SecureRandom, SystemRandom};
use chrono::{Utc, Duration};
use std::cmp::min;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct PluginReaderWriter <S: Plugin> {
  plugin: S,
  source_account: String,
  destination_account: String,
  incoming_buffer: Option<Bytes>,
  next_request_id: Arc<AtomicUsize>,
}

impl<S> PluginReaderWriter<S>
where S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()>
{
  pub fn new(plugin: S, source_account: String, destination_account: String) -> Self {
    PluginReaderWriter {
      plugin,
      source_account,
      destination_account,
      incoming_buffer: None,
      next_request_id: Arc::new(AtomicUsize::from(1)),
    }
  }
}

impl<S> Write for PluginReaderWriter<S>
where S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()>
{
  fn write(&mut self, buf: &[u8]) -> Result<usize, Error> {
    let mut data: Vec<u8> = Vec::new();
    let len = buf.len();
    data.write_var_octet_string(buf)?;
    let prepare = IlpPacket::Prepare(IlpPrepare::new(
      self.destination_account.clone(),
      0,
      random_condition(),
      // TODO use random expiry
      Utc::now() + Duration::seconds(30),
      data,
    ));
    // TODO should we call poll_complete?
    let request_id: u32 = self.next_request_id.fetch_add(1, Ordering::SeqCst) as u32;
    self.plugin.start_send((request_id, prepare))
      .map(|_| len)
      .map_err(|_err| {
        error!("Error sending data to plugin");
        Error::new(ErrorKind::Other, "Error sending data to plugin")
      })
  }

  fn flush(&mut self) -> Result<(), Error> {
    // TODO change this! looping over a future is terrible!
    loop {
      // TODO it would be better to call self.plugin.flush but that requires owning the plugin
      let next = self.plugin.poll_complete()
        .map_err(|_err| {
          Error::new(ErrorKind::Other, "Error flushing to plugin")
        })?;
      if let Async::Ready(_) = next {
        return Ok(())
      }
    }
  }
}

impl <S> Read for PluginReaderWriter<S>
where S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()>
{
  fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
    if let Some(mut buffered) = self.incoming_buffer.take() {
      let (bytes_read, rest) = read_from_bytes(&mut buffered, buf);
      if rest.is_some() {
        self.incoming_buffer = rest;
      }
      return Ok(bytes_read)
    }

    match self.plugin.poll() {
      Ok(Async::Ready(Some((request_id, packet)))) => {
        match packet {
          IlpPacket::Prepare(mut prepare) => {
            let mut reader = Cursor::new(prepare.data);
            let mut data = Bytes::from(reader.read_var_octet_string().unwrap());
            let (bytes_read, rest) = read_from_bytes(&mut data, buf);
            if rest.is_some() {
              self.incoming_buffer = rest;
            }

            // TODO make sure this is sent
            self.plugin.start_send((request_id, IlpPacket::Reject(IlpReject::new(
              "F99",
              "",
              "",
              Bytes::new(),
            )))).unwrap();

            Ok(bytes_read)
          },
          // TODO send data on Reject packets
          // IlpPacket::Reject(reject) => {
          // },
          _ => Err(Error::new(ErrorKind::Interrupted, "Got unexpected ILP packet"))
        }
      },
      Ok(Async::Ready(None)) => Ok(0),
      Ok(Async::NotReady) => Err(Error::new(ErrorKind::Interrupted, "No incoming packet")),
      Err(()) => Err(Error::new(ErrorKind::BrokenPipe, "Got error while polling plugin")),
    }
  }
}

fn read_from_bytes(from: &mut Bytes, to: &mut [u8]) -> (usize, Option<Bytes>) {
  let from_len = from.len();
  let to_len = to.len();

  if from_len > to_len {
    let rest = from.split_off(to_len);
    to.clone_from_slice(&from[..]);
    (to_len, Some(rest))
  } else if from_len < to_len {
    let (to_slice, _rest) = to.split_at_mut(from_len);
    to_slice.clone_from_slice(&from[..]);
    (from_len, None)
  } else {
    to.clone_from_slice(&from[..]);
    (to_len, None)
  }
}

pub fn random_condition() -> Bytes {
  let mut condition_slice: [u8; 32] = [0; 32];
  SystemRandom::new().fill(&mut condition_slice).unwrap();
  Bytes::from(&condition_slice[..])
}
