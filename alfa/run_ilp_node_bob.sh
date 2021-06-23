cargo run --bin ilp-node -- \
  --ilp_address example.bob \
  --secret_seed 1604966725982139900555208458637022875563691455429373719368053354 \
  --admin_auth_token hi_bob \
  --redis_url redis://127.0.0.1:6379/3 \
  --http_bind_address 127.0.0.1:8770 \
  --settlement_api_bind_address 127.0.0.1:8771 \
  --exchange_rate.provider CoinCap 
