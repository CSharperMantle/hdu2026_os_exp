const std = @import("std");

const common = @import("common.zig");

const FIFO_PERM: comptime_int = 0o600;
const CHECK_SIGINT_INTERVAL: comptime_int = 100;
const ZOMBIE_CHECK_INTERVAL_MS: i32 = 500;

const FIFO_NAME_PATTERN = "/tmp/mychat-{s}-{d}.fifo";

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

    data_fifo: []u8,
    alloc: std.mem.Allocator,

    pub fn init(alloc: std.mem.Allocator, data_fifo: []const u8) !Client {
        return Client{ .data_fifo = try alloc.dupe(u8, data_fifo), .alloc = alloc };
    }

    pub fn deinit(self: *Self) void {
        self.alloc.free(self.data_fifo);
        self.* = undefined;
    }
};

const RecvCtx = struct {
    alloc: std.mem.Allocator,
    my_name: []const u8,
    recv: std.fs.File,
};

fn mkfifo(path: []const u8, perm: u32) !void {
    const pathz = try std.heap.page_allocator.dupeZ(u8, path);
    defer std.heap.page_allocator.free(pathz);

    const rc = std.os.linux.mknod(pathz, std.posix.S.IFIFO | perm, 0);
    switch (std.posix.errno(rc)) {
        .SUCCESS => {},
        .EXIST => return error.PathAlreadyExists,
        .ACCES => return error.AccessDenied,
        .NOENT => return error.FileNotFound,
        else => return error.Unexpected,
    }
}

fn writeToPath(path: []const u8, buf: []const u8) !void {
    const file = try std.fs.openFileAbsolute(path, .{ .mode = .write_only });
    defer file.close();
    try file.writeAll(buf);
}

fn allocPrintFifoPath(alloc: std.mem.Allocator, role: []const u8) ![]u8 {
    const pid = std.os.linux.getpid();
    return std.fmt.allocPrint(alloc, FIFO_NAME_PATTERN, .{ role, pid });
}

fn hostHandleFrame(alloc: std.mem.Allocator, clients: *std.StringHashMap(Client), frame: []const u8) !void {
    var iter = std.mem.splitScalar(u8, frame, common.US);

    const kind = iter.next() orelse return error.MalformedFrame;
    if (std.mem.eql(u8, kind, "JOIN")) {
        const name = try alloc.dupe(u8, iter.next() orelse return error.MalformedFrame);
        errdefer alloc.free(name);
        const data_fifo = iter.next() orelse return error.MalformedFrame;

        const name_ = try common.allocColorizeUsername(alloc, name);
        defer alloc.free(name_);

        if (clients.contains(name)) {
            std.log.warn("Duplicated joining request for {s}, ignoring", .{name_});
            return;
        }
        try clients.put(name, try Client.init(alloc, data_fifo));

        const tag = try common.allocColorizeMetaTag(alloc, "Joined:");
        defer alloc.free(tag);
        std.log.info("{s} {s}", .{ tag, name_ });

        const bcast_msg = try common.allocJoinFrame(alloc, name, "<redacted>");
        defer alloc.free(bcast_msg);
        broadcast(clients, bcast_msg);
    } else if (std.mem.eql(u8, kind, "MSG")) {
        const name = iter.next() orelse return error.MalformedFrame;
        const msg = iter.next() orelse return error.MalformedFrame;
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
        const name = iter.next() orelse return error.MalformedFrame;
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
    if (iter.next() != null) {
        return error.MalformedFrame;
    }
}

fn reapZombies(alloc: std.mem.Allocator, clients: *std.StringHashMap(Client), zombies: *const std.ArrayList([]u8)) void {
    for (zombies.items) |name| {
        if (clients.fetchRemove(name)) |zombie| {
            const name_: ?[]u8 = common.allocColorizeUsername(alloc, zombie.key) catch null;
            defer if (name_) |v| alloc.free(v);

            std.log.warn("Reaped zombie client: {s} @ {s}", .{ name_ orelse zombie.key, zombie.value.data_fifo });
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

// Don't try to find zombies.
fn broadcast(clients: *const std.StringHashMap(Client), buffer: []const u8) void {
    var iter = clients.valueIterator();
    while (iter.next()) |client| {
        writeToPath(client.data_fifo, buffer) catch {};
    }
}

fn testFifoWritable(path: []const u8) bool {
    const fd = std.posix.open(path, std.posix.O{ .ACCMODE = .WRONLY, .NONBLOCK = true }, 0) catch return false;
    std.posix.close(fd);
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
        const data_fifo = entry.value_ptr.data_fifo;
        if (!testFifoWritable(data_fifo)) {
            try zombies.append(alloc, try alloc.dupe(u8, name));
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

    const ctrl_fifo_path = try allocPrintFifoPath(alloc, "host");
    defer alloc.free(ctrl_fifo_path);

    try mkfifo(ctrl_fifo_path, FIFO_PERM);
    defer std.fs.deleteFileAbsolute(ctrl_fifo_path) catch {};
    std.log.info("Control FIFO: {s}", .{ctrl_fifo_path});

    const ctrl_fifo = try std.fs.openFileAbsolute(ctrl_fifo_path, .{ .mode = .read_only });
    defer ctrl_fifo.close();
    const ctrl_fifo_fd = ctrl_fifo.handle;

    var reader_buf: [common.MAX_FRAME_LEN]u8 = undefined;
    var ctrl_fifo_reader = ctrl_fifo.reader(&reader_buf);

    while (!g_shutdown.load(.monotonic)) {
        // Poll ctrl FIFO, process data if there's any, otherwise probe for zombies
        var fds = [_]std.posix.pollfd{.{ .fd = ctrl_fifo_fd, .events = std.posix.POLL.IN, .revents = 0 }};
        const ready = std.posix.poll(&fds, ZOMBIE_CHECK_INTERVAL_MS) catch |err| {
            std.log.err("Poll on ctrl FIFO failed: {}", .{err});
            break;
        };
        if (ready > 0 and (fds[0].revents & std.posix.POLL.IN != 0)) {
            const frame = ctrl_fifo_reader.interface.takeDelimiter(common.RS) catch break orelse continue;
            if (frame.len == 0) continue;
            hostHandleFrame(alloc, &clients, frame) catch |err| {
                std.log.warn("Cannot handle frame: {}. raw={x})", .{ err, frame });
            };
        } else {
            try probeZombies(alloc, &clients);
        }
    }
}

fn sendJoin(alloc: std.mem.Allocator, fifo: []const u8, name: []const u8, data_fifo_path: []const u8) !void {
    const frame = try common.allocJoinFrame(alloc, name, data_fifo_path);
    defer alloc.free(frame);
    try writeToPath(fifo, frame);
}

fn sendMsg(alloc: std.mem.Allocator, fifo: []const u8, name: []const u8, msg: []const u8) !void {
    const frame = try common.allocMsgFrame(alloc, name, msg);
    defer alloc.free(frame);
    try writeToPath(fifo, frame);
}

fn sendLeaveBestEffort(alloc: std.mem.Allocator, fifo: []const u8, name: []const u8) void {
    const frame = common.allocLeaveFrame(alloc, name) catch return;
    defer alloc.free(frame);
    writeToPath(fifo, frame) catch {};
}

fn clientHandleFrame(alloc: std.mem.Allocator, my_name: []const u8, frame: []const u8) !void {
    var iter = std.mem.splitScalar(u8, frame, common.US);

    const kind = iter.next() orelse return error.MalformedFrame;
    if (std.mem.eql(u8, kind, "JOIN")) {
        const name = iter.next() orelse return error.MalformedFrame;
        // data_fifo
        _ = iter.next() orelse return error.MalformedFrame;

        const name_ = try common.allocColorizeUsername(alloc, name);
        defer alloc.free(name_);
        const tag = try common.allocColorizeMetaTag(alloc, "Joined:");
        defer alloc.free(tag);

        std.log.info("{s} {s}", .{ tag, name_ });
    } else if (std.mem.eql(u8, kind, "MSG")) {
        const name = iter.next() orelse return error.MalformedFrame;
        const msg = iter.next() orelse return error.MalformedFrame;

        const me_suffix = if (std.mem.eql(u8, name, my_name)) " (me)" else "";

        const name_ = try common.allocColorizeUsername(alloc, name);
        defer alloc.free(name_);

        std.log.info("[{s}{s}] {s}", .{ name_, me_suffix, msg });
    } else if (std.mem.eql(u8, kind, "LEAVE")) {
        const name = iter.next() orelse return error.MalformedFrame;

        const name_ = try common.allocColorizeUsername(alloc, name);
        defer alloc.free(name_);
        const tag = try common.allocColorizeMetaTag(alloc, "Left:");
        defer alloc.free(tag);

        std.log.info("{s} {s}", .{ tag, name_ });
    }
    if (iter.next() != null) {
        return error.MalformedFrame;
    }
}

fn clientRecvLoop(ctx: RecvCtx) void {
    const recv = ctx.recv;
    defer recv.close();

    var reader_buf: [common.MAX_FRAME_LEN]u8 = undefined;
    var recv_reader = recv.reader(&reader_buf);
    while (recv_reader.interface.takeDelimiter(common.RS)) |maybe_frame| {
        const frame = maybe_frame orelse continue;
        if (frame.len == 0) continue;
        clientHandleFrame(ctx.alloc, ctx.my_name, frame) catch |err| {
            std.log.warn("Cannot handle frame: {}. raw={x})", .{ err, frame });
        };
    } else |_| return;
}

pub fn runClient(alloc: std.mem.Allocator, ctrl_fifo_path: []const u8, name: []const u8) !void {
    // Set a global flag when receiving SIGINT for shutdown
    attachSigintHandler();

    const data_fifo_path = try allocPrintFifoPath(alloc, "client");
    defer alloc.free(data_fifo_path);
    std.log.debug("Data FIFO: {s}", .{data_fifo_path});

    try mkfifo(data_fifo_path, FIFO_PERM);
    defer std.fs.deleteFileAbsolute(data_fifo_path) catch {};

    var joined = false;
    defer if (joined) sendLeaveBestEffort(alloc, ctrl_fifo_path, name);

    try sendJoin(alloc, ctrl_fifo_path, name, data_fifo_path);
    joined = true;

    const my_name = try alloc.dupe(u8, name);
    const recv = try std.fs.openFileAbsolute(data_fifo_path, .{ .mode = .read_only });
    const recv_thread = try std.Thread.spawn(.{}, clientRecvLoop, .{RecvCtx{
        .alloc = alloc,
        .my_name = my_name,
        .recv = recv,
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
        try sendMsg(alloc, ctrl_fifo_path, name, line);
    }
}
