#![feature(specialization, custom_attribute)]
extern crate pyo3;
extern crate ilp;
extern crate futures;
extern crate tokio;

use pyo3::prelude::*;

use ilp::plugin::btp::connect_to_moneyd;
use ilp::spsp::{pay as spsp_pay};
use futures::Future;
use tokio::runtime::Runtime;


#[pymodinit]
fn ilpy(_py: Python, m: &PyModule) -> PyResult<()> {

    #[pyfn(m, "pay")]
    fn pay_py(receiver: String, amount: u64) -> PyResult<u64> {
        let future = connect_to_moneyd()
            .map_err(|err| {
                format!("Unable to connect to moneyd: {:?}", err)
            })
            .and_then(move |plugin| {
                spsp_pay(plugin, &receiver, amount)
                    .map_err(|err| {
                        format!("Error sending SPSP payment: {:?}", err)
                    })
            });

            let runtime = Runtime::new().unwrap();
            let result = runtime.block_on_all(future);
            Ok(result.unwrap())
    }
    Ok(())
}
