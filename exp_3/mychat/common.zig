const std = @import("std");

// <https://tldp.org/LDP/lpg/node13.html>
// #define _POSIX_PIPE_BUF 512
pub const MAX_FRAME_LEN: comptime_int = 512;

pub const US: comptime_int = '\x1f';
pub const RS: comptime_int = '\x1e';

pub fn allocJoinFrame(alloc: std.mem.Allocator, name: []const u8, info: []const u8) ![]u8 {
    const buf = try alloc.alloc(u8, MAX_FRAME_LEN);
    return std.fmt.bufPrint(buf, "JOIN{c}{s}{c}{s}{c}", .{ US, name, US, info, RS });
}

pub fn allocLeaveFrame(alloc: std.mem.Allocator, name: []const u8) ![]u8 {
    const buf = try alloc.alloc(u8, MAX_FRAME_LEN);
    return std.fmt.bufPrint(buf, "LEAVE{c}{s}{c}", .{ US, name, RS });
}

pub fn allocMsgFrame(alloc: std.mem.Allocator, name: []const u8, msg: []const u8) ![]u8 {
    const buf = try alloc.alloc(u8, MAX_FRAME_LEN);
    return std.fmt.bufPrint(buf, "MSG{c}{s}{c}{s}{c}", .{ US, name, US, msg, RS });
}

pub fn allocColorizeMetaTag(alloc: std.mem.Allocator, content: []const u8) ![]u8 {
    return std.fmt.allocPrint(alloc, "\x1b[0;93;49m{s}\x1b[0m", .{content});
}

pub fn allocColorizeUsername(alloc: std.mem.Allocator, content: []const u8) ![]u8 {
    return std.fmt.allocPrint(alloc, "\x1b[0;96;49m{s}\x1b[0m", .{content});
}

// From <mqueue.h>

pub const cmq = struct {
    pub const mq_attr = extern struct {
        mq_flags: i64,
        mq_maxmsg: i64,
        mq_msgsize: i64,
        mq_curmsgs: i64,
    };

    pub const mqd_t = i32;

    extern fn mq_open(name: [*:0]const u8, oflag: u32, mode: std.posix.mode_t, attr: ?*anyopaque) c_int;
    extern fn mq_close(mqd: c_int) c_int;
    extern fn mq_unlink(name: [*:0]const u8) c_int;
    extern fn mq_send(mqd: c_int, msg_ptr: [*]const u8, msg_len: usize, msg_prio: c_uint) c_int;
    extern fn mq_receive(mqd: c_int, msg_ptr: [*]u8, msg_len: usize, msg_prio: ?*c_uint) isize;

    pub fn openZ(name: [*:0]const u8, oflag: std.posix.O, mode: std.posix.mode_t, attr: ?*mq_attr) !mqd_t {
        const mqd = mq_open(name, @bitCast(oflag), mode, attr);
        switch (std.posix.errno(mqd)) {
            .SUCCESS => return @intCast(mqd),

            .ACCES => return error.AccessDenied,
            .EXIST => return error.PathAlreadyExists,
            .INVAL => return error.BadPathName,
            .MFILE => return error.ProcessFdQuotaExceeded,
            .NAMETOOLONG => return error.NameTooLong,
            .NFILE => return error.SystemFdQuotaExceeded,
            .NOENT => return error.FileNotFound,
            .NOMEM => return error.SystemResources,
            .NOSPC => return error.NoSpaceLeft,
            else => |err| return std.posix.unexpectedErrno(err),
        }
    }

    pub fn open(name: []const u8, oflag: std.posix.O, mode: std.posix.mode_t, attr: ?*mq_attr) !mqd_t {
        const name_ = try std.posix.toPosixPath(name);
        return openZ(&name_, oflag, mode, attr);
    }

    pub fn close(mqd: mqd_t) !void {
        const rc = mq_close(mqd);
        switch (std.posix.errno(rc)) {
            .SUCCESS => return,

            .BADF => return error.BadDescriptor,
            else => |err| return std.posix.unexpectedErrno(err),
        }
    }

    pub fn unlinkZ(name: [*:0]const u8) !void {
        const rc = mq_unlink(name);
        switch (std.posix.errno(rc)) {
            .SUCCESS => return,

            .ACCES => return error.AccessDenied,
            .NAMETOOLONG => return error.NameTooLong,
            .NOENT => return error.FileNotFound,
            else => |err| return std.posix.unexpectedErrno(err),
        }
    }

    pub fn unlink(name: []const u8) !void {
        const name_ = try std.posix.toPosixPath(name);
        return unlinkZ(&name_);
    }

    pub fn sendC(mqd: mqd_t, msg_ptr: [*]const u8, msg_len: usize, msg_prio: c_uint) !void {
        const rc = mq_send(mqd, msg_ptr, msg_len, msg_prio);
        switch (std.posix.errno(rc)) {
            .SUCCESS => return,

            .AGAIN => return error.WouldBlock,
            .BADF => return error.BadDescriptor,
            .INTR => return error.Interrupted,
            .INVAL => return error.BadPathName,
            .MSGSIZE => return error.MessageTooBig,
            .TIMEDOUT => return error.ConnectionTimedOut,
            else => |err| return std.posix.unexpectedErrno(err),
        }
    }

    pub fn send(mqd: mqd_t, msg: []const u8, msg_prio: c_uint) !void {
        return sendC(mqd, msg.ptr, msg.len, msg_prio);
    }

    pub fn receiveC(mqd: mqd_t, msg_ptr: [*]u8, msg_len: usize, msg_prio: ?*c_uint) !usize {
        const rc = mq_receive(mqd, msg_ptr, msg_len, msg_prio);
        if (rc >= 0) {
            return @intCast(rc);
        } else {
            switch (std.posix.errno(rc)) {
                .SUCCESS => unreachable,

                .AGAIN => return error.WouldBlock,
                .BADF => return error.BadDescriptor,
                .INTR => return error.Interrupted,
                .INVAL => return error.BadPathName,
                .MSGSIZE => return error.MessageTooBig,
                .TIMEDOUT => return error.ConnectionTimedOut,
                else => |err| return std.posix.unexpectedErrno(err),
            }
        }
    }

    pub fn receive(mqd: mqd_t, msg: []u8, msg_prio: ?*c_uint) !usize {
        return receiveC(mqd, msg.ptr, msg.len, msg_prio);
    }
};

// From <semaphore.h>

pub const csem = struct {
    // <bits/semaphore.h>
    //
    // #if __WORDSIZE == 64
    // # define __SIZEOF_SEM_T 32
    // #else
    // # define __SIZEOF_SEM_T 16
    // #endif
    // typedef union
    // {
    //   char __size[__SIZEOF_SEM_T];
    //   long int __align;
    // } sem_t;
    pub const sem_t = extern struct {
        _opaque: [64]u8,
    };

    extern fn sem_init(sem: *sem_t, pshared: c_int, value: c_uint) c_int;
    extern fn sem_destroy(sem: *sem_t) c_int;
    extern fn sem_wait(sem: *sem_t) c_int;
    extern fn sem_trywait(sem: *sem_t) c_int;
    extern fn sem_timedwait(sem: *sem_t, abstime: *const std.posix.timespec) c_int;
    extern fn sem_post(sem: *sem_t) c_int;

    pub fn init(sem: *sem_t, pshared: c_int, value: c_uint) !void {
        switch (std.posix.errno(sem_init(sem, pshared, value))) {
            .SUCCESS => return,
            .INVAL => return error.InvalidSemaphore,
            .NOSYS => return error.SystemOutdated,
            else => |err| return std.posix.unexpectedErrno(err),
        }
    }

    pub fn destroy(sem: *sem_t) !void {
        switch (std.posix.errno(sem_destroy(sem))) {
            .SUCCESS => return,
            .INVAL => return error.InvalidValue,
            else => |err| return std.posix.unexpectedErrno(err),
        }
    }

    pub fn wait(sem: *sem_t) !void {
        switch (std.posix.errno(sem_wait(sem))) {
            .SUCCESS => return,
            .INTR => return error.Interrupted,
            .INVAL => return error.InvalidSemaphore,
            else => |err| return std.posix.unexpectedErrno(err),
        }
    }

    pub fn timedWait(sem: *sem_t, abstime: *const std.posix.timespec) !void {
        switch (std.posix.errno(sem_timedwait(sem, abstime))) {
            .SUCCESS => return,
            .INTR => return error.Interrupted,
            .INVAL => return error.InvalidTimespec,
            .TIMEDOUT => return error.TimedOut,
            else => |err| return std.posix.unexpectedErrno(err),
        }
    }

    pub fn post(sem: *sem_t) !void {
        switch (std.posix.errno(sem_post(sem))) {
            .SUCCESS => return,
            .INVAL => return error.InvalidSemaphore,
            .OVERFLOW => return error.SemValueOverflow,
            else => |err| return std.posix.unexpectedErrno(err),
        }
    }
};

// From <sys/mman.h> and <sys/stat.h>

pub const cshm = struct {
    extern fn shm_open(name: [*:0]const u8, oflag: c_int, mode: std.posix.mode_t) c_int;
    extern fn shm_unlink(name: [*:0]const u8) c_int;

    pub fn openZ(name: [*:0]const u8, oflag: std.posix.O, mode: std.posix.mode_t) !c_int {
        const fd = shm_open(name, @bitCast(oflag), mode);
        if (fd < 0) {
            return switch (std.posix.errno(fd)) {
                .ACCES => return error.AccessDenied,
                .EXIST => return error.PathAlreadyExists,
                .INVAL => return error.BadPathName,
                .MFILE => return error.ProcessFdQuotaExceeded,
                .NAMETOOLONG => return error.NameTooLong,
                .NFILE => return error.SystemFdQuotaExceeded,
                .NOENT => return error.FileNotFound,
                .NOMEM => return error.OutOfMemory,
                .NOSPC => return error.NoSpaceLeft,
                else => |err| return std.posix.unexpectedErrno(err),
            };
        }
        return fd;
    }

    pub fn open(name: []const u8, oflag: std.posix.O, mode: std.posix.mode_t) !c_int {
        const name_ = try std.posix.toPosixPath(name);
        return openZ(&name_, oflag, mode);
    }

    pub fn create(name: []const u8, mode: std.posix.mode_t, size: usize) !c_int {
        const fd = try open(name, .{ .CREAT = true, .EXCL = true, .ACCMODE = .RDWR }, mode);
        errdefer {
            std.posix.close(fd);
            _ = shm_unlink(@as([*:0]const u8, @ptrFromInt(@intFromPtr(name.ptr))));
        }
        try std.posix.ftruncate(fd, size);
        return fd;
    }

    pub fn close(fd: c_int) void {
        std.posix.close(fd);
    }

    pub fn unlinkZ(name: [*:0]const u8) !void {
        if (shm_unlink(name) < 0) {
            return switch (std.posix.errno(shm_unlink(name))) {
                .ACCES => return error.AccessDenied,
                .NAMETOOLONG => return error.NameTooLong,
                .NOENT => return error.FileNotFound,
                else => |err| return std.posix.unexpectedErrno(err),
            };
        }
    }

    pub fn unlink(name: []const u8) !void {
        const name_ = try std.posix.toPosixPath(name);
        return unlinkZ(&name_);
    }
};
