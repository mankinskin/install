# install-ctl testing

`cargo test -p install-ctl` is self-contained. Its test-only
`install-ctl-test-support` dependency builds the raw credential and SSH helper
with Cargo's selected compiler; no Python interpreter, manual helper build, or
caller-supplied helper path is required.

The remote-guidance tests use loopback HTTP with real `git http-backend` and
`git upload-pack`. They exercise authentication, credential redaction,
filtering, cancellation, and SSH forwarding against disk-backed repositories.
Git must therefore be available on `PATH`.

For the dedicated hostile-environment validation, run the support-contract
target first with `INSTALL_CTL_FIXTURE_EVIDENCE_ROOT`. It writes a trap-first
`python-denial.path`; use that generated path only for the subsequent
Python-denial suite run. This acceptance setup is not needed for ordinary
test commands.

The fixture design is portable, but current runtime evidence is Linux-only.
