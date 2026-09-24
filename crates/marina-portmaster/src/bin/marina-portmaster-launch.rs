use std::{
    collections::BTreeSet,
    env,
    ffi::CString,
    fs,
    io::{Read, Write},
    os::{unix::ffi::OsStrExt, unix::net::UnixStream},
    path::{Path, PathBuf},
    process::Command,
};

use nix::{
    mount::{MsFlags, mount},
    sched::{CloneFlags, unshare},
    sys::wait::{WaitStatus, waitpid},
    unistd::{ForkResult, chdir, execve, fork, getgid, getuid},
};

const PORTS_DIR: &str = "/var/games/ports";
const SAVES_DIR: &str = "/var/games/saves/ports";
const PORTMASTER_DIR: &str = "/usr/libexec/marina-portmaster";
const RUNTIME_DIR: &str = "/run/portmaster/runtimes";

fn main() {
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
    let encoded_port = env::args().nth(1).ok_or("missing port instance")?;
    let port = unescape_instance(&encoded_port)?;
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
    for runtime in required_runtimes(&install_dir, &launcher)? {
        start_runtime(&runtime)?;
    }

    let layout = Layout::new(&port);
    layout.create_directories()?;
    let uid = getuid();
    let gid = getgid();
    let (mut parent_socket, mut child_socket) = UnixStream::pair()?;

    // All namespace changes happen after fork. The parent writes the child's
    // UID/GID maps after `unshare`, matching util-linux `unshare --map-root-user`
    // and preserving the supplemental credentials needed for controller nodes.
    match unsafe { fork()? } {
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
            if let Err(error) = launch_in_namespace(&layout, &launcher, &mut child_socket) {
                eprintln!("PortMaster sandbox setup failed: {error}");
                std::process::exit(1);
            }
            unreachable!("execve replaces the child process")
        }
    }
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
    home: PathBuf,
}

impl Layout {
    fn new(port: &str) -> Self {
        let saves = Path::new(SAVES_DIR);
        Self {
            install_dir: Path::new(PORTS_DIR).join(port),
            game_upper: saves.join("data").join(port),
            game_work: saves.join(".work").join(port),
            home_upper: saves.join("home").join(port),
            home_work: saves.join(".work").join(format!("home-{port}")),
            empty_lower: saves.join(".work/empty"),
            home: Path::new("/run/user/1000/portmaster/home").join(port),
        }
    }

    fn create_directories(&self) -> std::io::Result<()> {
        for directory in [
            &self.game_upper,
            &self.game_work,
            &self.home_upper,
            &self.home_work,
            &self.empty_lower,
        ] {
            fs::create_dir_all(directory)?;
        }
        Ok(())
    }
}

fn launch_in_namespace(
    layout: &Layout,
    launcher: &str,
    socket: &mut UnixStream,
) -> Result<(), Box<dyn std::error::Error>> {
    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS)?;
    socket.write_all(b"R")?;
    let mut mapped = [0_u8; 1];
    socket.read_exact(&mut mapped)?;
    if mapped != [b'M'] {
        return Err("parent could not configure PortMaster user namespace".into());
    }

    // Do not let mounts created by this port propagate back into the outer
    // systemd namespace.
    mount(
        None::<&str>,
        Path::new("/"),
        None::<&str>,
        MsFlags::MS_REC | MsFlags::MS_PRIVATE,
        None::<&str>,
    )?;

    fs::create_dir_all("/roms/ports")?;
    mount_overlay(
        &layout.install_dir,
        &layout.game_upper,
        &layout.game_work,
        "/roms/ports",
    )?;
    fs::create_dir_all("/roms/ports/PortMaster")?;
    mount(
        Some(Path::new(PORTMASTER_DIR)),
        Path::new("/roms/ports/PortMaster"),
        None::<&str>,
        MsFlags::MS_BIND,
        None::<&str>,
    )?;

    fs::create_dir_all(layout.home.join(".local/share"))?;
    mount_overlay(
        &layout.empty_lower,
        &layout.home_upper,
        &layout.home_work,
        &layout.home,
    )?;

    chdir("/roms/ports")?;
    exec_launcher(layout, launcher)
}

fn mount_overlay(
    lower: &Path,
    upper: &Path,
    work: &Path,
    target: impl AsRef<Path>,
) -> Result<(), Box<dyn std::error::Error>> {
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
    )?;
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

fn start_runtime(runtime: &str) -> Result<(), Box<dyn std::error::Error>> {
    let unit = Command::new("systemd-escape")
        .args(["--template=portmaster-runtime@.service", runtime])
        .output()?;
    if !unit.status.success() {
        return Err(format!("failed to encode runtime service: {runtime}").into());
    }
    let unit = String::from_utf8(unit.stdout)?.trim().to_owned();
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
