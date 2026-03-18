const std = @import("std");

const cmd = @import("cmd.zig");

pub const ParseError = error{
    UnterminatedQuote,
    MissingRedirectionTarget,
    UnexpectedRedirection,
    EmptyCommand,
};

pub const ParseResultError = std.mem.Allocator.Error || ParseError;

/// Eat all whitespace starting at `index`.
fn skipWhitespace(line: []const u8, index: *usize) void {
    while (index.* < line.len and std.ascii.isWhitespace(line[index.*])) : (index.* += 1) {}
}

/// Parses a single file descriptor (decimal).
fn parseFd(text: []const u8) u32 {
    return std.fmt.parseUnsigned(u32, text, 10) catch unreachable;
}

/// Parses an optional leading FD before a redirection operator.
///
/// * `2>file` -> `{ .fd = 2, .end = 1 }`
/// * `>file` -> `{ .fd = null, .end = 0 }`
fn parseLeadingFd(line: []const u8, start: usize) struct { fd: ?u32, end: usize } {
    if (start >= line.len or !std.ascii.isDigit(line[start])) {
        return .{ .fd = null, .end = start };
    }
    var j = start;
    while (j < line.len and std.ascii.isDigit(line[j])) : (j += 1) {}
    if (j >= line.len or (line[j] != '<' and line[j] != '>')) {
        return .{ .fd = null, .end = start };
    }
    return .{ .fd = parseFd(line[start..j]), .end = j };
}

/// Parses a file descriptor token after `&` in a redirection, e.g. `2>&1`.
fn parseFdToken(line: []const u8, start: usize) ?struct { fd: u32, end: usize } {
    var j = start;
    while (j < line.len and std.ascii.isDigit(line[j])) : (j += 1) {}
    if (j == start) return null;
    return .{ .fd = parseFd(line[start..j]), .end = j };
}

/// Parses a generic word, handling quotes.
fn parseWord(allocator: std.mem.Allocator, line: []const u8, index: *usize) ParseResultError![]u8 {
    var out: std.ArrayList(u8) = .empty;
    errdefer out.deinit(allocator);

    const QuoteState = enum { none, single, double };
    var quote: QuoteState = .none;

    while (index.* < line.len) : (index.* += 1) {
        const ch = line[index.*];
        switch (quote) {
            .none => switch (ch) {
                ' ', '\t', '\n', '\r', '|', '<', '>' => break,
                '\'' => quote = .single,
                '"' => quote = .double,
                else => try out.append(allocator, ch),
            },
            .single => switch (ch) {
                '\'' => quote = .none,
                else => try out.append(allocator, ch),
            },
            .double => switch (ch) {
                '"' => quote = .none,
                else => try out.append(allocator, ch),
            },
        }
    }

    if (quote != .none) return error.UnterminatedQuote;
    if (out.items.len == 0) return error.UnexpectedRedirection;
    return out.toOwnedSlice(allocator);
}

pub fn parse(allocator: std.mem.Allocator, line: []u8) ParseResultError!cmd.Pipeline {
    var pipeline = cmd.Pipeline.init(allocator);
    errdefer pipeline.deinit(allocator);

    var current = cmd.Command.init(allocator);
    errdefer current.deinit(allocator);

    var i: usize = 0;
    var pending_redir: ?struct { fd: u32, kind: cmd.RedirFileType } = null;

    while (true) {
        skipWhitespace(line, &i);
        if (i >= line.len) break;

        // Finalize a pending redirection.
        if (pending_redir != null) {
            const target = try parseWord(allocator, line, &i);
            try current.redirs.append(allocator, .{ .file = .{
                .type = pending_redir.?.kind,
                .fd = pending_redir.?.fd,
                .target = target,
            } });
            pending_redir = null;
            continue;
        }

        // Handle pipes.
        if (line[i] == '|') {
            if (current.argv.items.len == 0) return error.EmptyCommand;
            try pipeline.commands.append(allocator, current);
            current = cmd.Command.init(allocator);
            i += 1;
            continue;
        }

        const maybe_src_fd = parseLeadingFd(line, i);
        const redir_start = maybe_src_fd.end;
        if (redir_start < line.len) {
            // Dup2?
            if (redir_start + 1 < line.len and std.mem.eql(u8, line[redir_start .. redir_start + 2], ">&")) {
                const dst_fd = parseFdToken(line, redir_start + 2) orelse {
                    return error.UnexpectedRedirection;
                };
                try current.redirs.append(allocator, .{
                    .dup2 = .{
                        .src_fd = maybe_src_fd.fd orelse 1, // `>&2` -> `1>&2`
                        .dst_fd = dst_fd.fd,
                    },
                });
                i = dst_fd.end;
                continue;
            }
            // Input file redirection?
            if (line[redir_start] == '<') {
                pending_redir = .{ .fd = maybe_src_fd.fd orelse 0, .kind = .input };
                i = redir_start + 1;
                continue;
            }
            // Output file redirection?
            if (line[redir_start] == '>') {
                pending_redir = .{ .fd = maybe_src_fd.fd orelse 1, .kind = .output };
                if (redir_start + 1 < line.len and line[redir_start + 1] == '>') {
                    // `>>` - Append
                    pending_redir.?.kind = .append;
                    i = redir_start + 2;
                } else {
                    pending_redir.?.kind = .output;
                    i = redir_start + 1;
                }
                continue;
            }
        }
        // Regular word.
        const word = try parseWord(allocator, line, &i);
        try current.argv.append(allocator, word);
    }

    if (pending_redir != null) return error.MissingRedirectionTarget;
    if (current.argv.items.len != 0 or current.redirs.items.len != 0) {
        if (current.argv.items.len == 0) return error.EmptyCommand;
        try pipeline.commands.append(allocator, current);
    } else {
        current.deinit(allocator);
    }

    return pipeline;
}
