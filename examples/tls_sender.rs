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
use rustls::{ClientConfig, ClientSession};
use std::sync::Arc;
use tokio::prelude::*;
use webpki::DNSNameRef;

pub struct NoCertificateVerification {}

impl rustls::ServerCertVerifier for NoCertificateVerification {
  fn verify_server_cert(
    &self,
    _roots: &rustls::RootCertStore,
    _presented_certs: &[rustls::Certificate],
    _dns_name: webpki::DNSNameRef,
    _ocsp: &[u8],
  ) -> Result<rustls::ServerCertVerified, rustls::TLSError> {
    Ok(rustls::ServerCertVerified::assertion())
  }
}

fn main() {
  env_logger::init();

  let future =
    connect_async("ws://alice:alice@localhost:7768").and_then(move |plugin: ClientPlugin| {
      println!("Conected sender");

      let mut config = ClientConfig::new();
      config
        .dangerous()
        .set_certificate_verifier(Arc::new(NoCertificateVerification {}));
      let dns_name = DNSNameRef::try_from_ascii_str("localhost").unwrap();
      let tls_session = ClientSession::new(&Arc::new(config), dns_name);

      connect_tls(plugin, tls_session, "private.moneyd.local.bob").and_then(
        |(shared_secret, _plugin)| {
          println!("Shared secret {:x?}", &shared_secret[..]);
          Ok(())
        },
      )
    });

  tokio::runtime::run(future);
}
