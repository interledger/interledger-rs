function hook_before_kill() {
    test_equals_or_exit '{"balance":"-500"}' test_http_response_body -H "Authorization: Bearer hi_alice" http://localhost:7770/accounts/alice/balance
    test_equals_or_exit '{"balance":"0"}' test_http_response_body -H "Authorization: Bearer hi_alice" http://localhost:7770/accounts/bob/balance
    test_equals_or_exit '{"balance":"0"}' test_http_response_body -H "Authorization: Bearer hi_bob" http://localhost:8770/accounts/alice/balance
    test_equals_or_exit '{"balance":"0"}' test_http_response_body -H "Authorization: Bearer hi_bob" http://localhost:8770/accounts/charlie/balance
    test_equals_or_exit '{"balance":"0"}' test_http_response_body -H "Authorization: Bearer hi_charlie" http://localhost:9770/accounts/bob/balance
    test_equals_or_exit '{"balance":"500"}' test_http_response_body -H "Authorization: Bearer hi_charlie" http://localhost:9770/accounts/charlie/balance
}
