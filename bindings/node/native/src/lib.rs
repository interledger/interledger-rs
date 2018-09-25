#[macro_use]
extern crate neon;
extern crate ilp;
extern crate futures;

use neon::prelude::*;
use ilp::plugin::btp::connect_to_moneyd;
use ilp::spsp::{pay as spsp_pay};
use futures::Future;

fn pay(mut cx: FunctionContext) -> JsResult<JsNumber> {
    let receiver = cx.argument::<JsString>(0)?.value();
    let amount = cx.argument::<JsNumber>(1)?.value() as u64;
    let plugin = connect_to_moneyd().wait()
        .or_else(|_| {
            cx.throw_error("Unable to connect to moneyd")
        })?;
    // TODO this probably needs to be run by tokio
    let amount_sent = spsp_pay(plugin, &receiver, amount)
                .wait()
                .or_else(|_| {
                    cx.throw_error("Error sending SPSP payment")
                })?;
    Ok(cx.number(amount_sent as f64))
}

register_module!(mut cx, {
    cx.export_function("pay", pay)
});
