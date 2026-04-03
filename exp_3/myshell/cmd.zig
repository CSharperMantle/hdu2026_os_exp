const std = @import("std");

pub const Word = []u8;

pub const RedirType = enum {
    file,
    dup2,
};

pub const RedirFileType = enum {
    input,
    output,
    append,
};

pub const RedirFile = struct {
    type: RedirFileType,
    fd: u32,
    target: []u8,
};

pub const RedirDup2 = struct {
    src_fd: u32,
    dst_fd: u32,
};

pub const Redir = union(RedirType) {
    file: RedirFile,
    dup2: RedirDup2,
};

pub const Command = struct {
    // The command name and its arguments. The first element is the command name.
    argv: std.ArrayList(Word),
    // A list of redirections to apply before executing the command. The order matters.
    redirs: std.ArrayList(Redir),

    pub fn init() Command {
        return .{
            .argv = .empty,
            .redirs = .empty,
        };
    }

    pub fn deinit(self: *Command, alloc: std.mem.Allocator) void {
        for (self.argv.items) |word| alloc.free(word);
        for (self.redirs.items) |redir| switch (redir) {
            .file => |file| alloc.free(file.target),
            .dup2 => {},
        };
        self.argv.deinit(alloc);
        self.redirs.deinit(alloc);
    }
};

pub const Pipeline = struct {
    commands: std.ArrayList(Command),

    pub fn init() Pipeline {
        return .{ .commands = .empty };
    }

    pub fn deinit(self: *Pipeline, alloc: std.mem.Allocator) void {
        for (self.commands.items) |*command| command.deinit(alloc);
        self.commands.deinit(alloc);
    }
};
