# marina

a funny little emulation/gaming frontend, optimized for resource-constrained handhelds inspired by
[Argosy Launcher](https://github.com/rommapp/argosy-launcher) designed for Ultramarine Handheld

very VIP, doesnt even have a proper UI yet

## Cross-compiling and deploying

The graphical application can be built for AArch64 Linux with [`cross`](https://github.com/cross-rs/cross):

```sh
just cross-build
```

This uses the `aarch64-unknown-linux-gnu` target and produces
`target/aarch64-unknown-linux-gnu/release/marina-ui-slint`.

Deploy it over SSH with the `DEPLOY_TARGET` and `DEPLOY_PATH` values from `.env`:

```sh
just deploy
```

The deploy recipe uploads the binary to `DEPLOY_PATH` and the user service to `DEPLOY_SERVICE_PATH` using temporary files, then renames them into place. `DEPLOY_SERVICE_PATH` defaults to the global user-unit directory, `/etc/systemd/user/marina-shell.service`. Override it when needed. Override the SSH options when needed:

```sh
MARINA_SSH_OPTS="-i /home/user/.ssh/handheld" just deploy
```
