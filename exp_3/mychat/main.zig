const std = @import("std");
const clap = @import("clap");

const fifo = @import("fifo.zig");
const mq = @import("mq.zig");
const shmem = @import("shmem.zig");

const MAX_NAME_LEN: comptime_int = 32;

const Mode = enum { fifo, mq, shmem };

fn printHelp(params: anytype) !void {
    return clap.helpToFile(.stderr(), clap.Help, params, .{});
}

pub fn main() !void {
    const alloc = std.heap.c_allocator;

    const gpa = try std.process.argsAlloc(alloc);
    defer std.process.argsFree(alloc, gpa);

    const params = comptime clap.parseParamsComptime(
        \\-h,--help                 Display this help and exit.
        \\-H,--host                 Run as host. Cannot be used with '--client'.
        \\-C,--client <str>         Run as client and connect to the host. Cannot be used with '--host'.
        \\-m,--mode <str>           Connection mode. One of 'fifo', 'mq', and 'shmem'.
        \\-n,--name <str>           [Optional] Name to use. Max length: 32.
        \\
    );

    var diag = clap.Diagnostic{};
    var res = clap.parse(clap.Help, &params, clap.parsers.default, .{
        .diagnostic = &diag,
        .allocator = alloc,
    }) catch |err| {
        // Report useful error and exit.
        try diag.reportToFile(.stderr(), err);
        return err;
    };
    defer res.deinit();

    if (res.args.help != 0) return printHelp(&params);
    if (res.args.host != 0 and res.args.client != null) return printHelp(&params);
    if (res.args.host == 0 and res.args.client == null) return printHelp(&params);

    const mode_str = res.args.mode orelse {
        return printHelp(&params);
    };
    const mode = std.meta.stringToEnum(Mode, mode_str) orelse {
        return printHelp(&params);
    };

    const default_name = try std.fmt.allocPrint(alloc, "user-{d}", .{std.os.linux.getpid()});
    defer alloc.free(default_name);
    const name = res.args.name orelse default_name;
    if (name.len > MAX_NAME_LEN) return error.NameTooLong;

    if (res.args.host != 0) {
        switch (mode) {
            .fifo => try fifo.runHost(alloc, name),
            .mq => try mq.runHost(alloc, name),
            .shmem => try shmem.runHost(alloc, name),
        }
    } else {
        switch (mode) {
            .fifo => try fifo.runClient(alloc, res.args.client.?, name),
            .mq => try mq.runClient(alloc, res.args.client.?, name),
            .shmem => try shmem.runClient(alloc, res.args.client.?, name),
        }
    }
}
