# Interledger Service Utilities

Contains a few interledger services which are too small to be their own crate.

TODO: UML Sequence charts for each

## Rate Limit Service

Responsible for rejecting requests by users who have reached their account's rate limit. Talks with the associated Storein order to figure out and set the rate limits per account. Forwards everything else.

Requires a `RateLimitAccount` and a `RateLimitStore`. 

### Incoming Service


Handle Request flow:

1. Apply rate limit based on the sender of the request and the amount in the prepare packet in the request
1. If no limits were hit forward the request
    - If it succeeds, OK
    - If the request forwarding failed, the client should not be charged towards their throughput limit, so they are refunded, and return a reject
1. If the limit ws hit, return a reject with the appropriate ErrorCode.

## Validator Service

Responsible for rejecting timed out requests. Forwards everything else.

Requires a `Account` and _no store_

### Incoming Service

Handle Request Flow:

1. If the prepare packet in the request is not expired, forward it
    - otherwise return a reject


### Outgoing Service

Send Request flow:

1. If the outgoing packet has expired, return a reject with the appropriate ErrorCode
1. Tries to forward the request
    - If no response is received before the prepare packet's expiration, it assumes that the outgoing request has timed out. 
    - If no timeout occurred, but still errored it will just return the reject
    - If the forwarding is successful, it should receive a fulfill packet. Depending on if the hash of the fulfillment condition inside the fulfill is a preimage of the condition of the prepare:
        - return the fulfill if it matches
        - otherwise reject


## Balance Service

Responsible for managing the balances of the account and the interaction with the Settlement Engine 

Requires an `IldcpAccount` and a `BalanceStore`

### Outgoing Service

Send Request Flow:

1. Calls `store.update_balances_for_prepare` with the prepare. If it fails, it replies with a reject
1. Tries to forward the request:
    - If it returns a fullfil, calls `store.update_balances_for_fulfill` and replies with the fulfill INDEPENDENTLY of if the call suceeds or fails. THIS MAKES A `sendMoney` CALL TO THE SETTLEMENT ENGINE
    - if it returns an reject calls `store.update_balances_for_reject` and replies with the fulfill INDEPENDENTLY of if the call suceeds or fails

## Expiry Shortener Service

Each packet should have its expiry duration decreased as it gets propagated to nodes, since the time until it gets back to the original sender will have latency, and we want it to be still valid by then (without reducing the timeout the packet might expire by the time it's back after many hops). This service reduces the expiry time of each packet before forwarding it out.

Requires a `RoundtripTimeAccount` and _no store_

### Outgoing Service

Send Request Flow:

1. Get the sender and receiver's roundtrip time (default 500ms)
2. Reduce the packet's expiry by that amount
3. Forward the request


## Exchange Rates Service

Responsible for getting the exchange rates for the two assets in the outgoing request (`request.from.asset_code`, `request.to.asset_code`). 

Requires a `ExchangeRateStore`

### Outgoing Service

Send Request Flow:

1. If the prepare packet's amount is 0, it just forwards
1. Retrieves the exchange rate from the store (the store independently is responsible for polling the rates) 
    - return reject if the call to the store fails
1. Calculates the exchange rate AND scales it up/down depending on how many decimals each asset requires
1. Updates the amount in the prepare packet and forwards it

## MaxPacketAmount Service

Used by the connector to limit the maximum size of each packet they want to forward. They may want to limit that size for liquidity or security reasons (you might not want one big packet using up a bunch of liquidity at once and since each packet carries some risk because of the different timeouts, you might want to keep each individual packet relatively small)

Requires a `MaxPacketAmountAccount` and _no store_.

### Incoming Service

Handle Request Flow:

1. if request.prepare.amount <= request.from.max_packet_amount forward the request, else error





