# Logging

Logs are created via the `tracing` crates. We define various _scopes_ depending on the operation we want to trace at various debug levels. The log level can be set via the `RUST_LOG` environment variable, and via the `/tracing-level` at runtime by the node operator.

For each request we track various information depending on the error log lvel:
- **Incoming**:
    - `ERROR`:
        - `request.id`: a randomly generated uuid for that specific request
        - `prepare.destination`: the destination of the prepare packet inside the request
        - `prepare.amount`: the amount in the prepare packet inside the request
        - `from.id`: the request sender's account uuid
    - `DEBUG`:
        - `from.username`: the request sender's username
        - `from.ilp_address`: the request sender's ilp address
        - `from.asset_code`: the request sender's asset code
        - `from.asset_scale`: the request sender's asset scale
- **Forwarding** (this is shown when an incoming request is turned into an outgoing request and is being forwarded to a peer):
    - `ERROR`:
        - `prepare.amount`: the amount in the prepare packet inside the request
        - `from.id`: the request sender's account uuid
    - `DEBUG`:
        - `to.username`: the request receiver's username
        - `to.ilp_address`: the request receiver's ilp address
        - `to.asset_code`: the request receiver's asset code
        - `to.asset_scale`: the request receiver's asset scale
- **Outgoing**: 
    - `ERROR`:
        - `request.id`: a randomly generated uuid for that specific request
        - `prepare.destination`: the destination of the prepare packet inside the request
        - `from.id`: the request sender's account uuid
        - `to.id`: the request receiver's account uuid
    - `DEBUG`:
        - `from.username`: the request sender's username
        - `from.ilp_address`: the request sender's ilp address
        - `from.asset_code`: the request sender's asset code
        - `from.asset_scale`: the request sender's asset scale
        - `to.username`: the request receiver's username
        - `to.ilp_address`: the request receiver's ilp address
        - `to.asset_code`: the request receiver's asset code
        - `to.asset_scale`: the request receiver's asset scale

Then, depending on the response received for the request, we add additional information to that log:
- `Fulfill`: We add a scope `"result = fulfill"` at the `DEBUG` level
    - `fulfillment`: the fulfill packet's fulfillment condition
- `Reject`: We add a scope `"result = "reject"` at the INFO level
    - `reject.code`: the reject packet's error code field
    - `reject.message`: the reject packet's message field
    - `reject.triggered_by`: the reject packet's triggered_by field