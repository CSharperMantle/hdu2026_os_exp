use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use myfs::FileBlockDevice;
use myfs::FsConfig;
use myfs::LogicalBlockDevice;
use myfs::MyFileSystem;
use std::fs::OpenOptions;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version)]
struct Args {
    #[arg(value_name = "IMAGE", help = "Path to the new image")]
    image: PathBuf,
    #[arg(short = 's', long, help = "Set the block size of the image")]
    block_size: Option<u16>,
    #[arg(short = 'n', long, help = "Set number of blocks in the image")]
    block_count: Option<u16>,
    #[arg(short = 'c', long, help = "Set number of blocks in a cluster")]
    blocks_per_cluster: Option<u16>,
}

fn parse_args() -> (PathBuf, FsConfig) {
    let args = Args::parse();
    let mut config = FsConfig::default();
    if let Some(value) = args.block_size {
        config.block_size = value;
    }
    if let Some(value) = args.block_count {
        config.block_count = value;
    }
    if let Some(value) = args.blocks_per_cluster {
        config.blocks_per_cluster = value;
    }
    (args.image, config)
}

fn main() -> Result<()> {
    env_logger::init();

    let (image_path, config) = parse_args();
    config
        .validate()
        .with_context(|| "invalid config combination")?;

    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&image_path)
        .with_context(|| format!("failed to open image {}", image_path.display()))?;
    let device = FileBlockDevice::create(
        file,
        usize::from(config.block_size),
        usize::from(config.block_count),
    )
    .with_context(|| "failed to create file-backed device")?;
    let device = LogicalBlockDevice::new(device, usize::from(config.block_size))
        .with_context(|| "failed to create logical block adapter")?;

    let block_size = config.block_size;
    let block_count = config.block_count;
    let blocks_per_cluster = config.blocks_per_cluster;
    let _ = MyFileSystem::format_on_device(device, config)
        .with_context(|| format!("failed to format image {}", image_path.display()))?;

    println!(
        "Formatted myfs image at {}\n\tblock_size={}\n\tblock_count={}\n\tblocks_per_cluster={}",
        image_path.display(),
        block_size,
        block_count,
        blocks_per_cluster
    );
    Ok(())
}
