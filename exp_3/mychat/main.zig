const std = @import("std");
const clap = @import("clap");

pub fn main() !void {
    const alloc = std.heap.page_allocator;

    const gpa = try std.process.argsAlloc(alloc);
    defer std.process.argsFree(alloc, gpa);

    const params = comptime clap.parseParamsComptime(
        \\-h, --help                Display this help and exit.
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

    if (res.args.help != 0) {
        return clap.helpToFile(.stderr(), clap.Help, &params, .{});
    }
}
