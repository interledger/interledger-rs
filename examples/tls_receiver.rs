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

fn main() {
  env_logger::init();

  let future = connect_async("ws://bob:bob@localhost:7768")
    .and_then(move |plugin: ClientPlugin| {
      println!("Conected receiver");

      let cert_file = fs::File::open("cert.pem").expect("Cannot open cert file");
      let mut cert_reader = BufReader::new(cert_file);
      let cert = rustls::internal::pemfile::certs(&mut cert_reader).unwrap();

      println!("loaded cert");

      let keyfile = fs::File::open("key.pem").expect("cannot open private key file");
      let mut key_reader = BufReader::new(keyfile);
      let rsa_keys = rustls::internal::pemfile::pkcs8_private_keys(&mut key_reader)
        .expect("file contains invalid rsa private key");

      println!("loaded key");

      listen_for_tls_connections(plugin, cert, rsa_keys[0].clone())
        .and_then(|listener| {
        listener.for_each(|conn| {
          println!("Got incoming connection");
          Ok(())
        })
        .then(|_| Ok(()))
        })
    }).then(|_| Ok(()));

  tokio::runtime::run(future);
}
