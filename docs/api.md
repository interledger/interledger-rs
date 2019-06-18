# ILP Node HTTP API

For instructions on running the ILP Node, see the [Readme](../README.md).

## Authentication

The ILP Node uses HTTP Bearer Token authorization. Most requests must either be authenticated with the admin token configured on the node or the token configured for a particular account.

## Account-Related Routes

By default, the API is available on port `7770`.

### POST /accounts

#### Request

The request must include:

```json
{
    "ilp_address": "example.other-node",
    "asset_code": "ABC",
    "asset_scale": 9
}
```

Optional fields include:

```json
{
    "ilp_address": "example.other-node",
    "asset_code": "ABC",
    "asset_scale": 9,
    "max_packet_amount": 100000000000,
    "min_balance": 0,
    "http_incoming_token": "http bearer token they will use to authenticate with us",
    "http_endpoint": "https://peer-ilp-over-http-endpoint.example/ilp",
    "http_outgoing_token": "http bearer token we will use to authenticate with them",
    "btp_uri": "btp+wss://:auth-token@peer-btp-endpoint",
    "btp_incoming_token": "btp auth token they will use to authenticate with us",
    "settle_threshold": 1000000000,
    "settle_to": 0,
    "send_routes": true,
    "receive_routes": false,
    "routing_relation": "Peer",
    "round_trip_time": 500,
    "amount_per_minute_limit": 1000000000,
    "packets_per_minute_limit": 10
}
```

### GET /accounts

Admin only.

### GET /accounts/:id

Admin or account-holder only.

### GET /accounts/:id/balance

Admin or account-holder only.

#### Response

```json
{
    "balance": 1000
}
```

## SPSP (Sending Payments)

### POST /pay

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

### GET /spsp/:id

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

### Request

```json
{
    "ABC": 1.0,
    "XYZ": 2.517
}
```

### PUT /routes/static

Admin only.

Configure static routes for the node. These will override routes received by CCP broadcast from other nodes.

### Request

```json
{
    "example.some-prefix": 0,
    "example.other.more-specific.prefix": 4
}
```

### PUT /routes/:prefix

Admin only.

Configure a single route.

### Request

```
"4"
```

### GET /routes