use std::{env, path::Path, process::Command};

const ALLOWED_IMAGE_ROOTS: &[&str] = &[
    "/var/games/ports/PortMaster/libs",
    "/roms/ports/PortMaster/libs",
    "/var/lib/portmaster/runtimes",
];

fn usage() -> ! {
    eprintln!("usage: marina-portmaster-runtime mount-image <runtime-image> <mountpoint>");
    std::process::exit(2);
}

fn allowed_image(path: &Path) -> bool {
    ALLOWED_IMAGE_ROOTS
        .iter()
        .any(|root| path.starts_with(root))
        && path.extension().and_then(|ext| ext.to_str()) == Some("squashfs")
}

fn allowed_mountpoint(path: &Path) -> bool {
    path.starts_with("/tmp")
        || path.starts_with("/run/portmaster/runtimes")
        || path.starts_with("/run/user/1000/portmaster")
}

fn main() {
    marina_logging::init();

    let mut args = env::args().skip(1);
    if args.next().as_deref() != Some("mount-image") {
        usage();
    }
    let Some(image) = args.next().map(std::path::PathBuf::from) else {
        usage()
    };
    let Some(mountpoint) = args.next().map(std::path::PathBuf::from) else {
        usage()
    };
    if args.next().is_some() || !allowed_image(&image) || !allowed_mountpoint(&mountpoint) {
        eprintln!("runtime mount path is not allowed");
        std::process::exit(1);
    }
    if !image.is_file() {
        eprintln!("runtime image does not exist: {}", image.display());
        std::process::exit(1);
    }
    if let Err(error) = std::fs::create_dir_all(&mountpoint) {
        eprintln!("failed to create mountpoint: {error}");
        std::process::exit(1);
    }
    if Command::new("mountpoint")
        .arg("-q")
        .arg(&mountpoint)
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }
    let status = Command::new("/usr/bin/mount")
        .args(["-t", "squashfs", "-o", "ro,loop"])
        .arg(&image)
        .arg(&mountpoint)
        .status()
        .expect("failed to invoke mount");
    if !status.success() {
        eprintln!(
            "failed to mount {} at {}",
            image.display(),
            mountpoint.display()
        );
        std::process::exit(status.code().unwrap_or(1));
    }
}
