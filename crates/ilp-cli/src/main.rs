mod interpreter;
mod parser;

pub fn main() {
    // Define the arguments to the CLI
    let app = parser::build();

    // Interpret this CLI invocation
    interpreter::run(app);
}
