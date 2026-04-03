const std = @import("std");
const os = std.os;
const linux = os.linux;

const common = @import("common.zig");
const csem = common.csem;
const cshm = common.cshm;

const SHM_PERM: comptime_int = 0o600;
const CHECK_SIGINT_INTERVAL: comptime_int = 100;
const ZOMBIE_CHECK_INTERVAL_MS: comptime_int = 500;
const DELIVERY_TIMEOUT_MS: comptime_int = 500;
const MAX_RETRY_COUNT: comptime_int = 3; // Declare client as zombie after this threshold

const HOST_SHM_NAME = "/mychat-host";
const CLIENT_SHM_PATTERN = "/mychat-client-{d}";

const ShmRegion = extern struct {
    data_sem: csem.sem_t,
    space_sem: csem.sem_t,
    frame: [common.MAX_FRAME_LEN]u8,
};

const SigIntCtx = struct {
    triggered: std.atomic.Value(bool),
};

var g_sigint_ctx: ?*SigIntCtx = null;

fn sigintHandler(_: i32, _: *const std.posix.siginfo_t, _: ?*anyopaque) callconv(.c) void {
    if (g_sigint_ctx) |ctx| {
        ctx.triggered.store(true, .monotonic);
    }
}

fn attachSigintHandler() void {
    var act = std.posix.Sigaction{
        .handler = .{ .sigaction = &sigintHandler },
        .mask = std.posix.sigemptyset(),
        .flags = std.posix.SA.SIGINFO,
    };
    std.posix.sigaction(std.posix.SIG.INT, &act, null);
}

const Client = struct {
    const Self = @This();

    shm_name: []u8,
    mmap_slice: []align(std.heap.page_size_min) u8,
    n_retries: u8,
    alloc: std.mem.Allocator,

    pub fn init(alloc: std.mem.Allocator, shm_name: []const u8) !Client {
        const fd = try cshm.open(shm_name, .{ .ACCMODE = .RDWR }, SHM_PERM);
        defer cshm.close(fd);

        try std.posix.ftruncate(fd, @sizeOf(ShmRegion));

        const mmap_slice = try std.posix.mmap(
            null,
            @sizeOf(ShmRegion),
            linux.PROT.READ | linux.PROT.WRITE,
            .{ .TYPE = .SHARED },
            fd,
            0,
        );

        return Client{
            .shm_name = try alloc.dupe(u8, shm_name),
            .mmap_slice = mmap_slice,
            .n_retries = 0,
            .alloc = alloc,
        };
    }

    pub fn deinit(self: *Self) void {
        const shm = @as(*ShmRegion, @ptrCast(self.mmap_slice.ptr));
        csem.destroy(&shm.data_sem) catch {};
        csem.destroy(&shm.space_sem) catch {};
        std.posix.munmap(self.mmap_slice);
        cshm.unlink(self.shm_name) catch {};
        self.alloc.free(self.shm_name);
    }
};

fn hostHandleFrame(alloc: std.mem.Allocator, clients: *std.StringHashMap(Client), frame: []const u8) !void {
    var unit_iter = std.mem.splitScalar(u8, frame, common.US);

    const kind = unit_iter.next() orelse return error.MalformedFrame;
    if (std.mem.eql(u8, kind, "JOIN")) {
        const name = try alloc.dupe(u8, unit_iter.next() orelse return error.MalformedFrame);
        errdefer alloc.free(name);
        const client_shm_name = unit_iter.next() orelse return error.MalformedFrame;

        const name_ = try common.allocColorizeUsername(alloc, name);
        defer alloc.free(name_);

        if (clients.contains(name)) {
            std.log.warn("Duplicated joining request for {s}, ignoring", .{name_});
            return;
        }
        try clients.put(name, try Client.init(alloc, client_shm_name));

        const tag = try common.allocColorizeMetaTag(alloc, "Joined:");
        defer alloc.free(tag);
        std.log.info("{s} {s}", .{ tag, name_ });

        const bcast_msg = try common.allocJoinFrame(alloc, name, "<redacted>");
        defer alloc.free(bcast_msg);
        broadcast(clients, bcast_msg);
    } else if (std.mem.eql(u8, kind, "MSG")) {
        const name = unit_iter.next() orelse return error.MalformedFrame;
        const msg = unit_iter.next() orelse return error.MalformedFrame;
        if (clients.getEntry(name) != null) {
            const name_ = try common.allocColorizeUsername(alloc, name);
            defer alloc.free(name_);
            std.log.info("[{s}] {s}", .{ name_, msg });

            const bcast_msg = try common.allocMsgFrame(alloc, name, msg);
            defer alloc.free(bcast_msg);
            broadcast(clients, bcast_msg);
        } else {
            std.log.warn("Message from unregistered client: {s}", .{name});
        }
    } else if (std.mem.eql(u8, kind, "LEAVE")) {
        const name = unit_iter.next() orelse return error.MalformedFrame;
        if (clients.fetchRemove(name)) |client| {
            const name_ = try common.allocColorizeUsername(alloc, name);
            defer alloc.free(name_);
            const tag = try common.allocColorizeMetaTag(alloc, "Left:");
            defer alloc.free(tag);
            std.log.info("{s} {s}", .{ tag, name_ });

            alloc.free(client.key);
            var value = client.value;
            value.deinit();
            const bcast_msg = try common.allocLeaveFrame(alloc, name);
            defer alloc.free(bcast_msg);
            broadcast(clients, bcast_msg);
        } else {
            const name_ = try common.allocColorizeUsername(alloc, name);
            defer alloc.free(name_);
            std.log.warn("Received LEAVE for non-existent client: {s}", .{name_});
        }
    }
    if (unit_iter.next() != null) {
        return error.MalformedFrame;
    }
}

fn reapZombies(alloc: std.mem.Allocator, clients: *std.StringHashMap(Client), zombies: *const std.ArrayList([]u8)) void {
    for (zombies.items) |name| {
        if (clients.fetchRemove(name)) |zombie| {
            const name_: ?[]u8 = common.allocColorizeUsername(alloc, zombie.key) catch null;
            defer if (name_) |v| alloc.free(v);

            std.log.warn("Reaped zombie client: {s} @ {s}", .{ name_ orelse zombie.key, zombie.value.shm_name });
            alloc.free(zombie.key);
            var value = zombie.value;
            value.deinit();
        }
    }
    for (zombies.items) |name| {
        const msg = common.allocLeaveFrame(alloc, name) catch continue;
        defer alloc.free(msg);
        broadcast(clients, msg);
    }
}

fn deliverToClient(client: *Client, frame: []const u8) bool {
    const shm = @as(*ShmRegion, @ptrCast(client.mmap_slice.ptr));

    var deadline = std.posix.clock_gettime(.REALTIME) catch unreachable;
    deadline.nsec += DELIVERY_TIMEOUT_MS * 1_000_000;
    if (deadline.nsec >= 1_000_000_000) {
        deadline.sec += 1;
        deadline.nsec -= 1_000_000_000;
    }

    csem.timedWait(&shm.space_sem, &deadline) catch return false;

    @memcpy(shm.frame[0..frame.len], frame);
    csem.post(&shm.data_sem) catch return false;
    return true;
}

fn broadcast(clients: *const std.StringHashMap(Client), buffer: []const u8) void {
    var iter = clients.iterator();
    while (iter.next()) |entry| {
        const client = entry.value_ptr;
        if (deliverToClient(client, buffer)) {
            client.n_retries = 0;
        } else {
            client.n_retries += 1;
        }
    }
}

fn probeZombies(alloc: std.mem.Allocator, clients: *std.StringHashMap(Client)) !void {
    var zombies = std.ArrayList([]u8).empty;
    defer {
        for (zombies.items) |name| alloc.free(name);
        zombies.deinit(alloc);
    }

    var iter = clients.iterator();
    while (iter.next()) |entry| {
        if (entry.value_ptr.n_retries >= MAX_RETRY_COUNT) {
            try zombies.append(alloc, try alloc.dupe(u8, entry.key_ptr.*));
        }
    }
    reapZombies(alloc, clients, &zombies);
}

pub fn runHost(alloc: std.mem.Allocator, _: []const u8) !void {
    var clients: std.StringHashMap(Client) = .init(alloc);
    defer {
        var iter = clients.iterator();
        while (iter.next()) |entry| {
            alloc.free(entry.key_ptr.*);
            entry.value_ptr.deinit();
        }
        clients.deinit();
    }

    const fd = try cshm.create(HOST_SHM_NAME, SHM_PERM, @sizeOf(ShmRegion));
    defer {
        cshm.close(fd);
        cshm.unlink(HOST_SHM_NAME) catch unreachable;
    }

    const host_mmap_slice = try std.posix.mmap(
        null,
        @sizeOf(ShmRegion),
        linux.PROT.READ | linux.PROT.WRITE,
        .{ .TYPE = .SHARED },
        fd,
        0,
    );
    defer {
        std.posix.munmap(host_mmap_slice);
    }
    defer {
        cshm.unlink(HOST_SHM_NAME) catch {};
    }

    const host_shm = @as(*ShmRegion, @ptrCast(host_mmap_slice.ptr));

    try csem.init(&host_shm.data_sem, 1, 0);
    errdefer csem.destroy(&host_shm.data_sem) catch {};
    try csem.init(&host_shm.space_sem, 1, 1);
    errdefer csem.destroy(&host_shm.space_sem) catch {};

    std.log.info("Host SHM: {s}", .{HOST_SHM_NAME});

    while (true) {
        try csem.wait(&host_shm.data_sem);
        const rs_idx = std.mem.indexOfScalar(u8, &host_shm.frame, common.RS) orelse continue;
        const frame = host_shm.frame[0..rs_idx];
        try csem.post(&host_shm.space_sem);

        hostHandleFrame(alloc, &clients, frame) catch |err| {
            std.log.warn("Cannot handle frame: {}.", .{err});
        };
    }
}

fn clientHandleFrame(alloc: std.mem.Allocator, my_name: []const u8, frame: []const u8) !void {
    var unit_iter = std.mem.splitScalar(u8, frame, common.US);

    const kind = unit_iter.next() orelse return error.MalformedFrame;
    if (std.mem.eql(u8, kind, "JOIN")) {
        const name = unit_iter.next() orelse return error.MalformedFrame;
        // client_shm_name
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

const RecvCtx = struct {
    alloc: std.mem.Allocator,
    my_name: []u8,
    mmap_slice: []align(std.heap.page_size_min) u8,
};

fn clientRecvLoop(ctx: *RecvCtx) void {
    const shm = @as(*ShmRegion, @ptrCast(ctx.mmap_slice.ptr));

    while (!(g_sigint_ctx orelse unreachable).triggered.load(.monotonic)) {
        csem.wait(&shm.data_sem) catch break;

        const rs_idx = std.mem.indexOfScalar(u8, &shm.frame, common.RS) orelse continue;
        const frame = shm.frame[0..rs_idx];

        csem.post(&shm.space_sem) catch break;

        clientHandleFrame(ctx.alloc, ctx.my_name, frame) catch |err| {
            std.log.warn("Cannot handle frame: {}.", .{err});
        };
    }
}

pub fn runClient(alloc: std.mem.Allocator, host_shm_name: []const u8, name: []const u8) !void {
    var sigint_ctx = SigIntCtx{ .triggered = std.atomic.Value(bool).init(false) };
    g_sigint_ctx = &sigint_ctx;
    defer g_sigint_ctx = null;
    attachSigintHandler();

    const pid = std.os.linux.getpid();
    const client_shm_name = try std.fmt.allocPrint(alloc, CLIENT_SHM_PATTERN, .{pid});
    defer alloc.free(client_shm_name);
    std.log.debug("Client SHM: {s}", .{client_shm_name});

    const fd = try cshm.create(client_shm_name, SHM_PERM, @sizeOf(ShmRegion));
    defer {
        cshm.close(fd);
        cshm.unlink(client_shm_name) catch unreachable;
    }

    // Client shm for downlink
    const client_slice = try std.posix.mmap(
        null,
        @sizeOf(ShmRegion),
        linux.PROT.READ | linux.PROT.WRITE,
        .{ .TYPE = .SHARED },
        fd,
        0,
    );
    defer std.posix.munmap(client_slice);

    const client_shm = @as(*ShmRegion, @ptrCast(client_slice.ptr));

    try csem.init(&client_shm.data_sem, 1, 0);
    errdefer csem.destroy(&client_shm.data_sem) catch {};
    try csem.init(&client_shm.space_sem, 1, 1);
    errdefer csem.destroy(&client_shm.space_sem) catch {};

    // Host shm for uplink
    const host_fd = try cshm.open(host_shm_name, .{ .ACCMODE = .RDWR }, SHM_PERM);
    defer cshm.close(host_fd);

    const host_mmap_slice = try std.posix.mmap(
        null,
        @sizeOf(ShmRegion),
        linux.PROT.READ | linux.PROT.WRITE,
        .{ .TYPE = .SHARED },
        host_fd,
        0,
    );
    defer {
        std.posix.munmap(host_mmap_slice);
    }

    const host_shm = @as(*ShmRegion, @ptrCast(host_mmap_slice.ptr));

    var joined = false;
    defer if (joined) sendLeaveBestEffort(alloc, host_shm, name);

    // Send JOIN
    try sendJoin(alloc, host_shm, name, client_shm_name);
    joined = true;

    const my_name = try alloc.dupe(u8, name);
    defer alloc.free(my_name);

    const ctx = try alloc.create(RecvCtx);
    ctx.* = RecvCtx{
        .alloc = alloc,
        .my_name = my_name,
        .mmap_slice = client_slice,
    };
    const recv_thread = try std.Thread.spawn(.{}, clientRecvLoop, .{ctx});
    defer recv_thread.detach();

    const stdin = std.fs.File.stdin();
    var stdin_reader_buf: [common.MAX_FRAME_LEN]u8 = undefined;
    var stdin_reader = stdin.reader(&stdin_reader_buf);
    while (!sigint_ctx.triggered.load(.monotonic)) {
        var fds = [_]std.posix.pollfd{.{ .fd = std.posix.STDIN_FILENO, .events = std.posix.POLL.IN, .revents = 0 }};
        const ready = std.posix.poll(&fds, CHECK_SIGINT_INTERVAL) catch break;
        if (ready == 0) continue;
        if (fds[0].revents & std.posix.POLL.IN == 0) continue;

        const maybe_line = stdin_reader.interface.takeDelimiter('\n') catch break;
        const line = maybe_line orelse continue;
        if (line.len == 0) continue;

        sendMsg(alloc, host_shm, name, line) catch break;
    }
}

fn deliverToHost(host_shm: *ShmRegion, frame: []const u8) !void {
    try csem.wait(&host_shm.space_sem);
    @memcpy(host_shm.frame[0..frame.len], frame);
    try csem.post(&host_shm.data_sem);
}

fn sendJoin(alloc: std.mem.Allocator, host_shm: *ShmRegion, name: []const u8, client_shm_name: []const u8) !void {
    const frame = try common.allocJoinFrame(alloc, name, client_shm_name);
    defer alloc.free(frame);
    try deliverToHost(host_shm, frame);
}

fn sendMsg(alloc: std.mem.Allocator, host_shm: *ShmRegion, name: []const u8, msg: []const u8) !void {
    const frame = try common.allocMsgFrame(alloc, name, msg);
    defer alloc.free(frame);
    try deliverToHost(host_shm, frame);
}

fn sendLeaveBestEffort(alloc: std.mem.Allocator, host_shm: *ShmRegion, name: []const u8) void {
    const leave_frame = common.allocLeaveFrame(alloc, name) catch return;
    defer alloc.free(leave_frame);
    deliverToHost(host_shm, leave_frame) catch {};
}
