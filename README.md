# (Work in Progress) Interledger Rust Implementation

## TODOs

### STREAM

- [x] Stream server
- [x] Handle incoming money
- [x] Check stream packet responses correspond to the right requests
- [x] Sending data
- [x] Stream and connection closing
- [x] Max packet amount
- [ ] Window + congestion avoidance
- [ ] Respect flow control
- [ ] ERROR HANDLING

### Language bindings
- [ ] Node using Neon
- [ ] WASM using wasm-pack
- [ ] Python

### Plugin

- [ ] Request ID handling - should the plugin track the next outgoing ID?
- [ ] Balance logic
- [ ] Payment channels
- [ ] External store

### Connector

- [ ] Static routing table, multiple plugins
- [ ] Routing protocol
- [ ] Scaling plugin store and other persistance

### Performance

- [ ] Zero-copy parsing