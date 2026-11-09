# Contributing

Thanks for wanting to help. Contributions are under the project's MIT License.

## Before a pull request

- `cargo fmt --all`, `cargo clippy --workspace --all-features --all-targets -- -D warnings` and
  `cargo test --workspace --all-features` pass.
- The Python package's tests pass: `pip install "./crates/hdmv-menus-py[test]"`, then
  `pytest crates/hdmv-menus-py/tests`.
- A change to the C interface comes with the header the build makes (`crates/hdmv-menus-c/include`).
- New behaviour has a test. Tests use bytes made for them, never a disc's own files.

## What stays out

- A disc's own files or what's read from them (menu pictures, decrypted streams): they're someone's
  copyrighted work.
- Decryption keys, key lists, or decryption libraries, or links to them. Decryption stays the caller's,
  through a library they already have.
- Code translated from libbluray or other LGPL or GPL projects: this library is MIT, so its code is
  written from the formats themselves.
