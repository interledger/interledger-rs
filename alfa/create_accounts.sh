
# alice at alice node
cargo run --bin ilp-cli -- \
  --node http://127.0.0.1:7770 accounts create alice \
  --auth hi_alice \
  --ilp-address example.alice \
  --asset-code ETH \
  --asset-scale 18 \
  --ilp-over-http-incoming-token alice_password

# # bob at bob node
# cargo run --bin ilp-cli -- \
#   --node http://127.0.0.1:8770 accounts create bob \
#   --auth hi_bob \
#   --ilp-address example.bob \
#   --asset-code ETH \
#   --asset-scale 18 \
#   --ilp-over-http-incoming-token bob_password

#charlie at charlie node
cargo run --bin ilp-cli -- \
  --node http://127.0.0.1:9770 accounts create charlie \
  --auth hi_charlie \
  --ilp-address example.charlie \
  --asset-code XRP \
  --asset-scale 6 \
  --ilp-over-http-incoming-token charlie_password

# bob at alice node
cargo run  --bin ilp-cli -- \
  --node http://127.0.0.1:7770 accounts create bob \
  --auth hi_alice \
  --ilp-address example.bob \
  --asset-code ETH \
  --asset-scale 18 \
  --settlement-engine-url http://127.0.0.1:3000 \
  --ilp-over-http-incoming-token bob_password \
  --ilp-over-http-outgoing-token alice_password \
  --ilp-over-http-url http://127.0.0.1:8770/accounts/alice/ilp \
  --settle-threshold 1 \
  --settle-to 0 \
  --routing-relation Peer

# alice at bob node
cargo run  --bin ilp-cli -- \
  --node http://127.0.0.1:8770 accounts create alice \
  --auth hi_bob \
  --ilp-address example.alice \
  --asset-code ETH \
  --asset-scale 18 \
  --settlement-engine-url http://127.0.0.1:3001 \
  --ilp-over-http-incoming-token alice_password \
  --ilp-over-http-outgoing-token bob_password \
  --ilp-over-http-url http://127.0.0.1:7770/accounts/bob/ilp \
  --settle-threshold 1 \
  --settle-to 0 \
  --routing-relation Peer

# charlie at bob node
cargo run  --bin ilp-cli -- \
  --node http://127.0.0.1:8770 accounts create charlie \
  --auth hi_bob \
  --ilp-address example.charlie \
  --asset-code XRP \
  --asset-scale 6 \
  --settlement-engine-url http://127.0.0.1:3002 \
  --ilp-over-http-incoming-token charlie_password \
  --ilp-over-http-outgoing-token bob_password \
  --ilp-over-http-url http://127.0.0.1:9770/accounts/bob/ilp \
  --settle-threshold 1 \
  --settle-to 0 \
  --routing-relation Child

# bob at charlie node
cargo run  --bin ilp-cli -- \
  --node http://127.0.0.1:9770 accounts create bob \
  --auth hi_charlie \
  --ilp-address example.bob \
  --asset-code XRP \
  --asset-scale 6 \
  --settlement-engine-url http://127.0.0.1:3003 \
  --ilp-over-http-incoming-token bob_password \
  --ilp-over-http-outgoing-token charlie_password \
  --ilp-over-http-url http://127.0.0.1:8770/accounts/charlie/ilp \
  --settle-threshold 1 \
  --settle-to 0 \
  --routing-relation Parent