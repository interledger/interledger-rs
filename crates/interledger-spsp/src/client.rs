use super::{Error, SpspResponse};
use futures::{future::result, Future};
use interledger_packet::Address;
use interledger_service::{Account, IncomingService};
use interledger_stream::{send_money, StreamDelivery};
use log::{debug, error, trace};
use reqwest::r#async::Client;
use std::convert::TryFrom;

pub fn query(server: &str) -> impl Future<Item = SpspResponse, Error = Error> {
    let server = payment_pointer_to_url(server);
    trace!("Querying receiver: {}", server);

    let client = Client::new();
    client
        .get(&server)
        .header("Accept", "application/spsp4+json")
        .send()
        .map_err(|err| Error::HttpError(format!("Error querying SPSP receiver: {:?}", err)))
        .and_then(|res| {
            res.error_for_status()
                .map_err(|err| Error::HttpError(format!("Error querying SPSP receiver: {:?}", err)))
        })
        .and_then(|mut res| {
            res.json::<SpspResponse>()
                .map_err(|err| Error::InvalidSpspServerResponseError(format!("{:?}", err)))
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
) -> impl Future<Item = StreamDelivery, Error = Error>
where
    S: IncomingService<A> + Clone,
    A: Account,
{
    query(receiver).and_then(move |spsp| {
        let shared_secret = spsp.shared_secret;
        let dest = spsp.destination_account;
        result(Address::try_from(dest).map_err(move |err| {
            error!("Error parsing address");
            Error::InvalidSpspServerResponseError(err.to_string())
        }))
        .and_then(move |addr| {
            debug!("Sending SPSP payment to address: {}", addr);

            send_money(service, &from_account, addr, &shared_secret, source_amount)
                .map(move |(receipt, _plugin)| {
                    debug!("Sent SPSP payment. StreamDelivery: {:?}", receipt);
                    receipt
                })
                .map_err(move |err| {
                    error!("Error sending payment: {:?}", err);
                    Error::SendMoneyError(source_amount)
                })
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
    if num_slashes == 2 {
        url.push_str("/.well-known/pay");
    } else if num_slashes == 1 && url.ends_with('/') {
        url.push_str(".well-known/pay");
    }
    trace!(
        "Converted payment pointer: {} to URL: {}",
        payment_pointer,
        url
    );
    url
}

#[cfg(test)]
mod payment_pointer {
    use super::*;

    #[test]
    fn converts_pointer() {
        let pointer = "$subdomain.domain.example";
        assert_eq!(
            payment_pointer_to_url(pointer),
            "https://subdomain.domain.example/.well-known/pay"
        );
    }
}
