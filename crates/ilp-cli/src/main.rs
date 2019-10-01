mod interpreter;
mod parser;
use std::process::exit;

pub fn main() {
    // 1. Define the arguments to the CLI application
    let app = parser::build();

    // 2. Parse the command line
    let matches = app.clone().get_matches();

    // 3. Interpret this CLI invocation
    let result = interpreter::run(&matches);

    // 4. Handle interpreter output
    match result {
        Err(interpreter::Error::UsageErr(s)) => {
            // Clap doesn't seem to have a built-in way of manually printing the
            // help text for an arbitrary subcommand, but this works just the same.
            app.get_matches_from(s.split(' '));
        }
        Err(interpreter::Error::ClientErr(e)) => {
            eprintln!("ILP CLI error: failed to send request: {}", e);
            exit(1);
        }
        Ok(mut response) => match response.text() {
            Err(e) => {
                eprintln!("ILP CLI error: Failed to parse HTTP response: {}", e);
                exit(1);
            }
            Ok(body) => {
                if response.status().is_success() {
                    if !matches.is_present("quiet") {
                        println!("{}", body);
                    }
                } else {
                    eprintln!(
                        "ILP CLI error: Unexpected response from server: {}: {}",
                        response.status(),
                        body,
                    );
                    exit(1);
                }
            }
        },
    }
}

#[cfg(test)]
// Note that this module contains interface tests, not integration tests.
// These exist to detect changes to the parser or interpreter that cause
// breakage to the tool's user-facing interface.
// Since this doesn't require a properly configured (or mocked) server,
// errors from the interpreter are entirely expected, though we still
// run the interpreter in order to detect panics.
// Conveniently this section also serves as a reference for example invocations.
mod interface_tests {
    use crate::interpreter;
    use crate::parser;

    #[test]
    fn ilp_cli() {
        should_parse(&[
            "ilp-cli --quiet status",    // quiet
            "ilp-cli --node bar status", // non-default node
        ]);
    }

    #[test]
    fn accounts_balance() {
        should_parse(&[
            "ilp-cli accounts balance alice --auth foo", // minimal
        ]);
    }

    #[test]
    fn accounts_create() {
        should_parse(&[
            "ilp-cli accounts create alice --auth foo --asset-code ABC --asset-scale 9", // minimal
            "ilp-cli accounts create alice --auth foo --asset-code XYZ --asset-scale 6 --ilp-address bar --max-packet-amount 100 --min-balance 0 --ilp-over-http-url qux --ilp-over-http-incoming-token baz --ilp-over-http-outgoing-token qaz --ilp-over-btp-url spam --ilp-over-btp-outgoing-token ham --ilp-over-btp-incoming-token eggs --settle-threshold 0 --settle-to 0 --routing-relation foobar --round-trip-time 1000 --amount-per-minute-limit 42 --packets-per-minute-limit 4 --settlement-engine-url if_you_can_read_this_congratulations_youve_scrolled_too_far_right", // maximal
            "ilp-cli accounts create alice --auth foo --asset-code ABC --asset-scale 3 --min-balance -1000 --settle-threshold -10", // negative numbers
        ]);
    }

    #[test]
    fn accounts_delete() {
        should_parse(&[
            "ilp-cli accounts delete alice --auth foo", // minimal
        ]);
    }

    #[test]
    fn accounts_incoming_payments() {}

    #[test]
    fn accounts_info() {
        should_parse(&[
            "ilp-cli accounts info alice --auth foo", // minimal
        ]);
    }

    #[test]
    fn accounts_list() {
        should_parse(&[
            "ilp-cli accounts list --auth foo", // minimal
        ]);
    }

    #[test]
    fn accounts_update() {
        should_parse(&[
            "ilp-cli accounts update alice --auth foo", // minimal, non-admin
            "ilp-cli accounts update alice --auth foo --ilp-over-http-incoming-token bar --ilp-over-btp-incoming-token qux --ilp-over-http-outgoing-token baz --ilp-over-btp-outgoing-token qaz --ilp-over-http-url spam --ilp-over-btp-url eggs --settle-threshold 0 --settle-to 0", // maximal, non-admin
            "ilp-cli accounts --admin update alice --auth foo", // minimal, admin
            "ilp-cli accounts --admin update alice --auth foo --asset-code XYZ --asset-scale 6 --ilp-address bar --max-packet-amount 100 --min-balance 0 --ilp-over-http-url qux --ilp-over-http-incoming-token baz --ilp-over-http-outgoing-token qaz --ilp-over-btp-url spam --ilp-over-btp-outgoing-token ham --ilp-over-btp-incoming-token eggs --settle-threshold 0 --settle-to 0 --routing-relation foobar --round-trip-time 1000 --amount-per-minute-limit 42 --packets-per-minute-limit 4 --settlement-engine-url if_you_can_read_this_congratulations_youve_scrolled_too_far_right", // maximal, admin
            "ilp-cli accounts update alice --auth foo --settle-threshold -1000 --settle-to -10", // negative numbers
        ]);
    }

    #[test]
    fn pay() {
        should_parse(&[
            "ilp-cli pay alice --auth foo --source-amount 500 --receiver bar", // minimal
        ]);
    }

    #[test]
    fn rates_list() {
        should_parse(&[
            "ilp-cli rates list", // minimal
        ]);
    }

    #[test]
    fn rates_set_all() {
        should_parse(&["ilp-cli rates set-all --auth foo -r bar 1.0 -r baz 2.0"])
    }

    #[test]
    fn routes_list() {
        should_parse(&[
            "ilp-cli routes list", // minimal
        ]);
    }

    #[test]
    fn routes_set() {}

    #[test]
    fn routes_set_all() {
        should_parse(&["ilp-cli routes set-all --auth foo -r bar qux -r baz qaz"])
    }

    #[test]
    fn settlement_engines_set_all() {}

    #[test]
    fn status() {
        should_parse(&[
            "ilp-cli status", // minimal
        ]);
    }

    fn should_parse(examples: &[&str]) {
        let mut app = parser::build();
        for example in examples {
            let parser_result = app.get_matches_from_safe_borrow(example.split(' '));
            match parser_result {
                Err(e) => panic!("Failure while parsing command `{}`: {}", example, e),
                Ok(matches) => {
                    // Any unanticipated errors at this stage will result in a panic from
                    // within the interpreter, so no need to manually check the result.
                    let _interpreter_result = interpreter::run(&matches);
                }
            }
        }
    }
}
