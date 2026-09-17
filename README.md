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

Deploy it over SSH with the `DEPLOY_TARGET`, `USER_TARGET`, and `DEPLOY_PATH` values from `.env`:

```sh
just deploy
just run-remote # deploy, restart the user service, and follow logs
just logs      # follow logs without deploying
just status
just stop
```

The deploy recipe incrementally uploads the binary to `DEPLOY_PATH` with `rsync` and uploads the user service to `DEPLOY_SERVICE_PATH` with `scp`. Both use temporary files and rename them into place. Interrupted binary transfers retain partial data for resuming. `rsync` must be installed locally and on the handheld. `DEPLOY_SERVICE_PATH` defaults to the global user-unit directory, `/etc/systemd/user/marina-shell.service`. Override it when needed. Override the SSH options when needed:

```sh
MARINA_SSH_OPTS="-i /home/user/.ssh/handheld" just deploy
```
