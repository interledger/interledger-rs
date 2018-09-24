# (Work in Progress) Interledger Rust Implementation

## TODOs

### STREAM

- [x] Stream server
- [ ] Handle incoming money
- [ ] Sending data
- [ ] Determining exchange rate, max packet amount
- [ ] Window + congestion avoidance
- [ ] Connection-level statistics

### TLS

- [x] Test out creating STREAM connections using rustls
- [ ] TLS server that listens for multiple connections (Stream<Connection>)

### Plugin

- [ ] Split plugin into channels so sending doesn't consume it?
- [ ] Request ID handling - should the plugin track the next outgoing ID?
- [ ] Balance logic
- [ ] Payment channels
- [ ] External store

### Connector

- [ ] Static routing table, multiple plugins
- [ ] Routing protocol
- [ ] Scaling plugin store and other persistance