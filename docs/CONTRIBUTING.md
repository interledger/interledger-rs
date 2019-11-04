---
title: Contributing
---

# Contributing
Welcome to Interledger.rs!

Interledger.rs is an implementation of the [Interledger Protocol (ILP)](https://interledger.org) written in the Rust programming language. Contributions are very welcome and if you're interested in getting involved, this document provides some background on the project and how we work.

## Prerequisites
Before diving in to Interledger.rs, you may find it helpful to familiarize yourself with some aspects of the Interledger Protocol itself and the Rust-based technologies we use:

- Knowledge about the Interledger Protocol itself
    - [interledger.org](https://interledger.org/)
    - [RFCs](https://github.com/interledger/rfcs)
- Knowledge about Interledger.rs
    - [Interledger.rs Architecture](architecture.md)
    - [API Docs](https://docs.rs/interledger)
- Knowledge about the Rust language
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
    - If you have any opinion on the features of Interledger.rs, please [open issues](https://github.com/interledger-s/interledger-rs/issues) describing what you think we need.

## Pull Requests
We welcome pull requests (PRs) that address reported issues. You can find appropriate issues by `bug` or `help-wanted` labels. To avoid multiple PRs for a single issue, please let us know that you are working on it by a comment for the issue. Also we recommend to make discussions on the issue of how to address the issue, methodology towards the resolution or the architecture of your code, in order to avoid writing inefficient or inappropriate code.

Please follow the instruction below when making pull requests.

- Make sure that your branch is forked from the latest `master` branch.
- Make sure that you wrote tests, ran it and the results were all green (required to pass CI).
- Make sure to run `cargo fmt` before you commit (required to pass CI).
     - To install rustfmt, run `rustup component add rustfmt`.
     - To run rustfmt, use `cargo fmt -- --check `.
     - If you would like to make your local setup reject unformatted commits, you can add the command as a pre-commit hook in the file `interledger-rs/.git/hooks/pre-commit`.
- Make sure to run `cargo clippy` before you commit (required to pass CI).
    - To install clippy, run `rustup component add clippy`.
    - To run clippy, use `cargo clippy --all-targets --all-features -- -D warnings`.
    - If you would like to make your local setup reject unformatted commits, you can add the command as a pre-commit hook in the file `interledger-rs/.git/hooks/pre-commit`.
- Make sure to commit using `-s` or `--signoff` option like `git cz -s`.
    - `cz` means using the [commitizen](https://github.com/commitizen/cz-cli) explained below.
    - Why we use the option is explained later in the [Signing-off](#Signing-off) section.
- Make sure that you committed using the [commitizen](https://github.com/commitizen/cz-cli) format (commit messages should start with `feat:`, `docs:`, `refactor:`, `chore:`, etc). PRs may contain multiple commits but they should generally be squashed into one or a small number of complete change sets (for example, a feature followed by multiple refactors and another commit to add tests and docs should be combined into a single commit for that feature).
    - If you would like to make your local setup reject improperly formatted commit headers, you can add the following code to `interledger-rs/.git/hooks/commit-msg`:
```bash
if head -n 1 "$1" | grep -vqE "^(feat|fix|docs|style|refactor|perf|test|chore|ci|build)(\(.{1,30}\))?:[ ].{5,100}$"; then
    echo "Commit message rejected, commit aborted" && exit 1
fi
```
- Make pull requests against `master` branch of this repository (interledger-rs/interledger-rs) from your repository.
- If reviewers request some changes, please follow the instruction or make discussions if you have any constructive opinions on the PRs you made.
    - Then if you want to make some changes on your PRs, `push -f` is allowed to renew your branch after squashing your new commits. You don't need to open new PRs.
- For our [examples](../examples/README.md), we adopted a [literate programming](https://en.wikipedia.org/wiki/Literate_programming) approach. The examples are described in Markdown with shell commands included. The [`run-md.sh`](../scripts/run-md.sh) script parses the commands out of the Markdown file and runs them. If you want to add examples, please make sure your instruction file can be parsed and run by that script.
    - You can check if it is correct with running `../../scripts/run-md.sh README.md` (in your example directory).

### Signing-off
By using `-s` or `--signoff` option, you agree to the following agreement ([Developer Certificate of Origin](https://developercertificate.org/)) which assures that your commits consist of your own code and/or code which you have rights to submit.


```:Developer Certificate of Origin
Developer Certificate of Origin
Version 1.1

Copyright (C) 2004, 2006 The Linux Foundation and its contributors.
1 Letterman Drive
Suite D4700
San Francisco, CA, 94129

Everyone is permitted to copy and distribute verbatim copies of this
license document, but changing it is not allowed.


Developer's Certificate of Origin 1.1

By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or

(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or

(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.

(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off) is
    maintained indefinitely and may be redistributed consistent with
    this project or the open source license(s) involved.

```

## Bug Reports
If you find any bugs, please feel free to report them by opening an [issue](https://github.com/interledger-rs/interledger-rs/issues). Please look through existing [issues](https://github.com/interledger-rs/interledger-rs/issues?utf8=✓&q=is%3Aissue) before posting to avoid duplicating other bug reports.

To resolve the problem we need a detailed report of the bug. When you report your bugs, refer to the following example of a report.

```
Short summary: <summary>
Commit hash or branch: <hash or branch> (or even tag etc. to determine exact version)
How to reproduce the bug: <how to reproduce the bug> (procedure or sample code)
Description: <description> (explanation and/or backtrace)
```

## Questions
If you have any questions, you can get in touch with us by posting on the [Interledger Forum](https://forum.interledger.org/) or in the [#rust channel of the Interledger Slack](https://interledger.slack.com/messages/CHC51E54J) (to join Interledger's Slack workspace, follow [this link](https://communityinviter.com/apps/interledger/interledger-working-groups-slack)).

Once again, welcome to Interledger.rs -- we look forward to working with you!
