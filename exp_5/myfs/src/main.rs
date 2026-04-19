use myfs::{FsConfig, MemoryBlockDevice, MyFileSystem};

fn main() {
    let fs = MyFileSystem::<MemoryBlockDevice>::format_memory(FsConfig::default())
        .expect("default in-memory filesystem should format");
    let boot = fs.boot_sector();
    println!(
        "myfs core ready: {} blocks x {} bytes, root starts at cluster {}",
        boot.block_count, boot.block_size, boot.root_dir_start_cluster.0
    );
    println!("Use `cargo run -p myfs_shell` for the interactive shell.");
}
