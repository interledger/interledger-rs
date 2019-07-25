 #!/bin/bash

# Insert accounts on Node A
# One account represents Alice and the other represents Node B's account with Node A
printf "Alice's account:\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer admin-a" \
    -d @./node-a/alice.json \
    http://localhost:7770/accounts

printf "\nNode B's account on Node A:\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer admin-a" \
    -d @./node-a/node-b.json \
    http://localhost:7770/accounts

# Insert accounts on Node B
# One account represents Bob and the other represents Node A's account with Node B
printf "\nBob's Account:\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer admin-b" \
    -d @./node-b/bob.json \
    http://localhost:8770/accounts

printf "\nNode A's account on Node B:\n"
curl \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer admin-b" \
    -d @./node-b/node-a.json \
    http://localhost:8770/accounts

sleep 1

printf "\n\n"
