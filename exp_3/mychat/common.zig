const std = @import("std");

pub const MAX_FRAME_LEN: comptime_int = 4096;

pub const US: comptime_int = '\x1f';
pub const RS: comptime_int = '\x1e';

pub fn allocJoinFrame(alloc: std.mem.Allocator, name: []const u8, info: []const u8) ![]u8 {
    const buf = try alloc.alloc(u8, MAX_FRAME_LEN);
    return std.fmt.bufPrint(buf, "JOIN{c}{s}{c}{s}{c}", .{ US, name, US, info, RS });
}

pub fn allocLeaveFrame(alloc: std.mem.Allocator, name: []const u8) ![]u8 {
    const buf = try alloc.alloc(u8, MAX_FRAME_LEN);
    return std.fmt.bufPrint(buf, "LEAVE{c}{s}{c}", .{ US, name, RS });
}

pub fn allocMsgFrame(alloc: std.mem.Allocator, name: []const u8, msg: []const u8) ![]u8 {
    const buf = try alloc.alloc(u8, MAX_FRAME_LEN);
    return std.fmt.bufPrint(buf, "MSG{c}{s}{c}{s}{c}", .{ US, name, US, msg, RS });
}

pub fn allocColorizeMetaTag(alloc: std.mem.Allocator, content: []const u8) ![]u8 {
    return std.fmt.allocPrint(alloc, "\x1b[0;93;49m{s}\x1b[0m", .{content});
}
pub fn allocColorizeUsername(alloc: std.mem.Allocator, content: []const u8) ![]u8 {
    return std.fmt.allocPrint(alloc, "\x1b[0;96;49m{s}\x1b[0m", .{content});
}
