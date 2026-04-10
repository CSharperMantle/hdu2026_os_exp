const std = @import("std");

const common = @import("common.zig");
const cmq = common.cmq;

const MQ_PERM: comptime_int = 0o600;
const CHECK_SIGINT_INTERVAL: comptime_int = 100;
const ZOMBIE_CHECK_INTERVAL_MS: comptime_int = 500;

const HOST_MQ_NAME = "/mychat-host";
const CLIENT_MQ_PATTERN = "/mychat-client-{d}";

const N_MAX_MSG: comptime_int = 4;
const NEW_MQ_ATTR: cmq.mq_attr = .{
    .mq_flags = 0,
    .mq_maxmsg = N_MAX_MSG,
    .mq_msgsize = common.MAX_FRAME_LEN,
    .mq_curmsgs = 0,
};

var g_shutdown = std.atomic.Value(bool).init(false);

fn sigintHandler(_: i32, _: *const std.posix.siginfo_t, _: ?*anyopaque) callconv(.c) void {
    g_shutdown.store(true, .monotonic);
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

    mq_name: []u8,
    mq: cmq.mqd_t,
    alloc: std.mem.Allocator,

    pub fn init(alloc: std.mem.Allocator, mq_name: []const u8) !Client {
        const mq = try cmq.open(
            mq_name,
            .{ .ACCMODE = .WRONLY },
            0,
            null,
        );
        return Client{
            .mq_name = try alloc.dupe(u8, mq_name),
            .mq = mq,
            .alloc = alloc,
        };
    }

    pub fn deinit(self: *Self) void {
        cmq.close(self.mq) catch unreachable;
        self.alloc.free(self.mq_name);
        self.* = undefined;
    }
};

fn allocClientMqName(alloc: std.mem.Allocator) ![]u8 {
    const pid = std.os.linux.getpid();
    return std.fmt.allocPrint(alloc, CLIENT_MQ_PATTERN, .{pid});
}

fn send(mq_name: []const u8, buf: []const u8) !void {
    const mqd = try cmq.open(
        mq_name,
        .{ .ACCMODE = .WRONLY },
        0,
        null,
    );
    defer cmq.close(mqd) catch unreachable;

    try cmq.send(mqd, buf, 0);
}

// Don't try to find zombies.
fn broadcast(clients: *const std.StringHashMap(Client), buffer: []const u8) void {
    var iter = clients.valueIterator();
    while (iter.next()) |client| {
        cmq.send(client.mq, buffer, 0) catch {};
    }
}

fn hostHandleFrame(alloc: std.mem.Allocator, clients: *std.StringHashMap(Client), frame: []const u8) !void {
    // We can't takeDelimiter from an MQ, so manually check and unpack the record structure.
    var record_iter = std.mem.splitScalar(u8, frame, common.RS);
    const record = record_iter.next() orelse return error.MalformedFrame;
    // The frame layout is "xxx<RS>", so `trailer` should be non-null yet empty.
    const trailer = record_iter.next() orelse return error.MalformedFrame;
    if (!std.mem.eql(u8, trailer, "")) {
        // Each frame should be ended by RS and only contain one record.
        return error.MalformedFrame;
    }

    var unit_iter = std.mem.splitScalar(u8, record, common.US);

    const kind = unit_iter.next() orelse return error.MalformedFrame;
    if (std.mem.eql(u8, kind, "JOIN")) {
        const name = try alloc.dupe(u8, unit_iter.next() orelse return error.MalformedFrame);
        errdefer alloc.free(name);
        const client_mq_name = unit_iter.next() orelse return error.MalformedFrame;

        const name_ = try common.allocColorizeUsername(alloc, name);
        defer alloc.free(name_);

        if (clients.contains(name)) {
            std.log.warn("Duplicated joining request for {s}, ignoring", .{name_});
            return;
        }
        try clients.put(name, try Client.init(alloc, client_mq_name));

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

            std.log.warn("Reaped zombie client: {s} @ {s}", .{ name_ orelse zombie.key, zombie.value.mq_name });
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

fn testMqExists(mq_name: []const u8) bool {
    const mqd = cmq.open(
        mq_name,
        .{ .ACCMODE = .WRONLY },
        0,
        null,
    ) catch return false;
    cmq.close(mqd) catch unreachable;
    return true;
}

// Probe and reap newly-found zombies.
fn probeZombies(alloc: std.mem.Allocator, clients: *std.StringHashMap(Client)) !void {
    var zombies = std.ArrayList([]u8).empty;
    defer {
        for (zombies.items) |name| alloc.free(name);
        zombies.deinit(alloc);
    }
    var iter = clients.iterator();
    while (iter.next()) |entry| {
        const name = entry.key_ptr.*;
        const mq_name = entry.value_ptr.mq_name;
        if (!testMqExists(mq_name)) {
            try zombies.append(alloc, try alloc.dupe(u8, name));
        }
    }
    reapZombies(alloc, clients, &zombies);
}

const ZombieProbeCtx = struct {
    alloc: std.mem.Allocator,
    clients: *std.StringHashMap(Client),
    running: *std.atomic.Value(bool),
};

fn zombieProbeLoop(ctx: ZombieProbeCtx) void {
    while (ctx.running.load(.monotonic)) {
        std.Thread.sleep(ZOMBIE_CHECK_INTERVAL_MS * std.time.ns_per_ms);
        probeZombies(ctx.alloc, ctx.clients) catch {};
    }
}

pub fn runHost(alloc: std.mem.Allocator, _: []const u8) !void {
    attachSigintHandler();
    var clients: std.StringHashMap(Client) = .init(alloc);
    defer {
        var iter = clients.iterator();
        while (iter.next()) |entry| {
            alloc.free(entry.key_ptr.*);
            entry.value_ptr.deinit();
        }
        clients.deinit();
    }

    // Create the host MQ
    var attr = NEW_MQ_ATTR;
    const host_mq = try cmq.open(
        HOST_MQ_NAME,
        .{ .ACCMODE = .RDONLY, .CREAT = true, .EXCL = true },
        MQ_PERM,
        &attr,
    );
    defer {
        cmq.close(host_mq) catch unreachable;
        cmq.unlink(HOST_MQ_NAME) catch unreachable;
    }
    std.log.info("Host MQ: {s}", .{HOST_MQ_NAME});

    // Start zombie probe thread
    var running = std.atomic.Value(bool).init(true);
    const probe_ctx = ZombieProbeCtx{
        .alloc = alloc,
        .clients = &clients,
        .running = &running,
    };
    const probe_thread = try std.Thread.spawn(.{}, zombieProbeLoop, .{probe_ctx});
    defer {
        running.store(false, .monotonic);
        probe_thread.join();
    }

    var recv_buf: [common.MAX_FRAME_LEN]u8 = undefined;

    while (!g_shutdown.load(.monotonic)) {
        // Blocking receive - zombie detection happens in separate thread
        const received = cmq.receive(host_mq, &recv_buf, null) catch |err| {
            std.log.err("mq_receive failed: {}", .{err});
            break;
        };
        if (received == 0) continue;
        const frame = recv_buf[0..received];

        hostHandleFrame(alloc, &clients, frame) catch |err| {
            std.log.warn("Cannot handle frame: {}. raw={x})", .{ err, frame });
        };
    }
}

fn clientHandleFrame(alloc: std.mem.Allocator, my_name: []const u8, frame: []const u8) !void {
    // We can't takeDelimiter from an MQ, so manually check and unpack the record structure.
    var record_iter = std.mem.splitScalar(u8, frame, common.RS);
    const record = record_iter.next() orelse return error.MalformedFrame;
    // The frame layout is "xxx<RS>", so `trailer` should be non-null yet empty.
    const trailer = record_iter.next() orelse return error.MalformedFrame;
    if (!std.mem.eql(u8, trailer, "")) {
        // Each frame should be ended by RS and only contain one record.
        return error.MalformedFrame;
    }

    var unit_iter = std.mem.splitScalar(u8, record, common.US);

    const kind = unit_iter.next() orelse return error.MalformedFrame;
    if (std.mem.eql(u8, kind, "JOIN")) {
        const name = unit_iter.next() orelse return error.MalformedFrame;
        // client_mq_name
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
    mq: cmq.mqd_t,
};

fn clientRecvLoop(ctx: RecvCtx) void {
    var recv_buf: [common.MAX_FRAME_LEN]u8 = undefined;

    // Handle Ctrl-C
    while (!g_shutdown.load(.monotonic)) {
        // Blocking receive
        const received = cmq.receive(ctx.mq, &recv_buf, null) catch break;
        if (received == 0) continue;

        const frame = recv_buf[0..received];
        clientHandleFrame(ctx.alloc, ctx.my_name, frame) catch |err| {
            std.log.warn("Cannot handle frame: {}. raw={x})", .{ err, frame });
        };
    }
}

fn sendJoin(alloc: std.mem.Allocator, mq_name: []const u8, name: []const u8, client_mq_name: []const u8) !void {
    const frame = try common.allocJoinFrame(alloc, name, client_mq_name);
    defer alloc.free(frame);
    try send(mq_name, frame);
}

fn sendMsg(alloc: std.mem.Allocator, mq_name: []const u8, name: []const u8, msg: []const u8) !void {
    const frame = try common.allocMsgFrame(alloc, name, msg);
    defer alloc.free(frame);
    try send(mq_name, frame);
}

fn sendLeaveBestEffort(alloc: std.mem.Allocator, mq_name: []const u8, name: []const u8) void {
    const frame = common.allocLeaveFrame(alloc, name) catch return;
    defer alloc.free(frame);
    send(mq_name, frame) catch {};
}

pub fn runClient(alloc: std.mem.Allocator, host_mq_name: []const u8, name: []const u8) !void {
    // Set a global flag when receiving SIGINT for shutdown
    attachSigintHandler();

    const client_mq_name = try allocClientMqName(alloc);
    defer alloc.free(client_mq_name);
    std.log.debug("Client MQ: {s}", .{client_mq_name});

    // Create the client's private MQ
    var attr = NEW_MQ_ATTR;
    const client_mq = try cmq.open(
        client_mq_name,
        .{ .ACCMODE = .RDONLY, .CREAT = true, .EXCL = true },
        MQ_PERM,
        &attr,
    );
    defer {
        cmq.close(client_mq) catch unreachable;
        cmq.unlink(client_mq_name) catch unreachable;
    }

    var joined = false;
    defer if (joined) sendLeaveBestEffort(alloc, host_mq_name, name);

    try sendJoin(alloc, host_mq_name, name, client_mq_name);
    joined = true;

    const my_name = try alloc.dupe(u8, name);
    const recv_thread = try std.Thread.spawn(.{}, clientRecvLoop, .{RecvCtx{
        .alloc = alloc,
        .my_name = my_name,
        .mq = client_mq,
    }});
    defer recv_thread.detach();

    const stdin = std.fs.File.stdin();
    var stdin_reader_buf: [common.MAX_FRAME_LEN]u8 = undefined;
    var stdin_reader = stdin.reader(&stdin_reader_buf);
    while (!g_shutdown.load(.monotonic)) {
        // Stdin has data?
        var fds = [_]std.posix.pollfd{.{ .fd = std.posix.STDIN_FILENO, .events = std.posix.POLL.IN, .revents = 0 }};
        const ready = std.posix.poll(&fds, CHECK_SIGINT_INTERVAL) catch break;
        if (ready == 0) continue; // timeout, check if interrupted.
        if (fds[0].revents & std.posix.POLL.IN == 0) continue;

        const maybe_line = stdin_reader.interface.takeDelimiter('\n') catch break;
        const line = maybe_line orelse continue;
        if (line.len == 0) continue;
        try sendMsg(alloc, host_mq_name, name, line);
    }
}
