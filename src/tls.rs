use base64;
use bytes::Bytes;
use chrono::{Duration, Utc};
use futures::future::{err, ok};
use futures::{Async, AsyncSink, Future, Poll, Sink, Stream};
use ilp::{IlpPacket, IlpPrepare, IlpReject};
use oer::{ReadOerExt, WriteOerExt};
use plugin::{IlpRequest, Plugin};
use ring::digest::{digest, SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use rustls::{
  Certificate, ClientConfig, ClientSession, NoClientAuth, PrivateKey, ServerConfig, ServerSession,
  Session,
};
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex, RwLock};
use stream::{
  connect_async as connect_stream, Connection, PrepareToSharedSecretGenerator, StreamListener,
};
use webpki;

static TLS_KEY_EXPORT_LABEL: &'static str = "EXPERIMENTAL interledger stream tls";

pub struct HashVerification {
  expected_hash: Bytes,
}

impl rustls::ServerCertVerifier for HashVerification {
  fn verify_server_cert(
    &self,
    _roots: &rustls::RootCertStore,
    _presented_certs: &[rustls::Certificate],
    _dns_name: webpki::DNSNameRef,
    _ocsp: &[u8],
  ) -> Result<rustls::ServerCertVerified, rustls::TLSError> {
    let hash = digest(&SHA256, &_presented_certs[0].0);
    if Bytes::from(hash.as_ref()) == self.expected_hash {
      Ok(rustls::ServerCertVerified::assertion())
    } else {
      error!(
        "Server sent certificate that did not match expected fingerprint. Got: {}, expected: {}",
        base64::encode_config(hash.as_ref(), base64::URL_SAFE_NO_PAD),
        base64::encode_config(&self.expected_hash[..], base64::URL_SAFE_NO_PAD)
      );
      Err(rustls::TLSError::WebPKIError(
        webpki::Error::CertNotValidForName,
      ))
    }
  }
}

pub fn connect_async<P>(plugin: P, destination: &str) -> impl Future<Item = Connection, Error = ()>
where
  P: Plugin + 'static,
{
  let destination = destination.to_owned();

  ok(())
    .and_then(move |_| {
      let destination_parts: Vec<&str> = destination.split("#").collect();
      if destination_parts.len() != 2 {
        error!(
          "Expected destination in the form <Hash of TLS Certificate>#<ILP Address>, got: {}",
          destination
        );
        return err(());
      }
      let expected_hash = {
        match base64::decode_config(destination_parts[0], base64::URL_SAFE_NO_PAD) {
          Ok(hash_bytes) => {
            if hash_bytes.len() == 32 {
              Bytes::from(hash_bytes)
            } else {
              error!(
                "Certificate fingerprint must be 32 bytes, got: {}",
                hash_bytes.len()
              );
              return err(());
            }
          }
          Err(error) => {
            error!("Error decoding fingerprint as base64url: {:?}", error);
            return err(());
          }
        }
      };
      debug!(
        "Trying to connect to server with TLS fingerprint: {} at: {}",
        base64::encode_config(&expected_hash[..], base64::URL_SAFE_NO_PAD),
        destination_parts[1]
      );

      let mut config = ClientConfig::new();
      config
        .dangerous()
        .set_certificate_verifier(Arc::new(HashVerification { expected_hash }));
      let dns_name = webpki::DNSNameRef::try_from_ascii_str("localhost").unwrap();
      let tls_session = ClientSession::new(&Arc::new(config), dns_name);

      let destination_account = destination_parts[1];
      let token = generate_token();
      let token_64 = base64::encode_config(&token[..], base64::URL_SAFE_NO_PAD);
      let destination_account = format!("{}.{}", destination_account, token_64);

      ok(
        ConnectTls {
          plugin: Some(plugin),
          destination_account: destination_account.clone(),
          tls_session,
          buffered_outgoing: None,
          pending_request: None,
          next_request_id: 1,
        }
        .and_then(|(shared_secret, plugin)| {
          connect_stream(plugin, destination_account, shared_secret)
        }),
      )
    })
    .and_then(|connection| connection)
}

pub struct ConnectTls<P: Stream + Sink, S: Session> {
  plugin: Option<P>,
  // TODO the server doesn't know the destination account
  destination_account: String,
  tls_session: S,
  buffered_outgoing: Option<P::SinkItem>,
  pending_request: Option<u32>,
  next_request_id: u32,
}

impl<P, S> Future for ConnectTls<P, S>
where
  P: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()>,
  S: Session,
{
  type Item = (Bytes, P);
  type Error = ();

  fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
    let mut plugin = {
      if let Some(plugin) = self.plugin.take() {
        plugin
      } else {
        // TODO should this be an error?
        debug!("No plugin to poll");
        return Ok(Async::NotReady);
      }
    };

    // Try sending buffered request first
    if let Some(item) = self.buffered_outgoing.take() {
      if let AsyncSink::NotReady(item) = plugin.start_send(item)? {
        debug!("Plugin still not ready to send, re-buffering request");
        self.buffered_outgoing = Some(item);
        return Ok(Async::NotReady);
      } else {
        debug!("Sent buffered request");
      }
    }

    // Try reading incoming packet
    loop {
      // Poll until we get a NotReady
      // Note: this makes sure Tokio knows we're still waiting for more
      match plugin.poll()? {
        Async::Ready(Some((request_id, packet))) => match packet {
          IlpPacket::Prepare(prepare) => {
            debug!(
              "Reading {} bytes of TLS data from incoming Prepare packet {}",
              prepare.data.len(),
              request_id
            );
            if prepare.data.len() > 0 {
              let mut prepare_reader = Cursor::new(prepare.data);
              let data = prepare_reader.read_var_octet_string().map_err(|err| {
                error!(
                  "Error reading TLS data from packet {}: {:?}",
                  request_id, err
                );
              })?;
              let mut reader = Cursor::new(data);
              self.tls_session.read_tls(&mut reader).map_err(|err| {
                error!("Error reading TLS packets: {:?}", err);
              })?;
            }
            self.pending_request = Some(request_id);
          }
          IlpPacket::Reject(reject) => {
            debug!(
              "Reading {} bytes of TLS data from incoming Reject packet {}",
              reject.data.len(),
              request_id
            );
            if reject.data.len() > 0 {
              let mut reject_reader = Cursor::new(reject.data);
              let data = reject_reader.read_var_octet_string().map_err(|err| {
                error!(
                  "Error reading TLS data from packet {}: {:?}",
                  request_id, err
                );
              })?;
              let mut reader = Cursor::new(data);
              self.tls_session.read_tls(&mut reader).map_err(|err| {
                error!("Error reading TLS packets: {:?}", err);
              })?;
            }
          }
          _ => return Err(()),
        },
        Async::Ready(None) => return Err(()),
        Async::NotReady => break,
      };
      self.tls_session.process_new_packets().map_err(|err| {
        error!("Error processing TLS packet {:?}", err);
      })?;
    }

    // Try sending outgoing packets
    let mut to_send: Vec<u8> = Vec::new();
    while self.tls_session.wants_write() {
      debug!("Writing outgoing TLS data");
      self.tls_session.write_tls(&mut to_send).map_err(|err| {
        error!("Error writing TLS packets from session {:?}", err);
      })?;
    }
    if to_send.len() > 0 {
      let mut data: Vec<u8> = Vec::new();
      data.write_var_octet_string(&to_send).unwrap();

      // Either send the data on a reject or prepare
      let request = {
        if let Some(request_id) = self.pending_request.take() {
          let reject = IlpPacket::Reject(IlpReject::new(
            "F99", "", "", // TODO include our address?
            data,
          ));
          (request_id, reject)
        } else {
          let prepare = IlpPacket::Prepare(IlpPrepare::new(
            self.destination_account.clone(),
            0,
            random_condition(),
            // TODO use random expiry
            Utc::now() + Duration::seconds(30),
            data,
          ));
          let request_id = self.next_request_id;
          self.next_request_id += 1;
          (request_id, prepare)
        }
      };

      debug!("Sending request with TLS data {:?}", request);
      if let AsyncSink::NotReady(item) = plugin.start_send(request)? {
        debug!("Plugin not ready to send, buffering request ");
        self.buffered_outgoing = Some(item);
        return Ok(Async::NotReady);
      }
    }

    // Make sure we don't leave a Prepare hanging
    if let Some(request_id) = self.pending_request.take() {
      let reject = IlpPacket::Reject(IlpReject::new("F99", "", "", Bytes::new()));
      if let AsyncSink::NotReady(item) = plugin.start_send((request_id, reject))? {
        self.buffered_outgoing = Some(item);
        return Ok(Async::NotReady);
      }
    }

    // Export key material from TLS session
    if !self.tls_session.is_handshaking() {
      let mut shared_secret: [u8; 32] = [0; 32];
      self
        .tls_session
        // TODO do we need something for the Context?
        .export_keying_material(&mut shared_secret, TLS_KEY_EXPORT_LABEL.as_bytes(), None)
        .map_err(|err| {
          error!("Error exporting keying material {}", err);
        })?;
      println!("Shared secret {:x?}", &shared_secret[..]);
      return Ok(Async::Ready((Bytes::from(&shared_secret[..]), plugin)));
    }
    debug!("Still handshaking, will poll again");

    self.plugin = Some(plugin);
    Ok(Async::NotReady)
  }
}

pub fn listen_for_tls_connections<S>(
  plugin: S,
  cert: Vec<Certificate>,
  private_key: PrivateKey,
) -> impl Future<Item = StreamListener, Error = ()>
where
  S: Plugin<Item = IlpRequest, Error = (), SinkItem = IlpRequest, SinkError = ()> + 'static,
{
  let hash = digest(&SHA256, &cert[0].0);
  let fingerprint = base64::encode_config(hash.as_ref(), base64::URL_SAFE_NO_PAD);

  let shared_secrets: Arc<RwLock<HashMap<String, Bytes>>> = Arc::new(RwLock::new(HashMap::new()));
  // TODO remove old sessions after timeout
  // TODO remove sessions if there is an error in the handshake
  let sessions: Arc<Mutex<HashMap<String, Arc<Mutex<ServerSession>>>>> =
    Arc::new(Mutex::new(HashMap::new()));

  let prepare_handler: PrepareToSharedSecretGenerator = Box::new(move |local_address, prepare| {
    debug!(
      "Handling prepare for Connection {}: {:?}",
      local_address, prepare
    );
    let local_address_parts: Vec<&str> = local_address.split(".").collect();
    if local_address_parts.len() == 0 {
      warn!("Got Prepare with no Connection ID: {}", prepare.destination);
      return Err(IlpReject::new("F02", "", "", Bytes::new()));
    }
    let connection_id = local_address_parts[0];
    if prepare.data.len() == 0 {
      warn!("Got Prepare with no data");
      return Err(IlpReject::new("F02", "", "", Bytes::new()));
    }
    {
      let shared_secrets = shared_secrets.read().unwrap();
      if let Some(shared_secret) = shared_secrets.get(connection_id) {
        return Ok((connection_id.to_string(), shared_secret.clone()));
      }
    }
    let mut sessions = sessions.lock().unwrap();
    let session_lock: &mut Arc<Mutex<ServerSession>> = sessions
      .entry(connection_id.to_string())
      .or_insert_with(|| {
        let mut config = ServerConfig::new(Arc::new(NoClientAuth {}));
        config
          .set_single_cert(cert.clone(), private_key.clone())
          .or_else(|err| {
            error!("Error setting certificate: {:?}", err);
            return Err(IlpReject::new("F00", "", "", Bytes::new()));
          })
          .unwrap();
        let session = rustls::ServerSession::new(&Arc::new(config));
        let session_lock = Arc::new(Mutex::new(session));
        debug!("Created new TLS session for connection {}", connection_id);
        Arc::clone(&session_lock)
      });
    let mut session = session_lock.lock().unwrap();

    // Read the TLS handshake data from the prepare packet
    let mut prepare_reader = Cursor::new(&prepare.data[..]);
    let data = prepare_reader.read_var_octet_string().map_err(|err| {
      error!("Error reading TLS data from packet: {:?}", err);
      IlpReject::new("F06", "", "", Bytes::new())
    })?;
    let mut reader = Cursor::new(data);
    session.read_tls(&mut reader).map_err(|err| {
      error!("Error reading TLS packets: {:?}", err);
      IlpReject::new("F06", "", "", Bytes::new())
    })?;

    session.process_new_packets().map_err(|err| {
      error!("Error processing TLS data: {:?}", err);
      IlpReject::new("F06", "", "", Bytes::new())
    })?;

    // Send TLS data back to the sender
    let mut to_send: Vec<u8> = Vec::new();
    while session.wants_write() {
      session.write_tls(&mut to_send).map_err(|err| {
        error!("Error writing TLS packets from session {:?}", err);
        IlpReject::new("F00", "", "", Bytes::new())
      })?;
    }

    if !session.is_handshaking() {
      let mut shared_secret: [u8; 32] = [0; 32];
      session
        // TODO do we need something for the Context?
        .export_keying_material(&mut shared_secret, TLS_KEY_EXPORT_LABEL.as_bytes(), None)
        .map_err(|err| {
          error!("Error exporting keying material {}", err);
          IlpReject::new("F00", "", "", Bytes::new())
        })?;
      debug!("Finished TLS handshake for Connection {}", connection_id);
      let mut shared_secrets = shared_secrets.write().unwrap();
      shared_secrets.insert(connection_id.to_string(), Bytes::from(&shared_secret[..]));
    }

    let mut data: Vec<u8> = Vec::new();
    data.write_var_octet_string(&to_send).unwrap();
    Err(IlpReject::new("F99", "", "", Bytes::from(data)))
  });

  StreamListener::bind_with_custom_prepare_handler(plugin, prepare_handler).and_then(
    move |listener| {
      debug!(
        "Listening on: {}#{}",
        fingerprint,
        listener.source_account()
      );

      Ok(listener)
    },
  )
}

fn random_condition() -> Bytes {
  let mut condition_slice: [u8; 32] = [0; 32];
  SystemRandom::new().fill(&mut condition_slice).unwrap();
  Bytes::from(&condition_slice[..])
}

fn generate_token() -> Bytes {
  let mut token: [u8; 18] = [0; 18];
  SystemRandom::new().fill(&mut token).unwrap();
  Bytes::from(&token[..])
}
