# ILP Node HTTP API

For instructions on running the ILP Node, see the [Readme](../README.md).

## Authentication

The ILP Node uses HTTP Bearer Token authorization. Most requests must either be authenticated with the admin token configured on the node or the token configured for a particular account.

## Account-Related Routes

By default, the API is available on port `7770`.

### POST /accounts

#### Request

At minimum the request must include the following parameters:

```json
{
    "username": "other_node",
    "asset_code": "ABC",
    "asset_scale": 9
}
```

The comprehensive list of possible parameters is as follows:

```json
{
    "username": "other_node",
    "ilp_address": "example.other-node",
    "asset_code": "ABC",
    "asset_scale": 9,
    "max_packet_amount": 100000000000,
    "min_balance": 0,
    "ilp_over_http_url": "https://peer-ilp-over-http-endpoint.example/ilp",
    "ilp_over_http_incoming_token": "http bearer token they will use to authenticate with us",
    "ilp_over_http_outgoing_token": "http bearer token we will use to authenticate with them",
    "ilp_over_btp_url": "btp+wss://peer-btp-endpoint",
    "ilp_over_btp_outgoing_token": "btp auth token we will use to authenticate with them",
    "ilp_over_btp_incoming_token": "btp auth token they will use to authenticate with us",
    "settlement_engine_url": "http://settlement-engine-for-this-account:3000",
    "settle_threshold": 1000000000,
    "settle_to": 0,
    "routing_relation": "Peer",
    "round_trip_time": 500,
    "amount_per_minute_limit": 1000000000,
    "packets_per_minute_limit": 10
}
```

### GET /accounts

Admin only.

### GET /accounts/:username

Admin or account-holder only.

### GET /accounts/:username/balance

Admin or account-holder only.

#### Response

```json
{
    "balance": 1000
}
```

## SPSP (Sending Payments)

### POST /accounts/:username/payments

Account-holder only.

#### Request

```json
{
    "receiver": "$payment-pointer.example",
    "source_amount": 1000000
}
```

#### Response

```json
{
    "delivered_amount": 2000000
}
```

### GET /accounts/:username/spsp

No authentication required.

This is the SPSP receiver endpoing that others will use to pay accounts on this node.

See the [Simple Payment Setup Protocol (SPSP) RFC](https://interledger.org/rfcs/0009-simple-payment-setup-protocol/) for more details about how this protocol works.

#### Response

```json
{
    "destination_account":"test.21bae727127bd22d4d61f3e68eef80bc7d5a6edc.rH4jcsu2wcjMXS0-GhCRL0ZLwqssruLRspVsSJDMRcM","shared_secret":"5k/SCde7gR2QwN8a/vF2LneFt7EUt3WgzC3U6ym28aI="
}
```

### GET /.well-known/pay

No authentication required.

This is the "default" SPSP receiver account on this node. This endpoint is only enabled if the node is run with the configuration option `ILP_DEFAULT_SPSP_ACCOUNT={account id}`.

Same response as above.

## Bilateral Node-to-Node Communication for ILP Packets

### POST /ilp - ILP-over-HTTP

Account-holder only.

This endpoint is used by nodes to send ILP packets over HTTP requests, as the name suggests. This protocol is specified in [IL-RFC 35: ILP-over-HTTP](https://github.com/interledger/rfcs/blob/master/0035-ilp-over-http/0035-ilp-over-http.md).

Note this endpoint is the one referred to as `ilp_over_http_url` in the `AccountSettings`.

### (Websocket) /ilp/btp - Bilateral Transfer Protocol (BTP)

Account-holder only.

This endpoint implements BTP, a WebSocket-based protocol for sending and receiving ILP packets. This protocol is specified in [IL-RFC 22: Bilateral Transfer Protocol 2.0 (BTP/2.0)](https://github.com/interledger/rfcs/blob/master/0023-bilateral-transfer-protocol/0023-bilateral-transfer-protocol.md).

Note this endpoint is the one referred to as `ilp_over_btp_url` in the `AccountSettings`.

## Node Settings

### GET /

Health check.

#### Response

```json
{
    "status": "Ready"
}
```

### PUT /rates

Admin only.

Sets the exchange rates for the node.

#### Request

```json
{
    "ABC": 1.0,
    "XYZ": 2.517
}
```

### GET /rates

This is currently an open endpoint but it may become admin- and user-only in the future.

Get all of the node's exchange rates.

#### Response

```json
{
    "ABC": 1.0,
    "XYZ": 2.517
}
```

### PUT /routes/static

Admin only.

Configure static routes for the node. These will override routes received by CCP broadcast from other nodes.

#### Request

```json
{
    "example.some-prefix": 0,
    "example.other.more-specific.prefix": 4
}
```

### PUT /routes/:prefix

Admin only.

Configure a single route.

#### Request

```
"4"
```

### GET /routes

### PUT /settlement/engines

Admin only.

Configure the default settlement engines to use for the given asset codes. 
If an account is not configured with a `settlement_engine_url` but the account's `asset_code`
has a settlement engine configured here, the account will automatically be set up to use that settlement engine.

#### Request

```
{
    "ABC": "http://localhost:30001",
    "XYZ": "http://localhost:3002"
}
```