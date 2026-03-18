const std = @import("std");
const isocline = @import("isocline");

pub fn main() !void {
    while (isocline.readline("demo")) |line| {
        try std.fs.File.stdout().writeAll(std.mem.span(line));
        try std.fs.File.stdout().writeAll("\n");
    }
}
