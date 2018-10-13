extern crate bytes;
extern crate chrono;
extern crate env_logger;
extern crate futures;
extern crate ilp;
extern crate ring;
extern crate rustls;
extern crate tokio;
extern crate webpki;

use ilp::plugin::btp::{connect_async, ClientPlugin};
use ilp::tls::listen_for_tls_connections;
use std::fs;
use std::io::BufReader;
use tokio::prelude::*;
use ring::digest::{digest, SHA256};
use futures::future::poll_fn;
use futures::try_ready;

fn main() {
  env_logger::init();

  let future = connect_async("ws://bob:bob@localhost:7768")
    .and_then(move |plugin: ClientPlugin| {
      println!("Conected receiver");

      let cert_file = fs::File::open("cert.pem").expect("Cannot open cert file");
      let mut cert_reader = BufReader::new(cert_file);
      let cert = rustls::internal::pemfile::certs(&mut cert_reader).unwrap();

      println!("fingerprint {}", hex::encode(digest(&SHA256, &cert[0].clone().0)));

      let keyfile = fs::File::open("key.pem").expect("cannot open private key file");
      let mut key_reader = BufReader::new(keyfile);
      let rsa_keys = rustls::internal::pemfile::pkcs8_private_keys(&mut key_reader)
        .expect("file contains invalid rsa private key");

      println!("loaded key");

      listen_for_tls_connections(plugin, cert, rsa_keys[0].clone())
        .and_then(|listener| {
          listener.for_each(|(id, conn)| {
            println!("Got incoming connection {}", id);
          let handle_connection = conn.for_each(|mut stream| {
            println!("Got incoming stream");
            let handle_money = stream.money.clone().for_each(|amount| {
              println!("Got incoming money {}", amount);
              Ok(())
            });
            tokio::spawn(handle_money);

            // TODO fix inconsistent data receiving
            let handle_data = poll_fn(move || {
              let mut data: [u8; 100] = [0; 100];
              try_ready!(stream.data.poll_read(&mut data[..])
                .map_err(|err| {
                  println!("Error polling stream for data {:?}", err);
                }));
              println!("Got incoming data: {}", String::from_utf8(Vec::from(&data[..])).unwrap());
              Ok(Async::Ready(()))
            });
            tokio::spawn(handle_data);

            Ok(())
          });

          tokio::spawn(handle_connection);
          Ok(())
          })
        .then(|_| Ok(()))
        })
    }).then(|_| Ok(()));

  tokio::runtime::run(future);
}
