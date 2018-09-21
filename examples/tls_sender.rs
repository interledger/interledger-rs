extern crate ilp;
extern crate tokio;
extern crate bytes;
extern crate futures;
extern crate ring;
extern crate chrono;
extern crate env_logger;
extern crate rustls;
extern crate webpki;

use tokio::prelude::*;
use ilp::plugin::btp::{connect_async, ClientPlugin};
use ilp::stream::Connection;
use ilp::spsp::{connect_async as connect_spsp};
use ilp::tls::*;
use rustls::{ClientConfig, ClientSession, Session};
use std::sync::Arc;
use std::io::{Error, ErrorKind};

pub struct NoCertificateVerification {}

impl rustls::ServerCertVerifier for NoCertificateVerification {
        fn verify_server_cert(&self,
                              _roots: &rustls::RootCertStore,
                              _presented_certs: &[rustls::Certificate],
                              _dns_name: webpki::DNSNameRef,
                              _ocsp: &[u8]) -> Result<rustls::ServerCertVerified, rustls::TLSError> {
            Ok(rustls::ServerCertVerified::assertion())
        }
}

fn main() {
  env_logger::init();

  let future = connect_async("ws://alice:alice@localhost:7768")
    .and_then(move |plugin: ClientPlugin| {
      println!("Conected sender");

    let mut config = rustls::ClientConfig::new();
    config.dangerous().set_certificate_verifier(Arc::new(NoCertificateVerification {}));
    let dns_name = webpki::DNSNameRef::try_from_ascii_str("localhost").unwrap();
    let mut sess = rustls::ClientSession::new(&Arc::new(config), dns_name);

      let mut sock = PluginReaderWriter::new(plugin, String::from("private.moneyd.local.alice"), String::from("private.moneyd.local.bob"));
// let mut tls = rustls::Stream::new(&mut sess, &mut sock);
//     tls.write(String::from("private.moneyd.local.alice").as_bytes()).unwrap();

    // let label = String::from("EXPERIMENTAL interledger stream tls");
    // let mut shared_secret: [u8; 32] = [0; 32];
    // sess.export_keying_material(&mut shared_secret[..], label.as_bytes(), None).unwrap();

    // println!("Shared secret {:x?}", shared_secret);
    loop {
if sess.wants_read() {
    sess.read_tls(&mut sock)
      .or_else(|err| {
        if err.kind() == ErrorKind::Interrupted {
          Ok(0)
        } else {
          Err(err)
        }
      }).unwrap();
    sess.process_new_packets().unwrap();

    // let mut plaintext = Vec::new();
    // sess.read_to_end(&mut plaintext).unwrap();
    // io::stdout().write(&plaintext).unwrap();
  }

  if sess.wants_write() {
    sess.write_tls(&mut sock).unwrap();
  }


  if !sess.is_handshaking() {
    break
  }


    }

    let label = String::from("EXPERIMENTAL interledger stream tls");
    let mut shared_secret: [u8; 32] = [0; 32];
    sess.export_keying_material(&mut shared_secret[..], label.as_bytes(), None).unwrap();
    println!("shared secret {:x?}", &shared_secret[..]);


    Ok(())


    })
    // .and_then(|mut conn: Connection| {
    //   let stream = conn.create_stream();
    //   // TODO should send_money return the stream so we don't need to clone it?
    //   stream.clone().send_money(100)
    //     .and_then(move |_| {
    //       println!("Sent money on stream. Total sent: {}, total delivered: {}", stream.total_sent(), stream.total_delivered());
    //       Ok(())
    //     })
    //     .map_err(|err| {
    //       println!("Error sending: {:?}", err);
    //     })
    //     .and_then(move |_| {
    //       conn.close()
    //     })
    //     .and_then(|_| {
    //       println!("Closed connection");
    //       Ok(())
    //     })
    // })
    .then(|_| Ok(()));

  tokio::runtime::run(future);
}
