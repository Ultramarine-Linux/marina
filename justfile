set dotenv-load := true
set shell := ["sh", "-eu", "-c"]

target := env_var_or_default("MARINA_TARGET", "aarch64-unknown-linux-gnu")
package := "marina-ui-slint"
binary := "marina-ui-slint"
service := "systemd/marina-shell.service"
service_name := "marina-shell.service"
portmaster_service := "systemd/portmaster@.service"
portmaster_mount_helper := "scripts/portmaster/usr/local/libexec/portmaster-mount-stack"
portmaster_launch_helper := "scripts/portmaster/usr/local/libexec/portmaster-launch"
portmaster_focus_helper := "scripts/portmaster/usr/local/libexec/portmaster-focus"
portmaster_unmount_helper := "scripts/portmaster/usr/local/libexec/portmaster-unmount-stack"
portmaster_restore_helper := "scripts/portmaster/usr/local/libexec/portmaster-restore-marina"
portmaster_control := "scripts/portmaster/compat/control.txt"
portmaster_mod := "scripts/portmaster/compat/mod_MARINA.txt"
portmaster_libgl := "scripts/portmaster/compat/libgl_default.txt"
portmaster_chmod := "scripts/portmaster/compat/bin/chmod"
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
    CARGO_INCREMENTAL=1 cross build --config 'build.rustc-wrapper=""' --target {{target}} -p {{package}} --release

# Upload the cross-compiled binary and user service to the configured handheld.
deploy: cross-build deploy-service deploy-portmaster
    ssh {{ssh_opts}} {{deploy_target}} "mkdir -p \"$(dirname '{{deploy_path}}')\""
    rsync -e "ssh {{ssh_opts}}" --archive --compress --progress --stats "target/{{target}}/release/{{binary}}" "{{deploy_target}}:{{deploy_path}}"
    ssh {{ssh_opts}} {{deploy_target}} "chmod +x '{{deploy_path}}'"

deploy-service:
    ssh {{ssh_opts}} {{deploy_target}} "mkdir -p \"$(dirname '{{deploy_service_path}}')\""
    scp {{ssh_opts}} "{{service}}" "{{deploy_target}}:{{deploy_service_path}}.new"
    ssh {{ssh_opts}} {{deploy_target}} "mv '{{deploy_service_path}}.new' '{{deploy_service_path}}'"

# Deploy the PortMaster user template and namespace mount helpers.
deploy-portmaster:
    ssh {{ssh_opts}} {{deploy_target}} "mkdir -p /etc/systemd/user /usr/local/libexec /var/games/ports/PortMaster/bin"
    scp {{ssh_opts}} "{{portmaster_service}}" "{{deploy_target}}:/etc/systemd/user/portmaster@.service.new"
    scp {{ssh_opts}} "{{portmaster_mount_helper}}" "{{deploy_target}}:/usr/local/libexec/portmaster-mount-stack.new"
    scp {{ssh_opts}} "{{portmaster_launch_helper}}" "{{deploy_target}}:/usr/local/libexec/portmaster-launch.new"
    scp {{ssh_opts}} "{{portmaster_focus_helper}}" "{{deploy_target}}:/usr/local/libexec/portmaster-focus.new"
    scp {{ssh_opts}} "{{portmaster_unmount_helper}}" "{{deploy_target}}:/usr/local/libexec/portmaster-unmount-stack.new"
    scp {{ssh_opts}} "{{portmaster_restore_helper}}" "{{deploy_target}}:/usr/local/libexec/portmaster-restore-marina.new"
    scp {{ssh_opts}} "{{portmaster_control}}" "{{deploy_target}}:/var/games/ports/PortMaster/control.txt.new"
    scp {{ssh_opts}} "{{portmaster_mod}}" "{{deploy_target}}:/var/games/ports/PortMaster/mod_MARINA.txt.new"
    scp {{ssh_opts}} "{{portmaster_libgl}}" "{{deploy_target}}:/var/games/ports/PortMaster/libgl_default.txt.new"
    scp {{ssh_opts}} "{{portmaster_chmod}}" "{{deploy_target}}:/var/games/ports/PortMaster/bin/chmod.new"
    ssh {{ssh_opts}} {{deploy_target}} "mv /etc/systemd/user/portmaster@.service.new /etc/systemd/user/portmaster@.service && mv /usr/local/libexec/portmaster-mount-stack.new /usr/local/libexec/portmaster-mount-stack && mv /usr/local/libexec/portmaster-launch.new /usr/local/libexec/portmaster-launch && mv /usr/local/libexec/portmaster-focus.new /usr/local/libexec/portmaster-focus && mv /usr/local/libexec/portmaster-unmount-stack.new /usr/local/libexec/portmaster-unmount-stack && mv /usr/local/libexec/portmaster-restore-marina.new /usr/local/libexec/portmaster-restore-marina && mv /var/games/ports/PortMaster/control.txt.new /var/games/ports/PortMaster/control.txt && mv /var/games/ports/PortMaster/mod_MARINA.txt.new /var/games/ports/PortMaster/mod_MARINA.txt && rm -f /var/games/ports/PortMaster/mod_ROCKNIX.txt && mv /var/games/ports/PortMaster/libgl_default.txt.new /var/games/ports/PortMaster/libgl_default.txt && mv /var/games/ports/PortMaster/bin/chmod.new /var/games/ports/PortMaster/bin/chmod && chmod 0755 /usr/local/libexec/portmaster-mount-stack /usr/local/libexec/portmaster-launch /usr/local/libexec/portmaster-focus /usr/local/libexec/portmaster-unmount-stack /usr/local/libexec/portmaster-restore-marina /var/games/ports/PortMaster/control.txt /var/games/ports/PortMaster/mod_MARINA.txt /var/games/ports/PortMaster/bin/chmod /var/games/ports/PortMaster/gptokeyb"

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
