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

    return std.posix.execvpeZ(argv_z[0].?, argv_z.ptr, std.c.environ);
}

fn isBuiltin(argv0: []const u8) bool {
    return std.mem.eql(u8, argv0, "cd") or std.mem.eql(u8, argv0, "pwd") or std.mem.eql(u8, argv0, "exit");
}

fn exitOr(status: u8, no_exit: bool) u8 {
    if (no_exit) {
        return status;
    } else {
        std.posix.exit(status);
    }
}

fn execBuiltin(log: *std.io.Writer, command: *const cmd.Command, in_parent: bool) u8 {
    const name = command.argv.items[0];

    if (std.mem.eql(u8, name, "cd")) {
        const env_home = std.posix.getenv("HOME") orelse ".";
        const target = if (command.argv.items.len > 1)
            command.argv.items[1]
        else
            std.mem.span(@as([*:0]const u8, env_home));
        std.posix.chdir(target) catch |err| {
            log.print("cd: {s}: {s}\n", .{ target, @errorName(err) }) catch {};
            return exitOr(1, in_parent);
        };
        return exitOr(0, in_parent);
    } else if (std.mem.eql(u8, name, "pwd")) {
        var cwd_buf: [std.posix.PATH_MAX]u8 = undefined;
        const cwd = std.posix.getcwd(&cwd_buf) catch |err| {
            log.print("pwd: {s}", .{@errorName(err)}) catch {};
            return exitOr(1, in_parent);
        };
        var stdout_writer = std.fs.File.stdout().writer(&.{});
        stdout_writer.interface.writeAll(cwd) catch |err| {
            log.print("pwd: {s}", .{@errorName(err)}) catch {};
            return exitOr(1, in_parent);
        };
        stdout_writer.interface.writeAll("\n") catch |err| {
            log.print("pwd: {s}", .{@errorName(err)}) catch {};
            return exitOr(1, in_parent);
        };
        return exitOr(0, in_parent);
    } else if (std.mem.eql(u8, name, "exit")) {
        const code: u8 = if (command.argv.items.len == 2)
            std.fmt.parseUnsigned(u8, command.argv.items[1], 10) catch 0
        else
            0;
        std.posix.exit(code);
    } else {
        @panic("Unknown builtin");
    }
}

fn executePipeline(alloc: std.mem.Allocator, log: *std.io.Writer, pipeline: *const cmd.Pipeline) !void {
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
            if (isBuiltin(argv0)) {
                _ = execBuiltin(log, &command, false);
            } else {
                exec(alloc, command, prev_read, pipefd) catch |err| {
                    log.print("{s}: {s}\n", .{ argv0, @errorName(err) }) catch {};
                    std.posix.exit(127);
                };
            }
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
    const alloc = std.heap.page_allocator;

    var stderr_writer = std.fs.File.stdout().writer(&.{});
    const stderr: *std.io.Writer = &stderr_writer.interface;

    const argv = try std.process.argsAlloc(alloc);
    const argv0 = if (argv.len > 0) argv[0] else "";

    isocline.setHistory(null, -1);
    isocline.setPromptMarker("[white]>[/white] ", "[gray].[/gray] ");
    isocline.styleDef("ic-prompt", "yellow");

    var cwd_buf: [std.posix.PATH_MAX]u8 = undefined;
    var cwd = std.posix.getcwd(&cwd_buf) catch "";
    var cwdz = alloc.dupeZ(u8, cwd) catch |err| {
        stderr.print("{s}: {s}", .{ argv0, @errorName(err) }) catch {};
        std.posix.exit(128 + @as(comptime_int, @intFromEnum(std.posix.E.NOMEM)));
    };
    defer alloc.free(cwdz);

    while (isocline.readline(cwdz)) |line| {
        const input = std.mem.span(line);
        if (std.mem.trim(u8, input, &std.ascii.whitespace).len == 0) continue;

        const owned = try alloc.dupe(u8, input);
        defer alloc.free(owned);

        var pipeline = parse.parse(alloc, owned) catch |err| {
            stderr.print("{s}: Syntax error: {s}\n", .{ argv0, @errorName(err) }) catch {};
            continue;
        };
        defer pipeline.deinit(alloc);

        if (pipeline.commands.items.len == 1 and pipeline.commands.items[0].argv.items.len >= 1 and isBuiltin(pipeline.commands.items[0].argv.items[0])) {
            _ = execBuiltin(stderr, &pipeline.commands.items[0], true);
        } else {
            executePipeline(alloc, stderr, &pipeline) catch |err| {
                stderr.print("{s}\n", .{@errorName(err)}) catch {};
            };
        }

        cwd = std.posix.getcwd(&cwd_buf) catch "";
        alloc.free(cwdz);
        cwdz = alloc.dupeZ(u8, cwd) catch |err| {
            stderr.print("{s}: {s}", .{ argv0, @errorName(err) }) catch {};
            std.posix.exit(128 + @as(comptime_int, @intFromEnum(std.posix.E.NOMEM)));
        };
    }
}
