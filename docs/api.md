# ILP Node HTTP API

For instructions on running the ILP Node, see the [Readme](../README.md).

## Authorization

The ILP Node uses [HTTP Bearer](https://tools.ietf.org/html/rfc6750) tokens for authorization.

Your HTTP requests should look like:

```
GET /accounts HTTP/1.1
Authorization: Bearer BEARER-TOKEN-HERE
```

For administrative functionalities, the value of the token must be the value of `admin_auth_token` when the node was launched. When authorizing as a user, it must be the `ilp_over_http_incoming_token` which was specified during that user's account creation.

## HTTP REST API

### **By default, the API is available on port `7770` and it exposes endpoints as specified in [this OpenAPIv3 specification](https://app.swaggerhub.com/apis/interledger-rs/Interledger/1.0)  ([corresponding yml file](./api.yml)).**

## WebSockets API 

### `/accounts/:username/payments/incoming`

Admin or account-holder only.

#### Message

In the format of text message of WebSocket, the endpoint will send the following JSON when receiving payments:

```json
{
    "to_username": "Receiving account username",
    "from_username": "Sending account username",
    "destination": "Destination ILP address",
    "amount": 1000,
    "timestamp": "Receiving time in RFC3339 format"
}
```

Note that the `from_username` corresponds to the account that received the packet _on this node_, not the original sender.


### `/accounts/:username/ilp/btp` - Bilateral Transfer Protocol (BTP)

Account-holder only.

This endpoint implements BTP, a WebSocket-based protocol for sending and receiving ILP packets. This protocol is specified in [IL-RFC 22: Bilateral Transfer Protocol 2.0 (BTP/2.0)](https://github.com/interledger/rfcs/blob/master/0023-bilateral-transfer-protocol/0023-bilateral-transfer-protocol.md).

Note this endpoint is the one referred to as `ilp_over_btp_url` in the `AccountSettings`.