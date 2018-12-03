# Python Bindings

**Requirements**
To build the python bindings yourself, you need:

- Rust Nightly Compiler (installl using `[rustup](https://rustup.rs/) install nightly`)
- `pyo3-pack` (install with `cargo install pyo3-pack`)
- `virtualenv` (install with `pip install virtualenv`)

For development purposes, create a virtual environment (`virtualenv <DIR>`) and activate it (`<DIR>/bin/activate`), then run `pyo3-pack develop` in `ilp-rs/bindings/python`.

**Usage Example**
```
>>> from spsp import pay
>>> pay("http://localhost:3000",100)
100
```
