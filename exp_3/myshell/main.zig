const builtin = @import("builtin");
const std = @import("std");
const isocline = @import("isocline");

const cmd = @import("cmd.zig");
const parse = @import("parse.zig");

fn exec(alloc: std.mem.Allocator, command: cmd.Command, prev_read: ?std.posix.fd_t, pipefds: ?[2]std.posix.fd_t) !noreturn {
    if (prev_read) |fd| {
        // Read from previous stage
        try std.posix.dup2(fd, std.posix.STDIN_FILENO);
        std.posix.close(fd);
    }
    if (pipefds) |fds| {
        // Write to next stage
        try std.posix.dup2(fds[1], std.posix.STDOUT_FILENO);
        std.posix.close(fds[0]);
        std.posix.close(fds[1]);
    }

    for (command.redirs.items) |redir| switch (redir) {
        .dup2 => |dup| try std.posix.dup2(@intCast(dup.dst_fd), @intCast(dup.src_fd)),
        .file => |file| {
            const flags: std.posix.O = switch (file.type) {
                .input => .{ .ACCMODE = .RDONLY },
                .output => .{ .ACCMODE = .WRONLY, .CREAT = true, .TRUNC = true },
                .append => .{ .ACCMODE = .WRONLY, .CREAT = true, .APPEND = true },
            };
            const fd = try std.posix.open(file.target, flags, 0o644);
            defer std.posix.close(fd);
            try std.posix.dup2(fd, @intCast(file.fd));
        },
    };

    const argv_z = try alloc.allocSentinel(?[*:0]const u8, command.argv.items.len, null);
    defer alloc.free(argv_z);
    for (command.argv.items, 0..) |arg, i| {
        argv_z[i] = (try alloc.dupeZ(u8, arg)).ptr;
    }
    defer for (argv_z) |arg| if (arg) alloc.free(arg);

    try std.posix.execvpeZ(argv_z[0].?, argv_z.ptr, std.c.environ);

    unreachable;
}

fn executePipeline(alloc: std.mem.Allocator, pipeline: *const cmd.Pipeline) !void {
    const stderr = std.fs.File.stderr().writer(&.{});

    if (pipeline.commands.items.len == 0) return;

    var pids = try alloc.alloc(std.posix.pid_t, pipeline.commands.items.len);
    defer alloc.free(pids);

    var prev_read: ?std.posix.fd_t = null;
    for (pipeline.commands.items, 0..) |command, idx| {
        const argv0 = if (command.argv.items.len > 0) command.argv.items[0] else "";

        const pipefd = if (idx != pipeline.commands.items.len - 1) try std.posix.pipe() else null;
        errdefer if (pipefd) |fds| {
            std.posix.close(fds[0]);
            std.posix.close(fds[1]);
        };

        const pid = try std.posix.fork();
        if (pid == 0) {
            exec(alloc, command, prev_read, pipefd) catch |err| {
                stderr.print("{s}: {s}\n", .{ argv0, @errorName(err) });
                std.posix.exit(127);
            };
            unreachable;
        }
        pids[idx] = pid;

        if (prev_read) |fd| std.posix.close(fd);
        if (pipefd) |fds| {
            std.posix.close(fds[1]);
            prev_read = fds[0];
        } else {
            prev_read = null;
        }
    }

    if (prev_read) |fd| std.posix.close(fd);

    for (pids) |pid| {
        _ = std.posix.waitpid(pid, 0);
    }
}

pub fn main() !void {
    var gpa: std.heap.DebugAllocator(.{}) = .init;
    defer {
        const status = gpa.deinit();
        if (status != .ok) {
            @panic("Memory leak detected");
        }
    }
    const alloc = gpa.allocator();

    const stderr = std.fs.File.stderr().writer(&.{});

    const argv = try std.process.argsAlloc(alloc);
    const argv0 = if (argv.len > 0) argv[0] else "";

    isocline.setHistory(null, -1);
    isocline.setPromptMarker("> ", ". ");

    while (isocline.readline("demo")) |line| {
        const input = std.mem.span(line);
        if (std.mem.trim(u8, input, &std.ascii.whitespace).len == 0) continue;

        const owned = try alloc.dupe(u8, input);
        defer alloc.free(owned);

        var pipeline = parse.parse(alloc, owned) catch |err| {
            stderr.print("{s}: Syntax error: {s}\n", .{ argv0, @errorName(err) }) catch {};
            continue;
        };
        defer pipeline.deinit(alloc);

        executePipeline(alloc, &pipeline) catch |err| {
            stderr.print("{s}\n", .{@errorName(err)}) catch {};
        };
    }
}
