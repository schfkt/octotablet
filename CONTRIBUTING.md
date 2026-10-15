# Development checks

Use a Rust toolchain with the `rustfmt` and `clippy` components. The full checks
build the examples as well as the library. Linux development therefore needs
SDL2, Wayland, and xkbcommon development libraries, a C compiler, and pkg-config
available to Cargo. Windows checks use the Windows Ink backend.

On Linux, the Nix development shell supplies these tools and libraries, including
a C compiler:

```sh
nix develop
```

Lefthook is installed globally, separately from the Nix shell and the crate.
Enable the pre-commit hook after cloning:

```sh
lefthook install
```

Run the same checks manually with:

```sh
lefthook run pre-commit
```

With Nix, run the checks directly using `nix develop -c lefthook run pre-commit`.
Commit from inside `nix develop` so the installed hook can find its dependencies.

The hook checks formatting without modifying files, runs Clippy over all targets
with all features and warnings treated as errors, then runs the tests with all
features and with all tablet backends disabled. Cargo uses the checked-in lockfile.
The jobs run sequentially and stop at the first failure.

All-feature checks cover the backend supported by the host OS. Run them on both
Linux and Windows before changing both platform implementations; enabling all
features on Linux does not compile Windows Ink. Unit tests cover protocol
conversion without a live compositor, but a real tablet is still needed to
validate pen input, proximity, and hardware-specific axes end to end.

The initial fork review and the remediation record are in `docs/review/`.
