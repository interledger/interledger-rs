cargo run --bin ilp-cli -- \
  --node http://127.0.0.1:7770 pay alice \
  --auth alice_password \
  --amount 10000000000000000 \
  --to http://127.0.0.1:8770/accounts/bob/spsp
