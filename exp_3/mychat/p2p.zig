const std = @import("std");
const os = std.os;
const linux = os.linux;

const common = @import("common.zig");
const csem = common.csem;
const cshm = common.cshm;

const SHM_PERM: comptime_int = 0o600;
const CHECK_SIGINT_INTERVAL: comptime_int = 100;

const P2P_SHM_PATTERN = "/mychat-p2p-{d}";

const Role = enum { host, client };

const ShmRegion = extern struct {
    const Self = @This();

    turnstile: csem.sem_t,
    empty: csem.sem_t,
    full_host: csem.sem_t,
    full_client: csem.sem_t,

    data: [common.MAX_FRAME_LEN]u8,

    pub const SemPair = struct {
        mine: *csem.sem_t,
        opposite: *csem.sem_t,
    };

    pub fn getFullSems(self: *Self, me: Role) SemPair {
        return switch (me) {
            .host => .{ .mine = &self.full_host, .opposite = &self.full_client },
            .client => .{ .mine = &self.full_client, .opposite = &self.full_host },
        };
    }
};

var g_shutdown = std.atomic.Value(i32).init(0);

fn sigintHandler(_: i32, _: *const std.posix.siginfo_t, _: ?*anyopaque) callconv(.c) void {
    g_shutdown.store(1, .monotonic);
}

fn attachSigintHandler() void {
    var act = std.posix.Sigaction{
        .handler = .{ .sigaction = &sigintHandler },
        .mask = std.posix.sigemptyset(),
        .flags = std.posix.SA.SIGINFO,
    };
    std.posix.sigaction(std.posix.SIG.INT, &act, null);
}

fn p2pSend(shm: *ShmRegion, me: Role, frame: []const u8) void {
    const full_sems = shm.getFullSems(me);

    csem.wait(&shm.turnstile) catch @panic("csem.wait(&shm.turnstile)");
    defer csem.post(&shm.turnstile) catch @panic("csem.post(&shm.turnstile)");

    csem.wait(&shm.empty) catch @panic("csem.wait(&shm.empty)");
    @memcpy(shm.data[0..frame.len], frame);
    csem.post(full_sems.opposite) catch @panic("csem.post(full_sems.opposite)");
}

fn p2pRecv(shm: *ShmRegion, me: Role, buf: []u8) void {
    const full_sems = shm.getFullSems(me);

    csem.wait(full_sems.mine) catch @panic("csem.wait(full_sems.mine)");
    @memcpy(buf, &shm.data);
    csem.post(&shm.empty) catch @panic("csem.post(&shm.empty)");
}

fn handleFrame(alloc: std.mem.Allocator, my_name: []const u8, frame: []const u8) !void {
    // We can't takeDelimiter from a plain buffer, so manually check and unpack the record structure.
    var record_iter = std.mem.splitScalar(u8, frame, common.RS);
    const record = record_iter.next() orelse return error.MalformedFrame;
    // The frame layout is "xxx<RS>", so `trailer` should be non-null.
    // Note that we can't assert it's empty here, since the buffer can be under-filled.
    _ = record_iter.next() orelse return error.MalformedFrame;

    var unit_iter = std.mem.splitScalar(u8, record, common.US);

    const kind = unit_iter.next() orelse return error.MalformedFrame;
    if (std.mem.eql(u8, kind, "JOIN")) {
        const name = unit_iter.next() orelse return error.MalformedFrame;
        // shm
        _ = unit_iter.next() orelse return error.MalformedFrame;

        const name_ = try common.allocColorizeUsername(alloc, name);
        defer alloc.free(name_);
        const tag = try common.allocColorizeMetaTag(alloc, "Joined:");
        defer alloc.free(tag);

        std.log.info("{s} {s}", .{ tag, name_ });
    } else if (std.mem.eql(u8, kind, "MSG")) {
        const name = unit_iter.next() orelse return error.MalformedFrame;
        const msg = unit_iter.next() orelse return error.MalformedFrame;

        const me_suffix = if (std.mem.eql(u8, name, my_name)) " (me)" else "";

        const name_ = try common.allocColorizeUsername(alloc, name);
        defer alloc.free(name_);

        std.log.info("[{s}{s}] {s}", .{ name_, me_suffix, msg });
    } else if (std.mem.eql(u8, kind, "LEAVE")) {
        const name = unit_iter.next() orelse return error.MalformedFrame;

        const name_ = try common.allocColorizeUsername(alloc, name);
        defer alloc.free(name_);
        const tag = try common.allocColorizeMetaTag(alloc, "Left:");
        defer alloc.free(tag);

        std.log.info("{s} {s}", .{ tag, name_ });
    }
    if (unit_iter.next() != null) {
        return error.MalformedFrame;
    }
}

fn recvLoop(shm: *ShmRegion, me: Role, alloc: std.mem.Allocator, name: []const u8) void {
    var buf: [common.MAX_FRAME_LEN]u8 = undefined;
    while (g_shutdown.load(.monotonic) == 0) {
        p2pRecv(shm, me, &buf);
        handleFrame(alloc, name, &buf) catch |err| {
            std.log.warn("Cannot handle frame: {}.", .{err});
        };
    }
}

fn sendShutdown(alloc: std.mem.Allocator, shm: *ShmRegion, me: Role, name: []const u8) void {
    const msg = common.allocLeaveFrame(alloc, name) catch return;
    defer alloc.free(msg);
    p2pSend(shm, me, msg);
}

fn sendLoop(shm: *ShmRegion, me: Role, alloc: std.mem.Allocator, name: []const u8) void {
    defer sendShutdown(alloc, shm, me, name);

    var stdin_buf: [common.MAX_FRAME_LEN]u8 = undefined;
    var stdin_reader = std.fs.File.stdin().reader(&stdin_buf);
    while (g_shutdown.load(.monotonic) == 0) {
        var fds = [_]std.posix.pollfd{.{ .fd = std.posix.STDIN_FILENO, .events = std.posix.POLL.IN, .revents = 0 }};
        const ready = std.posix.poll(&fds, CHECK_SIGINT_INTERVAL) catch break;
        if (ready == 0) continue;
        if (fds[0].revents & std.posix.POLL.IN == 0) continue;

        const maybe_line = stdin_reader.interface.takeDelimiter('\n') catch break;
        const line = maybe_line orelse continue;
        if (line.len == 0) continue;

        const frame = common.allocMsgFrame(alloc, name, line) catch continue;
        defer alloc.free(frame);
        p2pSend(shm, me, frame);
    }
}

pub fn runHost(alloc: std.mem.Allocator, name: []const u8) !void {
    const pid = std.os.linux.getpid();
    const shm_name = try std.fmt.allocPrint(alloc, P2P_SHM_PATTERN, .{pid});
    defer alloc.free(shm_name);

    const fd = try cshm.create(shm_name, SHM_PERM, @sizeOf(ShmRegion));
    defer {
        cshm.close(fd);
        cshm.unlink(shm_name) catch {};
    }
    const shm_slice = try std.posix.mmap(
        null,
        @sizeOf(ShmRegion),
        linux.PROT.READ | linux.PROT.WRITE,
        .{ .TYPE = .SHARED },
        fd,
        0,
    );
    defer std.posix.munmap(shm_slice);

    const shm = @as(*ShmRegion, @ptrCast(shm_slice.ptr));

    try csem.init(&shm.turnstile, 1, 1);
    errdefer csem.destroy(&shm.turnstile) catch {};
    try csem.init(&shm.empty, 1, 1);
    errdefer csem.destroy(&shm.empty) catch {};
    try csem.init(&shm.full_host, 1, 0);
    errdefer csem.destroy(&shm.full_host) catch {};
    try csem.init(&shm.full_client, 1, 0);
    errdefer csem.destroy(&shm.full_client) catch {};

    std.log.info("Host SHM: {s}", .{shm_name});

    const recv_thread = try std.Thread.spawn(.{}, recvLoop, .{ shm, .host, alloc, name });
    defer recv_thread.detach();

    sendLoop(shm, .host, alloc, name);
}

pub fn runClient(alloc: std.mem.Allocator, host_shm_name: []const u8, name: []const u8) !void {
    attachSigintHandler();

    const host_fd = try cshm.open(host_shm_name, .{ .ACCMODE = .RDWR }, SHM_PERM);
    defer cshm.close(host_fd);

    const shm_slice = try std.posix.mmap(
        null,
        @sizeOf(ShmRegion),
        linux.PROT.READ | linux.PROT.WRITE,
        .{ .TYPE = .SHARED },
        host_fd,
        0,
    );
    defer std.posix.munmap(shm_slice);
    const shm = @as(*ShmRegion, @ptrCast(shm_slice.ptr));

    const join_frame = try common.allocJoinFrame(alloc, name, host_shm_name);
    defer alloc.free(join_frame);
    p2pSend(shm, .client, join_frame);

    const recv_thread = try std.Thread.spawn(.{}, recvLoop, .{ shm, .client, alloc, name });
    defer recv_thread.detach();

    sendLoop(shm, .client, alloc, name);
}
