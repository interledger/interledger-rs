# Interledger Router

The router implements an incoming service and includes an outgoing service.

It determines the next account to forward to and passes it on. Both incoming and outgoing services can respond to requests but many just pass the request on. It stores a RouterStore which stores the entire routing table. 

Once it receives a Prepare, it checks its destination in its routing table. If the destination exists in the routing table it forwards it there, otherwise it searches for a route where the prefix matches the address and forwards it there.
