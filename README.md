# marina

a funny little emulation/gaming frontend, optimized for resource-constrained handhelds inspired by
[Argosy Launcher](https://github.com/rommapp/argosy-launcher) designed for Ultramarine Embedded

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

The deploy recipe incrementally uploads the binary directly to `DEPLOY_PATH` with `rsync`, so each deployment can delta-transfer against the existing remote binary. The user service is uploaded to `DEPLOY_SERVICE_PATH` with `scp` using a temporary file and rename. If a binary transfer is interrupted, rsync leaves the existing deployed binary untouched and the next run can use it as the delta basis. `rsync` must be installed locally and on the handheld. `DEPLOY_SERVICE_PATH` defaults to the global user-unit directory, `/etc/systemd/user/marina-shell.service`. Override it when needed. Override the SSH options when needed:

```sh
MARINA_SSH_OPTS="-i /home/user/.ssh/handheld" just deploy
```

## PortMaster Support

Marina supports running ports from [PortMaster](https://portmaster.games).

Unlike the official PortMaster runtime however, Marina's PortMaster implementation is sandboxed inside a Linux user namespace.
This allows ports to be in a sandboxed environment mimicking a specific CFW stack
without enforcing its userspace semantics from those CFWs.
Currently Marina attempts to mimic ROCKNIX's userspace environment to accomplish this inside the sandbox.