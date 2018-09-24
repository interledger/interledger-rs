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
use ilp::tls::connect_async as connect_tls;
use rustls::{NoClientAuth, ServerConfig};
use std::fs;
use std::io::BufReader;
use std::sync::Arc;
use tokio::prelude::*;

fn main() {
  env_logger::init();

  let future = connect_async("ws://bob:bob@localhost:7768")
    .and_then(move |plugin: ClientPlugin| {
      println!("Conected receiver");

      let mut config = ServerConfig::new(Arc::new(NoClientAuth {}));
      let cert_file = fs::File::open("cert.pem").expect("Cannot open cert file");
      let mut cert_reader = BufReader::new(cert_file);
      let cert = rustls::internal::pemfile::certs(&mut cert_reader).unwrap();

      println!("loaded cert");

      let keyfile = fs::File::open("key.pem").expect("cannot open private key file");
      let mut key_reader = BufReader::new(keyfile);
      let rsa_keys = rustls::internal::pemfile::pkcs8_private_keys(&mut key_reader)
        .expect("file contains invalid rsa private key");

      println!("loaded key");

      config.set_single_cert(cert, rsa_keys[0].clone()).unwrap();

      let tls_session = rustls::ServerSession::new(&Arc::new(config));

      connect_tls(plugin, tls_session, "private.moneyd.local.alice").and_then(|(shared_secret, _plugin)| {
        println!("Shared secret {:x?}", &shared_secret[..]);
        Ok(())
      })
    }).then(|_| Ok(()));

  tokio::runtime::run(future);
}
