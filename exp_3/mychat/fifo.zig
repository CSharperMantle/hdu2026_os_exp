const std = @import("std");

const common = @import("common.zig");

const FIFO_PERM: comptime_int = 0o600;
const CHECK_SIGINT_INTERVAL: comptime_int = 100;

const SigIntCtx = struct {
    flag: std.atomic.Value(bool),
};

var g_sigint_ctx: ?*SigIntCtx = null;

fn sigintHandler(_: i32, _: *const std.posix.siginfo_t, _: ?*anyopaque) callconv(.c) void {
    if (g_sigint_ctx) |ctx| {
        ctx.flag.store(true, .monotonic);
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

fn allocPrintFifoPath(alloc: std.mem.Allocator, role: []const u8) ![]u8 {
    const pid = std.os.linux.getpid();
    const ts = std.time.milliTimestamp();
    return std.fmt.allocPrint(alloc, "/tmp/mychat_{s}_{d}_{d}.fifo", .{ role, pid, ts });
}

fn hostHandleFrame(alloc: std.mem.Allocator, clients: *std.StringHashMap(Client), frame: []const u8) !void {
    var iter = std.mem.splitScalar(u8, frame, common.US);

    const kind = iter.next() orelse return error.MalformedFrame;
    if (std.mem.eql(u8, kind, "JOIN")) {
        const name = try alloc.dupe(u8, iter.next() orelse return error.MalformedFrame);
        errdefer alloc.free(name);
        const data_fifo = iter.next() orelse return error.MalformedFrame;

        if (try clients.fetchPut(name, try Client.init(alloc, data_fifo))) |old_client| {
            std.log.warn("Kicking off old client: {s}", .{old_client.key});
            alloc.free(old_client.key);
            var old_value = old_client.value;
            old_value.deinit();
        }
        std.log.info("Joined: {s}", .{name});
        const bcast_msg = try common.allocJoinFrame(alloc, name, "<redacted>");
        defer alloc.free(bcast_msg);
        broadcastIgnorant(clients, bcast_msg);
    } else if (std.mem.eql(u8, kind, "MSG")) {
        const name = iter.next() orelse return error.MalformedFrame;
        const msg = iter.next() orelse return error.MalformedFrame;
        if (clients.getEntry(name) != null) {
            std.log.info("[{s}] {s}", .{ name, msg });
            const bcast_msg = try common.allocMsgFrame(alloc, name, msg);
            defer alloc.free(bcast_msg);
            broadcastIgnorant(clients, bcast_msg);
        } else {
            std.log.warn("Message from unregistered client: {s}", .{name});
        }
    } else if (std.mem.eql(u8, kind, "LEAVE")) {
        const name = iter.next() orelse return error.MalformedFrame;
        if (clients.fetchRemove(name)) |client| {
            std.log.info("Left: {s}", .{name});
            alloc.free(client.key);
            var value = client.value;
            value.deinit();
            const bcast_msg = try common.allocLeaveFrame(alloc, name);
            defer alloc.free(bcast_msg);
            broadcastIgnorant(clients, bcast_msg);
        } else {
            std.log.warn("Received LEAVE for non-existent client: {s}", .{name});
        }
    }
    if (iter.next() != null) {
        return error.MalformedFrame;
    }
}

// Don't try to find zombies.
fn broadcastIgnorant(clients: *const std.StringHashMap(Client), buffer: []const u8) void {
    var iter = clients.valueIterator();
    while (iter.next()) |client| {
        writeToPath(client.data_fifo, buffer) catch {};
    }
}

// Broadcast and reap newly-found zombies.
fn broadcast(alloc: std.mem.Allocator, clients: *std.StringHashMap(Client), buffer: []const u8) !void {
    var zombies = std.ArrayList([]u8){};
    defer {
        for (zombies.items) |name| alloc.free(name);
        zombies.deinit(alloc);
    }

    var iter = clients.iterator();
    while (iter.next()) |entry| {
        const name = entry.key_ptr.*;
        const data_fifo = entry.value_ptr.data_fifo;
        writeToPath(data_fifo, buffer) catch try zombies.append(alloc, try alloc.dupe(u8, name));
    }

    // Reap zombies.
    for (zombies.items) |name| {
        if (clients.remove(name)) {
            std.log.warn("Reaped zombie client: {s}", .{name});
        }
        const msg = common.allocLeaveFrame(alloc, name) catch continue;
        defer alloc.free(msg);
        broadcastIgnorant(clients, msg);
    }
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

    var reader_buf: [common.MAX_FRAME_LEN]u8 = undefined;
    var ctrl_fifo_reader = ctrl_fifo.reader(&reader_buf);
    const ctrl_fifo_io_reader = &ctrl_fifo_reader.interface;
    while (true) {
        const frame = try ctrl_fifo_io_reader.takeDelimiter(common.RS) orelse continue;
        if (frame.len == 0) continue;
        hostHandleFrame(alloc, &clients, frame) catch |err| {
            std.log.warn("Cannot handle frame: {}. raw={x})", .{ err, frame });
            continue;
        };
    }
}

fn clientRecvLoop(ctx: RecvCtx) void {
    const recv = ctx.recv;
    defer recv.close();

    var reader_buf: [common.MAX_FRAME_LEN]u8 = undefined;
    var recv_reader = recv.reader(&reader_buf);
    const recv_io_reader = &recv_reader.interface;
    while (recv_io_reader.takeDelimiter(common.RS)) |maybe_line| {
        const line = maybe_line orelse continue;
        if (line.len == 0) continue;
        std.log.info("{s}", .{line});
    } else |_| return;
}

pub fn runClient(alloc: std.mem.Allocator, ctrl_fifo_path: []const u8, name: []const u8) !void {
    // Set a global flag when receiving SIGINT for shutdown
    var sigint_ctx = SigIntCtx{ .flag = std.atomic.Value(bool).init(false) };
    g_sigint_ctx = &sigint_ctx;
    defer g_sigint_ctx = null;
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

    const recv = try std.fs.openFileAbsolute(data_fifo_path, .{ .mode = .read_only });
    const recv_thread = try std.Thread.spawn(.{}, clientRecvLoop, .{RecvCtx{ .recv = recv }});
    defer recv_thread.detach();

    const stdin = std.fs.File.stdin();
    var stdin_reader_buf: [common.MAX_FRAME_LEN]u8 = undefined;
    var stdin_reader = stdin.reader(&stdin_reader_buf);
    var stdin_io_reader = &stdin_reader.interface;
    while (!sigint_ctx.flag.load(.monotonic)) {
        // Stdin has data?
        var fds = [_]std.posix.pollfd{.{ .fd = std.posix.STDIN_FILENO, .events = std.posix.POLL.IN, .revents = 0 }};
        const ready = std.posix.poll(&fds, CHECK_SIGINT_INTERVAL) catch break;
        if (ready == 0) continue; // timeout, check if interrupted.
        if (fds[0].revents & std.posix.POLL.IN == 0) continue;

        const maybe_line = stdin_io_reader.takeDelimiter('\n') catch break;
        const line = maybe_line orelse continue;
        if (line.len == 0) continue;
        try sendMsg(alloc, ctrl_fifo_path, name, line);
    }
}
