---
title: Contributing
---

# Contributing
Welcome to Interledger.rs!

Interledger.rs is an implementation of Interledger Protocol written in Rust language. If you are considering to contribute to Interledger.rs, please read this document first of all.

## Prerequisites
You can learn some prerequisites with the following references.

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
    - If you have any opinion on how Interledger works, please visit [Interledger Forum](https://forum.interledger.org/) and post your opinion.
- On Interledger.rs
    - If you have any opinion on the features of Interledger.rs, please [open issues](https://github.com/emschwartz/interledger-rs/issues) describing what you think we need.

## Pull Requests
We welcome pull requests (PRs) that address reported issues. Please follow the instruction below when making pull requests.

- Make sure that your branch is forked from the very recent `master` branch.
- Make sure that you wrote tests, ran it and the results were all green.
- Make sure to run `cargo fmt` before you commit.
    - To install rustfmt, run `rustup component add rustfmt`
- Make sure that you committed using [commitizen](https://github.com/commitizen/cz-cli) and commits are squashed into a single commit.
- Make pull requests against `master` branch of this repository (emschwartz/interledger-rs) from your repository.
- If reviewers request some changes, please follow the instruction or make discussions if you have any constructive opinions on the PRs you made.
    - Then if you want to make some changes on your PRs, `push -f` is allowed to renew your branch after squashing your new commits. You don't need to open new PRs.

## Bug Reports
If you find any suspicious bugs, please feel free to report it on [issues](https://github.com/emschwartz/interledger-rs/issues). The similar bugs have possibly been reported before, so if you have time to take a look at existing [issues](https://github.com/emschwartz/interledger-rs/issues?utf8=✓&q=is%3Aissue), that would be really appreciated to avoid reporting duplicated bugs.

To resolve the problem we need a detailed report of the bug. When you report your bugs, refer to the following example of a report.

```
Short summary: <summary>
Commit hash or branch: <hash or branch> (or even tag etc. to determine exact version)
How to reproduce the bug: <how to reproduce the bug> (procedure or sample code)
Description: <description> (explanation and/or backtrace)
```

## Questions
If you have any questions, post it on [Interledger Forum](https://forum.interledger.org/) or [Interledger Slack #rust](https://interledger.slack.com/messages/CHC51E54J).