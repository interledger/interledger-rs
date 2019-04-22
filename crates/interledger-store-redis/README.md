# Redis Store
> An Interledger.rs store backed by Redis

## Recommended Configuration

See [./redis-example.conf].

## Internal Organization

### Account Details

Account IDs are unsigned 64-bit integers. The `next_account_id` stores the integer that should be used for the next account added to the store.

Static account details as well as balances are stored as hash maps under the keys `accounts:X`, where X is the account ID.

#### Balances

This store keeps track of 4 balance-related values per account:
- Payable Balance (`payable_balance`) - the amount that the operator of this node/store owes to accountholder X
- Receivable Balance (`receivable_balance`) - the amount that accountholder X owes the operator of this node/store
- Pending Incoming Prepares (`pending_incoming`) - the additional amount that accountholder X would owe the operator if all of the in-flight Prepare packets they have sent are fulfilled
- Pending Outgoing Prepares (`pending_outgoing`) - the additional amount that the operator would owe accountholder X if all of the in-flight Prepare packets the operator has sent are fulfilled

The `asset_code` and `asset_scale` for each of the accounts' balances can be found in the Account Details hash map. 
Note that this means that accounts' balances are not directly comparable (for example if account 1's `payable_balance` is 100 and account 2's `payable_balance` is 1000, this does not necessarily mean that we owe accountholder 2 more than accountholder 1, because these values represent completely different assets).

#### Outgoing Auth Tokens

Outgoing auth tokens are encrypted in the following manner:
- The encryption/decryption key is generated as `hmac_sha256(store_secret, "ilp_store_redis_encryption_key")`
- Tokens are encrypted using the AES-256-GCM symmetric encryption scheme using 12-byte randomly generated nonces
- The nonce is appended to the encrypted output (which includes the auth tag) and stored in the DB

### Routing Table

The current routing table is stored as a hash map under the key `routes:current`. The routing table maps ILP address prefixes to the account ID of the "next hop" that the packet should be forwarded to.

Statically configured routes are stored as a hash map of prefix to account ID under the key `routes:static`. These will take precedence over any routes added directly to the current routing table.

### Exchange Rates

Exchange rates are stored as a hash map of currency code to rate under the key `rates:current`.

### HTTP / BTP Auth Details

Incoming HTTP and BTP authentication tokens are stored in hash maps (under `http_auth` and `btp_auth`, respectively) for fast lookup of which account corresponds to a given auth token.

The auth tokens are stored in the following manner:
- The HMAC key is generated as `hmac_sha256(store_secret, "ilp_store_redis_hmac_key")`
- Only the output of `hmac_sha256(hmac_key, auth_token)` is stored in the database
- The hash maps are mappings of the HMAC output to an account ID

### Rate Limiting

This store uses [`redis-cell`](https://github.com/brandur/redis-cell) for rate limiting. This means that the module MUST be loaded when the Redis server is started.

`redis-cell` is used for both packet- and value throughput-based rate limiting. The limits are set on each account in the Account Details.
