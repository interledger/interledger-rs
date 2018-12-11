#[macro_use]
extern crate neon;
extern crate interledger;
extern crate futures;
extern crate tokio;

use neon::prelude::*;
use interledger::plugin::btp::connect_to_moneyd;
use interledger::spsp::{pay as spsp_pay};
use futures::Future;
use tokio::runtime::Runtime;

fn pay(mut cx: FunctionContext) -> JsResult<JsNumber> {
    let receiver = cx.argument::<JsString>(0)?.value();
    let amount = cx.argument::<JsNumber>(1)?.value() as u64;
    let future = connect_to_moneyd()
        .map_err(|err| {
            format!("Unable to connect to moneyd {:?}", err)
        })
        .and_then(move |plugin| {
            spsp_pay(plugin, &receiver, amount)
                .map_err(|err| {
                    format!("Error sending SPSP payment: {:?}", err)
                })
        });
    let runtime = Runtime::new().unwrap();
    let result = runtime.block_on_all(future);
    match result {
        Ok(amount_sent) => Ok(cx.number(amount_sent as f64)),
        Err(err) => cx.throw_error(err)
    }
}

register_module!(mut cx, {
    cx.export_function("pay", pay)
});
