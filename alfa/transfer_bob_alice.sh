cargo run --bin ilp-cli -- \
  --node http://127.0.0.1:8770 pay bob \
  --auth bob_password \
  --amount 30000000000000000 \
  --to http://127.0.0.1:7770/accounts/alice/spsp
