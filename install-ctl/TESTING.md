# install-ctl testing

`cargo test -p install-ctl` is self-contained. Its test-only
`install-ctl-test-support` dependency builds the raw credential and SSH helper
with Cargo's selected compiler; no Python interpreter, manual helper build, or
caller-supplied helper path is required.

The remote-guidance tests use loopback HTTP with real `git http-backend` and
`git upload-pack`. They exercise authentication, credential redaction,
filtering, cancellation, and SSH forwarding against disk-backed repositories.
Git must therefore be available on `PATH`.

Run the installer and fixture-support suites once:

```sh
cargo test -p install-ctl -p install-ctl-test-support
```

No interpreter traps, PATH overrides, preparatory fixture commands, or repeated
serial/parallel/alternate-build runs are required.

Current runtime validation is on Linux (WSL2 Ubuntu); Windows/macOS execution
has not been verified.
