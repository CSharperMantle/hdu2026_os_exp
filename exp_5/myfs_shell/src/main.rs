use std::io;
use std::io::Write;

use myfs::*;

fn main() {
    let mut shell = Shell::new().expect("filesystem should initialize");
    if let Err(err) = shell.run() {
        eprintln!("fatal: {err}");
        std::process::exit(1);
    }
}

struct Shell {
    fs: MyFileSystem<MemoryBlockDevice>,
    cwd_cluster: ClusterId,
    cwd_path: String,
}

impl Shell {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let fs = MyFileSystem::<MemoryBlockDevice>::format_memory(FsConfig::default())?;
        Ok(Self {
            cwd_cluster: fs.root_dir_cluster(),
            cwd_path: "/".to_string(),
            fs,
        })
    }

    fn run(&mut self) -> io::Result<()> {
        println!("myfs shell. type `help` for commands.");
        let stdin = io::stdin();
        loop {
            print!("myfs:{}> ", self.cwd_path);
            io::stdout().flush()?;

            let mut line = String::new();
            if stdin.read_line(&mut line)? == 0 {
                println!();
                break;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            match self.execute(line, &stdin) {
                Ok(ControlFlow::Continue) => {}
                Ok(ControlFlow::Exit) => break,
                Err(err) => eprintln!("error: {err}"),
            }
        }
        Ok(())
    }

    fn execute(
        &mut self,
        line: &str,
        stdin: &io::Stdin,
    ) -> Result<ControlFlow, Box<dyn std::error::Error>> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts[0] {
            "pwd" => println!("{}", self.cwd_path),
            "cd" => {
                let target = self.resolve_target(parts.get(1).ok_or("usage: cd <path>")?)?;
                match target {
                    ResolvedTarget::Root { path } => {
                        self.cwd_cluster = self.fs.root_dir_cluster();
                        self.cwd_path = path;
                    }
                    ResolvedTarget::Entry { fcb, path, .. } => {
                        if fcb.kind()? != NodeKind::Directory {
                            return Err("target is not directory".into());
                        }
                        self.cwd_cluster = fcb.start_cluster;
                        self.cwd_path = path;
                    }
                }
            }
            "ls" => {
                let target_cluster = if let Some(path) = parts.get(1) {
                    match self.resolve_target(path)? {
                        ResolvedTarget::Root { .. } => self.fs.root_dir_cluster(),
                        entry => entry.as_dir_cluster()?,
                    }
                } else {
                    self.cwd_cluster
                };
                self.print_dir(target_cluster)?;
            }
            "mkdir" => {
                let (parent_cluster, name) =
                    self.resolve_parent_and_name(parts.get(1).ok_or("usage: mkdir <path>")?)?;
                self.fs.mkdir(parent_cluster, &name)?;
            }
            "rmdir" => {
                let target = self.resolve_target(parts.get(1).ok_or("usage: rmdir <path>")?)?;
                let loc = target.loc().ok_or("cannot remove root directory")?;
                self.fs.rmdir(loc)?;
            }
            "create" => {
                let (parent_cluster, name) =
                    self.resolve_parent_and_name(parts.get(1).ok_or("usage: create <path>")?)?;
                self.fs.create_file(parent_cluster, &name)?;
            }
            "rm" => {
                let target = self.resolve_target(parts.get(1).ok_or("usage: rm <path>")?)?;
                let loc = target.loc().ok_or("cannot remove root directory")?;
                self.fs.remove_file(loc)?;
            }
            "open" => {
                let target = self.resolve_target(parts.get(1).ok_or("usage: open <path>")?)?;
                let loc = target.loc().ok_or("cannot open root directory")?;
                let handle = self.fs.open(loc)?;
                println!("handle {handle}");
            }
            "close" => {
                let arg = parts.get(1).ok_or("usage: close <handle|path>")?;
                let handle = self.resolve_handle(arg)?;
                self.fs.close(handle)?;
            }
            "read" => {
                let handle = self.parse_handle(parts.get(1), "usage: read <handle> [len]")?;
                let len = parts
                    .get(2)
                    .map(|value| value.parse::<usize>())
                    .transpose()?
                    .unwrap_or(1024);
                let bytes = self.fs.read(handle, len)?;
                println!("{}", String::from_utf8_lossy(&bytes));
                println!("[{} bytes]", bytes.len());
            }
            "write" => {
                let handle = self.parse_handle(parts.get(1), "usage: write <handle>")?;
                let data = self.read_interactive_payload(stdin)?;
                let written = self.fs.write(handle, data.as_bytes())?;
                println!("wrote {written} bytes");
            }
            "seek" => {
                let handle = self.parse_handle(parts.get(1), "usage: seek <handle> <offset>")?;
                let offset = parts
                    .get(2)
                    .ok_or("usage: seek <handle> <offset>")?
                    .parse::<usize>()?;
                self.fs.seek(handle, offset)?;
            }
            "fat" => print!("{}", self.fs.dump_fat()),
            "stat" => {
                let target = self.resolve_target(parts.get(1).ok_or("usage: stat <path>")?)?;
                match target {
                    ResolvedTarget::Root { .. } => {
                        let entry = self.fs.stat_root()?;
                        self.print_root_stat(&entry);
                    }
                    ResolvedTarget::Entry { loc, path, .. } => {
                        let entry = self.fs.stat(loc)?;
                        self.print_stat(&path, &entry);
                    }
                }
            }
            "openfiles" => self.print_open_files(),
            "help" => print!("{}", HELP),
            "exit" | "quit" => return Ok(ControlFlow::Exit),
            _ => return Err(format!("unknown command: {}", parts[0]).into()),
        }
        Ok(ControlFlow::Continue)
    }

    fn resolve_target(&self, raw: &str) -> Result<ResolvedTarget, Box<dyn std::error::Error>> {
        let canonical = self.canonicalize_path(raw)?;
        if canonical == "/" {
            return Ok(ResolvedTarget::Root { path: canonical });
        }

        let mut current = self.fs.root_dir_cluster();
        let mut last: Option<(DirEntryLoc, Fcb)> = None;
        for part in canonical.trim_start_matches('/').split('/') {
            let found = self.fs.lookup(current, part)?;
            current = found.1.start_cluster;
            last = Some(found);
        }

        let (loc, fcb) = last.expect("non-root path must resolve to entry");
        Ok(ResolvedTarget::Entry {
            loc,
            fcb,
            path: canonical,
        })
    }

    fn canonicalize_path(&self, raw: &str) -> Result<String, Box<dyn std::error::Error>> {
        if raw.is_empty() {
            return Err("empty path".into());
        }

        let joined = if raw.starts_with('/') {
            raw.to_string()
        } else if self.cwd_path == "/" {
            format!("/{raw}")
        } else {
            format!("{}/{}", self.cwd_path, raw)
        };

        let mut stack: Vec<String> = Vec::new();
        for part in joined.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                stack.pop();
                continue;
            }
            stack.push(part.to_ascii_uppercase());
        }

        if stack.is_empty() {
            Ok("/".to_string())
        } else {
            Ok(format!("/{}", stack.join("/")))
        }
    }

    fn resolve_parent_and_name(
        &self,
        raw: &str,
    ) -> Result<(ClusterId, String), Box<dyn std::error::Error>> {
        let canonical = self.canonicalize_path(raw)?;
        if canonical == "/" {
            return Err("invalid path".into());
        }
        let (parent_path, name) = canonical.rsplit_once('/').map_or(
            ("/", canonical.trim_start_matches('/')),
            |(left, right)| {
                if left.is_empty() {
                    ("/", right)
                } else {
                    (left, right)
                }
            },
        );
        if name.is_empty() {
            return Err("invalid path".into());
        }
        let parent_cluster = match self.resolve_target(parent_path)? {
            ResolvedTarget::Root { .. } => self.fs.root_dir_cluster(),
            ResolvedTarget::Entry { fcb, .. } => {
                if fcb.kind()? != NodeKind::Directory {
                    return Err("parent is not directory".into());
                }
                fcb.start_cluster
            }
        };
        Ok((parent_cluster, name.to_string()))
    }

    fn parse_handle(
        &self,
        value: Option<&&str>,
        usage: &str,
    ) -> Result<FileHandle, Box<dyn std::error::Error>> {
        Ok(value.ok_or(usage)?.parse::<u32>()?.into())
    }

    fn resolve_handle(&self, arg: &str) -> Result<FileHandle, Box<dyn std::error::Error>> {
        if let Ok(raw) = arg.parse::<u32>() {
            return Ok(raw.into());
        }
        let target = self.resolve_target(arg)?;
        let loc = target.loc().ok_or("root is not openable")?;
        self.fs
            .find_open_handle(loc)
            .ok_or_else(|| format!("file is not open: {loc}").into())
    }

    fn read_interactive_payload(
        &self,
        stdin: &io::Stdin,
    ) -> Result<String, Box<dyn std::error::Error>> {
        println!("enter text, finish with single `.` line:");
        let mut out = String::new();
        loop {
            let mut line = String::new();
            stdin.read_line(&mut line)?;
            if line.trim_end() == "." {
                break;
            }
            out.push_str(&line);
        }
        Ok(out)
    }

    fn print_dir(&self, dir_start: ClusterId) -> Result<(), Box<dyn std::error::Error>> {
        let entries = self.fs.list_dir(dir_start)?;
        if entries.is_empty() {
            println!("<empty>");
            return Ok(());
        }

        for DirEntry {
            loc,
            short_name,
            kind,
            size,
            start_cluster,
        } in entries
        {
            println!(
                "{:<4} {:>8} {:>4} {:>8} {}",
                kind, size, start_cluster, loc, short_name
            );
        }
        Ok(())
    }

    fn print_root_stat(&self, entry: &myfs::NodeMeta) {
        println!("path: /");
        println!("type: {}", entry.kind);
        println!("size: {}", entry.size);
        println!("start_cluster: {}", entry.start_cluster);
    }

    fn print_stat(&self, path: &str, entry: &myfs::NodeMeta) {
        println!("path: {}", path);
        if let Some(loc) = entry.loc {
            println!("loc: {}", loc);
        }
        println!("name: {}", entry.short_name);
        println!("type: {}", entry.kind);
        println!("size: {}", entry.size);
        println!("start_cluster: {}", entry.start_cluster);
        println!("ctime: {}", entry.ctime);
        println!("cdate: {}", entry.cdate);
    }

    fn print_open_files(&self) {
        let entries = self.fs.opened_files();
        if entries.is_empty() {
            println!("<no open files>");
            return;
        }

        for OpenFile {
            handle,
            loc,
            cursor,
            fcb,
        } in entries
        {
            println!(
                "#{:<3} loc={} cursor={:<6} size={:<8} cluster={:<4} name={}",
                handle,
                loc,
                cursor,
                fcb.size,
                fcb.start_cluster,
                fcb.short_name()
            );
        }
    }
}

enum ResolvedTarget {
    Root {
        path: String,
    },
    Entry {
        loc: DirEntryLoc,
        fcb: Fcb,
        path: String,
    },
}

impl ResolvedTarget {
    fn loc(&self) -> Option<DirEntryLoc> {
        match self {
            ResolvedTarget::Root { .. } => None,
            ResolvedTarget::Entry { loc, .. } => Some(*loc),
        }
    }

    fn as_dir_cluster(&self) -> Result<ClusterId, Box<dyn std::error::Error>> {
        match self {
            ResolvedTarget::Root { .. } => Err("target is root, handle separately".into()),
            ResolvedTarget::Entry { fcb, .. } => {
                if fcb.kind()? != NodeKind::Directory {
                    return Err("target is not directory".into());
                }
                Ok(fcb.start_cluster)
            }
        }
    }
}

enum ControlFlow {
    Continue,
    Exit,
}

const HELP: &str = "\
pwd
cd <path>
ls [path]
mkdir <path>
rmdir <path>
create <path>
rm <path>
open <path>
close <handle|path>
read <handle> [len]
write <handle>
seek <handle> <offset>
fat
stat <path>
openfiles
help
exit
";
