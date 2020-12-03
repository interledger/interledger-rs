use super::{Error, SpspResponse};
use futures::TryFutureExt;
use interledger_rates::ExchangeRateStore;
use interledger_service::{Account, IncomingService};
use interledger_stream::{send_money, StreamDelivery};
use reqwest::Client;
use tracing::{debug, error, trace};

/// Get an ILP Address and shared secret by the receiver of this payment for this connection
pub async fn query(server: &str) -> Result<SpspResponse, Error> {
    let server = payment_pointer_to_url(server);
    trace!("Querying receiver: {}", server);

    let client = Client::new();
    let res = client
        .get(&server)
        .header("Accept", "application/spsp4+json")
        .send()
        .map_err(|err| Error::HttpError(format!("Error querying SPSP receiver: {:?}", err)))
        .await?;

    let res = res
        .error_for_status()
        .map_err(|err| Error::HttpError(format!("Error querying SPSP receiver: {:?}", err)))?;

    res.json::<SpspResponse>()
        .map_err(|err| Error::InvalidSpspServerResponseError(format!("{:?}", err)))
        .await
}

/// Query the details of the given Payment Pointer and send a payment using the STREAM protocol.
///
/// This returns the amount delivered, as reported by the receiver and in the receiver's asset's units.
pub async fn pay<I, A, S>(
    service: I,
    from_account: A,
    store: S,
    receiver: &str,
    source_amount: u64,
    slippage: f64,
) -> Result<StreamDelivery, Error>
where
    I: IncomingService<A> + Clone + Send + Sync + 'static,
    A: Account + Send + Sync + 'static,
    S: ExchangeRateStore + Send + Sync + 'static,
{
    let spsp = query(receiver).await?;
    let shared_secret = spsp.shared_secret;
    let addr = spsp.destination_account;
    debug!("Sending SPSP payment to address: {}", addr);

    let receipt = send_money(
        service,
        &from_account,
        store,
        addr,
        shared_secret,
        source_amount,
        slippage,
    )
    .map_err(move |err| {
        error!("Error sending payment: {:?}", err);
        Error::SendMoneyError(source_amount)
    })
    .await?;

    debug!("Sent SPSP payment. StreamDelivery: {:?}", receipt);
    Ok(receipt)
}

fn payment_pointer_to_url(payment_pointer: &str) -> String {
    let mut url: String = if let Some(suffix) = payment_pointer.strip_prefix("$") {
        let prefix = "https://";
        let mut url = String::with_capacity(prefix.len() + suffix.len());
        url.push_str(prefix);
        url.push_str(suffix);
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
