function hook_before_kill() {
    test_equals_or_exit '{"balance":"-500"}' test_http_response_body -H "Authorization: Bearer admin-a" http://localhost:7770/accounts/alice/balance
    test_equals_or_exit '{"balance":"500"}' test_http_response_body -H "Authorization: Bearer admin-a" http://localhost:7770/accounts/node_b/balance
    test_equals_or_exit '{"balance":"-500"}' test_http_response_body -H "Authorization: Bearer admin-b" http://localhost:8770/accounts/node_a/balance
    test_equals_or_exit '{"balance":"500"}' test_http_response_body -H "Authorization: Bearer admin-b" http://localhost:8770/accounts/bob/balance
}
