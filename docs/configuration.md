### Configuration

Interledger.rs binaries such as `ilp-node` and `interledger-settlement-engines` accept configuration options in the following ways:

#### Environment variables

```bash #
# Passing as environment variables
# {parameter name (typically in capital)}={value}
# note that the parameter names MUST begin with a prefix of "ILP_" e.g. ILP_SECRET_SEED
ILP_ADDRESS=example.alice \
ILP_OTHER_PARAMETER=other_value \
cargo run
```

#### Standard In (stdin)

```bash #
# Passing from STDIN in JSON, TOML, YAML format.
some_command | cargo run
```

#### Configuration files

```bash #
# Passing by a configuration file in JSON, TOML, YAML format.
# The first argument after subcommands such as `node` is the path to the configuration file.
# Note that in order for a docker image to have access to a local file, it must be included in
# a directory that is mounted as a Volume at `/config`
cargo run -- config.yml
```

#### Command line arguments

```bash #
# Passing by command line arguments.
# --{parameter name} {value}
cargo run -- --admin_auth_token super-secret
```

Note that configurations are applied in the following order of priority:
1. Environment Variables
1. Stdin
1. Configuration files
1. Command line arguments.
