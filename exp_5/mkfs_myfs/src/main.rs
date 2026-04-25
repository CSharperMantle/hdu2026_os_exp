use myfs::FileBlockDevice;
use myfs::FsConfig;
use myfs::LogicalBlockDevice;
use myfs::MyFileSystem;
use std::env;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process;

fn usage() -> String {
    "usage: mkfs_myfs <image> [--block-size N] [--block-count N] [--blocks-per-cluster N]"
        .to_string()
}

fn parse_u16(flag: &str, value: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .map_err(|err| format!("invalid value for {flag}: {value} ({err})"))
}

fn parse_args() -> Result<(PathBuf, FsConfig), String> {
    let mut args = env::args_os();
    let _ = args.next();

    let mut image_path = None;
    let mut config = FsConfig::default();

    while let Some(arg) = args.next() {
        if arg == "--help" {
            return Err(usage());
        }
        if image_path.is_none() && !arg.to_string_lossy().starts_with('-') {
            image_path = Some(PathBuf::from(arg));
            continue;
        }

        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {}", arg.to_string_lossy()))?;
        let value = value
            .to_str()
            .ok_or_else(|| format!("non-utf8 value for {}", arg.to_string_lossy()))?;

        if arg == "--block-size" {
            config.block_size = parse_u16("--block-size", value)?;
        } else if arg == "--block-count" {
            config.block_count = parse_u16("--block-count", value)?;
        } else if arg == "--blocks-per-cluster" {
            config.blocks_per_cluster = parse_u16("--blocks-per-cluster", value)?;
        } else {
            return Err(format!("unknown option: {}", arg.to_string_lossy()));
        }
    }

    let image_path = image_path.ok_or_else(usage)?;
    Ok((image_path, config))
}

fn run() -> Result<(), String> {
    let (image_path, config) = parse_args()?;
    config
        .validate()
        .map_err(|err| format!("invalid filesystem config: {err}"))?;

    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&image_path)
        .map_err(|err| format!("failed to open image {}: {err}", image_path.display()))?;
    let device = FileBlockDevice::create(
        file,
        usize::from(config.block_size),
        usize::from(config.block_count),
    )
    .map_err(|err| format!("failed to create file-backed device: {err}"))?;
    let device = LogicalBlockDevice::new(device, usize::from(config.block_size))
        .map_err(|err| format!("failed to create logical block adapter: {err}"))?;

    let block_size = config.block_size;
    let block_count = config.block_count;
    let blocks_per_cluster = config.blocks_per_cluster;
    let _ = MyFileSystem::format_on_device(device, config)
        .map_err(|err| format!("failed to format image {}: {err}", image_path.display()))?;

    println!(
        "formatted myfs image. path={}, block_size={}, block_count={}, blocks_per_cluster={}",
        image_path.display(),
        block_size,
        block_count,
        blocks_per_cluster
    );
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        process::exit(1);
    }
}
