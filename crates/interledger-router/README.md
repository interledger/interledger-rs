# Interledger Router

The router implements an incoming service and includes an outgoing service.

It is the last path before all requests are responded to

It stores a RouterStore which MUST implement a method to retrieve AccountIds given an address. 

Once it receives a PREPARE, it checks its destination and if it finds it in the routing table 
