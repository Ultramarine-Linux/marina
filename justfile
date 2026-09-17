set dotenv-load := true
set shell := ["sh", "-eu", "-c"]

target := env_var_or_default("MARINA_TARGET", "aarch64-unknown-linux-gnu")
package := "marina-ui-slint"
binary := "marina-ui-slint"
service := "systemd/marina-shell.service"
service_name := "marina-shell.service"
deploy_target := env_var_or_default("DEPLOY_TARGET", "root@handheld")
user_target := env_var_or_default("USER_TARGET", "ultramarine@handheld")
deploy_path := env_var_or_default("DEPLOY_PATH", "/opt/marina/marina-ui-slint")
deploy_service_path := env_var_or_default("DEPLOY_SERVICE_PATH", "/etc/systemd/user/marina-shell.service")
ssh_opts := env_var_or_default("MARINA_SSH_OPTS", "")


# Check the native workspace.
check:
    cargo check --workspace

# Build the graphical application for the handheld target.
cross-build:
    CARGO_INCREMENTAL=1 cross build --config 'build.rustc-wrapper=""' --target {{target}} -p {{package}}

# Upload the cross-compiled binary and user service to the configured handheld.
deploy: cross-build deploy-service
    ssh {{ssh_opts}} {{deploy_target}} "mkdir -p \"$(dirname '{{deploy_path}}')\""
    rsync -e "ssh {{ssh_opts}}" --archive --compress --progress --stats "target/{{target}}/debug/{{binary}}" "{{deploy_target}}:{{deploy_path}}"
    ssh {{ssh_opts}} {{deploy_target}} "chmod +x '{{deploy_path}}'"

deploy-service:
    ssh {{ssh_opts}} {{deploy_target}} "mkdir -p \"$(dirname '{{deploy_service_path}}')\""
    scp {{ssh_opts}} "{{service}}" "{{deploy_target}}:{{deploy_service_path}}.new"
    ssh {{ssh_opts}} {{deploy_target}} "mv '{{deploy_service_path}}.new' '{{deploy_service_path}}'"

# Deploy, restart the graphical user service, and follow its logs over SSH.
run-remote: deploy
    ssh {{ssh_opts}} {{user_target}} 'uid=$(id -u); XDG_RUNTIME_DIR=/run/user/$uid systemctl --user daemon-reload && XDG_RUNTIME_DIR=/run/user/$uid systemctl --user enable {{service_name}} && XDG_RUNTIME_DIR=/run/user/$uid systemctl --user restart {{service_name}}'
    ssh {{ssh_opts}} {{user_target}} 'uid=$(id -u); XDG_RUNTIME_DIR=/run/user/$uid journalctl --user -u {{service_name}} -n 100 -f --no-pager'

# Follow logs from the already-running remote user service.
logs:
    ssh {{ssh_opts}} {{user_target}} 'uid=$(id -u); XDG_RUNTIME_DIR=/run/user/$uid journalctl --user -u {{service_name}} -n 100 -f --no-pager'

# Show the remote user service status and recent logs.
status:
    ssh {{ssh_opts}} {{user_target}} 'uid=$(id -u); XDG_RUNTIME_DIR=/run/user/$uid systemctl --user status {{service_name}} --no-pager'

# Stop the remote user service without rebuilding or deploying.
stop:
    ssh {{ssh_opts}} {{user_target}} 'uid=$(id -u); XDG_RUNTIME_DIR=/run/user/$uid systemctl --user stop {{service_name}}'

# Run the native workspace tests.
test:
    cargo test --workspace

# Build and deploy in one command (the default deployment entry point).
default: deploy
