# (Work in Progress) Interledger Rust Implementation

## TODOs

### STREAM

- [x] Stream server
- [x] Handle incoming money
- [x] Check stream packet responses correspond to the right requests
- [ ] Sending data
- [ ] Stream and connection closing
- [ ] Determining exchange rate, max packet amount
- [ ] Window + congestion avoidance
- [ ] Respect flow control
- [ ] ERROR HANDLING

### Language bindings
- [ ] Node using Neon
- [ ] WASM using wasm-pack
- [ ] Python

### TLS

- [x] Test out creating STREAM connections using rustls
- [ ] TLS server that listens for multiple connections (Stream<Connection>)
- [ ] DNS for looking up ILP address?

### Plugin

- [ ] Request ID handling - should the plugin track the next outgoing ID?
- [ ] Balance logic
- [ ] Payment channels
- [ ] External store

### Connector

- [ ] Static routing table, multiple plugins
- [ ] Routing protocol
- [ ] Scaling plugin store and other persistance
