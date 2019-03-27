use super::{Error, SpspResponse};
use futures::Future;
use interledger_service::{Account, IncomingService};
use interledger_stream::send_money;
use reqwest::r#async::Client;

pub fn query(server: &str) -> impl Future<Item = SpspResponse, Error = Error> {
    let server = payment_pointer_to_url(server);

    let client = Client::new();
    client
        .get(&server)
        .header("Accept", "application/spsp4+json")
        .send()
        .map_err(|err| Error::HttpError(format!("Error querying SPSP receiver: {:?}", err)))
        .and_then(|mut res| {
            res.json::<SpspResponse>()
                .map_err(|err| Error::InvalidResponseError(format!("{:?}", err)))
        })
}

/// Query the details of the given Payment Pointer and send a payment using the STREAM protocol.
///
/// This returns the amount delivered, as reported by the receiver and in the receiver's asset's units.
pub fn pay<S, A>(
    service: S,
    from_account: A,
    receiver: &str,
    source_amount: u64,
) -> impl Future<Item = u64, Error = Error>
where
    S: IncomingService<A> + Clone,
    A: Account,
{
    trace!("Querying receiver: {}", receiver);
    query(receiver).and_then(move |spsp| {
        debug!(
            "Sending SPSP payment to address: {}",
            spsp.destination_account
        );
        send_money(
            service,
            &from_account,
            spsp.destination_account.as_bytes(),
            &spsp.shared_secret,
            source_amount,
        )
        .map(move |(amount_delivered, _plugin)| {
            debug!(
                "Sent SPSP payment of {} and delivered {} of the receiver's units",
                source_amount, amount_delivered
            );
            amount_delivered
        })
        .map_err(move |err| {
            error!("Error sending payment: {:?}", err);
            Error::SendMoneyError(source_amount)
        })
    })
}

fn payment_pointer_to_url(payment_pointer: &str) -> String {
    let mut url: String = if payment_pointer.starts_with('$') {
        let mut url = "https://".to_string();
        url.push_str(&payment_pointer[1..]);
        url
    } else {
        payment_pointer.to_string()
    };

    let num_slashes = url.matches('/').count();
    if num_slashes == 0 {
        url.push_str("/.well-known/pay");
    } else if num_slashes == 1 && url.ends_with('/') {
        url.push_str(".well-known/pay");
    }
    url
}
