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
`target/aarch64-unknown-linux-gnu/release/marina-ui-slint` and
`marina-shell-overlay`. The target sysroot is provisioned with the native
Wayland/XKB/EGL and PipeWire development packages required by the layer-shell renderer and native audio controls. Native development additionally requires the distribution's `pipewire-devel` package.

Package and deploy Marina over SSH with the `DEPLOY_TARGET` and `USER_TARGET` values from `.env`:

```sh
just sysext-build
just deploy
just run-remote # deploy, restart the user service, and follow logs
just logs       # follow logs without deploying
just status
just stop
```

`just sysext-build` cross-compiles Marina and its PortMaster helpers for AArch64, stages them with their systemd units and policy files, and packages the result with `mkosi` as `target/marina-sysext/marina.raw`. The image contains `ARCHITECTURE=arm64` sysext metadata and is compatible with any host OS release.

`just deploy` uploads that one image to `SYSEXT_PATH` (default: `/var/lib/extensions/marina.raw`) under a temporary name, atomically replaces the old image, and runs `systemd-sysext refresh`. The extension overlays `/usr`, installing the graphical application at `/usr/bin/marina-ui-slint`; installed PortMaster games, runtime images, and saves remain persistent under `/var/games`. Local builds require `mkosi`, `mkfs.erofs` (from `erofs-utils`), and `rsync`; the handheld requires `rsync` and `systemd-sysext`. Override the SSH options when needed:

```sh
MARINA_SSH_OPTS="-i /home/user/.ssh/handheld" just deploy
```

## PortMaster Support

Marina supports running ports from [PortMaster](https://portmaster.games).

Unlike the official PortMaster runtime however, Marina's PortMaster implementation is sandboxed inside a Linux user namespace.
This allows ports to be in a sandboxed environment mimicking a specific CFW stack
without enforcing its userspace semantics from those CFWs.
Currently Marina attempts to mimic ROCKNIX's userspace environment to accomplish this inside the sandbox.