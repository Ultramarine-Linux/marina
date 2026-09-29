use std::{
    collections::BTreeSet,
    env,
    ffi::CString,
    fs,
    io::{Read, Write},
    os::unix::{ffi::OsStrExt, fs::symlink, net::UnixStream},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

use nix::{
    mount::{MntFlags, MsFlags, mount, umount2},
    sched::{CloneFlags, unshare},
    sys::wait::{WaitStatus, waitpid},
    unistd::{ForkResult, chdir, execve, fork, getgid, getuid, pivot_root},
};

const PORTS_DIR: &str = "/var/games/ports";
const SAVES_DIR: &str = "/var/games/saves/ports";
const PORTMASTER_DIR: &str = "/usr/libexec/marina-portmaster";
const RUNTIME_DIR: &str = "/run/portmaster/runtimes";

fn main() {
    marina_logging::init();

    let exit_code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("PortMaster launch failed: {error}");
            1
        }
    };
    std::process::exit(exit_code);
}

fn run() -> Result<i32, Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let first = args.next().ok_or("missing port instance")?;
    if first == "--stop-runtimes" {
        let encoded_port = args.next().ok_or("missing port instance")?;
        if args.next().is_some() {
            return Err("unexpected cleanup arguments".into());
        }
        stop_required_runtimes(&encoded_port)?;
        return Ok(0);
    }
    if args.next().is_some() {
        return Err("unexpected launcher arguments".into());
    }

    let port = unescape_instance(&first)?;
    validate_component(&port, "port title")?;

    let install_dir = Path::new(PORTS_DIR).join(&port);
    if !install_dir.is_dir() {
        return Err(format!(
            "PortMaster installation does not exist: {}",
            install_dir.display()
        )
        .into());
    }
    let launcher = find_launcher(&install_dir)?;
    let runtimes = required_runtimes(&install_dir, &launcher)?;
    for runtime in &runtimes {
        start_runtime(runtime)?;
    }

    let layout = Layout::new(&port);
    layout
        .create_directories()
        .map_err(|error| format!("prepare PortMaster save directories: {error}"))?;
    let uid = getuid();
    let gid = getgid();
    let (mut parent_socket, child_socket) = UnixStream::pair()?;

    // All namespace changes happen after fork. The parent writes the child's
    // UID/GID maps after `unshare`, matching util-linux `unshare --map-root-user`
    // and preserving the supplemental credentials needed for controller nodes.
    let result = match unsafe { fork()? } {
        ForkResult::Parent { child } => {
            drop(child_socket);
            let mut ready = [0_u8; 1];
            parent_socket.read_exact(&mut ready)?;
            if ready != [b'R'] {
                return Err("PortMaster child did not enter its user namespace".into());
            }
            let configured = configure_user_namespace(child.as_raw(), uid.as_raw(), gid.as_raw());
            let _ = parent_socket.write_all(if configured.is_ok() { b"M" } else { b"E" });
            configured?;
            match waitpid(child, None)? {
                WaitStatus::Exited(_, code) => Ok(code),
                WaitStatus::Signaled(_, signal, _) => Ok(128 + signal as i32),
                status => Err(format!("unexpected PortMaster child status: {status:?}").into()),
            }
        }
        ForkResult::Child => {
            drop(parent_socket);
            if let Err(error) = launch_in_namespace(&layout, &launcher, child_socket) {
                eprintln!("PortMaster sandbox setup failed: {error}");
                std::process::exit(1);
            }
            unreachable!("execve replaces the child process")
        }
    };

    for runtime in &runtimes {
        if let Err(error) = stop_runtime(runtime) {
            eprintln!("PortMaster: runtime cleanup failed: {error}");
        }
    }
    result
}

fn configure_user_namespace(pid: i32, uid: u32, gid: u32) -> std::io::Result<()> {
    let proc = Path::new("/proc").join(pid.to_string());
    fs::write(proc.join("setgroups"), "deny")?;
    fs::write(proc.join("uid_map"), format!("0 {uid} 1"))?;
    fs::write(proc.join("gid_map"), format!("0 {gid} 1"))
}

struct Layout {
    install_dir: PathBuf,
    game_upper: PathBuf,
    game_work: PathBuf,
    home_upper: PathBuf,
    home_work: PathBuf,
    empty_lower: PathBuf,
    ephemeral_upper: PathBuf,
    ephemeral_work: PathBuf,
    home: PathBuf,
}

impl Layout {
    fn new(port: &str) -> Self {
        let saves = Path::new(SAVES_DIR).join(port);
        Self {
            install_dir: Path::new(PORTS_DIR).join(port),
            game_upper: saves.join("data"),
            game_work: saves.join(".work/game"),
            home_upper: saves.join("home"),
            home_work: saves.join(".work/home"),
            empty_lower: saves.join(".empty"),
            ephemeral_upper: saves.join(".work/rootfs/upper"),
            ephemeral_work: saves.join(".work/rootfs/work"),
            home: Path::new("/run/user/1000/portmaster/home").join(port),
        }
    }

    fn create_directories(&self) -> std::io::Result<()> {
        if let Some(rootfs) = self.ephemeral_upper.parent() {
            if rootfs.exists() {
                fs::remove_dir_all(rootfs)?;
            }
        }
        for directory in [
            &self.game_upper,
            &self.game_work,
            &self.home_upper,
            &self.home_work,
            &self.empty_lower,
            &self.ephemeral_upper,
            &self.ephemeral_work,
        ] {
            fs::create_dir_all(directory)?;
        }
        Ok(())
    }
}

fn launch_in_namespace(
    layout: &Layout,
    launcher: &str,
    mut socket: UnixStream,
) -> Result<(), Box<dyn std::error::Error>> {
    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS)
        .map_err(|error| format!("enter user and mount namespaces: {error}"))?;
    socket.write_all(b"R")?;
    let mut mapped = [0_u8; 1];
    socket.read_exact(&mut mapped)?;
    if mapped != [b'M'] {
        return Err("parent could not configure PortMaster user namespace".into());
    }
    drop(socket);

    // Keep every mount made below private to this port's nested namespace.
    mount(
        None::<&str>,
        Path::new("/"),
        None::<&str>,
        MsFlags::MS_REC | MsFlags::MS_PRIVATE,
        None::<&str>,
    )
    .map_err(|error| format!("make inherited mounts private: {error}"))?;

    let staging = StagingLayout::new(std::process::id());
    staging.create()?;
    mount_tmpfs(&staging.new_root, 0o755)?;
    copy_root_symlinks(&staging.new_root)?;

    // Overlay only host OS trees needed by ports. OverlayFS cannot clone the
    // locked inherited root mount as one lower layer, and exposing /var or
    // /home would make unrelated host data visible. Their replacements below
    // therefore start empty in the private root tmpfs.
    for path in ["usr", "etc", "opt", "bin", "sbin", "lib", "lib64"] {
        let source = Path::new("/").join(path);
        if source
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_dir())
        {
            staging.overlay_directory(
                &source,
                &layout.ephemeral_upper.join(path),
                &layout.ephemeral_work.join(path),
            )?;
        }
    }
    for path in ["/var", "/home", "/run", "/roms"] {
        fs::create_dir_all(staging.in_new_root(path))?;
    }
    start_notification_proxy(&staging)?;

    // These are the only inherited mount trees exposed directly. /dev is the
    // systemd-private device tree; the others are kernel or runtime interfaces.
    for path in ["/dev", "/proc", "/sys", RUNTIME_DIR] {
        staging.bind_into_root(Path::new(path), true)?;
    }
    for path in [
        "/run/user/1000/wayland-1",
        "/run/user/1000/pipewire-0",
        "/run/user/1000/pulse",
    ] {
        staging.bind_into_root(Path::new(path), true)?;
    }
    staging.bind_sway_sockets(Path::new("/run/user/1000"))?;

    let ports = staging.in_new_root("/roms/ports");
    fs::create_dir_all(&ports)?;
    mount_overlay(
        &layout.install_dir,
        &layout.game_upper,
        &layout.game_work,
        &ports,
    )?;
    let portmaster = ports.join("PortMaster");
    fs::create_dir_all(&portmaster)?;
    // Merge Marina's compatibility files over the complete upstream payload.
    // Binding the compatibility directory itself would hide payload tools such
    // as gptokeyb and the runtime images.
    mount_lower_overlay(
        &[
            Path::new(PORTMASTER_DIR),
            &Path::new(PORTS_DIR).join("PortMaster"),
        ],
        &portmaster,
    )?;

    let home = staging.in_new_root(&layout.home);
    fs::create_dir_all(&home)?;
    mount_overlay(
        &layout.empty_lower,
        &layout.home_upper,
        &layout.home_work,
        &home,
    )?;
    fs::create_dir_all(home.join(".local/share"))?;
    fs::create_dir_all(home.join(".config"))?;

    let private_tmp = staging.in_new_root("/tmp");
    fs::create_dir_all(&private_tmp)?;
    fs::set_permissions(
        &private_tmp,
        std::os::unix::fs::PermissionsExt::from_mode(0o1777),
    )?;

    let old_root = staging.new_root.join(".old-root");
    fs::create_dir(&old_root)?;
    chdir(&staging.new_root)?;
    pivot_root(Path::new("."), Path::new(".old-root"))
        .map_err(|error| format!("pivot into ephemeral root: {error}"))?;
    chdir("/").map_err(|error| format!("change directory after pivot_root: {error}"))?;
    umount2("/.old-root", MntFlags::MNT_DETACH)
        .map_err(|error| format!("detach inherited root: {error}"))?;
    fs::remove_dir("/.old-root")?;

    chdir("/roms/ports")?;
    exec_launcher(layout, launcher)
}

struct StagingLayout {
    base: PathBuf,
    new_root: PathBuf,
}

impl StagingLayout {
    fn new(pid: u32) -> Self {
        let base = Path::new("/tmp").join(format!(".marina-portmaster-root-{pid}"));
        Self {
            new_root: base.join("root"),
            base,
        }
    }

    fn create(&self) -> std::io::Result<()> {
        fs::create_dir(&self.base)?;
        fs::create_dir(&self.new_root)
    }

    fn in_new_root(&self, path: impl AsRef<Path>) -> PathBuf {
        self.new_root.join(
            path.as_ref()
                .strip_prefix("/")
                .unwrap_or_else(|_| path.as_ref()),
        )
    }

    fn overlay_directory(
        &self,
        source: &Path,
        upper: &Path,
        work: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let target = self.in_new_root(source);
        fs::create_dir_all(&target)?;
        mount_overlay(source, upper, work, target)
    }

    fn bind_into_root(
        &self,
        source: &Path,
        recursive: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if !source.exists() {
            return Ok(());
        }
        let target = self.in_new_root(source);
        prepare_bind_target(source, &target)?;
        bind_mount(source, &target, recursive)
    }

    fn bind_sway_sockets(&self, runtime_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if !runtime_dir.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(runtime_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.as_bytes().starts_with(b"sway-ipc.") && name.as_bytes().ends_with(b".sock") {
                self.bind_into_root(&entry.path(), false)?;
            }
        }
        Ok(())
    }
}

fn copy_root_symlinks(new_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir("/")? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            symlink(
                fs::read_link(entry.path())?,
                new_root.join(entry.file_name()),
            )?;
        }
    }
    Ok(())
}

fn start_notification_proxy(staging: &StagingLayout) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_dir = staging.in_new_root("/run/user/1000");
    fs::create_dir_all(&runtime_dir)?;
    let proxy_socket = runtime_dir.join("bus");
    let upstream_socket = runtime_dir.join(".marina-session-bus");
    prepare_bind_target(Path::new("/run/user/1000/bus"), &upstream_socket)?;
    bind_mount(Path::new("/run/user/1000/bus"), &upstream_socket, false)?;

    let upstream_address = format!("unix:path={}", upstream_socket.display());
    Command::new("/usr/bin/xdg-dbus-proxy")
        .arg(upstream_address)
        .arg(&proxy_socket)
        .args(["--filter", "--talk=org.freedesktop.Notifications"])
        .spawn()
        .map_err(|error| format!("start notification D-Bus proxy: {error}"))?;

    let mut ready = false;
    for _ in 0..100 {
        if proxy_socket.exists() {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    umount2(&upstream_socket, MntFlags::MNT_DETACH)
        .map_err(|error| format!("hide notification proxy upstream socket: {error}"))?;
    fs::remove_file(&upstream_socket)?;

    if ready {
        Ok(())
    } else {
        Err("notification D-Bus proxy did not create its socket".into())
    }
}

fn mount_tmpfs(target: &Path, mode: u32) -> Result<(), Box<dyn std::error::Error>> {
    let options = format!("mode={mode:o}");
    mount(
        Some("tmpfs"),
        target,
        Some("tmpfs"),
        MsFlags::MS_NODEV | MsFlags::MS_NOSUID,
        Some(options.as_str()),
    )
    .map_err(|error| format!("mount tmpfs at {}: {error}", target.display()))?;
    Ok(())
}

fn prepare_bind_target(source: &Path, target: &Path) -> std::io::Result<()> {
    if target.symlink_metadata().is_ok() {
        return Ok(());
    }
    if source.is_dir() {
        fs::create_dir_all(target)
    } else {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::File::create(target).map(drop)
    }
}

fn bind_mount(
    source: &Path,
    target: &Path,
    recursive: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let flags = if recursive {
        MsFlags::MS_BIND | MsFlags::MS_REC
    } else {
        MsFlags::MS_BIND
    };
    mount(Some(source), target, None::<&str>, flags, None::<&str>).map_err(|error| {
        format!(
            "bind mount {} at {}: {error}",
            source.display(),
            target.display()
        )
    })?;
    Ok(())
}

fn mount_lower_overlay(lowers: &[&Path], target: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let lowerdirs = lowers
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join(":");
    let options = format!("userxattr,lowerdir={lowerdirs}");
    mount(
        Some("overlay"),
        target,
        Some("overlay"),
        MsFlags::MS_RDONLY,
        Some(options.as_str()),
    )
    .map_err(|error| {
        format!(
            "mount merged read-only overlay lower={lowerdirs} target={}: {error}",
            target.display()
        )
    })?;
    Ok(())
}

fn mount_overlay(
    lower: &Path,
    upper: &Path,
    work: &Path,
    target: impl AsRef<Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(upper)?;
    fs::create_dir_all(work)?;
    let options = format!(
        "userxattr,lowerdir={},upperdir={},workdir={}",
        lower.display(),
        upper.display(),
        work.display()
    );
    mount(
        Some("overlay"),
        target.as_ref(),
        Some("overlay"),
        MsFlags::empty(),
        Some(options.as_str()),
    )
    .map_err(|error| {
        format!(
            "mount overlay lower={} upper={} work={} target={}: {error}",
            lower.display(),
            upper.display(),
            work.display(),
            target.as_ref().display()
        )
    })?;
    Ok(())
}

fn exec_launcher(layout: &Layout, launcher: &str) -> Result<(), Box<dyn std::error::Error>> {
    let launcher_path = Path::new("/roms/ports").join(launcher);
    let path = "/roms/ports/PortMaster/bin:/usr/local/bin:/usr/bin:/bin";
    let home = layout.home.to_string_lossy();
    let xdg_data = layout
        .home
        .join(".local/share")
        .to_string_lossy()
        .into_owned();
    let xdg_config = layout.home.join(".config").to_string_lossy().into_owned();
    let session_bus = "unix:path=/run/user/1000/bus";

    let mut environment = env::vars_os()
        .map(|(key, value)| {
            let mut entry = key.as_bytes().to_vec();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            CString::new(entry)
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (key, value) in [
        ("PATH", path),
        ("HOME", home.as_ref()),
        ("XDG_DATA_HOME", xdg_data.as_str()),
        ("XDG_CONFIG_HOME", xdg_config.as_str()),
        ("DBUS_SESSION_BUS_ADDRESS", session_bus),
    ] {
        environment.retain(|entry| !entry.as_bytes().starts_with(format!("{key}=").as_bytes()));
        environment.push(CString::new(format!("{key}={value}"))?);
    }

    let launcher = CString::new(launcher_path.as_os_str().as_bytes())?;
    execve(&launcher, &[launcher.as_c_str()], &environment)?;
    unreachable!("execve returns only on error")
}

fn unescape_instance(encoded: &str) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("systemd-escape")
        .args(["--unescape", "--path", encoded])
        .output()?;
    if !output.status.success() {
        return Err(format!("invalid PortMaster service instance: {encoded}").into());
    }
    Ok(String::from_utf8(output.stdout)?
        .trim()
        .trim_start_matches('/')
        .to_owned())
}

fn validate_component(value: &str, description: &str) -> Result<(), Box<dyn std::error::Error>> {
    if value.is_empty()
        || value.contains("..")
        || value.contains('/')
        || value.contains('\0')
        || value.chars().any(char::is_control)
    {
        return Err(format!("invalid {description}: {value}").into());
    }
    Ok(())
}

fn find_launcher(install_dir: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut launchers = fs::read_dir(install_dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.is_file() && path.extension().is_some_and(|extension| extension == "sh"))
                .then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect::<Vec<_>>();
    launchers.sort_unstable();
    launchers
        .into_iter()
        .next()
        .ok_or_else(|| "PortMaster installation contains no launcher".into())
}

fn required_runtimes(
    install_dir: &Path,
    launcher: &str,
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let manifest = install_dir.join(".marina-runtimes");
    let mut runtimes = BTreeSet::new();
    if manifest.is_file() {
        for runtime in fs::read_to_string(manifest)?.lines() {
            insert_runtime(&mut runtimes, runtime)?;
        }
        return Ok(runtimes);
    }

    let launcher = fs::read_to_string(install_dir.join(launcher))?;
    for line in launcher.lines() {
        let line = line.trim();
        if let Some((name, value)) = line.split_once('=') {
            if name.trim_end().ends_with("_runtime") {
                insert_runtime(&mut runtimes, value.trim().trim_matches(['\'', '"']))?;
            }
        }
        for token in line.split(|character: char| {
            !character.is_ascii_alphanumeric()
                && character != '_'
                && character != '.'
                && character != '-'
        }) {
            if let Some(runtime) = token.strip_suffix(".squashfs") {
                insert_runtime(&mut runtimes, runtime)?;
            }
        }
    }
    Ok(runtimes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_are_grouped_by_port_before_data_and_home() {
        let layout = Layout::new("Moonlight New");
        let root = Path::new("/var/games/saves/ports/Moonlight New");

        assert_eq!(layout.game_upper, root.join("data"));
        assert_eq!(layout.home_upper, root.join("home"));
        assert_eq!(layout.game_work, root.join(".work/game"));
        assert_eq!(layout.home_work, root.join(".work/home"));
    }

    #[test]
    fn staging_layout_maps_paths_into_the_private_root() {
        let staging = StagingLayout::new(42);
        let base = Path::new("/tmp/.marina-portmaster-root-42");

        assert_eq!(staging.base, base);
        assert_eq!(staging.new_root, base.join("root"));
        assert_eq!(
            staging.in_new_root("/run/user/1000"),
            base.join("root/run/user/1000")
        );
    }
}

fn insert_runtime(
    runtimes: &mut BTreeSet<String>,
    runtime: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = runtime
        .trim()
        .strip_suffix(".squashfs")
        .unwrap_or(runtime.trim());
    if runtime.is_empty() {
        return Ok(());
    }
    if !runtime
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
    {
        return Err(format!("invalid PortMaster runtime: {runtime}").into());
    }
    runtimes.insert(runtime.to_owned());
    Ok(())
}

fn runtime_unit(runtime: &str) -> Result<String, Box<dyn std::error::Error>> {
    let unit = Command::new("systemd-escape")
        .args(["--template=portmaster-runtime@.service", runtime])
        .output()?;
    if !unit.status.success() {
        return Err(format!("failed to encode runtime service: {runtime}").into());
    }
    Ok(String::from_utf8(unit.stdout)?.trim().to_owned())
}

fn start_runtime(runtime: &str) -> Result<(), Box<dyn std::error::Error>> {
    let unit = runtime_unit(runtime)?;
    eprintln!("PortMaster: starting runtime unit {unit}");
    let status = Command::new("systemctl").args(["start", &unit]).status()?;
    if !status.success() {
        return Err(format!("failed to start runtime unit {unit}").into());
    }
    if !PathBuf::from(RUNTIME_DIR).join(runtime).is_dir() {
        return Err(format!("runtime mount is unavailable: {runtime}").into());
    }
    Ok(())
}

fn stop_runtime(runtime: &str) -> Result<(), Box<dyn std::error::Error>> {
    let unit = runtime_unit(runtime)?;
    eprintln!("PortMaster: stopping runtime unit {unit}");
    let status = Command::new("systemctl").args(["stop", &unit]).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("failed to stop runtime unit {unit}").into())
    }
}

fn stop_required_runtimes(encoded_port: &str) -> Result<(), Box<dyn std::error::Error>> {
    let port = unescape_instance(encoded_port)?;
    validate_component(&port, "port title")?;
    let install_dir = Path::new(PORTS_DIR).join(&port);
    if !install_dir.is_dir() {
        return Ok(());
    }
    let launcher = find_launcher(&install_dir)?;
    let mut failed = Vec::new();
    for runtime in required_runtimes(&install_dir, &launcher)? {
        if let Err(error) = stop_runtime(&runtime) {
            failed.push(error.to_string());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("runtime cleanup failed: {}", failed.join("; ")).into())
    }
}
