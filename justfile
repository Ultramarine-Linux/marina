set dotenv-load := true
set shell := ["sh", "-eu", "-c"]

target := env_var_or_default("MARINA_TARGET", "aarch64-unknown-linux-gnu")
package := "marina-ui-slint"
service_name := "marina-shell.service"
sysext_image := "target/marina-sysext/marina.raw"
deploy_target := env_var_or_default("DEPLOY_TARGET", "root@handheld")
user_target := env_var_or_default("USER_TARGET", "ultramarine@handheld")
sysext_path := env_var_or_default("SYSEXT_PATH", "/var/lib/extensions/marina.raw")
ssh_opts := env_var_or_default("MARINA_SSH_OPTS", "")

# Check the native workspace.
check:
    cargo check --workspace

# Build the graphical application for the handheld target.
cross-build:
    CARGO_INCREMENTAL=1 cross build --config 'build.rustc-wrapper=""' --target {{ target }} -p {{ package }} --release

# Build the privileged PortMaster helpers for the handheld target.
portmaster-helper-build:
    cross build --config 'build.rustc-wrapper=""' --target {{ target }} --release -p marina-portmaster --bin marina-portmaster-runtime --bin marina-portmaster-launch

# Build one AArch64 system extension containing Marina and PortMaster support.
sysext-build: cross-build portmaster-helper-build
    scripts/build-sysext

# Atomically replace the extension image, then refresh its /usr and /opt overlays.
deploy: sysext-build
    ssh {{ ssh_opts }} {{ deploy_target }} "mkdir -p \"$(dirname '{{ sysext_path }}')\""
    rsync -e "ssh {{ ssh_opts }}" --archive --compress --progress --stats "{{ sysext_image }}" "{{ deploy_target }}:{{ sysext_path }}.new"
    ssh {{ ssh_opts }} {{ deploy_target }} "mv '{{ sysext_path }}.new' '{{ sysext_path }}' && systemd-sysext refresh"

# Compatibility target: PortMaster is included in the same system extension.
deploy-portmaster: deploy

# Deploy, restart the graphical user service, and follow its structured JSON logs over SSH.
run-remote: deploy
    ssh {{ ssh_opts }} {{ user_target }} 'uid=$(id -u); XDG_RUNTIME_DIR=/run/user/$uid systemctl --user daemon-reload && XDG_RUNTIME_DIR=/run/user/$uid systemctl --user enable {{ service_name }} && XDG_RUNTIME_DIR=/run/user/$uid systemctl --user restart {{ service_name }}'
    ssh {{ ssh_opts }} {{ user_target }} 'uid=$(id -u); XDG_RUNTIME_DIR=/run/user/$uid journalctl --user -u {{ service_name }} -n 100 -f --no-pager --all --output=json'

# Follow one complete JSON object per event from the already-running remote user service.
logs:
    ssh {{ ssh_opts }} {{ user_target }} 'uid=$(id -u); XDG_RUNTIME_DIR=/run/user/$uid journalctl --user -u {{ service_name }} -n 100 -f --no-pager --all --output=json'

# Show the remote user service status and recent logs.
status:
    ssh {{ ssh_opts }} {{ user_target }} 'uid=$(id -u); XDG_RUNTIME_DIR=/run/user/$uid systemctl --user status {{ service_name }} --no-pager'

# Stop the remote user service without rebuilding or deploying.
stop:
    ssh {{ ssh_opts }} {{ user_target }} 'uid=$(id -u); XDG_RUNTIME_DIR=/run/user/$uid systemctl --user stop {{ service_name }}'

# Run the native workspace tests.
test:
    cargo test --workspace

# Build and deploy in one command (the default deployment entry point).
default: deploy
