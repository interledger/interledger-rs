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
use rustls::{ServerConfig, ServerSession, Session, NoClientAuth};
use std::sync::Arc;
use std::io::{Error, ErrorKind, BufReader};
use std::fs;

fn main() {
  env_logger::init();

  let future = connect_async("ws://bob:bob@localhost:7768")
    .and_then(move |plugin: ClientPlugin| {
      println!("Conected receiver");

    let mut config = rustls::ServerConfig::new(Arc::new(NoClientAuth {}));
    let mut cert_file = fs::File::open("cert.pem").expect("Cannot open cert file");
    let mut cert_reader = BufReader::new(cert_file);
    let cert = rustls::internal::pemfile::certs(&mut cert_reader).unwrap();

println!("loaded cert");

      let keyfile = fs::File::open("key.pem")
          .expect("cannot open private key file");
      let mut key_reader = BufReader::new(keyfile);
     let rsa_keys = rustls::internal::pemfile::pkcs8_private_keys(&mut key_reader)
.expect("file contains invalid rsa private key");

println!("loaded key");

config.set_single_cert(cert, rsa_keys[0].clone()).unwrap();

    let mut sess = rustls::ServerSession::new(&Arc::new(config));

      let mut sock = PluginReaderWriter::new(plugin, String::from("private.moneyd.local.bob"), String::from("private.moneyd.local.alice"));
// let mut tls = rustls::Stream::new(&mut sess, &mut sock);
//     tls.write(String::from("private.moneyd.local.alice").as_bytes()).unwrap();


    // println!("Shared secret {:x?}", shared_secret);
    loop {
if sess.wants_read() {
    sess.read_tls(&mut sock)
      .or_else(|err| {
        if err.kind() == ErrorKind::Interrupted {
          Ok(0)
        } else {
          println!("Error reading TLS {:?}", err);
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
