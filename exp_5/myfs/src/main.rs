use chrono::DateTime;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::NaiveTime;
use chrono::Utc;
use fuser::*;
use log::debug;
use log::warn;
use myfs::*;
use std::convert::TryFrom;
use std::env;
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io;
use std::io::Read;
use std::path::PathBuf;
use std::process;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const TTL_ZERO: Duration = Duration::ZERO;
const GENERATION_ZERO: Generation = Generation(0);

macro_rules! unwrap_or_reply {
    ($reply:ident, $expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => {
                $reply.error(err);
                return;
            }
        }
    };
}

macro_rules! unwrap_or_reply_fs_error {
    ($reply:ident, $expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => {
                warn!("{}", err);
                $reply.error(fuser::Errno::from(FuseErrno::from(err)));
                return;
            }
        }
    };
}

macro_rules! ok_or_reply {
    ($reply:ident, $expr:expr) => {
        if let Err(err) = $expr {
            $reply.error(err);
            return;
        }
    };
}

macro_rules! ok_or_reply_fs_error {
    ($reply:ident, $expr:expr) => {
        if let Err(err) = $expr {
            warn!("{}", err);
            $reply.error(fuser::Errno::from(FuseErrno::from(err)));
            return;
        }
    };
}

macro_rules! any_some {
    ($($opt:expr),+ $(,)?) => {
        $( $opt.is_some() )||+
    };
}

/// Local clone of [`NodeId`] for implementing conversions from [`INodeNo`].
#[repr(transparent)]
struct FuseNodeId(NodeId);

impl From<NodeId> for FuseNodeId {
    fn from(value: NodeId) -> Self {
        Self(value)
    }
}

impl From<FuseNodeId> for INodeNo {
    fn from(value: FuseNodeId) -> Self {
        match value.0 {
            val if val == NodeId::ROOT => fuser::INodeNo::ROOT,
            val => Self(u64::from(val)),
        }
    }
}

impl From<INodeNo> for FuseNodeId {
    fn from(value: INodeNo) -> Self {
        match value {
            val if val == fuser::INodeNo::ROOT => Self(NodeId::ROOT),
            val => Self(NodeId::from(val.0)),
        }
    }
}

/// Local clone of [`DirEntryLoc`] for implementing conversions from [`FuseNodeId`].
#[repr(transparent)]
struct FuseDirEntryLoc(DirEntryLoc);

impl TryFrom<FuseNodeId> for FuseDirEntryLoc {
    type Error = fuser::Errno;

    fn try_from(value: FuseNodeId) -> Result<Self, Self::Error> {
        DirEntryLoc::try_from(value.0)
            .map(Self)
            .map_err(|_| fuser::Errno::EISDIR)
    }
}

/// Local clone of [`FileType`] for implementing conversions from [`NodeKind`].
#[repr(transparent)]
struct FuseFileType(FileType);

impl From<NodeKind> for FuseFileType {
    fn from(value: NodeKind) -> Self {
        match value {
            NodeKind::File => Self(FileType::RegularFile),
            NodeKind::Directory => Self(FileType::Directory),
        }
    }
}

impl From<FuseFileType> for FileType {
    fn from(value: FuseFileType) -> Self {
        value.0
    }
}

/// Local clone of [`fuser::Errno`] for implementing conversions from [`FsError`].
#[repr(transparent)]
struct FuseErrno(fuser::Errno);

impl From<FsError> for FuseErrno {
    fn from(value: FsError) -> Self {
        match value {
            FsError::InvalidConfig(_)
            | FsError::InvalidName(_)
            | FsError::InvalidPath(_)
            | FsError::SeekOutOfBounds(_) => Self(fuser::Errno::EINVAL),
            FsError::NotFound(_) | FsError::NotFoundAt(_) => Self(fuser::Errno::ENOENT),
            FsError::NotADirectory(_) => Self(fuser::Errno::ENOTDIR),
            FsError::IsADirectory(_) => Self(fuser::Errno::EISDIR),
            FsError::DirectoryNotEmpty(_) => Self(fuser::Errno::ENOTEMPTY),
            FsError::NoSpace => Self(fuser::Errno::ENOSPC),
            FsError::TooManyOpenFiles => Self(fuser::Errno::EMFILE),
            FsError::AlreadyOpen(_) | FsError::FileOpen(_) => Self(fuser::Errno::EBUSY),
            FsError::InvalidHandle(_) => Self(fuser::Errno::EBADF),
            FsError::CorruptFs(_) => Self(fuser::Errno::EIO),
        }
    }
}

impl From<FuseErrno> for fuser::Errno {
    fn from(value: FuseErrno) -> Self {
        value.0
    }
}

#[repr(transparent)]
struct FuseSystemTime(SystemTime);

impl TryFrom<&myfs::NodeMeta> for FuseSystemTime {
    type Error = FsError;

    fn try_from(value: &myfs::NodeMeta) -> Result<Self, Self::Error> {
        if value.mdate == U16Date::EMPTY || value.mtime == U16Time::EMPTY {
            return Ok(Self(UNIX_EPOCH));
        }
        let mdate = NaiveDate::try_from(value.mdate)?;
        let mtime = NaiveTime::try_from(value.mtime)?;
        let mdatetime = NaiveDateTime::new(mdate, mtime);
        let mdatetime = DateTime::<Utc>::from_naive_utc_and_offset(mdatetime, Utc);
        Ok(Self(
            UNIX_EPOCH + Duration::from_secs(mdatetime.timestamp() as u64),
        ))
    }
}

impl From<FuseSystemTime> for SystemTime {
    fn from(value: FuseSystemTime) -> Self {
        value.0
    }
}

/// Local clone of [`DateTime<Utc>`] for implementing conversions from [`TimeOrNow`].
#[repr(transparent)]
struct FuseDateTimeUtc(DateTime<Utc>);

impl From<TimeOrNow> for FuseDateTimeUtc {
    fn from(value: TimeOrNow) -> Self {
        match value {
            TimeOrNow::Now => Self(Utc::now()),
            TimeOrNow::SpecificTime(value) => Self(DateTime::<Utc>::from(value)),
        }
    }
}

impl From<FuseDateTimeUtc> for DateTime<Utc> {
    fn from(value: FuseDateTimeUtc) -> Self {
        value.0
    }
}

/// Local clone of [`FileAttr`] for implementing conversions from [`FuseFileAttr`].
#[repr(transparent)]
struct FuseFileAttr(FileAttr);

impl<D: BufferedBlockDevice + Send> TryFrom<(&Request, &FuseMyFileSystem<D>, &NodeMeta)>
    for FuseFileAttr
{
    type Error = FsError;

    fn try_from(
        (req, owner, meta): (&Request, &FuseMyFileSystem<D>, &NodeMeta),
    ) -> Result<Self, Self::Error> {
        let mtime = SystemTime::from(FuseSystemTime::try_from(meta)?);
        Ok(Self(FileAttr {
            ino: INodeNo::from(FuseNodeId::from(meta.node_id)),
            size: u64::from(meta.size),
            blocks: u64::from(meta.size).div_ceil(512),
            atime: mtime,
            mtime,
            ctime: mtime,
            crtime: SystemTime::UNIX_EPOCH,
            kind: FileType::from(FuseFileType::from(meta.kind)),
            perm: 0o755,
            nlink: match meta.kind {
                NodeKind::File => 1,
                NodeKind::Directory => 2,
            },
            uid: req.uid(),
            gid: req.gid(),
            rdev: 0,
            blksize: u32::from(owner.block_size),
            flags: 0,
        }))
    }
}

impl From<FuseFileAttr> for FileAttr {
    fn from(value: FuseFileAttr) -> Self {
        value.0
    }
}

struct FuseMyFileSystem<D: BufferedBlockDevice + Send> {
    fs: Mutex<MyFileSystem<D>>,
    block_size: u16,
}

impl<D: BufferedBlockDevice + Send> FuseMyFileSystem<D> {
    fn new(fs: MyFileSystem<D>) -> Self {
        let block_size = fs.boot_sector().block_size;
        Self {
            fs: Mutex::new(fs),
            block_size,
        }
    }

    fn lock_fs(&self) -> Result<MutexGuard<'_, MyFileSystem<D>>, fuser::Errno> {
        self.fs.lock().map_err(|_| fuser::Errno::EIO)
    }

    fn dir_cluster(fs: &MyFileSystem<D>, node: NodeId) -> Result<ClusterId, FsError> {
        let meta = fs.stat_node(node)?;
        if meta.kind != NodeKind::Directory {
            return Err(FsError::NotADirectory(meta.short_name));
        }
        Ok(meta.start_cluster)
    }

    fn find_parent_under(
        fs: &MyFileSystem<D>,
        current: NodeId,
        target: NodeId,
    ) -> Result<Option<NodeId>, FsError> {
        for entry in fs.dir_entries_node(current)? {
            let entry = entry?;
            if entry.node_id == target {
                return Ok(Some(current));
            }
            if entry.kind == NodeKind::Directory
                && let Some(found) = Self::find_parent_under(fs, entry.node_id, target)?
            {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    fn parent_of(fs: &MyFileSystem<D>, node: NodeId) -> Result<NodeId, FsError> {
        if node == NodeId::ROOT {
            return Ok(NodeId::ROOT);
        }
        Self::find_parent_under(fs, NodeId::ROOT, node)?
            .ok_or_else(|| FsError::CorruptFs(format!("cannot find parent directory for {}", node)))
    }

    fn name_str(name: &OsStr) -> Result<&str, fuser::Errno> {
        name.to_str().ok_or(fuser::Errno::EINVAL)
    }

    fn lookup_meta(
        fs: &MyFileSystem<D>,
        parent: NodeId,
        name: &str,
    ) -> Result<myfs::NodeMeta, FsError> {
        if name == "." {
            return fs.stat_node(parent);
        }
        if name == ".." {
            let parent = Self::parent_of(fs, parent)?;
            return fs.stat_node(parent);
        }
        let parent_cluster = Self::dir_cluster(fs, parent)?;
        let (loc, _) = fs.lookup(parent_cluster, name)?;
        fs.stat(loc)
    }

    fn unsupported_if_large_offset(offset: u64) -> Result<usize, fuser::Errno> {
        usize::try_from(offset).map_err(|_| fuser::Errno::EOPNOTSUPP)
    }

    fn unsupported_special_name(name: &str) -> Result<(), fuser::Errno> {
        if name == "." || name == ".." {
            return Err(fuser::Errno::EOPNOTSUPP);
        }
        Ok(())
    }

    fn unsupported_non_regular(mode: u32, expected_dir: bool) -> Result<(), fuser::Errno> {
        let kind = mode & libc::S_IFMT;
        if expected_dir {
            if kind != 0 && kind != libc::S_IFDIR {
                return Err(fuser::Errno::EOPNOTSUPP);
            }
        } else if kind != 0 && kind != libc::S_IFREG {
            return Err(fuser::Errno::EOPNOTSUPP);
        }
        Ok(())
    }
}

impl<D: BufferedBlockDevice + Send + 'static> Filesystem for FuseMyFileSystem<D> {
    fn init(&mut self, _: &Request, _: &mut KernelConfig) -> io::Result<()> {
        Ok(())
    }

    fn destroy(&mut self) {
        if let Ok(mut fs) = self.fs.lock() {
            let _ = fs.sync();
        }
    }

    fn lookup(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        debug!("lookup(parent={}, name={:?})", parent.0, name);
        let parent = FuseNodeId::from(parent).0;
        let name = unwrap_or_reply!(reply, Self::name_str(name));
        let fs = unwrap_or_reply!(reply, self.lock_fs());
        let meta = unwrap_or_reply_fs_error!(reply, Self::lookup_meta(&fs, parent, name));
        let attr = FileAttr::from(unwrap_or_reply_fs_error!(
            reply,
            FuseFileAttr::try_from((req, self, &meta))
        ));
        reply.entry(&TTL_ZERO, &attr, GENERATION_ZERO);
    }

    fn getattr(&self, req: &Request, ino: INodeNo, _: Option<fuser::FileHandle>, reply: ReplyAttr) {
        debug!("getattr(ino={})", ino.0);
        let node = FuseNodeId::from(ino).0;
        let fs = unwrap_or_reply!(reply, self.lock_fs());
        let meta = unwrap_or_reply_fs_error!(reply, fs.stat_node(node));
        let attr = FileAttr::from(unwrap_or_reply_fs_error!(
            reply,
            FuseFileAttr::try_from((req, self, &meta))
        ));
        reply.attr(&TTL_ZERO, &attr);
    }

    fn setattr(
        &self,
        req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        fh: Option<fuser::FileHandle>,
        crtime: Option<SystemTime>,
        chgtime: Option<SystemTime>,
        bkuptime: Option<SystemTime>,
        flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        debug!(
            "setattr(ino={}), mode={:?}, uid={:?}, gid={:?}, size={:?}, atime={:?}, mtime={:?}, ctime={:?}, fh={:?}, crtime={:?}, chgtime={:?}, bkuptime={:?}, flags={:?}",
            ino.0, mode, uid, gid, size, atime, mtime, ctime, crtime, fh, chgtime, bkuptime, flags
        );
        if any_some!(mode, uid, gid, size, fh, crtime, chgtime, bkuptime, flags) {
            reply.error(fuser::Errno::EOPNOTSUPP);
            return;
        }

        let mtime = mtime
            .or(ctime.map(TimeOrNow::SpecificTime))
            .or(atime)
            .expect("one of mtime, ctime, or atime must be set");
        let node = FuseNodeId::from(ino).0;
        if node == NodeId::ROOT {
            reply.error(fuser::Errno::EOPNOTSUPP);
            return;
        }
        let loc = unwrap_or_reply!(reply, FuseDirEntryLoc::try_from(FuseNodeId::from(node))).0;
        let mut fs = unwrap_or_reply!(reply, self.lock_fs());
        ok_or_reply_fs_error!(
            reply,
            fs.set_mtime(loc, DateTime::<Utc>::from(FuseDateTimeUtc::from(mtime)))
        );
        let meta = unwrap_or_reply_fs_error!(reply, fs.stat(loc));
        let attr = FileAttr::from(unwrap_or_reply_fs_error!(
            reply,
            FuseFileAttr::try_from((req, self, &meta))
        ));
        reply.attr(&TTL_ZERO, &attr);
    }

    fn mknod(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _: u32,
        rdev: u32,
        reply: ReplyEntry,
    ) {
        debug!(
            "mknod(parent={}, name={:?}, mode={:#o}, rdev={})",
            parent.0, name, mode, rdev
        );
        if rdev != 0 {
            reply.error(fuser::Errno::EOPNOTSUPP);
            return;
        }
        ok_or_reply!(reply, Self::unsupported_non_regular(mode, false));
        let parent = FuseNodeId::from(parent).0;
        let name = unwrap_or_reply!(reply, Self::name_str(name));
        ok_or_reply!(reply, Self::unsupported_special_name(name));
        let mut fs = unwrap_or_reply!(reply, self.lock_fs());
        let parent_cluster = unwrap_or_reply_fs_error!(reply, Self::dir_cluster(&fs, parent));
        let loc = unwrap_or_reply_fs_error!(reply, fs.create_file(parent_cluster, name));
        let meta = unwrap_or_reply_fs_error!(reply, fs.stat(loc));
        let attr = FileAttr::from(unwrap_or_reply_fs_error!(
            reply,
            FuseFileAttr::try_from((req, self, &meta))
        ));
        reply.entry(&TTL_ZERO, &attr, GENERATION_ZERO);
    }

    fn mkdir(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _: u32,
        reply: ReplyEntry,
    ) {
        debug!(
            "mkdir(parent={}, name={:?}, mode={:#o})",
            parent.0, name, mode
        );
        ok_or_reply!(reply, Self::unsupported_non_regular(mode, true));
        let parent = FuseNodeId::from(parent).0;
        let name = unwrap_or_reply!(reply, Self::name_str(name));
        ok_or_reply!(reply, Self::unsupported_special_name(name));
        let mut fs = unwrap_or_reply!(reply, self.lock_fs());
        let parent_cluster = unwrap_or_reply_fs_error!(reply, Self::dir_cluster(&fs, parent));
        let loc = unwrap_or_reply_fs_error!(reply, fs.mkdir(parent_cluster, name));
        let meta = unwrap_or_reply_fs_error!(reply, fs.stat(loc));
        let attr = FileAttr::from(unwrap_or_reply_fs_error!(
            reply,
            FuseFileAttr::try_from((req, self, &meta))
        ));
        reply.entry(&TTL_ZERO, &attr, GENERATION_ZERO);
    }

    fn unlink(&self, _: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        debug!("unlink(parent={}, name={:?})", parent.0, name);
        let parent = FuseNodeId::from(parent).0;
        let name = unwrap_or_reply!(reply, Self::name_str(name));
        ok_or_reply!(reply, Self::unsupported_special_name(name));
        let mut fs = unwrap_or_reply!(reply, self.lock_fs());
        let parent_cluster = unwrap_or_reply_fs_error!(reply, Self::dir_cluster(&fs, parent));
        let loc = unwrap_or_reply_fs_error!(reply, fs.lookup(parent_cluster, name)).0;
        ok_or_reply_fs_error!(reply, fs.remove_file(loc));
        reply.ok();
    }

    fn rmdir(&self, _: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        debug!("rmdir(parent={}, name={:?})", parent.0, name);
        let parent = FuseNodeId::from(parent).0;
        let name = unwrap_or_reply!(reply, Self::name_str(name));
        ok_or_reply!(reply, Self::unsupported_special_name(name));
        let mut fs = unwrap_or_reply!(reply, self.lock_fs());
        let parent_cluster = unwrap_or_reply_fs_error!(reply, Self::dir_cluster(&fs, parent));
        let loc = unwrap_or_reply_fs_error!(reply, fs.lookup(parent_cluster, name)).0;
        ok_or_reply_fs_error!(reply, fs.rmdir(loc));
        reply.ok();
    }

    fn open(&self, _: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        debug!("open(ino={}, flags={:#x})", ino.0, flags.0);
        let node = FuseNodeId::from(ino).0;
        let fs = unwrap_or_reply!(reply, self.lock_fs());
        let meta = unwrap_or_reply_fs_error!(reply, fs.stat_node(node));
        if meta.kind == NodeKind::Directory {
            reply.error(fuser::Errno::EISDIR);
        } else {
            reply.opened(fuser::FileHandle(ino.0), FopenFlags::empty());
        }
    }

    fn read(
        &self,
        _: &Request,
        ino: INodeNo,
        _: fuser::FileHandle,
        offset: u64,
        size: u32,
        _: OpenFlags,
        _: Option<LockOwner>,
        reply: ReplyData,
    ) {
        debug!("read(ino={}, offset={}, size={})", ino.0, offset, size);
        let offset = unwrap_or_reply!(reply, Self::unsupported_if_large_offset(offset));
        let node = FuseNodeId::from(ino);
        let loc = unwrap_or_reply!(reply, FuseDirEntryLoc::try_from(node)).0;
        let fs = unwrap_or_reply!(reply, self.lock_fs());
        let data = unwrap_or_reply_fs_error!(reply, fs.read_file_at(loc, offset, size as usize));
        reply.data(&data);
    }

    fn write(
        &self,
        _: &Request,
        ino: INodeNo,
        _: fuser::FileHandle,
        offset: u64,
        data: &[u8],
        _: WriteFlags,
        _: OpenFlags,
        _: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        debug!(
            "write(ino={}, offset={}, bytes={})",
            ino.0,
            offset,
            data.len()
        );
        let offset = unwrap_or_reply!(reply, Self::unsupported_if_large_offset(offset));
        let node = FuseNodeId::from(ino);
        let loc = unwrap_or_reply!(reply, FuseDirEntryLoc::try_from(node)).0;
        let mut fs = unwrap_or_reply!(reply, self.lock_fs());
        let written = unwrap_or_reply_fs_error!(reply, fs.write_file_at(loc, offset, data)) as u32;
        reply.written(written);
    }

    fn flush(
        &self,
        _: &Request,
        ino: INodeNo,
        _: fuser::FileHandle,
        _: LockOwner,
        reply: ReplyEmpty,
    ) {
        debug!("flush(ino={})", ino.0);
        reply.ok();
    }

    fn fsync(&self, _: &Request, ino: INodeNo, _: fuser::FileHandle, _: bool, reply: ReplyEmpty) {
        debug!("fsync(ino={})", ino.0);
        let mut fs = unwrap_or_reply!(reply, self.lock_fs());
        ok_or_reply_fs_error!(reply, fs.sync());
        reply.ok();
    }

    fn readdir(
        &self,
        _: &Request,
        ino: INodeNo,
        _: fuser::FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir(ino={}, offset={})", ino.0, offset);
        let node = FuseNodeId::from(ino).0;
        let fs = unwrap_or_reply!(reply, self.lock_fs());
        let meta = unwrap_or_reply_fs_error!(reply, fs.stat_node(node));
        if meta.kind != NodeKind::Directory {
            reply.error(fuser::Errno::ENOTDIR);
            return;
        }
        let parent = unwrap_or_reply_fs_error!(reply, Self::parent_of(&fs, node));
        if offset < 1
            && reply.add(
                INodeNo::from(FuseNodeId::from(node)),
                1,
                FileType::Directory,
                ".",
            )
        {
            reply.ok();
            return;
        }
        if offset < 2
            && reply.add(
                INodeNo::from(FuseNodeId::from(parent)),
                2,
                FileType::Directory,
                "..",
            )
        {
            reply.ok();
            return;
        }
        let skip = offset.saturating_sub(2) as usize;
        let entries = unwrap_or_reply_fs_error!(reply, fs.dir_entries_node(node));
        for (idx, entry) in entries.skip(skip).enumerate() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    reply.error(fuser::Errno::from(FuseErrno::from(err)));
                    return;
                }
            };
            let next_offset = skip as u64 + idx as u64 + 3;
            if reply.add(
                INodeNo::from(FuseNodeId::from(entry.node_id)),
                next_offset,
                FileType::from(FuseFileType::from(entry.kind)),
                entry.short_name,
            ) {
                break;
            }
        }
        reply.ok();
    }

    fn create(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _: u32,
        _: i32,
        reply: ReplyCreate,
    ) {
        debug!(
            "create(parent={}, name={:?}, mode={:#o})",
            parent.0, name, mode
        );
        ok_or_reply!(reply, Self::unsupported_non_regular(mode, false));
        let parent = FuseNodeId::from(parent).0;
        let name = unwrap_or_reply!(reply, Self::name_str(name));
        ok_or_reply!(reply, Self::unsupported_special_name(name));
        let mut fs = unwrap_or_reply!(reply, self.lock_fs());
        let parent_cluster = unwrap_or_reply_fs_error!(reply, Self::dir_cluster(&fs, parent));
        let loc = unwrap_or_reply_fs_error!(reply, fs.create_file(parent_cluster, name));
        let meta = unwrap_or_reply_fs_error!(reply, fs.stat(loc));
        let attr = FileAttr::from(unwrap_or_reply_fs_error!(
            reply,
            FuseFileAttr::try_from((req, self, &meta))
        ));
        reply.created(
            &TTL_ZERO,
            &attr,
            GENERATION_ZERO,
            fuser::FileHandle(attr.ino.0),
            FopenFlags::empty(),
        );
    }
}

fn parse_args() -> Result<(Option<PathBuf>, PathBuf), String> {
    let mut args = env::args_os();
    let _ = args.next();
    let mut image_path = None;
    let mut mountpoint = None;
    let mut force_memory = false;

    for arg in args {
        if arg == "--help" {
            return Err("usage: myfs [--memory|-m] <image|-> <mountpoint>".to_string());
        }
        if arg == "--memory" || arg == "-m" {
            force_memory = true;
            continue;
        }
        if arg.to_string_lossy().starts_with('-') {
            return Err(format!("unknown option: {}", arg.to_string_lossy()));
        }
        if image_path.is_none() {
            image_path = Some(PathBuf::from(arg));
            continue;
        }
        if mountpoint.is_none() {
            mountpoint = Some(PathBuf::from(arg));
            continue;
        }
        return Err("too many positional arguments".to_string());
    }

    let image_path =
        image_path.ok_or_else(|| "usage: myfs [--memory|-m] <image|-> <mountpoint>".to_string())?;
    let mountpoint =
        mountpoint.ok_or_else(|| "usage: myfs [--memory|-m] <image|-> <mountpoint>".to_string())?;
    let image_path = if force_memory || image_path.as_os_str() == "-" {
        None
    } else {
        Some(image_path)
    };
    Ok((image_path, mountpoint))
}

fn open_memory_fs() -> Result<MyFileSystem<MemoryBlockDevice>, String> {
    MyFileSystem::format_memory(FsConfig::default())
        .map_err(|err| format!("failed to format in-memory filesystem: {err}"))
}

fn open_image_fs(
    path: &PathBuf,
) -> Result<MyFileSystem<LogicalBlockDevice<FileBlockDevice>>, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|err| format!("failed to open image read-write: {err}"))?;
    let mut boot_bytes = vec![0; BOOT_SECTOR_SIZE];
    file.read_exact(&mut boot_bytes)
        .map_err(|err| format!("failed to read boot sector: {err}"))?;
    let boot = BootSector::read_from_prefix(&boot_bytes)
        .map_err(|err| format!("failed to parse boot sector: {err}"))?;
    let device = FileBlockDevice::from_file(file, usize::from(boot.block_size))
        .map_err(|err| format!("failed to build file-backed device: {err}"))?;
    let device = LogicalBlockDevice::new(device, usize::from(boot.block_size))
        .map_err(|err| format!("failed to build logical block adapter: {err}"))?;
    MyFileSystem::open_on_device(device)
        .map_err(|err| format!("failed to open filesystem image: {err}"))
}

fn mount_fs<D: BufferedBlockDevice + Send + 'static>(
    fs: MyFileSystem<D>,
    mountpoint: PathBuf,
) -> io::Result<()> {
    let mut mount_config = fuser::Config::default();
    mount_config
        .mount_options
        .push(MountOption::FSName("myfs".to_string()));
    fuser::mount2(FuseMyFileSystem::new(fs), mountpoint, &mount_config)
}

fn main() {
    env_logger::init();

    let (image_path, mountpoint) = match parse_args() {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            process::exit(2);
        }
    };

    let mount_result = match image_path {
        None => match open_memory_fs() {
            Ok(fs) => mount_fs(fs, mountpoint),
            Err(err) => {
                eprintln!("{err}");
                process::exit(1);
            }
        },
        Some(path) => match open_image_fs(&path) {
            Ok(fs) => mount_fs(fs, mountpoint),
            Err(err) => {
                eprintln!("{err}");
                process::exit(1);
            }
        },
    };
    if let Err(err) = mount_result {
        eprintln!("failed to mount myfs: {err}");
        process::exit(1);
    }
}
