#![feature(specialization)]

#[macro_use]
extern crate pyo3;
extern crate ilp;
extern crate futures;
extern crate tokio;

use pyo3::prelude::*;

use ilp::plugin::btp::connect_async;
use ilp::spsp::{pay as spsp_pay, listen_with_random_secret};

use futures::{Future, Stream};
use tokio::runtime::current_thread;

#[pyfunction]
fn pay_py(btp_server: String, receiver: String, amount: u64) -> PyResult<u64> {
    let future = connect_async(&btp_server)
        .map_err(|err| format!("Unable to connect to BTP server: {:?}", err))
        .and_then(move |plugin| {
            spsp_pay(plugin, &receiver, amount)
                .map_err(|err| format!("Error sending SPSP payment: {:?}", err))
        });

        let result = tokio::runtime::current_thread::block_on_all(future);
        Ok(result.unwrap())
}

#[pyfunction]
fn run_server_py(py: Python, btp_server: String, port: u16) -> PyResult<()> {
    let runtime = current_thread::Runtime::new().unwrap();

    let future = connect_async(&btp_server)
        .map_err(|err| eprintln!("Error connecting to BTP Server: {:?}", err))
        .and_then(move |plugin| {
            println!("Connected receiver");
            listen_with_random_secret(plugin, port)
                .map_err(|err| eprintln!("Error listening: {:?}", err))
                .and_then(move |listener| {
                    let handle_connections = listener.for_each(move |(id, connection)| {
                        println!("Got incoming connection {:#}", id);
                        let handle_streams = connection.for_each(move |stream| {
                            let stream_id = stream.id;
                            println!("Got incoming stream {}", &stream_id);
                            let handle_money = stream.money
                                .for_each(move |amount| {
                                    // Arbitrary python function here...
                                    py.run("print('from py')", None, None);
                                    Ok(())
                                }).and_then(move |_| {
                                    println!("Money stream {} closed!", &stream_id);
                                    Ok(())
                            });
                            runtime.spawn(handle_money);
                            Ok(())
                        });
                        runtime.spawn(handle_streams);
                        Ok(())
                    });
                    runtime.spawn(handle_connections);
                    println!("Listening for SPSP connections on port: {}", port);
                    Ok(())
                })
        });
        runtime.run().unwrap();
        Ok(())
}

#[pymodinit]
fn spsp(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_function!(pay_py))?;
    m.add_function(wrap_function!(run_server_py))?;
    Ok(())
}


