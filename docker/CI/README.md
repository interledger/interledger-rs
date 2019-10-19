## Docker Images for CI
### Why Do We Need This?
In short, it is to **reduce the compilation time**.

In the CI process, we are testing runnable `README.md` files which are written in the literate programming style run by `run-md.sh`. Against the `README.md` files, we are testing both:

- **Normal** mode, which does NOT use Docker and just run `cargo build` and `cargo run` commands.
- **Docker** mode, which does use Docker.

For the Docker mode, we need **NEW Docker images** which contain the latest changes. Otherwise it is meaningless to test the markdown files because we definitely want to test the latest build of `interledger-rs`.

So we need new docker images of:

- ilp-node
- ilp-cli

In order to build new docker images, we build the source code once like `cargo build --bin ilp-node --bin ilp-cli`, not twice because we want to reduce compilation time. After the build, we just copy the binaries to docker containers using `CI/node.dockerimage` and `CI/ilp-cli.dockerimage`. That is why we don't use normal `node.dockerfile` and `ilp-cli.dockerfile`. It compiles the source files of `interledger-rs` **inside** the container, so we need **two** compilation of the source although that seems natural for normal uses.

The docker images `node.dockerfile` and `ilp-cli.dockerfile` contains platform-agnostic (although it is not completely agnostic) binaries which means it does not rely on dynamic libraries and uses `musl` instead. So we have to provide `musl` environment for `Cargo`. That is why `circleci-rust-dind.dockerfile` contains `musl`.

Also why `circleci-rust-dind.dockerfile` is an independent image is that there isn't any suitable image which contains `Cargo`, `musl` and `dind` (docker in docker). `dind` is required to test the Docker mode because `run-md.sh` runs docker commands such as `docker run`. It requires docker commands inside the docker image. CircleCI spins up a Docker environment somewhere outer the container. So it would be called rater "docker command in container" than "docker in docker".

![docker commands in container](./images/dind.svg)

In the runnable markdown files, we provide normal mode which does not use Docker. Now we compiled the source code for the Docker images, it seems natural that we use the binaries for the normal mode. That is why we set the default target of `Cargo` to `musl` so that `run-md.sh` never compiles the source code again.
