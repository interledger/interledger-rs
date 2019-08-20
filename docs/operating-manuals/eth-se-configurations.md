
# Ethereum Settlement Engine Configurations

The parameters are shown in the following format.

- `parameter name`
    - format
        - example
    - explanation
    
You can pass the parameters as either commandline arguments or environment variables when you run `node` or the settlement engine of `ethereum-ledger`.

```
# passing as a commandline argument
# --{parameter name} {value}
cargo run --package interledger -- node --ilp_address example.alice --other_parameter other_value

# passing as an environment variable
# {parameter name (typically in capital)}={value}
ILP_ADDRESS=example.alice OTHER_PARAMETER=other_value cargo run --package interledger -- node
``` 

### Parameters

to be written...
