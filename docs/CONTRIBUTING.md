---
title: Contributing
---

# Contributing
Welcome to Interledger.rs!

Interledger.rs is an implementation of the [Interledger Protocol (ILP)](https://interledger.org) written in the Rust programming language. Contributions are very welcome and if you're interested in getting involved, this document provides some background on the project and how we work.

## Prerequisites
Before diving in to Interledger.rs, you may find it helpful to familiarize yourself with some aspects of the Interledger Protocol itself and the Rust-based technologies we use:

- Knowledge about Interledger Protocol itself
    - [interledger.org](https://interledger.org/)
    - [RFCs](https://github.com/interledger/rfcs)
- Knowledge about Interledger.rs
    - [Interledger.rs Architecture](architecture.md)
- Knowledge about Rust language
    - [The Book](https://doc.rust-lang.org/book/)
    - [Rust by Example](https://doc.rust-lang.org/stable/rust-by-example/)
    - [Rust Language Cheat Sheet](https://cheats.rs/)
- Knowledge about crates used in Interledger.rs
    - [Futures.rs](https://rust-lang-nursery.github.io/futures-rs/)
    - [Tokio](https://tokio.rs/)
    - [hyper](https://hyper.rs/)

## Feature Requests

- On Interledger
    - If you have questions about how Interledger works or ideas for new features, please visit the [Interledger Forum](https://forum.interledger.org/) and post your questions or comments.
- On Interledger.rs
    - If you have any opinion on the features of Interledger.rs, please [open issues](https://github.com/emschwartz/interledger-rs/issues) describing what you think we need.

## Pull Requests
We welcome pull requests (PRs) that address reported issues. Please follow the instruction below when making pull requests.

- Make sure that your branch is forked from the latest `master` branch.
- Make sure that you wrote tests, ran it and the results were all green.
- Make sure to run `cargo fmt` before you commit.
    - To install rustfmt, run `rustup component add rustfmt`
- Make sure that you committed using the [commitizen](https://github.com/commitizen/cz-cli) format (commit messages should start with `feat:`, `docs:`, `refactor:`, `chore:`, etc). PRs may contain multiple commits but they should generally be squashed into one or a small number of complete change sets (for example, a feature followed by multiple refactors and another commit to add tests and docs should be combined into a single commit for that feature).
- Make pull requests against `master` branch of this repository (emschwartz/interledger-rs) from your repository.
- If reviewers request some changes, please follow the instruction or make discussions if you have any constructive opinions on the PRs you made.
    - Then if you want to make some changes on your PRs, `push -f` is allowed to renew your branch after squashing your new commits. You don't need to open new PRs.

## Bug Reports
If you find any bugs, please feel free to report them by opening an [issue](https://github.com/emschwartz/interledger-rs/issues). Please look through existing [issues](https://github.com/emschwartz/interledger-rs/issues?utf8=✓&q=is%3Aissue) before posting to avoid duplicating other bug reports.

To resolve the problem we need a detailed report of the bug. When you report your bugs, refer to the following example of a report.

```
Short summary: <summary>
Commit hash or branch: <hash or branch> (or even tag etc. to determine exact version)
How to reproduce the bug: <how to reproduce the bug> (procedure or sample code)
Description: <description> (explanation and/or backtrace)
```

## Questions
If you have any questions, you can get in touch with us by posting on the [Interledger Forum](https://forum.interledger.org/) or in the [#rust channel of the Interledger Slack](https://interledger.slack.com/messages/CHC51E54J).

Once again, welcome to Interledger.rs -- we look forward to working with you!
