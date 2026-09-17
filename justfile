set dotenv-load := true
set shell := ["sh", "-eu", "-c"]

target := env_var_or_default("MARINA_TARGET", "aarch64-unknown-linux-gnu")
package := "marina-ui-slint"
binary := "marina-ui-slint"
service := "systemd/marina-shell.service"
deploy_target := env_var_or_default("DEPLOY_TARGET", "root@handheld")
deploy_path := env_var_or_default("DEPLOY_PATH", "/opt/marina/marina")
deploy_service_path := env_var_or_default("DEPLOY_SERVICE_PATH", "/etc/systemd/user/marina-shell.service")
ssh_opts := env_var_or_default("MARINA_SSH_OPTS", "")

# Check the native workspace.
check:
    cargo check --workspace

# Build the graphical application for the handheld target.
cross-build:
    cross build --config 'build.rustc-wrapper=""' --release --target {{target}} -p {{package}}

# Upload the cross-compiled binary and user service to the configured handheld.
deploy: cross-build deploy-service
    ssh {{ssh_opts}} {{deploy_target}} "mkdir -p \"$(dirname '{{deploy_path}}')\""
    scp {{ssh_opts}} "target/{{target}}/release/{{binary}}" "{{deploy_target}}:{{deploy_path}}.new"
    ssh {{ssh_opts}} {{deploy_target}} "chmod +x '{{deploy_path}}.new' && mv '{{deploy_path}}.new' '{{deploy_path}}'"

deploy-service:
    ssh {{ssh_opts}} {{deploy_target}} "mkdir -p \"$(dirname '{{deploy_service_path}}')\""
    scp {{ssh_opts}} "{{service}}" "{{deploy_target}}:{{deploy_service_path}}.new"
    ssh {{ssh_opts}} {{deploy_target}} "mv '{{deploy_service_path}}.new' '{{deploy_service_path}}'"

# Build and deploy in one command (the default deployment entry point).
default: deploy
