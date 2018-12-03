#![feature(specialization, custom_attribute)]
extern crate pyo3;
extern crate ilp;
extern crate futures;
extern crate tokio;

use pyo3::prelude::*;

use ilp::plugin::btp::connect_async;
use ilp::spsp::{pay as spsp_pay, listen_with_random_secret};
use futures::{Future, Stream};
use tokio::runtime::Runtime;

#[pymodinit]
fn spsp(_py: Python, m: &PyModule) -> PyResult<()> {

    #[pyfn(m, "pay")]
    fn pay_py(btp_server: String, receiver: String, amount: u64) -> PyResult<u64> {
        let future = connect_async(&btp_server)
            .map_err(|err| format!("Unable to connect to BTP server: {:?}", err))
            .and_then(move |plugin| {
                spsp_pay(plugin, &receiver, amount)
                    .map_err(|err| format!("Error sending SPSP payment: {:?}", err))
            });

            let runtime = Runtime::new().unwrap();
            let result = runtime.block_on_all(future);
            Ok(result.unwrap())
    }

    // TODO: Make this a server object for improved ergonomics?
    #[pyfn(m, "run_server")]
    fn run_server_py(btp_server: String, port: u16) -> PyResult<()> {
        let future = connect_async(&btp_server)
            .map_err(|err| eprintln!("Error connecting to BTP Server: {:?}", err))
            .and_then(move |plugin| {
                println!("Connected receiver");
                listen_with_random_secret(plugin, port)
                    .map_err(|err| eprintln!("Error listening: {:?}", err))
                    .and_then(move |listener| {
                        let handle_connections = listener.for_each(|(id, connection)| {
                            println!("Got incoming connection {:#}", id);
                            let handle_streams = connection.for_each(move |stream| {
                                let stream_id = stream.id;
                                println!("Got incoming stream {}", &stream_id);
                                let handle_money = stream.money
                                    .for_each(|amount| {
                                        println!("Got incoming money {:#}", amount);
                                        Ok(())
                                    }).and_then(move |_| {
                                        println!("Money stream {} closed!", &stream_id);
                                        Ok(())
                                });
                                tokio::spawn(handle_money);
                                Ok(())
                            });
                            tokio::spawn(handle_streams);
                            Ok(())
                        });
                        tokio::spawn(handle_connections);
                        println!("Listening for SPSP connections on port: {}", port);
                        Ok(())
                    })
            });
            tokio::runtime::run(future);
            Ok(())
    }

    Ok(())
}
